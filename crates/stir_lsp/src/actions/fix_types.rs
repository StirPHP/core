//! Types left out, nullable types, and what's wrong with a body as a whole.
//!
//! A type is only added where it's right for what the code does as PHP will see it: all of a
//! function's returns have a known type, every path ends in one, and nothing related has to keep a
//! compatible signature that this would break. A type that can't be said natively, or says less
//! than is known, can go in PHPDoc.

use stir_checker::codes;
use stir_hir::ast::NodeExt;
use stir_hir::type_ref::{TypeRef, TypeRefKind};
use stir_hir::{ClassDef, file_items};
use stir_syntax::{SyntaxKind::*, SyntaxNode, TextRange, TextSize};
use stir_types::{Ty, TyKind};

use super::docs::add_tag;
use super::hierarchy::{self, class_of, method_of};
use super::style::indent_at;
use super::types::{Namer, doc_type, has, native, written_ref};
use super::{Change, Cx, FileEdit, Found, Kind, Offer};
use crate::hierarchy::class_def;

pub fn offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    for d in cx.found() {
        match d.code.as_str() {
            codes::PARAMETER_IMPLICITLY_NULLABLE => implicit_nullable(cx, &d, out),
            codes::RETURN_MISSING => return_null(cx, &d, out),
            "deadCode.unreachable" => remove_unreachable(cx, &d, out),
            codes::MISSING_TYPE_RETURN => {
                if let Some(f) = function_around(cx, d.range) {
                    return_type_offers(cx, &f, Kind::QuickFix, Some(&d), out);
                }
            }
            codes::MISSING_TYPE_ITERABLE_VALUE => {
                if let Some(f) = function_around(cx, d.range) {
                    iterable_doc_offer(cx, &f, &d, out);
                }
            }
            codes::MISSING_TYPE_PARAMETER => {
                if let Some(f) = function_around(cx, d.range) {
                    for param in params_of(&f) {
                        param_type_offer(cx, &f, &param, Kind::QuickFix, Some(&d), out);
                    }
                }
            }
            codes::MISSING_TYPE_PROPERTY => {
                if let Some(elem) = cx
                    .token_at(d.range.start())
                    .and_then(|t| t.parent_ancestors().find(|n| n.kind() == PROPERTY_ELEM))
                {
                    property_type_offer(cx, &elem, Kind::QuickFix, Some(&d), out);
                }
            }
            codes::RETURN_TYPE => nullable_return(cx, &d, out),
            codes::PROPERTY_DEFAULT_VALUE => nullable_property(cx, &d, out),
            _ => {}
        }
    }
}

/// With the cursor on a declaration: the same, for what isn't reported at this level.
pub fn refactor_offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    let Some(token) = cx.here() else { return };
    let Some(parent) = token.parent() else { return };
    if let Some(param) = parent.ancestors().find(|n| n.kind() == PARAM)
        && parent.kind() == PARAM
        && token.kind() == VARIABLE
    {
        if let Some(f) = param.ancestors().find(is_function_like) {
            param_type_offer(cx, &f, &param, Kind::RefactorRewrite, None, out);
        }
        return;
    }
    if parent.kind() == PROPERTY_ELEM && token.kind() == VARIABLE {
        property_type_offer(cx, &parent, Kind::RefactorRewrite, None, out);
        return;
    }
    if parent.kind() == NAME
        && let Some(f) = parent.parent().filter(is_function_like)
    {
        return_type_offers(cx, &f, Kind::RefactorRewrite, None, out);
    }
    if token.kind() == FUNCTION_KW
        && let Some(f) = parent
            .ancestors()
            .find(|n| n.kind() == CLOSURE_EXPR)
            .filter(|c| c == &parent)
    {
        return_type_offers(cx, &f, Kind::RefactorRewrite, None, out);
    }
}

fn is_function_like(n: &SyntaxNode) -> bool {
    matches!(n.kind(), FUNCTION_DECL | METHOD_DECL | CLOSURE_EXPR)
}

fn function_around(cx: &Cx<'_>, range: TextRange) -> Option<SyntaxNode> {
    cx.token_at(range.start())?
        .parent_ancestors()
        .find(is_function_like)
}

fn params_of(function: &SyntaxNode) -> Vec<SyntaxNode> {
    function
        .child(PARAM_LIST)
        .map(|l| l.children_of(PARAM))
        .unwrap_or_default()
}

/// The type a node declares, if it has one.
pub fn written_type(node: &SyntaxNode) -> Option<SyntaxNode> {
    stir_hir::first_type(node)
}

fn trimmed_range(node: &SyntaxNode) -> TextRange {
    let text = node.text().to_string();
    let lead = text.len() - text.trim_start().len();
    TextRange::at(
        node.text_range().start() + TextSize::from(lead as u32),
        TextSize::from(text.trim().len() as u32),
    )
}

