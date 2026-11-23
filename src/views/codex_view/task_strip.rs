//! Read-only transcript checklist, completion summary, and task markers.

use super::{
    action_key, c, codex, composer_tray, hash_transcript_value, rpx, task_mark, task_meter, tasks,
    tooltip, CodexView, Hash, Hasher, StatusKind, TaskList, TaskPhase, TaskStatus,
    COMPOSER_MENU_MAX_H,
};
use crate::views::tokens::{
    CONTROL_H, ICON_BTN_W, ICON_MD, ICON_SM, MOTION_SLOW_MS, RADIUS_CONTROL, RADIUS_GROUP,
    SPACE_2XL, SPACE_LG, SPACE_SM, TEXT_MICRO, TEXT_SMALL,
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled,
};
use gpui::AnimationExt as _;
use gpui::{div, App, Context, Window};

/// The task strip header uses the compact control height.
pub(super) const TASK_STRIP_HEAD_H: f32 = CONTROL_H;

/// One task row in the expanded strip (product/design.md row-h).
const TASK_ROW_H: f32 = CONTROL_H + SPACE_SM;
/// Rows reserve the header disclosure column before their status mark.
pub(super) const TASK_MARK_INSET: f32 = ICON_MD + SPACE_SM;
pub(super) const TASK_LABEL_INSET: f32 = TASK_MARK_INSET + ICON_SM + SPACE_LG;
/// One full turn of the running task mark.
const TASK_SPINNER_PERIOD: std::time::Duration =
    std::time::Duration::from_millis(MOTION_SLOW_MS * 4);
/// Clock ticks per spinner frame, the inverse of `animation_clock::spinner_frame`.
pub(super) const SPINNER_TICKS_PER_FRAME: u64 = 3;

/// The agent's checklist, derived once per checklist change rather than per frame.
pub(super) struct TaskCache {
    pub(super) key: u64,
    pub(super) list: Option<TaskList>,
}

/// Changes when a checklist snapshot changes or transcript indices change, so
/// streaming text reuses the cache.
pub(super) fn task_cache_key<T: std::borrow::Borrow<serde_json::Value>>(items: &[T]) -> u64 {
    let mut hasher = rustc_hash::FxHasher::default();
    items.len().hash(&mut hasher);
    for (index, item) in items.iter().enumerate() {
        let item = item.borrow();
        if tasks::is_checklist(item) {
            index.hash(&mut hasher);
            hash_transcript_value(item, &mut hasher);
        }
    }
    hasher.finish()
}

pub(super) fn task_cache<T: std::borrow::Borrow<serde_json::Value>>(
    items: &[T],
    key: u64,
) -> TaskCache {
    TaskCache {
        key,
        list: tasks::current(items),
    }
}

/// How in-progress marks read for the session's state. Idle or finished
/// sessions keep a static mark while retaining their checklist.
pub(super) fn task_phase(status: codex::Status) -> TaskPhase {
    match status {
        codex::Status::Starting | codex::Status::Working => TaskPhase::Running,
        codex::Status::Waiting => TaskPhase::Waiting,
        codex::Status::Interrupted | codex::Status::Failed | codex::Status::Disconnected => {
            TaskPhase::Halted
        }
        codex::Status::Idle | codex::Status::Done => TaskPhase::Stale,
    }
}

/// Completed marks stay green across session states and continuation turns.
pub(super) fn task_mark_phase(status: TaskStatus, phase: TaskPhase) -> TaskPhase {
    if status == TaskStatus::Completed {
        TaskPhase::Running
    } else {
        phase
    }
}

/// The status whose mark leads the strip header.
pub(super) fn task_header_status(list: &TaskList) -> TaskStatus {
    if list.all_done() {
        TaskStatus::Completed
    } else if list.current().is_some() {
        TaskStatus::InProgress
    } else {
        TaskStatus::Pending
    }
}

/// What the strip header says after its count and meter.
#[derive(Debug, PartialEq)]
pub(super) enum TaskSummary {
    /// "All N tasks done", secondary.
    AllDone(usize),
    /// The step text, then "Waiting for you" in amber.
    Waiting(String),
    /// "Stopped at {step}", secondary.
    Stopped(String),
    /// The current task's activeForm, else its step.
    Now(String),
    None,
}

impl TaskSummary {
    pub(super) fn text(&self) -> String {
        match self {
            Self::AllDone(total) => format!("All {total} tasks done"),
            Self::Waiting(step) => format!("{step} · Waiting for you"),
            Self::Stopped(step) => format!("Stopped at {step}"),
            Self::Now(label) => label.clone(),
            Self::None => String::new(),
        }
    }
}

pub(super) fn task_summary(list: &TaskList, phase: TaskPhase) -> TaskSummary {
    if list.all_done() {
        return TaskSummary::AllDone(list.total());
    }
    let Some(current) = list.current() else {
        return TaskSummary::None;
    };
    match phase {
        TaskPhase::Waiting => TaskSummary::Waiting(current.step.clone()),
        TaskPhase::Halted => TaskSummary::Stopped(current.step.clone()),
        TaskPhase::Running | TaskPhase::Stale => {
            list.now_label().map_or(TaskSummary::None, |label| {
                TaskSummary::Now(label.to_owned())
            })
        }
    }
}

