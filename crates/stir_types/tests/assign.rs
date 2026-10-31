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
            (Ty::nothing(), Ty::int(), true),
            (Ty::nothing(), cls("Dog"), true),
            (Ty::never(), Ty::php_string(), true),
            (Ty::never(), Ty::list(Ty::int()), true),
            (Ty::error(), Ty::int(), true),
            (Ty::int(), Ty::error(), true),
            (Ty::error(), Ty::nothing(), true),
            (Ty::int(), Ty::int(), true),
            (Ty::int(), Ty::nothing(), false),
            (Ty::php_int(), Ty::never(), false),
            (Ty::unit(), Ty::unit(), true),
            (Ty::unit(), Ty::int(), false),
        ],
    );
}

#[test]
fn safe_to_unsafe() {
    let e = env();
    assert_assign_table(
        &e,
        &[
            (Ty::int(), Ty::php_int(), true),
            (Ty::float(), Ty::php_float(), true),
            (Ty::string(), Ty::php_string(), true),
            (Ty::bool(), Ty::php_bool(), true),
            (Ty::unit(), Ty::void(), true),
            (Ty::nothing(), Ty::never(), true),
            (Ty::int(), Ty::scalar(), true),
            (Ty::int(), Ty::numeric(), true),
            (Ty::string(), Ty::array_key(), true),
            (Ty::int(), Ty::mixed(), true),
            (Ty::list(Ty::int()), Ty::mixed(), true),
            // PHP semantics after crossing over: Int is an int, which widens to float.
            (Ty::int(), Ty::php_float(), true),
            (Ty::string(), Ty::non_empty_string(), false),
            (Ty::int(), Ty::positive_int(), false),
            (Ty::int(), Ty::php_string(), false),
            (Ty::bool(), Ty::php_int(), false),
        ],
    );
}

#[test]
fn unsafe_to_safe_is_rejected() {
    let e = env();
    assert_assign_table(
        &e,
        &[
            (Ty::php_int(), Ty::int(), false),
            (Ty::php_float(), Ty::float(), false),
            (Ty::php_string(), Ty::string(), false),
            (Ty::php_bool(), Ty::bool(), false),
            (Ty::void(), Ty::unit(), false),
            (Ty::mixed(), Ty::int(), false),
            (Ty::mixed(), Ty::int().nullable(), false),
            (Ty::php_list(Ty::int()), Ty::list(Ty::int()), false),
            (
                Ty::array(Ty::string(), Ty::int()),
                Ty::map(Ty::string(), Ty::int()),
                false,
            ),
            (pi(5), Ty::int(), false),
            (ps("a"), Ty::string(), false),
            (Ty::positive_int(), Ty::int(), false),
            (Ty::non_empty_string(), Ty::string(), false),
        ],
    );
    let err = check_assignable(&e, &Ty::php_int(), &Ty::int()).unwrap_err();
    assert_eq!(err.reason, MismatchReason::UnsafeToSafe);
}

#[test]
fn numeric_widening() {
    let e = env();
    assert_assign_table(
        &e,
        &[
            // Kotlin: no implicit widening.
            (Ty::int(), Ty::float(), false),
            (Ty::lit_int(1), Ty::float(), false),
            (Ty::float(), Ty::int(), false),
            // PHP: int -> float is allowed even under strict_types.
            (Ty::php_int(), Ty::php_float(), true),
            (pi(1), Ty::php_float(), true),
            (Ty::positive_int(), Ty::php_float(), true),
            (Ty::php_float(), Ty::php_int(), false),
        ],
    );
    let err = check_assignable(&e, &Ty::int(), &Ty::float()).unwrap_err();
    assert_eq!(err.reason, MismatchReason::NoNumericWidening);
}

