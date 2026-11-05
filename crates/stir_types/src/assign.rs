//! The assignability relation ([`is_assignable`], [`check_assignable`]).
//!
//! The rules, in the order they are tried:
//!
//! 1. identical types, `Error` on either side, and `never` sources;
//! 2. a union source needs every member assignable; an intersection target needs the
//!    source assignable to every member;
//! 3. `mixed` and untyped targets accept everything;
//! 4. a union target needs some member to accept the source;
//! 5. widening sources: an untyped value only reaches `mixed`; a type parameter is replaced
//!    by its bound; `static`/`$this` by their class; an intersection source needs some member
//!    assignable;
//! 6. structural rules.

use std::collections::VecDeque;
use std::fmt;

use rustc_hash::FxHashSet;
use smol_str::SmolStr;

use crate::env::{TypeEnv, Variance};
use crate::ty::{FnTy, LitValue, ParamKind, SelfKind, ShapeKey, ShapeTy, Ty, TyKind as K};

/// Why `from` is not assignable to `to`. Nested reasons carry the [`Mismatch`] of the
/// component that failed, so the [`Display`](fmt::Display) output reads like
/// "key `y` of array shape: int is not assignable to string".
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Mismatch {
    pub from: Ty,
    pub to: Ty,
    pub reason: MismatchReason,
}

/// The reason of a [`Mismatch`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MismatchReason {
    /// No more specific explanation.
    Incompatible,
    /// A possibly-null value was used where null is not allowed.
    NullToNonNull,
    /// The source class does not extend/implement the target class.
    NotSubtype,
    /// A member of the source union is not assignable.
    UnionMember { member: Ty, cause: Box<Mismatch> },
    /// No member of the target union accepts the source.
    NoUnionMember,
    /// The source is not assignable to a member of the target intersection.
    IntersectionMember { member: Ty, cause: Box<Mismatch> },
    /// A type argument does not satisfy the parameter's variance.
    TypeArgument {
        index: usize,
        variance: Variance,
        cause: Box<Mismatch>,
    },
    /// An array-shape entry's type is not assignable.
    ShapeKey { key: ShapeKey, cause: Box<Mismatch> },
    /// The source shape lacks a required key.
    ShapeMissingKey { key: ShapeKey },
    /// The key is optional in the source but required in the target.
    ShapeOptionalKey { key: ShapeKey },
    /// The source may be empty but the target is `non-empty-*`.
    MaybeEmpty,
    /// A function-type parameter is not assignable (contravariantly).
    FnParam { index: usize, cause: Box<Mismatch> },
    /// A function-type return type is not assignable.
    FnReturn { cause: Box<Mismatch> },
    /// The source function requires parameter `index`, which callers of the target may omit.
    FnRequiresParam { index: usize },
    /// Callers of the target may pass argument `index`, which the source does not accept.
    FnExtraArgument { index: usize },
    /// A non-closure callable was used where a `Closure` is expected.
    NotAClosure,
    /// The relation recursed too deeply (pathological generic hierarchies).
    RecursionLimit,
}

impl Mismatch {
    fn new(from: &Ty, to: &Ty, reason: MismatchReason) -> Self {
        Self {
            from: from.clone(),
            to: to.clone(),
            reason,
        }
    }

    /// The innermost mismatch of the chain.
    pub fn root(&self) -> &Mismatch {
        match &self.reason {
            MismatchReason::UnionMember { cause, .. }
            | MismatchReason::IntersectionMember { cause, .. }
            | MismatchReason::TypeArgument { cause, .. }
            | MismatchReason::ShapeKey { cause, .. }
            | MismatchReason::FnParam { cause, .. }
            | MismatchReason::FnReturn { cause } => cause.root(),
            _ => self,
        }
    }
}

