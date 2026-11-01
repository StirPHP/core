//! Every expression node type.

mod common;

use common::*;
use stir_emit::ast::*;

#[test]
fn literals() {
    insta::assert_snapshot!(expr_cases(vec![
        ("int", int(42)),
        ("negative int", int(-7)),
        ("int min", int(i64::MIN)),
        ("int max", int(i64::MAX)),
        ("float integral", Expr::float(1.0)),
        ("float fraction", Expr::float(0.1)),
        ("float negative", Expr::float(-2.5)),
        ("float big", Expr::float(1e25)),
        ("float small", Expr::float(1.5e-10)),
        ("float max", Expr::float(f64::MAX)),
        ("float nan", Expr::float(f64::NAN)),
        ("float inf", Expr::float(f64::INFINITY)),
        ("float -inf", Expr::float(f64::NEG_INFINITY)),
        ("float -0", Expr::float(-0.0)),
        ("true", Expr::bool(true)),
        ("false", Expr::bool(false)),
        ("null", Expr::null()),
        ("const", Expr::name("PHP_EOL")),
        ("fq const", Expr::name("\\PHP_VERSION")),
        (
            "magic",
            Expr::array_of(vec![
                Expr::magic(MagicConst::Line),
                Expr::magic(MagicConst::File),
                Expr::magic(MagicConst::Dir),
                Expr::magic(MagicConst::Function),
                Expr::magic(MagicConst::Class),
                Expr::magic(MagicConst::Trait),
                Expr::magic(MagicConst::Method),
                Expr::magic(MagicConst::Namespace),
            ])
        ),
        ("raw", Expr::raw("PHP_INT_SIZE")),
        ("paren", Expr::paren(v("a"))),
    ]));
}

#[test]
fn arrays() {
    insta::assert_snapshot!(expr_cases(vec![
        ("empty", Expr::array(vec![])),
        ("list", Expr::array_of(vec![int(1), int(2), int(3)])),
        (
            "keyed",
            Expr::array(vec![
                ArrayItem::keyed(s("a"), int(1)),
                ArrayItem::keyed(int(5), v("x")),
            ])
        ),
        (
            "spread and ref",
            Expr::array(vec![
                ArrayItem::spread(v("xs")),
                ArrayItem::by_ref(v("y")),
                ArrayItem {
                    key: Some(s("k")),
                    value: v("z"),
                    by_ref: true,
                    spread: false
                },
            ])
        ),
        (
            "nested",
            Expr::array_of(vec![Expr::array_of(vec![int(1)]), Expr::array(vec![])])
        ),
        (
            "destructure keyed",
            Expr::list(vec![
                Some(ArrayItem::keyed(s("id"), v("id"))),
                Some(ArrayItem::keyed(
                    s("tags"),
                    Expr::list(vec![Some(ArrayItem::value(v("first")))])
                )),
            ])
            .assign(v("row"))
        ),
        (
            "destructure holes",
            Expr::list(vec![
                None,
                Some(ArrayItem::value(v("b"))),
                None,
                Some(ArrayItem::value(v("d")))
            ])
            .assign(v("arr"))
        ),
        (
            "destructure ref",
            Expr::list(vec![
                Some(ArrayItem::value(v("a"))),
                Some(ArrayItem::by_ref(v("b")))
            ])
            .assign(v("arr"))
        ),
    ]));
}

#[test]
fn variables_and_access() {
    insta::assert_snapshot!(expr_cases(vec![
        ("var", v("x")),
        ("weird var name", v("not an ident")),
        ("var var", Expr::new(ExprKind::VarVar(Box::new(v("name"))))),
        (
            "var var expr",
            Expr::new(ExprKind::VarVar(Box::new(s("a").concat(v("b")))))
        ),
        ("prop", v("o").prop("p")),
        ("nullsafe prop", v("o").nullsafe_prop("p")),
        ("dynamic prop", v("o").prop(v("name"))),
        ("dynamic prop expr", v("o").prop(s("a").concat(v("b")))),
        ("static prop", Expr::static_prop("Foo", "count")),
        ("static prop self", Expr::static_prop("self", "count")),
        ("static prop dynamic", Expr::static_prop("static", v("n"))),
        ("class const", Expr::class_const("\\Foo\\Bar", "BAZ")),
        ("class const dynamic", Expr::class_const("Foo", v("c"))),
        ("class name", Expr::class_name("parent")),
        ("index", v("a").index(s("k"))),
        (
            "nested index",
            v("a").index(int(0)).index(v("i").add(int(1)))
        ),
        ("push", v("a").push_index().assign(int(1))),
        ("this", Expr::this().prop("x")),
    ]));
}

