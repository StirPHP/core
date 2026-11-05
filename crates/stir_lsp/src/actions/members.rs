//! Refactorings of a class's relation to its parents: overriding a method, and saying so.

use stir_checker::php_version as v;
use stir_hir::ast::NodeExt;
use stir_hir::{ClassKind, Visibility};
use stir_syntax::{SyntaxKind::*, SyntaxNode};
use stir_types::TyKind;

use super::fix_members::{insert_member, params_text, return_text, uses_override_attribute};
use super::foreign::MISSING_OVERRIDE;
use super::hierarchy::{self, method_of};
use super::style::indent_at;
use super::types::{Namer, has};
use super::{Change, Cx, Kind, Offer};

/// How many methods to offer to override, at most: a menu can't take more.
const MAX_OVERRIDES: usize = 25;

pub fn offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    override_offers(cx, out);
    override_attribute_offer(cx, out);
}

/// Is the cursor in a class's body, between its members?
fn between_members(cx: &Cx<'_>) -> Option<SyntaxNode> {
    if !cx.range.is_empty() {
        return None;
    }
    let token = cx.token_at(cx.range.start())?;
    let body = token.parent().filter(|p| p.kind() == CLASS_BODY)?;
    matches!(token.kind(), WHITESPACE | L_CURLY | R_CURLY).then_some(body)
}

fn override_offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    let Some(body) = between_members(cx) else {
        return;
    };
    let Some(class) = body
        .parent()
        .and_then(|owner| {
            let range = owner.text_range();
            stir_hir::file_items(cx.db, cx.file)
                .classes
                .iter()
                .find(|c| c.ptr.text_range() == range)
        })
        .filter(|c| c.kind == ClassKind::Class)
    else {
        return;
    };
    let Some(model) = cx.ctx().class(&class.name.key()) else {
        return;
    };
    let previous = body
        .children()
        .filter(|c| c.text_range().end() <= cx.range.start())
        .last();
    let mut seen: Vec<String> = class
        .methods
        .iter()
        .map(|m| m.name.to_ascii_lowercase())
        .collect();
    let mut count = 0;
    for ancestor in cx.ctx().ancestors(&model.self_ty()).iter().skip(1) {
        let TyKind::Class { name, .. } = ancestor.kind() else {
            continue;
        };
        let Some(parent) = cx.ctx().class(name) else {
            continue;
        };
        if parent.kind() != ClassKind::Class {
            continue;
        }
        for m in &parent.def.methods {
            let lower = m.name.to_ascii_lowercase();
            if m.magic
                || m.mods.is_final
                || m.mods.is_abstract
                || m.mods.visibility == Visibility::Private
                || seen.contains(&lower)
                || count >= MAX_OVERRIDES
            {
                continue;
            }
            seen.push(lower);
            count += 1;
            let start = body.text_range().start();
            let mut namer = Namer::new(cx, start);
            let Some(params) = params_text(cx, &mut namer, parent.file, parent.def, m) else {
                continue;
            };
            let Some(ret) = return_text(cx, &mut namer, parent.def, m) else {
                continue;
            };
            let visibility = if m.mods.visibility == Visibility::Public {
                "public"
            } else {
                "protected"
            };
            let is_static = if m.mods.is_static { " static" } else { "" };
            let by_ref = if m.sig.by_ref { "&" } else { "" };
            let args: Vec<String> = m
                .sig
                .params
                .iter()
                .map(|p| {
                    let spread = if p.variadic { "..." } else { "" };
                    format!("{spread}${}", p.name)
                })
                .collect();
            let call = format!("parent::{}({})", m.name, args.join(", "));
            let is_void = matches!(
                ret.trim_start_matches(": ").to_ascii_lowercase().as_str(),
                "void" | "never"
            ) && !ret.is_empty();
            let line = if is_void || m.name.eq_ignore_ascii_case("__construct") {
                format!("{call};")
            } else {
                format!("return {call};")
            };
            let attribute = if has(cx, v::OVERRIDE_ATTRIBUTE)
                && uses_override_attribute(cx)
                && !m.name.eq_ignore_ascii_case("__construct")
            {
                "#[\\Override]\n"
            } else {
                ""
            };
            let code = format!(
                "{attribute}{visibility}{is_static} function {by_ref}{}({params}){ret}\n{{\n\t{line}\n}}",
                m.name
            );
            let member = insert_member(cx, &body, previous.as_ref(), &code);
            let mut edits = namer.edits();
            edits.push(member);
            out.push(Offer::new(
                format!("override:{}", m.name),
                format!("Override `{}::{}()`", parent.name, m.name),
                Kind::RefactorRewrite,
                move || Ok(Change::new(edits)),
            ));
        }
    }
}

fn override_attribute_offer<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    if !has(cx, v::OVERRIDE_ATTRIBUTE) {
        return;
    }
    let reported = cx.found().into_iter().find(|f| f.code == MISSING_OVERRIDE);
    let at = reported.as_ref().map_or(cx.range.start(), |f| f.range.start());
    let Some(token) = cx.token_at(at) else { return };
    let Some(name) = token.parent().filter(|p| p.kind() == NAME) else {
        return;
    };
    let Some(method) = name.parent().filter(|p| p.kind() == METHOD_DECL) else {
        return;
    };
    let Some((class, def)) = method_of(cx, &method) else {
        return;
    };
    if def.name.eq_ignore_ascii_case("__construct") || has_override(cx, &method) {
        return;
    }
    let key = class.name.key();
    let overrides = hierarchy::family(cx, &key, &def.name)
        .iter()
        .skip(1)
        .any(|m| cx.ctx().is_subclass(&key, &m.key()));
    if !overrides {
        return;
    }
    let start = super::docs::first_token_start(&method);
    let indent = indent_at(cx.text, start);
    let nl = cx.style.nl;
    let edit = cx.insert(start, format!("#[\\Override]{nl}{indent}"));
    let kind = if reported.is_some() {
        Kind::QuickFix
    } else {
        Kind::RefactorRewrite
    };
    let mut offer = Offer::new(
        format!("override.attribute:{}", def.name),
        "Add `#[\\Override]`",
        kind,
        move || Ok(Change::new(vec![edit])),
    );
    if let Some(f) = &reported {
        offer = offer.fixing(f.shown, MISSING_OVERRIDE).preferred();
    }
    out.push(offer);
}

fn has_override(cx: &Cx<'_>, method: &SyntaxNode) -> bool {
    let Some(list) = method.child(ATTRIBUTE_LIST) else {
        return false;
    };
    let scope = stir_hir::file_items(cx.db, cx.file).scope_at(list.text_range().start());
    list.descendants()
        .filter(|n| n.kind() == ATTRIBUTE)
        .filter_map(|a| a.child(NAME_REF))
        .any(|n| {
            scope.resolve_class(n.text().to_string().trim()).key() == "override"
        })
}
