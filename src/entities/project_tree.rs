//! Worktrees, the per-project worktree cache, and the two background git jobs
//! the sidebar depends on. Everything here is `git`-shaped and therefore
//! slow — none of it may happen during a repaint (ported from
//! `src/gui/state.rs:39-42`, `src/gui/view/sidebar.rs:26-31`).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Context, Task};
use grove_core::git::{self, Worktree, WorktreeGitState};
use grove_core::storage::Store;

use crate::entities::session_registry::{SessionId, SessionRegistry};
use crate::entities::workspace_state::{
    RailMode, SnapshotProject, SnapshotWorktree, TreeSnapshot, WorkspaceState,
};
use crate::views::rows::{normalize_wt_path, path_basename};

const IS_REPO_TTL: Duration = Duration::from_secs(5);

const GIT_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Twelve times [`GIT_POLL_INTERVAL`] while unfocused; spelled in minutes because `clippy::duration_suboptimal_units` insists.
const GIT_POLL_INTERVAL_UNFOCUSED: Duration = Duration::from_mins(1);

#[derive(Default)]
pub struct ProjectTree {
    worktrees: Vec<Worktree>,
    /// Which TRUE project index `worktrees` belongs to; check before treating it as `active_proj`'s list.
    active_idx: Option<usize>,
    /// Every other project's worktrees, keyed by TRUE project index.
    wt_cache: HashMap<usize, Vec<Worktree>>,
    /// Bumped by every project-list shape change; a sweep stamped with an older generation is discarded wholesale.
    generation: u64,
    is_repo_memo: HashMap<String, (bool, Instant)>,
    git_state: HashMap<String, WorktreeGitState>,
    last_git_poll: Option<Instant>,
    /// Shared with the running poll so a slow `git status` sweep is skipped rather than overlapped.
    git_poll_inflight: Arc<AtomicBool>,
    git_poll: Option<Task<()>>,
    // Never read — dropping a Task cancels it, so this field just keeps the sweep alive.
    #[allow(dead_code)]
    sweep: Option<Task<()>>,
}

impl ProjectTree {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A cache miss is an empty slice, never a panic.
    #[must_use]
    pub fn worktrees_for_project(&self, proj: usize, active_proj: usize) -> &[Worktree] {
        if proj == active_proj && self.active_idx == Some(proj) {
            &self.worktrees
        } else {
            self.wt_cache.get(&proj).map_or(&[][..], Vec::as_slice)
        }
    }

    /// Every writer of `worktrees` must go through here (or `switch_active_project`), or `active_idx` drifts.
    pub fn set_active_worktrees(&mut self, proj: usize, worktrees: Vec<Worktree>) {
        self.worktrees = worktrees;
        self.active_idx = Some(proj);
    }

    /// Outgoing project's list moves into the cache; incoming's stale entry is dropped for a fresh inline read.
    pub fn switch_active_project(&mut self, old: usize, new: usize, new_path: &str) {
        if old == new {
            return;
        }
        let outgoing = std::mem::take(&mut self.worktrees);
        self.wt_cache.insert(old, outgoing);
        self.wt_cache.remove(&new);
        self.worktrees = git::list_worktrees(new_path);
        self.active_idx = Some(new);
    }

    /// Heals a stale `active_idx`; out-of-range still gets stamped (empty list) or this re-shells `git` every frame.
    pub fn ensure_active(&mut self, active_proj: usize, store: &Store) {
        if self.active_idx == Some(active_proj) {
            return;
        }
        if let Some(old) = self.active_idx {
            let outgoing = std::mem::take(&mut self.worktrees);
            self.wt_cache.insert(old, outgoing);
        }
        self.wt_cache.remove(&active_proj);
        self.worktrees = store
            .projects
            .get(active_proj)
            .map(|p| git::list_worktrees(&p.path))
            .unwrap_or_default();
        self.active_idx = Some(active_proj);
    }

    /// Same hand-off as [`Self::switch_active_project`], for a session crossing project boundaries.
    pub fn adopt_session_project(
        tree: &gpui::Entity<Self>,
        snap: &TreeSnapshot,
        id: SessionId,
        old: usize,
        cx: &mut gpui::App,
    ) {
        let Some(proj) = snap
            .projects
            .iter()
            .find(|p| p.worktrees.iter().any(|w| w.sessions.contains(&id)))
            .map(|p| p.idx)
            .filter(|&p| p != old)
        else {
            return;
        };
        let path = cx
            .global::<crate::settings::SettingsState>()
            .store
            .projects
            .get(proj)
            .map(|p| p.path.clone());
        if let Some(path) = path {
            tree.update(cx, |t, cx| {
                t.switch_active_project(old, proj, &path);
                cx.notify();
            });
        }
    }

