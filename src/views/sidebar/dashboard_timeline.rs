//! Dashboard progress derived from cached structured-session facts; never provider guesses.
use super::*;
use gpui::AnimationExt as _;
use grove_core::{codex::Status, tasks::TaskList};
use serde_json::Value;
use std::sync::Arc;

const TIMELINE_TASK_LINE_H: f32 = super::SESSION_TITLE_LINE_H;
const TIMELINE_SPINE_W: f32 = 1.;
const ACTIVITY_LIMIT: usize = 3;
const ACTIVITY_LOOKBACK: usize = 64;
const ACTIVITY_TEXT_LIMIT: usize = 240;
pub(super) struct RowMetadata {
    pub project: String,
    pub worktree: String,
    pub provider: grove_core::codex::Provider,
    pub effort: Option<String>,
    pub context: String,
}
impl RowMetadata {
    fn provider_label(&self) -> String {
        let provider = self.provider.label();
        self.effort
            .as_deref()
            .map(str::trim)
            .filter(|effort| !effort.is_empty())
            .map_or_else(
                || provider.to_owned(),
                |effort| {
                    let effort = if effort == "xhigh" {
                        "Extra high".into()
                    } else {
                        let mut letters = effort.chars();
                        letters.next().map_or_else(String::new, |first| {
                            first.to_uppercase().chain(letters).collect()
                        })
                    };
                    format!("{provider} {effort}")
                },
            )
    }
}
fn metadata_group(glyph: &str, text: String) -> AnyElement {
    div()
        .id(gpui::SharedString::from(format!(
            "dashboard-meta-{glyph}-{text}"
        )))
        .role(gpui::Role::Group)
        .aria_label(text.clone())
        .min_w_0()
        .max_w(gpui::relative(1.))
        .flex()
        .items_center()
        .gap(rpx(SPACE_SM))
        .child(icon(glyph, ICON_SM, c::FG_DIM()).flex_shrink_0())
        .child(div().min_w_0().truncate().child(text))
        .into_any_element()
}
fn render_metadata(metadata: &RowMetadata) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .gap_x(rpx(SPACE_2XL))
        .gap_y(rpx(SPACE_SM))
        .text_size(rpx(TEXT_SMALL))
        .line_height(rpx(SESSION_META_LINE_H))
        .text_color(c::FG_DIM())
        .child(metadata_group("folder", metadata.project.clone()))
        .child(metadata_group("git-branch", metadata.worktree.clone()))
        .child(metadata_group(
            metadata.provider.agent().icon_name(),
            metadata.provider_label(),
        ))
        .into_any_element()
}
type Scope = (u64, SessionId);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Activity {
    pub index: usize,
    pub text: String,
    pub observed_active: Option<bool>,
}
#[derive(Clone, Default, Debug, PartialEq)]
pub(super) struct TimelineData {
    pub list: Option<Arc<TaskList>>,
    pub activity: Vec<Activity>,
}
/// Bounded excerpts retain transcript item identity without cloning transcript payloads.
pub(super) fn recent_activity<T: std::borrow::Borrow<Value>>(items: &[T]) -> Vec<Activity> {
    items
        .iter()
        .enumerate()
        .rev()
        .take(ACTIVITY_LOOKBACK)
        .filter_map(|(index, item)| {
            let item = item.borrow();
            let text = match item["type"].as_str()? {
                "agentMessage" => item["text"].as_str()?,
                "commandExecution" => item["command"].as_str()?,
                "mcpToolCall" => item["tool"].as_str()?,
                "fileChange" => item["changes"].as_array()?.first()?["path"].as_str()?,
                "webSearch" => item["action"]["query"].as_str()?,
                _ => return None,
            };
            let text = text
                .trim()
                .chars()
                .take(ACTIVITY_TEXT_LIMIT)
                .collect::<String>();
            (!text.is_empty()).then_some(Activity {
                index,
                text,
                observed_active: match item["status"].as_str() {
                    Some("inProgress" | "in_progress" | "running") => Some(true),
                    Some("completed" | "failed" | "interrupted" | "cancelled" | "canceled") => {
                        Some(false)
                    }
                    _ => None,
                },
            })
        })
        .take(ACTIVITY_LIMIT)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

#[derive(Default)]
pub(super) struct DashboardTimelines {
    activity_scope: Option<(u64, Vec<SessionId>)>,
    #[cfg(test)]
    warm_requests: usize,
    opened: HashSet<Scope>,
    focus: HashMap<Scope, FocusHandle>,
    open_focus: HashMap<Scope, FocusHandle>,
    reveals: HashMap<Scope, TimelineReveal>,
}
impl DashboardTimelines {
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
            self.invalidate_scope();
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
    pub fn is_open(&self, workspace: u64, id: SessionId) -> bool {
        self.opened.contains(&(workspace, id))
    }
    fn toggle(&mut self, key: Scope) {
        if !self.opened.remove(&key) {
            self.opened.insert(key);
        }
    }
    pub fn retain(&mut self, sessions: &HashSet<SessionId>, workspaces: &HashSet<u64>) {
        let keep = |key: &Scope| sessions.contains(&key.1) && workspaces.contains(&key.0);
        self.opened.retain(keep);
        self.focus.retain(|key, _| keep(key));
        self.open_focus.retain(|key, _| keep(key));
        self.reveals.retain(|key, _| keep(key));
    }
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
    pub(super) fn dashboard_owns_session(&self, id: SessionId) -> bool {
        self.snapshot
            .projects
            .iter()
            .any(|project| project.sessions.contains(&id))
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
            .filter(|id| self.dashboard_owns_session(*id))
            .collect();
        if self.dashboard_timelines.needs_activity_warm(
            self.active_workspace,
            self.dashboard_open,
            owned,
        ) {
            self.refresh_dashboard_timelines(cx);
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
                if !self.dashboard_owns_session(meta.id) {
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
            cache.refresh(&state.saved.items);
            cache.refresh_dashboard(&state.saved.items, state.status);
        }
    }
    pub(super) fn render_dashboard_progress(
        &mut self,
        id: SessionId,
        title: &str,
        status: Status,
        age: &str,
        metadata: &RowMetadata,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = (self.active_workspace, id);
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
        let accessible = format!("Progress · {}", metadata.context);
        let tooltip = accessible.clone();
        let toggle = div()
            .id(("dashboard-progress-toggle", id.raw()))
            .debug_selector(move || format!("dashboard-progress-toggle-{}", id.raw()))
            .role(gpui::Role::Button)
            .aria_label(accessible)
            .tooltip(move |window, cx| {
                super::super::components::tooltip(tooltip.clone(), window).build(window, cx)
            })
            .aria_expanded(open)
            .tab_index(0)
            .tab_stop(true)
            .track_focus(&focus)
            .flex_1()
            .min_w_0()
            .cursor_pointer()
            .flex()
            .items_start()
            .gap(rpx(SPACE_LG))
            .line_height(rpx(TIMELINE_TASK_LINE_H))
            .py(rpx(SPACE_SM))
            .hover(|style| style.bg(c::BG_HOVER()))
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
                        c::FG_DIM(),
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
                                    .child(single_line_session_title(title)),
                            )
                            .child(
                                super::super::status::label(
                                    format!("dashboard-session-{}", id.raw()),
                                    super::super::status::codex_kind(status),
                                    status.label(),
                                    TEXT_SMALL,
                                    cx,
                                )
                                .flex_shrink_0()
                                .text_size(rpx(TEXT_SMALL)),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child(age.to_owned()),
                            ),
                    )
                    .child(render_metadata(metadata)),
            );
        let open_button = super::super::components::header_control_frame(
            ("dashboard-open-session", id.raw()),
            format!("Open session · {}", metadata.context),
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
        .tooltip(|window, cx| {
            super::super::components::tooltip("Open session", window).build(window, cx)
        })
        .child(icon("arrow-right", ICON_SM, c::FG_DIM()));
        let retained = self
            .dashboard_timelines
            .reveals
            .get(&key)
            .is_some_and(|state| state.visible || state.exiting);
        let body = if open || retained {
            render_body(&format!("{}-{}", key.0, id.raw()), &data, status, cx)
        } else {
            div().into_any_element()
        };
        let detail = self.dashboard_progress_reveal(key, open, body, cx);
        div()
            .id(("dashboard-progress", id.raw()))
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
                    .child(open_button),
            )
            .child(detail)
            .into_any_element()
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
fn render_body(key: &str, data: &TimelineData, status: Status, cx: &App) -> AnyElement {
    use crate::views::components::task_mark;
    use grove_core::tasks::TaskStatus;
    let body_selector = format!("dashboard-timeline-body-{key}");
    let mut body = div()
        .debug_selector(move || body_selector.clone())
        .w_full()
        .pl(rpx(SPACE_20))
        .py(rpx(SPACE_LG))
        .flex()
        .flex_col();
    if let Some(list) = data.list.as_deref() {
        let activity_anchor = list.current().map(|task| task.id.as_str());
        for (index, task) in list.tasks.iter().enumerate() {
            let current = task.status == TaskStatus::InProgress;
            let label = match task.status {
                TaskStatus::Completed => "Done",
                TaskStatus::InProgress => match status {
                    Status::Starting | Status::Working => "Working",
                    Status::Waiting => "Waiting",
                    Status::Failed | Status::Disconnected | Status::Interrupted => "Stopped",
                    Status::Idle | Status::Done => "Last active",
                },
                TaskStatus::Pending => "Pending",
            };
            let mark = if current && matches!(status, Status::Starting | Status::Working) {
                super::super::status::mark(
                    format!("timeline-{key}-{}", task.id),
                    super::super::status::codex_kind(status),
                    ICON_SM,
                    cx,
                )
            } else {
                task_mark(task.status, super::task_glance::task_phase(status), 0).into_any_element()
            };
            let mut spine = div()
                .absolute()
                .left(rpx(ICON_LG / 2.))
                .ml(gpui::px(-TIMELINE_SPINE_W / 2.))
                .w(gpui::px(TIMELINE_SPINE_W))
                .bg(c::BORDER_SOFT());
            spine = spine.top(rpx(if index == 0 {
                TIMELINE_TASK_LINE_H / 2.
            } else {
                0.
            }));
            spine = if index + 1 == list.tasks.len() {
                spine.h(rpx(TIMELINE_TASK_LINE_H / 2.))
            } else {
                spine.bottom(gpui::px(0.))
            };
            let leading = div()
                .relative()
                .w(rpx(ICON_LG))
                .flex_shrink_0()
                .when(list.tasks.len() > 1, |column| column.child(spine))
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
                                .child(mark),
                        ),
                );
            let label_selector = format!("dashboard-task-label-{key}-{}", task.id);
            let row = div()
                .min_h(rpx(TIMELINE_TASK_LINE_H))
                .line_height(rpx(TIMELINE_TASK_LINE_H))
                .flex()
                .items_start()
                .gap(rpx(SPACE_LG))
                .child(
                    div()
                        .debug_selector(move || label_selector.clone())
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .text_size(rpx(TEXT_BODY))
                        .child(task.step.clone()),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(label),
                );
            let detail = if activity_anchor == Some(task.id.as_str()) {
                render_activity(key, &data.activity, status, cx)
            } else {
                div().into_any_element()
            };
            let contents = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(timeline_change(
                    row,
                    format!("task-{key}-{}-{}", task.id, task.status.as_str()),
                    cx,
                ))
                .child(detail)
                .when(index + 1 < list.tasks.len(), |contents| {
                    contents.pb(rpx(SPACE_XL))
                });
            body = body.child(
                div()
                    .id(gpui::SharedString::from(format!(
                        "timeline-task-{key}-{}",
                        task.id
                    )))
                    .role(gpui::Role::Group)
                    .aria_label(format!("{label} · {}", task.step))
                    .w_full()
                    .flex()
                    .gap(rpx(SPACE_LG))
                    .child(leading)
                    .child(contents),
            );
        }
        if list.current().is_none() {
            body = body.child(div().pl(rpx(ICON_LG + SPACE_LG)).child(render_activity(
                key,
                &data.activity,
                status,
                cx,
            )));
        }
    } else {
        let fallback_selector = format!("dashboard-no-plan-{key}");
        body = body
            .child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .id(gpui::SharedString::from(fallback_selector.clone()))
                    .role(gpui::Role::Group)
                    .aria_label("No plan published yet.")
                    .debug_selector(move || fallback_selector.clone())
                    .child("No plan published yet."),
            )
            .child(render_activity(key, &data.activity, status, cx));
    }
    body.into_any_element()
}

