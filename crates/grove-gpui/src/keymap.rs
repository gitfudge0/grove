//! The shortcut registry and the `KeyBinding` set generated from it.
//!
//! `ShortcutDef` / `Scope` / `Screen` / `SHORTCUTS` are copied from
//! `src/gui/update/shortcuts.rs:104-...` **unchanged in content** — the
//! registry stays the single source of truth for both bindings and (Plan 08's)
//! shortcut overlay, exactly as spec §5 requires. Only the *mechanism*
//! changed: iced's modifier-independent key names become gpui keystroke
//! strings, and the platform modifier is applied here rather than by a
//! `global_mods` predicate at match time.

// The registry is ported whole (spec §5: one source of truth for bindings AND
// the Plan 08 overlay), so the display-only helpers and the dynamic-chord
// variants have no caller yet.
#![allow(dead_code)]

use gpui::{actions, KeyBinding};

// ── platform chords ──────────────────────────────────────────────────────

/// The platform's global-shortcut modifier: Cmd on macOS (matching the Cmd+C /
/// Cmd+V pair), Ctrl+Shift elsewhere (matching Ctrl+Shift+C / Ctrl+Shift+V, so
/// plain Ctrl chords stay available to the PTY).
/// Port of `global_mods` + `platform_mod_label`.
pub fn platform_mod_prefix() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-"
    } else {
        "ctrl-shift-"
    }
}

/// Shared modifier for every `requires_alt` chord: Cmd+Alt (mac) / Ctrl+Alt
/// (elsewhere), independent of `platform_mod_prefix` (which already requires
/// Shift on non-mac and so can't be reused as a base for an Alt chord there).
/// Port of `alt_chord_mods`.
pub fn alt_chord_prefix() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-alt-"
    } else {
        "ctrl-alt-"
    }
}

/// Human-readable label for the global-shortcut modifier, for the overlay and
/// the status-bar chip (Plan 08).
pub fn platform_mod_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl+shift"
    }
}

// ── the registry (ported verbatim) ───────────────────────────────────────

/// App-level actions reachable from the global keyboard layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalShortcut {
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
    /// Create and focus a new home terminal — mod+Alt+T, reachable from the
    /// agent side or while already showing the terminal tab.
    NewHomeTerminal,
    /// Swap between the home-terminal tab and whatever the agent side was
    /// showing (grid or single session), preserving the other side's context.
    ToggleTerminal,
    /// Select the first session currently waiting for input, in tree order.
    JumpToWaitingSession,
    /// Move keyboard focus between grid tiles by `(dx, dy)`. Grid screen only.
    GridMove(i32, i32),
    /// Swap the focused tile with its neighbor by `(dx, dy)`. Grid screen only.
    GridSwap(i32, i32),
    /// Scroll the focused session by half a page (`true` = up).
    ScrollHalfPage(bool),
    /// Open the command palette straight into the "switch to session"
    /// drill-in. Zen-only — see `PaletteRow::SwitchToSession`.
    SwitchSession,
}

/// Coarse "which screen am I on" model, derived from existing UI flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Grid,
    Workspace,
    Zen,
}

impl Screen {
    /// Section header label used in the overlay when >1 scope is visible.
    pub fn label(self) -> &'static str {
        match self {
            Screen::Grid => "grid",
            Screen::Workspace => "workspace",
            Screen::Zen => "zen",
        }
    }

    /// gpui key-context string for this screen. Modal contexts arrive in
    /// Plan 08.
    pub fn key_context(self) -> &'static str {
        match self {
            Screen::Grid => "Grid",
            Screen::Workspace => "Workspace",
            Screen::Zen => "Zen",
        }
    }
}

/// Where a shortcut applies. A shortcut may list several scopes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Global,
    Screen(Screen),
}

/// One row of the shortcut registry — single source of truth for both the
/// key bindings (behavior) and the shortcut overlay (display).
pub struct ShortcutDef {
    /// `None` for display-only rows (the `1–9` row and the chords handled
    /// dynamically by the matcher).
    pub action: Option<GlobalShortcut>,
    /// Key names. Empty for display-only rows, and for `ToggleZen`, whose
    /// chord is Enter (`display_keys` carries it; see `keystrokes_for`).
    pub triggers: &'static [&'static str],
    /// Key label shown in the overlay; the platform modifier is prepended at
    /// render time (e.g. `"n"` -> `"cmd+n"`).
    pub display_keys: &'static str,
    pub description: &'static str,
    pub scopes: &'static [Scope],
    /// When true, this shortcut layers Alt on top of the platform's global
    /// modifier (e.g. Cmd+Alt+N / Ctrl+Alt+N) rather than using the plain
    /// platform modifier.
    pub requires_alt: bool,
    /// When true, `display_keys` is the complete chord text and the overlay
    /// renders it verbatim instead of prepending the platform modifier.
    pub literal: bool,
}

