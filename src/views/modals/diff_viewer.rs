//! The diff viewer's body: a viewport-filling surface listing a worktree's
//! changed files and rendering the selected file's diff, unified or split.
//!
//! Both bodies are `uniform_list`s, not `gpui::list`: `gpui::list` walks full
//! intrinsic-size layout for every visible item every frame, while
//! `uniform_list` measures one item and lays out at a fixed size (see
//! `list.rs:1040-1065` vs `uniform_list.rs:504-680`).
//!
//! Split mode uses two fixed 50/50-pixel-width halves that each pan
//! internally via a shared `pan_x` offset, rather than one scrollable strip —
//! so the divider's x never moves regardless of scroll position.

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, uniform_list, AnyElement, App, CursorStyle, Hsla,
    ListHorizontalSizingBehavior, MouseButton, Window,
};
use grove_core::diff::{FileChange, Line, LineKind, Patch, Run, Status, TreeNode};
use grove_core::highlight::{CodeScope, Span};
use grove_core::render_rows::{SplitCell, SplitRenderRow, UnifiedRenderRow};
use grove_core::storage::DiffMode;

use crate::entities::diff_viewer::{DiffViewerState, FileListStyle};
use crate::fonts::{MONO_FAMILY, UI_FAMILY};
use crate::icons;
use crate::theme as c;
use crate::views::components::{
    click_row_on, divider_h, divider_v, hint_tooltip, modal_header_slotted, mono, panel_surface,
    seg_button_content, seg_group, seg_text_color, ui, OnToggle, RowDensity, SegSide,
};
use crate::views::rpx;
use crate::views::tokens::*;
use crate::views::workspace::{text_px, token_px};

use super::{Modal, ModalClick, ModalDispatch, ModalLayer};

/// The status letter shown ahead of a file-list row's path, and its accent.
fn status_glyph(status: &Status) -> (&'static str, Hsla) {
    match status {
        Status::Added => ("A", c::GREEN()),
        Status::Modified => ("M", c::YELLOW()),
        Status::Deleted => ("D", c::RED()),
        Status::Renamed { .. } => ("R", c::MAGENTA()),
        Status::Untracked => ("U", c::BLUE()),
        Status::Binary => ("B", c::FG_MUTE()),
    }
}

/// Splits `path` into a dimmed directory prefix (with trailing `/`) and the file's own name.
fn split_dir_prefix(path: &str) -> (Option<&str>, &str) {
    match path.rfind('/') {
        Some(i) => (Some(&path[..=i]), &path[i + 1..]),
        None => (None, path),
    }
}

/// Full-viewport panel chrome, sized `size_full` rather than a `MODAL_W_*` tier — deliberately off the `modal_panel` scale, per DESIGN.md §13.
fn diff_surface(content: impl IntoElement) -> gpui::Div {
    panel_surface(c::PANEL_SHADOW(), content).size_full()
}

/// One [`SPACE_2XL`] step of indentation per tree depth.
fn tree_indent(depth: usize) -> f32 {
    depth as f32 * SPACE_2XL
}

/// One file row, shared by flat and tree presentations.
fn file_row_content(file: &FileChange, selected: bool, depth: usize) -> gpui::Div {
    let (letter, accent) = status_glyph(&file.status);
    let (dir, name) = split_dir_prefix(&file.path);
    let mut path_line = div().flex().flex_1().overflow_hidden();
    if let Some(dir) = dir {
        path_line = path_line.child(ui(dir.to_string(), TEXT_SMALL, c::FG_DIM()));
    }
    path_line = path_line.child(ui(name.to_string(), TEXT_SMALL, c::FG()));

    // `w_full`: this div carries the selection tint/ring and must reach both edges, not just the text width.
    div()
        .flex()
        .w_full()
        .items_center()
        .gap(rpx(SPACE_SM))
        .pl(rpx(SPACE_2XL + tree_indent(depth)))
        .pr(rpx(SPACE_2XL))
        .py(rpx(SPACE_SM))
        .when(selected, |d| {
            d.bg(c::SEL_TINT_SOFT())
                .border_1()
                .border_color(c::SEL_RING())
        })
        .child(mono(letter, TEXT_SMALL, accent))
        .child(path_line)
        .child(mono(format!("+{}", file.added), TEXT_MICRO, c::GREEN()))
        .child(mono(format!("-{}", file.removed), TEXT_MICRO, c::RED()))
}

fn file_row(
    file: &FileChange,
    selected: bool,
    depth: usize,
    dispatch: &ModalDispatch,
) -> AnyElement {
    let path = file.path.clone();
    let dispatch = std::rc::Rc::clone(dispatch);
    click_row_on(
        gpui::SharedString::from(format!("diff-file-{path}")),
        false,
        RowDensity::Card,
        move |window, cx| {
            dispatch(
                ModalClick::SelectDiffFile { path: path.clone() },
                window,
                cx,
            );
        },
        file_row_content(file, selected, depth),
    )
    .into_any_element()
}

