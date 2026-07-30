# Grove gpui Rewrite — Design

**Date:** 2026-07-31
**Status:** Approved for planning
**Decision:** Full rewrite of Grove's GUI layer from iced 0.14 to gpui, on a big-bang branch.

## 1. Goals and non-goals

**Goals**
- Feature-identical Grove from the user's POV: every screen, modal, animation, shortcut, and terminal behavior survives. Pixel-familiar, not pixel-perfect (renderer differs).
- Linux (Wayland + X11) and macOS at parity, including the macOS dock badge/bounce.
- Internals rebuilt idiomatically for gpui: entities, events/observation, actions + context-scoped keymaps, focus system. No mechanical Elm port.
- `crates/grove-core` reused unchanged except for the terminal-parser swap behind a trait.

**Non-goals**
- UX redesigns of any kind (deferred until after the port lands).
- Windows support.
- Keeping the iced code alive after the switch (deleted at the end of the branch).

**Motivations:** rendering/perf ceiling of iced, terminal text quality, ecosystem bet on gpui's trajectory.

## 2. Workspace layout

```
crates/grove-core/      unchanged domain layer (sessions, tmux, git, attention, storage, themes)
crates/grove-terminal/  NEW: alacritty_terminal wrapper. PTY I/O thread, grid state, damage
                        generations, selection model. No gpui types; testable headless.
src/                    gpui app
  main.rs               gpui::Application, window options, close-request interception
  app.rs                root wiring; globals: ThemeState, Settings, AppState
  entities/             state entities: workspace_state (selection/focus/grid/zen/panel — single
                        owner), project_tree (projects/worktrees/wt_cache+generation),
                        attention_state, upgrade_state, terminal_registry, animation_clock
  views/                one entity per stateful view (Zed granularity): workspace, sidebar,
                        appbar, statusbar, terminal_view, launcher, onboarding,
                        modals/* (single-slot ModalLayer; each modal its own entity)
  terminal_element.rs   custom Element painting the grid (paint_quad + shape_line, Zed-style)
  theme.rs              grove-core Theme -> gpui colors; follow-system via window appearance
  keymap.rs             actions + KeyBindings generated from the SHORTCUTS registry
  platform/             macOS dock (raw objc, ported as-is), drag-drop, clipboard/OSC52
```

**Dependency pinning:** gpui pinned to a git rev of `zed-industries/zed` (pre-1.0; crates.io lags). `gpui-component` (Apache-2.0) provides single-line inputs and multiline editors; its rev pinned in lockstep with the gpui rev it expects.

## 3. Terminal subsystem

- `alacritty_terminal` replaces `vt100`. **Reflow-on-resize is suppressed** to match vt100's non-reflowing behavior (native PTYs — home terminals, panel shells, teardown, tmux-off agent sessions — depend on it; `AbsCell` selection coordinates assume it).
- **Token-space cells:** grove-terminal emits `{text, fg: TermColor, bg: TermColor, bold}` runs where `TermColor = Default | Ansi(u8) | Rgb`. Color resolution (ANSI→theme-token table from `pty.rs::ansi_idx`, inverse-swap semantics, per-project pinned themes, launcher live preview) happens in `TerminalElement::paint`. alacritty's own palette is bypassed entirely. Italic/underline/dim/strikethrough remain ignored (explicit parity decision).
- **API contract is the port's first artifact** — the trait grove-core's `Session` swaps its parser behind: `process(bytes)`, `snapshot()`, `tail_contents(n)`, `selection_text(abs..abs)`, `title()`, `bell_count()` (own monotonic counter off `Event::Bell`), `mouse_mode()/encoding()`, `app_cursor()`, `cursor()` (+hidden), `display_offset()/scroll_to(n)`, `resize(rows, cols)` (scrollback snap-to-0 first, as today), damage generation counter. Grove's plain cell-rectangle selection model is reimplemented over the grid — alacritty's semantic `Selection` is NOT used.
- tmux interplay (copy-mode driving, `cancel` on keystroke, SGR/X10 mouse forwarding with the 223-coord X10 limit, scroll accumulation crossing CELL_H with direction-reset) ports verbatim; none of it touches the parser.
- Painting: bg quads merged per run, text via `text_system().shape_line().paint()` at fixed cell columns, cursor + selection overlays. Repaint driven by damage generations + `cx.notify`, keyed by `Session::id` — no Arc-address cache keys, no global 60ms repaint timer.

