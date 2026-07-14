use crate::agent::Agent;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Optional per-project shell scripts run at worktree lifecycle points. Each is
/// a shell snippet (run via `$SHELL -lc`); `None`/empty means that lifecycle
/// step is a no-op. Shared by every worktree of the project.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProjectScripts {
    /// Runs when a new worktree is created (in the new worktree's directory).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    /// Runs on demand when the user triggers it from a worktree row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// Runs when a worktree is deleted, before `git worktree remove`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teardown: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub scripts: ProjectScripts,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub default_agent: Option<Agent>,
    #[serde(default)]
    pub theme: Option<String>,
    /// None means the user has not made the one-time tmux/native choice yet.
    #[serde(default)]
    pub tmux_enabled: Option<bool>,
    #[serde(default)]
    pub ui_zoom: Option<f32>,
    /// Sidebar width in logical pixels, set by dragging the divider. None falls
    /// back to the default `RAIL_W`.
    #[serde(default)]
    pub sidebar_width: Option<f32>,
    /// Whether the first-run onboarding wizard has been completed or skipped.
    /// False (the serde default) means a fresh install — the wizard runs on the
    /// next launch. Set true once the user finishes or skips it, so it never
    /// reappears even if they later remove every project.
    #[serde(default)]
    pub onboarded: bool,
    /// Unix timestamp (seconds) of the last completed update check. Gates the
    /// periodic (24h) trigger. `#[serde(default)]` so old config files load.
    #[serde(default)]
    pub last_update_check: Option<i64>,
    /// The release tag the user chose to skip. While the latest release equals
    /// this value no update notice is shown; a newer release clears it.
    #[serde(default)]
    pub skipped_version: Option<String>,
    /// None means unset; treated as `true` (bypass enabled) to preserve the
    /// pre-existing hardcoded behavior for upgrading users.
    #[serde(default)]
    pub dangerously_skip_permissions_enabled: Option<bool>,
    /// Whether anonymous usage telemetry is sent. None means unset; treated
    /// as `true` (opt-out model) to match existing settings-field conventions.
    #[serde(default)]
    pub telemetry_enabled: Option<bool>,
    /// User-controlled ordering of Agent View grid tiles, keyed by
    /// `"{project}::{wt_path}"` (see `crate::gui::launcher::session_grid_key`).
    /// Sessions not present here (new sessions) are appended after the ones
    /// listed, in their current in-memory order. `#[serde(default)]` so old
    /// config files load with an empty order.
    #[serde(default)]
    pub grid_order: Vec<String>,
    /// Whether the "system" theme option is active: the active theme tracks
    /// the OS light/dark setting instead of a fixed choice. When true,
    /// `theme_dark`/`theme_light` supply the concrete theme for each mode.
    #[serde(default)]
    pub theme_follow_system: bool,
    /// The user's chosen dark-mode theme, used both directly and as the dark
    /// side of "system" mode. `#[serde(default)]` so old config files load.
    #[serde(default)]
    pub theme_dark: Option<String>,
    /// The user's chosen light-mode theme, used both directly and as the
    /// light side of "system" mode. `#[serde(default)]` so old config files load.
    #[serde(default)]
    pub theme_light: Option<String>,
}

pub fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no config dir")?;
    let dir = base.join("grove");
    let legacy = base.join("work-manager");
    if !dir.exists() && legacy.exists() {
        let _ = std::fs::rename(&legacy, &dir);
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("projects.json"))
}

pub fn load() -> Result<Store> {
    let p = config_path()?;
    if !p.exists() {
        return Ok(Store::default());
    }
    let s = std::fs::read_to_string(&p)?;
    match serde_json::from_str(&s) {
        Ok(store) => Ok(store),
        Err(e) => {
            // Don't silently reset config: keep the corrupted file aside for recovery.
            let backup = p.with_extension("json.corrupt");
            let _ = std::fs::copy(&p, &backup);
            Err(anyhow::anyhow!(
                "failed to parse {} ({}); original preserved at {}",
                p.display(),
                e,
                backup.display()
            ))
        }
    }
}

pub fn save(store: &Store) -> Result<()> {
    let p = config_path()?;
    let s = serde_json::to_string_pretty(store)?;
    write_atomic(&p, s.as_bytes())
}

