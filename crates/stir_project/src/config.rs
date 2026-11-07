use std::path::{Path, PathBuf};
use std::sync::Arc;

use stir_vfs::{Disk, Fs};

use stir_neon::phpstan::{FormatIndent, LoadOptions, PhpstanConfig, PhpstanRun};

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
    /// How to run PHPStan next to our own analysis.
    pub phpstan: PhpstanRun,
    /// PHPStan's `tmpDir`, whose `.php` files are its caches and never the project's sources.
    pub phpstan_tmp_dir: Option<PathBuf>,
    /// Framework support turned on or off by name, over what's found in the project.
    pub frameworks: Vec<(String, bool)>,
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
            phpstan: PhpstanRun::default(),
            phpstan_tmp_dir: None,
            frameworks: Vec::new(),
        }
    }
}

impl Config {
    /// Reads `parameters.stirphp` out of the NEON file at `path` and what it includes. `root` is
    /// what `%currentWorkingDirectory%` stands for.
    pub fn load(path: &Path, root: &Path) -> Result<Self> {
        Self::load_from(&disk(), path, root)
    }

    /// [`Config::load`], reading from `fs`.
    pub fn load_from(fs: &Arc<dyn Fs>, path: &Path, root: &Path) -> Result<Self> {
        let opts = LoadOptions::new()
            .param("currentWorkingDirectory", root.to_string_lossy())
            .fs(fs.clone());
        let config = PhpstanConfig::load_with(path, &opts)?;
        let phpstan_tmp_dir = config
            .param("tmpDir")
            .and_then(|v| v.as_str())
            .map(PathBuf::from);
        let own = config.stirphp;
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
            phpstan: own.phpstan,
            phpstan_tmp_dir,
            frameworks: own.frameworks,
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
    config_in_fs(&Disk, dir)
}

/// [`config_in`], in `fs`.
pub fn config_in_fs(fs: &dyn Fs, dir: &Path) -> Option<PathBuf> {
    std::iter::once(CONFIG_FILE)
        .chain(PHPSTAN_CONFIG_FILES)
        .map(|name| dir.join(name))
        .find(|path| fs.is_file(path))
}

pub(crate) fn disk() -> Arc<dyn Fs> {
    Arc::new(Disk)
}

impl Project {
    /// The nearest project at or above `start`: a directory with a configuration, or else
    /// `composer.json`. A project doesn't need either: then it's `start` itself.
    #[cfg(feature = "tokio")]
    pub async fn load_or_default(start_dir: impl AsRef<Path>) -> Result<Self> {
        let start = start_dir.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || Self::find(&disk(), &start)).await?
    }

    /// [`Project::load_or_default`], in `fs`.
    pub fn find(fs: &Arc<dyn Fs>, start_dir: &Path) -> Result<Self> {
        let start = canonical(fs.as_ref(), start_dir)?;
        for dir in start.ancestors() {
            if let Some(file) = config_in_fs(fs.as_ref(), dir) {
                return Self::read(fs, dir.to_path_buf(), Some(file));
            }
            if fs.is_file(&dir.join("composer.json")) {
                return Self::read(fs, dir.to_path_buf(), None);
            }
        }
        Self::read(fs, start, None)
    }

    /// Like [`Project::load_or_default`], with `file` as the configuration wherever it is.
    #[cfg(feature = "tokio")]
    pub async fn load_with_config(start_dir: impl AsRef<Path>, file: &Path) -> Result<Self> {
        let start = start_dir.as_ref().to_path_buf();
        let file = file.to_path_buf();
        tokio::task::spawn_blocking(move || Self::find_with_config(&disk(), &start, &file)).await?
    }

    /// [`Project::load_with_config`], in `fs`.
    pub fn find_with_config(fs: &Arc<dyn Fs>, start_dir: &Path, file: &Path) -> Result<Self> {
        let start = canonical(fs.as_ref(), start_dir)?;
        let root = start
            .ancestors()
            .find(|dir| {
                config_in_fs(fs.as_ref(), dir).is_some() || fs.is_file(&dir.join("composer.json"))
            })
            .unwrap_or(&start)
            .to_path_buf();
        let file = canonical(fs.as_ref(), &start.join(file))?;
        Self::read(fs, root, Some(file))
    }

    fn read(fs: &Arc<dyn Fs>, root: PathBuf, config_file: Option<PathBuf>) -> Result<Self> {
        let Some(file) = config_file else {
            return Ok(Self {
                root,
                config: Config::default(),
                config_file: None,
            });
        };
        let config = Config::load_from(fs, &file, &root)?;
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
    #[cfg(feature = "tokio")]
    pub async fn composer_path(&self) -> Option<PathBuf> {
        self.composer_path_in(&Disk)
    }

    /// [`Project::composer_path`], in `fs`.
    pub fn composer_path_in(&self, fs: &dyn Fs) -> Option<PathBuf> {
        match &self.config.composer {
            Some(p) => Some(self.root.join(p)),
            None => {
                let p = self.root.join("composer.json");
                fs.exists(&p).then_some(p)
            }
        }
    }
}

fn canonical(fs: &dyn Fs, path: &Path) -> Result<PathBuf> {
    fs.canonicalize(path).map_err(|e| Error::io(path, e))
}
