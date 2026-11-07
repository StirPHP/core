mod common;

use common::*;
use pretty_assertions::assert_eq;
use stir_types::{FnParam, FnTy, ShapeField, Ty, TyKind, TyList};

fn closure_sig(params: Vec<Ty>, ret: Ty) -> FnTy {
    FnTy::new(params.into_iter().map(FnParam::required).collect(), ret)
}

/// `(type, Display)`
fn cases() -> Vec<(Ty, &'static str)> {
    vec![
        (Ty::int(), "int"),
        (Ty::float(), "float"),
        (Ty::string(), "string"),
        (Ty::bool(), "bool"),
        (Ty::void(), "void"),
        (Ty::never(), "never"),
        (Ty::null(), "null"),
        (Ty::mixed(), "mixed"),
        (Ty::untyped(), "mixed"),
        (Ty::error(), "mixed"),
        (Ty::object(), "object"),
        (Ty::resource(), "resource"),
        (Ty::scalar(), "scalar"),
        (Ty::numeric(), "numeric"),
        (Ty::array_key(), "array-key"),
        (Ty::non_empty_string(), "non-empty-string"),
        (Ty::numeric_string(), "numeric-string"),
        (Ty::class_string(None), "class-string"),
        (Ty::class_string(Some(cls("Dog"))), "class-string<Dog>"),
        // nullability and unions
        (Ty::int().nullable(), "?int"),
        (Ty::list(Ty::int()).nullable(), "?list<int>"),
        (cls("Dog").nullable(), "?Dog"),
        (tp().nullable(), "?T"),
        (union(vec![Ty::int(), Ty::string()]), "int|string"),
        (
            union(vec![Ty::int(), Ty::string(), Ty::null()]),
            "int|string|null",
        ),
        // literals
        (union(vec![s("active"), s("banned")]), "'active'|'banned'"),
        (s("it's \"q\" $x"), "'it\\'s \"q\" $x'"),
        (Ty::lit_int(-5), "-5"),
        (Ty::lit_float(1.0), "1.0"),
        (Ty::lit_bool(true), "true"),
        // classes
        (cls_args("App\\Box", vec![Ty::int()]), "App\\Box<int>"),
        (tp(), "T"),
        (Ty::static_of(cls("Dog")), "static"),
        (Ty::this_of(cls("Dog")), "$this"),
        (Ty::intersection([cls("Pet"), cls("Named")]), "Pet&Named"),
        (
            Ty::intersection([cls("Pet"), cls("Named")]).nullable(),
            "(Pet&Named)|null",
        ),
        // refinements
        (Ty::positive_int(), "positive-int"),
        (Ty::non_negative_int(), "non-negative-int"),
        (Ty::int_range(None, Some(-1)), "negative-int"),
        (Ty::int_range(None, Some(0)), "non-positive-int"),
        (Ty::int_range(Some(1), Some(10)), "int<1, 10>"),
        (Ty::int_range(Some(-3), None), "int<-3, max>"),
        (Ty::int_range(None, Some(7)), "int<min, 7>"),
        // arrays
        (Ty::plain_array(), "array"),
        (Ty::array(Ty::array_key(), Ty::int()), "array<int>"),
        (Ty::array(Ty::string(), Ty::int()), "array<string, int>"),
        (
            Ty::non_empty_array(Ty::int(), Ty::string()),
            "non-empty-array<int, string>",
        ),
        (
            Ty::non_empty_array(Ty::array_key(), Ty::mixed()),
            "non-empty-array",
        ),
        (Ty::list(Ty::int()), "list<int>"),
        (Ty::non_empty_list(Ty::string()), "non-empty-list<string>"),
        (Ty::plain_iterable(), "iterable"),
        (Ty::iterable(Ty::mixed(), Ty::int()), "iterable<int>"),
        (
            Ty::iterable(Ty::string(), Ty::int()),
            "iterable<string, int>",
        ),
        (
            Ty::shape(
                vec![
                    ShapeField::new("a", Ty::int()),
                    ShapeField::optional("b", Ty::string()),
                ],
                true,
            ),
            "array{a: int, b?: string}",
        ),
        (shape(&[("a", Ty::int())], false), "array{a: int, ...}"),
        (shape(&[], false), "array{...}"),
        (
            Ty::shape(
                vec![
                    ShapeField::new(0, Ty::int()),
                    ShapeField::new(1, Ty::string()),
                ],
                true,
            ),
            "array{int, string}",
        ),
        (
            shape(&[("my key", Ty::int())], true),
            "array{'my key': int}",
        ),
        // callables
        (Ty::callable(None), "callable"),
        (
            Ty::callable(Some(closure_sig(vec![Ty::int()], Ty::string()))),
            "callable(int): string",
        ),
        (cls("Closure"), "Closure"),
        (fun(vec![Ty::int()], Ty::bool()), "Closure(int): bool"),
        (fun(vec![], Ty::void()), "Closure(): void"),
        (
            func(
                vec![
                    FnParam::required(Ty::int()),
                    FnParam::optional(Ty::string()),
                    FnParam::variadic(Ty::int()),
                ],
                Ty::void(),
            ),
            "Closure(int, string=, int...): void",
        ),
        (
            fun(vec![Ty::int()], Ty::bool()).nullable(),
            "(Closure(int): bool)|null",
        ),
        (
            fun(vec![Ty::int()], fun(vec![Ty::int()], Ty::int())),
            "Closure(int): (Closure(int): int)",
        ),
        (
            fun(vec![], union(vec![Ty::int(), Ty::string()])),
            "Closure(): (int|string)",
        ),
        (
            Ty::callable(Some(closure_sig(vec![], Ty::int().nullable()))),
            "callable(): ?int",
        ),
        (
            Ty::callable(Some(closure_sig(vec![], Ty::void()))).nullable(),
            "(callable(): void)|null",
        ),
    ]
}

