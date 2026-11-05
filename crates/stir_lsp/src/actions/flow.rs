//! Refactorings of how code flows, and of how a name's written: flipping `if`/`else`, returning
//! early, `switch` to `match`, and importing a fully qualified name.

use stir_checker::Def;
use stir_checker::php_version as v;
use stir_hir::ast::NodeExt;
use stir_hir::name::Fqn;
use stir_syntax::{SyntaxKind::*, SyntaxNode, TextRange, TextSize};
use stir_types::{LitValue, TyKind};

use super::fix_types::{list_leaves, own_nodes};
use super::reindent::reindented;
use super::style::indent_at;
use super::types::has;
use super::{Change, Cx, Kind, Offer};
use crate::imports::Imports;
use crate::nav;

pub fn offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    let Some(token) = cx.here() else { return };
    let Some(parent) = token.parent() else { return };
    match token.kind() {
        IF_KW if parent.kind() == IF_STMT => {
            flip_offer(cx, &parent, out);
            early_return_offer(cx, &parent, out);
        }
        ELSE_KW => {
            if let Some(stmt) = parent.parent().filter(|p| p.kind() == IF_STMT) {
                flip_offer(cx, &stmt, out);
                early_return_offer(cx, &stmt, out);
            }
        }
        SWITCH_KW if parent.kind() == SWITCH_STMT => switch_to_match_offer(cx, &parent, out),
        NAME_FULLY_QUALIFIED => import_name_offer(cx, &token, out),
        _ => {}
    }
}

/// The condition of an `if`, between its parentheses.
fn condition_of(stmt: &SyntaxNode) -> Option<SyntaxNode> {
    stmt.children().next()
}

/// `cond` the other way round, as far as a condition goes.
fn negated(cx: &Cx<'_>, cond: &SyntaxNode) -> String {
    let text = cx.slice(trimmed_node(cond));
    match cond.kind() {
        // the truth of what's left is what the condition was testing
        PREFIX_EXPR if cond.has_token(BANG) => {
            if let Some(inner) = cond.children().next() {
                let inner_text = cx.slice(trimmed_node(&inner));
                if inner.kind() == PAREN_EXPR
                    && let Some(core) = inner.children().next()
                {
                    return cx.slice(trimmed_node(&core)).to_owned();
                }
                return inner_text.to_owned();
            }
            format!("!({text})")
        }
        // these have an exact opposite; `<` and such don't, with NAN and things that don't compare
        BIN_EXPR => {
            let op = cond
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| matches!(t.kind(), EQ3 | NEQ2 | EQ2 | NEQ));
            let swapped = op.as_ref().map(|t| match t.kind() {
                EQ3 => "!==",
                NEQ2 => "===",
                EQ2 => "!=",
                _ => "==",
            });
            match (op, swapped) {
                (Some(op), Some(new)) => {
                    let start = trimmed_node(cond).start();
                    let at = usize::from(op.text_range().start()) - usize::from(start);
                    let end = usize::from(op.text_range().end()) - usize::from(start);
                    format!("{}{new}{}", &text[..at], &text[end..])
                }
                _ => format!("!({text})"),
            }
        }
        VARIABLE_EXPR | CALL_EXPR | METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR
        | PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR | STATIC_PROPERTY_FETCH_EXPR
        | INDEX_EXPR | ISSET_EXPR | EMPTY_EXPR | CLASS_CONST_FETCH_EXPR | NAME_EXPR | LITERAL => {
            format!("!{text}")
        }
        INSTANCEOF_EXPR | PAREN_EXPR => {
            if cond.kind() == PAREN_EXPR {
                format!("!{text}")
            } else {
                format!("!({text})")
            }
        }
        _ => format!("!({text})"),
    }
}

/// A node's range without the whitespace and comments the tree starts or ends it with.
pub fn trimmed_node(node: &SyntaxNode) -> TextRange {
    let tokens: Vec<_> = node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .collect();
    match (tokens.first(), tokens.last()) {
        (Some(a), Some(b)) => TextRange::new(a.text_range().start(), b.text_range().end()),
        _ => node.text_range(),
    }
}

fn has_comment_between(cx: &Cx<'_>, range: TextRange) -> bool {
    let slice = cx.slice(range);
    slice.contains("//") || slice.contains("/*") || slice.contains('#')
}

