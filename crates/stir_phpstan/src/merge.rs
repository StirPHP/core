//! Showing PHPStan's errors with ours.
//!
//! Two reports are the same when they have the same file, line and identifier, and never because of
//! their messages. Ours stays, because it has a real range where PHPStan only has a line, and it
//! keeps up as you type where PHPStan's results wait for a save.

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

/// `theirs` without what `ours` reports as well, for one file with the text `text`. Each of ours
/// cancels one of theirs at most: two errors on a line with one identifier may be two different
/// errors, and one too many shown beats a real one hidden. A hint isn't a report of ours: it only
/// marks something in the editor, and nobody would see an error about that if PHPStan's went.
pub fn merge(ours: &[Diagnostic], theirs: Vec<Diagnostic>, text: &str) -> Vec<Diagnostic> {
    if theirs.is_empty() {
        return theirs;
    }
    let starts = line_starts(text);
    let line_of = |range: TextRange| starts.partition_point(|&s| s <= range.start().into()) - 1;
    let mut left: FxHashMap<_, usize> = FxHashMap::default();
    let mut syntax_error = false;
    for d in ours {
        syntax_error |= d.code == codes::SYNTAX;
        if d.severity != Severity::Hint {
            *left.entry((line_of(d.range), d.code.clone())).or_default() += 1;
        }
    }
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
            match left.get_mut(&(line_of(d.range), d.code.clone())) {
                Some(count) if *count > 0 => {
                    *count -= 1;
                    false
                }
                _ => true,
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
    fn ours_are_left_as_they_are_whatever_phpstan_says() {
        // nothing here takes anything away from ours: what's returned is only theirs
        assert!(merge(&[ours("return.type")], Vec::new(), TEXT).is_empty());
    }
}
