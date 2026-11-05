//! Type operations: substitution, nullability, join/meet and flow narrowing.

use rustc_hash::FxHashMap;

use crate::assign::{Checker, is_closure_name};
use crate::env::TypeEnv;
use crate::ty::{ParamId, Ty, TyKind as K};

/// Replaces type parameters according to `map` (parameters not in the map are kept).
pub fn subst(ty: &Ty, map: &FxHashMap<ParamId, Ty>) -> Ty {
    if map.is_empty() {
        return ty.clone();
    }
    ty.transform(&mut |t| match t.kind() {
        K::Param { id, .. } => Some(map.get(id).cloned().unwrap_or_else(|| t.clone())),
        _ => None,
    })
}

/// The type parameters occurring in `ty`, in order of first occurrence.
pub fn free_params(ty: &Ty) -> Vec<ParamId> {
    let mut out = Vec::new();
    ty.walk(&mut |t| {
        if let K::Param { id, .. } = t.kind()
            && !out.contains(id)
        {
            out.push(*id);
        }
        true
    });
    out
}

/// `ty` with `null` removed (`?T` → `T`). `mixed` and untyped values are returned unchanged
/// (there is no non-null top type).
pub fn without_null(ty: &Ty) -> Ty {
    match ty.kind() {
        K::Null => Ty::never(),
        K::Union(ms) if ms.iter().any(Ty::is_null) => {
            Ty::union(ms.iter().filter(|m| !m.is_null()).cloned())
        }
        _ => ty.clone(),
    }
}

/// Whether a value of type `ty` may be null: `null`, nullable unions, `mixed`, untyped values,
/// and type parameters whose bound is nullable or absent (an unbounded parameter is
/// implicitly bounded by `mixed`).
pub fn is_nullable(env: &dyn TypeEnv, ty: &Ty) -> bool {
    match ty.kind() {
        K::Null | K::Mixed | K::Untyped => true,
        K::Union(ms) => ms.iter().any(|m| is_nullable(env, m)),
        K::Intersection(ms) => ms.iter().all(|m| is_nullable(env, m)),
        K::Param { id, .. } => env.param_bound(*id).is_none_or(|b| is_nullable(env, &b)),
        _ => false,
    }
}

/// Least upper bound used for e.g. the branches of a ternary: the more general of
/// the two if one is assignable to the other, otherwise their union (never widened to
/// `mixed`).
pub fn join(env: &dyn TypeEnv, a: &Ty, b: &Ty) -> Ty {
    let mut ck = Checker::new(env);
    // a plain `Closure` fits any `Closure(A): R`, but says less
    let plain_closure = matches!(a.kind(), K::Class { name, .. } if is_closure_name(name));
    if ck.ok(b, a) && plain_closure {
        a.clone()
    } else if ck.ok(a, b) {
        b.clone()
    } else if ck.ok(b, a) {
        a.clone()
    } else {
        Ty::union([a.clone(), b.clone()])
    }
}

/// Greatest lower bound: the more specific of the two if related; distributes over unions;
/// an intersection for unrelated class-like types that can have common instances;
/// otherwise `never`.
pub fn meet(env: &dyn TypeEnv, a: &Ty, b: &Ty) -> Ty {
    let mut ck = Checker::new(env);
    meet_inner(&mut ck, a, b)
}

fn meet_inner(ck: &mut Checker<'_>, a: &Ty, b: &Ty) -> Ty {
    if ck.ok(a, b) {
        return a.clone();
    }
    if ck.ok(b, a) {
        return b.clone();
    }
    if a.is_union() {
        return Ty::union(
            a.members()
                .iter()
                .map(|m| meet_inner(ck, m, b))
                .collect::<Vec<_>>(),
        );
    }
    if b.is_union() {
        return Ty::union(
            b.members()
                .iter()
                .map(|m| meet_inner(ck, a, m))
                .collect::<Vec<_>>(),
        );
    }
    if can_intersect(ck.env, a, b) {
        Ty::intersection([a.clone(), b.clone()])
    } else {
        Ty::never()
    }
}

