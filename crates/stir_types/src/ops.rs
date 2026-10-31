//! Type operations: substitution, nullability, join/meet and flow narrowing.

use rustc_hash::FxHashMap;

use crate::assign::Checker;
use crate::env::TypeEnv;
use crate::ty::{Family, LitValue, ParamId, Ty, TyKind as K};

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

/// `ty` with `null` removed (`T?` → `T`, `T?!` → `T!`). `mixed` and `Any!` are returned
/// unchanged (there is no non-null top type).
pub fn without_null(ty: &Ty) -> Ty {
    match ty.kind() {
        K::Null => Ty::nothing(),
        K::Union(ms) if ms.iter().any(Ty::is_null) => {
            Ty::union(ms.iter().filter(|m| !m.is_null()).cloned())
        }
        K::Platform(inner) => Ty::platform(without_null(inner)),
        _ => ty.clone(),
    }
}

/// Whether a value of type `ty` may be null: `null`, nullable unions, `mixed`, `Any!`, `T!`
/// where `T` is nullable, and type parameters whose bound is nullable or absent (an
/// unbounded parameter is implicitly bounded by `Any?`, as in Kotlin).
pub fn is_nullable(env: &dyn TypeEnv, ty: &Ty) -> bool {
    match ty.kind() {
        K::Null | K::Mixed | K::AnyPlatform => true,
        K::Union(ms) => ms.iter().any(|m| is_nullable(env, m)),
        K::Intersection(ms) => ms.iter().all(|m| is_nullable(env, m)),
        K::Platform(inner) => is_nullable(env, inner),
        K::Param { id, .. } => env.param_bound(*id).is_none_or(|b| is_nullable(env, &b)),
        _ => false,
    }
}

/// Least upper bound used for e.g. the branches of an `if` expression: the more general of
/// the two if one is assignable to the other, otherwise their union (never widened to
/// `mixed`).
pub fn join(env: &dyn TypeEnv, a: &Ty, b: &Ty) -> Ty {
    let mut ck = Checker::new(env);
    if ck.ok(a, b) {
        b.clone()
    } else if ck.ok(b, a) {
        a.clone()
    } else {
        Ty::union([a.clone(), b.clone()])
    }
}

/// Greatest lower bound: the more specific of the two if related; distributes over unions;
/// an intersection for unrelated class-like types that can have common instances;
/// otherwise `Nothing`.
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
        Ty::nothing()
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

/// Splits `Bool`/`bool` into `true | false` and strips platform wrappers, so that
/// narrowing can work member by member.
fn expand(ty: &Ty) -> Vec<Ty> {
    let mut out = Vec::new();
    for m in ty.members() {
        match m.kind() {
            K::Bool | K::PhpBool => {
                let family = if matches!(m.kind(), K::Bool) {
                    Family::Safe
                } else {
                    Family::Php
                };
                out.push(Ty::literal(LitValue::Bool(true), family));
                out.push(Ty::literal(LitValue::Bool(false), family));
            }
            K::Platform(inner) => out.extend(expand(&inner.clone().nullable())),
            _ => out.push(m.clone()),
        }
    }
    out
}

/// The type of a value of type `declared` after a successful `is test` check (or `== null`,
/// or a literal `when` branch). Platform types are resolved, `mixed`/`Any!` become `test`.
///
/// Examples: `narrow_to(Int?, null) = null`, `narrow_to(String, "a") = "a"`,
/// `narrow_to("a" | "b", "a") = "a"`, `narrow_to(Any!, Int) = Int`.
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
        if matches!(m.kind(), K::Mixed | K::AnyPlatform) || ck.ok(test, &m) {
            out.push(test.clone());
        } else if ck.ok(&m, test) {
            out.push(m);
        } else if can_intersect(ck.env, &m, test) {
            out.push(Ty::intersection([m, test.clone()]));
        }
    }
    Ty::union(out)
}

/// The type of a value of type `declared` after a failed `is test` check (`!is`, `!= null`,
/// the remaining `when` branches). Members fully covered by `test` are removed; sealed
/// classes are split into their permitted subclasses when `test` is one of them.
///
/// Examples: `narrow_away("a" | "b" | "c", "a") = "b" | "c"`, `narrow_away(Int?, null) = Int`,
/// `narrow_away(Bool, true) = false`.
pub fn narrow_away(env: &dyn TypeEnv, declared: &Ty, test: &Ty) -> Ty {
    let mut ck = Checker::new(env);
    narrow_away_inner(&mut ck, declared, test, 0)
}

fn narrow_away_inner(ck: &mut Checker<'_>, declared: &Ty, test: &Ty, depth: u32) -> Ty {
    if declared.is_error() || test.is_error() {
        return Ty::error();
    }
    let mut out = Vec::new();
    for m in expand(declared) {
        if ck.ok(&m, test) {
            continue;
        }
        if depth < 32
            && let K::Class { name, args } = m.kind()
            && ck.ok(test, &m)
            && let Some(subs) = ck.env.sealed_subclasses(name, args)
        {
            out.push(narrow_away_inner(ck, &Ty::union(subs), test, depth + 1));
            continue;
        }
        out.push(m);
    }
    Ty::union(out)
}
