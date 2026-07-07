//! Pure selection / navigation logic for the Agent View session launcher.
//! Kept free of Iced so it can be unit-tested without a GUI. The launcher's
//! transient state lives in `crate::app::Modal::SessionLauncher`; these helpers
//! compute the next state and derived display strings.

/// Clamp `v + delta` into `[0, len)`. Saturates at both ends; returns 0 when
/// `len == 0` (an empty column has no valid selection).
pub fn clamp(v: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let max = len - 1;
    let next = (v as i64 + delta as i64).clamp(0, max as i64);
    next as usize
}

/// Move the focused column left (`delta < 0`) or right, clamped to `[0, 2]`
/// (0 = project, 1 = worktree, 2 = agent).
pub fn move_column(col: u8, delta: i32) -> u8 {
    let next = (col as i32 + delta).clamp(0, 2);
    next as u8
}

/// Default session label following Grove's spawn convention: the project name
/// for the main checkout, otherwise the worktree path basename.
pub fn default_label(is_main: bool, project_name: &str, wt_path: &str) -> String {
    if is_main {
        project_name.to_string()
    } else {
        crate::app::path_basename(wt_path)
    }
}

/// Footer breadcrumb text: `project › branch › agent`.
pub fn breadcrumb(project_name: &str, branch: &str, agent_label: &str) -> String {
    format!("{project_name} › {branch} › {agent_label}")
}

/// Compute `(proj, wt, agent)` after an up/down (`delta = ±1`) move in the
/// focused column. Moving within the project column resets `wt` to 0 (the
/// worktree list changes with the project). Lengths are clamped independently.
pub fn nav_within_column(
    col: u8,
    proj: usize,
    wt: usize,
    agent: usize,
    delta: i32,
    proj_len: usize,
    wt_len: usize,
    agent_len: usize,
) -> (usize, usize, usize) {
    match col {
        0 => (clamp(proj, delta, proj_len), 0, agent),
        1 => (proj, clamp(wt, delta, wt_len), agent),
        _ => (proj, wt, clamp(agent, delta, agent_len)),
    }
}

/// Adjust `tile_order` after a new session was inserted at index `at` in the
/// sessions vec. Every existing tile index `>= at` shifts up by one (those
/// sessions moved right), then the new session (`at`) is appended so it shows
/// as the last tile. Keeps `tile_order` in sync with a mid-vector insert.
pub fn insert_into_tile_order(tile_order: &mut Vec<usize>, at: usize) {
    for idx in tile_order.iter_mut() {
        if *idx >= at {
            *idx += 1;
        }
    }
    tile_order.push(at);
}

/// Commit a tile drag with swap semantics: the dragged tile and the drop
/// target trade positions directly; all other tiles are unaffected. No-op
/// when the indices match or fall outside the order.
pub fn swap_tiles(order: &mut Vec<usize>, src: usize, dst: usize) {
    if src == dst || src >= order.len() || dst >= order.len() {
        return;
    }
    order.swap(src, dst);
}

/// Stable identity for a session across app restarts, used as the key in
/// `Store::grid_order`. Not unique if the same worktree somehow has two
/// sessions (shouldn't happen in practice); reconciliation treats a
/// duplicate key as "first live match wins".
pub fn session_grid_key(project: &str, wt_path: &str) -> String {
    format!("{project}::{wt_path}")
}

