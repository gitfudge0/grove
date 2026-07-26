//! Pure free functions used across the palette: scroll-offset math, row
//! ranking/grouping, row-identity resolution (used to keep the selection
//! glued to "the same row" across a re-render), and small theme-pane row
//! builders. Also home to `UpdateAction` and its helpers, which the
//! Settings drill-in's update-actions strip (`view.rs`) and `update.rs`
//! both depend on.

use super::state::{PaletteRow, PaletteRowIdentity};
use crate::gui::state::UpgradeMsg;
use crate::gui::state::{Msg as GMsg, UpgradeState};
use crate::gui::update::SettingRow;
use grove_core::agent::Agent;

/// Pure selection-index math for entering each Settings sub-pane (see the
/// `Grove::enter_*_pane` methods) — kept free of `Grove`/`Modal` so it's
/// directly unit-testable without building a GUI.
pub(super) fn backend_pane_selected_index(tmux_on: bool) -> usize {
    if tmux_on {
        1
    } else {
        0
    }
}

pub(super) fn permissions_pane_selected_index(skip_on: bool) -> usize {
    if skip_on {
        1
    } else {
        0
    }
}

pub(super) fn default_agent_pane_selected_index(default: Option<Agent>) -> usize {
    default
        .and_then(|a| Agent::ALL.iter().position(|&x| x == a))
        .unwrap_or(0)
}

/// Selects the row (builtin or custom) matching `name` within `kind`'s
/// *unfiltered* combined list — builtins first, then customs — falling back
/// to row 0. Builtins and customs never share a name (custom names that
/// collide with a builtin are rejected at creation/load), so this never
/// double-matches.
pub(super) fn theme_pane_selected_index(kind: grove_core::theme::ThemeKind, name: &str) -> usize {
    let builtins = grove_core::theme::themes_of(kind);
    if let Some(i) = builtins.iter().position(|t| t.name == name) {
        return i;
    }
    grove_core::theme::custom_themes_of(kind)
        .iter()
        .position(|t| t.name == name)
        .map(|i| builtins.len() + i)
        .unwrap_or(0)
}

/// Builtin themes of `kind` fuzzy-filtered by `input`, in `theme::themes_of`'s
/// alphabetical order — the Theme sub-pane's Built-in section (`view.rs`) and
/// its keyboard/mouse selection (`Grove::theme_pane_select`/`theme_pane_move`)
/// share this so they never disagree on what row N is.
pub(super) fn theme_pane_rows(
    kind: grove_core::theme::ThemeKind,
    input: &str,
) -> Vec<grove_core::theme::Theme> {
    grove_core::theme::themes_of(kind)
        .into_iter()
        .filter(|t| crate::gui::launcher::fuzzy_match(input, &t.name, "", ""))
        .collect()
}

/// Custom themes of `kind` fuzzy-filtered by `input`, same alphabetical/
/// fuzzy-match contract as `theme_pane_rows` — the Theme sub-pane's Custom
/// section, rendered *after* (never interleaved with) `theme_pane_rows`.
pub(super) fn theme_pane_custom_rows(
    kind: grove_core::theme::ThemeKind,
    input: &str,
) -> Vec<grove_core::theme::Theme> {
    grove_core::theme::custom_themes_of(kind)
        .into_iter()
        .filter(|t| crate::gui::launcher::fuzzy_match(input, &t.name, "", ""))
        .collect()
}

/// The Theme sub-pane's full selectable list: every builtin row followed by
/// every custom row (never interleaved) — selection/keyboard nav flows
/// continuously across the boundary, so callers that only need "the theme
/// at index N" or "how many rows total" use this rather than combining
/// `theme_pane_rows`/`theme_pane_custom_rows` themselves.
pub(super) fn theme_pane_combined_rows(
    kind: grove_core::theme::ThemeKind,
    input: &str,
) -> Vec<grove_core::theme::Theme> {
    let mut rows = theme_pane_rows(kind, input);
    rows.extend(theme_pane_custom_rows(kind, input));
    rows
}

