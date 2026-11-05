//! Extracting an expression to a variable, inlining a variable, and removing an unused one. Each
//! has to leave the same things evaluated in the same order, and refuses what it can't be sure of.

use rustc_hash::FxHashSet;
use stir_checker::model_api;
use stir_hir::ast::NodeExt;
use stir_syntax::{SyntaxKind, SyntaxKind::*, SyntaxNode, SyntaxToken, TextRange, TextSize};

use super::fix_types::{is_statement_list, own_nodes};
use super::flow::trimmed_node;
use super::style::indent_at;
use super::{Change, Cx, Kind, Offer};
use crate::nav;

/// Could evaluating `node` do anything but give a value, or read other than what it names?
/// Conservative: a property might be magic, but that's not something PHP's order of evaluation
/// should depend on in code anyone would want refactored, whereas a call might do anything.
pub fn is_pure(node: &SyntaxNode) -> bool {
    node.descendants().all(|n| {
        matches!(
            n.kind(),
            LITERAL
                | STRING_LITERAL
                | VARIABLE_EXPR
                | NAME_EXPR
                | NAME_REF
                | NAME
                | ARRAY_EXPR
                | ARRAY_ELEM
                | PAREN_EXPR
                | BIN_EXPR
                | TERNARY_EXPR
                | SHORT_TERNARY_EXPR
                | CAST_EXPR
                | INSTANCEOF_EXPR
                | PROPERTY_FETCH_EXPR
                | NULLSAFE_PROPERTY_FETCH_EXPR
                | STATIC_PROPERTY_FETCH_EXPR
                | CLASS_CONST_FETCH_EXPR
                | INDEX_EXPR
                | ISSET_EXPR
                | EMPTY_EXPR
                | MAGIC_CONST
                | INTERP_STRING
                | INTERP_VAR
                | INTERP_EXPR
                | CLOSURE_EXPR
                | ARROW_FN_EXPR
        ) || n.kind() == PREFIX_EXPR
            && !n.has_token(INC)
            && !n.has_token(DEC)
    })
}

/// Is `node` a constant value, the same wherever and whenever it's evaluated?
fn is_constant(node: &SyntaxNode) -> bool {
    node.descendants().all(|n| {
        matches!(
            n.kind(),
            LITERAL | STRING_LITERAL | ARRAY_EXPR | ARRAY_ELEM | PAREN_EXPR | NAME_EXPR | NAME_REF
                | NAME | CLASS_CONST_FETCH_EXPR | MAGIC_CONST
        )
    })
}