/// One task mark; the running spinner steps through its frames unless motion is reduced.
fn animated_task_mark(
    id: impl Into<gpui::ElementId>,
    status: TaskStatus,
    phase: TaskPhase,
    cx: &App,
) -> gpui::AnyElement {
    let id = id.into();
    if status == TaskStatus::InProgress && phase == TaskPhase::Running && !cx.reduce_motion() {
        return super::super::components::isolated_animation(id.clone(), move |_, cx| {
            animated_task_mark_inner(id.clone(), status, phase, cx)
        });
    }
    animated_task_mark_inner(id, status, phase, cx)
}

fn animated_task_mark_inner(
    id: gpui::ElementId,
    status: TaskStatus,
    phase: TaskPhase,
    cx: &App,
) -> gpui::AnyElement {
    let selector = match &id {
        gpui::ElementId::Name(name) if name.as_ref() == "codex-task-strip-mark" => {
            "codex-task-header-mark".to_owned()
        }
        gpui::ElementId::NamedInteger(name, index) if name.as_ref() == "codex-task-row-mark" => {
            format!("codex-task-row-status-{index}")
        }
        _ => "codex-task-status".to_owned(),
    };
    let slot = div()
        .debug_selector(move || selector.clone())
        .size(rpx(ICON_SM))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center();
    if status == TaskStatus::InProgress && phase == TaskPhase::Running && !cx.reduce_motion() {
        slot.with_animation(
            id,
            gpui::Animation::new(TASK_SPINNER_PERIOD).repeat(),
            move |slot, progress| {
                let frame =
                    (progress * crate::entities::animation_clock::SPINNER_FRAMES as f32) as u64;
                slot.child(task_mark(status, phase, frame * SPINNER_TICKS_PER_FRAME))
            },
        )
        .into_any_element()
    } else {
        slot.child(task_mark(status, phase, 0)).into_any_element()
    }
}

impl CodexView {
    /// Collapse once per arriving identity, never on a routine rerender.
    pub(super) fn toggle_tasks(&mut self, cx: &mut Context<Self>) {
        if self.composer_collapsed {
            self.set_composer_collapsed(false, cx);
            self.tasks_expanded = true;
        } else {
            self.tasks_expanded = !self.tasks_expanded;
        }
        cx.notify();
    }

