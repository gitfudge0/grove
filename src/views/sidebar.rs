//! Workspace-scoped navigation. UI state is kept per workspace while processes keep running.
mod action_dispatch;
mod action_feedback;
mod activity_reply;
mod agent_preview;
mod branch_history;
mod branch_preview;
mod canvas_navigation;
mod change_description;
mod change_review;
mod change_tools;
mod checkout_panel;
mod confirmation;
mod content;
mod context_panel;
mod contributions;
pub(crate) mod dashboard;
mod dashboard_timeline;
mod files;
mod global_navigation;
mod grid;
mod layout;
mod multi_project;
mod navigation_controls;
mod plan_preview;
mod project_flyout;
mod project_setup;
mod project_tree;
pub(super) mod projects;
mod rail_layout;
mod script_preview;
mod session_diff;
mod session_history;
mod session_motion;
mod session_navigation;
mod session_placement;
mod session_rows;
mod session_scope;
mod side_chat;
mod state_sync;
mod task_glance;
mod title_edit;
mod usage;
mod worktree_actions;
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
use action_dispatch::Action;
#[cfg(test)]
use gpui::MouseMoveEvent;
use gpui::{
    div, prelude::*, AnyElement, App, Context, CursorStyle, Div, Entity, EventEmitter, FocusHandle,
    Focusable, MouseButton, ScrollHandle, SharedString, Window,
};
use gpui_component::{input::InputState, ElementExt};
use grove_core::agent::Agent;
use multi_project::SidebarContext;
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
// Three distinct leading edges: project, worktree and session.
const WORKTREE_LEADING_INSET: f32 = HIERARCHY_INSET + SPACE_2XL;
const SESSION_LEADING_INSET: f32 = WORKTREE_LEADING_INSET + SPACE_3XL;
#[cfg(test)]
const WORKTREE_GROUP_GAP: f32 = 18.0;
const HIERARCHY_META_TEXT: f32 = TEXT_MICRO;
#[cfg(test)]
const HIERARCHY_TRAILING_W: f32 = 24.0;
#[cfg(test)]
const PROJECT_ROW_H: f32 = 35.0;
#[cfg(test)]
const PROJECT_TITLE_LINE_H: f32 = 21.0;
const SESSION_TREE_ROW_H: f32 = 32.0;
const SESSION_LAUNCHER_SIZE: f32 = ICON_SM;
const SESSION_TITLE_LINE_H: f32 = 17.0;
const SESSION_META_LINE_H: f32 = 14.0;
/// Single-line reference to a multi-checkout session under a secondary worktree group.
const SESSION_REFERENCE_ROW_H: f32 = 32.0;
const SESSION_LIST_CARD_H: f32 = 78.0;
const SESSION_LIST_HEADER_H: f32 = 20.0;
const SESSION_LIST_TITLE_H: f32 = 20.0;
const SESSION_LIST_FOOTER_H: f32 = 16.0;
#[cfg(test)]
const SESSION_FIRST_GAP: f32 = SPACE_XS;
#[cfg(test)]
const SESSION_SPINNER_PERIOD: Duration = Duration::from_millis(1200);

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
}
impl Focusable for Sidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
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
mod tests;
