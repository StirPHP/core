//! Composer's files, from disk or another [`Fs`]. What they say is `stir_composer`'s part.

pub use stir_composer::{Autoload, Composer, InstalledPackage, parse_installed};
use stir_vfs::Fs;

use crate::config::Project;
use crate::error::{Error, Result};

/// Reads the project's Composer files. Without a `composer.json`, an installed `vendor/` still
/// says what's in it. `None` without either.
#[cfg(feature = "tokio")]
pub async fn load_composer(project: &Project) -> Result<Option<Composer>> {
    let project = project.clone();
    tokio::task::spawn_blocking(move || load_composer_in(&stir_vfs::Disk, &project)).await?
}

/// [`load_composer`], from `fs`.
pub fn load_composer_in(fs: &dyn Fs, project: &Project) -> Result<Option<Composer>> {
    let manifest = project.composer_path_in(fs);
    let root = &project.root;
    let read = |path: &std::path::Path| fs.read_to_string(path).ok();
    match manifest {
        Some(path) if fs.is_file(&path) => Composer::load_with(&path, read)
            .map(Some)
            .map_err(|source| Error::Composer { path, source }),
        _ if fs.is_dir(&root.join("vendor")) => {
            let path = root.join("composer.json");
            let read = |p: &std::path::Path| {
                if p == path {
                    Some("{}".to_owned())
                } else {
                    read(p)
                }
            };
            Composer::load_with(&path, read)
                .map(Some)
                .map_err(|source| Error::Composer { path, source })
        }
        _ => Ok(None),
    }
}

/// Loads the project's Composer autoload rules, or empty rules if there is no `composer.json`.
#[cfg(feature = "tokio")]
pub async fn load_autoload(project: &Project) -> Result<Autoload> {
    Ok(load_composer(project)
        .await?
        .map(|c| c.autoload())
        .unwrap_or_default())
}
