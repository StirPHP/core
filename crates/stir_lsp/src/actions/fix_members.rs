//! What a class lacks: abstract methods it has to implement, and members it uses of itself that
//! aren't there.

use stir_checker::{codes, model_api};
use stir_hir::ast::NodeExt;
use stir_hir::{ClassDef, ClassKind, MethodDef, Visibility};
use stir_syntax::{SyntaxKind::*, SyntaxNode, TextRange, TextSize};

use super::fix_types::parent_of;
use super::hierarchy::class_of;
use super::style::{indent_at, starts_line};
use super::types::{Namer, has, native, written_ref};
use super::{Change, Cx, FileEdit, Found, Kind, Offer};
use crate::imports;
use stir_checker::php_version as v;

pub fn offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    for d in cx.found() {
        match d.code.as_str() {
            codes::CLASS_UNIMPLEMENTED => implement_offer(cx, &d, out),
            codes::METHOD_NOT_FOUND | codes::STATIC_METHOD_NOT_FOUND => {
                create_method_offer(cx, &d, out);
            }
            codes::PROPERTY_NOT_FOUND | codes::STATIC_PROPERTY_NOT_FOUND => {
                create_property_offer(cx, &d, out);
            }
            _ => {}
        }
    }
}

/// The members of a class's body.
pub fn members_of(body: &SyntaxNode) -> Vec<SyntaxNode> {
    body.children()
        .filter(|c| {
            matches!(
                c.kind(),
                METHOD_DECL | PROPERTY_DECL | CLASS_CONST_DECL | TRAIT_USE | ENUM_CASE
            )
        })
        .collect()
}

/// The indentation of a class's members.
pub fn member_indent(cx: &Cx<'_>, body: &SyntaxNode) -> String {
    if let Some(first) = members_of(body).first() {
        let at = super::docs::first_token_start(first);
        if starts_line(cx.text, at) {
            return indent_at(cx.text, at);
        }
    }
    format!(
        "{}{}",
        indent_at(cx.text, body.text_range().start()),
        cx.style.unit
    )
}

/// Where a member goes, and what to put there: `code` is written with `\n` and tabs, at the
/// member's level, and goes after `after`, or at the start of the body with nothing before it.
pub fn insert_member(
    cx: &Cx<'_>,
    body: &SyntaxNode,
    after: Option<&SyntaxNode>,
    code: &str,
) -> FileEdit {
    let indent = member_indent(cx, body);
    let code = cx.style.block(code, &indent);
    insert_written_member(cx, body, after, &code)
}

/// As [`insert_member`], for `code` that's as the file will have it.
pub fn insert_written_member(
    cx: &Cx<'_>,
    body: &SyntaxNode,
    after: Option<&SyntaxNode>,
    code: &str,
) -> FileEdit {
    let nl = cx.style.nl;
    let indent = member_indent(cx, body);
    let close = body
        .token(R_CURLY)
        .map_or_else(|| body.text_range().end(), |t| t.text_range().start());
    if let Some(after) = after {
        let end = after.text_range().end();
        let line_end = imports::line_end(cx.text, end);
        let rest = cx.slice(TextRange::new(end, line_end));
        // a comment on its line goes with it, and something else stays after what's new
        let at = if rest.trim().is_empty() || rest.trim_start().starts_with("//") {
            line_end
        } else {
            end
        };
        return cx.insert(at, format!("{nl}{nl}{code}"));
    }
    let members = members_of(body);
    if let Some(first) = members.first() {
        let start = first.text_range().start();
        if starts_line(cx.text, start) {
            let line = imports::line_start(cx.text, start);
            return cx.insert(line, format!("{code}{nl}{nl}"));
        }
        return cx.insert(start, format!("{nl}{code}{nl}{nl}{indent}"));
    }
    let open = body
        .token(L_CURLY)
        .map_or_else(|| body.text_range().start(), |t| t.text_range().end());
    let inside = cx.slice(TextRange::new(open, close));
    let class_indent = indent_at(cx.text, close);
    if inside.trim().is_empty() {
        let closing = if starts_line(cx.text, close) {
            class_indent
        } else {
            indent_at(cx.text, body.text_range().start())
        };
        return cx.edit(
            TextRange::new(open, close),
            format!("{nl}{code}{nl}{closing}"),
        );
    }
    // only comments in it
    if starts_line(cx.text, close) {
        let line = imports::line_start(cx.text, close);
        return cx.insert(line, format!("{nl}{code}{nl}"));
    }
    cx.insert(close, format!("{nl}{code}{nl}"))
}

