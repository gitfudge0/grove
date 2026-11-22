//! Checkout actions live beside the selected canvas, outside its scroll region.

mod git_operations;
#[cfg(test)]
use git_operations::commit_provider;
mod git_confirmation;
#[cfg(test)]
use git_confirmation::checkout_failure_copy;
use git_confirmation::prepare_resolution_draft;
mod agents;
#[cfg(test)]
use agents::{checkout_agent_groups, checkout_agents};
pub(super) use agents::{checkout_agent_presentation, checkout_agent_row};
mod panel_layout;

mod pull_request;

use super::*;
use crate::views::components::{form_action, project_field_well, transcript_action};
use gpui_component::{
    menu::{PopupMenu, PopupMenuItem},
    Disableable,
};
use grove_core::{
    checkout_git::{self, CheckoutGitState, GitAction},
    checkout_pr::{self, PullRequest},
    commit_agent::{self, CommitControl, CommitOutcome, CommitPhase},
    diff::{self, ChangeSummary},
};
use std::path::Path;

mod scripts;

const CARD_WIDTH: f32 = 280.;
pub(super) const WIDE_VIEWPORT: f32 = MODAL_W_XL + MODAL_W_SM;
pub(super) const CHECKOUT_SIDE_VIEWPORT: f32 = MODAL_W_XL;
const NARROW_HEIGHT_FRACTION: f32 = 0.25;
const STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const REMOTE_REFRESH_INTERVAL: Duration = Duration::from_mins(1);
/// Minimum count-column width; wider values expand the shared columns together.
const STAT_COL_W: f32 = 48.;
/// Top-level action and supporting copy starts after the leading icon slot.
const CHECKOUT_LABEL_INSET: f32 = SPACE_2XL + ICON_LG + SPACE_SM;

/// Reuse compact shared actions on the card's neutral field surface.
fn checkout_action(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
    window: &Window,
    cx: &App,
) -> gpui_component::button::Button {
    transcript_action(id, label, primary, window, cx)
        .bg(c::FIELD_FILL())
        .justify_start()
}

/// Constrain inner text as well as the tooltip surface at narrow viewport widths.
fn script_tooltip_content(text: SharedString, window: &Window) -> gpui_component::tooltip::Tooltip {
    let margin = rpx(SPACE_LG).to_pixels(window.rem_size());
    let surface_width = (window.viewport_size().width - margin * 2.)
        .min(rpx(CARD_WIDTH).to_pixels(window.rem_size()))
        .max(gpui::px(1.));
    let text_width = (surface_width - margin * 2.).max(gpui::px(1.));
    crate::views::components::tooltip_content(
        move |_, _| {
            div()
                .max_w(text_width)
                .min_w_0()
                .whitespace_normal()
                .debug_selector(|| "checkout-script-context-text".into())
                .child(text.clone())
        },
        window,
    )
    .m_0()
    .max_w(surface_width)
}

#[derive(Default)]
pub(super) struct CheckoutPanel {
    pub(super) change_tools: super::change_tools::ChangeTools,
    pub(super) feedback: super::action_feedback::Feedbacks,
    pub(super) script_feedback_observers: HashMap<u64, gpui::Subscription>,
    pub(super) script_feedback_tokens: HashMap<u64, u64>,
    pub(super) script_errors: HashMap<(String, String), String>,
    pub(super) stopping_runs: HashSet<u64>,
    pub(super) script_peek: Option<super::script_preview::ScriptPeek>,
    pub(super) script_peek_task: Option<gpui::Task<()>>,
    pub(super) script_hover_target: Option<u64>,
    pub(super) script_peek_focus: HashMap<u64, FocusHandle>,
    pub(super) script_peek_bounds:
        HashMap<u64, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    pub(super) script_terminals: HashMap<u64, Entity<super::super::terminal_view::TerminalView>>,
    pub(super) agents_visible: bool,
    pub(super) plan_peek: Option<super::plan_preview::PlanPeek>,
    pub(super) plan_peek_task: Option<gpui::Task<()>>,
    pub(super) plan_close_task: Option<gpui::Task<()>>,
    pub(super) plan_hover_target: Option<gpui::EntityId>,
    pub(super) plan_focus: HashMap<gpui::EntityId, FocusHandle>,
    pub(super) plan_bounds:
        HashMap<gpui::EntityId, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    pub(super) plan_presence: motion::Presence,
    agent_disclosures: HashMap<SessionId, AgentDisclosure>,
    pub(super) agent_observer: Option<(gpui::EntityId, gpui::Subscription)>,
    pub(super) peek: Option<AgentPeek>,
    pub(super) peek_task: Option<gpui::Task<()>>,
    pub(super) peek_hover_target: Option<(SessionId, String)>,
    pub(super) peek_close_task: Option<gpui::Task<()>>,
    pub(super) peek_presence: motion::Presence,
    pub(super) agent_focus: HashMap<String, FocusHandle>,
    pub(super) agent_bounds:
        HashMap<String, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    script_row_focus: HashMap<(String, String), (FocusHandle, gpui::Subscription)>,
    script_details_focus: HashMap<(String, String), FocusHandle>,
    script_run_focus: HashMap<(String, String), FocusHandle>,
    script_details_scroll: HashMap<(String, String), ScrollHandle>,
    script_name_bounds:
        HashMap<(String, String), std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    open_script_preview: Option<(String, String)>,
    scripts_show_all: std::collections::HashSet<String>,
    path: Option<String>,
    state: Option<Result<CheckoutGitState, String>>,
    summary: Option<ChangeSummary>,
    state_error: Option<String>,
    pr_error: Option<String>,
    main_state: Option<Result<Option<checkout_git::MainBranchState>, String>>,
    main_checked_at: Option<Instant>,
    main_loading: bool,
    main_error: Option<String>,
    focus_confirmation: bool,
    recovery_files: Vec<String>,
    revert_commits: Vec<grove_core::history::Commit>,
    recovery: Option<CheckoutRecovery>,
    expanded_categories: std::collections::HashSet<diff::ChangeGroup>,
    loading: bool,
    pr_state: Option<Result<Option<PullRequest>, String>>,
    pr_loading: bool,
    pr_identity: Option<[u8; 32]>,
    pr_checked_at: Option<Instant>,
    pr_expanded: bool,
    pr_composing: bool,
    pr_prompt_busy: bool,
    pr_merge: Option<PullRequest>,
    publish: Option<String>,
    pub(super) expanded: bool,
    pub(super) hidden: bool,
    width_override: Option<f32>,
    pub(super) toggle_available: bool,
    pub(super) toggle_focus: Option<FocusHandle>,
    pub(super) side_chat_owner: Option<Selection>,
    pub(super) side_chat_focus: Option<FocusHandle>,
    panel_motion: Option<(Instant, f32, f32, u64)>,
    panel_motion_revision: u64,
    // Preserve the outgoing panel's width while its empty shell collapses.
    panel_last_wide_width: f32,
    // Keep dismissal continuous for checkout, side chat, and recovery content.
    pub(super) panel_last_narrow_height: std::rc::Rc<std::cell::Cell<gpui::Pixels>>,
    pub(super) side_chat_panel_scroll: ScrollHandle,
    pub(super) side_chats: HashMap<(SessionId, String), super::side_chat::SideChat>,
    pub(super) chosen_root: Option<String>,
    root_owner: Option<Selection>,
    return_focus: Option<FocusHandle>,
    confirmation_focus: Option<FocusHandle>,
    trigger_focus: HashMap<&'static str, FocusHandle>,
    source_bounds: HashMap<&'static str, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    pub(super) popup: Option<Entity<PopupMenu>>,
    pub(super) popup_position: Option<gpui::Point<gpui::Pixels>>,
    pub(super) narrow_content: Option<AnyElement>,
    pub(super) popup_subscription: Option<gpui::Subscription>,
    generation: u64,
    checked_at: Option<Instant>,
    restore_focus: bool,
    operation: CheckoutOperation,
    operations: HashMap<String, CheckoutOperation>,
}

