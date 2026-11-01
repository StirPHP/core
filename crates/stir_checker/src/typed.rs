//! Type information about a checked file, for the transpiler.

use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use stir_db::{Db, SourceFile, Workspace};
use stir_syntax::TextRange;
use stir_types::Ty;

use crate::{Checker, Ctx};

/// What the checker learned about a file's expressions.
#[derive(Debug, Default, Clone)]
pub struct TypedFile {
    /// Type of every checked expression, by its range.
    pub types: FxHashMap<TextRange, Ty>,
    /// Names resolved as members of `$this` (`id` → `$this->id`).
    pub implicit_this: FxHashSet<TextRange>,
    /// Method calls resolved to extension functions.
    pub extension_calls: FxHashMap<TextRange, stir_hir::ItemLoc>,
    /// Types written in `is`/`as`/`when` patterns, by type-node range.
    pub pattern_types: FxHashMap<TextRange, Ty>,
    /// Function calls and the fully-qualified function they call.
    pub function_calls: FxHashMap<TextRange, smol_str::SmolStr>,
}

impl PartialEq for TypedFile {
    fn eq(&self, other: &Self) -> bool {
        self.types == other.types
            && self.implicit_this == other.implicit_this
            && self.extension_calls == other.extension_calls
            && self.pattern_types == other.pattern_types
            && self.function_calls == other.function_calls
    }
}

impl Eq for TypedFile {}

/// Runs the checker on `file` and keeps its type information.
#[salsa::tracked(returns(ref))]
pub fn typed_file(db: &dyn Db, ws: Workspace, file: SourceFile) -> Arc<TypedFile> {
    let ctx = Ctx::for_file(db, ws, file);
    let mut checker = Checker::new(&ctx, file);
    let root = stir_db::parse(db, file).syntax();
    checker.check_root(&root);
    Arc::new(TypedFile {
        types: checker.types,
        implicit_this: checker.implicit_this,
        extension_calls: checker.extension_calls,
        pattern_types: checker.pattern_types,
        function_calls: checker.function_calls,
    })
}
