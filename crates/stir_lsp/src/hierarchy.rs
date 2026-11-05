//! How declarations relate: what implements or overrides what, and who calls whom.

use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;
use stir_checker::{Def, MemberKind};
use stir_db::{Db, FileOrigin, SourceFile, Workspace};
use stir_hir::type_ref::TypeRefKind;
use stir_hir::{ClassDef, ClassKind, MethodDef, Visibility, file_items};
use stir_syntax::{SyntaxKind::*, SyntaxNode, TextRange, TextSize};

use crate::nav::{self, NavTarget, class_loc};

/// The class-like with lowercase name `key`, with the file it's in.
pub fn class_def<'db>(
    db: &'db dyn Db,
    ws: Workspace,
    key: &str,
) -> Option<(SourceFile, &'db ClassDef)> {
    let loc = class_loc(db, ws, key)?;
    Some((
        loc.file,
        &file_items(db, loc.file).classes[loc.index as usize],
    ))
}

fn names(list: &[stir_hir::type_ref::TypeRef]) -> impl Iterator<Item = SmolStr> + '_ {
    list.iter().filter_map(|t| match &t.kind {
        TypeRefKind::Named(name, _) => Some(name.key()),
        _ => None,
    })
}

/// What `class` extends, implements and uses directly.
fn direct_supertypes(class: &ClassDef) -> Vec<SmolStr> {
    names(&class.extends)
        .chain(names(&class.implements))
        .chain(names(&class.traits))
        .collect()
}

/// Everything under `key`, nearest first, without `key`. Stubs of extensions the project doesn't
/// mention aren't its business.
pub fn all_subtypes(db: &dyn Db, ws: Workspace, key: &str) -> Vec<SmolStr> {
    let graph = stir_hir::direct_subtypes(db, ws);
    let mut seen: FxHashSet<SmolStr> = FxHashSet::default();
    seen.insert(SmolStr::new(key));
    let mut out = Vec::new();
    let mut next: Vec<SmolStr> = vec![SmolStr::new(key)];
    while !next.is_empty() {
        let mut level = Vec::new();
        for k in &next {
            for sub in graph.get(k).into_iter().flatten() {
                if seen.insert(sub.clone()) {
                    level.push(sub.clone());
                }
            }
        }
        out.extend(level.iter().cloned());
        next = level;
    }
    out.retain(|k| class_loc(db, ws, k).is_some_and(|l| nav::extension_is_offered(db, ws, l.file)));
    out
}

/// Which declaration a class gets a method from: its own, its traits', then its parent's. By the
/// class declaring it, with the declaration.
fn resolve_method<'db>(
    db: &'db dyn Db,
    ws: Workspace,
    key: &str,
    name: &str,
) -> Option<(SmolStr, &'db MethodDef)> {
    let mut seen = FxHashSet::default();
    resolve_method_in(db, ws, key, name, &mut seen)
}

fn resolve_method_in<'db>(
    db: &'db dyn Db,
    ws: Workspace,
    key: &str,
    name: &str,
    seen: &mut FxHashSet<SmolStr>,
) -> Option<(SmolStr, &'db MethodDef)> {
    if !seen.insert(SmolStr::new(key)) {
        return None;
    }
    let (_, class) = class_def(db, ws, key)?;
    if let Some(m) = class
        .methods
        .iter()
        .find(|m| !m.magic && m.name.eq_ignore_ascii_case(name))
    {
        return Some((class.name.key(), m));
    }
    let from_traits = names(&class.traits).find_map(|t| resolve_method_in(db, ws, &t, name, seen));
    from_traits.or_else(|| {
        if class.kind == ClassKind::Interface {
            return None;
        }
        names(&class.extends).find_map(|p| resolve_method_in(db, ws, &p, name, seen))
    })
}

fn target_of(db: &dyn Db, ws: Workspace, file: SourceFile, def: &Def, out: &mut Vec<NavTarget>) {
    if let Some(t) = nav::target(db, ws, file, def)
        && !out.contains(&t)
    {
        out.push(t);
    }
}

fn method_def(class: SmolStr, name: &str) -> Def {
    Def::Member {
        class,
        kind: MemberKind::Method,
        name: SmolStr::new(name),
    }
}

/// Is this a method a call can actually end up in, rather than a promise of one?
fn is_concrete(class_kind: ClassKind, m: &MethodDef) -> bool {
    class_kind != ClassKind::Interface && !m.mods.is_abstract && m.has_body
}

