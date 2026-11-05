//! `stirphp init`: writes the configuration, or adds to the PHPStan one a project already has.

use std::path::{Path, PathBuf};

use crate::config::{CONFIG_FILE, PHPSTAN_CONFIG_FILES};
use crate::error::{Error, Result};
use crate::neon_edit::with_stirphp_setting;

/// A new `stirphp.neon`, without a setting yet: PHPStan's own parameters go here too.
const NEW_CONFIG: &str =
    "# StirPHP's settings, and PHPStan's parameters such as `level` and `paths`.
# To share them with PHPStan, include its configuration:
#
# includes:
#     - phpstan.neon

";

/// What [`init`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Created(PathBuf),
    Updated(PathBuf),
    /// The file has the settings already.
    AlreadySetUp(PathBuf),
}

/// Sets up `root`. A minimal `stirphp.neon`, unless there's a PHPStan configuration or `phpstan`
/// asks for one: then `parameters.stirphp` and the `parametersSchema` for it go in that.
pub fn init(root: &Path, phpstan: bool) -> Result<Outcome> {
    let existing = PHPSTAN_CONFIG_FILES
        .iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file());
    let own = root.join(CONFIG_FILE);
    let (path, for_phpstan) = match existing {
        Some(path) if phpstan || !own.is_file() => (path, true),
        _ if phpstan => (root.join(PHPSTAN_CONFIG_FILES[0]), true),
        _ => (own, false),
    };
    let text = read_if_any(&path)?;
    let new = with_stirphp_setting(
        &path,
        text_or_new(&text, for_phpstan),
        "rules",
        "[]",
        for_phpstan,
    )?;
    if text.as_deref() == Some(new.as_str()) {
        return Ok(Outcome::AlreadySetUp(path));
    }
    write(&path, &new)?;
    Ok(if text.is_some() {
        Outcome::Updated(path)
    } else {
        Outcome::Created(path)
    })
}

/// Makes `config` (or a new `stirphp.neon` in `root`, without one) name `baseline`. Returns the
/// file, if that changed it.
pub fn reference_baseline(
    root: &Path,
    config: Option<&Path>,
    baseline: &Path,
) -> Result<Option<PathBuf>> {
    let path = config.map_or_else(|| root.join(CONFIG_FILE), Path::to_path_buf);
    let for_phpstan = path.file_name().is_none_or(|name| name != CONFIG_FILE);
    let text = read_if_any(&path)?;
    let dir = path.parent().unwrap_or(root);
    let relative = crate::baseline::relative_to(baseline, dir);
    let value = crate::baseline::neon_string(&relative);
    let new = with_stirphp_setting(
        &path,
        text_or_new(&text, for_phpstan),
        "baseline",
        &value,
        for_phpstan,
    )?;
    if text.as_deref() == Some(new.as_str()) {
        return Ok(None);
    }
    write(&path, &new)?;
    Ok(Some(path))
}

fn text_or_new(text: &Option<String>, for_phpstan: bool) -> &str {
    match text {
        Some(text) => text,
        None if for_phpstan => "",
        None => NEW_CONFIG,
    }
}

fn read_if_any(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io(path, e)),
    }
}

fn write(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text).map_err(|e| Error::io(path, e))
}
