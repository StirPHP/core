use super::*;

const VISIBILITY: TokenSet = TokenSet::new(&[PUBLIC_KW, PRIVATE_KW, PROTECTED_KW]);
const MEMBER_MODIFIERS: TokenSet = TokenSet::new(&[
    PUBLIC_KW,
    PRIVATE_KW,
    PROTECTED_KW,
    READONLY_KW,
    FINAL_KW,
    ABSTRACT_KW,
    STATIC_KW,
]);

/// Does a declaration (function or class-like) start here?
pub(super) fn at_item(p: &Parser<'_>) -> bool {
    let mut n = 0;
    loop {
        match p.nth(n) {
            ATTR_OPEN => return true,
            ABSTRACT_KW | FINAL_KW | READONLY_KW => n += 1,
            _ => break,
        }
    }
    match p.nth(n) {
        FUNCTION_KW => {
            if n > 0 {
                return true;
            }
            // `function (` / `function &(` are closures
            !(p.nth(1) == L_PAREN || (p.nth(1) == AMP && p.nth(2) == L_PAREN))
        }
        CLASS_KW => p.nth(n + 1) != L_PAREN,
        INTERFACE_KW | TRAIT_KW => true,
        IDENT => {
            let text = p.nth_text(n);
            let next_is_name = p.nth(n + 1) == IDENT || p.nth(n + 1).is_keyword();
            text.eq_ignore_ascii_case("enum") && next_is_name
        }
        _ => false,
    }
}

pub(super) fn item(p: &mut Parser<'_>) {
    let m = p.start();
    attributes(p);
    let mods = modifiers(p, TokenSet::new(&[ABSTRACT_KW, FINAL_KW, READONLY_KW]));
    let _ = mods;
    match p.current() {
        FUNCTION_KW if !(p.nth(1) == L_PAREN || (p.nth(1) == AMP && p.nth(2) == L_PAREN)) => {
            function_rest(p);
            m.complete(p, FUNCTION_DECL);
        }
        CLASS_KW => {
            class_rest(p);
            m.complete(p, CLASS_DECL);
        }
        CONST_KW => {
            // PHP 8.5 attributes on global constants
            p.bump(CONST_KW);
            stmts::const_elems(p);
            stmt_end(p);
            m.complete(p, CONST_DECL);
        }
        INTERFACE_KW => {
            interface_rest(p);
            m.complete(p, INTERFACE_DECL);
        }
        TRAIT_KW => {
            p.bump(TRAIT_KW);
            decl_name(p);
            class_body(p);
            m.complete(p, TRAIT_DECL);
        }
        IDENT if p.nth_text(0).eq_ignore_ascii_case("enum") => {
            enum_rest(p);
            m.complete(p, ENUM_DECL);
        }
        _ => {
            // Attributes followed by an expression (`#[A] fn() => 1;`)
            if exprs::opt_expr(p).is_some() {
                stmt_end(p);
                m.complete(p, EXPR_STMT);
            } else {
                p.err_recover("expected a declaration", TokenSet::new(&[SEMI]));
                m.complete(p, ERROR);
            }
        }
    }
}

/// Parses leading modifiers from `allowed` into a `MODIFIERS` node.
fn modifiers(p: &mut Parser<'_>, allowed: TokenSet) -> bool {
    if !p.at_ts(allowed) {
        return false;
    }
    let m = p.start();
    while p.at_ts(allowed) {
        let vis = p.at_ts(VISIBILITY);
        p.bump_any();
        // asymmetric visibility: `private(set)`
        if vis && p.at(L_PAREN) && p.nth_at_contextual(1, "set") && p.nth(2) == R_PAREN {
            p.bump(L_PAREN);
            p.bump_any();
            p.bump(R_PAREN);
        }
    }
    m.complete(p, MODIFIERS);
    true
}

/// After the (optional) modifiers: `function [&] name (params) [: T] body`
fn function_rest(p: &mut Parser<'_>) {
    p.bump(FUNCTION_KW);
    p.eat(AMP);
    decl_name(p);
    param_list(p);
    opt_return_type(p);
    function_body(p);
}

fn opt_return_type(p: &mut Parser<'_>) {
    if p.at(COLON) {
        let m = p.start();
        p.bump(COLON);
        type_(p);
        m.complete(p, RETURN_TYPE);
    }
}

fn function_body(p: &mut Parser<'_>) {
    if p.at(L_CURLY) {
        stmts::block(p);
    } else {
        stmt_end(p);
    }
}

pub(crate) fn param_list(p: &mut Parser<'_>) {
    let m = p.start();
    p.expect(L_PAREN);
    while !p.at(R_PAREN) && !p.at(EOF) {
        param(p);
        if !p.at(R_PAREN) && !p.expect(COMMA) {
            break;
        }
    }
    p.expect(R_PAREN);
    m.complete(p, PARAM_LIST);
}

fn param(p: &mut Parser<'_>) {
    let m = p.start();
    attributes(p);
    modifiers(p, MEMBER_MODIFIERS);
    if !p.at(VARIABLE) && !p.at(AMP) && !p.at(ELLIPSIS) {
        type_(p);
    }
    p.eat(AMP);
    p.eat(ELLIPSIS);
    if !p.eat(VARIABLE) {
        p.error("expected a parameter name");
    }
    if p.eat(EQ) {
        expr(p);
    }
    if p.at(L_CURLY) {
        property_hooks(p);
    }
    m.complete(p, PARAM);
}

fn class_rest(p: &mut Parser<'_>) {
    p.bump(CLASS_KW);
    decl_name(p);
    class_header(p);
    class_body(p);
}

