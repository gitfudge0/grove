//! Pure grid math for the tile grid; every function here ports an iced original (see each function's doc for its `src/gui/...` origin).
//!
//! Deviation: `tile_order` is `Vec<SessionId>` (stable ids), not a sessions-vec index, so removing a session can't silently re-point a stored order.
//! Deviation: `slide_progress`'s `EaseOutCubic` is arithmetic since `iced::animation::Easing` must not become a dep.

use std::time::{Duration, Instant};

/// The axis a grid boundary divides. `Columns` moves left/right; `Rows` moves up/down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridAxis {
    Columns,
    Rows,
}

/// A stable address for one split in the column-first grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridBoundary {
    pub axis: GridAxis,
    pub boundary: usize,
    /// Set only for a row split, because each column owns independent row weights.
    pub column: Option<usize>,
}

/// One weighted region in physical or logical pixel space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WeightedSpan {
    pub start: f32,
    pub size: f32,
}

/// Used when a caller cannot supply a trustworthy tile minimum. This is positional grid geometry,
/// so it stays in the module that owns the calculation rather than joining the shared token scale.
pub const MIN_REGION_FALLBACK_PX: f32 = 64.0;

/// Converts a tile's cell requirement and design-space overhead into an outer minimum measured in
/// device pixels. Physical overhead covers hairlines and borders, which deliberately do not scale
/// with the application's rem zoom.
#[must_use]
pub fn tile_minimum_px(
    cell_design_px: f32,
    minimum_cells: f32,
    design_overhead_px: f32,
    physical_overhead_px: f32,
    zoom: f32,
) -> f32 {
    let zoom = if zoom.is_finite() && zoom > 0.0 {
        zoom
    } else {
        1.0
    };
    cell_design_px
        .mul_add(minimum_cells, design_overhead_px)
        .mul_add(zoom, physical_overhead_px.max(0.0))
}

/// The device-pixel content left after subtracting zoom-scaled design overhead and unscaled
/// physical hairlines/borders from an outer tile span.
#[must_use]
pub fn usable_tile_px(
    outer_px: f32,
    design_overhead_px: f32,
    physical_overhead_px: f32,
    zoom: f32,
) -> f32 {
    let zoom = if zoom.is_finite() && zoom > 0.0 {
        zoom
    } else {
        1.0
    };
    (outer_px - design_overhead_px * zoom - physical_overhead_px.max(0.0)).max(0.0)
}

/// Equal normalized weights for `count` regions.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn equal_weights(count: usize) -> Vec<f32> {
    if count == 0 {
        return Vec::new();
    }
    vec![1.0 / count as f32; count]
}

/// Normalizes positive finite weights in place. Invalid input falls back to equal weights.
pub fn normalize_weights(weights: &mut [f32]) {
    if weights.is_empty() {
        return;
    }
    let valid = weights
        .iter()
        .all(|weight| weight.is_finite() && *weight > 0.0);
    let sum: f32 = weights.iter().sum();
    if !valid || !sum.is_finite() || sum <= f32::EPSILON {
        #[allow(clippy::cast_precision_loss)]
        let equal = 1.0 / weights.len() as f32;
        weights.fill(equal);
        return;
    }
    for weight in weights {
        *weight /= sum;
    }
}

/// Returns the normalized minimum weight for one region. If every requested minimum cannot fit,
/// the caller still gets a positive equal-share floor instead of a zero-sized tile.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn minimum_weight(total_px: f32, gap_px: f32, count: usize, minimum_px: f32) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let minimum_px = if minimum_px.is_finite() && minimum_px > 0.0 {
        minimum_px
    } else {
        MIN_REGION_FALLBACK_PX
    };
    let available = (total_px - gap_px.max(0.0) * count.saturating_sub(1) as f32).max(1.0);
    (minimum_px / available).min(1.0 / count as f32)
}

