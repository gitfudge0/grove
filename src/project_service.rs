//! Project operations formerly owned by modal views. No rendered UI is required.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use gpui::{AppContext as _, Context, Entity, EventEmitter, Task};
use grove_core::{git, storage};

use crate::entities::{
    session_registry::{SessionId, SessionMeta, SessionRegistry},
    terminal_session::TerminalSession,
    workspace_state::WorkspaceState,
};
use crate::settings::SettingsState;

pub enum ProjectEvent {
    TreeInvalidated,
    WorktreeAdded { path: String },
    WorktreeRemoved { result: Result<(), String> },
    WorktreeRemovalChanged { path: String },
    ProjectRemoved { errors: Vec<String> },
    ProjectRemovalChanged { path: String },
}

#[derive(Clone, Debug, Default)]
pub struct ProjectRemoval {
    pub total: usize,
    pub completed: usize,
    pub current_target: Option<String>,
    pub errors: Vec<String>,
    pub finished: bool,
    pub unregistered: bool,
}

impl ProjectRemoval {
    fn complete_target(&mut self, result: Result<(), String>) {
        self.completed += 1;
        self.current_target = None;
        if let Err(error) = result {
            self.errors.push(error);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreeRemovalStage {
    RunningScript,
    Removing,
    Finished,
}

#[derive(Clone, Debug)]
pub struct WorktreeRemoval {
    pub stage: WorktreeRemovalStage,
    pub error: Option<String>,
    project_path: String,
    canonical_path: PathBuf,
}

pub struct ProjectService {
    registry: Entity<SessionRegistry>,
    state: Entity<WorkspaceState>,
    teardown_session: Option<Entity<TerminalSession>>,
    teardown_poll: Option<Task<()>>,
    teardown_target: Option<(String, String)>,
    removals: BTreeMap<String, ProjectRemoval>,
    worktree_removals: BTreeMap<String, WorktreeRemoval>,
}

impl EventEmitter<ProjectEvent> for ProjectService {}

impl ProjectService {
    pub fn new(registry: Entity<SessionRegistry>, state: Entity<WorkspaceState>) -> Self {
        Self {
            registry,
            state,
            teardown_session: None,
            teardown_poll: None,
            teardown_target: None,
            removals: BTreeMap::new(),
            worktree_removals: BTreeMap::new(),
        }
    }

    pub fn register_project(
        &mut self,
        name: String,
        path: String,
        cx: &mut Context<Self>,
    ) -> Result<usize, String> {
        let workspace = cx.global::<SettingsState>().store.workspaces.active;
        self.register_project_in_workspace(name, path, workspace, cx)
    }

    pub fn register_project_in_workspace(
        &mut self,
        name: String,
        path: String,
        workspace: u64,
        cx: &mut Context<Self>,
    ) -> Result<usize, String> {
        let store = &cx.global::<SettingsState>().store;
        let project = validated_registration(store, &name, &path, workspace)?;
        if self.is_worktree_removing(&project.path) {
            return Err(
                "Wait for the worktree removal to finish before registering this project.".into(),
            );
        }
        if self.removals.values().any(|removal| {
            !removal.finished
                && removal.current_target.as_deref().is_some_and(|target| {
                    path_within_worktree(
                        &project.path,
                        target,
                        Path::new(target).canonicalize().ok().as_deref(),
                    )
                })
        }) {
            return Err(
                "Wait for the worktree removal to finish before registering this project.".into(),
            );
        }
        let idx = store.projects.len();
        let ((), saved) = SettingsState::update_and_flush_checked(cx, |store| {
            store
                .project_workspaces
                .insert(project.path.clone(), workspace);
            store.projects.push(project);
        });
        saved.map_err(|error| format!("Could not save project: {error}"))?;
        cx.emit(ProjectEvent::TreeInvalidated);
        Ok(idx)
    }

    pub fn update_project(
        &mut self,
        path: &str,
        name: String,
        scripts: storage::ProjectScripts,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.ensure_not_removing(path)?;
        let store = &cx.global::<SettingsState>().store;
        let idx = project_index(&store.projects, path)
            .ok_or_else(|| "Project no longer exists.".to_string())?;
        let name = validated_name(&store.projects, &name, Some(path))?;
        let old_name = store.projects[idx].name.clone();
        self.archive_blockers(path, cx)?;
        if old_name != name {
            grove_core::session_meta::validate_project_ownership(&store.projects)?;
            if store
                .projects
                .iter()
                .filter(|project| project.name == old_name)
                .count()
                > 1
            {
                for recent in &store.recent_launches {
                    if recent.project == old_name {
                        grove_core::session_meta::project_owner(&store.projects, &recent.wt_path)?;
                    }
                }
                for key in &store.grid_order {
                    if let Some(wt) = key.strip_prefix(&format!("{old_name}::")) {
                        grove_core::session_meta::project_owner(&store.projects, wt)?;
                    }
                }
            }
        }
        let before = store.clone();
        let rename = if old_name != name {
            Some(grove_core::session_meta::ProjectRenamePlan::prepare(
                &before.projects,
                path,
                &name,
            )?)
        } else {
            None
        };
        // Sidecars are applied first; no settings or live metadata changes if this fails.
        if let Some(plan) = &rename {
            plan.apply()?;
        }
        let ((), saved) = SettingsState::update_and_flush_checked(cx, |store| {
            apply_project_update(store, idx, &name, scripts);
        });
        if let Err(error) = saved {
            let rollback = rename.as_ref().map_or(
                Ok(()),
                grove_core::session_meta::ProjectRenamePlan::rollback,
            );
            return Err(rename_failure(
                format!("Could not save project: {error}"),
                rollback,
            ));
        }
        if rename.is_some() {
            let result = self.registry.update(cx, |registry, cx| {
                let result = registry.rename_project_by_path(&before.projects, path, &name);
                if result.is_ok() {
                    cx.notify();
                }
                result
            });
            if let Err(error) = result {
                let ((), restored) =
                    SettingsState::update_and_flush_checked(cx, |store| *store = before);
                let rollback = rename.as_ref().map_or(
                    Ok(()),
                    grove_core::session_meta::ProjectRenamePlan::rollback,
                );
                let error = rename_failure(error, restored.map_err(|error| error.to_string()));
                return Err(rename_failure(error, rollback));
            }
        }
        cx.emit(ProjectEvent::TreeInvalidated);
        Ok(())
    }

    pub fn is_removing(&self, path: &str) -> bool {
        self.removals
            .get(path)
            .is_some_and(|operation| !operation.finished)
    }

    pub fn is_worktree_removing(&self, path: &str) -> bool {
        self.worktree_removals.iter().any(|(target, operation)| {
            operation.stage != WorktreeRemovalStage::Finished
                && path_within_worktree(path, target, Some(&operation.canonical_path))
        })
    }

    pub fn worktree_removal_status(&self, wt_path: &str) -> Option<&WorktreeRemoval> {
        self.worktree_removals.get(wt_path)
    }

    fn ensure_not_removing(&self, path: &str) -> Result<(), String> {
        if self.is_removing(path) {
            Err("This project is already being removed.".into())
        } else if self.worktree_removals.values().any(|operation| {
            operation.project_path == path && operation.stage != WorktreeRemovalStage::Finished
        }) {
            Err("Wait for the worktree removal to finish before changing this project.".into())
        } else {
            Ok(())
        }
    }

    pub fn create_worktree(
        &mut self,
        project: &storage::Project,
        name: &str,
        base: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        self.ensure_not_removing(&project.path)?;
        match git::add_worktree(&project.path, project.worktree_dir(), name, base) {
            Ok(path) => {
                if let Err(e) = git::copy_worktree_includes(&project.path, &path) {
                    tracing::warn!("grove-gpui: worktree includes not copied: {e}");
                }
                crate::telemetry::track("worktree_created", vec![]);
                cx.emit(ProjectEvent::WorktreeAdded { path: path.clone() });
                Ok(path)
            }
            Err(e) => {
                crate::telemetry::track("error", vec![("kind", "worktree_failed".into())]);
                Err(format!("Worktree failed: {e}"))
            }
        }
    }

    pub fn create_worktree_with_branch(
        &mut self,
        project: &storage::Project,
        name: &str,
        branch: &str,
        base: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        self.ensure_not_removing(&project.path)?;
        match git::add_worktree_with_branch(
            &project.path,
            project.worktree_dir(),
            name,
            branch,
            base,
        ) {
            Ok(path) => {
                if let Err(e) = git::copy_worktree_includes(&project.path, &path) {
                    tracing::warn!("grove-gpui: worktree includes not copied: {e}");
                }
                crate::telemetry::track("worktree_created", vec![]);
                cx.emit(ProjectEvent::WorktreeAdded { path: path.clone() });
                Ok(path)
            }
            Err(e) => {
                crate::telemetry::track("error", vec![("kind", "worktree_failed".into())]);
                Err(format!("Worktree failed: {e}"))
            }
        }
    }

    pub fn kill_sessions_for_project(&mut self, project: &str, cx: &mut Context<Self>) {
        self.kill_sessions(|m| m.project == project, cx);
    }

    fn kill_sessions(&mut self, matches: impl Fn(&SessionMeta) -> bool, cx: &mut Context<Self>) {
        let ids: Vec<_> = self
            .registry
            .read(cx)
            .all()
            .iter()
            .filter(|m| matches(m))
            .map(|m| m.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        self.registry.update(cx, |r, cx| {
            for id in &ids {
                r.remove(*id);
            }
            cx.notify();
        });
        self.state.update(cx, |s, cx| {
            for id in &ids {
                s.on_session_removed(*id);
            }
            cx.notify();
        });
    }

    /// Archive is blocked by every registered session, including exited sessions.
    pub fn archive_blockers(
        &self,
        path: &str,
        cx: &Context<Self>,
    ) -> Result<Vec<SessionId>, String> {
        let store = &cx.global::<SettingsState>().store;
        let idx = project_index(&store.projects, path)
            .ok_or_else(|| "Project no longer exists.".to_string())?;
        let mut blockers = Vec::new();
        for meta in self.registry.read(cx).all() {
            if session_touches_project(meta, &store.projects, &store.projects[idx].path)? {
                blockers.push(meta.id);
            }
        }
        Ok(blockers)
    }

    pub fn archive_project_by_path(
        &mut self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.ensure_not_removing(path)?;
        let blockers = self.archive_blockers(path, cx)?;
        if !blockers.is_empty() {
            return Err(format!(
                "Close all {} registered sessions before archiving this project.",
                blockers.len()
            ));
        }
        self.set_archived(path, true, cx)
    }

    pub fn restore_archived_by_path(
        &mut self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.ensure_not_removing(path)?;
        self.set_archived(path, false, cx)
    }

    fn set_archived(
        &mut self,
        path: &str,
        archived: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let idx = project_index(&cx.global::<SettingsState>().store.projects, path)
            .ok_or_else(|| "Project no longer exists.".to_string())?;
        let ((), saved) = SettingsState::update_and_flush_checked(cx, |store| {
            store.projects[idx].archived = archived;
        });
        saved.map_err(|error| format!("Could not save project: {error}"))?;
        cx.emit(ProjectEvent::TreeInvalidated);
        Ok(())
    }

    pub fn archive_project(&mut self, idx: usize, cx: &mut Context<Self>) -> bool {
        let path = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .map(|p| p.path.clone());
        path.is_some_and(|path| self.archive_project_by_path(&path, cx).is_ok())
    }

    pub fn restore_archived(&mut self, idx: usize, cx: &mut Context<Self>) {
        if let Some(path) = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .map(|p| p.path.clone())
        {
            let _ = self.restore_archived_by_path(&path, cx);
        }
    }

    pub fn delete_archived_by_path(
        &mut self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.ensure_not_removing(path)?;
        let store = &cx.global::<SettingsState>().store;
        let idx = project_index(&store.projects, path)
            .ok_or_else(|| "Project no longer exists.".to_string())?;
        if !store.projects[idx].archived {
            return Err("Archive the project before permanently deleting its registration.".into());
        }
        if !self.archive_blockers(path, cx)?.is_empty() {
            return Err("Close all registered sessions before deleting this project.".into());
        }
        self.unregister_project(path, cx)
    }

    pub fn delete_archived(&mut self, idx: usize, cx: &mut Context<Self>) {
        if let Some(path) = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .map(|p| p.path.clone())
        {
            let _ = self.delete_archived_by_path(&path, cx);
        }
    }

    fn unregister_project(&mut self, path: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let idx = project_index(&cx.global::<SettingsState>().store.projects, path)
            .ok_or_else(|| "Project no longer exists.".to_string())?;
        let ((), saved) = SettingsState::update_and_flush_checked(cx, |store| {
            store.projects.remove(idx);
            store.project_workspaces.remove(path);
        });
        saved.map_err(|error| format!("Could not remove project registration: {error}"))?;
        self.state.update(cx, |state, cx| {
            state.on_project_removed(idx);
            cx.notify();
        });
        cx.emit(ProjectEvent::TreeInvalidated);
        Ok(())
    }

    pub fn removal_status(&self, path: &str) -> Option<&ProjectRemoval> {
        self.removals.get(path)
    }

    pub fn remove_project(
        &mut self,
        idx: usize,
        also_remove_worktrees: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .map(|p| p.path.clone())
        {
            let _ = self.remove_project_by_path(&path, also_remove_worktrees, cx);
        }
    }

    pub fn remove_project_by_path(
        &mut self,
        path: &str,
        also_remove_worktrees: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.ensure_not_removing(path)?;
        let store = &cx.global::<SettingsState>().store;
        let idx = project_index(&store.projects, path)
            .ok_or_else(|| "Project no longer exists.".to_string())?;
        let project = store.projects[idx].clone();
        self.archive_blockers(path, cx)?;
        let path = project.path.clone();
        self.removals
            .insert(path.clone(), ProjectRemoval::default());
        self.kill_sessions_for_project_path(&project.path, cx)?;
        self.removal_changed(&path, cx);
        if !also_remove_worktrees {
            self.finish_removal(&path, cx);
            return Ok(());
        }
        cx.spawn(async move |this, cx| {
            let repository = project.path.clone();
            let targets = cx
                .background_executor()
                .spawn(async move {
                    git::list_worktrees_checked(&repository).map(|worktrees| {
                        worktrees
                            .into_iter()
                            .filter(|worktree| {
                                removable_worktree(&repository, &worktree.path, worktree.is_main)
                            })
                            .map(|worktree| worktree.path)
                            .collect::<Vec<_>>()
                    })
                })
                .await;
            let targets = match targets {
                Ok(targets) => targets,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(operation) = this.removals.get_mut(&path) {
                            operation
                                .errors
                                .push(format!("Could not list worktrees: {error}"));
                        }
                        this.finish_removal(&path, cx);
                    });
                    return;
                }
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(operation) = this.removals.get_mut(&path) {
                    operation.total = targets.len();
                }
                this.removal_changed(&path, cx);
            });
            for target in targets {
                let _ = this.update(cx, |this, cx| {
                    if let Some(operation) = this.removals.get_mut(&path) {
                        operation.current_target = Some(target.clone());
                    }
                    this.removal_changed(&path, cx);
                });
                let protected = this.update(cx, |_, cx| {
                    let canonical = Path::new(&target)
                        .canonicalize()
                        .map_err(|error| format!("Could not resolve worktree path: {error}"))?;
                    Ok::<_, String>(registered_project_at_path(
                        &cx.global::<SettingsState>().store.projects,
                        &target,
                        &canonical,
                    )
                    .map(|project| project.name.clone()))
                });
                let result = match protected {
                    Ok(Ok(Some(name))) => Err(format!(
                        "{target}: contains registered project '{name}'; remove that registration before deleting this worktree"
                    )),
                    Ok(Err(error)) => Err(format!("{target}: {error}")),
                    Err(_) => return,
                    Ok(Ok(None)) => {
                        let repository = project.path.clone();
                        let target_path = target.clone();
                        cx.background_executor()
                            .spawn(async move {
                                git::remove_worktree(&repository, &target_path)
                                    .map_err(|error| format!("{target_path}: {error}"))
                            })
                            .await
                    }
                };
                let _ = this.update(cx, |this, cx| {
                    if let Some(operation) = this.removals.get_mut(&path) {
                        operation.complete_target(result);
                    }
                    this.removal_changed(&path, cx);
                });
            }
            let _ = this.update(cx, |this, cx| this.finish_removal(&path, cx));
        })
        .detach();
        Ok(())
    }