/// One tree-mode directory row: a disclosure chevron plus its name, indented by `depth`.
fn dir_row(
    path: &str,
    name: &str,
    depth: usize,
    expanded: bool,
    dispatch: &ModalDispatch,
) -> AnyElement {
    let twist = if expanded { "chev-down" } else { "chev-right" };
    let content = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_SM))
        .pl(rpx(SPACE_2XL + tree_indent(depth)))
        .pr(rpx(SPACE_2XL))
        .py(rpx(SPACE_SM))
        .child(
            div()
                .flex_none()
                .w(rpx(SPACE_3XL))
                .flex()
                .items_center()
                .justify_center()
                .child(icons::icon(twist, ICON_XS, c::FG_MUTE())),
        )
        .child(ui(name.to_string(), TEXT_SMALL, c::FG_DIM()));

    let dispatch = std::rc::Rc::clone(dispatch);
    let path = path.to_string();
    click_row_on(
        gpui::SharedString::from(format!("diff-dir-{path}")),
        false,
        RowDensity::Card,
        move |window, cx| {
            dispatch(
                ModalClick::ToggleDiffTreeDir { path: path.clone() },
                window,
                cx,
            );
        },
        content,
    )
    .into_any_element()
}

/// [`file_row_content`]'s natural width in device pixels, measured rather than guessed since paths vary widely in length.
fn file_row_w(file: &FileChange, depth: usize, window: &Window) -> f32 {
    let (letter, _) = status_glyph(&file.status);
    let (dir, name) = split_dir_prefix(&file.path);
    let weight = gpui::FontWeight::default();

    let mut w = token_px(SPACE_2XL + tree_indent(depth), window)
        + token_px(SPACE_2XL, window)
        + text_px(window, letter, MONO_FAMILY, TEXT_SMALL, weight)
        + text_px(
            window,
            &format!("+{}", file.added),
            MONO_FAMILY,
            TEXT_MICRO,
            weight,
        )
        + text_px(
            window,
            &format!("-{}", file.removed),
            MONO_FAMILY,
            TEXT_MICRO,
            weight,
        )
        + token_px(SPACE_SM, window) * 3.0;
    if let Some(dir) = dir {
        w += text_px(window, dir, UI_FAMILY, TEXT_SMALL, weight);
    }
    w += text_px(window, name, UI_FAMILY, TEXT_SMALL, weight);
    w
}

/// [`dir_row`]'s natural width in device pixels.
fn dir_row_w(name: &str, depth: usize, window: &Window) -> f32 {
    token_px(SPACE_2XL + tree_indent(depth), window)
        + token_px(SPACE_2XL, window)
        + token_px(SPACE_3XL, window)
        + token_px(SPACE_SM, window)
        + text_px(
            window,
            name,
            UI_FAMILY,
            TEXT_SMALL,
            gpui::FontWeight::default(),
        )
}

/// `.max`/`.min`, not [`f32::clamp`]: an inverted range degrades to the ceiling instead of panicking.
pub(crate) fn clamp_file_list_w(measured_natural_logical: f32, win_w_logical: f32) -> f32 {
    let ceiling = win_w_logical * DIFF_FILE_LIST_MAX_FRAC;
    measured_natural_logical.max(DIFF_FILE_LIST_W).min(ceiling)
}

/// The diff viewer's file-list column width; all width-consuming call sites share this one function.
#[must_use]
pub(crate) fn file_list_w(
    state: &DiffViewerState,
    window: &Window,
    override_w: Option<f32>,
) -> f32 {
    let zoom = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
    let win_w_logical = f32::from(window.viewport_size().width) / zoom;

    if let Some(w) = override_w {
        return clamp_file_list_w(w, win_w_logical);
    }

    if state.files.is_empty() {
        return DIFF_FILE_LIST_W;
    }

    let measured_device = match state.list_style {
        FileListStyle::Flat => state
            .files
            .iter()
            .map(|file| file_row_w(file, 0, window))
            .fold(0.0_f32, f32::max),
        FileListStyle::Tree => {
            grove_core::diff::flatten_file_tree(&state.files, &state.tree_expanded)
                .into_iter()
                .map(|node| match node {
                    TreeNode::Dir { name, depth, .. } => dir_row_w(&name, depth, window),
                    TreeNode::File { file, depth } => file_row_w(&file, depth, window),
                })
                .fold(0.0_f32, f32::max)
        }
    };

    clamp_file_list_w(measured_device / zoom, win_w_logical)
}

