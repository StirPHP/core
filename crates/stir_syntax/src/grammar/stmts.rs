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
    loop {
        if p.at(EOF) || (p.at_ts(terminators) && !blade::at_flat_condition_part(p)) {
            break;
        }
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
    stmt_list(p);
    p.expect(R_CURLY);
    m.complete(p, BLOCK);
}

pub(super) fn stmt(p: &mut Parser<'_>, top_level: bool) {
    let _ = top_level;
    if p.at_directive() && blade::stmt(p) {
        return;
    }
    match p.current() {
        BLADE_DIRECTIVE_NAME => blade::directive(p),
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
            if !p.at(SEMI) && !p.at(CLOSE_TAG) && !p.at(R_CURLY) {
                expr(p);
            }
            stmt_end(p);
            m.complete(p, kind);
        }
        RETURN_KW => {
            let m = p.start();
            p.bump(RETURN_KW);
            if !p.at(SEMI) && !p.at(CLOSE_TAG) && !p.at(EOF) && !p.at(R_CURLY) {
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
            while !p.at(R_PAREN) && !p.at(EOF) {
                expr(p);
                if !p.eat(COMMA) {
                    break;
                }
            }
            p.expect(R_PAREN);
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
        IDENT if p.nth(1) == COLON && p.nth(2) != COLON => {
            let m = p.start();
            member_name(p);
            p.bump(COLON);
            m.complete(p, LABEL_STMT);
        }
        _ => {
            if items::at_item(p) {
                items::item(p);
            } else {
                expr_stmt(p);
            }
        }
    }
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

pub(super) fn paren_cond(p: &mut Parser<'_>) {
    p.expect(L_PAREN);
    expr(p);
    p.expect(R_PAREN);
}

/// The parenthesized condition of a statement, which a directive may have left out.
fn stmt_cond(p: &mut Parser<'_>, blade: bool) {
    if blade && !p.at(L_PAREN) {
        p.error("expected a condition in `(...)`");
    } else {
        paren_cond(p);
    }
}

/// Body of a control structure: a statement, or (with `:` or after a directive) an
/// alternative-syntax list.
fn control_body(p: &mut Parser<'_>, terminators: TokenSet, blade: bool) -> bool {
    if blade || p.at(COLON) {
        let m = p.start();
        if !blade {
            p.bump(COLON);
        }
        stmt_list_of_directive(p, terminators);
        m.complete(p, BLOCK);
        true
    } else {
        stmt(p, false);
        false
    }
}

/// An alternative-syntax list, which has no business with the `@auth` that's open around it.
pub(super) fn stmt_list_of_directive(p: &mut Parser<'_>, terminators: TokenSet) {
    let open = std::mem::take(&mut p.blade_conditions);
    alt_stmt_list(p, terminators);
    p.blade_conditions = open;
}

/// The end of an alternative-syntax list: `endif;` or `@endif`.
pub(super) fn alt_end(p: &mut Parser<'_>, end: SyntaxKind, blade: bool) {
    if p.at(end) && p.at_directive() {
        p.bump(end);
    } else if blade && !p.at(end) {
        p.error(format!("expected {}", blade::describe_end(end)));
    } else {
        p.expect(end);
        stmt_end(p);
    }
}

pub(super) fn if_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    let blade = p.at_directive();
    match p.current() {
        BANG | ISSET_KW | EMPTY_KW => blade::condition_of_if(p),
        _ => {
            p.bump(IF_KW);
            stmt_cond(p, blade);
        }
    }
    let alt = control_body(p, TokenSet::new(&[ELSEIF_KW, ELSE_KW, ENDIF_KW]), blade);
    loop {
        let clause_blade = p.at_directive();
        if p.at(ELSEIF_KW) || (p.at(ELSE_KW) && p.nth(1) == IF_KW && !alt) {
            let c = p.start();
            if p.at(ELSE_KW) {
                // `else if` is parsed as else { if } by PHP, but we keep it flat.
                p.bump(ELSE_KW);
                p.bump(IF_KW);
            } else {
                p.bump(ELSEIF_KW);
            }
            stmt_cond(p, clause_blade);
            control_body(
                p,
                TokenSet::new(&[ELSEIF_KW, ELSE_KW, ENDIF_KW]),
                clause_blade,
            );
            c.complete(p, ELSEIF_CLAUSE);
        } else if p.at(ELSE_KW) {
            let c = p.start();
            p.bump(ELSE_KW);
            control_body(p, TokenSet::new(&[ENDIF_KW]), clause_blade);
            c.complete(p, ELSE_CLAUSE);
        } else {
            break;
        }
    }
    if alt {
        alt_end(p, ENDIF_KW, blade);
    }
    m.complete(p, IF_STMT);
}

fn while_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    let blade = p.at_directive();
    p.bump(WHILE_KW);
    stmt_cond(p, blade);
    if control_body(p, TokenSet::new(&[ENDWHILE_KW]), blade) {
        alt_end(p, ENDWHILE_KW, blade);
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
    let blade = p.at_directive();
    p.bump(FOR_KW);
    if blade && !p.at(L_PAREN) {
        p.error("expected `(`");
    } else {
        p.expect(L_PAREN);
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
    }
    if control_body(p, TokenSet::new(&[ENDFOR_KW]), blade) {
        alt_end(p, ENDFOR_KW, blade);
    }
    m.complete(p, FOR_STMT);
}

fn foreach_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    let blade = p.at_directive();
    p.bump(FOREACH_KW);
    if blade && !p.at(L_PAREN) {
        p.error("expected `(`");
    } else {
        p.expect(L_PAREN);
        expr(p);
        p.expect(AS_KW);
        p.eat(AMP);
        expr(p);
        if p.eat(FAT_ARROW) {
            p.eat(AMP);
            expr(p);
        }
        p.expect(R_PAREN);
    }
    let mut forelse_empty = false;
    if control_body(p, TokenSet::new(&[ENDFOREACH_KW]), blade) {
        forelse_empty = blade::at_forelse_empty(p);
        alt_end(p, ENDFOREACH_KW, blade);
    }
    m.complete(p, FOREACH_STMT);
    if forelse_empty {
        blade::forelse_empty_branch(p);
    }
}

