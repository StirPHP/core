//! Extract method, and extract function for code outside classes.
//!
//! The selection is whole statements or one expression. What it reads and writes of the variables
//! around it is worked out from the tree, in the order things run; what can't be followed is
//! refused, with the reason shown, since a refusal costs far less than changing what code does:
//!
//! - **Inputs** are parameters: what it reads before it has certainly written it, if that can have
//!   a value by then. A type is written only where it's certain at the start of the selection: the
//!   type of a parameter of the function around it that isn't reassigned, or of a variable with
//!   one unconditional assignment before it. What a variable has been narrowed to in between is
//!   no use, as a narrower type could throw where the code didn't.
//! - **Outputs** are what it writes that's looked at afterwards. One is returned; several are
//!   returned as an array and destructured.
//! - **`return`** is only taken if every path through the selection returns, so that the call can
//!   take its place. `break` and `continue` out of it, `goto`, `yield`, references to what's used
//!   in it, and anything that reaches variables by name (`compact`, `extract`, `$$x`, `eval`, ...)
//!   are refused. So is a selection in a closure, whose `$this` might be bound to anything.
//!
//! The method is `static` if it doesn't need `$this`. The name is one nothing related uses, and an
//! editor that can rename it right away; see [`Change::rename_at`].

use rustc_hash::FxHashMap;
use stir_checker::php_version as v;
use stir_hir::ast::NodeExt;
use stir_hir::name::Fqn;
use stir_hir::{ClassDef, file_items};
use stir_syntax::{SyntaxKind::*, SyntaxNode, SyntaxToken, TextRange, TextSize};
use stir_types::Ty;

use super::fix_members::{insert_written_member, member_indent};
use super::fix_types::{always_leaves, is_statement_list, written_type};
use super::flow::trimmed_node;
use super::hierarchy::class_of;
use super::reindent::reindented;
use super::style::indent_at;
use super::types::{Namer, has, native};
use super::variables::{by_value, dynamic_scope, expression_at, takes_reference};
use super::{Change, Cx, FileEdit, Kind, Offer, Refusal};
use crate::hierarchy::all_subtypes;
use crate::imports;
use crate::nav;

const BASE_NAME: &str = "extracted";

pub fn offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    if cx.range.is_empty() {
        return;
    }
    let Some(selection) = read_selection(cx) else {
        return;
    };
    let in_class = selection.anchor.ancestors().any(|n| n.kind() == CLASS_BODY);
    let title = if in_class {
        "Extract method"
    } else {
        "Extract function"
    };
    let id = format!(
        "extract.function:{}:{}",
        u32::from(cx.range.start()),
        u32::from(cx.range.end())
    );
    match plan(cx, &selection) {
        Ok(plan) => out.push(Offer::new(id, title, Kind::RefactorExtract, move || {
            build(cx, &selection, plan)
        })),
        Err(reason) => out.push(Offer::refused(id, title, Kind::RefactorExtract, reason)),
    }
}

/// What's selected.
struct Selection {
    /// The statements, or the one expression.
    nodes: Vec<SyntaxNode>,
    expression: bool,
    /// All of the selection, comments included.
    range: TextRange,
    anchor: SyntaxNode,
}

fn only_trivia_between(cx: &Cx<'_>, from: TextSize, to: TextSize) -> bool {
    if from >= to {
        return true;
    }
    let range = TextRange::new(from, to);
    let tokens = cx
        .root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token());
    tokens
        .filter(|t| {
            t.text_range()
                .intersect(range)
                .is_some_and(|r| !r.is_empty())
        })
        .all(|t| t.kind().is_trivia())
}

fn read_selection(cx: &Cx<'_>) -> Option<Selection> {
    if let Some(expr) = expression_at(cx, cx.range) {
        // a statement is what's meant if it's all there's to it
        let whole_statement = expr
            .parent()
            .is_some_and(|p| p.kind() == EXPR_STMT && trimmed_node(&p).end() == cx.range.end());
        if !whole_statement {
            return Some(Selection {
                anchor: expr.clone(),
                nodes: vec![expr],
                expression: true,
                range: cx.range,
            });
        }
    }
    let cover = match cx.root.covering_element(cx.range) {
        stir_syntax::SyntaxElement::Node(n) => n,
        stir_syntax::SyntaxElement::Token(t) => t.parent()?,
    };
    let list = cover.ancestors().find(|n| is_statement_list(n.kind()))?;
    let nodes: Vec<SyntaxNode> = list
        .children()
        .filter(|c| is_statement_node(c.kind()))
        .filter(|c| {
            let t = trimmed_node(c);
            t.intersect(cx.range).is_some_and(|r| !r.is_empty())
        })
        .collect();
    let first = nodes.first()?;
    let last = nodes.last()?;
    let start = trimmed_node(first).start();
    let end = trimmed_node(last).end();
    // all of them, whole, with at most comments beyond
    if cx.range.start() > start
        || cx.range.end() < end
        || !only_trivia_between(cx, cx.range.start(), start)
        || !only_trivia_between(cx, end, cx.range.end())
    {
        return None;
    }
    Some(Selection {
        anchor: first.clone(),
        nodes,
        expression: false,
        range: cx.range,
    })
}

fn is_statement_node(kind: stir_syntax::SyntaxKind) -> bool {
    matches!(
        kind,
        EXPR_STMT
            | ECHO_STMT
            | RETURN_STMT
            | IF_STMT
            | WHILE_STMT
            | DO_WHILE_STMT
            | FOR_STMT
            | FOREACH_STMT
            | SWITCH_STMT
            | BREAK_STMT
            | CONTINUE_STMT
            | TRY_STMT
            | GLOBAL_STMT
            | STATIC_VAR_STMT
            | UNSET_STMT
            | GOTO_STMT
            | LABEL_STMT
            | EMPTY_STMT
            | INLINE_HTML_STMT
            | ECHO_TAG_STMT
            | FUNCTION_DECL
            | CLASS_DECL
            | INTERFACE_DECL
            | TRAIT_DECL
            | ENUM_DECL
            | DECLARE_STMT
            | CONST_DECL
            | BLOCK
            | USE_DECL
            | NAMESPACE_DECL
            | HALT_COMPILER_STMT
    )
}

/// One thing a selection does to a variable, in the order it runs.
#[derive(Clone)]
struct Event {
    name: String,
    reads: bool,
    writes: bool,
    /// It might not run.
    conditional: bool,
}

