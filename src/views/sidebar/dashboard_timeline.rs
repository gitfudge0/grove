//! Dashboard progress derived from cached structured-session facts; never provider guesses.
use super::super::codex_view::ActivityRequestTarget;
use super::*;
use gpui::AnimationExt as _;
use grove_core::{
    codex::Status,
    tasks::{Task, TaskStatus},
};

mod activity;
mod review_hold;
mod task_scope;

use activity::shimmering_activity;
pub(super) use activity::{recent_activity, Activity, TimelineData};

const TIMELINE_TASK_LINE_H: f32 = super::SESSION_TITLE_LINE_H;
const TIMELINE_SPINE_W: f32 = 1.;
const REVIEW_STATUS_SLOT_W: f32 = 112.;
// The approved Make room entrance plays the shared slow motion at half speed.
const SESSION_ENTRANCE_MS: u64 = MOTION_SLOW_MS * 2;
const SESSION_ENTRANCE_OPACITY: f32 = 0.35;
pub(super) struct RowMetadata {
    pub project: String,
    pub worktree: String,
    pub provider: grove_core::codex::Provider,
    pub model: String,
    pub effort: Option<String>,
    pub context: String,
    pub status: Status,
    pub display_status: &'static str,
    pub review_completion: Option<usize>,
    pub age: String,
}
impl RowMetadata {
    fn model_label(&self) -> String {
        let model = &self.model;
        self.effort
            .as_deref()
            .map(str::trim)
            .filter(|effort| !effort.is_empty())
            .map_or_else(
                || model.to_owned(),
                |effort| {
                    let effort = if effort == "xhigh" {
                        "Extra high".into()
                    } else {
                        let mut letters = effort.chars();
                        letters.next().map_or_else(String::new, |first| {
                            first.to_uppercase().chain(letters).collect()
                        })
                    };
                    format!("{model} {effort}")
                },
            )
    }
}
fn dashboard_review_completion(metadata: &RowMetadata) -> Option<usize> {
    matches!(metadata.display_status, "Review" | "Review · unread")
        .then_some(metadata.review_completion)
        .flatten()
}
fn metadata_group(glyph: &str, text: String, window: &Window) -> AnyElement {
    let selector = format!("dashboard-meta-label-{glyph}");
    let mut run = window.text_style().to_run(text.len());
    run.font.family = crate::fonts::UI_FAMILY.into();
    let text_width = super::super::components::truncation_safe_label_width(
        &text,
        rpx(TEXT_SMALL).to_pixels(window.rem_size()),
        &run,
        window.text_system(),
    );
    div()
        .id(gpui::SharedString::from(format!(
            "dashboard-meta-{glyph}-{text}"
        )))
        .role(gpui::Role::Group)
        .aria_label(text.clone())
        .min_w_0()
        // Reserve a physical pixel for fractional layout rounding at changed zoom.
        .w(text_width + rpx(ICON_SM + SPACE_SM).to_pixels(window.rem_size()) + gpui::px(1.))
        .max_w(gpui::relative(1.))
        .flex()
        .items_start()
        .gap(rpx(SPACE_SM))
        .child(icon(glyph, ICON_SM, c::FG_DIM()).flex_shrink_0())
        .child(
            div()
                .debug_selector(move || selector.clone())
                .flex_1()
                .min_w_0()
                .whitespace_normal()
                .child(text),
        )
        .into_any_element()
}
fn render_metadata(metadata: &RowMetadata, window: &Window) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .gap_x(rpx(SPACE_2XL))
        .gap_y(rpx(SPACE_SM))
        .text_size(rpx(TEXT_SMALL))
        .line_height(rpx(SESSION_META_LINE_H))
        .text_color(c::FG_DIM())
        .child(metadata_group("folder", metadata.project.clone(), window))
        .child(metadata_group(
            "git-branch",
            metadata.worktree.clone(),
            window,
        ))
        .child(metadata_group(
            metadata.provider.agent().icon_name(),
            metadata.model_label(),
            window,
        ))
        .into_any_element()
}
type Scope = (u64, SessionId);