/// One icon-only segment. `tooltip_text` is mandatory since the icon alone doesn't name what the segment does.
fn icon_seg(
    id: &'static str,
    icon_name: &'static str,
    tooltip_text: &'static str,
    active: bool,
    side: SegSide,
    glyph_color: Option<Hsla>,
    on_click: Option<OnToggle>,
) -> AnyElement {
    let color = glyph_color.unwrap_or_else(|| seg_text_color(active, false));
    div()
        .id(id)
        .tooltip(hint_tooltip(tooltip_text))
        .child(seg_button_content(
            format!("{id}-inner"),
            div()
                .h(rpx(CONTROL_H))
                .px(rpx(SPACE_2XL))
                .flex()
                .items_center()
                .justify_center()
                .child(icons::icon(icon_name, ICON_XS, color)),
            active,
            side,
            false,
            on_click,
        ))
        .into_any_element()
}

/// The file list's Flat|Tree segmented control, reusing the rail's `rail-sessions`/`rail-tree` glyphs.
fn list_style_seg(style: FileListStyle, dispatch: &ModalDispatch) -> AnyElement {
    let flat_active = style == FileListStyle::Flat;
    let tree_active = style == FileListStyle::Tree;
    let flat_btn = icon_seg(
        "diff-list-flat",
        "rail-sessions",
        "Flat list",
        flat_active,
        SegSide::Left,
        None,
        (!flat_active).then(|| -> OnToggle {
            let dispatch = std::rc::Rc::clone(dispatch);
            Box::new(move |window, cx| {
                dispatch(ModalClick::ToggleDiffListStyle, window, cx);
            })
        }),
    );
    let tree_btn = icon_seg(
        "diff-list-tree",
        "rail-tree",
        "File tree",
        tree_active,
        SegSide::Right,
        None,
        (!tree_active).then(|| -> OnToggle {
            let dispatch = std::rc::Rc::clone(dispatch);
            Box::new(move |window, cx| {
                dispatch(ModalClick::ToggleDiffListStyle, window, cx);
            })
        }),
    );
    // `flex_none`: a growable child here would stretch the group's border across the whole column.
    seg_group(div().flex().items_center().child(flat_btn).child(tree_btn))
        .flex_none()
        .into_any_element()
}

fn file_list(
    state: &DiffViewerState,
    dispatch: &ModalDispatch,
    window: &Window,
    override_w: Option<f32>,
) -> impl IntoElement {
    let mut list = div()
        .id("diff-file-list")
        .flex_none()
        .w(rpx(file_list_w(state, window, override_w)))
        .h_full()
        .flex()
        .flex_col();

    // Explicit `flex()`: without it the child stretches to the column's width instead of sitting at its natural size.
    list = list.child(
        div()
            .flex_none()
            .flex()
            .items_center()
            .px(rpx(SPACE_2XL))
            .py(rpx(SPACE_SM))
            .child(list_style_seg(state.list_style, dispatch)),
    );
    list = list.child(divider_h());

    let mut body = div()
        .id("diff-file-list-body")
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .flex()
        .flex_col();

    if state.loading && state.files.is_empty() {
        body = body.child(
            div()
                .p(rpx(SPACE_2XL))
                .child(ui("Loading…", TEXT_SMALL, c::FG_MUTE())),
        );
        return list.child(body);
    }
    if state.files.is_empty() {
        body = body.child(div().p(rpx(SPACE_2XL)).child(ui(
            "No changes",
            TEXT_SMALL,
            c::FG_MUTE(),
        )));
        return list.child(body);
    }

    match state.list_style {
        FileListStyle::Flat => {
            for file in &state.files {
                let selected = state.selected_path.as_deref() == Some(file.path.as_str());
                body = body.child(file_row(file, selected, 0, dispatch));
            }
        }
        FileListStyle::Tree => {
            for node in grove_core::diff::flatten_file_tree(&state.files, &state.tree_expanded) {
                body = body.child(match node {
                    TreeNode::Dir {
                        path,
                        name,
                        depth,
                        expanded,
                    } => dir_row(&path, &name, depth, expanded, dispatch),
                    TreeNode::File { file, depth } => {
                        let selected = state.selected_path.as_deref() == Some(file.path.as_str());
                        file_row(&file, selected, depth, dispatch)
                    }
                });
            }
        }
    }

    list.child(body)
}

/// The drag handle between the file list and the diff body.
fn file_list_divider(dispatch: &ModalDispatch) -> AnyElement {
    let dispatch = std::rc::Rc::clone(dispatch);
    div()
        .id("diff-file-list-divider")
        .w(rpx(DIVIDER_DRAG_HIT_W))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor(CursorStyle::ResizeLeftRight)
        .child(divider_v())
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            dispatch(ModalClick::DiffFileListDividerPress, window, cx);
        })
        .into_any_element()
}

