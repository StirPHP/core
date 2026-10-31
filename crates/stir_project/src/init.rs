use std::path::{Path, PathBuf};

use crate::config::{CONFIG_FILE, Config};
use crate::error::{Error, Result};

/// Sample program written to `<srcDir>/main.sphp` by [`init`].
pub const SAMPLE_MAIN: &str = r#"<?php

namespace App;

function greet(name: String, mut times: Int = 1): String {
    var $out = ""
    while (times > 0) {
        $out = $out . "Hello, ${name}! "
        times -= 1
    }
    return $out
}

echo greet("world")
"#;

/// Options for [`init`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOptions {
    /// Project name; defaults to the directory name.
    pub name: Option<String>,
    pub src_dir: PathBuf,
    pub out_dir: PathBuf,
    pub allow_php: bool,
    /// Overwrite an existing `sphp.toml`.
    pub force: bool,
}

impl Default for InitOptions {
    fn default() -> Self {
        let config = Config::default();
        Self {
            name: None,
            src_dir: config.src_dir,
            out_dir: config.out_dir,
            allow_php: config.allow_php,
            force: false,
        }
    }
}

/// What [`init`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitReport {
    /// Path of the written `sphp.toml`.
    pub config_path: PathBuf,
    /// The config that was written.
    pub config: Config,
    /// Files newly created (or overwritten, for `sphp.toml` with `force`).
    pub created: Vec<PathBuf>,
    /// Files that already existed and were left alone.
    pub skipped: Vec<PathBuf>,
    /// Whether `.gitignore` was created or appended to.
    pub gitignore_updated: bool,
    /// Detected `composer.json`, relative to the project root.
    pub composer: Option<PathBuf>,
    /// Detected `phpstan.neon` (or `phpstan.neon.dist`), relative to the project root.
    pub phpstan: Option<PathBuf>,
}

async fn exists(path: &Path) -> bool {
    tokio::fs::try_exists(path).await.unwrap_or(false)
}

/// Default project name for `dir`: its final path component.
pub fn default_name(dir: &Path) -> Option<String> {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    dir.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Initialises a StirPHP project in `dir`: writes `sphp.toml`, creates
/// `<srcDir>/main.sphp` (unless it exists) and adds `/<outDir>` to `.gitignore`.
pub async fn init(dir: impl AsRef<Path>, options: InitOptions) -> Result<InitReport> {
    let dir = dir.as_ref();
    let config_path = dir.join(CONFIG_FILE);
    if !options.force && exists(&config_path).await {
        return Err(Error::AlreadyInitialized { path: config_path });
    }

    let composer = exists(&dir.join("composer.json"))
        .await
        .then(|| PathBuf::from("composer.json"));
    let mut phpstan = None;
    for candidate in ["phpstan.neon", "phpstan.neon.dist"] {
        if exists(&dir.join(candidate)).await {
            phpstan = Some(PathBuf::from(candidate));
            break;
        }
    }

    let config = Config {
        name: options.name.or_else(|| default_name(dir)),
        src_dir: options.src_dir,
        out_dir: options.out_dir,
        allow_php: options.allow_php,
        composer: composer.clone(),
        phpstan: phpstan.clone(),
        ..Config::default()
    };

    let mut created = Vec::new();
    let mut skipped = Vec::new();

    let text = format!("# StirPHP project configuration\n\n{}", config.to_toml()?);
    tokio::fs::write(&config_path, text)
        .await
        .map_err(|e| Error::io(&config_path, e))?;
    created.push(config_path.clone());

    let src_dir = dir.join(&config.src_dir);
    tokio::fs::create_dir_all(&src_dir)
        .await
        .map_err(|e| Error::io(&src_dir, e))?;
    let main = src_dir.join("main.sphp");
    if exists(&main).await {
        skipped.push(main);
    } else {
        tokio::fs::write(&main, SAMPLE_MAIN)
            .await
            .map_err(|e| Error::io(&main, e))?;
        created.push(main);
    }

    let gitignore = dir.join(".gitignore");
    let gitignore_existed = exists(&gitignore).await;
    let gitignore_updated = add_to_gitignore(&gitignore, &config.out_dir).await?;
    if gitignore_updated && !gitignore_existed {
        created.push(gitignore);
    }

    Ok(InitReport {
        config_path,
        config,
        created,
        skipped,
        gitignore_updated,
        composer,
        phpstan,
    })
}

/// Appends `/<out_dir>` to the `.gitignore` at `path` (creating it) unless an
/// equivalent entry is already present. Returns whether the file changed.
async fn add_to_gitignore(path: &Path, out_dir: &Path) -> Result<bool> {
    let out = out_dir.to_string_lossy().replace('\\', "/");
    let out = out.trim_matches('/').trim_start_matches("./");
    let entry = format!("/{out}");

    let existing = match tokio::fs::read_to_string(path).await {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::io(path, e)),
    };
    let already = existing.lines().any(|line| {
        let line = line.trim();
        line.trim_start_matches('/').trim_end_matches('/') == out
    });
    if already {
        return Ok(false);
    }

    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&entry);
    text.push('\n');
    tokio::fs::write(path, text)
        .await
        .map_err(|e| Error::io(path, e))?;
    Ok(true)
}