/// Whether row `idx` of the Theme sub-pane's combined list (see
/// `theme_pane_combined_rows`) falls in the Custom section — the only rows
/// where rename/delete/edit are offered.
/// `Grove::reload_themes`'s "does the active theme still resolve?" decision,
/// pulled out as a pure function so it's testable without a full `Grove`:
/// `None` when `active_name` still resolves to something (builtin or
/// custom) — nothing to do; `Some(fallback)` — the mode default for `kind`
/// — when it doesn't, so the caller can silently reassign it (mock E3: this
/// is routine config drift, not an error).
pub(super) fn theme_reload_fallback(
    active_name: &str,
    kind: grove_core::theme::ThemeKind,
) -> Option<&'static str> {
    if grove_core::theme::by_name(active_name).is_some() {
        return None;
    }
    Some(match kind {
        grove_core::theme::ThemeKind::Dark => crate::app::DEFAULT_DARK_THEME,
        grove_core::theme::ThemeKind::Light => crate::app::DEFAULT_LIGHT_THEME,
    })
}

pub(super) fn theme_pane_row_is_custom(
    kind: grove_core::theme::ThemeKind,
    input: &str,
    idx: usize,
) -> bool {
    // Resolved by name via `theme::is_custom` rather than an
    // `idx >= builtins.len()` position check — same answer today (customs
    // always sort after builtins in `theme_pane_combined_rows`), but doesn't
    // depend on that ordering staying that way.
    theme_pane_combined_rows(kind, input)
        .get(idx)
        .map(|t| grove_core::theme::is_custom(&t.name))
        .unwrap_or(false)
}

/// The ProjectTheme sub-pane's list: same `theme_pane_combined_rows`
/// filtering (builtins then customs), fronted by a "Use app theme" row
/// (`None`) — only while the query is empty, so a fuzzy search doesn't
/// dangle a static row above an unrelated match list. Row N here is exactly
/// what `Grove::theme_pane_select`/`_move` (Project scope) index into,
/// mirroring `theme_pane_combined_rows`'s contract.
pub(super) fn project_theme_pane_rows(
    kind: grove_core::theme::ThemeKind,
    input: &str,
) -> Vec<Option<grove_core::theme::Theme>> {
    let mut rows: Vec<Option<grove_core::theme::Theme>> = Vec::new();
    if input.trim().is_empty() {
        rows.push(None);
    }
    rows.extend(theme_pane_combined_rows(kind, input).into_iter().map(Some));
    rows
}

/// The ProjectTheme sub-pane's Tab-cycle target — Dark ↔ Light only,
/// pulled out of `Grove::theme_pane_cycle_mode`'s Project arm so it's pure
/// and unit-testable. Contrast `next_theme_mode` above: App scope's Tab
/// cycle is a three-way Dark → Light → System → Dark; there is no System
/// mode for a per-project override, and this function's signature
/// (returning a bare `ThemeKind`, not `ThemeMode`) makes a third state
/// unrepresentable, not just untested.
pub(super) fn project_theme_next_kind(
    kind: grove_core::theme::ThemeKind,
) -> grove_core::theme::ThemeKind {
    match kind {
        grove_core::theme::ThemeKind::Dark => grove_core::theme::ThemeKind::Light,
        grove_core::theme::ThemeKind::Light => grove_core::theme::ThemeKind::Dark,
    }
}

/// The Theme sub-pane's list geometry: 36px rows (the sub-pane row height,
/// vs the standalone picker's `ROW_H`) under a 280px viewport cap — must
/// match the pane's `max_height` in `view.rs` or the centering drifts. The
/// full 280 is viewport: that container carries no padding of its own (the
/// pane's 8px padding sits on the outer wrapper around context/mode/list).
/// Both the Theme and ProjectTheme panes render their list as
/// `Column::new().spacing(2)` (view.rs ~3785/3806 and ~3919/3960) — the true
/// row pitch is 38px (36 + 2px column spacing), not the bare row height, and
/// the last row's gap doesn't exist (content is `total*38 − 2`, not
/// `total*38`).
const THEME_PANE_ROW_H: f32 = 36.0;
const THEME_PANE_SPACING: f32 = 2.0;
const THEME_PANE_VIEWPORT_CAP: f32 = 280.0;