/// A [`CodeScope`] resolved to its theme colour — the only place syntax colour is decided.
fn scope_color(scope: CodeScope) -> Hsla {
    match scope {
        CodeScope::Keyword => c::CODE_KEYWORD(),
        CodeScope::StringLit => c::CODE_STRING(),
        CodeScope::Number => c::CODE_NUMBER(),
        CodeScope::Comment => c::CODE_COMMENT(),
        CodeScope::Type => c::CODE_TYPE(),
        CodeScope::Func => c::CODE_FUNC(),
        CodeScope::Punct => c::CODE_PUNCT(),
        CodeScope::Plain => c::FG(),
    }
}

/// One line's code text as a run of `mono` spans coloured per [`Span::scope`].
fn code_row(text: &str, spans: &[Span]) -> gpui::Div {
    // `flex_none`, no `overflow_hidden`: keeps natural width for `uniform_list`'s MinContent measurement.
    let row = div().flex_none().flex().items_center();
    if spans.is_empty() {
        return row.child(mono(text.to_string(), TEXT_SMALL, c::FG()));
    }
    let chars: Vec<char> = text.chars().collect();
    let mut row = row;
    for s in spans {
        let end = (s.start + s.len).min(chars.len());
        if s.start >= end {
            continue;
        }
        let chunk: String = chars[s.start..end].iter().collect();
        row = row.child(mono(chunk, TEXT_SMALL, scope_color(s.scope)));
    }
    row
}

/// [`code_row`]'s split-mode counterpart: composes word-diff emphasis with syntax colour instead of one overriding the other.
fn intraline_code_row(runs: &[Run], spans: &[Span], strong_bg: Hsla) -> gpui::Div {
    let mut row = div().flex_none().flex().items_center();
    let mut pos = 0usize;
    for run in runs {
        let run_chars: Vec<char> = run.text.chars().collect();
        let run_end = pos + run_chars.len();
        let mut cursor = pos;
        while cursor < run_end {
            let scope = spans
                .iter()
                .find(|s| cursor >= s.start && cursor < s.start + s.len)
                .map_or(CodeScope::Plain, |s| s.scope);
            let boundary = spans
                .iter()
                .filter(|s| s.start > cursor && s.start < run_end)
                .map(|s| s.start)
                .min()
                .unwrap_or(run_end);
            let span_end = spans
                .iter()
                .find(|s| cursor >= s.start && cursor < s.start + s.len)
                .map_or(boundary, |s| (s.start + s.len).min(boundary).min(run_end));
            let chunk: String = run_chars[cursor - pos..span_end - pos].iter().collect();
            let mut el = mono(chunk, TEXT_SMALL, scope_color(scope));
            if run.changed {
                el = el.bg(strong_bg);
            }
            row = row.child(el);
            cursor = span_end;
        }
        pos = run_end;
    }
    row
}

/// One line-number gutter cell, right-aligned.
fn gutter_cell(no: Option<u32>) -> gpui::Div {
    div()
        .flex_none()
        .w(rpx(DIFF_GUTTER_W))
        .flex()
        .justify_end()
        .child(mono(
            no.map(|n| n.to_string()).unwrap_or_default(),
            TEXT_MICRO,
            c::FG_MUTE(),
        ))
}

/// `min_w_full()`, not `w_full()`: lets the tint grow past the viewport to the row's real width.
fn diff_line_row(line: &Line, spans: &[Span]) -> gpui::Div {
    let (bg, sign) = match line.kind {
        LineKind::Add => (Some(c::DIFF_ADD_BG()), "+"),
        LineKind::Del => (Some(c::DIFF_DEL_BG()), "-"),
        LineKind::Context => (None, " "),
    };
    let mut r = div()
        .min_w_full()
        .h(rpx(DIFF_BODY_LINE_H))
        .flex_none()
        .flex()
        .items_center()
        .child(gutter_cell(line.old_no))
        .child(gutter_cell(line.new_no))
        .child(
            div()
                .flex_none()
                .px(rpx(SPACE_SM))
                .child(mono(sign, TEXT_SMALL, c::FG_MUTE())),
        )
        .child(code_row(&line.text, spans));
    if let Some(bg) = bg {
        r = r.bg(bg);
    }
    r
}

