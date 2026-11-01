//! The public transpiler API: StirPHP files → plain PHP.

use stir_db::{Db, SourceFile, Workspace};

use crate::print::Mapping;

/// Options for code generation.
#[derive(Debug, Clone)]
pub struct EmitOptions {
    /// Inline extension functions at call sites instead of calling them.
    pub inline_extensions: bool,
    /// `require_once` expression (PHP source) for the runtime file, relative to
    /// the emitted file, e.g. `__DIR__ . '/../stir-runtime.php'`. Emitted only
    /// when the file calls non-inlined stdlib/extension functions.
    pub runtime_require: Option<String>,
    pub print: crate::PrintOptions,
}

impl Default for EmitOptions {
    fn default() -> Self {
        EmitOptions {
            inline_extensions: false,
            runtime_require: None,
            print: crate::PrintOptions::default(),
        }
    }
}

/// The generated PHP of one file.
#[derive(Debug, Clone, PartialEq)]
pub struct Emitted {
    pub code: String,
    /// Output ranges mapped to byte ranges of the source file (`Origin::start/end`).
    pub mappings: Vec<Mapping>,
    /// The file calls functions that live in the runtime file.
    pub needs_runtime: bool,
}

/// Transpiles one `.sphp` file. The caller should only call this for files
/// that type-check (the result for ill-typed code is unspecified).
pub fn transpile(db: &dyn Db, ws: Workspace, file: SourceFile, opts: &EmitOptions) -> Emitted {
    crate::lower::lower_file(db, ws, file, opts)
}

/// The runtime file: the StirPHP stdlib (and nothing else) as plain PHP.
pub fn runtime(db: &dyn Db, ws: Workspace, opts: &EmitOptions) -> Emitted {
    crate::lower::lower_runtime(db, ws, opts)
}
