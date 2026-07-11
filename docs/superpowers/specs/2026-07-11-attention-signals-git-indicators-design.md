# Design: Attention signals + git state indicators

**Date:** 2026-07-11
**Status:** Approved

## Problem

Grove's core job is supervising several parallel agent sessions. Today the
only status Grove shows is "running" (a green dot). To find out whether an
agent is waiting on permission or input, has finished, or is still working,
the user has to manually check each session. Worktree git state (dirty,
ahead, behind) is likewise invisible from the sidebar — the user has to open
a shell to check it.

This design adds two independent, additive signals to the existing session
list: agent attention state, and per-worktree git state.

## Feature 1: Attention signals

### Detection

Detection requires zero user setup, because Grove owns the spawn command
line for every agent session.

**Claude.** Grove spawns Claude with `--settings <path-to-generated.json>`,
where the generated file declares `Notification`, `Stop`, and
`UserPromptSubmit` hooks. `--settings` merges with the user's own Claude
settings, so nothing the user has configured is modified or overridden.
Each hook is a one-line command that appends a state word to a per-session
state file. Grove creates that file per session and passes its path to the
hook via an environment variable set on the session's PTY. The hooks map to
states as follows:

| Claude hook          | State word appended |
|-----------------------|----------------------|
| `Notification`        | `needs-you`          |
| `Stop`                 | `done`                |
| `UserPromptSubmit`     | `working`             |

**Codex.** Grove spawns Codex with `-c notify=[...]` pointing at the same
state-file writer used for Claude, invoked on the agent-turn-complete event.
Codex exposes no approval-prompt event, so Codex sessions only ever reach
`working` or `done` — `needs-you` is not observable for Codex in v1.

**OpenCode / Terminal.** Unchanged in v1: these session types continue to
report only `running` / `idle`, the same as today. An OpenCode plugin that
adds richer detection is a possible future extension, out of scope here.

**Polling.** Grove reads the per-session state files on its existing UI
tick. No filesystem-watch (inotify or equivalent) machinery is introduced;
the files are tiny and the existing tick cadence is sufficient.

### Session states & display

There are four session states:

- **working** — the agent is actively processing.
- **needs-you** — the agent is waiting on user input or permission.
- **done** — the agent finished its turn and is idle, without input having
  been consumed since.
- **idle** — no signal is available (OpenCode/Terminal sessions, or a
  missing/stale state file for a Claude/Codex session).

This extends the existing dot-based status system rather than replacing it,
per Grove's "show state, not chrome" principle. Each state gets a distinct
glyph, not just a distinct color, so the signal remains legible without
relying on color perception:

- `●` working
- `◆` needs-you (rendered in the accent color *and* a distinct glyph — color
  alone never carries the signal)
- `○` done / idle

These glyphs are shown consistently everywhere a session's status currently
appears: sidebar tree rows, the activity view, and agent-grid tiles.

**Clearing.** Focusing or viewing a session clears `needs-you` or `done`
back to the baseline `working`/`idle` state for that session. This mirrors
how the user's attention resolves the signal: once they've looked, there's
nothing left to flag.

**No OS notifications, no toasts.** Per Grove's product anti-references,
this feature is limited to in-app, in-place status glyphs. It does not add
system notifications, sounds, or toast popups.

## Feature 2: Git state indicators

Each worktree row in the tree view gains a compact suffix reflecting its git
state:

- `*` — the worktree has uncommitted changes (dirty).
- `↑N ↓M` — the worktree's branch is `N` commits ahead and/or `M` commits
  behind its upstream. Only the non-zero side is shown if one of the two is
  zero (e.g. `↑3` alone, or `↓2` alone); nothing is shown if both are zero
  and the worktree is otherwise clean.
- Nothing — the worktree is clean and, if it has an upstream, even with it.

These two suffixes can combine (e.g. `* ↑1`) when a worktree is both dirty
and diverged.

### Refresh

State is refreshed by a throttled background poll, roughly every 5 seconds,
covering only worktrees currently visible in the tree view. The poll runs
`git status --porcelain=v2 -b` per worktree and reuses the existing git
plumbing in `src/git.rs` rather than introducing a parallel git-invocation
path.

## Scope of v1

- Claude sessions get full three-state attention detection
  (working / needs-you / done).
- Codex sessions get two-state attention detection (working / done).
- OpenCode and Terminal sessions are unchanged (running / idle only).
- Git state indicators (dirty / ahead / behind) are shown for all worktrees
  in the tree view.
- No configuration surface: both features work out of the box, with no
  settings to enable, disable, or tune.

## Explicitly out of scope

- OS-level notifications or sounds for attention signals.
- Per-agent setup UI (the hook/notify wiring is entirely Grove-managed).
- An OpenCode plugin for richer OpenCode attention detection.
- Configurable poll intervals for either the state-file polling or the git
  status polling.

## Error handling

- **Hook/notify writer failures are silent by design.** The state-file
  writer must never cause the agent CLI (Claude or Codex) to fail or emit
  visible errors — a failed write is simply lost. Grove treats a missing or
  stale state file as the same as no signal: the session shows as plain
  `running`/`idle`.
- **Git poll failures degrade to showing nothing.** If `git status` fails
  for a worktree (for example, no upstream configured, or the worktree is
  in a bad state), that worktree's row shows no suffix rather than an error
  or a stale value.

## Testing

Unit tests cover:

- State-file parsing: raw state-file contents → resolved session state,
  including the missing/stale-file fallback to `running`/`idle`.
- Generated Claude `--settings` JSON: shape of the `Notification`, `Stop`,
  and `UserPromptSubmit` hook entries.
- Git status parsing: dirty, ahead, behind, and clean cases parsed from
  `git status --porcelain=v2 -b` output.

End-to-end hook firing (Claude and Codex actually invoking the writer and
Grove picking up the resulting state change) is verified manually, since it
depends on the external agent CLIs' runtime behavior.
