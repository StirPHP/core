//! Unknown names: importing them, and correcting what looks like a typo.

use rustc_hash::FxHashSet;
use smol_str::SmolStr;
use stir_checker::model_api::{self, MemberTy};
use stir_checker::{Def, codes};
use stir_db::{FileOrigin, SourceFile};
use stir_hir::name::Fqn;
use stir_hir::{ClassKind, Visibility, file_items};
use stir_syntax::{SyntaxKind::*, SyntaxNode, TextRange};
use stir_types::{Ty, TyKind};

use super::{Change, Cx, Found, Kind, Offer};
use crate::imports::Imports;
use crate::nav;

/// How many corrections to offer, at most.
const MAX_TYPOS: usize = 3;
/// How many imports to offer, at most.
const MAX_IMPORTS: usize = 15;

pub fn offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    for d in cx.found() {
        let d = &d;
        match d.code.as_str() {
            codes::CLASS_NOT_FOUND | codes::INTERFACE_NOT_FOUND | codes::TRAIT_NOT_FOUND => {
                class_name(cx, d, out);
            }
            codes::FUNCTION_NOT_FOUND => function_name(cx, d, out),
            codes::CONSTANT_NOT_FOUND => constant_name(cx, d, out),
            codes::METHOD_NOT_FOUND | codes::STATIC_METHOD_NOT_FOUND => {
                member_typo(cx, d, true, out);
            }
            codes::PROPERTY_NOT_FOUND | codes::STATIC_PROPERTY_NOT_FOUND => {
                member_typo(cx, d, false, out);
            }
            codes::VARIABLE_UNDEFINED => variable_typo(cx, d, out),
            _ => {}
        }
    }
}

/// The name written over `range`, if that's all that's there.
fn written_name<'c>(cx: &Cx<'c>, range: TextRange) -> Option<&'c str> {
    let written = cx.slice(range).trim();
    let ok = !written.is_empty()
        && written
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '\\' || !c.is_ascii());
    ok.then_some(written)
}

/// How close two names are: the edits from one to the other, ignoring case, or `None` if that's
/// too many for it to look like a typo.
pub fn typo_distance(a: &str, b: &str) -> Option<usize> {
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let limit = (a.len().max(b.len()) / 3).max(1);
    if a.len().abs_diff(b.len()) > limit || a == b && a.is_empty() {
        return None;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    let mut before = vec![0; b.len() + 1];
    let mut before2 = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        std::mem::swap(&mut before2, &mut before);
        std::mem::swap(&mut before, &mut row);
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let mut best = (before[j] + cost).min(before[j + 1] + 1).min(row[j] + 1);
            // two swapped are one slip
            if i > 0 && j > 0 && *ca == b[j - 1] && a[i - 1] == *cb {
                best = best.min(before2[j - 1] + 1);
            }
            row[j + 1] = best;
        }
    }
    let d = row[b.len()];
    (d <= limit).then_some(d)
}

/// How far `fqn` is from `namespace`, for putting what's nearer first: the fewer segments differ
/// the better.
fn namespace_distance(fqn: &Fqn, namespace: &str) -> usize {
    let theirs: Vec<&str> = fqn.namespace().split('\\').collect();
    let ours: Vec<&str> = namespace.split('\\').collect();
    let shared = theirs
        .iter()
        .zip(&ours)
        .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
        .count();
    theirs.len() + ours.len() - 2 * shared
}

/// Which classes fit where the name is written.
fn fits(node: Option<&SyntaxNode>, kind: ClassKind, is_abstract: bool, code: &str) -> bool {
    match code {
        codes::INTERFACE_NOT_FOUND => return kind == ClassKind::Interface,
        codes::TRAIT_NOT_FOUND => return kind == ClassKind::Trait,
        _ => {}
    }
    let Some(node) = node else { return true };
    let context = node.ancestors().skip(1).find(|n| {
        matches!(
            n.kind(),
            NEW_EXPR | EXTENDS_CLAUSE | IMPLEMENTS_CLAUSE | TRAIT_USE
        )
    });
    match context.map(|n| n.kind()) {
        Some(NEW_EXPR) => kind == ClassKind::Class && !is_abstract,
        Some(EXTENDS_CLAUSE) => matches!(kind, ClassKind::Class | ClassKind::Interface),
        Some(IMPLEMENTS_CLAUSE) => kind == ClassKind::Interface,
        Some(TRAIT_USE) => kind == ClassKind::Trait,
        _ => true,
    }
}

