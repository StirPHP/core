mod common;

use common::*;
use pretty_assertions::assert_eq;
use stir_types::{
    EmptyEnv, FxHashMap, ParamId, Ty, free_params, is_nullable, join, meet, narrow_away, narrow_to,
    subst, without_null,
};

#[test]
fn substitution() {
    let mut map = FxHashMap::default();
    map.insert(T_ID, Ty::int());
    map.insert(U_ID, cls("Dog"));
    let cases = [
        (tp(), Ty::int()),
        (np(), np()),
        (Ty::list(tp()), Ty::list(Ty::int())),
        (Ty::array(tp(), up()), Ty::array(Ty::int(), cls("Dog"))),
        (tp().nullable(), Ty::int().nullable()),
        (fun(vec![tp()], up()), fun(vec![Ty::int()], cls("Dog"))),
        (
            cls_args("Box", vec![tp()]),
            cls_args("Box", vec![Ty::int()]),
        ),
        (
            shape(&[("a", tp())], true),
            shape(&[("a", Ty::int())], true),
        ),
        // substitution re-normalises: `T|int` with T = int is `int`
        (union(vec![tp(), Ty::int()]), Ty::int()),
        (
            Ty::intersection([tp(), up()]),
            Ty::intersection([Ty::int(), cls("Dog")]),
        ),
    ];
    for (before, after) in cases {
        assert_eq!(subst(&before, &map), after, "subst({before})");
    }
    // Substituting with never collapses unions.
    let mut never = FxHashMap::default();
    never.insert(T_ID, Ty::never());
    assert_eq!(
        subst(&union(vec![tp(), Ty::string()]), &never),
        Ty::string()
    );
}

#[test]
fn free_params_in_order() {
    let ty = fun(vec![up(), Ty::list(tp())], Ty::array(up(), np()));
    assert_eq!(free_params(&ty), vec![U_ID, T_ID, N_ID]);
    assert_eq!(free_params(&Ty::int()), Vec::<ParamId>::new());
    assert_eq!(
        free_params(&Ty::static_of(cls_args("Box", vec![tp()]))),
        vec![T_ID]
    );
}

#[test]
fn nullability() {
    let e = env();
    assert_eq!(without_null(&Ty::int().nullable()), Ty::int());
    assert_eq!(without_null(&Ty::null()), Ty::never());
    assert_eq!(
        without_null(&union(vec![Ty::int(), Ty::string(), Ty::null()])),
        union(vec![Ty::int(), Ty::string()])
    );
    assert_eq!(without_null(&Ty::int()), Ty::int());

    for (ty, expected) in [
        (Ty::int(), false),
        (Ty::int().nullable(), true),
        (Ty::null(), true),
        (Ty::mixed(), true),
        (Ty::untyped(), true),
        (tp(), true),  // unbounded: mixed
        (up(), false), // bound Pet
        (np(), true),  // bound ?int
    ] {
        assert_eq!(is_nullable(&e, &ty), expected, "is_nullable({ty})");
    }
}

#[test]
fn join_and_meet() {
    let e = env();
    assert_eq!(join(&e, &Ty::lit_int(1), &Ty::int()), Ty::int());
    assert_eq!(join(&e, &cls("Dog"), &cls("Pet")), cls("Pet"));
    assert_eq!(
        join(&e, &cls("Dog"), &cls("Cat")),
        union(vec![cls("Dog"), cls("Cat")])
    );
    assert_eq!(join(&e, &Ty::int(), &Ty::null()), Ty::int().nullable());
    assert_eq!(join(&e, &Ty::never(), &Ty::string()), Ty::string());
    assert_eq!(
        join(&e, &Ty::int(), &Ty::string()),
        union(vec![Ty::int(), Ty::string()])
    );

    // a plain `Closure` is more general than one with a signature, either way round
    let closure = fun(vec![Ty::int()], Ty::bool());
    assert_eq!(join(&e, &cls("Closure"), &closure), cls("Closure"));
    assert_eq!(join(&e, &closure, &cls("Closure")), cls("Closure"));

    assert_eq!(meet(&e, &cls("Dog"), &cls("Pet")), cls("Dog"));
    assert_eq!(meet(&e, &Ty::int(), &Ty::string()), Ty::never());
    assert_eq!(meet(&e, &Ty::int().nullable(), &Ty::int()), Ty::int());
    assert_eq!(
        meet(
            &e,
            &union(vec![Ty::int(), Ty::string()]),
            &union(vec![Ty::string(), Ty::bool()])
        ),
        Ty::string()
    );
    assert_eq!(
        meet(&e, &cls("Pet"), &cls("Named")),
        Ty::intersection([cls("Pet"), cls("Named")])
    );
    assert_eq!(meet(&e, &cls("Cat"), &cls("Named")), Ty::never());
    assert_eq!(meet(&e, &cls("Pet"), &cls("Robot")), Ty::never());
}

