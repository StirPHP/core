//! String literal escaping and interpolation.
//!
//! With `STIR_EMIT_PHP_OUT=<dir>` this also writes `strings.php` / `strings.expected`: the PHP
//! program prints `bin2hex()` of every literal, the expected file holds the hex of the intended
//! bytes, so `php strings.php | diff - strings.expected` verifies escaping at runtime.

mod common;

use common::*;
use stir_emit::ast::*;

fn samples() -> Vec<&'static str> {
    vec![
        "",
        "plain",
        "it's",
        "back\\slash",
        "trailing\\",
        "\\'",
        "double \"quotes\"",
        "$var and {$braces} and ${dollar}",
        "line\nbreak",
        "tab\there",
        "cr\r\n",
        "nul\0byte",
        "nul\0",
        "\0",
        "\x01\x02\x1f\x7f",
        "esc\x1b[0m",
        "vt\x0bff\x0c",
        "mixed \n $x \\ \"q\" '",
        "\\n literally",
        "\\u{41} literally",
        "\\x41 \\101 literally\n",
        "héllo wörld",
        "日本語 ✓ 🎉",
        "nel\u{85}ls\u{2028}",
        "bom\u{feff}",
        "{$",
        "{\n$",
    ]
}

fn interp_samples() -> Vec<(Vec<InterpPart>, String)> {
    // `$x` is 'X', `$o->p` is 'P', `$a['k']` is 'K'.
    let lit = |s: &str| InterpPart::Lit(s.into());
    let ex = InterpPart::Expr;
    let x = || Expr::var("x");
    vec![
        (vec![lit("hello "), ex(x()), lit("!")], "hello X!".into()),
        (vec![ex(x()), ex(x())], "XX".into()),
        (vec![lit("{"), ex(x()), lit("}")], "{X}".into()),
        (vec![lit("$"), ex(x()), lit("$")], "$X$".into()),
        (vec![lit("{$"), ex(x()), lit("\\")], "{$X\\".into()),
        (
            vec![lit("\""), ex(Expr::var("o").prop("p")), lit("\n")],
            "\"P\n".into(),
        ),
        (
            vec![ex(Expr::var("a").index(Expr::string("k"))), lit("'")],
            "K'".into(),
        ),
        (
            vec![
                ex(Expr::var("o").nullsafe_prop("p")),
                lit(" "),
                ex(Expr::var("o").method("m", vec![])),
            ],
            "P M".into(),
        ),
        (vec![lit("only literal $x")], "only literal $x".into()),
        (vec![], "".into()),
        // Not starting with a variable: printed as concatenation.
        (
            vec![
                lit("n="),
                ex(Expr::call_fn("strtoupper", args(vec![Expr::string("q")]))),
                lit(";"),
            ],
            "n=Q;".into(),
        ),
        (
            vec![ex(Expr::call_fn(
                "strtoupper",
                args(vec![Expr::string("q")]),
            ))],
            "Q".into(),
        ),
        (
            vec![ex(Expr::int(1).add(Expr::int(2))), lit("\t")],
            "3\t".into(),
        ),
        (
            vec![ex(Expr::static_call("S", "m", vec![])), ex(x())],
            "MX".into(),
        ),
    ]
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn setup() -> Vec<Stmt> {
    vec![
        Expr::var("x").assign(Expr::string("X")).stmt(),
        Expr::var("o")
            .assign(Expr::new_anon(
                ClassDecl::new("")
                    .member(
                        PropertyDecl::new(Visibility::Public, "p").default_value(Expr::string("P")),
                    )
                    .member(
                        MethodDecl::new(Visibility::Public, "m")
                            .body(vec![Stmt::ret(Some(Expr::string("M")))]),
                    ),
                vec![],
            ))
            .stmt(),
        Expr::var("a")
            .assign(Expr::array(vec![ArrayItem::keyed(
                Expr::string("k"),
                Expr::string("K"),
            )]))
            .stmt(),
        ClassDecl::new("S")
            .member(
                MethodDecl::new(Visibility::Public, "m")
                    .static_()
                    .body(vec![Stmt::ret(Some(Expr::string("M")))]),
            )
            .into(),
    ]
}

fn program() -> (String, String) {
    let mut stmts = setup();
    let mut expected = String::new();
    let echo_hex = |e: Expr| {
        Stmt::echo(vec![
            Expr::call_fn("bin2hex", vec![e.into()]),
            Expr::string("\n"),
        ])
    };
    for s in samples() {
        stmts.push(echo_hex(Expr::string(s)));
        expected.push_str(&hex(s));
        expected.push('\n');
    }
    for (parts, want) in interp_samples() {
        stmts.push(echo_hex(Expr::interp(parts)));
        expected.push_str(&hex(&want));
        expected.push('\n');
    }
    (file(stmts), expected)
}

#[test]
fn string_literals() {
    let cases = samples()
        .into_iter()
        .map(|t| ("", Expr::string(t)))
        .collect();
    insta::assert_snapshot!(expr_cases(cases));
}

#[test]
fn interpolation() {
    let mut stmts = setup();
    for (parts, _) in interp_samples() {
        stmts.push(Expr::var("r").assign(Expr::interp(parts)).stmt());
    }
    insta::assert_snapshot!(file(stmts));
}

#[test]
fn runtime_program() {
    let (php, expected) = program();
    assert_eq!(
        expected.lines().count(),
        samples().len() + interp_samples().len()
    );
    if let Ok(dir) = std::env::var("STIR_EMIT_PHP_OUT") {
        let dir = std::path::Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("strings.php"), php).unwrap();
        std::fs::write(dir.join("strings.expected"), expected).unwrap();
    }
}
