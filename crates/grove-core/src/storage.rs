use crate::agent::Agent;
use fs_err as fs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

/// Typed config-layer errors; callers that just bubble up via `anyhow` are unaffected.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The original file is preserved at `backup` before this is returned.
    #[error("failed to parse {path} ({source}); original preserved at {backup}")]
    Parse {
        path: String,
        backup: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to serialize config: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("no config dir")]
    NoConfigDir,
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

/// Optional per-project shell scripts run at worktree lifecycle points, shared by every worktree.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProjectScripts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teardown: Option<String>,
}

/// A denormalized (not indexed) past-launch record so it survives project/worktree reordering.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentLaunch {
    pub project: String,
    pub wt_path: String,
    pub agent: Agent,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub scripts: ProjectScripts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub archived: bool,
    /// Pinned on the FIRST rename and never changed after, so renaming can't orphan existing worktrees; `None` means "same as `name`".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_dir: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Project {
    pub fn worktree_dir(&self) -> &str {
        self.worktree_dir.as_deref().unwrap_or(&self.name)
    }
}

/// Freeze `project.worktree_dir` at `old_name` if not already pinned; called during a rename, before `name` changes.
pub fn pin_worktree_dir_on_rename(project: &mut Project, old_name: &str) {
    if project.worktree_dir.is_none() {
        project.worktree_dir = Some(old_name.to_string());
    }
}

/// Resolves by path, not by the (possibly stale) project-name snapshot in session metadata.
pub fn project_for_worktree_path<'a>(
    projects: &'a [Project],
    wt_path: &str,
) -> Option<(usize, &'a Project)> {
    let wt = Path::new(wt_path.trim_end_matches('/'));

    if let Some(hit) = projects
        .iter()
        .enumerate()
        .find(|(_, p)| Path::new(p.path.trim_end_matches('/')) == wt)
    {
        return Some(hit);
    }

    if let Ok(root) = crate::git::worktrees_root() {
        if let Some(hit) = projects
            .iter()
            .enumerate()
            .find(|(_, p)| wt.starts_with(root.join(p.worktree_dir())))
        {
            return Some(hit);
        }
    }

    projects
        .iter()
        .enumerate()
        .filter(|(_, p)| wt.starts_with(Path::new(p.path.trim_end_matches('/'))))
        .max_by_key(|(_, p)| p.path.trim_end_matches('/').len())
}

/// Metadata only, one-shot startup pass — must never run on a render path.
pub fn adopt_orphaned_worktree_dirs(projects: &mut [Project]) -> usize {
    let root = match crate::git::worktrees_root() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "worktree adoption skipped: no worktrees root");
            return 0;
        }
    };
    adopt_orphaned_worktree_dirs_in(&root, projects, crate::git::worktree_owner_repo)
}

/// Testable core of [`adopt_orphaned_worktree_dirs`], with the root and owner oracle injected.
fn adopt_orphaned_worktree_dirs_in(
    root: &Path,
    projects: &mut [Project],
    owner_repo: impl Fn(&str) -> Option<PathBuf>,
) -> usize {
    let owned: Vec<String> = projects
        .iter()
        .map(|p| p.worktree_dir().to_string())
        .collect();

    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };

    // Collected first so a project claimed by two candidate dirs is detected, not won by read_dir order.
    let mut claims: Vec<(String, usize)> = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || !entry.path().is_dir() {
            continue;
        }
        if owned.contains(&name) {
            continue;
        }
        if let Some(idx) = resolve_candidate_owner(&entry.path(), projects, &owner_repo, &name) {
            if projects[idx].worktree_dir.is_some() {
                tracing::warn!(
                    dir = %name,
                    project = %projects[idx].name,
                    pinned = %projects[idx].worktree_dir(),
                    "not adopting orphaned worktree dir: project already has a pinned worktree dir"
                );
                continue;
            }
            claims.push((name, idx));
        }
    }

    let mut adopted = 0;
    for (dir, idx) in &claims {
        if claims.iter().any(|(d, i)| i == idx && d != dir) {
            let others: Vec<&str> = claims
                .iter()
                .filter(|(_, i)| i == idx)
                .map(|(d, _)| d.as_str())
                .collect();
            tracing::warn!(
                project = %projects[*idx].name,
                candidates = ?others,
                "not adopting orphaned worktree dirs: two directories claim the same project"
            );
            continue;
        }
        projects[*idx].worktree_dir = Some(dir.clone());
        tracing::info!(
            dir = %dir,
            project = %projects[*idx].name,
            "adopted orphaned worktree dir onto its owning project"
        );
        adopted += 1;
    }
    adopted
}