/// Everything between the class name and the body.
fn class_header(p: &mut Parser<'_>) {
    if p.at(EXTENDS_KW) {
        let e = p.start();
        p.bump(EXTENDS_KW);
        loop {
            types::class_type(p);
            if !p.eat(COMMA) {
                break;
            }
        }
        e.complete(p, EXTENDS_CLAUSE);
    }
    if p.at(IMPLEMENTS_KW) {
        let e = p.start();
        p.bump(IMPLEMENTS_KW);
        loop {
            types::class_type(p);
            if !p.eat(COMMA) {
                break;
            }
        }
        e.complete(p, IMPLEMENTS_CLAUSE);
    }
}

fn interface_rest(p: &mut Parser<'_>) {
    p.bump(INTERFACE_KW);
    decl_name(p);
    class_header(p);
    class_body(p);
}

fn enum_rest(p: &mut Parser<'_>) {
    p.bump_remap(IDENT);
    decl_name(p);
    if p.at(COLON) {
        p.bump(COLON);
        type_(p);
    }
    if p.at(IMPLEMENTS_KW) {
        let e = p.start();
        p.bump(IMPLEMENTS_KW);
        loop {
            types::class_type(p);
            if !p.eat(COMMA) {
                break;
            }
        }
        e.complete(p, IMPLEMENTS_CLAUSE);
    }
    class_body(p);
}

pub(super) fn class_body(p: &mut Parser<'_>) {
    let m = p.start();
    if !p.expect(L_CURLY) {
        m.complete(p, CLASS_BODY);
        return;
    }
    while !p.at(R_CURLY) && !p.at(EOF) {
        member(p);
    }
    p.expect(R_CURLY);
    m.complete(p, CLASS_BODY);
}

fn member(p: &mut Parser<'_>) {
    let m = p.start();
    attributes(p);
    modifiers(p, MEMBER_MODIFIERS);
    match p.current() {
        USE_KW => {
            trait_use(p);
            m.complete(p, TRAIT_USE);
        }
        CASE_KW => {
            p.bump(CASE_KW);
            member_name(p);
            if p.eat(EQ) {
                expr(p);
            }
            stmt_end(p);
            m.complete(p, ENUM_CASE);
        }
        CONST_KW => {
            p.bump(CONST_KW);
            stmts::const_elems(p);
            stmt_end(p);
            m.complete(p, CLASS_CONST_DECL);
        }
        FUNCTION_KW => {
            p.bump(FUNCTION_KW);
            p.eat(AMP);
            decl_name(p);
            param_list(p);
            opt_return_type(p);
            function_body(p);
            m.complete(p, METHOD_DECL);
        }
        R_CURLY | EOF => {
            p.error("expected a class member");
            m.complete(p, ERROR);
        }
        _ => {
            // PHP property: `[var] [Type] $a [= 1], $b [hooks];`
            p.eat(VAR_KW);
            if !p.at(VARIABLE) {
                let checkpoint_ok = types::at_type_start(p);
                if !checkpoint_ok {
                    p.err_and_bump("expected a class member");
                    m.complete(p, ERROR);
                    return;
                }
                type_(p);
            }
            let mut hooked = false;
            loop {
                let e = p.start();
                if !p.eat(VARIABLE) {
                    p.error("expected a property name");
                    e.complete(p, PROPERTY_ELEM);
                    break;
                }
                if p.eat(EQ) {
                    expr(p);
                }
                if p.at(L_CURLY) {
                    property_hooks(p);
                    hooked = true;
                }
                e.complete(p, PROPERTY_ELEM);
                if !p.eat(COMMA) {
                    break;
                }
            }
            if !hooked {
                stmt_end(p);
            }
            m.complete(p, PROPERTY_DECL);
        }
    }
}

/// PHP 8.4 property hooks: `{ get => ...; set(T $v) { ... } }`
fn property_hooks(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(L_CURLY);
    while !p.at(R_CURLY) && !p.at(EOF) {
        let h = p.start();
        attributes(p);
        modifiers(p, TokenSet::new(&[FINAL_KW]));
        p.eat(AMP);
        if !member_name(p) {
            h.abandon(p);
            p.err_and_bump("expected `get` or `set`");
            continue;
        }
        if p.at(L_PAREN) {
            param_list(p);
        }
        if p.eat(FAT_ARROW) {
            expr(p);
            stmt_end(p);
        } else if p.at(L_CURLY) {
            stmts::block(p);
        } else {
            stmt_end(p);
        }
        h.complete(p, PROPERTY_HOOK);
    }
    p.expect(R_CURLY);
    m.complete(p, PROPERTY_HOOKS);
}

fn trait_use(p: &mut Parser<'_>) {
    p.bump(USE_KW);
    loop {
        name_ref(p);
        if !p.eat(COMMA) {
            break;
        }
    }
    if p.at(L_CURLY) {
        let a = p.start();
        p.bump(L_CURLY);
        while !p.at(R_CURLY) && !p.at(EOF) {
            let r = p.start();
            // `[Trait::]method insteadof A, B;` or `[Trait::]method as [visibility] [alias];`
            if at_name_ref(p) && p.nth(1) == COLON2 {
                name_ref(p);
                p.bump(COLON2);
            }
            member_name(p);
            if p.eat(INSTEADOF_KW) {
                loop {
                    name_ref(p);
                    if !p.eat(COMMA) {
                        break;
                    }
                }
                stmt_end(p);
                r.complete(p, TRAIT_PRECEDENCE);
            } else {
                p.expect(AS_KW);
                if p.at_ts(MEMBER_MODIFIERS) {
                    p.bump_any();
                }
                if at_member_name(p) {
                    member_name(p);
                }
                stmt_end(p);
                r.complete(p, TRAIT_ALIAS);
            }
        }
        p.expect(R_CURLY);
        a.complete(p, TRAIT_ADAPTATIONS);
    } else {
        stmt_end(p);
    }
}