/// Center-and-clamp scroll offset for the Theme sub-pane's list: the y that
/// centers row `selected` of `total` in the capped viewport, clamped to the
/// scrollable's valid range (0 when everything already fits). Same math as
/// `scroll_theme_picker_to_selection`, kept pure for testing — but pitched at
/// `THEME_PANE_ROW_H + THEME_PANE_SPACING` per row (see the geometry note
/// above), not the bare row height.
pub(super) fn launcher_theme_scroll_offset(total: usize, selected: usize) -> f32 {
    let pitch = THEME_PANE_ROW_H + THEME_PANE_SPACING;
    let content_h = if total == 0 {
        0.0
    } else {
        total as f32 * pitch - THEME_PANE_SPACING
    };
    let sel_y = selected as f32 * pitch;
    let viewport_h = content_h.min(THEME_PANE_VIEWPORT_CAP);
    let max_y = (content_h - viewport_h).max(0.0);
    (sel_y - (viewport_h - THEME_PANE_ROW_H) / 2.0).clamp(0.0, max_y)
}

/// The Theme sub-pane's CUSTOM section header (`section_header("CUSTOM",
/// 12.0, 6.0)`, view.rs:3859): `top` + `SETTINGS_ROOT_HEADER_LABEL_H` (10px
/// text at iced's default 1.3 line height, same approximation
/// `settings_root_scroll_offset` uses for its own headers) + `bottom`.
const THEME_PANE_CUSTOM_HEADER_H: f32 = 12.0 + SETTINGS_ROOT_HEADER_LABEL_H + 6.0;
/// The "No custom themes yet…" hint row shown in place of any custom rows
/// when there are none (view.rs:3860-3868): `Padding::from([8, 12])` (8px
/// top + bottom) around one line of 11px text, approximated with the same
/// `SETTINGS_ROOT_HEADER_LABEL_H` label-height constant the header above
/// uses (close enough at this size difference, and this whole helper is
/// already a centering approximation, not a pixel-exact layout).
const THEME_PANE_CUSTOM_HINT_H: f32 = 8.0 + SETTINGS_ROOT_HEADER_LABEL_H + 8.0;
/// The trailing "Manage themes…" row (view.rs:3878-3890): `modal_list_row_
/// sized(..., height: 32.0, ...)`, never selectable, always present.
const THEME_PANE_MANAGE_ROW_H: f32 = 32.0;

/// Center-and-clamp scroll offset for the Theme sub-pane's list, accounting
/// for the CUSTOM section header, its "no custom themes yet" hint (only
/// rendered when `n_custom == 0`), and the trailing "Manage themes…" row —
/// `launcher_theme_scroll_offset`'s uniform-row math undercounts all three,
/// so centering drifted once the list scrolled anywhere near the Custom
/// section. Walks the exact child sequence `view.rs` pushes onto the same
/// `Column::new().spacing(2)` (builtin rows, header, hint-or-custom-rows,
/// manage row), so the column spacing between each pair of children is
/// counted exactly once, same idiom as `settings_root_scroll_offset`.
pub(super) fn theme_pane_scroll_offset(n_builtin: usize, n_custom: usize, selected: usize) -> f32 {
    let mut content_h: f32 = 0.0;
    let mut sel_y: f32 = 0.0;
    for i in 0..n_builtin {
        if content_h > 0.0 {
            content_h += THEME_PANE_SPACING;
        }
        if i == selected {
            sel_y = content_h;
        }
        content_h += THEME_PANE_ROW_H;
    }
    if content_h > 0.0 {
        content_h += THEME_PANE_SPACING;
    }
    content_h += THEME_PANE_CUSTOM_HEADER_H;
    if n_custom == 0 {
        content_h += THEME_PANE_SPACING;
        content_h += THEME_PANE_CUSTOM_HINT_H;
    } else {
        for j in 0..n_custom {
            content_h += THEME_PANE_SPACING;
            let i = n_builtin + j;
            if i == selected {
                sel_y = content_h;
            }
            content_h += THEME_PANE_ROW_H;
        }
    }
    content_h += THEME_PANE_SPACING;
    content_h += THEME_PANE_MANAGE_ROW_H;
    let viewport_h = content_h.min(THEME_PANE_VIEWPORT_CAP);
    let max_y = (content_h - viewport_h).max(0.0);
    (sel_y - (viewport_h - THEME_PANE_ROW_H) / 2.0).clamp(0.0, max_y)
}

