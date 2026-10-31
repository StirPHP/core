//! Expressions: a Pratt parser over PHP 8 precedence, extended with the
//! StirPHP operators (`is`, `as`, elvis `?:`, `!!`, lambdas, `when`, `if`
//! expressions and record literals).

use super::*;

/// Binding powers (left). Right-assoc operators use `rbp == lbp`,
/// left-assoc `rbp == lbp + 1`.
mod bp {
    pub const OR: u8 = 2;
    pub const XOR: u8 = 4;
    pub const AND: u8 = 6;
    pub const ASSIGN: u8 = 8;
    pub const TERNARY: u8 = 10;
    pub const COALESCE: u8 = 12;
    pub const LOR: u8 = 14;
    pub const LAND: u8 = 16;
    pub const BOR: u8 = 18;
    pub const BXOR: u8 = 20;
    pub const BAND: u8 = 22;
    pub const EQUALITY: u8 = 24;
    pub const COMPARISON: u8 = 26;
    /// StirPHP `is` / `!is`
    pub const IS: u8 = 27;
    /// StirPHP elvis `?:`
    pub const ELVIS: u8 = 28;
    /// PHP 8.5 pipe `|>`
    pub const PIPE: u8 = 29;
    pub const CONCAT: u8 = 30;
    pub const SHIFT: u8 = 32;
    pub const ADDITIVE: u8 = 34;
    pub const MULTIPLICATIVE: u8 = 36;
    /// StirPHP `as`
    pub const AS: u8 = 37;
    pub const INSTANCEOF: u8 = 38;
    /// Operand of `!`
    pub const NOT: u8 = 38;
    /// Operand of `-`, `+`, `~`, casts, `@`, `++`/`--`
    pub const UNARY: u8 = 40;
    pub const POW: u8 = 42;
    pub const POSTFIX: u8 = 44;
}

/// A required expression: reports an error if none is present.
pub(crate) fn expr(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    operand(p, 0)
}

/// An optional expression: the caller handles absence.
pub(crate) fn opt_expr(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    expr_bp(p, 0)
}

fn operand(p: &mut Parser<'_>, min_bp: u8) -> Option<CompletedMarker> {
    let r = expr_bp(p, min_bp);
    if r.is_none() {
        p.error("expected an expression");
    }
    r
}

const ASSIGN_OPS: TokenSet = TokenSet::new(&[
    EQ,
    PLUS_EQ,
    MINUS_EQ,
    STAR_EQ,
    SLASH_EQ,
    PERCENT_EQ,
    POW_EQ,
    DOT_EQ,
    AMP_EQ,
    PIPE_EQ,
    CARET_EQ,
    SHL_EQ,
    QUESTION2_EQ,
]);

fn is_assignable(kind: SyntaxKind, stir: bool) -> bool {
    matches!(
        kind,
        VARIABLE_EXPR
            | VARIABLE_VARIABLE
            | INDEX_EXPR
            | PROPERTY_FETCH_EXPR
            | NULLSAFE_PROPERTY_FETCH_EXPR
            | STATIC_PROPERTY_FETCH_EXPR
            | ARRAY_EXPR
            | LIST_EXPR
    ) || (stir && kind == NAME_EXPR)
        // PHP parses `f() = 1` and rejects it at compile time; so do we (in the checker).
        || (!stir
            && matches!(
                kind,
                CALL_EXPR | METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR
            ))
}

/// Infix operator at the current position: (kind-to-bump, lbp, rbp).
fn infix_op(p: &Parser<'_>) -> Option<(SyntaxKind, u8, u8)> {
    let stir = p.stir();
    let op = match p.current() {
        OR_KW => (OR_KW, bp::OR, bp::OR + 1),
        XOR_KW => (XOR_KW, bp::XOR, bp::XOR + 1),
        AND_KW => (AND_KW, bp::AND, bp::AND + 1),
        QUESTION2 => (QUESTION2, bp::COALESCE, bp::COALESCE),
        PIPE2 => (PIPE2, bp::LOR, bp::LOR + 1),
        AMP2 => (AMP2, bp::LAND, bp::LAND + 1),
        PIPE => (PIPE, bp::BOR, bp::BOR + 1),
        CARET => (CARET, bp::BXOR, bp::BXOR + 1),
        AMP => (AMP, bp::BAND, bp::BAND + 1),
        EQ2 | EQ3 | NEQ | NEQ2 | SPACESHIP => (p.current(), bp::EQUALITY, bp::EQUALITY + 1),
        L_ANGLE | LTEQ => (p.current(), bp::COMPARISON, bp::COMPARISON + 1),
        R_ANGLE => {
            if p.at(GTEQ) {
                (GTEQ, bp::COMPARISON, bp::COMPARISON + 1)
            } else if p.at(SHR) {
                (SHR, bp::SHIFT, bp::SHIFT + 1)
            } else if p.at(SHR_EQ) {
                return None;
            } else {
                (R_ANGLE, bp::COMPARISON, bp::COMPARISON + 1)
            }
        }
        PIPE_GT => (PIPE_GT, bp::PIPE, bp::PIPE + 1),
        DOT => (DOT, bp::CONCAT, bp::CONCAT + 1),
        SHL => (SHL, bp::SHIFT, bp::SHIFT + 1),
        PLUS | MINUS => (p.current(), bp::ADDITIVE, bp::ADDITIVE + 1),
        STAR | SLASH | PERCENT => (p.current(), bp::MULTIPLICATIVE, bp::MULTIPLICATIVE + 1),
        POW => (POW, bp::POW, bp::POW),
        _ if stir && p.at_contextual("is") => (IDENT, bp::IS, 0),
        _ if stir && p.at(BANG) && p.nth_joint(0) && p.nth_at_contextual(1, "is") => {
            (BANG, bp::IS, 0)
        }
        _ => return None,
    };
    Some(op)
}