/// Compute a grid `tile_order` from the currently live session keys
/// (indexed the same as `App::sessions`) and a saved key order loaded from
/// disk. Live sessions matching a saved key appear first, in saved order;
/// any live session with no match (new, or never previously seen) is
/// appended afterward in its current vector order. A saved key with no
/// live match (a closed session) is simply skipped.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_saturates_and_handles_empty() {
        assert_eq!(clamp(0, -1, 3), 0);
        assert_eq!(clamp(2, 1, 3), 2);
        assert_eq!(clamp(1, 1, 3), 2);
        assert_eq!(clamp(1, -1, 3), 0);
        assert_eq!(clamp(0, 5, 0), 0); // empty column
        assert_eq!(clamp(5, 0, 3), 2); // stale index re-clamped
    }

    #[test]
    fn move_column_is_bounded() {
        assert_eq!(move_column(0, -1), 0);
        assert_eq!(move_column(0, 1), 1);
        assert_eq!(move_column(2, 1), 2);
        assert_eq!(move_column(1, -1), 0);
    }

    #[test]
    fn default_label_follows_spawn_convention() {
        assert_eq!(default_label(true, "grove", "/home/u/grove"), "grove");
        assert_eq!(
            default_label(false, "grove", "/home/u/grove/.wt/fix-scroll"),
            "fix-scroll"
        );
    }

    #[test]
    fn breadcrumb_joins_with_chevrons() {
        assert_eq!(breadcrumb("grove", "main", "claude"), "grove › main › claude");
    }

    #[test]
    fn nav_within_column_moves_focused_axis() {
        // project column: moving resets wt to 0
        assert_eq!(nav_within_column(0, 0, 3, 1, 1, 3, 5, 4), (1, 0, 1));
        // worktree column: only wt changes
        assert_eq!(nav_within_column(1, 1, 2, 1, 1, 3, 5, 4), (1, 3, 1));
        assert_eq!(nav_within_column(1, 1, 4, 1, 1, 3, 5, 4), (1, 4, 1)); // clamp top
        // agent column: only agent changes
        assert_eq!(nav_within_column(2, 1, 2, 0, -1, 3, 5, 4), (1, 2, 0)); // clamp bottom
        assert_eq!(nav_within_column(2, 1, 2, 1, 1, 3, 5, 4), (1, 2, 2));
    }

    #[test]
    fn insert_into_tile_order_shifts_then_appends() {
        // new session inserted mid-vector at index 1: sessions 1,2 -> 2,3
        let mut order = vec![0, 1, 2];
        insert_into_tile_order(&mut order, 1);
        assert_eq!(order, vec![0, 2, 3, 1]);
        // inserted at the end: no shift, just append
        let mut order2 = vec![0, 1];
        insert_into_tile_order(&mut order2, 2);
        assert_eq!(order2, vec![0, 1, 2]);
        // empty order
        let mut empty: Vec<usize> = vec![];
        insert_into_tile_order(&mut empty, 0);
        assert_eq!(empty, vec![0]);
    }

    #[test]
    fn swap_tiles_swaps_positions() {
        // Dragging index 0 onto index 3: only those two positions trade
        // places, everything else is unchanged.
        let mut order = vec![0, 1, 2, 3, 4];
        swap_tiles(&mut order, 0, 3);
        assert_eq!(order, vec![3, 1, 2, 0, 4]);
        // Same slot and out-of-bounds are no-ops.
        let mut same = vec![1, 2];
        swap_tiles(&mut same, 1, 1);
        assert_eq!(same, vec![1, 2]);
        let mut oob = vec![1];
        swap_tiles(&mut oob, 0, 5);
        assert_eq!(oob, vec![1]);
    }

    #[test]
    fn session_grid_key_combines_project_and_wt_path() {
        assert_eq!(session_grid_key("proj", "/wt/a"), "proj::/wt/a");
    }

    #[test]
    fn reconcile_tile_order_uses_saved_order_first() {
        let live = vec!["p::a".to_string(), "p::b".to_string(), "p::c".to_string()];
        let saved = vec!["p::c".to_string(), "p::a".to_string()];
        // c (idx 2) and a (idx 0) come first per saved order; b (idx 1, no
        // saved entry) is appended last in its live-vector position.
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
        // Defensive: a key appearing twice in `saved` must not duplicate the
        // matching live index in the output.
        let live = vec!["p::a".to_string(), "p::b".to_string()];
        let saved = vec!["p::a".to_string(), "p::a".to_string()];
        assert_eq!(reconcile_tile_order(&live, &saved), vec![0, 1]);
    }
}
