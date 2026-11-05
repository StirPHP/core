//! PHPStan-style rule levels for diagnostics in `.php` files, plus `@phpstan-ignore` comments,
//! `ignoreErrors`, baselines and the per-file `// @stirphp-nocheck` directive.
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

use rustc_hash::FxHashMap;

use stir_db::{Diagnostic, IgnoreRule, IgnoreRules, Settings};
use stir_syntax::{SyntaxKind, SyntaxNode, TextRange, TextSize};

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
    d.php_level.unwrap_or_else(|| level_of(&d.code))
}

/// A leading `// @stirphp-nocheck` comment opts a file out of checking: only the
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
        if c.contains("@stirphp-nocheck") {
            return true;
        }
    }
    false
}

/// Drops `.php` diagnostics above the configured level or outside the analysed paths.
pub fn filter_php(diags: &mut Vec<Diagnostic>, settings: &Settings) {
    let level = settings.level();
    diags.retain(|d| {
        min_level(d) <= level && is_analysed(&d.file, settings) && !is_held_back(d, settings)
    });
}

/// Can `code` be hidden at all? As in PHPStan, what PHP itself can't run can't: it's left out of
/// baselines, and `@phpstan-ignore` and `ignoreErrors` don't apply to it (`RuleErrorBuilder`'s
/// `nonIgnorable()`, and the errors of its parser).
pub fn is_ignorable(code: &str) -> bool {
    !matches!(
        code,
        codes::SYNTAX
            | codes::IGNORE_PARSE_ERROR
            | codes::ASSIGN_THIS
            | codes::NULLSAFE_ASSIGN
            | codes::NAMED_ARGUMENTS_NOT_SUPPORTED
            | codes::CALLABLE_NOT_SUPPORTED
            | codes::CAST_VOID_NOT_SUPPORTED
            | codes::CATCH_NON_CAPTURING_NOT_SUPPORTED
            | codes::CLASS_CONSTANT_DYNAMIC_FETCH
            | codes::CLASS_CONSTANT_FINAL_NOT_SUPPORTED
            | codes::CLASS_CONSTANT_IN_TRAIT
            | codes::CLASS_CONSTANT_NATIVE_TYPE_NOT_SUPPORTED
            | codes::CLASS_CONSTANT_NOT_SUPPORTED
            | codes::CONSTANT_ATTRIBUTES_NOT_SUPPORTED
            | codes::PARAMETER_UNION_NOT_SUPPORTED
            | codes::PROPERTY_HOOKS_NOT_SUPPORTED
            | codes::PROPERTY_PROMOTED_NOT_SUPPORTED
            | codes::PROPERTY_READONLY_NOT_SUPPORTED
            | codes::PROPERTY_STATIC_ASYMMETRIC_VISIBILITY
            | codes::RETURN_NEVER_NOT_SUPPORTED
            | codes::RETURN_UNION_NOT_SUPPORTED
            | codes::THROW_NOT_SUPPORTED
            | codes::SCRIPT_FAILED
            | codes::SCRIPT_NOT_USABLE
    )
}

/// Hides what `rules` cover in `diags`, which are of one file, in order: past a rule's `count`
/// the rest show. Returns how many each rule that applied hid, by the rule's index.
pub fn apply_ignore_rules(
    diags: &mut Vec<Diagnostic>,
    file: &Path,
    rules: &IgnoreRules,
) -> Vec<(u32, u32)> {
    if rules.is_empty() || diags.is_empty() {
        return Vec::new();
    }
    let mut applying: Vec<(u32, &IgnoreRule, u32)> = rules
        .for_file(file, &path_matches)
        .map(|(i, r)| (i, r, 0))
        .collect();
    if applying.is_empty() {
        return Vec::new();
    }
    diags.retain(|d| {
        if !is_ignorable(&d.code) {
            return true;
        }
        let rule = applying.iter_mut().find(|(_, r, hidden)| {
            r.count.is_none_or(|n| *hidden < n)
                && (r.identifiers.is_empty() || r.identifiers.iter().any(|i| *i == d.code))
        });
        match rule {
            Some((_, _, hidden)) => {
                *hidden += 1;
                false
            }
            None => true,
        }
    });
    applying
        .into_iter()
        .filter(|(_, _, hidden)| *hidden > 0)
        .map(|(i, _, hidden)| (i, hidden))
        .collect()
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

/// What the comments of a file say to ignore, by 0-based line: everything, or one error for each
/// time an identifier is listed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LineIgnore {
    All,
    Identifiers(Vec<String>),
}

