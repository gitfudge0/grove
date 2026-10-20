//! Checkout actions live beside the selected canvas, outside its scroll region.
use super::*;
use crate::views::components::{form_action, project_field_well, shimmer_label, transcript_action};
use gpui_component::{
    menu::{PopupMenu, PopupMenuItem},
    Disableable,
};
use grove_core::{
    checkout_git::{self, CheckoutGitState, GitAction},
    commit_agent::{self, CommitControl, CommitOutcome, CommitPhase},
};
use std::path::Path;

const CARD_WIDTH: f32 = 280.;
const WIDE_VIEWPORT: f32 = MODAL_W_XL + MODAL_W_SM;
const NARROW_HEIGHT_FRACTION: f32 = 0.5;
const STATUS_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

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
    path: Option<String>,
    state: Option<Result<CheckoutGitState, String>>,
    loading: bool,
    busy: bool,
    network_action: Option<(&'static str, &'static str)>,
    network_target: Option<(String, u64)>,
    message: Option<String>,
    publish: Option<String>,
    expanded: bool,
    chosen_root: Option<String>,
    root_owner: Option<Selection>,
    return_focus: Option<FocusHandle>,
    confirmation_focus: Option<FocusHandle>,
    trigger_focus: HashMap<&'static str, FocusHandle>,
    popup: Option<Entity<PopupMenu>>,
    popup_position: Option<gpui::Point<gpui::Pixels>>,
    pub(super) narrow_content: Option<AnyElement>,
    popup_subscription: Option<gpui::Subscription>,
    generation: u64,
    checked_at: Option<Instant>,
    commit_status: CommitUi,
    commit_job: u64,
    commit_job_path: Option<String>,
    commit_job_generation: u64,
    commit_control: Option<std::sync::Arc<CommitControl>>,
    restore_focus: bool,
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

