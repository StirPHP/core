//! Which other declarations a method's signature is tied to: PHP checks an override against what
//! it overrides, and a call may reach any of them.

use rustc_hash::FxHashSet;
use smol_str::SmolStr;
use stir_db::SourceFile;
use stir_hir::{MethodDef, Visibility, file_items};
use stir_syntax::SyntaxKind::*;
use stir_syntax::SyntaxNode;
use stir_types::TyKind;

use super::Cx;
use crate::hierarchy::{all_subtypes, class_def};

/// A method of a family: a declaration of the method in a related class.
#[derive(Clone, Copy)]
pub struct Member<'c> {
    pub file: SourceFile,
    pub method: &'c MethodDef,
    pub class: &'c stir_hir::ClassDef,
}

impl Member<'_> {
    pub fn key(&self) -> SmolStr {
        self.class.name.key()
    }
}

/// What `class`'s ancestors are, without it.
pub fn ancestors_of(cx: &Cx<'_>, class: &str) -> Vec<SmolStr> {
    let Some(c) = cx.ctx().class(class) else {
        return Vec::new();
    };
    cx.ctx()
        .ancestors(&c.self_ty())
        .iter()
        .filter_map(|t| match t.kind() {
            TyKind::Class { name, .. } => Some(SmolStr::new(name.to_ascii_lowercase())),
            _ => None,
        })
        .filter(|k| k != class)
        .collect()
}

fn declared<'c>(cx: &Cx<'c>, key: &str, name: &str) -> Option<Member<'c>> {
    let (file, class) = class_def(cx.db, cx.ws, key)?;
    let method = class
        .methods
        .iter()
        .find(|m| !m.magic && m.name.eq_ignore_ascii_case(name))?;
    Some(Member {
        file,
        method,
        class,
    })
}

/// Who overrides `name` of `class`, or is overridden by it, over the whole hierarchy: everything
/// that has to keep a compatible signature with it, and every declaration a call can end up in.
/// `class`'s own comes first. A private method and a constructor are on their own, except for a
/// constructor an interface or an abstract method asks for.
pub fn family<'c>(cx: &Cx<'c>, class: &str, name: &str) -> Vec<Member<'c>> {
    let Some(own) = declared(cx, class, name) else {
        return Vec::new();
    };
    let lower = name.to_ascii_lowercase();
    if own.method.mods.visibility == Visibility::Private {
        return vec![own];
    }
    let mut seen: FxHashSet<SmolStr> = FxHashSet::default();
    seen.insert(SmolStr::new(class));
    let mut queue = vec![SmolStr::new(class)];
    let mut out = vec![own];
    while let Some(key) = queue.pop() {
        let related = ancestors_of(cx, &key)
            .into_iter()
            .chain(all_subtypes(cx.db, cx.ws, &key));
        for other in related {
            if !seen.insert(other.clone()) {
                continue;
            }
            let member = declared(cx, &other, &lower);
            let counts = member.as_ref().is_some_and(|m| {
                m.method.mods.visibility != Visibility::Private
                    && (lower != "__construct" || m.method.mods.is_abstract || !m.method.has_body)
            });
            if counts {
                out.extend(member);
                queue.push(other);
            }
        }
    }
    out
}

/// The class a node is a member of, as the items of its file have it.
pub fn class_of<'c>(cx: &Cx<'c>, node: &SyntaxNode) -> Option<&'c stir_hir::ClassDef> {
    let body = node.ancestors().find(|n| n.kind() == CLASS_BODY)?;
    let owner = body.parent()?;
    let range = owner.text_range();
    file_items(cx.db, cx.file)
        .classes
        .iter()
        .find(|c| c.ptr.text_range() == range)
}

/// The declared method a `METHOD_DECL` is.
pub fn method_of<'c>(
    cx: &Cx<'c>,
    node: &SyntaxNode,
) -> Option<(&'c stir_hir::ClassDef, &'c MethodDef)> {
    let class = class_of(cx, node)?;
    let range = node.text_range();
    let method = class.methods.iter().find(|m| m.ptr.text_range() == range)?;
    Some((class, method))
}
