//! Cross-session task-list progress: the sidebar row's done/total cell and the
//! grid tile header's current-step cluster.
//!
//! Progress is derived from a structured session's saved transcript items.
//! [`TaskGlanceCache`] runs from a per-session observer (not from render), so
//! a frame only reads the cached [`TaskGlance`].

use super::{Sidebar, HIERARCHY_META_TEXT, SESSION_META_LINE_H, SESSION_SPINNER_PERIOD};
use crate::{
    entities::session_registry::SessionId,
    theme as c,
    views::{
        components::{task_mark, task_meter, TaskPhase},
        rpx,
        tokens::*,
    },
};
use gpui::{div, prelude::*, AnimationExt as _, AnyElement, App, Context};
use grove_core::{
    codex::Status,
    tasks::{self, TaskList, TaskStatus},
};
use serde_json::Value;

/// The summary of a session's current task list that the sidebar and grid
/// headers render.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TaskGlance {
    pub done: usize,
    pub total: usize,
    pub stale: bool,
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
            stale: list.stale,
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

    /// The tile header's trailing text: the current step, or the finish line.
    pub fn now_label(&self) -> Option<String> {
        if self.all_done() {
            Some("All tasks done".into())
        } else {
            self.now.clone()
        }
    }

    /// Appended to the row tooltip: "Rendering task strip · 3 of 7 tasks done".
    pub fn tooltip_suffix(&self) -> String {
        let count = format!("{} of {} tasks done", self.done, self.total);
        match self.now.as_deref().filter(|_| !self.all_done()) {
            Some(now) => format!("{now} · {count}"),
            None => count,
        }
    }

    /// The status the leading mark represents.
    pub fn mark_status(&self) -> TaskStatus {
        if self.all_done() {
            TaskStatus::Completed
        } else {
            self.current.unwrap_or(TaskStatus::Pending)
        }
    }

    /// The mark's phase: a stale list is muted; a finished list keeps its
    /// green check; otherwise the owning session's status decides.
    pub fn mark_phase(&self, status: Status) -> TaskPhase {
        if self.all_done() && !self.stale {
            TaskPhase::Running
        } else {
            task_phase(status, self.stale)
        }
    }
}

/// How the current task's mark reads for a session in `status`.
pub(super) fn task_phase(status: Status, stale: bool) -> TaskPhase {
    if stale {
        return TaskPhase::Stale;
    }
    match status {
        Status::Starting | Status::Working => TaskPhase::Running,
        Status::Waiting => TaskPhase::Waiting,
        Status::Interrupted | Status::Failed | Status::Disconnected => TaskPhase::Halted,
        Status::Idle | Status::Done => TaskPhase::Stale,
    }
}

/// Memoises [`tasks::current`] for one session. Only a changed checklist
/// snapshot re-parses the list; newer items merely re-check staleness.
#[derive(Default)]
pub(super) struct TaskGlanceCache {
    index: Option<usize>,
    item: Value,
    glance: Option<TaskGlance>,
}

impl TaskGlanceCache {
    pub fn glance(&self) -> Option<&TaskGlance> {
        self.glance.as_ref()
    }

    /// Refreshes from `items`; returns whether the glance changed.
    pub fn refresh(&mut self, items: &[Value]) -> bool {
        let index = items.iter().rposition(tasks::is_checklist);
        let glance = match index {
            None => {
                self.item = Value::Null;
                None
            }
            Some(at) if self.index == Some(at) && self.item == items[at] => {
                self.glance.clone().map(|glance| TaskGlance {
                    stale: items[at + 1..]
                        .iter()
                        .any(|item| item["type"] == "userMessage"),
                    ..glance
                })
            }
            Some(at) => {
                self.item = items[at].clone();
                tasks::current(items).map(|list| TaskGlance::from_list(&list))
            }
        };
        self.index = index;
        let changed = glance != self.glance;
        self.glance = glance;
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
        self.task_glance_observers.retain(|id, _| live.contains(id));
        self.task_glances.retain(|id, _| live.contains(id));
        for (id, session) in sessions {
            if self
                .task_glance_observers
                .get(&id)
                .is_some_and(|(existing, _)| existing == &session)
            {
                continue;
            }
            let mut cache = TaskGlanceCache::default();
            cache.refresh(&session.read(cx).state.saved.items);
            self.task_glances.insert(id, cache);
            let observer = cx.observe(&session, move |this, session, cx| {
                let changed = this
                    .task_glances
                    .entry(id)
                    .or_default()
                    .refresh(&session.read(cx).state.saved.items);
                if changed {
                    cx.notify();
                }
            });
            self.task_glance_observers.insert(id, (session, observer));
        }
    }

