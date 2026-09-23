//! Workspace-scoped navigation. UI state is kept per workspace while processes keep running.
mod content;
mod grid;
mod project_setup;
mod projects;
use super::{rpx, terminal_view::TerminalView, tokens::*};
use crate::{
    activity::ActivityState,
    entities::{
        session_registry::{SessionId, SessionMeta},
        workspace_state::TreeSnapshot,
    },
    icons::icon,
    project_service::{ProjectEvent, WorktreeRemovalStage},
    runtime::Runtime,
    settings::SettingsState,
    theme as c,
};
use gpui::{
    div, prelude::*, AnyElement, App, Context, Div, Entity, FocusHandle, Focusable, ScrollHandle,
    SharedString, Stateful, Window,
};
use gpui_component::input::InputState;
use grove_core::agent::Agent;
use std::collections::{HashMap, HashSet};

const SIDEBAR_W: f32 = 260.0;
const SIDEBAR_COMPACT_THRESHOLD: f32 = 236.0;
const SIDEBAR_MAX_VIEWPORT_FRACTION: f32 = 0.4;
/// Below this width, a setup editor temporarily owns the full canvas.
const EDITOR_FULL_WIDTH_BREAKPOINT: f32 = 640.0;
const HEAD_H: f32 = 36.0;
const ROW_H: f32 = 28.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Selection {
    Project(usize),
    Worktree(usize, String),
    Session(SessionId),
    Home(SessionId),
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ViewMode {
    #[default]
    Project,
    List,
    Grid,
}
#[derive(Default)]
struct Navigation {
    selection: Option<Selection>,
    mode: ViewMode,
    last_mode: ViewMode,
    collapsed_projects: HashSet<usize>,
    collapsed_worktrees: HashSet<String>,
    scroll: ScrollHandle,
    terminals_collapsed: bool,
}
#[derive(Clone)]
enum Action {
    Select(Selection),
    Project(usize),
    Worktree(String),
    Mode(ViewMode),
    Menu(usize),
    NewWorktree(String),
    EditProject(String),
    AddProject,
    ArchivedProjects,
    Reveal(String),
    RemoveProject(String),
    ConfirmRemove(usize),
    RemoveWorktree(usize, String),
    ConfirmWorktreeRemoval,
    SkipWorktreeTeardown,
    DismissWorktreeRemoval,
    Launch(usize, String, Agent),
    Close(SessionId),
    ConfirmClose(SessionId),
    Retry(SessionId),
    AddTerminal,
    FoldTerminals,
    CloseHome(SessionId),
    ConfirmHome(SessionId),
    Cancel,
}

#[derive(Clone)]
struct PendingWorktreeRemoval {
    project_path: String,
    path: String,
    name: String,
    started: bool,
    error: Option<String>,
}

pub struct Sidebar {
    project_panel: Option<Entity<projects::ProjectPanel>>,
    project_setup: Option<Entity<project_setup::ProjectSetup>>,
    project_decision: bool,
    project_return_focus: Option<FocusHandle>,
    project_return_path: Option<String>,
    workspace_selector: Option<Entity<super::workspace_manager::WorkspaceManager>>,
    runtime: Entity<Runtime>,
    focus: FocusHandle,
    selection: Option<Selection>,
    mode: ViewMode,
    last_mode: ViewMode,
    snapshot: TreeSnapshot,
    active_workspace: u64,
    saved: HashMap<u64, Navigation>,
    collapsed_projects: HashSet<usize>,
    collapsed_worktrees: HashSet<String>,
    scroll: ScrollHandle,
    terminal_owners: HashMap<SessionId, u64>,
    terminals_collapsed: bool,
    menu: Option<usize>,
    menu_focus: FocusHandle,
    menu_index: usize,
    project_menu_focus: HashMap<usize, FocusHandle>,
    menu_return_focus: Option<FocusHandle>,
    project_menu_bounds: HashMap<usize, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    menu_trigger_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    confirm_focus: FocusHandle,
    cancel_focus: FocusHandle,
    confirmation_return_focus: Option<FocusHandle>,
    pending_close: Option<SessionId>,
    pending_home_close: Option<SessionId>,
    pending_remove: Option<usize>,
    pending_worktree_removal: Option<PendingWorktreeRemoval>,
    worktree_removal_focus: FocusHandle,
    worktree_delete_focus: HashMap<String, FocusHandle>,
    worktree_removal_return_focus: Option<FocusHandle>,
    pending_new_worktree: Option<usize>,
    worktree_name: Entity<InputState>,
    worktree_errors: [Option<String>; 3],
    worktree_return_focus: Option<FocusHandle>,
    worktree_branch: Entity<InputState>,
    worktree_base: Entity<InputState>,
    content_error: Option<String>,
    terminal_views: HashMap<SessionId, Entity<TerminalView>>,
    home_terminal_views: HashMap<SessionId, Entity<TerminalView>>,
    available: [bool; 3],
    project_paths: HashMap<usize, String>,
    worktree_focus: HashMap<String, FocusHandle>,
    cache_warm: Option<(u64, u64)>,
    compact_rail: bool,
    grid_layouts: HashMap<u64, grid::WorkspaceGrid>,
    grid_drag: Option<grid::GridDrag>,
    grid_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    canvas_close_anchor: Option<SessionId>,
    canvas_close_focus: HashMap<(SessionId, bool), FocusHandle>,
    canvas_bounds: HashMap<SessionId, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    canvas_observers: HashMap<SessionId, gpui::Subscription>,
    canvas_focus_observers: HashMap<SessionId, gpui::Subscription>,
    canvas_signatures: HashMap<SessionId, (Option<String>, Option<String>, bool, bool)>,
    observers: Vec<gpui::Subscription>,
}
impl Sidebar {
    pub(crate) fn set_workspace_selector(
        &mut self,
        selector: Entity<super::workspace_manager::WorkspaceManager>,
    ) {
        self.workspace_selector = Some(selector);
    }
    pub fn new(runtime: Entity<Runtime>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rt = runtime.read(cx);
        let (registry, activity, tree, projects) = (
            rt.registry.clone(),
            rt.activity.clone(),
            rt.tree.clone(),
            rt.projects.clone(),
        );
        let worktree_name = cx.new(|cx| InputState::new(window, cx).placeholder("billing-retry"));
        let worktree_branch =
            cx.new(|cx| InputState::new(window, cx).placeholder("feat/billing-retry"));
        let worktree_base = cx.new(|cx| InputState::new(window, cx).placeholder("main"));
        let mut observers = vec![
            cx.observe(&runtime, |_, _, cx| cx.notify()),
            cx.observe(&registry, |_, _, cx| cx.notify()),
            cx.observe(&activity, |_, _, cx| cx.notify()),
            cx.observe(&tree, |_, _, cx| cx.notify()),
            cx.subscribe(&projects, |this, _, event, cx| {
                if let ProjectEvent::WorktreeRemovalChanged { path } = event {
                    this.worktree_removal_changed(path, cx);
                }
            }),
            cx.observe_global::<SettingsState>(|_, cx| cx.notify()),
        ];
        for (index, input) in [&worktree_name, &worktree_branch, &worktree_base]
            .into_iter()
            .enumerate()
        {
            observers.push(cx.subscribe(input, move |this, _, event, cx| {
                if matches!(event, gpui_component::input::InputEvent::PressEnter { .. }) {
                    this.submit_worktree(cx);
                } else if matches!(event, gpui_component::input::InputEvent::Change) {
                    this.worktree_errors[index] = None;
                    cx.notify();
                }
            }));
        }
        Self {
            project_panel: None,
            project_setup: None,
            project_decision: false,
            project_return_focus: None,
            project_return_path: None,
            runtime,
            workspace_selector: None,
            focus: cx.focus_handle(),
            selection: None,
            mode: ViewMode::Project,
            last_mode: ViewMode::Project,
            snapshot: TreeSnapshot::default(),
            active_workspace: cx.global::<SettingsState>().store.workspaces.active,
            saved: HashMap::new(),
            collapsed_projects: HashSet::new(),
            collapsed_worktrees: HashSet::new(),
            scroll: ScrollHandle::new(),
            terminal_owners: HashMap::new(),
            terminals_collapsed: false,
            menu: None,
            menu_focus: cx.focus_handle(),
            menu_index: 0,
            project_menu_focus: HashMap::new(),
            menu_return_focus: None,
            menu_trigger_bounds: std::rc::Rc::default(),
            project_menu_bounds: HashMap::new(),
            confirm_focus: cx.focus_handle(),
            cancel_focus: cx.focus_handle(),
            confirmation_return_focus: None,
            pending_close: None,
            pending_home_close: None,
            pending_remove: None,
            pending_worktree_removal: None,
            worktree_removal_focus: cx.focus_handle(),
            worktree_delete_focus: HashMap::new(),
            worktree_removal_return_focus: None,
            pending_new_worktree: None,
            worktree_name,
            worktree_errors: [None, None, None],
            worktree_return_focus: None,
            worktree_branch,
            worktree_base,
            content_error: None,
            terminal_views: HashMap::new(),
            home_terminal_views: HashMap::new(),
            available: [Agent::Codex.available(), Agent::Claude.available(), true],
            project_paths: HashMap::new(),
            worktree_focus: HashMap::new(),
            cache_warm: None,
            compact_rail: false,
            grid_layouts: HashMap::new(),
            grid_drag: None,
            grid_bounds: std::rc::Rc::default(),
            canvas_close_anchor: None,
            canvas_close_focus: HashMap::new(),
            canvas_bounds: HashMap::new(),
            canvas_observers: HashMap::new(),
            canvas_focus_observers: HashMap::new(),
            canvas_signatures: HashMap::new(),
            observers,
        }
    }
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths: HashMap<usize, String> = cx
            .global::<SettingsState>()
            .store
            .projects
            .iter()
            .enumerate()
            .map(|(idx, p)| (idx, p.path.clone()))
            .collect();
        let mut mapping: HashMap<usize, usize> = self
            .project_paths
            .iter()
            .filter_map(|(old, path)| {
                paths
                    .iter()
                    .find(|(_, p)| *p == path)
                    .map(|(new, _)| (*old, *new))
            })
            .collect();
        for idx in paths.keys() {
            if !self.project_paths.contains_key(idx) {
                mapping.insert(*idx, *idx);
            }
        }
        if self.project_paths != paths {
            remap_selection(&mut self.selection, &mapping);
            self.collapsed_projects = self
                .collapsed_projects
                .iter()
                .filter_map(|i| mapping.get(i).copied())
                .collect();
            for saved in self.saved.values_mut() {
                remap_selection(&mut saved.selection, &mapping);
                saved.collapsed_projects = saved
                    .collapsed_projects
                    .iter()
                    .filter_map(|i| mapping.get(i).copied())
                    .collect();
            }
            self.menu = self.menu.and_then(|i| mapping.get(&i).copied());
            self.pending_remove = self.pending_remove.and_then(|i| mapping.get(&i).copied());
            self.pending_new_worktree = self
                .pending_new_worktree
                .and_then(|i| mapping.get(&i).copied());
            self.project_paths = paths;
        }
        let active = cx.global::<SettingsState>().store.workspaces.active;
        let switched = active != self.active_workspace;
        if self
            .project_panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).workspace != active)
        {
            self.project_panel = None;
            self.project_decision = false;
            self.project_return_focus = None;
            self.project_return_path = None;
            self.focus.focus(window, cx);
        }
        if active != self.active_workspace {
            self.saved.insert(
                self.active_workspace,
                Navigation {
                    selection: self.selection.take(),
                    mode: self.mode,
                    last_mode: self.last_mode,
                    collapsed_projects: std::mem::take(&mut self.collapsed_projects),
                    collapsed_worktrees: std::mem::take(&mut self.collapsed_worktrees),
                    scroll: self.scroll.clone(),
                    terminals_collapsed: self.terminals_collapsed,
                },
            );
            let next = self.saved.remove(&active).unwrap_or_default();
            self.selection = next.selection;
            self.mode = next.mode;
            self.last_mode = next.last_mode;
            self.collapsed_projects = next.collapsed_projects;
            self.collapsed_worktrees = next.collapsed_worktrees;
            self.scroll = next.scroll;
            self.terminals_collapsed = next.terminals_collapsed;
            self.active_workspace = active;
            self.menu = None;
            self.pending_close = None;
            self.pending_home_close = None;
            self.canvas_close_anchor = None;
            self.grid_drag = None;
            self.pending_remove = None;
            self.pending_new_worktree = None;
            self.content_error = None;
        }
        let existing: HashSet<_> = cx
            .global::<SettingsState>()
            .store
            .workspaces
            .rows
            .iter()
            .map(|w| w.id)
            .collect();
        for owner in self.terminal_owners.values_mut() {
            if !existing.contains(owner) {
                *owner = active;
            }
        }
        let rt = self.runtime.read(cx);
        let tree = rt.tree.clone();
        let proj = rt.state.read(cx).proj_idx();
        let ids: Vec<_> = cx
            .global::<SettingsState>()
            .store
            .workspace_projects(active)
            .map(|(i, _)| i)
            .collect();
        let generation = tree.read(cx).generation();
        if self.cache_warm != Some((active, generation)) {
            let targets = cx
                .global::<SettingsState>()
                .store
                .workspace_projects(active)
                .filter(|(i, _)| *i != proj)
                .map(|(i, p)| (i, p.path.clone()))
                .collect();
            tree.update(cx, |tree, cx| tree.sweep_wt_cache(targets, cx));
            self.cache_warm = Some((active, generation));
        }
        self.snapshot = self.runtime.update(cx, |r, cx| r.snapshot(cx));
        self.snapshot.projects.retain(|p| ids.contains(&p.idx));
        for project in &self.snapshot.projects {
            self.project_menu_bounds.entry(project.idx).or_default();
            self.project_menu_focus
                .entry(project.idx)
                .or_insert_with(|| cx.focus_handle());
        }
        for path in self
            .snapshot
            .projects
            .iter()
            .flat_map(|p| p.worktrees.iter().map(|w| w.path.clone()))
        {
            if !self.worktree_focus.contains_key(&path) {
                let handle = cx.focus_handle();
                self.observers
                    .push(cx.on_focus_in(&handle, window, |_, _, cx| cx.notify()));
                self.observers
                    .push(cx.on_focus_out(&handle, window, |_, _, _, cx| cx.notify()));
                self.worktree_focus.insert(path, handle);
            }
        }
        for path in self.snapshot.projects.iter().flat_map(|p| {
            p.worktrees
                .iter()
                .filter(|worktree| !worktree.is_main)
                .map(|worktree| worktree.path.clone())
        }) {
            self.worktree_delete_focus
                .entry(path)
                .or_insert_with(|| cx.focus_handle());
        }
        let registry = self.runtime.read(cx).registry.read(cx);
        let live: HashSet<_> = registry
            .all()
            .iter()
            .chain(registry.home_terminals().iter())
            .map(|meta| meta.id)
            .collect();
        self.canvas_bounds.retain(|id, _| live.contains(id));
        self.canvas_close_focus
            .retain(|(id, _), _| live.contains(id));
        self.canvas_observers.retain(|id, _| live.contains(id));
        self.canvas_focus_observers
            .retain(|id, _| live.contains(id));
        self.canvas_signatures.retain(|id, _| live.contains(id));
        self.terminal_views
            .retain(|id, _| registry.meta(*id).is_some());
        self.home_terminal_views
            .retain(|id, _| registry.home_terminals().iter().any(|m| m.id == *id));
        if self
            .selection
            .as_ref()
            .is_some_and(|selection| match selection {
                Selection::Session(id) => !self
                    .snapshot
                    .projects
                    .iter()
                    .any(|p| p.sessions.contains(id)),
                Selection::Home(id) => !registry.home_terminals().iter().any(|m| m.id == *id),
                Selection::Project(idx) | Selection::Worktree(idx, _) => !ids.contains(idx),
            })
        {
            self.selection = None;
        }
        if switched {
            self.runtime.read(cx).state.clone().update(cx, |state, cx| {
                state.clear_canvas_selection();
                cx.notify();
            });
            if let Some(selection) = self.selection.clone() {
                self.select(selection, cx);
            }
            cx.notify();
        }
    }
    pub(crate) fn active_canvas_sessions(&self, cx: &App) -> Vec<(SessionId, bool)> {
        let store = &cx.global::<SettingsState>().store;
        let workspace = store.workspaces.active;
        let projects: HashSet<_> = store
            .workspace_projects(workspace)
            .map(|(_, project)| project.name.as_str())
            .collect();
        let registry = self.runtime.read(cx).registry.read(cx);
        let mut sessions: Vec<_> = registry
            .all()
            .iter()
            .filter(|meta| projects.contains(meta.project.as_str()))
            .map(|meta| (meta.id, false))
            .collect();
        sessions.extend(
            registry
                .home_terminals()
                .iter()
                .filter(|meta| {
                    let owner = self.terminal_owners.get(&meta.id).copied().unwrap_or(1);
                    if store.workspaces.rows.iter().any(|row| row.id == owner) {
                        owner == workspace
                    } else {
                        true
                    }
                })
                .map(|meta| (meta.id, true)),
        );
        sessions
    }
    pub(super) fn request_canvas_close(
        &mut self,
        id: SessionId,
        home: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.canvas_close_anchor = Some(id);
        self.act(
            if home {
                Action::CloseHome(id)
            } else {
                Action::Close(id)
            },
            window,
            cx,
        );
    }
    fn focus_remaining_canvas(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((id, home)) = self.active_canvas_sessions(cx).first().copied() {
            self.select(
                if home {
                    Selection::Home(id)
                } else {
                    Selection::Session(id)
                },
                cx,
            );
            let view = if home {
                self.home_terminal_views.get(&id)
            } else {
                self.terminal_views.get(&id)
            };
            if let Some(view) = view {
                view.focus_handle(cx).focus(window, cx);
            } else {
                self.focus.focus(window, cx);
            }
        } else {
            self.focus.focus(window, cx);
        }
    }
    pub(super) fn canvas_confirmation(
        &self,
        id: SessionId,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.canvas_close_anchor != Some(id) || !self.confirmation_open() {
            return None;
        }
        let bounds = self.canvas_bounds.get(&id)?.get();
        let registry = self.runtime.read(cx).registry.read(cx);
        let (message, action) = if self.pending_home_close == Some(id) {
            let meta = registry
                .home_terminals()
                .iter()
                .find(|meta| meta.id == id)?;
            (
                format!(
                    "Close {}? Its shell and running commands will stop. Files remain on disk.",
                    meta.label
                ),
                Action::ConfirmHome(id),
            )
        } else {
            let meta = registry.meta(id)?;
            (
                format!(
                    "Close {} in {}? Its process will stop. The worktree remains at {}.",
                    meta.label, meta.project, meta.wt_path
                ),
                Action::ConfirmClose(id),
            )
        };
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width = (f32::from(bounds.size.width) / scale)
            .clamp(SIDEBAR_W, MODAL_W_SM)
            .min(f32::from(window.viewport_size().width) / scale - SPACE_LG * 2.0);
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position_mode(gpui::AnchoredPositionMode::Window)
                    .position(gpui::point(bounds.left(), bounds.bottom()))
                    .snap_to_window_with_margin(gpui::px(SPACE_LG * scale))
                    .child(
                        div()
                            .id("canvas-confirmation")
                            .debug_selector(|| "canvas-confirmation".into())
                            .w(rpx(width))
                            .occlude()
                            .child(self.confirmation(&message, action, cx)),
                    ),
            )
            .into_any_element(),
        )
    }
    pub fn confirmation_open(&self) -> bool {
        self.project_decision
            || self.pending_close.is_some()
            || self.pending_home_close.is_some()
            || self.pending_remove.is_some()
            || self.pending_worktree_removal.is_some()
    }
    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        if let Some(focus) = self.menu_return_focus.take() {
            cx.defer_in(window, move |_, window, cx| focus.focus(window, cx));
        }
        cx.notify();
    }
    pub(crate) fn rail_width(window: &Window) -> f32 {
        let logical_width = f32::from(window.viewport_size().width)
            / (f32::from(window.rem_size()) / crate::zoom::REM_BASE);
        SIDEBAR_W.min(logical_width * SIDEBAR_MAX_VIEWPORT_FRACTION)
    }
    pub fn is_grid(&self) -> bool {
        self.mode == ViewMode::Grid
    }
    pub fn view_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let target = if self.mode == ViewMode::Grid {
            self.last_mode
        } else if self.mode == ViewMode::Project {
            ViewMode::List
        } else {
            ViewMode::Project
        };
        div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_SM))
            .child(
                self.control(
                    "sidebar-view",
                    if target == ViewMode::List {
                        "Switch to list view"
                    } else {
                        "Switch to project view"
                    },
                    Action::Mode(target),
                    cx,
                )
                .child(icon(
                    if target == ViewMode::List {
                        "list"
                    } else {
                        "folder"
                    },
                    ICON_SM,
                    c::FG_DIM(),
                )),
            )
            .child(
                self.control(
                    "sidebar-grid",
                    "Open grid view",
                    Action::Mode(if self.is_grid() {
                        self.last_mode
                    } else {
                        ViewMode::Grid
                    }),
                    cx,
                )
                .debug_selector(|| "sidebar-grid".into())
                .when(self.is_grid(), |d| d.bg(c::BG_HOVER()))
                .child(icon("grid", ICON_SM, c::FG_DIM())),
            )
            .child(
                super::components::header_control("sidebar-settings", "Settings coming soon")
                    .debug_selector(|| "sidebar-settings".into())
                    .role(gpui::Role::Image)
                    .aria_description("Settings is currently unavailable")
                    .tab_index(-1)
                    .opacity(OPACITY_DISABLED)
                    .child(icon("cog", ICON_MD, c::FG_DIM())),
            )
            .into_any_element()
    }
    fn control(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        action: Action,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let label = label.into();
        let click = action.clone();
        let danger = matches!(action, Action::ConfirmWorktreeRemoval);
        let primary = matches!(action, Action::DismissWorktreeRemoval);
        let confirm = matches!(
            action,
            Action::ConfirmClose(_) | Action::ConfirmHome(_) | Action::ConfirmRemove(_)
        );
        let cancel = matches!(action, Action::Cancel);
        let confirming = self.project_decision
            || self.pending_close.is_some()
            || self.pending_home_close.is_some()
            || self.pending_remove.is_some()
            || self.pending_worktree_removal.is_some();
        let decision = matches!(
            action,
            Action::ConfirmClose(_)
                | Action::ConfirmHome(_)
                | Action::ConfirmRemove(_)
                | Action::ConfirmWorktreeRemoval
                | Action::SkipWorktreeTeardown
                | Action::DismissWorktreeRemoval
                | Action::Cancel
        );
        div()
            .id(id)
            .role(gpui::Role::Button)
            .aria_label(label.clone())
            .tab_index(if confirming && !decision { -1 } else { 0 })
            .when(confirm, |d| d.track_focus(&self.confirm_focus))
            .when(cancel, |d| d.track_focus(&self.cancel_focus))
            .size(rpx(CHROME_CONTROL_H))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(rpx(RADIUS_CONTROL))
            .hover(move |s| {
                if danger {
                    s.bg(c::RED_WASH()).text_color(c::RED())
                } else if primary {
                    s.bg(c::FG_DIM()).text_color(c::BG())
                } else {
                    s.bg(c::BG_HOVER())
                }
            })
            .focus_visible(move |s| {
                if danger {
                    s.bg(c::RED_WASH()).text_color(c::RED())
                } else if primary {
                    s.bg(c::FG_DIM()).text_color(c::BG())
                } else {
                    s.bg(c::BG_HOVER())
                }
            })
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(label.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.act(click.clone(), window, cx);
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        this.act(action.clone(), window, cx);
                    }
                }),
            )
    }
    fn row(
        &self,
        id: String,
        label: String,
        selected: bool,
        action: Action,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        self.control(SharedString::from(id.clone()), label, action, cx)
            .debug_selector(move || id.clone())
            .w_full()
            .h(rpx(ROW_H))
            .justify_start()
            .px(rpx(SPACE_SM))
            .gap(rpx(SPACE_MD))
            .rounded(rpx(RADIUS_CHROME))
            .when(selected, |d| d.bg(c::BG_HOVER()).text_color(c::FG()))
    }
    fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        self.runtime
            .read(cx)
            .state
            .clone()
            .update(cx, |state, _| state.clear_canvas_selection());
        if let Selection::Session(id) = selection {
            let snap = self.snapshot.clone();
            self.runtime.read(cx).state.clone().update(cx, |s, cx| {
                s.select_session(id, &snap);
                cx.notify();
            });
        }
        if let Selection::Home(id) = selection {
            let registry = self.runtime.read(cx).registry.read(cx);
            let index = registry.home_terminals().iter().position(|m| m.id == id);
            let count = registry.home_terminal_count();
            if let Some(index) = index {
                self.runtime.read(cx).state.clone().update(cx, |s, cx| {
                    s.select_home_terminal(index, count);
                    cx.notify();
                });
            }
        }
        self.selection = Some(selection);
        self.pending_new_worktree = None;
        cx.notify();
    }
    fn launch(&mut self, project: usize, path: String, agent: Agent, cx: &mut Context<Self>) {
        let Some(name) = self
            .snapshot
            .projects
            .iter()
            .find(|p| p.idx == project)
            .map(|p| p.name.clone())
        else {
            return;
        };
        self.runtime
            .update(cx, |r, cx| r.spawn_session_in(name, path, agent, cx));
        if let Some(id) = self.runtime.read(cx).state.read(cx).active_session() {
            self.selection = Some(Selection::Session(id));
        }
        cx.notify();
    }
    fn act(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(removal) = &self.pending_worktree_removal {
            let finished = removal.started
                && (removal.error.is_some()
                    || self
                        .runtime
                        .read(cx)
                        .projects
                        .read(cx)
                        .worktree_removal_status(&removal.path)
                        .is_some_and(|status| status.stage == WorktreeRemovalStage::Finished));
            let allowed = match action {
                Action::ConfirmWorktreeRemoval => !removal.started,
                Action::SkipWorktreeTeardown => removal.started && !finished,
                Action::DismissWorktreeRemoval | Action::Cancel => !removal.started || finished,
                _ => false,
            };
            if !allowed {
                return;
            }
        }
        if (self.project_decision
            || self.pending_close.is_some()
            || self.pending_home_close.is_some()
            || self.pending_remove.is_some())
            && !matches!(
                action,
                Action::ConfirmClose(_)
                    | Action::ConfirmHome(_)
                    | Action::ConfirmRemove(_)
                    | Action::Cancel
            )
        {
            return;
        }
        match action {
            Action::AddProject => self.add_project(window, cx),
            Action::ArchivedProjects => {
                self.open_project_panel(projects::Page::Archived, window, cx);
            }
            Action::EditProject(path) => {
                if !self.project_path_is_active(&path, cx) {
                    return;
                }
                self.open_project_panel(projects::Page::Edit(path), window, cx);
            }
            Action::Select(s) => {
                self.select(s.clone(), cx);
                let view = match s {
                    Selection::Session(id) => self.terminal_views.get(&id),
                    Selection::Home(id) => self.home_terminal_views.get(&id),
                    _ => None,
                };
                if let Some(view) = view {
                    view.focus_handle(cx).focus(window, cx);
                }
            }
            Action::Mode(mode) => {
                change_mode(&mut self.mode, &mut self.last_mode, mode);
                self.menu = None;
            }
            Action::Project(i) => {
                if !self.collapsed_projects.remove(&i) {
                    self.collapsed_projects.insert(i);
                }
            }
            Action::Worktree(path) => {
                if !self.collapsed_worktrees.remove(&path) {
                    self.collapsed_worktrees.insert(path);
                }
            }
            Action::Menu(i) => {
                if self.menu == Some(i) {
                    self.close_menu(window, cx);
                } else {
                    self.menu = Some(i);
                    if let Some(bounds) = self.project_menu_bounds.get(&i) {
                        self.menu_trigger_bounds = bounds.clone();
                    }
                    self.menu_index = 0;
                    self.menu_return_focus = self.project_menu_focus.get(&i).cloned();
                    self.menu_focus.focus(window, cx);
                }
            }
            Action::NewWorktree(path) => {
                if !self.project_path_is_active(&path, cx) {
                    return;
                }
                let Some(i) = cx
                    .global::<SettingsState>()
                    .store
                    .projects
                    .iter()
                    .position(|p| p.path == path)
                else {
                    return;
                };
                self.menu = None;
                self.worktree_return_focus =
                    self.menu_return_focus.take().or_else(|| window.focused(cx));
                self.begin_worktree(i, window, cx);
            }
            Action::Reveal(path) => {
                cx.reveal_path(std::path::Path::new(&path));
                self.close_menu(window, cx);
            }
            Action::RemoveProject(path) => {
                if !self.project_path_is_active(&path, cx) {
                    return;
                }
                self.open_project_panel(projects::Page::Remove(path), window, cx);
            }
            Action::ConfirmRemove(i) => {
                self.runtime
                    .read(cx)
                    .projects
                    .clone()
                    .update(cx, |p, cx| p.remove_project(i, false, cx));
                self.pending_remove = None;
            }
            Action::RemoveWorktree(idx, path) => {
                let Some(project) = self.snapshot.projects.iter().find(|p| p.idx == idx) else {
                    return;
                };
                let Some(worktree) = project
                    .worktrees
                    .iter()
                    .find(|worktree| worktree.path == path && !worktree.is_main)
                else {
                    return;
                };
                let project_path = cx
                    .global::<SettingsState>()
                    .store
                    .projects
                    .get(idx)
                    .map(|project| project.path.clone());
                let Some(project_path) = project_path else {
                    return;
                };
                self.pending_worktree_removal = Some(PendingWorktreeRemoval {
                    project_path,
                    path: path.clone(),
                    name: worktree.name.clone(),
                    started: false,
                    error: None,
                });
                self.worktree_removal_return_focus = self.worktree_delete_focus.get(&path).cloned();
                self.mode = ViewMode::Project;
                self.menu = None;
                self.worktree_removal_focus.focus(window, cx);
            }
            Action::ConfirmWorktreeRemoval => {
                let Some(removal) = self.pending_worktree_removal.as_mut() else {
                    return;
                };
                if removal.started {
                    return;
                }
                removal.started = true;
                let (project_path, path) = (removal.project_path.clone(), removal.path.clone());
                let service = self.runtime.read(cx).projects.clone();
                if let Err(error) = service.update(cx, |service, cx| {
                    service.remove_worktree_by_path(&project_path, &path, cx)
                }) {
                    if let Some(removal) = self.pending_worktree_removal.as_mut() {
                        removal.error = Some(error);
                    }
                }
                self.worktree_removal_focus.focus(window, cx);
            }
            Action::SkipWorktreeTeardown => {
                if let Some(removal) = &self.pending_worktree_removal {
                    if self
                        .runtime
                        .read(cx)
                        .projects
                        .read(cx)
                        .worktree_removal_status(&removal.path)
                        .is_some_and(|status| status.stage == WorktreeRemovalStage::RunningScript)
                    {
                        self.runtime.read(cx).projects.clone().update(
                            cx,
                            crate::project_service::ProjectService::skip_worktree_teardown,
                        );
                        self.worktree_removal_focus.focus(window, cx);
                    }
                }
            }
            Action::DismissWorktreeRemoval => {
                self.pending_worktree_removal = None;
                if let Some(focus) = self.worktree_removal_return_focus.take() {
                    focus.focus(window, cx);
                } else {
                    self.focus.focus(window, cx);
                }
            }
            Action::Launch(i, path, agent) => self.launch(i, path, agent, cx),
            Action::Close(id) => {
                self.pending_close = Some(id);
                self.confirmation_return_focus = window.focused(cx);
                self.cancel_focus.focus(window, cx);
            }
            Action::ConfirmClose(id) => {
                let from_canvas = self.canvas_close_anchor.take().is_some();
                self.runtime.update(cx, |r, cx| r.kill_session(id, cx));
                self.pending_close = None;
                if from_canvas {
                    self.focus_remaining_canvas(window, cx);
                }
            }
            Action::Retry(id) => {
                let meta = self.runtime.read(cx).registry.read(cx).meta(id).cloned();
                if let Some(meta) = meta {
                    let extra_roots: Vec<_> = meta
                        .context_roots
                        .iter()
                        .filter(|root| root.wt_path != meta.wt_path)
                        .map(|root| root.wt_path.clone())
                        .collect();
                    let store = &cx.global::<SettingsState>().store;
                    let args = meta
                        .agent
                        .multi_root_launch_args(
                            store.dangerously_skip_permissions_enabled.unwrap_or(false),
                            store.chrome_enabled.unwrap_or(false),
                            &extra_roots,
                        )
                        .unwrap_or_else(|| {
                            meta.agent.launch_args(
                                store.dangerously_skip_permissions_enabled.unwrap_or(false),
                                store.chrome_enabled.unwrap_or(false),
                            )
                        });
                    let bundle = if !extra_roots.is_empty()
                        && matches!(meta.agent, Agent::OpenCode | Agent::Terminal)
                    {
                        match grove_core::multi_root::SymlinkBundle::create(&extra_roots) {
                            Ok(bundle) => Some(bundle.into_path().to_string_lossy().into_owned()),
                            Err(error) => {
                                self.content_error =
                                    Some(format!("Could not prepare session context: {error}"));
                                cx.notify();
                                return;
                            }
                        }
                    } else {
                        None
                    };
                    self.runtime.update(cx, |r, cx| {
                        r.kill_session(id, cx);
                        r.spawn_session_in_with_context(
                            meta.project,
                            meta.wt_path,
                            meta.agent,
                            args,
                            meta.context_roots,
                            bundle,
                            cx,
                        );
                    });
                    if let Some(id) = self.runtime.read(cx).state.read(cx).active_session() {
                        self.selection = Some(Selection::Session(id));
                    }
                }
            }
            Action::AddTerminal => {
                if self.mode == ViewMode::Grid {
                    self.mode = self.last_mode;
                }
                self.runtime.update(cx, Runtime::new_home_terminal);
                if let Some(meta) = self
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .home_terminals()
                    .last()
                {
                    self.terminal_owners.insert(meta.id, self.active_workspace);
                    self.selection = Some(Selection::Home(meta.id));
                }
                self.terminals_collapsed = false;
            }
            Action::FoldTerminals => self.terminals_collapsed = !self.terminals_collapsed,
            Action::CloseHome(id) => {
                self.pending_home_close = Some(id);
                self.confirmation_return_focus = window.focused(cx);
                self.cancel_focus.focus(window, cx);
            }
            Action::ConfirmHome(id) => {
                let from_canvas = self.canvas_close_anchor.take().is_some();
                let index = self
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .home_terminals()
                    .iter()
                    .position(|m| m.id == id);
                if let Some(i) = index {
                    self.runtime
                        .update(cx, |r, cx| r.close_home_terminal(i, cx));
                }
                self.pending_home_close = None;
                self.terminal_owners.remove(&id);
                if from_canvas {
                    self.focus_remaining_canvas(window, cx);
                }
            }
            Action::Cancel => {
                if self.pending_worktree_removal.take().is_some() {
                    if let Some(focus) = self.worktree_removal_return_focus.take() {
                        focus.focus(window, cx);
                    } else {
                        self.focus.focus(window, cx);
                    }
                    cx.notify();
                    return;
                }
                self.canvas_close_anchor = None;
                if self.pending_new_worktree.take().is_some() {
                    self.content_error = None;
                    self.worktree_errors = [None, None, None];
                    if let Some(focus) = self.worktree_return_focus.take() {
                        focus.focus(window, cx);
                    }
                }
                if self.menu.is_some() {
                    self.close_menu(window, cx);
                }
                if let Some(focus) = self.confirmation_return_focus.take() {
                    focus.focus(window, cx);
                }
                self.pending_close = None;
                self.pending_home_close = None;
                self.pending_remove = None;
                self.menu = None;
            }
        }
        cx.notify();
    }
    fn worktree_removal_changed(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(removal) = &self.pending_worktree_removal else {
            return;
        };
        if removal.path != path {
            return;
        }
        let project_path = removal.project_path.clone();
        let finished_ok = self
            .runtime
            .read(cx)
            .projects
            .read(cx)
            .worktree_removal_status(path)
            .is_some_and(|status| {
                status.stage == WorktreeRemovalStage::Finished && status.error.is_none()
            });
        if finished_ok {
            if self.project_path_is_active(&project_path, cx) {
                self.select_project_path(&project_path, cx);
            }
            self.worktree_removal_return_focus = None;
        }
        cx.notify();
    }
    fn status(&self, meta: &SessionMeta, cx: &App) -> (&'static str, gpui::Hsla) {
        let runtime = self.runtime.read(cx);
        let registry = runtime.registry.read(cx);
        if registry
            .session(meta.id)
            .is_some_and(|t| t.read(cx).spawn_error().is_some())
        {
            return ("Failed", c::RED());
        }
        if registry
            .session(meta.id)
            .is_some_and(|t| t.read(cx).is_pending_attach())
        {
            return ("Starting", c::FG_DIM());
        }
        match runtime.activity.read(cx).state_of(meta.id) {
            ActivityState::WaitingForInput => ("Needs you", c::YELLOW()),
            ActivityState::Working => ("Working", c::GREEN()),
            ActivityState::Done => ("Done", c::FG_DIM()),
            ActivityState::Idle => ("Idle", c::FG_DIM()),
            ActivityState::Exited => ("Exited", c::FG_DIM()),
        }
    }
    fn confirmation(&self, label: &str, action: Action, cx: &mut Context<Self>) -> AnyElement {
        let verb = match &action {
            Action::ConfirmClose(_) => "Close session",
            Action::ConfirmHome(_) => "Close terminal",
            Action::ConfirmRemove(_) => "Remove project",
            _ => "Confirm",
        };
        div()
            .id("sidebar-confirmation")
            .role(gpui::Role::Dialog)
            .aria_label(verb)
            .aria_description(label.to_string())
            .text_color(c::FG())
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL))
            .p(rpx(SPACE_3XL))
            .m(rpx(SPACE_SM))
            .rounded(rpx(RADIUS_GROUP))
            .min_w_0()
            .whitespace_normal()
            .border_1()
            .border_color(c::BORDER())
            .bg(c::SURFACE_RAISED())
            .child(label.replace('/', "/\u{200b}"))
            .child(
                div()
                    .flex()
                    .when(self.compact_rail, gpui::Styled::flex_col)
                    .min_w_0()
                    .flex_wrap()
                    .mt(rpx(SPACE_SM))
                    .gap(rpx(SPACE_LG))
                    .child(
                        self.control("confirm-close", verb, action, cx)
                            .w_auto()
                            .px(rpx(SPACE_LG))
                            .text_color(c::RED())
                            .child(verb),
                    )
                    .child(
                        self.control("cancel-close", "Cancel", Action::Cancel, cx)
                            .w_auto()
                            .px(rpx(SPACE_LG))
                            .child("Cancel"),
                    ),
            )
            .into_any_element()
    }
    fn session_row(&self, meta: &SessionMeta, list: bool, cx: &mut Context<Self>) -> AnyElement {
        let (status, color) = self.status(meta, cx);
        let id = meta.id;
        let label = if list {
            format!("{} · {}", meta.label, meta.project)
        } else {
            meta.label.clone()
        };
        let row = if list {
            let worktree = self
                .snapshot
                .projects
                .iter()
                .flat_map(|project| project.worktrees.iter())
                .find(|worktree| worktree.path == meta.wt_path);
            let name = worktree.map_or_else(
                || {
                    std::path::Path::new(&meta.wt_path).file_name().map_or_else(
                        || meta.wt_path.clone(),
                        |name| name.to_string_lossy().into_owned(),
                    )
                },
                |worktree| worktree.name.clone(),
            );
            let branch = worktree
                .map(|worktree| worktree.branch.trim())
                .filter(|branch| {
                    !branch.is_empty()
                        && !matches!(*branch, "—" | "-")
                        && *branch != name
                        && *branch != meta.project
                });
            let context = if name == meta.project {
                name.clone()
            } else {
                format!("{} · {}", meta.project, name)
            };
            let title = display_task_title(
                self.runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .session(id)
                    .and_then(|session| session.read(cx).title()),
                &meta.label,
            );
            self.row(
                format!("session-{}", id.raw()),
                format!("{title} · {name} · {context} · {status}"),
                self.selection == Some(Selection::Session(id)),
                Action::Select(Selection::Session(id)),
                cx,
            )
            .h_auto()
            .min_h(rpx(ROW_H))
            .p(rpx(SPACE_2XL))
            .items_start()
            .child(
                div()
                    .h(rpx(CONTROL_H))
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .child(icon(meta.agent.icon_name(), ICON_SM, color)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(rpx(ROW_LINE_GAP))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .min_w_0()
                            .child(div().flex_1().min_w_0().truncate().child(title))
                            .child(
                                self.control(
                                    ("close-session", id.raw()),
                                    format!("Close {} in {}", meta.label, meta.project),
                                    Action::Close(id),
                                    cx,
                                )
                                .child(icon(
                                    "close",
                                    ICON_XS,
                                    c::FG_DIM(),
                                )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_MD))
                            .text_size(rpx(TEXT_SMALL))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(c::FG_DIM())
                                    .font_family(crate::fonts::MONO_FAMILY)
                                    .child(context),
                            )
                            .child(div().flex_shrink_0().text_color(color).child(status)),
                    )
                    .when_some(branch, |column, branch| {
                        column.child(
                            div()
                                .truncate()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(branch.to_string()),
                        )
                    }),
            )
        } else {
            self.row(
                format!("session-{}", id.raw()),
                format!("{label} · {status}"),
                self.selection == Some(Selection::Session(id)),
                Action::Select(Selection::Session(id)),
                cx,
            )
            .h_auto()
            .min_h(rpx(ROW_H))
            .pl(rpx(if list || self.compact_rail {
                SPACE_LG
            } else {
                SPACE_3XL * 2.0
            }))
            .child(div().size(rpx(DOT_SM)).rounded_full().bg(color))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when(!self.compact_rail, |row| {
                row.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(color)
                        .child(status),
                )
            })
            .child(
                self.control(
                    ("close-session", id.raw()),
                    format!("Close {} in {}", meta.label, meta.project),
                    Action::Close(id),
                    cx,
                )
                .child(icon("close", ICON_XS, c::FG_DIM())),
            )
        };
        let mut result = div().child(row);
        if status == "Failed" {
            result = result.child(
                self.control(
                    ("retry-session", id.raw()),
                    "Retry session",
                    Action::Retry(id),
                    cx,
                )
                .w_auto()
                .ml(rpx(SPACE_3XL))
                .child("Retry"),
            );
        }
        if self.pending_close == Some(id) && self.canvas_close_anchor.is_none() {
            result = result.child(self.confirmation(
                &format!(
                    "Close {} in {}? Its process will stop. The worktree remains at {}.",
                    meta.label, meta.project, meta.wt_path
                ),
                Action::ConfirmClose(id),
                cx,
            ));
        }
        result.into_any_element()
    }
    fn project_popup(&self, idx: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(project) = cx.global::<SettingsState>().store.projects.get(idx) else {
            return div().into_any_element();
        };
        let path = project.path.clone();
        let keyboard_path = path.clone();
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width = SIDEBAR_W.min(f32::from(window.viewport_size().width) / scale - SPACE_LG * 2.0);
        let mut panel = div()
            .text_color(c::FG())
            .id("project-actions-popup")
            .debug_selector(|| "project-actions-popup".into())
            .role(gpui::Role::Menu)
            .aria_label(format!(
                "Actions for {}",
                cx.global::<SettingsState>().store.projects[idx].name
            ))
            .track_focus(&self.menu_focus)
            .w(rpx(width))
            .p(rpx(SPACE_SM))
            .rounded(rpx(RADIUS_CHROME))
            .border_1()
            .border_color(c::BORDER())
            .bg(c::SURFACE_RAISED())
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if !this.menu_trigger_bounds.get().contains(&event.position) {
                        this.close_menu(window, cx);
                    }
                }),
            )
            .capture_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        "down" | "up" | "tab" => {
                            window.prevent_default();
                            let backward = event.keystroke.key == "up"
                                || (event.keystroke.key == "tab"
                                    && event.keystroke.modifiers.shift);
                            this.menu_index = if backward {
                                (this.menu_index + 3) % 4
                            } else {
                                (this.menu_index + 1) % 4
                            };
                            this.menu_focus.focus(window, cx);
                            cx.notify();
                            cx.stop_propagation();
                        }
                        "enter" | "space" => {
                            let action = match this.menu_index {
                                0 => Action::EditProject(keyboard_path.clone()),
                                1 => Action::NewWorktree(keyboard_path.clone()),
                                2 => Action::Reveal(keyboard_path.clone()),
                                _ => Action::RemoveProject(keyboard_path.clone()),
                            };
                            this.act(action, window, cx);
                            cx.stop_propagation();
                        }
                        "escape" => {
                            this.close_menu(window, cx);
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }),
            );
        for (index, (label, glyph, action)) in [
            ("Edit project", "edit", Action::EditProject(path.clone())),
            ("New worktree", "plus", Action::NewWorktree(path.clone())),
            (
                "Open in file manager",
                "folder",
                Action::Reveal(path.clone()),
            ),
            (
                "Remove project",
                "close",
                Action::RemoveProject(path.clone()),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            panel = panel.child(
                self.control(("project-action", index), label, action, cx)
                    .role(gpui::Role::MenuItem)
                    .w_full()
                    .h(rpx(ROW_H))
                    .justify_start()
                    .px(rpx(SPACE_LG))
                    .gap(rpx(SPACE_LG))
                    .when(index == self.menu_index, |d| d.bg(c::BG_HOVER()))
                    .child(icon(glyph, ICON_SM, c::FG_DIM()))
                    .child(label),
            );
        }
        let trigger = self.menu_trigger_bounds.get();
        gpui::anchored()
            .position_mode(gpui::AnchoredPositionMode::Window)
            .position(gpui::point(trigger.right(), trigger.top()))
            .snap_to_window_with_margin(gpui::px(SPACE_LG * scale))
            .child(panel)
            .into_any_element()
    }
    fn tree(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().flex().flex_col().gap(rpx(SPACE_XS));
        for (project_position, project) in self.snapshot.projects.iter().enumerate() {
            let idx = project.idx;
            let closed = self.collapsed_projects.contains(&idx);
            let rollup = closed
                .then(|| {
                    project_activity_rollup(project.sessions.iter().filter_map(|id| {
                        self.runtime
                            .read(cx)
                            .registry
                            .read(cx)
                            .meta(*id)
                            .map(|meta| self.status(meta, cx))
                    }))
                })
                .flatten();
            body = body.child(
                self.row(
                    format!("project-{idx}"),
                    project.name.clone(),
                    self.selection == Some(Selection::Project(idx)),
                    Action::Select(Selection::Project(idx)),
                    cx,
                )
                .when(project_position > 0, |row| row.mt(rpx(SPACE_2XL)))
                .px(rpx(SPACE_LG))
                .gap(rpx(SPACE_LG))
                .text_size(rpx(TEXT_TITLE))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(c::alpha(c::FG(), 0.88))
                .child(
                    self.control(
                        ("fold-project", idx),
                        if closed {
                            "Expand project"
                        } else {
                            "Collapse project"
                        },
                        Action::Project(idx),
                        cx,
                    )
                    .child(
                        div()
                            .relative()
                            .size(rpx(ICON_MD))
                            .child(icon(
                                if closed { "folder" } else { "folder-open" },
                                ICON_MD,
                                c::FG_DIM(),
                            ))
                            .when_some(rollup, |folder, (status, color)| {
                                folder.child(
                                    div()
                                        .id(("project-activity", idx))
                                        .debug_selector(move || format!("project-activity-{idx}"))
                                        .role(gpui::Role::Image)
                                        .aria_label(format!("{}: {status}", project.name))
                                        .tooltip(move |window, cx| {
                                            gpui_component::tooltip::Tooltip::new(status)
                                                .build(window, cx)
                                        })
                                        .absolute()
                                        .bottom_0()
                                        .right_0()
                                        .size(rpx(DOT_SM))
                                        .rounded_full()
                                        .bg(color),
                                )
                            }),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_SM))
                        .text_color(c::alpha(c::FG(), 0.88))
                        .child(
                            div()
                                .id(("project-title", idx))
                                .debug_selector(move || format!("project-title-{idx}"))
                                .min_w_0()
                                .truncate()
                                .child(project.name.clone()),
                        )
                        .when(!project.is_git, |name| {
                            name.child(
                                div()
                                    .id(("project-no-git", idx))
                                    .debug_selector(move || format!("project-no-git-{idx}"))
                                    .role(gpui::Role::Image)
                                    .aria_label("Not a Git repository")
                                    .size(rpx(ICON_MD))
                                    .flex_shrink_0()
                                    .tooltip(|window, cx| {
                                        gpui_component::tooltip::Tooltip::new(
                                            "Not a Git repository",
                                        )
                                        .build(window, cx)
                                    })
                                    .child(icon("no-git", ICON_MD, c::YELLOW())),
                            )
                        }),
                )
                .child(
                    div()
                        .id(("project-count", idx))
                        .debug_selector(move || format!("project-count-{idx}"))
                        .w(rpx(CHROME_CONTROL_H))
                        .min_w_0()
                        .flex_shrink_0()
                        .text_right()
                        .font_family(crate::fonts::MONO_FAMILY)
                        .font_weight(gpui::FontWeight::NORMAL)
                        .when(self.compact_rail, |style| style.w(rpx(0.0)))
                        .overflow_hidden()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(format!("{}", project.sessions.len())),
                )
                .child(
                    self.control(
                        ("project-menu", idx),
                        format!("Actions for {}", project.name),
                        Action::Menu(idx),
                        cx,
                    )
                    .relative()
                    .when_some(
                        self.project_menu_focus.get(&idx),
                        gpui::InteractiveElement::track_focus,
                    )
                    .child(icon("more", ICON_SM, c::FG_DIM()))
                    .debug_selector(move || format!("project-menu-{idx}"))
                    .when_some(
                        self.project_menu_bounds.get(&idx).cloned(),
                        |button, bounds| {
                            button.child(
                                gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                                    .absolute()
                                    .inset_0(),
                            )
                        },
                    )
                    .when(self.menu == Some(idx), |button| {
                        button.child(gpui::deferred(self.project_popup(idx, window, cx)))
                    }),
                ),
            );
            if self.pending_remove == Some(idx) {
                body = body.child(self.confirmation(
                    &format!("Remove {} from Grove? Its sessions will stop. The repository and worktrees remain on disk.",project.name),
                    Action::ConfirmRemove(idx),
                    cx,
                ));
            }
            if closed {
                continue;
            }
            if project.worktrees.is_empty() {
                body = body.child(
                    div()
                        .pl(rpx(SPACE_3XL))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("Loading worktrees…"),
                );
            }
            for worktree in &project.worktrees {
                let path = worktree.path.clone();
                let collapsed = self.collapsed_worktrees.contains(&path);
                let selection = Selection::Worktree(idx, path.clone());
                let focused = self
                    .worktree_focus
                    .get(&path)
                    .is_some_and(|f| f.contains_focused(window, cx));
                let mut launches = div()
                    .flex()
                    .flex_shrink_0()
                    .when(self.compact_rail, |d| d.bg(c::BG_RAIL()))
                    .opacity(if focused { 1.0 } else { 0.0 })
                    .group_hover("worktree-row", |s| s.opacity(1.0));
                for (n, agent) in [Agent::Codex, Agent::Claude, Agent::Terminal]
                    .into_iter()
                    .enumerate()
                {
                    let label = if self.available[n] {
                        format!(
                            "Start {} in {} · {}",
                            agent.label(),
                            project.name,
                            worktree.name
                        )
                    } else {
                        format!("{} is not installed", agent.label())
                    };
                    let action = if self.available[n] {
                        Action::Launch(idx, path.clone(), agent)
                    } else {
                        Action::Cancel
                    };
                    launches = launches.child(
                        self.control(
                            SharedString::from(format!("launch-{path}-{n}")),
                            label,
                            action,
                            cx,
                        )
                        .when(!self.available[n], |d| d.opacity(OPACITY_DISABLED))
                        .child(icon(
                            agent.icon_name(),
                            ICON_SM,
                            c::FG_DIM(),
                        )),
                    );
                }
                if !worktree.is_main {
                    launches = launches.child(
                        self.control(
                            SharedString::from(format!("delete-worktree-{path}")),
                            format!("Delete worktree {}", worktree.name),
                            Action::RemoveWorktree(idx, path.clone()),
                            cx,
                        )
                        .debug_selector({
                            let path = path.clone();
                            move || format!("delete-worktree-{path}")
                        })
                        .when_some(
                            self.worktree_delete_focus.get(&path),
                            gpui::InteractiveElement::track_focus,
                        )
                        .child(icon("trash", ICON_SM, c::FG_DIM())),
                    );
                }
                body = body.child(
                    self.row(
                        format!("worktree-{path}"),
                        format!("{} · {}", worktree.name, worktree.branch),
                        self.selection == Some(selection.clone()),
                        Action::Select(selection),
                        cx,
                    )
                    .relative()
                    .group("worktree-row")
                    .when_some(
                        self.worktree_focus.get(&path),
                        gpui::InteractiveElement::track_focus,
                    )
                    .h(rpx(ROW_H))
                    .px(rpx(SPACE_LG))
                    .gap(rpx(SPACE_LG))
                    .text_size(rpx(TEXT_BODY))
                    .text_color(c::alpha(c::FG(), 0.88))
                    .child(
                        self.control(
                            SharedString::from(format!("fold-{path}")),
                            if collapsed {
                                "Expand worktree"
                            } else {
                                "Collapse worktree"
                            },
                            Action::Worktree(path.clone()),
                            cx,
                        )
                        .child(icon(
                            if collapsed { "chev-right" } else { "chev-down" },
                            ICON_XS,
                            c::FG_DIM(),
                        )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .id(SharedString::from(format!("worktree-title-{path}")))
                            .debug_selector({
                                let path = path.clone();
                                move || format!("worktree-title-{path}")
                            })
                            .child(worktree.name.clone()),
                    )
                    .child(
                        div()
                            .relative()
                            .w(rpx(
                                CHROME_CONTROL_H * if worktree.is_main { 3.0 } else { 4.0 }
                            ))
                            .when(self.compact_rail, |d| d.absolute().right_0())
                            .h(rpx(CHROME_CONTROL_H))
                            .flex_shrink_0()
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .pr(rpx(CHROME_CONTROL_H + SPACE_LG))
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .opacity(if focused || self.compact_rail {
                                        0.0
                                    } else {
                                        1.0
                                    })
                                    .group_hover("worktree-row", |s| s.opacity(0.0))
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "worktree-count-{path}"
                                            )))
                                            .debug_selector({
                                                let path = path.clone();
                                                move || format!("worktree-count-{path}")
                                            })
                                            .w(rpx(CHROME_CONTROL_H))
                                            .font_family(crate::fonts::MONO_FAMILY)
                                            .font_weight(gpui::FontWeight::NORMAL)
                                            .text_right()
                                            .child(format!("{}", worktree.sessions.len())),
                                    ),
                            )
                            .child(launches),
                    ),
                );
                if !collapsed {
                    if worktree.sessions.is_empty() {
                        body = body.child(
                            div()
                                .id(SharedString::from(format!("worktree-empty-{path}")))
                                .pl(rpx(SPACE_LG + CHROME_CONTROL_H + SPACE_LG))
                                .py(rpx(SPACE_SM))
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "worktree-empty-text-{path}"
                                        )))
                                        .debug_selector({
                                            let path = path.clone();
                                            move || format!("worktree-empty-{path}")
                                        })
                                        .child("No sessions"),
                                ),
                        );
                    }
                    for id in &worktree.sessions {
                        if let Some(meta) =
                            self.runtime.read(cx).registry.read(cx).meta(*id).cloned()
                        {
                            body = body.child(self.session_row(&meta, false, cx));
                        }
                    }
                }
            }
        }
        body.into_any_element()
    }
    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        let metas: Vec<_> = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .all()
            .iter()
            .filter(|m| {
                self.snapshot
                    .projects
                    .iter()
                    .any(|p| p.sessions.contains(&m.id))
            })
            .cloned()
            .collect();
        let mut body = div().flex().flex_col().gap(rpx(SPACE_SM));
        for group in [
            "Needs you",
            "Working",
            "Starting",
            "Failed",
            "Done",
            "Idle",
            "Exited",
        ] {
            let items: Vec<_> = metas
                .iter()
                .filter(|m| self.status(m, cx).0 == group)
                .collect();
            if items.is_empty() {
                continue;
            }
            body = body.child(
                div()
                    .px(rpx(SPACE_LG))
                    .pt(rpx(SPACE_LG))
                    .text_size(rpx(TEXT_SMALL))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(c::FG_DIM())
                    .child(format!("{group} · {}", items.len())),
            );
            for meta in items {
                body = body.child(self.session_row(meta, true, cx));
            }
        }
        if metas.is_empty() {
            body = body.child(
                div()
                    .p(rpx(SPACE_2XL))
                    .text_color(c::FG_DIM())
                    .child("No sessions yet. Start an agent from Project view."),
            );
        }
        body.into_any_element()
    }
    fn terminals(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut panel = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .border_t_1()
            .border_color(c::BORDER())
            .child(
                div()
                    .h(rpx(HEAD_H))
                    .px(rpx(SPACE_LG))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child("Terminals"),
                    )
                    .child(
                        self.control("add-terminal", "Add terminal", Action::AddTerminal, cx)
                            .child(icon("plus", ICON_SM, c::FG_DIM())),
                    )
                    .child(
                        self.control(
                            "fold-terminals",
                            if self.terminals_collapsed {
                                "Expand terminals"
                            } else {
                                "Collapse terminals"
                            },
                            Action::FoldTerminals,
                            cx,
                        )
                        .child(icon(
                            if self.terminals_collapsed {
                                "chev-right"
                            } else {
                                "chev-down"
                            },
                            ICON_SM,
                            c::FG_DIM(),
                        )),
                    ),
            );
        if !self.terminals_collapsed {
            for meta in self
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .home_terminals()
                .to_vec()
            {
                if self.terminal_owners.get(&meta.id).copied().unwrap_or(1) != self.active_workspace
                {
                    continue;
                }
                let id = meta.id;
                panel = panel.child(
                    self.row(
                        format!("home-{}", id.raw()),
                        meta.label.clone(),
                        self.selection == Some(Selection::Home(id)),
                        Action::Select(Selection::Home(id)),
                        cx,
                    )
                    .w_auto()
                    .mx(rpx(SPACE_LG))
                    .px(rpx(SPACE_LG))
                    .h_auto()
                    .min_h(rpx(ROW_H))
                    .py(rpx(SPACE_SM))
                    .my(rpx(SPACE_XS))
                    .child(
                        div()
                            .id(("home-icon", id.raw()))
                            .debug_selector(move || format!("home-icon-{}", id.raw()))
                            .child(icon("terminal", ICON_SM, c::FG_DIM())),
                    )
                    .child(div().flex_1().truncate().child(meta.label.clone()))
                    .child(
                        self.control(
                            ("close-home", id.raw()),
                            format!("Close {}", meta.label),
                            Action::CloseHome(id),
                            cx,
                        )
                        .debug_selector(move || format!("close-home-{}", id.raw()))
                        .child(icon("close", ICON_XS, c::FG_DIM())),
                    ),
                );
                if self.pending_home_close == Some(id) && self.canvas_close_anchor.is_none() {
                    panel = panel.child(self.confirmation(
                        &format!("Close {}? Its shell and running commands will stop. Files remain on disk.",meta.label),
                        Action::ConfirmHome(id),
                        cx,
                    ));
                }
            }
        }
        panel.into_any_element()
    }
}
impl Focusable for Sidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(window, cx);
        let logical_width = f32::from(window.viewport_size().width)
            / (f32::from(window.rem_size()) / crate::zoom::REM_BASE);
        let rail_width = Self::rail_width(window);
        self.compact_rail = rail_width < SIDEBAR_COMPACT_THRESHOLD;
        let navigation = if self.mode == ViewMode::List {
            self.list(cx)
        } else {
            self.tree(window, cx)
        };
        let controls = self.view_controls(cx);
        let terminals = self.terminals(cx);
        let empty = self.snapshot.projects.is_empty();
        let rail = div()
            .id("sidebar-rail")
            .debug_selector(|| "sidebar-rail".into())
            .w(rpx(rail_width))
            .overflow_hidden()
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(c::BORDER())
            .bg(c::BG_RAIL())
            .text_size(rpx(TEXT_BODY))
            .line_height(rpx(SPACE_3XL))
            .font_weight(gpui::FontWeight::NORMAL)
            .text_color(c::alpha(c::FG(), 0.88))
            .child(div().h(rpx(APPBAR_H)).flex_shrink_0())
            .child(
                div()
                    .id("sidebar-workspace-header")
                    .debug_selector(|| "sidebar-workspace-header".into())
                    .h(rpx(HEAD_H + SPACE_2XL))
                    .flex_shrink_0()
                    .pl(rpx(SPACE_SM))
                    .pr(rpx(SPACE_2XL))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when_some(self.workspace_selector.clone(), |row, selector| {
                                row.child(selector)
                            }),
                    )
                    .child(controls),
            )
            .child(
                div()
                    .px(rpx(SPACE_3XL))
                    .pt(rpx(SPACE_LG))
                    .text_size(rpx(TEXT_SMALL))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(c::FG_DIM())
                    .flex()
                    .items_center()
                    .child(div().flex_1().child(if self.mode == ViewMode::List {
                        "Sessions"
                    } else {
                        "Projects"
                    }))
                    .child(
                        self.control(
                            "projects-archive",
                            "Archived projects",
                            Action::ArchivedProjects,
                            cx,
                        )
                        .debug_selector(|| "projects-archive".into())
                        .child(icon("folder", ICON_SM, c::FG_DIM())),
                    )
                    .child(
                        self.control("projects-add", "Add project", Action::AddProject, cx)
                            .debug_selector(|| "projects-add".into())
                            .child(icon("plus", ICON_SM, c::FG_DIM())),
                    ),
            )
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p(rpx(if self.compact_rail {
                        SPACE_SM
                    } else {
                        SPACE_LG
                    }))
                    .when(empty, |d| {
                        d.child(
                            div()
                                .p(rpx(SPACE_LG))
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child("No projects yet"),
                        )
                    })
                    .child(navigation),
            )
            .child(terminals);
        let hide_editor_navigation = (self.pending_new_worktree.is_some()
            || self.pending_worktree_removal.is_some()
            || self.project_panel.is_some()
            || self.project_setup.is_some())
            && logical_width < EDITOR_FULL_WIDTH_BREAKPOINT;
        let content = self.render_content(window, cx);
        let confirming = self.project_decision
            || self.pending_close.is_some()
            || self.pending_home_close.is_some()
            || self.pending_remove.is_some();
        div()
            .size_full()
            .flex()
            .min_h_0()
            .track_focus(&self.focus)
            .text_size(rpx(TEXT_BODY))
            .text_color(c::FG())
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if let (Some(removal), "tab") = (
                    this.pending_worktree_removal.as_ref(),
                    event.keystroke.key.as_str(),
                ) {
                    let status = this
                        .runtime
                        .read(cx)
                        .projects
                        .read(cx)
                        .worktree_removal_status(&removal.path)
                        .cloned();
                    let finished = removal.started
                        && (removal.error.is_some()
                            || status
                                .as_ref()
                                .is_some_and(|s| s.stage == WorktreeRemovalStage::Finished));
                    let handles: Vec<FocusHandle> = if !removal.started {
                        vec![this.cancel_focus.clone(), this.confirm_focus.clone()]
                    } else if finished
                        || status
                            .as_ref()
                            .is_some_and(|s| s.stage == WorktreeRemovalStage::RunningScript)
                    {
                        vec![this.confirm_focus.clone()]
                    } else {
                        Vec::new()
                    };
                    window.prevent_default();
                    if handles.is_empty() {
                        this.worktree_removal_focus.focus(window, cx);
                    } else {
                        let current = handles.iter().position(|focus| focus.is_focused(window));
                        let shift = event.keystroke.modifiers.shift;
                        let next = match current {
                            Some(index) if shift => (index + handles.len() - 1) % handles.len(),
                            Some(index) => (index + 1) % handles.len(),
                            None if shift => handles.len() - 1,
                            None => 0,
                        };
                        handles[next].focus(window, cx);
                    }
                    cx.stop_propagation();
                } else if this.confirmation_open()
                    && !this.project_decision
                    && this.pending_worktree_removal.is_none()
                    && event.keystroke.key == "tab"
                {
                    window.prevent_default();
                    if this.cancel_focus.is_focused(window) {
                        this.confirm_focus.focus(window, cx);
                    } else {
                        this.cancel_focus.focus(window, cx);
                    }
                    cx.stop_propagation();
                } else if event.keystroke.key == "escape"
                    && !this.project_decision
                    && (this.confirmation_open()
                        || this.menu.is_some()
                        || this.pending_new_worktree.is_some())
                {
                    this.act(Action::Cancel, window, cx);
                    cx.stop_propagation();
                }
            }))
            .when(
                self.mode != ViewMode::Grid && !hide_editor_navigation,
                |d| {
                    d.child(div().relative().h_full().child(rail).when(
                        self.project_decision || self.pending_worktree_removal.is_some(),
                        |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .occlude()
                                    .bg(c::alpha(c::BG(), 0.4))
                                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    }),
                            )
                        },
                    ))
                },
            )
            .child(
                div()
                    .id("sidebar-canvas")
                    .debug_selector(|| "sidebar-canvas".into())
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(content)
                    .when(confirming && !self.project_decision, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .bg(c::alpha(c::BG(), 0.4))
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                }),
                        )
                    }),
            )
    }
}

