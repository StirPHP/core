use std::path::{Path, PathBuf};

use stir_neon::phpstan::{FormatIndent, LoadOptions, PhpstanConfig};

use crate::error::{Error, Result};

/// Name of the project's own configuration file.
pub const CONFIG_FILE: &str = "stirphp.neon";
/// The names PHPStan finds its configuration under, which is read if there's no [`CONFIG_FILE`].
pub const PHPSTAN_CONFIG_FILES: [&str; 3] =
    ["phpstan.neon", "phpstan.neon.dist", "phpstan.dist.neon"];

/// The settings in `parameters.stirphp`. All are optional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Source directory: absolute, or relative to the project root.
    pub src_dir: PathBuf,
    /// Globs of files to include, relative to `srcDir`.
    pub include: Vec<String>,
    /// Globs of files to exclude, matched relative to both the project root and `srcDir`.
    pub exclude: Vec<String>,
    /// Path to `composer.json`. When unset, `composer.json` in the project root is used if it
    /// exists.
    pub composer: Option<PathBuf>,
    /// The file with our own baseline, which PHPStan doesn't read.
    pub baseline: Option<PathBuf>,
    /// The project's rules, as scripts.
    pub rules: Vec<PathBuf>,
    /// How `stirphp fmt` indents, if the project says.
    pub indent: Option<FormatIndent>,
    /// Report the use of what's deprecated in `stirphp check` too.
    pub deprecations: bool,
    /// Report what a function can throw and doesn't say it does.
    pub checked_exceptions: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            src_dir: PathBuf::from("."),
            include: vec!["**/*.php".into()],
            exclude: vec!["vendor/**".into()],
            composer: None,
            baseline: None,
            rules: Vec::new(),
            indent: None,
            deprecations: false,
            checked_exceptions: false,
        }
    }
}

impl Config {
    /// Reads `parameters.stirphp` out of the NEON file at `path` and what it includes. `root` is
    /// what `%currentWorkingDirectory%` stands for.
    pub fn load(path: &Path, root: &Path) -> Result<Self> {
        let opts = LoadOptions::new().param("currentWorkingDirectory", root.to_string_lossy());
        let own = PhpstanConfig::load_with(path, &opts)?.stirphp;
        let defaults = Self::default();
        Ok(Self {
            src_dir: own.src_dir.unwrap_or(defaults.src_dir),
            include: own.include.unwrap_or(defaults.include),
            exclude: own.exclude.unwrap_or(defaults.exclude),
            composer: own.composer,
            baseline: own.baseline,
            rules: own.rules,
            indent: own.indent,
            deprecations: own.deprecations,
            checked_exceptions: own.checked_exceptions,
        })
    }
}

/// A loaded project: its root directory, where its configuration comes from, and its settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
    pub config: Config,
    /// The NEON file the settings come from: PHPStan's parameters are read from it too. `None`
    /// means defaults.
    pub config_file: Option<PathBuf>,
}

/// The configuration in `dir`: `stirphp.neon` wins over PHPStan's, and can include that.
pub fn config_in(dir: &Path) -> Option<PathBuf> {
    std::iter::once(CONFIG_FILE)
        .chain(PHPSTAN_CONFIG_FILES)
        .map(|name| dir.join(name))
        .find(|path| path.is_file())
}

impl Project {
    /// The nearest project at or above `start`: a directory with a configuration, or else
    /// `composer.json`. A project doesn't need either: then it's `start` itself.
    pub async fn load_or_default(start_dir: impl AsRef<Path>) -> Result<Self> {
        let start = canonical(start_dir.as_ref()).await?;
        for dir in start.ancestors() {
            if let Some(file) = config_in(dir) {
                return Self::read(dir.to_path_buf(), Some(file)).await;
            }
            if dir.join("composer.json").is_file() {
                return Self::read(dir.to_path_buf(), None).await;
            }
        }
        Self::read(start, None).await
    }

    /// Like [`Project::load_or_default`], with `file` as the configuration wherever it is.
    pub async fn load_with_config(start_dir: impl AsRef<Path>, file: &Path) -> Result<Self> {
        let start = canonical(start_dir.as_ref()).await?;
        let root = start
            .ancestors()
            .find(|dir| config_in(dir).is_some() || dir.join("composer.json").is_file())
            .unwrap_or(&start)
            .to_path_buf();
        let file = canonical(&start.join(file)).await?;
        Self::read(root, Some(file)).await
    }

    async fn read(root: PathBuf, config_file: Option<PathBuf>) -> Result<Self> {
        let Some(file) = config_file else {
            return Ok(Self {
                root,
                config: Config::default(),
                config_file: None,
            });
        };
        let (config, root, file) = tokio::task::spawn_blocking(move || {
            Config::load(&file, &root).map(|config| (config, root, file))
        })
        .await??;
        Ok(Self {
            root,
            config,
            config_file: Some(file),
        })
    }

    /// Absolute source directory.
    pub fn src_dir(&self) -> PathBuf {
        stir_composer::normalize(&self.root.join(&self.config.src_dir))
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

async fn canonical(path: &Path) -> Result<PathBuf> {
    tokio::fs::canonicalize(path)
        .await
        .map_err(|e| Error::io(path, e))
}