fn key_str(key: &ShapeKey) -> String {
    match key {
        ShapeKey::Int(n) => n.to_string(),
        ShapeKey::Str(s) => s.to_string(),
    }
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use MismatchReason as R;
        let base = |f: &mut fmt::Formatter<'_>| {
            write!(f, "{} is not assignable to {}", self.from, self.to)
        };
        match &self.reason {
            R::UnionMember { cause, .. } | R::IntersectionMember { cause, .. } => cause.fmt(f),
            R::TypeArgument {
                index,
                variance,
                cause,
            } => {
                let v = match variance {
                    Variance::Invariant => " (invariant)",
                    Variance::In => " (contravariant)",
                    Variance::Out => "",
                };
                write!(f, "type argument #{} of {}{v}: {cause}", index + 1, self.to)
            }
            R::ShapeKey { key, cause } => {
                write!(f, "key `{}` of array shape: {cause}", key_str(key))
            }
            R::FnParam { index, cause } => {
                write!(f, "parameter #{} of function type: {cause}", index + 1)
            }
            R::FnReturn { cause } => write!(f, "return type of function type: {cause}"),
            R::Incompatible | R::NotSubtype | R::NoUnionMember => base(f),
            R::NullToNonNull => {
                base(f)?;
                f.write_str(" (the value may be null)")
            }
            R::ShapeMissingKey { key } => {
                base(f)?;
                write!(f, ": missing key `{}`", key_str(key))
            }
            R::ShapeOptionalKey { key } => {
                base(f)?;
                write!(
                    f,
                    ": key `{}` is optional but required by {}",
                    key_str(key),
                    self.to
                )
            }
            R::MaybeEmpty => {
                base(f)?;
                write!(f, ": {} may be empty", self.from)
            }
            R::FnRequiresParam { index } => {
                base(f)?;
                write!(
                    f,
                    ": it requires parameter #{} which callers may omit",
                    index + 1
                )
            }
            R::FnExtraArgument { index } => {
                base(f)?;
                write!(
                    f,
                    ": callers may pass argument #{} which it does not accept",
                    index + 1
                )
            }
            R::NotAClosure => {
                base(f)?;
                f.write_str(" (not a Closure)")
            }
            R::RecursionLimit => {
                base(f)?;
                f.write_str(" (type is too deeply nested)")
            }
        }
    }
}

/// Whether a value of type `from` may be used where `to` is expected.
pub fn is_assignable(env: &dyn TypeEnv, from: &Ty, to: &Ty) -> bool {
    check_assignable(env, from, to).is_ok()
}

/// Like [`is_assignable`], but explains a failure.
pub fn check_assignable(env: &dyn TypeEnv, from: &Ty, to: &Ty) -> Result<(), Mismatch> {
    Checker::new(env).check(from, to)
}

/// Would `from` be assignable to `to` if `mixed` were accepted everywhere, as PHPStan does
/// below level 9? `array-key` counts because a plain `array` has it as its key.
pub fn assignable_ignoring_mixed(env: &dyn TypeEnv, from: &Ty, to: &Ty) -> bool {
    let mut ck = Checker::new(env);
    ck.mixed_ok = true;
    ck.ok(from, to)
}

/// `from` isn't always acceptable to `to`, but can be: PHPStan's "maybe", which it reports
/// from level 7 (`checkUnionTypes`). It's a union with an acceptable member, or a wider type
/// of the same kind: `object` for a class, `iterable` for an array, `string` for a
/// `non-empty-string`, `array` for a `list`.
pub fn maybe_assignable(env: &dyn TypeEnv, from: &Ty, to: &Ty) -> bool {
    Checker::new(env).maybe(from, to, 0)
}

/// Mutual assignability (used for invariant positions).
pub fn is_equivalent(env: &dyn TypeEnv, a: &Ty, b: &Ty) -> bool {
    a == b || (is_assignable(env, a, b) && is_assignable(env, b, a))
}

/// PHP 8 numeric-string check (leading/trailing whitespace allowed).
pub(crate) fn is_numeric_str(s: &str) -> bool {
    let t = s.trim_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    !t.is_empty()
        && t.chars().any(|c| c.is_ascii_digit())
        && t.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E'))
        && t.parse::<f64>().is_ok()
}

pub(crate) fn is_closure_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("Closure")
}

const MAX_DEPTH: u32 = 128;

type Res = Result<(), Mismatch>;

pub(crate) struct Checker<'a> {
    pub(crate) env: &'a dyn TypeEnv,
    depth: u32,
    /// Accept `mixed` as a source: see [`assignable_ignoring_mixed`].
    mixed_ok: bool,
}

fn lit(t: &Ty) -> Option<&LitValue> {
    match t.kind() {
        K::Literal(l) => Some(l),
        _ => None,
    }
}

fn is_int_like(t: &Ty) -> bool {
    matches!(t.kind(), K::Int | K::IntRange { .. }) || matches!(lit(t), Some(LitValue::Int(_)))
}

fn is_float_like(t: &Ty) -> bool {
    matches!(t.kind(), K::Float) || matches!(lit(t), Some(LitValue::Float(_)))
}

fn is_string_like(t: &Ty) -> bool {
    matches!(
        t.kind(),
        K::String | K::NonEmptyString | K::NumericString | K::ClassString(_)
    ) || matches!(lit(t), Some(LitValue::String(_)))
}

