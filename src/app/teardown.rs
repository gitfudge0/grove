use super::{App, Modal, Teardown, TeardownStage};
use anyhow::Result;
use grove_core::git;
use grove_core::session::Session;
use grove_core::storage::{self, Project};

impl App {
    /// Open the project-removal modal for the project at `idx`. Discovers
    /// the project's non-main worktrees up front so the modal can show
    /// "Also delete N worktrees on disk" without re-shelling-out per frame.
    pub(crate) fn open_remove_project_modal(&mut self, idx: usize) {
        let Some(p) = self.store.projects.get(idx).cloned() else {
            return;
        };
        let worktrees: Vec<String> = git::list_worktrees(&p.path)
            .into_iter()
            .filter(|w| !w.is_main)
            .map(|w| w.path)
            .collect();
        self.modal = Modal::RemoveProject {
            idx,
            name: p.name,
            project_path: p.path,
            worktrees,
            also_remove_worktrees: false,
            in_progress: false,
            done: 0,
            current: String::new(),
            errors: Vec::new(),
        };
    }

    /// Kill any sessions belonging to the named project. Used when removing
    /// a project so its sessions don't linger in the sidebar as orphans.
    pub(crate) fn kill_sessions_for_project(&mut self, project: &str) {
        let mut i = 0;
        while i < self.sessions.len() {
            if self.sessions[i].project == project {
                self.sessions[i].kill();
                self.sessions.remove(i);
                match self.active_session {
                    Some(a) if a == i => self.active_session = None,
                    Some(a) if a > i => self.active_session = Some(a - 1),
                    _ => {}
                }
            } else {
                i += 1;
            }
        }
        if self.sessions.is_empty() {
            self.active_session = None;
        }
    }

