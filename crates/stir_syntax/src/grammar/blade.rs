//! Blade directives. Control flow is parsed by the PHP statement it compiles to, in `stmts`, with
//! a directive standing in for its keyword and no `:` or `;`. What's here is what doesn't fit that.

use super::*;
use crate::blade::{self, Condition};

/// Parses the directive at the current token if it isn't the plain case of its statement, in
/// which case it returns `false`.
pub(super) fn stmt(p: &mut Parser<'_>) -> bool {
    match p.current() {
        BANG | ISSET_KW | EMPTY_KW => {
            stmts::if_stmt(p);
        }
        BREAK_KW | CONTINUE_KW => jump(p),
        // `@else` where no `@if` wants it: the `@auth` it belongs to is flat, or a condition of
        // the project's own (`Blade::if`), which is text to us.
        ELSE_KW | ELSEIF_KW => directive(p),
        // Not the `@if` it would be anywhere else: `@hasSection(...) ... @endif`, and also
        // `@auth ... @endif`, which works in Blade as they all compile to `endif`.
        ENDIF_KW if p.blade_conditions.any_open() => {
            directive(p);
            let open = &mut p.blade_conditions;
            if open.until_endif > 0 {
                open.until_endif -= 1;
            } else {
                open.with_end -= 1;
            }
        }
        ENDIF_KW | ENDFOREACH_KW | ENDFOR_KW | ENDWHILE_KW | ENDSWITCH_KW | CASE_KW
        | DEFAULT_KW => {
            let m = p.start();
            p.error(format!("unmatched `{}`", p.nth_text(0)));
            p.bump_any();
            if p.at(L_PAREN) {
                arg_list(p);
            }
            m.complete(p, ERROR);
        }
        _ => return false,
    }
    true
}

/// A directive that's a node of its own.
pub(super) fn directive(p: &mut Parser<'_>) {
    let name = p.nth_text(0);
    let m = p.start();
    p.bump_remap(BLADE_DIRECTIVE_NAME);
    if name == "@verbatim" {
        p.eat(INLINE_HTML);
        p.bump_remap(BLADE_DIRECTIVE_NAME);
        m.complete(p, BLADE_DIRECTIVE);
        return;
    }
    let found = blade::directive(&name[1..]);
    if found.is_some_and(|d| d.name == "php") {
        // `<?php (...); ?>`
        exprs::expr(p);
        m.complete(p, EXPR_STMT);
        return;
    }
    if p.at(L_PAREN) {
        arg_list(p);
    } else {
        p.eat(BLADE_RAW_ARGS);
    }
    m.complete(p, BLADE_DIRECTIVE);
    match found.map(|d| d.condition) {
        Some(Condition::Opens) => p.blade_conditions.with_end += 1,
        Some(Condition::OpensUntilEndif) => p.blade_conditions.until_endif += 1,
        Some(Condition::Closes) => {
            p.blade_conditions.with_end = p.blade_conditions.with_end.saturating_sub(1);
        }
        _ => {}
    }
}

/// Is the current token an `@else`, `@elseif` or `@endif` of a flat directive around it?
pub(super) fn at_flat_condition_part(p: &Parser<'_>) -> bool {
    let open = p.blade_conditions;
    let theirs = match p.current() {
        ELSE_KW | ELSEIF_KW => open.any_open(),
        ENDIF_KW => open.until_endif > 0,
        _ => false,
    };
    theirs && p.at_directive()
}

/// `@unless (...)`, which is `if (! (...))`, `@isset (...)` and `@empty (...)`: what stands where
/// the statement would have its keyword and condition.
pub(super) fn condition_of_if(p: &mut Parser<'_>) {
    if !p.at(BANG) {
        if p.nth(1) == L_PAREN {
            exprs::expr(p);
        } else {
            p.error("expected `(...)`");
            p.bump_any();
        }
        return;
    }
    let m = p.start();
    p.bump(BANG);
    if p.at(L_PAREN) {
        exprs::expr(p);
    } else {
        p.error("expected a condition in `(...)`");
    }
    m.complete(p, PREFIX_EXPR);
}

/// `@break`, `@break(2)`, `@break($condition)`, and the same with `@continue`.
///
/// The one with a condition compiles to `if (...) break;`, so that's what it is: an `IF_STMT` with
/// this directive for its keyword, and an empty `BREAK_STMT` for its body. Inside an `@auth` it
/// can't be known to run either, so it's also in an `IF_STMT`, one without a condition.
fn jump(p: &mut Parser<'_>) {
    let kind = if p.at(BREAK_KW) {
        BREAK_STMT
    } else {
        CONTINUE_STMT
    };
    if p.nth(1) == L_PAREN && !has_levels(p) {
        let m = p.start();
        p.bump_any();
        stmts::paren_cond(p);
        let body = p.start();
        body.complete(p, kind);
        m.complete(p, IF_STMT);
        return;
    }
    let conditional = p.blade_conditions.any_open().then(|| p.start());
    let m = p.start();
    p.bump_any();
    if p.at(L_PAREN) {
        stmts::paren_cond(p);
    }
    m.complete(p, kind);
    if let Some(m) = conditional {
        m.complete(p, IF_STMT);
    }
}

/// Blade's `/\(\s*(-?\d+)\s*\)$/`: the current directive's argument is a number of levels.
fn has_levels(p: &Parser<'_>) -> bool {
    let number = if p.nth(2) == MINUS { 3 } else { 2 };
    p.nth(number) == INT_NUMBER
        && p.nth(number + 1) == R_PAREN
        && p.nth_text(number).bytes().all(|b| b.is_ascii_digit())
}

/// Is the current token the `@empty` of a `@forelse`?
pub(super) fn at_forelse_empty(p: &Parser<'_>) -> bool {
    p.at(ENDFOREACH_KW) && p.nth_text(0).eq_ignore_ascii_case("@empty")
}

/// What follows `@empty` up to `@endforelse`. Blade compiles `@empty` to `endforeach; if (...):`,
/// and so is it here: the loop ends there, and this is an `if` whose condition can't be known,
/// which doesn't have one.
pub(super) fn forelse_empty_branch(p: &mut Parser<'_>) {
    let m = p.start();
    let body = p.start();
    stmts::stmt_list_of_directive(p, TokenSet::new(&[ENDIF_KW]));
    body.complete(p, BLOCK);
    stmts::alt_end(p, ENDIF_KW, true);
    m.complete(p, IF_STMT);
}

/// Up to the first `@case`, a compiled `@switch` is still in PHP, where only white space can be.
pub(super) fn before_first_case(p: &mut Parser<'_>) {
    while p.at(INLINE_HTML) {
        if !p.nth_text(0).trim().is_empty() {
            p.error("expected `@case` or `@default`");
        }
        let m = p.start();
        p.bump(INLINE_HTML);
        m.complete(p, INLINE_HTML_STMT);
    }
}

pub(super) fn describe_end(end: SyntaxKind) -> &'static str {
    match end {
        ENDIF_KW => "`@endif`",
        ENDFOREACH_KW => "`@endforeach`",
        ENDFOR_KW => "`@endfor`",
        ENDWHILE_KW => "`@endwhile`",
        ENDSWITCH_KW => "`@endswitch`",
        _ => "the end of the directive",
    }
}
