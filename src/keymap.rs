//! The shortcut registry and the `KeyBinding` set generated from it. `ShortcutDef`/`Scope`/`Screen`/`SHORTCUTS` are
//! ported from `src/gui/update/shortcuts.rs:104-...` unchanged in content (spec §5).

use gpui::{actions, KeyBinding};

/// Cmd on macOS, Ctrl+Shift elsewhere (so plain Ctrl chords stay free for the PTY). Port of `global_mods`.
pub fn platform_mod_prefix() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-"
    } else {
        "ctrl-shift-"
    }
}

/// Port of `alt_chord_mods`; independent of `platform_mod_prefix` since that already carries Shift on non-mac.
pub fn alt_chord_prefix() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-alt-"
    } else {
        "ctrl-alt-"
    }
}

pub fn platform_mod_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl+shift"
    }
}

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
    // Dynamic-chord vocabulary entries, never constructed directly.
    #[allow(dead_code)]
    SelectSession(usize),
    ShortcutOverlay,
    CloseFocusedSession,
    NewHomeTerminal,
    ToggleTerminal,
    /// Grid never renders the panel; the handler gates on grid view itself.
    ToggleTermPanel,
    ToggleRailMode,
    JumpToWaitingSession,
    #[allow(dead_code)]
    GridMove(i32, i32),
    #[allow(dead_code)]
    GridSwap(i32, i32),
    ScrollHalfPage(bool),
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
    pub fn label(self) -> &'static str {
        match self {
            Screen::Grid => "grid",
            Screen::Workspace => "workspace",
            Screen::Zen => "zen",
        }
    }

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

/// Single source of truth for both key bindings and the shortcut overlay.
pub struct ShortcutDef {
    pub action: Option<GlobalShortcut>,
    /// Empty for display-only rows and for `ToggleZen`, whose chord is Enter (carried in `display_keys` instead).
    pub triggers: &'static [&'static str],
    pub display_keys: &'static str,
    pub description: &'static str,
    pub scopes: &'static [Scope],
    pub requires_alt: bool,
    /// When true, `display_keys` renders verbatim instead of getting the platform modifier prepended.
    pub literal: bool,
}

const G: &[Scope] = &[Scope::Global];

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
        action: Some(GlobalShortcut::ToggleTermPanel),
        triggers: &["e", "E"],
        display_keys: "e",
        description: "toggle side terminal",
        scopes: G,
        requires_alt: false,
        literal: false,
    },
    ShortcutDef {
        action: Some(GlobalShortcut::ToggleRailMode),
        triggers: &["b", "B"],
        display_keys: "b",
        description: "toggle sidebar tree / sessions",
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
    // Plain PageUp/PageDown/Home/End must fall through to the PTY.
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
    // Grid move/swap chords are dynamic; wired elsewhere.
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
    // Closing the terminal panel must fall through to the PTY.
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: "ctrl+shift+←/→",
        description: "Resize terminal panel",
        scopes: &[Scope::Screen(Screen::Workspace)],
        requires_alt: false,
        literal: true,
    },
    // `zen_focus_bindings` generates the real bindings from two distinct actions.
    ShortcutDef {
        action: None,
        triggers: &[],
        display_keys: "←/→",
        description: "focus side terminal / agent",
        scopes: &[Scope::Screen(Screen::Zen)],
        requires_alt: false,
        literal: false,
    },
];

/// Zen wins over grid: while chrome is hidden the user is in zen regardless of `grid_view`.
pub fn screen_from_flags(chrome_visible: bool, grid_view: bool) -> Screen {
    if !chrome_visible {
        Screen::Zen
    } else if grid_view {
        Screen::Grid
    } else {
        Screen::Workspace
    }
}

// Exercised only by tests; the live path uses `contexts_for` instead.
#[allow(dead_code)]
pub fn scope_allows(scopes: &[Scope], screen: Screen) -> bool {
    scopes
        .iter()
        .any(|s| matches!(s, Scope::Global) || *s == Scope::Screen(screen))
}

