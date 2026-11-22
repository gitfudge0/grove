//! Checkout Git action menus, confirmation cards, and failure messages.

use super::*;

/// A handoff populates a fresh session's composer and never submits a turn.
pub(super) fn prepare_resolution_draft(
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
    /// A failure belongs to its checkout operation, never to the current generic message.
    /// Static replacement is intentional: error/working feedback follows the operation immediately.
    pub(super) fn checkout_failure(&self, action: &'static str, cx: &App) -> AnyElement {
        if self.checkout_confirmation_open() {
            return div().into_any_element();
        }
        self.checkout_failure_content(action, cx)
    }

    fn checkout_failure_content(&self, action: &'static str, cx: &App) -> AnyElement {
        let Some(message) = self.checkout_panel.operation.failures.get(action) else {
            return div().into_any_element();
        };
        let (summary, guidance) = checkout_failure_copy(action, message);
        let id = format!("checkout-{action}-failure");
        let selector = id.clone();
        div()
            .id(id.clone())
            .debug_selector(move || selector.clone())
            .role(gpui::Role::Status)
            .pl(rpx(SPACE_2XL))
            .pr(rpx(SPACE_2XL))
            .py(rpx(SPACE_SM))
            .min_w_0()
            .text_size(rpx(TEXT_SMALL))
            .whitespace_normal()
            .child(crate::views::status::label(
                id,
                crate::views::status::StatusKind::Failed,
                summary,
                ICON_LG,
                cx,
            ))
            .when_some(guidance, |el, guidance| {
                el.child(
                    div()
                        .pl(rpx(ICON_LG + SPACE_SM))
                        .text_color(c::FG_DIM())
                        .child(guidance),
                )
            })
            .into_any_element()
    }

    pub(in crate::views::sidebar) fn render_checkout_confirmation(
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
                CheckoutRecovery::Main(..) => {}
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
                        this.checkout_execute_recovery(target.clone(), recovery.clone(), cx);
                    })),
            );
        }
        for channel in ["local", "recovery", "main", "pr-merge"] {
            if let Some(feedback) = self
                .checkout_panel
                .feedback
                .get(&path, channel)
                .filter(|f| f.completed.is_none())
            {
                form = form.child(
                    checkout_cycle_row(
                        "checkout-dialog-feedback",
                        "",
                        "git",
                        false,
                        Some(feedback),
                        window,
                        cx,
                    )
                    .disabled(true),
                );
                break;
            }
        }
        let failure_action = if self.checkout_panel.publish.is_some() {
            "publish"
        } else if matches!(
            self.checkout_panel.recovery,
            Some(CheckoutRecovery::Main(..))
        ) {
            "main"
        } else {
            "recovery"
        };
        // PR merge owns pr_message; an older branch-recovery failure must not
        // appear inside this unrelated confirmation.
        if self.checkout_panel.pr_merge.is_none() {
            form = form.child(self.checkout_failure_content(failure_action, cx));
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

    pub(super) fn checkout_menu(
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
            .map_or_else(|| path.clone(), |project| project.path.clone());
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

/// Separate a concise cause from genuinely different recovery guidance.
pub(super) fn checkout_failure_copy(action: &str, message: &str) -> (String, Option<String>) {
    let operation = match action {
        "pull" => "pull",
        "push" => "push",
        "publish" => "publish branch",
        "commit" => "commit",
        "main" => "merge main branch",
        _ => "complete branch recovery",
    };
    let (cause, guidance) = message
        .split_once(". ")
        .map_or((message, None), |(cause, rest)| {
            (cause, (!rest.is_empty()).then(|| rest.to_owned()))
        });
    (
        format!("Couldn’t {operation}: {}", cause.trim_end_matches('.')),
        guidance,
    )
}
