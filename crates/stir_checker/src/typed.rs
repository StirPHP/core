//! Type information about a checked file, for the language server.

use std::sync::Arc;

use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use stir_db::{Db, Diagnostic, SourceFile, Workspace};
use stir_syntax::TextRange;
use stir_types::Ty;

use crate::{Checker, Ctx};

/// What a name in the source resolves to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Def {
    /// A local binding (variable, parameter or loop binding) of the same
    /// file, identified by the range of its declaring name.
    Local { decl: TextRange },
    /// A function.
    Function(stir_hir::ItemLoc),
    /// A class, interface, trait or enum, by lowercase FQN.
    Class(SmolStr),
    /// A global constant.
    Const(stir_hir::ItemLoc),
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
}

/// What the checker learned about a file's expressions.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TypedFile {
    /// Type of every checked expression, by its range.
    pub types: FxHashMap<TextRange, Ty>,
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
    let ctx = Ctx::new(db, ws);
    let mut checker = Checker::new(&ctx, file);
    let root = stir_db::parse(db, file).syntax();
    checker.check_root(&root);
    Arc::new(TypedFile {
        types: checker.types,
        defs: checker.defs,
        locals: checker.locals,
        diags: checker.diags,
    })
}