#[test]
fn narrowing_to() {
    let e = env();
    let abc = union(vec![s("a"), s("b"), s("c")]);
    let cases = [
        (Ty::int().nullable(), Ty::null(), Ty::null()),
        (Ty::int().nullable(), Ty::int(), Ty::int()),
        (Ty::string(), s("a"), s("a")),
        (abc.clone(), s("a"), s("a")),
        (
            abc.clone(),
            union(vec![s("a"), s("c")]),
            union(vec![s("a"), s("c")]),
        ),
        (abc.clone(), s("z"), Ty::never()),
        (Ty::untyped(), Ty::int(), Ty::int()),
        (Ty::mixed(), Ty::int(), Ty::int()),
        (cls("Animal"), cls("Dog"), cls("Dog")),
        (cls("Dog"), cls("Animal"), cls("Dog")),
        (
            cls("Pet"),
            cls("Named"),
            Ty::intersection([cls("Pet"), cls("Named")]),
        ),
        (cls("Cat"), cls("Named"), Ty::never()),
        (union(vec![cls("Dog"), Ty::int()]), cls("Pet"), cls("Dog")),
        (Ty::bool(), Ty::lit_bool(true), Ty::lit_bool(true)),
        (tp(), Ty::int(), Ty::intersection([tp(), Ty::int()])),
        (up(), cls("Animal"), up()),
        (Ty::error(), Ty::int(), Ty::error()),
    ];
    for (declared, test, expected) in cases {
        assert_eq!(
            narrow_to(&e, &declared, &test),
            expected,
            "narrow_to({declared}, {test})"
        );
    }
}

#[test]
fn narrowing_away() {
    let e = env();
    let abc = union(vec![s("a"), s("b"), s("c")]);
    let cases = [
        (abc.clone(), s("a"), union(vec![s("b"), s("c")])),
        (abc.clone(), union(vec![s("a"), s("b")]), s("c")),
        (abc.clone(), abc.clone(), Ty::never()),
        (Ty::int().nullable(), Ty::null(), Ty::int()),
        (Ty::int(), Ty::null(), Ty::int()),
        (Ty::int(), Ty::lit_int(5), Ty::int()),
        (Ty::bool(), Ty::lit_bool(true), Ty::lit_bool(false)),
        (Ty::bool().nullable(), Ty::null(), Ty::bool()),
        (Ty::bool(), Ty::lit_bool(false), Ty::lit_bool(true)),
        (
            union(vec![Ty::int(), Ty::string(), Ty::null()]),
            Ty::string(),
            Ty::int().nullable(),
        ),
        (union(vec![cls("Dog"), cls("Cat")]), cls("Pet"), Ty::never()),
        (
            union(vec![cls("Dog"), cls("Robot")]),
            cls("Pet"),
            cls("Robot"),
        ),
        (cls("Pet"), cls("Dog"), cls("Pet")), // nothing to subtract
    ];
    for (declared, test, expected) in cases {
        assert_eq!(
            narrow_away(&e, &declared, &test),
            expected,
            "narrow_away({declared}, {test})"
        );
    }
    assert_eq!(
        narrow_away(&EmptyEnv, &Ty::error(), &Ty::null()),
        Ty::error()
    );
}
