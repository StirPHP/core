//! PHPDoc (PHPStan dialect) types and tags lowered into HIR [`TypeRef`]s.
//!
//! Project sources, vendor files and stubs all carry PHPDoc types.

use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use stir_phpdoc::{
    ConstExpr, DocBlock, GenericArg, Keyword, TagKind, TemplateVariance, TypeKind, TypeNode,
    Variance as DocVariance,
};
use stir_syntax::{TextRange, TextSize};

use crate::items::{TypeParamDef, Variance};
use crate::name::{Fqn, Scope};
use crate::type_ref::{
    CallableParamRef, DocKeyword, Literal, PhpBuiltin, ShapeFieldRef, ShapeKeyRef, TypeRef,
    TypeRefKind,
};

/// Type aliases in scope (`@phpstan-type` / `@phpstan-import-type`), by name.
pub type Aliases = FxHashMap<SmolStr, TypeRef>;

/// Everything a PHPDoc type is lowered against.
pub struct DocCx<'a> {
    pub scope: &'a Scope,
    /// Template names in scope.
    pub type_params: &'a [SmolStr],
    pub aliases: &'a Aliases,
}

fn tr(kind: TypeRefKind, range: TextRange) -> TypeRef {
    TypeRef { kind, range }
}

fn boxed(t: TypeRef) -> Box<TypeRef> {
    Box::new(t)
}

