//! The command palette's PURE half. Nothing here touches gpui.
//!
//! Ported from `src/gui/launcher.rs` (the fuzzy scorer, :95-240),
//! `src/gui/session_launcher/state.rs:13-247` (the row model and the
//! identity rule), `src/gui/session_launcher/helpers.rs` (ranking :599-628,
//! identity :546-598, `root_project_order` :629-649, `next_theme_mode`
//! :650-673, `update_available_actions` :701-716,
//! `switch_terminal_rows`/`merge_switch_rows` :717-737,
//! `check_updates_opens_strip` :738-745, `agent_sel_for` :540) and
//! `src/gui/update/settings_rows.rs:1-135` (the `SettingRow` table).
//!
//! # Deliberately NOT ported, with reasons
//!
//! `helpers.rs`'s four scroll-offset helpers — `launcher_theme_scroll_offset`
//! (:178-190), `theme_pane_scroll_offset` (:213-260),
//! `theme_editor_scroll_offset` (:280-330) and `settings_root_scroll_offset`
//! — and the tests that pin them are **not** carried across, and neither are
//! the `THEME_PANE_ROW_H`/`SETTINGS_ROOT_HEADER_LABEL_H` pixel constants they
//! are built on. They compute a pixel scroll offset by re-walking iced's own
//! `Column::spacing` layout, approximating iced's default 1.3 line height for
//! every section header. That arithmetic is a model of *iced's* layout engine
//! and is wrong for any other one: gpui scrolls a list by **item index**
//! (`ScrollHandle::scroll_to_item` / `uniform_list`), so the equivalent code
//! here is "keep the selected index visible" with no geometry at all. Porting
//! the pixel math would have meant maintaining a second, permanently-drifting
//! model of a layout engine Grove no longer uses.
//!
//! Everything else in `helpers.rs` and its acceptance tests is behavior, not
//! geometry, and is carried across below.

use grove_core::agent::Agent;

// ── the fuzzy scorer (`src/gui/launcher.rs:95-240`) ──────────────────────

/// A combo whose worktree/branch name matches at least as strongly as a
/// project-name match of the same quality — a flat additive bonus on top of a
/// worktree hit, since that is what the caller is usually typing toward.
const WORKTREE_BONUS: u32 = 10;

/// Fuzzy filter: split `query` on whitespace, require each term to match at
/// least one of `project`/`worktree`/`agent_label`. Empty query matches
/// everything.
pub fn fuzzy_match(query: &str, project: &str, worktree: &str, agent_label: &str) -> bool {
    fuzzy_score(query, project, worktree, agent_label).is_some()
}

/// Score a candidate row, `None` meaning "no match" (AND across terms).
/// Higher is better. Per term, per field: a match at the very start of the
/// field scores highest, then a match right after a `/ - _` or space, then any
/// other contiguous substring match, and last — below every contiguous
/// match — a scattered subsequence match. Empty query scores 0.
pub fn fuzzy_score(query: &str, project: &str, worktree: &str, agent_label: &str) -> Option<u32> {
    if query.trim().is_empty() {
        return Some(0);
    }
    let mut total: u32 = 0;
    for term in query.split_whitespace() {
        let p = term_field_score(term, project);
        let w = term_field_score(term, worktree).map(|s| s + WORKTREE_BONUS);
        let a = term_field_score(term, agent_label);
        total = total.saturating_add([p, w, a].into_iter().flatten().max()?);
    }
    Some(total)
}

fn term_field_score(term: &str, haystack: &str) -> Option<u32> {
    if term.is_empty() {
        return None;
    }
    let hay: Vec<char> = haystack.chars().collect();
    let need: Vec<char> = term.chars().collect();
    if need.len() <= hay.len() {
        for start in 0..=(hay.len() - need.len()) {
            let matches = need
                .iter()
                .enumerate()
                .all(|(i, nc)| hay[start + i].to_lowercase().eq(nc.to_lowercase()));
            if matches {
                return Some(if start == 0 {
                    100 // prefix of the whole field
                } else if matches!(hay[start - 1], '/' | '-' | '_' | ' ') {
                    80 // start of a token (after a path/branch separator)
                } else {
                    50 // contiguous, but mid-token
                });
            }
        }
    }
    subsequence_score(&hay, &need)
}

/// Scattered (non-contiguous, in-order) subsequence match, always scored below
/// every contiguous case above (max 50) — 1..=20, tighter clusters scoring
/// higher.
fn subsequence_score(hay: &[char], need: &[char]) -> Option<u32> {
    if need.is_empty() {
        return None;
    }
    let mut hi = 0;
    let mut first = None;
    let mut last = 0;
    for (ni, nc) in need.iter().enumerate() {
        let mut found = false;
        while hi < hay.len() {
            if hay[hi].to_lowercase().eq(nc.to_lowercase()) {
                if ni == 0 {
                    first = Some(hi);
                }
                last = hi;
                hi += 1;
                found = true;
                break;
            }
            hi += 1;
        }
        if !found {
            return None;
        }
    }
    let span = last - first.unwrap_or(0) + 1;
    let tightness = (need.len() as u32 * 10).saturating_sub(span as u32);
    Some(tightness.clamp(1, 20))
}

