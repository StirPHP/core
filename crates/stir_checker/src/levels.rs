//! PHPStan-style rule levels for diagnostics in `.php` files, plus
//! `ignoreErrors` and the per-file `// @sphp-check` / `// @sphp-nocheck`
//! directives. `.sphp` files are always fully checked.
//!
//! | level | what is reported |
//! |-------|------------------|
//! | 0 | unknown classes/functions/methods/constants/variables, wrong argument counts, syntax |
//! | 2 | PHPDoc validation (PHPDoc vs native types, generic arity, bounds), visibility, invalid operands |
//! | 3 | return types, property types, other assignments |
//! | 4 | always-true/false conditions (unnecessary null checks, incompatible comparisons) |
//! | 5 | argument types |
//! | 6 | missing typehints (not reported for `.php`) |
//! | 7 | partially wrong unions (needs a [`Diagnostic::php_level`] tag) |
//! | 8 | member access on nullable types |
//! | 9 | strictness about `mixed` / untyped values |
//! | 10 | implicit `mixed` (needs a [`Diagnostic::php_level`] tag) |

use std::path::Path;

use stir_db::{Diagnostic, IgnoreRule, Settings};

use crate::codes;

/// A PHPDoc type that is not a subtype of the native type it documents.
pub const PHPDOC_MISMATCH: &str = "SPHP0050";

/// The minimum rule level at which `d` is reported in a `.php` file.
pub fn min_level(d: &Diagnostic) -> u8 {
    if let Some(l) = d.php_level {
        return l;
    }
    let untyped =
        d.message.contains("untyped") || d.message.contains("Any!") || d.message.contains("mixed");
    match d.code {
        codes::TYPE_MISMATCH if untyped => 9,
        codes::TYPE_MISMATCH if d.message.contains("in argument") => 5,
        codes::TYPE_MISMATCH | codes::RETURN_VALUE => 3,
        codes::UNNECESSARY_NULL_CHECK | codes::INCOMPATIBLE_COMPARISON => 4,
        codes::MISSING_TYPE => 6,
        codes::NULLABLE_ACCESS => 8,
        codes::ANY_PLATFORM_ESCAPE => 9,
        codes::INVALID_OPERAND | codes::NOT_CALLABLE if untyped => 9,
        codes::INVALID_OPERAND
        | codes::NOT_CALLABLE
        | codes::VISIBILITY
        | codes::GENERIC_ARITY
        | codes::TYPE_BOUND
        | PHPDOC_MISMATCH => 2,
        // Unknown symbols, argument counts, syntax and everything StirPHP-only.
        _ => 0,
    }
}

/// A leading `// @sphp-check` (`Some(true)`) or `// @sphp-nocheck` (`Some(false)`)
/// comment: only the comments before the first statement count.
pub fn directive(text: &str) -> Option<bool> {
    let mut in_block = false;
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        let comment = if in_block {
            if l.contains("*/") {
                in_block = false;
            }
            Some(l)
        } else if let Some(rest) = l.strip_prefix("<?php") {
            if rest.trim().is_empty() {
                continue;
            }
            Some(rest.trim())
        } else if l.starts_with("//") || l.starts_with('#') && !l.starts_with("#[") {
            Some(l)
        } else if l.starts_with("/*") {
            in_block = !l.contains("*/");
            Some(l)
        } else {
            None
        };
        let c = comment?;
        if c.contains("@sphp-nocheck") {
            return Some(false);
        }
        if c.contains("@sphp-check") {
            return Some(true);
        }
    }
    None
}

/// Is a project `.php` file checked under `settings`?
pub fn php_checked(text: &str, settings: &Settings) -> bool {
    directive(text).unwrap_or(settings.check_php)
}

/// Drops `.php` diagnostics above the configured level or matched by `ignoreErrors`.
pub fn filter_php(diags: &mut Vec<Diagnostic>, settings: &Settings) {
    let level = settings.level();
    let rules: Vec<CompiledRule<'_>> = settings
        .ignore_errors
        .iter()
        .map(CompiledRule::new)
        .collect();
    diags.retain(|d| min_level(d) <= level && !rules.iter().any(|r| r.matches(d)));
}

struct CompiledRule<'a> {
    rule: &'a IgnoreRule,
    patterns: Vec<fancy_regex::Regex>,
}

impl<'a> CompiledRule<'a> {
    fn new(rule: &'a IgnoreRule) -> Self {
        CompiledRule {
            rule,
            patterns: rule.patterns.iter().filter_map(|p| pcre(p)).collect(),
        }
    }