/// Write via a sibling temp file + rename so a crash can never leave a
/// truncated file behind.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;

    /// `write_atomic` produces the full file content at the destination and
    /// leaves no `.tmp` sibling behind after a successful write.
    #[test]
    fn write_atomic_no_tmp_residue_and_full_content() {
        // Use a uniquely-named subdirectory of the system temp dir so parallel
        // test runs never collide and cleanup is easy.
        let dir = std::env::temp_dir().join(format!(
            "grove_test_storage_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let dest = dir.join("projects.json");
        let payload = b"{\"projects\":[]}";

        write_atomic(&dest, payload).expect("write_atomic");

        assert!(dest.exists(), "destination must exist after write_atomic");
        let tmp = dest.with_extension("json.tmp");
        assert!(
            !tmp.exists(),
            ".json.tmp sibling must be removed after rename"
        );
        let written = std::fs::read(&dest).expect("read back");
        assert_eq!(
            written, payload,
            "written bytes must exactly match the input"
        );
        // Cleanup.
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `Store` round-trips through `serde_json` with all fields populated,
    /// preserving each value exactly.
    #[test]
    fn store_serde_round_trip() {
        let original = Store {
            projects: vec![
                Project {
                    name: "myapp".into(),
                    path: "/home/user/myapp".into(),
                    scripts: Default::default(),
                },
                Project {
                    name: "other".into(),
                    path: "/tmp/other".into(),
                    scripts: Default::default(),
                },
            ],
            default_agent: Some(Agent::Claude),
            theme: Some("dark".into()),
            tmux_enabled: Some(true),
            ui_zoom: Some(1.25),
            sidebar_width: Some(360.0),
            onboarded: true,
            last_update_check: None,
            skipped_version: None,
            dangerously_skip_permissions_enabled: Some(false),
            telemetry_enabled: Some(true),
            grid_order: vec![],
            theme_follow_system: true,
            theme_dark: Some("tokyonight".into()),
            theme_light: Some("tokyonight-day".into()),
        };

        let json = serde_json::to_string_pretty(&original).expect("serialize");
        let recovered: Store = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(recovered.projects.len(), 2);
        assert_eq!(recovered.projects[0].name, "myapp");
        assert_eq!(recovered.projects[1].path, "/tmp/other");
        assert_eq!(recovered.default_agent, Some(Agent::Claude));
        assert_eq!(recovered.theme.as_deref(), Some("dark"));
        assert_eq!(recovered.tmux_enabled, Some(true));
        assert!((recovered.ui_zoom.unwrap() - 1.25).abs() < f32::EPSILON);
        assert!((recovered.sidebar_width.unwrap() - 360.0).abs() < f32::EPSILON);
        assert!(recovered.onboarded);
        assert_eq!(recovered.dangerously_skip_permissions_enabled, Some(false));
        assert!(recovered.theme_follow_system);
        assert_eq!(recovered.theme_dark.as_deref(), Some("tokyonight"));
        assert_eq!(recovered.theme_light.as_deref(), Some("tokyonight-day"));
    }

    /// `Store::default()` deserializes from an empty JSON object — the
    /// `#[serde(default)]` attributes on every field must all be present.
    #[test]
    fn store_deserializes_from_empty_object() {
        let store: Store = serde_json::from_str("{}").expect("deserialize empty object");
        assert!(store.projects.is_empty());
        assert!(store.default_agent.is_none());
        assert!(store.theme.is_none());
        assert!(store.tmux_enabled.is_none());
        assert!(store.ui_zoom.is_none());
        assert!(store.sidebar_width.is_none());
        assert!(!store.theme_follow_system);
        assert!(store.theme_dark.is_none());
        assert!(store.theme_light.is_none());
        assert!(
            !store.onboarded,
            "a fresh config must report onboarded=false so the wizard runs"
        );
        assert!(store.dangerously_skip_permissions_enabled.is_none());
    }

    /// A corrupted JSON file must make `write_atomic` + a subsequent manual
    /// corrupt-parse test return `Err` rather than silently yielding a default
    /// store. This mirrors the invariant in `load()` without touching the real
    /// config dir.
    #[test]
    fn corrupt_json_parse_returns_err_not_default() {
        let corrupt = "{ NOT VALID JSON !!!";
        let result: Result<Store, _> = serde_json::from_str(corrupt);
        assert!(
            result.is_err(),
            "corrupt JSON must not silently produce a default Store"
        );
    }

    #[test]
    fn store_loads_without_update_fields_and_defaults_them() {
        // Existing config files predate these fields; they must default to None.
        let store: Store = serde_json::from_str("{}").unwrap();
        assert!(store.last_update_check.is_none());
        assert!(store.skipped_version.is_none());
    }

    #[test]
    fn store_round_trips_update_fields() {
        let mut store = Store::default();
        store.last_update_check = Some(1_700_000_000);
        store.skipped_version = Some("v0.25.0".to_string());
        let json = serde_json::to_string(&store).unwrap();
        let back: Store = serde_json::from_str(&json).unwrap();
        assert_eq!(back.last_update_check, Some(1_700_000_000));
        assert_eq!(back.skipped_version.as_deref(), Some("v0.25.0"));
    }

    #[test]
    fn grid_order_round_trips() {
        let mut store = Store::default();
        store.grid_order = vec!["proj-a::/wt/a".to_string(), "proj-a::/wt/b".to_string()];
        let json = serde_json::to_string(&store).expect("serialize");
        let recovered: Store = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(recovered.grid_order, store.grid_order);
    }

    #[test]
    fn grid_order_defaults_empty_for_old_config() {
        // Simulates loading a config file written before this field existed.
        let store: Store = serde_json::from_str("{}").expect("deserialize");
        assert!(store.grid_order.is_empty());
    }
}
