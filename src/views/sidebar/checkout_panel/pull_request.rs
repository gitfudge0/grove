//! Checkout pull request refresh, display and actions.

use super::{
    c, checkout_cycle_row, checkout_divider, checkout_gate, checkout_pr, checkout_row, icon,
    pr_compact_summary, pr_status_kind, prepare_resolution_draft, rpx, Agent, CheckoutGitState,
    Disableable, Instant, Path, PullRequest, Selection, SettingsState, Sidebar,
    CHECKOUT_LABEL_INSET,
};
use crate::views::tokens::{CONTROL_H, ICON_SM, SPACE_2XL, SPACE_SM, TEXT_SMALL};
use gpui::prelude::{
    InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
};
use gpui::{div, AnyElement, Context, Window};

impl Sidebar {
    pub(super) fn checkout_load_pr(
        &mut self,
        path: String,
        state: CheckoutGitState,
        cx: &mut Context<Self>,
    ) {
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

    fn checkout_compose_pr(&mut self, path: String, cx: &mut Context<Self>) {
        if self.checkout_panel.pr_composing || self.checkout_panel.operation.busy {
            return;
        }
        let Some(Ok(state)) = self.checkout_panel.state.clone() else {
            return;
        };
        let provider = match self.checkout_commit_provider(cx) {
            Ok(provider) => provider,
            Err(error) => {
                self.checkout_panel.operation.pr_message =
                    Some(error.replace("generate commits", "generate PR text"));
                cx.notify();
                return;
            }
        };
        let token = self.begin_panel_feedback(&path, "pr-create", "Preparing PR form…", cx);
        let generation = self.checkout_panel.generation;
        let identity = state.target_identity();
        self.checkout_panel.pr_composing = true;
        self.checkout_panel.operation.pr_message = None;
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    checkout_pr::compose_url_with_provider(Path::new(&target), &state, provider)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this.checkout_panel.accepts_result(&path, generation) {
                    this.finish_panel_feedback(&path, "pr-create", token, None, cx);
                    return;
                }
                if this.checkout_panel.accepts_result(&path, generation) {
                    this.checkout_panel.pr_composing = false;
                    if this
                        .checkout_panel
                        .state
                        .as_ref()
                        .and_then(|s| s.as_ref().ok())
                        .is_none_or(|s| s.target_identity() != identity)
                    {
                        this.finish_panel_feedback(&path, "pr-create", token, None, cx);
                        cx.notify();
                        return;
                    }
                    match result {
                        Ok(url) => {
                            cx.open_url(&url);
                            this.finish_panel_feedback(
                                &path,
                                "pr-create",
                                token,
                                Some("PR form opened"),
                                cx,
                            );
                        }
                        Err(error) => {
                            this.finish_panel_feedback(&path, "pr-create", token, None, cx);
                            this.checkout_panel.operation.pr_message = Some(error);
                        }
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Typed scripts retain their identity and a fixed, independently focusable Run slot.
    pub(super) fn render_checkout_pr(
        &mut self,
        path: &str,
        state: Option<&CheckoutGitState>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mut section = div().flex().flex_col().min_w_0();
        let mut visible = false;
        let fresh = self.checkout_panel.pr_error.is_none()
            && self.checkout_panel.state_error.is_none()
            && self.checkout_panel.pr_checked_at.is_some();
        match self.checkout_panel.pr_state.clone() {
            Some(Ok(Some(pr))) => {
                let url = pr.url.clone();
                visible = true;
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
                            checkout_cycle_row(
                                "checkout-pr-resolve",
                                "Resolve conflicts",
                                "git-branch",
                                false,
                                self.checkout_panel.feedback.get(path, "pr-resolve"),
                                window,
                                cx,
                            )
                            .w_auto()
                            .h(rpx(CONTROL_H))
                            .debug_selector(|| "checkout-pr-resolve".into())
                            .disabled(
                                !fresh
                                    || self.checkout_panel.operation.busy
                                    || self
                                        .checkout_panel
                                        .feedback
                                        .get(path, "pr-resolve")
                                        .is_some(),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.checkout_resolve_pr(target.clone(), request.clone(), cx);
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
                                checkout_cycle_row(
                                    "checkout-pr-merge",
                                    "Merge",
                                    "check",
                                    false,
                                    self.checkout_panel.feedback.get(path, "pr-merge"),
                                    window,
                                    cx,
                                )
                                .w_auto()
                                .h(rpx(CONTROL_H))
                                .debug_selector(|| "checkout-pr-merge".into())
                                .with_focus_handle(
                                    self.checkout_panel.trigger_focus["pr-merge"].clone(),
                                )
                                .disabled(
                                    self.checkout_panel.operation.busy
                                        || self
                                            .checkout_panel
                                            .feedback
                                            .get(path, "pr-merge")
                                            .is_some(),
                                )
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
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
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
                                    },
                                )),
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
                                checkout_cycle_row(
                                    "checkout-pr-fix-ci",
                                    "Fix failed CI",
                                    "play",
                                    false,
                                    self.checkout_panel.feedback.get(path, "pr-ci"),
                                    window,
                                    cx,
                                )
                                .w_auto()
                                .h(rpx(CONTROL_H))
                                .disabled(
                                    !fresh
                                        || self.checkout_panel.pr_prompt_busy
                                        || self
                                            .checkout_panel
                                            .feedback
                                            .get(path, "pr-ci")
                                            .is_some(),
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.checkout_pr_prompt(
                                            target.clone(),
                                            request.clone(),
                                            cx,
                                        );
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
                    visible = true;
                    section = section.child(
                        checkout_cycle_row(
                            "checkout-pr",
                            "Create PR…",
                            "git-pull-request",
                            false,
                            self.checkout_panel.feedback.get(path, "pr-create"),
                            window,
                            cx,
                        )
                        .disabled(
                            !fresh
                                || self.checkout_panel.operation.busy
                                || self.checkout_panel.pr_composing
                                || self
                                    .checkout_panel
                                    .feedback
                                    .get(path, "pr-create")
                                    .is_some(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.checkout_compose_pr(target.clone(), cx);
                        })),
                    );
                }
            }
            Some(Err(_)) | None => {}
        }
        if let Some(error) = &self.checkout_panel.pr_error {
            visible = true;
            section = section.child(
                checkout_row(
                    "checkout-pr-refresh",
                    if self
                        .checkout_panel
                        .pr_state
                        .as_ref()
                        .is_some_and(std::result::Result::is_ok)
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
            visible = true;
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
        if let Some(feedback) = self
            .checkout_panel
            .feedback
            .get(path, "pr-merge")
            .filter(|_| {
                self.checkout_panel
                    .pr_state
                    .as_ref()
                    .and_then(|r| r.as_ref().ok())
                    .and_then(|r| r.as_ref())
                    .is_none_or(|pr| pr.state != "OPEN")
            })
        {
            visible = true;
            section = section.child(
                checkout_cycle_row(
                    "checkout-pr-merge-feedback",
                    "",
                    "git-pull-request",
                    false,
                    Some(feedback),
                    window,
                    cx,
                )
                .disabled(true),
            );
        }
        if let Some(message) = self
            .checkout_panel
            .operation
            .pr_message
            .as_ref()
            .filter(|message| !message.is_empty())
        {
            visible = true;
            section = section.child(
                div()
                    .pl(rpx(CHECKOUT_LABEL_INSET))
                    .pr(rpx(SPACE_2XL))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(message.clone()),
            );
        }
        visible |= self.has_change_tools(path, false, cx);
        if visible {
            Some(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(checkout_divider("checkout-pr-divider"))
                    .child(self.render_change_tools_heading(path, false, window, cx))
                    .child(section)
                    .into_any_element(),
            )
        } else {
            None
        }
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
        let token = self.begin_panel_feedback(&path, "pr-ci", "Preparing CI context…", cx);
        let thread = session.read(cx).state.saved.thread_id.clone();
        let generation = self.checkout_panel.generation;
        self.checkout_panel.pr_prompt_busy = true;
        self.checkout_panel.operation.pr_message = Some("Refreshing pull request context…".into());
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let result = cx.background_executor().spawn(async move { checkout_pr::action_prompt(Path::new(&target), &state, &pr) }).await;
            let _ = this.update(cx, |this, cx| {
                if !this.checkout_panel.accepts_result(&path, generation) {
                    this.finish_panel_feedback(&path, "pr-ci", token, None, cx); return;
                }
                this.checkout_panel.pr_prompt_busy = false;
                let result = if this.selection != Some(Selection::Session(id)) || session.read(cx).state.saved.cwd != path || session.read(cx).state.saved.thread_id != thread {
                    Err("Current chat changed while preparing the PR action. Retry in the intended chat".into())
                } else { result.and_then(|prompt| session.update(cx, |session,cx| session.project_prompt(prompt,cx)).map(|_| ())) };
                this.finish_panel_feedback(&path, "pr-ci", token, result.is_ok().then_some("CI request queued"), cx);
                this.checkout_panel.operation.pr_message = Some(match result { Ok(()) => String::new(), Err(error) => error });
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

        let token =
            self.begin_panel_feedback(&path, "pr-resolve", "Opening resolution session…", cx);
        let mut opened = false;
        if self.launch(idx, path.clone(), agent, cx) {
            if let Some(Selection::Session(id)) = self.selection {
                let session = self.runtime.read(cx).registry.read(cx).codex(id).cloned();
                if let Some(session) = session {
                    match prepare_resolution_draft(&session, &path, provider, &pr, cx) {
                        Ok(()) => opened = true,
                        Err(error) => self.show_launch_error(error, cx),
                    }
                }
            }
        }
        self.finish_panel_feedback(
            &path,
            "pr-resolve",
            token,
            opened.then_some("Resolution session opened"),
            cx,
        );
    }

    pub(super) fn checkout_merge_pr(
        &mut self,
        path: String,
        pr: PullRequest,
        cx: &mut Context<Self>,
    ) {
        if self.checkout_panel.operation.busy || self.checkout_panel.path.as_ref() != Some(&path) {
            return;
        }
        let Some(Ok(expected)) = self.checkout_panel.state.clone() else {
            return;
        };
        let feedback_token =
            self.begin_panel_feedback(&path, "pr-merge", "Merging pull request…", cx);
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
                this.finish_panel_feedback(
                    &path,
                    "pr-merge",
                    feedback_token,
                    result.is_ok().then_some("Pull request merged"),
                    cx,
                );
                this.checkout_panel
                    .update_operation(&path, |panel, current| {
                        panel.operation.busy = false;
                        panel.operation.message = None;
                        panel.operation.pr_message =
                            Some(result.map_or_else(|error| error, |()| String::new()));
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
}
