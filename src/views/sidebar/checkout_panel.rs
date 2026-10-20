//! Checkout actions live beside the selected canvas, outside its scroll region.
use super::*;
use crate::views::components::{project_field_well, transcript_action};
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
    bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    generation: u64,
    checked_at: Option<Instant>,
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

fn session_blocks_pull(native_attached: bool, terminal_exited: Option<bool>) -> bool {
    native_attached || terminal_exited != Some(true)
}

fn registry_blocks_pull(
    registry: &crate::entities::session_registry::SessionRegistry,
    path: &str,
    terminal_exited: impl Fn(SessionId) -> Option<bool>,
) -> bool {
    let target = crate::paths::normalize_wt_path(path);
    registry.all().iter().any(|meta| {
        let uses_target = crate::paths::normalize_wt_path(&meta.wt_path) == target
            || meta
                .context_roots
                .iter()
                .any(|root| crate::paths::normalize_wt_path(&root.wt_path) == target);
        uses_target
            && session_blocks_pull(registry.codex(meta.id).is_some(), terminal_exited(meta.id))
    })
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
        let Some(Ok(expected)) = self.checkout_panel.state.clone() else {
            return;
        };
        if matches!(action, GitAction::Pull) && self.checkout_has_live_session(&path, cx) {
            self.checkout_panel.message =
                Some("Stop sessions in this checkout before pulling.".into());
            cx.notify();
            return;
        }
        let generation = self.checkout_panel.generation;
        self.checkout_panel.busy = true;
        self.checkout_panel.publish = None;
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

    fn checkout_has_live_session(&self, path: &str, cx: &App) -> bool {
        let registry = self.runtime.read(cx).registry.read(cx);
        registry_blocks_pull(registry, path, |id| {
            registry
                .session(id)
                .map(|session| session.read(cx).has_exited())
        })
    }

    pub(super) fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.checkout_panel.narrow_content = None;
        for kind in ["folder", "branch", "git", "publish", "summary"] {
            self.checkout_panel
                .trigger_focus
                .entry(kind)
                .or_insert_with(|| cx.focus_handle());
        }
        let targets = self.checkout_targets(cx);
        if self.checkout_panel.root_owner != self.selection {
            self.checkout_panel.root_owner = self.selection.clone();
            self.checkout_panel.chosen_root = None;
            self.checkout_panel.path = None;
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
            return self.render_content_body(window, cx);
        }
        let wide = Self::logical_window_width(window) >= WIDE_VIEWPORT;
        let selected = selected_checkout(&targets, self.checkout_panel.chosen_root.as_deref());
        let multiple_roots = targets.len() > 1;
        let mut card = project_field_well(false, false)
            .h_auto()
            .justify_start()
            .w_full()
            .py(rpx(SPACE_2XL))
            .gap(rpx(SPACE_LG));
        if let Some((idx, path)) = selected {
            if self.checkout_panel.path.as_ref() != Some(&path) {
                self.checkout_panel.generation = self.checkout_panel.generation.wrapping_add(1);
                self.checkout_panel.path = Some(path.clone());
                self.checkout_panel.checked_at = None;
                self.checkout_panel.state = None;
                self.checkout_panel.loading = false;
                self.checkout_panel.message = None;
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
            let worktree = self
                .snapshot
                .projects
                .iter()
                .flat_map(|project| &project.worktrees)
                .find(|wt| wt.path == path);
            let name = worktree.map_or("Main checkout".to_string(), |wt| {
                sidebar_worktree_name(&wt.name, wt.is_main).to_string()
            });
            card = card.child(div().text_size(rpx(TEXT_BODY)).child(project.name.clone()));
            if wide {
                card = card.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(name),
                );
            } else {
                card = card.child(
                    checkout_action(
                        "checkout-summary",
                        format!(
                            "{name}  {}",
                            if self.checkout_panel.expanded {
                                "⌃"
                            } else {
                                "⌄"
                            }
                        ),
                        false,
                        window,
                        cx,
                    )
                    .with_focus_handle(self.checkout_panel.trigger_focus["summary"].clone())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.checkout_panel.return_focus =
                            this.checkout_panel.trigger_focus.get("summary").cloned();
                        this.checkout_panel.expanded = !this.checkout_panel.expanded;
                        cx.notify();
                    })),
                );
            }
            if multiple_roots {
                card = card.child(
                    checkout_action(
                        "checkout-choose-root",
                        "Choose checkout…",
                        false,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.checkout_panel.chosen_root = None;
                        this.checkout_panel.path = None;
                        this.checkout_panel.publish = None;
                        this.checkout_panel.generation =
                            this.checkout_panel.generation.wrapping_add(1);
                        cx.notify();
                    })),
                );
            }
            if wide || self.checkout_panel.expanded {
                let tooltip_path = path.clone();
                card = card.child(
                    div()
                        .id("checkout-path")
                        .debug_selector(|| "checkout-path".into())
                        .role(gpui::Role::Group)
                        .aria_label(format!("Checkout path: {path}"))
                        .min_w_0()
                        .w_full()
                        .text_size(rpx(TEXT_MICRO))
                        .text_color(c::FG_DIM())
                        .truncate()
                        .tooltip(move |window, cx| {
                            crate::views::components::tooltip(tooltip_path.clone(), window)
                                .build(window, cx)
                        })
                        .child(path.clone()),
                );
                let menu_path = path.clone();
                card = card
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_SM))
                            .child(
                                self.control(
                                    "checkout-finder",
                                    "Open checkout in Finder",
                                    Action::Reveal(path.clone()),
                                    cx,
                                )
                                .flex_1()
                                .justify_start()
                                .child("Open in Finder"),
                            )
                            .child(
                                checkout_action(
                                    "checkout-folder-menu",
                                    "Checkout folder actions",
                                    false,
                                    window,
                                    cx,
                                )
                                .icon_only()
                                .child(icon("chev-down", ICON_SM, c::FG_DIM()))
                                .tooltip("Checkout folder actions")
                                .with_focus_handle(
                                    self.checkout_panel.trigger_focus["folder"].clone(),
                                )
                                .on_click(cx.listener(
                                    move |this, event: &gpui::ClickEvent, window, cx| {
                                        this.checkout_menu(
                                            "folder",
                                            idx,
                                            menu_path.clone(),
                                            event.position(),
                                            window,
                                            cx,
                                        );
                                    },
                                )),
                            ),
                    )
                    .child(
                        self.control(
                            "checkout-script",
                            "Run or configure project script",
                            if project
                                .scripts
                                .run
                                .as_deref()
                                .is_some_and(|script| !script.trim().is_empty())
                            {
                                Action::RunScript(project.path.clone(), path.clone())
                            } else {
                                Action::EditProject(project.path.clone())
                            },
                            cx,
                        )
                        .w_full()
                        .justify_start()
                        .child(
                            if project
                                .scripts
                                .run
                                .as_deref()
                                .is_some_and(|script| !script.trim().is_empty())
                            {
                                "Run project script"
                            } else {
                                "Add project script…"
                            },
                        ),
                    )
                    .child(div().h(rpx(SPACE_XS)).bg(c::BORDER_SOFT()));
                let state = self.checkout_panel.state.clone();
                if let Some(Ok(state)) = state {
                    let menu_path = path.clone();
                    card = card.child(
                        checkout_action(
                            "checkout-branch",
                            format!(
                                "{}  ⌄",
                                state
                                    .branch
                                    .clone()
                                    .unwrap_or_else(|| "Detached HEAD".into())
                            ),
                            false,
                            window,
                            cx,
                        )
                        .with_focus_handle(self.checkout_panel.trigger_focus["branch"].clone())
                        .on_click(cx.listener(
                            move |this, event: &gpui::ClickEvent, window, cx| {
                                this.checkout_menu(
                                    "branch",
                                    idx,
                                    menu_path.clone(),
                                    event.position(),
                                    window,
                                    cx,
                                );
                            },
                        )),
                    );
                    let tracking = match (&state.upstream, state.ahead, state.behind) {
                        (Some(upstream), Some(ahead), Some(behind)) => {
                            format!("{upstream} · {ahead} ahead · {behind} behind")
                        }
                        (Some(upstream), _, _) => {
                            format!("{upstream} · tracking status unavailable")
                        }
                        _ if state.head.is_none() => "No commits yet".into(),
                        _ if state.remotes.is_empty() => "No remote configured".into(),
                        _ => "No upstream branch".into(),
                    };
                    card = card.child(
                        div()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child(tracking),
                    );
                    let blocked = self.checkout_panel.busy || self.checkout_panel.loading;
                    let primary = if state.upstream.is_none() {
                        None
                    } else if state.behind.is_some_and(|count| count > 0) {
                        Some((
                            "Pull",
                            GitAction::Pull,
                            state.dirty || self.checkout_has_live_session(&path, cx),
                        ))
                    } else if state.ahead.is_some_and(|count| count > 0) {
                        Some(("Push", GitAction::Push, false))
                    } else {
                        Some(("Fetch", GitAction::Fetch, state.remotes.is_empty()))
                    };
                    if let Some((label, action, disabled)) = primary {
                        let target = path.clone();
                        let menu_path = path.clone();
                        card = card.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .child(
                                    checkout_action(
                                        "checkout-git-primary",
                                        label,
                                        false,
                                        window,
                                        cx,
                                    )
                                    .disabled(blocked || disabled)
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.checkout_run(target.clone(), action.clone(), cx);
                                        },
                                    )),
                                )
                                .child(
                                    checkout_action(
                                        "checkout-git-menu",
                                        "Checkout Git actions",
                                        false,
                                        window,
                                        cx,
                                    )
                                    .icon_only()
                                    .child(icon("chev-down", ICON_SM, c::FG_DIM()))
                                    .tooltip("Checkout Git actions")
                                    .with_focus_handle(
                                        self.checkout_panel.trigger_focus["git"].clone(),
                                    )
                                    .disabled(blocked)
                                    .on_click(cx.listener(
                                        move |this, event: &gpui::ClickEvent, window, cx| {
                                            this.checkout_menu(
                                                "git",
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
                    }
                    if state.upstream.is_none() {
                        let target = path.clone();
                        card = card.child(
                            checkout_action(
                                "checkout-untracked-git-menu",
                                "Git actions  ⌄",
                                false,
                                window,
                                cx,
                            )
                            .with_focus_handle(self.checkout_panel.trigger_focus["git"].clone())
                            .disabled(blocked)
                            .on_click(cx.listener(
                                move |this, event: &gpui::ClickEvent, window, cx| {
                                    this.checkout_menu(
                                        "git",
                                        idx,
                                        target.clone(),
                                        event.position(),
                                        window,
                                        cx,
                                    );
                                },
                            )),
                        );
                    }
                    if state.dirty || self.checkout_has_live_session(&path, cx) {
                        card = card.child(
                            div()
                                .text_size(rpx(TEXT_MICRO))
                                .text_color(c::FG_DIM())
                                .child("Pull requires a clean checkout with stopped sessions."),
                        );
                    }
                    if state.upstream.is_none()
                        && state.head.is_some()
                        && state.branch.is_some()
                        && !state.remotes.is_empty()
                    {
                        let target = path.clone();
                        card = card.child(
                            checkout_action(
                                "checkout-publish-menu",
                                "Publish branch…  ⌄",
                                false,
                                window,
                                cx,
                            )
                            .with_focus_handle(self.checkout_panel.trigger_focus["publish"].clone())
                            .disabled(blocked)
                            .on_click(cx.listener(
                                move |this, event: &gpui::ClickEvent, window, cx| {
                                    this.checkout_menu(
                                        "publish",
                                        idx,
                                        target.clone(),
                                        event.position(),
                                        window,
                                        cx,
                                    );
                                },
                            )),
                        );
                    }
                    if let Some(remote) = self.checkout_panel.publish.clone() {
                        let target = path.clone();
                        card = card
                            .child(div().text_size(rpx(TEXT_SMALL)).child(format!(
                                "Publish {} to {remote}?",
                                state.branch.unwrap_or_default()
                            )))
                            .child(
                                checkout_action(
                                    "checkout-confirm-publish",
                                    "Publish branch",
                                    false,
                                    window,
                                    cx,
                                )
                                .disabled(blocked)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.checkout_run(
                                            target.clone(),
                                            GitAction::Publish {
                                                remote: remote.clone(),
                                            },
                                            cx,
                                        );
                                    },
                                )),
                            )
                            .child(
                                checkout_action(
                                    "checkout-cancel-publish",
                                    "Cancel",
                                    false,
                                    window,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        this.checkout_panel.publish = None;
                                        if let Some(focus) = this.checkout_panel.return_focus.take()
                                        {
                                            focus.focus(window, cx);
                                        }
                                        cx.notify();
                                    },
                                )),
                            );
                    }
                    let git = self.runtime.read(cx).tree.read(cx).git_states();
                    let changes = git.get(crate::paths::normalize_wt_path(&path));
                    let dirty = state.dirty
                        || changes.is_some_and(|changes| {
                            changes.dirty || changes.added > 0 || changes.removed > 0
                        });
                    let mut row = self
                        .control(
                            "checkout-changes",
                            "Review checkout changes",
                            Action::OpenWorktreeDiff(path.clone()),
                            cx,
                        )
                        .w_full()
                        .justify_start()
                        .gap(rpx(SPACE_SM))
                        .child(
                            div()
                                .flex_1()
                                .child(if dirty { "Changes" } else { "No changes" }),
                        );
                    if let Some(changes) = changes.filter(|_| dirty) {
                        row = row
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
                    card = card.child(row);
                } else {
                    card = card.child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(if self.checkout_panel.loading {
                                "Reading Git status…"
                            } else {
                                "Git status unavailable. Check this folder and retry."
                            }),
                    );
                }
                let target = path;
                if !matches!(self.checkout_panel.state, Some(Ok(_))) {
                    card = card.child(
                        checkout_action("checkout-refresh", "Refresh status", false, window, cx)
                            .disabled(self.checkout_panel.loading || self.checkout_panel.busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.checkout_load(target.clone(), cx);
                            })),
                    );
                }
                if let Some(message) = &self.checkout_panel.message {
                    card = card.child(
                        div()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child(message.clone()),
                    );
                }
            }
        } else {
            card = card.child("Choose a checkout");
            for (_, path) in targets {
                let label = path.clone();
                card = card.child(
                    checkout_action(
                        SharedString::from(format!("checkout-root-{path}")),
                        label,
                        false,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.checkout_panel.chosen_root = Some(path.clone());
                        cx.notify();
                    })),
                );
            }
        }
        let mut panel = div()
            .id("checkout-panel")
            .debug_selector(|| "checkout-panel".into())
            .relative()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape"
                    && (this.checkout_panel.publish.is_some() || this.checkout_panel.expanded)
                {
                    this.checkout_panel.publish = None;
                    this.checkout_panel.expanded = false;
                    if let Some(focus) = this.checkout_panel.return_focus.take() {
                        focus.focus(window, cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
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
            .child(
                gpui::canvas(
                    {
                        let bounds = self.checkout_panel.bounds.clone();
                        move |rect, _, _| bounds.set(rect)
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(card);
        if self.checkout_panel.publish.is_some() {
            if let Some(focus) = &self.checkout_panel.confirmation_focus {
                panel = panel.track_focus(focus);
            }
        }
        if let Some(popup) = self.checkout_panel.popup.clone() {
            let position = self
                .checkout_panel
                .popup_position
                .unwrap_or_else(|| self.checkout_panel.bounds.get().bottom_right());
            panel = panel.child(gpui::deferred(
                gpui::anchored()
                    .position_mode(gpui::AnchoredPositionMode::Window)
                    .anchor(gpui::Anchor::TopRight)
                    .position(position)
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
        kind: &'static str,
        idx: usize,
        path: String,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.checkout_panel.popup_position = Some(position);
        if self.checkout_panel.path.as_ref() != Some(&path) {
            return;
        }
        self.checkout_panel.return_focus = self.checkout_panel.trigger_focus.get(kind).cloned();
        if self.checkout_panel.confirmation_focus.is_none() {
            self.checkout_panel.confirmation_focus = Some(cx.focus_handle());
        }
        let owner = cx.entity().downgrade();
        let state = self.checkout_panel.state.clone().and_then(Result::ok);
        let project_path = cx.global::<SettingsState>().store.projects[idx]
            .path
            .clone();
        let live = self.checkout_has_live_session(&path, cx);
        let busy = self.checkout_panel.busy || self.checkout_panel.loading;
        let popup = PopupMenu::build(window, cx, move |mut popup, _, _| {
            let items: Vec<(String, bool)> = match kind {
                "folder" => vec![
                    ("Copy path".into(), false),
                    ("Open terminal".into(), false),
                    ("Refresh status".into(), busy),
                ],
                "branch" => vec![("New worktree…".into(), false)],
                "publish" => state
                    .as_ref()
                    .map(|state| {
                        state
                            .remotes
                            .iter()
                            .map(|remote| (remote.clone(), busy))
                            .collect()
                    })
                    .unwrap_or_default(),
                _ => vec![
                    (
                        "Fetch".into(),
                        busy || state.as_ref().is_none_or(|state| state.remotes.is_empty()),
                    ),
                    (
                        "Pull".into(),
                        busy || live
                            || state
                                .as_ref()
                                .is_none_or(|state| state.upstream.is_none() || state.dirty),
                    ),
                    (
                        "Push".into(),
                        busy || state.as_ref().is_none_or(|state| {
                            state.upstream.is_none()
                                || state.head.is_none()
                                || state.branch.is_none()
                        }),
                    ),
                    ("Refresh status".into(), busy),
                ],
            };
            for (label, disabled) in items {
                let owner = owner.clone();
                let path = path.clone();
                let project_path = project_path.clone();
                let value = label.clone();
                popup = popup.item(PopupMenuItem::new(label).disabled(disabled).on_click(
                    move |_, window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            if this.checkout_panel.path.as_ref() != Some(&path) {
                                return;
                            }
                            if kind == "publish" {
                                this.checkout_panel.publish = Some(value.clone());
                                cx.notify();
                                return;
                            }
                            match value.as_str() {
                                "Copy path" => cx.write_to_clipboard(
                                    gpui::ClipboardItem::new_string(path.clone()),
                                ),
                                "Open terminal" => {
                                    this.checkout_panel.return_focus = None;
                                    this.act(
                                        Action::Launch(idx, path.clone(), Agent::Terminal),
                                        window,
                                        cx,
                                    );
                                }
                                "New worktree…" => {
                                    this.checkout_panel.return_focus = None;
                                    this.act(Action::NewWorktree(project_path.clone()), window, cx);
                                }
                                "Refresh status" => this.checkout_load(path.clone(), cx),
                                "Fetch" => this.checkout_run(path.clone(), GitAction::Fetch, cx),
                                "Pull" => this.checkout_run(path.clone(), GitAction::Pull, cx),
                                "Push" => this.checkout_run(path.clone(), GitAction::Push, cx),
                                _ => {}
                            }
                        });
                    },
                ));
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
                    if let Some(focus) = this.checkout_panel.confirmation_focus.as_ref() {
                        focus.focus(window, cx);
                    }
                } else if let Some(focus) = this.checkout_panel.return_focus.as_ref() {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn card_clears_appbar_and_keeps_narrow_canvas_header_in_place(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "Checkout QA".into(),
                    path: "/grove-checkout-layout-fixture/a/very/long/path/which/must/not/wrap"
                        .into(),
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
        let path = cx.debug_bounds("checkout-path").unwrap();
        assert!(path.top() >= gpui::px(APPBAR_H + SPACE_3XL));
        assert!(path.size.height <= gpui::px(CONTROL_H), "{path:?}");
        cx.simulate_resize(gpui::size(gpui::px(500.), gpui::px(600.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let header = cx.debug_bounds("canvas-section-header").unwrap();
        let panel = cx.debug_bounds("checkout-panel").unwrap();
        assert_eq!(header.top(), gpui::px(0.));
        assert!(panel.top() >= header.bottom());
        assert!(panel.bottom() <= gpui::px(600.));
    }
    #[test]
    fn registry_native_target_and_secondary_root_block_pull_until_removed() {
        use crate::entities::session_registry::SessionRegistry;
        let mut registry = SessionRegistry::new();
        let id = registry.insert_meta_with_context(
            "One".into(),
            "/one".into(),
            Agent::Codex,
            vec![grove_core::session_meta::ContextRoot {
                project: "Two".into(),
                wt_path: "/two".into(),
            }],
            None,
        );
        assert!(registry_blocks_pull(&registry, "/one", |_| None));
        assert!(registry_blocks_pull(&registry, "/two", |_| None));
        assert!(!registry_blocks_pull(&registry, "/other", |_| None));
        registry.remove(id);
        assert!(!registry_blocks_pull(&registry, "/one", |_| None));
    }
    #[test]
    fn native_and_pending_sessions_block_pull_even_without_a_terminal_entity() {
        assert!(session_blocks_pull(true, None));
        assert!(session_blocks_pull(true, Some(true)));
        assert!(session_blocks_pull(false, None));
        assert!(session_blocks_pull(false, Some(false)));
        assert!(!session_blocks_pull(false, Some(true)));
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
        assert_eq!(selected_checkout(&[], Some("/one")), None);
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
        let hidden = CheckoutPanel {
            generation: 4,
            ..Default::default()
        };
        assert!(!hidden.accepts_result("/one", 3));
    }
}
