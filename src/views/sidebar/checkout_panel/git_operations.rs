//! Checkout-scoped Git refresh, mutations, and generated commit jobs.

use super::*;

const COMMIT_POLL_INTERVAL: Duration = Duration::from_millis(80);
const COMMIT_SUCCESS_FEEDBACK: Duration = Duration::from_secs(2);

impl CheckoutPanel {
    /// Keep operation state with its checkout while view inspection remains generation scoped.
    pub(super) fn select_path(&mut self, path: Option<String>) {
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
        self.open_script_preview = None;
        self.path = path;
    }

    /// Deliver background events to their owner, including while another checkout is visible.
    pub(super) fn update_operation(&mut self, path: &str, update: impl FnOnce(&mut Self, bool)) {
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

    pub(super) fn commit_event(&mut self, path: &str, job: u64, event: CommitEvent) -> bool {
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
                            operation.failures.insert("commit", error.clone());
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

    pub(super) fn local_refresh_due(&self, focused: bool) -> bool {
        !self.loading
            && !self.operation.busy
            && (self.state.is_none()
                || self.checked_at.is_none_or(|time| {
                    time.elapsed()
                        >= if focused {
                            STATUS_REFRESH_INTERVAL
                        } else {
                            Duration::from_mins(1)
                        }
                }))
    }

    pub(super) fn accepts_sample(&self, path: &str, generation: u64, revision: u64) -> bool {
        self.accepts_result(path, generation)
            && self.operation.revision == revision
            && !self.operation.busy
    }

    pub(super) fn accepts_result(&self, path: &str, generation: u64) -> bool {
        self.path.as_deref() == Some(path) && self.generation == generation
    }
}

impl Sidebar {
    pub(super) fn checkout_load(&mut self, path: String, cx: &mut Context<Self>) {
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
                    let result = checkout_git::inspect_for_poll(Path::new(&target));
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

    pub(super) fn checkout_load_main(
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

    pub(super) fn checkout_prepare_recovery(
        &mut self,
        path: String,
        revert: bool,
        cx: &mut Context<Self>,
    ) {
        if self.checkout_panel.operation.busy || self.checkout_panel.state_error.is_some() {
            return;
        }
        let Some(Ok(expected)) = self.checkout_panel.state.clone() else {
            return;
        };
        self.checkout_panel.operation.failures.remove("recovery");
        let feedback_token =
            self.begin_panel_feedback(&path, "recovery", "Preparing recovery…", cx);
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
                this.finish_panel_feedback(&path, "recovery", feedback_token, None, cx);
                this.checkout_panel
                    .update_operation(&path, |panel, current| {
                        panel.operation.busy = false;
                        if let Err(error) = &result {
                            panel.operation.failures.insert("recovery", error.clone());
                        }
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
                                        panel.operation.failures.insert(
                                            "recovery",
                                            "No ordinary commit is available to revert".into(),
                                        );
                                        return;
                                    }
                                } else {
                                    if files.is_empty() {
                                        panel.operation.failures.insert(
                                            "recovery",
                                            "No tracked edits to discard".into(),
                                        );
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

    pub(super) fn checkout_execute_recovery(
        &mut self,
        path: String,
        recovery: CheckoutRecovery,
        cx: &mut Context<Self>,
    ) {
        if self.checkout_panel.operation.busy || self.checkout_panel.path.as_ref() != Some(&path) {
            return;
        }
        let (feedback_action, working, completed) = match &recovery {
            CheckoutRecovery::Discard(_) => (
                "recovery",
                "Discarding tracked edits…".to_owned(),
                "Tracked edits discarded".to_owned(),
            ),
            CheckoutRecovery::Revert(..) => (
                "recovery",
                "Creating revert commit…".to_owned(),
                "Revert commit created".to_owned(),
            ),
            CheckoutRecovery::Main(_, main) => (
                "main",
                format!("Merging {}…", main.branch),
                format!("Merged {}", main.branch),
            ),
        };
        self.checkout_panel
            .operation
            .failures
            .remove(feedback_action);
        let feedback_token = self.begin_panel_feedback(&path, feedback_action, &working, cx);
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
                this.finish_panel_feedback(
                    &path,
                    feedback_action,
                    feedback_token,
                    result.is_ok().then_some(completed.as_str()),
                    cx,
                );
                this.checkout_panel
                    .update_operation(&path, |panel, current| {
                        panel.operation.busy = false;
                        panel.operation.message = result.as_ref().err().cloned();
                        if let Err(error) = &result {
                            panel
                                .operation
                                .failures
                                .insert(feedback_action, error.clone());
                        }
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

    pub(super) fn checkout_run(&mut self, path: String, action: GitAction, cx: &mut Context<Self>) {
        if self.checkout_panel.operation.busy || self.checkout_panel.path.as_ref() != Some(&path) {
            return;
        }
        let Some(Ok(expected)) = self.checkout_panel.state.clone() else {
            return;
        };
        let generation = self.checkout_panel.generation;
        let (feedback_action, working, completed) = match &action {
            GitAction::Push => ("local", "Pushing…", "Pushed".to_owned()),
            GitAction::Pull => (
                "pull",
                "Pulling…",
                format!(
                    "Pulled {} commit{}",
                    expected.behind.unwrap_or(0),
                    if expected.behind == Some(1) { "" } else { "s" }
                ),
            ),
            GitAction::Publish { .. } => ("local", "Publishing…", "Branch published".to_owned()),
            _ => ("local", "Updating checkout…", "Checkout updated".to_owned()),
        };
        let failure_action = match &action {
            GitAction::Pull => "pull",
            GitAction::Publish { .. } => "publish",
            _ => "push",
        };
        self.checkout_panel
            .operation
            .failures
            .remove(failure_action);
        let feedback_token = self.begin_panel_feedback(&path, feedback_action, working, cx);
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
                this.finish_panel_feedback(
                    &path,
                    feedback_action,
                    feedback_token,
                    result.is_ok().then_some(completed.as_str()),
                    cx,
                );
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
                        if let Err(error) = &result {
                            panel
                                .operation
                                .failures
                                .insert(failure_action, error.clone());
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

    pub(in crate::views::sidebar) fn checkout_commit_provider(
        &self,
        cx: &App,
    ) -> Result<grove_core::codex::Provider, String> {
        if self.dashboard_open {
            if let Some(dashboard) = &self.dashboard {
                return Ok(dashboard.session.read(cx).state.saved.provider);
            }
        }
        let selected = match self.selection {
            Some(Selection::Session(id) | Selection::Home(id)) => self
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

    pub(super) fn checkout_begin_commit(&mut self, path: String, cx: &mut Context<Self>) {
        if self.checkout_panel.operation.busy || self.checkout_panel.path.as_ref() != Some(&path) {
            return;
        }
        self.checkout_panel.operation.failures.remove("commit");
        let provider = match self.checkout_commit_provider(cx) {
            Ok(provider) => provider,
            Err(error) => {
                self.checkout_panel.operation.commit_status = CommitUi::Failed;
                self.checkout_panel
                    .operation
                    .failures
                    .insert("commit", error.clone());
                self.checkout_panel.operation.message = Some(error);
                cx.notify();
                return;
            }
        };
        self.checkout_panel.operation.commit_job =
            self.checkout_panel.operation.commit_job.wrapping_add(1);
        let job = self.checkout_panel.operation.commit_job;
        let feedback_token =
            self.begin_panel_feedback(&path, "local", "Generating message… · Cancel", cx);
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
                                let receipt = match &event {
                                    CommitEvent::Finished(finished) => Some(if finished.0.is_ok() { Some("Committed") } else {
                                        let canceled = if this.checkout_panel.path.as_deref() == Some(&path) {
                                            matches!(this.checkout_panel.operation.commit_status, CommitUi::Canceling)
                                        } else {
                                            this.checkout_panel.operations.get(&path).is_some_and(|op| matches!(op.commit_status, CommitUi::Canceling))
                                        };
                                        canceled.then_some("Commit canceled")
                                    }),
                                    CommitEvent::Progress(CommitPhase::Committing) => {
                                        this.checkout_panel.feedback.progress(&path, "local", feedback_token, "Committing…"); None
                                    }
                                    CommitEvent::Progress(_) => None,
                                };
                                succeeded |= this.checkout_panel.commit_event(&path, job, event);
                                if let Some(receipt) = receipt {
                                    this.finish_panel_feedback(&path, "local", feedback_token, receipt, cx);
                                }
                                cx.notify();
                            });
                            if updated.is_err() || finished { break 'updates; }
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            let _ = this.update(cx, |this, cx| {
                                this.finish_panel_feedback(&path, "local", feedback_token, None, cx);
                                this.checkout_panel.update_operation(&path, |panel, _| {
                                    if panel.operation.commit_job == job {
                                        panel.operation.busy = false;
                                        panel.operation.commit_control = None;
                                        panel.operation.commit_job_path = None;
                                        panel.operation.commit_status = CommitUi::Failed;
                                        panel.operation.failures.insert("commit", "Commit generation ended unexpectedly. Retry the commit.".into());
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

    pub(super) fn checkout_cancel_commit(&mut self, cx: &mut Context<Self>) {
        if let Some(control) = &self.checkout_panel.operation.commit_control {
            self.checkout_panel.operation.commit_status = if control.cancel() {
                CommitUi::Canceling
            } else {
                CommitUi::Committing
            };
            if let Some(path) = self.checkout_panel.path.clone() {
                if let Some(feedback) = self.checkout_panel.feedback.get(&path, "local") {
                    self.checkout_panel.feedback.progress(
                        &path,
                        "local",
                        feedback.token,
                        if matches!(
                            self.checkout_panel.operation.commit_status,
                            CommitUi::Canceling
                        ) {
                            "Canceling…"
                        } else {
                            "Committing…"
                        },
                    );
                }
            }
            cx.notify();
        }
    }

    pub(in crate::views::sidebar) fn checkout_cancel_confirmation(
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
}

pub(super) fn commit_provider(
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
