//! Placing what PHPStan reports, which only has a line. It gets a fix of ours only where its
//! identifier means that fix and the line can only be about one thing: never by what's in its
//! message, and never by guessing between two.

use stir_checker::codes;
use stir_hir::ast::NodeExt;
use stir_hir::file_items;
use stir_syntax::{SyntaxKind::*, SyntaxNode, TextRange, TextSize};

use super::{Cx, Found};

/// What PHPStan reports that a method lacks `#[\Override]`, with its `checkMissingOverrideMethodAttribute`.
pub const MISSING_OVERRIDE: &str = "method.missingOverride";

fn on_line(line: TextRange, at: TextSize) -> bool {
    line.start() <= at && at < line.end().max(line.start() + TextSize::from(1))
}

fn only<T>(mut found: Vec<T>) -> Option<T> {
    if found.len() == 1 { found.pop() } else { None }
}

/// The nodes of `kinds` under `root` starting on `line`.
fn starting_on(
    root: &SyntaxNode,
    line: TextRange,
    kinds: &[stir_syntax::SyntaxKind],
) -> Vec<SyntaxNode> {
    let cover = match root.covering_element(line) {
        stir_syntax::SyntaxElement::Node(n) => n,
        stir_syntax::SyntaxElement::Token(t) => match t.parent() {
            Some(p) => p,
            None => return Vec::new(),
        },
    };
    cover
        .descendants()
        .filter(|n| kinds.contains(&n.kind()) && on_line(line, n.text_range().start()))
        .collect()
}

/// The one thing PHPStan's `code` on `line` can be about.
fn place(cx: &Cx<'_>, code: &str, line: TextRange) -> Option<TextRange> {
    let name_of = |n: &SyntaxNode| n.child(NAME).map(|x| x.text_range());
    match code {
        codes::MISSING_TYPE_RETURN | codes::MISSING_TYPE_PARAMETER | MISSING_OVERRIDE => {
            let decls = starting_on(&cx.root, line, &[FUNCTION_DECL, METHOD_DECL]);
            let named: Vec<_> = decls
                .iter()
                .filter_map(name_of)
                .filter(|r| on_line(line, r.start()))
                .collect();
            only(named)
        }
        codes::MISSING_TYPE_PROPERTY => {
            only(starting_on(&cx.root, line, &[PROPERTY_ELEM])).map(|n| n.text_range())
        }
        "deadCode.unreachable" => {
            let statements: Vec<_> = starting_on(
                &cx.root,
                line,
                &[
                    EXPR_STMT,
                    ECHO_STMT,
                    RETURN_STMT,
                    IF_STMT,
                    FOREACH_STMT,
                    WHILE_STMT,
                    FOR_STMT,
                ],
            );
            only(statements).map(|n| n.text_range())
        }
        codes::CLASS_NOT_FOUND | codes::INTERFACE_NOT_FOUND | codes::TRAIT_NOT_FOUND => {
            let index = stir_hir::def_index(cx.db, cx.ws);
            let items = file_items(cx.db, cx.file);
            let unknown: Vec<_> = starting_on(&cx.root, line, &[NAME_REF])
                .into_iter()
                .filter(|n| {
                    let keyword = n.parent().is_some_and(|p| p.kind() == USE_CLAUSE);
                    let text = n.text().to_string();
                    let text = text.trim();
                    !keyword
                        && stir_hir::type_ref::PhpBuiltin::from_name(&text.to_ascii_lowercase())
                            .is_none()
                        && !index.classes.contains_key(
                            &items
                                .scope_at(n.text_range().start())
                                .resolve_class(text)
                                .key(),
                        )
                })
                .filter(|n| {
                    n.parent().is_some_and(|p| {
                        matches!(
                            p.kind(),
                            NAMED_TYPE
                                | NAME_EXPR
                                | EXTENDS_CLAUSE
                                | IMPLEMENTS_CLAUSE
                                | CATCH_CLAUSE
                                | TRAIT_USE
                                | ATTRIBUTE
                        )
                    })
                })
                .map(|n| n.text_range())
                .collect();
            only(unknown)
        }
        codes::METHOD_NOT_FOUND => {
            let calls = starting_on(
                &cx.root,
                line,
                &[METHOD_CALL_EXPR, NULLSAFE_METHOD_CALL_EXPR],
            );
            only(calls).and_then(|c| c.children().next()?.child(NAME).map(|n| n.text_range()))
        }
        codes::STATIC_METHOD_NOT_FOUND => {
            let calls = starting_on(&cx.root, line, &[STATIC_CALL_EXPR]);
            only(calls).and_then(|c| c.children().next()?.child(NAME).map(|n| n.text_range()))
        }
        codes::PROPERTY_NOT_FOUND => {
            let fetches = starting_on(
                &cx.root,
                line,
                &[PROPERTY_FETCH_EXPR, NULLSAFE_PROPERTY_FETCH_EXPR],
            );
            let fetches: Vec<_> = fetches
                .into_iter()
                .filter(|f| {
                    !f.parent().is_some_and(|p| {
                        matches!(p.kind(), METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR)
                    })
                })
                .collect();
            only(fetches).and_then(|f| f.child(NAME).map(|n| n.text_range()))
        }
        _ => None,
    }
}

/// What can be placed of the diagnostics the editor showed around the selection.
pub fn located(cx: &Cx<'_>) -> Vec<Found> {
    let at = cx.raw_range;
    cx.foreign
        .iter()
        .filter(|d| d.range.start() <= at.end() && at.start() <= d.range.end())
        .filter_map(|d| {
            Some(Found {
                range: place(cx, &d.code, d.range)?,
                shown: d.range,
                code: d.code.clone(),
            })
        })
        .collect()
}