/// What it needs known of the selection, in a walk over it in the order it runs.
struct Walk<'a, 'c> {
    cx: &'a Cx<'c>,
    events: Vec<Event>,
    uses_this: bool,
    uses_parent: bool,
    refusal: Option<Refusal>,
}

fn is_superglobal(name: &str) -> bool {
    matches!(
        name,
        "$GLOBALS"
            | "$_SERVER"
            | "$_GET"
            | "$_POST"
            | "$_FILES"
            | "$_COOKIE"
            | "$_SESSION"
            | "$_REQUEST"
            | "$_ENV"
    )
}

impl Walk<'_, '_> {
    fn refuse(&mut self, why: impl Into<String>) {
        if self.refusal.is_none() {
            self.refusal = Some(why.into());
        }
    }

    fn event(&mut self, token: &SyntaxToken, reads: bool, writes: bool, conditional: bool) {
        let name = token.text();
        if name == "$this" {
            self.uses_this = true;
            return;
        }
        if is_superglobal(name) {
            return;
        }
        self.events.push(Event {
            name: name.to_owned(),
            reads,
            writes,
            conditional,
        });
    }

    fn node_variable(&mut self, node: &SyntaxNode, reads: bool, writes: bool, conditional: bool) {
        if let Some(t) = node.token(VARIABLE) {
            self.event(&t, reads, writes, conditional);
        } else {
            self.walk(node, conditional);
        }
    }

    /// What's being assigned to.
    fn target(&mut self, node: &SyntaxNode, conditional: bool, compound: bool) {
        match node.kind() {
            VARIABLE_EXPR => self.node_variable(node, compound, true, conditional),
            ARRAY_EXPR | LIST_EXPR => {
                for elem in node.children() {
                    if elem.kind() == ARRAY_ELEM {
                        if elem.has_token(AMP) {
                            self.refuse("It takes a reference to what it's destructuring.");
                        }
                        // the last is the target, the first a key that's read
                        let parts: Vec<SyntaxNode> = elem.children().collect();
                        if let Some((target, keys)) = parts.split_last() {
                            for k in keys {
                                self.walk(k, conditional);
                            }
                            self.target(target, conditional, false);
                        }
                    } else {
                        self.target(&elem, conditional, false);
                    }
                }
            }
            INDEX_EXPR => {
                // `$a[$i] = ...` writes `$a`, and needs what it had
                let mut parts = node.children();
                if let Some(base) = parts.next() {
                    self.target(&base, conditional, true);
                }
                for rest in parts {
                    self.walk(&rest, conditional);
                }
            }
            // what's written is the object, which `$o` still is after
            _ => self.walk(node, conditional),
        }
    }

