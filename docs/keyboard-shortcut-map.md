# Keyboard shortcut map

Map of every binding × every screen, current as of the Bug 7 fix (registry
`.find()` now honors `requires_alt`). `mod` = `Cmd` on macOS, `Ctrl+Shift`
elsewhere.

## Dispatch spine

Every key enters through one funnel: `subscription()` (`src/gui/update.rs:122`)
→ `Msg::KeyPress` → `handle_key()` (`update.rs:1682`), with one bypass (guard 0
below). There is no per-screen handler; screens only change *which PTY* the
fallthrough writes to.

The subscription drops any event iced already marked `Status::Captured`,
except `Escape`: a focused `text_input` captures `Escape` to blur itself
(`iced_widget-0.13.4/src/text_input.rs`) without telling the app, so `Escape`
is forwarded regardless of capture status (`update.rs:129`-`139`) — this is
what makes a single `Escape` press cancel a modal even while a field is
focused.

Guard order inside `handle_key`, first match wins:

| # | Guard | Location | Consumes |
|---|-------|----------|----------|
| 0 | `Modal::RemoveProject` bypass — routed from `update()`, never reaches `handle_key` | `update.rs:547` | all keys |
| 1 | `show_changelog` | `update.rs:1684` | all keys (Esc closes) |
| 2 | any `Modal` open → `handle_modal_key` | `update.rs:1691` | all keys |
| 3 | no-modal Escape carve-out: dismisses armed `pending_kill` / `open_agent_menu` | `update.rs:1699` | bare `Escape` only, and only when one is armed (`Alt+Escape` still reaches the PTY as `ESC ESC`) |
| 4 | copy shortcut | `update.rs:1710` | `mod+C` |
| 5 | paste shortcut | `update.rs:1717` | `mod+V` |
| 6 | `match_global_shortcut` | `update.rs:1751` | the `mod+…` set (registry-scoped) |
| 7 | term-panel resize, gated on `term_panel_open` only | `update.rs:1761` | `Ctrl+Shift+←/→`, Workspace only |
| 8 | fallthrough → `key_to_bytes` → focused PTY | `update.rs:1770` | everything else |

Guard 2 means **no global shortcut works while any modal is open.** The one
exception is `ShortcutOverlay`, which re-checks `match_global_shortcut` itself
so `mod+/` can close it (`update.rs:2168`, inside `handle_modal_key`).

Guard 3 only fires with no modal open — `pending_kill` (armed kill-confirm on
a grid tile) and `open_agent_menu` (split-agent menu) were previously
mouse-only to clear; with neither armed, `Escape` falls through past guard 3
to the PTY, unchanged from before.

## Global shortcuts (`SHORTCUTS` registry, `update.rs:3140`)

| Keys | Action | Declared scope | Actually scoped? |
|------|--------|----------------|------------------|
| `mod+n` | new session | global | yes |
| `mod+alt+n` | new session in current worktree | global | yes |
| `mod+j` / `mod+k` | next / prev session | global | yes |
| `mod+1`…`mod+9` | select nth session | global | yes |
| `mod+g` | toggle grid view | global | yes |
| `mod+Enter` | toggle zen | global | yes |
| `mod+,` | settings | global | yes |
| `mod+=` / `mod+-` / `mod+0` | zoom in / out / reset | global | yes |
| `mod+/` or `mod+?` | shortcut overlay | global | yes |
| `mod+w` | close focused session | global | yes — target resolved per screen in `run_global_shortcut`: focused tile in Grid, active session elsewhere |
| `mod+c` / `mod+v` | copy selection / paste into PTY | global | yes |
| `Ctrl+Shift+←/→` | resize terminal panel | **Workspace only**, in the registry as a display-only row | yes, but hand-kept in sync — see Known issues |

`match_global_shortcut` (`update.rs:3191`) also special-cases
`mod+alt+n` *before* the registry lookup, via `new_session_in_worktree_mods`
(`update.rs:3049`). That early check is still required on non-mac: there,
`global_mods` (`update.rs:3039`) requires Ctrl+Shift, which Ctrl+Alt+N (no
Shift) never satisfies, so non-mac can't reach the registry lookup for this
chord any other way. On mac the early check is redundant (Cmd+Alt+N already
satisfies `global_mods`, so the registry `.find()` — now `requires_alt`-aware —
would resolve it unaided), but it's kept for symmetry and because deleting a
platform-conditional branch to save one redundant path on one platform isn't
worth the risk.

## Non-modal screens

