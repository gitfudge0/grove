//! Workspace-scoped navigation. UI state is kept per workspace while processes keep running.
mod collapsed;
mod content;
mod grid;
mod multi_project;
use multi_project::SidebarContext;
mod project_flyout;
mod project_setup;
mod projects;
mod session_diff;
use super::{motion, rpx, terminal_view::TerminalView, tokens::*};
use crate::{
    activity::ActivityState,
    entities::{
        diff_viewer::DiffViewerState,
        session_registry::{SessionId, SessionMeta},
        workspace_state::{clamp_sidebar_width, TreeSnapshot},
    },
    icons::icon,
    project_service::{ProjectEvent, WorktreeReadiness, WorktreeRemovalStage},
    runtime::Runtime,
    settings::SettingsState,
    theme as c,
};
use gpui::{
    div, prelude::*, AnyElement, App, Context, CursorStyle, Div, Entity, EventEmitter, FocusHandle,
    Focusable, MouseButton, MouseMoveEvent, ScrollHandle, SharedString, Stateful, Window,
};
use gpui_component::input::InputState;
use grove_core::{agent::Agent, storage::SidebarAppearance};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const SIDEBAR_W: f32 = 260.0;
const SIDEBAR_COLLAPSED_W: f32 = 52.0;
const SIDEBAR_COMPACT_THRESHOLD: f32 = 236.0;
const SIDEBAR_MAX_VIEWPORT_FRACTION: f32 = 0.4;
const SIDEBAR_NARROW_BREAKPOINT: f32 = SIDEBAR_W / SIDEBAR_MAX_VIEWPORT_FRACTION;
const SIDEBAR_DRAG_EPSILON: f32 = 1.0;
const WORKTREE_LAUNCH_AGENTS: [Agent; 4] = [
    Agent::Codex,
    Agent::Claude,
    Agent::OpenCode,
    Agent::Terminal,
];

fn rail_background(cx: &App) -> gpui::Hsla {
    match cx.global::<SettingsState>().store.sidebar_appearance {
        SidebarAppearance::Frosted => c::BG_RAIL_GLASS(),
        SidebarAppearance::Solid => c::BG_RAIL(),
    }
}
/// Below this width, a setup editor temporarily owns the full canvas.
const EDITOR_FULL_WIDTH_BREAKPOINT: f32 = 640.0;
const HEAD_H: f32 = 36.0;
const ROW_H: f32 = 28.0;
// Expanded project navigation uses nested type roles and a single session content column.
const HIERARCHY_INSET: f32 = 5.0;
const HIERARCHY_GAP: f32 = 7.0;
const HIERARCHY_ICON_SLOT: f32 = 18.0;
const HIERARCHY_ICON: f32 = 15.0;
const HIERARCHY_LABEL_INSET: f32 = HIERARCHY_INSET + HIERARCHY_ICON_SLOT + HIERARCHY_GAP;
const HIERARCHY_META_TEXT: f32 = TEXT_MICRO;
const HIERARCHY_TRAILING_W: f32 = 24.0;
const PROJECT_GROUP_GAP: f32 = 14.0;
const PROJECT_ROW_H: f32 = 35.0;
const PROJECT_TEXT: f32 = TEXT_TITLE;
const PROJECT_TITLE_LINE_H: f32 = 21.0;
const WORKTREE_ROW_H: f32 = 40.0;
const WORKTREE_TITLE_LINE_H: f32 = 17.0;
const WORKTREE_META_LINE_H: f32 = 14.0;
const WORKTREE_ACTION_W: f32 = 22.0;
const SESSION_ROW_H: f32 = 43.0;
const SESSION_TREE_ROW_H: f32 =
    HIERARCHY_INSET * 2.0 + SESSION_META_LINE_H * 2.0 + SESSION_TITLE_LINE_H + SPACE_XS * 2.0;
/// Launcher glyph and label share the approved small header size.
const SESSION_LAUNCHER_SIZE: f32 = ICON_XS;
const SESSION_TITLE_LINE_H: f32 = 17.0;
const SESSION_META_LINE_H: f32 = 14.0;
const SESSION_ROW_RADIUS: f32 = 7.0;
const SESSION_FIRST_GAP: f32 = 3.0;
const SESSION_AGE_REFRESH: Duration = Duration::from_secs(1);
const WORKTREE_READINESS_REFRESH: Duration = Duration::from_secs(5);

fn sidebar_worktree_name(name: &str, is_main: bool) -> &str {
    if is_main {
        "Main checkout"
    } else {
        name
    }
}

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
#[derive(Clone, Copy)]
struct SessionListEntry {
    id: SessionId,
    state: ActivityState,
    status: &'static str,
    since: Option<Instant>,
    active: Option<Instant>,
}

/// Sort workspace-scoped sessions by attention, then by the clock relevant to
/// each section. Indices retain the matching metadata and process identity.
fn group_sessions_for_list(
    entries: &[SessionListEntry],
    now: Instant,
) -> Vec<(&'static str, Vec<usize>)> {
    let mut groups: [Vec<usize>; 4] = std::array::from_fn(|_| Vec::new());
    for (index, entry) in entries.iter().enumerate() {
        let section = if entry.status == "Failed"
            || (entry.status != "Starting" && entry.state == ActivityState::WaitingForInput)
        {
            0
        } else if entry.status != "Starting" && entry.state == ActivityState::Done {
            1
        } else if entry.status == "Starting"
            || entry.state == ActivityState::Working
            || (entry.state != ActivityState::Exited
                && entry.active.is_some_and(|active| {
                    now.saturating_duration_since(active) < crate::activity::IDLE_DWELL
                }))
        {
            2
        } else {
            3
        };
        groups[section].push(index);
    }
    for group in &mut groups[..2] {
        group.sort_by(|a, b| {
            entries[*a]
                .since
                .cmp(&entries[*b].since)
                .then_with(|| entries[*a].id.cmp(&entries[*b].id))
        });
    }
    for group in &mut groups[2..] {
        group.sort_by(|a, b| {
            (entries[*a].state == ActivityState::Exited)
                .cmp(&(entries[*b].state == ActivityState::Exited))
                .then_with(|| entries[*b].active.cmp(&entries[*a].active))
                .then_with(|| entries[*b].id.cmp(&entries[*a].id))
        });
    }
    ["NEEDS YOU", "REVIEW", "WORKING", "IDLE"]
        .into_iter()
        .zip(groups)
        .filter(|(_, group)| !group.is_empty())
        .collect()
}

#[cfg(test)]
fn visible_tree_sessions(snapshot: &TreeSnapshot) -> Vec<SessionId> {
    snapshot
        .projects
        .iter()
        .flat_map(|project| {
            project
                .worktrees
                .iter()
                .flat_map(|worktree| worktree.sessions.iter().copied())
        })
        .collect()
}

fn step_session(
    order: &[SessionId],
    selected: Option<SessionId>,
    forward: bool,
) -> Option<SessionId> {
    if order.is_empty() {
        return None;
    }
    let next = match order.iter().position(|id| Some(*id) == selected) {
        Some(index) if forward => (index + 1) % order.len(),
        Some(0) => order.len() - 1,
        Some(index) => index - 1,
        None if !forward => order.len() - 1,
        None => 0,
    };
    Some(order[next])
}

fn zen_target(
    mode: ViewMode,
    selection: Option<&Selection>,
    sessions: &[(SessionId, bool)],
) -> Option<(SessionId, bool)> {
    let selected = match selection {
        Some(Selection::Session(id)) if sessions.contains(&(*id, false)) => Some((*id, false)),
        Some(Selection::Home(id)) if sessions.contains(&(*id, true)) => Some((*id, true)),
        _ => None,
    };
    if mode == ViewMode::Grid {
        selected.or_else(|| sessions.first().copied())
    } else {
        selected
    }
}

fn home_terminal_status(failed: bool, pending: bool, alive: bool) -> &'static str {
    if failed {
        "Failed"
    } else if pending {
        "Starting"
    } else if alive {
        "Running"
    } else {
        "Exited"
    }
}

fn managed_session_status(
    failed: bool,
    pending_attach: bool,
    activity: ActivityState,
) -> (&'static str, gpui::Hsla) {
    if failed {
        return ("Failed", c::RED());
    }
    if pending_attach {
        return ("Idle", c::FG_DIM());
    }
    match activity {
        ActivityState::WaitingForInput => ("Needs you", c::YELLOW()),
        ActivityState::Working => ("Working", c::GREEN()),
        ActivityState::Done => ("Done", c::FG_DIM()),
        ActivityState::Idle => ("Idle", c::FG_DIM()),
        ActivityState::Exited => ("Exited", c::FG_DIM()),
    }
}

