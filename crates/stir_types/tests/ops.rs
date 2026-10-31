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
        (Ty::map(tp(), up()), Ty::map(Ty::int(), cls("Dog"))),
        (tp().nullable(), Ty::int().nullable()),
        (fun(vec![tp()], up()), fun(vec![Ty::int()], cls("Dog"))),
        (
            cls_args("Box", vec![tp()]),
            cls_args("Box", vec![Ty::int()]),
        ),
        (rec(&[("x", tp())]), rec(&[("x", Ty::int())])),
        (
            shape(&[("a", tp())], true),
            shape(&[("a", Ty::int())], true),
        ),
        (Ty::platform(tp()), Ty::platform(Ty::int())),
        // substitution re-normalises: `T | Int` with T = Int is `Int`
        (union(vec![tp(), Ty::int()]), Ty::int()),
        (
            Ty::intersection([tp(), up()]),
            Ty::intersection([Ty::int(), cls("Dog")]),
        ),
    ];
    for (before, after) in cases {
        assert_eq!(subst(&before, &map), after, "subst({before})");
    }
    // Substituting with Nothing collapses unions.
    let mut nothing = FxHashMap::default();
    nothing.insert(T_ID, Ty::nothing());
    assert_eq!(
        subst(&union(vec![tp(), Ty::string()]), &nothing),
        Ty::string()
    );
}

#[test]
fn free_params_in_order() {
    let ty = fun(vec![up(), Ty::list(tp())], Ty::map(up(), np()));
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
    assert_eq!(without_null(&Ty::null()), Ty::nothing());
    assert_eq!(
        without_null(&union(vec![Ty::int(), Ty::string(), Ty::null()])),
        union(vec![Ty::int(), Ty::string()])
    );
    assert_eq!(
        without_null(&Ty::platform(Ty::int().nullable())),
        Ty::platform(Ty::int())
    );
    assert_eq!(without_null(&Ty::int()), Ty::int());

    for (ty, expected) in [
        (Ty::int(), false),
        (Ty::int().nullable(), true),
        (Ty::null(), true),
        (Ty::mixed(), true),
        (Ty::any_platform(), true),
        (Ty::platform(Ty::int()), false),
        (Ty::platform(Ty::int().nullable()), true),
        (tp(), true),  // unbounded: Any?
        (up(), false), // bound Pet
        (np(), true),  // bound Int?
        (Ty::php_int().nullable(), true),
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
    assert_eq!(join(&e, &Ty::nothing(), &Ty::string()), Ty::string());
    assert_eq!(
        join(&e, &Ty::int(), &Ty::string()),
        union(vec![Ty::int(), Ty::string()])
    );

    assert_eq!(meet(&e, &cls("Dog"), &cls("Pet")), cls("Dog"));
    assert_eq!(meet(&e, &Ty::int(), &Ty::string()), Ty::nothing());
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
    assert_eq!(meet(&e, &cls("Cat"), &cls("Named")), Ty::nothing());
    assert_eq!(meet(&e, &cls("Pet"), &cls("Robot")), Ty::nothing());
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
        (abc.clone(), s("z"), Ty::nothing()),
        (Ty::any_platform(), Ty::int(), Ty::int()),
        (Ty::mixed(), Ty::php_int(), Ty::php_int()),
        (Ty::platform(Ty::string()), Ty::string(), Ty::string()),
        (Ty::platform(Ty::string()), Ty::null(), Ty::null()),
        (cls("Animal"), cls("Dog"), cls("Dog")),
        (cls("Dog"), cls("Animal"), cls("Dog")),
        (
            cls("Pet"),
            cls("Named"),
            Ty::intersection([cls("Pet"), cls("Named")]),
        ),
        (cls("Cat"), cls("Named"), Ty::nothing()),
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
        (abc.clone(), abc.clone(), Ty::nothing()),
        (Ty::int().nullable(), Ty::null(), Ty::int()),
        (Ty::int(), Ty::null(), Ty::int()),
        (Ty::int(), Ty::lit_int(5), Ty::int()),
        (Ty::bool(), Ty::lit_bool(true), Ty::lit_bool(false)),
        (Ty::bool().nullable(), Ty::null(), Ty::bool()),
        (
            Ty::php_bool(),
            Ty::php_lit(stir_types::LitValue::Bool(false)),
            Ty::php_lit(stir_types::LitValue::Bool(true)),
        ),
        (Ty::platform(Ty::string()), Ty::null(), Ty::string()),
        (
            union(vec![Ty::int(), Ty::string(), Ty::null()]),
            Ty::string(),
            Ty::int().nullable(),
        ),
        (
            union(vec![cls("Dog"), cls("Cat")]),
            cls("Pet"),
            Ty::nothing(),
        ),
        (
            union(vec![cls("Dog"), cls("Robot")]),
            cls("Pet"),
            cls("Robot"),
        ),
        // sealed hierarchies are split
        (cls("Shape"), cls("Circle"), cls("Poly")),
        (
            cls("Shape"),
            cls("Square"),
            union(vec![cls("Circle"), cls("Triangle")]),
        ),
        (cls("Shape"), cls("Poly"), cls("Circle")),
        (cls("Pet"), cls("Dog"), cls("Pet")), // not sealed: nothing to subtract
        (
            cls_args("Res", vec![Ty::int()]),
            cls_args("Ok", vec![Ty::int()]),
            cls("Err"),
        ),
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