    fn removal_changed(&self, path: &str, cx: &mut Context<Self>) {
        cx.emit(ProjectEvent::ProjectRemovalChanged {
            path: path.to_string(),
        });
        cx.notify();
    }

    fn sweep_removal_sessions(&mut self, path: &str, cx: &mut Context<Self>) -> Result<(), String> {
        self.kill_sessions_for_project_path(path, cx)
    }

    pub fn kill_sessions_for_project_path(
        &mut self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let ids = self.archive_blockers(path, cx)?;
        self.kill_sessions(|meta| ids.contains(&meta.id), cx);
        Ok(())
    }

    fn finish_removal(&mut self, path: &str, cx: &mut Context<Self>) {
        let result = self
            .sweep_removal_sessions(path, cx)
            .and_then(|()| self.unregister_project(path, cx));
        if let Some(operation) = self.removals.get_mut(path) {
            operation.finished = true;
            operation.current_target = None;
            operation.unregistered = result.is_ok();
            if let Err(error) = result {
                operation.errors.push(error);
            }
            cx.emit(ProjectEvent::ProjectRemoved {
                errors: operation.errors.clone(),
            });
        }
        self.removal_changed(path, cx);
    }

    /// Validate against Git's checked inventory before touching any registered session.
    pub fn remove_worktree_by_path(
        &mut self,
        project_path: &str,
        wt_path: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.ensure_not_removing(project_path)?;
        if self.teardown_target.is_some()
            || self
                .worktree_removals
                .values()
                .any(|operation| operation.stage != WorktreeRemovalStage::Finished)
        {
            return Err("Another worktree is already being removed.".into());
        }
        let project = cx
            .global::<SettingsState>()
            .store
            .projects
            .iter()
            .find(|project| project.path == project_path)
            .cloned()
            .ok_or_else(|| "Project no longer exists.".to_string())?;
        let worktrees = git::list_worktrees_checked(project_path)
            .map_err(|error| format!("Could not list worktrees: {error}"))?;
        let target = worktrees
            .iter()
            .find(|worktree| worktree.path == wt_path)
            .ok_or_else(|| "Worktree is no longer listed by Git.".to_string())?;
        if !removable_worktree(project_path, &target.path, target.is_main) {
            return Err("The main project checkout cannot be removed.".into());
        }
        let canonical_path = Path::new(&target.path)
            .canonicalize()
            .map_err(|error| format!("Could not resolve worktree path: {error}"))?;
        if let Some(registered) = registered_project_at_path(
            &cx.global::<SettingsState>().store.projects,
            &target.path,
            &canonical_path,
        ) {
            return Err(format!(
                "This worktree contains registered project '{}'. Remove that project registration before deleting its folder.",
                registered.name
            ));
        }
        self.worktree_removals.insert(
            wt_path.to_string(),
            WorktreeRemoval {
                stage: WorktreeRemovalStage::RunningScript,
                error: None,
                project_path: project_path.to_string(),
                canonical_path,
            },
        );
        self.kill_sessions_touching_worktree(wt_path, cx);
        self.teardown_target = Some((project_path.to_string(), wt_path.to_string()));
        self.worktree_removal_changed(wt_path, cx);

        let Some(script) = project
            .scripts
            .teardown
            .as_deref()
            .map(str::trim)
            .filter(|script| !script.is_empty())
        else {
            self.skip_worktree_teardown(cx);
            return Ok(());
        };
        let session = cx.new(|cx| TerminalSession::spawn_script(script, wt_path, cx));
        if let Some(error) = session.read(cx).spawn_error() {
            if let Some(operation) = self.worktree_removals.get_mut(wt_path) {
                operation.error = Some(format!("Teardown could not start: {error}"));
            }
            self.worktree_removal_changed(wt_path, cx);
            return Ok(());
        }
        self.teardown_session = Some(session.clone());
        // TerminalSession exposes liveness but not an exit code; a script that starts and then
        // exits nonzero is treated as complete. Spawn failures stay visible for explicit skip.
        self.teardown_poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(120))
                .await;
            if session.update(cx, |session, _| session.alive()) {
                continue;
            }
            let _ = this.update(cx, ProjectService::skip_worktree_teardown);
            return;
        }));
        Ok(())
    }

    fn kill_sessions_touching_worktree(&mut self, path: &str, cx: &mut Context<Self>) {
        let canonical_path = self
            .worktree_removals
            .get(path)
            .map(|operation| operation.canonical_path.clone())
            .or_else(|| Path::new(path).canonicalize().ok());
        self.kill_sessions(
            |meta| {
                path_within_worktree(&meta.wt_path, path, canonical_path.as_deref())
                    || meta.context_roots.iter().any(|root| {
                        path_within_worktree(&root.wt_path, path, canonical_path.as_deref())
                    })
            },
            cx,
        );
    }

    fn worktree_removal_changed(&self, path: &str, cx: &mut Context<Self>) {
        cx.emit(ProjectEvent::WorktreeRemovalChanged {
            path: path.to_string(),
        });
        cx.notify();
    }

    /// Skipping cancels the owned script PTY, then proceeds with Git removal once.
    pub fn skip_worktree_teardown(&mut self, cx: &mut Context<Self>) {
        let Some((project_path, wt_path)) = self.teardown_target.take() else {
            return;
        };
        self.teardown_poll = None;
        self.teardown_session = None;
        if let Some(operation) = self.worktree_removals.get_mut(&wt_path) {
            operation.stage = WorktreeRemovalStage::Removing;
            // An explicit skip acknowledges a script start failure; only Git removal errors
            // determine whether the finished operation failed.
            operation.error = None;
        }
        self.worktree_removal_changed(&wt_path, cx);
        self.kill_sessions_touching_worktree(&wt_path, cx);
        cx.spawn(async move |this, cx| {
            let project = project_path.clone();
            let target = wt_path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    git::remove_worktree(&project, &target)
                        .map_err(|error| format!("{target}: {error}"))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.kill_sessions_touching_worktree(&wt_path, cx);
                if let Some(operation) = this.worktree_removals.get_mut(&wt_path) {
                    operation.stage = WorktreeRemovalStage::Finished;
                    if let Err(error) = &result {
                        operation.error = Some(match operation.error.take() {
                            Some(previous) => format!("{previous}; {error}"),
                            None => error.clone(),
                        });
                    }
                }
                cx.emit(ProjectEvent::TreeInvalidated);
                cx.emit(ProjectEvent::WorktreeRemoved { result });
                this.worktree_removal_changed(&wt_path, cx);
            });
        })
        .detach();
    }
}

