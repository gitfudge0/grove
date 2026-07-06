# Screen-scoped shortcuts + bottom-bar hint

**Date:** 2026-07-06
**Status:** Approved design, pending implementation plan

## Problem

1. The keyboard-shortcuts overlay is discoverable only by already knowing its
   shortcut (`cmd+/` on macOS, `ctrl+shift+/` elsewhere). There is no on-screen
   hint pointing users to it.
2. Shortcuts are declared twice and kept in sync by hand:
   - behavior: `GlobalShortcut` enum + `match_global_shortcut` in `src/gui/update.rs`
   - help text: a hardcoded `entries` array in `shortcut_overlay_modal`
     (`src/gui/view.rs:3271`).
   These drift apart.
3. The overlay shows one flat list with no notion of which screen the user is on,
   so it cannot grow into per-screen guidance later.

## Goals

- Add a **clickable hint chip** in the bottom status bar that opens the shortcuts
  overlay, with a key label that matches the real binding.
- Introduce a **data-driven shortcut registry** as the single source of truth for
  both matching and display.
- Give the registry a **scope** concept (Global / per-screen) and have the overlay
  render a "global" section plus a section for the current screen.

## Non-goals

- Re-scoping existing shortcuts. After review, every current shortcut is
  genuinely global (they must work from anywhere to *enter* a mode; zoom stays
  global too). The registry gains full scope *support*, but the current-screen
  section is empty until screen-scoped shortcuts are added in future work.

## Design

### Screen model (coarse)

```rust
enum Screen { Grid, Workspace, Zen }

fn current_screen(&self) -> Screen {
    if !self.app.chrome_visible { Screen::Zen }
    else if self.grid_view      { Screen::Grid }
    else                        { Screen::Workspace }
}
```

Derived from existing flags (`app.chrome_visible` = zen, `grid_view`). When a modal
is open, the underlying screen is still whatever this computes.

### Shortcut registry (single source of truth)

Replaces the hardcoded matcher `match` arm and the modal's `entries` array.

```rust
enum Scope { Global, Screen(Screen) }

struct ShortcutDef {
    action: GlobalShortcut,
    triggers: &'static [&'static str], // key chars matched, e.g. ["=", "+"]
    display_keys: &'static str,        // shown in overlay, modifier prepended at render
    description: &'static str,
    scopes: &'static [Scope],          // a shortcut may apply to several screens
}

const SHORTCUTS: &[ShortcutDef] = &[ /* one row per shortcut */ ];
```

- `match_global_shortcut(key, mods)` iterates `SHORTCUTS`, matching the platform
  modifier gate (`global_mods()`) + key against `triggers`, and returns the action.
- `SelectNth(1..9)` stays special-cased in the matcher (dynamic `n`), represented
  in the registry by a single display-only row (`"1–9"`, Global) so it appears in
  the overlay without duplicating logic.
- Copy/paste keep their existing dedicated matchers (`is_copy_shortcut` /
  `is_paste_shortcut`); they are not global-modifier shortcuts.

### Current shortcuts (all Global for now)

`n` new session · `,` settings · `g` toggle grid · `=`/`+` zoom in ·
`-`/`_` zoom out · `0` reset zoom · `j` next session · `k` prev session ·
`/`/`?` shortcuts overlay · `1`–`9` select nth session · `Enter` toggle zen.

### Bottom-bar hint chip

In `statusbar()` (`src/gui/view.rs:1670`), add a chip immediately left of the
`v{version}` label on the right:

- Label: `{modifier}/  shortcuts` — modifier from the registry's `ShortcutOverlay`
  entry (`⌘` / `cmd` on macOS, `ctrl+shift` elsewhere) so it never drifts.
- Styled like existing statusbar chips (dim text on `BG_STRIP`).
- **Clickable**: emits a `Msg` that opens the overlay via the same path as the
  keyboard shortcut (`Modal::ShortcutOverlay`).

### Overlay grouping (`shortcut_overlay_modal`, view.rs:3265)

- Filter `SHORTCUTS` to entries whose `scopes` contain `Global` or
  `Screen(current_screen())`.
- Render a **"global"** section, then a section titled by the current screen
  (`"grid"` / `"workspace"` / `"zen"`) only when it has entries (none today).
- Preserve the existing two-column layout, header, and "esc to close" footer.

## Affected files

- `src/gui/update.rs` — registry, matcher rewrite, click-to-open message wiring.
- `src/gui/view.rs` — statusbar chip, overlay grouping from registry.
- `src/app.rs` / `src/gui/state.rs` — `Screen` enum + `current_screen()` helper
  (placement TBD by plan).

## Testing

- `match_global_shortcut` returns the correct action for each registry trigger,
  including both aliases (`=`/`+`, `-`/`_`, `/`/`?`).
- `current_screen()` maps flag combinations correctly (zen wins over grid).
- Overlay filtering includes Global entries on every screen.
- Manual: chip renders on each screen, click opens overlay, key label matches
  platform.
