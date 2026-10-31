//! Comparability for `==`, `===`, `!=` and `!==`.

use crate::assign::{Checker, php_view};
use crate::env::TypeEnv;
use crate::ty::{Ty, TyKind as K};

/// Whether `a` and `b` may be compared with `==`/`===`/`!=`/`!==` ("comparing incompatible
/// types is an error").
///
/// * Two PHP-family types are always comparable (PHP's loose semantics).
/// * Otherwise the types must *overlap*: some value may inhabit both. Unions overlap if any
///   pair of members does (so `Int` vs `Int?` and overlapping literal unions are fine), a
///   literal overlaps its base type, and related types (one assignable to the other)
///   overlap. `Int` vs `String` and `Int` vs `Float` do not.
/// * A Kotlin type against a PHP type is checked through the Kotlin type's PHP view, with
///   strict (non-loose) overlap: `Int` vs `int` is fine, `Int` vs `string` is not.
/// * Kotlin collections are compared by constructor only (`List<Int>` vs `List<String>` is
///   allowed, both may be empty), unrelated interfaces/open classes may share a subclass,
///   `null` only overlaps nullable types, and `Error`, `mixed`, `Any!` and unbounded type
///   parameters overlap everything.
pub fn are_comparable(env: &dyn TypeEnv, a: &Ty, b: &Ty) -> bool {
    let mut ck = Checker::new(env);
    overlap(&mut ck, a, b, 0)
}

fn overlap(ck: &mut Checker<'_>, a: &Ty, b: &Ty, depth: u32) -> bool {
    if a == b || depth > 32 {
        return true;
    }
    let depth = depth + 1;
    let top = |t: &Ty| {
        matches!(
            t.kind(),
            K::Error | K::Nothing | K::Never | K::Mixed | K::AnyPlatform
        )
    };
    if top(a) || top(b) {
        return true;
    }
    if a.is_union() {
        return a.members().iter().any(|m| overlap(ck, m, b, depth));
    }
    if b.is_union() {
        return b.members().iter().any(|m| overlap(ck, a, m, depth));
    }
    // Unwrap one side at a time.
    for (x, y) in [(a, b), (b, a)] {
        match x.kind() {
            K::Platform(inner) => return overlap(ck, &inner.clone().nullable(), y, depth),
            K::Param { id, .. } => {
                return match ck.env.param_bound(*id) {
                    Some(bound) => overlap(ck, &bound, y, depth),
                    None => true,
                };
            }
            K::SelfType { class, .. } => return overlap(ck, class, y, depth),
            K::Intersection(ms) => return ms.iter().all(|m| overlap(ck, m, y, depth)),
            _ => {}
        }
    }
    if a.is_null() || b.is_null() {
        return false;
    }
    if a.is_php() && b.is_php() {
        return true;
    }
    if a.is_safe() && b.is_php() {
        let view = php_view(a).expect("safe type has a PHP view");
        return related(ck, &view, b);
    }
    if a.is_php() && b.is_safe() {
        let view = php_view(b).expect("safe type has a PHP view");
        return related(ck, a, &view);
    }
    match (a.kind(), b.kind()) {
        (K::List(_) | K::MutableList(_), K::List(_) | K::MutableList(_))
        | (K::Map(..) | K::MutableMap(..), K::Map(..) | K::MutableMap(..)) => true,
        (K::Class { name: n1, .. }, K::Class { name: n2, .. }) => {
            related(ck, a, b) || {
                let env = ck.env;
                let open_iface =
                    |iface: &str, other: &str| env.is_interface(iface) && !env.is_final(other);
                open_iface(n1, n2) || open_iface(n2, n1)
            }
        }
        _ => related(ck, a, b),
    }
}

fn related(ck: &mut Checker<'_>, a: &Ty, b: &Ty) -> bool {
    ck.ok(a, b) || ck.ok(b, a)
}