/// Tokens that may start a continuation line in StirPHP.
const CONTINUATION: TokenSet = TokenSet::new(&[
    ARROW,
    NULLSAFE_ARROW,
    DOT,
    AMP2,
    PIPE2,
    QUESTION2,
    COLON2,
    PIPE_GT,
]);

fn newline_breaks(p: &Parser<'_>) -> bool {
    if !p.newline_terminates() {
        return false;
    }
    if p.at_ts(CONTINUATION) {
        return false;
    }
    // elvis on a continuation line
    if p.at(QUESTION) && p.nth(1) == COLON && p.nth_joint(0) {
        return false;
    }
    true
}

pub(crate) fn expr_bp(p: &mut Parser<'_>, min_bp: u8) -> Option<CompletedMarker> {
    let mut lhs = unary(p)?;
    loop {
        if newline_breaks(p) {
            break;
        }
        // Assignment binds to any assignable left-hand side regardless of
        // precedence (`!$a = f()` is `!($a = f())` in PHP).
        if (p.at_ts(ASSIGN_OPS) || p.at(SHR_EQ)) && is_assignable(lhs.kind(), p.stir()) {
            let m = lhs.precede(p);
            if p.at(SHR_EQ) {
                p.bump(SHR_EQ);
            } else {
                let by_ref = p.at(EQ) && p.nth(1) == AMP;
                p.bump_any();
                if by_ref {
                    p.bump(AMP);
                }
            }
            operand(p, bp::ASSIGN);
            lhs = m.complete(p, ASSIGN_EXPR);
            continue;
        }

        // ternary / short ternary / elvis
        if p.at(QUESTION) {
            let elvis = p.nth(1) == COLON;
            if elvis {
                let (lbp, rbp) = if p.stir() {
                    (bp::ELVIS, bp::ELVIS + 1)
                } else {
                    (bp::TERNARY, bp::TERNARY + 1)
                };
                if lbp < min_bp {
                    break;
                }
                let m = lhs.precede(p);
                p.bump(QUESTION);
                p.bump(COLON);
                operand(p, rbp);
                lhs = m.complete(p, ELVIS_EXPR);
                continue;
            }
            if bp::TERNARY < min_bp {
                break;
            }
            let m = lhs.precede(p);
            p.bump(QUESTION);
            p.with_newlines(false, |p| expr_bp(p, 0));
            p.expect(COLON);
            operand(p, bp::TERNARY + 1);
            lhs = m.complete(p, TERNARY_EXPR);
            continue;
        }

        if p.at(INSTANCEOF_KW) {
            if bp::INSTANCEOF < min_bp {
                break;
            }
            let m = lhs.precede(p);
            p.bump(INSTANCEOF_KW);
            class_reference(p);
            lhs = m.complete(p, INSTANCEOF_EXPR);
            continue;
        }

        if p.stir() && p.at(AS_KW) {
            if bp::AS < min_bp {
                break;
            }
            let m = lhs.precede(p);
            p.bump(AS_KW);
            if p.at(QUESTION) && p.nth_joint(0) {
                p.bump(QUESTION); // `as?` safe cast
            }
            types::type_no_union(p);
            lhs = m.complete(p, AS_EXPR);
            continue;
        }

        let Some((op, lbp, rbp)) = infix_op(p) else {
            break;
        };
        if lbp < min_bp {
            break;
        }
        let m = lhs.precede(p);
        if op == IDENT || op == BANG {
            // `is` / `!is` take a type on the right
            if op == BANG {
                p.bump(BANG);
            }
            p.bump_remap(IDENT);
            types::type_(p);
            lhs = m.complete(p, IS_EXPR);
            continue;
        }
        p.bump(op);
        operand(p, rbp);
        lhs = m.complete(p, BIN_EXPR);
    }
    Some(lhs)
}

