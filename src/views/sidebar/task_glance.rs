//! Cross-session task progress: the sidebar row's done/total cell and the
//! shared main/grid header's compact lifecycle control and task disclosure.
//!
//! Progress is derived from a structured session's saved transcript items.
//! [`TaskGlanceCache`] runs from a per-session observer (not from render), so
//! compact headers read the cached [`TaskGlance`]; an open disclosure reads
//! the current full checklist.

use super::{Sidebar, HIERARCHY_META_TEXT, SESSION_META_LINE_H};
use crate::{
    entities::session_registry::SessionId,
    theme as c,
    views::{
        components::{task_mark, task_meter, TaskPhase},
        rpx,
        tokens::*,
    },
};
use gpui::{div, prelude::*, AnyElement, App, Context, FocusHandle, Window};
use gpui_component::ElementExt as _;
use grove_core::{
    codex::Status,
    tasks::{self, TaskList, TaskStatus},
};
use serde_json::Value;
use std::{cell::Cell, collections::HashMap, rc::Rc};

/// The summary of a session's current task list that the sidebar and grid
/// headers render.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TaskGlance {
    pub done: usize,
    pub total: usize,
    /// Status of the first in-progress task, if any.
    pub current: Option<TaskStatus>,
    /// The current task's active form, else its step.
    pub now: Option<String>,
}

impl TaskGlance {
    pub fn from_list(list: &TaskList) -> Self {
        Self {
            done: list.done(),
            total: list.total(),
            current: list.current().map(|task| task.status),
            now: list.now_label().map(str::to_owned),
        }
    }

    pub fn all_done(&self) -> bool {
        self.total > 0 && self.done >= self.total
    }

    /// Compact "3/7".
    pub fn count_label(&self) -> String {
        format!("{}/{}", self.done, self.total)
    }

    /// Appended to the row tooltip: "Rendering task strip · 3 of 7 tasks done".
    pub fn tooltip_suffix(&self) -> String {
        let count = format!("{} of {} tasks done", self.done, self.total);
        match self.now.as_deref().filter(|_| !self.all_done()) {
            Some(now) => format!("{now} · {count}"),
            None => count,
        }
    }
}

/// How the current task's mark reads for a session in `status`.
pub(super) fn task_phase(status: Status) -> TaskPhase {
    match status {
        Status::Starting | Status::Working => TaskPhase::Running,
        Status::Waiting => TaskPhase::Waiting,
        Status::Interrupted | Status::Failed | Status::Disconnected => TaskPhase::Halted,
        Status::Idle | Status::Done => TaskPhase::Stale,
    }
}

/// Memoises [`tasks::current`] for one session. Only a changed checklist
/// snapshot re-parses the list; continuation messages retain the summary.
#[derive(Default)]
pub(super) struct TaskGlanceCache {
    index: Option<usize>,
    item: Value,
    glance: Option<TaskGlance>,
    list: Option<std::sync::Arc<TaskList>>,
    timeline: std::sync::Arc<super::dashboard_timeline::TimelineData>,
    dashboard_status: Option<Status>,
}

impl TaskGlanceCache {
    pub fn timeline(&self) -> std::sync::Arc<super::dashboard_timeline::TimelineData> {
        self.timeline.clone()
    }
    pub fn refresh_dashboard<T: std::borrow::Borrow<Value>>(
        &mut self,
        items: &[T],
        status: Status,
    ) -> bool {
        let activity = super::dashboard_timeline::recent_activity(items);
        let changed = activity != self.timeline.activity || self.dashboard_status != Some(status);
        if activity != self.timeline.activity {
            self.timeline = std::sync::Arc::new(super::dashboard_timeline::TimelineData {
                list: self.list.clone(),
                activity,
            });
        }
        self.dashboard_status = Some(status);
        changed
    }
    pub fn glance(&self) -> Option<&TaskGlance> {
        self.glance.as_ref()
    }