/// `if (a) { x } else { y }` as `if (!a) { y } else { x }`.
fn flip_offer<'c>(cx: &'c Cx<'c>, stmt: &SyntaxNode, out: &mut Vec<Offer<'c>>) {
    if stmt.child(ELSEIF_CLAUSE).is_some() {
        return;
    }
    let Some(else_clause) = stmt.child(ELSE_CLAUSE) else {
        return;
    };
    let Some(then_block) = stmt.child(BLOCK).filter(|b| b.has_token(L_CURLY)) else {
        return;
    };
    let Some(else_block) = else_clause.child(BLOCK).filter(|b| b.has_token(L_CURLY)) else {
        return;
    };
    let Some(cond) = condition_of(stmt) else { return };
    // a comment between the parts is about one of them, but which
    let gap = TextRange::new(trimmed_node(&then_block).end(), trimmed_node(&else_block).start());
    if has_comment_between(cx, gap) {
        return;
    }
    let edits = vec![
        cx.edit(trimmed_node(&cond), negated(cx, &cond)),
        cx.edit(trimmed_node(&then_block), cx.slice(trimmed_node(&else_block))),
        cx.edit(trimmed_node(&else_block), cx.slice(trimmed_node(&then_block))),
    ];
    out.push(Offer::new(
        format!("flow.flip:{}", u32::from(stmt.text_range().start())),
        "Flip `if` and `else`",
        Kind::RefactorRewrite,
        move || Ok(Change::new(edits)),
    ));
}

/// How a place can leave what it's in without a value, if it's at the end of that: `return;` at
/// the end of a function that returns nothing, `continue;` at the end of a loop.
fn way_out(stmt: &SyntaxNode) -> Option<&'static str> {
    let block = stmt.parent().filter(|p| p.kind() == BLOCK)?;
    let last = block
        .children()
        .filter(|c| !matches!(c.kind(), EMPTY_STMT))
        .last()?;
    if &last != stmt {
        return None;
    }
    let owner = block.parent()?;
    match owner.kind() {
        WHILE_STMT | FOR_STMT | FOREACH_STMT | DO_WHILE_STMT => {
            // `continue` in a `do` goes to its condition, as the end of the body does
            Some("continue;")
        }
        FUNCTION_DECL | METHOD_DECL | CLOSURE_EXPR => {
            let ret = owner
                .child(RETURN_TYPE)
                .map(|r| r.text().to_string().to_ascii_lowercase());
            let void = ret
                .as_deref()
                .map(|r| r.trim_start_matches(':').trim() == "void");
            match void {
                Some(true) => {}
                Some(false) => return None,
                // no type: fine as long as nothing returns a value, or falling off here is odd
                None => {
                    let values = own_nodes(&block).into_iter().any(|n| {
                        n.kind() == RETURN_STMT && n.children().next().is_some()
                    });
                    if values || own_nodes(&block).iter().any(|n| matches!(n.kind(), YIELD_EXPR | YIELD_FROM_EXPR)) {
                        return None;
                    }
                }
            }
            Some("return;")
        }
        _ => None,
    }
}

