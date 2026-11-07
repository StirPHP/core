//! What scripts see of the project's classes: `class("App\\User")`, its members and what's written
//! in them. Only what the declarations say, and values only where they're written out literally:
//! nothing is run, and nothing is inferred from a body, which would make a class's members depend
//! on checking the code that uses them.

use stir_db::{Db, SourceFile, Workspace};
use stir_hir::name::Scope;
use stir_hir::{ClassKind, MethodDef, PropertyDef, Visibility};
use stir_script::rhai::{Array, Dynamic, Engine, INT, Map};
use stir_syntax::SyntaxKind::*;
use stir_syntax::{SyntaxNode, TextRange};
use stir_types::{Ty, TyKind};

use super::facade::TypeFacade;
use crate::Ctx;
use crate::model::ClassModel;

/// How deep a literal value is read, through the constants it names.
const MAX_DEPTH: usize = 8;

/// A class, as `class(name)` finds it.
#[derive(Clone)]
pub struct ClassFacade {
    /// Lowercase, without a leading `\`.
    key: String,
}

/// A method, as a class has it.
#[derive(Clone)]
pub struct MethodFacade {
    /// The class it's looked at on, which may have inherited it.
    on: String,
    /// The class declaring it.
    owner: String,
    name: String,
}

/// A property, as a class has it.
#[derive(Clone)]
pub struct PropertyFacade {
    on: String,
    owner: String,
    name: String,
}

fn workspace(db: &dyn Db) -> Option<Workspace> {
    Workspace::try_get(db)
}

