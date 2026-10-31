//! Type syntax shared by PHP native types and StirPHP types.
//!
//! PHP: `?T`, `A|B`, `A&B`, `(A&B)|null`.
//! StirPHP adds: `T?`, generic arguments `List<Int>`, function types
//! `(Int, String) -> Bool` and literal types `"a" | "b"`.

use super::*;

pub(crate) fn at_type_start(p: &Parser<'_>) -> bool {
    matches!(
        p.current(),
        IDENT
            | NAME_QUALIFIED
            | NAME_FULLY_QUALIFIED
            | NAME_RELATIVE
            | ARRAY_KW
            | CALLABLE_KW
            | STATIC_KW
            | QUESTION
            | L_PAREN
            | STRING_SINGLE
            | STRING_DOUBLE
            | INT_NUMBER
            | FLOAT_NUMBER
    ) || p.current().is_keyword()
        || (p.at(MINUS) && matches!(p.nth(1), INT_NUMBER | FLOAT_NUMBER))
}

pub(crate) fn type_(p: &mut Parser<'_>) {
    p.with_newlines(false, |p| union_type(p));
}

/// A type without top-level unions (used for receivers and supertypes).
pub(crate) fn type_no_union(p: &mut Parser<'_>) {
    p.with_newlines(false, |p| {
        postfix_type(p);
    });
}

fn union_type(p: &mut Parser<'_>) {
    let Some(first) = intersection_type(p) else {
        return;
    };
    if p.at(PIPE) {
        let m = first.precede(p);
        while p.eat(PIPE) {
            intersection_type(p);
        }
        m.complete(p, UNION_TYPE);
    }
}

/// `&` continues an intersection only when a type follows (in a parameter,
/// `Foo &$x` is a by-reference marker).
fn at_intersection_amp(p: &Parser<'_>) -> bool {
    p.at(AMP) && !matches!(p.nth(1), VARIABLE | ELLIPSIS | AMP)
}

fn intersection_type(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    let first = postfix_type(p)?;
    if at_intersection_amp(p) {
        let m = first.precede(p);
        while at_intersection_amp(p) {
            p.bump(AMP);
            postfix_type(p);
        }
        return Some(m.complete(p, INTERSECTION_TYPE));
    }
    Some(first)
}

fn postfix_type(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    let mut ty = prefix_type(p)?;
    // `T?` — but not the elvis `?:`
    while p.at(QUESTION) && !(p.nth_joint(0) && p.nth(1) == COLON) && !p.nth_at(1, QUESTION) {
        let m = ty.precede(p);
        p.bump(QUESTION);
        ty = m.complete(p, NULLABLE_TYPE);
    }
    Some(ty)
}

fn prefix_type(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    match p.current() {
        QUESTION => {
            let m = p.start();
            p.bump(QUESTION);
            postfix_type(p);
            Some(m.complete(p, NULLABLE_TYPE))
        }
        L_PAREN => Some(paren_or_function_type(p)),
        STRING_SINGLE | STRING_DOUBLE | INT_NUMBER | FLOAT_NUMBER => {
            let m = p.start();
            p.bump_any();
            Some(m.complete(p, LITERAL_TYPE))
        }
        MINUS if matches!(p.nth(1), INT_NUMBER | FLOAT_NUMBER) => {
            let m = p.start();
            p.bump(MINUS);
            p.bump_any();
            Some(m.complete(p, LITERAL_TYPE))
        }
        IDENT | NAME_QUALIFIED | NAME_FULLY_QUALIFIED | NAME_RELATIVE | ARRAY_KW | CALLABLE_KW
        | STATIC_KW => {
            let m = p.start();
            name_ref(p);
            if p.at(L_ANGLE) {
                generic_args(p);
            }
            Some(m.complete(p, NAMED_TYPE))
        }
        // Reserved words are fine as type names (`List` is the `list` keyword).
        k if k.is_keyword() => {
            let m = p.start();
            let n = p.start();
            p.bump_remap(IDENT);
            n.complete(p, NAME_REF);
            if p.at(L_ANGLE) {
                generic_args(p);
            }
            Some(m.complete(p, NAMED_TYPE))
        }
        _ => {
            p.error("expected a type");
            None
        }
    }
}

fn generic_args(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(L_ANGLE);
    while !p.at(R_ANGLE) && !p.at(EOF) {
        // `*` is PHPStan's wildcard; accept leniently.
        if p.at(STAR) {
            p.bump(STAR);
        } else {
            if (p.at_contextual("in") || p.at_contextual("out"))
                && matches!(
                    p.nth(1),
                    IDENT | NAME_QUALIFIED | NAME_FULLY_QUALIFIED | L_PAREN | QUESTION
                )
            {
                p.bump_remap(IDENT);
            }
            union_type(p);
        }
        if !p.eat(COMMA) {
            break;
        }
    }
    if p.nth(0) == R_ANGLE {
        // Always take a single `>` here, even when it is glued to another `>`.
        p.bump_remap(R_ANGLE);
    } else {
        p.error("expected `>`");
    }
    m.complete(p, GENERIC_ARG_LIST);
}

/// `(A&B)` (PHP DNF) or `(A, B) -> R` / `() -> R` (StirPHP function type).
fn paren_or_function_type(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    let params = p.start();
    p.bump(L_PAREN);
    let mut count = 0;
    while !p.at(R_PAREN) && !p.at(EOF) {
        p.eat(ELLIPSIS);
        // allow named parameters: `(x: Int) -> Int`
        if (p.current() == IDENT || p.current() == VARIABLE) && p.nth(1) == COLON {
            p.bump_any();
            p.bump(COLON);
        }
        union_type(p);
        p.eat(EQ); // optional-parameter marker (`callable(int=)`)
        count += 1;
        if !p.eat(COMMA) {
            break;
        }
    }
    p.expect(R_PAREN);
    if p.at(ARROW) {
        params.complete(p, FUNCTION_TYPE_PARAMS);
        p.bump(ARROW);
        union_type(p);
        m.complete(p, FUNCTION_TYPE)
    } else {
        if count != 1 {
            p.error("expected `->` after function type parameters");
        }
        params.abandon(p);
        m.complete(p, PAREN_TYPE)
    }
}