const G: &[Scope] = &[Scope::Global];

/// Single source of truth for behavioral matching and overlay display. Order
/// matches the overlay's reading order.
pub const SHORTCUTS: &[ShortcutDef] = &[
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
    // Display-only: the matcher handles 1–9 dynamically.
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
        action: Some(GlobalShortcut::ToggleTerminal),
        triggers: &["t", "T"],
        display_keys: "t",
        description: "toggle terminal",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::NewHomeTerminal),
        triggers: &["t", "T"],
        display_keys: "t",
        description: "New terminal",
        scopes: G,
        requires_alt: true,
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
    // Display-only: plain PageUp/PageDown/Home/End (no Shift) must fall
    // through to the PTY, so these live outside the binding set.
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
    // Display-only: the grid move/swap chords are dynamic (dx/dy per key,
    // swap modifier picks move vs. swap) and are wired in Plan 07.
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
        requires_alt: !cfg!(target_os = "macos"),
        literal: false,
    },
    // Display-only: closing the terminal panel must fall through to the PTY.
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
pub fn screen_from_flags(chrome_visible: bool, grid_view: bool) -> Screen {
    if !chrome_visible {
        Screen::Zen
    } else if grid_view {
        Screen::Grid
    } else {
        Screen::Workspace
    }
}

/// True if a shortcut whose registry row lists `scopes` may fire on `screen`.
pub fn scope_allows(scopes: &[Scope], screen: Screen) -> bool {
    scopes
        .iter()
        .any(|s| matches!(s, Scope::Global) || *s == Scope::Screen(screen))
}

// ── registry -> keystrokes ───────────────────────────────────────────────

/// iced trigger name -> gpui key name. The registry lists the shifted twin of
/// each key (`"p", "P"` / `"=", "+"`) because iced matches the produced
/// character; gpui keystrokes name the *unshifted* key and carry modifiers
/// separately, so both twins collapse onto one keystroke.
fn gpui_key(trigger: &str) -> Option<String> {
    let mut chars = trigger.chars();
    let (c, rest) = (chars.next()?, chars.next());
    if rest.is_some() {
        // Multi-char names ("enter") pass through as-is.
        return Some(trigger.to_string());
    }
    Some(match c {
        'A'..='Z' | 'a'..='z' => c.to_ascii_lowercase().to_string(),
        '+' => "=".to_string(),
        '_' => "-".to_string(),
        '?' => "/".to_string(),
        _ => c.to_string(),
    })
}

/// Every distinct gpui keystroke this row binds, already carrying its
/// platform modifier. Empty for display-only rows.
pub fn keystrokes_for(def: &ShortcutDef) -> Vec<String> {
    if def.action.is_none() {
        return Vec::new();
    }
    let prefix = if def.requires_alt {
        alt_chord_prefix()
    } else {
        platform_mod_prefix()
    };
    // `ToggleZen` is the one bound row with no `triggers`: iced matches Enter
    // outside the trigger table, so its key lives in `display_keys`.
    let raw: Vec<&str> = if def.triggers.is_empty() {
        vec![def.display_keys]
    } else {
        def.triggers.to_vec()
    };
    let mut out: Vec<String> = Vec::new();
    for t in raw {
        if let Some(k) = gpui_key(t) {
            let ks = format!("{prefix}{k}");
            if !out.contains(&ks) {
                out.push(ks);
            }
        }
    }
    out
}

/// Key contexts a row binds into. `Scope::Global` binds with no context (so it
/// fires anywhere); `Scope::Screen(s)` binds into that screen's context.
pub fn contexts_for(def: &ShortcutDef) -> Vec<Option<&'static str>> {
    let mut out: Vec<Option<&'static str>> = Vec::new();
    for s in def.scopes {
        let ctx = match s {
            Scope::Global => None,
            Scope::Screen(screen) => Some(screen.key_context()),
        };
        if !out.contains(&ctx) {
            out.push(ctx);
        }
    }
    out
}

// ── actions ──────────────────────────────────────────────────────────────

actions!(
    grove,
    [
        NewSession,
        NewSessionInWorktree,
        SwitchSession,
        NextSession,
        PrevSession,
        ToggleGrid,
        ToggleZen,
        Settings,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        ShortcutOverlay,
        CloseFocusedSession,
        ToggleTerminal,
        NewHomeTerminal,
        JumpToWaitingSession,
        ScrollHalfPageUp,
        ScrollHalfPageDown,
    ]
);