pub(super) struct AgentPeek {
    pub(super) session: SessionId,
    pub(super) child: String,
    pub(super) explicit: bool,
    pub(super) closing: bool,
    pub(super) focus: FocusHandle,
    pub(super) transcript: Entity<super::super::codex_view::CodexView>,
    pub(super) _tick: gpui::Task<()>,
}

#[derive(Default)]
struct AgentDisclosure {
    show_all: bool,
    previous: bool,
}

#[derive(Clone)]
enum CheckoutRecovery {
    Discard(CheckoutGitState),
    Revert(CheckoutGitState, String, String),
    Main(CheckoutGitState, checkout_git::MainBranchState),
}

#[derive(Default)]
struct CheckoutOperation {
    pr_message: Option<String>,
    revision: u64,
    busy: bool,
    network_action: Option<(&'static str, &'static str)>,
    network_target: Option<(String, u64)>,
    message: Option<String>,
    failures: HashMap<&'static str, String>,
    commit_status: CommitUi,
    commit_job: u64,
    commit_job_path: Option<String>,
    commit_control: Option<std::sync::Arc<CommitControl>>,
}

#[derive(Default)]
enum CommitUi {
    #[default]
    Idle,
    Generating,
    Canceling,
    Committing,
    Succeeded,
    Failed,
}

enum CommitEvent {
    Progress(CommitPhase),
    Finished(
        Box<(
            Result<CommitOutcome, String>,
            Result<CheckoutGitState, String>,
        )>,
    ),
}

pub(super) fn selected_checkout(
    targets: &[(usize, String)],
    chosen: Option<&str>,
) -> Option<(usize, String)> {
    if targets.len() == 1 {
        targets.first().cloned()
    } else {
        targets
            .iter()
            .find(|(_, path)| Some(path.as_str()) == chosen)
            .cloned()
    }
}

impl Sidebar {
    pub(super) fn checkout_confirmation_bounds(&self) -> gpui::Bounds<gpui::Pixels> {
        let source = if matches!(
            self.checkout_panel.recovery,
            Some(CheckoutRecovery::Main(..))
        ) {
            "main"
        } else if self.checkout_panel.recovery.is_some() {
            "recovery"
        } else if self.checkout_panel.pr_merge.is_some() {
            "pr-merge"
        } else {
            "publish"
        };
        self.checkout_panel
            .source_bounds
            .get(source)
            .map(|bounds| bounds.get())
            .unwrap_or_default()
    }
    pub(super) fn checkout_confirmation_open(&self) -> bool {
        self.checkout_panel.publish.is_some()
            || self.checkout_panel.pr_merge.is_some()
            || self.checkout_panel.recovery.is_some()
    }

    pub(super) fn focus_worktree_terminal_trigger(&self, window: &mut Window, cx: &mut App) {
        if let Some(focus) = self.checkout_panel.trigger_focus.get("terminal") {
            focus.focus(window, cx);
        } else {
            self.focus.focus(window, cx);
        }
    }

    pub(super) fn checkout_targets(&self, cx: &App) -> Vec<(usize, String)> {
        if self.dashboard_open {
            return self
                .dashboard_checkout(cx)
                .or_else(|| Some((usize::MAX, crate::entities::session_registry::home_dir())))
                .into_iter()
                .collect();
        }
        if let Some(Selection::Worktree(idx, path)) = &self.selection {
            return self
                .snapshot
                .projects
                .iter()
                .find(|project| project.idx == *idx)
                .filter(|project| {
                    project
                        .worktrees
                        .iter()
                        .any(|worktree| worktree.path == *path)
                })
                .map(|_| vec![(*idx, path.clone())])
                .unwrap_or_default();
        }
        if let Some(Selection::Session(id) | Selection::Home(id)) = self.selection {
            let registry = self.runtime.read(cx).registry.read(cx);
            if let Some(meta) = registry
                .meta(id)
                .or_else(|| registry.home_terminals().iter().find(|meta| meta.id == id))
            {
                if meta.project.is_empty() {
                    return vec![(usize::MAX, meta.wt_path.clone())];
                }
                let mut paths = vec![meta.wt_path.clone()];
                paths.extend(meta.context_roots.iter().map(|root| root.wt_path.clone()));
                paths.sort();
                paths.dedup();
                return paths
                    .into_iter()
                    .filter_map(|path| {
                        self.snapshot
                            .projects
                            .iter()
                            .find(|project| project.worktrees.iter().any(|wt| wt.path == path))
                            .map(|project| (project.idx, path))
                    })
                    .collect();
            }
        }
        Vec::new()
    }

    pub(super) fn checkout_has_local_changes(&self, path: &str) -> bool {
        self.checkout_panel.path.as_deref() == Some(path)
            && self
                .checkout_panel
                .summary
                .as_ref()
                .is_some_and(|summary| summary.files > 0)
    }

    #[cfg(test)]
    pub(super) fn set_change_tools_sources(
        &mut self,
        path: &str,
        local: bool,
        pr: Option<PullRequest>,
    ) {
        self.checkout_panel.path = Some(path.into());
        let mut summary = diff::summarize(&[]);
        summary.files = usize::from(local);
        self.checkout_panel.summary = Some(summary);
        self.checkout_panel.pr_state = Some(Ok(pr));
    }

