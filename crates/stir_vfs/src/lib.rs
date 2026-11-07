//! Where a project's files are read from: [`Disk`], or [`Memory`] for an editor with no disk (the
//! browser), where the page hands over the files.
//!
//! Loading a project goes through [`Fs`] alone, so it's the same whichever it is.

use std::collections::BTreeMap;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// Which directories a walk skips.
pub type Prune = Arc<dyn Fn(&Path) -> bool + Send + Sync>;

/// What a path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File { len: u64 },
    Dir,
}

/// A file system, as far as loading a project needs one. Paths are absolute.
pub trait Fs: Send + Sync {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// What's at `path`, following links. `None` if there's nothing.
    fn kind(&self, path: &Path) -> Option<Kind>;

    /// The paths of what's in the directory `path`, in no particular order.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>>;

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf>;

    /// The files below `root` that make up a project, sorted by name within each directory:
    /// without hidden ones and what `.gitignore` and `.ignore` files leave out (with or without
    /// git), nor what's in directories `prune` says to skip (it isn't asked about `root`).
    fn project_files(&self, root: &Path, prune: Prune) -> Vec<Result<PathBuf, ignore::Error>>;

    /// Everything below `root`, directories too, with nothing left out.
    fn all_below(&self, root: &Path) -> Vec<PathBuf>;

    /// The files at `paths`, in their order. A disk reads them all at once.
    fn read_all(&self, paths: &[PathBuf]) -> Vec<io::Result<Vec<u8>>> {
        paths.iter().map(|p| self.read(p)).collect()
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        String::from_utf8(self.read(path)?)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn is_file(&self, path: &Path) -> bool {
        matches!(self.kind(path), Some(Kind::File { .. }))
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.kind(path) == Some(Kind::Dir)
    }

    fn exists(&self, path: &Path) -> bool {
        self.kind(path).is_some()
    }
}

/// PHP source isn't always UTF-8, so invalid bytes are replaced rather than failing.
pub fn lossy(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// The real file system.
#[derive(Debug, Clone, Copy, Default)]
pub struct Disk;

impl Fs for Disk {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn kind(&self, path: &Path) -> Option<Kind> {
        let meta = std::fs::metadata(path).ok()?;
        if meta.is_dir() {
            Some(Kind::Dir)
        } else if meta.is_file() {
            Some(Kind::File { len: meta.len() })
        } else {
            None
        }
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        Ok(std::fs::read_dir(path)?
            .flatten()
            .map(|e| path.join(e.file_name()))
            .collect())
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }

    fn project_files(&self, root: &Path, prune: Prune) -> Vec<Result<PathBuf, ignore::Error>> {
        let top = root.to_path_buf();
        ignore::WalkBuilder::new(root)
            .require_git(false)
            .sort_by_file_name(|a, b| a.cmp(b))
            .filter_entry(move |entry| {
                let path = entry.path();
                !entry.file_type().is_some_and(|t| t.is_dir()) || path == top || !prune(path)
            })
            .build()
            .filter_map(|entry| match entry {
                Ok(e) if e.file_type().is_some_and(|t| t.is_file()) => Some(Ok(e.into_path())),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    fn all_below(&self, root: &Path) -> Vec<PathBuf> {
        ignore::WalkBuilder::new(root)
            .standard_filters(false)
            .build()
            .flatten()
            .map(ignore::DirEntry::into_path)
            .collect()
    }

    #[cfg(not(target_family = "wasm"))]
    fn read_all(&self, paths: &[PathBuf]) -> Vec<io::Result<Vec<u8>>> {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};
        /// Files read at once.
        const THREADS: usize = 32;
        if paths.len() < 2 {
            return paths.iter().map(|p| self.read(p)).collect();
        }
        let next = AtomicUsize::new(0);
        let out: Mutex<Vec<Option<io::Result<Vec<u8>>>>> =
            Mutex::new(paths.iter().map(|_| None).collect());
        std::thread::scope(|scope| {
            for _ in 0..THREADS.min(paths.len()) {
                scope.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(path) = paths.get(i) else { break };
                        let read = std::fs::read(path);
                        out.lock().unwrap()[i] = Some(read);
                    }
                });
            }
        });
        out.into_inner()
            .unwrap()
            .into_iter()
            .map(|r| r.unwrap_or_else(|| Err(io::Error::other("not read"))))
            .collect()
    }
}

/// Files held in memory, by absolute path. Directories are what the paths of files go through.
#[derive(Debug, Clone, Default)]
pub struct Memory {
    files: BTreeMap<PathBuf, Arc<str>>,
}

impl Memory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces the file at `path`, which must be absolute.
    pub fn insert(&mut self, path: impl AsRef<Path>, text: impl Into<Arc<str>>) {
        self.files.insert(normalize(path.as_ref()), text.into());
    }

    /// Removes the file at `path`, or everything below it.
    pub fn remove(&mut self, path: &Path) {
        let path = normalize(path);
        if self.files.remove(&path).is_none() {
            self.files.retain(|p, _| !p.starts_with(&path));
        }
    }

    pub fn get(&self, path: &Path) -> Option<&Arc<str>> {
        self.files.get(&normalize(path))
    }

