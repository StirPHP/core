//! Building the salsa database for a project: project sources, builtin stubs and
//! vendor PHP files reachable through Composer autoload rules.
//!
//! Shared by `sphp check` and the language server.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::{Autoload, Project};
use anyhow::Context as _;
use futures::{StreamExt as _, TryStreamExt as _};
use stir_db::{FileOrigin, IgnoreRule, RootDatabase, Settings, SourceFile, Workspace};
use stir_syntax::Dialect;

/// Maximum number of files read concurrently.
const READ_CONCURRENCY: usize = 64;

/// Options for [`load_workspace`].
#[derive(Debug, Clone, Copy)]
pub struct LoadOptions {
    /// Load vendor PHP files reachable through Composer autoload rules.
    pub vendor: bool,
    /// The rule level, over the one in the PHPStan config.
    pub level: Option<u8>,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            vendor: true,
            level: None,
        }
    }
}

/// A loaded workspace: the database, the workspace input and the project's own files.
pub struct Loaded {
    pub db: RootDatabase,
    pub workspace: Workspace,
    /// Project source files (origin [`FileOrigin::Source`]), sorted by path.
    pub sources: Vec<SourceFile>,
    pub stub_count: usize,
    pub vendor_count: usize,
}

/// Builds the database for `project`: discovered sources, all builtin stubs and
/// (unless disabled) vendor files from the root `composer.json` autoload rules.
pub async fn load_workspace(project: &Project, options: LoadOptions) -> anyhow::Result<Loaded> {
    let start = Instant::now();
    let discovered = crate::discover(project).await?;

    let source_texts: Vec<(PathBuf, String)> =
        futures::stream::iter(discovered.into_iter().map(|file| async move {
            let text = read_lossy(&file.path)
                .await
                .with_context(|| format!("failed to read `{}`", file.path.display()))?;
            anyhow::Ok((file.path, text))
        }))
        .buffered(READ_CONCURRENCY)
        .try_collect()
        .await?;

    let vendor_texts = if options.vendor {
        let autoload = crate::load_autoload(project).await?;
        let own: Vec<PathBuf> = source_texts.iter().map(|(p, _)| p.clone()).collect();
        let paths = vendor_paths(autoload, own).await?;
        read_vendor(paths).await
    } else {
        Vec::new()
    };

    let db = RootDatabase::default();
    let mut files = Vec::new();
    let mut sources = Vec::new();
    for (path, text) in source_texts {
        let file = SourceFile::new(&db, path, text, Dialect::Php, FileOrigin::Source);
        sources.push(file);
        files.push(file);
    }

    let builtins = builtin_files(&db);
    let stub_count = builtins
        .iter()
        .filter(|f| f.path(&db).to_string_lossy().starts_with("stubs://"))
        .count();
    files.extend(builtins);

    let vendor_count = vendor_texts.len();
    for (path, text) in vendor_texts {
        files.push(SourceFile::new(
            &db,
            path,
            text,
            Dialect::Php,
            FileOrigin::Vendor,
        ));
    }

    let config = load_phpstan_config(project).await?;
    if let Some(config) = &config {
        let own: BTreeSet<PathBuf> = sources
            .iter()
            .filter_map(|f| f.path(&db).canonicalize().ok())
            .collect();
        for (path, text) in read_vendor(scanned_paths(config, &own).await?).await {
            files.push(SourceFile::new(
                &db,
                path,
                text,
                Dialect::Php,
                FileOrigin::Vendor,
            ));
        }
    }
    let mut settings = settings_from(config);
    if let Some(level) = options.level {
        settings.php_level = level;
    }
    let workspace = Workspace::new(&db, files, settings);
    tracing::debug!(
        sources = sources.len(),
        stubs = stub_count,
        vendor = vendor_count,
        elapsed = ?start.elapsed(),
        "workspace loaded"
    );
    Ok(Loaded {
        db,
        workspace,
        sources,
        stub_count,
        vendor_count,
    })
}

