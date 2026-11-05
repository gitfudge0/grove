//! Checkout actions live beside the selected canvas, outside its scroll region.
use super::*;
use crate::views::components::{form_action, project_field_well, transcript_action};
use gpui::AnimationExt as _;
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

const CARD_WIDTH: f32 = 280.;
pub(super) const WIDE_VIEWPORT: f32 = MODAL_W_XL + MODAL_W_SM;
const CHECKOUT_SIDE_VIEWPORT: f32 = MODAL_W_XL;
const NARROW_HEIGHT_FRACTION: f32 = 0.25;
const STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const REMOTE_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
/// Minimum count-column width; wider values expand the shared columns together.
const STAT_COL_W: f32 = 48.;
const AGENT_STATUS_W: f32 = ICON_MD;
const AGENT_LINE_H: f32 = 18.;
const SCRIPT_VISIBLE_ROWS: usize = 5;
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

/// Preserve the native button's focus and hit target while using Grove's inverse tooltip.
fn script_control_tooltip(
    id: impl Into<gpui::ElementId>,
    text: impl Into<SharedString>,
    action: gpui_component::button::Button,
) -> gpui::Stateful<gpui::Div> {
    let text = text.into();
    div()
        .id(id)
        .flex_shrink_0()
        .tooltip(move |window, cx| script_tooltip_content(text.clone(), window).build(window, cx))
        .child(action)
}

#[derive(Default)]
pub(super) struct CheckoutPanel {
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
    script_context_scroll: HashMap<String, ScrollHandle>,
    script_run_bounds: HashMap<String, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    script_run_focus: HashMap<String, FocusHandle>,
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

const COMMIT_POLL_INTERVAL: Duration = Duration::from_millis(80);
const COMMIT_SUCCESS_FEEDBACK: Duration = Duration::from_secs(2);

impl CheckoutPanel {
    /// Keep operation state with its checkout while view inspection remains generation scoped.
    fn select_path(&mut self, path: Option<String>) {
        if self.path == path {
            return;
        }
        if let Some(previous) = self.path.take() {
            self.operations
                .insert(previous, std::mem::take(&mut self.operation));
        }
        self.operation = path
            .as_ref()
            .and_then(|path| self.operations.remove(path))
            .unwrap_or_default();
        self.path = path;
    }

    /// Deliver background events to their owner, including while another checkout is visible.
    fn update_operation(&mut self, path: &str, update: impl FnOnce(&mut Self, bool)) {
        let current = self.path.as_deref() == Some(path);
        if current {
            update(self, true);
            return;
        }
        let mut operation = self.operations.remove(path).unwrap_or_default();
        std::mem::swap(&mut self.operation, &mut operation);
        update(self, false);
        std::mem::swap(&mut self.operation, &mut operation);
        self.operations.insert(path.to_owned(), operation);
    }

    fn commit_event(&mut self, path: &str, job: u64, event: CommitEvent) -> bool {
        let mut succeeded = false;
        self.update_operation(path, |panel, current| {
            let operation = &mut panel.operation;
            if operation.commit_job != job {
                return;
            }
            match event {
                CommitEvent::Progress(CommitPhase::Generating) => {
                    if !matches!(operation.commit_status, CommitUi::Canceling) {
                        operation.commit_status = CommitUi::Generating;
                    }
                }
                CommitEvent::Progress(CommitPhase::Committing) => {
                    operation.commit_status = CommitUi::Committing;
                }
                CommitEvent::Finished(finished) => {
                    let (result, state) = *finished;
                    operation.busy = false;
                    operation.commit_control = None;
                    operation.commit_job_path = None;
                    let canceled = matches!(operation.commit_status, CommitUi::Canceling);
                    match result {
                        Ok(outcome) => {
                            succeeded = true;
                            operation.commit_status = CommitUi::Succeeded;
                            operation.message = Some(format!("Committed: {}", outcome.subject));
                        }
                        Err(_) if canceled => {
                            operation.commit_status = CommitUi::Idle;
                            operation.message = Some("Commit canceled.".into());
                        }
                        Err(error) => {
                            operation.commit_status = CommitUi::Failed;
                            operation.message = Some(error);
                        }
                    }
                    if current {
                        panel.state = Some(state);
                        panel.checked_at = Some(Instant::now());
                    }
                }
            }
        });
        succeeded
    }

    fn local_refresh_due(&self) -> bool {
        !self.loading
            && !self.operation.busy
            && (self.state.is_none()
                || self
                    .checked_at
                    .is_none_or(|time| time.elapsed() >= STATUS_REFRESH_INTERVAL))
    }

    fn accepts_sample(&self, path: &str, generation: u64, revision: u64) -> bool {
        self.accepts_result(path, generation)
            && self.operation.revision == revision
            && !self.operation.busy
    }

