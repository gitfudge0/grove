# Implementation plan: Screen-scoped shortcuts + bottom-bar hint

**Date:** 2026-07-06
**Spec:** `docs/superpowers/specs/2026-07-06-screen-scoped-shortcuts-design.md`
**Status:** Ready to implement

This plan turns the twice-declared shortcut list into a single data-driven
registry, adds a coarse `Screen` model with a `current_screen()` helper, and adds
a clickable `⌘/ shortcuts` chip to the status bar. It is deliberately scoped to
the spec: no existing shortcut is re-scoped (everything stays `Global`), so the
overlay must remain visually identical to today (flat list, no section headers).

Key existing anchors (verified against the current tree):
- `src/gui/update.rs:2884` `global_mods`, `:2892` `GlobalShortcut` enum,
  `:2910` `match_global_shortcut`, `:2939`/`:2953` copy/paste matchers,
  `:2963` `#[cfg(test)] mod tests`.
- `src/gui/update.rs:1723` `run_global_shortcut`, `:2043` modal key handling that
  references `match_global_shortcut`.
- `src/gui/view.rs:1670` `statusbar` (right side built at `:1749`),
  `:3265` `shortcut_overlay_modal` (hardcoded `entries` at `:3271`).
- `src/gui/state.rs:332` `Msg` enum; `:143` `grid_view`.
- `src/app.rs:287` `Modal::ShortcutOverlay`, `:418` `chrome_visible`.

---

## Step 1 — Add the `Screen` / `Scope` model and the registry types

**File:** `src/gui/update.rs` (near the shortcut code, immediately above
`GlobalShortcut` at line 2892).

Add the screen/scope enums and the `ShortcutDef` struct. Keep them private to the
module (matching `GlobalShortcut`, which is module-private).

```rust
/// Coarse "which screen am I on" model, derived from existing UI flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Grid,
    Workspace,
    Zen,
}

impl Screen {
    /// Section header label used in the overlay when >1 scope is visible.
    fn label(self) -> &'static str {
        match self {
            Screen::Grid => "grid",
            Screen::Workspace => "workspace",
            Screen::Zen => "zen",
        }
    }
}

/// Where a shortcut applies. A shortcut may list several scopes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    Global,
    Screen(Screen),
}

/// One row of the shortcut registry — single source of truth for both
/// `match_global_shortcut` (behavior) and `shortcut_overlay_modal` (display).
struct ShortcutDef {
    /// `None` for the display-only `1–9` row (matcher handles it dynamically).
    action: Option<GlobalShortcut>,
    /// Key chars matched against iced's modifier-independent `key`. Empty for
    /// the display-only row. `Enter` is matched separately (see the matcher).
    triggers: &'static [&'static str],
    /// Key label shown in the overlay; the platform modifier is prepended at
    /// render time (e.g. `"n"` -> `"cmd+n"`).
    display_keys: &'static str,
    description: &'static str,
    scopes: &'static [Scope],
}
```

Notes / rationale:
- `action: Option<GlobalShortcut>` lets the single `SelectNth` display row exist
  without a fake action. It is skipped by the matcher (Step 3).
- `Enter` (ToggleZen) has no character trigger, so its `triggers` stays empty and
  the matcher keeps a dedicated `Key::Named(Named::Enter)` arm (Step 3).

**Verify:** types compile once referenced by Steps 2–3 (`cargo build`).

---

## Step 2 — Populate the `SHORTCUTS` registry

**File:** `src/gui/update.rs`, directly after the types from Step 1.

Order matches the current overlay's reading order so the flat list looks
unchanged. All entries are `Global` per the spec's non-goals.