fn is_bool_like(t: &Ty) -> bool {
    matches!(t.kind(), K::Bool) || matches!(lit(t), Some(LitValue::Bool(_)))
}

fn str_lit(t: &Ty) -> Option<&str> {
    match lit(t) {
        Some(LitValue::String(s)) => Some(s),
        _ => None,
    }
}

fn shape_key_ty(key: &ShapeKey) -> Ty {
    match key {
        ShapeKey::Int(n) => Ty::lit_int(*n),
        ShapeKey::Str(s) => Ty::lit_string(s.clone()),
    }
}

fn shape_has_required(s: &ShapeTy) -> bool {
    s.fields.iter().any(|f| !f.optional)
}

/// Whether a sealed shape is list-like: integer keys `0..n` with optional keys trailing.
fn shape_is_list(s: &ShapeTy) -> bool {
    if !s.sealed {
        return false;
    }
    let mut keys: Vec<(i64, bool)> = Vec::new();
    for f in &s.fields {
        match f.key {
            ShapeKey::Int(n) => keys.push((n, f.optional)),
            ShapeKey::Str(_) => return false,
        }
    }
    keys.sort_unstable();
    let mut seen_optional = false;
    for (i, (k, opt)) in keys.iter().enumerate() {
        if *k != i as i64 || (seen_optional && !opt) {
            return false;
        }
        seen_optional |= opt;
    }
    true
}

impl<'a> Checker<'a> {
    pub(crate) fn new(env: &'a dyn TypeEnv) -> Self {
        Self {
            env,
            depth: 0,
            mixed_ok: false,
        }
    }

    pub(crate) fn ok(&mut self, from: &Ty, to: &Ty) -> bool {
        self.check(from, to).is_ok()
    }

    pub(crate) fn check(&mut self, from: &Ty, to: &Ty) -> Res {
        if self.depth > MAX_DEPTH {
            return Err(Mismatch::new(from, to, MismatchReason::RecursionLimit));
        }
        self.depth += 1;
        let r = self.check_inner(from, to);
        self.depth -= 1;
        r
    }

    fn check_inner(&mut self, from: &Ty, to: &Ty) -> Res {
        use MismatchReason as R;
        if from == to {
            return Ok(());
        }
        // undecided, it's either branch
        if let K::Conditional(c) = from.kind() {
            return self.check(&Ty::union([c.then.clone(), c.otherwise.clone()]), to);
        }
        if let K::Conditional(c) = to.kind() {
            return self.check(from, &Ty::union([c.then.clone(), c.otherwise.clone()]));
        }
        match (from.kind(), to.kind()) {
            (K::Error, _) | (_, K::Error) | (K::Never, _) => return Ok(()),
            (K::Mixed | K::Untyped | K::ArrayKey, _) if self.mixed_ok => return Ok(()),
            _ => {}
        }
        if let K::Union(ms) = from.kind() {
            for m in ms.iter() {
                self.check(m, to).map_err(|cause| {
                    Mismatch::new(
                        from,
                        to,
                        R::UnionMember {
                            member: m.clone(),
                            cause: Box::new(cause),
                        },
                    )
                })?;
            }
            return Ok(());
        }
        if let K::Intersection(ms) = to.kind() {
            for m in ms.iter() {
                self.check(from, m).map_err(|cause| {
                    Mismatch::new(
                        from,
                        to,
                        R::IntersectionMember {
                            member: m.clone(),
                            cause: Box::new(cause),
                        },
                    )
                })?;
            }
            return Ok(());
        }
        if matches!(to.kind(), K::Mixed | K::Untyped) {
            return Ok(());
        }
        // `array-key` is `int|string`
        if matches!(from.kind(), K::ArrayKey)
            && to.members().iter().any(|m| matches!(m.kind(), K::Int))
            && to.members().iter().any(|m| matches!(m.kind(), K::String))
        {
            return Ok(());
        }
        let to_union = match to.kind() {
            K::Union(ms) => {
                if ms.iter().any(|m| self.ok(from, m)) {
                    return Ok(());
                }
                true
            }
            _ => false,
        };

        // Widening sources.
        match from.kind() {
            K::Untyped => return Err(Mismatch::new(from, to, R::Incompatible)),
            K::Param { id, .. } => {
                if let Some(bound) = self.env.param_bound(*id)
                    && self.ok(&bound, to)
                {
                    return Ok(());
                }
            }
            K::SelfType { class, .. }
                if !matches!(to.kind(), K::SelfType { .. }) && self.ok(class, to) =>
            {
                return Ok(());
            }
            K::Intersection(ms) if ms.iter().any(|m| self.ok(m, to)) => return Ok(()),
            _ => {}
        }

        if to_union {
            let non_null: Vec<&Ty> = to.members().iter().filter(|m| !m.is_null()).collect();
            if let [single] = non_null.as_slice()
                && !from.is_null()
            {
                // `X -> ?T`: explain against `T`.
                let mut e = self.check(from, single).expect_err("checked above");
                if e.from == *from && e.to == **single {
                    e.to = to.clone();
                }
                return Err(e);
            }
            let reason = if from.is_null() {
                R::NullToNonNull
            } else {
                R::NoUnionMember
            };
            return Err(Mismatch::new(from, to, reason));
        }

        self.structural(from, to)
    }