## 4. State architecture

- **Single-owner rule:** `WorkspaceState` owns `active_session/proj_idx/wt_idx/terminal_focused/focused_pane/grid_*/zen`. Views read and call its methods; they never mutate selection. This replaces today's bidirectional `sync_wt_to_session`/`sync_session_to_wt` drift surface.
- **Attention is never event-driven.** `acknowledge_session` remains a direct synchronous call on every focus-transition path (grid move/entry, tile press, window refocus, jump-to-waiting, launcher switch). Classification runs as an independent 480ms background task over a synchronously captured `(sessions meta, active_session, window_focused)` snapshot. Precedence (native poller > hook state file > screen-scrape `classify()`), focused-never-WaitingForInput, bell-reset resync, quiet windows, OSC-title staleness belt: ported verbatim from `tick.rs`/`activity.rs`.
- **Modal layer is single-slot**, preserving the one-modal-deep contract including its documented quirks (quit-confirm replaces any modal and cancel does not restore; per-modal Escape semantics; changelog overlays Settings). Each modal is its own entity with its own key-context string.
- **AnimationClock entity** replicates the adaptive tick: 60ms when `busy || (has_ptys && (focused || animating || dirty))`, else 1s — all blink phases (cursor `%16<8`, 3-dot `/5%3`, toast pulse `%40`, spinner `/3`) read this one counter so phase relationships and idle-power behavior are preserved. Attention pulse (1s auto-reverse EaseInOut) and onboarding entrance map to `with_animation`; grid slide is a paint-time transform in the tile element.
- Tick decomposition: git 5s poll (in-flight guard), wt-rebuild (generation guard), upgrade drain, teardown poll, toast TTL become independent tasks; zoom-save tick-debounce becomes a 250ms timer.
- Async: gpui foreground/background executors + `Timer` replace iced subscriptions and tokio (only tokio use today is a 3s startup sleep). grove-core's `std::thread` + mutex-drain patterns port unchanged, bridged via channels awaited from `cx.spawn`.

## 5. Input

- Actions declared with `actions!`; **bindings and the shortcut overlay both generated from the existing `SHORTCUTS` registry** (single source of truth, cannot drift).
- Scoping via gpui key-context strings on focused elements' paths (Workspace/Grid/Zen contexts; each modal sets its own), replacing the `should_forward` carve-outs, `MODAL_OPEN`/`PALETTE_OPEN` statics, and the `ModifiersChanged` side channel. Observable contract preserved and enforced by the keyboard matrix test (e.g. Escape-despite-capture, palette arrow carve-outs, Cmd-chord suppression in palette, Alt+Escape reaching the PTY as ESC ESC, two-step confirm-kill arming/disarming).
- Key→PTY byte table (`keys.rs`: Ctrl arithmetic, Alt=ESC prefix), bracketed paste with `\n→\r`, Wayland `wl-paste` file-URI fallback, file-drop shell-escaping: ported verbatim.

## 6. Theming, fonts, zoom

