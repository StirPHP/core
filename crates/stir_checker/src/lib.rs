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
pub mod php_version;
mod typed;

pub use body::{Checker, LocalKind};
pub use infer::widen_literals;
pub use model::Ctx;
pub use typed::{Def, MemberKind, TypedFile, typed_file};

/// All diagnostics of `file` within `ws`.
#[salsa::tracked(returns(ref))]
pub fn check_file(db: &dyn Db, ws: Workspace, file: SourceFile) -> Arc<Vec<Diagnostic>> {
    let settings = ws.settings(db);
    if file.origin(db) == FileOrigin::Source
        && (levels::is_nocheck(file.text(db)) || !levels::is_analysed(file.path(db), settings))
    {
        return Arc::new(Vec::new());
    }
    let mut diags = stir_db::syntax_diagnostics(db, file).as_ref().clone();
    // Semantic errors on a broken tree are mostly noise: fix syntax first.
    if file.origin(db) == FileOrigin::Source && diags.is_empty() {
        diags.extend(
            php_version::syntax_version_diagnostics(db, ws, file)
                .iter()
                .cloned(),
        );
        diags.extend(typed_file(db, ws, file).diags.iter().cloned());
    }
    levels::filter_php(&mut diags, settings);
    levels::filter_phpstan_ignores(&mut diags, file.text(db));
    diags.sort_by_key(|d| (d.range.start(), d.range.end()));
    diags.dedup();
    Arc::new(diags)
}

/// How many errors about unknown names [`check_file`] leaves out of `file`, because Composer's
/// packages aren't installed as declared.
pub fn held_back_count(db: &dyn Db, ws: Workspace, file: SourceFile) -> usize {
    let settings = ws.settings(db);
    if settings.pending_install.is_none() || file.origin(db) != FileOrigin::Source {
        return 0;
    }
    if !stir_db::syntax_diagnostics(db, file).is_empty()
        || levels::is_nocheck(file.text(db))
        || !levels::is_analysed(file.path(db), settings)
    {
        return 0;
    }
    typed_file(db, ws, file)
        .diags
        .iter()
        .filter(|d| levels::is_held_back(d, settings) && levels::min_level(d) <= settings.level())
        .count()
}

#[cfg(test)]
mod interop_tests;
