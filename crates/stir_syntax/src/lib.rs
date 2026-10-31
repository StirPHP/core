//! Lexer and parser for PHP and StirPHP, producing a lossless rowan tree.

mod lexer;
mod syntax_kind;

pub use lexer::{LexError, Lexed, Token, lex};
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