/// `mod+1..9` — the one **data-carrying** action (Plan 03 deviation 3). The
/// `actions!` macro only generates unit structs, so this one is derived by hand.
/// `no_json` keeps the derive off `serde`/`schemars`, which grove-gpui does not
/// depend on; grove has no JSON keymap to load from.
#[derive(Clone, Debug, Default, PartialEq, gpui::Action)]
#[action(namespace = grove, no_json)]
pub struct SelectSession {
    /// 1-based, as displayed. Out of range is a no-op, never a clamp.
    pub index: usize,
}

/// The nine `mod+1..9` bindings, generated from the registry's display-only
/// `1–9` row so the registry stays the single source of truth.
fn select_session_bindings() -> Vec<KeyBinding> {
    let prefix = platform_mod_prefix();
    (1..=9)
        .map(|index| KeyBinding::new(&format!("{prefix}{index}"), SelectSession { index }, None))
        .collect()
}

/// One `KeyBinding` for a registry action, or `None` for the shortcuts whose
/// chords are dynamic (`SelectSession`, `GridMove`, `GridSwap`) and therefore
/// never reach a static binding.
fn binding_for(keystroke: &str, sc: GlobalShortcut, ctx: Option<&str>) -> Option<KeyBinding> {
    use GlobalShortcut as S;
    Some(match sc {
        S::NewSession => KeyBinding::new(keystroke, NewSession, ctx),
        S::NewSessionInWorktree => KeyBinding::new(keystroke, NewSessionInWorktree, ctx),
        S::SwitchSession => KeyBinding::new(keystroke, SwitchSession, ctx),
        S::NextSession => KeyBinding::new(keystroke, NextSession, ctx),
        S::PrevSession => KeyBinding::new(keystroke, PrevSession, ctx),
        S::ToggleGrid => KeyBinding::new(keystroke, ToggleGrid, ctx),
        S::ToggleZen => KeyBinding::new(keystroke, ToggleZen, ctx),
        S::Settings => KeyBinding::new(keystroke, Settings, ctx),
        S::ZoomIn => KeyBinding::new(keystroke, ZoomIn, ctx),
        S::ZoomOut => KeyBinding::new(keystroke, ZoomOut, ctx),
        S::ZoomReset => KeyBinding::new(keystroke, ZoomReset, ctx),
        S::ShortcutOverlay => KeyBinding::new(keystroke, ShortcutOverlay, ctx),
        S::CloseFocusedSession => KeyBinding::new(keystroke, CloseFocusedSession, ctx),
        S::ToggleTerminal => KeyBinding::new(keystroke, ToggleTerminal, ctx),
        S::NewHomeTerminal => KeyBinding::new(keystroke, NewHomeTerminal, ctx),
        S::JumpToWaitingSession => KeyBinding::new(keystroke, JumpToWaitingSession, ctx),
        S::ScrollHalfPage(true) => KeyBinding::new(keystroke, ScrollHalfPageUp, ctx),
        S::ScrollHalfPage(false) => KeyBinding::new(keystroke, ScrollHalfPageDown, ctx),
        S::SelectSession(_) | S::GridMove(..) | S::GridSwap(..) => return None,
    })
}