    /// Refreshes from `items`; returns whether checklist content or its summary changed.
    pub fn refresh<T: std::borrow::Borrow<Value>>(&mut self, items: &[T]) -> bool {
        let index = items
            .iter()
            .rposition(|item| tasks::is_checklist(item.borrow()));
        let changed =
            self.index != index || index.is_some_and(|at| self.item != *items[at].borrow());
        if changed {
            self.item = index.map_or(Value::Null, |at| items[at].borrow().clone());
            self.list = tasks::current(items).map(std::sync::Arc::new);
            self.glance = self.list.as_deref().map(TaskGlance::from_list);
            self.timeline = std::sync::Arc::new(super::dashboard_timeline::TimelineData {
                list: self.list.clone(),
                activity: self.timeline.activity.clone(),
            });
        }
        self.index = index;
        changed
    }
}

impl Sidebar {
    /// Keeps one observer per structured session so task progress repaints
    /// the sidebar only when the summary changes.
    pub(super) fn sync_task_glances(&mut self, cx: &mut Context<Self>) {
        let registry = self.runtime.read(cx).registry.read(cx);
        let sessions: Vec<_> = registry
            .all()
            .iter()
            .filter_map(|meta| registry.codex(meta.id).map(|s| (meta.id, s.clone())))
            .collect();
        let live: std::collections::HashSet<_> = sessions.iter().map(|(id, _)| *id).collect();
        self.header_tasks.triggers.retain(|id, _| live.contains(id));
        self.header_tasks.bounds.retain(|id, _| live.contains(id));
        self.task_glance_observers.retain(|id, _| live.contains(id));
        self.task_glances.retain(|id, _| live.contains(id));
        let workspaces = cx
            .global::<crate::settings::SettingsState>()
            .store
            .workspaces
            .rows
            .iter()
            .map(|workspace| workspace.id)
            .collect();
        self.dashboard_timelines.retain(&live, &workspaces);
        for (id, session) in sessions {
            if self
                .task_glance_observers
                .get(&id)
                .is_some_and(|(existing, _)| existing == &session)
            {
                continue;
            }
            let mut cache = TaskGlanceCache::default();
            {
                let state = &session.read(cx).state;
                cache.refresh(&state.saved.items);
                if self.dashboard_open && self.dashboard_owns_session(id) {
                    cache.refresh_dashboard(&state.saved.items, state.status);
                }
            }
            self.task_glances.insert(id, cache);
            let observer = cx.observe(&session, move |this, session, cx| {
                let visible = this.dashboard_open && this.dashboard_owns_session(id);
                let state = &session.read(cx).state;
                let cache = this.task_glances.entry(id).or_default();
                let changed = cache.refresh(&state.saved.items);
                let activity_changed =
                    visible && cache.refresh_dashboard(&state.saved.items, state.status);
                if changed || activity_changed {
                    cx.notify();
                }
            });
            self.task_glance_observers.insert(id, (session, observer));
        }
        self.sync_dashboard_timeline_scope(cx);
    }

    pub(super) fn task_glance(&self, id: SessionId) -> Option<&TaskGlance> {
        self.task_glances.get(&id).and_then(TaskGlanceCache::glance)
    }
}

/// The sidebar row's "3/7 ▬" cell: beside project identity in list cards, trailing in tree rows.
pub(super) fn row_progress(id: SessionId, glance: &TaskGlance) -> AnyElement {
    div()
        .id(("session-tasks", id.raw()))
        .debug_selector(move || format!("session-tasks-{}", id.raw()))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(rpx(SPACE_SM))
        .font_family(crate::fonts::UI_FAMILY)
        .text_size(rpx(HIERARCHY_META_TEXT))
        .line_height(rpx(SESSION_META_LINE_H))
        .text_color(c::FG_DIM())
        .whitespace_nowrap()
        .child(glance.count_label())
        .child(task_meter(glance.done, glance.total, ICON_LG))
        .into_any_element()
}