/// One way of writing an unknown name right: what it comes to, with the import that takes.
fn import_change(
    cx: &Cx<'_>,
    range: TextRange,
    text: String,
    import: Option<crate::imports::ImportEdit>,
) -> Change {
    let mut edits = vec![cx.edit(range, text)];
    if let Some(edit) = import {
        edits.push(cx.insert(edit.at, edit.text));
    }
    Change::new(edits)
}

fn class_name<'c>(cx: &'c Cx<'c>, d: &Found, out: &mut Vec<Offer<'c>>) {
    let Some(written) = written_name(cx, d.range) else {
        return;
    };
    let db = cx.db;
    let index = stir_hir::def_index(db, cx.ws);
    let Some(imports) = Imports::new(db, cx.file, d.range.start()) else {
        return;
    };
    let namespace = file_items(db, cx.file).scope_at(d.range.start()).namespace.clone();
    let node = cx
        .root
        .covering_element(d.range)
        .into_node()
        .or_else(|| cx.token_at(d.range.start()).and_then(|t| t.parent()));
    let bare = written.trim_start_matches('\\');
    let wanted_suffix = format!("\\{}", bare.to_ascii_lowercase());
    let qualified = bare.contains('\\');
    let short = bare.rsplit('\\').next().unwrap_or(bare);

    let mut exact = Vec::new();
    let mut close = Vec::new();
    for (key, locs) in &index.classes {
        let Some(loc) = locs.first() else { continue };
        let last = key.rsplit('\\').next().unwrap_or(key);
        let same = if qualified {
            key.ends_with(&wanted_suffix)
        } else {
            last.eq_ignore_ascii_case(short)
        };
        let typo = if same { None } else { typo_distance(short, last) };
        if !same && typo.is_none() {
            continue;
        }
        if !nav::extension_is_offered(db, cx.ws, loc.file) {
            continue;
        }
        let items = file_items(db, loc.file);
        let def = &items.classes[loc.index as usize];
        if !fits(node.as_ref(), def.kind, def.mods.is_abstract, &d.code) {
            continue;
        }
        let rank = (
            match loc.file.origin(db) {
                FileOrigin::Source => 0,
                FileOrigin::Vendor => 1,
                FileOrigin::Stub => 2,
            },
            namespace_distance(&def.name, &namespace),
            def.name.as_str().len(),
        );
        if same {
            exact.push((rank, def.name.clone()));
        } else if let Some(typo) = typo {
            close.push((typo, rank, def.name.clone()));
        }
    }
    exact.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.as_str().cmp(b.1.as_str())));
    exact.truncate(MAX_IMPORTS);
    // the only one of the project's, whatever else goes by that name in PHP's stubs
    let single = exact.iter().filter(|(rank, _)| rank.0 == 0).count() == 1 || exact.len() == 1;
    let mut first = true;
    for (_, fqn) in exact {
        let name = imports.class(db, cx.ws, &fqn);
        // qualified the way it's written: what the import is for is the first part of that
        let (title, text, import) = match (&name.import, qualified) {
            (Some(_), false) => (
                format!("Import `{}`", fqn.as_str()),
                written.to_owned(),
                name.import.clone(),
            ),
            _ => (
                format!("Change to `{}`", name.text),
                name.text.clone(),
                name.import.clone(),
            ),
        };
        // the same name twice in one file: the name itself may need no change at all
        let text = if import.is_some() { name.text.clone() } else { text };
        let range = d.range;
        let mut offer = Offer::new(
            format!("import.class:{}", fqn.key()),
            title,
            Kind::QuickFix,
            move || Ok(import_change(cx, range, text, import)),
        )
        .fixing(d.shown, &d.code);
        if single && first {
            offer = offer.preferred();
        }
        first = false;
        out.push(offer);
    }
    close.sort_by(|a, b| {
        (a.0, a.1)
            .cmp(&(b.0, b.1))
            .then_with(|| a.2.as_str().cmp(b.2.as_str()))
    });
    close.truncate(MAX_TYPOS);
    for (_, _, fqn) in close {
        let name = imports.class(db, cx.ws, &fqn);
        let range = d.range;
        out.push(
            Offer::new(
                format!("typo.class:{}", fqn.key()),
                format!("Change to `{}`", fqn.short()),
                Kind::QuickFix,
                {
                    let text = name.text.clone();
                    let import = name.import.clone();
                    move || Ok(import_change(cx, range, text, import))
                },
            )
            .fixing(d.shown, &d.code),
        );
    }
}

