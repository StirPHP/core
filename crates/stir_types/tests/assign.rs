mod common;

use common::*;
use pretty_assertions::assert_eq;
use stir_types::{
    EmptyEnv, FnParam, FnTy, MismatchReason, ShapeField, ShapeKey, Ty, Variance, check_assignable,
    is_assignable, is_equivalent,
};

#[test]
fn bottom_error_and_identity() {
    let e = env();
    assert_assign_table(
        &e,
        &[
            (Ty::never(), Ty::int(), true),
            (Ty::never(), cls("Dog"), true),
            (Ty::never(), Ty::list(Ty::int()), true),
            (Ty::error(), Ty::int(), true),
            (Ty::int(), Ty::error(), true),
            (Ty::error(), Ty::never(), true),
            (Ty::int(), Ty::int(), true),
            (Ty::int(), Ty::never(), false),
            (Ty::void(), Ty::void(), true),
            (Ty::void(), Ty::int(), false),
        ],
    );
}

#[test]
fn builtins() {
    let e = env();
    assert_assign_table(
        &e,
        &[
            (Ty::int(), Ty::scalar(), true),
            (Ty::int(), Ty::numeric(), true),
            (Ty::string(), Ty::array_key(), true),
            (Ty::int(), Ty::mixed(), true),
            (Ty::list(Ty::int()), Ty::mixed(), true),
            (Ty::string(), Ty::non_empty_string(), false),
            (Ty::int(), Ty::positive_int(), false),
            (Ty::int(), Ty::string(), false),
            (Ty::bool(), Ty::int(), false),
        ],
    );
}

#[test]
fn numeric_widening() {
    let e = env();
    assert_assign_table(
        &e,
        &[
            // int -> float is allowed even under strict_types.
            (Ty::int(), Ty::float(), true),
            (Ty::lit_int(1), Ty::float(), true),
            (Ty::positive_int(), Ty::float(), true),
            (Ty::float(), Ty::int(), false),
        ],
    );
}

#[test]
fn literals() {
    let e = env();
    let status = union(vec![s("active"), s("banned")]);
    assert_assign_table(
        &e,
        &[
            (Ty::lit_int(5), Ty::int(), true),
            (Ty::lit_int(5), Ty::positive_int(), true),
            (Ty::lit_int(0), Ty::positive_int(), false),
            (Ty::lit_int(5), Ty::int_range(Some(0), Some(10)), true),
            (Ty::lit_int(11), Ty::int_range(Some(0), Some(10)), false),
            (Ty::lit_int(5), Ty::float(), true),
            (Ty::lit_int(5), Ty::lit_int(6), false),
            (Ty::lit_float(1.5), Ty::float(), true),
            (Ty::lit_bool(true), Ty::bool(), true),
            (s("a"), Ty::string(), true),
            (s("a"), Ty::non_empty_string(), true),
            (s(""), Ty::non_empty_string(), false),
            (s("12"), Ty::numeric_string(), true),
            (s(" 1.5e3 "), Ty::numeric(), true),
            (s("abc"), Ty::numeric_string(), false),
            (s("a"), Ty::int(), false),
            (Ty::string(), s("a"), false),
            (Ty::int(), Ty::lit_int(5), false),
            // literal unions
            (s("active"), status.clone(), true),
            (s("deleted"), status.clone(), false),
            (status.clone(), Ty::string(), true),
            (
                status.clone(),
                union(vec![s("active"), s("banned"), s("deleted")]),
                true,
            ),
            (
                union(vec![s("active"), s("deleted")]),
                status.clone(),
                false,
            ),
            (Ty::string(), status.clone(), false),
        ],
    );
}

