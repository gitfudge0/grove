//! Session recovery and history inventory backed by the persistence journal.

use super::{persistence, SavedSession, TranscriptItems};
use serde::Deserialize;
use std::{collections::HashMap, path::PathBuf};

pub fn recovery_dir() -> Result<PathBuf, String> {
    crate::storage::config_dir()
        .map(|p| p.join("codex-sessions"))
        .map_err(|e| e.to_string())
}
/// Closed native sessions live separately from startup recovery.
pub fn session_history_dir() -> Result<PathBuf, String> {
    crate::storage::config_dir()
        .map(|p| p.join("session-history"))
        .map_err(|e| e.to_string())
}

/// Unresolved filesystem recovery remains an owner even after its actor closes.
/// Only an explicit reconnect/reopen may exclude its own journal identity.
pub fn workspace_recovery_blocked(
    paths: &[PathBuf],
    excluding_owner: Option<&str>,
) -> Result<bool, String> {
    workspace_recovery_blocked_in_dirs(
        paths,
        excluding_owner,
        &[recovery_dir()?, session_history_dir()?],
    )
}
pub(crate) fn workspace_recovery_blocked_in_dirs(
    paths: &[PathBuf],
    excluding_owner: Option<&str>,
    dirs: &[PathBuf],
) -> Result<bool, String> {
    #[derive(Deserialize)]
    struct Summary {
        #[serde(default)]
        pending_edit: Option<PendingSummary>,
    }
    #[derive(Deserialize)]
    struct PendingSummary {
        #[serde(default)]
        recovery: Option<crate::checkpoint::Checkpoint>,
    }
    type SummaryCache = HashMap<
        PathBuf,
        (
            (u64, std::time::SystemTime),
            Option<crate::checkpoint::Checkpoint>,
        ),
    >;
    static SUMMARIES: std::sync::OnceLock<std::sync::Mutex<SummaryCache>> =
        std::sync::OnceLock::new();
    let summaries = SUMMARIES.get_or_init(std::sync::Mutex::default);
    let paths = paths
        .iter()
        .map(|path| path.canonicalize().unwrap_or_else(|_| path.clone()))
        .collect::<Vec<_>>();
    for dir in dirs {
        let entries = match fs_err::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "Workspace recovery ownership could not be checked: {error}"
                ));
            }
        };
        for entry in entries {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            if excluding_owner
                .is_some_and(|owner| path.file_stem().and_then(|stem| stem.to_str()) == Some(owner))
            {
                continue;
            }
            let file = match fs_err::File::open(&path) {
                Ok(file) => file,
                // Atomic archival/recovery can remove an entry after read_dir.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!(
                        "Workspace recovery journal could not be read: {error}"
                    ));
                }
            };
            let metadata = file.metadata().map_err(|error| error.to_string())?;
            let stamp = (
                metadata.len(),
                metadata.modified().map_err(|error| error.to_string())?,
            );
            let cached = summaries
                .lock()
                .map_err(|error| error.to_string())?
                .get(&path)
                .filter(|(previous, _)| *previous == stamp)
                .cloned();
            let recovery = if let Some((_, recovery)) = cached {
                recovery
            } else {
                let summary: Summary = serde_json::from_reader(std::io::BufReader::new(file))
                    .map_err(|e| {
                        format!(
                            "Workspace recovery journal {} needs attention: {e}",
                            path.display()
                        )
                    })?;
                let recovery = summary.pending_edit.and_then(|pending| pending.recovery);
                let mut cache = summaries.lock().map_err(|error| error.to_string())?;
                if cache.len() >= 4096 {
                    cache.clear();
                }
                cache.insert(path.clone(), (stamp, recovery.clone()));
                recovery
            };
            if let Some(recovery) = recovery {
                if recovery.roots().iter().any(|root| {
                    paths
                        .iter()
                        .any(|path| root.starts_with(path) || path.starts_with(root))
                }) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// Load archive list metadata without replaying journal transcripts. Legacy
/// inline archives are read compatibly and become small heads on their next save.
pub fn load_session_history_metadata() -> Result<Recovery, String> {
    load_metadata_from_dir(&session_history_dir()?)
}
/// Restore hidden sessions without replaying their transcripts or launching actors.
pub fn load_sessions_metadata() -> Result<Recovery, String> {
    load_metadata_from_dir(&recovery_dir()?)
}
pub fn load_active_session(key: &str) -> Result<SavedSession, String> {
    if !valid_key(key) {
        return Err("Invalid native session recovery key".into());
    }
    load_saved_session(&recovery_dir()?.join(format!("{key}.json")))
}
fn load_metadata_from_dir(dir: &std::path::Path) -> Result<Recovery, String> {
    let mut recovery = Recovery {
        sessions: Vec::new(),
        warnings: Vec::new(),
    };
    if !dir.exists() {
        return Ok(recovery);
    }
    let mut sessions = Vec::new();
    for entry in fs_err::read_dir(dir).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let saved = fs_err::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<SavedSession>(&bytes).map_err(|error| error.to_string())
            });
        match saved {
            Ok(mut saved)
                if valid_key(&saved.key)
                    && path.file_stem().and_then(|stem| stem.to_str()) == Some(&saved.key) =>
            {
                saved.items = TranscriptItems::default();
                sessions.push((
                    std::cmp::Reverse(
                        entry
                            .metadata()
                            .and_then(|metadata| metadata.modified())
                            .ok(),
                    ),
                    saved,
                ));
            }
            _ => recovery.warnings.push(format!(
                "Could not read native session history {}. The recovery file was preserved.",
                path.display()
            )),
        }
    }
    sessions.sort_by_key(|(modified, _)| *modified);
    recovery.sessions = sessions.into_iter().map(|(_, saved)| saved).collect();
    Ok(recovery)
}
/// Hydrate exactly one archived transcript on explicit preview/reopen or a
/// background history search. Keys are validated before forming filesystem paths.
pub fn load_history_session(key: &str) -> Result<SavedSession, String> {
    if !valid_key(key) {
        return Err("Invalid native session history key".into());
    }
    let saved = load_saved_session(&session_history_dir()?.join(format!("{key}.json")))?;
    if saved.key != key {
        return Err("Mismatched native session history key".into());
    }
    Ok(saved)
}

