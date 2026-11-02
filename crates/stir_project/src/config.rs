use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Name of the project configuration file.
pub const CONFIG_FILE: &str = "sphp.toml";

/// Contents of `sphp.toml`. All keys are camelCase and optional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Config {
    /// Project name (informational).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Source directory, relative to the project root.
    pub src_dir: PathBuf,
    /// Output directory for transpiled PHP, relative to the project root.
    pub out_dir: PathBuf,
    /// Globs of files to include, relative to `srcDir`.
    pub include: Vec<String>,
    /// Globs of files to exclude, matched relative to both the project root and `srcDir`.
    pub exclude: Vec<String>,
    /// Whether plain `.php` files are part of the project.
    pub allow_php: bool,
    /// Whether plain `.php` files are type-checked.
    pub check_php: bool,
    /// Strict mode.
    pub strict: bool,
    /// Inline extension function calls instead of emitting helper functions.
    pub inline_extensions: bool,
    /// Generate `declare(strict_types=1)`. Turn it off while porting code whose callers rely
    /// on PHP's scalar coercion.
    pub strict_types: bool,
    /// Copy the other files under `srcDir` to `outDir` too, so it's a complete tree.
    pub copy_assets: bool,
    /// Path to a `phpstan.neon` file, relative to the project root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phpstan: Option<PathBuf>,
    /// Path to `composer.json`, relative to the project root. When unset,
    /// `composer.json` in the project root is used if it exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub composer: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: None,
            src_dir: PathBuf::from("src"),
            out_dir: PathBuf::from("build"),
            include: vec!["**/*.sphp".into(), "**/*.php".into()],
            exclude: vec!["vendor/**".into()],
            allow_php: true,
            check_php: false,
            strict: true,
            inline_extensions: false,
            strict_types: true,
            copy_assets: false,
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

    /// Serialises the config to TOML.
    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string(self)?)
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

    /// Absolute output directory.
    pub fn out_dir(&self) -> PathBuf {
        self.root.join(&self.config.out_dir)
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
