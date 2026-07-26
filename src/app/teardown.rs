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
