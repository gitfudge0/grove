//! Workspace-scoped command palette. The fuzzy matcher and row identities live in `launcher`.
mod picker;

use super::status::{self, StatusKind};
use super::{motion, rpx, tokens::*};
use crate::{
    entities::session_registry::SessionId,
    icons::icon,
    launcher::{self, PaletteRow, PaletteScope, RowIdentity, WorktreeSelection},
    runtime::Runtime,
    settings::SettingsState,
    theme as c,
};
use gpui::{
    div, prelude::*, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    ScrollHandle, Subscription, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use grove_core::agent::Agent;
use std::collections::{HashMap, HashSet};

fn missing_agent_message(agent: Agent) -> String {
    format!(
        "{} CLI not found. Install it or refresh the shell environment in Settings, then retry. Check that its executable is on your shell PATH.",
        agent.label()
    )
}

/// A palette row has an appbar-height body plus the design system's 8px row breathing room.
const ROW_H: f32 = APPBAR_H + SPACE_LG;
const OVERLAY_TOP: f32 = APPBAR_TOTAL_H + SPACE_3XL * 2.0;
const PANEL_MAX_H: f32 = MODAL_SCROLL_MAX_H + APPBAR_H * 4.0;
/// Keeps the title, search, one row, and footer readable at the minimum 320×200 window.
const PANEL_MIN_H: f32 = APPBAR_H * 4.0 + SPACE_2XL * 2.0;

pub enum WorktreeLauncherEvent {
    Command(PaletteRow),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PaletteMode {
    #[default]
    Root,
    Single,
    Multi,
}

impl EventEmitter<WorktreeLauncherEvent> for WorktreeLauncher {}

pub struct WorktreeLauncher {
    runtime: Entity<Runtime>,
    sidebar: Entity<super::sidebar::Sidebar>,
    input: Entity<InputState>,
    _input_subscription: Subscription,
    focus: FocusHandle,
    back_focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    open: bool,
    closing: bool,
    presence: motion::Presence,
    close_task: Option<gpui::Task<()>>,
    mode: PaletteMode,
    selected_project: Option<usize>,
    query: String,
    selected: usize,
    anchor: Option<RowIdentity>,
    agent_selected: usize,
    agent_touched: bool,
    agent_focus: bool,
    selected_worktrees: WorktreeSelection,
    selection_paths: HashMap<String, String>,
    loading_worktrees: bool,
    load_seq: u64,
    /// Test-only capture after the real palette validation, before the live PTY dispatch.
    #[cfg(test)]
    launch_probe: Option<Vec<(Vec<String>, Agent)>>,
    error: Option<String>,
    scroll: ScrollHandle,
    source_bounds: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
}

impl WorktreeLauncher {
    pub fn new(
        runtime: Entity<Runtime>,
        sidebar: Entity<super::sidebar::Sidebar>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search projects and worktrees"));
        let subscription = cx.subscribe_in(&input, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                let query = this.input.read(cx).value().to_string();
                this.update_query(query, cx);
            }
        });
        Self {
            runtime,
            sidebar,
            input,
            _input_subscription: subscription,
            focus: cx.focus_handle(),
            back_focus: cx.focus_handle(),
            return_focus: None,
            open: false,
            closing: false,
            presence: motion::Presence::default(),
            close_task: None,
            mode: PaletteMode::Root,
            selected_project: None,
            query: String::new(),
            selected: 0,
            anchor: None,
            agent_selected: 0,
            agent_touched: false,
            agent_focus: false,
            selected_worktrees: WorktreeSelection::default(),
            selection_paths: HashMap::new(),
            loading_worktrees: false,
            load_seq: 0,
            #[cfg(test)]
            launch_probe: None,
            error: None,
            scroll: ScrollHandle::new(),
            source_bounds: std::rc::Rc::default(),
        }
    }

    pub fn launch_source_bounds(&self) -> Option<gpui::Bounds<gpui::Pixels>> {
        self.source_bounds.get()
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn is_visible(&self, cx: &App) -> bool {
        self.open || (self.closing && !cx.reduce_motion())
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            return;
        }
        self.return_focus = window.focused(cx);
        self.open = true;
        self.closing = false;
        self.close_task = None;
        self.presence.set(true, cx);
        self.mode = PaletteMode::Root;
        self.selected_project = None;
        self.loading_worktrees = false;
        self.query.clear();
        self.selected = 0;
        self.anchor = None;
        self.agent_selected = launcher::agent_sel_for(
            &Agent::ALL,
            preferred_agent(cx.global::<SettingsState>().store.default_agent),
        );
        self.agent_touched = false;
        self.agent_focus = false;
        self.selected_worktrees.clear();
        self.selection_paths.clear();
        self.error = None;
        self.scroll.set_offset(gpui::Point::default());
        self.input.update(cx, |input, cx| {
            input.set_placeholder("Search projects and worktrees", window, cx);
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        let rows = self.rows(cx);
        self.sync_agent_to_row(&rows);
        cx.notify();
    }

    fn title(&self) -> &'static str {
        match self.mode {
            PaletteMode::Root => "Command palette",
            PaletteMode::Single => "New session",
            PaletteMode::Multi => "New cross-project session",
        }
    }

    /// Open the existing multi-worktree selector for retained sidebar fixtures.
    #[cfg(test)]
    pub(crate) fn open_multi_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open(window, cx);
        let rows = self.rows(cx);
        if let Some(index) = rows
            .iter()
            .position(|row| *row == PaletteRow::NewMultiProjectSession)
        {
            self.selected = index;
            self.anchor = Some(launcher::row_identity(&rows[index]));
            self.activate(window, cx);
        }
    }

    /// Focus the inline tool selector for a worktree selected in the active workspace.
    pub fn open_for_worktree(
        &mut self,
        project: usize,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.open {
            return;
        }
        self.open(window, cx);
        let target_query = worktree_name(path).to_string();
        self.input
            .update(cx, |input, cx| input.set_value(&target_query, window, cx));
        self.update_query(target_query, cx);
        let Some((index, row)) = self.rows(cx).into_iter().enumerate().find(|(_, row)| {
            matches!(row, PaletteRow::Recent { proj, wt_path, .. } | PaletteRow::Combo { proj, wt_path, .. }
                if *proj == project && wt_path == path)
        }) else {
            self.error = Some("Selected worktree is no longer in this workspace.".into());
            cx.notify();
            return;
        };
        self.selected = index;
        self.anchor = Some(launcher::row_identity(&row));
        if let PaletteRow::Recent { agent, .. } | PaletteRow::Combo { agent, .. } = row {
            self.agent_selected = launcher::agent_sel_for(&Agent::ALL, agent);
        }
        self.agent_focus = true;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.presence.set(false, cx);
        self.closing = !cx.reduce_motion();
        self.close_task = self.closing.then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(motion::Presence::duration())
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.closing = false;
                    cx.notify();
                });
            })
        });
        self.load_seq = self.load_seq.wrapping_add(1);
        if let Some(focus) = self.return_focus.take() {
            focus.focus(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn close_after_deferred_launch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.open {
            self.close(window, cx);
        }
    }

    /// Fill cold workspace caches off the UI thread; the picker can accept search text while loading.
    fn load_workspace_worktrees(&mut self, cx: &mut Context<Self>) {
        let store = &cx.global::<SettingsState>().store;
        let workspace = store.workspaces.active;
        let projects: Vec<_> = store
            .workspace_projects(workspace)
            .map(|(idx, project)| (idx, project.path.clone()))
            .collect();
        let active_proj = self.runtime.read(cx).state.read(cx).proj_idx();
        let tree = self.runtime.read(cx).tree.clone();
        let (generation, targets) = {
            let tree = tree.read(cx);
            let targets = projects
                .iter()
                .filter(|(proj, _)| {
                    (self.mode != PaletteMode::Single || self.selected_project == Some(*proj))
                        && tree.worktrees_for_project(*proj, active_proj).is_empty()
                })
                .cloned()
                .collect::<Vec<_>>();
            (tree.generation(), targets)
        };
        self.load_seq = self.load_seq.wrapping_add(1);
        let request = self.load_seq;
        self.loading_worktrees = !targets.is_empty();
        if targets.is_empty() {
            return;
        }
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            let swept = cx
                .background_executor()
                .spawn(async move {
                    let paths = targets
                        .iter()
                        .map(|(_, path)| path.clone())
                        .collect::<Vec<_>>();
                    targets
                        .into_iter()
                        .map(|(proj, _)| proj)
                        .zip(grove_core::git::list_worktrees_many(&paths))
                        .collect::<HashMap<_, _>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this.open || this.mode == PaletteMode::Root || this.load_seq != request {
                    return;
                }
                let store = &cx.global::<SettingsState>().store;
                let current_projects = store
                    .workspace_projects(store.workspaces.active)
                    .map(|(idx, project)| (idx, project.path.clone()))
                    .collect::<Vec<_>>();
                let current_active = this.runtime.read(cx).state.read(cx).proj_idx();
                if store.workspaces.active != workspace
                    || current_projects != projects
                    || current_active != active_proj
                {
                    this.load_workspace_worktrees(cx);
                    cx.notify();
                    return;
                }
                let tree = this.runtime.read(cx).tree.clone();
                let active_worktrees = swept.get(&active_proj).cloned();
                let applied = tree.update(cx, |tree, cx| {
                    if !tree.apply_sweep(generation, swept) {
                        return false;
                    }
                    if tree
                        .worktrees_for_project(active_proj, active_proj)
                        .is_empty()
                    {
                        if let Some(worktrees) = active_worktrees {
                            tree.set_active_worktrees(active_proj, worktrees);
                        }
                    }
                    cx.notify();
                    true
                });
                if !applied {
                    this.load_workspace_worktrees(cx);
                    cx.notify();
                    return;
                }
                this.loading_worktrees = false;
                let rows = this.rows(cx);
                this.selected =
                    launcher::resolve_row_by_identity(&rows, this.anchor.as_ref(), this.selected)
                        .unwrap_or(0);
                this.anchor = rows.get(this.selected).map(launcher::row_identity);
                cx.notify();
            });
        })
        .detach();
    }

    fn launch(
        &mut self,
        identity: RowIdentity,
        agent: Agent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let RowIdentity::Session { proj, wt_path, .. } = identity else {
            return;
        };
        let store = &cx.global::<SettingsState>().store;
        let project = store
            .workspace_projects(store.workspaces.active)
            .find(|(i, _)| *i == proj);
        let Some((_, _)) = project else {
            self.error = Some("Project is no longer in this workspace.".into());
            cx.notify();
            return;
        };
        if !agent.available() {
            self.show_launch_error(missing_agent_message(agent), cx);
            return;
        }
        if !self.valid_cached_worktree(proj, &wt_path, cx) {
            self.error = Some("Worktree is no longer available.".into());
            cx.notify();
            return;
        }
        self.launch_roots(vec![wt_path], agent, window, cx);
    }

    fn launch_roots(
        &mut self,
        paths: Vec<String>,
        agent: Agent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            self.error = Some("Select one or more worktrees.".into());
            cx.notify();
            return;
        }
        if !agent.available() {
            self.show_launch_error(missing_agent_message(agent), cx);
            return;
        }
        let store = &cx.global::<SettingsState>().store;
        let projects: Vec<_> = store
            .workspace_projects(store.workspaces.active)
            .map(|(index, _)| index)
            .collect();
        if paths.iter().any(|path| {
            !projects
                .iter()
                .any(|project| self.valid_cached_worktree(*project, path, cx))
        }) {
            self.error =
                Some("A selected worktree is no longer available in this workspace.".into());
            cx.notify();
            return;
        }
        #[cfg(test)]
        if let Some(requests) = &mut self.launch_probe {
            requests.push((paths, agent));
            self.close(window, cx);
            return;
        }
        let active_before = self.runtime.read(cx).state.read(cx).active_session();
        let did_launch = self.runtime.update(cx, |runtime, cx| {
            runtime.launch_multi_root_session(&paths, agent, cx)
        });
        self.finish_launch(did_launch, active_before, window, cx);
    }

    fn show_launch_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error.clone());
        self.runtime.read(cx).toast.clone().update(cx, |toast, cx| {
            toast.set_error(error, cx);
        });
        cx.notify();
    }

    /// A failed PTY still leaves a selected session in the registry so the sidebar can retry it.
    fn finish_launch(
        &mut self,
        did_launch: bool,
        active_before: Option<SessionId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.runtime.read(cx).has_pending_managed_launch() {
            return;
        }
        let active_after = self.runtime.read(cx).state.read(cx).active_session();
        if did_launch || active_after.is_some_and(|id| Some(id) != active_before) {
            self.close(window, cx);
            self.sidebar.update(cx, |sidebar, cx| {
                sidebar.focus_active_session_after_palette(window, cx);
            });
        } else {
            let message = self
                .runtime
                .read(cx)
                .toast
                .read(cx)
                .current()
                .filter(|toast| toast.kind == crate::entities::toast::ToastKind::Error)
                .map_or_else(
                    || "Could not start this session. Select a current checkout and retry.".into(),
                    |toast| toast.message.clone(),
                );
            self.show_launch_error(message, cx);
        }
    }

    fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        if self.mode != PaletteMode::Root && self.loading_worktrees {
            return;
        }
        let Some(agent) = Agent::ALL.get(self.agent_selected).copied() else {
            return;
        };
        if self.mode == PaletteMode::Multi {
            self.launch_roots(
                self.selected_worktrees.selected_targets(),
                agent,
                window,
                cx,
            );
            return;
        }
        let row = self.selected_row(cx);
        if self.mode == PaletteMode::Single && self.selected_project.is_none() {
            if let Some(PaletteRow::Combo { proj, wt_path, .. }) = row {
                let valid = cx
                    .global::<SettingsState>()
                    .store
                    .workspace_projects(cx.global::<SettingsState>().store.workspaces.active)
                    .any(|(idx, project)| idx == proj && project.path == wt_path);
                if !valid {
                    self.error = Some("Project is no longer in this workspace.".into());
                    cx.notify();
                    return;
                }
                self.close(window, cx);
                self.sidebar.update(cx, |sidebar, cx| {
                    sidebar.open_dashboard_for_project(proj, &wt_path, window, cx);
                });
            }
            return;
        }
        let Some(row) = row else {
            return;
        };
        match row {
            PaletteRow::Recent { .. } | PaletteRow::Combo { .. } => {
                self.launch(launcher::row_identity(&row), agent, window, cx);
            }
            PaletteRow::NewSession | PaletteRow::NewMultiProjectSession => {
                if self.combos(cx).is_empty() {
                    self.close(window, cx);
                    cx.emit(WorktreeLauncherEvent::Command(PaletteRow::AddProject));
                } else {
                    self.mode = if row == PaletteRow::NewSession {
                        PaletteMode::Single
                    } else {
                        PaletteMode::Multi
                    };
                    self.selected_project = None;
                    if self.mode == PaletteMode::Multi {
                        self.load_workspace_worktrees(cx);
                    }
                    self.selected_worktrees.clear();
                    self.query.clear();
                    self.selected = 0;
                    self.anchor = self.rows(cx).first().map(launcher::row_identity);
                    self.agent_focus = false;
                    self.error = None;
                    self.scroll.set_offset(gpui::Point::default());
                    self.reset_picker_search(window, cx);
                }
            }
            command => {
                self.close(window, cx);
                cx.emit(WorktreeLauncherEvent::Command(command));
            }
        }
    }

    fn reset_picker_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.query.clear();
        self.selected = 0;
        self.anchor = None;
        self.agent_focus = false;
        self.error = None;
        self.scroll.set_offset(gpui::Point::default());
        let placeholder = if self.mode == PaletteMode::Single && self.selected_project.is_none() {
            "Search projects"
        } else if self.mode == PaletteMode::Root {
            "Search projects and worktrees"
        } else {
            "Search worktrees"
        };
        self.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder, window, cx);
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == PaletteMode::Root {
            self.close(window, cx);
            return;
        }
        if self.mode == PaletteMode::Single && self.selected_project.is_some() {
            self.selected_project = None;
        } else {
            self.mode = PaletteMode::Root;
        }
        self.loading_worktrees = false;
        self.load_seq = self.load_seq.wrapping_add(1);
        self.selected_worktrees.clear();
        self.reset_picker_search(window, cx);
        let rows = self.rows(cx);
        self.sync_agent_to_row(&rows);
    }

    fn toggle_selected_row(&mut self, cx: &mut Context<Self>) {
        if self.mode != PaletteMode::Multi || self.loading_worktrees {
            return;
        }
        let Some(row) = self.selected_row(cx) else {
            self.error = Some("Select a worktree to add it.".into());
            cx.notify();
            return;
        };
        let RowIdentity::Session { wt_path, .. } = launcher::row_identity(&row) else {
            return;
        };
        let Ok(path) = fs_err::canonicalize(&wt_path) else {
            self.error = Some("Worktree is no longer available.".into());
            cx.notify();
            return;
        };
        let store = &cx.global::<SettingsState>().store;
        let valid = store
            .workspace_projects(store.workspaces.active)
            .any(|(index, _)| self.valid_cached_worktree(index, &wt_path, cx));
        if !valid {
            self.error = Some("Worktree is no longer available in this workspace.".into());
            cx.notify();
            return;
        }
        let canonical = path.to_string_lossy().into_owned();
        self.selected_worktrees.toggle(&canonical);
        self.selection_paths.insert(wt_path, canonical);
        self.error = None;
        cx.notify();
    }

    fn agent_selector(&self, cx: &mut Context<Self>, id_prefix: &'static str) -> impl IntoElement {
        div()
            .id(id_prefix)
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_XS))
            .children(
                Agent::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(agent_index, agent)| {
                        let selected = self.agent_selected == agent_index;
                        let available = agent.available();
                        let label = if available {
                            agent.label().to_string()
                        } else {
                            format!("{} (not installed)", agent.label())
                        };
                        let icon_name = match agent {
                            Agent::Claude => "claude",
                            Agent::Codex => "codex",
                            Agent::OpenCode => "opencode",
                            Agent::Terminal => "terminal",
                        };
                        let button_id = format!("{id_prefix}-{agent_index}");
                        div()
                            .id(gpui::SharedString::from(button_id.clone()))
                            .debug_selector(move || button_id.clone())
                            .role(gpui::Role::Button)
                            .cursor_pointer()
                            .aria_label(label.clone())
                            .size(rpx(CHROME_CONTROL_H))
                            .rounded(rpx(RADIUS_CONTROL))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(selected, |button| button.bg(c::BG_HL()).text_color(c::FG()))
                            .when(selected, |button| {
                                button.border_1().border_color(c::BORDER_STRONG())
                            })
                            .hover(|button| button.bg(c::BG_HOVER()).text_color(c::FG()))
                            .tooltip(move |window, cx| {
                                crate::views::components::tooltip(label.clone(), window)
                                    .build(window, cx)
                            })
                            .child(icon(
                                icon_name,
                                ICON_MD,
                                if available {
                                    c::MAGENTA()
                                } else {
                                    c::FG_MUTE()
                                },
                            ))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                if available {
                                    this.agent_selected = agent_index;
                                    this.agent_touched = true;
                                    this.agent_focus = true;
                                    this.error = None;
                                    this.focus.focus(window, cx);
                                } else {
                                    this.show_launch_error(missing_agent_message(agent), cx);
                                }
                                cx.notify();
                            }))
                    }),
            )
    }

    fn key(&mut self, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        let key = &event.keystroke;
        if self.back_focus.is_focused(window) && matches!(key.key.as_str(), "enter" | "space") {
            if !event.is_held {
                self.back(window, cx);
            }
            window.prevent_default();
            cx.stop_propagation();
            return;
        }
        match key.key.as_str() {
            "escape" => self.back(window, cx),
            "down" | "up" => {
                self.selected = launcher::cycle(
                    self.selected,
                    if key.key == "down" { 1 } else { -1 },
                    self.rows(cx).len(),
                );
                self.anchor = self.rows(cx).get(self.selected).map(launcher::row_identity);
                let rows = self.rows(cx);
                self.sync_agent_to_row(&rows);
                self.scroll.scroll_to_item(self.selected);
                cx.notify();
            }
            "tab" if !(self.mode == PaletteMode::Single && self.selected_project.is_none()) => {
                let has_back = self.mode == PaletteMode::Single && self.selected_project.is_some();
                let back_focused = self.back_focus.is_focused(window);
                let search_focused = self.input.read(cx).focus_handle(cx).is_focused(window);
                if has_back
                    && ((key.modifiers.shift && search_focused)
                        || (!key.modifiers.shift && self.agent_focus))
                {
                    self.agent_focus = false;
                    self.back_focus.focus(window, cx);
                } else if back_focused && !key.modifiers.shift || !has_back && self.agent_focus {
                    self.agent_focus = false;
                    self.input.update(cx, |input, cx| input.focus(window, cx));
                } else {
                    self.agent_focus = true;
                    self.focus.focus(window, cx);
                }
                cx.notify();
            }
            "left" | "right" if self.agent_focus => {
                let available = available_agents();
                let current = Agent::ALL
                    .get(self.agent_selected)
                    .copied()
                    .unwrap_or(Agent::Terminal);
                let index = launcher::agent_sel_for(&available, current);
                if let Some(agent) = available.get(launcher::cycle(
                    index,
                    if key.key == "right" { 1 } else { -1 },
                    available.len(),
                )) {
                    self.agent_selected = launcher::agent_sel_for(&Agent::ALL, *agent);
                    self.agent_touched = true;
                }
                cx.notify();
            }
            "space" => {
                if self.mode != PaletteMode::Multi || !key.modifiers.shift {
                    return;
                }
                let search_focused = self.input.read(cx).focus_handle(cx).is_focused(window);
                self.toggle_selected_row(cx);
                if search_focused {
                    self.input.update(cx, |input, cx| input.focus(window, cx));
                }
            }
            "enter" => self.activate(window, cx),
            _ => return,
        }
        window.prevent_default();
        cx.stop_propagation();
    }
}

