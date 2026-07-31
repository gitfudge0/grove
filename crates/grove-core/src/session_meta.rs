use crate::agent::Agent;
use crate::error::{Result, SessionError};
use fs_err as fs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMeta {
    pub wt_path: String,
    pub project: String,
    pub label: String,
    pub agent: Agent,
}

fn sessions_dir() -> Result<PathBuf> {
    // Via `storage::config_dir()` so `GROVE_CONFIG_DIR` (and the legacy dir
    // migration) is honored here as well.
    let dir = crate::storage::config_dir()?.join("sessions");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn path_for(name: &str) -> Result<PathBuf> {
    // Session names come back from `tmux list-sessions`; never let one escape
    // the sessions dir.
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(SessionError::InvalidName(name.to_string()));
    }
    Ok(sessions_dir()?.join(format!("{name}.json")))
}

pub fn write(name: &str, meta: &SessionMeta) -> Result<()> {
    let p = path_for(name)?;
    let s = serde_json::to_string_pretty(meta).map_err(SessionError::Serialize)?;
    crate::storage::write_atomic(&p, s.as_bytes()).map_err(|e| {
        tracing::debug!(name, error = %e, "session_meta: write failed");
        e
    })?;
    Ok(())
}

pub fn read(name: &str) -> Option<SessionMeta> {
    let p = path_for(name).ok()?;
    let s = match fs::read_to_string(&p) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(name, error = %e, "session_meta: read failed");
            return None;
        }
    };
    match serde_json::from_str(&s) {
        Ok(meta) => Some(meta),
        Err(e) => {
            tracing::debug!(name, error = %e, "session_meta: parse failed");
            None
        }
    }
}

pub fn delete(name: &str) {
    if let Ok(p) = path_for(name) {
        if let Err(e) = fs::remove_file(&p) {
            tracing::debug!(name, error = %e, "session_meta: delete failed");
        }
    }
}

/// Remove sidecar files whose tmux session no longer exists.
pub fn prune(live: &[String]) {
    let Ok(dir) = sessions_dir() else { return };
    let Ok(rd) = fs::read_dir(&dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if !live.iter().any(|n| n == stem) {
            let _ = fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;

    fn make_meta() -> SessionMeta {
        SessionMeta {
            wt_path: "/tmp/test-wt".into(),
            project: "testproject".into(),
            label: "test-label".into(),
            agent: Agent::Claude,
        }
    }

    /// A session name containing `/` must cause `write` to return `Err` without
    /// touching disk. The rejection happens inside `path_for` before any I/O.
    #[test]
    fn write_rejects_slash_in_name() {
        let result = write("evil/path", &make_meta());
        assert!(
            result.is_err(),
            "write must reject a session name containing '/'"
        );
    }

    /// A session name containing `\\` must be rejected.
    #[test]
    fn write_rejects_backslash_in_name() {
        let result = write("evil\\path", &make_meta());
        assert!(
            result.is_err(),
            "write must reject a session name containing '\\\\'"
        );
    }

    /// A session name containing `..` must be rejected (path traversal).
    #[test]
    fn write_rejects_double_dot_in_name() {
        let result = write("..evil", &make_meta());
        assert!(
            result.is_err(),
            "write must reject a session name containing '..'"
        );
        let result2 = write("a..b", &make_meta());
        assert!(
            result2.is_err(),
            "write must reject a session name with '..' in the middle"
        );
    }

    /// `read` returns `None` for a name containing `/` — it never reads a file
    /// outside the sessions directory.
    #[test]
    fn read_returns_none_for_slash_in_name() {
        assert!(
            read("evil/path").is_none(),
            "read must return None for a name containing '/'"
        );
    }

    /// `read` returns `None` for a name containing `..`.
    #[test]
    fn read_returns_none_for_double_dot_in_name() {
        assert!(
            read("../../../etc/passwd").is_none(),
            "read must return None for a path-traversal session name"
        );
    }

    /// `SessionMeta` round-trips through `serde_json` faithfully.
    #[test]
    fn session_meta_serde_round_trip() {
        let meta = make_meta();
        let json = serde_json::to_string_pretty(&meta).expect("serialize");
        let back: SessionMeta = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.wt_path, meta.wt_path);
        assert_eq!(back.project, meta.project);
        assert_eq!(back.label, meta.label);
        assert_eq!(back.agent, meta.agent);
    }
}