    fn accepts_result(&self, path: &str, generation: u64) -> bool {
        self.path.as_deref() == Some(path) && self.generation == generation
    }
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

/// A handoff populates a fresh session's composer and never submits a turn.
fn prepare_resolution_draft(
    session: &Entity<crate::entities::codex_session::CodexSession>,
    path: &str,
    provider: grove_core::codex::Provider,
    pr: &PullRequest,
    cx: &mut App,
) -> Result<(), String> {
    session.update(cx, |session, cx| {
        let saved = &session.state.saved;
        if saved.cwd != path || saved.provider != provider || !saved.draft.is_empty() || !saved.items.is_empty() {
            return Err("Could not prepare a fresh conflict-resolution session. Existing drafts were preserved".into());
        }
        let draft = format!("Help resolve merge conflicts for PR #{}: {}\n{}\n\nWork in this checkout: {}. First inspect the current branch, working tree, PR head and base on GitHub. Preserve all existing work. Explain the conflict-resolution approach before changing files. Do not push or merge the PR without my confirmation.", pr.number, pr.title(), pr.url, path);
        session.state.saved.draft.clone_from(&draft);
        session.send(grove_core::codex::Action::Draft(draft), cx);
        Ok(())
    })
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

    fn checkout_load(&mut self, path: String, cx: &mut Context<Self>) {
        if self.checkout_panel.loading || self.checkout_panel.operation.busy {
            return;
        }
        self.checkout_panel.loading = true;
        let generation = self.checkout_panel.generation;
        let revision = self.checkout_panel.operation.revision;
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let (result, summary) = cx
                .background_executor()
                .spawn(async move {
                    let result = checkout_git::inspect(Path::new(&target));
                    let summary = result
                        .as_ref()
                        .is_ok_and(|state| state.dirty)
                        .then(|| diff::summarize_checkout(&target));
                    (result, summary)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.checkout_panel.accepts_result(&path, generation) {
                    if !this
                        .checkout_panel
                        .accepts_sample(&path, generation, revision)
                    {
                        this.checkout_panel.loading = false;
                        this.checkout_panel.checked_at = None;
                        cx.notify();
                        return;
                    }
                    this.checkout_panel.checked_at = Some(Instant::now());
                    match result {
                        Ok(state) => {
                            let previous = this
                                .checkout_panel
                                .state
                                .as_ref()
                                .and_then(|v| v.as_ref().ok());
                            let changed_target = previous.is_some_and(|old| {
                                let mut same_head = state.clone();
                                same_head.head.clone_from(&old.head);
                                same_head.target_identity() != old.target_identity()
                            });
                            let identity = Some(state.target_identity());
                            if this.checkout_panel.pr_identity != identity {
                                this.checkout_panel.pr_identity = identity;
                                this.checkout_panel.pr_checked_at = None;
                                this.checkout_panel.main_checked_at = None;
                                this.checkout_panel.main_state = None;
                                if changed_target {
                                    this.checkout_panel.pr_state = None;
                                    this.checkout_panel.main_state = None;
                                    this.checkout_panel.pr_error = None;
                                    this.checkout_panel.main_error = None;
                                    this.checkout_panel.pr_expanded = false;
                                }
                            }
                            this.checkout_panel.state_error = None;
                            this.checkout_panel.state = Some(Ok(state));
                            this.checkout_panel.summary = summary;
                        }
                        Err(error) => {
                            this.checkout_panel.state_error = Some(error.clone());
                            if this.checkout_panel.state.is_none() {
                                this.checkout_panel.state = Some(Err(error));
                            }
                        }
                    }
                    this.checkout_panel.loading = false;
                    cx.notify();
                }
            });
            cx.background_executor()
                .timer(STATUS_REFRESH_INTERVAL)
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.checkout_panel.accepts_result(&path, generation) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn checkout_load_pr(&mut self, path: String, state: CheckoutGitState, cx: &mut Context<Self>) {
        if self.checkout_panel.pr_loading || self.checkout_panel.operation.busy {
            return;
        }
        self.checkout_panel.pr_loading = true;
        let generation = self.checkout_panel.generation;
        let revision = self.checkout_panel.operation.revision;
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let expected = state.clone();
            let result = cx
                .background_executor()
                .spawn(async move { checkout_pr::inspect(Path::new(&target), &expected) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.checkout_panel.accepts_result(&path, generation) {
                    this.checkout_panel.pr_loading = false;
                    let unchanged = this
                        .checkout_panel
                        .state
                        .as_ref()
                        .and_then(|s| s.as_ref().ok())
                        .is_some_and(|current| {
                            current.target_identity() == state.target_identity()
                        });
                    if unchanged
                        && this
                            .checkout_panel
                            .accepts_sample(&path, generation, revision)
                    {
                        match result {
                            Ok(pr) => {
                                this.checkout_panel.pr_state = Some(Ok(pr));
                                this.checkout_panel.pr_error = None;
                            }
                            Err(error) => {
                                this.checkout_panel.pr_error = Some(error.clone());
                                if this.checkout_panel.pr_state.is_none() {
                                    this.checkout_panel.pr_state = Some(Err(error));
                                }
                            }
                        }
                        this.checkout_panel.pr_checked_at = Some(Instant::now());
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn checkout_load_main(
        &mut self,
        path: String,
        expected: CheckoutGitState,
        cx: &mut Context<Self>,
    ) {
        if self.checkout_panel.main_loading || self.checkout_panel.operation.busy {
            return;
        }
        self.checkout_panel.main_loading = true;
        let generation = self.checkout_panel.generation;
        let revision = self.checkout_panel.operation.revision;
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let snapshot = expected.clone();
            let (result, state) = cx
                .background_executor()
                .spawn(async move {
                    let result = checkout_git::inspect_main(Path::new(&target), &snapshot, true);
                    (result, checkout_git::inspect(Path::new(&target)))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.checkout_panel.accepts_result(&path, generation) {
                    this.checkout_panel.main_loading = false;
                    this.checkout_panel.main_checked_at = Some(Instant::now());
                    let current = this
                        .checkout_panel
                        .state
                        .as_ref()
                        .and_then(|v| v.as_ref().ok());
                    if this
                        .checkout_panel
                        .accepts_sample(&path, generation, revision)
                        && current
                            .is_some_and(|v| v.target_identity() == expected.target_identity())
                    {
                        match result {
                            Ok(main) => {
                                this.checkout_panel.main_state = Some(Ok(main));
                                this.checkout_panel.main_error = None;
                            }
                            Err(error) => {
                                this.checkout_panel.main_error = Some(error.clone());
                                if this.checkout_panel.main_state.is_none() {
                                    this.checkout_panel.main_state = Some(Err(error));
                                }
                            }
                        }
                        if let Ok(state) = state {
                            this.checkout_panel.state = Some(Ok(state));
                        }
                    } else {
                        this.checkout_panel.main_checked_at = None;
                    }
                    cx.notify();
                }
            });
            cx.background_executor()
                .timer(REMOTE_REFRESH_INTERVAL)
                .await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    fn checkout_prepare_recovery(&mut self, path: String, revert: bool, cx: &mut Context<Self>) {
        if self.checkout_panel.operation.busy || self.checkout_panel.state_error.is_some() {
            return;
        }
        let Some(Ok(expected)) = self.checkout_panel.state.clone() else {
            return;
        };
        let generation = self.checkout_panel.generation;
        self.checkout_panel.operation.revision =
            self.checkout_panel.operation.revision.wrapping_add(1);
        self.checkout_panel.operation.busy = true;
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let snapshot = checkout_git::prepare_commit(Path::new(&target))?;
                    if snapshot.target_identity() != expected.target_identity() {
                        return Err("Checkout changed. Review it before retrying".to_owned());
                    }
                    let files = if revert {
                        Vec::new()
                    } else {
                        checkout_git::discard_tracked_paths(Path::new(&target), &snapshot)?
                    };
                    let commits = if revert {
                        grove_core::history::load(&target, 5)?
                            .commits
                            .into_iter()
                            .filter(|v| v.parents.len() <= 1)
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    Ok::<_, String>((snapshot, files, commits))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.checkout_panel
                    .update_operation(&path, |panel, current| {
                        panel.operation.busy = false;
                        if !current || panel.generation != generation {
                            return;
                        }
                        match result {
                            Ok((snapshot, files, commits)) => {
                                if revert {
                                    if let Some(commit) = commits.first() {
                                        panel.recovery = Some(CheckoutRecovery::Revert(
                                            snapshot,
                                            commit.id.clone(),
                                            commit.subject.clone(),
                                        ));
                                    } else {
                                        panel.operation.message = Some(
                                            "No ordinary commit is available to revert".into(),
                                        );
                                        return;
                                    }
                                } else {
                                    if files.is_empty() {
                                        panel.operation.message =
                                            Some("No tracked edits to discard".into());
                                        return;
                                    }
                                    panel.recovery = Some(CheckoutRecovery::Discard(snapshot));
                                }
                                panel.recovery_files = files;
                                panel.revert_commits = commits;
                                panel.operation.message = None;
                                panel.focus_confirmation = true;
                            }
                            Err(error) => panel.operation.message = Some(error),
                        }
                    });
                cx.notify();
            });
        })
        .detach();
    }

    fn checkout_execute_recovery(
        &mut self,
        path: String,
        recovery: CheckoutRecovery,
        cx: &mut Context<Self>,
    ) {
        if self.checkout_panel.operation.busy || self.checkout_panel.path.as_ref() != Some(&path) {
            return;
        }
        let generation = self.checkout_panel.generation;
        self.checkout_panel.operation.revision =
            self.checkout_panel.operation.revision.wrapping_add(1);
        self.checkout_panel.operation.busy = true;
        self.checkout_panel.operation.message = Some("Operation requested".into());
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let (result, state) = cx
                .background_executor()
                .spawn(async move {
                    let result = match recovery {
                        CheckoutRecovery::Discard(expected) => {
                            checkout_git::discard_tracked(Path::new(&target), &expected)
                        }
                        CheckoutRecovery::Revert(expected, commit, _) => {
                            checkout_git::revert_commit(Path::new(&target), &expected, &commit)
                        }
                        CheckoutRecovery::Main(expected, main) => {
                            checkout_git::merge_main(Path::new(&target), &expected, &main)
                        }
                    };
                    (result, checkout_git::inspect(Path::new(&target)))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.checkout_panel
                    .update_operation(&path, |panel, current| {
                        panel.operation.busy = false;
                        if current && panel.generation == generation {
                            panel.state = Some(state);
                            panel.checked_at = None;
                            panel.main_checked_at = None;
                            panel.pr_checked_at = None;
                            match result {
                                Ok(()) => {
                                    panel.recovery = None;
                                    panel.operation.message = None;
                                    panel.restore_focus = true;
                                }
                                Err(error) => panel.operation.message = Some(error),
                            }
                        }
                    });
                cx.notify();
            });
        })
        .detach();
    }

    fn checkout_compose_pr(&mut self, path: String, cx: &mut Context<Self>) {
        if self.checkout_panel.pr_composing || self.checkout_panel.operation.busy {
            return;
        }
        let Some(Ok(state)) = self.checkout_panel.state.clone() else {
            return;
        };
        let generation = self.checkout_panel.generation;
        let identity = state.target_identity();
        self.checkout_panel.pr_composing = true;
        self.checkout_panel.operation.pr_message = None;
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { checkout_pr::compose_url(Path::new(&target), &state) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.checkout_panel.accepts_result(&path, generation) {
                    this.checkout_panel.pr_composing = false;
                    if this
                        .checkout_panel
                        .state
                        .as_ref()
                        .and_then(|s| s.as_ref().ok())
                        .is_none_or(|s| s.target_identity() != identity)
                    {
                        cx.notify();
                        return;
                    }
                    match result {
                        Ok(url) => cx.open_url(&url),
                        Err(error) => this.checkout_panel.operation.pr_message = Some(error),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Typed scripts retain their identity and a fixed, independently focusable Run slot.
    fn render_checkout_scripts(
        &mut self,
        project: &grove_core::storage::Project,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        use grove_core::storage::ProjectActionKind;
        let entries: Vec<_> = project.scripts.effective_entries().into_iter().collect();
        let show_all = self.checkout_panel.scripts_show_all.contains(&project.path);
        let add_path = project.path.clone();
        let manage_path = project.path.clone();
        let mut section = div()
            .id("checkout-scripts")
            .debug_selector(|| "checkout-scripts".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .px(rpx(SPACE_2XL))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .text_size(rpx(TEXT_SMALL))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(c::FG_DIM())
                            .child(
                                div()
                                    .debug_selector(|| "checkout-scripts-heading".into())
                                    .child("Scripts"),
                            ),
                    )
                    .child(script_control_tooltip(
                        "checkout-script-add-tooltip",
                        "Add script",
                        checkout_action("checkout-script-add", "Add script", false, window, cx)
                            .debug_selector(|| "checkout-script-add".into())
                            .icon_only()
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/plus.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.act(
                                    Action::EditProjectScripts(add_path.clone(), true),
                                    window,
                                    cx,
                                );
                            })),
                    ))
                    .child(script_control_tooltip(
                        "checkout-script-manage-tooltip",
                        "Manage project scripts",
                        checkout_action(
                            "checkout-script-manage",
                            "Manage project scripts",
                            false,
                            window,
                            cx,
                        )
                        .debug_selector(|| "checkout-script-manage".into())
                        .icon_only()
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/more.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.act(
                                    Action::EditProjectScripts(manage_path.clone(), false),
                                    window,
                                    cx,
                                );
                            },
                        )),
                    )),
            );
        for entry in entries.iter().take(if show_all {
            usize::MAX
        } else {
            SCRIPT_VISIBLE_ROWS
        }) {
            let (command, prompt) = match &entry.kind {
                ProjectActionKind::Shell { command } => (command, false),
                ProjectActionKind::Prompt { template } => (template, true),
            };
            let context = format!("{}\n{}", entry.name, command);
            let project_path = project.path.clone();
            let target = path.to_owned();
            let entry_id = entry.id.clone();
            let focus_key = format!("{}:{}", project.path, entry.id);
            if !self
                .checkout_panel
                .script_run_focus
                .contains_key(&focus_key)
            {
                let focus = cx.focus_handle();
                self.observers
                    .push(cx.on_focus_in(&focus, window, |_, _, cx| cx.notify()));
                self.observers
                    .push(cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify()));
                self.checkout_panel
                    .script_run_focus
                    .insert(focus_key.clone(), focus);
            }
            let focus = self.checkout_panel.script_run_focus[&focus_key].clone();
            let focused = focus.is_focused(window);
            let context_scroll = self
                .checkout_panel
                .script_context_scroll
                .entry(focus_key.clone())
                .or_default()
                .clone();
            let bounds = self
                .checkout_panel
                .script_run_bounds
                .entry(focus_key)
                .or_default()
                .clone();
            let context_position = bounds.get().bottom_right();
            let selector = format!("checkout-script-{}", entry.id);
            section = section.child(
                crate::views::components::checkout_register_frame()
                    .id(SharedString::from(selector.clone()))
                    .debug_selector(move || selector.clone())
                    .relative()
                    .child(
                        gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                            .absolute()
                            .inset_0(),
                    )
                    .on_key_down({
                        let scroll = context_scroll.clone();
                        move |event: &gpui::KeyDownEvent, window, cx| {
                            if focused {
                                let direction = match event.keystroke.key.as_str() {
                                    "pagedown" => -1.,
                                    "pageup" => 1.,
                                    _ => return,
                                };
                                let mut offset = scroll.offset();
                                offset.y += scroll.bounds().size.height * 0.8 * direction;
                                scroll.set_offset(offset);
                                window.refresh();
                                cx.stop_propagation();
                            }
                        }
                    })
                    .child(
                        crate::views::components::checkout_register_leading().child(icon(
                            if prompt { "reply" } else { "terminal" },
                            ICON_SM,
                            c::FG_DIM(),
                        )),
                    )
                    .child(
                        crate::views::components::checkout_register_label()
                            .id(SharedString::from(format!(
                                "checkout-script-name-{}",
                                entry.id
                            )))
                            .debug_selector({
                                let id = entry.id.clone();
                                move || format!("checkout-script-name-{id}")
                            })
                            .child(entry.name.clone())
                            .tooltip({
                                let context = context.clone();
                                move |window, cx| {
                                    script_tooltip_content(context.clone().into(), window)
                                        .build(window, cx)
                                }
                            }),
                    )
                    .child(script_control_tooltip(
                        SharedString::from(format!("checkout-script-run-tooltip-{}", entry.id)),
                        format!(
                            "{context}\n{}",
                            if prompt {
                                "Send to the current chat."
                            } else {
                                "Run inline in this checkout."
                            }
                        ),
                        checkout_action(
                            SharedString::from(format!("checkout-script-run-{}", entry.id)),
                            format!(
                                "{} {}: {}",
                                if prompt { "Send prompt" } else { "Run script" },
                                entry.name,
                                command
                            ),
                            false,
                            window,
                            cx,
                        )
                        .debug_selector({
                            let id = entry.id.clone();
                            move || format!("checkout-script-run-{id}")
                        })
                        .icon_only()
                        .w(rpx(CONTROL_H))
                        .h(rpx(CONTROL_H))
                        .rounded(rpx(RADIUS_CONTROL).to_pixels(window.rem_size()))
                        .flex_shrink_0()
                        .with_focus_handle(focus)
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/play.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.act(
                                    Action::RunNamedScript(
                                        project_path.clone(),
                                        target.clone(),
                                        entry_id.clone(),
                                    ),
                                    window,
                                    cx,
                                );
                            },
                        )),
                    ))
                    .when(focused, |row| {
                        let margin = rpx(SPACE_LG).to_pixels(window.rem_size());
                        let max_width = window.viewport_size().width - margin * 2.;
                        let max_height = window.viewport_size().height - margin * 2.;
                        row.child(gpui::deferred(
                            gpui::anchored()
                                .position_mode(gpui::AnchoredPositionMode::Window)
                                .anchor(gpui::Anchor::TopRight)
                                .position(context_position)
                                .snap_to_window_with_margin(margin)
                                .child(
                                    div()
                                        .id("checkout-script-focus-context")
                                        .debug_selector(|| "checkout-script-focus-context".into())
                                        .max_w(max_width)
                                        .max_h(max_height)
                                        .overflow_y_scroll()
                                        .track_scroll(&context_scroll)
                                        .child(cx.new(|_| {
                                            script_tooltip_content(
                                                format!(
                                                    "{}\nPage Up/Down scroll command details.\n{}",
                                                    entry.name, command
                                                )
                                                .into(),
                                                window,
                                            )
                                            .max_w(max_width)
                                        })),
                                ),
                        ))
                    }),
            );
            let runs = self
                .runtime
                .read(cx)
                .script_runs
                .iter()
                .filter(|run| {
                    run.project_path == project.path
                        && run.checkout_path == path
                        && run.script_id == entry.id
                })
                .cloned()
                .collect::<Vec<_>>();
            for run in runs {
                section = section.child(self.script_run_row(run, cx));
            }
        }
        if entries.is_empty() {
            let project_path = project.path.clone();
            section = section
                .child(
                    div()
                        .pl(rpx(CHECKOUT_LABEL_INSET))
                        .pr(rpx(SPACE_2XL))
                        .py(rpx(SPACE_LG))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("No scripts yet. Add a shell command or prompt."),
                )
                .child(
                    checkout_row("checkout-script", "Add script", "plus", window, cx).on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.act(
                                Action::EditProjectScripts(project_path.clone(), true),
                                window,
                                cx,
                            );
                        }),
                    ),
                );
        } else if entries.len() > SCRIPT_VISIBLE_ROWS {
            let project_path = project.path.clone();
            section = section.child(
                checkout_action(
                    "checkout-scripts-disclosure",
                    if show_all {
                        "Show fewer".to_owned()
                    } else {
                        format!("Show all {} scripts", entries.len())
                    },
                    false,
                    window,
                    cx,
                )
                .debug_selector(|| "checkout-scripts-disclosure".into())
                .icon_only()
                .w_full()
                .px(rpx(SPACE_2XL))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_1()
                        .min_w_0()
                        .gap(rpx(SPACE_SM))
                        .child(div().w(rpx(ICON_LG)).flex_shrink_0().child(
                            icon("chev-down", ICON_SM, c::FG_DIM()).when(show_all, |glyph| {
                                glyph.with_transformation(gpui::Transformation::rotate(
                                    gpui::percentage(0.5),
                                ))
                            }),
                        ))
                        .child(
                            div()
                                .debug_selector(|| "checkout-scripts-disclosure-label".into())
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(if show_all {
                                    "Show fewer".to_owned()
                                } else {
                                    format!("Show all {} scripts", entries.len())
                                }),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.checkout_panel.scripts_show_all.remove(&project_path) {
                        this.checkout_panel
                            .scripts_show_all
                            .insert(project_path.clone());
                    }
                    cx.notify();
                })),
            );
        }
        section
    }

    fn render_checkout_pr(
        &mut self,
        path: &str,
        state: Option<&CheckoutGitState>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut section = div().flex().flex_col().min_w_0();
        let fresh = self.checkout_panel.pr_error.is_none()
            && self.checkout_panel.state_error.is_none()
            && self.checkout_panel.pr_checked_at.is_some();
        match self.checkout_panel.pr_state.clone() {
            Some(Ok(Some(pr))) => {
                let url = pr.url.clone();
                section = section.child(
                    checkout_row(
                        "checkout-pr",
                        format!(
                            "#{} · {}",
                            pr.number,
                            if pr.state == "MERGED" {
                                "Merged".to_owned()
                            } else {
                                pr.title()
                            }
                        ),
                        "git-pull-request",
                        window,
                        cx,
                    )
                    .tooltip("Open pull request in browser")
                    .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))),
                );
                if pr.state == "OPEN" {
                    let expanded = self.checkout_panel.pr_expanded;
                    let summary = if expanded {
                        "Checks & reviews".to_owned()
                    } else {
                        pr_compact_summary(&pr)
                    };
                    let mut metadata = div()
                        .id("checkout-pr-metadata")
                        .debug_selector(|| "checkout-pr-metadata".into())
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .min_w_0()
                        .pl(rpx(CHECKOUT_LABEL_INSET))
                        .pr(rpx(SPACE_2XL))
                        .gap(rpx(SPACE_SM));
                    if pr.mergeable == "CONFLICTING" {
                        let target = path.to_owned();
                        let request = pr.clone();
                        metadata = metadata.child(
                            checkout_action(
                                "checkout-pr-resolve",
                                "Resolve conflicts",
                                false,
                                window,
                                cx,
                            )
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/git-branch.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .debug_selector(|| "checkout-pr-resolve".into())
                            .disabled(!fresh || self.checkout_panel.operation.busy)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.checkout_resolve_pr(target.clone(), request.clone(), cx)
                                },
                            )),
                        );
                    } else {
                        metadata = metadata.child(
                            crate::views::components::inline_disclosure(
                                "checkout-pr-details",
                                "Checks & reviews",
                            )
                            .debug_selector(|| "checkout-pr-details".into())
                            .aria_expanded(expanded)
                            .w_auto()
                            .min_w_0()
                            .min_h(rpx(CONTROL_H))
                            .justify_start()
                            .gap(rpx(SPACE_SM))
                            .track_focus(&self.checkout_panel.trigger_focus["pr-details"])
                            .tab_index(0)
                            .focus_visible(|style| style.bg(c::BG_HOVER()))
                            .child(icon(
                                if expanded { "chev-down" } else { "chev-right" },
                                ICON_SM,
                                pr_status_kind(&pr).color(),
                            ))
                            .child(
                                div()
                                    .min_w_0()
                                    .whitespace_normal()
                                    .text_size(rpx(TEXT_SMALL))
                                    .child(summary),
                            )
                            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    this.checkout_panel.pr_expanded =
                                        !this.checkout_panel.pr_expanded;
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.checkout_panel.trigger_focus["pr-details"]
                                        .focus(window, cx);
                                    this.checkout_panel.pr_expanded =
                                        !this.checkout_panel.pr_expanded;
                                    cx.notify();
                                },
                            )),
                        );
                        if fresh && state.is_some_and(|state| pr.can_merge(state)) {
                            let request = pr.clone();
                            metadata = metadata.child(
                                checkout_action("checkout-pr-merge", "Merge", false, window, cx)
                                    .icon(
                                        gpui_component::Icon::default()
                                            .path("icons/check.svg")
                                            .size(rpx(ICON_SM)),
                                    )
                                    .debug_selector(|| "checkout-pr-merge".into())
                                    .with_focus_handle(
                                        self.checkout_panel.trigger_focus["pr-merge"].clone(),
                                    )
                                    .disabled(self.checkout_panel.operation.busy)
                                    .relative()
                                    .child({
                                        let bounds =
                                            self.checkout_panel.source_bounds["pr-merge"].clone();
                                        gpui::canvas(
                                            move |rect, _, _| bounds.set(rect),
                                            |_, (), _, _| {},
                                        )
                                        .absolute()
                                        .inset_0()
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.checkout_panel.return_focus = this
                                            .checkout_panel
                                            .trigger_focus
                                            .get("pr-merge")
                                            .cloned();
                                        this.checkout_panel.pr_merge = Some(request.clone());
                                        this.checkout_panel.operation.message = None;
                                        let focus = this
                                            .checkout_panel
                                            .confirmation_focus
                                            .get_or_insert_with(|| cx.focus_handle());
                                        focus.focus(window, cx);
                                        cx.notify();
                                    })),
                            );
                        }
                    }
                    section = section.child(metadata);
                    if expanded && pr.mergeable != "CONFLICTING" {
                        let mut details = div()
                            .id("checkout-pr-expanded")
                            .debug_selector(|| "checkout-pr-expanded".into())
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .pl(rpx(CHECKOUT_LABEL_INSET))
                            .pr(rpx(SPACE_2XL))
                            .py(rpx(SPACE_SM))
                            .gap(rpx(SPACE_SM))
                            .text_size(rpx(TEXT_SMALL));
                        for check in pr.status_check_rollup.as_deref().unwrap_or_default() {
                            let name = check["name"]
                                .as_str()
                                .or(check["context"].as_str())
                                .unwrap_or("Check");
                            let outcome = check["conclusion"]
                                .as_str()
                                .or(check["state"].as_str())
                                .filter(|v| !v.is_empty())
                                .unwrap_or("Pending");
                            details = details.child(
                                div()
                                    .min_w_0()
                                    .whitespace_normal()
                                    .child(format!("{name} · {outcome}")),
                            );
                        }
                        details = details.child(
                            div()
                                .debug_selector(|| "checkout-pr-checks".into())
                                .child(pr.review_progress()),
                        );
                        for (index, reason) in pr
                            .remote_readiness_reasons()
                            .into_iter()
                            .filter(|reason| {
                                !reason.contains("review")
                                    && !reason.contains("check")
                                    && *reason != "Merge conflicts"
                            })
                            .enumerate()
                        {
                            details = details.child(
                                checkout_gate(format!("checkout-pr-gate-{index}"), reason, cx)
                                    .pl_0(),
                            );
                        }
                        if state.is_some_and(|s| {
                            !s.dirty
                                && s.ahead == Some(0)
                                && s.behind == Some(0)
                                && s.head.as_deref() != Some(pr.head_ref_oid.as_str())
                        }) {
                            details = details.child("Checkout HEAD differs from the PR head");
                        }
                        if pr.checks() == "Checks failed" {
                            let target = path.to_owned();
                            let request = pr.clone();
                            details = details.child(
                                checkout_action(
                                    "checkout-pr-fix-ci",
                                    "Fix failed CI",
                                    false,
                                    window,
                                    cx,
                                )
                                .icon(
                                    gpui_component::Icon::default()
                                        .path("icons/play.svg")
                                        .size(rpx(ICON_SM)),
                                )
                                .disabled(!fresh || self.checkout_panel.pr_prompt_busy)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.checkout_pr_prompt(target.clone(), request.clone(), cx)
                                    },
                                )),
                            );
                        }
                        section = section.child(details);
                    }
                }
            }
            Some(Ok(None)) => {
                let ready = state.is_some_and(|s| {
                    !s.dirty && s.branch.is_some() && s.upstream.is_some() && s.ahead == Some(0)
                });
                if ready {
                    let target = path.to_owned();
                    section = section.child(
                        checkout_row("checkout-pr", "Create PR…", "git-pull-request", window, cx)
                            .disabled(
                                !fresh
                                    || self.checkout_panel.operation.busy
                                    || self.checkout_panel.pr_composing,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.checkout_compose_pr(target.clone(), cx)
                            })),
                    );
                }
            }
            Some(Err(_)) | None => {}
        }
        if let Some(error) = &self.checkout_panel.pr_error {
            section = section.child(
                checkout_row(
                    "checkout-pr-refresh",
                    if self
                        .checkout_panel
                        .pr_state
                        .as_ref()
                        .is_some_and(|v| v.is_ok())
                    {
                        "PR offline · last known status"
                    } else {
                        "PR status unavailable"
                    },
                    "status-unknown",
                    window,
                    cx,
                )
                .tooltip(error.clone())
                .disabled(self.checkout_panel.pr_loading)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.checkout_panel.pr_checked_at = None;
                    cx.notify();
                })),
            );
        }
        if self.checkout_panel.pr_state.is_none() && self.checkout_panel.pr_error.is_none() {
            section = section.child(
                checkout_row(
                    "checkout-pr-unavailable",
                    "PR status not yet available",
                    "status-unknown",
                    window,
                    cx,
                )
                .disabled(true),
            );
        }
        if let Some(message) = &self.checkout_panel.operation.pr_message {
            section = section.child(
                div()
                    .pl(rpx(CHECKOUT_LABEL_INSET))
                    .pr(rpx(SPACE_2XL))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(message.clone()),
            );
        }
        section.into_any_element()
    }

    fn checkout_pr_prompt(&mut self, path: String, pr: PullRequest, cx: &mut Context<Self>) {
        if self.checkout_panel.pr_prompt_busy || self.checkout_panel.path.as_ref() != Some(&path) {
            return;
        }
        let Some(Selection::Session(id)) = self.selection else {
            self.checkout_panel.operation.pr_message =
                Some("Open a native Codex or Claude chat in this checkout first".into());
            cx.notify();
            return;
        };
        let Some(session) = self.runtime.read(cx).registry.read(cx).codex(id).cloned() else {
            return;
        };
        if session.read(cx).state.saved.cwd != path {
            self.checkout_panel.operation.pr_message =
                Some("The current chat belongs to another checkout".into());
            cx.notify();
            return;
        }
        let Some(Ok(state)) = self.checkout_panel.state.clone() else {
            return;
        };
        let thread = session.read(cx).state.saved.thread_id.clone();
        let generation = self.checkout_panel.generation;
        self.checkout_panel.pr_prompt_busy = true;
        self.checkout_panel.operation.pr_message = Some("Refreshing pull request context…".into());
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let result = cx.background_executor().spawn(async move { checkout_pr::action_prompt(Path::new(&target), &state, &pr) }).await;
            let _ = this.update(cx, |this, cx| {
                if !this.checkout_panel.accepts_result(&path, generation) { return; }
                this.checkout_panel.pr_prompt_busy = false;
                let result = if this.selection != Some(Selection::Session(id)) || session.read(cx).state.saved.cwd != path || session.read(cx).state.saved.thread_id != thread {
                    Err("Current chat changed while preparing the PR action. Retry in the intended chat".into())
                } else { result.and_then(|prompt| session.update(cx, |session,cx| session.project_prompt(prompt,cx)).map(|_| ())) };
                this.checkout_panel.operation.pr_message = Some(match result { Ok(()) => "PR action requested in the current chat".into(), Err(error) => error });
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn checkout_resolve_pr(&mut self, path: String, pr: PullRequest, cx: &mut Context<Self>) {
        if self.checkout_panel.operation.busy
            || self.checkout_panel.path.as_ref() != Some(&path)
            || pr.state != "OPEN"
            || pr.mergeable != "CONFLICTING"
        {
            return;
        }
        if !crate::runtime::native_codex_enabled(&cx.global::<SettingsState>().store, None) {
            self.checkout_panel.operation.pr_message =
                Some("Enable native agent sessions to prepare conflict resolution".into());
            cx.notify();
            return;
        }
        let provider = match self.checkout_commit_provider(cx) {
            Ok(provider) => provider,
            Err(error) => {
                self.checkout_panel.operation.pr_message = Some(error);
                cx.notify();
                return;
            }
        };
        let agent = if provider == grove_core::codex::Provider::Claude {
            Agent::Claude
        } else {
            Agent::Codex
        };
        let Some((idx, _)) = self
            .checkout_targets(cx)
            .into_iter()
            .find(|(_, target)| target == &path)
        else {
            return;
        };

        if self.launch(idx, path.clone(), agent, cx) {
            if let Some(Selection::Session(id)) = self.selection {
                let session = self.runtime.read(cx).registry.read(cx).codex(id).cloned();
                if let Some(session) = session {
                    if let Err(error) = prepare_resolution_draft(&session, &path, provider, &pr, cx)
                    {
                        self.show_launch_error(error, cx);
                    }
                }
            }
        }
    }

    fn checkout_merge_pr(&mut self, path: String, pr: PullRequest, cx: &mut Context<Self>) {
        if self.checkout_panel.operation.busy || self.checkout_panel.path.as_ref() != Some(&path) {
            return;
        }
        let Some(Ok(expected)) = self.checkout_panel.state.clone() else {
            return;
        };
        let generation = self.checkout_panel.generation;
        self.checkout_panel.operation.revision =
            self.checkout_panel.operation.revision.wrapping_add(1);
        self.checkout_panel.operation.busy = true;
        self.checkout_panel.operation.message = Some("Merge requested".into());
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { checkout_pr::merge(Path::new(&target), &expected, &pr) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.checkout_panel
                    .update_operation(&path, |panel, current| {
                        panel.operation.busy = false;
                        panel.operation.message = None;
                        panel.operation.pr_message = Some(
                            result.map_or_else(|error| error, |()| "Pull request merged".into()),
                        );
                        if current && panel.generation == generation {
                            panel.pr_checked_at = None;
                            panel.pr_state = None;
                            panel.checked_at = None;
                            panel.restore_focus = panel.pr_merge.is_some();
                            panel.pr_merge = None;
                        }
                    });
                cx.notify();
            });
        })
        .detach();
    }

    fn checkout_run(&mut self, path: String, action: GitAction, cx: &mut Context<Self>) {
        if self.checkout_panel.operation.busy || self.checkout_panel.path.as_ref() != Some(&path) {
            return;
        }
        let Some(Ok(expected)) = self.checkout_panel.state.clone() else {
            return;
        };
        let generation = self.checkout_panel.generation;
        self.checkout_panel.operation.revision =
            self.checkout_panel.operation.revision.wrapping_add(1);
        self.checkout_panel.operation.busy = true;
        self.checkout_panel.operation.network_target = Some((path.clone(), generation));
        self.checkout_panel.operation.network_action = Some(match action {
            GitAction::Publish { .. } => ("Publish requested", "git-publish"),
            GitAction::Pull => ("Pull requested", "chev-down"),
            _ => ("Pushing…", "git-push"),
        });
        self.checkout_panel.operation.message = None;
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let (result, state) = cx
                .background_executor()
                .spawn(async move {
                    let result = checkout_git::execute(Path::new(&target), &expected, action);
                    (result, checkout_git::inspect(Path::new(&target)))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.checkout_panel
                    .update_operation(&path, |panel, current| {
                        panel.operation.busy = false;
                        panel.operation.network_action = None;
                        panel.operation.network_target = None;
                        if current {
                            panel.checked_at = Some(Instant::now());
                            panel.state = Some(state);
                            if result.is_ok() {
                                panel.restore_focus =
                                    panel.publish.is_some() && panel.generation == generation;
                                panel.publish = None;
                                panel.pr_checked_at = None;
                                panel.main_checked_at = None;
                            }
                        }
                        panel.operation.message =
                            Some(result.map_or_else(|error| error, |()| String::new()));
                    });
                cx.notify();
            });
            cx.background_executor()
                .timer(STATUS_REFRESH_INTERVAL)
                .await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    fn checkout_commit_provider(&self, cx: &App) -> Result<grove_core::codex::Provider, String> {
        let selected = match self.selection {
            Some(Selection::Session(id)) => self
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .meta(id)
                .map(|meta| meta.agent),
            _ => None,
        };
        commit_provider(selected, cx.global::<SettingsState>().store.default_agent)
    }

    fn checkout_begin_commit(&mut self, path: String, cx: &mut Context<Self>) {
        if self.checkout_panel.operation.busy || self.checkout_panel.path.as_ref() != Some(&path) {
            return;
        }
        let provider = match self.checkout_commit_provider(cx) {
            Ok(provider) => provider,
            Err(error) => {
                self.checkout_panel.operation.commit_status = CommitUi::Failed;
                self.checkout_panel.operation.message = Some(error);
                cx.notify();
                return;
            }
        };
        self.checkout_panel.operation.commit_job =
            self.checkout_panel.operation.commit_job.wrapping_add(1);
        let job = self.checkout_panel.operation.commit_job;
        let control = std::sync::Arc::new(CommitControl::default());
        self.checkout_panel.operation.commit_control = Some(control.clone());
        self.checkout_panel.operation.commit_job_path = Some(path.clone());
        self.checkout_panel.operation.commit_status = CommitUi::Generating;
        self.checkout_panel.operation.revision =
            self.checkout_panel.operation.revision.wrapping_add(1);
        self.checkout_panel.operation.busy = true;
        self.checkout_panel.operation.message = None;
        let (send, receive) = std::sync::mpsc::channel();
        let target = path.clone();
        cx.background_executor()
            .spawn(async move {
                let result = commit_agent::commit_with_control(
                    Path::new(&target),
                    provider,
                    &control,
                    |phase| {
                        let _ = send.send(CommitEvent::Progress(phase));
                    },
                );
                let state = checkout_git::inspect(Path::new(&target));
                let _ = send.send(CommitEvent::Finished(Box::new((result, state))));
            })
            .detach();
        cx.spawn(async move |this, cx| {
            let mut succeeded = false;
            'updates: loop {
                loop {
                    match receive.try_recv() {
                        Ok(event) => {
                            let finished = matches!(event, CommitEvent::Finished(..));
                            let updated = this.update(cx, |this, cx| {
                                succeeded |= this.checkout_panel.commit_event(&path, job, event);
                                cx.notify();
                            });
                            if updated.is_err() || finished { break 'updates; }
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            let _ = this.update(cx, |this, cx| {
                                this.checkout_panel.update_operation(&path, |panel, _| {
                                    if panel.operation.commit_job == job {
                                        panel.operation.busy = false;
                                        panel.operation.commit_control = None;
                                        panel.operation.commit_job_path = None;
                                        panel.operation.commit_status = CommitUi::Failed;
                                        panel.operation.message = Some("Commit generation ended unexpectedly. Retry the commit.".into());
                                    }
                                });
                                cx.notify();
                            });
                            break 'updates;
                        }
                    }
                }
                cx.background_executor().timer(COMMIT_POLL_INTERVAL).await;
            }
            if succeeded {
                cx.background_executor().timer(COMMIT_SUCCESS_FEEDBACK).await;
                let _ = this.update(cx, |this, cx| {
                    this.checkout_panel.update_operation(&path, |panel, _| {
                        if panel.operation.commit_job == job {
                            panel.operation.commit_status = CommitUi::Idle;
                        }
                    });
                    cx.notify();
                });
            }
            cx.background_executor().timer(STATUS_REFRESH_INTERVAL).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        }).detach();
        cx.notify();
    }

    fn checkout_cancel_commit(&mut self, cx: &mut Context<Self>) {
        if let Some(control) = &self.checkout_panel.operation.commit_control {
            self.checkout_panel.operation.commit_status = if control.cancel() {
                CommitUi::Canceling
            } else {
                CommitUi::Committing
            };
            cx.notify();
        }
    }

    pub(super) fn checkout_cancel_confirmation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.checkout_panel.operation.busy {
            return;
        }
        self.checkout_panel.publish = None;
        self.checkout_panel.pr_merge = None;
        self.checkout_panel.recovery = None;
        self.checkout_panel.operation.message = None;
        if let Some(focus) = self.checkout_panel.return_focus.take() {
            focus.focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn render_checkout_confirmation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(path) = self.checkout_panel.path.clone() else {
            return div().into_any_element();
        };
        let busy = self.checkout_panel.operation.busy;
        let mut form = div()
            .w_full()
            .max_w(rpx(MODAL_W_SM))
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_3XL));
        let mut actions = crate::views::components::form_actions().child(
            form_action("checkout-cancel-confirmation", "Cancel", false, window, cx)
                .icon(
                    gpui_component::Icon::default()
                        .path("icons/close.svg")
                        .size(rpx(ICON_SM)),
                )
                .with_focus_handle(self.cancel_focus.clone())
                .disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.checkout_cancel_confirmation(window, cx);
                })),
        );
        if let Some(remote) = self.checkout_panel.publish.clone() {
            let branch = self
                .checkout_panel
                .state
                .as_ref()
                .and_then(|state| state.as_ref().ok())
                .and_then(|state| state.branch.clone())
                .unwrap_or_default();
            let publish_path = path.clone();
            form = form
                .child(div().text_size(rpx(TEXT_DISPLAY)).child("Publish branch?"))
                .child(format!("Publish {branch} to existing remote {remote}."))
                .child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(path.clone()),
                );
            actions = actions.child(
                form_action(
                    "checkout-confirm-publish",
                    "Publish branch",
                    true,
                    window,
                    cx,
                )
                .icon(
                    gpui_component::Icon::default()
                        .path("icons/git-publish.svg")
                        .size(rpx(ICON_SM)),
                )
                .with_focus_handle(self.confirm_focus.clone())
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.checkout_run(
                        publish_path.clone(),
                        GitAction::Publish {
                            remote: remote.clone(),
                        },
                        cx,
                    );
                })),
            );
        }
        if let Some(pr) = self.checkout_panel.pr_merge.clone() {
            let merge_path = path.clone();
            form = form
                .child(div().text_size(rpx(TEXT_DISPLAY)).child("Merge pull request?"))
                .child(format!("Merge PR #{} with a merge commit: {}", pr.number, pr.title()))
                .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child(pr.url.clone()))
                .child("The exact PR head, checks, reviews and checkout will be revalidated. Repository protections apply.");
            actions = actions.child(
                form_action(
                    "checkout-confirm-merge",
                    if busy {
                        "Merge requested"
                    } else {
                        "Create merge commit"
                    },
                    true,
                    window,
                    cx,
                )
                .debug_selector(|| "checkout-confirm-merge".into())
                .icon(
                    gpui_component::Icon::default()
                        .path("icons/check.svg")
                        .size(rpx(ICON_SM)),
                )
                .with_focus_handle(self.confirm_focus.clone())
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.checkout_merge_pr(merge_path.clone(), pr.clone(), cx);
                })),
            );
        }
        if let Some(recovery) = self.checkout_panel.recovery.clone() {
            let (heading,description,action)=match &recovery {
                CheckoutRecovery::Discard(_) => ("Discard tracked edits?".to_owned(),"This removes the captured tracked edits, including staged edits. Untracked files are preserved. This cannot be undone here.".to_owned(),"Discard edits"),
                CheckoutRecovery::Revert(_,commit,title) => ("Revert a commit?".to_owned(),format!("Create an inverse commit for {} · {title}. Existing history stays intact.",&commit[..commit.len().min(8)]),"Create revert commit"),
                CheckoutRecovery::Main(_,main) => (format!("New commits on {}",main.branch),format!("{} commits on {}/{} are not in this branch. Merging creates a local merge commit; it does not change the remote branch.",main.behind,main.remote,main.branch),"Merge into local branch"),
            };
            form = form
                .child(div().text_size(rpx(TEXT_DISPLAY)).child(heading))
                .child(description);
            match &recovery {
                CheckoutRecovery::Discard(_) => {
                    for file in &self.checkout_panel.recovery_files {
                        form = form.child(div().text_size(rpx(TEXT_SMALL)).child(file.clone()));
                    }
                }
                CheckoutRecovery::Revert(expected, selected, _) => {
                    for (i, commit) in self.checkout_panel.revert_commits.iter().enumerate() {
                        let snapshot = expected.clone();
                        let id = commit.id.clone();
                        let title = commit.subject.clone();
                        form = form.child(
                            checkout_action(
                                format!("checkout-revert-choice-{i}"),
                                format!("{} · {}", &id[..id.len().min(8)], title),
                                false,
                                window,
                                cx,
                            )
                            .icon(
                                gpui_component::Icon::default()
                                    .path(if &id == selected {
                                        "icons/check.svg"
                                    } else {
                                        "icons/git.svg"
                                    })
                                    .size(rpx(ICON_SM)),
                            )
                            .disabled(busy)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.checkout_panel.recovery = Some(CheckoutRecovery::Revert(
                                        snapshot.clone(),
                                        id.clone(),
                                        title.clone(),
                                    ));
                                    cx.notify();
                                },
                            )),
                        );
                    }
                }
                _ => {}
            }
            let target = path.clone();
            let guarded = match &recovery {
                CheckoutRecovery::Main(expected, _) => {
                    expected.dirty || expected.behind != Some(0) && expected.upstream.is_some()
                }
                _ => false,
            };
            if guarded {
                form=form.child("Commit or discard edits and synchronize this checkout before integrating the remote branch.");
            }
            actions = actions.child(
                form_action("checkout-confirm-recovery", action, true, window, cx)
                    .debug_selector(|| "checkout-confirm-recovery".into())
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/check.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .with_focus_handle(self.confirm_focus.clone())
                    .disabled(busy || guarded)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.checkout_execute_recovery(target.clone(), recovery.clone(), cx)
                    })),
            );
        }
        if let Some(message) = &self.checkout_panel.operation.message {
            form = form.child(
                div()
                    .id("checkout-operation-message")
                    .role(gpui::Role::Status)
                    .text_color(c::FG_DIM())
                    .child(message.clone()),
            );
        }
        form = form.child(actions);

        div()
            .id("checkout-confirmation")
            .debug_selector(|| "checkout-confirmation".into())
            .role(gpui::Role::Dialog)
            .aria_label("Confirm checkout operation")
            .when_some(
                self.checkout_panel.confirmation_focus.as_ref(),
                gpui::InteractiveElement::track_focus,
            )
            .w(rpx(MODAL_W_SM))
            .max_w_full()
            .max_h(rpx(MODAL_SCROLL_MAX_H))
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .p(rpx(SPACE_3XL))
            .bg(c::SURFACE_RAISED())
            .border_1()
            .border_color(c::BORDER_SOFT())
            .rounded(rpx(RADIUS_PANEL))
            .flex()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.checkout_cancel_confirmation(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(form)
            .into_any_element()
    }

    fn render_checkout_agents(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Some(Selection::Session(id)) = self.selection else {
            self.checkout_panel.agent_observer = None;
            return None;
        };
        let Some(session) = self.runtime.read(cx).registry.read(cx).codex(id).cloned() else {
            self.checkout_panel.agent_observer = None;
            return None;
        };
        let saved = &session.read(cx).state.saved;
        let provider = saved.provider.agent().icon_name();
        let periods = saved.work_periods.clone();
        let agents = checkout_agents(&periods);
        if agents.is_empty() {
            return None;
        }
        let running = agents
            .iter()
            .filter(|agent| agent.status == grove_core::codex::SubagentStatus::Working)
            .count();
        let (active, previous) = checkout_agent_groups(&agents, &periods);
        let disclosure = self.checkout_panel.agent_disclosures.entry(id).or_default();
        let show_all = disclosure.show_all;
        let show_previous = disclosure.previous;
        let mut section = div()
            .id("checkout-agents")
            .debug_selector(|| "checkout-agents".into())
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .child(
                div()
                    .h(rpx(AGENT_ROW_H))
                    .px(rpx(SPACE_2XL))
                    .flex()
                    .items_center()
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .child(format!("Agents · {running} running")),
            )
            .children(
                active
                    .iter()
                    .take(if show_all { usize::MAX } else { AGENTS_VISIBLE })
                    .map(|agent| self.agent_peek_row(id, agent, provider, cx)),
            );
        if active.len() > AGENTS_VISIBLE {
            section = section.child(
                checkout_row(
                    "checkout-agents-more",
                    if show_all {
                        "Show fewer".to_owned()
                    } else {
                        format!("Show {} more", active.len() - AGENTS_VISIBLE)
                    },
                    "plus",
                    window,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let disclosure = this.checkout_panel.agent_disclosures.entry(id).or_default();
                    disclosure.show_all = !disclosure.show_all;
                    cx.notify();
                })),
            );
        }
        if !previous.is_empty() {
            section = section.child(
                checkout_action(
                    "checkout-agents-previous",
                    format!(
                        "{} previous agents ({})",
                        if show_previous { "Collapse" } else { "Expand" },
                        previous.len()
                    ),
                    false,
                    window,
                    cx,
                )
                .debug_selector(|| "checkout-agents-previous".into())
                .icon_only()
                .w_full()
                .h(rpx(AGENT_ROW_H))
                .px(rpx(SPACE_2XL))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_SM))
                        .w_full()
                        .min_w_0()
                        .text_size(rpx(TEXT_MICRO))
                        .text_color(c::FG_DIM())
                        .child(icon("chev-down", ICON_SM, c::FG_DIM()).when(
                            show_previous,
                            |glyph| {
                                glyph.with_transformation(gpui::Transformation::rotate(
                                    gpui::percentage(0.5),
                                ))
                            },
                        ))
                        .child(format!("Previous agents ({})", previous.len()))
                        .child(
                            div()
                                .flex_1()
                                .min_w(rpx(SPACE_LG))
                                .border_t_1()
                                .border_color(c::BORDER_SOFT()),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let disclosure = this.checkout_panel.agent_disclosures.entry(id).or_default();
                    disclosure.previous = !disclosure.previous;
                    cx.notify();
                })),
            );
            if show_previous {
                section = section.children(
                    previous
                        .iter()
                        .map(|agent| self.agent_peek_row(id, agent, provider, cx)),
                );
            }
        }
        let visible: std::collections::HashSet<_> = active
            .iter()
            .take(if show_all { usize::MAX } else { AGENTS_VISIBLE })
            .chain(previous.iter().filter(|_| show_previous))
            .map(|agent| agent.id.as_str())
            .collect();
        if self
            .checkout_panel
            .peek
            .as_ref()
            .is_some_and(|peek| !visible.contains(peek.child.as_str()))
        {
            self.checkout_panel.peek = None;
            self.checkout_panel.peek_task = None;
            self.checkout_panel.peek_hover_target = None;
        }
        Some(section.into_any_element())
    }

    pub(super) fn toggle_session_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was_visible = !self.checkout_panel.hidden;
        if self.files_active && self.files_owner == self.selection {
            self.files_active = false;
            self.checkout_panel.hidden = false;
            self.checkout_panel.expanded = true;
            cx.notify();
            return;
        }
        if self.checkout_panel.side_chat_owner.take().is_some() {
            self.checkout_panel.hidden = false;
            self.checkout_panel.expanded = true;
            self.start_panel_motion(was_visible, true, cx);
            cx.notify();
            return;
        }
        self.checkout_panel.hidden = !self.checkout_panel.hidden;
        if self.checkout_panel.hidden {
            self.dismiss_agent_peek(window, cx);
            self.dismiss_plan_peek(window, cx);
            self.dismiss_script_peek(window, cx);
            self.checkout_panel.popup = None;
            self.checkout_panel.popup_subscription = None;
        }
        self.start_panel_motion(was_visible, !self.checkout_panel.hidden, cx);
        cx.notify();
    }

    pub(super) fn start_panel_motion(&mut self, was_visible: bool, visible: bool, cx: &App) {
        if cx.reduce_motion() {
            self.checkout_panel.panel_motion = None;
            return;
        }
        let from = self
            .panel_motion_width()
            .unwrap_or(if was_visible { 1. } else { 0. });
        self.checkout_panel.panel_motion_revision =
            self.checkout_panel.panel_motion_revision.wrapping_add(1);
        self.checkout_panel.panel_motion = Some((
            Instant::now(),
            from,
            if visible { 1. } else { 0. },
            self.checkout_panel.panel_motion_revision,
        ));
    }

    fn panel_motion_width(&self) -> Option<f32> {
        self.checkout_panel
            .panel_motion
            .map(|(started, from, to, _)| {
                let progress = (started.elapsed().as_secs_f32() / (MOTION_BASE_MS as f32 / 1000.))
                    .clamp(0., 1.);
                from + (to - from) * motion::ease_standard(progress)
            })
    }

    #[cfg(test)]
    pub(super) fn hold_panel_motion_at_start(&mut self) {
        if let Some((started, _, _, _)) = self.checkout_panel.panel_motion.as_mut() {
            // Draw the exact first frame independently of machine/render latency.
            *started = Instant::now() + Duration::from_secs(1);
        }
    }

    pub(super) fn narrow_panel_entrance<E: IntoElement + gpui::Styled + 'static>(
        &self,
        panel: E,
        cx: &App,
    ) -> AnyElement {
        match self.checkout_panel.panel_motion {
            Some((started, _, _, id))
                if started.elapsed() < Duration::from_millis(MOTION_BASE_MS) =>
            {
                motion::fast(panel, format!("narrow-session-panel-enter-{id}"), cx)
            }
            _ => panel.into_any_element(),
        }
    }

    fn wide_panel_layout(
        &mut self,
        body: AnyElement,
        panel: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut width = 1.;
        let mut revision = None;
        if !cx.reduce_motion() {
            if let Some((started, _, _, id)) = self.checkout_panel.panel_motion {
                if started.elapsed() < Duration::from_millis(MOTION_BASE_MS) {
                    width = self.panel_motion_width().unwrap_or(1.);
                    revision = Some(id);
                    window.request_animation_frame();
                } else {
                    self.checkout_panel.panel_motion = None;
                }
            }
        } else {
            self.checkout_panel.panel_motion = None;
        }
        let panel = if let Some(id) = revision {
            motion::fast(
                div().size_full().child(panel),
                format!("session-panel-enter-{id}"),
                cx,
            )
        } else {
            panel
        };
        let panel_width = if !self.dashboard_open
            && self.checkout_panel.side_chat_owner.is_some()
            && self.checkout_panel.side_chat_owner == self.selection
        {
            super::side_chat::SIDE_CHAT_WIDTH
        } else if self.checkout_panel.hidden && self.checkout_panel.panel_last_wide_width > 0. {
            self.checkout_panel.panel_last_wide_width
        } else {
            CARD_WIDTH
        };
        if !self.checkout_panel.hidden {
            self.checkout_panel.panel_last_wide_width = panel_width;
        }
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .relative()
            .flex()
            .child(div().flex_1().min_w_0().min_h_0().child(body))
            .child(
                div()
                    .w(rpx(panel_width * width))
                    .h_full()
                    .flex_shrink_0()
                    .overflow_hidden()
                    .child(panel),
            )
            .into_any_element()
    }

    pub(super) fn render_checkout_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = self.render_checkout_container_body(window, cx);
        if !self.checkout_panel.toggle_available {
            return body;
        }
        let side_chat_available = !self.dashboard_open
            && match self.selection {
                Some(Selection::Session(id) | Selection::Home(id)) => self
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .codex(id)
                    .is_some_and(|session| !session.read(cx).is_deferred()),
                _ => false,
            };
        let controls = self.side_container_controls(side_chat_available, window, cx);
        // This container survives Home/session navigation and panel dismissal.
        // Keep the toolbar outside the animated body so reopening stays reachable.
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .relative()
            .child(body)
            .child(
                div()
                    .id("side-container-controls")
                    .debug_selector(|| "side-container-controls".into())
                    .absolute()
                    .top_0()
                    .right_0()
                    .h(rpx(APPBAR_H))
                    .px(rpx(SPACE_20))
                    .flex()
                    .items_center()
                    .child(controls),
            )
            .into_any_element()
    }

    fn render_checkout_container_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let wide = !self.is_zen() && Self::logical_window_width(window) >= CHECKOUT_SIDE_VIEWPORT;
        self.checkout_panel.agents_visible = !self.is_zen() || self.checkout_panel.expanded;
        let selected = selected_checkout(&targets, self.checkout_panel.chosen_root.as_deref());
        let mut card = project_field_well(false, false)
            .id("checkout-card")
            .debug_selector(|| "checkout-card".into())
            .h_auto()
            .justify_start()
            .w_full()
            .px_0()
            .py(rpx(SPACE_SM))
            .gap(rpx(SPACE_XS));
        if !agents_only {
            if let Some((idx, path)) = selected {
                if self.checkout_panel.path.as_ref() != Some(&path) {
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
                if self.checkout_panel.local_refresh_due() {
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
                card = card
                    .child(checkout_divider("checkout-scripts-divider"))
                    .child(self.render_checkout_scripts(&project, &path, window, cx));
                if self
                    .checkout_panel
                    .state
                    .as_ref()
                    .is_some_and(Result::is_ok)
                {
                    card = card
                        .child(checkout_divider("checkout-divider"))
                        .child(checkout_git_heading());
                    let state = self.checkout_panel.state.clone().and_then(Result::ok);
                    let branch_label = state.as_ref().map_or_else(
                        || {
                            if self
                                .checkout_panel
                                .state
                                .as_ref()
                                .is_some_and(Result::is_err)
                            {
                                "Branch unavailable".to_owned()
                            } else {
                                "Branch unavailable".to_owned()
                            }
                        },
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
                    card = card.child(
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
                                .icon_only()
                                .icon(
                                    gpui_component::Icon::default()
                                        .path("icons/more.svg")
                                        .size(rpx(ICON_SM)),
                                )
                                .with_focus_handle(
                                    self.checkout_panel.trigger_focus["recovery"].clone(),
                                )
                                .tooltip("Discard local edits or revert a commit")
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
                                        )
                                    },
                                )),
                            ),
                    );
                    if let Some(state) = state.as_ref() {
                        let identity = state.target_identity();
                        if self.checkout_panel.pr_identity.as_ref() != Some(&identity) {
                            self.checkout_panel.pr_identity = Some(identity);
                            self.checkout_panel.pr_checked_at = None;
                        }
                        if !self.checkout_panel.pr_loading
                            && self
                                .checkout_panel
                                .pr_checked_at
                                .is_none_or(|t| t.elapsed() >= Duration::from_secs(30))
                        {
                            self.checkout_load_pr(path.clone(), state.clone(), cx);
                        }
                    }
                    if let Some(state) = state.as_ref() {
                        if self
                            .checkout_panel
                            .main_checked_at
                            .is_none_or(|t| t.elapsed() >= REMOTE_REFRESH_INTERVAL)
                        {
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
                        .filter(|_| busy && same_network)
                    {
                        (label, glyph, false, glyph == "git-push")
                    } else if initial_loading {
                        ("Git status not yet available", "git", false, false)
                    } else {
                        match self.checkout_panel.operation.commit_status {
                            CommitUi::Generating if same_job => {
                                ("Commit requested · Cancel", "git", true, false)
                            }
                            CommitUi::Canceling if same_job => {
                                ("Cancel requested", "stop", false, false)
                            }
                            CommitUi::Committing if same_job => {
                                ("Commit requested", "git", false, false)
                            }
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
                    card = card.child(checkout_loading_row("checkout-commit-publish", label, glyph, loading, window, cx)
                .disabled(!enabled || self.checkout_panel.state_error.is_some())
                .with_focus_handle(self.checkout_panel.trigger_focus["commit"].clone())
                .relative()
                .child({ let bounds = self.checkout_panel.source_bounds["publish"].clone();
                    gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {}).absolute().inset_0() })
                .tooltip(self.checkout_panel.operation.message.clone().unwrap_or_else(|| {
                    self.checkout_panel.state.as_ref().and_then(|state| state.as_ref().err()).cloned().unwrap_or_else(||
                        "Generate a commit message and commit staged changes, or all nonignored changes when nothing is staged.".into())
                }))
                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    if this.checkout_panel.state_error.is_some() { return; }
                    if cancel { this.checkout_cancel_commit(cx); }
                    else if dirty || retry { this.checkout_begin_commit(target.clone(), cx); }
                    else if publish { this.checkout_menu(true, idx, target.clone(), event.position(), window, cx); }
                    else { this.checkout_run(target.clone(), GitAction::Push, cx); }
                })));
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
                            let git = self.runtime.read(cx).tree.read(cx).git_states();
                            git.get(crate::paths::normalize_wt_path(&path))
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
                    card = card.child(changes_row.on_click(cx.listener(
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
                            let expanded = self.checkout_panel.expanded_categories.contains(&group);
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
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
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
                                        },
                                    )),
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
                            card = card.child(row);
                            if expanded {
                                for (_, child, added, removed) in children {
                                    let target = path.clone();
                                    let child = child.clone();
                                    let label = child.clone();
                                    let selector =
                                        format!("checkout-subcategory-{}-{child}", group.label());
                                    let classifier = summary.classifier.clone();
                                    card = card.child(
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
                                        .child(change_stat_counts(*added, *removed, stat_widths))
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
                    card = card
                        .child(checkout_divider("checkout-pr-divider"))
                        .child(self.render_checkout_pr(&path, state.as_ref(), window, cx));
                    if state
                        .as_ref()
                        .is_some_and(|s| s.behind.is_some_and(|n| n > 0))
                    {
                        let target = path.clone();
                        let count = state.as_ref().and_then(|s| s.behind).unwrap_or(0);
                        card = card.child(
                            checkout_row(
                                "checkout-pull",
                                format!("Pull {count} remote commits"),
                                "chev-down",
                                window,
                                cx,
                            )
                            .disabled(
                                self.checkout_panel.operation.busy
                                    || self.checkout_panel.state_error.is_some()
                                    || state.as_ref().is_none_or(|s| s.dirty || s.ahead != Some(0)),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.checkout_run(target.clone(), GitAction::Pull, cx)
                                },
                            )),
                        );
                    }
                    if let Some(Ok(Some(main))) = self.checkout_panel.main_state.clone() {
                        if main.behind > 0
                            && !state.as_ref().is_some_and(|state| {
                                state.upstream.as_deref()
                                    == Some(format!("{}/{}", main.remote, main.branch).as_str())
                            })
                        {
                            let request = main.clone();
                            let expected = state.clone();
                            card = card.child(
                                checkout_row(
                                    "checkout-main",
                                    format!("{} · {} new commits", main.branch, main.behind),
                                    "git-branch",
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
                            );
                        }
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
                        card = card.child(
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
                    if !self.checkout_confirmation_open()
                        && self
                            .checkout_panel
                            .operation
                            .message
                            .as_ref()
                            .is_some_and(|m| !m.is_empty())
                        && matches!(
                            self.checkout_panel.operation.commit_status,
                            CommitUi::Failed | CommitUi::Idle
                        )
                    {
                        card = card.child(
                            div()
                                .pl(rpx(CHECKOUT_LABEL_INSET))
                                .pr(rpx(SPACE_2XL))
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(
                                    self.checkout_panel
                                        .operation
                                        .message
                                        .clone()
                                        .unwrap_or_default(),
                                ),
                        );
                    }
                }
            } else {
                let target = targets.first().cloned();
                card = card
                    .child(
                        checkout_row("checkout-finder", "Choose checkout", "folder", window, cx)
                            .with_focus_handle(
                                self.checkout_panel.trigger_focus["checkout"].clone(),
                            )
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
                    )
                    .child(checkout_divider("checkout-scripts-divider"))
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
                    .child(
                        checkout_row("checkout-script", "Add script", "plus", window, cx)
                            .disabled(true),
                    )
                    .child(checkout_divider("checkout-divider"))
                    .child(checkout_git_heading())
                    .child(
                        checkout_row(
                            "checkout-commit-publish",
                            "Commit changes",
                            "git",
                            window,
                            cx,
                        )
                        .disabled(true),
                    )
                    .child(
                        checkout_row("checkout-changes", "Changes", "unified", window, cx)
                            .disabled(true),
                    );
            }
        }
        if let Some(plan) = (!self.dashboard_open)
            .then(|| self.render_checkout_plan(window, cx))
            .flatten()
        {
            card = card
                .child(checkout_divider("checkout-plan-divider"))
                .child(plan);
        }
        if let Some(agents) = (!self.dashboard_open)
            .then(|| self.render_checkout_agents(window, cx))
            .flatten()
        {
            card = card
                .child(checkout_divider("checkout-agents-divider"))
                .child(agents);
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
            .when(wide, |panel| panel.pt(rpx(APPBAR_TOTAL_H + SPACE_3XL)))
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

    fn checkout_menu(
        &mut self,
        publish: bool,
        idx: usize,
        path: String,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_branch_peek(window, cx);
        self.checkout_panel.return_focus = self
            .checkout_panel
            .trigger_focus
            .get(if publish { "commit" } else { "recovery" })
            .cloned();
        self.checkout_panel.popup_position = Some(position);
        if self.checkout_panel.confirmation_focus.is_none() {
            self.checkout_panel.confirmation_focus = Some(cx.focus_handle());
        }
        let owner = cx.entity().downgrade();
        let state = self.checkout_panel.state.clone().and_then(Result::ok);
        let project_path = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .map(|project| project.path.clone())
            .unwrap_or_else(|| path.clone());
        let targets = self.checkout_targets(cx);
        let popup = PopupMenu::build(window, cx, move |mut popup, window, _| {
            popup = crate::views::components::dropdown_menu(popup, window);
            if publish {
                for remote in state
                    .as_ref()
                    .map(|state| state.remotes.clone())
                    .unwrap_or_default()
                {
                    let owner = owner.clone();
                    popup = popup.item(
                        PopupMenuItem::new(format!("Publish to {remote}…")).on_click(
                            move |_, _, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    this.checkout_panel.publish = Some(remote.clone());
                                    this.checkout_panel.operation.message = None;
                                    cx.notify();
                                });
                            },
                        ),
                    );
                }
            } else {
                for (label, revert, enabled) in [
                    (
                        "Discard local edits…",
                        false,
                        state.as_ref().is_some_and(|s| s.dirty),
                    ),
                    (
                        "Revert a commit…",
                        true,
                        state.as_ref().is_some_and(|s| !s.dirty && s.head.is_some()),
                    ),
                ] {
                    if enabled {
                        let owner = owner.clone();
                        let target = path.clone();
                        popup = popup.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                this.checkout_panel.return_focus =
                                    this.checkout_panel.trigger_focus.get("recovery").cloned();
                                this.checkout_prepare_recovery(target.clone(), revert, cx);
                            });
                        }));
                    }
                }
                if idx != usize::MAX && state.as_ref().is_some_and(|state| state.head.is_some()) {
                    let owner = owner.clone();
                    let project_path = project_path.clone();
                    popup = popup.item(PopupMenuItem::new("New worktree…").on_click(
                        move |_, window, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                this.checkout_panel.return_focus = None;
                                this.act(Action::NewWorktree(project_path.clone()), window, cx);
                            });
                        },
                    ));
                }
                if targets.len() > 1 {
                    for (_, target) in targets.clone() {
                        let owner = owner.clone();
                        popup = popup.item(
                            PopupMenuItem::new(target.clone())
                                .checked(target == path)
                                .on_click(move |_, _, cx| {
                                    let _ = owner.update(cx, |this, cx| {
                                        this.checkout_panel.chosen_root = Some(target.clone());
                                        this.checkout_panel.select_path(None);
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                }
            }
            popup
        });
        self.checkout_panel.popup_subscription = Some(cx.subscribe_in(
            &popup,
            window,
            |this, _, _: &gpui::DismissEvent, window, cx| {
                this.checkout_panel.popup = None;
                this.checkout_panel.popup_subscription = None;
                if this.checkout_panel.publish.is_some() {
                    if let Some(focus) = &this.checkout_panel.confirmation_focus {
                        focus.focus(window, cx);
                    }
                } else if let Some(focus) = &this.checkout_panel.return_focus {
                    focus.focus(window, cx);
                }
                cx.notify();
            },
        ));
        popup.focus_handle(cx).focus(window, cx);
        self.checkout_panel.popup = Some(popup);
        cx.notify();
    }
}

fn commit_provider(
    selected: Option<Agent>,
    configured: Option<Agent>,
) -> Result<grove_core::codex::Provider, String> {
    match selected
        .filter(|agent| matches!(agent, Agent::Codex | Agent::Claude))
        .or(configured)
    {
        Some(Agent::Codex) => Ok(grove_core::codex::Provider::Codex),
        Some(Agent::Claude) => Ok(grove_core::codex::Provider::Claude),
        _ => Err("Choose Codex or Claude Code as your default agent to generate commits.".into()),
    }
}

const AGENTS_VISIBLE: usize = 5;
const AGENT_ROW_H: f32 = 32.;

/// Later work periods replace earlier observations, including terminal states.
fn checkout_agents(periods: &[grove_core::codex::WorkPeriod]) -> Vec<grove_core::codex::Subagent> {
    let mut latest: Vec<grove_core::codex::Subagent> = Vec::new();
    for agent in periods.iter().flat_map(|period| &period.subagents) {
        if let Some(previous) = latest.iter_mut().find(|previous| previous.id == agent.id) {
            *previous = agent.clone();
        } else {
            latest.push(agent.clone());
        }
    }
    latest
}

fn checkout_agent_groups<'a>(
    agents: &'a [grove_core::codex::Subagent],
    periods: &[grove_core::codex::WorkPeriod],
) -> (
    Vec<&'a grove_core::codex::Subagent>,
    Vec<&'a grove_core::codex::Subagent>,
) {
    let latest_ids: std::collections::HashSet<_> = periods
        .last()
        .into_iter()
        .flat_map(|period| &period.subagents)
        .map(|agent| agent.id.as_str())
        .collect();
    agents.iter().partition(|agent| {
        latest_ids.contains(agent.id.as_str())
            || matches!(
                agent.status,
                grove_core::codex::SubagentStatus::Working
                    | grove_core::codex::SubagentStatus::Unknown
            )
    })
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
        .debug_selector(move || selector.clone().into())
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
    div()
        .px(rpx(SPACE_2XL))
        .h(rpx(CONTROL_H))
        .flex()
        .items_center()
        .text_size(rpx(TEXT_SMALL))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(c::FG_DIM())
        .child(
            div()
                .debug_selector(|| "checkout-git-heading".into())
                .child("Git"),
        )
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

/// Presentation uses only observed provider text; terminal activity is not an outcome.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct AgentPresentation {
    pub(super) name: String,
    pub(super) activity: Option<String>,
    pub(super) status: &'static str,
    detail: String,
}