/// `T $x = null` as `?T $x = null`.
fn implicit_nullable<'c>(cx: &'c Cx<'c>, d: &Found, out: &mut Vec<Offer<'c>>) {
    let Some(edit) = nullable_edit_at(cx, d.range) else {
        return;
    };
    out.push(
        Offer::new(
            format!("nullable.param:{}", u32::from(d.range.start())),
            "Make the parameter's type nullable",
            Kind::QuickFix,
            move || Ok(Change::new(vec![edit])),
        )
        .fixing(d.shown, &d.code)
        .preferred(),
    );
}

/// The edit that lets the type at `range` also be null.
fn nullable_edit_at(cx: &Cx<'_>, range: TextRange) -> Option<FileEdit> {
    let token = cx.token_at(range.start())?;
    let ty = token.parent_ancestors().find(|n| {
        matches!(
            n.kind(),
            NAMED_TYPE | UNION_TYPE | INTERSECTION_TYPE | PAREN_TYPE
        ) && n
            .parent()
            .is_some_and(|p| matches!(p.kind(), PARAM | PROPERTY_DECL | RETURN_TYPE))
    })?;
    nullable_edit(cx, &ty)
}

fn nullable_edit(cx: &Cx<'_>, ty: &SyntaxNode) -> Option<FileEdit> {
    let range = trimmed_range(ty);
    match ty.kind() {
        NAMED_TYPE => {
            let name = cx.slice(range).to_ascii_lowercase();
            if matches!(name.as_str(), "mixed" | "null" | "void" | "never") {
                return None;
            }
            Some(cx.insert(range.start(), "?"))
        }
        UNION_TYPE => {
            has(cx, stir_checker::php_version::UNION_TYPES).then(|| cx.insert(range.end(), "|null"))
        }
        INTERSECTION_TYPE if has(cx, stir_checker::php_version::DNF_TYPES) => {
            Some(cx.edit(range, format!("({})|null", cx.slice(range))))
        }
        _ => None,
    }
}

/// Is `node`'s native type one that takes null as it is?
fn takes_null(text: &str) -> bool {
    let t = text.trim().to_ascii_lowercase();
    t.starts_with('?') || t == "mixed" || t == "null" || t.split('|').any(|p| p.trim() == "null")
}

/// `return null;` at the end of a function whose type allows that. For other types there's no
/// value that wouldn't be made up.
fn return_null<'c>(cx: &'c Cx<'c>, d: &Found, out: &mut Vec<Offer<'c>>) {
    let Some(function) = function_around(cx, d.range) else {
        return;
    };
    let Some(ret) = function.child(RETURN_TYPE) else {
        return;
    };
    let Some(ty) = written_type(&ret) else { return };
    if !takes_null(&ty.text().to_string()) {
        return;
    }
    let Some(block) = function.child(BLOCK) else {
        return;
    };
    let Some(close) = block.token(R_CURLY) else {
        return;
    };
    let close = close.text_range().start();
    let line = crate::imports::line_start(cx.text, close);
    let before = cx.slice(TextRange::new(line, close));
    let unit = &cx.style.unit;
    let nl = cx.style.nl;
    let edit = if before.trim().is_empty() {
        cx.insert(line, format!("{before}{unit}return null;{nl}"))
    } else {
        cx.insert(close, "return null; ")
    };
    out.push(
        Offer::new(
            format!("return.null:{}", u32::from(close)),
            "Add `return null;`",
            Kind::QuickFix,
            move || Ok(Change::new(vec![edit])),
        )
        .fixing(d.shown, &d.code)
        .preferred(),
    );
}

/// The statement starting at `at`, and the list it's in.
fn statement_at(cx: &Cx<'_>, at: TextSize) -> Option<SyntaxNode> {
    let token = cx.token_at(at)?;
    token.parent_ancestors().find(|n| {
        n.parent().is_some_and(|p| is_statement_list(p.kind()))
            && super::docs::first_token_start(n) == at
            && !matches!(n.kind(), CASE_KW | L_CURLY)
    })
}

/// A node whose children are statements run one after another.
pub fn is_statement_list(kind: stir_syntax::SyntaxKind) -> bool {
    matches!(kind, BLOCK | SOURCE_FILE | SWITCH_CASE | NAMESPACE_DECL)
}