#[test]
fn calls() {
    insta::assert_snapshot!(expr_cases(vec![
        ("func", Expr::call_fn("strlen", args(vec![s("abc")]))),
        (
            "fq func",
            Expr::call_fn("\\array_map", args(vec![s("trim"), v("xs")]))
        ),
        (
            "named args",
            Expr::call_fn(
                "str_pad",
                vec![
                    v("s").into(),
                    int(10).into(),
                    Arg::named("pad_type", Expr::name("STR_PAD_LEFT"))
                ]
            )
        ),
        (
            "spread args",
            Expr::call_fn("f", vec![v("a").into(), Arg::spread(v("rest"))])
        ),
        ("first class callable", Expr::name("strlen").callable()),
        (
            "method callable",
            v("o").method("m", vec![Arg::Placeholder])
        ),
        (
            "static callable",
            Expr::static_call("Foo", "bar", vec![Arg::Placeholder])
        ),
        ("method", v("o").method("run", args(vec![int(1)]))),
        ("nullsafe method", v("o").nullsafe_method("run", vec![])),
        ("dynamic method", v("o").method(v("m"), vec![])),
        ("static call", Expr::static_call("Foo", "create", vec![])),
        (
            "parent call",
            Expr::static_call("parent", "__construct", args(vec![v("x")]))
        ),
        (
            "dynamic static method",
            Expr::static_call("Foo", v("m"), vec![])
        ),
        ("new", Expr::new_("Foo", vec![])),
        ("new args", Expr::new_("\\DateTime", args(vec![s("now")]))),
        ("new static", Expr::new_("static", vec![])),
        ("new anon", Expr::new_anon(ClassDecl::default(), vec![])),
        (
            "new anon full",
            Expr::new_anon(
                ClassDecl::new("")
                    .readonly()
                    .extends("Base")
                    .implements("\\Countable")
                    .member(
                        MethodDecl::new(Visibility::Public, "count")
                            .returns(Type::int())
                            .body(vec![Stmt::ret(Some(int(0)))])
                    ),
                args(vec![v("x")]),
            )
        ),
        (
            "new anon attrs",
            Expr::new_anon(ClassDecl::new("").attr(Attribute::new("Foo")), vec![])
        ),
        ("isset", Expr::isset(vec![v("a"), v("b").index(s("k"))])),
        ("empty", v("a").prop("b").empty()),
        ("exit", Expr::new(ExprKind::Exit(None))),
        (
            "exit code",
            Expr::new(ExprKind::Exit(Some(Box::new(int(1)))))
        ),
        ("print", s("hi").print()),
        ("include", Expr::include(IncludeKind::Include, s("a.php"))),
        (
            "require once",
            Expr::include(
                IncludeKind::RequireOnce,
                Expr::magic(MagicConst::Dir).concat(s("/b.php"))
            )
        ),
        ("clone", v("a").clone_()),
    ]));
}

