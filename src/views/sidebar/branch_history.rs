//! Full-canvas, read-only commit graph. The conversation remains mounted underneath.
mod commit_slot;
use super::{Action, Sidebar};
use crate::entities::branch_history::BranchHistoryState;
use crate::{
    icons::icon,
    theme as c,
    views::{rpx, tokens::*},
};
use gpui::{div, prelude::*, AnyElement, Context};
use gpui_component::Selectable as _;
use std::time::Duration;
pub(super) const HISTORY_HOVER_INTENT: Duration = Duration::from_millis(350);
pub(super) const HISTORY_HOVER_GRACE: Duration = Duration::from_millis(80);

#[derive(Default)]
pub(super) struct HistoryHover {
    hovered_row: Option<String>,
    card_hovered: bool,
    pinned: bool,
    intent: Option<gpui::Task<()>>,
    grace: Option<gpui::Task<()>>,
}
use grove_core::{
    diff::{LineKind, Patch, Status},
    history::{Commit, GraphEdge, HeadIdentity, RefKind},
};

const COMMIT_ROW_H: f32 = 36.0;
const GRAPH_LANE_W: f32 = 22.0;
const GRAPH_INSET: f32 = 20.0;
const PREVIEW_W: f32 = 380.0;
const MAX_HISTORY: usize = 500;
const GRAPH_STROKE: f32 = 1.7;
const GRAPH_NODE_RADIUS: f32 = 4.0;
const GRAPH_NODE_STROKE: f32 = 3.0;
const PREVIEW_MESSAGE_H: f32 = 160.0;
const PREVIEW_BOTTOM_SPACE: f32 = 320.0;
const DETAIL_HEADER_MAX_H: f32 = 134.0;
const DETAIL_WRAP_TITLE_W: f32 = 650.0;
const DETAIL_NARROW_W: f32 = 500.0;
const DETAIL_FILES_W: f32 = 200.0;
const DETAIL_FILES_MAX_FRAC: f32 = 0.32;
const DETAIL_FILES_MIN_W: f32 = 120.0;
const DETAIL_FILES_RESIZE_MAX_FRAC: f32 = 0.5;
const DETAIL_FILES_KEY_STEP: f32 = 16.0;
const FILE_ROW_H: f32 = 40.0;
const DIFF_NUMBER_W: f32 = 36.0;
const DIFF_SIGN_W: f32 = 14.0;
const SPLIT_SCROLL_LABEL_W: f32 = 98.0;
const METADATA_LINE_H: f32 = 18.0;
const ROW_SUBJECT_LINE_H: f32 = 16.0;
const ROW_SHA_W: f32 = 56.0;
const ROW_DATE_W: f32 = 80.0;
const ROW_CONTENT_MIN_W: f32 =
    ROW_SHA_W + ROW_AVATAR_SIZE + ROW_DATE_W + SPACE_LG * 3.0 + SPACE_2XL + SPACE_3XL;
const ROW_AVATAR_SIZE: f32 = 24.0;
const IDENTITY_AVATAR_SIZE: f32 = 32.0;
const IDENTITY_METADATA_LINE_H: f32 = 14.0;
const METADATA_LABEL_W: f32 = 72.0;
const PREVIEW_ANCHOR_MAX: f32 = 64.0;
const PREVIEW_MIN_W: f32 = 120.0;

#[derive(Clone)]
struct HistoryPanHandle {
    offset: std::rc::Rc<std::cell::Cell<f32>>,
    extent: f32,
    scale: f32,
    width: f32,
}

impl gpui_component::scroll::ScrollbarHandle for HistoryPanHandle {
    fn offset(&self) -> gpui::Point<gpui::Pixels> {
        gpui::point(
            gpui::px(-self.offset.get().clamp(0., self.extent) * self.scale),
            gpui::px(0.),
        )
    }

    fn set_offset(&self, offset: gpui::Point<gpui::Pixels>) {
        self.offset
            .set((-f32::from(offset.x) / self.scale).clamp(0., self.extent));
    }

    fn content_size(&self) -> gpui::Size<gpui::Pixels> {
        gpui::size(
            gpui::px((self.width + self.extent) * self.scale),
            gpui::px(0.),
        )
    }
}

fn file_register_width(canvas_width: f32, preferred: Option<f32>) -> f32 {
    let max = (canvas_width * DETAIL_FILES_RESIZE_MAX_FRAC).max(1.0);
    let min = DETAIL_FILES_MIN_W.min(max);
    preferred
        .unwrap_or_else(|| DETAIL_FILES_W.min(canvas_width * DETAIL_FILES_MAX_FRAC))
        .clamp(min, max)
}

