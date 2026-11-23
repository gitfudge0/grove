//! Session orchestration retained independently of the window during the UI rebuild.
//! Construction loads state; spawning and tmux attachment remain explicit operations.

mod managed_sessions;

mod scripts;

use crate::{
    entities::{
        activity_store::ActivityStore,
        project_tree::ProjectTree,
        session_registry::{SessionId, SessionRegistry},
        shell_environment::ShellEnvironment,
        terminal_session::TerminalSession,
        toast::ToastState,
        upgrade::Upgrade,
        workspace_state::{LiveTile, WorkspaceState},
    },
    settings::SettingsState,
};
use gpui::{prelude::*, App, Context, Entity, EventEmitter};
use grove_core::agent::Agent;
use std::{collections::HashMap, time::Instant};

fn saved_tmux_preference_enabled(preference: Option<bool>) -> bool {
    preference == Some(true)
}

fn managed_session_uses_tmux(preference: Option<bool>) -> bool {
    !cfg!(test) && saved_tmux_preference_enabled(preference)
}

pub(crate) fn native_codex_enabled(
    store: &grove_core::storage::Store,
    backend: Option<bool>,
) -> bool {
    store.native_agents_enabled && !cfg!(windows) && backend != Some(true)
}

fn should_choose_backend(preference: Option<bool>, tmux_available: impl FnOnce() -> bool) -> bool {
    preference.is_none() && tmux_available()
}

struct PendingManagedLaunch {
    name: String,
    cwd: String,
    agent: Agent,
    args: Vec<String>,
    context_roots: Vec<grove_core::session_meta::ContextRoot>,
    temp_bundle_path: Option<String>,
    record_recent: bool,
}

pub enum RuntimeEvent {
    BackendChoiceRequested,
}

impl EventEmitter<RuntimeEvent> for Runtime {}

fn tmux_discovery_enabled() -> bool {
    !cfg!(test)
}

#[derive(Clone, Copy)]
enum CheckoutValidation {
    RegisteredFolder,
    GitInventory,
    ProjectlessDirectory,
}

/// Authoritative launch preflight. Call only inside a provider/PTY startup
/// worker; UI selection uses cached inventory and never forks Git.
fn validate_checkout_paths(
    projects: &[grove_core::storage::Project],
    paths: &[std::path::PathBuf],
    policy: CheckoutValidation,
    expected_projects: &[String],
) -> Result<(), String> {
    if let Some(error) = crate::entities::codex_session::workspace_restore_error(paths, None) {
        return Err(error);
    }
    if !matches!(policy, CheckoutValidation::ProjectlessDirectory)
        && expected_projects.len() != paths.len()
    {
        return Err("Launch ownership is incomplete.".into());
    }
    for (index, path) in paths.iter().enumerate() {
        let canonical = fs_err::canonicalize(path)
            .map_err(|_| format!("{}: worktree is no longer available", path.display()))?;
        if !canonical.is_dir() {
            return Err(format!("{}: checkout is not a directory", path.display()));
        }
        if matches!(policy, CheckoutValidation::ProjectlessDirectory) {
            continue;
        }
        let project = projects
            .iter()
            .find(|project| project.path == expected_projects[index])
            .ok_or("The selected project is no longer registered.")?;
        if project.archived {
            return Err("Project is archived.".into());
        }
        if matches!(policy, CheckoutValidation::RegisteredFolder)
            && fs_err::canonicalize(&project.path).is_ok_and(|root| root == canonical)
        {
            continue;
        }
        let listed = grove_core::git::list_worktree_paths(&project.path, std::time::Duration::ZERO)
            .map_err(|error| {
                format!("Could not list worktrees for the selected project: {error}")
            })?;
        if !listed
            .iter()
            .any(|listed| fs_err::canonicalize(listed).is_ok_and(|listed| listed == canonical))
        {
            return Err(format!(
                "{}: checkout is no longer listed by the selected project",
                path.display()
            ));
        }
    }
    Ok(())
}

fn expected_project_paths(
    projects: &[grove_core::storage::Project],
    names: &[&str],
    policy: CheckoutValidation,
) -> Result<Vec<String>, String> {
    if matches!(policy, CheckoutValidation::ProjectlessDirectory) {
        return Ok(Vec::new());
    }
    names
        .iter()
        .map(|name| {
            projects
                .iter()
                .find(|project| project.name == *name)
                .map(|project| project.path.clone())
                .ok_or_else(|| "The selected project is no longer registered.".to_owned())
        })
        .collect()
}

