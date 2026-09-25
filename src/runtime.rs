//! Session orchestration retained independently of the window during the UI rebuild.
//! Construction loads state; spawning and tmux attachment remain explicit operations.

use crate::{
    entities::{
        activity_store::ActivityStore,
        project_tree::ProjectTree,
        session_registry::{SessionId, SessionRegistry},
        terminal_session::TerminalSession,
        toast::ToastState,
        upgrade::Upgrade,
        workspace_state::{LiveTile, PtyPane, WorkspaceState},
    },
    settings::SettingsState,
};
use gpui::{prelude::*, App, Context, Entity};
use grove_core::agent::Agent;
use std::{collections::HashMap, time::Instant};

fn saved_tmux_preference_enabled(preference: Option<bool>) -> bool {
    preference == Some(true)
}

fn managed_session_uses_tmux(preference: Option<bool>) -> bool {
    !cfg!(test) && saved_tmux_preference_enabled(preference)
}

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

pub struct Runtime {
    pub state: Entity<WorkspaceState>,
    pub registry: Entity<SessionRegistry>,
    pub tree: Entity<ProjectTree>,
    pub activity: Entity<ActivityStore>,
    pub upgrade: Entity<Upgrade>,
    pub toast: Entity<ToastState>,
    pub projects: Entity<crate::project_service::ProjectService>,
    last_pty_dims: (u16, u16),
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
            cx.observe(&registry, |this, _, cx| {
                this.sync_grid_tiles(cx);
                cx.notify();
            }),
            cx.subscribe(&projects, Self::on_project_event),
        ];
        let dims = cx.global::<crate::zoom::CurrentPtyDims>();
        Self {
            state,
            registry,
            tree,
            activity,
            upgrade,
            toast,
            projects,
            last_pty_dims: (dims.rows, dims.cols),
            _observers: observers,
        }
    }

    fn on_project_event(
        &mut self,
        _: Entity<crate::project_service::ProjectService>,
        event: &crate::project_service::ProjectEvent,
        cx: &mut Context<Self>,
    ) {
        use crate::project_service::ProjectEvent;
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

    pub fn spawn_session(&mut self, proj: usize, wt: usize, agent: Agent, cx: &mut Context<Self>) {
        let snap = self.snapshot(cx);
        let Some(project) = snap.projects.iter().find(|p| p.idx == proj) else {
            return;
        };
        let Some(worktree) = project.worktrees.get(wt) else {
            return;
        };
        let (name, cwd) = (project.name.clone(), worktree.path.clone());
        self.spawn_session_in(name, cwd, agent, cx);
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
            agent.launch_args(
                store.dangerously_skip_permissions_enabled.unwrap_or(false),
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

    /// Returns the replacement id only when a valid final terminal was closed and its new shell started.
    pub(crate) fn close_home_terminal(
        &mut self,
        i: usize,
        cx: &mut Context<Self>,
    ) -> Option<SessionId> {
        let invalid_index = {
            let registry = self.registry.read(cx);
            i >= registry.home_terminal_count() || registry.home_terminal(i).is_none()
        };
        if invalid_index {
            return None;
        }
        let (remaining, needs_spawn) = self.registry.update(cx, |r, cx| {
            let closed = r.close_home(i).is_some();
            cx.notify();
            (
                r.home_terminal_count(),
                closed && r.home_terminals_need_spawn(),
            )
        });
        self.state.update(cx, |s, cx| {
            s.close_home_terminal(i, remaining);
            cx.notify();
        });
        needs_spawn.then(|| self.spawn_home_terminal(cx)).flatten()
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
        self.persist_grid_order(cx);
        SettingsState::flush_now(cx);
    }

    pub fn native_sessions_running(&self, cx: &mut App) -> usize {
        let ids: Vec<SessionId> = self.registry.read(cx).all().iter().map(|m| m.id).collect();
        ids.into_iter()
            .filter(|&id| {
                let Some(term) = self.registry.read(cx).session(id).cloned() else {
                    return false;
                };
                term.update(cx, |t, _| {
                    matches!(
                        t.backend(),
                        crate::entities::terminal_session::Backend::Native
                    ) && t.alive()
                })
            })
            .count()
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

    /// Backstop only — `TerminalElement::prepaint` is the path that gets the dims right (`TMUX_ATTACH_FALLBACK_FRAMES`).
    pub fn attach_pending_tmux_sessions(&mut self, cx: &mut Context<Self>) {
        let dims = self.last_pty_dims;
        let pending: Vec<_> = {
            let registry = self.registry.read(cx);
            registry
                .all()
                .iter()
                .filter_map(|meta| registry.session(meta.id))
                .filter(|session| session.read(cx).is_pending_attach())
                .cloned()
                .collect()
        };
        for session in pending {
            session.update(cx, |session, cx| {
                session.resize(dims.0, dims.1);
                session.attach_now(cx);
            });
        }
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
        let target = (|| -> Result<Option<(String, String, String)>, String> {
            let projects = &cx.global::<SettingsState>().store.projects;
            let project = projects
                .iter()
                .find(|project| project.path == project_path && !project.archived)
                .ok_or_else(|| "Project is no longer available.".to_string())?;
            let script = match project.scripts.run.as_deref() {
                Some(script) if !script.trim().is_empty() => script,
                _ => return Ok(None),
            };
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
            agent.multi_root_launch_args(
                store.dangerously_skip_permissions_enabled.unwrap_or(false),
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
            SettingsState::update(cx, {
                let project = primary_project.clone();
                let wt_path = primary_cwd.clone();
                move |store| {
                    store.recent_launches.retain(|recent| {
                        !(recent.project == project
                            && recent.wt_path == wt_path
                            && recent.agent == agent)
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
                }
            });
            self.toast
                .clone()
                .update(cx, |toast, cx| toast.set_toast("launched 1 session", cx));
        }
        did_launch
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
    fn managed_launch_requires_explicit_tmux_preference() {
        assert!(!saved_tmux_preference_enabled(None));
        assert!(!saved_tmux_preference_enabled(Some(false)));
        assert!(saved_tmux_preference_enabled(Some(true)));
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
    fn run_script_uses_worktree_cwd_and_selects_native_session(cx: &mut gpui::TestAppContext) {
        let repo = git_fixture();
        let canonical = repo.path().canonicalize().unwrap();
        let path = canonical.to_str().unwrap();
        let output = repo.path().join("script-cwd.txt");
        let script = format!("pwd > '{}'", output.display());
        let runtime = cx.update(|cx| {
            let runtime = init_script_runtime(cx, script_project(path, &script));
            assert!(runtime.update(cx, |runtime, cx| runtime.spawn_run_script(path, path, cx)));
            let registry = runtime.read(cx).registry.read(cx);
            let sessions = registry.all();
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].project, "script fixture");
            assert_eq!(sessions[0].wt_path, path);
            assert_eq!(sessions[0].agent, Agent::Terminal);
            assert!(!sessions[0].tmux);
            assert!(registry.wt_shells(path).is_empty());
            assert_eq!(
                runtime.read(cx).state.read(cx).active_session(),
                Some(sessions[0].id)
            );
            let session = registry.session(sessions[0].id).unwrap();
            assert_eq!(
                session.read(cx).backend(),
                &crate::entities::terminal_session::Backend::Native
            );
            runtime
        });
        let mut actual = None;
        for _ in 0..100 {
            if let Ok(contents) = fs_err::read_to_string(&output) {
                actual = Some(contents);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(actual.as_deref().map(str::trim), Some(path));
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
    fn repeated_run_scripts_have_distinct_closeable_session_ids(cx: &mut gpui::TestAppContext) {
        let repo = git_fixture();
        let canonical = repo.path().canonicalize().unwrap();
        let path = canonical.to_str().unwrap();
        cx.update(|cx| {
            let runtime = init_script_runtime(cx, script_project(path, "true"));
            assert!(runtime.update(cx, |runtime, cx| runtime.spawn_run_script(path, path, cx)));
            let first = runtime.read(cx).registry.read(cx).all()[0].id;
            assert!(runtime.update(cx, |runtime, cx| runtime.spawn_run_script(path, path, cx)));
            let registry = runtime.read(cx).registry.read(cx);
            assert_eq!(registry.len(), 2);
            let second = registry.all()[1].id;
            assert_ne!(first, second);
            assert!(registry.session(first).is_some());
            assert!(registry.session(second).is_some());
            assert!(registry.wt_shells(path).is_empty());
            assert_eq!(
                runtime.read(cx).state.read(cx).active_session(),
                Some(second)
            );
            runtime.update(cx, |runtime, cx| runtime.kill_session(first, cx));
            let registry = runtime.read(cx).registry.read(cx);
            assert_eq!(registry.len(), 1);
            assert_eq!(registry.all()[0].id, second);
            assert!(registry.session(first).is_none());
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
    fn closing_only_home_terminal_spawns_one_fresh_shell(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let runtime = cx.new(Runtime::new);
            let first = runtime
                .update(cx, Runtime::spawn_home_terminal)
                .expect("shell should start");
            let replacement = runtime
                .update(cx, |runtime, cx| runtime.close_home_terminal(0, cx))
                .expect("last close should replace the shell");
            let registry = runtime.read(cx).registry.read(cx);
            assert_ne!(first, replacement);
            assert_eq!(registry.home_terminal_count(), 1);
            assert_eq!(registry.home_terminals()[0].id, replacement);
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
            assert_eq!(
                runtime.update(cx, |runtime, cx| runtime.close_home_terminal(2, cx)),
                None
            );
            assert_eq!(runtime.read(cx).registry.read(cx).home_terminal_count(), 2);
            assert_eq!(
                runtime.update(cx, |runtime, cx| runtime.close_home_terminal(0, cx)),
                None
            );
            let registry = runtime.read(cx).registry.read(cx);
            assert_eq!(registry.home_terminal_count(), 1);
            assert_eq!(registry.home_terminals()[0].id, second);
            assert_ne!(registry.home_terminals()[0].id, first);
            assert_eq!(registry.home_terminals()[0].label, "terminal 2");
        });
    }
}