fn rename_failure(error: String, rollback: Result<(), String>) -> String {
    match rollback {
        Ok(()) => error,
        Err(rollback) => format!("{error}; rollback failed: {rollback}"),
    }
}

fn session_touches_project(
    meta: &SessionMeta,
    projects: &[storage::Project],
    path: &str,
) -> Result<bool, String> {
    let mut touches = false;
    for wt in std::iter::once(meta.wt_path.as_str())
        .chain(meta.context_roots.iter().map(|root| root.wt_path.as_str()))
    {
        touches |= grove_core::session_meta::project_owner(projects, wt)?.path == path;
    }
    Ok(touches)
}

/// Resolve by repository identity; names and indices can change during a dialog.
fn project_index(projects: &[storage::Project], path: &str) -> Option<usize> {
    projects.iter().position(|project| project.path == path)
}

fn validated_name(
    projects: &[storage::Project],
    name: &str,
    except_path: Option<&str>,
) -> Result<String, String> {
    let name = name.trim();
    if !git::valid_project_name(name) {
        return Err(format!(
            "'{name}' isn't a valid project name; avoid path separators, leading '-', and '..'"
        ));
    }
    if projects.iter().any(|project| {
        Some(project.path.as_str()) != except_path && project.name.eq_ignore_ascii_case(name)
    }) {
        return Err(format!(
            "A project named '{name}' already exists, including archived projects."
        ));
    }
    Ok(name.to_string())
}