    fn walk(&mut self, node: &SyntaxNode, conditional: bool) {
        match node.kind() {
            RETURN_STMT | FUNCTION_DECL | CLASS_DECL | INTERFACE_DECL | TRAIT_DECL | ENUM_DECL => {
                if node.kind() != RETURN_STMT {
                    self.refuse("It declares something, which PHP does when it reads the file.");
                    return;
                }
                self.children(node, conditional);
            }
            VARIABLE_VARIABLE => self.refuse("It reaches variables by name (`$$x`)."),
            YIELD_EXPR | YIELD_FROM_EXPR => {
                self.refuse("It has a `yield`, which only works in this function.");
            }
            GOTO_STMT | LABEL_STMT => self.refuse("It has a `goto` or a label."),
            GLOBAL_STMT => self.refuse("It has a `global`, whose variables go with this function."),
            STATIC_VAR_STMT => {
                self.refuse("It has a `static` variable, which goes with this function.")
            }
            INLINE_HTML_STMT | ECHO_TAG_STMT => self.refuse("It has HTML in it."),
            EVAL_EXPR | INCLUDE_EXPR => {
                self.refuse("It has an `eval` or an `include`, which see all the variables.");
            }
            MAGIC_CONST => {
                if node.has_token(FUNCTION_C_KW)
                    || node.has_token(METHOD_C_KW)
                    || node.has_token(LINE_KW)
                {
                    self.refuse("It has `__FUNCTION__`, `__METHOD__` or `__LINE__`, whose values would change.");
                }
            }
            CALL_EXPR => {
                let callee = node.children().next();
                if let Some(c) = &callee {
                    let name = c.text().to_string();
                    let lower = name.trim().trim_start_matches('\\').to_ascii_lowercase();
                    if matches!(
                        lower.as_str(),
                        "compact"
                            | "extract"
                            | "get_defined_vars"
                            | "parse_str"
                            | "mb_parse_str"
                            | "func_get_args"
                            | "func_get_arg"
                            | "func_num_args"
                    ) {
                        self.refuse(format!(
                            "It calls `{}`, which reaches this function's variables or arguments.",
                            name.trim()
                        ));
                        return;
                    }
                }
                self.children(node, conditional);
            }
            ASSIGN_EXPR => {
                let parts: Vec<SyntaxNode> = node.children().collect();
                let [left, right] = parts.as_slice() else {
                    self.refuse("It has an assignment this can't read.");
                    return;
                };
                if node.has_token(AMP) {
                    self.refuse("It makes a reference, which can't be followed.");
                    return;
                }
                let plain = node.has_token(EQ);
                let lazy = node.has_token(QUESTION2_EQ);
                if plain {
                    // what's assigned runs first, for what's here
                    self.walk(right, conditional);
                    self.target(left, conditional, false);
                } else {
                    self.target(left, conditional, true);
                    self.walk(right, conditional || lazy);
                }
            }
            PREFIX_EXPR | POSTFIX_EXPR if node.has_token(INC) || node.has_token(DEC) => {
                if let Some(operand) = node.children().next() {
                    self.target(&operand, conditional, true);
                }
            }
            ARG | NAMED_ARG => {
                // `$x` for a parameter that may be by reference may be written
                let value = node.children().filter(|c| c.kind() != NAME).last();
                let Some(value) = value else { return };
                if matches!(value.kind(), VARIABLE_EXPR | INDEX_EXPR) && !by_value(self.cx, node) {
                    self.target(&value, conditional, true);
                } else {
                    self.walk(&value, conditional);
                }
            }
            UNSET_STMT => {
                for target in node.children() {
                    if target.kind() == VARIABLE_EXPR {
                        self.refuse("It unsets a variable, which can't be returned.");
                    } else {
                        self.walk(&target, conditional);
                    }
                }
            }
            IF_STMT | ELSEIF_CLAUSE | WHILE_STMT | SWITCH_STMT | MATCH_EXPR => {
                let mut first = true;
                for c in node.children() {
                    self.walk(&c, conditional || !first);
                    first = false;
                }
            }
            DO_WHILE_STMT | FOR_STEP | FOR_COND | ELSE_CLAUSE | SWITCH_CASE | MATCH_ARM
            | TRY_STMT | CATCH_CLAUSE | FINALLY_CLAUSE => {
                if node.kind() == CATCH_CLAUSE
                    && let Some(v) = node.token(VARIABLE)
                {
                    self.event(&v, false, true, true);
                }
                self.children(node, true);
            }
            FOR_STMT => {
                for c in node.children() {
                    let first_part = c.kind() == FOR_INIT;
                    self.walk(&c, conditional || !first_part);
                }
            }
            FOREACH_STMT => {
                let parts: Vec<SyntaxNode> = node.children().collect();
                let Some((subject, rest)) = parts.split_first() else {
                    return;
                };
                if node.has_token(AMP) {
                    self.refuse("A `foreach` in it takes references.");
                    return;
                }
                self.walk(subject, conditional);
                for part in rest {
                    if part.kind() == BLOCK || is_statement_node(part.kind()) {
                        self.walk(part, true);
                    } else {
                        self.target(part, true, false);
                    }
                }
            }
            BIN_EXPR => {
                let lazy = node
                    .children_with_tokens()
                    .any(|e| matches!(e.kind(), AMP2 | PIPE2 | AND_KW | OR_KW | QUESTION2));
                let mut first = true;
                for c in node.children() {
                    self.walk(&c, conditional || (lazy && !first));
                    first = false;
                }
            }
            TERNARY_EXPR | SHORT_TERNARY_EXPR => {
                let mut first = true;
                for c in node.children() {
                    self.walk(&c, conditional || !first);
                    first = false;
                }
            }
            ANON_CLASS => {
                // what's passed to it is read here; what's in it is its own
                if let Some(args) = node.child(ARG_LIST) {
                    self.walk(&args, conditional);
                }
            }
            CLOSURE_EXPR => {
                let is_static = node.has_token(STATIC_KW);
                if !is_static
                    && node.descendants_with_tokens().any(|e| {
                        e.as_token()
                            .is_some_and(|t| t.kind() == VARIABLE && t.text() == "$this")
                    })
                {
                    self.uses_this = true;
                }
                if let Some(uses) = node.child(CLOSURE_USE) {
                    for var in uses.children_of(CLOSURE_USE_VAR) {
                        if var.has_token(AMP) {
                            self.refuse("A closure in it takes a variable by reference.");
                        }
                        if let Some(t) = var.token(VARIABLE) {
                            self.event(&t, true, false, conditional);
                        }
                    }
                }
            }
            ARROW_FN_EXPR => {
                let params: Vec<String> = node
                    .child(PARAM_LIST)
                    .map(|l| {
                        l.children_of(PARAM)
                            .iter()
                            .filter_map(|p| p.token(VARIABLE).map(|t| t.text().to_owned()))
                            .collect()
                    })
                    .unwrap_or_default();
                let body = node
                    .children()
                    .filter(|c| {
                        c.kind() != PARAM_LIST
                            && !stir_hir::is_type_node(c.kind())
                            && c.kind() != RETURN_TYPE
                    })
                    .last();
                let Some(body) = body else { return };
                for t in body
                    .descendants_with_tokens()
                    .filter_map(|e| e.into_token())
                    .filter(|t| t.kind() == VARIABLE)
                {
                    if !params.iter().any(|p| p == t.text()) {
                        self.event(&t, true, false, true);
                    }
                }
            }
            NAME_EXPR => {
                let text = node.text().to_string();
                if text.trim().eq_ignore_ascii_case("parent") {
                    self.uses_parent = true;
                }
            }
            _ => {
                // `self::$x` is a property
                if node.kind() != STATIC_PROPERTY_FETCH_EXPR
                    && let Some(t) = node.token(VARIABLE)
                {
                    self.event(&t, true, false, conditional);
                }
                self.children(node, conditional);
            }
        }
    }

    fn children(&mut self, node: &SyntaxNode, conditional: bool) {
        for c in node.children() {
            self.walk(&c, conditional);
        }
    }
}

/// What a selection needs and gives.
struct Plan {
    inputs: Vec<Input>,
    outputs: Vec<Output>,
    is_static: bool,
    /// The function has to return what the call does: every path in it returns.
    returning: Returning,
    /// The native return type, if one can be said.
    return_type: Option<String>,
    doc_return: Option<String>,
    imports: Vec<FileEdit>,
    name: String,
    function: Option<SyntaxNode>,
}

#[derive(PartialEq, Eq)]
enum Returning {
    No,
    Value,
    Bare,
    /// It always leaves, but not by returning.
    Never,
}

struct Input {
    name: String,
    ty: Option<String>,
    doc: Option<String>,
}

struct Output {
    name: String,
    ty: Option<String>,
    doc: Option<String>,
}

/// Every occurrence of variables in `function` outside `range`, by name.
fn outside_occurrences(
    function: &SyntaxNode,
    range: TextRange,
) -> FxHashMap<String, Vec<TextRange>> {
    let mut out: FxHashMap<String, Vec<TextRange>> = FxHashMap::default();
    let mut stack = vec![function.clone()];
    while let Some(node) = stack.pop() {
        for element in node.children_with_tokens() {
            match element {
                stir_syntax::SyntaxElement::Token(t) => {
                    let property = t
                        .parent()
                        .is_some_and(|p| p.kind() == STATIC_PROPERTY_FETCH_EXPR);
                    if t.kind() == VARIABLE && !property && !range.contains_range(t.text_range()) {
                        out.entry(t.text().to_owned())
                            .or_default()
                            .push(t.text_range());
                    }
                }
                stir_syntax::SyntaxElement::Node(n) => {
                    // a closure's own aren't these, unless it's handed them
                    if n.kind() == CLOSURE_EXPR {
                        if let Some(u) = n.child(CLOSURE_USE) {
                            stack.push(u);
                        }
                        continue;
                    }
                    if matches!(n.kind(), CLASS_BODY | FUNCTION_DECL) && n != *function {
                        continue;
                    }
                    if !range.contains_range(n.text_range()) {
                        stack.push(n);
                    }
                }
            }
        }
    }
    out
}

