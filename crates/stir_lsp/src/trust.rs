//! Whether a workspace's scripts may run. Opening a repository shouldn't run what's in it, so the
//! person is asked, and the answer is kept here, in the user's config directory, out of reach of
//! any repository: by workspace, for the scripts it was given about. When they change, it no
//! longer applies.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Where answers are kept.
#[derive(Debug, Clone)]
pub struct TrustStore {
    path: Option<PathBuf>,
}

impl TrustStore {
    /// The one in the user's config directory. Without one to be found, nothing is remembered.
    pub fn in_config_dir() -> Self {
        let dir = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
        TrustStore {
            path: dir.map(|d| d.join("stirphp").join("trusted-rules.json")),
        }
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        TrustStore {
            path: Some(path.into()),
        }
    }

    fn read(&self) -> BTreeMap<String, Value> {
        let text = self
            .path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok());
        // a file that can't be read knows nothing, which only means asking again
        text.and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// What was answered about these scripts in `workspace`, if that's been asked.
    pub fn decision(&self, workspace: &Path, hash: &str) -> Option<bool> {
        let all = self.read();
        let entry = all.get(workspace.to_string_lossy().as_ref())?;
        if entry.get("scripts")?.as_str()? != hash {
            return None;
        }
        entry.get("allow")?.as_bool()
    }

    /// Keeps an answer, in place of what was answered about that workspace before.
    pub fn remember(&self, workspace: &Path, hash: &str, allow: bool) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut all = self.read();
        all.insert(
            workspace.to_string_lossy().into_owned(),
            json!({ "scripts": hash, "allow": allow }),
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // nothing reading it should see half of it
        let partial = path.with_extension("json.tmp");
        std::fs::write(&partial, serde_json::to_vec_pretty(&all)?)?;
        std::fs::rename(&partial, path)
    }
}

/// What identifies a set of scripts: where they are in the workspace and what they say.
pub fn scripts_hash(workspace: &Path, scripts: &[(&Path, Result<&str, &str>)]) -> String {
    let mut hash = Sha256::new();
    for (path, text) in scripts {
        let path = path.strip_prefix(workspace).unwrap_or(path);
        hash.update(path.to_string_lossy().as_bytes());
        hash.update([0]);
        match text {
            Ok(text) => {
                hash.update([1]);
                hash.update(text.len().to_le_bytes());
                hash.update(text.as_bytes());
            }
            Err(_) => hash.update([0]),
        }
    }
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_by_workspace_and_scripts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = TrustStore::at(tmp.path().join("deep/er/trust.json"));
        let (a, b) = (Path::new("/a"), Path::new("/b"));
        assert_eq!(store.decision(a, "h1"), None);
        store.remember(a, "h1", true).unwrap();
        store.remember(b, "h2", false).unwrap();
        assert_eq!(store.decision(a, "h1"), Some(true));
        assert_eq!(store.decision(b, "h2"), Some(false));
        // other scripts, or another place: that wasn't asked
        assert_eq!(store.decision(a, "h2"), None);
        assert_eq!(store.decision(Path::new("/c"), "h1"), None);
        store.remember(a, "h3", false).unwrap();
        assert_eq!(store.decision(a, "h1"), None);
        assert_eq!(store.decision(a, "h3"), Some(false));
    }

    #[test]
    fn a_broken_file_knows_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("trust.json");
        std::fs::write(&path, "{ not json").unwrap();
        let store = TrustStore::at(&path);
        assert_eq!(store.decision(Path::new("/a"), "h"), None);
        store.remember(Path::new("/a"), "h", true).unwrap();
        assert_eq!(store.decision(Path::new("/a"), "h"), Some(true));
    }

    #[test]
    fn the_hash_follows_paths_and_texts() {
        let w = Path::new("/w");
        let (a, b) = (Path::new("/w/a.rhai"), Path::new("/w/b.rhai"));
        let one = scripts_hash(w, &[(a, Ok("x"))]);
        assert_eq!(one, scripts_hash(w, &[(a, Ok("x"))]));
        assert_ne!(one, scripts_hash(w, &[(a, Ok("y"))]));
        assert_ne!(one, scripts_hash(w, &[(b, Ok("x"))]));
        assert_ne!(one, scripts_hash(w, &[(a, Err("gone"))]));
        assert_ne!(one, scripts_hash(w, &[(a, Ok("x")), (b, Ok(""))]));
        // the same scripts when the project moves are a different answer in a different place,
        // not a different set of scripts
        assert_eq!(
            one,
            scripts_hash(Path::new("/v"), &[(Path::new("/v/a.rhai"), Ok("x"))])
        );
    }
}