/// The theme editor's group-header geometry: same `SETTINGS_ROOT_HEADER_
/// LABEL_H` (10px text at iced's default 1.3 line height) the Settings Root
/// list uses, but the editor's `section_header(group, top, 4.0)` calls (see
/// `view.rs`) always pass `top = 10.0` for every header after the first
/// (never `12.0` like Root's), and a `4.0` bottom margin (Root uses `6.0`).
const THEME_EDITOR_GROUP_HEADER_TOP: f32 = 10.0;
const THEME_EDITOR_GROUP_HEADER_BOTTOM: f32 = 4.0;
/// The trailing "derived — not editable" chip row's height: its 12px
/// swatches sit shorter than the row's 10px label text at iced's default
/// 1.3 line height (`SETTINGS_ROOT_HEADER_LABEL_H`, same constant), plus the
/// row container's own `padding(Padding::from([4, 12]))` (4px top + bottom).
const THEME_EDITOR_DERIVED_ROW_H: f32 = SETTINGS_ROOT_HEADER_LABEL_H + 8.0;

/// Center-and-clamp scroll offset for the theme editor's row list: the
/// Theme sub-pane already keeps keyboard selection in view via
/// `launcher_theme_scroll_offset`, but that helper assumes every row is a
/// uniform `THEME_PANE_ROW_H` pitch — the editor's list isn't, since its 11
/// color rows are grouped under Surfaces/Text/Accents section headers
/// (`theme::FIELD_GROUPS`) and followed by a "derived — not editable"
/// header + chip row. Walks the same header/row sequence `view.rs` renders
/// (`settings_root_scroll_offset`'s idiom) so the selected row's y always
/// matches what's actually on screen. `selected` is always one of the 11
/// color rows — the derived strip is never selectable — but it still
/// contributes to `content_h`/`max_y` since it occupies real scroll height.
pub(in crate::gui) fn theme_editor_scroll_offset(selected: usize) -> f32 {
    let mut content_h: f32 = 0.0;
    let mut sel_y: f32 = 0.0;
    let mut prev_group: Option<&'static str> = None;
    for (i, &group) in grove_core::theme::FIELD_GROUPS.iter().enumerate() {
        if prev_group != Some(group) {
            let top = if prev_group.is_none() {
                0.0
            } else {
                THEME_EDITOR_GROUP_HEADER_TOP
            };
            if content_h > 0.0 {
                content_h += THEME_PANE_SPACING;
            }
            content_h += top + SETTINGS_ROOT_HEADER_LABEL_H + THEME_EDITOR_GROUP_HEADER_BOTTOM;
            prev_group = Some(group);
        }
        if content_h > 0.0 {
            content_h += THEME_PANE_SPACING;
        }
        if i == selected {
            sel_y = content_h;
        }
        content_h += THEME_PANE_ROW_H;
    }
    // The trailing "DERIVED — NOT EDITABLE" header + its chip row: not
    // selectable, but still real scrollable content that bounds `max_y`.
    content_h += THEME_PANE_SPACING;
    content_h += THEME_EDITOR_GROUP_HEADER_TOP
        + SETTINGS_ROOT_HEADER_LABEL_H
        + THEME_EDITOR_GROUP_HEADER_BOTTOM;
    content_h += THEME_PANE_SPACING;
    content_h += THEME_EDITOR_DERIVED_ROW_H;

    let viewport_h = content_h.min(THEME_PANE_VIEWPORT_CAP);
    let max_y = (content_h - viewport_h).max(0.0);
    (sel_y - (viewport_h - THEME_PANE_ROW_H) / 2.0).clamp(0.0, max_y)
}