/// The default of a parameter of a method elsewhere, as it has to be written here, or `None` if
/// that can't be told.
fn default_text(
    cx: &Cx<'_>,
    namer: &mut Namer<'_>,
    owner_file: stir_db::SourceFile,
    owner: &ClassDef,
    param: &stir_hir::ParamDef,
) -> Option<String> {
    let root = stir_db::parse(cx.db, owner_file).syntax();
    let node = root.covering_element(param.ptr.text_range()).into_node()?;
    let node = if node.kind() == PARAM {
        node
    } else {
        node.descendants()
            .find(|n| n.kind() == PARAM && n.text_range() == param.ptr.text_range())?
    };
    let eq = node.token(EQ)?;
    let value = node
        .children()
        .find(|c| c.text_range().start() >= eq.text_range().end())?;
    let items = stir_hir::file_items(cx.db, owner_file);
    let scope = items.scope_at(value.text_range().start());
    let start = usize::from(value.text_range().start());
    let source = owner_file.text(cx.db);
    let mut out = String::new();
    let mut at = start;
    for name in value.descendants().filter(|n| n.kind() == NAME_EXPR) {
        let range = name.text_range();
        let written = name.text().to_string();
        let trimmed = written.trim();
        let lower = trimmed.to_ascii_lowercase();
        if matches!(lower.as_str(), "null" | "true" | "false") {
            continue;
        }
        let is_class = name.parent().is_some_and(|p| {
            matches!(
                p.kind(),
                CLASS_CONST_FETCH_EXPR | STATIC_PROPERTY_FETCH_EXPR | NEW_EXPR
            )
        });
        let replaced = if !is_class {
            // a constant of PHP's or the project's: its full name works from anywhere
            let resolved = scope.resolve_const(trimmed);
            match cx.ctx().constant(resolved.primary.as_str()) {
                Some(_) => format!("\\{}", resolved.primary.as_str()),
                None => match resolved.fallback.as_ref() {
                    Some(f) => format!("\\{}", f.as_str()),
                    None => format!("\\{}", resolved.primary.as_str()),
                },
            }
        } else {
            match lower.as_str() {
                "self" | "static" => namer.class(owner.name.as_str()),
                "parent" => namer.class(&parent_of(owner)?),
                _ => namer.class(scope.resolve_class(trimmed).as_str()),
            }
        };
        out.push_str(&source[at..usize::from(range.start())]);
        out.push_str(&replaced);
        at = usize::from(range.end());
    }
    out.push_str(&source[at..usize::from(value.text_range().end())]);
    Some(out.trim().to_owned())
}

/// Does the file's version have this type, written as a declaration does?
fn expressible(cx: &Cx<'_>, ty: &stir_hir::type_ref::TypeRef) -> bool {
    use stir_hir::type_ref::{PhpBuiltin as P, TypeRefKind as K};
    match &ty.kind {
        K::Php(P::Mixed) => has(cx, v::MIXED_TYPE),
        K::Php(P::Never) => has(cx, v::NEVER_TYPE),
        K::Php(P::Static) => has(cx, v::STATIC_TYPE),
        K::Php(P::True) => has(cx, v::TRUE_TYPE),
        K::Php(P::Null | P::False) => has(cx, v::STANDALONE_NULL_FALSE),
        K::Nullable(inner) => expressible(cx, inner),
        K::Union(parts) => {
            has(cx, v::UNION_TYPES)
                && parts
                    .iter()
                    .all(|p| matches!(p.kind, K::Php(P::Null | P::False)) || expressible(cx, p))
        }
        K::Intersection(parts) => {
            has(cx, v::INTERSECTION_TYPES) && parts.iter().all(|p| expressible(cx, p))
        }
        _ => true,
    }
}

