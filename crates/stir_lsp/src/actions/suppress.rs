//! Silencing a diagnostic on its line, with PHPStan's comment, which works here for all codes.

use stir_checker::levels;
use stir_syntax::{SyntaxKind::*, TextRange, TextSize};

use super::style::starts_line;
use super::{Change, Cx, Kind, Offer};
use crate::imports;

pub fn offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    let ours = cx
        .diagnostics()
        .into_iter()
        .map(|d| (d.range, d.code.to_string()));
    let at = cx.raw_range;
    let theirs = cx
        .foreign
        .iter()
        .filter(|d| d.range.start() <= at.end() && at.start() <= d.range.end())
        .map(|d| (d.range, d.code.clone()));
    let mut seen: Vec<(u32, String)> = Vec::new();
    for (range, code) in ours.chain(theirs) {
        if code.is_empty() || !levels::is_ignorable(&code) {
            continue;
        }
        let line = usize::from(imports::line_start(cx.text, range.start())) as u32;
        if seen.contains(&(line, code.clone())) {
            continue;
        }
        seen.push((line, code.clone()));
        let Some(place) = place_of(cx, range.start()) else {
            continue;
        };
        let title = format!("Ignore `{code}` on this line");
        let id = format!("ignore:{code}:{line}");
        let fixed = code.clone();
        out.push(
            Offer::new(id, title, Kind::QuickFix, move || {
                Ok(Change::new(vec![edit_of(cx, place, &code)]))
            })
            .fixing(range, &fixed),
        );
    }
}

/// Where the comment goes.
#[derive(Clone, Copy)]
enum Place {
    /// Add to the comment that's there, whose list of identifiers ends here.
    AddTo(TextSize),
    /// At the end of the line, here.
    EndOfLine(TextSize),
    /// On a line of its own above: the line this code starts, at this indentation.
    Above(TextSize),
}

/// The end of the identifiers in the `@phpstan-ignore` comment `comment`, if it's one that has
/// a list and not a whole line's worth.
fn ignore_list_end(comment: &str) -> Option<usize> {
    const TAG: &str = "@phpstan-ignore";
    let at = comment.rfind(TAG)?;
    let rest = &comment[at + TAG.len()..];
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    // what's in parentheses is why; the list ends before that or before the comment does
    let end = rest
        .find(['(', '\n', '\r'])
        .or_else(|| rest.find("*/"))
        .unwrap_or(rest.len());
    let list = rest[..end].trim_end();
    Some(at + TAG.len() + list.len())
}

/// The end of the line `at` is on, before its line break.
fn end_of_line(text: &str, at: TextSize) -> TextSize {
    let rest = &text[usize::from(at)..];
    let len = rest.find('\n').unwrap_or(rest.len());
    let len = if rest[..len].ends_with('\r') {
        len - 1
    } else {
        len
    };
    at + TextSize::from(len as u32)
}

fn place_of(cx: &Cx<'_>, offset: TextSize) -> Option<Place> {
    let end = end_of_line(cx.text, offset);
    let line_start = imports::line_start(cx.text, offset);
    // the comment that's on the line already, or the line above if that's all there is to it
    for candidate in [end, line_start] {
        let t = cx.root.token_at_offset(candidate).left_biased();
        if let Some(t) = t.filter(|t| t.kind() == COMMENT)
            && !t.text().contains('\n')
        {
            let own_line = candidate == line_start;
            if own_line && !starts_line(cx.text, t.text_range().start()) {
                continue;
            }
            if let Some(list) = ignore_list_end(t.text()) {
                let at = usize::from(t.text_range().start()) + list;
                return Some(Place::AddTo(TextSize::from(at as u32)));
            }
        }
    }
    let token_at_end = cx
        .root
        .token_at_offset(end)
        .left_biased()
        .filter(|t| t.text_range().end() >= end)?;
    let verbatim = |t: &stir_syntax::SyntaxToken| {
        t.text_range().end() > end
            || t.kind() == INLINE_HTML
            || t.kind() == CLOSE_TAG
            || t.kind() == DOC_COMMENT
            || t.parent_ancestors().any(|n| {
                matches!(n.kind(), HEREDOC | INTERP_STRING | SHELL_EXEC | INLINE_HTML_STMT)
            })
            // an ending a comment would be part of
            || t.kind() == COMMENT
    };
    if !verbatim(&token_at_end) && !cx.slice(TextRange::new(line_start, end)).contains("?>") {
        return Some(Place::EndOfLine(end));
    }
    // PHPStan takes a comment on the line above too; that has to be PHP
    if line_start == TextSize::from(0) {
        return None;
    }
    let before = cx.root.token_at_offset(line_start).left_biased()?;
    let in_php = matches!(before.kind(), WHITESPACE | OPEN_TAG)
        && before
            .parent_ancestors()
            .all(|n| !matches!(n.kind(), HEREDOC | INTERP_STRING | SHELL_EXEC));
    let first = cx.root.token_at_offset(line_start).right_biased()?;
    let starts_in_php =
        first.text_range().start() >= line_start || matches!(first.kind(), WHITESPACE);
    (in_php && starts_in_php && first.kind() != INLINE_HTML).then_some(Place::Above(line_start))
}

fn edit_of(cx: &Cx<'_>, place: Place, code: &str) -> super::FileEdit {
    match place {
        Place::AddTo(at) => cx.insert(at, format!(", {code}")),
        Place::EndOfLine(at) => cx.insert(at, format!(" // @phpstan-ignore {code}")),
        Place::Above(at) => {
            let indent: String = cx.text[usize::from(at)..]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            cx.insert(
                at,
                format!("{indent}// @phpstan-ignore {code}{}", cx.style.nl),
            )
        }
    }
}
