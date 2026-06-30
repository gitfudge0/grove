# Agent View — Design Spec
date: 2026-06-30

## Overview

Agent view is a fullscreen tiled mode that renders all running agent sessions as equal-sized PTY tiles. It replaces the single-session workspace while active, giving the user a command-centre overview of every live agent without leaving Grove.

## Entry and Exit

An appbar grid icon button (`Msg::ToggleGridView`) toggles agent view on and off.

- Entering hides the sidebar entirely (same mechanism as zen mode's `chrome_visible` path, but sidebar-scoped).
- Exiting restores the sidebar in whatever view (tree/activity/term) it was in before.
- If all sessions are killed while in grid view, the app auto-exits to normal single-session view.
- No keyboard shortcut for entry/exit — the appbar button is the sole entry point. PTY-takes-everything input routing makes a shortcut impractical without a modifier prefix, which adds complexity not worth the tradeoff now.
- Zen mode is reachable from any tile (see Focus section). When zen is exited, the app returns to grid view rather than normal view.

## Grid Layout

Tile count drives the grid dimensions:

| Sessions | Grid |
|---|---|
| 1 | 1×1 |
| 2 | 2×1 |
| 3–4 | 2×2 |
| 5–6 | 3×2 |
| 7–9 | 3×3 |
| 10–12 | 4×3 |
| 13+ | 4×4 (excess sessions hidden) |

Formula: `cols = ceil(sqrt(n))`, `rows = ceil(n / cols)`. All tiles are equal size.

PTY resize fires on:
- Grid view entered or exited
- A session is spawned or killed while in grid view
- Window is resized

Each tile's PTY dimensions: `workspace_cols / cols` columns, `workspace_rows / rows` rows (integer division, same derivation as the existing `pty_cols` / `pty_rows`).

Newly spawned sessions auto-append to `tile_order` and trigger a grid reflow.

## State

New fields on `Grove`:

```rust
/// Whether agent view (fullscreen tiled grid) is active.
pub grid_view: bool,

/// Display order of sessions in the grid. Indices into `app.sessions`.
/// Populated on grid entry; sessions append on spawn, prune on kill.
pub tile_order: Vec<usize>,

/// Which tile currently has keyboard focus. None = no tile focused.
pub grid_focused: Option<usize>,

/// Active tile drag, if the user is dragging a tile header.
pub grid_drag: Option<GridDrag>,

/// Whether the app was in grid view when zen mode was entered, so
/// exiting zen returns to grid rather than normal view.
pub grid_view_before_zen: bool,
```

```rust
pub struct GridDrag {
    /// Index into `tile_order` for the tile being dragged.
    pub source_idx: usize,
    /// Index into `tile_order` for the tile currently under the cursor.
    pub hover_idx: usize,
}
```

## Focus and Input

**Clicking a tile** sets `grid_focused` to that tile's session index. The sidebar's active session also updates to match, so exiting grid view drops the user into the focused session.

**Visual:** Focused tile gets a 1.5px cyan outline. All other tiles render at ~65% opacity. No opacity effect when `grid_focused` is `None`.

**Keyboard input:** When `grid_focused` is `Some`, all `KeyPress` messages route to that session's PTY — identical to single-session routing. No Grove shortcuts are intercepted while a tile is focused.

**Mouse scroll / selection:** Routes to whichever tile is under the cursor, independent of `grid_focused`. Scroll does not change focus.

**Unfocusing:** Clicking the appbar clears `grid_focused`. There is no sidebar to click while in grid view.

**Exiting grid view:** If `grid_focused` is `Some` when the user toggles grid view off, that session becomes the active session in the normal workspace. If `grid_focused` is `None`, the previously active session (before grid was entered) is restored.

**Zen from grid:** The ⤢ expand icon appears in the top-right of a tile's header on hover. Clicking it sets `grid_view_before_zen = true` and enters zen mode for that session via the existing `ToggleZen` path. Exiting zen checks `grid_view_before_zen` and returns to grid view.

## Tile Structure

Each tile is a column of:

1. **Header (20px):** `cursor: grab` — the only draggable surface.
   - Running indicator dot (green/yellow)
   - Agent name (bold, `fg-dim`)
   - `·` separator
   - Project / branch (`fg-mute`, truncated)
   - ⤢ expand icon (hidden until tile hover, top-right)

2. **PTY canvas:** fills remaining height. Uses the existing canvas-based PTY renderer, resized to tile dimensions.

## Drag to Reorder

Drag initiates from the tile header only. The PTY area is reserved for text selection.

**Messages:**
- `Msg::GridDragStart(idx)` — press on header, sets `grid_drag`
- `Msg::GridDragHover(idx)` — cursor moves over another tile
- `Msg::GridDragEnd` — button release; swaps `tile_order[source_idx]` and `tile_order[hover_idx]`, clears `grid_drag`
- `Msg::GridDragCancel` — Escape or release outside any tile; clears `grid_drag` with no swap

**Visual:**
- Source tile: 25% opacity
- Ghost: cyan-bordered clone of the tile (header + partial PTY content), follows the cursor, `box-shadow` for lift
- Drop target: dashed cyan inset overlay on the hover tile
- Drop: tiles snap immediately, no animation

A global mouse subscription is active while `grid_drag` is `Some`, using the same pattern as `sidebar_drag` and `term_panel_dragging`.

## New Messages

```rust
ToggleGridView,
GridDragStart(usize),   // tile_order index
GridDragHover(usize),   // tile_order index
GridDragEnd,
GridDragCancel,
```

## Out of Scope

- Keyboard shortcut for grid entry/exit
- Persisting tile order to disk
- Resizable tiles (drag dividers between tiles)
- Per-project or per-worktree filtered grid views
- Scrolling for >16 sessions (4×4 cap; rare in practice)
