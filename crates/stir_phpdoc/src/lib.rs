//! PHPStan-flavoured PHPDoc parser.
//!
//! Mirrors the semantics of `phpstan/phpdoc-parser`: a doc comment is split into free text
//! (summary/description) and tags; tag values are parsed into typed variants, and types use
//! the full PHPStan type grammar. All nodes carry absolute [`TextRange`]s (the caller passes
//! the comment's offset in its file). Malformed types become [`TypeKind::Error`] nodes rather
//! than aborting.
//!
//! Entry points: [`parse_doc_comment`] and [`parse_type`].

mod ast;
mod doc_parser;
mod keyword;
mod lexer;
mod printer;
mod tags;
mod type_parser;

pub use ast::*;
pub use doc_parser::{parse_doc_comment, parse_type};
pub use keyword::Keyword;
pub use tags::*;
pub use text_size::{TextRange, TextSize};
pub use type_parser::parse_php_int;
