//! Project operations formerly owned by modal views. No rendered UI is required.

mod removal;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

use fs_err as fs;
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Task};
use grove_core::{git, storage};

use crate::entities::{
    session_registry::{SessionId, SessionMeta, SessionRegistry},
    terminal_session::TerminalSession,
    workspace_state::WorkspaceState,
};
use crate::settings::SettingsState;

#[derive(Clone)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReviewedWorktree {
    path: String,
    canonical_path: PathBuf,
    branch: String,
    folder_identity: WorktreeFileIdentity,
    git_entry_identity: WorktreeFileIdentity,
    git_entry_modified: std::time::SystemTime,
}

#[derive(Default)]
pub(crate) struct ProjectWorktreeReview {
    pub(crate) verified: Vec<ReviewedWorktree>,
    pub(crate) skipped_paths: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorktreeFileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    created: Option<std::time::SystemTime>,
}

impl WorktreeFileIdentity {
    fn from_metadata(metadata: &std::fs::Metadata) -> Result<Self, String> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt as _;
        Ok(Self {
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            created: metadata.created().ok(),
            #[cfg(not(unix))]
            created: Some(metadata.created().map_err(|error| error.to_string())?),
        })
    }
}

impl ReviewedWorktree {
    fn from_checkout(worktree: &git::Worktree) -> Result<Self, String> {
        let canonical_path = Path::new(&worktree.path)
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let folder = fs::metadata(&canonical_path).map_err(|error| error.to_string())?;
        let git_entry =
            fs::metadata(canonical_path.join(".git")).map_err(|error| error.to_string())?;
        Ok(Self {
            path: worktree.path.clone(),
            canonical_path,
            branch: worktree.branch.clone(),
            folder_identity: WorktreeFileIdentity::from_metadata(&folder)?,
            git_entry_identity: WorktreeFileIdentity::from_metadata(&git_entry)?,
            git_entry_modified: git_entry.modified().map_err(|error| error.to_string())?,
        })
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    fn verify(&self, repository: &str) -> Result<(), String> {
        let current = git::list_worktrees_checked(repository)
            .map_err(|error| format!("Could not list worktrees: {error}"))?
            .into_iter()
            .find(|tree| {
                tree.path == self.path && removable_worktree(repository, &tree.path, tree.is_main)
            })
            .ok_or("The reviewed checkout is no longer listed as a linked worktree.")?;
        self.verify_checkout(&current)
    }

    fn verify_checkout(&self, checkout: &git::Worktree) -> Result<(), String> {
        if Self::from_checkout(checkout)? != *self {
            return Err("The worktree changed after review. Its folder was left on disk. Reopen the worktree to review it again.".into());
        }
        Ok(())
    }
}

enum ProjectWorktreesToRemove {
    None,
    All,
    Reviewed(Vec<ReviewedWorktree>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorktreeRemovalStage {
    Checking,
    Stopping,
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
    reviewed: Option<ReviewedWorktree>,
}

pub struct ProjectService {
    registry: Entity<SessionRegistry>,
    state: Entity<WorkspaceState>,
    inline_scripts: Vec<(String, String, gpui::WeakEntity<TerminalSession>)>,
    teardown_session: Option<Entity<TerminalSession>>,
    teardown_poll: Option<Task<()>>,
    teardown_target: Option<(String, String)>,
    removals: BTreeMap<String, ProjectRemoval>,
    worktree_removals: BTreeMap<String, WorktreeRemoval>,
    creations: BTreeMap<String, Task<()>>,
    shell_owners: std::collections::HashMap<SessionId, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorktreeReadiness {
    Ready,
    NeedsGit,
    NeedsCommit,
}

/// A second checkout needs a Git repository with a resolvable starting commit.
pub(crate) fn worktree_readiness(path: &str) -> WorktreeReadiness {
    if !git::is_repo(path) {
        return WorktreeReadiness::NeedsGit;
    }
    let head = Command::new("git")
        .args(["-C", path, "rev-parse", "--verify", "HEAD"])
        .output();
    if !head.is_ok_and(|output| output.status.success()) {
        return WorktreeReadiness::NeedsCommit;
    }
    WorktreeReadiness::Ready
}

pub(crate) fn worktree_prerequisite(path: &str) -> Result<(), &'static str> {
    match worktree_readiness(path) {
        WorktreeReadiness::Ready => Ok(()),
        WorktreeReadiness::NeedsGit => Err("Initialize Git before creating a worktree. Sessions in this folder still work."),
        WorktreeReadiness::NeedsCommit => Err("Make the first commit before creating a worktree. Sessions in the main checkout still work."),
    }
}

impl EventEmitter<ProjectEvent> for ProjectService {}

impl ProjectService {
    pub(crate) fn track_inline_script(
        &mut self,
        project: String,
        checkout: String,
        terminal: &Entity<TerminalSession>,
    ) {
        self.inline_scripts
            .retain(|(_, _, session)| session.upgrade().is_some());
        self.inline_scripts
            .push((project, checkout, terminal.downgrade()));
    }

    fn ensure_no_inline_scripts(&self, path: &str, cx: &App) -> Result<(), String> {
        if self
            .inline_scripts
            .iter()
            .any(|(project, checkout, terminal)| {
                (project == path
                    || path_within_worktree(
                        checkout,
                        path,
                        Path::new(path).canonicalize().ok().as_deref(),
                    ))
                    && terminal
                        .upgrade()
                        .is_some_and(|terminal| !terminal.read(cx).has_exited())
            })
        {
            Err(
                "Stop running scripts in this checkout before removing, moving, or archiving it."
                    .into(),
            )
        } else {
            Ok(())
        }
    }

    pub(crate) fn project_git_initialized(&mut self, cx: &mut Context<Self>) {
        cx.emit(ProjectEvent::TreeInvalidated);
    }
    pub fn new(registry: Entity<SessionRegistry>, state: Entity<WorkspaceState>) -> Self {
        Self {
            registry,
            state,
            inline_scripts: Vec::new(),
            teardown_session: None,
            teardown_poll: None,
            teardown_target: None,
            removals: BTreeMap::new(),
            worktree_removals: BTreeMap::new(),
            creations: BTreeMap::new(),
            shell_owners: std::collections::HashMap::new(),
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
        self.register_project_with_appearance(name, path, workspace, None, cx)
    }

    pub(crate) fn register_project_with_appearance(
        &mut self,
        name: String,
        path: String,
        workspace: u64,
        appearance: Option<storage::identity::ProjectAppearance>,
        cx: &mut Context<Self>,
    ) -> Result<usize, String> {
        if appearance.is_some_and(|a| !a.automatic.valid() || a.custom.is_some_and(|m| !m.valid()))
        {
            return Err("Choose a valid project appearance.".into());
        }
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
            if let Some(appearance) = appearance {
                store
                    .project_appearances
                    .insert(project.path.clone(), appearance);
            }
            store.projects.push(project);
        });
        saved.map_err(|error| format!("Could not save project: {error}"))?;
        cx.emit(ProjectEvent::TreeInvalidated);
        Ok(idx)
    }

    /// Reassigns a registration without touching its repository or live sessions.
    pub fn move_project_to_workspace(
        &mut self,
        path: &str,
        destination: u64,
        open_destination: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.ensure_no_inline_scripts(path, cx)?;
        let store = &cx.global::<SettingsState>().store;
        project_index(&store.projects, path)
            .ok_or_else(|| "Project no longer exists.".to_string())?;
        if !store
            .workspaces
            .rows
            .iter()
            .any(|row| row.id == destination)
        {
            return Err("The selected workspace no longer exists.".into());
        }
        self.ensure_not_removing(path)?;
        if self.is_worktree_removing(path)
            || self.removals.values().any(|removal| {
                !removal.finished
                    && removal.current_target.as_deref().is_some_and(|target| {
                        path_within_worktree(
                            path,
                            target,
                            Path::new(target).canonicalize().ok().as_deref(),
                        )
                    })
            })
        {
            return Err(
                "Wait for the worktree removal to finish before moving this project.".into(),
            );
        }
        if store.project_workspace_id(path) == destination {
            return Err("This project is already in the selected workspace.".into());
        }
        let ((), saved) = SettingsState::update_and_flush_checked(cx, |store| {
            store
                .project_workspaces
                .insert(path.to_string(), destination);
            if open_destination {
                store.workspaces.select(destination);
            }
        });
        saved.map_err(|error| format!("Could not move project: {error}"))?;
        cx.emit(ProjectEvent::TreeInvalidated);
        Ok(())
    }

    pub fn update_project(
        &mut self,
        path: &str,
        name: String,
        scripts: storage::ProjectScripts,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.update_project_with_appearance(path, name, scripts, None, cx)
    }

    pub(crate) fn update_project_with_appearance(
        &mut self,
        path: &str,
        name: String,
        scripts: storage::ProjectScripts,
        appearance: Option<storage::identity::ProjectAppearance>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if appearance.is_some_and(|a| !a.automatic.valid() || a.custom.is_some_and(|m| !m.valid()))
        {
            return Err("Choose a valid project appearance.".into());
        }
        self.ensure_not_removing(path)?;
        let store = &cx.global::<SettingsState>().store;
        let idx = project_index(&store.projects, path)
            .ok_or_else(|| "Project no longer exists.".to_string())?;
        let name = validated_name(&store.projects, &name, Some(path))?;
        let old_name = store.projects[idx].name.clone();
        self.registered_project_blockers(path, cx)?;
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
            if let Some(appearance) = appearance {
                store
                    .project_appearances
                    .insert(path.to_string(), appearance);
            }
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

    #[cfg(test)]
    pub(crate) fn mark_history_removal_for_test(&mut self, project: &str, worktree: Option<&str>) {
        if let Some(path) = worktree {
            self.worktree_removals.insert(
                path.into(),
                WorktreeRemoval {
                    stage: WorktreeRemovalStage::Removing,
                    error: None,
                    project_path: project.into(),
                    canonical_path: std::path::PathBuf::from(path),
                    reviewed: None,
                },
            );
        } else {
            self.removals
                .insert(project.into(), ProjectRemoval::default());
        }
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
        if self.creations.contains_key(path) {
            Err("Wait for worktree creation to finish before changing this project.".into())
        } else if self.is_removing(path) {
            Err("This project is already being removed.".into())
        } else if self.worktree_removals.values().any(|operation| {
            operation.project_path == path && operation.stage != WorktreeRemovalStage::Finished
        }) {
            Err("Wait for the worktree removal to finish before changing this project.".into())
        } else {
            Ok(())
        }
    }

    pub fn create_worktree_with_branch(
        &mut self,
        project: &storage::Project,
        name: &str,
        branch: &str,
        base: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Task<Result<String, String>> {
        if let Err(error) = self.ensure_not_removing(&project.path) {
            return Task::ready(Err(error));
        }
        let project_path = project.path.clone();
        let project = project.clone();
        let name = name.to_owned();
        let branch = branch.to_owned();
        let base = base.map(str::to_owned);
        let work = cx.background_spawn(async move {
            worktree_prerequisite(&project.path).map_err(str::to_owned)?;
            let path = git::add_worktree_with_branch(
                &project.path,
                project.worktree_dir(),
                &name,
                &branch,
                base.as_deref(),
            )
            .map_err(|error| format!("Worktree failed: {error}"))?;
            if let Err(error) = git::copy_worktree_includes(&project.path, &path) {
                tracing::warn!("grove-gpui: worktree includes not copied: {error}");
            }
            Ok(path)
        });
        self.track_creation(project_path, work, cx)
    }

    fn track_creation(
        &mut self,
        project_path: String,
        work: Task<Result<String, String>>,
        cx: &mut Context<Self>,
    ) -> Task<Result<String, String>> {
        let (reply, receive) = futures::channel::oneshot::channel();
        let owner = project_path.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match &result {
                    Ok(path) => {
                        crate::telemetry::track("worktree_created", vec![]);
                        cx.emit(ProjectEvent::WorktreeAdded { path: path.clone() });
                    }
                    Err(_) => {
                        crate::telemetry::track("error", vec![("kind", "worktree_failed".into())]);
                    }
                }
                // The service owns completion even when the form/result future
                // has disappeared. Release only after the worker has finished.
                let finished = this.creations.remove(&owner);
                let _ = reply.send(result);
                drop(finished);
                cx.notify();
            });
        });
        self.creations.insert(project_path, task);
        cx.spawn(async move |_, _| {
            receive
                .await
                .unwrap_or_else(|_| Err("Worktree creation owner is no longer available.".into()))
        })
    }