/// `if (a) { everything }` at the end of something as `if (!a) { return; } everything`, and
/// `else` after a branch that leaves dropped.
fn early_return_offer<'c>(cx: &'c Cx<'c>, stmt: &SyntaxNode, out: &mut Vec<Offer<'c>>) {
    if stmt.child(ELSEIF_CLAUSE).is_some() {
        return;
    }
    let Some(then_block) = stmt.child(BLOCK).filter(|b| b.has_token(L_CURLY)) else {
        return;
    };
    let Some(cond) = condition_of(stmt) else { return };
    let outer = indent_at(cx.text, trimmed_node(stmt).start());
    let nl = cx.style.nl;
    let unit = &cx.style.unit;
    let inner_text = |block: &SyntaxNode| -> Option<String> {
        let open = block.token(L_CURLY)?;
        let close = block.token(R_CURLY)?;
        let inside = TextRange::new(open.text_range().end(), close.text_range().start());
        let text = cx.slice(inside);
        let lead = text.len() - text.trim_start_matches([' ', '\t', '\r', '\n']).len();
        let body = TextRange::new(
            inside.start() + TextSize::from(lead as u32),
            inside.start() + TextSize::from(text.trim_end().len() as u32),
        );
        if body.is_empty() {
            return Some(String::new());
        }
        Some(reindented(
            &cx.root,
            cx.text,
            body,
            &format!("{outer}{unit}"),
            &outer,
            nl,
        ))
    };
    if let Some(else_clause) = stmt.child(ELSE_CLAUSE) {
        // the `else` of something that's left by then does nothing
        let Some(else_block) = else_clause.child(BLOCK).filter(|b| b.has_token(L_CURLY)) else {
            return;
        };
        if !list_leaves(&then_block) {
            return;
        }
        let gap = TextRange::new(trimmed_node(&then_block).end(), trimmed_node(&else_block).start());
        if has_comment_between(cx, gap) {
            return;
        }
        // a name declared in it is declared either way: nothing else differs
        let Some(body) = inner_text(&else_block) else {
            return;
        };
        let range = TextRange::new(trimmed_node(&then_block).end(), trimmed_node(&else_block).end());
        let text = if body.is_empty() {
            String::new()
        } else {
            format!("{nl}{nl}{outer}{body}")
        };
        out.push(Offer::new(
            format!("flow.else:{}", u32::from(stmt.text_range().start())),
            "Remove the `else`, as the `if` leaves",
            Kind::RefactorRewrite,
            move || Ok(Change::new(vec![cx.edit(range, text)])),
        ));
        return;
    }
    if list_leaves(&then_block) {
        return;
    }
    let Some(exit) = way_out(stmt) else { return };
    let Some(body) = inner_text(&then_block) else {
        return;
    };
    if body.is_empty() {
        return;
    }
    let negation = negated(cx, &cond);
    let close = trimmed_node(stmt).end();
    let start = trimmed_node(stmt).start();
    let text = format!(
        "if ({negation}) {{{nl}{outer}{unit}{exit}{nl}{outer}}}{nl}{nl}{outer}{body}"
    );
    let range = TextRange::new(start, close);
    out.push(Offer::new(
        format!("flow.early:{}", u32::from(start)),
        format!(
            "Convert to an early {}",
            if exit == "return;" { "return" } else { "continue" }
        ),
        Kind::RefactorRewrite,
        move || Ok(Change::new(vec![cx.edit(range, text)])),
    ));
}

/// Is `==` the same as `===` between values of these, whatever they are? So far as can be told
/// from a string: one PHP wouldn't compare as a number can't be equal to another loosely unless
/// it's identical. Anything that looks at all like a number isn't that.
fn safe_string(s: &str) -> bool {
    match s.trim_start().chars().next() {
        Some(c) => !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+') && s == s.trim(),
        None => false,
    }
}

/// What a `switch` subject and its labels are, when `==` means what `===` does between them.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Compared {
    Int,
    String,
    Enum,
}

fn compared_kind(ty: &stir_types::Ty, cx: &Cx<'_>) -> Option<Compared> {
    let wide = stir_checker::widen_literals(ty);
    match wide.kind() {
        TyKind::Int | TyKind::IntRange { .. } => Some(Compared::Int),
        TyKind::String | TyKind::NonEmptyString => Some(Compared::String),
        TyKind::Class { name, .. } => {
            let c = cx.ctx().class(name)?;
            (c.kind() == stir_hir::ClassKind::Enum).then_some(Compared::Enum)
        }
        _ => None,
    }
}

/// The value a label compares against, if it's one that's the same either way.
fn label_is_safe(cx: &Cx<'_>, label: &SyntaxNode, subject: Compared) -> bool {
    let Some(ty) = cx.typed().types.get(&label.text_range()) else {
        return false;
    };
    match (subject, ty.kind()) {
        (Compared::Int, TyKind::Literal(LitValue::Int(_))) => true,
        (Compared::String, TyKind::Literal(LitValue::String(s))) => safe_string(s),
        (Compared::Enum, _) => compared_kind(ty, cx) == Some(Compared::Enum),
        _ => false,
    }
}

enum Shape {
    Return,
    Assign(String),
}

/// One arm of the `match` a `switch` can become.
struct Arm {
    labels: Vec<SyntaxNode>,
    default: bool,
    value: String,
}