    pub(super) fn checkout_description_pr(&self) -> Option<PullRequest> {
        self.checkout_panel
            .pr_state
            .as_ref()
            .and_then(|state| state.as_ref().ok())
            .and_then(Clone::clone)
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn configure_checkout_group_capture(
        &mut self,
        path: &str,
        frozen_git: &CheckoutGitState,
        summary: &ChangeSummary,
        state: &str,
    ) {
        // Invalidate background samples started by the normal Sidebar shell.
        self.checkout_panel.generation = self.checkout_panel.generation.wrapping_add(1);
        self.checkout_panel.loading = false;
        self.checkout_panel.main_loading = false;
        self.checkout_panel.pr_loading = false;
        self.checkout_panel.state_error = None;
        self.checkout_panel.main_error = None;
        self.checkout_panel.pr_error = None;
        let mut git = frozen_git.clone();
        git.branch = Some("feature/payment-retries-with-a-long-name".into());
        git.dirty = !matches!(state, "clean" | "pull-error" | "pull-error-narrow");
        git.upstream = Some("origin/feature/payment-retries".into());
        git.ahead = Some(0);
        git.behind = Some(if state == "pull-error-narrow" { 0 } else { 2 });
        self.checkout_panel.pr_identity = Some(git.target_identity());
        self.checkout_panel.state = Some(Ok(git));
        self.checkout_panel.summary = (state != "clean").then(|| summary.clone());
        self.checkout_panel.checked_at = Some(Instant::now() + Duration::from_hours(1));
        self.checkout_panel.pr_checked_at = Some(Instant::now() + Duration::from_hours(1));
        self.checkout_panel.main_checked_at = Some(Instant::now() + Duration::from_hours(1));
        self.checkout_panel.pr_state = Some(Ok(Some(serde_json::from_value(serde_json::json!({"number":142,"url":"https://github.com/example/northstar/pull/142","title":"Payment retries","state":"OPEN","isDraft":false,"headRefOid":"fixture","reviewDecision":"APPROVED","mergeable":"MERGEABLE","mergeStateStatus":"BLOCKED","latestReviews":[{"state":"APPROVED"}],"reviewRequests":[],"statusCheckRollup":[{"status":"COMPLETED","conclusion":"FAILURE"}]})).unwrap())));
        self.checkout_panel.pr_expanded = false;
        self.checkout_panel.operation.busy = state == "working";
        self.checkout_panel.operation.commit_status = if state == "working" {
            CommitUi::Generating
        } else {
            CommitUi::Idle
        };
        self.checkout_panel.operation.commit_job_path = Some(path.to_owned());
        self.checkout_panel.operation.failures.clear();
        self.checkout_panel.operation.message = None;
        if matches!(state, "pull-error" | "pull-error-narrow") {
            self.checkout_panel.operation.message = Some(
                "Branches have diverged. Resolve the branch divergence before pulling again".into(),
            );
            self.checkout_panel.operation.failures.insert(
                "pull",
                "Branches have diverged. Resolve the branch divergence before pulling again".into(),
            );
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn checkout_group_capture_state(&self) -> String {
        format!(
            "dirty={:?}, busy={}",
            self.checkout_panel
                .state
                .as_ref()
                .and_then(|state| state.as_ref().ok())
                .map(|state| state.dirty),
            self.checkout_panel.operation.busy
        )
    }

    fn render_checkout_container_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.checkout_panel.width_override != Some(Self::logical_window_width(window)) {
            self.checkout_panel.width_override = None;
        }
        self.checkout_panel.narrow_content = None;
        self.checkout_panel.agents_visible = false;
        self.checkout_panel.toggle_available = false;
        for kind in [
            "checkout",
            "terminal",
            "publish",
            "summary",
            "commit",
            "pr-merge",
            "pr-details",
            "recovery",
            "main",
        ] {
            self.checkout_panel
                .trigger_focus
                .entry(kind)
                .or_insert_with(|| cx.focus_handle());
            self.checkout_panel.source_bounds.entry(kind).or_default();
        }
        let targets = self.checkout_targets(cx);
        let owner = if self.dashboard_open {
            None
        } else {
            self.selection.clone()
        };
        if self.checkout_panel.root_owner != owner {
            self.checkout_panel.peek = None;
            self.checkout_panel.peek_task = None;
            self.checkout_panel.peek_hover_target = None;
            self.checkout_panel.agent_focus.clear();
            self.checkout_panel.agent_bounds.clear();
            self.checkout_panel.focus_confirmation = false;
            self.checkout_panel.restore_focus = false;
            self.checkout_panel.return_focus = None;
            self.checkout_panel.root_owner = owner;
            if self.dashboard_open {
                self.checkout_panel.plan_peek = None;
                self.checkout_panel.plan_peek_task = None;
                self.checkout_panel.script_peek = None;
                self.checkout_panel.script_peek_task = None;
            }
            self.checkout_panel.chosen_root = None;
            self.checkout_panel.select_path(None);
            self.checkout_panel.publish = None;
            self.checkout_panel.recovery = None;
            self.checkout_panel.pr_merge = None;
            self.checkout_panel.popup = None;
            self.checkout_panel.popup_subscription = None;
            self.checkout_panel.generation = self.checkout_panel.generation.wrapping_add(1);
        }
        let has_agents = !self.dashboard_open && self.selected_session_has_agents(cx);
        let has_plan = !self.dashboard_open && !self.selected_native_plans(cx).is_empty();
        let no_project = targets.is_empty()
            && (self.dashboard_open
                || matches!(
                    self.selection,
                    Some(Selection::Session(_) | Selection::Home(_))
                ));
        let agents_only = self.is_zen() || targets.is_empty();
        if agents_only {
            self.branch_entry = None;
            self.branch_peek = None;
            self.branch_peek_task = None;
        }
        if self.mode == ViewMode::Grid && !self.is_zen() && !self.dashboard_open
            || self.project_panel.is_some()
            || self.project_setup.is_some()
            || self.pending_new_worktree.is_some()
            || self
                .settings_panel
                .as_ref()
                .is_some_and(|panel| panel.read(cx).is_open())
            || (targets.is_empty() && !has_agents && !has_plan && !no_project)
            || (self.is_zen() && !has_agents && !has_plan)
        {
            self.branch_peek = None;
            self.branch_peek_task = None;
            self.branch_entry = None;
            return self.render_content_body(window, cx);
        }
        self.checkout_panel.toggle_available = !self.is_zen();
        if self.files_active && self.files_owner == self.selection && !self.is_zen() {
            return self.render_files_layout(window, cx);
        }
        if !self.dashboard_open
            && self.checkout_panel.side_chat_owner.is_some()
            && self.checkout_panel.side_chat_owner == self.selection
            && !self.is_zen()
        {
            let wide = Self::logical_window_width(window) >= WIDE_VIEWPORT;
            let panel = self.render_side_chat(window, cx);
            if wide {
                let body = self.render_content_body(window, cx);
                return self.wide_panel_layout(body, panel, window, cx);
            }
            self.checkout_panel.narrow_content = Some(panel);
            return self.render_content_body(window, cx);
        }
        if self.checkout_panel.hidden && !self.is_zen() {
            self.branch_peek = None;
            self.branch_peek_task = None;
            self.branch_entry = None;
            if !cx.reduce_motion()
                && self
                    .checkout_panel
                    .panel_motion
                    .is_some_and(|(started, _, _, _)| {
                        started.elapsed() < Duration::from_millis(MOTION_BASE_MS)
                    })
            {
                if !self.is_zen()
                    && ((self.checkout_panel.side_chat_owner.is_none()
                        && Self::logical_window_width(window) >= CHECKOUT_SIDE_VIEWPORT)
                        || Self::logical_window_width(window) >= WIDE_VIEWPORT)
                {
                    let body = self.render_content_body(window, cx);
                    return self.wide_panel_layout(body, div().into_any_element(), window, cx);
                }
                let height = self.checkout_panel.panel_last_narrow_height.get()
                    * self.panel_motion_width().unwrap_or(0.);
                self.checkout_panel.narrow_content = Some(
                    div()
                        .debug_selector(|| "narrow-session-panel-exit".into())
                        .w_full()
                        .h(height)
                        .flex_shrink_0()
                        .into_any_element(),
                );
                window.request_animation_frame();
                return self.render_content_body(window, cx);
            }
            self.checkout_panel.panel_motion = None;
            return self.render_content_body(window, cx);
        }
        // Resize fitting is transient: preserve panel preferences and session state.
        let available = Self::logical_window_width(window) - self.rail_width(window, cx);
        if !self.is_zen()
            && available < CARD_WIDTH + MODAL_W_LG
            && self.checkout_panel.width_override != Some(Self::logical_window_width(window))
        {
            self.checkout_panel.panel_motion = None;
            return self.render_content_body(window, cx);
        }
        let wide = !self.is_zen() && Self::logical_window_width(window) >= CHECKOUT_SIDE_VIEWPORT;
        self.checkout_panel.agents_visible = !self.is_zen() || self.checkout_panel.expanded;
        let selected = selected_checkout(&targets, self.checkout_panel.chosen_root.as_deref());
        self.restore_session_description(cx);
        let linked_description = self.linked_description_key();
        self.restore_session_review(cx);
        let linked_review = self.linked_review_key();
        let mut card = project_field_well(false, false)
            .id("checkout-card")
            .debug_selector(|| "checkout-card".into())
            .h_auto()
            .justify_start()
            .w_full()
            .px_0()
            .py(rpx(SPACE_SM))
            .gap(rpx(SPACE_XS));
        let mut changes_section = div().flex().flex_col().min_w_0();
        let mut git_section = div().flex().flex_col().min_w_0();
        let mut has_changes = false;
        let mut has_git = false;
        let mut scripts_section: Option<AnyElement> = None;
        let mut pr_section = None;
        if !agents_only {
            if let Some((idx, path)) = selected {
                if self.checkout_panel.path.as_ref() != Some(&path) {
                    self.checkout_panel.change_tools.dismiss();
                    self.checkout_panel.focus_confirmation = false;
                    self.checkout_panel.restore_focus = false;
                    self.checkout_panel.return_focus = None;
                    self.branch_peek = None;
                    self.branch_peek_task = None;
                    self.branch_entry = None;
                    if self
                        .history
                        .as_ref()
                        .is_some_and(|history| history.read(cx).checkout != path)
                    {
                        self.history_hover = super::branch_history::HistoryHover::default();
                        self.history = None;
                        self.history_observer = None;
                        self.history_selection = None;
                        self.history_return_focus = None;
                    }
                    self.checkout_panel.generation = self.checkout_panel.generation.wrapping_add(1);
                    self.checkout_panel.select_path(Some(path.clone()));
                    self.checkout_panel.checked_at = None;
                    self.checkout_panel.state = None;
                    self.checkout_panel.state_error = None;
                    self.checkout_panel.pr_error = None;
                    self.checkout_panel.main_state = None;
                    self.checkout_panel.main_error = None;
                    self.checkout_panel.main_checked_at = None;
                    self.checkout_panel.main_loading = false;
                    self.checkout_panel.recovery = None;
                    self.checkout_panel.pr_state = None;
                    self.checkout_panel.pr_identity = None;
                    self.checkout_panel.pr_loading = false;
                    self.checkout_panel.pr_checked_at = None;
                    self.checkout_panel.pr_expanded = false;
                    self.checkout_panel.pr_composing = false;
                    self.checkout_panel.pr_prompt_busy = false;
                    self.checkout_panel.pr_merge = None;
                    self.checkout_panel.summary = None;
                    self.checkout_panel.expanded_categories.clear();
                    self.checkout_panel.loading = false;
                    self.checkout_panel.publish = None;
                }
                if self.checkout_panel.focus_confirmation {
                    self.checkout_panel.focus_confirmation = false;
                    let focus = self
                        .checkout_panel
                        .confirmation_focus
                        .get_or_insert_with(|| cx.focus_handle());
                    focus.focus(window, cx);
                }
                if self.checkout_panel.restore_focus {
                    self.checkout_panel.restore_focus = false;
                    if let Some(focus) = self.checkout_panel.return_focus.take() {
                        focus.focus(window, cx);
                    }
                }
                if self
                    .checkout_panel
                    .local_refresh_due(window.is_window_active())
                {
                    self.checkout_load(path.clone(), cx);
                }
                let project = cx.global::<SettingsState>().store.script_project(&path);
                let target = path.clone();
                card = card.child(
                    checkout_row("checkout-finder", "Open in Finder", "folder", window, cx)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.act(Action::Reveal(target.clone()), window, cx);
                        })),
                );
                let terminal_path = path.clone();
                let terminal_focus = self.checkout_panel.trigger_focus["terminal"].clone();
                card = card.child(
                    checkout_row("checkout-terminal", "Open Terminal", "terminal", window, cx)
                        .with_focus_handle(terminal_focus)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_worktree_terminal(&terminal_path, cx);
                        })),
                );
                scripts_section = Some(
                    self.render_checkout_scripts(&project, &path, window, cx)
                        .into_any_element(),
                );
                if self
                    .checkout_panel
                    .state
                    .as_ref()
                    .is_some_and(Result::is_ok)
                    || !self.checkout_panel.operation.failures.is_empty()
                {
                    has_git = true;
                    git_section = git_section.child(checkout_git_heading());
                    let state = self.checkout_panel.state.clone().and_then(Result::ok);
                    let branch_label = state.as_ref().map_or_else(
                        || "Branch unavailable".to_owned(),
                        |state| {
                            state.branch.clone().unwrap_or_else(|| {
                                state.head.as_deref().map_or_else(
                                    || "No commits yet".to_owned(),
                                    |head| {
                                        format!("Detached HEAD · {}", &head[..head.len().min(7)])
                                    },
                                )
                            })
                        },
                    );
                    let history = self.render_branch_history_entry(
                        path.clone(),
                        branch_label,
                        state.as_ref().map(CheckoutGitState::target_identity),
                        window,
                        cx,
                    );
                    let menu_path = path.clone();
                    git_section = git_section.child(
                        div()
                            .flex()
                            .items_center()
                            .w_full()
                            .min_w_0()
                            .child(div().flex_1().min_w_0().child(history))
                            .child(
                                checkout_action(
                                    "checkout-recovery-menu",
                                    "Branch recovery actions",
                                    false,
                                    window,
                                    cx,
                                )
                                .debug_selector(|| "checkout-recovery-menu".into())
                                .w(rpx(CHROME_CONTROL_H))
                                .h(rpx(CHROME_CONTROL_H))
                                .icon_only()
                                .icon(
                                    gpui_component::Icon::default()
                                        .path("icons/more.svg")
                                        .size(rpx(ICON_SM)),
                                )
                                .with_focus_handle(
                                    self.checkout_panel.trigger_focus["recovery"].clone(),
                                )
                                .mr(rpx(SPACE_2XL))
                                .relative()
                                .child({
                                    let bounds =
                                        self.checkout_panel.source_bounds["recovery"].clone();
                                    gpui::canvas(
                                        move |rect, _, _| bounds.set(rect),
                                        |_, (), _, _| {},
                                    )
                                    .absolute()
                                    .inset_0()
                                })
                                .disabled(self.checkout_panel.operation.busy || state.is_none())
                                .on_click(cx.listener(
                                    move |this, event: &gpui::ClickEvent, window, cx| {
                                        this.checkout_menu(
                                            false,
                                            idx,
                                            menu_path.clone(),
                                            event.position(),
                                            window,
                                            cx,
                                        );
                                    },
                                )),
                            ),
                    );
                    git_section = git_section.child(self.checkout_failure("recovery", cx));
                    if let Some(state) = state.as_ref() {
                        let identity = state.target_identity();
                        if self.checkout_panel.pr_identity.as_ref() != Some(&identity) {
                            self.checkout_panel.pr_identity = Some(identity);
                            self.checkout_panel.pr_checked_at = None;
                        }
                        if !self.checkout_panel.pr_loading
                            && self.checkout_panel.pr_checked_at.is_none_or(|t| {
                                t.elapsed()
                                    >= if window.is_window_active() {
                                        Duration::from_secs(30)
                                    } else {
                                        Duration::from_mins(5)
                                    }
                            })
                        {
                            self.checkout_load_pr(path.clone(), state.clone(), cx);
                        }
                    }
                    if let Some(state) = state.as_ref() {
                        if self.checkout_panel.main_checked_at.is_none_or(|t| {
                            t.elapsed()
                                >= if window.is_window_active() {
                                    REMOTE_REFRESH_INTERVAL
                                } else {
                                    Duration::from_mins(5)
                                }
                        }) {
                            self.checkout_load_main(path.clone(), state.clone(), cx);
                        }
                    }
                    let target = path.clone();
                    let busy = self.checkout_panel.operation.busy;
                    let same_job =
                        self.checkout_panel.operation.commit_job_path.as_ref() == Some(&path);
                    let same_network = self.checkout_panel.operation.network_target.is_some();
                    let (base_label, base_glyph, base_enabled) = commit_row_state(state.as_ref());
                    let initial_loading =
                        self.checkout_panel.loading && self.checkout_panel.state.is_none();
                    let (label, glyph, enabled, loading) = if let Some((label, glyph)) = self
                        .checkout_panel
                        .operation
                        .network_action
                        .filter(|(_, glyph)| busy && same_network && *glyph != "chev-down")
                    {
                        (label, glyph, false, true)
                    } else if initial_loading {
                        ("Git status not yet available", "git", false, false)
                    } else {
                        match self.checkout_panel.operation.commit_status {
                            CommitUi::Generating if same_job => {
                                ("Generating message… · Cancel", "git", true, true)
                            }
                            CommitUi::Canceling if same_job => ("Canceling…", "stop", false, true),
                            CommitUi::Committing if same_job => ("Committing…", "git", false, true),
                            CommitUi::Succeeded => {
                                (base_label, base_glyph, base_enabled && !busy, false)
                            }
                            CommitUi::Failed => ("Retry commit", "git", !busy, false),
                            _ => (base_label, base_glyph, base_enabled && !busy, false),
                        }
                    };
                    let label = if !loading && base_label == "Push commits" && label == base_label {
                        format!(
                            "Push {} commit{}",
                            state.as_ref().and_then(|s| s.ahead).unwrap_or_default(),
                            if state.as_ref().and_then(|s| s.ahead) == Some(1) {
                                ""
                            } else {
                                "s"
                            }
                        )
                    } else {
                        label.to_string()
                    };
                    let publish = state.as_ref().is_some_and(|state| {
                        !state.dirty
                            && state.upstream.is_none()
                            && state.branch.is_some()
                            && state.head.is_some()
                            && !state.remotes.is_empty()
                    });
                    let dirty = state.as_ref().is_some_and(|state| state.dirty);
                    let retry = matches!(
                        self.checkout_panel.operation.commit_status,
                        CommitUi::Failed
                    );
                    let cancel = same_job
                        && matches!(
                            self.checkout_panel.operation.commit_status,
                            CommitUi::Generating
                        );
                    git_section = git_section.child(
                        checkout_cycle_row(
                            "checkout-commit-publish",
                            label,
                            glyph,
                            loading,
                            self.checkout_panel.feedback.get(&path, "local"),
                            window,
                            cx,
                        )
                        .disabled(
                            !enabled
                                || self.checkout_panel.state_error.is_some()
                                || self
                                    .checkout_panel
                                    .feedback
                                    .get(&path, "local")
                                    .is_some_and(|f| f.completed.is_some()),
                        )
                        .with_focus_handle(self.checkout_panel.trigger_focus["commit"].clone())
                        .relative()
                        .child({
                            let bounds = self.checkout_panel.source_bounds["publish"].clone();
                            gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                                .absolute()
                                .inset_0()
                        })
                        .on_click(cx.listener(
                            move |this, event: &gpui::ClickEvent, window, cx| {
                                if this.checkout_panel.state_error.is_some() {
                                    return;
                                }
                                if cancel {
                                    this.checkout_cancel_commit(cx);
                                } else if dirty || retry {
                                    this.checkout_begin_commit(target.clone(), cx);
                                } else if publish {
                                    this.checkout_menu(
                                        true,
                                        idx,
                                        target.clone(),
                                        event.position(),
                                        window,
                                        cx,
                                    );
                                } else {
                                    this.checkout_run(target.clone(), GitAction::Push, cx);
                                }
                            },
                        )),
                    );
                    let current_action = if dirty || retry {
                        "commit"
                    } else if publish {
                        "publish"
                    } else if base_label == "Push commits" {
                        "push"
                    } else {
                        ""
                    };
                    git_section = git_section.child(self.checkout_failure(current_action, cx));
                    for (action, label, glyph) in [
                        ("commit", "Commit changes", "git"),
                        ("push", "Push commits", "git-push"),
                        ("publish", "Publish branch", "git-publish"),
                    ] {
                        if action != current_action
                            && self.checkout_panel.operation.failures.contains_key(action)
                        {
                            // Preserve the failed operation's context when current data
                            // offers a different action. Recovery still uses current guards.
                            git_section = git_section
                                .child(
                                    checkout_row(
                                        match action {
                                            "commit" => "checkout-commit-failed-context",
                                            "push" => "checkout-push-failed-context",
                                            _ => "checkout-publish-failed-context",
                                        },
                                        label,
                                        glyph,
                                        window,
                                        cx,
                                    )
                                    .disabled(true),
                                )
                                .child(self.checkout_failure(action, cx));
                        }
                    }
                    if dirty {
                        has_changes = true;
                        let summary = self
                            .checkout_panel
                            .summary
                            .clone()
                            .filter(|summary| dirty && summary.files > 0);
                        let totals = match &summary {
                            Some(summary) => {
                                Some(summary.groups.iter().fold((0u32, 0u32), |(a, r), g| {
                                    (a.saturating_add(g.1), r.saturating_add(g.2))
                                }))
                            }
                            None => {
                                let tree = self.runtime.read(cx).tree.read(cx);
                                tree.git_state(crate::paths::normalize_wt_path(&path))
                                    .filter(|changes| {
                                        changes.dirty || changes.added > 0 || changes.removed > 0
                                    })
                                    .map(|changes| (changes.added, changes.removed))
                            }
                        };
                        let stat_widths = change_stat_widths(totals, summary.as_ref(), window);
                        let changes_label = match &summary {
                            Some(summary) if summary.files == 1 => "1 file".to_owned(),
                            Some(summary) => format!("{} files", summary.files),
                            None => "Changes".to_owned(),
                        };
                        let mut changes_row = checkout_loading_row(
                            "checkout-changes",
                            changes_label,
                            "unified",
                            false,
                            window,
                            cx,
                        )
                        .disabled(state.is_none());
                        if let Some((added, removed)) = totals {
                            changes_row =
                                changes_row.child(change_stat_counts(added, removed, stat_widths));
                        }
                        let target = path.clone();
                        changes_section = changes_section.child(changes_row.on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.act(Action::OpenWorktreeDiff(target.clone()), window, cx);
                            },
                        )));
                        if let Some(summary) = summary {
                            for &(group, added, removed) in &summary.groups {
                                let children: Vec<_> = summary
                                    .subgroups
                                    .iter()
                                    .filter(|(parent, _, _, _)| *parent == group)
                                    .collect();
                                let expanded =
                                    self.checkout_panel.expanded_categories.contains(&group);
                                let target = path.clone();
                                let classifier = summary.classifier.clone();
                                let mut row = div().flex().items_center().px(rpx(SPACE_2XL));
                                if !children.is_empty() {
                                    row = row.child(
                                        checkout_action(
                                            format!("checkout-category-toggle-{}", group.label()),
                                            format!(
                                                "{} {} subcategories",
                                                if expanded { "Hide" } else { "Show" },
                                                group.label()
                                            ),
                                            false,
                                            window,
                                            cx,
                                        )
                                        .debug_selector(move || {
                                            format!("checkout-category-toggle-{}", group.label())
                                        })
                                        .icon(
                                            gpui_component::Icon::default()
                                                .path(if expanded {
                                                    "icons/chev-down.svg"
                                                } else {
                                                    "icons/chev-right.svg"
                                                })
                                                .size(rpx(ICON_SM)),
                                        )
                                        .icon_only()
                                        .w(rpx(CONTROL_H))
                                        .h(rpx(CARD_LINE_SM_H))
                                        .flex_shrink_0()
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                if !this
                                                    .checkout_panel
                                                    .expanded_categories
                                                    .remove(&group)
                                                {
                                                    this.checkout_panel
                                                        .expanded_categories
                                                        .insert(group);
                                                }
                                                cx.notify();
                                            }),
                                        ),
                                    );
                                } else {
                                    row = row.child(div().w(rpx(CONTROL_H)).flex_shrink_0());
                                }
                                row = row.child(
                                    checkout_action(
                                        format!("checkout-category-{}", group.label()),
                                        format!("Show {} changes", group.label()),
                                        false,
                                        window,
                                        cx,
                                    )
                                    .debug_selector(move || group.label().into())
                                    .icon_only()
                                    .flex_1()
                                    .min_w_0()
                                    .h(rpx(CARD_LINE_SM_H))
                                    .px(rpx(SPACE_XS))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(rpx(TEXT_SMALL))
                                            .text_color(c::FG_DIM())
                                            .group_hover("neutral-control", |text| {
                                                text.text_color(c::FG())
                                            })
                                            .child(group.label()),
                                    )
                                    .child(change_stat_counts(added, removed, stat_widths))
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.act(
                                                Action::OpenWorktreeDiff(target.clone()),
                                                window,
                                                cx,
                                            );
                                            if let Some(viewer) = &this.diff_viewer {
                                                viewer.update(cx, |viewer, cx| {
                                                    viewer.set_category(
                                                        group,
                                                        None,
                                                        classifier.clone(),
                                                        cx,
                                                    );
                                                });
                                            }
                                        },
                                    )),
                                );
                                changes_section = changes_section.child(row);
                                if expanded {
                                    for (_, child, added, removed) in children {
                                        let target = path.clone();
                                        let child = child.clone();
                                        let label = child.clone();
                                        let selector = format!(
                                            "checkout-subcategory-{}-{child}",
                                            group.label()
                                        );
                                        let classifier = summary.classifier.clone();
                                        changes_section = changes_section.child(
                                            checkout_action(
                                                selector.clone(),
                                                format!("Show {}: {child} changes", group.label()),
                                                false,
                                                window,
                                                cx,
                                            )
                                            .debug_selector(move || selector.clone())
                                            .icon_only()
                                            .w_full()
                                            .h(rpx(CARD_LINE_SM_H))
                                            .px(rpx(SPACE_2XL))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .pl(rpx(CONTROL_H + SPACE_LG))
                                                    .truncate()
                                                    .text_size(rpx(TEXT_SMALL))
                                                    .text_color(c::FG_DIM())
                                                    .group_hover("neutral-control", |text| {
                                                        text.text_color(c::FG())
                                                    })
                                                    .child(label),
                                            )
                                            .child(change_stat_counts(
                                                *added,
                                                *removed,
                                                stat_widths,
                                            ))
                                            .on_click(
                                                cx.listener(move |this, _, window, cx| {
                                                    this.act(
                                                        Action::OpenWorktreeDiff(target.clone()),
                                                        window,
                                                        cx,
                                                    );
                                                    if let Some(viewer) = &this.diff_viewer {
                                                        viewer.update(cx, |viewer, cx| {
                                                            viewer.set_category(
                                                                group,
                                                                Some(child.clone()),
                                                                classifier.clone(),
                                                                cx,
                                                            );
                                                        });
                                                    }
                                                }),
                                            ),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    pr_section = self.render_checkout_pr(&path, state.as_ref(), window, cx);
                    if self.checkout_panel.feedback.get(&path, "pull").is_some()
                        || self.checkout_panel.operation.failures.contains_key("pull")
                        || state
                            .as_ref()
                            .is_some_and(|s| s.behind.is_some_and(|n| n > 0))
                    {
                        let target = path.clone();
                        let count = state.as_ref().and_then(|s| s.behind).unwrap_or(0);
                        git_section = git_section.child(super::action_feedback::retained_row(
                            checkout_cycle_row(
                                "checkout-pull",
                                if count == 0 {
                                    "Pull remote commits".to_owned()
                                } else {
                                    format!(
                                        "Pull {count} remote commit{}",
                                        if count == 1 { "" } else { "s" }
                                    )
                                },
                                "chev-down",
                                false,
                                self.checkout_panel.feedback.get(&path, "pull"),
                                window,
                                cx,
                            )
                            .disabled(
                                self.checkout_panel.operation.busy
                                    || self.checkout_panel.feedback.get(&path, "pull").is_some()
                                    || self.checkout_panel.state_error.is_some()
                                    || state.as_ref().is_none_or(|s| s.dirty || s.ahead != Some(0)),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.checkout_run(target.clone(), GitAction::Pull, cx);
                                },
                            )),
                            self.checkout_panel.feedback.get(&path, "pull"),
                            cx,
                        ));
                    }
                    git_section = git_section.child(self.checkout_failure("pull", cx));
                    if let Some(Ok(Some(main))) = self.checkout_panel.main_state.clone() {
                        if self.checkout_panel.operation.failures.contains_key("main")
                            || ((main.behind > 0
                                || self.checkout_panel.feedback.get(&path, "main").is_some())
                                && !state.as_ref().is_some_and(|state| {
                                    state.upstream.as_deref()
                                        == Some(format!("{}/{}", main.remote, main.branch).as_str())
                                }))
                        {
                            let request = main.clone();
                            let expected = state.clone();
                            git_section = git_section.child(super::action_feedback::retained_row(
                                checkout_cycle_row(
                                    "checkout-main",
                                    format!("{} · {} new commits", main.branch, main.behind),
                                    "git-branch",
                                    false,
                                    self.checkout_panel.feedback.get(&path, "main"),
                                    window,
                                    cx,
                                )
                                .with_focus_handle(
                                    self.checkout_panel.trigger_focus["main"].clone(),
                                )
                                .relative()
                                .child({
                                    let bounds = self.checkout_panel.source_bounds["main"].clone();
                                    gpui::canvas(
                                        move |rect, _, _| bounds.set(rect),
                                        |_, (), _, _| {},
                                    )
                                    .absolute()
                                    .inset_0()
                                })
                                .disabled(
                                    self.checkout_panel.operation.busy
                                        || state.is_none()
                                        || self
                                            .checkout_panel
                                            .feedback
                                            .get(&path, "main")
                                            .is_some()
                                        || self.checkout_panel.main_error.is_some()
                                        || self.checkout_panel.state_error.is_some(),
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        if let Some(expected) = expected.clone() {
                                            this.checkout_panel.recovery = Some(
                                                CheckoutRecovery::Main(expected, request.clone()),
                                            );
                                            this.checkout_panel.return_focus = this
                                                .checkout_panel
                                                .trigger_focus
                                                .get("main")
                                                .cloned();
                                            let focus = this
                                                .checkout_panel
                                                .confirmation_focus
                                                .get_or_insert_with(|| cx.focus_handle());
                                            focus.focus(window, cx);
                                            cx.notify();
                                        }
                                    },
                                )),
                                self.checkout_panel.feedback.get(&path, "main"),
                                cx,
                            ));
                        }
                    }
                    if self
                        .checkout_panel
                        .main_state
                        .as_ref()
                        .and_then(|r| r.as_ref().ok())
                        .and_then(|r| r.as_ref())
                        .is_none()
                    {
                        if self.checkout_panel.operation.failures.contains_key("main") {
                            git_section = git_section.child(
                                checkout_row(
                                    "checkout-main-failed",
                                    "Merge main branch",
                                    "git-branch",
                                    window,
                                    cx,
                                )
                                .disabled(true),
                            );
                        }
                        if let Some(feedback) = self.checkout_panel.feedback.get(&path, "main") {
                            git_section = git_section.child(super::action_feedback::retained_row(
                                checkout_cycle_row(
                                    "checkout-main-feedback",
                                    "",
                                    "git",
                                    false,
                                    Some(feedback),
                                    window,
                                    cx,
                                )
                                .disabled(true),
                                self.checkout_panel.feedback.get(&path, "main"),
                                cx,
                            ));
                        }
                    }
                    git_section = git_section.child(self.checkout_failure("main", cx));
                    if let Some(feedback) = self.checkout_panel.feedback.get(&path, "recovery") {
                        git_section = git_section.child(super::action_feedback::retained_row(
                            checkout_cycle_row(
                                "checkout-recovery-feedback",
                                "",
                                "git",
                                false,
                                Some(feedback),
                                window,
                                cx,
                            )
                            .disabled(true),
                            self.checkout_panel.feedback.get(&path, "recovery"),
                            cx,
                        ));
                    }
                    let diagnostic = self
                        .checkout_panel
                        .state_error
                        .as_ref()
                        .map(|error| ("Git status unavailable", error))
                        .or_else(|| {
                            self.checkout_panel
                                .main_error
                                .as_ref()
                                .map(|error| ("Remote unavailable · last known status", error))
                        });
                    if let Some((label, error)) = diagnostic {
                        git_section = git_section.child(
                            checkout_row(
                                "checkout-status-error",
                                label,
                                "status-unknown",
                                window,
                                cx,
                            )
                            .tooltip(error.clone())
                            .disabled(
                                self.checkout_panel.main_loading || self.checkout_panel.loading,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.checkout_panel.checked_at = None;
                                this.checkout_panel.main_checked_at = None;
                                cx.notify();
                            })),
                        );
                    }
                }
            } else {
                let target = targets.first().cloned();
                card = card.child(
                    checkout_row("checkout-finder", "Choose checkout", "folder", window, cx)
                        .with_focus_handle(self.checkout_panel.trigger_focus["checkout"].clone())
                        .child(icon("chev-down", ICON_SM, c::FG_DIM()))
                        .on_click(cx.listener(
                            move |this, event: &gpui::ClickEvent, window, cx| {
                                if let Some((idx, path)) = &target {
                                    this.checkout_menu(
                                        false,
                                        *idx,
                                        path.clone(),
                                        event.position(),
                                        window,
                                        cx,
                                    );
                                }
                            },
                        )),
                );
                scripts_section = Some(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            div()
                                .px(rpx(SPACE_2XL))
                                .text_size(rpx(TEXT_SMALL))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(c::FG_DIM())
                                .child(
                                    div()
                                        .debug_selector(|| "checkout-scripts-heading".into())
                                        .child("Scripts"),
                                ),
                        )
                        .child(
                            div()
                                .pl(rpx(CHECKOUT_LABEL_INSET))
                                .pr(rpx(SPACE_2XL))
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child("Choose a checkout to view and run scripts."),
                        )
                        .into_any_element(),
                );
            }
        }
        let tools_path = self
            .checkout_panel
            .path
            .clone()
            .or_else(|| linked_description.as_ref().map(|key| key.path.clone()))
            .or_else(|| linked_review.as_ref().map(|key| key.path.clone()));
        if let Some(path) = &tools_path {
            has_changes |= self.has_change_tools(path, true, cx);
            if pr_section.is_none() && self.has_change_tools(path, false, cx) {
                pr_section = self.render_checkout_pr(path, None, window, cx);
            }
        }
        if has_changes {
            card = card
                .child(checkout_divider("checkout-changes-divider"))
                .child(if let Some(path) = &tools_path {
                    self.render_change_tools_heading(path, true, window, cx)
                } else {
                    checkout_section_heading("checkout-changes-heading", "Local changes")
                        .into_any_element()
                })
                .child(changes_section);
        }
        if has_git {
            card = card
                .child(checkout_divider("checkout-divider"))
                .child(git_section);
        }
        if let Some(pr) = pr_section {
            card = card.child(pr);
        }
        if let Some(scripts) = scripts_section {
            card = card
                .child(checkout_divider("checkout-scripts-divider"))
                .child(scripts);
        }
        let plan = (!self.dashboard_open)
            .then(|| self.render_checkout_plan(window, cx))
            .flatten();
        let agents = (!self.dashboard_open)
            .then(|| self.render_checkout_agents(window, cx))
            .flatten();
        if plan.is_some() || agents.is_some() {
            card = card
                .child(checkout_divider("checkout-session-divider"))
                .child(checkout_section_heading(
                    "checkout-session-heading",
                    "Session",
                ))
                .children(plan)
                .children(agents);
        }
        let mut panel = div()
            .id("checkout-panel")
            .debug_selector(|| "checkout-panel".into())
            .relative()
            .flex_shrink_0()
            .min_h_0()
            .when(wide, |col| col.w(rpx(CARD_WIDTH)).h_full())
            .when(!wide, gpui::Styled::w_full)
            .when(wide, gpui::Styled::max_h_full)
            .when(!wide, |panel| {
                panel.max_h(window.viewport_size().height * NARROW_HEIGHT_FRACTION)
            })
            .p(rpx(SPACE_3XL))
            .when(wide, |panel| panel.pt(rpx(SIDE_PANEL_TOP_INSET)))
            .overflow_y_scroll()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.is_zen() && this.checkout_panel.expanded
                {
                    this.checkout_panel.expanded = false;
                    this.checkout_panel.trigger_focus["summary"].focus(window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }));
        if !wide {
            let height = self.checkout_panel.panel_last_narrow_height.clone();
            panel = panel.child(
                gpui::canvas(
                    move |bounds, _, _| height.set(bounds.size.height),
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0(),
            );
        }
        if self.is_zen() {
            panel = panel.child(
                transcript_action(
                    "checkout-summary",
                    if has_plan {
                        "Plan"
                    } else if has_agents {
                        "Agents"
                    } else if no_project {
                        "Project tools"
                    } else {
                        "Checkout"
                    },
                    false,
                    window,
                    cx,
                )
                .icon(
                    gpui_component::Icon::default()
                        .path("icons/panel-right.svg")
                        .size(rpx(ICON_SM)),
                )
                .debug_selector(|| "checkout-summary".into())
                .with_focus_handle(self.checkout_panel.trigger_focus["summary"].clone())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.checkout_panel.expanded = !this.checkout_panel.expanded;
                    this.checkout_panel.trigger_focus["summary"].focus(window, cx);
                    cx.notify();
                })),
            );
        }
        if !self.is_zen() || self.checkout_panel.expanded {
            panel = panel.child(card);
            if self.dashboard_open {
                panel = panel.child(
                    div()
                        .mt(rpx(SPACE_2XL))
                        .child(self.render_contributions(window, cx)),
                );
            } else if let Some(context) = self.render_context_panel(window, cx) {
                panel = panel.child(div().mt(rpx(SPACE_2XL)).child(context));
            }
        } else {
            self.branch_entry = None;
            self.branch_peek = None;
            self.branch_peek_task = None;
        }
        if let Some(popup) = self.checkout_panel.popup.clone() {
            panel = panel.child(gpui::deferred(
                gpui::anchored()
                    .position_mode(gpui::AnchoredPositionMode::Window)
                    .anchor(gpui::Anchor::TopRight)
                    .position(self.checkout_panel.popup_position.unwrap_or_default())
                    .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
                    .child(popup),
            ));
        }
        if wide {
            let body = self.render_content_body(window, cx);
            self.wide_panel_layout(body, panel.into_any_element(), window, cx)
        } else {
            self.checkout_panel.narrow_content = Some(self.narrow_panel_entrance(panel, cx));
            self.render_content_body(window, cx)
        }
    }
}