/// Can `node`, the place an expression's value is read, be given a variable instead, with its
/// expression evaluated before the statement? Everything between it and the statement has to
/// always evaluate it, once, and evaluate nothing with effects before it.
fn extractable_place(expr: &SyntaxNode) -> Result<SyntaxNode, &'static str> {
    let mut child = expr.clone();
    loop {
        let parent = child.parent().ok_or("It isn't in a statement.")?;
        let kind = parent.kind();
        if is_statement_list(kind) {
            return Ok(child);
        }
        let earlier = || {
            parent
                .children()
                .take_while(|c| c != &child)
                .collect::<Vec<_>>()
        };
        match kind {
            // these decide whether what's in them runs at all
            TERNARY_EXPR | SHORT_TERNARY_EXPR | MATCH_ARM | MATCH_CONDITIONS => {
                if earlier().is_empty() {
                    // the condition itself always runs
                } else {
                    return Err("It's only evaluated on some paths.");
                }
            }
            BIN_EXPR => {
                let conditional = parent.children_with_tokens().any(|e| {
                    matches!(
                        e.kind(),
                        AMP2 | PIPE2 | AND_KW | OR_KW | QUESTION2 | XOR_KW
                    )
                });
                if conditional && !earlier().is_empty() {
                    return Err("It's only evaluated on some paths.");
                }
                if parent.has_token(QUESTION2) {
                    return Err("`??` treats what's on its left differently.");
                }
            }
            ARROW_FN_EXPR | CLOSURE_EXPR | ANON_CLASS => {
                return Err("It's in a function that runs later.");
            }
            ISSET_EXPR | EMPTY_EXPR | UNSET_STMT | LIST_EXPR | ERROR_SUPPRESS_EXPR | CLOSURE_USE => {
                return Err("What's around it treats it differently from a value.");
            }
            YIELD_EXPR | YIELD_FROM_EXPR => {
                // fine: it's evaluated before it's yielded
            }
            // the body of one without braces: the condition's fine, as it always runs
            IF_STMT | ELSE_CLAUSE | FOREACH_STMT | SWITCH_STMT | DECLARE_STMT => {
                if !earlier().is_empty() || kind == ELSE_CLAUSE {
                    return Err("A variable can't be put before it there.");
                }
            }
            // what's run again each time around, or not in the order written
            WHILE_STMT | DO_WHILE_STMT | FOR_STMT | FOR_INIT | FOR_COND | FOR_STEP => {
                return Err("It's evaluated each time around a loop.");
            }
            ELSEIF_CLAUSE | SWITCH_CASE | CATCH_CLAUSE | FINALLY_CLAUSE | PARAM | PARAM_LIST
            | PROPERTY_ELEM | CONST_ELEM | ATTRIBUTE | ARG_LIST
                if kind != ARG_LIST =>
            {
                return Err("A variable can't be put before it there.");
            }
            ECHO_TAG_STMT | STATIC_VAR | STATIC_VAR_STMT | GLOBAL_STMT | INLINE_HTML_STMT => {
                return Err("A variable can't be put before it there.");
            }
            _ => {}
        }
        // anything evaluated before it has to be something that moving it past can't disturb
        for before in earlier() {
            let target_of_assign = kind == ASSIGN_EXPR
                && parent.children().next().as_ref() == Some(&before)
                && matches!(before.kind(), VARIABLE_EXPR);
            if !is_pure(&before) && !target_of_assign {
                return Err("Something before it has effects it would now run ahead of.");
            }
        }
        // a value it reads could be set before it, in the same statement
        child = parent;
    }
}

/// Would giving the value of `expr` a variable change what's done with it?
pub fn takes_reference(expr: &SyntaxNode) -> bool {
    let Some(parent) = expr.parent() else {
        return false;
    };
    let is_place = matches!(
        expr.kind(),
        VARIABLE_EXPR | INDEX_EXPR | PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR
            | STATIC_PROPERTY_FETCH_EXPR
    );
    match parent.kind() {
        ASSIGN_EXPR => {
            parent.children().next().as_ref() == Some(expr) || parent.has_token(AMP)
        }
        PREFIX_EXPR | POSTFIX_EXPR => {
            parent.has_token(INC) || parent.has_token(DEC) || parent.has_token(AMP)
        }
        // what a function does to its arguments depends on it: it may take them by reference
        ARG | NAMED_ARG => is_place,
        ARRAY_ELEM => parent.has_token(AMP) || is_place && {
            // `[$a, $b] = ...`
            parent
                .ancestors()
                .find(|a| a.kind() == ASSIGN_EXPR)
                .is_some()
        },
        FOREACH_STMT => is_place,
        // `new` and calls of a name can't have a variable where the name is
        NEW_EXPR | CLASS_CONST_FETCH_EXPR | STATIC_PROPERTY_FETCH_EXPR | INSTANCEOF_EXPR => {
            expr.kind() == NAME_EXPR
        }
        // the callee, which isn't a value
        CALL_EXPR | METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR => {
            parent.children().next().as_ref() == Some(expr)
                && !(parent.kind() == CALL_EXPR && expr.kind() != NAME_EXPR)
        }
        RETURN_STMT => is_place && {
            parent
                .ancestors()
                .find(|a| matches!(a.kind(), FUNCTION_DECL | METHOD_DECL))
                .is_some_and(|f| f.has_token(AMP))
        },
        _ => false,
    }
}