- 31 built-in + custom `themes.json` themes, 11 tokens with contrast partners, follow-system (seed + appearance observation, first-frame fallback order preserved), one-time stale-name migration: all reuse grove-core; `theme.rs` converts tokens to gpui colors.
- Bundled BlexMono Nerd Font registered via `AssetSource` (rust-embed); startup assertion that measured em advance == 7.5 @ 12.5pt, line height forced to 17px in the terminal element (mirrors today's `metrics.rs` test). `mono_covers` cmap hack expected unnecessary (gpui font fallback) — verified in spike 1, kept if not.
- SVG icons: existing single-color generated SVGs served from an in-memory `AssetSource`, tinted via `text_color`. Spinner stays 12 pre-rotated frames advanced by the clock (parity).
- **Zoom 0.6–2.0 via two rem scopes** (Zed's pattern, `WithRemSize`): chrome scope and terminal-content scope. PTY dimension math (`compute_pty_dims`) derives from the content scope so logical grid dims match today. Pinch/keyboard adjust + 250ms debounced persist.

## 7. Platform

- Linux: gpui's renderer is **wgpu as of Feb 2026** (Blade/Vulkan removed upstream) — broad backend fallback, NVIDIA/Wayland freezes fixed. X11 + Wayland both supported. Wayland file drag-drop is unverified first-party → spike 4; `wl-paste` hack stays until proven unnecessary.
- macOS: dock badge (waiting count) + bounce-once-per-enter has no gpui API; today's raw `objc` code ports as-is. Cmd-based shortcuts, Cmd+Opt+H collision workaround preserved.
- Close-request interception (running native sessions → quit confirm) via gpui's should-close callback; `flush_ui_zoom_save` on all exit paths. rfd (or gpui `prompt_for_paths`) with the `picker_open` guard. Clipboard: arboard + OSC52 as today. No single-instance mechanism exists today; none is added.

## 8. Parity verification harness (built BEFORE porting UI)

1. **Dual-parser golden tests:** recorded PTY byte streams (claude/codex/tmux sessions, vim, resize storms, existing `activity.rs` fixtures) fed to both vt100 (kept in-tree as oracle until the end) and grove-terminal; assert cell-by-cell equality of chars, token colors, cursor, title, bell count, `tail_contents` output.
2. **Keyboard matrix test:** table-driven — every `SHORTCUTS` row × {Workspace, Grid, Zen, each modal} × armed/disarmed, plus the `key_to_bytes` passthrough table; asserts dispatch target (PTY bytes vs action vs swallowed).
3. **Screenshot sweep:** both binaries scripted through every screen/modal × 3 zooms × 4 representative themes + follow-system flip × grid n∈{1,2,3,5} × panel open/zen; side-by-side human review.
4. **Per-screen parity checklist** (appendix A) — the exit gate for every build phase.

## 9. Spikes (throwaway, before committing to the port order)

1. **TerminalElement end-to-end** (riskiest): grove-terminal + element running `claude` in tmux — 7.5×17 metrics, Nerd Font/CJK fallback, scroll accumulation, mouse reporting, selection paint+extract, token-space theming, damage-driven repaint. Also verifies the monospace advance-width API.
2. **Text inputs:** gpui-component Input + multiline editor — palette search behaviors (Escape, ←→, Cmd-chord suppression), 3-buffer scripts editor, IME, clipboard.
3. **Zoom** via two rem scopes incl. PTY-dim math and live adjust + debounced persist.
4. **Linux platform matrix:** Wayland+X11 window behavior, close-request interception, file drag-drop on Wayland, clipboard.
5. **Idle-power model:** unfocused-quiet app ≈ today's 1s-tick cost; busy background agent still classifies at ~480ms and paints smoothly.

## 10. Build order (risk-first)

1. Spikes 1–4.
2. `crates/grove-terminal` + dual-parser golden harness.
3. App shell: root entities/globals, theme + follow-system, font registration + metric assertion, zoom, `SHORTCUTS`-generated keymap skeleton, AnimationClock, storage wiring.
4. TerminalElement + single-session workspace + full input path (keys/copy-paste/drop/scroll/click-to-caret/selection). Burn in the core early.
5. Sidebar tree (rows, activity glyphs, git suffix, 3-mode collapse cycle, hover actions, divider drag) + WorkspaceState sync logic.
6. Appbar + statusbar + attention queue + dock badge/bounce + toasts + 480ms activity task.
7. Grid view (layout math, focus/swap, drag reorder, slide anim) + zen + terminal tab + worktree panel + panel divider.
8. Modals, simple→text-heavy: Confirm/Message/ShortcutOverlay/TmuxChoice/AgentPicker → Settings → ThemePicker → SessionLauncher → AddProject/Onboarding → RemoveProject/Archive/Teardown → ScriptsEditor + ThemeManager editor.
9. Upgrade flow + telemetry + quit paths + persistence debounces.
10. Parity passes: keyboard matrix green, screenshot sweep, checklist sign-off, idle-power measurement.
11. Delete vt100 + iced; keep golden fixtures as grove-terminal regression tests.

## Appendix A — Parity inventory (acceptance checklist)

Every row must be demonstrably identical post-port. File references are to the iced code as the behavioral oracle.

**Screens/layout:** main workspace (sidebar+appbar+statusbar+session view; 1280×800 default, title "grove"); sidebar project→worktree→session tree with 3-mode expand cycle, per-row hover actions, git dirty/ahead/behind suffix (5s off-thread poll), archived-projects row, drag-resizable divider 220..min(win/2,win−400) with 350ms double-click reset to 320, pinned TERMINALS section (always ≥1 home terminal); grid ≤4×4, cols=ceil(sqrt(n)).clamp(1,4), short-column tiles fill height, header drag-reorder with 150ms slide, zen button, per-tile PTY resize on reorder, order persisted by stable session key; zen chrome-hidden with grid/terminal restore bookkeeping; terminal tab (native PTYs at ~) and right worktree panel 20–75% split (Ctrl+Shift+←/→ step 5, draggable divider, double-click reset), Agent/Panel focus routing; statusbar running count/tmux label/theme name/hint chips/toast with kind-dependent TTL; appbar attention pill + dropdown.

**Modals (one-deep, replace-don't-stack, incl. quit-confirm-clobbers gap):** Input, Confirm (incl. Quit), AddProject wizard w/ dir autocomplete, RemoveProject w/ async teardown progress, ArchiveProject + ArchivedProjects, Message, TmuxChoice, AgentPicker, SessionLauncher (recents-first palette, drill-in panes, live project-theme preview), ThemePicker (dark/light tabs, follow-system, app vs project scope), ThemeManager + multiline editor, Settings (immediate persist), ShortcutOverlay (registry-generated), Teardown (embedded live PTY), ScriptsEditor (3 multiline editors), Updating, Onboarding wizard (Tab focus alternation, entrance anim).

**Terminal:** CELL 7.5×17 @ 12.5pt BlexMono; ANSI→token map (0→bg_strip, 1|9→red, 2|10→green, 3|11→yellow, 4|12→blue, 5|13→magenta, 6|14→cyan, 7|15→fg, 8→fg_mute, cube v=55+40x, gray v=8+10n); inverse swap w/ theme-default fill; per-project pinned content themes, chrome global; project-themes toggle invalidates; block cursor, 533ms blink, hidden-cursor respect; selection in absolute scrollback-stable coords surviving scroll, overlay rgba(.40,.50,.78,.35), trailing-whitespace-cleaned extraction, tick-driven edge auto-scroll during drag, keypress kills selection+drag, focus-changing click doesn't move caret; trackpad pixel accumulation crossing CELL_H w/ direction reset, Lines |y|<1 swallowed; mouse-report forwarding vs self-scroll; tmux copy-mode drive + single cancel on keystroke; Shift+PgUp/Dn/Home/End, mod+u/d half-page, 200-notch flood cap, typing snaps to bottom; click-to-move-caret same-row-only, DECCKM-aware, no-op scrolled-back/hidden; Ctrl/Alt byte synthesis; bracketed paste \n→\r; wl-paste file-URI fallback; file drop shell-escaped + trailing space.

**Shortcuts:** registry-driven; mod = Cmd (mac) / Ctrl+Shift (other); alt-chords; mac grid-swap Shift-or-Alt; screen-scoped w/ fall-through-to-PTY; two-step confirm-kill (sessions and home terminals separately armed, Escape disarms); copy/paste per-platform; 1-9 select; overlay from registry.

**Attention/activity:** 480ms cadence; precedence native `claude agents --json` poller > hook state file > screen-scrape classify; focused never WaitingForInput; bell diff w/ backwards-reset resync; scroll/input quiet windows; OSC-title braille working marker w/ 60s staleness; Done only for non-Terminal agents; acknowledge on focus/refocus truncates state file; dock badge = waiting count, one bounce per enter-while-unfocused; amber pulse 1s auto-reverse; 12-frame spinner every 3 ticks; 3-dot `(tick/5)%3`.

**System:** 31+custom themes, 11 tokens, follow-system w/ startup seed order; zoom 0.6–2.0 whole-app driving logical PTY dims; sidebar width/zoom/grid-order/settings persisted; tmux sidecar discovery/reattach; attention stale-file GC at startup; login-PATH recovery; panic hook + telemetry; arboard+OSC52 clipboard; adaptive 60ms/1s tick and its gating (idle power); 3s-delayed + 24h/refocus upgrade checks, changelog, apply/restart.