    fn arg(&mut self, from: &Ty, to: &Ty, index: usize, variance: Variance, a: &Ty, b: &Ty) -> Res {
        let r = match variance {
            Variance::Out => self.check(a, b),
            Variance::In => self.check(b, a),
            Variance::Invariant => self.check(a, b).and_then(|()| self.check(b, a)),
        };
        r.map_err(|cause| {
            Mismatch::new(
                from,
                to,
                MismatchReason::TypeArgument {
                    index,
                    variance,
                    cause: Box::new(cause),
                },
            )
        })
    }

    fn fail(&self, from: &Ty, to: &Ty) -> Res {
        use MismatchReason as R;
        let reason = if from.is_null() {
            R::NullToNonNull
        } else if matches!(from.kind(), K::Class { .. }) && matches!(to.kind(), K::Class { .. }) {
            R::NotSubtype
        } else {
            R::Incompatible
        };
        Err(Mismatch::new(from, to, reason))
    }

    fn structural(&mut self, from: &Ty, to: &Ty) -> Res {
        use MismatchReason as R;
        let (f, t) = (from.kind(), to.kind());
        match (f, t) {
            // an empty PHP array literal fits any PHP array type
            (
                K::List {
                    elem,
                    non_empty: false,
                },
                K::Array {
                    non_empty: false, ..
                },
            ) if matches!(elem.kind(), K::Never) => {
                return Ok(());
            }
            (
                K::List {
                    elem,
                    non_empty: false,
                },
                K::Shape(_),
            ) if matches!(elem.kind(), K::Never) => {
                return Ok(());
            }
            (K::Class { .. }, K::Class { name: n2, args: a2 }) => {
                return self.class(from, to, n2, a2);
            }
            // a string naming an existing class
            (K::Literal(LitValue::String(s)), K::ClassString(of)) => {
                let name = s.trim_start_matches('\\');
                if name.is_empty() || !self.env.is_class(name) {
                    return self.fail(from, to);
                }
                return match of {
                    None => Ok(()),
                    Some(b) => {
                        self.arg(from, to, 0, Variance::Out, &Ty::class(name, Vec::new()), b)
                    }
                };
            }
            (K::ClassString(a), K::ClassString(b)) => {
                return match (a, b) {
                    (_, None) => Ok(()),
                    (None, Some(_)) => self.fail(from, to),
                    (Some(a), Some(b)) => self.arg(from, to, 0, Variance::Out, a, b),
                };
            }
            (
                K::Array {
                    key: k1,
                    value: v1,
                    non_empty: ne1,
                },
                K::Array {
                    key,
                    value,
                    non_empty,
                },
            ) => {
                if *non_empty && !ne1 {
                    return Err(Mismatch::new(from, to, R::MaybeEmpty));
                }
                self.arg(from, to, 0, Variance::Out, k1, key)?;
                return self.arg(from, to, 1, Variance::Out, v1, value);
            }
            (
                K::List {
                    elem,
                    non_empty: ne1,
                },
                K::Array {
                    key,
                    value,
                    non_empty,
                },
            ) => {
                if *non_empty && !ne1 {
                    return Err(Mismatch::new(from, to, R::MaybeEmpty));
                }
                self.arg(from, to, 0, Variance::Out, &Ty::non_negative_int(), key)?;
                return self.arg(from, to, 1, Variance::Out, elem, value);
            }
            (
                K::List {
                    elem: e1,
                    non_empty: ne1,
                },
                K::List { elem, non_empty },
            ) => {
                if *non_empty && !ne1 {
                    return Err(Mismatch::new(from, to, R::MaybeEmpty));
                }
                return self.arg(from, to, 0, Variance::Out, e1, elem);
            }
            (
                K::Shape(s),
                K::Array {
                    key,
                    value,
                    non_empty,
                },
            ) => {
                if *non_empty && !shape_has_required(s) {
                    return Err(Mismatch::new(from, to, R::MaybeEmpty));
                }
                for fl in &s.fields {
                    self.arg(from, to, 0, Variance::Out, &shape_key_ty(&fl.key), key)?;
                    self.check(&fl.ty, value).map_err(|cause| {
                        Mismatch::new(
                            from,
                            to,
                            R::ShapeKey {
                                key: fl.key.clone(),
                                cause: Box::new(cause),
                            },
                        )
                    })?;
                }
                if !s.sealed {
                    self.arg(from, to, 0, Variance::Out, &Ty::array_key(), key)?;
                    self.arg(from, to, 1, Variance::Out, &Ty::mixed(), value)?;
                }
                return Ok(());
            }
            (K::Shape(s), K::List { elem, non_empty }) => {
                if !shape_is_list(s) {
                    return self.fail(from, to);
                }
                if *non_empty && !shape_has_required(s) {
                    return Err(Mismatch::new(from, to, R::MaybeEmpty));
                }
                for fl in &s.fields {
                    self.check(&fl.ty, elem).map_err(|cause| {
                        Mismatch::new(
                            from,
                            to,
                            R::ShapeKey {
                                key: fl.key.clone(),
                                cause: Box::new(cause),
                            },
                        )
                    })?;
                }
                return Ok(());
            }
            (K::Shape(a), K::Shape(b)) => return self.shape(from, to, a, b),
            (_, K::Iterable { key, value }) => {
                let Some((k, v)) = self.iter_kv(from) else {
                    return self.fail(from, to);
                };
                self.arg(from, to, 0, Variance::Out, &k, key)?;
                return self.arg(from, to, 1, Variance::Out, &v, value);
            }
            // PHP callables: `'strlen'`, `[$object, 'method']`
            (
                K::String
                | K::NonEmptyString
                | K::ClassString(_)
                | K::List { .. }
                | K::Array { .. }
                | K::Shape(_),
                K::Callable { closure: false, .. },
            ) => return Ok(()),
            (K::Literal(LitValue::String(_)), K::Callable { closure: false, .. }) => {
                return Ok(());
            }
            (_, K::Callable { sig, closure }) => {
                return self.callable(from, to, sig.as_ref(), *closure);
            }
            (
                K::SelfType {
                    kind: k1,
                    class: c1,
                },
                K::SelfType {
                    kind: k2,
                    class: c2,
                },
            ) => {
                if k1 == k2 || (*k1 == SelfKind::This && *k2 == SelfKind::Static) {
                    return self.check(c1, c2);
                }
                return self.fail(from, to);
            }
            (K::Class { name, .. }, K::SelfType { class, .. }) => {
                if self.env.is_final(name) {
                    return self.check(from, class);
                }
                return self.fail(from, to);
            }
            _ => {}
        }

        let ok = match t {
            K::Int => is_int_like(from),
            K::Float => is_int_like(from) || is_float_like(from),
            K::String => is_string_like(from),
            K::Bool => is_bool_like(from),
            K::Object => matches!(f, K::Class { .. } | K::Callable { closure: true, .. }),
            K::Scalar => {
                is_int_like(from)
                    || is_float_like(from)
                    || is_string_like(from)
                    || is_bool_like(from)
                    || matches!(f, K::Numeric | K::ArrayKey)
            }
            K::Numeric => {
                is_int_like(from)
                    || is_float_like(from)
                    || matches!(f, K::NumericString)
                    || str_lit(from).is_some_and(is_numeric_str)
            }
            K::ArrayKey => is_int_like(from) || is_string_like(from),
            K::NonEmptyString => {
                matches!(f, K::NumericString | K::ClassString(_))
                    || str_lit(from).is_some_and(|s| !s.is_empty())
            }
            K::NumericString => str_lit(from).is_some_and(is_numeric_str),
            K::IntRange { min, max } => {
                let (lo, hi) = match f {
                    K::IntRange { min: a, max: b } => (*a, *b),
                    K::Literal(LitValue::Int(n)) => (Some(*n), Some(*n)),
                    _ => return self.fail(from, to),
                };
                let lo_ok = match (min, lo) {
                    (None, _) => true,
                    (Some(_), None) => false,
                    (Some(m), Some(l)) => l >= *m,
                };
                let hi_ok = match (max, hi) {
                    (None, _) => true,
                    (Some(_), None) => false,
                    (Some(m), Some(h)) => h <= *m,
                };
                lo_ok && hi_ok
            }
            K::Class { name, .. } if is_closure_name(name) => {
                matches!(f, K::Callable { closure: true, .. })
            }
            _ => false,
        };
        if ok { Ok(()) } else { self.fail(from, to) }
    }

