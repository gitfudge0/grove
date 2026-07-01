# Agent View — Session Launcher

**Date:** 2026-07-01
**Status:** Design approved (via `mock.html`), pending code-integration review

## Problem

Agent View is a fullscreen tiled grid of all running sessions. Today you can only
*start* a new session from the sidebar, which is hidden while Agent View is active.
To launch work against another worktree you must leave the grid, start the session,
then return. This feature lets you launch a session against **any worktree of any
project** without leaving Agent View.

## Design (as visualized in `mock.html`)

### Entry point — floating "+" pill

- A fixed **"+ New session"** pill anchored bottom-right of the grid workspace,
  floating *above* the tiles.
- It does **not** participate in grid packing, so the tile layout never reflows when
  the session count crosses a perfect-square boundary (the reason we rejected an
  in-grid "+" tile).
- Opened by click or the `Cmd/Ctrl+N` hotkey while Agent View is active.

### Launcher — progressive (Miller) columns

A centered modal overlay with three columns, left → right:

1. **Project** — every known project (`name` + parent path, worktree count). Selecting
   one populates column 2.
2. **Worktree** — the selected project's worktrees. Each row shows the branch name, a
   dirty/clean dot, and tags (`main`, `running` when a session already exists there).
   A **"+ New worktree…"** row at the bottom hands off to Grove's existing
   worktree-name modal, then returns with the new worktree selected.
3. **Agent** — `claude`, `codex`, `opencode`, `terminal` (the `Agent` enum). Below the
   agent list:
   - a **Skip permission prompts** toggle (feeds `spawn_session`'s skip-permissions
     path / `launch_args`),
   - an **optional label** field (defaults to `agent · branch` when empty).

### Footer

- A live **breadcrumb**: `project › branch › agent`.
- A **Start session** button.
- Keyboard hint: `←/→` switch columns · `↑/↓` move within a column · `Enter` start ·
  `Esc` close.

### Launch behavior

On Start (button or Enter): the overlay closes, `spawn_session` is invoked with the
selected project, worktree path, agent, skip-permissions, and label; the new session
auto-appends to `tile_order` and receives focus (same path newly-spawned sessions
already take when Agent View is active).

### Default selection

When opened, the launcher pre-selects a sensible default (the currently-active
session's project + worktree, agent `claude`) so `Enter` immediately does something
reasonable.

## Non-goals (YAGNI)

- No fuzzy-search box (progressive columns chosen over a flat searchable list).
- No multi-select / batch launch.
- No reordering of projects/worktrees from the launcher.
- Empty-state (Agent View with zero sessions) treatment is out of scope for this pass;
  the pill remains the entry point.

## Integration (confirmed against code)

**New `Modal` variant** — `Modal::SessionLauncher` in `src/app.rs` (enum ~L145–241),
carrying the launcher's transient state:
```rust
SessionLauncher {
    proj: usize,        // index into store.projects
    wt: usize,          // index into the selected project's worktrees
    agent: usize,       // index into available_agents
    col: u8,            // focused column: 0=project 1=worktree 2=agent
    skip_perms: bool,   // defaults to skip_permissions_enabled()
    label: String,      // optional; empty => default naming
}
```
Rendered via the existing modal dispatch in `view.rs` `modal_layer()` (~L1700), so it
overlays above the grid like every other modal. A new `session_launcher_modal(...)`
render fn mirrors `agent_picker_modal()` (view.rs ~L2373) but with three Miller columns.

**Floating "+" pill** — added to `grid_workspace()` (view.rs ~L809) via `stack![grid,
pill]`, anchored bottom-right, `on_press` → `Msg::OpenSessionLauncher`. It lives in the
grid layer, not the tile flow, so packing never reflows.

**All-project worktrees** — `store.projects` is always in memory. On open, ensure each
project's worktrees are available: reuse `wt_cache` (state.rs ~L47) for non-selected
projects and `app.worktrees` for the selected one, loading via `git::list_worktrees`
where the cache misses. Selecting a project in column 1 lazily loads its worktrees.

**Agents** — use `app.available_agents` (already scanned for the sidebar picker), same
as `agent_picker_modal`. Trigger the same scan when the launcher opens.

**Spawn** — on Start, call `App::spawn_session(label, project, wt_path, agent, args,
cwd)` (app.rs:662) with `args = agent.launch_args(skip_perms)`. Label defaults to the
worktree basename / project name (existing convention in `spawn()`, update.rs ~L2116)
when the field is empty. Then replicate the grid-append that `submit_agent_picker` does
(update.rs ~L1477): push the new session index onto `tile_order` and set
`grid_focused`.

**New `Msg` variants** (state.rs, near other grid msgs ~L533):
`OpenSessionLauncher`, `LauncherSelectProject(usize)`, `LauncherSelectWorktree(usize)`,
`LauncherSelectAgent(usize)`, `LauncherToggleSkip`, `LauncherLabelChanged(String)`,
`LauncherNewWorktree`, `LauncherStart`. (Close reuses `ModalCancel`.)

**Keyboard** — `handle_modal_key` (update.rs ~L1606) gets a `Modal::SessionLauncher`
arm: `Esc`→cancel, `Enter`→start, `←/→`→change `col`, `↑/↓`→move within focused column
(and, on the project column, reset `wt` + lazily load that project's worktrees). The
label text field is a normal focused `text_input`, so typing routes to it (captured
events are skipped by the subscription). `Cmd/Ctrl+N` while `grid_view` is active opens
the launcher — handled in the non-modal key path (update.rs `handle_key` ~L1483).

**"+ New worktree…"** — `LauncherNewWorktree` closes the launcher and opens Grove's
existing worktree-name `Modal::Input` flow for the selected project; after the worktree
is created we re-open the launcher with it selected. (If the round-trip proves fiddly,
fall back to selecting the new worktree without auto-reopening — tracked in the plan.)