fn function_name<'c>(cx: &'c Cx<'c>, d: &Found, out: &mut Vec<Offer<'c>>) {
    let Some(written) = written_name(cx, d.range) else {
        return;
    };
    let db = cx.db;
    let index = stir_hir::def_index(db, cx.ws);
    let Some(imports) = Imports::new(db, cx.file, d.range.start()) else {
        return;
    };
    let namespace = file_items(db, cx.file).scope_at(d.range.start()).namespace.clone();
    let bare = written.trim_start_matches('\\');
    let short = bare.rsplit('\\').next().unwrap_or(bare);
    let mut exact = Vec::new();
    let mut close = Vec::new();
    for (key, locs) in &index.functions {
        let Some(loc) = locs.first() else { continue };
        let last = key.rsplit('\\').next().unwrap_or(key);
        let same = last.eq_ignore_ascii_case(short);
        let typo = if same { None } else { typo_distance(short, last) };
        if !same && typo.is_none() {
            continue;
        }
        if !nav::extension_is_offered(db, cx.ws, loc.file) {
            continue;
        }
        let def = &file_items(db, loc.file).functions[loc.index as usize];
        let rank = (
            namespace_distance(&def.name, &namespace),
            def.name.as_str().len(),
        );
        if same {
            exact.push((rank, def.name.clone()));
        } else if let Some(typo) = typo {
            close.push((typo, rank, def.name.clone()));
        }
    }
    exact.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.as_str().cmp(b.1.as_str())));
    exact.truncate(MAX_IMPORTS);
    let single = exact.len() == 1;
    for (_, fqn) in exact {
        let name = imports.function(db, cx.ws, &fqn);
        let title = if name.import.is_some() {
            format!("Import function `{}`", fqn.as_str())
        } else {
            format!("Change to `{}`", name.text)
        };
        let range = d.range;
        let mut offer = Offer::new(
            format!("import.function:{}", fqn.key()),
            title,
            Kind::QuickFix,
            move || Ok(import_change(cx, range, name.text, name.import)),
        )
        .fixing(d.shown, &d.code);
        if single {
            offer = offer.preferred();
        }
        out.push(offer);
    }
    close.sort_by(|a, b| {
        (a.0, a.1)
            .cmp(&(b.0, b.1))
            .then_with(|| a.2.as_str().cmp(b.2.as_str()))
    });
    close.truncate(MAX_TYPOS);
    for (_, _, fqn) in close {
        let name = imports.function(db, cx.ws, &fqn);
        let range = d.range;
        out.push(
            Offer::new(
                format!("typo.function:{}", fqn.key()),
                format!("Change to `{}`", fqn.short()),
                Kind::QuickFix,
                move || Ok(import_change(cx, range, name.text, name.import)),
            )
            .fixing(d.shown, &d.code),
        );
    }
}