#[derive(Default)]
pub(super) struct DashboardTimelines {
    pub(super) replies: HashMap<Scope, super::activity_reply::ActivityReply>,
    pub(super) reply_open: Option<Scope>,
    pub(super) reply_heights: HashMap<Scope, std::rc::Rc<std::cell::Cell<gpui::Pixels>>>,
    pub(super) reply_focus: HashMap<Scope, FocusHandle>,
    activity_scope: Option<(u64, Vec<SessionId>)>,
    #[cfg(test)]
    warm_requests: usize,
    opened: HashSet<Scope>,
    focus: HashMap<Scope, FocusHandle>,
    open_focus: HashMap<Scope, FocusHandle>,
    request_focus: HashMap<Scope, FocusHandle>,
    pub(super) return_focus: Option<Scope>,
    pub(super) scroll: gpui::ScrollHandle,
    pub(super) row_anchors: HashMap<Scope, gpui::ScrollAnchor>,
    reveals: HashMap<Scope, TimelineReveal>,
    entrances: HashMap<Scope, SessionEntrance>,
    task_opened: HashSet<(Scope, String)>,
    task_focus: HashMap<(Scope, String), FocusHandle>,
    task_hovered: HashSet<(Scope, String)>,
    review_hovered: HashSet<Scope>,
    pub(super) review_holds: review_hold::ReviewHolds,
    review_disclosures:
        HashMap<Scope, Option<gpui::Entity<super::super::review_summary::SummaryDisclosure>>>,
    review_link_focus: HashMap<Scope, HashMap<String, FocusHandle>>,
    review_focus: HashMap<Scope, (FocusHandle, Vec<gpui::Subscription>)>,
    pub(super) review_order: Vec<SessionId>,
    pub(super) review_advance: Option<gpui::Subscription>,
    group_opened: HashSet<(Scope, String)>,
    group_focus: HashMap<(Scope, String), FocusHandle>,
}
impl DashboardTimelines {
    /// Both structured disclosures and terminal navigation rows keep a stable
    /// session focus target when their transient review action disappears.
    pub(super) fn session_focus(
        &mut self,
        workspace: u64,
        id: SessionId,
        cx: &mut App,
    ) -> FocusHandle {
        self.focus
            .entry((workspace, id))
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    pub(super) fn review_action_focus(&self, scope: Scope) -> Option<FocusHandle> {
        self.review_focus.get(&scope).map(|entry| entry.0.clone())
    }
    /// Review acknowledgment removes its action; return keyboard focus to the
    /// surviving row for the same session rather than a different row.
    pub(super) fn restore_review_focus(
        &self,
        workspace: u64,
        id: SessionId,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(focus) = self.focus.get(&(workspace, id)) {
            focus.focus(window, cx);
        }
    }
    pub(super) fn clear_entrances(&mut self) {
        self.entrances.clear();
    }
    pub(super) fn adopt_session(&mut self, workspace: u64, id: SessionId, reduced: bool) {
        if !reduced {
            self.entrances.entry((workspace, id)).or_default();
        }
    }
    #[cfg(test)]
    pub(super) fn is_entering(&self, workspace: u64, id: SessionId) -> bool {
        self.entrances.contains_key(&(workspace, id))
    }
    fn invalidate_scope(&mut self) {
        self.activity_scope = None;
    }
    fn needs_activity_warm(
        &mut self,
        workspace: u64,
        open: bool,
        mut sessions: Vec<SessionId>,
    ) -> bool {
        if !open {
            self.review_holds.clear();
            self.review_order.clear();
            self.review_advance = None;
            self.invalidate_scope();
            self.clear_entrances();
            return false;
        }
        sessions.sort_unstable();
        sessions.dedup();
        if self
            .activity_scope
            .as_ref()
            .is_some_and(|(owner, ids)| *owner == workspace && ids == &sessions)
        {
            return false;
        }
        self.activity_scope = Some((workspace, sessions));
        true
    }
    #[cfg(test)]
    pub fn is_open(&self, workspace: u64, id: SessionId) -> bool {
        self.opened.contains(&(workspace, id))
    }
    pub(super) fn toggle(&mut self, key: Scope) {
        if !self.opened.remove(&key) {
            self.opened.insert(key);
        }
    }
    fn toggle_task(&mut self, key: (Scope, String)) {
        if !self.task_opened.remove(&key) {
            self.task_opened.insert(key);
        }
    }
    pub fn retain(&mut self, sessions: &HashSet<SessionId>, workspaces: &HashSet<u64>) {
        let keep = |key: &Scope| sessions.contains(&key.1) && workspaces.contains(&key.0);
        self.replies.retain(|key, _| keep(key));
        self.reply_focus.retain(|key, _| keep(key));
        self.reply_heights.retain(|key, _| keep(key));
        if self.reply_open.is_some_and(|key| !keep(&key)) {
            self.reply_open = None;
        }
        self.opened.retain(keep);
        self.focus.retain(|key, _| keep(key));
        self.open_focus.retain(|key, _| keep(key));
        self.request_focus.retain(|key, _| keep(key));
        self.row_anchors.retain(|key, _| keep(key));
        self.return_focus = self.return_focus.filter(keep);
        self.reveals.retain(|key, _| keep(key));
        self.entrances.retain(|key, _| keep(key));
        self.review_hovered.retain(keep);
        self.review_holds.retain(sessions, workspaces);
        self.review_disclosures.retain(|key, _| keep(key));
        self.review_link_focus.retain(|key, _| keep(key));
        self.review_focus.retain(|key, _| keep(key));
        self.task_opened.retain(|(key, _)| keep(key));
        self.task_focus.retain(|(key, _), _| keep(key));
        self.task_hovered.retain(|(key, _)| keep(key));
        self.group_opened.retain(|(key, _)| keep(key));
        self.group_focus.retain(|(key, _), _| keep(key));
    }
}
#[derive(Default)]
struct SessionEntrance {
    measured: f32,
    completion: Option<gpui::Task<()>>,
}
#[derive(Default)]
struct TimelineReveal {
    visible: bool,
    exiting: bool,
    measured: f32,
    from: f32,
    to: f32,
    revision: u64,
    painted: std::rc::Rc<std::cell::Cell<f32>>,
    close_task: Option<gpui::Task<()>>,
}
impl TimelineReveal {
    fn observe_height(&mut self, height: f32) -> bool {
        if (self.measured - height).abs() <= 0.5 {
            return false;
        }
        self.measured = height;
        if self.visible {
            self.target(height);
            true
        } else {
            false
        }
    }
    fn target(&mut self, height: f32) {
        self.from = self.painted.get();
        self.to = height;
        self.revision = self.revision.wrapping_add(1);
    }
}
impl Sidebar {
    pub(super) fn focus_next_dashboard_review(
        &self,
        id: SessionId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let timelines = &self.dashboard_timelines;
        let Some(index) = timelines.review_order.iter().position(|item| *item == id) else {
            return;
        };
        let registry = self.runtime.read(cx).registry.read(cx);
        let next = timelines.review_order[index + 1..].iter().find(|next| {
            registry.codex(**next).is_some_and(|session| {
                let state = &session.read(cx).state;
                matches!(state.session_status(), "Review" | "Review · unread")
                    && state.review_completion().is_some()
            })
        });
        if let Some(focus) =
            next.and_then(|next| timelines.focus.get(&(self.active_workspace, *next)))
        {
            focus.focus(window, cx);
        }
    }

