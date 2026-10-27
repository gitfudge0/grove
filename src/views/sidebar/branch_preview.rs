//! Checkout-owned recent history, following the existing hover preview contract.
use super::*;
use crate::views::status::{self, StatusKind};
const PEEK_W: f32 = 380.;
const PEEK_MAX_H: f32 = 300.;
const PEEK_ROW_H: f32 = 36.;
use super::branch_history::{HISTORY_HOVER_GRACE, HISTORY_HOVER_INTENT};

pub(super) struct BranchPeek {
    selection: Option<Selection>,
    path: String,
    identity: Option<[u8; 32]>,
    focus: FocusHandle,
    pinned: bool,
}
pub(super) struct BranchPeekCache {
    path: String,
    identity: Option<[u8; 32]>,
    pub(super) data: Entity<BranchHistoryState>,
    _observer: gpui::Subscription,
}
impl Sidebar {
    pub(super) fn dismiss_branch_peek(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.branch_peek_task = None;
        if let Some(peek) = self.branch_peek.take() {
            if peek.focus.contains_focused(window, cx) {
                self.branch_entry_focus.focus(window, cx);
            }
        }
        cx.notify();
    }
    pub(super) fn open_branch_peek(
        &mut self,
        explicit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.history.is_some()
            || self.confirmation_open()
            || self.menu.is_some()
            || self.project_panel.is_some()
            || self.project_setup.is_some()
            || self.pending_new_worktree.is_some()
            || self
                .settings_panel
                .as_ref()
                .is_some_and(|settings| settings.read(cx).is_open())
        {
            return;
        }
        let Some((path, identity)) = self.branch_entry.clone() else {
            return;
        };
        self.dismiss_script_peek(window, cx);
        self.dismiss_agent_peek(window, cx);
        self.dismiss_plan_peek(window, cx);
        self.branch_peek_task = None;
        let replace = self
            .branch_peek_cache
            .as_ref()
            .is_none_or(|cache| cache.path != path || cache.identity != identity);
        if replace {
            let data = cx.new(|cx| BranchHistoryState::with_limit(path.clone(), 5, cx));
            let observer = cx.observe(&data, |_, _, cx| cx.notify());
            self.branch_peek_cache = Some(BranchPeekCache {
                path: path.clone(),
                identity,
                data,
                _observer: observer,
            });
        }
        let focus = cx.focus_handle();
        if explicit {
            crate::platform::inline_html::NativeDocument::focus_host(window);
            focus.focus(window, cx);
        }
        self.branch_peek = Some(BranchPeek {
            selection: self.selection.clone(),
            path,
            identity,
            focus,
            pinned: explicit,
        });
        cx.notify();
    }
    fn branch_hover(&mut self, hovered: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.branch_peek_task = None;
        if self.branch_peek.as_ref().is_some_and(|peek| peek.pinned) {
            return;
        }
        let entry = self.branch_entry.clone();
        let selection = self.selection.clone();
        self.branch_peek_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(if hovered {
                    HISTORY_HOVER_INTENT
                } else {
                    HISTORY_HOVER_GRACE
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.branch_entry != entry || this.selection != selection {
                    return;
                }
                if hovered {
                    this.open_branch_peek(false, window, cx);
                } else {
                    this.dismiss_branch_peek(window, cx);
                }
            });
        }));
    }
    pub(super) fn render_branch_history_entry(
        &mut self,
        path: String,
        label: String,
        identity: Option<[u8; 32]>,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entry = (path.clone(), identity);
        if self.branch_entry.as_ref() != Some(&entry) {
            self.branch_peek = None;
            self.branch_peek_task = None;
            self.branch_entry = Some(entry);
        }
        let bounds = self.branch_entry_bounds.clone();
        let tooltip = format!("{label} · {path} · Recent commits and branch history");
        div()
            .id("checkout-branch-history")
            .debug_selector(|| "checkout-branch-history".into())
            .role(gpui::Role::Button)
            .aria_label(tooltip)
            .aria_expanded(self.branch_peek.is_some())
            .track_focus(&self.branch_entry_focus)
            .tab_index(0)
            .relative()
            .cursor_pointer()
            .h(rpx(ROW_H))
            .w_full()
            .min_w_0()
            .px(rpx(SPACE_2XL))
            .flex()
            .items_center()
            .gap(rpx(SPACE_2XL))
            .text_size(rpx(TEXT_BODY))
            .hover(|style| style.bg(c::BG_HOVER()))
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .child(
                gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .child(icon("git-branch", ICON_SM, c::FG_DIM()))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                this.branch_hover(*hovered, window, cx);
            }))
            .on_click(cx.listener(|this, _, window, cx| this.open_branch_peek(true, window, cx)))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.open_branch_peek(true, window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .into_any_element()
    }
    pub(super) fn render_branch_peek(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let peek = self.branch_peek.as_ref()?;
        if peek.selection != self.selection
            || self.branch_entry.as_ref() != Some(&(peek.path.clone(), peek.identity))
            || self.history.is_some()
            || self.confirmation_open()
            || self.menu.is_some()
            || self.project_panel.is_some()
            || self.project_setup.is_some()
            || self
                .settings_panel
                .as_ref()
                .is_some_and(|settings| settings.read(cx).is_open())
        {
            self.branch_peek = None;
            self.branch_peek_task = None;
            return None;
        }
        let path = peek.path.clone();
        let focus = peek.focus.clone();
        let cache = self.branch_peek_cache.as_ref()?;
        let data = cache.data.read(cx).clone();
        let margin = rpx(SPACE_LG).to_pixels(window.rem_size());
        let width = rpx(PEEK_W)
            .to_pixels(window.rem_size())
            .min(window.viewport_size().width - margin * 2.);
        let height = rpx(PEEK_MAX_H)
            .to_pixels(window.rem_size())
            .min(window.viewport_size().height - margin * 2.);
        let mut rows = div()
            .id("branch-peek-rows")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if data.loading {
            rows = rows.child(status::label(
                "branch-peek-loading",
                StatusKind::Working,
                "Loading recent commits…",
                ICON_SM,
                cx,
            ));
        } else if let Some(error) = &data.error {
            rows = rows
                .child(status::label(
                    "branch-peek-error",
                    StatusKind::Failed,
                    error.clone(),
                    ICON_SM,
                    cx,
                ))
                .child(
                    self.control(
                        "branch-peek-retry",
                        "Retry recent commits",
                        Action::RefreshBranchPreview,
                        cx,
                    )
                    .w_auto()
                    .px(rpx(SPACE_LG))
                    .child(icon("reset", ICON_SM, c::FG_DIM()))
                    .child("Retry"),
                );
        } else if let Some(snapshot) = &data.snapshot {
            if snapshot.commits.is_empty() {
                rows = rows.child(status::label(
                    "branch-peek-empty",
                    StatusKind::Idle,
                    "No commits yet",
                    ICON_SM,
                    cx,
                ));
            }
            for commit in snapshot.commits.iter().take(5) {
                let identity = format!(
                    "{} · {} <{}> · {} · {}",
                    commit.subject,
                    commit.author_name,
                    commit.author_email,
                    super::branch_history::readable_date(&commit.author_date, false),
                    commit.id
                );
                rows = rows.child(
                    div()
                        .id(format!("branch-peek-commit-{}", commit.id))
                        .debug_selector(|| "branch-peek-commit".into())
                        .min_h(rpx(PEEK_ROW_H))
                        .aria_label(identity)
                        .child(super::branch_history::commit_identity(commit)),
                );
            }
        }
        let heading = data.snapshot.as_ref().map_or_else(
            || "Recent commits".to_owned(),
            |snapshot| match &snapshot.head {
                grove_core::history::HeadIdentity::Branch { name, .. } => {
                    format!("Recent commits · {name}")
                }
                grove_core::history::HeadIdentity::Detached { oid } => {
                    format!("Recent commits · Detached HEAD {}", &oid[..7])
                }
            },
        );
        let source = self.branch_entry_bounds.get();
        let owner = peek.selection.clone();
        let identity = peek.identity;
        let content = div()
            .id("branch-history-peek")
            .debug_selector(|| "branch-history-peek".into())
            .role(gpui::Role::Dialog)
            .aria_label("Recent branch commits")
            .track_focus(&focus)
            .occlude()
            .w(width)
            .h(height)
            .p(rpx(SPACE_2XL))
            .rounded(rpx(RADIUS_PANEL))
            .border_1()
            .border_color(c::BORDER_SOFT())
            .bg(c::SURFACE_RAISED())
            .text_size(rpx(TEXT_SMALL))
            .text_color(c::FG())
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                if *hovered {
                    this.branch_peek_task = None;
                } else {
                    this.branch_hover(false, window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    if let Some(peek) = this.branch_peek.as_mut() {
                        peek.pinned = true;
                        peek.focus.focus(window, cx);
                    }
                }),
            )
            .on_mouse_down_out(
                cx.listener(|this, _, window, cx| this.dismiss_branch_peek(window, cx)),
            )
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.dismiss_branch_peek(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                } else if event.keystroke.key == "tab" {
                    if let Some(peek) = &this.branch_peek {
                        let focus = peek.focus.clone();
                        for _ in 0..64 {
                            if event.keystroke.modifiers.shift {
                                window.focus_prev(cx);
                            } else {
                                window.focus_next(cx);
                            }
                            if focus.contains_focused(window, cx) {
                                break;
                            }
                        }
                        if !focus.contains_focused(window, cx) {
                            focus.focus(window, cx);
                        }
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .child(div().flex_shrink_0().text_color(c::FG_DIM()).child(heading))
            .child(rows)
            .child(
                self.control(
                    "branch-peek-view-history",
                    "View history",
                    Action::OpenBranchHistory(path, owner, identity),
                    cx,
                )
                .debug_selector(|| "branch-peek-view-history".into())
                .w_auto()
                .px(rpx(SPACE_LG))
                .child(icon("history", ICON_SM, c::FG_DIM()))
                .child("View history"),
            );
        let panel = div()
            .relative()
            .w(width)
            .h(height)
            .child(
                gpui::canvas(
                    |bounds, _, cx| {
                        super::super::inline_html::exclude_native_html(Some(bounds), cx);
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(content);
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position_mode(gpui::AnchoredPositionMode::Window)
                    .anchor(gpui::Anchor::TopRight)
                    .position(gpui::point(source.left() - margin, source.top()))
                    .snap_to_window_with_margin(margin)
                    .child(panel),
            )
            .into_any_element(),
        )
    }
}