/// The Settings drill-in Root list's geometry, mirroring its `view.rs`
/// render exactly: 44px palette rows and section headers in a 2px-spaced
/// column. The header total is its label — 10px text at iced's default 1.3
/// relative line height = 13px — plus the render loop's margins (top 0 for
/// the first header, 12 for later ones; bottom 6).
const SETTINGS_ROOT_ROW_H: f32 = 44.0;
const SETTINGS_ROOT_SPACING: f32 = 2.0;
const SETTINGS_ROOT_HEADER_LABEL_H: f32 = 13.0;
/// The scrollable's true viewport: the list container caps at
/// `max_height(380.0)` but carries `padding(8)` on that same container
/// (unlike the Theme pane's), and `max_height` bounds padding included —
/// 380 − 2·8. Clamping against the raw 380 under-scrolls by exactly that
/// 16px, clipping the bottom row.
const SETTINGS_ROOT_VIEWPORT_CAP: f32 = 380.0 - 16.0;

/// Center-and-clamp scroll offset for the Settings drill-in's Root list —
/// `launcher_theme_scroll_offset`'s idiom, but this list isn't uniform
/// height: a section header precedes every row whose section differs from
/// the previous row's, so the selected row's y comes from walking the
/// rendered element sequence rather than multiplying an index.
pub(super) fn settings_root_scroll_offset(rows: &[SettingRow], selected: usize) -> f32 {
    let mut content_h: f32 = 0.0;
    let mut sel_y: f32 = 0.0;
    let mut prev_section: Option<&'static str> = None;
    for (i, row) in rows.iter().enumerate() {
        let section = row.section();
        if prev_section != Some(section) {
            let top = if prev_section.is_none() { 0.0 } else { 12.0 };
            if content_h > 0.0 {
                content_h += SETTINGS_ROOT_SPACING;
            }
            content_h += top + SETTINGS_ROOT_HEADER_LABEL_H + 6.0;
            prev_section = Some(section);
        }
        if content_h > 0.0 {
            content_h += SETTINGS_ROOT_SPACING;
        }
        if i == selected {
            sel_y = content_h;
        }
        content_h += SETTINGS_ROOT_ROW_H;
    }
    let viewport_h = content_h.min(SETTINGS_ROOT_VIEWPORT_CAP);
    let max_y = (content_h - viewport_h).max(0.0);
    (sel_y - (viewport_h - SETTINGS_ROOT_ROW_H) / 2.0).clamp(0.0, max_y)
}

/// The root/typed palette list's geometry, mirroring its `view.rs` render
/// exactly — same shape as `SETTINGS_ROOT_*` above, just under the palette's
/// own section-header vocabulary (RECENT/WORKTREES/ACTIONS at root,
/// SETTINGS/SESSIONS/ACTIONS plus per-project sub-headers when typed/
/// browse-all list is long enough to group).
const PALETTE_LIST_ROW_H: f32 = 44.0;
const PALETTE_LIST_SPACING: f32 = 2.0;
const PALETTE_LIST_HEADER_LABEL_H: f32 = 13.0;
/// Same reasoning as `SETTINGS_ROOT_VIEWPORT_CAP`: the list container caps
/// at `max_height(380.0)` but that bound includes its own `padding(8)`.
const PALETTE_LIST_VIEWPORT_CAP: f32 = 380.0 - 16.0;