#[test]
fn php_refinements() {
    let e = env();
    assert_assign_table(
        &e,
        &[
            (Ty::positive_int(), Ty::int(), true),
            (Ty::positive_int(), Ty::non_negative_int(), true),
            (Ty::non_negative_int(), Ty::positive_int(), false),
            (
                Ty::int_range(Some(2), Some(5)),
                Ty::int_range(Some(0), Some(10)),
                true,
            ),
            (
                Ty::int_range(Some(-2), Some(5)),
                Ty::int_range(Some(0), Some(10)),
                false,
            ),
            (Ty::int(), Ty::positive_int(), false),
            (Ty::non_empty_string(), Ty::string(), true),
            (Ty::numeric_string(), Ty::non_empty_string(), true),
            (Ty::numeric_string(), Ty::numeric(), true),
            (Ty::string(), Ty::numeric(), false),
            (
                Ty::class_string(Some(cls("Dog"))),
                Ty::class_string(Some(cls("Pet"))),
                true,
            ),
            (
                Ty::class_string(Some(cls("Pet"))),
                Ty::class_string(Some(cls("Dog"))),
                false,
            ),
            (
                Ty::class_string(Some(cls("Pet"))),
                Ty::class_string(None),
                true,
            ),
            (
                Ty::class_string(None),
                Ty::class_string(Some(cls("Pet"))),
                false,
            ),
            (Ty::class_string(None), Ty::non_empty_string(), true),
            (Ty::int(), Ty::array_key(), true),
            (Ty::float(), Ty::array_key(), false),
            (Ty::bool(), Ty::scalar(), true),
            (Ty::array_key(), Ty::scalar(), true),
            (Ty::plain_array(), Ty::scalar(), false),
            (Ty::resource(), Ty::resource(), true),
            (Ty::resource(), Ty::object(), false),
            (Ty::int(), Ty::string(), false),
            (Ty::lit_int(3), Ty::scalar(), true),
        ],
    );
}

#[test]
fn null_and_nullability() {
    let e = env();
    assert_assign_table(
        &e,
        &[
            (Ty::null(), Ty::int().nullable(), true),
            (Ty::null(), Ty::mixed(), true),
            (Ty::null(), Ty::untyped(), true),
            (Ty::null(), Ty::int(), false),
            (Ty::null(), cls("Dog"), false),
            (Ty::int(), Ty::int().nullable(), true),
            (Ty::lit_int(3), Ty::int().nullable(), true),
            (Ty::int().nullable(), Ty::int(), false),
            (Ty::int().nullable(), Ty::int().nullable(), true),
            (Ty::int().nullable(), Ty::mixed(), true),
            (cls("Dog").nullable(), cls("Pet").nullable(), true),
            (cls("Pet").nullable(), cls("Dog").nullable(), false),
        ],
    );
    let err = check_assignable(&e, &Ty::null(), &Ty::int()).unwrap_err();
    assert_eq!(err.reason, MismatchReason::NullToNonNull);
    let err = check_assignable(&e, &Ty::int().nullable(), &Ty::int()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "null is not assignable to int (the value may be null)"
    );
}

#[test]
fn untyped_values() {
    let e = env();
    assert_assign_table(
        &e,
        &[
            (Ty::untyped(), Ty::mixed(), true),
            (Ty::untyped(), Ty::untyped(), true),
            (Ty::untyped(), Ty::int(), false),
            (Ty::untyped(), Ty::int().nullable(), false),
            (Ty::untyped(), Ty::string(), false),
            (Ty::untyped(), Ty::list(Ty::int()), false),
            (Ty::untyped(), cls("Dog"), false),
            (Ty::int(), Ty::untyped(), true),
            (Ty::mixed(), Ty::untyped(), true),
            (Ty::untyped(), Ty::error(), true),
        ],
    );
}

#[test]
fn mixed() {
    let e = env();
    assert_assign_table(
        &e,
        &[
            (Ty::int(), Ty::mixed(), true),
            (cls("Dog"), Ty::mixed(), true),
            (fun(vec![], Ty::void()), Ty::mixed(), true),
            (Ty::mixed(), Ty::mixed(), true),
            (Ty::mixed(), Ty::int(), false),
            (Ty::mixed(), Ty::int().nullable(), false),
            (Ty::mixed(), cls("Dog"), false),
            (Ty::mixed(), tp(), false),
        ],
    );
}

