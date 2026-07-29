//! Unit tests for the palette's pure logic: settings/theme pane selection
//! and row lists, scroll-offset math, row identity/ranking, and the
//! update-actions strip. Moved verbatim from the former single-file
//! `session_launcher.rs`'s `mod tests`.

use super::helpers::*;
use super::state::*;
use crate::gui::state::UpgradeMsg;
use crate::gui::state::UpgradeState;
use crate::gui::update::SettingRow;

// ── Settings sub-panes (phase 2) ─────────────────────────────────────

#[test]
fn backend_pane_selects_the_active_backend() {
    assert_eq!(backend_pane_selected_index(false), 0); // Native
    assert_eq!(backend_pane_selected_index(true), 1); // Tmux
}

#[test]
fn permissions_pane_selects_the_active_choice() {
    assert_eq!(permissions_pane_selected_index(false), 0); // Ask
    assert_eq!(permissions_pane_selected_index(true), 1); // Skip
}

#[test]
fn default_agent_pane_selects_the_current_default() {
    use grove_core::agent::Agent;
    assert_eq!(default_agent_pane_selected_index(None), 0);
    assert_eq!(default_agent_pane_selected_index(Some(Agent::Claude)), 0);
    assert_eq!(default_agent_pane_selected_index(Some(Agent::Codex)), 1);
    assert_eq!(default_agent_pane_selected_index(Some(Agent::OpenCode)), 2);
    assert_eq!(default_agent_pane_selected_index(Some(Agent::Terminal)), 3);
}

#[test]
fn theme_pane_selects_the_active_theme_within_its_kind() {
    use grove_core::theme::ThemeKind;
    // `tokyonight` is alphabetically first among the builtin dark
    // themes shipped today; a name with no match falls back to 0
    // rather than panicking.
    let dark = grove_core::theme::themes_of(ThemeKind::Dark);
    let idx = dark.iter().position(|t| t.name == "tokyonight").unwrap();
    assert_eq!(
        theme_pane_selected_index(ThemeKind::Dark, "tokyonight"),
        idx
    );
    assert_eq!(
        theme_pane_selected_index(ThemeKind::Dark, "no-such-theme"),
        0
    );
}

#[test]
fn theme_pane_rows_lists_only_the_requested_kind_fuzzy_filtered() {
    use grove_core::theme::ThemeKind;
    let all_dark = grove_core::theme::themes_of(ThemeKind::Dark);
    let all_light = grove_core::theme::themes_of(ThemeKind::Light);
    // Unfiltered: exactly the kind's own theme set, same order.
    assert_eq!(
        theme_pane_rows(ThemeKind::Dark, "")
            .iter()
            .map(|t| t.name.clone())
            .collect::<Vec<_>>(),
        all_dark.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
    );
    // Every row is actually of the requested kind — Light never leaks
    // into a Dark query or vice versa.
    assert!(theme_pane_rows(ThemeKind::Light, "")
        .iter()
        .all(|t| t.kind == ThemeKind::Light));
    assert_ne!(all_dark.len(), 0);
    assert_ne!(all_light.len(), 0);
    // Fuzzy-filtered: only names containing the query survive.
    let filtered = theme_pane_rows(ThemeKind::Dark, "tokyonight");
    assert!(!filtered.is_empty());
    assert!(filtered.iter().all(|t| t.name.contains("tokyonight")));
    // No match anywhere in the kind's list.
    assert!(theme_pane_rows(ThemeKind::Dark, "zzz-no-such-theme").is_empty());
}

#[test]
fn theme_pane_combined_rows_lists_customs_after_builtins() {
    use grove_core::theme::{Color, ThemeKind};
    let _lock = grove_core::theme::CUSTOM_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let name = "zz-test-custom-batch2";
    let custom = grove_core::theme::Theme {
        name: std::borrow::Cow::Owned(name.to_string()),
        kind: ThemeKind::Dark,
        bg: Color::Rgb(0, 0, 0),
        bg_highlight: Color::Rgb(0, 0, 0),
        fg: Color::Rgb(0, 0, 0),
        fg_dark: Color::Rgb(0, 0, 0),
        comment: Color::Rgb(0, 0, 0),
        blue: Color::Rgb(0, 0, 0),
        cyan: Color::Rgb(0, 0, 0),
        magenta: Color::Rgb(0, 0, 0),
        green: Color::Rgb(0, 0, 0),
        yellow: Color::Rgb(0, 0, 0),
        red: Color::Rgb(0, 0, 0),
    };
    grove_core::theme::add_custom(custom).expect("add_custom");

    let builtins_len = theme_pane_rows(ThemeKind::Dark, "").len();
    let combined = theme_pane_combined_rows(ThemeKind::Dark, "");
    // Every builtin row comes first, in the same order, then the custom
    // row(s) — never interleaved.
    assert_eq!(combined.len(), builtins_len + 1);
    assert_eq!(combined.last().unwrap().name, name);
    assert!(!theme_pane_row_is_custom(ThemeKind::Dark, "", 0));
    assert!(theme_pane_row_is_custom(ThemeKind::Dark, "", builtins_len));
    assert_eq!(
        theme_pane_selected_index(ThemeKind::Dark, name),
        builtins_len
    );

    grove_core::theme::delete_custom(name);
}

