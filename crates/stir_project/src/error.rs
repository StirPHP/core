use std::path::PathBuf;

/// Errors produced while loading, discovering or initialising a StirPHP project.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Config(#[from] stir_neon::phpstan::ConfigError),

    #[error("failed to read `{}`: {source}", .path.display())]
    Composer {
        path: PathBuf,
        #[source]
        source: stir_composer::ComposerError,
    },

    #[error("invalid glob pattern `{pattern}`: {source}")]
    Glob {
        pattern: String,
        #[source]
        source: globset::Error,
    },

    #[error("error while walking source files: {0}")]
    Walk(#[from] ignore::Error),

    #[error("can't add to `{}` without changing the rest of it: {reason}", .path.display())]
    CantEditConfig { path: PathBuf, reason: String },

    #[error("background task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
