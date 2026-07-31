# Plan 10 Task 5 — the consolidated sign-off

> ## **NOTHING IS DELETED UNTIL THE USER EXPLICITLY APPROVES.**
>
> This document **is** the approval. Phase C (Task 7 — deleting iced, vt100 and
> `grove-core/src/session.rs`, and relocating the gpui app into `src/`) does not
> start until the user has worked down these rows and said, in their own words,
> that the sign-off is complete and Phase C may proceed. Silence, "looks good",
> a green test suite and an agent's summary are **not** approval.
>
> ## **NO AGENT MAY FILL IN A RESULT CELL.**
>
> Every `PASS/FAIL/DEFER` cell below belongs to the user. An agent-filled cell
> invalidates the sign-off — the whole point of a manual row is that a human
> looked at it. If you are an agent reading this: leave every result cell empty.

## Header — count and structure

**This document contains 94 rows.** Section subtotals: **13 / 11 / 17 / 13 / 23 / 11 / 6**
(Plan 04 / Plan 05 / Plan 06 / Plan 07 / Plan 08 / Plan 09 / Plan 10) =
**94**. The gathered count agrees with the plan's 94.

The **17 Plan 06 rows each need two platform columns** (Wayland *and* X11), so
the number of *checks* is 111, not 94. The row count is 94.

`origin` is `<plan>/<row>` — e.g. `08/14` is Plan 08 row 14 — so a failure is
traceable back to its checklist in one hop. Row text is **verbatim** from the
origin checklist; it has not been summarized, renumbered or reworded.

## Run it

```bash
cd /home/gitfudge/dev/gitfudge0/grove
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui   # Wayland
WAYLAND_DISPLAY= PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui   # X11
~/.local/bin/grove                                                        # iced, side by side
```

Companion artifacts:

- `docs/superpowers/plans/2026-07-31-gpui-rewrite-10-idle-power.md` + `scripts/idle-power.sh` (row P5)
- `docs/superpowers/plans/2026-07-31-gpui-rewrite-10-screenshot-sweep.md` + `scripts/screenshot-sweep.sh` (row P6)

---

## Plan 04 — Terminal (13 rows)

Source: `2026-07-31-gpui-rewrite-04-terminal-element.md`, Task 6 Step 3.

| origin | # | row (verbatim) | PASS/FAIL/DEFER | notes |
|---|---|---|---|---|
| `04/1` | 1 | **Metrics/glyphs.** `CELL 7.5×17 @ 12.5pt BlexMono`. Long lines stay column-aligned to the right edge; box-drawing and Nerd/powerline glyphs occupy exactly one cell; bold renders as the bundled Bold face. Side-by-side vs iced Grove on the same `claude` session. | | |
| `04/2` | 2 | **CJK.** A wide char under-fills its two-cell slot (findings §S1). Judge whether the `force_width` attempt (Task 3 Step 5) improved it, made it worse, or was unavailable — and record the verdict, because it decides whether the helper survives. | | |
| `04/3` | 3 | **ANSI→token map.** Run a 256-color test script (`for i in $(seq 0 255)`) in both builds and compare: index 0 = bg_strip, 1|9 = red … 8 = fg_mute, the cube and grayscale ramps. Then flip themes (Plan 03's chords) and confirm terminal content re-colors on the next frame. | | |
| `04/4` | 4 | **Inverse.** Something using reverse video (`printf '\e[7mINVERSE\e[0m'`, a tmux status line, `less` search highlight) shows the theme-default fill, not transparent or double-swapped text. | | |
| `04/5` | 5 | **Cursor.** Block cursor, blinking on the AnimationClock phase (`%16<8`), matching iced's rhythm; hidden when the inner app hides it (`vim`, `htop`); parked correctly when scrolled back. | | |
| `04/6` | 6 | **Selection.** Drag-select across several rows: overlay is `rgba(.40,.50,.78,.35)`; scroll while selected and the highlight **stays on the same text**; copy (Ctrl+Shift+C / Cmd+C) yields trailing-whitespace-cleaned text; a keypress kills the selection and the drag; dragging to the top/bottom edge auto-scrolls and extends. | | |
| `04/7` | 7 | **Click-to-caret.** Click mid-line at a shell prompt: caret moves horizontally only; clicking a different row does nothing; clicking while scrolled back does nothing; the click that focuses the window does not move the caret; inside an app with mouse reporting the click is forwarded instead. | | |
| `04/8` | 8 | **Scroll feel.** Trackpad (pixel deltas) vs wheel (line deltas) both feel identical to iced Grove; a fast flick does not flood tmux; reversing direction mid-gesture responds immediately; scrolling up in tmux enters copy-mode and the next keystroke leaves it exactly once. | | |
| `04/9` | 9 | **Keyboard scrollback.** Shift+PageUp/PageDown, Shift+Home/End; the Home/End full-scrollback jump is capped and does not hang; typing snaps back to the bottom. | | |
| `04/10` | 10 | **Input bytes.** Ctrl+C interrupts; Alt chords reach the app as ESC-prefixed; Alt+Escape arrives as ESC ESC; arrows work in `vim` and at a readline prompt in both cursor modes. | | |
| `04/11` | 11 | **Paste and drop.** Bracketed paste of a multi-line block arrives as one paste with `\r` line endings; on Wayland, "Copy" a file in a file manager then paste → the shell-escaped path plus a trailing space; drag-drop a file (X11/macOS) → the same. | | |
| `04/12` | 12 | **Resize.** Resize the window and change zoom across `[0.6, 2.0]`: the grid re-dims, the inner app reflows to the new size, and nothing clips or overlaps. | | |
| `04/13` | 13 | **Idle cost.** Leave the window open and unfocused for 60s; CPU should sit near the spike's release figure (1.23%) and comfortably under iced Grove's ~3.6%. | | |

**That source's own deferrals** (do not re-litigate; recorded here so they are
not mistaken for misses): per-project pinned content themes and the
project-themes toggle → **closed by Plan 05** (rows `05/9`, `05/10`);
mouse-report forwarding inside grid tiles and Agent/Panel focus routing →
**closed by Plan 07** (rows `07/9`, `07/10`); the two-step confirm-kill and the
Escape-despite-capture carve-outs → **closed by Plan 08** (rows `08/2`,
`08/20`).

