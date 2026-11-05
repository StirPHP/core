//! The environment the type relations query: class hierarchy, variance, bounds.

use crate::ty::{FnTy, ParamId, Ty};

/// Declared variance of a type parameter.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Variance {
    /// `@template-covariant`.
    Out,
    /// `@template-contravariant`.
    In,
    /// Invariant (`@template`).
    Invariant,
}

/// Class-hierarchy and type-parameter information, implemented by the checker.
///
/// Class names are FQCNs without a leading `\`, in their canonical (declared) spelling.
/// Only [`supertypes`](TypeEnv::supertypes) is required; every other method has a
/// conservative default.
pub trait TypeEnv {
    /// The *direct* supertypes (parent class and implemented interfaces) of `class`
    /// instantiated with `args`, with the class's type parameters substituted by `args`
    /// (see [`subst`](crate::subst)). Each returned type should be a `Class` type.
    /// Unknown classes return an empty list.
    fn supertypes(&self, class: &str, args: &[Ty]) -> Vec<Ty>;

    /// Declared variance of the `index`-th type parameter of `class`.
    fn variance(&self, class: &str, index: usize) -> Variance {
        let _ = (class, index);
        Variance::Invariant
    }

    /// Upper bound of a type parameter (`T of Bound`), or `None` if unbounded (an unbounded
    /// parameter is only assignable to itself and `mixed`).
    fn param_bound(&self, param: ParamId) -> Option<Ty> {
        let _ = param;
        None
    }

    /// Whether `class` is final (cannot be subclassed). Enums are final.
    fn is_final(&self, class: &str) -> bool {
        let _ = class;
        false
    }

    /// Whether `class` is an interface.
    fn is_interface(&self, class: &str) -> bool {
        let _ = class;
        false
    }

    /// The `__invoke` signature of `class`, if instances are callable.
    fn invoke_signature(&self, class: &str, args: &[Ty]) -> Option<FnTy> {
        let _ = (class, args);
        None
    }
}

/// An environment with no classes; handy for relations that do not involve classes.
#[derive(Clone, Copy, Default, Debug)]
pub struct EmptyEnv;

impl TypeEnv for EmptyEnv {
    fn supertypes(&self, _class: &str, _args: &[Ty]) -> Vec<Ty> {
        Vec::new()
    }
}
