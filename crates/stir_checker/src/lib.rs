//! The StirPHP type checker.
//!
//! Entry point: [`check_file`], a salsa query returning every diagnostic of a
//! file (syntax errors included).
//!
//! Project `.php` files are checked only when the workspace [`Settings`] enable
//! `check_php` or the file opts in with a leading `// @sphp-check` comment
//! (`// @sphp-nocheck` opts out); their diagnostics are filtered by the PHPStan
//! rule level and `ignoreErrors` (see [`levels`]).
//!
//! [`Settings`]: stir_db::Settings

#![allow(clippy::nonminimal_bool)]

use std::sync::Arc;

use stir_db::{Db, Diagnostic, FileOrigin, SourceFile, Workspace};
use stir_syntax::Dialect;

mod body;
pub mod codes;
mod infer;
pub mod levels;
mod model;
pub mod model_api;
mod typed;

pub use body::{Checker, is_by_ref_receiver};
pub use model::Ctx;
pub use typed::{TypedFile, typed_file};

/// All diagnostics of `file` within `ws`.
#[salsa::tracked(returns(ref))]
pub fn check_file(db: &dyn Db, ws: Workspace, file: SourceFile) -> Arc<Vec<Diagnostic>> {
    let php = file.dialect(db) == Dialect::Php;
    let settings = ws.settings(db);
    if php
        && file.origin(db) == FileOrigin::Source
        && !levels::php_checked(file.text(db), settings)
    {
        return Arc::new(Vec::new());
    }
    let mut diags = stir_db::syntax_diagnostics(db, file).as_ref().clone();
    // Semantic errors on a broken tree are mostly noise: fix syntax first.
    if file.origin(db) == FileOrigin::Source && diags.is_empty() {
        let ctx = Ctx::for_file(db, ws, file);
        let mut checker = Checker::new(&ctx, file);
        let root = stir_db::parse(db, file).syntax();
        checker.check_root(&root);
        diags.extend(checker.diags);
    }
    if php {
        levels::filter_php(&mut diags, settings);
    }
    diags.sort_by_key(|d| (d.range.start(), d.range.end()));
    diags.dedup();
    Arc::new(diags)
}
#[cfg(test)]
mod interop_tests;