/// Runs `f` with a model of the workspace this thread is checking.
fn with_ctx<R: Default>(f: impl FnOnce(&Ctx<'_>) -> R) -> R {
    stir_db::attached(|db| match workspace(db) {
        Some(ws) => f(&Ctx::new(db, ws)),
        None => R::default(),
    })
    .unwrap_or_default()
}

fn with_class<R: Default>(
    key: &str,
    f: impl for<'db> FnOnce(&Ctx<'db>, &ClassModel<'db>) -> R,
) -> R {
    with_ctx(|ctx| match ctx.class(key) {
        Some(c) => f(ctx, &c),
        None => R::default(),
    })
}

fn find_class(name: &str) -> Dynamic {
    let key = name.trim_start_matches('\\').to_ascii_lowercase();
    let known = with_ctx(|ctx| ctx.class(&key).is_some());
    if known {
        Dynamic::from(ClassFacade { key })
    } else {
        Dynamic::UNIT
    }
}

fn visibility(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
}

fn type_facade(ctx: &Ctx<'_>, ty: Ty) -> Dynamic {
    Dynamic::from(TypeFacade::new(ty, ctx.ws))
}

impl ClassFacade {
    fn name(&self) -> String {
        with_class(&self.key, |_, c| c.name.to_string())
    }

    fn kind(&self) -> &'static str {
        with_class(&self.key, |_, c| match c.kind() {
            ClassKind::Class => "class",
            ClassKind::Interface => "interface",
            ClassKind::Trait => "trait",
            ClassKind::Enum => "enum",
        })
    }

    fn parent(&self) -> Dynamic {
        with_class(&self.key, |ctx, c| match ctx.supers(c).parent.as_ref() {
            Some(p) => match p.kind() {
                TyKind::Class { name, .. } => name.as_str().into(),
                _ => Dynamic::UNIT,
            },
            None => Dynamic::UNIT,
        })
    }

    /// Is it `name`, or does it extend or implement it, or use it as a trait?
    fn is_subtype_of(&self, name: &str) -> bool {
        let wanted = name.trim_start_matches('\\');
        with_class(&self.key, |ctx, c| {
            ctx.ancestors(&c.self_ty()).iter().any(|a| {
                matches!(a.kind(), TyKind::Class { name, .. } if name.eq_ignore_ascii_case(wanted))
            })
        })
    }

    /// Each class it is, nearest first: itself, its traits, its parents and their traits, and
    /// the interfaces.
    fn ancestors(&self) -> Array {
        with_class(&self.key, |ctx, c| {
            ctx.ancestors(&c.self_ty())
                .iter()
                .filter_map(|a| match a.kind() {
                    TyKind::Class { name, .. } => Some(Dynamic::from(name.to_string())),
                    _ => None,
                })
                .collect()
        })
    }

    /// Its methods and those it inherits, the nearest declaration of each name first. With the
    /// `@method` tags of its PHPDoc, but not what scripts add.
    fn methods(&self) -> Array {
        with_class(&self.key, |ctx, c| {
            let mut seen = rustc_hash::FxHashSet::default();
            let mut out = Array::new();
            for a in ctx.ancestors(&c.self_ty()).iter() {
                let TyKind::Class { name, .. } = a.kind() else {
                    continue;
                };
                let Some(owner) = ctx.class(name) else {
                    continue;
                };
                for m in &owner.def.methods {
                    if seen.insert(m.name.to_ascii_lowercase()) {
                        out.push(Dynamic::from(MethodFacade {
                            on: self.key.clone(),
                            owner: owner.key.to_string(),
                            name: m.name.to_string(),
                        }));
                    }
                }
            }
            out
        })
    }

    fn method(&self, name: &str) -> Dynamic {
        with_class(&self.key, |ctx, c| {
            for a in ctx.ancestors(&c.self_ty()).iter() {
                let TyKind::Class { name: class, .. } = a.kind() else {
                    continue;
                };
                let Some(owner) = ctx.class(class) else {
                    continue;
                };
                if let Some(m) = owner
                    .def
                    .methods
                    .iter()
                    .find(|m| m.name.eq_ignore_ascii_case(name))
                {
                    return Dynamic::from(MethodFacade {
                        on: self.key.clone(),
                        owner: owner.key.to_string(),
                        name: m.name.to_string(),
                    });
                }
            }
            Dynamic::UNIT
        })
    }

    /// Its properties and those it inherits, with constructor-promoted ones and the `@property`
    /// tags of its PHPDoc, but not what scripts add.
    fn properties(&self) -> Array {
        with_class(&self.key, |ctx, c| {
            let mut seen = rustc_hash::FxHashSet::default();
            let mut out = Array::new();
            for a in ctx.ancestors(&c.self_ty()).iter() {
                let TyKind::Class { name, .. } = a.kind() else {
                    continue;
                };
                let Some(owner) = ctx.class(name) else {
                    continue;
                };
                for name in property_names(owner.def) {
                    if seen.insert(name.to_string()) {
                        out.push(Dynamic::from(PropertyFacade {
                            on: self.key.clone(),
                            owner: owner.key.to_string(),
                            name: name.to_string(),
                        }));
                    }
                }
            }
            out
        })
    }

    fn property(&self, name: &str) -> Dynamic {
        let name = name.trim_start_matches('$');
        with_class(&self.key, |ctx, c| {
            for a in ctx.ancestors(&c.self_ty()).iter() {
                let TyKind::Class { name: class, .. } = a.kind() else {
                    continue;
                };
                let Some(owner) = ctx.class(class) else {
                    continue;
                };
                if property_names(owner.def).any(|p| p == name) {
                    return Dynamic::from(PropertyFacade {
                        on: self.key.clone(),
                        owner: owner.key.to_string(),
                        name: name.to_owned(),
                    });
                }
            }
            Dynamic::UNIT
        })
    }

    /// The value of the constant `name`, if it's written out literally.
    fn constant(&self, name: &str) -> Dynamic {
        with_ctx(|ctx| constant_value(ctx, &self.key, name, 0))
    }

    /// The attributes on the class, as `#{ name, args }`.
    fn attributes(&self) -> Array {
        with_class(&self.key, |ctx, c| {
            let Some(node) = node_of(ctx.db, c.file, c.def.ptr.text_range(), CLASS_DECL) else {
                return Array::new();
            };
            attributes_of(ctx, c, &node)
        })
    }

    fn self_type(&self) -> Dynamic {
        with_class(&self.key, |ctx, c| type_facade(ctx, c.self_ty()))
    }

    fn type_params(&self) -> Array {
        with_class(&self.key, |_, c| {
            c.params
                .iter()
                .map(|p| Dynamic::from(p.name.to_string()))
                .collect()
        })
    }
}

