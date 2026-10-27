//! Full-canvas, read-only commit graph. The conversation remains mounted underneath.
use super::{Action, Sidebar};
use crate::entities::branch_history::BranchHistoryState;
use crate::{
    icons::icon,
    theme as c,
    views::{rpx, tokens::*},
};
use gpui::{div, prelude::*, AnyElement, Context};
use std::time::Duration;
pub(super) const HISTORY_HOVER_INTENT: Duration = Duration::from_millis(600);
pub(super) const HISTORY_HOVER_GRACE: Duration = Duration::from_millis(260);

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
const DETAIL_HEADER_MAX_H: f32 = 320.0;
const DETAIL_FILES_W: f32 = 200.0;
const DETAIL_FILES_MAX_FRAC: f32 = 0.32;
const FILE_ROW_H: f32 = 56.0;
const DIFF_NUMBER_W: f32 = 36.0;
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
fn split_cell(line: Option<&grove_core::diff::Line>, old: bool, pan: f32, width: f32) -> gpui::Div {
    let (bg, color) = line.map_or((c::BG_RAIL(), c::FG_DIM()), |line| {
        let (_, bg, color) = line_style(line.kind);
        (bg, color)
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
    pub(super) fn render_history(
        &self,
        window: &gpui::Window,
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
                    "reset",
                    Action::RefreshHistory,
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
            for (index, commit) in snapshot.commits.iter().enumerate() {
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
                    .relative()
                    .w_full()
                    .min_w(rpx(graph_w + ROW_CONTENT_MIN_W))
                    .h(rpx(COMMIT_ROW_H))
                    .justify_start()
                    .rounded_none()
                    .gap_0()
                    .when(selected, |row| row.bg(c::BG_HL()))
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
                        f32::from(window.viewport_size().width - self.rail_bounds.get().size.width)
                            / (f32::from(window.rem_size()) / 16.0)
                            - left
                            - SPACE_2XL;
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
                    if let Some(details) = &state.details {
                        let (added, removed) =
                            details.files.iter().fold((0u64, 0u64), |(a, r), f| {
                                (a + u64::from(f.added), r + u64::from(f.removed))
                            });
                        card = card.child(
                            self.control(
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
                            .child(icon("chev-right", ICON_SM, c::FG_DIM()))
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
                            .child(div().text_color(c::RED()).child(format!("−{removed}"))),
                        );
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
                    }
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
                list = list.child(row);
            }
            if snapshot.has_more {
                list = list.child(div().p(rpx(SPACE_2XL)).child(if state.limit < MAX_HISTORY {
                    self.history_action("history-load-more", "Load older commits", "chev-down", Action::MoreHistory, cx).into_any_element()
                } else { div().text_color(c::FG_DIM()).child("Showing the newest 500 reachable commits. Parent lanes continue beyond this limit.").into_any_element() }));
            }
            // Space allows the last commit's floating preview to remain reachable inside the scroll pane.
            list = list.child(div().h(rpx(PREVIEW_BOTTOM_SPACE)));
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
                    div()
                        .flex()
                        .items_center()
                        .flex_shrink_0()
                        .gap(rpx(SPACE_LG))
                        .p(rpx(SPACE_2XL))
                        .border_b_1()
                        .border_color(c::BORDER())
                        .child(self.history_action(
                            "history-back",
                            "Back",
                            "chev-left",
                            Action::CloseHistory,
                            cx,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_size(rpx(TEXT_TITLE)).child("Commit history"))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(c::FG_DIM())
                                        .child(label),
                                ),
                        )
                        .child(self.history_action(
                            "history-refresh",
                            "Refresh",
                            "reset",
                            Action::RefreshHistory,
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
    fn history_action(
        &self,
        id: &'static str,
        label: &'static str,
        glyph: &'static str,
        action: Action,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        self.control(id, label, action, cx)
            .w_auto()
            .px(rpx(SPACE_LG))
            .text_size(rpx(TEXT_SMALL))
            .child(icon(glyph, ICON_SM, c::FG_DIM()))
            .child(label)
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
        let file_width = DETAIL_FILES_W.min(canvas_width * DETAIL_FILES_MAX_FRAC);
        let half_width = ((canvas_width - file_width) / 2.0 - DIFF_NUMBER_W).max(SPACE_2XL);
        let mut header = div()
            .id("history-detail-header")
            .debug_selector(|| "history-detail-header".into())
            .flex()
            .flex_col()
            .flex_shrink_0()
            .min_w_0()
            .max_h(rpx(DETAIL_HEADER_MAX_H))
            .overflow_y_scroll()
            .p(rpx(SPACE_2XL))
            .gap(rpx(SPACE_LG))
            .border_b_1()
            .border_color(c::BORDER())
            .child(self.history_action(
                "history-return-graph",
                "Back to history",
                "chev-left",
                Action::HistoryGraph,
                cx,
            ))
            .child(
                div()
                    .text_size(rpx(TEXT_DISPLAY))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(commit.subject.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(avatar(&commit.author_name, IDENTITY_AVATAR_SIZE))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(rpx(TEXT_BODY))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child(commit.author_name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(rpx(TEXT_MICRO))
                                    .text_color(c::FG_DIM())
                                    .child(readable_date(&commit.author_date, true)),
                            ),
                    )
                    .child(
                        self.history_action(
                            "history-copy-sha",
                            "Copy SHA",
                            "copy",
                            Action::CopyHistoryCommit,
                            cx,
                        )
                        .child(
                            div()
                                .text_color(c::FG_DIM())
                                .child(commit.id.chars().take(7).collect::<String>()),
                        ),
                    ),
            );
        let message = commit_body(&commit.subject, &commit.message);
        if !message.is_empty() {
            header = header.child(literal_message("history-detail-body", message));
        }
        if !commit.refs.is_empty() {
            header = header.child(
                div()
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .child(refs_label(commit)),
            );
        }
        header = header.child(
            self.history_action(
                "history-metadata-toggle",
                "Commit metadata",
                if state.metadata_expanded {
                    "chev-down"
                } else {
                    "chev-right"
                },
                Action::ToggleHistoryMetadata,
                cx,
            )
            .aria_expanded(state.metadata_expanded),
        );
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
            header = header.child(metadata);
        }
        let semantics = if commit.parents.is_empty() {
            "Initial commit · compared with empty tree"
        } else if commit.parents.len() > 1 {
            "Merge commit · compared with first parent"
        } else {
            "Changes compared with parent"
        };
        header = header.child(
            div()
                .text_size(rpx(TEXT_MICRO))
                .text_color(c::FG_DIM())
                .child(semantics),
        );
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
                .child(
                    div().p(rpx(SPACE_2XL)).child(
                        state
                            .detail_error
                            .clone()
                            .unwrap_or_else(|| "Loading changed files…".into()),
                    ),
                )
                .into_any_element();
        };
        if details.files.is_empty() {
            return body
                .child(
                    div()
                        .p(rpx(SPACE_2XL))
                        .child("No file changes against this parent."),
                )
                .into_any_element();
        }
        let (added, removed) = details.files.iter().fold((0u64, 0u64), |(a, r), file| {
            (a + u64::from(file.added), r + u64::from(file.removed))
        });
        let mut files = div()
            .id("history-files")
            .debug_selector(|| "history-files".into())
            .w(rpx(file_width))
            .flex_shrink_0()
            .min_h_0()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(c::BORDER())
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
                    .flex()
                    .gap(rpx(SPACE_LG))
                    .pb(rpx(SPACE_LG))
                    .text_size(rpx(TEXT_MICRO))
                    .child(div().text_color(c::GREEN()).child(format!("+{added}")))
                    .child(div().text_color(c::RED()).child(format!("−{removed}"))),
            );
        for file in &details.files {
            let (status, identity) = file_identity(
                &file.status,
                &file.path,
                details.rename_sources.get(&file.path).map(String::as_str),
            );
            let (directory, name) = file.path.rsplit_once('/').unwrap_or(("", &file.path));
            files = files.child(
                self.control(
                    format!("history-file-{}", file.path),
                    format!("{status}: {identity}. Open diff"),
                    Action::SelectHistoryFile(file.path.clone()),
                    cx,
                )
                .w_full()
                .h_auto()
                .min_h(rpx(FILE_ROW_H))
                .justify_start()
                .px(rpx(SPACE_SM))
                .py(rpx(SPACE_LG))
                .when(state.selected_file.as_ref() == Some(&file.path), |row| {
                    row.bg(c::BG_HL())
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .flex_1()
                        .gap(rpx(SPACE_SM))
                        .child(
                            div()
                                .truncate()
                                .text_size(rpx(TEXT_SMALL))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(name.to_owned()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(rpx(TEXT_MICRO))
                                .text_color(c::FG_DIM())
                                .child(format!("{status} · {directory}")),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(rpx(SPACE_LG))
                                .text_size(rpx(TEXT_MICRO))
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
                ),
            );
        }
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
                    div().flex_1().min_w_0().child(
                        crate::views::components::readonly_value("history-selected-file", identity)
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
                .flex()
                .border_1()
                .border_color(c::BORDER())
                .rounded(rpx(RADIUS_CONTROL))
                .child(
                    self.history_action(
                        "history-mode-unified",
                        "Unified",
                        "unified",
                        Action::HistoryDiffMode(HistoryDiffMode::Unified),
                        cx,
                    )
                    .aria_selected(state.diff_mode == HistoryDiffMode::Unified)
                    .when(state.diff_mode == HistoryDiffMode::Unified, |control| {
                        control.bg(c::BG_HL())
                    }),
                )
                .child(
                    self.history_action(
                        "history-mode-split",
                        "Split",
                        "split",
                        Action::HistoryDiffMode(HistoryDiffMode::Split),
                        cx,
                    )
                    .aria_selected(state.diff_mode == HistoryDiffMode::Split)
                    .when(state.diff_mode == HistoryDiffMode::Split, |control| {
                        control.bg(c::BG_HL())
                    }),
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
        let mut patch = div()
            .id("history-patch")
            .debug_selector(|| "history-patch".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .text_size(rpx(TEXT_CODE_SMALL));
        if let Some(error) = &state.patch_error {
            patch = patch.child(div().p(rpx(SPACE_2XL)).child(error.clone()));
        } else if state.patch_loading {
            patch = patch.child(div().p(rpx(SPACE_2XL)).child("Loading patch…"));
        } else if let Some(value) = &state.patch {
            match value {
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
                    if state.diff_mode == HistoryDiffMode::Split {
                        let mut widest = 0.0f32;
                        for hunk in hunks {
                            for line in &hunk.lines {
                                let mut run = window.text_style().to_run(line.text.len());
                                run.font.family = crate::fonts::UI_FAMILY.into();
                                let width = window
                                    .text_system()
                                    .shape_line(
                                        line.text.clone().into(),
                                        rpx(TEXT_CODE_SMALL).to_pixels(window.rem_size()),
                                        &[run],
                                        None,
                                    )
                                    .width();
                                widest = widest.max(f32::from(width) / scale);
                            }
                        }
                        let pan = grove_core::render_rows::clamp_split_pan(
                            state.pan_x,
                            widest,
                            widest,
                            half_width,
                        );
                        patch = patch.on_scroll_wheel(cx.listener(
                            move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                                let delta = match event.delta {
                                    gpui::ScrollDelta::Pixels(p) => f32::from(p.x) / scale,
                                    gpui::ScrollDelta::Lines(p) => p.x * DIFF_BODY_LINE_H,
                                };
                                if delta != 0.0 {
                                    if let Some(history) = &this.history {
                                        history.update(cx, |history, cx| {
                                            history.pan_x =
                                                grove_core::render_rows::clamp_split_pan(
                                                    history.pan_x - delta,
                                                    widest,
                                                    widest,
                                                    half_width,
                                                );
                                            cx.notify();
                                        });
                                    }
                                    cx.stop_propagation();
                                }
                            },
                        ));
                        for row in value.paired_rows() {
                            patch = match row {
                                grove_core::diff::PairedRow::HunkHeader(text) => {
                                    patch.child(diff_hunk(text))
                                }
                                grove_core::diff::PairedRow::Lines { old, new } => patch.child(
                                    div()
                                        .flex()
                                        .h(rpx(DIFF_BODY_LINE_H))
                                        .child(
                                            split_cell(old.as_ref(), true, pan, widest)
                                                .border_l_1()
                                                .border_color(c::alpha(c::BORDER(), 0.0)),
                                        )
                                        .child(
                                            split_cell(new.as_ref(), false, pan, widest)
                                                .border_l_1()
                                                .border_color(c::BORDER()),
                                        ),
                                ),
                            };
                        }
                    } else {
                        patch = patch.overflow_x_scroll();
                        for hunk in hunks {
                            patch = patch.child(diff_hunk(hunk.header.clone()));
                            for line in &hunk.lines {
                                let (prefix, bg, color) = line_style(line.kind);
                                patch = patch.child(
                                    div()
                                        .h(rpx(DIFF_BODY_LINE_H))
                                        .whitespace_nowrap()
                                        .px(rpx(SPACE_LG))
                                        .bg(bg)
                                        .text_color(color)
                                        .child(format!(
                                            "{:>4} {:>4} {prefix} {}",
                                            line.old_no.map_or(String::new(), |n| n.to_string()),
                                            line.new_no.map_or(String::new(), |n| n.to_string()),
                                            line.text
                                        )),
                                );
                            }
                        }
                    }
                }
            }
        } else {
            patch = patch.child(div().p(rpx(SPACE_2XL)).child("Loading selected file…"));
        }
        diff = diff.child(patch);
        body = body.child(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(files)
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
