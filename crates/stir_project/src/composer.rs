//! Composer's files, from disk. What they say is `stir_composer`'s part.

pub use stir_composer::{Autoload, Composer, InstalledPackage, parse_installed};

use crate::config::Project;
use crate::error::{Error, Result};

/// Reads the project's Composer files. Without a `composer.json`, an installed `vendor/` still
/// says what's in it. `None` without either.
pub async fn load_composer(project: &Project) -> Result<Option<Composer>> {
    let manifest = project.composer_path().await;
    let root = project.root.clone();
    tokio::task::spawn_blocking(move || {
        let read = |path: &std::path::Path| std::fs::read_to_string(path).ok();
        match manifest {
            Some(path) if path.is_file() => Composer::load_with(&path, read)
                .map(Some)
                .map_err(|source| Error::Composer { path, source }),
            _ if root.join("vendor").is_dir() => {
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
    })
    .await?
}

/// Loads the project's Composer autoload rules, or empty rules if there is no `composer.json`.
pub async fn load_autoload(project: &Project) -> Result<Autoload> {
    Ok(load_composer(project)
        .await?
        .map(|c| c.autoload())
        .unwrap_or_default())
}