/// What `def` can stand for at run time: the classes under a class-like, and the declarations of a
/// member under its class, with what's inherited from elsewhere where an interface needs it. A
/// method that has a body counts itself.
pub fn implementations(db: &dyn Db, ws: Workspace, file: SourceFile, def: &Def) -> Vec<NavTarget> {
    let mut out = Vec::new();
    match def {
        Def::Class(key) => {
            for sub in all_subtypes(db, ws, key) {
                let Some((_, class)) = class_def(db, ws, &sub) else {
                    continue;
                };
                if class.kind != ClassKind::Interface {
                    target_of(db, ws, file, &Def::Class(sub), &mut out);
                }
            }
        }
        Def::Member {
            class,
            kind: MemberKind::Method,
            name,
        } => {
            let own = resolve_method(db, ws, class, name);
            if own.as_ref().is_some_and(|(_, m)| {
                m.mods.visibility == Visibility::Private
                    || m.mods.is_final
                    || name.eq_ignore_ascii_case("__construct")
            }) {
                target_of(db, ws, file, def, &mut out);
                return out;
            }
            let concrete = |key: &str, m: &MethodDef| {
                class_def(db, ws, key).is_some_and(|(_, c)| is_concrete(c.kind, m))
            };
            if let Some((declaring, m)) = &own
                && concrete(declaring, m)
            {
                target_of(db, ws, file, &method_def(declaring.clone(), name), &mut out);
            }
            for sub in all_subtypes(db, ws, class) {
                if let Some((declaring, m)) = resolve_method(db, ws, &sub, name)
                    && concrete(&declaring, m)
                {
                    target_of(db, ws, file, &method_def(declaring, name), &mut out);
                }
            }
        }
        Def::Member {
            class, kind, name, ..
        } => {
            target_of(db, ws, file, def, &mut out);
            for sub in all_subtypes(db, ws, class) {
                let Some((_, c)) = class_def(db, ws, &sub) else {
                    continue;
                };
                let redeclared = match kind {
                    MemberKind::Property => {
                        c.properties.iter().any(|p| !p.magic && &p.name == name)
                    }
                    MemberKind::Const => c.consts.iter().any(|k| &k.name == name),
                    MemberKind::Case | MemberKind::Method => false,
                };
                if redeclared {
                    let def = Def::Member {
                        class: sub.clone(),
                        kind: *kind,
                        name: name.clone(),
                    };
                    target_of(db, ws, file, &def, &mut out);
                }
            }
        }
        Def::Function(_) | Def::Const(_) => target_of(db, ws, file, def, &mut out),
        Def::Local { .. } => {}
    }
    out
}

/// Where `def` is first promised: for a method, the declarations highest up that it overrides.
/// Everything else is declared where it's defined.
pub fn declarations(db: &dyn Db, ws: Workspace, file: SourceFile, def: &Def) -> Vec<NavTarget> {
    let mut out = Vec::new();
    if let Def::Member {
        class,
        kind: MemberKind::Method,
        name,
    } = def
    {
        let private = resolve_method(db, ws, class, name)
            .is_some_and(|(_, m)| m.mods.visibility == Visibility::Private);
        if !private {
            let mut roots = Vec::new();
            let mut seen = FxHashSet::default();
            method_roots(db, ws, class, name, &mut seen, &mut roots);
            for root in roots {
                target_of(db, ws, file, &method_def(root, name), &mut out);
            }
        }
    }
    if out.is_empty() {
        target_of(db, ws, file, def, &mut out);
    }
    out
}

/// The classes above and including `key` that declare a method that nothing above them does.
/// Returns whether anything from `key` up declares it.
fn method_roots(
    db: &dyn Db,
    ws: Workspace,
    key: &str,
    name: &str,
    seen: &mut FxHashSet<SmolStr>,
    roots: &mut Vec<SmolStr>,
) -> bool {
    if !seen.insert(SmolStr::new(key)) {
        return false;
    }
    let Some((_, class)) = class_def(db, ws, key) else {
        return false;
    };
    let mut above = false;
    for sup in direct_supertypes(class) {
        above |= method_roots(db, ws, &sup, name, seen, roots);
    }
    let own = class
        .methods
        .iter()
        .find(|m| !m.magic && m.name.eq_ignore_ascii_case(name));
    // a constructor is only a promise where it has to be kept
    let binding = own.is_some_and(|m| {
        !name.eq_ignore_ascii_case("__construct")
            || class.kind == ClassKind::Interface
            || m.mods.is_abstract
    });
    if !above && binding {
        roots.push(class.name.key());
    }
    above || binding
}