/// What a method of `owner` is seen in: a trait's, in the class using it.
fn context<'a, 'db>(owner: &'a ClassModel<'db>, on: &'a ClassModel<'db>) -> &'a ClassModel<'db> {
    if owner.kind() == ClassKind::Trait {
        on
    } else {
        owner
    }
}

fn property_names(def: &stir_hir::ClassDef) -> impl Iterator<Item = &str> {
    let promoted = def
        .methods
        .iter()
        .filter(|m| m.name.eq_ignore_ascii_case("__construct"))
        .flat_map(|m| m.sig.params.iter())
        .filter(|p| p.promoted.is_some())
        .map(|p| p.name.as_str());
    def.properties
        .iter()
        .map(|p| p.name.as_str())
        .chain(promoted)
}

impl MethodFacade {
    fn with<R: Default>(
        &self,
        f: impl for<'db> FnOnce(&Ctx<'db>, &ClassModel<'db>, &MethodDef, &ClassModel<'db>) -> R,
    ) -> R {
        with_ctx(|ctx| {
            let (Some(owner), Some(on)) = (ctx.class(&self.owner), ctx.class(&self.on)) else {
                return R::default();
            };
            let Some(m) = owner
                .def
                .methods
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(&self.name))
            else {
                return R::default();
            };
            f(ctx, &owner, m, &on)
        })
    }

    fn class_name(&self) -> String {
        with_ctx(|ctx| {
            ctx.class(&self.owner)
                .map_or_else(String::new, |c| c.name.to_string())
        })
    }

    /// What it says it returns, as seen on the class it was looked up on.
    fn return_type(&self) -> Dynamic {
        self.with(|ctx, owner, m, on| {
            let sig = ctx.method_sig_in(owner, m, context(owner, on));
            type_facade(ctx, sig.ret)
        })
    }

    fn params(&self) -> Array {
        self.with(|ctx, owner, m, on| {
            let sig = ctx.method_sig_in(owner, m, context(owner, on));
            sig.params
                .into_iter()
                .map(|p| {
                    let mut param = Map::new();
                    param.insert("name".into(), p.name.as_str().into());
                    param.insert("type".into(), type_facade(ctx, p.ty));
                    param.insert("optional".into(), p.optional.into());
                    param.insert("variadic".into(), p.variadic.into());
                    param.insert("by_ref".into(), p.by_ref.into());
                    Dynamic::from(param)
                })
                .collect()
        })
    }

    fn attributes(&self) -> Array {
        self.with(|ctx, owner, m, _| {
            if m.magic {
                return Array::new();
            }
            match node_of(ctx.db, owner.file, m.ptr.text_range(), METHOD_DECL) {
                Some(node) => attributes_of(ctx, owner, &node),
                None => Array::new(),
            }
        })
    }

    /// The expression of its `return`, if it has one: the last one in its body, whatever comes
    /// before.
    fn returned(&self, ctx: &Ctx<'_>, owner: &ClassModel<'_>, m: &MethodDef) -> Option<SyntaxNode> {
        if m.magic {
            return None;
        }
        let node = node_of(ctx.db, owner.file, m.ptr.text_range(), METHOD_DECL)?;
        let body = node.children().find(|c| c.kind() == BLOCK)?;
        let ret = body.children().filter(|c| c.kind() == RETURN_STMT).last()?;
        ret.children().next()
    }

    /// The value it returns, if that's written out literally.
    fn returned_value(&self) -> Dynamic {
        self.with(|ctx, owner, m, _| match self.returned(ctx, owner, m) {
            Some(expr) => literal(ctx, owner, &expr, 0),
            None => Dynamic::UNIT,
        })
    }

    /// The first call on `$this` that what it returns is made from, as `#{ name, args }`, such as
    /// the `hasMany` of `return $this->hasMany(Post::class)->latest();`. The arguments are their
    /// values where they're written out literally.
    fn returned_call(&self) -> Dynamic {
        self.with(|ctx, owner, m, _| {
            let Some(mut expr) = self.returned(ctx, owner, m) else {
                return Dynamic::UNIT;
            };
            // down the chain to the call whose receiver is `$this`
            loop {
                if expr.kind() != METHOD_CALL_EXPR {
                    return Dynamic::UNIT;
                }
                let Some(fetch) = expr.children().next() else {
                    return Dynamic::UNIT;
                };
                let Some(receiver) = fetch.children().next() else {
                    return Dynamic::UNIT;
                };
                let on_this = receiver.kind() == VARIABLE_EXPR
                    && receiver.text().to_string().trim() == "$this";
                if on_this {
                    let name = fetch
                        .children()
                        .find(|c| c.kind() == NAME)
                        .map(|n| n.text().to_string());
                    let Some(name) = name else {
                        return Dynamic::UNIT;
                    };
                    let mut call = Map::new();
                    call.insert("name".into(), name.into());
                    call.insert("args".into(), args_of(ctx, owner, &expr).into());
                    return call.into();
                }
                expr = receiver;
            }
        })
    }
}

