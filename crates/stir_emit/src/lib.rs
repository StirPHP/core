//! PHP code generation for StirPHP: an owned PHP output AST ([`ast`]), builder helpers
//! ([`builders`]), a table-driven PHP 8 precedence model ([`prec`]) and a pretty-printer
//! ([`print`]) producing PSR-12 / PER-CS styled PHP 8.3 with an optional source-map hook.

pub mod ast;
pub mod builders;
pub mod escape;
mod lower;
pub mod prec;
pub mod print;
pub mod transpile;

pub use print::{
    Mapping, Position, PrintOptions, print_expr, print_file, print_file_with_map, print_stmts,
};

pub use transpile::{EmitOptions, Emitted, runtime, transpile};