/// The node that is exactly `range`, the outermost of those there are.
pub fn expression_at(cx: &Cx<'_>, range: TextRange) -> Option<SyntaxNode> {
    if range.is_empty() {
        return None;
    }
    let node = match cx.root.covering_element(range) {
        stir_syntax::SyntaxElement::Node(n) => n,
        stir_syntax::SyntaxElement::Token(t) => t.parent()?,
    };
    node.ancestors()
        .take_while(|n| trimmed_node(n) == range || n.text_range().contains_range(range))
        .filter(|n| trimmed_node(n) == range)
        .filter(|n| is_expression(n.kind()))
        .last()
}

pub fn is_expression(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        LITERAL | STRING_LITERAL | INTERP_STRING | HEREDOC | SHELL_EXEC | VARIABLE_EXPR
            | VARIABLE_VARIABLE | NAME_EXPR | ARRAY_EXPR | PAREN_EXPR | BIN_EXPR | PREFIX_EXPR
            | POSTFIX_EXPR | ASSIGN_EXPR | TERNARY_EXPR | SHORT_TERNARY_EXPR | CAST_EXPR
            | INSTANCEOF_EXPR | CALL_EXPR | METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR
            | PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR | STATIC_CALL_EXPR
            | STATIC_PROPERTY_FETCH_EXPR | CLASS_CONST_FETCH_EXPR | INDEX_EXPR | NEW_EXPR
            | CLONE_EXPR | CLOSURE_EXPR | ARROW_FN_EXPR | MATCH_EXPR | ISSET_EXPR | EMPTY_EXPR
            | PRINT_EXPR | ERROR_SUPPRESS_EXPR | MAGIC_CONST | LIST_EXPR | INCLUDE_EXPR
            | EVAL_EXPR | EXIT_EXPR | THROW_EXPR | YIELD_EXPR | YIELD_FROM_EXPR
    )
}

/// All variable names in the function-like around a place, or the file for top-level code.
pub fn names_in_scope(cx: &Cx<'_>, node: &SyntaxNode) -> FxHashSet<String> {
    let scope = nav::enclosing_function(node).unwrap_or_else(|| cx.root.clone());
    scope
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == VARIABLE)
        .map(|t| t.text().trim_start_matches('$').to_owned())
        .collect()
}

/// A name for what `expr` is, that nothing here's using.
fn name_for(cx: &Cx<'_>, expr: &SyntaxNode) -> String {
    let used = names_in_scope(cx, expr);
    let lower_first = |word: &str| {
        let mut c = word.chars();
        c.next().map_or_else(String::new, |f| {
            format!("{}{}", f.to_lowercase(), c.as_str())
        })
    };
    let base = match expr.kind() {
        METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR => {
            expr.children().next().and_then(|f| f.name_text()).map(|n| {
                let rest = n
                    .strip_prefix("get")
                    .filter(|r| r.starts_with(char::is_uppercase))
                    .unwrap_or(&n);
                lower_first(rest)
            })
        }
        PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR | CLASS_CONST_FETCH_EXPR => {
            expr.name_text().map(|n| lower_first(&n.to_ascii_lowercase()))
        }
        CALL_EXPR => expr
            .children()
            .next()
            .map(|c| c.text().to_string())
            .and_then(|t| t.trim().rsplit('\\').next().map(|n| lower_first(n))),
        NEW_EXPR => expr
            .children()
            .find(|c| c.kind() == NAME_EXPR)
            .map(|c| c.text().to_string())
            .and_then(|t| t.trim().rsplit('\\').next().map(lower_first)),
        _ => None,
    };
    let base = base
        .filter(|b| {
            b.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
                && b.chars().all(|c| c.is_alphanumeric() || c == '_' || !c.is_ascii())
        })
        .unwrap_or_else(|| "value".to_owned());
    let mut name = base.clone();
    let mut n = 2;
    while used.contains(&name) || name == "this" {
        name = format!("{base}{n}");
        n += 1;
    }
    name
}

