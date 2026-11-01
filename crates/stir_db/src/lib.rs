//! The salsa database: file inputs, parsing, and the shared diagnostic type.
//!
//! Higher layers (`stir_hir`, `stir_checker`) add their own tracked queries
//! over the inputs defined here; they all run against any `dyn Db`.

use std::path::PathBuf;
use std::sync::Arc;

use stir_syntax::{Dialect, Parse, TextRange};

/// Marker trait for databases that can run StirPHP queries.
#[salsa::db]
pub trait Db: salsa::Database {}

/// The concrete database used by the CLI (and tests).
#[salsa::db]
#[derive(Default, Clone)]
pub struct RootDatabase {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for RootDatabase {}

#[salsa::db]
impl Db for RootDatabase {}

/// Where a file comes from; affects which diagnostics are reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileOrigin {
    /// A project source file.
    Source,
    /// A dependency under `vendor/` (signatures only, never checked).
    Vendor,
    /// A builtin stub (signatures only).
    Stub,
}

#[salsa::input(debug)]
pub struct SourceFile {
    #[returns(ref)]
    pub path: PathBuf,
    #[returns(ref)]
    pub text: String,
    #[returns(copy)]
    pub dialect: Dialect,
    #[returns(copy)]
    pub origin: FileOrigin,
}

/// The set of files that make up a program (sources, vendor and stubs).
#[salsa::input(debug)]
pub struct Workspace {
    #[returns(ref)]
    pub files: Vec<SourceFile>,
}

#[salsa::tracked(returns(ref), no_eq)]
pub fn parse(db: &dyn Db, file: SourceFile) -> Parse {
    stir_syntax::parse(file.text(db), file.dialect(db))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Hint,
}

/// A diagnostic attached to a range of a file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub file: PathBuf,
    pub range: TextRange,
    pub severity: Severity,
    /// Stable code, e.g. `SPHP0001`.
    pub code: &'static str,
    pub message: String,
    /// Secondary labelled ranges in the same file.
    pub labels: Vec<(TextRange, String)>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn error(
        file: impl Into<PathBuf>,
        range: TextRange,
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Diagnostic {
            file: file.into(),
            range,
            severity: Severity::Error,
            code,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    pub fn with_label(mut self, range: TextRange, label: impl Into<String>) -> Self {
        self.labels.push((range, label.into()));
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

/// Syntax errors of a file as diagnostics.
#[salsa::tracked(returns(ref))]
pub fn syntax_diagnostics(db: &dyn Db, file: SourceFile) -> Arc<Vec<Diagnostic>> {
    let path = file.path(db).clone();
    Arc::new(
        parse(db, file)
            .errors()
            .iter()
            .map(|e| Diagnostic::error(path.clone(), e.range, codes::SYNTAX, e.message.clone()))
            .collect(),
    )
}

/// Stable diagnostic codes.
pub mod codes {
    pub const SYNTAX: &str = "SPHP0001";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_is_memoized_and_updates() {
        use salsa::Setter;
        let mut db = RootDatabase::default();
        let file = SourceFile::new(
            &db,
            "a.sphp".into(),
            "val x = 1\n".into(),
            Dialect::Stir,
            FileOrigin::Source,
        );
        assert!(parse(&db, file).errors().is_empty());
        file.set_text(&mut db).to("val = \n".into());
        assert!(!syntax_diagnostics(&db, file).is_empty());
        assert_eq!(syntax_diagnostics(&db, file)[0].code, codes::SYNTAX);
    }
}
