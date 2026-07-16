use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("no .sapphire-timer directory found in current directory or any parent")]
    TimerNotFound,

    #[error("no preset named `{0}` in presets/")]
    PresetNotFound(String),

    #[error("preset name `{0}` is ambiguous ({1} matches)")]
    AmbiguousPreset(String, usize),

    #[error("{path}: invalid preset: {message}")]
    InvalidPreset { path: PathBuf, message: String },

    #[error("{path}: invalid session log line {line}: {message}")]
    InvalidSession {
        path: PathBuf,
        line: usize,
        message: String,
    },

    #[error("invalid config: {0}")]
    InvalidConfig(String),

    #[error("failed to serialize config: {0}")]
    ConfigSerialize(#[from] toml::ser::Error),

    /// Failure from the framework's pure-Rust retrieve backend (redb/tantivy).
    #[error("retrieve cache error: {0}")]
    RetrieveCache(String),

    #[error("workspace error: {0}")]
    Workspace(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<sapphire_workspace::Error> for Error {
    fn from(e: sapphire_workspace::Error) -> Self {
        Error::Workspace(e.to_string())
    }
}

impl From<sapphire_workspace::RetrieveError> for Error {
    fn from(e: sapphire_workspace::RetrieveError) -> Self {
        Error::RetrieveCache(e.to_string())
    }
}
