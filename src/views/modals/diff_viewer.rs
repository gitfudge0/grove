//! The diff viewer's body: a viewport-filling surface (not a `modal_panel`,
//! per the design decision documented on [`diff_surface`]) that lists a
//! worktree's changed files and renders the selected file's diff — unified or
//! side-by-side — with sticky per-file context, hunk headers, and
//! line-number gutters.
//!
//! Built from [`crate::views::components`]'s shared shapes
//! (`modal_header_slotted`, `click_row_on`, `divider_h`, `divider_v`, `mono`,
//! `ui`, `seg_group`/`seg_button_content`) plus a local `diff_surface` for the panel
//! chrome itself — no other forked shape, per CLAUDE.md.
//!
//! # Virtualization: both bodies are `uniform_list`s over uniform-height rows
//!
//! **Both the unified and the split body are `uniform_list`s**, each
//! building only its visible range, at ZED_REV `1a246ef`.
//!
//! This matters because `gpui::list` (`crates/gpui/src/elements/list.rs`)
//! lays out every *visible* item with `AvailableSpace::MinContent` for
//! height (`list.rs:1040-1045`), which forces a full intrinsic-size Taffy
//! resolution of that item's entire subtree — and it does this
//! unconditionally, every frame, for every visible item, not just newly
//! scrolled-in ones (`list.rs:1058-1065`; the height cache it keeps only
//! spares off-screen overdraw items, and any width change invalidates every
//! cached size at once, `list.rs:1531-1545`). `uniform_list` avoids all of
//! that: it measures exactly one representative item
//! (`uniform_list.rs:658-680`) and lays out every visible item with *both*
//! axes `AvailableSpace::Definite` (`uniform_list.rs:504-508`) — a cheap,
//! fully-constrained layout with no intrinsic-size walk. That gap is why
//! unified mode (already a `uniform_list`) scrolls smoothly on a large file
//! while split mode, previously a `gpui::list` of two-column blocks (each
//! block's rows nested inside two `overflow_x_scroll` flex columns,
//! multiplying the subtree `gpui::list` had to walk), did not.
//!
//! [`crate::views::sidebar`]'s own doc rejects `uniform_list` for the
//! project tree, but for a different reason that does not transfer here:
//! tree rows are not uniform height (a directory row's disclosure state
//! changes its height), and `uniform_list` lays every item out at one
//! measured item's height. Diff rows *are* uniform height — every row here
//! is [`DIFF_BODY_LINE_H`] tall, line or hunk header alike — so that
//! objection does not apply.
//!
//! `uniform_list` gives **unified** mode horizontal scrolling for free via
//! [`ListHorizontalSizingBehavior::Unconstrained`], which sets
//! `overflow.x = Scroll` and takes the content width from the one measured
//! item (`uniform_list.rs:354-364`, `:636-650`). Unified still works exactly
//! that way.
//!
//! # Split mode: two fixed halves, each panning inside itself
//!
//! Split mode does **not** use `Unconstrained`. Its body is a `uniform_list`
//! of paired rows, one item per
//! [`grove_core::render_rows::SplitRenderRow`], rendered by [`split_row`] as
//! [left half | `divider_v()` | right half] — but the geometry is a fixed
//! 50/50 split, not content-sized columns:
//!
//! - Each half is a **fixed pixel width** ([`split_half_w`]: the body's own
//!   `content_w`, less the divider's [`DIVIDER_W`] hairline, halved) applied
//!   as `.w(px(half_w)).flex_none()`. Deliberately not `flex_1`: a half is a
//!   viewport, not a flex child, and `left + divider + right` is arranged to
//!   fill the body exactly. `half_w` comes from the *same* `content_w`
//!   arithmetic that gates split availability in [`effective_mode`], never
//!   from element bounds — bounds are a frame late and would disagree with
//!   the modal's own layout.
//! - Each half is `.overflow_hidden()` and holds one `flex_none` content row
//!   at its natural text width ([`split_content_w`], floored at `half_w` so a
//!   short file still fills its half), offset by `.ml(px(-pan_x))`. **The
//!   half never moves; only its content slides inside it.** The outer
//!   two-column strip therefore has no horizontal extent of its own and can
//!   never translate — that is the whole difference from the old model, where
//!   one shared viewport slid the entire strip sideways and pushed the right
//!   column off screen.
//! - `pan_x` is a **single shared offset**
//!   ([`crate::entities::diff_viewer::DiffViewerState::pan_x`]) that both
//!   halves read, so they pan in lockstep and a line stays beside its
//!   counterpart. Its travel is
//!   [`grove_core::render_rows::split_pan_extent`] — the *wider* side governs
//!   (the narrower one would strand the wider side short of its right edge),
//!   floored at 0 — and it is clamped on every read, so it survives a file
//!   switch and collapses by itself when the new content fits.
//! - Input: an `on_scroll_wheel` on the split body folds `delta.x` (and a
//!   shift-modified `delta.y`) into `pan_x`, while unmodified `delta.y` keeps
//!   driving the list's own vertical scroll.
//!
//! There is no word wrap, and row heights stay uniform, so `uniform_list`
//! remains valid in both modes.
//!
//! Both bodies share [`crate::entities::diff_viewer::DiffViewerState::body_scroll`],
//! a single [`gpui::UniformListScrollHandle`] — there is no more separate
//! `gpui::ListState` for split, and no more per-mode branch in
//! [`crate::entities::diff_viewer::DiffViewerState::scroll_body`].

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, uniform_list, AnyElement, App, Hsla, ListHorizontalSizingBehavior, Window,
};
use grove_core::diff::{FileChange, Line, LineKind, Patch, Run, Status, TreeNode};
use grove_core::highlight::{CodeScope, Span};
use grove_core::render_rows::{SplitCell, SplitRenderRow, UnifiedRenderRow};
use grove_core::storage::DiffMode;