#[test]
fn unions_and_intersections() {
    let e = env();
    let int_or_str = union(vec![Ty::int(), Ty::string()]);
    let dog_named = Ty::intersection([cls("Pet"), cls("Named")]);
    assert_assign_table(
        &e,
        &[
            (Ty::int(), int_or_str.clone(), true),
            (Ty::string(), int_or_str.clone(), true),
            (Ty::bool(), int_or_str.clone(), false),
            (int_or_str.clone(), Ty::int(), false),
            (
                int_or_str.clone(),
                union(vec![Ty::string(), Ty::bool(), Ty::int()]),
                true,
            ),
            (int_or_str.clone(), Ty::int().nullable(), false),
            (int_or_str.clone(), Ty::array_key(), true),
            (
                Ty::bool(),
                union(vec![Ty::lit_bool(true), Ty::lit_bool(false)]),
                true,
            ),
            (cls("Dog"), dog_named.clone(), true),
            (cls("Cat"), dog_named.clone(), false),
            (dog_named.clone(), cls("Named"), true),
            (dog_named.clone(), cls("Animal"), true),
            (dog_named.clone(), cls("Dog"), false),
        ],
    );
}

#[test]
fn classes_nominal_and_variance() {
    let e = env();
    let bx = |t: Ty| cls_args("Box", vec![t]);
    let sink = |t: Ty| cls_args("Sink", vec![t]);
    let cell = |t: Ty| cls_args("Cell", vec![t]);
    assert_assign_table(
        &e,
        &[
            (cls("Dog"), cls("Pet"), true),
            (cls("Dog"), cls("Animal"), true),
            (cls("Dog"), cls("Named"), true),
            (cls("Cat"), cls("Named"), false),
            (cls("Pet"), cls("Dog"), false),
            (cls("Robot"), cls("Animal"), false),
            (cls("Dog"), Ty::object(), true),
            (Ty::object(), cls("Dog"), false),
            (bx(cls("Dog")), bx(cls("Pet")), true),
            (bx(cls("Pet")), bx(cls("Dog")), false),
            (sink(cls("Pet")), sink(cls("Dog")), true),
            (sink(cls("Dog")), sink(cls("Pet")), false),
            (cell(cls("Dog")), cell(cls("Dog")), true),
            (cell(cls("Dog")), cell(cls("Pet")), false),
            (cell(cls("Pet")), cell(cls("Dog")), false),
            (cls("IntBox"), bx(Ty::int()), true),
            (cls("IntBox"), bx(Ty::int().nullable()), true),
            (cls("IntBox"), bx(Ty::string()), false),
            // raw usage
            (bx(Ty::int()), cls("Box"), true),
            // leading backslash is normalised away
            (Ty::class("\\Dog", vec![]), cls("Pet"), true),
        ],
    );
    let err = check_assignable(&e, &cell(cls("Dog")), &cell(cls("Pet"))).unwrap_err();
    assert!(matches!(
        err.reason,
        MismatchReason::TypeArgument {
            index: 0,
            variance: Variance::Invariant,
            ..
        }
    ));
    assert_eq!(
        err.to_string(),
        "type argument #1 of Cell<Pet> (invariant): Pet is not assignable to Dog"
    );
}

#[test]
fn lists_are_covariant() {
    let e = env();
    let ni = Ty::int().nullable();
    assert_assign_table(
        &e,
        &[
            (Ty::list(Ty::int()), Ty::list(ni.clone()), true),
            (Ty::list(cls("Dog")), Ty::list(cls("Pet")), true),
            (Ty::list(ni.clone()), Ty::list(Ty::int()), false),
            (
                Ty::array(Ty::string(), Ty::int()),
                Ty::array(Ty::string(), ni.clone()),
                true,
            ),
            (Ty::list(Ty::int()), Ty::plain_iterable(), true),
            (
                Ty::list(Ty::int()),
                Ty::iterable(Ty::int(), Ty::int()),
                true,
            ),
            (Ty::list(Ty::int()), Ty::list(Ty::string()), false),
            (Ty::list(Ty::int()), Ty::non_empty_list(Ty::int()), false),
            (
                Ty::array(Ty::string(), Ty::int()),
                Ty::list(Ty::int()),
                false,
            ),
            (
                Ty::array(Ty::string(), Ty::int()),
                Ty::array(Ty::int(), Ty::int()),
                false,
            ),
        ],
    );
}