/// The builtin PHP stubs (`stubs://...`).
pub fn builtin_files(db: &RootDatabase) -> Vec<SourceFile> {
    stir_stubs::stub_files()
        .map(|stub| {
            SourceFile::new(
                db,
                PathBuf::from(format!("stubs://{}", stub.path)),
                stub.text.to_owned(),
                Dialect::Php,
                FileOrigin::Stub,
            )
        })
        .collect()
}

/// Check settings: the rule level and `ignoreErrors` of the PHPStan config (the
/// `phpstan` key of `sphp.toml`, else
/// `phpstan.neon` or `phpstan.neon.dist` at the project root).
pub async fn load_settings(project: &Project) -> anyhow::Result<Settings> {
    Ok(settings_from(load_phpstan_config(project).await?))
}

async fn load_phpstan_config(
    project: &Project,
) -> anyhow::Result<Option<stir_neon::phpstan::PhpstanConfig>> {
    let path = match project.phpstan_path() {
        Some(p) => Some(p),
        None => ["phpstan.neon", "phpstan.neon.dist"]
            .iter()
            .map(|n| project.root.join(n))
            .find(|p| p.is_file()),
    };
    let Some(path) = path else {
        return Ok(None);
    };
    let root = project.root.clone();
    let config = tokio::task::spawn_blocking(move || {
        let opts = stir_neon::phpstan::LoadOptions::new()
            .param("currentWorkingDirectory", root.to_string_lossy());
        stir_neon::phpstan::PhpstanConfig::load_with(&path, &opts)
    })
    .await?
    .context("failed to load the PHPStan configuration")?;
    for skipped in &config.skipped_includes {
        tracing::warn!(
            "skipping PHPStan include {}: not installed, or a PHP file, which is never run",
            skipped.display()
        );
    }
    Ok(Some(config))
}

/// The files PHPStan reads for their symbols (`bootstrapFiles`, `scanFiles` and
/// `scanDirectories`), other than the project's own.
async fn scanned_paths(
    config: &stir_neon::phpstan::PhpstanConfig,
    own: &BTreeSet<PathBuf>,
) -> anyhow::Result<Vec<PathBuf>> {
    let files: Vec<PathBuf> = config
        .bootstrap_files
        .iter()
        .chain(&config.scan_files)
        .cloned()
        .collect();
    let dirs = config.scan_directories.clone();
    let own = own.clone();
    tokio::task::spawn_blocking(move || {
        let mut out = BTreeSet::new();
        for file in files.into_iter().filter(|f| f.is_file()) {
            out.insert(file);
        }
        for dir in dirs {
            for entry in ignore::WalkBuilder::new(dir)
                .standard_filters(false)
                .build()
                .flatten()
            {
                if entry.path().extension().is_some_and(|e| e == "php") {
                    out.insert(entry.into_path());
                }
            }
        }
        out.into_iter()
            .filter(|p| p.canonicalize().map_or(true, |c| !own.contains(&c)))
            .collect()
    })
    .await
    .context("failed to list the PHPStan scan paths")
}

fn settings_from(config: Option<stir_neon::phpstan::PhpstanConfig>) -> Settings {
    let mut settings = Settings::default();
    let Some(config) = config else {
        return settings;
    };
    settings.php_paths = config.paths.clone();
    settings.php_excludes = config
        .exclude_paths
        .analyse
        .iter()
        .chain(&config.exclude_paths.analyse_and_scan)
        .cloned()
        .collect();
    if let Some(level) = config.level {
        settings.php_level = match level {
            stir_neon::phpstan::Level::Level(n) => n.min(stir_db::MAX_PHP_LEVEL),
            stir_neon::phpstan::Level::Max => stir_db::MAX_PHP_LEVEL,
        };
    }
    settings.ignore_errors = config
        .ignore_errors
        .into_iter()
        .map(|e| {
            let mut rule = IgnoreRule {
                identifiers: e.identifiers,
                paths: e.paths,
                ..IgnoreRule::default()
            };
            for m in e.messages {
                match m {
                    stir_neon::phpstan::IgnoreMessage::Regex(r) => rule.patterns.push(r),
                    stir_neon::phpstan::IgnoreMessage::Raw(r) => rule.raw_messages.push(r),
                }
            }
            rule
        })
        .collect();
    settings
}

