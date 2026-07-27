//! Shared shape of the command palette's presentation state: `LauncherState`
//! and its nested pane/settings/row-actions structs, the palette's
//! `Msg`, and the internal `PaletteRow` list-item type. No behavior lives
//! here — see `palette.rs`/`settings.rs`/`theme_panes.rs`/`keys.rs` for the
//! `impl Grove` methods that drive this state, and `view.rs` for rendering
//! it.

use crate::gui::update::SettingRow;
use grove_core::agent::Agent;
use grove_core::storage::Project;
use grove_core::theme;

/// Live state for the command palette, when open. `Some` exactly when
/// `app.modal` is `Modal::SessionLauncher` — same idiom as
/// `Grove::add_project` / `Grove::scripts_editor`.
///
/// Two list states, plus the drill-ins below:
/// - root: `input` empty and `browse_all` false — recents + actions list.
/// - typing/browse-all: `input` non-empty OR `browse_all` true — every
///   project×worktree combo, fuzzy-filtered by `input` (unfiltered if empty
///   and `browse_all` is what got us here via "+ new session…").
#[derive(Clone)]
pub struct LauncherState {
    pub input: String,
    /// Selected row index into whatever list is currently rendered (root's
    /// combined recents+actions list, or the typing/browse-all list).
    pub selected: usize,
    /// Identity of the row at `selected`, captured whenever `selected` is
    /// written (see `PaletteRowIdentity`) — activation resolves by this,
    /// not by `selected` alone. `None` only defensively, for states this
    /// field predates having always been threaded through.
    pub selected_identity: Option<PaletteRowIdentity>,
    /// Set once the root "+ new session…" action row is activated: forces
    /// the unfiltered every-combo list even while `input` is empty.
    pub browse_all: bool,
    /// "Switch to session…" drill-in: `Some(selected)` lists every active
    /// session (index into `App::sessions`-derived display order = the
    /// selection cursor within that list). `None` outside this state.
    pub switch: Option<usize>,
    /// The `id` of the session at `switch`'s position, captured whenever
    /// `switch` is written — same principle as `selected_identity`,
    /// applied to this drill-in's own list (`Session::id` is the stable
    /// key here rather than `PaletteRowIdentity`, since a switch-drill-in
    /// row already *is* a session, with its own never-reused id).
    pub switch_identity: Option<u64>,
    /// Inline contextual actions revealed by Tab under a highlighted
    /// `Recent`/`Combo` row (root or typing/browse-all list). `None` when
    /// no action strip is open.
    pub row_actions: Option<RowActionsState>,
    /// "Settings…" drill-in: `Some` shows the scoped settings list in
    /// place of the root/typing list. Entered via the root "Settings…"
    /// action row (Enter or Tab); `None` outside this state.
    pub settings: Option<LauncherSettings>,
}

/// The command palette's inline contextual-action strip, revealed by Tab
/// under a highlighted `Recent`/`Combo` row. Identifies the row by
/// `(proj, wt_path, agent)` rather than a list index, so it stays valid even
/// if the rendered row list is rebuilt out from under it (e.g. `browse_all`
/// flips, or the query changes) — the strip is simply not found/collapsed
/// rather than silently acting on a different row. `agent` is part of the
/// identity because recent launches are deduped by `(project, wt_path,
/// agent)` (see `push_recent_launch`), so two Recent rows can share a
/// worktree with different agents. `action` is the selected action within
/// the strip (`0` = primary/"Launch session…", `1` = danger/"Delete
/// worktree").
#[derive(Clone)]
pub struct RowActionsState {
    pub proj: usize,
    pub wt_path: String,
    pub agent: grove_core::agent::Agent,
    pub action: usize,
    /// The agent icon bar hosted on the strip's "Launch session…" row
    /// (mock F): an index into `App::available_agents`, seeded from the
    /// row's own `agent` when the strip opens and walked by ←/→ while
    /// `action == 0`. ⏎ (or a click on one of the icon buttons) launches
    /// with this agent directly — the arrows only mean "agent" inside the
    /// strip, so the search caret keeps ←/→ everywhere else.
    pub agent_sel: usize,
}

/// Same "identify by content, not list position" principle as
/// `RowActionsState`, applied to `LauncherState::selected` — the main
/// palette's own selection cursor. Captured whenever `selected` is written
/// (`Grove::set_palette_selected`) and re-resolved against a freshly rebuilt
/// row list at activation time (`Grove::resolve_selected` /
/// `resolve_row_by_identity`) rather than trusting the raw index: a query
/// edit, a recency-driven re-sort, or the root's no-recents worktree fallback
/// swapping in between two keystrokes can't make Enter silently activate a
/// different row than the one the user was looking at.
///
/// `proj` is an index rather than the project's name: a project can only be
/// removed via its own confirmation modal (`Modal::RemoveProject`), and
/// `Modal` holds exactly one variant at a time, so a project can't be
/// removed out from under an *open* launcher — its project indices are
/// stable for the palette's whole lifetime.
#[derive(Clone, PartialEq, Debug)]
pub enum PaletteRowIdentity {
    Session {
        proj: usize,
        wt_path: String,
        agent: grove_core::agent::Agent,
    },
    NewSession,
    TerminalHome,
    TerminalWt,
    AddProject,
    SwitchToSession,
    Settings,
    Setting(crate::gui::update::SettingRow),
    ReloadThemes,
}

