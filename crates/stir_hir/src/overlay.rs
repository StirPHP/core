//! Stub files, PHPStan's and our own: what they say of a class is laid over what's declared of it.
//! The types in their PHPDoc stand in for the types in the class's, and what the declaration doesn't
//! have is added. What isn't declared anywhere else is declared by the stub.

use smol_str::SmolStr;

use crate::items::{ClassDef, MethodDef};
use crate::type_ref::{ConditionalSubjectRef, TypeRef, TypeRefKind};

/// Replaces the supertype of `list` that `sup` names, giving it type arguments, or adds it.
fn lay_super(list: &mut Vec<TypeRef>, others: &mut [TypeRef], sup: &TypeRef) {
    let TypeRefKind::Named(name, _) = &sup.kind else {
        return;
    };
    let same =
        |t: &&mut TypeRef| matches!(&t.kind, TypeRefKind::Named(n, _) if n.key() == name.key());
    if let Some(found) = list.iter_mut().find(|t| same(t)) {
        found.kind = sup.kind.clone();
    } else if let Some(found) = others.iter_mut().find(|t| same(t)) {
        found.kind = sup.kind.clone();
    } else {
        list.push(sup.clone());
    }
}

fn lay_method(method: &mut MethodDef, over: &MethodDef) {
    if !over.sig.type_params.is_empty() {
        method.sig.type_params = over.sig.type_params.clone();
    }
    for (param, stub) in method.sig.params.iter_mut().zip(&over.sig.params) {
        if stub.doc_ty.is_some() {
            param.doc_ty = stub.doc_ty.clone();
        }
        if stub.out_ty.is_some() {
            param.out_ty = stub.out_ty.clone();
        }
        if stub.closure_this.is_some() {
            param.closure_this = stub.closure_this.clone();
        }
        if param.ty.is_none() {
            param.ty = stub.ty.clone();
        }
    }
    if over.sig.doc_ret.is_some() {
        method.sig.doc_ret = over.sig.doc_ret.clone();
    }
    if method.sig.ret.is_none() {
        method.sig.ret = over.sig.ret.clone();
    }
    if !over.sig.asserts.is_empty() {
        method.sig.asserts = over.sig.asserts.clone();
    }
    if over.sig.throws.is_some() {
        method.sig.throws = over.sig.throws.clone();
    }
    if over.deprecated.is_some() {
        method.deprecated = over.deprecated.clone();
    }
}

/// Calls a type parameter by another name, wherever it's mentioned.
fn rename(t: &mut TypeRef, names: &[(SmolStr, SmolStr)]) {
    let many = |list: &mut Vec<TypeRef>| list.iter_mut().for_each(|t| rename(t, names));
    match &mut t.kind {
        TypeRefKind::Param(name) => {
            if let Some((_, to)) = names.iter().find(|(from, _)| from == name) {
                *name = to.clone();
            }
        }
        TypeRefKind::CovariantArg(inner)
        | TypeRefKind::ContravariantArg(inner)
        | TypeRefKind::Nullable(inner) => rename(inner, names),
        TypeRefKind::Conditional {
            subject,
            target,
            then,
            otherwise,
        } => {
            if let ConditionalSubjectRef::Type(subject) = subject {
                rename(subject, names);
            }
            rename(target, names);
            rename(then, names);
            rename(otherwise, names);
        }
        TypeRefKind::SelfGeneric(_, list)
        | TypeRefKind::Named(_, list)
        | TypeRefKind::Union(list)
        | TypeRefKind::Intersection(list) => many(list),
        TypeRefKind::PhpList { elem, .. } => rename(elem, names),
        TypeRefKind::PhpArray { key, value, .. } | TypeRefKind::Iterable { key, value } => {
            if let Some(key) = key {
                rename(key, names);
            }
            rename(value, names);
        }
        TypeRefKind::Shape { fields, .. } => {
            fields.iter_mut().for_each(|f| rename(&mut f.ty, names));
        }
        TypeRefKind::ClassString(Some(inner)) => rename(inner, names),
        TypeRefKind::Callable { params, ret, .. } => {
            params.iter_mut().for_each(|p| rename(&mut p.ty, names));
            rename(ret, names);
        }
        TypeRefKind::Php(_)
        | TypeRefKind::Literal(_)
        | TypeRefKind::IntRange { .. }
        | TypeRefKind::ClassString(None)
        | TypeRefKind::DocKeyword(_)
        | TypeRefKind::ImportedAlias { .. }
        | TypeRefKind::Error => {}
    }
}

