//! Type AST. All ranges are absolute file offsets.

use smol_str::SmolStr;
use text_size::TextRange;

use crate::keyword::Keyword;

/// A parsed PHPStan type.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeNode {
    pub kind: TypeKind,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    /// A name: a class (`\Foo\Bar`), or a keyword such as `int`, `non-empty-string`,
    /// `static`, `self`. See [`TypeNode::keyword`].
    Identifier(SmolStr),
    /// `$this`
    This,
    /// `?T`
    Nullable(Box<TypeNode>),
    /// `A|B`
    Union(Vec<TypeNode>),
    /// `A&B`
    Intersection(Vec<TypeNode>),
    /// `A<B, covariant C, *>`, also `int<0, max>`, `key-of<T>`, `class-string<T>`...
    Generic { base: Name, args: Vec<GenericArg> },
    /// `T[]`
    Array(Box<TypeNode>),
    /// `T[K]`
    OffsetAccess {
        ty: Box<TypeNode>,
        offset: Box<TypeNode>,
    },
    /// `array{...}`, `list{...}`, `non-empty-array{...}`, `non-empty-list{...}`
    ArrayShape(Box<ArrayShape>),
    /// `object{a: int, b?: string}`
    ObjectShape(Vec<ObjectShapeItem>),
    /// Literal or class-constant type.
    Const(ConstExpr),
    /// `callable(int, string=): void`, `Closure<T>(T $x): T`, `pure-callable(): int`
    Callable(Box<CallableType>),
    /// `($x is int ? A : B)` / `(T is not int ? A : B)`
    Conditional(Box<ConditionalType>),
    /// Unparseable type; the message describes the problem.
    Error(String),
}

impl TypeNode {
    pub fn new(kind: TypeKind, range: TextRange) -> Self {
        Self { kind, range }
    }

    /// The identifier name for [`TypeKind::Identifier`] nodes.
    pub fn as_identifier(&self) -> Option<&str> {
        match &self.kind {
            TypeKind::Identifier(n) => Some(n),
            _ => None,
        }
    }

    /// Well-known keyword for identifier nodes (`int`, `list`, `positive-int`, ...).
    pub fn keyword(&self) -> Option<Keyword> {
        match &self.kind {
            TypeKind::Identifier(n) => Keyword::from_name(n),
            TypeKind::Generic { base, .. } => Keyword::from_name(&base.name),
            _ => None,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self.kind, TypeKind::Error(_))
    }

    /// Visits this node and all descendants (pre-order).
    pub fn walk<'a>(&'a self, f: &mut dyn FnMut(&'a TypeNode)) {
        f(self);
        match &self.kind {
            TypeKind::Identifier(_) | TypeKind::This | TypeKind::Const(_) | TypeKind::Error(_) => {}
            TypeKind::Nullable(t) | TypeKind::Array(t) => t.walk(f),
            TypeKind::Union(ts) | TypeKind::Intersection(ts) => ts.iter().for_each(|t| t.walk(f)),
            TypeKind::Generic { args, .. } => args.iter().for_each(|a| a.ty.walk(f)),
            TypeKind::OffsetAccess { ty, offset } => {
                ty.walk(f);
                offset.walk(f);
            }
            TypeKind::ArrayShape(s) => {
                s.items.iter().for_each(|i| i.value.walk(f));
                if let Some(u) = &s.unsealed {
                    if let Some(k) = &u.key {
                        k.walk(f);
                    }
                    u.value.walk(f);
                }
            }
            TypeKind::ObjectShape(items) => items.iter().for_each(|i| i.value.walk(f)),
            TypeKind::Callable(c) => {
                for t in &c.templates {
                    t.walk(f);
                }
                c.params.iter().for_each(|p| p.ty.walk(f));
                c.return_type.walk(f);
            }
            TypeKind::Conditional(c) => {
                if let ConditionalSubject::Type(t) = &c.subject {
                    t.walk(f);
                }
                c.target.walk(f);
                c.if_type.walk(f);
                c.else_type.walk(f);
            }
        }
    }

