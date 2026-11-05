//! StirPHP project tooling: `stirphp.neon` loading, source file discovery,
//! Composer autoload rules.

pub mod baseline;
mod composer;
mod config;
mod discover;
mod error;
pub mod init;
mod neon_edit;
pub mod workspace;

pub use composer::{
    Autoload, Composer, InstalledPackage, load_autoload, load_composer, parse_installed,
};
pub use config::{CONFIG_FILE, Config, PHPSTAN_CONFIG_FILES, Project, config_in};
pub use discover::{SourceFile, discover, php_files_under};
pub use error::{Error, Result};