/// Which pane the palette's Settings drill-in is showing. `Root` is the
/// sectioned all-settings list; the others are one-level-deeper pickers for
/// enum-shaped settings, entered via `Grove::activate_setting`. Esc pops one
/// level back to `Root` (and `Root`'s own Esc closes the whole drill-in).
#[derive(Clone)]
pub enum SettingsPane {
    Root,
    /// Theme picker with live preview, mirroring `Modal::ThemePicker`:
    /// `original` restores the pre-entry theme when Esc backs out without
    /// applying. `kind` is which theme list is currently shown (System mode
    /// still lists the dark set, like the real picker's own tab). `follow_system`
    /// is a local draft of "follow system appearance" — toggled and previewed
    /// by the System segment, only written to `Store::theme_follow_system` on
    /// Enter, exactly like `Modal::ThemePicker::follow_system`.
    /// Duplicate/rename/delete/⌘N-new all moved out to `Modal::ThemeManager`
    /// (reached via the "Manage themes…" row / ⌘M below); this pane only
    /// browses and previews. ⌘E (open the swatch editor) stays wired here
    /// until `Modal::ThemeManager`'s own editor view lands.
    Theme {
        original: theme::Theme,
        kind: theme::ThemeKind,
        follow_system: bool,
    },
    Backend,
    Permissions,
    DefaultAgent,
    /// Project-scoped theme picker entered from a session row's actions strip.
    /// `preview` is the pane's whole state: Some = highlighted theme (live-
    /// previewed on that project's tiles only), None = "Use app theme".
    /// Nothing persists until Enter commits.
    ProjectTheme {
        proj: usize,
        kind: theme::ThemeKind,
        preview: Option<theme::Theme>,
    },
    // The in-app swatch editor moved to `Modal::ThemeManager`'s EDITOR
    // sub-view (`Grove::theme_manager_editor`) — no `SettingsPane` variant
    // for it anymore.
}

/// "Settings" drill-in state for the session-launcher palette: a `pane`
/// (root list or a one-level-deeper enum picker) plus a selection cursor into
/// whatever that pane currently renders.
#[derive(Clone)]
pub struct LauncherSettings {
    pub pane: SettingsPane,
    /// Selection cursor into the current pane's rendered list.
    pub selected: usize,
    /// Root pane only: the App-size row is in inline-edit mode (←/→ or the
    /// on-row stepper adjust zoom; ⏎ or Esc leaves the mode without popping
    /// out of the drill-in).
    pub resizing: bool,
    /// Root pane only: the update-available actions strip expanded under the
    /// Check-for-updates row — `Some(selected strip index)` while open. ⏎ on
    /// that row expands this instead of re-firing a check whenever an update
    /// is already known to be available; Esc closes just the strip, not the
    /// drill-in.
    pub update_actions: Option<usize>,
}

/// Live project-theme preview from an open Settings→ProjectTheme drill-in
/// (`LauncherState::settings`), if `project_name` is the project currently
/// being edited there. Extracted out of `App::project_theme_override` — that
/// method can't reach `Grove::launcher` itself (`App` is the domain layer and
/// never sees `Grove` state), so its caller (`Grove::pty`, in `view.rs`)
/// resolves this first and passes the result in. `Some(inner)` wins outright
/// over `App::project_theme_override`'s own persisted-pin lookup (`inner ==
/// None` means "preview the global theme", matching `project_use_default` in
/// `Modal::ThemePicker`).
pub fn project_theme_preview(
    launcher: &Option<LauncherState>,
    projects: &[Project],
    project_name: &str,
) -> Option<Option<theme::Theme>> {
    let settings = launcher.as_ref()?.settings.as_ref()?;
    let SettingsPane::ProjectTheme { proj, preview, .. } = &settings.pane else {
        return None;
    };
    if projects.get(*proj).map(|p| p.name.as_str()) == Some(project_name) {
        Some(preview.clone())
    } else {
        None
    }
}
/// One row of the command palette's list, in display order.
#[derive(Clone)]
pub(super) enum PaletteRow {
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
    /// ACTIONS row: Enter or Tab opens the "switch to session" drill-in
    /// (`Modal::SessionLauncher::switch`).
    SwitchToSession,
    /// ACTIONS row: Enter or Tab opens the Settings drill-in
    /// (`Modal::SessionLauncher::settings`).
    Settings,
    /// A direct settings match surfaced while typing at root (not
    /// `browse_all`) — name, current value, and section all searchable. See
    /// `Grove::activate_setting`: toggles flip in place here without opening
    /// the drill-in; enum rows are phase-1 no-ops.
    Setting(SettingRow),
    /// ACTIONS row, keyword-only (never shown at bare root — see
    /// `palette_rows`): re-reads `themes.json` via `theme::load_custom` and
    /// surfaces any skipped-entry errors (mock E2). Stateless, so unlike
    /// `Setting` there's no value to display or toggle.
    ReloadThemes,
}

