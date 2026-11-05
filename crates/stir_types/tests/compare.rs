mod common;

use common::*;
use stir_types::{Ty, TypeEnv, are_comparable};

#[track_caller]
fn assert_cmp_table(env: &dyn TypeEnv, cases: &[(Ty, Ty, bool)]) {
    let mut failures = Vec::new();
    for (a, b, expected) in cases {
        for (x, y) in [(a, b), (b, a)] {
            if are_comparable(env, x, y) != *expected {
                failures.push(format!("  {x} == {y}: expected {expected}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "comparability failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn comparability() {
    let e = env();
    assert_cmp_table(
        &e,
        &[
            // anything but a class is loosely comparable to anything but a class
            (Ty::int(), Ty::int(), true),
            (Ty::int(), Ty::string(), true),
            (Ty::plain_array(), Ty::bool(), true),
            (s("a"), s("b"), true),
            (Ty::mixed(), Ty::int(), true),
            // null needs a nullable side
            (Ty::int(), Ty::int().nullable(), true),
            (Ty::int().nullable(), Ty::string().nullable(), true), // both may be null
            (Ty::int(), Ty::null(), false),
            (Ty::int().nullable(), Ty::null(), true),
            (Ty::null(), Ty::null(), true),
            // classes
            (cls("Dog"), cls("Pet"), true),
            (cls("Dog"), cls("Cat"), false),
            (cls("Pet"), cls("Robot"), false),
            (cls("Robot"), cls("Named"), true), // non-final class may implement the interface
            (cls("Cat"), cls("Named"), false),  // final class doesn't
            (cls("Dog"), Ty::int(), false),
            (cls("Dog"), Ty::object(), true),
            (union(vec![cls("Dog"), Ty::int()]), Ty::string(), true),
            // untyped / params / error
            (Ty::untyped(), Ty::int(), true),
            (tp(), Ty::int(), true),
            (up(), cls("Dog"), true),
            (up(), Ty::int(), false),
            (Ty::error(), Ty::int(), true),
        ],
    );
}
