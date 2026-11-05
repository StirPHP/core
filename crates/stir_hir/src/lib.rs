//! High-level intermediate representation: declarations, name resolution and
//! the program-wide index, as salsa queries.

pub mod ast;
pub mod attrs;
pub mod doc;
mod index;
mod items;
pub mod name;
mod overlay;
pub mod type_ref;

use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use std::sync::Arc;

use stir_db::{Db, FileOrigin, SourceFile, Workspace};

pub use index::{DefIndex, ItemLoc};
pub use items::*;

/// What a [`Symbol`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Class(ClassKind),
    Function,
    Const,
}

/// A name a program's code can mention from anywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub name: name::Fqn,
    /// The name's last segment, in lowercase, for matching.
    pub short_lower: SmolStr,
    pub file: SourceFile,
    /// In the file's list of items of that kind.
    pub index: u32,
}

/// The names of what one file declares. Without positions, so that editing the file doesn't change
/// this unless a declaration does.
#[salsa::tracked(returns(ref))]
pub fn file_symbols(db: &dyn Db, file: SourceFile) -> Arc<Vec<Symbol>> {
    let items = file_items(db, file);
    let symbol = |kind, name: &name::Fqn, index: usize| Symbol {
        kind,
        name: name.clone(),
        short_lower: SmolStr::new(name.short().to_ascii_lowercase()),
        file,
        index: index as u32,
    };
    let classes = items
        .classes
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.is_anonymous())
        .map(|(i, c)| symbol(SymbolKind::Class(c.kind), &c.name, i));
    let functions = items
        .functions
        .iter()
        .enumerate()
        .map(|(i, f)| symbol(SymbolKind::Function, &f.name, i));
    let consts = items
        .consts
        .iter()
        .enumerate()
        .map(|(i, c)| symbol(SymbolKind::Const, &c.name, i));
    Arc::new(classes.chain(functions).chain(consts).collect())
}

/// Every name of the workspace, once each: the declarations a name resolves to. Not re-made for a
/// keystroke that changes no declaration, so that completion can walk it.
#[salsa::tracked(returns(ref))]
pub fn symbol_table(db: &dyn Db, ws: Workspace) -> Arc<Vec<Symbol>> {
    let index = def_index(db, ws);
    let first = |symbol: &Symbol| {
        let (locs, key) = match symbol.kind {
            SymbolKind::Class(_) => (&index.classes, symbol.name.key()),
            SymbolKind::Function => (&index.functions, symbol.name.key()),
            SymbolKind::Const => (&index.consts, symbol.name.0.clone()),
        };
        locs.get(&key)
            .and_then(|l| l.first())
            .is_some_and(|l| l.file == symbol.file && l.index == symbol.index)
    };
    let mut out = Vec::new();
    for &file in ws.files(db) {
        out.extend(file_symbols(db, file).iter().filter(|s| first(s)).cloned());
    }
    Arc::new(out)
}

/// Declarations of one file, as it has them.
#[salsa::tracked(returns(ref))]
fn own_items(db: &dyn Db, file: SourceFile) -> Arc<FileItems> {
    let parse = stir_db::parse(db, file);
    let versions = if file.origin(db) == FileOrigin::Stub {
        items::StubVersions::for_stub(file.path(db), stir_db::php_versions(db))
    } else {
        items::StubVersions::default()
    };
    Arc::new(items::lower_file(&parse.syntax(), versions))
}

/// By lowercase name, what the stub files say of classes, in the order they're to be laid.
#[salsa::tracked(returns(ref))]
fn overlay_classes(db: &dyn Db) -> Arc<FxHashMap<SmolStr, Vec<ClassDef>>> {
    let mut out: FxHashMap<SmolStr, Vec<ClassDef>> = FxHashMap::default();
    let Some(ws) = Workspace::try_get(db) else {
        return Arc::new(out);
    };
    for &file in ws.files(db) {
        if stir_db::is_overlay(file.path(db)) {
            for class in &own_items(db, file).classes {
                out.entry(class.name.key()).or_default().push(class.clone());
            }
        }
    }
    Arc::new(out)
}

/// Declarations of one file, with what stub files say of them.
#[salsa::tracked(returns(ref))]
pub fn file_items(db: &dyn Db, file: SourceFile) -> Arc<FileItems> {
    let own = own_items(db, file);
    if stir_db::is_overlay(file.path(db)) {
        return own.clone();
    }
    let overlays = overlay_classes(db);
    if !own
        .classes
        .iter()
        .any(|c| overlays.contains_key(&c.name.key()))
    {
        return own.clone();
    }
    let mut items = own.as_ref().clone();
    for class in &mut items.classes {
        for over in overlays.get(&class.name.key()).into_iter().flatten() {
            overlay::lay_class(class, over);
        }
    }
    Arc::new(items)
}