fn unary(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    let m;
    match p.current() {
        BANG => {
            m = p.start();
            p.bump(BANG);
            operand(p, bp::NOT);
            Some(m.complete(p, PREFIX_EXPR))
        }
        MINUS | PLUS | TILDE => {
            m = p.start();
            p.bump_any();
            operand(p, bp::UNARY);
            Some(m.complete(p, PREFIX_EXPR))
        }
        INC | DEC => {
            m = p.start();
            p.bump_any();
            operand(p, bp::UNARY);
            Some(m.complete(p, PREFIX_EXPR))
        }
        AT => {
            m = p.start();
            p.bump(AT);
            operand(p, bp::UNARY);
            Some(m.complete(p, ERROR_SUPPRESS_EXPR))
        }
        AMP => {
            // by-reference in array elements / arguments: `[&$a]`
            m = p.start();
            p.bump(AMP);
            operand(p, bp::UNARY);
            Some(m.complete(p, PREFIX_EXPR))
        }
        INT_CAST | FLOAT_CAST | STRING_CAST | BOOL_CAST | ARRAY_CAST | OBJECT_CAST | UNSET_CAST
        | VOID_CAST => {
            m = p.start();
            p.bump_any();
            operand(p, bp::UNARY);
            Some(m.complete(p, CAST_EXPR))
        }
        CLONE_KW => {
            m = p.start();
            p.bump(CLONE_KW);
            if p.at(L_PAREN) {
                // PHP 8.5 `clone($obj, ['prop' => v])`; also covers `clone ($x)`.
                arg_list(p);
            } else {
                operand(p, bp::POSTFIX - 1);
            }
            Some(m.complete(p, CLONE_EXPR))
        }
        PRINT_KW => {
            m = p.start();
            p.bump(PRINT_KW);
            operand(p, bp::ASSIGN);
            Some(m.complete(p, PRINT_EXPR))
        }
        YIELD_KW => {
            m = p.start();
            p.bump(YIELD_KW);
            if p.at_contextual("from") || p.nth_text(0).eq_ignore_ascii_case("from") && p.at(IDENT)
            {
                p.bump_remap(IDENT);
                operand(p, bp::ASSIGN);
                return Some(m.complete(p, YIELD_FROM_EXPR));
            }
            if !matches!(
                p.current(),
                SEMI | R_PAREN | COMMA | R_BRACK | EOF | CLOSE_TAG
            ) && !p.newline_terminates()
            {
                operand(p, bp::ASSIGN);
                if p.eat(FAT_ARROW) {
                    operand(p, bp::ASSIGN);
                }
            }
            Some(m.complete(p, YIELD_EXPR))
        }
        THROW_KW => {
            m = p.start();
            p.bump(THROW_KW);
            operand(p, 0);
            Some(m.complete(p, THROW_EXPR))
        }
        INCLUDE_KW | INCLUDE_ONCE_KW | REQUIRE_KW | REQUIRE_ONCE_KW => {
            m = p.start();
            p.bump_any();
            operand(p, bp::ASSIGN);
            Some(m.complete(p, INCLUDE_EXPR))
        }
        _ => postfix(p),
    }
}

