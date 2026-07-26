use iced::keyboard::{key::Named, Key, Modifiers};
use std::time::Duration;

/// The platform's global-shortcut modifier: Cmd on macOS (matching the Cmd+C /
/// Cmd+V pair), Ctrl+Shift elsewhere (matching Ctrl+Shift+C / Ctrl+Shift+V, so
/// plain Ctrl chords stay available to the PTY).
pub(in crate::gui) fn global_mods(mods: Modifiers) -> bool {
    #[cfg(target_os = "macos")]
    return mods.logo() && !mods.control();
    #[cfg(not(target_os = "macos"))]
    return mods.control() && mods.shift();
}

/// Modifier for "new session in current worktree": Cmd+Alt (mac) / Ctrl+Alt
/// (elsewhere), independent of [`global_mods`] (which already requires Shift
/// on non-mac and so can't be reused as a base for an Alt chord there).
fn new_session_in_worktree_mods(mods: Modifiers) -> bool {
    #[cfg(target_os = "macos")]
    return mods.logo() && mods.alt() && !mods.control();
    #[cfg(not(target_os = "macos"))]
    return mods.control() && mods.alt() && !mods.shift();
}

/// Whether `mods` carries the grid tile-swap modifier on top of
/// [`global_mods`]. On mac this is Alt *or* Shift: Cmd+Opt+H collides with the
/// OS-level "Hide Others" shortcut, so Shift is accepted as an equivalent
/// swap modifier there (Cmd+Shift+h/j/k/l/arrows), and is what's displayed.
/// Cmd+Alt keeps working too, since some layouts/users already rely on it.
/// On non-mac, `global_mods` already requires Shift as part of its base
/// chord, so only Alt distinguishes swap from move there.
fn grid_swap_mods(mods: Modifiers) -> bool {
    #[cfg(target_os = "macos")]
    return mods.alt() || mods.shift();
    #[cfg(not(target_os = "macos"))]
    return mods.alt();
}

/// Human-readable label for the global-shortcut modifier, matching
/// [`global_mods`]. Shown in the status-bar chip and the shortcut overlay so the
/// displayed text can't drift from the actual chord.
pub(crate) fn platform_mod_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl+shift"
    }
}

/// App-level actions reachable from the global keyboard layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GlobalShortcut {
    NewSession,
    NewSessionInWorktree,
    Settings,
    ToggleZen,
    ToggleGrid,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    NextSession,
    PrevSession,
    SelectSession(usize),
    ShortcutOverlay,
    CloseFocusedSession,
    /// Spawn a new home terminal and focus it.
    NewHomeTerminal,
    /// Select the first session currently waiting for input, in tree order.
    JumpToWaitingSession,
    /// Move keyboard focus between grid tiles by `(dx, dy)`. Grid screen only.
    GridMove(i32, i32),
    /// Swap the focused tile with its neighbor by `(dx, dy)`. Grid screen only.
    GridSwap(i32, i32),
    /// Scroll the focused session by half a page (`true` = up).
    ScrollHalfPage(bool),
    /// Open the command palette straight into the "switch to session"
    /// drill-in. Zen-only (a no-op outside zen) — see `PaletteRow::SwitchToSession`.
    SwitchSession,
}

/// Coarse "which screen am I on" model, derived from existing UI flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Screen {
    Grid,
    Workspace,
    Zen,
}

impl Screen {
    /// Section header label used in the overlay when >1 scope is visible.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Screen::Grid => "grid",
            Screen::Workspace => "workspace",
            Screen::Zen => "zen",
        }
    }
}

/// Where a shortcut applies. A shortcut may list several scopes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    Global,
    Screen(Screen),
}

/// One row of the shortcut registry — single source of truth for both
/// `match_global_shortcut` (behavior) and `shortcut_overlay_modal` (display).
pub(crate) struct ShortcutDef {
    /// `None` for the display-only `1–9` row (matcher handles it dynamically).
    pub(crate) action: Option<GlobalShortcut>,
    /// Key chars matched against iced's modifier-independent `key`. Empty for
    /// the display-only row. `Enter` is matched separately (see the matcher).
    pub(crate) triggers: &'static [&'static str],
    /// Key label shown in the overlay; the platform modifier is prepended at
    /// render time (e.g. `"n"` -> `"cmd+n"`).
    pub(crate) display_keys: &'static str,
    pub(crate) description: &'static str,
    pub(crate) scopes: &'static [Scope],
    /// When true, this shortcut layers Alt on top of the platform's global
    /// modifier (e.g. Cmd+Alt+N / Ctrl+Alt+N) rather than using the plain
    /// platform modifier. Rendered with an "+alt+" infix by the overlay.
    pub(crate) requires_alt: bool,
    /// When true, `display_keys` is the complete chord text and the overlay
    /// renders it verbatim instead of prepending the platform modifier. Used
    /// by the one shortcut that is the same literal chord on every platform
    /// (`Ctrl+Shift+Arrow`, unlike `mod`'s Cmd-on-mac / Ctrl+Shift-elsewhere).
    pub(crate) literal: bool,
}

