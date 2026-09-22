//! Adaptive session grid. Proportions belong to the workspace, never to a PTY.
use super::{rpx, Sidebar};
use crate::{
    grid::{
        equal_weights, minimum_weight, normalize_weights, transfer_pair, GridAxis, GridBoundary,
    },
    theme as c,
};
use gpui::{div, prelude::*, AnyElement, Context, Div, FocusHandle, MouseButton, Stateful, Window};
use std::collections::HashMap;

const GRID_PADDING: f32 = 0.0;
const GRID_GAP: f32 = 0.0;
const GRID_RESIZE_HIT: f32 = 16.0;
const GRID_TILE_TARGET_W: f32 = 320.0;
const GRID_TILE_MIN_W: f32 = 200.0;
const GRID_TILE_MIN_H: f32 = 120.0;
const GRID_KEYBOARD_STEP: f32 = 0.025;

#[derive(Default)]
pub(super) struct WorkspaceGrid {
    shapes: HashMap<Vec<usize>, GridWeights>,
    current: Vec<usize>,
    focus: HashMap<(usize, Option<usize>), FocusHandle>,
}
#[derive(Clone)]
struct GridWeights {
    columns: Vec<f32>,
    rows: Vec<Vec<f32>>,
}
#[derive(Clone)]
pub(super) struct GridDrag {
    boundary: GridBoundary,
    origin: f32,
    initial: Vec<f32>,
}