#[test]
fn php_arrays_and_shapes() {
    let e = env();
    let a_int = shape(&[("a", Ty::int())], true);
    let opt_b = Ty::shape(
        vec![
            ShapeField::new("a", Ty::int()),
            ShapeField::optional("b", Ty::string()),
        ],
        true,
    );
    let list_shape = Ty::shape(
        vec![ShapeField::new(0, Ty::int()), ShapeField::new(1, Ty::int())],
        true,
    );
    assert_assign_table(
        &e,
        &[
            (Ty::list(Ty::int()), Ty::plain_array(), true),
            (Ty::list(Ty::int()), Ty::array(Ty::int(), Ty::int()), true),
            (
                Ty::list(Ty::int()),
                Ty::array(Ty::string(), Ty::int()),
                false,
            ),
            (Ty::array(Ty::int(), Ty::int()), Ty::list(Ty::int()), false),
            (Ty::non_empty_list(Ty::int()), Ty::list(Ty::int()), true),
            (Ty::list(Ty::int()), Ty::non_empty_list(Ty::int()), false),
            (Ty::list(Ty::positive_int()), Ty::list(Ty::int()), true),
            (
                Ty::non_empty_array(Ty::int(), Ty::int()),
                Ty::plain_array(),
                true,
            ),
            (
                Ty::plain_array(),
                Ty::non_empty_array(Ty::array_key(), Ty::mixed()),
                false,
            ),
            (Ty::array(Ty::string(), Ty::int()), Ty::plain_array(), true),
            (Ty::plain_array(), Ty::array(Ty::string(), Ty::int()), false),
            // shapes
            (a_int.clone(), opt_b.clone(), true),
            (opt_b.clone(), a_int.clone(), false), // extra key `b` in a sealed target
            (opt_b.clone(), shape(&[("a", Ty::int())], false), true),
            (
                a_int.clone(),
                shape(&[("a", Ty::int()), ("b", Ty::int())], true),
                false,
            ),
            (shape(&[("a", Ty::lit_int(1))], true), a_int.clone(), true),
            (shape(&[("a", Ty::string())], true), a_int.clone(), false),
            (shape(&[("a", Ty::int())], false), a_int.clone(), false),
            (
                Ty::shape(vec![ShapeField::optional("a", Ty::int())], true),
                a_int.clone(),
                false,
            ),
            (a_int.clone(), Ty::array(Ty::string(), Ty::int()), true),
            (
                a_int.clone(),
                Ty::non_empty_array(Ty::string(), Ty::int()),
                true,
            ),
            (a_int.clone(), Ty::array(Ty::int(), Ty::int()), false),
            (a_int.clone(), Ty::plain_iterable(), true),
            (list_shape.clone(), Ty::list(Ty::int()), true),
            (list_shape.clone(), Ty::non_empty_list(Ty::int()), true),
            (a_int.clone(), Ty::list(Ty::int()), false),
            (
                shape(&[], true),
                Ty::non_empty_array(Ty::array_key(), Ty::mixed()),
                false,
            ),
            // numeric-string keys are integer keys
            (
                shape(&[("0", Ty::int()), ("1", Ty::int())], true),
                list_shape.clone(),
                true,
            ),
        ],
    );
    let err = check_assignable(&e, &opt_b, &a_int).unwrap_err();
    assert_eq!(
        err.reason,
        MismatchReason::ShapeExtraKey {
            key: ShapeKey::Str("b".into())
        }
    );
}

#[test]
fn iterables_and_traversable() {
    let e = env();
    let coll = cls_args("Coll", vec![cls("Dog")]);
    assert_assign_table(
        &e,
        &[
            (coll.clone(), Ty::iterable(Ty::int(), cls("Pet")), true),
            (coll.clone(), Ty::plain_iterable(), true),
            (coll.clone(), Ty::iterable(Ty::string(), cls("Pet")), false),
            (
                coll.clone(),
                cls_args("Traversable", vec![Ty::int(), cls("Animal")]),
                true,
            ),
            (cls("Dog"), Ty::plain_iterable(), false),
            (Ty::plain_array(), Ty::plain_iterable(), true),
            (Ty::iterable(Ty::int(), Ty::int()), Ty::plain_array(), false),
        ],
    );
}

