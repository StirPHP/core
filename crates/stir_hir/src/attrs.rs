//! The phpstorm-stubs attributes that change declarations depending on the
//! target PHP version (see the `stir_stubs` crate docs).

use stir_syntax::{SyntaxKind::*, SyntaxNode, TextSize};

use crate::ast::NodeExt;
use crate::name::Scope;
use crate::type_ref::unquote;

/// What the version-dependent attributes of one declaration say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StubAttrs {
    /// `#[PhpStormStubsElementAvailable]` excludes the element on the target.
    pub unavailable: bool,
    /// `#[LanguageLevelTypeAware]`: the type string for the target and its
    /// offset; an empty string means "no native type".
    pub type_aware: Option<(String, TextSize)>,
}

/// Reads the attributes written directly on `node`.
pub fn stub_attrs(node: &SyntaxNode, scope: &Scope, version: (u8, u8)) -> StubAttrs {
    let mut out = StubAttrs::default();
    for list in node.children_of(ATTRIBUTE_LIST) {
        for group in list.children_of(ATTRIBUTE_GROUP) {
            for attr in group.children_of(ATTRIBUTE) {
                let Some(name) = attr.child(NAME_REF) else {
                    continue;
                };
                let fqn = scope.resolve_class(name.text().to_string().trim());
                let short = fqn.short().to_ascii_lowercase();
                let args = attr.child(ARG_LIST);
                match short.as_str() {
                    "phpstormstubselementavailable" => {
                        let (from, to) = args.map(|a| available_args(&a)).unwrap_or_default();
                        if !stir_stubs::is_available(from.as_deref(), to.as_deref(), version) {
                            out.unavailable = true;
                        }
                    }
                    "languageleveltypeaware" => {
                        if let Some(a) = args {
                            out.type_aware = type_aware(&a, version);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

fn string_of(node: &SyntaxNode) -> Option<(String, TextSize)> {
    let lit = if node.kind() == STRING_LITERAL {
        node.clone()
    } else {
        node.child(STRING_LITERAL)?
    };
    let text = lit.text().to_string();
    let trimmed = text.trim();
    // offset of the string contents (after the quote)
    let lead = (text.len() - text.trim_start().len()) as u32 + 1;
    Some((
        unquote(trimmed),
        lit.text_range().start() + TextSize::new(lead),
    ))
}

/// `(from: 'X', to: 'Y')` or positional `('X', 'Y')`.
fn available_args(args: &SyntaxNode) -> (Option<String>, Option<String>) {
    let mut from = None;
    let mut to = None;
    let mut pos = 0;
    for a in args.children() {
        match a.kind() {
            ARG => {
                let v = string_of(&a).map(|s| s.0);
                if pos == 0 {
                    from = v;
                } else if pos == 1 {
                    to = v;
                }
                pos += 1;
            }
            NAMED_ARG => {
                let v = string_of(&a).map(|s| s.0);
                match a.name_text().as_deref() {
                    Some("from") => from = v,
                    Some("to") => to = v,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    (from, to)
}

fn parse_version(s: &str) -> Option<(u8, u8)> {
    let mut parts = s.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |m| m.parse().ok())?;
    Some((major, minor))
}

/// `(['8.0' => 'T1', …], default: 'T0')`: the entry with the greatest version
/// `<=` target, else `default`.
fn type_aware(args: &SyntaxNode, version: (u8, u8)) -> Option<(String, TextSize)> {
    let mut best: Option<((u8, u8), (String, TextSize))> = None;
    let mut default = None;
    for a in args.children() {
        match a.kind() {
            ARG => {
                let Some(arr) = a.child(ARRAY_EXPR) else {
                    continue;
                };
                for elem in arr.children_of(ARRAY_ELEM) {
                    let strings = elem.children_of(STRING_LITERAL);
                    let [k, v] = strings.as_slice() else {
                        continue;
                    };
                    let (Some((key, _)), Some(value)) = (string_of(k), string_of(v)) else {
                        continue;
                    };
                    let Some(ver) = parse_version(&key) else {
                        continue;
                    };
                    if ver <= version && best.as_ref().is_none_or(|(b, _)| ver > *b) {
                        best = Some((ver, value));
                    }
                }
            }
            NAMED_ARG if a.name_text().as_deref() == Some("default") => {
                default = string_of(&a);
            }
            _ => {}
        }
    }
    best.map(|(_, v)| v).or(default)
}