fn remove_unreachable<'c>(cx: &'c Cx<'c>, d: &Found, out: &mut Vec<Offer<'c>>) {
    let Some(first) = statement_at(cx, d.range.start()) else {
        return;
    };
    let Some(list) = first.parent() else { return };
    let statements: Vec<SyntaxNode> = list
        .children()
        .filter(|c| c.text_range().start() >= first.text_range().start())
        .filter(|c| !matches!(c.kind(), SWITCH_CASE))
        .collect();
    // what PHP declares before it runs anything, and where a `goto` may go, aren't dead
    let alive = |n: &SyntaxNode| {
        matches!(
            n.kind(),
            FUNCTION_DECL
                | CLASS_DECL
                | INTERFACE_DECL
                | TRAIT_DECL
                | ENUM_DECL
                | LABEL_STMT
                | NAMESPACE_DECL
                | USE_DECL
                | CONST_DECL
                | INLINE_HTML_STMT
                | HALT_COMPILER_STMT
        ) || n.descendants().any(|c| c.kind() == LABEL_STMT)
    };
    if statements.iter().any(alive) {
        return;
    }
    let Some(last) = statements.last() else {
        return;
    };
    let start = super::docs::first_token_start(&first);
    let end = last.text_range().end();
    let range = TextRange::new(
        crate::imports::line_start(cx.text, start),
        crate::imports::line_end(cx.text, end),
    );
    let before = cx.slice(TextRange::new(range.start(), start));
    let after = cx.slice(TextRange::new(end, range.end()));
    let nl = cx.style.nl;
    let (range, text) = if before.trim().is_empty() && after.trim().is_empty() {
        // and the line break that ended the line before
        let previous = cx.text[..usize::from(range.start())]
            .strip_suffix(nl)
            .map_or(range.start(), |p| TextSize::from(p.len() as u32));
        (TextRange::new(previous, range.end()), String::new())
    } else {
        (TextRange::new(start, end), String::new())
    };
    out.push(
        Offer::new(
            format!("unreachable:{}", u32::from(start)),
            "Remove the unreachable code",
            Kind::QuickFix,
            move || Ok(Change::new(vec![cx.edit(range, text)])),
        )
        .fixing(d.shown, &d.code)
        .preferred(),
    );
}

/// Does running `stmt` never get past its end?
pub fn always_leaves(stmt: &SyntaxNode) -> bool {
    match stmt.kind() {
        RETURN_STMT | CONTINUE_STMT | BREAK_STMT | GOTO_STMT => true,
        EXPR_STMT => stmt
            .children()
            .next()
            .is_some_and(|e| matches!(e.kind(), THROW_EXPR | EXIT_EXPR)),
        BLOCK => list_leaves(stmt),
        IF_STMT => {
            let Some(else_clause) = stmt.child(ELSE_CLAUSE) else {
                return false;
            };
            let body_of = |n: &SyntaxNode| {
                n.children()
                    .filter(|c| is_stmt(c.kind()))
                    .last()
                    .is_some_and(|b| always_leaves(&b))
            };
            body_of(stmt)
                && stmt.children_of(ELSEIF_CLAUSE).iter().all(body_of)
                && body_of(&else_clause)
        }
        TRY_STMT => {
            if stmt
                .child(FINALLY_CLAUSE)
                .and_then(|f| f.child(BLOCK))
                .is_some_and(|b| list_leaves(&b))
            {
                return true;
            }
            stmt.child(BLOCK).is_some_and(|b| list_leaves(&b))
                && stmt
                    .children_of(CATCH_CLAUSE)
                    .iter()
                    .all(|c| c.child(BLOCK).is_some_and(|b| list_leaves(&b)))
        }
        _ => false,
    }
}

fn is_stmt(kind: stir_syntax::SyntaxKind) -> bool {
    matches!(
        kind,
        BLOCK
            | EXPR_STMT
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
    )
}

/// Does a list of statements never get past its end?
pub fn list_leaves(list: &SyntaxNode) -> bool {
    list.children()
        .any(|c| is_stmt(c.kind()) && always_leaves(&c))
}

/// The `return`s and `yield`s of a body, leaving out what's in functions and classes within it.
pub fn own_nodes(body: &SyntaxNode) -> Vec<SyntaxNode> {
    fn walk(node: &SyntaxNode, out: &mut Vec<SyntaxNode>) {
        for child in node.children() {
            if matches!(
                child.kind(),
                FUNCTION_DECL
                    | CLOSURE_EXPR
                    | ARROW_FN_EXPR
                    | CLASS_DECL
                    | ANON_CLASS
                    | INTERFACE_DECL
                    | TRAIT_DECL
                    | ENUM_DECL
            ) {
                continue;
            }
            out.push(child.clone());
            walk(&child, out);
        }
    }
    let mut out = Vec::new();
    walk(body, &mut out);
    out
}

/// What a function's body returns, as far as it can be known of all of it.
pub enum Returns {
    Nothing,
    Value(Ty),
    Generator,
}

/// See [`Returns`]; `None` if it can't be told.
pub fn inferred_returns(cx: &Cx<'_>, function: &SyntaxNode) -> Option<Returns> {
    let block = function.child(BLOCK)?;
    let typed = cx.typed();
    let mut values = Vec::new();
    let mut bare = false;
    let mut generator = false;
    for node in own_nodes(&block) {
        match node.kind() {
            YIELD_EXPR | YIELD_FROM_EXPR => generator = true,
            RETURN_STMT => match node.children().next() {
                Some(expr) => {
                    let ty = typed.types.get(&expr.text_range())?;
                    if ty.contains_error() || ty.is_untyped() {
                        return None;
                    }
                    values.push(ty.clone());
                }
                None => bare = true,
            },
            _ => {}
        }
    }
    if generator {
        return Some(Returns::Generator);
    }
    if values.is_empty() {
        return Some(Returns::Nothing);
    }
    // every path has to end in one: PHP has no nullable way of falling off the end
    if bare || !list_leaves(&block) {
        return None;
    }
    Some(Returns::Value(Ty::union(
        values.iter().map(stir_checker::widen_literals),
    )))
}