/// The registry is the only source of key bindings — a shortcut that exists in
/// SHORTCUTS but has no binding here is a bug, not a feature. Asserted below.
pub fn bindings() -> Vec<KeyBinding> {
    let mut out = Vec::new();
    for def in SHORTCUTS {
        let Some(sc) = def.action else { continue };
        for ctx in contexts_for(def) {
            for ks in keystrokes_for(def) {
                if let Some(b) = binding_for(&ks, sc, ctx) {
                    out.push(b);
                }
            }
        }
    }
    out.extend(select_session_bindings());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every bound row must actually produce a keystroke — the drift guard the
    /// "cannot drift" claim rests on.
    #[test]
    fn every_actionable_row_produces_a_binding() {
        for def in SHORTCUTS {
            let Some(sc) = def.action else { continue };
            if matches!(
                sc,
                GlobalShortcut::GridMove(..) | GlobalShortcut::GridSwap(..)
            ) {
                continue;
            }
            let ks = keystrokes_for(def);
            assert!(
                !ks.is_empty(),
                "{:?} ({}) produced no keystroke",
                sc,
                def.description
            );
            for k in &ks {
                assert!(
                    binding_for(k, sc, None).is_some(),
                    "{sc:?} has no KeyBinding arm"
                );
            }
        }
        assert!(!bindings().is_empty());
    }

    /// Plan 05 Task 6 Step 1: after this phase the only registry actions
    /// without a `KeyBinding` are the grid ones (Plan 07).
    #[test]
    fn only_the_grid_actions_remain_unbound() {
        for def in SHORTCUTS {
            let Some(sc) = def.action else { continue };
            if binding_for("ctrl-x", sc, None).is_none() {
                assert!(
                    matches!(
                        sc,
                        GlobalShortcut::GridMove(..) | GlobalShortcut::GridSwap(..)
                    ),
                    "{sc:?} has no binding and is not a grid action"
                );
            }
        }
        // `SelectSession` is now bound through its own payload action.
        assert_eq!(select_session_bindings().len(), 9);
    }

    #[test]
    fn no_two_rows_share_a_keystroke_in_the_same_context() {
        let mut seen: HashSet<(Option<&'static str>, String)> = HashSet::new();
        for def in SHORTCUTS {
            if def.action.is_none() {
                continue;
            }
            for ctx in contexts_for(def) {
                for ks in keystrokes_for(def) {
                    assert!(
                        seen.insert((ctx, ks.clone())),
                        "duplicate keystroke {ks} in context {ctx:?} ({})",
                        def.description
                    );
                }
            }
        }
    }

    #[test]
    fn shifted_twins_collapse_onto_one_keystroke() {
        let Some(new_session) = SHORTCUTS
            .iter()
            .find(|d| d.action == Some(GlobalShortcut::NewSession))
        else {
            unreachable!("GlobalShortcut::NewSession must have a registry row");
        };
        assert_eq!(new_session.triggers, &["p", "P"]);
        assert_eq!(keystrokes_for(new_session).len(), 1);

        let Some(zoom_in) = SHORTCUTS
            .iter()
            .find(|d| d.action == Some(GlobalShortcut::ZoomIn))
        else {
            unreachable!("GlobalShortcut::ZoomIn must have a registry row");
        };
        assert_eq!(
            keystrokes_for(zoom_in),
            vec![format!("{}=", platform_mod_prefix())]
        );
    }

    #[test]
    fn toggle_zen_binds_enter_despite_empty_triggers() {
        let Some(zen) = SHORTCUTS
            .iter()
            .find(|d| d.action == Some(GlobalShortcut::ToggleZen))
        else {
            unreachable!("GlobalShortcut::ToggleZen must have a registry row");
        };
        assert!(zen.triggers.is_empty());
        assert_eq!(
            keystrokes_for(zen),
            vec![format!("{}enter", platform_mod_prefix())]
        );
    }

    #[test]
    fn display_only_rows_bind_nothing() {
        for def in SHORTCUTS.iter().filter(|d| d.action.is_none()) {
            assert!(keystrokes_for(def).is_empty(), "{}", def.description);
        }
    }

    #[test]
    fn alt_rows_use_the_alt_chord() {
        let Some(nht) = SHORTCUTS
            .iter()
            .find(|d| d.action == Some(GlobalShortcut::NewHomeTerminal))
        else {
            unreachable!("GlobalShortcut::NewHomeTerminal must have a registry row");
        };
        assert!(nht.requires_alt);
        assert_eq!(
            keystrokes_for(nht),
            vec![format!("{}t", alt_chord_prefix())]
        );
    }

    #[test]
    fn scoped_rows_get_their_screen_context() {
        let grid_only = ShortcutDef {
            action: Some(GlobalShortcut::ToggleGrid),
            triggers: &["g"],
            display_keys: "g",
            description: "test",
            scopes: &[Scope::Screen(Screen::Grid)],
            requires_alt: false,
            literal: false,
        };
        assert_eq!(contexts_for(&grid_only), vec![Some("Grid")]);
        assert!(scope_allows(grid_only.scopes, Screen::Grid));
        assert!(!scope_allows(grid_only.scopes, Screen::Zen));
        assert!(scope_allows(G, Screen::Zen));
    }

    #[test]
    fn screen_from_flags_prefers_zen() {
        assert_eq!(screen_from_flags(false, true), Screen::Zen);
        assert_eq!(screen_from_flags(true, true), Screen::Grid);
        assert_eq!(screen_from_flags(true, false), Screen::Workspace);
        assert_eq!(Screen::Grid.label(), "grid");
    }

    #[test]
    fn platform_labels_agree_with_the_prefix() {
        assert_eq!(
            platform_mod_label().replace('+', "-"),
            platform_mod_prefix().trim_end_matches('-')
        );
    }
}