/// Center-and-clamp scroll offset for the palette's root/typed list —
/// `settings_root_scroll_offset`'s idiom, walking the same header/row
/// sequence `view.rs`'s `else` branch (view.rs:4462-4544) renders, so the
/// selected row's y always matches what's actually on screen.
pub(super) fn palette_scroll_offset(rows: &[PaletteRow], selected: usize, root_mode: bool) -> f32 {
    let mut content_h: f32 = 0.0;
    let mut sel_y: f32 = 0.0;

    let push_header = |content_h: &mut f32, top: f32, bottom: f32| {
        if *content_h > 0.0 {
            *content_h += PALETTE_LIST_SPACING;
        }
        *content_h += top + PALETTE_LIST_HEADER_LABEL_H + bottom;
    };

    if root_mode {
        let mut printed_recent = false;
        let mut last_wt_project: Option<usize> = None;
        let mut printed_actions = false;
        for (i, row) in rows.iter().enumerate() {
            match row {
                PaletteRow::Recent { .. } => {
                    if !printed_recent {
                        push_header(&mut content_h, 0.0, 6.0);
                        printed_recent = true;
                    }
                }
                PaletteRow::Combo { proj, .. } => {
                    if last_wt_project != Some(*proj) {
                        let top = if i == 0 { 0.0 } else { 12.0 };
                        push_header(&mut content_h, top, 6.0);
                        last_wt_project = Some(*proj);
                    }
                }
                _ => {
                    if !printed_actions {
                        let top = if printed_recent || last_wt_project.is_some() {
                            12.0
                        } else {
                            0.0
                        };
                        push_header(&mut content_h, top, 6.0);
                        printed_actions = true;
                    }
                }
            }
            if content_h > 0.0 {
                content_h += PALETTE_LIST_SPACING;
            }
            if i == selected {
                sel_y = content_h;
            }
            content_h += PALETTE_LIST_ROW_H;
        }
    } else {
        let has_settings = rows.iter().any(|r| matches!(r, PaletteRow::Setting(_)));
        let mut printed_settings = false;
        let mut printed_sessions = false;
        let mut printed_actions = false;
        let session_project_order: Vec<usize> = {
            let mut seen = Vec::new();
            for r in rows.iter() {
                if let PaletteRow::Recent { proj, .. } | PaletteRow::Combo { proj, .. } = r {
                    if !seen.contains(proj) {
                        seen.push(*proj);
                    }
                }
            }
            seen
        };
        let session_row_count = rows
            .iter()
            .filter(|r| matches!(r, PaletteRow::Recent { .. } | PaletteRow::Combo { .. }))
            .count();
        let grouped_by_project = session_project_order.len() > 2 || session_row_count > 10;
        let mut last_grouped_project: Option<usize> = None;

        for (i, row) in rows.iter().enumerate() {
            let is_setting = matches!(row, PaletteRow::Setting(_));
            let is_session = matches!(row, PaletteRow::Recent { .. } | PaletteRow::Combo { .. });
            if has_settings && is_setting && !printed_settings {
                push_header(&mut content_h, 0.0, 6.0);
                printed_settings = true;
            } else if is_session && !printed_sessions {
                let top = if printed_settings { 12.0 } else { 0.0 };
                push_header(&mut content_h, top, 6.0);
                printed_sessions = true;
            } else if !is_setting && !is_session && !printed_actions {
                let top = if printed_sessions || printed_settings {
                    12.0
                } else {
                    0.0
                };
                push_header(&mut content_h, top, 6.0);
                printed_actions = true;
            }
            if grouped_by_project && is_session {
                let proj = match row {
                    PaletteRow::Recent { proj, .. } | PaletteRow::Combo { proj, .. } => *proj,
                    _ => unreachable!(),
                };
                if last_grouped_project != Some(proj) {
                    let top = if last_grouped_project.is_none() {
                        0.0
                    } else {
                        8.0
                    };
                    push_header(&mut content_h, top, 4.0);
                    last_grouped_project = Some(proj);
                }
            }
            if content_h > 0.0 {
                content_h += PALETTE_LIST_SPACING;
            }
            if i == selected {
                sel_y = content_h;
            }
            content_h += PALETTE_LIST_ROW_H;
        }
    }

    let viewport_h = content_h.min(PALETTE_LIST_VIEWPORT_CAP);
    let max_y = (content_h - viewport_h).max(0.0);
    (sel_y - (viewport_h - PALETTE_LIST_ROW_H) / 2.0).clamp(0.0, max_y)
}

