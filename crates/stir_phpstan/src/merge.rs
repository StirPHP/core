//! Showing PHPStan's errors with ours.
//!
//! Two reports are the same when they have the same file, line and identifier, and never because of
//! their messages. Ours stays, because it has a real range where PHPStan only has a line, and it
//! keeps up as you type where PHPStan's results wait for a save. What's wrong in a doc comment is
//! at the comment for us and at what it documents for PHPStan, and counts as either.

use std::path::Path;

use rustc_hash::FxHashMap;
use stir_db::{Diagnostic, Severity, codes};
use stir_syntax::{TextRange, TextSize};

use crate::report::Reported;

/// PHPStan's identifier for a syntax error, where we have [`codes::SYNTAX`].
const PHPSTAN_SYNTAX: &str = "phpstan.parse";

/// Where each line of `text` starts.
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// The text of line `line` (from 1) without the whitespace around it. Past the end of the file,
/// it's the end of the file.
fn line_range(text: &str, starts: &[usize], line: u32) -> TextRange {
    let at = |offset: usize| TextSize::try_from(offset).unwrap_or_default();
    let index = line.saturating_sub(1) as usize;
    let Some(&start) = starts.get(index) else {
        return TextRange::empty(at(text.len()));
    };
    let end = starts.get(index + 1).map_or(text.len(), |&next| next);
    let whole = &text[start..end];
    let trimmed = whole.trim();
    if trimmed.is_empty() {
        return TextRange::empty(at(start));
    }
    let leading = whole.len() - whole.trim_start().len();
    TextRange::new(at(start + leading), at(start + leading + trimmed.len()))
}

/// `reported`, as diagnostics of the file `path` whose text is `text`, the one PHPStan read.
pub fn diagnostics(path: &Path, text: &str, reported: &[Reported]) -> Vec<Diagnostic> {
    let starts = line_starts(text);
    reported
        .iter()
        .map(|r| {
            let range = line_range(text, &starts, r.line.unwrap_or(1));
            let code = r.identifier.as_deref().unwrap_or_default();
            let mut d = Diagnostic::custom(path, range, Severity::Error, code, r.message.clone());
            d.from_phpstan = true;
            if let Some(tip) = &r.tip {
                d = d.with_note(tip.clone());
            }
            d
        })
        .collect()
}

/// The line of what the doc comment around `offset` documents, if it's in one.
fn line_documented(text: &str, starts: &[usize], offset: usize) -> Option<usize> {
    let before = &text[..offset];
    let open = before.rfind("/*")?;
    if before[open..].contains("*/") {
        return None;
    }
    let close = offset + text[offset..].find("*/")? + 2;
    let after = &text[close..];
    let code = close + after.len() - after.trim_start().len();
    Some(starts.partition_point(|&s| s <= code) - 1)
}