/// `int $a, string &...$b`, for a method elsewhere, with its names as they're written here.
pub fn params_text(
    cx: &Cx<'_>,
    namer: &mut Namer<'_>,
    owner_file: stir_db::SourceFile,
    owner: &ClassDef,
    method: &MethodDef,
) -> Option<String> {
    let parent = parent_of(owner);
    let mut parts = Vec::new();
    for p in &method.sig.params {
        let mut part = String::new();
        if let Some(ty) = &p.ty
            && expressible(cx, ty)
        {
            part.push_str(&written_ref(
                namer,
                ty,
                Some(owner.name.as_str()),
                parent.as_deref(),
            )?);
            part.push(' ');
        }
        if p.by_ref {
            part.push('&');
        }
        if p.variadic {
            part.push_str("...");
        }
        part.push('$');
        part.push_str(&p.name);
        if p.has_default {
            part.push_str(" = ");
            part.push_str(&default_text(cx, namer, owner_file, owner, p)?);
        }
        parts.push(part);
    }
    Some(parts.join(", "))
}

/// ` : int`, for a method elsewhere, or nothing if it has none this version can say.
pub fn return_text(
    cx: &Cx<'_>,
    namer: &mut Namer<'_>,
    owner: &ClassDef,
    method: &MethodDef,
) -> Option<String> {
    let parent = parent_of(owner);
    let Some(ret) = method.sig.ret.as_ref().filter(|r| expressible(cx, r)) else {
        return Some(String::new());
    };
    let text = written_ref(namer, ret, Some(owner.name.as_str()), parent.as_deref())?;
    Some(format!(": {text}"))
}

/// A string literal, in the quotes the file uses, of text with nothing to escape.
pub fn string_literal(cx: &Cx<'_>, text: &str) -> String {
    let (mut single, mut double) = (0, 0);
    for element in cx.root.descendants_with_tokens() {
        match element.kind() {
            STRING_SINGLE => single += 1,
            STRING_DOUBLE => double += 1,
            _ => {}
        }
    }
    if double > single {
        format!("\"{text}\"")
    } else {
        format!("'{text}'")
    }
}

/// What a method does until someone writes it, rather than something a person might not notice
/// is wrong.
pub fn not_implemented_body(cx: &Cx<'_>, namer: &mut Namer<'_>, name: &str) -> String {
    let exception = namer.class("LogicException");
    let message = string_literal(cx, &format!("Not implemented: {name}"));
    format!("throw new {exception}({message});")
}

/// Does the file already mark what it overrides?
pub fn uses_override_attribute(cx: &Cx<'_>) -> bool {
    cx.text.contains("#[\\Override]") || cx.text.contains("#[Override]")
}

