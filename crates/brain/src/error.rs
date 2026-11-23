use std::path::PathBuf;

/// Errors produced by `tag-brain`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Filesystem error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// SQLite error.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Invalid YAML frontmatter.
    #[error("invalid frontmatter in {}: {msg}", path.display())]
    Frontmatter {
        /// File that failed to parse.
        path: PathBuf,
        /// Parser message.
        msg: String,
    },
}

/// Result alias for `tag-brain`.
pub type Result<T> = std::result::Result<T, Error>;
