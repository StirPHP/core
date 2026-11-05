//! Syntactic types with names already resolved.

use smol_str::SmolStr;
use stir_syntax::{SyntaxKind::*, SyntaxNode, TextRange};

use crate::ast::NodeExt;
use crate::name::{Fqn, Scope};

/// PHP builtin type keywords (case-insensitive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhpBuiltin {
    Int,
    Float,
    String,
    Bool,
    Array,
    Mixed,
    Void,
    Never,
    Null,
    False,
    True,
    Object,
    Callable,
    Iterable,
    SelfType,
    Static,
    Parent,
}

impl PhpBuiltin {
    pub fn from_name(s: &str) -> Option<PhpBuiltin> {
        Some(match s {
            "int" | "integer" => PhpBuiltin::Int,
            "float" | "double" => PhpBuiltin::Float,
            "string" => PhpBuiltin::String,
            "bool" | "boolean" => PhpBuiltin::Bool,
            "array" => PhpBuiltin::Array,
            "mixed" => PhpBuiltin::Mixed,
            "void" => PhpBuiltin::Void,
            "never" => PhpBuiltin::Never,
            "null" => PhpBuiltin::Null,
            "false" => PhpBuiltin::False,
            "true" => PhpBuiltin::True,
            "object" => PhpBuiltin::Object,
            "callable" => PhpBuiltin::Callable,
            "iterable" => PhpBuiltin::Iterable,
            "self" => PhpBuiltin::SelfType,
            "static" => PhpBuiltin::Static,
            "parent" => PhpBuiltin::Parent,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Literal {
    Int(i64),
    Float(SmolStr),
    String(SmolStr),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeRefKind {
    Php(PhpBuiltin),
    /// A type parameter in scope.
    Param(SmolStr),
    /// `(subject is target ? then : otherwise)`.
    Conditional {
        subject: ConditionalSubjectRef,
        target: Box<TypeRef>,
        then: Box<TypeRef>,
        otherwise: Box<TypeRef>,
    },
    /// `self<A, B>`, `static<A, B>` or `parent<A, B>` in PHPDoc: the class with these arguments.
    SelfGeneric(PhpBuiltin, Vec<TypeRef>),
    /// A class, interface or enum.
    Named(Fqn, Vec<TypeRef>),
    Nullable(Box<TypeRef>),
    Union(Vec<TypeRef>),
    Intersection(Vec<TypeRef>),
    Literal(Literal),
    /// PHPStan `list<T>` / `non-empty-list<T>`.
    PhpList {
        elem: Box<TypeRef>,
        non_empty: bool,
    },
    /// PHPStan `array<K, V>`, `array<V>`, `T[]`, `non-empty-array<…>`; a missing
    /// key is `array-key`.
    PhpArray {
        key: Option<Box<TypeRef>>,
        value: Box<TypeRef>,
        non_empty: bool,
    },
    /// `iterable<K, V>` / `iterable<V>`.
    Iterable {
        key: Option<Box<TypeRef>>,
        value: Box<TypeRef>,
    },
    /// Array shape `array{a: int, b?: string}` / `list{int, string}`.
    Shape {
        fields: Vec<ShapeFieldRef>,
        sealed: bool,
    },
    /// `int<min, max>`, `positive-int`, … (`None` is unbounded).
    IntRange {
        min: Option<i64>,
        max: Option<i64>,
    },
    /// `class-string` / `class-string<T>`.
    ClassString(Option<Box<TypeRef>>),
    /// `callable(A, B=): R` / `Closure(A): R`.
    Callable {
        params: Vec<CallableParamRef>,
        ret: Box<TypeRef>,
        closure: bool,
    },
    /// PHPStan keyword types without a native counterpart.
    DocKeyword(DocKeyword),
    /// `@phpstan-import-type Name from Class`: resolved against the class's
    /// `type_aliases` by the checker.
    ImportedAlias {
        class: Fqn,
        name: SmolStr,
    },
    Error,
}

/// What a conditional type is about.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConditionalSubjectRef {
    /// A template.
    Type(Box<TypeRef>),
    /// A parameter of the function, by name.
    Param(SmolStr),
}

/// PHPStan-only keyword types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocKeyword {
    ArrayKey,
    Scalar,
    Numeric,
    Resource,
    NonEmptyString,
    NumericString,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ShapeKeyRef {
    Int(i64),
    Str(SmolStr),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ShapeFieldRef {
    pub key: ShapeKeyRef,
    pub optional: bool,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallableParamRef {
    pub ty: TypeRef,
    pub optional: bool,
    pub variadic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeRef {
    pub kind: TypeRefKind,
    pub range: TextRange,
}

impl TypeRef {
    pub fn new(kind: TypeRefKind, range: TextRange) -> TypeRef {
        TypeRef { kind, range }
    }
}

/// Context needed to lower a type node.
pub struct TypeCtx<'a> {
    pub scope: &'a Scope,
    /// Type parameter names in scope (innermost last).
    pub type_params: &'a [SmolStr],
}

impl TypeCtx<'_> {
    pub fn lower(&self, node: &SyntaxNode) -> TypeRef {
        let range = node.text_range();
        let kind = match node.kind() {
            NAMED_TYPE => self.lower_named(node),
            NULLABLE_TYPE => match node.children().next() {
                Some(inner) => TypeRefKind::Nullable(Box::new(self.lower(&inner))),
                None => TypeRefKind::Error,
            },
            UNION_TYPE => TypeRefKind::Union(node.children().map(|c| self.lower(&c)).collect()),
            INTERSECTION_TYPE => {
                TypeRefKind::Intersection(node.children().map(|c| self.lower(&c)).collect())
            }
            PAREN_TYPE => match node.children().next() {
                Some(inner) => return self.lower(&inner),
                None => TypeRefKind::Error,
            },
            _ => TypeRefKind::Error,
        };
        TypeRef { kind, range }
    }

    fn lower_named(&self, node: &SyntaxNode) -> TypeRefKind {
        let Some(name) = node.child(NAME_REF) else {
            return TypeRefKind::Error;
        };
        let text = name.text().to_string();
        let text = text.trim();
        if !text.contains('\\') {
            if self.type_params.iter().any(|p| p == text) {
                return TypeRefKind::Param(SmolStr::new(text));
            }
            let php = PhpBuiltin::from_name(&text.to_ascii_lowercase());
            if let Some(b) = php {
                return TypeRefKind::Php(b);
            }
        }
        TypeRefKind::Named(self.scope.resolve_class(text), Vec::new())
    }
}

/// Parses a PHP integer literal (decimal, hex, octal, binary, `_` separators).
pub fn parse_int(text: &str) -> Option<i64> {
    let t: String = text.chars().filter(|c| *c != '_').collect();
    let lower = t.to_ascii_lowercase();
    if let Some(h) = lower.strip_prefix("0x") {
        i64::from_str_radix(h, 16).ok()
    } else if let Some(b) = lower.strip_prefix("0b") {
        i64::from_str_radix(b, 2).ok()
    } else if let Some(o) = lower.strip_prefix("0o") {
        i64::from_str_radix(o, 8).ok()
    } else if lower.len() > 1 && lower.starts_with('0') {
        i64::from_str_radix(&lower[1..], 8).ok()
    } else {
        lower.parse().ok()
    }
}

/// The value of a simple quoted string literal (handles the common escapes).
/// PHP's `\x41`, `\101` and `\u{1F600}`, with `chars` just after the backslash. A byte above
/// 0x7F isn't valid text on its own, so it stays as written.
fn decode_numeric_escape(
    first: char,
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    out: &mut String,
) -> bool {
    let mut lookahead = chars.clone();
    let (digits, radix, max) = match first {
        'x' => {
            lookahead.next();
            (
                lookahead
                    .clone()
                    .take(2)
                    .take_while(char::is_ascii_hexdigit)
                    .count(),
                16,
                2,
            )
        }
        '0'..='7' => (
            lookahead
                .clone()
                .take(3)
                .take_while(|c| ('0'..='7').contains(c))
                .count(),
            8,
            3,
        ),
        'u' => {
            lookahead.next();
            if lookahead.next() != Some('{') {
                return false;
            }
            let hex: String = lookahead.clone().take_while(|c| *c != '}').collect();
            let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) else {
                return false;
            };
            out.push(c);
            for _ in 0..hex.chars().count() + 3 {
                chars.next();
            }
            return true;
        }
        _ => return false,
    };
    if digits == 0 || digits > max {
        return false;
    }
    let digits_text: String = lookahead.take(digits).collect();
    let Some(value) = u32::from_str_radix(&digits_text, radix)
        .ok()
        .filter(|v| *v < 0x80)
    else {
        return false;
    };
    out.push(char::from_u32(value).expect("ASCII"));
    for _ in 0..digits + usize::from(first == 'x') {
        chars.next();
    }
    true
}

pub fn unquote(text: &str) -> String {
    let text = text.strip_prefix(['b', 'B']).unwrap_or(text);
    if text.len() < 2 {
        return String::new();
    }
    let quote = text.as_bytes()[0];
    let inner = &text[1..text.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match (quote, chars.peek().copied()) {
            (b'\'', Some(n @ ('\\' | '\''))) => {
                out.push(n);
                chars.next();
            }
            (b'"', Some(n)) if decode_numeric_escape(n, &mut chars, &mut out) => {}
            (b'"', Some(n)) => {
                let mapped = match n {
                    'n' => Some('\n'),
                    't' => Some('\t'),
                    'r' => Some('\r'),
                    'v' => Some('\u{b}'),
                    'e' => Some('\u{1b}'),
                    'f' => Some('\u{c}'),
                    '\\' | '"' | '$' => Some(n),
                    _ => None,
                };
                match mapped {
                    Some(m) => {
                        out.push(m);
                        chars.next();
                    }
                    None => out.push('\\'),
                }
            }
            _ => out.push('\\'),
        }
    }
    out
}
