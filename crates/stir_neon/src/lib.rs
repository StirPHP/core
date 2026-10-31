//! NEON parser (<https://ne-on.org/>, semantics of nette/neon v3) and a typed loader for
//! `phpstan.neon` configuration ([`phpstan::PhpstanConfig`]).
//!
//! Entry point: [`parse`].

mod lexer;
mod parser;
pub mod phpstan;
mod value;

pub use indexmap::IndexMap;
pub use value::{Entity, Key, Value};

/// A NEON syntax error with a 1-based line/column.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message} on line {line} at column {column}")]
pub struct Error {
    pub message: String,
    pub line: usize,
    pub column: usize,
    /// Byte offset into the (CRLF-normalised) input.
    pub offset: usize,
}

impl Error {
    pub(crate) fn at(src: &str, offset: usize, message: String) -> Self {
        let offset = offset.min(src.len());
        let before = &src[..offset];
        let line = before.matches('\n').count() + 1;
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let column = src[line_start..offset].chars().count() + 1;
        Error {
            message,
            line,
            column,
            offset,
        }
    }
}

/// Parses a NEON document.
pub fn parse(src: &str) -> Result<Value, Error> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    if src.contains('\r') {
        let normalized = src.replace("\r\n", "\n").replace('\r', "\n");
        parser::parse(&normalized)
    } else {
        parser::parse(src)
    }
}