/// `min_w_full()`, not `w_full()`: the tint must reach the full pannable content width.
fn split_line_cell(
    line: Option<&Line>,
    is_old: bool,
    runs: Option<&[Run]>,
    spans: &[Span],
) -> gpui::Div {
    let Some(line) = line else {
        return div()
            .min_w_full()
            .h(rpx(DIFF_BODY_LINE_H))
            .flex_none()
            .bg(c::BG_HL());
    };
    let no = if is_old { line.old_no } else { line.new_no };
    let (base_bg, sign) = match line.kind {
        LineKind::Add => (Some(c::DIFF_ADD_BG()), "+"),
        LineKind::Del => (Some(c::DIFF_DEL_BG()), "-"),
        LineKind::Context => (None, " "),
    };
    let mut row = div()
        .min_w_full()
        .h(rpx(DIFF_BODY_LINE_H))
        .flex_none()
        .flex()
        .items_center()
        .child(gutter_cell(no))
        .child(
            div()
                .flex_none()
                .px(rpx(SPACE_SM))
                .child(mono(sign, TEXT_SMALL, c::FG_MUTE())),
        );
    let code: AnyElement = match runs {
        Some(runs) => {
            let strong = if is_old {
                c::DIFF_DEL_BG_STRONG()
            } else {
                c::DIFF_ADD_BG_STRONG()
            };
            intraline_code_row(runs, spans, strong).into_any_element()
        }
        None => code_row(&line.text, spans).into_any_element(),
    };
    row = row.child(code);
    if let Some(bg) = base_bg {
        row = row.bg(bg);
    }
    row
}

/// Unwraps one baked [`SplitCell`] into [`split_line_cell`]'s arguments.
fn split_cell_from(cell: Option<&SplitCell>, is_old: bool) -> gpui::Div {
    match cell {
        Some(cell) => split_line_cell(Some(&cell.line), is_old, cell.runs.as_deref(), &cell.spans),
        None => split_line_cell(None, is_old, None, &[]),
    }
}

/// The advance width of a mono sign glyph at [`TEXT_SMALL`]; `+`/`-`/` ` are equal-width in a true monospace font, so one measurement covers all three.
fn sign_glyph_w(window: &Window) -> f32 {
    text_px(
        window,
        "+",
        MONO_FAMILY,
        TEXT_SMALL,
        gpui::FontWeight::default(),
    )
}

/// [`divider_v`]'s hairline width in device pixels; a hairline is `px(1.0)` at every zoom and is not on any token scale.
const DIVIDER_W: f32 = 1.0;

/// Uses `content_w_device`, not measured element bounds — those lag a frame behind [`effective_mode`]'s gate.
fn split_half_w(content_w_device: f32) -> f32 {
    ((content_w_device - DIVIDER_W) / 2.0).max(0.0)
}

/// One split side's natural content width — not the (fixed) column size, but how far this half's inner content row, and hence its pan range, extends.
fn split_content_w(window: &Window, widest_text: &str, sign_w: f32) -> f32 {
    token_px(DIFF_GUTTER_W, window)
        + token_px(SPACE_SM, window) * 2.0
        + sign_w
        + text_px(
            window,
            widest_text,
            MONO_FAMILY,
            TEXT_SMALL,
            gpui::FontWeight::default(),
        )
}

/// The viewport never translates; panning is entirely the inner content row's negative-margin offset.
fn split_half(cell: gpui::Div, half_w: f32, content_w: f32, pan_x: f32) -> gpui::Div {
    div()
        .flex_none()
        .w(px(half_w))
        .h_full()
        .overflow_hidden()
        .child(
            div()
                .flex_none()
                .w(px(content_w.max(half_w)))
                .ml(px(-pan_x))
                .child(cell),
        )
}

/// A fixed `half_w` for both halves keeps the divider at the same x on every row.
fn split_row(
    row: &SplitRenderRow,
    half_w: f32,
    left_content_w: f32,
    right_content_w: f32,
    pan_x: f32,
) -> gpui::Div {
    match row {
        SplitRenderRow::HunkHeader(h) => hunk_header_row(h),
        SplitRenderRow::Lines { old, new } => div()
            .min_w_full()
            .h(rpx(DIFF_BODY_LINE_H))
            .flex_none()
            .flex()
            .items_center()
            .child(split_half(
                split_cell_from(old.as_ref(), true),
                half_w,
                left_content_w,
                pan_x,
            ))
            .child(divider_v())
            .child(split_half(
                split_cell_from(new.as_ref(), false),
                half_w,
                right_content_w,
                pan_x,
            )),
    }
}

fn hunk_header_row(header: &str) -> gpui::Div {
    div()
        // `min_w_full()`, not `w_full()`: same reason as `diff_line_row` — a hunk header can be the widest row.
        .min_w_full()
        .bg(c::BG_HL())
        .px(rpx(SPACE_2XL))
        .h(rpx(DIFF_BODY_LINE_H))
        .flex_none()
        .flex()
        .items_center()
        .child(mono(format!("@@ {header}"), TEXT_MICRO, c::FG_MUTE()))
}

fn stub(text: impl Into<gpui::SharedString>) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(ui(text, TEXT_SMALL, c::FG_MUTE()))
        .into_any_element()
}

