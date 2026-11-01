//! The StirPHP type checker.
//!
//! Entry point: [`check_file`], a salsa query returning every diagnostic of a
//! file (syntax errors included).

#![allow(clippy::nonminimal_bool)]

use std::sync::Arc;

use stir_db::{Db, Diagnostic, FileOrigin, SourceFile, Workspace};

mod body;
pub mod codes;
mod infer;
mod model;
mod typed;

pub use body::Checker;
pub use model::Ctx;
pub use typed::{TypedFile, typed_file};

/// All diagnostics of `file` within `ws`.
#[salsa::tracked(returns(ref))]
pub fn check_file(db: &dyn Db, ws: Workspace, file: SourceFile) -> Arc<Vec<Diagnostic>> {
    let mut diags = stir_db::syntax_diagnostics(db, file).as_ref().clone();
    // Semantic errors on a broken tree are mostly noise: fix syntax first.
    if file.origin(db) == FileOrigin::Source && diags.is_empty() {
        let ctx = Ctx::new(db, ws);
        let mut checker = Checker::new(&ctx, file);
        let root = stir_db::parse(db, file).syntax();
        checker.check_root(&root);
        diags.extend(checker.diags);
    }
    diags.sort_by_key(|d| (d.range.start(), d.range.end()));
    diags.dedup();
    Arc::new(diags)
}