#[test]
fn app_scope_rows_never_get_a_use_default_row_project_scope_always_does_when_query_empty() {
    use grove_core::theme::ThemeKind;
    // Pins the scope difference the unified `theme_pane_select`/`_move`/
    // `_set_kind` (`theme_panes.rs`) rely on without ever spelling out
    // twice: App scope's row list (`theme_pane_combined_rows`) is exactly
    // the builtin+custom themes, with no leading "use default" placeholder
    // — app scope only ever has a concrete theme or "follow system", never
    // a default to fall back to. Project scope's list
    // (`project_theme_pane_rows`) fronts that exact same list with exactly
    // one extra row (`None`, "Use app theme") whenever the query is empty.
    // A future edit that accidentally adds the row to App or drops it from
    // Project breaks this count relationship, not just a screenshot.
    let app_rows = theme_pane_combined_rows(ThemeKind::Dark, "").len();
    let project_rows = project_theme_pane_rows(ThemeKind::Dark, "").len();
    assert_eq!(project_rows, app_rows + 1);
}

#[test]
fn project_theme_next_kind_toggles_dark_light_only_no_system() {
    use grove_core::theme::ThemeKind;
    // Contrast with `theme_pane_tab_cycles_dark_light_system` below (App
    // scope, three-way Dark → Light → System → Dark): Project scope's Tab
    // cycle only ever alternates Dark/Light.
    assert_eq!(project_theme_next_kind(ThemeKind::Dark), ThemeKind::Light);
    assert_eq!(project_theme_next_kind(ThemeKind::Light), ThemeKind::Dark);
}

#[test]
fn theme_reload_fallback_none_when_active_theme_still_resolves() {
    use grove_core::theme::ThemeKind;
    let name = grove_core::theme::BUILTINS[0].name.to_string();
    assert_eq!(theme_reload_fallback(&name, ThemeKind::Dark), None);
}

#[test]
fn theme_reload_fallback_falls_back_to_mode_default_when_active_theme_vanished() {
    use grove_core::theme::ThemeKind;
    assert_eq!(
        theme_reload_fallback("a-name-that-was-deleted", ThemeKind::Dark),
        Some(crate::app::DEFAULT_DARK_THEME)
    );
    assert_eq!(
        theme_reload_fallback("a-name-that-was-deleted", ThemeKind::Light),
        Some(crate::app::DEFAULT_LIGHT_THEME)
    );
}

#[test]
fn project_theme_pane_rows_has_use_default_row_only_when_query_is_empty() {
    use grove_core::theme::ThemeKind;
    // Reads `CUSTOM` (via `theme_pane_combined_rows`) — hold the same
    // lock the tests that mutate it use, so a concurrently-running test
    // can't leave a transient custom theme in the list this test doesn't
    // expect.
    let _lock = grove_core::theme::CUSTOM_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Empty query: "Use app theme" (None) leads, followed by every dark
    // theme in `theme_pane_combined_rows` order (builtins then customs).
    let rows = project_theme_pane_rows(ThemeKind::Dark, "");
    assert!(rows[0].is_none());
    assert_eq!(
        rows[1..]
            .iter()
            .map(|t| t.clone().unwrap().name)
            .collect::<Vec<_>>(),
        theme_pane_combined_rows(ThemeKind::Dark, "")
            .iter()
            .map(|t| t.name.clone())
            .collect::<Vec<_>>()
    );
    // Whitespace-only query counts as empty too.
    assert!(project_theme_pane_rows(ThemeKind::Dark, "   ")[0].is_none());
    // Any real query drops the "Use app theme" row — only fuzzy matches
    // remain.
    let filtered = project_theme_pane_rows(ThemeKind::Dark, "tokyonight");
    assert!(!filtered.is_empty());
    assert!(filtered.iter().all(std::option::Option::is_some));
    assert!(filtered
        .iter()
        .all(|t| t.clone().unwrap().name.contains("tokyonight")));
    // No match anywhere still yields an empty list, not a dangling
    // default row.
    assert!(project_theme_pane_rows(ThemeKind::Dark, "zzz-no-such-theme").is_empty());
}

