//! The salsa database: file inputs, parsing, and the shared diagnostic type.
//!
//! Higher layers (`stir_hir`, `stir_checker`) add their own tracked queries
//! over the inputs defined here; they all run against any `dyn Db`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use smol_str::SmolStr;
use stir_syntax::{Parse, TextRange};

mod version;

pub use version::{Availability, PhpVersion, PhpVersions};

/// What StirPHP queries run against. Plain salsa, so that what's attached to the thread during a
/// query (see [`attached`]) can run queries too.
pub use salsa::Database as Db;

/// The concrete database used by the CLI (and tests).
#[salsa::db]
#[derive(Default, Clone)]
pub struct RootDatabase {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for RootDatabase {}

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

/// Where the stub files of a project are in a workspace: under a name that isn't a place on disk,
/// as their declarations aren't where a name is declared.
pub const OVERLAY_PREFIX: &str = "stubs://overlay/";

/// The path a project's stub file at `path` has in a workspace.
pub fn overlay_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{OVERLAY_PREFIX}{}", path.display()))
}

/// Is what's at `path` a stub file, laid over what's declared elsewhere (see the `stir_hir` crate)?
/// Our own ones are under `stubs://override`, and those of a project under [`OVERLAY_PREFIX`].
pub fn is_overlay(path: &Path) -> bool {
    path.starts_with("stubs://override") || path.starts_with("stubs://overlay")
}

/// Where our own scripts are in a workspace (`stir_stubs::frameworks`): a name that isn't a place
/// on disk, so no script of a project can be taken for one.
pub const BUNDLED_PREFIX: &str = "bundled://";

/// A project's script (`parameters.stirphp.rules`), or one of ours. An input of its own, so that
/// changing a script invalidates what ran it and nothing else.
#[salsa::input(debug)]
pub struct ScriptFile {
    #[returns(ref)]
    pub path: PathBuf,
    /// Its text, or why it can't be read.
    #[returns(ref)]
    pub text: Result<String, String>,
}

/// A file of a project that isn't PHP but says something about it, such as the container Symfony
/// compiles: read by what we ship for frameworks, through the scripts' API.
#[salsa::input(debug)]
pub struct DataFile {
    #[returns(ref)]
    pub path: PathBuf,
    #[returns(ref)]
    pub text: String,
}

/// Runs `f` with the database of the query this thread is in, if it's in one. For code called from
/// a query that can't be handed the database, a script's native functions: what `f` reads is tracked
/// as what the query depends on.
pub fn attached<R>(f: impl FnOnce(&dyn Db) -> R) -> Option<R> {
    salsa::with_attached_database(f)
}

/// The set of files that make up a program (sources, vendor and stubs), plus the
/// settings they are checked with. A database holds one.
#[salsa::input(debug, singleton)]
pub struct Workspace {
    #[returns(ref)]
    pub files: Vec<SourceFile>,
    #[returns(ref)]
    pub settings: Settings,
    /// The scripts that may run: none, unless whoever runs this chose them or trusts them.
    #[returns(ref)]
    #[default]
    pub scripts: Vec<ScriptFile>,
    /// Our own scripts for the frameworks the project uses (see `stir_stubs::frameworks`). They
    /// run without anyone's say-so, and after the project's, which can override what they say.
    #[returns(ref)]
    #[default]
    pub bundled_scripts: Vec<ScriptFile>,
    /// What the frameworks the project uses left for it to read: see [`DataFile`].
    #[returns(ref)]
    #[default]
    pub data_files: Vec<DataFile>,
}

/// The highest PHPStan rule level (`max`).
pub const MAX_PHP_LEVEL: u8 = 10;
/// The rule level without a configured one: argument types and everything below.
pub const DEFAULT_PHP_LEVEL: u8 = 5;

/// How a workspace is checked (from `stirphp.neon` or `phpstan.neon`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Settings {
    /// PHPStan rule level (0..=10) files are checked at.
    pub php_level: u8,
    /// The PHP versions the project has to run on.
    pub php_version: PhpVersions,
    /// PHPStan `ignoreErrors`, applied to diagnostics.
    pub ignore_errors: IgnoreRules,
    /// The entries of the file `parameters.stirphp.baseline` names: `ignoreErrors` too, and what
    /// `stirphp check` says about the ones that match nothing.
    pub baseline: IgnoreRules,
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
    /// Report the use of what's deprecated as warnings: `parameters.stirphp.deprecations`, or
    /// PHPStan's rules for it being installed. Editors mark it either way.
    pub deprecations: bool,
    /// PHPStan's `treatPhpDocTypesAsCertain`. Without it, what only PHPDoc says can't be taken to
    /// make a condition always go one way.
    pub treat_phpdoc_types_as_certain: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            php_level: DEFAULT_PHP_LEVEL,
            php_version: PhpVersions::default(),
            ignore_errors: IgnoreRules::default(),
            baseline: IgnoreRules::default(),
            php_paths: Vec::new(),
            php_excludes: Vec::new(),
            pending_install: None,
            declared_extensions: None,
            deprecations: false,
            treat_phpdoc_types_as_certain: true,
        }
    }
}