fn postfix(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    let mut lhs = primary(p)?;
    loop {
        if newline_breaks(p) {
            break;
        }
        lhs = match p.current() {
            L_PAREN => {
                let m = lhs.precede(p);
                arg_list(p);
                trailing_lambda(p);
                let kind = match lhs.kind() {
                    PROPERTY_FETCH_EXPR => METHOD_CALL_EXPR,
                    NULLSAFE_PROPERTY_FETCH_EXPR => NULLSAFE_METHOD_CALL_EXPR,
                    STATIC_PROPERTY_FETCH_EXPR | CLASS_CONST_FETCH_EXPR => STATIC_CALL_EXPR,
                    _ => CALL_EXPR,
                };
                // Re-tag the callee wrapper: a call node wraps the fetch.
                m.complete(p, kind)
            }
            L_BRACK => {
                let m = lhs.precede(p);
                p.bump(L_BRACK);
                p.with_newlines(false, |p| {
                    if !p.at(R_BRACK) {
                        expr(p);
                    }
                });
                p.expect(R_BRACK);
                m.complete(p, INDEX_EXPR)
            }
            L_CURLY if p.stir() => {
                if p.newline_before() {
                    break;
                }
                if lhs.kind() == NAME_EXPR && at_record_literal_body(p) {
                    let m = lhs.precede(p);
                    record_literal_body(p);
                    m.complete(p, RECORD_LITERAL)
                } else if matches!(
                    lhs.kind(),
                    NAME_EXPR | PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR
                ) {
                    let m = lhs.precede(p);
                    let a = p.start();
                    lambda(p);
                    a.complete(p, ARG_LIST);
                    let kind = match lhs.kind() {
                        PROPERTY_FETCH_EXPR => METHOD_CALL_EXPR,
                        NULLSAFE_PROPERTY_FETCH_EXPR => NULLSAFE_METHOD_CALL_EXPR,
                        _ => CALL_EXPR,
                    };
                    m.complete(p, kind)
                } else {
                    break;
                }
            }
            ARROW if p.at_spaced_arrow_stop() => break,
            ARROW | NULLSAFE_ARROW => {
                let nullsafe = p.at(NULLSAFE_ARROW);
                let m = lhs.precede(p);
                p.bump_any();
                member_selector(p);
                m.complete(
                    p,
                    if nullsafe {
                        NULLSAFE_PROPERTY_FETCH_EXPR
                    } else {
                        PROPERTY_FETCH_EXPR
                    },
                )
            }
            COLON2 => {
                let m = lhs.precede(p);
                p.bump(COLON2);
                match p.current() {
                    VARIABLE => {
                        p.bump(VARIABLE);
                        m.complete(p, STATIC_PROPERTY_FETCH_EXPR)
                    }
                    DOLLAR => {
                        simple_variable(p);
                        m.complete(p, STATIC_PROPERTY_FETCH_EXPR)
                    }
                    L_CURLY => {
                        p.bump(L_CURLY);
                        p.with_newlines(false, |p| expr(p));
                        p.expect(R_CURLY);
                        m.complete(p, CLASS_CONST_FETCH_EXPR)
                    }
                    _ if at_member_name(p) => {
                        member_name(p);
                        m.complete(p, CLASS_CONST_FETCH_EXPR)
                    }
                    _ => {
                        p.error("expected a member name");
                        m.complete(p, CLASS_CONST_FETCH_EXPR)
                    }
                }
            }
            INC | DEC => {
                let m = lhs.precede(p);
                p.bump_any();
                m.complete(p, POSTFIX_EXPR)
            }
            BANG if p.stir()
                && p.nth(1) == BANG
                && p.nth_joint(0)
                && !p.nth_at_contextual(2, "is") =>
            {
                let m = lhs.precede(p);
                p.bump(BANG);
                p.bump(BANG);
                m.complete(p, NOT_NULL_EXPR)
            }
            _ => break,
        };
    }
    Some(lhs)
}

/// After `->`: a name, `$var`, or `{expr}`.
fn member_selector(p: &mut Parser<'_>) {
    match p.current() {
        VARIABLE => {
            let m = p.start();
            p.bump(VARIABLE);
            m.complete(p, VARIABLE_EXPR);
        }
        L_CURLY => {
            p.bump(L_CURLY);
            p.with_newlines(false, |p| expr(p));
            p.expect(R_CURLY);
        }
        DOLLAR => {
            simple_variable(p);
        }
        _ if at_member_name(p) => {
            member_name(p);
        }
        _ => p.error("expected a member name"),
    }
}

/// `Name { ident = ...` starts a record literal (rather than a trailing lambda).
fn at_record_literal_body(p: &Parser<'_>) -> bool {
    p.at(L_CURLY) && at_member_name_n(p, 1) && p.nth(2) == EQ && p.nth(3) != EQ
}

fn at_member_name_n(p: &Parser<'_>, n: usize) -> bool {
    p.nth(n) == IDENT || p.nth(n).is_keyword()
}

fn record_literal_body(p: &mut Parser<'_>) {
    p.bump(L_CURLY);
    p.with_newlines(false, |p| {
        while !p.at(R_CURLY) && !p.at(EOF) {
            let f = p.start();
            member_name(p);
            p.expect(EQ);
            expr(p);
            f.complete(p, RECORD_LITERAL_FIELD);
            if !p.eat(COMMA) {
                break;
            }
        }
    });
    p.expect(R_CURLY);
}

fn trailing_lambda(p: &mut Parser<'_>) {
    if p.stir() && p.at(L_CURLY) && !p.newline_before() {
        let a = p.start();
        lambda(p);
        a.complete(p, ARG);
    }
}

/// A plain variable (`$a`, `$$a`, `${expr}`), used by `global` and `::$x`.
pub(super) fn simple_variable(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    match p.current() {
        VARIABLE => {
            let m = p.start();
            p.bump(VARIABLE);
            Some(m.complete(p, VARIABLE_EXPR))
        }
        DOLLAR => {
            let m = p.start();
            p.bump(DOLLAR);
            if p.at(L_CURLY) {
                p.bump(L_CURLY);
                p.with_newlines(false, |p| expr(p));
                p.expect(R_CURLY);
            } else {
                simple_variable(p);
            }
            Some(m.complete(p, VARIABLE_VARIABLE))
        }
        _ => {
            p.error("expected a variable");
            None
        }
    }
}

