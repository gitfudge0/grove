# Session Status Indicators — Design

**Date:** 2026-06-11
**Status:** Approved

## Goal

Make every session item answer at a glance: is something going on, does it need my
input, or is it done? Today the sidebar shows only a static dot (running / idle-45s /
exited) with no animation and no "needs input" state.

## Scope

- Session rows in both tree view and activity view.
- Roll-up of the most urgent descendant state onto collapsed worktree/project rows.
- macOS dock signal: badge count of waiting sessions + one attention bounce.
- Best-effort per-agent detection for Claude Code, Codex, and OpenCode; plain
  terminals and unknown agents use generic recency-based detection.

## State model

A derived `ActivityState`, computed per GUI tick (~500ms), per session:

| State | Meaning | Conditions |
|---|---|---|
| `Working` | Agent actively producing | Process alive, and output < ~2s old **or** screen shows the agent's active-work marker (Claude: "esc to interrupt"; Codex/OpenCode: their spinner/working lines) |
| `WaitingForInput` | Agent needs the user | Process alive, output quiet, and (BEL rang since last viewed **or** screen bottom matches the agent's pending-question/permission pattern, e.g. Claude's "Do you want to…" box or numbered option menus) |
| `Done` | Agent finished its turn | Process alive, output quiet, at-rest prompt with no pending dialog, and the session was previously `Working` this stretch |
| `Idle` | At rest, nothing happened | Process alive, quiet past the existing 45s threshold, no recent `Working` history (terminals, untouched agents) |
| `Exited` | Process ended | Unchanged from today |

`WaitingForInput` and `Done` are **acknowledged** when the user focuses the session:
bell flag clears and styling downgrades to plain idle. The currently focused session
never shows `WaitingForInput` and never triggers the dock.

## Detection architecture (hybrid)

Two layers, extending the existing `dirty` / `last_output_at` pattern in `src/session.rs`:

1. **Reader thread (stream signals):** a new `bell: Arc<AtomicBool>` is set when BEL
   (0x07) appears in PTY output, alongside the existing `dirty` and `last_output_at`
   updates. Process exit is already captured.
2. **GUI tick (screen classification):** every ~500ms, read the atomics plus the last
   ~15 rows of the already-parsed vt100 grid and classify via a per-agent pattern
   table — a `match agent` over simple `&str` contains-checks, no regex dependency.
   Patterns live in one module (`src/gui/activity.rs`) so agent-UI drift is a
   one-file fix.

Fallbacks: if no agent pattern matches, generic recency rules apply (recent output =
`Working`, quiet = `Idle`), so detection degrades gracefully. A bell alone only
implies `WaitingForInput` when output has also gone quiet (decorative BELs ignored).
Tmux and native backends behave identically — both feed Grove's own vt100 grid.

Misclassification is cosmetic by design: states drive only visuals and the dock
badge, never behavior.

## Visuals

Status glyphs replace the dot, rendered as text in the existing monospace font with
existing theme colors plus one new amber:

| State | Glyph | Treatment |
|---|---|---|
| Working | rotating spinner (`◜ ◠ ◝ ◞ ◡ ◟` or braille frames) | green, frame-advanced by tick |
| WaitingForInput | `?` | amber, blinking ~1Hz, label full-brightness |
| Done | `✓` | green, static, label slightly dimmed |
| Idle | `·` | dim (today's faint treatment) |
| Exited | `○` | hollow, muted (unchanged) |

- **Activity view grouping** gains "waiting" as the top group:
  waiting → running → done/idle → exited.
- **Roll-ups:** collapsed worktree/project rows show the most urgent descendant
  state as a small trailing glyph; urgency = waiting > working > done; idle/exited
  show nothing. Expanded parents show nothing extra.
- **Animation plumbing:** an `iced::time::every(250ms)` subscription, active only
  while ≥1 session is `Working` or `WaitingForInput`, drives spinner frames, blink
  phase, and re-classification. It drops when nothing animates so Grove stays
  idle-cheap.

## Dock (macOS only)

- Badge = count of `WaitingForInput` sessions; updates on state change, clears on
  acknowledgment.
- One attention bounce (`requestUserAttention:` with `NSInformationalRequest`, via
  objc bindings) when a session *enters* `WaitingForInput` while Grove is unfocused.
- All behind `#[cfg(target_os = "macos")]`; no-op elsewhere.

## Testing

- **Classifier unit tests:** synthetic screen-bottom text + signal combinations
  (bell, output age, working-history) per agent → asserted `ActivityState`.
  Real captured screen snippets from each agent become fixture strings, so UI drift
  shows up as a pinpointed failing fixture.
- **Ordering/roll-up unit tests:** plain tests over lists of states for activity-view
  grouping and parent urgency.
- **Dock glue:** thin, manually verified.
