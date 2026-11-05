//! Adding a tag to a declaration's doc comment.

use stir_hir::ast::NodeExt;
use stir_syntax::{SyntaxNode, TextRange, TextSize};

use super::style::{indent_at, starts_line};
use super::{Cx, FileEdit};

/// The edit that says `tag` (`@return int`) in the doc comment of `decl`, adding one if there
/// isn't one.
pub fn add_tag(cx: &Cx<'_>, decl: &SyntaxNode, tag: &str) -> FileEdit {
    let nl = cx.style.nl;
    if let Some(doc) = decl.doc_comment() {
        let range = doc.text_range();
        let indent = indent_at(cx.text, range.start());
        let text = doc.text();
        if !text.contains('\n') {
            let inner = text.trim_start_matches("/**").trim_end_matches("*/").trim();
            let body = if inner.is_empty() {
                format!("/**{nl}{indent} * {tag}{nl}{indent} */")
            } else {
                format!("/**{nl}{indent} * {inner}{nl}{indent} * {tag}{nl}{indent} */")
            };
            return cx.edit(range, body);
        }
        let close = range.end() - TextSize::of("*/");
        let line = crate::imports::line_start(cx.text, close);
        let before = &cx.text[usize::from(line)..usize::from(close)];
        if before.trim().is_empty() {
            return cx.insert(line, format!("{before}* {tag}{nl}"));
        }
        // text and the end on one line
        return cx.edit(
            TextRange::at(close, TextSize::of("*/")),
            format!("{nl}{indent} * {tag}{nl}{indent} */"),
        );
    }
    let at = first_token_start(decl);
    if starts_line(cx.text, at) {
        let indent = indent_at(cx.text, at);
        cx.insert(
            at,
            format!("/**{nl}{indent} * {tag}{nl}{indent} */{nl}{indent}"),
        )
    } else {
        cx.insert(at, format!("/** {tag} */ "))
    }
}

/// Where what's written of `node` starts, past any comments the tree has in it.
pub fn first_token_start(node: &SyntaxNode) -> TextSize {
    node.children_with_tokens()
        .find(|e| !e.kind().is_trivia())
        .map_or_else(|| node.text_range().start(), |e| e.text_range().start())
}