/// Case-insensitive substring search returning the **char** index range of the
/// first occurrence, so callers can slice the original string by `chars()`
/// without a lowercase transform changing UTF-8 byte lengths.
// TODO(unwired): the palette's match-highlight trio (`ci_find_range` ->
// `fuzzy_match_indices` -> `FuzzyMatch`) is complete and tested, but no row
// renderer highlights the matched spans, so nothing calls it.
#[allow(dead_code)]
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

/// The matched char ranges per field, for the typing-state highlight.
// TODO(unwired): see `ci_find_range`.
#[allow(dead_code)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub matched: bool,
    pub project: Vec<(usize, usize)>,
    pub worktree: Vec<(usize, usize)>,
    pub agent: Vec<(usize, usize)>,
}

/// Same matching semantics as [`fuzzy_match`], plus the ranges to highlight.
// TODO(unwired): see `ci_find_range`.
#[allow(dead_code)]
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
        if !hit && !fuzzy_match(term, project, worktree, agent_label) {
            out.matched = false;
            return out;
        }
    }
    out
}

// ── the settings table (`src/gui/update/settings_rows.rs:1-106`) ─────────

/// One settings entry surfaced by the palette, either as a root-mode direct
/// match or as a row in the Settings drill-in. Variant order is the drill-in's
/// display order within its section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingRow {
    Theme,
    AppSize,
    ProjectThemes,
    Backend,
    Permissions,
    Telemetry,
    Chrome,
    DefaultAgent,
    CheckUpdates,
}

impl SettingRow {
    /// Every setting, in section/definition (= drill-in display) order.
    pub const ALL: [SettingRow; 9] = [
        SettingRow::Theme,
        SettingRow::AppSize,
        SettingRow::ProjectThemes,
        SettingRow::Backend,
        SettingRow::Permissions,
        SettingRow::Telemetry,
        SettingRow::Chrome,
        SettingRow::DefaultAgent,
        SettingRow::CheckUpdates,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingRow::Theme => "App theme",
            SettingRow::AppSize => "App size",
            SettingRow::ProjectThemes => "Project themes",
            SettingRow::Backend => "Backend",
            SettingRow::Permissions => "Permissions",
            SettingRow::Telemetry => "Telemetry",
            SettingRow::Chrome => "Claude in Chrome",
            SettingRow::DefaultAgent => "Default agent",
            SettingRow::CheckUpdates => "Check for updates",
        }
    }

    /// Leading 24px icon sprite. `ProjectThemes`/`Telemetry`/`Chrome` render a
    /// checkbox glyph instead and never consult this.
    pub fn icon_name(self) -> &'static str {
        match self {
            SettingRow::Theme => "contrast",
            SettingRow::AppSize => "grid",
            SettingRow::ProjectThemes | SettingRow::Telemetry | SettingRow::Chrome => "check",
            SettingRow::Backend => "term",
            SettingRow::Permissions => "ring",
            SettingRow::DefaultAgent => "sparkle",
            SettingRow::CheckUpdates => "restart",
        }
    }

    pub fn section(self) -> &'static str {
        match self {
            SettingRow::Theme | SettingRow::AppSize | SettingRow::ProjectThemes => "APPEARANCE",
            SettingRow::Backend
            | SettingRow::Permissions
            | SettingRow::Telemetry
            | SettingRow::Chrome => "AGENTS / TERMINAL",
            SettingRow::DefaultAgent => "TOOLS",
            SettingRow::CheckUpdates => "UPDATES",
        }
    }

    /// Rows that flip in place instead of opening a pane.
    // Exercised only by this module's `#[cfg(test)]` row table; the rebuilt
    // Settings modal decides toggle-vs-pane per row at its own call site.
    #[allow(dead_code)]
    pub fn is_toggle(self) -> bool {
        matches!(
            self,
            SettingRow::ProjectThemes | SettingRow::Telemetry | SettingRow::Chrome
        )
    }
}

// ── the row model (`session_launcher/state.rs:216-247`) ──────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteRow {
    Recent {
        proj: usize,
        wt_path: String,
        agent: Agent,
    },
    Combo {
        proj: usize,
        wt_path: String,
        agent: Agent,
    },
    NewSession,
    TerminalHome,
    TerminalWt,
    AddProject,
    /// ACTIONS row: opens the "switch to session" drill-in.
    SwitchToSession,
    /// ACTIONS row: opens the Settings drill-in.
    Settings,
    /// A direct settings match surfaced while typing at root.
    Setting(SettingRow),
    /// ACTIONS row, keyword-only: re-reads `themes.json`.
    ReloadThemes,
}