/// What a function's PHPDoc says it returns, if it says.
fn documented_return(cx: &Cx<'_>, function: &SyntaxNode) -> Option<Ty> {
    let sig = match function.kind() {
        METHOD_DECL => &method_of(cx, function)?.1.sig,
        FUNCTION_DECL => {
            let at = function.text_range();
            &file_items(cx.db, cx.file)
                .functions
                .iter()
                .find(|f| f.ptr.text_range() == at)?
                .sig
        }
        _ => return None,
    };
    Some(cx.ctx().lower_plain(sig.doc_ret.as_ref()?))
}

/// A subclass that overrides this without a return type would stop compiling.
fn return_type_blockers(cx: &Cx<'_>, function: &SyntaxNode) -> Option<()> {
    if function.kind() != METHOD_DECL {
        return Some(());
    }
    let (class, method) = method_of(cx, function)?;
    let key = class.name.key();
    for member in hierarchy::family(cx, &key, &method.name).iter().skip(1) {
        let is_above = cx.ctx().is_subclass(&key, &member.key());
        if !is_above && member.method.sig.ret.is_none() {
            return None;
        }
    }
    Some(())
}

/// The native return type the method has to keep, from what it overrides: what PHP's own classes
/// declare is tentative, so an override needn't say it, but what it does say has to fit.
fn inherited_return<'c>(cx: &Cx<'c>, function: &SyntaxNode) -> Option<(Member2<'c>, TypeRef)> {
    if function.kind() != METHOD_DECL {
        return None;
    }
    let (class, method) = method_of(cx, function)?;
    let key = class.name.key();
    hierarchy::family(cx, &key, &method.name)
        .into_iter()
        .skip(1)
        .find(|m| cx.ctx().is_subclass(&key, &m.key()) && m.method.sig.ret.is_some())
        .and_then(|m| {
            let ret = m.method.sig.ret.clone()?;
            Some((m, ret))
        })
}

type Member2<'c> = hierarchy::Member<'c>;

/// The parent of `class`'s key, for `parent` in a type.
pub fn parent_of(class: &ClassDef) -> Option<String> {
    class.extends.first().and_then(|t| match &t.kind {
        TypeRefKind::Named(name, _) => Some(name.as_str().to_owned()),
        _ => None,
    })
}

fn return_type_offers<'c>(
    cx: &'c Cx<'c>,
    function: &SyntaxNode,
    kind: Kind,
    d: Option<&Found>,
    out: &mut Vec<Offer<'c>>,
) {
    if function.child(RETURN_TYPE).is_some() || function.child(BLOCK).is_none() {
        return;
    }
    let name = function
        .name_text()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(name.as_str(), "__construct" | "__destruct") {
        return;
    }
    let after = function
        .child(CLOSURE_USE)
        .or_else(|| function.child(PARAM_LIST))
        .map(|n| trimmed_range(&n).end());
    let Some(after) = after else { return };
    if return_type_blockers(cx, function).is_none() {
        return;
    }
    let decl = function.clone();
    let start = function.text_range().start();
    let mut namer = Namer::new(cx, start);
    let (native_text, doc) = if let Some((member, ret)) = inherited_return(cx, function) {
        let parent = parent_of(member.class);
        let Some(text) = written_ref(
            &mut namer,
            &ret,
            Some(member.class.name.as_str()),
            parent.as_deref(),
        ) else {
            return;
        };
        (Some(text), None)
    } else {
        let Some(returns) = inferred_returns(cx, function) else {
            return;
        };
        let documented = documented_return(cx, function);
        match returns {
            Returns::Nothing => (Some("void".to_owned()), None),
            Returns::Generator => (Some(namer.class("Generator")), None),
            Returns::Value(ty) => match native(cx, &mut namer, &ty) {
                Some(w) => {
                    // what the doc comment says has to fit in it, or that's an error of its own
                    if let Some(doc) = &documented {
                        match native(cx, &mut namer, doc) {
                            Some(d) if d.native == w.native => {}
                            _ => return,
                        }
                    }
                    (Some(w.native), w.doc)
                }
                None => (None, doc_type(&mut namer, &ty)),
            },
        }
    };
    let fixes = d.map(|d| (d.shown, d.code.clone()));
    let imports = namer.finish();
    let has_tag = function
        .doc_comment()
        .is_some_and(|t| t.text().contains("@return") || t.text().contains("@phpstan-return"));
    let doc = doc.filter(|_| !has_tag);
    if let Some(text) = native_text {
        let mut edits = vec![cx.insert(after, format!(": {text}"))];
        if let Some(doc) = &doc {
            edits.push(add_tag(cx, &decl, &format!("@return {doc}")));
        }
        edits.extend(imports.iter().map(|e| cx.insert(e.at, e.text.clone())));
        let mut offer = Offer::new(
            format!("type.return:{}", u32::from(start)),
            format!("Add return type `{text}`"),
            kind,
            move || Ok(Change::new(edits)),
        );
        if let Some((range, code)) = &fixes {
            offer = offer.fixing(*range, code);
            offer = offer.preferred();
        }
        out.push(offer);
    } else if let Some(doc) = doc {
        let mut edits = vec![add_tag(cx, &decl, &format!("@return {doc}"))];
        edits.extend(imports.iter().map(|e| cx.insert(e.at, e.text.clone())));
        let mut offer = Offer::new(
            format!("doc.return:{}", u32::from(start)),
            format!("Add `@return {doc}` to the doc comment"),
            kind,
            move || Ok(Change::new(edits)),
        );
        if let Some((range, code)) = &fixes {
            offer = offer.fixing(*range, code);
        }
        out.push(offer);
    }
}