fn normalized_agent_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn informative_agent_activity(text: &str) -> Option<String> {
    let text = normalized_agent_text(text);
    (!text.is_empty() && !text.eq_ignore_ascii_case("Activity unavailable")).then_some(text)
}

pub(super) fn checkout_agent_presentation(
    agent: &grove_core::codex::Subagent,
) -> AgentPresentation {
    use grove_core::codex::SubagentStatus;
    let status = match agent.status {
        SubagentStatus::Working => "Working",
        SubagentStatus::Completed => "Done",
        SubagentStatus::Failed => "Failed",
        SubagentStatus::Interrupted => "Interrupted",
        SubagentStatus::Unknown => "Unknown",
    };
    let name = normalized_agent_text(&agent.name);
    let name = if name.is_empty() {
        "Agent".into()
    } else {
        name
    };
    let task = agent
        .task
        .as_deref()
        .map(normalized_agent_text)
        .filter(|s| !s.is_empty());
    let observed = informative_agent_activity(&agent.activity);
    let activity = observed.or_else(|| task.clone());
    let mut detail = format!("{name} · {status}");
    if let Some(task) = task {
        detail.push_str(" · Task: ");
        detail.push_str(&task);
    }
    if let Some(observed) = informative_agent_activity(&agent.activity) {
        detail.push_str(" · Latest observed activity: ");
        detail.push_str(&observed);
    }
    AgentPresentation {
        name,
        activity,
        status,
        detail,
    }
}

