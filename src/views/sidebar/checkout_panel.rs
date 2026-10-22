//! Checkout actions live beside the selected canvas, outside its scroll region.
use super::*;
use crate::views::components::{
    form_action, project_field_well, shimmer_label, tooltip, transcript_action,
};
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
const WIDE_VIEWPORT: f32 = MODAL_W_XL + MODAL_W_SM;
const NARROW_HEIGHT_FRACTION: f32 = 0.5;
const STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
/// Minimum count-column width; wider values expand the shared columns together.
const STAT_COL_W: f32 = 48.;
const AGENT_STATUS_W: f32 = 64.;
const AGENT_LINE_H: f32 = 18.;
const AGENT_TASK_LINES: usize = 2;

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

#[derive(Default)]
pub(super) struct CheckoutPanel {
    pub(super) agents_visible: bool,
    agent_disclosures: HashMap<SessionId, AgentDisclosure>,
    agent_observer: Option<(gpui::EntityId, gpui::Subscription)>,
    path: Option<String>,
    state: Option<Result<CheckoutGitState, String>>,
    summary: Option<ChangeSummary>,
    expanded_categories: std::collections::HashSet<diff::ChangeGroup>,
    loading: bool,
    pr_state: Option<Result<Option<PullRequest>, String>>,
    pr_loading: bool,
    pr_identity: Option<[u8; 32]>,
    pr_checked_at: Option<Instant>,
    pr_expanded: bool,
    pr_composing: bool,
    pr_merge: Option<PullRequest>,
    publish: Option<String>,
    expanded: bool,
    chosen_root: Option<String>,
    root_owner: Option<Selection>,
    return_focus: Option<FocusHandle>,
    confirmation_focus: Option<FocusHandle>,
    trigger_focus: HashMap<&'static str, FocusHandle>,
    source_bounds: HashMap<&'static str, std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>>,
    popup: Option<Entity<PopupMenu>>,
    popup_position: Option<gpui::Point<gpui::Pixels>>,
    pub(super) narrow_content: Option<AnyElement>,
    popup_subscription: Option<gpui::Subscription>,
    generation: u64,
    checked_at: Option<Instant>,
    restore_focus: bool,
    operation: CheckoutOperation,
    operations: HashMap<String, CheckoutOperation>,
}

#[derive(Default)]
struct AgentDisclosure {
    show_all: bool,
    previous: bool,
}

#[derive(Default)]
struct CheckoutOperation {
    pr_message: Option<String>,
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

    fn accepts_result(&self, path: &str, generation: u64) -> bool {
        self.path.as_deref() == Some(path) && self.generation == generation
    }
}