/// `@return array<int, string>` for an `array` that doesn't say what it holds.
fn iterable_doc_offer<'c>(
    cx: &'c Cx<'c>,
    function: &SyntaxNode,
    d: &Found,
    out: &mut Vec<Offer<'c>>,
) {
    let Some(ret) = function.child(RETURN_TYPE).and_then(|r| written_type(&r)) else {
        return;
    };
    if function
        .doc_comment()
        .is_some_and(|t| t.text().contains("@return"))
    {
        return;
    }
    let native_text = ret.text().to_string();
    if !native_text.trim().eq_ignore_ascii_case("array") {
        return;
    }
    let Some(Returns::Value(ty)) = inferred_returns(cx, function) else {
        return;
    };
    let start = function.text_range().start();
    let mut namer = Namer::new(cx, start);
    let Some(doc) = doc_type(&mut namer, &ty) else {
        return;
    };
    if !doc.contains(['<', '{']) {
        return;
    }
    let mut edits = namer.edits();
    edits.push(add_tag(cx, function, &format!("@return {doc}")));
    out.push(
        Offer::new(
            format!("doc.iterable:{}", u32::from(start)),
            format!("Add `@return {doc}` to the doc comment"),
            Kind::QuickFix,
            move || Ok(Change::new(edits)),
        )
        .fixing(d.shown, &d.code),
    );
}

/// The signature's declaration of `param`.
fn param_def<'c>(
    cx: &Cx<'c>,
    function: &SyntaxNode,
    param: &SyntaxNode,
) -> Option<&'c stir_hir::ParamDef> {
    let range = param.text_range();
    let sig = match function.kind() {
        METHOD_DECL => &method_of(cx, function)?.1.sig,
        FUNCTION_DECL => {
            let at = function.text_range();
            &file_items(cx.db, cx.file)
                .functions
                .iter()
                .find(|f| f.ptr.text_range() == at)?
                .sig
        }
        _ => return None,
    };
    sig.params.iter().find(|p| p.ptr.text_range() == range)
}

/// Adds what PHPDoc says of a parameter as its native type, or what the method it overrides has.
/// A parameter's type can't be inferred from the body in a way that's right for all callers.
fn param_type_offer<'c>(
    cx: &'c Cx<'c>,
    function: &SyntaxNode,
    param: &SyntaxNode,
    kind: Kind,
    d: Option<&Found>,
    out: &mut Vec<Offer<'c>>,
) {
    if written_type(param).is_some() {
        return;
    }
    let Some(def) = param_def(cx, function, param) else {
        return;
    };
    let Some(variable) = param.token(VARIABLE) else {
        return;
    };
    let at = variable.text_range().start();
    // `&` and `...` come after the type
    let at = param
        .tokens()
        .into_iter()
        .find(|t| matches!(t.kind(), AMP | ELLIPSIS | VARIABLE))
        .map_or(at, |t| t.text_range().start());
    let start = function.text_range().start();
    let mut namer = Namer::new(cx, start);
    let index = function
        .child(PARAM_LIST)
        .map(|l| l.children_of(PARAM))
        .and_then(|ps| ps.iter().position(|p| p == param));
    let Some(index) = index else { return };
    let inherited = inherited_param(cx, function, index, &mut namer);
    let text = match inherited {
        ParentParam::Written(text) => Some(text),
        ParentParam::Refuses => None,
        ParentParam::Free => def.doc_ty.as_ref().and_then(|t| {
            let ty = cx.ctx().lower_plain(t);
            if def.variadic && !matches!(t.kind, TypeRefKind::Named(..) | TypeRefKind::Php(_)) {
                return None;
            }
            let w = native(cx, &mut namer, &ty)?;
            // `int $x = null` is what's deprecated
            let w = if def.default_is_null && !takes_null(&w.native) {
                native(cx, &mut namer, &ty.nullable())?.native
            } else {
                w.native
            };
            Some(w)
        }),
    };
    let Some(text) = text else { return };
    if text == "void" || text == "never" {
        return;
    }
    // a default that isn't of this type would be an error
    if let Some(default) = param.children().find(|c| {
        !matches!(c.kind(), MODIFIERS | ATTRIBUTE_LIST) && !stir_hir::is_type_node(c.kind())
    }) && !default_fits(cx, &default, &text)
    {
        return;
    }
    let edits_imports = namer.finish();
    let name = variable.text().to_owned();
    let mut edits = vec![cx.insert(at, format!("{text} "))];
    edits.extend(
        edits_imports
            .iter()
            .map(|e| cx.insert(e.at, e.text.clone())),
    );
    let mut offer = Offer::new(
        format!("type.param:{}", u32::from(at)),
        format!("Add type `{text}` to `{name}`"),
        kind,
        move || Ok(Change::new(edits)),
    );
    if let Some(d) = d {
        offer = offer.fixing(d.shown, &d.code);
    }
    out.push(offer);
}