use crate::entities::diff_viewer::{DiffViewerState, FileListStyle};
use crate::fonts::MONO_FAMILY;
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
/// Colours per the design decision: modified=YELLOW, added=GREEN,
/// deleted=RED, untracked=BLUE, renamed=MAGENTA.
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

/// Split `path` into a dimmed directory prefix (including the trailing `/`)
/// and the file's own name, so the file list reads name-forward — the same
/// idea a breadcrumb path uses everywhere else in the app.
fn split_dir_prefix(path: &str) -> (Option<&str>, &str) {
    match path.rfind('/') {
        Some(i) => (Some(&path[..=i]), &path[i + 1..]),
        None => (None, path),
    }
}

/// The diff viewer's panel chrome: the shared
/// [`crate::views::components::panel_surface`] sized `size_full` rather than
/// pinned to a `MODAL_W_*` width by
/// [`crate::views::components::modal_panel`]. This surface is
/// viewport-relative (inset by [`DIFF_PANEL_INSET`]) the same way Onboarding
/// escapes the `MODAL_W_*` scale, rather than being a new step on it —
/// DESIGN.md §13 treats "needs a tier that doesn't exist" as a signal the
/// design is wrong, and a full-viewport diff body is exactly that signal. R9
/// (§8.5), which requires a `modal_panel` call site to name a literal
/// `MODAL_W_*` token, does not apply because there is no `modal_panel` call
/// site — the *chrome* is shared, only the sizing differs.
fn diff_surface(content: impl IntoElement) -> gpui::Div {
    panel_surface(c::PANEL_SHADOW(), content).size_full()
}

/// One [`SPACE_2XL`] step of indentation per tree depth, plus the
/// [`SPACE_3XL`]-wide disclosure slot every row (file or directory) reserves
/// so file rows line up under their siblings' chevrons rather than under
/// their text.
fn tree_indent(depth: usize) -> f32 {
    depth as f32 * SPACE_2XL
}