/// Does a name in `function` have a reference taken to it, or a meaning this can't follow?
fn tied_up(function: &SyntaxNode, name: &str) -> bool {
    function.descendants().any(|n| match n.kind() {
        ASSIGN_EXPR if n.has_token(AMP) => n
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == VARIABLE && t.text() == name),
        CLOSURE_USE_VAR if n.has_token(AMP) => n.token(VARIABLE).is_some_and(|t| t.text() == name),
        GLOBAL_STMT | STATIC_VAR_STMT => n
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == VARIABLE && t.text() == name),
        FOREACH_STMT if n.has_token(AMP) => n.children().filter(|c| c.kind() != BLOCK).any(|c| {
            c.descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .any(|t| t.kind() == VARIABLE && t.text() == name)
        }),
        ARRAY_ELEM if n.has_token(AMP) => n
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == VARIABLE && t.text() == name),
        _ => false,
    })
}

/// The loop around `node` within `function`, outermost.
fn outer_loop(node: &SyntaxNode, function: &SyntaxNode) -> Option<SyntaxNode> {
    node.ancestors()
        .skip(1)
        .take_while(|a| a != function)
        .filter(|a| {
            matches!(
                a.kind(),
                WHILE_STMT | DO_WHILE_STMT | FOR_STMT | FOREACH_STMT
            )
        })
        .last()
}

/// Does anything in `node` jump out of it: a `break` or `continue` of a loop it isn't in?
fn jumps_out(node: &SyntaxNode, depth: u64) -> bool {
    let levels = |stmt: &SyntaxNode| -> u64 {
        stmt.children()
            .next()
            .and_then(|l| l.text().to_string().trim().parse::<u64>().ok())
            .unwrap_or(1)
    };
    match node.kind() {
        BREAK_STMT | CONTINUE_STMT => levels(node) > depth,
        CLOSURE_EXPR | ARROW_FN_EXPR | CLASS_BODY | FUNCTION_DECL => false,
        WHILE_STMT | DO_WHILE_STMT | FOR_STMT | FOREACH_STMT | SWITCH_STMT => {
            node.children().any(|c| jumps_out(&c, depth + 1))
        }
        _ => node.children().any(|c| jumps_out(&c, depth)),
    }
}

/// Is `name` certainly set when the selection starts: a parameter, a variable of a `foreach` or
/// `catch` it's in, or assigned by a statement that always runs before it?
fn defined_on_every_path(
    scope: &SyntaxNode,
    function: Option<&SyntaxNode>,
    name: &str,
    selection: &Selection,
) -> bool {
    let start = selection.range.start();
    if let Some(f) = function {
        let params = f
            .child(PARAM_LIST)
            .map(|l| l.children_of(PARAM))
            .unwrap_or_default();
        if params
            .iter()
            .any(|p| p.token(VARIABLE).is_some_and(|t| t.text() == name))
        {
            return true;
        }
        if f.child(CLOSURE_USE).is_some_and(|u| {
            u.children_of(CLOSURE_USE_VAR)
                .iter()
                .any(|v| v.token(VARIABLE).is_some_and(|t| t.text() == name))
        }) {
            return true;
        }
    }
    let mut child = selection.anchor.clone();
    for parent in selection.anchor.ancestors().skip(1) {
        match parent.kind() {
            // in the body of these, their variables are set
            FOREACH_STMT if child.kind() == BLOCK || is_statement_node(child.kind()) => {
                let declares = parent
                    .children()
                    .skip(1)
                    .filter(|c| c.kind() != BLOCK && !is_statement_node(c.kind()))
                    .any(|c| {
                        c.descendants_with_tokens()
                            .filter_map(|e| e.into_token())
                            .any(|t| t.kind() == VARIABLE && t.text() == name)
                    });
                if declares {
                    return true;
                }
            }
            CATCH_CLAUSE => {
                if parent.token(VARIABLE).is_some_and(|t| t.text() == name) {
                    return true;
                }
            }
            kind if is_statement_list(kind) => {
                for earlier in parent.children().take_while(|c| c != &child) {
                    if earlier.text_range().end() > start {
                        break;
                    }
                    let sets = earlier.kind() == EXPR_STMT
                        && earlier
                            .children()
                            .next()
                            .is_some_and(|e| writes_directly(&e, name) && e.kind() == ASSIGN_EXPR);
                    if sets {
                        return true;
                    }
                }
            }
            _ => {}
        }
        if &parent == scope {
            break;
        }
        child = parent;
    }
    false
}

/// The type a variable certainly has when it gets to the selection, if that can be told.
fn input_type(
    cx: &Cx<'_>,
    namer: &mut Namer<'_>,
    function: Option<&SyntaxNode>,
    name: &str,
    selection: &Selection,
) -> Option<(String, Option<String>)> {
    let function = function?;
    let start = selection.range.start();
    // around a loop, it's whatever the last time made it
    if let Some(around) = outer_loop(&selection.anchor, function)
        && around.descendants().any(|n| writes_directly(&n, name))
    {
        return None;
    }
    let writes_before: Vec<SyntaxNode> = function
        .descendants()
        .filter(|n| n.text_range().end() <= start)
        .filter(|n| writes_directly(n, name))
        .collect();
    let param = function
        .child(PARAM_LIST)?
        .children_of(PARAM)
        .into_iter()
        .find(|p| p.token(VARIABLE).is_some_and(|t| t.text() == name));
    if let Some(param) = param {
        if !writes_before.is_empty() || param.has_token(ELLIPSIS) || param.has_token(AMP) {
            // what's in a variadic is an array of those
            return None;
        }
        let ty = written_type(&param)?;
        let text = ty.text().to_string().trim().to_owned();
        // with the types this leaves as they are, this reads the same anywhere in the class;
        // what PHPDoc says of it is the function's, and could depend on its templates
        return Some((text, None));
    }
    let [only] = writes_before.as_slice() else {
        return None;
    };
    // `$x = ...;` as a statement of a list that the selection is further on in
    let statement = only.parent().filter(|s| s.kind() == EXPR_STMT)?;
    let list = statement.parent()?;
    if !is_statement_list(list.kind()) || !selection.anchor.ancestors().any(|a| a == list) {
        return None;
    }
    let value = only.children().nth(1)?;
    // `@var` on it says what it is, whatever the value says
    let declared = statement
        .doc_comment()
        .is_some_and(|d| d.text().contains("@var"));
    let ty = if declared {
        selection
            .nodes
            .iter()
            .flat_map(|n| n.descendants())
            .find(|n| n.kind() == VARIABLE_EXPR && n.text().to_string().trim() == name)
            .and_then(|read| cx.typed().types.get(&read.text_range()))?
    } else {
        cx.typed().types.get(&value.text_range())?
    };
    written_of(cx, namer, ty)
}