fn switch_stmt(p: &mut Parser<'_>) {
    let m = p.start();
    let blade = p.at_directive();
    p.bump(SWITCH_KW);
    stmt_cond(p, blade);
    let alt = if blade {
        true
    } else if p.at(COLON) {
        p.bump(COLON);
        true
    } else {
        p.expect(L_CURLY);
        false
    };
    let end = if alt { ENDSWITCH_KW } else { R_CURLY };
    p.eat(SEMI);
    if blade {
        blade::before_first_case(p);
    }
    while !p.at(end) && !p.at(EOF) {
        let c = p.start();
        let case_blade = p.at_directive();
        if case_blade && p.at(CASE_KW) {
            p.bump(CASE_KW);
            stmt_cond(p, true);
        } else if p.eat(CASE_KW) {
            expr(p);
        } else if !p.eat(DEFAULT_KW) {
            c.abandon(p);
            p.err_and_bump("expected `case` or `default`");
            continue;
        }
        if !case_blade && !p.eat(COLON) {
            p.expect(SEMI);
        }
        while !p.at(CASE_KW) && !p.at(DEFAULT_KW) && !p.at(end) && !p.at(EOF) {
            if p.at(R_CURLY) {
                p.err_and_bump("unmatched `}`");
                continue;
            }
            stmt(p, false);
        }
        c.complete(p, SWITCH_CASE);
    }
    if alt {
        alt_end(p, end, blade);
    } else {
        p.expect(end);
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
        loop {
            name_ref(p);
            if !p.eat(PIPE) {
                break;
            }
        }
        p.eat(VARIABLE);
        p.expect(R_PAREN);
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
    if p.at(SEMI) || p.at(CLOSE_TAG) || p.at(EOF) {
        stmt_end(p);
    } else if control_body(p, TokenSet::new(&[ENDDECLARE_KW]), false) {
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
        while !p.at(R_CURLY) && !p.at(EOF) {
            use_clause(p, false);
            if !p.eat(COMMA) {
                break;
            }
        }
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
        if types::at_type_start(p) && p.nth(1) != EQ {
            type_(p);
        }
        member_name(p);
        p.expect(EQ);
        expr(p);
        e.complete(p, CONST_ELEM);
        if !p.eat(COMMA) {
            break;
        }
    }
}