#[test]
fn project_theme_pane_rows_kind_switch_yields_different_lists() {
    use grove_core::theme::ThemeKind;
    let dark = project_theme_pane_rows(ThemeKind::Dark, "");
    let light = project_theme_pane_rows(ThemeKind::Light, "");
    assert!(dark
        .iter()
        .skip(1)
        .all(|t| t.clone().unwrap().kind == ThemeKind::Dark));
    assert!(light
        .iter()
        .skip(1)
        .all(|t| t.clone().unwrap().kind == ThemeKind::Light));
    assert_ne!(dark.len(), light.len());
}

#[test]
fn update_actions_hide_update_now_for_unknown_installs() {
    // Known install method: all three actions, "Update now" first.
    assert_eq!(
        update_available_actions(false),
        vec![
            UpdateAction::UpdateNow,
            UpdateAction::SkipVersion,
            UpdateAction::CopyUrl
        ]
    );
    // Unknown install (notify-only): "Update now" is hidden, same guard
    // `settings_modal` applies — indices shift down with it.
    assert_eq!(
        update_available_actions(true),
        vec![UpdateAction::SkipVersion, UpdateAction::CopyUrl]
    );
}

#[test]
fn update_action_to_msg_is_total_and_matches_the_expected_top_level_msg() {
    // `settings_modal` (src/gui/view.rs) renders full-size buttons from
    // `update_available_actions` and dispatches each one through
    // `UpdateAction::to_msg`, instead of hardcoding `Msg::Upgrade(UpgradeMsg::StartUpdate)` /
    // `Msg::Upgrade(UpgradeMsg::SkipVersion)` / `Msg::Upgrade(UpgradeMsg::CopyReleaseUrl)` directly. This pins down
    // that every action still resolves to the same top-level `Msg` the
    // Settings modal emitted before the two surfaces shared one source of
    // truth — and, since the match in `to_msg` is exhaustive with no
    // wildcard arm, a future `UpdateAction` variant fails to compile here
    // rather than silently falling through.
    for action in update_available_actions(false) {
        let msg = action.to_msg();
        let matches_expected = match action {
            UpdateAction::UpdateNow => matches!(
                msg,
                crate::gui::state::Msg::Upgrade(UpgradeMsg::StartUpdate)
            ),
            UpdateAction::SkipVersion => matches!(
                msg,
                crate::gui::state::Msg::Upgrade(UpgradeMsg::SkipVersion)
            ),
            UpdateAction::CopyUrl => matches!(
                msg,
                crate::gui::state::Msg::Upgrade(UpgradeMsg::CopyReleaseUrl)
            ),
        };
        assert!(matches_expected, "unexpected Msg for {action:?}: {msg:?}");
    }
}

#[test]
fn check_updates_activation_opens_strip_only_when_update_available() {
    let release = grove_core::upgrade::Release {
        version: semver::Version::new(0, 9, 5),
        tag: "v0.9.5".into(),
        html_url: String::new(),
        body: String::new(),
        dmg_url: None,
        dmg_sha256_url: None,
        target_commitish: String::new(),
    };
    // Only a known-available release expands the strip…
    assert!(check_updates_opens_strip(&UpgradeState::Available(release)));
    // …every other state falls through to firing a fresh check.
    assert!(!check_updates_opens_strip(&UpgradeState::Idle));
    assert!(!check_updates_opens_strip(&UpgradeState::Checking));
    assert!(!check_updates_opens_strip(&UpgradeState::UpToDate));
    assert!(!check_updates_opens_strip(&UpgradeState::Error(
        "offline".into()
    )));
}