/// The native type of a value of `ty`, and what PHPDoc can say better of it.
fn written_of(cx: &Cx<'_>, namer: &mut Namer<'_>, ty: &Ty) -> Option<(String, Option<String>)> {
    if ty.is_untyped() || ty.contains_error() {
        return None;
    }
    let w = native(cx, namer, ty)?;
    // an array that's empty so far says nothing of what it's for: `array`, not `list<never>`
    let doc = w.doc.filter(|d| !d.contains("<never>"));
    (!matches!(w.native.as_str(), "void" | "never" | "null")).then_some((w.native, doc))
}

/// Is `node` a plain or compound assignment of the variable `name`?
fn writes_directly(node: &SyntaxNode, name: &str) -> bool {
    match node.kind() {
        ASSIGN_EXPR => node.children().next().is_some_and(|t| {
            t.kind() == VARIABLE_EXPR && t.text() == name
                || (matches!(t.kind(), ARRAY_EXPR | LIST_EXPR | INDEX_EXPR)
                    && t.descendants_with_tokens()
                        .filter_map(|e| e.into_token())
                        .any(|v| v.kind() == VARIABLE && v.text() == name))
        }),
        POSTFIX_EXPR | PREFIX_EXPR if node.has_token(INC) || node.has_token(DEC) => {
            node.text() == name
        }
        FOREACH_STMT => node
            .children()
            .skip(1)
            .filter(|c| c.kind() != BLOCK)
            .any(|c| c.text() == name),
        _ => false,
    }
}