    fn shape(&mut self, from: &Ty, to: &Ty, a: &ShapeTy, b: &ShapeTy) -> Res {
        use MismatchReason as R;
        for bf in &b.fields {
            match a.field(&bf.key) {
                Some(af) => {
                    if af.optional && !bf.optional {
                        return Err(Mismatch::new(
                            from,
                            to,
                            R::ShapeOptionalKey {
                                key: bf.key.clone(),
                            },
                        ));
                    }
                    self.check(&af.ty, &bf.ty).map_err(|cause| {
                        Mismatch::new(
                            from,
                            to,
                            R::ShapeKey {
                                key: bf.key.clone(),
                                cause: Box::new(cause),
                            },
                        )
                    })?;
                }
                None => {
                    if !bf.optional {
                        return Err(Mismatch::new(
                            from,
                            to,
                            R::ShapeMissingKey {
                                key: bf.key.clone(),
                            },
                        ));
                    }
                    if !a.sealed {
                        self.check(&Ty::mixed(), &bf.ty).map_err(|cause| {
                            Mismatch::new(
                                from,
                                to,
                                R::ShapeKey {
                                    key: bf.key.clone(),
                                    cause: Box::new(cause),
                                },
                            )
                        })?;
                    }
                }
            }
        }
        // like PHPStan, a shape only says what it has: more keys in the value are fine
        Ok(())
    }