enum ParentParam {
    /// Nothing above has it, so it's free to choose.
    Free,
    /// It has to accept this.
    Written(String),
    /// What's above leaves nothing that can be added.
    Refuses,
}

fn inherited_param(
    cx: &Cx<'_>,
    function: &SyntaxNode,
    index: usize,
    namer: &mut Namer<'_>,
) -> ParentParam {
    if function.kind() != METHOD_DECL {
        return ParentParam::Free;
    }
    let Some((class, method)) = method_of(cx, function) else {
        return ParentParam::Refuses;
    };
    let key = class.name.key();
    let mut result = ParentParam::Free;
    for member in hierarchy::family(cx, &key, &method.name).iter().skip(1) {
        let above = cx.ctx().is_subclass(&key, &member.key());
        if !above {
            // what's below has to accept all of it: fine if it's left out, not otherwise
            if member
                .method
                .sig
                .params
                .get(index)
                .is_some_and(|p| p.ty.is_some())
            {
                return ParentParam::Refuses;
            }
            continue;
        }
        let Some(p) = member.method.sig.params.get(index) else {
            continue;
        };
        let Some(ty) = &p.ty else {
            // an override can't be narrower than what it overrides
            return ParentParam::Refuses;
        };
        let parent = parent_of(member.class);
        match written_ref(
            namer,
            ty,
            Some(member.class.name.as_str()),
            parent.as_deref(),
        ) {
            Some(text) => result = ParentParam::Written(text),
            None => return ParentParam::Refuses,
        }
    }
    result
}

/// Could the default written in `default` be a value of the type `text`?
fn default_fits(cx: &Cx<'_>, default: &SyntaxNode, text: &str) -> bool {
    if default.kind() == NAME_EXPR {
        let name = default.text().to_string().trim().to_ascii_lowercase();
        if name == "null" {
            return takes_null(text);
        }
    }
    let Some(ty) = cx.typed().types.get(&default.text_range()) else {
        // not a literal, and not something this can say
        return default.kind() != LITERAL;
    };
    let ty = stir_checker::widen_literals(ty);
    let word = match ty.kind() {
        TyKind::Int => "int",
        TyKind::Float => "float",
        TyKind::String => "string",
        TyKind::Bool => "bool",
        TyKind::Array { .. } | TyKind::List { .. } | TyKind::Shape(_) => "array",
        _ => return true,
    };
    let lower = text.to_ascii_lowercase();
    let lower = lower.trim_start_matches('?');
    lower == "mixed"
        || lower
            .split('|')
            .any(|p| p == word || (word == "array" && p == "iterable"))
        || (word == "int" && lower.split('|').any(|p| p == "float"))
}