/// Transfers normalized weight across one adjacent pair while keeping the pair total fixed.
/// `delta > 0` moves the boundary right/down, growing the region before it.
pub fn transfer_pair(weights: &mut [f32], boundary: usize, delta: f32, minimum: f32) -> bool {
    let Some(right) = boundary.checked_add(1) else {
        return false;
    };
    if right >= weights.len() || !delta.is_finite() {
        return false;
    }
    normalize_weights(weights);
    let pair_total = weights[boundary] + weights[right];
    let floor = minimum.max(f32::EPSILON).min(pair_total / 2.0);
    let next_left = (weights[boundary] + delta).clamp(floor, pair_total - floor);
    let next_right = pair_total - next_left;
    let changed = (weights[boundary] - next_left).abs() > f32::EPSILON;
    weights[boundary] = next_left;
    weights[right] = next_right;
    changed
}

/// Resets only the addressed pair to equal shares of its current pair total.
pub fn reset_pair(weights: &mut [f32], boundary: usize) -> bool {
    let Some(right) = boundary.checked_add(1) else {
        return false;
    };
    if right >= weights.len() {
        return false;
    }
    normalize_weights(weights);
    let equal = f32::midpoint(weights[boundary], weights[right]);
    let changed = (weights[boundary] - equal).abs() > f32::EPSILON;
    weights[boundary] = equal;
    weights[right] = equal;
    changed
}

/// Converts normalized weights into exact spans after reserving the 1px gaps.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn weighted_spans(total_px: f32, gap_px: f32, weights: &[f32]) -> Vec<WeightedSpan> {
    if weights.is_empty() {
        return Vec::new();
    }
    let mut normalized = weights.to_vec();
    normalize_weights(&mut normalized);
    let gap = gap_px.max(0.0);
    let available = (total_px - gap * weights.len().saturating_sub(1) as f32).max(0.0);
    let mut cursor = 0.0;
    normalized
        .into_iter()
        .map(|weight| {
            let span = WeightedSpan {
                start: cursor,
                size: available * weight,
            };
            cursor += span.size + gap;
            span
        })
        .collect()
}

/// Picks the boundary next to a tile in the requested direction.
#[must_use]
pub fn boundary_adjacent_to_tile(
    tile_idx: usize,
    tile_count: usize,
    dx: i32,
    dy: i32,
) -> Option<GridBoundary> {
    if tile_idx >= tile_count || (dx == 0) == (dy == 0) {
        return None;
    }
    let (cols, _) = grid_layout(tile_count);
    let column = tile_idx % cols;
    let row = tile_idx / cols;
    if dx < 0 && column > 0 {
        Some(GridBoundary {
            axis: GridAxis::Columns,
            boundary: column - 1,
            column: None,
        })
    } else if dx > 0 && column + 1 < cols {
        Some(GridBoundary {
            axis: GridAxis::Columns,
            boundary: column,
            column: None,
        })
    } else if dy < 0 && row > 0 {
        Some(GridBoundary {
            axis: GridAxis::Rows,
            boundary: row - 1,
            column: Some(column),
        })
    } else if dy > 0 && tile_idx + cols < tile_count {
        Some(GridBoundary {
            axis: GridAxis::Rows,
            boundary: row,
            column: Some(column),
        })
    } else {
        None
    }
}

/// Port of `src/gui/metrics.rs:325-330`.
#[must_use]
pub fn grid_layout(n: usize) -> (usize, usize) {
    let n = n.max(1);
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let cols = ((n as f64).sqrt().ceil() as usize).clamp(1, 4);
    let rows = n.div_ceil(cols).min(4);
    (cols, rows)
}

/// Vertical moves require the naive target index to exist (no "nearest in column" fallback); horizontal moves clamp the row downward to the largest row with a tile in the target column.
#[must_use]
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
pub fn grid_neighbor(i: usize, n: usize, dx: i32, dy: i32) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let (cols, _rows) = grid_layout(n);
    let cols = cols as i32;
    let row = i as i32 / cols;
    let col = i as i32 % cols;
    let target_col = col + dx;
    if target_col < 0 || target_col >= cols {
        return None;
    }
    if dx == 0 {
        let target_row = row + dy;
        if target_row < 0 {
            return None;
        }
        let idx = target_row * cols + target_col;
        return (idx >= 0 && (idx as usize) < n).then_some(idx as usize);
    }
    let mut r = row;
    loop {
        if r < 0 {
            return None;
        }
        let idx = r * cols + target_col;
        if idx >= 0 && (idx as usize) < n {
            return Some(idx as usize);
        }
        r -= 1;
    }
}