/// `theirs` without what `ours` reports as well, for one file with the text `text`. Each of ours
/// cancels one of theirs at most: two errors on a line with one identifier may be two different
/// errors, and one too many shown beats a real one hidden. A hint isn't a report of ours: it only
/// marks something in the editor, and nobody would see an error about that if PHPStan's went.
pub fn merge(ours: &[Diagnostic], theirs: Vec<Diagnostic>, text: &str) -> Vec<Diagnostic> {
    if theirs.is_empty() {
        return theirs;
    }
    let starts = line_starts(text);
    let line_of = |offset: usize| starts.partition_point(|&s| s <= offset) - 1;
    // which of ours answer to a line and identifier
    let mut places: FxHashMap<_, Vec<usize>> = FxHashMap::default();
    let mut syntax_error = false;
    for (index, d) in ours.iter().enumerate() {
        syntax_error |= d.code == codes::SYNTAX;
        if d.severity == Severity::Hint {
            continue;
        }
        let start = usize::from(d.range.start());
        places
            .entry((line_of(start), d.code.clone()))
            .or_default()
            .push(index);
        if let Some(line) = line_documented(text, &starts, start) {
            places
                .entry((line, d.code.clone()))
                .or_default()
                .push(index);
        }
    }
    let mut used = vec![false; ours.len()];
    theirs
        .into_iter()
        .filter(|d| {
            // the same error, though each parser words and places it its own way
            if syntax_error && d.code == PHPSTAN_SYNTAX {
                return false;
            }
            if d.code.is_empty() {
                return true;
            }
            let line = line_of(d.range.start().into());
            let same = places
                .get(&(line, d.code.clone()))
                .and_then(|candidates| candidates.iter().find(|i| !used[**i]));
            match same {
                Some(&index) => {
                    used[index] = true;
                    false
                }
                None => true,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "<?php\n\nfunction f(): int {\n    return 'x';\n}\n";

    fn reported(line: Option<u32>, identifier: Option<&str>, message: &str) -> Reported {
        Reported {
            line,
            message: message.to_owned(),
            identifier: identifier.map(str::to_owned),
            tip: None,
            ignorable: true,
        }
    }

    /// One of ours, on the `'x'` of line 4.
    fn ours(code: &'static str) -> Diagnostic {
        let at = TEXT.find("'x'").unwrap() as u32;
        Diagnostic::error(
            "/a.php",
            TextRange::new(at.into(), (at + 3).into()),
            code,
            "ours",
        )
    }

    fn theirs(items: &[Reported]) -> Vec<Diagnostic> {
        diagnostics(Path::new("/a.php"), TEXT, items)
    }

    #[test]
    fn a_line_is_its_text_without_the_indentation() {
        let d = &theirs(&[reported(Some(4), Some("return.type"), "theirs")])[0];
        assert_eq!(&TEXT[d.range], "return 'x';");
        assert!(d.from_phpstan);
        assert_eq!(d.severity, Severity::Error);
    }

    #[test]
    fn lines_at_the_edges() {
        // about the file as a whole: its first line
        let d = &theirs(&[reported(None, Some("ignore.unmatched"), "theirs")])[0];
        assert_eq!(&TEXT[d.range], "<?php");
        // an empty line, and one past the end of an edited file
        let d = &theirs(&[reported(Some(2), None, "theirs")])[0];
        assert!(d.range.is_empty());
        let d = &theirs(&[reported(Some(99), None, "theirs")])[0];
        assert_eq!(usize::from(d.range.start()), TEXT.len());
    }

    #[test]
    fn carriage_returns_are_not_part_of_a_line() {
        let text = "<?php\r\n  foo();\r\n";
        let d = &diagnostics(Path::new("/a.php"), text, &[reported(Some(2), None, "x")])[0];
        assert_eq!(&text[d.range], "foo();");
    }

    #[test]
    fn the_same_line_and_identifier_is_one_report_and_ours_stays() {
        let theirs = theirs(&[reported(Some(4), Some("return.type"), "worded otherwise")]);
        assert!(merge(&[ours("return.type")], theirs, TEXT).is_empty());
    }

    #[test]
    fn another_line_or_identifier_is_another_report() {
        let kept = merge(
            &[ours("return.type")],
            theirs(&[
                reported(Some(3), Some("return.type"), "another line"),
                reported(Some(4), Some("argument.type"), "another identifier"),
            ]),
            TEXT,
        );
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn messages_do_not_count() {
        let mut same_words = ours("SPHP0061");
        same_words.message = "same words".into();
        let kept = merge(
            &[same_words],
            theirs(&[reported(Some(4), Some("return.type"), "same words")]),
            TEXT,
        );
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn no_identifier_can_not_be_recognized_and_stays() {
        let kept = merge(
            &[ours("return.type")],
            theirs(&[reported(Some(4), None, "no identifier")]),
            TEXT,
        );
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].code, "");
    }

    #[test]
    fn one_of_ours_cancels_one_of_theirs() {
        let two = [
            reported(Some(4), Some("return.type"), "one"),
            reported(Some(4), Some("return.type"), "two"),
        ];
        let kept = merge(&[ours("return.type")], theirs(&two), TEXT);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].message, "two");
        assert!(
            merge(
                &[ours("return.type"), ours("return.type")],
                theirs(&two),
                TEXT
            )
            .is_empty()
        );
    }

    #[test]
    fn a_hint_of_ours_leaves_their_error() {
        let hint = ours("function.deprecated").with_severity(Severity::Hint);
        let kept = merge(
            &[hint],
            theirs(&[reported(Some(4), Some("function.deprecated"), "theirs")]),
            TEXT,
        );
        assert_eq!(kept.len(), 1);
        let warning = ours("function.deprecated").with_severity(Severity::Warning);
        let kept = merge(
            &[warning],
            theirs(&[reported(Some(4), Some("function.deprecated"), "theirs")]),
            TEXT,
        );
        assert!(kept.is_empty());
    }

    #[test]
    fn a_syntax_error_is_one_whichever_line_each_parser_blames() {
        let parse = [reported(Some(3), Some(PHPSTAN_SYNTAX), "theirs")];
        assert!(merge(&[ours(codes::SYNTAX)], theirs(&parse), TEXT).is_empty());
        assert_eq!(merge(&[ours("return.type")], theirs(&parse), TEXT).len(), 1);
    }

    #[test]
    fn what_is_wrong_in_a_doc_comment_is_at_the_comment_or_at_what_it_documents() {
        let text = "<?php\n/**\n * @param array<int> $x\n */\n\nfunction f(int $x) {}\nfoo();\n";
        let at = text.find("array<int>").unwrap() as u32;
        let ours = |code| {
            Diagnostic::error(
                "/a.php",
                TextRange::new(at.into(), (at + 10).into()),
                code,
                "ours",
            )
        };
        let theirs = |line| {
            diagnostics(
                Path::new("/a.php"),
                text,
                &[reported(Some(line), Some("parameter.phpDocType"), "theirs")],
            )
        };
        let doc = [ours("parameter.phpDocType")];
        // at the function, and in the comment itself
        assert!(merge(&doc, theirs(6), text).is_empty());
        assert!(merge(&doc, theirs(3), text).is_empty());
        // not further on
        assert_eq!(merge(&doc, theirs(7), text).len(), 1);
        // not another identifier
        assert_eq!(merge(&[ours("class.notFound")], theirs(6), text).len(), 1);
        // and one of ours is still one report, wherever it answers
        let mut two = theirs(6);
        two.extend(theirs(3));
        assert_eq!(merge(&doc, two, text).len(), 1);
    }

    #[test]
    fn code_after_a_closed_comment_is_not_in_it() {
        let text = "<?php\n/** doc */\nfoo();\nbar();\n";
        let starts = line_starts(text);
        assert_eq!(
            line_documented(text, &starts, text.find("doc").unwrap()),
            Some(2)
        );
        assert_eq!(
            line_documented(text, &starts, text.find("foo").unwrap()),
            None
        );
        assert_eq!(
            line_documented(text, &starts, text.find("bar").unwrap()),
            None
        );
        assert_eq!(line_documented("<?php\nfoo();\n", &starts, 6), None);
    }

    #[test]
    fn ours_are_left_as_they_are_whatever_phpstan_says() {
        // nothing here takes anything away from ours: what's returned is only theirs
        assert!(merge(&[ours("return.type")], Vec::new(), TEXT).is_empty());
    }
}
