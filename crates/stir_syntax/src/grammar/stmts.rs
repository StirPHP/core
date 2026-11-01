use super::*;

const STMT_RECOVERY: TokenSet = TokenSet::new(&[
    SEMI,
    R_CURLY,
    IF_KW,
    WHILE_KW,
    FOR_KW,
    FOREACH_KW,
    RETURN_KW,
    FUNCTION_KW,
    CLASS_KW,
    ECHO_KW,
]);

pub(super) fn top_level_stmt(p: &mut Parser<'_>) {
    stmt(p, true);
}

/// Parses statements until `}` (not consumed) or EOF.
pub(super) fn stmt_list(p: &mut Parser<'_>) {
    while !p.at(R_CURLY) && !p.at(EOF) {
        stmt(p, false);
    }
}

/// Statements until one of the alternative-syntax terminators.
fn alt_stmt_list(p: &mut Parser<'_>, terminators: TokenSet) {
    while !p.at_ts(terminators) && !p.at(EOF) {
        if p.at(R_CURLY) {
            p.err_and_bump("unmatched `}`");
            continue;
        }
        stmt(p, false);
    }
}

pub(super) fn block(p: &mut Parser<'_>) {
    let m = p.start();
    p.expect(L_CURLY);
    p.with_newlines(true, |p| stmt_list(p));
    p.expect(R_CURLY);
    m.complete(p, BLOCK);
}

pub(super) fn stmt(p: &mut Parser<'_>, top_level: bool) {
    let _ = top_level;
    match p.current() {
        OPEN_TAG | CLOSE_TAG => {
            // Tags behave like statement separators.
            p.bump_any();
        }
        INLINE_HTML => {
            let m = p.start();
            p.bump(INLINE_HTML);
            m.complete(p, INLINE_HTML_STMT);
        }
        OPEN_TAG_WITH_ECHO => {
            let m = p.start();
            p.bump(OPEN_TAG_WITH_ECHO);
            expr_list(p);
            stmt_end(p);
            m.complete(p, ECHO_TAG_STMT);
        }
        SEMI => {
            let m = p.start();
            p.bump(SEMI);
            m.complete(p, EMPTY_STMT);
        }
        L_CURLY => block(p),
        IF_KW => if_stmt(p),
        WHILE_KW => while_stmt(p),
        DO_KW => do_while_stmt(p),
        FOR_KW => for_stmt(p),
        FOREACH_KW => foreach_stmt(p),
        SWITCH_KW => switch_stmt(p),
        BREAK_KW | CONTINUE_KW => {
            let m = p.start();
            let kind = if p.at(BREAK_KW) {
                BREAK_STMT
            } else {
                CONTINUE_STMT
            };
            p.bump_any();
            if !p.at(SEMI) && !p.at(CLOSE_TAG) && !p.newline_terminates() && !p.at(R_CURLY) {
                expr(p);
            }
            stmt_end(p);
            m.complete(p, kind);
        }
        RETURN_KW => {
            let m = p.start();
            p.bump(RETURN_KW);
            if !p.at(SEMI)
                && !p.at(CLOSE_TAG)
                && !p.at(EOF)
                && !p.at(R_CURLY)
                && !p.newline_terminates()
            {
                expr(p);
            }
            stmt_end(p);
            m.complete(p, RETURN_STMT);
        }
        ECHO_KW => {
            let m = p.start();
            p.bump(ECHO_KW);
            expr_list(p);
            stmt_end(p);
            m.complete(p, ECHO_STMT);
        }
        TRY_KW => try_stmt(p),
        GLOBAL_KW => {
            let m = p.start();
            p.bump(GLOBAL_KW);
            loop {
                exprs::simple_variable(p);
                if !p.eat(COMMA) {
                    break;
                }
            }
            stmt_end(p);
            m.complete(p, GLOBAL_STMT);
        }
        STATIC_KW if p.nth(1) == VARIABLE => {
            let m = p.start();
            p.bump(STATIC_KW);
            loop {
                let v = p.start();
                p.expect(VARIABLE);
                if p.eat(EQ) {
                    expr(p);
                }
                v.complete(p, STATIC_VAR);
                if !p.eat(COMMA) {
                    break;
                }
            }
            stmt_end(p);
            m.complete(p, STATIC_VAR_STMT);
        }
        UNSET_KW => {
            let m = p.start();
            p.bump(UNSET_KW);
            p.expect(L_PAREN);
            p.with_newlines(false, |p| {
                while !p.at(R_PAREN) && !p.at(EOF) {
                    expr(p);
                    if !p.eat(COMMA) {
                        break;
                    }
                }
                p.expect(R_PAREN);
            });
            stmt_end(p);
            m.complete(p, UNSET_STMT);
        }
        GOTO_KW => {
            let m = p.start();
            p.bump(GOTO_KW);
            member_name(p);
            stmt_end(p);
            m.complete(p, GOTO_STMT);
        }
        DECLARE_KW => declare_stmt(p),
        NAMESPACE_KW if matches!(p.nth(1), IDENT | NAME_QUALIFIED | L_CURLY | SEMI) => {
            namespace_decl(p)
        }
        USE_KW => use_decl(p),
        CONST_KW => {
            let m = p.start();
            p.bump(CONST_KW);
            const_elems(p);
            stmt_end(p);
            m.complete(p, CONST_DECL);
        }
        HALT_COMPILER_KW => {
            let m = p.start();
            p.bump(HALT_COMPILER_KW);
            p.expect(L_PAREN);
            p.expect(R_PAREN);
            if !p.eat(SEMI) {
                p.eat(CLOSE_TAG);
            }
            p.eat(HALT_DATA);
            m.complete(p, HALT_COMPILER_STMT);
        }
        IDENT if p.nth(1) == COLON && !p.stir() && p.nth(2) != COLON => {
            let m = p.start();
            member_name(p);
            p.bump(COLON);
            m.complete(p, LABEL_STMT);
        }
        VAR_KW if p.stir() => items::var_decl(p),
        IDENT if p.stir() && p.at_contextual("val") && at_name_after(p, 1) => items::val_decl(p),
        _ => {
            if items::at_item(p) {
                items::item(p);
            } else {
                expr_stmt(p);
            }
        }
    }
}