fn implement_offer<'c>(cx: &'c Cx<'c>, d: &Found, out: &mut Vec<Offer<'c>>) {
    let Some(token) = cx.token_at(d.range.start()) else {
        return;
    };
    let Some(owner) = token
        .parent_ancestors()
        .find(|n| n.child(CLASS_BODY).is_some())
    else {
        return;
    };
    let Some(body) = owner.child(CLASS_BODY) else {
        return;
    };
    let Some(class) = class_of(cx, &body.children().next().unwrap_or_else(|| body.clone()))
        .or_else(|| class_of_owner(cx, &owner))
    else {
        return;
    };
    let Some(model) = cx.ctx().class(&class.name.key()) else {
        return;
    };
    let missing = model_api::unimplemented_methods(cx.ctx(), &model);
    if missing.is_empty() {
        return;
    }
    let title = match missing.as_slice() {
        [(_, m)] => format!("Implement `{}()`", m.name),
        all => format!("Implement {} missing methods", all.len()),
    };
    let start = body.text_range().start();
    let mut namer = Namer::new(cx, start);
    let mut code = Vec::new();
    for (owner_model, m) in &missing {
        let Some(params) = params_text(cx, &mut namer, owner_model.file, owner_model.def, m) else {
            return;
        };
        let Some(ret) = return_text(cx, &mut namer, owner_model.def, m) else {
            return;
        };
        let visibility = match m.mods.visibility {
            Visibility::Public => "public",
            Visibility::Protected => "protected",
            Visibility::Private => "private",
        };
        let is_static = if m.mods.is_static { " static" } else { "" };
        let by_ref = if m.sig.by_ref { "&" } else { "" };
        let throw = not_implemented_body(cx, &mut namer, &m.name);
        let attribute = if has(cx, v::OVERRIDE_ATTRIBUTE) && uses_override_attribute(cx) {
            "#[\\Override]\n"
        } else {
            ""
        };
        code.push(format!(
            "{attribute}{visibility}{is_static} function {by_ref}{}({params}){ret}\n{{\n\t{throw}\n}}",
            m.name
        ));
    }
    let methods = members_of(&body);
    let after = methods.last().cloned();
    let member = insert_member(cx, &body, after.as_ref(), &code.join("\n\n"));
    let mut edits = namer.edits();
    edits.push(member);
    out.push(
        Offer::new(
            format!("implement:{}", u32::from(start)),
            title,
            Kind::QuickFix,
            move || Ok(Change::new(edits)),
        )
        .fixing(d.shown, &d.code)
        .preferred(),
    );
}

fn class_of_owner<'c>(cx: &Cx<'c>, owner: &SyntaxNode) -> Option<&'c ClassDef> {
    let range = owner.text_range();
    stir_hir::file_items(cx.db, cx.file)
        .classes
        .iter()
        .find(|c| c.ptr.text_range() == range)
}

/// What a call of a member that's not there is to be created from: the class, if the receiver is
/// that class itself, and its node.
fn own_receiver<'c>(
    cx: &Cx<'c>,
    fetch: &SyntaxNode,
    want_static: bool,
) -> Option<(&'c ClassDef, SyntaxNode)> {
    let recv = fetch.children().next()?;
    let body = fetch.ancestors().find(|n| n.kind() == CLASS_BODY)?;
    let class = class_of(cx, fetch)?;
    let ok = if want_static {
        recv.kind() == NAME_EXPR && {
            let text = recv.text().to_string();
            let lower = text.trim().to_ascii_lowercase();
            lower == "self"
                || lower == "static"
                || stir_hir::file_items(cx.db, cx.file)
                    .scope_at(recv.text_range().start())
                    .resolve_class(text.trim())
                    .key()
                    == class.name.key()
        }
    } else {
        recv.kind() == VARIABLE_EXPR && recv.text() == "$this"
    };
    if !ok {
        return None;
    }
    if !matches!(class.kind, ClassKind::Class | ClassKind::Enum) {
        return None;
    }
    Some((class, body))
}

/// The member of `body` that `node` is in.
fn member_around(node: &SyntaxNode, body: &SyntaxNode) -> Option<SyntaxNode> {
    node.ancestors().find(|n| n.parent().as_ref() == Some(body))
}

/// A variable name that says what `expr` is, if it does.
fn name_from_expr(expr: &SyntaxNode) -> Option<String> {
    let word = match expr.kind() {
        VARIABLE_EXPR => expr.text().to_string().trim_start_matches('$').to_owned(),
        PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR | CLASS_CONST_FETCH_EXPR => {
            expr.name_text()?
        }
        METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR => {
            let name = expr.children().next()?.name_text()?;
            let rest = name
                .strip_prefix("get")
                .filter(|r| r.starts_with(char::is_uppercase))
                .unwrap_or(&name);
            let mut chars = rest.chars();
            let first = chars.next()?;
            format!("{}{}", first.to_lowercase(), chars.as_str())
        }
        _ => return None,
    };
    (word != "this" && !word.is_empty()).then_some(word)
}

