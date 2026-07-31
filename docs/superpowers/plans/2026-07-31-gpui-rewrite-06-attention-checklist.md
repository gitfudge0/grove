# Plan 06 exit gate — manual parity checklist (human sign-off required)

Rows 1–12 are spec Appendix A → *Attention/activity*, verbatim and in order.
13–17 are the appbar/statusbar clauses of *Screens/layout* plus the two
*System* clauses this phase owns.

**Nothing here may be signed off by an agent.** Run both backends, side by side
with the installed iced build, and fill in W (Wayland) / X (X11).

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui              # Wayland
WAYLAND_DISPLAY= PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui   # X11
~/.local/bin/grove                                                                   # iced, side by side
```

| # | Row | W | X | Notes |
|---|---|---|---|---|
| 1 | **480ms cadence.** Background agent state changes land within ~half a second, focused or not (own timer, not the frame clock). | ☐ | ☐ | |
| 2 | **Precedence: native poller > hook file > screen-scrape.** `~/.config/grove/attention/{pid}-{id}.state` exists and is 0600; `{pid}-{id}.claude-settings.json` is valid JSON declaring Notification/Stop/UserPromptSubmit; Codex gets `-c notify=[…]` and no settings file; OpenCode/plain terminal get neither. Killing `claude agents --json` support demotes to the hook file; no state file at all still classifies from the screen. | ☐ | ☐ | |
| 3 | **Focused never `WaitingForInput`.** A permission prompt on the *visible* session shows Working/Done, never amber; leaving and returning re-checks. | ☐ | ☐ | |
| 4 | **Bell diff with backwards-reset resync.** BEL on an unfocused session flags waiting once; a decorative BEL during active output does not; a parser reset does not go permanently bell-deaf. | ☐ | ☐ | |
| 5 | **Scroll/input quiet windows.** Scrolling or typing in a Done session does not flip it to Working (3s / 2s), but a genuinely working agent still shows Working. | ☐ | ☐ | |
| 6 | **OSC-title working marker, 60s staleness.** Braille title asserts Working on a quiet PTY; a frozen title stops asserting after 60s; `✳` alone asserts nothing. | ☐ | ☐ | |
| 7 | **Done only for non-Terminal agents.** A home terminal never shows the green check; a finished agent turn does. | ☐ | ☐ | |
| 8 | **Acknowledge on focus/refocus truncates the state file.** Selecting a waiting session clears its glyph *and* empties its `.state` (`wc -c`); re-focusing the window acknowledges the visible session the same way; the file still **exists** (truncated, not deleted) and later hooks still land. | ☐ | ☐ | |
| 9 | **Dock badge + one bounce per enter-while-unfocused.** **Linux: verify the no-op** — nothing renders, nothing bounces, and the waiting count still drives the appbar pill. **macOS: MANUAL-on-macOS, deferred to Plan 10** (code ships here). | ☐ | ☐ | macOS deferred |
| 10 | **Amber pulse, 1s auto-reverse.** Sidebar waiting glyph and appbar pill dot dim/brighten in lockstep on a ~2s round trip, never disappearing (no layout shift), and stop completely when the last waiting session is acknowledged. | ☐ | ☐ | |
| 11 | **12-frame spinner every 3 ticks.** A working session's sidebar spinner turns at the iced build's rate, side by side. | ☐ | ☐ | |
| 12 | **3-dot `(tick/5)%3`.** A session whose OSC title says "in progress" shows the three-dot walk plus the green "in progress" label in the session header, at iced's rate. | ☐ | ☐ | |
| 13 | **Appbar.** Brand over the rail at the current sidebar width; the lone grid toggle in non-grid view; the cog; the pill appears only while something waits, reads "1 needs you" / "n need you", and toggles the dropdown. | ☐ | ☐ | |
| 14 | **Attention dropdown.** Anchored under the appbar's right edge, 280px, one row per waiting session in **tree order**, each with glyph, agent label, `project / worktree` subtitle and 3px amber accent; clicking a row jumps **and snaps to the bottom**; backdrop dismisses; footer shows the real `mod+'` binding; `mod+'` itself cycles. | ☐ | ☐ | |
| 15 | **Statusbar.** Running count and dot, `BACKEND tmux\|native`, `THEME <name>`, the `bypass` chip when enabled, the version, and the palette/shortcuts chips showing the registry's real keys. | ☐ | ☐ | |
| 16 | **Toast with kind-dependent TTL.** Info clears after 4s, error after 8s, a newer toast replaces an older one immediately and gets its own full TTL. | ☐ | ☐ | No trigger ships this phase — see below. |
| 17 | **System: stale-file GC + idle power.** Killing Grove mid-session leaves `.state` files; the next start deletes exactly the dead-pid ones and leaves a concurrently-running Grove's alone. Nothing waiting + unfocused + no PTY output ⇒ 1s cadence (check `top`); with an agent working, ~480ms and smooth. | ☐ | ☐ | |

## Deferred, not failed

- Grid tile waiting-scrim + 40-tick pulse, tile "respond" chip, zen floating
  attention pill, per-tile session headers → **Plan 07**.
- Every modal behind the cog, the `+`, and the two statusbar chips; text inputs
  → **Plan 08**. All of them dispatch to logged stubs today.
- The upgrade dot's real state (stubbed `false`), telemetry, quit paths, tmux
  sidecar reattach discovery → **Plan 09**.
- macOS dock badge/bounce (row 9) → **Plan 10 on a macOS host**.
- Screenshot sweep and measured idle-power comparison → **Plan 10**.

## Known gap affecting row 16

`ToastState` ships with its TTL task, supersession guard and statusbar slot, and
is unit-tested, but **nothing calls `set_toast`/`set_error` yet** — every
producer in the iced build is a modal or a clipboard/script action owned by
Plan 07/08. Row 16 is therefore only verifiable by temporarily calling
`set_toast` from a stub, or deferred to the first real producer.

## Known gap affecting row 9's Linux "waiting count still drives the pill"

Verifiable today. The badge/bounce calls themselves are compiled-in no-ops off
macOS by construction (`platform/dock.rs`), so the Linux rows are an
*absence* check, not a behavior check.