    /// Live sessions belonging to `project` (by name), across every worktree.
    /// Returns indices into `self.sessions` so the caller can render details.
    ///
    /// Sessions are per-worktree, so a project with three worktrees can hold
    /// any number of sessions — this counts SESSIONS, never worktrees. The
    /// match is plain name equality on `Session::project`, deliberately
    /// identical to `kill_sessions_for_project` above: if the gate counted a
    /// different set than the killer kills, the confirm modal would either
    /// under-report or leave sessions behind.
    pub(crate) fn session_indices_for_project(&self, project: &str) -> Vec<usize> {
        self.sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| s.project == project)
            .map(|(i, _)| i)
            .collect()
    }

    /// Hide `idx` from the sidebar. Never kills sessions and never touches disk
    /// beyond projects.json. It is not purely in-memory, though: when the
    /// archived project was the selected one, the selection is reseated onto the
    /// first remaining visible project and `refresh_worktrees()` re-reads that
    /// project's worktrees from git. If no visible project remains,
    /// `selected_project()` reports `None` and the refresh clears
    /// `self.worktrees` without shelling out at all.
    pub(crate) fn archive_project(&mut self, idx: usize) {
        let Some(name) = self.store.projects.get(idx).map(|p| p.name.clone()) else {
            return;
        };
        // The confirm-modal gate is a UI precondition, not a store invariant.
        // This is the backstop for any future code path that archives without
        // going through the modal: archiving a project with live sessions
        // would strand them in the sidebar under a project the user can no
        // longer see, with no way to reach or kill them.
        //
        // The `debug_assert!` is the loud development signal, but it is compiled
        // out of release builds, so it cannot be the whole backstop. The guard
        // below it is what actually refuses in a shipped binary (house style:
        // see `finalize_remove_project`'s leading guard). It sits *after* the
        // assertion deliberately — returning first would make the assertion
        // unreachable in debug builds and silence the signal entirely.
        debug_assert!(
            self.session_indices_for_project(&name).is_empty(),
            "archive_project({idx}) called for {name:?} while it still has live sessions — \
             callers must kill or refuse first (the confirm modal is the intended gate)"
        );
        if !self.session_indices_for_project(&name).is_empty() {
            return;
        }
        self.store.projects[idx].archived = true;
        storage::persist(&self.store);
        // Archiving does NOT `projects.remove(idx)`, so neither of the
        // existing `proj_idx >= len` clamps fires and `proj_idx` would stay
        // pointing at a now-hidden project. `selected_project()` filters
        // archived entries out, so a selection parked on a hidden slot resolves
        // to `None` and `refresh_worktrees()` clears the list instead of
        // shelling out to git for a project the user cannot see — but reseating
        // onto a visible project when one exists is still what the user wants.
        if idx == self.proj_idx {
            self.proj_idx = self.store.active_projects().next().map_or(0, |(i, _)| i);
            self.refresh_worktrees();
        }
    }

    /// Unhide `idx`. Scripts, theme, and list position are preserved for free
    /// because the entry never left `store.projects`.
    pub(crate) fn restore_project(&mut self, idx: usize) {
        if self.store.projects.get(idx).is_none() {
            return;
        }
        self.store.projects[idx].archived = false;
        storage::persist(&self.store);
        // A restored project whose directory is gone is fine: when `git -C`
        // fails, `git::list_worktrees` degrades to a single synthetic root
        // worktree rather than erroring or panicking.
        if idx == self.proj_idx {
            self.refresh_worktrees();
        } else if self.selected_project().is_none() {
            // The selection was parked on a hidden slot (every project was
            // archived, so `archive_project` had nothing visible to reseat
            // onto). Restoring a project is the moment a visible one exists
            // again — land the user on the project they just brought back.
            self.proj_idx = idx;
            self.refresh_worktrees();
        }
    }

    /// Finalize project removal after any worktree teardown has completed.
    /// Kills lingering sessions, drops the project from the store, and
    /// persists. Returns a status string for the caller to display.
    pub(crate) fn finalize_remove_project(&mut self, idx: usize) -> Result<String> {
        if idx >= self.store.projects.len() {
            return Ok(String::new());
        }
        let name = self.store.projects[idx].name.clone();
        self.kill_sessions_for_project(&name);
        let removed = self.store.projects.remove(idx);
        storage::save(&self.store)?;
        if self.proj_idx >= self.store.projects.len() {
            self.proj_idx = self.store.projects.len().saturating_sub(1);
        }
        self.refresh_worktrees();
        Ok(format!("removed project {}", removed.name))
    }

    /// Kill any sessions whose worktree path matches `wt_path` and remove them
    /// from the sessions list. Adjusts `active_session` so it still points at a
    /// valid session (or `None` when the list is empty).
    pub(crate) fn kill_sessions_for_wt(&mut self, wt_path: &str) {
        let mut i = 0;
        while i < self.sessions.len() {
            if self.sessions[i].wt_path == wt_path {
                self.sessions[i].kill();
                self.sessions.remove(i);
                match self.active_session {
                    Some(a) if a == i => self.active_session = None,
                    Some(a) if a > i => self.active_session = Some(a - 1),
                    _ => {}
                }
            } else {
                i += 1;
            }
        }
        if self.sessions.is_empty() {
            self.active_session = None;
        }
        // The worktree is going away — drop its panel shells too.
        self.kill_wt_terminals(wt_path);
    }

    /// Begin tearing down `path`: kill its sessions, then either run the
    /// project's teardown script in a modal PTY (advancing to removal when it
    /// exits) or remove the worktree immediately. Opens `Modal::Teardown`.
    pub(crate) fn start_teardown(&mut self, p: &Project, path: String) {
        self.kill_sessions_for_wt(&path);
        let script = p
            .scripts
            .teardown
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let session = script.and_then(|s| {
            Session::spawn_script("teardown".into(), p.name.clone(), path.clone(), s, &path).ok()
        });
        self.modal = Modal::Teardown;
        if session.is_some() {
            self.teardown = Some(Teardown {
                wt_path: path,
                project_path: p.path.clone(),
                session,
                stage: TeardownStage::RunningScript,
                message: "running teardown script…".into(),
                removal_started: false,
            });
        } else {
            // No teardown script (or it failed to spawn): the next poll paints
            // "removing worktree…" then runs the blocking git removal.
            self.teardown = Some(Teardown {
                wt_path: path,
                project_path: p.path.clone(),
                session: None,
                stage: TeardownStage::Removing,
                message: "removing worktree…".into(),
                removal_started: false,
            });
        }
    }

    /// Drive an in-progress teardown forward. Called every GUI tick. When the
    /// teardown script's PTY exits, performs the git removal.
    pub(crate) fn poll_teardown(&mut self) {
        let Some(td) = self.teardown.as_mut() else {
            return;
        };
        // Script finished → switch to the "removing…" stage. Removal itself
        // waits for the next poll so that frame paints before we block.
        if td.stage == TeardownStage::RunningScript
            && td.session.as_ref().is_none_or(|s| !s.is_running())
        {
            td.stage = TeardownStage::Removing;
            td.session = None;
            td.message = "removing worktree…".into();
            return;
        }
        if td.stage == TeardownStage::Removing && !td.removal_started {
            td.removal_started = true;
            self.do_teardown_removal();
        }
    }

    /// Run `git worktree remove` for the active teardown and transition to
    /// `Done`. Drops the (exited) teardown PTY session.
    fn do_teardown_removal(&mut self) {
        let Some(td) = self.teardown.as_mut() else {
            return;
        };
        td.stage = TeardownStage::Removing;
        td.session = None;
        let wt_path = td.wt_path.clone();
        let project_path = td.project_path.clone();
        let err = git::remove_worktree(&project_path, &wt_path)
            .err()
            .map(|e| e.to_string());
        if let Some(td) = self.teardown.as_mut() {
            td.stage = TeardownStage::Done {
                failed: err.is_some(),
            };
            td.message = match &err {
                Some(e) => format!("removal failed: {e}"),
                None => "worktree deleted".into(),
            };
        }
        match &err {
            Some(e) => self.set_error_toast(format!("teardown err: {e}")),
            None => self.set_toast(format!("removed worktree {wt_path}")),
        }
        self.refresh_worktrees();
    }

    /// Skip a still-running teardown script: kill it and proceed to removal.
    pub(crate) fn skip_teardown_script(&mut self) {
        if let Some(td) = self.teardown.as_mut() {
            if td.stage == TeardownStage::RunningScript {
                if let Some(s) = td.session.as_mut() {
                    s.kill();
                }
                self.do_teardown_removal();
            }
        }
    }

    /// Dismiss the teardown modal. Only meaningful once removal has finished.
    pub(crate) fn close_teardown(&mut self) {
        self.teardown = None;
        self.modal = Modal::None;
    }
}

