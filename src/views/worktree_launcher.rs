//! Workspace-scoped command palette. The fuzzy matcher and row identities live in `launcher`.
use super::{rpx, tokens::*};
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
use std::collections::HashSet;

/// A palette row has an appbar-height body plus the design system's 8px row breathing room.
const ROW_H: f32 = APPBAR_H + SPACE_LG;
const OVERLAY_TOP: f32 = APPBAR_H + SPACE_3XL * 2.0;
const PANEL_MAX_H: f32 = MODAL_SCROLL_MAX_H + APPBAR_H * 4.0;
/// Keeps the title, search, one row, and footer readable at the minimum 320×200 window.
const PANEL_MIN_H: f32 = APPBAR_H * 4.0 + SPACE_2XL * 2.0;

pub enum WorktreeLauncherEvent {
    Command(PaletteRow),
}

impl EventEmitter<WorktreeLauncherEvent> for WorktreeLauncher {}

pub struct WorktreeLauncher {
    runtime: Entity<Runtime>,
    sidebar: Entity<super::sidebar::Sidebar>,
    input: Entity<InputState>,
    _input_subscription: Subscription,
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    open: bool,
    query: String,
    selected: usize,
    anchor: Option<RowIdentity>,
    agent_selected: usize,
    agent_touched: bool,
    agent_focus: bool,
    selected_worktrees: WorktreeSelection,
    error: Option<String>,
    scroll: ScrollHandle,
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
            return_focus: None,
            open: false,
            query: String::new(),
            selected: 0,
            anchor: None,
            agent_selected: 0,
            agent_touched: false,
            agent_focus: false,
            selected_worktrees: WorktreeSelection::default(),
            error: None,
            scroll: ScrollHandle::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    fn update_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        let rows = self.rows(cx);
        self.selected =
            launcher::resolve_row_by_identity(&rows, self.anchor.as_ref(), 0).unwrap_or(0);
        self.anchor = rows.get(self.selected).map(launcher::row_identity);
        self.sync_agent_to_row(&rows);
        self.error = None;
        cx.notify();
    }

    fn sync_agent_to_row(&mut self, rows: &[PaletteRow]) {
        if self.agent_touched {
            return;
        }
        if let Some(PaletteRow::Recent { agent, .. } | PaletteRow::Combo { agent, .. }) =
            rows.get(self.selected)
        {
            self.agent_selected = launcher::agent_sel_for(&Agent::ALL, *agent);
        }
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            return;
        }
        self.return_focus = window.focused(cx);
        self.open = true;
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
        self.error = None;
        self.scroll.set_offset(gpui::Point::default());
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        let rows = self.rows(cx);
        self.sync_agent_to_row(&rows);
        cx.notify();
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
        self.open = false;
        self.selected_worktrees.clear();
        self.error = None;
        if let Some(focus) = self.return_focus.take() {
            focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Cached worktrees for the active workspace, with a project root on a cold cache.
    fn combos(&self, cx: &App) -> Vec<(usize, String, String, Agent)> {
        let store = &cx.global::<SettingsState>().store;
        let state = self.runtime.read(cx).state.read(cx);
        let tree = self.runtime.read(cx).tree.read(cx);
        let agent = preferred_agent(store.default_agent);
        store
            .workspace_projects(store.workspaces.active)
            .flat_map(|(idx, project)| {
                let cached = tree.worktrees_for_project(idx, state.proj_idx());
                let paths: Vec<String> = if cached.is_empty() {
                    vec![project.path.clone()]
                } else {
                    cached.iter().map(|wt| wt.path.clone()).collect()
                };
                paths
                    .into_iter()
                    .map(move |path| (idx, project.name.clone(), path, agent))
            })
            .collect()
    }

    fn rows(&self, cx: &App) -> Vec<PaletteRow> {
        let store = &cx.global::<SettingsState>().store;
        let combos = self.combos(cx);
        let mut recent = Vec::new();
        for item in &store.recent_launches {
            if !item.agent.available() {
                continue;
            }
            if let Some((idx, _)) =
                store
                    .workspace_projects(store.workspaces.active)
                    .find(|(i, p)| {
                        p.name == item.project
                            && combos.iter().any(|(combo_idx, _, path, _)| {
                                combo_idx == i && path == &item.wt_path
                            })
                    })
            {
                if !recent
                    .iter()
                    .any(|(p, w, a)| *p == idx && w == &item.wt_path && *a == item.agent)
                {
                    recent.push((idx, item.wt_path.clone(), item.agent));
                }
            }
        }
        let mut rows = Vec::new();
        let mut seen = HashSet::new();
        for (proj, path, agent) in &recent {
            let name = store.projects.get(*proj).map_or("", |p| p.name.as_str());
            let wt_name = worktree_name(path);
            if launcher::fuzzy_match(&self.query, name, wt_name, agent.label()) {
                rows.push(PaletteRow::Recent {
                    proj: *proj,
                    wt_path: path.clone(),
                    agent: *agent,
                });
                seen.insert((*proj, path.clone()));
            }
        }
        let sidebar = self.sidebar.read(cx);
        let has_script = sidebar.palette_has_run_script(cx);
        let has_diff = sidebar.palette_has_diff(cx);
        let has_worktree = sidebar.selected_worktree().is_some();
        let has_session = !sidebar.visible_session_targets(cx).is_empty();
        if self.query.trim().is_empty() {
            if recent.is_empty() {
                rows.extend(launcher::root_rows(
                    &[],
                    &combos
                        .iter()
                        .map(|(p, _, w, a)| (*p, w.clone(), *a))
                        .collect::<Vec<_>>(),
                    store.projects.len(),
                    self.runtime.read(cx).state.read(cx).proj_idx(),
                    has_script,
                    has_diff,
                ));
            } else {
                rows.truncate(launcher::MAX_ROOT_RECENTS);
                rows.extend(
                    launcher::root_rows(&[], &[], 0, 0, has_script, has_diff)
                        .into_iter()
                        .filter(|row| {
                            !matches!(row, PaletteRow::Recent { .. } | PaletteRow::Combo { .. })
                        }),
                );
            }
            rows.dedup_by(|a, b| launcher::row_identity(a) == launcher::row_identity(b));
        } else {
            rows.extend(
                launcher::typed_rows(
                    &self.query,
                    &combos,
                    &recent,
                    has_script,
                    has_diff,
                    PaletteScope::All,
                )
                .into_iter()
                .filter(|row| match row {
                    PaletteRow::Combo { proj, wt_path, .. } => {
                        !seen.contains(&(*proj, wt_path.clone()))
                    }
                    _ => true,
                }),
            );
        }
        rows.retain(|row| match row {
            PaletteRow::TerminalWt => has_worktree,
            PaletteRow::SwitchToSession => has_session,
            _ => true,
        });
        rows
    }

    fn selected_row(&self, cx: &App) -> Option<PaletteRow> {
        let rows = self.rows(cx);
        let idx = launcher::resolve_row_by_identity(&rows, self.anchor.as_ref(), self.selected)?;
        rows.get(idx).cloned()
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
        let Some((_, project)) = project else {
            self.error = Some("Project is no longer in this workspace.".into());
            cx.notify();
            return;
        };
        if !agent.available() {
            self.error = Some(format!("{} is not installed.", agent.label()));
            cx.notify();
            return;
        }
        if !valid_worktree(&project.path, &wt_path) {
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
            self.error = Some(format!("{} is not installed.", agent.label()));
            cx.notify();
            return;
        }
        let store = &cx.global::<SettingsState>().store;
        let projects: Vec<_> = store
            .workspace_projects(store.workspaces.active)
            .map(|(_, project)| project.path.clone())
            .collect();
        if paths
            .iter()
            .any(|path| !projects.iter().any(|project| valid_worktree(project, path)))
        {
            self.error =
                Some("A selected worktree is no longer available in this workspace.".into());
            cx.notify();
            return;
        }
        let active_before = self.runtime.read(cx).state.read(cx).active_session();
        let did_launch = self.runtime.update(cx, |runtime, cx| {
            runtime.launch_multi_root_session(&paths, agent, cx)
        });
        self.finish_launch(did_launch, active_before, window, cx);
    }

    /// A failed PTY still leaves a selected session in the registry so the sidebar can retry it.
    fn finish_launch(
        &mut self,
        did_launch: bool,
        active_before: Option<SessionId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active_after = self.runtime.read(cx).state.read(cx).active_session();
        if did_launch || active_after.is_some_and(|id| Some(id) != active_before) {
            self.sidebar.update(cx, |sidebar, cx| {
                sidebar.select_active_session(cx);
            });
            self.close(window, cx);
        } else {
            self.error =
                Some("Could not start this session. Check the notification for details.".into());
            cx.notify();
        }
    }

    fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = Agent::ALL.get(self.agent_selected).copied() else {
            return;
        };
        let row = self.selected_row(cx);
        if self.selected_worktrees.count() > 0
            && row.as_ref().is_none_or(|row| {
                matches!(row, PaletteRow::Recent { .. } | PaletteRow::Combo { .. })
            })
        {
            self.launch_roots(
                self.selected_worktrees.selected_targets(),
                agent,
                window,
                cx,
            );
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
                self.query.clear();
                self.input
                    .update(cx, |input, cx| input.set_value("", window, cx));
                if let Some((index, row)) =
                    self.rows(cx).into_iter().enumerate().find(|(_, row)| {
                        matches!(row, PaletteRow::Recent { .. } | PaletteRow::Combo { .. })
                    })
                {
                    self.selected = index;
                    self.anchor = Some(launcher::row_identity(&row));
                    let rows = self.rows(cx);
                    self.sync_agent_to_row(&rows);
                    if matches!(row, PaletteRow::Recent { .. } | PaletteRow::Combo { .. }) {
                        self.agent_focus = true;
                    }
                    self.focus.focus(window, cx);
                    cx.notify();
                } else {
                    self.close(window, cx);
                    cx.emit(WorktreeLauncherEvent::Command(PaletteRow::AddProject));
                }
            }
            command => {
                self.close(window, cx);
                cx.emit(WorktreeLauncherEvent::Command(command));
            }
        }
    }

    fn toggle_selected_row(&mut self, cx: &mut Context<Self>) {
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
            .any(|(_, project)| valid_worktree(&project.path, &wt_path));
        if !valid {
            self.error = Some("Worktree is no longer available in this workspace.".into());
            cx.notify();
            return;
        }
        self.selected_worktrees.toggle(&path.to_string_lossy());
        self.error = None;
        cx.notify();
    }

    fn key(&mut self, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        let key = &event.keystroke;
        match key.key.as_str() {
            "escape" => self.close(window, cx),
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
            "tab" => {
                self.agent_focus = !self.agent_focus;
                if self.agent_focus {
                    self.focus.focus(window, cx);
                } else {
                    self.input.update(cx, |input, cx| input.focus(window, cx));
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
                let search_focused = self.input.read(cx).focus_handle(cx).is_focused(window);
                if search_focused && !key.modifiers.shift {
                    return;
                }
                self.toggle_selected_row(cx);
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
        let store = &cx.global::<SettingsState>().store;
        let rows = self.rows(cx);
        let selected_count = self.selected_worktrees.count();
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let viewport_w = f32::from(window.viewport_size().width) / scale;
        let viewport_h = f32::from(window.viewport_size().height) / scale;
        let panel_w = MODAL_W_LG.min((viewport_w - SPACE_LG * 2.0).max(0.0));
        let top = OVERLAY_TOP.min((viewport_h - PANEL_MIN_H - SPACE_LG).max(SPACE_LG));
        let panel_h = PANEL_MAX_H.min((viewport_h - top - SPACE_LG).max(0.0));
        let compact = panel_h < PANEL_MIN_H + APPBAR_H;
        div()
            .id("worktree-launcher-overlay")
            .debug_selector(|| "worktree-launcher-overlay".into())
            .absolute()
            .inset_0()
            .occlude()
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
                    .track_focus(&self.focus)
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
                            .pt(rpx(if compact { SPACE_2XL } else { SPACE_3XL }))
                            .pb(rpx(if compact { SPACE_LG } else { SPACE_2XL }))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_2XL))
                            .child(div().text_size(rpx(TEXT_TITLE)).text_color(c::FG()).child("Command palette"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(rpx(SPACE_LG))
                                    .child(
                                        div()
                                            .id("worktree-launcher-search")
                                            .debug_selector(|| "worktree-launcher-search".into())
                                            .flex_1()
                                            .min_w_0()
                                            .h(rpx(APPBAR_H))
                                            .px(rpx(SPACE_2XL))
                                            .rounded(rpx(RADIUS_PANEL))
                                            .bg(c::FIELD_FILL())
                                            .flex()
                                            .items_center()
                                            .child(
                                                Input::new(&self.input)
                                                    .appearance(false)
                                                    .bordered(false)
                                                    .focus_bordered(false)
                                                    .text_size(rpx(TEXT_BODY))
                                                    .text_color(c::FG())
                                                    .p_0(),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("launcher-agent-selector")
                                            .flex()
                                            .items_center()
                                            .gap(rpx(SPACE_XS))
                                            .children(Agent::ALL.into_iter().enumerate().map(|(index, agent)| {
                                                let selected = self.agent_selected == index;
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
                                                div()
                                                    .id(gpui::SharedString::from(format!("launcher-agent-{index}")))
                                                    .debug_selector(move || format!("launcher-agent-{index}"))
                                                    .role(gpui::Role::Button)
                                                    .aria_label(label.clone())
                                                    .size(rpx(ICON_BTN_W))
                                                    .rounded(rpx(RADIUS_CONTROL))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .when(selected, |button| button.bg(c::BG_HL()))
                                                    .when(selected && self.agent_focus, |button| button.border_1().border_color(c::SEL_RING()))
                                                    .hover(|button| button.bg(c::BG_HOVER()))
                                                    .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(label.clone()).build(window, cx))
                                                    .child(icon(icon_name, ICON_MD, if available { c::MAGENTA() } else { c::FG_MUTE() }))
                                                    .on_click(cx.listener(move |this, _, window, cx| {
                                                        if available {
                                                            this.agent_selected = index;
                                                            this.agent_touched = true;
                                                            this.agent_focus = true;
                                                            this.error = None;
                                                            this.focus.focus(window, cx);
                                                        } else {
                                                            this.error = Some(format!("{} is not installed.", agent.label()));
                                                        }
                                                        cx.notify();
                                                    }))
                                            })),
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
                            .pb(rpx(if compact { SPACE_XS } else { SPACE_LG }))
                            .when(rows.is_empty(), |list| list.child(
                                div().p(rpx(SPACE_3XL)).text_color(c::FG_DIM()).text_size(rpx(TEXT_BODY)).child("No matching commands")
                            ))
                            .children(rows.into_iter().enumerate().map(|(index, row)| {
                                let (label, detail, icon_name, suffix) = match &row {
                                    PaletteRow::Recent { proj, wt_path, agent } | PaletteRow::Combo { proj, wt_path, agent } => {
                                        let project = store.projects.get(*proj).map_or("", |p| p.name.as_str());
                                        (format!("{} / {}", project, worktree_name(wt_path)), wt_path.clone(), "git-branch", agent.label().to_string())
                                    }
                                    PaletteRow::NewSession => ("New session".into(), "Choose a worktree".into(), "plus", String::new()),
                                    PaletteRow::NewMultiProjectSession => ("New multi-project session".into(), "Select multiple worktrees".into(), "plus", String::new()),
                                    PaletteRow::TerminalHome => ("New home terminal".into(), "Open a terminal in the workspace".into(), "terminal", String::new()),
                                    PaletteRow::TerminalWt => ("New worktree terminal".into(), "Open a terminal in the selected worktree".into(), "terminal", String::new()),
                                    PaletteRow::AddProject => ("Add project".into(), "Add a local project to this workspace".into(), "plus", String::new()),
                                    PaletteRow::RunScript => ("Run script".into(), "Run the selected worktree script".into(), "play", String::new()),
                                    PaletteRow::ViewDiff => ("View diff".into(), "Open the selected session diff".into(), "git-branch", String::new()),
                                    PaletteRow::SwitchToSession => ("Switch to session".into(), "Choose an open session".into(), "list", String::new()),
                                    PaletteRow::Settings => ("Settings".into(), "App preferences".into(), "cog", String::new()),
                                    PaletteRow::Setting(setting) => (setting.label().into(), setting.section().into(), setting.icon_name(), String::new()),
                                    PaletteRow::ReloadThemes => ("Reload themes".into(), "Refresh installed themes".into(), "restart", String::new()),
                                };
                                let identity = launcher::row_identity(&row);
                                let checked = match &row {
                                    PaletteRow::Recent { wt_path, .. } | PaletteRow::Combo { wt_path, .. } => fs_err::canonicalize(wt_path)
                                        .ok().is_some_and(|path| self.selected_worktrees.contains(&path.to_string_lossy())),
                                    _ => false,
                                };
                                div()
                                    .id(gpui::SharedString::from(format!("launcher-row-{index}")))
                                    .debug_selector(move || format!("launcher-row-{index}"))
                                    .role(gpui::Role::Button)
                                    .aria_label(if checked { format!("Selected: {label}") } else { label.clone() })
                                    .h(rpx(ROW_H))
                                    .px(rpx(SPACE_2XL))
                                    .rounded(rpx(RADIUS_GROUP))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(rpx(SPACE_LG))
                                    .when(index == self.selected, |row| row.bg(c::BG_HL()))
                                    .hover(|row| row.bg(c::BG_HOVER()))
                                    .when(checked, |row| row.border_l_2().border_color(c::SEL_RING()))
                                    .child(
                                        div().flex().items_center().gap(rpx(SPACE_LG)).min_w_0()
                                            .child(icon(icon_name, ICON_MD, c::FG_DIM()))
                                            .child(div().flex().flex_col().min_w_0().gap(rpx(SPACE_XS))
                                                .child(div().text_size(rpx(TEXT_BODY)).text_color(c::FG()).truncate().child(label))
                                                .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_MUTE()).truncate().child(detail)))
                                    )
                                    .when(!suffix.is_empty() || checked, |row| row.child(
                                        div().flex_shrink_0().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM())
                                            .child(if checked { "Selected".to_string() } else { suffix })
                                    ))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let rows = this.rows(cx);
                                        if let Some(current) = launcher::resolve_row_by_identity(&rows, Some(&identity), index) {
                                            this.selected = current;
                                            this.anchor = Some(identity.clone());
                                            if this.selected_worktrees.count() > 0 && matches!(identity, RowIdentity::Session { .. }) {
                                                this.toggle_selected_row(cx);
                                            } else {
                                                this.activate(window, cx);
                                            }
                                        }
                                    }))
                            })),
                    )
                    .when_some(self.error.clone(), |panel, error| panel.child(
                        div().px(rpx(SPACE_3XL)).pb(rpx(SPACE_LG)).text_size(rpx(TEXT_SMALL)).text_color(c::RED()).child(error)
                    ))
                    .child(
                        div()
                            .id("worktree-launcher-footer")
                            .debug_selector(|| "worktree-launcher-footer".into())
                            .border_t_1()
                            .border_color(c::BORDER_SOFT())
                            .px(rpx(SPACE_3XL))
                            .py(rpx(SPACE_LG))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_MUTE())
                            .child(if selected_count > 0 {
                                format!("Shift+Space toggle  ·  Enter launch with selected tool  ·  {selected_count} selected  ·  Esc close")
                            } else if compact {
                                "↑↓ rows · Tab tools · ↵ activate · Esc close".to_string()
                            } else {
                                "↑↓ navigate  ·  Tab tools  ·  ←→ choose tool  ·  Shift+Space select  ·  Enter activate  ·  Esc close".to_string()
                            }),
                    ),
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