```rust
const G: &[Scope] = &[Scope::Global];

const SHORTCUTS: &[ShortcutDef] = &[
    ShortcutDef { action: Some(GlobalShortcut::NewSession),      triggers: &["n", "N"],      display_keys: "n",         description: "new session",            scopes: G },
    ShortcutDef { action: Some(GlobalShortcut::NextSession),     triggers: &["j", "J"],      display_keys: "j",         description: "next session",           scopes: G },
    ShortcutDef { action: Some(GlobalShortcut::PrevSession),     triggers: &["k", "K"],      display_keys: "k",         description: "previous session",       scopes: G },
    // Display-only: the matcher handles 1–9 dynamically (see match_global_shortcut).
    ShortcutDef { action: None,                                  triggers: &[],              display_keys: "1–9",       description: "select nth session",     scopes: G },
    ShortcutDef { action: Some(GlobalShortcut::ToggleGrid),      triggers: &["g", "G"],      display_keys: "g",         description: "toggle grid view",       scopes: G },
    ShortcutDef { action: Some(GlobalShortcut::ToggleZen),       triggers: &[],              display_keys: "enter",     description: "toggle zen mode",        scopes: G },
    ShortcutDef { action: Some(GlobalShortcut::Settings),        triggers: &[","],           display_keys: ",",         description: "settings",               scopes: G },
    ShortcutDef { action: Some(GlobalShortcut::ZoomIn),          triggers: &["=", "+"],      display_keys: "=",         description: "zoom in",                scopes: G },
    ShortcutDef { action: Some(GlobalShortcut::ZoomOut),         triggers: &["-", "_"],      display_keys: "-",         description: "zoom out",               scopes: G },
    ShortcutDef { action: Some(GlobalShortcut::ZoomReset),       triggers: &["0"],           display_keys: "0",         description: "reset zoom",             scopes: G },
    ShortcutDef { action: Some(GlobalShortcut::ShortcutOverlay), triggers: &["/", "?"],      display_keys: "/",         description: "this overlay",           scopes: G },
];
```

Notes:
- Copy/paste and `esc` are intentionally NOT in the registry — copy/paste keep
  their dedicated matchers (`is_copy_shortcut`/`is_paste_shortcut`, not
  global-modifier shortcuts) and `esc` is not a global-modifier shortcut. To keep
  the overlay visually identical, Step 6 re-adds the `copy / paste` and `esc to
  close` rows as static overlay-only lines (they were in the old `entries`
  array). This preserves display parity without polluting the behavioral
  registry.

**Verify:** `cargo build` after Step 3 wires it in.

---

## Step 3 — Rewrite `match_global_shortcut` to iterate the registry

**File:** `src/gui/update.rs:2910` `match_global_shortcut`.

Keep the `global_mods` gate, the dynamic `1–9` handling, and the `Enter` arm.
Replace the hardcoded character `match` with a registry scan.

```rust
fn match_global_shortcut(key: &Key, mods: Modifiers) -> Option<GlobalShortcut> {
    if !global_mods(mods) {
        return None;
    }
    match key {
        Key::Named(Named::Enter) => Some(GlobalShortcut::ToggleZen),
        Key::Character(s) => {
            let s = s.as_str();
            // Registry-driven character shortcuts.
            if let Some(def) = SHORTCUTS
                .iter()
                .find(|d| d.action.is_some() && d.triggers.contains(&s))
            {
                return def.action;
            }
            // SelectNth stays special-cased: dynamic n, display-only in registry.
            s.parse::<usize>()
                .ok()
                .filter(|n| (1..=9).contains(n))
                .map(|n| GlobalShortcut::SelectSession(n - 1))
        }
        _ => None,
    }
}
```

Notes:
- `triggers` are case-sensitive `contains`, and the registry already lists both
  cases (`"n"`, `"N"`) exactly as the old matcher did, so behavior is identical.
- The `ToggleZen` registry row has empty `triggers`, so it's never matched here —
  correct, since `Enter` is a `Named` key, handled by its own arm.

**Verify:** existing tests in `mod tests` (`:2981`
`global_shortcuts_map_with_platform_modifier`, and the unmapped-keys test) must
still pass unchanged: `cargo test match_global_shortcut` / `cargo test -p ...`.
Add two assertions to that test for the aliases the registry now drives:

```rust
assert_eq!(match_global_shortcut(&ch("+"), gmods()), Some(ZoomIn));
assert_eq!(match_global_shortcut(&ch("_"), gmods()), Some(ZoomOut));
assert_eq!(match_global_shortcut(&ch("?"), gmods()), Some(ShortcutOverlay));
```

---

## Step 4 — Add `current_screen()` helper

**File:** `src/gui/update.rs` (as a method on the `impl Grove` block that already
holds `run_global_shortcut` at `:1723`), or wherever `grid_view`/`app` are in
scope. Place it near `run_global_shortcut`.

```rust
/// Coarse current screen, derived from existing flags. Zen wins over grid:
/// while chrome is hidden the user is in zen regardless of grid_view.
fn current_screen(&self) -> Screen {
    if !self.app.chrome_visible {
        Screen::Zen
    } else if self.grid_view {
        Screen::Grid
    } else {
        Screen::Workspace
    }
}
```