fn create_method_offer<'c>(cx: &'c Cx<'c>, d: &Found, out: &mut Vec<Offer<'c>>) {
    let want_static = d.code == codes::STATIC_METHOD_NOT_FOUND;
    let Some(token) = cx.token_at(d.range.start()) else {
        return;
    };
    let Some(name) = token.parent().filter(|p| p.kind() == NAME) else {
        return;
    };
    let Some(fetch) = name.parent() else { return };
    let Some(call) = fetch.parent().filter(|c| {
        matches!(
            c.kind(),
            METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR
        )
    }) else {
        return;
    };
    let Some((_, body)) = own_receiver(cx, &fetch, want_static) else {
        return;
    };
    let Some(args) = call.child(ARG_LIST) else {
        return;
    };
    let mut params: Vec<String> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    let start = body.text_range().start();
    let mut namer = Namer::new(cx, start);
    let typed = cx.typed();
    for (i, arg) in args
        .children()
        .filter(|c| matches!(c.kind(), ARG | NAMED_ARG))
        .enumerate()
    {
        if arg.has_token(ELLIPSIS) {
            return;
        }
        let expr = arg.children().filter(|c| c.kind() != NAME).last();
        let Some(expr) = expr else { return };
        if expr.kind() == CALLABLE_PLACEHOLDER {
            return;
        }
        let base = if arg.kind() == NAMED_ARG {
            arg.name_text()
        } else {
            name_from_expr(&expr)
        }
        .unwrap_or_else(|| format!("arg{}", i + 1));
        let mut unique = base.clone();
        let mut n = 2;
        while used.contains(&unique) {
            unique = format!("{base}{n}");
            n += 1;
        }
        used.push(unique.clone());
        let ty = typed
            .types
            .get(&expr.text_range())
            .filter(|t| !t.is_null())
            .and_then(|t| native(cx, &mut namer, t))
            .map(|w| w.native)
            .filter(|t| t != "void" && t != "never");
        let ty = ty.or_else(|| has(cx, v::MIXED_TYPE).then(|| "mixed".to_owned()));
        params.push(match ty {
            Some(ty) => format!("{ty} ${unique}"),
            None => format!("${unique}"),
        });
    }
    // what's expected back, where that can be seen
    let parent = call.parent();
    let ret = match parent.as_ref().map(SyntaxNode::kind) {
        Some(EXPR_STMT) => Some("void".to_owned()),
        Some(RETURN_STMT) => call
            .ancestors()
            .find(|n| matches!(n.kind(), METHOD_DECL | FUNCTION_DECL | CLOSURE_EXPR))
            .and_then(|f| f.child(RETURN_TYPE))
            .and_then(|r| super::fix_types::written_type(&r))
            .map(|t| t.text().to_string().trim().to_owned())
            .filter(|t| !matches!(t.to_ascii_lowercase().as_str(), "void" | "never"))
            .or_else(|| has(cx, v::MIXED_TYPE).then(|| "mixed".to_owned())),
        _ => has(cx, v::MIXED_TYPE).then(|| "mixed".to_owned()),
    };
    let ret = ret.map_or_else(String::new, |r| format!(": {r}"));
    let method_name = token.text().to_owned();
    let throw = not_implemented_body(cx, &mut namer, &method_name);
    let is_static = if want_static { " static" } else { "" };
    let code = format!(
        "private{is_static} function {method_name}({}){ret}\n{{\n\t{throw}\n}}",
        params.join(", ")
    );
    let after = member_around(&fetch, &body);
    let member = insert_member(cx, &body, after.as_ref(), &code);
    let mut edits = namer.edits();
    edits.push(member);
    let title = format!("Create method `{method_name}()`");
    out.push(
        Offer::new(
            format!("create.method:{method_name}"),
            title,
            Kind::QuickFix,
            move || Ok(Change::new(edits)),
        )
        .fixing(d.shown, &d.code),
    );
}