fn constant_name<'c>(cx: &'c Cx<'c>, d: &Found, out: &mut Vec<Offer<'c>>) {
    let Some(written) = written_name(cx, d.range) else {
        return;
    };
    let db = cx.db;
    let index = stir_hir::def_index(db, cx.ws);
    let Some(imports) = Imports::new(db, cx.file, d.range.start()) else {
        return;
    };
    let bare = written.trim_start_matches('\\');
    let short = bare.rsplit('\\').next().unwrap_or(bare);
    let mut exact: Vec<Fqn> = Vec::new();
    let mut close = Vec::new();
    for (key, locs) in &index.consts {
        let Some(loc) = locs.first() else { continue };
        let last = key.rsplit('\\').next().unwrap_or(key);
        let same = last == short;
        let typo = if same { None } else { typo_distance(short, last) };
        if !same && typo.is_none() {
            continue;
        }
        if !nav::extension_is_offered(db, cx.ws, loc.file) {
            continue;
        }
        let def = &file_items(db, loc.file).consts[loc.index as usize];
        if same {
            exact.push(def.name.clone());
        } else if let Some(typo) = typo {
            close.push((typo, def.name.clone()));
        }
    }
    exact.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    exact.truncate(MAX_IMPORTS);
    let single = exact.len() == 1;
    for fqn in exact {
        let name = imports.constant(db, cx.ws, &fqn);
        let title = if name.import.is_some() {
            format!("Import constant `{}`", fqn.as_str())
        } else {
            format!("Change to `{}`", name.text)
        };
        let range = d.range;
        let mut offer = Offer::new(
            format!("import.const:{}", fqn.as_str()),
            title,
            Kind::QuickFix,
            move || Ok(import_change(cx, range, name.text, name.import)),
        )
        .fixing(d.shown, &d.code);
        if single {
            offer = offer.preferred();
        }
        out.push(offer);
    }
    close.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.as_str().cmp(b.1.as_str())));
    close.truncate(MAX_TYPOS);
    for (_, fqn) in close {
        let name = imports.constant(db, cx.ws, &fqn);
        let range = d.range;
        out.push(
            Offer::new(
                format!("typo.const:{}", fqn.as_str()),
                format!("Change to `{}`", fqn.short()),
                Kind::QuickFix,
                move || Ok(import_change(cx, range, name.text, name.import)),
            )
            .fixing(d.shown, &d.code),
        );
    }
}

/// The class around `node`, with the file it's in as the place to look it up.
pub fn enclosing_class_key(cx: &Cx<'_>, file: SourceFile, node: &SyntaxNode) -> Option<SmolStr> {
    let body = node.ancestors().find(|n| n.kind() == CLASS_BODY)?;
    let owner = body.parent()?;
    let range = owner.text_range();
    file_items(cx.db, file)
        .classes
        .iter()
        .find(|c| c.ptr.text_range() == range)
        .map(|c| c.name.key())
}

/// The class type of the receiver of a member's name.
fn receiver_class(cx: &Cx<'_>, fetch: &SyntaxNode) -> Option<(Ty, bool)> {
    let recv = fetch.children().next()?;
    let typed = cx.typed();
    if recv.kind() == NAME_EXPR {
        let text = recv.text().to_string();
        let lower = text.trim().to_ascii_lowercase();
        let class = enclosing_class_key(cx, cx.file, fetch);
        let key = match lower.as_str() {
            "self" | "static" => class,
            "parent" => {
                let c = cx.ctx().class(&class?)?;
                let parent = cx.ctx().supers(&c).parent.clone()?;
                let TyKind::Class { name, .. } = parent.kind() else {
                    return None;
                };
                Some(SmolStr::new(name.to_ascii_lowercase()))
            }
            _ => match nav::def_at(cx.db, cx.ws, cx.file, recv.text_range().start()) {
                Some((_, Def::Class(key))) => Some(key),
                _ => None,
            },
        }?;
        let c = cx.ctx().class(&key)?;
        return Some((c.self_ty(), true));
    }
    let ty = typed.types.get(&recv.text_range())?.clone();
    let ty = stir_types::without_null(&ty);
    match ty.kind() {
        TyKind::Class { .. } | TyKind::SelfType { .. } => Some((ty, false)),
        _ => None,
    }
}