/// Collects every `.php` file reachable from the autoload rules, canonicalised and
/// deduplicated, excluding the project's own sources.
async fn vendor_paths(autoload: Autoload, own: Vec<PathBuf>) -> anyhow::Result<Vec<PathBuf>> {
    tokio::task::spawn_blocking(move || {
        let roots = autoload
            .psr4
            .into_iter()
            .map(|(_, dir)| dir)
            .chain(autoload.psr0.into_iter().map(|(_, dir)| dir))
            .chain(autoload.classmap_paths)
            .chain(autoload.files);
        let own: BTreeSet<PathBuf> = own
            .iter()
            .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
            .collect();

        let mut seen_dirs = BTreeSet::new();
        let mut found = BTreeSet::new();
        for root in roots {
            walk_php(&root, &mut seen_dirs, &mut found);
        }
        found.into_iter().filter(|p| !own.contains(p)).collect()
    })
    .await
    .map_err(Into::into)
}

/// Adds `root` (a `.php` file) or every `.php` file below `root` (a directory) to `found`.
/// Missing paths are ignored; directories are visited once (by canonical path).
fn walk_php(root: &Path, seen_dirs: &mut BTreeSet<PathBuf>, found: &mut BTreeSet<PathBuf>) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(path) = std::fs::canonicalize(&path) else {
            continue;
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.is_file() {
            if path.extension().is_some_and(|e| e == "php") {
                found.insert(path);
            }
            continue;
        }
        if !meta.is_dir() || !seen_dirs.insert(path.clone()) {
            continue;
        }
        match std::fs::read_dir(&path) {
            Ok(entries) => stack.extend(entries.flatten().map(|e| e.path())),
            Err(error) => tracing::warn!(path = %path.display(), %error, "cannot read directory"),
        }
    }
}

