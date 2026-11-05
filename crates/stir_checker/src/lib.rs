//! The StirPHP type checker.
//!
//! Entry point: [`check_file`], a salsa query returning every diagnostic of a
//! file (syntax errors included).
//!
//! Project `.php` files are checked unless they opt out with a leading
//! `// @sphp-nocheck` comment; their diagnostics are filtered by the PHPStan
//! rule level and `ignoreErrors` (see [`levels`]).

#![allow(clippy::nonminimal_bool)]

use std::sync::Arc;

use stir_db::{Db, Diagnostic, FileOrigin, SourceFile, Workspace};

mod body;
pub mod codes;
mod infer;
pub mod levels;
mod model;
pub mod model_api;
mod typed;

pub use body::{Checker, LocalKind};
pub use infer::widen_literals;
pub use model::Ctx;
pub use typed::{Def, MemberKind, TypedFile, typed_file};

/// All diagnostics of `file` within `ws`.
#[salsa::tracked(returns(ref))]
pub fn check_file(db: &dyn Db, ws: Workspace, file: SourceFile) -> Arc<Vec<Diagnostic>> {
    let settings = ws.settings(db);
    if file.origin(db) == FileOrigin::Source && levels::is_nocheck(file.text(db)) {
        return Arc::new(Vec::new());
    }
    let mut diags = stir_db::syntax_diagnostics(db, file).as_ref().clone();
    // Semantic errors on a broken tree are mostly noise: fix syntax first.
    if file.origin(db) == FileOrigin::Source && diags.is_empty() {
        diags.extend(typed_file(db, ws, file).diags.iter().cloned());
    }
    levels::filter_php(&mut diags, settings);
    levels::filter_phpstan_ignores(&mut diags, file.text(db));
    diags.sort_by_key(|d| (d.range.start(), d.range.end()));
    diags.dedup();
    Arc::new(diags)
}

#[cfg(test)]
mod interop_tests;
