//! Pure grid math for the tile grid (Plan 07 Task 1).
//!
//! Every function here is a port of an iced original, reimplemented rather than
//! shared because the iced crate is read-only-as-oracle:
//!
//! | here | iced origin |
//! |---|---|
//! | [`grid_layout`] | `src/gui/metrics.rs:325-330` |
//! | [`grid_neighbor`] | `src/gui/update/shortcuts.rs:581-627` |
//! | [`session_grid_key`] | `src/gui/launcher.rs:53-55` |
//! | [`reconcile_tile_order`] | `src/gui/launcher.rs:63-81` |
//! | [`swap_tiles`] | `src/gui/launcher.rs:43-49` |
//! | [`grid_focus_after_kill`] | `src/gui/update/shortcuts.rs:574-580` |
//! | [`should_sync_grid_focus`] | `src/gui/update/shortcuts.rs:564-566` |
//! | [`GRID_SLIDE`] / [`slide_progress`] | `src/gui/update/shortcuts.rs:629-650` |
//! | [`slide_offsets`] | `src/gui/update/layout.rs:363-377` |
//!
//! **Deviation (Plan 07 Task 1 Step 2).** iced keys `tile_order` by *index into
//! `App::sessions`*, which is the only reason `reconcile_grid_after_teardown`
//! exists there: removing a session shifts every later index, so a stored order
//! silently re-points at the wrong agents. grove-gpui has stable `SessionId`s
//! (Plan 05 Task 2), so `tile_order` is a `Vec<SessionId>` and that hazard is
//! gone. Reconciliation itself stays, because `Store::grid_order` persists
//! **string keys** — that is the cross-restart identity, and it still has to be
//! matched against the live set. So [`reconcile_tile_order`] keeps operating on
//! keys and only its return type is adapted: it yields *positions* into the
//! live slice, which the caller maps to ids.
//!
//! **Deviation (carried amendment 6).** `slide_progress`'s easing is written
//! out arithmetically: `iced::animation::Easing` is not a dependency of
//! grove-gpui and must not become one. `EaseOutCubic` is `1 - (1 - t)^3`.

// The slide helpers' only consumer is the tile painter (Task 4); the layout
// helpers' is the grid view. Same pattern as `keymap.rs` and
// `workspace_state.rs` carry for their not-yet-landed consumers.
#![allow(dead_code)]

use std::time::{Duration, Instant};

/// Grid dimensions `(cols, rows)` for `n` sessions.
/// Formula: cols = ceil(sqrt(n)).clamp(1,4), rows = ceil(n/cols).min(4).
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

/// Tile index reached by moving `(dx, dy)` from tile `i` in a grid of `n`
/// tiles, or `None` if there's no such tile. Tiles are numbered row-major
/// (`tile_idx = row * cols + col`, see `grid_layout`/`grid_workspace`) but
/// rendered into per-column containers that skip any `tile_idx >= n`, so a
/// short column simply stacks the tiles it has, full height. E.g. n=3 gives
/// cols=2, rows=2: the left column shows tiles 0 (top) and 2 (bottom); the
/// right column shows only tile 1, spanning the full height.
///
/// Vertical moves (`dx == 0`) require the naive target index to exist —
/// there's no "nearest tile in that column" fallback, since the columns
/// don't share a row grid. Horizontal moves (`dy == 0`) instead clamp the row
/// downward to the largest row `<= target_row` that has a tile in the target
/// column, matching what's visually below the cursor's row.
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
    // Horizontal move: clamp the row downward to the largest row that still
    // has a tile in the target column.
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

/// Nominal (equal-cell) grid tile size in logical pixels, ignoring the
/// column-height stacking that ragged grids do. Used **only** to size the
/// slide animation's draw offset, where an equal-cell approximation is good
/// enough — it settles exactly at `t = 1`. Port of `src/gui/metrics.rs:367-375`.
///
/// This is a *layout* fact, not a PTY fact, so carried amendment 1 does not
/// supersede it: no PTY is sized from it.
#[must_use]
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

/// Commit a tile drag with swap semantics: the dragged tile and the drop
/// target trade positions directly; all other tiles are unaffected. No-op
/// when the indices match or fall outside the order.
/// Port of `src/gui/launcher.rs:43-49`.
pub fn swap_tiles<T>(order: &mut [T], src: usize, dst: usize) {
    if src == dst || src >= order.len() || dst >= order.len() {
        return;
    }
    order.swap(src, dst);
}

/// Stable identity for a session across app restarts, used as the key in
/// `Store::grid_order`. Not unique if the same worktree somehow has two
/// sessions (shouldn't happen in practice); reconciliation treats a
/// duplicate key as "first live match wins".
/// Port of `src/gui/launcher.rs:53-55`.
#[must_use]
pub fn session_grid_key(project: &str, wt_path: &str) -> String {
    format!("{project}::{wt_path}")
}