fn lane_color(lane: usize) -> gpui::Hsla {
    // Approved topology palette: never status or selected-state semantics.
    match lane % 3 {
        0 => c::BLUE(),
        1 => c::AMBER(),
        _ => c::RED(),
    }
}
fn refs_label(commit: &Commit) -> String {
    commit
        .refs
        .iter()
        .map(|reference| match reference.kind {
            RefKind::Head => reference.name.clone(),
            RefKind::Branch => format!("Branch: {}", reference.name),
            RefKind::Tag => format!("Tag: {}", reference.name),
            RefKind::Remote => format!("Remote-tracking (local): {}", reference.name),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}
fn file_identity(
    status: &Status,
    path: &str,
    rename_source: Option<&str>,
) -> (&'static str, String) {
    if let Some(from) = rename_source {
        return (
            if matches!(status, Status::Binary) {
                "Renamed · Binary"
            } else {
                "Renamed"
            },
            format!("{from} → {path}"),
        );
    }
    match status {
        Status::Added => ("Added", path.into()),
        Status::Deleted => ("Deleted", path.into()),
        Status::Modified => ("Modified", path.into()),
        Status::Renamed { from } => ("Renamed", format!("{from} → {path}")),
        Status::Binary => ("Binary", path.into()),
        Status::Untracked => ("Untracked", path.into()),
    }
}
/// Preserve Git's recorded timezone, with a human-readable calendar date.
pub(super) fn readable_date(iso: &str, compact: bool) -> String {
    let Some((date, time)) = iso.split_once('T') else {
        return iso.into();
    };
    let parts: Vec<_> = date.split('-').collect();
    if parts.len() != 3 || !time.is_ascii() || time.len() < 8 {
        return iso.into();
    }
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Some(month) = parts[1]
        .parse::<usize>()
        .ok()
        .and_then(|m| m.checked_sub(1))
        .and_then(|m| months.get(m))
    else {
        return iso.into();
    };
    let day = parts[2]
        .parse::<u8>()
        .map_or_else(|_| parts[2].to_owned(), |d| d.to_string());
    let zone = &time[8..];
    let zone = if zone == "Z" {
        "UTC".to_owned()
    } else {
        format!("UTC{zone}")
    };
    format!(
        "{day} {month} {} · {} ({zone})",
        parts[0],
        &time[..if compact { 5 } else { 8 }]
    )
}
fn avatar(name: &str, size: f32) -> gpui::Div {
    let initials: String = name
        .split_whitespace()
        .filter_map(|s| s.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    div()
        .debug_selector(|| "history-identity-avatar".into())
        .size(rpx(size))
        .flex_shrink_0()
        .rounded_full()
        .bg(c::alpha(c::FG(), 0.12))
        .border_1()
        .border_color(c::BORDER())
        .text_color(c::FG())
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_size(rpx(TEXT_MICRO))
        .flex()
        .items_center()
        .justify_center()
        .when(initials.is_empty(), |avatar| {
            avatar.child(icon("person", ICON_SM, c::FG_DIM()))
        })
        .when(!initials.is_empty(), |avatar| avatar.child(initials))
}
/// Calendar dates belong in rows; the recorded time and timezone stay in commit details.
pub(super) fn calendar_date(iso: &str) -> String {
    let readable = readable_date(iso, false);
    readable
        .split_once(" · ")
        .map_or_else(|| readable.clone(), |(date, _)| date.to_owned())
}
/// Fixed SHA, initials, subject and date slots share one vertically centered row.
pub(super) fn commit_identity(commit: &Commit) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .h(rpx(COMMIT_ROW_H))
        .min_w_0()
        .flex_1()
        .child(
            div()
                .debug_selector(|| "history-identity-sha".into())
                .w(rpx(ROW_SHA_W))
                .flex_shrink_0()
                .text_size(rpx(TEXT_SMALL))
                .line_height(rpx(ROW_SUBJECT_LINE_H))
                .text_color(c::FG_DIM())
                .child(commit.id.chars().take(7).collect::<String>()),
        )
        .child(avatar(&commit.author_name, ROW_AVATAR_SIZE))
        .child(
            div()
                .debug_selector(|| "history-identity-subject".into())
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(rpx(TEXT_BODY))
                .font_weight(gpui::FontWeight::MEDIUM)
                .line_height(rpx(ROW_SUBJECT_LINE_H))
                .child(commit.subject.clone()),
        )
        .child(
            div()
                .debug_selector(|| "history-identity-date".into())
                .w(rpx(ROW_DATE_W))
                .flex_shrink_0()
                .text_size(rpx(TEXT_MICRO))
                .line_height(rpx(ROW_SUBJECT_LINE_H))
                .text_color(c::FG_DIM())
                .text_right()
                .child(calendar_date(&commit.author_date)),
        )
}
fn detailed_identity(name: &str, email: &str, date: &str, role: Option<&str>) -> gpui::Div {
    div()
        .flex()
        .items_start()
        .gap(rpx(SPACE_LG))
        .min_w_0()
        .child(avatar(name, IDENTITY_AVATAR_SIZE))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .flex_1()
                .when_some(role, |body, role| {
                    body.child(
                        div()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child(role.to_owned()),
                    )
                })
                .child(
                    div()
                        .text_size(rpx(TEXT_BODY))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .line_height(rpx(ROW_SUBJECT_LINE_H))
                        .child(name.to_owned()),
                )
                .child(
                    div()
                        .text_size(rpx(TEXT_MICRO))
                        .text_color(c::FG_DIM())
                        .line_height(rpx(IDENTITY_METADATA_LINE_H))
                        .child(email.to_owned()),
                )
                .child(
                    div()
                        .text_size(rpx(TEXT_MICRO))
                        .text_color(c::FG_DIM())
                        .line_height(rpx(IDENTITY_METADATA_LINE_H))
                        .child(readable_date(date, false)),
                ),
        )
}
fn metadata_value(id: String, label: &str, value: String) -> gpui::Div {
    div()
        .flex()
        .items_start()
        .min_w_0()
        .gap(rpx(SPACE_LG))
        .child(
            div()
                .w(rpx(METADATA_LABEL_W))
                .flex_shrink_0()
                .text_size(rpx(TEXT_MICRO))
                .line_height(rpx(METADATA_LINE_H))
                .text_color(c::FG_DIM())
                .child(label.to_owned()),
        )
        .child(div().min_w_0().flex_1().child(literal_message(&id, &value)))
}
fn literal_message(id: &str, message: &str) -> gpui::Div {
    let mut body = div().flex().flex_col().min_w_0();
    for (index, line) in message.split('\n').enumerate() {
        body = body.child(
            div().min_h(rpx(METADATA_LINE_H)).child(
                crate::views::components::readonly_value(format!("{id}-{index}"), line.to_owned())
                    .text_size(rpx(TEXT_BODY))
                    .line_height(rpx(METADATA_LINE_H)),
            ),
        );
    }
    body
}

/// Git's first paragraph is the subject; retain the literal raw message in metadata.
fn commit_body<'a>(subject: &str, message: &'a str) -> &'a str {
    let trimmed = message.trim_end_matches('\n');
    if trimmed.split_whitespace().eq(subject.split_whitespace()) {
        return "";
    }
    if let Some((first, body)) = trimmed.split_once("\n\n") {
        if first.split_whitespace().eq(subject.split_whitespace()) {
            return body.trim_start_matches('\n');
        }
    }
    trimmed
}
fn line_style(kind: LineKind) -> (&'static str, gpui::Hsla, gpui::Hsla) {
    match kind {
        LineKind::Add => ("+", c::DIFF_ADD_BG(), c::GREEN()),
        LineKind::Del => ("−", c::DIFF_DEL_BG(), c::RED()),
        LineKind::Context => (" ", c::BG(), c::FG()),
    }
}
fn diff_hunk(text: String) -> gpui::Div {
    div()
        .h(rpx(DIFF_BODY_LINE_H))
        .px(rpx(SPACE_LG))
        .overflow_hidden()
        .whitespace_nowrap()
        .bg(c::SURFACE_RAISED())
        .text_color(c::BLUE())
        .child(format!("@@ {text}"))
}
fn split_patch_row(row: &grove_core::diff::PairedRow, pan: f32, widest: f32) -> AnyElement {
    match row {
        grove_core::diff::PairedRow::HunkHeader(text) => diff_hunk(text.clone()).into_any_element(),
        grove_core::diff::PairedRow::Lines { old, new } => div()
            .debug_selector(|| "history-split-row".into())
            .w_full()
            .flex()
            .h(rpx(DIFF_BODY_LINE_H))
            .child(
                split_cell(old.as_ref(), true, pan, widest)
                    .border_l_1()
                    .border_color(c::alpha(c::BORDER(), 0.)),
            )
            .child(
                split_cell(new.as_ref(), false, pan, widest)
                    .border_l_1()
                    .border_color(c::BORDER()),
            )
            .into_any_element(),
    }
}
fn unified_patch_row(row: &grove_core::render_rows::UnifiedRenderRow, width: f32) -> AnyElement {
    match row {
        grove_core::render_rows::UnifiedRenderRow::HunkHeader(text) => {
            diff_hunk(text.clone()).min_w(rpx(width)).into_any_element()
        }
        grove_core::render_rows::UnifiedRenderRow::Line { line, .. } => {
            let (prefix, bg, color) = line_style(line.kind);
            div()
                .debug_selector(|| "history-unified-row".into())
                .h(rpx(DIFF_BODY_LINE_H))
                .min_w(rpx(width))
                .w_full()
                .whitespace_nowrap()
                .px(rpx(SPACE_LG))
                .bg(bg)
                .text_color(color)
                .flex()
                .child(
                    div()
                        .w(rpx(DIFF_NUMBER_W))
                        .flex_shrink_0()
                        .text_right()
                        .pr(rpx(SPACE_LG))
                        .text_color(c::FG_DIM())
                        .child(line.old_no.map_or(String::new(), |n| n.to_string())),
                )
                .child(
                    div()
                        .w(rpx(DIFF_NUMBER_W))
                        .flex_shrink_0()
                        .text_right()
                        .pr(rpx(SPACE_LG))
                        .text_color(c::FG_DIM())
                        .child(line.new_no.map_or(String::new(), |n| n.to_string())),
                )
                .child(div().w(rpx(SPACE_2XL)).flex_shrink_0().child(prefix))
                .child(line.text.clone())
                .into_any_element()
        }
    }
}

fn split_cell(line: Option<&grove_core::diff::Line>, old: bool, pan: f32, width: f32) -> gpui::Div {
    let (sign, bg, color) = line.map_or((" ", c::BG_RAIL(), c::FG_DIM()), |line| {
        line_style(line.kind)
    });
    let number = line
        .and_then(|line| if old { line.old_no } else { line.new_no })
        .map_or(String::new(), |n| n.to_string());
    div()
        .when(line.is_none(), |cell| {
            cell.debug_selector(|| "history-split-spacer".into())
        })
        .flex()
        .flex_1()
        .flex_basis(gpui::px(0.0))
        .min_w_0()
        .h(rpx(DIFF_BODY_LINE_H))
        .overflow_hidden()
        .bg(bg)
        .text_color(color)
        .child(
            div()
                .w(rpx(DIFF_NUMBER_W))
                .flex_shrink_0()
                .text_right()
                .pr(rpx(SPACE_LG))
                .text_color(c::FG_DIM())
                .child(number),
        )
        .child(
            div()
                .w(rpx(DIFF_SIGN_W))
                .flex_shrink_0()
                .text_center()
                .child(sign),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .left(rpx(-pan))
                        .w(rpx(width.max(1.0)))
                        .whitespace_nowrap()
                        .child(line.map_or(String::new(), |line| line.text.clone())),
                ),
        )
}