/// The content-based key activation resolves against, decoupled from a row's
/// transient index (`state.rs:88-115`).
///
/// `proj` is an index rather than a name: a project can only be removed via
/// its own confirmation modal, and the slot holds exactly one modal at a time,
/// so a project cannot be removed out from under an *open* launcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowIdentity {
    Session {
        proj: usize,
        wt_path: String,
        agent: Agent,
    },
    NewSession,
    TerminalHome,
    TerminalWt,
    AddProject,
    SwitchToSession,
    Settings,
    Setting(SettingRow),
    ReloadThemes,
}

/// `helpers.rs:546-570`.
pub fn row_identity(row: &PaletteRow) -> RowIdentity {
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
        } => RowIdentity::Session {
            proj: *proj,
            wt_path: wt_path.clone(),
            agent: *agent,
        },
        PaletteRow::NewSession => RowIdentity::NewSession,
        PaletteRow::TerminalHome => RowIdentity::TerminalHome,
        PaletteRow::TerminalWt => RowIdentity::TerminalWt,
        PaletteRow::AddProject => RowIdentity::AddProject,
        PaletteRow::SwitchToSession => RowIdentity::SwitchToSession,
        PaletteRow::Settings => RowIdentity::Settings,
        PaletteRow::Setting(s) => RowIdentity::Setting(*s),
        PaletteRow::ReloadThemes => RowIdentity::ReloadThemes,
    }
}

/// Resolve an activation target by identity rather than by trusting a
/// possibly-stale index (`helpers.rs:572-598`). Finds the row wherever it now
/// sits in a freshly rebuilt list, or reports `None` if it is gone — it never
/// falls back to activating whatever row now happens to sit at the stale
/// index. `fallback` applies only when `identity` is `None`.
pub fn resolve_row_by_identity(
    rows: &[PaletteRow],
    identity: Option<&RowIdentity>,
    fallback: usize,
) -> Option<usize> {
    match identity {
        Some(id) => rows.iter().position(|r| row_identity(r) == *id),
        None => (fallback < rows.len()).then_some(fallback),
    }
}

/// Rank the typed/browse-all Combo list: score desc, recency asc as a tiebreak
/// (`sort_by` is stable, so combos absent from recents — tied at
/// `usize::MAX` — keep their relative store-build order), then re-cluster into
/// per-project runs once the list is too broad to read as one flat ranking
/// (`helpers.rs:599-628`).
pub fn rank_and_group_combos(mut scored: Vec<(u32, usize, PaletteRow)>) -> Vec<PaletteRow> {
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

/// Project visit order for the root state's no-recents worktree fallback: the
/// active project first, then every other project in store order. Clamps
/// `active` so a stale index cannot panic (`helpers.rs:629-649`).
pub fn root_project_order(n: usize, active: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let first = active.min(n - 1);
    let mut order = vec![first];
    order.extend((0..n).filter(|&i| i != first));
    order
}

/// `helpers.rs:540-542`.
pub fn agent_sel_for(available: &[Agent], agent: Agent) -> usize {
    available.iter().position(|a| *a == agent).unwrap_or(0)
}

/// The three states of the Theme sub-pane's mode row, in Tab-cycle order.
// TODO(unwired): the Tab-cycles-the-theme-mode row was ported with its test but
// never given a key handler; the rebuilt Settings modal sets the mode directly.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
    System,
}

/// Tab in the Theme sub-pane cycles Dark → Light → System → Dark. The current
/// mode is System whenever `follow_system` is set (`helpers.rs:650-673`).
// TODO(unwired): see `ThemeMode`.
#[allow(dead_code)]
pub fn next_theme_mode(dark: bool, follow_system: bool) -> ThemeMode {
    if follow_system {
        ThemeMode::Dark
    } else if dark {
        ThemeMode::Light
    } else {
        ThemeMode::System
    }
}

/// One action in the update-available strip (`helpers.rs:664-716`).
// TODO(unwired): the whole update-available strip — `UpdateAction`, its
// `label`, `update_available_actions` and `check_updates_opens_strip` — is
// built and tested, but no view expands a strip when an update is known.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAction {
    UpdateNow,
    SkipVersion,
    CopyUrl,
}

impl UpdateAction {
    // TODO(unwired): see `UpdateAction`.
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            UpdateAction::UpdateNow => "Update now",
            UpdateAction::SkipVersion => "Skip version",
            UpdateAction::CopyUrl => "Copy URL",
        }
    }
}

/// The strip's actions, in display order. "Update now" is hidden for an
/// unknown install method (notify-only), so the strip and the keyboard nav
/// derive from one list and their indices can never disagree.
// TODO(unwired): see `UpdateAction`.
#[allow(dead_code)]
pub fn update_available_actions(method_unknown: bool) -> Vec<UpdateAction> {
    let mut actions = Vec::with_capacity(3);
    if !method_unknown {
        actions.push(UpdateAction::UpdateNow);
    }
    actions.push(UpdateAction::SkipVersion);
    actions.push(UpdateAction::CopyUrl);
    actions
}

/// One row of the switch drill-in: sessions first, then home terminals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchRow {
    Session(usize),
    Terminal(usize),
}