/// gpui keystrokes name the unshifted key, so both trigger twins collapse onto one keystroke.
fn gpui_key(trigger: &str) -> Option<String> {
    let mut chars = trigger.chars();
    let (c, rest) = (chars.next()?, chars.next());
    if rest.is_some() {
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

/// gpui's Linux backend reports the shifted glyph and drops the shift modifier, so Ctrl+Shift+, arrives as `ctrl-<`.
const SHIFTED_TWINS: &[(char, char)] = &[
    (',', '<'),
    ('.', '>'),
    ('/', '?'),
    ('=', '+'),
    ('-', '_'),
    (';', ':'),
    ('\'', '"'),
    ('[', '{'),
    (']', '}'),
    ('\\', '|'),
    ('`', '~'),
    ('0', ')'),
    ('1', '!'),
    ('2', '@'),
    ('3', '#'),
    ('4', '$'),
    ('5', '%'),
    ('6', '^'),
    ('7', '&'),
    ('8', '*'),
    ('9', '('),
];

fn shifted_twin(key: &str) -> Option<String> {
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    SHIFTED_TWINS
        .iter()
        .find(|(base, _)| *base == c)
        .map(|(_, t)| t.to_string())
}

fn needs_shifted_twins() -> bool {
    !cfg!(target_os = "macos")
}

/// `ctrl-shift-` -> `ctrl-`, since gpui strips shift for these keys (see `SHIFTED_TWINS`).
fn twin_prefix(prefix: &str) -> String {
    prefix.replace("shift-", "")
}

pub fn keystrokes_for(def: &ShortcutDef) -> Vec<String> {
    if def.action.is_none() {
        return Vec::new();
    }
    let prefix = if def.requires_alt {
        alt_chord_prefix()
    } else {
        platform_mod_prefix()
    };
    // `ToggleZen` has no `triggers`; its key lives in `display_keys` instead.
    let raw: Vec<&str> = if def.triggers.is_empty() {
        vec![def.display_keys]
    } else {
        def.triggers.to_vec()
    };
    let mut out: Vec<String> = Vec::new();
    let push = |ks: String, out: &mut Vec<String>| {
        if !out.contains(&ks) {
            out.push(ks);
        }
    };
    for t in raw {
        if let Some(k) = gpui_key(t) {
            push(format!("{prefix}{k}"), &mut out);
            // Only the platform prefix carries Shift; the alt chord does not.
            if needs_shifted_twins() && !def.requires_alt {
                if let Some(tw) = shifted_twin(&k) {
                    push(format!("{}{tw}", twin_prefix(prefix)), &mut out);
                }
            }
        }
    }
    out
}

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
        ToggleTermPanel,
        ToggleRailMode,
        NewHomeTerminal,
        JumpToWaitingSession,
        ScrollHalfPageUp,
        ScrollHalfPageDown,
        FocusSidePanel,
        FocusAgentPane,
    ]
);

// Bound in the descendant context `"<ModalKind> > Input"`, which wins over gpui-component's plain `"Input"` context.
actions!(
    grove_modal,
    [
        ModalUp,
        ModalDown,
        ModalLeft,
        ModalRight,
        ModalTab,
        ModalShiftTab,
        ModalEnter,
    ]
);

/// `actions!` only generates unit structs, so data-carrying actions are derived by hand.
#[derive(Clone, Debug, Default, PartialEq, gpui::Action)]
#[action(namespace = grove, no_json)]
pub struct SelectSession {
    /// 1-based, as displayed. Out of range is a no-op, never a clamp.
    pub index: usize,
}

#[derive(Clone, Debug, Default, PartialEq, gpui::Action)]
#[action(namespace = grove, no_json)]
pub struct GridMove {
    pub dx: i32,
    pub dy: i32,
}

