//! The salsa database: file inputs, parsing, and the shared diagnostic type.
//!
//! Higher layers (`stir_hir`, `stir_checker`) add their own tracked queries
//! over the inputs defined here; they all run against any `dyn Db`.

use std::path::PathBuf;
use std::sync::Arc;

use stir_syntax::{Parse, TextRange};

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
    pub origin: FileOrigin,
}

/// The set of files that make up a program (sources, vendor and stubs), plus the
/// settings they are checked with.
#[salsa::input(debug)]
pub struct Workspace {
    #[returns(ref)]
    pub files: Vec<SourceFile>,
    #[returns(ref)]
    pub settings: Settings,
}

/// The PHP version stubs are interpreted for when nothing else is configured.
pub const DEFAULT_PHP_VERSION: (u8, u8) = (8, 4);

/// The highest PHPStan rule level (`max`).
pub const MAX_PHP_LEVEL: u8 = 10;
/// The rule level without a configured one: argument types and everything below.
pub const DEFAULT_PHP_LEVEL: u8 = 5;

/// How a workspace is checked (from `sphp.toml` and `phpstan.neon`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Settings {
    /// PHPStan rule level (0..=10) `.php` files are checked at. `.sphp` files are
    /// always fully checked.
    pub php_level: u8,
    /// Target PHP version `(major, minor)`.
    pub php_version: (u8, u8),
    /// PHPStan `ignoreErrors`, applied to diagnostics in `.php` files only.
    pub ignore_errors: Vec<IgnoreRule>,
    /// PHPStan `paths`: if any, only `.php` files under these are reported.
    pub php_paths: Vec<PathBuf>,
    /// PHPStan `excludePaths` (may contain fnmatch globs): `.php` files not reported.
    pub php_excludes: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            php_level: DEFAULT_PHP_LEVEL,
            php_version: DEFAULT_PHP_VERSION,
            ignore_errors: Vec::new(),
            php_paths: Vec::new(),
            php_excludes: Vec::new(),
        }
    }
}

impl Settings {
    /// The effective level for `.php` files.
    pub fn level(&self) -> u8 {
        self.php_level.min(MAX_PHP_LEVEL)
    }
}

/// One `ignoreErrors` entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct IgnoreRule {
    /// PCRE-style patterns (`#^Foo$#i`) matched against the message.
    pub patterns: Vec<String>,
    /// Exact messages (`rawMessage`).
    pub raw_messages: Vec<String>,
    /// Diagnostic codes (`identifier`), e.g. `SPHP0005`.
    pub identifiers: Vec<String>,
    /// Absolute paths or fnmatch globs; empty means every file.
    pub paths: Vec<PathBuf>,
}

#[salsa::tracked(returns(ref), no_eq)]
pub fn parse(db: &dyn Db, file: SourceFile) -> Parse {
    stir_syntax::parse(file.text(db))
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
    /// The PHPStan rule level this diagnostic belongs to when reported in a
    /// `.php` file; `None` derives it from the code (see the checker's `levels`).
    pub php_level: Option<u8>,
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
            php_level: None,
        }
    }

    /// Sets the PHPStan rule level at which this is reported in `.php` files.
    pub fn with_php_level(mut self, level: u8) -> Self {
        self.php_level = Some(level);
        self
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
            "a.php".into(),
            "<?php $x = 1;\n".into(),
            FileOrigin::Source,
        );
        assert!(parse(&db, file).errors().is_empty());
        file.set_text(&mut db).to("<?php $x = ;\n".into());
        assert!(!syntax_diagnostics(&db, file).is_empty());
        assert_eq!(syntax_diagnostics(&db, file)[0].code, codes::SYNTAX);
    }
}