/// Home terminals matching `input`, as indices in sidebar order. Matched on
/// the terminal's own label plus the constant subtitle the drill-in renders,
/// so typing either the number or "term"/"home terminal" finds it
/// (`helpers.rs:717-729`).
pub fn switch_terminal_rows(labels: &[String], input: &str) -> Vec<usize> {
    (0..labels.len())
        .filter(|&i| fuzzy_match(input, &labels[i], "", "home terminal"))
        .collect()
}

/// The switch drill-in's session order: most-recently-used first, except the
/// currently focused session, which goes last — alt-tab semantics, so the
/// first row is always "the session I was in before this one". Ties (never
/// focused, `used == 0`) keep insertion order via the stable sort.
///
/// Takes `(index, used, is_active)` so the rule stays testable without a
/// registry or a gpui `App` — the pure-helper idiom this module is built on.
pub fn order_switch_sessions(sessions: &[(usize, u64, bool)]) -> Vec<usize> {
    let mut sessions = sessions.to_vec();
    sessions.sort_by(|a, b| a.2.cmp(&b.2).then(b.1.cmp(&a.1)));
    sessions.into_iter().map(|(i, _, _)| i).collect()
}

/// Splice the drill-in's two filtered groups into its single display order
/// (`helpers.rs:730-737`).
pub fn merge_switch_rows(sessions: &[usize], terminals: &[usize]) -> Vec<SwitchRow> {
    sessions
        .iter()
        .map(|&i| SwitchRow::Session(i))
        .chain(terminals.iter().map(|&i| SwitchRow::Terminal(i)))
        .collect()
}

/// Whether activating the Check-for-updates row expands the actions strip (a
/// release is already known to be available — re-checking would only throw
/// that answer away) instead of firing a fresh check (`helpers.rs:738-745`).
// TODO(unwired): see `UpdateAction`.
#[allow(dead_code)]
pub fn check_updates_opens_strip(update_available: bool) -> bool {
    update_available
}

/// Keep `selected` inside the window `[offset, offset + visible)`, returning
/// the new offset.
///
/// This is the gpui replacement for `helpers.rs`'s four pixel scroll-offset
/// helpers (see the module doc): gpui scrolls by item index, so "keep the
/// selection visible" needs no layout model at all.
///
/// Test-only: gpui's own list scrolling now handles this at runtime, so
/// nothing calls this outside its unit tests below — kept `#[cfg(test)]`
/// because those tests still document the intended windowing behaviour.
#[cfg(test)]
pub fn scroll_offset_for(offset: usize, selected: usize, visible: usize, total: usize) -> usize {
    if visible == 0 || total == 0 {
        return 0;
    }
    let max_offset = total.saturating_sub(visible);
    let offset = offset.min(max_offset);
    if selected < offset {
        selected
    } else if selected >= offset + visible {
        (selected + 1 - visible).min(max_offset)
    } else {
        offset
    }
}

/// Move a selection cursor by `delta`, wrapping (`src/app/util.rs:5-10`).
pub fn cycle(cur: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (cur as i32 + delta).rem_euclid(len as i32) as usize
}

/// The root list shows at most this many recents so the action rows stay in
/// view.
pub const MAX_ROOT_RECENTS: usize = 3;

/// The root list: recents first, then the actions block
/// (`session_launcher/palette.rs`'s `palette_rows` root arm).
///
/// `recents` are `(proj, wt_path, agent)` in most-recent-first order and are
/// emitted as `Recent` rows; when there are none, the worktree fallback walks
/// [`root_project_order`] and emits the first worktree of each project as a
/// `Combo` instead, so the root list is never empty on a fresh install.
pub fn root_rows(
    recents: &[(usize, String, Agent)],
    fallback_worktrees: &[(usize, String, Agent)],
    project_count: usize,
    active_project: usize,
) -> Vec<PaletteRow> {
    let mut rows: Vec<PaletteRow> = Vec::new();
    if recents.is_empty() {
        let order = root_project_order(project_count, active_project);
        for proj in order {
            if let Some((p, wt, agent)) = fallback_worktrees.iter().find(|(p, ..)| *p == proj) {
                rows.push(PaletteRow::Combo {
                    proj: *p,
                    wt_path: wt.clone(),
                    agent: *agent,
                });
            }
        }
    } else {
        for (proj, wt, agent) in recents.iter().take(MAX_ROOT_RECENTS) {
            rows.push(PaletteRow::Recent {
                proj: *proj,
                wt_path: wt.clone(),
                agent: *agent,
            });
        }
    }
    rows.extend([
        PaletteRow::NewSession,
        PaletteRow::TerminalHome,
        PaletteRow::TerminalWt,
        PaletteRow::SwitchToSession,
        PaletteRow::AddProject,
        PaletteRow::Settings,
    ]);
    rows
}

