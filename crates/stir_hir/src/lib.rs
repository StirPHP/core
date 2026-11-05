//! High-level intermediate representation: declarations, name resolution and
//! the program-wide index, as salsa queries.

pub mod ast;
pub mod attrs;
pub mod doc;
mod index;
mod items;
pub mod name;
pub mod type_ref;

use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use std::sync::Arc;

use stir_db::{Db, FileOrigin, SourceFile, Workspace};

pub use index::{DefIndex, ItemLoc};
pub use items::*;

/// Declarations of one file.
#[salsa::tracked(returns(ref))]
pub fn file_items(db: &dyn Db, file: SourceFile) -> Arc<FileItems> {
    let parse = stir_db::parse(db, file);
    let versions = if file.origin(db) == FileOrigin::Stub {
        items::StubVersions::for_stub(file.path(db), stir_db::php_versions(db))
    } else {
        items::StubVersions::default()
    };
    Arc::new(items::lower_file(&parse.syntax(), versions))
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
    // our own stubs replace the phpstorm-stubs declarations of a function
    for locs in index.functions.values_mut() {
        let overriding = |l: &ItemLoc| l.file.path(db).starts_with("stubs://override");
        if locs.iter().any(overriding) {
            locs.retain(|l| overriding(l) || l.file.origin(db) != stir_db::FileOrigin::Stub);
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