    /// Collects all `Error` nodes in this type.
    pub fn errors(&self) -> Vec<&TypeNode> {
        let mut out = Vec::new();
        self.walk(&mut |t| {
            if t.is_error() {
                out.push(t);
            }
        });
        out
    }
}

/// A name with its source range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    pub name: SmolStr,
    pub range: TextRange,
}

/// A `$variable` (name stored without the `$`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub name: SmolStr,
    pub range: TextRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Variance {
    Invariant,
    Covariant,
    Contravariant,
    /// `*` wildcard; the type is `mixed`.
    Bivariant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenericArg {
    pub variance: Variance,
    pub ty: TypeNode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrayShapeKind {
    Array,
    List,
    NonEmptyArray,
    NonEmptyList,
}

impl ArrayShapeKind {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "array" => Self::Array,
            "list" => Self::List,
            "non-empty-array" => Self::NonEmptyArray,
            "non-empty-list" => Self::NonEmptyList,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Array => "array",
            Self::List => "list",
            Self::NonEmptyArray => "non-empty-array",
            Self::NonEmptyList => "non-empty-list",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArrayShape {
    pub kind: ArrayShapeKind,
    pub items: Vec<ArrayShapeItem>,
    /// `false` when the shape ends with `...`.
    pub sealed: bool,
    /// `...<K, V>` / `...<V>`
    pub unsealed: Option<Box<UnsealedType>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArrayShapeItem {
    pub key: Option<ShapeKey>,
    pub optional: bool,
    pub value: TypeNode,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnsealedType {
    pub key: Option<TypeNode>,
    pub value: TypeNode,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectShapeItem {
    pub key: ShapeKey,
    pub optional: bool,
    pub value: TypeNode,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeKey {
    pub kind: ShapeKeyKind,
    pub range: TextRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShapeKeyKind {
    Identifier(SmolStr),
    /// Integer key (raw source text).
    Int(SmolStr),
    String {
        value: String,
        quote: Quote,
    },
}

impl ShapeKey {
    /// The key as PHP would see it (strings unquoted).
    pub fn text(&self) -> &str {
        match &self.kind {
            ShapeKeyKind::Identifier(s) | ShapeKeyKind::Int(s) => s,
            ShapeKeyKind::String { value, .. } => value,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quote {
    Single,
    Double,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstExpr {
    /// Integer literal; `value` is `None` on overflow.
    Int {
        raw: SmolStr,
        value: Option<i64>,
    },
    Float {
        raw: SmolStr,
        value: f64,
    },
    String {
        value: String,
        quote: Quote,
    },
    /// `Foo::BAR`, `Foo::BAR_*`, `Foo::*`, `self::X`
    ClassConst {
        class: SmolStr,
        name: SmolStr,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallableType {
    /// `callable`, `Closure`, `pure-callable`, ...
    pub name: Name,
    pub templates: Vec<TemplateParam>,
    pub params: Vec<CallableParam>,
    pub return_type: TypeNode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallableParam {
    pub ty: TypeNode,
    pub by_ref: bool,
    pub variadic: bool,
    pub name: Option<Variable>,
    /// Trailing `=`.
    pub optional: bool,
    pub range: TextRange,
}

/// A template parameter, used by `@template` tags, `@method` and callable templates.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateParam {
    pub name: Name,
    /// `of`/`as` bound.
    pub bound: Option<TypeNode>,
    /// `super` lower bound.
    pub lower_bound: Option<TypeNode>,
    /// `= Default`
    pub default: Option<TypeNode>,
    pub range: TextRange,
}

impl TemplateParam {
    fn walk<'a>(&'a self, f: &mut dyn FnMut(&'a TypeNode)) {
        for t in [&self.bound, &self.lower_bound, &self.default]
            .into_iter()
            .flatten()
        {
            t.walk(f);
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConditionalType {
    pub subject: ConditionalSubject,
    /// `is not`
    pub negated: bool,
    pub target: TypeNode,
    pub if_type: TypeNode,
    pub else_type: TypeNode,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConditionalSubject {
    Type(TypeNode),
    /// `$param is ...` (conditional return types)
    Parameter(Variable),
}