/// Is token `n` a plain declaration name (identifier)?
fn at_name_after(p: &Parser<'_>, n: usize) -> bool {
    p.nth(n) == IDENT || p.nth(n).is_keyword()
}

fn expr_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    if exprs::opt_expr(p).is_none() {
        m.abandon(p);
        p.err_recover("expected a statement", STMT_RECOVERY);
        return;
    }
    stmt_end(p);
    m.complete(p, EXPR_STMT);
}

fn expr_list(p: &mut Parser<'_>) {
    loop {
        expr(p);
        if !p.eat(COMMA) {
            break;
        }
    }
}

fn paren_cond(p: &mut Parser<'_>) {
    p.expect(L_PAREN);
    p.with_newlines(false, |p| {
        expr(p);
    });
    p.expect(R_PAREN);
}

/// Body of a control structure: a statement, or (with `:`) an alternative-syntax list.
fn control_body(p: &mut Parser<'_>, terminators: TokenSet) -> bool {
    if p.at(COLON) && !p.stir() {
        let m = p.start();
        p.bump(COLON);
        alt_stmt_list(p, terminators);
        m.complete(p, BLOCK);
        true
    } else {
        stmt(p, false);
        false
    }
}

fn if_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(IF_KW);
    paren_cond(p);
    let alt = control_body(p, TokenSet::new(&[ELSEIF_KW, ELSE_KW, ENDIF_KW]));
    loop {
        if p.at(ELSEIF_KW) || (p.at(ELSE_KW) && p.nth(1) == IF_KW && !alt) {
            let c = p.start();
            if p.at(ELSE_KW) {
                // `else if` is parsed as else { if } by PHP, but we keep it flat.
                p.bump(ELSE_KW);
                p.bump(IF_KW);
            } else {
                p.bump(ELSEIF_KW);
            }
            paren_cond(p);
            control_body(p, TokenSet::new(&[ELSEIF_KW, ELSE_KW, ENDIF_KW]));
            c.complete(p, ELSEIF_CLAUSE);
        } else if p.at(ELSE_KW) {
            let c = p.start();
            p.bump(ELSE_KW);
            control_body(p, TokenSet::new(&[ENDIF_KW]));
            c.complete(p, ELSE_CLAUSE);
        } else {
            break;
        }
    }
    if alt {
        p.expect(ENDIF_KW);
        stmt_end(p);
    }
    m.complete(p, IF_STMT);
}

fn while_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(WHILE_KW);
    paren_cond(p);
    if control_body(p, TokenSet::new(&[ENDWHILE_KW])) {
        p.expect(ENDWHILE_KW);
        stmt_end(p);
    }
    m.complete(p, WHILE_STMT);
}

fn do_while_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(DO_KW);
    stmt(p, false);
    p.expect(WHILE_KW);
    paren_cond(p);
    stmt_end(p);
    m.complete(p, DO_WHILE_STMT);
}

fn for_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(FOR_KW);
    p.expect(L_PAREN);
    p.with_newlines(false, |p| {
        for (i, kind) in [FOR_INIT, FOR_COND, FOR_STEP].into_iter().enumerate() {
            let c = p.start();
            let end = if i == 2 { R_PAREN } else { SEMI };
            while !p.at(end) && !p.at(EOF) {
                if exprs::opt_expr(p).is_none() {
                    p.err_and_bump("expected an expression");
                }
                if !p.eat(COMMA) {
                    break;
                }
            }
            c.complete(p, kind);
            p.expect(end);
        }
    });
    if control_body(p, TokenSet::new(&[ENDFOR_KW])) {
        p.expect(ENDFOR_KW);
        stmt_end(p);
    }
    m.complete(p, FOR_STMT);
}

