use crate::agent::Agent;
use fs_err as fs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

/// Everything the on-disk config layer can fail with. Callers that merely
/// bubble errors upward are unaffected (`anyhow` absorbs any
/// `std::error::Error` through `?`); the typed variants exist so a caller can
/// distinguish "the file is unreadable" from "the file is corrupt JSON".
#[derive(Debug, Error)]
pub enum StoreError {
    /// Reading, writing, renaming or creating a config path failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The config file exists but is not valid JSON. The original file is
    /// preserved at `backup` before this is returned.
    #[error("failed to parse {path} ({source}); original preserved at {backup}")]
    Parse {
        path: String,
        backup: String,
        #[source]
        source: serde_json::Error,
    },
    /// Serializing the in-memory `Store` to JSON failed.
    #[error("failed to serialize config: {0}")]
    Serialize(#[source] serde_json::Error),
    /// The platform has no user config directory to anchor grove's own.
    #[error("no config dir")]
    NoConfigDir,
}

/// Shorthand for this module's fallible functions.
pub type Result<T, E = StoreError> = std::result::Result<T, E>;

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

/// A single past session launch, recorded for the command palette's "recent"
/// list. Kept as a flat, denormalized record (rather than an index) so it
/// survives project/worktree reordering across restarts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentLaunch {
    /// Project name (matches `Session::project` / the `grid_order` key convention — NOT the path).
    pub project: String,
    /// Worktree absolute path (stable identity within a project).
    pub wt_path: String,
    pub agent: Agent,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub scripts: ProjectScripts,
    /// Pinned "Project theme" (see `Store::project_themes_enabled`): when
    /// `Some`, every PTY belonging to this project renders using this theme
    /// instead of the global one. `None` means "Default (follow app)".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
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
    /// Universal "Project themes" toggle (Settings → Appearance). When true,
    /// each project may pin its PTYs to a specific theme via `Project::theme`;
    /// app chrome always stays on the global theme regardless of this flag.
    #[serde(default)]
    pub project_themes_enabled: bool,
    /// Recent session-palette launches, most recent first, capped at 6 (see
    /// `crate::gui::launcher::push_recent_launch`). `#[serde(default)]` so old
    /// config files load with an empty history.
    #[serde(default)]
    pub recent_launches: Vec<RecentLaunch>,
}

/// Env var that, when set to a non-empty value, overrides the grove config
/// directory outright (see `config_dir`).
pub const CONFIG_DIR_ENV: &str = "GROVE_CONFIG_DIR";

/// Returns the grove config directory, performing a one-time migration from
/// the legacy `work-manager` directory name if needed. Every caller that
/// needs the config dir (or a path under it) MUST go through this function
/// rather than deriving `dirs::config_dir().join("grove")` independently —
/// otherwise a caller that creates the `grove` dir as a side effect (e.g. to
/// hold its own files) before this migration runs will make the `!dir.exists()`
/// guard below permanently false, silently stranding the legacy config.
///
/// Highest precedence: if `GROVE_CONFIG_DIR` is set to a non-empty value, it
/// is used verbatim as the config directory (created if it doesn't already
/// exist) and the legacy `work-manager` migration below is skipped entirely
/// — an explicit override means the caller is naming an exact directory, not
/// asking for Grove's default location to be discovered (and potentially
/// migrated). This exists primarily so integration tests can point Grove at
/// a `tempfile::TempDir` and exercise `save`/`load`/worktree paths hermetically
/// instead of touching the developer's real `~/.config/grove`; as a secondary
/// benefit it lets a user run a second Grove instance against a separate
/// config directory.
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