pub(super) fn checkout_agent_row(
    agent: &grove_core::codex::Subagent,
    provider: &'static str,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let presentation = checkout_agent_presentation(agent);
    let name_id = agent.id.clone();
    let activity_id = agent.id.clone();
    let status_id = agent.id.clone();
    let provider_label = if provider == Agent::Claude.icon_name() {
        "Claude"
    } else {
        "Codex"
    };
    let detail = format!("{provider_label} · {}", presentation.detail);
    div()
        .id(SharedString::from(format!("checkout-agent-{}", agent.id)))
        .role(gpui::Role::Button)
        .group("neutral-control")
        .tab_index(0)
        .focus_visible(|style| style.bg(c::BG_HOVER()))
        .hover(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
        .aria_label(detail.clone())
        .px(rpx(SPACE_2XL))
        .py(rpx(SPACE_SM))
        .flex()
        .flex_col()
        .min_w_0()
        .text_size(rpx(TEXT_BODY))
        .font_weight(gpui::FontWeight::NORMAL)
        .text_color(c::FG_DIM())
        .line_height(rpx(AGENT_LINE_H))
        .child(
            div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_SM))
                .min_w_0()
                .child(
                    div()
                        .flex_shrink_0()
                        .child(icon(provider, ICON_SM, c::FG())),
                )
                .child(
                    div()
                        .debug_selector(move || format!("checkout-agent-name-{name_id}"))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(presentation.name),
                )
                .child(
                    div()
                        .debug_selector(move || format!("checkout-agent-status-{status_id}"))
                        .w(rpx(AGENT_STATUS_W))
                        .flex_shrink_0()
                        .h(rpx(AGENT_LINE_H))
                        .flex()
                        .items_center()
                        .justify_end()
                        .child(status_icon_sized(
                            format!("checkout-agent-{}", agent.id).into(),
                            presentation.status,
                            ICON_MD,
                            cx,
                        )),
                ),
        )
        // Reserve one compact supporting line so activity updates keep rows stable.
        .child(
            div()
                .debug_selector(move || format!("checkout-agent-activity-{activity_id}"))
                .h(rpx(AGENT_LINE_H))
                .w_full()
                .min_w_0()
                .truncate()
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .group_hover("neutral-control", |text| text.text_color(c::FG()))
                .child(presentation.activity.unwrap_or_default()),
        )
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

/// A fixed upstream bar with a gently rising arrow; layout and label never move.
fn checkout_push_mark(id: &'static str, cx: &App) -> AnyElement {
    const PUSH_CYCLE: Duration = Duration::from_millis(1150);
    const PUSH_TRAVEL: f32 = 1.0;
    let arrow = div()
        .absolute()
        .top_0()
        .left_0()
        .size(rpx(ICON_MD))
        .child(icon("git-push-arrow", ICON_MD, c::STATUS_INFO()));
    let arrow = if cx.reduce_motion() {
        arrow.into_any_element()
    } else {
        arrow
            .with_animation(
                SharedString::from(format!("{id}-push-rise")),
                gpui::Animation::new(PUSH_CYCLE).repeat(),
                |el, progress| {
                    let rise = (1. - (progress * std::f32::consts::TAU).cos()) * 0.5;
                    el.top(rpx(PUSH_TRAVEL * (1. - 2. * rise)))
                        .opacity(0.4 + 0.6 * rise)
                },
            )
            .into_any_element()
    };
    div()
        .debug_selector(|| "checkout-push-mark".into())
        .relative()
        .size(rpx(ICON_MD))
        .child(icon("git-push-bar", ICON_MD, c::STATUS_INFO()))
        .child(arrow)
        .into_any_element()
}

