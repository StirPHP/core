//! Composer's autoload rules.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

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
    /// `exclude-from-classmap`, as regular expressions over a whole path.
    exclusions: Vec<String>,
}

/// The paths `exclude-from-classmap` leaves out.
#[derive(Debug, Clone)]
pub struct Exclusions(regex::Regex);

impl Exclusions {
    pub fn contains(&self, path: &Path) -> bool {
        self.0.is_match(&path.to_string_lossy())
    }
}

impl Autoload {
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
        if let Some(items) = rules.get("exclude-from-classmap").and_then(Value::as_array) {
            self.exclusions.extend(
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|p| exclusion_pattern(p, base)),
            );
        }
    }

    /// Adds everything in `other`.
    pub fn merge(&mut self, other: Autoload) {
        self.psr4.extend(other.psr4);
        self.psr0.extend(other.psr0);
        self.classmap_paths.extend(other.classmap_paths);
        self.files.extend(other.files);
        self.exclusions.extend(other.exclusions);
    }

    /// What `exclude-from-classmap` leaves out.
    pub fn exclusions(&self) -> Option<Exclusions> {
        if self.exclusions.is_empty() {
            return None;
        }
        regex::Regex::new(&self.exclusions.join("|"))
            .ok()
            .map(Exclusions)
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

    /// The class autoloading would look for in `file`, by PSR-4, if the path can name one.
    pub fn class_of_file(&self, file: &Path) -> Option<String> {
        self.psr4.iter().find_map(|(prefix, dir)| {
            let rel = file.strip_prefix(dir).ok()?.with_extension("");
            let mut class = prefix.clone();
            let mut first = true;
            for part in rel.components() {
                let part = part.as_os_str().to_str()?;
                let valid = part
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || !c.is_ascii());
                if !valid || part.is_empty() {
                    return None;
                }
                if !first {
                    class.push('\\');
                }
                class.push_str(part);
                first = false;
            }
            Some(class)
        })
    }
}

/// Composer's reading of an `exclude-from-classmap` entry: relative to the package, `**` for
/// anything, `*` for anything within one directory, matching whole path components.
fn exclusion_pattern(path: &str, base: &Path) -> String {
    let unslashed = path.replace('\\', "/");
    let trimmed = unslashed.trim_matches('/');
    let mut pattern = regex::escape(&base.to_string_lossy());
    pattern.insert(0, '^');
    pattern.push('/');
    let escaped = regex::escape(trimmed);
    pattern.push_str(&escaped.replace(r"\*\*", ".+?").replace(r"\*", "[^/]+?"));
    pattern.push_str("($|/)");
    pattern
}

/// Lexically removes `.` and resolves `..` components without touching the filesystem.
pub fn normalize(path: &Path) -> PathBuf {
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
    use serde_json::json;

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

    #[test]
    fn exclude_from_classmap_is_read_as_composer_reads_it() {
        let mut a = Autoload::default();
        a.add_rules(
            &json!({"exclude-from-classmap": ["/Tests/", "**/fixtures/", "src/data/", "x/*/y.php"]}),
            Path::new("/p/lib"),
        );
        let re = a.exclusions().unwrap();
        for hit in [
            "/p/lib/Tests",
            "/p/lib/Tests/A.php",
            "/p/lib/src/a/fixtures/b.php",
            "/p/lib/src/data/c.php",
            "/p/lib/x/q/y.php",
        ] {
            assert!(re.contains(Path::new(hit)), "{hit}");
        }
        for miss in [
            "/p/lib/TestsX/A.php",
            "/p/lib/src/Tests/A.php",
            "/p/lib/fixtures/b.php",
            "/q/lib/Tests/A.php",
            "/p/lib/x/q/r/y.php",
        ] {
            assert!(!re.contains(Path::new(miss)), "{miss}");
        }
    }

    #[test]
    fn a_file_names_the_class_autoloading_finds_there() {
        let mut a = Autoload::default();
        a.add_rules(&json!({"psr-4": {"App\\": "src/"}}), Path::new("/p"));
        assert_eq!(
            a.class_of_file(Path::new("/p/src/A/B.php")).as_deref(),
            Some("App\\A\\B")
        );
        assert_eq!(a.class_of_file(Path::new("/p/src/data/1-2.php")), None);
        assert_eq!(a.class_of_file(Path::new("/q/A.php")), None);
    }
}