/// Right side of `instanceof` / target of `new`: a class name, `static`,
/// a variable with property fetches, or `(expr)`.
fn class_reference(p: &mut Parser<'_>) {
    match p.current() {
        VARIABLE | DOLLAR => {
            let Some(mut lhs) = simple_variable(p) else {
                return;
            };
            loop {
                match p.current() {
                    ARROW | NULLSAFE_ARROW => {
                        let nullsafe = p.at(NULLSAFE_ARROW);
                        let m = lhs.precede(p);
                        p.bump_any();
                        member_selector(p);
                        lhs = m.complete(
                            p,
                            if nullsafe {
                                NULLSAFE_PROPERTY_FETCH_EXPR
                            } else {
                                PROPERTY_FETCH_EXPR
                            },
                        );
                    }
                    COLON2 if matches!(p.nth(1), VARIABLE | DOLLAR) => {
                        let m = lhs.precede(p);
                        p.bump(COLON2);
                        simple_variable(p);
                        lhs = m.complete(p, STATIC_PROPERTY_FETCH_EXPR);
                    }
                    L_BRACK => {
                        let m = lhs.precede(p);
                        p.bump(L_BRACK);
                        expr(p);
                        p.expect(R_BRACK);
                        lhs = m.complete(p, INDEX_EXPR);
                    }
                    _ => break,
                }
            }
        }
        L_PAREN => {
            let m = p.start();
            p.bump(L_PAREN);
            p.with_newlines(false, |p| expr(p));
            p.expect(R_PAREN);
            m.complete(p, PAREN_EXPR);
        }
        _ => {
            let m = p.start();
            name_ref(p);
            m.complete(p, NAME_EXPR);
        }
    }
}

fn primary(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    let stir = p.stir();
    let cm = match p.current() {
        VARIABLE => {
            let m = p.start();
            p.bump(VARIABLE);
            m.complete(p, VARIABLE_EXPR)
        }
        DOLLAR => {
            let m = p.start();
            p.bump(DOLLAR);
            if p.at(L_CURLY) {
                p.bump(L_CURLY);
                p.with_newlines(false, |p| expr(p));
                p.expect(R_CURLY);
            } else {
                simple_variable(p);
            }
            m.complete(p, VARIABLE_VARIABLE)
        }
        INT_NUMBER | FLOAT_NUMBER => {
            let m = p.start();
            p.bump_any();
            m.complete(p, LITERAL)
        }
        STRING_SINGLE | STRING_DOUBLE => {
            let m = p.start();
            p.bump_any();
            m.complete(p, STRING_LITERAL)
        }
        DQUOTE => interp_string(p, DQUOTE, INTERP_STRING),
        BACKTICK => interp_string(p, BACKTICK, SHELL_EXEC),
        HEREDOC_START => heredoc(p),
        CLASS_C_KW | DIR_KW | FILE_KW | FUNCTION_C_KW | LINE_KW | METHOD_C_KW | NAMESPACE_C_KW
        | TRAIT_C_KW | PROPERTY_C_KW => {
            let m = p.start();
            p.bump_any();
            m.complete(p, MAGIC_CONST)
        }
        L_PAREN => {
            let m = p.start();
            p.bump(L_PAREN);
            p.with_newlines(false, |p| expr(p));
            p.expect(R_PAREN);
            m.complete(p, PAREN_EXPR)
        }
        L_BRACK => array_literal(p, L_BRACK, R_BRACK, ARRAY_EXPR),
        ARRAY_KW if p.nth(1) == L_PAREN => {
            let m = p.start();
            p.bump(ARRAY_KW);
            array_body(p, L_PAREN, R_PAREN);
            m.complete(p, ARRAY_EXPR)
        }
        LIST_KW if p.nth(1) == L_PAREN => {
            let m = p.start();
            p.bump(LIST_KW);
            array_body(p, L_PAREN, R_PAREN);
            m.complete(p, LIST_EXPR)
        }
        ISSET_KW => {
            let m = p.start();
            p.bump(ISSET_KW);
            arg_list(p);
            m.complete(p, ISSET_EXPR)
        }
        EMPTY_KW => {
            let m = p.start();
            p.bump(EMPTY_KW);
            arg_list(p);
            m.complete(p, EMPTY_EXPR)
        }
        EVAL_KW => {
            let m = p.start();
            p.bump(EVAL_KW);
            arg_list(p);
            m.complete(p, EVAL_EXPR)
        }
        EXIT_KW => {
            let m = p.start();
            p.bump(EXIT_KW);
            if p.at(L_PAREN) {
                arg_list(p);
            }
            m.complete(p, EXIT_EXPR)
        }
        NEW_KW => new_expr(p),
        MATCH_KW if p.nth(1) == L_PAREN => match_expr(p),
        FUNCTION_KW => closure(p, None),
        FN_KW => arrow_fn(p, None),
        STATIC_KW if matches!(p.nth(1), FUNCTION_KW | FN_KW) => {
            let m = p.start();
            p.bump(STATIC_KW);
            if p.at(FUNCTION_KW) {
                closure(p, Some(m))
            } else {
                arrow_fn(p, Some(m))
            }
        }
        ATTR_OPEN => {
            let m = p.start();
            attributes(p);
            match p.current() {
                FUNCTION_KW => closure(p, Some(m)),
                FN_KW => arrow_fn(p, Some(m)),
                STATIC_KW => {
                    p.bump(STATIC_KW);
                    if p.at(FUNCTION_KW) {
                        closure(p, Some(m))
                    } else {
                        arrow_fn(p, Some(m))
                    }
                }
                _ => {
                    p.error("expected a closure after attributes");
                    m.complete(p, ERROR)
                }
            }
        }
        L_CURLY if stir => lambda(p),
        IF_KW if stir => if_expr(p),
        IDENT if stir && p.at_contextual("when") && matches!(p.nth(1), L_PAREN | L_CURLY) => {
            when_expr(p)
        }
        IDENT | NAME_QUALIFIED | NAME_FULLY_QUALIFIED | NAME_RELATIVE | STATIC_KW => {
            let m = p.start();
            name_ref(p);
            m.complete(p, NAME_EXPR)
        }
        k if k.is_keyword()
            && p.nth(1) == L_PAREN
            && !matches!(
                k,
                IF_KW | WHILE_KW | FOR_KW | FOREACH_KW | SWITCH_KW | CATCH_KW
            ) =>
        {
            // A reserved word used as a function name (`list`, `array` handled above).
            let m = p.start();
            let n = p.start();
            p.bump_remap(IDENT);
            n.complete(p, NAME_REF);
            m.complete(p, NAME_EXPR)
        }
        _ => return None,
    };
    Some(cm)
}