fn checkout_gate(id: String, reason: &'static str, cx: &App) -> gpui::Stateful<gpui::Div> {
    let kind = if reason.contains("unavailable") || reason.contains("pending") {
        super::super::status::StatusKind::Unknown
    } else {
        super::super::status::StatusKind::Warning
    };
    let label = match reason {
        "Merge conflicts" => "Conflicts",
        "Required review is missing or changes requested" => "Review required or changes requested",
        "GitHub branch policy is blocked, pending or unavailable" => {
            "Branch policy blocked or unavailable"
        }
        "Local checkout has uncommitted changes" => "Uncommitted changes",
        "Local branch has unpushed commits or ahead status is unavailable" => {
            "Unpushed commits or status unavailable"
        }
        "Local branch is behind upstream or behind status is unavailable" => {
            "Behind upstream or status unavailable"
        }
        _ => reason,
    };
    let selector = id.clone();
    div()
        .id(id.clone())
        .debug_selector(move || selector.clone())
        .pl(rpx(CHECKOUT_LABEL_INSET))
        .pr(rpx(SPACE_2XL))
        .py(rpx(SPACE_SM))
        .text_size(rpx(TEXT_SMALL))
        .child(super::super::status::label(id, kind, label, ICON_SM, cx))
        .when(label != reason, |row| {
            row.tooltip(move |window, cx| {
                script_tooltip_content(reason.into(), window).build(window, cx)
            })
        })
}