const G: &[Scope] = &[Scope::Global];

/// Single source of truth for behavioral matching and overlay display. Order
/// matches the overlay's reading order. Most entries are `Global`; a few are
/// scoped to a single screen (see each row's `scopes`).
pub(crate) const SHORTCUTS: &[ShortcutDef] = &[
    ShortcutDef {
        action: Some(GlobalShortcut::NewSession),
        triggers: &["p", "P"],
        display_keys: "p",
        description: "New session",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::NewSessionInWorktree),
        triggers: &["n", "N"],
        display_keys: "n",
        description: "New session in current worktree",
        scopes: G,
        requires_alt: true,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::SwitchSession),
        triggers: &["s", "S"],
        display_keys: "s",
        description: "Switch to session",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::NextSession),
        triggers: &["j", "J"],
        display_keys: "j",
        description: "Next session",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::PrevSession),
        triggers: &["k", "K"],
        display_keys: "k",
        description: "Previous session",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    // Display-only: the matcher handles 1–9 dynamically (see match_global_shortcut).
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: "1–9",
        description: "Select nth session",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ToggleGrid),
        triggers: &["g", "G"],
        display_keys: "g",
        description: "Toggle grid view",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ToggleZen),
        triggers: &[],
        display_keys: "enter",
        description: "Toggle zen mode",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::Settings),
        triggers: &[","],
        display_keys: ",",
        description: "Settings",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ZoomIn),
        triggers: &["=", "+"],
        display_keys: "=",
        description: "Zoom in",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ZoomOut),
        triggers: &["-", "_"],
        display_keys: "-",
        description: "Zoom out",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ZoomReset),
        triggers: &["0"],
        display_keys: "0",
        description: "Reset zoom",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ShortcutOverlay),
        triggers: &["/", "?"],
        display_keys: "/",
        description: "This overlay",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::CloseFocusedSession),
        triggers: &["w", "W"],
        display_keys: "w",
        description: "close focused session / terminal",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::NewHomeTerminal),
        triggers: &["t", "T"],
        display_keys: "t",
        description: "New home terminal",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::JumpToWaitingSession),
        triggers: &["'"],
        display_keys: "'",
        description: "Jump to session needing you",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    // Display-only: matched by `keyboard_scroll_intent` in `handle_key`, not
    // `match_global_shortcut` — plain PageUp/PageDown/Home/End (no Shift) must
    // fall through to the PTY, so these live outside the registry lookup.
    // Applies on every screen: `focused_session_mut()` resolves the grid's
    // focused tile too.
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: "shift+pgup/pgdn",
        description: "Scroll session by page",
        scopes: G,
        requires_alt: false,
        literal: true,
    },
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: "shift+home/end",
        description: "Scroll to top / bottom",
        scopes: G,
        requires_alt: false,
        literal: true,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ScrollHalfPage(true)),
        triggers: &["u", "U"],
        display_keys: "u",
        description: "Scroll half page up",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ScrollHalfPage(false)),
        triggers: &["d", "D"],
        display_keys: "d",
        description: "Scroll half page down",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    // Display-only: `match_global_shortcut` handles both of these rows ahead
    // of the registry lookup (dynamic dx/dy per key, `grid_swap_mods` picks
    // move vs. swap), scoped to Screen::Grid by hand there — keep the three
    // in sync. The swap row's `display_keys` differs per platform: mac shows
    // the Shift chord (Cmd+Opt collides with the OS "Hide Others" shortcut on
    // H), non-mac keeps Alt (Cmd+Alt still works on mac too, just isn't the
    // one advertised).
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: "h j k l / ←↓↑→",
        description: "Move focus in grid",
        scopes: &[Scope::Screen(Screen::Grid)],
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: if cfg!(target_os = "macos") {
            "shift+h j k l / ←↓↑→"
        } else {
            "alt+h j k l / ←↓↑→"
        },
        description: "Move tile in grid",
        scopes: &[Scope::Screen(Screen::Grid)],
        // On mac this row displays as `{platform_mod_label()}+shift+...` (no
        // Alt infix — see `shortcut_overlay_modal`'s `key_label`); non-mac
        // keeps the Alt infix as before.
        requires_alt: !cfg!(target_os = "macos"),
        literal: false,
    },
    // Display-only: matched by `term_panel_resize_delta`, not
    // `match_global_shortcut` — closing the panel must fall through to the
    // PTY, which a registry-matched shortcut never does (see the guard's
    // comment in `handle_key`). Listed here purely so it's scoped and
    // discoverable in the `mod+/` overlay; `scopes` here must track
    // `term_panel_resize_delta`'s `Screen::Workspace` check by hand.
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: "ctrl+shift+←/→",
        description: "Resize terminal panel",
        scopes: &[Scope::Screen(Screen::Workspace)],
        requires_alt: false,
        literal: true,
    },
];

