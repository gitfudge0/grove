//! Worktrees, the per-project worktree cache, and the two background git jobs
//! the sidebar depends on.
//!
//! Everything here is `git`-shaped and therefore slow: `git worktree list` and
//! `git status` are subprocesses, and `is_repo` stats the filesystem. None of
//! it may happen during a repaint. The iced build learned this the hard way
//! (`src/gui/state.rs:39-42`, `src/gui/view/sidebar.rs:26-31`) and this is the
//! port of the machinery it grew:
//!
//! * `worktrees_for_project` / `ensure_wt_cached` / `rebuild_wt_cache` +
//!   the **generation guard** (`update/mod.rs:1185`, `:1326`, `:1351`),
//! * the 5s `is_repo` memo (`view/sidebar.rs:26-54`),
//! * the 5s git-state poll with its in-flight guard (`update/mod.rs:1251`) and
//!   `visible_worktree_paths` (`:1308`).
//!
//! grove-core supplies every git call as-is (Global Constraint 3 candidate 2):
//! `git::{is_repo, list_worktrees, worktree_git_state, git_state_suffix}`.

// The view-facing readers land in Task 5.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Context, Task};
use grove_core::git::{self, Worktree, WorktreeGitState};
use grove_core::storage::Store;

use crate::entities::session_registry::SessionRegistry;
use crate::entities::workspace_state::{
    SnapshotProject, SnapshotWorktree, TreeSnapshot, WorkspaceState,
};
use crate::views::rows::path_basename;

/// How long a memoized `git::is_repo` answer stays good
/// (`src/gui/view/sidebar.rs:32`). A project gaining or losing its `.git` is
/// rare and not something the app drives.
const IS_REPO_TTL: Duration = Duration::from_secs(5);

/// Throttle window for the git-state poll (`update/mod.rs:1254`).
const GIT_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Degraded cadence while the window is unfocused — still catches up
/// eventually rather than stopping outright.
const GIT_POLL_INTERVAL_UNFOCUSED: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct ProjectTree {
    /// The **active** project's worktrees (`App::worktrees`).
    worktrees: Vec<Worktree>,
    /// Every other project's worktrees, keyed by TRUE project index.
    wt_cache: HashMap<usize, Vec<Worktree>>,
    /// Bumped by every change to the project list's *shape*. A sweep stamped
    /// with an older generation is discarded wholesale: a sweep launched
    /// before an add/remove/archive describes an index space that no longer
    /// exists, and folding it in would attach one project's worktrees to
    /// another (`update/mod.rs:1341-1350`).
    generation: u64,
    is_repo_memo: HashMap<String, (bool, Instant)>,
    git_state: HashMap<String, WorktreeGitState>,
    last_git_poll: Option<Instant>,
    /// Shared with the running poll so a slow `git status` sweep is *skipped*
    /// rather than overlapped (`update/mod.rs:1266-1278`).
    git_poll_inflight: Arc<AtomicBool>,
    /// Dropping a `Task` cancels it, so these fields *are* the running jobs;
    /// overwriting one supersedes the previous run.
    git_poll: Option<Task<()>>,
    sweep: Option<Task<()>>,
}

impl ProjectTree {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// `update/mod.rs:1185-1193`. A cache miss is an empty slice, never a
    /// panic: the tree simply renders that project with no worktrees until the
    /// sweep lands.
    #[must_use]
    pub fn worktrees_for_project(&self, proj: usize, active_proj: usize) -> &[Worktree] {
        if proj == active_proj {
            &self.worktrees
        } else {
            self.wt_cache.get(&proj).map_or(&[][..], Vec::as_slice)
        }
    }

    pub fn set_active_worktrees(&mut self, worktrees: Vec<Worktree>) {
        self.worktrees = worktrees;
    }

    /// `switch_active_project`'s cache hand-off (`update/mod.rs:1121-1130`):
    /// the outgoing project's list moves into the cache so the tree can still
    /// render its children, and the incoming project's stale entry is dropped
    /// in favour of a fresh inline read. Selection itself belongs to
    /// [`WorkspaceState::select_project`].
    pub fn switch_active_project(&mut self, old: usize, new: usize, new_path: &str) {
        if old == new {
            return;
        }
        let outgoing = std::mem::take(&mut self.worktrees);
        self.wt_cache.insert(old, outgoing);
        self.wt_cache.remove(&new);
        self.worktrees = git::list_worktrees(new_path);
    }