/// Focus and disclosure state for the shared main/grid session header.
#[derive(Default)]
pub(super) struct HeaderTasks {
    pub open: Option<SessionId>,
    triggers: HashMap<SessionId, FocusHandle>,
    bounds: HashMap<SessionId, Rc<Cell<gpui::Bounds<gpui::Pixels>>>>,
    popup_focus: Option<FocusHandle>,
}

fn header_label(status: Status, glance: &TaskGlance) -> String {
    match status {
        Status::Starting | Status::Working => glance.count_label(),
        _ => format!("{} · {}", lifecycle_label(status), glance.count_label()),
    }
}

fn lifecycle_label(status: Status) -> &'static str {
    match status {
        Status::Starting => "Starting",
        Status::Working => "Working",
        Status::Waiting => "Needs you",
        Status::Failed | Status::Disconnected => "Failed",
        Status::Done => "Done",
        Status::Idle | Status::Interrupted => "Idle",
    }
}

fn lifecycle_color(status: Status) -> gpui::Hsla {
    super::super::status::codex_kind(status).color()
}

fn lifecycle_mark(status: Status, cx: &App) -> AnyElement {
    super::super::status::mark(
        "header-task-spinner",
        super::super::status::codex_kind(status),
        ICON_SM,
        cx,
    )
}

impl Sidebar {
    pub(super) fn sync_header_task_disclosure(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.header_tasks.open else {
            return;
        };
        let active = self.active_canvas_sessions(cx).contains(&(id, false));
        let visible = active
            && (self.mode == super::ViewMode::Grid
                || self.selection == Some(super::Selection::Session(id)));
        let has_list = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(id)
            .is_some_and(|session| tasks::current(&session.read(cx).state.saved.items).is_some());
        if !visible || !has_list || self.is_zen() {
            if self
                .header_tasks
                .popup_focus
                .as_ref()
                .is_some_and(|focus| focus.contains_focused(window, cx))
            {
                self.focus.focus(window, cx);
            }
            self.dismiss_header_tasks(false, window, cx);
        }
    }

