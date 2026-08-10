//! File-based `tracing` setup for grove.
//!
//! Logging is entirely best-effort: if the log directory or file can't be
//! created, `init()` silently does nothing rather than failing startup. There
//! is no fallback to stdout/stderr — a headless GUI app has nowhere sane to
//! put that output, so "no logging installed" is the correct degraded state.

use fs_err as fs;
use fs_err::OpenOptions;
use std::io;

/// Log file is truncated once it exceeds this size, rather than rotated.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Install a file-backed `tracing` subscriber. Non-fatal: any failure (no
/// config dir, can't create the logs directory, can't open the log file)
/// leaves no subscriber installed and `init()` just returns.
pub fn init() {
    let Some(path) = log_path() else { return };
    let Some(dir) = path.parent() else { return };
    if fs::create_dir_all(dir).is_err() {
        return;
    }

    // ponytail: single 5 MiB self-truncating log; swap in tracing-appender if per-day rotation is ever needed.
    let should_truncate = fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES);

    // The log records project paths, branch names and command output — keep it
    // readable only by its owner. `mode` applies on creation only; a log file
    // that already exists keeps whatever mode it has.
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
    // `tracing_subscriber`'s `with_writer` only has a `MakeWriter` impl for
    // `std::fs::File`, not `fs_err::File`, so convert at this one boundary
    // and keep the rest of the module on `fs_err`.
    let file: std::fs::File = file.into();

    let filter = tracing_subscriber::EnvFilter::try_from_env("GROVE_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(file)
        .with_ansi(false)
        .finish();

    // Best-effort: if a subscriber is already installed (shouldn't happen —
    // `init()` is called once at startup), just skip rather than panic.
    let _ = tracing::subscriber::set_global_default(subscriber);
}

/// `<storage::config_dir()>/logs/grove.log`. Deliberately routes through
/// `storage::config_dir()` (rather than deriving `dirs::config_dir()`
/// independently) so that the legacy `work-manager` -> `grove` directory
/// migration it performs always runs before anything else can create the
/// `grove` directory as a side effect. `logging::init()` runs first in
/// `main()`, so if this function computed its own path it would create
/// `grove` before the migration ever got a chance to see an absent
/// directory, permanently defeating the migration guard.
fn log_path() -> Option<std::path::PathBuf> {
    let dir = grove_core::storage::config_dir().ok()?;
    Some(dir.join("logs").join("grove.log"))
}
