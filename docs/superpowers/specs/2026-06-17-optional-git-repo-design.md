# Optional Git Repository for Projects

**Date:** 2026-06-17
**Status:** Design — awaiting review

## Problem

Grove currently forces every added project to be a git repository before any
session or terminal can be created. The block is structural: sessions are
children of worktrees, worktrees can only be created in a git repo (hard guard
at `src/app.rs:1290`), so a non-git project has nowhere to host a session.

Users who just want to run an agent/terminal in a plain directory are forced to
run `git init`. We want git to be **optional**: if the user declines git, they
should still be able to create sessions and terminals.

## Goal

Allow sessions and terminals to be created in a project that is not a git
repository, with no worktrees, running directly in the project path.

Non-goals:
- No worktree creation for non-git projects (worktrees require git; confirmed).
- No branch isolation for non-git projects.
- No silent auto-`git init`.

## Current Behavior (verified)

| Layer | Check | Location |
|-------|-------|----------|
| Add project | `.git` exists? | `src/app.rs:1258` |
| Create worktree | `.git` exists? (hard block) | `src/app.rs:1290` |
| List worktrees | `git worktree list` | `src/git.rs:14-20` (returns `[]` gracefully) |
| Session spawn | `current_branch()` | `src/session.rs:300` → `src/git.rs:86-101` (returns `""` gracefully) |

Key insight: the **session spawn / PTY path already tolerates non-git paths**
(empty branch string). The only thing missing is a worktree-shaped host to
attach the session to in the UI/tree model.

Project model (`src/storage.rs:22-28`) has no `is_git` field — git status is
detected at runtime from `.git` existence.

## Design (Approach A: synthetic root worktree)

For a non-git project, represent the project path itself as a single implicit
"root" worktree:

- name: project name (or a fixed label such as `root`)
- path: the project path
- branch: empty / none
- not creatable, not deletable, no "+ add worktree" affordance

This reuses the entire existing session/PTY plumbing. Sessions attach to this
implicit worktree exactly as they would to a real one; `current_branch("")`
already degrades gracefully so the branch chip simply renders empty.

### Detecting git status

Add a runtime helper (cheap `.git` existence check, the same one already used in
`app.rs`) surfaced where the tree is built. Optionally cache on the in-memory
project struct to avoid repeated stat calls; persistence schema unchanged.

### Worktree listing

`git::list_worktrees()` already returns `[]` for non-git dirs. The tree builder
should, for a non-git project, synthesize exactly one implicit root worktree
entry instead of the (empty) git worktree list. For git projects: unchanged.

## Flow / UI Changes

### 1. Add-project modal (three-way)

When the chosen folder is not a git repo, replace the current forced
"initialize git repo?" confirm (`src/app.rs:1263-1269`) with a three-action
confirm:

- **Initialize git** (primary / recommended) → existing `ConfirmKind::InitRepo`
  behavior (`git init`, generate `.worktreeinclude`, etc.).
- **Continue without git** → add the project as-is (it is already added to
  storage today); no git init. Project shows with an implicit root worktree.
- **Cancel** → abort add.

Prompt copy: `'{path}' is not a git repository. Initialize git for branch
isolation, or continue without it.`

This needs a confirm modal that supports three actions; today `Modal::Confirm`
is two-action (cancel/confirm). Either extend `Modal::Confirm` to allow an
optional third action, or introduce a small dedicated modal variant. Prefer
extending with an optional middle action to keep one code path.

### 2. Sidebar tree

- **Git project (unchanged):** project row → N worktree rows (branch chips) →
  session rows; "+ add worktree" present.
- **Non-git project:** project row → single implicit root worktree row (no
  branch chip, subtle "direct path / no git" hint) → session rows. No "+ add
  worktree". Hover reveals the start/spawn button at the right edge, same as a
  normal worktree row. "+ new terminal" works against the project path.

### 3. Spawn

`spawn(proj, wt, agent)` for the implicit root worktree passes the project path
as the worktree path. No `git::add_worktree`, no lifecycle worktree-setup script
keyed on a new worktree (setup/run/teardown that assume a fresh worktree dir
should be skipped or no-op for the root worktree). Session spawns with empty
branch via the existing path.

### 4. Optional upgrade (future, not in scope)

A small "init git" action on a non-git project row to upgrade in place. Noted
for later; not part of this change.

## Components Touched

- `src/app.rs` — add-project flow (three-way modal), `ConfirmKind` variants,
  worktree-creation guard messaging.
- `src/gui/view.rs` / `src/gui/rows.rs` / `src/gui/widgets.rs` — tree rendering
  for non-git projects (synthetic root row, hide "+ add worktree", spawn button).
- Tree/worktree assembly (wherever `list_worktrees` results are turned into rows)
  — synthesize the implicit root worktree for non-git projects.
- `src/gui/update.rs` — message handling for "continue without git".
- Possibly `src/storage.rs` — optional cached `is_git` (not required).

## Error Handling

- Non-git project where `.git` later appears (user runs `git init` externally):
  on next tree refresh it becomes a normal git project with real worktrees. The
  implicit root worktree disappears; existing sessions in the project path keep
  running (they are PTYs bound to a path, not a worktree handle).
- Spawn failure surfaces via the existing `Modal::Message("failed to start
  agent: …")` path (`src/app.rs:495-523`).

## Testing

- Unit: tree assembly yields exactly one implicit root worktree for a non-git
  dir and the real list for a git dir.
- Unit: three-way confirm dispatches the correct action for each button.
- Manual: add a non-git folder → Continue without git → spawn an agent and a
  terminal in it; confirm they run in the project path with no worktree created.
- Manual: add a non-git folder → Initialize git → behaves exactly as today.
- Regression: existing git projects show worktrees and branch chips unchanged.

## Mockup

See `mock.html` at repo root: the three-way add-project confirm and the sidebar
tree comparison (git vs non-git project).