fn plan(cx: &Cx<'_>, selection: &Selection) -> Result<Plan, Refusal> {
    let anchor = &selection.anchor;
    let function = nav::enclosing_function(anchor);
    if let Some(f) = &function {
        if matches!(f.kind(), CLOSURE_EXPR | ARROW_FN_EXPR) {
            return Err(
                "It's in a closure, whose `$this` and variables aren't the method's.".into(),
            );
        }
        // in a nested function in a method, there's no class to speak of
        if f.kind() == FUNCTION_DECL && anchor.ancestors().any(|n| n.kind() == CLASS_BODY) {
            return Err("It's in a function inside a method.".into());
        }
        if f.has_token(AMP) {
            return Err("The function around it returns by reference.".into());
        }
        if anchor
            .ancestors()
            .take_while(|a| a != f)
            .any(|a| a.kind() == PARAM)
        {
            return Err("It's a parameter's default, which has to be constant.".into());
        }
    }
    if anchor.ancestors().any(|a| {
        matches!(
            a.kind(),
            PROPERTY_ELEM | CONST_ELEM | ATTRIBUTE | ENUM_CASE | STATIC_VAR | DECLARE_ITEM | PARAM
        )
    }) {
        return Err("It has to be constant where it is.".into());
    }
    if function.is_none() && anchor.ancestors().any(|a| a.kind() == CLASS_BODY) {
        return Err("It isn't in a method.".into());
    }
    let scope = function.clone().unwrap_or_else(|| cx.root.clone());
    if dynamic_scope(&scope) {
        return Err("The function around it reaches its variables by name, so what this uses can't be told.".into());
    }
    if function.is_none() && cx.root.descendants().any(|n| n.kind() == INLINE_HTML_STMT) {
        return Err("The file has HTML in it, so there's nowhere to put a function.".into());
    }
    if selection.expression {
        let expr = &selection.nodes[0];
        if takes_reference(expr) {
            return Err("Something needs this place, not only its value.".into());
        }
        if expr.ancestors().skip(1).any(|a| {
            matches!(
                a.kind(),
                ISSET_EXPR
                    | EMPTY_EXPR
                    | UNSET_STMT
                    | LIST_EXPR
                    | INTERP_STRING
                    | HEREDOC
                    | INTERP_EXPR
                    | SHELL_EXEC
            )
        }) {
            return Err("What's around it treats it differently from a value.".into());
        }
        if let Some(p) = expr.parent()
            && p.kind() == BIN_EXPR
            && p.has_token(QUESTION2)
            && p.children().next().as_ref() == Some(expr)
        {
            return Err("`??` treats what's on its left differently.".into());
        }
    }
    let mut walk = Walk {
        cx,
        events: Vec::new(),
        uses_this: false,
        uses_parent: false,
        refusal: None,
    };
    for node in &selection.nodes {
        walk.walk(node, false);
    }
    if let Some(reason) = walk.refusal {
        return Err(reason);
    }
    for node in &selection.nodes {
        if jumps_out(node, 0) {
            return Err("It has a `break` or `continue` of a loop outside it.".into());
        }
        if node
            .descendants()
            .any(|n| n.kind() == LABEL_STMT || n.kind() == GOTO_STMT)
        {
            return Err("It has a `goto` or a label.".into());
        }
    }
    // the variables, in the order they're first looked at
    let mut order: Vec<String> = Vec::new();
    let mut needs_value: FxHashMap<String, bool> = FxHashMap::default();
    let mut definite: FxHashMap<String, bool> = FxHashMap::default();
    let mut written: FxHashMap<String, bool> = FxHashMap::default();
    for e in &walk.events {
        if !order.contains(&e.name) {
            order.push(e.name.clone());
        }
        let done = definite.get(&e.name).copied().unwrap_or(false);
        if e.reads && !done {
            needs_value.insert(e.name.clone(), true);
        }
        if e.writes {
            written.insert(e.name.clone(), true);
            if !e.conditional {
                definite.insert(e.name.clone(), true);
            }
        }
    }
    let outside = outside_occurrences(&scope, selection.range);
    let start = selection.range.start();
    let end = selection.range.end();
    let around_loop = outer_loop(anchor, &scope);
    let mut namer = Namer::new(cx, start);
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for name in &order {
        if tied_up(&scope, name) {
            return Err(format!(
                "`{name}` has a reference to it, or is `global` or `static`, so it can't be followed."
            ));
        }
        let occurs = outside.get(name.as_str());
        let before = occurs.is_some_and(|o| o.iter().any(|r| r.start() < start));
        let after = occurs.is_some_and(|o| {
            o.iter().any(|r| {
                r.start() >= end
                    // around again, what's before it runs after it
                    || around_loop
                        .as_ref()
                        .is_some_and(|l| l.text_range().contains_range(*r))
            })
        });
        let writes = written.get(name.as_str()).copied().unwrap_or(false);
        let needs = needs_value.get(name.as_str()).copied().unwrap_or(false);
        let is_definite = definite.get(name.as_str()).copied().unwrap_or(false);
        if needs && !before && around_loop.is_some() && writes {
            return Err(format!(
                "`{name}` carries over from one time around the loop to the next."
            ));
        }
        let output = writes && after;
        if selection.expression && output {
            return Err(format!("It assigns `{name}`, which is used elsewhere."));
        }
        if output && !is_definite && !before {
            return Err(format!(
                "`{name}` is only set on some paths, and is used afterwards."
            ));
        }
        if (needs && before) || (output && !is_definite && before) {
            // passing what might not be set is a warning where reading it wasn't
            if !defined_on_every_path(&scope, function.as_ref(), name, selection) {
                return Err(format!(
                    "`{name}` might not be set by the time it gets here."
                ));
            }
            let (ty, doc) = input_type(cx, &mut namer, function.as_ref(), name, selection)
                .map_or((None, None), |(t, d)| (Some(t), d));
            inputs.push(Input {
                name: name.clone(),
                ty,
                doc,
            });
        }
        if output {
            let (ty, doc) = output_type(cx, &mut namer, selection, name)
                .map_or((None, None), |(t, d)| (Some(t), d));
            outputs.push(Output {
                name: name.clone(),
                ty,
                doc,
            });
        }
    }

    // how it ends
    let returns: Vec<SyntaxNode> = selection
        .nodes
        .iter()
        .flat_map(|n| std::iter::once(n.clone()).chain(super::fix_types::own_nodes(n)))
        .filter(|n| n.kind() == RETURN_STMT)
        .collect();
    // it can be a call of what never comes back, too, like `abort()`
    let ends_in_never = selection.nodes.last().is_some_and(|n| {
        n.kind() == EXPR_STMT
            && n.children()
                .next()
                .and_then(|e| cx.typed().types.get(&e.text_range()).cloned())
                .is_some_and(|t| t.is_never())
    });
    let leaves =
        !selection.expression && (selection.nodes.iter().any(always_leaves) || ends_in_never);
    // what comes after isn't run once this has left: nothing it sets is wanted there
    if leaves {
        outputs.clear();
    }
    let mut returning = Returning::No;
    if !returns.is_empty() {
        if !leaves {
            return Err("It has a `return`, but not on every path.".into());
        }
        let valued = returns
            .iter()
            .filter(|r| r.children().next().is_some())
            .count();
        returning = if valued == returns.len() {
            Returning::Value
        } else if valued == 0 {
            Returning::Bare
        } else {
            return Err("Some of its `return`s have a value and some don't.".into());
        };
    } else if leaves {
        returning = Returning::Never;
    }
    let declared_return = function
        .as_ref()
        .and_then(|f| f.child(RETURN_TYPE))
        .and_then(|r| written_type(&r))
        .map(|t| t.text().to_string().trim().to_owned());
    // what the function says of itself that the checker has an issue with isn't worth repeating
    let declared_doc = function
        .as_ref()
        .filter(|f| !return_doc_is_wrong(cx, f))
        .and_then(return_tag);
    let (return_type, doc_return) = match returning {
        Returning::Value => match declared_return {
            Some(t) => (Some(t), declared_doc),
            None => {
                let typed = cx.typed();
                let mut types = Vec::new();
                for r in &returns {
                    match r
                        .children()
                        .next()
                        .and_then(|v| typed.types.get(&v.text_range()))
                    {
                        Some(t) if !t.is_untyped() && !t.contains_error() => types.push(t.clone()),
                        _ => {
                            types.clear();
                            break;
                        }
                    }
                }
                let ty = (!types.is_empty())
                    .then(|| Ty::union(types.iter().map(stir_checker::widen_literals)));
                match ty.and_then(|t| written_of(cx, &mut namer, &t)) {
                    Some((t, doc)) => (Some(t), doc),
                    None => (None, None),
                }
            }
        },
        Returning::Bare => (Some("void".to_owned()), None),
        Returning::Never => {
            if has(cx, v::NEVER_TYPE) {
                (Some("never".to_owned()), None)
            } else {
                // the code after the call would look reachable to whatever reads it
                return Err("It always throws or exits, and `never` isn't on this version of PHP, so the code after a call of it would seem to run.".into());
            }
        }
        // what's returned as it is: what the function says it returns says what this does
        Returning::No
            if selection.expression
                && declared_return.is_some()
                && selection.nodes[0]
                    .parent()
                    .is_some_and(|p| p.kind() == RETURN_STMT) =>
        {
            (declared_return, declared_doc)
        }
        Returning::No if selection.expression => {
            let expr = &selection.nodes[0];
            let ty = cx.typed().types.get(&expr.text_range());
            match ty {
                Some(t) if t.is_never() => return Err("It never gives a value.".into()),
                Some(t) if matches!(t.kind(), stir_types::TyKind::Void) => {
                    return Err("It doesn't give a value.".into());
                }
                Some(t) => match written_of(cx, &mut namer, t) {
                    Some((t, doc)) => (Some(t), doc),
                    None => (None, None),
                },
                None => (None, None),
            }
        }
        Returning::No => match outputs.as_slice() {
            [] => (Some("void".to_owned()), None),
            [one] => (one.ty.clone(), one.doc.clone()),
            many => {
                let types: Option<Vec<String>> = many
                    .iter()
                    .map(|o| o.doc.clone().or_else(|| o.ty.clone()))
                    .collect();
                (
                    Some("array".to_owned()),
                    types.map(|t| format!("array{{{}}}", t.join(", "))),
                )
            }
        },
    };
    // readonly properties are only for the class's own scope, which this is, but what reads this
    // as a check takes them to be for the constructor
    if function
        .as_ref()
        .and_then(|f| f.name_text())
        .is_some_and(|n| n.eq_ignore_ascii_case("__construct"))
        && let Some(class) = class_of(cx, anchor)
    {
        let readonly_set = selection.nodes.iter().any(|n| {
            n.descendants().any(|a| {
                a.kind() == ASSIGN_EXPR
                    && a.children().next().is_some_and(|t| {
                        t.kind() == PROPERTY_FETCH_EXPR
                            && t.name_text().is_some_and(|name| {
                                class.mods.is_readonly
                                    || class
                                        .properties
                                        .iter()
                                        .any(|p| p.name == name && p.mods.is_readonly)
                                    || class.methods.iter().any(|m| {
                                        m.sig.params.iter().any(|p| {
                                            p.name == name
                                                && p.promoted.as_ref().is_some_and(|x| x.immutable)
                                        })
                                    })
                            })
                    })
            })
        });
        if readonly_set {
            return Err("It sets a `readonly` property in the constructor.".into());
        }
    }
    let in_class = anchor.ancestors().any(|n| n.kind() == CLASS_BODY);
    let name = if in_class {
        unique_method_name(
            cx,
            class_of(cx, anchor).ok_or("It isn't in a class this knows.")?,
        )
    } else {
        unique_function_name(cx, start)
    };
    let enclosing_static = function
        .as_ref()
        .filter(|f| f.kind() == METHOD_DECL)
        .and_then(|f| f.child(MODIFIERS))
        .is_some_and(|m| m.has_token(STATIC_KW));
    if enclosing_static && walk.uses_this {
        return Err("It uses `$this` in a static method.".into());
    }
    let is_static = in_class && (enclosing_static || (!walk.uses_this && !walk.uses_parent));
    Ok(Plan {
        inputs,
        outputs,
        is_static,
        returning,
        return_type,
        doc_return,
        imports: namer.edits(),
        name,
        function,
    })
}