/// The `@phpstan-ignore` comments of a file, read as PHPStan's `RichParser` does.
#[derive(Debug, Default)]
struct LocalIgnores {
    lines: FxHashMap<usize, LineIgnore>,
    /// Comments it can't read.
    errors: Vec<(TextRange, String)>,
}

impl LocalIgnores {
    fn add(&mut self, line: usize, identifiers: Vec<String>) {
        match self
            .lines
            .entry(line)
            .or_insert_with(|| LineIgnore::Identifiers(Vec::new()))
        {
            LineIgnore::Identifiers(all) => all.extend(identifiers),
            LineIgnore::All => {}
        }
    }

    fn add_parsed(&mut self, line: usize, range: TextRange, text: &str) {
        match parse_ignored_identifiers(text) {
            Ok(identifiers) => self.add(line, identifiers),
            Err(message) => self.errors.push((range, message)),
        }
    }

    fn read(root: &SyntaxNode, source: &str) -> Self {
        let line_of = |offset: TextSize| {
            source.as_bytes()[..usize::from(offset).min(source.len())]
                .iter()
                .filter(|b| **b == b'\n')
                .count()
        };
        let mut out = LocalIgnores::default();
        // the line of the last token that isn't a comment, and a comment alone on its line:
        // that's about the next token, if that's on the next line
        let mut previous = None;
        let mut pending: Option<(String, usize, TextRange)> = None;
        for token in root
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
        {
            let line = line_of(token.text_range().start());
            let kind = token.kind();
            if kind == SyntaxKind::WHITESPACE {
                continue;
            }
            if kind != SyntaxKind::COMMENT && kind != SyntaxKind::DOC_COMMENT {
                if let Some((text, last, range)) = pending.take() {
                    let target = if line == last + 1 { line } else { last };
                    out.add_parsed(target, range, &text);
                }
                previous = Some(line);
                continue;
            }
            let text = token.text();
            let last = line + text.matches('\n').count();
            let next_line = text.contains("@phpstan-ignore-next-line");
            let this_line = text.contains("@phpstan-ignore-line");
            if kind == SyntaxKind::DOC_COMMENT && (next_line || this_line) {
                for (at, _) in text.match_indices("@phpstan-ignore-line") {
                    let target = line + text[..at].matches('\n').count();
                    out.lines.insert(target, LineIgnore::All);
                }
                for (at, _) in text.match_indices("@phpstan-ignore-next-line") {
                    let target = line + text[..at].matches('\n').count() + 1;
                    out.lines.insert(target, LineIgnore::All);
                }
                // the last tag of the comment reaches what's after it
                if next_line && last_phpdoc_tag(text) == Some("@phpstan-ignore-next-line") {
                    out.lines.insert(last + 1, LineIgnore::All);
                }
                continue;
            }
            if next_line || this_line {
                let target = last + usize::from(next_line);
                out.lines.insert(target, LineIgnore::All);
                continue;
            }
            let Some(at) = text.find(IGNORE_TAG) else {
                continue;
            };
            let rest = &text[at + IGNORE_TAG.len()..];
            if previous == Some(line) {
                out.add_parsed(line, token.text_range(), rest);
            } else {
                pending = Some((rest.to_owned(), last, token.text_range()));
            }
        }
        if let Some((text, last, range)) = pending {
            out.add_parsed(last, range, &text);
        }
        out
    }
}

const IGNORE_TAG: &str = "@phpstan-ignore";

/// The last `@tag` at the start of a line of a PHPDoc comment.
fn last_phpdoc_tag(text: &str) -> Option<&str> {
    text.lines().rev().find_map(|line| {
        let line = line.trim_start().trim_start_matches("/**").trim_start();
        let line = line.strip_prefix('*').unwrap_or(line).trim_start();
        line.starts_with('@')
            .then(|| line.split_whitespace().next())
            .flatten()
    })
}