#[test]
fn display() {
    let mut failures = Vec::new();
    for (ty, expected) in cases() {
        let got = ty.to_string();
        if got != expected {
            failures.push(format!("  expected `{expected}`, got `{got}`"));
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
    // ?T == T|null, and nullable is idempotent
    assert_eq!(int.clone().nullable(), union(vec![int.clone(), Ty::null()]));
    assert_eq!(int.clone().nullable().nullable(), int.clone().nullable());
    // never drops out
    assert_eq!(union(vec![int.clone(), Ty::never()]), int);
    assert_eq!(union(vec![]), Ty::never());
    assert_eq!(union(vec![int.clone()]), int);
    // Error is contagious, mixed and untyped values absorb
    assert_eq!(union(vec![int.clone(), Ty::error()]), Ty::error());
    assert_eq!(union(vec![Ty::int(), Ty::mixed()]), Ty::mixed());
    assert_eq!(union(vec![Ty::mixed(), Ty::untyped()]), Ty::untyped());
    assert_eq!(Ty::untyped().nullable(), Ty::untyped());
    // literals are absorbed by their base type
    assert_eq!(union(vec![s("a"), str_.clone()]), str_);
    assert_eq!(union(vec![Ty::lit_int(1), Ty::int()]), Ty::int());
    assert_eq!(union(vec![Ty::positive_int(), Ty::int()]), Ty::int());
    assert_eq!(
        union(vec![Ty::non_empty_string(), Ty::string()]),
        Ty::string()
    );
    // true | false
    assert_eq!(
        union(vec![Ty::lit_bool(true), Ty::lit_bool(false)]),
        Ty::bool()
    );
    // intersections
    assert_eq!(
        Ty::intersection([cls("A"), Ty::intersection([cls("B"), cls("A")])]),
        Ty::intersection([cls("B"), cls("A")])
    );
    assert_eq!(Ty::intersection([cls("A"), Ty::never()]), Ty::never());
    assert_eq!(Ty::intersection([cls("A"), Ty::mixed()]), cls("A"));
    // int ranges
    assert_eq!(Ty::int_range(None, None), Ty::int());
    assert_eq!(Ty::int_range(Some(3), Some(3)), Ty::lit_int(3));
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
    // shape keys
    assert_eq!(
        shape(&[("1", Ty::int())], true),
        Ty::shape(vec![ShapeField::new(1, Ty::int())], true)
    );
    assert_ne!(
        shape(&[("01", Ty::int())], true),
        Ty::shape(vec![ShapeField::new(1, Ty::int())], true)
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
fn many_arrays_of_the_same_form_are_one() {
    // a few stay apart
    let few = union(
        (0..16)
            .map(|n| shape(&[("a", Ty::lit_int(n))], true))
            .collect(),
    );
    assert_eq!(few.members().len(), 16);
    // more become one shape, beside what isn't one
    let many = union(
        (0..17)
            .map(|n| shape(&[("a", Ty::lit_int(n)), ("b", Ty::string())], true))
            .chain([shape(&[("a", Ty::int())], true), Ty::null()])
            .collect(),
    );
    assert_eq!(
        many,
        union(vec![
            shape(
                &[
                    ("a", union((0..17).map(Ty::lit_int).collect())),
                    ("b", Ty::string())
                ],
                true
            ),
            shape(&[("a", Ty::int())], true),
            Ty::null(),
        ])
    );
    // as do arrays with the same key type
    let arrays = union(
        (0..17)
            .map(|n| Ty::non_empty_array(Ty::string(), shape(&[("a", Ty::lit_int(n))], true)))
            .collect(),
    );
    assert_eq!(
        arrays,
        Ty::non_empty_array(
            Ty::string(),
            shape(&[("a", union((0..17).map(Ty::lit_int).collect()))], true)
        )
    );
}