/// A `switch` whose every case gives one value, the same way, as a `match`. Only where `==` and
/// `===` can't differ for what's compared, and with a `default`, without which `match` throws
/// where `switch` does nothing. Falling through to a body isn't something `match` has.
fn switch_to_match_offer<'c>(cx: &'c Cx<'c>, stmt: &SyntaxNode, out: &mut Vec<Offer<'c>>) {
    if !has(cx, v::MATCH) {
        return;
    }
    let Some(subject) = stmt.children().find(|c| c.kind() != SWITCH_CASE) else {
        return;
    };
    let typed = cx.typed();
    let Some(subject_ty) = typed.types.get(&subject.text_range()) else {
        return;
    };
    let Some(compared) = compared_kind(subject_ty, cx) else {
        return;
    };
    let cases = stmt.children_of(SWITCH_CASE);
    if cases.is_empty() {
        return;
    }
    let mut arms: Vec<Arm> = Vec::new();
    let mut pending: Vec<SyntaxNode> = Vec::new();
    let mut pending_default = false;
    let mut shape: Option<Shape> = None;
    let mut seen_default = false;
    for case in &cases {
        let is_default = case.has_token(DEFAULT_KW);
        let parts: Vec<SyntaxNode> = case.children().collect();
        let (label, body) = if is_default {
            (None, parts.as_slice())
        } else {
            let Some((first, rest)) = parts.split_first() else {
                return;
            };
            (Some(first.clone()), rest)
        };
        if let Some(label) = &label
            && !label_is_safe(cx, label, compared)
        {
            return;
        }
        if case
            .children_with_tokens()
            .any(|e| matches!(e.kind(), COMMENT | DOC_COMMENT))
        {
            return;
        }
        if let Some(l) = label {
            pending.push(l);
        }
        pending_default |= is_default;
        if body.is_empty() {
            continue;
        }
        seen_default |= pending_default;
        let value = match body {
            [only] if only.kind() == RETURN_STMT => {
                match shape {
                    Some(Shape::Assign(_)) => return,
                    _ => shape = Some(Shape::Return),
                }
                let Some(v) = only.children().next() else {
                    return;
                };
                cx.slice(trimmed_node(&v)).to_owned()
            }
            [only] if is_throw(only) => {
                let Some(e) = only.children().next() else {
                    return;
                };
                cx.slice(trimmed_node(&e)).to_owned()
            }
            [assign, end] if end.kind() == BREAK_STMT && end.children().next().is_none() => {
                let Some(e) = assign.children().next().filter(|e| {
                    assign.kind() == EXPR_STMT && e.kind() == ASSIGN_EXPR && e.has_token(EQ) && !e.has_token(AMP)
                }) else {
                    return;
                };
                let sides: Vec<SyntaxNode> = e.children().collect();
                let [left, right] = sides.as_slice() else { return };
                if left.kind() != VARIABLE_EXPR {
                    return;
                }
                let target = left.text().to_string();
                match &shape {
                    Some(Shape::Assign(t)) if *t != target => return,
                    Some(Shape::Return) => return,
                    _ => shape = Some(Shape::Assign(target)),
                }
                cx.slice(trimmed_node(right)).to_owned()
            }
            [only] if only.kind() == EXPR_STMT
                && only.children().next().is_some_and(|e| e.kind() == ASSIGN_EXPR)
                && std::ptr::eq(case, cases.last().unwrap_or(case)) =>
            {
                // the last case has no need of a `break`
                let Some(e) = only.children().next().filter(|e| e.has_token(EQ) && !e.has_token(AMP)) else {
                    return;
                };
                let sides: Vec<SyntaxNode> = e.children().collect();
                let [left, right] = sides.as_slice() else { return };
                if left.kind() != VARIABLE_EXPR {
                    return;
                }
                let target = left.text().to_string();
                match &shape {
                    Some(Shape::Assign(t)) if *t != target => return,
                    Some(Shape::Return) => return,
                    _ => shape = Some(Shape::Assign(target)),
                }
                cx.slice(trimmed_node(right)).to_owned()
            }
            _ => return,
        };
        arms.push(Arm {
            labels: std::mem::take(&mut pending),
            default: std::mem::take(&mut pending_default),
            value,
        });
    }
    // a label with nothing after it does nothing, which `match` can't say
    if !pending.is_empty() || pending_default || !seen_default {
        return;
    }
    let Some(shape) = shape else { return };
    // a value's own `$x` must not be what's being assigned to: it wouldn't have been set yet
    // either way, so that's the same. `throw` needs PHP 8.0, which `match` already does.
    let outer = indent_at(cx.text, trimmed_node(stmt).start());
    let unit = &cx.style.unit;
    let nl = cx.style.nl;
    let lines: Vec<String> = arms
        .iter()
        .map(|arm| {
            let head = if arm.default {
                "default".to_owned()
            } else {
                arm.labels
                    .iter()
                    .map(|l| cx.slice(trimmed_node(l)).to_owned())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            format!("{outer}{unit}{head} => {},", arm.value)
        })
        .collect();
    if arms.iter().any(|a| a.default && !a.labels.is_empty()) {
        return;
    }
    let lead = match shape {
        Shape::Return => "return ".to_owned(),
        Shape::Assign(t) => format!("{t} = "),
    };
    let text = format!(
        "{lead}match ({}) {{{nl}{}{nl}{outer}}};",
        cx.slice(trimmed_node(&subject)),
        lines.join(nl)
    );
    let range = trimmed_node(stmt);
    out.push(Offer::new(
        format!("flow.match:{}", u32::from(range.start())),
        "Convert `switch` to `match`",
        Kind::RefactorRewrite,
        move || Ok(Change::new(vec![cx.edit(range, text)])),
    ));
}

fn is_throw(node: &SyntaxNode) -> bool {
    node.kind() == EXPR_STMT
        && node
            .children()
            .next()
            .is_some_and(|e| e.kind() == THROW_EXPR)
}

/// The parts of `root` where a name's in the same namespace as the one at `at`, for replacing them.
fn same_namespace_range(root: &SyntaxNode, at: TextSize) -> TextRange {
    let mut start = root.text_range().start();
    let mut end = root.text_range().end();
    for ns in root.children_of(NAMESPACE_DECL) {
        let range = ns.text_range();
        if ns.child(BLOCK).is_some() {
            if range.contains_inclusive(at) {
                return range;
            }
        } else if range.start() <= at {
            start = range.start();
        } else {
            end = end.min(range.start());
            break;
        }
    }
    TextRange::new(start, end)
}

/// `\App\Foo` as `Foo`, with a `use` line.
fn import_name_offer<'c>(
    cx: &'c Cx<'c>,
    token: &stir_syntax::SyntaxToken,
    out: &mut Vec<Offer<'c>>,
) {
    let range = token.text_range();
    let at = range.start();
    let Some(imports) = Imports::new(cx.db, cx.file, at) else {
        return;
    };
    if token.parent_ancestors().any(|n| n.kind() == USE_DECL) {
        return;
    }
    let written = token.text().to_owned();
    let bare = written.trim_start_matches('\\');
    let fqn = Fqn::new(bare);
    // what it is decides how it's imported
    let def = nav::def_at(cx.db, cx.ws, cx.file, at);
    let (name, is_class) = match def.map(|(_, d)| d) {
        Some(Def::Class(_)) => (imports.class(cx.db, cx.ws, &fqn), true),
        Some(Def::Function(_)) => (imports.function(cx.db, cx.ws, &fqn), false),
        Some(Def::Const(_)) => (imports.constant(cx.db, cx.ws, &fqn), false),
        _ => {
            // a name that's in a type, a `catch`, or isn't known
            let index = stir_hir::def_index(cx.db, cx.ws);
            if !index.classes.contains_key(&fqn.key()) {
                return;
            }
            (imports.class(cx.db, cx.ws, &fqn), true)
        }
    };
    if name.text.starts_with('\\') {
        return;
    }
    let scope = same_namespace_range(&cx.root, at);
    let mut edits = Vec::new();
    for element in cx.root.descendants_with_tokens() {
        let Some(t) = element.into_token() else { continue };
        if t.kind() != NAME_FULLY_QUALIFIED
            || !scope.contains_range(t.text_range())
            || t.parent_ancestors().any(|n| n.kind() == USE_DECL)
        {
            continue;
        }
        let same = if is_class {
            t.text().eq_ignore_ascii_case(&written)
        } else {
            t.text() == written && t.text_range() == range
        };
        if same {
            edits.push(cx.edit(t.text_range(), name.text.clone()));
        }
    }
    if let Some(import) = name.import {
        edits.push(cx.insert(import.at, import.text));
    }
    out.push(Offer::new(
        format!("name.import:{}", fqn.key()),
        format!("Import `{bare}`"),
        Kind::RefactorRewrite,
        move || Ok(Change::new(edits)),
    ));
}