/// The selected file's sticky sub-header, shown above the scrolling hunk body.
fn file_sub_header(state: &DiffViewerState) -> Option<gpui::Div> {
    let path = state.selected_path.as_deref()?;
    let file = state.files.iter().find(|f| f.path == path)?;
    Some(
        div()
            .flex_none()
            .w_full()
            .flex()
            .items_center()
            .gap(rpx(SPACE_LG))
            .px(rpx(SPACE_2XL))
            .py(rpx(SPACE_SM))
            .bg(c::BG_HL())
            .child(mono(file.path.clone(), TEXT_SMALL, c::FG()))
            .child(mono(format!("+{}", file.added), TEXT_MICRO, c::GREEN()))
            .child(mono(format!("-{}", file.removed), TEXT_MICRO, c::RED())),
    )
}

/// The selected file's diff body, in `mode`; `half_w` and `dv` are used only by split mode's pan handler.
fn diff_body(
    state: &DiffViewerState,
    mode: DiffMode,
    window: &Window,
    half_w: f32,
    dv: &gpui::Entity<DiffViewerState>,
) -> AnyElement {
    let Some(path) = state.selected_path.as_deref() else {
        return stub("No changes");
    };
    let Some(patch) = state.selected_patch() else {
        return stub("Loading…");
    };
    // A file with a cached patch may no longer be in the current file list.
    let still_changed = state.files.iter().any(|f| f.path == path);
    if !still_changed {
        return stub("No longer changed");
    }
    match patch {
        Patch::Binary => stub("Binary file changed"),
        Patch::TooLarge { added, removed } => {
            stub(format!("File too large to display (+{added} -{removed})"))
        }
        Patch::Text { .. } => {
            let hunks: AnyElement = match mode {
                DiffMode::Unified => {
                    // Pure cache lookup: rows were baked once when the patch loaded.
                    let Some((rows, widest)) = state
                        .selected_unified()
                        .filter(|(rows, _)| !rows.is_empty())
                    else {
                        return stub("No longer changed");
                    };
                    // `'static` render closure moves an `Rc` clone (O(1)), never a `Vec` clone.
                    let rows = std::rc::Rc::clone(rows);
                    let row_count = rows.len();
                    uniform_list(
                        "diff-unified-body",
                        row_count,
                        move |range, _window, _cx| {
                            range
                                .filter_map(|ix| rows.get(ix))
                                .map(|row| match row {
                                    UnifiedRenderRow::HunkHeader(h) => hunk_header_row(h),
                                    UnifiedRenderRow::Line { line, spans } => {
                                        diff_line_row(line, spans)
                                    }
                                })
                                .collect::<Vec<_>>()
                        },
                    )
                    // Horizontal scroll comes from `Unconstrained`, not a wrapping `overflow_x_scroll` — two nested scroll regions would fight.
                    .flex_1()
                    .min_h(px(0.0))
                    .track_scroll(&state.body_scroll)
                    .with_width_from_item(Some(widest))
                    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                    .into_any_element()
                }
                DiffMode::Split => {
                    let rows = state.selected_split();
                    let Some(rows) = rows.filter(|rows| !rows.is_empty()) else {
                        return stub("No longer changed");
                    };
                    let Some((old_widest_text, new_widest_text)) = state.selected_split_col_texts()
                    else {
                        return stub("No longer changed");
                    };
                    let sign_w = sign_glyph_w(window);
                    let left_content_w = split_content_w(window, old_widest_text, sign_w);
                    let right_content_w = split_content_w(window, new_widest_text, sign_w);
                    // Clamped on read, never on write: a pan carried over from another file collapses to what this layout can show.
                    let pan_x = state.split_pan(left_content_w, right_content_w, half_w);

                    let rows = Rc::clone(rows);
                    let row_count = rows.len();
                    let list =
                        uniform_list("diff-split-body", row_count, move |range, _window, _cx| {
                            range
                                .filter_map(|ix| rows.get(ix))
                                .map(|row| {
                                    split_row(row, half_w, left_content_w, right_content_w, pan_x)
                                })
                                .collect::<Vec<_>>()
                        })
                        .flex_1()
                        .min_h(px(0.0))
                        .track_scroll(&state.body_scroll);
                    // `overflow.x = Scroll` absorbs `delta.x` (div.rs:3097-3112) without moving anything.
                    let mut list = list;
                    list.style().overflow.x = Some(gpui::Overflow::Scroll);

                    // `delta.y` is left alone for vertical scroll; only `delta.x` and an explicit shift-modified `delta.y` pan.
                    let dv = dv.clone();
                    let line_h = token_px(DIFF_BODY_LINE_H, window);
                    div()
                        .flex_1()
                        .min_h(px(0.0))
                        .flex()
                        .flex_col()
                        .on_scroll_wheel(move |ev, _window, cx| {
                            let d = ev.delta.pixel_delta(px(line_h));
                            let mut dx = f32::from(d.x);
                            if ev.modifiers.shift && dx == 0.0 {
                                dx = f32::from(d.y);
                            }
                            if dx == 0.0 {
                                return;
                            }
                            // Sign flip: gpui's scroll offset grows negative leftward, `pan_x` grows positive for the same motion.
                            dv.update(cx, |state, cx| {
                                state.pan_split(-dx, left_content_w, right_content_w, half_w, cx);
                            });
                        })
                        .child(list)
                        .into_any_element()
                }
            };
            let mut col = div().flex().flex_col().size_full();
            if let Some(header) = file_sub_header(state) {
                col = col.child(header).child(divider_h());
            }
            col.child(hunks).into_any_element()
        }
    }
}