#[cfg(test)]
mod tests {
    use super::App;
    use crate::app::test_app;
    use grove_core::session::Session;
    use grove_core::storage::{self, Project, ProjectScripts};

    /// `archive_project` / `restore_project` call `storage::persist`, which
    /// writes `projects.json` under `GROVE_CONFIG_DIR`. Tests share one
    /// process, so the env var is guarded by a mutex and restored on drop —
    /// otherwise a test would clobber the developer's real `~/.config/grove`.
    static CONFIG_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct ConfigDir {
        dir: std::path::PathBuf,
        prev: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl ConfigDir {
        fn new(tag: &str) -> Self {
            let lock = CONFIG_DIR_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let dir = std::env::temp_dir().join(format!(
                "grove_test_archive_{tag}_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            let _ = fs_err::remove_dir_all(&dir);
            let prev = std::env::var(storage::CONFIG_DIR_ENV).ok();
            std::env::set_var(storage::CONFIG_DIR_ENV, &dir);
            Self {
                dir,
                prev,
                _lock: lock,
            }
        }
    }

    impl Drop for ConfigDir {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(storage::CONFIG_DIR_ENV, v),
                None => std::env::remove_var(storage::CONFIG_DIR_ENV),
            }
            let _ = fs_err::remove_dir_all(&self.dir);
        }
    }

    fn project(name: &str) -> Project {
        Project {
            name: name.into(),
            // Deliberately nonexistent: `git::list_worktrees` returns an empty
            // Vec when `git -C` fails, so `refresh_worktrees()` is safe here.
            path: format!("/nonexistent/grove-test/{name}"),
            scripts: ProjectScripts::default(),
            theme: None,
            archived: false,
        }
    }

    /// A real, long-lived session tagged with an arbitrary project/worktree
    /// pair. `Session`'s PTY fields are private, so spawning a shell is the
    /// lightest way to get a well-formed `Session`. The shell `exec`s `cat`,
    /// which blocks on its PTY rather than exiting, so its monitor thread
    /// never publishes an `Exited` status mid-test; it is reaped when the
    /// `Session` drops (`Drop for Session` kills the process group).
    fn session(proj: &str, wt: &str) -> Session {
        Session::spawn_script("t".into(), proj.into(), wt.into(), "exec cat", ".")
            .expect("spawn test session")
    }

    /// The A6/A7 fixture: project "sandbox" with three worktrees holding FOUR
    /// sessions (two of them in `fix/palette`), so anything that counts
    /// worktrees instead of sessions reports 3 and fails.
    fn app_with_sandbox_fixture() -> App {
        test_app(vec![
            session("sandbox", "/wt/main"),
            session("sandbox", "/wt/feat-attention"),
            session("sandbox", "/wt/fix-palette"),
            session("sandbox", "/wt/fix-palette"),
        ])
    }

    // ── A6 ───────────────────────────────────────────────────────────────

    #[test]
    fn gate_counts_sessions_not_worktrees() {
        let app = app_with_sandbox_fixture();
        let idx = app.session_indices_for_project("sandbox");
        assert_eq!(
            idx,
            vec![0, 1, 2, 3],
            "the gate must count SESSIONS (4) across 3 worktrees, not worktrees (3)"
        );
    }

    #[test]
    fn gate_matches_by_exact_name_not_prefix() {
        let mut app = app_with_sandbox_fixture();
        app.sessions.push(session("sandbox-old", "/wt/old"));
        assert_eq!(
            app.session_indices_for_project("sandbox").len(),
            4,
            "\"sandbox-old\" must not be counted as a \"sandbox\" session"
        );
        assert_eq!(app.session_indices_for_project("sandbox-old"), vec![4]);
        assert!(app.session_indices_for_project("sand").is_empty());
    }

    // ── A7 ───────────────────────────────────────────────────────────────

    #[test]
    fn kill_all_clears_the_gate_without_collateral() {
        let mut app = app_with_sandbox_fixture();
        app.sessions.push(session("sandbox-old", "/wt/old"));
        // Points past every "sandbox" session, so the bookkeeping must walk it
        // down by exactly the four removals.
        app.active_session = Some(4);

        app.kill_sessions_for_project("sandbox");

        assert!(
            app.session_indices_for_project("sandbox").is_empty(),
            "the gate must be clear after kill-all"
        );
        assert_eq!(app.sessions.len(), 1, "only \"sandbox-old\" may survive");
        assert_eq!(app.sessions[0].project, "sandbox-old");
        assert_eq!(app.sessions[0].wt_path, "/wt/old");
        assert_eq!(
            app.active_session,
            Some(0),
            "active_session must shift down by the four removals below it"
        );
    }

    // ── A1 ───────────────────────────────────────────────────────────────

    #[test]
    fn archive_never_touches_sessions() {
        let _cfg = ConfigDir::new("no_kill");
        let mut app = test_app(vec![
            session("sandbox", "/wt/main"),
            session("sandbox", "/wt/fix-palette"),
        ]);
        // The archived project is a different, session-free one, because
        // `archive_project`'s `debug_assert!` refuses (correctly) to archive a
        // project that still has sessions — see
        // `archive_with_live_sessions_trips_the_backstop`. Survivorship is the
        // proxy for not-killed: `kill_sessions_for_project` kills and removes
        // in the same step, so a surviving session was never killed.
        app.store.projects = vec![project("quiet"), project("sandbox")];
        app.proj_idx = 0;
        app.active_session = Some(1);

        app.archive_project(0);

        assert!(app.store.projects[0].archived);
        assert_eq!(app.sessions.len(), 2, "archive must not remove sessions");
        assert_eq!(app.sessions[0].project, "sandbox");
        assert_eq!(app.sessions[0].wt_path, "/wt/main");
        assert_eq!(app.sessions[1].project, "sandbox");
        assert_eq!(app.sessions[1].wt_path, "/wt/fix-palette");
        assert_eq!(
            app.active_session,
            Some(1),
            "archive must not disturb active_session"
        );
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "still has live sessions")]
    fn archive_with_live_sessions_trips_the_backstop() {
        let _cfg = ConfigDir::new("backstop");
        let mut app = test_app(vec![session("sandbox", "/wt/main")]);
        app.store.projects = vec![project("sandbox")];
        app.archive_project(0);
    }

