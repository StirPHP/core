//! The "no top type in `.sphp`" rule.

use crate::ty::{Ty, TyKind as K};

/// How a top type occurs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TopKind {
    /// An explicit `mixed` (the checker should lower `Any` / `Any?` to `mixed` too, or
    /// report them directly).
    Mixed,
    /// A PHP type that implicitly contains `mixed`: untyped `array`, `iterable` or
    /// `callable`. The checker decides whether these are reported in `.sphp`.
    ImplicitMixed,
}

/// An occurrence of a top type, see [`top_type_violation`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TopViolation {
    /// The offending sub-type (`mixed`, `array`, ...).
    pub ty: Ty,
    pub kind: TopKind,
}

/// Finds the first (pre-order) occurrence of a top type in a type written in `.sphp`.
///
/// `Any!` is *not* reported (it is allowed in locals; the checker enforces where), and the
/// contents of platform types `T!` are not inspected (they describe external PHP values).
pub fn top_type_violation(ty: &Ty) -> Option<TopViolation> {
    let mut found = None;
    ty.walk(&mut |t| {
        if found.is_some() {
            return false;
        }
        let kind = match t.kind() {
            K::Mixed => Some(TopKind::Mixed),
            K::Array { key, value, .. }
                if matches!(key.kind(), K::ArrayKey) && matches!(value.kind(), K::Mixed) =>
            {
                Some(TopKind::ImplicitMixed)
            }
            K::Iterable { key, value }
                if matches!(key.kind(), K::Mixed) && matches!(value.kind(), K::Mixed) =>
            {
                Some(TopKind::ImplicitMixed)
            }
            K::Callable { sig: None, .. } => Some(TopKind::ImplicitMixed),
            K::Platform(_) => return false,
            _ => None,
        };
        if let Some(kind) = kind {
            found = Some(TopViolation {
                ty: t.clone(),
                kind,
            });
            return false;
        }
        true
    });
    found
}
