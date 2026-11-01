//! `stir_types`: the StirPHP type core.
//!
//! This crate defines the type representation shared by the checker and the transpiler,
//! and the relations between types:
//!
//! * [`Ty`] / [`TyKind`]: the StirPHP ("safe") family (`Int`, `String`, `List<T>`, records,
//!   function types, ...), the PHP ("unsafe") family (`int`, `array<K, V>`, array shapes,
//!   PHPStan refinements, ...), literals, unions, intersections, classes, type parameters
//!   and platform types (`T!`, `Any!`). See [`ty`] for the representation and normalisation.
//! * [`TypeEnv`]: what the relations need to know about classes and type parameters;
//!   implemented by the checker.
//! * [`is_assignable`] / [`check_assignable`] (with a structured [`Mismatch`]),
//!   [`are_comparable`], [`top_type_violation`].
//! * operations: [`subst`], [`free_params`], [`join`], [`meet`], [`without_null`],
//!   [`is_nullable`], [`narrow_to`], [`narrow_away`].
//! * rendering: `Display` for diagnostics and [`to_phpdoc`] for emitted PHPDoc.
//!
//! This is a leaf crate: it knows nothing about syntax, files or salsa.

pub mod assign;
pub mod compare;
pub mod display;
pub mod env;
pub mod ops;
pub mod top;
pub mod ty;

pub use assign::{
    Mismatch, MismatchReason, check_assignable, is_assignable, is_equivalent, php_view,
    record_shape,
};
pub use compare::are_comparable;
pub use display::to_phpdoc;
pub use env::{EmptyEnv, TypeEnv, Variance};
pub use ops::{free_params, is_nullable, join, meet, narrow_away, narrow_to, subst, without_null};
pub use top::{TopKind, TopViolation, top_type_violation};
pub use ty::{
    Family, FloatLit, FnParam, FnTy, LitValue, Literal, ParamId, ParamKind, RecordField, RecordTy,
    SelfKind, ShapeField, ShapeKey, ShapeTy, Ty, TyKind, TyList,
};

/// Re-exported so callers can build substitution maps without depending on `rustc-hash`.
pub use rustc_hash::FxHashMap;