fn array_literal(
    p: &mut Parser<'_>,
    open: SyntaxKind,
    close: SyntaxKind,
    kind: SyntaxKind,
) -> CompletedMarker {
    let m = p.start();
    array_body(p, open, close);
    m.complete(p, kind)
}

fn array_body(p: &mut Parser<'_>, open: SyntaxKind, close: SyntaxKind) {
    p.bump(open);
    p.with_newlines(false, |p| {
        while !p.at(close) && !p.at(EOF) {
            let e = p.start();
            if p.at(COMMA) {
                // skipped element in destructuring: `[, $b]`
                e.complete(p, ARRAY_ELEM);
                p.bump(COMMA);
                continue;
            }
            if p.eat(ELLIPSIS) {
                expr(p);
            } else {
                if opt_expr(p).is_none() {
                    p.err_recover(
                        "expected an array element",
                        TokenSet::new(&[COMMA, R_BRACK, R_PAREN]),
                    );
                }
                if p.eat(FAT_ARROW) {
                    expr(p);
                }
            }
            e.complete(p, ARRAY_ELEM);
            if !p.eat(COMMA) {
                break;
            }
        }
        p.expect(close);
    });
}

fn new_expr(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump(NEW_KW);
    if p.at(CLASS_KW) || (p.at(ATTR_OPEN)) || (p.at(READONLY_KW) && p.nth(1) == CLASS_KW) {
        let c = p.start();
        attributes(p);
        p.eat(READONLY_KW);
        p.bump(CLASS_KW);
        if p.at(L_PAREN) {
            arg_list(p);
        }
        if p.at(EXTENDS_KW) {
            let e = p.start();
            p.bump(EXTENDS_KW);
            name_ref(p);
            e.complete(p, EXTENDS_CLAUSE);
        }
        if p.at(IMPLEMENTS_KW) {
            let e = p.start();
            p.bump(IMPLEMENTS_KW);
            loop {
                name_ref(p);
                if !p.eat(COMMA) {
                    break;
                }
            }
            e.complete(p, IMPLEMENTS_CLAUSE);
        }
        items::class_body(p);
        c.complete(p, ANON_CLASS);
        return m.complete(p, NEW_EXPR);
    }
    class_reference(p);
    if p.at(L_PAREN) {
        arg_list(p);
        trailing_lambda(p);
    }
    m.complete(p, NEW_EXPR)
}

fn match_expr(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump(MATCH_KW);
    p.bump(L_PAREN);
    p.with_newlines(false, |p| expr(p));
    p.expect(R_PAREN);
    p.expect(L_CURLY);
    p.with_newlines(false, |p| {
        while !p.at(R_CURLY) && !p.at(EOF) {
            let arm = p.start();
            if p.at(DEFAULT_KW) {
                p.bump(DEFAULT_KW);
                p.eat(COMMA);
            } else {
                let c = p.start();
                while !p.at(FAT_ARROW) && !p.at(EOF) && !p.at(R_CURLY) {
                    if opt_expr(p).is_none() {
                        p.err_and_bump("expected a match condition");
                    }
                    if !p.eat(COMMA) {
                        break;
                    }
                }
                c.complete(p, MATCH_CONDITIONS);
            }
            p.expect(FAT_ARROW);
            expr(p);
            arm.complete(p, MATCH_ARM);
            if !p.eat(COMMA) {
                break;
            }
        }
    });
    p.expect(R_CURLY);
    m.complete(p, MATCH_EXPR)
}

