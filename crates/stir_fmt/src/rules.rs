//! What goes between two tokens on a line.

use stir_syntax::{SyntaxKind, SyntaxKind::*, SyntaxToken};

/// A token, or a string that's all of a piece.
#[derive(Debug, Clone)]
pub(crate) struct Atom {
    pub kind: SyntaxKind,
    /// The node it's directly in.
    pub parent: SyntaxKind,
    pub start: usize,
    pub end: usize,
    /// `None` for what's kept whole.
    pub token: Option<SyntaxToken>,
}

/// The space between two tokens that stay on the same line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Space {
    None,
    One,
    /// As found: nothing stays nothing, and any space becomes one.
    Keep,
    /// At least one, and more where they're there: the padding of aligned columns.
    Aligned,
    /// Exactly as found.
    Verbatim,
}

pub(crate) fn is_comment(kind: SyntaxKind) -> bool {
    matches!(kind, COMMENT | DOC_COMMENT)
}

pub(crate) fn is_line_comment(atom: &Atom, source: &str) -> bool {
    atom.kind == COMMENT && !source[atom.start..atom.end].starts_with("/*")
}

/// A `{` or `}` that delimits statements or members, rather than an expression's.
pub(crate) fn is_structural_brace(atom: &Atom) -> bool {
    matches!(atom.kind, L_CURLY | R_CURLY)
        && matches!(
            atom.parent,
            BLOCK | CLASS_BODY | SWITCH_STMT | MATCH_EXPR | PROPERTY_HOOKS | TRAIT_ADAPTATIONS
        )
}

/// Keywords written against their parenthesis.
fn tight_before_paren(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        ISSET_KW
            | EMPTY_KW
            | ARRAY_KW
            | LIST_KW
            | UNSET_KW
            | EXIT_KW
            | EVAL_KW
            | DECLARE_KW
            | FN_KW
            | HALT_COMPILER_KW
            | PRINT_KW
            | CLASS_KW
            | STATIC_KW
    )
}

fn is_binary_operator(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        DOT | EQ2
            | EQ3
            | NEQ
            | NEQ2
            | L_ANGLE
            | R_ANGLE
            | GTEQ
            | LTEQ
            | SPACESHIP
            | SHL
            | SHR
            | PLUS
            | MINUS
            | STAR
            | SLASH
            | PERCENT
            | POW
            | AMP
            | PIPE
            | CARET
            | AMP2
            | PIPE2
            | PIPE_GT
            | QUESTION2
            | AND_KW
            | OR_KW
            | XOR_KW
    )
}

fn is_assign_operator(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        EQ | DOT_EQ
            | PLUS_EQ
            | MINUS_EQ
            | STAR_EQ
            | SLASH_EQ
            | PERCENT_EQ
            | POW_EQ
            | AMP_EQ
            | PIPE_EQ
            | CARET_EQ
            | SHL_EQ
            | SHR_EQ
            | QUESTION2_EQ
    )
}