    // TODO(unwired): no caller pre-warms the worktree cache — see `sweep_wt_cache` below.
    #[allow(dead_code)]
    pub fn ensure_wt_cached(&mut self, proj: usize, active_proj: usize, store: &Store) {
        if proj == active_proj || self.wt_cache.contains_key(&proj) {
            return;
        }
        if let Some(p) = store.projects.get(proj) {
            let wts = git::list_worktrees(&p.path);
            self.wt_cache.insert(proj, wts);
        }
    }

    /// Called by every add/remove/archive/restore, so any sweep in flight is now stale.
    pub fn rebuild_wt_cache(&mut self) {
        self.wt_cache.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    /// Discards the sweep if the generation moved while it ran (reached in production only from unwired `sweep_wt_cache`).
    #[allow(dead_code)]
    pub fn apply_sweep(&mut self, generation: u64, swept: HashMap<usize, Vec<Worktree>>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.wt_cache.extend(swept);
        true
    }

    // TODO(unwired): nothing calls this yet.
    #[allow(dead_code)]
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

    /// `git::is_repo` memoized for [`IS_REPO_TTL`]; `now` is injected so expiry is testable without sleeping.
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

    #[cfg(test)]
    fn seed_is_repo(&mut self, path: &str, answer: bool, at: Instant) {
        self.is_repo_memo.insert(path.to_string(), (answer, at));
    }

    /// `None` from `git_state_suffix` means "nothing worth showing", so that worktree has no suffix.
    #[must_use]
    pub fn git_suffixes(&self) -> HashMap<String, String> {
        self.git_state
            .iter()
            .filter_map(|(path, state)| git::git_state_suffix(state).map(|s| (path.clone(), s)))
            .collect()
    }

    /// Includes the uncommitted diff counts that `git_suffixes` deliberately does not render.
    #[must_use]
    pub fn git_states(&self) -> HashMap<String, WorktreeGitState> {
        self.git_state.clone()
    }

    /// Failures drop the cached entry rather than leaving stale data on screen.
    pub fn apply_git_poll(&mut self, fresh: HashMap<String, WorktreeGitState>, stale: &[String]) {
        self.git_state.extend(fresh);
        for path in stale {
            self.git_state.remove(path);
        }
    }

    /// Stamps `last_git_poll` as a side effect when true — a caller must not call and discard, that still burns the window.
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

    /// Skips archived/collapsed projects, or `git status` keeps polling worktrees nothing renders.
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
                    .map(|w| normalize_wt_path(&w.path).to_string()),
            );
        }
        paths
    }

    /// `Sessions` mode polls the set backing live sessions instead of [`Self::visible_worktree_paths`]'s tree-visible set.
    #[must_use]
    pub fn polled_worktree_paths(
        &self,
        store: &Store,
        ws: &WorkspaceState,
        session_wt_paths: &[String],
    ) -> Vec<String> {
        match ws.rail_mode() {
            RailMode::Tree => self.visible_worktree_paths(store, ws),
            RailMode::Sessions => {
                let mut seen = HashSet::new();
                session_wt_paths
                    .iter()
                    .map(|p| normalize_wt_path(p))
                    .filter(|p| !p.is_empty())
                    .filter(|p| seen.insert(*p))
                    .map(ToString::to_string)
                    .collect()
            }
        }
    }

    /// Empty check must come before `git_poll_due`, or its side-effect stamp burns the window before there's anything to poll.
    fn poll_decision(&mut self, now: Instant, focused: bool, paths_empty: bool) -> bool {
        if paths_empty {
            return false;
        }
        self.git_poll_due(now, focused)
    }

    pub fn maybe_poll_git_state(
        &mut self,
        paths: Vec<String>,
        focused: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.poll_decision(Instant::now(), focused, paths.is_empty()) {
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

    /// One pass over the registry up front, so no row rescans the session list.
    #[must_use]
    pub fn snapshot(
        &mut self,
        store: &Store,
        registry: &SessionRegistry,
        active_proj: usize,
    ) -> TreeSnapshot {
        self.ensure_active(active_proj, store);
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
                // Main worktree shows the project name; others show their basename.
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
            worktree_dir: None,
        }
    }

    #[test]
    fn an_empty_path_poll_does_not_consume_the_throttle_window() {
        let mut tree = ProjectTree::new();
        let now = Instant::now();
        assert!(!tree.poll_decision(now, true, true));
        assert!(tree.last_git_poll.is_none());
        assert!(tree.poll_decision(now, true, false));
    }

    #[test]
    fn worktrees_for_project_reads_the_active_list_then_the_cache() {
        let mut tree = ProjectTree::new();
        tree.set_active_worktrees(0, vec![wt("/a", true)]);
        tree.wt_cache.insert(2, vec![wt("/g", true)]);
        assert_eq!(tree.worktrees_for_project(0, 0).len(), 1);
        assert_eq!(tree.worktrees_for_project(2, 0)[0].path, "/g");
        assert!(tree.worktrees_for_project(7, 0).is_empty());
    }

    #[test]
    fn a_list_seeded_for_one_project_is_never_served_for_another() {
        let mut tree = ProjectTree::new();
        tree.set_active_worktrees(0, vec![wt("/a", true)]);
        assert!(tree.worktrees_for_project(1, 1).is_empty());
        assert!(tree.worktrees_for_project(0, 1).is_empty());
        assert_eq!(tree.worktrees_for_project(0, 0).len(), 1);
    }

    #[test]
    fn a_sweep_from_before_a_rebuild_is_discarded_wholesale() {
        let mut tree = ProjectTree::new();
        let generation = tree.generation();
        let mut swept = HashMap::new();
        swept.insert(1, vec![wt("/one", true)]);

        assert!(tree.apply_sweep(generation, swept.clone()));
        assert_eq!(tree.worktrees_for_project(1, 0).len(), 1);

        tree.rebuild_wt_cache();
        assert_ne!(tree.generation(), generation);
        assert!(tree.worktrees_for_project(1, 0).is_empty());
        assert!(!tree.apply_sweep(generation, swept));
        assert!(tree.worktrees_for_project(1, 0).is_empty());
    }

    #[test]
    fn switching_the_active_project_hands_its_worktrees_to_the_cache() {
        let mut tree = ProjectTree::new();
        tree.set_active_worktrees(0, vec![wt("/a", true), wt("/a-x", false)]);
        tree.wt_cache.insert(2, vec![wt("/stale", true)]);
        tree.switch_active_project(0, 2, "/nope");
        assert_eq!(tree.worktrees_for_project(0, 2).len(), 2);
        assert!(!tree.wt_cache.contains_key(&2));
    }

    #[test]
    fn switching_to_the_same_project_is_a_no_op() {
        let mut tree = ProjectTree::new();
        tree.set_active_worktrees(1, vec![wt("/a", true)]);
        tree.switch_active_project(1, 1, "/nope");
        assert_eq!(tree.worktrees_for_project(1, 1).len(), 1);
    }

    #[test]
    fn the_is_repo_answer_is_memoized_for_five_seconds() {
        let mut tree = ProjectTree::new();
        let t0 = Instant::now();
        tree.seed_is_repo("/definitely-not-a-repo", true, t0);
        assert!(tree.is_repo_cached("/definitely-not-a-repo", t0 + Duration::from_secs(4)));
        assert!(!tree.is_repo_cached("/definitely-not-a-repo", t0 + Duration::from_secs(6)));
    }

    #[test]
    fn the_git_poll_is_throttled_to_one_run_per_five_seconds() {
        let mut tree = ProjectTree::new();
        let t0 = Instant::now();
        assert!(tree.git_poll_due(t0, true));
        assert!(!tree.git_poll_due(t0 + Duration::from_secs(4), true));
        assert!(tree.git_poll_due(t0 + Duration::from_secs(5), true));
    }

    #[test]
    fn the_git_poll_degrades_to_sixty_seconds_when_unfocused() {
        let mut tree = ProjectTree::new();
        let t0 = Instant::now();
        assert!(tree.git_poll_due(t0, false));
        assert!(!tree.git_poll_due(t0 + Duration::from_secs(5), false));
        assert!(!tree.git_poll_due(t0 + Duration::from_secs(59), false));
        assert!(tree.git_poll_due(t0 + Duration::from_mins(1), false));
    }

    #[test]
    fn a_failed_git_call_drops_the_cached_state_instead_of_showing_stale_data() {
        let mut tree = ProjectTree::new();
        let state = WorktreeGitState {
            dirty: true,
            ahead: 0,
            behind: 0,
            added: 0,
            removed: 0,
        };
        let mut fresh = HashMap::new();
        fresh.insert("/a".to_string(), state);
        tree.apply_git_poll(fresh, &[]);
        assert!(tree.git_suffixes().contains_key("/a"));

        tree.apply_git_poll(HashMap::new(), &["/a".to_string()]);
        assert!(tree.git_suffixes().is_empty());
    }

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
        tree.set_active_worktrees(0, vec![wt("/a", true)]);
        tree.wt_cache.insert(1, vec![wt("/h", true)]);
        tree.wt_cache.insert(2, vec![wt("/g", true)]);

        let mut ws = WorkspaceState::default();
        assert_eq!(
            tree.visible_worktree_paths(&store, &ws),
            vec!["/a".to_string(), "/g".to_string()]
        );

        ws.select_project(0);
        assert_eq!(
            tree.visible_worktree_paths(&store, &ws),
            vec!["/g".to_string()]
        );
    }

    #[test]
    fn sessions_mode_polls_the_worktrees_behind_live_sessions_even_when_collapsed() {
        let store = Store {
            projects: vec![project("alpha", "/a"), project("gamma", "/g")],
            ..Store::default()
        };
        let mut tree = ProjectTree::new();
        tree.set_active_worktrees(0, vec![wt("/a", true)]);
        tree.wt_cache.insert(1, vec![wt("/g", true)]);

        let mut ws = WorkspaceState::default();
        ws.select_project(0);
        let sessions = vec!["/a".to_string(), "/g/".to_string(), "/g".to_string()];
        assert_eq!(
            tree.polled_worktree_paths(&store, &ws, &sessions),
            vec!["/g".to_string()]
        );

        ws.toggle_rail_mode();
        assert_eq!(ws.rail_mode(), RailMode::Sessions);
        assert_eq!(
            tree.polled_worktree_paths(&store, &ws, &sessions),
            vec!["/a".to_string(), "/g".to_string()]
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
        tree.set_active_worktrees(0, vec![wt("/a", true), wt("/a/wt/feature", false)]);

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

    /// `git::list_worktrees` on a non-repo path falls back to a synthetic root worktree, never an empty list.
    #[test]
    fn ensure_active_heals_a_snapshot_that_skipped_the_hand_off() {
        let store = Store {
            projects: vec![
                project("p0", "/grove-test-p0-does-not-exist"),
                project("p1", "/grove-test-p1-does-not-exist"),
            ],
            ..Store::default()
        };
        let mut tree = ProjectTree::new();
        tree.set_active_worktrees(0, vec![wt("/a", true), wt("/a-x", false)]);

        let snap = tree.snapshot(&store, &SessionRegistry::new(), 1);

        let Some(p1) = snap.projects.iter().find(|p| p.idx == 1) else {
            panic!("snapshot must still include project 1");
        };
        assert!(!p1.worktrees.iter().any(|w| w.path == "/a"));
        assert!(!p1.worktrees.iter().any(|w| w.path == "/a-x"));
        assert_eq!(p1.worktrees.len(), 1);
        assert_eq!(p1.worktrees[0].path, "/grove-test-p1-does-not-exist");

        let Some(p0) = snap.projects.iter().find(|p| p.idx == 0) else {
            panic!("snapshot must still include project 0");
        };
        let p0_paths: Vec<&str> = p0.worktrees.iter().map(|w| w.path.as_str()).collect();
        assert_eq!(p0_paths, vec!["/a", "/a-x"]);
    }

    #[test]
    fn ensure_active_is_a_no_op_once_healed() {
        let store = Store {
            projects: vec![
                project("p0", "/grove-test-p0-does-not-exist"),
                project("p1", "/grove-test-p1-does-not-exist"),
            ],
            ..Store::default()
        };
        let mut tree = ProjectTree::new();
        tree.set_active_worktrees(0, vec![wt("/a", true)]);

        tree.ensure_active(1, &store);
        let healed: Vec<String> = tree.worktrees.iter().map(|w| w.path.clone()).collect();
        let cached_0: Option<Vec<String>> = tree
            .wt_cache
            .get(&0)
            .map(|wts| wts.iter().map(|w| w.path.clone()).collect());

        tree.ensure_active(1, &store);
        let after: Vec<String> = tree.worktrees.iter().map(|w| w.path.clone()).collect();
        let cached_0_after: Option<Vec<String>> = tree
            .wt_cache
            .get(&0)
            .map(|wts| wts.iter().map(|w| w.path.clone()).collect());
        assert_eq!(after, healed);
        assert_eq!(cached_0_after, cached_0);
    }

    #[test]
    fn ensure_active_stamps_an_out_of_range_index_instead_of_retrying() {
        let store = Store {
            projects: vec![project("p0", "/grove-test-p0-does-not-exist")],
            ..Store::default()
        };
        let mut tree = ProjectTree::new();
        tree.set_active_worktrees(0, vec![wt("/a", true)]);

        tree.ensure_active(5, &store);
        assert_eq!(tree.active_idx, Some(5));
        assert!(tree.worktrees.is_empty());

        tree.ensure_active(5, &store);
        assert_eq!(tree.active_idx, Some(5));
        assert!(tree.worktrees.is_empty());
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
