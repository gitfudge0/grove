# Drag-resizable sidebar — design

## Goal
Let the user resize the sidebar by dragging the divider between the sidebar and
the workspace. Width persists across restarts, is clamped to sane bounds, and
keeps the PTY column math correct at any width.

## States covered
- **Default** — no persisted width → `RAIL_W` (320px).
- **Persisted** — width loaded from `Store.sidebar_width` on launch.
- **Dragging** — divider grabbed; width tracks the cursor live (visual only).
- **Drag end** — width committed: PTY grid recomputed, width persisted.
- **Clamped (bounds)** — width clamped to `[SIDEBAR_MIN_W, window-relative cap]`.
- **Clamped (window)** — on shrink, width re-clamped so the workspace keeps a
  usable minimum; upper cap = `min(0.5 * logical_win_w, logical_win_w - WORKSPACE_MIN_W)`.
- **Reset** — double-click the divider restores `RAIL_W`.
- **Hover** — divider shows the horizontal-resize cursor.
- **Views** — Tree / Activity / Terminal sidebars all read the same width.
- **Chrome hidden (zen)** — sidebar absent; width contributes 0 to PTY math (unchanged).

## Coordinate space
`ui_zoom` is applied as iced's `scale_factor`, so every widget-space coordinate
(`mouse_area` positions, `CursorMoved.position`) is in *logical* units — the same
space as layout widths and `RAIL_W`. No zoom scaling of the drag delta is needed.
`window_size` is stored as physical (`logical * ui_zoom`); the logical window
width used for clamping is `window_size.width / ui_zoom`.

## Changes
- **metrics.rs**: `SIDEBAR_MIN_W = 220.0`, `WORKSPACE_MIN_W = 400.0`; bump
  `SIDEBAR_DIVIDER_W` to `6.0` (the grab-handle width; pty tests reference the
  symbol so they stay green). Add `clamp_sidebar_width(width, logical_win_w)`.
  `compute_pty_dims` / `pty_cols_for_fraction` gain a `sidebar_w` parameter
  replacing the hardcoded `RAIL_W`.
- **storage.rs**: `Store.sidebar_width: Option<f32>` (`#[serde(default)]`).
- **state.rs**: `Grove.sidebar_width: f32`, `Grove.sidebar_drag: Option<SidebarDrag>`,
  `Grove.last_divider_press: Option<Instant>`; `struct SidebarDrag { grab_offset: Option<f32> }`;
  `Msg::SidebarDragStart | SidebarDragMove(f32) | SidebarDragEnd`.
- **update.rs**: load + clamp width in `new()`; pass `sidebar_width` into the pty
  helpers in `refresh_pty_viewport`; re-clamp on `WindowResized`; conditional mouse
  subscription active only while dragging (maps `CursorMoved` → `SidebarDragMove`,
  `ButtonReleased(Left)` → `SidebarDragEnd`); drag/reset/persist handlers.
- **view.rs**: appbar brand and sidebar containers use `self.sidebar_width`;
  replace the static `divider_v` with a draggable handle (`mouse_area` +
  `Interaction::ResizingHorizontally`, centered 1px line in a 6px hit zone).

## Drag mechanics
A thin divider can't drive `mouse_area::on_move` (it fires only while hovered, and
the cursor leaves a 1px strip instantly). Instead: `on_press` → `SidebarDragStart`;
while `sidebar_drag` is set, a global event subscription feeds `CursorMoved` and
the mouse-up. `grab_offset` (lazily set on the first move as `width - cursor_x`)
prevents a jump when the press lands a few px off the exact edge.
Double-click reset uses a press-timestamp window (<350ms) tracked in `update`.

## Tests
- `clamp_sidebar_width`: below-min, above window cap, window-constrained, pass-through.
- `compute_pty_dims` / `pty_cols_for_fraction`: wider sidebar → fewer cols; hidden
  chrome ignores sidebar width.
- `storage`: round-trips `sidebar_width`; absent field → `None`.