fn member_typo<'c>(cx: &'c Cx<'c>, d: &Found, method: bool, out: &mut Vec<Offer<'c>>) {
    let Some(token) = cx.token_at(d.range.start()) else {
        return;
    };
    let Some(fetch) = token.parent_ancestors().find(|n| {
        matches!(
            n.kind(),
            PROPERTY_FETCH_EXPR
                | NULLSAFE_PROPERTY_FETCH_EXPR
                | CLASS_CONST_FETCH_EXPR
                | STATIC_PROPERTY_FETCH_EXPR
        )
    }) else {
        return;
    };
    let is_static_prop = fetch.kind() == STATIC_PROPERTY_FETCH_EXPR;
    let written = cx.slice(token.text_range());
    if !d.range.contains_range(token.text_range()) {
        return;
    }
    let bare = written.trim_start_matches('$');
    let Some((ty, _)) = receiver_class(cx, &fetch) else {
        return;
    };
    let own = enclosing_class_key(cx, cx.file, &fetch);
    let mut found = Vec::new();
    for m in model_api::members(cx.ctx(), &ty) {
        let right_kind = match &m.ty {
            MemberTy::Method(_) => method,
            MemberTy::Property { .. } => !method,
            _ => false,
        };
        if !right_kind || (!method && m.is_static != is_static_prop) {
            continue;
        }
        let reachable = match m.visibility {
            Visibility::Public => true,
            Visibility::Private => own.as_deref() == Some(m.class.as_str()),
            Visibility::Protected => own
                .as_deref()
                .is_some_and(|o| cx.ctx().is_subclass(o, &m.class)),
        };
        if !reachable {
            continue;
        }
        if let Some(distance) = typo_distance(bare, &m.name) {
            found.push((distance, m.name.clone()));
        }
    }
    found.sort();
    found.truncate(MAX_TYPOS);
    let range = token.text_range();
    let single = found.len() == 1;
    for (_, name) in found {
        let text = if is_static_prop {
            format!("${name}")
        } else {
            name.to_string()
        };
        let mut offer = Offer::new(
            format!("typo.member:{name}"),
            format!("Change to `{text}`"),
            Kind::QuickFix,
            move || Ok(Change::new(vec![cx.edit(range, text)])),
        )
        .fixing(d.shown, &d.code);
        if single {
            offer = offer.preferred();
        }
        out.push(offer);
    }
}

/// The names variables have in the function-like around `node`, those declared before `before`.
fn locals_before(cx: &Cx<'_>, node: &SyntaxNode, before: TextRange) -> FxHashSet<String> {
    let function = nav::enclosing_function(node);
    let scope = function
        .as_ref()
        .map_or_else(|| cx.root.text_range(), |f| f.text_range());
    let typed = cx.typed();
    let mut names = FxHashSet::default();
    for decl in typed.locals.keys() {
        if !scope.contains_range(*decl) || decl.start() >= before.start() {
            continue;
        }
        let Some(t) = cx.token_at(decl.start()) else {
            continue;
        };
        if t.kind() != VARIABLE || t.text_range() != *decl {
            continue;
        }
        // a closure's own, unless this is in it
        if t.parent().and_then(|p| nav::enclosing_function(&p)) != function {
            continue;
        }
        names.insert(t.text().trim_start_matches('$').to_owned());
    }
    names
}

fn variable_typo<'c>(cx: &'c Cx<'c>, d: &Found, out: &mut Vec<Offer<'c>>) {
    let Some(token) = cx.token_at(d.range.start()) else {
        return;
    };
    if token.kind() != VARIABLE {
        return;
    }
    let bare = token.text().trim_start_matches('$').to_owned();
    if bare == "this" {
        return;
    }
    let Some(parent) = token.parent() else { return };
    let names = locals_before(cx, &parent, token.text_range());
    // it's there, only not on every path: not a typo
    if names.contains(&bare) {
        return;
    }
    let mut found: Vec<(usize, String)> = names
        .into_iter()
        .filter(|n| n != "this")
        .filter_map(|n| Some((typo_distance(&bare, &n)?, n)))
        .collect();
    found.sort();
    found.truncate(MAX_TYPOS);
    let range = token.text_range();
    let single = found.len() == 1;
    for (_, name) in found {
        let text = format!("${name}");
        let mut offer = Offer::new(
            format!("typo.variable:{name}"),
            format!("Change to `{text}`"),
            Kind::QuickFix,
            move || Ok(Change::new(vec![cx.edit(range, text)])),
        )
        .fixing(d.shown, &d.code);
        if single {
            offer = offer.preferred();
        }
        out.push(offer);
    }
}
