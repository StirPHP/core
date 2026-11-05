//! PHPStan-style rule levels for diagnostics in `.php` files, plus
//! `ignoreErrors` and the per-file `// @sphp-nocheck` directive.
//!
//! | level | what is reported |
//! |-------|------------------|
//! | 0 | unknown classes/functions/methods/constants/variables, wrong argument counts, syntax |
//! | 2 | PHPDoc validation (PHPDoc vs native types, generic arity, bounds), visibility, invalid operands |
//! | 3 | return types, property types, other assignments |
//! | 5 | argument types |
//! | 6 | overrides (needs a [`Diagnostic::php_level`] tag) |
//! | 7 | partially wrong unions (needs a [`Diagnostic::php_level`] tag) |
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
    // an untyped value prints as `mixed`
    let untyped = d.message.contains("untyped") || d.message.contains("mixed");
    match d.code {
        codes::TYPE_MISMATCH if untyped => 9,
        codes::TYPE_MISMATCH if d.message.contains("in argument") => 5,
        codes::TYPE_MISMATCH | codes::RETURN_VALUE => 3,
        codes::INVALID_OPERAND | codes::NOT_CALLABLE if untyped => 9,
        codes::INVALID_OPERAND
        | codes::NOT_CALLABLE
        | codes::VISIBILITY
        | codes::GENERIC_ARITY
        | codes::TYPE_BOUND
        | PHPDOC_MISMATCH => 2,
        // Unknown symbols, argument counts and syntax.
        _ => 0,
    }
}

/// A leading `// @sphp-nocheck` comment opts a file out of checking: only the
/// comments before the first statement count.
pub fn is_nocheck(text: &str) -> bool {
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
        let Some(c) = comment else {
            return false;
        };
        if c.contains("@sphp-nocheck") {
            return true;
        }
    }
    false
}

/// Drops `.php` diagnostics above the configured level, matched by `ignoreErrors`
/// or outside the analysed paths.
pub fn filter_php(diags: &mut Vec<Diagnostic>, settings: &Settings) {
    let level = settings.level();
    let rules: Vec<CompiledRule<'_>> = settings
        .ignore_errors
        .iter()
        .map(CompiledRule::new)
        .collect();
    diags.retain(|d| {
        min_level(d) <= level
            && is_analysed(&d.file, settings)
            && !rules.iter().any(|r| r.matches(d))
    });
}

/// Is `file` within the PHPStan `paths` and outside its `excludePaths`?
pub fn is_analysed(file: &Path, settings: &Settings) -> bool {
    (settings.php_paths.is_empty() || settings.php_paths.iter().any(|p| path_matches(p, file)))
        && !settings.php_excludes.iter().any(|p| path_matches(p, file))
}

/// Drops diagnostics on lines silenced by `@phpstan-ignore-line`,
/// `@phpstan-ignore-next-line` or `@phpstan-ignore <identifier>` (the next line if the
/// comment is alone on its line). The identifiers are PHPStan's, so any error counts.
pub fn filter_phpstan_ignores(diags: &mut Vec<Diagnostic>, text: &str) {
    let marker = "@phpstan-ignore";
    let mut ignored = std::collections::HashSet::new();
    for (i, line) in text.lines().enumerate() {
        let Some(at) = line.find(marker) else {
            continue;
        };
        let rest = &line[at + marker.len()..];
        let before = line[..at].trim_start();
        let comment_only =
            before.starts_with("//") || before.starts_with('#') || before.starts_with("/*");
        if rest.starts_with("-next-line") || (!rest.starts_with("-line") && comment_only) {
            ignored.insert(i + 1);
        } else {
            ignored.insert(i);
        }
    }
    if ignored.is_empty() {
        return;
    }
    diags.retain(|d| {
        let start = usize::from(d.range.start()).min(text.len());
        let line = text.as_bytes()[..start]
            .iter()
            .filter(|b| **b == b'\n')
            .count();
        !ignored.contains(&line)
    });
}

/// An ignore rule whose patterns are compiled the first time a diagnostic reaches them: a
/// baseline has thousands of rules, and nearly all are for some other file.
struct CompiledRule<'a> {
    rule: &'a IgnoreRule,
    patterns: std::cell::OnceCell<Vec<fancy_regex::Regex>>,
}

impl<'a> CompiledRule<'a> {
    fn new(rule: &'a IgnoreRule) -> Self {
        CompiledRule {
            rule,
            patterns: std::cell::OnceCell::new(),
        }
    }

    fn matches(&self, d: &Diagnostic) -> bool {
        let r = self.rule;
        let has_message = !r.patterns.is_empty() || !r.raw_messages.is_empty();
        if !has_message && r.identifiers.is_empty() {
            return false;
        }
        if !r.paths.is_empty() && !r.paths.iter().any(|p| path_matches(p, &d.file)) {
            return false;
        }
        if !r.identifiers.is_empty() && !r.identifiers.iter().any(|i| i == d.code) {
            return false;
        }
        !has_message
            || r.raw_messages.contains(&d.message)
            || self
                .patterns
                .get_or_init(|| r.patterns.iter().filter_map(|p| pcre(p)).collect())
                .iter()
                .any(|p| p.is_match(&d.message).unwrap_or(false))
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
        // a source directory of `.` leaves `/./` in the file's path, which `components` drops
        let file: std::path::PathBuf = file.components().collect();
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

    #[test]
    fn analysed_paths() {
        let settings = Settings {
            php_paths: vec!["/p/src".into()],
            php_excludes: vec!["/p/src/lib".into(), "*/gen/*".into()],
            ..Settings::default()
        };
        let mut diags = [
            "/p/src/a.php",
            "/p/src/lib/b.php",
            "/p/src/gen/c.php",
            "/p/other/d.php",
        ]
        .map(|f| diag(codes::UNDEFINED_VARIABLE, "x", f))
        .to_vec();
        filter_php(&mut diags, &settings);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].file, Path::new("/p/src/a.php"));

        // a source directory of `.`
        let mut diags = vec![diag(codes::UNDEFINED_VARIABLE, "x", "/p/./src/gen/c.php")];
        filter_php(&mut diags, &settings);
        assert!(diags.is_empty());
    }

    fn diag(code: &'static str, msg: &str, file: &str) -> Diagnostic {
        Diagnostic::error(file, TextRange::default(), code, msg)
    }

    #[test]
    fn nocheck_directive() {
        assert!(is_nocheck("<?php // @sphp-nocheck\n"));
        assert!(is_nocheck("<?php\n/**\n * header\n * @sphp-nocheck\n */\n"));
        assert!(!is_nocheck("<?php\nfoo(); // @sphp-nocheck\n"));
        assert!(!is_nocheck("<?php\n\nnamespace A;\n"));
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
        // an untyped value prints as `mixed`
        assert_eq!(
            min_level(&diag(
                codes::TYPE_MISMATCH,
                "type mismatch in argument `a`: mixed is not assignable to int",
                "a.php"
            )),
            9
        );
        assert_eq!(
            min_level(&diag(
                codes::TYPE_MISMATCH,
                "type mismatch in property: narrow the untyped value first",
                "a.php"
            )),
            9
        );

        let settings = Settings {
            php_level: 4,
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