fn foreach_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(FOREACH_KW);
    p.expect(L_PAREN);
    p.with_newlines(false, |p| {
        p.with_no_as(true, |p| expr(p));
        p.expect(AS_KW);
        p.eat(AMP);
        expr(p);
        if p.eat(FAT_ARROW) {
            p.eat(AMP);
            expr(p);
        }
        p.expect(R_PAREN);
    });
    if control_body(p, TokenSet::new(&[ENDFOREACH_KW])) {
        p.expect(ENDFOREACH_KW);
        stmt_end(p);
    }
    m.complete(p, FOREACH_STMT);
}

fn switch_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(SWITCH_KW);
    paren_cond(p);
    let alt = if p.at(COLON) && !p.stir() {
        p.bump(COLON);
        true
    } else {
        p.expect(L_CURLY);
        false
    };
    let end = if alt { ENDSWITCH_KW } else { R_CURLY };
    p.with_newlines(true, |p| {
        p.eat(SEMI);
        while !p.at(end) && !p.at(EOF) {
            let c = p.start();
            if p.eat(CASE_KW) {
                p.with_newlines(false, |p| expr(p));
            } else if !p.eat(DEFAULT_KW) {
                c.abandon(p);
                p.err_and_bump("expected `case` or `default`");
                continue;
            }
            if !p.eat(COLON) {
                p.expect(SEMI);
            }
            while !p.at(CASE_KW) && !p.at(DEFAULT_KW) && !p.at(end) && !p.at(EOF) {
                stmt(p, false);
            }
            c.complete(p, SWITCH_CASE);
        }
    });
    p.expect(end);
    if alt {
        stmt_end(p);
    }
    m.complete(p, SWITCH_STMT);
}

fn try_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(TRY_KW);
    block(p);
    while p.at(CATCH_KW) {
        let c = p.start();
        p.bump(CATCH_KW);
        p.expect(L_PAREN);
        p.with_newlines(false, |p| {
            loop {
                name_ref(p);
                if !p.eat(PIPE) {
                    break;
                }
            }
            p.eat(VARIABLE);
            p.expect(R_PAREN);
        });
        block(p);
        c.complete(p, CATCH_CLAUSE);
    }
    if p.at(FINALLY_KW) {
        let c = p.start();
        p.bump(FINALLY_KW);
        block(p);
        c.complete(p, FINALLY_CLAUSE);
    }
    m.complete(p, TRY_STMT);
}

fn declare_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(DECLARE_KW);
    p.expect(L_PAREN);
    p.with_newlines(false, |p| {
        while !p.at(R_PAREN) && !p.at(EOF) {
            let d = p.start();
            member_name(p);
            p.expect(EQ);
            expr(p);
            d.complete(p, DECLARE_ITEM);
            if !p.eat(COMMA) {
                break;
            }
        }
        p.expect(R_PAREN);
    });
    if p.at(SEMI) || p.at(CLOSE_TAG) || (p.stir() && p.newline_before()) || p.at(EOF) {
        stmt_end(p);
    } else if control_body(p, TokenSet::new(&[ENDDECLARE_KW])) {
        p.expect(ENDDECLARE_KW);
        stmt_end(p);
    }
    m.complete(p, DECLARE_STMT);
}

fn namespace_decl(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(NAMESPACE_KW);
    if at_name_ref(p) {
        name_ref(p);
    }
    if p.at(L_CURLY) {
        block(p);
    } else {
        stmt_end(p);
    }
    m.complete(p, NAMESPACE_DECL);
}

fn use_decl(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(USE_KW);
    if p.at(FUNCTION_KW) || p.at(CONST_KW) {
        p.bump_any();
    }
    loop {
        use_clause(p, true);
        if !p.eat(COMMA) {
            break;
        }
    }
    stmt_end(p);
    m.complete(p, USE_DECL);
}

fn use_clause(p: &mut Parser<'_>, allow_group: bool) {
    let m = p.start();
    if p.at(FUNCTION_KW) || p.at(CONST_KW) {
        p.bump_any();
    }
    if name_ref(p).is_none() {
        m.abandon(p);
        return;
    }
    if allow_group && p.at(BACKSLASH) && p.nth(1) == L_CURLY {
        p.bump(BACKSLASH);
        let g = p.start();
        p.bump(L_CURLY);
        p.with_newlines(false, |p| {
            while !p.at(R_CURLY) && !p.at(EOF) {
                use_clause(p, false);
                if !p.eat(COMMA) {
                    break;
                }
            }
        });
        p.expect(R_CURLY);
        g.complete(p, USE_GROUP);
    } else if p.eat(AS_KW) {
        member_name(p);
    }
    m.complete(p, USE_CLAUSE);
}

pub(super) fn const_elems(p: &mut Parser<'_>) {
    loop {
        let e = p.start();
        // PHP 8.3 typed class constants: `const int X = 1;`
        if (p.at_ts(NAME_TOKENS) || p.current().is_keyword() || p.at(QUESTION) || p.at(L_PAREN))
            && !matches!(p.nth(1), EQ | COLON)
        {
            type_(p);
        }
        member_name(p);
        if p.stir() && p.at(COLON) {
            p.bump(COLON);
            type_(p);
        }
        p.expect(EQ);
        expr(p);
        e.complete(p, CONST_ELEM);
        if !p.eat(COMMA) {
            break;
        }
    }
}
