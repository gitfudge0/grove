use super::{path_basename, App, Modal};
use grove_core::agent::Agent;
use grove_core::git;
use grove_core::session::Session;
use grove_core::storage::Project;

impl App {
    /// Index of the default agent in the picker, or 0 if none is set.
    fn picker_sel(&self) -> usize {
        self.store
            .default_agent
            .and_then(|a| self.available_agents.iter().position(|&x| x == a))
            .unwrap_or(0)
    }

    /// Launch the default agent for `wt_path`, or open the picker if no
    /// default is configured or the saved default is no longer available.
    pub(crate) fn launch_or_pick(&mut self, project: String, wt_path: String) -> Option<usize> {
        self.refresh_available_agents();
        let default = self
            .store
            .default_agent
            .filter(|a| self.available_agents.contains(a));
        if let Some(agent) = default {
            let label = path_basename(&wt_path);
            let args = agent.launch_args(self.skip_permissions_enabled());
            self.spawn_session(
                label,
                project,
                wt_path.clone(),
                agent,
                args,
                &wt_path,
                false,
            )
        } else {
            if let Some(saved) = self.store.default_agent {
                if !self.available_agents.contains(&saved) {
                    self.set_toast(format!("{} not found; pick an agent", saved.label()));
                }
            }
            self.modal = Modal::AgentPicker {
                project,
                wt_path,
                sel: self.picker_sel(),
            };
            None
        }
    }

    /// Compute the index at which a newly spawned session should be inserted
    /// so the sessions list stays grouped by project, and worktrees within a
    /// project follow the project's actual worktree order (rather than the
    /// order sessions happened to be created in).
    pub(crate) fn session_insert_index(&self, s: &Session) -> usize {
        let proj_path = self
            .store
            .projects
            .iter()
            .find(|p| p.name == s.project)
            .map(|p| p.path.clone());
        let wt_order: Vec<String> = match proj_path {
            Some(p) => git::list_worktrees(&p)
                .into_iter()
                .map(|w| w.path)
                .collect(),
            None => Vec::new(),
        };
        let new_pos = wt_order.iter().position(|p| p == &s.wt_path);

        let proj_block: Vec<usize> = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, x)| x.project == s.project)
            .map(|(i, _)| i)
            .collect();
        if proj_block.is_empty() {
            return self.sessions.len();
        }
        if let Some(new_pos) = new_pos {
            for &i in &proj_block {
                let other_pos = wt_order.iter().position(|p| p == &self.sessions[i].wt_path);
                if other_pos.is_some_and(|o| o > new_pos) {
                    return i;
                }
            }
        }
        // `proj_block` is non-empty (checked above), so `last()` always
        // succeeds; the `unwrap_or` arm is unreachable in practice.
        proj_block
            .last()
            .map_or(self.sessions.len(), |last| last + 1)
    }

    /// Spawn an agent in a new embedded PTY session and focus it. Returns the
    /// index in `self.sessions` where the new session was inserted, or `None`
    /// if the spawn failed (an error modal is set).
    ///
    /// When `at_end` is false the session is grouped by project and sorted by
    /// worktree (so the insert can be mid-vector). When `at_end` is true it is
    /// appended after all existing sessions — used by the Agent View launcher
    /// so a freshly launched session always lands last in the grid.
    // One arg over the limit; splitting into a param struct would obscure more than it clarifies.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_session(
        &mut self,
        label: String,
        project: String,
        wt_path: String,
        agent: Agent,
        args: Vec<String>,
        cwd: &str,
        at_end: bool,
    ) -> Option<usize> {
        match Session::spawn(
            label.clone(),
            project,
            wt_path,
            agent,
            &args,
            cwd,
            self.use_tmux(),
        ) {
            Ok(s) => {
                let at = if at_end {
                    self.sessions.len()
                } else {
                    self.session_insert_index(&s)
                };
                let project = s.project.clone();
                let wt_path = s.wt_path.clone();
                self.sessions.insert(at, s);
                self.active_session = Some(at);
                self.set_toast(format!("started {label}"));
                crate::gui::launcher::push_recent_launch(
                    &mut self.store.recent_launches,
                    grove_core::storage::RecentLaunch {
                        project,
                        wt_path,
                        agent,
                    },
                );
                grove_core::storage::persist(&self.store);
                Some(at)
            }
            Err(e) => {
                self.modal = Modal::Message(format!("Failed to start agent: {e}"));
                None
            }
        }
    }

    /// Spawn a lifecycle script (`setup`/`run`) as a focused session tab under
    /// `wt_path`. No-op when the snippet is empty/whitespace.
    pub(crate) fn spawn_script_session(
        &mut self,
        stage: &str,
        project: String,
        wt_path: String,
        script: &str,
    ) {
        let script = script.trim();
        if script.is_empty() {
            return;
        }
        match Session::spawn_script(
            stage.to_string(),
            project,
            wt_path.clone(),
            script,
            &wt_path,
        ) {
            Ok(s) => {
                let at = self.session_insert_index(&s);
                self.sessions.insert(at, s);
                self.active_session = Some(at);
                self.set_toast(format!("running {stage} script"));
            }
            Err(e) => {
                self.set_error_toast(format!("{stage} script failed: {e}"));
            }
        }
    }

    /// Run the project's `run` script in the given worktree, if configured.
    /// Runs as an additional shell in that worktree's terminal panel rather
    /// than a sibling session tab — like an ad hoc terminal, its output
    /// shouldn't clutter the tree or take over the agent view.
    pub(crate) fn run_worktree_script(&mut self, wt_path: &str, rows: u16, cols: u16) {
        let Some(p) = self.selected_project().cloned() else {
            return;
        };
        match p.scripts.run.as_deref() {
            Some(script) if !script.trim().is_empty() => {
                match Session::spawn_script(
                    "run".to_string(),
                    p.name.clone(),
                    wt_path.to_string(),
                    script,
                    wt_path,
                ) {
                    Ok(mut s) => {
                        s.resize(rows, cols);
                        let v = self.wt_terminals.entry(wt_path.to_string()).or_default();
                        v.push(s);
                        self.wt_active_terminal
                            .insert(wt_path.to_string(), v.len() - 1);
                    }
                    Err(e) => self.set_error_toast(format!("run script failed: {e}")),
                }
            }
            _ => self.set_toast("no run script configured for this project"),
        }
    }

    pub(crate) fn create_worktree(&mut self, p: &Project, name: &str) {
        let wt_path = match git::add_worktree(&p.path, &p.name, name) {
            Ok(path) => path,
            Err(e) => {
                crate::telemetry::track("error", vec![("kind", "worktree_failed".into())]);
                self.modal = Modal::Message(format!("Add worktree failed: {e}"));
                return;
            }
        };
        if let Err(e) = git::copy_worktree_includes(&p.path, &wt_path) {
            self.set_error_toast(format!("worktreeinclude: {e}"));
        }
        crate::telemetry::track("worktree_created", vec![]);
        self.refresh_worktrees();
        // Launch the agent first, then the setup script (if any) so the setup
        // tab is spawned last and is the one focused by default — when a setup
        // script exists, the user wants to watch it run before touching the
        // agent. Both tabs coexist.
        self.launch_or_pick(p.name.clone(), wt_path.clone());
        if let Some(setup) = p.scripts.setup.clone() {
            self.spawn_script_session("setup", p.name.clone(), wt_path, &setup);
        }
    }
}
