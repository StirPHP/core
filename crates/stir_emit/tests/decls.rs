//! Declarations: functions, classes, interfaces, traits, enums, members, types, docblocks.

mod common;

use common::*;
use stir_emit::ast::*;
use stir_emit::print::{print_doc, print_type};

#[test]
fn functions() {
    let full = FunctionDecl {
        doc: Some(
            DocComment::new()
                .line("Adds things.")
                .tag("param", "list<int> $xs")
                .tag("return", "int"),
        ),
        attrs: vec![AttributeGroup::new(vec![
            Attribute::new("\\Deprecated"),
            Attribute::with_args("Pure", vec![]),
        ])],
        by_ref: true,
        name: "sum".into(),
        params: vec![
            Param::new("xs").ty(Type::array()),
            Param::new("start").ty(Type::int()).default_value(int(0)),
            Param::new("out").by_ref().default_value(Expr::null()),
            Param::new("secret")
                .ty(Type::string())
                .attr(Attribute::new("\\SensitiveParameter"))
                .default_value(s("")),
            Param::new("rest").ty(Type::int()).variadic(),
        ],
        return_type: Some(Type::int()),
        body: vec![Stmt::ret(Some(
            Expr::call_fn("array_sum", args(vec![v("xs")])).add(v("start")),
        ))],
        origin: None,
    };
    insta::assert_snapshot!(file(vec![
        FunctionDecl::new("noop").into(),
        full.into(),
        FunctionDecl::new("by_ref_variadic")
            .param(Param::new("xs").ty(Type::int()).by_ref().variadic())
            .returns(Type::never())
            .body(vec![Stmt::throw(Expr::new_("\\Exception", vec![]))])
            .into(),
    ]));
}

#[test]
fn classes() {
    let ctor = MethodDecl::new(Visibility::Public, "__construct")
        .param(
            Param::new("id")
                .ty(Type::int())
                .promote(Visibility::Private, true),
        )
        .param(Param {
            promote: Some(Promotion {
                visibility: None,
                set_visibility: None,
                readonly: false,
            }),
            ..Param::new("name").ty(Type::string())
        })
        .param(Param {
            promote: Some(Promotion {
                visibility: Some(Visibility::Public),
                set_visibility: Some(Visibility::Private),
                readonly: false,
            }),
            ..Param::new("tags")
                .ty(Type::array())
                .default_value(Expr::array(vec![]))
        })
        .param(
            Param::new("plain")
                .ty(Type::bool())
                .default_value(Expr::bool(false)),
        );
    let class = ClassDecl::new("User")
        .doc(
            DocComment::new()
                .line("A user.")
                .tag("template", "T of object"),
        )
        .attr(Attribute::with_args(
            "Entity",
            vec![Arg::named("table", s("users"))],
        ))
        .final_()
        .extends("\\App\\Model")
        .implements("\\JsonSerializable")
        .implements("Countable")
        .member(TraitUse {
            traits: vec![QName::new("HasId"), QName::new("HasName")],
            adaptations: vec![],
            origin: None,
        })
        .member(TraitUse {
            traits: vec![QName::new("A"), QName::new("B")],
            adaptations: vec![
                TraitAdaptation::Insteadof {
                    trait_name: QName::new("A"),
                    method: "hello".into(),
                    instead_of: vec![QName::new("B")],
                },
                TraitAdaptation::Alias {
                    trait_name: Some(QName::new("B")),
                    method: "hello".into(),
                    visibility: Some(Visibility::Protected),
                    alias: Some("bHello".into()),
                },
                TraitAdaptation::Alias {
                    trait_name: None,
                    method: "bye".into(),
                    visibility: Some(Visibility::Private),
                    alias: None,
                },
            ],
            origin: None,
        })
        .member(
            ClassConstDecl::new("TABLE", s("users"))
                .visibility(Visibility::Public)
                .ty(Type::string()),
        )
        .member(ClassConstDecl {
            is_final: true,
            ..ClassConstDecl::new("MAX", int(10)).visibility(Visibility::Protected)
        })
        .member(ClassConstDecl::new("UNTYPED", Expr::null()))
        .member(
            PropertyDecl::new(Visibility::Private, "count")
                .static_()
                .ty(Type::int())
                .default_value(int(0)),
        )
        .member(
            PropertyDecl::new(Visibility::Public, "email")
                .ty(Type::string().nullable())
                .default_value(Expr::null()),
        )
        .member(PropertyDecl::new(Visibility::Protected, "untyped"))
        .member(
            PropertyDecl::new(Visibility::Public, "list")
                .readonly()
                .ty(Type::array())
                .doc(DocComment::tag_only("var", "list<string>")),
        )
        .member(ctor)
        .member(
            MethodDecl::new(Visibility::Public, "create")
                .static_()
                .returns(Type::static_())
                .body(vec![Stmt::ret(Some(Expr::new_(
                    "static",
                    args(vec![int(1), s("x")]),
                )))]),
        )
        .member(
            MethodDecl::new(Visibility::Public, "count")
                .final_()
                .returns(Type::int())
                .body(vec![Stmt::ret(Some(int(1)))]),
        )
        .member(
            MethodDecl::new(Visibility::Public, "jsonSerialize")
                .returns(Type::mixed())
                .body(vec![Stmt::ret(Some(Expr::this().prop("email")))]),
        )
        .member(
            MethodDecl {
                by_ref: true,
                ..MethodDecl::new(Visibility::Protected, "ref")
            }
            .returns(Type::array())
            .body(vec![Stmt::ret(Some(Expr::static_prop("self", "arr")))]),
        )
        .member(
            PropertyDecl::new(Visibility::Private, "arr")
                .static_()
                .ty(Type::array())
                .default_value(Expr::array(vec![])),
        );
    let abs = ClassDecl::new("Shape")
        .abstract_()
        .member(
            MethodDecl::new(Visibility::Public, "area")
                .abstract_()
                .returns(Type::float()),
        )
        .member(
            MethodDecl::new(Visibility::Protected, "name")
                .abstract_()
                .static_()
                .param(Param::new("x"))
                .returns(Type::string()),
        );
    let ro = ClassDecl::new("Point").final_().readonly().member(
        MethodDecl::new(Visibility::Public, "__construct")
            .param(
                Param::new("x")
                    .ty(Type::float())
                    .promote(Visibility::Public, false),
            )
            .param(
                Param::new("y")
                    .ty(Type::float())
                    .promote(Visibility::Public, false),
            ),
    );
    let empty = ClassDecl::new("Nothing");
    insta::assert_snapshot!(file(vec![
        Stmt::comment("helpers referenced below"),
        Stmt::from(TraitDecl::new("HasId")),
        TraitDecl::new("HasName").into(),
        TraitDecl::new("A")
            .member(MethodDecl::new(Visibility::Public, "hello"))
            .into(),
        TraitDecl::new("B")
            .member(MethodDecl::new(Visibility::Public, "hello"))
            .member(MethodDecl::new(Visibility::Public, "bye"))
            .into(),
        class.into(),
        abs.into(),
        ro.into(),
        empty.into(),
    ]));
}