impl PropertyFacade {
    fn with<R: Default>(
        &self,
        f: impl for<'db> FnOnce(&Ctx<'db>, &ClassModel<'db>, Option<&PropertyDef>) -> R,
    ) -> R {
        with_ctx(|ctx| {
            let Some(owner) = ctx.class(&self.owner) else {
                return R::default();
            };
            let def = owner.def.properties.iter().find(|p| p.name == self.name);
            f(ctx, &owner, def)
        })
    }

    fn class_name(&self) -> String {
        with_ctx(|ctx| {
            ctx.class(&self.owner)
                .map_or_else(String::new, |c| c.name.to_string())
        })
    }

    fn get_type(&self) -> Dynamic {
        with_ctx(|ctx| {
            let Some(on) = ctx.class(&self.on) else {
                return Dynamic::UNIT;
            };
            match ctx.find_property(&on.self_ty(), &self.name) {
                Some(p) => type_facade(ctx, p.ty),
                None => Dynamic::UNIT,
            }
        })
    }

    fn is_static(&self) -> bool {
        self.with(|_, _, p| p.is_some_and(|p| p.mods.is_static))
    }

    fn visibility(&self) -> &'static str {
        self.with(|_, _, p| visibility(p.map_or(Visibility::Public, |p| p.mods.visibility)))
    }

    /// Its default value, if that's written out literally.
    fn default(&self) -> Dynamic {
        self.with(|ctx, owner, p| {
            let Some(p) = p.filter(|p| p.has_default && !p.magic) else {
                return Dynamic::UNIT;
            };
            let Some(node) = node_of(ctx.db, owner.file, p.ptr.text_range(), PROPERTY_DECL) else {
                return Dynamic::UNIT;
            };
            // `public $a = 1, $b = 2;`
            let elem = node
                .children()
                .filter(|c| c.kind() == PROPERTY_ELEM)
                .find(|e| {
                    e.children_with_tokens()
                        .any(|t| t.kind() == VARIABLE && t.to_string() == format!("${}", self.name))
                });
            let value = elem.and_then(|e| e.children().last());
            match value {
                Some(v) => literal(ctx, owner, &v, 0),
                None => Dynamic::UNIT,
            }
        })
    }
}