/// What kind of thing a [`CallItem`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    Function,
    Method,
    Constructor,
    /// A file's top-level code.
    File,
}

/// One end of a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallItem {
    pub name: String,
    pub kind: CallKind,
    /// The class of a method, or the namespace of a function.
    pub detail: Option<String>,
    pub file: SourceFile,
    pub full_range: TextRange,
    pub focus_range: TextRange,
}

/// The name of the class around `offset`.
fn class_at(db: &dyn Db, file: SourceFile, offset: TextSize) -> Option<&stir_hir::name::Fqn> {
    file_items(db, file)
        .classes
        .iter()
        .filter(|c| c.ptr.text_range().contains(offset))
        .min_by_key(|c| c.ptr.text_range().len())
        .map(|c| &c.name)
}

fn callee_item(db: &dyn Db, ws: Workspace, file: SourceFile, def: &Def) -> Option<CallItem> {
    let t = nav::target(db, ws, file, def)?;
    let (name, kind, detail) = match def {
        Def::Function(loc) => {
            let f = &file_items(db, loc.file).functions[loc.index as usize];
            let ns = f.name.namespace();
            let detail = (!ns.is_empty()).then(|| ns.to_owned());
            (f.name.short().to_owned(), CallKind::Function, detail)
        }
        Def::Member { class, name, .. } => {
            let (_, c) = class_def(db, ws, class)?;
            let kind = if name.eq_ignore_ascii_case("__construct") {
                CallKind::Constructor
            } else {
                CallKind::Method
            };
            (name.to_string(), kind, Some(c.name.as_str().to_owned()))
        }
        _ => return None,
    };
    Some(CallItem {
        name,
        kind,
        detail,
        file: t.file,
        full_range: t.full_range,
        focus_range: t.focus_range,
    })
}

/// The constructor a `new` of `class` runs, whichever class declares it. A class without one
/// stands for it itself.
fn constructor(db: &dyn Db, ws: Workspace, class: &str) -> Def {
    let declaring = resolve_method(db, ws, class, "__construct").map(|(c, _)| c);
    method_def(
        declaring.unwrap_or_else(|| SmolStr::new(class)),
        "__construct",
    )
}

/// What can be called: a function, a method, or what `new` runs for a class.
pub fn callable_at(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    offset: TextSize,
) -> Option<(Def, CallItem)> {
    let (_, def) = nav::def_at(db, ws, file, offset)?;
    let def = match def {
        Def::Function(_) => def,
        Def::Member {
            kind: MemberKind::Method,
            ..
        } => def,
        Def::Class(key) => constructor(db, ws, &key),
        _ => return None,
    };
    let item = callee_item(db, ws, file, &def)?;
    Some((def, item))
}

/// The nearest node around the name at `range` that isn't just more of the name.
fn around_name(root: &SyntaxNode, range: TextRange) -> Option<SyntaxNode> {
    root.token_at_offset(range.start())
        .right_biased()?
        .parent_ancestors()
        .find(|n| !matches!(n.kind(), NAME | NAME_REF | NAME_EXPR))
}

/// Is the name of a function or method at `range` what a call calls?
fn is_call(root: &SyntaxNode, range: TextRange) -> bool {
    let Some(node) = around_name(root, range) else {
        return false;
    };
    // a call of a member wraps the fetch of it
    let node = match node.kind() {
        PROPERTY_FETCH_EXPR
        | NULLSAFE_PROPERTY_FETCH_EXPR
        | STATIC_PROPERTY_FETCH_EXPR
        | CLASS_CONST_FETCH_EXPR => node.parent(),
        _ => Some(node),
    };
    node.is_some_and(|n| {
        matches!(
            n.kind(),
            CALL_EXPR | METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR
        )
    })
}

/// Is the name of a class at `range` what a `new` creates?
fn is_new(root: &SyntaxNode, range: TextRange) -> bool {
    around_name(root, range).is_some_and(|n| n.kind() == NEW_EXPR)
}