/// Derive the coarse screen from UI flags. Zen wins over grid: while chrome is
/// hidden the user is in zen regardless of `grid_view`.
pub(crate) fn screen_from_flags(chrome_visible: bool, grid_view: bool) -> Screen {
    if !chrome_visible {
        Screen::Zen
    } else if grid_view {
        Screen::Grid
    } else {
        Screen::Workspace
    }
}

/// True if a shortcut whose registry row lists `scopes` may fire on `screen`:
/// always for `Global`, otherwise only on its matching `Screen(screen)` entry.
/// Shared by the matcher (behavior) and `shortcut_overlay_modal` (display) so
/// the two can never disagree about what's visible/active on a given screen.
pub(crate) fn scope_allows(scopes: &[Scope], screen: Screen) -> bool {
    scopes
        .iter()
        .any(|s| matches!(s, Scope::Global) || *s == Scope::Screen(screen))
}

/// Map a key event to a global shortcut, or `None` if the chord doesn't match
/// or its registry row is out of scope on `screen` — callers must fall
/// through to the PTY on `None` rather than treat it as consumed. Matches
/// iced's modifier-independent `key`, so Shift in the non-mac Ctrl+Shift
/// chords doesn't change the character being compared.
pub(super) fn match_global_shortcut(
    key: &Key,
    mods: Modifiers,
    screen: Screen,
) -> Option<GlobalShortcut> {
    // Checked ahead of `global_mods`: on non-mac, `global_mods` already
    // requires Shift, so Ctrl+Alt+N (no Shift) would never reach it. This
    // chord is Cmd+Alt+N (mac) / Ctrl+Alt+N (elsewhere), independent of the
    // platform's base global-shortcut modifier.
    //
    // On mac this early check is technically redundant now that the registry
    // lookup below honors `requires_alt`: `global_mods` there is just
    // `logo() && !control()`, which Cmd+Alt+N already satisfies, so the
    // registry `.find()` alone would resolve it to `NewSessionInWorktree`.
    // It still has to stay because non-mac needs it — `global_mods` there
    // requires Shift, which Ctrl+Alt+N (no Shift) never has, so non-mac
    // can't reach the registry lookup at all for this chord.
    if new_session_in_worktree_mods(mods) {
        if let Key::Character(s) = key {
            if s.eq_ignore_ascii_case("n") {
                // Global today, but scope-checked like everything else below
                // rather than bypassing it, so a future rescoping can't be
                // missed here.
                let scopes = SHORTCUTS
                    .iter()
                    .find(|d| d.action == Some(GlobalShortcut::NewSessionInWorktree))
                    .map_or(G, |d| d.scopes);
                if scope_allows(scopes, screen) {
                    return Some(GlobalShortcut::NewSessionInWorktree);
                }
            }
        }
    }
    if !global_mods(mods) {
        return None;
    }
    // Grid-only directional focus move. Checked ahead of the registry lookup
    // so it shadows the global `mod+j`/`mod+k` NextSession/PrevSession
    // bindings on this screen only — those two rows, and every other screen,
    // are untouched.
    if screen == Screen::Grid {
        let dir = match key {
            Key::Character(s) => match s.as_str() {
                "h" | "H" => Some((-1, 0)),
                "l" | "L" => Some((1, 0)),
                "k" | "K" => Some((0, -1)),
                "j" | "J" => Some((0, 1)),
                _ => None,
            },
            Key::Named(Named::ArrowLeft) => Some((-1, 0)),
            Key::Named(Named::ArrowRight) => Some((1, 0)),
            Key::Named(Named::ArrowUp) => Some((0, -1)),
            Key::Named(Named::ArrowDown) => Some((0, 1)),
            _ => None,
        };
        if let Some((dx, dy)) = dir {
            return Some(if grid_swap_mods(mods) {
                GlobalShortcut::GridSwap(dx, dy)
            } else {
                GlobalShortcut::GridMove(dx, dy)
            });
        }
    }
    match key {
        // Not registry-`.find()`-driven like the char rows below (it's a
        // `Key::Named`, not a `Key::Character`), but still scope-checked
        // against its row (`G` today) for the same reason as the Alt chord
        // above.
        Key::Named(Named::Enter) => scope_allows(G, screen).then_some(GlobalShortcut::ToggleZen),
        Key::Character(s) => {
            let s = s.as_str();
            // Registry-driven character shortcuts. `requires_alt` must be part
            // of the match, not just display metadata: `NewSession` and
            // `NewSessionInWorktree` share `triggers` and only differ by Alt,
            // so without this the first row in array order would always win
            // (Bug 7) — swapping the two rows would silently swap their
            // meaning with no compiler error and no failing test.
            if let Some(def) = SHORTCUTS.iter().find(|d| {
                d.action.is_some() && d.triggers.contains(&s) && d.requires_alt == mods.alt()
            }) {
                return def.action.filter(|_| scope_allows(def.scopes, screen));
            }
            // SelectNth stays special-cased: dynamic n, display-only in
            // registry (`G` — scope-checked for the same reason as above).
            s.parse::<usize>()
                .ok()
                .filter(|n| (1..=9).contains(n) && scope_allows(G, screen))
                .map(|n| GlobalShortcut::SelectSession(n - 1))
        }
        _ => None,
    }
}