pub fn extract_offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    let Some(expr) = expression_at(cx, cx.range) else {
        return;
    };
    // nothing's gained by a variable for a variable
    if matches!(expr.kind(), VARIABLE_EXPR | ASSIGN_EXPR) {
        return;
    }
    const TITLE: &str = "Extract to a variable";
    let id = format!("extract.variable:{}", u32::from(cx.range.start()));
    let refuse = |reason: &str| Offer::refused(id.clone(), TITLE, Kind::RefactorExtract, reason);
    if takes_reference(&expr) {
        out.push(refuse("Something needs this place, not only its value."));
        return;
    }
    if matches!(expr.kind(), THROW_EXPR | YIELD_EXPR | YIELD_FROM_EXPR | EXIT_EXPR | LIST_EXPR | INCLUDE_EXPR) {
        out.push(refuse("It isn't an ordinary value."));
        return;
    }
    let statement = match extractable_place(&expr) {
        Ok(s) => s,
        Err(reason) => {
            out.push(refuse(reason));
            return;
        }
    };
    if matches!(
        statement.kind(),
        ECHO_TAG_STMT | INLINE_HTML_STMT | CONST_DECL | USE_DECL | NAMESPACE_DECL | FUNCTION_DECL
            | CLASS_DECL | INTERFACE_DECL | TRAIT_DECL | ENUM_DECL | DECLARE_STMT
    ) {
        out.push(refuse("A variable can't be put before it there."));
        return;
    }
    let name = name_for(cx, &expr);
    let expr_range = cx.range;
    let text = cx.slice(expr_range).to_owned();
    // the same thing again later in the statement is the same value, if nothing can change it
    let mut also = Vec::new();
    if is_pure(&expr) && !assigns_anything(&statement) {
        for other in statement.descendants() {
            if other.text_range().start() > expr_range.end()
                && other.kind() == expr.kind()
                && cx.slice(trimmed_node(&other)) == text
                && !other.ancestors().any(|a| matches!(a.kind(), CLOSURE_EXPR | ARROW_FN_EXPR | ANON_CLASS))
                && !takes_reference(&other)
                && !other.ancestors().any(|a| matches!(a.kind(), ISSET_EXPR | EMPTY_EXPR | INTERP_STRING | HEREDOC))
                && !also.iter().any(|r: &TextRange| r.contains_range(other.text_range()))
            {
                also.push(trimmed_node(&other));
            }
        }
    }
    // a doc comment about the statement stays with it, and an inline `@var` applies to the next one
    let doc = statement
        .doc_comment()
        .map(|d| d.text_range().start());
    let start = doc.unwrap_or_else(|| super::docs::first_token_start(&statement));
    let indent = indent_at(cx.text, start);
    let nl = cx.style.nl;
    let mut edits = vec![
        cx.insert(start, format!("${name} = {text};{nl}{indent}")),
        cx.edit(expr_range, format!("${name}")),
    ];
    edits.extend(also.into_iter().map(|r| cx.edit(r, format!("${name}"))));
    out.push(Offer::new(id, TITLE, Kind::RefactorExtract, move || {
        let mut change = Change::new(edits);
        // the variable, where it's being assigned
        change.rename_at = Some(start + TextSize::from(1));
        Ok(change)
    }));
}

/// Does `node` assign anything, or step it up or down?
fn assigns_anything(node: &SyntaxNode) -> bool {
    node.descendants().any(|n| {
        n.kind() == ASSIGN_EXPR
            || matches!(n.kind(), PREFIX_EXPR | POSTFIX_EXPR)
                && n.children_with_tokens().any(|e| matches!(e.kind(), INC | DEC))
    })
}

/// What's known of every occurrence of a variable in its function.
struct Occurrences {
    /// The assignment statement, if there's exactly one and it's a plain statement.
    assign: SyntaxNode,
    value: SyntaxNode,
    /// The tokens reading it afterwards.
    reads: Vec<SyntaxToken>,
}

