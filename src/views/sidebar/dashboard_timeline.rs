//! Dashboard progress derived from cached structured-session facts; never provider guesses.
use super::*;
use gpui::AnimationExt as _;
use grove_core::{codex::Status, tasks::TaskList};
use serde_json::Value;
use std::sync::Arc;

const TIMELINE_TASK_LINE_H: f32 = super::SESSION_TITLE_LINE_H;
const TIMELINE_SPINE_W: f32 = 1.;
const ACTIVITY_LIMIT: usize = 3;
const ACTIVITY_HISTORY_LIMIT: usize = 64;
const ACTIVITY_TEXT_LIMIT: usize = 120;
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
/// A short excerpt of the provider's own progress prose, never a command or reasoning.
fn activity_excerpt(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    if line.starts_with("```") {
        return None;
    }
    let line = line.trim_start_matches(['#', '*', '-', ' ']);
    let sentence_end = line.char_indices().find_map(|(index, ch)| {
        (matches!(ch, '.' | '!' | '?')
            && line[index + ch.len_utf8()..].starts_with(char::is_whitespace))
        .then_some(index + ch.len_utf8())
    });
    let sentence = &line[..sentence_end.unwrap_or(line.len())];
    let mut excerpt = sentence
        .chars()
        .take(ACTIVITY_TEXT_LIMIT)
        .collect::<String>();
    if sentence.chars().count() > ACTIVITY_TEXT_LIMIT {
        if let Some(space) = excerpt.rfind(' ') {
            excerpt.truncate(space);
        }
        excerpt.push('…');
    }
    (!excerpt.is_empty()).then_some(excerpt)
}
/// History is bounded by summaries, so a long tool phase cannot erase its current step.
pub(super) fn recent_activity<T: std::borrow::Borrow<Value>>(items: &[T]) -> Vec<Activity> {
    let latest_prompt = items
        .iter()
        .rposition(|item| item.borrow()["type"] == "userMessage");
    items
        .iter()
        .enumerate()
        .rev()
        .filter_map(|(index, item)| {
            let item = item.borrow();
            if item["type"] != "agentMessage" {
                return None;
            }
            let text = activity_excerpt(item["text"].as_str()?)?;
            Some(Activity {
                index,
                text,
                // Finishing a text stream does not finish the work described by it.
                // A final answer or a later user prompt does end that activity.
                observed_active: (item["phase"] == "final_answer"
                    || latest_prompt.is_some_and(|prompt| index < prompt))
                .then_some(false),
            })
        })
        .take(ACTIVITY_HISTORY_LIMIT)
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
    pub(super) fn toggle(&mut self, key: Scope) {
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
        let accessible = format!("Activity history · {}", metadata.context);
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
        let body = render_body(&format!("{}-{}", key.0, id.raw()), &data, status, open, cx);
        let detail = self.dashboard_progress_reveal(key, true, body, cx);
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
fn render_body(
    key: &str,
    data: &TimelineData,
    status: Status,
    expanded: bool,
    cx: &App,
) -> AnyElement {
    let body_selector = format!("dashboard-timeline-body-{key}");
    let start = if expanded {
        0
    } else {
        data.activity.len().saturating_sub(ACTIVITY_LIMIT)
    };
    div()
        .debug_selector(move || body_selector.clone())
        .w_full()
        .pl(rpx(SPACE_20))
        .py(rpx(SPACE_LG))
        .child(render_activity(key, &data.activity[start..], status, cx))
        .into_any_element()
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
    let shimmer_index = shimmering_activity(items, status);
    for (position, activity_item) in items.iter().enumerate() {
        let Activity {
            index,
            text: label,
            observed_active,
        } = activity_item;
        let identity = format!("{key}-{index}");
        let latest = position + 1 == items.len();
        let kind = if latest && *observed_active != Some(false) {
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
                    if latest && *observed_active != Some(false) {
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
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recent_activity_groups_tools_under_short_provider_updates() {
        let mut items = vec![
            json!({"type":"agentMessage","text":"Inspecting hover styling. More detail is in the conversation."}),
            json!({"type":"commandExecution","command":"cat DESIGN.md"}),
            json!({"type":"reasoning","text":"private reasoning"}),
            json!({"type":"mcpToolCall","tool":"read_file"}),
            json!({"type":"agentMessage","text":"Removing the highlight\nMore detail"}),
        ];
        items.extend((0..1000).map(|_| json!({"type":"commandExecution","command":"raw command"})));
        let activity = recent_activity(&items);
        assert_eq!(
            activity.iter().map(|entry| entry.index).collect::<Vec<_>>(),
            vec![0, 4]
        );
        assert_eq!(activity[0].text, "Inspecting hover styling.");
        assert_eq!(activity[1].text, "Removing the highlight");
        assert_eq!(shimmering_activity(&activity, Status::Working), Some(4));
        assert!(recent_activity(&[json!({"type":"commandExecution","command":"pwd"})]).is_empty());
        assert!(activity_excerpt("```sh\npwd").is_none());
        assert!(
            activity_excerpt(&"界".repeat(1000))
                .unwrap()
                .chars()
                .count()
                <= ACTIVITY_TEXT_LIMIT + 1
        );
        let history = (0..100)
            .map(|n| json!({"type":"agentMessage","text":format!("Step {n}")}))
            .collect::<Vec<_>>();
        assert_eq!(recent_activity(&history).len(), ACTIVITY_HISTORY_LIMIT);
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
                .child(render_body("probe", &self.0, Status::Working, false, cx))
        }
    }
    #[gpui::test]
    fn compact_activity_shows_three_steps_without_task_checklist(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_reduce_motion(true);
        });
        let items = vec![
            json!({"type":"plan","plan":[{"step":"Planned task","status":"inProgress"}]}),
            json!({"type":"agentMessage","text":"First step"}),
            json!({"type":"agentMessage","text":"Second step"}),
            json!({"type":"agentMessage","text":"Third step"}),
            json!({"type":"agentMessage","text":"Current step"}),
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
        assert!(cx
            .debug_bounds("dashboard-activity-label-probe-1")
            .is_none());
        for selector in [
            "dashboard-activity-label-probe-2",
            "dashboard-activity-label-probe-3",
            "dashboard-activity-label-probe-4",
        ] {
            assert!(cx.debug_bounds(selector).is_some());
        }
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
            json!({"type":"agentMessage","text":"First update"}),
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
            json!({"type":"agentMessage","text":"First update"}),
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
    fn prior_turn_and_final_answers_never_shimmer_as_current_work() {
        for items in [
            vec![json!({"type":"agentMessage","text":"Final outcome","phase":"final_answer"})],
            vec![
                json!({"type":"agentMessage","text":"Prior step"}),
                json!({"type":"userMessage"}),
                json!({"type":"commandExecution","command":"pwd"}),
            ],
        ] {
            let activity = recent_activity(&items);
            assert_eq!(shimmering_activity(&activity, Status::Working), None);
        }
    }
    #[gpui::test]
    fn expanded_activity_history_preserves_columns_at_narrow_width_and_zoom(
        cx: &mut gpui::TestAppContext,
    ) {
        struct HistoryProbe(TimelineData);
        impl gpui::Render for HistoryProbe {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .w_full()
                    .child(render_body("history", &self.0, Status::Waiting, true, cx))
            }
        }
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_reduce_motion(true);
        });
        let items = (0..5).map(|n| json!({"type":"agentMessage","text":format!("Observed step {n} with a longer description that wraps in narrow layouts")})).collect::<Vec<_>>();
        let (_, cx) = cx.add_window_view(|_, _| {
            HistoryProbe(TimelineData {
                list: None,
                activity: recent_activity(&items),
            })
        });
        for width in [440., 240.] {
            for rem in [crate::zoom::REM_BASE, 20.] {
                cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(700.)));
                cx.run_until_parked();
                cx.update(|window, cx| {
                    window.set_rem_size(gpui::px(rem));
                    let _ = window.draw(cx);
                });
                let first = cx
                    .debug_bounds("dashboard-activity-label-history-0")
                    .unwrap();
                for selector in [
                    "dashboard-activity-label-history-1",
                    "dashboard-activity-label-history-2",
                    "dashboard-activity-label-history-3",
                    "dashboard-activity-label-history-4",
                ] {
                    let label = cx.debug_bounds(selector).unwrap();
                    assert_eq!(first.left(), label.left());
                    assert!(label.right() <= gpui::px(width));
                }
            }
        }
    }
}
