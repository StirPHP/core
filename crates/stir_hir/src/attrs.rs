//! The phpstorm-stubs attributes that change declarations depending on the
//! target PHP version (see the `stir_stubs` crate docs).

use stir_db::{Availability, PhpVersion};
use stir_syntax::{SyntaxKind::*, SyntaxNode, TextSize};

use crate::ast::NodeExt;
use crate::name::Scope;
use crate::type_ref::unquote;

/// What the version-dependent attributes of one declaration say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StubAttrs {
    /// `#[PhpStormStubsElementAvailable]`: the versions with the element.
    pub availability: Availability,
    /// `#[LanguageLevelTypeAware]`: the type string for the target and its
    /// offset; an empty string means "no native type".
    pub type_aware: Option<(String, TextSize)>,
    /// `#[Deprecated]`, PHP's own or the stubs'.
    pub deprecated: Option<DeprecatedAttr>,
}

/// What a `#[Deprecated]` says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeprecatedAttr {
    pub reason: String,
    /// `since: '8.1'`
    pub since: Option<String>,
}

/// Reads the attributes written directly on `node`.
pub fn stub_attrs(node: &SyntaxNode, scope: &Scope, version: PhpVersion) -> StubAttrs {
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
                        // both bounds are inclusive and optional; one that isn't a version
                        // is ignored
                        out.availability = Availability {
                            since: from.as_deref().and_then(PhpVersion::parse),
                            removed: to
                                .as_deref()
                                .and_then(PhpVersion::parse)
                                .map(PhpVersion::next),
                        };
                    }
                    "languageleveltypeaware" => {
                        if let Some(a) = args {
                            out.type_aware = type_aware(&a, version);
                        }
                    }
                    "deprecated" => {
                        out.deprecated =
                            Some(args.map(|a| deprecated_args(&a)).unwrap_or_default());
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// `(reason: 'R', since: 'S')` or positional `('R', 'S')`: PHP's own is `(message:, since:)`.
fn deprecated_args(args: &SyntaxNode) -> DeprecatedAttr {
    let mut out = DeprecatedAttr::default();
    let mut pos = 0;
    for a in args.children() {
        match a.kind() {
            ARG => {
                let v = string_of(&a).map(|s| s.0);
                if pos == 0 {
                    out.reason = v.unwrap_or_default();
                } else if pos == 1 {
                    out.since = v;
                }
                pos += 1;
            }
            NAMED_ARG => {
                let v = string_of(&a).map(|s| s.0);
                match a.name_text().as_deref() {
                    Some("reason" | "message") => out.reason = v.unwrap_or_default(),
                    Some("since") => out.since = v,
                    _ => {}
                }
            }
            _ => {}
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

/// What `@since` and `@removed` in a doc comment say. `@removed X` isn't on `X` itself.
pub fn version_tags(doc: &str) -> Availability {
    let earliest = |tag: &str| {
        doc.match_indices(tag)
            .filter_map(|(at, _)| {
                let rest = doc[at + tag.len()..].trim_start_matches([' ', '\t']);
                PhpVersion::parse(rest.split_whitespace().next()?)
            })
            .min()
    };
    Availability {
        since: earliest("@since"),
        removed: earliest("@removed"),
    }
}

/// `(['8.0' => 'T1', …], default: 'T0')`: the entry with the greatest version
/// `<=` target, else `default`.
fn type_aware(args: &SyntaxNode, version: PhpVersion) -> Option<(String, TextSize)> {
    let mut best: Option<(PhpVersion, (String, TextSize))> = None;
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
                    let Some(ver) = PhpVersion::parse(&key) else {
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