/// Is what `function`'s doc comment says it returns reported as not fitting its native type?
fn return_doc_is_wrong(cx: &Cx<'_>, function: &SyntaxNode) -> bool {
    let range = function.text_range();
    stir_checker::check_file(cx.db, cx.ws, cx.file)
        .iter()
        .any(|d| d.code == "return.phpDocType" && range.contains_range(d.range))
}

/// What a function's doc comment says it returns, as the tag is written: `@return T`.
fn return_tag(function: &SyntaxNode) -> Option<String> {
    let doc = function.doc_comment()?;
    let tags: Vec<String> = doc
        .text()
        .lines()
        .map(|l| {
            l.trim()
                .trim_start_matches("/**")
                .trim_start_matches('*')
                .trim()
                .trim_end_matches("*/")
                .trim()
                .to_owned()
        })
        .filter(|l| {
            l.starts_with("@return ")
                || l.starts_with("@phpstan-return ")
                || l.starts_with("@psalm-return ")
        })
        .collect();
    match tags.as_slice() {
        [] => None,
        // the more particular wins, as PHPStan has it
        [one] => Some(one.clone()),
        many => many
            .iter()
            .find(|t| t.starts_with("@phpstan-"))
            .or(many.first())
            .cloned(),
    }
}

/// The type of `name` once the selection's done, if one unconditional assignment in it says.
fn output_type(
    cx: &Cx<'_>,
    namer: &mut Namer<'_>,
    selection: &Selection,
    name: &str,
) -> Option<(String, Option<String>)> {
    let writes: Vec<SyntaxNode> = selection
        .nodes
        .iter()
        .flat_map(|n| n.descendants())
        .filter(|n| writes_directly(n, name))
        .collect();
    let [only] = writes.as_slice() else {
        return None;
    };
    if only.kind() != ASSIGN_EXPR
        || !only.has_token(EQ)
        || only
            .children()
            .next()
            .is_none_or(|t| t.kind() != VARIABLE_EXPR)
    {
        return None;
    }
    let statement = only.parent().filter(|s| s.kind() == EXPR_STMT)?;
    if !selection.nodes.contains(&statement) {
        return None;
    }
    let value = only.children().nth(1)?;
    let ty = cx.typed().types.get(&value.text_range())?;
    written_of(cx, namer, ty)
}

fn unique_method_name(cx: &Cx<'_>, class: &ClassDef) -> String {
    let key = class.name.key();
    let taken = |name: &str| {
        let self_ty = cx.ctx().class(&key).map(|c| c.self_ty());
        if self_ty.is_some_and(|t| cx.ctx().find_method(&t, name).is_some()) {
            return true;
        }
        all_subtypes(cx.db, cx.ws, &key).iter().any(|sub| {
            crate::hierarchy::class_def(cx.db, cx.ws, sub)
                .is_some_and(|(_, c)| c.methods.iter().any(|m| m.name.eq_ignore_ascii_case(name)))
        })
    };
    let mut name = BASE_NAME.to_owned();
    let mut n = 2;
    while taken(&name) {
        name = format!("{BASE_NAME}{n}");
        n += 1;
    }
    name
}

fn unique_function_name(cx: &Cx<'_>, at: TextSize) -> String {
    let index = stir_hir::def_index(cx.db, cx.ws);
    let scope = file_items(cx.db, cx.file).scope_at(at);
    let mut name = BASE_NAME.to_owned();
    let mut n = 2;
    while index
        .functions
        .contains_key(&Fqn::join(&scope.namespace, &name).key())
    {
        name = format!("{BASE_NAME}{n}");
        n += 1;
    }
    name
}