/// What comes after `@phpstan-ignore`: `a.b, c.d (why) ...`, as `RichParser::parseIdentifiers`.
fn parse_ignored_identifiers(text: &str) -> Result<Vec<String>, String> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Token<'a> {
        End,
        Identifier(&'a str),
        Comma,
        Open,
        Close,
        Other(&'a str),
    }
    fn is_identifier(word: &str) -> bool {
        let edge = |c: char| c.is_ascii_alphanumeric();
        word.starts_with(edge)
            && word.ends_with(edge)
            && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
    }
    let mut tokens = Vec::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if c == ' ' || c == '\t' {
            rest = &rest[1..];
        } else if c == ',' {
            tokens.push(Token::Comma);
            rest = &rest[1..];
        } else if c == '(' {
            tokens.push(Token::Open);
            rest = &rest[1..];
        } else if c == ')' {
            tokens.push(Token::Close);
            rest = &rest[1..];
        } else if rest.starts_with("*/") {
            tokens.push(Token::End);
            rest = &rest[2..];
        } else if c == '\n' || rest.starts_with("\r\n") {
            tokens.push(Token::End);
            rest = rest[rest.find('\n').unwrap_or(0) + 1..].trim_start_matches([' ', '\t']);
            if rest.starts_with('*') && !rest.starts_with("*/") {
                rest = rest[1..].strip_prefix(' ').unwrap_or(&rest[1..]);
            }
        } else if c.is_whitespace() {
            rest = &rest[c.len_utf8()..];
        } else {
            let len = rest
                .find(|c: char| c.is_ascii_whitespace() || c == '(' || c == ')')
                .unwrap_or(rest.len());
            let word = &rest[..len];
            let identifier = word
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
                .map_or(word, |end| &word[..end]);
            let identifier = identifier.trim_end_matches('.');
            if is_identifier(identifier) {
                tokens.push(Token::Identifier(identifier));
                rest = &rest[identifier.len()..];
            } else {
                tokens.push(Token::Other(word));
                rest = &rest[len..];
            }
        }
    }
    if tokens.last() != Some(&Token::End) {
        tokens.push(Token::End);
    }

    let label = |t: Token<'_>| match t {
        Token::End => "end".to_owned(),
        Token::Identifier(_) => "identifier".to_owned(),
        Token::Comma => "comma (,)".to_owned(),
        Token::Open => "T_OPEN_PARENTHESIS".to_owned(),
        Token::Close => "T_CLOSE_PARENTHESIS".to_owned(),
        Token::Other(o) => format!("T_OTHER '{o}'"),
    };
    let mut identifiers = Vec::new();
    let mut depth = 0u32;
    let mut after_identifier = false;
    let mut last = "@phpstan-ignore".to_owned();
    let mut in_comment_tail = false;
    for token in tokens {
        if depth > 0 {
            match token {
                Token::Open => depth += 1,
                Token::Close => {
                    depth -= 1;
                    in_comment_tail = depth == 0;
                }
                _ => {}
            }
            last = label(token);
            continue;
        }
        let allowed = match token {
            Token::Identifier(_) => !after_identifier && !in_comment_tail,
            Token::Comma | Token::End => after_identifier || in_comment_tail,
            Token::Open => after_identifier && !in_comment_tail,
            Token::Close | Token::Other(_) => false,
        };
        if !allowed {
            if token == Token::End && identifiers.is_empty() {
                return Err("Missing identifier".to_owned());
            }
            let expected = if after_identifier {
                "comma (,) or end or T_OPEN_PARENTHESIS"
            } else if in_comment_tail {
                "comma (,) or end"
            } else {
                "identifier"
            };
            return Err(format!(
                "Unexpected {} after {last}, expected {expected}",
                label(token)
            ));
        }
        match token {
            Token::Identifier(name) => {
                identifiers.push(name.to_owned());
                after_identifier = true;
            }
            Token::Comma => {
                after_identifier = false;
                in_comment_tail = false;
            }
            Token::Open => depth = 1,
            _ => {}
        }
        last = label(token);
    }
    if depth > 0 {
        return Err("Unexpected end, unclosed opening parenthesis".to_owned());
    }
    if identifiers.is_empty() {
        return Err("Missing identifier".to_owned());
    }
    Ok(identifiers)
}

/// Applies the `@phpstan-ignore` comments of a file to its diagnostics: `-line` and `-next-line`
/// hide all of a line's, and each identifier listed one error of that identifier. Reports the
/// comments it can't read, as PHPStan does.
pub fn apply_phpstan_ignores(
    diags: &mut Vec<Diagnostic>,
    root: &SyntaxNode,
    text: &str,
    file: &Path,
) {
    if !text.contains(IGNORE_TAG) {
        return;
    }
    let mut ignores = LocalIgnores::read(root, text);
    if !diags.is_empty() && !ignores.lines.is_empty() {
        let starts: Vec<usize> = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        diags.retain(|d| {
            if !is_ignorable(&d.code) {
                return true;
            }
            let start = usize::from(d.range.start()).min(text.len());
            let line = starts.partition_point(|&s| s <= start) - 1;
            match ignores.lines.get_mut(&line) {
                Some(LineIgnore::All) => false,
                Some(LineIgnore::Identifiers(listed)) => {
                    match listed.iter().position(|i| *i == d.code) {
                        Some(at) => {
                            listed.remove(at);
                            false
                        }
                        None => true,
                    }
                }
                None => true,
            }
        });
    }
    for (range, message) in ignores.errors {
        diags.push(Diagnostic::error(
            file,
            range,
            codes::IGNORE_PARSE_ERROR,
            format!("Parse error in @phpstan-ignore: {message}"),
        ));
    }
}