/// Every declaration in the workspace.
#[salsa::tracked(returns(ref))]
pub fn def_index(db: &dyn Db, ws: Workspace) -> Arc<DefIndex> {
    let mut index = DefIndex::default();
    for &file in ws.files(db) {
        index.origins.insert(file, file.origin(db));
        index.ranks.insert(file, DefIndex::rank(file.origin(db)));
        let items = file_items(db, file);
        for limited in &items.limited {
            let (map, key) = match limited.kind {
                LimitedKind::Class => (&mut index.limited_classes, limited.name.key()),
                LimitedKind::Function => (&mut index.limited_functions, limited.name.key()),
                LimitedKind::Const => (&mut index.limited_consts, limited.name.0.clone()),
            };
            map.entry(key)
                .and_modify(|a| *a = a.either(limited.availability))
                .or_insert(limited.availability);
        }
        for (i, c) in items.classes.iter().enumerate() {
            index
                .classes
                .entry(c.name.key())
                .or_default()
                .push(ItemLoc {
                    file,
                    index: i as u32,
                });
        }
        for (i, f) in items.functions.iter().enumerate() {
            let loc = ItemLoc {
                file,
                index: i as u32,
            };
            index.functions.entry(f.name.key()).or_default().push(loc);
        }
        for (i, c) in items.consts.iter().enumerate() {
            index
                .consts
                .entry(c.name.0.clone())
                .or_default()
                .push(ItemLoc {
                    file,
                    index: i as u32,
                });
        }
    }
    index.sort();
    // our own stubs replace the phpstorm-stubs declarations of a function, and a project's replace
    // every other
    let in_project_stub = |l: &ItemLoc| l.file.path(db).starts_with("stubs://overlay");
    let in_our_stub = |l: &ItemLoc| l.file.path(db).starts_with("stubs://override");
    for locs in index.functions.values_mut() {
        if locs.iter().any(in_project_stub) {
            locs.retain(in_project_stub);
        } else if locs.iter().any(in_our_stub) {
            locs.retain(|l| in_our_stub(l) || l.file.origin(db) != FileOrigin::Stub);
        }
    }
    // a class is declared where it's declared, and only without that by a stub
    let overlaid = |l: &ItemLoc| stir_db::is_overlay(l.file.path(db));
    for locs in index.classes.values_mut() {
        if !locs.iter().all(overlaid) {
            locs.retain(|l| !overlaid(l));
        }
    }
    Arc::new(index)
}

/// By lowercase trait name, the project's classes and traits that use the trait directly,
/// sorted. A vendor class using a trait says nothing about the project's code.
#[salsa::tracked(returns(ref))]
pub fn trait_uses(db: &dyn Db, ws: Workspace) -> Arc<FxHashMap<SmolStr, Vec<SmolStr>>> {
    let mut uses: FxHashMap<SmolStr, Vec<SmolStr>> = FxHashMap::default();
    for &file in ws.files(db) {
        if file.origin(db) != stir_db::FileOrigin::Source {
            continue;
        }
        for c in &file_items(db, file).classes {
            for t in &c.traits {
                if let type_ref::TypeRefKind::Named(name, _) = &t.kind {
                    uses.entry(name.key()).or_default().push(c.name.key());
                }
            }
        }
    }
    for users in uses.values_mut() {
        users.sort();
        users.dedup();
    }
    Arc::new(uses)
}

/// By lowercase name, the classes that extend, implement or use a class-like directly, sorted.
/// Everything in the workspace counts: vendor classes and stubs implement interfaces too.
#[salsa::tracked(returns(ref))]
pub fn direct_subtypes(db: &dyn Db, ws: Workspace) -> Arc<FxHashMap<SmolStr, Vec<SmolStr>>> {
    let mut subs: FxHashMap<SmolStr, Vec<SmolStr>> = FxHashMap::default();
    for &file in ws.files(db) {
        for c in &file_items(db, file).classes {
            for t in c.extends.iter().chain(&c.implements).chain(&c.traits) {
                if let type_ref::TypeRefKind::Named(name, _) = &t.kind {
                    subs.entry(name.key()).or_default().push(c.name.key());
                }
            }
        }
    }
    for list in subs.values_mut() {
        list.sort();
        list.dedup();
    }
    Arc::new(subs)
}

#[cfg(test)]
mod tests;