    /// `update/mod.rs:1326-1334`.
    pub fn ensure_wt_cached(&mut self, proj: usize, active_proj: usize, store: &Store) {
        if proj == active_proj || self.wt_cache.contains_key(&proj) {
            return;
        }
        if let Some(p) = store.projects.get(proj) {
            let wts = git::list_worktrees(&p.path);
            self.wt_cache.insert(proj, wts);
        }
    }

    /// The single invalidation point for the cache (`update/mod.rs:1351-1365`).
    /// Every path that changes the project list's shape — add, remove, archive,
    /// restore, onboarding — calls this, so any sweep already in flight is now
    /// stale.
    pub fn rebuild_wt_cache(&mut self) {
        self.wt_cache.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    /// Fold in an off-thread sweep, unless the generation moved while it ran.
    /// Returns whether the result was applied — the discard is the whole point
    /// of the guard, so it is observable.
    pub fn apply_sweep(&mut self, generation: u64, swept: HashMap<usize, Vec<Worktree>>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.wt_cache.extend(swept);
        true
    }

    /// Sweep every non-active project's worktrees off the UI thread. Until the
    /// result lands, inactive projects render with no worktrees — exactly as
    /// they already do on a cold cache.
    pub fn sweep_wt_cache(&mut self, store: &Store, active_proj: usize, cx: &mut Context<Self>) {
        let generation = self.generation;
        let targets: Vec<(usize, String)> = store
            .active_projects()
            .filter(|(i, _)| *i != active_proj)
            .map(|(i, p)| (i, p.path.clone()))
            .collect();
        if targets.is_empty() {
            return;
        }
        self.sweep = Some(cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            let swept = cx
                .background_executor()
                .spawn(async move {
                    targets
                        .into_iter()
                        .map(|(i, path)| (i, git::list_worktrees(&path)))
                        .collect::<HashMap<usize, Vec<Worktree>>>()
                })
                .await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                if this.apply_sweep(generation, swept) {
                    cx.notify();
                }
            });
        }));
    }

    // ── is_repo memo ────────────────────────────────────────────────────

    /// `git::is_repo` memoized for [`IS_REPO_TTL`] (`view/sidebar.rs:40-54`).
    /// `now` is injected so the expiry is testable without sleeping.
    pub fn is_repo_cached(&mut self, path: &str, now: Instant) -> bool {
        if let Some((answer, at)) = self.is_repo_memo.get(path) {
            if now.duration_since(*at) < IS_REPO_TTL {
                return *answer;
            }
        }
        let answer = git::is_repo(path);
        self.is_repo_memo.insert(path.to_string(), (answer, now));
        answer
    }

    /// Test seam: seed the memo without touching the filesystem.
    #[cfg(test)]
    fn seed_is_repo(&mut self, path: &str, answer: bool, at: Instant) {
        self.is_repo_memo.insert(path.to_string(), (answer, at));
    }

    // ── the 5s git-state poll ───────────────────────────────────────────

    /// Rendered dirty/ahead/behind text per worktree path, for the flattened
    /// worktree rows. `git_state_suffix` returning `None` means "nothing worth
    /// showing", so that worktree simply has no suffix.
    #[must_use]
    pub fn git_suffixes(&self) -> HashMap<String, String> {
        self.git_state
            .iter()
            .filter_map(|(path, state)| git::git_state_suffix(state).map(|s| (path.clone(), s)))
            .collect()
    }

    /// Fold a completed poll in: fresh entries overwrite, failures **drop** the
    /// cached entry rather than leaving stale data on screen
    /// (`update/mod.rs:1288-1299`).
    pub fn apply_git_poll(&mut self, fresh: HashMap<String, WorktreeGitState>, stale: &[String]) {
        self.git_state.extend(fresh);
        for path in stale {
            self.git_state.remove(path);
        }
    }

    /// Whether the throttle window has elapsed (`update/mod.rs:1252-1259`).
    /// Stamps `last_git_poll` when it returns true, like the original. When
    /// `focused` is false the interval degrades to
    /// [`GIT_POLL_INTERVAL_UNFOCUSED`] rather than stopping outright, so an
    /// unfocused window still catches up occasionally.
    pub fn git_poll_due(&mut self, now: Instant, focused: bool) -> bool {
        let interval = if focused {
            GIT_POLL_INTERVAL
        } else {
            GIT_POLL_INTERVAL_UNFOCUSED
        };
        let due = self
            .last_git_poll
            .is_none_or(|t| now.duration_since(t) >= interval);
        if due {
            self.last_git_poll = Some(now);
        }
        due
    }

    /// Paths of every worktree currently rendered in the tree — every worktree
    /// of a non-collapsed **active** project (`update/mod.rs:1308-1324`).
    /// Archived projects are skipped here too, or `git status` keeps polling
    /// worktrees nothing renders.
    #[must_use]
    pub fn visible_worktree_paths(&self, store: &Store, ws: &WorkspaceState) -> Vec<String> {
        let mut paths = Vec::new();
        for (pi, _) in store.active_projects() {
            if ws.project_collapsed(pi) {
                continue;
            }
            paths.extend(
                self.worktrees_for_project(pi, ws.proj_idx())
                    .iter()
                    .map(|w| w.path.clone()),
            );
        }
        paths
    }

    /// The 5s poll, as its own background task rather than a tick branch
    /// (spec §4). Three behaviors, all load-bearing: the 5s throttle, the
    /// in-flight guard that **skips** rather than overlaps, and dropping —
    /// never staling — an entry whose `git` call failed.
    pub fn maybe_poll_git_state(&mut self, paths: Vec<String>, focused: bool, cx: &mut Context<Self>) {
        if !self.git_poll_due(Instant::now(), focused) || paths.is_empty() {
            return;
        }
        if self
            .git_poll_inflight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // Previous poll is still running — skip rather than overlap it.
            return;
        }
        let inflight = Arc::clone(&self.git_poll_inflight);
        self.git_poll = Some(cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            let (fresh, stale) = cx
                .background_executor()
                .spawn(async move {
                    let mut fresh = HashMap::new();
                    let mut stale = Vec::new();
                    for path in paths {
                        match git::worktree_git_state(&path) {
                            Some(state) => {
                                fresh.insert(path, state);
                            }
                            // Any failure (no repo, no upstream, git missing)
                            // degrades to "no signal".
                            None => stale.push(path),
                        }
                    }
                    (fresh, stale)
                })
                .await;
            let _ = this.update(cx, |this: &mut Self, cx| {
                this.apply_git_poll(fresh, &stale);
                cx.notify();
            });
            inflight.store(false, Ordering::Release);
        }));
    }

    // ── the snapshot the pure logic reads ───────────────────────────────

    /// Materialize the [`TreeSnapshot`] that [`WorkspaceState`]'s transitions
    /// and `views::rows::flatten` both read. One pass over the registry up
    /// front, so no row rescans the session list (`view/sidebar.rs:227-237`).
    #[must_use]
    pub fn snapshot(
        &mut self,
        store: &Store,
        registry: &SessionRegistry,
        active_proj: usize,
    ) -> TreeSnapshot {
        let now = Instant::now();
        let mut projects = Vec::new();
        for (pi, p) in store.active_projects() {
            let is_git = self.is_repo_cached(&p.path, now);
            let has_run = p
                .scripts
                .run
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty());
            let worktrees = if pi == active_proj {
                &self.worktrees
            } else {
                self.wt_cache.get(&pi).map_or(&[][..], Vec::as_slice)
            }
            .iter()
            .map(|w| SnapshotWorktree {
                // The main worktree shows the project name; every other one
                // shows its directory basename (`view/sidebar.rs:285-289`).
                name: if w.is_main {
                    p.name.clone()
                } else {
                    path_basename(&w.path)
                },
                sessions: registry.sessions_in_worktree(&w.path),
                path: w.path.clone(),
                branch: w.branch.clone(),
                is_main: w.is_main,
            })
            .collect();
            projects.push(SnapshotProject {
                idx: pi,
                name: p.name.clone(),
                is_git,
                has_run,
                worktrees,
                sessions: registry.by_project(&p.name),
            });
        }
        TreeSnapshot {
            projects,
            total_projects: store.projects.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grove_core::storage::Project;

    fn wt(path: &str, is_main: bool) -> Worktree {
        Worktree {
            path: path.to_string(),
            branch: "main".to_string(),
            mtime: None,
            is_main,
        }
    }

    fn project(name: &str, path: &str) -> Project {
        Project {
            name: name.to_string(),
            path: path.to_string(),
            scripts: grove_core::storage::ProjectScripts::default(),
            theme: None,
            archived: false,
        }
    }

    /// `update/mod.rs:1185-1193` — a cache miss is empty, not a panic.
    #[test]
    fn worktrees_for_project_reads_the_active_list_then_the_cache() {
        let mut tree = ProjectTree::new();
        tree.set_active_worktrees(vec![wt("/a", true)]);
        tree.wt_cache.insert(2, vec![wt("/g", true)]);
        assert_eq!(tree.worktrees_for_project(0, 0).len(), 1);
        assert_eq!(tree.worktrees_for_project(2, 0)[0].path, "/g");
        assert!(tree.worktrees_for_project(7, 0).is_empty());
    }

    /// `update/mod.rs:1351-1365` + `:1341-1350`.
    #[test]
    fn a_sweep_from_before_a_rebuild_is_discarded_wholesale() {
        let mut tree = ProjectTree::new();
        let generation = tree.generation();
        let mut swept = HashMap::new();
        swept.insert(1, vec![wt("/one", true)]);

        // Same generation: folded in.
        assert!(tree.apply_sweep(generation, swept.clone()));
        assert_eq!(tree.worktrees_for_project(1, 0).len(), 1);

        // The project list changed shape while a second sweep was running.
        tree.rebuild_wt_cache();
        assert_ne!(tree.generation(), generation);
        assert!(tree.worktrees_for_project(1, 0).is_empty());
        assert!(!tree.apply_sweep(generation, swept));
        assert!(tree.worktrees_for_project(1, 0).is_empty());
    }

    #[test]
    fn switching_the_active_project_hands_its_worktrees_to_the_cache() {
        let mut tree = ProjectTree::new();
        tree.set_active_worktrees(vec![wt("/a", true), wt("/a-x", false)]);
        // A stale entry for the incoming project must be dropped in favour of
        // the fresh inline read (`update/mod.rs:1128`).
        tree.wt_cache.insert(2, vec![wt("/stale", true)]);
        tree.switch_active_project(0, 2, "/nope");
        // The outgoing project's list is now readable from the cache…
        assert_eq!(tree.worktrees_for_project(0, 2).len(), 2);
        // …and the incoming project's stale cache entry is gone.
        assert!(!tree.wt_cache.contains_key(&2));
    }

    #[test]
    fn switching_to_the_same_project_is_a_no_op() {
        let mut tree = ProjectTree::new();
        tree.set_active_worktrees(vec![wt("/a", true)]);
        tree.switch_active_project(1, 1, "/nope");
        assert_eq!(tree.worktrees_for_project(1, 1).len(), 1);
    }

    /// `view/sidebar.rs:40-54`.
    #[test]
    fn the_is_repo_answer_is_memoized_for_five_seconds() {
        let mut tree = ProjectTree::new();
        let t0 = Instant::now();
        tree.seed_is_repo("/definitely-not-a-repo", true, t0);
        // Inside the TTL the (deliberately wrong) memo is returned…
        assert!(tree.is_repo_cached("/definitely-not-a-repo", t0 + Duration::from_secs(4)));
        // …and past it the real answer replaces it.
        assert!(!tree.is_repo_cached("/definitely-not-a-repo", t0 + Duration::from_secs(6)));
    }

    /// `update/mod.rs:1252-1259`.
    #[test]
    fn the_git_poll_is_throttled_to_one_run_per_five_seconds() {
        let mut tree = ProjectTree::new();
        let t0 = Instant::now();
        assert!(tree.git_poll_due(t0, true));
        assert!(!tree.git_poll_due(t0 + Duration::from_secs(4), true));
        assert!(tree.git_poll_due(t0 + Duration::from_secs(5), true));
    }

    /// Unfocused windows still catch up, just on a slower cadence.
    #[test]
    fn the_git_poll_degrades_to_sixty_seconds_when_unfocused() {
        let mut tree = ProjectTree::new();
        let t0 = Instant::now();
        assert!(tree.git_poll_due(t0, false));
        assert!(!tree.git_poll_due(t0 + Duration::from_secs(5), false));
        assert!(!tree.git_poll_due(t0 + Duration::from_secs(59), false));
        assert!(tree.git_poll_due(t0 + Duration::from_secs(60), false));
    }

    /// `update/mod.rs:1288-1299` — failure drops, it does not stale.
    #[test]
    fn a_failed_git_call_drops_the_cached_state_instead_of_showing_stale_data() {
        let mut tree = ProjectTree::new();
        let state = WorktreeGitState {
            dirty: true,
            ahead: 0,
            behind: 0,
        };
        let mut fresh = HashMap::new();
        fresh.insert("/a".to_string(), state);
        tree.apply_git_poll(fresh, &[]);
        assert!(tree.git_suffixes().contains_key("/a"));

        tree.apply_git_poll(HashMap::new(), &["/a".to_string()]);
        assert!(tree.git_suffixes().is_empty());
    }

    /// `update/mod.rs:1308-1324` — collapsed and archived projects are skipped.
    #[test]
    fn only_visible_worktrees_of_active_projects_are_polled() {
        let mut store = Store {
            projects: vec![
                project("alpha", "/a"),
                project("hidden", "/h"),
                project("gamma", "/g"),
            ],
            ..Store::default()
        };
        store.projects[1].archived = true;

        let mut tree = ProjectTree::new();
        tree.set_active_worktrees(vec![wt("/a", true)]);
        tree.wt_cache.insert(1, vec![wt("/h", true)]);
        tree.wt_cache.insert(2, vec![wt("/g", true)]);

        let mut ws = WorkspaceState::default();
        assert_eq!(
            tree.visible_worktree_paths(&store, &ws),
            vec!["/a".to_string(), "/g".to_string()]
        );

        // Collapsing a project removes its worktrees from the poll set.
        // (`select_project(0)` keeps `proj_idx` at 0, so the active list stays
        // where this fixture put it.)
        ws.select_project(0);
        assert_eq!(
            tree.visible_worktree_paths(&store, &ws),
            vec!["/g".to_string()]
        );
    }

    #[test]
    fn the_snapshot_names_worktrees_the_way_the_tree_renders_them() {
        let store = Store {
            projects: vec![project("alpha", "/a")],
            ..Store::default()
        };
        let mut registry = SessionRegistry::new();
        let id = registry.insert_meta(
            "alpha".into(),
            "/a".into(),
            grove_core::agent::Agent::Claude,
        );

        let mut tree = ProjectTree::new();
        tree.set_active_worktrees(vec![wt("/a", true), wt("/a/wt/feature", false)]);

        let snap = tree.snapshot(&store, &registry, 0);
        assert_eq!(snap.total_projects, 1);
        assert_eq!(snap.projects[0].idx, 0);
        let names: Vec<&str> = snap.projects[0]
            .worktrees
            .iter()
            .map(|w| w.name.as_str())
            .collect();
        // Main worktree = the project name; the rest = their basename.
        assert_eq!(names, vec!["alpha", "feature"]);
        assert_eq!(snap.projects[0].worktrees[0].sessions, vec![id]);
        assert!(snap.projects[0].worktrees[1].sessions.is_empty());
    }

    #[test]
    fn the_snapshot_skips_archived_projects_but_keeps_true_indices() {
        let mut store = Store {
            projects: vec![
                project("alpha", "/a"),
                project("hidden", "/h"),
                project("gamma", "/g"),
            ],
            ..Store::default()
        };
        store.projects[1].archived = true;
        let mut tree = ProjectTree::new();
        let snap = tree.snapshot(&store, &SessionRegistry::new(), 0);
        let idxs: Vec<usize> = snap.projects.iter().map(|p| p.idx).collect();
        assert_eq!(idxs, vec![0, 2]);
        assert_eq!(snap.total_projects, 3);
    }
}
