//! The session layer's error type. Shared by `session_meta` and `attention`, neither of which owns it.

use thiserror::Error;

/// Mirrors `git::GitError` / `storage::StoreError`: `anyhow` callers keep working through `?`,
/// and a caller that cares can match a variant instead of grepping a formatted string.
#[derive(Debug, Error)]
pub enum SessionError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Store(#[from] crate::storage::StoreError),
    #[error(transparent)]
    Tmux(#[from] crate::tmux::TmuxError),
    /// Names come back from `tmux list-sessions`, so they are never fully trusted.
    #[error("invalid session name: {0:?}")]
    InvalidName(String),
    #[error("failed to serialize session metadata: {0}")]
    Serialize(#[source] serde_json::Error),
    /// Unrediscoverable after a restart if unwritten; the caller kills the tmux session.
    #[error("failed to write session metadata")]
    MetaWrite(#[source] Box<SessionError>),
    /// `portable-pty` reports these as `anyhow` errors, so the payload stays boxed with its message preserved.
    #[error("{0}")]
    Pty(#[source] Box<dyn std::error::Error + Send + Sync>),
}

pub type Result<T, E = SessionError> = std::result::Result<T, E>;
