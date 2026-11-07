//! What scripts see of the project's classes: `class("App\\User")`, its members and what's written
//! in them. Only what the declarations say, and values only where they're written out literally:
//! nothing is run, and nothing is inferred from a body, which would make a class's members depend
//! on checking the code that uses them.

use std::sync::{Arc, OnceLock};

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

/// A class, as `class(name)` finds it. What's cheap to know is known from the start, and its
/// members once asked for: each script call that asks reads them once.
#[derive(Clone)]
pub struct ClassFacade(Arc<ClassData>);

struct ClassData {
    /// Lowercase, without a leading `\`.
    key: String,
    name: String,
    kind: &'static str,
    is_abstract: bool,
    parent: Option<String>,
    /// Itself and every class it is, nearest first, as declared.
    ancestors: Vec<String>,
    type_params: Vec<String>,
    methods: OnceLock<Vec<MethodFacade>>,
    properties: OnceLock<Vec<PropertyFacade>>,
}

/// A method, as a class has it.
#[derive(Clone)]
pub struct MethodFacade(Arc<MethodData>);

struct MethodData {
    /// The class it's looked at on, which may have inherited it.
    on: String,
    /// The class declaring it, lowercase, and as declared.
    owner: String,
    owner_name: String,
    name: String,
    is_static: bool,
    is_magic: bool,
    visibility: &'static str,
}

/// A property, as a class has it.
#[derive(Clone)]
pub struct PropertyFacade(Arc<PropertyData>);

struct PropertyData {
    on: String,
    owner: String,
    owner_name: String,
    name: String,
    is_static: bool,
    visibility: &'static str,
}

fn workspace(db: &dyn Db) -> Option<Workspace> {
    Workspace::try_get(db)
}