/// Is what's in `function` something a variable can't be followed through?
pub fn dynamic_scope(function: &SyntaxNode) -> bool {
    function.descendants().any(|n| match n.kind() {
        VARIABLE_VARIABLE | GLOBAL_STMT | EVAL_EXPR | INCLUDE_EXPR => true,
        CALL_EXPR => n.children().next().is_some_and(|c| {
            let text = c.text().to_string();
            let lower = text.trim().trim_start_matches('\\').to_ascii_lowercase();
            matches!(
                lower.as_str(),
                "compact" | "extract" | "get_defined_vars" | "parse_str" | "func_get_args"
                    | "func_get_arg" | "mb_parse_str"
            )
        }),
        _ => false,
    })
}

fn occurrences(cx: &Cx<'_>, function: &SyntaxNode, name: &str) -> Option<Occurrences> {
    let body = function.child(BLOCK)?;
    if dynamic_scope(function) {
        return None;
    }
    // parameters of this function, and anything captured
    if function
        .child(PARAM_LIST)
        .is_some_and(|p| p.descendants_with_tokens().any(|e| e.as_token().is_some_and(|t| t.kind() == VARIABLE && t.text() == name)))
    {
        return None;
    }
    let mut assign = None;
    let mut reads = Vec::new();
    for t in body
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == VARIABLE && t.text() == name)
    {
        let parent = t.parent()?;
        // in a closure, by `use` or as its own, which is more than this follows
        if t.parent_ancestors()
            .any(|n| matches!(n.kind(), CLOSURE_EXPR | ANON_CLASS | CLOSURE_USE_VAR))
        {
            return None;
        }
        // in a string, or a `static`, which can't take an expression
        if parent.kind() != VARIABLE_EXPR
            || t.parent_ancestors()
                .any(|n| matches!(n.kind(), INTERP_STRING | HEREDOC | SHELL_EXEC))
        {
            return None;
        }
        let up = parent.parent()?;
        let is_target = up.kind() == ASSIGN_EXPR && up.children().next().as_ref() == Some(&parent);
        if is_target {
            if assign.is_some() || !up.has_token(EQ) || up.has_token(AMP) {
                return None;
            }
            let statement = up.parent().filter(|s| {
                s.kind() == EXPR_STMT && s.parent().is_some_and(|l| is_statement_list(l.kind()))
            })?;
            assign = Some((statement, up.children().nth(1)?));
            continue;
        }
        // anything that might write it
        if matches!(
            up.kind(),
            POSTFIX_EXPR | PREFIX_EXPR | ARRAY_ELEM | LIST_EXPR | FOREACH_STMT | UNSET_STMT
                | CATCH_CLAUSE | ISSET_EXPR | EMPTY_EXPR
        ) && !matches!(up.kind(), PREFIX_EXPR if !up.has_token(INC) && !up.has_token(DEC) && !up.has_token(AMP))
        {
            return None;
        }
        if matches!(up.kind(), ARG | NAMED_ARG) {
            // it may be taken by reference: a variable's fine for that, an expression isn't
            if !by_value(cx, &up) {
                return None;
            }
        }
        if matches!(up.kind(), INDEX_EXPR | PROPERTY_FETCH_EXPR)
            && up.parent().is_some_and(|g| {
                g.kind() == ASSIGN_EXPR && g.children().next().as_ref() == Some(&up)
            })
        {
            return None;
        }
        if up.kind() == ASSIGN_EXPR && up.has_token(AMP) {
            return None;
        }
        reads.push(t);
    }
    let (assign, value) = assign?;
    Some(Occurrences {
        assign,
        value,
        reads,
    })
}