#[test]
fn interfaces_traits_enums() {
    let iface = InterfaceDecl::new("Repo")
        .member(ClassConstDecl::new("VERSION", int(1)).visibility(Visibility::Public))
        .member(
            MethodDecl::new(Visibility::Public, "find")
                .no_body()
                .param(Param::new("id").ty(Type::int()))
                .returns(Type::class("Entity").nullable()),
        )
        .member(
            MethodDecl::new(Visibility::Public, "all")
                .no_body()
                .static_()
                .returns(Type::array()),
        );
    let iface = InterfaceDecl {
        extends: vec![QName::new("\\Countable"), QName::new("\\IteratorAggregate")],
        ..iface
    };
    let tr = TraitDecl::new("Greets")
        .member(
            PropertyDecl::new(Visibility::Protected, "greeting")
                .ty(Type::string())
                .default_value(s("hi")),
        )
        .member(
            MethodDecl::new(Visibility::Public, "greet")
                .returns(Type::string())
                .body(vec![Stmt::ret(Some(Expr::this().prop("greeting")))]),
        );
    let pure = EnumDecl::new("Status")
        .member(EnumCase::new("Active", None))
        .member(EnumCase::new("Inactive", None));
    let backed = EnumDecl {
        implements: vec![QName::new("HasLabel")],
        ..EnumDecl::new("Suit").backed(Type::string())
    }
    .member(TraitUse {
        traits: vec![QName::new("EnumHelpers")],
        adaptations: vec![],
        origin: None,
    })
    .member(EnumCase::new("Hearts", Some(s("H"))))
    .member(EnumCase {
        doc: Some(DocComment::new().line("Spades.")),
        ..EnumCase::new("Spades", Some(s("S")))
    })
    .member(
        ClassConstDecl::new("Wild", Expr::class_const("self", "Spades"))
            .visibility(Visibility::Public),
    )
    .member(
        MethodDecl::new(Visibility::Public, "label")
            .returns(Type::string())
            .body(vec![Stmt::ret(Some(Expr::match_(
                Expr::this(),
                vec![
                    MatchArm::new(vec![Expr::class_const("self", "Hearts")], s("Hearts")),
                    MatchArm::new(vec![Expr::class_const("self", "Spades")], s("Spades")),
                ],
            )))]),
    );
    let int_enum = EnumDecl::new("Level")
        .backed(Type::int())
        .member(EnumCase::new("Low", Some(int(1))))
        .member(EnumCase::new("High", Some(int(10))));
    insta::assert_snapshot!(file(vec![
        ClassDecl::new("Entity").into(),
        iface.into(),
        tr.into(),
        TraitDecl::new("EnumHelpers").into(),
        InterfaceDecl::new("HasLabel")
            .member(
                MethodDecl::new(Visibility::Public, "label")
                    .no_body()
                    .returns(Type::string())
            )
            .into(),
        pure.into(),
        backed.into(),
        int_enum.into(),
        StmtKind::Const(ConstDecl {
            doc: Some(DocComment::tag_only("var", "int")),
            items: vec![("LIMIT".into(), int(5))],
            origin: None
        })
        .into(),
    ]));
}