/// Runs `f` with a model of the workspace this thread is checking.
pub(crate) fn with_ctx<R: Default>(f: impl FnOnce(&Ctx<'_>) -> R) -> R {
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
    let data = with_ctx(|ctx| {
        let c = ctx.class(&key)?;
        let parent = ctx.supers(&c).parent.as_ref().and_then(|p| match p.kind() {
            TyKind::Class { name, .. } => Some(name.to_string()),
            _ => None,
        });
        let ancestors = ctx
            .ancestors(&c.self_ty())
            .iter()
            .filter_map(|a| match a.kind() {
                TyKind::Class { name, .. } => Some(name.to_string()),
                _ => None,
            })
            .collect();
        Some(ClassData {
            key: key.clone(),
            name: c.name.to_string(),
            kind: match c.kind() {
                ClassKind::Class => "class",
                ClassKind::Interface => "interface",
                ClassKind::Trait => "trait",
                ClassKind::Enum => "enum",
            },
            is_abstract: c.def.mods.is_abstract,
            parent,
            ancestors,
            type_params: c.params.iter().map(|p| p.name.to_string()).collect(),
            methods: OnceLock::new(),
            properties: OnceLock::new(),
        })
    });
    match data {
        Some(data) => Dynamic::from(ClassFacade(Arc::new(data))),
        None => Dynamic::UNIT,
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
    /// Is it `name`, or does it extend or implement it, or use it as a trait?
    fn is_subtype_of(&self, name: &str) -> bool {
        let wanted = name.trim_start_matches('\\');
        self.0
            .ancestors
            .iter()
            .any(|a| a.eq_ignore_ascii_case(wanted))
    }

    /// Its methods and those it inherits, the nearest declaration of each name first. With the
    /// `@method` tags of its PHPDoc, but not what scripts add.
    fn methods(&self) -> &[MethodFacade] {
        self.0.methods.get_or_init(|| {
            with_ctx(|ctx| {
                let mut seen = rustc_hash::FxHashSet::default();
                let mut out = Vec::new();
                for a in &self.0.ancestors {
                    let Some(owner) = ctx.class(a) else { continue };
                    for m in &owner.def.methods {
                        if seen.insert(m.name.to_ascii_lowercase()) {
                            out.push(MethodFacade(Arc::new(MethodData {
                                on: self.0.key.clone(),
                                owner: owner.key.to_string(),
                                owner_name: owner.name.to_string(),
                                name: m.name.to_string(),
                                is_static: m.mods.is_static,
                                is_magic: m.magic,
                                visibility: visibility(m.mods.visibility),
                            })));
                        }
                    }
                }
                out
            })
        })
    }

    /// What there is to know of the methods named, all at once: much cheaper than asking each
    /// method in turn. Each is `#{ name, class, is_static, visibility, return_type, params,
    /// attributes, returned_call }`, for the ones the class has, in the order asked.
    fn method_details(&self, names: Array) -> Array {
        let methods: Vec<MethodFacade> = names
            .into_iter()
            .filter_map(|n| n.into_string().ok())
            .filter_map(|n| {
                self.methods()
                    .iter()
                    .find(|m| m.0.name.eq_ignore_ascii_case(&n))
                    .cloned()
            })
            .collect();
        with_ctx(|ctx| {
            methods
                .iter()
                .filter_map(|m| m.details(ctx))
                .map(Dynamic::from)
                .collect()
        })
    }

    /// [`method_details`](Self::method_details) of the methods the class itself declares and
    /// those it has from the classes and traits between it and `ancestor`: not those of `ancestor`,
    /// or what it's made of. The methods an application's model adds to Eloquent's, say.
    fn methods_below(&self, ancestor: &str) -> Array {
        let ancestor = ancestor.trim_start_matches('\\').to_ascii_lowercase();
        with_ctx(|ctx| {
            let Some(top) = ctx.class(&ancestor) else {
                return Array::new();
            };
            let above: rustc_hash::FxHashSet<String> = ctx
                .ancestors(&top.self_ty())
                .iter()
                .filter_map(|a| match a.kind() {
                    TyKind::Class { name, .. } => Some(name.to_ascii_lowercase()),
                    _ => None,
                })
                .collect();
            let mut seen = rustc_hash::FxHashSet::default();
            let mut out = Array::new();
            for a in &self.0.ancestors {
                if above.contains(&a.to_ascii_lowercase()) {
                    continue;
                }
                let Some(owner) = ctx.class(a) else { continue };
                for m in &owner.def.methods {
                    if !seen.insert(m.name.to_ascii_lowercase()) {
                        continue;
                    }
                    let facade = MethodFacade(Arc::new(MethodData {
                        on: self.0.key.clone(),
                        owner: owner.key.to_string(),
                        owner_name: owner.name.to_string(),
                        name: m.name.to_string(),
                        is_static: m.mods.is_static,
                        is_magic: m.magic,
                        visibility: visibility(m.mods.visibility),
                    }));
                    out.extend(facade.details(ctx).map(Dynamic::from));
                }
            }
            out
        })
    }

    fn method(&self, name: &str) -> Dynamic {
        self.methods()
            .iter()
            .find(|m| m.0.name.eq_ignore_ascii_case(name))
            .map_or(Dynamic::UNIT, |m| Dynamic::from(m.clone()))
    }

    /// Its properties and those it inherits, with constructor-promoted ones and the `@property`
    /// tags of its PHPDoc, but not what scripts add.
    fn properties(&self) -> &[PropertyFacade] {
        self.0.properties.get_or_init(|| {
            with_ctx(|ctx| {
                let mut seen = rustc_hash::FxHashSet::default();
                let mut out = Vec::new();
                for a in &self.0.ancestors {
                    let Some(owner) = ctx.class(a) else { continue };
                    for (name, is_static, vis) in property_names(owner.def) {
                        if seen.insert(name.to_owned()) {
                            out.push(PropertyFacade(Arc::new(PropertyData {
                                on: self.0.key.clone(),
                                owner: owner.key.to_string(),
                                owner_name: owner.name.to_string(),
                                name: name.to_owned(),
                                is_static,
                                visibility: visibility(vis),
                            })));
                        }
                    }
                }
                out
            })
        })
    }

    fn property(&self, name: &str) -> Dynamic {
        let name = name.trim_start_matches('$');
        self.properties()
            .iter()
            .find(|p| p.0.name == name)
            .map_or(Dynamic::UNIT, |p| Dynamic::from(p.clone()))
    }

    /// The value of the constant `name`, if it's written out literally.
    fn constant(&self, name: &str) -> Dynamic {
        with_ctx(|ctx| constant_value(ctx, &self.0.key, name, 0))
    }

    /// The attributes on the class, as `#{ name, args }`.
    fn attributes(&self) -> Array {
        with_class(&self.0.key, |ctx, c| {
            let Some(node) = node_of(ctx.db, c.file, c.def.ptr.text_range(), CLASS_DECL) else {
                return Array::new();
            };
            attributes_of(ctx, c, &node)
        })
    }

    fn self_type(&self) -> Dynamic {
        with_class(&self.0.key, |ctx, c| type_facade(ctx, c.self_ty()))
    }
}

/// The class a written type names, if it's one class and maybe `null`, and whether it has type
/// arguments.
fn class_named(t: &stir_hir::type_ref::TypeRef) -> Option<(String, bool)> {
    use stir_hir::type_ref::TypeRefKind;
    match &t.kind {
        TypeRefKind::Named(fqn, args) => Some((fqn.as_str().to_owned(), !args.is_empty())),
        TypeRefKind::Nullable(inner) => class_named(inner),
        TypeRefKind::Union(members) => {
            let mut found = members.iter().filter_map(class_named);
            let one = found.next();
            if found.next().is_some() { None } else { one }
        }
        _ => None,
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

fn property_names(def: &stir_hir::ClassDef) -> impl Iterator<Item = (&str, bool, Visibility)> {
    let promoted = def
        .methods
        .iter()
        .filter(|m| m.name.eq_ignore_ascii_case("__construct"))
        .flat_map(|m| m.sig.params.iter())
        .filter_map(|p| {
            let promoted = p.promoted.as_ref()?;
            Some((p.name.as_str(), false, promoted.mods.visibility))
        });
    def.properties
        .iter()
        .map(|p| (p.name.as_str(), p.mods.is_static, p.mods.visibility))
        .chain(promoted)
}

impl MethodFacade {
    fn with_in<'db, R: Default>(
        &self,
        ctx: &Ctx<'db>,
        f: impl FnOnce(&Ctx<'db>, &ClassModel<'db>, &MethodDef, &ClassModel<'db>) -> R,
    ) -> R {
        let (Some(owner), Some(on)) = (ctx.class(&self.0.owner), ctx.class(&self.0.on)) else {
            return R::default();
        };
        let Some(m) = owner
            .def
            .methods
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(&self.0.name))
        else {
            return R::default();
        };
        f(ctx, &owner, m, &on)
    }

    /// Everything [`ClassFacade::method_details`] gives of it: what can be known without
    /// working out its types, which `method` has.
    fn details(&self, ctx: &Ctx<'_>) -> Option<Map> {
        let mut out = Map::new();
        out.insert("name".into(), self.0.name.as_str().into());
        out.insert("class".into(), self.0.owner_name.as_str().into());
        out.insert("is_static".into(), self.0.is_static.into());
        out.insert("is_magic".into(), self.0.is_magic.into());
        out.insert("visibility".into(), self.0.visibility.into());
        out.insert("method".into(), Dynamic::from(self.clone()));
        let (written, required, count) = self.with_in(ctx, |_, _, m, _| {
            let written = m
                .sig
                .doc_ret
                .as_ref()
                .or(m.sig.ret.as_ref())
                .and_then(class_named);
            let required = m
                .sig
                .params
                .iter()
                .filter(|p| !p.has_default && !p.variadic)
                .count();
            (written, required, m.sig.params.len())
        });
        let (class, args) = match written {
            Some((class, args)) => (Dynamic::from(class), args),
            None => (Dynamic::UNIT, false),
        };
        out.insert("returns_class".into(), class);
        out.insert("returns_generic".into(), args.into());
        out.insert("required_params".into(), (required as INT).into());
        out.insert("param_count".into(), (count as INT).into());
        out.insert("attributes".into(), self.attributes_in(ctx).into());
        out.insert("returned_call".into(), self.returned_call_in(ctx));
        Some(out)
    }

    /// What it says it returns, as seen on the class it was looked up on.
    fn return_type(&self) -> Dynamic {
        with_ctx(|ctx| self.return_type_in(ctx))
    }

    fn return_type_in(&self, ctx: &Ctx<'_>) -> Dynamic {
        self.with_in(ctx, |ctx, owner, m, on| {
            let sig = ctx.method_sig_in(owner, m, context(owner, on));
            type_facade(ctx, sig.ret)
        })
    }

    fn params(&self) -> Array {
        with_ctx(|ctx| self.params_in(ctx))
    }

    fn params_in(&self, ctx: &Ctx<'_>) -> Array {
        self.with_in(ctx, |ctx, owner, m, on| {
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

    /// Its parameters as PHPDoc writes them in a `@method` tag, past the first `skip` of them:
    /// `int $year, string ...$names`, with names complete.
    fn params_text(&self, skip: INT) -> String {
        with_ctx(|ctx| {
            self.with_in(ctx, |ctx, owner, m, on| {
                let sig = ctx.method_sig_in(owner, m, context(owner, on));
                let root = stir_db::parse(ctx.db, owner.file).syntax();
                let skip = usize::try_from(skip).unwrap_or(0);
                let params = m.sig.params.iter().zip(&sig.params).skip(skip);
                params
                    .map(|(def, p)| {
                        let mut text = String::new();
                        if !p.ty.is_untyped() && !p.ty.is_error() {
                            text.push_str(&format!("{} ", p.ty));
                        }
                        if def.by_ref {
                            text.push('&');
                        }
                        if def.variadic {
                            text.push_str("...");
                        }
                        text.push_str(&format!("${}", def.name));
                        // as it's written, which a tag keeps as text
                        let node = (!m.magic).then(|| def.ptr.to_node(&root));
                        let default = node.and_then(|node| {
                            let has_one = node
                                .children_with_tokens()
                                .skip_while(|t| t.kind() != EQ)
                                .nth(1)
                                .is_some();
                            let value = node.children().last()?;
                            has_one.then(|| value.text().to_string().trim().to_owned())
                        });
                        if def.has_default {
                            let default = default.filter(|d| !d.is_empty() && !d.contains('\n'));
                            text.push_str(&format!(" = {}", default.as_deref().unwrap_or("null")));
                        }
                        text
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
        })
    }

    fn attributes(&self) -> Array {
        with_ctx(|ctx| self.attributes_in(ctx))
    }

    fn attributes_in(&self, ctx: &Ctx<'_>) -> Array {
        self.with_in(ctx, |ctx, owner, m, _| {
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
        with_ctx(|ctx| self.returned_value_in(ctx))
    }

    fn returned_value_in(&self, ctx: &Ctx<'_>) -> Dynamic {
        self.with_in(ctx, |ctx, owner, m, _| match self.returned(ctx, owner, m) {
            Some(expr) => literal(ctx, &Place::of(owner), &expr, 0),
            None => Dynamic::UNIT,
        })
    }

    /// The first call on `$this` that what it returns is made from, as `#{ name, args }`, such as
    /// the `hasMany` of `return $this->hasMany(Post::class)->latest();`. The arguments are their
    /// values where they're written out literally.
    fn returned_call(&self) -> Dynamic {
        with_ctx(|ctx| self.returned_call_in(ctx))
    }

    fn returned_call_in(&self, ctx: &Ctx<'_>) -> Dynamic {
        self.with_in(ctx, |ctx, owner, m, _| {
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
                    call.insert("args".into(), args_of(ctx, &Place::of(owner), &expr).into());
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
            let Some(owner) = ctx.class(&self.0.owner) else {
                return R::default();
            };
            let def = owner.def.properties.iter().find(|p| p.name == self.0.name);
            f(ctx, &owner, def)
        })
    }

    fn get_type(&self) -> Dynamic {
        with_ctx(|ctx| {
            let Some(on) = ctx.class(&self.0.on) else {
                return Dynamic::UNIT;
            };
            match ctx.find_property(&on.self_ty(), &self.0.name) {
                Some(p) => type_facade(ctx, p.ty),
                None => Dynamic::UNIT,
            }
        })
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
                    e.children_with_tokens().any(|t| {
                        t.kind() == VARIABLE && t.to_string() == format!("${}", self.0.name)
                    })
                });
            let value = elem.and_then(|e| e.children().last());
            match value {
                Some(v) => literal(ctx, &Place::of(owner), &v, 0),
                None => Dynamic::UNIT,
            }
        })
    }
}

/// Where a value is written: its file, for the names in it, and its class, if it's in one.
pub(super) struct Place<'a, 'db> {
    pub file: SourceFile,
    pub class: Option<&'a ClassModel<'db>>,
}

impl<'a, 'db> Place<'a, 'db> {
    fn of(class: &'a ClassModel<'db>) -> Self {
        Place {
            file: class.file,
            class: Some(class),
        }
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
            attribute.insert("args".into(), args_of(ctx, &Place::of(owner), &a).into());
            Some(Dynamic::from(attribute))
        })
        .collect()
}

/// The arguments of a call or attribute, by position and, for named ones, by name: a map with
/// keys `"0"`, `"1"` and so on, and the names.
fn args_of<'db>(ctx: &Ctx<'db>, at: &Place<'_, 'db>, node: &SyntaxNode) -> Map {
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
            .map_or(Dynamic::UNIT, |v| literal(ctx, at, &v, 0));
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
            return literal(ctx, &Place::of(&owner), &node, depth + 1);
        }
    }
    Dynamic::UNIT
}

/// The value an expression in `owner`'s file is written as: strings, numbers, `true`, `false`,
/// `null`, arrays of them, `Foo::class` as the class's name, constants with such values, and the
/// default of `env('NAME', default)`. Anything else is `()`.
pub(super) fn literal<'db>(
    ctx: &Ctx<'db>,
    at: &Place<'_, 'db>,
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
            .map_or(Dynamic::UNIT, |n| literal(ctx, at, &n, depth)),
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
                .map_or(Dynamic::UNIT, |n| literal(ctx, at, &n, depth));
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
                literal(ctx, at, &parts[0], depth + 1),
                literal(ctx, at, &parts[1], depth + 1),
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
                    .map_or(Dynamic::UNIT, |v| literal(ctx, at, v, depth + 1));
                if has_key && parts.len() == 2 {
                    let key = literal(ctx, at, &parts[0], depth + 1);
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
                "self" | "static" => match at.class {
                    Some(c) => c.name.to_string(),
                    None => return Dynamic::UNIT,
                },
                "parent" => match at.class.and_then(|c| ctx.supers(c).parent.clone()) {
                    Some(p) => match p.kind() {
                        TyKind::Class { name, .. } => name.to_string(),
                        _ => return Dynamic::UNIT,
                    },
                    None => return Dynamic::UNIT,
                },
                _ if base.kind() == NAME_EXPR => scope_at(ctx, at.file, node)
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
            let args = args_of(ctx, at, node);
            args.get("1").cloned().unwrap_or(Dynamic::UNIT)
        }
        _ => Dynamic::UNIT,
    }
}

pub(super) fn register(engine: &mut Engine) {
    engine.register_fn("class", |name: &str| find_class(name));
    engine
        .register_type_with_name::<ClassFacade>("Class")
        .register_get("name", |c: &mut ClassFacade| c.0.name.clone())
        .register_get("kind", |c: &mut ClassFacade| c.0.kind)
        .register_get("parent", |c: &mut ClassFacade| {
            c.0.parent.clone().map_or(Dynamic::UNIT, Dynamic::from)
        })
        .register_get("type", |c: &mut ClassFacade| c.self_type())
        .register_get("type_params", |c: &mut ClassFacade| {
            c.0.type_params
                .iter()
                .cloned()
                .map(Dynamic::from)
                .collect::<Array>()
        })
        .register_get("is_abstract", |c: &mut ClassFacade| c.0.is_abstract)
        .register_fn("is_subtype_of", |c: &mut ClassFacade, n: &str| {
            c.is_subtype_of(n)
        })
        .register_fn("ancestors", |c: &mut ClassFacade| {
            c.0.ancestors
                .iter()
                .cloned()
                .map(Dynamic::from)
                .collect::<Array>()
        })
        .register_fn("methods", |c: &mut ClassFacade| {
            c.methods()
                .iter()
                .cloned()
                .map(Dynamic::from)
                .collect::<Array>()
        })
        .register_fn("method", |c: &mut ClassFacade, n: &str| c.method(n))
        .register_fn("methods_below", |c: &mut ClassFacade, n: &str| {
            c.methods_below(n)
        })
        .register_fn("method_details", |c: &mut ClassFacade, n: Array| {
            c.method_details(n)
        })
        .register_fn("properties", |c: &mut ClassFacade| {
            c.properties()
                .iter()
                .cloned()
                .map(Dynamic::from)
                .collect::<Array>()
        })
        .register_fn("property", |c: &mut ClassFacade, n: &str| c.property(n))
        .register_fn("constant", |c: &mut ClassFacade, n: &str| c.constant(n))
        .register_fn("attributes", |c: &mut ClassFacade| c.attributes());
    engine
        .register_type_with_name::<MethodFacade>("Method")
        .register_get("name", |m: &mut MethodFacade| m.0.name.clone())
        .register_get("class", |m: &mut MethodFacade| m.0.owner_name.clone())
        .register_get("is_static", |m: &mut MethodFacade| m.0.is_static)
        .register_get("is_magic", |m: &mut MethodFacade| m.0.is_magic)
        .register_get("visibility", |m: &mut MethodFacade| m.0.visibility)
        .register_get("return_type", |m: &mut MethodFacade| m.return_type())
        .register_get("params", |m: &mut MethodFacade| m.params())
        .register_fn("attributes", |m: &mut MethodFacade| m.attributes())
        .register_fn("params_text", |m: &mut MethodFacade, skip: INT| {
            m.params_text(skip)
        })
        .register_fn("returned_value", |m: &mut MethodFacade| m.returned_value())
        .register_fn("returned_call", |m: &mut MethodFacade| m.returned_call());
    engine
        .register_type_with_name::<PropertyFacade>("Property")
        .register_get("name", |p: &mut PropertyFacade| p.0.name.clone())
        .register_get("class", |p: &mut PropertyFacade| p.0.owner_name.clone())
        .register_get("type", |p: &mut PropertyFacade| p.get_type())
        .register_get("is_static", |p: &mut PropertyFacade| p.0.is_static)
        .register_get("visibility", |p: &mut PropertyFacade| p.0.visibility)
        .register_get("default_value", |p: &mut PropertyFacade| p.default());
}