/// Keep-identity-else-clamp reselection for the drill-in Root list after a
/// toggle refilters it: the cursor follows `activated` to its new position
/// when the row survived, and otherwise clamps the old index into the new
/// length (`launcher::clamp` handles the empty list).
pub(super) fn reselect_setting(rows: &[SettingRow], activated: SettingRow, old: usize) -> usize {
    rows.iter()
        .position(|s| *s == activated)
        .unwrap_or_else(|| crate::gui::launcher::clamp(old, 0, rows.len()))
}

/// Resolve mod+digit `n` (1-based) to the list index of the nth session
/// (`Recent`/`Combo`) row, skipping settings and action rows — in typed
/// mode those sort above sessions (B2), and the digits must keep meaning
/// "nth session", not "nth row". `None` when fewer than `n` session rows
/// exist. Root mode is unchanged by construction: recents come first there.
pub(super) fn nth_session_row(rows: &[PaletteRow], n: usize) -> Option<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| matches!(r, PaletteRow::Recent { .. } | PaletteRow::Combo { .. }))
        .nth(n.checked_sub(1)?)
        .map(|(i, _)| i)
}

/// A row's [`PaletteRowIdentity`] — the content-based key `resolve_row_by_
/// identity` matches against, decoupled from the row's transient index.
pub(super) fn row_identity(row: &PaletteRow) -> PaletteRowIdentity {
    match row {
        PaletteRow::Recent {
            proj,
            wt_path,
            agent,
        }
        | PaletteRow::Combo {
            proj,
            wt_path,
            agent,
        } => PaletteRowIdentity::Session {
            proj: *proj,
            wt_path: wt_path.clone(),
            agent: *agent,
        },
        PaletteRow::NewSession => PaletteRowIdentity::NewSession,
        PaletteRow::TerminalHome => PaletteRowIdentity::TerminalHome,
        PaletteRow::TerminalWt => PaletteRowIdentity::TerminalWt,
        PaletteRow::AddProject => PaletteRowIdentity::AddProject,
        PaletteRow::SwitchToSession => PaletteRowIdentity::SwitchToSession,
        PaletteRow::Settings => PaletteRowIdentity::Settings,
        PaletteRow::Setting(s) => PaletteRowIdentity::Setting(*s),
        PaletteRow::ReloadThemes => PaletteRowIdentity::ReloadThemes,
    }
}

/// Resolve an activation target by identity rather than by trusting a
/// possibly-stale index: `identity` is the row that was highlighted the last
/// time `selected` was written (see `Grove::set_palette_selected`); this
/// finds that same row wherever it now sits in a freshly rebuilt `rows`
/// (self-healing past a re-sort/re-group/re-filter that happened since), or
/// reports `None` if it's simply gone — it never falls back to activating
/// whatever row now happens to sit at the stale index. `fallback` only
/// applies when `identity` itself is `None` (defensive: a state that hasn't
/// captured one yet).
pub(super) fn resolve_row_by_identity(
    rows: &[PaletteRow],
    identity: &Option<PaletteRowIdentity>,
    fallback: usize,
) -> Option<usize> {
    match identity {
        Some(id) => rows.iter().position(|r| row_identity(r) == *id),
        None => (fallback < rows.len()).then_some(fallback),
    }
}

/// Rank the typed/browse-all Combo list: score desc, recency asc as a
/// tiebreak (`sort_by` is stable, so combos absent from recents — tied at
/// `usize::MAX` — keep their relative store-build order), then re-cluster
/// into per-project runs once the list is too broad to read as one flat
/// ranking (see `view.rs`'s header logic, which recomputes the same
/// threshold from the returned rows rather than trusting a flag).
pub(super) fn rank_and_group_combos(mut scored: Vec<(u32, usize, PaletteRow)>) -> Vec<PaletteRow> {
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut combos: Vec<PaletteRow> = scored.into_iter().map(|(_, _, r)| r).collect();
    let mut project_order: Vec<usize> = Vec::new();
    for r in &combos {
        if let PaletteRow::Combo { proj, .. } = r {
            if !project_order.contains(proj) {
                project_order.push(*proj);
            }
        }
    }
    if project_order.len() > 2 || combos.len() > 10 {
        let mut grouped = Vec::with_capacity(combos.len());
        for proj in project_order {
            grouped.extend(
                combos
                    .iter()
                    .filter(|r| matches!(r, PaletteRow::Combo { proj: p, .. } if *p == proj))
                    .cloned(),
            );
        }
        combos = grouped;
    }
    combos
}

