use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::config::Project;
use crate::error::{Error, Result};

/// A source file belonging to the project.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceFile {
    /// Absolute path (project root joined with the relative location).
    pub path: PathBuf,
}

/// Finds all project source files under `srcDir`, honouring `.gitignore` and
/// `include`/`exclude`. The result is sorted by path.
pub async fn discover(project: &Project) -> Result<Vec<SourceFile>> {
    let project = project.clone();
    tokio::task::spawn_blocking(move || discover_blocking(&project)).await?
}

/// The PHP files under `dir`, honouring `.gitignore`, without a `vendor/` folder's. Sorted.
pub fn php_files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = ignore::WalkBuilder::new(dir)
        .require_git(false)
        .filter_entry(|e| !(e.depth() > 0 && e.file_name() == "vendor"))
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(ignore::DirEntry::into_path)
        .filter(|p| stir_syntax::is_php_path(p))
        .collect();
    files.sort();
    files
}

fn build_set<'a>(patterns: impl IntoIterator<Item = &'a str>) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|source| Error::Glob {
                pattern: pattern.to_owned(),
                source,
            })?;
        builder.add(glob);
    }
    builder.build().map_err(|source| Error::Glob {
        pattern: String::new(),
        source,
    })
}

fn discover_blocking(project: &Project) -> Result<Vec<SourceFile>> {
    let config = &project.config;
    let root = project.root.clone();
    let src_dir = project.src_dir();

    let include = build_set(config.include.iter().map(String::as_str))?;
    let exclude = build_set(config.exclude.iter().map(String::as_str))?;
    // Patterns like `vendor/**` also exclude the directory itself, so the walk can prune it.
    let exclude_dirs = build_set(
        config
            .exclude
            .iter()
            .filter_map(|p| p.strip_suffix("/**"))
            .filter(|p| !p.is_empty()),
    )?;

    if !src_dir.is_dir() {
        return Ok(Vec::new());
    }

    let excluded = |path: &Path, set: &GlobSet| {
        path.strip_prefix(&src_dir)
            .is_ok_and(|rel| set.is_match(rel))
            || path.strip_prefix(&root).is_ok_and(|rel| set.is_match(rel))
    };

    let filter_exclude = exclude.clone();
    let filter_exclude_dirs = exclude_dirs.clone();
    let filter_src = src_dir.clone();
    let filter_project_root = root.clone();
    let walker = ignore::WalkBuilder::new(&src_dir)
        .require_git(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(move |entry| {
            let path = entry.path();
            if !entry.file_type().is_some_and(|t| t.is_dir()) || path == filter_src {
                return true;
            }
            let matches = |set: &GlobSet| {
                path.strip_prefix(&filter_src)
                    .is_ok_and(|rel| set.is_match(rel))
                    || path
                        .strip_prefix(&filter_project_root)
                        .is_ok_and(|rel| set.is_match(rel))
            };
            !(matches(&filter_exclude_dirs) || matches(&filter_exclude))
        })
        .build();

    let mut files = Vec::new();
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "php") {
            continue;
        }
        let Ok(rel) = path.strip_prefix(&src_dir) else {
            continue;
        };
        if !include.is_match(rel) || excluded(path, &exclude) {
            continue;
        }
        // PHPStan's caches are `.php` files, and the project may well have them in its folder
        if config
            .phpstan_tmp_dir
            .as_ref()
            .is_some_and(|dir| path.starts_with(dir))
        {
            continue;
        }
        files.push(SourceFile {
            path: path.to_path_buf(),
        });
    }
    files.sort();
    Ok(files)
}