#[derive(Clone)]
pub(crate) struct ScriptRun {
    pub id: u64,
    pub project_path: String,
    pub checkout_path: String,
    pub script_id: String,
    pub name: String,
    pub command: String,
    pub terminal: Entity<TerminalSession>,
    _observer: std::rc::Rc<gpui::Subscription>,
}

pub struct Runtime {
    pub state: Entity<WorkspaceState>,
    pub registry: Entity<SessionRegistry>,
    pub tree: Entity<ProjectTree>,
    pub activity: Entity<ActivityStore>,
    pub upgrade: Entity<Upgrade>,
    pub provider_updates: Entity<crate::entities::provider_updates::ProviderUpdates>,
    pub shell_environment: Entity<ShellEnvironment>,
    pub toast: Entity<ToastState>,
    pub projects: Entity<crate::project_service::ProjectService>,
    pub(crate) script_runs: Vec<ScriptRun>,
    next_script_run: u64,
    last_pty_dims: (u16, u16),
    pending_managed_launch: Option<PendingManagedLaunch>,
    pub(crate) restoring_codex: bool,
    native_title_observers: HashMap<SessionId, gpui::Subscription>,
    _observers: Vec<gpui::Subscription>,
}

impl Runtime {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let state =
            cx.new(|cx| WorkspaceState::new(&cx.global::<SettingsState>().store, crate::WINDOW_W));
        let registry = cx.new(|_| SessionRegistry::new());
        let tree = cx.new(|_| ProjectTree::new());
        let activity = cx.new({
            let (state, registry) = (state.clone(), registry.clone());
            |cx| ActivityStore::start(state, registry, cx)
        });
        let upgrade = cx.new(Upgrade::new);
        let provider_updates = cx.new(|cx| {
            crate::entities::provider_updates::ProviderUpdates::new(registry.clone(), cx)
        });
        let shell_environment = cx.new(ShellEnvironment::new);
        let toast = cx.new(|_| ToastState::new());
        let projects = cx
            .new(|_| crate::project_service::ProjectService::new(registry.clone(), state.clone()));
        let active_project = cx
            .global::<SettingsState>()
            .store
            .active_projects()
            .next()
            .map(|(i, p)| (i, p.path.clone()));
        if let Some((idx, path)) = active_project {
            tree.update(cx, |tree, cx| {
                // Stamp ownership before first render so snapshot construction
                // cannot synchronously refill this initial cache miss.
                tree.set_active_worktrees(idx, Vec::new());
                let generation = tree.generation();
                cx.spawn(async move |this, cx| {
                    let worktrees = cx
                        .background_spawn(async move { grove_core::git::list_worktrees(&path) })
                        .await;
                    let _ = this.update(cx, |tree, cx| {
                        if tree.generation() == generation && tree.active_project() == Some(idx) {
                            tree.set_active_worktrees(idx, worktrees);
                            cx.notify();
                        }
                    });
                })
                .detach();
            });
        }
        let observers = vec![
            cx.on_app_quit(|runtime, cx| {
                for run in &runtime.script_runs {
                    run.terminal.update(cx, TerminalSession::stop_script);
                }
                let stopped = runtime
                    .registry
                    .update(cx, |registry, _| registry.stop_codex_sessions());
                let wait = cx.background_spawn(async move {
                    for completion in stopped {
                        let _ = completion.recv_timeout(std::time::Duration::from_secs(1));
                    }
                });
                async move {
                    wait.await;
                }
            }),
            cx.observe(&registry, |this, _, cx| {
                let registry = this.registry.clone();
                let warnings = registry.update(cx, |registry, _| registry.take_history_warnings());
                if !warnings.is_empty() {
                    this.toast
                        .update(cx, |toast, cx| toast.set_error(warnings.join("\n"), cx));
                }
                let outcomes = registry.update(cx, |registry, _| registry.take_history_outcomes());
                for outcome in outcomes {
                    cx.spawn(async move |this, cx| {
                        // Cleanup may itself need foreground callbacks before it can
                        // complete. Yield while this owned result is pending rather
                        // than occupying an executor thread with a blocking receive.
                        let mut delay = std::time::Duration::from_millis(10);
                        let result = loop {
                            match outcome.try_recv() {
                                Ok(result) => break result,
                                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                    break Err("Could not confirm that session History was saved.".to_owned());
                                }
                                Err(std::sync::mpsc::TryRecvError::Empty) => {
                                    cx.background_executor().timer(delay).await;
                                    delay = (delay * 2).min(std::time::Duration::from_millis(250));
                                }
                            }
                        };
                        if let Err(error) = result {
                            let _ = this.update(cx, |this, cx| {
                                this.toast.update(cx, |toast, cx| {
                                    toast.set_error(format!("Could not save session History: {error} Recovery was kept and may restore on restart."), cx);
                                });
                            });
                        }
                    }).detach();
                }
                this.native_title_observers
                    .retain(|id, _| registry.read(cx).codex(*id).is_some());
                this.sync_grid_tiles(cx);
                cx.notify();
            }),
            cx.subscribe(&projects, Self::on_project_event),
        ];
        let dims = cx.global::<crate::zoom::CurrentPtyDims>();
        let mut runtime = Self {
            state,
            registry,
            tree,
            activity,
            upgrade,
            provider_updates,
            shell_environment,
            toast,
            projects,
            script_runs: Vec::new(),
            next_script_run: 0,
            last_pty_dims: (dims.rows, dims.cols),
            pending_managed_launch: None,
            restoring_codex: !cfg!(test)
                && native_codex_enabled(&cx.global::<SettingsState>().store, None),
            native_title_observers: HashMap::new(),
            _observers: observers,
        };
        if runtime.restoring_codex {
            runtime
                .provider_updates
                .update(cx, |updates, cx| updates.set_restoring(true, cx));
            runtime.restore_native_codex(cx);
        }
        runtime
    }

    fn on_project_event(
        &mut self,
        _: Entity<crate::project_service::ProjectService>,
        event: &crate::project_service::ProjectEvent,
        cx: &mut Context<Self>,
    ) {
        use crate::project_service::ProjectEvent;
        self.script_runs.retain(|run| {
            let project = cx
                .global::<SettingsState>()
                .store
                .script_project(&run.project_path);
            let exists = !project.archived
                && project.scripts.effective_entries().iter().any(|entry| {
                    entry.id == run.script_id
                        && matches!(
                            entry.kind,
                            grove_core::storage::ProjectActionKind::Shell { .. }
                        )
                })
                && std::path::Path::new(&run.checkout_path).exists();
            if !exists {
                run.terminal.update(cx, TerminalSession::stop_script);
            }
            exists
        });
        if matches!(
            event,
            ProjectEvent::WorktreeRemovalChanged { .. }
                | ProjectEvent::ProjectRemovalChanged { .. }
        ) {
            cx.notify();
            return;
        }
        let idx = self.state.read(cx).proj_idx();
        if matches!(
            event,
            ProjectEvent::WorktreeRemoved { .. } | ProjectEvent::ProjectRemoved { .. }
        ) {
            self.finish_project_event(event, idx, cx);
        }
        let active = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .map(|project| project.path.clone());
        let generation = self.tree.update(cx, |tree, cx| {
            tree.rebuild_wt_cache();
            cx.notify();
            tree.generation()
        });
        let event = event.clone();
        let read = cx.background_spawn(async move {
            active.map(|path| grove_core::git::list_worktrees(&path))
        });
        cx.spawn(async move |this, cx| {
            let inventory = read.await;
            let _ = this.update(cx, |this, cx| {
                let active = this.state.read(cx).proj_idx();
                let applied = this.tree.update(cx, |tree, cx| {
                    if tree.generation() != generation {
                        return false;
                    }
                    if let Some(inventory) = inventory {
                        tree.register_project_worktrees(idx, active, inventory);
                    }
                    cx.notify();
                    true
                });
                if applied && matches!(event, ProjectEvent::WorktreeAdded { .. }) {
                    this.finish_project_event(&event, idx, cx);
                }
            });
        })
        .detach();
    }

    fn finish_project_event(
        &mut self,
        event: &crate::project_service::ProjectEvent,
        idx: usize,
        cx: &mut Context<Self>,
    ) {
        use crate::project_service::ProjectEvent;
        match event {
            ProjectEvent::WorktreeAdded { path } if self.state.read(cx).proj_idx() == idx => {
                let snap = self.snapshot(cx);
                if let Some(wt) = snap
                    .projects
                    .iter()
                    .find(|p| p.idx == idx)
                    .and_then(|p| p.worktrees.iter().position(|w| w.path == *path))
                {
                    self.state.update(cx, |state, cx| {
                        state.focus_added_worktree(idx, wt, &snap);
                        cx.notify();
                    });
                }
            }
            ProjectEvent::WorktreeRemoved { result: Err(error) } => {
                self.toast
                    .update(cx, |toast, cx| toast.set_error(error.clone(), cx));
            }
            ProjectEvent::ProjectRemoved { errors } if !errors.is_empty() => {
                self.toast
                    .update(cx, |toast, cx| toast.set_error(errors.join("\n"), cx));
            }
            _ => {}
        }
        cx.notify();
    }

    /// Leaves the grid first, or a terminal spawned behind the tiles would be invisible.
    pub fn new_home_terminal(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.exit_grid_for_terminal();
            cx.notify();
        });
        self.spawn_home_terminal(cx);
    }

    /// Returns the new id only after its shell starts successfully.
    pub fn spawn_home_terminal(&mut self, cx: &mut Context<Self>) -> Option<SessionId> {
        let (label, target) = self.registry.update(cx, |r, _| {
            let label = r.next_home_label();
            (
                label.clone(),
                crate::entities::session_registry::SpawnTarget::home(label),
            )
        });
        let session = cx.new(|cx| {
            crate::entities::terminal_session::TerminalSession::spawn(&target, &[], None, cx)
        });
        if let Some(error) = session.read(cx).spawn_error().map(str::to_string) {
            self.toast.update(cx, |toast, cx| {
                toast.set_error(format!("terminal failed: {error}"), cx);
            });
            return None;
        }
        let (id, count) = self.registry.update(cx, |r, cx| {
            let id = r.next_home_id();
            r.push_home(
                crate::entities::session_registry::SessionMeta {
                    id,
                    project: String::new(),
                    wt_path: target.cwd.clone(),
                    agent: Agent::Terminal,
                    context_roots: Vec::new(),
                    temp_bundle_path: None,
                    label,
                    restored_title: None,
                    spawned_at: Instant::now(),
                    attention: None,
                    tmux: false,
                    tmux_name: None,
                },
                session,
            );
            cx.notify();
            (id, r.home_terminal_count())
        });
        self.state.update(cx, |s, cx| {
            s.select_home_terminal(count.saturating_sub(1), count);
            cx.notify();
        });
        Some(id)
    }

    /// Closes the terminal and leaves an empty panel when it was the last one.
    pub(crate) fn close_home_terminal(&mut self, i: usize, cx: &mut Context<Self>) {
        let invalid_index = {
            let registry = self.registry.read(cx);
            i >= registry.home_terminal_count() || registry.home_terminal(i).is_none()
        };
        if invalid_index {
            return;
        }
        let remaining = self.registry.update(cx, |r, cx| {
            r.close_home_background(i, cx);
            cx.notify();
            r.home_terminal_count()
        });
        self.state.update(cx, |s, cx| {
            s.close_home_terminal(i, remaining);
            cx.notify();
        });
    }

    pub(crate) fn snapshot(&self, cx: &mut App) -> crate::entities::workspace_state::TreeSnapshot {
        let active_proj = self.state.read(cx).proj_idx();
        let registry = self.registry.clone();
        self.tree.clone().update(cx, |tree, cx| {
            let store = &cx.global::<SettingsState>().store;
            tree.snapshot(store, registry.read(cx), active_proj)
        })
    }

    fn live_tiles(&self, cx: &App) -> Vec<LiveTile> {
        self.registry
            .read(cx)
            .all()
            .iter()
            .map(|m| LiveTile {
                id: m.id,
                key: crate::grid::session_grid_key(&m.project, &m.wt_path),
            })
            .collect()
    }

    fn saved_grid_order(cx: &App) -> Vec<String> {
        cx.global::<SettingsState>().store.grid_order.clone()
    }

    /// Every registry change funnels through here so a session spawned while the grid is up still gets a tile.
    fn sync_grid_tiles(&mut self, cx: &mut Context<Self>) {
        let (grid, before_zen, known) = {
            let ws = self.state.read(cx);
            (
                ws.grid_view(),
                ws.grid_view_before_zen(),
                ws.tile_order().to_vec(),
            )
        };
        if !grid && !before_zen {
            return;
        }
        let live = self.live_tiles(cx);
        if live.len() == known.len() && live.iter().all(|t| known.contains(&t.id)) {
            return;
        }
        let saved = Self::saved_grid_order(cx);
        self.state
            .update(cx, |s, _| s.reconcile_after_teardown(&live, &saved));
    }

    /// Drains the last transition's staged order into `Store::grid_order` (`layout.rs:481-489`).
    fn persist_grid_order(&mut self, cx: &mut Context<Self>) {
        let Some(order) = self.state.update(cx, |s, _| s.take_grid_order_to_persist()) else {
            return;
        };
        let registry = self.registry.read(cx);
        let keys: Vec<String> = order
            .iter()
            .filter_map(|&id| registry.meta(id))
            .map(|m| crate::grid::session_grid_key(&m.project, &m.wt_path))
            .collect();
        SettingsState::update(cx, |s| s.grid_order = keys);
    }

    /// The single flush every process-terminating path calls (carried decision 7; `src/gui/update/layout.rs:518-522`).
    pub(crate) fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.cancel_pending_managed_launch();
        self.persist_grid_order(cx);
        SettingsState::flush_now(cx);
    }

    pub(crate) fn stop_codex(&mut self, cx: &mut Context<Self>) -> gpui::Task<()> {
        self.shutdown(cx);
        let completions = self
            .registry
            .update(cx, |registry, _| registry.stop_codex_sessions());
        cx.background_spawn(async move {
            for completion in completions {
                let _ = completion.recv();
            }
        })
    }
    pub(crate) fn request_exit(&mut self, restart: bool, cx: &mut Context<Self>) {
        let stopped = self.stop_codex(cx);
        cx.spawn(async move |_, cx| {
            stopped.await;
            cx.update(|cx| {
                if restart {
                    cx.restart();
                } else {
                    cx.quit();
                }
            });
        })
        .detach();
    }

    pub fn discover_tmux_sessions(&mut self, cx: &mut Context<Self>) {
        if !tmux_discovery_enabled() {
            return;
        }
        let existing = self.registry.read(cx).all().to_vec();
        let paths: HashMap<String, String> = cx
            .global::<SettingsState>()
            .store
            .active_projects()
            .map(|(_, project)| (project.name.clone(), project.path.clone()))
            .collect();
        let ready = crate::app::startup_ready(cx);
        let work = cx.background_spawn(async move {
            ready.await;
            grove_core::env_path::wait_for_initial_environment();
            if !grove_core::tmux::available() {
                return Vec::new();
            }
            let discovered = grove_core::tmux::list_grove_sessions();
            let plan = {
                let wt_order = |project: &str| -> Vec<String> {
                    paths.get(project).map_or_else(Vec::new, |path| {
                        grove_core::git::list_worktree_paths(
                            path,
                            std::time::Duration::from_secs(2),
                        )
                        .unwrap_or_default()
                    })
                };
                crate::reattach::plan(&discovered, &existing, &wt_order)
            };
            plan.into_iter()
                .filter(|entry| {
                    crate::entities::codex_session::workspace_restore_error(
                        &std::iter::once(std::path::PathBuf::from(&entry.session.wt_path))
                            .chain(
                                entry
                                    .session
                                    .context_roots
                                    .iter()
                                    .map(|root| root.wt_path.clone().into()),
                            )
                            .collect::<Vec<_>>(),
                        None,
                    )
                    .is_none()
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let plan = work.await;
            let _ = this.update(cx, |this, cx| {
                let dims = this.last_pty_dims;
                for mut entry in plan {
                    if !this.revalidate_tmux_restore(&mut entry.session, cx) {
                        continue;
                    }
                    if this
                        .registry
                        .read(cx)
                        .all()
                        .iter()
                        .any(|meta| meta.tmux_name.as_deref() == Some(entry.session.name.as_str()))
                    {
                        continue;
                    }
                    if this
                        .provider_updates
                        .read(cx)
                        .is_updating(entry.session.agent)
                        || grove_core::connections::credentials_busy(entry.session.agent)
                    {
                        continue;
                    }
                    let name = entry.session.name.clone();
                    // `dims` only seeds the emulator; the tmux client attaches later at the painting tile's actual dims.
                    let session =
                        cx.new(|cx| TerminalSession::attach_existing(&name, dims.0, dims.1, cx));
                    if let Some(err) = session.read(cx).spawn_error().map(str::to_string) {
                        tracing::warn!(session = %name, error = %err, "reattach failed; skipping");
                        continue;
                    }
                    let id = this
                        .registry
                        .update(cx, |r, _| r.insert_reattached(entry.at, &entry.session));
                    this.registry.update(cx, |r, cx| {
                        r.attach_observed(id, session, Some(name.clone()), cx);
                        cx.notify();
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Apply ownership guards again after asynchronous discovery. Metadata can
    /// outlive a project rename, archive or removal while the worker waits.
    fn revalidate_tmux_restore(
        &self,
        session: &mut grove_core::tmux::DiscoveredSession,
        cx: &App,
    ) -> bool {
        let projects = &cx.global::<SettingsState>().store.projects;
        let service = self.projects.read(cx);
        let owner = |path: &str| {
            grove_core::session_meta::project_owner(projects, path)
                .ok()
                .filter(|project| {
                    !project.archived
                        && !service.is_removing(&project.path)
                        && !service.is_worktree_removing(path)
                })
                .map(|project| project.name.clone())
        };
        let Some(primary) = owner(&session.wt_path) else {
            return false;
        };
        let Some(context): Option<Vec<_>> = session
            .context_roots
            .iter()
            .map(|root| owner(&root.wt_path))
            .collect()
        else {
            return false;
        };
        session.project = primary;
        for (root, project) in session.context_roots.iter_mut().zip(context) {
            root.project = project;
        }
        true
    }

    /// Resolve menu ownership from the registered roots and warmed inventory.
    /// Fresh Git membership is checked later by the startup worker.
    fn cached_checkout_owner(
        &self,
        path: &str,
        cx: &App,
    ) -> Result<grove_core::storage::Project, String> {
        let projects = &cx.global::<SettingsState>().store.projects;
        let canonical = fs_err::canonicalize(path)
            .map_err(|_| format!("{path}: checkout is no longer available"))?;
        let direct: Vec<_> = projects
            .iter()
            .filter(|project| {
                fs_err::canonicalize(&project.path).is_ok_and(|root| root == canonical)
            })
            .collect();
        if direct.len() == 1 {
            return Ok(direct[0].clone());
        }
        if direct.len() > 1 {
            return Err("Checkout belongs to multiple project registrations.".into());
        }
        let active = self.state.read(cx).proj_idx();
        let tree = self.tree.read(cx);
        let cached: Vec<_> = projects
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                tree.worktrees_for_project(*index, active)
                    .iter()
                    .any(|worktree| {
                        fs_err::canonicalize(&worktree.path).is_ok_and(|listed| listed == canonical)
                    })
            })
            .map(|(_, project)| project)
            .collect();
        if cached.len() == 1 {
            return Ok(cached[0].clone());
        }
        if cached.len() > 1 {
            return Err("Checkout belongs to multiple project inventories.".into());
        }
        grove_core::session_meta::project_owner(projects, path).cloned()
    }

    fn directory_tools_available(&self, path: &str, cx: &gpui::App) -> bool {
        std::path::Path::new(path).is_dir()
            && (path == crate::entities::session_registry::home_dir()
                || self
                    .registry
                    .read(cx)
                    .all()
                    .iter()
                    .any(|meta| meta.project.is_empty() && meta.wt_path == path))
            && !cx
                .global::<SettingsState>()
                .store
                .projects
                .iter()
                .any(|p| p.path == path)
    }

    pub fn spawn_wt_shell(&mut self, wt_path: &str, cx: &mut Context<Self>) -> bool {
        let target = (|| -> Result<(), String> {
            let canonical = fs_err::canonicalize(wt_path)
                .map_err(|_| format!("{wt_path}: worktree is no longer available"))?;
            if !canonical.is_dir() {
                return Err(format!("{wt_path}: checkout is not a directory"));
            }
            if self.directory_tools_available(wt_path, cx) {
                return Ok(());
            }
            let project = self
                .cached_checkout_owner(wt_path, cx)
                .ok()
                .filter(|project| !project.archived)
                .ok_or_else(|| format!("{wt_path}: checkout is no longer registered"))?;
            if self.projects.read(cx).is_removing(&project.path)
                || self.projects.read(cx).is_worktree_removing(wt_path)
            {
                return Err("Cannot open a terminal while this checkout is being removed.".into());
            }
            Ok(())
        })();
        if let Err(error) = target {
            self.toast
                .update(cx, |toast, cx| toast.set_error(error, cx));
            return false;
        }
        let (id, label) = self
            .registry
            .update(cx, |r, _| (r.next_home_id(), r.next_wt_label()));
        let target = crate::entities::session_registry::SpawnTarget {
            cwd: wt_path.to_string(),
            agent: grove_core::agent::Agent::Terminal,
            project: String::new(),
            label: label.clone(),
            args: Vec::new(),
            context_roots: Vec::new(),
            temp_bundle_path: None,
            use_tmux: false,
        };
        let validation_projects = cx.global::<SettingsState>().store.projects.clone();
        let validation_paths = vec![std::path::PathBuf::from(wt_path)];
        let validation_policy = if self.directory_tools_available(wt_path, cx) {
            CheckoutValidation::ProjectlessDirectory
        } else {
            CheckoutValidation::RegisteredFolder
        };
        let validation_expected =
            if matches!(validation_policy, CheckoutValidation::ProjectlessDirectory) {
                Ok(Vec::new())
            } else {
                self.cached_checkout_owner(wt_path, cx)
                    .map(|owner| vec![owner.path])
            };
        let session = cx.new(|cx| {
            TerminalSession::spawn_checked(
                &target,
                &[],
                None,
                Box::new(move || {
                    let expected = validation_expected.as_ref().map_err(Clone::clone)?;
                    validate_checkout_paths(
                        &validation_projects,
                        &validation_paths,
                        validation_policy,
                        expected,
                    )
                }),
                cx,
            )
        });
        if let Some(err) = session.read(cx).spawn_error().map(str::to_string) {
            self.toast.update(cx, |t, cx| {
                t.set_error(format!("terminal failed: {err}"), cx);
            });
            return false;
        }
        if let Ok(owner) = self.cached_checkout_owner(wt_path, cx) {
            self.projects.update(cx, |service, _| {
                service.track_worktree_shell_owner(id, owner.path);
            });
        }
        let meta = crate::entities::session_registry::SessionMeta {
            id,
            project: String::new(),
            wt_path: wt_path.to_string(),
            agent: grove_core::agent::Agent::Terminal,
            context_roots: Vec::new(),
            temp_bundle_path: None,
            label,
            restored_title: None,
            spawned_at: std::time::Instant::now(),
            attention: None,
            tmux: false,
            tmux_name: None,
        };
        self.registry.update(cx, |r, cx| {
            r.push_wt_shell(wt_path, meta, Some(session));
            cx.notify();
        });
        true
    }

    /// Launch one agent with the same ordered multi-worktree context as the previous launcher.
    pub fn launch_multi_root_session(
        &mut self,
        worktree_paths: &[String],
        agent: Agent,
        cx: &mut Context<Self>,
    ) -> bool {
        let targets = worktree_paths
            .iter()
            .map(|path| {
                let owner = self.cached_checkout_owner(path, cx)?;
                if owner.archived {
                    return Err("Project is archived.".to_owned());
                }
                let canonical = fs_err::canonicalize(path)
                    .map_err(|_| format!("{path}: worktree is no longer available"))?;
                Ok((owner.name, canonical.to_string_lossy().into_owned()))
            })
            .collect::<Result<Vec<_>, String>>();
        let targets = match targets {
            Ok(targets) => targets,
            Err(error) => {
                self.toast
                    .clone()
                    .update(cx, |toast, cx| toast.set_error(error, cx));
                return false;
            }
        };
        let Some((primary_project, primary_cwd)) = targets.first().cloned() else {
            self.toast.clone().update(cx, |toast, cx| {
                toast.set_error("select one or more worktrees", cx);
            });
            return false;
        };
        let extra_roots = targets
            .iter()
            .skip(1)
            .map(|(_, path)| path.clone())
            .collect::<Vec<_>>();
        let context_roots = targets
            .iter()
            .map(|(project, wt_path)| grove_core::session_meta::ContextRoot {
                project: project.clone(),
                wt_path: wt_path.clone(),
            })
            .collect::<Vec<_>>();
        let launch_args = {
            let store = &cx.global::<SettingsState>().store;
            agent.multi_root_launch_args_for_backend(
                native_codex_enabled(store, None),
                store.yolo_enabled,
                store.chrome_enabled.unwrap_or(false),
                &extra_roots,
            )
        };
        let Some(launch_args) = launch_args else {
            self.toast.clone().update(cx, |toast, cx| {
                toast.set_error("multi-worktree sessions require Claude or Codex", cx);
            });
            return false;
        };
        let temp_bundle_path = if matches!(
            agent,
            grove_core::agent::Agent::OpenCode | grove_core::agent::Agent::Terminal
        ) {
            match grove_core::multi_root::SymlinkBundle::create(&extra_roots) {
                Ok(bundle) => Some(bundle.into_path().to_string_lossy().into_owned()),
                Err(error) => {
                    self.toast.clone().update(cx, |toast, cx| {
                        toast.set_error(
                            format!("could not prepare multi-worktree session: {error}"),
                            cx,
                        );
                    });
                    return false;
                }
            }
        } else {
            None
        };
        let did_launch = self.spawn_session_in_with_context(
            primary_project.clone(),
            primary_cwd.clone(),
            agent,
            launch_args,
            context_roots,
            temp_bundle_path,
            cx,
        );
        if did_launch {
            self.record_recent_launch(primary_project, primary_cwd, agent, cx);
        } else if let Some(pending) = &mut self.pending_managed_launch {
            pending.record_recent = true;
        }
        did_launch
    }

    fn record_recent_launch(
        &mut self,
        project: String,
        wt_path: String,
        agent: Agent,
        cx: &mut Context<Self>,
    ) {
        SettingsState::update(cx, move |store| {
            store.recent_launches.retain(|recent| {
                !(recent.project == project && recent.wt_path == wt_path && recent.agent == agent)
            });
            store.recent_launches.insert(
                0,
                grove_core::storage::RecentLaunch {
                    project,
                    wt_path,
                    agent,
                },
            );
            store.recent_launches.truncate(12);
        });
        self.toast
            .clone()
            .update(cx, |toast, cx| toast.set_toast("launched 1 session", cx));
    }

    pub fn kill_session(&mut self, id: SessionId, cx: &mut Context<Self>) {
        self.registry.update(cx, |r, cx| {
            r.remove_background(id, cx);
            cx.notify();
        });
        let (live, saved) = (self.live_tiles(cx), Self::saved_grid_order(cx));
        self.state.update(cx, |s, cx| {
            s.on_session_removed(id);
            s.disarm_kill();
            s.reconcile_after_teardown(&live, &saved);
            cx.notify();
        });
        self.persist_grid_order(cx);
    }
}

#[cfg(test)]
mod tests;

// Empty project identity is intentional, even when the directory later belongs
// to a registered project. Never infer membership for a No project session.
fn restore_session_project(
    saved: &mut grove_core::codex::SavedSession,
    projects: &[grove_core::storage::Project],
) -> bool {
    if !std::path::Path::new(&saved.cwd).is_dir() {
        return false;
    }
    if saved.project.is_empty() {
        return true;
    }
    match grove_core::session_meta::project_owner(projects, &saved.cwd) {
        Ok(owner) if !owner.archived => {
            saved.project.clone_from(&owner.name);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod projectless_recovery_tests {
    #[test]
    fn projectless_recovery_preserves_codex_and_claude_identity_and_draft() {
        let directory = tempfile::tempdir().unwrap();
        for provider in ["codex", "claude"] {
            let mut saved: grove_core::codex::SavedSession =
                serde_json::from_value(serde_json::json!({
                    "key": "projectless-fixture", "provider": provider, "project": "",
                    "cwd": directory.path(), "label": "Session", "draft": "keep this draft",
                    "thread_id": "retained-thread", "roots": [], "items": []
                }))
                .unwrap();
            assert!(super::restore_session_project(&mut saved, &[]));
            assert!(saved.project.is_empty());
            assert_eq!(saved.draft, "keep this draft");
            assert_eq!(saved.thread_id.as_deref(), Some("retained-thread"));
            saved.cwd = directory
                .path()
                .join("missing")
                .to_string_lossy()
                .into_owned();
            assert!(!super::restore_session_project(&mut saved, &[]));
        }
    }
}