impl DocCx<'_> {
    pub fn lower(&self, node: &TypeNode) -> TypeRef {
        let range = node.range;
        let mixed = || tr(TypeRefKind::Php(PhpBuiltin::Mixed), range);
        let kind = match &node.kind {
            TypeKind::Identifier(name) => return self.identifier(name, range),
            TypeKind::This => TypeRefKind::Php(PhpBuiltin::Static),
            TypeKind::Nullable(inner) => TypeRefKind::Nullable(boxed(self.lower(inner))),
            TypeKind::Union(ms) => TypeRefKind::Union(ms.iter().map(|m| self.lower(m)).collect()),
            TypeKind::Intersection(ms) => {
                TypeRefKind::Intersection(ms.iter().map(|m| self.lower(m)).collect())
            }
            TypeKind::Generic { base, args } => return self.generic(&base.name, args, range),
            TypeKind::Array(elem) => TypeRefKind::PhpArray {
                key: None,
                value: boxed(self.lower(elem)),
                non_empty: false,
            },
            TypeKind::OffsetAccess { .. } => return mixed(),
            // `list{…}` shapes get the implicit `0..n` keys like `array{…}`.
            TypeKind::ArrayShape(shape) => {
                let mut next = 0i64;
                let fields = shape
                    .items
                    .iter()
                    .map(|item| {
                        let key = match &item.key {
                            Some(k) => match k.text().parse::<i64>() {
                                Ok(i) => {
                                    next = i + 1;
                                    ShapeKeyRef::Int(i)
                                }
                                Err(_) => ShapeKeyRef::Str(SmolStr::new(k.text())),
                            },
                            None => {
                                next += 1;
                                ShapeKeyRef::Int(next - 1)
                            }
                        };
                        ShapeFieldRef {
                            key,
                            optional: item.optional,
                            ty: self.lower(&item.value),
                        }
                    })
                    .collect();
                TypeRefKind::Shape {
                    fields,
                    sealed: shape.sealed && shape.unsealed.is_none(),
                }
            }
            TypeKind::ObjectShape(_) => TypeRefKind::Php(PhpBuiltin::Object),
            TypeKind::Const(c) => match c {
                ConstExpr::Int { value: Some(v), .. } => TypeRefKind::Literal(Literal::Int(*v)),
                ConstExpr::Int { value: None, .. } => TypeRefKind::Php(PhpBuiltin::Int),
                ConstExpr::Float { raw, .. } => TypeRefKind::Literal(Literal::Float(raw.clone())),
                ConstExpr::String { value, .. } => {
                    TypeRefKind::Literal(Literal::String(SmolStr::new(value)))
                }
                ConstExpr::ClassConst { .. } => return mixed(),
            },
            TypeKind::Callable(c) => {
                // Callable-level templates are treated as `mixed`.
                let mut aliases;
                let inner_cx;
                let cx = if c.templates.is_empty() {
                    self
                } else {
                    aliases = self.aliases.clone();
                    for t in &c.templates {
                        aliases.insert(t.name.name.clone(), mixed());
                    }
                    inner_cx = DocCx {
                        scope: self.scope,
                        type_params: self.type_params,
                        aliases: &aliases,
                    };
                    &inner_cx
                };
                let closure = c.name.name.eq_ignore_ascii_case("closure")
                    || c.name.name.eq_ignore_ascii_case("\\closure")
                    || c.name.name.eq_ignore_ascii_case("pure-closure");
                TypeRefKind::Callable {
                    params: c
                        .params
                        .iter()
                        .map(|p| CallableParamRef {
                            ty: cx.lower(&p.ty),
                            optional: p.optional,
                            variadic: p.variadic,
                        })
                        .collect(),
                    ret: boxed(cx.lower(&c.return_type)),
                    closure,
                }
            }
            // Conditional types degrade to the union of their branches.
            TypeKind::Conditional(c) => {
                let (a, b) = (self.lower(&c.if_type), self.lower(&c.else_type));
                // what a template resolves to decides the branch, which isn't tracked: say
                // nothing, and leave it to the native type if there is one
                if mentions_type_param(&a) || mentions_type_param(&b) {
                    return mixed();
                }
                TypeRefKind::Union(vec![a, b])
            }
            TypeKind::Error(_) => return mixed(),
        };
        tr(kind, range)
    }

    fn identifier(&self, name: &str, range: TextRange) -> TypeRef {
        if !name.starts_with('\\') {
            if self.type_params.iter().any(|p| p == name) {
                return tr(TypeRefKind::Param(SmolStr::new(name)), range);
            }
            if let Some(alias) = self.aliases.get(name) {
                let mut t = alias.clone();
                t.range = range;
                return t;
            }
        }
        if let Some(kw) = Keyword::from_name(name) {
            return tr(keyword(kw), range);
        }
        tr(
            TypeRefKind::Named(self.scope.resolve_class(name), Vec::new()),
            range,
        )
    }

    fn arg(&self, a: &GenericArg) -> TypeRef {
        if a.variance == DocVariance::Bivariant {
            tr(TypeRefKind::Php(PhpBuiltin::Mixed), a.ty.range)
        } else {
            self.lower(&a.ty)
        }
    }

    fn generic(&self, base: &str, args: &[GenericArg], range: TextRange) -> TypeRef {
        let lowered = || args.iter().map(|a| self.arg(a)).collect::<Vec<_>>();
        let kw = if base.starts_with('\\') || self.type_params.iter().any(|p| p == base) {
            None
        } else {
            Keyword::from_name(base)
        };
        let key_value = |non_empty: bool, iterable: bool| {
            let mut ls = lowered();
            let value = ls
                .pop()
                .unwrap_or_else(|| tr(TypeRefKind::Php(PhpBuiltin::Mixed), range));
            let key = ls.pop().map(boxed);
            if iterable {
                TypeRefKind::Iterable {
                    key,
                    value: boxed(value),
                }
            } else {
                TypeRefKind::PhpArray {
                    key,
                    value: boxed(value),
                    non_empty,
                }
            }
        };
        let kind = match kw {
            Some(Keyword::Array | Keyword::AssociativeArray) => key_value(false, false),
            Some(Keyword::NonEmptyArray) => key_value(true, false),
            Some(Keyword::Iterable) => key_value(false, true),
            Some(Keyword::List | Keyword::NonEmptyList) => TypeRefKind::PhpList {
                elem: boxed(
                    lowered()
                        .pop()
                        .unwrap_or_else(|| tr(TypeRefKind::Php(PhpBuiltin::Mixed), range)),
                ),
                non_empty: kw == Some(Keyword::NonEmptyList),
            },
            Some(
                Keyword::ClassString
                | Keyword::InterfaceString
                | Keyword::TraitString
                | Keyword::EnumString,
            ) => TypeRefKind::ClassString(lowered().into_iter().next().map(boxed)),
            Some(Keyword::Int) => {
                let bound = |a: Option<&GenericArg>| -> Option<i64> {
                    match &a?.ty.kind {
                        TypeKind::Const(ConstExpr::Int { value, .. }) => *value,
                        _ => None,
                    }
                };
                TypeRefKind::IntRange {
                    min: bound(args.first()),
                    max: bound(args.get(1)),
                }
            }
            Some(Keyword::KeyOf) => TypeRefKind::DocKeyword(DocKeyword::ArrayKey),
            Some(Keyword::IntMask | Keyword::IntMaskOf) => TypeRefKind::Php(PhpBuiltin::Int),
            Some(k @ (Keyword::Static | Keyword::SelfType | Keyword::Parent)) => {
                let b = match k {
                    Keyword::Static => PhpBuiltin::Static,
                    Keyword::SelfType => PhpBuiltin::SelfType,
                    _ => PhpBuiltin::Parent,
                };
                TypeRefKind::SelfGeneric(b, lowered())
            }
            // `value-of<…>`, `new<…>`, `template-type<…>` and other keyword generics
            Some(Keyword::Object) => TypeRefKind::Php(PhpBuiltin::Object),
            Some(_) => TypeRefKind::Php(PhpBuiltin::Mixed),
            None => {
                if self.type_params.iter().any(|p| p == base) {
                    TypeRefKind::Param(SmolStr::new(base))
                } else if let Some(alias) = self.aliases.get(base) {
                    alias.kind.clone()
                } else {
                    TypeRefKind::Named(self.scope.resolve_class(base), lowered())
                }
            }
        };
        tr(kind, range)
    }
}