#[test]
fn launcher_theme_scroll_offset_centers_and_clamps() {
    // Everything fits (7 rows ≤ 280px cap): no scrolling, ever.
    assert_eq!(launcher_theme_scroll_offset(7, 0), 0.0);
    assert_eq!(launcher_theme_scroll_offset(7, 6), 0.0);
    // 30 rows at the true 38px pitch (36px row + 2px column spacing) =
    // 30·38 − 2 = 1138px content (no trailing gap after the last row)
    // against a 280px viewport.
    // Top rows clamp to 0 rather than centering above the list…
    assert_eq!(launcher_theme_scroll_offset(30, 0), 0.0);
    // …the last row clamps to the bottom (1138 − 280 = 858)…
    assert_eq!(launcher_theme_scroll_offset(30, 29), 858.0);
    // …and a middle row centers: y = 15·38 − (280 − 36)/2 = 448.
    assert_eq!(launcher_theme_scroll_offset(30, 15), 448.0);
    // Empty list degenerates to 0, not NaN/negative.
    assert_eq!(launcher_theme_scroll_offset(0, 0), 0.0);
}

/// Regression for the Theme sub-pane's `scroll_launcher_theme_to_
/// selection`: `launcher_theme_scroll_offset` above pitches the CUSTOM
/// section header/hint/"Manage themes…" rows at the uniform 38px row
/// pitch, drifting centering once the list scrolls anywhere near that
/// section. Values hand-derived from the exact child sequence `view.rs`
/// pushes onto `Column::new().spacing(2)` (view.rs:3856-3890): builtin
/// rows (38px pitch), `section_header("CUSTOM", 12.0, 6.0)` (31px),
/// either the "no custom themes yet" hint (29px) or the custom rows
/// (38px pitch), then the "Manage themes…" row (32px, never selectable).
#[test]
fn theme_pane_scroll_offset_accounts_for_custom_header_hint_and_manage_row() {
    // 7 builtins, no custom themes yet (hint row shown in their place):
    // a builtin row well clear of both edges still centers cleanly —
    // the header/hint/manage rows below only affect how far it can
    // clamp, not this row's own y.
    assert_eq!(theme_pane_scroll_offset(7, 0, 4), 30.0);
    // The very first row clamps to the top…
    assert_eq!(theme_pane_scroll_offset(7, 0, 0), 0.0);
    // …and the last builtin row clamps to the bottom: the header/hint/
    // manage rows below it push max_y past where it'd naturally center
    // (106), which the old uniform-pitch version undercounted.
    assert_eq!(theme_pane_scroll_offset(7, 0, 6), 82.0);
    // 5 builtins + 5 custom themes: selecting the first custom row
    // (row 5) lands just past the CUSTOM header, still shy of the
    // bottom clamp — exactly the geometry a uniform 38px pitch gets
    // wrong (the header renders at 31px, not 38px).
    assert_eq!(theme_pane_scroll_offset(5, 5, 5), 101.0);
    // No themes match the filter at all: header + hint + manage row
    // still total well under the 280px cap, so nothing scrolls.
    assert_eq!(theme_pane_scroll_offset(0, 0, 0), 0.0);
}

/// Regression for the reported bug: ↑/↓ in the theme editor didn't
/// scroll at all (`theme_manager_editor_row_select` never called any
/// scroll function). Values hand-derived from the exact `view.rs` render
/// geometry (2 Surfaces + 3 Text + 6 Accents rows, each group preceded
/// by a 10/13/4 header, followed by the derived strip's own header +
/// chip row) — see `theme_editor_scroll_offset`'s doc comment for the
/// constants.
#[test]
fn theme_editor_scroll_offset_centers_and_clamps() {
    // Row 0 (first "Surfaces" row): would center above the list, so it
    // clamps to the top instead.
    assert_eq!(theme_editor_scroll_offset(0), 0.0);
    // Row 2 (first "Text" row, right after a group-header transition):
    // sel_y = 124 (17 + 2 + 36 + 2 + 36 + 27 + 2), centered at
    // 124 − (280 − 36)/2 = 2.
    assert_eq!(theme_editor_scroll_offset(2), 2.0);
    // Row 5 (first "Accents" row): centers normally, no clamp.
    assert_eq!(theme_editor_scroll_offset(5), 145.0);
    // Row 10 ("red", the last color row): the trailing derived-strip
    // header + chip row push total content to 545px, so the bottom
    // clamp (545 − 280 = 265) kicks in rather than over-scrolling past
    // the real content.
    assert_eq!(theme_editor_scroll_offset(10), 265.0);
}

