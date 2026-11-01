#![allow(dead_code)]

use stir_emit::ast::*;
use stir_emit::print::{PrintOptions, print_file};

pub fn opts() -> PrintOptions {
    PrintOptions::default()
}

/// A complete `<?php` file with `declare(strict_types=1);` and the given statements.
pub fn file(stmts: Vec<Stmt>) -> String {
    let mut f = File::new();
    f.stmts = stmts;
    print_file(&f, &opts())
}

/// One labelled expression statement per case: `// label` then `expr;`.
pub fn expr_cases(cases: Vec<(&str, Expr)>) -> String {
    let mut stmts = Vec::new();
    for (label, e) in cases {
        stmts.push(Stmt::comment(label));
        stmts.push(Stmt::expr(e));
    }
    file(stmts)
}

pub fn v(name: &str) -> Expr {
    Expr::var(name)
}

pub fn int(i: i64) -> Expr {
    Expr::int(i)
}

pub fn s(text: &str) -> Expr {
    Expr::string(text)
}

pub fn bin(op: BinaryOp, a: Expr, b: Expr) -> Expr {
    a.bin(op, b)
}

pub fn un(op: UnaryOp, e: Expr) -> Expr {
    Expr::unary(op, e)
}

pub fn args(es: Vec<Expr>) -> Vec<Arg> {
    es.into_iter().map(Arg::from).collect()
}

/// `function name() { stmts }` wrapper (for `yield`, `return`, ...).
pub fn in_function(name: &str, body: Vec<Stmt>) -> Stmt {
    FunctionDecl::new(name).body(body).into()
}