pub fn path_matches(rule: &Path, file: &Path) -> bool {
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
        assert!(is_nocheck("<?php // @stirphp-nocheck\n"));
        assert!(is_nocheck(
            "<?php\n/**\n * header\n * @stirphp-nocheck\n */\n"
        ));
        assert!(!is_nocheck("<?php\nfoo(); // @stirphp-nocheck\n"));
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
            ..Settings::default()
        };
        let rules = IgnoreRules::new(vec![IgnoreRule {
            identifiers: vec![codes::CLASS_NOT_FOUND.into()],
            paths: vec!["/p/legacy/*".into()],
            ..IgnoreRule::default()
        }]);
        let mut diags = vec![diag(
            codes::ARGUMENT_TYPE,
            "type mismatch in argument `a`",
            "/p/a.php",
        )];
        filter_php(&mut diags, &settings);
        assert!(diags.is_empty(), "level 5 at level 4");

        let mut diags = vec![diag(
            codes::CLASS_NOT_FOUND,
            "unknown class",
            "/p/legacy/x.php",
        )];
        let hits = apply_ignore_rules(&mut diags, Path::new("/p/legacy/x.php"), &rules);
        assert!(diags.is_empty());
        assert_eq!(hits, [(0, 1)]);

        let mut diags = vec![diag(
            codes::CLASS_NOT_FOUND,
            "unknown class",
            "/p/src/x.php",
        )];
        let hits = apply_ignore_rules(&mut diags, Path::new("/p/src/x.php"), &rules);
        assert_eq!(diags.len(), 1);
        assert!(hits.is_empty());
    }

    /// `(identifier, path, count)` rules over `codes` of one file: what's left, and the hits.
    fn hide(
        rules: Vec<IgnoreRule>,
        file: &str,
        codes: &[&'static str],
    ) -> (Vec<String>, Vec<(u32, u32)>) {
        let mut diags: Vec<_> = codes.iter().map(|c| diag(c, "m", file)).collect();
        let hits = apply_ignore_rules(&mut diags, Path::new(file), &IgnoreRules::new(rules));
        (diags.iter().map(|d| d.code.to_string()).collect(), hits)
    }

    fn rule(identifier: &str, path: &str, count: Option<u32>) -> IgnoreRule {
        IgnoreRule {
            identifiers: if identifier.is_empty() {
                Vec::new()
            } else {
                vec![identifier.into()]
            },
            paths: if path.is_empty() {
                Vec::new()
            } else {
                vec![path.into()]
            },
            count,
            ..IgnoreRule::default()
        }
    }

    #[test]
    fn count_hides_up_to_that_many_per_file() {
        let codes = [codes::RETURN_TYPE; 3];
        let (left, hits) = hide(
            vec![rule(codes::RETURN_TYPE, "/p/a.php", Some(2))],
            "/p/a.php",
            &codes,
        );
        assert_eq!(left, [codes::RETURN_TYPE]);
        assert_eq!(hits, [(0, 2)]);

        // per file, through a glob or a directory too
        for path in ["/p/*", "/p"] {
            let (left, _) = hide(
                vec![rule(codes::RETURN_TYPE, path, Some(1))],
                "/p/b.php",
                &codes,
            );
            assert_eq!(left.len(), 2, "{path}");
        }
        // no count: all of them
        let (left, _) = hide(
            vec![rule(codes::RETURN_TYPE, "/p/a.php", None)],
            "/p/a.php",
            &codes,
        );
        assert!(left.is_empty());
        // two entries of an identifier add up, as a baseline's split by message do
        let (left, hits) = hide(
            vec![
                rule(codes::RETURN_TYPE, "/p/a.php", Some(1)),
                rule(codes::RETURN_TYPE, "/p/a.php", Some(1)),
            ],
            "/p/a.php",
            &codes,
        );
        assert_eq!(left.len(), 1);
        assert_eq!(hits, [(0, 1), (1, 1)]);
    }

    #[test]
    fn rules_by_identifier_path_or_both() {
        let codes = [codes::RETURN_TYPE, codes::ARGUMENT_TYPE];
        // an identifier alone: every file
        let (left, _) = hide(vec![rule(codes::RETURN_TYPE, "", None)], "/q/a.php", &codes);
        assert_eq!(left, [codes::ARGUMENT_TYPE]);
        // a path alone: everything in it
        let (left, _) = hide(vec![rule("", "/q", None)], "/q/a.php", &codes);
        assert!(left.is_empty());
        let (left, _) = hide(vec![rule("", "/r", None)], "/q/a.php", &codes);
        assert_eq!(left.len(), 2);
    }

    #[test]
    fn what_php_refuses_cant_be_hidden() {
        let codes = [codes::SYNTAX, codes::ASSIGN_THIS, codes::RETURN_TYPE];
        let (left, _) = hide(vec![rule("", "/q", None)], "/q/a.php", &codes);
        assert_eq!(left, [codes::SYNTAX, codes::ASSIGN_THIS]);
        let (left, _) = hide(
            vec![rule(codes::SYNTAX, "/q", None)],
            "/q/a.php",
            &[codes::SYNTAX],
        );
        assert_eq!(left, [codes::SYNTAX]);
    }

    fn ignored(source: &str, codes_at: &[(&'static str, usize)]) -> Vec<(usize, String)> {
        let parse = stir_syntax::parse(source);
        let starts: Vec<usize> = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        let mut diags: Vec<Diagnostic> = codes_at
            .iter()
            .map(|(code, line)| {
                let at = TextSize::try_from(starts[*line]).unwrap();
                Diagnostic::error("a.php", TextRange::empty(at), code, "m")
            })
            .collect();
        apply_phpstan_ignores(&mut diags, &parse.syntax(), source, Path::new("a.php"));
        diags
            .iter()
            .map(|d| {
                let line = starts.partition_point(|&s| s <= usize::from(d.range.start())) - 1;
                (line, d.code.to_string())
            })
            .collect()
    }

    #[test]
    fn ignore_comments_name_identifiers() {
        let kept = |c: &str, l: usize| (l, c.to_owned());
        // an identifier only silences itself
        assert_eq!(
            ignored(
                "<?php\nfoo(); // @phpstan-ignore return.type\n",
                &[(codes::RETURN_TYPE, 1), (codes::ARGUMENT_TYPE, 1)]
            ),
            [kept(codes::ARGUMENT_TYPE, 1)]
        );
        // alone on its line: the next one
        assert_eq!(
            ignored(
                "<?php\n// @phpstan-ignore return.type, argument.type (because)\nfoo();\n",
                &[(codes::RETURN_TYPE, 2), (codes::ARGUMENT_TYPE, 2)]
            ),
            []
        );
        // one error for each time it's listed
        assert_eq!(
            ignored(
                "<?php\nfoo(); // @phpstan-ignore return.type\n",
                &[(codes::RETURN_TYPE, 1), (codes::RETURN_TYPE, 1)]
            ),
            [kept(codes::RETURN_TYPE, 1)]
        );
        // the whole line, whatever it is
        assert_eq!(
            ignored(
                "<?php\nfoo(); // @phpstan-ignore-line\n// @phpstan-ignore-next-line\nbar();\n",
                &[(codes::RETURN_TYPE, 1), (codes::ARGUMENT_TYPE, 3)]
            ),
            []
        );
        // in a block comment
        assert_eq!(
            ignored(
                "<?php\n/* @phpstan-ignore return.type */\nfoo();\n",
                &[(codes::RETURN_TYPE, 2)]
            ),
            []
        );
        // what can't be hidden isn't
        assert_eq!(
            ignored(
                "<?php\nfoo(); // @phpstan-ignore-line\n",
                &[(codes::SYNTAX, 1)]
            ),
            [kept(codes::SYNTAX, 1)]
        );
        // a string isn't a comment
        assert_eq!(
            ignored(
                "<?php\n$a = '// @phpstan-ignore-line';\n",
                &[(codes::RETURN_TYPE, 1)]
            ),
            [kept(codes::RETURN_TYPE, 1)]
        );
    }

    #[test]
    fn ignore_comments_that_cant_be_read() {
        assert_eq!(
            parse_ignored_identifiers(" a.b, c.d").unwrap(),
            ["a.b", "c.d"]
        );
        assert_eq!(
            parse_ignored_identifiers(" a.b (why, (ever)), c").unwrap(),
            ["a.b", "c"]
        );
        assert_eq!(parse_ignored_identifiers(" a.b */").unwrap(), ["a.b"]);
        for broken in ["", " ", " , a", " a b", " a,", " a (open", " a) b"] {
            assert!(parse_ignored_identifiers(broken).is_err(), "{broken:?}");
        }
        let errors = ignored("<?php\nfoo(); // @phpstan-ignore\n", &[]);
        assert_eq!(errors, [(1, codes::IGNORE_PARSE_ERROR.to_owned())]);
    }
}
