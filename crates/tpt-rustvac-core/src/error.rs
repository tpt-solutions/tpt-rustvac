use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error at {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("config error: {0}")]
    Config(String),

    #[error("refusing to scan {}: {reason}. Pass --force to override.", path.display())]
    Guardrail { path: PathBuf, reason: String },

    #[error("target is busy (modified within the last {grace_secs}s): {}", path.display())]
    Busy { path: PathBuf, grace_secs: u64 },

    #[error("archive verification failed for {}: {reason}", path.display())]
    Verify { path: PathBuf, reason: String },

    #[error("destination already exists: {}", .0.display())]
    Exists(PathBuf),

    #[error("archive not found: {0}")]
    NotFound(String),

    #[error("cannot archive {}: {reason}", path.display())]
    Archive { path: PathBuf, reason: String },

    #[error("could not move {} to the trash: {reason}", path.display())]
    Trash { path: PathBuf, reason: String },

    #[error("{0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn io(path: &Path, source: io::Error) -> Self {
        Error::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}
