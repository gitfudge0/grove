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
    /// Archived: hidden from the main projects list but still present in
    /// `Store::projects` (see `Store::active_projects`). Skipped when false so
    /// `projects.json` stays clean for the overwhelmingly common active case,
    /// matching how `ProjectScripts`' fields are handled above.
    #[serde(default, skip_serializing_if = "is_false")]
    pub archived: bool,
    /// The directory component under `git::worktrees_root()` that holds this
    /// project's grove-managed worktrees. Pinned on the FIRST rename and never
    /// changed afterwards, so renaming a project cannot orphan worktrees that
    /// already exist on disk. `None` means "same as `name`" — the state every
    /// project starts in and most stay in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_dir: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Project {
    /// The directory this project's grove-managed worktrees live under, inside
    /// `git::worktrees_root()`. Falls back to the display name for every
    /// project that has never been renamed (`worktree_dir == None`), which is
    /// the layout `git::add_worktree` has always created.
    pub fn worktree_dir(&self) -> &str {
        self.worktree_dir.as_deref().unwrap_or(&self.name)
    }
}

/// Freeze `project.worktree_dir` at `old_name` if it is not already pinned —
/// the single decision a project rename has to make about its worktree
/// directory.
///
/// Called from the store mutation that applies a rename, BEFORE/while
/// `name` changes. An unrenamed project carries `None` (dir == name); the
/// first rename pins the dir at the name the directory on disk was actually
/// created under; every later rename leaves that pin alone. Nothing on disk is
/// moved, created or deleted — this is metadata only.
pub fn pin_worktree_dir_on_rename(project: &mut Project, old_name: &str) {
    if project.worktree_dir.is_none() {
        project.worktree_dir = Some(old_name.to_string());
    }
}

/// Resolves the project that owns a worktree path, by path rather than by
/// the (possibly stale) project-name snapshot session metadata records.
///
/// Persisted session metadata (`crate::session_meta`) stores the project as
/// a NAME snapshot taken when the session launched; that name goes stale the
/// moment the project is renamed, while `wt_path` — an actual filesystem
/// path — never does. A lifecycle-script lookup that keys the SCRIPT off the
/// stale name while the CWD it runs in comes from `wt_path` can end up
/// pointing at two different projects, running one project's script inside
/// another's worktree. Resolving the project from `wt_path` through this
/// function keeps "which script" and "where it runs" anchored to the same
/// project.
///
/// Resolution order (first match wins):
/// 1. Exact match: `project.path == wt_path` (trailing `/` trimmed from
///    both sides before comparing).
/// 2. Grove-managed worktree: `wt_path` lives under
///    `worktrees_root()/<project.worktree_dir()>/...` — the layout
///    `git::add_worktree` (`root.join(worktree_dir).join(name)`) actually
///    creates worktrees at. The directory key is the PINNED
///    `Project::worktree_dir` (falling back to `name` when unpinned), never
///    the live display name: keying off the mutable name meant a rename
///    orphaned every worktree directory the project already had.
/// 3. Native worktree under the project root: `project.path` is a
///    path-component prefix of `wt_path`. When several projects match by
///    prefix (a project nested inside its parent's directory tree), the
///    LONGEST `project.path` wins, so a nested project's own worktree is
///    attributed to it rather than to its parent.
///
/// Comparisons are by path COMPONENT (via `std::path::Path`), never by raw
/// string prefix: `/globus/code` must not match `/globus/codebase`.
///
/// Returns `None` when no project claims the path.
pub fn project_for_worktree_path<'a>(
    projects: &'a [Project],
    wt_path: &str,
) -> Option<(usize, &'a Project)> {
    let wt = Path::new(wt_path.trim_end_matches('/'));

    // 1. Exact match.
    if let Some(hit) = projects
        .iter()
        .enumerate()
        .find(|(_, p)| Path::new(p.path.trim_end_matches('/')) == wt)
    {
        return Some(hit);
    }

    // 2. Grove-managed worktree layout:
    //    <worktrees_root>/<project.worktree_dir()>/<name>.
    if let Ok(root) = crate::git::worktrees_root() {
        if let Some(hit) = projects
            .iter()
            .enumerate()
            .find(|(_, p)| wt.starts_with(root.join(p.worktree_dir())))
        {
            return Some(hit);
        }
    }

    // 3. Native worktree under the project root: longest-prefix match.
    projects
        .iter()
        .enumerate()
        .filter(|(_, p)| wt.starts_with(Path::new(p.path.trim_end_matches('/'))))
        .max_by_key(|(_, p)| p.path.trim_end_matches('/').len())
}