## Plan 05 — Sidebar (11 rows)

Source: `2026-07-31-gpui-rewrite-05-sidebar.md`, Task 7 Step 2.

| origin | # | row (verbatim) | PASS/FAIL/DEFER | notes |
|---|---|---|---|---|
| `05/1` | 1 | **Tree shape.** `sidebar project→worktree→session tree` — same order, indent ladder, chevrons, session-count badges, git glyph, `main` tag, branch chip, context text, and the same active-row highlight rules (a session never looks active while a home terminal is on screen). | | |
| `05/2` | 2 | **3-mode expand cycle.** The header button cycles Collapsed → SessionsOnly → All, its glyph always previews the **next** action, and `SessionsOnly` collapses exactly the projects/worktrees with no sessions. Manual per-row toggles are fully overridden by a cycle press. | | |
| `05/3` | 3 | **Per-row hover actions.** Hovering a worktree reveals the spawn/run/add/delete strip in the `main`-tag slot with no layout shift; the agent menu opens anchored to the correct row at any scroll position and any collapse state; session rows arm and disarm their two-step kill. | | |
| `05/4` | 4 | **Git suffix.** dirty/ahead/behind text appears within ~5s of a change, on off-thread polls only (no UI stutter), for visible worktrees only, and **disappears** rather than going stale when `git` fails or the repo goes away. | | |
| `05/5` | 5 | **Archived-projects row.** Archived projects are absent from the tree (TRUE indices preserved for the rest); archiving every project shows the "All projects archived / Restore one from Settings → Archived projects." copy, and having no projects at all shows the *other* copy. (See the recorded ambiguity in Global Constraints — the archived *list* is Plan 08.) | | |
| `05/6` | 6 | **Divider.** Drag-resizable within `220 .. min(win/2, win−400)`; the width does not jump on an off-edge grab; a **350ms** double-click resets to 320; the width survives a restart; the terminal re-dims to the new width; a plain click writes nothing. | | |
| `05/7` | 7 | **Pinned TERMINALS section.** Always ≥1 home terminal; expanded shows header + rows with the header dot **off**; collapsed docks the header at the bottom with the dot **on** iff a shell is running; closing the last terminal respawns one. | | |
| `05/8` | 8 | **Selection and keyboard nav.** `mod+1..9` selects the nth **visible** session (numbering follows collapse state); next/prev cycle in tree order and wrap; selecting a session moves the sidebar highlight and the workspace body together with no visible two-step; `mod+w`-style close arms the confirm. | | |
| `05/9` | 9 | **Per-project pinned content themes** (deferred from Plan 04). With Project themes **on** and a project pinned to a different theme, that project's session content re-colors while app chrome stays on the global theme; sessions of unpinned projects are unaffected. | | |
| `05/10` | 10 | **Project-themes toggle invalidates** (deferred from Plan 04). Flipping the toggle in the store re-colors pinned projects' content on the next frame — no restart, no stale frame. | | |
| `05/11` | 11 | **Scroll behavior.** A tree taller than the viewport scrolls smoothly, keeps its scroll position across a repaint/selection change, and the docked TERMINALS header stays pinned outside the scroll area. | | |