#[test]
fn theme_pane_tab_cycles_dark_light_system() {
    use grove_core::theme::ThemeKind;
    // Dark → Light → System → Dark, matching the segment order (System
    // is active whenever follow_system is set, whatever the list kind).
    assert_eq!(next_theme_mode(ThemeKind::Dark, false), ThemeMode::Light);
    assert_eq!(next_theme_mode(ThemeKind::Light, false), ThemeMode::System);
    assert_eq!(next_theme_mode(ThemeKind::Dark, true), ThemeMode::Dark);
    assert_eq!(next_theme_mode(ThemeKind::Light, true), ThemeMode::Dark);
}

#[test]
fn settings_root_scroll_offset_accounts_for_headers_and_clamps() {
    // The full unfiltered list: 9 rows across 4 sections. Element walk
    // (2px column spacing throughout): first header 19px (0+13+6), later
    // headers 31px (12+13+6), rows 44px — content = 4 headers (112) +
    // 9 rows (396) + 12 gaps (24) = 532px against the 364px viewport
    // (the 380px max_height minus the same container's 2·8px padding),
    // so max scroll = 168.
    let rows = SettingRow::ALL;
    // Row 0 sits right under the first header: centering clamps to 0.
    assert_eq!(settings_root_scroll_offset(&rows, 0), 0.0);
    // The last row (CheckUpdates, y = 488) clamps to the bottom:
    // content_h − viewport_h = 532 − 364…
    assert_eq!(settings_root_scroll_offset(&rows, 8), 168.0);
    // …which leaves all 44px of it inside the viewport: its bottom edge
    // (y + row) sits exactly at the viewport's bottom (offset + 364).
    let max_offset = settings_root_scroll_offset(&rows, 8);
    assert!(488.0 + 44.0 <= max_offset + 364.0);
    // A row past a mid-list header (Backend, first of AGENTS/TERMINAL):
    // y = 192 → centered 192 − (364 − 44)/2 = 32. Uniform-height math
    // (i·46) would put y at 138 and clamp the centering to 0 — the
    // headers are what make the difference.
    assert_eq!(settings_root_scroll_offset(&rows, 3), 32.0);
    assert!(
        settings_root_scroll_offset(&rows, 3) > (3.0 * 46.0 - (364.0 - 44.0) / 2.0_f32).max(0.0)
    );
    // Empty (fully filtered-out) list degenerates to 0.
    assert_eq!(settings_root_scroll_offset(&[], 0), 0.0);
}

#[test]
fn palette_scroll_offset_root_mode_accounts_for_headers_and_clamps() {
    use grove_core::agent::Agent;
    let combo = |proj: usize| PaletteRow::Combo {
        proj,
        wt_path: format!("/wt/{proj}"),
        agent: Agent::Claude,
    };
    // Root-mode shape: one RECENT row, two per-project WORKTREES groups
    // of three rows each, then one ACTIONS row — same header sizes as
    // `settings_root_scroll_offset`'s test (19px first header, 31px
    // later ones, 44px rows, 2px gaps throughout), so content sums to
    // the same 486px against the same 364px viewport (max scroll 122).
    let rows = vec![
        PaletteRow::Recent {
            proj: 0,
            wt_path: "/wt/0".into(),
            agent: Agent::Claude,
        },
        combo(0),
        combo(0),
        combo(0),
        combo(1),
        combo(1),
        combo(1),
        PaletteRow::NewSession,
    ];
    // Row 0 sits right under the RECENT header: centering clamps to 0.
    assert_eq!(palette_scroll_offset(&rows, 0, true), 0.0);
    // The last row (NewSession, y = 442) clamps to the bottom.
    assert_eq!(palette_scroll_offset(&rows, 7, true), 122.0);
    // A row past the second WORKTREES header (proj 0's 3rd combo row,
    // y = 192): centered 192 − (364 − 44)/2 = 32 — the header is what
    // shifts it off the naive index·46 estimate.
    assert_eq!(palette_scroll_offset(&rows, 3, true), 32.0);
    // Empty list degenerates to 0.
    assert_eq!(palette_scroll_offset(&[], 0, true), 0.0);
}

