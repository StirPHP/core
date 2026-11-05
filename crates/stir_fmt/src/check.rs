//! That formatting changed nothing but whitespace.

use stir_syntax::SyntaxKind::*;

/// Do both have the same tokens, whitespace aside? A comment may differ in the whitespace at the
/// ends of its lines, as those are whitespace too.
pub fn same_tokens(a: &str, b: &str) -> bool {
    let (la, lb) = (stir_syntax::lex(a), stir_syntax::lex(b));
    let significant = |text: &'_ str, tokens: &[stir_syntax::Token]| -> Vec<(u16, String)> {
        let mut at = 0usize;
        let mut out = Vec::with_capacity(tokens.len());
        for t in tokens {
            let len = t.len as usize;
            let piece = &text[at..at + len];
            at += len;
            if t.kind == WHITESPACE {
                continue;
            }
            let piece = if matches!(t.kind, COMMENT | DOC_COMMENT) {
                piece.lines().map(str::trim).collect::<Vec<_>>().join("\n")
            } else {
                piece.to_owned()
            };
            out.push((t.kind as u16, piece));
        }
        out
    };
    significant(a, &la.tokens) == significant(b, &lb.tokens)
}