impl Settings {
    /// The effective level for `.php` files.
    pub fn level(&self) -> u8 {
        self.php_level.min(MAX_PHP_LEVEL)
    }
}

/// One rule hiding diagnostics, from an `ignoreErrors` entry. It never looks at their messages,
/// because those may be translated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IgnoreRule {
    /// Diagnostic codes (`identifier`); empty means every code.
    pub identifiers: Vec<String>,
    /// Absolute paths or fnmatch globs; empty means every file.
    pub paths: Vec<PathBuf>,
    /// How many diagnostics it hides per file; `None` means all of them.
    pub count: Option<u32>,
    /// `reportUnmatched`: should a baseline say when this matches nothing?
    pub report_unmatched: bool,
}

impl Default for IgnoreRule {
    fn default() -> Self {
        IgnoreRule {
            identifiers: Vec::new(),
            paths: Vec::new(),
            count: None,
            report_unmatched: true,
        }
    }
}

/// Rules, and which apply where: a baseline has thousands, nearly all for some other file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct IgnoreRules {
    rules: Vec<IgnoreRule>,
    /// Rules by a path without globs, which can be looked up.
    by_path: BTreeMap<PathBuf, Vec<u32>>,
    /// The other rules: those for every file, and those with globs.
    others: Vec<u32>,
}

impl IgnoreRules {
    pub fn new(rules: Vec<IgnoreRule>) -> Self {
        let mut by_path: BTreeMap<PathBuf, Vec<u32>> = BTreeMap::new();
        let mut others = Vec::new();
        for (i, rule) in rules.iter().enumerate() {
            let i = i as u32;
            let plain = |p: &PathBuf| !p.to_string_lossy().contains(['*', '?']);
            if rule.paths.is_empty() || !rule.paths.iter().all(plain) {
                others.push(i);
            } else {
                for path in &rule.paths {
                    by_path.entry(path.clone()).or_default().push(i);
                }
            }
        }
        IgnoreRules {
            rules,
            by_path,
            others,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn rules(&self) -> &[IgnoreRule] {
        &self.rules
    }

    /// The rules that apply to `file` with their indexes, in order. `matches` says whether a path
    /// of a rule covers it.
    pub fn for_file(
        &self,
        file: &Path,
        matches: &dyn Fn(&Path, &Path) -> bool,
    ) -> impl Iterator<Item = (u32, &IgnoreRule)> {
        // a source directory of `.` leaves `/./` in the file's path, which `components` drops
        let file: PathBuf = file.components().collect();
        let mut found: Vec<u32> = file
            .ancestors()
            .filter_map(|dir| self.by_path.get(dir))
            .flatten()
            .copied()
            .collect();
        found.extend(self.others.iter().copied().filter(|&i| {
            let paths = &self.rules[i as usize].paths;
            paths.is_empty() || paths.iter().any(|p| matches(p, &file))
        }));
        found.sort_unstable();
        found.dedup();
        found.into_iter().map(|i| (i, &self.rules[i as usize]))
    }
}

/// The PHP versions the stubs are read for. Only a stub's declarations depend on them, which is
/// why they aren't an argument of everything that reads a file's declarations.
pub fn php_versions(db: &dyn Db) -> PhpVersions {
    Workspace::try_get(db).map_or_else(PhpVersions::default, |ws| ws.settings(db).php_version)
}

#[salsa::tracked(returns(ref), no_eq)]
pub fn parse(db: &dyn Db, file: SourceFile) -> Parse {
    if stir_syntax::is_blade_path(file.path(db)) {
        stir_syntax::parse_blade(file.text(db))
    } else {
        stir_syntax::parse(file.text(db))
    }
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
    /// Stable code: PHPStan's identifier if it has one for this, else `SPHP00xx`.
    pub code: SmolStr,
    pub message: String,
    /// Secondary labelled ranges in the same file.
    pub labels: Vec<(TextRange, String)>,
    pub notes: Vec<String>,
    /// The PHPStan rule level this diagnostic belongs to when reported in a
    /// `.php` file; `None` derives it from the code (see the checker's `levels`).
    pub php_level: Option<u8>,
    /// The name the project doesn't declare, on an error about an unknown symbol.
    pub symbol: Option<String>,
    /// PHPStan reported this, and it's shown along with what's ours. `code` is empty if PHPStan
    /// gave no identifier.
    pub from_phpstan: bool,
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
            code: SmolStr::new_static(code),
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            php_level: None,
            symbol: None,
            from_phpstan: false,
        }
    }

    /// A diagnostic whose code isn't one of ours: the identifier a project's rule chose.
    pub fn custom(
        file: impl Into<PathBuf>,
        range: TextRange,
        severity: Severity,
        code: impl Into<SmolStr>,
        message: impl Into<String>,
    ) -> Self {
        let mut d = Diagnostic::error(file, range, "", message);
        d.code = code.into();
        d.severity = severity;
        d
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
    /// PHPStan's identifier for syntax errors.
    pub const SYNTAX: &str = "phpstan.parse";
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