/// Adds a type to a property: PHPDoc's if it says one natively, or for a private one what's
/// assigned to it, all of which can be seen in the class.
fn property_type_offer<'c>(
    cx: &'c Cx<'c>,
    elem: &SyntaxNode,
    kind: Kind,
    d: Option<&Found>,
    out: &mut Vec<Offer<'c>>,
) {
    let Some(decl) = elem.parent().filter(|p| p.kind() == PROPERTY_DECL) else {
        return;
    };
    if written_type(&decl).is_some()
        || decl.children_of(PROPERTY_ELEM).len() != 1
        || !has(cx, stir_checker::php_version::TYPED_PROPERTIES)
    {
        return;
    }
    let Some(var) = elem.token(VARIABLE) else {
        return;
    };
    let name = var.text().trim_start_matches('$').to_owned();
    let Some(class) = class_of(cx, elem) else {
        return;
    };
    if class.kind == stir_hir::ClassKind::Trait {
        return;
    }
    let Some(def) = class.properties.iter().find(|p| p.name == name && !p.magic) else {
        return;
    };
    // properties are invariant: whatever else declares it has to change with it
    if redeclared(cx, class, &name) {
        return;
    }
    let start = decl.text_range().start();
    let mut namer = Namer::new(cx, start);
    let value_of_default = elem.children().next();
    let default_is_null = value_of_default
        .as_ref()
        .is_some_and(|n| n.text().to_string().trim().eq_ignore_ascii_case("null"));
    let private = def.mods.visibility == stir_hir::Visibility::Private;
    let doc_ty = def.doc_ty.as_ref().map(|t| cx.ctx().lower_plain(t));
    let ty = match doc_ty {
        Some(ty) => ty,
        None if private && !def.mods.is_static => {
            let Some(ty) = assigned_types(cx, elem, &name, def.has_default) else {
                return;
            };
            ty
        }
        None => return,
    };
    let Some(w) = native(cx, &mut namer, &ty) else {
        return;
    };
    let text = w.native;
    if text == "void" || text == "never" || text.eq_ignore_ascii_case("callable") {
        return;
    }
    // without a type a property is null until it's set; with one it's an error to read it
    let mut add_null_default = false;
    if !def.has_default {
        if takes_null(&text) {
            add_null_default = true;
        } else if !set_in_constructor(cx, class, &name) {
            return;
        }
    } else if let Some(value) = &value_of_default
        && !default_is_null
        && !default_fits(cx, value, &text)
    {
        return;
    } else if default_is_null && !takes_null(&text) {
        return;
    }
    let mut edits = namer.edits();
    edits.push(cx.insert(var.text_range().start(), format!("{text} ")));
    if add_null_default {
        edits.push(cx.insert(var.text_range().end(), " = null"));
    }
    let mut offer = Offer::new(
        format!("type.property:{name}"),
        format!("Add type `{text}` to `${name}`"),
        kind,
        move || Ok(Change::new(edits)),
    );
    if let Some(d) = d {
        offer = offer.fixing(d.shown, &d.code);
    }
    out.push(offer);
}

/// Does anything above or below `class` declare the property too?
fn redeclared(cx: &Cx<'_>, class: &ClassDef, name: &str) -> bool {
    let key = class.name.key();
    let declares = |other: &str| {
        class_def(cx.db, cx.ws, other).is_some_and(|(_, c)| {
            c.properties.iter().any(|p| !p.magic && p.name == name)
                || c.methods.iter().any(|m| {
                    m.name.eq_ignore_ascii_case("__construct")
                        && m.sig
                            .params
                            .iter()
                            .any(|p| p.promoted.is_some() && p.name == name)
                })
        })
    };
    hierarchy::ancestors_of(cx, &key)
        .iter()
        .any(|a| declares(a))
        || crate::hierarchy::all_subtypes(cx.db, cx.ws, &key)
            .iter()
            .any(|s| declares(s))
}

/// Is `$this->name = ...;` a statement of the constructor itself, not under a condition?
fn set_in_constructor(cx: &Cx<'_>, class: &ClassDef, name: &str) -> bool {
    let Some(ctor) = class
        .methods
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case("__construct"))
    else {
        return false;
    };
    let range = ctor.ptr.text_range();
    if !cx.root.text_range().contains_range(range) {
        return false;
    }
    let Some(node) = cx.root.covering_element(range).into_node() else {
        return false;
    };
    let Some(block) = node.child(BLOCK) else {
        return false;
    };
    block.children_of(EXPR_STMT).iter().any(|s| {
        s.children().next().is_some_and(|e| {
            e.kind() == ASSIGN_EXPR && e.children().next().is_some_and(|l| is_this_fetch(&l, name))
        })
    })
}

/// `$this->name`.
pub fn is_this_fetch(node: &SyntaxNode, name: &str) -> bool {
    node.kind() == PROPERTY_FETCH_EXPR
        && node
            .children()
            .next()
            .is_some_and(|r| r.kind() == VARIABLE_EXPR && r.text() == "$this")
        && node.name_text().as_deref() == Some(name)
}