fn timeline_change<E: IntoElement + gpui::Styled + 'static>(
    element: E,
    key: String,
    cx: &App,
) -> AnyElement {
    motion::fast(element, format!("dashboard-change-{key}"), cx)
}
fn shimmering_activity(items: &[Activity], status: Status) -> Option<usize> {
    let latest = items.last()?;
    (matches!(status, Status::Starting | Status::Working) && latest.observed_active != Some(false))
        .then_some(latest.index)
}
fn render_activity(key: &str, items: &[Activity], status: Status, cx: &gpui::App) -> AnyElement {
    let selector = format!("dashboard-session-activity-{key}");
    let mut activity = div()
        .id(gpui::SharedString::from(selector.clone()))
        .debug_selector(move || selector.clone())
        .role(gpui::Role::Group)
        .aria_label(if items.is_empty() {
            "Recent session activity. No recent activity yet."
        } else {
            "Recent session activity"
        })
        .mb(rpx(SPACE_LG))
        .flex()
        .flex_col()
        .gap(rpx(SPACE_SM))
        .text_size(rpx(TEXT_SMALL))
        .text_color(c::FG_DIM());
    let labels = items;
    if labels.is_empty() {
        activity = activity.child("No recent activity yet.");
    }
    let shimmer_index = shimmering_activity(labels, status);
    for activity_item in labels {
        let Activity {
            index,
            text: label,
            observed_active: _,
        } = activity_item;
        let identity = format!("{key}-{index}");
        let shimmer = shimmer_index == Some(*index);
        let text_selector = format!("dashboard-activity-label-{key}-{index}");
        activity = activity.child(timeline_change(
            div()
                .id(gpui::SharedString::from(format!(
                    "timeline-event-{identity}"
                )))
                .debug_selector(move || text_selector.clone())
                .role(gpui::Role::Group)
                .aria_label(label.clone())
                .whitespace_normal()
                .child(super::super::components::shimmer_label(
                    gpui::SharedString::from(format!("dashboard-shimmer-{identity}")).into(),
                    label.clone(),
                    shimmer,
                    cx,
                )),
            identity,
            cx,
        ));
    }
    activity.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recent_activity_is_bounded_and_preserves_observed_identity() {
        let items = vec![
            json!({"type":"agentMessage","text":"older"}),
            json!({"type":"commandExecution","command":"x".repeat(100_000)}),
            json!({"type":"reasoning","text":"private reasoning"}),
            json!({"type":"mcpToolCall","tool":"read_file"}),
            json!({"type":"agentMessage","text":"Observed update"}),
        ];
        let activity = recent_activity(&items);
        assert_eq!(
            activity.iter().map(|entry| entry.index).collect::<Vec<_>>(),
            vec![1, 3, 4]
        );
        assert_eq!(activity[0].text.chars().count(), ACTIVITY_TEXT_LIMIT);
        assert_eq!(activity[1].text, "read_file");
        assert!(!activity.iter().any(|entry| entry.text.contains("private")));
        let mut old = vec![json!({"type":"agentMessage","text":"old"})];
        old.extend((0..ACTIVITY_LOOKBACK).map(|_| json!({"type":"reasoning"})));
        assert!(recent_activity(&old).is_empty());
    }
    #[test]
    fn dashboard_disclosure_scope_is_workspace_and_session_owned() {
        let first = SessionId::from_raw(1);
        let second = SessionId::from_raw(2);
        let mut state = DashboardTimelines::default();
        state.toggle((1, first));
        state.toggle((2, first));
        state.toggle((1, second));
        state.reveals.insert((1, second), TimelineReveal::default());
        assert!(!state.is_open(3, first));
        state.toggle((1, first));
        assert!(state.is_open(2, first));
        assert!(!state.is_open(1, first));
        state.retain(&HashSet::from([first]), &HashSet::from([1, 2]));
        assert!(!state.is_open(1, second));
        assert!(state.reveals.is_empty());
        state.retain(&HashSet::from([first]), &HashSet::from([1]));
        assert!(!state.is_open(2, first));
    }
    #[test]
    fn reveal_reversal_uses_painted_height_and_stable_measurements_do_not_restart() {
        let mut state = TimelineReveal {
            visible: true,
            ..Default::default()
        };
        assert!(state.observe_height(214.));
        let revision = state.revision;
        for _ in 0..30 {
            assert!(!state.observe_height(214.));
        }
        assert_eq!(state.revision, revision);
        state.painted.set(121.28);
        state.target(0.);
        assert_eq!(state.from, 121.28);
        assert_eq!(state.to, 0.);
        state.target(214.);
        assert_eq!(state.from, 121.28);
        assert_eq!(state.to, 214.);
    }
    struct BodyProbe(TimelineData);
    impl gpui::Render for BodyProbe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(gpui::px(440.))
                .child(render_body("probe", &self.0, Status::Working, cx))
        }
    }
    #[gpui::test]
    fn no_plan_and_completed_plan_keep_real_session_activity_visible(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_reduce_motion(true);
        });
        for complete in [false, true] {
            let items = vec![
                json!({"type":"plan","plan":[{"step":"Completed task","status":"completed"}]}),
                json!({"type":"agentMessage","text":"Work continues after the published plan"}),
            ];
            let data = TimelineData {
                list: complete.then(|| Arc::new(grove_core::tasks::current(&items).unwrap())),
                activity: recent_activity(&items),
            };
            let (_, cx) = cx.add_window_view(|_, _| BodyProbe(data));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(
                cx.debug_bounds("dashboard-session-activity-probe")
                    .is_some(),
                "no-plan and stale completed plans must keep activity visible"
            );
            assert_eq!(
                cx.debug_bounds("dashboard-no-plan-probe").is_some(),
                !complete
            );
        }
    }
    #[gpui::test]
    fn multiple_active_tasks_show_session_activity_once(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_reduce_motion(true);
        });
        let mut heights = Vec::new();
        for first_status in ["completed", "inProgress"] {
            let items = vec![
                json!({"type":"plan","plan":[{"step":"First","status":first_status},{"step":"Second","status":"inProgress"}]}),
                json!({"type":"agentMessage","text":"One observed session activity"}),
            ];
            let data = TimelineData {
                list: Some(Arc::new(grove_core::tasks::current(&items).unwrap())),
                activity: recent_activity(&items),
            };
            let (_, cx) = cx.add_window_view(|_, _| BodyProbe(data));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            heights.push(f32::from(
                cx.debug_bounds("dashboard-timeline-body-probe")
                    .unwrap()
                    .size
                    .height,
            ));
        }
        assert!((heights[0]-heights[1]).abs()<1.,"multiple active tasks must not duplicate the session-wide activity section: {heights:?}");
    }
    #[test]
    fn activity_scope_warms_only_on_entry_membership_or_workspace_change() {
        let a = SessionId::from_raw(1);
        let b = SessionId::from_raw(2);
        let mut state = DashboardTimelines::default();
        assert!(state.needs_activity_warm(1, true, vec![a, b]));
        for _ in 0..60 {
            assert!(!state.needs_activity_warm(1, true, vec![a, b]));
        }
        assert!(
            !state.needs_activity_warm(1, true, vec![b, a, a]),
            "metadata order/duplicates must not rescan transcripts"
        );
        assert!(
            state.needs_activity_warm(1, true, vec![a]),
            "removed membership changes scope"
        );
        assert!(
            state.needs_activity_warm(1, true, vec![a, b]),
            "newly owned existing sessions need a warm"
        );
        assert!(
            state.needs_activity_warm(2, true, vec![a, b]),
            "workspace switch must not reuse another scope"
        );
        assert!(!state.needs_activity_warm(2, false, vec![a, b]));
        assert!(
            state.needs_activity_warm(2, true, vec![a, b]),
            "reopen must refresh updates missed while hidden"
        );
    }
    #[test]
    fn only_newest_eligible_activity_shimmers_while_session_is_working() {
        let mut activity = recent_activity(&[
            json!({"type":"commandExecution","command":"first","status":"inProgress"}),
            json!({"type":"agentMessage","text":"Latest streamed text"}),
        ]);
        assert_eq!(shimmering_activity(&activity, Status::Working), Some(1));
        assert_eq!(shimmering_activity(&activity, Status::Starting), Some(1));
        for status in [
            Status::Waiting,
            Status::Done,
            Status::Idle,
            Status::Failed,
            Status::Disconnected,
            Status::Interrupted,
        ] {
            assert_eq!(shimmering_activity(&activity, status), None);
        }
        activity[1].observed_active = Some(false);
        assert_eq!(
            shimmering_activity(&activity, Status::Working),
            None,
            "completed newest item must not move shimmer onto older items"
        );
        let revised = recent_activity(&[
            json!({"type":"commandExecution","command":"first","status":"inProgress"}),
            json!({"type":"agentMessage","text":"Latest streamed text has progressed"}),
        ]);
        assert_eq!(
            activity[1].index, revised[1].index,
            "stream revisions preserve source identity"
        );
    }
    #[gpui::test]
    fn unchanged_sidebar_sync_does_not_request_transcript_refresh(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::settings::SettingsState::new(
                grove_core::storage::Store::default(),
            ));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.dashboard_open = true;
                sidebar.sync_task_glances(cx);
                let baseline = sidebar.dashboard_timelines.warm_requests;
                for _ in 0..60 {
                    sidebar.sync_task_glances(cx);
                }
                assert_eq!(
                    sidebar.dashboard_timelines.warm_requests, baseline,
                    "unchanged render-time sync must not request saved.items reads"
                );
                sidebar.dashboard_open = false;
                sidebar.sync_task_glances(cx);
                sidebar.dashboard_open = true;
                sidebar.sync_task_glances(cx);
                assert_eq!(
                    sidebar.dashboard_timelines.warm_requests,
                    baseline + 1,
                    "reopen refreshes activity missed while hidden"
                );
            });
        });
    }
    #[test]
    fn metadata_preserves_provider_and_formats_only_actual_effort() {
        let mut metadata = RowMetadata {
            project: "Actual project".into(),
            worktree: sidebar_worktree_name("root", true).into(),
            provider: grove_core::codex::Provider::Codex,
            effort: None,
            context: String::new(),
        };
        assert_eq!(metadata.worktree, "Main checkout");
        assert_eq!(metadata.provider_label(), "Codex");
        metadata.effort = Some("xhigh".into());
        assert_eq!(metadata.provider_label(), "Codex Extra high");
        metadata.provider = grove_core::codex::Provider::Claude;
        metadata.effort = Some("high".into());
        assert_eq!(metadata.provider_label(), "Claude Code High");
        metadata.effort = Some(String::new());
        assert_eq!(metadata.provider_label(), "Claude Code");
    }
    #[test]
    fn activity_contains_only_provider_supplied_content() {
        let items = vec![
            json!({"type":"commandExecution","command":"pwd"}),
            json!({"type":"fileChange","changes":[{"path":"src/main.rs"}]}),
            json!({"type":"webSearch","action":{"query":"actual supplied query"}}),
            json!({"type":"fileChange","changes":[]}),
        ];
        assert_eq!(
            recent_activity(&items)
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["pwd", "src/main.rs", "actual supplied query"]
        );
    }
    #[gpui::test]
    fn activity_text_shares_task_column_for_current_completed_and_missing_plans(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_reduce_motion(true);
        });
        for rem in [crate::zoom::REM_BASE, 20.] {
            for scenario in ["current", "completed", "missing"] {
                let items = vec![
                    json!({"type":"plan","plan":[{"id":"first","step":"Inspect","status":if scenario=="current" {"inProgress"}else{"completed"}},{"id":"second","step":"Finish","status":"completed"}]}),
                    json!({"type":"agentMessage","text":"Actual supplied progress"}),
                ];
                let data = TimelineData {
                    list: (scenario != "missing")
                        .then(|| Arc::new(grove_core::tasks::current(&items).unwrap())),
                    activity: recent_activity(&items),
                };
                let (_, cx) = cx.add_window_view(|_, _| BodyProbe(data));
                cx.run_until_parked();
                cx.update(|window, cx| {
                    window.set_rem_size(gpui::px(rem));
                    let _ = window.draw(cx);
                });
                let reference = if scenario == "missing" {
                    "dashboard-no-plan-probe"
                } else {
                    "dashboard-task-label-probe-first"
                };
                let expected = cx.debug_bounds(reference).unwrap().left();
                let actual = cx
                    .debug_bounds("dashboard-activity-label-probe-1")
                    .unwrap()
                    .left();
                assert!((f32::from(expected)-f32::from(actual)).abs()<0.5,"{scenario} activity must align with its source text column at rem{rem}: expected{expected:?}, actual{actual:?}");
            }
        }
    }
}