fn create_property_offer<'c>(cx: &'c Cx<'c>, d: &Found, out: &mut Vec<Offer<'c>>) {
    let want_static = d.code == codes::STATIC_PROPERTY_NOT_FOUND;
    let Some(token) = cx.token_at(d.range.start()) else {
        return;
    };
    let fetch = if want_static {
        token
            .parent()
            .filter(|p| p.kind() == STATIC_PROPERTY_FETCH_EXPR)
    } else {
        token
            .parent()
            .filter(|p| p.kind() == NAME)
            .and_then(|n| n.parent())
            .filter(|p| p.kind() == PROPERTY_FETCH_EXPR)
    };
    let Some(fetch) = fetch else { return };
    let Some((class, body)) = own_receiver(cx, &fetch, want_static) else {
        return;
    };
    if class.kind != ClassKind::Class || class.mods.is_readonly {
        return;
    }
    let name = token.text().trim_start_matches('$').to_owned();
    let typed_properties = has(cx, v::TYPED_PROPERTIES);
    let mut namer = Namer::new(cx, body.text_range().start());
    let assigned = fetch
        .parent()
        .filter(|p| {
            p.kind() == ASSIGN_EXPR
                && p.has_token(EQ)
                && !p.has_token(AMP)
                && p.children().next().as_ref() == Some(&fetch)
        })
        .and_then(|p| p.children().nth(1))
        .and_then(|value| cx.typed().types.get(&value.text_range()).cloned())
        .filter(|t| !t.is_null())
        .and_then(|t| native(cx, &mut namer, &t))
        .map(|w| w.native)
        .filter(|t| typed_properties && !t.eq_ignore_ascii_case("callable"));
    let is_static = if want_static { " static" } else { "" };
    let code = match assigned {
        // it's set where this is: uninitialized until then, as the type says
        Some(ty) => format!("private{is_static} {ty} ${name};"),
        None if typed_properties && has(cx, v::MIXED_TYPE) => {
            format!("private{is_static} mixed ${name} = null;")
        }
        None => format!("private{is_static} ${name};"),
    };
    let members = members_of(&body);
    let after = members
        .iter()
        .rfind(|m| m.kind() == PROPERTY_DECL)
        .or_else(|| {
            members
                .iter()
                .take_while(|m| m.kind() != METHOD_DECL)
                .last()
        });
    let edit = insert_property(cx, &body, after, &code);
    let mut edits = namer.edits();
    edits.push(edit);
    out.push(
        Offer::new(
            format!("create.property:{name}"),
            format!("Create property `${name}`"),
            Kind::QuickFix,
            move || Ok(Change::new(edits)),
        )
        .fixing(d.shown, &d.code),
    );
}

/// As [`insert_member`], without the blank line after another property.
pub fn insert_property(
    cx: &Cx<'_>,
    body: &SyntaxNode,
    after: Option<&SyntaxNode>,
    code: &str,
) -> FileEdit {
    match after {
        Some(after) if after.kind() == PROPERTY_DECL => {
            let indent = member_indent(cx, body);
            let nl = cx.style.nl;
            let end = after.text_range().end();
            let line_end = imports::line_end(cx.text, end);
            let rest = cx.slice(TextRange::new(end, line_end));
            let at: TextSize = if rest.trim().is_empty() || rest.trim_start().starts_with("//") {
                line_end
            } else {
                end
            };
            cx.insert(at, format!("{nl}{indent}{code}"))
        }
        _ => insert_member(cx, body, after, code),
    }
}