impl Focusable for WorktreeLauncher {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for WorktreeLauncher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let project_names = cx
            .global::<SettingsState>()
            .store
            .projects
            .iter()
            .map(|project| project.name.clone())
            .collect::<Vec<_>>();
        let rows = self.rows(cx);
        let project_step = self.mode == PaletteMode::Single && self.selected_project.is_none();
        let selected_project_name = self
            .selected_project
            .and_then(|idx| project_names.get(idx))
            .cloned();
        let selected_count = self.selected_worktrees.count();
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let viewport_w = f32::from(window.viewport_size().width) / scale;
        let viewport_h = f32::from(window.viewport_size().height) / scale;
        let panel_w = MODAL_W_LG.min((viewport_w - SPACE_LG * 2.0).max(0.0));
        let narrow = panel_w < 440.0;
        let top = OVERLAY_TOP.min((viewport_h - PANEL_MIN_H - SPACE_LG).max(SPACE_LG));
        let panel_h = PANEL_MAX_H.min((viewport_h - top - SPACE_LG).max(0.0));
        let compact = panel_h < PANEL_MIN_H + APPBAR_H;
        let overlay = div()
            .id("worktree-launcher-overlay")
            .debug_selector(|| "worktree-launcher-overlay".into())
            .absolute()
            .inset_0()
            .when(self.open, gpui::InteractiveElement::occlude)
            .bg(c::SCRIM())
            .flex()
            .items_start()
            .justify_center()
            .pt(rpx(top))
            .capture_key_down(cx.listener(Self::key))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close(window, cx)))
            .child(
                div()
                    .id("worktree-launcher-panel")
                    .debug_selector(|| "worktree-launcher-panel".into())
                    .relative()
                    .child({ let bounds = self.source_bounds.clone(); gpui::canvas(move |rect, _, _| bounds.set(Some(rect)), |_, (), _, _| {}).absolute().size_full() })
                    .when(self.open, |panel| panel.track_focus(&self.focus))
                    .w(rpx(panel_w))
                    .max_h(rpx(panel_h))
                    .flex()
                    .flex_col()
                    .rounded(rpx(RADIUS_PANEL))
                    .border_1()
                    .border_color(c::BORDER())
                    .bg(c::SURFACE_RAISED())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px(rpx(SPACE_3XL))
                            .pt(rpx(if compact { 0.0 } else { SPACE_3XL }))
                            .pb(rpx(if compact { SPACE_XS } else { SPACE_2XL }))
                            .flex()
                            .flex_col()
                            .gap(rpx(if compact { SPACE_SM } else { SPACE_2XL }))
                            .when(!compact, |header| header.child(div().flex().items_center().gap(rpx(SPACE_SM))
                                .when(selected_project_name.is_some(), |header| header.child(
                                    div().id("launcher-back").role(gpui::Role::Button).aria_label("Back to projects").track_focus(&self.back_focus).tab_index(0).focus_visible(|button| button.border_1().border_color(c::FG()))
                                        .cursor_pointer().on_click(cx.listener(|this, _, window, cx| this.back(window, cx)))
                                        .child(icon("arrow-left", ICON_MD, c::FG_DIM()))))
                                .child(div().text_size(rpx(TEXT_TITLE)).text_color(c::FG()).child(selected_project_name.clone().unwrap_or_else(|| self.title().to_string())))))
                            .child(
                                div()
                                    .id("worktree-launcher-search")
                                    .debug_selector(|| "worktree-launcher-search".into())
                                    .h(rpx(if compact {
                                        ICON_BTN_W
                                    } else {
                                        APPBAR_H
                                    }))
                                    .px(rpx(SPACE_2XL))
                                    .rounded(rpx(RADIUS_PANEL))
                                    .bg(if self.input.read(cx).focus_handle(cx).is_focused(window) { c::BG_HOVER() } else { c::FIELD_FILL() })
                                    .flex()
                                    .items_center()
                                    .when(compact && selected_project_name.is_some(), |search| search.child(
                                        div().id("launcher-back-compact").role(gpui::Role::Button).aria_label("Back to projects").track_focus(&self.back_focus).tab_index(0).focus_visible(|button| button.border_1().border_color(c::FG()))
                                            .cursor_pointer().on_click(cx.listener(|this, _, window, cx| this.back(window, cx)))
                                            .child(icon("arrow-left", ICON_MD, c::FG_DIM()))))
                                    .child(
                                        Input::new(&self.input).file_drop_overlay(crate::views::components::file_drop_overlay()).disabled(!self.open).tab_index(if self.open { 0 } else { -1 })
                                            .aria_label(if project_step { "Search projects" } else if self.mode == PaletteMode::Single { "Search worktrees" } else { "Search commands and worktrees" })
                                            .appearance(false)
                                            .bordered(false)
                                            .focus_bordered(false)
                                            .text_size(rpx(TEXT_BODY))
                                            .text_color(c::FG())
                                            .p_0(),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("worktree-launcher-list")
                            .debug_selector(|| "worktree-launcher-list".into())
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .px(rpx(SPACE_LG))
                            .when(!compact, |list| list.pb(rpx(SPACE_LG)))
                            .when(rows.is_empty(), |list| list.child(
                                div().p(rpx(SPACE_3XL)).text_color(c::FG_DIM()).text_size(rpx(TEXT_BODY)).child(
                                    if self.mode != PaletteMode::Root && self.loading_worktrees {
                                        status::label("launcher-loading-status", StatusKind::Working, "Loading worktrees…", ICON_SM, cx).into_any_element()
                                    } else { div().child(if self.mode == PaletteMode::Root { "No matching commands" } else if project_step { "No matching projects" } else { "No matching worktrees" }).into_any_element() }
                                )
                            ))
                            .children(rows.into_iter().enumerate().map(|(index, row)| {
                                let (label, detail, icon_name, suffix) = match &row {
                                    PaletteRow::Recent { proj, wt_path, agent } | PaletteRow::Combo { proj, wt_path, agent } => {
                                        let project = project_names.get(*proj).map_or("", |name| name.as_str());
                                        (if project_step { project.to_string() } else if self.mode == PaletteMode::Single { worktree_name(wt_path).to_string() } else { format!("{} / {}", project, worktree_name(wt_path)) }, wt_path.clone(), "git-branch", if project_step { String::new() } else { agent.label().to_string() })
                                    }
                                    PaletteRow::NewSession => ("New session".into(), "Choose a worktree".into(), "plus", String::new()),
                                    PaletteRow::NewMultiProjectSession => ("New cross-project session".into(), "Select multiple worktrees".into(), "plus", String::new()),
                                    PaletteRow::TerminalHome => ("New home terminal".into(), "Open a terminal in the workspace".into(), "terminal", String::new()),
                                    PaletteRow::TerminalWt => ("New worktree terminal".into(), "Open a terminal in the selected worktree".into(), "terminal", String::new()),
                                    PaletteRow::AddProject => ("Add project".into(), "Add a local project to this workspace".into(), "plus", String::new()),
                                    PaletteRow::RunScript => ("Run script".into(), "Run the selected worktree script".into(), "play", String::new()),
                                    PaletteRow::ViewDiff => ("View diff".into(), "Open the selected session diff".into(), "diff", String::new()),
                                    PaletteRow::SwitchToSession => ("Switch workspace or session".into(), "Choose a workspace or a session in this workspace".into(), "list", String::new()),
                                    PaletteRow::Settings => ("Settings".into(), "App preferences".into(), "cog", String::new()),
                                    PaletteRow::Setting(setting) => (setting.label().into(), setting.section().into(), setting.icon_name(), String::new()),
                                };
                                let symbol = match &row {
                                    PaletteRow::Recent { proj, .. } | PaletteRow::Combo { proj, .. } => cx.global::<SettingsState>().store.projects.get(*proj).map(|p| cx.global::<SettingsState>().store.project_appearance(&p.path).current()),
                                    _ => None,
                                };
                                let identity = launcher::row_identity(&row);
                                let show_agent_selector = !project_step && self.mode != PaletteMode::Multi
                                    && self.agent_focus
                                    && index == self.selected;
                                let is_worktree_row = matches!(
                                    &row,
                                    PaletteRow::Recent { .. } | PaletteRow::Combo { .. }
                                );
                                let checked = self.mode == PaletteMode::Multi && match &row {
                                    PaletteRow::Recent { wt_path, .. } | PaletteRow::Combo { wt_path, .. } => self.selected_worktrees.contains(wt_path)
                                        || self.selection_paths.get(wt_path).is_some_and(|path| self.selected_worktrees.contains(path)),
                                    _ => false,
                                };
                                div()
                                    .id(gpui::SharedString::from(format!("launcher-row-{index}")))
                                    .debug_selector(move || format!("launcher-row-{index}"))
                                    .role(gpui::Role::Button)
                                    .cursor_pointer()
                                    .aria_label(if self.mode == PaletteMode::Multi && is_worktree_row {
                                        format!("{label}, {}", if checked { "selected" } else { "not selected" })
                                    } else {
                                        label.clone()
                                    })
                                    .h(rpx(ROW_H))
                                    .px(rpx(SPACE_2XL))
                                    .rounded(rpx(RADIUS_GROUP))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(rpx(SPACE_LG))
                                    .text_color(c::FG_DIM())
                                    .when(index == self.selected, |row| row.bg(c::BG_HL()).text_color(c::FG()))
                                    .hover(|row| row.bg(c::BG_HOVER()).text_color(c::FG()))
                                    .child(
                                        div().flex().flex_1().items_start().gap(rpx(SPACE_2XL)).min_w_0()
                                            .child(
                                                div()
                                                    .size(rpx(if project_step { 28.0 } else { CONTROL_H }))
                                                    .flex_shrink_0()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .when(project_step, |glyph| glyph.when_some(symbol, |glyph, symbol| glyph.child(super::components::project_symbol(symbol, 28.0))))
                                                    .when(!project_step, |glyph| glyph.child(icon(icon_name, ICON_20, c::FG_DIM()))),
                                            )
                                            .child(div().flex().flex_1().flex_col().min_w_0().gap(rpx(SPACE_XS))
                                                .child(div().line_height(rpx(SPACE_3XL)).text_size(rpx(TEXT_BODY)).text_color(c::FG()).truncate().child(label))
                                                .child(div().line_height(rpx(SPACE_3XL)).text_size(rpx(TEXT_SMALL)).text_color(c::FG_MUTE()).truncate().child(detail)))
                                    )
                                    .when(project_step && is_worktree_row, |row| row.child(
                                        div().flex_shrink_0().child(icon("chev-right", ICON_20, c::FG_DIM()))
                                    ))
                                    .when(show_agent_selector, |row| row.child(
                                        self.agent_selector(cx, "launcher-agent-selector")
                                    ))
                                    .when(!(show_agent_selector || suffix.is_empty() || self.mode == PaletteMode::Multi && is_worktree_row), |row| row.child(
                                        div().flex_shrink_0().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM())
                                            .child(suffix)
                                    ))
                                    .when(self.mode == PaletteMode::Multi && is_worktree_row, |row| row.child(
                                        div()
                                            .debug_selector(move || format!("launcher-row-{index}-selection"))
                                            .size(rpx(CONTROL_H * 0.72))
                                            .flex_shrink_0()
                                            .rounded(rpx(RADIUS_CONTROL))
                                            .border_1()
                                            .border_color(if checked { c::SEL_RING() } else { c::BORDER_STRONG() })
                                            .bg(if checked { c::SEL_RING() } else { c::FIELD_FILL() })
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(checked, |checkbox| checkbox.child(icon("check", ICON_SM, c::FG())))
                                    ))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let rows = this.rows(cx);
                                        if let Some(current) = launcher::resolve_row_by_identity(&rows, Some(&identity), index) {
                                            this.selected = current;
                                            this.anchor = Some(identity.clone());
                                            if this.mode == PaletteMode::Multi && matches!(identity, RowIdentity::Session { .. }) {
                                                this.toggle_selected_row(cx);
                                            } else {
                                                this.activate(window, cx);
                                            }
                                        }
                                    }))
                            })),
                    )
                    .when_some(self.error.clone().filter(|_| !compact), |panel, error| panel.child(
                        motion::fast(
                            div().px(rpx(SPACE_3XL)).pb(rpx(SPACE_LG)).text_size(rpx(TEXT_SMALL)).child(status::label("launcher-error-status", StatusKind::Failed, error.clone(), ICON_SM, cx)),
                            format!("worktree-launcher-error-{error}"),
                            cx,
                        )
                    ))
                    .when(self.mode == PaletteMode::Multi && !self.loading_worktrees, |panel| panel.child(
                        {
                            let count_text = if narrow {
                                format!("{selected_count} selected")
                            } else {
                                format!("{selected_count} worktrees selected")
                            };
                            let count = div().debug_selector(|| "worktree-launcher-selected-count".into())
                                .min_w_0().text_size(rpx(TEXT_SMALL)).text_color(c::FG())
                                .child(count_text);
                            let selector = div()
                                .child(self.agent_selector(cx, "launcher-multi-agent-selector"));
                            let launch = div()
                                .id("worktree-launcher-launch-selected")
                                .debug_selector(|| "worktree-launcher-launch-selected".into())
                                .role(gpui::Role::Button)
                                .when(selected_count > 0, gpui::Styled::cursor_pointer)
                                .when(selected_count == 0, gpui::Styled::cursor_default)
                                .aria_label(format!("Launch {selected_count} selected worktrees"))
                                .tab_index(-1)
                                .px(rpx(SPACE_2XL))
                                .h(rpx(if compact { CONTROL_H } else { APPBAR_H }))
                                .rounded(rpx(RADIUS_CONTROL))
                                .bg(c::BG_HL())
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG())
                                .when(selected_count == 0, |button| button.opacity(OPACITY_DISABLED))
                                .when(selected_count > 0, |button| button.hover(|button| button.bg(c::BG_HOVER()).text_color(c::FG())))
                                .focus_visible(|button| button.border_1().border_color(c::FG()))
                                .child(div().w(rpx(ICON_SM)).flex_shrink_0().child(icon("play", ICON_SM, c::FG())))
                                .child("Launch selected")
                                .on_click(cx.listener(|this, _, window, cx| this.activate(window, cx)));
                            if narrow {
                                div()
                                    .px(rpx(SPACE_3XL))
                                    .pb(rpx(if compact { SPACE_SM } else { SPACE_LG }))
                                    .flex()
                                    .flex_col()
                                    .gap(rpx(if compact { SPACE_XS } else { SPACE_SM }))
                                    .child(div().flex().justify_end().child(selector))
                                    .child(div().flex().items_center().justify_between().gap(rpx(SPACE_LG)).child(count).child(launch))
                            } else {
                                div()
                                    .px(rpx(SPACE_3XL))
                                    .pb(rpx(if compact { SPACE_SM } else { SPACE_LG }))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(rpx(SPACE_LG))
                                    .child(count)
                                    .child(div().flex().items_center().gap(rpx(SPACE_LG)).child(selector).child(launch))
                            }
                        }
                    ))
                    .child(
                        div()
                            .id("worktree-launcher-footer")
                            .debug_selector(|| "worktree-launcher-footer".into())
                            .border_t_1()
                            .border_color(c::BORDER_SOFT())
                            .px(rpx(SPACE_3XL))
                            .py(rpx(if compact { SPACE_XS } else { SPACE_LG }))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_MUTE())
                            .when_some(self.error.clone().filter(|_| compact), |footer, error| footer.child(
                                motion::fast(div()
                                    .debug_selector(|| "worktree-launcher-compact-error".into())
                                    .child(status::label("launcher-compact-error-status", StatusKind::Failed, error.clone(), ICON_SM, cx)),
                                    format!("worktree-launcher-compact-error-{error}"),
                                    cx,
                                )
                            ))
                            .when(!compact || self.error.is_none(), |footer| footer.child(super::components::shortcut_hint(match self.mode {
                                PaletteMode::Root if compact => "↑↓ · Tab tools · Enter · Esc",
                                _ if self.loading_worktrees && compact => "Loading worktrees… · Esc back",
                                _ if self.loading_worktrees => "Loading worktrees… · Search is ready · Esc back",
                                PaletteMode::Single if project_step => "↑↓ projects · Enter choose · Esc back",
                                PaletteMode::Single if compact => "↑↓ · Tab tools · Enter start · Esc",
                                PaletteMode::Multi if compact => "Shift+Space select · Tab tools · Enter · Esc",
                                PaletteMode::Root => "↑↓ rows · Tab tools · Enter activate · Esc close",
                                PaletteMode::Single => "↑↓ worktrees · Tab tools · Enter start session · Esc back",
                                PaletteMode::Multi => "↑↓ worktrees · Shift+Space select · Tab tools · ←→ change tool · Enter launch selected · Esc back",
                            }))),
                    ),
            );
        self.presence.render(
            overlay.when(!self.open, |overlay| {
                overlay.child(
                    div()
                        .absolute()
                        .inset_0()
                        .occlude()
                        .capture_any_mouse_down(|_, _, cx| cx.stop_propagation())
                        .capture_any_mouse_up(|_, _, cx| cx.stop_propagation()),
                )
            }),
            "worktree-launcher-presence",
            cx,
        )
    }
}

fn available_agents() -> Vec<Agent> {
    Agent::ALL
        .into_iter()
        .filter(|agent| agent.available())
        .collect()
}

fn preferred_agent(configured: Option<Agent>) -> Agent {
    configured
        .filter(|agent| agent.available())
        .or_else(|| available_agents().into_iter().next())
        .unwrap_or(Agent::Terminal)
}

fn worktree_name(path: &str) -> &str {
    std::path::Path::new(path)
        .file_name()
        .and_then(|part| part.to_str())
        .unwrap_or(path)
}

#[cfg(test)]
mod tests;