/// Project visit order for the root state's no-recents worktree fallback:
/// the active project first, then every other project in store order.
/// Clamps `active` into range so a stale index (project removed mid-session)
/// can't panic.
pub(super) fn root_project_order(n: usize, active: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let first = active.min(n - 1);
    let mut order = vec![first];
    order.extend((0..n).filter(|&i| i != first));
    order
}

/// The three states of the Theme sub-pane's mode row, in Tab-cycle order.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum ThemeMode {
    Dark,
    Light,
    System,
}

/// Tab in the Theme sub-pane cycles the mode row Dark → Light → System →
/// Dark. The current mode is System whenever `follow_system` is set (that's
/// also how the segments render), else the shown list's kind.
pub(super) fn next_theme_mode(
    kind: grove_core::theme::ThemeKind,
    follow_system: bool,
) -> ThemeMode {
    if follow_system {
        ThemeMode::Dark
    } else {
        match kind {
            grove_core::theme::ThemeKind::Dark => ThemeMode::Light,
            grove_core::theme::ThemeKind::Light => ThemeMode::System,
        }
    }
}

/// One action in the update-available strip under the Check-for-updates row
/// (E3). Mirrors the Settings modal's update-available action row.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(in crate::gui) enum UpdateAction {
    UpdateNow,
    SkipVersion,
    CopyUrl,
}

impl UpdateAction {
    pub(in crate::gui) fn label(self) -> &'static str {
        match self {
            UpdateAction::UpdateNow => "Update now",
            UpdateAction::SkipVersion => "Skip version",
            UpdateAction::CopyUrl => "Copy URL",
        }
    }

    /// The top-level `Msg` this action fires when a full-size Settings-modal
    /// button (as opposed to the palette's index-based
    /// `Msg::SessionLauncher(UpdateActionPick(i))` routing) activates it.
    /// This is the single place to update when a new `UpdateAction` variant
    /// is added — an exhaustive match (no wildcard arm) so the compiler
    /// forces every call site that renders `UpdateAction`s to be revisited.
    pub(in crate::gui) fn to_msg(self) -> GMsg {
        match self {
            UpdateAction::UpdateNow => GMsg::Upgrade(UpgradeMsg::StartUpdate),
            UpdateAction::SkipVersion => GMsg::Upgrade(UpgradeMsg::SkipVersion),
            UpdateAction::CopyUrl => GMsg::Upgrade(UpgradeMsg::CopyReleaseUrl),
        }
    }
}

/// The update-available strip's actions, in display order. "Update now" is
/// hidden for `InstallMethod::Unknown` installs (notify-only) — the same
/// guard `settings_modal`'s action row applies — so the strip and the
/// keyboard nav derive from one list and indices can never disagree.
pub(in crate::gui) fn update_available_actions(method_unknown: bool) -> Vec<UpdateAction> {
    let mut actions = Vec::with_capacity(3);
    if !method_unknown {
        actions.push(UpdateAction::UpdateNow);
    }
    actions.push(UpdateAction::SkipVersion);
    actions.push(UpdateAction::CopyUrl);
    actions
}

/// Whether activating the Check-for-updates row expands the actions strip
/// (a release is already known to be available — re-checking would only
/// throw that answer away) instead of firing a fresh check.
pub(super) fn check_updates_opens_strip(upgrade: &UpgradeState) -> bool {
    matches!(upgrade, UpgradeState::Available(_))
}