Notes:
- `chrome_visible == false` means zen (see `src/app.rs:418` doc comment).
- Must be callable from `view.rs`. `run_global_shortcut` is a private method on
  the same type used from `update.rs`; `view.rs` methods are on the same `Grove`
  type in the same crate, so a private method is reachable. If a visibility
  error appears, mark it `pub(crate)` (and `Screen`/`Scope`/`ShortcutDef`/
  `SHORTCUTS` `pub(crate)` too) so `view.rs` can use them. Prefer the smallest
  visibility that compiles.

**Verify:** unit test in `mod tests` is awkward (needs a `Grove`); instead assert
the ordering logic by inspection + `cargo build`. Optionally add a small pure
helper `fn screen_from_flags(chrome_visible: bool, grid_view: bool) -> Screen`
that `current_screen` delegates to, and unit-test that:

```rust
#[test]
fn screen_zen_wins_over_grid() {
    assert_eq!(screen_from_flags(false, true), Screen::Zen);
    assert_eq!(screen_from_flags(false, false), Screen::Zen);
    assert_eq!(screen_from_flags(true, true), Screen::Grid);
    assert_eq!(screen_from_flags(true, false), Screen::Workspace);
}
```

(If adding the pure helper, `current_screen` becomes
`screen_from_flags(self.app.chrome_visible, self.grid_view)`.)

---

## Step 5 — Add a `Msg` to open the overlay and wire its handler

**File:** `src/gui/state.rs:332` `Msg` enum, and `src/gui/update.rs` update match.

Today the overlay is opened only by the keyboard path
(`run_global_shortcut` -> `Modal::ShortcutOverlay` at `:1745`). The chip needs a
`Msg`. Add:

```rust
// in Msg enum (state.rs)
/// Open the keyboard-shortcuts overlay (status-bar chip / cmd+/).
OpenShortcutOverlay,
```

Handler in `update.rs` (near `Msg::OpenSettings =>` at `:959`):

```rust
Msg::OpenShortcutOverlay => {
    self.app.modal = Modal::ShortcutOverlay;
    Task::none()
}
```

Optional cleanup: have `run_global_shortcut`'s `ShortcutOverlay` arm delegate via
`self.update(Msg::OpenShortcutOverlay)` for a single open-path, mirroring how
`NewSession`/`Settings` already delegate to `self.update(...)`. Keep it if it
compiles cleanly; otherwise leave the direct assignment.

**Verify:** `cargo build`. Behavior: no visible change yet (chip added next).

---

## Step 6 — Status-bar `⌘/ shortcuts` chip

**File:** `src/gui/view.rs:1749`, the `right` row of `statusbar`.

Build the chip label from the registry's `ShortcutOverlay` entry so it never
drifts, prepend the platform modifier, and make it clickable via a `button` with
`on_press(Msg::OpenShortcutOverlay)`. Place it left of the version label.

```rust
let modifier = if cfg!(target_os = "macos") { "cmd" } else { "ctrl+shift" };
// Pull the key label from the registry (single source of truth).
let overlay_key = SHORTCUTS
    .iter()
    .find(|d| d.action == Some(GlobalShortcut::ShortcutOverlay))
    .map(|d| d.display_keys)
    .unwrap_or("/");
let shortcuts_chip = button(
    text(format!("{modifier}+{overlay_key}  shortcuts"))
        .size(11)
        .color(c::FG_DIM()),
)
.padding(Padding::from([0, 6]))
.on_press(Msg::OpenShortcutOverlay)
.style(|_, status| {
    // Dim text on BG_STRIP, matching the existing statusbar chrome; no border
    // unless hovered (mirror the existing `bypass`/tab chip styling vocabulary).
    button::Style {
        background: None,
        text_color: if matches!(status, button::Status::Hovered) {
            c::FG()
        } else {
            c::FG_DIM()
        },
        ..Default::default()
    }
});

let right = row![
    shortcuts_chip,
    Space::with_width(12),
    text(format!("v{}", env!("CARGO_PKG_VERSION")))
        .size(11)
        .color(c::FG_DIM()),
]
.align_y(iced::Alignment::Center);
```

Notes:
- `SHORTCUTS`, `GlobalShortcut`, and `ShortcutOverlay` must be importable in
  `view.rs`. Add `use super::update::{...}` or the appropriate path; if they are
  `pub(crate)` (Step 4 fallback), reference them fully. Confirm the exact module
  path (`crate::gui::update`).
- Match the existing chip idiom: the `bypass` chip (`:1721`) uses a bordered
  container; the terminal tab (`:1074`) uses a `button` with hover styling. The
  spec says "styled like existing statusbar chips (dim text on `BG_STRIP`)", so a
  borderless hover-brightening button is the closest fit. Keep it text-only
  (bundled fonts lack modifier-symbol glyphs — see `shortcut_overlay_modal`'s
  own comment at `:3263`; use the word `cmd`, not `⌘`).

