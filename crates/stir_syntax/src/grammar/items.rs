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

/// Contextual StirPHP modifiers usable on classes / members.
const STIR_MODIFIERS: &[&str] = &["open", "sealed", "data", "override"];

fn at_stir_modifier(p: &Parser<'_>, n: usize) -> bool {
    p.stir() && p.nth(n) == IDENT && STIR_MODIFIERS.contains(&p.nth_text(n))
}

/// Does a declaration (function, class-like, record, typealias) start here?
pub(super) fn at_item(p: &Parser<'_>) -> bool {
    let mut n = 0;
    loop {
        match p.nth(n) {
            ATTR_OPEN => return true,
            ABSTRACT_KW | FINAL_KW | READONLY_KW => n += 1,
            IDENT
                if at_stir_modifier(p, n)
                    && (matches!(
                        p.nth(n + 1),
                        CLASS_KW | INTERFACE_KW | ABSTRACT_KW | FINAL_KW | READONLY_KW
                    ) || at_stir_modifier(p, n + 1)) =>
            {
                n += 1
            }
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
            (text.eq_ignore_ascii_case("enum") && next_is_name)
                || (p.stir() && (text == "record" || text == "typealias") && next_is_name)
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
        IDENT if p.at_contextual("record") || p.nth_text(0) == "record" => {
            record_rest(p);
            m.complete(p, RECORD_DECL);
        }
        IDENT if p.at_contextual("typealias") => {
            typealias_rest(p);
            m.complete(p, TYPEALIAS_DECL);
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

/// Parses leading modifiers (PHP keywords from `allowed` plus StirPHP
/// contextual modifiers) into a `MODIFIERS` node.
fn modifiers(p: &mut Parser<'_>, allowed: TokenSet) -> bool {
    let at_mod = |p: &Parser<'_>| {
        p.at_ts(allowed)
            || (at_stir_modifier(p, 0)
                && !matches!(p.nth(1), L_PAREN | COLON | EQ | SEMI | R_CURLY | COMMA))
    };
    if !at_mod(p) {
        return false;
    }
    let m = p.start();
    while at_mod(p) {
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

/// After the (optional) modifiers: `function [&] [<T>] [Recv.] name [<T>] (params) [: T] body`
fn function_rest(p: &mut Parser<'_>) {
    p.bump(FUNCTION_KW);
    p.eat(AMP);
    if p.at(L_ANGLE) {
        type_param_list(p);
    }
    if p.stir() && has_receiver(p) {
        let r = p.start();
        types::type_no_union(p);
        p.expect(DOT);
        r.complete(p, RECEIVER_TYPE);
    }
    decl_name(p);
    if p.at(L_ANGLE) {
        type_param_list(p);
    }
    param_list(p);
    opt_return_type(p);
    where_clause(p);
    function_body(p);
}

/// Scans ahead for `Type.name(` (an extension-function receiver).
fn has_receiver(p: &Parser<'_>) -> bool {
    let mut depth = 0i32;
    let mut n = 0;
    loop {
        match p.nth(n) {
            L_ANGLE | L_BRACK => depth += 1,
            R_ANGLE | R_BRACK => depth -= 1,
            L_PAREN if depth == 0 => return false,
            L_PAREN => depth += 1,
            R_PAREN => depth -= 1,
            DOT if depth == 0 => return true,
            EOF | L_CURLY | SEMI | EQ => return false,
            _ => {}
        }
        n += 1;
        if n > 64 {
            return false;
        }
    }
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
    } else if p.stir() && p.at(EQ) {
        // expression body: `function f(): Int = 1`
        let m = p.start();
        p.bump(EQ);
        expr(p);
        m.complete(p, FUNCTION_BODY);
        stmt_end(p);
    } else {
        stmt_end(p);
    }
}

fn where_clause(p: &mut Parser<'_>) {
    if !(p.stir() && p.at_contextual("where")) {
        return;
    }
    let m = p.start();
    p.bump_remap(IDENT);
    loop {
        let w = p.start();
        member_name(p);
        if p.at_contextual("of") {
            p.bump_remap(IDENT);
        } else {
            p.expect(COLON);
        }
        type_(p);
        w.complete(p, WHERE_PREDICATE);
        if !p.eat(COMMA) {
            break;
        }
    }
    m.complete(p, WHERE_CLAUSE);
}

/// `<T, in K of Bound, out V>`
pub(crate) fn type_param_list(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(L_ANGLE);
    p.with_newlines(false, |p| {
        while !p.at(R_ANGLE) && !p.at(EOF) {
            let t = p.start();
            if (p.at_contextual("in") || p.at_contextual("out")) && at_member_name_n(p, 1) {
                p.bump_remap(IDENT);
            }
            member_name(p);
            if p.at_contextual("of") {
                p.bump_remap(IDENT);
                type_(p);
            }
            if p.eat(EQ) {
                type_(p);
            }
            t.complete(p, TYPE_PARAM);
            if !p.eat(COMMA) {
                break;
            }
        }
        p.expect(R_ANGLE);
    });
    m.complete(p, TYPE_PARAM_LIST);
}

fn at_member_name_n(p: &Parser<'_>, n: usize) -> bool {
    p.nth(n) == IDENT || p.nth(n).is_keyword()
}

pub(crate) fn param_list(p: &mut Parser<'_>) {
    let m = p.start();
    p.expect(L_PAREN);
    p.with_newlines(false, |p| {
        while !p.at(R_PAREN) && !p.at(EOF) {
            param(p);
            if !p.at(R_PAREN) && !p.expect(COMMA) {
                break;
            }
        }
        p.expect(R_PAREN);
    });
    m.complete(p, PARAM_LIST);
}

fn param(p: &mut Parser<'_>) {
    let m = p.start();
    attributes(p);
    modifiers(p, MEMBER_MODIFIERS);
    // StirPHP binding keywords
    if (p.at_contextual("mut") && matches!(p.nth(1), IDENT | VARIABLE | ELLIPSIS))
        || (p.at_contextual("val") && matches!(p.nth(1), IDENT))
    {
        p.bump_remap(IDENT);
    } else if p.at(VAR_KW) && p.nth(1) == VARIABLE {
        p.bump(VAR_KW);
    }
    // `&x: array` is a by-reference parameter
    if p.stir() && p.at(AMP) && p.nth(2) == COLON {
        p.bump(AMP);
    }
    let stir_style = {
        let mut n = 0;
        if p.nth(0) == ELLIPSIS {
            n = 1;
        }
        let name = p.nth(n);
        (name == IDENT || name == VARIABLE || name.is_keyword())
            && (p.nth(n + 1) == COLON
                // an untyped StirPHP parameter (`f(x)`): reported by the checker
                || (p.stir() && name != VARIABLE && matches!(p.nth(n + 1), COMMA | R_PAREN | EQ)))
    };
    if stir_style {
        p.eat(ELLIPSIS);
        if p.at(VARIABLE) {
            p.bump(VARIABLE);
        } else {
            member_name(p);
        }
        if p.eat(COLON) {
            type_(p);
        }
    } else {
        if !p.at(VARIABLE) && !p.at(AMP) && !p.at(ELLIPSIS) {
            type_(p);
        }
        p.eat(AMP);
        p.eat(ELLIPSIS);
        if !p.eat(VARIABLE) {
            p.error("expected a parameter name");
        }
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
    if p.at(L_CURLY) || !p.stir() {
        class_body(p);
    } else {
        stmt_end(p);
    }
}

/// Everything between the class name and the body.
fn class_header(p: &mut Parser<'_>) {
    if p.at(L_ANGLE) {
        type_param_list(p);
    }
    if p.stir() && p.at(L_PAREN) {
        let c = p.start();
        param_list(p);
        c.complete(p, PRIMARY_CTOR);
    }
    if p.at(EXTENDS_KW) {
        let e = p.start();
        p.bump(EXTENDS_KW);
        loop {
            types::type_no_union(p);
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
            types::type_no_union(p);
            if !p.eat(COMMA) {
                break;
            }
        }
        e.complete(p, IMPLEMENTS_CLAUSE);
    }
    if p.stir() && p.at(COLON) {
        let s = p.start();
        p.bump(COLON);
        loop {
            let t = p.start();
            types::type_no_union(p);
            if p.at(L_PAREN) {
                arg_list(p);
            }
            t.complete(p, SUPERTYPE);
            if !p.eat(COMMA) {
                break;
            }
        }
        s.complete(p, SUPERTYPE_LIST);
    }
    where_clause(p);
}

fn interface_rest(p: &mut Parser<'_>) {
    p.bump(INTERFACE_KW);
    decl_name(p);
    class_header(p);
    if p.at(L_CURLY) || !p.stir() {
        class_body(p);
    } else {
        stmt_end(p);
    }
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
            types::type_no_union(p);
            if !p.eat(COMMA) {
                break;
            }
        }
        e.complete(p, IMPLEMENTS_CLAUSE);
    }
    class_body(p);
}

fn record_rest(p: &mut Parser<'_>) {
    p.bump_remap(IDENT);
    decl_name(p);
    if p.at(L_ANGLE) {
        type_param_list(p);
    }
    p.expect(L_CURLY);
    p.with_newlines(true, |p| {
        while !p.at(R_CURLY) && !p.at(EOF) {
            let f = p.start();
            attributes(p);
            if !member_name(p) {
                f.abandon(p);
                p.err_and_bump("expected a record field");
                continue;
            }
            p.expect(COLON);
            type_(p);
            if p.eat(EQ) {
                expr(p);
            }
            f.complete(p, RECORD_FIELD);
            if !p.eat(SEMI) && !p.eat(COMMA) && !p.newline_before() && !p.at(R_CURLY) {
                p.error("expected `;` or a newline");
            }
        }
    });
    p.expect(R_CURLY);
}

fn typealias_rest(p: &mut Parser<'_>) {
    p.bump_remap(IDENT);
    decl_name(p);
    if p.at(L_ANGLE) {
        type_param_list(p);
    }
    p.expect(EQ);
    type_(p);
    stmt_end(p);
}

pub(super) fn class_body(p: &mut Parser<'_>) {
    let m = p.start();
    if !p.expect(L_CURLY) {
        m.complete(p, CLASS_BODY);
        return;
    }
    p.with_newlines(true, |p| {
        while !p.at(R_CURLY) && !p.at(EOF) {
            member(p);
        }
    });
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
            if p.at(L_ANGLE) {
                type_param_list(p);
            }
            decl_name(p);
            if p.at(L_ANGLE) {
                type_param_list(p);
            }
            param_list(p);
            opt_return_type(p);
            where_clause(p);
            function_body(p);
            m.complete(p, METHOD_DECL);
        }
        IDENT if p.at_contextual("val") && at_member_name_n(p, 1) => {
            p.bump_remap(IDENT);
            if !property_elem_stir(p) {
                stmt_end(p);
            }
            m.complete(p, PROPERTY_DECL);
        }
        VAR_KW if p.stir() && p.nth(1) == VARIABLE && p.nth(2) == COLON => {
            p.bump(VAR_KW);
            if !property_elem_stir(p) {
                stmt_end(p);
            }
            m.complete(p, PROPERTY_DECL);
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

/// `name: Type = default` (val) / `$name: Type = default` (var).
/// Returns whether property hooks followed.
fn property_elem_stir(p: &mut Parser<'_>) -> bool {
    let e = p.start();
    if !p.eat(VARIABLE) {
        member_name(p);
    }
    if p.eat(COLON) {
        type_(p);
    }
    if p.eat(EQ) {
        expr(p);
    }
    let hooked = p.at(L_CURLY);
    if hooked {
        property_hooks(p);
    }
    e.complete(p, PROPERTY_ELEM);
    hooked
}

/// PHP 8.4 property hooks: `{ get => ...; set(T $v) { ... } }`
fn property_hooks(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(L_CURLY);
    p.with_newlines(true, |p| {
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
    });
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
        p.with_newlines(true, |p| {
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
        });
        p.expect(R_CURLY);
        a.complete(p, TRAIT_ADAPTATIONS);
    } else {
        stmt_end(p);
    }
}

/// `val name[: T] = expr`
pub(super) fn val_decl(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump_remap(IDENT);
    member_name(p);
    if p.eat(COLON) {
        type_(p);
    }
    if p.expect(EQ) {
        expr(p);
    }
    stmt_end(p);
    m.complete(p, VAL_DECL);
}

/// `var $name[: T] [= expr]`
pub(super) fn var_decl(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(VAR_KW);
    if !p.eat(VARIABLE) {
        if at_member_name(p) {
            p.error("mutable variables must start with `$`");
            member_name(p);
        } else {
            p.error("expected a variable");
        }
    }
    if p.eat(COLON) {
        type_(p);
    }
    if p.eat(EQ) {
        expr(p);
    }
    stmt_end(p);
    m.complete(p, VAR_DECL);
}
