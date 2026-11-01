//! Line wrapping of argument lists, arrays and parameter lists.

mod common;

use common::*;
use stir_emit::ast::*;
use stir_emit::print::{PrintOptions, print_file};

fn long_names(n: usize) -> Vec<Expr> {
    (0..n).map(|i| v(&format!("argumentNumber{i}"))).collect()
}

#[test]
fn wrapping() {
    let nested = Expr::array(vec![
        ArrayItem::keyed(s("short"), Expr::array_of(vec![int(1), int(2)])),
        ArrayItem::keyed(
            s("long"),
            Expr::array(
                (0..8)
                    .map(|i| {
                        ArrayItem::keyed(s(&format!("key_{i}")), s(&format!("value number {i}")))
                    })
                    .collect(),
            ),
        ),
    ]);
    let closure_arg = Expr::call_fn(
        "array_map",
        args(vec![
            Expr::closure(Closure {
                params: vec![Param::new("x")],
                uses: vec![ClosureUse::val("factor")],
                body: vec![Stmt::ret(Some(v("x").mul(v("factor"))))],
                ..Closure::default()
            }),
            v("xs"),
        ]),
    );
    let long_fn = FunctionDecl::new("aFunctionWithAVeryLongName")
        .param(Param::new("firstParameter").ty(Type::class("\\Some\\Namespaced\\ClassName")))
        .param(
            Param::new("secondParameter")
                .ty(Type::string().nullable())
                .default_value(Expr::null()),
        )
        .returns(Type::class("\\Some\\Namespaced\\ReturnType"))
        .body(vec![Stmt::ret(Some(Expr::new_(
            "\\Some\\Namespaced\\ReturnType",
            vec![],
        )))]);
    let long_method = ClassDecl::new("K").member(
        MethodDecl::new(Visibility::Public, "aMethodWithAVeryLongNameIndeed")
            .param(Param::new("firstParameter").ty(Type::class("\\Some\\Namespaced\\ClassName")))
            .param(Param::new("secondParameter").ty(Type::string()))
            .returns(Type::void()),
    );
    insta::assert_snapshot!(file(vec![
        Stmt::comment("fits"),
        Expr::call_fn("f", args(long_names(3))).stmt(),
        Stmt::comment("too long: one per line with trailing comma"),
        Expr::call_fn("someFunction", args(long_names(6))).stmt(),
        Stmt::comment("nested call breaks only where needed"),
        v("result")
            .assign(Expr::call_fn(
                "outer",
                vec![
                    Expr::call_fn("inner", args(long_names(5))).into(),
                    int(1).into(),
                ]
            ))
            .stmt(),
        Stmt::comment("arrays"),
        v("config").assign(nested).stmt(),
        Stmt::comment("closure argument stays inline"),
        v("ys").assign(closure_arg).stmt(),
        Stmt::comment("isset / unset"),
        Stmt::if_(
            Expr::isset(long_names(6)),
            vec![StmtKind::Unset(long_names(6)).into()]
        ),
        Stmt::comment("new and method args"),
        v("x")
            .assign(Expr::new_(
                "\\Very\\Long\\Namespace\\ClassName",
                args(long_names(4))
            ))
            .stmt(),
        Expr::this()
            .method("doSomethingWith", args(long_names(5)))
            .stmt(),
        long_fn.into(),
        long_method.into(),
    ]));
}

#[test]
fn narrow_width_and_tabs() {
    let opts = PrintOptions {
        indent: "\t".into(),
        line_width: 30,
    };
    let mut f = File::new();
    f.stmts = vec![
        FunctionDecl::new("f")
            .param(Param::new("a").ty(Type::int()))
            .param(Param::new("b").ty(Type::int()))
            .returns(Type::int())
            .body(vec![Stmt::ret(Some(Expr::call_fn(
                "max",
                args(vec![v("a"), v("b"), int(100), int(200)]),
            )))])
            .into(),
        v("xs")
            .assign(Expr::array_of(vec![s("alpha"), s("beta"), s("gamma")]))
            .stmt(),
        v("ok").assign(Expr::array_of(vec![int(1), int(2)])).stmt(),
    ];
    insta::assert_snapshot!(print_file(&f, &opts));
}
