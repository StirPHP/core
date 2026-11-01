//! Syntactic types with names already resolved.

use smol_str::SmolStr;
use stir_syntax::{Dialect, SyntaxKind::*, SyntaxNode, TextRange};

use crate::ast::NodeExt;
use crate::name::{Fqn, Scope};

/// StirPHP-family builtin type names (case-sensitive, `.sphp` only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StirBuiltin {
    Int,
    Float,
    String,
    Bool,
    Unit,
    Nothing,
    Any,
    List,
    MutableList,
    Map,
    MutableMap,
}

impl StirBuiltin {
    pub fn from_name(s: &str) -> Option<StirBuiltin> {
        Some(match s {
            "Int" => StirBuiltin::Int,
            "Float" => StirBuiltin::Float,
            "String" => StirBuiltin::String,
            "Bool" | "Boolean" => StirBuiltin::Bool,
            "Unit" => StirBuiltin::Unit,
            "Nothing" => StirBuiltin::Nothing,
            "Any" => StirBuiltin::Any,
            "List" => StirBuiltin::List,
            "MutableList" => StirBuiltin::MutableList,
            "Map" => StirBuiltin::Map,
            "MutableMap" => StirBuiltin::MutableMap,
            _ => return None,
        })
    }
}

/// PHP builtin type keywords (case-insensitive in `.php`, lowercase in `.sphp`).
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
    Stir(StirBuiltin, Vec<TypeRef>),
    Php(PhpBuiltin),
    /// A type parameter in scope.
    Param(SmolStr),
    /// A class, interface, enum, record or type alias.
    Named(Fqn, Vec<TypeRef>),
    Nullable(Box<TypeRef>),
    Union(Vec<TypeRef>),
    Intersection(Vec<TypeRef>),
    Function {
        params: Vec<TypeRef>,
        ret: Box<TypeRef>,
    },
    Literal(Literal),
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeRef {
    pub kind: TypeRefKind,
    pub range: TextRange,
}

/// Context needed to lower a type node.
pub struct TypeCtx<'a> {
    pub scope: &'a Scope,
    pub dialect: Dialect,
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
            FUNCTION_TYPE => {
                let params = node
                    .child(FUNCTION_TYPE_PARAMS)
                    .map(|p| p.children().map(|c| self.lower(&c)).collect())
                    .unwrap_or_default();
                let ret = node
                    .children()
                    .filter(|c| c.kind() != FUNCTION_TYPE_PARAMS)
                    .last()
                    .map(|c| self.lower(&c))
                    .unwrap_or(TypeRef {
                        kind: TypeRefKind::Error,
                        range,
                    });
                TypeRefKind::Function {
                    params,
                    ret: Box::new(ret),
                }
            }
            LITERAL_TYPE => lower_literal(node),
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
        let args: Vec<TypeRef> = node
            .child(GENERIC_ARG_LIST)
            .map(|a| a.children().map(|c| self.lower(&c)).collect())
            .unwrap_or_default();
        let stir = self.dialect == Dialect::Stir;
        if !text.contains('\\') {
            if self.type_params.iter().any(|p| p == text) {
                return TypeRefKind::Param(SmolStr::new(text));
            }
            if stir && let Some(k) = StirBuiltin::from_name(text) {
                return TypeRefKind::Stir(k, args);
            }
            let php = if stir {
                PhpBuiltin::from_name(text)
            } else {
                PhpBuiltin::from_name(&text.to_ascii_lowercase())
            };
            if let Some(b) = php {
                return TypeRefKind::Php(b);
            }
        }
        TypeRefKind::Named(self.scope.resolve_class(text), args)
    }
}

fn lower_literal(node: &SyntaxNode) -> TypeRefKind {
    let toks = node.tokens();
    let negative = toks.first().is_some_and(|t| t.kind() == MINUS);
    let Some(tok) = toks.last() else {
        return TypeRefKind::Error;
    };
    match tok.kind() {
        INT_NUMBER => match parse_int(tok.text()) {
            Some(v) => TypeRefKind::Literal(Literal::Int(if negative { -v } else { v })),
            None => TypeRefKind::Error,
        },
        FLOAT_NUMBER => {
            let s = if negative {
                format!("-{}", tok.text())
            } else {
                tok.text().to_owned()
            };
            TypeRefKind::Literal(Literal::Float(SmolStr::new(s)))
        }
        STRING_SINGLE | STRING_DOUBLE => {
            TypeRefKind::Literal(Literal::String(SmolStr::new(unquote(tok.text()))))
        }
        _ => TypeRefKind::Error,
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
            (b'"', Some(n)) => {
                let mapped = match n {
                    'n' => Some('\n'),
                    't' => Some('\t'),
                    'r' => Some('\r'),
                    'v' => Some('\u{b}'),
                    'e' => Some('\u{1b}'),
                    'f' => Some('\u{c}'),
                    '0' => Some('\0'),
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
