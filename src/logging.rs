//! File-based `tracing` setup for grove; entirely best-effort — `init()` silently no-ops on failure rather than falling back to stdout/stderr, since a headless GUI app has nowhere sane to put it.

use fs_err as fs;
use fs_err::OpenOptions;
use std::io;

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Non-fatal: any failure leaves no subscriber installed and `init()` just returns.
pub fn init() {
    let Some(path) = log_path() else { return };
    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }

    // Single 5 MiB self-truncating log; swap in tracing-appender if per-day rotation is ever needed.
    let should_truncate = fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES);

    // Readable only by its owner since it records paths/branches/command output; `mode` applies on creation only.
    let mut opts = OpenOptions::new();
    #[cfg(unix)]
    {
        use fs_err::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file: io::Result<fs::File> = if should_truncate {
        opts.create(true).write(true).truncate(true).open(&path)
    } else {
        opts.create(true).append(true).open(&path)
    };
    let Ok(file) = file else { return };
    // Converts to `std::fs::File` at this one boundary since `with_writer` has no `MakeWriter` impl for `fs_err::File`.
    let file: std::fs::File = file.into();

    let filter = tracing_subscriber::EnvFilter::try_from_env("GROVE_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(file)
        .with_ansi(false)
        .finish();

    // Best-effort: skip rather than panic if a subscriber is already installed.
    let _ = tracing::subscriber::set_global_default(subscriber);
}

/// `<storage::config_dir()>/logs/grove.log`; routed through `storage::config_dir()` (not `dirs::config_dir()` directly) so the legacy directory migration always runs before anything creates `grove` as a side effect.
fn log_path() -> Option<std::path::PathBuf> {
    let dir = grove_core::storage::config_dir().ok()?;
    Some(dir.join("logs").join("grove.log"))
}