/// Adopt worktree directories under `git::worktrees_root()` that no project
/// currently claims, pinning each onto the project that genuinely owns its
/// worktrees.
///
/// This closes the gap left by the era when the directory was keyed off the
/// mutable display name: a project renamed BEFORE `Project::worktree_dir`
/// existed has a directory on disk named after its old name that nothing
/// matches any more. Rather than guess from the name, each candidate
/// directory's worktrees are handed to `git` (see
/// `git::worktree_owner_repo`), which reports the repository that actually
/// owns them; only a unanimous, unambiguous answer results in an adoption.
///
/// Metadata only: no directory is ever created, moved or removed. Returns the
/// number of projects whose `worktree_dir` was pinned, so the caller can log
/// and decide whether to persist.
///
/// COST: this walks a directory tree and spawns one `git` per worktree. It is
/// a ONE-SHOT startup pass and must never be called from a render path.
/// `project_for_worktree_path` deliberately stays pure and subprocess-free.
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

/// Testable core of [`adopt_orphaned_worktree_dirs`]: the worktrees root and
/// the owning-repo oracle are both injected, so tests can drive the whole
/// decision table against a `tempfile::TempDir` without a real `git` or the
/// user's real `~/.config/grove/worktrees`.
fn adopt_orphaned_worktree_dirs_in(
    root: &Path,
    projects: &mut [Project],
    owner_repo: impl Fn(&str) -> Option<PathBuf>,
) -> usize {
    let owned: Vec<String> = projects
        .iter()
        .map(|p| p.worktree_dir().to_string())
        .collect();

    // A machine that has never created a grove worktree has no root yet.
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };

    // dir name -> resolved project index, for the candidates that produced a
    // unanimous answer. Collected first so a project claimed by TWO candidate
    // directories can be detected and refused rather than won by whichever
    // `read_dir` happened to yield first.
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

/// The single project every worktree inside `candidate` belongs to, or `None`
/// when the answer is not unanimous (mixed owners, an owner that is not a
/// registered project, or no worktrees at all). Each refusal is logged.
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

/// Whether two paths name the same directory, compared as paths (canonicalized
/// where possible, trailing separators trimmed) rather than as strings.
fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let trimmed = Path::new(p.to_string_lossy().trim_end_matches('/')).to_path_buf();
        fs::canonicalize(&trimmed).unwrap_or(trimmed)
    };
    norm(a) == norm(b)
}

/// The diff viewer's list layout: side-by-side columns or one interleaved
/// column. `Unified` is the default so a first-run user sees the familiar
/// `git diff` shape.
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
    /// Sidebar width in logical pixels, set by dragging the divider. None falls
    /// back to the default `RAIL_W`.
    #[serde(default)]
    pub sidebar_width: Option<f32>,
    /// Which contents the left rail shows: `false` (the serde default) is the
    /// project → worktree → session tree, `true` the flat cross-project
    /// session list. Persisted next to `sidebar_width` because it is the same
    /// kind of state — a rail presentation choice that must round-trip.
    #[serde(default)]
    pub rail_sessions: bool,
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
    /// Whether launched Claude sessions get `--chrome` (the Claude in Chrome
    /// integration). None means unset; treated as `false`.
    #[serde(default)]
    pub chrome_enabled: Option<bool>,
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
    /// The diff viewer's persisted mode (Unified/Split). `#[serde(default)]`
    /// so old config files load into `DiffMode::Unified`.
    #[serde(default)]
    pub diff_mode: DiffMode,
}

