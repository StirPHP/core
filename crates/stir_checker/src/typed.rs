//! Type information about a checked file, for the transpiler and the language server.

use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;
use stir_db::{Db, Diagnostic, SourceFile, Workspace};
use stir_syntax::TextRange;
use stir_types::Ty;

use crate::{Checker, Ctx};

/// What a name in the source resolves to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Def {
    /// A local binding (variable, parameter, loop or pattern binding) of the same
    /// file, identified by the range of its declaring name.
    Local { decl: TextRange },
    /// A function (including extension functions).
    Function(stir_hir::ItemLoc),
    /// A class, interface, trait, enum or record, by lowercase FQN.
    Class(SmolStr),
    /// A global constant.
    Const(stir_hir::ItemLoc),
    /// A `typealias`.
    TypeAlias(stir_hir::ItemLoc),
    /// A member of the class with lowercase FQN `class` (the declaring class).
    Member {
        class: SmolStr,
        kind: MemberKind,
        name: SmolStr,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemberKind {
    Property,
    Method,
    Const,
    Case,
    /// A record field.
    Field,
}

/// What the checker learned about a file's expressions.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
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
    pub function_calls: FxHashMap<TextRange, SmolStr>,
    /// Resolved names, by the range of the name.
    pub defs: FxHashMap<TextRange, Def>,
    /// Local bindings by declaring-name range: declared type and kind.
    pub locals: FxHashMap<TextRange, (Ty, crate::body::LocalKind)>,
    /// Semantic diagnostics (unfiltered).
    pub diags: Vec<Diagnostic>,
}

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
        defs: checker.defs,
        locals: checker.locals,
        diags: checker.diags,
    })
}
