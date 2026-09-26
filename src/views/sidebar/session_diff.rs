//! Native diff surface opened from a session's Git counts.

use super::{Action, Sidebar, ROW_H};
use crate::{
    entities::diff_viewer::DiffViewerState,
    theme as c,
    views::{rpx, tokens::*},
};
use gpui::{div, prelude::*, AnyElement, Context, MouseButton};
use grove_core::{
    diff::{LineKind, Patch, Status},
    render_rows::UnifiedRenderRow,
};

fn status_letter(status: &Status) -> &'static str {
    match status {
        Status::Added => "A",
        Status::Modified => "M",
        Status::Deleted => "D",
        Status::Renamed { .. } => "R",
        Status::Untracked => "U",
        Status::Binary => "B",
    }
}

impl Sidebar {
    pub(super) fn render_diff(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(viewer) = &self.diff_viewer else {
            return div().into_any_element();
        };
        if !viewer.read(cx).loading {
            viewer.update(cx, DiffViewerState::maybe_refresh_live);
        }
        let (wt_path, branch, changed_files, selected_path, loading, patch, rows) = {
            let viewer = viewer.read(cx);
            (
                viewer.wt_path.clone(),
                viewer.branch.clone(),
                viewer.files.clone(),
                viewer.selected_path.clone(),
                viewer.loading,
                viewer.selected_patch().cloned(),
                viewer.selected_unified().map(|(rows, _)| rows.clone()),
            )
        };
        let mut files = div()
            .id("diff-files")
            .flex()
            .flex_col()
            .min_w_0()
            .h_full()
            .overflow_y_scroll();
        if loading && changed_files.is_empty() {
            files = files.child(div().p(rpx(SPACE_2XL)).child("Loading changes…"));
        } else if changed_files.is_empty() {
            files = files.child(div().p(rpx(SPACE_2XL)).child("No changed files"));
        }
        for file in &changed_files {
            let path = file.path.clone();
            let selected = selected_path.as_deref() == Some(path.as_str());
            files = files.child(
                self.control(
                    format!("diff-file-{path}"),
                    format!("Open diff for {path}"),
                    Action::SelectDiffFile(path.clone()),
                    cx,
                )
                .w_full()
                .h_auto()
                .min_h(rpx(ROW_H))
                .px(rpx(SPACE_LG))
                .justify_start()
                .gap(rpx(SPACE_SM))
                .when(selected, |row| row.bg(c::SEL_TINT_SOFT()))
                .child(status_letter(&file.status))
                .child(div().flex_1().min_w_0().truncate().child(path))
                .child(format!("+{} -{}", file.added, file.removed)),
            );
        }

        let mut body = div()
            .id("diff-body")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .font_family(crate::fonts::MONO_FAMILY)
            .text_size(rpx(TEXT_CODE_SMALL));
        if let Some(path) = &selected_path {
            if !changed_files.iter().any(|file| &file.path == path) {
                body = body.child(div().p(rpx(SPACE_2XL)).child("No longer changed"));
            } else if let Some(patch) = &patch {
                match patch {
                    Patch::Binary => {
                        body = body.child(div().p(rpx(SPACE_2XL)).child("Binary file"));
                    }
                    Patch::TooLarge { added, removed } => {
                        body = body.child(
                            div()
                                .p(rpx(SPACE_2XL))
                                .child(format!("Diff too large to display · +{added} -{removed}")),
                        );
                    }
                    Patch::Text { .. } => {
                        if let Some(rows) = &rows {
                            for row in rows.iter() {
                                let (text, color, bg) = match row {
                                    UnifiedRenderRow::HunkHeader(header) => {
                                        (format!("@@ {header}"), c::BLUE(), c::SURFACE_RAISED())
                                    }
                                    UnifiedRenderRow::Line { line, .. } => {
                                        let (mark, color, bg) = match line.kind {
                                            LineKind::Add => ('+', c::GREEN(), c::DIFF_ADD_BG()),
                                            LineKind::Del => ('-', c::RED(), c::DIFF_DEL_BG()),
                                            LineKind::Context => (' ', c::FG_DIM(), c::BG()),
                                        };
                                        (
                                            format!(
                                                "{:>4} {:>4} {mark} {}",
                                                line.old_no
                                                    .map_or(String::new(), |n| n.to_string()),
                                                line.new_no
                                                    .map_or(String::new(), |n| n.to_string()),
                                                line.text
                                            ),
                                            color,
                                            bg,
                                        )
                                    }
                                };
                                body = body.child(
                                    div()
                                        .w_full()
                                        .px(rpx(SPACE_LG))
                                        .bg(bg)
                                        .text_color(color)
                                        .child(text),
                                );
                            }
                        }
                    }
                }
            } else {
                body = body.child(div().p(rpx(SPACE_2XL)).child("Loading patch…"));
            }
        } else if !loading && !changed_files.is_empty() {
            body = body.child(div().p(rpx(SPACE_2XL)).child("Select a changed file"));
        }

        div()
            .id("session-diff-viewer")
            .debug_selector(|| "session-diff-viewer".into())
            .role(gpui::Role::Dialog)
            .aria_label(format!("Changes in {wt_path}"))
            .track_focus(&self.diff_focus)
            .absolute()
            .inset_0()
            .bg(c::BG())
            .text_color(c::FG())
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .p(rpx(SPACE_LG))
                    .border_b_1()
                    .border_color(c::BORDER())
                    .child(div().flex_1().min_w_0().truncate().child(format!(
                        "Changes · {}",
                        branch.as_deref().unwrap_or(&wt_path)
                    )))
                    .child(
                        self.control("diff-refresh", "Refresh changes", Action::RefreshDiff, cx)
                            .child("Refresh"),
                    )
                    .child(
                        self.control("diff-close", "Close changes", Action::CloseDiff, cx)
                            .child("Close"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .w(rpx(DIFF_FILE_LIST_W))
                            .max_w(rpx(DIFF_FILE_LIST_W))
                            .h_full()
                            .child(files),
                    )
                    .child(body),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(rpx(DIFF_FILE_LIST_W))
                    .w(rpx(SPACE_XS))
                    .border_l_1()
                    .border_color(c::BORDER()),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .into_any_element()
    }
}