    pub(super) fn dashboard_owns_session(&self, id: SessionId) -> bool {
        self.snapshot
            .projects
            .iter()
            .any(|project| project.sessions.contains(&id))
    }
    pub(super) fn dashboard_renders_session(&self, id: SessionId, cx: &App) -> bool {
        self.dashboard_owns_session(id)
            || self
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .meta(id)
                .is_some_and(|meta| meta.project.is_empty())
    }
    pub(super) fn enter_dashboard_timeline_scope(&mut self, cx: &mut Context<Self>) {
        self.dashboard_timelines.invalidate_scope();
        self.sync_dashboard_timeline_scope(cx);
    }
    pub(super) fn sync_dashboard_timeline_scope(&mut self, cx: &mut Context<Self>) {
        let owned = self
            .task_glances
            .keys()
            .copied()
            .filter(|id| self.dashboard_renders_session(*id, cx))
            .collect();
        if self.dashboard_timelines.needs_activity_warm(
            self.active_workspace,
            self.dashboard_open,
            owned,
        ) {
            self.refresh_dashboard_timelines(cx);
        }
    }
    pub(super) fn open_activity_link(
        &mut self,
        id: SessionId,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(id)
            .map(|session| std::path::PathBuf::from(&session.read(cx).state.saved.cwd));
        let Some(target) = cwd.and_then(|cwd| super::super::file_browser::local_target(url, &cwd))
        else {
            cx.open_url(url);
            return;
        };
        if !self.navigation_available() {
            return;
        }
        self.dashboard_timelines.return_focus = Some((self.active_workspace, id));
        self.navigate_to_session(id, window, cx);
        // Materialize the owning transcript before opening, including sessions
        // that have never been visited in this app process.
        let _ = self.terminal_content(id, false, window, cx);
        if let Some(view) = self.codex_views.get(&id) {
            view.read(cx)
                .file_browser
                .clone()
                .update(cx, |browser, cx| {
                    browser.open(target, window, cx);
                });
            cx.notify();
        }
    }

    fn open_activity_request(
        &mut self,
        scope: Scope,
        target: ActivityRequestTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = scope.1;
        let registry = self.runtime.read(cx).registry.read(cx);
        let in_scope = self.dashboard_renders_session(id, cx);
        let current = registry
            .codex(id)
            .is_some_and(|session| target.is_current(&session.read(cx).state));
        if !self.dashboard_open
            || self.active_workspace != scope.0
            || !in_scope
            || !self.navigation_available()
            || !current
        {
            if self.dashboard_open && self.active_workspace == scope.0 {
                self.dashboard_timelines
                    .restore_review_focus(scope.0, id, window, cx);
            }
            cx.notify();
            return;
        }
        self.dashboard_timelines.return_focus = Some(scope);
        self.navigate_to_session(id, window, cx);
        let _ = self.terminal_content(id, false, window, cx);
        if let Some(view) = self.codex_views.get(&id) {
            view.update(cx, |view, cx| {
                view.focus_activity_request(target, window, cx);
            });
        }
    }

