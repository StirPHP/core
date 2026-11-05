//! What Composer's files say about a project. Composer itself is never run, so no PHP is needed
//! and no package's scripts run.
//!
//! Everything here works on the contents of `composer.json`, `composer.lock` and
//! `vendor/composer/installed.json`, with no file access: the caller supplies them through
//! [`Composer::load_with`], from a disk or from an editor's virtual file system.

mod autoload;
mod constraint;
mod project;

pub use autoload::{Autoload, Exclusions, normalize};
pub use constraint::lowest_php_version;
pub use project::{Composer, ComposerError, Framework, InstallState, Package};
pub use project::{InstalledPackage, parse_installed};
