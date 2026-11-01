//! Building the salsa database for a project: project sources, builtin stubs and
//! vendor PHP files reachable through Composer autoload rules.
//!
//! Shared by `sphp check`, `sphp build` and the language server.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::{Autoload, FileKind, Project};
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
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self { vendor: true }
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

    let source_texts: Vec<(PathBuf, FileKind, String)> =
        futures::stream::iter(discovered.into_iter().map(|file| async move {
            let text = tokio::fs::read_to_string(&file.path)
                .await
                .with_context(|| format!("failed to read `{}`", file.path.display()))?;
            anyhow::Ok((file.path, file.kind, text))
        }))
        .buffered(READ_CONCURRENCY)
        .try_collect()
        .await?;

    let vendor_texts = if options.vendor {
        let autoload = crate::load_autoload(project).await?;
        let own: Vec<PathBuf> = source_texts.iter().map(|(p, ..)| p.clone()).collect();
        let paths = vendor_paths(autoload, own, project.out_dir()).await?;
        read_vendor(paths).await
    } else {
        Vec::new()
    };

    let db = RootDatabase::default();
    let mut files = Vec::new();
    let mut sources = Vec::new();
    for (path, kind, text) in source_texts {
        let dialect = match kind {
            FileKind::Sphp => Dialect::Stir,
            FileKind::Php => Dialect::Php,
        };
        let file = SourceFile::new(&db, path, text, dialect, FileOrigin::Source);
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

    let settings = load_settings(project).await?;
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

/// The builtin PHP stubs (`stubs://...`) and the StirPHP stdlib (`stdlib://...`).
pub fn builtin_files(db: &RootDatabase) -> Vec<SourceFile> {
    let stubs = stir_stubs::stub_files().map(|stub| {
        SourceFile::new(
            db,
            PathBuf::from(format!("stubs://{}", stub.path)),
            stub.text.to_owned(),
            Dialect::Php,
            FileOrigin::Stub,
        )
    });
    let stdlib = stir_stubs::stdlib_files().map(|lib| {
        SourceFile::new(
            db,
            PathBuf::from(format!("stdlib://{}", lib.path)),
            lib.text.to_owned(),
            Dialect::Stir,
            FileOrigin::Stub,
        )
    });
    stubs.chain(stdlib).collect()
}

/// Check settings: `checkPhp` from `sphp.toml`, plus the rule level and
/// `ignoreErrors` of the PHPStan config (the `phpstan` key of `sphp.toml`, else
/// `phpstan.neon` or `phpstan.neon.dist` at the project root).
pub async fn load_settings(project: &Project) -> anyhow::Result<Settings> {
    let mut settings = Settings {
        check_php: project.config.check_php,
        ..Settings::default()
    };
    let path = match project.phpstan_path() {
        Some(p) => Some(p),
        None => ["phpstan.neon", "phpstan.neon.dist"]
            .iter()
            .map(|n| project.root.join(n))
            .find(|p| p.is_file()),
    };
    let Some(path) = path else {
        return Ok(settings);
    };
    let root = project.root.clone();
    let config = tokio::task::spawn_blocking(move || {
        let opts = stir_neon::phpstan::LoadOptions::new()
            .param("currentWorkingDirectory", root.to_string_lossy());
        stir_neon::phpstan::PhpstanConfig::load_with(&path, &opts)
    })
    .await?
    .context("failed to load the PHPStan configuration")?;
    settings.php_level = config.level.map(|l| match l {
        stir_neon::phpstan::Level::Level(n) => n.min(stir_db::MAX_PHP_LEVEL),
        stir_neon::phpstan::Level::Max => stir_db::MAX_PHP_LEVEL,
    });
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
    Ok(settings)
}

/// Collects every `.php` file reachable from the autoload rules, canonicalised and
/// deduplicated, excluding the project's own sources and its output directory.
async fn vendor_paths(
    autoload: Autoload,
    own: Vec<PathBuf>,
    out_dir: PathBuf,
) -> anyhow::Result<Vec<PathBuf>> {
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
        let out_dir = std::fs::canonicalize(&out_dir).ok();

        let mut seen_dirs = BTreeSet::new();
        let mut found = BTreeSet::new();
        for root in roots {
            walk_php(&root, &mut seen_dirs, &mut found);
        }
        found
            .into_iter()
            .filter(|p| !own.contains(p))
            .filter(|p| out_dir.as_ref().is_none_or(|out| !p.starts_with(out)))
            .collect()
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

/// Reads vendor files concurrently. Unreadable files are skipped with a warning and
/// invalid UTF-8 is replaced.
async fn read_vendor(paths: Vec<PathBuf>) -> Vec<(PathBuf, String)> {
    futures::stream::iter(paths.into_iter().map(|path| async move {
        match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let text = String::from_utf8(bytes)
                    .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
                Some((path, text))
            }
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
        write(&root.join("src/main.sphp"), "val x = 1\n");
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
        assert_eq!(
            count(FileOrigin::Stub),
            loaded.stub_count + stir_stubs::stdlib_files().count()
        );
        assert!(loaded.stub_count > 0);
        assert!(
            files
                .iter()
                .filter(|f| f.origin(db) == FileOrigin::Stub)
                .all(|f| {
                    let p = f.path(db).to_string_lossy();
                    p.starts_with("stubs://") || p.starts_with("stdlib://")
                })
        );

        let without = load_workspace(&project, LoadOptions { vendor: false })
            .await
            .unwrap();
        assert_eq!(without.vendor_count, 0);
    }

    #[tokio::test]
    async fn reads_phpstan_settings() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        write(
            &root.join("sphp.toml"),
            "srcDir = \"src\"\ncheckPhp = true\n",
        );
        write(
            &root.join("phpstan.neon.dist"),
            "parameters:\n    level: 5\n    ignoreErrors:\n        - '#^unknown function#'\n        -\n            identifier: SPHP0007\n            path: src/legacy.php\n",
        );
        let project = Project::load(root).await.unwrap();
        let settings = load_settings(&project).await.unwrap();
        assert!(settings.check_php);
        assert_eq!(settings.php_level, Some(5));
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
        assert!(!settings.check_php);
        assert_eq!(settings.php_level, Some(stir_db::MAX_PHP_LEVEL));
    }
}