#[test]
fn literals() {
    let e = env();
    let status = union(vec![s("active"), s("banned")]);
    assert_assign_table(
        &e,
        &[
            (Ty::lit_int(5), Ty::int(), true),
            (Ty::lit_int(5), Ty::php_int(), true),
            (Ty::lit_int(5), Ty::positive_int(), true),
            (Ty::lit_int(0), Ty::positive_int(), false),
            (Ty::lit_int(5), Ty::int_range(Some(0), Some(10)), true),
            (Ty::lit_int(11), Ty::int_range(Some(0), Some(10)), false),
            (Ty::lit_int(5), Ty::php_float(), true),
            (Ty::lit_int(5), pi(5), true),
            (Ty::lit_int(5), Ty::lit_int(6), false),
            (Ty::lit_float(1.5), Ty::float(), true),
            (Ty::lit_bool(true), Ty::bool(), true),
            (Ty::lit_bool(true), Ty::php_bool(), true),
            (s("a"), Ty::string(), true),
            (s("a"), Ty::php_string(), true),
            (s("a"), Ty::non_empty_string(), true),
            (s(""), Ty::non_empty_string(), false),
            (s("12"), Ty::numeric_string(), true),
            (s(" 1.5e3 "), Ty::numeric(), true),
            (s("abc"), Ty::numeric_string(), false),
            (s("a"), Ty::int(), false),
            (Ty::string(), s("a"), false),
            (Ty::int(), Ty::lit_int(5), false),
            (pi(5), Ty::lit_int(5), false),
            // literal unions
            (s("active"), status.clone(), true),
            (s("deleted"), status.clone(), false),
            (status.clone(), Ty::string(), true),
            (status.clone(), Ty::php_string(), true),
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
            (Ty::positive_int(), Ty::php_int(), true),
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
            (Ty::php_int(), Ty::positive_int(), false),
            (Ty::non_empty_string(), Ty::php_string(), true),
            (Ty::numeric_string(), Ty::non_empty_string(), true),
            (Ty::numeric_string(), Ty::numeric(), true),
            (Ty::php_string(), Ty::numeric(), false),
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
            (Ty::php_int(), Ty::array_key(), true),
            (Ty::php_float(), Ty::array_key(), false),
            (Ty::php_bool(), Ty::scalar(), true),
            (Ty::array_key(), Ty::scalar(), true),
            (Ty::php_array(), Ty::scalar(), false),
            (Ty::resource(), Ty::resource(), true),
            (Ty::resource(), Ty::object(), false),
            (Ty::php_int(), Ty::php_string(), false),
            (ps("x"), Ty::php_string(), true),
            (pi(3), Ty::scalar(), true),
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
            (Ty::null(), Ty::php_int().nullable(), true),
            (Ty::null(), Ty::mixed(), true),
            (Ty::null(), Ty::platform(Ty::string()), true),
            (Ty::null(), Ty::any_platform(), true),
            (Ty::null(), Ty::int(), false),
            (Ty::null(), Ty::php_int(), false),
            (Ty::null(), cls("Dog"), false),
            (Ty::int(), Ty::int().nullable(), true),
            (Ty::lit_int(3), Ty::int().nullable(), true),
            (Ty::int().nullable(), Ty::int(), false),
            (Ty::int().nullable(), Ty::php_int().nullable(), true),
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
        "null is not assignable to Int (the value may be null)"
    );
}

#[test]
fn platform_types() {
    let e = env();
    let sp = Ty::platform(Ty::string());
    assert_assign_table(
        &e,
        &[
            (sp.clone(), Ty::string(), true),
            (sp.clone(), Ty::string().nullable(), true),
            (sp.clone(), Ty::php_string(), true),
            (Ty::string(), sp.clone(), true),
            (Ty::string().nullable(), sp.clone(), true),
            (Ty::null(), sp.clone(), true),
            (sp.clone(), Ty::int(), false),
            (Ty::int(), sp.clone(), false),
            // a platform of a nullable type is still usable as non-null
            (Ty::platform(Ty::string().nullable()), Ty::string(), true),
            (Ty::platform(cls("Dog")), cls("Pet"), true),
            (Ty::platform(cls("Pet")), cls("Dog"), false),
            (sp.clone(), Ty::platform(Ty::string().nullable()), true),
            // Any!
            (Ty::any_platform(), Ty::mixed(), true),
            (Ty::any_platform(), Ty::any_platform(), true),
            (Ty::any_platform(), Ty::int(), false),
            (Ty::any_platform(), Ty::int().nullable(), false),
            (Ty::any_platform(), Ty::string(), false),
            (Ty::any_platform(), Ty::php_int(), false),
            (Ty::any_platform(), Ty::list(Ty::int()), false),
            (Ty::any_platform(), cls("Dog"), false),
            (Ty::int(), Ty::any_platform(), true),
            (Ty::mixed(), Ty::any_platform(), true),
            (Ty::any_platform(), Ty::error(), true),
        ],
    );
    let err = check_assignable(&e, &Ty::any_platform(), &Ty::int()).unwrap_err();
    assert_eq!(err.reason, MismatchReason::AnyPlatformToSafe);
}

#[test]
fn mixed() {
    let e = env();
    assert_assign_table(
        &e,
        &[
            (Ty::php_int(), Ty::mixed(), true),
            (cls("Dog"), Ty::mixed(), true),
            (rec(&[("x", Ty::int())]), Ty::mixed(), true),
            (fun(vec![], Ty::unit()), Ty::mixed(), true),
            (Ty::mixed(), Ty::mixed(), true),
            (Ty::mixed(), Ty::php_int(), false),
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
            (int_or_str.clone(), Ty::php_int().nullable(), false),
            (
                int_or_str.clone(),
                union(vec![Ty::php_int(), Ty::php_string()]),
                true,
            ),
            (int_or_str.clone(), Ty::array_key(), true),
            (
                union(vec![Ty::php_int(), Ty::php_string()]),
                int_or_str.clone(),
                false,
            ),
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
            // Platform of a union against that union.
            (Ty::platform(int_or_str.clone()), int_or_str.clone(), true),
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
fn kotlin_collections() {
    let e = env();
    let ni = Ty::int().nullable();
    assert_assign_table(
        &e,
        &[
            (Ty::list(Ty::int()), Ty::list(ni.clone()), true),
            (Ty::list(cls("Dog")), Ty::list(cls("Pet")), true),
            (Ty::list(ni.clone()), Ty::list(Ty::int()), false),
            (
                Ty::mutable_list(Ty::int()),
                Ty::mutable_list(ni.clone()),
                false,
            ),
            (
                Ty::mutable_list(Ty::int()),
                Ty::mutable_list(Ty::int()),
                true,
            ),
            (Ty::mutable_list(Ty::int()), Ty::list(Ty::int()), true),
            (Ty::mutable_list(Ty::int()), Ty::list(ni.clone()), true),
            (Ty::list(Ty::int()), Ty::mutable_list(Ty::int()), false),
            (
                Ty::map(Ty::string(), Ty::int()),
                Ty::map(Ty::string(), ni.clone()),
                true,
            ),
            (
                Ty::map(Ty::string(), Ty::int()),
                Ty::map(s("a"), Ty::int()),
                false,
            ),
            (
                Ty::map(s("a"), Ty::int()),
                Ty::map(Ty::string(), Ty::int()),
                false,
            ),
            (
                Ty::mutable_map(Ty::string(), Ty::int()),
                Ty::map(Ty::string(), ni.clone()),
                true,
            ),
            (
                Ty::mutable_map(Ty::string(), Ty::int()),
                Ty::mutable_map(Ty::string(), ni.clone()),
                false,
            ),
            (
                Ty::map(Ty::string(), Ty::int()),
                Ty::mutable_map(Ty::string(), Ty::int()),
                false,
            ),
            (Ty::list(Ty::int()), Ty::map(Ty::int(), Ty::int()), false),
            // to PHP
            (Ty::list(Ty::int()), Ty::php_list(Ty::php_int()), true),
            (Ty::list(Ty::int()), Ty::php_list(Ty::int()), true),
            (
                Ty::list(Ty::int()),
                Ty::array(Ty::php_int(), Ty::php_int()),
                true,
            ),
            (Ty::list(Ty::int()), Ty::php_array(), true),
            (Ty::list(Ty::int()), Ty::php_iterable(), true),
            (
                Ty::list(Ty::int()),
                Ty::iterable(Ty::php_int(), Ty::php_int()),
                true,
            ),
            (Ty::list(Ty::int()), Ty::php_list(Ty::php_string()), false),
            (
                Ty::list(Ty::int()),
                Ty::non_empty_list(Ty::php_int()),
                false,
            ),
            (
                Ty::mutable_list(Ty::string()),
                Ty::php_list(Ty::php_string()),
                true,
            ),
            (
                Ty::map(Ty::string(), Ty::int()),
                Ty::array(Ty::php_string(), Ty::php_int()),
                true,
            ),
            (Ty::map(Ty::string(), Ty::int()), Ty::php_array(), true),
            (
                Ty::map(Ty::string(), Ty::int()),
                Ty::php_list(Ty::php_int()),
                false,
            ),
            (
                Ty::map(Ty::string(), Ty::int()),
                Ty::array(Ty::php_int(), Ty::php_int()),
                false,
            ),
        ],
    );
}

#[test]
fn records() {
    let e = env();
    let p2 = rec(&[("x", Ty::int()), ("y", Ty::int())]);
    let p3 = rec(&[("x", Ty::int()), ("y", Ty::int()), ("z", Ty::int())]);
    let named = Ty::named_record("Point", vec![], [("x", Ty::int()), ("y", Ty::int())]);
    assert_assign_table(
        &e,
        &[
            (p2.clone(), p2.clone(), true),
            (p3.clone(), p2.clone(), true),
            (p2.clone(), p3.clone(), false),
            (named.clone(), p2.clone(), true),
            (p2.clone(), named.clone(), true),
            (
                rec(&[("y", Ty::int()), ("x", Ty::int())]),
                p2.clone(),
                false,
            ),
            (
                rec(&[("x", Ty::int()), ("y", Ty::string())]),
                p2.clone(),
                false,
            ),
            (
                rec(&[("x", Ty::int()), ("y", Ty::lit_int(3))]),
                p2.clone(),
                true,
            ),
            (
                rec(&[("x", Ty::int())]),
                rec(&[("x", Ty::int().nullable())]),
                true,
            ),
            (rec(&[]), rec(&[]), true),
            (p2.clone(), rec(&[]), true),
            // to PHP
            (
                p2.clone(),
                shape(&[("x", Ty::php_int()), ("y", Ty::php_int())], true),
                true,
            ),
            (p2.clone(), shape(&[("x", Ty::php_int())], false), true),
            (p2.clone(), shape(&[("x", Ty::php_int())], true), false),
            (p2.clone(), Ty::php_array(), true),
            (p2.clone(), Ty::array(Ty::php_string(), Ty::php_int()), true),
            (p2.clone(), Ty::php_list(Ty::php_int()), false),
            // from PHP: only a sealed shape with exactly the same layout
            (
                shape(&[("x", Ty::int()), ("y", Ty::int())], true),
                p2.clone(),
                true,
            ),
            (
                shape(&[("y", Ty::int()), ("x", Ty::int())], true),
                p2.clone(),
                false,
            ),
            (
                shape(&[("x", Ty::int()), ("y", Ty::int())], false),
                p2.clone(),
                false,
            ),
            (shape(&[("x", Ty::int())], true), p2.clone(), false),
            (
                shape(
                    &[("x", Ty::int()), ("y", Ty::int()), ("z", Ty::int())],
                    true,
                ),
                p2.clone(),
                false,
            ),
            (
                shape(&[("x", Ty::php_int()), ("y", Ty::php_int())], true),
                p2.clone(),
                false,
            ),
            (Ty::php_array(), p2.clone(), false),
        ],
    );

    let err = check_assignable(
        &e,
        &rec(&[("x", Ty::int()), ("y", Ty::int())]),
        &rec(&[("x", Ty::int()), ("y", Ty::string())]),
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "field `y` of record: Int is not assignable to String"
    );

    let err = check_assignable(&e, &rec(&[("y", Ty::int()), ("x", Ty::int())]), &p2).unwrap_err();
    assert!(matches!(
        err.reason,
        MismatchReason::RecordFieldOrder { index: 0, .. }
    ));

    let err = check_assignable(&e, &p2, &p3).unwrap_err();
    assert_eq!(
        err.reason,
        MismatchReason::RecordMissingField { name: "z".into() }
    );
}

#[test]
fn php_arrays_and_shapes() {
    let e = env();
    let a_int = shape(&[("a", Ty::php_int())], true);
    let opt_b = Ty::shape(
        vec![
            ShapeField::new("a", Ty::php_int()),
            ShapeField::optional("b", Ty::php_string()),
        ],
        true,
    );
    let list_shape = Ty::shape(
        vec![
            ShapeField::new(0, Ty::php_int()),
            ShapeField::new(1, Ty::php_int()),
        ],
        true,
    );
    assert_assign_table(
        &e,
        &[
            (Ty::php_list(Ty::php_int()), Ty::php_array(), true),
            (
                Ty::php_list(Ty::php_int()),
                Ty::array(Ty::php_int(), Ty::php_int()),
                true,
            ),
            (
                Ty::php_list(Ty::php_int()),
                Ty::array(Ty::php_string(), Ty::php_int()),
                false,
            ),
            (
                Ty::array(Ty::php_int(), Ty::php_int()),
                Ty::php_list(Ty::php_int()),
                false,
            ),
            (
                Ty::non_empty_list(Ty::php_int()),
                Ty::php_list(Ty::php_int()),
                true,
            ),
            (
                Ty::php_list(Ty::php_int()),
                Ty::non_empty_list(Ty::php_int()),
                false,
            ),
            (
                Ty::php_list(Ty::positive_int()),
                Ty::php_list(Ty::php_int()),
                true,
            ),
            (
                Ty::non_empty_array(Ty::php_int(), Ty::php_int()),
                Ty::php_array(),
                true,
            ),
            (
                Ty::php_array(),
                Ty::non_empty_array(Ty::array_key(), Ty::mixed()),
                false,
            ),
            (
                Ty::array(Ty::php_string(), Ty::php_int()),
                Ty::php_array(),
                true,
            ),
            (
                Ty::php_array(),
                Ty::array(Ty::php_string(), Ty::php_int()),
                false,
            ),
            // shapes
            (a_int.clone(), opt_b.clone(), true),
            (opt_b.clone(), a_int.clone(), false), // extra key `b` in a sealed target
            (opt_b.clone(), shape(&[("a", Ty::php_int())], false), true),
            (
                a_int.clone(),
                shape(&[("a", Ty::php_int()), ("b", Ty::php_int())], true),
                false,
            ),
            (shape(&[("a", pi(1))], true), a_int.clone(), true),
            (
                shape(&[("a", Ty::php_string())], true),
                a_int.clone(),
                false,
            ),
            (shape(&[("a", Ty::php_int())], false), a_int.clone(), false),
            (
                Ty::shape(vec![ShapeField::optional("a", Ty::php_int())], true),
                a_int.clone(),
                false,
            ),
            (
                a_int.clone(),
                Ty::array(Ty::php_string(), Ty::php_int()),
                true,
            ),
            (
                a_int.clone(),
                Ty::non_empty_array(Ty::php_string(), Ty::php_int()),
                true,
            ),
            (
                a_int.clone(),
                Ty::array(Ty::php_int(), Ty::php_int()),
                false,
            ),
            (a_int.clone(), Ty::php_iterable(), true),
            (list_shape.clone(), Ty::php_list(Ty::php_int()), true),
            (list_shape.clone(), Ty::non_empty_list(Ty::php_int()), true),
            (a_int.clone(), Ty::php_list(Ty::php_int()), false),
            (
                shape(&[], true),
                Ty::non_empty_array(Ty::array_key(), Ty::mixed()),
                false,
            ),
            // numeric-string keys are integer keys
            (
                shape(&[("0", Ty::php_int()), ("1", Ty::php_int())], true),
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
            (coll.clone(), Ty::iterable(Ty::php_int(), cls("Pet")), true),
            (coll.clone(), Ty::php_iterable(), true),
            (
                coll.clone(),
                Ty::iterable(Ty::php_string(), cls("Pet")),
                false,
            ),
            (
                coll.clone(),
                cls_args("Traversable", vec![Ty::php_int(), cls("Animal")]),
                true,
            ),
            (cls("Dog"), Ty::php_iterable(), false),
            (Ty::php_array(), Ty::php_iterable(), true),
            (
                Ty::iterable(Ty::php_int(), Ty::php_int()),
                Ty::php_array(),
                false,
            ),
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
            (tp(), Ty::any_platform(), true),
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
            (Ty::nothing(), tp(), true),
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
    let f = |params: Vec<FnParam>, ret: Ty| Ty::func(params, ret);
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
            (fun(vec![], Ty::nothing()), fun(vec![], Ty::unit()), true),
            (fun(vec![], Ty::int()), fun(vec![], Ty::unit()), false),
            // optional / variadic
            (
                f(vec![req(Ty::int()), opt(Ty::int())], Ty::unit()),
                fun(vec![Ty::int()], Ty::unit()),
                true,
            ),
            (
                f(vec![req(Ty::int()), opt(Ty::int())], Ty::unit()),
                fun(vec![Ty::int(), Ty::int()], Ty::unit()),
                true,
            ),
            (
                fun(vec![Ty::int()], Ty::unit()),
                f(vec![opt(Ty::int())], Ty::unit()),
                false,
            ),
            (
                f(vec![opt(Ty::int())], Ty::unit()),
                f(vec![opt(Ty::int())], Ty::unit()),
                true,
            ),
            (
                f(vec![var(Ty::int())], Ty::unit()),
                fun(vec![Ty::int(), Ty::int(), Ty::int()], Ty::unit()),
                true,
            ),
            (
                f(vec![var(Ty::int())], Ty::unit()),
                fun(vec![], Ty::unit()),
                true,
            ),
            (
                f(vec![var(Ty::int())], Ty::unit()),
                f(vec![var(Ty::int())], Ty::unit()),
                true,
            ),
            (
                fun(vec![Ty::int()], Ty::unit()),
                f(vec![var(Ty::int())], Ty::unit()),
                false,
            ),
            (
                f(vec![var(Ty::int())], Ty::unit()),
                fun(vec![Ty::string()], Ty::unit()),
                false,
            ),
            // to PHP callables
            (fun(vec![Ty::int()], Ty::string()), Ty::callable(None), true),
            (fun(vec![Ty::int()], Ty::string()), cls("Closure"), true),
            (fun(vec![Ty::int()], Ty::string()), Ty::object(), true),
            (
                fun(vec![Ty::php_int()], Ty::string()),
                Ty::callable(Some(FnTy::new(vec![req(Ty::php_int())], Ty::php_string()))),
                true,
            ),
            (
                fun(vec![Ty::int()], Ty::string()),
                Ty::closure(Some(FnTy::new(vec![req(Ty::php_int())], Ty::php_string()))),
                // `int` is not assignable to the Kotlin parameter type `Int`
                false,
            ),
            (Ty::callable(None), fun(vec![], Ty::unit()), false),
            (cls("Closure"), fun(vec![], Ty::unit()), false),
            // PHP callables among themselves
            (cls("Closure"), Ty::callable(None), true),
            (Ty::callable(None), cls("Closure"), false),
            (
                Ty::closure(Some(FnTy::new(vec![], Ty::php_int()))),
                Ty::callable(Some(FnTy::new(vec![], Ty::php_float()))),
                true,
            ),
            (
                Ty::callable(Some(FnTy::new(vec![], Ty::php_int()))),
                Ty::closure(Some(FnTy::new(vec![], Ty::php_int()))),
                false,
            ),
            (cls("Doubler"), Ty::callable(None), true),
            (
                cls("Doubler"),
                Ty::callable(Some(FnTy::new(
                    vec![req(Ty::positive_int())],
                    Ty::php_float(),
                ))),
                true,
            ),
            (
                cls("Doubler"),
                Ty::callable(Some(FnTy::new(vec![req(Ty::php_string())], Ty::php_int()))),
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
        "parameter #1 of function type: String is not assignable to Int"
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
        msg(&Ty::php_int(), &Ty::int()),
        "int is not assignable to Int (a PHP type cannot be used as a Kotlin type; convert it explicitly)"
    );
    assert_eq!(
        msg(&Ty::list(Ty::int().nullable()), &Ty::list(Ty::int())),
        "type argument #1 of List<Int>: null is not assignable to Int (the value may be null)"
    );
    assert_eq!(
        msg(
            &Ty::mutable_list(Ty::int()),
            &Ty::mutable_list(Ty::int().nullable())
        ),
        "type argument #1 of MutableList<Int?> (invariant): null is not assignable to Int (the value may be null)"
    );
    assert_eq!(
        msg(&union(vec![Ty::int(), Ty::string()]), &Ty::int()),
        "String is not assignable to Int"
    );
    assert_eq!(
        msg(&Ty::string(), &Ty::int().nullable()),
        "String is not assignable to Int?"
    );
    assert_eq!(
        msg(
            &rec(&[("x", Ty::int())]),
            &rec(&[("x", Ty::int()), ("y", Ty::int())])
        ),
        "{ x: Int } is not assignable to { x: Int, y: Int }: missing field `y`"
    );
    assert_eq!(
        msg(&Ty::any_platform(), &Ty::int()),
        "Any! is not assignable to Int (an untyped PHP value must be narrowed with `is` first)"
    );
    assert_eq!(
        msg(&fun(vec![], Ty::int()), &fun(vec![], Ty::string())),
        "return type of function type: Int is not assignable to String"
    );
    let err = check_assignable(
        &e,
        &Ty::list(Ty::list(Ty::string())),
        &Ty::list(Ty::list(Ty::int())),
    )
    .unwrap_err();
    assert_eq!(err.root().to_string(), "String is not assignable to Int");
}

#[test]
#[should_panic(expected = "assignability failures")]
fn table_harness_reports_failures() {
    assert_assign_table(&env(), &[(Ty::int(), Ty::float(), true)]);
}
