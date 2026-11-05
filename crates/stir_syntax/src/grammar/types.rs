//! PHP native type syntax: `T`, `?T`, `A|B`, `A&B`, `(A&B)|null`.

use super::*;

pub(crate) fn at_type_start(p: &Parser<'_>) -> bool {
    p.at_ts(NAME_TOKENS)
        || matches!(
            p.current(),
            ARRAY_KW | CALLABLE_KW | STATIC_KW | QUESTION | L_PAREN
        )
}

pub(crate) fn type_(p: &mut Parser<'_>) {
    if p.at(QUESTION) {
        let m = p.start();
        p.bump(QUESTION);
        class_type(p);
        m.complete(p, NULLABLE_TYPE);
    } else {
        union_or_intersection(p);
    }
}

/// `A|(B&C)|D` or `A&B`: PHP does not mix the two without parentheses.
fn union_or_intersection(p: &mut Parser<'_>) {
    let Some(first) = atom(p) else {
        return;
    };
    if p.at(PIPE) {
        let m = first.precede(p);
        while p.eat(PIPE) {
            atom(p);
        }
        m.complete(p, UNION_TYPE);
    } else if at_intersection_amp(p) {
        intersection_rest(p, first);
    }
}

/// `&` continues an intersection only when a type follows (in a parameter,
/// `Foo &$x` is a by-reference marker).
fn at_intersection_amp(p: &Parser<'_>) -> bool {
    p.at(AMP) && !matches!(p.nth(1), VARIABLE | ELLIPSIS | AMP)
}

fn intersection_rest(p: &mut Parser<'_>, first: CompletedMarker) {
    let m = first.precede(p);
    while at_intersection_amp(p) {
        p.bump(AMP);
        class_type(p);
    }
    m.complete(p, INTERSECTION_TYPE);
}

fn atom(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    if p.at(L_PAREN) {
        let m = p.start();
        p.bump(L_PAREN);
        if let Some(first) = class_type(p) {
            intersection_rest(p, first);
        }
        p.expect(R_PAREN);
        Some(m.complete(p, PAREN_TYPE))
    } else {
        class_type(p)
    }
}

/// A single named type: a class name or a reserved type name.
pub(crate) fn class_type(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    if p.at_ts(NAME_TOKENS) || matches!(p.current(), ARRAY_KW | CALLABLE_KW | STATIC_KW) {
        let m = p.start();
        name_ref(p);
        Some(m.complete(p, NAMED_TYPE))
    } else {
        p.error("expected a type");
        None
    }
}
