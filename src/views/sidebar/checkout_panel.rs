//! Checkout actions live beside the selected canvas, outside its scroll region.
use super::*;
use crate::views::components::{form_action, form_field, project_field_well, transcript_action};
use gpui_component::{
    menu::{PopupMenu, PopupMenuItem},
    Disableable,
};
use grove_core::checkout_git::{self, CheckoutGitState, GitAction};
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
    commit_open: bool,
    commit_generation: u64,
    commit_all: bool,
    commit_expected: Option<Result<CheckoutGitState, String>>,
    commit_message: Option<Entity<InputState>>,
    commit_observer: Option<gpui::Subscription>,
    restore_focus: bool,
}

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
        if let Some(target) = self.selected_worktree() {
            return vec![target];
        }
        if let Some(Selection::Project(idx)) = self.selection {
            if let Some(project) = cx.global::<SettingsState>().store.projects.get(idx) {
                return vec![(idx, project.path.clone())];
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
        let commit = matches!(
            action,
            GitAction::Commit { .. } | GitAction::CommitAll { .. }
        );
        let source = if commit {
            &self.checkout_panel.commit_expected
        } else {
            &self.checkout_panel.state
        };
        let Some(Ok(expected)) = source.clone() else {
            return;
        };
        let generation = self.checkout_panel.generation;
        self.checkout_panel.busy = true;
        self.checkout_panel.message = Some("Working…".into());
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
                if this.checkout_panel.accepts_result(&path, generation) {
                    this.checkout_panel.checked_at = Some(Instant::now());
                    this.checkout_panel.state = Some(state);
                    if result.is_ok() {
                        this.checkout_panel.commit_open = false;
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

    fn checkout_begin_commit(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.checkout_panel.busy || self.checkout_panel.path.as_ref() != Some(&path) {
            return;
        }
        let refreshing = self.checkout_panel.commit_open;
        self.checkout_panel.commit_generation =
            self.checkout_panel.commit_generation.wrapping_add(1);
        let request = self.checkout_panel.commit_generation;
        self.checkout_panel.return_focus = self.checkout_panel.trigger_focus.get("commit").cloned();
        self.checkout_panel.commit_open = true;
        self.checkout_panel.commit_expected = None;
        self.checkout_panel.message = None;
        let input = if refreshing {
            self.checkout_panel.commit_message.clone()
        } else {
            None
        }
        .unwrap_or_else(|| {
            cx.new(|cx| InputState::new(window, cx).placeholder("e.g. Fix checkout actions"))
        });
        self.checkout_panel.commit_observer = Some(cx.observe(&input, |_, _, cx| cx.notify()));
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.checkout_panel.commit_message = Some(input);
        let generation = self.checkout_panel.generation;
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let prepared = cx
                .background_executor()
                .spawn(async move { checkout_git::prepare_commit(Path::new(&target)) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.checkout_panel.accepts_result(&path, generation)
                    && this.checkout_panel.commit_open
                    && this.checkout_panel.commit_generation == request
                {
                    if let Ok(state) = &prepared {
                        this.checkout_panel.commit_all = state.staged_files.is_empty();
                    }
                    this.checkout_panel.commit_expected = Some(prepared);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn checkout_cancel_confirmation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.checkout_panel.busy {
            return;
        }
        self.checkout_panel.commit_generation =
            self.checkout_panel.commit_generation.wrapping_add(1);
        self.checkout_panel.commit_open = false;
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
        if self.checkout_panel.commit_open {
            form = form.child(div().text_size(rpx(TEXT_DISPLAY)).child("Commit changes"));
            if let Some(Ok(expected)) = self.checkout_panel.commit_expected.clone() {
                let all = self.checkout_panel.commit_all;
                let files = if all {
                    expected.changed_files.clone()
                } else {
                    expected.staged_files.clone()
                };
                form = form.child(div().text_color(c::FG_DIM()).child(if all {
                    "All listed tracked and untracked changes will be staged and committed. Ignored files stay excluded."
                } else { "Only the listed staged changes will be committed. Unstaged changes stay outside this commit." }))
                    .child(div().text_size(rpx(TEXT_SMALL)).child(format!("{} · {}", expected.branch.clone().unwrap_or_else(|| "Detached HEAD".into()), path)));
                let mut scope = project_field_well(false, false)
                    .id("checkout-commit-scope")
                    .h_auto()
                    .justify_start()
                    .py(rpx(SPACE_LG))
                    .max_h(rpx(MODAL_SCROLL_MAX_H))
                    .overflow_y_scroll();
                for file in &files {
                    scope = scope.child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .min_w_0()
                            .truncate()
                            .child(file.clone()),
                    );
                }
                form = form.child(scope).child(
                    form_action(
                        "checkout-review-commit",
                        "Review changes",
                        false,
                        window,
                        cx,
                    )
                    .on_click(cx.listener({
                        let path = path.clone();
                        move |this, _, window, cx| {
                            this.act(Action::OpenWorktreeDiff(path.clone()), window, cx);
                        }
                    })),
                );
                if let Some(input) = &self.checkout_panel.commit_message {
                    let message = input.read(cx).value().trim().to_string();
                    form = form.child(form_field(
                        "checkout-commit-message",
                        "Commit message",
                        input,
                        None,
                        window,
                        cx,
                    ));
                    if !expected.staged_files.is_empty() && !all {
                        form = form.child(
                            form_action(
                                "checkout-commit-all-scope",
                                "Include all changes instead",
                                false,
                                window,
                                cx,
                            )
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.checkout_panel.commit_all = true;
                                cx.notify();
                            })),
                        );
                    }
                    let target = path.clone();
                    let label = if all {
                        "Stage all and commit"
                    } else {
                        "Commit staged changes"
                    };
                    form = form.child(
                        form_action("checkout-confirm-commit", label, true, window, cx)
                            .disabled(busy || message.is_empty() || files.is_empty())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let action = if all {
                                    GitAction::CommitAll {
                                        message: message.clone(),
                                    }
                                } else {
                                    GitAction::Commit {
                                        message: message.clone(),
                                    }
                                };
                                this.checkout_run(target.clone(), action, cx);
                            })),
                    );
                }
            } else {
                let text = match &self.checkout_panel.commit_expected {
                    Some(Err(error)) => error.as_str(),
                    _ => "Preparing the exact commit scope…",
                };
                form = form.child(div().text_color(c::FG_DIM()).child(text.to_string()));
            }
        } else if let Some(remote) = self.checkout_panel.publish.clone() {
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
        if self.checkout_panel.commit_open && !busy && self.checkout_panel.message.is_some() {
            let target = path.clone();
            form = form.child(
                form_action(
                    "checkout-refresh-commit-scope",
                    "Review current scope",
                    false,
                    window,
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.checkout_begin_commit(target.clone(), window, cx);
                })),
            );
        }
        if let Some(message) = &self.checkout_panel.message {
            form = form.child(
                div()
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
        for kind in ["branch", "publish", "summary", "commit"] {
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
            self.checkout_panel.commit_open = false;
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
            .py(rpx(SPACE_LG))
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
                self.checkout_panel.commit_open = false;
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
            if self.checkout_panel.commit_open || self.checkout_panel.publish.is_some() {
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
                        .border_t_1()
                        .border_color(c::BORDER_SOFT())
                        .my(rpx(SPACE_SM)),
                );
            let state = self.checkout_panel.state.clone().and_then(Result::ok);
            let branch = state.as_ref().map_or(
                if self.checkout_panel.loading {
                    "Reading branch…".into()
                } else {
                    "Git unavailable".into()
                },
                |state| {
                    state
                        .branch
                        .clone()
                        .unwrap_or_else(|| "Detached HEAD".into())
                },
            );
            let target = path.clone();
            card = card.child(
                checkout_row("checkout-branch", branch, "git-branch", window, cx)
                    .with_focus_handle(self.checkout_panel.trigger_focus["branch"].clone())
                    .disabled(state.is_none() && targets.len() == 1)
                    .child(icon("chev-down", ICON_SM, c::FG_DIM()))
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            this.checkout_menu(
                                false,
                                idx,
                                target.clone(),
                                event.position(),
                                window,
                                cx,
                            );
                        }),
                    ),
            );
            let target = path.clone();
            let busy = self.checkout_panel.busy;
            let (label, glyph, enabled) = commit_row_state(state.as_ref());
            let publish = state.as_ref().is_some_and(|state| {
                !state.dirty
                    && state.upstream.is_none()
                    && state.branch.is_some()
                    && state.head.is_some()
                    && !state.remotes.is_empty()
            });
            let dirty = state.as_ref().is_some_and(|state| state.dirty);
            card = card.child(
                checkout_row("checkout-commit-publish", label, glyph, window, cx)
                    .disabled(busy || !enabled)
                    .with_focus_handle(
                        self.checkout_panel.trigger_focus
                            [if publish { "publish" } else { "commit" }]
                        .clone(),
                    )
                    .tooltip(self.checkout_panel.message.clone().unwrap_or_else(|| {
                        self.checkout_panel
                            .state
                            .as_ref()
                            .and_then(|state| state.as_ref().err())
                            .cloned()
                            .unwrap_or_else(|| {
                                "Commit or publish changes from this checkout".into()
                            })
                    }))
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            if dirty {
                                this.checkout_begin_commit(target.clone(), window, cx);
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
                        }),
                    ),
            );
            let git = self.runtime.read(cx).tree.read(cx).git_states();
            let changes = git.get(crate::paths::normalize_wt_path(&path));
            let mut changes_row =
                checkout_row("checkout-changes", "Changes", "unified", window, cx)
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
            card = card
                .child(
                    checkout_row("checkout-finder", "Open in Finder", "folder", window, cx)
                        .disabled(true),
                )
                .child(
                    checkout_row("checkout-script", "Add project script", "plus", window, cx)
                        .disabled(true),
                );
            let target = targets.first().cloned();
            card = card
                .child(
                    checkout_row(
                        "checkout-branch",
                        "Choose checkout",
                        "git-branch",
                        window,
                        cx,
                    )
                    .with_focus_handle(self.checkout_panel.trigger_focus["branch"].clone())
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
                    .with_focus_handle(self.checkout_panel.trigger_focus["summary"].clone())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.checkout_panel.expanded = !this.checkout_panel.expanded;
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
            .get(if publish { "publish" } else { "branch" })
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

fn checkout_row(
    id: &'static str,
    label: impl Into<SharedString>,
    glyph: &'static str,
    window: &Window,
    cx: &App,
) -> gpui_component::button::Button {
    let label = label.into();
    checkout_action(id, label.clone(), false, window, cx)
        .debug_selector(move || id.into())
        .icon_only()
        .w_full()
        .h(rpx(ROW_H))
        .gap(rpx(SPACE_LG))
        .child(
            div()
                .id(SharedString::from(format!("{id}-icon")))
                .debug_selector(move || format!("{id}-icon"))
                .w(rpx(ICON_LG))
                .flex_shrink_0()
                .child(icon(glyph, ICON_MD, c::FG_DIM())),
        )
        .child(div().flex_1().min_w_0().truncate().child(label))
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
            ("No remote configured", "arrow-up", false)
        } else {
            ("Publish branch", "arrow-up", true)
        };
    }
    if state.ahead.is_some_and(|ahead| ahead > 0) {
        ("Push commits", "arrow-up", true)
    } else {
        ("No local commits", "git", false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn five_icon_rows_clear_appbar_and_narrow_disclosure_keeps_header(
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
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.selection = Some(Selection::Project(0));
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
        let rows = [
            "checkout-finder",
            "checkout-script",
            "checkout-branch",
            "checkout-commit-publish",
            "checkout-changes",
        ];
        let mut icon_x = None;
        for id in rows {
            let row = cx.debug_bounds(id).unwrap();
            let glyph = cx.debug_bounds(&format!("{id}-icon")).unwrap();
            assert!(row.top() >= card.top() && row.bottom() <= card.bottom());
            assert_eq!(row.size.height, gpui::px(ROW_H));
            if let Some(x) = icon_x {
                assert_eq!(glyph.left(), x);
            } else {
                icon_x = Some(glyph.left());
            }
        }
        assert!(cx.debug_bounds("checkout-path").is_none());
        cx.simulate_resize(gpui::size(gpui::px(500.), gpui::px(600.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let header = cx.debug_bounds("canvas-section-header").unwrap();
        let panel = cx.debug_bounds("checkout-panel").unwrap();
        assert_eq!(header.top(), gpui::px(0.));
        assert!(panel.top() >= header.bottom());
        assert!(cx.debug_bounds("checkout-card").is_none());
    }
}