    fn fn_sig(&mut self, from: &Ty, to: &Ty, a: &FnTy, b: &FnTy) -> Res {
        use MismatchReason as R;
        let n = a.params.len().max(b.params.len());
        for i in 0..n {
            match (a.param_at(i), b.param_at(i)) {
                (None, None) => break,
                (Some(ap), None) => {
                    if ap.kind == ParamKind::Required {
                        return Err(Mismatch::new(from, to, R::FnRequiresParam { index: i }));
                    }
                }
                // PHP ignores the extra arguments a callback doesn't take
                (None, Some(_)) => {}
                (Some(ap), Some(bp)) => {
                    match (ap.kind, bp.kind) {
                        (ParamKind::Required, ParamKind::Optional | ParamKind::Variadic) => {
                            return Err(Mismatch::new(from, to, R::FnRequiresParam { index: i }));
                        }
                        (ParamKind::Optional, ParamKind::Variadic) => {
                            return Err(Mismatch::new(from, to, R::FnExtraArgument { index: i }));
                        }
                        _ => {}
                    }
                    let any = |t: &Ty| matches!(t.kind(), K::Mixed | K::Untyped);
                    if any(&ap.ty) || any(&bp.ty) {
                        continue;
                    }
                    self.check(&bp.ty, &ap.ty).map_err(|cause| {
                        Mismatch::new(
                            from,
                            to,
                            R::FnParam {
                                index: i,
                                cause: Box::new(cause),
                            },
                        )
                    })?;
                }
            }
        }
        // a caller that expects anything takes whatever comes back
        if matches!(b.ret.kind(), K::Mixed | K::Untyped)
            || matches!(a.ret.kind(), K::Mixed | K::Untyped)
        {
            return Ok(());
        }
        self.check(&a.ret, &b.ret).map_err(|cause| {
            Mismatch::new(
                from,
                to,
                R::FnReturn {
                    cause: Box::new(cause),
                },
            )
        })
    }

    fn callable(&mut self, from: &Ty, to: &Ty, tsig: Option<&FnTy>, tclosure: bool) -> Res {
        match from.kind() {
            K::Callable { sig, closure } => {
                if tclosure && !closure {
                    return Err(Mismatch::new(from, to, MismatchReason::NotAClosure));
                }
                match (sig, tsig) {
                    // An untyped callable is accepted by a typed one (PHP leniency).
                    (_, None) | (None, Some(_)) => Ok(()),
                    (Some(a), Some(b)) => self.fn_sig(from, to, a, b),
                }
            }
            K::Class { name, .. } if is_closure_name(name) => Ok(()),
            K::Class { name, args } if !tclosure => match self.env.invoke_signature(name, args) {
                Some(sig) => match tsig {
                    None => Ok(()),
                    Some(b) => self.fn_sig(from, to, &sig, b),
                },
                None => self.fail(from, to),
            },
            _ => self.fail(from, to),
        }
    }