/// Compute a grid `tile_order` from the currently live session keys and a saved
/// key order loaded from disk. Live sessions matching a saved key appear first,
/// in saved order; any live session with no match (new, or never previously
/// seen) is appended afterward in its current vector order. A saved key with no
/// live match (a closed session) is simply skipped.
///
/// Returns **positions into `live_keys`** — see the module doc's deviation note;
/// the caller maps them to `SessionId`s.
/// Port of `src/gui/launcher.rs:63-81`.
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

/// `tile_order` position to refocus after killing the focused tile, given the
/// killed tile's position before removal (`killed_pos`) and the tile count
/// after removal (`len`). The killed slot is filled by whatever shifted into
/// it, so we refocus that same slot; if the killed tile was last, clamp to
/// the new last slot instead.
/// Port of `src/gui/update/shortcuts.rs:574-580`.
#[must_use]
pub fn grid_focus_after_kill(killed_pos: Option<usize>, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(killed_pos.unwrap_or(0).min(len - 1))
}

/// Whether `grid_focused` should follow the active session: only while the grid
/// is showing, or while zen was entered *from* the grid and will restore it.
/// Port of `src/gui/update/shortcuts.rs:564-566`.
#[must_use]
pub fn should_sync_grid_focus(grid_view: bool, grid_view_before_zen: bool) -> bool {
    grid_view || grid_view_before_zen
}

/// Duration of the draw-only tile-slide animation triggered by a grid
/// reorder (drag or keyboard swap). `src/gui/update/shortcuts.rs:631`.
pub const GRID_SLIDE: Duration = Duration::from_millis(150);

/// `EaseOutCubic`, written out (carried amendment 6): `1 - (1 - t)^3`.
fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

/// Eased progress `[0, 1]` for a [`GRID_SLIDE`]-duration animation that started
/// at `start`, evaluated at `now`.
/// Port of `src/gui/update/shortcuts.rs:645-650`.
#[must_use]
pub fn slide_progress(start: Instant, now: Instant) -> f32 {
    let elapsed = now.saturating_duration_since(start);
    if elapsed >= GRID_SLIDE {
        return 1.0;
    }
    ease_out_cubic(elapsed.as_secs_f32() / GRID_SLIDE.as_secs_f32())
}

/// The two tiles that just swapped places in `tile_order`, each with the
/// `(d_col, d_row)` offset that points back at where it came from, so the
/// painter can translate the drawing and ease it out to zero.
///
/// Must be called **after** [`swap_tiles`]: `src`/`dst` are the tile-order
/// positions the two tiles now occupy (post-swap).
/// Port of `begin_grid_slide`, `src/gui/update/layout.rs:363-377`.
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
    /// n=3 → cols=2, rows=2. Left column: 0 (top), 2 (bottom). Right column:
    /// 1 only, spanning the full height. Moving right from tile 2 (row 1)
    /// clamps the row *downward* to row 0, the largest row with a tile in
    /// column 1 — landing on tile 1.
    #[test]
    fn horizontal_moves_clamp_the_row_downward_in_a_ragged_grid() {
        assert_eq!(grid_neighbor(2, 3, 1, 0), Some(1));
        assert_eq!(grid_neighbor(1, 3, -1, 0), Some(0));
        assert_eq!(grid_neighbor(0, 3, -1, 0), None);
    }

    #[test]
    fn vertical_moves_require_the_naive_target_to_exist() {
        // n=3: from tile 1 (row 0, col 1) down would be index 3 ≥ n.
        assert_eq!(grid_neighbor(1, 3, 0, 1), None);
        assert_eq!(grid_neighbor(0, 3, 0, 1), Some(2));
        // n=4: the same move lands on tile 3.
        assert_eq!(grid_neighbor(1, 4, 0, 1), Some(3));
        // Up from the top row falls off.
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

    /// `src/gui/update/layout.rs:363-377`. Called post-swap, so each entry's
    /// offset points back at where that tile came from.
    #[test]
    fn slide_offsets_point_each_tile_back_where_it_came_from() {
        // 4 tiles → 2 cols. Horizontal swap 0 <-> 1 (same row).
        assert_eq!(slide_offsets(0, 1, 4), [(1, -1, 0), (0, 1, 0)]);
        // Vertical swap 0 <-> 2 (same column).
        assert_eq!(slide_offsets(0, 2, 4), [(2, 0, -1), (0, 0, 1)]);
        // Diagonal swap 0 <-> 3.
        assert_eq!(slide_offsets(0, 3, 4), [(3, -1, -1), (0, 1, 1)]);
    }
}