#[test]
fn palette_scroll_offset_typed_list_fits_and_clamps() {
    use grove_core::agent::Agent;
    let combo = |proj: usize| PaletteRow::Combo {
        proj,
        wt_path: format!("/wt/{proj}"),
        agent: Agent::Claude,
    };
    // A short typed list (well under the 364px viewport): no scrolling,
    // ever, whichever row is selected.
    let short = vec![
        PaletteRow::Setting(SettingRow::Theme),
        PaletteRow::Setting(SettingRow::Telemetry),
        combo(0),
    ];
    assert_eq!(palette_scroll_offset(&short, 0, false), 0.0);
    assert_eq!(palette_scroll_offset(&short, 2, false), 0.0);
    // A long typed list of 15 same-project session rows: >10 rows alone
    // triggers `grouped_by_project`, adding a per-project sub-header
    // under the SESSIONS header. Selection 0 still clamps to 0 (row 0
    // sits right under both headers)…
    let long: Vec<PaletteRow> = (0..15).map(|_| combo(0)).collect();
    assert_eq!(palette_scroll_offset(&long, 0, false), 0.0);
    // …and the last row clamps to the bottom of the (728px) content
    // against the 364px viewport: max scroll = 364.
    assert_eq!(palette_scroll_offset(&long, 14, false), 364.0);
}

#[test]
fn nth_session_row_skips_settings_and_action_rows() {
    use grove_core::agent::Agent;
    let combo = |proj: usize| PaletteRow::Combo {
        proj,
        wt_path: format!("/wt/{proj}"),
        agent: Agent::Claude,
    };
    // Typed-mode shape: settings sort above the session rows (B2).
    let rows = vec![
        PaletteRow::Setting(SettingRow::Theme),
        PaletteRow::Setting(SettingRow::Telemetry),
        combo(0),
        combo(1),
        PaletteRow::SwitchToSession,
    ];
    // ⌘1/⌘2 land on the sessions, not the settings above them…
    assert_eq!(nth_session_row(&rows, 1), Some(2));
    assert_eq!(nth_session_row(&rows, 2), Some(3));
    // …and digits past the session count are a no-op, even though other
    // row kinds are still below.
    assert_eq!(nth_session_row(&rows, 3), None);
    assert_eq!(nth_session_row(&rows, 0), None);
    // Recent rows count the same as Combo (root-mode list shape).
    let root = vec![
        PaletteRow::Recent {
            proj: 0,
            wt_path: "/wt/0".into(),
            agent: Agent::Codex,
        },
        PaletteRow::NewSession,
    ];
    assert_eq!(nth_session_row(&root, 1), Some(0));
    assert_eq!(nth_session_row(&root, 2), None);
}

#[test]
fn resolve_row_by_identity_follows_the_row_after_a_synthetic_reorder() {
    use grove_core::agent::Agent;
    let combo = |proj: usize| PaletteRow::Combo {
        proj,
        wt_path: format!("/wt/{proj}"),
        agent: Agent::Claude,
    };
    // The user highlighted combo(1) (index 1) and its identity was
    // captured then — simulating `set_palette_selected`.
    let rendered = [combo(0), combo(1), combo(2)];
    let identity = Some(row_identity(&rendered[1]));
    // Before Enter lands, something reorders the list out from under
    // the stale index (e.g. a re-rank/re-group pass, or an async
    // recents update) — combo(1) is now at index 0, not 1.
    let mutated = vec![combo(1), combo(0), combo(2)];
    // A naive `mutated.get(1)` would now resolve to combo(0) — the
    // wrong row. Identity resolution finds combo(1) wherever it moved.
    assert_eq!(resolve_row_by_identity(&mutated, &identity, 1), Some(0));
}

#[test]
fn resolve_row_by_identity_refuses_to_substitute_a_different_row() {
    use grove_core::agent::Agent;
    let combo = |proj: usize| PaletteRow::Combo {
        proj,
        wt_path: format!("/wt/{proj}"),
        agent: Agent::Claude,
    };
    let identity = Some(row_identity(&combo(1)));
    // combo(1) is simply gone from the rebuilt list (worktree removed,
    // filtered out, …): must not silently activate whatever now sits at
    // the stale index instead.
    let mutated = vec![combo(0), combo(2)];
    assert_eq!(resolve_row_by_identity(&mutated, &identity, 1), None);
}

#[test]
fn resolve_row_by_identity_falls_back_to_index_with_no_identity() {
    use grove_core::agent::Agent;
    let combo = |proj: usize| PaletteRow::Combo {
        proj,
        wt_path: format!("/wt/{proj}"),
        agent: Agent::Claude,
    };
    let rows = vec![combo(0), combo(1)];
    // No identity captured yet: falls back to the raw index (clamped to
    // the list bounds).
    assert_eq!(resolve_row_by_identity(&rows, &None, 1), Some(1));
    assert_eq!(resolve_row_by_identity(&rows, &None, 5), None);
}

