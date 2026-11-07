//! StirPHP project tooling: `stirphp.neon` loading, source file discovery,
//! Composer autoload rules. Everything is read through an [`Fs`]: the disk, or (in the browser)
//! files held in memory.

pub mod baseline;
mod composer;
mod config;
mod discover;
mod error;
pub mod frameworks;
pub mod init;
mod neon_edit;
pub mod workspace;

pub use composer::{Autoload, Composer, InstalledPackage, load_composer_in, parse_installed};
#[cfg(feature = "tokio")]
pub use composer::{load_autoload, load_composer};
pub use config::{CONFIG_FILE, Config, PHPSTAN_CONFIG_FILES, Project, config_in, config_in_fs};
#[cfg(feature = "tokio")]
pub use discover::discover;
pub use discover::{SourceFile, discover_in, php_files_under};
pub use error::{Error, Result};
pub use stir_vfs::{Disk, Fs, Memory};
