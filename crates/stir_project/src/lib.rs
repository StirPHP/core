//! StirPHP project tooling: `sphp.toml` loading, source file discovery,
//! Composer autoload rules and `sphp init`.

mod composer;
mod config;
mod discover;
mod error;
mod init;
pub mod workspace;

pub use composer::{Autoload, InstalledPackage, load_autoload, parse_installed};
pub use config::{CONFIG_FILE, Config, Project};
pub use discover::{FileKind, SourceFile, discover};
pub use error::{Error, Result};
pub use init::{InitOptions, InitReport, SAMPLE_MAIN, default_name, init};
