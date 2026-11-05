//! High-level intermediate representation: declarations, name resolution and
//! the program-wide index, as salsa queries.

pub mod ast;
pub mod attrs;
pub mod doc;
mod index;
mod items;
pub mod name;
pub mod type_ref;

use std::sync::Arc;

use stir_db::{Db, SourceFile, Workspace};

pub use index::{DefIndex, ItemLoc};
pub use items::*;

/// Declarations of one file.
#[salsa::tracked(returns(ref))]
pub fn file_items(db: &dyn Db, file: SourceFile) -> Arc<FileItems> {
    let parse = stir_db::parse(db, file);
    Arc::new(items::lower_file(
        &parse.syntax(),
        stir_db::DEFAULT_PHP_VERSION,
    ))
}

/// Every declaration in the workspace.
#[salsa::tracked(returns(ref))]
pub fn def_index(db: &dyn Db, ws: Workspace) -> Arc<DefIndex> {
    let mut index = DefIndex::default();
    for &file in ws.files(db) {
        index.origins.insert(file, file.origin(db));
        index.ranks.insert(file, DefIndex::rank(file.origin(db)));
        let items = file_items(db, file);
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
    Arc::new(index)
}

#[cfg(test)]
mod tests;