    pub fn kill_sessions_for_project(&mut self, project: &str, cx: &mut Context<Self>) {
        self.kill_sessions(|m| m.project == project, cx);
    }

    fn kill_sessions(&mut self, matches: impl Fn(&SessionMeta) -> bool, cx: &mut Context<Self>) {
        self.registry.update(cx, |registry, cx| {
            registry.remove_worktree_shells_matching_background(&matches, cx);
            cx.notify();
        });
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
                r.remove_background(*id, cx);
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
        self.ensure_no_inline_scripts(path, cx)?;
        self.registered_project_blockers(path, cx)
    }

    fn registered_project_blockers(
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

    /// The runtime records the project path once when it creates a panel shell.
    /// This survives project renames and covers external Git worktree locations
    /// without a new `git worktree list` during removal.
    pub fn track_worktree_shell_owner(&mut self, id: SessionId, project_path: String) {
        self.shell_owners.insert(id, project_path);
    }

    pub fn kill_sessions_for_project_path(
        &mut self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let ids = self.archive_blockers(path, cx)?;
        let projects = cx.global::<SettingsState>().store.projects.clone();
        let live: std::collections::HashSet<_> = self
            .registry
            .read(cx)
            .worktree_shells()
            .map(|meta| meta.id)
            .collect();
        self.shell_owners.retain(|id, _| live.contains(id));
        let owned = &self.shell_owners;
        self.registry.update(cx, |registry, cx| {
            registry.remove_worktree_shells_matching_background(
                |meta| {
                    owned.get(&meta.id).is_some_and(|owner| owner == path)
                        || grove_core::session_meta::project_owner(&projects, &meta.wt_path)
                            .is_ok_and(|owner| owner.path == path)
                },
                cx,
            );
            cx.notify();
        });
        self.kill_sessions(|meta| ids.contains(&meta.id), cx);
        Ok(())
    }
}

async fn wait_for_session_cleanup(
    registry: &Entity<SessionRegistry>,
    cx: &mut gpui::AsyncApp,
) -> Result<(), String> {
    loop {
        let (error, pending) = registry.read_with(cx, |registry, _| {
            (
                registry.session_cleanup_error(),
                registry.has_pending_session_cleanup(),
            )
        });
        if let Some(error) = error {
            return Err(format!("Sessions could not stop safely: {error}"));
        }
        if !pending {
            return Ok(());
        }
        cx.background_executor()
            .timer(std::time::Duration::from_millis(20))
            .await;
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

    #[test]
    fn worktree_requires_git_and_first_commit() {
        if !Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "grove-onboarding-git-{}-{:?}",
            std::process::id(),
            SystemTime::now()
        ));
        fs::create_dir_all(&root).unwrap();
        let _fixture = GitFixture(root.clone());
        let path = root.to_string_lossy();
        assert!(worktree_prerequisite(&path)
            .unwrap_err()
            .contains("Initialize Git"));
        git::init_if_needed(&path).unwrap();
        assert!(worktree_prerequisite(&path)
            .unwrap_err()
            .contains("first commit"));
        let commit = Command::new("git")
            .args([
                "-C",
                &path,
                "-c",
                "user.name=Grove Test",
                "-c",
                "user.email=grove@example.test",
                "commit",
                "--allow-empty",
                "-m",
                "Initial commit",
            ])
            .output()
            .unwrap();
        assert!(
            commit.status.success(),
            "{}",
            String::from_utf8_lossy(&commit.stderr)
        );
        assert!(worktree_prerequisite(&path).is_ok());
    }

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

