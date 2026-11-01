//! Every statement node type.

mod common;

use common::*;
use stir_emit::ast::*;

#[test]
fn control_flow() {
    let loop_body = vec![
        Stmt::if_(
            bin(BinaryOp::Gt, v("i"), int(5)),
            vec![StmtKind::Break(None).into()],
        ),
        Stmt::if_(
            bin(BinaryOp::Identical, v("i"), int(2)),
            vec![StmtKind::Continue(None).into()],
        ),
        Stmt::echo(vec![v("i"), s("\n")]),
    ];
    insta::assert_snapshot!(file(vec![
        Stmt::if_(v("a"), vec![Stmt::echo(vec![s("a")])])
            .elseif(v("b").not(), vec![Stmt::echo(vec![s("b")])])
            .elseif(v("c").assign(Expr::call_fn("f", vec![])), vec![])
            .else_(vec![Stmt::echo(vec![s("c")])]),
        Stmt::while_(bin(BinaryOp::Lt, v("i"), int(10)), loop_body.clone()),
        StmtKind::DoWhile {
            body: vec![un(UnaryOp::PostInc, v("i")).stmt()],
            cond: bin(BinaryOp::Lt, v("i"), int(3))
        }
        .into(),
        StmtKind::For {
            init: vec![v("i").assign(int(0)), v("j").assign(int(10))],
            cond: vec![bin(BinaryOp::Lt, v("i"), v("j"))],
            step: vec![un(UnaryOp::PostInc, v("i")), un(UnaryOp::PreDec, v("j"))],
            body: vec![
                StmtKind::For {
                    init: vec![],
                    cond: vec![],
                    step: vec![],
                    body: vec![StmtKind::Break(Some(2)).into()]
                }
                .into()
            ],
        }
        .into(),
        Stmt::foreach(v("xs"), None, v("x"), vec![Stmt::echo(vec![v("x")])]),
        Stmt::foreach(
            v("xs"),
            Some(v("k")),
            v("x"),
            vec![StmtKind::Continue(Some(1)).into()]
        ),
        StmtKind::Foreach {
            expr: v("xs"),
            key: None,
            value: v("x"),
            by_ref: true,
            body: vec![v("x").assign_op(AssignOp::Mul, int(2)).stmt()]
        }
        .into(),
        Stmt::foreach(
            v("pairs"),
            None,
            Expr::list(vec![
                Some(ArrayItem::value(v("a"))),
                Some(ArrayItem::value(v("b")))
            ]),
            vec![]
        ),
        Stmt::foreach(
            v("rows"),
            Some(v("i")),
            Expr::list(vec![Some(ArrayItem::keyed(s("id"), v("id")))]),
            vec![]
        ),
        StmtKind::Switch {
            subject: v("x"),
            cases: vec![
                SwitchCase {
                    test: Some(int(1)),
                    body: vec![]
                },
                SwitchCase {
                    test: Some(int(2)),
                    body: vec![Stmt::echo(vec![s("1 or 2")]), StmtKind::Break(None).into()]
                },
                SwitchCase {
                    test: None,
                    body: vec![Stmt::echo(vec![s("other")])]
                },
            ],
        }
        .into(),
        StmtKind::Try {
            body: vec![Expr::call_fn("risky", vec![]).stmt()],
            catches: vec![
                Catch {
                    types: vec![
                        QName::new("\\InvalidArgumentException"),
                        QName::new("\\LogicException")
                    ],
                    var: Some("e".into()),
                    body: vec![Stmt::throw(v("e"))]
                },
                Catch {
                    types: vec![QName::new("\\Throwable")],
                    var: None,
                    body: vec![]
                },
            ],
            finally: Some(vec![Expr::call_fn("cleanup", vec![]).stmt()]),
        }
        .into(),
        StmtKind::Try {
            body: vec![],
            catches: vec![],
            finally: Some(vec![])
        }
        .into(),
    ]));
}

