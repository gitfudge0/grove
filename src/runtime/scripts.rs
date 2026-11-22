//! Runtime launch and lifetime of worktree script commands.

use super::*;

impl Runtime {
    /// Run the configured project script as a native session in this worktree.
    /// Returns true only when a process was started and registered.
    pub fn spawn_run_script(
        &mut self,
        project_path: &str,
        wt_path: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let entry = cx
            .global::<SettingsState>()
            .store
            .projects
            .iter()
            .find(|p| p.path == project_path)
            .and_then(|p| {
                p.scripts.effective_entries().into_iter().find(|e| {
                    matches!(e.kind, grove_core::storage::ProjectActionKind::Shell { .. })
                })
            });
        let Some(entry) = entry else { return false };
        self.spawn_project_script(project_path, wt_path, &entry.id, cx)
    }

    pub fn spawn_project_script(
        &mut self,
        project_path: &str,
        wt_path: &str,
        entry_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let command = cx
            .global::<SettingsState>()
            .store
            .script_project(project_path)
            .scripts
            .effective_entries()
            .into_iter()
            .find(|e| e.id == entry_id)
            .and_then(|e| match e.kind {
                grove_core::storage::ProjectActionKind::Shell { command } => Some(command),
                grove_core::storage::ProjectActionKind::Prompt { .. } => None,
            });
        let Some(command) = command else {
            self.toast.update(cx, |toast, cx| {
                toast.set_error("Shell script is no longer available.", cx);
            });
            return false;
        };
        self.spawn_checkout_command_inner(project_path, wt_path, &command, Some(entry_id), cx)
    }

    pub fn spawn_checkout_command(
        &mut self,
        project_path: &str,
        wt_path: &str,
        command: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        self.spawn_checkout_command_inner(project_path, wt_path, command, None, cx)
    }

    fn spawn_checkout_command_inner(
        &mut self,
        project_path: &str,
        wt_path: &str,
        command: &str,
        script_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> bool {
        let target = (|| -> Result<Option<(String, String, String)>, String> {
            if project_path == wt_path && self.directory_tools_available(wt_path, cx) {
                if command.trim().is_empty() {
                    return Ok(None);
                }
                return Ok(Some((
                    String::new(),
                    wt_path.to_owned(),
                    command.to_owned(),
                )));
            }
            let projects = &cx.global::<SettingsState>().store.projects;
            let project = projects
                .iter()
                .find(|project| project.path == project_path && !project.archived)
                .ok_or_else(|| "Project is no longer available.".to_string())?;
            let script = command;
            if script.trim().is_empty() {
                return Ok(None);
            }
            if self.projects.read(cx).is_removing(&project.path) {
                return Err("Cannot launch a script while this project is being removed.".into());
            }
            let canonical = fs_err::canonicalize(wt_path)
                .map_err(|_| format!("{wt_path}: worktree is no longer available"))?;
            let worktree = canonical.to_string_lossy().into_owned();
            if self.projects.read(cx).is_worktree_removing(&worktree) {
                return Err("Cannot launch a script while this worktree is being removed.".into());
            }
            let owner = self.cached_checkout_owner(&worktree, cx)?;
            if owner.path != project.path {
                return Err(format!("{wt_path}: worktree belongs to another project"));
            }
            Ok(Some((project.name.clone(), worktree, script.to_string())))
        })();
        let (project, cwd, script) = match target {
            Ok(Some(target)) => target,
            Ok(None) => return false,
            Err(error) => {
                self.toast
                    .update(cx, |toast, cx| toast.set_error(error, cx));
                return false;
            }
        };

        let validation_projects = cx.global::<SettingsState>().store.projects.clone();
        let validation_paths = vec![std::path::PathBuf::from(&cwd)];
        let validation_policy = if project.is_empty() {
            CheckoutValidation::ProjectlessDirectory
        } else {
            CheckoutValidation::GitInventory
        };
        let validation_expected = vec![project_path.to_owned()];
        let session = cx.new(|cx| {
            TerminalSession::spawn_script_checked(
                &script,
                &cwd,
                Box::new(move || {
                    validate_checkout_paths(
                        &validation_projects,
                        &validation_paths,
                        validation_policy,
                        &validation_expected,
                    )
                }),
                cx,
            )
        });
        if let Some(error) = session.read(cx).spawn_error().map(str::to_string) {
            self.toast.update(cx, |toast, cx| {
                toast.set_error(format!("terminal failed: {error}"), cx);
            });
            return false;
        }
        if let Some(script_id) = script_id {
            let id = self.next_script_run;
            self.next_script_run += 1;
            let name = cx
                .global::<SettingsState>()
                .store
                .script_project(project_path)
                .scripts
                .effective_entries()
                .into_iter()
                .find(|entry| entry.id == script_id)
                .map_or_else(|| "Script".to_string(), |entry| entry.name);
            self.projects.update(cx, |projects, _| {
                projects.track_inline_script(project_path.to_owned(), cwd.clone(), &session);
            });
            self.script_runs.push(ScriptRun {
                id,
                project_path: project_path.to_owned(),
                checkout_path: cwd,
                script_id: script_id.to_owned(),
                name,
                command: script,
                terminal: session.clone(),
                _observer: std::rc::Rc::new(cx.observe(&session, move |this, terminal, cx| {
                    if let Some(error) = terminal.read(cx).spawn_error().map(str::to_owned) {
                        this.script_runs.retain(|run| run.id != id);
                        this.toast.update(cx, |toast, cx| {
                            toast.set_error(format!("terminal failed: {error}"), cx);
                        });
                    }
                    cx.notify();
                })),
            });
            cx.notify();
            return true;
        }
        self.state.update(cx, |state, cx| {
            state.set_open_agent_menu(None);
            cx.notify();
        });
        let id = self.registry.update(cx, |registry, cx| {
            let id = registry.insert_meta(project, cwd, Agent::Terminal);
            registry.attach_observed(id, session, None, cx);
            cx.notify();
            id
        });
        let snap = self.snapshot(cx);
        let old = self.state.read(cx).proj_idx();
        ProjectTree::adopt_session_project(&self.tree, &snap, id, old, cx);
        self.state.update(cx, |state, cx| {
            state.select_session(id, &snap);
            cx.notify();
        });
        true
    }
}