/// Every message the command palette can emit. Dispatched from a nested
/// match in `Grove::update`'s `Msg::SessionLauncher` arm rather than a
/// free-fn `update`/`handle_key` here — see this module's doc comment for
/// why the palette can't follow the `add_project`/`scripts_editor`/
/// `theme_manager_editor` shape.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Open the palette (pill click or Cmd/Ctrl+N while the grid is open).
    Open,
    /// Live edit of the palette's search field. Resets `selected` to 0.
    InputChanged(String),
    /// `text_input`'s dedicated ⌘V callback (`on_paste`) for the palette's
    /// search field — fires instead of `InputChanged` for an actual paste,
    /// so it can skip that message's `global_mods` spurious-edit guard
    /// (which would otherwise drop the paste too, since the chord's modifier
    /// is still held when the content arrives).
    InputPasted(String),
    /// Activate (launch/act on) the row at this index in the currently
    /// rendered root/typing/browse-all list; driven by both row click and the
    /// Enter/mod+digit keyboard paths.
    Activate(usize),
    /// Click an agent icon button in the row-actions strip's "Launch
    /// session…" bar (index into `App::available_agents`): selects it and
    /// launches the strip's row with it immediately, same as ⏎ would.
    RowActionAgentLaunch(usize),
    /// Click a session row by index into `App::sessions`, in the "switch to
    /// session" drill-in: switches focus to it and closes the palette.
    SwitchSessionPick(usize),
    /// Click one of the two inline contextual-action rows revealed by Tab
    /// under a highlighted `Recent`/`Combo` row (`0` = "Launch session…",
    /// `1` = "Delete worktree").
    RowActionPick(usize),
    /// Click a row by index into `Grove::settings_rows_filtered`'s current
    /// list, in the Settings drill-in: distinct from `Activate` because the
    /// drill-in's row list is unrelated to `palette_rows`'s root/typing
    /// list, so the same index would mean a different row. Selects the row,
    /// then applies the same activation Enter would.
    SettingActivate(usize),
    /// Theme sub-pane: click (or the equivalent of hover) on list row `i` —
    /// selects it and live-previews the theme, same as ↑↓. Distinct from
    /// `SettingsPaneActivate` because the Theme pane defers the actual
    /// persist to a separate ⏎ (`Msg::KeyPress`), not the click.
    ThemePaneSelect(usize),
    /// Theme sub-pane: "Dark"/"Light" segment click — switches which kind's
    /// theme list is shown and opts out of "follow system" (mirrors picking
    /// a concrete theme in `Modal::ThemePicker`). Two unit variants rather
    /// than one carrying `theme::ThemeKind` since `Msg` derives `Debug` and
    /// that type doesn't.
    ThemePaneDark,
    ThemePaneLight,
    /// Theme sub-pane: "System" segment click — previews the resolved system
    /// theme and marks "follow system" as a local draft, persisted on ⏎
    /// (mirrors `Modal::ThemePicker`'s follow-system checkbox).
    ThemePaneSystem,
    /// Backend/Permissions/DefaultAgent sub-pane: click on row `i` — selects
    /// and immediately commits (mirrors ⏎). Unlike the Theme pane, these
    /// panes have no live-preview step to defer.
    SettingsPaneActivate(usize),
    /// Click action `i` in the update-available strip expanded under the
    /// Settings drill-in's Check-for-updates row: selects it and runs it
    /// (mirrors ⏎ there). Handled at the parent (`Grove::update`'s
    /// `Msg::SessionLauncher` arm), not by any free fn here: it needs
    /// `Grove`-only upgrade state (`upgrade`/`upgrade_method`) and
    /// recursively dispatches `Msg::Upgrade(UpgradeMsg::StartUpdate)`/`SkipVersion`/`CopyReleaseUrl`
    /// through `Grove::update`, so promoting that state onto `App` (the only
    /// alternative) would be the wrong layering for what is purely
    /// per-window UI state.
    UpdateActionPick(usize),
}