/// The header's Unified|Split segmented control; `split_enabled` disables Split on a narrow window.
fn mode_seg(mode: DiffMode, split_enabled: bool, dispatch: &ModalDispatch) -> AnyElement {
    let unified_active = mode == DiffMode::Unified;
    let split_active = mode == DiffMode::Split;

    let unified_btn = icon_seg(
        "diff-mode-unified",
        "unified",
        "Unified",
        unified_active,
        SegSide::Left,
        None,
        (!unified_active).then(|| -> OnToggle {
            let dispatch = std::rc::Rc::clone(dispatch);
            Box::new(move |window, cx| {
                dispatch(ModalClick::SetDiffMode(DiffMode::Unified), window, cx);
            })
        }),
    );

    let split_seg = if split_enabled {
        icon_seg(
            "diff-mode-split",
            "split",
            "Split",
            split_active,
            SegSide::Right,
            None,
            (!split_active).then(|| -> OnToggle {
                let dispatch = std::rc::Rc::clone(dispatch);
                Box::new(move |window, cx| {
                    dispatch(ModalClick::SetDiffMode(DiffMode::Split), window, cx);
                })
            }),
        )
    } else {
        // Disabled tier per §9.1.1: no handler, `FG_MUTE` glyph, never a greyed-out fill.
        icon_seg(
            "diff-mode-split-disabled",
            "split",
            "Window too narrow for side-by-side",
            false,
            SegSide::Right,
            Some(c::FG_MUTE()),
            None,
        )
    };

    seg_group(
        div()
            .flex()
            .items_center()
            .child(unified_btn)
            .child(split_seg),
    )
    .into_any_element()
}

/// `stored` is never written here — widening the window brings Split back automatically.
pub fn effective_mode(content_w: f32, stored: DiffMode) -> (DiffMode, bool) {
    let split_available = content_w >= DIFF_SPLIT_MIN_W;
    let mode = if split_available {
        stored
    } else {
        DiffMode::Unified
    };
    (mode, split_available)
}

