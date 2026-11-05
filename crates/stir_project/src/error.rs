use std::path::PathBuf;

/// Errors produced while loading, discovering or initialising a StirPHP project.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not find `{file}` in `{start}` or any parent directory", file = crate::CONFIG_FILE, start = .start.display())]
    ConfigNotFound { start: PathBuf },

    #[error("{}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse `{}`: {source}", .path.display())]
    TomlParse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    #[error("failed to parse `{}`: {source}", .path.display())]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid glob pattern `{pattern}`: {source}")]
    Glob {
        pattern: String,
        #[source]
        source: globset::Error,
    },

    #[error("error while walking source files: {0}")]
    Walk(#[from] ignore::Error),

    #[error("`{}` already exists (use --force to overwrite)", .path.display())]
    AlreadyInitialized { path: PathBuf },

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