#[test]
fn types() {
    let a = || Type::class("A");
    let b = || Type::class("\\B");
    let cases = vec![
        Type::int(),
        Type::int().nullable(),
        Type::class(QName::fq("Foo\\Bar")).nullable(),
        Type::union(vec![Type::int(), Type::string()]),
        Type::union(vec![Type::int(), Type::null()]),
        Type::union(vec![Type::null(), Type::int(), Type::string()]),
        Type::union(vec![Type::int().nullable(), Type::string()]),
        Type::union(vec![
            Type::int(),
            Type::union(vec![Type::string(), Type::bool()]),
        ]),
        Type::intersection(vec![a(), b()]),
        Type::intersection(vec![a(), b()]).nullable(),
        Type::union(vec![Type::intersection(vec![a(), b()]), Type::null()]),
        Type::union(vec![Type::intersection(vec![a(), b()]), Type::class("C")]),
        Type::union(vec![Type::int(), Type::string()]).nullable(),
        Type::mixed().nullable(),
        Type::static_(),
        Type::self_(),
        Type::Builtin(BuiltinType::Iterable),
        Type::Builtin(BuiltinType::Callable),
        Type::Builtin(BuiltinType::Object),
        Type::union(vec![Type::Builtin(BuiltinType::False), Type::string()]),
        Type::Builtin(BuiltinType::True),
        Type::void(),
        Type::never(),
    ];
    let printed: Vec<String> = cases.iter().map(print_type).collect();
    insta::assert_snapshot!(printed.join("\n"));
    // And as actual parameter types (linted).
    let mut f = FunctionDecl::new("typed").returns(Type::void());
    for (i, t) in cases.iter().enumerate() {
        if matches!(
            t,
            Type::Builtin(
                BuiltinType::Void
                    | BuiltinType::Never
                    | BuiltinType::Static
                    | BuiltinType::SelfType
            )
        ) {
            continue;
        }
        f = f.param(Param::new(format!("p{i}")).ty(t.clone()));
    }
    insta::assert_snapshot!(
        "types_as_params",
        file(vec![
            ClassDecl::new("A").into(),
            ClassDecl::new("B").into(),
            ClassDecl::new("C").into(),
            ClassDecl::new("Foo")
                .member(
                    MethodDecl::new(Visibility::Public, "m")
                        .static_()
                        .param(Param::new("x").ty(Type::self_()))
                        .body(vec![])
                )
                .into(),
            f.into(),
        ])
    );
}

#[test]
fn doc_comments() {
    let o = opts();
    let docs = vec![
        DocComment::tag_only("var", "int"),
        DocComment::tag_only("phpstan-type", "UserShape array{id: int, name: string}"),
        DocComment::tag_only("deprecated", ""),
        DocComment::new().line("Summary only."),
        DocComment::new()
            .line("Summary.")
            .line("")
            .line("Longer description\nover lines."),
        DocComment::new()
            .line("Summary.")
            .tag("template", "T")
            .tag("param", "array<T> $xs")
            .tag("return", "T|null"),
        DocComment::new()
            .tag("param", "int $a")
            .tag("param", "int $b"),
        DocComment::tag_only("return", "array{\n  id: int,\n  name: string\n}"),
        DocComment::tag_only("var", "string Contains */ terminator"),
        DocComment::tag_only(
            "var",
            format!(
                "array{{{}}}",
                (0..30)
                    .map(|i| format!("k{i}: int"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        DocComment::new(),
    ];
    let printed: Vec<String> = docs.iter().map(|d| print_doc(d, &o)).collect();
    insta::assert_snapshot!(printed.join("\n---\n"));
}