/// The single project every worktree inside `candidate` belongs to, or `None` if not unanimous.
fn resolve_candidate_owner(
    candidate: &Path,
    projects: &[Project],
    owner_repo: &impl Fn(&str) -> Option<PathBuf>,
    dir_name: &str,
) -> Option<usize> {
    let entries = match fs::read_dir(candidate) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(dir = %dir_name, error = %e, "not adopting orphaned worktree dir: unreadable");
            return None;
        }
    };
    let mut resolved: Option<usize> = None;
    for entry in entries.flatten() {
        let wt = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') || !wt.is_dir() {
            continue;
        }
        let Some(wt_str) = wt.to_str() else { continue };
        let idx = owner_repo(wt_str).and_then(|repo| {
            projects
                .iter()
                .position(|p| same_dir(&repo, Path::new(&p.path)))
        });
        match idx {
            None => {
                tracing::warn!(
                    dir = %dir_name,
                    worktree = %wt.display(),
                    "not adopting orphaned worktree dir: worktree resolves to no known project"
                );
                return None;
            }
            Some(i) => match resolved {
                Some(prev) if prev != i => {
                    tracing::warn!(
                        dir = %dir_name,
                        first = %projects[prev].name,
                        second = %projects[i].name,
                        "not adopting orphaned worktree dir: worktrees resolve to different projects"
                    );
                    return None;
                }
                _ => resolved = Some(i),
            },
        }
    }
    if resolved.is_none() {
        tracing::warn!(
            dir = %dir_name,
            "not adopting orphaned worktree dir: no worktree inside it resolved to a project"
        );
    }
    resolved
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let trimmed = Path::new(p.to_string_lossy().trim_end_matches('/')).to_path_buf();
        fs::canonicalize(&trimmed).unwrap_or(trimmed)
    };
    norm(a) == norm(b)
}

/// The diff viewer's list layout. `Unified` is the default to match plain `git diff`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffMode {
    #[default]
    Unified,
    Split,
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
    /// Logical pixels; `None` falls back to the default `RAIL_W`.
    #[serde(default)]
    pub sidebar_width: Option<f32>,
    /// `false` = project → worktree → session tree, `true` = flat cross-project session list.
    #[serde(default)]
    pub rail_sessions: bool,
    #[serde(default)]
    pub onboarded: bool,
    /// Unix seconds; gates the periodic (24h) update check.
    #[serde(default)]
    pub last_update_check: Option<i64>,
    /// While the latest release equals this, no update notice is shown.
    #[serde(default)]
    pub skipped_version: Option<String>,
    /// None is treated as `true` (bypass enabled), preserving pre-existing upgrade behavior.
    #[serde(default)]
    pub dangerously_skip_permissions_enabled: Option<bool>,
    #[serde(default)]
    pub chrome_enabled: Option<bool>,
    /// None is treated as `true` (opt-out model).
    #[serde(default)]
    pub telemetry_enabled: Option<bool>,
    /// Keyed by `"{project}::{wt_path}"`; sessions absent here are appended after in current order.
    #[serde(default)]
    pub grid_order: Vec<String>,
    #[serde(default)]
    pub theme_follow_system: bool,
    #[serde(default)]
    pub theme_dark: Option<String>,
    #[serde(default)]
    pub theme_light: Option<String>,
    #[serde(default)]
    pub project_themes_enabled: bool,
    /// Most recent first, capped at 6 (see `push_recent_launch`).
    #[serde(default)]
    pub recent_launches: Vec<RecentLaunch>,
    #[serde(default)]
    pub diff_mode: DiffMode,
}