**That source's own deferrals:** live attention/activity glyph *content* and the
appbar attention pill/dropdown → **closed by Plan 06** (`06/10`–`06/14`);
grid/zen/worktree panel/terminal tab and Agent/Panel focus routing → **closed by
Plan 07**; every modal a hover action opens, and the keyboard matrix's
Escape/confirm-kill carve-outs → **closed by Plan 08**.

## Plan 06 — Attention / activity (17 rows × Wayland **and** X11)

Source: `2026-07-31-gpui-rewrite-06-attention-checklist.md`.

**This is the only section that must be run twice** — once on Wayland and once
on X11 (`WAYLAND_DISPLAY=` forces the X11 backend). A row is not signed off
until both columns are marked.

| origin | # | row (verbatim) | Wayland | X11 | notes |
|---|---|---|---|---|---|
| `06/1` | 1 | **480ms cadence.** Background agent state changes land within ~half a second, focused or not (own timer, not the frame clock). | | | |
| `06/2` | 2 | **Precedence: native poller > hook file > screen-scrape.** `~/.config/grove/attention/{pid}-{id}.state` exists and is 0600; `{pid}-{id}.claude-settings.json` is valid JSON declaring Notification/Stop/UserPromptSubmit; Codex gets `-c notify=[…]` and no settings file; OpenCode/plain terminal get neither. Killing `claude agents --json` support demotes to the hook file; no state file at all still classifies from the screen. | | | |
| `06/3` | 3 | **Focused never `WaitingForInput`.** A permission prompt on the *visible* session shows Working/Done, never amber; leaving and returning re-checks. | | | |
| `06/4` | 4 | **Bell diff with backwards-reset resync.** BEL on an unfocused session flags waiting once; a decorative BEL during active output does not; a parser reset does not go permanently bell-deaf. | | | |
| `06/5` | 5 | **Scroll/input quiet windows.** Scrolling or typing in a Done session does not flip it to Working (3s / 2s), but a genuinely working agent still shows Working. | | | |
| `06/6` | 6 | **OSC-title working marker, 60s staleness.** Braille title asserts Working on a quiet PTY; a frozen title stops asserting after 60s; `✳` alone asserts nothing. | | | |
| `06/7` | 7 | **Done only for non-Terminal agents.** A home terminal never shows the green check; a finished agent turn does. | | | |
| `06/8` | 8 | **Acknowledge on focus/refocus truncates the state file.** Selecting a waiting session clears its glyph *and* empties its `.state` (`wc -c`); re-focusing the window acknowledges the visible session the same way; the file still **exists** (truncated, not deleted) and later hooks still land. | | | |
| `06/9` | 9 | **Dock badge + one bounce per enter-while-unfocused.** **Linux: verify the no-op** — nothing renders, nothing bounces, and the waiting count still drives the appbar pill. **macOS: MANUAL-on-macOS, deferred to Plan 10** (code ships here). | | | |
| `06/10` | 10 | **Amber pulse, 1s auto-reverse.** Sidebar waiting glyph and appbar pill dot dim/brighten in lockstep on a ~2s round trip, never disappearing (no layout shift), and stop completely when the last waiting session is acknowledged. | | | |
| `06/11` | 11 | **12-frame spinner every 3 ticks.** A working session's sidebar spinner turns at the iced build's rate, side by side. | | | |
| `06/12` | 12 | **3-dot `(tick/5)%3`.** A session whose OSC title says "in progress" shows the three-dot walk plus the green "in progress" label in the session header, at iced's rate. | | | |
| `06/13` | 13 | **Appbar.** Brand over the rail at the current sidebar width; the lone grid toggle in non-grid view; the cog; the pill appears only while something waits, reads "1 needs you" / "n need you", and toggles the dropdown. | | | |
| `06/14` | 14 | **Attention dropdown.** Anchored under the appbar's right edge, 280px, one row per waiting session in **tree order**, each with glyph, agent label, `project / worktree` subtitle and 3px amber accent; clicking a row jumps **and snaps to the bottom**; backdrop dismisses; footer shows the real `mod+'` binding; `mod+'` itself cycles. | | | |
| `06/15` | 15 | **Statusbar.** Running count and dot, `BACKEND tmux\ | | | |
| `06/16` | 16 | **Toast with kind-dependent TTL.** Info clears after 4s, error after 8s, a newer toast replaces an older one immediately and gets its own full TTL. | | | |
| `06/17` | 17 | **System: stale-file GC + idle power.** Killing Grove mid-session leaves `.state` files; the next start deletes exactly the dead-pid ones and leaves a concurrently-running Grove's alone. Nothing waiting + unfocused + no PTY output ⇒ 1s cadence (check `top`); with an agent working, ~480ms and smooth. | | | |

**Deferrals that a later phase closed:** row `06/16`'s "no toast producer" gap
— **closed by Plan 07**, whose row `07/13` says in its own words "Plan 06 row
16, now producible" and gives the trigger (force a spawn failure). Run `06/16`
for real; it is no longer blocked.

Row `06/9`'s macOS half stays deferred and is carried below as row **P1**.

## Plan 07 — Grid / zen / terminal tab / panel (13 rows)

Source: `2026-07-31-gpui-rewrite-07-grid-zen.md`, Task 7 Step 2.

| origin | # | row (verbatim) | PASS/FAIL/DEFER | notes |
|---|---|---|---|---|
| `07/1` | 1 | **Grid ≤4×4, `cols = ceil(sqrt(n)).clamp(1,4)`.** n = 1,2,3,4,5,9,16 tile exactly as the iced build does, and a 17th session does not add a 17th tile. | | |
| `07/2` | 2 | **Short-column tiles fill height.** With 3 sessions, the lone right-hand tile spans the full workspace height — no empty cell beside it — and its PTY has roughly twice the rows of the stacked pair. | | |
| `07/3` | 3 | **Header drag-reorder with a 150ms slide.** Dragging a tile header onto another swaps them; the source dims while held, the target shows the cyan inset, and on release both tiles ease into place over ~150ms with their PTYs already correctly sized. | | |
| `07/4` | 4 | **Per-tile PTY resize on reorder.** After a swap between columns of different heights, both tiles' shells wrap at their new size immediately (run `tput lines; tput cols` in each). | | |
| `07/5` | 5 | **Order persisted by stable session key.** Reorder, quit, relaunch: the arrangement survives, and a session closed while the app was shut still leaves the rest in order. | | |
| `07/6` | 6 | **Zen chrome-hidden with grid/terminal restore bookkeeping.** Zen from the single-session workspace and back returns there; zen from a grid tile shows that one session and exiting restores the grid exactly as it was; `mod+g` while zenned cancels the restore; `mod+t` in zen swaps content **without** unhiding the chrome. The floating amber pill appears top-right in zen only while something waits, shows the count, and jumps to it on click. | | |
| `07/7` | 7 | **Terminal tab.** `mod+t` from the workspace, from the grid (which it leaves and later restores), and from zen; native PTY rooted at `~`; restart recovers an exited shell in place keeping its label; no kill button; `mod+shift+t` (or the registry's real `NewHomeTerminal` key) adds another. | | |
| `07/8` | 8 | **Worktree panel, 20–75% split.** The session bar's `term` toggle opens the right-docked panel for the active session's worktree; Ctrl+Shift+←/→ steps it 5% at a time and clamps at 20/75; dragging the divider tracks the cursor and double-clicking it resets to 40%; both PTYs rewrap at every settle. Multiple shells per worktree via `＋`, switchable and closable by tab, and the `collapse-right` button dismisses the panel from inside itself. | | |
| `07/9` | 9 | **Agent/Panel focus routing.** Opening the panel focuses the panel shell; clicking the agent PTY moves input back and clicking the panel returns it; switching sessions re-anchors the panel to the new worktree and refocuses it; a worktree whose panel has no shell routes typing to the agent rather than eating it; the focus-changing click does **not** move the caret. | | |
| `07/10` | 10 | **Grid-tile mouse routing (deferred from Plan 04).** Clicking any tile — header, body or scrim — focuses that tile, makes it the active session and clears its amber glyph; keystrokes, scroll, selection and copy all go to the focused tile only, and never to a neighbor; `mod+1..9` selects by the tile's own number hint. | | |
| `07/11` | 11 | **Tile waiting affordances (deferred from Plan 06).** A tile whose agent needs input shows the solid amber 1.5px border (winning over the focused cyan), the pulsing `respond · {mod}+{n}` chip in its header, and the full-tile "NEEDS ATTENTION" scrim pulsing on the same ~2.4s 40-tick wave as the iced build, side by side. Clicking the scrim responds to it. | | |
| `07/12` | 12 | **Tile headers (deferred from Plan 06).** Agent icon, agent label, project, branch (absent for branchless sessions — no orphan dot), the number hint chip with the registry's real modifier, and the zen/kill buttons with the two-step kill confirm. | | |
| `07/13` | 13 | **Toast with kind-dependent TTL (Plan 06 row 16, now producible).** Force a spawn failure (e.g. point a project at a directory that no longer exists, or make the shell unspawnable): the error toast appears in the statusbar and clears after 8s; a second failure replaces it immediately with a fresh full TTL. | | |

**That source's own deferrals:** every modal behind the `+`, the cog, the two
statusbar chips and the session bar's `run script` button, plus
`gpui-component` text inputs → **closed by Plan 08**; the upgrade dot's real
state, telemetry, quit paths and tmux sidecar reattach → **closed by Plan 09**;
the macOS dock badge/bounce → row **P1** below; the scripted screenshot sweep
and the measured idle-power comparison → rows **P6** and **P5** below.

**Note on `07/8` (worktree panel) vs the grid:** Plan 07 recorded that the panel
is **suppressed in grid view, with no exception**. A `grid + panel open`
combination is therefore **N/A**, not a miss — see the same note in the
screenshot-sweep index.

## Plan 08 — Modals (23 rows)

Source: `2026-07-31-gpui-rewrite-08-modals-checklist.md`.

| origin | # | row (verbatim) | PASS/FAIL/DEFER | notes |
|---|---|---|---|---|
| `08/1` | 1 | **Input** — the worktree-name prompt: focused on open, Enter submits, Escape and Ctrl+C cancel, an invalid name shows the inline red note and the note clears on the next keystroke. | | |
| `08/2` | 2 | **Confirm (incl. Quit)** — Escape=no / Enter=yes / `y` / `n`; destructive styling on the destructive kinds; closing the window with running **native** sessions confirms first, tmux-backed ones quit straight away. | | |
| `08/3` | 3 | **AddProject wizard w/ dir autocomplete** — typing a partial path lists matching directories, ↑↓ walks them, Enter/Tab picks; the browse button opens the OS picker once (no double-open); step two probes for a git repo and offers init; Escape from step one cancels, Ctrl+C cancels from either. | | |
| `08/4` | 4 | **RemoveProject w/ async teardown progress** — the checkbox controls whether worktrees are deleted on disk; the progress view counts up with the current path, collects per-worktree errors, and refuses to cancel while busy; the window stays responsive throughout. | | |
| `08/5` | 5 | **ArchiveProject + ArchivedProjects** — the gate lists one row per **session** (not per worktree) with honest running/exited labels, refuses while any live session remains, and re-counts after each kill; the archived list restores and deletes, and a restored project reappears in the tree. | | |
| `08/6` | 6 | **Message** — Escape, Enter and `q` all dismiss. | | |
| `08/7` | 7 | **TmuxChoice** — Enter/`t`/`y` chooses tmux, `n` chooses native, **Escape persists nothing** and the choice is re-asked on the next launch. | | |
| `08/8` | 8 | **AgentPicker** — ↑↓/`j`/`k` move, Space toggles "default agent", Enter spawns; a failing spawn raises the error toast. | | |
| `08/9` | 9 | **SessionLauncher** — recents first; typing filters every project×worktree combo; Tab opens the row-action strip and ←/→ cycle the agent **inside** it while the caret never moves; the switch drill-in lists sessions then home terminals; the settings drill-in reaches all four panes; **live project-theme preview repaints the terminal behind the palette** and cancelling restores the previous theme. | | |
| `08/10` | 10 | **ThemePicker** — dark/light tabs (Tab, `h`/`l`), follow-system, app-vs-project scope, "Default (follow app)" for project scope; Escape restores the original theme; entered from Settings it returns to Settings. | | |
| `08/11` | 11 | **ThemeManager + multiline editor** — rename (with the collision error), duplicate, delete (y/n), new; the paste-first editor accepts a multi-line paste, saves, and re-colors the terminal immediately. | | |
| `08/12` | 12 | **Settings (immediate persist)** — every control writes through with no apply button; the **archived-projects** row opens the archived list; the **tmux** setting flips the backend for the next spawn; reopening Settings shows the persisted values after a restart. | | |
| `08/13` | 13 | **ShortcutOverlay (registry-generated)** — every chord shown matches what the key actually does, the visible set changes with the current screen, alt-chords render as `{mod}+alt+…`, macOS shows ⌘, and the two static rows (copy/paste, "Close modals") are present. Escape **and** the overlay's own chord close it. | | |
| `08/14` | 14 | **Teardown (embedded live PTY)** — the teardown script runs visibly inside the modal, Escape skips it and proceeds to removal, removal cannot be interrupted, and the final state reports success or failure. | | |
| `08/15` | 15 | **ScriptsEditor (3 multiline editors)** — all three buffers edit independently and scroll independently; **Tab indents** and **ctrl-tab / clicking** moves between them; Save persists (an emptied buffer clears that script) and Cancel discards; "Project theme" opens the picker and coming back **preserves unsaved buffers**. | | |
| `08/16` | 16 | **Updating** — the shell renders the current upgrade state and Escape closes it only when no update is in flight (live stages are Plan 09). | | |
| `08/17` | 17 | **Onboarding wizard** — full-viewport with no sidebar/statusbar/scrim, entrance animation, Tab alternates path/name on the project step, the session step's agent + permissions choices persist, and finishing hands off to the tmux choice. | | |
| `08/18` | 18 | **One-deep, replace-don't-stack** — opening a second modal replaces the first; there is no back-stack anywhere. | | |
| `08/19` | 19 | **Quit-confirm clobbers (the preserved gap)** — requesting close while any modal is open replaces it with the quit confirm, and cancelling that confirm leaves **no** modal, not the one it clobbered. | | |
| `08/20` | 20 | **Per-modal Escape semantics** — spot-check the asymmetries: Teardown's Escape means "skip", RemoveProject's is refused while busy, Updating's is refused mid-update, TmuxChoice's persists nothing, everything else closes. | | |
| `08/21` | 21 | **Focus on open** — every modal with a field has the caret in it immediately; every modal without one still responds to its letter keys and Escape with no click first. | | |
| `08/22` | 22 | **Changelog overlays Settings** — opening the changelog from Settings returns to Settings on dismiss. | | |
| `08/23` | 23 | **Toasts from modals** — a failing session spawn from the launcher/agent picker shows `failed to start session: …` in the statusbar and it clears on the error TTL. | | |

**Deferrals that a later phase closed:** row `08/12`'s Settings **Tools** gap —
**closed by Plan 09**, whose row `09/A` says in its own words "This closes the
Plan 08 **Modals** row 12 gap". Run `08/12` in full; the Tools pane exists.

Row `08/13`'s ⌘-SVG substitution stays macOS-only and is carried below as row
**P2**.

## Plan 09 — System (10 rows + row A = 11)

Source: `2026-07-31-gpui-rewrite-09-system-checklist.md`.

| origin | # | row (verbatim) | PASS/FAIL/DEFER | notes |
|---|---|---|---|---|
| `09/1` | 1 | **31 + custom themes, 11 tokens, follow-system with the startup seed order** — every builtin and every `themes.json` custom theme resolves; the 11 tokens and their contrast partners render identically to iced; flipping the OS appearance flips the app; a fresh launch in follow-system mode paints the **right** mode on the first frame, not a flash of the wrong one. *(Shipped in Plan 03.)* | | |
| `09/2` | 2 | **Zoom 0.6–2.0 whole-app, driving logical PTY dims** — every step re-lays the chrome and the terminal together, and the PTY's reported `(rows, cols)` matches iced at the same window size and zoom. *(Plan 03/07.)* | | |
| `09/3` | 3 | **Sidebar width / zoom / grid order / settings persisted** — set all four, quit **normally**, relaunch: all four come back. Then set the zoom and quit **within 250 ms** (the debounce window) — it must still come back. Then rearrange the grid and quit from grid view — the order comes back. | | |
| `09/4` | 4 | **tmux sidecar discovery/reattach** — with live agent sessions, quit grove and relaunch: every tmux-backed session reappears in the tree with its project/worktree/label/agent intact and its scrollback live; native sessions do **not** reappear; a session killed outside grove leaves no ghost row and no orphan sidecar; the reattached terminal is sized correctly on the first frame, not after a resize. Then turn tmux **off** and back **on** in Settings and confirm the re-scan neither duplicates nor drops a session. | | |
| `09/5` | 5 | **Attention stale-file GC at startup** — leftover state files from a previous run are cleared before any session spawns, and a reused session id cannot read a stale file. *(Plan 03/06.)* | | |
| `09/6` | 6 | **Login-PATH recovery** — launched from a desktop launcher (not a shell), agents still resolve on `$PATH`. *(Plan 03.)* | | |
| `09/7` | 7 | **Panic hook + telemetry** — with no key compiled in, **nothing** is transmitted (say whether this was confirmed with a network monitor or by reading the three gates); the panic hook logs the message locally and would transmit only a scrubbed location; the Settings **Telemetry** row defaults to on, persists, and flipping it takes effect immediately. | | |
| `09/8` | 8 | **arboard + OSC52 clipboard** — copy a terminal selection and paste into another app; paste in; on Wayland the `wl-paste` file-URI fallback still works. *(Plan 04.)* | | |
| `09/9` | 9 | **Adaptive 60 ms/1 s tick and its gating** — an unfocused, quiet window drops to the slow cadence while a background agent still streams and classifies at full rate. *(Plan 03/06; the numeric idle-power measurement is Plan 10 — eyeball only here.)* | | |
| `09/10` | 10 | **3 s-delayed + 24 h/refocus upgrade checks, changelog, apply/restart** — the launch check fires ~3 s after startup and not before; a newer release lights the **green dot on the cog** and offers Update / Skip / Copy URL; Skip suppresses that tag and a *newer* tag surfaces again; a manual check from Settings surfaces its error inline while a silent one stays quiet; the changelog fetches the 10 most recent releases, renders them, and **returns to Settings** on dismiss; starting an update shows live Downloading → Building → Installing stages with the window still responsive, **Escape is refused while it runs**, and Restart relaunches into the new version with the sidebar width and zoom intact. | | |
| `09/A` | A | **Settings → Tools** — the section lists Claude, Codex and OpenCode (never the plain Terminal), each row showing "Detecting…" then either its `--version` or "Not installed"; a missing tool's label and dot recede (hollow dot, dimmed label); "Set default" appears only on installed non-default tools and adopting one persists `default_agent`; "Re-detect tools" re-runs the scan without freezing the window. This closes the Plan 08 **Modals** row 12 gap. | | |

**Note on `09/9`:** that row says "the numeric idle-power measurement is Plan 10
— eyeball only here". The numbers now exist; row **P5** below is where they are
accepted or rejected.

## Plan 10 — the six rows this plan itself owes (6 rows)

These exist nowhere else.

| origin | # | row | host | PASS/FAIL/DEFER | notes |
|---|---|---|---|---|---|
| `10/P1` | P1 | **macOS dock badge + one bounce per enter-while-unfocused** (Plan 06 row 9's macOS half) | macOS | | |
| `10/P2` | P2 | **macOS ⌘ chords + the ⌘-SVG substitution in ShortcutOverlay** (Plan 08 row 13's deferral), incl. the Cmd+Opt+H collision workaround and Cmd-chord suppression in the palette (spec §5, §7) | macOS | | |
| `10/P3` | P3 | **IME composition** inside every text-heavy modal field (findings §S2) — compose a CJK string in the launcher, the add-project path field, and a multiline editor; the preedit renders in place and commits once | Linux + macOS | | |
| `10/P4` | P4 | **Wayland clipboard round-trip** (findings §S4) — copy from the terminal into another Wayland app and back; the `wl-paste` file-URI fallback; OSC 52 from inside tmux | Wayland | | |
| `10/P5` | P5 | **Idle-power numbers accepted** — read `…-10-idle-power.md` and accept or reject each of the four scenarios | Linux | | |
| `10/P6` | P6 | **Screenshot sweep reviewed** — work down `…-10-screenshot-sweep.md` and fill every verdict cell | any | | |

**On P1 and P2:** these are the only rows that cannot be run on this machine at
all. If no macOS host is available, mark them **DEFER** and say so explicitly.
Phase C may then proceed **only** if the user accepts shipping with macOS
unverified — and **that acceptance must be written down**, here, in the user's
own words:

> macOS acceptance (user, verbatim): _______________________________________

---

## Open decisions — what the gate *decides* rather than verifies

These are not pass/fail rows. Each needs an answer, and Task 7 acts on it.

### D1 — the CJK `force_width` helper (Plan 04 row `04/2`)

Row `04/2` asks the user to judge whether the `force_width` attempt improved the
wide-character slot fill, made it worse, or was unavailable. That verdict is
what decides whether the helper survives, so it cannot be settled before the
gate. **Leave the helper in place until the user answers.** Task 7 deletes it
only if the answer is "delete".

> Decision: ☐ keep  ☐ delete  ☐ keep, revisit post-merge — reason: ______________

### D2 — the duplicate tmux attach for a home terminal (found in Task 2, **not on any checklist**)

Measuring idle power with an identical empty `GROVE_CONFIG_DIR` showed:

- **grove-gpui** spawns **two** `tmux -L grove … attach-session` children pointed
  at the **same** target (`…__terminal__6` twice) for a single home terminal;
- the **iced** build spawns a **single** native `/usr/bin/zsh`.

This is reproducible. Plan 10 Task 1 Step 4's rule is that a genuine functional
gap not on any checklist is a **scope decision, not a worker decision**, so it
was **not fixed**. It is plausibly the reason gpui's Scenario-A idle cost is
higher than it needs to be.

> Decision: ☐ fix before Phase C  ☐ fix after the cutover  ☐ accept — reason: ______________

### D3 — zoom scales cells, not chrome (recorded by Task 1, **architectural**)

Task 1 made the single-session PTY grid match iced's `compute_pty_dims`
**exactly (delta 0)** at zoom 1.0, for sidebar 320, sidebar 220 and zen. It
**cannot** be made exact at other zooms, and the residual is large:

| config | gpui `(rows, cols)` | iced oracle `(rows, cols)` |
|---|---|---|
| 1280×800, zoom **1.0**, sidebar 320, chrome | (39, 122) | (39, 122) ✅ exact |
| 1280×800, zoom **1.0**, sidebar 220, chrome | (39, 135) | (39, 135) ✅ exact |
| 1280×800, zoom **1.0**, sidebar 320, **zen** | (43, 165) | (43, 165) ✅ exact |
| 1280×800, zoom **2.0**, sidebar 320, chrome | (19, 61) | (15, 37) ❌ |
| 1280×800, zoom **0.6**, sidebar 320, chrome | (65, 204) | (70, 236) ❌ |

**Cause:** iced applies `ui_zoom` as the *application scale factor*, so the whole
viewport shrinks and `compute_pty_dims` divides every chrome constant by the
zoom. grove-gpui applies zoom as `rem_size` + cell size only — the appbar,
statusbar, sidebar and session header are `px()`-sized and do **not** scale — so
a zoomed grove-gpui keeps its chrome and hands the terminal proportionally more
cells. This is findings amendment 7 ("`compute_pty_dims`'s chrome subtraction is
superseded by gpui layout") playing out at the extremes; **padding cannot close
it, because the gap grows with zoom**. Both tests are pinned in
`views/workspace.rs` so a future change has to come past them.

Row `09/2` ("the PTY's reported `(rows, cols)` matches iced at the same window
size and zoom") is the row this bears on — judge it with this table in hand.

> Decision: ☐ gpui behavior is correct, amend `09/2`  ☐ port iced's scaling  ☐ defer — reason: ______________

### D4 — Task 1 Step 4 won't-fix list

| item | status | reason |
|---|---|---|
| Plan 06 row 16 "no toast producer" | **confirmed closed by Plan 07** | Plan 07 row 13 ships the producer and names the trigger. Nothing to fix. |
| Plan 08 row 12 Tools gap | **confirmed closed by Plan 09** | Plan 09 row A states it closes exactly this gap. Nothing to fix. |
| Plan 04 row 2 CJK `force_width` | **deliberately not touched** | Its survival is a checklist *outcome* — see D1. Deleting it before the gate would destroy the thing being judged. |
| D2 duplicate tmux attach | **won't-fix in Phase A** | New functional gap, on no checklist ⇒ scope decision, not a worker decision. |
| D3 zoom/chrome divergence | **won't-fix** | Architectural, recorded, pinned by tests. Not closable by padding. |

---

## What "signed off" means

Every row above is marked, every open decision above has an answer, and the user
has said — explicitly, in their own words — that the sign-off is complete and
Phase C may proceed.

**If any row FAILs:** Phase C does not start. The failure comes back as new work,
the affected rows are re-run, and the gate is re-presented. Deleting the oracle
while a parity row is red destroys the only thing that can diagnose it.