fn keyword(kw: Keyword) -> TypeRefKind {
    use Keyword as K;
    let php = TypeRefKind::Php;
    let doc = TypeRefKind::DocKeyword;
    let mixed = || {
        Box::new(tr(
            TypeRefKind::Php(PhpBuiltin::Mixed),
            TextRange::default(),
        ))
    };
    match kw {
        K::Int | K::NonZeroInt | K::IntMask | K::IntMaskOf => php(PhpBuiltin::Int),
        K::String
        | K::LiteralString
        | K::LowercaseString
        | K::UppercaseString
        | K::CallableString => php(PhpBuiltin::String),
        K::Bool => php(PhpBuiltin::Bool),
        K::Float => php(PhpBuiltin::Float),
        K::Mixed
        | K::NonEmptyMixed
        | K::Empty
        | K::KeyOf
        | K::ValueOf
        | K::New
        | K::TemplateType => php(PhpBuiltin::Mixed),
        K::Void => php(PhpBuiltin::Void),
        K::Never => php(PhpBuiltin::Never),
        K::Null => php(PhpBuiltin::Null),
        K::True => php(PhpBuiltin::True),
        K::False => php(PhpBuiltin::False),
        K::Array | K::AssociativeArray | K::CallableArray => php(PhpBuiltin::Array),
        K::NonEmptyArray => TypeRefKind::PhpArray {
            key: None,
            value: mixed(),
            non_empty: true,
        },
        K::List => TypeRefKind::PhpList {
            elem: mixed(),
            non_empty: false,
        },
        K::NonEmptyList => TypeRefKind::PhpList {
            elem: mixed(),
            non_empty: true,
        },
        K::Iterable => php(PhpBuiltin::Iterable),
        K::Callable | K::PureCallable => php(PhpBuiltin::Callable),
        K::PureClosure => TypeRefKind::Named(Fqn::new("Closure"), Vec::new()),
        K::CallableObject | K::Object => php(PhpBuiltin::Object),
        K::Resource | K::ClosedResource | K::OpenResource => doc(DocKeyword::Resource),
        K::Scalar | K::NonEmptyScalar | K::EmptyScalar => doc(DocKeyword::Scalar),
        K::Numeric => doc(DocKeyword::Numeric),
        K::ArrayKey => doc(DocKeyword::ArrayKey),
        K::PositiveInt => TypeRefKind::IntRange {
            min: Some(1),
            max: None,
        },
        K::NegativeInt => TypeRefKind::IntRange {
            min: None,
            max: Some(-1),
        },
        K::NonNegativeInt => TypeRefKind::IntRange {
            min: Some(0),
            max: None,
        },
        K::NonPositiveInt => TypeRefKind::IntRange {
            min: None,
            max: Some(0),
        },
        K::NonEmptyString
        | K::NonFalsyString
        | K::NonEmptyLiteralString
        | K::NonEmptyLowercaseString
        | K::NonEmptyUppercaseString => doc(DocKeyword::NonEmptyString),
        K::NumericString => doc(DocKeyword::NumericString),
        K::ClassString | K::InterfaceString | K::TraitString | K::EnumString => {
            TypeRefKind::ClassString(None)
        }
        K::Static => php(PhpBuiltin::Static),
        K::SelfType => php(PhpBuiltin::SelfType),
        K::Parent => php(PhpBuiltin::Parent),
    }
}