    fn refresh_dashboard_timelines(&mut self, cx: &mut Context<Self>) {
        #[cfg(test)]
        {
            self.dashboard_timelines.warm_requests += 1;
        }
        let sessions = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .all()
            .iter()
            .filter_map(|meta| {
                if !self.dashboard_renders_session(meta.id, cx) {
                    return None;
                }
                self.runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .codex(meta.id)
                    .map(|session| (meta.id, session.clone()))
            })
            .collect::<Vec<_>>();
        for (id, session) in sessions {
            let state = &session.read(cx).state;
            let cache = self.task_glances.entry(id).or_default();
            cache.refresh_snapshot(&state.saved.items);
            cache.refresh_dashboard_snapshot(state);
        }
    }
    pub(super) fn render_dashboard_progress(
        &mut self,
        id: SessionId,
        title: &str,
        metadata: &RowMetadata,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status = metadata.status;
        let age = &metadata.age;
        let key = (self.active_workspace, id);
        let scroll = self.dashboard_timelines.scroll.clone();
        let anchor = self
            .dashboard_timelines
            .row_anchors
            .entry(key)
            .or_insert_with(|| gpui::ScrollAnchor::for_handle(scroll))
            .clone();
        let open = self.dashboard_timelines.opened.contains(&key);
        let focus = self
            .dashboard_timelines
            .focus
            .entry(key)
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let open_focus = self
            .dashboard_timelines
            .open_focus
            .entry(key)
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let click_focus = focus.clone();
        let open_click_focus = open_focus.clone();
        let data = self
            .task_glances
            .get(&id)
            .map(super::task_glance::TaskGlanceCache::timeline)
            .unwrap_or_default();
        let accessible = format!("Activity history · {}", metadata.context);
        let review_completion = dashboard_review_completion(metadata);
        let request = matches!(metadata.display_status, "Approval needed" | "Answer needed")
            .then(|| {
                self.task_glances
                    .get(&id)
                    .and_then(super::task_glance::TaskGlanceCache::dashboard_request)
            })
            .flatten();
        let review_summary = review_completion.and_then(|_| {
            let registry = self.runtime.read(cx).registry.read(cx);
            registry.codex(id).and_then(|session| {
                super::super::review_summary::ReviewSummary::from_snapshot(&session.read(cx).state)
            })
        });
        let summary_owner = cx.entity().downgrade();
        let summary_view = review_summary.map(|summary| {
            summary.render(
                &format!("dashboard-review-summary-{}", id.raw()),
                self.dashboard_timelines
                    .review_link_focus
                    .entry(key)
                    .or_default(),
                self.dashboard_timelines
                    .review_disclosures
                    .entry(key)
                    .or_default(),
                move |url, window, cx| {
                    let _ = summary_owner.update(cx, |this, cx| {
                        this.open_activity_link(id, url, window, cx);
                    });
                },
                cx,
            )
        });
        let status_action = if let Some(receipt) = review_completion {
            Some((
                "dashboard-mark-reviewed",
                "Mark reviewed",
                Action::MarkReviewed(id, receipt),
            ))
        } else if metadata.display_status == "Ready" {
            Some((
                "dashboard-mark-done",
                "Mark done",
                Action::MarkActivityDone(id),
            ))
        } else {
            None
        };
        if status_action.is_some() && !self.dashboard_timelines.review_focus.contains_key(&key) {
            let handle = cx.focus_handle();
            let subscriptions = vec![
                cx.on_focus_in(&handle, window, |_, _, cx| cx.notify()),
                cx.on_focus_out(&handle, window, |_, _, _, cx| cx.notify()),
            ];
            self.dashboard_timelines
                .review_focus
                .insert(key, (handle, subscriptions));
        }
        let review_focus = self.dashboard_timelines.review_action_focus(key);
        let show_action = review_completion.is_some()
            || status_action.is_some()
                && (self.dashboard_timelines.review_hovered.contains(&key)
                    || window.last_input_was_keyboard()
                        && review_focus
                            .as_ref()
                            .is_some_and(|focus| focus.contains_focused(window, cx)));
        let status_label = super::super::status::label(
            format!("dashboard-session-{}", id.raw()),
            super::super::status::session_kind(metadata.display_status),
            metadata.display_status,
            TEXT_SMALL,
            cx,
        )
        .debug_selector(move || format!("dashboard-session-status-{}", id.raw()))
        .flex_shrink_0()
        .text_size(rpx(TEXT_SMALL));
        let status_slot = if let (Some((action_id, label, action)), Some(action_focus)) =
            (status_action, review_focus.as_ref())
        {
            let action = self
                .control((action_id, id.raw()), label, action, cx)
                .debug_selector(move || format!("{action_id}-{}", id.raw()))
                .track_focus(action_focus)
                .tab_stop(true)
                .w_auto()
                .h(rpx(TIMELINE_TASK_LINE_H))
                .px_0()
                .gap(rpx(SPACE_SM))
                .text_size(rpx(TEXT_SMALL))
                .child(icon("check", ICON_SM, c::FG_DIM()))
                .child(label);
            // Both states keep the same trailing geometry.
            div()
                .relative()
                .flex_shrink_0()
                .min_w(rpx(REVIEW_STATUS_SLOT_W))
                .h(rpx(TIMELINE_TASK_LINE_H))
                .flex()
                .items_center()
                .justify_end()
                .child(
                    div()
                        .opacity(if show_action { 0. } else { 1. })
                        .child(status_label),
                )
                .child(div().absolute().right_0().top_0().child(if show_action {
                    if review_completion.is_some() || cx.reduce_motion() {
                        action.into_any_element()
                    } else {
                        action
                            .with_animation(
                                format!("review-reveal-{key:?}"),
                                gpui::Animation::new(std::time::Duration::from_millis(
                                    MOTION_FAST_MS,
                                ))
                                .with_easing(motion::ease_standard),
                                gpui::Styled::opacity,
                            )
                            .into_any_element()
                    }
                } else {
                    action.opacity(0.).into_any_element()
                }))
                .into_any_element()
        } else {
            status_label.into_any_element()
        };
        let review_key_focus = review_focus.clone();
        let toggle = div()
            .id(("dashboard-progress-toggle", id.raw()))
            .debug_selector(move || format!("dashboard-progress-toggle-{}", id.raw()))
            .role(gpui::Role::Button)
            .aria_label(accessible)
            .aria_expanded(open)
            .tab_index(0)
            .tab_stop(true)
            .track_focus(&focus)
            .flex_1()
            .min_w_0()
            .cursor_pointer()
            .group("neutral-control")
            .flex()
            .items_start()
            .gap(rpx(SPACE_LG))
            .line_height(rpx(TIMELINE_TASK_LINE_H))
            .py(rpx(SPACE_SM))
            .text_color(if open { c::FG() } else { c::FG_DIM() })
            .hover(|style| style.text_color(c::FG()))
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .on_mouse_down(gpui::MouseButton::Left, {
                let focus = focus.clone();
                move |_, window, cx| {
                    focus.focus(window, cx);
                    cx.stop_propagation();
                }
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                click_focus.focus(window, cx);
                cx.stop_propagation();
                this.dashboard_timelines.toggle(key);
                cx.notify();
            }))
            .capture_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if review_key_focus
                        .as_ref()
                        .is_some_and(|focus| focus.contains_focused(window, cx))
                    {
                        return;
                    }
                    if !event.is_held && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.dashboard_timelines.toggle(key);
                        window.prevent_default();
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .w(rpx(ICON_LG))
                    .h(rpx(TIMELINE_TASK_LINE_H))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(
                        if open { "chev-down" } else { "chev-right" },
                        ICON_SM,
                        if open { c::FG() } else { c::FG_DIM() },
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(rpx(SPACE_LG))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .whitespace_normal()
                                    .text_size(rpx(TEXT_BODY))
                                    .child(self.session_title_label(
                                        id,
                                        title,
                                        "dashboard-title-shimmer",
                                        cx,
                                    )),
                            )
                            .child(status_slot)
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child(age.to_owned()),
                            ),
                    )
                    .child(render_metadata(metadata, window)),
            );
        let open_label = if metadata.display_status == "Plan ready" {
            "Review plan"
        } else {
            "Open session"
        };
        let open_button = super::super::components::header_control_frame(
            ("dashboard-open-session", id.raw()),
            format!("{open_label} · {}", metadata.context),
            c::BG_HOVER(),
        )
        .debug_selector(move || format!("dashboard-open-session-{}", id.raw()))
        .track_focus(&open_focus)
        .tab_index(0)
        .tab_stop(true)
        .focus_visible(|style| style.bg(c::BG_HOVER()))
        .on_click(cx.listener(move |this, _, window, cx| {
            open_click_focus.focus(window, cx);
            cx.stop_propagation();
            this.act(Action::Select(Selection::Session(id)), window, cx);
        }))
        .capture_key_down(
            cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                if !event.is_held && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.act(Action::Select(Selection::Session(id)), window, cx);
                }
            }),
        )
        .tooltip(move |window, cx| {
            super::super::components::tooltip(open_label, window).build(window, cx)
        })
        .child(icon("arrow-right", ICON_SM, c::FG_DIM()));
        let respond = self.activity_reply_button(key, cx);
        let body = self.render_dashboard_tasks(key, &data, status, window, cx);
        let detail = self.dashboard_progress_reveal(key, open, body, cx);
        let row = div()
            .id(("dashboard-progress", id.raw()))
            .anchor_scroll(Some(anchor))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.dashboard_timelines.review_hovered.insert(key);
                } else {
                    this.dashboard_timelines.review_hovered.remove(&key);
                }
                cx.notify();
            }))
            .w_full()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .py(rpx(SPACE_LG))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(rpx(SPACE_SM))
                    .child(toggle)
                    .child(respond)
                    .child(open_button),
            )
            .when_some(summary_view, |row, summary| {
                row.child(
                    div()
                        .min_w_0()
                        .pl(rpx(ICON_LG + SPACE_LG))
                        .pr(rpx(ICON_BTN_W))
                        .pt(rpx(SPACE_LG))
                        .child(summary),
                )
            })
            .when_some(request, |row, request| {
                let focus = self
                    .dashboard_timelines
                    .request_focus
                    .entry(key)
                    .or_insert_with(|| cx.focus_handle())
                    .clone();
                let mouse_target = request.target.clone();
                let action = super::super::components::inline_disclosure(
                    ("dashboard-review-request", id.raw()),
                    "Review request",
                )
                .debug_selector(move || format!("dashboard-review-request-{}", id.raw()))
                .track_focus(&focus)
                .tab_index(0)
                .tab_stop(true)
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .justify_start()
                .w_auto()
                .gap(rpx(SPACE_SM))
                .text_size(rpx(TEXT_SMALL))
                .child(icon("arrow-right", ICON_SM, c::FG_DIM()))
                .child("Review request")
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.open_activity_request(key, mouse_target.clone(), window, cx);
                }))
                .on_key_down(cx.listener(
                    move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if !event.is_held
                            && matches!(event.keystroke.key.as_str(), "enter" | "space")
                        {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.open_activity_request(key, request.target.clone(), window, cx);
                        }
                    },
                ));
                row.child(
                    div()
                        .pl(rpx(ICON_LG + SPACE_LG))
                        .pt(rpx(SPACE_SM))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_x(rpx(SPACE_LG))
                        .child(
                            div()
                                .min_w_0()
                                .whitespace_normal()
                                .text_size(rpx(TEXT_SMALL))
                                .child(request.cue),
                        )
                        .child(action),
                )
            })
            .when_some(
                matches!(metadata.display_status, "Starting" | "Working")
                    .then(|| data.current_action(status).map(str::to_owned))
                    .flatten(),
                |row, action| {
                    row.child(
                        div()
                            .debug_selector(move || {
                                format!("dashboard-current-action-{}", id.raw())
                            })
                            .min_w_0()
                            .pl(rpx(ICON_LG + SPACE_LG))
                            .pt(rpx(SPACE_SM))
                            .text_size(rpx(TEXT_SMALL))
                            .whitespace_normal()
                            .child(action),
                    )
                },
            )
            .child(detail)
            .into_any_element();
        let row = self.wrap_activity_reply(key, title, row, window, cx);
        let row = self.track_review_hold(key, row, window, cx);
        self.dashboard_session_entrance(key, row, cx)
    }
    fn dashboard_session_entrance(
        &mut self,
        key: Scope,
        row: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if cx.reduce_motion() {
            self.dashboard_timelines.clear_entrances();
            return row;
        }
        let Some(state) = self.dashboard_timelines.entrances.get(&key) else {
            return row;
        };
        let measured = state.measured;
        let owner = cx.entity().downgrade();
        let content =
            div()
                .w_full()
                .flex_shrink_0()
                .child(row)
                .on_prepaint(move |bounds, _, cx| {
                    let height = f32::from(bounds.size.height);
                    let _ = owner.update(cx, |this, cx| {
                        let Some(state) = this.dashboard_timelines.entrances.get_mut(&key) else {
                            return;
                        };
                        if height <= 0. || (state.measured - height).abs() <= 0.5 {
                            return;
                        }
                        state.measured = height;
                        if state.completion.is_none() {
                            state.completion = Some(cx.spawn(async move |this, cx| {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(SESSION_ENTRANCE_MS))
                                    .await;
                                let _ = this.update(cx, |this, cx| {
                                    this.dashboard_timelines.entrances.remove(&key);
                                    cx.notify();
                                });
                            }));
                        }
                        cx.notify();
                    });
                });
        let frame = div()
            .id(gpui::SharedString::from(format!(
                "session-entrance-{key:?}"
            )))
            .debug_selector(move || format!("dashboard-session-entrance-{}", key.1.raw()))
            .w_full()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(content);
        if measured <= 0. {
            return frame.max_h(gpui::px(0.)).into_any_element();
        }
        frame
            .with_animation(
                format!("session-entrance-{key:?}"),
                gpui::Animation::new(std::time::Duration::from_millis(SESSION_ENTRANCE_MS))
                    .with_easing(motion::ease_standard),
                move |element, t| {
                    element
                        .max_h(gpui::px(measured * t))
                        .opacity(SESSION_ENTRANCE_OPACITY + (1. - SESSION_ENTRANCE_OPACITY) * t)
                        .relative()
                        .top(rpx(-SPACE_MD * (1. - t)))
                },
            )
            .into_any_element()
    }
    fn render_dashboard_task_rows(
        &mut self,
        scope: Scope,
        data: &TimelineData,
        status: Status,
        tasks: &[Task],
        historical: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = format!("{}-{}", scope.0, scope.1.raw());
        let mut body = div().w_full().flex().flex_col().gap(rpx(SPACE_SM));
        let shimmer = shimmering_activity(&data.activity, status);
        for task in tasks {
            let identity = task_key(task);
            let state_key = (scope, identity.clone());
            let complete = task.status == TaskStatus::Completed;
            let open = (!complete && !historical)
                || self.dashboard_timelines.task_opened.contains(&state_key);
            let hovered = self.dashboard_timelines.task_hovered.contains(&state_key);
            let focus = self
                .dashboard_timelines
                .task_focus
                .entry(state_key.clone())
                .or_insert_with(|| cx.focus_handle())
                .clone();
            let row_id = format!("dashboard-task-{key}-{identity}");
            let label_id = format!("dashboard-task-label-{key}-{identity}");
            let disclosure = (complete || historical) && hovered;
            let mark_selector = format!(
                "dashboard-task-{}-{key}-{identity}",
                if disclosure { "chevron" } else { "status" }
            );
            let mark = div()
                .debug_selector(move || mark_selector.clone())
                .w(rpx(ICON_LG))
                .h(rpx(TIMELINE_TASK_LINE_H))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(if disclosure {
                    icon(task_disclosure_glyph(open), ICON_SM, c::FG_DIM()).into_any_element()
                } else if historical && !complete {
                    icon("minus", ICON_SM, c::FG_DIM()).into_any_element()
                } else if task.status == TaskStatus::InProgress
                    && matches!(status, Status::Working | Status::Starting)
                {
                    super::super::status::mark(
                        format!("dashboard-task-{key}-{identity}"),
                        super::super::status::StatusKind::Working,
                        ICON_SM,
                        cx,
                    )
                } else {
                    super::super::components::task_mark(
                        task.status,
                        super::task_glance::task_phase(status),
                        0,
                    )
                    .into_any_element()
                });
            let mut row = div()
                .id(gpui::SharedString::from(row_id.clone()))
                .w_full()
                .min_w_0()
                .debug_selector(move || row_id.clone())
                .flex()
                .items_start()
                .gap(rpx(SPACE_LG))
                .line_height(rpx(TIMELINE_TASK_LINE_H))
                .py(rpx(SPACE_SM))
                .text_size(rpx(TEXT_BODY))
                .text_color(c::FG())
                .child(mark)
                .child(
                    div()
                        .debug_selector(move || label_id.clone())
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .child(task.step.clone())
                        .when(historical && !complete, |el| {
                            el.child(
                                div()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child("Removed from current scope"),
                            )
                        }),
                );
            if complete || historical {
                let hover_key = state_key.clone();
                let click_key = state_key.clone();
                let keyboard_key = state_key.clone();
                row = row
                    .role(gpui::Role::Button)
                    .aria_label(format!(
                        "{} · {}",
                        if complete {
                            "Completed task"
                        } else {
                            "Removed from current scope"
                        },
                        task.step
                    ))
                    .aria_expanded(open)
                    .cursor_pointer()
                    .tab_index(0)
                    .tab_stop(true)
                    .track_focus(&focus)
                    .text_color(c::FG_DIM())
                    .hover(|style| style.text_color(c::FG()))
                    .focus_visible(|style| style.bg(c::BG_HOVER()))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.dashboard_timelines
                                .task_hovered
                                .insert(hover_key.clone());
                        } else {
                            this.dashboard_timelines.task_hovered.remove(&hover_key);
                        }
                        cx.notify();
                    }))
                    .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                        focus.focus(window, cx);
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.dashboard_timelines.toggle_task(click_key.clone());
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .capture_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if !event.is_held
                                && matches!(event.keystroke.key.as_str(), "enter" | "space")
                            {
                                this.dashboard_timelines.toggle_task(keyboard_key.clone());
                                window.prevent_default();
                                cx.stop_propagation();
                                cx.notify();
                            }
                        },
                    ));
            }
            body = body.child(row);
            if open {
                let items: Vec<_> = data
                    .activity
                    .iter()
                    .filter(|activity| {
                        activity
                            .task
                            .as_ref()
                            .is_some_and(|owner| task.owns_activity(owner))
                    })
                    .cloned()
                    .collect();
                if !items.is_empty() {
                    body = body.child(div().pl(rpx(ICON_LG + SPACE_LG)).child(
                        render_activity_with_current(
                            &key,
                            &format!("{key}-task-{identity}"),
                            &items,
                            status,
                            if historical { None } else { shimmer },
                            complete,
                            cx,
                        ),
                    ));
                }
            }
        }
        body.into_any_element()
    }
    fn dashboard_progress_reveal(
        &mut self,
        key: Scope,
        visible: bool,
        body: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if cx.reduce_motion() {
            self.dashboard_timelines.reveals.remove(&key);
            return if visible {
                body
            } else {
                div().into_any_element()
            };
        }
        let state = self.dashboard_timelines.reveals.entry(key).or_default();
        if state.visible != visible {
            state.visible = visible;
            state.exiting = !visible;
            state.close_task = None;
            state.target(if visible { state.measured } else { 0. });
            if !visible {
                let close_key = key;
                let revision = state.revision;
                state.close_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(MOTION_BASE_MS))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(state) = this.dashboard_timelines.reveals.get_mut(&close_key) {
                            if !state.visible && state.revision == revision {
                                state.exiting = false;
                                if this.dashboard_open && this.active_workspace == close_key.0 {
                                    cx.notify();
                                }
                            }
                        }
                    });
                }));
            }
        }
        if !visible && !state.exiting {
            return div().into_any_element();
        }
        let from = state.from;
        let to = state.to;
        let revision = state.revision;
        let painted = state.painted.clone();
        let measure_key = key;
        let owner = cx.entity().downgrade();
        let measured =
            div()
                .w_full()
                .flex_shrink_0()
                .child(body)
                .on_prepaint(move |bounds, _, cx| {
                    let height = f32::from(bounds.size.height);
                    let _ = owner.update(cx, |this, cx| {
                        if let Some(state) = this.dashboard_timelines.reveals.get_mut(&measure_key)
                        {
                            if state.observe_height(height) {
                                cx.notify();
                            }
                        }
                    });
                });
        div()
            .id(gpui::SharedString::from(format!("reveal-{key:?}")))
            .w_full()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(measured)
            .with_animation(
                format!("reveal-{key:?}-{revision}"),
                gpui::Animation::new(std::time::Duration::from_millis(MOTION_BASE_MS))
                    .with_easing(motion::ease_standard),
                move |element, t| {
                    let height = from + (to - from) * t;
                    painted.set(height);
                    element.max_h(gpui::px(height.max(0.)))
                },
            )
            .into_any_element()
    }
}
fn task_disclosure_glyph(open: bool) -> &'static str {
    if open {
        "chev-up"
    } else {
        "chev-down"
    }
}