fn checkout_loading_row(
    id: &'static str,
    label: impl Into<SharedString>,
    glyph: &'static str,
    loading: bool,
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
                        .child(if loading && glyph == "git-push" {
                            checkout_push_mark(id, cx)
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
mod tests {
    use super::*;
    #[gpui::test]
    fn projectless_provider_sessions_keep_directory_targets(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            let mut sidebar = Sidebar::new(runtime.clone(), window, cx);
            sidebar.dashboard_open = false;
            for agent in [Agent::Codex, Agent::Claude] {
                let id = runtime.read(cx).registry.clone().update(cx, |registry, _| {
                    registry.insert_meta(String::new(), "/tmp/projectless-fixture".into(), agent)
                });
                sidebar.selection = Some(Selection::Session(id));
                assert_eq!(
                    sidebar.checkout_targets(cx),
                    vec![(usize::MAX, "/tmp/projectless-fixture".into())]
                );
            }
            sidebar
        });
        let _ = cx;
    }

    #[test]
    fn invalidated_cached_status_is_polled_again() {
        let mut panel = CheckoutPanel::default();
        panel.state = Some(Err("Last observation unavailable".into()));
        assert!(panel.local_refresh_due());
        panel.checked_at = Some(Instant::now());
        assert!(!panel.local_refresh_due());
        panel.checked_at = None;
        panel.loading = true;
        assert!(!panel.local_refresh_due());
        panel.loading = false;
        panel.operation.busy = true;
        assert!(!panel.local_refresh_due());
        panel.operation.busy = false;
        assert!(panel.local_refresh_due());
    }

    #[test]
    fn background_samples_cannot_overwrite_a_completed_mutation() {
        let mut panel = CheckoutPanel::default();
        panel.path = Some("/checkout".into());
        panel.generation = 4;
        assert!(panel.accepts_sample("/checkout", 4, 0));
        panel.operation.revision += 1;
        panel.operation.busy = true;
        assert!(!panel.accepts_sample("/checkout", 4, 0));
        panel.operation.busy = false;
        assert!(!panel.accepts_sample("/checkout", 4, 0));
        assert!(panel.accepts_sample("/checkout", 4, 1));
        panel.generation += 1;
        assert!(!panel.accepts_sample("/checkout", 4, 1));
        assert!(!panel.accepts_sample("/other", 5, 1));
    }

    #[test]
    fn background_commit_provider_follows_session_then_supported_default() {
        use grove_core::codex::Provider;
        assert_eq!(
            commit_provider(Some(Agent::Claude), Some(Agent::Codex)),
            Ok(Provider::Claude)
        );
        assert_eq!(
            commit_provider(Some(Agent::Terminal), Some(Agent::Codex)),
            Ok(Provider::Codex)
        );
        assert!(commit_provider(Some(Agent::OpenCode), Some(Agent::OpenCode)).is_err());
        assert!(commit_provider(None, None).is_err());
    }
    #[test]
    fn multiple_roots_require_a_current_explicit_choice() {
        let targets = vec![(0, "/one".into()), (1, "/two".into())];
        assert_eq!(selected_checkout(&targets, None), None);
        assert_eq!(selected_checkout(&targets, Some("/removed")), None);
        assert_eq!(
            selected_checkout(&targets, Some("/two")),
            Some((1, "/two".into()))
        );
        assert_eq!(
            selected_checkout(&targets[..1], None),
            Some((0, "/one".into()))
        );
    }
    #[gpui::test]
    fn codex_and_claude_resolution_handoff_only_drafts_in_fresh_checkout(
        cx: &mut gpui::TestAppContext,
    ) {
        for provider in [
            grove_core::codex::Provider::Codex,
            grove_core::codex::Provider::Claude,
        ] {
            let (handle, actions) = grove_core::codex::Handle::channel();
            let (_updates, receive) = std::sync::mpsc::channel();
            let saved = grove_core::codex::SavedSession {
                session_review: Default::default(),
                rich_plans: grove_core::rich_plan::PlanStore::default(),
                side_chats: Default::default(),
                fork_origin: None,
                claude_resume: None,
                claude_subagents: std::collections::HashMap::default(),
                edit_checkpoints: Vec::new(),
                pending_edit: None,
                rewind_drafts: Vec::new(),
                provider,
                composer: grove_core::codex::ComposerSettings::default(),
                context: None,
                key: "conflict-fixture".into(),
                project: "Fixture".into(),
                cwd: "/fixture/checkout".into(),
                label: provider.label().into(),
                title: None,
                title_generation: None,
                roots: Vec::new(),
                thread_id: None,
                items: Vec::new().into(),
                work_periods: Vec::new(),
                draft: String::new(),
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
                        handle,
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            let pr: PullRequest = serde_json::from_value(serde_json::json!({"number":142,"title":"Fix checkout","url":"https://github.com/o/r/pull/142","state":"OPEN","isDraft":false,"headRefOid":"head","mergeable":"CONFLICTING"})).unwrap();
            cx.update(|cx| {
                assert!(
                    prepare_resolution_draft(&session, "/wrong/checkout", provider, &pr, cx)
                        .is_err()
                );
                assert!(
                    prepare_resolution_draft(&session, "/fixture/checkout", provider, &pr, cx)
                        .is_ok()
                );
            });
            let draft = session.read_with(cx, |session, _| session.state.saved.draft.clone());
            assert!(
                draft.contains("PR #142")
                    && draft.contains("/fixture/checkout")
                    && draft.contains("Do not push or merge")
            );
            assert!(
                matches!(actions.try_recv().unwrap(), grove_core::codex::Action::Draft(text) if text == draft)
            );
            assert!(actions.try_recv().is_err());
            cx.update(|cx| {
                assert!(
                    prepare_resolution_draft(&session, "/fixture/checkout", provider, &pr, cx)
                        .is_err()
                );
            });
            assert_eq!(
                session.read_with(cx, |session, _| session.state.saved.draft.clone()),
                draft
            );
            assert!(actions.try_recv().is_err());
        }
    }

    #[gpui::test]
    fn codex_and_claude_named_prompt_dispatch_preserves_drafts(cx: &mut gpui::TestAppContext) {
        for provider in [
            grove_core::codex::Provider::Codex,
            grove_core::codex::Provider::Claude,
        ] {
            let project = scripts_project(0);
            let path = project.path.clone();
            cx.update(|cx| {
                gpui_component::init(cx);
                c::sync_component_theme(cx);
                cx.set_global(SettingsState::new(grove_core::storage::Store {
                    projects: vec![project.clone()],
                    ..Default::default()
                }));
                cx.set_global(crate::zoom::CurrentPtyDims::default());
                cx.set_global(crate::zoom::ZoomState::new(1.0));
            });
            let (handle, actions) = grove_core::codex::Handle::channel();
            let (_updates, receive) = std::sync::mpsc::channel();
            let saved = grove_core::codex::SavedSession {
                session_review: Default::default(),
                rich_plans: grove_core::rich_plan::PlanStore::default(),
                side_chats: Default::default(),
                fork_origin: None,
                claude_resume: None,
                claude_subagents: std::collections::HashMap::default(),
                edit_checkpoints: Vec::new(),
                pending_edit: None,
                rewind_drafts: Vec::new(),
                provider,
                composer: grove_core::codex::ComposerSettings::default(),
                context: None,
                key: "named-script-fixture".into(),
                project: project.name.clone(),
                cwd: path.clone(),
                label: provider.label().into(),
                title: None,
                title_generation: None,
                roots: Vec::new(),
                thread_id: Some("fixture-thread".into()),
                items: Vec::new().into(),
                work_periods: Vec::new(),
                draft: "Keep my draft".into(),
                draft_images: vec!["data:image/png;base64,aGVsbG8=".into()],
                draft_reference: None,
                config_args: Vec::new(),
                native_config_version: 1,
                activity_reporting: false,
            };
            let mut snapshot = grove_core::codex::Snapshot::new(saved);
            snapshot.status = grove_core::codex::Status::Idle;
            let session = cx.new(|cx| {
                crate::entities::codex_session::CodexSession::from_connection(
                    snapshot,
                    grove_core::codex::Connection {
                        handle: handle.clone(),
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            let session_for_view = session.clone();
            let (sidebar, cx) = cx.add_window_view(|window, cx| {
                let runtime = cx.new(crate::runtime::Runtime::new);
                let registry = runtime.read(cx).registry.clone();
                let id = registry.update(cx, |registry, _| {
                    let id =
                        registry.insert_meta(project.name.clone(), path.clone(), provider.agent());
                    registry.attach_codex(id, session_for_view, handle);
                    id
                });
                let mut sidebar = Sidebar::new(runtime, window, cx);
                sidebar.selection = Some(Selection::Session(id));
                sidebar.initial_selection_pending = false;
                sidebar
            });
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.act(
                        Action::RunNamedScript(path.clone(), path.clone(), "prompt-only".into()),
                        window,
                        cx,
                    );
                    assert!(sidebar.content_error.is_none());
                });
            });
            let submitted: Vec<_> = actions
                .try_iter()
                .filter_map(|action| match action {
                    grove_core::codex::Action::ProjectPrompt { text, .. } => Some(text),
                    _ => None,
                })
                .collect();
            assert_eq!(submitted, vec!["Review the changes".to_owned()]);
            session.read_with(cx, |session, _| {
                assert_eq!(session.state.saved.draft, "Keep my draft");
                assert_eq!(session.state.saved.draft_images.len(), 1);
            });
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.act(
                        Action::RunNamedScript(
                            path.clone(),
                            "/wrong/checkout".into(),
                            "prompt-only".into(),
                        ),
                        window,
                        cx,
                    );
                    assert_eq!(
                        sidebar.content_error.as_deref(),
                        Some("The current chat belongs to another checkout")
                    );
                });
            });
            assert!(!actions
                .try_iter()
                .any(|action| matches!(action, grove_core::codex::Action::ProjectPrompt { .. })));
        }
    }

    #[test]
    fn commit_jobs_and_cancellation_follow_their_checkout_across_navigation() {
        let mut panel = CheckoutPanel::default();
        let first = std::sync::Arc::new(CommitControl::default());
        let second = std::sync::Arc::new(CommitControl::default());
        panel.select_path(Some("/project/one".into()));
        panel.operation.busy = true;
        panel.operation.commit_job = 1;
        panel.operation.commit_job_path = panel.path.clone();
        panel.operation.commit_control = Some(first.clone());
        panel.operation.commit_status = CommitUi::Generating;
        panel.select_path(Some("/project/two".into()));
        assert!(
            !panel.operation.busy,
            "another worktree must remain available"
        );
        panel.operation.busy = true;
        panel.operation.commit_job = 1;
        panel.operation.commit_job_path = panel.path.clone();
        panel.operation.commit_control = Some(second.clone());
        panel.operation.commit_status = CommitUi::Generating;
        panel.commit_event(
            "/project/one",
            1,
            CommitEvent::Progress(CommitPhase::Committing),
        );
        assert!(matches!(
            panel.operation.commit_status,
            CommitUi::Generating
        ));
        panel.select_path(Some("/project/one".into()));
        assert!(
            panel.operation.busy,
            "returning must preserve duplicate prevention"
        );
        assert!(matches!(
            panel.operation.commit_status,
            CommitUi::Committing
        ));
        assert!(std::sync::Arc::ptr_eq(
            panel.operation.commit_control.as_ref().unwrap(),
            &first
        ));
        panel.select_path(Some("/project/two".into()));
        assert!(panel.operation.commit_control.as_ref().unwrap().cancel());
        panel.operation.commit_status = CommitUi::Canceling;
        panel.select_path(None);
        panel.commit_event(
            "/project/two",
            1,
            CommitEvent::Finished(Box::new((Err("canceled".into()), Err("fixture".into())))),
        );
        panel.commit_event(
            "/project/one",
            1,
            CommitEvent::Finished(Box::new((
                Ok(CommitOutcome {
                    subject: "First checkout".into(),
                    hash: None,
                }),
                Err("fixture".into()),
            ))),
        );
        assert!(!panel.operation.busy);
        panel.select_path(Some("/project/two".into()));
        assert!(!panel.operation.busy);
        assert_eq!(panel.operation.message.as_deref(), Some("Commit canceled."));
        panel.select_path(Some("/project/one".into()));
        assert!(!panel.operation.busy);
        assert!(matches!(panel.operation.commit_status, CommitUi::Succeeded));
        assert_eq!(
            panel.operation.message.as_deref(),
            Some("Committed: First checkout")
        );
        panel.commit_event(
            "/project/one",
            0,
            CommitEvent::Progress(CommitPhase::Generating),
        );
        assert!(matches!(panel.operation.commit_status, CommitUi::Succeeded));
    }

    #[test]
    fn hidden_network_completion_preserves_other_checkout_busy_and_error() {
        let mut panel = CheckoutPanel::default();
        panel.select_path(Some("/one".into()));
        panel.operation.busy = true;
        panel.operation.network_action = Some(("Pushing…", "git-push"));
        panel.select_path(Some("/two".into()));
        assert!(!panel.operation.busy);
        panel.operation.busy = true;
        panel.update_operation("/one", |panel, current| {
            assert!(!current);
            panel.operation.busy = false;
            panel.operation.network_action = None;
            panel.operation.message = Some("Push failed".into());
        });
        assert!(panel.operation.busy);
        assert!(panel.operation.message.is_none());
        panel.select_path(Some("/one".into()));
        assert!(!panel.operation.busy);
        assert_eq!(panel.operation.message.as_deref(), Some("Push failed"));
    }

    #[test]
    fn completion_cannot_cross_targets_or_a_return_to_the_same_path() {
        let panel = CheckoutPanel {
            path: Some("/one".into()),
            generation: 3,
            ..Default::default()
        };
        assert!(panel.accepts_result("/one", 3));
        assert!(!panel.accepts_result("/two", 3));
        assert!(!panel.accepts_result("/one", 1));
    }

    fn scripts_project(count: usize) -> grove_core::storage::Project {
        use grove_core::storage::{ProjectAction, ProjectActionKind, ProjectScripts};
        let mut entries: Vec<_> = (0..count)
            .map(|index| ProjectAction {
                id: format!("shell-{index}"),
                name: if index == 0 {
                    "Run integration tests against local services with a long name".into()
                } else {
                    format!("Script {}", index + 1)
                },
                kind: ProjectActionKind::Shell {
                    command: if index == 4 {
                        (0..30)
                            .map(|line| {
                                format!(
                                    "echo detailed command line {line} with full readable context"
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    } else {
                        format!("printf 'script {index}\\n'")
                    },
                },
            })
            .collect();
        entries.push(ProjectAction {
            id: "prompt-only".into(),
            name: "Review".into(),
            kind: ProjectActionKind::Prompt {
                template: "Review the changes".into(),
            },
        });
        grove_core::storage::Project {
            name: "Scripts QA".into(),
            path: "/tmp/grove-panel-scripts-fixture".into(),
            archived: false,
            worktree_dir: None,
            scripts: ProjectScripts {
                setup: Some("echo automatic setup".into()),
                entries,
                ..Default::default()
            },
        }
    }

    struct ScriptsFixture(Entity<Sidebar>, grove_core::storage::Project);
    impl gpui::Render for ScriptsFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |sidebar, cx| {
                if let Some(panel) = &sidebar.project_panel {
                    div().size_full().child(panel.clone()).into_any_element()
                } else {
                    project_field_well(false, false)
                        .w_full()
                        .h_auto()
                        .px_0()
                        .py(rpx(SPACE_SM))
                        .child(checkout_row(
                            "checkout-finder",
                            "Open in Finder",
                            "folder",
                            window,
                            cx,
                        ))
                        .child(checkout_divider("checkout-scripts-divider"))
                        .child(sidebar.render_checkout_scripts(
                            &self.1,
                            "/tmp/selected-checkout",
                            window,
                            cx,
                        ))
                        .child(checkout_divider("checkout-divider"))
                        .child(checkout_git_heading())
                        .child(checkout_row(
                            "checkout-pr",
                            "main",
                            "git-branch",
                            window,
                            cx,
                        ))
                        .child(
                            checkout_row(
                                "checkout-commit-publish",
                                "No local commits",
                                "git",
                                window,
                                cx,
                            )
                            .disabled(true),
                        )
                        .child(checkout_row(
                            "checkout-changes",
                            "Changes",
                            "unified",
                            window,
                            cx,
                        ))
                        .into_any_element()
                }
            })
        }
    }

    #[gpui::test]
    fn scripts_register_limits_rows_preserves_actions_and_opens_editor(
        cx: &mut gpui::TestAppContext,
    ) {
        let project = scripts_project(7);
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![project.clone()],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
        });
        let owner = std::rc::Rc::new(std::cell::RefCell::new(None));
        let owner_for_view = owner.clone();
        let (_, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
            *owner_for_view.borrow_mut() = Some(sidebar.clone());
            ScriptsFixture(sidebar, project.clone())
        });
        cx.simulate_resize(gpui::size(gpui::px(240.), gpui::px(800.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let sidebar = owner.borrow().clone().unwrap();
        assert!(cx.debug_bounds("checkout-script-shell-4").is_some());
        assert!(cx.debug_bounds("checkout-script-shell-5").is_none());
        assert!(cx.debug_bounds("checkout-script-prompt-only").is_none());
        let heading = cx.debug_bounds("checkout-scripts-heading").unwrap();
        let git_heading = cx.debug_bounds("checkout-git-heading").unwrap();
        assert_eq!(heading.left(), git_heading.left());
        let label_column = cx.debug_bounds("checkout-finder-label").unwrap().left();
        assert!(heading.left() < label_column);
        let finder = cx.debug_bounds("checkout-finder").unwrap();
        for selector in ["checkout-scripts-divider", "checkout-divider"] {
            let divider = cx.debug_bounds(selector).unwrap();
            assert_eq!(divider.left(), finder.left());
            assert_eq!(divider.right(), finder.right());
        }
        for selector in [
            "checkout-finder-label",
            "checkout-pr-label",
            "checkout-commit-publish-label",
            "checkout-changes-label",
            "checkout-scripts-disclosure-label",
        ] {
            assert_eq!(cx.debug_bounds(selector).unwrap().left(), label_column);
        }
        for selector in [
            "checkout-script-name-shell-0",
            "checkout-script-name-shell-1",
            "checkout-script-name-shell-2",
            "checkout-script-name-shell-3",
            "checkout-script-name-shell-4",
        ] {
            let label = cx.debug_bounds(selector).unwrap();
            assert_eq!(label.left(), label_column);
        }
        let run = cx.debug_bounds("checkout-script-run-shell-0").unwrap();
        let row = cx.debug_bounds("checkout-script-shell-0").unwrap();
        assert!(run.left() > row.left() && run.right() <= row.right());
        assert!(cx.debug_bounds("checkout-script-add").is_some());
        assert!(cx.debug_bounds("checkout-script-manage").is_some());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.checkout_panel.script_run_focus[&format!("{}:shell-0", project.path)]
                    .focus(window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("checkout-script-focus-context").is_some());
        cx.simulate_resize(gpui::size(gpui::px(220.), gpui::px(220.)));
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.checkout_panel.script_run_focus[&format!("{}:shell-4", project.path)]
                    .focus(window, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let context = cx.debug_bounds("checkout-script-focus-context").unwrap();
        assert!(context.left() >= gpui::px(0.) && context.right() <= gpui::px(220.));
        assert!(
            context.top() >= gpui::px(0.) && context.bottom() <= gpui::px(220.),
            "keyboard context stays within viewport: {context:?}"
        );
        let text = cx.debug_bounds("checkout-script-context-text").unwrap();
        assert!(
            text.left() >= context.left() && text.right() <= context.right(),
            "literal command text stays within the context viewport: {text:?} in {context:?}"
        );
        cx.simulate_keystrokes("pagedown");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        sidebar.read_with(cx, |sidebar, _| {
            assert!(
                sidebar.checkout_panel.script_context_scroll[&format!("{}:shell-4", project.path)]
                    .offset()
                    .y
                    < gpui::px(0.),
                "Page Down reveals later command lines"
            );
        });
        cx.update(|window, cx| {
            window.blur();
            sidebar.update(cx, |_, cx| cx.notify());
        });
        cx.simulate_resize(gpui::size(gpui::px(240.), gpui::px(800.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let disclosure = cx.debug_bounds("checkout-scripts-disclosure").unwrap();
        cx.simulate_click(disclosure.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("checkout-script-shell-6").is_some());
        assert!(cx.debug_bounds("checkout-script-prompt-only").is_some());
        let prompt_run = cx.debug_bounds("checkout-script-run-prompt-only").unwrap();
        assert_eq!(prompt_run.size.width, prompt_run.size.height);
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(
                    Action::RunNamedScript(
                        project.path.clone(),
                        project.path.clone(),
                        "prompt-only".into(),
                    ),
                    window,
                    cx,
                );
                assert_eq!(
                    sidebar.content_error.as_deref(),
                    Some("Open a native Codex or Claude chat in this checkout first")
                );
            });
        });
        let disclosure = cx.debug_bounds("checkout-scripts-disclosure").unwrap();
        cx.simulate_click(disclosure.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("checkout-script-shell-5").is_none());
        let manage = cx.debug_bounds("checkout-script-manage").unwrap();
        cx.simulate_click(manage.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("project-scripts-register").is_some());
        assert!(cx.debug_bounds("Shell command").is_none());
        cx.update(|window, cx| {
            assert_eq!(
                cx.global::<SettingsState>().store.projects[0]
                    .scripts
                    .entries
                    .len(),
                8
            );
            sidebar.update(cx, |sidebar, cx| {
                sidebar.project_panel = None;
                sidebar.act(
                    Action::EditProjectScripts(project.path.clone(), true),
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("Shell command").is_some());
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<SettingsState>().store.projects[0]
                    .scripts
                    .entries
                    .len(),
                8
            );
        });
    }

    #[test]
    #[ignore = "requires macOS Metal renderer; writes scripts panel screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn checkout_scripts_native_snapshots() {
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
            cx.set_global(crate::zoom::ZoomState::new(1.0));
        });
        for (label, width, rem, count, focus, light) in [
            ("wide", 280., 16., 7, false, false),
            ("expanded", 280., 16., 7, false, false),
            ("narrow", 220., 16., 7, false, false),
            ("focus", 280., 16., 7, true, false),
            ("bottom-context", 220., 16., 7, true, false),
            ("prompt-only", 280., 16., 0, false, false),
            ("empty", 280., 16., 0, false, false),
            ("add-many", 768., 16., 15, false, false),
            ("manage", 768., 16., 7, false, false),
            ("light-zoom", 280., 20., 7, false, true),
        ] {
            let mut project = scripts_project(count);
            if label == "empty" {
                project.scripts.entries.clear();
            }
            cx.update(|cx| {
                cx.set_global(SettingsState::new(grove_core::storage::Store {
                    projects: vec![project.clone()],
                    ..Default::default()
                }));
                assert!(grove_core::theme::set_by_name(if light {
                    c::DEFAULT_LIGHT_THEME
                } else {
                    c::DEFAULT_DARK_THEME
                }));
                c::sync_component_theme(cx);
                gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
            });
            let owner = std::rc::Rc::new(std::cell::RefCell::new(None));
            let owner_for_window = owner.clone();
            let window = cx
                .open_window(
                    gpui::size(
                        gpui::px(width),
                        gpui::px(if label == "bottom-context" {
                            220.
                        } else if matches!(label, "add-many" | "manage") {
                            720.
                        } else {
                            600.
                        }),
                    ),
                    |window, cx| {
                        let runtime = cx.new(crate::runtime::Runtime::new);
                        let sidebar = cx.new(|cx| {
                            let mut sidebar = Sidebar::new(runtime, window, cx);
                            if label == "expanded" {
                                sidebar
                                    .checkout_panel
                                    .scripts_show_all
                                    .insert(project.path.clone());
                            }
                            sidebar
                        });
                        *owner_for_window.borrow_mut() = Some(sidebar.clone());
                        let fixture = cx.new(|_| ScriptsFixture(sidebar, project.clone()));
                        cx.new(|cx| {
                            gpui_component::Root::new(fixture, window, cx)
                                .bordered(false)
                                .bg(c::BG())
                        })
                    },
                )
                .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
            if matches!(label, "add-many" | "manage") {
                cx.update_window(window.into(), |_, window, cx| {
                    owner.borrow().as_ref().unwrap().update(cx, |sidebar, cx| {
                        sidebar.act(
                            Action::EditProjectScripts(project.path.clone(), label == "add-many"),
                            window,
                            cx,
                        );
                    });
                })
                .unwrap();
                for _ in 0..3 {
                    cx.run_until_parked();
                    cx.update_window(window.into(), |_, window, cx| {
                        window.simulate_next_frame(cx);
                        let _ = window.draw(cx);
                    })
                    .unwrap();
                }
            }
            if focus {
                cx.update_window(window.into(), |_, window, cx| {
                    owner.borrow().as_ref().unwrap().update(cx, |sidebar, cx| {
                        sidebar.checkout_panel.script_run_focus[&format!(
                            "{}:shell-{}",
                            project.path,
                            if label == "bottom-context" { 4 } else { 0 }
                        )]
                            .focus(window, cx);
                        cx.notify();
                    });
                })
                .unwrap();
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    let _ = window.draw(cx);
                })
                .unwrap();
            }
            let path = format!("/tmp/grove-checkout-scripts-{label}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&path)
                .unwrap();
            eprintln!("Scripts native fixture: {path}");
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }

    #[test]
    #[ignore = "requires macOS Metal renderer; writes screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn checkout_pr_native_snapshots() {
        use gpui::AppContext as _;
        struct PrFixture(Entity<Sidebar>, CheckoutGitState);
        impl gpui::Render for PrFixture {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.0.update(cx, |sidebar, cx| {
                    sidebar.render_checkout_pr("/fixture", Some(&self.1), window, cx)
                })
            }
        }
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
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
        });
        for (width, expanded) in [(280., true), (220., true), (280., false), (220., false)] {
            let mut sidebar_entity = None;
            let window = cx.open_window(gpui::size(gpui::px(width),gpui::px(700.)), |window,cx| {
                let runtime = cx.new(crate::runtime::Runtime::new);
                let sidebar = cx.new(|cx| { let mut sidebar = Sidebar::new(runtime,window,cx);
                    sidebar.checkout_panel.pr_state = Some(Ok(Some(serde_json::from_value(serde_json::json!({"number":142,"title":"A long pull request title that must yield to fixed actions and wrap readiness", "url":"https://github.com/example/project/pull/142","state":"OPEN","isDraft":false,"headRefOid":"head","headRefName":"topic","baseRefName":"main","mergeable":"MERGEABLE","mergeStateStatus":"BLOCKED","statusCheckRollup":[{"name":"CI / integration","status":"COMPLETED","conclusion":"SUCCESS"}]})).unwrap())));
                    sidebar.checkout_panel.pr_expanded = expanded; sidebar });
                sidebar_entity = Some(sidebar.clone());
                let repo = tempfile::tempdir().unwrap();
                assert!(std::process::Command::new("git").current_dir(repo.path()).args(["init", "-b", "topic"]).output().unwrap().status.success());
                let state = grove_core::checkout_git::inspect(repo.path()).unwrap();
                let fixture = cx.new(|_| PrFixture(sidebar,state));
                cx.new(|cx| gpui_component::Root::new(fixture,window,cx).bordered(false).bg(c::BG()))
            }).unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
            let image = cx.capture_screenshot(window.into()).unwrap();
            let path = if expanded {
                format!("/tmp/grove-pr-workflows-{width}.png")
            } else {
                format!("/tmp/grove-pr-workflows-{width}-collapsed.png")
            };
            image.save(&path).unwrap();
            let sidebar = sidebar_entity.unwrap();
            for (loading, pr_loading) in [(true, false), (false, true), (true, true)] {
                cx.update(|cx| {
                    sidebar.update(cx, |sidebar, cx| {
                        sidebar.checkout_panel.loading = loading;
                        sidebar.checkout_panel.pr_loading = pr_loading;
                        cx.notify();
                    });
                });
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    let _ = window.draw(cx);
                })
                .unwrap();
                let refreshing = cx.capture_screenshot(window.into()).unwrap();
                assert!(
                    image.as_raw() == refreshing.as_raw(),
                    "cached PR summary and expanded gates must remain unchanged during refresh \
                     (width={width}, expanded={expanded}, git={loading}, pr={pr_loading})"
                );
            }
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }

    /// Native Metal screenshots with channel-backed provider fixtures; no model calls.
    #[test]
    #[ignore = "requires macOS Metal renderer; writes screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn checkout_agents_native_snapshots() {
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
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "Preview fixture".into(),
                    path: "/tmp/grove-agent-preview-project".into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
        });
        for (provider, label, width, rem, light) in [
            (
                grove_core::codex::Provider::Codex,
                "codex-wide",
                1280.,
                16.,
                false,
            ),
            (
                grove_core::codex::Provider::Claude,
                "claude-narrow",
                760.,
                16.,
                false,
            ),
            (
                grove_core::codex::Provider::Claude,
                "claude-light-zoom",
                1280.,
                20.,
                true,
            ),
        ] {
            cx.update(|cx| {
                assert!(grove_core::theme::set_by_name(if light {
                    c::DEFAULT_LIGHT_THEME
                } else {
                    c::DEFAULT_DARK_THEME
                }));
                c::sync_component_theme(cx);
                gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
            });
            let owner = std::rc::Rc::new(std::cell::RefCell::new(None));
            let owner_for_window = owner.clone();
            let window = cx.open_window(gpui::size(gpui::px(width), gpui::px(900.)), |window, cx| {
                let runtime = cx.new(crate::runtime::Runtime::new);
                let id = runtime.read(cx).registry.clone().update(cx, |registry, _| registry.insert_meta("Preview fixture".into(), "/tmp/grove-agent-preview-fixture".into(), provider.agent()));
                let (handle, _actions) = grove_core::codex::Handle::channel();
                let (_updates, receive) = std::sync::mpsc::channel();
            let saved = grove_core::codex::SavedSession {
                session_review: Default::default(),
                rich_plans: grove_core::rich_plan::PlanStore::default(),
                side_chats: Default::default(),
                fork_origin: None,
            claude_resume: None,
                claude_subagents: std::collections::HashMap::default(),
                edit_checkpoints: Vec::new(),
                pending_edit: None,
            rewind_drafts: Vec::new(),
                provider,
                composer: grove_core::codex::ComposerSettings::default(),
                context: None,
                key: "agents-fixture".into(),
                project: "Preview fixture".into(),
                cwd: "/tmp/grove-agent-preview-fixture".into(),
                label: provider.label().into(),
                title: None,
                title_generation: None,
                roots: Vec::new(),
                thread_id: None,
                items: Vec::new().into(),
                work_periods: Vec::new(),
                draft: String::new(),
                draft_images: Vec::new(),
                draft_reference: None,
                config_args: Vec::new(),
                native_config_version: 1,
                activity_reporting: false,
            };

                let mut snapshot = grove_core::codex::Snapshot::new(saved);
                snapshot.saved.items = vec![serde_json::json!({"type":"userMessage", "text":"Investigate authentication retries"}), serde_json::json!({"type":"reasoning", "text":"Thinking"})].into();
                snapshot.saved.work_periods = vec![grove_core::codex::WorkPeriod { start_item: 0, end_item: None, duration_ms: None, subagents: vec![grove_core::codex::Subagent { id: "explorer".into(), name: "Explorer".into(), role: Some("Investigation".into()), task: Some("Investigate authentication retry failures and report the owning layer".into()), activity: "Reading authentication tests".into(), status: grove_core::codex::SubagentStatus::Working }] }];
                for (id, name, status) in [
                    ("interrupted", "Interrupted agent", grove_core::codex::SubagentStatus::Interrupted),
                    ("done", "Completed agent", grove_core::codex::SubagentStatus::Completed),
                    ("failed", "Failed agent", grove_core::codex::SubagentStatus::Failed),
                    ("unknown", "Unknown agent with a long name that must truncate", grove_core::codex::SubagentStatus::Unknown),
                ] {
                    snapshot.saved.work_periods[0].subagents.push(grove_core::codex::Subagent {
                        id: id.into(), name: name.into(), role: None, task: None,
                        activity: "Observed activity".into(), status,
                    });
                }
                snapshot.status = grove_core::codex::Status::Working;
                let observed = Instant::now();
                let mut activity = grove_core::codex::SubagentActivity::default();
                activity.items = (0..24).map(|index| serde_json::json!({"id":format!("comment-{index}"),"type":"agentMessage","text":format!("### Investigation step {index}\n\nFollowing the authentication retry path and checking the owning layer.\n\n- Preserve the request identifier\n- Inspect retry status and cancellation")})).collect();
                activity.items.push(serde_json::json!({"id":"tool-completed","type":"commandExecution","command":"rg retry tests/auth","status":"completed","aggregatedOutput":"tests/auth/retry.rs:42: retry_after_rejection\ntests/auth/retry.rs:87: cancellation_preserves_request"}));
                activity.items.push(serde_json::json!({"id":"latest-comment","type":"agentMessage","text":"## Retry entry point found\n\nThe request reaches **the authentication owner** before retrying. This detailed investigation preserves markdown, `request_id`, and the complete visible message.\n\n```rust\nassert_eq!(retry.request_id, original.request_id);\n```\n\nNext I will run the focused regression and inspect its full output."}));
                activity.items.push(serde_json::json!({"id":"tool-active","type":"commandExecution","command":"cargo test authentication_retry -- --nocapture","status":"inProgress","aggregatedOutput":"running 3 tests\ntest retry_after_rejection ... ok\ntest cancellation_preserves_request ... ok"}));
                activity.observed_at = Some(observed);
                snapshot.subagent_activity.insert("explorer".into(), activity);
                let session = cx.new(|cx| crate::entities::codex_session::CodexSession::from_connection(snapshot, grove_core::codex::Connection { handle:handle.clone(), updates:receive.into() },cx));
                runtime.read(cx).registry.clone().update(cx, |registry,_| registry.attach_codex(id,session,handle));
                let sidebar = cx.new(|cx| { let mut sidebar=Sidebar::new(runtime,window,cx); sidebar.selection=Some(Selection::Session(id)); sidebar.initial_selection_pending=false; sidebar.checkout_panel.expanded=true; sidebar });
                *owner_for_window.borrow_mut()=Some((sidebar.clone(),id));
                cx.new(|cx| gpui_component::Root::new(sidebar,window,cx).bordered(false).bg(c::BG()))
            }).unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
            let (sidebar, id) = owner.borrow().as_ref().unwrap().clone();
            cx.update_window(window.into(), |_, window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    assert_eq!(
                        sidebar.selection,
                        Some(Selection::Session(id)),
                        "fixture session remains selected"
                    );
                    assert!(
                        sidebar.checkout_targets(cx).is_empty(),
                        "fixture has no registered checkout"
                    );
                    assert!(
                        sidebar
                            .checkout_panel
                            .agent_bounds
                            .get("explorer")
                            .is_some_and(|bounds| bounds.get().size.width > gpui::px(0.)),
                        "fixture agent row was actually rendered"
                    );
                    sidebar.open_agent_peek(id, "explorer".into(), true, window, cx);
                });
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
            let path = format!("/tmp/grove-agent-preview-{label}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&path)
                .unwrap();
            eprintln!("Native fixture (provider channels, not live): {path}");
            // Capture retention states independently of provider channels. The live
            // fixture above remains the original complete/streaming transcript.
            for retention in ["awaiting", "unavailable", "restored"] {
                cx.update_window(window.into(), |_, window, cx| {
                    sidebar.update(cx, |sidebar, cx| {
                        let session = sidebar.runtime.read(cx).registry.read(cx).codex(id).cloned().unwrap();
                        session.update(cx, |session, cx| {
                            if retention == "awaiting" {
                                session.state.subagent_activity.clear();
                            } else if retention == "unavailable" {
                                session.state = grove_core::codex::Snapshot::new(session.state.saved.clone()).into();
                            } else {
                                let mut activity = grove_core::codex::SubagentActivity::default();
                                activity.items = vec![serde_json::json!({"id":"saved-reply", "type":"agentMessage", "text":"## Saved investigation\n\nThe authentication owner preserves the original request identifier."})].into();
                                session.state.subagent_activity.insert("explorer".into(), activity);
                            }
                            let (freshness, empty) = super::agent_preview::child_transcript_labels(&session.state, "explorer");
                            match retention {
                                "awaiting" => assert_eq!(empty, Some("No child messages received yet")),
                                "unavailable" => assert_eq!(empty, Some("Historical child messages are unavailable")),
                                _ => {
                                    assert!(empty.is_none());
                                    assert_eq!(freshness, "Saved child transcript · observation time unavailable");
                                }
                            }
                            cx.notify();
                        });
                        sidebar.open_agent_peek(id, "explorer".into(), true, window, cx);
                    });
                    let _ = window.draw(cx);
                }).unwrap();
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    let _ = window.draw(cx);
                })
                .unwrap();
                let path = format!("/tmp/grove-agent-preview-{label}-{retention}.png");
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Native retention fixture (not live): {path}");
            }
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }

    #[gpui::test]
    fn codex_and_claude_first_child_appears_without_registered_checkout(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "Unregistered fixture".into(),
                    path: "/tmp/grove-unregistered-project".into(),
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
            let saved = grove_core::codex::SavedSession {
                session_review: Default::default(),
                rich_plans: grove_core::rich_plan::PlanStore::default(),
                side_chats: Default::default(),
                fork_origin: None,
                claude_resume: None,
                claude_subagents: std::collections::HashMap::default(),
                edit_checkpoints: Vec::new(),
                pending_edit: None,
                rewind_drafts: Vec::new(),
                provider,
                composer: grove_core::codex::ComposerSettings::default(),
                context: None,
                key: "agents-fixture".into(),
                project: "Unregistered fixture".into(),
                cwd: "/tmp/grove-unregistered-agent-fixture".into(),
                label: provider.label().into(),
                title: None,
                title_generation: None,
                roots: Vec::new(),
                thread_id: None,
                items: Vec::new().into(),
                work_periods: Vec::new(),
                draft: String::new(),
                draft_images: Vec::new(),
                draft_reference: None,
                config_args: Vec::new(),
                native_config_version: 1,
                activity_reporting: false,
            };

            let mut initial = grove_core::codex::Snapshot::new(saved.clone());
            initial.status = grove_core::codex::Status::Working;
            let (handle, _actions) = grove_core::codex::Handle::channel();
            let (updates, receive) = std::sync::mpsc::channel();
            let (sidebar, cx) = cx.add_window_view(|window, cx| {
                let runtime = cx.new(crate::runtime::Runtime::new);
                let id = runtime.read(cx).registry.clone().update(cx, |registry, _| {
                    registry.insert_meta(
                        "Unregistered fixture".into(),
                        "/tmp/grove-unregistered-agent-fixture".into(),
                        provider.agent(),
                    )
                });
                let session = cx.new(|cx| {
                    crate::entities::codex_session::CodexSession::from_connection(
                        initial,
                        grove_core::codex::Connection {
                            handle: handle.clone(),
                            updates: receive.into(),
                        },
                        cx,
                    )
                });
                runtime
                    .read(cx)
                    .registry
                    .clone()
                    .update(cx, |registry, _| registry.attach_codex(id, session, handle));
                let mut sidebar = Sidebar::new(runtime, window, cx);
                sidebar.selection = Some(Selection::Session(id));
                sidebar.initial_selection_pending = false;
                sidebar
            });
            cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(sidebar.read_with(cx, |sidebar, cx| sidebar.checkout_targets(cx).is_empty()));
            assert!(cx.debug_bounds("checkout-agents").is_none());
            assert!(sidebar.read_with(cx, |sidebar, _| sidebar
                .checkout_panel
                .agent_observer
                .is_some()));
            let mut next = grove_core::codex::Snapshot::new(saved);
            next.status = grove_core::codex::Status::Working;
            next.saved.work_periods = vec![grove_core::codex::WorkPeriod {
                start_item: 0,
                end_item: None,
                duration_ms: None,
                subagents: vec![grove_core::codex::Subagent {
                    id: "first-child".into(),
                    name: "Explorer".into(),
                    task: Some("Inspect source".into()),
                    role: None,
                    activity: "Reading files".into(),
                    status: grove_core::codex::SubagentStatus::Working,
                }],
            }];
            let mut second = next.saved.work_periods[0].subagents[0].clone();
            second.id = "second-child".into();
            second.name = "Reviewer".into();
            next.saved.work_periods[0].subagents.push(second);
            // Only the provider update channel changes; do not notify Sidebar.
            updates.send(next).unwrap();
            cx.executor().advance_clock(Duration::from_millis(41));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(
                cx.debug_bounds("checkout-agents").is_some(),
                "first child makes independent right panel visible for {}",
                provider.label()
            );
            assert!(cx.debug_bounds("checkout-finder").is_none());
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    cx.set_reduce_motion(false);
                    let Some(Selection::Session(id)) = sidebar.selection else {
                        panic!("fixture session");
                    };
                    assert!(
                        !sidebar.html_overlay_open(),
                        "resting transcript permits HTML"
                    );
                    sidebar.open_agent_peek(id, "first-child".into(), true, window, cx);
                    assert!(
                        !sidebar.html_overlay_open(),
                        "preview must preserve native HTML outside its mask"
                    );
                    sidebar.dismiss_agent_peek(window, cx);
                    assert!(sidebar.checkout_panel.peek.as_ref().unwrap().closing);
                    assert!(
                        !sidebar.html_overlay_open(),
                        "HTML remains visible during dismissal"
                    );
                    assert!(sidebar.checkout_panel.agent_focus["first-child"].is_focused(window));
                    sidebar.open_agent_peek(id, "first-child".into(), true, window, cx);
                });
            });
            cx.run_until_parked();
            cx.executor()
                .advance_clock(Duration::from_millis(MOTION_FAST_MS) * 2);
            cx.run_until_parked();
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    assert!(
                        !sidebar.checkout_panel.peek.as_ref().unwrap().closing,
                        "old dismissal must not remove reopened {} preview",
                        provider.label()
                    );
                    assert!(!sidebar.html_overlay_open(), "reopening preserves HTML");
                    sidebar.dismiss_agent_peek(window, cx);
                });
            });
            cx.run_until_parked();
            cx.executor()
                .advance_clock(Duration::from_millis(MOTION_FAST_MS - 1));
            cx.run_until_parked();
            assert!(
                sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_some()),
                "exit remains painted until the fast-motion deadline"
            );
            cx.executor().advance_clock(Duration::from_millis(1));
            cx.run_until_parked();
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    assert!(sidebar.checkout_panel.peek.is_none());
                    assert!(!sidebar.html_overlay_open(), "HTML returns after dismissal");
                    cx.set_reduce_motion(true);
                    let Some(Selection::Session(id)) = sidebar.selection else {
                        panic!("fixture session");
                    };
                    sidebar.open_agent_peek(id, "first-child".into(), true, window, cx);
                    assert!(
                        !sidebar.html_overlay_open(),
                        "static preview preserves native HTML"
                    );
                    sidebar.dismiss_agent_peek(window, cx);
                    assert!(sidebar.checkout_panel.peek.is_none());
                    assert!(
                        !sidebar.html_overlay_open(),
                        "static dismissal restores HTML"
                    );
                    sidebar.agent_peek_hover(id, "first-child".into(), true, window, cx);
                });
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(220));
            cx.run_until_parked();
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    let Some(Selection::Session(id)) = sidebar.selection else {
                        panic!("fixture session");
                    };
                    assert_eq!(
                        sidebar.checkout_panel.peek.as_ref().unwrap().child,
                        "first-child"
                    );
                    // Enter can arrive before the old row/preview's exit.
                    sidebar.agent_peek_hover(id, "second-child".into(), true, window, cx);
                    sidebar.agent_peek_hover(id, "first-child".into(), false, window, cx);
                });
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(220));
            cx.run_until_parked();
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    let Some(Selection::Session(id)) = sidebar.selection else {
                        panic!("fixture session");
                    };
                    assert_eq!(
                        sidebar.checkout_panel.peek.as_ref().unwrap().child,
                        "second-child",
                        "quick hover handoff opens the next {} agent",
                        provider.label()
                    );
                    sidebar.agent_peek_hover(id, "first-child".into(), false, window, cx);
                });
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(260));
            cx.run_until_parked();
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    let Some(Selection::Session(id)) = sidebar.selection else {
                        panic!("fixture session");
                    };
                    assert_eq!(
                        sidebar.checkout_panel.peek.as_ref().unwrap().child,
                        "second-child",
                        "stale exits cannot dismiss the new preview"
                    );
                    sidebar.agent_peek_hover(id, "second-child".into(), false, window, cx);
                });
            });
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(260));
            cx.run_until_parked();
            assert!(
                sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_none()),
                "leaving the current agent still dismisses its preview"
            );
        }
    }

    #[gpui::test]
    fn codex_and_claude_checkout_agents_follow_session_updates_and_disclosures(
        cx: &mut gpui::TestAppContext,
    ) {
        let fixture = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(fixture.path())
            .args(["init", "-b", "main"])
            .output()
            .unwrap()
            .status
            .success());
        fs_err::write(fixture.path().join("main.rs"), "fn main() {}\n").unwrap();
        fs_err::write(fixture.path().join("README.md"), "# Example\n").unwrap();
        let fixture_path = fixture.path().to_string_lossy().into_owned();
        cx.update(|cx| {
            gpui_component::init(cx);
            // Immediate interaction checks; the separate lifecycle test covers exit motion.
            cx.set_reduce_motion(true);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "Checkout QA".into(),
                    path: fixture_path.clone(),
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
            let runtime = cx.new(crate::runtime::Runtime::new);
            let path = fixture_path.clone();
            let id = runtime.read(cx).registry.clone().update(cx, |registry, _| {
                registry.insert_meta("Checkout QA".into(), path.clone(), Agent::Codex)
            });
            runtime.read(cx).tree.clone().update(cx, |tree, _| {
                tree.set_active_worktrees(
                    0,
                    vec![grove_core::git::Worktree {
                        path,
                        branch: "main".into(),
                        mtime: None,
                        is_main: true,
                    }],
                );
            });
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.selection = Some(Selection::Session(id));
            sidebar.initial_selection_pending = false;
            sidebar
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        for provider in [
            grove_core::codex::Provider::Codex,
            grove_core::codex::Provider::Claude,
        ] {
            let id = cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    let id = sidebar
                        .runtime
                        .read(cx)
                        .registry
                        .clone()
                        .update(cx, |registry, _| {
                            registry.insert_meta(
                                "Checkout QA".into(),
                                fixture_path.clone(),
                                provider.agent(),
                            )
                        });
                    sidebar.selection = Some(Selection::Session(id));
                    id
                })
            });
            let (handle, _actions) = grove_core::codex::Handle::channel();
            let (_updates, receive) = std::sync::mpsc::channel();
            let saved = grove_core::codex::SavedSession {
                session_review: Default::default(),
                rich_plans: grove_core::rich_plan::PlanStore::default(),
                side_chats: Default::default(),
                fork_origin: None,
                claude_resume: None,
                claude_subagents: std::collections::HashMap::default(),
                edit_checkpoints: Vec::new(),
                pending_edit: None,
                rewind_drafts: Vec::new(),
                provider,
                composer: grove_core::codex::ComposerSettings::default(),
                context: None,
                key: "agents-fixture".into(),
                project: "Checkout QA".into(),
                cwd: fixture_path.clone(),
                label: provider.label().into(),
                title: None,
                title_generation: None,
                roots: Vec::new(),
                thread_id: None,
                items: Vec::new().into(),
                work_periods: Vec::new(),
                draft: String::new(),
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
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar
                        .runtime
                        .read(cx)
                        .registry
                        .clone()
                        .update(cx, |registry, _| {
                            registry.attach_codex(id, session.clone(), handle);
                        });
                    sidebar.checkout_panel.expanded = true;
                    cx.notify();
                });
            });
            cx.run_until_parked();
            // The observer is installed even before a child has appeared.
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            cx.update(|_, cx| {
                session.update(cx, |session, cx| {
                    use grove_core::codex::{Subagent, SubagentStatus, WorkPeriod};
                    let agent = |id: &str, status| Subagent {
                        id: id.into(),
                        name: format!("A deliberately long agent name for {id}"),
                        activity: "Inspecting checkout layout".into(),
                        role: None,
                        task: Some("Inspect checkout layout and verify long task headings remain readable at narrow widths".into()),
                        status,
                    };
                    session.state.saved.items = vec![
                        serde_json::json!({"type":"userMessage", "text":"Inspect checkout"}),
                        serde_json::json!({"type":"reasoning", "text":"Inspecting"}),
                        serde_json::json!({"type":"userMessage", "text":"Continue checkout"}),
                        serde_json::json!({"type":"reasoning", "text":"Continuing"}),
                    ].into();
                    session.state.saved.work_periods = vec![
                        WorkPeriod {
                            start_item: 0,
                            end_item: Some(2),
                            duration_ms: None,
                            subagents: vec![
                                agent("old-running", SubagentStatus::Working),
                                agent("old-done", SubagentStatus::Completed),
                                agent("updated", SubagentStatus::Working),
                            ],
                        },
                        WorkPeriod {
                            start_item: 2,
                            end_item: None,
                            duration_ms: None,
                            subagents: (0..6)
                                .map(|index| {
                                    agent(
                                        &format!("current-{index}"),
                                        if index == 0 {
                                            SubagentStatus::Completed
                                        } else {
                                            SubagentStatus::Working
                                        },
                                    )
                                })
                                .chain(std::iter::once(agent("updated", SubagentStatus::Failed)))
                                .collect(),
                        },
                    ];
                    cx.notify();
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            // Exercise both provider fixtures at normal and narrow viewport sizes and UI zoom.
            for (width, scale) in [(1280., 1.), (500., 1.), (500., 1.1)] {
                cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.)));
                cx.update(|window, _| {
                    window.set_rem_size(gpui::px(crate::zoom::REM_BASE * scale));
                });
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                assert!(cx.debug_bounds("checkout-agents").is_some());
                let card = cx.debug_bounds("checkout-card").unwrap();
                let name = cx.debug_bounds("checkout-agent-name-old-running").unwrap();
                let status = cx
                    .debug_bounds("checkout-agent-status-old-running")
                    .unwrap();
                assert!(status.right() < card.right());
                assert!(
                    (status.size.width - gpui::px(AGENT_STATUS_W * scale)).abs() < gpui::px(0.5)
                );
                let activity = cx
                    .debug_bounds("checkout-agent-activity-old-running")
                    .unwrap();
                assert_eq!(name.top(), status.top());
                assert!(name.right() <= status.left());
                // One foreground identity/status line, then one smaller activity line.
                assert!((activity.top() - name.bottom()).abs() < gpui::px(0.5));
                assert!(
                    (activity.size.height - gpui::px(AGENT_LINE_H * scale)).abs() < gpui::px(0.5)
                );
                let next_name = cx.debug_bounds("checkout-agent-name-updated").unwrap();
                assert!(
                    (next_name.top()
                        - name.top()
                        - gpui::px((AGENT_LINE_H * 2. + SPACE_SM * 2.) * scale))
                    .abs()
                        < gpui::px(1.)
                );
                assert!(activity.left() >= card.left());
                assert!(activity.right() < card.right());
                let pane = sidebar.read_with(cx, |sidebar, cx| {
                    sidebar.codex_views[&id].read(cx).transcript_bounds.get()
                });
                assert!(
                    pane.size.height >= gpui::px(80.),
                    "{} transcript must remain usable with checkout expanded: {pane:?}",
                    provider.label()
                );
                cx.update(|window, cx| {
                    sidebar.update(cx, |sidebar, cx| {
                        sidebar.open_agent_peek(id, "old-running".into(), true, window, cx);
                    });
                });
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                let preview = cx.debug_bounds("agent-peek").unwrap_or_else(|| {
                    let (open, visible, source, pane) = sidebar.read_with(cx, |sidebar, cx| {
                        (
                            sidebar.checkout_panel.peek.is_some(),
                            sidebar.checkout_panel.agents_visible,
                            sidebar.checkout_panel.agent_bounds.get("old-running").map(|b| b.get()),
                            sidebar.codex_views.get(&id).map(|view| view.read(cx).transcript_bounds.get()),
                        )
                    });
                    let composer = cx.debug_bounds("codex-composer");
                    let input = cx.debug_bounds("codex-composer-input");
                    panic!(
                        "{} preview missing at width {width} and scale {scale}: open={open}, visible={visible}, source={source:?}, pane={pane:?}, composer={composer:?}, input={input:?}",
                        provider.label()
                    );
                });
                assert_eq!(
                    cx.update(|_, cx| super::super::super::inline_html::native_exclusion(cx)),
                    Some(preview),
                    "{} preview mask follows narrow layout and zoom",
                    provider.label()
                );
                cx.simulate_keystrokes("escape");
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                assert_eq!(
                    cx.update(|_, cx| super::super::super::inline_html::native_exclusion(cx)),
                    None
                );
            }
            cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
            cx.update(|window, cx| {
                window.set_rem_size(gpui::px(crate::zoom::REM_BASE));
                let _ = window.draw(cx);
            });
            let row = cx.debug_bounds("checkout-agent-old-running").unwrap();
            // Pointer intent does not open immediately or steal composer focus.
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    gpui::Focusable::focus_handle(sidebar.codex_views[&id].read(cx), cx)
                        .focus(window, cx);
                });
            });
            let outside = gpui::point(gpui::px(1.), gpui::px(1.));
            cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
            cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
            cx.run_until_parked();
            assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_none()));
            cx.executor().advance_clock(Duration::from_millis(221));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(sidebar.read_with(cx, |sidebar, _| sidebar
                .checkout_panel
                .peek
                .as_ref()
                .is_some_and(|peek| !peek.explicit)));
            assert!(
                cx.update(|window, cx| sidebar.read(cx).codex_views[&id]
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)),
                "hover keeps composer focus"
            );
            let hover_preview = cx.debug_bounds("agent-peek").unwrap();
            assert_eq!(
                cx.update(|_, cx| super::super::super::inline_html::native_exclusion(cx)),
                Some(hover_preview),
                "{} preview mask matches the painted panel",
                provider.label()
            );
            // A short crossing outside both surfaces must not dismiss the preview.
            cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(100));
            cx.run_until_parked();
            assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_some()));
            cx.simulate_mouse_move(hover_preview.center(), None, gpui::Modifiers::default());
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(300));
            cx.run_until_parked();
            assert!(
                sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_some()),
                "entering preview cancels transit dismissal"
            );
            // Root Escape must work while hover has left focus in the composer.
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_none()));
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert_eq!(
                cx.update(|_, cx| super::super::super::inline_html::native_exclusion(cx)),
                None,
                "dismissal clears the native mask"
            );
            cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(221));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            cx.simulate_mouse_move(outside, None, gpui::Modifiers::default());
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(261));
            cx.run_until_parked();
            assert!(
                sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_none()),
                "hover dismissal runs after transit grace"
            );
            cx.simulate_click(row.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(sidebar.read_with(cx, |sidebar, _| sidebar
                .checkout_panel
                .peek
                .as_ref()
                .is_some_and(|peek| peek.explicit)));
            let preview = cx.debug_bounds("agent-peek").unwrap();
            assert_eq!(
                cx.update(|_, cx| super::super::super::inline_html::native_exclusion(cx)),
                Some(preview)
            );
            let composer = cx.debug_bounds("codex-composer").unwrap();
            assert!(preview.bottom() <= composer.top());
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_none()));
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert_eq!(
                cx.update(|_, cx| super::super::super::inline_html::native_exclusion(cx)),
                None,
                "dismissal clears the native mask"
            );
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.checkout_panel.agent_focus["old-running"].focus(window, cx);
                });
            });
            cx.simulate_keystrokes("space");
            cx.run_until_parked();
            assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.peek.is_some()));
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            let card = cx.debug_bounds("checkout-card").unwrap();
            let divider = cx.debug_bounds("checkout-agents-divider").unwrap();
            assert!(divider.left() <= card.left() + gpui::px(SPACE_XS));
            assert!(divider.right() >= card.right() - gpui::px(SPACE_XS));
            assert!(
                !sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.agent_disclosures
                    [&id]
                    .show_all)
            );
            assert!(
                !sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.agent_disclosures
                    [&id]
                    .previous)
            );
            // Taller two-line tasks may put Show more below the checkout viewport.
            let panel = cx.debug_bounds("checkout-panel").unwrap();
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: panel.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-2000.))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(cx.debug_bounds("checkout-agents-more").is_some());
            let more = cx.debug_bounds("checkout-agents-more").unwrap();
            cx.simulate_click(more.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(
                sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.agent_disclosures
                    [&id]
                    .show_all)
            );
            // Expanded activity rows use the existing checkout scroll region.
            let panel = cx.debug_bounds("checkout-panel").unwrap();
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: panel.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-2000.))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let previous = cx.debug_bounds("checkout-agents-previous").unwrap();
            assert!(
                previous.bottom() <= panel.bottom(),
                "scroll reveals previous agents"
            );
            cx.simulate_click(previous.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(
                sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.agent_disclosures
                    [&id]
                    .previous)
            );
            cx.simulate_resize(gpui::size(gpui::px(500.), gpui::px(800.)));
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.checkout_panel.expanded = false;
                    cx.notify();
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(sidebar.read_with(cx, |sidebar, cx| sidebar.codex_views[&id]
                .read(cx)
                .agents_in_checkout));
            cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(sidebar.read_with(cx, |sidebar, cx| sidebar.codex_views[&id]
                .read(cx)
                .agents_in_checkout));
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| sidebar.toggle_zen(window, cx));
                let _ = window.draw(cx);
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(
                cx.debug_bounds("checkout-summary").is_some(),
                "Agents remains reachable in Zen"
            );
            assert!(
                cx.debug_bounds("checkout-finder").is_none(),
                "Zen contains session agents only"
            );
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| sidebar.toggle_zen(window, cx));
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.open_agent_peek(id, "old-running".into(), true, window, cx);
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(cx
                .update(|_, cx| super::super::super::inline_html::native_exclusion(cx))
                .is_some());
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.selection = Some(Selection::Project(0));
                    cx.notify();
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert_eq!(
                cx.update(|_, cx| super::super::super::inline_html::native_exclusion(cx)),
                None,
                "leaving the session clears its preview mask"
            );
        }
    }

    #[test]
    fn checkout_agent_presentation_uses_observed_text_without_duplicates() {
        use grove_core::codex::{Subagent, SubagentStatus};
        let mut agent = Subagent {
            id: "one".into(),
            name: " Agent 1 ".into(),
            task: Some("  Fix\n retries  ".into()),
            activity: " Inspecting\n tests ".into(),
            role: None,
            status: SubagentStatus::Working,
        };
        let working = checkout_agent_presentation(&agent);
        assert_eq!(working.name, "Agent 1");
        assert_eq!(working.activity.as_deref(), Some("Inspecting tests"));
        assert!(working.detail.contains("Task: Fix retries"));
        for (status, label) in [
            (SubagentStatus::Completed, "Done"),
            (SubagentStatus::Failed, "Failed"),
            (SubagentStatus::Interrupted, "Interrupted"),
            (SubagentStatus::Unknown, "Unknown"),
        ] {
            agent.status = status;
            let row = checkout_agent_presentation(&agent);
            assert_eq!(row.status, label);
            assert_eq!(row.activity.as_deref(), Some("Inspecting tests"));
        }
        agent.activity = "fix RETRIES".into();
        assert_eq!(
            checkout_agent_presentation(&agent).activity.as_deref(),
            Some("fix RETRIES")
        );
        agent.activity = "Activity unavailable".into();
        assert_eq!(
            checkout_agent_presentation(&agent).activity.as_deref(),
            Some("Fix retries")
        );
        agent.activity = "fix RETRIES".into();
        agent.task = Some(" \n ".into());
        let fallback = checkout_agent_presentation(&agent);
        assert_eq!(fallback.activity.as_deref(), Some("fix RETRIES"));
        for placeholder in ["", " \n ", " Activity unavailable ", "ACTIVITY UNAVAILABLE"] {
            agent.activity = placeholder.into();
            assert_eq!(checkout_agent_presentation(&agent).name, "Agent 1");
            assert_eq!(checkout_agent_presentation(&agent).activity, None);
        }
        agent.name.clear();
        assert_eq!(checkout_agent_presentation(&agent).name, "Agent");
        agent.task = Some("Long task ".repeat(100));
        agent.activity = "Long observation ".repeat(100);
        let long = checkout_agent_presentation(&agent);
        assert!(long.activity.as_ref().unwrap().len() > 900);
        assert!(long.detail.contains(agent.task.as_deref().unwrap().trim()));
        assert!(long.detail.contains(agent.activity.trim()));
    }

    #[test]
    fn checkout_agent_groups_keep_latest_terminal_observations_and_independent_children() {
        use grove_core::codex::{Subagent, SubagentStatus, WorkPeriod};
        let agent = |id: &str, status| Subagent {
            id: id.into(),
            name: id.into(),
            activity: String::new(),
            role: None,
            task: None,
            status,
        };
        let mut periods = vec![
            WorkPeriod {
                start_item: 0,
                end_item: Some(1),
                duration_ms: None,
                subagents: vec![
                    agent("running", SubagentStatus::Working),
                    agent("unknown", SubagentStatus::Unknown),
                    agent("done", SubagentStatus::Completed),
                    agent("changed", SubagentStatus::Working),
                ],
            },
            WorkPeriod {
                start_item: 1,
                end_item: None,
                duration_ms: None,
                subagents: vec![
                    agent("current-done", SubagentStatus::Completed),
                    agent("changed", SubagentStatus::Failed),
                ],
            },
        ];
        let agents = checkout_agents(&periods);
        assert_eq!(agents.len(), 5);
        let (main, previous) = checkout_agent_groups(&agents, &periods);
        assert_eq!(
            main.iter()
                .map(|agent| agent.id.as_str())
                .collect::<Vec<_>>(),
            ["running", "unknown", "changed", "current-done"]
        );
        assert_eq!(previous[0].id, "done");
        assert_eq!(main[2].status, SubagentStatus::Failed);
        periods.push(WorkPeriod {
            start_item: 2,
            end_item: None,
            duration_ms: None,
            subagents: Vec::new(),
        });
        let (main, previous) = checkout_agent_groups(&agents, &periods);
        assert_eq!(main.len(), 2);
        assert_eq!(previous.len(), 3);
    }

    #[gpui::test]
    fn panel_controls_remain_reachable_on_home_and_when_hidden(cx: &mut gpui::TestAppContext) {
        let fixture = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(fixture.path())
            .args(["init", "-b", "main"])
            .output()
            .unwrap()
            .status
            .success());
        fs_err::write(fixture.path().join("main.rs"), "fn main() {}\n").unwrap();
        fs_err::write(fixture.path().join("README.md"), "# Example\n").unwrap();
        let fixture_path = fixture.path().to_string_lossy().into_owned();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "Checkout QA".into(),
                    path: fixture_path.clone(),
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
            let runtime = cx.new(crate::runtime::Runtime::new);
            let path = fixture_path.clone();
            let id = runtime.read(cx).registry.clone().update(cx, |registry, _| {
                registry.insert_meta("Checkout QA".into(), path.clone(), Agent::Codex)
            });
            runtime.read(cx).tree.clone().update(cx, |tree, _| {
                tree.set_active_worktrees(
                    0,
                    vec![grove_core::git::Worktree {
                        path,
                        branch: "main".into(),
                        mtime: None,
                        is_main: true,
                    }],
                );
            });
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.selection = Some(Selection::Session(id));
            sidebar.initial_selection_pending = false;
            sidebar
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.open_dashboard(window, cx));
            let _ = window.draw(cx);
        });
        for width in [1280., 760.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.)));
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    cx.set_reduce_motion(true);
                    sidebar.checkout_panel.expanded = true;
                });
                let _ = window.draw(cx);
            });
            let controls = cx.debug_bounds("side-container-controls").unwrap();
            assert!(cx.debug_bounds("checkout-summary").is_none());
            let panel = cx.debug_bounds("checkout-panel").unwrap();
            assert_eq!(panel.top(), gpui::px(0.));
            assert_eq!(panel.size.width, gpui::px(CARD_WIDTH));
            assert!(cx.debug_bounds("side-chat-toggle").is_some());
            let toggle = cx.debug_bounds("session-panel-toggle").unwrap();
            cx.simulate_click(toggle.center(), gpui::Modifiers::default());
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.hidden));
            assert!(cx.debug_bounds("checkout-card").is_none());
            assert_eq!(
                cx.debug_bounds("side-container-controls").unwrap(),
                controls
            );
            let toggle = cx.debug_bounds("session-panel-toggle").unwrap();
            cx.simulate_click(toggle.center(), gpui::Modifiers::default());
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.hidden));
            assert!(cx.debug_bounds("checkout-card").is_some());
        }
    }

    #[gpui::test]
    fn checkout_rows_pr_disclosure_and_filtered_changes_keep_layout(cx: &mut gpui::TestAppContext) {
        let fixture = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(fixture.path())
            .args(["init", "-b", "main"])
            .output()
            .unwrap()
            .status
            .success());
        fs_err::write(fixture.path().join("main.rs"), "fn main() {}\n").unwrap();
        fs_err::write(fixture.path().join("README.md"), "# Example\n").unwrap();
        let fixture_path = fixture.path().to_string_lossy().into_owned();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "Checkout QA".into(),
                    path: fixture_path.clone(),
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
            let runtime = cx.new(crate::runtime::Runtime::new);
            let path = fixture_path.clone();
            let id = runtime.read(cx).registry.clone().update(cx, |registry, _| {
                registry.insert_meta("Checkout QA".into(), path.clone(), Agent::Codex)
            });
            runtime.read(cx).tree.clone().update(cx, |tree, _| {
                tree.set_active_worktrees(
                    0,
                    vec![grove_core::git::Worktree {
                        path,
                        branch: "main".into(),
                        mtime: None,
                        is_main: true,
                    }],
                );
            });
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.selection = Some(Selection::Session(id));
            sidebar.initial_selection_pending = false;
            sidebar
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let card = cx.debug_bounds("checkout-card").unwrap();
        let panel = cx.debug_bounds("checkout-panel").unwrap();
        let toggle = cx.debug_bounds("files-toggle").unwrap();
        let checkout_toggle = cx.debug_bounds("session-panel-toggle").unwrap();
        assert!(checkout_toggle.right() < toggle.left());
        assert!((panel.right() - toggle.right() - gpui::px(SPACE_20)).abs() <= gpui::px(1.));
        assert!(card.top() >= gpui::px(APPBAR_TOTAL_H + SPACE_3XL));
        let divider = cx.debug_bounds("checkout-divider").unwrap();
        assert!(divider.left() <= card.left() + gpui::px(SPACE_XS));
        assert!(divider.right() >= card.right() - gpui::px(SPACE_XS));
        let rows = [
            (
                "checkout-finder",
                "checkout-finder-icon",
                "checkout-finder-label",
            ),
            (
                "checkout-script",
                "checkout-script-icon",
                "checkout-script-label",
            ),
            (
                "checkout-commit-publish",
                "checkout-commit-publish-icon",
                "checkout-commit-publish-label",
            ),
            (
                "checkout-changes",
                "checkout-changes-icon",
                "checkout-changes-label",
            ),
        ];
        let mut icon_x = None;
        for (id, icon_id, label_id) in rows {
            let row = cx.debug_bounds(id).unwrap();
            let glyph = cx.debug_bounds(icon_id).unwrap();
            let label = cx.debug_bounds(label_id).unwrap();
            assert_eq!(label.left() - glyph.right(), gpui::px(SPACE_SM));
            assert_eq!(glyph.left() - row.left(), gpui::px(SPACE_2XL));
            assert!(row.top() >= card.top() && row.bottom() <= card.bottom());
            assert_eq!(row.size.height, gpui::px(ROW_H));
            if let Some(x) = icon_x {
                assert_eq!(glyph.left(), x);
            } else {
                icon_x = Some(glyph.left());
            }
        }
        assert!(cx.debug_bounds("checkout-history").is_none());
        let changes = cx.debug_bounds("checkout-changes").unwrap();
        assert!(changes.left() <= card.left() + gpui::px(SPACE_XS));
        assert!(changes.right() >= card.right() - gpui::px(SPACE_XS));
        let initial_selection = sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone());
        for provider in [Agent::Codex, Agent::Claude] {
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    let registry = sidebar.runtime.read(cx).registry.clone();
                    let id = registry.update(cx, |registry, _| registry.insert_meta("Checkout QA".into(), fixture_path.clone(), provider));
                    sidebar.selection = Some(Selection::Session(id));
                    sidebar.checkout_panel.root_owner = sidebar.selection.clone();
                    let state = checkout_git::inspect(fixture.path()).unwrap();
                    sidebar.checkout_panel.pr_error = None;
                    sidebar.checkout_panel.state_error = None;
                    sidebar.checkout_panel.pr_identity = Some(state.target_identity());
                sidebar.checkout_panel.pr_error = None;
                sidebar.checkout_panel.state_error = None;
                sidebar.checkout_panel.pr_checked_at = Some(Instant::now() + Duration::from_hours(1));
                    sidebar.checkout_panel.state = Some(Ok(state));
                    // This layout fixture owns synthetic Git/PR state; periodic
                    // refreshes must not replace it during slow parallel runs.
                    sidebar.checkout_panel.checked_at = None;
                    sidebar.checkout_panel.main_checked_at = Some(Instant::now() + Duration::from_hours(1));
                    sidebar.checkout_panel.pr_checked_at =
                        Some(Instant::now() + Duration::from_hours(1));
                    sidebar.checkout_panel.pr_state = Some(Ok(Some(serde_json::from_value(serde_json::json!({
                        "number":142,"url":"https://github.com/example/project/pull/142","state":"OPEN","isDraft":false,"headRefOid":"fixture","reviewDecision":"APPROVED","mergeable":"MERGEABLE","mergeStateStatus":"BLOCKED",
                        "latestReviews":[{"state":"APPROVED"}],"reviewRequests":[],"statusCheckRollup":[{"status":"COMPLETED","conclusion":"FAILURE"}]
                    })).unwrap())));
                    sidebar.checkout_panel.summary = Some(diff::summarize(&diff::changed_files(&fixture_path)));
                    sidebar.checkout_panel.pr_expanded = false;
                    cx.notify();
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let pr = cx.debug_bounds("checkout-pr").unwrap();
            let commit = cx.debug_bounds("checkout-commit-publish").unwrap();
            let changes = cx.debug_bounds("checkout-changes").unwrap();
            assert!(commit.bottom() <= changes.top() && changes.bottom() <= pr.top());
            assert!(cx.debug_bounds("checkout-local-heading").is_none());
            assert!(cx.debug_bounds("checkout-pr-heading").is_none());
            assert!(cx.debug_bounds("checkout-merge-total").is_none());
            assert!(cx.debug_bounds("checkout-pr-readiness").is_none());
            let panel = cx.debug_bounds("checkout-panel").unwrap();
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: panel.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-2000.))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let details = cx.debug_bounds("checkout-pr-details").unwrap();
            cx.simulate_click(details.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.pr_expanded));
            cx.simulate_keystrokes("space");
            cx.run_until_parked();
            assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.pr_expanded));
            cx.simulate_keystrokes("enter");
            cx.run_until_parked();
            assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.pr_expanded));
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let expanded = cx.debug_bounds("checkout-pr-expanded").unwrap();
            let disclosure_label = cx.debug_bounds("checkout-pr-label").unwrap();
            let checks = cx.debug_bounds("checkout-pr-checks").unwrap();
            assert_eq!(checks.left(), disclosure_label.left());
            assert!(cx.debug_bounds("checkout-pr-number").is_none());
            assert!(cx.debug_bounds("checkout-pr-resolve").is_none());
            let commit = cx.debug_bounds("checkout-commit-publish").unwrap();
            let changes = cx.debug_bounds("checkout-changes").unwrap();
            assert!(commit.bottom() <= changes.top() && changes.bottom() <= expanded.top());
            for group in [diff::ChangeGroup::Code, diff::ChangeGroup::Docs] {
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                // Expanded readiness shares the existing checkout scroll region.
                // Scroll the actual panel, then prove lower actions are visible and clickable.
                let panel = cx.debug_bounds("checkout-panel").unwrap();
                let group_row = cx.debug_bounds(group.label()).unwrap();
                if group_row.bottom() > panel.bottom() || group_row.top() < panel.top() {
                    cx.simulate_event(gpui::ScrollWheelEvent {
                        position: panel.center(),
                        delta: gpui::ScrollDelta::Pixels(gpui::point(
                            gpui::px(0.),
                            panel.center().y - group_row.center().y,
                        )),
                        modifiers: gpui::Modifiers::default(),
                        touch_phase: gpui::TouchPhase::Moved,
                    });
                    cx.run_until_parked();
                    cx.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                let group_row = cx.debug_bounds(group.label()).unwrap();
                assert!(
                    group_row.top() >= panel.top() && group_row.bottom() <= panel.bottom(),
                    "checkout scrolling reveals the complete change category action"
                );
                cx.simulate_click(group_row.center(), gpui::Modifiers::default());
                cx.run_until_parked();
                cx.update(|window, cx| {
                    sidebar.update(cx, |sidebar, cx| {
                        let viewer = sidebar.diff_viewer.as_ref().unwrap().read(cx);
                        assert_eq!(viewer.group, Some(group));
                        assert!(!viewer.files.is_empty());
                        assert!(viewer
                            .files
                            .iter()
                            .all(|file| diff::ChangeGroup::of(&file.path) == group));
                        sidebar.act(Action::CloseDiff, window, cx);
                    });
                });
            }
            let panel = cx.debug_bounds("checkout-panel").unwrap();
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: panel.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-2000.))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let pr_row = cx.debug_bounds("checkout-pr").unwrap();
            assert!(
                pr_row.top() >= panel.top() && pr_row.bottom() <= panel.bottom(),
                "checkout scrolling returns to PR actions"
            );
        }
        // A server-ready PR exposes merge only with a synchronized, clean checkout.
        let (saved_state, saved_pr) = sidebar.read_with(cx, |sidebar, _| {
            (
                sidebar.checkout_panel.state.clone(),
                sidebar.checkout_panel.pr_state.clone(),
            )
        });
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let state = sidebar
                    .checkout_panel
                    .state
                    .as_mut()
                    .unwrap()
                    .as_mut()
                    .unwrap();
                state.head = Some("fixture".into());
                state.dirty = false;
                state.upstream = Some("origin/main".into());
                state.ahead = Some(0);
                state.behind = Some(0);
                sidebar.checkout_panel.pr_identity = Some(state.target_identity());
                let pr = sidebar
                    .checkout_panel
                    .pr_state
                    .as_mut()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .as_mut()
                    .unwrap();
                pr.title =
                    "Improve checkout actions with a deliberately long title for truncation".into();
                pr.mergeable = "MERGEABLE".into();
                pr.merge_state_status = "CLEAN".into();
                pr.status_check_rollup = Some(vec![
                    serde_json::json!({"status":"COMPLETED","conclusion":"SUCCESS"}),
                ]);
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let merge = cx.debug_bounds("checkout-pr-merge").unwrap();
        cx.simulate_click(merge.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("checkout-confirm-merge").is_some());
        assert!(cx.debug_bounds("checkout-card").is_some());
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.confirmation_open()));
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| assert!(sidebar.read(cx).cancel_focus.is_focused(window)));
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| assert!(sidebar.read(cx).confirm_focus.is_focused(window)));
        cx.simulate_keystrokes("shift-tab");
        cx.update(|window, cx| assert!(sidebar.read(cx).cancel_focus.is_focused(window)));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert!(sidebar.read(cx).checkout_panel.trigger_focus["pr-merge"].is_focused(window));
        });
        assert!(cx.debug_bounds("checkout-confirm-merge").is_none());
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.checkout_panel.state = saved_state;
                sidebar.checkout_panel.pr_state = saved_pr;
                sidebar.checkout_panel.pr_identity = sidebar
                    .checkout_panel
                    .state
                    .as_ref()
                    .and_then(|state| state.as_ref().ok())
                    .map(CheckoutGitState::target_identity);
                cx.notify();
            });
        });
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.selection.clone_from(&initial_selection);
                sidebar
                    .checkout_panel
                    .root_owner
                    .clone_from(&initial_selection);
                cx.notify();
            });
        });
        assert!(cx.debug_bounds("checkout-path").is_none());
        assert!(cx.debug_bounds("checkout-branch").is_none());
        cx.simulate_resize(gpui::size(gpui::px(500.), gpui::px(600.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let header = cx.debug_bounds("terminal-header-1").unwrap();
        let panel = cx.debug_bounds("checkout-panel").unwrap();
        assert_eq!(header.top(), gpui::px(0.));
        assert!(panel.top() >= header.bottom());
        assert!(cx.debug_bounds("checkout-summary").is_none());
        let narrow_card = cx.debug_bounds("checkout-card").unwrap();
        let narrow_pr = cx.debug_bounds("checkout-pr").unwrap();
        assert!(narrow_pr.left() >= narrow_card.left() && narrow_pr.right() <= narrow_card.right());
        assert!(cx.debug_bounds("checkout-pr-expanded").is_some());
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("checkout-card").is_some());

        for width in [1280., 500.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.)));
            for selection in [
                Some(Selection::Project(0)),
                Some(Selection::Session(SessionId::from_raw(u64::MAX))),
                None,
            ] {
                cx.update(|_, cx| {
                    sidebar.update(cx, |sidebar, cx| {
                        sidebar.selection = selection;
                        sidebar.checkout_panel.expanded = true;
                        assert!(sidebar.checkout_targets(cx).is_empty());
                        cx.notify();
                    });
                });
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                assert!(cx.debug_bounds("checkout-card").is_none());
                assert!(cx.debug_bounds("checkout-panel").is_none());
                assert!(cx.debug_bounds("checkout-summary").is_none());
            }
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.selection = Some(Selection::Worktree(0, fixture_path.clone()));
                    sidebar.checkout_panel.expanded = true;
                    assert_eq!(
                        sidebar.checkout_targets(cx),
                        vec![(0, fixture_path.clone())]
                    );
                    cx.notify();
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(cx.debug_bounds("checkout-card").is_some());
            assert!(cx.debug_bounds("checkout-terminal").is_some());
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod push_native_qa {
    use super::*;
    use gpui::AppContext as _;

    struct PushProbe;
    impl gpui::Render for PushProbe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .bg(c::BG())
                .font_family(crate::fonts::UI_FAMILY)
                .child(
                    project_field_well(false, false)
                        .w(rpx(CARD_WIDTH))
                        .h_auto()
                        .child(checkout_row(
                            "push-qa-branch",
                            "feat/new-sidebar-with-a-long-branch-name",
                            "git-branch",
                            window,
                            cx,
                        ))
                        .child(
                            checkout_loading_row(
                                "push-qa-busy",
                                "Pushing…",
                                "git-push",
                                true,
                                window,
                                cx,
                            )
                            .disabled(true),
                        )
                        .child(checkout_row(
                            "push-qa-changes",
                            "Changes",
                            "unified",
                            window,
                            cx,
                        )),
                )
        }
    }

    #[test]
    #[ignore = "opens an isolated native X11 push-animation window"]
    fn push_animation_linux_capture() {
        gpui_platform::application()
            .with_assets(crate::assets::Assets)
            .run(|cx| {
                gpui_component::init(cx);
                crate::fonts::register(cx).unwrap();
                let window = cx
                    .open_window(
                        gpui::WindowOptions {
                            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                                gpui::point(gpui::px(40.), gpui::px(40.)),
                                gpui::size(gpui::px(300.), gpui::px(160.)),
                            ))),
                            ..Default::default()
                        },
                        |window, cx| {
                            window.set_window_title("Grove push animation QA");
                            let probe = cx.new(|_| PushProbe);
                            cx.new(|cx| gpui_component::Root::new(probe, window, cx))
                        },
                    )
                    .unwrap();
                cx.activate(true);
                cx.spawn(async move |cx| {
                    for (theme, rem, reduced) in [
                        ("grove-dark", 16., false),
                        ("grove-light", 16., false),
                        ("grove-dark", 20., true),
                    ] {
                        cx.update(|cx| {
                            grove_core::theme::set_by_name(theme);
                            c::sync_component_theme(cx);
                            cx.set_reduce_motion(reduced);
                        });
                        cx.update_window(window.into(), |_, window, _| {
                            window.set_rem_size(gpui::px(rem));
                            window.refresh();
                        })
                        .unwrap();
                        cx.background_executor()
                            .timer(Duration::from_millis(450))
                            .await;
                        let output = format!(
                            "{}/.grove-artifacts/push-motion/native-{theme}-{rem}-{reduced}.png",
                            env!("CARGO_MANIFEST_DIR")
                        );
                        std::fs::create_dir_all(std::path::Path::new(&output).parent().unwrap())
                            .unwrap();
                        assert!(std::process::Command::new("import")
                            .args(["-window", "Grove push animation QA", &output])
                            .status()
                            .unwrap()
                            .success());
                    }
                    cx.update(|cx| cx.quit());
                })
                .detach();
            });
    }
}