fn graph(commit: &Commit, incoming: &[GraphEdge], width: f32) -> impl IntoElement {
    let incoming = incoming.to_vec();
    let edges = commit.edges.clone();
    let lane = commit.lane;
    let color_lane = commit.color_lane;
    gpui::canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let unit = rpx(GRAPH_LANE_W).to_pixels(window.rem_size());
            let inset = rpx(GRAPH_INSET).to_pixels(window.rem_size());
            let mid = bounds.top() + bounds.size.height / 2.0;
            let x = |lane: usize| bounds.left() + inset + unit * lane as f32;
            // Each edge spans the previous node to the current node; split at row boundary.
            for edge in &incoming {
                let mut path =
                    gpui::PathBuilder::stroke(rpx(GRAPH_STROKE).to_pixels(window.rem_size()));
                let halfway = (x(edge.from_lane) + x(edge.to_lane)) / 2.0;
                path.move_to(gpui::point(halfway, bounds.top()));
                path.cubic_bezier_to(
                    gpui::point(x(edge.to_lane), mid),
                    gpui::point(x(edge.to_lane), bounds.top() + bounds.size.height / 4.0),
                    gpui::point(x(edge.to_lane), mid - bounds.size.height / 4.0),
                );
                if let Ok(path) = path.build() {
                    window.paint_path(path, lane_color(edge.color_lane));
                }
            }
            for edge in &edges {
                let mut path =
                    gpui::PathBuilder::stroke(rpx(GRAPH_STROKE).to_pixels(window.rem_size()));
                let halfway = (x(edge.from_lane) + x(edge.to_lane)) / 2.0;
                path.move_to(gpui::point(x(edge.from_lane), mid));
                path.cubic_bezier_to(
                    gpui::point(halfway, bounds.bottom()),
                    gpui::point(x(edge.from_lane), mid + bounds.size.height / 4.0),
                    gpui::point(
                        x(edge.from_lane),
                        bounds.bottom() - bounds.size.height / 4.0,
                    ),
                );
                if let Ok(path) = path.build() {
                    window.paint_path(path, lane_color(edge.color_lane));
                }
            }
            let center = gpui::point(x(lane), mid);
            let radius = rpx(GRAPH_NODE_RADIUS).to_pixels(window.rem_size());
            window.paint_quad(
                gpui::fill(
                    gpui::Bounds::new(
                        center - gpui::point(radius, radius),
                        gpui::size(radius * 2.0, radius * 2.0),
                    ),
                    c::BG(),
                )
                .corner_radii(radius),
            );
            let mut node =
                gpui::PathBuilder::stroke(rpx(GRAPH_NODE_STROKE).to_pixels(window.rem_size()));
            for step in 0..=24 {
                let angle = std::f32::consts::TAU * step as f32 / 24.0;
                let point = gpui::point(
                    center.x + radius * angle.cos(),
                    center.y + radius * angle.sin(),
                );
                if step == 0 {
                    node.move_to(point);
                } else {
                    node.line_to(point);
                }
            }
            if let Ok(node) = node.build() {
                window.paint_path(node, lane_color(color_lane));
            }
        },
    )
    .w(rpx(width))
    .h(rpx(COMMIT_ROW_H))
    .flex_shrink_0()
}

