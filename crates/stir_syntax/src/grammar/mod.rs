//! The PHP grammar.

mod exprs;
mod items;
mod stmts;
mod types;

use crate::{
    SyntaxKind::{self, *},
    parser::{CompletedMarker, Parser, TokenSet},
};

pub(crate) fn source_file(p: &mut Parser<'_>) {
    let m = p.start();
    while !p.at(EOF) {
        if p.at(R_CURLY) {
            p.err_and_bump("unmatched `}`");
            continue;
        }
        stmts::top_level_stmt(p);
    }
    m.complete(p, SOURCE_FILE);
}

/// Can the current token be used as a name in a member position
/// (after `->`, `::`, as a method/const name, named argument, ...)?
/// PHP allows reserved words there.
pub(super) fn at_member_name(p: &Parser<'_>) -> bool {
    let k = p.current();
    k == IDENT || k.is_keyword()
}

/// Bumps an identifier-like token (keywords allowed) as `IDENT` inside a `NAME` node.
pub(super) fn member_name(p: &mut Parser<'_>) -> bool {
    if at_member_name(p) {
        let m = p.start();
        p.bump_remap(IDENT);
        m.complete(p, NAME);
        true
    } else {
        p.error("expected a name");
        false
    }
}

/// A declaration name (`function foo`, `class Foo`). Keywords are accepted
/// with remapping so recovery stays smooth.
pub(super) fn decl_name(p: &mut Parser<'_>) -> bool {
    member_name(p)
}

pub(super) const NAME_TOKENS: TokenSet =
    TokenSet::new(&[IDENT, NAME_QUALIFIED, NAME_FULLY_QUALIFIED, NAME_RELATIVE]);

pub(super) fn at_name_ref(p: &Parser<'_>) -> bool {
    p.at_ts(NAME_TOKENS)
}

/// A (possibly qualified) reference to a class/function/constant.
pub(super) fn name_ref(p: &mut Parser<'_>) -> Option<CompletedMarker> {
    if at_name_ref(p) || p.at(STATIC_KW) || p.at(ARRAY_KW) || p.at(CALLABLE_KW) {
        let m = p.start();
        if p.at(STATIC_KW) || p.at(ARRAY_KW) || p.at(CALLABLE_KW) {
            p.bump_remap(IDENT);
        } else {
            p.bump_any();
        }
        Some(m.complete(p, NAME_REF))
    } else {
        p.error("expected a name");
        None
    }
}

/// `#[A, B(1)] #[C]`
pub(super) fn attributes(p: &mut Parser<'_>) {
    if !p.at(ATTR_OPEN) {
        return;
    }
    let list = p.start();
    while p.at(ATTR_OPEN) {
        let g = p.start();
        p.bump(ATTR_OPEN);
        while !p.at(R_BRACK) && !p.at(EOF) {
            let a = p.start();
            if name_ref(p).is_none() {
                p.bump_any();
            }
            if p.at(L_PAREN) {
                arg_list(p);
            }
            a.complete(p, ATTRIBUTE);
            if !p.eat(COMMA) {
                break;
            }
        }
        p.expect(R_BRACK);
        g.complete(p, ATTRIBUTE_GROUP);
    }
    list.complete(p, ATTRIBUTE_LIST);
}

/// `(a, b: c, ...$d)` or the first-class-callable `(...)`.
pub(super) fn arg_list(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(L_PAREN);
    if p.at(ELLIPSIS) && p.nth(1) == R_PAREN {
        let c = p.start();
        p.bump(ELLIPSIS);
        c.complete(p, CALLABLE_PLACEHOLDER);
    } else {
        while !p.at(R_PAREN) && !p.at(EOF) {
            arg(p);
            if !p.at(R_PAREN) && !p.expect(COMMA) {
                break;
            }
        }
    }
    p.expect(R_PAREN);
    m.complete(p, ARG_LIST);
}

fn arg(p: &mut Parser<'_>) {
    let m = p.start();
    if at_member_name(p) && p.nth(1) == COLON && p.nth(2) != COLON {
        // named argument (`::` is lexed as COLON2, so a single COLON is safe)
        member_name(p);
        p.bump(COLON);
        exprs::expr(p);
        m.complete(p, NAMED_ARG);
        return;
    }
    p.eat(ELLIPSIS);
    if exprs::opt_expr(p).is_none() {
        p.err_recover("expected an argument", TokenSet::new(&[COMMA, R_PAREN]));
    }
    m.complete(p, ARG);
}

/// Statement terminator: `;` or `?>`.
pub(super) fn stmt_end(p: &mut Parser<'_>) {
    if !p.eat(SEMI) && !p.at(CLOSE_TAG) {
        p.error("expected `;`");
    }
}

pub(super) use exprs::expr;
pub(super) use items::param_list;
pub(super) use types::type_;
