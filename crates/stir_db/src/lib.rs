//! The salsa database: file inputs, parsing, and the shared diagnostic type.
//!
//! Higher layers (`stir_hir`, `stir_checker`) add their own tracked queries
//! over the inputs defined here; they all run against any `dyn Db`.

use std::path::PathBuf;
use std::sync::Arc;

use stir_syntax::{Parse, TextRange};

mod version;

pub use version::{Availability, PhpVersion, PhpVersions};

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
/// settings they are checked with. A database holds one.
#[salsa::input(debug, singleton)]
pub struct Workspace {
    #[returns(ref)]
    pub files: Vec<SourceFile>,
    #[returns(ref)]
    pub settings: Settings,
}

/// The highest PHPStan rule level (`max`).
pub const MAX_PHP_LEVEL: u8 = 10;
/// The rule level without a configured one: argument types and everything below.
pub const DEFAULT_PHP_LEVEL: u8 = 5;

/// How a workspace is checked (from `sphp.toml` and `phpstan.neon`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Settings {
    /// PHPStan rule level (0..=10) files are checked at.
    pub php_level: u8,
    /// The PHP versions the project has to run on.
    pub php_version: PhpVersions,
    /// PHPStan `ignoreErrors`, applied to diagnostics.
    pub ignore_errors: Vec<IgnoreRule>,
    /// PHPStan `paths`: if any, only `.php` files under these are reported.
    pub php_paths: Vec<PathBuf>,
    /// PHPStan `excludePaths` (may contain fnmatch globs): `.php` files not reported.
    pub php_excludes: Vec<PathBuf>,
    /// Composer's packages aren't installed as declared, so a name they might declare isn't
    /// known to be wrong: the namespaces of the project's own code (lowercase), where it is.
    pub pending_install: Option<Vec<String>>,
    /// The `ext-*` that Composer's files mention (lowercase, no `ext-`). `None` without Composer.
    /// Editors don't offer extensions that aren't PHP's own unless they're in here.
    pub declared_extensions: Option<Vec<String>>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            php_level: DEFAULT_PHP_LEVEL,
            php_version: PhpVersions::default(),
            ignore_errors: Vec::new(),
            php_paths: Vec::new(),
            php_excludes: Vec::new(),
            pending_install: None,
            declared_extensions: None,
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

/// The PHP versions the stubs are read for. Only a stub's declarations depend on them, which is
/// why they aren't an argument of everything that reads a file's declarations.
pub fn php_versions(db: &dyn Db) -> PhpVersions {
    Workspace::try_get(db).map_or_else(PhpVersions::default, |ws| ws.settings(db).php_version)
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
    /// The name the project doesn't declare, on an error about an unknown symbol.
    pub symbol: Option<String>,
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
            symbol: None,
        }
    }

    /// Says which name this is an unknown-symbol error about.
    pub fn with_symbol(mut self, symbol: impl Into<String>) -> Self {
        self.symbol = Some(symbol.into());
        self
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
