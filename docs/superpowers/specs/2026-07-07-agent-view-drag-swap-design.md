# Agent view drag-drop: swap instead of reorder

## Problem

In Agent View, dragging a session tile and dropping it on another tile
currently uses remove-then-insert semantics (`reorder_tiles` in
`src/gui/launcher.rs`). Dragging tile 2 onto tile 4 in `[1,2,3,4]` shifts
everything between them, producing `[1,3,4,2]` — tiles 3 and 4 each move
one slot instead of tile 2 and tile 4 trading places.

Desired behavior: dragging tile 2 onto tile 4 should swap them in place,
producing `[1,4,3,2]`.

## Change

Replace `reorder_tiles` with a direct swap:

```rust
pub fn swap_tiles(order: &mut Vec<usize>, src: usize, dst: usize) {
    if src == dst || src >= order.len() || dst >= order.len() {
        return;
    }
    order.swap(src, dst);
}
```

File: `src/gui/launcher.rs:75-81` (replaces `reorder_tiles`).

## Call site

`src/gui/update.rs:1144-1155`, the `GridDragEnd` handler — change the call
from `reorder_tiles` to `swap_tiles`. Everything else in that handler
(persisting `tile_order` via `persist_grid_order`, resizing PTYs via
`refresh_pty_viewport`) is unchanged.

## Drag preview

No change to `GridDragHover`, the drop-zone inset, or the dim overlay on
the source tile (`src/gui/view.rs:1579-1791`). The swap is only applied on
release (`GridDragEnd`), not previewed live while hovering.

## Tests

`src/gui/launcher.rs:180-197` has `reorder_tiles_inserts_instead_of_swapping`,
which documents and asserts the shift behavior. Replace it with a test
asserting swap behavior, e.g. dragging index 0 onto index 3 in
`[0,1,2,3,4]` produces `[3,1,2,0,4]` (positions 0 and 3 trade places;
1,2,4 unchanged) — not `[1,2,3,0,4]` (the old shift result).

## Scope

Single function body, single call site, single test. No state, message,
or rendering changes beyond the call-site swap.