/// Equal-cell approximation used only to size the slide animation's draw offset (it settles exactly at `t = 1`). Port of `src/gui/metrics.rs:367-375`.
#[must_use]
#[allow(dead_code)]
// Kept as the stable equal-grid approximation for callers that do not have a `GridSizing` snapshot.
#[allow(clippy::cast_precision_loss)]
pub fn grid_tile_size(win_w: f32, win_h: f32, zoom: f32, chrome_h: f32, n: usize) -> (f32, f32) {
    let (cols, rows) = grid_layout(n);
    let zoom = zoom.max(0.1);
    let workspace_w = win_w / zoom;
    let workspace_h = win_h / zoom - chrome_h;
    (
        (workspace_w - (cols as f32 - 1.0)) / cols as f32,
        (workspace_h - (rows as f32 - 1.0)) / rows as f32,
    )
}

/// Port of `src/gui/launcher.rs:43-49`.
pub fn swap_tiles<T>(order: &mut [T], src: usize, dst: usize) {
    if src == dst || src >= order.len() || dst >= order.len() {
        return;
    }
    order.swap(src, dst);
}

/// The key in `Store::grid_order`; reconciliation treats a duplicate key as "first live match wins". Port of `src/gui/launcher.rs:53-55`.
#[must_use]
pub fn session_grid_key(project: &str, wt_path: &str) -> String {
    format!("{project}::{wt_path}")
}

/// Returns positions into `live_keys` (see the module doc's deviation note); the caller maps them to `SessionId`s. Port of `src/gui/launcher.rs:63-81`.
#[must_use]
pub fn reconcile_tile_order(live_keys: &[String], saved_order: &[String]) -> Vec<usize> {
    let mut order = Vec::with_capacity(live_keys.len());
    let mut used = vec![false; live_keys.len()];
    for key in saved_order {
        if let Some(idx) = live_keys.iter().position(|k| k == key) {
            if !used[idx] {
                used[idx] = true;
                order.push(idx);
            }
        }
    }
    for (idx, was_used) in used.into_iter().enumerate() {
        if !was_used {
            order.push(idx);
        }
    }
    order
}

/// Refocuses the slot the killed tile was in (clamped to the new last slot if it was last). Port of `src/gui/update/shortcuts.rs:574-580`.
// TODO(unwired): passing test but no caller — killing the focused grid tile never consults it.
#[allow(dead_code)]
#[must_use]
pub fn grid_focus_after_kill(killed_pos: Option<usize>, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(killed_pos.unwrap_or(0).min(len - 1))
}

/// Port of `src/gui/update/shortcuts.rs:564-566`.
#[must_use]
pub fn should_sync_grid_focus(grid_view: bool, grid_view_before_zen: bool) -> bool {
    grid_view || grid_view_before_zen
}

/// `src/gui/update/shortcuts.rs:631`.
pub const GRID_SLIDE: Duration = Duration::from_millis(150);

/// `EaseOutCubic`: `1 - (1 - t)^3`.
fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

/// Port of `src/gui/update/shortcuts.rs:645-650`.
#[must_use]
pub fn slide_progress(start: Instant, now: Instant) -> f32 {
    let elapsed = now.saturating_duration_since(start);
    if elapsed >= GRID_SLIDE {
        return 1.0;
    }
    ease_out_cubic(elapsed.as_secs_f32() / GRID_SLIDE.as_secs_f32())
}

