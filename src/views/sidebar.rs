//! Workspace-scoped navigation. UI state is kept per workspace while processes keep running.
mod action_feedback;
mod activity_reply;
mod agent_preview;
mod branch_history;
mod branch_preview;
mod change_description;
mod change_review;
mod change_tools;
mod checkout_panel;
mod content;
mod context_panel;
mod contributions;
mod files;
mod global_navigation;
mod grid;
mod multi_project;
mod plan_preview;
mod rail_layout;

mod session_navigation;

use multi_project::SidebarContext;
pub(crate) mod dashboard;
mod dashboard_timeline;
mod project_flyout;
mod project_setup;
pub(super) mod projects;
mod script_preview;
mod session_diff;
mod session_history;
mod session_motion;
mod session_placement;
mod session_scope;
mod side_chat;
mod task_glance;
mod title_edit;
mod usage;
mod worktree_form;
mod worktree_terminal;
use super::{motion, rpx, terminal_view::TerminalView, tokens::*};
use crate::{
    activity::ActivityState,
    entities::{
        branch_history::BranchHistoryState,
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
use gpui_component::{input::InputState, ElementExt};
use grove_core::agent::Agent;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const SIDEBAR_W: f32 = 260.0;
const SIDEBAR_COLLAPSED_W: f32 = 0.0;
const SIDEBAR_MAX_VIEWPORT_FRACTION: f32 = 0.4;
const SIDEBAR_NARROW_BREAKPOINT: f32 = SIDEBAR_W / SIDEBAR_MAX_VIEWPORT_FRACTION;
const SIDEBAR_DRAG_EPSILON: f32 = 1.0;
const WORKTREE_LAUNCH_AGENTS: [Agent; 4] = [
    Agent::Codex,
    Agent::Claude,
    Agent::OpenCode,
    Agent::Terminal,
];

const HEAD_H: f32 = 36.0;
const ROW_H: f32 = 28.0;
// Expanded project navigation keeps session activity, identity and changes on one line.
const HIERARCHY_INSET: f32 = SIDEBAR_CONTENT_INSET - SPACE_LG;
#[cfg(test)]
const HIERARCHY_GAP: f32 = 7.0;
#[cfg(test)]
const HIERARCHY_ICON_SLOT: f32 = 18.0;
#[cfg(test)]
const HIERARCHY_ICON: f32 = 15.0;
#[cfg(test)]
const HIERARCHY_LABEL_INSET: f32 = HIERARCHY_INSET + HIERARCHY_ICON_SLOT + HIERARCHY_GAP;
// Three distinct leading edges: project, worktree and session.
const WORKTREE_LEADING_INSET: f32 = HIERARCHY_INSET + SPACE_2XL;
const SESSION_LEADING_INSET: f32 = WORKTREE_LEADING_INSET + SPACE_3XL;
#[cfg(test)]
const WORKTREE_GROUP_GAP: f32 = 18.0;
const HIERARCHY_META_TEXT: f32 = TEXT_MICRO;
#[cfg(test)]
const HIERARCHY_TRAILING_W: f32 = 24.0;
#[cfg(test)]
const PROJECT_GROUP_GAP: f32 = 14.0;
#[cfg(test)]
const PROJECT_ROW_H: f32 = 35.0;
#[cfg(test)]
const PROJECT_TEXT: f32 = TEXT_TITLE;
#[cfg(test)]
const PROJECT_TITLE_LINE_H: f32 = 21.0;
#[cfg(test)]
const WORKTREE_ROW_H: f32 = ROW_H;
#[cfg(test)]
const WORKTREE_TITLE_LINE_H: f32 = 17.0;
#[cfg(test)]
const WORKTREE_ACTION_W: f32 = CHROME_CONTROL_H;
const SESSION_ROW_H: f32 = 43.0;
const SESSION_TREE_ROW_H: f32 = 32.0;
const SESSION_LAUNCHER_SIZE: f32 = ICON_SM;
const SESSION_TITLE_LINE_H: f32 = 17.0;
const SESSION_META_LINE_H: f32 = 14.0;
/// Single-line reference to a multi-checkout session under a secondary worktree group.
const SESSION_REFERENCE_ROW_H: f32 = 32.0;
const SESSION_ROW_RADIUS: f32 = 7.0;
const SESSION_LIST_CARD_H: f32 = 78.0;
const SESSION_LIST_HEADER_H: f32 = 20.0;
const SESSION_LIST_TITLE_H: f32 = 20.0;
const SESSION_LIST_FOOTER_H: f32 = 16.0;
#[cfg(test)]
const SESSION_FIRST_GAP: f32 = SPACE_XS;
#[cfg(test)]
const SESSION_SPINNER_PERIOD: Duration = Duration::from_millis(1200);
const SESSION_AGE_REFRESH: Duration = Duration::from_secs(1);
const WORKTREE_READINESS_REFRESH: Duration = Duration::from_secs(5);

/// Visible activity owns its repeating animation; unmounted and quiet states have no timer.
fn session_status_icon(id: SessionId, status: &str, cx: &App) -> AnyElement {
    session_status_icon_sized(id, status, ICON_MD, cx)
}

fn session_status_icon_sized(id: SessionId, status: &str, size: f32, cx: &App) -> AnyElement {
    status_icon_sized(id.raw().to_string().into(), status, size, cx)
}

/// Shared status marks keep session and child-agent semantics and motion identical.
fn status_icon_sized(id: SharedString, status: &str, size: f32, cx: &App) -> AnyElement {
    super::status::mark(id, super::status::session_kind(status), size, cx)
}

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
    let _ = now; // Activity clocks order rows; they never establish running work.
    let mut groups: [Vec<usize>; 4] = std::array::from_fn(|_| Vec::new());
    for (index, entry) in entries.iter().enumerate() {
        let section = session_placement::section(entry.status);
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
    ["Needs you", "Review", "Working", "Ready"]
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

fn managed_session_status(
    failed: bool,
    pending_attach: bool,
    activity: ActivityState,
) -> (&'static str, gpui::Hsla) {
    if failed {
        return ("Failed", c::RED());
    }
    if pending_attach {
        return ("Starting", c::BLUE());
    }
    match activity {
        ActivityState::WaitingForInput => ("Needs you", c::AMBER()),
        ActivityState::Working => ("Working", c::BLUE()),
        ActivityState::Done => ("Done", c::GREEN()),
        ActivityState::Idle => ("Ready", c::FG_DIM()),
        ActivityState::Exited => ("Exited", c::FG_DIM()),
    }
}

/// Pending interactions include requests forwarded from child agents. Their specific
/// action outranks a coarse runtime state; terminal sessions retain the generic fallback.
fn structured_session_status(
    state: &grove_core::codex::Snapshot,
    activity: ActivityState,
) -> &'static str {
    match state.session_status() {
        "Review" if activity == ActivityState::Done => "Review · unread",
        label => label,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SidebarEvent {
    NewSession,
    Settings,
    #[cfg(test)]
    NewMultiProjectSession,
}
#[derive(Default)]
struct Navigation {
    selection: Option<Selection>,
    scroll: ScrollHandle,
}
#[derive(Clone)]
enum Action {
    Destination(global_navigation::Destination),
    Select(Selection),
    #[cfg(test)]
    ToggleProject(String),
    Mode(ViewMode),
    #[cfg(test)]
    Menu(usize),
    NewWorktree(String),
    InitializeGit(String),
    RefreshGit(String),
    #[cfg(test)]
    EditProject(String),
    EditProjectScripts(String, bool),
    #[cfg(test)]
    MoveProject(String),
    AddProject,
    #[cfg(test)]
    ArchivedProjects,
    Reveal(String),
    #[cfg(test)]
    RemoveProject(String),
    #[cfg(test)]
    ConfirmRemove(usize),
    RemoveWorktree(usize, String),
    ConfirmWorktreeRemoval,
    SkipWorktreeTeardown,
    DismissWorktreeRemoval,
    #[cfg(test)]
    Launch(usize, String, Agent),
    RunScript(String, String),
    RunNamedScript(String, String, String),
    Close(SessionId),
    FinishSession(SessionId),
    MarkActivityDone(SessionId),
    MarkTerminalReviewed(SessionId, Instant),
    MarkReviewed(SessionId, usize),
    ConfirmClose(SessionId),
    Retry(SessionId),
    OpenDiff(SessionId),
    OpenWorktreeDiff(String),
    OpenHistory(String),
    OpenBranchHistory(String, Option<Selection>, Option<[u8; 32]>),
    RefreshBranchPreview,
    CloseHistory,
    RefreshHistory,
    MoreHistory,
    SelectCommit(String),
    ExpandCommit,
    HistoryGraph,
    HistoryDiffMode(crate::entities::branch_history::HistoryDiffMode),
    ToggleHistoryMetadata,
    CopyHistoryCommit,
    SelectHistoryFile(String),
    CloseDiff,
    SelectDiffFile(String),
    RefreshDiff,
    AddTerminal,
    OpenSettings,
    DashboardCheckout {
        project: usize,
        root: String,
        path: String,
    },
    ToggleUsage,
    UpdateApp,
    #[cfg(test)]
    NewMultiProjectSession,
    ToggleSidebar,
    ToggleGrid,
    GroupSessions(global_navigation::SessionGrouping),
    RefreshSessionHistory,
    HistoryFilterMenu(bool),
    ClearHistoryFilters,
    ClearHistorySearch,
    PreviewSessionHistory(String),
    PreviewCliHistory(String),
    ReopenSessionHistory(String),
    OpenWorkspaces,
    CloseHome(SessionId),
    ConfirmHome(SessionId),
    Cancel,
}
impl Action {
    fn is_fixture_launch(&self) -> bool {
        #[cfg(test)]
        {
            matches!(self, Self::Launch(..))
        }
        #[cfg(not(test))]
        {
            false
        }
    }
    fn is_fixture_remove(&self) -> bool {
        #[cfg(test)]
        {
            matches!(self, Self::ConfirmRemove(_))
        }
        #[cfg(not(test))]
        {
            false
        }
    }
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

type SessionGroups = Vec<(&'static str, Vec<SessionId>)>;

pub struct Sidebar {
    worktree_branch_edited: bool,
    worktree_source_generation: u64,
    worktree_source: String,
    checkout_panel: checkout_panel::CheckoutPanel,
    contributions: contributions::Contributions,
    context_panel: context_panel::ContextPanel,
    descriptions: change_description::Descriptions,
    reviews: change_review::Reviews,
    files_active: bool,
    files_owner: Option<Selection>,
    files_toggle_focus: Option<FocusHandle>,
    standalone_files: Option<(
        Option<Selection>,
        String,
        Entity<super::file_browser::FileBrowser>,
    )>,
    action_source_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    project_panel: Option<Entity<projects::ProjectPanel>>,
    project_setup: Option<Entity<project_setup::ProjectSetup>>,
    settings_panel: Option<Entity<super::settings_panel::SettingsPanel>>,
    project_decision: bool,
    project_return_focus: Option<FocusHandle>,
    project_return_path: Option<String>,
    project_return_navigation: Option<(ViewMode, Option<Selection>)>,
    workspace_selector: Option<Entity<super::workspace_manager::WorkspaceManager>>,
    runtime: Entity<Runtime>,
    focus: FocusHandle,
    shell_focus: Option<FocusHandle>,
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
    rendered_terminal_ids: HashSet<SessionId>,
    cached_terminal_ids: HashSet<SessionId>,
    project_flyout: Option<SidebarContext>,
    project_flyout_hover_open: bool,
    project_flyout_hover_blocked: bool,
    project_flyout_suppressed: Option<SidebarContext>,
    project_flyout_hide_task: Option<gpui::Task<()>>,
    project_group_bounds: HashMap<usize, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    project_flyout_popup_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    project_flyout_focus: FocusHandle,
    project_flyout_launch_path: Option<String>,
    project_flyout_launch_error: Option<String>,
    project_flyout_plus_focus: HashMap<String, FocusHandle>,
    project_flyout_item_focus: HashMap<String, FocusHandle>,
    rail_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    multi_project_focus: FocusHandle,
    multi_project_group_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    menu: Option<usize>,
    menu_opened_by_hover: bool,
    #[cfg(test)]
    menu_focus: FocusHandle,
    #[cfg(test)]
    menu_index: usize,
    project_menu_focus: HashMap<usize, FocusHandle>,
    project_toggle_focus: HashMap<String, FocusHandle>,
    menu_return_focus: Option<FocusHandle>,
    project_menu_bounds: HashMap<usize, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    session_close_focus: HashMap<SessionId, FocusHandle>,
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
    pub(super) worktree_creating: bool,
    worktree_name: Entity<InputState>,
    worktree_errors: [Option<String>; 3],
    worktree_return_focus: Option<FocusHandle>,
    worktree_branch: Entity<InputState>,
    worktree_base: Entity<InputState>,
    content_error: Option<String>,
    git_init_pending: Option<String>,
    worktree_readiness: HashMap<String, (Instant, Option<WorktreeReadiness>)>,
    branch_peek: Option<branch_preview::BranchPeek>,
    branch_peek_cache: Option<branch_preview::BranchPeekCache>,
    branch_peek_task: Option<gpui::Task<()>>,
    branch_entry: Option<(String, Option<[u8; 32]>)>,
    branch_entry_focus: FocusHandle,
    branch_entry_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    history_hover: branch_history::HistoryHover,
    history: Option<Entity<BranchHistoryState>>,
    history_observer: Option<gpui::Subscription>,
    history_focus: FocusHandle,
    history_file_resize_focus: FocusHandle,
    history_return_focus: Option<FocusHandle>,
    history_selection: Option<Selection>,
    diff_viewer: Option<Entity<DiffViewerState>>,
    diff_focus: FocusHandle,
    diff_return_focus: Option<FocusHandle>,
    diff_observer: Option<gpui::Subscription>,
    terminal_views: HashMap<SessionId, Entity<TerminalView>>,
    worktree_terminal: worktree_terminal::WorktreeTerminal,
    dashboard: Option<dashboard::Dashboard>,
    dashboard_open: bool,
    destination: global_navigation::Destination,
    history_canvas_destination: global_navigation::Destination,
    navigation_transition: Option<(Instant, f32, f32)>,
    navigation_width_override: Option<f32>,
    navigation_focus: [FocusHandle; 2],
    session_grid_focus: FocusHandle,
    grid_blocked_by_shell: bool,
    settings_button_focus: FocusHandle,
    usage_button_focus: FocusHandle,
    usage_panel: Option<Entity<super::usage_panel::UsagePanel>>,
    usage_subscription: Option<gpui::Subscription>,
    usage_hover_open: bool,
    usage_hover_suppressed: bool,
    usage_open_task: Option<gpui::Task<()>>,
    usage_hide_task: Option<gpui::Task<()>>,
    usage_anchor: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    session_history: Vec<grove_core::codex::SavedSession>,
    cli_session_history: Vec<grove_core::session_meta::ClosedCliSession>,
    session_history_loading: bool,
    session_history_error: Option<String>,
    session_history_selected: Option<session_history::HistorySelection>,
    session_history_view: Option<Entity<super::codex_view::CodexView>>,
    history_search: Entity<InputState>,
    history_search_query: String,
    history_search_results: HashMap<String, String>,
    history_search_task: Option<gpui::Task<()>>,
    history_load_generation: u64,
    history_project_filter: Option<String>,
    history_provider_filter: Option<Agent>,
    history_filter_popup: Option<Entity<gpui_component::menu::PopupMenu>>,
    history_filter_subscription: Option<gpui::Subscription>,
    history_filter_position: gpui::Point<gpui::Pixels>,
    codex_views: HashMap<SessionId, Entity<super::codex_view::CodexView>>,
    home_terminal_views: HashMap<SessionId, Entity<TerminalView>>,
    available: [bool; 4],
    project_paths: HashMap<usize, String>,
    sync_dirty: bool,
    registry_sync_revision: u64,
    pub(super) window_active: bool,
    list_groups_cache: std::cell::RefCell<Option<SessionGroups>>,
    git_paths: Vec<String>,
    git_refresh_timer: Option<gpui::Task<()>>,
    collapsed_projects: HashSet<String>,
    worktree_focus: HashMap<String, FocusHandle>,
    worktree_diff_focus: HashMap<String, FocusHandle>,
    #[cfg(test)]
    hovered_worktree: Option<String>,
    session_diff_focus: HashMap<SessionId, FocusHandle>,
    session_diff_observers: HashMap<SessionId, (gpui::Subscription, gpui::Subscription)>,
    cache_warm: Option<(u64, u64)>,
    drag: Option<SidebarDrag>,
    grid_layouts: HashMap<u64, grid::WorkspaceGrid>,
    grid_drag: Option<grid::GridDrag>,
    grid_session_drag: Option<grid::GridSessionDrag>,
    grid_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    title_edit: Option<title_edit::TitleEdit>,
    title_edit_focus: HashMap<SessionId, FocusHandle>,
    canvas_close_anchor: Option<SessionId>,
    canvas_close_focus: HashMap<(SessionId, bool), FocusHandle>,
    canvas_breadcrumb_focus: HashMap<(SessionId, bool), FocusHandle>,
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
    task_glance_observers: HashMap<
        SessionId,
        (
            Entity<crate::entities::codex_session::CodexSession>,
            gpui::Subscription,
        ),
    >,
    task_glances: HashMap<SessionId, task_glance::TaskGlanceCache>,
    header_tasks: task_glance::HeaderTasks,
    dashboard_timelines: dashboard_timeline::DashboardTimelines,
    session_motion: session_motion::SessionMotion,
    session_placement: session_placement::SessionPlacement,
    placement_timer: Option<gpui::Task<()>>,
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
    pub(crate) fn credentials_changed(
        &mut self,
        provider: grove_core::provider::Provider,
        cx: &mut Context<Self>,
    ) {
        if self
            .dashboard
            .as_ref()
            .is_some_and(|draft| draft.session.read(cx).state.saved.provider == provider)
        {
            self.refresh_dashboard_catalog(cx);
        }
        cx.notify();
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
        let (registry, activity, tree, projects, shell_environment, upgrade) = (
            rt.registry.clone(),
            rt.activity.clone(),
            rt.tree.clone(),
            rt.projects.clone(),
            rt.shell_environment.clone(),
            rt.upgrade.clone(),
        );
        let worktree_name = cx.new(|cx| InputState::new(window, cx).placeholder("billing-retry"));
        let worktree_branch =
            cx.new(|cx| InputState::new(window, cx).placeholder("feat/billing-retry"));
        let worktree_base = cx.new(|cx| InputState::new(window, cx).placeholder("main"));
        let history_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search sessions and transcripts"));
        let mut observers = vec![
            cx.observe(&runtime, |this, _, cx| {
                this.sync_dirty = true;
                this.list_groups_cache.borrow_mut().take();
                cx.notify();
            }),
            cx.observe(&registry, |this, _, cx| {
                this.sync_dirty = true;
                this.list_groups_cache.borrow_mut().take();
                cx.notify();
            }),
            cx.observe(&activity, |this, _, cx| {
                this.list_groups_cache.borrow_mut().take();
                cx.notify();
            }),
            cx.observe(&upgrade, |_, _, cx| cx.notify()),
            cx.observe(&shell_environment, |this, _, cx| {
                this.available = WORKTREE_LAUNCH_AGENTS.map(Agent::available);
                cx.notify();
            }),
            cx.observe(&tree, |this, _, cx| {
                this.sync_dirty = true;
                this.list_groups_cache.borrow_mut().take();
                cx.notify();
            }),
            cx.subscribe(&projects, |this, _, event, cx| {
                if let ProjectEvent::WorktreeRemovalChanged { path } = event {
                    this.worktree_removal_changed(path, cx);
                }
            }),
            cx.observe_global::<SettingsState>(|this, cx| {
                this.refresh_dashboard_projects(cx);
                this.sync_dirty = true;
                cx.notify();
            }),
            cx.subscribe_in(&history_search, window, |this, _, event, _, cx| {
                if matches!(event, gpui_component::input::InputEvent::Change) {
                    this.schedule_history_search(cx);
                    cx.notify();
                }
            }),
        ];
        for (index, input) in [&worktree_name, &worktree_branch, &worktree_base]
            .into_iter()
            .enumerate()
        {
            observers.push(
                cx.subscribe_in(input, window, move |this, _, event, window, cx| {
                    if matches!(event, gpui_component::input::InputEvent::PressEnter { .. }) {
                        this.submit_worktree(cx);
                    } else if matches!(event, gpui_component::input::InputEvent::Change) {
                        this.worktree_errors[index] = None;
                        this.worktree_field_changed(index, window, cx);
                        cx.notify();
                    }
                }),
            );
        }
        Self {
            worktree_branch_edited: false,
            worktree_source_generation: 0,
            worktree_source: String::new(),
            checkout_panel: checkout_panel::CheckoutPanel::default(),
            contributions: contributions::Contributions::default(),
            context_panel: context_panel::ContextPanel::default(),
            descriptions: change_description::Descriptions::default(),
            reviews: change_review::Reviews::default(),
            files_active: false,
            files_owner: None,
            files_toggle_focus: None,
            standalone_files: None,
            action_source_bounds: std::rc::Rc::default(),
            project_panel: None,
            project_setup: None,
            settings_panel: None,
            project_decision: false,
            project_return_focus: None,
            project_return_path: None,
            project_return_navigation: None,
            runtime,
            workspace_selector: None,
            focus: cx.focus_handle(),
            shell_focus: None,
            compact_workspace_focus: cx.focus_handle().tab_stop(true),
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
            rendered_terminal_ids: HashSet::new(),
            cached_terminal_ids: HashSet::new(),
            project_flyout: None,
            project_flyout_hover_open: false,
            project_flyout_hover_blocked: false,
            project_flyout_suppressed: None,
            project_flyout_hide_task: None,
            project_group_bounds: HashMap::new(),
            project_flyout_popup_bounds: std::rc::Rc::default(),
            project_flyout_focus: cx.focus_handle(),
            project_flyout_launch_path: None,
            project_flyout_launch_error: None,
            project_flyout_plus_focus: HashMap::new(),
            project_flyout_item_focus: HashMap::new(),
            rail_bounds: std::rc::Rc::default(),
            multi_project_focus: cx.focus_handle(),
            multi_project_group_bounds: std::rc::Rc::default(),
            menu: None,
            menu_opened_by_hover: false,
            #[cfg(test)]
            menu_focus: cx.focus_handle(),
            #[cfg(test)]
            menu_index: 0,
            project_menu_focus: HashMap::new(),
            project_toggle_focus: HashMap::new(),
            menu_return_focus: None,
            menu_trigger_bounds: std::rc::Rc::default(),
            menu_popup_bounds: std::rc::Rc::default(),
            project_menu_bounds: HashMap::new(),
            session_close_focus: HashMap::new(),
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
            worktree_creating: false,
            worktree_name,
            worktree_errors: [None, None, None],
            worktree_return_focus: None,
            worktree_branch,
            worktree_base,
            content_error: None,
            git_init_pending: None,
            worktree_readiness: HashMap::new(),
            branch_peek: None,
            branch_peek_cache: None,
            branch_peek_task: None,
            branch_entry: None,
            branch_entry_focus: cx.focus_handle().tab_stop(true),
            branch_entry_bounds: std::rc::Rc::default(),
            history_hover: branch_history::HistoryHover::default(),
            history: None,
            history_observer: None,
            history_focus: cx.focus_handle(),
            history_file_resize_focus: cx.focus_handle(),
            history_return_focus: None,
            history_selection: None,
            diff_viewer: None,
            diff_focus: cx.focus_handle(),
            diff_return_focus: None,
            diff_observer: None,
            terminal_views: HashMap::new(),
            worktree_terminal: worktree_terminal::WorktreeTerminal::default(),
            dashboard: None,
            dashboard_open: false,
            destination: global_navigation::Destination::Home,
            history_canvas_destination: global_navigation::Destination::Home,
            navigation_transition: None,
            navigation_width_override: None,
            navigation_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            session_grid_focus: cx.focus_handle().tab_stop(true),
            grid_blocked_by_shell: false,
            settings_button_focus: cx.focus_handle().tab_stop(true),
            usage_button_focus: cx.focus_handle().tab_stop(true),
            usage_panel: None,
            usage_subscription: None,
            usage_hover_open: false,
            usage_hover_suppressed: false,
            usage_open_task: None,
            usage_hide_task: None,
            usage_anchor: std::rc::Rc::new(std::cell::Cell::new(gpui::Bounds::default())),
            session_history: Vec::new(),
            cli_session_history: Vec::new(),
            session_history_loading: false,
            session_history_error: None,
            session_history_selected: None,
            session_history_view: None,
            history_search,
            history_search_query: String::new(),
            history_search_results: HashMap::new(),
            history_search_task: None,
            history_load_generation: 0,
            history_project_filter: None,
            history_provider_filter: None,
            history_filter_popup: None,
            history_filter_subscription: None,
            history_filter_position: gpui::Point::default(),
            codex_views: HashMap::new(),
            home_terminal_views: HashMap::new(),
            available: WORKTREE_LAUNCH_AGENTS.map(Agent::available),
            project_paths: HashMap::new(),
            sync_dirty: true,
            registry_sync_revision: 0,
            window_active: true,
            list_groups_cache: std::cell::RefCell::default(),
            git_paths: Vec::new(),
            git_refresh_timer: None,
            collapsed_projects: HashSet::new(),
            worktree_focus: HashMap::new(),
            worktree_diff_focus: HashMap::new(),
            #[cfg(test)]
            hovered_worktree: None,
            session_diff_focus: HashMap::new(),
            session_diff_observers: HashMap::new(),
            cache_warm: None,
            drag: None,
            grid_layouts: HashMap::new(),
            grid_drag: None,
            grid_session_drag: None,
            grid_bounds: std::rc::Rc::default(),
            title_edit: None,
            title_edit_focus: HashMap::new(),
            canvas_close_anchor: None,
            canvas_close_focus: HashMap::new(),
            canvas_breadcrumb_focus: HashMap::new(),
            canvas_bounds: HashMap::new(),
            canvas_observers: HashMap::new(),
            canvas_focus_observers: HashMap::new(),
            canvas_signatures: HashMap::new(),
            title_observers: HashMap::new(),
            observed_titles: HashMap::new(),
            task_glance_observers: HashMap::new(),
            task_glances: HashMap::new(),
            header_tasks: task_glance::HeaderTasks::default(),
            dashboard_timelines: dashboard_timeline::DashboardTimelines::default(),
            session_motion: session_motion::SessionMotion::default(),
            session_placement: session_placement::SessionPlacement::default(),
            placement_timer: None,
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
        self.sync_dirty = false;
        self.registry_sync_revision = self.runtime.read(cx).registry.read(cx).revision();
        self.list_groups_cache.borrow_mut().take();
        if self.git_refresh_timer.is_none() {
            self.git_refresh_timer = Some(cx.spawn_in(window, async move |this, cx| loop {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                if this
                    .update_in(cx, |this, window, cx| {
                        if this.git_paths.is_empty() {
                            return;
                        }
                        let paths = this.git_paths.clone();
                        this.runtime.read(cx).tree.clone().update(cx, |tree, cx| {
                            tree.maybe_poll_git_state(paths, window.is_window_active(), cx);
                        });
                    })
                    .is_err()
                {
                    break;
                }
            }));
        }
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
        let new_indices: HashMap<_, _> = paths.iter().map(|(index, path)| (path, *index)).collect();
        let mut mapping: HashMap<usize, usize> = self
            .project_paths
            .iter()
            .filter_map(|(old, path)| new_indices.get(path).map(|new| (*old, *new)))
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
            self.project_return_navigation = None;
            self.focus.focus(window, cx);
        }
        if active != self.active_workspace {
            self.session_history_selected = None;
            self.session_history_view = None;
            self.history_project_filter = None;
            self.history_provider_filter = None;
            self.history_search
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.history_filter_popup = None;
            self.history_filter_subscription = None;
            self.session_history.clear();
            self.dashboard_open = false;
            self.branch_peek = None;
            self.branch_peek_task = None;
            self.branch_entry = None;
            self.history_hover = branch_history::HistoryHover::default();
            self.history = None;
            self.history_observer = None;
            self.history_selection = None;
            self.history_return_focus = None;
            self.zen_return = None;
            self.saved.insert(
                self.active_workspace,
                Navigation {
                    selection: self.selection.take(),
                    scroll: self.scroll.clone(),
                },
            );
            let next = self.saved.remove(&active).unwrap_or_default();
            self.selection = next.selection;
            self.scroll = next.scroll;
            self.active_workspace = active;
            self.menu = None;
            self.pending_close = None;
            self.pending_home_close = None;
            self.canvas_close_anchor = None;
            self.dismiss_header_tasks_for_navigation(window, cx);
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
        let ids: Vec<_> = cx
            .global::<SettingsState>()
            .store
            .workspace_projects(active)
            .map(|(i, _)| i)
            .collect();
        let generation = tree.read(cx).generation();
        let active_project = rt.state.read(cx).proj_idx();
        let warm_active = self.cache_warm.is_none()
            && !tree
                .read(cx)
                .worktrees_for_project(active_project, active_project)
                .is_empty();
        if self.cache_warm != Some((active, generation)) {
            let targets = cx
                .global::<SettingsState>()
                .store
                .workspace_projects(active)
                .filter(|(index, _)| !warm_active || *index != active_project)
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
        self.git_paths = {
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
            tree.maybe_poll_git_state(self.git_paths.clone(), window.is_window_active(), cx);
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
            if !self.session_close_focus.contains_key(id) {
                let handle = cx.focus_handle().tab_stop(true);
                self.observers
                    .push(cx.on_focus_in(&handle, window, |_, _, cx| cx.notify()));
                self.observers
                    .push(cx.on_focus_out(&handle, window, |_, _, _, cx| cx.notify()));
                self.session_close_focus.insert(*id, handle);
            }
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
            if !self.worktree_diff_focus.contains_key(&path) {
                let handle = cx.focus_handle().tab_stop(true);
                self.observers
                    .push(cx.on_focus_in(&handle, window, |_, _, cx| cx.notify()));
                self.observers
                    .push(cx.on_focus_out(&handle, window, |_, _, _, cx| cx.notify()));
                self.worktree_diff_focus.insert(path.clone(), handle);
            }
            if !self.worktree_focus.contains_key(&path) {
                let handle = cx.focus_handle().tab_stop(true);
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
        self.title_edit_focus.retain(|id, _| live.contains(id));
        if self
            .title_edit
            .as_ref()
            .is_some_and(|edit| !live.contains(&edit.session))
        {
            self.title_edit = None;
        }
        self.canvas_close_focus
            .retain(|(id, _), _| live.contains(id));
        self.canvas_breadcrumb_focus
            .retain(|(id, _), _| live.contains(id));
        self.canvas_observers.retain(|id, _| live.contains(id));
        self.canvas_focus_observers
            .retain(|id, _| live.contains(id));
        self.canvas_signatures.retain(|id, _| live.contains(id));
        self.codex_views
            .retain(|id, _| registry.meta(*id).is_some());
        self.terminal_views
            .retain(|id, _| registry.meta(*id).is_some());
        self.home_terminal_views
            .retain(|id, _| registry.home_terminals().iter().any(|m| m.id == *id));
        if self
            .selection
            .as_ref()
            .is_some_and(|selection| match selection {
                Selection::Session(id) => {
                    !registry
                        .meta(*id)
                        .is_some_and(|meta| meta.project.is_empty())
                        && !self
                            .snapshot
                            .projects
                            .iter()
                            .any(|p| p.sessions.contains(id))
                }
                Selection::Home(id) => !registry.home_terminals().iter().any(|m| m.id == *id),
                Selection::Project(idx) | Selection::Worktree(idx, _) => !ids.contains(idx),
            })
        {
            self.selection = None;
        }
        let show_dashboard_on_startup = self.initial_selection_pending;
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
                let (mode, last_mode) = (self.mode, self.last_mode);
                self.select(selection, cx);
                self.mode = mode;
                self.last_mode = last_mode;
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
                let (mode, last_mode) = (self.mode, self.last_mode);
                self.select(selection, cx);
                self.mode = mode;
                self.last_mode = last_mode;
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
        if show_dashboard_on_startup || switched {
            self.initial_selection_pending = false;
            self.pending_canvas_focus = None;
            self.pending_grid_workspace_focus = None;
            self.open_landing_page(window, cx);
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
        self.sync_task_glances(cx);
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
        for (index, meta) in registry.home_terminals().iter().enumerate() {
            if self.terminal_owners.get(&meta.id).copied().unwrap_or(1) == self.active_workspace {
                if let Some(session) = registry.home_terminal(index) {
                    observed.push((meta.id, session.clone()));
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
        (self.mode == ViewMode::List || self.dashboard_open)
            && self
                .snapshot
                .projects
                .iter()
                .any(|project| !project.sessions.is_empty())
    }

    fn age_refresh_delay(&self, cx: &App) -> Duration {
        if !self.window_active {
            return Duration::from_mins(1);
        }
        let runtime = self.runtime.read(cx);
        let registry = runtime.registry.read(cx);
        let activity = runtime.activity.read(cx);
        let now = Instant::now();
        registry
            .all()
            .iter()
            .map(|meta| {
                let since = activity.since_of(meta.id).unwrap_or(meta.spawned_at);
                let age = now.saturating_duration_since(since);
                if age < Duration::from_mins(1) {
                    SESSION_AGE_REFRESH
                } else {
                    Duration::from_secs(60 - age.as_secs() % 60)
                }
            })
            .min()
            .unwrap_or(Duration::from_mins(1))
    }

    fn sync_age_timer(&mut self, cx: &mut Context<Self>) {
        if !self.age_visible() {
            self.age_timer = None;
        } else if self.age_timer.is_none() {
            self.age_timer = Some(
                cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| loop {
                    let delay = this
                        .update(cx, |this, cx| this.age_refresh_delay(cx))
                        .unwrap_or(Duration::from_mins(1));
                    cx.background_executor().timer(delay).await;
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
            .filter(|meta| meta.project.is_empty() || projects.contains(meta.project.as_str()))
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
        if self.dashboard_open {
            self.open_destination(self.destination, window, cx);
            return;
        }
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
        let next = self
            .active_canvas_sessions(cx)
            .into_iter()
            .find(|(_, home)| {
                self.destination != global_navigation::Destination::Terminals
                    || self.mode == ViewMode::Grid
                    || self.is_zen()
                    || *home
            });
        if let Some((id, home)) = next {
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
    pub(super) fn action_source_bounds(&self) -> gpui::Bounds<gpui::Pixels> {
        self.action_source_bounds.get()
    }
    pub(super) fn edit_confirmation_open(&self, cx: &App) -> bool {
        self.codex_views
            .values()
            .any(|view| view.read(cx).edit_confirmation_open())
    }
    pub fn confirmation_open(&self) -> bool {
        self.project_decision
            || self.pending_close.is_some()
            || self.pending_home_close.is_some()
            || self.pending_remove.is_some()
            || self.pending_worktree_removal.is_some()
            || self.checkout_confirmation_open()
    }
    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.menu_opened_by_hover = false;
        if let Some(focus) = self.menu_return_focus.take() {
            cx.defer_in(window, move |_, window, cx| focus.focus(window, cx));
        }
        cx.notify();
    }
    #[cfg(test)]
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
        self.project_return_navigation = None;
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn menu_state(&self, window: &Window, cx: &App) -> crate::app_menus::MenuState {
        crate::app_menus::MenuState {
            window: true,
            fullscreen: window.is_fullscreen(),
            navigation: self.navigation_available() && !self.settings_navigation_blocked(cx),
            worktree: self.selected_worktree().is_some(),
            close_session: matches!(
                self.selection,
                Some(Selection::Session(_) | Selection::Home(_))
            ),
            sessions: !self.visible_session_order(cx).is_empty(),
            waiting: self.waiting_session(cx).is_some(),
            terminal_edit: self
                .terminal_views
                .values()
                .chain(self.home_terminal_views.values())
                .chain(self.worktree_terminal.views.values())
                .find_map(|view| view.read(cx).terminal_menu_state(window, cx)),
            grid: self.is_grid(),
            zen: self.is_zen(),
            collapsed: self.is_collapsed(cx),
            list: self.mode == ViewMode::List,
            ..Default::default()
        }
    }

    pub fn is_grid(&self) -> bool {
        self.mode == ViewMode::Grid
    }
    pub(crate) fn is_zen(&self) -> bool {
        self.zen_return.is_some()
    }

    pub(crate) fn toggle_zen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_available() || self.settings_navigation_blocked(cx) {
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
    #[cfg(test)]
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
            .control_with_tooltip(
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
            .child(icon("grid", ICON_MD, c::FG_DIM()));
        div()
            .flex()
            .items_center()
            .gap(rpx(gap))
            .child(
                self.control_with_tooltip(
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
                    ICON_MD,
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
    #[cfg(test)]
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
        let heading_size = if self.mode == ViewMode::List {
            TEXT_MICRO
        } else {
            TEXT_SMALL
        };
        let font_size = rpx(heading_size).to_pixels(window.rem_size());
        let label_width = crate::views::components::truncation_safe_label_width(
            label,
            font_size,
            &run,
            window.text_system(),
        );
        let count = format!("({})", self.snapshot.projects.len());
        let mut count_run = window.text_style().to_run(count.len());
        count_run.font.weight = gpui::FontWeight::NORMAL;
        let count_width = crate::views::components::truncation_safe_label_width(
            &count,
            font_size,
            &count_run,
            window.text_system(),
        );
        div()
            .id("sidebar-navigation-header")
            .debug_selector(|| "sidebar-navigation-header".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_wrap()
            .justify_end()
            .items_center()
            .gap(rpx(SPACE_SM))
            .text_size(rpx(heading_size))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(c::FG_DIM())
            .child(motion::fast(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(project, |heading| {
                        heading.min_w(count_width + rpx(SPACE_SM).to_pixels(window.rem_size()))
                    })
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
                                .child(count),
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
                        self.control_with_tooltip(
                            "projects-add",
                            if self.mode == ViewMode::List {
                                "New session"
                            } else {
                                "Add project"
                            },
                            if self.mode == ViewMode::List {
                                Action::NewMultiProjectSession
                            } else {
                                Action::AddProject
                            },
                            cx,
                        )
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
        let source_bounds = if matches!(action, Action::ToggleUsage) {
            self.usage_anchor.clone()
        } else {
            std::rc::Rc::new(std::cell::Cell::new(gpui::Bounds::default()))
        };
        let click_bounds = source_bounds.clone();
        let key_bounds = source_bounds.clone();
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
        let project_diff_button = self.mode == ViewMode::Project
            && matches!(&action, Action::OpenDiff(_) | Action::OpenWorktreeDiff(_));
        let quiet_launch_control =
            matches!(&action, Action::RunScript(..) | Action::RunNamedScript(..));
        let quiet_launch_control = quiet_launch_control || action.is_fixture_launch();
        let project_disclosure = matches!(
            &action,
            Action::GroupSessions(_)
                | Action::ToggleGrid
                | Action::OpenWorkspaces
                | Action::MarkReviewed(..)
                | Action::MarkActivityDone(_)
        );
        let grouping_control = matches!(&action, Action::GroupSessions(_) | Action::ToggleGrid);
        let enabled = !matches!(&action, Action::ToggleGrid) || self.grid_navigation_available();
        let danger = matches!(action, Action::ConfirmWorktreeRemoval);
        let primary = matches!(action, Action::DismissWorktreeRemoval);
        let confirm = matches!(action, Action::ConfirmClose(_) | Action::ConfirmHome(_));
        let confirm = confirm || action.is_fixture_remove();
        let cancel = matches!(action, Action::Cancel);
        let confirming = self.confirmation_open();
        let decision = matches!(
            action,
            Action::ConfirmClose(_)
                | Action::ConfirmHome(_)
                | Action::ConfirmWorktreeRemoval
                | Action::SkipWorktreeTeardown
                | Action::DismissWorktreeRemoval
                | Action::Cancel
        );
        let decision = decision || action.is_fixture_remove();
        div()
            .id(id)
            .relative()
            .child(
                gpui::canvas(move |rect, _, _| source_bounds.set(rect), |_, (), _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .role(gpui::Role::Button)
            .when(
                enabled && (!confirming || decision),
                gpui::Styled::cursor_pointer,
            )
            .when(
                !enabled || confirming && !decision,
                gpui::Styled::cursor_default,
            )
            .when(!enabled, |control| control.opacity(OPACITY_DISABLED))
            .aria_label(label.clone())
            .tab_index(if !enabled || confirming && !decision {
                -1
            } else {
                0
            })
            .when(confirm, |d| d.track_focus(&self.confirm_focus))
            .when(cancel, |d| d.track_focus(&self.cancel_focus))
            .size(rpx(CHROME_CONTROL_H))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_SM))
            .justify_center()
            .rounded(rpx(RADIUS_CONTROL))
            .text_color(c::FG_DIM())
            .when(
                enabled && (!confirming || decision) && !danger && !primary,
                |control| control.group("neutral-control"),
            )
            .hover(move |s| {
                if !enabled || confirming && !decision {
                    s
                } else if danger {
                    s.text_color(c::RED())
                } else if primary {
                    s.text_color(c::BG())
                } else if project_disclosure {
                    s.text_color(c::FG())
                } else {
                    s.bg(c::BG_HOVER()).text_color(c::FG())
                }
            })
            .focus_visible(move |s| {
                if !enabled {
                    s
                } else if grouping_control || hierarchy_row {
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
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                gpui_component::global_state::GlobalState::suppress_text_selection(cx);
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                if !enabled {
                    return;
                }
                if click.is_fixture_launch()
                    || matches!(
                        &click,
                        Action::RunScript(..)
                            | Action::RunNamedScript(..)
                            | Action::RemoveWorktree(..)
                            | Action::HistoryFilterMenu(..)
                            | Action::ToggleUsage
                    )
                {
                    this.action_source_bounds.set(click_bounds.get());
                }
                #[cfg(test)]
                if matches!(
                    &click,
                    Action::OpenWorktreeDiff(_) | Action::Select(Selection::Worktree(..))
                ) {
                    eprintln!(
                        "worktree diff click {}",
                        if matches!(&click, Action::OpenWorktreeDiff(_)) {
                            "diff"
                        } else {
                            "row"
                        }
                    );
                }
                this.act(click.clone(), window, cx);
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if !enabled {
                        return;
                    }
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        if action.is_fixture_launch()
                            || matches!(
                                &action,
                                Action::RunScript(..)
                                    | Action::RunNamedScript(..)
                                    | Action::RemoveWorktree(..)
                                    | Action::HistoryFilterMenu(..)
                                    | Action::ToggleUsage
                            )
                        {
                            this.action_source_bounds.set(key_bounds.get());
                        }
                        this.act(action.clone(), window, cx);
                    }
                }),
            )
    }
    /// Add a tooltip only for controls whose owner explicitly needs one.
    fn control_with_tooltip(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        action: Action,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let label = label.into();
        let tooltip_label = label.clone();
        self.control(id, label, action, cx)
            .tooltip(move |window, cx| {
                crate::views::components::tooltip(tooltip_label.clone(), window).build(window, cx)
            })
    }
    fn row(
        &self,
        id: String,
        label: String,
        selected: bool,
        action: Action,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let worktree = matches!(&action, Action::Select(Selection::Worktree(..)));
        let tooltip_label = label.clone();
        self.control(SharedString::from(id.clone()), label, action, cx)
            .when(worktree, |row| {
                row.tooltip(move |window, cx| {
                    crate::views::components::tooltip(tooltip_label.clone(), window)
                        .build(window, cx)
                })
            })
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
    #[cfg(test)]
    pub(crate) fn dashboard_draft_for_test(&self, cx: &App) -> Option<String> {
        self.dashboard
            .as_ref()
            .map(|dashboard| dashboard.session.read(cx).state.saved.draft.clone())
    }

    #[cfg(test)]
    pub(crate) fn select_project_canvas_for_test(&mut self, cx: &mut Context<Self>) {
        self.select(Selection::Project(0), cx);
    }

    fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        self.checkout_panel.change_tools.dismiss();
        self.dashboard_open = false;
        if self.destination == global_navigation::Destination::History {
            self.destination = global_navigation::Destination::Sessions;
            self.session_history_selected = None;
            self.session_history_view = None;
        }
        match selection {
            Selection::Project(_) | Selection::Worktree(..) => {
                self.destination = global_navigation::Destination::Projects;
                self.mode = ViewMode::Project;
                self.last_mode = ViewMode::Project;
            }
            Selection::Home(_) => self.destination = global_navigation::Destination::Terminals,
            Selection::Session(_)
                if self.destination == global_navigation::Destination::Home
                    || self.destination == global_navigation::Destination::Terminals =>
            {
                if self.mode == ViewMode::Grid {
                    self.destination = global_navigation::Destination::Projects;
                } else {
                    self.destination = global_navigation::Destination::Sessions;
                    self.mode = ViewMode::List;
                }
            }
            Selection::Session(_) => {}
        }
        if self
            .history_selection
            .as_ref()
            .is_some_and(|owner| owner != &selection)
        {
            self.history_hover = branch_history::HistoryHover::default();
            self.history = None;
            self.history_observer = None;
            self.history_selection = None;
            self.history_return_focus = None;
        }
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
        let name = self.launch_project_name(project, &path, cx)?;
        Some((name, path, agent))
    }

    fn launch_project_name(&self, project: usize, path: &str, cx: &App) -> Option<String> {
        self.snapshot
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
                                    .project_workspace_id(path)
                                    == cx.global::<SettingsState>().store.workspaces.active
                        })
            })
            .map(|p| p.name.clone())
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
            self.show_launch_error(format!(
                "{} CLI not found. Install it or refresh the shell environment in Settings, then retry. Check that its executable is on your shell PATH.",
                match agent {
                    Agent::Codex => "Codex",
                    Agent::Claude => "Claude Code",
                    Agent::OpenCode => "OpenCode",
                    Agent::Terminal => "Terminal",
                }
            ), cx);
            return false;
        }
        let Some((name, path, agent)) = self.launch_target(project, path, agent, cx) else {
            self.show_launch_error(
                "This checkout is no longer available in this workspace. Select a current checkout and retry.".into(),
                cx,
            );
            return false;
        };
        if !self
            .runtime
            .update(cx, |r, cx| r.spawn_session_in(name, path, agent, cx))
        {
            return false;
        }
        if let Some(id) = self.runtime.read(cx).state.read(cx).active_session() {
            self.select(Selection::Session(id), cx);
        }
        self.content_error = None;
        cx.notify();
        true
    }
    fn show_launch_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.content_error = Some(error.clone());
        if self.project_flyout.is_some() {
            self.project_flyout_launch_error = Some(error.clone());
        }
        self.runtime.read(cx).toast.clone().update(cx, |toast, cx| {
            toast.set_error(error, cx);
        });
        cx.notify();
    }
    pub(crate) fn select_active_session(&mut self, cx: &mut Context<Self>) -> Option<SessionId> {
        if self.settings_navigation_blocked(cx) {
            return None;
        }
        let id = self.runtime.read(cx).state.read(cx).active_session()?;
        self.select(Selection::Session(id), cx);
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
        if let Some(view) = self.codex_views.get(&id) {
            self.pending_canvas_focus = None;
            self.pending_grid_workspace_focus = None;
            view.focus_handle(cx).focus(window, cx);
        } else if let Some(view) = self.terminal_views.get(&id) {
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

    pub(crate) fn request_close_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.navigation_available() || self.settings_navigation_blocked(cx) {
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
                    self.act(Action::CloseHome(id), window, cx);
                }
            }
            Some(Selection::Project(_) | Selection::Worktree(_, _)) | None => {}
        }
    }

    pub(super) fn html_owner(&self, cx: &App) -> Option<gpui::EntityId> {
        let Some(Selection::Session(id)) = self.selection else {
            return None;
        };
        self.codex_views
            .get(&id)
            .map(|view| view.read(cx).html.entity_id())
    }

    pub(super) fn html_overlay_open(&self) -> bool {
        self.usage_panel.is_some()
            || self.confirmation_open()
            || self.header_tasks.open.is_some()
            || self.history.is_some()
            || self.diff_viewer.is_some()
            || self.project_panel.is_some()
            || self.project_setup.is_some()
            || self.pending_new_worktree.is_some()
            || self.project_flyout.is_some()
    }

    pub(crate) fn highlights_blocked(&self) -> bool {
        !self.navigation_available()
    }

    pub(crate) fn navigation_available(&self) -> bool {
        !self.confirmation_open()
            && self.header_tasks.open.is_none()
            && self.history.is_none()
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
            || self.settings_navigation_blocked(cx)
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

    /// Preserve Shell's modal/Settings gate after moving Grid into the sidebar.
    /// Other destinations retain their existing independent navigation rules.
    pub(crate) fn set_grid_blocked_by_shell(&mut self, blocked: bool, cx: &mut Context<Self>) {
        if self.grid_blocked_by_shell != blocked {
            self.grid_blocked_by_shell = blocked;
            cx.notify();
        }
    }

    fn grid_navigation_available(&self) -> bool {
        self.navigation_available() && !self.grid_blocked_by_shell
    }

    pub(crate) fn toggle_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.grid_navigation_available() {
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
        if self.navigation_available() && !self.settings_navigation_blocked(cx) {
            self.act(Action::AddTerminal, window, cx);
            self.focus.focus(window, cx);
        }
    }

    pub(crate) fn open_project_checkout_from_settings(
        &mut self,
        root: &str,
        path: &str,
        create: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = cx
            .global::<SettingsState>()
            .store
            .workspace_projects(self.active_workspace)
            .find(|(_, project)| project.path == root)
            .map(|(index, _)| index);
        if let Some(index) = index {
            if self.settings_navigation_blocked(cx) {
                return;
            }
            if self.open_dashboard_for_checkout(index, root, path, window, cx) && create {
                self.begin_worktree(index, window, cx);
            }
        }
    }

    pub(crate) fn manage_project_checkouts(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = cx
            .global::<SettingsState>()
            .store
            .workspace_projects(self.active_workspace)
            .find(|(_, project)| project.path == path)
            .map(|(index, _)| index);
        if let Some(index) = index {
            if !self.close_settings_for_session_navigation(window, cx) {
                return;
            }
            self.select(Selection::Project(index), cx);
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
                        && project.scripts.effective_entries().iter().any(|entry| {
                            matches!(
                                entry.kind,
                                grove_core::storage::ProjectActionKind::Shell { .. }
                            )
                        })
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
        if !self.navigation_available() || self.settings_navigation_blocked(cx) {
            return;
        }
        let targets = self.checkout_targets(cx);
        if let Some((_, path)) =
            checkout_panel::selected_checkout(&targets, self.checkout_panel.chosen_root.as_deref())
        {
            self.open_worktree_terminal(&path, cx);
        }
    }

    #[cfg(test)]
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
    fn choose_project_script(
        &mut self,
        project_path: String,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_component::menu::{PopupMenu, PopupMenuItem};
        let entries = cx
            .global::<SettingsState>()
            .store
            .projects
            .iter()
            .find(|p| p.path == project_path)
            .map(|p| p.scripts.effective_entries())
            .unwrap_or_default()
            .into_iter()
            .filter(|e| matches!(e.kind, grove_core::storage::ProjectActionKind::Shell { .. }))
            .collect::<Vec<_>>();
        if entries.len() <= 1 {
            self.run_script(project_path, path, cx);
            return;
        }
        let focus = window.focused(cx);
        let owner = cx.entity().downgrade();
        let popup = PopupMenu::build(window, cx, move |mut popup, window, _| {
            popup = crate::views::components::dropdown_menu(popup, window);
            for entry in entries {
                let owner = owner.clone();
                let project_path = project_path.clone();
                let path = path.clone();
                popup = popup.item(PopupMenuItem::new(entry.name).on_click(
                    move |_, window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.act(
                                Action::RunNamedScript(
                                    project_path.clone(),
                                    path.clone(),
                                    entry.id.clone(),
                                ),
                                window,
                                cx,
                            );
                        });
                    },
                ));
            }
            popup
        });
        self.checkout_panel.popup_position = Some(window.mouse_position());
        self.checkout_panel.popup_subscription = Some(cx.subscribe_in(
            &popup,
            window,
            move |this, _, _: &gpui::DismissEvent, window, cx| {
                this.checkout_panel.popup = None;
                this.checkout_panel.popup_subscription = None;
                if let Some(focus) = &focus {
                    focus.focus(window, cx);
                }
                cx.notify();
            },
        ));
        popup.focus_handle(cx).focus(window, cx);
        self.checkout_panel.popup = Some(popup);
        cx.notify();
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
        if self.settings_navigation_blocked(cx) {
            return;
        }
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
            && !action.is_fixture_remove()
            && !matches!(
                action,
                Action::ConfirmClose(_) | Action::ConfirmHome(_) | Action::Cancel
            )
        {
            return;
        }
        if !matches!(action, Action::ToggleUsage) {
            self.dismiss_usage(false, window, cx);
        }
        let was_project_flyout = self.project_flyout.is_some();
        if !action.is_fixture_launch()
            && !matches!(
                &action,
                Action::Close(_)
                    | Action::FinishSession(_)
                    | Action::ConfirmClose(_)
                    | Action::Cancel
            )
        {
            self.dismiss_project_context(cx);
            self.track_project_flyout_pointer(window.mouse_position(), window, cx);
        }
        match action {
            Action::ToggleGrid => self.toggle_grid(window, cx),
            Action::ToggleSidebar => {
                let from = self.animated_sidebar_width(window, cx);
                let collapsed = !self.navigation_collapsed(window, cx);
                self.navigation_width_override =
                    (!collapsed).then_some(Self::logical_window_width(window));
                let to = if collapsed {
                    SIDEBAR_COLLAPSED_W
                } else {
                    self.context_width(window, cx)
                };
                self.navigation_transition =
                    (!cx.reduce_motion()).then_some((Instant::now(), from, to));
                self.drag = None;
                self.menu = None;
                self.menu_return_focus = None;
                self.menu_opened_by_hover = false;
                SettingsState::update(cx, |store| store.sidebar_collapsed = collapsed);
                self.focus.focus(window, cx);
                cx.notify();
            }
            Action::GroupSessions(grouping) => {
                SettingsState::update(cx, |store| {
                    store.sidebar_group_by_project = false;
                    store.sidebar_group_by_worktree =
                        grouping == global_navigation::SessionGrouping::Worktrees;
                });
                cx.notify();
            }
            Action::RefreshSessionHistory => {
                self.runtime
                    .read(cx)
                    .registry
                    .clone()
                    .update(cx, |registry, cx| {
                        let _ = registry.retry_cli_history();
                        cx.notify();
                    });
                self.load_session_history(cx);
            }
            Action::HistoryFilterMenu(project) => self.open_history_filter(project, window, cx),
            Action::ClearHistorySearch => {
                self.history_search.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                    input.focus_handle(cx).focus(window, cx);
                });
                cx.notify();
            }
            Action::ClearHistoryFilters => {
                self.history_project_filter = None;
                self.history_provider_filter = None;
                self.history_search
                    .update(cx, |input, cx| input.set_value("", window, cx));
                cx.notify();
            }
            Action::PreviewSessionHistory(key) => {
                self.hydrate_history_session(&key, false, window, cx);
            }
            Action::PreviewCliHistory(key) => {
                if self.cli_session_history.iter().any(|s| s.key == key) {
                    self.session_history_selected =
                        Some(session_history::HistorySelection::Cli(key));
                    self.session_history_view = None;
                }
                cx.notify();
            }
            Action::ReopenSessionHistory(key) => self.reopen_history_session(&key, window, cx),
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
            #[cfg(test)]
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
            #[cfg(test)]
            Action::ArchivedProjects => {
                self.open_project_panel(projects::Page::Archived, window, cx);
            }
            #[cfg(test)]
            Action::NewMultiProjectSession => {
                if self.navigation_available() {
                    cx.emit(SidebarEvent::NewMultiProjectSession);
                }
            }
            Action::UpdateApp => {
                if self.navigation_available() {
                    let upgrade = self.runtime.read(cx).upgrade.clone();
                    if upgrade.read(cx).can_update() {
                        if let Some(settings) = &self.settings_panel {
                            settings.update(cx, |settings, cx| settings.open_updates(window, cx));
                        }
                        upgrade.update(cx, crate::entities::upgrade::Upgrade::start_update);
                    }
                }
            }
            Action::Destination(destination) => self.open_destination(destination, window, cx),
            Action::ToggleUsage => self.toggle_usage(window, cx),
            Action::DashboardCheckout {
                project,
                root,
                path,
            } => {
                self.open_dashboard_for_checkout(project, &root, &path, window, cx);
            }
            Action::OpenSettings => {
                if self.navigation_available() {
                    cx.emit(SidebarEvent::Settings);
                }
            }
            #[cfg(test)]
            Action::EditProject(path) => {
                if !self.project_path_is_active(&path, cx) {
                    return;
                }
                self.open_project_panel(projects::Page::Edit(path), window, cx);
            }
            Action::EditProjectScripts(path, add) => {
                if !self.project_path_is_active(&path, cx)
                    && !self
                        .checkout_targets(cx)
                        .iter()
                        .any(|(_, target)| target == &path)
                {
                    return;
                }
                self.open_project_panel(projects::Page::Edit(path), window, cx);
                if let Some(panel) = &self.project_panel {
                    panel.update(cx, |panel, cx| panel.open_scripts(add, window, cx));
                }
            }
            #[cfg(test)]
            Action::MoveProject(path) => {
                if !self.project_path_is_active(&path, cx) {
                    return;
                }
                self.open_project_panel(projects::Page::Move(path), window, cx);
            }
            Action::Select(s) => {
                let leaving_project_panel = self.project_panel.is_some();
                if let Selection::Session(id) = s {
                    self.navigate_to_session(id, window, cx);
                    return;
                }
                if matches!(s, Selection::Home(_))
                    && !self.close_settings_for_session_navigation(window, cx)
                {
                    return;
                }
                self.select(s.clone(), cx);
                let view = match s {
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
                self.destination = if mode == ViewMode::List {
                    global_navigation::Destination::Sessions
                } else {
                    global_navigation::Destination::Projects
                };
                self.dashboard_open = false;
                self.menu = None;
                if leaving_project_panel {
                    self.focus.focus(window, cx);
                }
            }
            #[cfg(test)]
            Action::Menu(i) => {
                // Masking keeps the document alive, including its native responder.
                // GPUI focus alone cannot take keys back from that responder.
                crate::platform::inline_html::NativeDocument::focus_host(window);
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
            #[cfg(test)]
            Action::RemoveProject(path) => {
                if !self.project_path_is_active(&path, cx) {
                    return;
                }
                self.open_project_panel(projects::Page::Remove(path), window, cx);
            }
            #[cfg(test)]
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
            #[cfg(test)]
            Action::Launch(i, path, agent) => {
                if self.launch(i, path, agent, cx) {
                    if was_project_flyout {
                        self.dismiss_project_context(cx);
                    }
                    let native = self
                        .runtime
                        .read(cx)
                        .state
                        .read(cx)
                        .active_session()
                        .is_some_and(|id| {
                            self.runtime.read(cx).registry.read(cx).codex(id).is_some()
                        });
                    if was_project_flyout || native {
                        self.focus_active_session_after_palette(window, cx);
                    }
                }
            }
            Action::RunScript(project_path, path) => {
                self.choose_project_script(project_path, path, window, cx);
            }
            Action::RunNamedScript(project_path, path, id) => {
                let entry = cx
                    .global::<SettingsState>()
                    .store
                    .script_project(&project_path)
                    .scripts
                    .effective_entries()
                    .into_iter()
                    .find(|entry| entry.id == id);

                if let Some(grove_core::storage::ProjectAction {
                    kind: grove_core::storage::ProjectActionKind::Prompt { template },
                    ..
                }) = entry
                {
                    let feedback_action = format!("script-{id}");
                    let token =
                        self.begin_panel_feedback(&path, &feedback_action, "Sending prompt…", cx);
                    let result = match self.selection {
                        Some(Selection::Session(session_id)) => self
                            .runtime
                            .read(cx)
                            .registry
                            .read(cx)
                            .codex(session_id)
                            .cloned()
                            .ok_or_else(|| {
                                "Open a native Codex or Claude chat in this checkout first"
                                    .to_owned()
                            })
                            .and_then(|session| {
                                if session.read(cx).state.saved.cwd != path {
                                    return Err(
                                        "The current chat belongs to another checkout".into()
                                    );
                                }
                                session
                                    .update(cx, |session, cx| session.project_prompt(template, cx))
                                    .map(|_| ())
                            }),
                        _ => {
                            Err("Open a native Codex or Claude chat in this checkout first".into())
                        }
                    };
                    self.finish_panel_feedback(
                        &path,
                        &feedback_action,
                        token,
                        result.is_ok().then_some("Prompt queued"),
                        cx,
                    );
                    let error_key = (path.clone(), id.clone());
                    if let Err(error) = &result {
                        self.checkout_panel
                            .script_errors
                            .insert(error_key, error.clone());
                    } else {
                        self.checkout_panel.script_errors.remove(&error_key);
                    }
                    self.content_error = result.err();
                    cx.notify();
                } else if self.runtime.update(cx, |runtime, cx| {
                    runtime.spawn_project_script(&project_path, &path, &id, cx)
                }) {
                    let run = self.runtime.read(cx).script_runs.last().cloned();
                    if let Some(run) = run {
                        self.watch_script_feedback(&run, cx);
                    }
                    if self.mode == ViewMode::Grid {
                        self.mode = self.last_mode;
                    }
                    self.select_active_session(cx);
                    cx.notify();
                }
            }

            Action::Close(id) => {
                if self.project_flyout.as_ref() == Some(&SidebarContext::MultiProject) {
                    self.project_flyout_hide_task = None;
                }
                self.pending_close = Some(id);
                self.confirmation_return_focus = window.focused(cx);
                self.cancel_focus.focus(window, cx);
            }
            Action::MarkReviewed(id, work_period) => {
                let keyboard = window.last_input_was_keyboard();
                let workspace = self.active_workspace;
                let session = self.runtime.read(cx).registry.read(cx).codex(id).cloned();
                if let Some(session) = session {
                    let state = &session.read(cx).state;
                    if state.session_status() == "Review"
                        && state.review_completion() == Some(work_period)
                    {
                        let review = &state.saved.session_review;
                        let end = review
                            .pending
                            .as_ref()
                            .and_then(|receipt| state.saved.work_periods.get(receipt.period))
                            .and_then(|period| period.end_item);
                        let epoch = state.interaction_epoch;
                        let error = state.error.clone();
                        self.dashboard_timelines
                            .review_holds
                            .begin(workspace, id, keyboard);
                        self.dashboard_timelines.review_advance = (keyboard && self.dashboard_open)
                            .then(|| {
                                let row_focus =
                                    self.dashboard_timelines.session_focus(workspace, id, cx);
                                cx.observe_in(&session, window, move |this, session, window, cx| {
                                    let state = &session.read(cx).state;
                                    let review = &state.saved.session_review;
                                    if !this.dashboard_open
                                        || this.active_workspace != workspace
                                        || state.interaction_epoch != epoch
                                        || !row_focus.is_focused(window)
                                    {
                                        this.dashboard_timelines.review_advance = None;
                                        return;
                                    }
                                    if state.review_completion() == Some(work_period) {
                                        if state.error != error {
                                            this.dashboard_timelines.review_advance = None;
                                        }
                                        return;
                                    }
                                    this.dashboard_timelines.review_advance = None;
                                    if review.pending.is_none()
                                        && end == Some(review.reviewed_through)
                                        && state.session_status() == "Ready"
                                    {
                                        this.focus_next_dashboard_review(id, window, cx);
                                    }
                                })
                            });
                        session.update(cx, |session, cx| {
                            session
                                .send(grove_core::codex::Action::MarkReviewed { work_period }, cx);
                        });
                    }
                }
                self.dashboard_timelines.restore_review_focus(
                    self.active_workspace,
                    id,
                    window,
                    cx,
                );
                cx.notify();
            }
            Action::MarkTerminalReviewed(id, completion) => {
                self.dashboard_timelines.review_holds.begin(
                    self.active_workspace,
                    id,
                    window.last_input_was_keyboard(),
                );
                self.runtime
                    .read(cx)
                    .activity
                    .clone()
                    .update(cx, |activity, cx| {
                        activity.mark_terminal_reviewed(id, completion, cx);
                    });
                self.dashboard_timelines.restore_review_focus(
                    self.active_workspace,
                    id,
                    window,
                    cx,
                );
                cx.notify();
            }
            Action::MarkActivityDone(id) => {
                // Recheck the live state: a row may have changed since it was painted.
                let ready = self
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .codex(id)
                    .is_some_and(|session| session.read(cx).state.session_status() == "Ready");
                if ready {
                    self.act(Action::FinishSession(id), window, cx);
                }
            }
            Action::FinishSession(id) => {
                let selected = self.selection == Some(Selection::Session(id));
                self.runtime
                    .update(cx, |runtime, cx| runtime.kill_session(id, cx));
                if selected {
                    self.open_landing_page(window, cx);
                }
                cx.notify();
            }
            Action::ConfirmClose(id) => {
                self.canvas_close_anchor = None;
                self.runtime.update(cx, |r, cx| r.kill_session(id, cx));
                self.pending_close = None;
                self.open_landing_page(window, cx);
            }
            Action::Retry(id) => {
                if let Some(session) = self.runtime.read(cx).registry.read(cx).codex(id).cloned() {
                    session.update(cx, |session, cx| {
                        session.send(grove_core::codex::Action::Reconnect, cx);
                    });
                    return;
                }
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
                        .multi_root_launch_args_for_backend(
                            crate::runtime::native_codex_enabled(store, None),
                            store.yolo_enabled,
                            store.chrome_enabled.unwrap_or(false),
                            &extra_roots,
                        )
                        .unwrap_or_else(|| {
                            meta.agent.launch_args_for_backend(
                                crate::runtime::native_codex_enabled(store, None),
                                store.yolo_enabled,
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
                    let active = self.runtime.read(cx).state.read(cx).active_session();
                    if let Some(id) = active {
                        self.select(Selection::Session(id), cx);
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
                self.act(Action::OpenWorktreeDiff(path), window, cx);
            }
            Action::OpenBranchHistory(path, selection, identity) => {
                if self.selection == selection
                    && self.branch_entry.as_ref() == Some(&(path.clone(), identity))
                {
                    self.branch_entry_focus.focus(window, cx);
                    self.act(Action::OpenHistory(path), window, cx);
                }
            }
            Action::RefreshBranchPreview => {
                if let Some(cache) = &self.branch_peek_cache {
                    cache.data.update(cx, BranchHistoryState::reload);
                }
            }
            Action::OpenHistory(path) => {
                self.history_hover = branch_history::HistoryHover::default();
                self.dismiss_branch_peek(window, cx);
                self.dismiss_agent_peek(window, cx);
                let path = crate::paths::normalize_wt_path(&path).to_string();
                let history = cx.new(|cx| BranchHistoryState::new(path, cx));
                self.history_observer = Some(cx.observe(&history, |_, _, cx| cx.notify()));
                self.history = Some(history);
                self.history_selection = self.selection.clone();
                self.history_return_focus = window.focused(cx);
                self.history_focus.focus(window, cx);
            }
            Action::CloseHistory => {
                self.history_hover = branch_history::HistoryHover::default();
                self.history = None;
                self.history_observer = None;
                self.history_selection = None;
                if let Some(focus) = self.history_return_focus.take() {
                    focus.focus(window, cx);
                }
            }
            Action::HistoryGraph => {
                if let Some(history) = &self.history {
                    history.update(cx, BranchHistoryState::collapse);
                    self.history_focus.focus(window, cx);
                }
            }
            Action::RefreshHistory => {
                self.history_hover = branch_history::HistoryHover::default();
                if let Some(history) = &self.history {
                    history.update(cx, BranchHistoryState::reload);
                }
            }
            Action::MoreHistory => {
                self.history_hover = branch_history::HistoryHover::default();
                if let Some(history) = &self.history {
                    history.update(cx, BranchHistoryState::more);
                }
            }
            Action::SelectCommit(id) => {
                self.pin_history_card();
                if let Some(history) = &self.history {
                    history.update(cx, |history, cx| history.select(id, cx));
                }
            }
            Action::ExpandCommit => {
                self.pin_history_card();
                if let Some(history) = &self.history {
                    history.update(cx, BranchHistoryState::expand);
                }
            }
            Action::HistoryDiffMode(mode) => {
                if let Some(history) = &self.history {
                    history.update(cx, |history, cx| history.set_diff_mode(mode, cx));
                }
            }
            Action::ToggleHistoryMetadata => {
                if let Some(history) = &self.history {
                    history.update(cx, BranchHistoryState::toggle_metadata);
                }
            }
            Action::CopyHistoryCommit => {
                if let Some(id) = self
                    .history
                    .as_ref()
                    .and_then(|history| history.read(cx).selected.clone())
                {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(id));
                }
            }
            Action::SelectHistoryFile(path) => {
                if let Some(history) = &self.history {
                    let file = history
                        .read(cx)
                        .details
                        .as_ref()
                        .and_then(|details| details.files.iter().find(|f| f.path == path))
                        .cloned();
                    if let Some(file) = file {
                        history.update(cx, |history, cx| history.file(file, cx));
                    }
                }
            }
            Action::OpenWorktreeDiff(path) => {
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
                    if !self.close_settings_for_session_navigation(window, cx) {
                        return;
                    }
                    self.select(Selection::Home(id), cx);
                    self.mode = ViewMode::Project;
                    SettingsState::update(cx, |store| store.sidebar_collapsed = false);
                    self.pending_canvas_focus = Some(id);
                }
            }
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
    fn session_title_label(
        &self,
        id: SessionId,
        title: &str,
        surface: &'static str,
        cx: &App,
    ) -> AnyElement {
        let pending = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(id)
            .is_some_and(|session| session.read(cx).state.initial_title_pending());
        super::components::shimmer_label(
            (surface, id.raw()).into(),
            single_line_session_title(title),
            pending,
            cx,
        )
    }
    fn status(&self, meta: &SessionMeta, cx: &App) -> (&'static str, gpui::Hsla) {
        let runtime = self.runtime.read(cx);
        let registry = runtime.registry.read(cx);
        if let Some(session) = registry.codex(meta.id) {
            let state = &session.read(cx).state;
            let label =
                structured_session_status(state, runtime.activity.read(cx).state_of(meta.id));
            return (label, super::status::session_kind(label).color());
        }
        let failed = registry
            .session(meta.id)
            .is_some_and(|t| t.read(cx).spawn_error().is_some());
        let pending_attach = registry
            .session(meta.id)
            .is_some_and(|t| t.read(cx).is_pending_attach());
        let activity = runtime.activity.read(cx);
        let status = managed_session_status(failed, pending_attach, activity.state_of(meta.id));
        if !failed
            && !pending_attach
            && activity.terminal_review(meta.id).is_some()
            && matches!(status.0, "Ready" | "Done")
        {
            let label = if activity.state_of(meta.id) == ActivityState::Done {
                "Review · unread"
            } else {
                "Review"
            };
            return (label, super::status::session_kind(label).color());
        }
        status
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
            #[cfg(test)]
            Action::ConfirmRemove(_) => "Remove project",
            _ => "Confirm",
        };
        let confirm_button = self
            .control("confirm-close", verb, action, cx)
            .debug_selector(|| "confirm-close".into())
            .w_auto()
            .px(rpx(SPACE_LG))
            .text_color(c::RED())
            .when(!session_close, |button| {
                button.child(icon("check", ICON_XS, c::RED()))
            })
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
            .when(!session_close, |button| {
                button.child(icon("close", ICON_XS, c::FG_DIM()))
            })
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
    /// Keep source-attached close dialogs mounted independently of the row's
    /// visibility, including responsive navigation collapse and list filtering.
    fn navigation_close_confirmation(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.canvas_close_anchor.is_some() {
            return None;
        }
        let (id, message, action, source) = if let Some(id) = self.pending_close {
            let registry = self.runtime.read(cx).registry.read(cx);
            let meta = registry.meta(id)?;
            (
                id,
                format!(
                    "Close {} in {}? Its process will stop. The worktree stays on disk.",
                    meta.label, meta.project
                ),
                Action::ConfirmClose(id),
                self.session_close_bounds.get(&id)?.get(),
            )
        } else if let Some(id) = self.pending_home_close {
            let registry = self.runtime.read(cx).registry.read(cx);
            let meta = registry
                .home_terminals()
                .iter()
                .find(|meta| meta.id == id)?;
            (
                id,
                format!(
                    "Close {}? Its shell and running commands will stop. Files remain on disk.",
                    meta.label
                ),
                Action::ConfirmHome(id),
                self.home_close_bounds.get(&id)?.get(),
            )
        } else {
            return None;
        };
        let popup = self.confirmation_popup(&message, action, source, window, cx);
        Some(
            div()
                .id(("session-close-popup", id.raw()))
                .absolute()
                .debug_selector(move || format!("session-close-popup-{}", id.raw()))
                .child(gpui::deferred(popup))
                .into_any_element(),
        )
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
        let (status, _) = self.status(meta, cx);
        let id = meta.id;
        let tasks = self.task_glance(id).cloned();
        let tasks_suffix = tasks.as_ref().map_or_else(String::new, |tasks| {
            format!(" · {}", tasks.tooltip_suffix())
        });
        let registry = self.runtime.read(cx).registry.read(cx);
        let title = session_display_title(
            meta,
            registry
                .codex(id)
                .and_then(|session| session.read(cx).state.saved.title.clone())
                .or_else(|| {
                    registry
                        .session(id)
                        .and_then(|session| session.read(cx).title())
                }),
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
            let selected = self.selection == Some(Selection::Session(id)) && !self.dashboard_open;
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
            let scope = session_scope::session_scope(meta, &self.snapshot);
            let diff = self
                .runtime
                .read(cx)
                .tree
                .read(cx)
                .git_state(crate::paths::normalize_wt_path(&meta.wt_path))
                .map_or_else(
                    || ("Git status unavailable".into(), false),
                    session_diff_status,
                );
            let full_context = scope.as_ref().map_or_else(
                || branch.map_or_else(|| context.clone(), |branch| format!("{context} · {branch}")),
                |scope| {
                    session_scope::scope_accessible_details(
                        scope,
                        &session_root_inventory(meta, &self.snapshot),
                    )
                },
            );
            let (header_project, header_worktree) = scope.as_ref().map_or_else(
                || (meta.project.clone(), name.clone()),
                |scope| (scope.projects.clone(), scope.shared.clone()),
            );
            self.row(
                format!("session-{}", id.raw()),
                format!(
                    "{title} · {full_context} · {status}{tasks_suffix} · {} · {age} · {} session{}",
                    diff.0,
                    meta.agent.label(),
                    number.filter(|number| *number <= 9).map_or_else(
                        String::new,
                        |number| format!(
                            " · shortcut {}{number}",
                            crate::keymap::alt_chord_prefix().replace('-', "+")
                        )
                    )
                ),
                selected,
                Action::Select(Selection::Session(id)),
                cx,
            )
            .relative()
            .group("session-row")
            .h_auto()
            .min_h(rpx(SESSION_LIST_CARD_H))
            .px(rpx(SPACE_LG))
            .py(rpx(SPACE_LG))
            .rounded(rpx(RADIUS_GROUP))
            .bg(if selected {
                c::alpha(c::FG(), 0.14)
            } else {
                c::alpha(c::BG(), 0.0)
            })
            .items_start()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .relative()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .w_full()
                            .min_w_0()
                            .h(rpx(SESSION_LIST_HEADER_H))
                            .gap(rpx(SPACE_SM))
                            .child(super::components::project_symbol_for_path(
                                &meta.wt_path,
                                ICON_MD,
                                cx,
                            ))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(rpx(TEXT_MICRO))
                                    .line_height(rpx(SESSION_LIST_FOOTER_H))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(if selected { c::FG() } else { c::FG_DIM() })
                                    .group_hover("session-row", |label| label.text_color(c::FG()))
                                    .id(("session-list-context", id.raw()))
                                    .debug_selector(move || {
                                        format!("session-list-context-{}", id.raw())
                                    })
                                    .child(header_project),
                            )
                            .child(div().flex_shrink_0().ml(rpx(SPACE_SM)).child(icon(
                                "git-branch",
                                ICON_SM,
                                c::FG_DIM(),
                            )))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(rpx(TEXT_MICRO))
                                    .line_height(rpx(SESSION_LIST_FOOTER_H))
                                    .text_color(c::FG_DIM())
                                    .id(("session-list-worktree", id.raw()))
                                    .debug_selector(move || {
                                        format!("session-list-worktree-{}", id.raw())
                                    })
                                    .child(header_worktree),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .w(rpx(SIDEBAR_ICON_SLOT))
                                    .h(rpx(SESSION_LIST_HEADER_H))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .id(("session-list-agent", id.raw()))
                                    .debug_selector(move || {
                                        format!("session-list-agent-{}", id.raw())
                                    })
                                    .child(icon(meta.agent.icon_name(), TEXT_MICRO, c::FG_DIM())),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .w_full()
                            .truncate()
                            .mt(rpx(SPACE_SM))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_size(rpx(TEXT_TITLE))
                            .line_height(rpx(SESSION_LIST_TITLE_H))
                            .id(("session-list-title", id.raw()))
                            .debug_selector(move || format!("session-list-title-{}", id.raw()))
                            .child(self.session_title_label(
                                id,
                                &title,
                                "session-list-title-shimmer",
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .w_full()
                            .min_w_0()
                            .mt(rpx(SPACE_XS))
                            .h(rpx(CHROME_CONTROL_H))
                            .gap(rpx(SPACE_SM))
                            .child(motion::fast(
                                div()
                                    .flex()
                                    .items_center()
                                    .min_w_0()
                                    .gap(rpx(SPACE_SM))
                                    .text_size(rpx(TEXT_MICRO))
                                    .line_height(rpx(SESSION_LIST_FOOTER_H))
                                    .id(("session-list-status", id.raw()))
                                    .debug_selector(move || {
                                        format!("session-list-status-{}", id.raw())
                                    })
                                    .text_color(super::status::session_kind(status).color())
                                    .child(
                                        div().flex_shrink_0().child(session_status_icon_sized(
                                            id, status, ICON_MD, cx,
                                        )),
                                    )
                                    .child(div().min_w_0().truncate().child(status)),
                                format!("session-status-list-{}-{status}", id.raw()),
                                cx,
                            ))
                            .when_some(tasks.as_ref(), |line, tasks| {
                                line.child(task_glance::row_progress(id, tasks))
                            })
                            .child(div().flex_1().min_w_0())
                            .child(
                                self.control(
                                    ("close-session", id.raw()),
                                    format!("Done: close {} in {}", meta.label, meta.project),
                                    Action::FinishSession(id),
                                    cx,
                                )
                                .debug_selector(move || format!("close-session-{}", id.raw()))
                                .w_auto()
                                .px(rpx(SPACE_MD))
                                .gap(rpx(SPACE_SM))
                                .text_size(rpx(TEXT_MICRO))
                                .when_some(
                                    self.session_close_focus.get(&id),
                                    gpui::InteractiveElement::track_focus,
                                )
                                .opacity(0.0)
                                .group_hover("session-row", |button| button.opacity(1.0))
                                .focus_visible(|button| button.opacity(1.0))
                                .when(
                                    self.session_close_focus
                                        .get(&id)
                                        .is_some_and(|focus| focus.is_focused(window)),
                                    |button| button.opacity(1.0),
                                )
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
                                .relative()
                                .child(icon("check", ICON_XS, c::FG_DIM()))
                                .child("Done"),
                            ),
                    )
                    .children(scope.iter().flat_map(|scope| {
                        scope
                            .exceptions
                            .iter()
                            .map(session_scope::scope_exception_line)
                    })),
            )
        } else {
            let selected = self.selection == Some(Selection::Session(id));
            let worktree_names = multi_project::project_names(meta)
                .map(|_| multi_project::worktree_names(meta, &self.snapshot));
            let row_height = if worktree_names.is_some() {
                SESSION_ROW_H
            } else {
                SESSION_TREE_ROW_H
            };
            self.row(
                format!("session-{}", id.raw()),
                format!(
                    "{label} · {} session · {status}{tasks_suffix}{}",
                    meta.agent.label(),
                    number
                        .filter(|n| *n <= 9)
                        .map_or_else(String::new, |n| format!(
                            " · shortcut {}{n}",
                            crate::keymap::alt_chord_prefix().replace('-', "+")
                        ))
                ),
                selected,
                Action::Select(Selection::Session(id)),
                cx,
            )
            .when(diff_focused && !selected, |row| row.bg(c::BG_HOVER()))
            .relative()
            .group("session-row")
            .h(rpx(row_height))
            .pl(rpx(SESSION_LEADING_INSET))
            .pr(rpx(HIERARCHY_INSET))
            .gap(rpx(SPACE_SM))
            .items_center()
            .rounded(rpx(SESSION_ROW_RADIUS))
            .child(
                div()
                    .id(("session-agent", id.raw()))
                    .debug_selector(move || format!("session-agent-{}", id.raw()))
                    .size(rpx(SESSION_LAUNCHER_SIZE))
                    .flex_shrink_0()
                    .child(icon(
                        meta.agent.icon_name(),
                        SESSION_LAUNCHER_SIZE,
                        c::BLUE(),
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(("session-tree-title", id.raw()))
                            .debug_selector(move || format!("session-tree-title-{}", id.raw()))
                            .truncate()
                            .line_height(rpx(SESSION_TITLE_LINE_H))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_size(rpx(TEXT_SMALL))
                            .child(self.session_title_label(
                                id,
                                &title,
                                "session-tree-title-shimmer",
                                cx,
                            )),
                    )
                    .when_some(worktree_names, |column, names| {
                        column.child(
                            div()
                                .id(("session-tree-worktrees", id.raw()))
                                .debug_selector(move || {
                                    format!("session-tree-worktrees-{}", id.raw())
                                })
                                .truncate()
                                .line_height(rpx(SESSION_META_LINE_H))
                                .font_weight(gpui::FontWeight::NORMAL)
                                .text_size(rpx(TEXT_MICRO))
                                .text_color(c::FG_DIM())
                                .child(names),
                        )
                    }),
            )
            .child(
                div()
                    .id(("session-tree-trailing", id.raw()))
                    .group("session-trailing")
                    .debug_selector(move || format!("session-tree-trailing-{}", id.raw()))
                    .relative()
                    .min_w(rpx(CONTROL_H))
                    .h(rpx(CONTROL_H))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(rpx(SPACE_SM))
                    .when_some(tasks.as_ref(), |slot, tasks| {
                        slot.child(task_glance::row_progress(id, tasks))
                    })
                    .child(
                        div()
                            .size(rpx(CONTROL_H))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .group_hover("session-row", Styled::invisible)
                            .when(
                                self.session_close_focus
                                    .get(&id)
                                    .is_some_and(|focus| focus.is_focused(window)),
                                Styled::invisible,
                            )
                            .child(motion::fast(
                                div()
                                    .id(("session-tree-status", id.raw()))
                                    .debug_selector(move || {
                                        format!("session-tree-status-{}", id.raw())
                                    })
                                    .size(rpx(ICON_MD))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(session_status_icon(id, status, cx)),
                                format!("session-status-tree-{}-{status}", id.raw()),
                                cx,
                            )),
                    )
                    .child(
                        self.control_with_tooltip(
                            ("close-session", id.raw()),
                            format!("Close {} in {}", meta.label, meta.project),
                            Action::Close(id),
                            cx,
                        )
                        .absolute()
                        .right_0()
                        .top_0()
                        .size(rpx(CONTROL_H))
                        .debug_selector(move || format!("session-tree-close-{}", id.raw()))
                        .when_some(
                            self.session_close_focus.get(&id),
                            gpui::InteractiveElement::track_focus,
                        )
                        .opacity(0.0)
                        .group_hover("session-row", |button| button.opacity(1.0))
                        .focus_visible(|button| button.opacity(1.0))
                        .when(
                            self.session_close_focus
                                .get(&id)
                                .is_some_and(|focus| focus.is_focused(window)),
                            |button| button.opacity(1.0),
                        )
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
                    ),
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
                .child(icon("restart", ICON_SM, c::FG_DIM()))
                .child("Retry"),
            );
        }
        result.into_any_element()
    }
    #[cfg(test)]
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
                "refresh",
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
            .on_prepaint({
                let bounds = self.menu_popup_bounds.clone();
                move |rect, _, cx| {
                    bounds.set(rect);
                    super::inline_html::exclude_native_html(Some(rect), cx);
                }
            })
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
    #[cfg(test)]
    fn project_tree(
        &self,
        project_filter: Option<usize>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let visible_sessions = self.visible_session_order(cx);
        let mut snapshot = self.project_navigation_snapshot(cx);
        if let Some(idx) = project_filter {
            snapshot.projects.retain(|p| p.idx == idx);
        }
        let mut body = div().flex().flex_col();
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
                    self.control_with_tooltip(
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
                    self.control_with_tooltip(
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
                        .child(super::status::label(
                            format!("worktrees-loading-{idx}"),
                            super::status::StatusKind::Working,
                            "Loading worktrees…",
                            ICON_SM,
                            cx,
                        )),
                );
            }
            for (worktree_position, worktree) in project.worktrees.iter().enumerate() {
                let path = worktree.path.clone();
                let worktree_name = sidebar_worktree_name(&worktree.name, worktree.is_main);
                let action_count = WORKTREE_LAUNCH_AGENTS.len() + (!worktree.is_main) as usize;
                let launch_width = WORKTREE_ACTION_W * action_count as f32;
                let selection = Selection::Worktree(idx, path.clone());
                let selected = self.selection == Some(selection.clone());
                let focused = self
                    .worktree_focus
                    .get(&path)
                    .is_some_and(|f| f.contains_focused(window, cx));
                let hovered = self.hovered_worktree.as_ref() == Some(&path);
                let diff_focused = self
                    .worktree_diff_focus
                    .get(&path)
                    .is_some_and(|focus| focus.is_focused(window));
                let show_launchers = (focused || hovered) && !diff_focused;
                let mut launches = div()
                    .absolute()
                    .right_0()
                    .top_0()
                    .w(rpx(if show_launchers { launch_width } else { 0.0 }))
                    .overflow_hidden()
                    .h(rpx(CHROME_CONTROL_H))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .opacity(if show_launchers { 1.0 } else { 0.0 });
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
                        self.control_with_tooltip(
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
                        self.control_with_tooltip(
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
                let diff = self
                    .runtime
                    .read(cx)
                    .tree
                    .read(cx)
                    .git_state(crate::paths::normalize_wt_path(&path))
                    .copied();
                let diff_label = match &diff {
                    Some(state) if state.dirty || state.added > 0 || state.removed > 0 => {
                        format!("Worktree changes: +{} −{}", state.added, state.removed)
                    }
                    Some(_) => "Worktree clean".into(),
                    None => "Git status unavailable".into(),
                };
                let diff_element = match &diff {
                    Some(state) if state.dirty || state.added > 0 || state.removed > 0 => self
                        .control(
                            SharedString::from(format!("worktree-diff-{path}")),
                            format!("{diff_label}; open changes in {path}"),
                            Action::OpenWorktreeDiff(path.clone()),
                            cx,
                        )
                        .debug_selector({
                            let path = path.clone();
                            move || {
                                if show_launchers {
                                    format!("worktree-diff-hidden-{path}")
                                } else {
                                    format!("worktree-diff-{path}")
                                }
                            }
                        })
                        .when_some(
                            self.worktree_diff_focus.get(&path),
                            gpui::InteractiveElement::track_focus,
                        )
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                        })
                        .w_auto()
                        .h_auto()
                        .flex_shrink_0()
                        .gap(rpx(SPACE_MD))
                        .pr(rpx((CONTROL_H - ICON_MD) / 2.0))
                        .text_size(rpx(HIERARCHY_META_TEXT))
                        .when(state.added > 0, |chip| {
                            chip.child(
                                div()
                                    .text_color(c::GREEN())
                                    .child(format!("+{}", state.added)),
                            )
                        })
                        .when(state.removed > 0, |chip| {
                            chip.child(
                                div()
                                    .text_color(c::RED())
                                    .child(format!("−{}", state.removed)),
                            )
                        })
                        .when(state.added == 0 && state.removed == 0, |chip| {
                            chip.child(div().text_color(c::GREEN()).child("Changes"))
                        })
                        .into_any_element(),
                    _ => div().into_any_element(),
                };
                group = group.child(
                    self.row(
                        format!("worktree-{path}"),
                        format!(
                            "{} worktree, branch {} · {diff_label}",
                            worktree_name, worktree.branch
                        ),
                        false,
                        Action::Select(selection),
                        cx,
                    )
                    .relative()
                    .group("worktree-row")
                    .tab_group()
                    .on_hover(cx.listener({
                        let path = path.clone();
                        move |this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.hovered_worktree = Some(path.clone());
                            } else if this.hovered_worktree.as_ref() == Some(&path) {
                                this.hovered_worktree = None;
                            }
                            cx.notify();
                        }
                    }))
                    .when_some(
                        self.worktree_focus.get(&path),
                        gpui::InteractiveElement::track_focus,
                    )
                    .h(rpx(WORKTREE_ROW_H))
                    .mt(rpx(if worktree_position == 0 {
                        SPACE_SM
                    } else {
                        WORKTREE_GROUP_GAP
                    }))
                    .pl(rpx(WORKTREE_LEADING_INSET))
                    .pr(rpx(HIERARCHY_INSET))
                    .gap(rpx(HIERARCHY_GAP))
                    .text_size(rpx(TEXT_BODY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(c::FG())
                    .child(
                        div()
                            .w(rpx(HIERARCHY_ICON_SLOT))
                            .h(rpx(WORKTREE_TITLE_LINE_H))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
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
                            .h(rpx(WORKTREE_TITLE_LINE_H))
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
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .child(worktree_name.to_owned()),
                                    ),
                            ),
                    )
                    .child(
                        // Launch controls replace the checkout metadata; focusing changes reveals its keyboard target.
                        div()
                            .id(SharedString::from(format!("worktree-actions-{path}")))
                            .debug_selector({
                                let path = path.clone();
                                move || format!("worktree-actions-{path}")
                            })
                            .relative()
                            .min_w(rpx(if show_launchers {
                                launch_width
                            } else {
                                HIERARCHY_TRAILING_W
                            }))
                            .h(rpx(CHROME_CONTROL_H))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_end()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .opacity(if show_launchers { 0.0 } else { 1.0 })
                                    .child(diff_element),
                            )
                            .child(launches),
                    ),
                );
                if worktree.sessions.is_empty() {
                    group = group.child(
                        div()
                            .id(SharedString::from(format!("worktree-empty-{path}")))
                            .pl(rpx(SESSION_LEADING_INSET
                                + SESSION_LAUNCHER_SIZE
                                + SPACE_SM))
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
                                .when(session_position == 0, |row| row.mt(rpx(SESSION_FIRST_GAP)))
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
                    .text_size(rpx(TEXT_MICRO))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(c::FG_DIM())
                    .child(
                        div()
                            .debug_selector(|| "session-list-section-heading".into())
                            .child(format!("{group} · {}", items.len())),
                    ),
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
                    .child("No sessions yet. Start one with +."),
            );
        }
        body.into_any_element()
    }
    fn terminals(&self, _window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let mut panel = div().flex().flex_col().flex_shrink_0().child(
            div()
                .h(rpx(HEAD_H))
                .px(rpx(SIDEBAR_CONTENT_INSET))
                .flex()
                .items_center()
                .gap(rpx(SPACE_LG))
                .text_color(c::FG_DIM())
                .child(crate::icons::action_icon("terminal", ICON_LG))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(rpx(TEXT_TITLE))
                        .text_color(c::FG())
                        .font_weight(gpui::FontWeight::NORMAL)
                        .debug_selector(|| "sidebar-terminals-heading".into())
                        .child("Terminals"),
                )
                .child(
                    self.control_with_tooltip(
                        "add-terminal",
                        "Add terminal",
                        Action::AddTerminal,
                        cx,
                    )
                    .child(icon("plus", ICON_SM, c::FG_DIM())),
                ),
        );
        for meta in self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .home_terminals()
            .to_vec()
        {
            if self.terminal_owners.get(&meta.id).copied().unwrap_or(1) != self.active_workspace {
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
            let title = terminal.update(cx, |terminal, _| {
                terminal
                    .title()
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| meta.label.clone())
            });
            panel = panel.child(
                self.row(
                    format!("home-{}", id.raw()),
                    title.clone(),
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
                .gap(rpx(SPACE_SM))
                .child(
                    div()
                        .w(rpx(SIDEBAR_ICON_SLOT))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .id(("home-icon", id.raw()))
                        .debug_selector(move || format!("home-icon-{}", id.raw()))
                        .child(icon("terminal", ICON_SM, c::FG_DIM())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .id(("home-title", id.raw()))
                        .debug_selector(move || format!("home-title-{}", id.raw()))
                        .truncate()
                        .child(single_line_session_title(&title)),
                )
                .child(
                    self.control_with_tooltip(
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
                                gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                                    .absolute()
                                    .inset_0(),
                            )
                        },
                    )
                    .child(icon("close", ICON_XS, c::FG_DIM())),
                ),
            );
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
        if self.mode != ViewMode::Grid || self.is_zen() || !self.navigation_available() {
            self.grid_session_drag = None;
        }
        self.window_active = window.is_window_active();
        self.sync_dirty |=
            self.registry_sync_revision != self.runtime.read(cx).registry.read(cx).revision();
        super::inline_html::exclude_native_html(None, cx);
        if self.sync_dirty {
            self.sync(window, cx);
        }
        if self.history.is_some() && self.history_selection != self.selection {
            self.history_hover = branch_history::HistoryHover::default();
            self.history = None;
            self.history_observer = None;
            self.history_selection = None;
            self.history_return_focus = None;
        }
        self.sync_age_timer(cx);
        self.sync_session_placement(cx);
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
        let global_rail = self.global_rail(window, cx);
        self.rendered_terminal_ids.clear();
        let content = self.render_content(window, cx);
        if self.rendered_terminal_ids != self.cached_terminal_ids {
            let registry = self.runtime.read(cx).registry.clone();
            registry.update(cx, |registry, cx| {
                registry.release_hidden_scenes(&self.rendered_terminal_ids, cx);
            });
            self.cached_terminal_ids
                .clone_from(&self.rendered_terminal_ids);
        }
        let navigation_close_confirmation = self.navigation_close_confirmation(window, cx);
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
            .bg(c::BG_RAIL())
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.divider_move(event, window, cx);
                this.track_project_flyout_pointer(event.position, window, cx);
                this.track_usage_pointer(event.position, window, cx);
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
                if event.keystroke.key == "escape" && this.usage_panel.is_some() {
                    this.dismiss_usage(!this.usage_hover_open, window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if event.keystroke.key == "escape" && this.branch_peek.is_some() {
                    this.dismiss_branch_peek(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape"
                    && this.checkout_panel.plan_peek.is_some()
                    && !this.confirmation_open()
                {
                    this.dismiss_plan_peek(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.checkout_panel.script_peek.is_some() {
                    this.dismiss_script_peek(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape"
                    && this.checkout_panel.peek.is_some()
                    && !this.confirmation_open()
                {
                    this.dismiss_agent_peek(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "tab" && this.header_tasks.open.is_some() {
                    this.focus_header_task_disclosure(window, cx);
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" && this.header_tasks.open.is_some() {
                    this.dismiss_header_tasks(true, window, cx);
                    cx.stop_propagation();
                    return;
                }
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
                if this.history.is_some() {
                    if event.keystroke.key == "tab" {
                        for _ in 0..64 {
                            if event.keystroke.modifiers.shift {
                                window.focus_prev(cx);
                            } else {
                                window.focus_next(cx);
                            }
                            if this.history_focus.contains_focused(window, cx) {
                                break;
                            }
                        }
                        if !this.history_focus.contains_focused(window, cx) {
                            this.history_focus.focus(window, cx);
                        }
                        cx.stop_propagation();
                    }
                    if event.keystroke.key == "escape" {
                        this.act(Action::CloseHistory, window, cx);
                        cx.stop_propagation();
                    }
                    return;
                }
                if this.review_is_open(cx) && event.keystroke.key == "escape" {
                    this.close_review(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }
                if this.description_is_open(cx) && event.keystroke.key == "escape" {
                    this.close_description(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
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
                    if this.checkout_confirmation_open() {
                        this.checkout_cancel_confirmation(window, cx);
                    } else {
                        this.act(Action::Cancel, window, cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .child(global_rail)
            .child(
                div()
                    .id("sidebar-canvas")
                    .debug_selector(|| "sidebar-canvas".into())
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .when(cfg!(target_os = "macos"), gpui::Styled::h_full)
                    .when(!cfg!(target_os = "macos"), |canvas| {
                        canvas.mt(rpx(APPBAR_TOTAL_H))
                    })
                    .bg(c::BG())
                    .rounded_tl(rpx(RADIUS_PANEL))
                    .overflow_hidden()
                    .child(content)
                    .when(self.history.is_some(), |canvas| {
                        canvas.child(gpui::deferred(self.render_history(window, cx)))
                    })
                    .when_some(super::native_plan::render_expanded(cx), |canvas, plan| {
                        canvas.child(gpui::deferred(plan))
                    })
                    .when(confirming && !self.project_decision, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .bg(c::QUIET_VEIL())
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                }),
                        )
                    }),
            )
            .when_some(self.render_usage_popup(window, cx), |root, popup| {
                root.child(popup)
            })
            .when_some(navigation_close_confirmation, |root, popup| {
                root.child(popup)
            })
            .when_some(self.render_branch_peek(window, cx), |root, peek| {
                root.child(peek)
            })
            .when_some(self.render_script_peek(window, cx), |root, peek| {
                root.child(peek)
            })
            .when_some(self.render_agent_peek(window, cx), |root, peek| {
                root.child(peek)
            })
            .when_some(self.render_plan_peek(window, cx), |root, peek| {
                root.child(peek)
            })
            .when(self.checkout_confirmation_open(), |root| {
                let source = self.checkout_confirmation_bounds();
                root.child(gpui::deferred(
                    div()
                        .absolute()
                        .inset_0()
                        .occlude()
                        .bg(c::QUIET_VEIL())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            gpui::anchored()
                                .position_mode(gpui::AnchoredPositionMode::Window)
                                .position(gpui::point(source.left(), source.bottom()))
                                .snap_to_window_with_margin(
                                    rpx(SPACE_LG).to_pixels(window.rem_size()),
                                )
                                .child(self.render_checkout_confirmation(window, cx)),
                        ),
                ))
            })
            .when_some(self.pending_worktree_removal.as_ref(), |root, removal| {
                let source = self.action_source_bounds.get();
                root.child(gpui::deferred(
                    div()
                        .absolute()
                        .inset_0()
                        .occlude()
                        .bg(c::QUIET_VEIL())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            gpui::anchored()
                                .position_mode(gpui::AnchoredPositionMode::Window)
                                .position(gpui::point(source.left(), source.bottom()))
                                .snap_to_window_with_margin(
                                    rpx(SPACE_LG).to_pixels(window.rem_size()),
                                )
                                .child(self.render_worktree_removal(removal, cx)),
                        ),
                ))
            })
            .when(self.diff_viewer.is_some(), |d| {
                d.child(gpui::deferred(self.render_diff(window, cx)))
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

/// Keep hard line breaks from escaping single-line sidebar title slots.
/// The original title remains available to accessibility labels.
fn single_line_session_title(title: &str) -> String {
    if title.contains(['\n', '\r', '\u{0085}', '\u{2028}', '\u{2029}']) {
        title.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        title.to_owned()
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
    mod activity;
    mod checkout;
    mod project_lifecycle;
    mod session_navigation;
    mod sidebar_interaction;

    use super::*;
    use std::time::{Duration, Instant};

    struct WorkspaceHeaderFixture {
        sidebar: Entity<Sidebar>,
        _observers: Vec<gpui::Subscription>,
    }
    impl WorkspaceHeaderFixture {
        fn new(sidebar: Entity<Sidebar>, cx: &mut Context<Self>) -> Self {
            let observers = vec![
                cx.observe(&sidebar, |_, _, cx| cx.notify()),
                cx.observe_global::<SettingsState>(|_, cx| cx.notify()),
            ];
            Self {
                sidebar,
                _observers: observers,
            }
        }
    }
    impl Render for WorkspaceHeaderFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            // Shell owns this shared header above the sidebar. Mount its actual
            // control here because this component fixture does not mount Shell.
            let header = self.sidebar.update(cx, |sidebar, cx| {
                let width = sidebar.rail_width(window, cx);
                (width > 0.).then(|| {
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .w(rpx(width))
                        .h(rpx(APPBAR_TOTAL_H))
                        .px(rpx(SIDEBAR_CONTENT_INSET))
                        .child(sidebar.header_workspace_control(cx))
                })
            });
            div()
                .size_full()
                .relative()
                .child(self.sidebar.clone())
                .children(header)
        }
    }

    /// Mount retained tree/list recipes explicitly; the app's single sidebar no
    /// longer owns these larger project and session-management compositions.
    struct NavigationComponentFixture {
        sidebar: Entity<Sidebar>,
        project_recipe: bool,
    }
    impl Render for NavigationComponentFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let owner = self.sidebar.clone();
            let project_recipe = self.project_recipe;
            self.sidebar.update(cx, |sidebar, cx| {
                super::super::inline_html::exclude_native_html(None, cx);
                sidebar.sync(window, cx);
                sidebar.sync_age_timer(cx);
                let actual_mode = sidebar.mode;
                if project_recipe {
                    sidebar.mode = ViewMode::Project;
                }
                let list = sidebar.mode == ViewMode::List;
                let body = if list {
                    sidebar.list(window, cx)
                } else {
                    sidebar.project_tree(None, window, cx)
                };
                let hide_component = (sidebar.project_panel.is_some()
                    || sidebar.pending_new_worktree.is_some())
                    && Sidebar::logical_window_width(window) < 640.0;
                let component = div()
                    .id("sidebar-rail")
                    .debug_selector(|| "sidebar-rail".into())
                    .w(rpx(sidebar.context_width(window, cx)))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .bg(c::BG_RAIL())
                    .when(list, |rail| {
                        rail.child(
                            div()
                                .px(rpx(SIDEBAR_CONTENT_INSET))
                                .child(sidebar.navigation_heading(window, cx)),
                        )
                    })
                    .child(
                        div()
                            .id("component-navigation-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p(rpx(SPACE_LG))
                            .child(body),
                    );
                // Invoke the retained canvas owner without also mounting the new
                // Projects destination's copy of the tree under test.
                sidebar.mode = actual_mode;
                let destination = sidebar.destination;
                let dashboard = sidebar.dashboard_open;
                sidebar.destination = global_navigation::Destination::Sessions;
                sidebar.dashboard_open = false;
                let content = if sidebar.selection.is_none()
                    && sidebar.project_panel.is_none()
                    && sidebar.project_setup.is_none()
                    && sidebar.pending_new_worktree.is_none()
                {
                    div().into_any_element()
                } else {
                    sidebar.render_content(window, cx)
                };
                sidebar.destination = destination;
                sidebar.dashboard_open = dashboard;
                div()
                    .size_full()
                    .flex()
                    .track_focus(&sidebar.focus)
                    .when(!hide_component, |root| root.child(component))
                    .child(
                        div()
                            .id("sidebar-canvas")
                            .debug_selector(|| "sidebar-canvas".into())
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(content),
                    )
                    .on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
                        owner.update(cx, |sidebar, cx| {
                            if sidebar.menu_opened_by_hover
                                && !sidebar.menu_trigger_bounds.get().contains(&event.position)
                                && !sidebar.menu_popup_bounds.get().contains(&event.position)
                            {
                                sidebar.close_menu(window, cx);
                            }
                        });
                    })
                    .capture_key_down(cx.listener(
                        |sidebar, event: &gpui::KeyDownEvent, window, cx| {
                            if event.keystroke.key == "escape"
                                && !sidebar.project_decision
                                && (sidebar.pending_new_worktree.is_some()
                                    || sidebar.menu.is_some()
                                    || sidebar.confirmation_open())
                            {
                                sidebar.act(Action::Cancel, window, cx);
                                cx.stop_propagation();
                            }
                        },
                    ))
                    .on_key_down(super::super::shell::traverse_unhandled_tab)
            })
        }
    }

    fn add_component_with_mode(
        cx: &mut gpui::TestAppContext,
        project_recipe: bool,
        build: impl FnOnce(&mut Window, &mut Context<Sidebar>) -> Sidebar,
    ) -> (Entity<Sidebar>, &mut gpui::VisualTestContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (fixture, cx) = cx.add_window_view(|window, cx| {
            let sidebar = cx.new(|cx| build(window, cx));
            cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
            NavigationComponentFixture {
                sidebar,
                project_recipe,
            }
        });
        let sidebar = cx.update(|_, cx| fixture.read(cx).sidebar.clone());
        (sidebar, cx)
    }

    fn add_navigation_component(
        cx: &mut gpui::TestAppContext,
        build: impl FnOnce(&mut Window, &mut Context<Sidebar>) -> Sidebar,
    ) -> (Entity<Sidebar>, &mut gpui::VisualTestContext) {
        add_component_with_mode(cx, false, build)
    }

    fn add_project_component(
        cx: &mut gpui::TestAppContext,
        build: impl FnOnce(&mut Window, &mut Context<Sidebar>) -> Sidebar,
    ) -> (Entity<Sidebar>, &mut gpui::VisualTestContext) {
        add_component_with_mode(cx, true, |window, cx| {
            let mut sidebar = build(window, cx);
            sidebar.sync(window, cx);
            sidebar.mode = ViewMode::Project;
            sidebar.last_mode = ViewMode::Project;
            sidebar
        })
    }

    struct NavigationSelectionFixture {
        sidebar: Entity<Sidebar>,
        text: Entity<gpui_component::text::TextViewState>,
    }

    impl Render for NavigationSelectionFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let control = self.sidebar.update(cx, |sidebar, cx| {
                sidebar
                    .control(
                        "selection-session",
                        "Session",
                        Action::Select(Selection::Session(SessionId::from_raw(1))),
                        cx,
                    )
                    .debug_selector(|| "selection-session".into())
                    .w(gpui::px(100.))
                    .h(gpui::px(40.))
            });
            div()
                .size_full()
                .child(control)
                .child(
                    div()
                        .id("consumed-navigation")
                        .debug_selector(|| "consumed-navigation".into())
                        .h(gpui::px(40.))
                        .on_click(|_, _, cx| cx.stop_propagation()),
                )
                .child(
                    div()
                        .debug_selector(|| "selection-transcript".into())
                        .h(gpui::px(80.))
                        .child(gpui_component::text::TextView::new(&self.text).selectable(true)),
                )
        }
    }

    fn navigation_selection_fixture(cx: &mut gpui::TestAppContext) -> &mut gpui::VisualTestContext {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
            let text = cx.new(|cx| {
                gpui_component::text::TextViewState::markdown(
                    "Transcript must remain unselected",
                    cx,
                )
            });
            let fixture = cx.new(|_| NavigationSelectionFixture { sidebar, text });
            gpui_component::Root::new(fixture, window, cx)
        });
        draw(cx);
        cx
    }

    fn assert_empty_cross_project_launch(
        cx: &mut gpui::TestAppContext,
        collapsed: bool,
        projects: bool,
        list: bool,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            let mut store = grove_core::storage::Store {
                default_landing_page: if list {
                    grove_core::storage::LandingPage::Sessions
                } else {
                    grove_core::storage::LandingPage::Projects
                },
                ..Default::default()
            };
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
        let (sidebar, cx) = add_navigation_component(cx, |window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        let requests = std::rc::Rc::new(std::cell::Cell::new(0));
        cx.update(|window, cx| {
            let requests = requests.clone();
            cx.subscribe(&sidebar, move |_, event, _| {
                if matches!(event, SidebarEvent::NewMultiProjectSession) {
                    requests.set(requests.get() + 1);
                }
            })
            .detach();
            sidebar.update(cx, |sidebar, cx| {
                if list {
                    sidebar.mode = ViewMode::List;
                }
                assert!(sidebar.multi_project_sessions(cx).is_empty());
                assert_eq!(sidebar.snapshot.projects.is_empty(), !projects);
                if collapsed {
                    sidebar.toggle_sidebar(window, cx);
                }
            });
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        assert!(cx.debug_bounds("multi-project-sessions").is_none());
        assert!(cx.debug_bounds("multi-project-anchor").is_none());
        if list && !collapsed {
            let launch = cx
                .debug_bounds("projects-add")
                .expect("session list launcher");
            cx.simulate_click(launch.center(), gpui::Modifiers::default());
            draw(cx);
            assert_eq!(requests.get(), 1);
        } else {
            assert_eq!(requests.get(), 0);
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
                sidebar
                    .scroll
                    .set_offset(gpui::point(gpui::px(0.0), gpui::px(-40.0)));
            });
            // A -> B retains the explicitly selected project/list organization.
            cx.global_mut::<SettingsState>().store.workspaces.select(2);
            sidebar.update(cx, |sidebar, cx| {
                sidebar.sync(window, cx);
                assert!(sidebar.dashboard_open);
                assert!(sidebar.pending_canvas_focus.is_none());
                assert!(sidebar.pending_grid_workspace_focus.is_none());
                assert_eq!(sidebar.mode, sidebar_mode);
                assert_eq!(sidebar.last_mode, sidebar_mode);
                assert_eq!(sidebar.selection, Some(Selection::Project(1)));
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
                assert!(sidebar.dashboard_open);
                assert!(sidebar.pending_canvas_focus.is_none());
                assert!(sidebar.pending_grid_workspace_focus.is_none());
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
                assert!(sidebar.dashboard_open);
                assert!(sidebar.pending_canvas_focus.is_none());
                assert!(sidebar.pending_grid_workspace_focus.is_none());
                assert_eq!(sidebar.mode, ViewMode::Grid);
                // Grid selects a canvas session; this fixture has no sessions.
                assert_eq!(sidebar.selection, None);
                assert_eq!(sidebar.scroll.offset().y, gpui::px(-40.0));
                assert!(sidebar.is_collapsed(cx));
                sidebar.toggle_grid(window, cx);
                assert_eq!(sidebar.mode, sidebar_mode);
                sidebar.select(Selection::Project(0), cx);
                sidebar.sync(window, cx);
                assert!(!sidebar.dashboard_open);
            });
            // Revisiting C must honor the latest explicit exit from grid.
            cx.global_mut::<SettingsState>().store.workspaces.select(3);
            sidebar.update(cx, |sidebar, cx| {
                sidebar.sync(window, cx);
                assert!(sidebar.dashboard_open);
                assert!(sidebar.pending_canvas_focus.is_none());
                assert!(sidebar.pending_grid_workspace_focus.is_none());
                assert_eq!(sidebar.mode, ViewMode::Project);
                assert_eq!(sidebar.last_mode, ViewMode::Project);
                assert_eq!(sidebar.selection, Some(Selection::Project(2)));
                assert!(sidebar.is_collapsed(cx));
            });
            cx.global_mut::<SettingsState>().store.workspaces.select(2);
            sidebar.update(cx, |sidebar, cx| {
                sidebar.sync(window, cx);
                assert!(sidebar.dashboard_open);
                assert!(sidebar.pending_canvas_focus.is_none());
                assert!(sidebar.pending_grid_workspace_focus.is_none());
                assert_eq!(sidebar.mode, ViewMode::Project);
                assert_eq!(sidebar.selection, Some(Selection::Project(1)));
                assert_eq!(sidebar.scroll.offset().y, gpui::px(-80.0));
            });
        });
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

    /// Real native text shaping and Metal rendering without opening or installing the app.
    #[test]
    #[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn update_header_native_snapshots() {
        use gpui::AppContext as _;
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            std::sync::Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            c::sync_component_theme(cx);
            cx.set_reduce_motion(true);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        for rem in [16.0, 20.0] {
            for (label, width, collapsed) in [
                ("narrow", 220.0, false),
                ("default", 260.0, false),
                ("wide", 360.0, false),
                ("collapsed", 260.0, true),
            ] {
                cx.update(|cx| {
                    gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                    let mut store = grove_core::storage::Store {
                        sidebar_width: Some(width),
                        sidebar_collapsed: collapsed,
                        ..Default::default()
                    };
                    let active = store.workspaces.active;
                    store
                        .workspaces
                        .rename(active, "A very long workspace name for the update action")
                        .unwrap();
                    cx.set_global(SettingsState::new(store));
                });
                let window = cx.open_window(gpui::size(gpui::px(900.0), gpui::px(480.0)), |window, cx| {
                    let runtime = cx.new(Runtime::new);
                    runtime.read(cx).upgrade.clone().update(cx, |upgrade, cx| {
                        upgrade.offer_for_test(grove_core::upgrade::InstallMethod::Dmg, grove_core::upgrade::Release {
                            version: semver::Version::new(99, 0, 0), tag: "v99.0.0".into(),
                            html_url: "https://github.com/gitfudge0/grove/releases/tag/v99.0.0".into(), body: String::new(),
                            dmg_url: Some("https://github.com/gitfudge0/grove/releases/download/v99.0.0/Grove.dmg".into()),
                            dmg_sha256_url: None, target_commitish: "main".into(),
                        }, cx);
                    });
                    let picker = cx.new(|cx| super::super::workspace_manager::WorkspaceManager::new(window, cx));
                    let sidebar = cx.new(|cx| {
                        let mut sidebar = Sidebar::new(runtime, window, cx);
                        sidebar.set_workspace_selector(picker);
                        sidebar
                    });
                    let fixture = cx.new(|cx| {
                        cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
                        WorkspaceHeaderFixture::new(sidebar, cx)
                    });
                    cx.new(|cx| gpui_component::Root::new(fixture, window, cx).bordered(false).bg(c::BG()))
                }).unwrap();
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    let _ = window.draw(cx);
                    assert_eq!(window.rem_size(), gpui::px(rem));
                })
                .unwrap();
                let path = format!("/tmp/grove-update-header-{label}-rem{rem:.0}.png");
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Update header native fixture: {path}");
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }

    /// Native renderer fixtures exercise Codex and Claude card identities without live providers.
    #[test]
    #[ignore = "requires macOS Metal renderer; writes fixture screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn session_list_native_snapshots() {
        use gpui::AppContext as _;
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            std::sync::Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        for light in [false, true] {
            for rem in [16.0, 20.0] {
                for width in [220.0, 260.0, 340.0] {
                    cx.update(|cx| {
                        cx.set_global(SettingsState::new(grove_core::storage::Store {
                            sidebar_width: Some(width),
                            projects: ["grove", "a-very-long-project-name"]
                                .into_iter()
                                .map(|name| grove_core::storage::Project {
                                    name: name.into(),
                                    path: format!("/tmp/{name}"),
                                    scripts: grove_core::storage::ProjectScripts::default(),
                                    archived: false,
                                    worktree_dir: None,
                                })
                                .collect(),
                            ..Default::default()
                        }));
                        grove_core::theme::set(
                            grove_core::theme::by_name(if light {
                                "grove-light"
                            } else {
                                "grove-dark"
                            })
                            .unwrap(),
                        );
                        c::sync_component_theme(cx);
                        gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                    });
                    let mut fixture_sidebar = None;
                    let window = cx.open_window(gpui::size(gpui::px(1000.0), gpui::px(620.0)), |window, cx| {
                        let runtime = cx.new(Runtime::new);
                        let registry = runtime.read(cx).registry.clone();
                        runtime.read(cx).tree.clone().update(cx, |tree, _| {
                            for (idx, name) in ["grove", "a-very-long-project-name"].into_iter().enumerate() {
                                tree.set_active_worktrees(idx, vec![grove_core::git::Worktree {
                                    path: format!("/tmp/{name}"), branch: "main".into(), mtime: None, is_main: true,
                                }]);
                            }
                        });
                        let mut ids = Vec::new();
                        for (agent, project, title, state) in [
                            (Agent::Codex, "grove", "T3 Code Features to Add", ActivityState::Done),
                            (Agent::Claude, "grove", "Fix sidebar resize", ActivityState::Working),
                            (Agent::Codex, "a-very-long-project-name", "Repair very long session navigation titles without hiding meaningful actions", ActivityState::WaitingForInput),
                        ] {
                            let id = registry.update(cx, |registry, _| {
                                let id = registry.insert_meta(project.into(), format!("/tmp/{project}"), agent);
                                registry.set_restored_title(id, Some(title.into()));
                                id
                            });
                            runtime.read(cx).activity.clone().update(cx, |activity, _| activity.set_state_for_test(id, state));
                            ids.push(id);
                        }
                        let saved: grove_core::codex::SavedSession = serde_json::from_value(serde_json::json!({
                            "key":"session-card-fixture", "project":"grove", "cwd":"/tmp/grove", "label":"codex 1",
                            "title":"T3 Code Features to Add", "roots":[], "thread_id":null,
                            "items":[{"type":"plan", "plan":[{"step":"Done", "status":"completed"},{"step":"Done too", "status":"completed"}]}]
                        })).unwrap();
                        let (handle, _actions) = grove_core::codex::Handle::channel();
                        let (_updates, receive) = std::sync::mpsc::channel();
                        let session = cx.new(|cx| {
                            let mut state = grove_core::codex::Snapshot::new(saved);
                            state.status = grove_core::codex::Status::Done;
                            crate::entities::codex_session::CodexSession::from_connection(state,
                                grove_core::codex::Connection { handle: handle.clone(), updates: receive.into() }, cx)
                        });
                        registry.update(cx, |registry, _| registry.attach_codex(ids[0], session, handle));
                        let sidebar = cx.new(|cx| {
                            let mut sidebar = Sidebar::new(runtime, window, cx);
                            sidebar.mode = ViewMode::List;
                            sidebar.selection = Some(Selection::Session(ids[0]));
                            sidebar.snapshot = TreeSnapshot { projects: vec![crate::entities::workspace_state::SnapshotProject {
                                idx: 0, name: "grove".into(), sessions: ids.clone(),
                                worktrees: vec![crate::entities::workspace_state::SnapshotWorktree {
                                    path: "/tmp/grove".into(), name: "grove".into(), branch: "main".into(), sessions: ids.clone(), ..Default::default()
                                }], ..Default::default()
                            }], total_projects: 1 };
                            sidebar
                        });
                        fixture_sidebar = Some(sidebar.clone());
                        cx.new(|cx| gpui_component::Root::new(sidebar, window, cx).bordered(false).bg(c::BG()))
                    }).unwrap();
                    cx.run_until_parked();
                    cx.update_window(window.into(), |_, window, cx| {
                        // Seed after background refresh so the captured native cards
                        // exercise both short/long branch adjacency and semantic Git counts.
                        fixture_sidebar.as_ref().unwrap().update(cx, |sidebar, cx| {
                            let projects = [
                                (
                                    0,
                                    "grove",
                                    "main",
                                    vec![SessionId::from_raw(1), SessionId::from_raw(2)],
                                ),
                                (
                                    1,
                                    "a-very-long-project-name",
                                    "feature/session-cards-with-a-deliberately-long-branch-name",
                                    vec![SessionId::from_raw(3)],
                                ),
                            ];
                            sidebar.runtime.read(cx).tree.clone().update(cx, |tree, _| {
                                tree.apply_git_poll(
                                    projects
                                        .iter()
                                        .map(|(_, name, _, _)| {
                                            (
                                                format!("/tmp/{name}"),
                                                grove_core::git::WorktreeGitState {
                                                    dirty: true,
                                                    added: 23,
                                                    removed: 3,
                                                    ..Default::default()
                                                },
                                            )
                                        })
                                        .collect(),
                                    &[],
                                );
                            });
                            sidebar.snapshot = TreeSnapshot {
                                projects: projects
                                    .into_iter()
                                    .map(|(idx, name, branch, sessions)| crate::entities::workspace_state::SnapshotProject {
                                        idx,
                                        name: name.into(),
                                        sessions: sessions.clone(),
                                        worktrees: vec![crate::entities::workspace_state::SnapshotWorktree {
                                            path: format!("/tmp/{name}"),
                                            name: name.into(),
                                            branch: branch.into(),
                                            sessions,
                                            ..Default::default()
                                        }],
                                        ..Default::default()
                                    })
                                    .collect(),
                                total_projects: 2,
                            };
                            cx.notify();
                        });
                        let _ = window.draw(cx);
                    })
                    .unwrap();
                    cx.update(|cx| {
                        let sidebar = fixture_sidebar.as_ref().unwrap().read(cx);
                        for id in [1, 2, 3] {
                            let bounds =
                                sidebar.session_close_bounds[&SessionId::from_raw(id)].get();
                            assert!(
                                bounds.size.width > gpui::px(0.0)
                                    && bounds.size.height > gpui::px(0.0),
                                "session {id} must actually render before capture"
                            );
                        }
                    });
                    let path = format!(
                        "/tmp/grove-session-list-{}-{width:.0}-rem{rem:.0}.png",
                        if light { "light" } else { "dark" }
                    );
                    cx.capture_screenshot(window.into())
                        .unwrap()
                        .save(&path)
                        .unwrap();
                    eprintln!("Native session-list fixture: {path}");
                    for focused in [false, true] {
                        cx.update_window(window.into(), |_, window, cx| {
                            fixture_sidebar.as_ref().unwrap().update(cx, |sidebar, cx| {
                                sidebar.mode = ViewMode::Project;
                                if focused {
                                    sidebar.session_close_focus[&SessionId::from_raw(1)]
                                        .focus(window, cx);
                                }
                                cx.notify();
                            });
                            let _ = window.draw(cx);
                        })
                        .unwrap();
                        let path = format!(
                            "/tmp/grove-session-tree-{}-{width:.0}-rem{rem:.0}-focus{focused}.png",
                            if light { "light" } else { "dark" }
                        );
                        cx.capture_screenshot(window.into())
                            .unwrap()
                            .save(&path)
                            .unwrap();
                        eprintln!("Native session-tree fixture: {path}");
                    }
                    cx.update_window(window.into(), |_, window, _| window.remove_window())
                        .unwrap();
                }
            }
        }
    }

    fn assert_session_list_cards(cx: &mut gpui::TestAppContext, list_only: bool) {
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
        let (sidebar, cx) = add_navigation_component(cx, |window, cx| {
            let runtime = cx.new(Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            let ids = [Agent::Codex, Agent::Claude]
                .into_iter()
                .map(|agent| {
                    registry.update(cx, |registry, _| {
                        if list_only && agent == Agent::Claude {
                            registry.insert_meta_with_context(
                                "demo".into(),
                                path.clone(),
                                agent,
                                vec![grove_core::session_meta::ContextRoot {
                                    project: "other".into(),
                                    wt_path: "/other".into(),
                                }],
                                None,
                            )
                        } else {
                            registry.insert_meta("demo".into(), path.clone(), agent)
                        }
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
                assert_eq!(
                    cx.debug_bounds("sidebar-heading-label").unwrap().left(),
                    card.left()
                );
                assert_eq!(
                    cx.debug_bounds("session-list-section-heading")
                        .unwrap()
                        .left(),
                    card.left()
                );
                assert_eq!(
                    cx.debug_bounds(if card_id == "session-1" {
                        "session-list-title-1"
                    } else {
                        "session-list-title-2"
                    })
                    .unwrap()
                    .left(),
                    card.left() + gpui::px(SPACE_LG)
                );
                assert!(card.size.height >= gpui::px(SESSION_LIST_CARD_H));
                assert!(close.left() >= card.left() && close.right() <= card.right());
                assert!(close.top() >= card.top() && close.bottom() <= card.bottom());
                assert!(cx.debug_bounds("diff-chip-open-1").is_none());
            }
            for (title_id, project_id, worktree_id, status_id, done_id) in [
                (
                    "session-list-title-1",
                    "session-list-context-1",
                    "session-list-worktree-1",
                    "session-list-status-1",
                    "close-session-1",
                ),
                (
                    "session-list-title-2",
                    "session-list-context-2",
                    "session-list-worktree-2",
                    "session-list-status-2",
                    "close-session-2",
                ),
            ] {
                let title = cx.debug_bounds(title_id).unwrap();
                let project = cx.debug_bounds(project_id).unwrap();
                let worktree = cx.debug_bounds(worktree_id).unwrap();
                let status = cx.debug_bounds(status_id).unwrap();
                let done = cx.debug_bounds(done_id).unwrap();
                assert!(project.bottom() <= title.top());
                assert!(worktree.bottom() <= title.top());
                assert!(worktree.left() >= project.right());
                let provider = cx
                    .debug_bounds(if project_id == "session-list-context-1" {
                        "session-list-agent-1"
                    } else {
                        "session-list-agent-2"
                    })
                    .unwrap();
                assert!(provider.left() >= worktree.right());
                assert_eq!(provider.right(), title.right());
                assert!(provider.bottom() <= title.top());
                assert!(status.top() >= title.bottom());
                assert!(status.right() <= done.left());
                assert!(done.right() <= title.right());
                assert!(cx.debug_bounds("session-number-1").is_none());
                cx.simulate_mouse_move(title.center(), None, gpui::Modifiers::default());
                draw(cx);
                assert_eq!(cx.debug_bounds(title_id).unwrap(), title);
                assert_eq!(cx.debug_bounds(status_id).unwrap(), status);
            }
            assert_eq!(
                sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
                Some(Selection::Session(SessionId::from_raw(2)))
            );
        }
        if list_only {
            sidebar.read_with(cx, |sidebar, cx| {
                assert_eq!(
                    sidebar.visible_session_order(cx),
                    vec![SessionId::from_raw(2), SessionId::from_raw(1)]
                );
                let grouped: Vec<_> = sidebar
                    .list_session_groups(cx)
                    .into_iter()
                    .flat_map(|(_, ids)| ids)
                    .collect();
                assert_eq!(
                    grouped,
                    vec![SessionId::from_raw(2), SessionId::from_raw(1)]
                );
                assert_eq!(sidebar.multi_project_sessions(cx).len(), 1);
            });
        }
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.age_timer.is_some()));
        let before = sidebar.read_with(cx, |sidebar, _| sidebar.age_ticks);
        cx.executor().advance_clock(SESSION_AGE_REFRESH);
        draw(cx);
        assert_eq!(
            sidebar.read_with(cx, |sidebar, _| sidebar.age_ticks),
            before + 1
        );
        if list_only {
            for (id, selector) in [(1, "close-session-1"), (2, "close-session-2")] {
                let done = cx.debug_bounds(selector).unwrap();
                cx.simulate_mouse_move(done.center(), None, gpui::Modifiers::default());
                draw(cx);
                cx.simulate_click(done.center(), gpui::Modifiers::default());
                draw(cx);
                sidebar.read_with(cx, |sidebar, cx| {
                    assert!(sidebar.pending_close.is_none());
                    assert!(sidebar
                        .runtime
                        .read(cx)
                        .registry
                        .read(cx)
                        .meta(SessionId::from_raw(id))
                        .is_none());
                    if id == 1 {
                        assert_eq!(
                            sidebar.selection,
                            Some(Selection::Session(SessionId::from_raw(2)))
                        );
                        assert!(sidebar
                            .runtime
                            .read(cx)
                            .registry
                            .read(cx)
                            .meta(SessionId::from_raw(2))
                            .is_some());
                    }
                });
            }
            return;
        }
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
        assert_eq!(worktree.left() - project.left(), gpui::px(SPACE_2XL));
        let status = cx.debug_bounds("session-tree-status-1").unwrap();
        assert!(cx.debug_bounds("session-tree-title-1").unwrap().right() <= status.left());
        assert!(first.right() <= cx.debug_bounds("session-tree-title-1").unwrap().left());
        assert_eq!(first.left(), second.left());
        // Expanded session items omit checkout metadata; only the worktree owns changes.
        assert!(cx.debug_bounds("diff-chip-open-1").is_none());
        assert!(cx.debug_bounds("diff-chip-open-2").is_none());
        assert!(cx.debug_bounds("session-tree-diff-1").is_none());
        assert!(cx.debug_bounds("session-tree-diff-2").is_none());
        let worktree_diff: &'static str =
            Box::leak(format!("worktree-diff-{path}").into_boxed_str());
        assert!(cx.debug_bounds(worktree_diff).is_some());
        assert!(cx.debug_bounds("session-number-1").is_none());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.toggle_tree_list(window, cx));
        });
        draw(cx);
        // Metadata clicks bubble to the session card for both provider identities.
        for (id, provider) in [(1, "Codex"), (2, "Claude")] {
            let counts = cx
                .debug_bounds(if id == 1 {
                    "session-list-worktree-1"
                } else {
                    "session-list-worktree-2"
                })
                .unwrap();
            cx.simulate_click(counts.center(), gpui::Modifiers::default());
            draw(cx);
            assert_eq!(
                sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone()),
                Some(Selection::Session(SessionId::from_raw(id))),
                "{provider} worktree metadata selects its owning session"
            );
            assert!(
                sidebar.read_with(cx, |sidebar, _| sidebar.diff_viewer.is_none()),
                "{provider} metadata must not open a diff"
            );
        }
    }

    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    #[cfg(unix)]
    #[gpui::test]
    fn codex_launch_action_creates_ready_native_chat_and_keeps_composer_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        if std::env::var_os("GROVE_NATIVE_LAUNCH_TEST_CHILD").is_none() {
            use std::os::unix::fs::PermissionsExt;
            let root = std::env::temp_dir().join(format!(
                "grove-native-launch-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs_err::create_dir_all(root.join("bin")).unwrap();
            fs_err::create_dir_all(root.join("config")).unwrap();
            let provider = root.join("bin/codex");
            fs_err::write(
                &provider,
                r#"#!/usr/bin/python3
import json,sys
from pathlib import Path
Path(__file__).resolve().parent.parent.joinpath("provider-invocation").write_text(str(Path(__file__).resolve()))
for line in sys.stdin:
    request=json.loads(line)
    method=request.get("method")
    ident=request.get("id")
    result=None
    if method=="initialize": result={"userAgent":"fixture"}
    elif method=="config/read": result={"config":{}}
    elif method in ("thread/start","thread/resume"):
        result={"thread":{"id":"fixture-chat","turns":[]}}
    if result is not None:
        print(json.dumps({"id":ident,"result":result}),flush=True)
"#,
            )
            .unwrap();
            fs_err::set_permissions(&provider, std::fs::Permissions::from_mode(0o755)).unwrap();
            let shell = root.join("bin/fixture-shell");
            fs_err::write(
                &shell,
                "#!/bin/sh\nif [ \"$1\" = \"-lic\" ]; then\n  shift 3\n  exec \"$@\"\nfi\nexec /bin/sh \"$@\"\n",
            )
            .unwrap();
            fs_err::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
            let path = std::env::join_paths(std::iter::once(root.join("bin")).chain(
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            ))
            .unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "views::sidebar::tests::codex_launch_action_creates_ready_native_chat_and_keeps_composer_focus", "--nocapture"])
                .env("GROVE_NATIVE_LAUNCH_TEST_CHILD", &root)
                .env("GROVE_CONFIG_DIR", root.join("config"))
                .env("GROVE_TEST_CONFIG_DIR", root.join("config"))
                .env("SHELL", shell)
                .env("PATH", path)
                .output().unwrap();
            let _ = fs_err::remove_dir_all(&root);
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let path = std::env::var("GROVE_NATIVE_LAUNCH_TEST_CHILD").unwrap();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "Native launch".into(),
                    path: path.clone(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                tmux_enabled: Some(true),
                native_agents_enabled: true,
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
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::Launch(0, path.clone(), Agent::Codex), window, cx);
                assert!(sidebar.content_error.is_none());
                assert_eq!(sidebar.runtime.read(cx).registry.read(cx).len(), 1);
            });
        });
        // Starting a real fixture subprocess can take longer on a busy CI runner.
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            cx.executor().advance_clock(Duration::from_millis(50));
            draw(cx);
            let (status, error) = sidebar.read_with(cx, |sidebar, cx| {
                let registry = sidebar.runtime.read(cx).registry.read(cx);
                let id = registry.all()[0].id;
                assert!(
                    registry.session(id).is_none(),
                    "Codex must not attach a PTY"
                );
                let state = &registry.codex(id).unwrap().read(cx).state;
                (state.status, state.error.clone())
            });
            if status == grove_core::codex::Status::Idle {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Native launch did not become ready: status={status:?}, error={error:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            fs_err::read_to_string(std::path::Path::new(&path).join("provider-invocation"))
                .unwrap(),
            std::path::Path::new(&path)
                .join("bin/codex")
                .canonicalize()
                .unwrap()
                .to_string_lossy(),
            "native launch must execute the isolated provider fixture"
        );
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let id = sidebar.runtime.read(cx).registry.read(cx).all()[0].id;
                let view = sidebar
                    .codex_views
                    .get(&id)
                    .expect("native conversation rendered")
                    .clone();
                assert!(
                    view.focus_handle(cx).contains_focused(window, cx),
                    "new native chat must focus its composer"
                );
                sidebar.focus_active_session_after_palette(window, cx);
                assert!(view.focus_handle(cx).contains_focused(window, cx));
                assert!(sidebar.pending_canvas_focus.is_none());
                assert!(sidebar.pending_grid_workspace_focus.is_none());
                sidebar.focus.focus(window, cx);
                sidebar.navigate_to_session(id, window, cx);
                assert!(view.focus_handle(cx).contains_focused(window, cx));
                sidebar.focus.focus(window, cx);
                sidebar.act(Action::Select(Selection::Session(id)), window, cx);
                assert!(view.focus_handle(cx).contains_focused(window, cx));
                sidebar.mode = ViewMode::Grid;
                sidebar.focus.focus(window, cx);
                sidebar.act(Action::Select(Selection::Session(id)), window, cx);
                assert!(view.focus_handle(cx).contains_focused(window, cx));
                sidebar.focus_active_session_after_palette(window, cx);
                assert!(view.focus_handle(cx).contains_focused(window, cx));
                cx.global_mut::<SettingsState>().store.native_agents_enabled = false;
                sidebar.act(Action::Launch(0, path.clone(), Agent::Codex), window, cx);
                assert!(sidebar.content_error.is_none());
                let fallback = {
                    let registry = sidebar.runtime.read(cx).registry.read(cx);
                    assert_eq!(registry.len(), 2);
                    assert!(
                        registry.codex(id).is_some(),
                        "existing native chat stays open"
                    );
                    let fallback = registry.all().iter().find(|meta| meta.id != id).unwrap().id;
                    assert!(registry.codex(fallback).is_none());
                    assert!(
                        registry.session(fallback).is_some(),
                        "disabled native agents use a PTY"
                    );
                    fallback
                };
                sidebar
                    .runtime
                    .update(cx, |runtime, cx| runtime.kill_session(fallback, cx));
                let stopped = sidebar
                    .runtime
                    .read(cx)
                    .registry
                    .clone()
                    .update(cx, |registry, _| registry.stop_codex_sessions());
                for receiver in stopped {
                    receiver.recv_timeout(Duration::from_secs(5)).unwrap();
                }
            });
        });
    }

    fn attach_removal_layout_session(
        sidebar: &mut Sidebar,
        path: &str,
        provider: grove_core::codex::Provider,
        window: &mut Window,
        cx: &mut Context<Sidebar>,
    ) {
        let registry = sidebar.runtime.read(cx).registry.clone();
        let id = registry.update(cx, |registry, _| {
            registry.insert_meta("Layout fixture".into(), path.into(), provider.agent())
        });
        let (handle, _actions) = grove_core::codex::Handle::channel();
        let (_updates, receive) = std::sync::mpsc::channel();
        let saved = grove_core::codex::SavedSession {
            html_policy: grove_core::visuals::HtmlPolicy::Legacy,
            visuals: Vec::new(),
            session_review: grove_core::codex::SessionReview::default(),
            rich_plans: grove_core::rich_plan::PlanStore::default(),
            side_chats: std::collections::HashMap::default(),
            fork_origin: None,
            claude_resume: None,
            claude_subagents: std::collections::HashMap::default(),
            edit_checkpoints: Vec::new(),
            pending_edit: None,
            rewind_drafts: Vec::new(),
            provider,
            composer: grove_core::codex::ComposerSettings::default(),
            context: None,
            context_details: None,
            key: "layout-fixture".into(),
            project: "Layout fixture".into(),
            cwd: path.into(),
            label: provider.label().into(),
            title: None,
            title_generation: None,
            roots: Vec::new(),
            thread_id: None,
            items: vec![
                serde_json::json!({"type":"userMessage", "text":"Keep this conversation in place"}),
            ]
            .into(),
            work_periods: Vec::new(),
            draft: "Preserved draft".into(),
            draft_images: Vec::new(),
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
            activity_reporting: false,
        };
        let session = cx.new(|cx| {
            crate::entities::codex_session::CodexSession::from_connection(
                grove_core::codex::Snapshot::new(saved),
                grove_core::codex::Connection {
                    handle: handle.clone(),
                    updates: receive.into(),
                },
                cx,
            )
        });
        registry.update(cx, |registry, _| registry.attach_codex(id, session, handle));
        sidebar.select(Selection::Session(id), cx);
        sidebar
            .worktree_delete_focus
            .insert(path.into(), cx.focus_handle());
        sidebar.sync(window, cx);
        cx.notify();
    }

    #[test]
    #[ignore = "requires macOS Metal renderer; writes single-sidebar fixture screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn single_sidebar_native_snapshots() {
        use gpui::AppContext as _;
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            std::sync::Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        for (name, light, width, rem, collapsed, projects, provider) in [
            (
                "light-activity-codex",
                true,
                1100.,
                16.,
                false,
                false,
                grove_core::codex::Provider::Codex,
            ),
            (
                "light-projects-claude",
                true,
                1100.,
                16.,
                false,
                true,
                grove_core::codex::Provider::Claude,
            ),
            (
                "light-collapsed-narrow",
                true,
                760.,
                16.,
                true,
                false,
                grove_core::codex::Provider::Codex,
            ),
            (
                "dark-activity-claude",
                false,
                1100.,
                16.,
                false,
                false,
                grove_core::codex::Provider::Claude,
            ),
            (
                "dark-projects-narrow-zoom",
                false,
                760.,
                20.,
                false,
                true,
                grove_core::codex::Provider::Claude,
            ),
            (
                "dark-collapsed-codex",
                false,
                1100.,
                20.,
                true,
                true,
                grove_core::codex::Provider::Codex,
            ),
            (
                "light-worktrees-codex",
                true,
                1100.,
                16.,
                false,
                false,
                grove_core::codex::Provider::Codex,
            ),
            (
                "dark-worktrees-narrow-claude",
                false,
                760.,
                20.,
                false,
                false,
                grove_core::codex::Provider::Claude,
            ),
        ] {
            let path = "/grove-single-sidebar-fixture";
            cx.update(|cx| {
                cx.set_global(SettingsState::new(grove_core::storage::Store {
                    sidebar_collapsed: collapsed,
                    sidebar_group_by_project: projects,
                    sidebar_group_by_worktree: name.contains("worktrees"),
                    projects: vec![grove_core::storage::Project {
                        name: "Layout fixture".into(),
                        path: path.into(),
                        scripts: grove_core::storage::ProjectScripts::default(),
                        archived: false,
                        worktree_dir: None,
                    }],
                    ..Default::default()
                }));
                grove_core::theme::set(
                    grove_core::theme::by_name(if light { "grove-light" } else { "grove-dark" })
                        .unwrap(),
                );
                c::sync_component_theme(cx);
                gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
            });
            let window = cx
                .open_window(gpui::size(gpui::px(width), gpui::px(720.)), |window, cx| {
                    cx.new(|cx| {
                        let runtime = cx.new(Runtime::new);
                        runtime.read(cx).tree.clone().update(cx, |tree, _| {
                            tree.set_active_worktrees(
                                0,
                                vec![
                                    grove_core::git::Worktree {
                                        path: path.into(),
                                        branch: "feature/navigation-with-a-long-worktree-name"
                                            .into(),
                                        mtime: None,
                                        is_main: true,
                                    },
                                    grove_core::git::Worktree {
                                        path: format!("{path}/review-checkout"),
                                        branch: "feature/navigation-with-a-long-worktree-name"
                                            .into(),
                                        mtime: None,
                                        is_main: false,
                                    },
                                ],
                            );
                        });
                        let mut sidebar = Sidebar::new(runtime, window, cx);
                        attach_removal_layout_session(
                            &mut sidebar,
                            path,
                            grove_core::codex::Provider::Codex,
                            window,
                            cx,
                        );
                        attach_removal_layout_session(
                            &mut sidebar,
                            &format!("{path}/review-checkout"),
                            grove_core::codex::Provider::Claude,
                            window,
                            cx,
                        );
                        let id = sidebar
                            .runtime
                            .read(cx)
                            .registry
                            .read(cx)
                            .all()
                            .iter()
                            .find(|m| m.agent == provider.agent())
                            .unwrap()
                            .id;
                        sidebar.select(Selection::Session(id), cx);
                        sidebar.destination = super::global_navigation::Destination::Sessions;
                        sidebar.mode = ViewMode::List;
                        sidebar
                    })
                })
                .unwrap();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
            let output = format!("/tmp/grove-single-sidebar-{name}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&output)
                .unwrap();
            eprintln!("Single sidebar native fixture: {output}");
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }

    /// Real macOS rendering with channel-backed Codex/Claude fixtures; no live provider calls.
    #[test]
    #[ignore = "requires macOS Metal renderer; writes worktree removal screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn worktree_removal_native_snapshots() {
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            std::sync::Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        let path = "/grove-removal-layout-fixture";
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            crate::theme::sync_component_theme(cx);
            cx.set_reduce_motion(true);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "Layout fixture".into(),
                    path: path.into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
        });
        for provider in [
            grove_core::codex::Provider::Codex,
            grove_core::codex::Provider::Claude,
        ] {
            for width in [1600., 768., 600.] {
                let owner = std::rc::Rc::new(std::cell::RefCell::new(None));
                let capture = owner.clone();
                let window = cx
                    .open_window(gpui::size(gpui::px(width), gpui::px(900.)), |window, cx| {
                        let runtime = cx.new(Runtime::new);
                        runtime.read(cx).tree.clone().update(cx, |tree, _| {
                            tree.set_active_worktrees(
                                0,
                                vec![grove_core::git::Worktree {
                                    path: path.into(),
                                    branch: "feature".into(),
                                    mtime: None,
                                    is_main: false,
                                }],
                            );
                        });
                        let sidebar = cx.new(|cx| {
                            let mut sidebar = Sidebar::new(runtime, window, cx);
                            attach_removal_layout_session(&mut sidebar, path, provider, window, cx);
                            sidebar
                        });
                        *capture.borrow_mut() = Some(sidebar.clone());
                        cx.new(|cx| {
                            gpui_component::Root::new(sidebar, window, cx)
                                .bordered(false)
                                .bg(c::BG())
                        })
                    })
                    .unwrap();
                for open in [false, true] {
                    cx.update_window(window.into(), |_, window, cx| {
                        owner.borrow().as_ref().unwrap().update(cx, |sidebar, cx| {
                            if open {
                                sidebar.act(Action::RemoveWorktree(0, path.into()), window, cx);
                            }
                        });
                    })
                    .unwrap();
                    cx.run_until_parked();
                    cx.update_window(window.into(), |_, window, cx| {
                        let _ = window.draw(cx);
                    })
                    .unwrap();
                    let output = format!(
                        "/tmp/grove-worktree-removal-{provider:?}-{width}-{}.png",
                        if open { "open" } else { "rest" }
                    );
                    cx.capture_screenshot(window.into())
                        .unwrap()
                        .save(&output)
                        .unwrap();
                    eprintln!("Worktree removal native fixture: {output}");
                }
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
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
                .env("GROVE_TEST_CONFIG_DIR", &root)
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
        cx.simulate_keystrokes("tab tab tab tab enter");
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
        cx.simulate_keystrokes("tab tab space tab tab enter");
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
}