/// Does what's called take this argument by value, as far as the checker can tell which it is?
pub fn by_value(cx: &Cx<'_>, arg: &SyntaxNode) -> bool {
    let Some(list) = arg.parent() else { return false };
    let Some(call) = list.parent() else { return false };
    let callee = match call.kind() {
        CALL_EXPR => call.children().next().map(|c| c.text_range()),
        METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR => call
            .children()
            .next()
            .and_then(|f| f.child(NAME))
            .map(|n| n.text_range()),
        _ => None,
    };
    let Some(callee) = callee else { return false };
    let refs = nav::file_refs(cx.db, cx.ws, cx.file);
    let Some((_, def)) = refs.iter().find(|(r, _)| *r == callee) else {
        return false;
    };
    let sig = match def {
        stir_checker::Def::Function(loc) => model_api::function_info(cx.ctx(), *loc),
        stir_checker::Def::Member {
            class,
            kind: stir_checker::MemberKind::Method,
            name,
        } => match model_api::method_info(cx.ctx(), class, name) {
            Some(sig) => sig,
            None => return false,
        },
        _ => return false,
    };
    if arg.kind() == NAMED_ARG {
        let name = arg.name_text().unwrap_or_default();
        return sig.params.iter().find(|p| p.name == name).is_some_and(|p| !p.by_ref);
    }
    let index = list
        .children()
        .filter(|c| matches!(c.kind(), ARG | NAMED_ARG))
        .position(|c| &c == arg);
    let Some(index) = index else { return false };
    let param = sig
        .params
        .get(index)
        .or_else(|| sig.params.last().filter(|p| p.variadic));
    param.is_some_and(|p| !p.by_ref)
}

/// What it takes to put `value` where a variable is read: parentheses, unless nothing around it
/// could bind tighter.
fn value_for(cx: &Cx<'_>, value: &SyntaxNode, read: &SyntaxToken) -> Option<String> {
    let text = cx.slice(trimmed_node(value)).to_owned();
    let atom = matches!(
        value.kind(),
        LITERAL | STRING_LITERAL | INTERP_STRING | VARIABLE_EXPR | NAME_EXPR | ARRAY_EXPR
            | PAREN_EXPR | CALL_EXPR | METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR
            | STATIC_CALL_EXPR | PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR
            | STATIC_PROPERTY_FETCH_EXPR | CLASS_CONST_FETCH_EXPR | INDEX_EXPR | MAGIC_CONST
            | ISSET_EXPR | EMPTY_EXPR
    );
    let up = read.parent()?.parent()?;
    let stands_alone = matches!(
        up.kind(),
        RETURN_STMT | EXPR_STMT | ECHO_STMT | ARG | ARRAY_ELEM | IF_STMT | PAREN_EXPR | NAMED_ARG
    ) || up.kind() == ASSIGN_EXPR;
    if atom || stands_alone {
        Some(text)
    } else {
        Some(format!("({text})"))
    }
}

pub fn inline_offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    let Some(token) = cx.here().filter(|t| t.kind() == VARIABLE) else {
        return;
    };
    let name = token.text().to_owned();
    if name == "$this" {
        return;
    }
    let Some(parent) = token.parent() else { return };
    let Some(function) = nav::enclosing_function(&parent) else {
        return;
    };
    if function.kind() == ARROW_FN_EXPR {
        return;
    }
    let Some(found) = occurrences(cx, &function, &name) else {
        return;
    };
    let Occurrences {
        assign,
        value,
        reads,
    } = found;
    let assign_end = assign.text_range().end();
    if reads.is_empty() || reads.iter().any(|r| r.text_range().start() < assign_end) {
        return;
    }
    // the value's own variable isn't what's being replaced
    if value
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .any(|t| t.kind() == VARIABLE && t.text() == name)
    {
        return;
    }
    if !is_constant(&value) {
        // something that can change or do something: only into the very next thing that runs,
        // when that's the first thing it does
        let [read] = reads.as_slice() else { return };
        let Some(next) = assign.next_sibling() else {
            return;
        };
        if !next.text_range().contains_range(read.text_range()) {
            return;
        }
        if !runs_first(read, &next) {
            return;
        }
    }
    // the assignment has to run for it to have this value, wherever it's read
    let list = assign.parent();
    let in_same_list = reads.iter().all(|r| {
        r.parent_ancestors()
            .find(|n| n.parent() == list)
            .is_some()
    });
    if !in_same_list {
        return;
    }
    // loops around the assignment don't matter: it and its reads are inside the same list
    let mut edits = Vec::new();
    for read in &reads {
        let Some(text) = value_for(cx, &value, read) else {
            return;
        };
        edits.push(cx.edit(read.text_range(), text));
    }
    let range = statement_lines(cx, &assign);
    edits.push(cx.edit(range, ""));
    out.push(Offer::new(
        format!("inline.variable:{name}"),
        format!("Inline `{name}`"),
        Kind::RefactorInline,
        move || Ok(Change::new(edits)),
    ));
}