impl Sidebar {
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
        if self.checkout_panel.loading || self.checkout_panel.busy {
            return;
        }
        self.checkout_panel.loading = true;
        let generation = self.checkout_panel.generation;
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { checkout_git::inspect(Path::new(&target)) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.checkout_panel.accepts_result(&path, generation) {
                    this.checkout_panel.checked_at = Some(Instant::now());
                    this.checkout_panel.state = Some(result);
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

    fn checkout_run(&mut self, path: String, action: GitAction, cx: &mut Context<Self>) {
        if self.checkout_panel.busy || self.checkout_panel.path.as_ref() != Some(&path) {
            return;
        }
        let Some(Ok(expected)) = self.checkout_panel.state.clone() else {
            return;
        };
        let generation = self.checkout_panel.generation;
        self.checkout_panel.busy = true;
        self.checkout_panel.network_target = Some((path.clone(), generation));
        self.checkout_panel.network_action = Some(match action {
            GitAction::Publish { .. } => ("Publishing…", "git-publish"),
            _ => ("Pushing…", "git-push"),
        });
        self.checkout_panel.message = None;
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
                this.checkout_panel.busy = false;
                this.checkout_panel.network_action = None;
                this.checkout_panel.network_target = None;
                if this.checkout_panel.accepts_result(&path, generation) {
                    this.checkout_panel.checked_at = Some(Instant::now());
                    this.checkout_panel.state = Some(state);
                    if result.is_ok() {
                        this.checkout_panel.publish = None;
                        this.checkout_panel.restore_focus = true;
                    }
                    this.checkout_panel.message = Some(result.map_or_else(
                        |error| error,
                        |()| "Completed. Local status refreshed.".into(),
                    ));
                }
                cx.notify();
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
        if self.checkout_panel.busy || self.checkout_panel.path.as_ref() != Some(&path) {
            return;
        }
        let provider = match self.checkout_commit_provider(cx) {
            Ok(provider) => provider,
            Err(error) => {
                self.checkout_panel.commit_status = CommitUi::Failed;
                self.checkout_panel.message = Some(error);
                cx.notify();
                return;
            }
        };
        self.checkout_panel.commit_job = self.checkout_panel.commit_job.wrapping_add(1);
        let job = self.checkout_panel.commit_job;
        let generation = self.checkout_panel.generation;
        let control = std::sync::Arc::new(CommitControl::default());
        self.checkout_panel.commit_control = Some(control.clone());
        self.checkout_panel.commit_job_path = Some(path.clone());
        self.checkout_panel.commit_job_generation = generation;
        self.checkout_panel.commit_status = CommitUi::Generating;
        self.checkout_panel.busy = true;
        self.checkout_panel.message = None;
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
                                if this.checkout_panel.commit_job != job { return; }
                                let current = this.checkout_panel.accepts_result(&path, generation);
                                match event {
                                    CommitEvent::Progress(phase) if current => {
                                        match phase {
                                            CommitPhase::Generating if !matches!(this.checkout_panel.commit_status, CommitUi::Canceling) => this.checkout_panel.commit_status = CommitUi::Generating,
                                            CommitPhase::Committing => this.checkout_panel.commit_status = CommitUi::Committing,
                                            CommitPhase::Generating => {},
                                        }
                                    }
                                    CommitEvent::Finished(finished) => {
                                        let (result, state) = *finished;
                                        this.checkout_panel.busy = false;
                                        this.checkout_panel.commit_control = None;
                                        this.checkout_panel.commit_job_path = None;
                                        if current {
                                            this.checkout_panel.state = Some(state);
                                            this.checkout_panel.checked_at = Some(Instant::now());
                                            let canceled = matches!(this.checkout_panel.commit_status, CommitUi::Canceling);
                                            match result {
                                                Ok(outcome) => { succeeded = true; this.checkout_panel.commit_status = CommitUi::Succeeded; this.checkout_panel.message = Some(format!("Committed: {}", outcome.subject)); }
                                                Err(_) if canceled => { this.checkout_panel.commit_status = CommitUi::Idle; this.checkout_panel.message = Some("Commit canceled.".into()); }
                                                Err(error) => { this.checkout_panel.commit_status = CommitUi::Failed; this.checkout_panel.message = Some(error); }
                                            }
                                        }
                                    }
                                    CommitEvent::Progress(_) => {},
                                }
                                cx.notify();
                            });
                            if updated.is_err() || finished { break 'updates; }
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            let _ = this.update(cx, |this, cx| {
                                if this.checkout_panel.commit_job == job {
                                    this.checkout_panel.busy = false; this.checkout_panel.commit_control = None; this.checkout_panel.commit_job_path = None;
                                    if this.checkout_panel.accepts_result(&path, generation) {
                                        this.checkout_panel.commit_status = CommitUi::Failed;
                                        this.checkout_panel.message = Some("Commit generation ended unexpectedly. Retry the commit.".into());
                                    }
                                    cx.notify();
                                }
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
                    if this.checkout_panel.commit_job == job && this.checkout_panel.accepts_result(&path, generation) {
                        this.checkout_panel.commit_status = CommitUi::Idle; cx.notify();
                    }
                });
            }
            cx.background_executor().timer(STATUS_REFRESH_INTERVAL).await;
            let _ = this.update(cx, |this, cx| {
                if this.checkout_panel.commit_job == job && this.checkout_panel.accepts_result(&path, generation) { cx.notify(); }
            });
        }).detach();
        cx.notify();
    }

    fn checkout_cancel_commit(&mut self, cx: &mut Context<Self>) {
        if let Some(control) = &self.checkout_panel.commit_control {
            self.checkout_panel.commit_status = if control.cancel() {
                CommitUi::Canceling
            } else {
                CommitUi::Committing
            };
            cx.notify();
        }
    }

    fn checkout_cancel_confirmation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.checkout_panel.busy {
            return;
        }
        self.checkout_panel.publish = None;
        self.checkout_panel.message = None;
        if let Some(focus) = self.checkout_panel.return_focus.take() {
            focus.focus(window, cx);
        }
        cx.notify();
    }

    fn render_checkout_confirmation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(path) = self.checkout_panel.path.clone() else {
            return div().into_any_element();
        };
        let busy = self.checkout_panel.busy;
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
                    .disabled(busy)
                    .when(busy, |button| {
                        button
                            .icon_only()
                            .child(icon("git-publish", ICON_MD, c::FG_DIM()))
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
        if let Some(message) = &self.checkout_panel.message {
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
                .disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.checkout_cancel_confirmation(window, cx);
                })),
        );
        div()
            .id("checkout-confirmation")
            .when_some(
                self.checkout_panel.confirmation_focus.as_ref(),
                gpui::InteractiveElement::track_focus,
            )
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .p(rpx(SPACE_3XL))
            .pt(rpx(APPBAR_H + SPACE_3XL))
            .flex()
            .justify_center()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.checkout_cancel_confirmation(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(form)
            .into_any_element()
    }

    pub(super) fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.checkout_panel.narrow_content = None;
        for kind in ["checkout", "publish", "summary", "commit"] {
            self.checkout_panel
                .trigger_focus
                .entry(kind)
                .or_insert_with(|| cx.focus_handle());
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
            self.checkout_panel.path = None;
            self.checkout_panel.publish = None;
            self.checkout_panel.commit_status = CommitUi::Idle;
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
            return self.render_content_body(window, cx);
        }
        let wide = Self::logical_window_width(window) >= WIDE_VIEWPORT;
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
                self.checkout_panel.path = Some(path.clone());
                self.checkout_panel.checked_at = None;
                self.checkout_panel.state = None;
                self.checkout_panel.loading = false;
                self.checkout_panel.message = None;
                self.checkout_panel.publish = None;
                self.checkout_panel.commit_status = CommitUi::Idle;
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
            if self.checkout_panel.publish.is_some() {
                return self.render_checkout_confirmation(window, cx);
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
                .child(
                    div()
                        .id("checkout-divider")
                        .debug_selector(|| "checkout-divider".into())
                        .w_full()
                        .border_t_1()
                        .border_color(c::BORDER_SOFT())
                        .my(rpx(SPACE_SM)),
                );
            let state = self.checkout_panel.state.clone().and_then(Result::ok);
            let target = path.clone();
            let busy = self.checkout_panel.busy;
            let same_job = self.checkout_panel.commit_job_path.as_ref() == Some(&path)
                && self.checkout_panel.commit_job_generation == self.checkout_panel.generation;
            let same_network =
                self.checkout_panel
                    .network_target
                    .as_ref()
                    .is_some_and(|(target, generation)| {
                        target == &path && *generation == self.checkout_panel.generation
                    });
            let active_other = busy
                && ((self.checkout_panel.commit_job_path.is_some() && !same_job)
                    || (self.checkout_panel.network_target.is_some() && !same_network));
            let (base_label, base_glyph, base_enabled) = commit_row_state(state.as_ref());
            let initial_loading =
                self.checkout_panel.loading && self.checkout_panel.state.is_none();
            let (label, glyph, enabled, loading) = if active_other {
                ("Git operation in progress", "git", false, true)
            } else if let Some((label, glyph)) = self
                .checkout_panel
                .network_action
                .filter(|_| busy && same_network)
            {
                (label, glyph, false, true)
            } else if initial_loading {
                ("Checking status…", "git", false, true)
            } else {
                match self.checkout_panel.commit_status {
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
            let publish = state.as_ref().is_some_and(|state| {
                !state.dirty
                    && state.upstream.is_none()
                    && state.branch.is_some()
                    && state.head.is_some()
                    && !state.remotes.is_empty()
            });
            let dirty = state.as_ref().is_some_and(|state| state.dirty);
            let retry = matches!(self.checkout_panel.commit_status, CommitUi::Failed);
            let cancel =
                same_job && matches!(self.checkout_panel.commit_status, CommitUi::Generating);
            card = card.child(checkout_loading_row("checkout-commit-publish", label, glyph, loading, window, cx)
                .disabled(!enabled)
                .with_focus_handle(self.checkout_panel.trigger_focus[if publish { "publish" } else { "commit" }].clone())
                .tooltip(self.checkout_panel.message.clone().unwrap_or_else(|| {
                    self.checkout_panel.state.as_ref().and_then(|state| state.as_ref().err()).cloned().unwrap_or_else(||
                        "Generate a commit message and commit staged changes, or all nonignored changes when nothing is staged.".into())
                }))
                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    if cancel { this.checkout_cancel_commit(cx); }
                    else if dirty || retry { this.checkout_begin_commit(target.clone(), cx); }
                    else if publish { this.checkout_menu(true, idx, target.clone(), event.position(), window, cx); }
                    else { this.checkout_run(target.clone(), GitAction::Push, cx); }
                })));
            let git = self.runtime.read(cx).tree.read(cx).git_states();
            let changes = git.get(crate::paths::normalize_wt_path(&path));
            let mut changes_row = checkout_loading_row(
                "checkout-changes",
                "Changes",
                "unified",
                initial_loading,
                window,
                cx,
            )
            .disabled(state.is_none());
            if let Some(changes) =
                changes.filter(|changes| changes.dirty || changes.added > 0 || changes.removed > 0)
            {
                changes_row = changes_row
                    .child(
                        div()
                            .text_color(c::GREEN())
                            .child(format!("+{}", changes.added)),
                    )
                    .child(
                        div()
                            .text_color(c::RED())
                            .child(format!("−{}", changes.removed)),
                    );
            }
            let target = path;
            card = card.child(
                changes_row.on_click(cx.listener(move |this, _, window, cx| {
                    this.act(Action::OpenWorktreeDiff(target.clone()), window, cx);
                })),
            );
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
                .child(
                    div()
                        .id("checkout-divider")
                        .debug_selector(|| "checkout-divider".into())
                        .w_full()
                        .border_t_1()
                        .border_color(c::BORDER_SOFT())
                        .my(rpx(SPACE_SM)),
                )
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
                                    this.checkout_panel.message = None;
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
                                        this.checkout_panel.path = None;
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
    fn four_icon_rows_clear_appbar_and_narrow_disclosure_keeps_header(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "Checkout QA".into(),
                    path: "/grove-checkout-layout-fixture".into(),
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
            let path = "/grove-checkout-layout-fixture".to_owned();
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
        assert!(cx.debug_bounds("checkout-card").is_some());
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
                Some(Selection::Worktree(
                    0,
                    "/grove-checkout-layout-fixture".into(),
                )),
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