fn checkout_git_heading() -> gpui::Div {
    checkout_section_heading("checkout-git-heading", "Git")
}

fn checkout_section_heading(id: &'static str, label: &'static str) -> gpui::Div {
    div()
        .px(rpx(SPACE_2XL))
        .h(rpx(CONTROL_H))
        .flex()
        .items_center()
        .text_size(rpx(TEXT_SMALL))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(c::FG_DIM())
        .child(div().debug_selector(move || id.into()).child(label))
}

fn checkout_divider(id: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .w_full()
        .border_t_1()
        .border_color(c::BORDER_SOFT())
        .my(rpx(SPACE_SM))
}

fn checkout_row(
    id: &'static str,
    label: impl Into<SharedString>,
    glyph: &'static str,
    window: &Window,
    cx: &App,
) -> gpui_component::button::Button {
    checkout_loading_row(id, label, glyph, false, window, cx)
}

pub(super) fn checkout_cycle_row(
    id: &'static str,
    label: impl Into<SharedString>,
    glyph: &'static str,
    loading: bool,
    feedback: Option<super::action_feedback::Feedback>,
    window: &Window,
    cx: &App,
) -> gpui_component::button::Button {
    if let Some(feedback) = feedback {
        checkout_feedback_row(
            id,
            feedback.label,
            if feedback.completed.is_some() {
                "status-success"
            } else {
                "status-working"
            },
            feedback.completed.is_none(),
            feedback.completed.map(|started| (feedback.token, started)),
            window,
            cx,
        )
    } else {
        checkout_loading_row(id, label, glyph, loading, window, cx)
    }
}