    // ── A2 ───────────────────────────────────────────────────────────────

    #[test]
    fn archiving_the_selected_project_moves_proj_idx_to_an_active_one() {
        let _cfg = ConfigDir::new("proj_idx");
        let mut app = test_app(vec![]);
        app.store.projects = vec![project("a"), project("b"), project("c"), project("d")];
        app.proj_idx = 2;
        // Sentinel: `refresh_worktrees()` must overwrite this. Fixture paths
        // are nonexistent, so a real refresh yields the single synthetic root
        // worktree `git::list_worktrees` falls back to on a failed `git -C`.
        app.worktrees = vec![grove_core::git::Worktree {
            path: "/stale".into(),
            branch: "stale".into(),
            mtime: None,
            is_main: true,
        }];

        app.archive_project(2);

        assert!(app.store.projects[2].archived);
        assert_ne!(app.proj_idx, 2, "proj_idx must leave the archived project");
        assert!(
            !app.store.projects[app.proj_idx].archived,
            "proj_idx must land on an ACTIVE project"
        );
        assert_eq!(app.proj_idx, 0, "first active project wins");
        let sel = app.selected_project().expect("a project is selected");
        assert!(!sel.archived);
        assert_eq!(sel.name, "a");
        assert_eq!(
            app.worktrees
                .iter()
                .map(|w| w.path.as_str())
                .collect::<Vec<_>>(),
            vec![app.store.projects[0].path.as_str()],
            "refresh_worktrees() must have run for the NEW selection"
        );
    }

