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
mod tests;