pub(super) fn checkout_loading_row(
    id: &'static str,
    label: impl Into<SharedString>,
    glyph: &'static str,
    loading: bool,
    window: &Window,
    cx: &App,
) -> gpui_component::button::Button {
    checkout_feedback_row(id, label, glyph, loading, None, window, cx)
}

fn checkout_feedback_row(
    id: &'static str,
    label: impl Into<SharedString>,
    glyph: &'static str,
    loading: bool,
    confirmation: Option<(u64, Instant)>,
    window: &Window,
    cx: &App,
) -> gpui_component::button::Button {
    let label = label.into();
    checkout_action(id, label.clone(), false, window, cx)
        .debug_selector(move || id.into())
        .icon_only()
        .w_full()
        .h(rpx(ROW_H))
        .px(rpx(SPACE_2XL))
        .child(
            div()
                .flex()
                .items_center()
                .flex_1()
                .min_w_0()
                .gap(rpx(SPACE_SM))
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-icon")))
                        .debug_selector(move || format!("{id}-icon"))
                        .w(rpx(ICON_LG))
                        .flex_shrink_0()
                        .child(if let Some((revision, started)) = confirmation {
                            crate::views::status::drawn_check(id, revision, started, ICON_MD, cx)
                        } else if loading {
                            super::super::status::mark(
                                id,
                                super::super::status::StatusKind::Working,
                                ICON_MD,
                                cx,
                            )
                        } else {
                            icon(
                                glyph,
                                ICON_MD,
                                if glyph == "status-success" {
                                    c::STATUS_SUCCESS()
                                } else {
                                    c::FG_DIM()
                                },
                            )
                            .into_any_element()
                        }),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-label")))
                        .debug_selector(move || format!("{id}-label"))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(if loading {
                            c::STATUS_INFO()
                        } else if glyph == "status-success" {
                            c::STATUS_SUCCESS()
                        } else {
                            c::FG()
                        })
                        .child(label),
                ),
        )
}

