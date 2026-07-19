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

/// Default session label following Grove's spawn convention: the project name
/// for the main checkout, otherwise the worktree path basename.
pub fn default_label(is_main: bool, project_name: &str, wt_path: &str) -> String {
    if is_main {
        project_name.to_string()
    } else {
        crate::app::path_basename(wt_path)
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

/// Push a launch to the front of `recent`, deduping by (project, wt_path,
/// agent) — i.e. re-launching the same target moves it to the front instead
/// of creating a duplicate entry — then truncate to 6.
pub fn push_recent_launch(
    recent: &mut Vec<crate::storage::RecentLaunch>,
    launch: crate::storage::RecentLaunch,
) {
    recent.retain(|r| {
        !(r.project == launch.project && r.wt_path == launch.wt_path && r.agent == launch.agent)
    });
    recent.insert(0, launch);
    recent.truncate(6);
}

/// Fuzzy filter: split `query` on whitespace, lowercase every term, require
/// each term to substring-match at least one of `project`/`worktree`/
/// `agent_label` (also lowercased). Empty query matches everything.
pub fn fuzzy_match(query: &str, project: &str, worktree: &str, agent_label: &str) -> bool {
    fuzzy_match_indices(query, project, worktree, agent_label).matched
}

/// Result of [`fuzzy_match_indices`]: whether the row matched, plus the
/// **character** index ranges (not byte offsets — see [`ci_find_range`])
/// within each field where a query term was found, for the typing-state
/// cyan-highlight render in `view.rs`. Each matched term contributes at most
/// one range per field (its first occurrence); a term that matches nowhere
/// makes `matched` false for the whole (AND-across-terms) query, same as the
/// original bool-only `fuzzy_match`.
#[derive(Debug, Default, PartialEq)]
pub struct FuzzyMatch {
    pub matched: bool,
    pub project: Vec<(usize, usize)>,
    pub worktree: Vec<(usize, usize)>,
    pub agent: Vec<(usize, usize)>,
}

/// Case-insensitive substring search returning the **char** index range
/// `(start, end)` (end-exclusive) of the first occurrence of `needle` in
/// `haystack`. Compares character-by-character via `char::to_lowercase`
/// rather than searching byte offsets of a separately-lowercased string, so
/// callers can slice the *original* string by `chars()` without worrying
/// about a lowercase transform changing UTF-8 byte lengths. Returns `None`
/// for an empty needle or no match.
fn ci_find_range(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let hay: Vec<char> = haystack.chars().collect();
    let need: Vec<char> = needle.chars().collect();
    if need.len() > hay.len() {
        return None;
    }
    for start in 0..=(hay.len() - need.len()) {
        let matches = need
            .iter()
            .enumerate()
            .all(|(i, nc)| hay[start + i].to_lowercase().eq(nc.to_lowercase()));
        if matches {
            return Some((start, start + need.len()));
        }
    }
    None
}

/// Same matching semantics as [`fuzzy_match`], but also reports the matched
/// char ranges per field for highlighting.
pub fn fuzzy_match_indices(
    query: &str,
    project: &str,
    worktree: &str,
    agent_label: &str,
) -> FuzzyMatch {
    let mut out = FuzzyMatch {
        matched: true,
        ..Default::default()
    };
    for term in query.split_whitespace() {
        let mut hit = false;
        if let Some(r) = ci_find_range(project, term) {
            out.project.push(r);
            hit = true;
        }
        if let Some(r) = ci_find_range(worktree, term) {
            out.worktree.push(r);
            hit = true;
        }
        if let Some(r) = ci_find_range(agent_label, term) {
            out.agent.push(r);
            hit = true;
        }
        if !hit {
            out.matched = false;
        }
    }
    out
}

/// Filter+order settings rows for the command palette's Settings features
/// (root-mode direct matches and the Settings drill-in's own list): each
/// candidate is `(id, label, value, section)`; `id` is returned, in
/// candidate order, for every row whose label/value/section 3-way
/// fuzzy-matches `input` (same match semantics as `fuzzy_match` — an empty
/// query matches everything). Generic over the id type so this stays free of
/// `crate::gui::update::SettingRow` (and any Iced dependency); callers own
/// the mapping from id back to that enum.
pub fn matching_settings<T: Copy>(input: &str, candidates: &[(T, &str, &str, &str)]) -> Vec<T> {
    candidates
        .iter()
        .filter(|(_, label, value, section)| fuzzy_match(input, label, value, section))
        .map(|&(id, ..)| id)
        .collect()
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
    fn default_label_follows_spawn_convention() {
        assert_eq!(default_label(true, "grove", "/home/u/grove"), "grove");
        assert_eq!(
            default_label(false, "grove", "/home/u/grove/.wt/fix-scroll"),
            "fix-scroll"
        );
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

    fn recent(
        project: &str,
        wt_path: &str,
        agent: crate::agent::Agent,
    ) -> crate::storage::RecentLaunch {
        crate::storage::RecentLaunch {
            project: project.to_string(),
            wt_path: wt_path.to_string(),
            agent,
        }
    }

    #[test]
    fn push_recent_launch_dedups_and_moves_to_front() {
        use crate::agent::Agent;
        let mut recents = vec![
            recent("a", "/a", Agent::Claude),
            recent("b", "/b", Agent::Codex),
        ];
        push_recent_launch(&mut recents, recent("a", "/a", Agent::Claude));
        assert_eq!(recents.len(), 2);
        assert_eq!(recents[0], recent("a", "/a", Agent::Claude));
        assert_eq!(recents[1], recent("b", "/b", Agent::Codex));
    }

    #[test]
    fn push_recent_launch_truncates_beyond_six() {
        use crate::agent::Agent;
        let mut recents: Vec<crate::storage::RecentLaunch> = (0..6)
            .map(|i| recent(&format!("p{i}"), &format!("/p{i}"), Agent::Claude))
            .collect();
        push_recent_launch(&mut recents, recent("new", "/new", Agent::Terminal));
        assert_eq!(recents.len(), 6);
        assert_eq!(recents[0], recent("new", "/new", Agent::Terminal));
        // The oldest entry (p5) fell off the end.
        assert!(!recents.iter().any(|r| r.project == "p5"));
    }

    #[test]
    fn fuzzy_match_is_multi_term_and_across_fields_case_insensitive() {
        assert!(fuzzy_match("grove fix", "Grove", "fix-scroll", "claude"));
        assert!(fuzzy_match("CLAUDE", "grove", "main", "claude"));
        assert!(fuzzy_match("", "anything", "anything", "anything"));
        assert!(!fuzzy_match("nomatch", "grove", "main", "claude"));
        // A term matching nothing fails the whole (AND-across-terms) query,
        // even when the other term matches.
        assert!(!fuzzy_match("grove nomatch", "grove", "main", "claude"));
    }

    #[test]
    fn fuzzy_match_indices_reports_matched_char_ranges() {
        let m = fuzzy_match_indices("gro", "grove", "main", "claude");
        assert!(m.matched);
        assert_eq!(m.project, vec![(0, 3)]);
        assert!(m.worktree.is_empty());
        assert!(m.agent.is_empty());

        // Multi-term query: each term contributes its own range, possibly in
        // different fields.
        let m2 = fuzzy_match_indices("grove main", "Grove", "main", "claude");
        assert!(m2.matched);
        assert_eq!(m2.project, vec![(0, 5)]);
        assert_eq!(m2.worktree, vec![(0, 4)]);

        // No match anywhere: not matched, and no ranges recorded.
        let m3 = fuzzy_match_indices("zzz", "grove", "main", "claude");
        assert!(!m3.matched);
        assert!(m3.project.is_empty() && m3.worktree.is_empty() && m3.agent.is_empty());
    }

    // `palette_rows` needs a full `Grove` (sessions, PTYs, store, …) to
    // construct — impractical to build in a unit test — so the settings
    // portion of its matching logic is exercised here directly against the
    // pure `matching_settings` helper it calls into, using the real
    // `SettingRow` enum (visible here: `pub(super)` in `gui::update` is
    // visible throughout `gui`, including this sibling module).
    #[test]
    fn matching_settings_filters_by_label_value_or_section_and_preserves_order() {
        use crate::gui::update::SettingRow;
        let candidates: Vec<(SettingRow, &str, &str, &str)> = vec![
            (SettingRow::Theme, "App theme", "tokyonight", "APPEARANCE"),
            (
                SettingRow::Telemetry,
                "Telemetry",
                "On",
                "AGENTS / TERMINAL",
            ),
            (
                SettingRow::CheckUpdates,
                "Check for updates",
                "v0.9.4 · Up to date",
                "UPDATES",
            ),
        ];

        // "telem" matches Telemetry's label — a stand-in for this repo's
        // `palette_rows` test (c): a typed input matching a setting's label
        // resolves to exactly that `SettingRow`, and nothing else.
        assert_eq!(
            matching_settings("telem", &candidates),
            vec![SettingRow::Telemetry]
        );
        // Matches by current *value*, not just label/section.
        assert_eq!(
            matching_settings("tokyonight", &candidates),
            vec![SettingRow::Theme]
        );
        // Matches by section keyword.
        assert_eq!(
            matching_settings("appearance", &candidates),
            vec![SettingRow::Theme]
        );
        // Empty query matches everything, in candidate order.
        assert_eq!(
            matching_settings("", &candidates),
            vec![
                SettingRow::Theme,
                SettingRow::Telemetry,
                SettingRow::CheckUpdates
            ]
        );
        // No match anywhere.
        assert!(matching_settings("zzz", &candidates).is_empty());
    }
}
