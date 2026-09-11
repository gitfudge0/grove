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
        self.state.update(cx, |s, cx| {
            s.set_open_agent_menu(None);
            cx.notify();
        });
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
                    use_tmux: true,
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

    pub fn spawn_home_terminal(&mut self, cx: &mut Context<Self>) {
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
        let count = self.registry.update(cx, |r, cx| {
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
                    spawned_at: Instant::now(),
                    attention: None,
                    tmux: false,
                    tmux_name: None,
                },
                session,
            );
            cx.notify();
            r.home_terminal_count()
        });
        self.state.update(cx, |s, cx| {
            s.select_home_terminal(count.saturating_sub(1), count);
            cx.notify();
        });
    }

    /// Always ≥1 home terminal: closing the last one immediately respawns a fresh shell.
    pub(crate) fn close_home_terminal(&mut self, i: usize, cx: &mut Context<Self>) {
        let remaining = self.registry.update(cx, |r, cx| {
            r.close_home(i);
            cx.notify();
            r.home_terminal_count()
        });
        self.state.update(cx, |s, cx| {
            s.close_home_terminal(i, remaining);
            cx.notify();
        });
        if remaining == 0 {
            self.spawn_home_terminal(cx);
        }
    }

    fn snapshot(&self, cx: &mut App) -> crate::entities::workspace_state::TreeSnapshot {
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

    /// Launch one agent with the same ordered multi-worktree context as the previous launcher.
    pub fn launch_multi_root_session(
        &mut self,
        worktree_paths: &[String],
        agent: Agent,
        cx: &mut Context<Self>,
    ) {
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
                return;
            }
        };
        let Some((primary_project, primary_cwd)) = targets.first().cloned() else {
            self.toast.clone().update(cx, |toast, cx| {
                toast.set_error("select one or more worktrees", cx);
            });
            return;
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
            return;
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
                    return;
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
            runtime.update(cx, Runtime::shutdown);
            assert_eq!(
                serde_json::to_value(&cx.global::<SettingsState>().store).ok(),
                before
            );
        });
    }
}