/// Stable urgency order; failure remains visible ahead of ordinary activity.
fn project_activity_rollup<T>(
    statuses: impl IntoIterator<Item = (&'static str, T)>,
) -> Option<(&'static str, T)> {
    statuses
        .into_iter()
        .min_by_key(|(status, _)| match *status {
            "Needs you" => 0,
            "Failed" => 1,
            "Working" | "Starting" => 2,
            "Done" => 3,
            "Idle" => 4,
            _ => 5,
        })
}

/// Strip only a separated leading braille activity glyph from chrome titles.
fn display_task_title(title: Option<String>, fallback: &str) -> String {
    let Some(title) = title else {
        return fallback.to_string();
    };
    let mut title = title.trim();
    let separator = |ch: char| matches!(ch, '·' | '|' | '-' | '–' | '—' | ':');
    if let Some(first) = title.chars().next() {
        let rest = &title[first.len_utf8()..];
        if ('\u{2800}'..='\u{28ff}').contains(&first)
            && (rest.is_empty()
                || rest.starts_with(char::is_whitespace)
                || rest.starts_with(separator))
        {
            title = rest.trim_start();
            if let Some(separator_char) = title.chars().next().filter(|ch| separator(*ch)) {
                title = title[separator_char.len_utf8()..].trim_start();
            }
        }
    }
    let uuid = title.len() == 36
        && title.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-')
        && [8, 13, 18, 23]
            .iter()
            .all(|index| title.as_bytes()[*index] == b'-');
    if title.is_empty() || uuid {
        fallback.to_string()
    } else {
        title.to_string()
    }
}

fn remap_selection(selection: &mut Option<Selection>, mapping: &HashMap<usize, usize>) {
    let replacement = match selection.as_ref() {
        Some(Selection::Project(i)) => mapping.get(i).copied().map(Selection::Project),
        Some(Selection::Worktree(i, path)) => mapping
            .get(i)
            .copied()
            .map(|idx| Selection::Worktree(idx, path.clone())),
        _ => return,
    };
    *selection = replacement;
}

fn change_mode(mode: &mut ViewMode, last: &mut ViewMode, next: ViewMode) {
    if next != ViewMode::Grid {
        *last = next;
    }
    *mode = next;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    #[test]
    fn display_titles_remove_only_transient_braille_prefixes() {
        for title in [
            " ⠋ Respond to a greeting ",
            "⠹ · Respond to a greeting",
            "⠋— Respond to a greeting",
        ] {
            assert_eq!(
                display_task_title(Some(title.into()), "Codex 1"),
                "Respond to a greeting"
            );
        }
        for title in [
            "",
            " ",
            "⠋ ",
            " 3e9f01f9-54cd-4f09-8617-137231faccc9 ",
            "⠋ 3e9f01f9-54cd-4f09-8617-137231faccc9",
        ] {
            assert_eq!(display_task_title(Some(title.into()), "Codex 1"), "Codex 1");
        }
        for title in [
            "! Important",
            "- Keep this",
            "… Thinking",
            "⠋braille",
            "⠋ ⠹ Keep second glyph",
        ] {
            let expected = if title == "⠋ ⠹ Keep second glyph" {
                "⠹ Keep second glyph"
            } else {
                title
            };
            assert_eq!(display_task_title(Some(title.into()), "Codex 1"), expected);
        }
        assert_eq!(
            display_task_title(None, "  Original label  "),
            "  Original label  "
        );
    }

    #[test]
    fn collapsed_project_rollup_uses_highest_descendant_urgency() {
        assert_eq!(project_activity_rollup::<()>([]), None);
        for (states, expected) in [
            (vec!["Idle", "Done", "Working", "Needs you"], "Needs you"),
            (vec!["Exited", "Done", "Starting"], "Starting"),
            (vec!["Working", "Failed"], "Failed"),
            (vec!["Exited", "Idle", "Done"], "Done"),
            (vec!["Exited", "Idle"], "Idle"),
            (vec!["Exited"], "Exited"),
        ] {
            assert_eq!(
                project_activity_rollup(states.into_iter().map(|state| (state, ()))),
                Some((expected, ()))
            );
        }
    }

    #[gpui::test]
    fn worktree_removal_rejects_main_and_keeps_failure_until_dismissed(
        cx: &mut gpui::TestAppContext,
    ) {
        let project_path = "/grove-worktree-removal-ui-test";
        let worktree_path = "/grove-worktree-removal-ui-test-feature";
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "demo".into(),
                    path: project_path.into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        assert!(cx
            .debug_bounds("delete-worktree-/grove-worktree-removal-ui-test")
            .is_none());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.snapshot.projects =
                    vec![crate::entities::workspace_state::SnapshotProject {
                        idx: 0,
                        name: "demo".into(),
                        is_git: true,
                        has_run: false,
                        worktrees: vec![
                            crate::entities::workspace_state::SnapshotWorktree {
                                path: project_path.into(),
                                name: "demo".into(),
                                is_main: true,
                                ..Default::default()
                            },
                            crate::entities::workspace_state::SnapshotWorktree {
                                path: worktree_path.into(),
                                name: "feature".into(),
                                is_main: false,
                                ..Default::default()
                            },
                        ],
                        sessions: Vec::new(),
                    }];
                sidebar.act(Action::RemoveWorktree(0, project_path.into()), window, cx);
                assert!(sidebar.pending_worktree_removal.is_none());
                sidebar
                    .worktree_delete_focus
                    .insert(worktree_path.into(), cx.focus_handle());
                sidebar.act(Action::RemoveWorktree(0, worktree_path.into()), window, cx);
                assert!(sidebar.confirmation_open());
                assert!(!sidebar.pending_worktree_removal.as_ref().unwrap().started);
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("worktree-removal-decision").is_some());
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| assert!(sidebar.read(cx).cancel_focus.is_focused(window)));
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| assert!(sidebar.read(cx).confirm_focus.is_focused(window)));
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.pending_worktree_removal.is_none()));
        cx.update(|window, cx| {
            assert!(sidebar.read(cx).worktree_delete_focus[worktree_path].is_focused(window));
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.snapshot.projects[0].worktrees.push(
                    crate::entities::workspace_state::SnapshotWorktree {
                        path: worktree_path.into(),
                        name: "feature".into(),
                        is_main: false,
                        ..Default::default()
                    },
                );
                sidebar.act(Action::RemoveWorktree(0, worktree_path.into()), window, cx);
                sidebar.act(Action::ConfirmWorktreeRemoval, window, cx);
                let error = sidebar
                    .pending_worktree_removal
                    .as_ref()
                    .unwrap()
                    .error
                    .clone();
                assert!(error.is_some());
                sidebar.act(Action::ConfirmWorktreeRemoval, window, cx);
                assert_eq!(
                    sidebar.pending_worktree_removal.as_ref().unwrap().error,
                    error
                );
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("worktree-removal-error").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.pending_worktree_removal.is_none()));
    }

    #[gpui::test]
    fn worktree_removal_decision_fits_desktop_and_narrow_canvas(cx: &mut gpui::TestAppContext) {
        let project_path = "/grove-worktree-removal-geometry";
        let worktree_path = "/grove-worktree-removal-geometry/feature-with-a-long-folder-name";
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "demo".into(),
                    path: project_path.into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.pending_worktree_removal = Some(PendingWorktreeRemoval {
                project_path: project_path.into(),
                path: worktree_path.into(),
                name: "feature-with-a-long-folder-name".into(),
                started: false,
                error: None,
            });
            sidebar
        });
        for width in [1280.0, 768.0] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(640.0)));
            draw(cx);
            let canvas = cx.debug_bounds("sidebar-canvas").unwrap();
            let dialog = cx.debug_bounds("worktree-removal-decision").unwrap();
            assert!(dialog.left() >= canvas.left() && dialog.right() <= canvas.right());
            assert!(dialog.top() >= canvas.top() && dialog.bottom() <= canvas.bottom());
            for id in [
                "worktree-removal-path",
                "worktree-removal-path-value",
                "cancel-worktree-removal",
                "confirm-worktree-removal",
            ] {
                let item = cx.debug_bounds(id).unwrap();
                assert!(
                    item.left() >= dialog.left() && item.right() <= dialog.right(),
                    "{id} clips horizontally at {width}px"
                );
                assert!(
                    item.top() >= dialog.top() && item.bottom() <= dialog.bottom(),
                    "{id} clips vertically at {width}px"
                );
            }
        }
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let removal = sidebar.pending_worktree_removal.as_mut().unwrap();
                removal.started = true;
                cx.notify();
            });
        });
        draw(cx);
        let dialog = cx.debug_bounds("worktree-removal-decision").unwrap();
        let status = cx.debug_bounds("worktree-removal-status").unwrap();
        assert!(status.left() >= dialog.left() && status.right() <= dialog.right());
        assert!(status.bottom() <= dialog.bottom());
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.pending_worktree_removal.as_mut().unwrap().error = Some(
                    "Git could not remove the worktree because the folder still contains local changes. Review the worktree and try again.".into(),
                );
                cx.notify();
            });
        });
        draw(cx);
        let dialog = cx.debug_bounds("worktree-removal-decision").unwrap();
        let error = cx.debug_bounds("worktree-removal-error").unwrap();
        let dismiss = cx.debug_bounds("dismiss-worktree-removal").unwrap();
        let canvas = cx.debug_bounds("sidebar-canvas").unwrap();
        assert!(dialog.left() >= canvas.left() && dialog.right() <= canvas.right());
        assert!(dialog.top() >= canvas.top() && dialog.bottom() <= canvas.bottom());
        for item in [error, dismiss] {
            assert!(item.left() >= dialog.left() && item.right() <= dialog.right());
            assert!(item.top() >= dialog.top() && item.bottom() <= dialog.bottom());
        }
    }

    #[gpui::test]
    fn home_terminal_rows_have_inset_padding_and_aligned_controls(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            for index in 0..2 {
                let term = cx.new(|cx| {
                    crate::entities::terminal_session::TerminalSession::attach_existing(
                        "grove-test-layout-not-attached",
                        24,
                        80,
                        cx,
                    )
                });
                registry.update(cx, |registry, _| {
                    let id = registry.next_home_id();
                    registry.push_home(
                        SessionMeta {
                            id,
                            project: String::new(),
                            wt_path: String::new(),
                            agent: Agent::Terminal,
                            context_roots: Vec::new(),
                            temp_bundle_path: None,
                            label: format!("terminal {index}"),
                            spawned_at: std::time::Instant::now(),
                            attention: None,
                            tmux: false,
                            tmux_name: None,
                        },
                        term,
                    );
                });
            }
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        let rail = cx.debug_bounds("sidebar-rail").unwrap();
        let mut previous: Option<gpui::Bounds<gpui::Pixels>> = None;
        for (row_id, close_id, icon_id) in [
            ("home-1", "close-home-1", "home-icon-1"),
            ("home-2", "close-home-2", "home-icon-2"),
        ] {
            let row = cx.debug_bounds(row_id).unwrap();
            let close = cx.debug_bounds(close_id).unwrap();
            let icon = cx.debug_bounds(icon_id).unwrap();
            assert!((f32::from(row.left() - rail.left()) - SPACE_LG).abs() <= 1.0);
            assert!((f32::from(rail.right() - row.right()) - SPACE_LG).abs() <= 1.0);
            assert_eq!(row.center().y, close.center().y);
            assert!((f32::from(icon.center().y - close.center().y)).abs() <= 1.0);
            assert!(close.left() >= row.left() && close.right() <= row.right());
            assert!(close.top() >= row.top() && close.bottom() <= row.bottom());
            if let Some(previous) = previous {
                assert!((f32::from(row.top() - previous.bottom()) - SPACE_XS * 2.0).abs() <= 1.0);
            }
            previous = Some(row);
        }
    }

    #[gpui::test]
    fn project_popup_preserves_rows_and_restores_focus_on_dismiss(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            let projects = ["one", "two"]
                .into_iter()
                .map(|name| grove_core::storage::Project {
                    name: name.into(),
                    path: format!("/grove-sidebar-test-{name}"),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                })
                .collect();
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects,
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::EditProject("/missing-project".into()), window, cx);
                sidebar.act(Action::RemoveProject("/missing-project".into()), window, cx);
                assert!(sidebar.project_panel.is_none());
                sidebar.act(
                    Action::EditProject("/grove-sidebar-test-one".into()),
                    window,
                    cx,
                );
                assert!(sidebar.project_panel.is_some());
                cx.global_mut::<SettingsState>()
                    .store
                    .workspaces
                    .create("Other")
                    .unwrap();
                sidebar.sync(window, cx);
                assert!(sidebar.project_panel.is_none());
                assert!(sidebar.project_return_focus.is_none());
                assert_eq!(window.focused(cx), Some(sidebar.focus.clone()));
                sidebar.act(
                    Action::EditProject("/grove-sidebar-test-one".into()),
                    window,
                    cx,
                );
                assert!(
                    sidebar.project_panel.is_none(),
                    "cross-workspace action is stale"
                );
                sidebar.finish_project_selection("/grove-sidebar-test-one", window, cx);
                assert_eq!(cx.global::<SettingsState>().store.workspaces.active, 1);
                assert_eq!(
                    window.focused(cx),
                    sidebar.project_menu_focus.get(&0).cloned()
                );
                assert!(sidebar.project_return_focus.is_none());
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("projects-add").is_some());
        assert!(cx.debug_bounds("projects-archive").is_some());
        let project_title = cx.debug_bounds("project-title-0").unwrap();
        let worktree_title = cx
            .debug_bounds("worktree-title-/grove-sidebar-test-one")
            .unwrap();
        let empty = cx
            .debug_bounds("worktree-empty-/grove-sidebar-test-one")
            .unwrap();
        assert_eq!(project_title.left(), worktree_title.left());
        assert_eq!(project_title.size.height, worktree_title.size.height);
        assert_eq!(empty.size.height, worktree_title.size.height);
        assert_eq!(f32::from(worktree_title.size.height), SPACE_3XL);
        assert_eq!(empty.left(), worktree_title.left());
        let project_count = cx.debug_bounds("project-count-0").unwrap();
        let worktree_count = cx
            .debug_bounds("worktree-count-/grove-sidebar-test-one")
            .unwrap();
        assert_eq!(project_count.right(), worktree_count.right());
        assert_eq!(project_count.size.width, worktree_count.size.width);
        assert!(cx.debug_bounds("project-activity-0").is_none());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let registry = sidebar.runtime.read(cx).registry.clone();
                registry.update(cx, |registry, cx| {
                    registry.insert_meta(
                        "one".into(),
                        "/grove-sidebar-test-one".into(),
                        Agent::Codex,
                    );
                    cx.notify();
                });
                sidebar.act(Action::Project(0), window, cx);
            });
        });
        draw(cx);
        let marker = cx
            .debug_bounds("project-activity-0")
            .expect("collapsed activity");
        let row = cx.debug_bounds("project-0").unwrap();
        assert!(row.contains(&marker.center()));
        assert_eq!(
            cx.debug_bounds("project-count-0").unwrap().right(),
            project_count.right()
        );
        assert_eq!(
            cx.debug_bounds("project-title-0").unwrap().left(),
            project_title.left()
        );
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::Project(0), window, cx);
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("project-activity-0").is_none());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.act(Action::Menu(0), window, cx));
        });
        draw(cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.project_panel.is_some()));
        assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.project_decision));
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.project_panel.is_none()));
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::ArchivedProjects, window, cx);
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("project-panel").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::AddProject, window, cx);
            });
        });
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.project_setup.is_some()));
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.project_setup.is_none()));
        let before = cx.debug_bounds("project-1").unwrap();
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.act(Action::Menu(0), window, cx));
        });
        draw(cx);
        let popup = cx.debug_bounds("project-actions-popup").unwrap();
        let trigger = cx.debug_bounds("project-menu-0").unwrap();
        assert!(
            (f32::from(popup.left() - trigger.right())).abs() <= 1.0,
            "popup {popup:?} must attach to trigger {trigger:?}"
        );
        assert!(
            (f32::from(popup.top() - trigger.top())).abs() <= 1.0,
            "popup must align vertically to its trigger"
        );
        assert_eq!(cx.debug_bounds("project-1").unwrap(), before);
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.menu.is_none()));
        cx.update(|window, cx| assert!(sidebar.read(cx).project_menu_focus[&0].is_focused(window)));
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.act(Action::Menu(0), window, cx));
        });
        draw(cx);
        cx.simulate_mouse_down(
            gpui::point(gpui::px(1.0), gpui::px(1.0)),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.menu.is_none()));
        cx.update(|window, cx| assert!(sidebar.read(cx).project_menu_focus[&0].is_focused(window)));
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.act(Action::Menu(0), window, cx));
        });
        draw(cx);
        cx.simulate_keystrokes("down down down enter");
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.confirmation_open()));
        for _ in 0..5 {
            cx.simulate_keystrokes("tab");
            draw(cx);
            cx.update(|window, cx| {
                let panel = sidebar.read(cx).project_panel.as_ref().unwrap();
                assert!(panel.focus_handle(cx).contains_focused(window, cx));
            });
        }
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.confirmation_open()));
        cx.update(|window, cx| assert!(sidebar.read(cx).project_menu_focus[&0].is_focused(window)));
        cx.simulate_resize(gpui::size(gpui::px(320.0), gpui::px(200.0)));
        draw(cx);
        let rail = cx.debug_bounds("sidebar-rail").unwrap();
        let canvas = cx.debug_bounds("sidebar-canvas").unwrap();
        assert!((f32::from(rail.size.width) - 128.0).abs() <= 1.0);
        assert!((f32::from(canvas.size.width) - 192.0).abs() <= 1.0);
        assert!(canvas.left() >= rail.right());
        let project = cx.debug_bounds("project-0").unwrap();
        assert!(project.right() <= rail.right());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.act(Action::Menu(0), window, cx));
        });
        draw(cx);
        let popup = cx.debug_bounds("project-actions-popup").unwrap();
        assert!(
            popup.left() >= gpui::px(0.0) && popup.right() <= gpui::px(320.0),
            "popup: {popup:?}; viewport: {:?}",
            cx.update(|window, _| window.viewport_size())
        );
        assert!(popup.top() >= gpui::px(0.0) && popup.bottom() <= gpui::px(200.0));
        cx.simulate_keystrokes("down enter");
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.pending_new_worktree == Some(0)));
        assert!(
            cx.debug_bounds("project-0").is_none(),
            "narrow editor must receive full canvas"
        );
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.pending_new_worktree.is_none()));
        assert!(cx.debug_bounds("project-0").is_some());
        cx.update(|window, cx| assert!(sidebar.read(cx).project_menu_focus[&0].is_focused(window)));
    }
    #[gpui::test]
    fn grid_close_is_attached_to_canvas_and_cancels_to_source(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            let project = grove_core::storage::Project {
                name: "demo".into(),
                path: "/grove-grid-modal-test".into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            };
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![project],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            runtime.read(cx).registry.clone().update(cx, |registry, _| {
                registry.insert_meta("demo".into(), "/grove-grid-modal-test".into(), Agent::Codex)
            });
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.mode = ViewMode::Grid;
            sidebar
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        let header = cx.debug_bounds("terminal-header-1").unwrap();
        let close = cx.debug_bounds("canvas-close-1").unwrap().center();
        cx.simulate_mouse_down(close, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(close, gpui::MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert!(
            sidebar.read_with(cx, |sidebar, _| sidebar.canvas_close_anchor
                == Some(SessionId::from_raw(1)))
        );
        let popup = cx.debug_bounds("canvas-confirmation").unwrap();
        assert!((f32::from(popup.left() - header.left())).abs() <= 1.0);
        assert!((f32::from(popup.top() - header.bottom())).abs() <= 1.0);
        let source = sidebar
            .read_with(cx, |sidebar, _| sidebar.confirmation_return_focus.clone())
            .unwrap();
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.confirmation_open()));
        cx.update(|window, _| assert!(source.is_focused(window)));
        assert_eq!(
            sidebar.read_with(cx, |sidebar, cx| sidebar
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .len()),
            1
        );
    }
    #[test]
    fn removing_an_earlier_project_keeps_repository_selection() {
        let mapping = HashMap::from([(1, 0), (2, 1)]);
        let mut selection = Some(Selection::Worktree(2, "/third/feature".into()));
        remap_selection(&mut selection, &mapping);
        assert_eq!(
            selection,
            Some(Selection::Worktree(1, "/third/feature".into()))
        );
        let mut removed = Some(Selection::Project(0));
        remap_selection(&mut removed, &mapping);
        assert_eq!(removed, None);
    }
    #[test]
    fn session_identity_survives_project_reindexing() {
        let mut selection = Some(Selection::Session(SessionId::from_raw(7)));
        remap_selection(&mut selection, &HashMap::new());
        assert_eq!(selection, Some(Selection::Session(SessionId::from_raw(7))));
    }
    #[test]
    fn grid_retains_last_sidebar_mode() {
        let (mut mode, mut last) = (ViewMode::Project, ViewMode::Project);
        change_mode(&mut mode, &mut last, ViewMode::List);
        change_mode(&mut mode, &mut last, ViewMode::Grid);
        assert_eq!(last, ViewMode::List);
        let restore = last;
        change_mode(&mut mode, &mut last, restore);
        assert_eq!(mode, ViewMode::List);
    }
}