pub fn load_session_history() -> Result<Recovery, String> {
    let dir = session_history_dir()?;
    if !dir.exists() {
        return Ok(Recovery {
            sessions: Vec::new(),
            warnings: Vec::new(),
        });
    }
    let mut history = load_from_dir(&dir)?;
    history.sessions.sort_by_cached_key(|saved| {
        std::cmp::Reverse(
            fs_err::metadata(dir.join(format!("{}.json", saved.key)))
                .and_then(|metadata| metadata.modified())
                .ok(),
        )
    });
    Ok(history)
}

/// Persist recovery before an archived record is adopted by the live registry.
pub fn prepare_history_reopen(saved: &SavedSession) -> Result<(), String> {
    persist(saved)
}

/// Called only after the live registry owns the reopened session.
pub fn finish_history_reopen(key: &str) -> Result<(), String> {
    if !valid_key(key) {
        return Err("Invalid native session history key".into());
    }
    persistence::remove(&session_history_dir()?.join(format!("{key}.json")))
}

pub(crate) fn archive_closed_session(saved: &SavedSession) -> Result<(), String> {
    archive_closed_in_dirs(saved, &recovery_dir()?, &session_history_dir()?)
}

pub(crate) fn archive_closed_in_dirs(
    saved: &SavedSession,
    active: &std::path::Path,
    history: &std::path::Path,
) -> Result<(), String> {
    // The archive is durable before removing the only startup recovery record.
    persist_in_dir(saved, history)?;
    persistence::remove(&active.join(format!("{}.json", saved.key)))
}
fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() < 128
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
pub struct Recovery {
    pub sessions: Vec<SavedSession>,
    pub warnings: Vec<String>,
}
pub fn load_sessions() -> Result<Recovery, String> {
    let dir = recovery_dir()?;
    if !dir.exists() {
        return Ok(Recovery {
            sessions: Vec::new(),
            warnings: Vec::new(),
        });
    }
    load_from_dir(&dir)
}
/// Read a native session recovery/archive file, hydrating its committed transcript
/// journal when present. Direct JSON deserialization is only for inline wire data.
pub fn load_saved_session(path: &std::path::Path) -> Result<SavedSession, String> {
    let mut saved = persistence::load(path)?;
    if !valid_key(&saved.key) || path.file_stem().and_then(|stem| stem.to_str()) != Some(&saved.key)
    {
        return Err("Mismatched native session recovery key".into());
    }
    crate::visuals::recover(&mut saved);
    Ok(saved)
}
pub(crate) fn load_from_dir(dir: &std::path::Path) -> Result<Recovery, String> {
    let mut recovery = Recovery {
        sessions: Vec::new(),
        warnings: Vec::new(),
    };
    for entry in fs_err::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let parsed = persistence::load(&path);
        match parsed {
            Ok(session)
                if valid_key(&session.key)
                    && path.file_stem().and_then(|x| x.to_str()) == Some(&session.key) =>
            {
                let mut session = session;
                crate::visuals::recover(&mut session);
                recovery.sessions.push(session);
            }
            _ => recovery.warnings.push(format!(
                "Could not restore native session {}. The recovery file was preserved.",
                path.display()
            )),
        }
    }
    Ok(recovery)
}

pub(crate) fn persist(saved: &SavedSession) -> Result<(), String> {
    if !valid_key(&saved.key) {
        return Err("Invalid provider recovery key".into());
    }
    persist_in_dir(saved, &recovery_dir()?)
}
pub(crate) fn persist_in_dir(saved: &SavedSession, dir: &std::path::Path) -> Result<(), String> {
    if !valid_key(&saved.key) {
        return Err("Invalid provider recovery key".into());
    }
    persistence::persist(saved, dir)
}

#[cfg(test)]
pub(crate) fn recover_saved_for_test(saved: &SavedSession) -> SavedSession {
    let dir = tempfile::tempdir().unwrap();
    persist_in_dir(saved, dir.path()).unwrap();
    load_from_dir(dir.path()).unwrap().sessions.remove(0)
}