/// Result of [`close_focused_session_decision`]: what `CloseFocusedSession`
/// should do given the current confirm-to-kill state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CloseFocusedDecision {
    /// No session is focused.
    NoOp,
    /// First press: arm the confirm-to-kill state for this session.
    Request(usize),
    /// Second press while armed for this session: actually kill it.
    Kill(usize),
}

/// Pure decision logic for `GlobalShortcut::CloseFocusedSession`, mirroring the
/// close-button toggle on both the grid tile (`grid_tile`) and the sidebar row
/// (`session_row`). Kept as a free function so it's testable without
/// constructing a full `Grove`. `target` is whichever session the current
/// screen considers focused; the caller resolves it.
pub(super) fn close_focused_session_decision(
    target: Option<usize>,
    pending_kill: Option<usize>,
) -> CloseFocusedDecision {
    match target {
        Some(si) if pending_kill == Some(si) => CloseFocusedDecision::Kill(si),
        Some(si) => CloseFocusedDecision::Request(si),
        None => CloseFocusedDecision::NoOp,
    }
}

/// New value for `grid_focused` after the active session changes, given
/// whether the grid is showing or will show again once zen exits. `None`
/// means "leave `grid_focused` alone" — outside the grid (and not zenned in
/// from it) there's no tile to track. Kept as a free function so it's
/// testable without constructing a full `Grove` (Bug 5).
pub(super) fn should_sync_grid_focus(grid_view: bool, grid_view_before_zen: bool) -> bool {
    grid_view || grid_view_before_zen
}

