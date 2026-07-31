# gpui rewrite plan 08 — MANUAL checklist (Appendix A *Modals*)

Run the two builds side by side and report each row **pass / fail / deferred**.
Do not mark a row from reading code; every row here needs a human at a real
desktop.

```bash
cd /home/gitfudge/dev/gitfudge0/grove
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui
# and, side by side, the installed iced build:
~/.local/bin/grove
```

Rows 1–19 are spec Appendix A's *Modals* clause, verbatim and in order; 20–23
are the cross-cutting contracts that paragraph's preamble names.

| # | Row | Result | Notes |
|---|---|---|---|
| 1 | **Input** — the worktree-name prompt: focused on open, Enter submits, Escape and Ctrl+C cancel, an invalid name shows the inline red note and the note clears on the next keystroke. | | |
| 2 | **Confirm (incl. Quit)** — Escape=no / Enter=yes / `y` / `n`; destructive styling on the destructive kinds; closing the window with running **native** sessions confirms first, tmux-backed ones quit straight away. | | |
| 3 | **AddProject wizard w/ dir autocomplete** — typing a partial path lists matching directories, ↑↓ walks them, Enter/Tab picks; the browse button opens the OS picker once (no double-open); step two probes for a git repo and offers init; Escape from step one cancels, Ctrl+C cancels from either. | | |
| 4 | **RemoveProject w/ async teardown progress** — the checkbox controls whether worktrees are deleted on disk; the progress view counts up with the current path, collects per-worktree errors, and refuses to cancel while busy; the window stays responsive throughout. | | |
| 5 | **ArchiveProject + ArchivedProjects** — the gate lists one row per **session** (not per worktree) with honest running/exited labels, refuses while any live session remains, and re-counts after each kill; the archived list restores and deletes, and a restored project reappears in the tree. | | Entry points: ScriptsEditor → "Archive project"; Settings → "Archived projects". |
| 6 | **Message** — Escape, Enter and `q` all dismiss. | | |
| 7 | **TmuxChoice** — Enter/`t`/`y` chooses tmux, `n` chooses native, **Escape persists nothing** and the choice is re-asked on the next launch. | | |
| 8 | **AgentPicker** — ↑↓/`j`/`k` move, Space toggles "default agent", Enter spawns; a failing spawn raises the error toast. | | |
| 9 | **SessionLauncher** — recents first; typing filters every project×worktree combo; Tab opens the row-action strip and ←/→ cycle the agent **inside** it while the caret never moves; the switch drill-in lists sessions then home terminals; the settings drill-in reaches all four panes; **live project-theme preview repaints the terminal behind the palette** and cancelling restores the previous theme. | | |
| 10 | **ThemePicker** — dark/light tabs (Tab, `h`/`l`), follow-system, app-vs-project scope, "Default (follow app)" for project scope; Escape restores the original theme; entered from Settings it returns to Settings. | | |
| 11 | **ThemeManager + multiline editor** — rename (with the collision error), duplicate, delete (y/n), new; the paste-first editor accepts a multi-line paste, saves, and re-colors the terminal immediately. | | |
| 12 | **Settings (immediate persist)** — every control writes through with no apply button; the **archived-projects** row opens the archived list; the **tmux** setting flips the backend for the next spawn; reopening Settings shows the persisted values after a restart. | | |
| 13 | **ShortcutOverlay (registry-generated)** — every chord shown matches what the key actually does, the visible set changes with the current screen, alt-chords render as `{mod}+alt+…`, macOS shows ⌘, and the two static rows (copy/paste, "Close modals") are present. Escape **and** the overlay's own chord close it. | | The ⌘-SVG substitution is macOS-only → Plan 10. |
| 14 | **Teardown (embedded live PTY)** — the teardown script runs visibly inside the modal, Escape skips it and proceeds to removal, removal cannot be interrupted, and the final state reports success or failure. | | Needs a project with a non-empty teardown script. |
| 15 | **ScriptsEditor (3 multiline editors)** — all three buffers edit independently and scroll independently; **Tab indents** and **ctrl-tab / clicking** moves between them; Save persists (an emptied buffer clears that script) and Cancel discards; "Project theme" opens the picker and coming back **preserves unsaved buffers**. | | |
| 16 | **Updating** — the shell renders the current upgrade state and Escape closes it only when no update is in flight (live stages are Plan 09). | | |
| 17 | **Onboarding wizard** — full-viewport with no sidebar/statusbar/scrim, entrance animation, Tab alternates path/name on the project step, the session step's agent + permissions choices persist, and finishing hands off to the tmux choice. | | Reach it with a fresh `GROVE_CONFIG_DIR`. |
| 18 | **One-deep, replace-don't-stack** — opening a second modal replaces the first; there is no back-stack anywhere. | | |
| 19 | **Quit-confirm clobbers (the preserved gap)** — requesting close while any modal is open replaces it with the quit confirm, and cancelling that confirm leaves **no** modal, not the one it clobbered. | | |
| 20 | **Per-modal Escape semantics** — spot-check the asymmetries: Teardown's Escape means "skip", RemoveProject's is refused while busy, Updating's is refused mid-update, TmuxChoice's persists nothing, everything else closes. | | |
| 21 | **Focus on open** — every modal with a field has the caret in it immediately; every modal without one still responds to its letter keys and Escape with no click first. | | |
| 22 | **Changelog overlays Settings** — opening the changelog from Settings returns to Settings on dismiss. | | |
| 23 | **Toasts from modals** — a failing session spawn from the launcher/agent picker shows `failed to start session: …` in the statusbar and it clears on the error TTL. | | |

## Explicitly deferred (record as deferred, not failed)

- The upgrade flow's live stages, changelog fetch and apply/restart → **Plan 09**.
- Telemetry, quit-path persistence (`flush_ui_zoom_save`) and tmux sidecar
  reattach → **Plan 09**.
- The macOS dock badge/bounce and the mac-only ⌘ chord behaviors → **Plan 10 on
  a macOS host**.
- The scripted screenshot sweep across every modal × 3 zooms × 4 themes and the
  idle-power measurement → **Plan 10**.
- IME composition inside modal fields and the Wayland clipboard round-trip
  (findings amendments 5/6) → **Plan 10's manual sweep**.

## Automated evidence already green (do not re-verify by hand)

- `cargo +1.95.0 test -p grove-gpui` — **404 passed, 0 failed**.
- `cargo +1.95.0 test -p grove-gpui keyboard_matrix` — **13 passed**, pinning
  all six named contracts plus both drift guards.
- `cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings` —
  clean.
- Default toolchain (rustc 1.94.1): `cargo build` and `cargo test` green;
  `git status` reports no changes under `src/`, `crates/grove-core/`,
  `crates/grove-terminal/`.
- `grep -c '^name = "gpui"' Cargo.lock` = **1**, at ZED_REV.