/// Same shape as [`GridMove`]; the modifier is what tells them apart (`grid_swap_mods`, `shortcuts.rs:25-32`).
#[derive(Clone, Debug, Default, PartialEq, gpui::Action)]
#[action(namespace = grove, no_json)]
pub struct GridSwap {
    pub dx: i32,
    pub dy: i32,
}

/// Bound only in the workspace context (recorded ambiguity 4), so a closed panel falls through to the PTY.
#[derive(Clone, Debug, Default, PartialEq, gpui::Action)]
#[action(namespace = grove, no_json)]
pub struct AdjustTermPanel {
    pub delta: i16,
}

/// h/j/k/l and the four arrows, matching the by-hand arm at `shortcuts.rs:446-463`.
pub const GRID_DIRECTIONS: &[(&str, i32, i32)] = &[
    ("h", -1, 0),
    ("l", 1, 0),
    ("k", 0, -1),
    ("j", 0, 1),
    ("left", -1, 0),
    ("right", 1, 0),
    ("up", 0, -1),
    ("down", 0, 1),
];

/// Port of `grid_swap_mods`: on macOS Alt or Shift (Cmd+Opt+H collides with OS "Hide Others"), Alt alone elsewhere.
pub fn grid_swap_prefixes() -> Vec<String> {
    if cfg!(target_os = "macos") {
        vec!["cmd-alt-".to_string(), "cmd-shift-".to_string()]
    } else {
        vec![format!("{}alt-", platform_mod_prefix())]
    }
}

fn grid_bindings() -> Vec<KeyBinding> {
    let ctx = Some(Screen::Grid.key_context());
    let prefix = platform_mod_prefix();
    let swap_prefixes = grid_swap_prefixes();
    let mut out = Vec::new();
    for &(key, dx, dy) in GRID_DIRECTIONS {
        out.push(KeyBinding::new(
            &format!("{prefix}{key}"),
            GridMove { dx, dy },
            ctx,
        ));
        for sp in &swap_prefixes {
            out.push(KeyBinding::new(
                &format!("{sp}{key}"),
                GridSwap { dx, dy },
                ctx,
            ));
        }
    }
    out
}

/// Workspace context only (recorded ambiguity 4).
fn term_panel_bindings() -> Vec<KeyBinding> {
    #[allow(clippy::cast_possible_wrap)]
    let step = crate::entities::workspace_state::TERM_PANEL_PORTION_STEP as i16;
    let ctx = Some(Screen::Workspace.key_context());
    vec![
        KeyBinding::new("ctrl-shift-right", AdjustTermPanel { delta: step }, ctx),
        KeyBinding::new("ctrl-shift-left", AdjustTermPanel { delta: -step }, ctx),
    ]
}

/// Focus only, not open/close — scoped to `Zen` so the same chords stay free for `AdjustTermPanel` elsewhere.
fn zen_focus_bindings() -> Vec<KeyBinding> {
    let ctx = Some(Screen::Zen.key_context());
    let prefix = platform_mod_prefix();
    vec![
        KeyBinding::new(&format!("{prefix}right"), FocusSidePanel, ctx),
        KeyBinding::new(&format!("{prefix}left"), FocusAgentPane, ctx),
    ]
}

/// Derived from [`crate::views::modals::input::InputPolicy`] so bindings and the field's own policy can't disagree.
pub fn modal_input_bindings() -> Vec<KeyBinding> {
    use crate::views::modals::input::{InputPolicy, ModalInput};

    let mut out = Vec::new();
    for kind in crate::modal::ModalKind::ALL {
        let policy = InputPolicy::for_modal(kind);
        if policy.multi_line {
            continue;
        }
        let ctx = ModalInput::override_context(kind);
        let ctx = Some(ctx.as_str());
        out.push(KeyBinding::new("up", ModalUp, ctx));
        out.push(KeyBinding::new("down", ModalDown, ctx));
        out.push(KeyBinding::new("enter", ModalEnter, ctx));
        if policy.wants_tab {
            out.push(KeyBinding::new("tab", ModalTab, ctx));
            out.push(KeyBinding::new("shift-tab", ModalShiftTab, ctx));
        }
        if policy.wants_arrows {
            out.push(KeyBinding::new("left", ModalLeft, ctx));
            out.push(KeyBinding::new("right", ModalRight, ctx));
        }
    }
    out
}