/// `[static] function [&] (params) [use (...)] [: T] { ... }`
fn closure(p: &mut Parser<'_>, m: Option<crate::parser::Marker>) -> CompletedMarker {
    let m = m.unwrap_or_else(|| p.start());
    p.bump(FUNCTION_KW);
    p.eat(AMP);
    param_list(p);
    if p.at(USE_KW) {
        let u = p.start();
        p.bump(USE_KW);
        p.expect(L_PAREN);
        p.with_newlines(false, |p| {
            while !p.at(R_PAREN) && !p.at(EOF) {
                let v = p.start();
                p.eat(AMP);
                p.expect(VARIABLE);
                v.complete(p, CLOSURE_USE_VAR);
                if !p.eat(COMMA) {
                    break;
                }
            }
            p.expect(R_PAREN);
        });
        u.complete(p, CLOSURE_USE);
    }
    if p.at(COLON) {
        let r = p.start();
        p.bump(COLON);
        type_(p);
        r.complete(p, RETURN_TYPE);
    }
    stmts::block(p);
    m.complete(p, CLOSURE_EXPR)
}

/// `[static] fn [&] (params) [: T] => expr`
fn arrow_fn(p: &mut Parser<'_>, m: Option<crate::parser::Marker>) -> CompletedMarker {
    let m = m.unwrap_or_else(|| p.start());
    p.bump(FN_KW);
    p.eat(AMP);
    param_list(p);
    if p.at(COLON) {
        let r = p.start();
        p.bump(COLON);
        type_(p);
        r.complete(p, RETURN_TYPE);
    }
    p.expect(FAT_ARROW);
    operand(p, bp::ASSIGN);
    m.complete(p, ARROW_FN_EXPR)
}

/// StirPHP lambda: `{ stmts }`, `{ a, b -> stmts }`, `{ x: Int -> stmts }`.
///
/// Because `->` is also member access, a single untyped parameter is only
/// recognised when its `->` is followed by whitespace: `{ u -> u->name }` has a
/// parameter, `{ it->name }` does not.
pub(super) fn lambda(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump(L_CURLY);
    if at_lambda_params(p) {
        let params = p.start();
        loop {
            let prm = p.start();
            if p.at_contextual("mut") && matches!(p.nth(1), IDENT | VARIABLE) {
                p.bump_remap(IDENT);
            }
            if p.at(VARIABLE) {
                p.bump(VARIABLE);
            } else {
                member_name(p);
            }
            if p.eat(COLON) {
                types::type_no_union(p);
            }
            prm.complete(p, PARAM);
            if !p.eat(COMMA) {
                break;
            }
        }
        params.complete(p, LAMBDA_PARAM_LIST);
        p.expect(ARROW);
    }
    p.with_spaced_arrow_stop(false, |p| p.with_newlines(true, |p| stmts::stmt_list(p)));
    p.expect(R_CURLY);
    m.complete(p, LAMBDA_EXPR)
}

fn at_lambda_params(p: &Parser<'_>) -> bool {
    let mut n = 0;
    loop {
        if p.nth_at_contextual(n, "mut") && matches!(p.nth(n + 1), IDENT | VARIABLE) {
            n += 1;
        }
        if !(matches!(p.nth(n), IDENT | VARIABLE)) {
            return false;
        }
        n += 1;
        let mut typed = false;
        if p.nth(n) == COLON {
            typed = true;
            // skip a simple type: name [<...>] [?]
            n += 1;
            let mut depth = 0;
            loop {
                match p.nth(n) {
                    L_ANGLE | L_PAREN => depth += 1,
                    R_ANGLE | R_PAREN if depth > 0 => depth -= 1,
                    EOF | R_CURLY | SEMI => return false,
                    ARROW | COMMA if depth == 0 => break,
                    _ => {}
                }
                n += 1;
                if n > 64 {
                    return false;
                }
            }
        }
        match p.nth(n) {
            COMMA => n += 1,
            ARROW => {
                if typed || n > 1 {
                    return true;
                }
                // single untyped param: require whitespace after `->`
                return !p.nth_joint(n);
            }
            _ => return false,
        }
    }
}

/// StirPHP `if (c) a else b` used as an expression.
fn if_expr(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump(IF_KW);
    p.expect(L_PAREN);
    p.with_newlines(false, |p| expr(p));
    p.expect(R_PAREN);
    branch(p);
    if p.at(ELSE_KW) || (p.newline_before() && p.nth(0) == ELSE_KW) {
        p.bump(ELSE_KW);
        branch(p);
    }
    m.complete(p, IF_EXPR)
}