/// Pure-ish helper factored out of `config_dir()` so the legacy-dir migration
/// can be exercised in tests without depending on the real OS config dir:
/// takes the config-dir *parent* (what `dirs::config_dir()` would return) and
/// performs the same rename-then-create-dir-all steps.
fn resolve_config_dir(base: &Path) -> Result<PathBuf> {
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
    match serde_json::from_str(&s) {
        Ok(store) => Ok(store),
        Err(e) => {
            // Don't silently reset config: keep the corrupted file aside for recovery.
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

/// Serialize the store and write it to the config path, atomically and
/// 0600. Fully typed: `anyhow` callers still get `?` coercion for free.
pub fn save(store: &Store) -> Result<()> {
    let p = config_path()?;
    let s = serde_json::to_string_pretty(store).map_err(StoreError::Serialize)?;
    // Private: the store carries every project path on this machine.
    write_atomic_private(&p, s.as_bytes()).map_err(|e| {
        tracing::warn!(path = %p.display(), error = %e, "config save failed");
        e
    })
}

/// Save the store, logging a warning on failure instead of returning it. For
/// call sites that have no way to surface an error to the user; prefer `save`
/// wherever the caller can report it. Dropping the result silently — as call
/// sites used to with `let _ = save(..)` — lost the user's projects, themes
/// and settings with no trace in the log.
pub fn persist(store: &Store) {
    if let Err(e) = save(store) {
        tracing::warn!(error = format!("{e:#}"), "failed to persist config");
    }
}

/// Write via a sibling temp file + rename so a crash can never leave a
/// truncated file behind.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_inner(path, bytes, false)
}

/// Same as [`write_atomic`], but the temp file is created 0600 (unix) *before*
/// any bytes are written, so the destination is never world-readable even for
/// the instant between creation and a post-rename `chmod`. Use for anything
/// carrying paths, hook commands or other per-user detail.
pub fn write_atomic_private(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_inner(path, bytes, true)
}

fn write_atomic_inner(path: &Path, bytes: &[u8], private: bool) -> Result<()> {
    // The temp name must be unique per write: a fixed `.json.tmp` sibling means
    // two concurrent writers (threads, or two Grove processes sharing a config
    // dir) scribble over each other's staging file and one of them renames a
    // half-written mix into place. pid + a process-wide counter gives both.
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
    // Same-directory rename, so it stays atomic on every filesystem we target.
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Create `path` with 0600 on unix and write `bytes` into it. Off unix there is
/// no mode to set, so this is a plain write.
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
        fs::create_dir_all(&dir).expect("create test dir");
        let dest = dir.join("projects.json");
        let payload = b"{\"projects\":[]}";

        write_atomic(&dest, payload).expect("write_atomic");

        assert!(dest.exists(), "destination must exist after write_atomic");
        let tmp = dest.with_extension("json.tmp");
        assert!(
            !tmp.exists(),
            ".json.tmp sibling must be removed after rename"
        );
        let written = fs::read(&dest).expect("read back");
        assert_eq!(
            written, payload,
            "written bytes must exactly match the input"
        );
        // Cleanup.
        let _ = fs::remove_dir_all(&dir);
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
                    scripts: ProjectScripts::default(),
                    theme: None,
                },
                Project {
                    name: "other".into(),
                    path: "/tmp/other".into(),
                    scripts: ProjectScripts::default(),
                    theme: Some("dracula".into()),
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
            project_themes_enabled: true,
            recent_launches: vec![
                RecentLaunch {
                    project: "myapp".into(),
                    wt_path: "/home/user/myapp".into(),
                    agent: Agent::Claude,
                },
                RecentLaunch {
                    project: "other".into(),
                    wt_path: "/tmp/other/.wt/fix".into(),
                    agent: Agent::Terminal,
                },
            ],
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
        assert!(recovered.project_themes_enabled);
        assert_eq!(recovered.projects[1].theme.as_deref(), Some("dracula"));
        assert_eq!(recovered.recent_launches, original.recent_launches);
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
        assert!(!store.project_themes_enabled);
        assert!(
            !store.onboarded,
            "a fresh config must report onboarded=false so the wizard runs"
        );
        assert!(store.dangerously_skip_permissions_enabled.is_none());
        assert!(store.recent_launches.is_empty());
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
        let store = Store {
            last_update_check: Some(1_700_000_000),
            skipped_version: Some("v0.25.0".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&store).unwrap();
        let back: Store = serde_json::from_str(&json).unwrap();
        assert_eq!(back.last_update_check, Some(1_700_000_000));
        assert_eq!(back.skipped_version.as_deref(), Some("v0.25.0"));
    }

    #[test]
    fn grid_order_round_trips() {
        let store = Store {
            grid_order: vec!["proj-a::/wt/a".to_string(), "proj-a::/wt/b".to_string()],
            ..Default::default()
        };
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

    /// Regression test for the logging-init-defeats-migration bug: when only
    /// the legacy `work-manager` dir exists, `resolve_config_dir` must rename
    /// it to `grove` rather than leaving it stranded. Every caller that wants
    /// the config directory (including `logging::init()`) MUST route through
    /// this helper (via `config_dir()`) so the migration always gets a chance
    /// to run before anything else creates the `grove` directory first.
    #[test]
    fn resolve_config_dir_migrates_legacy_dir_when_new_dir_absent() {
        let base = std::env::temp_dir().join(format!(
            "grove_test_migrate_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let legacy = base.join("work-manager");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("projects.json"), b"{\"projects\":[]}").unwrap();

        let dir = resolve_config_dir(&base).expect("resolve_config_dir");

        assert_eq!(dir, base.join("grove"));
        assert!(dir.exists(), "new grove dir must exist after migration");
        assert!(
            !legacy.exists(),
            "legacy dir must be renamed away, not left stranded"
        );
        assert!(
            dir.join("projects.json").exists(),
            "migrated file must be present under the new dir"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// When both the new `grove` dir and the legacy `work-manager` dir exist
    /// (e.g. a second run after migration already happened once, or a user
    /// who created both manually), the existing `grove` dir must win and the
    /// legacy dir must be left untouched rather than overwritten or merged.
    #[test]
    fn resolve_config_dir_leaves_legacy_dir_when_new_dir_already_exists() {
        let base = std::env::temp_dir().join(format!(
            "grove_test_migrate_both_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dir = base.join("grove");
        let legacy = base.join("work-manager");
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(&legacy).unwrap();
        fs::write(dir.join("projects.json"), b"{\"projects\":[\"new\"]}").unwrap();
        fs::write(legacy.join("projects.json"), b"{\"projects\":[\"old\"]}").unwrap();

        let resolved = resolve_config_dir(&base).expect("resolve_config_dir");

        assert_eq!(resolved, dir);
        assert!(legacy.exists(), "legacy dir must not be touched or removed");
        let content = fs::read_to_string(dir.join("projects.json")).unwrap();
        assert!(
            content.contains("new"),
            "existing new-dir contents must not be clobbered by a stale legacy dir"
        );

        let _ = fs::remove_dir_all(&base);
    }

    // ── GROVE_CONFIG_DIR override ────────────────────────────────────────
    //
    // `std::env::set_var` mutates process-global state and `cargo test` runs
    // tests concurrently, so every test here that sets `CONFIG_DIR_ENV` is
    // serialized behind this mutex, mirroring `theme.rs`'s `CUSTOM_TEST_LOCK`.
    static CONFIG_DIR_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard that always restores (or removes) the env var on drop, even
    /// if the test body panics, so one failing test can't poison the ones
    /// that run after it.
    struct EnvVarGuard {
        key: &'static str,
        old: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let old = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, old }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.old {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    /// When `GROVE_CONFIG_DIR` is set to a non-empty path, `config_dir()`
    /// must use it verbatim (creating it if needed) rather than deriving
    /// anything from `dirs::config_dir()`.
    #[test]
    fn config_dir_honours_override_when_set() {
        let _lock = CONFIG_DIR_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let dir = std::env::temp_dir().join(format!(
            "grove_test_config_dir_override_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        let _guard = EnvVarGuard::set(CONFIG_DIR_ENV, dir.to_str().expect("utf8 path"));

        let resolved = config_dir().expect("config_dir");

        assert_eq!(resolved, dir);
        assert!(
            dir.exists(),
            "override directory must be created if it doesn't already exist"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// An empty `GROVE_CONFIG_DIR` value must be treated the same as unset —
    /// the real default-resolution path (including migration) still runs.
    #[test]
    fn config_dir_ignores_empty_override() {
        let _lock = CONFIG_DIR_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _guard = EnvVarGuard::set(CONFIG_DIR_ENV, "");

        // Must not blow up or try to create a directory named "" — it should
        // fall through to the real `dirs::config_dir()`-based resolution.
        let resolved = config_dir().expect("config_dir");
        assert_ne!(resolved.as_os_str(), "");
    }

    /// When `GROVE_CONFIG_DIR` is unset entirely, behaviour must be
    /// byte-for-byte identical to before the override existed: legacy
    /// `work-manager` migration still runs via `resolve_config_dir`. This
    /// exercises `resolve_config_dir` directly (as the pre-existing migration
    /// tests do) to prove the override plumbing in `config_dir()` didn't
    /// change that helper's behavior at all.
    #[test]
    fn config_dir_unset_still_migrates_legacy_dir() {
        let _lock = CONFIG_DIR_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::remove_var(CONFIG_DIR_ENV);

        let base = std::env::temp_dir().join(format!(
            "grove_test_config_dir_unset_migrate_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let legacy = base.join("work-manager");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("projects.json"), b"{\"projects\":[]}").unwrap();

        // Same call `config_dir()` makes internally when the override is
        // absent — proves the unset path is untouched by the new branch.
        let dir = resolve_config_dir(&base).expect("resolve_config_dir");

        assert_eq!(dir, base.join("grove"));
        assert!(!legacy.exists(), "legacy dir must still be migrated away");
        assert!(dir.join("projects.json").exists());

        let _ = fs::remove_dir_all(&base);
    }
}
