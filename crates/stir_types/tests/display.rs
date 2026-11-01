mod common;

use common::*;
use pretty_assertions::assert_eq;
use stir_types::{
    FnParam, FnTy, LitValue, ShapeField, TopKind, Ty, TyKind, TyList, to_phpdoc, top_type_violation,
};

fn closure_sig(params: Vec<Ty>, ret: Ty) -> FnTy {
    FnTy::new(params.into_iter().map(FnParam::required).collect(), ret)
}

/// `(type, Display, to_phpdoc)`
fn cases() -> Vec<(Ty, &'static str, &'static str)> {
    let bx = cls_args("App\\Box", vec![Ty::int()]);
    vec![
        // StirPHP family
        (Ty::int(), "Int", "int"),
        (Ty::float(), "Float", "float"),
        (Ty::string(), "String", "string"),
        (Ty::bool(), "Bool", "bool"),
        (Ty::unit(), "Unit", "void"),
        (Ty::nothing(), "Nothing", "never"),
        (Ty::int().nullable(), "Int?", "?int"),
        (Ty::list(Ty::string()), "List<String>", "list<string>"),
        (
            Ty::mutable_list(Ty::int().nullable()),
            "MutableList<Int?>",
            "list<?int>",
        ),
        (
            Ty::map(Ty::string(), Ty::int()),
            "Map<String, Int>",
            "array<string, int>",
        ),
        (
            Ty::mutable_map(Ty::int(), Ty::bool()),
            "MutableMap<Int, Bool>",
            "array<int, bool>",
        ),
        (Ty::list(Ty::int()).nullable(), "List<Int>?", "?list<int>"),
        (
            union(vec![Ty::int(), Ty::string()]),
            "Int | String",
            "int|string",
        ),
        (
            union(vec![Ty::int(), Ty::string(), Ty::null()]),
            "Int | String | null",
            "int|string|null",
        ),
        (
            union(vec![s("active"), s("banned")]),
            "\"active\" | \"banned\"",
            "'active'|'banned'",
        ),
        (
            s("it's \"q\" $x"),
            "\"it's \\\"q\\\" \\$x\"",
            "'it\\'s \"q\" $x'",
        ),
        (Ty::lit_int(-5), "-5", "-5"),
        (Ty::lit_float(1.0), "1.0", "1.0"),
        (Ty::lit_bool(true), "true", "true"),
        (
            fun(vec![Ty::int()], Ty::bool()),
            "(Int) -> Bool",
            "\\Closure(int): bool",
        ),
        (fun(vec![], Ty::unit()), "() -> Unit", "\\Closure(): void"),
        (
            Ty::func(
                vec![
                    FnParam::required(Ty::int()),
                    FnParam::optional(Ty::string()),
                    FnParam::variadic(Ty::int()),
                ],
                Ty::unit(),
            ),
            "(Int, String=, ...Int) -> Unit",
            "\\Closure(int, string=, int...): void",
        ),
        (
            fun(vec![Ty::int()], Ty::bool()).nullable(),
            "((Int) -> Bool)?",
            "(\\Closure(int): bool)|null",
        ),
        (
            fun(vec![Ty::int()], fun(vec![Ty::int()], Ty::int())),
            "(Int) -> (Int) -> Int",
            "\\Closure(int): (\\Closure(int): int)",
        ),
        (
            fun(vec![], union(vec![Ty::int(), Ty::string()])),
            "() -> (Int | String)",
            "\\Closure(): (int|string)",
        ),
        (
            rec(&[("x", Ty::int()), ("y", Ty::int())]),
            "{ x: Int, y: Int }",
            "array{x: int, y: int}",
        ),
        (rec(&[]), "{}", "array{}"),
        (
            Ty::named_record("Point", vec![], [("x", Ty::int())]),
            "Point",
            "array{x: int}",
        ),
        (
            Ty::named_record(
                "Pair",
                vec![Ty::int(), Ty::string()],
                [("a", Ty::int()), ("b", Ty::string())],
            ),
            "Pair<Int, String>",
            "array{a: int, b: string}",
        ),
        (Ty::platform(Ty::string()), "String!", "string"),
        (Ty::platform(Ty::string().nullable()), "String?!", "?string"),
        (
            Ty::platform(union(vec![Ty::int(), Ty::string()])),
            "(Int | String)!",
            "int|string",
        ),
        (Ty::any_platform(), "Any!", "mixed"),
        (Ty::error(), "{error}", "mixed"),
        (bx.clone(), "App\\Box<Int>", "\\App\\Box<int>"),
        (cls("Dog").nullable(), "Dog?", "?\\Dog"),
        (tp(), "T", "T"),
        (tp().nullable(), "T?", "?T"),
        (Ty::static_of(cls("Dog")), "static", "static"),
        (Ty::this_of(cls("Dog")), "$this", "$this"),
        (
            Ty::intersection([cls("Pet"), cls("Named")]),
            "Pet & Named",
            "\\Pet&\\Named",
        ),
        (
            Ty::intersection([cls("Pet"), cls("Named")]).nullable(),
            "(Pet & Named)?",
            "(\\Pet&\\Named)|null",
        ),
        // PHP family
        (Ty::php_int(), "int", "int"),
        (Ty::php_int().nullable(), "?int", "?int"),
        (
            union(vec![Ty::php_int(), Ty::php_string()]),
            "int|string",
            "int|string",
        ),
        (
            union(vec![Ty::php_int(), Ty::php_string(), Ty::null()]),
            "int|string|null",
            "int|string|null",
        ),
        (Ty::void(), "void", "void"),
        (Ty::never(), "never", "never"),
        (Ty::null(), "null", "null"),
        (Ty::mixed(), "mixed", "mixed"),
        (Ty::object(), "object", "object"),
        (Ty::resource(), "resource", "resource"),
        (Ty::scalar(), "scalar", "scalar"),
        (Ty::numeric(), "numeric", "numeric"),
        (Ty::array_key(), "array-key", "array-key"),
        (
            Ty::non_empty_string(),
            "non-empty-string",
            "non-empty-string",
        ),
        (Ty::numeric_string(), "numeric-string", "numeric-string"),
        (Ty::class_string(None), "class-string", "class-string"),
        (
            Ty::class_string(Some(cls("Dog"))),
            "class-string<Dog>",
            "class-string<\\Dog>",
        ),
        (Ty::positive_int(), "positive-int", "positive-int"),
        (
            Ty::non_negative_int(),
            "non-negative-int",
            "non-negative-int",
        ),
        (
            Ty::int_range(None, Some(-1)),
            "negative-int",
            "negative-int",
        ),
        (
            Ty::int_range(None, Some(0)),
            "non-positive-int",
            "non-positive-int",
        ),
        (Ty::int_range(Some(1), Some(10)), "int<1, 10>", "int<1, 10>"),
        (
            Ty::int_range(Some(-3), None),
            "int<-3, max>",
            "int<-3, max>",
        ),
        (Ty::int_range(None, Some(7)), "int<min, 7>", "int<min, 7>"),
        (Ty::php_array(), "array", "array"),
        (
            Ty::array(Ty::array_key(), Ty::php_int()),
            "array<int>",
            "array<int>",
        ),
        (
            Ty::array(Ty::php_string(), Ty::php_int()),
            "array<string, int>",
            "array<string, int>",
        ),
        (
            Ty::non_empty_array(Ty::php_int(), Ty::php_string()),
            "non-empty-array<int, string>",
            "non-empty-array<int, string>",
        ),
        (
            Ty::non_empty_array(Ty::array_key(), Ty::mixed()),
            "non-empty-array",
            "non-empty-array",
        ),
        (Ty::php_list(Ty::php_int()), "list<int>", "list<int>"),
        (
            Ty::non_empty_list(Ty::php_string()),
            "non-empty-list<string>",
            "non-empty-list<string>",
        ),
        (Ty::php_iterable(), "iterable", "iterable"),
        (
            Ty::iterable(Ty::mixed(), Ty::php_int()),
            "iterable<int>",
            "iterable<int>",
        ),
        (
            Ty::iterable(Ty::php_string(), Ty::php_int()),
            "iterable<string, int>",
            "iterable<string, int>",
        ),
        (
            Ty::shape(
                vec![
                    ShapeField::new("a", Ty::php_int()),
                    ShapeField::optional("b", Ty::php_string()),
                ],
                true,
            ),
            "array{a: int, b?: string}",
            "array{a: int, b?: string}",
        ),
        (
            shape(&[("a", Ty::php_int())], false),
            "array{a: int, ...}",
            "array{a: int, ...}",
        ),
        (shape(&[], false), "array{...}", "array{...}"),
        (
            Ty::shape(
                vec![
                    ShapeField::new(0, Ty::php_int()),
                    ShapeField::new(1, Ty::php_string()),
                ],
                true,
            ),
            "array{int, string}",
            "array{int, string}",
        ),
        (
            shape(&[("my key", Ty::php_int())], true),
            "array{'my key': int}",
            "array{'my key': int}",
        ),
        (Ty::callable(None), "callable", "callable"),
        (
            Ty::callable(Some(closure_sig(vec![Ty::php_int()], Ty::php_string()))),
            "callable(int): string",
            "callable(int): string",
        ),
        (
            Ty::closure(Some(closure_sig(vec![Ty::php_int()], Ty::php_string()))),
            "Closure(int): string",
            "\\Closure(int): string",
        ),
        (
            Ty::callable(Some(closure_sig(vec![], Ty::php_int().nullable()))),
            "callable(): ?int",
            "callable(): ?int",
        ),
        (
            Ty::callable(Some(closure_sig(
                vec![],
                union(vec![Ty::php_int(), Ty::php_string()]),
            ))),
            "callable(): (int|string)",
            "callable(): (int|string)",
        ),
        (
            Ty::callable(Some(closure_sig(vec![], Ty::void()))).nullable(),
            "(callable(): void)|null",
            "(callable(): void)|null",
        ),
        (cls("Closure"), "Closure", "\\Closure"),
        (ps("a"), "'a'", "'a'"),
        (pi(3), "3", "3"),
        (union(vec![ps("a"), ps("b")]), "'a'|'b'", "'a'|'b'"),
        // mixed families
        (
            union(vec![Ty::int(), Ty::php_string()]),
            "Int | string",
            "int|string",
        ),
        (Ty::list(Ty::php_int()), "List<int>", "list<int>"),
    ]
}