fn valid_worktree(project_path: &str, target_path: &str) -> bool {
    let Ok(target) = fs_err::canonicalize(target_path) else {
        return false;
    };
    grove_core::git::list_worktrees(project_path)
        .iter()
        .any(|wt| fs_err::canonicalize(&wt.path).is_ok_and(|path| path == target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use grove_core::storage::{Project, ProjectScripts, RecentLaunch, Store};
    use std::process::Command;

    struct TempProjects(std::path::PathBuf);

    impl Drop for TempProjects {
        fn drop(&mut self) {
            let _ = fs_err::remove_dir_all(&self.0);
        }
    }

    fn project(name: &str, path: &str) -> Project {
        Project {
            name: name.into(),
            path: path.into(),
            scripts: ProjectScripts::default(),
            archived: false,
            worktree_dir: None,
        }
    }

    fn setup(cx: &mut App) {
        gpui_component::init(cx);
        let root = env!("CARGO_MANIFEST_DIR").to_string();
        let missing = "/grove-launcher-test-missing".to_string();
        let outside = "/grove-launcher-test-outside".to_string();
        let mut store = Store {
            projects: vec![
                project("current", &root),
                project("missing", &missing),
                project("outside", &outside),
            ],
            recent_launches: vec![
                RecentLaunch {
                    project: "current".into(),
                    wt_path: root.clone(),
                    agent: Agent::Terminal,
                },
                RecentLaunch {
                    project: "missing".into(),
                    wt_path: "/grove-launcher-test-stale".into(),
                    agent: Agent::Terminal,
                },
                RecentLaunch {
                    project: "outside".into(),
                    wt_path: outside.clone(),
                    agent: Agent::Terminal,
                },
            ],
            ..Store::default()
        };
        store.workspaces.create("Other").expect("workspace");
        let other = store.workspaces.active;
        store.project_workspaces.insert(outside, other);
        store.workspaces.select(1);
        cx.set_global(SettingsState::new(store));
        cx.set_global(crate::zoom::ZoomState::new(1.0));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::theme::ThemeState::new(
            false,
            "tokyonight".into(),
            "tokyonight-day".into(),
        ));
    }

    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn scoped_open_skips_project_search_and_preserves_selected_worktree(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(setup);
        let (launcher, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar =
                cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
            WorktreeLauncher::new(runtime, sidebar, window, cx)
        });
        let path = env!("CARGO_MANIFEST_DIR").to_string();
        cx.update(|window, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.open_for_worktree(0, &path, window, cx);
            });
            let launcher = launcher.read(cx);
            assert!(launcher.is_open());
            assert!(matches!(&launcher.anchor,
                Some(RowIdentity::Session { proj: 0, wt_path, .. }) if wt_path == &path));
            assert!(launcher.agent_focus);
            assert!(launcher.focus.is_focused(window));
        });
        draw(cx);
        assert!(cx.debug_bounds("launcher-agent-0").is_some());
        cx.update(|window, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.close(window, cx);
                launcher.open_for_worktree(1, "/grove-launcher-test-missing", window, cx);
                assert!(matches!(&launcher.anchor,
                    Some(RowIdentity::Session { proj: 1, wt_path, .. }) if wt_path == "/grove-launcher-test-missing"));
            });
        });
    }

    #[gpui::test]
    fn rows_scope_filter_and_validate_recents(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (launcher, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar =
                cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
            WorktreeLauncher::new(runtime, sidebar, window, cx)
        });
        cx.update(|_, cx| {
            let rows = launcher.read(cx).rows(cx);
            assert!(matches!(
                rows.first(),
                Some(PaletteRow::Recent { proj: 0, .. })
            ));
            assert!(rows.iter().any(|row| matches!(row, PaletteRow::Settings)));
            assert!(rows.iter().any(|row| matches!(row, PaletteRow::AddProject)));
            assert!(!rows
                .iter()
                .any(|row| matches!(row, PaletteRow::Recent { proj: 1, .. })));
            assert!(!rows.iter().any(|row| matches!(
                row,
                PaletteRow::Recent { proj: 2, .. } | PaletteRow::Combo { proj: 2, .. }
            )));
            launcher.update(cx, |launcher, cx| {
                launcher.update_query("missing".into(), cx);
                let initial = launcher.rows(cx);
                let combo = initial
                    .iter()
                    .position(|row| matches!(row, PaletteRow::Combo { proj: 1, .. }))
                    .expect("uncached project root");
                let query = match &initial[combo] {
                    PaletteRow::Combo { wt_path, .. } => worktree_name(wt_path).to_string(),
                    _ => unreachable!(),
                };
                launcher.selected = combo;
                launcher.anchor = Some(launcher::row_identity(&initial[combo]));
                let identity = launcher.anchor.clone();
                launcher.update_query(query, cx);
                assert_eq!(launcher.anchor, identity);
                assert_eq!(
                    launcher
                        .rows(cx)
                        .get(launcher.selected)
                        .map(launcher::row_identity),
                    identity
                );
                launcher.update_query("ui-overhaul".into(), cx);
                assert_eq!(launcher.rows(cx).len(), 1);
                launcher.update_query("absent query".into(), cx);
                assert!(launcher.rows(cx).is_empty());
            });
        });
    }

    #[gpui::test]
    fn command_rows_and_setting_search_are_present(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (launcher, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar =
                cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
            WorktreeLauncher::new(runtime, sidebar, window, cx)
        });
        cx.update(|_, cx| {
            let rows = launcher.read(cx).rows(cx);
            for action in [
                PaletteRow::NewSession,
                PaletteRow::NewMultiProjectSession,
                PaletteRow::TerminalHome,
                PaletteRow::AddProject,
                PaletteRow::Settings,
            ] {
                assert!(rows.contains(&action), "missing {action:?}");
            }
            launcher.update(cx, |launcher, cx| {
                launcher.update_query("app theme".into(), cx);
                assert!(launcher
                    .rows(cx)
                    .contains(&PaletteRow::Setting(launcher::SettingRow::Theme)));
                launcher.update_query("terminal home".into(), cx);
                assert!(launcher.rows(cx).contains(&PaletteRow::TerminalHome));
                launcher.update_query("reload themes".into(), cx);
                assert!(launcher.rows(cx).contains(&PaletteRow::ReloadThemes));
            });
        });
    }

    #[gpui::test]
    fn tool_icons_share_search_row_and_keyboard_cycles_available_tools(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(setup);
        let (launcher, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar =
                cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
            WorktreeLauncher::new(runtime, sidebar, window, cx)
        });
        cx.update(|window, cx| launcher.update(cx, |launcher, cx| launcher.open(window, cx)));
        draw(cx);
        let search = cx.debug_bounds("worktree-launcher-search").expect("search");
        for name in [
            "launcher-agent-0",
            "launcher-agent-1",
            "launcher-agent-2",
            "launcher-agent-3",
        ] {
            let bounds = cx.debug_bounds(name).expect("tool icon");
            assert!(bounds.top() < search.bottom() && bounds.bottom() > search.top());
        }
        let before = cx.update(|_, cx| launcher.read(cx).agent_selected);
        cx.simulate_keystrokes("tab right");
        draw(cx);
        cx.update(|_, cx| {
            let view = launcher.read(cx);
            let available = available_agents();
            let before_agent = Agent::ALL[before];
            let expected = available[launcher::cycle(
                launcher::agent_sel_for(&available, before_agent),
                1,
                available.len(),
            )];
            assert_eq!(Agent::ALL[view.agent_selected], expected);
            assert!(view.agent_focus);
            assert!(view.agent_touched);
        });
    }

    #[gpui::test]
    fn keyboard_selection_agent_step_and_cancel_restore_focus(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (launcher, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar =
                cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
            WorktreeLauncher::new(runtime, sidebar, window, cx)
        });
        let prior = cx.update(|window, cx| {
            let prior = cx.focus_handle();
            prior.focus(window, cx);
            launcher.update(cx, |launcher, cx| launcher.open(window, cx));
            assert!(launcher.read(cx).is_open());
            assert!(launcher.read(cx).input.focus_handle(cx).is_focused(window));
            prior
        });
        draw(cx);
        cx.simulate_keystrokes("up");
        draw(cx);
        cx.update(|_, cx| {
            let view = launcher.read(cx);
            assert_eq!(view.selected, view.rows(cx).len() - 1);
            assert_eq!(
                view.anchor,
                view.rows(cx).last().map(launcher::row_identity)
            );
        });
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| {
            assert!(launcher.read(cx).agent_focus);
            assert!(launcher.read(cx).focus.is_focused(window));
        });
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            assert!(!launcher.read(cx).is_open());
            assert!(prior.is_focused(window));
        });
    }

    #[gpui::test]
    fn space_selection_survives_filter_and_agent_back(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (launcher, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar =
                cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
            WorktreeLauncher::new(runtime, sidebar, window, cx)
        });
        cx.update(|window, cx| launcher.update(cx, |launcher, cx| launcher.open(window, cx)));
        draw(cx);
        cx.simulate_keystrokes("shift-space");
        draw(cx);
        cx.update(|_, cx| {
            let view = launcher.read(cx);
            assert_eq!(view.selected_worktrees.count(), 1);
            assert!(view.query.is_empty());
        });
        cx.update(|_, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.update_query("no matching worktree".into(), cx);
                assert!(launcher.rows(cx).is_empty());
                assert_eq!(launcher.selected_worktrees.count(), 1);
            });
        });
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| {
            let view = launcher.read(cx);
            assert_eq!(view.selected_worktrees.count(), 1);
            assert!(view.agent_focus);
            assert!(view.focus.is_focused(window));
        });
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|_, cx| assert_eq!(launcher.read(cx).selected_worktrees.count(), 0));
    }

    #[gpui::test]
    fn search_accepts_spaces_without_toggling_worktrees(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (launcher, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar =
                cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
            WorktreeLauncher::new(runtime, sidebar, window, cx)
        });
        cx.update(|window, cx| launcher.update(cx, |launcher, cx| launcher.open(window, cx)));
        draw(cx);
        cx.simulate_input("current");
        cx.simulate_keystrokes("space");
        cx.simulate_input("branch");
        draw(cx);
        cx.update(|_, cx| {
            let view = launcher.read(cx);
            assert_eq!(view.input.read(cx).value().as_ref(), "current branch");
            assert_eq!(view.query, "current branch");
            assert_eq!(view.selected_worktrees.count(), 0);
        });
    }

    #[gpui::test]
    fn selected_stale_path_cannot_launch(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (launcher, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar =
                cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
            WorktreeLauncher::new(runtime, sidebar, window, cx)
        });
        cx.update(|window, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.open(window, cx);
                launcher
                    .selected_worktrees
                    .toggle("/grove-launcher-test-stale");
                launcher.activate(window, cx);
                assert_eq!(
                    launcher.error.as_deref(),
                    Some("A selected worktree is no longer available in this workspace.")
                );
                assert!(launcher.is_open());
                assert!(launcher
                    .runtime
                    .read(cx)
                    .state
                    .read(cx)
                    .active_session()
                    .is_none());
                assert!(launcher.sidebar.read(cx).selected_session().is_none());
            });
        });
    }

    #[gpui::test]
    fn registered_failed_session_closes_palette_and_selects_retry_row(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(setup);
        let (launcher, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar =
                cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
            WorktreeLauncher::new(runtime, sidebar, window, cx)
        });
        cx.update(|window, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.open(window, cx);
                let runtime = launcher.runtime.clone();
                let before = runtime.read(cx).state.read(cx).active_session();
                let path = env!("CARGO_MANIFEST_DIR").to_string();
                let did_spawn = runtime.update(cx, |runtime, cx| {
                    runtime.spawn_session_in_with_context(
                        "current".into(),
                        path,
                        Agent::Terminal,
                        vec!["\0".into()],
                        Vec::new(),
                        None,
                        cx,
                    )
                });
                assert!(!did_spawn);
                let failed_id = runtime
                    .read(cx)
                    .state
                    .read(cx)
                    .active_session()
                    .expect("failed session is registered");
                assert!(runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .session(failed_id)
                    .expect("failed session terminal")
                    .read(cx)
                    .spawn_error()
                    .is_some());
                launcher.finish_launch(did_spawn, before, window, cx);
                assert!(!launcher.is_open());
                assert_eq!(
                    launcher.sidebar.read(cx).selected_session(),
                    Some(failed_id)
                );
            });
        });
    }

    #[gpui::test]
    fn selected_roots_launch_one_session_in_selection_order(cx: &mut gpui::TestAppContext) {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp = TempProjects(std::env::temp_dir().join(format!(
            "grove-launcher-test-{}-{unique}",
            std::process::id()
        )));
        fs_err::create_dir(&temp.0).expect("temporary projects");
        let first = temp.0.join("first");
        let second = temp.0.join("second");
        for path in [&first, &second] {
            assert!(Command::new("git")
                .args(["init", "-q"])
                .arg(path)
                .status()
                .expect("git init")
                .success());
        }
        let first = fs_err::canonicalize(&first)
            .expect("first project")
            .to_string_lossy()
            .into_owned();
        let second = fs_err::canonicalize(&second)
            .expect("second project")
            .to_string_lossy()
            .into_owned();
        cx.update(|cx| {
            setup(cx);
            let store = &mut cx.global_mut::<SettingsState>().store;
            store.projects = vec![project("first", &first), project("second", &second)];
            store.recent_launches.clear();
            store.assign_project_to_active_workspace(&first);
            store.assign_project_to_active_workspace(&second);
        });
        let (launcher, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar =
                cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
            WorktreeLauncher::new(runtime, sidebar, window, cx)
        });
        cx.update(|window, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.open(window, cx);
                let rows = launcher.rows(cx);
                for target in [&second, &first] {
                    let index = rows
                        .iter()
                        .position(|row| matches!(row, PaletteRow::Combo { wt_path, .. } if wt_path == target))
                        .expect("worktree row");
                    launcher.selected = index;
                    launcher.anchor = Some(launcher::row_identity(&rows[index]));
                    launcher.toggle_selected_row(cx);
                }
                assert_eq!(launcher.selected_worktrees.selected_targets(), vec![second.clone(), first.clone()]);
                launcher.agent_selected = launcher::agent_sel_for(&Agent::ALL, Agent::Terminal);
                launcher.activate(window, cx);
                assert!(!launcher.is_open());
            });
            let runtime = launcher.read(cx).runtime.clone();
            let registry = runtime.read(cx).registry.read(cx);
            assert_eq!(registry.all().len(), 1);
            let roots = &registry.all()[0].context_roots;
            assert_eq!(roots.iter().map(|root| root.wt_path.as_str()).collect::<Vec<_>>(), vec![second.as_str(), first.as_str()]);
        });
    }

    #[gpui::test]
    fn stale_selection_cannot_launch(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (launcher, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar =
                cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
            WorktreeLauncher::new(runtime, sidebar, window, cx)
        });
        cx.update(|window, cx| {
            launcher.update(cx, |launcher, cx| {
                launcher.open(window, cx);
                launcher.launch(
                    RowIdentity::Session {
                        proj: 1,
                        wt_path: "/grove-launcher-test-missing".into(),
                        agent: Agent::Terminal,
                    },
                    Agent::Terminal,
                    window,
                    cx,
                );
                assert_eq!(
                    launcher.error.as_deref(),
                    Some("Worktree is no longer available.")
                );
                assert!(launcher.is_open());
                launcher.launch(
                    RowIdentity::Session {
                        proj: 2,
                        wt_path: "/grove-launcher-test-outside".into(),
                        agent: Agent::Terminal,
                    },
                    Agent::Terminal,
                    window,
                    cx,
                );
                assert_eq!(
                    launcher.error.as_deref(),
                    Some("Project is no longer in this workspace.")
                );
            });
        });
    }

    #[gpui::test]
    fn successful_launch_records_recent_and_closes(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (launcher, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar =
                cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
            WorktreeLauncher::new(runtime, sidebar, window, cx)
        });
        cx.update(|window, cx| {
            let path = env!("CARGO_MANIFEST_DIR").to_string();
            launcher.update(cx, |launcher, cx| {
                launcher.open(window, cx);
                launcher.launch(
                    RowIdentity::Session {
                        proj: 0,
                        wt_path: path.clone(),
                        agent: Agent::Terminal,
                    },
                    Agent::Terminal,
                    window,
                    cx,
                );
                assert!(!launcher.is_open());
            });
            let recent = &cx.global::<SettingsState>().store.recent_launches;
            assert_eq!(recent[0].project, "current");
            assert_eq!(recent[0].wt_path, path);
            assert_eq!(recent[0].agent, Agent::Terminal);
            assert_eq!(
                recent
                    .iter()
                    .filter(|item| item.project == "current"
                        && item.wt_path == path
                        && item.agent == Agent::Terminal)
                    .count(),
                1
            );
            let sidebar = launcher.read(cx).sidebar.clone();
            let selected = sidebar
                .read(cx)
                .selected_session()
                .expect("new canvas selection");
            assert_eq!(
                launcher
                    .read(cx)
                    .runtime
                    .read(cx)
                    .state
                    .read(cx)
                    .active_session(),
                Some(selected)
            );
            launcher.update(cx, |launcher, cx| {
                launcher.open(window, cx);
                launcher.launch(
                    RowIdentity::Session {
                        proj: 1,
                        wt_path: "/grove-launcher-test-missing".into(),
                        agent: Agent::Terminal,
                    },
                    Agent::Terminal,
                    window,
                    cx,
                );
                assert!(launcher.is_open());
            });
            assert_eq!(sidebar.read(cx).selected_session(), Some(selected));
        });
    }

    #[gpui::test]
    fn palette_fits_minimum_and_desktop_viewports(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (launcher, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar =
                cx.new(|cx| super::super::sidebar::Sidebar::new(runtime.clone(), window, cx));
            WorktreeLauncher::new(runtime, sidebar, window, cx)
        });
        cx.update(|window, cx| launcher.update(cx, |launcher, cx| launcher.open(window, cx)));
        for (width, height) in [(320.0, 200.0), (1280.0, 800.0)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
            draw(cx);
            let panel = cx.debug_bounds("worktree-launcher-panel").expect("panel");
            let search = cx.debug_bounds("worktree-launcher-search").expect("search");
            let list = cx
                .debug_bounds("worktree-launcher-list")
                .expect("scroll list");
            let row = cx.debug_bounds("launcher-row-0").expect("first row");
            let footer = cx.debug_bounds("worktree-launcher-footer").expect("footer");
            assert!(
                panel.left() >= gpui::px(0.0) && panel.right() <= gpui::px(width),
                "panel width at {width}: {panel:?}"
            );
            assert!(
                panel.top() >= gpui::px(0.0) && panel.bottom() <= gpui::px(height),
                "panel height at {height}: {panel:?}"
            );
            for (name, bounds) in [
                ("search", search),
                ("list", list),
                ("row", row),
                ("footer", footer),
            ] {
                assert!(
                    bounds.left() >= panel.left() && bounds.right() <= panel.right(),
                    "{name} clips horizontally at {width}: {bounds:?} / {panel:?}"
                );
                assert!(
                    bounds.top() >= panel.top() && bounds.bottom() <= panel.bottom(),
                    "{name} clips vertically at {height}: {bounds:?} / {panel:?}"
                );
            }
            assert!(search.bottom() <= list.top());
            assert!(list.bottom() <= footer.top());
            assert!(f32::from(panel.size.width) <= MODAL_W_LG);
            assert!(f32::from(panel.size.height) <= PANEL_MAX_H);
            cx.update(|window, cx| {
                assert!(launcher.read(cx).input.focus_handle(cx).is_focused(window));
            });
            cx.update(|window, cx| {
                launcher.update(cx, |launcher, cx| {
                    launcher.agent_focus = true;
                    launcher.focus.focus(window, cx);
                    cx.notify();
                });
            });
            draw(cx);
            let panel = cx
                .debug_bounds("worktree-launcher-panel")
                .expect("agent panel");
            let agent = cx.debug_bounds("launcher-agent-0").expect("first agent");
            let footer = cx
                .debug_bounds("worktree-launcher-footer")
                .expect("agent footer");
            assert!(agent.left() >= panel.left() && agent.right() <= panel.right());
            assert!(agent.top() >= panel.top() && agent.bottom() <= panel.bottom());
            assert!(footer.left() >= panel.left() && footer.right() <= panel.right());
            assert!(footer.top() >= panel.top() && footer.bottom() <= panel.bottom());
            cx.update(|window, cx| {
                assert!(launcher.read(cx).focus.is_focused(window));
                launcher.update(cx, |launcher, cx| {
                    launcher.agent_focus = false;
                    launcher
                        .input
                        .update(cx, |input, cx| input.focus(window, cx));
                    cx.notify();
                });
            });
        }
    }
}