fn select_session_bindings() -> Vec<KeyBinding> {
    let prefix = platform_mod_prefix();
    let mut out = Vec::new();
    for index in 1..=9usize {
        let key = index.to_string();
        out.push(KeyBinding::new(
            &format!("{prefix}{key}"),
            SelectSession { index },
            None,
        ));
        // On non-mac these arrive as `!@#$%^&*(` with shift stripped (see `SHIFTED_TWINS`).
        if needs_shifted_twins() {
            if let Some(tw) = shifted_twin(&key) {
                out.push(KeyBinding::new(
                    &format!("{}{tw}", twin_prefix(prefix)),
                    SelectSession { index },
                    None,
                ));
            }
        }
    }
    out
}

/// `None` for the dynamic-chord shortcuts, which never reach a static binding.
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
        S::ToggleTermPanel => KeyBinding::new(keystroke, ToggleTermPanel, ctx),
        S::ToggleRailMode => KeyBinding::new(keystroke, ToggleRailMode, ctx),
        S::NewHomeTerminal => KeyBinding::new(keystroke, NewHomeTerminal, ctx),
        S::JumpToWaitingSession => KeyBinding::new(keystroke, JumpToWaitingSession, ctx),
        S::ScrollHalfPage(true) => KeyBinding::new(keystroke, ScrollHalfPageUp, ctx),
        S::ScrollHalfPage(false) => KeyBinding::new(keystroke, ScrollHalfPageDown, ctx),
        S::SelectSession(_) | S::GridMove(..) | S::GridSwap(..) => return None,
    })
}