    /// The docked, read-only checklist; jumps identify the task’s transcript start.
    pub(super) fn task_strip(
        &self,
        attached_above: bool,
        list: &TaskList,
        status: codex::Status,
        jumps: Vec<usize>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // Browsing temporarily compacts both docked surfaces without changing
        // the user's task disclosure preference.
        let expanded = self.tasks_expanded && !self.composer_collapsed;
        let phase = task_phase(status);
        let (done, total) = (list.done(), list.total());
        let summary = task_summary(list, phase);
        let header_status = task_header_status(list);
        let separator = || div().flex_shrink_0().text_color(c::FG_MUTE()).child("·");
        let note = match (&summary, expanded) {
            // Expanded, the rows carry the step; the header keeps only the state.
            (TaskSummary::Waiting(_), true) => Some(
                div()
                    .flex_shrink_0()
                    .text_color(StatusKind::NeedsInput.color())
                    .child("Waiting for you")
                    .into_any_element(),
            ),
            (TaskSummary::Stopped(_), true) => Some(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(summary.text())
                    .into_any_element(),
            ),
            (_, true) | (TaskSummary::None, false) => None,
            // Trailing notes grow into the slack: GPUI truncates by summed glyph
            // advances, which exceed a content-sized box's kerned width. The
            // step therefore trails the fixed "Waiting for you".
            (TaskSummary::Waiting(step), false) => Some(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(StatusKind::NeedsInput.color())
                            .child("Waiting for you"),
                    )
                    .child(separator())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(c::FG())
                            .child(step.clone()),
                    )
                    .into_any_element(),
            ),
            (TaskSummary::Now(label), false) => Some(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(c::FG())
                    .child(label.clone())
                    .into_any_element(),
            ),
            (TaskSummary::AllDone(_) | TaskSummary::Stopped(_), false) => Some(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(summary.text())
                    .into_any_element(),
            ),
        };
        let progress = format!("{done} of {total} done");
        let cluster = div()
            .id("codex-task-strip-summary")
            .debug_selector(|| "codex-task-strip-summary".into())
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_LG))
            .child(animated_task_mark(
                "codex-task-strip-mark",
                header_status,
                task_mark_phase(header_status, phase),
                cx,
            ))
            .child(
                div()
                    .debug_selector(|| "codex-task-header-label".into())
                    .flex_shrink_0()
                    .text_color(if expanded { c::FG() } else { c::FG_DIM() })
                    .group_hover("neutral-control", |text| text.text_color(c::FG()))
                    .child("Tasks"),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(format!("{done}/{total}")),
            )
            .child(task_meter(done, total, ICON_BTN_W))
            .when_some(note, |cluster, note| cluster.child(separator()).child(note));
        let header = div()
            .id("codex-task-strip")
            .debug_selector(|| "codex-task-strip".into())
            .role(gpui::Role::Button)
            .cursor_pointer()
            .group("neutral-control")
            .aria_label(format!("Tasks, {progress}"))
            .aria_description(summary.text())
            .aria_expanded(expanded)
            .tab_index(0)
            .w_full()
            .h(rpx(TASK_STRIP_HEAD_H))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_SM))
            .text_size(rpx(TEXT_SMALL))
            .text_color(c::FG_DIM())
            .whitespace_nowrap()
            .when(expanded, |style| style.text_color(c::FG()))
            .hover(|style| style.text_color(c::FG()))
            .focus_visible(|style| style.bg(c::MENU_HOVER()))
            .child(crate::icons::icon(
                if expanded { "chev-down" } else { "chev-right" },
                ICON_MD,
                if expanded { c::FG() } else { c::FG_DIM() },
            ))
            .child(cluster)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if action_key(event) {
                    this.toggle_tasks(cx);
                    cx.stop_propagation();
                }
            }))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_tasks(cx)));
        let body = expanded.then(|| {
            let mut rows = div()
                .id("codex-task-list")
                .debug_selector(|| "codex-task-list".into())
                .flex_1()
                .min_h_0()
                .max_h(rpx(COMPOSER_MENU_MAX_H))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .when_some(list.explanation.clone(), |rows, explanation| {
                    rows.child(
                        div()
                            .id("codex-task-explanation")
                            .debug_selector(|| "codex-task-explanation".into())
                            .pl(rpx(TASK_LABEL_INSET))
                            .pr(rpx(SPACE_LG))
                            .py(rpx(SPACE_SM))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .whitespace_normal()
                            .child(
                                div()
                                    .debug_selector(|| "codex-task-explanation-text".into())
                                    .child(explanation),
                            ),
                    )
                });
            for (task_index, (task, child)) in list.tasks.iter().zip(jumps).enumerate() {
                let current = task.status == TaskStatus::InProgress;
                rows = rows.child(
                    div()
                        .id(("codex-task-row", task_index))
                        .debug_selector(move || format!("codex-task-row-{task_index}"))
                        .group("codex-task-row")
                        .relative()
                        .role(gpui::Role::Button)
                        .cursor_pointer()
                        .aria_label(format!("Jump to start: {}", task.step))
                        .tooltip({
                            let step = task.step.clone();
                            move |window, cx| tooltip(step.clone(), window).build(window, cx)
                        })
                        .tab_index(0)
                        .min_h(rpx(TASK_ROW_H))
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .pl(rpx(TASK_MARK_INSET))
                        .pr(rpx(SPACE_LG))
                        .rounded(rpx(RADIUS_CONTROL))
                        .text_size(rpx(TEXT_SMALL))
                        .hover(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
                        .focus_visible(|style| style.bg(c::MENU_HOVER()))
                        .child(animated_task_mark(
                            ("codex-task-row-mark", task_index),
                            task.status,
                            task_mark_phase(task.status, phase),
                            cx,
                        ))
                        .child(
                            div()
                                .debug_selector(move || format!("codex-task-title-{task_index}"))
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(if task.status == TaskStatus::Completed {
                                    c::FG_DIM()
                                } else {
                                    c::FG()
                                })
                                .when(current, |text| text.font_weight(gpui::FontWeight::MEDIUM))
                                .child(task.step.clone()),
                        )
                        .child(
                            div()
                                .id(("codex-task-jump", task_index))
                                .debug_selector(move || format!("codex-task-jump-{task_index}"))
                                // Reserve the action slot so revealing it never covers task text.
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .px(rpx(SPACE_LG))
                                .rounded_r(rpx(RADIUS_GROUP))
                                .bg(c::MENU_HOVER())
                                .opacity(0.0)
                                .group_hover("codex-task-row", |style| style.opacity(1.0))
                                .text_size(rpx(TEXT_MICRO))
                                .text_color(c::FG_DIM())
                                .child(crate::icons::icon("arrow-up", ICON_SM, c::FG_DIM()))
                                .child("Jump to start"),
                        )
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                if action_key(event) {
                                    this.jump_to_transcript_item(child, window, cx);
                                    cx.stop_propagation();
                                }
                            },
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.jump_to_transcript_item(child, window, cx);
                        })),
                );
            }
            rows
        });
        composer_tray(window)
            .when(attached_above, gpui::Styled::rounded_none)
            .id("codex-task-strip-panel")
            .debug_selector(|| "codex-task-strip-panel".into())
            // Balance the visible bottom attachment against the header's top inset.
            .when(!expanded, |panel| panel.pb(rpx(SPACE_2XL + SPACE_LG)))
            .when(!expanded, |panel| panel.bg(c::BG()))
            .role(gpui::Role::Group)
            .aria_label(format!("Agent tasks, {progress}"))
            // Shrinks by default: in a short pane the strip yields height to
            // the composer, its list scrolls, and the header row stays visible.
            .min_h(rpx(TASK_STRIP_HEAD_H))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header)
            .children(body)
            .into_any_element()
    }
}
