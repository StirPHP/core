//! The StirPHP type checker.
//!
//! Entry point: [`check_file`], a salsa query returning every diagnostic of a
//! file (syntax errors included).

use std::sync::Arc;

use stir_db::{Db, Diagnostic, SourceFile, Workspace};

/// All diagnostics of `file` within `ws`.
#[salsa::tracked(returns(ref))]
pub fn check_file(db: &dyn Db, ws: Workspace, file: SourceFile) -> Arc<Vec<Diagnostic>> {
    let _ = ws;
    stir_db::syntax_diagnostics(db, file).clone()
}