/// The registry is the only source of key bindings; a row with no binding here is a bug (asserted below).
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
    out.extend(grid_bindings());
    out.extend(term_panel_bindings());
    out.extend(zen_focus_bindings());
    // Last, so a modal's `"… > Input"` binding out-ranks anything above on a tie.
    out.extend(modal_input_bindings());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_actionable_row_produces_a_binding() {
        for def in SHORTCUTS {
            let Some(sc) = def.action else { continue };
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

    /// Carried amendment 3: no registry action is left without a `KeyBinding`, no exceptions.
    #[test]
    fn no_registry_action_is_left_unbound() {
        for def in SHORTCUTS {
            let Some(sc) = def.action else { continue };
            assert!(
                binding_for("ctrl-x", sc, None).is_some(),
                "{sc:?} has no binding"
            );
        }
        // Twins bound without `shift-`, since gpui strips it for digit keys.
        let sel = select_session_bindings();
        assert_eq!(sel.len(), if needs_shifted_twins() { 18 } else { 9 });
        if needs_shifted_twins() {
            let strokes: Vec<String> = sel
                .iter()
                .map(|b| {
                    b.keystrokes()
                        .iter()
                        .map(gpui::KeybindingKeystroke::unparse)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect();
            assert!(strokes.contains(&"ctrl-!".to_string()), "{strokes:?}");
            assert!(strokes.contains(&"ctrl-(".to_string()), "{strokes:?}");
            assert!(!strokes.iter().any(|s| s.starts_with("ctrl-shift-!")));
        }
        // 8 directions × (1 move + 1 swap chord per platform modifier set).
        let per_dir = 1 + grid_swap_prefixes().len();
        assert_eq!(grid_bindings().len(), GRID_DIRECTIONS.len() * per_dir);
        assert_eq!(term_panel_bindings().len(), 2);
        assert_eq!(zen_focus_bindings().len(), 2);
    }

    /// h/j/k/l must agree with their arrow twins (`shortcuts.rs:446-463`).
    #[test]
    fn every_grid_direction_carries_its_delta() {
        let by_key = |k: &str| {
            GRID_DIRECTIONS
                .iter()
                .find(|(key, ..)| *key == k)
                .map(|&(_, dx, dy)| (dx, dy))
        };
        assert_eq!(by_key("h"), Some((-1, 0)));
        assert_eq!(by_key("l"), Some((1, 0)));
        assert_eq!(by_key("k"), Some((0, -1)));
        assert_eq!(by_key("j"), Some((0, 1)));
        assert_eq!(by_key("left"), by_key("h"));
        assert_eq!(by_key("right"), by_key("l"));
        assert_eq!(by_key("up"), by_key("k"));
        assert_eq!(by_key("down"), by_key("j"));
        assert_eq!(GRID_DIRECTIONS.len(), 8);
    }

    /// Alt or Shift on macOS, Alt alone elsewhere — never the same chord as a plain move.
    #[test]
    fn the_swap_modifier_set_is_platform_correct() {
        let swap = grid_swap_prefixes();
        if cfg!(target_os = "macos") {
            assert_eq!(swap, vec!["cmd-alt-".to_string(), "cmd-shift-".to_string()]);
        } else {
            assert_eq!(swap, vec!["ctrl-shift-alt-".to_string()]);
        }
        for p in &swap {
            assert_ne!(p.as_str(), platform_mod_prefix());
        }
    }

    /// Recorded ambiguity 4: workspace-scoped, so it never shadows the grid's own arrow chords.
    #[test]
    fn the_panel_step_is_workspace_scoped_only() {
        let Some(row) = SHORTCUTS
            .iter()
            .find(|d| d.description == "Resize terminal panel")
        else {
            unreachable!("the panel-resize row must stay in the registry");
        };
        assert!(row.action.is_none());
        assert!(keystrokes_for(row).is_empty());
        assert_eq!(row.scopes, &[Scope::Screen(Screen::Workspace)]);
        assert!(!scope_allows(row.scopes, Screen::Grid));
        assert!(!scope_allows(row.scopes, Screen::Zen));
    }

    /// The terminal tab is not a fourth `Screen`; `terminal_focused` is orthogonal to it (`shortcuts.rs:87-91`).
    #[test]
    fn the_screen_model_has_exactly_three_states() {
        for screen in [Screen::Grid, Screen::Workspace, Screen::Zen] {
            assert!(!screen.key_context().is_empty());
            assert!(!screen.label().is_empty());
        }
        for def in SHORTCUTS {
            for ctx in contexts_for(def).into_iter().flatten() {
                assert!(
                    ["Grid", "Workspace", "Zen"].contains(&ctx),
                    "unexpected key context {ctx}"
                );
            }
        }
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
        let p = platform_mod_prefix();
        let expected = if needs_shifted_twins() {
            vec![format!("{p}="), format!("{}+", twin_prefix(p))]
        } else {
            vec![format!("{p}=")]
        };
        assert_eq!(keystrokes_for(zoom_in), expected);
    }

    /// On Linux/Windows, Ctrl+Shift+, arrives as `ctrl-<` and only that form can reach `Settings`.
    #[test]
    fn shifted_twins_are_also_bound_on_non_mac() {
        let Some(settings) = SHORTCUTS
            .iter()
            .find(|d| d.action == Some(GlobalShortcut::Settings))
        else {
            unreachable!("GlobalShortcut::Settings must have a registry row");
        };
        let p = platform_mod_prefix();
        let ks = keystrokes_for(settings);
        assert!(ks.contains(&format!("{p},")));
        assert_eq!(
            ks.contains(&format!("{}<", twin_prefix(p))),
            needs_shifted_twins()
        );
        assert!(!ks.contains(&format!("{p}<")));
        if needs_shifted_twins() {
            assert_eq!(twin_prefix(p), "ctrl-");
        }
        assert!(!bindings().is_empty());
        assert_eq!(shifted_twin("1").as_deref(), Some("!"));
        assert_eq!(shifted_twin("enter"), None);
        assert_eq!(shifted_twin("a"), None);
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
