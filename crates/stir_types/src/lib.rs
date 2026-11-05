//! `stir_types`: the type core.
//!
//! This crate defines the type representation used by the checker, and the relations between
//! types:
//!
//! * [`Ty`] / [`TyKind`]: PHP's native types (`int`, `array`, `callable`, ...), PHPStan's
//!   (`array<K, V>`, array shapes, refinements, literals, ...), unions, intersections, classes,
//!   type parameters and untyped values. See [`ty`] for the representation and normalisation.
//! * [`TypeEnv`]: what the relations need to know about classes and type parameters;
//!   implemented by the checker.
//! * [`is_assignable`] / [`check_assignable`] (with a structured [`Mismatch`]),
//!   [`are_comparable`].
//! * operations: [`subst`], [`free_params`], [`join`], [`meet`], [`without_null`],
//!   [`is_nullable`], [`narrow_to`], [`narrow_away`].
//! * rendering: `Display`, in PHP type syntax.
//!
//! This is a leaf crate: it knows nothing about syntax, files or salsa.

pub mod assign;
pub mod compare;
pub mod display;
pub mod env;
pub mod ops;
pub mod ty;

pub use assign::{
    Mismatch, MismatchReason, assignable_ignoring_mixed, check_assignable, is_assignable,
    is_equivalent, maybe_assignable,
};
pub use compare::are_comparable;
pub use env::{EmptyEnv, TypeEnv, Variance};
pub use ops::{
    free_params, is_nullable, join, meet, narrow_away, narrow_falsy, narrow_to, narrow_truthy,
    strip_nulls, subst, without_null,
};
pub use ty::{
    ConditionalTy, FloatLit, FnParam, FnTy, LitValue, ParamId, ParamKind, SelfKind, ShapeField,
    ShapeKey, ShapeTy, Ty, TyKind, TyList,
};

/// Re-exported so callers can build substitution maps without depending on `rustc-hash`.
pub use rustc_hash::FxHashMap;
