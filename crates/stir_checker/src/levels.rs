//! PHPStan-style rule levels for diagnostics in `.php` files, plus
//! `ignoreErrors` and the per-file `// @sphp-nocheck` directive.
//!
//! | level | what is reported |
//! |-------|------------------|
//! | 0 | unknown classes/functions/constants/variables, members on `$this`, wrong argument counts on it, static access, syntax |
//! | 2 | the same on any other value, PHPDoc validation (PHPDoc vs native types, generic arity, bounds, variance), visibility, invalid operators and callables |
//! | 3 | return types, property types, other assignments, PHPDoc types of overrides, iterating and indexing, writes to readonly properties |
//! | 5 | argument types |
//! | 7 | what can be right but isn't always: unions with a right member, `object` for a class, `string` for a `non-empty-string` |
//! | 8 | nullable values, as arguments and when reached through |
//! | 9 | explicit `mixed` |
//! | 10 | implicit `mixed`: no type at all |
//!
//! These follow PHPStan's documented levels and its `RuleLevelHelper`. A code has the level of its
//! rule ([`level_of`]); where it depends on more than that, the checker sets
//! [`Diagnostic::php_level`].

use std::path::Path;

use stir_db::{Diagnostic, IgnoreRule, Settings};

use crate::codes;

/// The rule level of the PHPStan rule behind `code`: the attribute on the rule's class, and
/// `checkThisOnly` and such in its `config.level*.neon`. What only some of a code's reports need
/// (see `RuleLevelHelper`) is on the [`Diagnostic`].
pub fn level_of(code: &str) -> u8 {
    match code {
        codes::ARGUMENT_TYPE => 5,
        codes::RETURN_TYPE
        | codes::RETURN_VOID
        | codes::ASSIGN_PROPERTY_TYPE
        | codes::OFFSET_ASSIGN_VALUE
        | codes::TYPE_MISMATCH
        | codes::PROPERTY_DEFAULT_VALUE
        | codes::PROPERTY_READONLY_NOT_IN_CONSTRUCTOR
        | codes::PROPERTY_READONLY_OUT_OF_CLASS
        | codes::THROW_NOT_THROWABLE
        | codes::FOREACH_NON_ITERABLE
        | codes::OFFSET_NOT_FOUND
        | codes::OFFSET_NOT_ACCESSIBLE => 3,
        codes::CALLABLE_NON_CALLABLE
        | codes::ASSIGN_PROPERTY_PROTECTED_SET
        | codes::ASSIGN_PROPERTY_READ_ONLY
        | codes::METHOD_NON_OBJECT
        | codes::PROPERTY_NON_OBJECT
        | codes::PARAMETER_DEFAULT_VALUE
        | codes::PARAMETER_PHPDOC_TYPE
        | codes::RETURN_PHPDOC_TYPE
        | codes::PROPERTY_PHPDOC_TYPE
        | codes::CLASS_CONSTANT_PHPDOC_TYPE
        | codes::GENERICS_MORE_TYPES
        | codes::GENERICS_NOT_GENERIC
        | codes::GENERICS_NOT_SUBTYPE
        | codes::GENERICS_VARIANCE => 2,
        codes::CONSTANT_NOT_FOUND | codes::NULL_COALESCE_VARIABLE => 1,
        // unknown symbols, argument counts, what PHP itself refuses
        _ => 0,
    }
}

/// The minimum rule level at which `d` is reported in a `.php` file.
pub fn min_level(d: &Diagnostic) -> u8 {
    d.php_level.unwrap_or_else(|| level_of(d.code))
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
            && !is_held_back(d, settings)
            && !rules.iter().any(|r| r.matches(d))
    });
}

/// Composer's packages aren't installed as declared, and `d` is about a name they might declare:
/// the name's not in a namespace of the project's own code.
pub fn is_held_back(d: &Diagnostic, settings: &Settings) -> bool {
    let (Some(own), Some(symbol)) = (&settings.pending_install, &d.symbol) else {
        return false;
    };
    let symbol = symbol.trim_start_matches('\\').to_ascii_lowercase();
    !own.iter().any(|prefix| symbol.starts_with(prefix.as_str()))
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
        .map(|f| diag(codes::VARIABLE_UNDEFINED, "x", f))
        .to_vec();
        filter_php(&mut diags, &settings);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].file, Path::new("/p/src/a.php"));

        // a source directory of `.`
        let mut diags = vec![diag(codes::VARIABLE_UNDEFINED, "x", "/p/./src/gen/c.php")];
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
        assert_eq!(min_level(&diag(codes::FUNCTION_NOT_FOUND, "x", "a.php")), 0);
        assert_eq!(min_level(&diag(codes::ARGUMENT_TYPE, "x", "a.php")), 5);
        assert_eq!(min_level(&diag(codes::RETURN_TYPE, "x", "a.php")), 3);
        assert_eq!(
            min_level(&diag(codes::ARGUMENT_TYPE, "x", "a.php").with_php_level(7)),
            7
        );
        // only the code and the level on the diagnostic count, never what the message says
        assert_eq!(
            min_level(&diag(codes::RETURN_TYPE, "mixed is untyped", "a.php")),
            3
        );

        let settings = Settings {
            php_level: 4,
            ignore_errors: vec![
                IgnoreRule {
                    patterns: vec!["#^unknown function `ig.*`$#i".into()],
                    ..IgnoreRule::default()
                },
                IgnoreRule {
                    identifiers: vec![codes::CLASS_NOT_FOUND.into()],
                    paths: vec!["/p/legacy/*".into()],
                    ..IgnoreRule::default()
                },
            ],
            ..Settings::default()
        };
        let mut diags = vec![
            diag(
                codes::FUNCTION_NOT_FOUND,
                "unknown function `IGNORED`",
                "/p/a.php",
            ),
            diag(
                codes::FUNCTION_NOT_FOUND,
                "unknown function `kept`",
                "/p/a.php",
            ),
            diag(
                codes::ARGUMENT_TYPE,
                "type mismatch in argument `a`",
                "/p/a.php",
            ),
            diag(codes::CLASS_NOT_FOUND, "unknown class", "/p/legacy/x.php"),
            diag(codes::CLASS_NOT_FOUND, "unknown class", "/p/src/x.php"),
        ];
        filter_php(&mut diags, &settings);
        let kept: Vec<_> = diags.iter().map(|d| (d.code, d.file.clone())).collect();
        assert_eq!(
            kept,
            [
                (codes::FUNCTION_NOT_FOUND, "/p/a.php".into()),
                (codes::CLASS_NOT_FOUND, "/p/src/x.php".into()),
            ]
        );
    }
}