    fn matches(&self, d: &Diagnostic) -> bool {
        let r = self.rule;
        let has_message = !r.patterns.is_empty() || !r.raw_messages.is_empty();
        if !has_message && r.identifiers.is_empty() {
            return false;
        }
        let message_ok = !has_message
            || self
                .patterns
                .iter()
                .any(|p| p.is_match(&d.message).unwrap_or(false))
            || r.raw_messages.contains(&d.message);
        let id_ok = r.identifiers.is_empty() || r.identifiers.iter().any(|i| i == d.code);
        let path_ok = r.paths.is_empty() || r.paths.iter().any(|p| path_matches(p, &d.file));
        message_ok && id_ok && path_ok
    }
}

/// Compiles a PCRE pattern with delimiters and flags (`#^Foo$#i`).
fn pcre(pattern: &str) -> Option<fancy_regex::Regex> {
    let mut chars = pattern.chars();
    let open = chars.next()?;
    let close = match open {
        '(' => ')',
        '{' => '}',
        '[' => ']',
        '<' => '>',
        c if c.is_alphanumeric() || c == '\\' || c.is_whitespace() => {
            return fancy_regex::Regex::new(pattern).ok();
        }
        c => c,
    };
    let end = pattern.rfind(close).filter(|&e| e > 0)?;
    let body = &pattern[open.len_utf8()..end];
    let flags: String = pattern[end + close.len_utf8()..]
        .chars()
        .filter(|c| matches!(c, 'i' | 'm' | 's' | 'x'))
        .collect();
    let src = if flags.is_empty() {
        body.to_owned()
    } else {
        format!("(?{flags}){body}")
    };
    fancy_regex::Regex::new(&src).ok()
}

fn path_matches(rule: &Path, file: &Path) -> bool {
    let r = rule.to_string_lossy();
    if r.contains(['*', '?']) {
        fnmatch(r.as_bytes(), file.to_string_lossy().as_bytes())
    } else {
        file == rule || file.starts_with(rule)
    }
}

/// `fnmatch()` without flags: `*` and `?` also match `/`.
fn fnmatch(pat: &[u8], s: &[u8]) -> bool {
    match pat.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|i| fnmatch(rest, &s[i..])),
        Some((b'?', rest)) => !s.is_empty() && fnmatch(rest, &s[1..]),
        Some((c, rest)) => s.first() == Some(c) && fnmatch(rest, &s[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stir_syntax::TextRange;

    fn diag(code: &'static str, msg: &str, file: &str) -> Diagnostic {
        Diagnostic::error(file, TextRange::default(), code, msg)
    }

    #[test]
    fn directives() {
        assert_eq!(directive("<?php\n// @sphp-check\nfoo();"), Some(true));
        assert_eq!(directive("<?php // @sphp-nocheck\n"), Some(false));
        assert_eq!(
            directive("<?php\n/**\n * header\n * @sphp-check\n */\n"),
            Some(true)
        );
        assert_eq!(directive("<?php\nfoo(); // @sphp-check\n"), None);
        assert_eq!(directive("<?php\n\nnamespace A;\n"), None);
    }

    #[test]
    fn levels_and_ignores() {
        assert_eq!(min_level(&diag(codes::UNDEFINED_FUNCTION, "x", "a.php")), 0);
        assert_eq!(
            min_level(&diag(
                codes::TYPE_MISMATCH,
                "type mismatch in argument `a`",
                "a.php"
            )),
            5
        );
        assert_eq!(
            min_level(&diag(
                codes::TYPE_MISMATCH,
                "type mismatch in return value",
                "a.php"
            )),
            3
        );
        assert_eq!(
            min_level(&diag(codes::TYPE_MISMATCH, "x", "a.php").with_php_level(7)),
            7
        );

        let settings = Settings {
            php_level: Some(4),
            ignore_errors: vec![
                IgnoreRule {
                    patterns: vec!["#^unknown function `ig.*`$#i".into()],
                    ..IgnoreRule::default()
                },
                IgnoreRule {
                    identifiers: vec![codes::UNDEFINED_CLASS.into()],
                    paths: vec!["/p/legacy/*".into()],
                    ..IgnoreRule::default()
                },
            ],
            ..Settings::default()
        };
        let mut diags = vec![
            diag(
                codes::UNDEFINED_FUNCTION,
                "unknown function `IGNORED`",
                "/p/a.php",
            ),
            diag(
                codes::UNDEFINED_FUNCTION,
                "unknown function `kept`",
                "/p/a.php",
            ),
            diag(
                codes::TYPE_MISMATCH,
                "type mismatch in argument `a`",
                "/p/a.php",
            ),
            diag(codes::UNDEFINED_CLASS, "unknown class", "/p/legacy/x.php"),
            diag(codes::UNDEFINED_CLASS, "unknown class", "/p/src/x.php"),
        ];
        filter_php(&mut diags, &settings);
        let kept: Vec<_> = diags.iter().map(|d| (d.code, d.file.clone())).collect();
        assert_eq!(
            kept,
            [
                (codes::UNDEFINED_FUNCTION, "/p/a.php".into()),
                (codes::UNDEFINED_CLASS, "/p/src/x.php".into()),
            ]
        );
    }
}