fn terminal_directory_label(
    current: Option<&str>,
    initial: Option<&str>,
) -> Option<(&'static str, String)> {
    current
        .map(|cwd| ("home-current-cwd", format!("Current directory {cwd}")))
        .or_else(|| initial.map(|cwd| ("home-launch-cwd", format!("Launched in {cwd}"))))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidebarEvent {
    SettingsRequested,
    NewMultiProjectSessionRequested,
}
#[derive(Default)]
struct Navigation {
    selection: Option<Selection>,
    scroll: ScrollHandle,
    terminals_collapsed: bool,
}
#[derive(Clone)]
enum Action {
    Select(Selection),
    ToggleProject(String),
    ProjectFlyout(SidebarContext),
    Mode(ViewMode),
    Menu(usize),
    NewWorktree(String),
    InitializeGit(String),
    RefreshGit(String),
    EditProject(String),
    MoveProject(String),
    AddProject,
    ArchivedProjects,
    Reveal(String),
    RemoveProject(String),
    ConfirmRemove(usize),
    RemoveWorktree(usize, String),
    ConfirmWorktreeRemoval,
    SkipWorktreeTeardown,
    DismissWorktreeRemoval,
    ProjectFlyoutLaunchOptions(String),
    Launch(usize, String, Agent),
    RunScript(String, String),
    Close(SessionId),
    ConfirmClose(SessionId),
    Retry(SessionId),
    OpenDiff(SessionId),
    CloseDiff,
    SelectDiffFile(String),
    RefreshDiff,
    AddTerminal,
    OpenSettings,
    NewMultiProjectSession,
    ToggleSidebar,
    OpenWorkspaces,
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

#[derive(Clone, Copy)]
struct SidebarDrag {
    start_width: f32,
    width: f32,
    grab_offset: f32,
}

/// Keep a temporary narrow-window cap out of the saved, app-wide preference.
fn effective_rail_width(preferred: f32, logical_width: f32) -> f32 {
    let preferred = if preferred.is_finite() {
        preferred
    } else {
        SIDEBAR_W
    };
    let logical_width = if logical_width.is_finite() {
        logical_width.max(0.0)
    } else {
        SIDEBAR_W / SIDEBAR_MAX_VIEWPORT_FRACTION
    };
    let width = clamp_sidebar_width(preferred, logical_width);
    if logical_width < SIDEBAR_NARROW_BREAKPOINT {
        width.min(logical_width * SIDEBAR_MAX_VIEWPORT_FRACTION)
    } else {
        width
    }
}

pub struct Sidebar {
    project_panel: Option<Entity<projects::ProjectPanel>>,
    project_setup: Option<Entity<project_setup::ProjectSetup>>,
    settings_panel: Option<Entity<super::settings_panel::SettingsPanel>>,
    project_decision: bool,
    project_return_focus: Option<FocusHandle>,
    project_return_path: Option<String>,
    workspace_selector: Option<Entity<super::workspace_manager::WorkspaceManager>>,
    runtime: Entity<Runtime>,
    focus: FocusHandle,
    shell_focus: Option<FocusHandle>,
    collapse_focus: FocusHandle,
    compact_workspace_focus: FocusHandle,
    selection: Option<Selection>,
    initial_selection_pending: bool,
    pending_canvas_focus: Option<SessionId>,
    /// One-time focus handoff to the first tile after a Grid workspace change.
    pending_grid_workspace_focus: Option<SessionId>,
    mode: ViewMode,
    last_mode: ViewMode,
    zen_return: Option<(ViewMode, Option<Selection>)>,
    snapshot: TreeSnapshot,
    active_workspace: u64,
    saved: HashMap<u64, Navigation>,
    scroll: ScrollHandle,
    terminal_owners: HashMap<SessionId, u64>,
    terminals_collapsed: bool,
    project_flyout: Option<SidebarContext>,
    project_flyout_hover_open: bool,
    project_flyout_hover_blocked: bool,
    project_flyout_suppressed: Option<SidebarContext>,
    project_flyout_hide_task: Option<gpui::Task<()>>,
    project_group_bounds: HashMap<usize, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    project_flyout_popup_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    project_flyout_focus: FocusHandle,
    project_flyout_index: usize,
    project_flyout_launch_path: Option<String>,
    project_flyout_launch_error: Option<String>,
    project_flyout_launch_focus: FocusHandle,
    project_flyout_plus_focus: HashMap<String, FocusHandle>,
    project_flyout_item_focus: HashMap<String, FocusHandle>,
    rail_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    project_flyout_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    project_flyout_scroll: ScrollHandle,
    multi_project_focus: FocusHandle,
    multi_project_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    multi_project_group_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    menu: Option<usize>,
    menu_opened_by_hover: bool,
    menu_focus: FocusHandle,
    menu_index: usize,
    project_menu_focus: HashMap<usize, FocusHandle>,
    project_toggle_focus: HashMap<String, FocusHandle>,
    menu_return_focus: Option<FocusHandle>,
    project_menu_bounds: HashMap<usize, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    session_close_bounds:
        HashMap<SessionId, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    home_close_bounds: HashMap<SessionId, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    menu_trigger_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    menu_popup_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
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
    git_init_pending: Option<String>,
    worktree_readiness: HashMap<String, (Instant, Option<WorktreeReadiness>)>,
    diff_viewer: Option<Entity<DiffViewerState>>,
    diff_focus: FocusHandle,
    diff_return_focus: Option<FocusHandle>,
    diff_observer: Option<gpui::Subscription>,
    terminal_views: HashMap<SessionId, Entity<TerminalView>>,
    home_terminal_views: HashMap<SessionId, Entity<TerminalView>>,
    available: [bool; 4],
    project_paths: HashMap<usize, String>,
    collapsed_projects: HashSet<String>,
    worktree_focus: HashMap<String, FocusHandle>,
    session_diff_focus: HashMap<SessionId, FocusHandle>,
    session_diff_observers: HashMap<SessionId, (gpui::Subscription, gpui::Subscription)>,
    cache_warm: Option<(u64, u64)>,
    compact_rail: bool,
    drag: Option<SidebarDrag>,
    grid_layouts: HashMap<u64, grid::WorkspaceGrid>,
    grid_drag: Option<grid::GridDrag>,
    grid_session_drag: Option<grid::GridSessionDrag>,
    grid_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    canvas_close_anchor: Option<SessionId>,
    canvas_close_focus: HashMap<(SessionId, bool), FocusHandle>,
    canvas_bounds: HashMap<SessionId, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    canvas_observers: HashMap<SessionId, gpui::Subscription>,
    canvas_focus_observers: HashMap<SessionId, gpui::Subscription>,
    canvas_signatures: HashMap<SessionId, (Option<String>, Option<String>, bool, bool)>,
    title_observers: HashMap<
        SessionId,
        (
            Entity<crate::entities::terminal_session::TerminalSession>,
            gpui::Subscription,
        ),
    >,
    observed_titles: HashMap<SessionId, Option<String>>,
    age_timer: Option<gpui::Task<()>>,
    #[cfg(test)]
    age_ticks: u64,
    observers: Vec<gpui::Subscription>,
}
impl EventEmitter<SidebarEvent> for Sidebar {}
impl Sidebar {
    pub(crate) fn set_workspace_selector(
        &mut self,
        selector: Entity<super::workspace_manager::WorkspaceManager>,
    ) {
        self.workspace_selector = Some(selector);
    }
    pub(crate) fn set_shell_focus(&mut self, focus: FocusHandle) {
        self.shell_focus = Some(focus);
    }
    pub(crate) fn set_settings_panel(
        &mut self,
        panel: Entity<super::settings_panel::SettingsPanel>,
        cx: &mut Context<Self>,
    ) {
        self.observers
            .push(cx.observe(&panel, |_, _, cx| cx.notify()));
        self.settings_panel = Some(panel);
        cx.notify();
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
            settings_panel: None,
            project_decision: false,
            project_return_focus: None,
            project_return_path: None,
            runtime,
            workspace_selector: None,
            focus: cx.focus_handle(),
            shell_focus: None,
            collapse_focus: cx.focus_handle(),
            compact_workspace_focus: cx.focus_handle(),
            selection: None,
            initial_selection_pending: true,
            pending_canvas_focus: None,
            pending_grid_workspace_focus: None,
            mode: ViewMode::Project,
            last_mode: ViewMode::Project,
            zen_return: None,
            snapshot: TreeSnapshot::default(),
            active_workspace: cx.global::<SettingsState>().store.workspaces.active,
            saved: HashMap::new(),
            scroll: ScrollHandle::new(),
            terminal_owners: HashMap::new(),
            terminals_collapsed: false,
            project_flyout: None,
            project_flyout_hover_open: false,
            project_flyout_hover_blocked: false,
            project_flyout_suppressed: None,
            project_flyout_hide_task: None,
            project_group_bounds: HashMap::new(),
            project_flyout_popup_bounds: std::rc::Rc::default(),
            project_flyout_focus: cx.focus_handle(),
            project_flyout_index: 0,
            project_flyout_launch_path: None,
            project_flyout_launch_error: None,
            project_flyout_launch_focus: cx.focus_handle().tab_stop(true),
            project_flyout_plus_focus: HashMap::new(),
            project_flyout_item_focus: HashMap::new(),
            rail_bounds: std::rc::Rc::default(),
            project_flyout_bounds: std::rc::Rc::default(),
            project_flyout_scroll: ScrollHandle::new(),
            multi_project_focus: cx.focus_handle(),
            multi_project_bounds: std::rc::Rc::default(),
            multi_project_group_bounds: std::rc::Rc::default(),
            menu: None,
            menu_opened_by_hover: false,
            menu_focus: cx.focus_handle(),
            menu_index: 0,
            project_menu_focus: HashMap::new(),
            project_toggle_focus: HashMap::new(),
            menu_return_focus: None,
            menu_trigger_bounds: std::rc::Rc::default(),
            menu_popup_bounds: std::rc::Rc::default(),
            project_menu_bounds: HashMap::new(),
            session_close_bounds: HashMap::new(),
            home_close_bounds: HashMap::new(),
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
            git_init_pending: None,
            worktree_readiness: HashMap::new(),
            diff_viewer: None,
            diff_focus: cx.focus_handle(),
            diff_return_focus: None,
            diff_observer: None,
            terminal_views: HashMap::new(),
            home_terminal_views: HashMap::new(),
            available: WORKTREE_LAUNCH_AGENTS.map(Agent::available),
            project_paths: HashMap::new(),
            collapsed_projects: HashSet::new(),
            worktree_focus: HashMap::new(),
            session_diff_focus: HashMap::new(),
            session_diff_observers: HashMap::new(),
            cache_warm: None,
            compact_rail: false,
            drag: None,
            grid_layouts: HashMap::new(),
            grid_drag: None,
            grid_session_drag: None,
            grid_bounds: std::rc::Rc::default(),
            canvas_close_anchor: None,
            canvas_close_focus: HashMap::new(),
            canvas_bounds: HashMap::new(),
            canvas_observers: HashMap::new(),
            canvas_focus_observers: HashMap::new(),
            canvas_signatures: HashMap::new(),
            title_observers: HashMap::new(),
            observed_titles: HashMap::new(),
            age_timer: None,
            #[cfg(test)]
            age_ticks: 0,
            observers,
        }
    }
    fn queue_worktree_readiness(&mut self, path: String, cx: &mut Context<Self>) {
        let now = Instant::now();
        if self
            .worktree_readiness
            .get(&path)
            .is_some_and(|(checked, _)| now.duration_since(*checked) < WORKTREE_READINESS_REFRESH)
        {
            return;
        }
        self.worktree_readiness.insert(path.clone(), (now, None));
        cx.spawn(async move |this, cx| {
            let checked_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { crate::project_service::worktree_readiness(&checked_path) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .worktree_readiness
                    .get(&path)
                    .is_some_and(|(started, state)| *started == now && state.is_none())
                {
                    this.worktree_readiness
                        .insert(path, (Instant::now(), Some(result)));
                    cx.notify();
                }
            });
        })
        .detach();
    }
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != ViewMode::Grid || self.is_zen() || !self.navigation_available() {
            self.grid_session_drag = None;
        }
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
            for saved in self.saved.values_mut() {
                remap_selection(&mut saved.selection, &mapping);
            }
            self.menu = self.menu.and_then(|i| mapping.get(&i).copied());
            self.pending_remove = self.pending_remove.and_then(|i| mapping.get(&i).copied());
            self.pending_new_worktree = self
                .pending_new_worktree
                .and_then(|i| mapping.get(&i).copied());
            self.project_paths = paths;
        }
        self.collapsed_projects
            .retain(|path| self.project_paths.values().any(|project| project == path));
        self.project_toggle_focus
            .retain(|path, _| self.project_paths.values().any(|project| project == path));
        for path in self.project_paths.values() {
            self.project_toggle_focus
                .entry(path.clone())
                .or_insert_with(|| cx.focus_handle());
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
            self.zen_return = None;
            self.saved.insert(
                self.active_workspace,
                Navigation {
                    selection: self.selection.take(),
                    scroll: self.scroll.clone(),
                    terminals_collapsed: self.terminals_collapsed,
                },
            );
            let next = self.saved.remove(&active).unwrap_or_default();
            self.selection = next.selection;
            self.scroll = next.scroll;
            self.terminals_collapsed = next.terminals_collapsed;
            self.active_workspace = active;
            self.menu = None;
            self.pending_close = None;
            self.pending_home_close = None;
            self.canvas_close_anchor = None;
            self.grid_drag = None;
            self.grid_session_drag = None;
            self.pending_remove = None;
            self.pending_new_worktree = None;
            self.content_error = None;
            self.pending_canvas_focus = None;
            self.pending_grid_workspace_focus = None;
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
        let stale_project_flyout = switched
            || !self.is_collapsed(cx)
            || self.mode != ViewMode::Project
            || self.zen_return.is_some()
            || self
                .settings_panel
                .as_ref()
                .is_some_and(|panel| panel.read(cx).is_open())
            || self.project_flyout.as_ref().is_some_and(|target| {
                if *target == SidebarContext::MultiProject {
                    return false;
                }
                let Some(path) = target.project_path() else {
                    return true;
                };
                !self
                    .snapshot
                    .projects
                    .iter()
                    .any(|p| self.project_paths.get(&p.idx) == Some(path))
            });
        if stale_project_flyout {
            let restore_focus = self.project_flyout.is_some()
                && self.project_flyout_focus.contains_focused(window, cx);
            self.project_flyout = None;
            self.project_flyout_launch_path = None;
            self.project_flyout_launch_error = None;
            self.project_flyout_hover_open = false;
            self.project_flyout_hide_task = None;
            if restore_focus {
                self.focus.focus(window, cx);
            }
        }
        let git_paths = {
            let registry = self.runtime.read(cx).registry.read(cx);
            let mut seen = HashSet::new();
            self.snapshot
                .projects
                .iter()
                .flat_map(|project| project.sessions.iter())
                .filter_map(|id| registry.meta(*id))
                .map(|meta| crate::paths::normalize_wt_path(&meta.wt_path).to_string())
                .filter(|path| !path.is_empty() && seen.insert(path.clone()))
                .collect::<Vec<_>>()
        };
        tree.update(cx, |tree, cx| {
            tree.maybe_poll_git_state(git_paths, window.is_window_active(), cx);
        });
        for project in &self.snapshot.projects {
            self.project_menu_bounds.entry(project.idx).or_default();
            self.project_group_bounds.entry(project.idx).or_default();
            self.project_menu_focus
                .entry(project.idx)
                .or_insert_with(|| cx.focus_handle());
        }
        for id in self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| &project.sessions)
        {
            self.session_close_bounds.entry(*id).or_default();
        }
        for meta in self.runtime.read(cx).registry.read(cx).home_terminals() {
            self.home_close_bounds.entry(meta.id).or_default();
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
        let visible_session_ids: HashSet<_> = self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| project.worktrees.iter())
            .flat_map(|worktree| worktree.sessions.iter().copied())
            .collect();
        self.session_diff_focus
            .retain(|id, _| visible_session_ids.contains(id));
        self.session_diff_observers
            .retain(|id, _| visible_session_ids.contains(id));
        for id in visible_session_ids {
            if let std::collections::hash_map::Entry::Vacant(entry) =
                self.session_diff_focus.entry(id)
            {
                let handle = cx.focus_handle();
                let focus_in = cx.on_focus_in(&handle, window, |_, _, cx| cx.notify());
                let focus_out = cx.on_focus_out(&handle, window, |_, _, _, cx| cx.notify());
                entry.insert(handle);
                self.session_diff_observers
                    .insert(id, (focus_in, focus_out));
            }
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
        if self.selection.is_none() && (switched || self.initial_selection_pending) {
            let next = self
                .visible_session_order(cx)
                .first()
                .copied()
                .map(Selection::Session)
                .or_else(|| {
                    self.active_canvas_sessions(cx)
                        .into_iter()
                        .find_map(|(id, home)| home.then_some(Selection::Home(id)))
                })
                .or_else(|| {
                    self.snapshot
                        .projects
                        .first()
                        .map(|project| Selection::Project(project.idx))
                });
            if let Some(selection) = next {
                self.select(selection, cx);
                self.pending_canvas_focus = match self.selection {
                    Some(Selection::Session(id) | Selection::Home(id)) => Some(id),
                    _ => None,
                };
            }
        }
        if switched {
            self.runtime.read(cx).state.clone().update(cx, |state, cx| {
                state.clear_canvas_selection();
                cx.notify();
            });
            if self.mode == ViewMode::Grid && !self.is_zen() {
                self.selection = self.active_canvas_sessions(cx).first().map(|(id, home)| {
                    if *home {
                        Selection::Home(*id)
                    } else {
                        Selection::Session(*id)
                    }
                });
            }
            if let Some(selection) = self.selection.clone() {
                self.select(selection, cx);
            }
            self.pending_canvas_focus = match self.selection {
                Some(Selection::Session(id) | Selection::Home(id)) => Some(id),
                _ => None,
            };
            if self.mode == ViewMode::Grid && !self.is_zen() {
                self.pending_grid_workspace_focus = self.pending_canvas_focus;
            }
            cx.notify();
        }
        let checked_indices = [
            match &self.selection {
                Some(Selection::Project(idx) | Selection::Worktree(idx, _)) => Some(*idx),
                _ => None,
            },
            self.menu,
        ];
        let checked_paths = checked_indices
            .into_iter()
            .flatten()
            .filter_map(|idx| {
                cx.global::<SettingsState>()
                    .store
                    .projects
                    .get(idx)
                    .map(|p| p.path.clone())
            })
            .collect::<Vec<_>>();
        for path in checked_paths {
            self.queue_worktree_readiness(path, cx);
        }
        self.sync_title_observers(cx);
    }

    fn sync_title_observers(&mut self, cx: &mut Context<Self>) {
        let registry = self.runtime.read(cx).registry.read(cx);
        let mut observed = Vec::new();
        let mut seen = HashSet::new();
        for id in self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| project.sessions.iter().copied())
        {
            if seen.insert(id) {
                if let Some(session) = registry.session(id) {
                    observed.push((id, session.clone()));
                }
            }
        }
        if !self.terminals_collapsed {
            for (index, meta) in registry.home_terminals().iter().enumerate() {
                if self.terminal_owners.get(&meta.id).copied().unwrap_or(1) == self.active_workspace
                {
                    if let Some(session) = registry.home_terminal(index) {
                        observed.push((meta.id, session.clone()));
                    }
                }
            }
        }
        let observed_ids: HashSet<_> = observed.iter().map(|(id, _)| *id).collect();
        self.title_observers
            .retain(|id, _| observed_ids.contains(id));
        self.observed_titles
            .retain(|id, _| observed_ids.contains(id));
        for (id, session) in observed {
            if self
                .title_observers
                .get(&id)
                .is_some_and(|(existing, _)| existing == &session)
            {
                continue;
            }
            self.observed_titles.insert(id, session.read(cx).title());
            let observer = cx.observe(&session, move |this, session, cx| {
                let title = session.read(cx).title();
                if this.observed_titles.get(&id) != Some(&title) {
                    this.observed_titles.insert(id, title);
                    cx.notify();
                }
            });
            self.title_observers.insert(id, (session, observer));
        }
    }
    fn age_visible(&self) -> bool {
        self.mode == ViewMode::List
            && self
                .snapshot
                .projects
                .iter()
                .any(|project| !project.sessions.is_empty())
    }

    fn sync_age_timer(&mut self, cx: &mut Context<Self>) {
        if !self.age_visible() {
            self.age_timer = None;
        } else if self.age_timer.is_none() {
            self.age_timer = Some(
                cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| loop {
                    cx.background_executor().timer(SESSION_AGE_REFRESH).await;
                    let visible = this
                        .update(cx, |this, cx| {
                            if this.age_visible() {
                                #[cfg(test)]
                                {
                                    this.age_ticks += 1;
                                }
                                cx.notify();
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false);
                    if !visible {
                        break;
                    }
                }),
            );
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
        if let Some(grid) = self.grid_layouts.get(&workspace) {
            grid::ordered_sessions(sessions, &grid.order)
        } else {
            sessions
        }
    }
    /// Called after the switcher closes so focus never moves behind its overlay.
    pub(crate) fn focus_grid_workspace_after_switch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync(window, cx);
        if self.mode != ViewMode::Grid || self.is_zen() {
            return;
        }
        let Some((id, home)) = self.active_canvas_sessions(cx).first().copied() else {
            self.pending_grid_workspace_focus = None;
            self.focus.focus(window, cx);
            return;
        };
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
            self.pending_grid_workspace_focus = Some(id);
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn canvas_terminal_focus(
        &self,
        id: SessionId,
        home: bool,
        cx: &App,
    ) -> Option<FocusHandle> {
        let view = if home {
            self.home_terminal_views.get(&id)
        } else {
            self.terminal_views.get(&id)
        }?;
        Some(view.focus_handle(cx))
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
        // Zen hides the header that records tile bounds, so its saved bounds are stale.
        // Anchor the tray at the top inset of the full-screen pane instead.
        let bounds = if self.is_zen() {
            None
        } else {
            Some(self.canvas_bounds.get(&id)?.get())
        };
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
                    "Close {} in {}? Its process will stop. The worktree stays on disk.",
                    meta.label, meta.project
                ),
                Action::ConfirmClose(id),
            )
        };
        let session_close = matches!(&action, Action::ConfirmClose(_));
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let gutter = if session_close { SPACE_3XL } else { SPACE_LG };
        let max_width = if session_close {
            SESSION_CLOSE_W
        } else {
            MODAL_W_SM
        };
        let available_width =
            (f32::from(window.viewport_size().width) / scale - gutter * 2.0).max(0.0);
        let width = bounds
            .map_or(max_width, |bounds| f32::from(bounds.size.width) / scale)
            .clamp(SIDEBAR_W, max_width)
            .min(available_width);
        let position = bounds.map_or_else(
            || gpui::point(gpui::px(gutter * scale), gpui::px(gutter * scale)),
            |bounds| gpui::point(bounds.left(), bounds.bottom()),
        );
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position_mode(gpui::AnchoredPositionMode::Window)
                    .position(position)
                    .snap_to_window_with_margin(gpui::px(gutter * scale))
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
        self.menu_opened_by_hover = false;
        if let Some(focus) = self.menu_return_focus.take() {
            cx.defer_in(window, move |_, window, cx| focus.focus(window, cx));
        }
        cx.notify();
    }
    fn open_menu_on_hover(&mut self, idx: usize, cx: &mut Context<Self>) {
        if self.menu == Some(idx) || self.confirmation_open() {
            return;
        }
        self.menu = Some(idx);
        self.menu_opened_by_hover = true;
        self.menu_index = 0;
        self.menu_return_focus = None;
        if let Some(bounds) = self.project_menu_bounds.get(&idx) {
            self.menu_trigger_bounds = bounds.clone();
        }
        cx.notify();
    }
    fn dismiss_project_panel_for_navigation(&mut self) {
        if self.project_decision {
            return;
        }
        self.project_panel = None;
        self.project_return_focus = None;
        self.project_return_path = None;
    }
    fn logical_window_width(window: &Window) -> f32 {
        f32::from(window.viewport_size().width)
            / (f32::from(window.rem_size()) / crate::zoom::REM_BASE)
    }
    /// Shared with the appbar so window controls follow the rail actually on screen.
    pub(crate) fn rail_visible(&self, window: &Window, cx: &App) -> bool {
        let settings_open = self
            .settings_panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).is_open());
        let editor_open = self.pending_new_worktree.is_some()
            || self.pending_worktree_removal.is_some()
            || self.project_panel.is_some()
            || self.project_setup.is_some()
            || settings_open;
        ((self.mode != ViewMode::Grid && !self.is_zen()) || settings_open)
            && !(editor_open && Self::logical_window_width(window) < EDITOR_FULL_WIDTH_BREAKPOINT)
    }
    pub(crate) fn is_collapsed(&self, cx: &App) -> bool {
        cx.global::<SettingsState>().store.sidebar_collapsed
    }
    pub(crate) fn rail_width(&self, window: &Window, cx: &App) -> f32 {
        if self.is_collapsed(cx) {
            return SIDEBAR_COLLAPSED_W;
        }
        let preferred = self.drag.map_or_else(
            || {
                cx.global::<SettingsState>()
                    .store
                    .sidebar_width
                    .unwrap_or(SIDEBAR_W)
            },
            |drag| drag.width,
        );
        effective_rail_width(preferred, Self::logical_window_width(window))
    }
    fn divider_press(
        &mut self,
        event: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.prevent_default();
        cx.stop_propagation();
        if self.is_collapsed(cx) {
            return;
        }
        if event.click_count == 2 {
            self.drag = None;
            SettingsState::update(cx, |store| store.sidebar_width = Some(SIDEBAR_W));
            cx.notify();
            return;
        }
        let width = self.rail_width(window, cx);
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        self.drag = Some(SidebarDrag {
            start_width: width,
            width,
            grab_offset: width - f32::from(event.position.x) / scale,
        });
    }
    fn divider_move(&mut self, event: &MouseMoveEvent, window: &Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        if !event.dragging() {
            self.drag = None;
            return;
        }
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let cursor = f32::from(event.position.x) / scale;
        let width = effective_rail_width(
            cursor + drag.grab_offset,
            Self::logical_window_width(window),
        );
        if (drag.width - width).abs() > f32::EPSILON {
            drag.width = width;
            cx.notify();
        }
        cx.stop_propagation();
    }
    fn divider_release(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if (drag.width - drag.start_width).abs() >= SIDEBAR_DRAG_EPSILON {
            SettingsState::update(cx, |store| store.sidebar_width = Some(drag.width));
        }
        cx.notify();
    }
    pub fn is_grid(&self) -> bool {
        self.mode == ViewMode::Grid
    }
    pub(crate) fn is_zen(&self) -> bool {
        self.zen_return.is_some()
    }

    pub(crate) fn toggle_zen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_available() {
            return;
        }
        self.sync(window, cx);
        if let Some((mode, selection)) = self.zen_return.take() {
            self.mode = mode;
            self.selection = selection;
            let view = match self.selection {
                Some(Selection::Session(id)) => self.terminal_views.get(&id),
                Some(Selection::Home(id)) => self.home_terminal_views.get(&id),
                _ => None,
            };
            if let Some(view) = view {
                view.focus_handle(cx).focus(window, cx);
            } else {
                self.focus.focus(window, cx);
            }
            cx.notify();
            return;
        }
        let sessions = self.active_canvas_sessions(cx);
        let target = zen_target(self.mode, self.selection.as_ref(), &sessions);
        self.zen_return = Some((self.mode, self.selection.clone()));
        if let Some((id, home)) = target {
            self.selection = Some(if home {
                Selection::Home(id)
            } else {
                Selection::Session(id)
            });
            let view = if home {
                self.home_terminal_views.get(&id)
            } else {
                self.terminal_views.get(&id)
            };
            if let Some(view) = view {
                view.focus_handle(cx).focus(window, cx);
            } else {
                self.pending_canvas_focus = Some(id);
                self.focus.focus(window, cx);
            }
        } else {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }
    pub fn view_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        self.view_controls_with_gap(SPACE_LG, cx)
    }
    fn view_controls_with_gap(&self, gap: f32, cx: &mut Context<Self>) -> AnyElement {
        let target = if self.mode == ViewMode::Grid {
            self.last_mode
        } else if self.mode == ViewMode::Project {
            ViewMode::List
        } else {
            ViewMode::Project
        };
        let grid_selected = self.is_grid();
        let grid_control = self
            .control(
                "sidebar-grid",
                "Open grid view",
                Action::Mode(if grid_selected {
                    self.last_mode
                } else {
                    ViewMode::Grid
                }),
                cx,
            )
            .debug_selector(|| "sidebar-grid".into())
            .child(icon("grid", ICON_SM, c::FG_DIM()));
        div()
            .flex()
            .items_center()
            .gap(rpx(gap))
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
                .debug_selector(|| "sidebar-view".into())
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
            .child(motion::background(
                grid_control,
                format!("sidebar-grid-selection-{grid_selected}"),
                if grid_selected {
                    c::alpha(c::BG(), 0.0)
                } else {
                    c::BG_HOVER()
                },
                if grid_selected {
                    c::BG_HOVER()
                } else {
                    c::alpha(c::BG(), 0.0)
                },
                Duration::from_millis(MOTION_FAST_MS),
                cx,
            ))
            .into_any_element()
    }
    fn settings_control(&self, cx: &mut Context<Self>) -> AnyElement {
        self.control(
            "sidebar-settings",
            "Open settings",
            Action::OpenSettings,
            cx,
        )
        .debug_selector(|| "sidebar-settings".into())
        .child(icon("cog", ICON_MD, c::FG_DIM()))
        .into_any_element()
    }
    fn navigation_heading(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let project = self.mode == ViewMode::Project;
        let gap = if project { SPACE_SM } else { SPACE_LG };
        let label = if self.mode == ViewMode::List {
            "Sessions"
        } else if !project && !self.multi_project_sessions(cx).is_empty() {
            ""
        } else {
            "Projects"
        };
        let mut run = window.text_style().to_run(label.len());
        run.font.weight = gpui::FontWeight::MEDIUM;
        let font_size = rpx(TEXT_SMALL).to_pixels(window.rem_size());
        let label_width = navigation_label_width(label, font_size, &run, window.text_system());
        div()
            .id("sidebar-navigation-header")
            .debug_selector(|| "sidebar-navigation-header".into())
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_SM))
            .text_size(rpx(TEXT_SMALL))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(c::FG_DIM())
            .child(motion::fast(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .debug_selector(|| "sidebar-heading-label".into())
                            .w(label_width)
                            .min_w_0()
                            .truncate()
                            .child(label),
                    )
                    .when(project, |heading| {
                        heading.child(
                            div()
                                .id("projects-count")
                                .debug_selector(|| "projects-count".into())
                                .flex_shrink_0()
                                .font_family(crate::fonts::UI_FAMILY)
                                .font_weight(gpui::FontWeight::NORMAL)
                                .child(format!("({})", self.snapshot.projects.len())),
                        )
                    }),
                format!("sidebar-heading-{:?}", self.mode),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(rpx(gap))
                    .child(self.view_controls_with_gap(gap, cx))
                    .child(
                        self.control(
                            "projects-archive",
                            "Archived projects",
                            Action::ArchivedProjects,
                            cx,
                        )
                        .debug_selector(|| "projects-archive".into())
                        .child(icon("archive", ICON_SM, c::FG_DIM())),
                    )
                    .child(
                        self.control("projects-add", "Add project", Action::AddProject, cx)
                            .debug_selector(|| "projects-add".into())
                            .child(icon("plus", ICON_SM, c::FG_DIM())),
                    ),
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
        let icon_rail = self.is_collapsed(cx);
        let hierarchy_row = !icon_rail
            && matches!(
                &action,
                Action::Select(Selection::Project(_) | Selection::Worktree(..))
            );
        let project_session_row = !icon_rail
            && self.mode == ViewMode::Project
            && matches!(&action, Action::Select(Selection::Session(_)));
        let selected_session_row = matches!(
            &action,
            Action::Select(Selection::Session(id)) if self.selection == Some(Selection::Session(*id))
        );
        let project_diff_button =
            self.mode == ViewMode::Project && matches!(&action, Action::OpenDiff(_));
        let quiet_launch_control = matches!(&action, Action::Launch(..) | Action::RunScript(..));
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
                if hierarchy_row {
                    s.text_color(c::FG())
                } else if project_session_row {
                    s.bg(if selected_session_row {
                        c::alpha(c::FG(), 0.14)
                    } else {
                        c::BG_HOVER()
                    })
                } else if project_diff_button || quiet_launch_control {
                    s
                } else if danger {
                    s.bg(c::RED_WASH()).text_color(c::RED())
                } else if primary {
                    s.bg(c::FG_DIM()).text_color(c::BG())
                } else {
                    s.bg(c::BG_HOVER())
                }
            })
            .focus_visible(move |s| {
                if hierarchy_row {
                    s.border_1().border_color(c::FG())
                } else if project_session_row {
                    s.bg(if selected_session_row {
                        c::alpha(c::FG(), 0.14)
                    } else {
                        c::BG_HOVER()
                    })
                    .border_1()
                    .border_color(c::FG())
                } else if project_diff_button || quiet_launch_control {
                    s.border_1().border_color(c::FG())
                } else if danger {
                    s.bg(c::RED_WASH()).text_color(c::RED())
                } else if primary {
                    s.bg(c::FG_DIM()).text_color(c::BG())
                } else {
                    let style = s.bg(c::BG_HOVER());
                    if icon_rail {
                        style.border_1().border_color(c::FG())
                    } else {
                        style
                    }
                }
            })
            .when(
                !(matches!(&action, Action::ProjectFlyout(_))
                    || icon_rail && matches!(&action, Action::Select(Selection::Session(_)))),
                |control| {
                    control.tooltip(move |window, cx| {
                        crate::views::components::tooltip(label.clone(), window).build(window, cx)
                    })
                },
            )
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
            .when(selected, |d| {
                d.bg(c::alpha(c::FG(), 0.14)).text_color(c::FG())
            })
    }
    fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        self.dismiss_project_context(cx);
        self.initial_selection_pending = false;
        self.pending_canvas_focus = None;
        self.pending_grid_workspace_focus = None;
        self.dismiss_project_panel_for_navigation();
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
    fn launch_target(
        &self,
        project: usize,
        path: String,
        agent: Agent,
        cx: &App,
    ) -> Option<(String, String, Agent)> {
        if !WORKTREE_LAUNCH_AGENTS
            .iter()
            .zip(self.available)
            .any(|(candidate, available)| *candidate == agent && available)
        {
            return None;
        }
        let name = self
            .snapshot
            .projects
            .iter()
            .find(|p| p.idx == project)
            .filter(|p| {
                p.worktrees.iter().any(|worktree| worktree.path == path)
                    || cx
                        .global::<SettingsState>()
                        .store
                        .projects
                        .get(project)
                        .is_some_and(|stored| {
                            stored.path == path
                                && !stored.archived
                                && cx
                                    .global::<SettingsState>()
                                    .store
                                    .project_workspace_id(&path)
                                    == cx.global::<SettingsState>().store.workspaces.active
                        })
            })
            .map(|p| p.name.clone())?;
        Some((name, path, agent))
    }
    fn launch(
        &mut self,
        project: usize,
        path: String,
        agent: Agent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !WORKTREE_LAUNCH_AGENTS
            .iter()
            .zip(self.available)
            .any(|(candidate, available)| *candidate == agent && available)
        {
            self.content_error = Some(format!(
                "{} CLI not found. Install it, check that it runs in your shell, then restart Grove so it sees your PATH. You can start a Terminal session here now.",
                match agent {
                    Agent::Codex => "Codex",
                    Agent::Claude => "Claude Code",
                    Agent::OpenCode => "OpenCode",
                    Agent::Terminal => "Terminal",
                }
            ));
            if self.project_flyout.is_some() {
                self.project_flyout_launch_error = self.content_error.clone();
            }
            cx.notify();
            return false;
        }
        let Some((name, path, agent)) = self.launch_target(project, path, agent, cx) else {
            return false;
        };
        if !self
            .runtime
            .update(cx, |r, cx| r.spawn_session_in(name, path, agent, cx))
        {
            return false;
        }
        if let Some(id) = self.runtime.read(cx).state.read(cx).active_session() {
            self.selection = Some(Selection::Session(id));
        }
        self.content_error = None;
        cx.notify();
        true
    }
    /// Mirror a successful launch from the shell palette into the canvas selection.
    /// Runtime already owns the active session; keeping this local selection in sync
    /// lets the existing canvas renderer create and focus its terminal view.
    pub(crate) fn select_active_session(&mut self, cx: &mut Context<Self>) -> Option<SessionId> {
        let id = self.runtime.read(cx).state.read(cx).active_session()?;
        self.selection = Some(Selection::Session(id));
        self.pending_new_worktree = None;
        cx.notify();
        Some(id)
    }

    /// Hand focus to the session after the command palette has restored its prior focus.
    pub(crate) fn focus_active_session_after_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.select_active_session(cx) else {
            return;
        };
        if let Some(view) = self.terminal_views.get(&id) {
            self.pending_canvas_focus = None;
            self.pending_grid_workspace_focus = None;
            view.focus_handle(cx).focus(window, cx);
        } else {
            self.pending_canvas_focus = Some(id);
            self.pending_grid_workspace_focus = (self.mode == ViewMode::Grid).then_some(id);
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    fn list_session_groups(&self, cx: &App) -> Vec<(&'static str, Vec<SessionId>)> {
        let registry = self.runtime.read(cx).registry.read(cx);
        let activity = self.runtime.read(cx).activity.read(cx);
        let entries: Vec<_> = registry
            .all()
            .iter()
            .filter(|meta| {
                self.snapshot
                    .projects
                    .iter()
                    .any(|project| project.sessions.contains(&meta.id))
            })
            .map(|meta| SessionListEntry {
                id: meta.id,
                state: activity.state_of(meta.id),
                status: self.status(meta, cx).0,
                since: activity.since_of(meta.id),
                active: activity.active_of(meta.id),
            })
            .collect();
        group_sessions_for_list(&entries, Instant::now())
            .into_iter()
            .map(|(heading, indices)| {
                (
                    heading,
                    indices.into_iter().map(|index| entries[index].id).collect(),
                )
            })
            .collect()
    }

    fn visible_session_order(&self, cx: &App) -> Vec<SessionId> {
        match self.mode {
            ViewMode::Project => {
                let multi = self.multi_project_sessions(cx);
                let mut order = multi.iter().map(|meta| meta.id).collect::<Vec<_>>();
                let snapshot = self.project_navigation_snapshot(cx);
                order.extend(
                    snapshot
                        .projects
                        .iter()
                        .filter(|project| {
                            self.project_paths
                                .get(&project.idx)
                                .or_else(|| {
                                    cx.global::<SettingsState>()
                                        .store
                                        .projects
                                        .get(project.idx)
                                        .map(|stored| &stored.path)
                                })
                                .is_none_or(|path| {
                                    self.is_collapsed(cx) || !self.collapsed_projects.contains(path)
                                })
                        })
                        .flat_map(|project| {
                            project
                                .worktrees
                                .iter()
                                .flat_map(|worktree| worktree.sessions.iter().copied())
                        }),
                );
                order
            }
            ViewMode::List => self
                .list_session_groups(cx)
                .into_iter()
                .flat_map(|(_, ids)| ids)
                .collect(),
            ViewMode::Grid => self
                .active_canvas_sessions(cx)
                .into_iter()
                .filter_map(|(id, home)| (!home).then_some(id))
                .collect(),
        }
    }

    pub(crate) fn visible_session_targets(&self, cx: &App) -> Vec<(SessionId, String)> {
        let registry = self.runtime.read(cx).registry.read(cx);
        self.visible_session_order(cx)
            .into_iter()
            .filter_map(|id| {
                let meta = registry.meta(id)?;
                let title = session_display_title(
                    meta,
                    registry
                        .session(id)
                        .and_then(|session| session.read(cx).title()),
                );
                Some((id, format!("{title} · {}", meta.project)))
            })
            .collect()
    }

    pub(crate) fn select_session_id(
        &mut self,
        id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.navigation_available() {
            return;
        }
        self.sync(window, cx);
        if self.visible_session_order(cx).contains(&id) {
            self.navigate_to_session(id, window, cx);
        }
    }

    pub(crate) fn request_close_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_available() {
            return;
        }
        let canvas_visible = self.mode == ViewMode::Grid || self.is_zen();
        match self.selection {
            Some(Selection::Session(id)) => {
                if canvas_visible {
                    self.request_canvas_close(id, false, window, cx);
                } else {
                    if let Some(project) = self.snapshot.projects.iter().find(|project| {
                        project
                            .worktrees
                            .iter()
                            .any(|worktree| worktree.sessions.contains(&id))
                    }) {
                        if let Some(stored) =
                            cx.global::<SettingsState>().store.projects.get(project.idx)
                        {
                            self.collapsed_projects.remove(&stored.path);
                        }
                    }
                    self.act(Action::Close(id), window, cx);
                }
            }
            Some(Selection::Home(id)) => {
                if canvas_visible {
                    self.request_canvas_close(id, true, window, cx);
                } else {
                    self.terminals_collapsed = false;
                    self.act(Action::CloseHome(id), window, cx);
                }
            }
            Some(Selection::Project(_) | Selection::Worktree(_, _)) | None => {}
        }
    }

    pub(crate) fn highlights_blocked(&self) -> bool {
        !self.navigation_available()
    }

    fn navigation_available(&self) -> bool {
        !self.confirmation_open()
            && self.diff_viewer.is_none()
            && self.project_panel.is_none()
            && self.project_setup.is_none()
            && self.pending_new_worktree.is_none()
            && self.menu.is_none()
    }

    fn canvas_focus_allowed(&self, window: &Window, cx: &App) -> bool {
        self.canvas_focus_allowed_for_mode(window, cx, false)
    }

    fn grid_workspace_focus_allowed(&self, window: &Window, cx: &App) -> bool {
        self.canvas_focus_allowed_for_mode(window, cx, true)
    }

    fn canvas_focus_allowed_for_mode(&self, window: &Window, cx: &App, allow_grid: bool) -> bool {
        if !self.navigation_available()
            || (self.mode == ViewMode::Grid && !self.is_zen() && !allow_grid)
            || self
                .workspace_selector
                .as_ref()
                .is_some_and(|selector| selector.read(cx).is_open())
        {
            return false;
        }
        window.focused(cx).is_none()
            || self.focus.contains_focused(window, cx)
            || self
                .shell_focus
                .as_ref()
                .is_some_and(|focus| focus.is_focused(window))
            || self
                .workspace_selector
                .as_ref()
                .is_some_and(|selector| selector.focus_handle(cx).is_focused(window))
    }

    fn navigate_to_session(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        self.select(Selection::Session(id), cx);
        if let Some(view) = self.terminal_views.get(&id) {
            view.focus_handle(cx).focus(window, cx);
        } else {
            self.focus.focus(window, cx);
        }
    }

    pub(crate) fn select_next_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_relative_session(1, window, cx);
    }

    pub(crate) fn select_previous_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_relative_session(-1, window, cx);
    }

    fn select_relative_session(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_available() {
            return;
        }
        self.sync(window, cx);
        let selected = self
            .selection
            .as_ref()
            .and_then(|selection| match selection {
                Selection::Session(id) => Some(*id),
                _ => None,
            });
        if let Some(id) = step_session(&self.visible_session_order(cx), selected, delta > 0) {
            self.navigate_to_session(id, window, cx);
        }
    }

    /// `number` is one based and matches the badges shown beside visible rows.
    pub(crate) fn select_numbered_session(
        &mut self,
        number: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.navigation_available() || number == 0 {
            return;
        }
        self.sync(window, cx);
        if let Some(id) = self.visible_session_order(cx).get(number - 1).copied() {
            self.navigate_to_session(id, window, cx);
        }
    }

    pub(crate) fn select_waiting_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_available() {
            return;
        }
        self.sync(window, cx);
        let workspace_ids: HashSet<_> = self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| project.sessions.iter().copied())
            .collect();
        let target = {
            let activity = self.runtime.read(cx).activity.read(cx);
            activity
                .waiting_sessions()
                .iter()
                .copied()
                .find(|id| workspace_ids.contains(id))
                .or_else(|| {
                    self.list_session_groups(cx)
                        .into_iter()
                        .flat_map(|(_, ids)| ids)
                        .find(|id| {
                            workspace_ids.contains(id)
                                && activity.state_of(*id) == ActivityState::WaitingForInput
                        })
                })
        };
        if let Some(id) = target {
            let session = self.runtime.read(cx).registry.read(cx).session(id).cloned();
            if let Some(session) = session {
                session.update(cx, |terminal, _| terminal.snap_to_bottom());
            }
            self.navigate_to_session(id, window, cx);
        }
    }

    pub(crate) fn toggle_tree_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_available() {
            return;
        }
        let target = match self.mode {
            ViewMode::Grid => self.last_mode,
            ViewMode::Project => ViewMode::List,
            ViewMode::List => ViewMode::Project,
        };
        self.act(Action::Mode(target), window, cx);
    }

    pub(crate) fn toggle_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_available() {
            return;
        }
        let target = if self.mode == ViewMode::Grid {
            self.last_mode
        } else {
            ViewMode::Grid
        };
        self.act(Action::Mode(target), window, cx);
    }

    pub(crate) fn add_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.navigation_available() {
            self.act(Action::AddTerminal, window, cx);
            self.focus.focus(window, cx);
        }
    }

    pub(crate) fn add_project_from_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.navigation_available() {
            self.act(Action::AddProject, window, cx);
        }
    }

    pub(crate) fn palette_has_run_script(&self, cx: &App) -> bool {
        self.selected_worktree().is_some_and(|(idx, _)| {
            cx.global::<SettingsState>()
                .store
                .projects
                .get(idx)
                .is_some_and(|project| {
                    !project.archived
                        && project
                            .scripts
                            .run
                            .as_deref()
                            .is_some_and(|script| !script.trim().is_empty())
                })
        })
    }

    pub(crate) fn palette_has_diff(&self, cx: &App) -> bool {
        matches!(self.selection, Some(Selection::Session(id)) if self.runtime.read(cx).registry.read(cx).meta(id).is_some())
    }

    pub(crate) fn run_selected_script_from_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.navigation_available() || !self.palette_has_run_script(cx) {
            return;
        }
        if let Some((idx, path)) = self.selected_worktree() {
            if let Some(project_path) = cx
                .global::<SettingsState>()
                .store
                .projects
                .get(idx)
                .map(|project| project.path.clone())
            {
                self.act(Action::RunScript(project_path, path), window, cx);
            }
        }
    }

    pub(crate) fn open_selected_diff_from_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.navigation_available() && self.palette_has_diff(cx) {
            if let Some(Selection::Session(id)) = self.selection {
                self.act(Action::OpenDiff(id), window, cx);
            }
        }
    }

    pub(crate) fn add_worktree_terminal_from_palette(&mut self, cx: &mut Context<Self>) {
        if !self.navigation_available() {
            return;
        }
        if let Some((_, path)) = self.selected_worktree() {
            self.runtime
                .update(cx, |runtime, cx| runtime.spawn_wt_shell(&path, cx));
        }
    }

    pub(crate) fn open_archived_projects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.navigation_available() {
            self.act(Action::ArchivedProjects, window, cx);
        }
    }

    /// Resolve a launch target only inside the active workspace snapshot.
    pub(crate) fn selected_worktree(&self) -> Option<(usize, String)> {
        match self.selection.as_ref()? {
            Selection::Worktree(project, path) => self
                .snapshot
                .projects
                .iter()
                .find(|row| row.idx == *project)
                .and_then(|row| row.worktrees.iter().find(|worktree| &worktree.path == path))
                .map(|_| (*project, path.clone())),
            Selection::Session(id) => self.snapshot.projects.iter().find_map(|project| {
                project
                    .worktrees
                    .iter()
                    .find(|worktree| worktree.sessions.contains(id))
                    .map(|worktree| (project.idx, worktree.path.clone()))
            }),
            Selection::Project(_) | Selection::Home(_) => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn selected_session(&self) -> Option<SessionId> {
        match self.selection {
            Some(Selection::Session(id)) => Some(id),
            _ => None,
        }
    }
    fn run_script(&mut self, project_path: String, path: String, cx: &mut Context<Self>) {
        let valid_target = self.snapshot.projects.iter().any(|project| {
            cx.global::<SettingsState>()
                .store
                .projects
                .get(project.idx)
                .is_some_and(|stored| stored.path == project_path)
                && project
                    .worktrees
                    .iter()
                    .any(|worktree| worktree.path == path)
        });
        if !valid_target {
            return;
        }
        let toast = self.runtime.read(cx).toast.clone();
        let previous_error = toast.read(cx).current().map(|message| message.created);
        if !self.runtime.update(cx, |runtime, cx| {
            runtime.spawn_run_script(&project_path, &path, cx)
        }) {
            if let Some(message) = toast.read(cx).current().filter(|message| {
                message.kind == crate::entities::toast::ToastKind::Error
                    && Some(message.created) != previous_error
            }) {
                self.content_error = Some(message.message.clone());
                cx.notify();
            }
            return;
        }
        if self.mode == ViewMode::Grid {
            self.mode = self.last_mode;
        }
        self.select_active_session(cx);
        self.content_error = None;
        cx.notify();
    }
    pub(crate) fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.act(Action::ToggleSidebar, window, cx);
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
        let was_project_flyout = self.project_flyout.is_some();
        if !matches!(
            &action,
            Action::ProjectFlyout(_)
                | Action::ProjectFlyoutLaunchOptions(_)
                | Action::Launch(..)
                | Action::Close(_)
                | Action::ConfirmClose(_)
                | Action::Cancel
        ) {
            self.dismiss_project_context(cx);
            self.track_project_flyout_pointer(window.mouse_position(), window, cx);
        }
        match action {
            Action::ProjectFlyout(target) => {
                if self.project_flyout.as_ref() == Some(&target) && self.project_flyout_hover_open {
                    self.project_flyout_hover_open = false;
                    self.project_flyout_hide_task = None;
                    self.project_flyout_focus.focus(window, cx);
                    cx.notify();
                } else if self.project_flyout.as_ref() == Some(&target) {
                    self.close_project_flyout(window, cx);
                } else if self.sidebar_context_is_active(&target, cx) {
                    self.menu = None;
                    if let Some(bounds) = self.sidebar_context_trigger(&target) {
                        self.project_flyout_bounds = bounds;
                    }
                    self.project_flyout_hover_open = false;
                    self.project_flyout_hide_task = None;
                    self.project_flyout_launch_path = None;
                    self.project_flyout_launch_error = None;
                    self.project_flyout = Some(target);
                    self.project_flyout_index = 0;
                    self.project_flyout_scroll = ScrollHandle::new();
                    self.project_flyout_focus.focus(window, cx);
                    cx.notify();
                }
            }
            Action::ToggleSidebar => {
                let collapsed = !self.is_collapsed(cx);
                self.drag = None;
                self.menu = None;
                self.menu_return_focus = None;
                self.menu_opened_by_hover = false;
                SettingsState::update(cx, |store| store.sidebar_collapsed = collapsed);
                self.collapse_focus.focus(window, cx);
                cx.notify();
            }
            Action::OpenWorkspaces => {
                if let Some(selector) = self.workspace_selector.clone() {
                    let return_focus = self.compact_workspace_focus.clone();
                    cx.defer_in(window, move |_, window, cx| {
                        selector.update(cx, |selector, cx| {
                            selector.open_compact_menu(return_focus, window, cx);
                        });
                    });
                }
                cx.notify();
            }
            Action::AddProject => self.add_project(window, cx),
            Action::ToggleProject(path) => {
                if self.project_path_is_active(&path, cx) {
                    let leaving_project_panel = self.project_panel.is_some();
                    self.dismiss_project_panel_for_navigation();
                    if !self.collapsed_projects.insert(path.clone()) {
                        self.collapsed_projects.remove(&path);
                    }
                    cx.notify();
                    if leaving_project_panel && self.project_panel.is_none() {
                        self.focus.focus(window, cx);
                    }
                }
            }
            Action::ArchivedProjects => {
                self.open_project_panel(projects::Page::Archived, window, cx);
            }
            Action::NewMultiProjectSession => {
                if self.navigation_available() {
                    cx.emit(SidebarEvent::NewMultiProjectSessionRequested);
                }
            }
            Action::OpenSettings => {
                if self.navigation_available() {
                    cx.emit(SidebarEvent::SettingsRequested);
                }
            }
            Action::EditProject(path) => {
                if !self.project_path_is_active(&path, cx) {
                    return;
                }
                self.open_project_panel(projects::Page::Edit(path), window, cx);
            }
            Action::MoveProject(path) => {
                if !self.project_path_is_active(&path, cx) {
                    return;
                }
                self.open_project_panel(projects::Page::Move(path), window, cx);
            }
            Action::Select(s) => {
                let leaving_project_panel = self.project_panel.is_some();
                self.select(s.clone(), cx);
                let view = match s {
                    Selection::Session(id) => self.terminal_views.get(&id),
                    Selection::Home(id) => self.home_terminal_views.get(&id),
                    _ => None,
                };
                if let Some(view) = view {
                    view.focus_handle(cx).focus(window, cx);
                } else if leaving_project_panel || was_project_flyout {
                    self.focus.focus(window, cx);
                }
            }
            Action::Mode(mode) => {
                let leaving_project_panel = self.project_panel.is_some();
                self.dismiss_project_panel_for_navigation();
                change_mode(&mut self.mode, &mut self.last_mode, mode);
                self.menu = None;
                if leaving_project_panel {
                    self.focus.focus(window, cx);
                }
            }
            Action::Menu(i) => {
                if self.menu == Some(i) {
                    if self.menu_opened_by_hover {
                        self.menu_opened_by_hover = false;
                        self.menu_return_focus = self.project_menu_focus.get(&i).cloned();
                        self.menu_focus.focus(window, cx);
                    } else {
                        self.close_menu(window, cx);
                    }
                } else {
                    self.menu = Some(i);
                    self.menu_opened_by_hover = false;
                    if let Some(bounds) = self.project_menu_bounds.get(&i) {
                        self.menu_trigger_bounds = bounds.clone();
                    }
                    self.menu_index = 0;
                    self.menu_return_focus = if was_project_flyout {
                        self.project_paths
                            .get(&i)
                            .and_then(|path| self.project_toggle_focus.get(path))
                            .cloned()
                    } else {
                        self.project_menu_focus.get(&i).cloned()
                    };
                    self.menu_focus.focus(window, cx);
                }
            }
            Action::NewWorktree(path) => {
                if !self.project_path_is_active(&path, cx) {
                    return;
                }
                if let Err(reason) = crate::project_service::worktree_prerequisite(&path) {
                    self.menu = None;
                    self.content_error = Some(reason.into());
                    cx.notify();
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
            Action::InitializeGit(path) => {
                if !self.project_path_is_active(&path, cx) || self.git_init_pending.is_some() {
                    return;
                }
                self.menu = None;
                self.git_init_pending = Some(path.clone());
                self.content_error = None;
                cx.notify();
                cx.spawn(async move |this, cx| {
                    let initialized_path = path.clone();
                    let result = cx
                        .background_executor()
                        .spawn(async move {
                            grove_core::git::init_if_needed(&path)
                                .map_err(|error| error.to_string())
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        this.git_init_pending = None;
                        match result {
                            Ok(()) => {
                                this.worktree_readiness.remove(&initialized_path);
                                let service = this.runtime.read(cx).projects.clone();
                                service.update(cx, |service, cx| {
                                    service.project_git_initialized(cx);
                                });
                                this.content_error = None;
                            }
                            Err(error) => {
                                this.content_error =
                                    Some(format!("Could not initialize Git: {error}"));
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            Action::RefreshGit(path) => {
                if !self.project_path_is_active(&path, cx) {
                    return;
                }
                self.menu = None;
                self.worktree_readiness.remove(&path);
                self.queue_worktree_readiness(path, cx);
                cx.notify();
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
            Action::ProjectFlyoutLaunchOptions(path) => {
                if self.project_flyout.is_some() {
                    self.project_flyout_launch_path = Some(path);
                    self.project_flyout_launch_error = None;
                    self.project_flyout_hover_open = false;
                    self.project_flyout_hide_task = None;
                    let focus = self.project_flyout_launch_focus.clone();
                    cx.defer_in(window, move |_, window, cx| focus.focus(window, cx));
                }
            }
            Action::Launch(i, path, agent) => {
                if self.launch(i, path, agent, cx) && was_project_flyout {
                    self.dismiss_project_context(cx);
                    self.focus_active_session_after_palette(window, cx);
                }
            }
            Action::RunScript(project_path, path) => self.run_script(project_path, path, cx),
            Action::Close(id) => {
                if self.project_flyout.as_ref() == Some(&SidebarContext::MultiProject) {
                    self.project_flyout_hide_task = None;
                }
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
                } else {
                    // The confirmation controls disappear after closing the session.
                    // Keep keyboard shortcuts routed through the mounted sidebar.
                    self.focus.focus(window, cx);
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
            Action::OpenDiff(id) => {
                let Some(path) = self
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .meta(id)
                    .map(|meta| meta.wt_path.clone())
                else {
                    return;
                };
                let path = crate::paths::normalize_wt_path(&path).to_string();
                let mode = cx.global::<SettingsState>().store.diff_mode;
                let viewer = cx.new(|cx| DiffViewerState::new(path, mode, cx));
                self.diff_observer = Some(cx.observe(&viewer, |_, _, cx| cx.notify()));
                self.diff_viewer = Some(viewer);
                self.diff_return_focus = window.focused(cx);
                self.diff_focus.focus(window, cx);
            }
            Action::CloseDiff => {
                self.diff_viewer = None;
                self.diff_observer = None;
                if let Some(focus) = self.diff_return_focus.take() {
                    focus.focus(window, cx);
                } else {
                    self.focus.focus(window, cx);
                }
            }
            Action::SelectDiffFile(path) => {
                if let Some(viewer) = &self.diff_viewer {
                    viewer.update(cx, |viewer, cx| viewer.select(path, cx));
                }
            }
            Action::RefreshDiff => {
                if let Some(viewer) = &self.diff_viewer {
                    viewer.update(cx, DiffViewerState::load_files);
                }
            }
            Action::AddTerminal => {
                if self.mode == ViewMode::Grid {
                    self.mode = self.last_mode;
                }
                let previous_count = self
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .home_terminal_count();
                self.runtime.update(cx, Runtime::new_home_terminal);
                let new_id = {
                    let registry = self.runtime.read(cx).registry.read(cx);
                    (registry.home_terminal_count() > previous_count)
                        .then(|| registry.home_terminals().last().map(|meta| meta.id))
                        .flatten()
                };
                if let Some(id) = new_id {
                    self.terminal_owners.insert(id, self.active_workspace);
                    self.selection = Some(Selection::Home(id));
                    self.terminals_collapsed = false;
                }
            }
            Action::FoldTerminals => self.terminals_collapsed = !self.terminals_collapsed,
            Action::CloseHome(id) => {
                self.pending_home_close = Some(id);
                self.confirmation_return_focus = window.focused(cx);
                self.cancel_focus.focus(window, cx);
            }
            Action::ConfirmHome(id) => {
                let from_canvas = self.canvas_close_anchor.take().is_some();
                let was_selected = self.selection == Some(Selection::Home(id));
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
                if was_selected {
                    self.selection = None;
                }
                if from_canvas || was_selected {
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
        let failed = registry
            .session(meta.id)
            .is_some_and(|t| t.read(cx).spawn_error().is_some());
        let pending_attach = registry
            .session(meta.id)
            .is_some_and(|t| t.read(cx).is_pending_attach());
        managed_session_status(
            failed,
            pending_attach,
            runtime.activity.read(cx).state_of(meta.id),
        )
    }
    fn confirmation(&self, label: &str, action: Action, cx: &mut Context<Self>) -> AnyElement {
        let session_close = matches!(&action, Action::ConfirmClose(_));
        let wrap_context = |text: &str| {
            text.chars()
                .map(|character| character.to_string())
                .collect::<Vec<_>>()
                .join("\u{200b}")
        };
        let session_meta = match &action {
            Action::ConfirmClose(id) => self.runtime.read(cx).registry.read(cx).meta(*id).cloned(),
            _ => None,
        };
        let verb = match &action {
            Action::ConfirmClose(_) => "Close session",
            Action::ConfirmHome(_) => "Close terminal",
            Action::ConfirmRemove(_) => "Remove project",
            _ => "Confirm",
        };
        let confirm_button = self
            .control("confirm-close", verb, action, cx)
            .debug_selector(|| "confirm-close".into())
            .w_auto()
            .px(rpx(SPACE_LG))
            .text_color(c::RED())
            .when(session_close, |button| {
                button
                    .text_size(rpx(TEXT_SMALL))
                    .min_h(rpx(CONTROL_H))
                    .border_1()
                    .border_color(c::alpha(c::RED(), 0.35))
                    .bg(c::alpha(c::RED(), 0.1))
                    .gap(rpx(SPACE_SM))
                    .child(icon("check", ICON_XS, c::RED()))
            })
            .child(verb);
        let cancel_button = self
            .control("cancel-close", "Cancel", Action::Cancel, cx)
            .debug_selector(|| "cancel-close".into())
            .w_auto()
            .px(rpx(SPACE_LG))
            .when(session_close, |button| {
                button
                    .text_size(rpx(TEXT_SMALL))
                    .min_h(rpx(CONTROL_H))
                    .border_1()
                    .border_color(c::BORDER())
                    .bg(c::SURFACE_RAISED())
                    .gap(rpx(SPACE_SM))
                    .child(icon("close", ICON_XS, c::FG_DIM()))
            })
            .child("Cancel");
        let action_row = div()
            .flex()
            .min_w_0()
            .when(session_close, gpui::Styled::justify_end)
            .flex_wrap()
            .when(!session_close, |row| row.mt(rpx(SPACE_SM)))
            .gap(rpx(if session_close { SPACE_MD } else { SPACE_LG }));
        let action_row = if session_close {
            action_row.child(cancel_button).child(confirm_button)
        } else {
            action_row.child(confirm_button).child(cancel_button)
        };
        let confirmation = div()
            .id("sidebar-confirmation")
            .debug_selector(|| "sidebar-confirmation".into())
            .role(gpui::Role::Dialog)
            .aria_label(verb)
            .aria_description(label.to_string())
            .text_color(c::FG())
            .flex()
            .flex_col()
            .gap(rpx(if session_close { SPACE_MD } else { SPACE_2XL }))
            .p(rpx(if session_close { SPACE_2XL } else { SPACE_3XL }))
            .m(rpx(SPACE_SM))
            .rounded(rpx(RADIUS_GROUP))
            .min_w_0()
            .whitespace_normal()
            .border_1()
            .border_color(c::BORDER())
            .bg(c::SURFACE_RAISED())
            .child(if let Some(meta) = session_meta {
                div()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .text_size(rpx(TEXT_TITLE))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(format!("Close {}?", wrap_context(&meta.label))),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(wrap_context(&meta.project)),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_BODY))
                            .text_color(c::FG_DIM())
                            .child("Its process will stop. The worktree stays on disk."),
                    )
                    .into_any_element()
            } else {
                div()
                    .child(label.replace('/', "/\u{200b}"))
                    .into_any_element()
            })
            .child(action_row);
        motion::slow(confirmation, format!("sidebar-confirmation-{verb}"), cx)
    }
    fn confirmation_popup(
        &self,
        label: &str,
        action: Action,
        trigger: gpui::Bounds<gpui::Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let preferred_width = if matches!(&action, Action::ConfirmClose(_)) {
            SESSION_CLOSE_W
        } else {
            SIDEBAR_W
        };
        let width = preferred_width
            .min((f32::from(window.viewport_size().width) / scale - SPACE_LG * 2.0).max(0.0));
        gpui::anchored()
            .position_mode(gpui::AnchoredPositionMode::Window)
            .position(gpui::point(trigger.right(), trigger.top()))
            .snap_to_window_with_margin(gpui::px(SPACE_LG * scale))
            .child(
                div()
                    .w(rpx(width))
                    .child(self.confirmation(label, action, cx)),
            )
            .into_any_element()
    }
    fn session_row(
        &self,
        meta: &SessionMeta,
        list: bool,
        number: Option<usize>,
        diff_focused: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (status, color) = self.status(meta, cx);
        let id = meta.id;
        let title = session_display_title(
            meta,
            self.runtime
                .read(cx)
                .registry
                .read(cx)
                .session(id)
                .and_then(|session| session.read(cx).title()),
        );
        let label = if list {
            format!("{} · {}", title, meta.project)
        } else if multi_project::project_names(meta).is_some() {
            format!(
                "{title} · {}",
                multi_project::root_details(meta, &self.snapshot)
            )
        } else {
            title.clone()
        };
        let row = if list {
            let selected = self.selection == Some(Selection::Session(id));
            let attention = matches!(status, "Needs you" | "Failed");
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
            let since = self
                .runtime
                .read(cx)
                .activity
                .read(cx)
                .since_of(id)
                .unwrap_or(meta.spawned_at);
            let age = session_age_label(since, Instant::now());
            let roots = session_root_inventory(meta, &self.snapshot);
            let git = self.runtime.read(cx).tree.read(cx).git_states();
            let diff = git
                .get(crate::paths::normalize_wt_path(&meta.wt_path))
                .map_or_else(
                    || ("Git status unavailable".into(), false),
                    session_diff_status,
                );
            let full_context =
                branch.map_or_else(|| context.clone(), |branch| format!("{context} · {branch}"));
            self.row(
                format!("session-{}", id.raw()),
                format!(
                    "{title} · {full_context} · {status} · {} · {age} · {} session{}",
                    diff.0,
                    meta.agent.label(),
                    number
                        .filter(|number| *number <= 9)
                        .map_or_else(String::new, |number| format!(" · shortcut {number}"))
                ),
                selected,
                Action::Select(Selection::Session(id)),
                cx,
            )
            .relative()
            .group("session-row")
            .h_auto()
            .min_h(rpx(ROW_H))
            .px(rpx(SPACE_LG))
            .py(rpx(SPACE_MD))
            .rounded(rpx(RADIUS_GROUP))
            .border_1()
            .border_color(if attention {
                c::AMBER()
            } else {
                c::alpha(c::BORDER(), 0.0)
            })
            .bg(if selected {
                c::alpha(c::FG(), 0.14)
            } else if attention {
                c::AMBER_ROW_TINT()
            } else {
                c::alpha(c::BG(), 0.0)
            })
            .items_start()
            .gap(rpx(SPACE_MD))
            .child(
                div()
                    .id(("session-number", id.raw()))
                    .debug_selector(move || format!("session-number-{}", id.raw()))
                    .w(rpx(10.0))
                    .flex_shrink_0()
                    .text_size(rpx(TEXT_MICRO))
                    .line_height(rpx(SESSION_TITLE_LINE_H))
                    .text_color(c::FG_MUTE())
                    .font_family(crate::fonts::UI_FAMILY)
                    .child(
                        number
                            .filter(|number| *number <= 9)
                            .map_or_else(String::new, |number| number.to_string()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .relative()
                    .gap(rpx(SPACE_XS))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .min_w_0()
                            .gap(rpx(SPACE_SM))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_size(rpx(TEXT_BODY))
                                    .line_height(rpx(SESSION_TITLE_LINE_H))
                                    .id(("session-list-title", id.raw()))
                                    .debug_selector(move || {
                                        format!("session-list-title-{}", id.raw())
                                    })
                                    .child(title),
                            )
                            .child(motion::fast(
                                div()
                                    .flex_shrink_0()
                                    .text_size(rpx(TEXT_MICRO))
                                    .line_height(rpx(SESSION_TITLE_LINE_H))
                                    .id(("session-list-status", id.raw()))
                                    .debug_selector(move || {
                                        format!("session-list-status-{}", id.raw())
                                    })
                                    .text_color(color)
                                    .child(status),
                                format!("session-status-list-{}-{status}", id.raw()),
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .min_w_0()
                            .gap(rpx(SPACE_XS))
                            .pr(rpx(CONTROL_H + SPACE_XS))
                            .text_size(rpx(TEXT_MICRO))
                            .line_height(rpx(SESSION_META_LINE_H))
                            .text_color(c::FG_DIM())
                            .id(("session-list-context-line", id.raw()))
                            .tooltip({
                                let context = full_context.clone();
                                move |window, cx| {
                                    crate::views::components::tooltip(context.clone(), window)
                                        .build(window, cx)
                                }
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(crate::fonts::UI_FAMILY)
                                    .font_weight(gpui::FontWeight::NORMAL)
                                    .id(("session-list-context", id.raw()))
                                    .debug_selector(move || {
                                        format!("session-list-context-{}", id.raw())
                                    })
                                    .child(context),
                            )
                            .when_some(branch, |line, branch| {
                                line.child(div().flex_shrink_0().child("·")).child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .font_family(crate::fonts::UI_FAMILY)
                                        .id(("session-list-branch", id.raw()))
                                        .debug_selector(move || {
                                            format!("session-list-branch-{}", id.raw())
                                        })
                                        .child(branch.to_string()),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .min_w_0()
                            .gap(rpx(SPACE_SM))
                            .pr(rpx(CONTROL_H + SPACE_XS))
                            .text_size(rpx(TEXT_MICRO))
                            .line_height(rpx(SESSION_META_LINE_H))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_color(c::FG_DIM())
                            .when_some(Some(diff), |line, (label, actionable)| {
                                line.child(div().flex_1().min_w_0().child(if actionable {
                                    self.control(
                                        ("diff-chip-open", id.raw()),
                                        format!("Open changes in {}", meta.wt_path),
                                        Action::OpenDiff(id),
                                        cx,
                                    )
                                    .debug_selector(move || format!("diff-chip-open-{}", id.raw()))
                                    .when_some(
                                        self.session_diff_focus.get(&id),
                                        gpui::InteractiveElement::track_focus,
                                    )
                                    .w_full()
                                    .min_w_0()
                                    .h_auto()
                                    .px(rpx(0.0))
                                    .justify_start()
                                    .text_color(c::GREEN())
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .id(("session-list-diff-label", id.raw()))
                                            .debug_selector(move || {
                                                format!("session-list-diff-label-{}", id.raw())
                                            })
                                            .child(label),
                                    )
                                    .into_any_element()
                                } else {
                                    div().truncate().child(label).into_any_element()
                                }))
                            })
                            .child(div().flex_shrink_0().child(age))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .gap(rpx(SPACE_XS))
                                    .child(icon(meta.agent.icon_name(), ICON_XS, c::FG_DIM()))
                                    .child(meta.agent.label()),
                            ),
                    )
                    .child(
                        self.control(
                            ("close-session", id.raw()),
                            format!("Close {} in {}", meta.label, meta.project),
                            Action::Close(id),
                            cx,
                        )
                        .debug_selector(move || format!("close-session-{}", id.raw()))
                        .absolute()
                        .right_0()
                        .top(rpx(SESSION_TITLE_LINE_H
                            + SPACE_XS
                            + SESSION_META_LINE_H
                            + SPACE_XS
                            + SESSION_META_LINE_H
                            - CONTROL_H))
                        .opacity(0.0)
                        .group_hover("session-row", |button| button.opacity(1.0))
                        .focus_visible(|button| button.opacity(1.0))
                        .when_some(
                            self.session_close_bounds.get(&id).cloned(),
                            |button, bounds| {
                                button.child(
                                    gpui::canvas(
                                        move |rect, _, _| bounds.set(rect),
                                        |_, (), _, _| {},
                                    )
                                    .absolute()
                                    .inset_0(),
                                )
                            },
                        )
                        .child(icon("close", ICON_XS, c::FG_DIM())),
                    )
                    .children(roots.into_iter().map(|root| {
                        div()
                            .truncate()
                            .text_size(rpx(TEXT_MICRO))
                            .line_height(rpx(SESSION_META_LINE_H))
                            .font_family(crate::fonts::UI_FAMILY)
                            .text_color(c::FG_DIM())
                            .child(root)
                    })),
            )
        } else {
            let selected = self.selection == Some(Selection::Session(id));
            let git = self.runtime.read(cx).tree.read(cx).git_states();
            let diff = git
                .get(crate::paths::normalize_wt_path(&meta.wt_path))
                .map(|state| (state.added, state.removed, state.dirty));
            let diff_label = match diff {
                Some((added, removed, _)) if added > 0 || removed > 0 => {
                    format!("Worktree changes: +{added} −{removed}")
                }
                Some((_, _, true)) => "Worktree changes".into(),
                Some(_) => "Worktree clean".into(),
                None => "Git status unavailable".into(),
            };
            let diff_element = match diff {
                Some((added, removed, dirty)) if dirty || added > 0 || removed > 0 => self
                    .control(
                        ("diff-chip-open", id.raw()),
                        format!("{diff_label}; open changes in {}", meta.wt_path),
                        Action::OpenDiff(id),
                        cx,
                    )
                    .debug_selector(move || format!("diff-chip-open-{}", id.raw()))
                    .when_some(
                        self.session_diff_focus.get(&id),
                        gpui::InteractiveElement::track_focus,
                    )
                    .w_auto()
                    .h_auto()
                    .justify_start()
                    .gap(rpx(SPACE_MD))
                    .font_family(crate::fonts::UI_FAMILY)
                    .text_size(rpx(HIERARCHY_META_TEXT))
                    .when(added > 0 || removed > 0, |diff| {
                        diff.when(added > 0, |diff| {
                            diff.child(div().text_color(c::GREEN()).child(format!("+{added}")))
                        })
                        .when(removed > 0, |diff| {
                            diff.child(div().text_color(c::RED()).child(format!("−{removed}")))
                        })
                    })
                    .when(added == 0 && removed == 0, |diff| {
                        diff.text_color(c::GREEN()).child("Changes")
                    })
                    .into_any_element(),
                Some(_) => div()
                    .font_family(crate::fonts::UI_FAMILY)
                    .text_size(rpx(HIERARCHY_META_TEXT))
                    .text_color(c::FG_DIM())
                    .child("clean")
                    .into_any_element(),
                None => div()
                    .font_family(crate::fonts::UI_FAMILY)
                    .text_size(rpx(HIERARCHY_META_TEXT))
                    .text_color(c::FG_DIM())
                    .child("—")
                    .into_any_element(),
            };
            self.row(
                format!("session-{}", id.raw()),
                format!(
                    "{label} · {} session · {diff_label} · {status}{}",
                    meta.agent.label(),
                    number
                        .filter(|n| *n <= 9)
                        .map_or_else(String::new, |n| format!(" · shortcut {n}"))
                ),
                selected,
                Action::Select(Selection::Session(id)),
                cx,
            )
            .when(diff_focused && !selected, |row| row.bg(c::BG_HOVER()))
            .relative()
            .group("session-row")
            .h_auto()
            .min_h(rpx(SESSION_TREE_ROW_H))
            .px(rpx(HIERARCHY_INSET))
            .py(rpx(HIERARCHY_INSET))
            .gap(rpx(HIERARCHY_GAP))
            .items_start()
            .rounded(rpx(SESSION_ROW_RADIUS))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_XS))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .min_w_0()
                            .gap(rpx(SPACE_SM))
                            .line_height(rpx(SESSION_META_LINE_H))
                            .child(
                                div()
                                    .id(("session-agent", id.raw()))
                                    .debug_selector(move || format!("session-agent-{}", id.raw()))
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap(rpx(HIERARCHY_GAP))
                                    .text_size(rpx(SESSION_LAUNCHER_SIZE))
                                    .text_color(c::BLUE())
                                    .child(
                                        div()
                                            .w(rpx(HIERARCHY_ICON_SLOT))
                                            .h(rpx(SESSION_META_LINE_H))
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .justify_start()
                                            .child(icon(
                                                meta.agent.icon_name(),
                                                SESSION_LAUNCHER_SIZE,
                                                c::BLUE(),
                                            )),
                                    )
                                    .child(
                                        div()
                                            .id(("session-agent-label", id.raw()))
                                            .debug_selector(move || {
                                                format!("session-agent-label-{}", id.raw())
                                            })
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .child(match meta.agent {
                                                Agent::Claude => "Claude Code",
                                                Agent::Codex => "Codex",
                                                Agent::OpenCode => "OpenCode",
                                                Agent::Terminal => "Terminal",
                                            }),
                                    ),
                            )
                            .child(motion::fast(
                                div()
                                    .id(("session-tree-status", id.raw()))
                                    .debug_selector(move || {
                                        format!("session-tree-status-{}", id.raw())
                                    })
                                    .flex_shrink_0()
                                    .text_size(rpx(TEXT_MICRO))
                                    .text_color(color)
                                    .flex()
                                    .items_center()
                                    .gap(rpx(SPACE_SM))
                                    .when(status == "Working", |label| {
                                        label.child(
                                            div()
                                                .size(rpx(DOT_SM))
                                                .flex_shrink_0()
                                                .rounded_full()
                                                .bg(color),
                                        )
                                    })
                                    .child(status),
                                format!("session-status-tree-{}-{status}", id.raw()),
                                cx,
                            )),
                    )
                    .child(div().pl(rpx(HIERARCHY_ICON_SLOT + HIERARCHY_GAP)).child(
                        if multi_project::project_names(meta).is_some() {
                            self.multi_project_content(meta, title)
                        } else {
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(rpx(SPACE_XS))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .min_w_0()
                                        .gap(rpx(SPACE_SM))
                                        .pr(rpx(SPACE_20))
                                        .line_height(rpx(SESSION_TITLE_LINE_H))
                                        .child(
                                            div()
                                                .id(("session-tree-title", id.raw()))
                                                .debug_selector(move || {
                                                    format!("session-tree-title-{}", id.raw())
                                                })
                                                .flex_1()
                                                .min_w_0()
                                                .whitespace_normal()
                                                .line_clamp(2)
                                                .text_ellipsis()
                                                .font_weight(gpui::FontWeight::NORMAL)
                                                .text_size(rpx(TEXT_SMALL))
                                                .child(title),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .min_w_0()
                                        .gap(rpx(SPACE_SM))
                                        .id(("session-tree-metadata", id.raw()))
                                        .debug_selector(move || {
                                            format!("session-tree-metadata-{}", id.raw())
                                        })
                                        .line_height(rpx(SESSION_META_LINE_H))
                                        .pr(rpx(CONTROL_H))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .justify_start()
                                                .child(diff_element),
                                        ),
                                )
                                .into_any_element()
                        },
                    )),
            )
            .child(
                self.control(
                    ("close-session", id.raw()),
                    format!("Close {} in {}", meta.label, meta.project),
                    Action::Close(id),
                    cx,
                )
                .absolute()
                .right_0()
                .bottom_0()
                .debug_selector(move || format!("session-tree-close-{}", id.raw()))
                .opacity(0.0)
                .group_hover("session-row", |button| button.opacity(1.0))
                .focus_visible(|button| button.opacity(1.0))
                .when_some(
                    self.session_close_bounds.get(&id).cloned(),
                    |button, bounds| {
                        button.child(
                            gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                                .absolute()
                                .inset_0(),
                        )
                    },
                )
                .child(icon("close", ICON_XS, c::FG_DIM())),
            )
        };
        let row = if list {
            let selected = self.selection == Some(Selection::Session(id));
            let attention = matches!(status, "Needs you" | "Failed");
            let idle_fill = if attention {
                c::AMBER_ROW_TINT()
            } else {
                c::alpha(c::BG(), 0.0)
            };
            let selected_fill = c::alpha(c::FG(), 0.14);
            motion::background(
                row,
                format!("session-row-fill-{}-{selected}", id.raw()),
                if selected { idle_fill } else { selected_fill },
                if selected { selected_fill } else { idle_fill },
                Duration::from_millis(MOTION_FAST_MS),
                cx,
            )
        } else {
            row.into_any_element()
        };
        let mut result = div().when(list, |d| d.px(rpx(SPACE_LG))).child(row);
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
            if let Some(bounds) = self.session_close_bounds.get(&id) {
                result = result.child(
                    div()
                        .id(("session-close-popup", id.raw()))
                        .debug_selector(move || format!("session-close-popup-{}", id.raw()))
                        .child(gpui::deferred(self.confirmation_popup(
                            &format!(
                        "Close {} in {}? Its process will stop. The worktree stays on disk.",
                        meta.label, meta.project
                    ),
                            Action::ConfirmClose(id),
                            bounds.get(),
                            window,
                            cx,
                        ))),
                );
            }
        }
        result.into_any_element()
    }
    fn project_popup(&self, idx: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(project) = cx.global::<SettingsState>().store.projects.get(idx) else {
            return div().into_any_element();
        };
        let path = project.path.clone();
        let readiness = self
            .worktree_readiness
            .get(&path)
            .and_then(|(_, value)| *value);
        let mut actions = vec![
            ("Edit project", "edit", Action::EditProject(path.clone())),
            (
                "Move to workspace…",
                "folder",
                Action::MoveProject(path.clone()),
            ),
        ];
        if readiness == Some(WorktreeReadiness::Ready) {
            actions.push(("New worktree", "plus", Action::NewWorktree(path.clone())));
        } else if readiness == Some(WorktreeReadiness::NeedsGit) {
            actions.push((
                "Initialize Git",
                "plus",
                Action::InitializeGit(path.clone()),
            ));
        } else if readiness == Some(WorktreeReadiness::NeedsCommit) {
            actions.push((
                "Check for first commit",
                "restart",
                Action::RefreshGit(path.clone()),
            ));
        }
        actions.extend([
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
        ]);
        let keyboard_actions = actions
            .iter()
            .map(|(_, _, action)| action.clone())
            .collect::<Vec<_>>();
        let action_count = actions.len();
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width = SIDEBAR_W.min(f32::from(window.viewport_size().width) / scale - SPACE_LG * 2.0);
        let mut panel = div()
            .text_color(c::FG())
            .relative()
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
            .child(
                gpui::canvas(
                    {
                        let bounds = self.menu_popup_bounds.clone();
                        move |rect, _, _| bounds.set(rect)
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0(),
            )
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
                                (this.menu_index + action_count - 1) % action_count
                            } else {
                                (this.menu_index + 1) % action_count
                            };
                            this.menu_focus.focus(window, cx);
                            cx.notify();
                            cx.stop_propagation();
                        }
                        "enter" | "space" => {
                            if let Some(action) = keyboard_actions
                                .get(this.menu_index % action_count)
                                .cloned()
                            {
                                this.act(action, window, cx);
                            }
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
        for (index, (label, glyph, action)) in actions.into_iter().enumerate() {
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
            .child(motion::fast(panel, format!("project-menu-enter-{idx}"), cx))
            .into_any_element()
    }
    fn tree(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let visible_sessions = self.visible_session_order(cx);
        let snapshot = self.project_navigation_snapshot(cx);
        let multi = self.multi_project_sessions(cx);
        let mut body = div().flex().flex_col();
        if self.mode == ViewMode::Project || !multi.is_empty() {
            body = body.child(self.multi_project_section(&multi, window, cx));
        }
        if self.mode == ViewMode::Project {
            body = body.child(
                div()
                    .mt(rpx(SPACE_2XL))
                    .mb(rpx(SPACE_SM))
                    .px(rpx(HIERARCHY_INSET))
                    .child(self.navigation_heading(window, cx)),
            );
            if snapshot.projects.is_empty() {
                body = body.child(
                    div()
                        .p(rpx(SPACE_LG))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("No projects yet"),
                );
            }
        }
        for (project_position, project) in snapshot.projects.iter().enumerate() {
            let idx = project.idx;
            let project_path = self
                .project_paths
                .get(&idx)
                .cloned()
                .or_else(|| {
                    cx.global::<SettingsState>()
                        .store
                        .projects
                        .get(idx)
                        .map(|stored| stored.path.clone())
                })
                .unwrap_or_else(|| format!("missing-project-{idx}"));
            let expanded = !self.collapsed_projects.contains(&project_path);
            let previous_collapsed = project_position > 0
                && self
                    .snapshot
                    .projects
                    .get(project_position - 1)
                    .is_some_and(|previous| {
                        self.project_paths
                            .get(&previous.idx)
                            .is_some_and(|path| self.collapsed_projects.contains(path))
                    });
            let mut group = div().flex().flex_col().when(project_position > 0, |group| {
                group
                    .mt(rpx(if previous_collapsed {
                        SPACE_SM
                    } else {
                        PROJECT_GROUP_GAP
                    }))
                    .pt(rpx(if previous_collapsed {
                        SPACE_XS
                    } else {
                        SPACE_LG
                    }))
                    .border_t_1()
                    .border_color(c::alpha(c::FG(), 0.08))
            });
            group = group.child(
                self.row(
                    format!("project-{idx}"),
                    format!("{} project", project.name),
                    false,
                    Action::ToggleProject(project_path.clone()),
                    cx,
                )
                .h(rpx(PROJECT_ROW_H))
                .group("project-row")
                .px(rpx(HIERARCHY_INSET))
                .gap(rpx(HIERARCHY_GAP))
                .text_size(rpx(PROJECT_TEXT))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(c::FG())
                .child(
                    self.control(
                        ("project-folder-toggle", idx),
                        format!(
                            "{} {} project",
                            if expanded { "Collapse" } else { "Expand" },
                            project.name
                        ),
                        Action::ToggleProject(project_path.clone()),
                        cx,
                    )
                    .debug_selector(move || format!("project-folder-toggle-{idx}"))
                    .size(rpx(HIERARCHY_ICON_SLOT))
                    .justify_start()
                    .when_some(
                        self.project_toggle_focus.get(&project_path),
                        gpui::InteractiveElement::track_focus,
                    )
                    .aria_expanded(expanded)
                    .child(motion::fast(
                        icon(
                            if expanded { "folder-open" } else { "folder" },
                            HIERARCHY_ICON,
                            c::FG_DIM(),
                        ),
                        format!("project-disclosure-icon-{idx}-{expanded}"),
                        cx,
                    )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_SM))
                        .child(
                            div()
                                .id(("project-title", idx))
                                .debug_selector(move || format!("project-title-{idx}"))
                                .flex_1()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .truncate()
                                .line_height(rpx(PROJECT_TITLE_LINE_H))
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
                                        crate::views::components::tooltip(
                                            "Not a Git repository",
                                            window,
                                        )
                                        .build(window, cx)
                                    })
                                    .child(icon("no-git", ICON_MD, c::YELLOW())),
                            )
                        }),
                )
                .child(
                    self.control(
                        ("project-menu", idx),
                        format!("Actions for {}", project.name),
                        Action::Menu(idx),
                        cx,
                    )
                    .relative()
                    .size(rpx(HIERARCHY_TRAILING_W))
                    .opacity(0.0)
                    .group_hover("project-row", |button| button.opacity(1.0))
                    .focus_visible(|button| button.opacity(1.0))
                    .when_some(
                        self.project_menu_focus.get(&idx),
                        gpui::InteractiveElement::track_focus,
                    )
                    .child(
                        div()
                            .id(("project-menu-glyph", idx))
                            .debug_selector(move || format!("project-menu-glyph-{idx}"))
                            .size(rpx(ICON_SM))
                            .child(icon("more", ICON_SM, c::FG_DIM())),
                    )
                    .debug_selector(move || format!("project-menu-{idx}"))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.open_menu_on_hover(idx, cx);
                        }
                    }))
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
                group = group.child(self.confirmation(
                    &format!("Remove {} from Grove? Its sessions will stop. The repository and worktrees remain on disk.",project.name),
                    Action::ConfirmRemove(idx),
                    cx,
                ));
            }
            if !expanded {
                body = body.child(group);
                continue;
            }
            if project.worktrees.is_empty() {
                group = group.child(
                    div()
                        .pl(rpx(HIERARCHY_LABEL_INSET))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("Loading worktrees…"),
                );
            }
            for (worktree_position, worktree) in project.worktrees.iter().enumerate() {
                let path = worktree.path.clone();
                let worktree_name = sidebar_worktree_name(&worktree.name, worktree.is_main);
                let project_path = cx
                    .global::<SettingsState>()
                    .store
                    .projects
                    .get(idx)
                    .map(|project| project.path.clone());
                let has_run_script = project.has_run && project_path.is_some();
                let action_count = WORKTREE_LAUNCH_AGENTS.len()
                    + has_run_script as usize
                    + (!worktree.is_main) as usize;
                let launch_width = WORKTREE_ACTION_W * action_count as f32;
                let selection = Selection::Worktree(idx, path.clone());
                let selected = self.selection == Some(selection.clone());
                let focused = self
                    .worktree_focus
                    .get(&path)
                    .is_some_and(|f| f.contains_focused(window, cx));
                let mut launches = div()
                    .absolute()
                    .right_0()
                    .top_0()
                    .w(rpx(launch_width))
                    .h(rpx(CHROME_CONTROL_H))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .opacity(if focused { 1.0 } else { 0.0 })
                    .group_hover("worktree-row", |s| s.opacity(1.0));
                if let Some(project_path) = project_path.filter(|_| has_run_script) {
                    launches = launches.child(
                        self.control(
                            SharedString::from(format!("run-script-{path}")),
                            format!("Run script in {} · {}", project.name, worktree_name),
                            Action::RunScript(project_path, path.clone()),
                            cx,
                        )
                        .size(rpx(WORKTREE_ACTION_W))
                        .debug_selector({
                            let path = path.clone();
                            move || format!("run-script-{path}")
                        })
                        .child(icon("play", ICON_SM, c::FG_DIM())),
                    );
                }
                for (n, agent) in WORKTREE_LAUNCH_AGENTS.into_iter().enumerate() {
                    let agent_name = match agent {
                        Agent::Codex => "Codex",
                        Agent::Claude => "Claude",
                        Agent::OpenCode => "OpenCode",
                        Agent::Terminal => "Terminal",
                    };
                    let label = if self.available[n] {
                        format!(
                            "Start {} in {} · {}",
                            agent_name, project.name, worktree_name
                        )
                    } else {
                        format!("{agent_name} is not installed")
                    };
                    launches = launches.child(
                        self.control(
                            SharedString::from(format!("launch-{path}-{n}")),
                            label,
                            Action::Launch(idx, path.clone(), agent),
                            cx,
                        )
                        .size(rpx(WORKTREE_ACTION_W))
                        .debug_selector({
                            let path = path.clone();
                            move || format!("launch-{path}-{n}")
                        })
                        .when(!self.available[n], |d| {
                            d.tab_index(-1).opacity(OPACITY_DISABLED)
                        })
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
                        .size(rpx(WORKTREE_ACTION_W))
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
                group = group.child(
                    self.row(
                        format!("worktree-{path}"),
                        format!("{} worktree, branch {}", worktree_name, worktree.branch),
                        false,
                        Action::Select(selection),
                        cx,
                    )
                    .relative()
                    .group("worktree-row")
                    .when_some(
                        self.worktree_focus.get(&path),
                        gpui::InteractiveElement::track_focus,
                    )
                    .h(rpx(WORKTREE_ROW_H))
                    .mt(rpx(if worktree_position == 0 {
                        SPACE_XS
                    } else {
                        SPACE_2XL
                    }))
                    .pl(rpx(HIERARCHY_INSET))
                    .pr(rpx(HIERARCHY_INSET))
                    .gap(rpx(HIERARCHY_GAP))
                    .text_size(rpx(TEXT_SMALL))
                    .font_weight(if selected {
                        gpui::FontWeight::SEMIBOLD
                    } else {
                        gpui::FontWeight::MEDIUM
                    })
                    .text_color(c::FG())
                    .child(
                        div()
                            .w(rpx(HIERARCHY_ICON_SLOT))
                            .h(rpx(WORKTREE_TITLE_LINE_H + SPACE_XS + WORKTREE_META_LINE_H))
                            .flex_shrink_0()
                            .flex()
                            .items_start()
                            .justify_start()
                            .child(icon(
                                "git-branch",
                                HIERARCHY_ICON,
                                if selected { c::FG() } else { c::FG_DIM() },
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(rpx(WORKTREE_TITLE_LINE_H + SPACE_XS + WORKTREE_META_LINE_H))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_XS))
                            .id(SharedString::from(format!("worktree-title-{path}")))
                            .debug_selector({
                                let path = path.clone();
                                move || format!("worktree-title-{path}")
                            })
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_ellipsis_middle()
                                    .line_height(rpx(WORKTREE_TITLE_LINE_H))
                                    .flex()
                                    .items_center()
                                    .gap(rpx(SPACE_SM))
                                    .group_hover("worktree-row", move |s| {
                                        s.pr(rpx((launch_width - HIERARCHY_TRAILING_W).max(0.0)))
                                    })
                                    .when(focused, move |s| {
                                        s.pr(rpx((launch_width - HIERARCHY_TRAILING_W).max(0.0)))
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .child(worktree_name.to_owned()),
                                    ),
                            )
                            .when(!worktree.branch.is_empty(), |title| {
                                title.child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_ellipsis_middle()
                                        .text_size(rpx(HIERARCHY_META_TEXT))
                                        .line_height(rpx(WORKTREE_META_LINE_H))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(c::FG_DIM())
                                        .child(worktree.branch.clone()),
                                )
                            }),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("worktree-actions-{path}")))
                            .debug_selector({
                                let path = path.clone();
                                move || format!("worktree-actions-{path}")
                            })
                            .w(rpx(HIERARCHY_TRAILING_W))
                            .h(rpx(CHROME_CONTROL_H))
                            .flex_shrink_0()
                            .flex(),
                    )
                    .child(launches),
                );
                if worktree.sessions.is_empty() {
                    group = group.child(
                        div()
                            .id(SharedString::from(format!("worktree-empty-{path}")))
                            .pl(rpx(HIERARCHY_LABEL_INSET))
                            .py(rpx(SPACE_SM))
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child(
                                div()
                                    .id(SharedString::from(format!("worktree-empty-text-{path}")))
                                    .debug_selector({
                                        let path = path.clone();
                                        move || format!("worktree-empty-{path}")
                                    })
                                    .child("No sessions"),
                            ),
                    );
                }
                for (session_position, id) in worktree.sessions.iter().enumerate() {
                    if let Some(meta) = self.runtime.read(cx).registry.read(cx).meta(*id).cloned() {
                        let number = visible_sessions
                            .iter()
                            .position(|visible| *visible == *id)
                            .map(|index| index + 1);
                        group = group.child(
                            div()
                                .mt(rpx(if session_position == 0 {
                                    SESSION_FIRST_GAP
                                } else {
                                    SPACE_XS
                                }))
                                .child(
                                    self.session_row(
                                        &meta,
                                        false,
                                        number,
                                        self.session_diff_focus
                                            .get(id)
                                            .is_some_and(|focus| focus.is_focused(window)),
                                        window,
                                        cx,
                                    ),
                                ),
                        );
                    }
                }
            }
            body = body.child(group);
        }
        body.into_any_element()
    }
    fn list(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let groups = self.list_session_groups(cx);
        let mut body = div().flex().flex_col().gap(rpx(SPACE_SM));
        let mut number = 0;
        for (group, items) in &groups {
            body = body.child(
                div()
                    .px(rpx(SPACE_LG))
                    .pt(rpx(SPACE_LG))
                    .text_size(rpx(TEXT_SMALL))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(c::FG_DIM())
                    .child(format!("{group} · {}", items.len())),
            );
            for id in items {
                if let Some(meta) = self.runtime.read(cx).registry.read(cx).meta(*id).cloned() {
                    number += 1;
                    body =
                        body.child(self.session_row(&meta, true, Some(number), false, window, cx));
                }
            }
        }
        if number == 0 {
            body = body.child(
                div()
                    .p(rpx(SPACE_2XL))
                    .text_color(c::FG_DIM())
                    .child("No sessions yet. Start an agent from Project view."),
            );
        }
        body.into_any_element()
    }
    fn terminals(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
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
                        .child(motion::fast(
                            icon(
                                if self.terminals_collapsed {
                                    "chev-right"
                                } else {
                                    "chev-down"
                                },
                                ICON_SM,
                                c::FG_DIM(),
                            ),
                            format!("terminal-disclosure-{}", self.terminals_collapsed),
                            cx,
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
                let terminal = {
                    let registry = self.runtime.read(cx).registry.read(cx);
                    let Some(index) = registry
                        .home_terminals()
                        .iter()
                        .position(|row| row.id == id)
                    else {
                        continue;
                    };
                    let Some(terminal) = registry.home_terminal(index) else {
                        continue;
                    };
                    terminal.clone()
                };
                let (title, status, directory) = terminal.update(cx, |terminal, _| {
                    let title = session_display_title(&meta, terminal.title());
                    let status = home_terminal_status(
                        terminal.spawn_error().is_some(),
                        terminal.is_pending_attach(),
                        terminal.alive(),
                    );
                    (
                        title,
                        status,
                        terminal_directory_label(terminal.current_cwd(), terminal.initial_cwd()),
                    )
                });
                panel = panel.child(
                    self.row(
                        format!("home-{}", id.raw()),
                        format!("{title} · {status}"),
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .id(("home-title", id.raw()))
                                    .debug_selector(move || format!("home-title-{}", id.raw()))
                                    .truncate()
                                    .child(title),
                            )
                            .when_some(directory, |column, (kind, cwd)| {
                                column.child(
                                    div()
                                        .id((kind, id.raw()))
                                        .debug_selector(move || format!("{kind}-{}", id.raw()))
                                        .truncate()
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(c::FG_DIM())
                                        .child(cwd),
                                )
                            }),
                    )
                    .child(motion::fast(
                        div()
                            .id(("home-status", id.raw()))
                            .debug_selector(move || format!("home-status-{}", id.raw()))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(if status == "Running" {
                                c::GREEN()
                            } else {
                                c::FG_DIM()
                            })
                            .child(status),
                        format!("home-status-{}-{status}", id.raw()),
                        cx,
                    ))
                    .child(
                        self.control(
                            ("close-home", id.raw()),
                            format!("Close {}", meta.label),
                            Action::CloseHome(id),
                            cx,
                        )
                        .debug_selector(move || format!("close-home-{}", id.raw()))
                        .when_some(
                            self.home_close_bounds.get(&id).cloned(),
                            |button, bounds| {
                                button.child(
                                    gpui::canvas(
                                        move |rect, _, _| bounds.set(rect),
                                        |_, (), _, _| {},
                                    )
                                    .absolute()
                                    .inset_0(),
                                )
                            },
                        )
                        .child(icon("close", ICON_XS, c::FG_DIM())),
                    ),
                );
                if self.pending_home_close == Some(id) && self.canvas_close_anchor.is_none() {
                    if let Some(bounds) = self.home_close_bounds.get(&id) {
                        panel = panel.child(gpui::deferred(self.confirmation_popup(
                            &format!("Close {}? Its shell and running commands will stop. Files remain on disk.", meta.label),
                            Action::ConfirmHome(id), bounds.get(), window, cx,
                        )));
                    }
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
fn navigation_label_width(
    label: &str,
    font_size: gpui::Pixels,
    run: &gpui::TextRun,
    text_system: &gpui::WindowTextSystem,
) -> gpui::Pixels {
    let shaped_width = text_system
        .shape_line(
            label.to_owned().into(),
            font_size,
            std::slice::from_ref(run),
            None,
        )
        .width()
        .ceil();
    // GPUI truncates using separate glyph advances, which can exceed a kerned
    // word's intrinsic width. Reserve that budget before flex layout shrinks it.
    let font_id = text_system.resolve_font(&run.font);
    let glyph_width: gpui::Pixels = label
        .chars()
        .map(|ch| text_system.layout_width(font_id, font_size, ch))
        .sum();
    shaped_width.max(glyph_width.floor())
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(window, cx);
        self.sync_age_timer(cx);
        if self.project_flyout.is_some() {
            let mut keys = HashSet::new();
            for project in self.project_navigation_snapshot(cx).projects {
                keys.insert(format!("overview-{}", project.idx));
                for worktree in project.worktrees {
                    for agent in [Agent::Claude, Agent::Codex, Agent::OpenCode] {
                        keys.insert(format!("launch-{}-{}", agent.label(), worktree.path));
                    }
                    for id in worktree.sessions {
                        keys.insert(format!("session-{}", id.raw()));
                    }
                }
            }
            self.project_flyout_item_focus
                .retain(|key, _| keys.contains(key));
            for key in keys {
                self.project_flyout_item_focus
                    .entry(key)
                    .or_insert_with(|| cx.focus_handle().tab_stop(true));
            }
            let paths = self
                .project_navigation_snapshot(cx)
                .projects
                .iter()
                .flat_map(|project| {
                    project
                        .worktrees
                        .iter()
                        .map(|worktree| worktree.path.clone())
                })
                .collect::<HashSet<_>>();
            self.project_flyout_plus_focus
                .retain(|path, _| paths.contains(path));
            for path in paths {
                self.project_flyout_plus_focus
                    .entry(path)
                    .or_insert_with(|| cx.focus_handle().tab_stop(true));
            }
            if self
                .project_flyout_launch_path
                .as_ref()
                .is_some_and(|path| !self.project_flyout_plus_focus.contains_key(path))
            {
                self.project_flyout_launch_path = None;
                self.project_flyout_launch_error = None;
            }
        }
        let rail_width = self.rail_width(window, cx);
        self.compact_rail = rail_width < SIDEBAR_COMPACT_THRESHOLD;
        let collapsed = self.is_collapsed(cx);
        let navigation = if collapsed {
            div()
                .flex()
                .flex_col()
                .w_full()
                .min_h_full()
                .child(self.collapsed_controls(cx))
                .child(self.collapsed_navigation(window, cx))
                .child(self.collapsed_terminals(window, cx))
                .child(div().flex_1().min_h(rpx(SPACE_LG)))
                .child(self.collapsed_utilities(cx))
                .into_any_element()
        } else if self.mode == ViewMode::List {
            self.list(window, cx)
        } else {
            self.tree(window, cx)
        };
        let settings_control = self.settings_control(cx);
        let terminals = (!collapsed).then(|| self.terminals(window, cx));
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
            .bg(rail_background(cx))
            .text_size(rpx(TEXT_BODY))
            .line_height(rpx(SPACE_3XL))
            .font_weight(gpui::FontWeight::NORMAL)
            .text_color(c::alpha(c::FG(), 0.88))
            .when(collapsed, |rail| rail.pt(rpx(SPACE_LG)))
            .when(!collapsed, |rail| {
                rail.child(
                    div()
                        .id("sidebar-appbar-spacer")
                        .debug_selector(|| "sidebar-appbar-spacer".into())
                        .h(rpx(APPBAR_H))
                        .flex_shrink_0(),
                )
            })
            .child(
                div()
                    .id("sidebar-workspace-header")
                    .debug_selector(|| "sidebar-workspace-header".into())
                    .h(rpx(HEAD_H))
                    .flex_shrink_0()
                    .pl(rpx(SPACE_SM))
                    .pr(rpx(SPACE_SM))
                    .flex()
                    .items_center()
                    .justify_between()
                    .when(!collapsed, |header| {
                        header
                            .child(div().flex_1().min_w_0().when_some(
                                self.workspace_selector.clone(),
                                gpui::ParentElement::child,
                            ))
                            .child(settings_control)
                    })
                    .child(self.collapse_control(cx))
                    .when(collapsed, gpui::Styled::justify_center),
            )
            .when(!collapsed && self.mode != ViewMode::Project, |rail| {
                rail.child(
                    div()
                        .px(rpx(SPACE_3XL))
                        .pt(rpx(SPACE_LG))
                        .child(self.navigation_heading(window, cx)),
                )
            })
            .child(
                div()
                    .id("sidebar-scroll")
                    .debug_selector(|| "sidebar-scroll".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p(rpx(if self.compact_rail {
                        SPACE_SM
                    } else {
                        SPACE_LG
                    }))
                    .when(empty && !collapsed && self.mode != ViewMode::Project, |d| {
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
            .when_some(terminals, gpui::ParentElement::child);
        let rail_visible = self.rail_visible(window, cx);
        let content = self.render_content(window, cx);
        let confirming = self.project_decision
            || self.pending_close.is_some()
            || self.pending_home_close.is_some()
            || self.pending_remove.is_some();
        div()
            .size_full()
            .relative()
            .flex()
            .min_h_0()
            .track_focus(&self.focus)
            .text_size(rpx(TEXT_BODY))
            .text_color(c::FG())
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.divider_move(event, window, cx);
                this.track_project_flyout_pointer(event.position, window, cx);
                if this.menu_opened_by_hover
                    && !this.menu_trigger_bounds.get().contains(&event.position)
                    && !this.menu_popup_bounds.get().contains(&event.position)
                {
                    this.close_menu(window, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.divider_release(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.divider_release(cx);
                }),
            )
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    if let Some(selector) = this.workspace_selector.clone() {
                        if selector
                            .update(cx, |selector, cx| selector.dismiss_hover_menu(window, cx))
                        {
                            cx.stop_propagation();
                            return;
                        }
                    }
                }
                if event.keystroke.key == "escape"
                    && this.project_flyout.is_some()
                    && !this.confirmation_open()
                {
                    if this.collapse_project_launch_options(window, cx) {
                        window.prevent_default();
                        cx.stop_propagation();
                        return;
                    }
                    if this.project_flyout_hover_open
                        && !this.project_flyout_focus.contains_focused(window, cx)
                    {
                        this.dismiss_project_context(cx);
                        this.track_project_flyout_pointer(window.mouse_position(), window, cx);
                    } else {
                        this.close_project_flyout(window, cx);
                        cx.stop_propagation();
                    }
                    return;
                }
                if this.diff_viewer.is_some() {
                    if event.keystroke.key == "escape" {
                        this.act(Action::CloseDiff, window, cx);
                        cx.stop_propagation();
                    }
                    return;
                }
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
            .when(rail_visible, |d| {
                d.child(
                    div()
                        .relative()
                        .h_full()
                        .child(rail)
                        .child(
                            gpui::canvas(
                                {
                                    let bounds = self.rail_bounds.clone();
                                    move |rect, _, _| bounds.set(rect)
                                },
                                |_, (), _, _| {},
                            )
                            .absolute()
                            .inset_0(),
                        )
                        .when(!collapsed, |rail| {
                            rail.child(
                                div()
                                    .id("sidebar-divider")
                                    .debug_selector(|| "sidebar-divider".into())
                                    .absolute()
                                    .top_0()
                                    .right(rpx(-DIVIDER_DRAG_HIT_W / 2.0))
                                    .w(rpx(DIVIDER_DRAG_HIT_W))
                                    .h_full()
                                    .cursor(CursorStyle::ResizeLeftRight)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, event, window, cx| {
                                            this.divider_press(event, window, cx);
                                        }),
                                    ),
                            )
                        })
                        .when(
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
                        ),
                )
            })
            .child(
                div()
                    .id("sidebar-canvas")
                    .debug_selector(|| "sidebar-canvas".into())
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .bg(c::BG())
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
            .when(self.diff_viewer.is_some(), |d| {
                d.child(gpui::deferred(self.render_diff(cx)))
            })
    }
}

/// Prefer live OSC titles, then tmux's discovery title, then the stable label.
fn session_display_title(meta: &SessionMeta, live_title: Option<String>) -> String {
    let live = display_task_title(live_title, "");
    if !live.is_empty() {
        live
    } else {
        display_task_title(meta.restored_title.clone(), &meta.label)
    }
}

/// Strip only separated leading activity glyphs from chrome titles.
fn display_task_title(title: Option<String>, fallback: &str) -> String {
    let Some(title) = title else {
        return fallback.to_string();
    };
    let mut title = title.trim();
    let separator = |ch: char| matches!(ch, '·' | '|' | '-' | '–' | '—' | ':');
    if let Some(first) = title.chars().next() {
        let rest = &title[first.len_utf8()..];
        if (('\u{2800}'..='\u{28ff}').contains(&first) || first == '✳')
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

/// A short, continuously recomputed age for the current activity state.
fn elapsed_short(elapsed: std::time::Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86400)
    }
}

fn session_age_label(since: Instant, now: Instant) -> String {
    format!(
        "for {}",
        elapsed_short(now.saturating_duration_since(since))
    )
}

/// Snapshot data may lag a live session; preserve every root using its path.
fn session_root_inventory(meta: &SessionMeta, snapshot: &TreeSnapshot) -> Vec<String> {
    if meta.context_roots.len() < 2 {
        return Vec::new();
    }
    meta.context_roots
        .iter()
        .map(|root| {
            let known = snapshot
                .projects
                .iter()
                .flat_map(|project| &project.worktrees)
                .find(|worktree| {
                    crate::paths::normalize_wt_path(&worktree.path)
                        == crate::paths::normalize_wt_path(&root.wt_path)
                });
            let name = known.map_or_else(
                || crate::paths::path_basename(&root.wt_path),
                |worktree| worktree.name.clone(),
            );
            let branch = known
                .map(|worktree| worktree.branch.trim())
                .filter(|branch| {
                    !branch.is_empty() && !matches!(*branch, "—" | "-") && *branch != name
                });
            branch.map_or_else(
                || format!("{} · {}", root.project, name),
                |branch| format!("{} · {} · {branch}", root.project, name),
            )
        })
        .collect()
}

fn session_diff_status(state: &grove_core::git::WorktreeGitState) -> (String, bool) {
    if state.added > 0 || state.removed > 0 {
        (format!("+{} -{}", state.added, state.removed), true)
    } else if state.dirty {
        ("Changes".into(), true)
    } else {
        ("clean".into(), false)
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
    use std::time::{Duration, Instant};

    fn assert_empty_cross_project_launch(
        cx: &mut gpui::TestAppContext,
        collapsed: bool,
        projects: bool,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            let mut store = grove_core::storage::Store::default();
            if projects {
                for name in ["alpha", "beta"] {
                    let path = format!("/grove-empty-multi-{name}");
                    store.projects.push(grove_core::storage::Project {
                        name: name.into(),
                        path: path.clone(),
                        scripts: grove_core::storage::ProjectScripts::default(),
                        archived: false,
                        worktree_dir: None,
                    });
                    store.assign_project_to_active_workspace(&path);
                }
            }
            cx.set_global(SettingsState::new(store));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        let requests = std::rc::Rc::new(std::cell::Cell::new(0));
        cx.update(|window, cx| {
            let requests = requests.clone();
            cx.subscribe(&sidebar, move |_, event, _| {
                if matches!(event, SidebarEvent::NewMultiProjectSessionRequested) {
                    requests.set(requests.get() + 1);
                }
            })
            .detach();
            sidebar.update(cx, |sidebar, cx| {
                assert!(sidebar.multi_project_sessions(cx).is_empty());
                assert_eq!(sidebar.snapshot.projects.is_empty(), !projects);
                if collapsed {
                    sidebar.toggle_sidebar(window, cx);
                }
            });
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        let launch_id = if collapsed {
            let anchor = cx
                .debug_bounds("multi-project-anchor")
                .expect("empty compact sidebar must expose the cross-project launcher");
            cx.simulate_click(anchor.center(), gpui::Modifiers::default());
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| sidebar.sync(window, cx));
            });
            draw(cx);
            assert!(cx.debug_bounds("multi-project-session-flyout").is_some());
            assert!(cx.debug_bounds("multi-project-flyout-empty").is_some());
            "multi-project-flyout-new"
        } else {
            assert!(cx.debug_bounds("multi-project-empty").is_some());
            "multi-project-new"
        };
        let launch = cx
            .debug_bounds(launch_id)
            .expect("empty sidebar must expose a new cross-project session control");
        cx.simulate_click(launch.center(), gpui::Modifiers::default());
        draw(cx);
        assert_eq!(requests.get(), 1);
    }

    #[gpui::test]
    fn empty_expanded_sidebar_can_launch_cross_project_session(cx: &mut gpui::TestAppContext) {
        assert_empty_cross_project_launch(cx, false, false);
    }

    #[gpui::test]
    fn empty_compact_sidebar_can_launch_cross_project_session(cx: &mut gpui::TestAppContext) {
        assert_empty_cross_project_launch(cx, true, false);
    }

    #[gpui::test]
    fn populated_expanded_sidebar_can_launch_cross_project_session(cx: &mut gpui::TestAppContext) {
        assert_empty_cross_project_launch(cx, false, true);
    }

    #[gpui::test]
    fn populated_compact_sidebar_can_launch_cross_project_session(cx: &mut gpui::TestAppContext) {
        assert_empty_cross_project_launch(cx, true, true);
    }

    #[gpui::test]
    fn multi_project_sessions_have_one_workspace_scoped_navigation_row(
        cx: &mut gpui::TestAppContext,
    ) {
        use grove_core::session_meta::ContextRoot;
        let paths = ["/grove-multi-alpha", "/grove-multi-beta"];
        cx.update(|cx| {
            gpui_component::init(cx);
            let mut store = grove_core::storage::Store {
                projects: paths
                    .iter()
                    .enumerate()
                    .map(|(index, path)| grove_core::storage::Project {
                        name: ["alpha", "beta"][index].into(),
                        path: (*path).into(),
                        scripts: grove_core::storage::ProjectScripts::default(),
                        archived: false,
                        worktree_dir: None,
                    })
                    .collect(),
                sidebar_width: Some(300.0),
                ..Default::default()
            };
            store.assign_project_to_active_workspace(paths[0]);
            store.workspaces.create("Other").unwrap();
            store.assign_project_to_active_workspace(paths[1]);
            store.workspaces.select(1);
            cx.set_global(SettingsState::new(store));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            runtime.read(cx).registry.clone().update(cx, |registry, _| {
                // Additional projects do not widen the primary session's workspace.
                registry.insert_meta_with_context(
                    "alpha".into(),
                    paths[0].into(),
                    Agent::Codex,
                    vec![ContextRoot {
                        project: "beta".into(),
                        wt_path: paths[1].into(),
                    }],
                    None,
                );
                // Two roots in one project remain ordinary project sessions.
                registry.insert_meta_with_context(
                    "alpha".into(),
                    paths[0].into(),
                    Agent::Claude,
                    vec![ContextRoot {
                        project: "alpha".into(),
                        wt_path: format!("{}/feature", paths[0]),
                    }],
                    None,
                );
                registry.insert_meta_with_context(
                    "beta".into(),
                    paths[1].into(),
                    Agent::Codex,
                    vec![ContextRoot {
                        project: "alpha".into(),
                        wt_path: paths[0].into(),
                    }],
                    None,
                );
            });
            for (index, path) in paths.iter().enumerate() {
                runtime.read(cx).tree.clone().update(cx, |tree, _| {
                    tree.set_active_worktrees(
                        index,
                        vec![grove_core::git::Worktree {
                            path: (*path).into(),
                            branch: "main".into(),
                            mtime: None,
                            is_main: true,
                        }],
                    );
                });
            }
            Sidebar::new(runtime, window, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        let section = cx.debug_bounds("multi-project-sessions").unwrap();
        assert!(cx.debug_bounds("multi-project-empty").is_none());
        let multi = cx.debug_bounds("session-1").unwrap();
        let project = cx.debug_bounds("project-0").unwrap();
        let single = cx.debug_bounds("session-2").unwrap();
        assert!(multi.top() >= section.top() && multi.bottom() <= section.bottom());
        assert!(section.bottom() + gpui::px(SPACE_2XL) <= project.top());
        assert!(single.top() > project.top());
        assert!(cx.debug_bounds("session-3").is_none());
        let mut project_width = None;
        for width in [220.0, 260.0, 320.0] {
            cx.update(|window, cx| {
                cx.global_mut::<SettingsState>().store.sidebar_width = Some(width);
                sidebar.update(cx, |sidebar, cx| sidebar.sync(window, cx));
            });
            draw(cx);
            let section = cx.debug_bounds("multi-project-sessions").unwrap();
            let heading = cx.debug_bounds("sidebar-heading-label").unwrap();
            assert!(heading.top() >= section.bottom());
            let mut previous = heading.right();
            for id in [
                "sidebar-view",
                "sidebar-grid",
                "projects-archive",
                "projects-add",
            ] {
                let control = cx.debug_bounds(id).unwrap();
                assert!(control.top() >= section.bottom());
                assert!((f32::from(control.center().y - heading.center().y)).abs() <= 1.0);
                assert!(control.left() >= previous);
                assert!(control.right() <= cx.debug_bounds("sidebar-rail").unwrap().right());
                previous = control.right();
            }
            let row = cx.debug_bounds("session-1").unwrap();
            let title = cx.debug_bounds("multi-session-title-1").unwrap();
            let status = cx.debug_bounds("session-tree-status-1").unwrap();
            let first = cx.debug_bounds("multi-root-row-1-0").unwrap();
            let second = cx.debug_bounds("multi-root-row-1-1").unwrap();
            let project = cx.debug_bounds("multi-root-project-1-0").unwrap();
            let worktree = cx.debug_bounds("multi-root-worktree-1-0").unwrap();
            assert!(status.bottom() <= title.top());
            assert!(status.left() >= cx.debug_bounds("session-agent-1").unwrap().right());
            assert!(status.right() < row.right());
            assert!(first.top() >= title.bottom());
            assert!(second.top() >= first.bottom());
            assert_eq!(first.left(), title.left());
            assert_eq!(second.left(), title.left());
            assert!(worktree.size.width > gpui::px(0.0));
            assert!((f32::from(worktree.right() - first.right())).abs() <= 1.0);
            if let Some(previous) = project_width {
                assert_eq!(
                    project.size.width, previous,
                    "project label must not shrink at narrow widths"
                );
            }
            project_width = Some(project.size.width);
        }
        let long_project = "A project name that is much longer than the entire available sidebar";
        cx.update(|_, cx| {
            sidebar
                .read(cx)
                .runtime
                .read(cx)
                .registry
                .clone()
                .update(cx, |registry, _| {
                    registry.rename_project("beta", long_project);
                });
        });
        for width in [220.0, 260.0, 320.0] {
            cx.update(|window, cx| {
                cx.global_mut::<SettingsState>().store.sidebar_width = Some(width);
                sidebar.update(cx, |sidebar, cx| sidebar.sync(window, cx));
            });
            draw(cx);
            let root = cx.debug_bounds("multi-root-row-1-1").unwrap();
            let project = cx.debug_bounds("multi-root-project-1-1").unwrap();
            let worktree = cx.debug_bounds("multi-root-worktree-1-1").unwrap();
            assert!(project.size.width <= root.size.width / 2.0 + gpui::px(1.0));
            assert!(worktree.size.width > gpui::px(0.0));
            assert!(worktree.left() >= project.right());
            assert!(worktree.right() <= root.right() + gpui::px(1.0));
        }
        cx.update(|_, cx| {
            sidebar
                .read(cx)
                .runtime
                .read(cx)
                .registry
                .clone()
                .update(cx, |registry, _| {
                    registry.rename_project(long_project, "beta");
                });
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                assert_eq!(
                    sidebar.visible_session_order(cx),
                    vec![SessionId::from_raw(1), SessionId::from_raw(2)]
                );
                let ordinary = sidebar.project_navigation_snapshot(cx);
                assert_eq!(ordinary.projects[0].sessions, vec![SessionId::from_raw(2)]);
                assert_eq!(
                    ordinary.projects[0].worktrees[0].sessions,
                    vec![SessionId::from_raw(2)]
                );
                assert_eq!(
                    sidebar.snapshot.projects[0].sessions,
                    vec![SessionId::from_raw(1), SessionId::from_raw(2)]
                );
                sidebar.collapsed_projects.insert(paths[0].into());
                assert_eq!(
                    sidebar.visible_session_order(cx),
                    vec![SessionId::from_raw(1)]
                );
                sidebar.select_numbered_session(1, window, cx);
                assert_eq!(sidebar.selected_session(), Some(SessionId::from_raw(1)));
                sidebar.toggle_sidebar(window, cx);
            });
        });
        draw(cx);
        let compact = cx.debug_bounds("compact-multi-project-sessions").unwrap();
        let multi = cx.debug_bounds("session-1").unwrap();
        assert!(multi.top() >= compact.top() && multi.bottom() <= compact.bottom());
        assert!(compact.bottom() <= cx.debug_bounds("project-0").unwrap().top());
        // The ordinary project's popup shares the same filtered placement as its tiles.
        let project = cx.debug_bounds("project-0").unwrap();
        cx.simulate_mouse_move(project.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("project-session-flyout").is_some());
        assert!(cx.debug_bounds("flyout-session-1").is_none());
        assert!(cx.debug_bounds("flyout-session-2").is_some());
        let anchor = cx.debug_bounds("multi-project-anchor").unwrap();
        cx.simulate_mouse_move(anchor.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("multi-project-session-flyout").is_some());
        assert!(cx.debug_bounds("multi-flyout-session-1").is_some());
        assert!(cx.debug_bounds("multi-project-flyout-empty").is_none());
        assert!(cx.debug_bounds("flyout-project-actions").is_none());
        cx.simulate_click(anchor.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.project_flyout_hover_open));
        // Tab reaches nested session controls rather than cycling only the row action.
        let panel_focus = cx.update(|window, cx| window.focused(cx));
        cx.simulate_keystrokes("tab tab");
        let session_focus = cx.update(|window, cx| window.focused(cx));
        assert_ne!(
            session_focus, panel_focus,
            "Tab should enter the session row"
        );
        cx.simulate_keystrokes("tab");
        let close_focus = cx.update(|window, cx| window.focused(cx));
        assert_ne!(
            close_focus, session_focus,
            "Tab should reach the nested Close control"
        );
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.pending_close),
            Some(SessionId::from_raw(1))
        );
        assert!(cx.debug_bounds("compact-close-session-1").is_none());
        assert!(cx.debug_bounds("session-close-popup-1").is_some());
        assert!(cx.debug_bounds("compact-close-popup-1").is_none());
        let popup = cx.debug_bounds("sidebar-confirmation").unwrap();
        let flyout = cx.debug_bounds("multi-project-session-flyout").unwrap();
        assert!(popup.left() >= flyout.left());
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.confirmation_open()));
        assert!(cx.debug_bounds("multi-project-session-flyout").is_some());
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.confirmation_open()));
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.confirmation_open()));
        assert!(cx.debug_bounds("multi-project-session-flyout").is_some());
        // Escape from a focused descendant dismisses the named context and restores its anchor.
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.debug_bounds("multi-project-session-flyout").is_none());
        assert!(cx.update(|window, cx| sidebar.read(cx).multi_project_focus.is_focused(window)));
        // A direct session click dismisses and suppresses this context until pointer exit.
        let outside = gpui::point(gpui::px(600.0), gpui::px(500.0));
        cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
        cx.simulate_mouse_move(multi.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("multi-project-session-flyout").is_some());
        cx.update(|window, cx| {
            let focus = sidebar.read(cx).project_flyout_focus.clone();
            focus.focus(window, cx);
        });
        cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.project_flyout_hide_task.is_some()));
        cx.simulate_keystrokes("tab tab tab enter");
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.confirmation_open()));
        cx.executor()
            .advance_clock(Duration::from_millis(MOTION_SLOW_MS * 2));
        draw(cx);
        assert!(cx.debug_bounds("multi-project-session-flyout").is_some());
        assert!(cx.debug_bounds("session-close-popup-1").is_some());
        assert!(cx.debug_bounds("compact-close-popup-1").is_none());
        cx.simulate_keystrokes("escape escape");
        cx.simulate_mouse_move(multi.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("multi-project-session-flyout").is_some());
        cx.simulate_click(multi.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("multi-project-session-flyout").is_none());
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.project_flyout_suppressed.clone()),
            Some(SidebarContext::MultiProject)
        );
        cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
        cx.simulate_mouse_move(anchor.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("multi-project-session-flyout").is_some());
        cx.simulate_click(anchor.center(), gpui::Modifiers::default());
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(cx.debug_bounds("multi-project-session-flyout").is_none());
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selected_session()),
            Some(SessionId::from_raw(1))
        );
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::Close(SessionId::from_raw(1)), window, cx);
                assert_eq!(sidebar.pending_close, Some(SessionId::from_raw(1)));
                assert!(sidebar
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .meta(SessionId::from_raw(1))
                    .is_some());
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("compact-close-popup-1").is_some());
        assert!(cx.debug_bounds("session-close-popup-1").is_none());
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.confirmation_open()));
        cx.update(|window, cx| {
            cx.global_mut::<SettingsState>().store.workspaces.select(2);
            sidebar.update(cx, |sidebar, cx| sidebar.sync(window, cx));
        });
        draw(cx);
        assert!(cx.debug_bounds("session-1").is_none());
        assert!(cx.debug_bounds("session-2").is_none());
        assert!(cx.debug_bounds("session-3").is_some());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::ConfirmClose(SessionId::from_raw(3)), window, cx);
                sidebar.sync(window, cx);
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("compact-multi-project-sessions").is_some());
        let anchor = cx.debug_bounds("multi-project-anchor").unwrap();
        cx.simulate_click(anchor.center(), gpui::Modifiers::default());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.sync(window, cx));
        });
        draw(cx);
        assert!(cx.debug_bounds("multi-project-session-flyout").is_some());
        assert!(cx.debug_bounds("multi-project-flyout-new").is_some());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.toggle_sidebar(window, cx));
        });
        draw(cx);
        assert!(cx.debug_bounds("multi-project-sessions").is_some());
        assert!(cx.debug_bounds("multi-project-new").is_some());
        assert!(cx.debug_bounds("sidebar-navigation-header").is_some());
        assert!(cx.debug_bounds("projects-count").is_some());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.toggle_tree_list(window, cx));
        });
        draw(cx);
        assert!(cx.debug_bounds("projects-count").is_none());
        assert!(
            cx.debug_bounds("sidebar-heading-label").unwrap().top()
                < cx.debug_bounds("sidebar-scroll").unwrap().top()
        );
        for id in [
            "sidebar-view",
            "sidebar-grid",
            "projects-archive",
            "projects-add",
        ] {
            assert!(cx.debug_bounds(id).is_some());
        }
        cx.update(|window, cx| {
            cx.global_mut::<SettingsState>()
                .store
                .workspaces
                .create("Empty")
                .unwrap();
            sidebar.update(cx, |sidebar, cx| {
                sidebar.toggle_tree_list(window, cx);
                sidebar.sync(window, cx);
                assert!(sidebar.snapshot.projects.is_empty());
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("multi-project-sessions").is_some());
        assert!(cx.debug_bounds("multi-project-new").is_some());
        assert!(cx.debug_bounds("projects-count").is_some());
        let heading = cx.debug_bounds("sidebar-heading-label").unwrap();
        for id in [
            "sidebar-view",
            "sidebar-grid",
            "projects-archive",
            "projects-add",
        ] {
            let control = cx.debug_bounds(id).unwrap();
            assert!((f32::from(control.center().y - heading.center().y)).abs() <= 1.0);
        }
    }

    fn assert_workspace_switches_retain_view_mode(
        cx: &mut gpui::TestAppContext,
        sidebar_mode: ViewMode,
    ) {
        let paths = [
            "/grove-view-workspace-a",
            "/grove-view-workspace-b",
            "/grove-view-workspace-c",
        ];
        cx.update(|cx| {
            gpui_component::init(cx);
            let mut store = grove_core::storage::Store {
                projects: paths
                    .iter()
                    .enumerate()
                    .map(|(index, path)| grove_core::storage::Project {
                        name: format!("project-{index}"),
                        path: (*path).into(),
                        scripts: grove_core::storage::ProjectScripts::default(),
                        archived: false,
                        worktree_dir: None,
                    })
                    .collect(),
                sidebar_width: Some(310.0),
                ..Default::default()
            };
            store.assign_project_to_active_workspace(paths[0]);
            store.workspaces.create("B").unwrap();
            store.assign_project_to_active_workspace(paths[1]);
            store.workspaces.create("C").unwrap();
            store.assign_project_to_active_workspace(paths[2]);
            store.workspaces.select(1);
            cx.set_global(SettingsState::new(store));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.sync(window, cx);
                sidebar.act(Action::Mode(sidebar_mode), window, cx);
                sidebar.selection = Some(Selection::Project(0));
                sidebar.terminals_collapsed = true;
                sidebar
                    .scroll
                    .set_offset(gpui::point(gpui::px(0.0), gpui::px(-40.0)));
            });
            // A -> B retains the explicitly selected project/list organization.
            cx.global_mut::<SettingsState>().store.workspaces.select(2);
            sidebar.update(cx, |sidebar, cx| {
                sidebar.sync(window, cx);
                assert_eq!(sidebar.mode, sidebar_mode);
                assert_eq!(sidebar.last_mode, sidebar_mode);
                assert_eq!(sidebar.selection, Some(Selection::Project(1)));
                assert!(!sidebar.terminals_collapsed);
                assert_eq!(sidebar.scroll.offset().y, gpui::px(0.0));
                sidebar
                    .scroll
                    .set_offset(gpui::point(gpui::px(0.0), gpui::px(-80.0)));
                sidebar.toggle_grid(window, cx);
            });
            // B -> C retains grid, including its global return organization.
            cx.global_mut::<SettingsState>().store.workspaces.select(3);
            sidebar.update(cx, |sidebar, cx| {
                sidebar.sync(window, cx);
                assert_eq!(sidebar.mode, ViewMode::Grid);
                assert_eq!(sidebar.last_mode, sidebar_mode);
                sidebar.toggle_sidebar(window, cx);
                assert_eq!(sidebar.mode, ViewMode::Grid);
                assert!(sidebar.is_collapsed(cx));
                assert_eq!(
                    cx.global::<SettingsState>().store.sidebar_width,
                    Some(310.0)
                );
            });
            // C -> A restores A's navigation without restoring its former mode.
            cx.global_mut::<SettingsState>().store.workspaces.select(1);
            sidebar.update(cx, |sidebar, cx| {
                sidebar.sync(window, cx);
                assert_eq!(sidebar.mode, ViewMode::Grid);
                // Grid selects a canvas session; this fixture has no sessions.
                assert_eq!(sidebar.selection, None);
                assert!(sidebar.terminals_collapsed);
                assert_eq!(sidebar.scroll.offset().y, gpui::px(-40.0));
                assert!(sidebar.is_collapsed(cx));
                sidebar.toggle_grid(window, cx);
                assert_eq!(sidebar.mode, sidebar_mode);
            });
            // Revisiting C must honor the latest explicit exit from grid.
            cx.global_mut::<SettingsState>().store.workspaces.select(3);
            sidebar.update(cx, |sidebar, cx| {
                sidebar.sync(window, cx);
                assert_eq!(sidebar.mode, sidebar_mode);
                assert_eq!(sidebar.last_mode, sidebar_mode);
                assert_eq!(sidebar.selection, Some(Selection::Project(2)));
                assert!(!sidebar.terminals_collapsed);
                assert!(sidebar.is_collapsed(cx));
            });
            cx.global_mut::<SettingsState>().store.workspaces.select(2);
            sidebar.update(cx, |sidebar, cx| {
                sidebar.sync(window, cx);
                assert_eq!(sidebar.mode, sidebar_mode);
                assert_eq!(sidebar.selection, Some(Selection::Project(1)));
                assert_eq!(sidebar.scroll.offset().y, gpui::px(-80.0));
                assert!(!sidebar.terminals_collapsed);
            });
        });
    }

    #[gpui::test]
    fn workspace_switches_retain_project_and_grid_mode(cx: &mut gpui::TestAppContext) {
        assert_workspace_switches_retain_view_mode(cx, ViewMode::Project);
    }

    #[gpui::test]
    fn workspace_switches_retain_list_and_grid_mode(cx: &mut gpui::TestAppContext) {
        assert_workspace_switches_retain_view_mode(cx, ViewMode::List);
    }

    #[test]
    fn zen_target_uses_selection_or_first_grid_session() {
        let first = SessionId::from_raw(1);
        let second = SessionId::from_raw(2);
        let sessions = [(first, false), (second, true)];
        assert_eq!(zen_target(ViewMode::Project, None, &sessions), None);
        assert_eq!(zen_target(ViewMode::List, None, &sessions), None);
        assert_eq!(
            zen_target(ViewMode::Grid, None, &sessions),
            Some((first, false))
        );
        assert_eq!(
            zen_target(ViewMode::Grid, Some(&Selection::Home(second)), &sessions),
            Some((second, true))
        );
        assert_eq!(zen_target(ViewMode::Grid, None, &[]), None);
    }

    pub(super) struct ChangedGitRepo(std::path::PathBuf);

    impl ChangedGitRepo {
        pub(super) fn new() -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "grove-sidebar-git-test-{}-{unique}",
                std::process::id()
            ));
            fs_err::create_dir(&path).unwrap();
            let repo = Self(path);
            repo.git(&["init", "-q"]);
            fs_err::write(repo.0.join("tracked.txt"), "first\n").unwrap();
            repo.git(&["add", "tracked.txt"]);
            repo.git(&[
                "-c",
                "user.name=Grove Test",
                "-c",
                "user.email=grove@example.invalid",
                "commit",
                "--no-gpg-sign",
                "-q",
                "-m",
                "baseline",
            ]);
            fs_err::write(repo.0.join("tracked.txt"), "first\nsecond\n").unwrap();
            repo
        }

        pub(super) fn path(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }

        fn git(&self, args: &[&str]) {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
    }

    impl Drop for ChangedGitRepo {
        fn drop(&mut self) {
            let _ = fs_err::remove_dir_all(&self.0);
        }
    }

    #[gpui::test]
    fn saved_project_without_sessions_resumes_to_start_panel(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "saved".into(),
                    path: "/grove-saved-project-start-test".into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            Some(Selection::Project(0))
        );
        assert!(cx.debug_bounds("project-start-card").is_some());
        assert!(cx.debug_bounds("start-terminal").is_some());
    }

    fn assert_compact_column(
        cx: &mut gpui::VisualTestContext,
        items: &[(&'static str, &'static str)],
    ) {
        let rail = cx.debug_bounds("sidebar-rail").unwrap();
        let expand = cx
            .debug_bounds("compact-glyph-sidebar-collapse-toggle")
            .unwrap();
        assert!((f32::from(expand.center().x - rail.center().x)).abs() <= 0.5);
        for &(row_selector, glyph_selector) in items {
            let row = cx.debug_bounds(row_selector).unwrap();
            let glyph = cx.debug_bounds(glyph_selector).unwrap();
            assert_eq!(f32::from(row.size.width), 36.0, "{row_selector}");
            assert_eq!(f32::from(row.size.height), 32.0, "{row_selector}");
            assert_eq!(row.center().x, expand.center().x, "{row_selector}");
            assert_eq!(glyph.center().x, expand.center().x, "{glyph_selector}");
            assert_eq!(glyph.center().y, row.center().y, "{glyph_selector}");
            assert!(
                row.left() >= rail.left() && row.right() <= rail.right(),
                "{row_selector}"
            );
            assert!(
                glyph.left() >= row.left() && glyph.right() <= row.right(),
                "{glyph_selector}"
            );
        }
    }

    fn assert_compact_overlay(
        cx: &mut gpui::VisualTestContext,
        row_selector: &'static str,
        overlay_selector: &'static str,
    ) {
        let row = cx.debug_bounds(row_selector).unwrap();
        let overlay = cx.debug_bounds(overlay_selector).unwrap();
        assert!(
            overlay.left() >= row.left() && overlay.right() <= row.right(),
            "{overlay_selector}"
        );
        assert!(
            overlay.top() >= row.top() && overlay.bottom() <= row.bottom(),
            "{overlay_selector}"
        );
    }

    fn assert_compact_disclosure(
        cx: &mut gpui::VisualTestContext,
        row_selector: &'static str,
        glyph_selector: &'static str,
        disclosure_selector: &'static str,
    ) {
        assert_compact_overlay(cx, row_selector, disclosure_selector);
        let row = cx.debug_bounds(row_selector).unwrap();
        let glyph = cx.debug_bounds(glyph_selector).unwrap();
        let disclosure = cx.debug_bounds(disclosure_selector).unwrap();
        assert!(
            (f32::from(disclosure.center().y - row.center().y)).abs() <= 0.5,
            "{disclosure_selector}: row={row:?}, disclosure={disclosure:?}"
        );
        assert!(disclosure.left() >= glyph.right(), "{disclosure_selector}");
    }

    #[gpui::test]
    fn project_flyout_launch_picker_replaces_plus_and_restores_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        let path = "/grove-sidebar-collapse-test";
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![
                    grove_core::storage::Project {
                        name: "demo".into(),
                        path: path.into(),
                        scripts: grove_core::storage::ProjectScripts::default(),
                        archived: false,
                        worktree_dir: None,
                    },
                    grove_core::storage::Project {
                        name: "second".into(),
                        path: "/grove-sidebar-second".into(),
                        scripts: grove_core::storage::ProjectScripts::default(),
                        archived: false,
                        worktree_dir: None,
                    },
                ],
                sidebar_width: Some(300.0),
                sidebar_collapsed: true,
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
            cx.set_global(crate::theme::ThemeState::new(
                false,
                "tokyonight".into(),
                "tokyonight-day".into(),
            ));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            let ids = registry.update(cx, |registry, _| {
                [Agent::Codex, Agent::Claude].map(|agent| {
                    registry.insert_meta(
                        "demo".into(),
                        if agent == Agent::Claude {
                            format!("{path}/feature")
                        } else {
                            path.into()
                        },
                        agent,
                    )
                })
            });
            let home_terminal = cx.new(|cx| {
                crate::entities::terminal_session::TerminalSession::attach_existing(
                    "grove_compact_alignment_test",
                    24,
                    80,
                    cx,
                )
            });
            registry.update(cx, |registry, _| {
                let mut home = registry.meta(ids[0]).unwrap().clone();
                home.id = SessionId::from_raw(3);
                home.project.clear();
                home.wt_path = "/".into();
                home.agent = Agent::Terminal;
                home.label = "Terminal 3".into();
                home.context_roots.clear();
                home.attention = None;
                registry.push_home(home, home_terminal);
            });
            runtime.read(cx).tree.clone().update(cx, |tree, _| {
                tree.set_active_worktrees(
                    0,
                    vec![
                        grove_core::git::Worktree {
                            path: path.into(),
                            branch: "main".into(),
                            mtime: None,
                            is_main: true,
                        },
                        grove_core::git::Worktree {
                            path: format!("{path}/feature"),
                            branch: "feature".into(),
                            mtime: None,
                            is_main: false,
                        },
                        grove_core::git::Worktree {
                            path: format!("{path}/empty"),
                            branch: "empty".into(),
                            mtime: None,
                            is_main: false,
                        },
                    ],
                );
            });
            runtime.read(cx).activity.clone().update(cx, |activity, _| {
                activity.set_state_for_test(ids[0], ActivityState::Working);
                activity.set_state_for_test(ids[1], ActivityState::WaitingForInput);
            });
            Sidebar::new(runtime, window, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        let anchor = cx.debug_bounds("project-0").unwrap();
        cx.simulate_mouse_move(anchor.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("flyout-project-count").is_none());
        let plus = cx
            .debug_bounds("flyout-launch-plus-/grove-sidebar-collapse-test")
            .unwrap();
        cx.simulate_click(plus.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx
            .debug_bounds("flyout-launch-plus-/grove-sidebar-collapse-test")
            .is_none());
        let terminal = cx
            .debug_bounds("flyout-launch-terminal-/grove-sidebar-collapse-test")
            .unwrap();
        let title = cx
            .debug_bounds("flyout-worktree-title-/grove-sidebar-collapse-test")
            .unwrap();
        assert!((f32::from(terminal.center().y - title.center().y)).abs() <= 1.0);
        assert!(sidebar.read_with(cx, |sidebar, _| !sidebar.project_flyout_hover_open));
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                assert!(sidebar.project_flyout_launch_focus.is_focused(window));
                assert_eq!(
                    sidebar.launch_target(0, format!("{path}/feature"), Agent::Terminal, cx),
                    Some(("demo".into(), format!("{path}/feature"), Agent::Terminal))
                );
            });
        });
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| {
            assert!(!sidebar
                .read(cx)
                .project_flyout_launch_focus
                .is_focused(window));
        });
        cx.simulate_keystrokes("shift-tab");
        cx.update(|window, cx| {
            assert!(sidebar
                .read(cx)
                .project_flyout_launch_focus
                .is_focused(window));
        });
        let feature_plus = cx
            .debug_bounds("flyout-launch-plus-/grove-sidebar-collapse-test/feature")
            .unwrap();
        cx.simulate_click(feature_plus.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx
            .debug_bounds("flyout-launch-plus-/grove-sidebar-collapse-test")
            .is_some());
        assert!(cx
            .debug_bounds("flyout-launch-plus-/grove-sidebar-collapse-test/feature")
            .is_none());
        cx.update(|_, cx| sidebar.update(cx, |sidebar, _| sidebar.available[2] = false));
        let opencode = cx
            .debug_bounds("flyout-launch-opencode-/grove-sidebar-collapse-test/feature")
            .unwrap();
        cx.simulate_click(opencode.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar
            .content_error
            .as_deref()
            .is_some_and(|error| error.contains("OpenCode CLI not found"))));
        assert!(cx
            .debug_bounds("flyout-launch-opencode-/grove-sidebar-collapse-test/feature")
            .is_some());
        assert!(cx
            .debug_bounds("flyout-launch-error-/grove-sidebar-collapse-test/feature")
            .is_some());
        cx.simulate_mouse_move(
            gpui::point(gpui::px(900.0), gpui::px(740.0)),
            None,
            gpui::Modifiers::default(),
        );
        cx.executor()
            .advance_clock(Duration::from_millis(MOTION_SLOW_MS * 2));
        draw(cx);
        assert!(cx.debug_bounds("project-session-flyout").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx
            .debug_bounds("flyout-launch-plus-/grove-sidebar-collapse-test/feature")
            .is_some());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, _| {
                assert!(
                    sidebar.project_flyout_plus_focus["/grove-sidebar-collapse-test/feature"]
                        .is_focused(window)
                );
            });
        });
        assert!(cx.debug_bounds("project-session-flyout").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.debug_bounds("project-session-flyout").is_none());
        cx.simulate_resize(gpui::size(gpui::px(240.0), gpui::px(300.0)));
        cx.simulate_click(anchor.center(), gpui::Modifiers::default());
        draw(cx);
        cx.simulate_keystrokes("tab tab tab tab tab tab tab");
        draw(cx);
        let popup = cx.debug_bounds("project-session-flyout").unwrap();
        let footer = cx.debug_bounds("flyout-project-actions").unwrap();
        assert!(
            footer.top() >= popup.top() && footer.bottom() <= popup.bottom(),
            "keyboard-focused footer={footer:?}; popup={popup:?}"
        );
    }

    #[gpui::test]
    fn collapsed_tree_and_list_preserve_session_selection_and_safe_close(
        cx: &mut gpui::TestAppContext,
    ) {
        let path = "/grove-sidebar-collapse-test";
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![
                    grove_core::storage::Project {
                        name: "demo".into(),
                        path: path.into(),
                        scripts: grove_core::storage::ProjectScripts::default(),
                        archived: false,
                        worktree_dir: None,
                    },
                    grove_core::storage::Project {
                        name: "second".into(),
                        path: "/grove-sidebar-second".into(),
                        scripts: grove_core::storage::ProjectScripts::default(),
                        archived: false,
                        worktree_dir: None,
                    },
                ],
                sidebar_width: Some(300.0),
                sidebar_collapsed: true,
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
            cx.set_global(crate::theme::ThemeState::new(
                false,
                "tokyonight".into(),
                "tokyonight-day".into(),
            ));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            let ids = registry.update(cx, |registry, _| {
                [Agent::Codex, Agent::Claude].map(|agent| {
                    registry.insert_meta(
                        "demo".into(),
                        if agent == Agent::Claude {
                            format!("{path}/feature")
                        } else {
                            path.into()
                        },
                        agent,
                    )
                })
            });
            let home_terminal = cx.new(|cx| {
                crate::entities::terminal_session::TerminalSession::attach_existing(
                    "grove_compact_alignment_test",
                    24,
                    80,
                    cx,
                )
            });
            registry.update(cx, |registry, _| {
                let mut home = registry.meta(ids[0]).unwrap().clone();
                home.id = SessionId::from_raw(3);
                home.project.clear();
                home.wt_path = "/".into();
                home.agent = Agent::Terminal;
                home.label = "Terminal 3".into();
                home.context_roots.clear();
                home.attention = None;
                registry.push_home(home, home_terminal);
            });
            runtime.read(cx).tree.clone().update(cx, |tree, _| {
                tree.set_active_worktrees(
                    0,
                    vec![
                        grove_core::git::Worktree {
                            path: path.into(),
                            branch: "main".into(),
                            mtime: None,
                            is_main: true,
                        },
                        grove_core::git::Worktree {
                            path: format!("{path}/feature"),
                            branch: "feature".into(),
                            mtime: None,
                            is_main: false,
                        },
                        grove_core::git::Worktree {
                            path: format!("{path}/empty"),
                            branch: "empty".into(),
                            mtime: None,
                            is_main: false,
                        },
                    ],
                );
            });
            runtime.read(cx).activity.clone().update(cx, |activity, _| {
                activity.set_state_for_test(ids[0], ActivityState::Working);
                activity.set_state_for_test(ids[1], ActivityState::WaitingForInput);
            });
            Sidebar::new(runtime, window, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        let rail = cx.debug_bounds("sidebar-rail").unwrap();
        let header = cx.debug_bounds("sidebar-workspace-header").unwrap();
        assert_eq!(header.top(), rail.top() + gpui::px(SPACE_LG));
        let toggle = cx.debug_bounds("sidebar-collapse-toggle").unwrap();
        assert!(
            (f32::from(toggle.top() - rail.top()) - f32::from(toggle.left() - rail.left())).abs()
                <= 1.0
        );
        assert!(cx.debug_bounds("sidebar-appbar-spacer").is_none());
        assert_compact_disclosure(
            cx,
            "fold-terminals",
            "compact-glyph-fold-terminals",
            "compact-terminals-disclosure",
        );
        let project = cx.debug_bounds("project-0").unwrap();
        let first = cx.debug_bounds("session-1").unwrap();
        let second = cx.debug_bounds("session-2").unwrap();
        assert!(project.top() < first.top() && first.top() < second.top());
        for bounds in [project, first, second] {
            assert_eq!(f32::from(bounds.size.width), 36.0);
            assert_eq!(f32::from(bounds.size.height), 32.0);
        }
        let other_project = cx.debug_bounds("project-1").unwrap();
        let initials = cx.debug_bounds("compact-project-initials-0").unwrap();
        let other_initials = cx.debug_bounds("compact-project-initials-1").unwrap();
        assert_eq!(initials.center().x, project.center().x);
        assert_eq!(other_initials.center().x, other_project.center().x);
        for (initial_selector, other_selector) in [
            ("compact-project-initial-0-0", "compact-project-initial-1-0"),
            ("compact-project-initial-0-1", "compact-project-initial-1-1"),
        ] {
            let initial = cx.debug_bounds(initial_selector).unwrap();
            let other_initial = cx.debug_bounds(other_selector).unwrap();
            assert_eq!(initial.center().x, other_initial.center().x);
        }
        assert_compact_overlay(cx, "project-0", "compact-project-count-0");
        assert_compact_overlay(cx, "project-1", "compact-project-count-1");
        assert_compact_column(
            cx,
            &[
                ("session-1", "compact-glyph-session-1"),
                ("session-2", "compact-glyph-session-2"),
                ("home-3", "compact-glyph-home-3"),
                ("sidebar-view", "compact-glyph-sidebar-view"),
                ("sidebar-grid", "compact-glyph-sidebar-grid"),
                ("fold-terminals", "compact-glyph-fold-terminals"),
                ("add-terminal", "compact-glyph-add-terminal"),
                ("projects-add", "compact-glyph-projects-add"),
            ],
        );
        assert_compact_overlay(cx, "session-2", "compact-session-status-2");
        assert_compact_overlay(cx, "home-3", "compact-home-status-3");
        let canvas_left = cx.debug_bounds("sidebar-canvas").unwrap().left();
        let selected_before_hover = sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone());
        let focus_before_hover = cx.update(|window, cx| window.focused(cx));
        cx.simulate_mouse_move(second.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.project_flyout_hover_open));
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            selected_before_hover
        );
        cx.update(|window, cx| assert_eq!(window.focused(cx), focus_before_hover));
        let hovered_popup = cx.debug_bounds("project-session-flyout").unwrap();
        assert!((f32::from(hovered_popup.left() - rail.right()) - 8.0).abs() <= 0.5);
        for selector in [
            "flyout-worktree-divider-/grove-sidebar-collapse-test/feature",
            "flyout-worktree-divider-/grove-sidebar-collapse-test/empty",
            "flyout-footer-divider",
        ] {
            let divider = cx.debug_bounds(selector).unwrap();
            assert_eq!(divider.left(), hovered_popup.left() + gpui::px(1.0));
            assert_eq!(divider.right(), hovered_popup.right() - gpui::px(1.0));
        }
        assert!(cx.debug_bounds("flyout-project-count").is_none());
        let session_row = cx.debug_bounds("flyout-session-1").unwrap();
        assert_eq!(f32::from(session_row.size.height), SESSION_ROW_H);
        let session_label = cx.debug_bounds("flyout-session-title-1").unwrap();
        let footer_label = cx.debug_bounds("flyout-actions-label").unwrap();
        assert!((f32::from(session_label.left() - footer_label.left())).abs() <= 1.0);

        let group = cx.debug_bounds("compact-project-group-0").unwrap();
        let crossing = gpui::point(
            (group.right() + hovered_popup.left()) / 2.0,
            second.center().y,
        );
        cx.simulate_mouse_move(crossing, None, gpui::Modifiers::default());
        draw(cx);
        cx.executor()
            .advance_clock(Duration::from_millis(MOTION_SLOW_MS * 2));
        draw(cx);
        assert!(cx.debug_bounds("project-session-flyout").is_some());
        cx.simulate_mouse_move(hovered_popup.center(), None, gpui::Modifiers::default());
        draw(cx);
        cx.executor()
            .advance_clock(Duration::from_millis(MOTION_SLOW_MS * 2));
        draw(cx);
        assert!(cx.debug_bounds("project-session-flyout").is_some());
        let reentry_outside = gpui::point(gpui::px(900.0), gpui::px(740.0));
        cx.simulate_mouse_move(reentry_outside, None, gpui::Modifiers::default());
        draw(cx);
        cx.executor()
            .advance_clock(Duration::from_millis(MOTION_SLOW_MS * 2));
        draw(cx);
        assert!(cx.debug_bounds("project-session-flyout").is_none());
        cx.simulate_mouse_move(second.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("project-session-flyout").is_some());
        let other_project = cx.debug_bounds("project-1").unwrap();
        cx.simulate_mouse_move(other_project.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar
            .project_flyout
            .as_ref()
            .and_then(SidebarContext::project_path)
            .map(String::as_str)
            == Some("/grove-sidebar-second")));
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            selected_before_hover
        );
        cx.update(|window, cx| assert_eq!(window.focused(cx), focus_before_hover));
        let outside_context = gpui::point(gpui::px(900.0), gpui::px(740.0));
        cx.simulate_mouse_move(outside_context, None, gpui::Modifiers::default());
        draw(cx);
        cx.executor()
            .advance_clock(Duration::from_millis(MOTION_SLOW_MS * 2));
        draw(cx);
        assert!(cx.debug_bounds("project-session-flyout").is_none());
        cx.update(|window, cx| assert_eq!(window.focused(cx), focus_before_hover));
        cx.simulate_mouse_move(second.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("project-session-flyout").is_some());
        cx.simulate_click(second.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("project-session-flyout").is_none());
        cx.simulate_mouse_move(second.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("project-session-flyout").is_none());
        cx.simulate_mouse_move(outside_context, None, gpui::Modifiers::default());
        draw(cx);
        cx.simulate_click(project.center(), gpui::Modifiers::default());
        draw(cx);
        cx.simulate_mouse_move(outside_context, None, gpui::Modifiers::default());
        draw(cx);
        cx.executor()
            .advance_clock(Duration::from_millis(MOTION_SLOW_MS * 2));
        draw(cx);
        assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.project_flyout_hover_open));
        draw(cx);
        assert!(cx.debug_bounds("project-session-flyout").is_some());
        let popup = cx.debug_bounds("project-session-flyout").unwrap();
        let trigger = sidebar.read_with(cx, |sidebar, _| sidebar.project_flyout_bounds.get());
        assert!(
            popup.left() >= rail.right(),
            "popup={popup:?}; rail={rail:?}; trigger={trigger:?}"
        );
        assert!(cx
            .debug_bounds("flyout-worktree-/grove-sidebar-collapse-test/feature")
            .is_some());
        assert!(cx
            .debug_bounds("flyout-empty-/grove-sidebar-collapse-test/empty")
            .is_some());
        let selection_before_inspecting =
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone());
        let second_project = cx.debug_bounds("project-1").unwrap();
        cx.simulate_click(second_project.center(), gpui::Modifiers::default());
        draw(cx);
        let second_popup = cx.debug_bounds("project-session-flyout").unwrap();
        assert!(second_popup.left() >= rail.right());
        assert!(
            (f32::from(second_popup.top() - second_project.top())).abs() <= 1.0,
            "second popup={second_popup:?}; project={second_project:?}"
        );
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            selection_before_inspecting
        );
        cx.simulate_click(project.center(), gpui::Modifiers::default());
        draw(cx);
        let project_menu = cx.debug_bounds("flyout-project-actions").unwrap();
        cx.simulate_click(project_menu.center(), gpui::Modifiers::default());
        draw(cx);
        let popup = cx.debug_bounds("project-actions-popup").unwrap();
        assert!(popup.right() > cx.debug_bounds("sidebar-rail").unwrap().right());
        assert_eq!(
            cx.debug_bounds("sidebar-canvas").unwrap().left(),
            canvas_left
        );
        assert_eq!(
            f32::from(cx.debug_bounds("sidebar-rail").unwrap().size.width),
            52.0
        );
        cx.simulate_keystrokes("escape");
        draw(cx);
        let home_close = cx.debug_bounds("compact-close-home-3").unwrap();
        cx.simulate_click(home_close.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(
            sidebar.read_with(cx, |sidebar, _| sidebar.pending_home_close
                == Some(SessionId::from_raw(3)))
        );
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.canvas_close_anchor.is_none()));
        assert!(cx.debug_bounds("compact-close-home-3").is_some());
        assert!(cx.debug_bounds("confirm-close").is_some());
        assert!(
            cx.debug_bounds("sidebar-confirmation").unwrap().right()
                > cx.debug_bounds("sidebar-rail").unwrap().right()
        );
        assert_eq!(
            cx.debug_bounds("sidebar-canvas").unwrap().left(),
            canvas_left
        );
        cx.simulate_resize(gpui::size(gpui::px(240.0), gpui::px(800.0)));
        draw(cx);
        let narrow_confirmation = cx.debug_bounds("sidebar-confirmation").unwrap();
        assert!(narrow_confirmation.left() >= gpui::px(0.0));
        assert!(narrow_confirmation.right() <= gpui::px(240.0));
        assert_eq!(
            f32::from(cx.debug_bounds("sidebar-rail").unwrap().size.width),
            52.0
        );
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.debug_bounds("fold-terminals").is_some());
        assert!(cx.debug_bounds("add-terminal").is_some());
        assert!(cx.debug_bounds("projects-add").is_some());
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.collapsed_projects.insert(path.to_string());
                assert_eq!(
                    sidebar.visible_session_order(cx),
                    vec![SessionId::from_raw(1), SessionId::from_raw(2)]
                );
                cx.notify();
            });
        });
        let project_center = cx.debug_bounds("project-0").unwrap().center();
        cx.simulate_click(project_center, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("project-session-flyout").is_some());
        assert!(cx.debug_bounds("session-1").is_some());
        cx.simulate_resize(gpui::size(gpui::px(240.0), gpui::px(300.0)));
        draw(cx);
        let flyout = cx.debug_bounds("project-session-flyout").unwrap();
        assert!(flyout.left() >= gpui::px(0.0));
        assert!(flyout.right() <= gpui::px(240.0));
        assert!(flyout.bottom() <= gpui::px(300.0));
        cx.simulate_keystrokes("down down down down down down");
        draw(cx);
        let actions = cx.debug_bounds("flyout-project-actions").unwrap();
        assert!(
            actions.top() >= flyout.top() && actions.bottom() <= flyout.bottom(),
            "scrolled actions={actions:?}; flyout={flyout:?}"
        );

        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        cx.update(|window, _| window.set_rem_size(gpui::px(crate::zoom::REM_BASE * 1.5)));
        draw(cx);
        let zoomed_rail = cx.debug_bounds("sidebar-rail").unwrap();
        let zoomed_popup = cx.debug_bounds("project-session-flyout").unwrap();
        assert!((f32::from(zoomed_popup.left() - zoomed_rail.right()) - 12.0).abs() <= 0.5);
        let zoomed_divider = cx.debug_bounds("flyout-footer-divider").unwrap();
        assert_eq!(zoomed_divider.left(), zoomed_popup.left() + gpui::px(1.0));
        assert_eq!(zoomed_divider.right(), zoomed_popup.right() - gpui::px(1.0));
        cx.update(|window, _| window.set_rem_size(gpui::px(crate::zoom::REM_BASE)));
        draw(cx);
        assert!(cx
            .debug_bounds("worktree-/grove-sidebar-collapse-test")
            .is_none());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.debug_bounds("project-session-flyout").is_none());
        cx.update(|window, cx| {
            assert!(sidebar.read(cx).project_toggle_focus[path].is_focused(window));
        });
        cx.simulate_click(project_center, gpui::Modifiers::default());
        draw(cx);
        let second = cx.debug_bounds("flyout-session-2").unwrap();
        cx.simulate_click(second.center(), gpui::Modifiers::default());
        draw(cx);
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            Some(Selection::Session(SessionId::from_raw(2)))
        );
        assert!(cx.debug_bounds("project-session-flyout").is_none());
        assert_compact_column(cx, &[("session-2", "compact-glyph-session-2")]);
        assert_compact_overlay(cx, "session-2", "compact-session-status-2");
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.toggle_tree_list(window, cx));
        });
        draw(cx);
        assert!(cx.debug_bounds("project-0").is_none());
        assert!(cx.debug_bounds("session-1").is_some());
        assert!(
            cx.debug_bounds("session-2").unwrap().top()
                < cx.debug_bounds("session-1").unwrap().top()
        );
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            Some(Selection::Session(SessionId::from_raw(2)))
        );
        cx.update(|window, cx| sidebar.update(cx, |sidebar, cx| sidebar.toggle_zen(window, cx)));
        draw(cx);
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        cx.update(|window, cx| sidebar.update(cx, |sidebar, cx| sidebar.toggle_zen(window, cx)));
        draw(cx);
        assert_eq!(
            f32::from(cx.debug_bounds("sidebar-rail").unwrap().size.width),
            52.0
        );
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.request_close_focused(window, cx));
        });
        draw(cx);
        assert_eq!(
            f32::from(cx.debug_bounds("sidebar-rail").unwrap().size.width),
            52.0
        );
        let rail = cx.debug_bounds("sidebar-rail").unwrap();
        assert!(cx.debug_bounds("sidebar-appbar-spacer").is_none());
        assert_eq!(
            cx.debug_bounds("sidebar-canvas").unwrap().left(),
            canvas_left
        );
        assert!(cx.debug_bounds("sidebar-confirmation").unwrap().right() > rail.right());
        assert!(cx.debug_bounds("confirm-close").is_some());
        assert!(cx.debug_bounds("cancel-close").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.confirmation_open()));
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            Some(Selection::Session(SessionId::from_raw(2)))
        );
        let toggle = cx.debug_bounds("sidebar-collapse-toggle").unwrap();
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        draw(cx);
        let expanded_rail = cx.debug_bounds("sidebar-rail").unwrap();
        let expanded_canvas_left = cx.debug_bounds("sidebar-canvas").unwrap().left();
        let session_row_top = cx.debug_bounds("session-2").unwrap().top();
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.request_close_focused(window, cx));
        });
        draw(cx);
        assert_eq!(cx.debug_bounds("sidebar-rail").unwrap(), expanded_rail);
        assert_eq!(
            cx.debug_bounds("sidebar-canvas").unwrap().left(),
            expanded_canvas_left
        );
        assert_eq!(cx.debug_bounds("session-2").unwrap().top(), session_row_top);
        assert!(cx.debug_bounds("sidebar-confirmation").unwrap().right() > expanded_rail.right());
        cx.simulate_keystrokes("escape");
        draw(cx);
    }

    #[gpui::test]
    fn collapsed_short_window_scrolls_controls_and_standalone_terminals(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                sidebar_collapsed: true,
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
            cx.set_global(crate::theme::ThemeState::new(
                false,
                "tokyonight".into(),
                "tokyonight-day".into(),
            ));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            for number in 1..=12 {
                let terminal = cx.new(|cx| {
                    crate::entities::terminal_session::TerminalSession::attach_existing(
                        "grove_compact_scroll_test",
                        24,
                        80,
                        cx,
                    )
                });
                registry.update(cx, |registry, _| {
                    registry.push_home(
                        SessionMeta {
                            id: SessionId::from_raw(number),
                            project: String::new(),
                            wt_path: "/".into(),
                            agent: Agent::Terminal,
                            context_roots: vec![],
                            temp_bundle_path: None,
                            label: format!("Terminal {number}"),
                            restored_title: None,
                            spawned_at: Instant::now(),
                            attention: None,
                            tmux: false,
                            tmux_name: None,
                        },
                        terminal,
                    );
                });
            }
            Sidebar::new(runtime, window, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(320.0), gpui::px(200.0)));
        draw(cx);
        let toggle = cx.debug_bounds("sidebar-collapse-toggle").unwrap();
        let rail = cx.debug_bounds("sidebar-rail").unwrap();
        assert!(rail.contains(&toggle.center()));
        assert_eq!(
            cx.debug_bounds("sidebar-workspace-header").unwrap().top(),
            rail.top() + gpui::px(SPACE_LG)
        );
        assert_compact_disclosure(
            cx,
            "fold-terminals",
            "compact-glyph-fold-terminals",
            "compact-terminals-disclosure",
        );
        assert_compact_column(
            cx,
            &[
                ("sidebar-view", "compact-glyph-sidebar-view"),
                ("fold-terminals", "compact-glyph-fold-terminals"),
                ("home-12", "compact-glyph-home-12"),
                ("add-terminal", "compact-glyph-add-terminal"),
                ("projects-add", "compact-glyph-projects-add"),
            ],
        );
        assert_compact_overlay(cx, "home-12", "compact-home-status-12");
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.scroll.max_offset().y) > gpui::px(0.0));
        sidebar.update(cx, |sidebar, cx| {
            sidebar.scroll.scroll_to_bottom();
            cx.notify();
        });
        draw(cx);
        let scroll = cx.debug_bounds("sidebar-scroll").unwrap();
        assert!(scroll.contains(&cx.debug_bounds("projects-add").unwrap().center()));
        assert_eq!(cx.debug_bounds("sidebar-collapse-toggle"), Some(toggle));
        // Scroll up from utilities to bring the separate Add terminal and final shell into view.
        let add_terminal = cx.debug_bounds("add-terminal").unwrap();
        sidebar.update(cx, |sidebar, cx| {
            let mut offset = sidebar.scroll.offset();
            offset.y += scroll.bottom() - add_terminal.bottom() - gpui::px(SPACE_SM);
            sidebar.scroll.set_offset(offset);
            cx.notify();
        });
        draw(cx);
        assert!(scroll.contains(&cx.debug_bounds("add-terminal").unwrap().center()));
        let terminal = cx.debug_bounds("home-12").unwrap();
        assert!(scroll.contains(&terminal.center()));
        assert_compact_column(
            cx,
            &[
                ("home-12", "compact-glyph-home-12"),
                ("add-terminal", "compact-glyph-add-terminal"),
            ],
        );
        assert_compact_overlay(cx, "home-12", "compact-home-status-12");
        cx.simulate_click(terminal.center(), gpui::Modifiers::default());
        draw(cx);
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            Some(Selection::Home(SessionId::from_raw(12)))
        );
        assert_compact_column(
            cx,
            &[
                ("home-12", "compact-glyph-home-12"),
                ("fold-terminals", "compact-glyph-fold-terminals"),
            ],
        );
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.scroll.offset().x),
            gpui::px(0.0)
        );
        cx.update(|window, cx| {
            let focus = sidebar.read(cx).focus.clone();
            focus.focus(window, cx);
        });
        let fold = cx.debug_bounds("fold-terminals").unwrap();
        sidebar.update(cx, |sidebar, cx| {
            let mut offset = sidebar.scroll.offset();
            offset.y += scroll.top() - fold.top() + gpui::px(SPACE_SM);
            sidebar.scroll.set_offset(offset);
            cx.notify();
        });
        draw(cx);
        let fold = cx.debug_bounds("fold-terminals").unwrap().center();
        cx.simulate_click(fold, gpui::Modifiers::default());
        draw(cx);
        cx.simulate_keystrokes("tab shift-tab");
        draw(cx);
        assert_compact_column(cx, &[("fold-terminals", "compact-glyph-fold-terminals")]);
        assert_compact_disclosure(
            cx,
            "fold-terminals",
            "compact-glyph-fold-terminals",
            "compact-terminals-disclosure",
        );
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.scroll.offset().x),
            gpui::px(0.0)
        );
    }

    #[test]
    fn numbered_tree_order_includes_every_group_and_navigation_wraps() {
        use crate::entities::workspace_state::{SnapshotProject, SnapshotWorktree};
        let ids: Vec<_> = (1..=4).map(SessionId::from_raw).collect();
        let snapshot = TreeSnapshot {
            projects: vec![
                SnapshotProject {
                    idx: 0,
                    worktrees: vec![
                        SnapshotWorktree {
                            path: "/a/main".into(),
                            sessions: ids[..2].to_vec(),
                            ..Default::default()
                        },
                        SnapshotWorktree {
                            path: "/a/branch".into(),
                            sessions: vec![ids[2]],
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
                SnapshotProject {
                    idx: 1,
                    worktrees: vec![SnapshotWorktree {
                        path: "/b/main".into(),
                        sessions: vec![ids[3]],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let order = visible_tree_sessions(&snapshot);
        assert_eq!(order, ids);
        assert_eq!(step_session(&order, Some(ids[3]), true), Some(ids[0]));
        assert_eq!(step_session(&order, Some(ids[0]), false), Some(ids[3]));
        assert_eq!(step_session(&order, Some(ids[2]), true), Some(ids[3]));
        assert_eq!(step_session(&order, None, false), Some(ids[3]));
    }

    #[test]
    fn home_terminal_row_uses_live_directory_with_labeled_launch_fallback() {
        assert_eq!(home_terminal_status(false, true, false), "Starting");
        assert_eq!(home_terminal_status(false, false, false), "Exited");
        assert_eq!(home_terminal_status(true, false, false), "Failed");
        assert_eq!(home_terminal_status(false, false, true), "Running");
        assert_eq!(
            terminal_directory_label(Some("/live"), Some("/repo")),
            Some(("home-current-cwd", "Current directory /live".into()))
        );
        assert_eq!(
            terminal_directory_label(None, Some("/repo")),
            Some(("home-launch-cwd", "Launched in /repo".into()))
        );
        assert_eq!(terminal_directory_label(None, None), None);
    }

    #[test]
    fn unpainted_reattached_session_is_idle_in_sidebar() {
        assert_eq!(
            managed_session_status(false, true, ActivityState::Idle).0,
            "Idle"
        );
        assert_eq!(
            managed_session_status(true, true, ActivityState::Idle).0,
            "Failed"
        );
        assert_eq!(
            managed_session_status(false, false, ActivityState::Working).0,
            "Working"
        );
    }

    #[gpui::test]
    fn sidebar_commands_follow_group_order_and_keep_process_identity(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "demo".into(),
                    path: "/grove-sidebar-navigation-test".into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            let ids = registry.update(cx, |registry, _| {
                (0..3)
                    .map(|_| {
                        registry.insert_meta(
                            "demo".into(),
                            "/grove-sidebar-navigation-test".into(),
                            Agent::Codex,
                        )
                    })
                    .collect::<Vec<_>>()
            });
            let tree = runtime.read(cx).tree.clone();
            tree.update(cx, |tree, _| {
                tree.set_active_worktrees(
                    0,
                    vec![grove_core::git::Worktree {
                        path: "/grove-sidebar-navigation-test".into(),
                        branch: "main".into(),
                        mtime: None,
                        is_main: true,
                    }],
                );
            });
            let activity = runtime.read(cx).activity.clone();
            activity.update(cx, |activity, _| {
                activity.set_state_for_test(ids[0], ActivityState::Idle);
                activity.set_state_for_test(ids[1], ActivityState::Done);
                activity.set_state_for_test(ids[2], ActivityState::WaitingForInput);
            });
            Sidebar::new(runtime, window, cx)
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.sync(window, cx);
                let ids: Vec<_> = sidebar
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .all()
                    .iter()
                    .map(|meta| meta.id)
                    .collect();
                sidebar.toggle_tree_list(window, cx);
                assert_eq!(
                    sidebar.visible_session_order(cx),
                    vec![ids[2], ids[1], ids[0]]
                );
                sidebar.select_numbered_session(1, window, cx);
                assert_eq!(sidebar.selected_session(), Some(ids[2]));
                sidebar.select_next_session(window, cx);
                assert_eq!(sidebar.selected_session(), Some(ids[1]));
                sidebar.select_waiting_session(window, cx);
                assert_eq!(sidebar.selected_session(), Some(ids[2]));
                sidebar.select_numbered_session(9, window, cx);
                assert_eq!(sidebar.selected_session(), Some(ids[2]));
                sidebar.toggle_grid(window, cx);
                sidebar.toggle_grid(window, cx);
                assert_eq!(sidebar.mode, ViewMode::List);
                assert_eq!(sidebar.selected_session(), Some(ids[2]));
                sidebar.toggle_tree_list(window, cx);
                assert_eq!(sidebar.mode, ViewMode::Project);
                assert_eq!(sidebar.selected_session(), Some(ids[2]));
                assert_eq!(
                    sidebar.selected_worktree(),
                    Some((0, "/grove-sidebar-navigation-test".into()))
                );
                assert_eq!(sidebar.visible_session_order(cx), ids);
                sidebar.select(Selection::Session(ids[0]), cx);
                sidebar.select_waiting_session(window, cx);
                assert_eq!(sidebar.selected_session(), Some(ids[2]));
                sidebar.select_session_id(ids[0], window, cx);
                assert_eq!(sidebar.selected_session(), Some(ids[0]));
                assert!(sidebar.session_diff_focus.contains_key(&ids[2]));
                assert!(sidebar.session_diff_observers.contains_key(&ids[2]));
                sidebar.toggle_tree_list(window, cx);
                let registry = sidebar.runtime.read(cx).registry.clone();
                registry.update(cx, |registry, _| {
                    registry.remove(ids[2]);
                });
                sidebar.sync(window, cx);
                assert!(!sidebar.session_diff_focus.contains_key(&ids[2]));
                assert!(!sidebar.session_diff_observers.contains_key(&ids[2]));
                sidebar.select_session_id(ids[2], window, cx);
                assert_ne!(sidebar.selected_session(), Some(ids[2]));
            });
        });
    }

    #[gpui::test]
    fn final_home_close_stays_empty_until_explicitly_opened_in_its_workspace(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            let mut store = grove_core::storage::Store::default();
            store.workspaces.create("Other").unwrap();
            cx.set_global(SettingsState::new(store));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
            cx.set_global(crate::theme::ThemeState::new(
                false,
                "tokyonight".into(),
                "tokyonight-day".into(),
            ));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.sync(window, cx);
                assert_eq!(sidebar.active_workspace, 2);
                sidebar.add_terminal(window, cx);
                let Some(Selection::Home(original)) = sidebar.selection else {
                    panic!("terminal action should select a shell")
                };
                assert_eq!(sidebar.terminal_owners.get(&original), Some(&2));
                sidebar.act(Action::CloseHome(original), window, cx);
                sidebar.act(Action::ConfirmHome(original), window, cx);
                assert_eq!(
                    sidebar
                        .runtime
                        .read(cx)
                        .registry
                        .read(cx)
                        .home_terminal_count(),
                    0
                );
                assert!(!sidebar.terminal_owners.contains_key(&original));
                assert_ne!(sidebar.selection, Some(Selection::Home(original)));
                sidebar.sync(window, cx);
                assert_eq!(
                    sidebar
                        .runtime
                        .read(cx)
                        .registry
                        .read(cx)
                        .home_terminal_count(),
                    0
                );
                sidebar.add_terminal(window, cx);
                let Some(Selection::Home(reopened)) = sidebar.selection else {
                    panic!("explicit terminal action should select a shell")
                };
                assert_ne!(reopened, original);
                assert_eq!(sidebar.terminal_owners.get(&reopened), Some(&2));
                assert_eq!(
                    sidebar
                        .runtime
                        .read(cx)
                        .registry
                        .read(cx)
                        .home_terminal_count(),
                    1
                );
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("home-title-2").is_some());
        assert!(cx.debug_bounds("home-status-2").is_some());
        assert!(cx.debug_bounds("home-launch-cwd-2").is_some());
    }

    #[gpui::test]
    fn nonfinal_home_close_keeps_remaining_owner_and_selection(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
            cx.set_global(crate::theme::ThemeState::new(
                false,
                "tokyonight".into(),
                "tokyonight-day".into(),
            ));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.add_terminal(window, cx);
                let Some(Selection::Home(first)) = sidebar.selection else {
                    panic!("first shell")
                };
                sidebar.add_terminal(window, cx);
                let Some(Selection::Home(second)) = sidebar.selection else {
                    panic!("second shell")
                };
                sidebar.act(Action::CloseHome(first), window, cx);
                sidebar.act(Action::ConfirmHome(first), window, cx);
                let registry = sidebar.runtime.read(cx).registry.read(cx);
                assert_eq!(registry.home_terminal_count(), 1);
                assert_eq!(registry.home_terminals()[0].id, second);
                assert_eq!(sidebar.terminal_owners.get(&second), Some(&1));
                assert_eq!(sidebar.selection, Some(Selection::Home(second)));
            });
        });
    }

    #[gpui::test]
    fn settings_control_emits_typed_request_only_outside_a_modal(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        let requests = std::rc::Rc::new(std::cell::Cell::new(0));
        cx.update(|window, cx| {
            let requests_for_event = requests.clone();
            cx.subscribe(&sidebar, move |_, event, _| {
                assert!(matches!(
                    event,
                    SidebarEvent::SettingsRequested | SidebarEvent::NewMultiProjectSessionRequested
                ));
                requests_for_event.set(requests_for_event.get() + 1);
            })
            .detach();
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::OpenSettings, window, cx);
                sidebar.act(Action::NewMultiProjectSession, window, cx);
                sidebar.pending_close = Some(SessionId::from_raw(99));
                sidebar.act(Action::OpenSettings, window, cx);
                sidebar.act(Action::NewMultiProjectSession, window, cx);
            });
        });
        assert_eq!(requests.get(), 2);
        draw(cx);
        assert!(cx.debug_bounds("sidebar-settings").is_some());
    }

    #[test]
    fn session_age_uses_activity_scale() {
        assert_eq!(elapsed_short(Duration::from_secs(59)), "59s");
        assert_eq!(elapsed_short(Duration::from_mins(1)), "1m");
        assert_eq!(elapsed_short(Duration::from_hours(1)), "1h");
        assert_eq!(elapsed_short(Duration::from_hours(24)), "1d");
        let started = Instant::now();
        assert_eq!(
            session_age_label(started, started + Duration::from_secs(59)),
            "for 59s"
        );
        assert_eq!(
            session_age_label(started, started + Duration::from_mins(1)),
            "for 1m"
        );
    }

    #[test]
    fn multi_root_inventory_keeps_unknown_roots_visible() {
        use crate::entities::workspace_state::{SnapshotProject, SnapshotWorktree};
        let meta = SessionMeta {
            id: SessionId::from_raw(1),
            project: "alpha".into(),
            wt_path: "/alpha/main".into(),
            agent: Agent::Codex,
            context_roots: vec![
                grove_core::session_meta::ContextRoot {
                    project: "alpha".into(),
                    wt_path: "/alpha/main/".into(),
                },
                grove_core::session_meta::ContextRoot {
                    project: "beta".into(),
                    wt_path: "/beta/stale/".into(),
                },
            ],
            temp_bundle_path: None,
            label: "codex 1".into(),
            restored_title: None,
            spawned_at: Instant::now(),
            attention: None,
            tmux: false,
            tmux_name: None,
        };
        let snapshot = TreeSnapshot {
            projects: vec![SnapshotProject {
                worktrees: vec![SnapshotWorktree {
                    path: "/alpha/main".into(),
                    name: "alpha".into(),
                    branch: "feature".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            session_root_inventory(&meta, &snapshot),
            ["alpha · alpha · feature", "beta · stale"]
        );
    }

    #[test]
    fn diff_status_distinguishes_unknown_clean_and_changed() {
        use grove_core::git::WorktreeGitState;
        assert_eq!(
            session_diff_status(&WorktreeGitState::default()),
            ("clean".into(), false)
        );
        assert_eq!(
            session_diff_status(&WorktreeGitState {
                dirty: true,
                ..Default::default()
            }),
            ("Changes".into(), true)
        );
        assert_eq!(
            session_diff_status(&WorktreeGitState {
                dirty: true,
                added: 3,
                removed: 2,
                ..Default::default()
            }),
            ("+3 -2".into(), true)
        );
    }

    #[gpui::test]
    fn diff_opens_for_selected_session_repo_and_restores_focus(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            runtime.read(cx).registry.clone().update(cx, |registry, _| {
                registry.insert_meta("alpha".into(), "/repo/alpha/".into(), Agent::Codex);
                registry.insert_meta("beta".into(), "/repo/beta/".into(), Agent::Claude);
            });
            let sidebar = Sidebar::new(runtime, window, cx);
            sidebar.focus.focus(window, cx);
            sidebar
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::OpenDiff(SessionId::from_raw(2)), window, cx);
                assert_eq!(
                    sidebar.diff_viewer.as_ref().unwrap().read(cx).wt_path,
                    "/repo/beta"
                );
                assert!(sidebar.diff_focus.is_focused(window));
                sidebar.act(Action::CloseDiff, window, cx);
                assert!(sidebar.diff_viewer.is_none());
                assert!(sidebar.focus.is_focused(window));
            });
        });
    }

    #[test]
    fn effective_width_clamps_desktop_and_temporarily_caps_narrow_windows() {
        assert_eq!(effective_rail_width(260.0, 1280.0), 260.0);
        assert_eq!(effective_rail_width(10.0, 1280.0), 220.0);
        assert_eq!(effective_rail_width(900.0, 1280.0), 640.0);
        assert_eq!(effective_rail_width(900.0, 800.0), 400.0);
        assert_eq!(effective_rail_width(260.0, 500.0), 200.0);
        assert_eq!(effective_rail_width(f32::NAN, 1280.0), SIDEBAR_W);
        assert_eq!(effective_rail_width(f32::INFINITY, 1280.0), SIDEBAR_W);
        assert_eq!(effective_rail_width(SIDEBAR_W, 1280.0), SIDEBAR_W);
    }

    #[test]
    fn sessions_list_orders_mixed_attention_and_activity_by_the_right_clocks() {
        let now = Instant::now();
        let row = |id, state, status, since_secs, active_secs| SessionListEntry {
            id: SessionId::from_raw(id),
            state,
            status,
            since: Some(
                now.checked_sub(Duration::from_secs(since_secs))
                    .expect("fixture clock supports elapsed seconds"),
            ),
            active: Some(
                now.checked_sub(Duration::from_secs(active_secs))
                    .expect("fixture clock supports elapsed seconds"),
            ),
        };
        let rows = [
            row(1, ActivityState::Idle, "Idle", 1, 80),
            row(2, ActivityState::Working, "Working", 1, 7),
            row(3, ActivityState::Done, "Done", 40, 2),
            row(4, ActivityState::WaitingForInput, "Needs you", 20, 1),
            row(5, ActivityState::WaitingForInput, "Needs you", 90, 1),
            row(6, ActivityState::Done, "Done", 70, 1),
            row(7, ActivityState::Idle, "Idle", 1, 3),
            row(8, ActivityState::Working, "Working", 1, 1),
            row(9, ActivityState::Exited, "Exited", 1, 1),
        ];
        assert_eq!(
            group_sessions_for_list(&rows, now),
            vec![
                ("NEEDS YOU", vec![4, 3]),
                ("REVIEW", vec![5, 2]),
                ("WORKING", vec![7, 6, 1]),
                ("IDLE", vec![0, 8]),
            ]
        );
    }

    #[test]
    fn sessions_list_ties_and_idle_dwell_have_stable_boundaries() {
        let now = Instant::now();
        let rows = [
            SessionListEntry {
                id: SessionId::from_raw(4),
                state: ActivityState::WaitingForInput,
                status: "Needs you",
                since: None,
                active: None,
            },
            SessionListEntry {
                id: SessionId::from_raw(2),
                state: ActivityState::WaitingForInput,
                status: "Needs you",
                since: None,
                active: None,
            },
            SessionListEntry {
                id: SessionId::from_raw(5),
                state: ActivityState::Idle,
                status: "Idle",
                since: None,
                active: Some(
                    now.checked_sub(crate::activity::IDLE_DWELL)
                        .expect("fixture clock supports idle dwell")
                        + Duration::from_nanos(1),
                ),
            },
            SessionListEntry {
                id: SessionId::from_raw(7),
                state: ActivityState::Idle,
                status: "Idle",
                since: None,
                active: Some(
                    now.checked_sub(crate::activity::IDLE_DWELL)
                        .expect("fixture clock supports idle dwell"),
                ),
            },
            SessionListEntry {
                id: SessionId::from_raw(3),
                state: ActivityState::Working,
                status: "Working",
                since: None,
                active: None,
            },
            SessionListEntry {
                id: SessionId::from_raw(9),
                state: ActivityState::Exited,
                status: "Exited",
                since: None,
                active: Some(now),
            },
        ];
        assert_eq!(
            group_sessions_for_list(&rows, now),
            vec![
                ("NEEDS YOU", vec![1, 0]),
                ("WORKING", vec![2, 4]),
                ("IDLE", vec![3, 5]),
            ]
        );
    }

    #[test]
    fn sessions_list_lifecycle_overrides_activity_without_changing_status() {
        let now = Instant::now();
        let rows = [
            SessionListEntry {
                id: SessionId::from_raw(1),
                state: ActivityState::Working,
                status: "Failed",
                since: None,
                active: None,
            },
            SessionListEntry {
                id: SessionId::from_raw(2),
                state: ActivityState::Done,
                status: "Starting",
                since: None,
                active: None,
            },
        ];
        assert_eq!(
            group_sessions_for_list(&rows, now),
            vec![("NEEDS YOU", vec![0]), ("WORKING", vec![1]),]
        );
        assert_eq!(rows[0].status, "Failed");
        assert_eq!(rows[1].status, "Starting");
    }

    #[gpui::test]
    fn sessions_list_cards_keep_inset_and_controls_inside_narrow_rail(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::entities::workspace_state::{SnapshotProject, SnapshotWorktree};
        let repo = ChangedGitRepo::new();
        let path = repo.path();

        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "demo".into(),
                    path: path.clone(),
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
            let registry = runtime.read(cx).registry.clone();
            let ids = (0..2)
                .map(|_| {
                    registry.update(cx, |registry, _| {
                        registry.insert_meta("demo".into(), path.clone(), Agent::Codex)
                    })
                })
                .collect::<Vec<_>>();
            runtime.read(cx).tree.clone().update(cx, |tree, _| {
                tree.set_active_worktrees(
                    0,
                    vec![grove_core::git::Worktree {
                        path: path.clone(),
                        branch: "feature/session-cards".into(),
                        mtime: None,
                        is_main: true,
                    }],
                );
                tree.apply_git_poll(
                    HashMap::from([(
                        path.clone(),
                        grove_core::git::WorktreeGitState {
                            dirty: true,
                            added: 3,
                            removed: 2,
                            ..Default::default()
                        },
                    )]),
                    &[],
                );
            });
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.mode = ViewMode::List;
            sidebar.snapshot = TreeSnapshot {
                projects: vec![SnapshotProject {
                    idx: 0,
                    name: "demo".into(),
                    sessions: ids.clone(),
                    worktrees: vec![SnapshotWorktree {
                        path: path.clone(),
                        name: "demo".into(),
                        branch: "feature/session-cards".into(),
                        sessions: ids.clone(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                total_projects: 1,
            };
            sidebar.select(Selection::Session(ids[1]), cx);
            sidebar
        });
        for width in [1280.0, 320.0] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(700.0)));
            draw(cx);
            let rail = cx.debug_bounds("sidebar-rail").unwrap();
            for (card_id, close_id) in [
                ("session-1", "close-session-1"),
                ("session-2", "close-session-2"),
            ] {
                let card = cx.debug_bounds(card_id).unwrap();
                let close = cx.debug_bounds(close_id).unwrap();
                assert!(card.left() >= rail.left() + gpui::px(SPACE_LG - 1.0));
                assert!(card.right() <= rail.right() - gpui::px(SPACE_LG - 1.0));
                assert!(card.size.height >= gpui::px(ROW_H));
                assert!(close.left() >= card.left() && close.right() <= card.right());
                assert!(close.top() >= card.top() && close.bottom() <= card.bottom());
                assert!(cx.debug_bounds("diff-chip-open-1").is_some());
            }
            for (title, context, branch, diff, status) in [
                (
                    "session-list-title-1",
                    "session-list-context-1",
                    "session-list-branch-1",
                    "session-list-diff-label-1",
                    "session-list-status-1",
                ),
                (
                    "session-list-title-2",
                    "session-list-context-2",
                    "session-list-branch-2",
                    "session-list-diff-label-2",
                    "session-list-status-2",
                ),
            ] {
                let title = cx.debug_bounds(title).unwrap();
                let context_bounds = cx.debug_bounds(context).unwrap();
                let branch_bounds = cx.debug_bounds(branch).unwrap();
                assert_eq!(context_bounds.top(), branch_bounds.top());
                assert!(branch_bounds.left() >= context_bounds.right());
                assert!(branch_bounds.right() <= rail.right());
                for selector in [context, diff] {
                    let bounds = cx.debug_bounds(selector).unwrap();
                    assert!((bounds.left() - title.left()).abs() <= gpui::px(1.0));
                    assert!(bounds.right() <= rail.right());
                }
                let status = cx.debug_bounds(status).unwrap();
                assert!(status.left() >= title.right());
                assert!(status.right() <= rail.right());
            }
            assert_eq!(
                sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
                Some(Selection::Session(SessionId::from_raw(2)))
            );
        }
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.age_timer.is_some()));
        let before = sidebar.read_with(cx, |sidebar, _| sidebar.age_ticks);
        cx.executor().advance_clock(SESSION_AGE_REFRESH);
        draw(cx);
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.age_ticks),
            before + 1
        );
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.toggle_tree_list(window, cx));
        });
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.age_timer.is_none()));
        let project = cx.debug_bounds("project-title-0").unwrap();
        let worktree_selector: &'static str =
            Box::leak(format!("worktree-title-{path}").into_boxed_str());
        let worktree = cx.debug_bounds(worktree_selector).unwrap();
        let first = cx.debug_bounds("session-agent-1").unwrap();
        let second = cx.debug_bounds("session-agent-2").unwrap();
        assert_eq!(project.left() + gpui::px(SPACE_LG), worktree.left());
        assert_eq!(first.left(), project.left() + gpui::px(HIERARCHY_INSET));
        assert_eq!(first.left(), second.left());
        assert!(cx.debug_bounds("diff-chip-open-1").is_some());
        assert!(cx.debug_bounds("diff-chip-open-2").is_some());
        assert!(cx.debug_bounds("session-number-1").is_none());
        let diff_focus = sidebar.read_with(cx, |sidebar, _| {
            sidebar.session_diff_focus[&SessionId::from_raw(1)].clone()
        });
        cx.update(|window, cx| diff_focus.focus(window, cx));
        draw(cx);
        cx.update(|window, _| assert!(diff_focus.is_focused(window)));
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.diff_viewer.is_some()));
    }

    #[gpui::test]
    fn sessions_list_polls_real_git_changes_and_opens_diff_chip(cx: &mut gpui::TestAppContext) {
        let repo = ChangedGitRepo::new();
        let path = repo.path();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "poll demo".into(),
                    path: path.clone(),
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
            runtime.read(cx).registry.clone().update(cx, |registry, _| {
                registry.insert_meta("poll demo".into(), path.clone(), Agent::Codex);
            });
            runtime.read(cx).tree.clone().update(cx, |tree, _| {
                tree.set_active_worktrees(
                    0,
                    vec![grove_core::git::Worktree {
                        path: path.clone(),
                        branch: "main".into(),
                        mtime: None,
                        is_main: true,
                    }],
                );
            });
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.mode = ViewMode::List;
            sidebar
        });
        draw(cx);
        draw(cx);
        let git = sidebar.read_with(cx, |sidebar, cx| {
            sidebar.runtime.read(cx).tree.read(cx).git_states()
        });
        assert_eq!(
            git.get(&path)
                .map(|state| (state.dirty, state.added, state.removed)),
            Some((true, 1, 0))
        );
        assert!(cx.debug_bounds("diff-chip-open-1").is_some());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.toggle_tree_list(window, cx));
        });
        draw(cx);
        let diff = cx.debug_bounds("diff-chip-open-1").unwrap();
        let row = cx.debug_bounds("session-1").unwrap();
        let outside = gpui::point(row.center().x, diff.center().y);
        assert!(outside.x > diff.right());
        cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
        draw(cx);
        cx.simulate_click(outside, gpui::Modifiers::default());
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.diff_viewer.is_none()));
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            Some(Selection::Session(SessionId::from_raw(1)))
        );
        cx.simulate_mouse_move(diff.center(), None, gpui::Modifiers::default());
        draw(cx);
        cx.simulate_click(diff.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.diff_viewer.is_some()));
    }
    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn palette_session_focus_reaches_new_terminal_in_list_and_grid(cx: &mut gpui::TestAppContext) {
        let path = env!("CARGO_MANIFEST_DIR").to_string();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "current".into(),
                    path: path.clone(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                tmux_enabled: Some(false),
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
            cx.set_global(crate::theme::ThemeState::new(
                false,
                "tokyonight".into(),
                "tokyonight-day".into(),
            ));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.mode = ViewMode::List;
            sidebar
        });
        draw(cx);

        for mode in [ViewMode::List, ViewMode::Grid] {
            let id = cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.mode = mode;
                    let prior_focus = cx.focus_handle();
                    prior_focus.focus(window, cx);
                    let runtime = sidebar.runtime.clone();
                    let did_spawn = runtime.update(cx, |runtime, cx| {
                        runtime.spawn_session_in_with_context(
                            "current".into(),
                            path.clone(),
                            Agent::Terminal,
                            vec!["\0".into()],
                            Vec::new(),
                            None,
                            cx,
                        )
                    });
                    assert!(!did_spawn);
                    let id = runtime
                        .read(cx)
                        .state
                        .read(cx)
                        .active_session()
                        .expect("failed session is registered");
                    sidebar.focus_active_session_after_palette(window, cx);
                    assert_eq!(sidebar.selection, Some(Selection::Session(id)));
                    assert!(sidebar.focus.is_focused(window));
                    assert_eq!(sidebar.pending_canvas_focus, Some(id));
                    assert_eq!(
                        sidebar.pending_grid_workspace_focus,
                        (mode == ViewMode::Grid).then_some(id)
                    );
                    id
                })
            });
            draw(cx);
            cx.update(|window, cx| {
                let sidebar = sidebar.read(cx);
                assert!(sidebar.terminal_views[&id]
                    .focus_handle(cx)
                    .is_focused(window));
            });
        }
    }

    #[gpui::test]
    fn project_title_has_descender_room_and_header_controls_align_at_zoom(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: ["grove", "alpha", "beta"]
                    .into_iter()
                    .map(|name| grove_core::storage::Project {
                        name: name.into(),
                        path: format!("/grove-project-line-height-{name}"),
                        scripts: grove_core::storage::ProjectScripts::default(),
                        archived: false,
                        worktree_dir: None,
                    })
                    .collect(),
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        for zoom in [0.6, 1.0, 2.0] {
            cx.update(|window, _| window.set_rem_size(gpui::px(16.0 * zoom)));
            draw(cx);
            let row = cx.debug_bounds("project-0").unwrap();
            let title = cx.debug_bounds("project-title-0").unwrap();
            assert!(
                f32::from(title.size.height) >= PROJECT_TITLE_LINE_H * zoom - 1.0,
                "project title line is too short at {zoom}x"
            );
            assert!(title.top() >= row.top() && title.bottom() <= row.bottom());
            assert!((f32::from(row.size.height) - PROJECT_ROW_H * zoom).abs() <= 1.0);
            let heading = cx.debug_bounds("sidebar-heading-label").unwrap();
            let count = cx.debug_bounds("projects-count").unwrap();
            assert!(
                (f32::from(count.left() - heading.right()) - SPACE_SM * zoom).abs() <= 1.0,
                "project count is not adjacent to its title at {zoom}x"
            );
            let centers = [
                "sidebar-view",
                "sidebar-grid",
                "projects-archive",
                "projects-add",
            ]
            .map(|id| f32::from(cx.debug_bounds(id).unwrap().center().x));
            for pair in centers.windows(2) {
                assert!(
                    (pair[1] - pair[0] - (CHROME_CONTROL_H + SPACE_SM) * zoom).abs() <= 1.0,
                    "header spacing differs at {zoom}x: {centers:?}"
                );
            }
            for id in [
                "sidebar-view",
                "sidebar-grid",
                "projects-archive",
                "projects-add",
            ] {
                assert!(
                    (f32::from(cx.debug_bounds(id).unwrap().size.width) - CHROME_CONTROL_H * zoom)
                        .abs()
                        <= 1.0,
                    "{id} has a different slot width at {zoom}x"
                );
            }
        }
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn heading_intrinsic_width_does_not_trigger_geist_truncation() {
        use gpui::AssetSource as _;
        let platform = gpui_platform::current_platform(true);
        let text_system = std::sync::Arc::new(gpui::TextSystem::new(platform.text_system()));
        let fonts = ["fonts/Geist-Regular.ttf", "fonts/Geist-Medium.ttf"]
            .map(|path| crate::assets::Assets.load(path).unwrap().unwrap());
        text_system.add_fonts(fonts.into()).unwrap();
        let window_text_system = gpui::WindowTextSystem::new(text_system.clone());
        let style = gpui::TextStyle {
            font_family: crate::fonts::UI_FAMILY.into(),
            font_weight: gpui::FontWeight::MEDIUM,
            ..Default::default()
        };
        for zoom in [0.9, 1.0, 1.1, 1.2, 1.5, 2.0] {
            for label in ["Projects", "Sessions", ""] {
                let size = gpui::px(TEXT_SMALL * zoom);
                let run = style.to_run(label.len());
                let width = navigation_label_width(label, size, &run, &window_text_system);
                let mut wrapper = text_system.line_wrapper(run.font, size);
                assert_eq!(
                    wrapper.should_truncate_line(label, width, "…", gpui::TruncateFrom::End),
                    None,
                    "{label} incorrectly truncates at {zoom}x with preferred width {width:?}"
                );
            }
        }
    }

    #[gpui::test]
    fn project_heading_fits_full_text_at_zoom(cx: &mut gpui::TestAppContext) {
        struct Fixture(Entity<Sidebar>);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .font_family(crate::fonts::UI_FAMILY)
                    .child(self.0.clone())
            }
        }
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: (0..4)
                    .map(|index| grove_core::storage::Project {
                        name: format!("project-{index}"),
                        path: format!("/grove-heading-{index}"),
                        scripts: grove_core::storage::ProjectScripts::default(),
                        archived: false,
                        worktree_dir: None,
                    })
                    .collect(),
                sidebar_width: Some(300.0),
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Fixture(cx.new(|cx| Sidebar::new(runtime, window, cx)))
        });
        for rail_width in [260.0, 280.0, 300.0] {
            cx.update(|_, cx| {
                cx.global_mut::<SettingsState>().store.sidebar_width = Some(rail_width);
            });
            for zoom in [0.9, 1.0, 1.1, 1.2, 1.5, 2.0] {
                cx.update(|window, _| window.set_rem_size(gpui::px(16.0 * zoom)));
                draw(cx);
                let heading = cx.debug_bounds("sidebar-heading-label").unwrap();
                let width = cx.update(|window, _| {
                    let text: SharedString = "Projects".into();
                    let mut run = window.text_style().to_run(text.len());
                    run.font.weight = gpui::FontWeight::MEDIUM;
                    run.font.family = crate::fonts::UI_FAMILY.into();
                    let font_id = window.text_system().resolve_font(&run.font);
                    let glyph_width: gpui::Pixels = text
                        .chars()
                        .map(|ch| {
                            window.text_system().layout_width(
                                font_id,
                                gpui::px(TEXT_SMALL * zoom),
                                ch,
                            )
                        })
                        .sum();
                    let shaped_width = window
                        .text_system()
                        .shape_line(text, gpui::px(TEXT_SMALL * zoom), &[run], None)
                        .width();
                    shaped_width.max(glyph_width.floor())
                });
                assert!(
                    heading.size.width >= width,
                    "heading truncated at {zoom}x: {:?}, full text width {width:?}",
                    heading.size.width
                );
            }
        }
        // A genuinely narrow viewport must still shrink the label, keeping the
        // count and controls visible instead of forcing the word to overflow.
        cx.update(|window, _| window.set_rem_size(gpui::px(16.0)));
        draw(cx);
        let full_width = cx.debug_bounds("sidebar-heading-label").unwrap().size.width;
        cx.simulate_resize(gpui::size(gpui::px(500.0), gpui::px(800.0)));
        draw(cx);
        let heading = cx.debug_bounds("sidebar-heading-label").unwrap();
        let count = cx.debug_bounds("projects-count").unwrap();
        let control = cx.debug_bounds("sidebar-view").unwrap();
        assert!(heading.size.width < full_width);
        assert!(heading.right() < count.left());
        assert!((f32::from(count.left() - heading.right()) - SPACE_SM).abs() <= 1.0);
        assert!(count.right() <= control.left());
    }

    #[gpui::test]
    fn project_title_toggles_without_selecting_or_preserving_descendants(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            let projects = ["one", "two"]
                .into_iter()
                .map(|name| grove_core::storage::Project {
                    name: name.into(),
                    path: format!("/grove-project-select-test-{name}"),
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
            let registry = runtime.read(cx).registry.clone();
            registry.update(cx, |registry, _| {
                registry.insert_meta(
                    "one".into(),
                    "/grove-project-select-test-one".into(),
                    Agent::Codex,
                );
            });
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        let first = cx.debug_bounds("project-0").unwrap();
        let second = cx.debug_bounds("project-1").unwrap();
        assert!(f32::from(second.top() - first.bottom()) >= SPACE_20);
        assert!(cx.debug_bounds("tree-expand-cycle").is_none());
        assert!(cx.debug_bounds("projects-count").is_some());
        assert!(cx.debug_bounds("project-count-0").is_none());
        assert!(cx.debug_bounds("project-count-1").is_none());
        assert!(cx.debug_bounds("session-1").is_some());
        let initial_selection = sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone());
        let title = cx.debug_bounds("project-title-0").unwrap().center();
        cx.simulate_click(title, gpui::Modifiers::default());
        draw(cx);
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            initial_selection
        );
        assert!(cx.debug_bounds("session-1").is_none());
        assert!(cx.debug_bounds("project-1").is_some());
    }

    #[gpui::test]
    fn project_folder_toggle_hides_only_its_descendants_and_preserves_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        let paths = ["/grove-folder-toggle-one", "/grove-folder-toggle-two"];
        cx.update(|cx| {
            gpui_component::init(cx);
            let projects = paths
                .iter()
                .enumerate()
                .map(|(index, path)| grove_core::storage::Project {
                    name: format!("project-{index}"),
                    path: (*path).into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                })
                .collect();
            let mut store = grove_core::storage::Store {
                projects,
                ..Default::default()
            };
            store.workspaces.create("Other").unwrap();
            store.workspaces.select(1);
            cx.set_global(SettingsState::new(store));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            runtime.read(cx).registry.clone().update(cx, |registry, _| {
                for (index, path) in paths.iter().enumerate() {
                    registry.insert_meta(format!("project-{index}"), (*path).into(), Agent::Codex);
                }
            });
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        assert!(cx
            .debug_bounds("worktree-/grove-folder-toggle-one")
            .is_some());
        assert!(cx
            .debug_bounds("worktree-/grove-folder-toggle-two")
            .is_some());
        assert!(cx.debug_bounds("session-1").is_some());
        assert!(cx.debug_bounds("session-2").is_some());
        let initial = sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone());
        let folder = cx.debug_bounds("project-folder-toggle-0").unwrap().center();
        cx.simulate_click(folder, gpui::Modifiers::default());
        draw(cx);
        sidebar.read_with(cx, |sidebar, cx| {
            assert!(sidebar.collapsed_projects.contains(paths[0]));
            assert_eq!(sidebar.selection, initial);
            assert_eq!(
                sidebar.visible_session_order(cx),
                vec![SessionId::from_raw(2)]
            );
            assert!(sidebar.menu.is_none());
        });
        assert!(cx.debug_bounds("project-0").is_some());
        assert!(cx
            .debug_bounds("worktree-/grove-folder-toggle-one")
            .is_none());
        assert!(cx.debug_bounds("session-1").is_none());
        assert!(cx.debug_bounds("terminal-header-1").is_some());
        assert!(cx
            .debug_bounds("worktree-/grove-folder-toggle-two")
            .is_some());
        assert!(cx.debug_bounds("session-2").is_some());
        let title = cx.debug_bounds("project-title-0").unwrap().center();
        cx.simulate_click(title, gpui::Modifiers::default());
        draw(cx);
        sidebar.read_with(cx, |sidebar, _| {
            assert_eq!(sidebar.selection, initial);
            assert!(!sidebar.collapsed_projects.contains(paths[0]));
        });
        cx.update(|_, cx| cx.global_mut::<SettingsState>().store.workspaces.select(2));
        draw(cx);
        assert!(cx.debug_bounds("project-0").is_none());
        cx.update(|_, cx| cx.global_mut::<SettingsState>().store.workspaces.select(1));
        draw(cx);
        assert!(cx
            .debug_bounds("worktree-/grove-folder-toggle-one")
            .is_some());
        cx.update(|window, cx| {
            let handle = sidebar.read(cx).project_toggle_focus[paths[0]].clone();
            handle.focus(window, cx);
        });
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(cx
            .debug_bounds("worktree-/grove-folder-toggle-one")
            .is_none());
        assert!(cx.debug_bounds("session-1").is_none());
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            initial
        );
        cx.simulate_keystrokes("space");
        draw(cx);
        assert!(cx
            .debug_bounds("worktree-/grove-folder-toggle-one")
            .is_some());
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, _| sidebar.selection = None);
            cx.global_mut::<SettingsState>().store.workspaces.select(2);
        });
        draw(cx);
        cx.update(|_, cx| cx.global_mut::<SettingsState>().store.workspaces.select(1));
        draw(cx);
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            Some(Selection::Session(SessionId::from_raw(1)))
        );
        cx.update(|window, cx| {
            cx.global_mut::<SettingsState>().store.projects.remove(0);
            sidebar.update(cx, |sidebar, cx| sidebar.sync(window, cx));
        });
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.collapsed_projects.is_empty()));
    }

    #[gpui::test]
    fn project_menu_click_preserves_selection_and_rows(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "one".into(),
                    path: "/grove-project-menu-test".into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        let before = sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone());
        let menu = cx.debug_bounds("project-menu-0").unwrap().center();
        cx.simulate_click(menu, gpui::Modifiers::default());
        draw(cx);
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            before
        );
        assert!(cx.debug_bounds("project-0").is_some());
        assert!(cx.debug_bounds("project-actions-popup").is_some());
    }

    #[gpui::test]
    fn project_hierarchy_spaces_worktree_groups_more_than_sibling_sessions(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "demo".into(),
                    path: "/grove-spacing-main".into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            registry.update(cx, |registry, _| {
                registry.insert_meta("demo".into(), "/grove-spacing-main".into(), Agent::Codex);
                registry.insert_meta("demo".into(), "/grove-spacing-main".into(), Agent::Claude);
                registry.insert_meta(
                    "demo".into(),
                    "/grove-spacing-feature".into(),
                    Agent::Terminal,
                );
            });
            runtime.read(cx).tree.clone().update(cx, |tree, _| {
                tree.set_active_worktrees(
                    0,
                    vec![
                        grove_core::git::Worktree {
                            path: "/grove-spacing-main".into(),
                            branch: "main".into(),
                            mtime: None,
                            is_main: true,
                        },
                        grove_core::git::Worktree {
                            path: "/grove-spacing-feature".into(),
                            branch: "feature".into(),
                            mtime: None,
                            is_main: false,
                        },
                    ],
                );
            });
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        let labels = sidebar.read_with(cx, |sidebar, _| {
            sidebar.snapshot.projects[0]
                .worktrees
                .iter()
                .map(|worktree| sidebar_worktree_name(&worktree.name, worktree.is_main).to_owned())
                .collect::<Vec<_>>()
        });
        assert_eq!(labels, ["Main checkout", "grove-spacing-feature"]);
        assert!(cx
            .debug_bounds("worktree-title-/grove-spacing-main")
            .is_some());
        assert!(cx
            .debug_bounds("worktree-title-/grove-spacing-feature")
            .is_some());
        let first = cx.debug_bounds("session-1").unwrap();
        let second = cx.debug_bounds("session-2").unwrap();
        let next_worktree = cx.debug_bounds("worktree-/grove-spacing-feature").unwrap();
        assert!(
            f32::from(next_worktree.top() - second.bottom())
                > f32::from(second.top() - first.bottom())
        );
        for width in [220.0, 260.0, 320.0] {
            cx.update(|window, cx| {
                cx.global_mut::<SettingsState>().store.sidebar_width = Some(width);
                sidebar.update(cx, |sidebar, cx| sidebar.sync(window, cx));
            });
            draw(cx);
            let launcher = cx.debug_bounds("session-agent-1").unwrap();
            let title = cx.debug_bounds("session-tree-title-1").unwrap();
            let metadata = cx.debug_bounds("session-tree-metadata-1").unwrap();
            let status = cx.debug_bounds("session-tree-status-1").unwrap();
            let close = cx.debug_bounds("session-tree-close-1").unwrap();
            let launcher_label = cx.debug_bounds("session-agent-label-1").unwrap();
            let project_title = cx.debug_bounds("project-title-0").unwrap();
            let worktree_title = cx
                .debug_bounds("worktree-title-/grove-spacing-main")
                .unwrap();
            assert_eq!(project_title.left(), worktree_title.left());
            assert_eq!(worktree_title.left(), launcher_label.left());
            assert_eq!(launcher_label.left(), title.left());
            assert_eq!(title.left(), metadata.left());
            assert!(status.left() >= launcher.right());
            assert!(status.bottom() <= title.top());
            assert!(close.top() >= status.bottom());
        }
        assert!(cx.debug_bounds("session-3").is_some());
        assert!(cx.debug_bounds("fold-/grove-spacing-feature").is_none());
    }

    #[gpui::test]
    fn run_script_sidebar_control_and_normal_canvas_lifecycle(cx: &mut gpui::TestAppContext) {
        struct ScriptRepo(std::path::PathBuf);
        impl Drop for ScriptRepo {
            fn drop(&mut self) {
                let _ = fs_err::remove_dir_all(&self.0);
            }
        }
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let repo = ScriptRepo(std::env::temp_dir().join(format!(
            "grove-sidebar-script-test-{}-{unique}",
            std::process::id()
        )));
        fs_err::create_dir(&repo.0).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&repo.0)
            .status()
            .unwrap()
            .success());
        let path = repo
            .0
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "script demo".into(),
                    path: path.clone(),
                    scripts: grove_core::storage::ProjectScripts {
                        run: Some(" \n ".into()),
                        ..Default::default()
                    },
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
            cx.set_global(crate::theme::ThemeState::new(
                false,
                "tokyonight".into(),
                "tokyonight-day".into(),
            ));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        let run_selector: &'static str = Box::leak(format!("run-script-{path}").into_boxed_str());
        assert!(cx.debug_bounds(run_selector).is_none());
        let actions_selector: &'static str =
            Box::leak(format!("worktree-actions-{path}").into_boxed_str());
        let worktree_selector: &'static str =
            Box::leak(format!("worktree-{path}").into_boxed_str());
        let count_selector: &'static str =
            Box::leak(format!("worktree-count-{path}").into_boxed_str());
        let actions_right = cx.debug_bounds(actions_selector).unwrap().right();
        assert_eq!(
            f32::from(cx.debug_bounds(actions_selector).unwrap().size.width),
            HIERARCHY_TRAILING_W
        );
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.select(Selection::Project(0), cx);
                sidebar.focus.focus(window, cx);
                let focus = window.focused(cx);
                sidebar.act(Action::RunScript(path.clone(), path.clone()), window, cx);
                sidebar.act(
                    Action::RunScript(path.clone(), format!("{path}/missing")),
                    window,
                    cx,
                );
                assert_eq!(sidebar.selection, Some(Selection::Project(0)));
                assert_eq!(window.focused(cx), focus);
                assert!(sidebar.runtime.read(cx).registry.read(cx).all().is_empty());
            });
        });

        cx.update(|window, cx| {
            cx.global_mut::<SettingsState>().store.projects[0]
                .scripts
                .run = Some("\0".into());
            sidebar.update(cx, |sidebar, cx| sidebar.sync(window, cx));
        });
        draw(cx);
        assert!(cx.debug_bounds(run_selector).is_some());
        assert_eq!(
            cx.debug_bounds(actions_selector).unwrap().right(),
            actions_right
        );
        assert!(cx.debug_bounds(count_selector).is_none());
        assert!(cx
            .debug_bounds(worktree_selector)
            .unwrap()
            .contains(&cx.debug_bounds(run_selector).unwrap().center()));
        let run = cx.debug_bounds(run_selector).unwrap().center();
        cx.simulate_mouse_down(run, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(run, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                assert_eq!(sidebar.selection, Some(Selection::Project(0)));
                assert!(sidebar.runtime.read(cx).registry.read(cx).all().is_empty());
                assert!(sidebar
                    .content_error
                    .as_deref()
                    .is_some_and(|error| error.starts_with("terminal failed:")));
                let focus = window.focused(cx);
                sidebar.act(Action::RunScript(path.clone(), path.clone()), window, cx);
                assert_eq!(window.focused(cx), focus);
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("canvas-overview").is_some());

        // Invalid script bytes produce no PTY reader. These registered sessions
        // exercise the normal canvas and close path without racing GPUI's test scheduler.
        let (first_id, second_id) = cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let registry = sidebar.runtime.read(cx).registry.clone();
                let mut ids = Vec::new();
                for _ in 0..2 {
                    let session = cx.new(|cx| {
                        crate::entities::terminal_session::TerminalSession::spawn_script(
                            "\0", &path, cx,
                        )
                    });
                    ids.push(registry.update(cx, |registry, cx| {
                        let id = registry.insert_meta(
                            "script demo".into(),
                            path.clone(),
                            Agent::Terminal,
                        );
                        registry.attach(id, session, None);
                        cx.notify();
                        id
                    }));
                }
                sidebar.sync(window, cx);
                let snap = sidebar.snapshot.clone();
                sidebar
                    .runtime
                    .read(cx)
                    .state
                    .clone()
                    .update(cx, |state, cx| {
                        state.select_session(ids[0], &snap);
                        cx.notify();
                    });
                assert_eq!(sidebar.select_active_session(cx), Some(ids[0]));
                (ids[0], ids[1])
            })
        });
        draw(cx);
        let first_header: &'static str =
            Box::leak(format!("terminal-header-{}", first_id.raw()).into_boxed_str());
        assert!(cx.debug_bounds(first_header).is_some());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                assert!(sidebar.terminal_views[&first_id]
                    .focus_handle(cx)
                    .contains_focused(window, cx));
                let snap = sidebar.snapshot.clone();
                sidebar
                    .runtime
                    .read(cx)
                    .state
                    .clone()
                    .update(cx, |state, cx| {
                        state.select_session(second_id, &snap);
                        cx.notify();
                    });
                assert_eq!(sidebar.select_active_session(cx), Some(second_id));
            });
        });
        draw(cx);
        let second_header: &'static str =
            Box::leak(format!("terminal-header-{}", second_id.raw()).into_boxed_str());
        assert!(cx.debug_bounds(second_header).is_some());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                assert!(sidebar.terminal_views[&second_id]
                    .focus_handle(cx)
                    .contains_focused(window, cx));
                sidebar.act(Action::Select(Selection::Session(first_id)), window, cx);
                assert_eq!(sidebar.selection, Some(Selection::Session(first_id)));
                sidebar.act(Action::Close(first_id), window, cx);
                sidebar.act(Action::ConfirmClose(first_id), window, cx);
                let registry = sidebar.runtime.read(cx).registry.read(cx);
                assert!(registry.meta(first_id).is_none());
                assert!(registry.meta(second_id).is_some());
                sidebar.act(Action::Select(Selection::Session(second_id)), window, cx);
                assert_eq!(sidebar.selection, Some(Selection::Session(second_id)));
                sidebar.act(Action::Close(second_id), window, cx);
                sidebar.act(Action::ConfirmClose(second_id), window, cx);
                assert!(sidebar.runtime.read(cx).registry.read(cx).all().is_empty());
            });
        });
    }

    #[gpui::test]
    fn worktree_launch_controls_route_opencode_and_ignore_missing_backends(
        cx: &mut gpui::TestAppContext,
    ) {
        let project_path = "/grove-worktree-launch-ui-test";
        let worktree_path = format!("{project_path}/missing-feature");
        let launch_ids = [
            "launch-/grove-worktree-launch-ui-test/missing-feature-0",
            "launch-/grove-worktree-launch-ui-test/missing-feature-1",
            "launch-/grove-worktree-launch-ui-test/missing-feature-2",
            "launch-/grove-worktree-launch-ui-test/missing-feature-3",
        ];
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
            cx.set_global(crate::theme::ThemeState::new(
                false,
                "tokyonight".into(),
                "tokyonight-day".into(),
            ));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let mut sidebar = Sidebar::new(runtime.clone(), window, cx);
            sidebar.available = [false, false, false, true];
            runtime.read(cx).tree.clone().update(cx, |tree, _| {
                tree.set_active_worktrees(
                    0,
                    vec![
                        grove_core::git::Worktree {
                            path: project_path.into(),
                            branch: "main".into(),
                            mtime: None,
                            is_main: true,
                        },
                        grove_core::git::Worktree {
                            path: worktree_path.clone(),
                            branch: "feature".into(),
                            mtime: None,
                            is_main: false,
                        },
                    ],
                );
            });
            sidebar
        });
        draw(cx);
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.select(Selection::Project(0), cx);
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("project-start-card").is_some());
        assert!(cx.debug_bounds("start-terminal").is_some());
        assert!(cx.debug_bounds("project-initialize-git").is_some());
        assert!(cx.debug_bounds("project-new-worktree").is_none());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::NewWorktree(project_path.into()), window, cx);
                assert!(sidebar.pending_new_worktree.is_none());
                assert!(sidebar
                    .content_error
                    .as_deref()
                    .unwrap()
                    .contains("Initialize Git"));
            });
        });
        for id in launch_ids {
            assert!(cx.debug_bounds(id).is_some());
        }
        assert_eq!(
            f32::from(
                cx.debug_bounds("worktree-actions-/grove-worktree-launch-ui-test/missing-feature")
                    .unwrap()
                    .size
                    .width
            ),
            HIERARCHY_TRAILING_W
        );
        let opencode = cx.debug_bounds(launch_ids[2]).unwrap().center();
        cx.simulate_mouse_down(
            opencode,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            opencode,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                assert_eq!(sidebar.runtime.read(cx).registry.read(cx).len(), 0);
                sidebar.pending_new_worktree = Some(0);
                sidebar.act(
                    Action::Launch(0, worktree_path.clone(), Agent::OpenCode),
                    window,
                    cx,
                );
                assert_eq!(sidebar.pending_new_worktree, Some(0));
                assert_eq!(sidebar.runtime.read(cx).registry.read(cx).len(), 0);
                assert!(sidebar.content_error.as_deref().unwrap().contains("PATH"));
                sidebar.pending_new_worktree = None;
                sidebar.available[2] = true;
                cx.notify();
            });
        });
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                assert_eq!(
                    sidebar.launch_target(0, worktree_path.clone(), Agent::OpenCode, cx),
                    Some(("demo".into(), worktree_path.clone(), Agent::OpenCode))
                );
                assert_eq!(
                    sidebar.launch_target(0, project_path.into(), Agent::Terminal, cx),
                    Some(("demo".into(), project_path.into(), Agent::Terminal))
                );
                assert_eq!(
                    sidebar.launch_target(0, "/stale/worktree".into(), Agent::OpenCode, cx),
                    None
                );
                let id = sidebar
                    .runtime
                    .read(cx)
                    .registry
                    .clone()
                    .update(cx, |registry, _| {
                        registry.insert_meta("demo".into(), worktree_path.clone(), Agent::Codex)
                    });
                sidebar.select(Selection::Session(id), cx);
                sidebar.selection = None;
            });
            let duplicate = cx.global::<SettingsState>().store.projects[0].clone();
            cx.global_mut::<SettingsState>()
                .store
                .projects
                .push(duplicate);
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(
                    Action::Launch(0, worktree_path.clone(), Agent::OpenCode),
                    window,
                    cx,
                );
                assert_eq!(sidebar.selection, None);
                assert_eq!(sidebar.runtime.read(cx).registry.read(cx).len(), 1);
                assert_eq!(
                    sidebar.runtime.read(cx).state.read(cx).active_session(),
                    Some(sidebar.runtime.read(cx).registry.read(cx).all()[0].id)
                );
            });
        });
    }
    #[test]
    fn display_titles_remove_only_transient_braille_prefixes() {
        for title in [
            " ⠋ Respond to a greeting ",
            "⠹ · Respond to a greeting",
            "⠋— Respond to a greeting",
            "✳ Respond to a greeting",
            "✳ · Respond to a greeting",
            "✳— Respond to a greeting",
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
            "✳literal",
            "A ✳ literal star",
            "* Keep literal star",
            "✳️Keep literal variation selector",
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
            cx.set_global(crate::zoom::ZoomState::new(1.0));
            cx.set_global(crate::theme::ThemeState::new(
                false,
                "tokyonight".into(),
                "tokyonight-day".into(),
            ));
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
                            restored_title: None,
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
    fn project_hierarchy_trailing_actions_align_with_blank_worktree_slot(
        cx: &mut gpui::TestAppContext,
    ) {
        let path = "/grove-sidebar-trailing-alignment";
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "alignment".into(),
                    path: path.into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        let centers = [
            "project-menu-glyph-0",
            "worktree-actions-/grove-sidebar-trailing-alignment",
        ]
        .map(|selector| f32::from(cx.debug_bounds(selector).unwrap().center().x));
        assert!(cx
            .debug_bounds("worktree-/grove-sidebar-trailing-alignment")
            .is_some());
        assert!(cx.debug_bounds("worktrees-count-0").is_none());
        assert!(cx
            .debug_bounds("worktree-count-/grove-sidebar-trailing-alignment")
            .is_none());
        assert!(
            centers
                .iter()
                .all(|center| (center - centers[0]).abs() <= 1.0),
            "{centers:?}"
        );
    }

    #[gpui::test]
    fn workspace_restoration_focuses_selected_cached_terminal(cx: &mut gpui::TestAppContext) {
        let paths = ["/grove-focus-one", "/grove-focus-two"];
        cx.update(|cx| {
            gpui_component::init(cx);
            let mut store = grove_core::storage::Store {
                projects: paths
                    .iter()
                    .enumerate()
                    .map(|(index, path)| grove_core::storage::Project {
                        name: format!("project-{index}"),
                        path: (*path).into(),
                        scripts: grove_core::storage::ProjectScripts::default(),
                        archived: false,
                        worktree_dir: None,
                    })
                    .collect(),
                ..Default::default()
            };
            store.workspaces.create("Other").unwrap();
            store.assign_project_to_active_workspace(paths[1]);
            store.workspaces.select(1);
            cx.set_global(SettingsState::new(store));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
            cx.set_global(crate::theme::ThemeState::new(
                false,
                "tokyonight".into(),
                "tokyonight-day".into(),
            ));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            for (index, path) in paths.iter().enumerate() {
                let session = cx.new(|cx| {
                    crate::entities::terminal_session::TerminalSession::attach_existing(
                        "grove-focus-test-not-attached",
                        24,
                        80,
                        cx,
                    )
                });
                registry.update(cx, |registry, cx| {
                    let id = registry.insert_meta(
                        format!("project-{index}"),
                        (*path).into(),
                        Agent::Codex,
                    );
                    registry.attach(id, session, None);
                    cx.notify();
                });
            }
            runtime.read(cx).tree.clone().update(cx, |tree, _| {
                for (index, path) in paths.iter().enumerate() {
                    tree.set_active_worktrees(
                        index,
                        vec![grove_core::git::Worktree {
                            path: (*path).into(),
                            branch: "main".into(),
                            mtime: None,
                            is_main: true,
                        }],
                    );
                }
            });
            let picker =
                cx.new(|cx| super::super::workspace_manager::WorkspaceManager::new(window, cx));
            let shell_focus = cx.focus_handle();
            shell_focus.focus(window, cx);
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.set_workspace_selector(picker);
            sidebar.set_shell_focus(shell_focus);
            sidebar
        });
        draw(cx);
        cx.update(|window, cx| {
            let sidebar = sidebar.read(cx);
            let first = sidebar.runtime.read(cx).registry.read(cx).all()[0].id;
            assert_eq!(sidebar.selection, Some(Selection::Session(first)));
            assert!(sidebar.terminal_views[&first]
                .focus_handle(cx)
                .is_focused(window));
        });
        cx.update(|window, cx| {
            let picker = sidebar.read(cx).workspace_selector.clone().unwrap();
            picker.focus_handle(cx).focus(window, cx);
            cx.global_mut::<SettingsState>().store.workspaces.select(2);
        });
        draw(cx);
        cx.update(|window, cx| {
            let sidebar = sidebar.read(cx);
            let second = sidebar.runtime.read(cx).registry.read(cx).all()[1].id;
            assert_eq!(sidebar.selection, Some(Selection::Session(second)));
            assert!(sidebar.terminal_views[&second]
                .focus_handle(cx)
                .is_focused(window));
        });
        cx.update(|window, cx| {
            let picker = sidebar.read(cx).workspace_selector.clone().unwrap();
            picker.focus_handle(cx).focus(window, cx);
            cx.global_mut::<SettingsState>().store.workspaces.select(1);
        });
        draw(cx);
        cx.update(|window, cx| {
            let sidebar = sidebar.read(cx);
            let first = sidebar.runtime.read(cx).registry.read(cx).all()[0].id;
            assert_eq!(sidebar.selection, Some(Selection::Session(first)));
            assert!(sidebar.terminal_views[&first]
                .focus_handle(cx)
                .is_focused(window));
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.select(Selection::Project(0), cx));
            let picker = sidebar.read(cx).workspace_selector.clone().unwrap();
            picker.focus_handle(cx).focus(window, cx);
            cx.global_mut::<SettingsState>().store.workspaces.select(2);
        });
        draw(cx);
        cx.update(|window, cx| {
            let picker = sidebar.read(cx).workspace_selector.clone().unwrap();
            picker.focus_handle(cx).focus(window, cx);
            cx.global_mut::<SettingsState>().store.workspaces.select(1);
        });
        draw(cx);
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
            Some(Selection::Project(0))
        );
    }

    #[gpui::test]
    fn restored_title_is_visible_before_terminal_attach(cx: &mut gpui::TestAppContext) {
        let path = "/grove-restored-title-test";
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "demo".into(),
                    path: path.into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            runtime.read(cx).registry.clone().update(cx, |registry, _| {
                registry.insert_reattached(
                    0,
                    &grove_core::tmux::DiscoveredSession {
                        name: "grove__restored_title_test".into(),
                        pane_title: Some("Continue migration".into()),
                        wt_path: path.into(),
                        project: "demo".into(),
                        label: "Codex 1".into(),
                        agent: Agent::Codex,
                        context_roots: Vec::new(),
                        temp_bundle_path: None,
                    },
                );
            });
            runtime.read(cx).tree.clone().update(cx, |tree, _| {
                tree.set_active_worktrees(
                    0,
                    vec![grove_core::git::Worktree {
                        path: path.into(),
                        branch: "main".into(),
                        mtime: None,
                        is_main: true,
                    }],
                );
            });
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        sidebar.read_with(cx, |sidebar, cx| {
            assert_eq!(
                sidebar.visible_session_targets(cx),
                vec![(SessionId::from_raw(1), "Continue migration · demo".into())]
            );
            assert!(sidebar.terminal_views.is_empty());
        });
        assert!(cx.debug_bounds("session-1").is_some());
        assert!(cx.debug_bounds("terminal-header-1").is_some());
    }

    #[gpui::test]
    fn background_title_change_notifies_sidebar_without_selection(cx: &mut gpui::TestAppContext) {
        let path = "/grove-background-title-test";
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "demo".into(),
                    path: path.into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
            cx.set_global(crate::theme::ThemeState::new(
                false,
                "tokyonight".into(),
                "tokyonight-day".into(),
            ));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            for _ in 0..2 {
                let session = cx.new(|cx| {
                    crate::entities::terminal_session::TerminalSession::attach_existing(
                        "grove-background-title-not-attached",
                        24,
                        80,
                        cx,
                    )
                });
                registry.update(cx, |registry, cx| {
                    let id = registry.insert_meta("demo".into(), path.into(), Agent::Codex);
                    registry.attach(id, session, None);
                    cx.notify();
                });
            }
            runtime.read(cx).tree.clone().update(cx, |tree, _| {
                tree.set_active_worktrees(
                    0,
                    vec![grove_core::git::Worktree {
                        path: path.into(),
                        branch: "main".into(),
                        mtime: None,
                        is_main: true,
                    }],
                );
            });
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        let notifications = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = notifications.clone();
        let _observer =
            cx.update(|_, cx| cx.observe(&sidebar, move |_, _| seen.set(seen.get() + 1)));
        let background = sidebar.read_with(cx, |sidebar, cx| {
            let ids = sidebar
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .all()
                .iter()
                .map(|meta| meta.id)
                .collect::<Vec<_>>();
            assert_eq!(sidebar.selection, Some(Selection::Session(ids[0])));
            assert!(sidebar.title_observers.contains_key(&ids[1]));
            assert!(!sidebar.terminal_views.contains_key(&ids[1]));
            sidebar
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .session(ids[1])
                .unwrap()
                .clone()
        });
        let before = notifications.get();
        cx.update(|_, cx| {
            background.update(cx, |session, cx| {
                session.ingest_for_test(b"\x1b]2;Fix refresh state\x07", cx);
            });
        });
        cx.run_until_parked();
        assert!(notifications.get() > before);
        sidebar.read_with(cx, |sidebar, cx| {
            let second = sidebar.runtime.read(cx).registry.read(cx).all()[1].id;
            assert_eq!(
                sidebar.observed_titles.get(&second),
                Some(&Some("Fix refresh state".into()))
            );
            assert_eq!(
                sidebar.selection,
                Some(Selection::Session(SessionId::from_raw(1)))
            );
            assert!(!sidebar.terminal_views.contains_key(&second));
            assert!(sidebar
                .visible_session_targets(cx)
                .iter()
                .any(|(id, title)| *id == second && title.starts_with("Fix refresh state")));
        });
        let after_title_change = notifications.get();
        cx.update(|_, cx| {
            background.update(cx, |session, cx| {
                session.ingest_for_test(b"\x1b]2;Fix refresh state\x07", cx);
            });
        });
        cx.run_until_parked();
        assert_eq!(notifications.get(), after_title_change);
        cx.update(|_, cx| {
            cx.global_mut::<SettingsState>()
                .store
                .workspaces
                .create("Other")
                .unwrap();
        });
        draw(cx);
        sidebar.read_with(cx, |sidebar, _| {
            assert!(sidebar.title_observers.is_empty());
            assert!(sidebar.observed_titles.is_empty());
        });
    }

    #[gpui::test]
    fn project_menu_opens_on_hover_and_closes_after_pointer_leaves(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "one".into(),
                    path: "/grove-sidebar-hover-test".into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        let trigger = cx.debug_bounds("project-menu-0").unwrap();
        cx.simulate_mouse_move(trigger.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.menu == Some(0)));
        let popup = cx.debug_bounds("project-actions-popup").unwrap();
        cx.simulate_mouse_move(popup.center(), None, gpui::Modifiers::default());
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.menu == Some(0)));
        cx.simulate_mouse_move(
            gpui::point(gpui::px(1.0), gpui::px(1.0)),
            None,
            gpui::Modifiers::default(),
        );
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.menu.is_none()));
    }

    #[gpui::test]
    fn archived_panel_dismisses_on_sidebar_navigation(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "one".into(),
                    path: "/grove-sidebar-archive-test".into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
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
                sidebar.act(Action::ArchivedProjects, window, cx);
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("project-panel").is_some());
        let project = cx.debug_bounds("project-0").unwrap();
        cx.simulate_click(project.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.project_panel.is_none()));
        assert!(cx.debug_bounds("project-panel").is_none());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::ArchivedProjects, window, cx);
                sidebar.act(Action::Mode(ViewMode::List), window, cx);
                assert!(sidebar.project_panel.is_none());
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("project-panel").is_none());
        for action in [
            Action::Select(Selection::Project(0)),
            Action::Select(Selection::Worktree(0, "/grove-sidebar-archive-test".into())),
        ] {
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.act(Action::ArchivedProjects, window, cx);
                    assert!(sidebar.project_panel.is_some());
                    sidebar.act(action.clone(), window, cx);
                    assert!(sidebar.project_panel.is_none());
                });
            });
            draw(cx);
            assert!(cx.debug_bounds("project-panel").is_none());
        }
    }

    #[gpui::test]
    fn project_move_preserves_session_and_source_navigation(cx: &mut gpui::TestAppContext) {
        if std::env::var_os("GROVE_MOVE_SIDEBAR_TEST_CHILD").is_none() {
            let root = std::env::temp_dir().join(format!(
                "grove-move-sidebar-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs_err::create_dir_all(&root).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "views::sidebar::tests::project_move_preserves_session_and_source_navigation",
                    "--nocapture",
                ])
                .env("GROVE_MOVE_SIDEBAR_TEST_CHILD", "1")
                .env("GROVE_CONFIG_DIR", &root)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let repo = ChangedGitRepo::new();
        let path = repo
            .0
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        cx.update(|cx| {
            gpui_component::init(cx);
            let mut store = grove_core::storage::Store::default();
            store.projects.push(grove_core::storage::Project {
                name: "Moving".into(),
                path: path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            });
            store.assign_project_to_active_workspace(&path);
            store.workspaces.create("Destination").unwrap();
            store.workspaces.select(1);
            cx.set_global(SettingsState::new(store));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        draw(cx);
        let id = cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let registry = sidebar.runtime.read(cx).registry.clone();
                let id = registry.update(cx, |registry, cx| {
                    let id = registry.insert_meta("Moving".into(), path.clone(), Agent::Codex);
                    cx.notify();
                    id
                });
                sidebar.sync(window, cx);
                sidebar.act(Action::Select(Selection::Session(id)), window, cx);
                sidebar.act(Action::Menu(0), window, cx);
                sidebar.act(Action::MoveProject(path.clone()), window, cx);
                id
            })
        });
        draw(cx);
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                assert!(sidebar.project_panel.is_none());
                assert_eq!(sidebar.selection, Some(Selection::Session(id)));
                assert!(sidebar.project_menu_focus[&0].is_focused(window));
                assert_eq!(
                    cx.global::<SettingsState>()
                        .store
                        .project_workspace_id(&path),
                    1
                );
                sidebar.act(Action::Menu(0), window, cx);
                sidebar.act(Action::MoveProject(path.clone()), window, cx);
            });
        });
        draw(cx);
        // Pointer chooses the destination; keyboard traverses to the explicit primary action.
        let destination = cx.debug_bounds("project-move-destination-2").unwrap();
        cx.simulate_click(destination.center(), gpui::Modifiers::default());
        draw(cx);
        cx.simulate_keystrokes("tab tab tab enter");
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                assert!(sidebar.project_panel.is_none());
                assert_eq!(sidebar.selection, None);
                assert_eq!(cx.global::<SettingsState>().store.workspaces.active, 1);
                assert_eq!(
                    cx.global::<SettingsState>()
                        .store
                        .project_workspace_id(&path),
                    2
                );
                assert_eq!(
                    sidebar
                        .runtime
                        .read(cx)
                        .toast
                        .read(cx)
                        .current()
                        .unwrap()
                        .message,
                    "Moved Moving to Destination."
                );
                assert!(sidebar
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .meta(id)
                    .is_some());
                assert!(!sidebar
                    .snapshot
                    .projects
                    .iter()
                    .any(|project| project.sessions.contains(&id)));
                cx.global_mut::<SettingsState>().store.workspaces.select(2);
                sidebar.sync(window, cx);
                assert!(sidebar
                    .snapshot
                    .projects
                    .iter()
                    .any(|project| project.sessions.contains(&id)));
            });
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::MoveProject(path.clone()), window, cx);
            });
        });
        draw(cx);
        let destination = cx.debug_bounds("project-move-destination-1").unwrap();
        cx.simulate_click(destination.center(), gpui::Modifiers::default());
        draw(cx);
        cx.simulate_keystrokes("tab space tab tab enter");
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                assert!(sidebar.project_panel.is_none());
                assert_eq!(cx.global::<SettingsState>().store.workspaces.active, 1);
                assert_eq!(
                    cx.global::<SettingsState>()
                        .store
                        .project_workspace_id(&path),
                    1
                );
                assert_eq!(sidebar.selection, Some(Selection::Project(0)));
                assert!(sidebar.project_menu_focus[&0].is_focused(window));
                assert!(sidebar
                    .snapshot
                    .projects
                    .iter()
                    .any(|project| project.sessions.contains(&id)));
                assert!(sidebar
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .meta(id)
                    .is_some());
            });
        });
    }
    #[gpui::test]
    fn project_popup_preserves_rows_and_restores_focus_on_dismiss(cx: &mut gpui::TestAppContext) {
        let repo = ChangedGitRepo::new();
        let one_path = repo
            .0
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        cx.update(|cx| {
            gpui_component::init(cx);
            let projects = ["one", "two"]
                .into_iter()
                .map(|name| grove_core::storage::Project {
                    name: name.into(),
                    path: if name == "one" {
                        one_path.clone()
                    } else {
                        format!("/grove-sidebar-test-{name}")
                    },
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
                sidebar.act(Action::EditProject(one_path.clone()), window, cx);
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
                sidebar.act(Action::EditProject(one_path.clone()), window, cx);
                assert!(
                    sidebar.project_panel.is_none(),
                    "cross-workspace action is stale"
                );
                sidebar.finish_project_selection(&one_path, window, cx);
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
        let selector = |prefix: &str| -> &'static str {
            Box::leak(format!("{prefix}{one_path}").into_boxed_str())
        };
        let project_title = cx.debug_bounds("project-title-0").unwrap();
        let worktree_title = cx.debug_bounds(selector("worktree-title-")).unwrap();
        let empty = cx.debug_bounds(selector("worktree-empty-")).unwrap();
        assert_eq!(
            worktree_title.left(),
            project_title.left() + gpui::px(SPACE_LG)
        );
        assert_eq!(
            empty.left(),
            project_title.left() + gpui::px(HIERARCHY_INSET)
        );
        assert!(cx.debug_bounds("projects-count").is_some());
        assert!(cx.debug_bounds(selector("worktree-count-")).is_none());
        assert!(cx.debug_bounds("tree-expand-cycle").is_none());
        assert!(cx.debug_bounds(selector("fold-")).is_none());
        let worktree_actions = cx.debug_bounds(selector("worktree-actions-")).unwrap();
        assert_eq!(f32::from(worktree_actions.size.width), HIERARCHY_TRAILING_W);
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let registry = sidebar.runtime.read(cx).registry.clone();
                registry.update(cx, |registry, cx| {
                    registry.insert_meta("one".into(), one_path.clone(), Agent::Codex);
                    cx.notify();
                });
                sidebar.act(Action::Select(Selection::Project(0)), window, cx);
            });
        });
        draw(cx);
        assert_eq!(
            cx.debug_bounds("project-title-0").unwrap().left(),
            project_title.left()
        );
        assert!(cx.debug_bounds("session-1").is_some());
        let agent = cx.debug_bounds("session-agent-1").unwrap();
        assert_eq!(
            agent.left(),
            project_title.left() + gpui::px(HIERARCHY_INSET)
        );
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(
                    Action::Select(Selection::Worktree(0, one_path.clone())),
                    window,
                    cx,
                );
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("session-1").is_some());
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
        cx.simulate_keystrokes("down down down down enter");
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
        cx.simulate_keystrokes("down down enter");
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
    #[gpui::test]
    fn grid_close_shortcut_confirms_selected_tile(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            let project = grove_core::storage::Project {
                name: "demo".into(),
                path: "/grove-grid-shortcut-test".into(),
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
                registry.insert_meta(
                    "demo".into(),
                    "/grove-grid-shortcut-test".into(),
                    Agent::Codex,
                );
                registry.insert_meta(
                    "demo".into(),
                    "/grove-grid-shortcut-test".into(),
                    Agent::Codex,
                );
            });
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.mode = ViewMode::Grid;
            sidebar.selection = Some(Selection::Session(SessionId::from_raw(2)));
            sidebar
        });
        let first = SessionId::from_raw(1);
        let selected = SessionId::from_raw(2);
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        let header = cx.debug_bounds("terminal-header-2").unwrap();
        let source = sidebar.read_with(cx, |sidebar, _| {
            sidebar.canvas_close_focus[&(selected, false)].clone()
        });
        cx.update(|window, cx| {
            source.focus(window, cx);
            sidebar.update(cx, |sidebar, cx| sidebar.request_close_focused(window, cx));
        });
        draw(cx);
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        let popup = cx.debug_bounds("canvas-confirmation").unwrap();
        assert!((f32::from(popup.left() - header.left())).abs() <= 1.0);
        assert!((f32::from(popup.top() - header.bottom())).abs() <= 1.0);
        assert!(sidebar.read_with(cx, |sidebar, _| {
            sidebar.pending_close == Some(selected) && sidebar.canvas_close_anchor == Some(selected)
        }));
        cx.update(|window, cx| assert!(sidebar.read(cx).cancel_focus.is_focused(window)));
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, _| assert!(source.is_focused(window)));
        assert!(sidebar.read_with(cx, |sidebar, _| !sidebar.confirmation_open()));
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.toggle_zen(window, cx));
        });
        draw(cx);
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.is_zen()));
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.request_close_focused(window, cx));
        });
        draw(cx);
        let zen_popup = cx.debug_bounds("canvas-confirmation").unwrap();
        assert!(zen_popup.left() >= gpui::px(0.0));
        assert!(zen_popup.top() >= gpui::px(0.0));
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.toggle_zen(window, cx));
        });
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.request_close_focused(window, cx));
        });
        draw(cx);
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::ConfirmClose(selected), window, cx);
            });
        });
        assert!(sidebar.read_with(cx, |sidebar, cx| {
            let registry = sidebar.runtime.read(cx).registry.read(cx);
            registry.meta(first).is_some() && registry.meta(selected).is_none()
        }));
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