/// Parses a doc comment located at `offset`.
pub fn parse(text: &str, offset: TextSize) -> DocBlock {
    stir_phpdoc::parse_doc_comment(text, offset)
}

/// Parses and lowers a standalone type string (e.g. from `#[LanguageLevelTypeAware]`).
pub fn lower_type_str(cx: &DocCx<'_>, text: &str, offset: TextSize) -> TypeRef {
    cx.lower(&stir_phpdoc::parse_type(text, offset))
}

/// `@template` tags as type parameters (names already declared in `existing`
/// are skipped). Bounds may mention the templates themselves and `outer` names.
pub fn templates(
    doc: &DocBlock,
    outer: &[SmolStr],
    scope: &Scope,
    aliases: &Aliases,
) -> Vec<TypeParamDef> {
    let tags = doc.templates();
    let names: Vec<SmolStr> = tags
        .iter()
        .map(|t| t.param.name.name.clone())
        .chain(outer.iter().cloned())
        .collect();
    let cx = DocCx {
        scope,
        type_params: &names,
        aliases,
    };
    let mut out: Vec<TypeParamDef> = Vec::new();
    for t in tags {
        let name = t.param.name.name.clone();
        if out.iter().any(|e| e.name == name) {
            continue;
        }
        out.push(TypeParamDef {
            name,
            variance: match t.variance {
                TemplateVariance::Invariant => Variance::Invariant,
                TemplateVariance::Covariant => Variance::Out,
                TemplateVariance::Contravariant => Variance::In,
            },
            bound: t.param.bound.as_ref().map(|b| cx.lower(b)),
            range: t.param.range,
        });
    }
    out
}

/// Local `@phpstan-type` aliases and `@phpstan-import-type` imports declared in a
/// class doc block. Returns `(in-scope aliases, locally defined aliases)`.
pub fn class_aliases(
    doc: &DocBlock,
    scope: &Scope,
    type_params: &[SmolStr],
) -> (Aliases, Vec<(SmolStr, TypeRef)>) {
    let mut aliases = Aliases::default();
    let mut defs: Vec<(SmolStr, &TypeNode)> = Vec::new();
    for tag in doc.effective_tags() {
        match &tag.kind {
            TagKind::TypeAlias(a) => defs.push((a.name.name.clone(), &a.ty)),
            TagKind::TypeAliasImport(i) => {
                let from = match &i.from.kind {
                    TypeKind::Identifier(n) => scope.resolve_class(n),
                    TypeKind::Generic { base, .. } => scope.resolve_class(&base.name),
                    _ => continue,
                };
                let local = i.alias.as_ref().unwrap_or(&i.name).name.clone();
                aliases.insert(
                    local,
                    tr(
                        TypeRefKind::ImportedAlias {
                            class: from,
                            name: i.name.name.clone(),
                        },
                        tag.range,
                    ),
                );
            }
            _ => {}
        }
    }
    // Two passes so aliases can refer to aliases declared after them.
    let mut lowered: Vec<(SmolStr, TypeRef)> = Vec::new();
    for _ in 0..2 {
        let cx = DocCx {
            scope,
            type_params,
            aliases: &aliases,
        };
        lowered = defs.iter().map(|(n, t)| (n.clone(), cx.lower(t))).collect();
        for (n, t) in &lowered {
            aliases.insert(n.clone(), t.clone());
        }
    }
    (aliases, lowered)
}

/// Is the parameter documented as optional with the phpstorm-stubs
/// `@param T $x [optional]` convention?
pub fn optional_marker(doc: &DocBlock, name: &str) -> bool {
    doc.param(name).is_some_and(|p| {
        p.description
            .as_ref()
            .is_some_and(|d| d.text.trim_start().starts_with("[optional]"))
    })
}

fn mentions_type_param(t: &TypeRef) -> bool {
    match &t.kind {
        TypeRefKind::Param(_) => true,
        TypeRefKind::Nullable(inner) => mentions_type_param(inner),
        TypeRefKind::Union(ms) | TypeRefKind::Intersection(ms) => {
            ms.iter().any(mentions_type_param)
        }
        TypeRefKind::Named(_, args) => args.iter().any(mentions_type_param),
        TypeRefKind::PhpList { elem, .. } => mentions_type_param(elem),
        TypeRefKind::PhpArray { key, value, .. } | TypeRefKind::Iterable { key, value } => {
            key.as_deref().is_some_and(mentions_type_param) || mentions_type_param(value)
        }
        _ => false,
    }
}