    /// Breadth-first search of `start`'s supertypes (including itself) for a class whose
    /// name satisfies `pred`.
    pub(crate) fn find_super(&self, start: &Ty, pred: &dyn Fn(&str) -> bool) -> Option<Ty> {
        let mut queue = VecDeque::from([start.clone()]);
        let mut visited: FxHashSet<SmolStr> = FxHashSet::default();
        while let Some(t) = queue.pop_front() {
            let K::Class { name, args } = t.kind() else {
                continue;
            };
            if !visited.insert(name.clone()) {
                continue;
            }
            if pred(name) {
                return Some(t);
            }
            queue.extend(self.env.supertypes(name, args));
        }
        None
    }

    fn class(&mut self, from: &Ty, to: &Ty, n2: &str, a2: &[Ty]) -> Res {
        let Some(sup) = self.find_super(from, &|n| n == n2) else {
            return Err(Mismatch::new(from, to, MismatchReason::NotSubtype));
        };
        let K::Class { args: sargs, .. } = sup.kind() else {
            unreachable!("find_super returns classes")
        };
        if sargs.is_empty() || a2.is_empty() {
            // Raw (unparameterised) use on either side: accept, as PHPStan does.
            return Ok(());
        }
        for (i, (x, y)) in sargs.iter().zip(a2).enumerate() {
            if self.mixed_ok && matches!(y.kind(), K::Mixed | K::Untyped) {
                continue;
            }
            let (v, y) = match y.kind() {
                K::CovariantArg(t) => (Variance::Out, t),
                K::ContravariantArg(t) => (Variance::In, t),
                _ => (self.env.variance(n2, i), y),
            };
            self.arg(from, to, i, v, x, y)?;
        }
        Ok(())
    }