    #[test]
    fn archiving_the_last_active_project_falls_back_to_zero() {
        let _cfg = ConfigDir::new("all_archived");
        let mut app = test_app(vec![]);
        app.store.projects = vec![project("only")];
        app.proj_idx = 0;

        app.archive_project(0);

        assert_eq!(app.proj_idx, 0, "no active project remains: fall back to 0");
        assert!(app.store.active_projects().next().is_none());
        // Slot 0 is itself archived now, so the selection resolves to nothing
        // and the refresh must NOT have shelled out to git for a hidden
        // project — the workspace falls to its empty state instead.
        assert!(
            app.selected_project().is_none(),
            "a selection parked on an archived slot must not resolve to a project"
        );
        assert!(
            app.worktrees.is_empty(),
            "no visible project selected: worktrees must be cleared, not read from a hidden one"
        );
    }

    #[test]
    fn restoring_a_project_reseats_a_selection_parked_on_a_hidden_one() {
        let _cfg = ConfigDir::new("restore_reseat");
        let mut app = test_app(vec![]);
        app.store.projects = vec![project("a"), project("b"), project("c")];
        app.proj_idx = 0;

        // Archive every project. The last archive has nothing visible left to
        // reseat onto, so the selection parks on the (archived) slot 0.
        app.archive_project(1);
        app.archive_project(2);
        app.archive_project(0);
        assert!(app.store.active_projects().next().is_none());
        assert!(app.selected_project().is_none());

        // Restore a project that is NOT the parked selection.
        app.restore_project(2);

        assert_eq!(
            app.proj_idx, 2,
            "restoring must land the selection on the project just brought back"
        );
        let sel = app
            .selected_project()
            .expect("the selection must resolve to a visible project");
        assert!(!sel.archived);
        assert_eq!(sel.name, "c");
        assert_eq!(
            app.worktrees
                .iter()
                .map(|w| w.path.as_str())
                .collect::<Vec<_>>(),
            vec![app.store.projects[2].path.as_str()],
            "app.worktrees must belong to the newly selected project"
        );
    }

    // ── A3 ───────────────────────────────────────────────────────────────

    #[test]
    fn restore_preserves_scripts_theme_and_index() {
        let _cfg = ConfigDir::new("restore");
        let mut app = test_app(vec![]);
        let mut middle = project("middle");
        middle.scripts = ProjectScripts {
            setup: Some("echo setup".into()),
            run: Some("echo run".into()),
            teardown: Some("echo teardown".into()),
        };
        middle.theme = Some("tokyonight-day".into());
        app.store.projects = vec![project("first"), middle, project("last")];
        app.proj_idx = 0;

        app.archive_project(1);
        assert!(app.store.projects[1].archived);

        // Round-trip through disk: `archive_project` already persisted.
        app.store = storage::load().expect("load persisted store");
        assert_eq!(app.store.projects.len(), 3);
        assert!(app.store.projects[1].archived, "archived state must reload");

        app.restore_project(1);

        let p = &app.store.projects[1];
        assert!(!p.archived);
        assert_eq!(p.name, "middle", "list position must be preserved");
        assert_eq!(p.scripts.setup.as_deref(), Some("echo setup"));
        assert_eq!(p.scripts.run.as_deref(), Some("echo run"));
        assert_eq!(p.scripts.teardown.as_deref(), Some("echo teardown"));
        assert_eq!(p.theme.as_deref(), Some("tokyonight-day"));
        assert_eq!(
            app.store.projects[0].name, "first",
            "neighbours must be untouched"
        );
        assert_eq!(app.store.projects[2].name, "last");
    }

    #[test]
    fn restore_of_a_project_whose_directory_is_gone_does_not_panic() {
        let _cfg = ConfigDir::new("gone");
        let mut app = test_app(vec![]);
        app.store.projects = vec![project("vanished")];
        app.proj_idx = 0;
        app.archive_project(0);

        app.restore_project(0);

        assert!(!app.store.projects[0].archived);
        assert_eq!(
            app.worktrees
                .iter()
                .map(|w| w.path.as_str())
                .collect::<Vec<_>>(),
            vec![app.store.projects[0].path.as_str()],
            "a missing directory degrades to one synthetic root worktree, no panic"
        );
    }
}