    pub fn files(&self) -> impl Iterator<Item = (&Path, &Arc<str>)> {
        self.files.iter().map(|(p, t)| (p.as_path(), t))
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The names directly in the directory `dir`, with whether each is a directory.
    fn entries(&self, dir: &Path) -> BTreeMap<PathBuf, bool> {
        let mut out = BTreeMap::new();
        for path in self.files.range(dir.to_path_buf()..).map(|(p, _)| p) {
            let Ok(rest) = path.strip_prefix(dir) else {
                break;
            };
            let mut parts = rest.components();
            let Some(first) = parts.next() else { continue };
            let is_dir = parts.next().is_some();
            *out.entry(dir.join(first)).or_insert(false) |= is_dir;
        }
        out
    }

    /// The rules of the ignore files in `dir`: `.ignore` over `.gitignore`.
    fn ignore_rules(&self, dir: &Path) -> Vec<ignore::gitignore::Gitignore> {
        [".ignore", ".gitignore"]
            .iter()
            .filter_map(|name| {
                let text = self.files.get(&dir.join(name))?;
                let mut builder = ignore::gitignore::GitignoreBuilder::new(dir);
                for line in text.lines() {
                    let _ = builder.add_line(None, line);
                }
                builder.build().ok()
            })
            .collect()
    }
}

/// `path` with `.` and `..` worked out, without looking at any file system.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn not_found(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("{} isn't there", path.display()),
    )
}

impl Fs for Memory {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.get(path)
            .map(|t| t.as_bytes().to_vec())
            .ok_or_else(|| not_found(path))
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.get(path)
            .map(|t| t.to_string())
            .ok_or_else(|| not_found(path))
    }

    fn kind(&self, path: &Path) -> Option<Kind> {
        let path = normalize(path);
        if let Some(text) = self.files.get(&path) {
            return Some(Kind::File {
                len: text.len() as u64,
            });
        }
        let below = self
            .files
            .range(path.clone()..)
            .next()
            .is_some_and(|(p, _)| p.starts_with(&path));
        below.then_some(Kind::Dir)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let path = normalize(path);
        if !self.is_dir(&path) {
            return Err(not_found(&path));
        }
        Ok(self.entries(&path).into_keys().collect())
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        let path = normalize(path);
        if !path.is_absolute() || !self.exists(&path) {
            return Err(not_found(&path));
        }
        Ok(path)
    }

    fn project_files(&self, root: &Path, prune: Prune) -> Vec<Result<PathBuf, ignore::Error>> {
        let root = normalize(root);
        let mut out = Vec::new();
        if !self.is_dir(&root) {
            if self.is_file(&root) {
                out.push(Ok(root));
            }
            return out;
        }
        // the ignore files of the directories above too, as a walk on a disk reads them
        let mut rules: Vec<Vec<ignore::gitignore::Gitignore>> = root
            .ancestors()
            .skip(1)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|dir| self.ignore_rules(dir))
            .collect();
        self.walk_project(&root, &root, &prune, &mut rules, &mut out);
        out
    }

    fn all_below(&self, root: &Path) -> Vec<PathBuf> {
        let root = normalize(root);
        let mut out = Vec::new();
        if !self.exists(&root) {
            return out;
        }
        out.push(root.clone());
        let mut dirs = vec![root];
        while let Some(dir) = dirs.pop() {
            for (path, is_dir) in self.entries(&dir) {
                if is_dir {
                    dirs.push(path.clone());
                }
                out.push(path);
            }
        }
        out
    }
}

impl Memory {
    fn walk_project(
        &self,
        root: &Path,
        dir: &Path,
        prune: &Prune,
        rules: &mut Vec<Vec<ignore::gitignore::Gitignore>>,
        out: &mut Vec<Result<PathBuf, ignore::Error>>,
    ) {
        rules.push(self.ignore_rules(dir));
        for (path, is_dir) in self.entries(dir) {
            let hidden = path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden || ignored(rules, &path, is_dir) {
                continue;
            }
            if is_dir {
                if path != root && prune(&path) {
                    continue;
                }
                self.walk_project(root, &path, prune, rules, out);
            } else {
                out.push(Ok(path));
            }
        }
        rules.pop();
    }
}

/// Do the ignore files from the root down to `path`'s directory leave it out? The nearest one
/// that says anything decides.
fn ignored(rules: &[Vec<ignore::gitignore::Gitignore>], path: &Path, is_dir: bool) -> bool {
    for level in rules.iter().rev() {
        for rule in level {
            if !path.starts_with(rule.path()) {
                continue;
            }
            let m = rule.matched(path, is_dir);
            if m.is_ignore() {
                return true;
            }
            if m.is_whitelist() {
                return false;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_walks_like_a_project_folder() {
        let mut fs = Memory::new();
        fs.insert("/p/.gitignore", "*.gen.php\n/cache/\n");
        fs.insert("/p/a.php", "");
        fs.insert("/p/b.gen.php", "");
        fs.insert("/p/cache/c.php", "");
        fs.insert("/p/.hidden/d.php", "");
        fs.insert("/p/lib/.gitignore", "!keep.gen.php\n");
        fs.insert("/p/lib/keep.gen.php", "");
        fs.insert("/p/lib/vendor/e.php", "");
        fs.insert("/p/lib/./../lib/z.php", "");
        let prune: Prune = Arc::new(|p: &Path| p.ends_with("vendor"));
        let files: Vec<PathBuf> = fs
            .project_files(Path::new("/p"), prune)
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(
            files,
            [
                PathBuf::from("/p/a.php"),
                PathBuf::from("/p/lib/keep.gen.php"),
                PathBuf::from("/p/lib/z.php"),
            ]
        );
        assert!(fs.is_dir(Path::new("/p/lib")));
        assert!(!fs.is_dir(Path::new("/p/li")));
        assert_eq!(
            fs.canonicalize(Path::new("/p/lib/../a.php")).unwrap(),
            Path::new("/p/a.php")
        );
        assert!(fs.canonicalize(Path::new("/p/nothing.php")).is_err());
        let mut listed = fs.read_dir(Path::new("/p/lib")).unwrap();
        listed.sort();
        assert_eq!(listed.len(), 4, "{listed:?}");
        fs.remove(Path::new("/p/lib"));
        assert!(!fs.exists(Path::new("/p/lib/z.php")));
    }
}
