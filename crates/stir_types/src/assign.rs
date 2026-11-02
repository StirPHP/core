//! The assignability relation ([`is_assignable`], [`check_assignable`]).
//!
//! The rules, in the order they are tried:
//!
//! 1. identical types, `Error` on either side, and bottom (`Nothing`/`never`) sources;
//! 2. a union source needs every member assignable; an intersection target needs the
//!    source assignable to every member;
//! 3. `mixed` and `Any!` accept everything;
//! 4. a union target needs some member to accept the source;
//! 5. widening sources: `T!` behaves as `T` or as non-null `T`; `Any!` only reaches `mixed`;
//!    a type parameter is replaced by its bound; `static`/`$this` by their class; an
//!    intersection source needs some member assignable;
//! 6. a platform target `T!` accepts what `T` or `T?` accepts;
//! 7. structural rules. A StirPHP-family source used where a non-StirPHP type is expected is
//!    first converted to its PHP view (`Int` → `int`, `List<T>` → `list<T>`,
//!    `Map<K, V>` → `array<K, V>`, records → sealed array shapes, `(A) -> R` →
//!    `Closure(A): R`), which is how "safe → unsafe is allowed" falls out. PHP-family
//!    sources are never assignable to StirPHP-family targets (except a sealed array shape with
//!    exactly a record's layout).

use std::collections::VecDeque;
use std::fmt;

use rustc_hash::FxHashSet;
use smol_str::SmolStr;

use crate::env::{TypeEnv, Variance};
use crate::ops::without_null;
use crate::ty::{
    Family, FnTy, LitValue, Literal, ParamKind, RecordTy, SelfKind, ShapeField, ShapeKey, ShapeTy,
    Ty, TyKind as K,
};

