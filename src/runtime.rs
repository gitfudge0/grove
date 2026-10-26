//! Session orchestration retained independently of the window during the UI rebuild.
//! Construction loads state; spawning and tmux attachment remain explicit operations.

use crate::{
    entities::{
        activity_store::ActivityStore,
        project_tree::ProjectTree,
        session_registry::{SessionId, SessionRegistry},
        shell_environment::ShellEnvironment,
        terminal_session::TerminalSession,
        toast::ToastState,
        upgrade::Upgrade,
        workspace_state::{LiveTile, PtyPane, WorkspaceState},
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

fn resolve_multi_root_worktrees(
    selected_paths: &[String],
    projects: &[grove_core::storage::Project],
) -> Result<Vec<(String, String)>, String> {
    selected_paths
        .iter()
        .map(|selected_path| resolve_multi_root_worktree(selected_path, projects))
        .collect()
}

fn resolve_multi_root_worktree(
    selected_path: &str,
    projects: &[grove_core::storage::Project],
) -> Result<(String, String), String> {
    let canonical = fs_err::canonicalize(selected_path)
        .map_err(|_| format!("{selected_path}: worktree is no longer available"))?;
    let canonical_text = canonical.to_string_lossy().into_owned();

    for project in projects {
        let listed = grove_core::git::list_worktrees(&project.path);
        if listed
            .into_iter()
            .any(|worktree| fs_err::canonicalize(worktree.path).is_ok_and(|path| path == canonical))
        {
            return Ok((project.name.clone(), canonical_text));
        }
    }

    Err(format!("{selected_path}: worktree is no longer available"))
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
            tree.update(cx, |t, _| {
                t.set_active_worktrees(idx, grove_core::git::list_worktrees(&path));
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
            let exists = cx
                .global::<SettingsState>()
                .store
                .projects
                .iter()
                .any(|project| {
                    project.path == run.project_path
                        && !project.archived
                        && project.scripts.effective_entries().iter().any(|entry| {
                            entry.id == run.script_id
                                && matches!(
                                    entry.kind,
                                    grove_core::storage::ProjectActionKind::Shell { .. }
                                )
                        })
                })
                && std::path::Path::new(&run.checkout_path).exists();
            if !exists {
                run.terminal.update(cx, TerminalSession::stop_script);
            }
            exists
        });
        let idx = self.state.read(cx).proj_idx();
        let active = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .map(|p| p.path.clone());
        self.tree.update(cx, |tree, cx| {
            tree.rebuild_wt_cache();
            if let Some(path) = active {
                tree.set_active_worktrees(idx, grove_core::git::list_worktrees(&path));
            }
            cx.notify();
        });
        match event {
            ProjectEvent::WorktreeAdded { path } => {
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

    /// Spawn by concrete project name + worktree path; the palette's target may live in a project with no cached snapshot position.
    pub fn spawn_session_in(
        &mut self,
        name: String,
        cwd: String,
        agent: Agent,
        cx: &mut Context<Self>,
    ) -> bool {
        let args = {
            let store = &cx.global::<SettingsState>().store;
            agent.launch_args_for_backend(
                native_codex_enabled(store, None),
                store.yolo_enabled,
                store.chrome_enabled.unwrap_or(false),
            )
        };
        self.spawn_session_in_with_args(name, cwd, agent, args, cx)
    }

    /// Spawn one agent session with a primary worktree and agent-specific extra arguments.
    pub fn spawn_session_in_with_args(
        &mut self,
        name: String,
        cwd: String,
        agent: Agent,
        args: Vec<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.spawn_session_in_with_context(name, cwd, agent, args, Vec::new(), None, cx)
    }

    /// Spawn one agent session with its ordered writable worktree context.
    #[allow(clippy::too_many_arguments)] // Preserve the existing launch API during UI extraction.
    pub fn spawn_session_in_with_context(
        &mut self,
        name: String,
        cwd: String,
        agent: Agent,
        args: Vec<String>,
        context_roots: Vec<grove_core::session_meta::ContextRoot>,
        temp_bundle_path: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.pending_managed_launch.is_some() {
            if let Some(path) = temp_bundle_path.as_deref() {
                grove_core::multi_root::cleanup_path(std::path::Path::new(path));
            }
            return false;
        }
        let projects = &cx.global::<SettingsState>().store.projects;
        let launch_error = std::iter::once(cwd.as_str())
            .chain(context_roots.iter().map(|root| root.wt_path.as_str()))
            .find_map(
                |wt| match grove_core::session_meta::project_owner(projects, wt) {
                    Ok(project) if self.projects.read(cx).is_removing(&project.path) => Some(
                        "Cannot launch a session while this project is being removed.".to_string(),
                    ),
                    Ok(_) if self.projects.read(cx).is_worktree_removing(wt) => Some(
                        "Cannot launch a session while this worktree is being removed.".to_string(),
                    ),
                    Ok(_) => None,
                    Err(error) => Some(error),
                },
            );
        if let Some(error) = launch_error {
            if let Some(path) = temp_bundle_path.as_deref() {
                grove_core::multi_root::cleanup_path(std::path::Path::new(path));
            }
            self.toast
                .update(cx, |toast, cx| toast.set_error(error, cx));
            return false;
        }
        if matches!(agent, Agent::Codex | Agent::Claude)
            && native_codex_enabled(&cx.global::<SettingsState>().store, None)
        {
            return self.spawn_native_codex(
                PendingManagedLaunch {
                    name,
                    cwd,
                    agent,
                    args,
                    context_roots,
                    temp_bundle_path,
                    record_recent: false,
                },
                cx,
            );
        }
        if should_choose_backend(
            cx.global::<SettingsState>().store.tmux_enabled,
            grove_core::tmux::available,
        ) {
            self.queue_backend_choice(
                PendingManagedLaunch {
                    name,
                    cwd,
                    agent,
                    args,
                    context_roots,
                    temp_bundle_path,
                    record_recent: false,
                },
                cx,
            );
            return false;
        }
        self.state.update(cx, |s, cx| {
            s.set_open_agent_menu(None);
            cx.notify();
        });
        let use_tmux = managed_session_uses_tmux(cx.global::<SettingsState>().store.tmux_enabled);
        let (id, extra_args, state_file, target) = self.registry.update(cx, |r, cx| {
            let id = r.insert_meta_with_context(
                name.clone(),
                cwd.clone(),
                agent,
                context_roots.clone(),
                temp_bundle_path.clone(),
            );
            let label = r.meta(id).map_or_else(String::new, |m| m.label.clone());
            let extra_args = r.take_attention_args(id);
            let state_file = r.attention_files(id).map(|f| f.state_file.clone());
            cx.notify();
            (
                id,
                extra_args,
                state_file,
                crate::entities::session_registry::SpawnTarget {
                    cwd,
                    agent,
                    project: name,
                    label,
                    args,
                    context_roots,
                    temp_bundle_path: temp_bundle_path.clone(),
                    use_tmux,
                },
            )
        });
        let session = cx.new(|cx| {
            crate::entities::terminal_session::TerminalSession::spawn(
                &target,
                &extra_args,
                state_file.as_deref(),
                cx,
            )
        });
        let tmux_name = match session.read(cx).backend() {
            crate::entities::terminal_session::Backend::Tmux { name } => Some(name.clone()),
            crate::entities::terminal_session::Backend::Native => None,
        };
        let tmux_backed = tmux_name.is_some();
        let spawn_error = session.read(cx).spawn_error().map(str::to_string);
        if let Some(e) = spawn_error.as_deref() {
            if let Some(path) = temp_bundle_path.as_deref() {
                grove_core::multi_root::cleanup_path(std::path::Path::new(path));
            }
            crate::telemetry::track("error", vec![("kind", "spawn_failed".into())]);
            let msg = format!("failed to start session: {e}");
            self.toast.update(cx, |t, cx| t.set_error(msg, cx));
        }
        self.registry.update(cx, |r, cx| {
            r.attach(id, session, tmux_name);
            cx.notify();
        });
        if spawn_error.is_none() {
            let (open, open_tmux) = {
                let r = self.registry.read(cx);
                (
                    r.len() as u64,
                    r.all().iter().filter(|m| m.tmux).count() as u64,
                )
            };
            crate::telemetry::track(
                "session_created",
                vec![
                    ("agent", agent.label().into()),
                    ("tmux", tmux_backed.into()),
                    ("open_sessions", open.into()),
                    ("open_native", (open - open_tmux).into()),
                    ("open_tmux", open_tmux.into()),
                ],
            );
        }
        let snap = self.snapshot(cx);
        let old = self.state.read(cx).proj_idx();
        ProjectTree::adopt_session_project(&self.tree.clone(), &snap, id, old, cx);
        self.state.update(cx, |s, cx| {
            s.select_session(id, &snap);
            cx.notify();
        });
        spawn_error.is_none()
    }

    fn attach_native_codex(
        &mut self,
        saved: grove_core::codex::SavedSession,
        args: Vec<String>,
        cx: &mut Context<Self>,
    ) -> SessionId {
        let id = self.registry.update(cx, |registry, _| {
            registry.insert_meta_with_context(
                saved.project.clone(),
                saved.cwd.clone(),
                saved.provider.agent(),
                saved.roots.clone(),
                None,
            )
        });
        let session =
            cx.new(|cx| crate::entities::codex_session::CodexSession::new(saved, args, cx));
        let handle = session.read(cx).handle.clone();
        self.registry.update(cx, |registry, cx| {
            registry.take_attention_args(id);
            registry.attach_codex(id, session.clone(), handle);
            cx.notify();
        });
        self.observe_native_title(id, &session, cx);
        id
    }
    fn observe_native_title(
        &mut self,
        id: SessionId,
        session: &Entity<crate::entities::codex_session::CodexSession>,
        cx: &mut Context<Self>,
    ) {
        let title = session.read(cx).state.saved.title.clone();
        self.registry.update(cx, |registry, cx| {
            if registry.set_restored_title(id, title) {
                cx.notify();
            }
        });
        let observer = cx.observe(session, move |runtime, session, cx| {
            let title = session.read(cx).state.saved.title.clone();
            runtime.registry.update(cx, |registry, cx| {
                if registry.set_restored_title(id, title) {
                    cx.notify();
                }
            });
        });
        self.native_title_observers.insert(id, observer);
    }
    fn spawn_native_codex(&mut self, launch: PendingManagedLaunch, cx: &mut Context<Self>) -> bool {
        let PendingManagedLaunch {
            name: project,
            cwd,
            args,
            context_roots: roots,
            agent,
            ..
        } = launch;
        let provider = if agent == Agent::Claude {
            grove_core::codex::Provider::Claude
        } else {
            grove_core::codex::Provider::Codex
        };
        if let Err(error) = provider.launch_options(&args) {
            self.toast
                .update(cx, |toast, cx| toast.set_error(error, cx));
            return false;
        }
        let key = match grove_core::identity::random_id() {
            Ok(key) => key,
            Err(error) => {
                self.toast
                    .update(cx, |toast, cx| toast.set_error(error, cx));
                return false;
            }
        };
        let saved = grove_core::codex::SavedSession {
            rich_plans: grove_core::rich_plan::PlanStore::default(),
            claude_resume: None,
            edit_checkpoints: Vec::new(),
            pending_edit: None,
            provider,
            composer: grove_core::codex::ComposerSettings::default(),
            context: None,
            key,
            project,
            cwd,
            label: provider.label().into(),
            title: None,
            title_generation: None,
            roots,
            thread_id: None,
            items: Vec::new(),
            work_periods: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
        };
        let id = self.attach_native_codex(saved, args, cx);
        let snap = self.snapshot(cx);
        let old = self.state.read(cx).proj_idx();
        ProjectTree::adopt_session_project(&self.tree.clone(), &snap, id, old, cx);
        self.state.update(cx, |state, cx| {
            state.select_session(id, &snap);
            cx.notify();
        });
        true
    }
    fn restore_native_codex(&mut self, cx: &mut Context<Self>) {
        let load = cx.background_spawn(async { grove_core::codex::load_sessions() });
        cx.spawn(async move |this, cx| {
            let loaded = load.await;
            let _ = this.update(cx, |runtime, cx| {
                runtime.restoring_codex = false;
                if !native_codex_enabled(&cx.global::<SettingsState>().store, None) {
                    return;
                }
                match loaded {
                    Ok(recovery) => {
                        if !recovery.warnings.is_empty() {
                            runtime.toast.update(cx, |toast, cx| {
                                toast.set_error(recovery.warnings.join("\n"), cx);
                            });
                        }
                        for mut saved in recovery.sessions {
                            let projects = &cx.global::<SettingsState>().store.projects;
                            if let Ok(owner) =
                                grove_core::session_meta::project_owner(projects, &saved.cwd)
                            {
                                if !owner.archived && std::path::Path::new(&saved.cwd).is_dir() {
                                    saved.project.clone_from(&owner.name);
                                    runtime.attach_native_codex(saved, Vec::new(), cx);
                                }
                            }
                        }
                    }
                    Err(error) => runtime
                        .toast
                        .update(cx, |toast, cx| toast.set_error(error, cx)),
                }
            });
        })
        .detach();
    }

    pub fn has_pending_managed_launch(&self) -> bool {
        self.pending_managed_launch.is_some()
    }

    fn queue_backend_choice(&mut self, launch: PendingManagedLaunch, cx: &mut Context<Self>) {
        self.pending_managed_launch = Some(launch);
        cx.emit(RuntimeEvent::BackendChoiceRequested);
    }

    pub fn cancel_pending_managed_launch(&mut self) {
        if let Some(pending) = self.pending_managed_launch.take() {
            if let Some(path) = pending.temp_bundle_path.as_deref() {
                grove_core::multi_root::cleanup_path(std::path::Path::new(path));
            }
        }
    }

    /// The caller saves the backend preference first. Taking the request makes
    /// repeated confirmation clicks harmless and preserves its original args.
    pub fn resume_pending_managed_launch(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pending) = self.pending_managed_launch.take() else {
            return false;
        };
        let (name, cwd, agent, record_recent) = (
            pending.name.clone(),
            pending.cwd.clone(),
            pending.agent,
            pending.record_recent,
        );
        let launched = self.spawn_session_in_with_context(
            pending.name,
            pending.cwd,
            pending.agent,
            pending.args,
            pending.context_roots,
            pending.temp_bundle_path,
            cx,
        );
        if launched && record_recent {
            self.record_recent_launch(name, cwd, agent, cx);
        }
        launched
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
            r.close_home(i);
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
        if !grove_core::tmux::available() {
            return;
        }
        let discovered = grove_core::tmux::list_grove_sessions();
        if discovered.is_empty() {
            return;
        }
        let plan = {
            let existing = self.registry.read(cx).all().to_vec();
            let store = &cx.global::<SettingsState>().store;
            let paths: HashMap<String, String> = store
                .active_projects()
                .map(|(_, p)| (p.name.clone(), p.path.clone()))
                .collect();
            let wt_order = |project: &str| -> Vec<String> {
                paths.get(project).map_or_else(Vec::new, |path| {
                    grove_core::git::list_worktrees(path)
                        .into_iter()
                        .map(|w| w.path)
                        .collect()
                })
            };
            crate::reattach::plan(&discovered, &existing, &wt_order)
        };
        let dims = self.last_pty_dims;
        for entry in plan {
            let name = entry.session.name.clone();
            // `dims` only seeds the emulator; the tmux client attaches later at the painting tile's actual dims.
            let session = cx.new(|cx| TerminalSession::attach_existing(&name, dims.0, dims.1, cx));
            if let Some(err) = session.read(cx).spawn_error().map(str::to_string) {
                tracing::warn!(session = %name, error = %err, "reattach failed; skipping");
                continue;
            }
            let id = self
                .registry
                .update(cx, |r, _| r.insert_reattached(entry.at, &entry.session));
            self.registry.update(cx, |r, cx| {
                r.attach(id, session, Some(name.clone()));
                cx.notify();
            });
        }
        cx.notify();
    }

    pub fn spawn_wt_shell(&mut self, wt_path: &str, cx: &mut Context<Self>) {
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
        let session = cx.new(|cx| TerminalSession::spawn(&target, &[], None, cx));
        if let Some(err) = session.read(cx).spawn_error().map(str::to_string) {
            self.toast.update(cx, |t, cx| {
                t.set_error(format!("terminal failed: {err}"), cx);
            });
            return;
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
        self.state.update(cx, |s, cx| {
            s.focus_pane(PtyPane::Panel);
            cx.notify();
        });
    }

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
            .projects
            .iter()
            .find(|p| p.path == project_path)
            .and_then(|p| {
                p.scripts
                    .effective_entries()
                    .into_iter()
                    .find(|e| e.id == entry_id)
            })
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
            let worktree = grove_core::git::list_worktrees_checked(&project.path)
                .map_err(|error| format!("Could not list worktrees: {error}"))?
                .into_iter()
                .find(|worktree| {
                    fs_err::canonicalize(&worktree.path).is_ok_and(|path| path == canonical)
                })
                .ok_or_else(|| format!("{wt_path}: worktree is no longer available"))?;
            if self.projects.read(cx).is_worktree_removing(&worktree.path) {
                return Err("Cannot launch a script while this worktree is being removed.".into());
            }
            let owner = grove_core::session_meta::project_owner(projects, &worktree.path)?;
            if owner.path != project.path {
                return Err(format!("{wt_path}: worktree belongs to another project"));
            }
            Ok(Some((
                project.name.clone(),
                worktree.path,
                script.to_string(),
            )))
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

        let session = cx.new(|cx| TerminalSession::spawn_script(&script, &cwd, cx));
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
                .projects
                .iter()
                .find(|project| project.path == project_path)
                .and_then(|project| {
                    project
                        .scripts
                        .effective_entries()
                        .into_iter()
                        .find(|entry| entry.id == script_id)
                })
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
                _observer: std::rc::Rc::new(cx.observe(&session, |_, _, cx| cx.notify())),
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
            registry.attach(id, session, None);
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

    /// Launch one agent with the same ordered multi-worktree context as the previous launcher.
    pub fn launch_multi_root_session(
        &mut self,
        worktree_paths: &[String],
        agent: Agent,
        cx: &mut Context<Self>,
    ) -> bool {
        let targets = {
            let store = &cx.global::<SettingsState>().store;
            resolve_multi_root_worktrees(worktree_paths, &store.projects)
        };
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
            r.remove(id);
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
mod tests {
    use super::*;

    #[test]
    fn native_codex_requires_opt_in_and_respects_tmux_override() {
        let mut store = grove_core::storage::Store::default();
        assert!(!native_codex_enabled(&store, None));
        assert!(!native_codex_enabled(&store, Some(false)));
        store.native_agents_enabled = true;
        assert_eq!(native_codex_enabled(&store, None), !cfg!(windows));
        assert_eq!(native_codex_enabled(&store, Some(false)), !cfg!(windows));
        assert!(!native_codex_enabled(&store, Some(true)));
        store.native_agents_enabled = false;
        assert!(!native_codex_enabled(&store, Some(false)));
    }

    #[test]
    fn managed_launch_requires_explicit_tmux_preference() {
        assert!(!saved_tmux_preference_enabled(None));
        assert!(!saved_tmux_preference_enabled(Some(false)));
        assert!(saved_tmux_preference_enabled(Some(true)));
    }

    #[test]
    fn backend_choice_only_on_first_managed_launch_with_tmux() {
        assert!(should_choose_backend(None, || true));
        assert!(!should_choose_backend(None, || false));
        assert!(!should_choose_backend(Some(false), || panic!(
            "must not probe tmux"
        )));
        assert!(!should_choose_backend(Some(true), || panic!(
            "must not probe tmux"
        )));
    }

    #[test]
    fn root_binary_tests_cannot_use_production_tmux() {
        assert!(!managed_session_uses_tmux(Some(true)));
        assert!(!tmux_discovery_enabled());
    }

    use std::{
        path::Path,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    struct GitFixture(std::path::PathBuf);

    impl GitFixture {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for GitFixture {
        fn drop(&mut self) {
            let _ = fs_err::remove_dir_all(&self.0);
        }
    }

    fn script_project(path: &str, script: &str) -> grove_core::storage::Project {
        grove_core::storage::Project {
            name: "script fixture".into(),
            path: path.into(),
            scripts: grove_core::storage::ProjectScripts {
                run: Some(script.into()),
                ..Default::default()
            },
            archived: false,
            worktree_dir: None,
        }
    }

    fn init_script_runtime(cx: &mut App, project: grove_core::storage::Project) -> Entity<Runtime> {
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![project],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.new(Runtime::new)
    }

    fn git_fixture() -> GitFixture {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
        let dir = GitFixture(std::env::temp_dir().join(format!(
            "grove-script-test-{}-{unique}-{sequence}",
            std::process::id()
        )));
        fs_err::create_dir(&dir.0).unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(dir.path())
            .status()
            .unwrap()
            .success());
        dir
    }

    fn pending_fixture(path: &str) -> PendingManagedLaunch {
        PendingManagedLaunch {
            name: "script fixture".into(),
            cwd: path.into(),
            agent: Agent::Terminal,
            args: Vec::new(),
            context_roots: Vec::new(),
            temp_bundle_path: None,
            record_recent: false,
        }
    }

    #[gpui::test]
    fn deferred_launch_does_not_spawn_and_cancel_discards_it(cx: &mut gpui::TestAppContext) {
        let repo = git_fixture();
        let path = repo
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        cx.update(|cx| {
            let runtime = init_script_runtime(cx, script_project(&path, "true"));
            runtime.update(cx, |runtime, cx| {
                runtime.queue_backend_choice(pending_fixture(&path), cx);
            });
            assert!(runtime.read(cx).has_pending_managed_launch());
            assert!(runtime.read(cx).registry.read(cx).is_empty());
            runtime.update(cx, |runtime, _| runtime.cancel_pending_managed_launch());
            assert!(!runtime.read(cx).has_pending_managed_launch());
            assert!(!runtime.update(cx, Runtime::resume_pending_managed_launch));
            assert!(runtime.read(cx).registry.read(cx).is_empty());
        });
    }

    #[gpui::test]
    fn deferred_launch_resumes_once_with_original_target(cx: &mut gpui::TestAppContext) {
        let repo = git_fixture();
        let path = repo
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        cx.update(|cx| {
            let runtime = init_script_runtime(cx, script_project(&path, "true"));
            runtime.update(cx, |runtime, cx| {
                runtime.queue_backend_choice(pending_fixture(&path), cx);
            });
            assert!(runtime.read(cx).registry.read(cx).is_empty());
            cx.global_mut::<SettingsState>().store.tmux_enabled = Some(false);
            assert!(runtime.update(cx, Runtime::resume_pending_managed_launch));
            assert!(!runtime.read(cx).has_pending_managed_launch());
            assert!(!runtime.update(cx, Runtime::resume_pending_managed_launch));
            let registry = runtime.read(cx).registry.read(cx);
            assert_eq!(registry.len(), 1);
            assert_eq!(registry.all()[0].wt_path, path);
            assert_eq!(registry.all()[0].agent, Agent::Terminal);
        });
    }

    #[gpui::test]
    fn blank_run_script_does_not_spawn_or_register(cx: &mut gpui::TestAppContext) {
        let repo = git_fixture();
        let canonical = repo.path().canonicalize().unwrap();
        let path = canonical.to_str().unwrap();
        cx.update(|cx| {
            let runtime = init_script_runtime(cx, script_project(path, "  \n "));
            assert!(!runtime.update(cx, |runtime, cx| runtime.spawn_run_script(path, path, cx)));
            let registry = runtime.read(cx).registry.read(cx);
            assert!(registry.wt_shells(path).is_empty());
            assert_eq!(registry.active_wt_shell_idx(path), None);
            assert!(registry.is_empty());
        });
    }

    #[gpui::test]
    fn run_script_uses_worktree_cwd_without_selecting_session(cx: &mut gpui::TestAppContext) {
        let repo = git_fixture();
        let canonical = repo.path().canonicalize().unwrap();
        let path = canonical.to_str().unwrap();
        let output = repo.path().join("script-cwd.txt");
        let script = format!("pwd > '{}'", output.display());
        let runtime = cx.update(|cx| {
            let runtime = init_script_runtime(cx, script_project(path, &script));
            assert!(runtime.update(cx, |runtime, cx| runtime.spawn_run_script(path, path, cx)));
            let state = runtime.read(cx);
            assert!(state.registry.read(cx).is_empty());
            assert_eq!(state.script_runs.len(), 1);
            let run = &state.script_runs[0];
            assert_eq!(run.project_path, path);
            assert_eq!(run.checkout_path, path);
            assert_eq!(run.command, script);
            assert!(state.state.read(cx).active_session().is_none());
            assert_eq!(
                run.terminal.read(cx).backend(),
                &crate::entities::terminal_session::Backend::Native
            );
            runtime
        });
        let mut actual = None;
        // A login shell can start slowly on a busy CI runner. Wait for its
        // complete pwd line rather than the file's creation or a fixed poll count.
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Ok(contents) = fs_err::read_to_string(&output) {
                if contents.ends_with('\n') {
                    actual = Some(contents);
                    break;
                }
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            actual.as_deref().map(str::trim),
            Some(path),
            "login shell did not write the expected cwd within 10 seconds"
        );
        drop(runtime);
    }

    #[gpui::test]
    fn failed_run_script_spawn_shows_error_without_bogus_shell(cx: &mut gpui::TestAppContext) {
        let repo = git_fixture();
        let canonical = repo.path().canonicalize().unwrap();
        let path = canonical.to_str().unwrap();
        cx.update(|cx| {
            let runtime = init_script_runtime(cx, script_project(path, "\0"));
            assert!(!runtime.update(cx, |runtime, cx| runtime.spawn_run_script(path, path, cx)));
            let registry = runtime.read(cx).registry.read(cx);
            assert!(registry.wt_shells(path).is_empty());
            assert_eq!(registry.active_wt_shell_idx(path), None);
            assert!(registry.is_empty());
            let toast = runtime.read(cx).toast.read(cx);
            assert!(toast
                .current()
                .unwrap()
                .message
                .starts_with("terminal failed:"));
        });
    }

    #[gpui::test]
    fn repeated_run_scripts_have_distinct_inline_ids_and_removal_guard(
        cx: &mut gpui::TestAppContext,
    ) {
        let repo = git_fixture();
        let canonical = repo.path().canonicalize().unwrap();
        let path = canonical.to_str().unwrap();
        cx.update(|cx| {
            let runtime = init_script_runtime(cx, script_project(path, "sleep 30"));
            assert!(runtime.update(cx, |runtime, cx| runtime.spawn_run_script(path, path, cx)));
            assert!(runtime.update(cx, |runtime, cx| runtime.spawn_run_script(path, path, cx)));
            let state = runtime.read(cx);
            assert!(state.registry.read(cx).is_empty());
            assert_eq!(state.script_runs.len(), 2);
            let first = state.script_runs[0].clone();
            let second = state.script_runs[1].clone();
            assert_ne!(first.id, second.id);
            assert_eq!(first.script_id, second.script_id);
            let service = state.projects.clone();
            assert!(service
                .update(cx, |service, cx| service.archive_blockers(path, cx))
                .is_err());
            first.terminal.update(cx, TerminalSession::stop_script);
            second.terminal.update(cx, TerminalSession::stop_script);
            runtime.update(cx, |runtime, _| {
                runtime.script_runs.retain(|run| run.id != first.id);
            });
            assert_eq!(runtime.read(cx).script_runs.len(), 1);
            assert_eq!(runtime.read(cx).script_runs[0].id, second.id);
        });
    }

    #[gpui::test]
    fn script_edits_preserve_run_snapshots_and_stop_retyped_or_deleted_runs(
        cx: &mut gpui::TestAppContext,
    ) {
        use grove_core::storage::{ProjectAction, ProjectActionKind, ProjectScripts};
        let repo = git_fixture();
        let path = repo
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let scripts = ProjectScripts {
            entries: vec![ProjectAction {
                id: "stable-action".into(),
                name: "Original".into(),
                kind: ProjectActionKind::Shell {
                    command: "sleep 30".into(),
                },
            }],
            ..Default::default()
        };
        let runtime = cx.update(|cx| {
            let mut project = script_project(&path, "");
            project.scripts = scripts.clone();
            let runtime = init_script_runtime(cx, project);
            assert!(
                runtime.update(cx, |runtime, cx| runtime.spawn_project_script(
                    &path,
                    &path,
                    "stable-action",
                    cx
                ))
            );
            runtime
        });
        let (service, original) = cx.update(|cx| {
            let state = runtime.read(cx);
            (state.projects.clone(), state.script_runs[0].clone())
        });
        cx.update(|cx| {
            assert!(service
                .update(cx, |service, cx| service.archive_project_by_path(&path, cx))
                .is_err());
            let mut renamed = scripts.clone();
            renamed.entries[0].name = "Renamed".into();
            renamed.entries[0].kind = ProjectActionKind::Shell {
                command: "sleep 31".into(),
            };
            service
                .update(cx, |service, cx| {
                    service.update_project(&path, "script fixture".into(), renamed, cx)
                })
                .unwrap();
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let state = runtime.read(cx);
            assert_eq!(state.script_runs.len(), 1);
            assert_eq!(state.script_runs[0].id, original.id);
            assert_eq!(state.script_runs[0].name, "Original");
            assert_eq!(state.script_runs[0].command, "sleep 30");
            assert_eq!(state.script_runs[0].checkout_path, path);
            assert!(!original.terminal.read(cx).has_exited());
            let mut prompt = scripts.clone();
            prompt.entries[0].kind = ProjectActionKind::Prompt {
                template: "Review the changes".into(),
            };
            service
                .update(cx, |service, cx| {
                    service.update_project(&path, "script fixture".into(), prompt, cx)
                })
                .unwrap();
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(runtime.read(cx).script_runs.is_empty());
            assert!(original.terminal.read(cx).has_exited());
            service
                .update(cx, |service, cx| {
                    service.update_project(&path, "script fixture".into(), scripts.clone(), cx)
                })
                .unwrap();
        });
        cx.run_until_parked();
        let deleted = cx.update(|cx| {
            assert!(
                runtime.update(cx, |runtime, cx| runtime.spawn_project_script(
                    &path,
                    &path,
                    "stable-action",
                    cx
                ))
            );
            let deleted = runtime.read(cx).script_runs[0].clone();
            assert_ne!(deleted.id, original.id);
            service
                .update(cx, |service, cx| {
                    service.update_project(
                        &path,
                        "script fixture".into(),
                        ProjectScripts::default(),
                        cx,
                    )
                })
                .unwrap();
            deleted
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(runtime.read(cx).script_runs.is_empty());
            assert!(deleted.terminal.read(cx).has_exited());
            assert!(service
                .update(cx, |service, cx| service.archive_blockers(&path, cx))
                .unwrap()
                .is_empty());
        });
    }

    #[gpui::test]
    fn multi_root_launch_returns_false_for_empty_or_stale_selection(cx: &mut gpui::TestAppContext) {
        let repo = git_fixture();
        let canonical = repo.path().canonicalize().unwrap();
        let path = canonical.to_str().unwrap();
        let stale = canonical
            .join("removed-worktree")
            .to_string_lossy()
            .into_owned();
        cx.update(|cx| {
            let runtime = init_script_runtime(cx, script_project(path, "echo test"));
            assert!(!runtime.update(cx, |runtime, cx| {
                runtime.launch_multi_root_session(&[], Agent::Terminal, cx)
            }));
            assert!(!runtime.update(cx, |runtime, cx| {
                runtime.launch_multi_root_session(&[stale], Agent::Terminal, cx)
            }));
            assert!(runtime.read(cx).registry.read(cx).is_empty());
            assert!(cx
                .global::<SettingsState>()
                .store
                .recent_launches
                .is_empty());
        });
    }

    #[gpui::test]
    fn empty_shell_runtime_does_not_spawn_sessions_or_rewrite_preferences(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            let store = grove_core::storage::Store {
                onboarded: true,
                ui_zoom: Some(1.4),
                grid_order: vec!["saved-session-order".to_string()],
                ..Default::default()
            };
            let before = serde_json::to_value(&store).ok();
            cx.set_global(SettingsState::new(store));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let runtime = cx.new(Runtime::new);
            assert!(runtime.read(cx).registry.read(cx).is_empty());
            assert_eq!(runtime.read(cx).registry.read(cx).home_terminal_count(), 0);
            runtime.update(cx, |runtime, cx| runtime.close_home_terminal(0, cx));
            assert_eq!(runtime.read(cx).registry.read(cx).home_terminal_count(), 0);
            runtime.update(cx, Runtime::shutdown);
            assert_eq!(
                serde_json::to_value(&cx.global::<SettingsState>().store).ok(),
                before
            );
        });
    }

    #[gpui::test]
    fn closing_only_home_terminal_stays_empty_until_explicitly_opened(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let runtime = cx.new(Runtime::new);
            let first = runtime
                .update(cx, Runtime::spawn_home_terminal)
                .expect("shell should start");
            runtime.update(cx, |runtime, cx| runtime.close_home_terminal(0, cx));
            assert_eq!(runtime.read(cx).registry.read(cx).home_terminal_count(), 0);
            assert_eq!(runtime.read(cx).state.read(cx).active_terminal(), None);
            let reopened = runtime
                .update(cx, Runtime::spawn_home_terminal)
                .expect("explicitly opening a shell should still work");
            let registry = runtime.read(cx).registry.read(cx);
            assert_ne!(first, reopened);
            assert_eq!(registry.home_terminal_count(), 1);
            assert_eq!(registry.home_terminals()[0].id, reopened);
            assert_eq!(registry.home_terminals()[0].label, "terminal 2");
            assert_eq!(runtime.read(cx).state.read(cx).active_terminal(), Some(0));
        });
    }

    #[gpui::test]
    fn closing_nonfinal_home_terminal_does_not_spawn(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let runtime = cx.new(Runtime::new);
            let first = runtime
                .update(cx, Runtime::spawn_home_terminal)
                .expect("first shell should start");
            let second = runtime
                .update(cx, Runtime::spawn_home_terminal)
                .expect("second shell should start");
            runtime.update(cx, |runtime, cx| runtime.close_home_terminal(2, cx));
            assert_eq!(runtime.read(cx).registry.read(cx).home_terminal_count(), 2);
            runtime.update(cx, |runtime, cx| runtime.close_home_terminal(0, cx));
            let registry = runtime.read(cx).registry.read(cx);
            assert_eq!(registry.home_terminal_count(), 1);
            assert_eq!(registry.home_terminals()[0].id, second);
            assert_ne!(registry.home_terminals()[0].id, first);
            assert_eq!(registry.home_terminals()[0].label, "terminal 2");
        });
    }
    #[gpui::test]
    fn native_title_updates_existing_metadata_only_when_title_changes(
        cx: &mut gpui::TestAppContext,
    ) {
        let (handle, _actions) = grove_core::codex::Handle::channel();
        let (_updates, receive) = std::sync::mpsc::channel();
        let (runtime, session, id) = cx.update(|cx| {
            let runtime = init_script_runtime(cx, script_project("/native-title-fixture", "true"));
            let registry = runtime.read(cx).registry.clone();
            let id = registry.update(cx, |registry, _| {
                registry.insert_meta(
                    "script fixture".into(),
                    "/native-title-fixture".into(),
                    Agent::Codex,
                )
            });
            let saved = grove_core::codex::SavedSession {
                rich_plans: grove_core::rich_plan::PlanStore::default(),
                claude_resume: None,
                edit_checkpoints: Vec::new(),
                pending_edit: None,
                provider: grove_core::codex::Provider::Codex,
                composer: grove_core::codex::ComposerSettings::default(),
                context: None,
                key: "native-title-test".into(),
                project: "script fixture".into(),
                cwd: "/native-title-fixture".into(),
                label: "Codex".into(),
                title: Some("Recovered conversation".into()),
                title_generation: None,
                roots: Vec::new(),
                thread_id: Some("thread-title".into()),
                items: Vec::new(),
                work_periods: Vec::new(),
                draft: String::new(),
                draft_images: Vec::new(),
                draft_reference: None,
                config_args: Vec::new(),
                native_config_version: 1,
            };
            let connection = grove_core::codex::Connection {
                handle: handle.clone(),
                updates: receive.into(),
            };
            let session = cx.new(|cx| {
                crate::entities::codex_session::CodexSession::from_connection(
                    grove_core::codex::Snapshot::new(saved),
                    connection,
                    cx,
                )
            });
            registry.update(cx, |registry, _| {
                registry.attach_codex(id, session.clone(), handle);
            });
            runtime.update(cx, |runtime, cx| {
                runtime.observe_native_title(id, &session, cx);
            });
            (runtime, session, id)
        });
        cx.run_until_parked();
        let registry = runtime.read_with(cx, |runtime, _| runtime.registry.clone());
        let stable_label =
            registry.read_with(cx, |registry, _| registry.meta(id).unwrap().label.clone());
        assert_eq!(
            registry.read_with(cx, |registry, _| registry
                .meta(id)
                .unwrap()
                .restored_title
                .clone()),
            Some("Recovered conversation".into())
        );
        let notifications = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = notifications.clone();
        let _observer = cx.update(|cx| cx.observe(&registry, move |_, _| seen.set(seen.get() + 1)));
        session.update(cx, |session, cx| {
            session.state.saved.title = Some("Updated provider title".into());
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(
            registry.read_with(cx, |registry, _| registry
                .meta(id)
                .unwrap()
                .restored_title
                .clone()),
            Some("Updated provider title".into())
        );
        assert_eq!(notifications.get(), 1);
        session.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert_eq!(notifications.get(), 1);
        assert_eq!(
            registry.read_with(cx, |registry, _| registry.meta(id).unwrap().label.clone()),
            stable_label
        );
        assert_eq!(
            registry.read_with(cx, |registry, _| registry.meta(id).unwrap().id),
            id
        );
    }
}