/// The node of `kind` at `range` in `file`.
fn node_of(
    db: &dyn Db,
    file: SourceFile,
    range: TextRange,
    kind: stir_syntax::SyntaxKind,
) -> Option<SyntaxNode> {
    let root = stir_db::parse(db, file).syntax();
    let element = root.covering_element(range);
    let node = match element {
        stir_syntax::NodeOrToken::Node(n) => n,
        stir_syntax::NodeOrToken::Token(t) => t.parent()?,
    };
    node.ancestors().find(|n| n.kind() == kind)
}

/// The scope names in `node` are resolved in.
fn scope_at(ctx: &Ctx<'_>, file: SourceFile, node: &SyntaxNode) -> std::sync::Arc<Scope> {
    ctx.items(file).scope_at(node.text_range().start()).clone()
}

fn attributes_of<'db>(ctx: &Ctx<'db>, owner: &ClassModel<'db>, decl: &SyntaxNode) -> Array {
    let scope = scope_at(ctx, owner.file, decl);
    decl.children()
        .filter(|c| c.kind() == ATTRIBUTE_LIST)
        .flat_map(|l| l.descendants().filter(|d| d.kind() == ATTRIBUTE))
        .filter_map(|a| {
            let name = a.children().find(|c| c.kind() == NAME_REF)?;
            let name = scope.resolve_class(name.text().to_string().trim());
            let mut attribute = Map::new();
            attribute.insert("name".into(), name.as_str().into());
            attribute.insert("args".into(), args_of(ctx, owner, &a).into());
            Some(Dynamic::from(attribute))
        })
        .collect()
}

/// The arguments of a call or attribute, by position and, for named ones, by name: a map with
/// keys `"0"`, `"1"` and so on, and the names.
fn args_of<'db>(ctx: &Ctx<'db>, owner: &ClassModel<'db>, node: &SyntaxNode) -> Map {
    let mut out = Map::new();
    let Some(list) = node.children().find(|c| c.kind() == ARG_LIST) else {
        return out;
    };
    for (i, arg) in list
        .children()
        .filter(|c| matches!(c.kind(), ARG | NAMED_ARG))
        .enumerate()
    {
        let value = arg
            .children()
            .last()
            .map_or(Dynamic::UNIT, |v| literal(ctx, owner, &v, 0));
        if arg.kind() == NAMED_ARG
            && let Some(name) = arg.children().find(|c| c.kind() == NAME)
        {
            out.insert(name.text().to_string().trim().into(), value);
        } else {
            out.insert(i.to_string().into(), value);
        }
    }
    out
}

/// The value of a class constant, where it's written out literally.
fn constant_value(ctx: &Ctx<'_>, class: &str, name: &str, depth: usize) -> Dynamic {
    let Some(c) = ctx.class(class) else {
        return Dynamic::UNIT;
    };
    let ancestors = ctx.ancestors(&c.self_ty());
    for a in ancestors.iter() {
        let TyKind::Class { name: owner, .. } = a.kind() else {
            continue;
        };
        let Some(owner) = ctx.class(owner) else {
            continue;
        };
        if let Some(k) = owner.def.consts.iter().find(|k| k.name == name) {
            let Some(value) = &k.value else {
                return Dynamic::UNIT;
            };
            let root = stir_db::parse(ctx.db, owner.file).syntax();
            let node = value.to_node(&root);
            return literal(ctx, &owner, &node, depth + 1);
        }
    }
    Dynamic::UNIT
}