/// Why `from` is not assignable to `to`. Nested reasons carry the [`Mismatch`] of the
/// component that failed, so the [`Display`](fmt::Display) output reads like
/// "field `y` of record: Int is not assignable to String".
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
    /// A PHP-family type was used where a StirPHP-family type is expected (`int` → `Int`).
    UnsafeToSafe,
    /// `Any!` was used where something other than `mixed` is expected.
    AnyPlatformToSafe,
    /// A possibly-null value was used where null is not allowed.
    NullToNonNull,
    /// StirPHP numeric types do not widen (`Int` → `Float`).
    NoNumericWidening,
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
    /// A record field's type is not assignable.
    RecordField { name: SmolStr, cause: Box<Mismatch> },
    /// The source record lacks a field of the target.
    RecordMissingField { name: SmolStr },
    /// The source record has the target's field, but at another position.
    RecordFieldOrder {
        index: usize,
        expected: SmolStr,
        found: SmolStr,
    },
    /// An array shape is not exactly the layout of the target record.
    ShapeLayout,
    /// An array-shape entry's type is not assignable.
    ShapeKey { key: ShapeKey, cause: Box<Mismatch> },
    /// The source shape lacks a required key.
    ShapeMissingKey { key: ShapeKey },
    /// The key is optional in the source but required in the target.
    ShapeOptionalKey { key: ShapeKey },
    /// The source shape has a key the sealed target does not.
    ShapeExtraKey { key: ShapeKey },
    /// The source shape is unsealed but the target is sealed.
    ShapeUnsealed,
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
            | MismatchReason::RecordField { cause, .. }
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
            R::RecordField { name, cause } => write!(f, "field `{name}` of record: {cause}"),
            R::ShapeKey { key, cause } => {
                write!(f, "key `{}` of array shape: {cause}", key_str(key))
            }
            R::FnParam { index, cause } => {
                write!(f, "parameter #{} of function type: {cause}", index + 1)
            }
            R::FnReturn { cause } => write!(f, "return type of function type: {cause}"),
            R::Incompatible | R::NotSubtype | R::NoUnionMember => base(f),
            R::UnsafeToSafe => {
                base(f)?;
                f.write_str(" (a PHP type cannot be used as a StirPHP type; convert it explicitly)")
            }
            R::AnyPlatformToSafe => {
                base(f)?;
                f.write_str(" (an untyped PHP value must be narrowed with `is` first)")
            }
            R::NullToNonNull => {
                base(f)?;
                f.write_str(" (the value may be null)")
            }
            R::NoNumericWidening => {
                base(f)?;
                f.write_str(" (StirPHP numbers are not widened implicitly)")
            }
            R::RecordMissingField { name } => {
                base(f)?;
                write!(f, ": missing field `{name}`")
            }
            R::RecordFieldOrder {
                index,
                expected,
                found,
            } => {
                base(f)?;
                write!(
                    f,
                    ": field #{} is `{found}` but `{expected}` was expected (record fields must \
                     appear in the same order)",
                    index + 1
                )
            }
            R::ShapeLayout => {
                base(f)?;
                f.write_str(
                    ": the array shape must be sealed and have exactly the record's fields, \
                     in order",
                )
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
            R::ShapeExtraKey { key } => {
                base(f)?;
                write!(f, ": unexpected key `{}`", key_str(key))
            }
            R::ShapeUnsealed => {
                base(f)?;
                write!(f, ": {} may contain extra keys", self.from)
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

/// Mutual assignability (used for invariant positions).
pub fn is_equivalent(env: &dyn TypeEnv, a: &Ty, b: &Ty) -> bool {
    a == b || (is_assignable(env, a, b) && is_assignable(env, b, a))
}

/// The PHP-family view of a StirPHP-family type (shallow): what it is at runtime.
/// Returns `None` for types that are not StirPHP-family.
pub fn php_view(ty: &Ty) -> Option<Ty> {
    Some(match ty.kind() {
        K::Int => Ty::php_int(),
        K::Float => Ty::php_float(),
        K::String => Ty::php_string(),
        K::Bool => Ty::php_bool(),
        K::Unit => Ty::void(),
        K::Nothing => Ty::never(),
        K::Literal(Literal {
            value,
            family: Family::Safe,
        }) => Ty::php_lit(value.clone()),
        K::List(t) | K::MutableList(t) => Ty::php_list(t.clone()),
        K::Map(k, v) | K::MutableMap(k, v) => Ty::array(k.clone(), v.clone()),
        K::Record(r) => record_shape(r),
        K::RecordRef { .. } => Ty::array(Ty::php_string(), Ty::mixed()),
        K::Fn(sig) => Ty::closure(Some(sig.clone())),
        _ => return None,
    })
}

/// The sealed array shape a record lowers to.
pub fn record_shape(r: &RecordTy) -> Ty {
    Ty::shape(
        r.fields
            .iter()
            .map(|f| ShapeField::new(f.name.as_str(), f.ty.clone()))
            .collect(),
        true,
    )
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
    /// Record-reference pairs assumed related while unfolding them (recursive
    /// records are related coinductively).
    assuming: Vec<(Ty, Ty)>,
}

fn lit(t: &Ty) -> Option<&Literal> {
    match t.kind() {
        K::Literal(l) => Some(l),
        _ => None,
    }
}

fn is_int_like(t: &Ty) -> bool {
    matches!(t.kind(), K::PhpInt | K::IntRange { .. })
        || matches!(
            lit(t),
            Some(Literal {
                value: LitValue::Int(_),
                family: Family::Php
            })
        )
}

fn is_float_like(t: &Ty) -> bool {
    matches!(t.kind(), K::PhpFloat)
        || matches!(
            lit(t),
            Some(Literal {
                value: LitValue::Float(_),
                family: Family::Php
            })
        )
}

fn is_string_like(t: &Ty) -> bool {
    matches!(
        t.kind(),
        K::PhpString | K::NonEmptyString | K::NumericString | K::ClassString(_)
    ) || matches!(
        lit(t),
        Some(Literal {
            value: LitValue::String(_),
            family: Family::Php
        })
    )
}

fn is_bool_like(t: &Ty) -> bool {
    matches!(t.kind(), K::PhpBool)
        || matches!(
            lit(t),
            Some(Literal {
                value: LitValue::Bool(_),
                family: Family::Php
            })
        )
}

fn php_str_lit(t: &Ty) -> Option<&str> {
    match lit(t) {
        Some(Literal {
            value: LitValue::String(s),
            family: Family::Php,
        }) => Some(s),
        _ => None,
    }
}

fn shape_key_ty(key: &ShapeKey) -> Ty {
    match key {
        ShapeKey::Int(n) => Ty::php_lit(LitValue::Int(*n)),
        ShapeKey::Str(s) => Ty::php_lit(LitValue::String(s.clone())),
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
            assuming: Vec::new(),
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
        match (from.kind(), to.kind()) {
            (K::Error, _) | (_, K::Error) | (K::Nothing | K::Never, _) => return Ok(()),
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
        if matches!(to.kind(), K::Mixed | K::AnyPlatform) {
            return Ok(());
        }
        // `array-key` is `int|string`
        if matches!(from.kind(), K::ArrayKey)
            && to
                .members()
                .iter()
                .any(|m| matches!(m.kind(), K::Int | K::PhpInt))
            && to
                .members()
                .iter()
                .any(|m| matches!(m.kind(), K::String | K::PhpString))
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
            K::Platform(inner) => {
                if self.ok(inner, to) {
                    return Ok(());
                }
                return self.check(&without_null(inner), to);
            }
            K::AnyPlatform => {
                let reason = if to.members().iter().any(|m| m.is_safe()) {
                    R::AnyPlatformToSafe
                } else {
                    R::Incompatible
                };
                return Err(Mismatch::new(from, to, reason));
            }
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
                // `X -> T?`: explain against `T`.
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

        if let K::Platform(inner) = to.kind() {
            if self.ok(from, inner) {
                return Ok(());
            }
            return self.check(from, &inner.clone().nullable());
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
        let int_src = matches!(from.kind(), K::Int)
            || matches!(
                lit(from),
                Some(Literal {
                    value: LitValue::Int(_),
                    family: Family::Safe
                })
            );
        let reason = if from.is_null() {
            R::NullToNonNull
        } else if int_src && matches!(to.kind(), K::Float) {
            R::NoNumericWidening
        } else if from.is_php() && to.is_safe() {
            R::UnsafeToSafe
        } else if matches!(from.kind(), K::Class { .. }) && matches!(to.kind(), K::Class { .. }) {
            R::NotSubtype
        } else {
            R::Incompatible
        };
        Err(Mismatch::new(from, to, reason))
    }

    fn structural(&mut self, from: &Ty, to: &Ty) -> Res {
        use MismatchReason as R;
        if from.is_safe()
            && !to.is_safe()
            && let Some(view) = php_view(from)
        {
            return self.check(&view, to).map_err(|mut e| {
                if e.from == view {
                    e.from = from.clone();
                }
                e
            });
        }

        let (f, t) = (from.kind(), to.kind());
        match (f, t) {
            // an empty PHP array literal fits any PHP array type
            (
                K::PhpList {
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
                K::PhpList {
                    elem,
                    non_empty: false,
                },
                K::Shape(_),
            ) if matches!(elem.kind(), K::Never) => {
                return Ok(());
            }
            (K::List(a) | K::MutableList(a), K::List(b)) => {
                return self.arg(from, to, 0, Variance::Out, a, b);
            }
            (K::MutableList(a), K::MutableList(b)) => {
                return self.arg(from, to, 0, Variance::Invariant, a, b);
            }
            (K::Map(k1, v1) | K::MutableMap(k1, v1), K::Map(k2, v2)) => {
                self.arg(from, to, 0, Variance::Invariant, k1, k2)?;
                return self.arg(from, to, 1, Variance::Out, v1, v2);
            }
            (K::MutableMap(k1, v1), K::MutableMap(k2, v2)) => {
                self.arg(from, to, 0, Variance::Invariant, k1, k2)?;
                return self.arg(from, to, 1, Variance::Invariant, v1, v2);
            }
            (K::RecordRef { name: n1, args: a1 }, K::RecordRef { name: n2, args: a2 })
                if n1.eq_ignore_ascii_case(n2) && a1 == a2 =>
            {
                return Ok(());
            }
            (K::RecordRef { .. }, _) | (_, K::RecordRef { .. }) => {
                let pair = (from.clone(), to.clone());
                if self.assuming.contains(&pair) {
                    return Ok(());
                }
                let (uf, ut) = (crate::unfold(self.env, from), crate::unfold(self.env, to));
                if &uf == from && &ut == to {
                    return self.fail(from, to);
                }
                self.assuming.push(pair);
                let r = self.check(&uf, &ut);
                self.assuming.pop();
                return r;
            }
            (K::Record(a), K::Record(b)) => return self.record_prefix(from, to, a, b),
            (K::Shape(s), K::Record(r)) => return self.shape_to_record(from, to, s, r),
            (K::Fn(a), K::Fn(b)) => return self.fn_sig(from, to, a, b),
            (K::Class { .. }, K::Class { name: n2, args: a2 }) => {
                return self.class(from, to, n2, a2);
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
                K::PhpList {
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
                // a list's keys fit StirPHP's `Int` as well as PHP's `int`
                let list_key = if matches!(key.kind(), K::Int) {
                    Ty::int()
                } else {
                    Ty::non_negative_int()
                };
                self.arg(from, to, 0, Variance::Out, &list_key, key)?;
                return self.arg(from, to, 1, Variance::Out, elem, value);
            }
            (
                K::PhpList {
                    elem: e1,
                    non_empty: ne1,
                },
                K::PhpList { elem, non_empty },
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
            (K::Shape(s), K::PhpList { elem, non_empty }) => {
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
                K::PhpString | K::PhpList { .. } | K::Array { .. } | K::Shape(_),
                K::Callable { closure: false, .. },
            ) => return Ok(()),
            (K::Literal(l), K::Callable { closure: false, .. })
                if matches!(l.value, LitValue::String(_)) =>
            {
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
            K::Int => matches!(
                lit(from),
                Some(Literal {
                    value: LitValue::Int(_),
                    family: Family::Safe
                })
            ),
            K::Float => matches!(
                lit(from),
                Some(Literal {
                    value: LitValue::Float(_),
                    family: Family::Safe
                })
            ),
            K::String => matches!(
                lit(from),
                Some(Literal {
                    value: LitValue::String(_),
                    family: Family::Safe
                })
            ),
            K::Bool => matches!(
                lit(from),
                Some(Literal {
                    value: LitValue::Bool(_),
                    family: Family::Safe
                })
            ),
            K::PhpInt => is_int_like(from),
            K::PhpFloat => is_int_like(from) || is_float_like(from),
            K::PhpString => is_string_like(from),
            K::PhpBool => is_bool_like(from),
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
                    || php_str_lit(from).is_some_and(is_numeric_str)
            }
            K::ArrayKey => is_int_like(from) || is_string_like(from),
            K::NonEmptyString => {
                matches!(f, K::NumericString | K::ClassString(_))
                    || php_str_lit(from).is_some_and(|s| !s.is_empty())
            }
            K::NumericString => php_str_lit(from).is_some_and(is_numeric_str),
            K::IntRange { min, max } => {
                let (lo, hi) = match f {
                    K::IntRange { min: a, max: b } => (*a, *b),
                    K::Literal(Literal {
                        value: LitValue::Int(n),
                        family: Family::Php,
                    }) => (Some(*n), Some(*n)),
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

    fn record_prefix(&mut self, from: &Ty, to: &Ty, a: &RecordTy, b: &RecordTy) -> Res {
        use MismatchReason as R;
        for (i, bf) in b.fields.iter().enumerate() {
            let Some(af) = a.fields.get(i) else {
                return Err(Mismatch::new(
                    from,
                    to,
                    R::RecordMissingField {
                        name: bf.name.clone(),
                    },
                ));
            };
            if af.name != bf.name {
                let reason = if a.field(&bf.name).is_some() {
                    R::RecordFieldOrder {
                        index: i,
                        expected: bf.name.clone(),
                        found: af.name.clone(),
                    }
                } else {
                    R::RecordMissingField {
                        name: bf.name.clone(),
                    }
                };
                return Err(Mismatch::new(from, to, reason));
            }
            self.check(&af.ty, &bf.ty).map_err(|cause| {
                Mismatch::new(
                    from,
                    to,
                    R::RecordField {
                        name: bf.name.clone(),
                        cause: Box::new(cause),
                    },
                )
            })?;
        }
        Ok(())
    }

    fn shape_to_record(&mut self, from: &Ty, to: &Ty, s: &ShapeTy, r: &RecordTy) -> Res {
        use MismatchReason as R;
        let layout_ok = s.sealed
            && s.fields.len() == r.fields.len()
            && s.fields
                .iter()
                .zip(&r.fields)
                .all(|(sf, rf)| !sf.optional && ShapeKey::from_str_key(&rf.name) == sf.key);
        if !layout_ok {
            return Err(Mismatch::new(from, to, R::ShapeLayout));
        }
        for (sf, rf) in s.fields.iter().zip(&r.fields) {
            self.check(&sf.ty, &rf.ty).map_err(|cause| {
                Mismatch::new(
                    from,
                    to,
                    R::RecordField {
                        name: rf.name.clone(),
                        cause: Box::new(cause),
                    },
                )
            })?;
        }
        Ok(())
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
        if b.sealed {
            if !a.sealed {
                return Err(Mismatch::new(from, to, R::ShapeUnsealed));
            }
            if let Some(extra) = a.fields.iter().find(|af| b.field(&af.key).is_none()) {
                return Err(Mismatch::new(
                    from,
                    to,
                    R::ShapeExtraKey {
                        key: extra.key.clone(),
                    },
                ));
            }
        }
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
                (None, Some(_)) => {
                    return Err(Mismatch::new(from, to, R::FnExtraArgument { index: i }));
                }
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
            let v = self.env.variance(n2, i);
            self.arg(from, to, i, v, x, y)?;
        }
        Ok(())
    }

    /// Key and value types when iterating `from`, if it is iterable.
    fn iter_kv(&self, from: &Ty) -> Option<(Ty, Ty)> {
        Some(match from.kind() {
            K::Array { key, value, .. } | K::Iterable { key, value } => {
                (key.clone(), value.clone())
            }
            K::PhpList { elem, .. } => (Ty::non_negative_int(), elem.clone()),
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