#[test]
fn type_params() {
    let e = env();
    assert_assign_table(
        &e,
        &[
            (tp(), tp(), true),
            (tp(), up(), false),
            (tp(), Ty::mixed(), true),
            (tp(), Ty::untyped(), true),
            (tp(), tp().nullable(), true),
            (tp(), Ty::int(), false),
            (Ty::int(), tp(), false),
            (up(), cls("Pet"), true),
            (up(), cls("Animal"), true),
            (up(), cls("Animal").nullable(), true),
            (up(), cls("Dog"), false),
            (cls("Pet"), up(), false),
            (np(), Ty::int().nullable(), true),
            (np(), Ty::int(), false),
            (Ty::list(up()), Ty::list(cls("Animal")), true),
            (Ty::never(), tp(), true),
        ],
    );
}

#[test]
fn static_and_this() {
    let e = env();
    let pet = cls("Pet");
    assert_assign_table(
        &e,
        &[
            (Ty::this_of(pet.clone()), Ty::static_of(pet.clone()), true),
            (Ty::static_of(pet.clone()), Ty::this_of(pet.clone()), false),
            (Ty::static_of(pet.clone()), pet.clone(), true),
            (Ty::static_of(pet.clone()), cls("Animal"), true),
            (Ty::static_of(pet.clone()), Ty::object(), true),
            (pet.clone(), Ty::static_of(pet.clone()), false),
            (cls("Dog"), Ty::static_of(cls("Dog")), true), // final
            (Ty::static_of(cls("Dog")), Ty::static_of(pet.clone()), true),
        ],
    );
}

#[test]
fn function_types() {
    let e = env();
    let f = func;
    let req = FnParam::required;
    let opt = FnParam::optional;
    let var = FnParam::variadic;
    assert_assign_table(
        &e,
        &[
            (
                fun(vec![cls("Pet")], cls("Dog")),
                fun(vec![cls("Dog")], cls("Pet")),
                true,
            ),
            (
                fun(vec![cls("Dog")], cls("Dog")),
                fun(vec![cls("Pet")], cls("Dog")),
                false,
            ),
            (fun(vec![], cls("Pet")), fun(vec![], cls("Dog")), false),
            (
                fun(vec![Ty::int().nullable()], Ty::int()),
                fun(vec![Ty::int()], Ty::int()),
                true,
            ),
            (
                fun(vec![Ty::int()], Ty::int()),
                fun(vec![Ty::int().nullable()], Ty::int()),
                false,
            ),
            (
                fun(vec![Ty::int()], Ty::int()),
                fun(vec![Ty::int(), Ty::int()], Ty::int()),
                false,
            ),
            (
                fun(vec![Ty::int(), Ty::int()], Ty::int()),
                fun(vec![Ty::int()], Ty::int()),
                false,
            ),
            (fun(vec![], Ty::never()), fun(vec![], Ty::void()), true),
            (fun(vec![], Ty::int()), fun(vec![], Ty::void()), false),
            // optional / variadic
            (
                f(vec![req(Ty::int()), opt(Ty::int())], Ty::void()),
                fun(vec![Ty::int()], Ty::void()),
                true,
            ),
            (
                f(vec![req(Ty::int()), opt(Ty::int())], Ty::void()),
                fun(vec![Ty::int(), Ty::int()], Ty::void()),
                true,
            ),
            (
                fun(vec![Ty::int()], Ty::void()),
                f(vec![opt(Ty::int())], Ty::void()),
                false,
            ),
            (
                f(vec![opt(Ty::int())], Ty::void()),
                f(vec![opt(Ty::int())], Ty::void()),
                true,
            ),
            (
                f(vec![var(Ty::int())], Ty::void()),
                fun(vec![Ty::int(), Ty::int(), Ty::int()], Ty::void()),
                true,
            ),
            (
                f(vec![var(Ty::int())], Ty::void()),
                fun(vec![], Ty::void()),
                true,
            ),
            (
                f(vec![var(Ty::int())], Ty::void()),
                f(vec![var(Ty::int())], Ty::void()),
                true,
            ),
            (
                fun(vec![Ty::int()], Ty::void()),
                f(vec![var(Ty::int())], Ty::void()),
                false,
            ),
            (
                f(vec![var(Ty::int())], Ty::void()),
                fun(vec![Ty::string()], Ty::void()),
                false,
            ),
            // to callables
            (fun(vec![Ty::int()], Ty::string()), Ty::callable(None), true),
            (fun(vec![Ty::int()], Ty::string()), cls("Closure"), true),
            (fun(vec![Ty::int()], Ty::string()), Ty::object(), true),
            (
                fun(vec![Ty::int()], Ty::string()),
                Ty::callable(Some(FnTy::new(vec![req(Ty::int())], Ty::string()))),
                true,
            ),
            (Ty::callable(None), fun(vec![], Ty::void()), false),
            // a plain `Closure` says nothing about its signature
            (cls("Closure"), fun(vec![], Ty::void()), true),
            // callables among themselves
            (cls("Closure"), Ty::callable(None), true),
            (Ty::callable(None), cls("Closure"), false),
            (
                Ty::closure(Some(FnTy::new(vec![], Ty::int()))),
                Ty::callable(Some(FnTy::new(vec![], Ty::float()))),
                true,
            ),
            (
                Ty::callable(Some(FnTy::new(vec![], Ty::int()))),
                Ty::closure(Some(FnTy::new(vec![], Ty::int()))),
                false,
            ),
            (cls("Doubler"), Ty::callable(None), true),
            (
                cls("Doubler"),
                Ty::callable(Some(FnTy::new(vec![req(Ty::positive_int())], Ty::float()))),
                true,
            ),
            (
                cls("Doubler"),
                Ty::callable(Some(FnTy::new(vec![req(Ty::string())], Ty::int()))),
                false,
            ),
            (cls("Doubler"), cls("Closure"), false),
            (cls("Dog"), Ty::callable(None), false),
        ],
    );
    let err = check_assignable(
        &e,
        &fun(vec![Ty::int()], Ty::int()),
        &fun(vec![Ty::string()], Ty::int()),
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "parameter #1 of function type: string is not assignable to int"
    );
}