/// Is `read` the first thing evaluated in `stmt`, always evaluated?
fn runs_first(read: &SyntaxToken, stmt: &SyntaxNode) -> bool {
    let Some(mut child) = read.parent() else {
        return false;
    };
    while &child != stmt {
        let Some(parent) = child.parent() else {
            return false;
        };
        let conditional = matches!(
            parent.kind(),
            TERNARY_EXPR | SHORT_TERNARY_EXPR | MATCH_ARM | MATCH_EXPR | IF_STMT | ELSE_CLAUSE
                | ELSEIF_CLAUSE | WHILE_STMT | DO_WHILE_STMT | FOR_STMT | FOREACH_STMT
                | SWITCH_STMT | SWITCH_CASE | TRY_STMT | CATCH_CLAUSE | FINALLY_CLAUSE | BLOCK
                | CLOSURE_EXPR | ARROW_FN_EXPR
        );
        let first_child = parent.children().next().as_ref() == Some(&child);
        if conditional && !first_child {
            return false;
        }
        if parent.kind() == BLOCK || parent.kind() == FOREACH_STMT && !first_child {
            return false;
        }
        for before in parent.children().take_while(|c| c != &child) {
            if !is_constant(&before) {
                // `$y = $x`: the target isn't evaluated first in any way that matters
                let target = parent.kind() == ASSIGN_EXPR && before.kind() == VARIABLE_EXPR;
                if !target {
                    return false;
                }
            }
        }
        child = parent;
    }
    true
}

/// The lines `stmt` is on, if it has them to itself, without a comment before it.
fn statement_lines(cx: &Cx<'_>, stmt: &SyntaxNode) -> TextRange {
    let core = TextRange::new(super::docs::first_token_start(stmt), stmt.text_range().end());
    super::organize::whole_lines(cx, core)
}

/// Removing `$x = <something with no effects>;`, offered with the cursor on `$x`.
pub fn remove_unused_offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    let Some(token) = cx.here().filter(|t| t.kind() == VARIABLE) else {
        return;
    };
    let name = token.text().to_owned();
    if name == "$this" {
        return;
    }
    let Some(parent) = token.parent() else { return };
    // top-level code's variables are globals that anything might read
    let Some(function) = nav::enclosing_function(&parent).filter(|f| f.child(BLOCK).is_some())
    else {
        return;
    };
    let Some(found) = occurrences(cx, &function, &name) else {
        return;
    };
    if !found.reads.is_empty() || !is_pure_value(&found.value) {
        return;
    }
    if own_nodes(&function).iter().any(|n| n.kind() == STATIC_VAR_STMT) {
        return;
    }
    let range = statement_lines(cx, &found.assign);
    out.push(Offer::new(
        format!("unused.variable:{name}"),
        format!("Remove unused variable `{name}`"),
        Kind::QuickFix,
        move || Ok(Change::new(vec![cx.edit(range, "")])),
    ));
}

/// Is `node` something evaluating which can't do anything, even warn?
fn is_pure_value(node: &SyntaxNode) -> bool {
    is_constant(node) || matches!(node.kind(), CLOSURE_EXPR | ARROW_FN_EXPR)
}