/// Measure both columns once for the total and every breakdown row.
fn change_stat_widths(
    totals: Option<(u32, u32)>,
    summary: Option<&ChangeSummary>,
    window: &Window,
) -> [gpui::Pixels; 2] {
    let mut widths = [rpx(STAT_COL_W).to_pixels(window.rem_size()); 2];
    let font_size = rpx(TEXT_SMALL).to_pixels(window.rem_size());
    for (added, removed) in totals
        .into_iter()
        .chain(summary.into_iter().flat_map(|summary| {
            summary
                .groups
                .iter()
                .map(|&(_, added, removed)| (added, removed))
                .chain(
                    summary
                        .subgroups
                        .iter()
                        .map(|(_, _, added, removed)| (*added, *removed)),
                )
        }))
    {
        for (column, text) in [format!("+{added}"), format!("−{removed}")]
            .into_iter()
            .enumerate()
        {
            let mut run = window.text_style().to_run(text.len());
            run.font.family = crate::fonts::UI_FAMILY.into();
            run.font.weight = gpui::FontWeight::NORMAL;
            widths[column] = widths[column].max(
                window
                    .text_system()
                    .shape_line(text.into(), font_size, &[run], None)
                    .width()
                    .ceil(),
            );
        }
    }
    widths
}

/// Shared borderless numeric cells keep total and group counts on the same edges.
fn change_stat_counts(added: u32, removed: u32, widths: [gpui::Pixels; 2]) -> impl IntoElement {
    div()
        .flex()
        .flex_shrink_0()
        .gap(rpx(SPACE_SM))
        .text_size(rpx(TEXT_SMALL))
        .children(
            [
                (format!("+{added}"), c::GREEN(), widths[0]),
                (
                    if removed == 0 {
                        String::new()
                    } else {
                        format!("−{removed}")
                    },
                    c::RED(),
                    widths[1],
                ),
            ]
            .into_iter()
            .map(|(text, color, width)| {
                div()
                    .w(width)
                    .flex_shrink_0()
                    .flex()
                    .justify_end()
                    .text_color(color)
                    .child(text)
            }),
        )
}

