//! The session layer's error type.
//!
//! Lived in `session.rs` until Plan 10 Task 7 Step 2 deleted that module along
//! with the iced app's PTY-backed session runtime. `session_meta` and
//! `attention` are the surviving consumers, and neither owns the type, so it
//! gets its own module rather than being folded into one of them.

use thiserror::Error;

/// Everything the session layer — the sidecar metadata and attention files
/// that hang off a session — can fail with.
/// Mirrors `git::GitError` / `storage::StoreError`: `anyhow` callers keep
/// working through `?`, and a caller that cares can match a variant instead
/// of grepping a formatted string.
///
/// `session_meta` and `attention` share this type rather than defining their
/// own: both are session-scoped sidecars whose only failure modes are the
/// config-dir/I/O ones already spelled out here.
#[derive(Debug, Error)]
pub enum SessionError {
    /// Raw I/O on a session path (creating the sessions dir, reading a
    /// sidecar, …).
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The config layer failed: no config dir, or an atomic write failed.
    #[error(transparent)]
    Store(#[from] crate::storage::StoreError),
    /// Creating or configuring the backing tmux session failed.
    #[error(transparent)]
    Tmux(#[from] crate::tmux::TmuxError),
    /// A tmux session name would escape the sessions directory. Names come
    /// back from `tmux list-sessions`, so they are never fully trusted.
    #[error("invalid session name: {0:?}")]
    InvalidName(String),
    /// Serializing a `SessionMeta` sidecar to JSON failed.
    #[error("failed to serialize session metadata: {0}")]
    Serialize(#[source] serde_json::Error),
    /// The sidecar could not be written, so the session would be
    /// unrediscoverable after a restart; the caller kills the tmux session.
    #[error("failed to write session metadata")]
    MetaWrite(#[source] Box<SessionError>),
    /// Opening the pseudo-terminal, spawning into it, or cloning its
    /// reader/writer failed. `portable-pty` reports these as `anyhow`
    /// errors, so the payload is kept boxed and its message preserved.
    #[error("{0}")]
    Pty(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Shorthand for the session layer's fallible functions.
pub type Result<T, E = SessionError> = std::result::Result<T, E>;
