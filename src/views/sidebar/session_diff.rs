//! Native diff surface opened from a session's Git counts.

use super::{Action, Sidebar, ROW_H};
use crate::{
    entities::diff_viewer::DiffViewerState,
    theme as c,
    views::{motion, rpx, tokens::*},
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

fn row_text(row: &UnifiedRenderRow) -> String {
    match row {
        UnifiedRenderRow::HunkHeader(header) => format!("@@ {header}"),
        UnifiedRenderRow::Line { line, .. } => format!(
            "{:>4} {:>4} {} {}",
            line.old_no.map_or(String::new(), |n| n.to_string()),
            line.new_no.map_or(String::new(), |n| n.to_string()),
            match line.kind {
                LineKind::Add => '+',
                LineKind::Del => '-',
                LineKind::Context => ' ',
            },
            line.text,
        ),
    }
}

fn shaped_width(
    rows: &[UnifiedRenderRow],
    cache: &std::cell::RefCell<Option<(gpui::Pixels, gpui::Pixels)>>,
    window: &gpui::Window,
) -> gpui::Pixels {
    let font_size = rpx(TEXT_CODE_SMALL).to_pixels(window.rem_size());
    if let Some((_, width)) = cache.borrow().filter(|(size, _)| *size == font_size) {
        return width;
    }
    let width = rows
        .iter()
        .map(|row| {
            let text = row_text(row);
            let mut run = window.text_style().to_run(text.len());
            run.font.family = crate::fonts::UI_FAMILY.into();
            window
                .text_system()
                .shape_line(text.into(), font_size, &[run], None)
                .width()
        })
        .fold(gpui::px(0.), std::cmp::Ord::max)
        + rpx(SPACE_LG * 2.).to_pixels(window.rem_size());
    *cache.borrow_mut() = Some((font_size, width));
    width
}

fn diff_row(index: usize, row: &UnifiedRenderRow, width: gpui::Pixels) -> gpui::Div {
    let (color, bg) = match row {
        UnifiedRenderRow::HunkHeader(_) => (c::BLUE(), c::SURFACE_RAISED()),
        UnifiedRenderRow::Line { line, .. } => match line.kind {
            LineKind::Add => (c::GREEN(), c::DIFF_ADD_BG()),
            LineKind::Del => (c::RED(), c::DIFF_DEL_BG()),
            LineKind::Context => (c::FG_DIM(), c::BG()),
        },
    };
    div()
        .debug_selector(move || format!("session-diff-line-{index}"))
        .h(rpx(DIFF_BODY_LINE_H))
        .line_height(rpx(DIFF_BODY_LINE_H))
        .min_w(width)
        .whitespace_nowrap()
        .px(rpx(SPACE_LG))
        .bg(bg)
        .text_color(color)
        .child(row_text(row))
}

impl Sidebar {
    pub(super) fn render_diff(&self, window: &gpui::Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(viewer) = &self.diff_viewer else {
            return div().into_any_element();
        };
        if window.is_window_active() && !viewer.read(cx).loading {
            viewer.update(cx, DiffViewerState::maybe_refresh_live);
        }
        let (
            wt_path,
            branch,
            group,
            changed_files,
            selected_path,
            loading,
            patch,
            rows,
            width_cache,
        ) = {
            let viewer = viewer.read(cx);
            (
                viewer.wt_path.clone(),
                viewer.branch.clone(),
                viewer.group.map(|g| match &viewer.subcategory {
                    Some(child) => format!("{}: {child}", g.label()),
                    None => g.label().to_owned(),
                }),
                viewer.files.clone(),
                viewer.selected_path.clone(),
                viewer.loading,
                viewer.selected_patch().cloned(),
                viewer.selected_unified().cloned(),
                viewer
                    .selected_path
                    .as_ref()
                    .and_then(|path| viewer.patch_cache.get(path))
                    .map(|patch| patch.width_cache.clone()),
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
            files = files.child(div().p(rpx(SPACE_2XL)).child(crate::views::status::label(
                "diff-loading-changes",
                crate::views::status::StatusKind::Working,
                "Loading changes…",
                ICON_SM,
                cx,
            )));
        } else if changed_files.is_empty() {
            files = files.child(div().p(rpx(SPACE_2XL)).child(crate::views::status::label(
                "diff-no-changes",
                crate::views::status::StatusKind::Idle,
                "No changed files",
                ICON_SM,
                cx,
            )));
        }
        for file in changed_files.iter() {
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
                .when(selected, |row| row.bg(c::BG_HL()).text_color(c::FG()))
                .child(crate::icons::icon("diff", ICON_MD, c::FG_DIM()))
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
            .font_family(crate::fonts::UI_FAMILY)
            .text_size(rpx(TEXT_CODE_SMALL));
        if let Some(path) = &selected_path {
            if !changed_files.iter().any(|file| &file.path == path) {
                body = body.child(div().p(rpx(SPACE_2XL)).child(crate::views::status::label(
                    "diff-no-longer-changed",
                    crate::views::status::StatusKind::Info,
                    "No longer changed",
                    ICON_SM,
                    cx,
                )));
            } else if let Some(patch) = &patch {
                match patch.as_ref() {
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
                        if let (Some(rows), Some(width_cache)) = (&rows, &width_cache) {
                            let rows = rows.clone();
                            let width = shaped_width(&rows, width_cache, window);
                            if window.is_a11y_active() || window.is_inspector_picking(cx) {
                                // Keep complete semantic text available to assistive readers.
                                body = body.overflow_x_scroll().children(
                                    rows.iter()
                                        .enumerate()
                                        .map(|(index, row)| diff_row(index, row, width)),
                                );
                            } else {
                                let scroll = viewer.read(cx).body_scroll.clone();
                                body = body.overflow_hidden().child(
                                    gpui::uniform_list(
                                        "diff-visible-lines",
                                        rows.len(),
                                        cx.processor(
                                            move |_, range: std::ops::Range<usize>, _, _| {
                                                range
                                                    .map(|index| {
                                                        diff_row(index, &rows[index], width)
                                                    })
                                                    .collect::<Vec<_>>()
                                            },
                                        ),
                                    )
                                    .h_full()
                                    .with_horizontal_sizing_behavior(
                                        gpui::ListHorizontalSizingBehavior::Unconstrained,
                                    )
                                    .track_scroll(&scroll),
                                );
                            }
                        }
                    }
                }
            } else {
                body = body.child(div().p(rpx(SPACE_2XL)).child(crate::views::status::label(
                    "diff-loading-patch",
                    crate::views::status::StatusKind::Working,
                    "Loading patch…",
                    ICON_SM,
                    cx,
                )));
            }
        } else if !loading && !changed_files.is_empty() {
            body = body.child(div().p(rpx(SPACE_2XL)).child("Select a changed file"));
        }

        motion::slow(
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
                    crate::views::components::content_header()
                        .p(rpx(SPACE_LG))
                        .border_b_1()
                        .border_color(c::BORDER())
                        .child(
                            crate::views::components::back_action(
                                "diff-close",
                                "Back to checkout",
                                window,
                                cx,
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| this.act(Action::CloseDiff, window, cx),
                            )),
                        )
                        .child(crate::views::components::content_header_title(format!(
                            "{} · {}",
                            group.as_deref().unwrap_or("Changes"),
                            branch.as_deref().unwrap_or(&wt_path)
                        )))
                        .child(
                            crate::views::components::transcript_action(
                                "diff-refresh",
                                "Refresh",
                                false,
                                window,
                                cx,
                            )
                            .icon(gpui_component::Icon::default().path("icons/refresh.svg"))
                            .on_click(cx.listener(
                                |this, _, window, cx| this.act(Action::RefreshDiff, window, cx),
                            )),
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
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
            format!("session-diff-enter-{wt_path}"),
            cx,
        )
    }
}

#[cfg(test)]
mod performance_tests {
    use super::*;
    use gpui::{Entity, Render, Window};
    use grove_core::diff::{Hunk, Line};

    struct DiffScene {
        sidebar: Entity<Sidebar>,
        viewport: Option<gpui::Size<gpui::Pixels>>,
    }
    impl Render for DiffScene {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .size_full()
                .when_some(self.viewport, |view, size| {
                    view.w(size.width).h(size.height)
                })
                .child(
                    self.sidebar
                        .update(cx, |sidebar, cx| sidebar.render_diff(window, cx)),
                )
        }
    }
    fn patch(count: u32) -> Patch {
        Patch::Text {
            hunks: vec![Hunk {
                old_start: 1,
                new_start: 1,
                header: "Large isolated diff".into(),
                lines: (1..=count)
                    .map(|line| Line {
                        kind: LineKind::Add,
                        old_no: None,
                        new_no: Some(line),
                        text: if line == 1 {
                            "i".repeat(2_000)
                        } else if line == count {
                            "W".repeat(1_500)
                        } else {
                            format!("let row_{line} = {line};")
                        },
                    })
                    .collect(),
            }],
            no_newline_at_eof: false,
        }
    }
    fn scene(window: &mut Window, cx: &mut Context<DiffScene>) -> DiffScene {
        let runtime = cx.new(crate::runtime::Runtime::new);
        let viewer = cx.new(|_| DiffViewerState::fixture(patch(20_000)));
        let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
        sidebar.update(cx, |sidebar, _| sidebar.diff_viewer = Some(viewer));
        DiffScene {
            sidebar,
            viewport: None,
        }
    }
    fn init(cx: &mut gpui::App) {
        gpui_component::init(cx);
        cx.set_global(crate::settings::SettingsState::new(
            grove_core::storage::Store::default(),
        ));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
        cx.set_reduce_motion(true);
        crate::theme::sync_component_theme(cx);
    }
    #[gpui::test]
    fn large_diff_mounts_visible_rows_and_reaches_the_final_line(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (scene, cx) = cx.add_window_view(scene);
        cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(700.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("session-diff-line-1").is_some());
        assert!(cx.debug_bounds("session-diff-line-1000").is_none());
        let viewer = scene.read_with(cx, |scene, cx| {
            scene.sidebar.read(cx).diff_viewer.clone().unwrap()
        });
        cx.update(|window, cx| {
            viewer.read(cx).body_scroll.scroll_to_bottom();
            window.refresh();
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            viewer.read_with(cx, |viewer, _| viewer
                .body_scroll
                .0
                .borrow()
                .base_handle
                .max_offset()
                .x
                > gpui::px(1_000.)),
            "offscreen long rows must contribute horizontal scroll extent"
        );
        assert!(cx.debug_bounds("session-diff-line-20000").is_some());
        assert!(cx.debug_bounds("session-diff-line-1").is_none());
        assert_eq!(
            viewer.read_with(cx, |viewer, _| viewer.selected_unified().unwrap().len()),
            20_001
        );
    }

    #[test]
    #[ignore = "manual isolated Linux native diff capture"]
    #[cfg(target_os = "linux")]
    fn large_diff_linux_capture() {
        assert!(std::env::var_os("GROVE_CONFIG_DIR").is_some());
        let output = std::env::var("GROVE_DIFF_QA_OUTPUT").expect("task-owned output directory");
        gpui_platform::application()
            .with_assets(crate::assets::Assets)
            .run(move |cx| {
                init(cx);
                crate::fonts::register(cx).unwrap();
                let mut retained_scene = None;
                let window = cx
                    .open_window(
                        gpui::WindowOptions {
                            kind: gpui::WindowKind::Normal,
                            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                                gpui::point(gpui::px(30.), gpui::px(30.)),
                                gpui::size(gpui::px(1000.), gpui::px(700.)),
                            ))),
                            ..Default::default()
                        },
                        |window, cx| {
                            window.set_window_title("Grove issue 44 diff QA");
                            let scene = cx.new(|cx| scene(window, cx));
                            retained_scene = Some(scene.clone());
                            cx.new(|cx| gpui_component::Root::new(scene, window, cx))
                        },
                    )
                    .unwrap();
                let scene = retained_scene.unwrap();
                cx.activate(true);
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(800))
                        .await;
                    fs_err::create_dir_all(&output).unwrap();
                    for (name, width, height, rem, theme) in [
                        (
                            "large-diff-dark",
                            1000.,
                            700.,
                            16.,
                            crate::theme::DEFAULT_DARK_THEME,
                        ),
                        (
                            "large-diff-light-narrow",
                            700.,
                            500.,
                            20.,
                            crate::theme::DEFAULT_LIGHT_THEME,
                        ),
                    ] {
                        cx.update_window(window.into(), |_, window, cx| {
                            assert!(grove_core::theme::set_by_name(theme));
                            crate::theme::sync_component_theme(cx);
                            scene.update(cx, |scene, cx| {
                                scene.viewport =
                                    Some(gpui::size(gpui::px(width), gpui::px(height)));
                                cx.notify();
                            });
                            window.set_rem_size(gpui::px(rem));
                            window.refresh();
                        })
                        .unwrap();
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(250))
                            .await;
                        cx.update_window(window.into(), |_, _, cx| {
                            let sidebar = &scene.read(cx).sidebar;
                            let viewer = sidebar.read(cx).diff_viewer.as_ref().unwrap().read(cx);
                            assert_eq!(
                                viewer.files.len(),
                                1,
                                "capture must retain the large patch"
                            );
                            assert_eq!(viewer.selected_unified().unwrap().len(), 20_001);
                        })
                        .unwrap();
                        let image = std::path::Path::new(&output).join(format!("{name}.png"));
                        assert!(std::process::Command::new("import")
                            .args(["-window", "Grove issue 44 diff QA"])
                            .arg(&image)
                            .status()
                            .unwrap()
                            .success());
                        let pixels = image::open(&image).unwrap().to_rgb8();
                        assert!(
                            pixels.pixels().any(|pixel| pixel.0 != [0, 0, 0]),
                            "native capture must contain painted content"
                        );
                        eprintln!(
                            "Native diff capture {name}: {}x{}, rem={rem}",
                            pixels.width(),
                            pixels.height()
                        );
                    }
                    cx.update_window(window.into(), |_, window, _| window.remove_window())
                        .unwrap();
                    cx.update(|cx| cx.quit());
                })
                .detach();
            });
    }
}