fn pr_compact_summary(pr: &PullRequest) -> String {
    if pr.is_draft {
        return "Draft".into();
    }
    let mut parts = Vec::new();
    if pr.checks() != "No checks reported" {
        parts.push(pr.compact_status());
    }
    match pr.review_decision.as_str() {
        "CHANGES_REQUESTED" => {
            if !parts.iter().any(|v| v == "Changes requested") {
                parts.push("Changes requested".into());
            }
        }
        "REVIEW_REQUIRED" => parts.push("Review required".into()),
        "APPROVED" => parts.push("Review approved".into()),
        _ => {
            if let Some(requests) = &pr.review_requests {
                if !requests.is_empty() {
                    parts.push(format!("{} awaiting review", requests.len()));
                }
            }
        }
    }
    if parts.is_empty() {
        parts.push(
            if pr.merge_state_status == "CLEAN" {
                "Requirements met"
            } else {
                "Merge status unavailable"
            }
            .into(),
        );
    }
    parts.join(" · ")
}

fn pr_checks_kind(pr: &grove_core::checkout_pr::PullRequest) -> super::super::status::StatusKind {
    use super::super::status::StatusKind;
    match pr.checks() {
        "Checks passing" => StatusKind::Success,
        "Checks failed" => StatusKind::Failed,
        "Checks running" => StatusKind::Working,
        _ => StatusKind::Unknown,
    }
}
fn pr_review_kind(pr: &grove_core::checkout_pr::PullRequest) -> super::super::status::StatusKind {
    use super::super::status::StatusKind;
    match pr.review_decision.as_str() {
        "APPROVED" if pr.review_requests.as_ref().is_none_or(Vec::is_empty) => StatusKind::Success,
        "CHANGES_REQUESTED" => StatusKind::NeedsInput,
        _ if pr.review_requests.as_ref().is_some_and(|r| !r.is_empty()) => StatusKind::Pending,
        _ => StatusKind::Unknown,
    }
}
fn pr_status_kind(pr: &grove_core::checkout_pr::PullRequest) -> super::super::status::StatusKind {
    use super::super::status::StatusKind;
    match pr.state.as_str() {
        "MERGED" => StatusKind::Success,
        "CLOSED" => StatusKind::Stopped,
        _ if pr.is_draft => StatusKind::Pending,
        _ if pr.mergeable == "CONFLICTING" => StatusKind::Warning,
        _ if pr.blocker().is_some() => {
            if pr.checks() == "Checks failed" {
                StatusKind::Failed
            } else {
                StatusKind::NeedsInput
            }
        }
        _ if pr
            .status_check_rollup
            .as_ref()
            .is_some_and(|r| !r.is_empty()) =>
        {
            pr_checks_kind(pr)
        }
        _ => pr_review_kind(pr),
    }
}

fn commit_row_state(state: Option<&CheckoutGitState>) -> (&'static str, &'static str, bool) {
    let Some(state) = state else {
        return ("Git unavailable", "git", false);
    };
    if state.dirty {
        return ("Commit changes", "git", true);
    }
    if state.head.is_none() {
        return ("No commits yet", "git", false);
    }
    if state.branch.is_none() {
        return ("Branch required", "git", false);
    }
    if state.upstream.is_none() {
        return if state.remotes.is_empty() {
            ("No remote configured", "git", false)
        } else {
            ("Publish branch", "git-publish", true)
        };
    }
    if state.ahead.is_some_and(|ahead| ahead > 0) {
        ("Push commits", "git-push", true)
    } else if state.ahead == Some(0) {
        ("No local commits", "git", false)
    } else {
        ("Commit status unavailable", "git", false)
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(test, target_os = "linux"))]
mod push_native_qa;