`Screen` is derived from two flags (`screen_from_flags`, `update.rs:3166`):
`chrome_visible` and `grid_view`. Zen wins over grid.

| Screen | Condition | Fallthrough PTY target |
|--------|-----------|------------------------|
| Workspace | chrome visible, `!grid_view` | see routing below |
| Grid / agent view | chrome visible, `grid_view` | `grid_focused` tile, else none |
| Zen | `!chrome_visible` | as workspace/grid |

PTY routing (`focused_session_mut`, `update.rs:2900`), strict priority:

1. `grid_view` → the `grid_focused` tile (`None` → keystroke dropped)
2. `sidebar_view == Terminal` → the home terminal
3. `term_panel_open && focused_pane == Panel` → worktree panel shell
   (falls back to the agent session if the worktree has no panel shell)
4. otherwise → the active agent session

Sidebar tree-vs-activity view does not gate key handling, but it *does* change
which session `mod+1..9` selects, because the numbering comes from
`visible_session_order()` (`view.rs:614`) — in tree view, sessions under a
collapsed project or worktree are excluded from the numbering (see Known
issues).

## Key translation to the PTY (`src/gui/keys.rs`)

`key_to_bytes` (`keys.rs:83`) receives `key` when Ctrl is held (so
control-byte math sees the base letter) and `modified_key` otherwise (so
Shift/AltGr text survives).

Handled: `Ctrl+<ascii letter>` → control byte, plain characters → UTF-8,
`Enter Tab Backspace Escape Space` and the four arrows, `Home End PageUp
PageDown Delete Insert`. `Alt` prepends `ESC`.

Silently dropped (returns `None`): every function key `F1`–`F12`, and every
other named key (see Known issues).

Modifier loss: `Key::Named` ignores Ctrl and Shift entirely. `Ctrl+→` sends the
same bytes as bare `→`. `Ctrl+Shift+L` sends the same byte as `Ctrl+L` (see
Known issues).

## Modals

16 variants including `None` (`src/app.rs:202`). Global shortcuts are dead in
all of them.

| Modal | Esc | Enter | Tab | Space | Arrows | Notes |
|-------|-----|-------|-----|-------|--------|-------|
| `Input` | cancel | submit | — | — | — | focused text field |
| `Confirm` | no | yes | — | — | — | `y`/`n` also; Quit variant exits immediately |
| `AddProject` | back/cancel | next/submit | dir-complete (step 1); no-op (step 2) | — | dir list ↑↓ | focused text field; Tab never switches field here |
| `RemoveProject` | cancel | **no** (deliberate) | — | toggle | — | separate handler, bypasses guard 1 |
| `Message` | close | close | — | — | — | |
| `TmuxChoice` | decline | **enable tmux** | — | — | — | Enter's default has no visual affordance |
| `AgentPicker` | cancel | submit | — | toggle default | ↑↓ move | |
| `SessionLauncher` | cancel | start | — | toggle default | ↑↓←→ move | only modal that prints its own key legend |
| `ThemePicker` | cancel | submit | switch tab | — | ↑↓ move | |
| `Settings` | close | **nothing** | — | — | — | |
| `ShortcutOverlay` | close | — | — | — | — | re-checks `mod+/` to self-close |
| `Teardown` | dismiss/skip, stage-dependent | **nothing** | — | — | — | same path as the Cancel button; no-op mid-removal |
| `ScriptsEditor` | cancel | **nothing** | — | — | — | same path as the Cancel button |
| `Updating` | close | **nothing** | — | — | — | disabled while an update is actively applying |
| `Onboarding` | skip | advance | switch field | — | ↑↓ on Project/Theme steps only | Tab targets the focused field (name if present, else path) |
| changelog overlay | close → Settings | — | — | — | — | not a `Modal`; own flag |

## Fixed

Each of these was a live bug in an earlier pass; kept here as a changelog so a
future regression has something to diff against.

1. **`mod+w` reached every screen but only did something in Grid.**
   It now closes the focused session everywhere: the focused tile in Grid, the
   active session otherwise (its sidebar row renders the same confirm-to-kill
   state, so the first press is visible in `Tree` and `Activity` alike).
   `run_global_shortcut` resolves the target per screen; the registry row is
   `Global` again. An earlier pass instead scoped the row to `Screen::Grid` —
   that made `match_global_shortcut` return `None` off-Grid, which is *not*
   inert: `handle_key` falls through to `key_to_bytes`, so Ctrl+Shift+W sent
   `0x17` (readline delete-word) on Linux and Cmd+W typed a literal `w` on
   macOS. `scope_allows` still gates the matcher, but no action row uses a
   narrow scope today.