fn validated_registration(
    store: &storage::Store,
    name: &str,
    path: &str,
    workspace: u64,
) -> Result<storage::Project, String> {
    let name = validated_name(&store.projects, name, None)?;
    if !store.workspaces.rows.iter().any(|row| row.id == workspace) {
        return Err("The selected workspace no longer exists.".into());
    }
    let canonical = Path::new(path)
        .canonicalize()
        .map_err(|error| format!("Cannot open project directory: {error}"))?;
    if !canonical.is_dir() {
        return Err("Choose a directory for the project.".into());
    }
    if store.projects.iter().any(|project| {
        Path::new(&project.path)
            .canonicalize()
            .is_ok_and(|existing| existing == canonical)
    }) {
        return Err("This project directory is already registered.".into());
    }
    let path = canonical
        .to_str()
        .ok_or_else(|| "The project path contains unsupported characters.".to_string())?
        .to_string();
    Ok(storage::Project {
        name,
        path,
        scripts: storage::ProjectScripts::default(),
        archived: false,
        worktree_dir: None,
    })
}

fn apply_project_update(
    store: &mut storage::Store,
    idx: usize,
    name: &str,
    scripts: storage::ProjectScripts,
) {
    let before = store.projects.clone();
    let target = before[idx].path.clone();
    let project = &mut store.projects[idx];
    let old = project.name.clone();
    let duplicated = before
        .iter()
        .filter(|candidate| candidate.name == old)
        .count()
        > 1;
    let matches_path = |wt: &str| {
        !duplicated
            || grove_core::session_meta::project_owner(&before, wt)
                .is_ok_and(|owner| owner.path == target)
    };
    if old != name {
        storage::pin_worktree_dir_on_rename(project, &old);
        project.name = name.to_string();
        for recent in &mut store.recent_launches {
            if recent.project == old && matches_path(&recent.wt_path) {
                recent.project = name.to_string();
            }
        }
        let prefix = format!("{old}::");
        for key in &mut store.grid_order {
            if let Some(suffix) = key.strip_prefix(&prefix) {
                if matches_path(suffix) {
                    *key = format!("{name}::{suffix}");
                }
            }
        }
    }
    project.scripts = scripts;
}

fn path_within_worktree(path: &str, target: &str, canonical_target: Option<&Path>) -> bool {
    Path::new(path).starts_with(target)
        || canonical_target.is_some_and(|target| {
            Path::new(path)
                .canonicalize()
                .is_ok_and(|path| path.starts_with(target))
        })
}

fn registered_project_at_path<'a>(
    projects: &'a [storage::Project],
    target: &str,
    canonical_target: &Path,
) -> Option<&'a storage::Project> {
    projects
        .iter()
        .find(|project| path_within_worktree(&project.path, target, Some(canonical_target)))
}