impl Store {
    /// Projects visible in the UI, paired with their TRUE index into
    /// `self.projects`.
    ///
    /// Archived projects stay in `projects` so every index-keyed thing in the
    /// GUI (`proj_idx`, the per-project worktree caches, `Modal::ScriptsEditor`)
    /// stays valid. Callers MUST use the yielded index and never re-`enumerate`
    /// the filtered sequence — renumbering hands those callers the wrong
    /// project. Note the ordering below: `.enumerate()` comes BEFORE
    /// `.filter()`; swapping them renumbers the survivors from zero, which is
    /// the single bug this whole design exists to avoid.
    pub fn active_projects(&self) -> impl Iterator<Item = (usize, &Project)> {
        self.projects
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.archived)
    }

    /// Archived projects with their TRUE index, for the archived-projects list.
    /// Same `.enumerate()`-before-`.filter()` contract as `active_projects`.
    pub fn archived_projects(&self) -> impl Iterator<Item = (usize, &Project)> {
        self.projects.iter().enumerate().filter(|(_, p)| p.archived)
    }

    /// Count of archived projects, for the Settings row.
    pub fn archived_count(&self) -> usize {
        self.archived_projects().count()
    }
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
pub(crate) mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

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

    /// U4: a config file written before `chrome_enabled` existed must still
    /// parse, leaving the field `None` (= off).
    #[test]
    fn store_without_chrome_key_parses_as_none() {
        let json = r#"{"projects":[],"dangerously_skip_permissions_enabled":true}"#;
        let store: Store = serde_json::from_str(json).expect("deserialize legacy store");
        assert!(store.chrome_enabled.is_none());
        assert_eq!(store.dangerously_skip_permissions_enabled, Some(true));
    }

    /// U5: `chrome_enabled` survives a real `save()` -> `load()` round trip
    /// through `GROVE_CONFIG_DIR`-overridden on-disk config.
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
    //
    // `pub(crate)`: `session_meta.rs`'s tests also mutate this same
    // process-global env var and must serialize against these tests too, not
    // just against each other — a second, independent mutex over the same
    // global would not prevent interleaving between the two modules.
    pub(crate) static CONFIG_DIR_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

    // ── archived projects ────────────────────────────────────────────────

    /// Test helper: `n` projects named `p0..p{n-1}`, archived exactly at the
    /// given indices.
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

    /// S1: every existing user's `projects.json` predates `archived`. Such a
    /// file must still parse, with every project treated as active.
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

    /// S2: the archived flag survives a real `save()` -> `load()` cycle and is
    /// actually present in the bytes on disk.
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

    /// S3: pins the `skip_serializing_if` decision — an all-active store must
    /// not write `archived` at all, so upgrading never bloats projects.json.
    #[test]
    fn archived_false_is_omitted_from_serialized_json() {
        let store = store_with_archived_at(3, &[]);
        let json = serde_json::to_string_pretty(&store).expect("serialize");
        assert!(
            !json.contains("archived"),
            "archived: false must be skipped during serialization: {json}"
        );
    }

    /// S5: `archived` is a strict bool — a string value is a parse error, not a
    /// coerced truthy value.
    #[test]
    fn non_bool_archived_value_returns_parse_error() {
        let json = r#"{"projects":[{"name":"a","path":"/tmp/a","scripts":{},"archived":"yes"}]}"#;
        let result: Result<Store, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "a non-bool archived value must fail to parse rather than being coerced"
        );
    }

    /// H1: archived projects at the FRONT are the case a renumbering bug hides
    /// behind — a `.filter()`-before-`.enumerate()` implementation returns
    /// `[0, 1, 2]` here instead of the true indices.
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

    /// H2: same invariant with the archived projects interleaved.
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

    /// H3: tail-archived, single-archived, and none-archived shapes.
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

    /// H4: an entirely archived store yields an empty iterator without
    /// panicking, and the archived-side helpers report the full set.
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

    /// S6: guard against scope creep — `archived` lives on `Project` only, so a
    /// single project serializes exactly one `"archived"` key and nothing
    /// per-worktree was invented alongside it.
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

    // ── project_for_worktree_path ───────────────────────────────────────

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

    /// Rule 1: an exact `project.path == wt_path` match wins, trailing
    /// slashes trimmed on both sides.
    #[test]
    fn project_for_worktree_path_exact_match() {
        let projects = vec![project("myapp", "/home/user/myapp/")];
        let (idx, p) =
            project_for_worktree_path(&projects, "/home/user/myapp").expect("exact match");
        assert_eq!(idx, 0);
        assert_eq!(p.name, "myapp");
    }

    /// Rule 2: a worktree under `worktrees_root()/<project.name>/...` is
    /// attributed to that project even though `project.path` (the project's
    /// own checkout root) is unrelated to the worktree path — this is
    /// grove's own managed-worktree layout (`git::add_worktree`).
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

    /// Rule 3, nested case: a project registered at a path nested inside
    /// another project's root must win the prefix match over its parent —
    /// the longest `project.path` wins.
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

    /// Rule 3 must compare by path COMPONENT, not raw string prefix:
    /// `/globus/code` is not a prefix of `/globus/codebase`.
    #[test]
    fn project_for_worktree_path_rejects_false_string_prefix() {
        let projects = vec![project("SIP-ROOT", "/globus/code")];
        assert!(
            project_for_worktree_path(&projects, "/globus/codebase/some-worktree").is_none(),
            "/globus/codebase must not be treated as nested under /globus/code"
        );
    }

    /// No project claims the path at all: `None`, not a panic or a wrong
    /// fallback guess.
    #[test]
    fn project_for_worktree_path_no_match_returns_none() {
        let projects = vec![project("myapp", "/home/user/myapp")];
        assert!(project_for_worktree_path(&projects, "/completely/unrelated").is_none());
    }

    // ── worktree_dir: the rename-proof directory key ─────────────────────

    /// The accessor is the whole point of the `Option`: unpinned projects read
    /// as their name, pinned ones as the frozen directory.
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

    /// Rule 2 after a rename: the worktree lives under the PINNED directory,
    /// which no longer matches the project's display name at all. Keying rule 2
    /// off `name` (as it once did) orphans exactly this worktree.
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

        // And the NEW name must claim nothing: no such directory relationship
        // exists, so inventing one would re-introduce the split-brain the pin
        // exists to prevent.
        let under_new_name = root.join("SIP-ROOT").join("super-user-segregation");
        assert!(
            project_for_worktree_path(&projects, under_new_name.to_str().expect("utf8 path"))
                .is_none(),
            "a pinned project must not also claim <worktrees_root>/<new name>/..."
        );
    }

    /// `projects.json` must stay clean for the overwhelmingly common unpinned
    /// case, and a pin must survive a round trip.
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

        // A config file written before the field existed loads as unpinned.
        let legacy: Project =
            serde_json::from_str(r#"{"name":"a","path":"/tmp/a","scripts":{}}"#).expect("legacy");
        assert!(legacy.worktree_dir.is_none());
        assert_eq!(legacy.worktree_dir(), "a");
    }

    /// The pinning rule: the FIRST rename freezes the directory at the name it
    /// was created under; later renames must leave that pin alone, or the
    /// second rename would orphan the directory the first one saved.
    #[test]
    fn pin_worktree_dir_on_rename_pins_once_and_only_once() {
        let mut p = project("careconvoy-ai-web", "/globus/code");
        pin_worktree_dir_on_rename(&mut p, "careconvoy-ai-web");
        assert_eq!(
            p.worktree_dir.as_deref(),
            Some("careconvoy-ai-web"),
            "the first rename must pin the dir at the OLD name"
        );

        // Second rename: the pin must not follow the intermediate name.
        p.name = "SIP-ROOT".into();
        pin_worktree_dir_on_rename(&mut p, "SIP-ROOT");
        assert_eq!(
            p.worktree_dir.as_deref(),
            Some("careconvoy-ai-web"),
            "an already-pinned worktree dir must never be re-pinned"
        );
    }

    // ── adoption of already-orphaned worktree directories ────────────────
    //
    // These drive `adopt_orphaned_worktree_dirs_in` with an injected root and
    // an injected owning-repo oracle, so they need neither a real `git` nor
    // the user's real `~/.config/grove/worktrees`, and they never touch
    // `GROVE_CONFIG_DIR` (which `worktrees_root()` does not consult anyway).

    /// Builds `<tmp>/<dir>/<wt>` for every `(dir, wt)` pair and returns the
    /// root. Caller removes the tree.
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

    /// Oracle that maps a worktree path to an owning repo by the LAST path
    /// component, standing in for what `git rev-parse --git-common-dir` reports.
    fn oracle(map: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<PathBuf> {
        move |wt: &str| {
            let leaf = Path::new(wt).file_name()?.to_string_lossy().to_string();
            map.iter()
                .find(|(name, _)| *name == leaf)
                .map(|(_, repo)| PathBuf::from(*repo))
        }
    }

    /// Happy path: a directory named after the project's OLD name, whose
    /// worktrees all belong to one project, is adopted onto that project.
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
        // `other/` is already that project's own dir, so it is not a candidate
        // and needs no oracle answer at all.
        let _ = fs::remove_dir_all(&root);
    }

    /// Refusal: the candidate's worktrees belong to two different projects, so
    /// there is no single right answer — leave the state visible instead.
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

    /// Refusal: two candidate directories both resolve to the same project.
    /// First-one-wins is not acceptable — skip BOTH and leave it visible.
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

    /// Refusal: a worktree whose owning repo is not a registered project at
    /// all, and an empty candidate directory — neither yields an owner.
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

    /// Refusal: the resolved project is already pinned, so its directory was
    /// decided once and must not be silently repointed.
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

    /// Dotfiles (`.DS_Store` really is in the user's worktrees root) and plain
    /// files must never be treated as candidate directories.
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

    /// A worktrees root that does not exist yet (a machine that has never
    /// created a grove worktree) is a no-op, not an error.
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