/// A function-like with a name of its own around the node, for calls to be attributed to.
fn enclosing_callable(node: &SyntaxNode) -> Option<SyntaxNode> {
    node.ancestors()
        .find(|n| matches!(n.kind(), FUNCTION_DECL | METHOD_DECL))
}

fn caller_item(db: &dyn Db, file: SourceFile, node: Option<SyntaxNode>) -> CallItem {
    let named = node.and_then(|n| {
        let name = n.children().find(|c| c.kind() == NAME)?;
        Some((n, name))
    });
    match named {
        Some((n, name)) => {
            let text = name.text().to_string();
            let start = n.text_range().start();
            let (kind, detail) = if n.kind() == METHOD_DECL {
                let kind = if text.trim().eq_ignore_ascii_case("__construct") {
                    CallKind::Constructor
                } else {
                    CallKind::Method
                };
                let class = class_at(db, file, start).filter(|c| !is_anonymous(c));
                (kind, class.map(|c| c.as_str().to_owned()))
            } else {
                let scope = file_items(db, file).scope_at(start);
                let ns = scope.namespace.to_string();
                (CallKind::Function, (!ns.is_empty()).then_some(ns))
            };
            CallItem {
                name: text.trim().to_owned(),
                kind,
                detail,
                file,
                full_range: n.text_range(),
                focus_range: name.text_range(),
            }
        }
        None => {
            let path = file.path(db);
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            let end = TextSize::of(file.text(db).as_str());
            CallItem {
                name,
                kind: CallKind::File,
                detail: None,
                file,
                full_range: TextRange::new(0.into(), end),
                focus_range: TextRange::empty(0.into()),
            }
        }
    }
}

fn is_anonymous(name: &stir_hir::name::Fqn) -> bool {
    name.as_str().starts_with("class@anonymous")
}

/// The call sites of a method may be written against anything it overrides: all of those.
fn callee_names(db: &dyn Db, ws: Workspace, def: &Def) -> Vec<Def> {
    let Def::Member {
        class,
        kind: MemberKind::Method,
        name,
    } = def
    else {
        return vec![def.clone()];
    };
    let mut out = vec![def.clone()];
    if name.eq_ignore_ascii_case("__construct") {
        return out;
    }
    let mut seen = FxHashSet::default();
    let mut next = vec![class.clone()];
    while let Some(key) = next.pop() {
        if !seen.insert(key.clone()) {
            continue;
        }
        let Some((_, c)) = class_def(db, ws, &key) else {
            continue;
        };
        let private = |m: &MethodDef| m.mods.visibility == Visibility::Private;
        if c.methods
            .iter()
            .any(|m| !m.magic && !private(m) && m.name.eq_ignore_ascii_case(name))
        {
            let d = method_def(key.clone(), name);
            if !out.contains(&d) {
                out.push(d);
            }
        }
        next.extend(direct_supertypes(c));
    }
    out
}

/// Who calls `def`, grouped by caller, with where in the caller. Only the project's code is
/// searched, as for references: vendor code is never checked.
pub fn incoming_calls(db: &dyn Db, ws: Workspace, def: &Def) -> Vec<(CallItem, Vec<TextRange>)> {
    let wanted = callee_names(db, ws, def);
    let spelled = match def {
        Def::Member { name, .. } if name.eq_ignore_ascii_case("__construct") => None,
        Def::Member { name, .. } => Some(name.to_ascii_lowercase()),
        Def::Function(loc) => {
            let f = &file_items(db, loc.file).functions[loc.index as usize];
            Some(f.name.short().to_ascii_lowercase())
        }
        _ => None,
    };
    let ctor_of = match def {
        Def::Member { class, name, .. } if name.eq_ignore_ascii_case("__construct") => Some(class),
        _ => None,
    };
    let mut callers: FxHashMap<(SourceFile, TextRange), (CallItem, Vec<TextRange>)> =
        FxHashMap::default();
    let sources = ws
        .files(db)
        .iter()
        .copied()
        .filter(|f| f.origin(db) == FileOrigin::Source);
    for file in sources {
        // most files don't spell the name at all, and those don't need checking
        if let Some(name) = &spelled
            && !file.text(db).to_ascii_lowercase().contains(name.as_str())
        {
            continue;
        }
        let root = stir_db::parse(db, file).syntax();
        for (range, d) in nav::file_refs(db, ws, file).iter() {
            let hit = match d {
                Def::Class(key) => {
                    ctor_of.is_some_and(|c| {
                        constructor(db, ws, key) == method_def(c.clone(), "__construct")
                    }) && is_new(&root, *range)
                }
                d => wanted.contains(d) && is_call(&root, *range),
            };
            if !hit {
                continue;
            }
            let Some(token) = root.token_at_offset(range.start()).right_biased() else {
                continue;
            };
            let node = token.parent().and_then(|n| enclosing_callable(&n));
            let item = caller_item(db, file, node);
            let entry = callers
                .entry((file, item.full_range))
                .or_insert_with(|| (item, Vec::new()));
            entry.1.push(*range);
        }
    }
    let mut out: Vec<_> = callers.into_values().collect();
    out.sort_by_key(|(item, _)| {
        (
            item.file.path(db).clone(),
            item.full_range.start(),
            item.full_range.end(),
        )
    });
    out
}