fn removable_worktree(repository: &str, target: &str, is_main: bool) -> bool {
    !is_main
        && Path::new(repository) != Path::new(target)
        && match (
            Path::new(repository).canonicalize(),
            Path::new(target).canonicalize(),
        ) {
            (Ok(main), Ok(candidate)) => main != candidate,
            _ => true,
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs_err as fs;
    use std::{path::PathBuf, process::Command, time::SystemTime};

    struct GitFixture(PathBuf);

    impl Drop for GitFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn project(name: &str, path: &str) -> storage::Project {
        storage::Project {
            name: name.into(),
            path: path.into(),
            scripts: storage::ProjectScripts::default(),
            archived: false,
            worktree_dir: None,
        }
    }

    fn git_fixture() -> Option<(GitFixture, String, String)> {
        if !Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            return None;
        }
        let root = std::env::temp_dir().join(format!(
            "grove-worktree-removal-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let repo = root.join("repo");
        let wt = root.join("feature");
        fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        let repo_path = repo.to_str().unwrap();
        let wt_path = wt.to_str().unwrap();
        git(&["-C", repo_path, "init", "-q"]);
        fs::write(repo.join("README.md"), "fixture").unwrap();
        git(&["-C", repo_path, "add", "."]);
        git(&[
            "-C",
            repo_path,
            "-c",
            "user.name=grove-test",
            "-c",
            "user.email=grove-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "fixture",
        ]);
        git(&["-C", repo_path, "worktree", "add", "--detach", wt_path]);
        Some((GitFixture(root), repo_path.to_string(), wt_path.to_string()))
    }

    #[test]
    fn pending_removal_tracks_target_after_another_project_is_removed() {
        let mut projects = vec![project("first", "/first"), project("target", "/target")];
        let target = "/target";
        projects.remove(0);
        assert_eq!(project_index(&projects, target), Some(0));
        projects.remove(0);
        projects.push(project("replacement", "/replacement"));
        assert_eq!(project_index(&projects, target), None);
    }
    #[test]
    fn names_trim_and_collide_with_archived_projects_case_insensitively() {
        let mut existing = project("Grove", "/grove");
        existing.archived = true;
        let projects = vec![existing];
        assert!(validated_name(&projects, " grove ", None).is_err());
        assert!(validated_name(&projects, "bad/name", None).is_err());
        assert!(validated_name(&projects, " ", None).is_err());
        assert_eq!(
            validated_name(&projects, " Grove ", Some("/grove")).unwrap(),
            "Grove"
        );
        assert_eq!(
            validated_name(&projects, " valid-name ", None).unwrap(),
            "valid-name"
        );
    }

    #[test]
    fn registration_validates_directory_canonical_identity_and_workspace() {
        let directory = std::env::current_dir().unwrap();
        let path = directory.to_str().unwrap();
        let mut store = storage::Store::default();
        let workspace = store.workspaces.active;
        let registered = validated_registration(&store, " repo ", path, workspace).unwrap();
        assert_eq!(registered.name, "repo");
        assert_eq!(
            Path::new(&registered.path),
            directory.canonicalize().unwrap()
        );
        store.projects.push(registered);
        assert!(
            validated_registration(&store, "different", &format!("{path}/."), workspace).is_err()
        );
        assert!(validated_registration(&store, "other", path, u64::MAX).is_err());
        assert!(
            validated_registration(&store, "other", &format!("{path}/missing"), workspace).is_err()
        );
        let file = std::env::current_exe().unwrap();
        assert!(validated_registration(&store, "file", file.to_str().unwrap(), workspace).is_err());
    }

    #[test]
    fn rename_preserves_worktree_identity_and_rewrites_only_exact_project_keys() {
        let mut store = storage::Store {
            projects: vec![project("old", "/repo")],
            grid_order: vec!["old::/repo::session".into(), "older::/other".into()],
            recent_launches: vec![
                storage::RecentLaunch {
                    project: "old".into(),
                    wt_path: "/repo".into(),
                    agent: grove_core::agent::Agent::Codex,
                },
                storage::RecentLaunch {
                    project: "older".into(),
                    wt_path: "/other".into(),
                    agent: grove_core::agent::Agent::Codex,
                },
            ],
            ..Default::default()
        };
        let scripts = storage::ProjectScripts {
            setup: Some("setup".into()),
            run: Some("run".into()),
            teardown: Some("teardown".into()),
        };
        apply_project_update(&mut store, 0, "new", scripts);
        assert_eq!(store.projects[0].name, "new");
        assert_eq!(store.projects[0].worktree_dir(), "old");
        assert_eq!(store.projects[0].scripts.run.as_deref(), Some("run"));
        assert_eq!(store.projects[0].scripts.setup.as_deref(), Some("setup"));
        assert_eq!(
            store.projects[0].scripts.teardown.as_deref(),
            Some("teardown")
        );
        assert_eq!(store.grid_order, ["new::/repo::session", "older::/other"]);
        assert_eq!(store.recent_launches[0].project, "new");
        assert_eq!(store.recent_launches[1].project, "older");
        assert_eq!(project_index(&store.projects, "/repo"), Some(0));
        apply_project_update(&mut store, 0, "third", storage::ProjectScripts::default());
        assert_eq!(store.projects[0].worktree_dir(), "old");
        assert_eq!(store.grid_order[0], "third::/repo::session");
    }

    #[test]
    fn removal_progress_counts_failed_attempts_and_retains_errors() {
        let mut operation = ProjectRemoval {
            total: 2,
            current_target: Some("/first".into()),
            ..Default::default()
        };
        operation.complete_target(Err("/first: busy".into()));
        assert_eq!(operation.completed, 1);
        assert!(operation.current_target.is_none());
        assert_eq!(operation.errors, ["/first: busy"]);
        operation.current_target = Some("/second".into());
        operation.complete_target(Ok(()));
        assert_eq!(operation.completed, operation.total);
        assert_eq!(operation.errors, ["/first: busy"]);
        assert!(!operation.finished);
    }

    #[test]
    fn removal_never_targets_main_checkout_even_if_flag_is_wrong() {
        let directory = std::env::current_dir().unwrap();
        let path = directory.to_str().unwrap();
        assert!(!removable_worktree(path, path, false));
        assert!(!removable_worktree(path, &format!("{path}/."), false));
        assert!(!removable_worktree(path, "/some-worktree", true));
        assert!(removable_worktree(path, "/some-worktree", false));
    }

    #[gpui::test]
    fn worktree_removal_validates_then_sweeps_all_roots_and_blocks_launches(
        cx: &mut gpui::TestAppContext,
    ) {
        let Some((fixture, repo, wt)) = git_fixture() else {
            return;
        };
        let alias = fixture.0.join("worktree-alias");
        std::os::unix::fs::symlink(&wt, &alias).unwrap();
        let alias = alias.to_str().unwrap().to_string();
        let service = cx.update(|cx| {
            cx.set_global(SettingsState::new(storage::Store {
                projects: vec![project("fixture", &repo)],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let runtime = cx.new(crate::runtime::Runtime::new);
            let service = runtime.read(cx).projects.clone();
            let registry = runtime.read(cx).registry.clone();
            let (primary, context, alias_primary, alias_context, main) =
                registry.update(cx, |registry, _| {
                    let primary = registry.insert_meta(
                        "fixture".into(),
                        wt.clone(),
                        grove_core::agent::Agent::Terminal,
                    );
                    let context = registry.insert_meta_with_context(
                        "fixture".into(),
                        repo.clone(),
                        grove_core::agent::Agent::Codex,
                        vec![grove_core::session_meta::ContextRoot {
                            project: "fixture".into(),
                            wt_path: wt.clone(),
                        }],
                        None,
                    );
                    let alias_primary = registry.insert_meta(
                        "fixture".into(),
                        alias.clone(),
                        grove_core::agent::Agent::Terminal,
                    );
                    let alias_context = registry.insert_meta_with_context(
                        "fixture".into(),
                        repo.clone(),
                        grove_core::agent::Agent::Codex,
                        vec![grove_core::session_meta::ContextRoot {
                            project: "fixture".into(),
                            wt_path: alias.clone(),
                        }],
                        None,
                    );
                    let main = registry.insert_meta(
                        "fixture".into(),
                        repo.clone(),
                        grove_core::agent::Agent::Terminal,
                    );
                    (primary, context, alias_primary, alias_context, main)
                });
            service.update(cx, |service, cx| {
                assert!(service.remove_worktree_by_path(&repo, &repo, cx).is_err());
                assert!(service
                    .remove_worktree_by_path(&repo, &format!("{repo}/."), cx)
                    .is_err());
                assert!(service
                    .remove_worktree_by_path(&repo, &format!("{wt}/."), cx)
                    .is_err());
                assert!(service
                    .remove_worktree_by_path(&repo, "/unlisted-worktree", cx)
                    .is_err());
            });
            assert!(registry.read(cx).meta(primary).is_some());
            assert!(registry.read(cx).meta(context).is_some());
            assert!(registry.read(cx).meta(alias_primary).is_some());
            assert!(registry.read(cx).meta(alias_context).is_some());
            service.update(cx, |service, cx| {
                service.remove_worktree_by_path(&repo, &wt, cx).unwrap();
                assert_eq!(
                    service.worktree_removal_status(&wt).unwrap().stage,
                    WorktreeRemovalStage::Removing,
                );
                assert!(service.remove_worktree_by_path(&repo, &wt, cx).is_err());
                assert!(service.archive_project_by_path(&repo, cx).is_err());
                assert!(service
                    .register_project("late registration".into(), alias.clone(), cx)
                    .is_err());
            });
            assert!(registry.read(cx).meta(primary).is_none());
            assert!(registry.read(cx).meta(context).is_none());
            assert!(registry.read(cx).meta(alias_primary).is_none());
            assert!(registry.read(cx).meta(alias_context).is_none());
            assert!(registry.read(cx).meta(main).is_some());
            let blocked_primary = runtime.update(cx, |runtime, cx| {
                runtime.spawn_session_in_with_args(
                    "fixture".into(),
                    wt.clone(),
                    grove_core::agent::Agent::Terminal,
                    Vec::new(),
                    cx,
                )
            });
            assert!(!blocked_primary);
            let blocked_context = runtime.update(cx, |runtime, cx| {
                runtime.spawn_session_in_with_context(
                    "fixture".into(),
                    repo.clone(),
                    grove_core::agent::Agent::Codex,
                    Vec::new(),
                    vec![grove_core::session_meta::ContextRoot {
                        project: "fixture".into(),
                        wt_path: wt.clone(),
                    }],
                    None,
                    cx,
                )
            });
            assert!(!blocked_context);
            service
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let status = service.read(cx).worktree_removal_status(&wt).unwrap();
            assert_eq!(status.stage, WorktreeRemovalStage::Finished);
            assert!(status.error.is_none(), "{:?}", status.error);
            assert!(!git::list_worktrees_checked(&repo)
                .unwrap()
                .iter()
                .any(|w| w.path == wt));
            assert!(!Path::new(&wt).exists());
        });
    }

    #[gpui::test]
    fn worktree_removal_refuses_another_registered_project_root(cx: &mut gpui::TestAppContext) {
        let Some((fixture, repo, wt)) = git_fixture() else {
            return;
        };
        let alias = fixture.0.join("registered-project-alias");
        std::os::unix::fs::symlink(&wt, &alias).unwrap();
        let alias = alias.to_str().unwrap().to_string();
        cx.update(|cx| {
            let mut other = project("other project", &alias);
            other.archived = true;
            cx.set_global(SettingsState::new(storage::Store {
                projects: vec![project("source", &repo), other],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let runtime = cx.new(crate::runtime::Runtime::new);
            let service = runtime.read(cx).projects.clone();
            let registry = runtime.read(cx).registry.clone();
            let id = registry.update(cx, |registry, _| {
                registry.insert_meta(
                    "other project".into(),
                    wt.clone(),
                    grove_core::agent::Agent::Terminal,
                )
            });
            let error = service.update(cx, |service, cx| {
                service.remove_worktree_by_path(&repo, &wt, cx).unwrap_err()
            });
            assert!(error.contains("other project"), "{error}");
            assert!(service.read(cx).worktree_removal_status(&wt).is_none());
            assert!(registry.read(cx).meta(id).is_some());
            assert!(Path::new(&wt).exists());
            assert!(git::list_worktrees_checked(&repo)
                .unwrap()
                .iter()
                .any(|entry| entry.path == wt));
        });
    }

    #[gpui::test]
    fn worktree_removal_refuses_active_project_nested_inside_target(cx: &mut gpui::TestAppContext) {
        let Some((_fixture, repo, wt)) = git_fixture() else {
            return;
        };
        let nested = Path::new(&wt).join("subproject");
        fs::create_dir_all(&nested).unwrap();
        let nested = nested.to_str().unwrap().to_string();
        cx.update(|cx| {
            cx.set_global(SettingsState::new(storage::Store {
                projects: vec![project("source", &repo), project("nested", &nested)],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let runtime = cx.new(crate::runtime::Runtime::new);
            let service = runtime.read(cx).projects.clone();
            let registry = runtime.read(cx).registry.clone();
            let id = registry.update(cx, |registry, _| {
                registry.insert_meta(
                    "nested".into(),
                    nested.clone(),
                    grove_core::agent::Agent::Terminal,
                )
            });
            let error = service.update(cx, |service, cx| {
                service.remove_worktree_by_path(&repo, &wt, cx).unwrap_err()
            });
            assert!(error.contains("nested"), "{error}");
            assert!(service.read(cx).worktree_removal_status(&wt).is_none());
            assert!(registry.read(cx).meta(id).is_some());
            assert!(Path::new(&nested).exists());
            assert!(Path::new(&wt).exists());
        });
    }

    #[gpui::test]
    fn bulk_project_removal_preserves_registered_worktree_and_continues(
        cx: &mut gpui::TestAppContext,
    ) {
        // Project removal persists settings, so isolate its config in a child process.
        if std::env::var_os("GROVE_BULK_REMOVAL_TEST_CHILD").is_none() {
            let config = std::env::temp_dir().join(format!(
                "grove-bulk-removal-test-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&config).unwrap();
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "project_service::tests::bulk_project_removal_preserves_registered_worktree_and_continues",
                    "--nocapture",
                ])
                .env("GROVE_BULK_REMOVAL_TEST_CHILD", "1")
                .env("GROVE_CONFIG_DIR", &config)
                .output()
                .unwrap();
            let _ = fs::remove_dir_all(&config);
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let Some((fixture, repo, protected)) = git_fixture() else {
            return;
        };
        let safe = fixture.0.join("ordinary-worktree");
        let safe = safe.to_str().unwrap().to_string();
        let output = Command::new("git")
            .args(["-C", &repo, "worktree", "add", "--detach", &safe])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let nested = Path::new(&protected).join("archived-subproject");
        fs::create_dir_all(&nested).unwrap();
        let alias = fixture.0.join("registered-project-alias");
        std::os::unix::fs::symlink(&nested, &alias).unwrap();
        let mut other = project("other project", alias.to_str().unwrap());
        other.archived = true;
        let service = cx.update(|cx| {
            cx.set_global(SettingsState::new(storage::Store {
                projects: vec![project("source", &repo), other],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let runtime = cx.new(crate::runtime::Runtime::new);
            let service = runtime.read(cx).projects.clone();
            service.update(cx, |service, cx| {
                service.remove_project_by_path(&repo, true, cx).unwrap();
            });
            service
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let operation = service.read(cx).removal_status(&repo).unwrap();
            assert!(operation.finished && operation.unregistered);
            assert_eq!((operation.completed, operation.total), (2, 2));
            assert_eq!(operation.errors.len(), 1, "{:?}", operation.errors);
            assert!(operation.errors[0].contains("other project"));
            assert!(operation.errors[0].contains(&protected));
            let remaining = &cx.global::<SettingsState>().store.projects;
            assert_eq!(remaining.len(), 1);
            assert_eq!(remaining[0].name, "other project");
            assert!(remaining[0].archived);
            assert!(Path::new(&protected).exists());
            assert!(nested.exists());
            assert!(!Path::new(&safe).exists());
            let listed = git::list_worktrees_checked(&repo).unwrap();
            assert!(listed.iter().any(|entry| entry.path == protected));
            assert!(!listed.iter().any(|entry| entry.path == safe));
        });
    }

    #[gpui::test]
    fn worktree_teardown_spawn_failure_can_be_skipped_once(cx: &mut gpui::TestAppContext) {
        let Some((fixture, repo, wt)) = git_fixture() else {
            return;
        };
        let alias = fixture.0.join("late-session-alias");
        std::os::unix::fs::symlink(&wt, &alias).unwrap();
        let alias = alias.to_str().unwrap().to_string();
        let service = cx.update(|cx| {
            let mut configured = project("fixture", &repo);
            configured.scripts.teardown = Some("\0".into());
            cx.set_global(SettingsState::new(storage::Store {
                projects: vec![configured],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let runtime = cx.new(crate::runtime::Runtime::new);
            let service = runtime.read(cx).projects.clone();
            let registry = runtime.read(cx).registry.clone();
            service.update(cx, |service, cx| {
                service.remove_worktree_by_path(&repo, &wt, cx).unwrap();
                assert_eq!(
                    service.worktree_removal_status(&wt).unwrap().stage,
                    WorktreeRemovalStage::RunningScript,
                );
                assert!(service
                    .worktree_removal_status(&wt)
                    .unwrap()
                    .error
                    .is_some());
                let (late_primary, late_context) = registry.update(cx, |registry, _| {
                    let primary = registry.insert_meta(
                        "fixture".into(),
                        alias.clone(),
                        grove_core::agent::Agent::Terminal,
                    );
                    let context = registry.insert_meta_with_context(
                        "fixture".into(),
                        repo.clone(),
                        grove_core::agent::Agent::Codex,
                        vec![grove_core::session_meta::ContextRoot {
                            project: "fixture".into(),
                            wt_path: alias.clone(),
                        }],
                        None,
                    );
                    (primary, context)
                });
                service.skip_worktree_teardown(cx);
                service.skip_worktree_teardown(cx);
                assert!(registry.read(cx).meta(late_primary).is_none());
                assert!(registry.read(cx).meta(late_context).is_none());
                assert_eq!(
                    service.worktree_removal_status(&wt).unwrap().stage,
                    WorktreeRemovalStage::Removing,
                );
            });
            service
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let status = service.read(cx).worktree_removal_status(&wt).unwrap();
            assert_eq!(status.stage, WorktreeRemovalStage::Finished);
            assert!(status.error.is_none());
            assert!(!Path::new(&wt).exists());
        });
    }

    #[gpui::test]
    fn failed_worktree_removal_keeps_inspectable_error(cx: &mut gpui::TestAppContext) {
        let Some((_fixture, repo, wt)) = git_fixture() else {
            return;
        };
        let locked = Command::new("git")
            .args(["-C", &repo, "worktree", "lock", &wt])
            .status()
            .unwrap();
        assert!(locked.success());
        let service = cx.update(|cx| {
            cx.set_global(SettingsState::new(storage::Store {
                projects: vec![project("fixture", &repo)],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let runtime = cx.new(crate::runtime::Runtime::new);
            let service = runtime.read(cx).projects.clone();
            service.update(cx, |service, cx| {
                service.remove_worktree_by_path(&repo, &wt, cx).unwrap();
            });
            service
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let status = service.read(cx).worktree_removal_status(&wt).unwrap();
            assert_eq!(status.stage, WorktreeRemovalStage::Finished);
            assert!(status
                .error
                .as_ref()
                .is_some_and(|error| error.contains("lock")));
            assert!(Path::new(&wt).exists());
        });
    }
    #[gpui::test]
    fn removal_blocks_runtime_launch_and_sweeps_late_metadata(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(SettingsState::new(storage::Store {
                projects: vec![
                    project("removing", "/grove-removal-fixture"),
                    project("other", "/other"),
                ],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let runtime = cx.new(crate::runtime::Runtime::new);
            let service = runtime.read(cx).projects.clone();
            service.update(cx, |service, _| {
                service
                    .removals
                    .insert("/grove-removal-fixture".into(), ProjectRemoval::default());
                assert!(service.is_removing("/grove-removal-fixture"));
                assert!(service
                    .ensure_not_removing("/grove-removal-fixture")
                    .is_err());
            });
            let launched = runtime.update(cx, |runtime, cx| {
                runtime.spawn_session_in_with_args(
                    "removing".into(),
                    "/grove-removal-fixture".into(),
                    grove_core::agent::Agent::Terminal,
                    Vec::new(),
                    cx,
                )
            });
            assert!(!launched);
            let stale_name_launch = runtime.update(cx, |runtime, cx| {
                runtime.spawn_session_in_with_args(
                    "old-name".into(),
                    "/grove-removal-fixture".into(),
                    grove_core::agent::Agent::Terminal,
                    Vec::new(),
                    cx,
                )
            });
            assert!(!stale_name_launch);
            let registry = runtime.read(cx).registry.clone();
            assert!(registry.read(cx).is_empty());
            registry.update(cx, |registry, _| {
                registry.insert_meta(
                    "removing".into(),
                    "/grove-removal-fixture".into(),
                    grove_core::agent::Agent::Terminal,
                );
                registry.insert_meta(
                    "other".into(),
                    "/other".into(),
                    grove_core::agent::Agent::Terminal,
                );
            });
            service.update(cx, |service, cx| {
                service
                    .sweep_removal_sessions("/grove-removal-fixture", cx)
                    .unwrap();
                service
                    .removals
                    .get_mut("/grove-removal-fixture")
                    .unwrap()
                    .finished = true;
                assert!(!service.is_removing("/grove-removal-fixture"));
            });
            assert!(registry.read(cx).by_project("removing").is_empty());
            assert_eq!(registry.read(cx).by_project("other").len(), 1);
        });
    }
    #[gpui::test]
    fn duplicate_names_and_context_roots_use_path_ownership(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let projects = vec![project("same", "/fixture/a"), project("same", "/fixture/b")];
            cx.set_global(SettingsState::new(storage::Store {
                projects: projects.clone(),
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let runtime = cx.new(crate::runtime::Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            let roots = vec![grove_core::session_meta::ContextRoot {
                project: "same".into(),
                wt_path: "/fixture/a".into(),
            }];
            let (a, b, multi) = registry.update(cx, |r, _| {
                let a = r.insert_meta(
                    "same".into(),
                    "/fixture/a".into(),
                    grove_core::agent::Agent::Terminal,
                );
                let b = r.insert_meta(
                    "same".into(),
                    "/fixture/b".into(),
                    grove_core::agent::Agent::Terminal,
                );
                let multi = r.insert_meta_with_context(
                    "same".into(),
                    "/fixture/b".into(),
                    grove_core::agent::Agent::Codex,
                    roots.clone(),
                    None,
                );
                r.rename_project_by_path(&projects, "/fixture/a", "renamed")
                    .unwrap();
                (a, b, multi)
            });
            assert_eq!(registry.read(cx).meta(a).unwrap().project, "renamed");
            assert_eq!(registry.read(cx).meta(b).unwrap().project, "same");
            assert_eq!(registry.read(cx).meta(multi).unwrap().project, "same");
            assert_eq!(
                registry.read(cx).meta(multi).unwrap().context_roots[0].project,
                "renamed"
            );
            let service = runtime.read(cx).projects.clone();
            service.update(cx, |service, cx| {
                assert_eq!(
                    service.archive_blockers("/fixture/a", cx).unwrap(),
                    vec![a, multi]
                );
                service
                    .removals
                    .insert("/fixture/a".into(), ProjectRemoval::default());
            });
            let launched = runtime.update(cx, |runtime, cx| {
                runtime.spawn_session_in_with_context(
                    "same".into(),
                    "/fixture/b".into(),
                    grove_core::agent::Agent::Codex,
                    Vec::new(),
                    roots,
                    None,
                    cx,
                )
            });
            assert!(!launched);
            service.update(cx, |service, cx| {
                service.sweep_removal_sessions("/fixture/a", cx).unwrap();
            });
            assert!(registry.read(cx).meta(a).is_none());
            assert!(registry.read(cx).meta(multi).is_none());
            assert!(registry.read(cx).meta(b).is_some());
        });
    }
    #[gpui::test]
    fn unknown_roots_block_archive_removal_rename_and_launch(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let projects = vec![project("known", "/fixture/known")];
            cx.set_global(SettingsState::new(storage::Store {
                projects: projects.clone(),
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let runtime = cx.new(crate::runtime::Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            registry.update(cx, |registry, _| {
                registry.insert_meta(
                    "known".into(),
                    "/external/unknown".into(),
                    grove_core::agent::Agent::Terminal,
                );
                assert!(registry
                    .rename_project_by_path(&projects, "/fixture/known", "new")
                    .is_err());
            });
            let service = runtime.read(cx).projects.clone();
            service.update(cx, |service, cx| {
                assert!(service
                    .archive_project_by_path("/fixture/known", cx)
                    .is_err());
                assert!(service
                    .remove_project_by_path("/fixture/known", false, cx)
                    .is_err());
                assert!(service
                    .update_project(
                        "/fixture/known",
                        "new".into(),
                        storage::ProjectScripts::default(),
                        cx
                    )
                    .is_err());
            });
            let launched = runtime.update(cx, |runtime, cx| {
                runtime.spawn_session_in_with_args(
                    "known".into(),
                    "/external/unknown".into(),
                    grove_core::agent::Agent::Terminal,
                    Vec::new(),
                    cx,
                )
            });
            assert!(!launched);
            assert_eq!(cx.global::<SettingsState>().store.projects[0].name, "known");
            assert_eq!(registry.read(cx).all().len(), 1);
        });
    }
    #[test]
    fn duplicate_name_saved_keys_only_rename_the_owned_path() {
        let mut store = storage::Store {
            projects: vec![project("same", "/fixture/a"), project("same", "/fixture/b")],
            grid_order: vec!["same::/fixture/a".into(), "same::/fixture/b".into()],
            recent_launches: vec![
                storage::RecentLaunch {
                    project: "same".into(),
                    wt_path: "/fixture/a".into(),
                    agent: grove_core::agent::Agent::Codex,
                },
                storage::RecentLaunch {
                    project: "same".into(),
                    wt_path: "/fixture/b".into(),
                    agent: grove_core::agent::Agent::Codex,
                },
            ],
            ..Default::default()
        };
        apply_project_update(&mut store, 0, "new", storage::ProjectScripts::default());
        assert_eq!(store.grid_order, ["new::/fixture/a", "same::/fixture/b"]);
        assert_eq!(store.recent_launches[0].project, "new");
        assert_eq!(store.recent_launches[1].project, "same");
    }
}