#[cfg(test)]
thread_local! {
    static COMMIT_FOCUS_TARGETS: std::cell::RefCell<Vec<gpui::FocusHandle>> = const { std::cell::RefCell::new(Vec::new()) };
    static MATERIALIZED_COMMITS: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[derive(Default)]
struct CommitListControls {
    focus: std::collections::HashMap<String, gpui::FocusHandle>,
    scroll: gpui::ScrollHandle,
    revealed: Option<(String, gpui::Pixels, gpui::Size<gpui::Pixels>)>,
}

impl Sidebar {
    pub(super) fn pin_history_card(&mut self) {
        self.history_hover.intent = None;
        self.history_hover.grace = None;
        self.history_hover.pinned = true;
    }
    pub(super) fn commit_row_hover(&mut self, id: String, hovered: bool, cx: &mut Context<Self>) {
        if self.history_hover.pinned {
            return;
        }
        let Some(history) = self.history.clone() else {
            return;
        };
        let state = history.read(cx);
        if state.expanded || state.loading {
            return;
        }
        let generation = state.snapshot_generation();
        let selected = state.selected.clone();
        if !hovered && self.history_hover.hovered_row.as_ref() != Some(&id) {
            return;
        }
        self.history_hover.intent = None;
        self.history_hover.hovered_row = hovered.then_some(id.clone());
        if hovered && selected.as_ref() == Some(&id) {
            self.history_hover.grace = None;
            return;
        }
        if selected.is_some() {
            self.schedule_commit_dismiss(cx);
        }
        if !hovered {
            return;
        }
        let selection = self.selection.clone();
        let workspace = self.active_workspace;
        self.history_hover.intent = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HISTORY_HOVER_INTENT).await;
            let _ = this.update(cx, |this, cx| {
                let valid = this.history.as_ref() == Some(&history)
                    && this.selection == selection
                    && this.active_workspace == workspace
                    && !this.history_hover.pinned
                    && !this.history_hover.card_hovered
                    && this.history_hover.hovered_row.as_ref() == Some(&id)
                    && history.read(cx).snapshot_generation() == generation
                    && !history.read(cx).loading
                    && history.read(cx).snapshot.as_ref().is_some_and(|snapshot| {
                        snapshot.commits.iter().any(|commit| commit.id == id)
                    });
                if valid {
                    this.history_hover.intent = None;
                    this.history_hover.grace = None;
                    history.update(cx, |history, cx| history.select(id, cx));
                }
            });
        }));
    }
    fn schedule_commit_dismiss(&mut self, cx: &mut Context<Self>) {
        self.history_hover.grace = None;
        if self.history_hover.pinned || self.history_hover.card_hovered {
            return;
        }
        let Some(history) = self.history.clone() else {
            return;
        };
        let Some(selected) = history.read(cx).selected.clone() else {
            return;
        };
        let generation = history.read(cx).snapshot_generation();
        let selection = self.selection.clone();
        let workspace = self.active_workspace;
        self.history_hover.grace = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HISTORY_HOVER_GRACE).await;
            let _ = this.update(cx, |this, cx| {
                if this.history.as_ref() == Some(&history)
                    && this.selection == selection
                    && this.active_workspace == workspace
                    && !this.history_hover.pinned
                    && !this.history_hover.card_hovered
                    && this.history_hover.hovered_row.as_ref() != Some(&selected)
                    && history.read(cx).snapshot_generation() == generation
                    && !history.read(cx).expanded
                    && history.read(cx).selected.as_ref() == Some(&selected)
                {
                    history.update(cx, BranchHistoryState::dismiss_preview);
                    this.history_hover.grace = None;
                }
            });
        }));
    }
    pub(super) fn cancel_history_hover_intent(&mut self, cx: &mut Context<Self>) {
        self.history_hover.intent = None;
        self.history_hover.hovered_row = None;
        self.schedule_commit_dismiss(cx);
    }
    pub(super) fn commit_card_hover(&mut self, hovered: bool, cx: &mut Context<Self>) {
        self.history_hover.card_hovered = hovered;
        if hovered {
            self.history_hover.intent = None;
            self.history_hover.grace = None;
        } else {
            self.schedule_commit_dismiss(cx);
        }
    }
    fn render_history_commit(
        &self,
        state: &BranchHistoryState,
        index: usize,
        graph_w: f32,
        focus: &gpui::FocusHandle,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(snapshot) = &state.snapshot else {
            return div().into_any_element();
        };
        #[cfg(test)]
        MATERIALIZED_COMMITS.with(|rows| rows.borrow_mut().push(index));
        let commit = &snapshot.commits[index];

        let selected = state.selected.as_ref() == Some(&commit.id);
        let id = commit.id.clone();
        let hover_id = id.clone();
        let incoming = index
            .checked_sub(1)
            .map_or(&[][..], |prev| snapshot.commits[prev].edges.as_slice());
        let mut row = self
            .control(
                format!("history-commit-{id}"),
                format!(
                    "{} by {} <{}>. {}. Commit {}",
                    commit.subject,
                    commit.author_name,
                    commit.author_email,
                    readable_date(&commit.author_date, false),
                    commit.id
                ),
                Action::SelectCommit(id.clone()),
                cx,
            )
            .debug_selector(|| "history-commit-row".into())
            .track_focus(focus)
            .tab_index(0)
            .relative()
            .w_full()
            .min_w(rpx(graph_w + ROW_CONTENT_MIN_W))
            .h(rpx(COMMIT_ROW_H))
            .justify_start()
            .rounded_none()
            .gap_0()
            .when(selected, |row| row.bg(c::BG_HL()).text_color(c::FG()))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.commit_row_hover(hover_id.clone(), *hovered, cx);
            }))
            .child(
                div()
                    .debug_selector(|| "history-row-graph".into())
                    .w(rpx(graph_w))
                    .h(rpx(COMMIT_ROW_H))
                    .flex_shrink_0()
                    .child(graph(commit, incoming, graph_w)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pr(rpx(SPACE_2XL))
                    .child(commit_identity(commit)),
            );
        if selected && !state.expanded {
            let left = graph_w.min(PREVIEW_ANCHOR_MAX);
            let available =
                Self::logical_window_width(window) - self.rail_width(window, cx) - left - SPACE_2XL;
            let mut card = div()
                .id("history-commit-preview")
                .debug_selector(|| "history-commit-preview".into())
                .occlude()
                .w(rpx(PREVIEW_W.min(available.max(PREVIEW_MIN_W))))
                .max_w_full()
                .rounded(rpx(RADIUS_PANEL))
                .border_1()
                .border_color(c::BORDER())
                .bg(c::BG())
                .shadow_md()
                .cursor_default()
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    this.commit_card_hover(*hovered, cx);
                }))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.pin_history_card();
                        cx.stop_propagation();
                    }),
                )
                .child(div().p(rpx(SPACE_2XL)).child(detailed_identity(
                    &commit.author_name,
                    &commit.author_email,
                    &commit.author_date,
                    None,
                )))
                .child(
                    div()
                        .id("history-preview-message")
                        .max_h(rpx(PREVIEW_MESSAGE_H))
                        .overflow_y_scroll()
                        .px(rpx(SPACE_2XL))
                        .pb(rpx(SPACE_2XL))
                        .text_size(rpx(TEXT_BODY))
                        .child(
                            div()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(commit.id[..7].to_owned()),
                        )
                        .when(!commit.refs.is_empty(), |body| {
                            body.child(
                                div()
                                    .py(rpx(SPACE_SM))
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child(refs_label(commit)),
                            )
                        })
                        .child(literal_message(
                            "history-preview-message-line",
                            &commit.message,
                        )),
                );
            let mut open_diff = self
                .control(
                    "history-open-commit",
                    "Open changed files and diff",
                    Action::ExpandCommit,
                    cx,
                )
                .w_full()
                .h_auto()
                .min_h(rpx(CONTROL_H))
                .justify_start()
                .px(rpx(SPACE_2XL))
                .py(rpx(SPACE_LG))
                .border_t_1()
                .border_color(c::BORDER())
                .child(icon("chev-right", ICON_SM, c::FG_DIM()));
            if let Some(details) = &state.details {
                let (added, removed) = details.files.iter().fold((0u64, 0u64), |(a, r), f| {
                    (a + u64::from(f.added), r + u64::from(f.removed))
                });
                open_diff = open_diff
                    .child(format!(
                        "{} {} changed",
                        details.files.len(),
                        if details.files.len() == 1 {
                            "file"
                        } else {
                            "files"
                        }
                    ))
                    .child(div().text_color(c::GREEN()).child(format!("+{added}")))
                    .child(div().text_color(c::RED()).child(format!("−{removed}")));
            } else {
                card = card.child(
                    div()
                        .px(rpx(SPACE_2XL))
                        .pb(rpx(SPACE_LG))
                        .text_size(rpx(TEXT_SMALL))
                        .child(
                            state
                                .detail_error
                                .clone()
                                .unwrap_or_else(|| "Loading changed files…".into()),
                        ),
                );
                open_diff = open_diff.child("Open changed files");
            }
            card = card.child(open_diff);
            // The bridge belongs to the row: crossing from subject to card keeps the preview open.
            row = row.child(gpui::deferred(
                div()
                    .absolute()
                    .left(rpx(left))
                    .top(rpx(COMMIT_ROW_H - SPACE_SM))
                    .pt(rpx(SPACE_SM))
                    .max_w_full()
                    .child(card),
            ));
        }

        row.into_any_element()
    }

    pub(super) fn render_history(
        &self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(entity) = &self.history else {
            return div().into_any_element();
        };
        let state = entity.read(cx).clone();
        let label = state.snapshot.as_ref().map_or_else(
            || state.checkout.clone(),
            |s| match &s.head {
                HeadIdentity::Branch {
                    name,
                    oid: Some(oid),
                } => format!("{name} · {}", &oid[..7]),
                HeadIdentity::Branch { name, oid: None } => format!("{name} · No commits yet"),
                HeadIdentity::Detached { oid } => format!("Detached HEAD · {}", &oid[..7]),
            },
        );
        let mut list = div()
            .id("history-commits")
            .debug_selector(|| "history-commits".into())
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .overflow_x_scroll()
            .on_scroll_wheel(cx.listener(|this, _, _, cx| this.cancel_history_hover_intent(cx)));
        if state.loading {
            list = list.child(div().p(rpx(SPACE_2XL)).child("Loading commit history…"));
        } else if let Some(error) = &state.error {
            list = list.child(div().p(rpx(SPACE_2XL)).child(error.clone()).child(
                self.history_action(
                    "history-retry",
                    "Retry",
                    "refresh",
                    Action::RefreshHistory,
                    window,
                    cx,
                ),
            ));
        } else if let Some(snapshot) = &state.snapshot {
            if snapshot.commits.is_empty() {
                list = list.child(
                    div()
                        .p(rpx(SPACE_2XL))
                        .child("No commits yet. Your first commit will appear here."),
                );
            }
            let lane_count = snapshot
                .commits
                .iter()
                .flat_map(|commit| {
                    commit
                        .edges
                        .iter()
                        .map(|edge| edge.from_lane.max(edge.to_lane))
                })
                .max()
                .unwrap_or(0)
                + 1;
            let graph_w = GRAPH_INSET * 2.0 + GRAPH_LANE_W * lane_count as f32;
            let controls = window.use_keyed_state("history-commit-controls", cx, |_, _| {
                CommitListControls::default()
            });
            let (focuses, scroll) = controls.update(cx, |controls, cx| {
                let live: std::collections::HashSet<_> =
                    snapshot.commits.iter().map(|commit| &commit.id).collect();
                controls.focus.retain(|id, _| live.contains(id));
                let focuses: Vec<_> = snapshot
                    .commits
                    .iter()
                    .map(|commit| {
                        controls
                            .focus
                            .entry(commit.id.clone())
                            .or_insert_with(|| cx.focus_handle().tab_stop(true).tab_index(0))
                            .clone()
                    })
                    .collect();
                if !state.expanded && self.history_restore_commit_focus.replace(false) {
                    if let Some(focus) = state
                        .selected
                        .as_ref()
                        .and_then(|id| controls.focus.get(id))
                    {
                        focus.focus(window, cx);
                    }
                }
                let focused = focuses.iter().position(|focus| focus.is_focused(window));
                if let Some(index) = focused {
                    let signature = (
                        snapshot.commits[index].id.clone(),
                        window.rem_size(),
                        controls.scroll.bounds().size,
                    );
                    if controls.revealed.as_ref() != Some(&signature) {
                        controls.scroll.scroll_to_item(index);
                        controls.revealed = Some(signature);
                    }
                } else {
                    controls.revealed = None;
                }
                (focuses, controls.scroll.clone())
            });
            #[cfg(test)]
            COMMIT_FOCUS_TARGETS.with(|targets| targets.replace(focuses.clone()));
            let row_state = std::rc::Rc::new(state.clone());
            for (index, focus) in focuses.into_iter().enumerate() {
                list = list.child(commit_slot::CommitSlot::new(
                    cx.entity().downgrade(),
                    row_state.clone(),
                    index,
                    graph_w,
                    focus,
                ));
            }
            if snapshot.has_more {
                list = list.child(div().h(rpx(COMMIT_ROW_H)).px(rpx(SPACE_2XL)).child(if state.limit < MAX_HISTORY {
                    self.history_action("history-load-more", "Load older commits", "chev-down", Action::MoreHistory, window, cx).into_any_element()
                } else { div().truncate().text_color(c::FG_DIM()).child("Showing the newest 500 reachable commits. Parent lanes continue beyond this limit.").into_any_element() }));
            }
            list = list
                .track_scroll(&scroll)
                .child(div().h(rpx(PREVIEW_BOTTOM_SPACE)));
        }
        let expanded = state.expanded;
        let mut canvas = div()
            .id("branch-history-viewer")
            .role(gpui::Role::Dialog)
            .aria_label("Commit history")
            .debug_selector(|| "branch-history-viewer".into())
            .absolute()
            .inset_0()
            .occlude()
            .track_focus(&self.history_focus)
            .bg(c::BG())
            .text_color(c::FG())
            .font_family(crate::fonts::UI_FAMILY)
            .flex()
            .flex_col()
            .when(!expanded, |canvas| {
                canvas.child(
                    crate::views::components::content_header()
                        .p(rpx(SPACE_2XL))
                        .border_b_1()
                        .border_color(c::BORDER())
                        .child(self.history_action(
                            "history-back",
                            "Back to checkout",
                            "arrow-left",
                            Action::CloseHistory,
                            window,
                            cx,
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_LG))
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .debug_selector(|| "history-header-title".into())
                                        .flex_shrink_0()
                                        .text_size(rpx(TEXT_TITLE))
                                        .child("Commit history"),
                                )
                                .child(
                                    div()
                                        .debug_selector(|| "history-header-branch".into())
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(c::FG_DIM())
                                        .child(label),
                                ),
                        )
                        .child(self.history_action(
                            "history-refresh",
                            "Refresh",
                            "refresh",
                            Action::RefreshHistory,
                            window,
                            cx,
                        )),
                )
            });
        if expanded {
            canvas = canvas.child(self.render_history_details(window, cx));
        } else {
            canvas = canvas.child(list);
        }
        canvas.into_any_element()
    }
    pub(super) fn history_action(
        &self,
        id: &'static str,
        label: &'static str,
        glyph: &'static str,
        action: Action,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> gpui_component::button::Button {
        let key_action = action.clone();
        crate::views::components::dense_action_with_colors(
            id,
            label,
            gpui::transparent_black(),
            c::FG(),
            c::BG_HOVER(),
            window,
            cx,
        )
        .when(!glyph.is_empty(), |button| {
            button.icon(gpui_component::Icon::default().path(format!("icons/{glyph}.svg")))
        })
        .focus_visible(|style| style.bg(c::BG_HOVER()))
        .on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.act(action.clone(), window, cx);
        }))
        .on_key_down(
            cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    this.act(key_action.clone(), window, cx);
                }
            }),
        )
    }
    fn history_read_error(
        title: &'static str,
        error: &str,
        retry: gpui_component::button::Button,
    ) -> AnyElement {
        div()
            .p(rpx(SPACE_2XL))
            .flex()
            .flex_col()
            .items_start()
            .gap(rpx(SPACE_SM))
            .child(
                div()
                    .text_size(rpx(TEXT_BODY))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(title),
            )
            .child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(error.to_owned()),
            )
            .child(retry.border_1().border_color(c::BORDER()))
            .into_any_element()
    }

    fn history_split_scrollbar(
        &self,
        state: &BranchHistoryState,
        pan: f32,
        extent: f32,
        width: f32,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        use gpui_component::scroll::{Scrollbar, ScrollbarShow};
        let offset = state.pan_x.clone();
        let handle = HistoryPanHandle {
            offset: offset.clone(),
            extent,
            scale,
            width: (width - SPLIT_SCROLL_LABEL_W).max(1.),
        };
        div()
            .id("history-split-scroll")
            .debug_selector(|| "history-split-scroll".into())
            .role(gpui::Role::ScrollBar)
            .aria_label("Horizontal scroll for both diff panes")
            .aria_min_numeric_value(0.)
            .aria_max_numeric_value(f64::from(extent))
            .aria_orientation(gpui::Orientation::Horizontal)
            .aria_numeric_value(f64::from(pan))
            .tab_index(0)
            .flex()
            .items_center()
            .flex_shrink_0()
            .h(rpx(CONTROL_H))
            .bg(c::SURFACE_RAISED())
            .border_t_1()
            .border_color(c::BORDER())
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .child(
                div()
                    .w(rpx(SPLIT_SCROLL_LABEL_W))
                    .flex_shrink_0()
                    .pl(rpx(SPACE_LG))
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .child("Horizontal scroll"),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .h_full()
                    .child(Scrollbar::horizontal(&handle).scrollbar_show(ScrollbarShow::Always)),
            )
            .on_key_down(cx.listener(move |_, event: &gpui::KeyDownEvent, _, cx| {
                let next = match event.keystroke.key.as_str() {
                    "left" => pan - DETAIL_FILES_KEY_STEP,
                    "right" => pan + DETAIL_FILES_KEY_STEP,
                    "home" => 0.,
                    "end" => extent,
                    _ => return,
                };
                offset.set(next.clamp(0., extent));
                cx.notify();
                cx.stop_propagation();
            }))
    }

    fn render_history_details(&self, window: &gpui::Window, cx: &mut Context<Self>) -> AnyElement {
        use crate::entities::branch_history::HistoryDiffMode;
        let Some(entity) = &self.history else {
            return div().into_any_element();
        };
        let state = entity.read(cx).clone();
        let commit = state.snapshot.as_ref().and_then(|s| {
            s.commits
                .iter()
                .find(|c| Some(&c.id) == state.selected.as_ref())
        });
        let Some(commit) = commit else {
            return div()
                .p(rpx(SPACE_2XL))
                .child("Commit no longer available. Return to history.")
                .into_any_element();
        };
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let canvas_width = self
            .selection
            .as_ref()
            .and_then(|s| match s {
                super::Selection::Session(id) => self.canvas_bounds.get(id),
                _ => None,
            })
            .map_or_else(
                || Self::logical_window_width(window),
                |bounds| f32::from(bounds.get().size.width) / scale,
            );
        let file_width = if canvas_width <= DETAIL_NARROW_W {
            0.
        } else {
            file_register_width(canvas_width, state.file_width)
        };
        let half_width =
            ((canvas_width - file_width) / 2.0 - DIFF_NUMBER_W - DIFF_SIGN_W).max(SPACE_2XL);
        let narrow = canvas_width <= DETAIL_NARROW_W;
        let subject_on_own_row = canvas_width <= DETAIL_WRAP_TITLE_W;
        let back = self
            .history_action(
                "history-return-graph",
                "Back to history",
                "arrow-left",
                Action::HistoryGraph,
                window,
                cx,
            )
            .flex_shrink_0();
        let copy = self
            .history_action(
                "history-copy-sha",
                "Copy SHA",
                "copy",
                Action::CopyHistoryCommit,
                window,
                cx,
            )
            .label("Copy full commit SHA")
            .icon_only()
            .flex_shrink_0()
            .border_1()
            .border_color(c::BORDER())
            .child(div().child("Copy SHA"))
            .child(
                div()
                    .text_color(c::FG_DIM())
                    .child(commit.id.chars().take(7).collect::<String>()),
            );
        let subject = div()
            .debug_selector(|| "history-detail-subject".into())
            .flex_1()
            .min_w_0()
            .text_size(rpx(TEXT_TITLE))
            .font_weight(gpui::FontWeight::MEDIUM)
            .child(commit.subject.clone());
        let title_row = crate::views::components::content_header()
            .debug_selector(|| "history-detail-title-row".into())
            .child(back);
        let title_row = if subject_on_own_row {
            div()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_SM))
                .child(title_row.justify_between().child(copy))
                .child(subject.flex_none())
                .into_any_element()
        } else {
            title_row
                .child(div().w(gpui::px(1.)).h(rpx(CONTROL_H)).bg(c::BORDER()))
                .child(subject)
                .child(copy)
                .into_any_element()
        };
        let metadata_toggle = crate::views::components::focusable_inline_disclosure(
            "history-metadata-toggle",
            "Commit metadata",
        )
        .w_auto()
        .gap(rpx(SPACE_SM))
        .focus_visible(|style| style.bg(c::BG_HOVER()))
        .aria_expanded(state.metadata_expanded)
        .child(icon(
            if state.metadata_expanded {
                "chev-down"
            } else {
                "chev-right"
            },
            ICON_SM,
            c::FG_DIM(),
        ))
        .child(div().text_size(rpx(TEXT_SMALL)).child("Commit metadata"))
        .on_click(
            cx.listener(|this, _, window, cx| this.act(Action::ToggleHistoryMetadata, window, cx)),
        )
        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation();
                this.act(Action::ToggleHistoryMetadata, window, cx);
            }
        }));
        let byline = div()
            .flex()
            .items_center()
            .min_w_0()
            .gap(rpx(SPACE_MD))
            .when(!narrow, |row| row.flex_1())
            .child(avatar(&commit.author_name, ROW_AVATAR_SIZE))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_baseline()
                    .min_w_0()
                    .gap_x(rpx(SPACE_LG))
                    .child(
                        div()
                            .text_size(rpx(TEXT_BODY))
                            .child(commit.author_name.clone()),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child(readable_date(&commit.author_date, true)),
                    ),
            );
        let mut header = div()
            .id("history-detail-header")
            .debug_selector(|| "history-detail-header".into())
            .flex()
            .flex_col()
            .flex_shrink_0()
            .min_w_0()
            .px(rpx(if narrow { SPACE_LG } else { SPACE_2XL }))
            .py(rpx(SPACE_MD))
            .gap(rpx(SPACE_SM))
            .border_b_1()
            .border_color(c::BORDER())
            .child(title_row)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .when(narrow, |row| row.flex_col().items_start())
                    .child(byline)
                    .child(metadata_toggle),
            );
        let mut extra = div()
            .id("history-detail-extra")
            .max_h(rpx(DETAIL_HEADER_MAX_H))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG));
        let message = commit_body(&commit.subject, &commit.message);
        if !message.is_empty() {
            extra = extra.child(literal_message("history-detail-body", message));
        }
        if !commit.refs.is_empty() {
            extra = extra.child(
                div()
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .child(refs_label(commit)),
            );
        }
        if state.metadata_expanded {
            let mut metadata = div()
                .id("history-commit-metadata")
                .debug_selector(|| "history-commit-metadata".into())
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .min_w_0()
                .child(metadata_value(
                    "history-detail-sha".into(),
                    "Commit",
                    commit.id.clone(),
                ))
                .child(metadata_value(
                    "history-detail-author".into(),
                    "Author",
                    format!(
                        "{} <{}>\n{}",
                        commit.author_name,
                        commit.author_email,
                        readable_date(&commit.author_date, false)
                    ),
                ));
            if commit.committer_name != commit.author_name
                || commit.committer_email != commit.author_email
                || commit.committer_date != commit.author_date
            {
                metadata = metadata.child(metadata_value(
                    "history-detail-committer".into(),
                    "Committed by",
                    format!(
                        "{} <{}>\n{}",
                        commit.committer_name,
                        commit.committer_email,
                        readable_date(&commit.committer_date, false)
                    ),
                ));
            }
            for (index, parent) in commit.parents.iter().enumerate() {
                metadata = metadata.child(metadata_value(
                    format!("history-detail-parent-{index}"),
                    &format!("Parent {}", index + 1),
                    parent.clone(),
                ));
            }
            if !commit.refs.is_empty() {
                metadata = metadata.child(metadata_value(
                    "history-detail-refs".into(),
                    "Local refs",
                    refs_label(commit),
                ));
            }
            metadata = metadata.child(metadata_value(
                "history-detail-message".into(),
                "Message",
                commit.message.clone(),
            ));
            extra = extra.child(metadata);
        }
        if state.metadata_expanded || !message.is_empty() || !commit.refs.is_empty() {
            header = header.child(extra);
        }
        let semantics = match commit.parents.as_slice() {
            [] => "Compared with empty tree".into(),
            [parent] => format!("Compared with parent · {}", &parent[..7]),
            [parent, ..] => format!("Merge · first parent · {}", &parent[..7]),
        };
        let mut body = div()
            .id("history-details")
            .debug_selector(|| "history-details".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(header);
        let Some(details) = &state.details else {
            return body
                .child(if let Some(error) = &state.detail_error {
                    Self::history_read_error(
                        "Couldn’t load changed files",
                        error,
                        self.history_action(
                            "history-retry-files",
                            "Retry changed files",
                            "refresh",
                            Action::RetryHistoryFiles,
                            window,
                            cx,
                        ),
                    )
                } else {
                    div()
                        .p(rpx(SPACE_2XL))
                        .child("Loading changed files…")
                        .into_any_element()
                })
                .into_any_element();
        };
        if details.files.is_empty() {
            return body
                .child(
                    div()
                        .p(rpx(SPACE_2XL))
                        .child(
                            div()
                                .text_size(rpx(TEXT_MICRO))
                                .text_color(c::FG_DIM())
                                .child(semantics),
                        )
                        .child("No file changes in this comparison."),
                )
                .into_any_element();
        }
        let (added, removed) = details.files.iter().fold((0u64, 0u64), |(a, r), file| {
            (a + u64::from(file.added), r + u64::from(file.removed))
        });
        let files = div()
            .id("history-files")
            .debug_selector(|| "history-files".into())
            .when(!narrow, |files| {
                files.w(rpx(file_width)).overflow_y_scroll()
            })
            .when(narrow, |files| {
                files.w_full().border_b_1().border_color(c::BORDER())
            })
            .flex_shrink_0()
            .min_h_0()
            .bg(c::BG_RAIL())
            .p(rpx(SPACE_LG))
            .child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(format!(
                        "{} {} changed",
                        details.files.len(),
                        if details.files.len() == 1 {
                            "file"
                        } else {
                            "files"
                        }
                    )),
            )
            .child(
                div()
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .child(semantics),
            )
            .child(
                div()
                    .flex()
                    .gap(rpx(SPACE_LG))
                    .pb(rpx(SPACE_LG))
                    .text_size(rpx(TEXT_MICRO))
                    .child(div().text_color(c::GREEN()).child(format!("+{added}")))
                    .child(div().text_color(c::RED()).child(format!("−{removed}"))),
            );
        let mut file_rows = div()
            .id("history-file-rows")
            .flex()
            .when(narrow, |rows| rows.overflow_x_scroll())
            .when(!narrow, |rows| rows.flex_col());
        for file in &details.files {
            let (status, identity) = file_identity(
                &file.status,
                &file.path,
                details.rename_sources.get(&file.path).map(String::as_str),
            );
            let (directory, name) = file.path.rsplit_once('/').unwrap_or(("", &file.path));
            file_rows = file_rows.child(
                self.control(
                    format!("history-file-{}", file.path),
                    format!("{status}: {identity}. Open diff"),
                    Action::SelectHistoryFile(file.path.clone()),
                    cx,
                )
                .debug_selector(|| "history-file-row".into())
                .when(narrow, |row| row.w(rpx(DETAIL_FILES_MIN_W)))
                .when(!narrow, |row| row.w_full())
                .aria_selected(state.selected_file.as_ref() == Some(&file.path))
                .h_auto()
                .min_h(rpx(FILE_ROW_H))
                .justify_start()
                .px(rpx(SPACE_SM))
                .py(rpx(SPACE_SM))
                .when(state.selected_file.as_ref() == Some(&file.path), |row| {
                    row.bg(c::BG_HL()).text_color(c::FG())
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .flex_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(rpx(TEXT_SMALL))
                                        .line_height(rpx(ROW_SUBJECT_LINE_H))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .child(name.to_owned()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_shrink_0()
                                        .gap(rpx(SPACE_SM))
                                        .text_size(rpx(TEXT_MICRO))
                                        .line_height(rpx(IDENTITY_METADATA_LINE_H))
                                        .child(
                                            div()
                                                .text_color(c::GREEN())
                                                .child(format!("+{}", file.added)),
                                        )
                                        .child(
                                            div()
                                                .text_color(c::RED())
                                                .child(format!("−{}", file.removed)),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(rpx(TEXT_MICRO))
                                .line_height(rpx(IDENTITY_METADATA_LINE_H))
                                .text_color(c::FG_DIM())
                                .child(if identity != file.path {
                                    format!("{status} · {identity}")
                                } else {
                                    format!("{status} · {directory}")
                                }),
                        ),
                ),
            );
        }
        let files = files.child(file_rows);
        let mut diff = div()
            .id("history-diff-area")
            .debug_selector(|| "history-diff-area".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0();
        let mut toolbar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(rpx(SPACE_LG))
            .p(rpx(SPACE_LG))
            .border_b_1()
            .border_color(c::BORDER());
        if let Some(file) = details
            .files
            .iter()
            .find(|file| Some(&file.path) == state.selected_file.as_ref())
        {
            let (status, identity) = file_identity(
                &file.status,
                &file.path,
                details.rename_sources.get(&file.path).map(String::as_str),
            );
            toolbar = toolbar
                .child(
                    div()
                        .min_w_0()
                        .when(!narrow, |path| path.flex_1())
                        .when(narrow, |path| path.w_full())
                        .child(
                            crate::views::components::readonly_value(
                                "history-selected-file",
                                identity,
                            )
                            .text_size(rpx(TEXT_SMALL)),
                        ),
                )
                .child(
                    div()
                        .text_size(rpx(TEXT_MICRO))
                        .text_color(c::FG_DIM())
                        .child(status),
                );
        }
        toolbar = toolbar.child(
            div()
                .id("history-diff-mode")
                .role(gpui::Role::Group)
                .aria_label("Diff layout")
                .flex()
                .border_1()
                .border_color(c::BORDER())
                .rounded(rpx(RADIUS_CONTROL))
                .child(
                    self.history_action(
                        "history-mode-unified",
                        "Unified",
                        "",
                        Action::HistoryDiffMode(HistoryDiffMode::Unified),
                        window,
                        cx,
                    )
                    .selected(state.diff_mode == HistoryDiffMode::Unified),
                )
                .child(
                    self.history_action(
                        "history-mode-split",
                        "Split",
                        "",
                        Action::HistoryDiffMode(HistoryDiffMode::Split),
                        window,
                        cx,
                    )
                    .selected(state.diff_mode == HistoryDiffMode::Split),
                ),
        );
        diff = diff.child(toolbar);
        if state.diff_mode == HistoryDiffMode::Split {
            let parent = commit.parents.first().map_or_else(
                || "Empty tree".to_owned(),
                |id| {
                    format!(
                        "{} · {}",
                        if commit.parents.len() > 1 {
                            "First parent"
                        } else {
                            "Parent"
                        },
                        &id[..7]
                    )
                },
            );
            diff = diff.child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(c::BORDER())
                    .child(
                        div()
                            .debug_selector(|| "history-split-left-label".into())
                            // Both halves reserve the same border-box budget.
                            .border_l_1()
                            .border_color(c::alpha(c::BORDER(), 0.0))
                            .flex_1()
                            .flex_basis(gpui::px(0.0))
                            .min_w_0()
                            .p(rpx(SPACE_LG))
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child(parent),
                    )
                    .child(
                        div()
                            .debug_selector(|| "history-split-right-label".into())
                            .flex_1()
                            .flex_basis(gpui::px(0.0))
                            .min_w_0()
                            .p(rpx(SPACE_LG))
                            .border_l_1()
                            .border_color(c::BORDER())
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child(format!("Commit · {}", &commit.id[..7])),
                    ),
            );
        }
        let mut split_scroller = None;
        let mut patch = div()
            .id("history-patch")
            .debug_selector(|| "history-patch".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .text_size(rpx(TEXT_CODE_SMALL));
        if let Some(error) = &state.patch_error {
            patch = patch.child(Self::history_read_error(
                "Couldn’t load the patch",
                error,
                self.history_action(
                    "history-retry-patch",
                    "Retry patch",
                    "refresh",
                    Action::RetryHistoryPatch,
                    window,
                    cx,
                ),
            ));
        } else if state.patch_loading {
            patch = patch.child(div().p(rpx(SPACE_2XL)).child("Loading patch…"));
        } else if let Some(value) = &state.patch {
            match value.as_ref() {
                Patch::Binary => {
                    patch =
                        patch.child(div().p(rpx(SPACE_2XL)).child("Binary file · no text diff"));
                }
                Patch::TooLarge { added, removed } => {
                    patch = patch.child(
                        div()
                            .p(rpx(SPACE_2XL))
                            .child(format!("Diff too large to display · +{added} −{removed}")),
                    );
                }
                Patch::Text { hunks, .. } => {
                    if hunks.is_empty() {
                        patch=patch.child(div().p(rpx(SPACE_2XL)).child("No text changes. This commit may only change the filename or file mode."));
                    }
                    let font_size = rpx(TEXT_CODE_SMALL).to_pixels(window.rem_size());
                    let cached_width = *state.width_cache.borrow();
                    let (widest, unified_width) = if let Some((_, split, unified)) =
                        cached_width.filter(|(key, _, _)| *key == font_size)
                    {
                        (split, unified)
                    } else {
                        let text_width = |text: &str| {
                            let mut run = window.text_style().to_run(text.len());
                            run.font.family = crate::fonts::UI_FAMILY.into();
                            f32::from(
                                window
                                    .text_system()
                                    .shape_line(text.to_owned().into(), font_size, &[run], None)
                                    .width(),
                            ) / scale
                        };
                        let split = hunks
                            .iter()
                            .flat_map(|hunk| &hunk.lines)
                            .map(|line| text_width(&line.text))
                            .fold(0.0f32, f32::max);
                        let unified = state
                            .unified_rows
                            .iter()
                            .map(|row| match row {
                                grove_core::render_rows::UnifiedRenderRow::HunkHeader(text) => {
                                    text_width(&format!("@@ {text}")) + SPACE_LG * 2.
                                }
                                grove_core::render_rows::UnifiedRenderRow::Line {
                                    line, ..
                                } => {
                                    text_width(&line.text)
                                        + DIFF_NUMBER_W * 2.
                                        + SPACE_2XL
                                        + SPACE_LG * 2.
                                }
                            })
                            .fold(0.0f32, f32::max);
                        *state.width_cache.borrow_mut() = Some((font_size, split, unified));
                        (split, unified)
                    };
                    let eager = window.is_a11y_active() || window.is_inspector_picking(cx);
                    if state.diff_mode == HistoryDiffMode::Split {
                        let extent =
                            grove_core::render_rows::split_pan_extent(widest, widest, half_width);
                        let pan = grove_core::render_rows::clamp_split_pan(
                            state.pan_x.get(),
                            widest,
                            widest,
                            half_width,
                        );
                        if extent > 0. {
                            split_scroller = Some(self.history_split_scrollbar(
                                &state,
                                pan,
                                extent,
                                (canvas_width - file_width).max(0.),
                                scale,
                                cx,
                            ));
                        }
                        let offset = state.pan_x.clone();
                        patch = patch.on_scroll_wheel(cx.listener(
                            move |_, event: &gpui::ScrollWheelEvent, _, cx| {
                                let delta = match event.delta {
                                    gpui::ScrollDelta::Pixels(p) => f32::from(p.x) / scale,
                                    gpui::ScrollDelta::Lines(p) => p.x * DIFF_BODY_LINE_H,
                                };
                                if delta != 0.0 {
                                    offset.set((pan - delta).clamp(0., extent));
                                    cx.notify();
                                    cx.stop_propagation();
                                }
                            },
                        ));
                        let rows = state.paired_rows.clone();
                        if eager {
                            patch = patch
                                .children(rows.iter().map(|row| split_patch_row(row, pan, widest)));
                        } else {
                            patch = patch.overflow_hidden().child(
                                gpui::uniform_list(
                                    "history-split-visible-lines",
                                    rows.len(),
                                    cx.processor(move |_, range: std::ops::Range<usize>, _, _| {
                                        range
                                            .map(|index| split_patch_row(&rows[index], pan, widest))
                                            .collect::<Vec<_>>()
                                    }),
                                )
                                .h_full(),
                            );
                        }
                    } else {
                        let rows = state.unified_rows.clone();
                        if eager {
                            patch = patch.overflow_x_scroll().children(
                                rows.iter().map(|row| unified_patch_row(row, unified_width)),
                            );
                        } else {
                            patch = patch.overflow_hidden().child(
                                gpui::uniform_list(
                                    "history-unified-visible-lines",
                                    rows.len(),
                                    cx.processor(move |_, range: std::ops::Range<usize>, _, _| {
                                        range
                                            .map(|index| {
                                                unified_patch_row(&rows[index], unified_width)
                                            })
                                            .collect::<Vec<_>>()
                                    }),
                                )
                                .h_full()
                                .with_horizontal_sizing_behavior(
                                    gpui::ListHorizontalSizingBehavior::Unconstrained,
                                ),
                            );
                        }
                    }
                }
            }
        } else {
            patch = patch.child(div().p(rpx(SPACE_2XL)).child("Loading selected file…"));
        }
        diff = diff.child(patch).children(split_scroller);
        let divider = div()
            .id("history-file-divider")
            .debug_selector(|| "history-file-divider".into())
            .role(gpui::Role::Splitter)
            .aria_label("Resize changed files list. Use Left and Right arrows.")
            .tab_index(0)
            .track_focus(&self.history_file_resize_focus)
            .w(rpx(DIVIDER_DRAG_HIT_W))
            .mx(rpx(-DIVIDER_DRAG_HIT_W / 2.0))
            .h_full()
            .flex_shrink_0()
            .flex()
            .justify_center()
            .cursor_col_resize()
            .child(div().w(gpui::px(1.0)).h_full().bg(c::BORDER()))
            .hover(|style| style.bg(c::BG_HOVER()))
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    this.history_file_resize_focus.focus(window, cx);
                    if let Some(history) = &this.history {
                        history.update(cx, |history, _| {
                            history.file_resize_drag =
                                Some((f32::from(event.position.x), file_width));
                        });
                    }
                    cx.stop_propagation();
                }),
            )
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.modifiers.platform
                    || event.keystroke.modifiers.control
                    || event.keystroke.modifiers.alt
                {
                    return;
                }
                let delta = match event.keystroke.key.as_str() {
                    "left" => -DETAIL_FILES_KEY_STEP,
                    "right" => DETAIL_FILES_KEY_STEP,
                    _ => return,
                };
                if let Some(history) = &this.history {
                    history.update(cx, |history, cx| {
                        history.file_width =
                            Some(file_register_width(canvas_width, Some(file_width + delta)));
                        cx.notify();
                    });
                }
                cx.stop_propagation();
            }));
        body = body.child(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .on_mouse_move(cx.listener(
                    move |this, event: &gpui::MouseMoveEvent, window, cx| {
                        let Some(history) = &this.history else { return };
                        let Some((origin, initial)) = history.read(cx).file_resize_drag else {
                            return;
                        };
                        if !event.dragging() {
                            history.update(cx, |history, _| history.file_resize_drag = None);
                            return;
                        }
                        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
                        let width = file_register_width(
                            canvas_width,
                            Some(initial + (f32::from(event.position.x) - origin) / scale),
                        );
                        history.update(cx, |history, cx| {
                            history.file_width = Some(width);
                            cx.notify();
                        });
                        cx.stop_propagation();
                    },
                ))
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        if let Some(history) = &this.history {
                            history.update(cx, |history, _| history.file_resize_drag = None);
                        }
                    }),
                )
                .on_mouse_up_out(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        if let Some(history) = &this.history {
                            history.update(cx, |history, _| history.file_resize_drag = None);
                        }
                    }),
                )
                .when(narrow, |layout| layout.flex_col())
                .child(files)
                .when(!narrow, |layout| layout.child(divider))
                .child(diff),
        );
        body.into_any_element()
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::*;
    #[test]
    fn compact_body_avoids_subject_duplication_and_preserves_literal_body() {
        assert_eq!(commit_body("Only a subject", "Only a subject\n"), "");
        assert_eq!(
            commit_body(
                "A multiline subject",
                "A multiline\nsubject\n\nBody <literal>\nSecond paragraph\n"
            ),
            "Body <literal>\nSecond paragraph"
        );
        assert_eq!(
            commit_body("Unusual subject", "Different raw message\n"),
            "Different raw message"
        );
    }
    #[test]
    fn binary_rename_keeps_both_status_and_paths() {
        assert_eq!(
            file_identity(&Status::Binary, "new.png", Some("old.png")),
            ("Renamed · Binary", "old.png → new.png".into())
        );
        assert_eq!(
            file_identity(&Status::Binary, "image.png", None),
            ("Binary", "image.png".into())
        );
    }
}

