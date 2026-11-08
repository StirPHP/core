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
    /// The names of the locals whose declaring range doesn't spell them: what a template's
    /// directives declare (`$loop`, by `@foreach`).
    pub local_names: FxHashMap<TextRange, SmolStr>,
    /// Semantic diagnostics (unfiltered).
    pub diags: Vec<Diagnostic>,
    /// What went wrong with the project's scripts while checking.
    pub script_failures: Vec<crate::ScriptFailure>,
}

/// Runs the checker on `file` and keeps its type information.
#[salsa::tracked(returns(ref))]
pub fn typed_file(db: &dyn Db, ws: Workspace, file: SourceFile) -> Arc<TypedFile> {
    let ctx = Ctx::new(db, ws);
    let check = || {
        let mut checker = Checker::new(&ctx, file);
        let root = stir_db::parse(db, file).syntax();
        checker.check_root(&root);
        let mut script_failures = checker.script_failures;
        script_failures.extend(ctx.take_script_failures());
        Arc::new(TypedFile {
            types: checker.types,
            defs: checker.defs,
            locals: checker.locals,
            local_names: checker.local_names,
            diags: checker.diags,
            script_failures,
        })
    };
    // hooks will run far down in it: this way they all fit on one stack, made once
    if ctx.scripts.has_return_hooks() || !ctx.scripts.magic.is_empty() {
        stir_script::with_stack(check)
    } else {
        check()
    }
}