#[test]
fn equivalence_and_empty_env() {
    let e = env();
    assert!(is_equivalent(&e, &Ty::int(), &Ty::int()));
    assert!(!is_equivalent(&e, &Ty::int(), &Ty::int().nullable()));
    assert!(is_equivalent(
        &e,
        &union(vec![Ty::int(), Ty::string()]),
        &union(vec![Ty::string(), Ty::int()])
    ));
    assert!(is_assignable(&EmptyEnv, &Ty::lit_int(1), &Ty::int()));
    assert!(!is_assignable(&EmptyEnv, &cls("A"), &cls("B")));
}

#[test]
fn mismatch_messages() {
    let e = env();
    let msg = |a: &Ty, b: &Ty| check_assignable(&e, a, b).unwrap_err().to_string();
    assert_eq!(
        msg(&Ty::list(Ty::int().nullable()), &Ty::list(Ty::int())),
        "type argument #1 of list<int>: null is not assignable to int (the value may be null)"
    );
    assert_eq!(
        msg(&union(vec![Ty::int(), Ty::string()]), &Ty::int()),
        "string is not assignable to int"
    );
    assert_eq!(
        msg(&Ty::string(), &Ty::int().nullable()),
        "string is not assignable to ?int"
    );
    assert_eq!(
        msg(
            &shape(&[("x", Ty::int())], true),
            &shape(&[("x", Ty::int()), ("y", Ty::int())], true)
        ),
        "array{x: int} is not assignable to array{x: int, y: int}: missing key `y`"
    );
    assert_eq!(
        msg(&Ty::untyped(), &Ty::int()),
        "mixed is not assignable to int"
    );
    assert_eq!(
        msg(&fun(vec![], Ty::int()), &fun(vec![], Ty::string())),
        "return type of function type: int is not assignable to string"
    );
    let err = check_assignable(
        &e,
        &Ty::list(Ty::list(Ty::string())),
        &Ty::list(Ty::list(Ty::int())),
    )
    .unwrap_err();
    assert_eq!(err.root().to_string(), "string is not assignable to int");
}

#[test]
#[should_panic(expected = "assignability failures")]
fn table_harness_reports_failures() {
    assert_assign_table(&env(), &[(Ty::float(), Ty::int(), true)]);
}