    #[gpui::test]
    fn pending_creation_survives_dismissal_and_guards_project_changes(
        cx: &mut gpui::TestAppContext,
    ) {
        let (release, wait) = futures::channel::oneshot::channel();
        let service = cx.update(|cx| {
            cx.set_global(SettingsState::new(storage::Store {
                projects: vec![project("fixture", "/pending-creation-fixture")],
                ..Default::default()
            }));
            let registry = cx.new(|_| SessionRegistry::new());
            let state = cx.new(|_| WorkspaceState::default());
            let service = cx.new(|_| ProjectService::new(registry, state));
            let result = service.update(cx, |service, cx| {
                let work = cx.background_spawn(async move {
                    wait.await
                        .unwrap_or_else(|_| Err("fixture canceled".into()))
                });
                service.track_creation("/pending-creation-fixture".into(), work, cx)
            });
            drop(result); // Closing the form must not cancel service ownership.
            service.update(cx, |service, cx| {
                assert!(service
                    .ensure_not_removing("/pending-creation-fixture")
                    .unwrap_err()
                    .contains("creation"));
                assert!(service
                    .archive_project_by_path("/pending-creation-fixture", cx)
                    .unwrap_err()
                    .contains("creation"));
                assert!(service
                    .remove_worktree_by_path(
                        "/pending-creation-fixture",
                        "/pending-creation-fixture/child",
                        cx
                    )
                    .unwrap_err()
                    .contains("creation"));
            });
            service
        });
        release
            .send(Err("expected fixture failure".into()))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while service.read_with(cx, |service, _| !service.creations.is_empty())
            && std::time::Instant::now() < deadline
        {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        service.read_with(cx, |service, _| {
            assert!(service.creations.is_empty());
            assert!(service
                .ensure_not_removing("/pending-creation-fixture")
                .is_ok());
        });
    }

    #[gpui::test]
    fn workspace_move_persists_preserves_sessions_and_rolls_back(cx: &mut gpui::TestAppContext) {
        // Every write stays in a subprocess-local config directory.
        if std::env::var_os("GROVE_WORKSPACE_MOVE_TEST_CHILD").is_none() {
            let config = std::env::temp_dir().join(format!(
                "grove-workspace-move-test-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&config).unwrap();
            let fixture = GitFixture(config.clone());
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "project_service::tests::workspace_move_persists_preserves_sessions_and_rolls_back",
                    "--nocapture",
                ])
                .env("GROVE_WORKSPACE_MOVE_TEST_CHILD", "1")
                .env("GROVE_CONFIG_DIR", &config).env("GROVE_TEST_CONFIG_DIR", &config)
                .output()
                .unwrap();
            drop(fixture);
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let repo = storage::config_dir().unwrap().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let session =
            cx.new(|cx| TerminalSession::spawn_script("sleep 60", repo.to_str().unwrap(), cx));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while session.read_with(cx, |session, _| session.root_pid().is_none())
            && std::time::Instant::now() < deadline
        {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        cx.update(|cx| {
            let config = storage::config_dir().unwrap();
            let repo = config.join("repo");
            fs::create_dir_all(&repo).unwrap();
            let path = repo.to_str().unwrap();
            let mut store = storage::Store {
                projects: vec![project("moving", path), project("other", "/other")],
                appearance: Some(storage::AppearancePreference::Dark),
                grid_order: vec![format!("moving::{path}::session")],
                recent_launches: vec![storage::RecentLaunch {
                    project: "moving".into(),
                    wt_path: path.into(),
                    agent: grove_core::agent::Agent::Terminal,
                }],
                ..Default::default()
            };
            store.projects[1].archived = true;
            store.projects[0].scripts.run = Some("echo retained".into());
            store.projects[0].worktree_dir = Some("original-directory".into());
            let source = store.workspaces.active;
            store.workspaces.create("Destination").unwrap();
            let destination = store.workspaces.active;
            store.workspaces.select(source);
            store.normalize_workspaces();
            storage::save(&store).unwrap();
            let original = serde_json::to_value(&store).unwrap();
            let config_path = storage::config_path().unwrap();
            let saved_before = fs::read(&config_path).unwrap();
            cx.set_global(SettingsState::new(store));
            let registry = cx.new(|_| SessionRegistry::new());
            let state = cx.new(|_| WorkspaceState::default());
            let service = cx.new(|_| ProjectService::new(registry.clone(), state.clone()));
            assert!(session.read(cx).spawn_error().is_none());
            let pid = session.read(cx).root_pid().unwrap();
            let id = registry.update(cx, |registry, _| {
                let id = registry.insert_meta(
                    "moving".into(),
                    path.into(),
                    grove_core::agent::Agent::Terminal,
                );
                registry.attach(id, session.clone(), None);
                id
            });
            let spawned_at = registry.read(cx).meta(id).unwrap().spawned_at;
            service.update(cx, |service, cx| {
                assert!(service
                    .move_project_to_workspace("/missing", destination, false, cx)
                    .is_err());
                assert!(service
                    .move_project_to_workspace(path, u64::MAX, false, cx)
                    .is_err());
                assert!(service
                    .move_project_to_workspace(path, source, false, cx)
                    .is_err());
                service
                    .removals
                    .insert(path.into(), ProjectRemoval::default());
                assert!(service
                    .move_project_to_workspace(path, destination, false, cx)
                    .is_err());
                service.removals.clear();
                service.worktree_removals.insert(
                    path.into(),
                    WorktreeRemoval {
                        stage: WorktreeRemovalStage::RunningScript,
                        error: None,
                        project_path: path.into(),
                        canonical_path: repo.clone(),
                        reviewed: None,
                    },
                );
                assert!(service
                    .move_project_to_workspace(path, destination, false, cx)
                    .is_err());
                service.worktree_removals.clear();
            });
            assert_eq!(fs::read(&config_path).unwrap(), saved_before);
            assert_eq!(
                serde_json::to_value(&cx.global::<SettingsState>().store).unwrap(),
                original
            );
            service.update(cx, |service, cx| {
                service
                    .move_project_to_workspace(path, destination, false, cx)
                    .unwrap();
            });
            let store = &cx.global::<SettingsState>().store;
            assert_eq!(store.workspaces.active, source);
            assert_eq!(store.project_workspace_id(path), destination);
            assert_eq!(store.workspaces.rows[0].projects, 1);
            assert_eq!(store.workspaces.rows[1].projects, 1);
            let mut expected = original;
            expected["project_workspaces"][path] = destination.into();
            assert_eq!(serde_json::to_value(store).unwrap(), expected);
            assert_eq!(
                serde_json::to_value(storage::load().unwrap()).unwrap(),
                expected
            );
            assert_eq!(registry.read(cx).meta(id).unwrap().spawned_at, spawned_at);
            assert_eq!(registry.read(cx).meta(id).unwrap().project, "moving");
            assert_eq!(registry.read(cx).meta(id).unwrap().wt_path, path);
            assert_eq!(registry.read(cx).session(id), Some(&session));
            assert_eq!(session.read(cx).root_pid(), Some(pid));
            assert!(session.update(cx, |session, _| session.alive()));
            assert!(repo.is_dir());

            // Opening the destination persists its selection and MRU with membership.
            service.update(cx, |service, cx| {
                service
                    .move_project_to_workspace(path, source, false, cx)
                    .unwrap();
                service
                    .move_project_to_workspace(path, destination, true, cx)
                    .unwrap();
            });
            expected["workspaces"]["active"] = destination.into();
            expected["workspaces"]["mru"] = serde_json::json!([destination, source]);
            assert_eq!(
                serde_json::to_value(&cx.global::<SettingsState>().store).unwrap(),
                expected
            );
            assert_eq!(
                serde_json::to_value(storage::load().unwrap()).unwrap(),
                expected
            );

            // A directory at the save target makes the atomic rename fail reliably.
            fs::rename(&config_path, config.join("saved-before-failure.json")).unwrap();
            fs::create_dir(&config_path).unwrap();
            service.update(cx, |service, cx| {
                assert!(service
                    .move_project_to_workspace(path, source, true, cx)
                    .unwrap_err()
                    .contains("Could not move project"));
            });
            let restored = &cx.global::<SettingsState>().store;
            assert_eq!(serde_json::to_value(restored).unwrap(), expected);
            assert_eq!(restored.workspaces.rows[0].projects, 1);
            assert_eq!(restored.workspaces.rows[1].projects, 1);
            assert_eq!(restored.workspaces.active, destination);
            assert!(!cx.global::<SettingsState>().is_dirty());
            assert_eq!(registry.read(cx).meta(id).unwrap().spawned_at, spawned_at);
            assert_eq!(registry.read(cx).session(id), Some(&session));
            assert_eq!(session.read(cx).root_pid(), Some(pid));
            assert!(session.update(cx, |session, _| session.alive()));
        });
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
            ..Default::default()
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
    fn removal_waits_for_cancelled_startup_before_teardown_and_git(cx: &mut gpui::TestAppContext) {
        let Some((fixture, repo, wt)) = git_fixture() else {
            return;
        };
        let marker = fixture.0.join("teardown-started");
        let (release, gate) = futures::channel::oneshot::channel();
        let (service, terminal) = cx.update(|cx| {
            let mut configured = project("fixture", &repo);
            configured.scripts.teardown = Some(format!("printf done > '{}'", marker.display()));
            cx.set_global(SettingsState::new(storage::Store {
                projects: vec![configured],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let registry = cx.new(|_| SessionRegistry::new());
            let state = cx.new(|_| WorkspaceState::default());
            let service = cx.new(|_| ProjectService::new(registry.clone(), state));
            let terminal = cx.new(|cx| TerminalSession::pending_startup_for_test(gate, cx));
            registry.update(cx, |registry, cx| {
                let id = registry.insert_meta(
                    "fixture".into(),
                    wt.clone(),
                    grove_core::agent::Agent::Terminal,
                );
                registry.attach_observed(id, terminal.clone(), None, cx);
            });
            (service, terminal)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        service.update(cx, |service, cx| {
            service.remove_worktree_by_path(&repo, &wt, cx).unwrap();
        });
        while service.read_with(cx, |service, _| {
            service.worktree_removal_status(&wt).unwrap().stage != WorktreeRemovalStage::Stopping
        }) {
            cx.run_until_parked();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(20));
            assert!(
                std::time::Instant::now() < deadline,
                "{:?}",
                service.read_with(cx, |service, _| service
                    .worktree_removal_status(&wt)
                    .cloned())
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(terminal.read_with(cx, |terminal, _| terminal.is_starting()));
        assert!(Path::new(&wt).exists());
        assert!(
            !marker.exists(),
            "teardown must wait for startup cancellation acknowledgement"
        );
        release.send(()).unwrap();
        while service.read_with(cx, |service, _| {
            service.worktree_removal_status(&wt).unwrap().stage != WorktreeRemovalStage::Finished
        }) {
            cx.run_until_parked();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(120));
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(terminal.read_with(cx, |terminal, _| terminal.has_exited()));
        assert!(marker.exists());
        assert!(!Path::new(&wt).exists());
        assert!(service.read_with(cx, |service, _| service
            .worktree_removal_status(&wt)
            .unwrap()
            .error
            .is_none()));
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
                    WorktreeRemovalStage::Checking,
                );
                assert!(service.remove_worktree_by_path(&repo, &wt, cx).is_err());
                assert!(service.archive_project_by_path(&repo, cx).is_err());
                assert!(service
                    .register_project("late registration".into(), alias.clone(), cx)
                    .is_err());
            });

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
            (
                service,
                registry,
                [primary, context, alias_primary, alias_context],
                main,
            )
        });
        let (service, registry, removed, main) = service;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while service.read_with(cx, |service, _| {
            service.worktree_removal_status(&wt).unwrap().stage != WorktreeRemovalStage::Finished
        }) {
            cx.run_until_parked();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(20));
            assert!(
                std::time::Instant::now() < deadline,
                "removal must complete after actual session cleanup"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        cx.update(|cx| {
            for id in removed {
                assert!(registry.read(cx).meta(id).is_none());
            }
            assert!(registry.read(cx).meta(main).is_some());
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
    fn stale_worktree_removal_refreshes_inventory_without_touching_replaced_folder(
        cx: &mut gpui::TestAppContext,
    ) {
        let Some((_fixture, repo, wt)) = git_fixture() else {
            return;
        };
        let (runtime, id) = cx.update(|cx| {
            cx.set_global(SettingsState::new(storage::Store {
                projects: vec![project("fixture", &repo)],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let runtime = cx.new(crate::runtime::Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            let id = registry.update(cx, |registry, _| {
                registry.insert_meta(
                    "fixture".into(),
                    wt.clone(),
                    grove_core::agent::Agent::Terminal,
                )
            });
            (runtime, id)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !runtime.read_with(cx, |runtime, cx| {
            runtime
                .tree
                .read(cx)
                .worktrees_for_project(0, 0)
                .iter()
                .any(|entry| entry.path == wt)
        }) && std::time::Instant::now() < deadline
        {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        cx.update(|cx| {
            assert!(runtime
                .read(cx)
                .tree
                .read(cx)
                .worktrees_for_project(0, 0)
                .iter()
                .any(|entry| entry.path == wt));
        });
        // Another Git client removes the checkout while Grove still displays its cached row.
        git::remove_worktree(&repo, &wt).unwrap();
        fs::create_dir(&wt).unwrap();
        let marker = Path::new(&wt).join("unrelated.txt");
        fs::write(&marker, "keep me").unwrap();
        cx.update(|cx| {
            let service = runtime.read(cx).projects.clone();
            service.update(cx, |service, cx| {
                service.remove_worktree_by_path(&repo, &wt, cx).unwrap();
            });
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let runtime = runtime.read(cx);
            let status = runtime
                .projects
                .read(cx)
                .worktree_removal_status(&wt)
                .unwrap();
            assert_eq!(status.stage, WorktreeRemovalStage::Finished);
            assert_eq!(
                status.error.as_deref(),
                Some("Worktree is no longer listed by Git.")
            );
            let inventory = runtime.tree.read(cx).worktrees_for_project(0, 0);
            assert!(!inventory.iter().any(|entry| entry.path == wt));
            assert!(inventory
                .iter()
                .any(|entry| entry.path == repo && entry.is_main));
            assert!(runtime.registry.read(cx).meta(id).is_some());
            assert_eq!(fs::read_to_string(&marker).unwrap(), "keep me");
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
                .env("GROVE_CONFIG_DIR", &config).env("GROVE_TEST_CONFIG_DIR", &config)
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
    fn reviewed_bulk_project_removal_keeps_new_and_replaced_worktrees(
        cx: &mut gpui::TestAppContext,
    ) {
        // Project removal persists settings. All filesystem effects belong to this fixture.
        if std::env::var_os("GROVE_REVIEWED_BULK_REMOVAL_TEST_CHILD").is_none() {
            let config = tempfile::tempdir().unwrap();
            let result = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "project_service::tests::reviewed_bulk_project_removal_keeps_new_and_replaced_worktrees",
                    "--nocapture",
                ])
                .env("GROVE_REVIEWED_BULK_REMOVAL_TEST_CHILD", "1")
                .env("GROVE_CONFIG_DIR", config.path())
                .env("GROVE_TEST_CONFIG_DIR", config.path())
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }

        let Some((fixture, repo, replaced)) = git_fixture() else {
            return;
        };
        let retained_path = |name: &str| fixture.0.join(name).to_str().unwrap().to_owned();
        let unchanged = retained_path("reviewed-worktree");
        let unavailable = retained_path("unavailable-worktree");
        let unavailable_folder = retained_path("unavailable-worktree-folder");
        let moved = retained_path("moved-worktree");
        let added = retained_path("new-worktree");
        let git = |args: &[&str]| {
            let result = Command::new("git")
                .args(["-C", &repo])
                .args(args)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        };
        git(&["worktree", "add", "--detach", &unchanged]);
        git(&["worktree", "add", "--detach", &unavailable]);
        fs::rename(&unavailable, &unavailable_folder).unwrap();
        let review = ProjectService::review_project_worktrees(&repo).unwrap();
        assert_eq!(review.skipped_paths, [unavailable.clone()]);
        assert_eq!(review.verified.len(), 2);
        assert!(review.verified.iter().any(|tree| tree.path() == replaced));
        assert!(review.verified.iter().any(|tree| tree.path() == unchanged));

        git(&["worktree", "move", &replaced, &moved]);
        git(&["worktree", "add", "--detach", &replaced]);
        git(&["worktree", "add", "--detach", &added]);
        let marker = Path::new(&replaced).join("keep.txt");
        fs::write(&marker, "new checkout").unwrap();
        let service = cx.update(|cx| {
            cx.set_global(SettingsState::new(storage::Store {
                projects: vec![project("source", &repo)],
                ..Default::default()
            }));
            let registry = cx.new(|_| SessionRegistry::new());
            let state = cx.new(|_| WorkspaceState::default());
            let service = cx.new(|_| ProjectService::new(registry, state));
            service.update(cx, |service, cx| {
                service
                    .remove_project_with_reviewed_worktrees(&repo, review.verified, cx)
                    .unwrap();
            });
            service
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let operation = service.read(cx).removal_status(&repo).unwrap();
            assert!(operation.finished && operation.unregistered);
            assert_eq!((operation.completed, operation.total), (2, 2));
            assert_eq!(operation.errors.len(), 1, "{:?}", operation.errors);
            assert!(operation.errors[0].contains(&replaced));
            assert!(operation.errors[0].contains("changed after review"));
            assert_eq!(fs::read_to_string(&marker).unwrap(), "new checkout");
            assert!(Path::new(&unavailable_folder).exists());
            assert!(git::list_worktrees_checked(&repo)
                .unwrap()
                .iter()
                .any(|entry| entry.path == unavailable));
            assert!(Path::new(&moved).exists());
            assert!(Path::new(&added).exists());
            assert!(Path::new(&repo).exists());
            assert!(!Path::new(&unchanged).exists());
        });
    }

    #[gpui::test]
    fn reviewed_worktree_removal_rechecks_identity_before_sessions_and_after_teardown(
        cx: &mut gpui::TestAppContext,
    ) {
        let Some((fixture, repo, path)) = git_fixture() else {
            return;
        };
        let review = || {
            let branch = git::list_worktrees_checked(&repo)
                .unwrap()
                .into_iter()
                .find(|checkout| checkout.path == path)
                .unwrap()
                .branch;
            ProjectService::review_worktree(&repo, &path, &branch).unwrap()
        };
        let replace = |name| {
            let moved = fixture.0.join(name);
            let git = |args: &[&str]| {
                let result = Command::new("git")
                    .args(["-C", &repo])
                    .args(args)
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
            };
            git(&["worktree", "move", &path, moved.to_str().unwrap()]);
            git(&["worktree", "add", "--detach", &path]);
            fs::write(Path::new(&path).join("keep.txt"), name).unwrap();
        };
        let original = review();
        replace("before-start");

        let (service, registry, session) = cx.update(|cx| {
            cx.set_global(SettingsState::new(storage::Store {
                projects: vec![project("source", &repo)],
                ..Default::default()
            }));
            let registry = cx.new(|_| SessionRegistry::new());
            let state = cx.new(|_| WorkspaceState::default());
            let service = cx.new(|_| ProjectService::new(registry.clone(), state));
            let session = registry.update(cx, |registry, _| {
                registry.insert_meta(
                    "source".into(),
                    path.clone(),
                    grove_core::agent::Agent::Terminal,
                )
            });
            service.update(cx, |service, cx| {
                service
                    .remove_reviewed_worktree(&repo, original, cx)
                    .unwrap();
            });
            (service, registry, session)
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let operation = service.read(cx).worktree_removal_status(&path).unwrap();
            assert_eq!(operation.stage, WorktreeRemovalStage::Finished);
            assert!(operation
                .error
                .as_deref()
                .unwrap()
                .contains("changed after review"));
            assert!(registry.read(cx).meta(session).is_some());
            assert_eq!(
                fs::read_to_string(Path::new(&path).join("keep.txt")).unwrap(),
                "before-start"
            );
            registry.update(cx, |registry, cx| registry.remove_background(session, cx));
        });

        let reviewed = review();
        replace("during-teardown");
        cx.update(|cx| {
            service.update(cx, |service, cx| {
                // Exercise the continuation after a teardown that was already in progress.
                service.teardown_target = Some((repo.clone(), path.clone()));
                service.worktree_removals.insert(
                    path.clone(),
                    WorktreeRemoval {
                        stage: WorktreeRemovalStage::RunningScript,
                        error: None,
                        project_path: repo.clone(),
                        canonical_path: reviewed.canonical_path.clone(),
                        reviewed: Some(reviewed),
                    },
                );
                service.skip_worktree_teardown_for(&path, cx).unwrap();
            });
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let operation = service.read(cx).worktree_removal_status(&path).unwrap();
            assert_eq!(operation.stage, WorktreeRemovalStage::Finished);
            assert!(operation
                .error
                .as_deref()
                .unwrap()
                .contains("changed after review"));
            assert_eq!(
                fs::read_to_string(Path::new(&path).join("keep.txt")).unwrap(),
                "during-teardown"
            );
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
                    WorktreeRemovalStage::Checking,
                );
            });
            (service, registry)
        });
        let (service, registry) = service;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while service.read_with(cx, |service, _| {
            service
                .worktree_removal_status(&wt)
                .is_some_and(|state| state.error.is_none())
        }) && std::time::Instant::now() < deadline
        {
            cx.run_until_parked();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(120));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        service.update(cx, |service, cx| {
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
                WorktreeRemovalStage::Stopping,
            );
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while service.read_with(cx, |service, _| {
            service.worktree_removal_status(&wt).unwrap().stage != WorktreeRemovalStage::Finished
        }) {
            cx.run_until_parked();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(20));
            assert!(
                std::time::Instant::now() < deadline,
                "removal must complete after actual session cleanup"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
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