/// One file row, shared by flat and tree presentations — `depth` indents it
/// and `leading` is the disclosure-slot filler (`div()` in flat mode, a real
/// chevron never appears on a file row since only directories disclose).
fn file_row_content(file: &FileChange, selected: bool, depth: usize) -> gpui::Div {
    let (letter, accent) = status_glyph(&file.status);
    let (dir, name) = split_dir_prefix(&file.path);
    let mut path_line = div().flex().flex_1().overflow_hidden();
    if let Some(dir) = dir {
        path_line = path_line.child(ui(dir.to_string(), TEXT_SMALL, c::FG_DIM()));
    }
    path_line = path_line.child(ui(name.to_string(), TEXT_SMALL, c::FG()));

    div()
        .flex()
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

/// One tree-mode directory row: a disclosure chevron plus its name, indented
/// by `depth`. Clicking anywhere on the row toggles it — the same
/// click-covers-the-row idiom `rows.rs`'s `project_row`/`worktree_row` use.
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

/// One icon-only segment: [`seg_button_content`]'s shell around a single
/// glyph sized [`ICON_XS`] and tinted by [`crate::views::components::seg_text_color`]
/// (or `glyph_color`, when a segment needs a colour that rule can't express —
/// see the disabled Split segment in [`mode_seg`]) so the active/inactive
/// read matches every text `seg_button` exactly — only the child differs.
/// `tooltip_text` is mandatory: removing a segment's label removes the only
/// affordance naming what it does, so every icon-only segment owes the user
/// a tooltip in its place.
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

/// The file list's own Flat|Tree segmented control, now icon-only:
/// `rail-sessions` for flat, `rail-tree` for tree — the same two sprites the
/// rail's own mode switch already uses for "flat list" and "project tree",
/// reused here so the same glyph keeps meaning the same thing across the app.
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
    // `flex_none` so the group is only as wide as its two glyphs: it is the
    // sole child of the file list's header row, and a growable child there
    // would stretch the group's 1px `BORDER` across the whole
    // `DIFF_FILE_LIST_W` column, reading as a stray full-width rule.
    seg_group(div().flex().items_center().child(flat_btn).child(tree_btn))
        .flex_none()
        .into_any_element()
}