/// What the function-like whose whole declaration is `range` calls, by callee. Calls inside
/// closures in it are its own; top-level code has what's outside any named function.
pub fn outgoing_calls(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    range: TextRange,
) -> Vec<(CallItem, Vec<TextRange>)> {
    let root = stir_db::parse(db, file).syntax();
    let mut groups: Vec<(Def, Vec<TextRange>)> = Vec::new();
    let top_level =
        range.start() == 0.into() && range.len() == TextSize::of(file.text(db).as_str());
    for (r, d) in nav::file_refs(db, ws, file).iter() {
        if !range.contains_range(*r) {
            continue;
        }
        if top_level {
            let inside = root
                .token_at_offset(r.start())
                .right_biased()
                .and_then(|t| t.parent())
                .and_then(|n| enclosing_callable(&n));
            if inside.is_some() {
                continue;
            }
        }
        let callee = match d {
            Def::Function(_)
            | Def::Member {
                kind: MemberKind::Method,
                ..
            } if is_call(&root, *r) => d.clone(),
            Def::Class(key) if is_new(&root, *r) => constructor(db, ws, key),
            _ => continue,
        };
        match groups.iter_mut().find(|(g, _)| *g == callee) {
            Some((_, ranges)) => ranges.push(*r),
            None => groups.push((callee, vec![*r])),
        }
    }
    groups
        .into_iter()
        .filter_map(|(def, ranges)| Some((callee_item(db, ws, file, &def)?, ranges)))
        .collect()
}

/// One class-like in a type hierarchy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeItem {
    /// Its lowercase name, which finds it again.
    pub key: SmolStr,
    pub name: String,
    pub kind: ClassKind,
    /// Its namespace.
    pub detail: Option<String>,
    pub file: SourceFile,
    pub full_range: TextRange,
    pub focus_range: TextRange,
}

pub fn type_item(db: &dyn Db, ws: Workspace, key: &str) -> Option<TypeItem> {
    let (file, class) = class_def(db, ws, key)?;
    let ns = class.name.namespace();
    Some(TypeItem {
        key: SmolStr::new(key),
        name: if class.is_anonymous() {
            "class@anonymous".to_owned()
        } else {
            class.name.short().to_owned()
        },
        kind: class.kind,
        detail: (!ns.is_empty()).then(|| ns.to_owned()),
        file,
        full_range: class.ptr.text_range(),
        focus_range: class.name_range,
    })
}

/// The class-like at `offset`.
pub fn type_at(db: &dyn Db, ws: Workspace, file: SourceFile, offset: TextSize) -> Option<TypeItem> {
    match nav::def_at(db, ws, file, offset)? {
        (_, Def::Class(key)) => type_item(db, ws, &key),
        _ => None,
    }
}

pub fn supertypes(db: &dyn Db, ws: Workspace, key: &str) -> Vec<TypeItem> {
    class_def(db, ws, key).map_or_else(Vec::new, |(_, class)| {
        direct_supertypes(class)
            .iter()
            .filter_map(|k| type_item(db, ws, k))
            .collect()
    })
}

pub fn subtypes(db: &dyn Db, ws: Workspace, key: &str) -> Vec<TypeItem> {
    let graph = stir_hir::direct_subtypes(db, ws);
    graph
        .get(key)
        .into_iter()
        .flatten()
        .filter_map(|k| type_item(db, ws, k))
        .filter(|t| nav::extension_is_offered(db, ws, t.file))
        .collect()
}