#[test]
fn row_identity_distinguishes_action_and_setting_rows() {
    // Action/singleton rows carry no per-row data, but must still
    // compare unequal to each other so a stale identity can't match
    // the wrong action.
    assert!(row_identity(&PaletteRow::NewSession) != row_identity(&PaletteRow::AddProject));
    assert_eq!(
        row_identity(&PaletteRow::Setting(SettingRow::Theme)),
        row_identity(&PaletteRow::Setting(SettingRow::Theme))
    );
    assert!(
        row_identity(&PaletteRow::Setting(SettingRow::Theme))
            != row_identity(&PaletteRow::Setting(SettingRow::Telemetry))
    );
}

#[test]
fn rank_and_group_combos_orders_by_score_then_recency() {
    use grove_core::agent::Agent;
    let combo = |proj: usize, path: &str| PaletteRow::Combo {
        proj,
        wt_path: path.into(),
        agent: Agent::Claude,
    };
    // Two combos tie on score (10): the one at recency index 0 (earlier
    // in `recent_launches`) must sort first, even though it was pushed
    // second.
    let scored = vec![
        (10, usize::MAX, combo(0, "/b")),
        (10, 0, combo(0, "/a")),
        (5, usize::MAX, combo(0, "/c")),
    ];
    let ranked = rank_and_group_combos(scored);
    let paths: Vec<&str> = ranked
        .iter()
        .map(|r| match r {
            PaletteRow::Combo { wt_path, .. } => wt_path.as_str(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(paths, vec!["/a", "/b", "/c"]);
}

#[test]
fn rank_and_group_combos_ties_with_no_recency_keep_store_order() {
    use grove_core::agent::Agent;
    let combo = |path: &str| PaletteRow::Combo {
        proj: 0,
        wt_path: path.into(),
        agent: Agent::Claude,
    };
    // Both tied at usize::MAX (absent from recents): stable sort keeps
    // their original (store) relative order.
    let scored = vec![
        (10, usize::MAX, combo("/first")),
        (10, usize::MAX, combo("/second")),
    ];
    let ranked = rank_and_group_combos(scored);
    let paths: Vec<&str> = ranked
        .iter()
        .map(|r| match r {
            PaletteRow::Combo { wt_path, .. } => wt_path.as_str(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(paths, vec!["/first", "/second"]);
}

#[test]
fn rank_and_group_combos_groups_by_project_above_threshold() {
    use grove_core::agent::Agent;
    let combo = |proj: usize, path: &str, score: u32| {
        (
            score,
            usize::MAX,
            PaletteRow::Combo {
                proj,
                wt_path: path.into(),
                agent: Agent::Claude,
            },
        )
    };
    // 3 distinct projects (over the 2-project threshold): re-clustered
    // by project, each project's run led by its own best-scored row —
    // project 1's best (20) beats project 0's best (15), so project 1's
    // whole run comes first.
    let scored = vec![
        combo(0, "/p0/a", 15),
        combo(1, "/p1/a", 20),
        combo(2, "/p2/a", 5),
        combo(0, "/p0/b", 10),
        combo(1, "/p1/b", 8),
    ];
    let ranked = rank_and_group_combos(scored);
    let projects: Vec<usize> = ranked
        .iter()
        .map(|r| match r {
            PaletteRow::Combo { proj, .. } => *proj,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(projects, vec![1, 1, 0, 0, 2]);
}

#[test]
fn rank_and_group_combos_stays_flat_at_or_below_threshold() {
    use grove_core::agent::Agent;
    let combo = |proj: usize, path: &str, score: u32| {
        (
            score,
            usize::MAX,
            PaletteRow::Combo {
                proj,
                wt_path: path.into(),
                agent: Agent::Claude,
            },
        )
    };
    // Exactly 2 distinct projects, ≤10 rows: flat rank order, untouched.
    let scored = vec![combo(0, "/p0/a", 5), combo(1, "/p1/a", 20)];
    let ranked = rank_and_group_combos(scored);
    let projects: Vec<usize> = ranked
        .iter()
        .map(|r| match r {
            PaletteRow::Combo { proj, .. } => *proj,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(projects, vec![1, 0]);
}

#[test]
fn root_project_order_puts_active_first_then_clamps_stale_index() {
    assert_eq!(root_project_order(3, 1), vec![1, 0, 2]);
    assert_eq!(root_project_order(3, 0), vec![0, 1, 2]);
    // Stale/out-of-range active index clamps rather than panicking.
    assert_eq!(root_project_order(3, 99), vec![2, 0, 1]);
    assert_eq!(root_project_order(0, 0), Vec::<usize>::new());
}

#[test]
fn reselect_setting_keeps_identity_else_clamps() {
    // The toggled row survived the refilter (moved up): follow it.
    let rows = [SettingRow::Telemetry, SettingRow::CheckUpdates];
    assert_eq!(reselect_setting(&rows, SettingRow::Telemetry, 5), 0);
    // The toggled row dropped out (value no longer matches the query):
    // the stale index clamps into the shrunk list.
    assert_eq!(reselect_setting(&rows, SettingRow::ProjectThemes, 5), 1);
    assert_eq!(reselect_setting(&rows, SettingRow::ProjectThemes, 0), 0);
    // Everything filtered out: clamp degenerates to 0.
    assert_eq!(reselect_setting(&[], SettingRow::Telemetry, 3), 0);
}

// ── Row-actions strip agent bar (←→ on "Launch session…") ───────────

#[test]
fn cycle_agent_wraps_in_both_directions() {
    assert_eq!(cycle_agent(0, 1, 3), 1);
    assert_eq!(cycle_agent(1, 1, 3), 2);
    // Off the end wraps to the start, and off the start to the end.
    assert_eq!(cycle_agent(2, 1, 3), 0);
    assert_eq!(cycle_agent(0, -1, 3), 2);
    assert_eq!(cycle_agent(1, -1, 3), 0);
    // Single agent: every step is a no-op rather than an out-of-range index.
    assert_eq!(cycle_agent(0, 1, 1), 0);
    assert_eq!(cycle_agent(0, -1, 1), 0);
    // No agents at all: never divides by zero.
    assert_eq!(cycle_agent(0, 1, 0), 0);
    // A stale index (agents list shrank under an open palette) re-wraps
    // into range rather than escaping it.
    assert_eq!(cycle_agent(9, 1, 3), 1);
}

#[test]
fn strip_agent_bar_opens_on_the_rows_own_agent() {
    use grove_core::agent::Agent;
    let available = [
        Agent::Claude,
        Agent::Codex,
        Agent::OpenCode,
        Agent::Terminal,
    ];
    // Tab on a row opens the strip with that row's agent already ringed,
    // so ⏎ straight after is still the plain remembered-agent launch.
    assert_eq!(agent_sel_for(&available, Agent::Claude), 0);
    assert_eq!(agent_sel_for(&available, Agent::Codex), 1);
    assert_eq!(agent_sel_for(&available, Agent::Terminal), 3);
    // The row's remembered agent is no longer installed (recents outlive
    // an uninstall): fall back to the first available one, never a stale
    // index into a list that doesn't contain it.
    assert_eq!(agent_sel_for(&[Agent::Codex], Agent::Claude), 0);
    assert_eq!(agent_sel_for(&[], Agent::Claude), 0);
}

// ── Switch drill-in: home terminals as a second group ─────────────────

#[test]
fn switch_terminal_rows_filters_by_label_and_the_home_terminal_subtitle() {
    let labels = vec![
        "terminal 1".to_string(),
        "terminal 2".to_string(),
        "terminal 3".to_string(),
    ];
    // Empty query lists every terminal, in order.
    assert_eq!(switch_terminal_rows(&labels, ""), vec![0, 1, 2]);
    // The label's own number narrows to one row.
    assert_eq!(switch_terminal_rows(&labels, "terminal 2"), vec![1]);
    // The rendered subtitle is searchable too, so "home" finds them all.
    assert_eq!(switch_terminal_rows(&labels, "home"), vec![0, 1, 2]);
    // No match anywhere drops the whole group (and with it its header).
    assert!(switch_terminal_rows(&labels, "zzz-nope").is_empty());
}

#[test]
fn merge_switch_rows_lists_sessions_before_terminals() {
    assert_eq!(
        merge_switch_rows(&[3, 0], &[1, 2]),
        vec![
            SwitchRow::Session(3),
            SwitchRow::Session(0),
            SwitchRow::Terminal(1),
            SwitchRow::Terminal(2),
        ]
    );
    // Either group filtering to empty just leaves the other one.
    assert_eq!(merge_switch_rows(&[], &[0]), vec![SwitchRow::Terminal(0)]);
    assert_eq!(merge_switch_rows(&[7], &[]), vec![SwitchRow::Session(7)]);
    assert!(merge_switch_rows(&[], &[]).is_empty());
}
