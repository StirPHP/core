//! Lexer and parser for PHP and StirPHP, producing a lossless rowan tree.

mod grammar;
mod lexer;
mod parser;
mod syntax_kind;

use rowan::GreenNode;

pub use lexer::{LexError, Lexed, Token, lex};
pub use parser::SyntaxError;
pub use rowan::{TextRange, TextSize};
pub use syntax_kind::{StirLanguage, SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};

/// Which surface language a file is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// Plain `.php`: starts in inline-HTML mode, PHP interpolation rules.
    Php,
    /// `.sphp`: code from the first byte, only `${...}` interpolates.
    Stir,
}

impl Dialect {
    pub fn from_path(path: &std::path::Path) -> Option<Dialect> {
        match path.extension()?.to_str()? {
            "php" | "phtml" | "inc" => Some(Dialect::Php),
            "sphp" => Some(Dialect::Stir),
            _ => None,
        }
    }
}

/// The result of parsing a file: a lossless tree plus syntax errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parse {
    green: GreenNode,
    errors: Vec<SyntaxError>,
}

impl Parse {
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    pub fn green(&self) -> &GreenNode {
        &self.green
    }

    pub fn errors(&self) -> &[SyntaxError] {
        &self.errors
    }

    /// An indented dump of the tree and its errors, for tests and debugging.
    pub fn debug_tree(&self) -> String {
        let mut out = String::new();
        dump(&self.syntax(), 0, &mut out);
        for err in &self.errors {
            out.push_str(&format!(
                "error {}..{}: {}\n",
                u32::from(err.range.start()),
                u32::from(err.range.end()),
                err.message
            ));
        }
        out
    }
}

fn dump(node: &SyntaxNode, depth: usize, out: &mut String) {
    use std::fmt::Write;
    let _ = writeln!(out, "{:indent$}{:?}", "", node.kind(), indent = depth * 2);
    for child in node.children_with_tokens() {
        match child {
            rowan::NodeOrToken::Node(n) => dump(&n, depth + 1, out),
            rowan::NodeOrToken::Token(t) => {
                if t.kind() != SyntaxKind::WHITESPACE {
                    let _ = writeln!(
                        out,
                        "{:indent$}{:?} {:?}",
                        "",
                        t.kind(),
                        t.text(),
                        indent = (depth + 1) * 2
                    );
                }
            }
        }
    }
}

/// Parses a whole file.
pub fn parse(src: &str, dialect: Dialect) -> Parse {
    let lexed = lex(src, dialect);
    let mut p = parser::Parser::new(src, &lexed, dialect);
    grammar::source_file(&mut p);
    let events = p.finish();
    let (green, mut errors) = parser::build_tree(src, &lexed, events);
    errors.extend(lexed.errors.iter().map(|e| SyntaxError {
        range: TextRange::empty(TextSize::new(e.offset)),
        message: e.message.clone(),
    }));
    errors.sort_by_key(|e| e.range.start());
    Parse { green, errors }
}