/// The value an expression in `owner`'s file is written as: strings, numbers, `true`, `false`,
/// `null`, arrays of them, `Foo::class` as the class's name, constants with such values, and the
/// default of `env('NAME', default)`. Anything else is `()`.
fn literal<'db>(
    ctx: &Ctx<'db>,
    owner: &ClassModel<'db>,
    node: &SyntaxNode,
    depth: usize,
) -> Dynamic {
    if depth > MAX_DEPTH {
        return Dynamic::UNIT;
    }
    match node.kind() {
        PAREN_EXPR => node
            .children()
            .next()
            .map_or(Dynamic::UNIT, |n| literal(ctx, owner, &n, depth)),
        STRING_LITERAL => {
            let text = node.text().to_string();
            let text = text.trim();
            if text.starts_with('"') && text.contains('$') {
                return Dynamic::UNIT;
            }
            stir_hir::type_ref::unquote(text).into()
        }
        LITERAL => {
            let text = node.text().to_string().replace('_', "");
            let text = text.trim();
            if let Ok(i) = text.parse::<INT>() {
                return i.into();
            }
            if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
                return INT::from_str_radix(hex, 16).map_or(Dynamic::UNIT, Dynamic::from);
            }
            text.parse::<f64>().map_or(Dynamic::UNIT, Dynamic::from)
        }
        PREFIX_EXPR => {
            let negative = node.children_with_tokens().any(|t| t.kind() == MINUS);
            let value = node
                .children()
                .next()
                .map_or(Dynamic::UNIT, |n| literal(ctx, owner, &n, depth));
            if !negative {
                return value;
            }
            if let Ok(i) = value.as_int() {
                return (-i).into();
            }
            value.as_float().map_or(Dynamic::UNIT, |f| (-f).into())
        }
        NAME_EXPR => match node.text().to_string().trim().to_ascii_lowercase().as_str() {
            "true" => true.into(),
            "false" => false.into(),
            _ => Dynamic::UNIT,
        },
        BIN_EXPR => {
            let is_concat = node.children_with_tokens().any(|t| t.kind() == DOT);
            let parts: Vec<SyntaxNode> = node.children().collect();
            if !is_concat || parts.len() != 2 {
                return Dynamic::UNIT;
            }
            let (a, b) = (
                literal(ctx, owner, &parts[0], depth + 1),
                literal(ctx, owner, &parts[1], depth + 1),
            );
            match (a.into_string(), b.into_string()) {
                (Ok(a), Ok(b)) => format!("{a}{b}").into(),
                _ => Dynamic::UNIT,
            }
        }
        ARRAY_EXPR => {
            let mut list = Array::new();
            let mut map = Map::new();
            let mut keyed = false;
            let mut next = 0;
            for elem in node.children().filter(|c| c.kind() == ARRAY_ELEM) {
                let parts: Vec<SyntaxNode> = elem.children().collect();
                let has_key = elem.children_with_tokens().any(|t| t.kind() == FAT_ARROW);
                // `...$spread` can't be known
                if elem.children_with_tokens().any(|t| t.kind() == ELLIPSIS) {
                    return Dynamic::UNIT;
                }
                let value = parts
                    .last()
                    .map_or(Dynamic::UNIT, |v| literal(ctx, owner, v, depth + 1));
                if has_key && parts.len() == 2 {
                    let key = literal(ctx, owner, &parts[0], depth + 1);
                    let key = if let Ok(i) = key.as_int() {
                        next = next.max(i + 1);
                        i.to_string()
                    } else if let Ok(s) = key.into_string() {
                        s
                    } else {
                        return Dynamic::UNIT;
                    };
                    keyed = true;
                    map.insert(key.into(), value.clone());
                } else {
                    map.insert(next.to_string().into(), value.clone());
                    next += 1;
                }
                list.push(value);
            }
            if keyed { map.into() } else { list.into() }
        }
        CLASS_CONST_FETCH_EXPR => {
            let Some(base) = node.children().next() else {
                return Dynamic::UNIT;
            };
            let Some(name) = node.children().find(|c| c.kind() == NAME) else {
                return Dynamic::UNIT;
            };
            let base_text = base.text().to_string();
            let base_text = base_text.trim();
            let class = match base_text.to_ascii_lowercase().as_str() {
                "self" | "static" => owner.name.to_string(),
                "parent" => match ctx.supers(owner).parent.as_ref().map(Ty::kind) {
                    Some(TyKind::Class { name, .. }) => name.to_string(),
                    _ => return Dynamic::UNIT,
                },
                _ if base.kind() == NAME_EXPR => scope_at(ctx, owner.file, node)
                    .resolve_class(base_text)
                    .as_str()
                    .to_owned(),
                _ => return Dynamic::UNIT,
            };
            let name = name.text().to_string();
            if name.trim().eq_ignore_ascii_case("class") {
                // the name as declared, where the class is known
                return ctx
                    .class(&class)
                    .map_or(class, |c| c.name.to_string())
                    .into();
            }
            constant_value(ctx, &class, name.trim(), depth + 1)
        }
        CALL_EXPR => {
            // what a configuration file gets when the variable isn't set
            let callee = node.children().next().map(|c| c.text().to_string());
            let is_env = callee.is_some_and(|c| {
                c.trim()
                    .trim_start_matches('\\')
                    .eq_ignore_ascii_case("env")
            });
            if !is_env {
                return Dynamic::UNIT;
            }
            let args = args_of(ctx, owner, node);
            args.get("1").cloned().unwrap_or(Dynamic::UNIT)
        }
        _ => Dynamic::UNIT,
    }
}