fn file_list(state: &DiffViewerState, dispatch: &ModalDispatch) -> impl IntoElement {
    let mut list = div()
        .id("diff-file-list")
        .flex_none()
        .w(rpx(DIFF_FILE_LIST_W))
        .h_full()
        .flex()
        .flex_col();

    // An explicit `flex()` row: the enclosing `list` is a `flex_col`, so
    // without one this row's own layout axis is ambiguous and its single
    // child gets stretched to the column's width rather than sitting at its
    // natural size on the left.
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

/// A [`CodeScope`] resolved to its theme colour — the *only* place syntax
/// colour is decided, per the brief's "never syntect's own theme" rule.
/// [`CodeScope::Plain`] gets the same [`c::FG`] a diff line rendered with no
/// highlighting at all already used, so an oversize/unhighlighted file's
/// text does not visually change.
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

/// One line's code text as a run of `mono` spans coloured per
/// [`Span::scope`] — a plain single `mono(text)` when `spans` is empty (no
/// highlighting: an unrecognised extension, or the oversize guard tripped),
/// otherwise one child per span, each holding exactly its own chars so the
/// line reconstructs exactly regardless of multibyte content.
fn code_row(text: &str, spans: &[Span]) -> gpui::Div {
    // `flex_none` and no `overflow_hidden`: the unified body is a
    // `uniform_list`, which measures one item at `AvailableSpace::MinContent`
    // to decide the horizontal scroll extent (`uniform_list.rs:658-680`). A
    // `flex_1` + `overflow_hidden` container measures as zero at MinContent
    // and would collapse the scrollable width to the viewport, so the text's
    // natural width drives the row instead. Split mode keeps its clipping at
    // the column level, where each column is its own `overflow_x_scroll`.
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

/// [`code_row`]'s split-mode counterpart: composes the intraline word-diff
/// emphasis (`runs`, background only) with syntax colour (`spans`, foreground
/// only) rather than either overriding the other — a
/// changed word inside a string literal shows the string's colour on the
/// emphasis background, not one replacing the other. Walks both sequences by
/// char offset, splitting at whichever boundary (a run edge or a span edge)
/// comes first.
fn intraline_code_row(runs: &[Run], spans: &[Span], strong_bg: Hsla) -> gpui::Div {
    // Same shape as [`code_row`], for the same MinContent-measurement
    // reason documented there.
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

/// One line-number gutter cell, right-aligned per the brief.
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

/// One unified-mode row. Fixed [`DIFF_BODY_LINE_H`] height in exactly
/// [`split_line_cell`]'s shape (`.h(...).flex_none().flex().items_center()`)
/// because `uniform_list` lays every row out at one measured item's height
/// (`uniform_list.rs:359-371`) — a row that grew taller than the measure
/// would be clipped.
///
/// `min_w_full()`, not `w_full()`: under `Unconstrained` horizontal sizing,
/// `uniform_list` hands each item `available_width = viewport + scroll_offset.x.abs()`
/// (`uniform_list.rs:498-501`) rather than the full scrollable content width
/// up front, so a plain `w_full()` row's background would stop short of a
/// wide row's actual text until scrolled all the way right. `min_w_full()`
/// still fills the viewport when the row is narrower than it (matching the
/// old look), but lets the row grow past `available_width` to its own
/// content's natural width when that's wider, so the tint always reaches the
/// row's real right edge regardless of current scroll position.
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

/// One split-mode cell: `line` is `None` for a half-empty row's inert filler
/// side (an unpaired add/delete), rendered as a plain [`DIFF_BODY_LINE_H`]-tall
/// `BG_HL()` fill with no gutter, no sign and no text, never interactive.
/// `runs`, when given, replaces the plain coloured text with
/// [`intraline_code_row`]'s per-run emphasis composed with syntax colour —
/// only ever passed for a real Del/Add pair. `spans` is always passed
/// (empty when nothing is cached yet) and drives [`code_row`] when `runs` is
/// `None`.
///
/// `min_w_full()`, not `w_full()`: [`split_half`] wraps this cell in a
/// `flex_none` content row whose width is this side's natural content width
/// (floored at the half's own width), so `min_w_full()` makes the row's tint
/// reach that full content width — the whole pannable strip, not just the
/// visible part — while still letting a line that somehow exceeds it grow.
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

/// Unwrap one baked [`SplitCell`] into [`split_line_cell`]'s arguments — a
/// missing cell is the half-empty row's inert filler side.
fn split_cell_from(cell: Option<&SplitCell>, is_old: bool) -> gpui::Div {
    match cell {
        Some(cell) => split_line_cell(Some(&cell.line), is_old, cell.runs.as_deref(), &cell.spans),
        None => split_line_cell(None, is_old, None, &[]),
    }
}

/// The advance width of a single mono sign glyph (`"+"`/`"-"`/`" "`) at
/// [`TEXT_SMALL`] — the three are equal-width in a true monospace font, so
/// one measurement covers all three signs [`split_line_cell`] can paint.
fn sign_glyph_w(window: &Window) -> f32 {
    text_px(
        window,
        "+",
        MONO_FAMILY,
        TEXT_SMALL,
        gpui::FontWeight::default(),
    )
}

/// [`divider_v`]'s hairline width in device pixels. A hairline is `px(1.0)` at
/// every zoom and is not on any scale (DESIGN.md §13 rule 1), so this is not a
/// missing token — it is the same literal `divider_v` itself paints, named here
/// because [`split_half_w`] has to subtract it before halving.
const DIVIDER_W: f32 = 1.0;

/// One split half's fixed pixel width: the body's available width less the
/// divider hairline, halved, so `left + divider + right` fills the body
/// exactly. `content_w_device` is [`render`]'s own `content_w` converted to
/// device pixels — the same value [`effective_mode`] gates split availability
/// on, deliberately *not* an element's measured bounds, which lag a frame and
/// can disagree with this arithmetic.
fn split_half_w(content_w_device: f32) -> f32 {
    ((content_w_device - DIVIDER_W) / 2.0).max(0.0)
}

/// One split side's *natural content* width: the gutter, the sign box, and
/// `widest_text`'s real painted width — the same three children
/// [`split_line_cell`] actually lays out. This is no longer a column size (a
/// half's width is [`split_half_w`]'s, fixed); it is how wide that half's
/// inner content row is, and hence — via
/// [`grove_core::render_rows::split_pan_extent`] — how far it can pan.
/// `widest_text` is [`grove_core::render_rows::widest_split_side_text`]'s
/// result for this side, baked once per patch load; only the pixel
/// measurement happens here, because that needs a live `Window`.
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

/// One half of a split row: a fixed-`half_w`, clipped viewport holding `cell`
/// in a `flex_none` content row of its side's natural width (floored at
/// `half_w`, so a short file still fills its half rather than leaving a gap),
/// shifted left by the shared `pan_x`. The viewport itself is `flex_none` at a
/// fixed width and never translates — panning is entirely this inner
/// negative-margin offset, which is what keeps the outer strip, and so the
/// divider's x, absolutely still (see this module's doc).
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

/// One split-mode row: a hunk header spans the full row exactly like
/// unified's, or a [`SplitRenderRow::Lines`] pair renders as
/// [left half | `divider_v()` | right half], each half built by
/// [`split_half`] at the *same* fixed `half_w` — a 50/50 split, never
/// `flex_1` and never content-sized, so the divider lands at the same x on
/// every row regardless of that row's own content. Spans and intraline runs
/// are read straight off each [`SplitCell`] — baked once per patch load by
/// `grove_core::render_rows`, never derived here.
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
        // `min_w_full()`, not `w_full()`, for the same reason as
        // `diff_line_row` — this row is also a `uniform_list` item under
        // `Unconstrained` sizing when a hunk header happens to be the
        // widest row (`widest_unified_can_be_the_hunk_header`, tested in
        // `grove_core::render_rows`).
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

/// The selected file's sticky sub-header: its path plus that file's own
/// `+N -M`, shown above the scrolling hunk body so it stays put while the
/// body scrolls underneath it.
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

/// The selected file's diff body, in `mode`. Shared plumbing (selection,
/// loading, "no longer changed", Binary/TooLarge stubs) is mode-independent;
/// only the hunk rows differ.
///
/// `half_w` is [`split_half_w`]'s fixed half width (device px) and `dv` the
/// entity the split body's wheel handler pans; unified mode uses neither.
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
    // A file that stopped being changed still has a selection and a cached
    // patch, but the current file list no longer names it — the brief's
    // "No longer changed" placeholder, shown in place rather than snapping
    // selection elsewhere.
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
                    // The rows were baked once when the patch loaded — this
                    // is a pure cache lookup. `unified_rows()`/`paired_rows()`
                    // are empty exactly when `Patch::Text`'s `hunks` is (each
                    // hunk always pushes a header row), so testing the cached
                    // vector is the same condition as before.
                    let Some((rows, widest)) = state
                        .selected_unified()
                        .filter(|(rows, _)| !rows.is_empty())
                    else {
                        return stub("No longer changed");
                    };
                    // `uniform_list`'s render closure is `'static` and cannot
                    // borrow `state`, so it moves an `Rc` clone of the cached
                    // rows — O(1), never a `Vec` clone. This is the reason
                    // the cache stores rows behind an `Rc` at all.
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
                    // The same scroll position the split body tracks:
                    // Enter-then-arrows scrolls whichever body is showing.
                    // Horizontal scrolling comes from `Unconstrained`
                    // (`uniform_list.rs:636-650`), NOT a wrapping
                    // `overflow_x_scroll` — two nested horizontal scroll
                    // regions would fight each other.
                    .flex_1()
                    .min_h(px(0.0))
                    .track_scroll(&state.body_scroll)
                    .with_width_from_item(Some(widest))
                    .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                    .into_any_element()
                }
                DiffMode::Split => {
                    // Derived once at load, same as Unified above — the old
                    // code derived the split rows twice per frame here.
                    let rows = state.selected_split();
                    let Some(rows) = rows.filter(|rows| !rows.is_empty()) else {
                        return stub("No longer changed");
                    };
                    let Some((old_widest_text, new_widest_text)) = state.selected_split_col_texts()
                    else {
                        return stub("No longer changed");
                    };
                    // Both columns' widths, and the sign glyph's, are
                    // measured for real off the current `Window` — cheap
                    // (a couple of cached `layout_line` lookups, see
                    // `text_px`'s own doc), not an O(rows) rescan, since
                    // "widest" was already reduced to one string per side at
                    // load time.
                    let sign_w = sign_glyph_w(window);
                    let left_content_w = split_content_w(window, old_widest_text, sign_w);
                    let right_content_w = split_content_w(window, new_widest_text, sign_w);
                    // Clamped on read, never on write: a pan carried over
                    // from another file collapses to whatever this layout can
                    // actually show.
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
                    // Neither `Unconstrained` nor `with_width_from_item`: the
                    // split body has NO horizontal extent of its own — the
                    // halves are fixed and pan internally, so the outer strip
                    // must never translate.
                    //
                    // But default (`FitList`) sizing leaves
                    // `overflow.x = Visible`, and `div.rs:3097-3112` then
                    // re-routes a horizontal-only wheel delta onto the
                    // *vertical* axis (`delta_y = delta.x` when `overflow.y`
                    // is Scroll, `delta.y` is zero and `overflow.x` is not
                    // Scroll) — a sideways trackpad swipe would scroll the
                    // diff up and down instead of panning. Setting
                    // `overflow.x = Scroll` purely absorbs `delta.x`: under
                    // `FitList` the content width is the padded viewport
                    // width (`uniform_list.rs:355-361`), so the extent is
                    // zero and nothing actually moves. It has to go through
                    // `Styled::style()` because `overflow_x_scroll()` lives on
                    // `StatefulInteractiveElement`, which `UniformList` does
                    // not implement.
                    let mut list = list;
                    list.style().overflow.x = Some(gpui::Overflow::Scroll);

                    // The pan input surface. `delta.y` is left alone so the
                    // list's own vertical scroll keeps working; only `delta.x`
                    // (trackpad swipe, and the OS's own shift+wheel
                    // translation on macOS) and an explicitly shift-modified
                    // `delta.y` (platforms that do not translate it) pan.
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
                            // gpui's scroll offset grows negative as content
                            // moves left; `pan_x` grows positive for the same
                            // motion, hence the sign flip.
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

/// The header's Unified|Split segmented control (C), now icon-only:
/// `unified` (the single-pane sprite) and `split` (the two-pane sprite) —
/// see [`icons::icon`]'s `"unified"`/`"split"` cases, deliberately built on
/// the same outline so the pair reads as one set. `split_enabled` is (D)'s
/// narrow-window fallback: when `false` the Split segment renders in the
/// disabled tier — a dropped handler plus `FG_MUTE` glyph — with a tooltip
/// explaining why, rather than a segment that silently does nothing.
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
        // The shared segment shell with a dropped handler (§9.1.1's disabled
        // tier: no handler, `FG_MUTE` glyph, and never a greyed-out fill) —
        // routed through `icon_seg`'s `glyph_color` override since this
        // segment's glyph is not `seg_text_color`'s. This keeps its own
        // narrow-window tooltip rather than the plain "Split" one —
        // preserved exactly, unlike the enabled case's affordance tooltip.
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

/// (D) The narrow-window fallback, pure: no gpui, no rendering. Split is
/// available only when `content_w >= DIFF_SPLIT_MIN_W`; when unavailable the
/// effective mode is `Unified` regardless of `stored`, and the Split segment
/// is disabled. `stored` itself is never written here — widening the window
/// brings Split back automatically because the fallback lives only in this
/// function's return value.
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
    let content_w = win_w - DIFF_PANEL_INSET * 2.0 - DIFF_FILE_LIST_W;
    let (mode, split_enabled) = effective_mode(content_w, state.mode);
    // Device pixels, from the same `content_w` the fallback above gates on —
    // never from element bounds (see [`split_half_w`]).
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
        .child(file_list(state, dispatch))
        .child(divider_v())
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
        // The fallback lives only in the return value: `stored` is passed
        // through unchanged, so a wider `content_w` alone flips the result
        // back to `Split` without anything having written to settings.
        let stored = DiffMode::Split;
        let (narrow_mode, _) = effective_mode(DIFF_SPLIT_MIN_W - 1.0, stored);
        let (wide_mode, wide_enabled) = effective_mode(DIFF_SPLIT_MIN_W + 100.0, stored);
        assert_eq!(narrow_mode, DiffMode::Unified);
        assert_eq!(wide_mode, DiffMode::Split);
        assert!(wide_enabled);
    }
}