    /// See [`maybe_assignable`].
    fn maybe(&mut self, from: &Ty, to: &Ty, depth: u32) -> bool {
        if depth > 16 {
            return false;
        }
        if self.ok(from, to) {
            return true;
        }
        let d = depth + 1;
        let (f, t) = (from.kind(), to.kind());
        if let K::Union(ms) = f {
            return ms.iter().any(|m| self.maybe(m, to, d));
        }
        if let K::Union(ms) = t {
            return ms.iter().any(|m| self.maybe(from, m, d));
        }
        if let K::Intersection(ms) = t {
            return ms.iter().all(|m| self.maybe(from, m, d));
        }
        if let K::Intersection(ms) = f {
            return ms.iter().any(|m| self.maybe(m, to, d));
        }
        match (f, t) {
            (K::Mixed | K::Untyped, _) => true,
            // a value within a template's bound may be what it turns out to be
            (_, K::Param { id, .. }) => self
                .env
                .param_bound(*id)
                .is_none_or(|b| self.maybe(from, &b, d)),
            (K::Param { id, .. }, _) => self
                .env
                .param_bound(*id)
                .is_none_or(|b| self.maybe(&b, to, d)),
            (K::SelfType { class, .. }, _) => self.maybe(&class.clone(), to, d),
            (K::Object, K::Class { .. } | K::SelfType { .. } | K::Callable { .. }) => true,
            (K::Object, K::Iterable { .. }) => true,
            (
                K::Iterable { key, value },
                K::Array {
                    key: k2, value: v2, ..
                },
            ) => self.maybe(key, k2, d) && self.maybe(value, v2, d),
            (K::Iterable { value, .. }, K::List { elem, .. }) => self.maybe(value, elem, d),
            (K::Iterable { .. }, K::Shape(_)) => true,
            (K::Iterable { .. }, K::Class { name, .. }) => {
                self.env.is_interface(name)
                    || self
                        .find_super(to, &|n| n.eq_ignore_ascii_case("Traversable"))
                        .is_some()
            }
            (K::Class { .. }, K::Iterable { .. }) => false,
            (
                K::Callable { .. },
                K::String | K::NonEmptyString | K::List { .. } | K::Array { .. } | K::Shape(_),
            ) => true,
            (K::Callable { .. }, K::Class { name, .. }) => {
                is_closure_name(name) || self.env.is_interface(name)
            }
            (K::Callable { closure: false, .. }, K::Callable { closure: true, .. }) => true,
            (K::Class { name, .. }, K::Callable { .. }) => !self.env.is_final(name),
            (K::ArrayKey, _) => is_int_like(to) || is_string_like(to),
            (K::Scalar, _) => {
                is_int_like(to)
                    || is_float_like(to)
                    || is_string_like(to)
                    || is_bool_like(to)
                    || matches!(t, K::Numeric | K::ArrayKey)
            }
            (K::Numeric, _) => {
                is_int_like(to) || is_float_like(to) || matches!(t, K::NumericString | K::String)
            }
            (K::Bool, K::Literal(LitValue::Bool(_))) => true,
            (
                K::String | K::NonEmptyString | K::NumericString,
                K::NonEmptyString | K::NumericString | K::ClassString(_),
            ) => true,
            (K::ClassString(_), K::ClassString(_)) => true,
            (K::Literal(LitValue::String(s)), K::ClassString(_)) => {
                let name = s.trim_start_matches('\\');
                !name.is_empty() && self.env.is_class(name)
            }
            (K::Int | K::IntRange { .. }, K::IntRange { .. }) => true,
            (K::Float, K::Numeric) => true,
            (
                K::Array { key, value, .. },
                K::Array {
                    key: k2, value: v2, ..
                },
            ) => self.maybe(key, k2, d) && self.maybe(value, v2, d),
            (K::List { elem, .. }, K::List { elem: e2, .. }) => self.maybe(elem, e2, d),
            (
                K::List { elem, .. },
                K::Array {
                    key: k2, value: v2, ..
                },
            ) => self.maybe(&Ty::non_negative_int(), k2, d) && self.maybe(elem, v2, d),
            (K::Array { key, value, .. }, K::List { elem, .. }) => {
                self.maybe(key, &Ty::non_negative_int(), d) && self.maybe(value, elem, d)
            }
            (K::Shape(a), K::Array { key, value, .. }) => a.fields.iter().all(|fl| {
                self.maybe(&shape_key_ty(&fl.key), key, d) && self.maybe(&fl.ty, value, d)
            }),
            (K::Shape(a), K::List { elem, .. }) => a
                .fields
                .iter()
                .all(|fl| matches!(fl.key, ShapeKey::Int(_)) && self.maybe(&fl.ty, elem, d)),
            (K::Array { key, value, .. }, K::Shape(b)) => b.fields.iter().all(|fl| {
                fl.optional
                    || (self.maybe(&shape_key_ty(&fl.key), key, d) && self.maybe(value, &fl.ty, d))
            }),
            (K::List { elem, .. }, K::Shape(b)) => b.fields.iter().all(|fl| {
                fl.optional || (matches!(fl.key, ShapeKey::Int(_)) && self.maybe(elem, &fl.ty, d))
            }),
            (K::Shape(a), K::Shape(b)) => b.fields.iter().all(|fl| match a.field(&fl.key) {
                Some(af) => self.maybe(&af.ty, &fl.ty, d),
                None => fl.optional || !a.sealed,
            }),
            (K::Class { .. }, K::Class { name: n2, args: a2 }) => {
                let Some(sup) = self.find_super(from, &|n| n == n2.as_str()) else {
                    return false;
                };
                let K::Class { args: sargs, .. } = sup.kind() else {
                    return false;
                };
                sargs
                    .iter()
                    .zip(a2)
                    .enumerate()
                    .all(|(i, (x, y))| match self.env.variance(n2, i) {
                        _ if matches!(y.kind(), K::CovariantArg(_) | K::ContravariantArg(_)) => {
                            match y.kind() {
                                K::CovariantArg(t) => self.maybe(x, t, d),
                                K::ContravariantArg(t) => self.maybe(t, x, d),
                                _ => false,
                            }
                        }
                        Variance::Out => self.maybe(x, y, d),
                        Variance::In => self.maybe(y, x, d),
                        Variance::Invariant => self.ok(x, y) && self.ok(y, x),
                    })
            }
            _ => false,
        }
    }

    /// Key and value types when iterating `from`, if it is iterable.
    fn iter_kv(&self, from: &Ty) -> Option<(Ty, Ty)> {
        Some(match from.kind() {
            K::Array { key, value, .. } | K::Iterable { key, value } => {
                (key.clone(), value.clone())
            }
            K::List { elem, .. } => (Ty::non_negative_int(), elem.clone()),
            K::Shape(s) => {
                let mut keys: Vec<Ty> = s.fields.iter().map(|f| shape_key_ty(&f.key)).collect();
                let mut values: Vec<Ty> = s.fields.iter().map(|f| f.ty.clone()).collect();
                if !s.sealed {
                    keys.push(Ty::array_key());
                    values.push(Ty::mixed());
                }
                (Ty::union(keys), Ty::union(values))
            }
            K::Class { .. } => {
                let t = self.find_super(from, &|n| n.eq_ignore_ascii_case("Traversable"))?;
                let K::Class { args, .. } = t.kind() else {
                    return None;
                };
                match args.as_slice() {
                    [k, v] => (k.clone(), v.clone()),
                    [v] => (Ty::mixed(), v.clone()),
                    _ => (Ty::mixed(), Ty::mixed()),
                }
            }
            _ => return None,
        })
    }
}
