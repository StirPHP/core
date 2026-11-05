//! StirPHP project tooling: `sphp.toml` loading, source file discovery,
//! Composer autoload rules.

mod composer;
mod config;
mod discover;
mod error;
pub mod workspace;

pub use composer::{Autoload, InstalledPackage, load_autoload, parse_installed};
pub use config::{CONFIG_FILE, Config, Project};
pub use discover::{SourceFile, discover};
pub use error::{Error, Result};
