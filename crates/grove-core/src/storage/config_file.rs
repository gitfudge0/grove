//! Config location, compatibility migration and atomic file persistence.

use super::{Result, Store, StoreError};
use fs_err as fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const CONFIG_DIR_ENV: &str = "GROVE_CONFIG_DIR";

/// Callers MUST go through this, not derive `dirs::config_dir().join("grove")` directly, or the legacy `work-manager` migration can be defeated.
pub fn config_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var(CONFIG_DIR_ENV) {
        if !dir.is_empty() {
            let path = PathBuf::from(dir);
            fs::create_dir_all(&path)?;
            return Ok(path);
        }
    }
    let base = dirs::config_dir().ok_or(StoreError::NoConfigDir)?;
    let dir = resolve_config_dir(&base)?;
    Ok(dir)
}

/// Pure-ish helper factored out of `config_dir()` so the legacy-dir migration is testable without the real OS config dir.
pub(super) fn resolve_config_dir(base: &Path) -> Result<PathBuf> {
    let dir = base.join("grove");
    let legacy = base.join("work-manager");
    if !dir.exists() && legacy.exists() {
        let _ = fs::rename(&legacy, &dir);
    }
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("projects.json"))
}

pub fn load() -> Result<Store> {
    let p = config_path()?;
    if !p.exists() {
        return Ok(Store::default());
    }
    let s = fs::read_to_string(&p).map_err(|e| {
        tracing::warn!(path = %p.display(), error = %e, "config load failed: read error");
        e
    })?;
    match serde_json::from_str::<Store>(&s) {
        Ok(mut store) => {
            if store.appearance.is_none() {
                store.appearance = Some(store.appearance());
            }
            store.normalize_workspaces();
            Ok(store)
        }
        Err(e) => {
            // Keep the corrupted file aside for recovery rather than silently resetting.
            tracing::warn!(path = %p.display(), error = %e, "config load failed: corrupt JSON");
            let backup = p.with_extension("json.corrupt");
            let _ = fs::copy(&p, &backup);
            Err(StoreError::Parse {
                path: p.display().to_string(),
                backup: backup.display().to_string(),
                source: e,
            })
        }
    }
}

pub fn save(store: &Store) -> Result<()> {
    let p = config_path()?;
    let s = serde_json::to_string_pretty(store).map_err(StoreError::Serialize)?;
    write_atomic_private(&p, s.as_bytes()).map_err(|e| {
        tracing::warn!(path = %p.display(), error = %e, "config save failed");
        e
    })
}

/// Logs on failure instead of returning it; prefer `save` wherever the caller can report errors — `let _ = save(..)` used to silently drop projects/settings.
pub fn persist(store: &Store) {
    if let Err(e) = save(store) {
        tracing::warn!(error = format!("{e:#}"), "failed to persist config");
    }
}

/// Write via a sibling temp file + rename so a crash never leaves a truncated file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_inner(path, bytes, false)
}

/// Like [`write_atomic`], but the temp file is created 0600 before any bytes are written.
pub fn write_atomic_private(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_inner(path, bytes, true)
}

fn write_atomic_inner(path: &Path, bytes: &[u8], private: bool) -> Result<()> {
    // pid + process-wide counter: a fixed tmp name would let concurrent writers clobber each other.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let stem = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!(
        "{stem}.tmp.{}.{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    if private {
        write_private(&tmp, bytes)?;
    } else {
        fs::write(&tmp, bytes)?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use fs_err::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.write_all(bytes)?;
    Ok(())
}