/// Whether two unrelated class-like types may still share instances.
fn can_intersect(env: &dyn TypeEnv, a: &Ty, b: &Ty) -> bool {
    // A type parameter can be instantiated with anything.
    if matches!(a.kind(), K::Param { .. }) || matches!(b.kind(), K::Param { .. }) {
        return true;
    }
    let class_like = |t: &Ty| {
        matches!(
            t.kind(),
            K::Class { .. } | K::SelfType { .. } | K::Intersection(_)
        )
    };
    if !class_like(a) || !class_like(b) {
        return false;
    }
    let info = |t: &Ty| match t.kind() {
        K::Class { name, .. } => Some((env.is_interface(name), env.is_final(name))),
        _ => None,
    };
    match (info(a), info(b)) {
        // `static` or an intersection can be anything class-like.
        (None, _) | (_, None) => true,
        (Some((ia, fa)), Some((ib, fb))) => (ia && !fb) || (ib && !fa),
    }
}

/// Splits `bool` into `true|false`, so that narrowing can work member by member.
fn expand(ty: &Ty) -> Vec<Ty> {
    let mut out = Vec::new();
    for m in ty.members() {
        match m.kind() {
            K::Bool => {
                out.push(Ty::lit_bool(true));
                out.push(Ty::lit_bool(false));
            }
            _ => out.push(m.clone()),
        }
    }
    out
}

/// The type of a value of type `declared` after a successful type check (`is_int($x)`,
/// `instanceof`, `=== null`, a literal `match` arm). `mixed` and untyped values become `test`.
///
/// Examples: `narrow_to(?int, null) = null`, `narrow_to(string, 'a') = 'a'`,
/// `narrow_to('a'|'b', 'a') = 'a'`, `narrow_to(mixed, int) = int`.
pub fn narrow_to(env: &dyn TypeEnv, declared: &Ty, test: &Ty) -> Ty {
    let mut ck = Checker::new(env);
    narrow_to_inner(&mut ck, declared, test)
}

fn narrow_to_inner(ck: &mut Checker<'_>, declared: &Ty, test: &Ty) -> Ty {
    if declared.is_error() || test.is_error() {
        return Ty::error();
    }
    if test.is_union() {
        return Ty::union(
            test.members()
                .iter()
                .map(|t| narrow_to_inner(ck, declared, t))
                .collect::<Vec<_>>(),
        );
    }
    let mut out = Vec::new();
    for m in expand(declared) {
        if matches!(m.kind(), K::Mixed | K::Untyped) || ck.ok(test, &m) {
            out.push(test.clone());
        } else if ck.ok(&m, test) {
            out.push(m);
        } else if can_intersect(ck.env, &m, test) {
            out.push(Ty::intersection([m, test.clone()]));
        }
    }
    Ty::union(out)
}

/// The type of a value of type `declared` after a failed type check (`!is_int($x)`,
/// `!== null`, the remaining `match` arms). Members fully covered by `test` are removed.
///
/// Examples: `narrow_away('a'|'b'|'c', 'a') = 'b'|'c'`, `narrow_away(?int, null) = int`,
/// `narrow_away(bool, true) = false`.
pub fn narrow_away(env: &dyn TypeEnv, declared: &Ty, test: &Ty) -> Ty {
    let mut ck = Checker::new(env);
    narrow_away_inner(&mut ck, declared, test)
}

fn narrow_away_inner(ck: &mut Checker<'_>, declared: &Ty, test: &Ty) -> Ty {
    if declared.is_error() || test.is_error() {
        return Ty::error();
    }
    let out: Vec<Ty> = expand(declared)
        .into_iter()
        .filter(|m| !ck.ok(m, test))
        .collect();
    Ty::union(out)
}
