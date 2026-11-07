//! Which frameworks a project uses, for the support StirPHP ships for them
//! (`stir_stubs::frameworks`): found from Composer's files and a few telling files, unless
//! `parameters.stirphp.frameworks` says otherwise.

use std::path::{Path, PathBuf};

use stir_composer::Framework as Found;
use stir_db::{BUNDLED_PREFIX, FileOrigin, RootDatabase, ScriptFile, SourceFile};
use stir_stubs::frameworks::{FRAMEWORKS, Framework};

use crate::{Composer, Project};

/// The frameworks to support in `project`, in the order of [`FRAMEWORKS`]. `sources` are the
/// project's own files and their text.
pub fn frameworks_of(
    project: &Project,
    composer: Option<&Composer>,
    sources: &[(PathBuf, std::sync::Arc<str>)],
) -> Vec<&'static Framework> {
    let found = composer.map(Composer::frameworks).unwrap_or_default();
    let has = |f: Found| found.contains(&f);
    let uses = |name: &str| match name {
        "laravel" => has(Found::Laravel),
        // a project made of Symfony's components, but not one built on another framework
        "symfony" => {
            has(Found::Symfony)
                || composer.is_some_and(|c| c.requires("symfony/dependency-injection"))
                    && !has(Found::Drupal)
                    && !has(Found::Laravel)
        }
        "wordpress" => has(Found::WordPress) || is_wordpress(&project.root, sources),
        _ => false,
    };
    FRAMEWORKS
        .iter()
        .filter(|f| {
            let set = project
                .config
                .frameworks
                .iter()
                .rev()
                .find(|(n, _)| n == f.name);
            set.map_or_else(|| uses(f.name), |(_, on)| *on)
        })
        .collect()
}

/// WordPress itself, a site built on it, or a plugin or theme of one.
fn is_wordpress(root: &Path, sources: &[(PathBuf, std::sync::Arc<str>)]) -> bool {
    if root.join("wp-config.php").is_file()
        || root.join("wp-content").is_dir()
        || root.join("wp-includes/version.php").is_file()
    {
        return true;
    }
    // a plugin says so in the leading comment of a file in its folder
    sources.iter().any(|(path, text)| {
        path.parent() == Some(root) && {
            let head = text.get(..text.len().min(8 * 1024)).unwrap_or(text);
            head.contains("Plugin Name:")
        }
    }) || std::fs::read_to_string(root.join("style.css")).is_ok_and(|css| {
        css.get(..css.len().min(8 * 1024))
            .unwrap_or(&css)
            .contains("Theme Name:")
    })
}

/// What `frameworks` add to a workspace: our scripts for them, and our stub files.
pub fn bundled_inputs(
    db: &RootDatabase,
    frameworks: &[&'static Framework],
) -> (Vec<ScriptFile>, Vec<SourceFile>) {
    let scripts = frameworks
        .iter()
        .map(|f| {
            let path = PathBuf::from(format!("{BUNDLED_PREFIX}{}", f.script_name));
            ScriptFile::new(db, path, Ok(f.script.to_owned()))
        })
        .collect();
    let stubs = frameworks
        .iter()
        .flat_map(|f| f.stubs.iter())
        .map(|(path, text)| {
            let path = PathBuf::from(format!("stubs://{path}"));
            SourceFile::new(db, path, (*text).to_owned(), FileOrigin::Stub)
        })
        .collect();
    (scripts, stubs)
}

/// The compiled container of a Symfony project, as phpstan-symfony reads it: the file
/// `parameters.symfony.containerXmlPath` names, or else what `var/cache/` has, the `dev`
/// environment's first. Read only; nothing is warmed up or built.
pub fn container_xml(root: &Path, configured: Option<PathBuf>) -> Option<(PathBuf, String)> {
    /// The most of a container that's read.
    const MAX: u64 = 64 * 1024 * 1024;
    let path = configured.filter(|p| p.is_file()).or_else(|| {
        let cache = root.join("var/cache");
        let mut found: Vec<PathBuf> = std::fs::read_dir(&cache)
            .ok()?
            .flatten()
            .filter(|e| e.path().is_dir())
            .flat_map(|e| std::fs::read_dir(e.path()).into_iter().flatten().flatten())
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|e| e == "xml")
                    && p.file_stem()
                        .is_some_and(|s| s.to_string_lossy().ends_with("Container"))
            })
            .collect();
        // `dev` first, then by name, so the same project always gets the same one
        found.sort_by_key(|p| (!p.components().any(|c| c.as_os_str() == "dev"), p.clone()));
        found.into_iter().next()
    })?;
    if std::fs::metadata(&path).ok()?.len() > MAX {
        tracing::warn!(path = %path.display(), "the container is too large to read");
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    Some((path, text))
}
