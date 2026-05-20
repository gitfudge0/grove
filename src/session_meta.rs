use crate::agent::Agent;
use anyhow::{Context, Result};
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
    let base = dirs::config_dir().context("no config dir")?;
    let dir = base.join("grove").join("sessions");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn path_for(name: &str) -> Result<PathBuf> {
    Ok(sessions_dir()?.join(format!("{}.json", name)))
}

pub fn write(name: &str, meta: &SessionMeta) -> Result<()> {
    let p = path_for(name)?;
    let s = serde_json::to_string_pretty(meta)?;
    std::fs::write(p, s)?;
    Ok(())
}

pub fn read(name: &str) -> Option<SessionMeta> {
    let p = path_for(name).ok()?;
    let s = std::fs::read_to_string(&p).ok()?;
    serde_json::from_str(&s).ok()
}

pub fn delete(name: &str) {
    if let Ok(p) = path_for(name) {
        let _ = std::fs::remove_file(p);
    }
}

/// Remove sidecar files whose tmux session no longer exists.
pub fn prune(live: &[String]) {
    let Ok(dir) = sessions_dir() else { return };
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if !live.iter().any(|n| n == stem) {
            let _ = std::fs::remove_file(&path);
        }
    }
}