pub fn render(
    layer: &ModalLayer,
    dispatch: &ModalDispatch,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let Some(Modal::DiffViewer { wt_path }) = layer.slot().get() else {
        return div().into_any_element();
    };
    let Some(dv) = layer.diff_viewer.as_ref() else {
        return div().into_any_element();
    };
    let state = dv.read(cx);

    let title = state
        .branch
        .clone()
        .map_or_else(|| format!("Diff — {wt_path}"), |b| format!("Diff — {b}"));

    let zoom = cx.global::<crate::zoom::ZoomState>().zoom;
    let win_w = f32::from(window.viewport_size().width) / zoom;
    let override_w = layer.file_list_w_override;
    let content_w = win_w - DIFF_PANEL_INSET * 2.0 - file_list_w(state, window, override_w);
    let (mode, split_enabled) = effective_mode(content_w, state.mode);
    // Device pixels from `content_w`, never from element bounds (see [`split_half_w`]).
    let half_w = split_half_w(content_w * zoom);

    let header = modal_header_slotted(
        Some("diff-viewer-close"),
        title,
        c::FG(),
        Some(mode_seg(mode, split_enabled, dispatch)),
        None,
        Some(dispatch),
    );

    let body = div()
        .flex()
        .flex_1()
        .min_h(px(0.0))
        .child(file_list(state, dispatch, window, override_w))
        .child(file_list_divider(dispatch))
        .child(
            div()
                .flex_1()
                .h_full()
                .overflow_hidden()
                .child(diff_body(state, mode, window, half_w, dv)),
        );

    div()
        .size_full()
        .p(rpx(DIFF_PANEL_INSET))
        .child(diff_surface(
            div()
                .flex()
                .flex_col()
                .size_full()
                .child(header)
                .child(divider_h())
                .child(body),
        ))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_below_the_floor_clamps_up_to_it() {
        assert_eq!(clamp_file_list_w(100.0, 2000.0), DIFF_FILE_LIST_W);
    }

    #[test]
    fn measured_above_the_ceiling_clamps_down_to_it() {
        let win_w = 1000.0;
        assert_eq!(
            clamp_file_list_w(600.0, win_w),
            win_w * DIFF_FILE_LIST_MAX_FRAC
        );
    }

    #[test]
    fn measured_between_the_floor_and_ceiling_passes_through() {
        let win_w = 1000.0;
        let measured = 300.0;
        assert!(measured > DIFF_FILE_LIST_W && measured < win_w * DIFF_FILE_LIST_MAX_FRAC);
        assert_eq!(clamp_file_list_w(measured, win_w), measured);
    }

    #[test]
    fn the_ceiling_is_forty_percent_of_the_window_width() {
        let win_w = 1234.5;
        assert_eq!(
            clamp_file_list_w(f32::MAX, win_w),
            win_w * DIFF_FILE_LIST_MAX_FRAC
        );
        assert_eq!(win_w * DIFF_FILE_LIST_MAX_FRAC, win_w * 0.4);
    }

    // Exercises the divider-drag override path directly, without a real `Window`.
    #[test]
    fn a_too_small_override_clamps_up_to_the_floor() {
        assert_eq!(clamp_file_list_w(10.0, 2000.0), DIFF_FILE_LIST_W);
    }

    #[test]
    fn a_too_large_override_clamps_down_to_the_ceiling() {
        let win_w = 1000.0;
        assert_eq!(
            clamp_file_list_w(f32::MAX, win_w),
            win_w * DIFF_FILE_LIST_MAX_FRAC
        );
    }

    // Tests `on_diff_divider_press`'s double-click condition against synthetic durations, avoiding a real `Instant::now()` pair.
    #[test]
    fn two_presses_inside_the_double_click_window_register_as_one() {
        let gap = std::time::Duration::from_millis(200);
        assert!(gap < crate::views::components::DOUBLE_CLICK);
    }

    #[test]
    fn two_presses_further_apart_than_the_window_do_not_register() {
        let gap = std::time::Duration::from_millis(400);
        assert!(gap >= crate::views::components::DOUBLE_CLICK);
    }

    // The `>=` comparison `on_root_mouse_up`/`on_diff_divider_mouse_up` gate a persist on.
    #[test]
    fn a_delta_just_under_the_epsilon_is_not_a_real_drag() {
        let delta: f32 = crate::views::components::DRAG_EPSILON - 0.1;
        assert!(delta.abs() < crate::views::components::DRAG_EPSILON);
    }

    #[test]
    fn a_delta_at_or_over_the_epsilon_is_a_real_drag() {
        let delta: f32 = crate::views::components::DRAG_EPSILON;
        assert!(delta.abs() >= crate::views::components::DRAG_EPSILON);
    }

    #[test]
    fn split_available_only_at_or_above_the_min_width() {
        let cases = [
            (
                DIFF_SPLIT_MIN_W - 1.0,
                DiffMode::Unified,
                DiffMode::Unified,
                false,
            ),
            (
                DIFF_SPLIT_MIN_W - 1.0,
                DiffMode::Split,
                DiffMode::Unified,
                false,
            ),
            (DIFF_SPLIT_MIN_W, DiffMode::Unified, DiffMode::Unified, true),
            (DIFF_SPLIT_MIN_W, DiffMode::Split, DiffMode::Split, true),
            (
                DIFF_SPLIT_MIN_W + 1.0,
                DiffMode::Unified,
                DiffMode::Unified,
                true,
            ),
            (
                DIFF_SPLIT_MIN_W + 1.0,
                DiffMode::Split,
                DiffMode::Split,
                true,
            ),
        ];
        for (content_w, stored, want_mode, want_enabled) in cases {
            let (mode, enabled) = effective_mode(content_w, stored);
            assert_eq!(mode, want_mode, "content_w={content_w} stored={stored:?}");
            assert_eq!(
                enabled, want_enabled,
                "content_w={content_w} stored={stored:?}"
            );
        }
    }

    #[test]
    fn stored_split_while_narrow_yields_effective_unified_and_disabled_segment() {
        let (mode, enabled) = effective_mode(DIFF_SPLIT_MIN_W - 50.0, DiffMode::Split);
        assert_eq!(mode, DiffMode::Unified);
        assert!(!enabled);
    }

    #[test]
    fn widening_the_window_brings_back_a_stored_split_preference() {
        let stored = DiffMode::Split;
        let (narrow_mode, _) = effective_mode(DIFF_SPLIT_MIN_W - 1.0, stored);
        let (wide_mode, wide_enabled) = effective_mode(DIFF_SPLIT_MIN_W + 100.0, stored);
        assert_eq!(narrow_mode, DiffMode::Unified);
        assert_eq!(wide_mode, DiffMode::Split);
        assert!(wide_enabled);
    }
}