/// The typing / browse-all list: every project x worktree combo, fuzzy-scored
/// and ranked, plus any directly-matching settings rows, the Settings
/// drill-in opener, and the keyword-only action rows.
pub fn typed_rows(
    query: &str,
    combos: &[(usize, String, String, Agent)],
    recency: &[(usize, String, Agent)],
) -> Vec<PaletteRow> {
    let mut scored: Vec<(u32, usize, PaletteRow)> = Vec::new();
    for (proj, project_name, wt_path, agent) in combos {
        let wt_name = std::path::Path::new(wt_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(wt_path);
        let Some(score) = fuzzy_score(query, project_name, wt_name, agent.label()) else {
            continue;
        };
        let rank = recency
            .iter()
            .position(|(p, w, a)| p == proj && w == wt_path && a == agent)
            .unwrap_or(usize::MAX);
        scored.push((
            score,
            rank,
            PaletteRow::Combo {
                proj: *proj,
                wt_path: wt_path.clone(),
                agent: *agent,
            },
        ));
    }
    let mut rows = rank_and_group_combos(scored);
    if !query.trim().is_empty() && fuzzy_match(query, "settings", "", "") {
        rows.push(PaletteRow::Settings);
    }
    for s in SettingRow::ALL {
        if fuzzy_score(query, s.label(), "", s.section()).is_some() && !query.trim().is_empty() {
            rows.push(PaletteRow::Setting(s));
        }
    }
    if !query.trim().is_empty() && fuzzy_match(query, "add project", "", "") {
        rows.push(PaletteRow::AddProject);
    }
    if !query.trim().is_empty() && fuzzy_match(query, "reload themes", "", "") {
        rows.push(PaletteRow::ReloadThemes);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `unwrap_used` is denied crate-wide; this names the failure instead.
    fn score(q: &str, p: &str, w: &str, a: &str) -> u32 {
        let Some(s) = fuzzy_score(q, p, w, a) else {
            unreachable!("{q:?} must match {p:?}/{w:?}/{a:?}")
        };
        s
    }

    fn combo(proj: usize, wt: &str) -> PaletteRow {
        PaletteRow::Combo {
            proj,
            wt_path: wt.into(),
            agent: Agent::Claude,
        }
    }

    // ── the scorer ───────────────────────────────────────────────────────

    #[test]
    fn an_empty_query_matches_everything_at_score_zero() {
        assert_eq!(fuzzy_score("", "grove", "main", "claude"), Some(0));
        assert_eq!(fuzzy_score("   ", "grove", "main", "claude"), Some(0));
        assert!(fuzzy_match("", "anything", "", ""));
    }

    #[test]
    fn a_field_prefix_outranks_a_token_start_which_outranks_a_mid_token_hit() {
        let prefix = score("gro", "grove", "", "");
        let token = score("core", "grove-core", "", "");
        let mid = score("rov", "grove", "", "");
        assert!(prefix > token, "{prefix} !> {token}");
        assert!(token > mid, "{token} !> {mid}");
    }

    #[test]
    fn every_contiguous_match_outranks_a_scattered_subsequence() {
        let contiguous = score("rov", "grove", "", "");
        let scattered = score("grv", "grove", "", "");
        assert!(contiguous > scattered, "{contiguous} !> {scattered}");
        assert!(scattered > 0);
    }

    #[test]
    fn a_worktree_hit_outranks_an_equal_quality_project_hit() {
        let on_project = score("feat", "feat", "other", "");
        let on_worktree = score("feat", "other", "feat", "");
        assert_eq!(on_worktree, on_project + WORKTREE_BONUS);
    }

    #[test]
    fn every_term_must_match_somewhere() {
        assert!(fuzzy_match("grove main", "grove", "main", "claude"));
        assert!(!fuzzy_match("grove nope", "grove", "main", "claude"));
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(fuzzy_match("GROVE", "grove", "", ""));
        assert!(fuzzy_match("grove", "GROVE", "", ""));
    }

    #[test]
    fn the_agent_label_is_searchable() {
        assert!(fuzzy_match("codex", "grove", "main", "codex"));
    }

    #[test]
    fn match_indices_are_char_ranges_not_byte_offsets() {
        let m = fuzzy_match_indices("é", "café", "", "");
        assert!(m.matched);
        // "café" is 5 bytes but 4 chars; the range must be char-based.
        assert_eq!(m.project, vec![(3, 4)]);
    }

    #[test]
    fn match_indices_report_no_match_for_an_unmatched_term() {
        let m = fuzzy_match_indices("grove zzz", "grove", "main", "claude");
        assert!(!m.matched);
    }

    // ── ranking ──────────────────────────────────────────────────────────

    #[test]
    fn ranking_is_score_desc_then_recency_asc() {
        let rows = rank_and_group_combos(vec![
            (10, 5, combo(0, "/a")),
            (10, 1, combo(0, "/b")),
            (50, 9, combo(0, "/c")),
        ]);
        assert_eq!(
            rows,
            vec![combo(0, "/c"), combo(0, "/b"), combo(0, "/a")],
            "higher score first; equal scores break on recency"
        );
    }

    #[test]
    fn ranking_is_stable_for_rows_absent_from_recents() {
        let rows = rank_and_group_combos(vec![
            (10, usize::MAX, combo(0, "/first")),
            (10, usize::MAX, combo(0, "/second")),
        ]);
        assert_eq!(rows, vec![combo(0, "/first"), combo(0, "/second")]);
    }

    #[test]
    fn a_narrow_list_stays_a_flat_ranking() {
        // Two projects, four rows: below both re-cluster thresholds.
        let rows = rank_and_group_combos(vec![
            (30, 0, combo(0, "/a")),
            (20, 1, combo(1, "/b")),
            (10, 2, combo(0, "/c")),
        ]);
        assert_eq!(rows, vec![combo(0, "/a"), combo(1, "/b"), combo(0, "/c")]);
    }

    #[test]
    fn three_projects_re_cluster_into_per_project_runs() {
        let rows = rank_and_group_combos(vec![
            (30, 0, combo(0, "/a")),
            (25, 1, combo(1, "/b")),
            (20, 2, combo(2, "/c")),
            (15, 3, combo(0, "/d")),
        ]);
        assert_eq!(
            rows,
            vec![
                combo(0, "/a"),
                combo(0, "/d"),
                combo(1, "/b"),
                combo(2, "/c")
            ],
            "project order follows first appearance in the flat ranking"
        );
    }

    #[test]
    fn more_than_ten_rows_re_cluster_even_with_two_projects() {
        let scored: Vec<_> = (0..12)
            .map(|i| (100 - i as u32, i, combo(i % 2, &format!("/w{i}"))))
            .collect();
        let rows = rank_and_group_combos(scored);
        let projects: Vec<usize> = rows
            .iter()
            .filter_map(|r| match r {
                PaletteRow::Combo { proj, .. } => Some(*proj),
                _ => None,
            })
            .collect();
        // Every project-0 row comes before every project-1 row.
        let Some(split) = projects.iter().position(|p| *p == 1) else {
            unreachable!("the re-clustered list must contain project 1")
        };
        assert!(projects[..split].iter().all(|p| *p == 0));
        assert!(projects[split..].iter().all(|p| *p == 1));
    }

    // ── identity, the load-bearing invariant ─────────────────────────────

    #[test]
    fn selection_resolves_by_identity_not_by_index_after_a_re_sort() {
        let before = [combo(0, "/a"), combo(0, "/b"), combo(0, "/c")];
        let id = row_identity(&before[2]);
        // The list re-sorts under the cursor.
        let after = vec![combo(0, "/c"), combo(0, "/a"), combo(0, "/b")];
        assert_eq!(resolve_row_by_identity(&after, Some(&id), 2), Some(0));
    }

    #[test]
    fn a_vanished_row_resolves_to_none_never_to_the_stale_index() {
        let id = row_identity(&combo(0, "/gone"));
        let after = vec![combo(0, "/a"), combo(0, "/b")];
        assert_eq!(
            resolve_row_by_identity(&after, Some(&id), 0),
            None,
            "it must NOT activate whatever now sits at index 0"
        );
    }

    #[test]
    fn the_fallback_index_applies_only_without_an_identity() {
        let rows = vec![combo(0, "/a"), combo(0, "/b")];
        assert_eq!(resolve_row_by_identity(&rows, None, 1), Some(1));
        assert_eq!(resolve_row_by_identity(&rows, None, 9), None);
    }

    #[test]
    fn recent_and_combo_rows_share_one_session_identity() {
        let recent = PaletteRow::Recent {
            proj: 1,
            wt_path: "/w".into(),
            agent: Agent::Codex,
        };
        let combo = PaletteRow::Combo {
            proj: 1,
            wt_path: "/w".into(),
            agent: Agent::Codex,
        };
        assert_eq!(row_identity(&recent), row_identity(&combo));
    }

    #[test]
    fn the_agent_is_part_of_a_session_identity() {
        let a = PaletteRow::Recent {
            proj: 0,
            wt_path: "/w".into(),
            agent: Agent::Claude,
        };
        let b = PaletteRow::Recent {
            proj: 0,
            wt_path: "/w".into(),
            agent: Agent::Codex,
        };
        assert_ne!(row_identity(&a), row_identity(&b));
    }

    #[test]
    fn every_action_row_has_its_own_identity() {
        let rows = [
            PaletteRow::NewSession,
            PaletteRow::TerminalHome,
            PaletteRow::TerminalWt,
            PaletteRow::AddProject,
            PaletteRow::SwitchToSession,
            PaletteRow::Settings,
            PaletteRow::ReloadThemes,
        ];
        let ids: Vec<_> = rows.iter().map(row_identity).collect();
        for (i, a) in ids.iter().enumerate() {
            for b in &ids[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    // ── root order and the row builders ──────────────────────────────────

    #[test]
    fn root_project_order_puts_the_active_project_first() {
        assert_eq!(root_project_order(4, 2), vec![2, 0, 1, 3]);
        assert_eq!(root_project_order(0, 0), Vec::<usize>::new());
    }

    #[test]
    fn root_project_order_clamps_a_stale_active_index() {
        assert_eq!(root_project_order(2, 9), vec![1, 0]);
    }

    #[test]
    fn the_root_list_is_recents_first_then_the_action_block() {
        let recents = vec![(0, "/w".to_string(), Agent::Claude)];
        let rows = root_rows(&recents, &[], 1, 0);
        assert!(matches!(rows[0], PaletteRow::Recent { .. }));
        assert_eq!(rows[1], PaletteRow::NewSession);
        assert!(rows.contains(&PaletteRow::Settings));
        assert!(rows.contains(&PaletteRow::SwitchToSession));
    }

    #[test]
    fn the_root_list_caps_recents_at_max_root_recents() {
        let recents = vec![
            (0, "/w0".to_string(), Agent::Claude),
            (1, "/w1".to_string(), Agent::Claude),
            (2, "/w2".to_string(), Agent::Claude),
            (3, "/w3".to_string(), Agent::Claude),
            (4, "/w4".to_string(), Agent::Claude),
        ];
        let rows = root_rows(&recents, &[], 5, 0);
        let recent_count = rows
            .iter()
            .filter(|r| matches!(r, PaletteRow::Recent { .. }))
            .count();
        assert_eq!(recent_count, MAX_ROOT_RECENTS);
        assert_eq!(
            rows,
            vec![
                PaletteRow::Recent {
                    proj: 0,
                    wt_path: "/w0".to_string(),
                    agent: Agent::Claude,
                },
                PaletteRow::Recent {
                    proj: 1,
                    wt_path: "/w1".to_string(),
                    agent: Agent::Claude,
                },
                PaletteRow::Recent {
                    proj: 2,
                    wt_path: "/w2".to_string(),
                    agent: Agent::Claude,
                },
                PaletteRow::NewSession,
                PaletteRow::TerminalHome,
                PaletteRow::TerminalWt,
                PaletteRow::SwitchToSession,
                PaletteRow::AddProject,
                PaletteRow::Settings,
            ]
        );
    }

    #[test]
    fn the_root_list_falls_back_to_one_worktree_per_project_with_no_recents() {
        let fallback = vec![
            (0, "/p0".to_string(), Agent::Claude),
            (1, "/p1".to_string(), Agent::Claude),
        ];
        let rows = root_rows(&[], &fallback, 2, 1);
        // Active project first (`root_project_order`).
        assert_eq!(rows[0], combo(1, "/p1"));
        assert_eq!(rows[1], combo(0, "/p0"));
    }

    #[test]
    fn the_root_list_is_never_empty_even_with_no_projects() {
        let rows = root_rows(&[], &[], 0, 0);
        assert_eq!(rows[0], PaletteRow::NewSession);
    }

    #[test]
    fn typing_filters_and_ranks_every_combo() {
        let combos = vec![
            (
                0,
                "grove".to_string(),
                "/wt/main".to_string(),
                Agent::Claude,
            ),
            (
                0,
                "grove".to_string(),
                "/wt/feat".to_string(),
                Agent::Claude,
            ),
            (1, "other".to_string(), "/wt/x".to_string(), Agent::Claude),
        ];
        let rows = typed_rows("feat", &combos, &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], combo(0, "/wt/feat"));
    }

    #[test]
    fn typing_settings_surfaces_the_settings_rows() {
        let rows = typed_rows("theme", &[], &[]);
        assert!(rows.contains(&PaletteRow::Setting(SettingRow::Theme)));
    }

    #[test]
    fn an_empty_query_surfaces_no_settings_rows() {
        // Browse-all lists combos only; a bare query must not inject settings.
        let rows = typed_rows("", &[], &[]);
        assert!(!rows.iter().any(|r| matches!(r, PaletteRow::Setting(_))));
        assert!(!rows.contains(&PaletteRow::ReloadThemes));
        assert!(!rows.contains(&PaletteRow::AddProject));
    }

    #[test]
    fn typing_add_project_surfaces_the_add_project_row() {
        let rows = typed_rows("add project", &[], &[]);
        assert!(rows.contains(&PaletteRow::AddProject));
    }

    #[test]
    fn typing_a_prefix_of_add_project_still_surfaces_it() {
        let rows = typed_rows("add", &[], &[]);
        assert!(rows.contains(&PaletteRow::AddProject));
    }

    #[test]
    fn typing_settings_surfaces_the_settings_drill_in_opener() {
        let rows = typed_rows("settings", &[], &[]);
        assert!(rows.contains(&PaletteRow::Settings));
    }

    #[test]
    fn an_unrelated_query_does_not_surface_the_settings_drill_in_opener() {
        let rows = typed_rows("zzz", &[], &[]);
        assert!(!rows.contains(&PaletteRow::Settings));
    }

    // ── the drill-ins ────────────────────────────────────────────────────

    #[test]
    fn switch_rows_are_sessions_then_terminals() {
        assert_eq!(
            merge_switch_rows(&[3, 1], &[0, 2]),
            vec![
                SwitchRow::Session(3),
                SwitchRow::Session(1),
                SwitchRow::Terminal(0),
                SwitchRow::Terminal(2),
            ]
        );
    }

    #[test]
    fn switch_sessions_are_recent_first_with_the_active_one_last() {
        // 0 never focused, 1 focused twice ago, 2 is active, 3 focused last.
        let sessions = [(0, 0, false), (1, 4, false), (2, 9, true), (3, 7, false)];
        assert_eq!(order_switch_sessions(&sessions), vec![3, 1, 0, 2]);
        // Never-focused sessions keep insertion order among themselves.
        assert_eq!(
            order_switch_sessions(&[(5, 0, false), (2, 0, false)]),
            vec![5, 2]
        );
    }

    #[test]
    fn terminal_rows_match_the_label_or_the_constant_subtitle() {
        let labels = vec!["terminal 1".to_string(), "terminal 2".to_string()];
        assert_eq!(switch_terminal_rows(&labels, ""), vec![0, 1]);
        assert_eq!(switch_terminal_rows(&labels, "2"), vec![1]);
        assert_eq!(switch_terminal_rows(&labels, "home"), vec![0, 1]);
        assert!(switch_terminal_rows(&labels, "zzzz").is_empty());
    }

    #[test]
    fn agent_sel_for_falls_back_to_zero_for_an_absent_agent() {
        let available = [Agent::Claude, Agent::Codex];
        assert_eq!(agent_sel_for(&available, Agent::Codex), 1);
        assert_eq!(agent_sel_for(&available, Agent::OpenCode), 0);
    }

    #[test]
    fn update_now_is_hidden_for_an_unknown_install_method() {
        assert_eq!(
            update_available_actions(false),
            vec![
                UpdateAction::UpdateNow,
                UpdateAction::SkipVersion,
                UpdateAction::CopyUrl
            ]
        );
        assert_eq!(
            update_available_actions(true),
            vec![UpdateAction::SkipVersion, UpdateAction::CopyUrl]
        );
    }

    #[test]
    fn check_updates_expands_the_strip_only_when_one_is_available() {
        assert!(check_updates_opens_strip(true));
        assert!(!check_updates_opens_strip(false));
    }

    #[test]
    fn the_theme_mode_row_cycles_dark_light_system_dark() {
        assert_eq!(next_theme_mode(true, false), ThemeMode::Light);
        assert_eq!(next_theme_mode(false, false), ThemeMode::System);
        assert_eq!(next_theme_mode(true, true), ThemeMode::Dark);
        assert_eq!(next_theme_mode(false, true), ThemeMode::Dark);
    }

    // ── the settings table ───────────────────────────────────────────────

    #[test]
    fn setting_row_label_section_and_icon_are_total_and_nonempty() {
        for s in SettingRow::ALL {
            assert!(!s.label().is_empty());
            assert!(!s.section().is_empty());
            assert!(!s.icon_name().is_empty());
        }
        assert_eq!(SettingRow::Telemetry.label(), "Telemetry");
        assert_eq!(SettingRow::Telemetry.section(), "AGENTS / TERMINAL");
        assert_eq!(SettingRow::CheckUpdates.label(), "Check for updates");
        assert_eq!(SettingRow::CheckUpdates.section(), "UPDATES");
    }

    #[test]
    fn settings_row_keyword_matches_root_query() {
        assert!(fuzzy_match("settings", "settings", "", ""));
        assert!(fuzzy_match("set", "settings", "", ""));
        assert!(!fuzzy_match("zzz", "settings", "", ""));
    }

    #[test]
    fn exactly_three_settings_rows_toggle_in_place() {
        let toggles: Vec<_> = SettingRow::ALL
            .into_iter()
            .filter(|s| s.is_toggle())
            .collect();
        assert_eq!(
            toggles,
            vec![
                SettingRow::ProjectThemes,
                SettingRow::Telemetry,
                SettingRow::Chrome
            ]
        );
    }

    // ── the index-based scroll window (the gpui replacement) ─────────────

    #[test]
    fn the_window_follows_the_selection_down_and_up() {
        assert_eq!(scroll_offset_for(0, 5, 4, 20), 2);
        assert_eq!(scroll_offset_for(6, 3, 4, 20), 3);
        assert_eq!(scroll_offset_for(0, 2, 4, 20), 0, "already visible");
    }

    #[test]
    fn the_window_clamps_at_the_end_and_is_zero_when_degenerate() {
        assert_eq!(scroll_offset_for(0, 19, 4, 20), 16);
        assert_eq!(scroll_offset_for(9, 0, 4, 2), 0);
        assert_eq!(scroll_offset_for(0, 0, 0, 10), 0);
        assert_eq!(scroll_offset_for(0, 0, 4, 0), 0);
    }

    #[test]
    fn cycle_wraps_both_ways() {
        assert_eq!(cycle(0, -1, 3), 2);
        assert_eq!(cycle(2, 1, 3), 0);
        assert_eq!(cycle(0, 1, 0), 0);
    }
}