/// Must be called after [`swap_tiles`]: `src`/`dst` are the post-swap positions. Port of `begin_grid_slide`, `src/gui/update/layout.rs:363-377`.
#[must_use]
#[allow(clippy::cast_possible_wrap)]
pub fn slide_offsets(src: usize, dst: usize, n: usize) -> [(usize, i32, i32); 2] {
    let (cols, _) = grid_layout(n);
    let cols = cols.max(1);
    let cell = |i: usize| ((i % cols) as i32, (i / cols) as i32);
    let (src_col, src_row) = cell(src);
    let (dst_col, dst_row) = cell(dst);
    [
        (dst, src_col - dst_col, src_row - dst_row),
        (src, dst_col - src_col, dst_row - src_row),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `src/gui/metrics.rs:558-572`.
    #[test]
    fn grid_layout_picks_sensible_dimensions() {
        assert_eq!(grid_layout(1), (1, 1));
        assert_eq!(grid_layout(2), (2, 1));
        assert_eq!(grid_layout(3), (2, 2));
        assert_eq!(grid_layout(4), (2, 2));
        assert_eq!(grid_layout(5), (3, 2));
        assert_eq!(grid_layout(6), (3, 2));
        assert_eq!(grid_layout(7), (3, 3));
        assert_eq!(grid_layout(9), (3, 3));
        assert_eq!(grid_layout(10), (4, 3));
        assert_eq!(grid_layout(16), (4, 4));
        assert_eq!(grid_layout(20), (4, 4)); // capped at 4×4
    }

    /// `src/gui/update/shortcuts.rs:1242-1251`, ported verbatim.
    #[test]
    fn horizontal_moves_clamp_the_row_downward_in_a_ragged_grid() {
        assert_eq!(grid_neighbor(2, 3, 1, 0), Some(1));
        assert_eq!(grid_neighbor(1, 3, -1, 0), Some(0));
        assert_eq!(grid_neighbor(0, 3, -1, 0), None);
    }

    #[test]
    fn vertical_moves_require_the_naive_target_to_exist() {
        assert_eq!(grid_neighbor(1, 3, 0, 1), None);
        assert_eq!(grid_neighbor(0, 3, 0, 1), Some(2));
        assert_eq!(grid_neighbor(1, 4, 0, 1), Some(3));
        assert_eq!(grid_neighbor(0, 4, 0, -1), None);
        assert_eq!(grid_neighbor(2, 4, 0, -1), Some(0));
    }

    /// `src/gui/update/shortcuts.rs:1254-1259`.
    #[test]
    fn moves_off_the_column_edge_are_none() {
        assert_eq!(grid_neighbor(0, 4, 1, 0), Some(1));
        assert_eq!(grid_neighbor(0, 4, 0, 1), Some(2));
        assert_eq!(grid_neighbor(3, 4, 1, 0), None);
        assert_eq!(grid_neighbor(0, 4, -1, 0), None);
        assert_eq!(grid_neighbor(0, 0, 1, 0), None);
    }

    /// `src/gui/launcher.rs:335-348`.
    #[test]
    fn swap_tiles_swaps_positions() {
        let mut order = vec![0, 1, 2, 3, 4];
        swap_tiles(&mut order, 0, 3);
        assert_eq!(order, vec![3, 1, 2, 0, 4]);
        let mut same = vec![1, 2];
        swap_tiles(&mut same, 1, 1);
        assert_eq!(same, vec![1, 2]);
        let mut oob = vec![1];
        swap_tiles(&mut oob, 0, 5);
        assert_eq!(oob, vec![1]);
    }

    /// `src/gui/launcher.rs:350-353`.
    #[test]
    fn session_grid_key_combines_project_and_wt_path() {
        assert_eq!(session_grid_key("proj", "/wt/a"), "proj::/wt/a");
    }

    /// `src/gui/launcher.rs:355-362`.
    #[test]
    fn reconcile_tile_order_uses_saved_order_first() {
        let live = vec!["p::a".to_string(), "p::b".to_string(), "p::c".to_string()];
        let saved = vec!["p::c".to_string(), "p::a".to_string()];
        assert_eq!(reconcile_tile_order(&live, &saved), vec![2, 0, 1]);
    }

    #[test]
    fn reconcile_tile_order_appends_all_when_saved_empty() {
        let live = vec!["p::a".to_string(), "p::b".to_string()];
        assert_eq!(reconcile_tile_order(&live, &[]), vec![0, 1]);
    }

    #[test]
    fn reconcile_tile_order_ignores_saved_keys_with_no_live_match() {
        let live = vec!["p::a".to_string()];
        let saved = vec!["p::stale".to_string(), "p::a".to_string()];
        assert_eq!(reconcile_tile_order(&live, &saved), vec![0]);
    }

    #[test]
    fn reconcile_tile_order_handles_duplicate_saved_keys() {
        let live = vec!["p::a".to_string(), "p::b".to_string()];
        let saved = vec!["p::a".to_string(), "p::a".to_string()];
        assert_eq!(reconcile_tile_order(&live, &saved), vec![0, 1]);
    }

    /// `src/gui/update/shortcuts.rs:574-580`.
    #[test]
    fn grid_focus_after_kill_refills_the_killed_slot() {
        assert_eq!(grid_focus_after_kill(Some(1), 3), Some(1));
        // Killing the last tile clamps to the new last slot.
        assert_eq!(grid_focus_after_kill(Some(3), 3), Some(2));
        // Nothing left.
        assert_eq!(grid_focus_after_kill(Some(0), 0), None);
        // No known position falls back to the head.
        assert_eq!(grid_focus_after_kill(None, 2), Some(0));
    }

    /// `src/gui/update/shortcuts.rs:564-566`.
    #[test]
    fn grid_focus_syncs_only_in_grid_or_zen_entered_from_grid() {
        assert!(should_sync_grid_focus(true, false));
        assert!(should_sync_grid_focus(false, true));
        assert!(!should_sync_grid_focus(false, false));
    }

    #[test]
    fn slide_progress_starts_at_zero_and_saturates_at_one() {
        let t0 = Instant::now();
        assert!((slide_progress(t0, t0) - 0.0).abs() < f32::EPSILON);
        assert!((slide_progress(t0, t0 + GRID_SLIDE) - 1.0).abs() < f32::EPSILON);
        assert!(
            (slide_progress(t0, t0 + GRID_SLIDE + Duration::from_secs(1)) - 1.0).abs()
                < f32::EPSILON
        );
        // `now` before `start` saturates at 0 rather than going negative.
        assert!((slide_progress(t0 + GRID_SLIDE, t0) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn slide_progress_is_monotone_non_decreasing() {
        let t0 = Instant::now();
        let mut prev = 0.0;
        for ms in 0..=200 {
            let p = slide_progress(t0, t0 + Duration::from_millis(ms));
            assert!(p >= prev, "regressed at {ms}ms: {p} < {prev}");
            assert!((0.0..=1.0).contains(&p));
            prev = p;
        }
    }

    #[test]
    fn slide_progress_follows_ease_out_cubic() {
        let t0 = Instant::now();
        // EaseOutCubic at the midpoint: 1 - 0.5^3 = 0.875.
        let mid = slide_progress(t0, t0 + Duration::from_millis(75));
        assert!((mid - 0.875).abs() < 1e-4, "midpoint was {mid}");
        // At a quarter: 1 - 0.75^3 = 0.578125.
        let q = slide_progress(t0, t0 + Duration::from_millis(37));
        assert!(q > 0.5 && q < 0.62, "quarter was {q}");
    }

    /// `src/gui/update/layout.rs:363-377`. Called post-swap, so each entry's offset points back where the tile came from.
    #[test]
    fn slide_offsets_point_each_tile_back_where_it_came_from() {
        // 4 tiles → 2 cols. Horizontal swap 0 <-> 1 (same row).
        assert_eq!(slide_offsets(0, 1, 4), [(1, -1, 0), (0, 1, 0)]);
        // Vertical swap 0 <-> 2 (same column).
        assert_eq!(slide_offsets(0, 2, 4), [(2, 0, -1), (0, 0, 1)]);
        // Diagonal swap 0 <-> 3.
        assert_eq!(slide_offsets(0, 3, 4), [(3, -1, -1), (0, 1, 1)]);
    }

    #[test]
    fn equal_and_invalid_weights_normalize_without_zero_regions() {
        assert_eq!(equal_weights(0), Vec::<f32>::new());
        assert_eq!(equal_weights(2), vec![0.5, 0.5]);
        let mut invalid = vec![f32::NAN, 0.0, -1.0];
        normalize_weights(&mut invalid);
        for weight in invalid {
            assert!((weight - 1.0 / 3.0).abs() < 1e-6);
        }
    }

    #[test]
    fn pair_transfer_preserves_sum_and_clamps_both_tiles() {
        let mut weights = vec![0.25, 0.5, 0.25];
        assert!(transfer_pair(&mut weights, 0, 0.4, 0.2));
        assert!((weights[0] - 0.55).abs() < 1e-6);
        assert!((weights[1] - 0.2).abs() < 1e-6);
        assert!((weights.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!(!transfer_pair(&mut weights, 8, 0.1, 0.2));
    }

    #[test]
    fn reset_changes_only_the_addressed_pair() {
        let mut weights = vec![0.2, 0.5, 0.3];
        assert!(reset_pair(&mut weights, 0));
        assert!((weights[0] - 0.35).abs() < 1e-6);
        assert!((weights[1] - 0.35).abs() < 1e-6);
        assert!((weights[2] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn weighted_spans_reserve_hairline_gaps() {
        let spans = weighted_spans(101.0, 1.0, &[0.25, 0.75]);
        assert_eq!(spans.len(), 2);
        assert!((spans[0].start - 0.0).abs() < 1e-6);
        assert!((spans[0].size - 25.0).abs() < 1e-6);
        assert!((spans[1].start - 26.0).abs() < 1e-6);
        assert!((spans[1].size - 75.0).abs() < 1e-6);
    }

    #[test]
    fn weighted_spans_keep_seams_one_physical_pixel_across_zoom_levels() {
        for zoom in [0.6, 1.0, 1.75] {
            let spans = weighted_spans(200.0 * zoom, 1.0, &[0.4, 0.6]);
            let gap = spans[1].start - (spans[0].start + spans[0].size);
            assert!((gap - 1.0).abs() < 1e-5, "zoom {zoom}: gap {gap}");
        }
    }

    #[test]
    fn minimum_weight_uses_fallback_and_degrades_to_equal_on_tiny_spans() {
        assert!((minimum_weight(400.0, 1.0, 2, 100.0) - 100.0 / 399.0).abs() < 1e-6);
        assert_eq!(minimum_weight(50.0, 1.0, 2, 100.0), 0.5);
        assert!(minimum_weight(400.0, 1.0, 2, f32::NAN) > 0.0);
    }

    #[test]
    fn tile_minimum_keeps_scaled_content_and_physical_hairlines_in_separate_spaces() {
        for zoom in [0.6, 1.0, 1.75] {
            let minimum = tile_minimum_px(8.0, 10.0, 32.0, 3.0, zoom);
            let usable = usable_tile_px(minimum, 32.0, 3.0, zoom);
            assert!((usable - 80.0 * zoom).abs() < 1e-5, "zoom {zoom}");
        }
    }

    #[test]
    fn boundary_selection_respects_ragged_columns() {
        assert_eq!(
            boundary_adjacent_to_tile(0, 3, 1, 0),
            Some(GridBoundary {
                axis: GridAxis::Columns,
                boundary: 0,
                column: None,
            })
        );
        assert_eq!(boundary_adjacent_to_tile(1, 3, 0, 1), None);
        assert_eq!(
            boundary_adjacent_to_tile(2, 3, 0, -1),
            Some(GridBoundary {
                axis: GridAxis::Rows,
                boundary: 0,
                column: Some(0),
            })
        );
    }
}
