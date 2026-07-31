# gpui Rewrite Plan 09 — System checklist (MANUAL, human, real desktop)

Spec Appendix A's **System** clause, verbatim and in order (Plan 09 Task 7
Step 2). Run the two builds side by side and mark each row PASS / FAIL /
DEFERRED. **Nobody may claim these from a test suite** — they are the exit gate
for row 09 and they are human rows.

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui
# and, side by side, the installed iced build:
~/.local/bin/grove
```

| # | Row | Result |
|---|-----|--------|
| 1 | **31 + custom themes, 11 tokens, follow-system with the startup seed order** — every builtin and every `themes.json` custom theme resolves; the 11 tokens and their contrast partners render identically to iced; flipping the OS appearance flips the app; a fresh launch in follow-system mode paints the **right** mode on the first frame, not a flash of the wrong one. *(Shipped in Plan 03.)* | |
| 2 | **Zoom 0.6–2.0 whole-app, driving logical PTY dims** — every step re-lays the chrome and the terminal together, and the PTY's reported `(rows, cols)` matches iced at the same window size and zoom. *(Plan 03/07.)* | |
| 3 | **Sidebar width / zoom / grid order / settings persisted** — set all four, quit **normally**, relaunch: all four come back. Then set the zoom and quit **within 250 ms** (the debounce window) — it must still come back. Then rearrange the grid and quit from grid view — the order comes back. | |
| 4 | **tmux sidecar discovery/reattach** — with live agent sessions, quit grove and relaunch: every tmux-backed session reappears in the tree with its project/worktree/label/agent intact and its scrollback live; native sessions do **not** reappear; a session killed outside grove leaves no ghost row and no orphan sidecar; the reattached terminal is sized correctly on the first frame, not after a resize. Then turn tmux **off** and back **on** in Settings and confirm the re-scan neither duplicates nor drops a session. | |
| 5 | **Attention stale-file GC at startup** — leftover state files from a previous run are cleared before any session spawns, and a reused session id cannot read a stale file. *(Plan 03/06.)* | |
| 6 | **Login-PATH recovery** — launched from a desktop launcher (not a shell), agents still resolve on `$PATH`. *(Plan 03.)* | |
| 7 | **Panic hook + telemetry** — with no key compiled in, **nothing** is transmitted (say whether this was confirmed with a network monitor or by reading the three gates); the panic hook logs the message locally and would transmit only a scrubbed location; the Settings **Telemetry** row defaults to on, persists, and flipping it takes effect immediately. | |
| 8 | **arboard + OSC52 clipboard** — copy a terminal selection and paste into another app; paste in; on Wayland the `wl-paste` file-URI fallback still works. *(Plan 04.)* | |
| 9 | **Adaptive 60 ms/1 s tick and its gating** — an unfocused, quiet window drops to the slow cadence while a background agent still streams and classifies at full rate. *(Plan 03/06; the numeric idle-power measurement is Plan 10 — eyeball only here.)* | |
| 10 | **3 s-delayed + 24 h/refocus upgrade checks, changelog, apply/restart** — the launch check fires ~3 s after startup and not before; a newer release lights the **green dot on the cog** and offers Update / Skip / Copy URL; Skip suppresses that tag and a *newer* tag surfaces again; a manual check from Settings surfaces its error inline while a silent one stays quiet; the changelog fetches the 10 most recent releases, renders them, and **returns to Settings** on dismiss; starting an update shows live Downloading → Building → Installing stages with the window still responsive, **Escape is refused while it runs**, and Restart relaunches into the new version with the sidebar width and zoom intact. | |

## Added this phase (authorized scope addition, not an Appendix A System row)

| # | Row | Result |
|---|-----|--------|
| A | **Settings → Tools** — the section lists Claude, Codex and OpenCode (never the plain Terminal), each row showing "Detecting…" then either its `--version` or "Not installed"; a missing tool's label and dot recede (hollow dot, dimmed label); "Set default" appears only on installed non-default tools and adopting one persists `default_agent`; "Re-detect tools" re-runs the scan without freezing the window. This closes the Plan 08 **Modals** row 12 gap. | |

## Deferred — record as deferred, not failed

- The scripted screenshot sweep (every screen/modal × 3 zooms × 4 themes) and the numeric idle-power measurement → **Plan 10**.
- The macOS dock badge/bounce and the mac-only ⌘ chords → **Plan 10, on a macOS host**.
- IME composition and the full Wayland clipboard round-trip (findings §S4 Deviation 4) → **Plan 10's manual sweep**.
- Deleting the iced app and vt100 → **Plan 10**.