    pub(super) fn dismiss_header_tasks_for_navigation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .header_tasks
            .popup_focus
            .as_ref()
            .is_some_and(|focus| focus.contains_focused(window, cx))
        {
            self.focus.focus(window, cx);
        }
        self.dismiss_header_tasks(false, window, cx);
    }

    pub(super) fn focus_header_task_disclosure(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(focus) = &self.header_tasks.popup_focus {
            focus.focus(window, cx);
        }
    }

    pub(super) fn dismiss_header_tasks(
        &mut self,
        restore_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.header_tasks.open.take() {
            if restore_focus {
                if let Some(focus) = self.header_tasks.triggers.get(&id) {
                    focus.focus(window, cx);
                }
            }
            cx.notify();
        }
    }

    fn toggle_header_tasks(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        if self.header_tasks.open == Some(id) {
            self.dismiss_header_tasks(true, window, cx);
        } else {
            self.dismiss_project_context(cx);
            self.header_tasks.open = Some(id);
            self.header_tasks
                .popup_focus
                .get_or_insert_with(|| cx.focus_handle())
                .focus(window, cx);
            cx.notify();
        }
    }

    pub(super) fn header_task_progress(
        &mut self,
        id: SessionId,
        status: Status,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let glance = self.task_glance(id)?.clone();
        let focus = self
            .header_tasks
            .triggers
            .entry(id)
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let bounds = self.header_tasks.bounds.entry(id).or_default().clone();
        let name = format!(
            "{} · {} of {} tasks complete",
            lifecycle_label(status),
            glance.done,
            glance.total
        );
        let tooltip = format!("{name} · Show task list");
        Some(
            div()
                .id(("header-tasks", id.raw()))
                .debug_selector(move || format!("header-tasks-{}", id.raw()))
                .track_focus(&focus)
                .relative()
                .role(gpui::Role::Button)
                .cursor_pointer()
                .tab_index(0)
                .aria_label(name)
                .aria_expanded(self.header_tasks.open == Some(id))
                .h(rpx(CHROME_CONTROL_H))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(rpx(SPACE_SM))
                .px(rpx(SPACE_SM))
                .rounded(rpx(RADIUS_CONTROL))
                .text_size(rpx(TEXT_MICRO))
                .text_color(lifecycle_color(status))
                .whitespace_nowrap()
                .hover(|style| style.bg(c::BG_HOVER()))
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .when(self.header_tasks.open == Some(id), |el| {
                    el.bg(c::BG_HOVER())
                })
                .on_prepaint(move |rect, _, _| bounds.set(rect))
                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                    focus.focus(window, cx);
                    cx.stop_propagation();
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.toggle_header_tasks(id, window, cx);
                }))
                .on_key_down(
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                            && !event.is_held
                        {
                            cx.stop_propagation();
                            this.toggle_header_tasks(id, window, cx);
                        }
                    }),
                )
                .tooltip(move |window, cx| {
                    crate::views::components::tooltip(tooltip.clone(), window).build(window, cx)
                })
                .child(lifecycle_mark(status, cx))
                .child(header_label(status, &glance))
                .into_any_element(),
        )
    }

    pub(super) fn header_task_disclosure(
        &mut self,
        id: SessionId,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.header_tasks.open != Some(id) {
            return None;
        }
        let session = self.runtime.read(cx).registry.read(cx).codex(id)?.clone();
        let state = &session.read(cx).state;
        let Some(list) = tasks::current(&state.saved.items) else {
            self.header_tasks.open = None;
            return None;
        };
        let status = state.status;
        let focus = self.header_tasks.popup_focus.as_ref()?.clone();
        let trigger = self.header_tasks.bounds.get(&id)?.clone();
        let position = trigger.get().bottom_right();
        let margin = rpx(SPACE_LG).to_pixels(window.rem_size());
        let mut rows = div()
            .id(("header-task-list", id.raw()))
            .role(gpui::Role::Group)
            .aria_label("Tasks")
            .max_h(rpx(MODAL_SCROLL_MAX_H))
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG));
        for (index, task) in list.tasks.iter().enumerate() {
            let status_label = match task.status {
                TaskStatus::Completed => "Completed",
                TaskStatus::InProgress => "In progress",
                TaskStatus::Pending => "Pending",
            };
            rows = rows.child(
                div()
                    .id((
                        gpui::SharedString::from(format!("header-task-row-{}", id.raw())),
                        index,
                    ))
                    .debug_selector(move || format!("header-task-row-{}-{index}", id.raw()))
                    .role(gpui::Role::Group)
                    .aria_label(format!("{status_label} · {}", task.step))
                    .flex()
                    .items_start()
                    .gap(rpx(SPACE_LG))
                    .text_size(rpx(TEXT_SMALL))
                    .child(task_mark(task.status, task_phase(status), 0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .child(task.step.clone()),
                    ),
            );
        }
        let heading = format!(
            "{} · {} of {} tasks complete",
            lifecycle_label(status),
            list.done(),
            list.total()
        );
        let panel = div()
            .id(("header-task-disclosure", id.raw()))
            .debug_selector(move || format!("header-task-disclosure-{}", id.raw()))
            .track_focus(&focus)
            .tab_index(0)
            .role(gpui::Role::Group)
            .aria_label(heading.clone())
            .occlude()
            .w(rpx(MODAL_W_SM))
            .max_w(window.viewport_size().width - margin - margin)
            .max_h(window.viewport_size().height - margin - margin)
            .overflow_y_scroll()
            .p(rpx(SPACE_3XL))
            .rounded(rpx(RADIUS_GROUP))
            .bg(c::SURFACE_RAISED())
            .border_1()
            .border_color(c::BORDER())
            .font_family(crate::fonts::UI_FAMILY)
            .text_color(c::FG())
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL))
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.dismiss_header_tasks(true, window, cx);
                }
            }))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(
                move |this, event: &gpui::MouseDownEvent, window, cx| {
                    if !trigger.get().contains(&event.position) {
                        this.dismiss_header_tasks(false, window, cx);
                    }
                },
            ))
            .child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(heading),
            )
            .when(list.all_done(), |panel| {
                panel.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("All tasks done"),
                )
            })
            .when_some(list.now_label(), |panel, now| {
                panel.child(
                    div()
                        .id(("header-task-current", id.raw()))
                        .debug_selector(move || format!("header-task-current-{}", id.raw()))
                        .role(gpui::Role::Status)
                        .aria_label(format!("Current: {now}"))
                        .text_size(rpx(TEXT_SMALL))
                        .whitespace_normal()
                        .child(format!("Current: {now}")),
                )
            })
            .when_some(list.explanation, |panel, explanation| {
                panel.child(
                    div()
                        .id(("header-task-explanation", id.raw()))
                        .role(gpui::Role::Group)
                        .aria_label(explanation.clone())
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .whitespace_normal()
                        .child(explanation),
                )
            })
            .child(rows);
        Some(
            gpui::deferred(
                gpui::anchored()
                    .anchor(gpui::Anchor::TopRight)
                    .position(position)
                    .snap_to_window_with_margin(margin)
                    .child(panel),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan(statuses: &[(&str, &str)]) -> Value {
        json!({
            "type": "plan",
            "plan": statuses
                .iter()
                .enumerate()
                .map(|(i, (step, status))| json!({"id": i.to_string(), "step": step, "status": status}))
                .collect::<Vec<_>>(),
        })
    }

    #[test]
    fn task_glance_labels_progress_and_current_step() {
        let items = vec![plan(&[
            ("Map statuses", "completed"),
            ("Render task strip", "in_progress"),
            ("Run cargo test", "pending"),
        ])];
        let glance = TaskGlance::from_list(&tasks::current(&items).unwrap());
        assert_eq!(glance.count_label(), "1/3");
        assert_eq!(glance.now.as_deref(), Some("Render task strip"));
        assert_eq!(
            glance.tooltip_suffix(),
            "Render task strip · 1 of 3 tasks done"
        );
        assert!(!glance.all_done());
    }

    #[test]
    fn task_glance_all_done_reads_as_finished() {
        let items = vec![plan(&[("A", "completed"), ("B", "completed")])];
        let glance = TaskGlance::from_list(&tasks::current(&items).unwrap());
        assert!(glance.all_done());
        assert_eq!(glance.count_label(), "2/2");
        assert!(glance.now.is_none());
        assert_eq!(glance.tooltip_suffix(), "2 of 2 tasks done");
    }

    #[test]
    fn header_progress_preserves_lifecycle_when_all_tasks_are_complete() {
        let glance = TaskGlance::from_list(
            &tasks::current(&[plan(&[("A", "completed"), ("B", "completed")])]).unwrap(),
        );
        assert_eq!(header_label(Status::Working, &glance), "2/2");
        assert_eq!(header_label(Status::Waiting, &glance), "Needs you · 2/2");
        assert_eq!(header_label(Status::Failed, &glance), "Failed · 2/2");
        assert_eq!(header_label(Status::Disconnected, &glance), "Failed · 2/2");
        assert_eq!(lifecycle_color(Status::Waiting), c::STATUS_WARNING());
        assert_eq!(lifecycle_color(Status::Failed), c::STATUS_ERROR());
    }

    #[test]
    fn task_phase_follows_session_status() {
        assert_eq!(task_phase(Status::Working), TaskPhase::Running);
        assert_eq!(task_phase(Status::Starting), TaskPhase::Running);
        assert_eq!(task_phase(Status::Waiting), TaskPhase::Waiting);
        assert_eq!(task_phase(Status::Interrupted), TaskPhase::Halted);
        assert_eq!(task_phase(Status::Failed), TaskPhase::Halted);
        assert_eq!(task_phase(Status::Disconnected), TaskPhase::Halted);
        assert_eq!(task_phase(Status::Idle), TaskPhase::Stale);
        assert_eq!(task_phase(Status::Done), TaskPhase::Stale);
    }

    #[test]
    fn task_glance_cache_is_absent_without_a_list_and_tracks_changes() {
        let mut cache = TaskGlanceCache::default();
        let mut items = vec![json!({"type": "agentMessage", "text": "hi"})];
        assert!(!cache.refresh(&items));
        assert!(cache.glance().is_none());

        items.push(plan(&[("A", "in_progress"), ("B", "pending")]));
        assert!(cache.refresh(&items));
        assert_eq!(cache.glance().unwrap().count_label(), "0/2");

        // Newer non-checklist items leave the summary untouched.
        items.push(json!({"type": "agentMessage", "text": "working"}));
        assert!(!cache.refresh(&items));

        // An in-place update of the snapshot (Codex rewrites the plan item).
        items[1] = plan(&[("A", "completed"), ("B", "in_progress")]);
        assert!(cache.refresh(&items));
        assert_eq!(cache.glance().unwrap().now.as_deref(), Some("B"));

        // A clarification answer retains the same session summary.
        items.push(json!({"type": "userMessage", "text": "next"}));
        let before = cache.glance().cloned();
        assert!(!cache.refresh(&items));
        assert_eq!(cache.glance(), before.as_ref());
        let restored: Vec<Value> =
            serde_json::from_str(&serde_json::to_string(&items).unwrap()).unwrap();
        let mut restored_cache = TaskGlanceCache::default();
        assert!(restored_cache.refresh(&restored));
        assert_eq!(restored_cache.glance(), cache.glance());

        // An empty latest snapshot clears the list.
        items.push(plan(&[]));
        assert!(cache.refresh(&items));
        assert!(cache.glance().is_none());
    }
    #[test]
    fn dashboard_activity_and_lifecycle_refresh_without_checklist_change() {
        let mut cache = TaskGlanceCache::default();
        let mut items = grove_core::codex::TranscriptItems::from(vec![
            plan(&[("A", "completed")]),
            json!({"type":"agentMessage","text":"First observed activity"}),
        ]);
        assert!(cache.refresh(&items));
        assert!(cache.refresh_dashboard(&items, Status::Working));
        let first = cache.timeline();
        items.push(json!({"type":"agentMessage","text":"New activity after the plan completed"}));
        assert!(!cache.refresh(&items));
        assert!(cache.refresh_dashboard(&items, Status::Working));
        assert_eq!(cache.timeline().list.as_ref().unwrap().done(), 1);
        assert_eq!(
            cache.timeline().activity.last().unwrap().text,
            "New activity after the plan completed"
        );
        assert!(!std::sync::Arc::ptr_eq(&first, &cache.timeline()));
        let current = cache.timeline();
        items.push(json!({"type":"reasoning","text":"not displayed"}));
        assert!(!cache.refresh_dashboard(&items, Status::Working));
        assert!(std::sync::Arc::ptr_eq(&current, &cache.timeline()));
        assert!(cache.refresh_dashboard(&items, Status::Waiting));
        assert!(!cache.refresh_dashboard(&items, Status::Waiting));
        let mut no_plan = TaskGlanceCache::default();
        no_plan.refresh(&items[1..]);
        assert!(no_plan.refresh_dashboard(&items[1..], Status::Working));
        assert!(no_plan.timeline().list.is_none());
        assert!(!no_plan.timeline().activity.is_empty());
    }
    #[test]
    fn dashboard_tool_completion_refreshes_shimmer_fact_without_text_change() {
        let mut cache = TaskGlanceCache::default();
        let mut items = vec![
            json!({"type":"commandExecution","command":"observe progress","status":"inProgress"}),
        ];
        assert!(cache.refresh_dashboard(&items, Status::Working));
        assert_eq!(cache.timeline().activity[0].observed_active, Some(true));
        items[0]["status"] = json!("completed");
        assert!(!cache.refresh(&items));
        assert!(cache.refresh_dashboard(&items, Status::Working));
        assert_eq!(cache.timeline().activity[0].observed_active, Some(false));
        assert_eq!(cache.timeline().activity[0].text, "observe progress");
    }
}