**Verify:** `cargo build`; run the app (`cargo run`) and confirm the chip renders
on grid/workspace/zen-with-chrome, sits left of `vX.Y.Z`, and clicking it opens
the overlay. On non-mac the label reads `ctrl+shift+/  shortcuts`.

---

## Step 7 — Registry-driven, conditionally-grouped overlay

**File:** `src/gui/view.rs:3265` `shortcut_overlay_modal`.

Replace the hardcoded `entries` array with a filtered projection of `SHORTCUTS`,
plus the two static display-only rows (copy/paste, esc) that the registry
deliberately omits (Step 2 note). Group ONLY when the visible shortcuts span more
than one scope; today all are `Global`, so render a flat headerless list identical
to the current overlay.

```rust
let m = if cfg!(target_os = "macos") { "cmd" } else { "ctrl+shift" };
let screen = self.current_screen();

// Registry entries visible on this screen: Global or matching current screen.
let visible: Vec<&ShortcutDef> = SHORTCUTS
    .iter()
    .filter(|d| {
        d.scopes.iter().any(|s| {
            matches!(s, Scope::Global) || *s == Scope::Screen(screen)
        })
    })
    .collect();

// Does the visible set span more than one scope? (Global vs current-screen)
let has_global = visible.iter().any(|d| d.scopes.contains(&Scope::Global));
let has_screen = visible
    .iter()
    .any(|d| d.scopes.contains(&Scope::Screen(screen)));
let grouped = has_global && has_screen;
```

Build a helper closure that renders one `(keys, desc)` row exactly as today
(fixed-width cyan key label + dim description), then:

- **Flat case (`!grouped`, today):** collect rows for `visible` (formatting the
  key as `format!("{m}+{}", d.display_keys)`), append the two static rows
  `("{m}+c / {m}+v", "copy / paste in session")` and `("esc", "close modals")`,
  split into two columns with `div_ceil(2)` exactly like the current code, and
  render with NO section header. This must be byte-for-byte equivalent in
  appearance to the current overlay.

- **Grouped case (future, when a `Screen(_)` shortcut exists):** render a
  `global` section (its rows + the static copy/paste/esc rows) then a
  `screen.label()` section with the screen-scoped rows. Each section gets a small
  dim header (`text(section_label).size(11).color(c::FG_MUTE())`). **Never render
  an empty section** — the `grouped` flag already guarantees both sides are
  non-empty, but still guard each section's row count before pushing its header.

Keep the outer frame unchanged: `text("keyboard shortcuts")` header, the
`Space::with_height(4)`, `text("esc to close")` footer, and
`modal_panel(body.into(), 640.0, c::MAGENTA())`.

Illustrative row helper (unchanged styling from `:3288`):

```rust
let make_row = |keys: String, desc: &str| {
    row![
        container(text(keys).size(11).color(c::CYAN()))
            .width(Length::Fixed(170.0)),
        text(desc).size(11).color(c::FG_DIM()),
    ]
    .spacing(10)
    .align_y(iced::Alignment::Center)
};
```

Notes:
- The static copy/paste + esc rows are display-only extras (Step 2). Keep them in
  the `global` group so the flat list is identical to today's ordering: registry
  rows first, then `copy / paste`, then `esc`. Confirm against the current
  `entries` ordering at `:3271` and preserve it.
- `display_keys` for `1–9` is `"1–9"`; formatted it becomes `"cmd+1–9"`, matching
  the old `"{m}+1..9"` intent (spec shows `1–9`).

**Verify:** `cargo build`; run and open the overlay on each screen — it must look
identical to before (flat, no headers), with correct platform modifier. Confirm
via a temporary assertion or manual check that `visible` contains all 11 registry
rows on every screen (all Global).

---

## Step 8 — Full check must pass

Run the project's checks. No `justfile`/`Makefile` exists; README documents a
Cargo/`cargo-bundle` workflow, so use Cargo directly:

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings   # if the repo treats clippy as gating
```

**Verify:** `cargo build` completes with no errors; `cargo test` passes,
including the updated `match_global_shortcut` assertions (Step 3) and the
`screen_from_flags` test (Step 4). Manually confirm the chip + overlay behavior
described in Steps 6–7.

---

## Out of scope (do not add)

- Re-scoping any existing shortcut to a specific screen.
- New shortcuts, new modals, or icon glyphs for modifiers.
- Persisting anything; `Screen` is derived, not stored.