/// Reads a PHP file. PHP source isn't always UTF-8, so invalid bytes are replaced
/// rather than failing the whole project.
async fn read_lossy(path: &Path) -> std::io::Result<String> {
    let bytes = tokio::fs::read(path).await?;
    Ok(String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

/// Reads vendor files concurrently. Unreadable files are skipped with a warning.
async fn read_vendor(paths: Vec<PathBuf>) -> Vec<(PathBuf, String)> {
    futures::stream::iter(paths.into_iter().map(|path| async move {
        match read_lossy(&path).await {
            Ok(text) => Some((path, text)),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "cannot read vendor file");
                None
            }
        }
    }))
    .buffer_unordered(READ_CONCURRENCY)
    .filter_map(std::future::ready)
    .collect()
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[tokio::test]
    async fn loads_sources_stubs_and_vendor() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        write(&root.join("sphp.toml"), "srcDir = \"src\"\n");
        write(&root.join("src/main.php"), "<?php\n$x = 1;\n");
        write(&root.join("src/Legacy.php"), "<?php\nclass Legacy {}\n");
        write(
            &root.join("composer.json"),
            r#"{"autoload": {"psr-4": {"App\\": "src/"}}}"#,
        );
        write(
            &root.join("vendor/composer/installed.json"),
            r#"{"packages": [{"name": "acme/lib", "install-path": "../acme/lib",
                "autoload": {"psr-4": {"Acme\\": "src/"}, "files": ["helpers.php"]}}]}"#,
        );
        write(
            &root.join("vendor/acme/lib/src/Foo.php"),
            "<?php\nnamespace Acme;\nclass Foo {}\n",
        );
        write(
            &root.join("vendor/acme/lib/src/Sub/Bar.php"),
            "<?php\nnamespace Acme\\Sub;\nclass Bar {}\n",
        );
        write(&root.join("vendor/acme/lib/src/README.md"), "not php");
        write(
            &root.join("vendor/acme/lib/helpers.php"),
            "<?php\nfunction h() {}\n",
        );

        let project = Project::load(root).await.unwrap();
        let loaded = load_workspace(&project, LoadOptions::default())
            .await
            .unwrap();
        let db = &loaded.db;
        let files = loaded.workspace.files(db);
        let count = |origin| files.iter().filter(|f| f.origin(db) == origin).count();

        assert_eq!(loaded.sources.len(), 2);
        assert_eq!(count(FileOrigin::Source), 2);
        // `src/Legacy.php` is reachable through the root PSR-4 rule but is a project
        // source, so it is not loaded twice.
        assert_eq!(count(FileOrigin::Vendor), 3);
        assert_eq!(loaded.vendor_count, 3);
        assert_eq!(count(FileOrigin::Stub), loaded.stub_count);
        assert!(loaded.stub_count > 0);
        assert!(
            files
                .iter()
                .filter(|f| f.origin(db) == FileOrigin::Stub)
                .all(|f| f.path(db).to_string_lossy().starts_with("stubs://"))
        );

        let without = load_workspace(
            &project,
            LoadOptions {
                vendor: false,
                ..LoadOptions::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(without.vendor_count, 0);
    }

    #[tokio::test]
    async fn a_source_that_is_not_utf8_still_loads() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        write(&root.join("sphp.toml"), "srcDir = \"src\"\n");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/latin1.php"), b"<?php\n// caf\xe9\n").unwrap();

        let project = Project::load(root).await.unwrap();
        let loaded = load_workspace(
            &project,
            LoadOptions {
                vendor: false,
                ..LoadOptions::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(loaded.sources.len(), 1);
        assert!(loaded.sources[0].text(&loaded.db).contains("caf\u{fffd}"));
    }

    #[tokio::test]
    async fn reads_phpstan_settings() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        write(&root.join("sphp.toml"), "srcDir = \"src\"\n");
        write(
            &root.join("phpstan.neon.dist"),
            "parameters:\n    level: 5\n    ignoreErrors:\n        - '#^unknown function#'\n        -\n            identifier: SPHP0007\n            path: src/legacy.php\n",
        );
        let project = Project::load(root).await.unwrap();
        let settings = load_settings(&project).await.unwrap();
        assert_eq!(settings.php_level, 5);
        assert_eq!(settings.ignore_errors.len(), 2);
        assert_eq!(settings.ignore_errors[0].patterns, ["#^unknown function#"]);
        assert_eq!(settings.ignore_errors[1].identifiers, ["SPHP0007"]);
        assert!(settings.ignore_errors[1].paths[0].ends_with("src/legacy.php"));

        write(
            &root.join("sphp.toml"),
            "srcDir = \"src\"\nphpstan = \"conf/custom.neon\"\n",
        );
        write(
            &root.join("conf/custom.neon"),
            "parameters:\n    level: max\n",
        );
        let project = Project::load(root).await.unwrap();
        let settings = load_settings(&project).await.unwrap();
        assert_eq!(settings.php_level, stir_db::MAX_PHP_LEVEL);
    }

    #[tokio::test]
    async fn loads_phpstan_paths_and_scanned_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join("sphp.toml"), "srcDir = \"src\"\n");
        write(&root.join("src/a.php"), "<?php\necho BOOTED;\n");
        write(
            &root.join("tools/boot.php"),
            "<?php\ndefine('BOOTED', 1);\n",
        );
        write(
            &root.join("phpstan.neon"),
            "parameters:\n    paths:\n        - src\n    excludePaths:\n        analyse:\n            - src/gen\n    bootstrapFiles:\n        - tools/boot.php\n    scanFiles:\n        - src/a.php\n",
        );
        let project = Project::load(&root).await.unwrap();
        let loaded = load_workspace(
            &project,
            LoadOptions {
                vendor: false,
                ..LoadOptions::default()
            },
        )
        .await
        .unwrap();
        let settings = loaded.workspace.settings(&loaded.db);
        assert_eq!(settings.php_paths, [root.join("src")]);
        assert_eq!(settings.php_excludes, [root.join("src/gen")]);
        assert_eq!(loaded.sources.len(), 1);
        let paths: Vec<_> = loaded
            .workspace
            .files(&loaded.db)
            .iter()
            .map(|f| f.path(&loaded.db).clone())
            .collect();
        assert_eq!(
            paths
                .iter()
                .filter(|p| p.ends_with("tools/boot.php"))
                .count(),
            1
        );
        assert_eq!(paths.iter().filter(|p| p.ends_with("src/a.php")).count(), 1);
    }
}