#[cfg(test)]
mod typography_tests {
    use super::{calendar_date, readable_date};

    #[test]
    fn human_date_preserves_recorded_zone_and_exact_seconds() {
        assert_eq!(calendar_date("2026-10-26T08:40:12+05:30"), "26 Oct 2026");
        assert_eq!(calendar_date("unavailable"), "unavailable");
        assert_eq!(
            readable_date("2026-10-26T08:40:12+05:30", true),
            "26 Oct 2026 · 08:40 (UTC+05:30)"
        );
        assert_eq!(
            readable_date("2026-10-26T08:40:12+05:30", false),
            "26 Oct 2026 · 08:40:12 (UTC+05:30)"
        );
        assert_eq!(
            readable_date("2026-01-02T03:04:05Z", false),
            "2 Jan 2026 · 03:04:05 (UTC)"
        );
        assert_eq!(readable_date("unavailable", true), "unavailable");
        assert_eq!(
            readable_date("2026-13-26T08:40:12Z", true),
            "2026-13-26T08:40:12Z"
        );
    }
}

#[cfg(test)]
mod virtual_commit_tests {
    use super::*;
    use gpui::{Entity, Render, Window};

    struct Fixture {
        sidebar: Entity<Sidebar>,
        _observer: gpui::Subscription,
    }
    impl Render for Fixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .on_key_down(crate::views::shell::traverse_unhandled_tab)
                .child(
                    self.sidebar
                        .update(cx, |sidebar, cx| sidebar.render_history(window, cx)),
                )
        }
    }
    #[gpui::test]
    fn offscreen_commit_tab_and_enter_preserve_logical_destinations(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(crate::settings::SettingsState::new(
                grove_core::storage::Store::default(),
            ));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (fixture, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            let sidebar = cx.new(|cx| {
                let mut sidebar = Sidebar::new(runtime, window, cx);
                sidebar.history = Some(cx.new(|cx| BranchHistoryState::new("/tmp".into(), cx)));
                sidebar
            });
            let observer = cx.observe(&sidebar, |_, _, cx| cx.notify());
            Fixture {
                sidebar,
                _observer: observer,
            }
        });
        cx.run_until_parked();
        let sidebar = fixture.read_with(cx, |fixture, _| fixture.sidebar.clone());
        let last = format!("{:040x}", 199);
        let history = sidebar.read_with(cx, |sidebar, _| sidebar.history.as_ref().unwrap().clone());
        cx.update(|_, cx| {
            history.update(cx, |history, cx| {
                history.loading = false;
                history.error = None;
                history.snapshot = Some(std::rc::Rc::new(grove_core::history::BranchHistory {
                    head: HeadIdentity::Detached { oid: last.clone() },
                    has_more: false,
                    commits: (0..200)
                        .map(|index| Commit {
                            id: format!("{index:040x}"),
                            parents: vec![],
                            author_name: "Author".into(),
                            author_email: "author@example.com".into(),
                            timestamp: 0,
                            author_date: "2026-11-17T00:00:00Z".into(),
                            committer_name: "Author".into(),
                            committer_email: "author@example.com".into(),
                            committer_timestamp: 0,
                            committer_date: "2026-11-17T00:00:00Z".into(),
                            refs: vec![],
                            subject: format!("Commit {index}"),
                            message: format!("Commit {index}"),
                            lane: 0,
                            color_lane: 0,
                            edges: vec![],
                        })
                        .collect(),
                }));
                cx.notify();
            });
        });
        MATERIALIZED_COMMITS.with(|rows| rows.borrow_mut().clear());
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear());
        }
        assert!(
            !MATERIALIZED_COMMITS.with(|rows| rows.borrow().contains(&199)),
            "cold offscreen rows do not build graphs and subjects"
        );
        let focuses = COMMIT_FOCUS_TARGETS.with(|targets| targets.borrow().clone());
        assert_eq!(focuses.len(), 200);
        cx.update(|window, cx| focuses[198].focus(window, cx));
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear());
        }
        cx.simulate_keystrokes("tab");
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear());
        }
        assert!(
            cx.update(|window, _| focuses[199].is_focused(window)),
            "Tab reaches the next original commit destination"
        );
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(
            history.read_with(cx, |history, _| history.selected.clone()),
            Some(last)
        );
        cx.update(|_, cx| {
            history.update(cx, |history, cx| {
                history.expanded = true;
                history.diff_mode = crate::entities::branch_history::HistoryDiffMode::Unified;
                history.detail_loading = false;
                history.detail_error = None;
                history.details = Some(std::rc::Rc::new(grove_core::history::CommitDetails {
                    files: vec![grove_core::diff::FileChange {
                        path: "wide.rs".into(),
                        status: Status::Modified,
                        added: 200,
                        removed: 0,
                    }],
                    first_parent: None,
                    rename_sources: std::collections::HashMap::new(),
                }));
                history.selected_file = Some("wide.rs".into());
                history.patch_loading = false;
                history.patch_error = None;
                history.install_patch(Patch::Text {
                    hunks: vec![grove_core::diff::Hunk {
                        old_start: 0,
                        new_start: 1,
                        header: "Wide offscreen source".into(),
                        lines: (0..200)
                            .map(|index| grove_core::diff::Line {
                                kind: LineKind::Add,
                                text: if index == 199 {
                                    "W".repeat(2000)
                                } else {
                                    "short".into()
                                },
                                old_no: None,
                                new_no: Some(index + 1),
                            })
                            .collect(),
                    }],
                    no_newline_at_eof: false,
                });
                cx.notify();
            });
        });
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear());
        }
        let viewport = cx.debug_bounds("history-patch").unwrap();
        let row_before = cx.debug_bounds("history-unified-row").unwrap();
        assert!(
            row_before.size.width > viewport.size.width * 2.,
            "offscreen longest text contributes to horizontal extent"
        );
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: viewport.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(-300.), gpui::px(0.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear());
        }
        assert!(
            cx.debug_bounds("history-unified-row").unwrap().left() < row_before.left(),
            "the full unified text remains horizontally reachable"
        );

        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(
                    Action::HistoryDiffMode(
                        crate::entities::branch_history::HistoryDiffMode::Split,
                    ),
                    window,
                    cx,
                );
            });
        });
        cx.simulate_resize(gpui::size(gpui::px(360.), gpui::px(680.)));
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear());
        }
        let files = cx.debug_bounds("history-files").unwrap();
        let diff = cx.debug_bounds("history-diff-area").unwrap();
        assert!(
            files.bottom() <= diff.top(),
            "narrow files sit above the diff"
        );
        assert!(cx.debug_bounds("history-file-divider").is_none());
        let title = cx.debug_bounds("history-detail-subject").unwrap();
        let top = cx.debug_bounds("history-detail-title-row").unwrap();
        assert!(top.bottom() <= title.top());
        assert!(title.right() <= files.right());
        assert!(cx.debug_bounds("history-split-row").unwrap().size.width > gpui::px(300.));
        let pan = cx.debug_bounds("history-split-scroll").unwrap();
        cx.simulate_mouse_down(
            gpui::point(pan.left() + gpui::px(10.), pan.center().y),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_keystrokes("right");
        assert!(history.read_with(cx, |history, _| history.pan_x.get() > 0.));
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(Action::HistoryGraph, window, cx)
            });
        });
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear());
        }
        assert!(
            cx.update(|window, _| focuses[199].is_focused(window)),
            "return from details restores the originating commit's focus"
        );
    }
}
