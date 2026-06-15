# Worktree Lifecycle Scripts — Design

## Goal

Give every project three optional shell scripts that run at worktree lifecycle
points:

- **Setup** — runs when a new worktree is created.
- **Run** — runs on demand when the user clicks a button on a worktree.
- **Teardown** — runs when a worktree is deleted, before the git removal.

Scripts are defined **per project** (shared by all of that project's worktrees)
and stored as command strings in Grove's config. Empty/unset script = that
lifecycle step is a no-op.

## Data model

`storage.rs` gains a per-project scripts block, persisted in
`~/.config/grove/projects.json`:

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProjectScripts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teardown: Option<String>,
}
```

`Project` gains `#[serde(default)] pub scripts: ProjectScripts`. Serde defaults
keep existing config files loadable. An empty/whitespace-only string is treated
as `None` on save.

## Execution mechanism

All three reuse the existing PTY `Session` machinery. A new
`Session::spawn_script(label, project, wt_path, script, cwd)` builds a
`CommandBuilder` for the user's login shell with `-lc "<script>"`, runs it
**native** (never tmux — these are ephemeral and not worth persisting), and
otherwise flows through the existing `launch_pty`. Script sessions carry
`Agent::Terminal` so they reuse the terminal icon/handling; the label
(`setup` / `run` / `teardown`) distinguishes them.

- **Run** → a normal session tab under the worktree (`app.sessions`), focused.
  Long-lived processes (dev servers) keep running; one-shot scripts exit and
  leave the tab in `Exited` state. Fully interactive.
- **Setup** → a session tab under the freshly-created worktree. Same lifecycle
  as Run; it exits and stays visible until the user closes the tab.
- **Teardown** → not a sidebar session; runs inside a modal (below).

## Lifecycle wiring

- **Setup** — in `app.rs::create_worktree`, after `git::add_worktree` +
  `copy_worktree_includes` and before `launch_or_pick`: if the project's
  `scripts.setup` is set, spawn a setup session in the new worktree path. The
  existing auto agent-launch still happens; setup and the agent coexist as two
  tabs (non-blocking).
- **Run** — new `Msg::RunScript { proj, wt }` dispatched from a play-icon button
  in the worktree row's hover buttons. Hidden when `scripts.run` is unset.
- **Teardown** — replaces the inline removal in the `RemoveWorktree` path with
  the teardown modal.

## Teardown modal + state machine

A new `Modal::Teardown` variant holds the worktree path, project path, and a
state: `RunningScript → Removing → Done { error }`.

Flow on delete confirm:
1. `kill_sessions_for_wt(path)` (existing).
2. If `scripts.teardown` is set: spawn a teardown session in the worktree path
   and render its PTY (read-only) inside the modal. Wait for the shell to exit.
   If unset, skip straight to step 3.
3. Run `git::remove_worktree(project_path, path) --force`.
4. Transition to `Done`; show "worktree deleted" (or the error). User dismisses.

The teardown session is held in a dedicated field on the `Grove` GUI model (not
inside `App`, so the existing modal cloning stays cheap) and advanced by the
existing `Msg::Tick`: when its status flips to `Exited`, the tick handler runs
the git removal and transitions the modal.

## Scripts editor UI

- A **cog/edit hover button** added to `project_row` (next to add/remove) →
  `Msg::EditScripts { proj }`.
- The editor uses Iced's `text_editor` widget, which requires persistent
  `text_editor::Content`. A `scripts_editor: Option<ScriptsEditorState>` field
  on the `Grove` model holds the target project index and three `Content`
  buffers, plus which field is focused. Rendered as an overlay (same mechanism
  as other modals) with setup / run / teardown editors stacked, Tab to cycle
  fields, save/cancel buttons.
- `Msg::EditScripts { proj }` populates the state from the stored strings.
  `Msg::ScriptsEditorAction(field, Action)` edits a buffer.
  `Msg::ScriptsEditorSave` writes trimmed strings back into the `Project` and
  `storage::save`s. `Msg::ScriptsEditorCancel` drops the state.

Icons reuse the existing `play`, `cog`/`edit` sprite entries — no new SVGs.

## Out of scope

- Per-worktree script overrides (scripts are project-level only).
- tmux persistence for script sessions.
- Environment-variable injection beyond what normal sessions already set.
