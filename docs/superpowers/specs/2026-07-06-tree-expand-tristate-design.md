# Three-state tree expand/collapse

Date: 2026-07-06

## Problem

The sidebar tree header has a single toggle that flips between two states:
"collapse all except worktrees with sessions" and "expand all". Users want a
third, fully-collapsed state and a predictable cycle between all three.

## Behavior

A single header button cycles through three modes on click:

1. **Collapsed** — every project row collapsed; you see only the project list.
2. **SessionsOnly** — projects/worktrees with no sessions are collapsed, the
   rest expanded (today's "collapse-all" behavior).
3. **All** — everything expanded.

Cycle ring: `Collapsed → SessionsOnly → All → Collapsed`.

The glyph shows the **next action** (what the next click will do):

| Current mode | Glyph            |
|--------------|------------------|
| Collapsed    | `expand-sessions`|
| SessionsOnly | `expand-all`     |
| All          | `collapse-all`   |

## State model

Add an explicit tri-state field, replacing the fragile
`is_collapsed_to_sessionful_worktrees()` inference:

```rust
enum TreeExpand { Collapsed, SessionsOnly, All }
```

- Stored on the GUI state; default `All` (matches fresh startup where nothing
  is collapsed).
- `Msg::ToggleCollapseAll` advances the ring, then rebuilds
  `collapsed`/`collapsed_wt` to match the new mode:
  - **Collapsed**: insert every project index into `collapsed`; clear
    `collapsed_wt`.
  - **SessionsOnly**: collapse projects without a sessionful worktree and
    worktrees without sessions; expand the rest.
  - **All**: clear both sets.

Manual per-row toggles (`ProjectClicked` / `WorktreeClicked`) keep working and
do **not** change the mode. Each button click fully overrides any manual row
state — this is intended and predictable.

## New icon

`expand-sessions`: based on `expand-all` but reading as "partial" — one open
chevron stacked over one closed chevron, plus the existing label hatch marks.

## Scope

- `state.rs` — enum + field.
- `update.rs` — cycle logic replaces the if/else in `ToggleCollapseAll`;
  `is_collapsed_to_sessionful_worktrees()` retired.
- `view.rs` — glyph selection reads the mode.
- `icons.rs` — new `expand-sessions` glyph.

No new keybinding.