#[test]
fn operators() {
    use BinaryOp::*;
    let ops = [
        Add,
        Sub,
        Mul,
        Div,
        Mod,
        Pow,
        Concat,
        BitAnd,
        BitOr,
        BitXor,
        Shl,
        Shr,
        And,
        Or,
        LogicalAnd,
        LogicalOr,
        LogicalXor,
        Eq,
        NotEq,
        Identical,
        NotIdentical,
        Lt,
        Le,
        Gt,
        Ge,
        Spaceship,
        Coalesce,
    ];
    let mut cases: Vec<(&str, Expr)> = ops
        .iter()
        .map(|op| ("binary", v("r").assign(bin(*op, v("a"), v("b")))))
        .collect();
    use AssignOp as A;
    for op in [
        A::Add,
        A::Sub,
        A::Mul,
        A::Div,
        A::Mod,
        A::Pow,
        A::Concat,
        A::BitAnd,
        A::BitOr,
        A::BitXor,
        A::Shl,
        A::Shr,
        A::Coalesce,
    ] {
        cases.push(("assign op", v("a").assign_op(op, v("b"))));
    }
    for op in [
        UnaryOp::Not,
        UnaryOp::Neg,
        UnaryOp::Plus,
        UnaryOp::BitNot,
        UnaryOp::PreInc,
        UnaryOp::PreDec,
        UnaryOp::PostInc,
        UnaryOp::PostDec,
    ] {
        cases.push(("unary", un(op, v("a"))));
    }
    for k in [
        CastKind::Int,
        CastKind::Float,
        CastKind::String,
        CastKind::Bool,
        CastKind::Array,
        CastKind::Object,
    ] {
        cases.push(("cast", v("a").cast(k)));
    }
    cases.push(("assign ref", v("a").assign_ref(v("b"))));
    cases.push(("ternary", Expr::ternary(v("a"), v("b"), v("c"))));
    cases.push(("short ternary", v("a").short_ternary(v("b"))));
    cases.push(("instanceof", v("a").instanceof("\\Foo\\Bar")));
    cases.push((
        "silence",
        Expr::call_fn("file_get_contents", args(vec![v("f")])).silence(),
    ));
    insta::assert_snapshot!(expr_cases(cases));
}

#[test]
fn functions_and_match() {
    let closure = Closure {
        params: vec![Param::new("x").ty(Type::int())],
        uses: vec![ClosureUse::val("y"), ClosureUse::by_ref("acc")],
        return_type: Some(Type::void()),
        body: vec![v("acc").push_index().assign(v("x").add(v("y"))).stmt()],
        ..Closure::default()
    };
    let static_ref = Closure {
        is_static: true,
        by_ref: true,
        attrs: vec![AttributeGroup::single(Attribute::new("Pure"))],
        body: vec![Stmt::ret(Some(v("this_is_fine")))],
        ..Closure::default()
    };
    let arrow = ExprKind::ArrowFn(ArrowFn {
        attrs: vec![],
        is_static: true,
        by_ref: true,
        params: vec![
            Param::new("x")
                .ty(Type::int().nullable())
                .default_value(Expr::null()),
        ],
        return_type: Some(Type::int()),
        body: Box::new(v("x").coalesce(int(0))),
    });
    let m = Expr::match_(
        v("x"),
        vec![
            MatchArm::new(vec![int(1), int(2)], s("low")),
            MatchArm::new(vec![int(3)], s("mid")),
            MatchArm::default(Expr::new_("\\LogicException", vec![]).throw()),
        ],
    );
    let match_true = Expr::match_(
        Expr::bool(true),
        vec![
            MatchArm::new(vec![v("x").instanceof("A")], int(1)),
            MatchArm::new(
                vec![bin(BinaryOp::Gt, v("x").prop("n"), int(1))],
                Expr::ternary(v("a"), int(1), int(2)),
            ),
        ],
    );
    let gen_body = vec![
        Expr::yield_(None).stmt(),
        Expr::yield_(Some(int(1))).stmt(),
        Expr::yield_kv(s("k"), v("v")).stmt(),
        v("got").assign(Expr::yield_(Some(v("v")))).stmt(),
        Expr::call_fn("other", vec![]).yield_from().stmt(),
    ];
    insta::assert_snapshot!(file(vec![
        Stmt::comment("closure"),
        v("f").assign(Expr::closure(closure)).stmt(),
        Stmt::comment("static by-ref closure"),
        v("g").assign(Expr::closure(static_ref)).stmt(),
        Stmt::comment("empty closure"),
        v("h").assign(Expr::closure(Closure::default())).stmt(),
        Stmt::comment("arrow fn"),
        v("k").assign(Expr::new(arrow)).stmt(),
        Stmt::comment("match"),
        v("r").assign(m).stmt(),
        v("r").assign(match_true).stmt(),
        Stmt::comment("closure as argument"),
        Expr::call_fn(
            "usort",
            args(vec![
                v("xs"),
                Expr::closure(Closure::new(
                    vec![Param::new("a"), Param::new("b")],
                    vec![Stmt::ret(Some(bin(BinaryOp::Spaceship, v("a"), v("b"))))],
                ))
            ])
        )
        .stmt(),
        Stmt::comment("generator"),
        in_function("gen", gen_body),
    ]));
}