fn build(cx: &Cx<'_>, selection: &Selection, plan: Plan) -> Result<Change, Refusal> {
    let anchor = &selection.anchor;
    let nl = cx.style.nl;
    let args: Vec<String> = plan.inputs.iter().map(|i| i.name.clone()).collect();
    let params: Vec<String> = plan
        .inputs
        .iter()
        .map(|i| match &i.ty {
            Some(t) => format!("{t} {}", i.name),
            None => i.name.clone(),
        })
        .collect();
    let in_class = anchor.ancestors().any(|n| n.kind() == CLASS_BODY);
    let receiver = if !in_class {
        ""
    } else if plan.is_static {
        "self::"
    } else {
        "$this->"
    };
    let start_of = selection.range.start();
    let statement_indent = indent_at(cx.text, start_of);
    let call = format!("{receiver}{}({})", plan.name, args.join(", "));
    // where the name is in what replaces the selection
    let (replacement, name_at): (String, usize) = if selection.expression {
        (call.clone(), receiver.len())
    } else {
        match plan.returning {
            Returning::Value => (format!("return {call};"), "return ".len() + receiver.len()),
            Returning::Bare => (
                format!("{call};{nl}{statement_indent}return;"),
                receiver.len(),
            ),
            Returning::Never | Returning::No => match plan.outputs.as_slice() {
                [] => (format!("{call};"), receiver.len()),
                [one] => {
                    let lead = format!("{} = ", one.name);
                    let at = lead.len() + receiver.len();
                    (format!("{lead}{call};"), at)
                }
                many => {
                    let names: Vec<&str> = many.iter().map(|o| o.name.as_str()).collect();
                    let lead = format!("[{}] = ", names.join(", "));
                    let at = lead.len() + receiver.len();
                    (format!("{lead}{call};"), at)
                }
            },
        }
    };

    // the new declaration
    let ret = plan
        .return_type
        .as_ref()
        .map_or_else(String::new, |t| format!(": {t}"));
    // what only PHPDoc says, which the code it came from could rely on
    let mut tags: Vec<String> = plan
        .inputs
        .iter()
        .filter_map(|i| i.doc.as_ref().map(|d| format!("@param {d} {}", i.name)))
        .collect();
    if let Some(d) = &plan.doc_return {
        tags.push(if d.starts_with('@') {
            d.clone()
        } else {
            format!("@return {d}")
        });
    }
    let (decl_indent, insert): (String, Inserter<'_>) = if in_class {
        let body = anchor
            .ancestors()
            .find(|n| n.kind() == CLASS_BODY)
            .ok_or("It isn't in a class.")?;
        let member = plan
            .function
            .as_ref()
            .ok_or("It isn't in a method.")?
            .clone();
        let indent = member_indent(cx, &body);
        let body2 = body.clone();
        (
            indent,
            Box::new(move |text: &str| insert_written_member(cx, &body2, Some(&member), text)),
        )
    } else {
        let (at, indent): (TextSize, String) = match &plan.function {
            Some(f) => (
                imports::line_end(cx.text, f.text_range().end()),
                indent_at(cx.text, trimmed_node(f).start()),
            ),
            None => end_of_region(cx, start_of),
        };
        (
            indent,
            Box::new(move |text: &str| FileEdit {
                file: cx.file,
                range: TextRange::empty(at),
                text: format!("{nl}{nl}{text}"),
            }),
        )
    };
    let visibility = if in_class { "private " } else { "" };
    let stat = if plan.is_static { "static " } else { "" };
    let unit = &cx.style.unit;
    let body_indent = format!("{decl_indent}{unit}");
    let brace = format!("{decl_indent}{{");
    let mut text = String::new();
    match tags.as_slice() {
        [] => {}
        [one] => text.push_str(&format!("{decl_indent}/** {one} */{nl}")),
        many => {
            text.push_str(&format!("{decl_indent}/**{nl}"));
            for tag in many {
                text.push_str(&format!("{decl_indent} * {tag}{nl}"));
            }
            text.push_str(&format!("{decl_indent} */{nl}"));
        }
    }
    text.push_str(&format!(
        "{decl_indent}{visibility}{stat}function {}({}){ret}{nl}{brace}{nl}",
        plan.name,
        params.join(", ")
    ));
    if selection.expression {
        let code = reindented(
            &cx.root,
            cx.text,
            selection.range,
            &statement_indent,
            &body_indent,
            nl,
        );
        text.push_str(&format!("{body_indent}return {code};{nl}"));
    } else {
        let code = reindented(
            &cx.root,
            cx.text,
            selection.range,
            &statement_indent,
            &body_indent,
            nl,
        );
        text.push_str(&format!("{body_indent}{code}{nl}"));
        match plan.outputs.as_slice() {
            [] => {}
            [one] => text.push_str(&format!("{body_indent}return {};{nl}", one.name)),
            many => {
                let names: Vec<&str> = many.iter().map(|o| o.name.as_str()).collect();
                text.push_str(&format!("{body_indent}return [{}];{nl}", names.join(", ")));
            }
        }
    }
    text.push_str(&format!("{decl_indent}}}"));
    let mut edits: Vec<FileEdit> = plan.imports;
    let shift: i64 = edits
        .iter()
        .filter(|e| e.range.start() <= selection.range.start())
        .map(|e| e.text.len() as i64 - i64::from(u32::from(e.range.len())))
        .sum();
    edits.push(cx.edit(selection.range, replacement));
    edits.push(insert(&text));
    let name_offset = i64::from(u32::from(selection.range.start())) + shift + name_at as i64;
    let mut change = Change::new(edits);
    change.rename_at = u32::try_from(name_offset).ok().map(TextSize::from);
    Ok(change)
}

/// What puts the new declaration in its place.
type Inserter<'a> = Box<dyn Fn(&str) -> FileEdit + 'a>;

/// Where a new function goes for code outside functions: at the end of its namespace.
fn end_of_region(cx: &Cx<'_>, at: TextSize) -> (TextSize, String) {
    let mut end = cx.root.text_range().end();
    for ns in cx.root.children_of(NAMESPACE_DECL) {
        let range = ns.text_range();
        if ns.child(BLOCK).is_some() {
            if range.contains_inclusive(at)
                && let Some(block) = ns.child(BLOCK)
                && let Some(close) = block.token(R_CURLY)
            {
                let line = imports::line_start(cx.text, close.text_range().start());
                let before_close = TextSize::from(
                    cx.text[..usize::from(line)]
                        .trim_end_matches(['\r', '\n'])
                        .len() as u32,
                );
                let indent = super::fix_types::body_indent(cx, &block);
                return (before_close, indent);
            }
        } else if range.start() > at {
            end = range.start();
            break;
        }
    }
    let at_end = TextSize::from(
        cx.text[..usize::from(end)]
            .trim_end_matches(['\r', '\n', ' ', '\t'])
            .len() as u32,
    );
    (at_end, String::new())
}
