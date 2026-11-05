//! Comparability for `==`, `===`, `!=` and `!==`.

use crate::assign::Checker;
use crate::env::TypeEnv;
use crate::ty::{Ty, TyKind as K};

/// Whether `a` and `b` may be compared with `==`/`===`/`!=`/`!==`.
///
/// * Two types that aren't classes are always comparable (PHP's loose semantics).
/// * Unions overlap if any pair of members does. `null` only overlaps nullable types.
/// * A class only overlaps related types (one assignable to the other), and unrelated
///   interfaces and non-final classes may share a subclass.
/// * `Error`, `never`, `mixed`, untyped values and unbounded type parameters overlap
///   everything.
pub fn are_comparable(env: &dyn TypeEnv, a: &Ty, b: &Ty) -> bool {
    let mut ck = Checker::new(env);
    overlap(&mut ck, a, b, 0)
}

fn overlap(ck: &mut Checker<'_>, a: &Ty, b: &Ty, depth: u32) -> bool {
    if a == b || depth > 32 {
        return true;
    }
    let depth = depth + 1;
    let top = |t: &Ty| matches!(t.kind(), K::Error | K::Never | K::Mixed | K::Untyped);
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
    match (a.kind(), b.kind()) {
        (K::Class { name: n1, .. }, K::Class { name: n2, .. }) => {
            related(ck, a, b) || {
                let env = ck.env;
                let open_iface =
                    |iface: &str, other: &str| env.is_interface(iface) && !env.is_final(other);
                open_iface(n1, n2) || open_iface(n2, n1)
            }
        }
        (K::Class { .. }, _) | (_, K::Class { .. }) => related(ck, a, b),
        _ => true,
    }
}

fn related(ck: &mut Checker<'_>, a: &Ty, b: &Ty) -> bool {
    ck.ok(a, b) || ck.ok(b, a)
}