/// `tile_order` position to refocus after killing the focused tile, given the
/// killed tile's position before removal (`killed_pos`) and the tile count
/// after removal (`len`). The killed slot is filled by whatever shifted into
/// it, so we refocus that same slot; if the killed tile was last, clamp to
/// the new last slot instead. Kept as a free function so it's testable
/// without constructing a full `Grove`.
pub(super) fn grid_focus_after_kill(killed_pos: Option<usize>, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(killed_pos.unwrap_or(0).min(len - 1))
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
pub(crate) fn grid_neighbor(i: usize, n: usize, dx: i32, dy: i32) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let (cols, _rows) = crate::gui::metrics::grid_layout(n);
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

/// Duration of the draw-only tile-slide animation triggered by a grid
/// reorder (drag or keyboard swap).
pub(crate) const GRID_SLIDE: Duration = Duration::from_millis(150);

/// Timing curve of the tile slide. `lilt` (iced's animation crate) exposes
/// its easings as plain `fn(f32) -> f32`, so the curve is a one-word swap —
/// see `iced::animation::Easing` for the full set, incl. `Custom` for a
/// hand-rolled cubic-bezier if a named curve ever stops being enough.
const GRID_SLIDE_EASING: iced::animation::Easing = iced::animation::Easing::EaseOutCubic;

/// Eased progress `[0, 1]` for a `GRID_SLIDE`-duration animation that started
/// at `start`, evaluated at `now`.
pub(crate) fn slide_progress(start: std::time::Instant, now: std::time::Instant) -> f32 {
    let elapsed = now.saturating_duration_since(start);
    if elapsed >= GRID_SLIDE {
        return 1.0;
    }
    GRID_SLIDE_EASING.value(elapsed.as_secs_f32() / GRID_SLIDE.as_secs_f32())
}

#[cfg(test)]
mod tests {
    use super::{match_global_shortcut, slide_progress, GlobalShortcut, Screen, GRID_SLIDE};
    use iced::keyboard::{key::Named, Key, Modifiers};
    use smol_str::SmolStr;

    /// The platform's global modifier: Cmd on macOS, Ctrl+Shift elsewhere.
    fn gmods() -> Modifiers {
        #[cfg(target_os = "macos")]
        return Modifiers::LOGO;
        #[cfg(not(target_os = "macos"))]
        return Modifiers::CTRL | Modifiers::SHIFT;
    }

    fn ch(s: &str) -> Key {
        Key::Character(SmolStr::new(s))
    }

    #[test]
    fn global_shortcuts_map_with_platform_modifier() {
        // All of these are `Global`-scoped, so Workspace is an arbitrary pick —
        // `screen_scoped_shortcuts_respect_scopes` below covers the Grid-only row.
        use GlobalShortcut::*;
        let screen = Screen::Workspace;
        assert_eq!(
            match_global_shortcut(&ch("p"), gmods(), screen),
            Some(NewSession)
        );
        assert_eq!(
            match_global_shortcut(&ch(","), gmods(), screen),
            Some(Settings)
        );
        assert_eq!(
            match_global_shortcut(&ch("g"), gmods(), screen),
            Some(ToggleGrid)
        );
        assert_eq!(
            match_global_shortcut(&ch("j"), gmods(), screen),
            Some(NextSession)
        );
        assert_eq!(
            match_global_shortcut(&ch("k"), gmods(), screen),
            Some(PrevSession)
        );
        assert_eq!(
            match_global_shortcut(&ch("="), gmods(), screen),
            Some(ZoomIn)
        );
        assert_eq!(
            match_global_shortcut(&ch("-"), gmods(), screen),
            Some(ZoomOut)
        );
        assert_eq!(
            match_global_shortcut(&ch("0"), gmods(), screen),
            Some(ZoomReset)
        );
        assert_eq!(
            match_global_shortcut(&ch("3"), gmods(), screen),
            Some(SelectSession(2))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::Enter), gmods(), screen),
            Some(ToggleZen)
        );
        assert_eq!(
            match_global_shortcut(&ch("/"), gmods(), screen),
            Some(ShortcutOverlay)
        );
        // Registry-driven aliases.
        assert_eq!(
            match_global_shortcut(&ch("+"), gmods(), screen),
            Some(ZoomIn)
        );
        assert_eq!(
            match_global_shortcut(&ch("_"), gmods(), screen),
            Some(ZoomOut)
        );
        assert_eq!(
            match_global_shortcut(&ch("?"), gmods(), screen),
            Some(ShortcutOverlay)
        );
    }

    /// `mod+u`/`mod+d` scroll the focused session by half a page; plain
    /// "u"/"d" (no platform modifier) must not be treated as shortcuts, so
    /// they keep reaching the PTY (e.g. Ctrl+U/D line-kill / EOF).
    #[test]
    fn scroll_half_page_requires_platform_modifier() {
        use GlobalShortcut::ScrollHalfPage;
        let screen = Screen::Workspace;
        assert_eq!(
            match_global_shortcut(&ch("u"), gmods(), screen),
            Some(ScrollHalfPage(true))
        );
        assert_eq!(
            match_global_shortcut(&ch("d"), gmods(), screen),
            Some(ScrollHalfPage(false))
        );
        assert_eq!(
            match_global_shortcut(&ch("u"), Modifiers::empty(), screen),
            None
        );
        assert_eq!(
            match_global_shortcut(&ch("d"), Modifiers::empty(), screen),
            None
        );
    }

    /// `mod+w` closes the focused session on every screen: the grid tile in
    /// Grid, the active session's sidebar row otherwise. It must never fall
    /// through to the PTY, where `key_to_bytes` would turn Ctrl+Shift+W into
    /// `0x17` (readline delete-word) on Linux and a literal `w` on macOS.
    #[test]
    fn close_focused_session_matches_on_every_screen() {
        use GlobalShortcut::CloseFocusedSession;
        for screen in [Screen::Grid, Screen::Workspace, Screen::Zen] {
            assert_eq!(
                match_global_shortcut(&ch("w"), gmods(), screen),
                Some(CloseFocusedSession)
            );
            assert_eq!(
                match_global_shortcut(&ch("W"), gmods(), screen),
                Some(CloseFocusedSession)
            );
        }
    }

    /// The real "new session in worktree" chord: Cmd+Alt (mac) / Ctrl+Alt
    /// (elsewhere) — independent of `gmods()`, which on non-mac already
    /// includes Shift and would mask a regression back to requiring it.
    fn alt_mods() -> Modifiers {
        #[cfg(target_os = "macos")]
        return Modifiers::LOGO | Modifiers::ALT;
        #[cfg(not(target_os = "macos"))]
        return Modifiers::CTRL | Modifiers::ALT;
    }

    #[test]
    fn alt_n_maps_to_new_session_in_worktree() {
        use GlobalShortcut::*;
        let alt = alt_mods();
        let screen = Screen::Workspace;
        assert_eq!(
            match_global_shortcut(&ch("n"), alt, screen),
            Some(NewSessionInWorktree)
        );
        assert_eq!(
            match_global_shortcut(&ch("N"), alt, screen),
            Some(NewSessionInWorktree)
        );
        // Plain platform modifier (no Alt) on `n` is no longer a shortcut —
        // NewSession moved to `p`; only the alt-chord claims `n` now.
        assert_eq!(match_global_shortcut(&ch("n"), gmods(), screen), None);
        assert_eq!(
            match_global_shortcut(&ch("p"), gmods(), screen),
            Some(NewSession)
        );
        // Alt held on an unclaimed key is *not* a shortcut on either platform:
        // the registry now requires an exact `requires_alt` match (Bug 7's
        // fix), and `ToggleGrid`'s row has `requires_alt: false`, so holding
        // Alt no longer falls through to it even on mac, where `alt_mods()`
        // (Cmd+Alt) still satisfies `global_mods` (Cmd, no Ctrl). On non-mac,
        // `alt_mods()` is Ctrl+Alt with no Shift, which `global_mods`
        // (Ctrl+Shift) rejects outright, so it never even reaches the registry.
        assert_eq!(match_global_shortcut(&ch("g"), alt, screen), None);
    }

    /// Pins Bug 7's fix directly, non-mac only: a chord that carries Shift (so
    /// `new_session_in_worktree_mods`'s `!shift()` fails and the early-check
    /// never fires) but still satisfies `global_mods` (Ctrl+Shift) and holds
    /// Alt must still resolve through the registry alone, proving the registry
    /// lookup — not just the early-check — is `requires_alt`-correct.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn registry_lookup_resolves_worktree_variant_when_early_check_is_bypassed() {
        use GlobalShortcut::*;
        let mods = Modifiers::CTRL | Modifiers::SHIFT | Modifiers::ALT;
        assert_eq!(
            match_global_shortcut(&ch("n"), mods, Screen::Workspace),
            Some(NewSessionInWorktree)
        );
    }

    #[test]
    fn screen_zen_wins_over_grid() {
        use super::screen_from_flags;
        assert_eq!(screen_from_flags(false, true), Screen::Zen);
        assert_eq!(screen_from_flags(false, false), Screen::Zen);
        assert_eq!(screen_from_flags(true, true), Screen::Grid);
        assert_eq!(screen_from_flags(true, false), Screen::Workspace);
    }

    #[test]
    fn grid_move_shortcuts_scoped_to_grid_screen() {
        use GlobalShortcut::*;
        let screen = Screen::Grid;
        assert_eq!(
            match_global_shortcut(&ch("h"), gmods(), screen),
            Some(GridMove(-1, 0))
        );
        assert_eq!(
            match_global_shortcut(&ch("l"), gmods(), screen),
            Some(GridMove(1, 0))
        );
        assert_eq!(
            match_global_shortcut(&ch("k"), gmods(), screen),
            Some(GridMove(0, -1))
        );
        assert_eq!(
            match_global_shortcut(&ch("j"), gmods(), screen),
            Some(GridMove(0, 1))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowLeft), gmods(), screen),
            Some(GridMove(-1, 0))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowRight), gmods(), screen),
            Some(GridMove(1, 0))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowUp), gmods(), screen),
            Some(GridMove(0, -1))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowDown), gmods(), screen),
            Some(GridMove(0, 1))
        );
        // Elsewhere, mod+j/mod+k are still NextSession/PrevSession — the Grid
        // shadow must not leak to other screens.
        assert_eq!(
            match_global_shortcut(&ch("j"), gmods(), Screen::Workspace),
            Some(NextSession)
        );
        assert_eq!(
            match_global_shortcut(&ch("k"), gmods(), Screen::Workspace),
            Some(PrevSession)
        );
    }

    #[test]
    fn grid_swap_shortcuts_scoped_to_grid_screen() {
        use GlobalShortcut::*;
        let screen = Screen::Grid;
        let alt = gmods() | Modifiers::ALT;
        assert_eq!(
            match_global_shortcut(&ch("h"), alt, screen),
            Some(GridSwap(-1, 0))
        );
        assert_eq!(
            match_global_shortcut(&ch("l"), alt, screen),
            Some(GridSwap(1, 0))
        );
        assert_eq!(
            match_global_shortcut(&ch("k"), alt, screen),
            Some(GridSwap(0, -1))
        );
        assert_eq!(
            match_global_shortcut(&ch("j"), alt, screen),
            Some(GridSwap(0, 1))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowLeft), alt, screen),
            Some(GridSwap(-1, 0))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowRight), alt, screen),
            Some(GridSwap(1, 0))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowUp), alt, screen),
            Some(GridSwap(0, -1))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowDown), alt, screen),
            Some(GridSwap(0, 1))
        );
        // Without Alt, the same keys still resolve to GridMove — no regression
        // from layering the Alt dispatch on top.
        assert_eq!(
            match_global_shortcut(&ch("h"), gmods(), screen),
            Some(GridMove(-1, 0))
        );
        assert_eq!(
            match_global_shortcut(&ch("j"), gmods(), screen),
            Some(GridMove(0, 1))
        );
        assert_eq!(
            match_global_shortcut(&ch("k"), gmods(), screen),
            Some(GridMove(0, -1))
        );
        assert_eq!(
            match_global_shortcut(&ch("l"), gmods(), screen),
            Some(GridMove(1, 0))
        );
        // Alt+h/j/k/l elsewhere is not GridSwap — the Grid-only shadow must
        // not leak to other screens (mirrors the GridMove scoping check above).
        assert_eq!(
            match_global_shortcut(&ch("h"), alt, Screen::Workspace),
            None
        );
        assert_eq!(
            match_global_shortcut(&ch("j"), alt, Screen::Workspace),
            None
        );
        assert_eq!(
            match_global_shortcut(&ch("k"), alt, Screen::Workspace),
            None
        );
        assert_eq!(
            match_global_shortcut(&ch("l"), alt, Screen::Workspace),
            None
        );
    }

    /// Mac-only: Cmd+Opt+H collides with the OS "Hide Others" shortcut, so
    /// Cmd+Shift is also accepted (and is what's displayed) for the swap
    /// chord there. Cmd+Alt must keep working too (checked above by the
    /// shared `alt` chord in `grid_swap_shortcuts_scoped_to_grid_screen`).
    #[test]
    #[cfg(target_os = "macos")]
    fn grid_swap_shortcuts_accept_shift_on_mac() {
        use GlobalShortcut::*;
        let screen = Screen::Grid;
        let shift = gmods() | Modifiers::SHIFT;
        assert_eq!(
            match_global_shortcut(&ch("h"), shift, screen),
            Some(GridSwap(-1, 0))
        );
        assert_eq!(
            match_global_shortcut(&ch("l"), shift, screen),
            Some(GridSwap(1, 0))
        );
        assert_eq!(
            match_global_shortcut(&ch("k"), shift, screen),
            Some(GridSwap(0, -1))
        );
        assert_eq!(
            match_global_shortcut(&ch("j"), shift, screen),
            Some(GridSwap(0, 1))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowLeft), shift, screen),
            Some(GridSwap(-1, 0))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowRight), shift, screen),
            Some(GridSwap(1, 0))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowUp), shift, screen),
            Some(GridSwap(0, -1))
        );
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::ArrowDown), shift, screen),
            Some(GridSwap(0, 1))
        );
        // Cmd alone (no Alt, no Shift) is still GridMove, not GridSwap.
        assert_eq!(
            match_global_shortcut(&ch("h"), gmods(), screen),
            Some(GridMove(-1, 0))
        );
    }

    #[test]
    fn unmodified_or_unmapped_keys_are_not_shortcuts() {
        let screen = Screen::Workspace;
        assert_eq!(
            match_global_shortcut(&ch("n"), Modifiers::empty(), screen),
            None
        );
        assert_eq!(match_global_shortcut(&ch("x"), gmods(), screen), None);
        assert_eq!(
            match_global_shortcut(&Key::Named(Named::Tab), gmods(), screen),
            None
        );
    }

    #[test]
    fn slide_progress_eases_out_from_zero_to_one() {
        let start = std::time::Instant::now();
        assert_eq!(slide_progress(start, start), 0.0);
        // Halfway through, the cubic ease-out has already covered more than
        // half the distance (front-loaded motion).
        let half = start + GRID_SLIDE / 2;
        let p = slide_progress(start, half);
        assert!(p > 0.5 && p < 1.0, "expected ease-out progress, got {p}");
        // At and beyond the duration, progress clamps to 1.0.
        assert_eq!(slide_progress(start, start + GRID_SLIDE), 1.0);
        assert_eq!(slide_progress(start, start + GRID_SLIDE * 10), 1.0);
    }

    /// `CloseFocusedSession`'s decision logic is screen-independent — the
    /// caller resolves `target` per screen (grid tile vs active session), so
    /// these exercise the remaining runtime state (whether anything is focused,
    /// confirm-to-kill arming) directly rather than the full `Grove`, which is
    /// expensive to construct for a single match arm.
    mod close_focused_session_decision {
        use super::super::{close_focused_session_decision, CloseFocusedDecision};

        #[test]
        fn no_op_with_nothing_focused() {
            assert_eq!(
                close_focused_session_decision(None, None),
                CloseFocusedDecision::NoOp
            );
        }

        #[test]
        fn requests_kill_when_not_yet_armed() {
            assert_eq!(
                close_focused_session_decision(Some(2), None),
                CloseFocusedDecision::Request(2)
            );
            // Pending kill armed for a *different* session still requests.
            assert_eq!(
                close_focused_session_decision(Some(2), Some(5)),
                CloseFocusedDecision::Request(2)
            );
        }

        #[test]
        fn kills_when_already_armed_for_focused_session() {
            assert_eq!(
                close_focused_session_decision(Some(2), Some(2)),
                CloseFocusedDecision::Kill(2)
            );
        }
    }

    /// `grid_focused` must track the active session whenever the grid is
    /// showing, or will show again once zen exits — otherwise cycling/
    /// selecting sessions while zenned in from a tile leaves the tile
    /// pointer stale for when zen exits (Bug 5).
    mod should_sync_grid_focus {
        use super::super::should_sync_grid_focus;

        #[test]
        fn untouched_outside_grid_and_not_zenned_from_it() {
            assert!(!should_sync_grid_focus(false, false));
        }

        #[test]
        fn syncs_while_grid_is_open() {
            assert!(should_sync_grid_focus(true, false));
        }

        #[test]
        fn syncs_while_zenned_in_from_the_grid() {
            // grid_view is false during zen (it's temporarily suspended), but
            // grid_view_before_zen remembers to restore it on exit — that's
            // exactly the state where the desync used to happen.
            assert!(should_sync_grid_focus(false, true));
        }
    }

    /// Killing the focused tile shouldn't leave the grid with nothing
    /// focused — whatever slides into the killed slot should take focus.
    mod grid_focus_after_kill {
        use super::super::grid_focus_after_kill;

        #[test]
        fn killed_in_middle_focuses_same_slot() {
            // Tile that shifted into the killed slot takes focus.
            assert_eq!(grid_focus_after_kill(Some(1), 3), Some(1));
        }

        #[test]
        fn killed_at_end_clamps_to_new_last_slot() {
            // Nothing slides into the last slot, so clamp instead.
            assert_eq!(grid_focus_after_kill(Some(2), 2), Some(1));
        }

        #[test]
        fn no_tiles_remain_focuses_nothing() {
            assert_eq!(grid_focus_after_kill(Some(0), 0), None);
        }
    }

    /// Pure tile-index arithmetic for directional grid focus movement — see
    /// `grid_neighbor`'s doc comment for the row-major-but-column-rendered
    /// geometry this covers.
    mod grid_neighbor {
        use super::super::grid_neighbor;

        #[test]
        fn n3_horizontal_and_vertical_moves() {
            // n=3 -> cols=2, rows=2. Left column: 0 (top), 2 (bottom).
            // Right column: 1 only, spanning the full height.
            assert_eq!(grid_neighbor(2, 3, 1, 0), Some(1));
            assert_eq!(grid_neighbor(1, 3, -1, 0), Some(0));
            assert_eq!(grid_neighbor(1, 3, 0, 1), None);
            assert_eq!(grid_neighbor(0, 3, 0, 1), Some(2));
            assert_eq!(grid_neighbor(0, 3, -1, 0), None);
        }

        #[test]
        fn n4_full_2x2_grid() {
            assert_eq!(grid_neighbor(0, 4, 1, 0), Some(1));
            assert_eq!(grid_neighbor(0, 4, 0, 1), Some(2));
            assert_eq!(grid_neighbor(3, 4, 1, 0), None);
        }
    }
}