fn branch(p: &mut Parser<'_>) {
    if p.at(L_CURLY) {
        stmts::block(p);
    } else if matches!(
        p.current(),
        ECHO_KW | RETURN_KW | BREAK_KW | CONTINUE_KW | UNSET_KW | GLOBAL_KW
    ) {
        // statement-only constructs are allowed as `when`/`if` branches
        stmts::stmt(p, false);
    } else {
        expr(p);
    }
}

/// `when (subject) { cond, cond -> body; is T -> body; else -> body }`
fn when_expr(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump_remap(IDENT);
    if p.at(L_PAREN) {
        p.bump(L_PAREN);
        p.with_newlines(false, |p| {
            if p.at_contextual("val") && p.nth(2) == EQ {
                // `when (val x = f())`
                p.bump_remap(IDENT);
                member_name(p);
                p.bump(EQ);
            }
            expr(p)
        });
        p.expect(R_PAREN);
    }
    p.expect(L_CURLY);
    p.with_newlines(true, |p| {
        while !p.at(R_CURLY) && !p.at(EOF) {
            let arm = p.start();
            if p.at(ELSE_KW) {
                p.bump(ELSE_KW);
            } else {
                loop {
                    let c = p.start();
                    if p.at_contextual("is") {
                        p.bump_remap(IDENT);
                        types::type_(p);
                    } else if p.at(BANG) && p.nth_at_contextual(1, "is") {
                        p.bump(BANG);
                        p.bump_remap(IDENT);
                        types::type_(p);
                    } else if p
                        .with_spaced_arrow_stop(true, |p| p.with_newlines(false, |p| opt_expr(p)))
                        .is_none()
                    {
                        p.err_and_bump("expected a `when` condition");
                    }
                    c.complete(p, WHEN_CONDITION);
                    if !p.eat(COMMA) {
                        break;
                    }
                }
            }
            p.expect(ARROW);
            branch(p);
            p.eat(SEMI);
            arm.complete(p, WHEN_ARM);
        }
    });
    p.expect(R_CURLY);
    m.complete(p, WHEN_EXPR)
}

// ---- strings ----

fn interp_string(p: &mut Parser<'_>, quote: SyntaxKind, kind: SyntaxKind) -> CompletedMarker {
    let m = p.start();
    p.bump(quote);
    interp_parts(p, quote);
    p.expect(quote);
    m.complete(p, kind)
}

fn heredoc(p: &mut Parser<'_>) -> CompletedMarker {
    let m = p.start();
    p.bump(HEREDOC_START);
    interp_parts(p, HEREDOC_END);
    p.expect(HEREDOC_END);
    m.complete(p, HEREDOC)
}

fn interp_parts(p: &mut Parser<'_>, end: SyntaxKind) {
    p.with_newlines(false, |p| {
        while !p.at(end) && !p.at(EOF) {
            match p.current() {
                STRING_CONTENT => p.bump(STRING_CONTENT),
                VARIABLE => {
                    let iv = p.start();
                    let v = p.start();
                    p.bump(VARIABLE);
                    let mut lhs = v.complete(p, VARIABLE_EXPR);
                    if p.at(L_BRACK) {
                        let m = lhs.precede(p);
                        p.bump(L_BRACK);
                        let e = p.start();
                        match p.current() {
                            MINUS => {
                                p.bump(MINUS);
                                p.eat(INT_NUMBER);
                                e.complete(p, PREFIX_EXPR);
                            }
                            INT_NUMBER => {
                                p.bump(INT_NUMBER);
                                e.complete(p, LITERAL);
                            }
                            VARIABLE => {
                                p.bump(VARIABLE);
                                e.complete(p, VARIABLE_EXPR);
                            }
                            _ => {
                                // bare key: `$a[key]` means `$a['key']`
                                p.bump_any();
                                e.complete(p, STRING_LITERAL);
                            }
                        }
                        p.expect(R_BRACK);
                        lhs = m.complete(p, INDEX_EXPR);
                    } else if p.at(ARROW) || p.at(NULLSAFE_ARROW) {
                        let nullsafe = p.at(NULLSAFE_ARROW);
                        let m = lhs.precede(p);
                        p.bump_any();
                        member_name(p);
                        lhs = m.complete(
                            p,
                            if nullsafe {
                                NULLSAFE_PROPERTY_FETCH_EXPR
                            } else {
                                PROPERTY_FETCH_EXPR
                            },
                        );
                    }
                    let _ = lhs;
                    iv.complete(p, INTERP_VAR);
                }
                CURLY_OPEN | DOLLAR_CURLY_OPEN => {
                    let ie = p.start();
                    p.bump_any();
                    expr(p);
                    p.expect(R_CURLY);
                    ie.complete(p, INTERP_EXPR);
                }
                _ => p.err_and_bump("unexpected token in string"),
            }
        }
    });
}
