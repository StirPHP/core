//! Lowering of StirPHP syntax trees into the PHP output AST.

use stir_db::{Db, SourceFile, Workspace};

use crate::transpile::{EmitOptions, Emitted};

pub(crate) fn lower_file(db: &dyn Db, _ws: Workspace, file: SourceFile, _opts: &EmitOptions) -> Emitted {
    // Placeholder until the lowering lands: pass the source through.
    Emitted { code: file.text(db).clone(), mappings: Vec::new(), needs_runtime: false }
}

pub(crate) fn lower_runtime(_db: &dyn Db, _ws: Workspace, _opts: &EmitOptions) -> Emitted {
    Emitted { code: "<?php\n\ndeclare(strict_types=1);\n".into(), mappings: Vec::new(), needs_runtime: false }
}
