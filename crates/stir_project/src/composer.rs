//! Composer autoload rules from `composer.json` and `vendor/composer/installed.json`.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::config::Project;
use crate::error::{Error, Result};

/// Merged autoload rules. All paths are absolute (or relative to whatever base the
/// `composer.json` path was given as) and lexically normalised.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Autoload {
    /// PSR-4 `(namespace prefix, directory)` pairs, e.g. `("App\\", "/proj/src")`.
    /// An empty prefix is a fallback directory.
    pub psr4: Vec<(String, PathBuf)>,
    /// PSR-0 `(namespace or class prefix, directory)` pairs.
    pub psr0: Vec<(String, PathBuf)>,
    /// Directories and files listed under `classmap`.
    pub classmap_paths: Vec<PathBuf>,
    /// Files that are always included (`files`).
    pub files: Vec<PathBuf>,
}

/// An installed Composer package read from `installed.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackage {
    pub name: String,
    /// Absolute install directory of the package.
    pub install_path: PathBuf,
}

impl Autoload {
    /// Loads the root package's `autoload` and `autoload-dev` rules from `composer_json`,
    /// plus the `autoload` rules of every package in `<vendor-dir>/composer/installed.json`
    /// (if that file exists).
    pub async fn load(composer_json: &Path) -> Result<Self> {
        let root = read_json(composer_json).await?;
        let base = composer_json
            .parent()
            .unwrap_or(Path::new(""))
            .to_path_buf();
        let mut autoload = Self::default();
        if let Some(rules) = root.get("autoload") {
            autoload.add_rules(rules, &base);
        }
        if let Some(rules) = root.get("autoload-dev") {
            autoload.add_rules(rules, &base);
        }

        let vendor_dir = root
            .pointer("/config/vendor-dir")
            .and_then(Value::as_str)
            .unwrap_or("vendor");
        let vendor = normalize(&base.join(vendor_dir));
        let installed = vendor.join("composer").join("installed.json");
        if tokio::fs::try_exists(&installed).await.unwrap_or(false) {
            let value = read_json(&installed).await?;
            for (package, rules) in parse_installed(&value, &vendor) {
                if let Some(rules) = rules {
                    autoload.add_rules(rules, &package.install_path);
                }
            }
        }
        Ok(autoload)
    }

    /// Adds the rules of one `autoload`-shaped JSON object, resolving paths against `base`.
    /// Malformed or unknown entries are ignored.
    pub fn add_rules(&mut self, rules: &Value, base: &Path) {
        let Some(rules) = rules.as_object() else {
            return;
        };
        let dirs = |v: &Value| -> Vec<PathBuf> {
            match v {
                Value::String(s) => vec![normalize(&base.join(s))],
                Value::Array(items) => items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|s| normalize(&base.join(s)))
                    .collect(),
                _ => Vec::new(),
            }
        };
        if let Some(map) = rules.get("psr-4").and_then(Value::as_object) {
            for (prefix, v) in map {
                self.psr4
                    .extend(dirs(v).into_iter().map(|d| (prefix.clone(), d)));
            }
        }
        if let Some(map) = rules.get("psr-0").and_then(Value::as_object) {
            for (prefix, v) in map {
                self.psr0
                    .extend(dirs(v).into_iter().map(|d| (prefix.clone(), d)));
            }
        }
        if let Some(v) = rules.get("classmap") {
            self.classmap_paths.extend(dirs(v));
        }
        if let Some(v) = rules.get("files") {
            self.files.extend(dirs(v));
        }
    }

    /// Candidate files that may define `fqcn` according to the PSR-4 rules (most specific
    /// prefix first), followed by PSR-0 candidates. A leading `\` is ignored. Candidates are
    /// not checked for existence.
    pub fn resolve_class(&self, fqcn: &str) -> Vec<PathBuf> {
        let fqcn = fqcn.trim_start_matches('\\');
        let mut out = Vec::new();

        let mut psr4: Vec<&(String, PathBuf)> = self
            .psr4
            .iter()
            .filter(|(prefix, _)| fqcn.starts_with(prefix.as_str()))
            .collect();
        psr4.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
        for (prefix, dir) in psr4 {
            let rest = &fqcn[prefix.len()..];
            if rest.is_empty() {
                continue;
            }
            out.push(dir.join(format!("{}.php", rest.replace('\\', "/"))));
        }

        let mut psr0: Vec<&(String, PathBuf)> = self
            .psr0
            .iter()
            .filter(|(prefix, _)| fqcn.starts_with(prefix.as_str()))
            .collect();
        psr0.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
        for (_, dir) in psr0 {
            let (ns, class) = match fqcn.rfind('\\') {
                Some(i) => (&fqcn[..=i], &fqcn[i + 1..]),
                None => ("", fqcn),
            };
            let rel = format!("{}{}.php", ns.replace('\\', "/"), class.replace('_', "/"));
            out.push(dir.join(rel));
        }
        out
    }
}

/// Parses `installed.json` in either the Composer 2 format (`{"packages": [...]}`) or the
/// legacy plain-array format. Each package's install path is `install-path` resolved against
/// `<vendor>/composer` when present, otherwise `<vendor>/<name>`. Returns each package with
/// its raw `autoload` value, if any.
pub fn parse_installed<'a>(
    value: &'a Value,
    vendor: &Path,
) -> Vec<(InstalledPackage, Option<&'a Value>)> {
    let packages = match value {
        Value::Array(items) => items.as_slice(),
        Value::Object(obj) => obj
            .get("packages")
            .and_then(Value::as_array)
            .map_or(&[][..], |v| v),
        _ => &[],
    };
    packages
        .iter()
        .filter_map(|pkg| {
            let name = pkg.get("name")?.as_str()?.to_owned();
            let install_path = match pkg.get("install-path").and_then(Value::as_str) {
                Some(p) => normalize(&vendor.join("composer").join(p)),
                None => normalize(&vendor.join(&name)),
            };
            Some((InstalledPackage { name, install_path }, pkg.get("autoload")))
        })
        .collect()
}

/// Loads the project's Composer autoload rules, or empty rules if there is no `composer.json`.
pub async fn load_autoload(project: &Project) -> Result<Autoload> {
    match project.composer_path().await {
        Some(path) if tokio::fs::try_exists(&path).await.unwrap_or(false) => {
            Autoload::load(&path).await
        }
        _ => Ok(Autoload::default()),
    }
}

async fn read_json(path: &Path) -> Result<Value> {
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| Error::io(path, e))?;
    serde_json::from_str(&text).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })
}

/// Lexically removes `.` and resolves `..` components without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !matches!(
                    out.components().next_back(),
                    Some(Component::RootDir | Component::Prefix(_))
                ) {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_resolves_parent_dirs() {
        assert_eq!(
            normalize(Path::new("/a/vendor/composer/../psr/log")),
            Path::new("/a/vendor/psr/log")
        );
        assert_eq!(normalize(Path::new("./src/./x")), Path::new("src/x"));
        assert_eq!(normalize(Path::new("../x")), Path::new("../x"));
    }
}
