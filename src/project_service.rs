//! Project operations formerly owned by modal views. No rendered UI is required.

use gpui::{AppContext as _, Context, Entity, EventEmitter, Task};
use grove_core::{git, storage};

use crate::entities::{
    session_registry::{SessionMeta, SessionRegistry},
    terminal_session::TerminalSession,
    workspace_state::WorkspaceState,
};
use crate::settings::SettingsState;

pub enum ProjectEvent {
    TreeInvalidated,
    WorktreeAdded { path: String },
    WorktreeRemoved { result: Result<(), String> },
    ProjectRemoved { errors: Vec<String> },
}

pub struct ProjectService {
    registry: Entity<SessionRegistry>,
    state: Entity<WorkspaceState>,
    teardown_session: Option<Entity<TerminalSession>>,
    teardown_poll: Option<Task<()>>,
    teardown_target: Option<(String, String)>,
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
        }
    }

    pub fn register_project(
        &mut self,
        name: String,
        path: String,
        cx: &mut Context<Self>,
    ) -> Result<usize, String> {
        if !git::valid_project_name(&name) {
            return Err(format!(
                "'{name}' isn't a valid project name; use letters, digits, '.', '-' or '_'"
            ));
        }
        let idx = cx.global::<SettingsState>().store.projects.len();
        SettingsState::update(cx, move |store| {
            store.projects.push(storage::Project {
                name,
                path,
                scripts: storage::ProjectScripts::default(),
                theme: None,
                archived: false,
                worktree_dir: None,
            });
        });
        SettingsState::flush_now(cx);
        cx.emit(ProjectEvent::TreeInvalidated);
        Ok(idx)
    }

    pub fn create_worktree(
        &mut self,
        project: &storage::Project,
        name: &str,
        base: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
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
    pub fn archive_project(&mut self, idx: usize, cx: &mut Context<Self>) -> bool {
        let Some(project) = cx.global::<SettingsState>().store.projects.get(idx) else {
            return false;
        };
        if self
            .registry
            .read(cx)
            .all()
            .iter()
            .any(|m| m.project == project.name)
        {
            return false;
        }
        SettingsState::update(cx, |store| store.projects[idx].archived = true);
        SettingsState::flush_now(cx);
        cx.emit(ProjectEvent::TreeInvalidated);
        true
    }

    pub fn restore_archived(&mut self, idx: usize, cx: &mut Context<Self>) {
        SettingsState::update(cx, |store| {
            if let Some(project) = store.projects.get_mut(idx) {
                project.archived = false;
            }
        });
        SettingsState::flush_now(cx);
        cx.emit(ProjectEvent::TreeInvalidated);
    }

    pub fn delete_archived(&mut self, idx: usize, cx: &mut Context<Self>) {
        SettingsState::update(cx, |store| {
            if idx < store.projects.len() {
                store.projects.remove(idx);
            }
        });
        SettingsState::flush_now(cx);
        self.state.update(cx, |state, cx| {
            state.on_project_removed(idx);
            cx.notify();
        });
        cx.emit(ProjectEvent::TreeInvalidated);
    }

    pub fn remove_project(
        &mut self,
        idx: usize,
        also_remove_worktrees: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .cloned()
        else {
            return;
        };
        self.kill_sessions_for_project(&project.name, cx);
        if !also_remove_worktrees {
            self.delete_archived(idx, cx);
            cx.emit(ProjectEvent::ProjectRemoved { errors: Vec::new() });
            return;
        }
        let target = (project.name.clone(), project.path.clone());
        cx.spawn(async move |this, cx| {
            let errors = cx
                .background_executor()
                .spawn(async move {
                    git::list_worktrees(&project.path)
                        .into_iter()
                        .filter(|w| !w.is_main)
                        .filter_map(|w| {
                            git::remove_worktree(&project.path, &w.path)
                                .err()
                                .map(|e| format!("{}: {e}", w.path))
                        })
                        .collect()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let idx = project_index(&cx.global::<SettingsState>().store.projects, &target);
                if let Some(idx) = idx {
                    this.delete_archived(idx, cx);
                }
                cx.emit(ProjectEvent::ProjectRemoved { errors });
            });
        })
        .detach();
    }

    /// Own the script PTY independently of any terminal view, then remove on exit.
    pub fn start_teardown(
        &mut self,
        project: &storage::Project,
        path: String,
        cx: &mut Context<Self>,
    ) {
        self.kill_sessions(|m| m.wt_path == path, cx);
        self.teardown_poll = None;
        self.teardown_session = None;
        self.teardown_target = Some((project.path.clone(), path.clone()));
        let Some(script) = project
            .scripts
            .teardown
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            self.skip_teardown_script(cx);
            return;
        };
        let session = cx.new(|cx| TerminalSession::spawn_script(script, &path, cx));
        self.teardown_session = Some(session.clone());
        self.teardown_poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(120))
                .await;
            if session.update(cx, |s, _| s.alive()) {
                continue;
            }
            let _ = this.update(cx, ProjectService::skip_teardown_script);
            return;
        }));
    }

    pub fn skip_teardown_script(&mut self, cx: &mut Context<Self>) {
        let Some((project_path, wt)) = self.teardown_target.take() else {
            return;
        };
        self.teardown_poll = None;
        self.teardown_session = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    git::remove_worktree(&project_path, &wt).map_err(|e| e.to_string())
                })
                .await;
            let _ = this.update(cx, |_, cx| {
                cx.emit(ProjectEvent::TreeInvalidated);
                cx.emit(ProjectEvent::WorktreeRemoved { result });
            });
        })
        .detach();
    }
}

/// Resolve after background work: removing another project may have shifted indices.
fn project_index(projects: &[storage::Project], target: &(String, String)) -> Option<usize> {
    projects
        .iter()
        .position(|p| p.name == target.0 && p.path == target.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str, path: &str) -> storage::Project {
        storage::Project {
            name: name.into(),
            path: path.into(),
            scripts: storage::ProjectScripts::default(),
            theme: None,
            archived: false,
            worktree_dir: None,
        }
    }

    #[test]
    fn pending_removal_tracks_target_after_another_project_is_removed() {
        let mut projects = vec![project("first", "/first"), project("target", "/target")];
        let target = ("target".into(), "/target".into());
        projects.remove(0);
        assert_eq!(project_index(&projects, &target), Some(0));
        projects.remove(0);
        projects.push(project("replacement", "/replacement"));
        assert_eq!(project_index(&projects, &target), None);
    }
}