fn columns(count: usize, width: f32) -> usize {
    let available = (width - GRID_PADDING * 2.0 + GRID_GAP).max(0.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let fit = (available / (GRID_TILE_TARGET_W + GRID_GAP)) as usize;
    fit.clamp(1, 3).min(count.max(1))
}
fn shape(count: usize, columns: usize) -> Vec<usize> {
    (0..columns)
        .map(|column| (column..count).step_by(columns).count())
        .collect()
}
fn constrain_weights(weights: &mut [f32], minimum: f32) {
    if weights.is_empty() {
        return;
    }
    normalize_weights(weights);
    #[allow(clippy::cast_precision_loss)]
    let floor = minimum.min(1.0 / weights.len() as f32);
    if weights.iter().all(|weight| *weight >= floor) {
        return;
    }
    let excess: f32 = weights
        .iter()
        .map(|weight| (*weight - floor).max(0.0))
        .sum();
    #[allow(clippy::cast_precision_loss)]
    let remaining = (1.0 - floor * weights.len() as f32).max(0.0);
    for weight in weights {
        *weight = floor
            + if excess > f32::EPSILON {
                (*weight - floor).max(0.0) / excess * remaining
            } else {
                0.0
            };
    }
}
impl WorkspaceGrid {
    fn prepare(&mut self, count: usize, width: f32) {
        let previous = self.weights().cloned();
        self.current = shape(count, columns(count, width));
        self.shapes.entry(self.current.clone()).or_insert_with(|| {
            let mut weights = GridWeights {
                columns: equal_weights(self.current.len()),
                rows: self.current.iter().copied().map(equal_weights).collect(),
            };
            if let Some(previous) =
                previous.filter(|previous| previous.columns.len() == self.current.len())
            {
                weights.columns = previous.columns;
                for (index, rows) in weights.rows.iter_mut().enumerate() {
                    if let Some(saved) = previous
                        .rows
                        .get(index)
                        .filter(|saved| saved.len() == rows.len())
                    {
                        *rows = saved.clone();
                    }
                }
            }
            weights
        });
    }
    fn weights(&self) -> Option<&GridWeights> {
        self.shapes.get(&self.current)
    }
    fn weights_mut(&mut self, boundary: GridBoundary) -> Option<&mut Vec<f32>> {
        let weights = self.shapes.get_mut(&self.current)?;
        match boundary.axis {
            GridAxis::Columns => Some(&mut weights.columns),
            GridAxis::Rows => weights.rows.get_mut(boundary.column?),
        }
    }
}
impl Sidebar {
    fn grid_resize(
        &mut self,
        boundary: GridBoundary,
        delta: f32,
        initial: Option<Vec<f32>>,
        cx: &mut Context<Self>,
    ) {
        if self.confirmation_open() {
            return;
        }
        let bounds = self.grid_bounds.get();
        let Some(grid) = self.grid_layouts.get_mut(&self.active_workspace) else {
            return;
        };
        let Some(weights) = grid.weights_mut(boundary) else {
            return;
        };
        let previous = weights.clone();
        if let Some(initial) = initial {
            if initial.len() != weights.len() {
                return;
            }
            *weights = initial;
        }
        let total = match boundary.axis {
            GridAxis::Columns => f32::from(bounds.size.width),
            GridAxis::Rows => f32::from(bounds.size.height),
        };
        let scale = cx.global::<crate::zoom::ZoomState>().rem_size() / crate::zoom::REM_BASE;
        let minimum = minimum_weight(
            total,
            GRID_GAP * scale,
            weights.len(),
            match boundary.axis {
                GridAxis::Columns => GRID_TILE_MIN_W,
                GridAxis::Rows => GRID_TILE_MIN_H,
            } * scale,
        );
        transfer_pair(weights, boundary.boundary, delta, minimum);
        if *weights != previous {
            cx.notify();
        }
    }
    fn grid_separator(&mut self, boundary: GridBoundary, cx: &mut Context<Self>) -> Stateful<Div> {
        let key = (boundary.boundary, boundary.column);
        let grid = self.grid_layouts.entry(self.active_workspace).or_default();
        let focus = grid
            .focus
            .entry(key)
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let vertical = boundary.axis == GridAxis::Columns;
        let label = if vertical {
            format!(
                "Resize grid columns {} and {}. Use Left and Right arrows.",
                boundary.boundary + 1,
                boundary.boundary + 2
            )
        } else {
            format!(
                "Resize grid rows {} and {} in column {}. Use Up and Down arrows.",
                boundary.boundary + 1,
                boundary.boundary + 2,
                boundary.column.unwrap_or(0) + 1
            )
        };
        div()
            .id(gpui::SharedString::from(format!(
                "grid-divider-{}-{}",
                boundary
                    .column
                    .map_or_else(|| "columns".into(), |c| format!("row-{c}")),
                boundary.boundary
            )))
            .debug_selector(move || {
                format!(
                    "grid-divider-{}-{}",
                    boundary
                        .column
                        .map_or_else(|| "columns".into(), |c| format!("row-{c}")),
                    boundary.boundary
                )
            })
            .role(gpui::Role::Splitter)
            .aria_label(label)
            .tab_index(0)
            .track_focus(&focus)
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .when(vertical, |d| {
                d.w(rpx(GRID_RESIZE_HIT))
                    .mx(rpx(-GRID_RESIZE_HIT / 2.0))
                    .h_full()
                    .cursor_col_resize()
            })
            .when(!vertical, |d| {
                d.h(rpx(GRID_RESIZE_HIT))
                    .my(rpx(-GRID_RESIZE_HIT / 2.0))
                    .w_full()
                    .cursor_row_resize()
            })
            .hover(|s| s.bg(c::BG_HOVER()))
            .focus(|s| s.bg(c::BG_HOVER()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    if this.confirmation_open() {
                        return;
                    }
                    focus.focus(window, cx);
                    let initial = this
                        .grid_layouts
                        .get_mut(&this.active_workspace)
                        .and_then(|grid| grid.weights_mut(boundary))
                        .cloned();
                    if let Some(initial) = initial {
                        this.grid_drag = Some(GridDrag {
                            boundary,
                            origin: f32::from(if vertical {
                                event.position.x
                            } else {
                                event.position.y
                            }),
                            initial,
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
                let delta = match (vertical, event.keystroke.key.as_str()) {
                    (true, "left") | (false, "up") => -GRID_KEYBOARD_STEP,
                    (true, "right") | (false, "down") => GRID_KEYBOARD_STEP,
                    _ => return,
                };
                this.grid_resize(boundary, delta, None, cx);
                cx.stop_propagation();
            }))
    }
    #[allow(clippy::cast_precision_loss)] // View dimensions and bounded row counts use float layout units.
    pub(super) fn render_grid(
        &mut self,
        tiles: Vec<AnyElement>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width = f32::from(window.viewport_size().width) / scale;
        let grid = self.grid_layouts.entry(self.active_workspace).or_default();
        grid.prepare(tiles.len(), width);
        if let Some(weights) = grid.shapes.get_mut(&grid.current) {
            let column_min = minimum_weight(
                width - GRID_PADDING * 2.0,
                GRID_GAP,
                weights.columns.len(),
                GRID_TILE_MIN_W,
            );
            constrain_weights(&mut weights.columns, column_min);
            let height = f32::from(self.grid_bounds.get().size.height) / scale;
            if height > 0.0 {
                for rows in &mut weights.rows {
                    let floor = minimum_weight(height, GRID_GAP, rows.len(), GRID_TILE_MIN_H);
                    constrain_weights(rows, floor);
                }
            }
        }
        let Some(weights) = grid.weights().cloned() else {
            return div().into_any_element();
        };
        let columns = weights.columns.len();
        let max_rows = weights.rows.iter().map(Vec::len).max().unwrap_or(1);
        let mut tiles: Vec<_> = tiles.into_iter().map(Some).collect();
        let bounds = self.grid_bounds.clone();
        let sidebar = cx.entity().downgrade();
        let mut layout = div()
            .id("session-grid-layout")
            .debug_selector(|| "session-grid-layout".into())
            .relative()
            .w_full()
            .h_full()
            .min_h(rpx(if max_rows > 1 {
                GRID_TILE_MIN_H * max_rows as f32 + GRID_GAP * (max_rows - 1) as f32
            } else {
                0.0
            }))
            .flex()
            .child(
                gpui::canvas(
                    move |rect, _, cx| {
                        let previous = bounds.replace(rect);
                        if previous.size != rect.size {
                            let _ = sidebar.update(cx, |_, cx| cx.notify());
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0(),
            );
        for (column, column_weight) in weights.columns.iter().copied().enumerate() {
            if column > 0 {
                layout = layout.child(self.grid_separator(
                    GridBoundary {
                        axis: GridAxis::Columns,
                        boundary: column - 1,
                        column: None,
                    },
                    cx,
                ));
            }
            let mut column_el = div()
                .flex()
                .flex_col()
                .flex_basis(gpui::px(0.0))
                .flex_grow(column_weight)
                .min_w_0()
                .h_full();
            for (row, row_weight) in weights.rows[column].iter().copied().enumerate() {
                if row > 0 {
                    column_el = column_el.child(self.grid_separator(
                        GridBoundary {
                            axis: GridAxis::Rows,
                            boundary: row - 1,
                            column: Some(column),
                        },
                        cx,
                    ));
                }
                let index = row * columns + column;
                if let Some(tile) = tiles.get_mut(index).and_then(Option::take) {
                    column_el = column_el.child(
                        div()
                            .id(("grid-tile", index))
                            .debug_selector(move || format!("grid-tile-{index}"))
                            .flex_basis(gpui::px(0.0))
                            .flex_grow(row_weight)
                            .min_h_0()
                            .w_full()
                            .min_w_0()
                            // Each shared edge has one owner; outer edges stay unframed.
                            .when(column + 1 < columns, gpui::Styled::border_r_1)
                            .when(row + 1 < weights.rows[column].len(), |tile| {
                                tile.border_b_1()
                            })
                            .border_color(c::BORDER_SOFT())
                            .overflow_hidden()
                            .child(tile),
                    );
                }
            }
            layout = layout.child(column_el);
        }
        div()
            .id("session-grid")
            .debug_selector(|| "session-grid".into())
            .size_full()
            .min_w_0()
            .min_h_0()
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .overflow_y_scroll()
            .p(rpx(GRID_PADDING))
            .child(layout)
            .on_mouse_move(
                cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                    let Some(drag) = this.grid_drag.clone() else {
                        return;
                    };
                    if !event.dragging() {
                        this.grid_drag = None;
                        return;
                    }
                    let bounds = this.grid_bounds.get();
                    let vertical = drag.boundary.axis == GridAxis::Columns;
                    let size = f32::from(if vertical {
                        bounds.size.width
                    } else {
                        bounds.size.height
                    });
                    let count = drag.initial.len();
                    let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
                    let available =
                        (size - GRID_GAP * scale * count.saturating_sub(1) as f32).max(1.0);
                    let current = f32::from(if vertical {
                        event.position.x
                    } else {
                        event.position.y
                    });
                    this.grid_resize(
                        drag.boundary,
                        (current - drag.origin) / available,
                        Some(drag.initial),
                        cx,
                    );
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.grid_drag = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.grid_drag = None),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        runtime::Runtime,
        settings::SettingsState,
        zoom::{CurrentPtyDims, ZoomState},
    };
    use gpui::{Entity, Render};
    struct GridHarness {
        sidebar: Entity<Sidebar>,
        count: usize,
    }
    impl Render for GridHarness {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let count = self.count;
            self.sidebar.update(cx, |sidebar, cx| {
                sidebar.render_grid(
                    (0..count)
                        .map(|index| {
                            div()
                                .size_full()
                                .child(format!("Session {index}"))
                                .into_any_element()
                        })
                        .collect(),
                    window,
                    cx,
                )
            })
        }
    }
    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    #[gpui::test]
    fn rendered_grid_adapts_and_resizes_without_replacing_session_owner(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(CurrentPtyDims::default());
            cx.set_global(ZoomState::new(1.0));
        });
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
            GridHarness { sidebar, count: 3 }
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        let first = cx.debug_bounds("grid-tile-0").unwrap();
        let second = cx.debug_bounds("grid-tile-1").unwrap();
        let third = cx.debug_bounds("grid-tile-2").unwrap();
        let grid = cx.debug_bounds("session-grid").unwrap();
        assert_eq!(first.top() - grid.top(), gpui::px(1.0));
        assert_eq!(first.left(), gpui::px(GRID_PADDING));
        assert_eq!(first.top(), second.top());
        assert_eq!(second.top(), third.top());
        assert!((f32::from(second.left() - first.right())).abs() < 1.0);
        assert!((f32::from(third.right()) - (1280.0 - GRID_PADDING)).abs() < 1.0);
        let owner = harness.read_with(cx, |harness, _| harness.sidebar.entity_id());
        let divider = cx.debug_bounds("grid-divider-columns-0").unwrap();
        assert_eq!(f32::from(divider.size.width), GRID_RESIZE_HIT);
        assert!((f32::from(divider.center().x - first.right())).abs() < 1.0);
        let separator = divider.center();
        cx.simulate_mouse_down(separator, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(separator, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_keystrokes("right");
        draw(cx);
        let keyboard_resized = cx.debug_bounds("grid-tile-0").unwrap();
        assert!(keyboard_resized.size.width > first.size.width);
        let separator = cx.debug_bounds("grid-divider-columns-0").unwrap().center();
        cx.simulate_mouse_down(separator, MouseButton::Left, gpui::Modifiers::default());
        let destination = gpui::point(separator.x + gpui::px(30.0), separator.y);
        cx.simulate_mouse_move(destination, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("grid-tile-0").unwrap().size.width > keyboard_resized.size.width);
        cx.simulate_mouse_move(separator, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert!(
            (f32::from(
                cx.debug_bounds("grid-tile-0").unwrap().size.width - keyboard_resized.size.width
            ))
            .abs()
                < 1.0
        );
        cx.simulate_mouse_move(destination, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(destination, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        let resized = cx.debug_bounds("grid-tile-0").unwrap();
        assert!(resized.size.width > keyboard_resized.size.width);
        assert!(resized.size.width > first.size.width);
        assert_eq!(
            harness.read_with(cx, |harness, _| harness.sidebar.entity_id()),
            owner
        );
        cx.simulate_resize(gpui::size(gpui::px(900.0), gpui::px(800.0)));
        draw(cx);
        let first = cx.debug_bounds("grid-tile-0").unwrap();
        let second = cx.debug_bounds("grid-tile-1").unwrap();
        let third = cx.debug_bounds("grid-tile-2").unwrap();
        assert_eq!(first.top(), second.top());
        assert!((f32::from(third.top() - first.bottom())).abs() < 1.0);
        assert_eq!(third.left(), first.left());
        let separator = cx.debug_bounds("grid-divider-row-0-0").unwrap().center();
        cx.simulate_mouse_down(separator, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(separator, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_keystrokes("down");
        draw(cx);
        assert!(cx.debug_bounds("grid-tile-0").unwrap().size.height > first.size.height);
        cx.simulate_resize(gpui::size(gpui::px(320.0), gpui::px(200.0)));
        draw(cx);
        let first = cx.debug_bounds("grid-tile-0").unwrap();
        let second = cx.debug_bounds("grid-tile-1").unwrap();
        assert_eq!(first.left(), second.left());
        assert!((f32::from(second.top() - first.bottom())).abs() < 1.0);
        assert!(first.right() <= gpui::px(320.0 - GRID_PADDING));
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        assert!(
            (f32::from(cx.debug_bounds("grid-tile-0").unwrap().size.width - resized.size.width))
                .abs()
                < 1.0
        );
    }
    #[test]
    fn constrained_weights_keep_minimum_after_window_shrinks() {
        let mut weights = vec![0.165, 0.5, 0.335];
        let floor = minimum_weight(1000.0, GRID_GAP, 3, GRID_TILE_MIN_W);
        constrain_weights(&mut weights, floor);
        assert!(weights.iter().all(|weight| *weight >= floor));
        assert!((weights.iter().sum::<f32>() - 1.0).abs() < 0.0001);
    }
    #[test]
    fn minimums_and_workspace_weight_caches_preserve_ratios() {
        let mut workspace = WorkspaceGrid::default();
        workspace.prepare(3, 1280.0);
        let boundary = GridBoundary {
            axis: GridAxis::Columns,
            boundary: 0,
            column: None,
        };
        let weights = workspace.weights_mut(boundary).unwrap();
        transfer_pair(weights, 0, 1.0, 0.15);
        assert!((weights[1] - 0.15).abs() < f32::EPSILON);
        let saved = weights.clone();
        workspace.prepare(3, 320.0);
        workspace.prepare(3, 1280.0);
        assert_eq!(workspace.weights().unwrap().columns, saved);
        let mut other = WorkspaceGrid::default();
        other.prepare(3, 1280.0);
        assert_ne!(other.weights().unwrap().columns, saved);
    }
}
