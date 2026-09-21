//! Workspace-scoped navigation. UI state is kept per workspace while processes keep running.
mod content;
use super::{rpx, terminal_view::TerminalView, tokens::*};
use crate::{
    activity::ActivityState,
    entities::{
        session_registry::{SessionId, SessionMeta},
        workspace_state::TreeSnapshot,
    },
    icons::icon,
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

const SIDEBAR_W: f32 = 236.0;
const SIDEBAR_MAX_VIEWPORT_FRACTION: f32 = 0.4;
/// Below this width, a setup editor temporarily owns the full canvas.
const EDITOR_FULL_WIDTH_BREAKPOINT: f32 = 640.0;
const HEAD_H: f32 = 36.0;
const ROW_H: f32 = 28.0;
const COUNT_W: f32 = 48.0;

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
    NewWorktree(usize),
    Reveal(String),
    RemoveProject(usize),
    ConfirmRemove(usize),
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

pub struct Sidebar {
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
    observers: Vec<gpui::Subscription>,
}
impl Sidebar {
    pub fn new(runtime: Entity<Runtime>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rt = runtime.read(cx);
        let (registry, activity, tree) =
            (rt.registry.clone(), rt.activity.clone(), rt.tree.clone());
        let worktree_name = cx.new(|cx| InputState::new(window, cx).placeholder("billing-retry"));
        let worktree_branch =
            cx.new(|cx| InputState::new(window, cx).placeholder("feat/billing-retry"));
        let worktree_base = cx.new(|cx| InputState::new(window, cx).placeholder("main"));
        let mut observers = vec![
            cx.observe(&runtime, |_, _, cx| cx.notify()),
            cx.observe(&registry, |_, _, cx| cx.notify()),
            cx.observe(&activity, |_, _, cx| cx.notify()),
            cx.observe(&tree, |_, _, cx| cx.notify()),
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
            runtime,
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
        let registry = self.runtime.read(cx).registry.read(cx);
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
    pub fn confirmation_open(&self) -> bool {
        self.pending_close.is_some()
            || self.pending_home_close.is_some()
            || self.pending_remove.is_some()
    }
    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        if let Some(focus) = self.menu_return_focus.take() {
            cx.defer_in(window, move |_, window, cx| focus.focus(window, cx));
        }
        cx.notify();
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
                .when(self.is_grid(), |d| d.bg(c::BG_HOVER()))
                .child(icon("grid", ICON_SM, c::FG_DIM())),
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
        let confirm = matches!(
            action,
            Action::ConfirmClose(_) | Action::ConfirmHome(_) | Action::ConfirmRemove(_)
        );
        let cancel = matches!(action, Action::Cancel);
        let confirming = self.pending_close.is_some()
            || self.pending_home_close.is_some()
            || self.pending_remove.is_some();
        let decision = matches!(
            action,
            Action::ConfirmClose(_)
                | Action::ConfirmHome(_)
                | Action::ConfirmRemove(_)
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
            .hover(|s| s.bg(c::BG_HOVER()))
            .focus(|s| s.bg(c::BG_HOVER()).border_1().border_color(c::FG_DIM()))
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
        if (self.pending_close.is_some()
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
            Action::NewWorktree(i) => {
                self.menu = None;
                self.worktree_return_focus =
                    self.menu_return_focus.take().or_else(|| window.focused(cx));
                self.begin_worktree(i, window, cx);
            }
            Action::Reveal(path) => {
                cx.reveal_path(std::path::Path::new(&path));
                self.close_menu(window, cx);
            }
            Action::RemoveProject(i) => {
                self.pending_remove = Some(i);
                self.menu = None;
                self.confirmation_return_focus =
                    self.menu_return_focus.take().or_else(|| window.focused(cx));
                self.cancel_focus.focus(window, cx);
            }
            Action::ConfirmRemove(i) => {
                self.runtime
                    .read(cx)
                    .projects
                    .clone()
                    .update(cx, |p, cx| p.remove_project(i, false, cx));
                self.pending_remove = None;
            }
            Action::Launch(i, path, agent) => self.launch(i, path, agent, cx),
            Action::Close(id) => {
                self.pending_close = Some(id);
                self.confirmation_return_focus = window.focused(cx);
                self.cancel_focus.focus(window, cx);
            }
            Action::ConfirmClose(id) => {
                self.runtime.update(cx, |r, cx| r.kill_session(id, cx));
                self.pending_close = None;
            }
            Action::Retry(id) => {
                let meta = self.runtime.read(cx).registry.read(cx).meta(id).cloned();
                if let Some(meta) = meta {
                    self.runtime.update(cx, |r, cx| {
                        r.kill_session(id, cx);
                        r.spawn_session_in(meta.project, meta.wt_path, meta.agent, cx)
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
            }
            Action::Cancel => {
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
            .gap(rpx(SPACE_LG))
            .p(rpx(SPACE_LG))
            .border_1()
            .border_color(c::BORDER_STRONG())
            .bg(c::SURFACE_RAISED())
            .child(label.to_string())
            .child(
                div()
                    .flex()
                    .when(self.compact_rail, gpui::Styled::flex_col)
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
        let row = self
            .row(
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
                        .text_size(rpx(TEXT_MICRO))
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
            );
        let mut result = div().child(row);
        if list {
            result = result.child(
                div()
                    .pl(rpx(SPACE_3XL))
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .truncate()
                    .child(meta.wt_path.clone()),
            );
        }
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
        if self.pending_close == Some(id) {
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
        let path = cx.global::<SettingsState>().store.projects[idx]
            .path
            .clone();
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
            .border_color(c::BORDER_STRONG())
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
                                (this.menu_index + 2) % 3
                            } else {
                                (this.menu_index + 1) % 3
                            };
                            this.menu_focus.focus(window, cx);
                            cx.notify();
                            cx.stop_propagation();
                        }
                        "enter" | "space" => {
                            let action = match this.menu_index {
                                0 => Action::NewWorktree(idx),
                                1 => Action::Reveal(
                                    cx.global::<SettingsState>().store.projects[idx]
                                        .path
                                        .clone(),
                                ),
                                _ => Action::RemoveProject(idx),
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
            ("New worktree", "plus", Action::NewWorktree(idx)),
            ("Open in file manager", "folder", Action::Reveal(path)),
            ("Remove project", "close", Action::RemoveProject(idx)),
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
        for project in &self.snapshot.projects {
            let idx = project.idx;
            let closed = self.collapsed_projects.contains(&idx);
            body = body.child(
                self.row(
                    format!("project-{idx}"),
                    project.name.clone(),
                    self.selection == Some(Selection::Project(idx)),
                    Action::Select(Selection::Project(idx)),
                    cx,
                )
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
                    .child(icon(
                        if closed { "chev-right" } else { "chev-down" },
                        ICON_XS,
                        c::FG_DIM(),
                    )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(c::FG())
                        .child(project.name.clone()),
                )
                .child(
                    div()
                        .w(rpx(if self.compact_rail { 0.0 } else { COUNT_W }))
                        .overflow_hidden()
                        .text_size(rpx(TEXT_MICRO))
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
            if !project.is_git {
                body = body.child(
                    div()
                        .pl(rpx(SPACE_3XL))
                        .text_size(rpx(TEXT_MICRO))
                        .text_color(c::YELLOW())
                        .child("Not a Git repository"),
                );
            }
            if project.worktrees.is_empty() {
                body = body.child(
                    div()
                        .pl(rpx(SPACE_3XL))
                        .text_size(rpx(TEXT_MICRO))
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
                    .pl(rpx(SPACE_LG))
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
                            .child(worktree.name.clone()),
                    )
                    .child(
                        div()
                            .relative()
                            .w(rpx(CHROME_CONTROL_H * 3.0))
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
                                    .pr(rpx(SPACE_LG))
                                    .text_size(rpx(TEXT_MICRO))
                                    .text_color(c::FG_DIM())
                                    .opacity(if focused || self.compact_rail {
                                        0.0
                                    } else {
                                        1.0
                                    })
                                    .group_hover("worktree-row", |s| s.opacity(0.0))
                                    .child(format!("{}", worktree.sessions.len())),
                            )
                            .child(launches),
                    ),
                );
                if !collapsed {
                    if worktree.sessions.is_empty() {
                        body = body.child(
                            div()
                                .pl(rpx(SPACE_3XL * 2.0))
                                .py(rpx(SPACE_SM))
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child("No sessions"),
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
                    .text_size(rpx(TEXT_MICRO))
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
            .flex_shrink_0()
            .border_t_1()
            .border_color(c::BORDER_STRONG())
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
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child("TERMINALS"),
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
                    .child(icon("terminal", ICON_SM, c::FG_DIM()))
                    .child(div().flex_1().truncate().child(meta.label.clone()))
                    .child(
                        self.control(
                            ("close-home", id.raw()),
                            format!("Close {}", meta.label),
                            Action::CloseHome(id),
                            cx,
                        )
                        .child(icon("close", ICON_XS, c::FG_DIM())),
                    ),
                );
                if self.pending_home_close == Some(id) {
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
        let rail_width = SIDEBAR_W.min(logical_width * SIDEBAR_MAX_VIEWPORT_FRACTION);
        self.compact_rail = rail_width < SIDEBAR_W;
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
            .border_color(c::BORDER_STRONG())
            .bg(c::BG_RAIL())
            .child(
                div()
                    .h(rpx(HEAD_H))
                    .flex_shrink_0()
                    .px(rpx(SPACE_LG))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(c::BORDER_STRONG())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child("PROJECTS"),
                    )
                    .child(controls),
            )
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p(rpx(SPACE_SM))
                    .when(empty, |d| {
                        d.child(
                            div()
                                .p(rpx(SPACE_LG))
                                .text_color(c::FG_DIM())
                                .child("No projects yet"),
                        )
                    })
                    .child(navigation),
            )
            .child(terminals);
        let hide_editor_navigation =
            self.pending_new_worktree.is_some() && logical_width < EDITOR_FULL_WIDTH_BREAKPOINT;
        let content = self.render_content(window, cx);
        let confirming = self.pending_close.is_some()
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
                if this.confirmation_open() && event.keystroke.key == "tab" {
                    window.prevent_default();
                    if this.cancel_focus.is_focused(window) {
                        this.confirm_focus.focus(window, cx);
                    } else {
                        this.cancel_focus.focus(window, cx);
                    }
                    cx.stop_propagation();
                } else if event.keystroke.key == "escape"
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
                |d| d.child(rail),
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
                    .when(confirming, |d| {
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
                    theme: None,
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
        cx.simulate_keystrokes("down down enter");
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.confirmation_open()));
        cx.update(|window, cx| assert!(sidebar.read(cx).cancel_focus.is_focused(window)));
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| assert!(sidebar.read(cx).confirm_focus.is_focused(window)));
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| assert!(sidebar.read(cx).cancel_focus.is_focused(window)));
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
        cx.simulate_keystrokes("enter");
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