#[test]
fn simple_statements() {
    insta::assert_snapshot!(file(vec![
        Stmt::echo(vec![s("a"), v("b"), int(1)]),
        in_function(
            "f",
            vec![
                StmtKind::Global(vec!["config".into(), "db".into()]).into(),
                StmtKind::Static(vec![
                    StaticVar {
                        name: "count".into(),
                        default: Some(int(0))
                    },
                    StaticVar {
                        name: "cache".into(),
                        default: None
                    },
                ])
                .into(),
                StmtKind::Unset(vec![v("a"), v("b").index(s("k"))]).into(),
                Stmt::ret(None),
            ]
        ),
        in_function("g", vec![Stmt::ret(Some(v("x").add(int(1))))]),
        Stmt::block(vec![Stmt::echo(vec![s("in block")])]),
        Stmt::block(vec![]),
        StmtKind::Goto("end".into()).into(),
        Stmt::echo(vec![s("skipped")]),
        StmtKind::Label("end".into()).into(),
        Stmt::nop(),
        Stmt::comment("a comment\n\nwith lines"),
        Stmt::doc(DocComment::tag_only("var", "list<int> $xs")),
        v("xs").assign(Expr::array(vec![])).stmt(),
        Stmt::throw(Expr::new_("\\RuntimeException", args(vec![s("boom")]))),
        StmtKind::Declare(vec![("ticks".into(), int(1))]).into(),
        StmtKind::InlineHtml("\n<p>html</p>\n".into()).into(),
        Stmt::echo(vec![s("after html")]),
        StmtKind::Use(UseDecl::class("Foo\\Bar", None)).into(),
        StmtKind::Const(ConstDecl {
            doc: None,
            items: vec![("A".into(), int(1)), ("B".into(), s("b"))],
            origin: None
        })
        .into(),
    ]));
}

#[test]
fn file_header_namespace_uses() {
    let mut f = File::new()
        .namespace("App\\Model")
        .use_(UseDecl::class("Foo\\Bar", None))
        .use_(UseDecl::class("\\Foo\\Baz", Some("Qux")))
        .use_(UseDecl {
            kind: UseKind::Class,
            group_prefix: Some("Some\\Grouped".into()),
            items: vec![
                UseItem {
                    name: "A".into(),
                    alias: None,
                },
                UseItem {
                    name: "B".into(),
                    alias: Some("C".into()),
                },
            ],
        })
        .use_(UseDecl::function("Lib\\helper", None))
        .use_(UseDecl::constant("Lib\\VERSION", None))
        .stmt(FunctionDecl::new("main").returns(Type::void()));
    f.doc = Some(DocComment::new().line("Generated by StirPHP. Do not edit."));
    insta::assert_snapshot!(stir_emit::print_file(&f, &opts()));
}

#[test]
fn braced_namespaces() {
    let f = File {
        doc: None,
        strict_types: false,
        namespace: Some(Namespace {
            name: Some("App".into()),
            braced: true,
        }),
        uses: vec![UseDecl::class("Foo\\Bar", None)],
        stmts: vec![Stmt::echo(vec![s("x")]), ClassDecl::new("A").into()],
    };
    insta::assert_snapshot!("braced_named", stir_emit::print_file(&f, &opts()));
    let f = File {
        doc: None,
        strict_types: false,
        namespace: None,
        uses: vec![],
        stmts: vec![
            StmtKind::Namespace {
                name: Some("A".into()),
                body: Some(vec![FunctionDecl::new("f").into()]),
            }
            .into(),
            StmtKind::Namespace {
                name: None,
                body: Some(vec![Stmt::echo(vec![Expr::call_fn("A\\f", vec![])])]),
            }
            .into(),
        ],
    };
    insta::assert_snapshot!("braced_multiple", stir_emit::print_file(&f, &opts()));
    let f = File::new();
    insta::assert_snapshot!("empty_file", stir_emit::print_file(&f, &opts()));
}

#[test]
fn statement_level_edge_cases() {
    insta::assert_snapshot!(file(vec![
        Stmt::comment("closing tag ?> inside a comment"),
        Expr::closure(Closure::default()).stmt(),
        Expr::new(ExprKind::ArrowFn(ArrowFn {
            attrs: vec![],
            is_static: true,
            by_ref: false,
            params: vec![],
            return_type: None,
            body: Box::new(int(1)),
        }))
        .stmt(),
        Expr::new_anon(ClassDecl::default(), vec![])
            .method("m", vec![])
            .stmt(),
        Expr::match_(v("x"), vec![]).stmt(),
        Expr::list(vec![Some(ArrayItem::value(v("a")))])
            .assign(Expr::array_of(vec![int(1)]))
            .stmt(),
        Stmt::if_(
            v("x")
                .assign(Expr::call_fn("next", args(vec![v("it")])))
                .not_identical(Expr::bool(false)),
            vec![]
        ),
        Stmt::if_(
            v("x")
                .assign(Expr::call_fn("next", args(vec![v("it")])))
                .not(),
            vec![]
        ),
        Stmt::while_(v("row").assign(Expr::call_fn("fetch", vec![])), vec![]),
        Stmt::echo(vec![Expr::ternary(v("a"), v("b").print(), v("c").print())]),
        Stmt::expr(int(-1).neg().neg()),
        Stmt::expr(Expr::static_call("static", "m", vec![])),
    ]));
}