/// The union of the types assigned to a private property, in its default and throughout its
/// class. `None` if any use isn't a plain read or a plain assignment of what's known.
fn assigned_types(cx: &Cx<'_>, elem: &SyntaxNode, name: &str, has_default: bool) -> Option<Ty> {
    let body = elem.ancestors().find(|n| n.kind() == CLASS_BODY)?;
    let typed = cx.typed();
    let mut types = Vec::new();
    match elem.children().next() {
        Some(default) => {
            let text = default.text().to_string();
            if text.trim().eq_ignore_ascii_case("null") {
                types.push(Ty::null());
            } else {
                let ty = typed.types.get(&default.text_range());
                match (ty, default.kind()) {
                    (Some(ty), _) => types.push(ty.clone()),
                    (None, LITERAL) => {
                        let literal = literal_type(&default)?;
                        types.push(literal);
                    }
                    _ => return None,
                }
            }
        }
        None if !has_default => types.push(Ty::null()),
        None => {}
    }
    for node in body.descendants() {
        match node.kind() {
            // a name that's worked out while running could be this one
            PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR if node.child(NAME).is_none() => {
                return None;
            }
            PROPERTY_FETCH_EXPR if is_this_fetch(&node, name) => {
                let parent = node.parent()?;
                match parent.kind() {
                    ASSIGN_EXPR if parent.children().next().as_ref() == Some(&node) => {
                        if !parent.has_token(EQ) || parent.has_token(AMP) {
                            return None;
                        }
                        let value = parent.children().nth(1)?;
                        let ty = typed.types.get(&value.text_range())?;
                        if ty.contains_error() || ty.is_untyped() {
                            return None;
                        }
                        types.push(ty.clone());
                    }
                    INDEX_EXPR | POSTFIX_EXPR | PREFIX_EXPR | UNSET_STMT | ARG | ARRAY_ELEM
                    | LIST_EXPR | FOREACH_STMT | ISSET_EXPR | EMPTY_EXPR | CLOSURE_USE_VAR => {
                        return None;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if types.is_empty() {
        return None;
    }
    Some(Ty::union(types.iter().map(stir_checker::widen_literals)))
}

fn literal_type(node: &SyntaxNode) -> Option<Ty> {
    let token = node.first_token()?;
    Some(match token.kind() {
        INT_NUMBER => Ty::int(),
        FLOAT_NUMBER => Ty::float(),
        STRING_SINGLE | STRING_DOUBLE => Ty::string(),
        _ => return None,
    })
}

/// `return null;` in something declared as not taking it.
fn nullable_return<'c>(cx: &'c Cx<'c>, d: &Found, out: &mut Vec<Offer<'c>>) {
    if !cx.slice(d.range).trim().eq_ignore_ascii_case("null") {
        return;
    }
    let Some(function) = function_around(cx, d.range) else {
        return;
    };
    let Some(ty) = function.child(RETURN_TYPE).and_then(|r| written_type(&r)) else {
        return;
    };
    // what it overrides said it wouldn't be null
    if function.kind() == METHOD_DECL && inherited_return(cx, &function).is_some() {
        return;
    }
    let Some(edit) = nullable_edit(cx, &ty) else {
        return;
    };
    out.push(
        Offer::new(
            format!(
                "nullable.return:{}",
                u32::from(function.text_range().start())
            ),
            "Make the return type nullable",
            Kind::QuickFix,
            move || Ok(Change::new(vec![edit])),
        )
        .fixing(d.shown, &d.code),
    );
}

/// `private int $x = null;`.
fn nullable_property<'c>(cx: &'c Cx<'c>, d: &Found, out: &mut Vec<Offer<'c>>) {
    let Some(token) = cx.token_at(d.range.start()) else {
        return;
    };
    let Some(elem) = token.parent_ancestors().find(|n| n.kind() == PROPERTY_ELEM) else {
        return;
    };
    let is_null = elem
        .children()
        .next()
        .is_some_and(|v| v.text().to_string().trim().eq_ignore_ascii_case("null"));
    let Some(decl) = elem.parent().filter(|_| is_null) else {
        return;
    };
    let Some(ty) = written_type(&decl) else {
        return;
    };
    let Some(var) = elem.token(VARIABLE) else {
        return;
    };
    let name = var.text().trim_start_matches('$');
    let Some(class) = class_of(cx, &elem) else {
        return;
    };
    if redeclared(cx, class, name) {
        return;
    }
    let Some(edit) = nullable_edit(cx, &ty) else {
        return;
    };
    out.push(
        Offer::new(
            format!("nullable.property:{name}"),
            "Make the property's type nullable",
            Kind::QuickFix,
            move || Ok(Change::new(vec![edit])),
        )
        .fixing(d.shown, &d.code)
        .preferred(),
    );
}

/// Every `T $x = null` of a file made nullable, for fixing all at once.
pub fn nullable_everywhere(cx: &Cx<'_>) -> Vec<FileEdit> {
    stir_checker::check_file(cx.db, cx.ws, cx.file)
        .iter()
        .filter(|d| d.code == codes::PARAMETER_IMPLICITLY_NULLABLE)
        .filter_map(|d| nullable_edit_at(cx, d.range))
        .collect()
}

/// The indentation of what's in `block`.
pub fn body_indent(cx: &Cx<'_>, block: &SyntaxNode) -> String {
    let first = block.children().find(|c| is_stmt(c.kind()));
    match first {
        Some(s) => indent_at(cx.text, s.text_range().start()),
        None => format!(
            "{}{}",
            indent_at(cx.text, block.text_range().start()),
            cx.style.unit
        ),
    }
}