fn task_key(task: &Task) -> String {
    let identity = if task.id.starts_with("step-") || task.id.starts_with("todo-") {
        &task.step
    } else {
        &task.id
    };
    format!("{}-{identity}", task.identity_at)
}

fn render_body(key: &str, data: &TimelineData, status: Status, cx: &App) -> AnyElement {
    let body_selector = format!("dashboard-timeline-body-{key}");
    div()
        .debug_selector(move || body_selector.clone())
        .w_full()
        .pl(rpx(SPACE_20))
        .py(rpx(SPACE_LG))
        .child(render_activity(key, &data.activity, status, cx))
        .into_any_element()
}

fn timeline_change<E: IntoElement + gpui::Styled + 'static>(
    element: E,
    key: String,
    cx: &App,
) -> AnyElement {
    motion::fast(element, format!("dashboard-change-{key}"), cx)
}
fn render_activity(key: &str, items: &[Activity], status: Status, cx: &gpui::App) -> AnyElement {
    render_activity_with_current(
        key,
        key,
        items,
        status,
        shimmering_activity(items, status),
        false,
        cx,
    )
}
fn render_activity_with_current(
    key: &str,
    group_key: &str,
    items: &[Activity],
    status: Status,
    current: Option<usize>,
    completed_task: bool,
    cx: &gpui::App,
) -> AnyElement {
    let selector = format!("dashboard-session-activity-{group_key}");
    let mut activity = div()
        .id(gpui::SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .role(gpui::Role::Group)
        .aria_label(if items.is_empty() {
            "Recent session activity. No activity updates yet."
        } else {
            "Recent session activity"
        })
        .mb(rpx(SPACE_LG))
        .flex()
        .flex_col()
        .gap(rpx(SPACE_SM))
        .text_size(rpx(TEXT_SMALL))
        .text_color(c::FG_DIM());
    if items.is_empty() {
        activity = activity.child(
            div()
                .pl(rpx(ICON_LG + SPACE_LG))
                .child("No activity updates yet."),
        );
    }
    let shimmer_index = (!completed_task).then_some(current).flatten();
    for (position, activity_item) in items.iter().enumerate() {
        let Activity {
            index,
            text: label,
            observed_active,
            ..
        } = activity_item;
        let identity = format!("{key}-{index}");
        let latest = position + 1 == items.len();
        let kind = if let Some(explicit) = activity_item.explicit_status {
            match explicit {
                grove_core::activity::StepStatus::Completed => {
                    super::super::status::StatusKind::Success
                }
                grove_core::activity::StepStatus::Failed => {
                    super::super::status::StatusKind::Failed
                }
                grove_core::activity::StepStatus::InProgress
                    if current == Some(*index)
                        && !completed_task
                        && *observed_active != Some(false) =>
                {
                    super::super::status::codex_kind(status)
                }
                grove_core::activity::StepStatus::InProgress => {
                    super::super::status::StatusKind::Stopped
                }
            }
        } else if completed_task {
            super::super::status::StatusKind::Idle
        } else if current == Some(*index) && *observed_active != Some(false) {
            super::super::status::codex_kind(status)
        } else {
            super::super::status::StatusKind::Idle
        };
        let text_selector = format!("dashboard-activity-label-{key}-{index}");
        let mut spine = div()
            .absolute()
            .left(rpx(ICON_LG / 2.))
            .ml(gpui::px(-TIMELINE_SPINE_W / 2.))
            .w(gpui::px(TIMELINE_SPINE_W))
            .bg(c::BORDER_SOFT())
            .top(rpx(if position == 0 {
                TIMELINE_TASK_LINE_H / 2.
            } else {
                0.
            }));
        spine = if latest {
            spine.h(rpx(TIMELINE_TASK_LINE_H / 2.))
        } else {
            spine.bottom(gpui::px(0.))
        };
        let leading = div()
            .relative()
            .w(rpx(ICON_LG))
            .flex_shrink_0()
            .when(items.len() > 1, |column| column.child(spine))
            .child(
                div()
                    .relative()
                    .w(rpx(ICON_LG))
                    .h(rpx(TIMELINE_TASK_LINE_H))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(rpx(ICON_SM))
                            .rounded(rpx(RADIUS_FULL))
                            .bg(c::BG())
                            .child(super::super::status::mark(
                                format!("activity-{identity}"),
                                kind,
                                ICON_SM,
                                cx,
                            )),
                    ),
            );
        activity = activity.child(
            div()
                .id(gpui::SharedString::from(format!(
                    "timeline-event-{identity}"
                )))
                .role(gpui::Role::Group)
                .aria_label(format!(
                    "{} · {label}",
                    if activity_item.explicit_status
                        == Some(grove_core::activity::StepStatus::Completed)
                    {
                        "Completed action"
                    } else if activity_item.explicit_status
                        == Some(grove_core::activity::StepStatus::Failed)
                    {
                        "Failed action"
                    } else if activity_item.explicit_status.is_some()
                        && (completed_task || *observed_active == Some(false))
                    {
                        "Stopped before reported completion"
                    } else if completed_task {
                        "Completed task activity"
                    } else if current == Some(*index) && *observed_active != Some(false) {
                        status.label()
                    } else {
                        "Earlier activity"
                    }
                ))
                .flex()
                .items_start()
                .gap(rpx(SPACE_LG))
                .line_height(rpx(TIMELINE_TASK_LINE_H))
                .child(leading)
                .child(timeline_change(
                    div()
                        .debug_selector(move || text_selector.clone())
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .child(super::super::components::shimmer_label(
                            gpui::SharedString::from(format!("dashboard-shimmer-{identity}"))
                                .into(),
                            label.clone(),
                            shimmer_index == Some(*index),
                            cx,
                        )),
                    identity,
                    cx,
                )),
        );
    }
    activity.into_any_element()
}

#[cfg(test)]
mod tests;
