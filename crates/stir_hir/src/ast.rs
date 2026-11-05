//! Small helpers for navigating the untyped rowan tree.

use stir_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

pub trait NodeExt {
    fn child(&self, kind: SyntaxKind) -> Option<SyntaxNode>;
    fn children_of(&self, kind: SyntaxKind) -> Vec<SyntaxNode>;
    fn token(&self, kind: SyntaxKind) -> Option<SyntaxToken>;
    fn tokens(&self) -> Vec<SyntaxToken>;
    fn has_token(&self, kind: SyntaxKind) -> bool;
    /// Text of the direct `NAME` child.
    fn name_text(&self) -> Option<String>;
    /// The doc comment attached as the node's first token, if any.
    fn doc_comment(&self) -> Option<SyntaxToken>;
}

impl NodeExt for SyntaxNode {
    fn child(&self, kind: SyntaxKind) -> Option<SyntaxNode> {
        self.children().find(|c| c.kind() == kind)
    }

    fn children_of(&self, kind: SyntaxKind) -> Vec<SyntaxNode> {
        self.children().filter(|c| c.kind() == kind).collect()
    }

    fn token(&self, kind: SyntaxKind) -> Option<SyntaxToken> {
        self.children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == kind)
    }

    fn tokens(&self) -> Vec<SyntaxToken> {
        self.children_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| !t.kind().is_trivia())
            .collect()
    }

    fn has_token(&self, kind: SyntaxKind) -> bool {
        self.token(kind).is_some()
    }

    fn name_text(&self) -> Option<String> {
        let name = self.child(SyntaxKind::NAME)?;
        Some(name.text().to_string().trim().to_owned())
    }

    fn doc_comment(&self) -> Option<SyntaxToken> {
        self.children_with_tokens()
            .take_while(|e| e.as_token().is_some_and(|t| t.kind().is_trivia()))
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == SyntaxKind::DOC_COMMENT)
            .last()
    }
}

/// The text of the first non-trivia token of a node.
pub fn first_token_text(node: &SyntaxNode) -> Option<String> {
    node.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())
        .map(|t| t.text().to_owned())
}