#[test]
fn display_and_phpdoc() {
    let mut failures = Vec::new();
    for (ty, display, phpdoc) in cases() {
        let got_display = ty.to_string();
        let got_doc = to_phpdoc(&ty);
        if got_display != display {
            failures.push(format!(
                "  display: expected `{display}`, got `{got_display}`"
            ));
        }
        if got_doc != phpdoc {
            failures.push(format!(
                "  phpdoc of {display}: expected `{phpdoc}`, got `{got_doc}`"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "rendering failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn normalisation() {
    let int = Ty::int();
    let str_ = Ty::string();
    // nested unions flatten, duplicates go, order doesn't matter for equality
    assert_eq!(
        union(vec![int.clone(), union(vec![str_.clone(), int.clone()])]),
        union(vec![str_.clone(), int.clone()])
    );
    // T? == T | null, and nullable is idempotent
    assert_eq!(int.clone().nullable(), union(vec![int.clone(), Ty::null()]));
    assert_eq!(int.clone().nullable().nullable(), int.clone().nullable());
    // Nothing / never drop out
    assert_eq!(union(vec![int.clone(), Ty::nothing()]), int);
    assert_eq!(union(vec![Ty::php_int(), Ty::never()]), Ty::php_int());
    assert_eq!(union(vec![]), Ty::nothing());
    assert_eq!(union(vec![int.clone()]), int);
    // Error is contagious, mixed and Any! absorb
    assert_eq!(union(vec![int.clone(), Ty::error()]), Ty::error());
    assert_eq!(union(vec![Ty::php_int(), Ty::mixed()]), Ty::mixed());
    assert_eq!(
        union(vec![Ty::mixed(), Ty::any_platform()]),
        Ty::any_platform()
    );
    assert_eq!(Ty::any_platform().nullable(), Ty::any_platform());
    // literals are absorbed by their base type
    assert_eq!(union(vec![s("a"), str_.clone()]), str_);
    assert_eq!(union(vec![Ty::lit_int(1), Ty::php_int()]), Ty::php_int());
    assert_eq!(union(vec![pi(1), Ty::int()]), union(vec![pi(1), Ty::int()]));
    assert_eq!(
        union(vec![Ty::positive_int(), Ty::php_int()]),
        Ty::php_int()
    );
    assert_eq!(
        union(vec![Ty::non_empty_string(), Ty::php_string()]),
        Ty::php_string()
    );
    // true | false
    assert_eq!(
        union(vec![Ty::lit_bool(true), Ty::lit_bool(false)]),
        Ty::bool()
    );
    assert_eq!(
        union(vec![
            Ty::php_lit(LitValue::Bool(false)),
            Ty::php_lit(LitValue::Bool(true))
        ]),
        Ty::php_bool()
    );
    // intersections
    assert_eq!(
        Ty::intersection([cls("A"), Ty::intersection([cls("B"), cls("A")])]),
        Ty::intersection([cls("B"), cls("A")])
    );
    assert_eq!(Ty::intersection([cls("A"), Ty::nothing()]), Ty::nothing());
    assert_eq!(Ty::intersection([cls("A"), Ty::mixed()]), cls("A"));
    // platform
    assert_eq!(
        Ty::platform(Ty::platform(int.clone())),
        Ty::platform(int.clone())
    );
    assert_eq!(Ty::platform(Ty::mixed()), Ty::any_platform());
    assert_eq!(Ty::platform(Ty::error()), Ty::error());
    assert_eq!(Ty::platform(Ty::null()), Ty::null());
    // int ranges
    assert_eq!(Ty::int_range(None, None), Ty::php_int());
    assert_eq!(Ty::int_range(Some(3), Some(3)), pi(3));
    assert_eq!(Ty::int_range(Some(3), Some(1)), Ty::never());
    // class names, Closure
    assert_eq!(
        Ty::class("\\App\\Foo", vec![]),
        Ty::class("App\\Foo", vec![])
    );
    assert_eq!(Ty::closure(None), cls("Closure"));
    // Ty::new normalises too
    assert_eq!(
        Ty::new(TyKind::Union(TyList::new(vec![int.clone(), int.clone()]))),
        int
    );
    assert_eq!(Ty::new(TyKind::Platform(Ty::mixed())), Ty::any_platform());
    // shape keys
    assert_eq!(
        shape(&[("1", Ty::php_int())], true),
        Ty::shape(vec![ShapeField::new(1, Ty::php_int())], true)
    );
    assert_ne!(
        shape(&[("01", Ty::php_int())], true),
        Ty::shape(vec![ShapeField::new(1, Ty::php_int())], true)
    );
}

#[test]
fn union_hash_is_order_independent() {
    use std::collections::HashSet;
    let a = union(vec![Ty::int(), Ty::string(), Ty::null()]);
    let b = union(vec![Ty::null(), Ty::string(), Ty::int()]);
    let set: HashSet<Ty> = [a, b].into_iter().collect();
    assert_eq!(set.len(), 1);
}

#[test]
fn top_type_rule() {
    let cases: Vec<(Ty, Option<(Ty, TopKind)>)> = vec![
        (Ty::int(), None),
        (Ty::list(tp()), None),
        (Ty::any_platform(), None),
        (Ty::platform(Ty::php_array()), None),
        (Ty::mixed(), Some((Ty::mixed(), TopKind::Mixed))),
        (Ty::list(Ty::mixed()), Some((Ty::mixed(), TopKind::Mixed))),
        (
            fun(vec![Ty::int()], Ty::mixed()),
            Some((Ty::mixed(), TopKind::Mixed)),
        ),
        (
            rec(&[("x", Ty::array(Ty::php_int(), Ty::mixed()))]),
            Some((Ty::mixed(), TopKind::Mixed)),
        ),
        (
            Ty::php_array(),
            Some((Ty::php_array(), TopKind::ImplicitMixed)),
        ),
        (
            Ty::map(Ty::string(), Ty::php_iterable()),
            Some((Ty::php_iterable(), TopKind::ImplicitMixed)),
        ),
        (
            Ty::callable(None).nullable(),
            Some((Ty::callable(None), TopKind::ImplicitMixed)),
        ),
        (Ty::array(Ty::php_string(), Ty::php_int()), None),
    ];
    for (ty, expected) in cases {
        let got = top_type_violation(&ty).map(|v| (v.ty, v.kind));
        assert_eq!(got, expected, "top_type_violation({ty})");
    }
    assert!(Ty::list(Ty::any_platform()).contains_any_platform());
    assert!(!Ty::list(Ty::any_platform()).is_any_platform());
    assert!(Ty::any_platform().is_any_platform());
}