    pub(super) fn task_glance(&self, id: SessionId) -> Option<&TaskGlance> {
        self.task_glances.get(&id).and_then(TaskGlanceCache::glance)
    }
}

/// The sidebar row's "3/7 ▬" cell, placed just before the Git counts.
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
        .when(glance.stale, |cell| cell.opacity(OPACITY_DISABLED))
        .child(glance.count_label())
        .child(task_meter(glance.done, glance.total, ICON_LG))
        .into_any_element()
}

/// The grid tile header cluster: mark, "3/7", meter and the current step.
pub(super) fn tile_progress(
    id: SessionId,
    glance: &TaskGlance,
    status: Status,
    cx: &App,
) -> AnyElement {
    let mark_status = glance.mark_status();
    let phase = glance.mark_phase(status);
    let mark = div()
        .id(("grid-header-task-mark", id.raw()))
        .size(rpx(ICON_SM))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center();
    let mark = if mark_status == TaskStatus::InProgress
        && phase == TaskPhase::Running
        && !cx.reduce_motion()
    {
        mark.with_animation(
            ("grid-header-task-spinner", id.raw()),
            gpui::Animation::new(SESSION_SPINNER_PERIOD).repeat(),
            move |element, progress| {
                let frame =
                    (progress * crate::entities::animation_clock::SPINNER_FRAMES as f32) as u64;
                element.child(task_mark(mark_status, phase, frame))
            },
        )
        .into_any_element()
    } else {
        mark.child(task_mark(mark_status, phase, 0))
            .into_any_element()
    };
    div()
        .id(("grid-header-tasks", id.raw()))
        .debug_selector(move || format!("grid-header-tasks-{}", id.raw()))
        .min_w_0()
        .overflow_hidden()
        .flex()
        .items_center()
        .gap(rpx(SPACE_SM))
        .font_family(crate::fonts::UI_FAMILY)
        .text_size(rpx(TEXT_SMALL))
        .text_color(c::FG_DIM())
        .whitespace_nowrap()
        .when(glance.stale, |cluster| cluster.opacity(OPACITY_DISABLED))
        .child(mark)
        .child(
            div()
                .flex_shrink_0()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(glance.count_label()),
        )
        .child(task_meter(glance.done, glance.total, ICON_LG))
        .when_some(glance.now_label(), |cluster, now| {
            cluster.child(
                div()
                    .debug_selector(move || format!("grid-header-task-now-{}", id.raw()))
                    .min_w_0()
                    .max_w(rpx(MODAL_W_SM / 3.))
                    .truncate()
                    .child(now),
            )
        })
        .into_any_element()
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
        assert_eq!(glance.now_label().as_deref(), Some("Render task strip"));
        assert_eq!(
            glance.tooltip_suffix(),
            "Render task strip · 1 of 3 tasks done"
        );
        assert_eq!(glance.mark_status(), TaskStatus::InProgress);
        assert!(!glance.all_done());
    }

    #[test]
    fn task_glance_all_done_reads_as_finished() {
        let items = vec![plan(&[("A", "completed"), ("B", "completed")])];
        let glance = TaskGlance::from_list(&tasks::current(&items).unwrap());
        assert!(glance.all_done());
        assert_eq!(glance.count_label(), "2/2");
        assert_eq!(glance.now_label().as_deref(), Some("All tasks done"));
        assert_eq!(glance.tooltip_suffix(), "2 of 2 tasks done");
        assert_eq!(glance.mark_status(), TaskStatus::Completed);
        assert_eq!(glance.mark_phase(Status::Done), TaskPhase::Running);
    }

    #[test]
    fn task_phase_follows_session_status_and_staleness() {
        assert_eq!(task_phase(Status::Working, false), TaskPhase::Running);
        assert_eq!(task_phase(Status::Starting, false), TaskPhase::Running);
        assert_eq!(task_phase(Status::Waiting, false), TaskPhase::Waiting);
        assert_eq!(task_phase(Status::Interrupted, false), TaskPhase::Halted);
        assert_eq!(task_phase(Status::Failed, false), TaskPhase::Halted);
        assert_eq!(task_phase(Status::Disconnected, false), TaskPhase::Halted);
        assert_eq!(task_phase(Status::Idle, false), TaskPhase::Stale);
        assert_eq!(task_phase(Status::Done, false), TaskPhase::Stale);
        assert_eq!(task_phase(Status::Working, true), TaskPhase::Stale);
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

        // A new turn without a list marks it stale.
        items.push(json!({"type": "userMessage", "text": "next"}));
        assert!(cache.refresh(&items));
        assert!(cache.glance().unwrap().stale);

        // An empty latest snapshot clears the list.
        items.push(plan(&[]));
        assert!(cache.refresh(&items));
        assert!(cache.glance().is_none());
    }
}