/// The space between `prev` and `next`, when they stay on one line. Whatever isn't known to want
/// something stays as found.
pub(crate) fn between(prev: &Atom, next: &Atom, source: &str) -> Space {
    let (p, n) = (prev.kind, next.kind);
    if n == COMMENT || n == DOC_COMMENT {
        return if is_structural_brace(prev) {
            Space::One
        } else if p == L_PAREN || p == L_BRACK {
            Space::Keep
        } else {
            Space::Verbatim
        };
    }
    if is_comment(p) {
        return Space::Keep;
    }
    if matches!(p, OPEN_TAG | OPEN_TAG_WITH_ECHO) {
        // `<?php ` has its own space, and `<?=` doesn't
        return if p == OPEN_TAG_WITH_ECHO {
            Space::One
        } else {
            Space::Keep
        };
    }
    if n == CLOSE_TAG {
        return Space::One;
    }

    if n == SEMI {
        return Space::None;
    }
    if p == SEMI {
        // the empty parts of `for (;;)`
        return if n == R_PAREN {
            Space::None
        } else {
            Space::One
        };
    }
    if n == COMMA {
        return Space::None;
    }
    if p == COMMA {
        return if matches!(n, R_PAREN | R_BRACK | R_CURLY) {
            Space::None
        } else {
            Space::One
        };
    }
    if matches!(p, L_PAREN | L_BRACK | ATTR_OPEN) || matches!(n, R_PAREN | R_BRACK) {
        return Space::None;
    }

    // braces
    if is_structural_brace(prev) || is_structural_brace(next) {
        if p == L_CURLY && n == R_CURLY {
            return Space::None;
        }
        if p == R_CURLY && n == COMMA {
            return Space::None;
        }
        if n == L_CURLY || p == L_CURLY || n == R_CURLY || p == R_CURLY {
            if p == R_CURLY && matches!(n, ARROW | NULLSAFE_ARROW | L_PAREN | L_BRACK | COLON2) {
                return Space::None;
            }
            return Space::One;
        }
    }
    if p == L_CURLY || n == R_CURLY || n == L_CURLY {
        // `$a->{$b}`, `${'a'}`, `A\{B, C}`
        return Space::None;
    }
    if p == BACKSLASH || n == BACKSLASH {
        return Space::None;
    }

    // what names and members are made of
    if matches!(p, ARROW | NULLSAFE_ARROW | COLON2) || matches!(n, ARROW | NULLSAFE_ARROW | COLON2)
    {
        return Space::None;
    }
    if p == DOLLAR {
        return Space::None;
    }

    // operators, by the node they're in
    if p == QUESTION && prev.parent == NULLABLE_TYPE {
        return Space::None;
    }
    if matches!(next.parent, UNION_TYPE | INTERSECTION_TYPE) && matches!(n, PIPE | AMP) {
        return Space::None;
    }
    if matches!(prev.parent, UNION_TYPE | INTERSECTION_TYPE) && matches!(p, PIPE | AMP) {
        return Space::None;
    }
    if next.parent == CATCH_CLAUSE && n == PIPE || prev.parent == CATCH_CLAUSE && p == PIPE {
        return Space::Keep;
    }
    if next.parent == DECLARE_ITEM && n == EQ || prev.parent == DECLARE_ITEM && p == EQ {
        return Space::None;
    }
    if prev.parent == PREFIX_EXPR && matches!(p, BANG | PLUS | MINUS | TILDE | INC | DEC | AT | AMP)
    {
        // `- -$a` mustn't become `--$a`
        let next_text = &source[next.start..next.end];
        let merges =
            (p == MINUS && next_text.starts_with('-')) || (p == PLUS && next_text.starts_with('+'));
        return if merges { Space::One } else { Space::None };
    }
    if next.parent == POSTFIX_EXPR && matches!(n, INC | DEC) {
        return Space::None;
    }
    if p == AT {
        return Space::None;
    }
    if p == AMP && prev.parent != BIN_EXPR {
        return Space::None;
    }
    if p == ELLIPSIS {
        return Space::None;
    }
    if n == AMP && next.parent != BIN_EXPR {
        return Space::Keep;
    }
    if matches!(
        p,
        INT_CAST
            | FLOAT_CAST
            | STRING_CAST
            | BOOL_CAST
            | ARRAY_CAST
            | OBJECT_CAST
            | UNSET_CAST
            | VOID_CAST
    ) {
        return Space::One;
    }

    if n == FAT_ARROW || is_assign_operator(n) {
        return Space::Aligned;
    }
    if p == FAT_ARROW || is_assign_operator(p) {
        return Space::One;
    }
    if next.parent == BIN_EXPR && is_binary_operator(n) {
        return Space::One;
    }
    if prev.parent == BIN_EXPR && is_binary_operator(p) {
        return Space::One;
    }
    if matches!(next.parent, INSTANCEOF_EXPR) && n == INSTANCEOF_KW {
        return Space::One;
    }
    if matches!(prev.parent, INSTANCEOF_EXPR) && p == INSTANCEOF_KW {
        return Space::One;
    }

    // `a ? b : c`, `a ?: b`
    if prev.parent == SHORT_TERNARY_EXPR && next.parent == SHORT_TERNARY_EXPR && p == QUESTION {
        return Space::None;
    }
    if matches!(next.parent, TERNARY_EXPR | SHORT_TERNARY_EXPR) && matches!(n, QUESTION | COLON) {
        return Space::One;
    }
    if matches!(prev.parent, TERNARY_EXPR | SHORT_TERNARY_EXPR) && matches!(p, QUESTION | COLON) {
        return Space::One;
    }
    // every other colon: return types, named arguments, enums' backing types, labels and cases
    if n == COLON {
        return Space::None;
    }
    if p == COLON {
        return match prev.parent {
            RETURN_TYPE | NAMED_ARG | ENUM_DECL => Space::One,
            _ => Space::Keep,
        };
    }

    if n == USE_KW && next.parent == CLOSURE_USE {
        return Space::One;
    }
    if n == L_BRACK && next.parent == INDEX_EXPR {
        return Space::None;
    }
    if n == L_PAREN {
        if p.is_keyword() {
            if tight_before_paren(p) {
                return if p == STATIC_KW || p == CLASS_KW {
                    Space::Keep
                } else {
                    Space::None
                };
            }
            if prev.parent == MODIFIERS {
                return Space::None;
            }
            return Space::One;
        }
        return match p {
            IDENT | NAME_QUALIFIED | NAME_FULLY_QUALIFIED | NAME_RELATIVE | VARIABLE | R_PAREN
            | R_BRACK | R_CURLY => Space::None,
            _ => Space::Keep,
        };
    }
    Space::Keep
}
