use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};

/// Name of the project configuration file.
pub const CONFIG_FILE: &str = "sphp.toml";

/// Contents of `sphp.toml`. All keys are camelCase and optional.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Config {
    /// Source directory, relative to the project root.
    pub src_dir: PathBuf,
    /// Globs of files to include, relative to `srcDir`.
    pub include: Vec<String>,
    /// Globs of files to exclude, matched relative to both the project root and `srcDir`.
    pub exclude: Vec<String>,
    /// Path to a `phpstan.neon` file, relative to the project root.
    pub phpstan: Option<PathBuf>,
    /// Path to `composer.json`, relative to the project root. When unset,
    /// `composer.json` in the project root is used if it exists.
    pub composer: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            src_dir: PathBuf::from("."),
            include: vec!["**/*.php".into()],
            exclude: vec!["vendor/**".into()],
            phpstan: None,
            composer: None,
        }
    }
}

impl Config {
    /// Parses config from TOML text. `path` is only used for error messages.
    pub fn parse(text: &str, path: &Path) -> Result<Self> {
        toml::from_str(text).map_err(|source| Error::TomlParse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })
    }
}

/// A loaded project: its root directory (the one containing `sphp.toml`) and config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
    pub config: Config,
}

impl Project {
    /// Searches `start_dir` and its ancestors for `sphp.toml` and loads it.
    pub async fn load(start_dir: impl AsRef<Path>) -> Result<Self> {
        let start = start_dir.as_ref();
        let start = tokio::fs::canonicalize(start)
            .await
            .map_err(|e| Error::io(start, e))?;
        for dir in start.ancestors() {
            let candidate = dir.join(CONFIG_FILE);
            if tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
                return Self::load_exact(dir).await;
            }
        }
        Err(Error::ConfigNotFound { start })
    }

    /// Like [`Project::load`], but a project doesn't need a config file: without one the
    /// root is the nearest ancestor with a `composer.json`, else `start_dir` itself.
    pub async fn load_or_default(start_dir: impl AsRef<Path>) -> Result<Self> {
        let start = start_dir.as_ref();
        match Self::load(start).await {
            Err(Error::ConfigNotFound { start }) => {
                let root = start
                    .ancestors()
                    .find(|dir| dir.join("composer.json").is_file())
                    .unwrap_or(&start)
                    .to_path_buf();
                Ok(Self {
                    root,
                    config: Config::default(),
                })
            }
            other => other,
        }
    }

    /// Loads `sphp.toml` from exactly `root`, without searching upward.
    pub async fn load_exact(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let path = root.join(CONFIG_FILE);
        let text = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| Error::io(&path, e))?;
        let config = Config::parse(&text, &path)?;
        Ok(Self { root, config })
    }

    /// Path of this project's `sphp.toml`.
    pub fn config_path(&self) -> PathBuf {
        self.root.join(CONFIG_FILE)
    }

    /// Absolute source directory.
    pub fn src_dir(&self) -> PathBuf {
        self.root.join(&self.config.src_dir)
    }

    /// Absolute path of `phpstan.neon`, if configured.
    pub fn phpstan_path(&self) -> Option<PathBuf> {
        self.config.phpstan.as_ref().map(|p| self.root.join(p))
    }

    /// Absolute path of `composer.json`: the configured one, or `composer.json`
    /// in the project root if it exists.
    pub async fn composer_path(&self) -> Option<PathBuf> {
        match &self.config.composer {
            Some(p) => Some(self.root.join(p)),
            None => {
                let p = self.root.join("composer.json");
                tokio::fs::try_exists(&p)
                    .await
                    .unwrap_or(false)
                    .then_some(p)
            }
        }
    }
}