2. **`Ctrl+Shift+←/→` resized an off-screen terminal panel from Grid/Zen.**
   `term_panel_resize_delta` now takes the current `Screen` and requires
   `Screen::Workspace`, and the chord has a display-only row in `SHORTCUTS` so
   it shows up in the `mod+/` overlay.
3. **Escape needed two presses on any modal with a focused text field.**
   The subscription now forwards `Escape` regardless of `Status::Captured`
   (`update.rs:129`), so the field-blur `Escape` that iced's `text_input`
   swallows internally no longer costs the app a keypress.
4. **The launcher's "+ New worktree…" opened an unfocused input.**
   `launcher_new_worktree` now returns `text_input::focus(modal_input_id())`
   like the launcher's other two entry points into the same modal.
5. **Grid focus desynced from the active session** when cycling with
   `mod+j`/`mod+k` while zenned in from a tile. `grid_focus_sync` is now a
   pure, tested function called on every active-session change, and keeps
   `grid_focused` following whenever the grid is showing or will show again on
   zen exit.
6. **Onboarding's Tab always targeted the path field.** `onboard_toggle_project_focus`
   now toggles between the name field (when one exists) and the path field,
   and Tab focuses whichever one it lands on.
7. **`NewSessionInWorktree`'s registry row was unreachable.** The registry
   `.find()` now requires `d.requires_alt == mods.alt()`, so `NewSession` and
   `NewSessionInWorktree` can no longer silently swap meaning by array order —
   only the modifier state decides. The `new_session_in_worktree_mods` early
   check stays (still required on non-mac; see the "Global shortcuts" section
   above), but the registry lookup is now correct independent of it.
8. **A keypress during a selection drag left the drag alive.**
   `clear_pty_selection` now clears both `pty_selection` and `pty_drag`.
9. **No keyboard escape from `pending_kill` or `open_agent_menu`.**
   `handle_key` now dismisses either (`escape_dismiss_target`) on an
   unmodified `Escape` before falling through to the PTY, when no modal is
   open.
10. **`Teardown` and `ScriptsEditor` trapped the keyboard.** Both now have an
    arm in `handle_modal_key`: `Escape` routes through the same path as their
    Cancel button.
11. **Docs drift.** `mod+alt+n`, `mod+w`, and `Ctrl+Shift+←/→` are now in
    `README.md`'s keyboard table. This file's line numbers and bug list are
    refreshed to match the current tree. Not addressed: `mockups/agent-view.html`
    still advertises "esc to exit grid/zen" (never implemented) and
    `mock-onboarding.html` still advertises a `⌘K` command palette that
    doesn't exist (`mod+k` is `PrevSession`) — both are static HTML mockups,
    not shipped UI, and out of scope here.

## Known issues

Real, currently live gaps. Not to be "fixed" as a side effect of touching
adjacent code — each needs its own deliberate change.

- **`key_to_bytes` drops every function key and loses modifiers on named
  keys.** `F1`–`F12` (and any other unhandled `Key::Named`) return `None`
  (`keys.rs:83`, the `_ => return None` arm). Ctrl and Shift are read only for
  `Key::Character`; a `Key::Named` ignores both entirely, so `Ctrl+→` sends
  the same bytes as bare `→`, and `Ctrl+Shift+L` sends the same byte as
  `Ctrl+L`.
- **`mod+1..9` numbering depends on sidebar view.** The nth-session shortcut
  resolves through `visible_session_order()` (`view.rs:614`), which returns a
  different order (and different membership) depending on
  `SidebarView::{Activity, Tree, Terminal}` — in `Tree`, sessions under a
  collapsed project or worktree aren't in the list at all, so the same digit
  can select a different session, or nothing, depending on what's collapsed.
- **The term-panel-resize registry row is display-only and hand-synced.**
  `SHORTCUTS`' `"resize terminal panel"` row (`update.rs:3161`) exists purely
  so `mod+/` can list the chord; the actual match happens in
  `term_panel_resize_delta` (`update.rs:3328`), which `handle_key` calls
  separately because a closed panel must fall through to the PTY rather than
  being consumed like every other registry-matched shortcut. The row's
  `scopes` has to be kept equal to `term_panel_resize_delta`'s `Screen::Workspace`
  check by hand — nothing enforces they agree if one changes without the
  other. This duplication was introduced by an earlier pass (Phase 1) and
  hasn't been collapsed into a single source of truth.
