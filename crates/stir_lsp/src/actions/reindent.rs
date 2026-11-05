//! Moving code to another level of indentation, which mustn't change what's in a string.

use stir_syntax::{SyntaxKind::*, SyntaxNode, TextRange, TextSize};

/// The tokens whose text can't change: a line break in one is part of a value, or of how it ends.
fn is_verbatim(root: &SyntaxNode, line_start: TextSize) -> bool {
    if line_start == TextSize::from(0) {
        return false;
    }
    // what the line break before the line belongs to
    let Some(t) = root
        .token_at_offset(line_start)
        .left_biased()
        .filter(|t| t.text_range().end() >= line_start)
    else {
        return false;
    };
    if t.kind() == COMMENT || t.kind() == DOC_COMMENT {
        return false;
    }
    if t.kind() == WHITESPACE {
        return false;
    }
    if t.kind() == OPEN_TAG {
        return false;
    }
    // inside a heredoc, a string or inline HTML, or between the parts of one
    t.text_range().end() > line_start
        || t.parent_ancestors().any(|n| {
            matches!(
                n.kind(),
                HEREDOC | INTERP_STRING | STRING_LITERAL | SHELL_EXEC | INLINE_HTML_STMT
            )
        })
}

/// `range` of the file as text whose lines are `from` deeper than `to`, for putting it elsewhere.
/// Lines inside a string or heredoc stay as they are: their closing marker can stay further in
/// than the code around it, but a line less far in than the marker wouldn't parse, and either way
/// its text is what the string says. Only the lines after the first change.
pub fn reindented(
    root: &SyntaxNode,
    text: &str,
    range: TextRange,
    from: &str,
    to: &str,
    nl: &str,
) -> String {
    let body = &text[usize::from(range.start())..usize::from(range.end())];
    let mut out = String::with_capacity(body.len());
    let mut offset = usize::from(range.start());
    for (i, piece) in body.split_inclusive('\n').enumerate() {
        let start = TextSize::from(offset as u32);
        offset += piece.len();
        if i == 0 || is_verbatim(root, start) {
            out.push_str(piece);
            continue;
        }
        let (line, ending) = match piece.strip_suffix("\r\n") {
            Some(l) => (l, "\r\n"),
            None => match piece.strip_suffix('\n') {
                Some(l) => (l, "\n"),
                None => (piece, ""),
            },
        };
        let _ = nl;
        if line.trim().is_empty() {
            out.push_str(ending);
            continue;
        }
        match line.strip_prefix(from) {
            Some(rest) => {
                out.push_str(to);
                out.push_str(rest);
            }
            // less far in than expected: as far in, whatever it was, as it can stay
            None => out.push_str(line),
        }
        out.push_str(ending);
    }
    out
}
