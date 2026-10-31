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
    let status = union(vec![s("active"), s("banned")]);
    assert_cmp_table(
        &e,
        &[
            // safe types
            (Ty::int(), Ty::int(), true),
            (Ty::int(), Ty::string(), false),
            (Ty::int(), Ty::float(), false),
            (Ty::int(), Ty::bool(), false),
            (Ty::int(), Ty::int().nullable(), true),
            (Ty::int().nullable(), Ty::string().nullable(), true), // both may be null
            (Ty::int(), Ty::null(), false),
            (Ty::int().nullable(), Ty::null(), true),
            (Ty::null(), Ty::null(), true),
            (s("a"), Ty::string(), true),
            (s("a"), s("b"), false),
            (s("a"), status.clone(), false),
            (s("active"), status.clone(), true),
            (status.clone(), union(vec![s("banned"), s("deleted")]), true),
            (status.clone(), union(vec![s("x"), s("y")]), false),
            (Ty::lit_int(1), Ty::int(), true),
            (Ty::lit_int(1), Ty::string(), false),
            (union(vec![Ty::int(), Ty::string()]), Ty::string(), true),
            (Ty::list(Ty::int()), Ty::list(Ty::string()), true),
            (Ty::list(Ty::int()), Ty::mutable_list(Ty::int()), true),
            (Ty::list(Ty::int()), Ty::map(Ty::int(), Ty::int()), false),
            (Ty::list(Ty::int()), Ty::int(), false),
            (rec(&[("x", Ty::int())]), rec(&[("x", Ty::int())]), true),
            (rec(&[("x", Ty::int())]), rec(&[("y", Ty::int())]), false),
            // classes
            (cls("Dog"), cls("Pet"), true),
            (cls("Dog"), cls("Cat"), false),
            (cls("Pet"), cls("Robot"), false),
            (cls("Robot"), cls("Named"), true), // open class may implement the interface
            (cls("Cat"), cls("Named"), false),  // final class doesn't
            (cls("Dog"), Ty::int(), false),
            (cls("Dog"), Ty::object(), true),
            // PHP family: loose
            (Ty::php_int(), Ty::php_string(), true),
            (Ty::php_array(), Ty::php_bool(), true),
            (Ty::mixed(), Ty::int(), true),
            // safe vs PHP: strict overlap
            (Ty::int(), Ty::php_int(), true),
            (Ty::int(), Ty::positive_int(), true),
            (Ty::int(), Ty::scalar(), true),
            (Ty::int(), Ty::php_string(), false),
            (Ty::string(), Ty::non_empty_string(), true),
            (s("a"), Ty::php_string(), true),
            (Ty::list(Ty::int()), Ty::php_list(Ty::php_int()), true),
            (Ty::list(Ty::int()), Ty::php_array(), true),
            (Ty::int(), Ty::php_array(), false),
            // platform / params / error
            (Ty::platform(Ty::string()), Ty::string(), true),
            (Ty::platform(Ty::string()), Ty::null(), true),
            (Ty::platform(Ty::string()), Ty::int(), false),
            (Ty::any_platform(), Ty::int(), true),
            (tp(), Ty::int(), true),
            (up(), cls("Dog"), true),
            (up(), Ty::int(), false),
            (Ty::error(), Ty::int(), true),
        ],
    );
}