pub(super) fn register(engine: &mut Engine) {
    engine.register_fn("class", |name: &str| find_class(name));
    engine
        .register_type_with_name::<ClassFacade>("Class")
        .register_get("name", |c: &mut ClassFacade| c.name())
        .register_get("kind", |c: &mut ClassFacade| c.kind())
        .register_get("parent", |c: &mut ClassFacade| c.parent())
        .register_get("type", |c: &mut ClassFacade| c.self_type())
        .register_get("type_params", |c: &mut ClassFacade| c.type_params())
        .register_get("is_abstract", |c: &mut ClassFacade| {
            with_class(&c.key, |_, m| m.def.mods.is_abstract)
        })
        .register_fn("is_subtype_of", |c: &mut ClassFacade, n: &str| {
            c.is_subtype_of(n)
        })
        .register_fn("ancestors", |c: &mut ClassFacade| c.ancestors())
        .register_fn("methods", |c: &mut ClassFacade| c.methods())
        .register_fn("method", |c: &mut ClassFacade, n: &str| c.method(n))
        .register_fn("properties", |c: &mut ClassFacade| c.properties())
        .register_fn("property", |c: &mut ClassFacade, n: &str| c.property(n))
        .register_fn("constant", |c: &mut ClassFacade, n: &str| c.constant(n))
        .register_fn("attributes", |c: &mut ClassFacade| c.attributes());
    engine
        .register_type_with_name::<MethodFacade>("Method")
        .register_get("name", |m: &mut MethodFacade| m.name.clone())
        .register_get("class", |m: &mut MethodFacade| m.class_name())
        .register_get("is_static", |m: &mut MethodFacade| {
            m.with(|_, _, d, _| d.mods.is_static)
        })
        .register_get("is_magic", |m: &mut MethodFacade| {
            m.with(|_, _, d, _| d.magic)
        })
        .register_get("visibility", |m: &mut MethodFacade| {
            m.with(|_, _, d, _| visibility(d.mods.visibility))
        })
        .register_get("return_type", |m: &mut MethodFacade| m.return_type())
        .register_get("params", |m: &mut MethodFacade| m.params())
        .register_fn("attributes", |m: &mut MethodFacade| m.attributes())
        .register_fn("returned_value", |m: &mut MethodFacade| m.returned_value())
        .register_fn("returned_call", |m: &mut MethodFacade| m.returned_call());
    engine
        .register_type_with_name::<PropertyFacade>("Property")
        .register_get("name", |p: &mut PropertyFacade| p.name.clone())
        .register_get("class", |p: &mut PropertyFacade| p.class_name())
        .register_get("type", |p: &mut PropertyFacade| p.get_type())
        .register_get("is_static", |p: &mut PropertyFacade| p.is_static())
        .register_get("visibility", |p: &mut PropertyFacade| p.visibility())
        .register_get("default_value", |p: &mut PropertyFacade| p.default());
}