impl Store {
    /// Callers MUST use the yielded index, never re-`enumerate` — `.enumerate()` must stay BEFORE `.filter()`.
    pub fn active_projects(&self) -> impl Iterator<Item = (usize, &Project)> {
        self.projects
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.archived)
    }

    pub fn archived_projects(&self) -> impl Iterator<Item = (usize, &Project)> {
        self.projects.iter().enumerate().filter(|(_, p)| p.archived)
    }

    pub fn archived_count(&self) -> usize {
        self.archived_projects().count()
    }
}

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

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::agent::Agent;

    #[test]
    fn write_atomic_no_tmp_residue_and_full_content() {
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
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_serde_round_trip() {
        let original = Store {
            projects: vec![
                Project {
                    name: "myapp".into(),
                    path: "/home/user/myapp".into(),
                    scripts: ProjectScripts::default(),
                    theme: None,
                    archived: false,
                    worktree_dir: None,
                },
                Project {
                    name: "other".into(),
                    path: "/tmp/other".into(),
                    scripts: ProjectScripts::default(),
                    theme: Some("dracula".into()),
                    archived: true,
                    worktree_dir: None,
                },
            ],
            default_agent: Some(Agent::Claude),
            theme: Some("dark".into()),
            tmux_enabled: Some(true),
            ui_zoom: Some(1.25),
            sidebar_width: Some(360.0),
            rail_sessions: true,
            onboarded: true,
            last_update_check: None,
            skipped_version: None,
            dangerously_skip_permissions_enabled: Some(false),
            chrome_enabled: Some(true),
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
            diff_mode: DiffMode::Split,
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
        assert_eq!(recovered.diff_mode, DiffMode::Split);
        assert_eq!(recovered.projects[1].theme.as_deref(), Some("dracula"));
        assert!(
            !recovered.projects[0].archived,
            "active project must round-trip as active"
        );
        assert!(
            recovered.projects[1].archived,
            "archived project must round-trip as archived"
        );
        assert_eq!(recovered.recent_launches, original.recent_launches);
    }

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

    #[test]
    fn store_without_chrome_key_parses_as_none() {
        let json = r#"{"projects":[],"dangerously_skip_permissions_enabled":true}"#;
        let store: Store = serde_json::from_str(json).expect("deserialize legacy store");
        assert!(store.chrome_enabled.is_none());
        assert_eq!(store.dangerously_skip_permissions_enabled, Some(true));
    }

    #[test]
    fn chrome_enabled_round_trips_through_save_and_load() {
        let _lock = CONFIG_DIR_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let dir = std::env::temp_dir().join(format!(
            "grove_test_chrome_enabled_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        let _guard = EnvVarGuard::set(CONFIG_DIR_ENV, dir.to_str().expect("utf8 path"));

        let store = Store {
            chrome_enabled: Some(true),
            ..Store::default()
        };
        save(&store).expect("save");

        let recovered = load().expect("load");
        assert_eq!(recovered.chrome_enabled, Some(true));

        let _ = fs::remove_dir_all(&dir);
    }

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
        let store: Store = serde_json::from_str("{}").expect("deserialize");
        assert!(store.grid_order.is_empty());
    }

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

    // `std::env::set_var` mutates process-global state; every test setting `CONFIG_DIR_ENV` (here and in session_meta.rs) serializes behind this shared mutex.
    pub(crate) static CONFIG_DIR_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Restores (or removes) the env var on drop, even on panic.
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

    #[test]
    fn config_dir_ignores_empty_override() {
        let _lock = CONFIG_DIR_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _guard = EnvVarGuard::set(CONFIG_DIR_ENV, "");

        let resolved = config_dir().expect("config_dir");
        assert_ne!(resolved.as_os_str(), "");
    }

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

        let dir = resolve_config_dir(&base).expect("resolve_config_dir");

        assert_eq!(dir, base.join("grove"));
        assert!(!legacy.exists(), "legacy dir must still be migrated away");
        assert!(dir.join("projects.json").exists());

        let _ = fs::remove_dir_all(&base);
    }

    fn store_with_archived_at(n: usize, archived: &[usize]) -> Store {
        Store {
            projects: (0..n)
                .map(|i| Project {
                    name: format!("p{i}"),
                    path: format!("/tmp/p{i}"),
                    scripts: ProjectScripts::default(),
                    theme: None,
                    archived: archived.contains(&i),
                    worktree_dir: None,
                })
                .collect(),
            ..Store::default()
        }
    }

    #[test]
    fn legacy_projects_without_archived_key_default_to_active() {
        let json = r#"{
            "projects": [
                { "name": "myapp", "path": "/home/user/myapp", "scripts": {} },
                { "name": "other", "path": "/tmp/other", "scripts": {} }
            ],
            "onboarded": true
        }"#;
        assert!(
            !json.contains("archived"),
            "the legacy fixture must not mention archived at all"
        );

        let store: Store = serde_json::from_str(json).expect("deserialize legacy store");

        assert_eq!(store.projects.len(), 2);
        for p in &store.projects {
            assert!(
                !p.archived,
                "legacy project {} must default to active, not archived",
                p.name
            );
        }
    }

    #[test]
    fn archived_survives_save_and_load() {
        let _lock = CONFIG_DIR_ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let dir = std::env::temp_dir().join(format!(
            "grove_test_archived_round_trip_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        let _guard = EnvVarGuard::set(CONFIG_DIR_ENV, dir.to_str().expect("utf8 path"));

        let store = store_with_archived_at(3, &[1]);
        save(&store).expect("save");

        let recovered = load().expect("load");
        let flags: Vec<bool> = recovered.projects.iter().map(|p| p.archived).collect();
        assert_eq!(
            flags,
            vec![false, true, false],
            "archived flags must match by position after save+load"
        );

        let raw = fs::read_to_string(config_path().expect("config_path")).expect("read raw json");
        assert!(
            raw.contains("\"archived\""),
            "the archived project must write an \"archived\" key: {raw}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn archived_false_is_omitted_from_serialized_json() {
        let store = store_with_archived_at(3, &[]);
        let json = serde_json::to_string_pretty(&store).expect("serialize");
        assert!(
            !json.contains("archived"),
            "archived: false must be skipped during serialization: {json}"
        );
    }

    #[test]
    fn non_bool_archived_value_returns_parse_error() {
        let json = r#"{"projects":[{"name":"a","path":"/tmp/a","scripts":{},"archived":"yes"}]}"#;
        let result: Result<Store, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "a non-bool archived value must fail to parse rather than being coerced"
        );
    }

    #[test]
    fn active_projects_yields_true_indices_with_archived_at_front() {
        let store = store_with_archived_at(5, &[0, 1]);
        let idx: Vec<usize> = store.active_projects().map(|(i, _)| i).collect();
        assert_eq!(
            idx,
            vec![2, 3, 4],
            "active_projects must yield TRUE indices into store.projects, not renumbered ones"
        );
    }

    #[test]
    fn active_projects_yields_true_indices_when_interleaved() {
        let store = store_with_archived_at(5, &[1, 3]);
        let idx: Vec<usize> = store.active_projects().map(|(i, _)| i).collect();
        assert_eq!(
            idx,
            vec![0, 2, 4],
            "interleaved archived projects must not shift the surviving indices"
        );
    }

    #[test]
    fn active_projects_handles_tail_single_and_none_archived() {
        let tail = store_with_archived_at(4, &[3]);
        assert_eq!(
            tail.active_projects().map(|(i, _)| i).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "archived tail project must simply be dropped"
        );

        let middle = store_with_archived_at(3, &[1]);
        assert_eq!(
            middle.active_projects().map(|(i, _)| i).collect::<Vec<_>>(),
            vec![0, 2],
            "a single archived project must leave the others' indices intact"
        );

        let none = store_with_archived_at(4, &[]);
        assert_eq!(
            none.active_projects().map(|(i, _)| i).collect::<Vec<_>>(),
            (0..4).collect::<Vec<_>>(),
            "with nothing archived, active_projects must be equivalent to plain enumerate()"
        );
    }

    #[test]
    fn all_archived_yields_empty_iterator_and_correct_counts() {
        let store = store_with_archived_at(3, &[0, 1, 2]);
        assert_eq!(store.active_projects().count(), 0);
        assert!(store.active_projects().next().is_none());
        assert_eq!(store.archived_count(), 3);
        assert_eq!(
            store
                .archived_projects()
                .map(|(i, _)| i)
                .collect::<Vec<_>>(),
            vec![0, 1, 2],
            "archived_projects must also yield TRUE indices"
        );
    }

    #[test]
    fn archived_is_project_level_not_worktree_level() {
        let project = Project {
            name: "myapp".into(),
            path: "/home/user/myapp".into(),
            scripts: ProjectScripts::default(),
            theme: None,
            archived: true,
            worktree_dir: None,
        };
        let json = serde_json::to_string(&project).expect("serialize project");
        let recovered: Project = serde_json::from_str(&json).expect("deserialize project");

        assert!(
            recovered.archived,
            "archived flag must survive a round trip"
        );
        assert_eq!(
            json.matches("\"archived\"").count(),
            1,
            "exactly one archived key per project — no per-worktree variant: {json}"
        );
    }

    fn project(name: &str, path: &str) -> Project {
        Project {
            name: name.to_string(),
            path: path.to_string(),
            scripts: ProjectScripts::default(),
            theme: None,
            archived: false,
            worktree_dir: None,
        }
    }

    #[test]
    fn project_for_worktree_path_exact_match() {
        let projects = vec![project("myapp", "/home/user/myapp/")];
        let (idx, p) =
            project_for_worktree_path(&projects, "/home/user/myapp").expect("exact match");
        assert_eq!(idx, 0);
        assert_eq!(p.name, "myapp");
    }

    #[test]
    fn project_for_worktree_path_grove_managed_worktree() {
        let root = crate::git::worktrees_root().expect("worktrees_root");
        let projects = vec![project("myapp", "/completely/unrelated/path")];
        let wt = root.join("myapp").join("feature-x");
        let (idx, p) = project_for_worktree_path(&projects, wt.to_str().expect("utf8 path"))
            .expect("grove-managed match");
        assert_eq!(idx, 0);
        assert_eq!(p.name, "myapp");
    }

    #[test]
    fn project_for_worktree_path_nested_project_longest_prefix_wins() {
        let projects = vec![
            project("SIP-ROOT", "/globus/code"),
            project("physician-portal", "/globus/code/physician-portal"),
        ];
        let (idx, p) =
            project_for_worktree_path(&projects, "/globus/code/physician-portal/feature-x")
                .expect("nested match");
        assert_eq!(p.name, "physician-portal");
        assert_eq!(idx, 1);
    }

    #[test]
    fn project_for_worktree_path_rejects_false_string_prefix() {
        let projects = vec![project("SIP-ROOT", "/globus/code")];
        assert!(
            project_for_worktree_path(&projects, "/globus/codebase/some-worktree").is_none(),
            "/globus/codebase must not be treated as nested under /globus/code"
        );
    }

    #[test]
    fn project_for_worktree_path_no_match_returns_none() {
        let projects = vec![project("myapp", "/home/user/myapp")];
        assert!(project_for_worktree_path(&projects, "/completely/unrelated").is_none());
    }

    #[test]
    fn worktree_dir_falls_back_to_name_and_honours_pin() {
        let mut p = project("myapp", "/home/user/myapp");
        assert_eq!(
            p.worktree_dir(),
            "myapp",
            "an unpinned project's worktree dir must be its name"
        );
        p.worktree_dir = Some("old-name".into());
        p.name = "NewName".into();
        assert_eq!(
            p.worktree_dir(),
            "old-name",
            "a pinned worktree dir must survive a rename of the display name"
        );
    }

    #[test]
    fn project_for_worktree_path_uses_pinned_dir_after_rename() {
        let root = crate::git::worktrees_root().expect("worktrees_root");
        let mut p = project("SIP-ROOT", "/completely/unrelated/path");
        p.worktree_dir = Some("careconvoy-ai-web".into());
        let projects = vec![p];

        let wt = root
            .join("careconvoy-ai-web")
            .join("super-user-segregation");
        let (idx, hit) = project_for_worktree_path(&projects, wt.to_str().expect("utf8 path"))
            .expect("pinned-dir match");
        assert_eq!(idx, 0);
        assert_eq!(hit.name, "SIP-ROOT");

        let under_new_name = root.join("SIP-ROOT").join("super-user-segregation");
        assert!(
            project_for_worktree_path(&projects, under_new_name.to_str().expect("utf8 path"))
                .is_none(),
            "a pinned project must not also claim <worktrees_root>/<new name>/..."
        );
    }

    #[test]
    fn worktree_dir_none_is_omitted_and_some_round_trips() {
        let unpinned = project("myapp", "/home/user/myapp");
        let json = serde_json::to_string(&unpinned).expect("serialize");
        assert!(
            !json.contains("worktree_dir"),
            "worktree_dir: None must be skipped during serialization: {json}"
        );

        let mut pinned = project("NewName", "/home/user/myapp");
        pinned.worktree_dir = Some("old-name".into());
        let json = serde_json::to_string(&pinned).expect("serialize");
        let recovered: Project = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(recovered.worktree_dir.as_deref(), Some("old-name"));

        let legacy: Project =
            serde_json::from_str(r#"{"name":"a","path":"/tmp/a","scripts":{}}"#).expect("legacy");
        assert!(legacy.worktree_dir.is_none());
        assert_eq!(legacy.worktree_dir(), "a");
    }

    #[test]
    fn pin_worktree_dir_on_rename_pins_once_and_only_once() {
        let mut p = project("careconvoy-ai-web", "/globus/code");
        pin_worktree_dir_on_rename(&mut p, "careconvoy-ai-web");
        assert_eq!(
            p.worktree_dir.as_deref(),
            Some("careconvoy-ai-web"),
            "the first rename must pin the dir at the OLD name"
        );

        p.name = "SIP-ROOT".into();
        pin_worktree_dir_on_rename(&mut p, "SIP-ROOT");
        assert_eq!(
            p.worktree_dir.as_deref(),
            Some("careconvoy-ai-web"),
            "an already-pinned worktree dir must never be re-pinned"
        );
    }

    fn worktree_fixture(tag: &str, layout: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "grove_test_adopt_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        for (dir, wt) in layout {
            fs::create_dir_all(root.join(dir).join(wt)).expect("create fixture worktree");
        }
        root
    }

    /// Maps a worktree path to an owning repo by its LAST path component, standing in for `git rev-parse --git-common-dir`.
    fn oracle(map: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<PathBuf> {
        move |wt: &str| {
            let leaf = Path::new(wt).file_name()?.to_string_lossy().to_string();
            map.iter()
                .find(|(name, _)| *name == leaf)
                .map(|(_, repo)| PathBuf::from(*repo))
        }
    }

    #[test]
    fn adopt_pins_unclaimed_dir_onto_its_unanimous_owner() {
        let root = worktree_fixture("happy", &[("careconvoy-ai-web", "wt-a"), ("other", "kept")]);
        let mut projects = vec![
            project("SIP-ROOT", "/globus/code"),
            project("other", "/globus/other"),
        ];

        let adopted = adopt_orphaned_worktree_dirs_in(
            &root,
            &mut projects,
            oracle(&[("wt-a", "/globus/code")]),
        );

        assert_eq!(adopted, 1, "exactly one directory was adoptable");
        assert_eq!(
            projects[0].worktree_dir.as_deref(),
            Some("careconvoy-ai-web"),
            "the orphaned dir must be pinned onto the project git says owns it"
        );
        assert!(
            projects[1].worktree_dir.is_none(),
            "an uninvolved project must be left alone"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn adopt_refuses_when_worktrees_resolve_to_different_projects() {
        let root = worktree_fixture("mixed", &[("mystery", "wt-a"), ("mystery", "wt-b")]);
        let mut projects = vec![
            project("alpha", "/globus/alpha"),
            project("beta", "/globus/beta"),
        ];

        let adopted = adopt_orphaned_worktree_dirs_in(
            &root,
            &mut projects,
            oracle(&[("wt-a", "/globus/alpha"), ("wt-b", "/globus/beta")]),
        );

        assert_eq!(adopted, 0);
        assert!(
            projects.iter().all(|p| p.worktree_dir.is_none()),
            "an ambiguous directory must pin nothing at all"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn adopt_refuses_when_two_dirs_claim_the_same_project() {
        let root = worktree_fixture("dupe", &[("old-name", "wt-a"), ("older-name", "wt-b")]);
        let mut projects = vec![project("alpha", "/globus/alpha")];

        let adopted = adopt_orphaned_worktree_dirs_in(
            &root,
            &mut projects,
            oracle(&[("wt-a", "/globus/alpha"), ("wt-b", "/globus/alpha")]),
        );

        assert_eq!(adopted, 0);
        assert!(
            projects[0].worktree_dir.is_none(),
            "neither contender may win when two dirs claim one project"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn adopt_refuses_unresolvable_and_empty_candidates() {
        let root = worktree_fixture("unresolved", &[("mystery", "wt-a")]);
        fs::create_dir_all(root.join("empty")).expect("create empty candidate");
        let mut projects = vec![project("alpha", "/globus/alpha")];

        let adopted = adopt_orphaned_worktree_dirs_in(&root, &mut projects, oracle(&[]));

        assert_eq!(adopted, 0);
        assert!(projects[0].worktree_dir.is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn adopt_refuses_when_project_is_already_pinned() {
        let root = worktree_fixture("pinned", &[("even-older", "wt-a")]);
        let mut projects = vec![project("alpha", "/globus/alpha")];
        projects[0].worktree_dir = Some("old-name".into());

        let adopted = adopt_orphaned_worktree_dirs_in(
            &root,
            &mut projects,
            oracle(&[("wt-a", "/globus/alpha")]),
        );

        assert_eq!(adopted, 0);
        assert_eq!(
            projects[0].worktree_dir.as_deref(),
            Some("old-name"),
            "an existing pin must not be overwritten by adoption"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn adopt_ignores_dotfiles_and_plain_files() {
        let root = worktree_fixture("noise", &[(".hidden", "wt-a")]);
        fs::write(root.join(".DS_Store"), b"junk").expect("write junk file");
        fs::write(root.join("stray.txt"), b"junk").expect("write junk file");
        let mut projects = vec![project("alpha", "/globus/alpha")];

        let adopted = adopt_orphaned_worktree_dirs_in(
            &root,
            &mut projects,
            oracle(&[("wt-a", "/globus/alpha")]),
        );

        assert_eq!(adopted, 0, "hidden entries and files are not candidates");
        assert!(projects[0].worktree_dir.is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn adopt_on_missing_root_is_a_no_op() {
        let root = std::env::temp_dir().join("grove_test_adopt_absent_root_does_not_exist");
        let _ = fs::remove_dir_all(&root);
        let mut projects = vec![project("alpha", "/globus/alpha")];
        assert_eq!(
            adopt_orphaned_worktree_dirs_in(&root, &mut projects, oracle(&[])),
            0
        );
        assert!(projects[0].worktree_dir.is_none());
    }
}