fn selected_checkout(targets: &[(usize, String)], chosen: Option<&str>) -> Option<(usize, String)> {
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
        let source = if self.checkout_panel.pr_merge.is_some() {
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
        self.checkout_panel.publish.is_some() || self.checkout_panel.pr_merge.is_some()
    }

    fn checkout_targets(&self, cx: &App) -> Vec<(usize, String)> {
        if let Some(Selection::Session(id)) = self.selection {
            let registry = self.runtime.read(cx).registry.read(cx);
            if let Some(meta) = registry.meta(id) {
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
                    this.checkout_panel.checked_at = Some(Instant::now());
                    let identity = result.as_ref().ok().map(CheckoutGitState::target_identity);
                    if this.checkout_panel.pr_identity != identity {
                        this.checkout_panel.pr_identity = identity;
                        this.checkout_panel.pr_state = None;
                        this.checkout_panel.pr_checked_at = None;
                        this.checkout_panel.pr_expanded = false;
                    }
                    this.checkout_panel.state = Some(result);
                    this.checkout_panel.summary = summary;
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
        if self.checkout_panel.pr_loading {
            return;
        }
        self.checkout_panel.pr_loading = true;
        let generation = self.checkout_panel.generation;
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
                    if unchanged {
                        this.checkout_panel.pr_state = Some(result);
                        this.checkout_panel.pr_checked_at = Some(Instant::now());
                    }
                    cx.notify();
                }
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
                        Err(error) => this.checkout_panel.pr_state = Some(Err(error)),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn render_checkout_pr(
        &mut self,
        path: &str,
        state: Option<&CheckoutGitState>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut section = div().flex().flex_col().min_w_0();
        let status = self.checkout_panel.pr_state.clone();
        match status {
            Some(Ok(Some(pr))) => {
                let url = pr.url.clone();
                section = section.child(
                    checkout_row("checkout-pr", pr.title(), "git-pull-request", window, cx)
                        .tooltip("Open pull request in browser")
                        .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))),
                );
                let mut metadata = div()
                    .id("checkout-pr-metadata")
                    .debug_selector(|| "checkout-pr-metadata".into())
                    .flex()
                    .items_center()
                    .min_w_0()
                    .pl(rpx(SPACE_2XL + ICON_LG + SPACE_2XL))
                    .pr(rpx(SPACE_2XL))
                    .gap(rpx(SPACE_MD))
                    .min_h(rpx(CONTROL_H))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(
                        div()
                            .id("checkout-pr-number")
                            .debug_selector(|| "checkout-pr-number".into())
                            .flex_shrink_0()
                            .child(format!("#{}", pr.number)),
                    )
                    .child(
                        checkout_action(
                            "checkout-pr-details",
                            pr.compact_status(),
                            false,
                            window,
                            cx,
                        )
                        .debug_selector(|| "checkout-pr-details".into())
                        .flex_1()
                        .min_w_0()
                        .px(rpx(SPACE_XS))
                        .tooltip(if self.checkout_panel.pr_expanded {
                            "Hide checks and reviews"
                        } else {
                            "Show checks and reviews"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.checkout_panel.pr_expanded = !this.checkout_panel.pr_expanded;
                            cx.notify();
                        })),
                    );
                if pr.state == "OPEN" && pr.mergeable == "CONFLICTING" {
                    let target = path.to_string();
                    let request = pr.clone();
                    metadata = metadata.child(
                        checkout_action("checkout-pr-resolve", "Resolve", false, window, cx)
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/git-branch.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .debug_selector(|| "checkout-pr-resolve".into())
                            .disabled(self.checkout_panel.operation.busy)
                            .tooltip("Prepare a new agent session with a conflict-resolution draft")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.checkout_resolve_pr(target.clone(), request.clone(), cx);
                            })),
                    );
                } else if state.is_some_and(|state| pr.can_merge(state)) {
                    let request = pr.clone();
                    metadata = metadata.child(
                        checkout_action("checkout-pr-merge", "Merge", false, window, cx)
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/check.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .debug_selector(|| "checkout-pr-merge".into())
                            .relative()
                            .child({
                                let bounds = self.checkout_panel.source_bounds["pr-merge"].clone();
                                gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                                    .absolute()
                                    .inset_0()
                            })
                            .with_focus_handle(
                                self.checkout_panel.trigger_focus["pr-merge"].clone(),
                            )
                            .disabled(self.checkout_panel.operation.busy)
                            .tooltip("Review and confirm a merge commit")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.checkout_panel.return_focus =
                                    this.checkout_panel.trigger_focus.get("pr-merge").cloned();
                                this.checkout_panel.pr_merge = Some(request.clone());
                                this.checkout_panel.operation.message = None;
                                if this.checkout_panel.confirmation_focus.is_none() {
                                    this.checkout_panel.confirmation_focus =
                                        Some(cx.focus_handle());
                                }
                                if let Some(focus) = &this.checkout_panel.confirmation_focus {
                                    focus.focus(window, cx);
                                }
                                cx.notify();
                            })),
                    );
                }
                section = section.child(metadata);
                if self.checkout_panel.pr_expanded {
                    section = section.child(
                        div()
                            .id("checkout-pr-expanded")
                            .debug_selector(|| "checkout-pr-expanded".into())
                            .flex()
                            .flex_col()
                            .pl(rpx(SPACE_2XL + ICON_LG + SPACE_2XL))
                            .pr(rpx(SPACE_2XL))
                            .py(rpx(SPACE_SM))
                            .gap(rpx(SPACE_SM))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(
                                div()
                                    .id("checkout-pr-checks")
                                    .debug_selector(|| "checkout-pr-checks".into())
                                    .child(pr.checks()),
                            )
                            .child(pr.review_progress()),
                    );
                }
            }
            Some(Ok(None)) => {
                let ready = state.is_some_and(|s| {
                    s.branch.is_some() && s.upstream.is_some() && s.ahead == Some(0)
                });
                let target = path.to_string();
                section = section.child(checkout_loading_row("checkout-pr", if self.checkout_panel.pr_composing { "Preparing PR…" } else { "Create PR…" }, "plus", self.checkout_panel.pr_composing, window, cx)
                    .disabled(!ready || self.checkout_panel.operation.busy || self.checkout_panel.pr_composing)
                    .tooltip(if ready { "Review title, description, base and changes on GitHub before creating the PR" } else { "Publish or push this branch before creating a PR" })
                    .on_click(cx.listener(move |this, _, _, cx| this.checkout_compose_pr(target.clone(), cx))));
            }
            Some(Err(error)) => {
                section = section.child(
                    checkout_row("checkout-pr", "PR status unavailable", "git", window, cx)
                        .tooltip(error.clone())
                        .disabled(self.checkout_panel.pr_loading)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.checkout_panel.pr_checked_at = None;
                            this.checkout_panel.pr_state = None;
                            cx.notify();
                        })),
                );
                section = section.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .whitespace_normal()
                        .px(rpx(SPACE_2XL))
                        .pb(rpx(SPACE_SM))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(error),
                );
            }
            None => {
                section = section.child(
                    checkout_loading_row("checkout-pr", "Checking PR…", "git", true, window, cx)
                        .disabled(true),
                );
            }
        }
        if let Some(message) = &self.checkout_panel.operation.pr_message {
            section = section.child(
                div()
                    .px(rpx(SPACE_2XL))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(message.clone()),
            );
        }
        section.into_any_element()
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
        self.checkout_panel.operation.busy = true;
        self.checkout_panel.operation.message = Some("Validating pull request…".into());
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
                        if current {
                            panel.pr_checked_at = None;
                            panel.pr_state = None;
                            panel.checked_at = None;
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
        self.checkout_panel.operation.busy = true;
        self.checkout_panel.operation.network_target = Some((path.clone(), generation));
        self.checkout_panel.operation.network_action = Some(match action {
            GitAction::Publish { .. } => ("Publishing…", "git-publish"),
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
                                panel.publish = None;
                                panel.restore_focus = true;
                            }
                        }
                        panel.operation.message = Some(result.map_or_else(
                            |error| error,
                            |()| "Completed. Local status refreshed.".into(),
                        ));
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
                )
                .child(
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
                    .when(busy, |button| {
                        button
                            .icon_only()
                            .child(icon("git-publish", ICON_SM, c::FG_DIM()))
                            .child(shimmer_label(
                                "checkout-publish-shimmer".into(),
                                "Publishing…".into(),
                                true,
                                cx,
                            ))
                    })
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
                .child("The exact PR head, checks, reviews and checkout will be revalidated. Repository protections apply.")
                .child(form_action("checkout-confirm-merge", if busy { "Merging…" } else { "Create merge commit" }, true, window, cx)
                    .debug_selector(|| "checkout-confirm-merge".into())
                    .icon(gpui_component::Icon::default().path("icons/check.svg").size(rpx(ICON_SM)))
                    .with_focus_handle(self.confirm_focus.clone())
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.checkout_merge_pr(merge_path.clone(), pr.clone(), cx);
                    })));
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
        form = form.child(
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
        if self
            .checkout_panel
            .agent_observer
            .as_ref()
            .is_none_or(|(owner, _)| *owner != session.entity_id())
        {
            let observer = cx.observe(&session, |_, _, cx| cx.notify());
            self.checkout_panel.agent_observer = Some((session.entity_id(), observer));
        }
        let saved = &session.read(cx).state.saved;
        let provider = saved.provider.agent().icon_name();
        let agents = checkout_agents(&saved.work_periods);
        if agents.is_empty() {
            return None;
        }
        let running = agents
            .iter()
            .filter(|agent| agent.status == grove_core::codex::SubagentStatus::Working)
            .count();
        let (active, previous) = checkout_agent_groups(&agents, &saved.work_periods);
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
                    .map(|agent| checkout_agent_row(agent, provider)),
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
                        .map(|agent| checkout_agent_row(agent, provider)),
                );
            }
        }
        Some(section.into_any_element())
    }

    pub(super) fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.checkout_panel.narrow_content = None;
        self.checkout_panel.agents_visible = false;
        for kind in ["checkout", "publish", "summary", "commit", "pr-merge"] {
            self.checkout_panel
                .trigger_focus
                .entry(kind)
                .or_insert_with(|| cx.focus_handle());
            self.checkout_panel.source_bounds.entry(kind).or_default();
        }
        if self.checkout_panel.restore_focus {
            self.checkout_panel.restore_focus = false;
            if let Some(focus) = self.checkout_panel.return_focus.take() {
                focus.focus(window, cx);
            }
        }
        let targets = self.checkout_targets(cx);
        if self.checkout_panel.root_owner != self.selection {
            self.checkout_panel.root_owner = self.selection.clone();
            self.checkout_panel.chosen_root = None;
            self.checkout_panel.select_path(None);
            self.checkout_panel.publish = None;
            self.checkout_panel.popup = None;
            self.checkout_panel.popup_subscription = None;
            self.checkout_panel.generation = self.checkout_panel.generation.wrapping_add(1);
        }
        if self.mode == ViewMode::Grid
            || self.is_zen()
            || self.project_panel.is_some()
            || self.project_setup.is_some()
            || self.pending_new_worktree.is_some()
            || self.pending_worktree_removal.is_some()
            || self
                .settings_panel
                .as_ref()
                .is_some_and(|panel| panel.read(cx).is_open())
            || targets.is_empty()
        {
            self.checkout_panel.agent_observer = None;
            return self.render_content_body(window, cx);
        }
        let wide = Self::logical_window_width(window) >= WIDE_VIEWPORT;
        self.checkout_panel.agents_visible = wide || self.checkout_panel.expanded;
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
        if let Some((idx, path)) = selected {
            if self.checkout_panel.path.as_ref() != Some(&path) {
                self.checkout_panel.generation = self.checkout_panel.generation.wrapping_add(1);
                self.checkout_panel.select_path(Some(path.clone()));
                self.checkout_panel.checked_at = None;
                self.checkout_panel.state = None;
                self.checkout_panel.pr_state = None;
                self.checkout_panel.pr_identity = None;
                self.checkout_panel.pr_loading = false;
                self.checkout_panel.pr_checked_at = None;
                self.checkout_panel.pr_expanded = false;
                self.checkout_panel.pr_composing = false;
                self.checkout_panel.pr_merge = None;
                self.checkout_panel.summary = None;
                self.checkout_panel.expanded_categories.clear();
                self.checkout_panel.loading = false;
                self.checkout_panel.publish = None;
            }
            if (self.checkout_panel.state.is_none()
                || self
                    .checkout_panel
                    .checked_at
                    .is_some_and(|checked| checked.elapsed() >= STATUS_REFRESH_INTERVAL))
                && !self.checkout_panel.loading
            {
                self.checkout_load(path.clone(), cx);
            }
            let project = cx.global::<SettingsState>().store.projects[idx].clone();
            let target = path.clone();
            card = card.child(
                checkout_row("checkout-finder", "Open in Finder", "folder", window, cx).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.act(Action::Reveal(target.clone()), window, cx);
                    }),
                ),
            );
            let target = path.clone();
            let project_path = project.path.clone();
            let has_script = project
                .scripts
                .run
                .as_deref()
                .is_some_and(|script| !script.trim().is_empty());
            card = card
                .child(
                    checkout_row(
                        "checkout-script",
                        if has_script {
                            "Run script"
                        } else {
                            "Add project script"
                        },
                        if has_script { "play" } else { "plus" },
                        window,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.act(
                            if has_script {
                                Action::RunScript(project_path.clone(), target.clone())
                            } else {
                                Action::EditProject(project_path.clone())
                            },
                            window,
                            cx,
                        );
                    })),
                )
                .child(checkout_divider("checkout-divider"));
            let state = self.checkout_panel.state.clone().and_then(Result::ok);
            if let Some(state) = state.as_ref() {
                let identity = state.target_identity();
                if self.checkout_panel.pr_identity.as_ref() != Some(&identity) {
                    self.checkout_panel.pr_identity = Some(identity);
                    self.checkout_panel.pr_state = None;
                    self.checkout_panel.pr_checked_at = None;
                    self.checkout_panel.pr_expanded = false;
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
            card = card.child(self.render_checkout_pr(&path, state.as_ref(), window, cx));
            let target = path.clone();
            let busy = self.checkout_panel.operation.busy;
            let same_job = self.checkout_panel.operation.commit_job_path.as_ref() == Some(&path);
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
                (label, glyph, false, true)
            } else if initial_loading {
                ("Checking status…", "git", false, true)
            } else {
                match self.checkout_panel.operation.commit_status {
                    CommitUi::Generating if same_job => {
                        ("Generating… Cancel", "sparkle", true, true)
                    }
                    CommitUi::Canceling if same_job => ("Canceling…", "stop", false, true),
                    CommitUi::Committing if same_job => ("Committing…", "git", false, true),
                    CommitUi::Succeeded => ("Committed", "check", false, false),
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
                .disabled(!enabled)
                .with_focus_handle(self.checkout_panel.trigger_focus[if publish { "publish" } else { "commit" }].clone())
                .relative()
                .child({ let bounds = self.checkout_panel.source_bounds["publish"].clone();
                    gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {}).absolute().inset_0() })
                .tooltip(self.checkout_panel.operation.message.clone().unwrap_or_else(|| {
                    self.checkout_panel.state.as_ref().and_then(|state| state.as_ref().err()).cloned().unwrap_or_else(||
                        "Generate a commit message and commit staged changes, or all nonignored changes when nothing is staged.".into())
                }))
                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
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
                Some(summary) => Some(summary.groups.iter().fold((0u32, 0u32), |(a, r), g| {
                    (a.saturating_add(g.1), r.saturating_add(g.2))
                })),
                None => {
                    let git = self.runtime.read(cx).tree.read(cx).git_states();
                    git.get(crate::paths::normalize_wt_path(&path))
                        .filter(|changes| changes.dirty || changes.added > 0 || changes.removed > 0)
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
                initial_loading,
                window,
                cx,
            )
            .disabled(state.is_none());
            if let Some((added, removed)) = totals {
                changes_row = changes_row.child(change_stat_counts(added, removed, stat_widths));
            }
            let target = path.clone();
            card = card.child(
                changes_row.on_click(cx.listener(move |this, _, window, cx| {
                    this.act(Action::OpenWorktreeDiff(target.clone()), window, cx);
                })),
            );
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
                                    if !this.checkout_panel.expanded_categories.remove(&group) {
                                        this.checkout_panel.expanded_categories.insert(group);
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
                                .child(group.label()),
                        )
                        .child(change_stat_counts(added, removed, stat_widths))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.act(Action::OpenWorktreeDiff(target.clone()), window, cx);
                                if let Some(viewer) = &this.diff_viewer {
                                    viewer.update(cx, |viewer, cx| {
                                        viewer.set_category(group, None, classifier.clone(), cx);
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
                                .tooltip(label.clone())
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .pl(rpx(CONTROL_H + SPACE_LG))
                                        .truncate()
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(c::FG_DIM())
                                        .child(label),
                                )
                                .child(change_stat_counts(*added, *removed, stat_widths))
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
                                                    Some(child.clone()),
                                                    classifier.clone(),
                                                    cx,
                                                );
                                            });
                                        }
                                    },
                                )),
                            );
                        }
                    }
                }
            }
        } else {
            let target = targets.first().cloned();
            card = card
                .child(
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
                )
                .child(
                    checkout_row("checkout-script", "Add project script", "plus", window, cx)
                        .disabled(true),
                )
                .child(checkout_divider("checkout-divider"))
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
        if let Some(agents) = self.render_checkout_agents(window, cx) {
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
            .when(wide, |panel| panel.pt(rpx(APPBAR_H + SPACE_3XL)))
            .overflow_y_scroll()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.checkout_panel.expanded {
                    this.checkout_panel.expanded = false;
                    this.checkout_panel.trigger_focus["summary"].focus(window, cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }));
        if !wide {
            panel = panel.child(
                transcript_action("checkout-summary", "Checkout", false, window, cx)
                    .debug_selector(|| "checkout-summary".into())
                    .with_focus_handle(self.checkout_panel.trigger_focus["summary"].clone())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.checkout_panel.expanded = !this.checkout_panel.expanded;
                        this.checkout_panel.trigger_focus["summary"].focus(window, cx);
                        cx.notify();
                    })),
            );
        }
        if wide || self.checkout_panel.expanded {
            panel = panel.child(card);
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
            div()
                .size_full()
                .min_w_0()
                .min_h_0()
                .flex()
                .child(div().flex_1().min_w_0().min_h_0().child(body))
                .child(panel)
                .into_any_element()
        } else {
            self.checkout_panel.narrow_content = Some(panel.into_any_element());
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
        self.checkout_panel.return_focus = self
            .checkout_panel
            .trigger_focus
            .get(if publish { "publish" } else { "checkout" })
            .cloned();
        self.checkout_panel.popup_position = Some(position);
        if self.checkout_panel.confirmation_focus.is_none() {
            self.checkout_panel.confirmation_focus = Some(cx.focus_handle());
        }
        let owner = cx.entity().downgrade();
        let state = self.checkout_panel.state.clone().and_then(Result::ok);
        let project_path = cx.global::<SettingsState>().store.projects[idx]
            .path
            .clone();
        let targets = self.checkout_targets(cx);
        let popup = PopupMenu::build(window, cx, move |mut popup, _, _| {
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
                if state.as_ref().is_some_and(|state| state.head.is_some()) {
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
struct AgentPresentation {
    heading: String,
    name: String,
    activity: Option<String>,
    status: &'static str,
    detail: String,
}

fn normalized_agent_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn informative_agent_activity(text: &str) -> Option<String> {
    let text = normalized_agent_text(text);
    (!text.is_empty() && !text.eq_ignore_ascii_case("Activity unavailable")).then_some(text)
}

fn checkout_agent_presentation(agent: &grove_core::codex::Subagent) -> AgentPresentation {
    use grove_core::codex::SubagentStatus;
    let status = match agent.status {
        SubagentStatus::Working => "Working",
        SubagentStatus::Completed => "Done",
        SubagentStatus::Failed => "Failed",
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
    let heading = task
        .clone()
        .or_else(|| observed.clone())
        .unwrap_or_else(|| name.clone());
    let activity = observed.filter(|text| !text.eq_ignore_ascii_case(&heading));
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
        heading,
        name,
        activity,
        status,
        detail,
    }
}

fn checkout_agent_row(
    agent: &grove_core::codex::Subagent,
    provider: &'static str,
) -> gpui::Stateful<gpui::Div> {
    use grove_core::codex::SubagentStatus;
    let presentation = checkout_agent_presentation(agent);
    let name_id = agent.id.clone();
    let task_id = agent.id.clone();
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
        .role(gpui::Role::Group)
        .tab_index(0)
        .focus_visible(|style| style.bg(c::BG_HOVER()))
        .aria_label(detail.clone())
        .tooltip(move |window, cx| tooltip(detail.clone(), window).build(window, cx))
        .px(rpx(SPACE_2XL))
        .py(rpx(SPACE_MD))
        .flex()
        .flex_col()
        .min_w_0()
        .text_size(rpx(TEXT_SMALL))
        .font_weight(gpui::FontWeight::NORMAL)
        .text_color(c::FG_DIM())
        .line_height(rpx(AGENT_LINE_H))
        .child(
            div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_SM))
                .mb(rpx(SPACE_SM))
                .min_w_0()
                .line_height(rpx(SPACE_3XL))
                .child(
                    div()
                        .flex_shrink_0()
                        .child(icon(provider, ICON_SM, c::FG_DIM())),
                )
                .child(
                    div()
                        .debug_selector(move || format!("checkout-agent-name-{name_id}"))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(rpx(TEXT_MICRO))
                        .child(presentation.name),
                ),
        )
        .child(
            div()
                .flex()
                .items_start()
                .gap(rpx(SPACE_LG))
                .min_w_0()
                .text_size(rpx(TEXT_BODY))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(
                    div()
                        .debug_selector(move || format!("checkout-agent-task-{task_id}"))
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .line_clamp(AGENT_TASK_LINES)
                        .text_color(c::FG())
                        .child(presentation.heading),
                )
                .child(
                    div()
                        .debug_selector(move || format!("checkout-agent-status-{status_id}"))
                        .w(rpx(AGENT_STATUS_W))
                        .flex_shrink_0()
                        .text_right()
                        .text_color(match agent.status {
                            SubagentStatus::Working => c::BLUE(),
                            SubagentStatus::Completed => c::GREEN(),
                            SubagentStatus::Failed => c::FORM_ERROR(),
                            SubagentStatus::Unknown => c::FG_DIM(),
                        })
                        .child(presentation.status),
                ),
        )
        // Reserve one supporting line so activity updates do not move following rows.
        .child(
            div()
                .debug_selector(move || format!("checkout-agent-activity-{activity_id}"))
                .mt(rpx(SPACE_XS))
                .h(rpx(AGENT_LINE_H))
                .w_full()
                .min_w_0()
                .truncate()
                .text_size(rpx(TEXT_SMALL))
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
                .gap(rpx(SPACE_2XL))
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-icon")))
                        .debug_selector(move || format!("{id}-icon"))
                        .w(rpx(ICON_LG))
                        .flex_shrink_0()
                        .child(icon(glyph, ICON_MD, c::FG_DIM())),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-label")))
                        .debug_selector(move || format!("{id}-label"))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(shimmer_label(
                            SharedString::from(format!("{id}-shimmer")).into(),
                            label.to_string(),
                            loading,
                            cx,
                        )),
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
                rich_plans: grove_core::rich_plan::PlanStore::default(),
                claude_resume: None,
                edit_checkpoints: Vec::new(),
                pending_edit: None,
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
                items: Vec::new(),
                work_periods: Vec::new(),
                draft: String::new(),
                draft_images: Vec::new(),
                config_args: Vec::new(),
                native_config_version: 1,
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
                rich_plans: grove_core::rich_plan::PlanStore::default(),
                claude_resume: None,
                edit_checkpoints: Vec::new(),
                pending_edit: None,
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
                items: Vec::new(),
                work_periods: Vec::new(),
                draft: String::new(),
                draft_images: Vec::new(),
                config_args: Vec::new(),
                native_config_version: 1,
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
                    ];
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
                    for label in ["Working", "Done", "Failed", "Unknown"] {
                        let mut run = window.text_style().to_run(label.len());
                        run.font.family = crate::fonts::UI_FAMILY.into();
                        run.font.weight = gpui::FontWeight::SEMIBOLD;
                        let width = window
                            .text_system()
                            .shape_line(
                                label.into(),
                                rpx(TEXT_BODY).to_pixels(window.rem_size()),
                                &[run],
                                None,
                            )
                            .width;
                        assert!(
                            width <= rpx(AGENT_STATUS_W).to_pixels(window.rem_size()),
                            "{label} must fit the fixed status slot"
                        );
                    }
                });
                assert!(cx.debug_bounds("checkout-agents").is_some());
                let card = cx.debug_bounds("checkout-card").unwrap();
                let name = cx.debug_bounds("checkout-agent-name-old-running").unwrap();
                let status = cx
                    .debug_bounds("checkout-agent-status-old-running")
                    .unwrap();
                assert!(status.right() < card.right());
                let task = cx.debug_bounds("checkout-agent-task-old-running").unwrap();
                let activity = cx
                    .debug_bounds("checkout-agent-activity-old-running")
                    .unwrap();
                // Native layout rounds fractional zoom dimensions to device pixels.
                assert!(
                    task.size.height
                        <= gpui::px((AGENT_LINE_H * AGENT_TASK_LINES as f32 * scale).ceil())
                );
                assert!(name.bottom() < task.top());
                assert!(task.right() <= status.left());
                assert_eq!(task.top(), status.top());
                assert!(
                    (activity.top() - task.bottom() - gpui::px(SPACE_XS * scale)).abs()
                        < gpui::px(0.5)
                );
                assert!(
                    (activity.size.height - gpui::px(AGENT_LINE_H * scale)).abs() < gpui::px(0.5)
                );
                assert!(task.left() >= card.left() && task.right() < card.right());
                assert!(activity.right() < card.right());
            }
            cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
            cx.update(|window, cx| {
                window.set_rem_size(gpui::px(crate::zoom::REM_BASE));
                let _ = window.draw(cx);
            });
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
            assert!(
                !sidebar.read_with(cx, |sidebar, cx| sidebar.codex_views[&id]
                    .read(cx)
                    .agents_in_checkout)
            );
            cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(sidebar.read_with(cx, |sidebar, cx| sidebar.codex_views[&id]
                .read(cx)
                .agents_in_checkout));
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
        assert_eq!(working.heading, "Fix retries");
        assert_eq!(working.activity.as_deref(), Some("Inspecting tests"));
        assert!(working.detail.contains("Task: Fix retries"));
        for (status, label) in [
            (SubagentStatus::Completed, "Done"),
            (SubagentStatus::Failed, "Failed"),
            (SubagentStatus::Unknown, "Unknown"),
        ] {
            agent.status = status;
            let row = checkout_agent_presentation(&agent);
            assert_eq!(row.status, label);
            assert_eq!(row.activity.as_deref(), Some("Inspecting tests"));
        }
        agent.activity = "fix RETRIES".into();
        assert_eq!(checkout_agent_presentation(&agent).activity, None);
        agent.task = Some(" \n ".into());
        let fallback = checkout_agent_presentation(&agent);
        assert_eq!(fallback.heading, "fix RETRIES");
        assert_eq!(fallback.activity, None);
        for placeholder in ["", " \n ", " Activity unavailable ", "ACTIVITY UNAVAILABLE"] {
            agent.activity = placeholder.into();
            assert_eq!(checkout_agent_presentation(&agent).heading, "Agent 1");
            assert_eq!(checkout_agent_presentation(&agent).activity, None);
        }
        agent.name.clear();
        assert_eq!(checkout_agent_presentation(&agent).heading, "Agent");
        agent.task = Some("Long task ".repeat(100));
        agent.activity = "Long observation ".repeat(100);
        let long = checkout_agent_presentation(&agent);
        assert!(long.heading.len() > 900);
        assert!(long.detail.contains(long.heading.as_str()));
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
        assert!(card.top() >= gpui::px(APPBAR_H + SPACE_3XL));
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
            assert_eq!(label.left() - glyph.right(), gpui::px(SPACE_2XL));
            assert_eq!(glyph.left() - row.left(), gpui::px(SPACE_2XL));
            assert!(row.top() >= card.top() && row.bottom() <= card.bottom());
            assert_eq!(row.size.height, gpui::px(ROW_H));
            if let Some(x) = icon_x {
                assert_eq!(glyph.left(), x);
            } else {
                icon_x = Some(glyph.left());
            }
        }
        let initial_selection = sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone());
        for provider in [Agent::Codex, Agent::Claude] {
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    let registry = sidebar.runtime.read(cx).registry.clone();
                    let id = registry.update(cx, |registry, _| registry.insert_meta("Checkout QA".into(), fixture_path.clone(), provider));
                    sidebar.selection = Some(Selection::Session(id));
                    sidebar.checkout_panel.root_owner = sidebar.selection.clone();
                    let state = checkout_git::inspect(fixture.path()).unwrap();
                    sidebar.checkout_panel.pr_identity = Some(state.target_identity());
                    sidebar.checkout_panel.state = Some(Ok(state));
                    sidebar.checkout_panel.checked_at = Some(Instant::now());
                    sidebar.checkout_panel.pr_checked_at = Some(Instant::now());
                    sidebar.checkout_panel.pr_state = Some(Ok(Some(serde_json::from_value(serde_json::json!({
                        "number":142,"url":"https://github.com/example/project/pull/142","state":"OPEN","isDraft":false,"headRefOid":"fixture","reviewDecision":"APPROVED","mergeable":"CONFLICTING",
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
            assert!(pr.bottom() <= commit.top() && commit.bottom() <= changes.top());
            let details = cx.debug_bounds("checkout-pr-details").unwrap();
            cx.simulate_click(details.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let expanded = cx.debug_bounds("checkout-pr-expanded").unwrap();
            let disclosure_label = cx.debug_bounds("checkout-pr-label").unwrap();
            let checks = cx.debug_bounds("checkout-pr-checks").unwrap();
            assert_eq!(checks.left(), disclosure_label.left());
            assert_eq!(
                cx.debug_bounds("checkout-pr-number").unwrap().left(),
                disclosure_label.left()
            );
            let resolve = cx.debug_bounds("checkout-pr-resolve").unwrap();
            assert!(resolve.right() <= cx.debug_bounds("checkout-card").unwrap().right());
            let commit = cx.debug_bounds("checkout-commit-publish").unwrap();
            let changes = cx.debug_bounds("checkout-changes").unwrap();
            assert!(expanded.bottom() <= commit.top() && commit.bottom() <= changes.top());
            for group in [diff::ChangeGroup::Code, diff::ChangeGroup::Docs] {
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                let group_row = cx.debug_bounds(group.label()).unwrap();
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
        assert!(cx.debug_bounds("checkout-card").is_none());
        let summary = cx.debug_bounds("checkout-summary").unwrap();
        cx.simulate_click(summary.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let narrow_card = cx.debug_bounds("checkout-card").unwrap();
        let narrow_pr = cx.debug_bounds("checkout-pr").unwrap();
        assert!(narrow_pr.left() >= narrow_card.left() && narrow_pr.right() <= narrow_card.right());
        assert!(cx.debug_bounds("checkout-pr-expanded").is_some());
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("checkout-card").is_none());

        for width in [1280., 500.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.)));
            for selection in [
                Some(Selection::Project(0)),
                Some(Selection::Worktree(0, fixture_path.clone())),
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
        }
    }
}