/// `over`, with its type parameters called what `class` calls them.
fn renamed_for(class: &ClassDef, over: &ClassDef) -> Option<ClassDef> {
    let names: Vec<(SmolStr, SmolStr)> = over
        .type_params
        .iter()
        .zip(&class.type_params)
        .filter(|(o, c)| o.name != c.name)
        .map(|(o, c)| (o.name.clone(), c.name.clone()))
        .collect();
    if names.is_empty() {
        return None;
    }
    let mut over = over.clone();
    for p in &mut over.type_params {
        if let Some((_, to)) = names.iter().find(|(from, _)| *from == p.name) {
            p.name = to.clone();
        }
        if let Some(bound) = &mut p.bound {
            rename(bound, &names);
        }
    }
    for t in over
        .extends
        .iter_mut()
        .chain(&mut over.implements)
        .chain(&mut over.traits)
        .chain(&mut over.mixins)
    {
        rename(t, &names);
    }
    for m in &mut over.methods {
        for tp in &mut m.sig.type_params {
            if let Some(bound) = &mut tp.bound {
                rename(bound, &names);
            }
        }
        for p in &mut m.sig.params {
            for t in [&mut p.ty, &mut p.doc_ty, &mut p.out_ty, &mut p.closure_this]
                .into_iter()
                .flatten()
            {
                rename(t, &names);
            }
        }
        for t in [&mut m.sig.ret, &mut m.sig.doc_ret, &mut m.sig.throws]
            .into_iter()
            .flatten()
        {
            rename(t, &names);
        }
        for a in &mut m.sig.asserts {
            rename(&mut a.ty, &names);
        }
    }
    for p in &mut over.properties {
        for t in [&mut p.ty, &mut p.doc_ty].into_iter().flatten() {
            rename(t, &names);
        }
    }
    Some(over)
}

/// `class` with `over` laid over it.
pub(crate) fn lay_class(class: &mut ClassDef, over: &ClassDef) {
    // what the class's own PHPDoc says stays right, whatever the stub calls things
    let renamed = renamed_for(class, over);
    let over = renamed.as_ref().unwrap_or(over);
    if !over.type_params.is_empty() {
        class.type_params = over.type_params.clone();
    }
    for sup in &over.extends {
        lay_super(&mut class.extends, &mut class.implements, sup);
    }
    for sup in &over.implements {
        lay_super(&mut class.implements, &mut class.extends, sup);
    }
    for t in &over.traits {
        let TypeRefKind::Named(name, _) = &t.kind else {
            continue;
        };
        let same =
            |x: &&mut TypeRef| matches!(&x.kind, TypeRefKind::Named(n, _) if n.key() == name.key());
        if let Some(found) = class.traits.iter_mut().find(|x| same(x)) {
            found.kind = t.kind.clone();
        }
    }
    for m in &over.methods {
        match class
            .methods
            .iter_mut()
            .find(|x| x.name.eq_ignore_ascii_case(&m.name))
        {
            Some(found) => lay_method(found, m),
            None => {
                // nowhere to go to but the class
                let mut added = m.clone();
                added.magic = true;
                added.has_body = true;
                class.methods.push(added);
            }
        }
    }
    for p in &over.properties {
        match class.properties.iter_mut().find(|x| x.name == p.name) {
            Some(found) => {
                if p.doc_ty.is_some() {
                    found.doc_ty = p.doc_ty.clone();
                }
                if found.ty.is_none() {
                    found.ty = p.ty.clone();
                }
            }
            None => {
                let mut added = p.clone();
                added.magic = true;
                class.properties.push(added);
            }
        }
    }
    for k in &over.consts {
        if let Some(found) = class.consts.iter_mut().find(|x| x.name == k.name)
            && k.doc_ty.is_some()
        {
            found.doc_ty = k.doc_ty.clone();
        }
    }
    let new_aliases: Vec<(SmolStr, TypeRef)> = over
        .type_aliases
        .iter()
        .filter(|(n, _)| !class.type_aliases.iter().any(|(m, _)| m == n))
        .cloned()
        .collect();
    class.type_aliases.extend(new_aliases);
    for (name, ty) in &over.aliases {
        class
            .aliases
            .entry(name.clone())
            .or_insert_with(|| ty.clone());
    }
    class.mixins.extend(over.mixins.iter().cloned());
    if over.deprecated.is_some() {
        class.deprecated = over.deprecated.clone();
    }
}
