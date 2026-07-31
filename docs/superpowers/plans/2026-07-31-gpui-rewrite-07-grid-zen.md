# gpui Rewrite Plan 07: Grid view, zen, the terminal tab and the worktree panel

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. This is **production** code: the workspace clippy denies apply (`unwrap_used`/`expect_used`), superpowers:test-driven-development governs every pure helper (tests before implementation, red before green), and superpowers:verification-before-completion governs every "done" claim — read raw command output, never a summary line. Also load the `gpui-development` skill before writing any gpui code; training-data gpui is stale and this rev is pinned.

**Goal:** Grove currently has exactly one screen. `WorkspaceState::grid_view` is a field that is always `false`, `chrome_visible` is a `let` binding in `Workspace::render` that is always `true`, `GridMove`/`GridSwap` are the only registry actions with no `KeyBinding`, `ToggleGrid`/`ToggleZen`/`ToggleTerminal`/`NewHomeTerminal` all dispatch to logged stubs, and the worktree terminal panel does not exist at all. This phase builds the other three screens — **grid**, **zen**, and the **home-terminal tab** — plus the right-docked **worktree slide-over panel** and its split divider, and turns `FocusedPane` from a declared-but-unread field into the real Agent/Panel input routing. It is also the first phase where several `TerminalView`s render at once, so it is where "which terminal owns the keyboard" stops being trivial.

Exit gate (master plan row 07): **Grid+zen checklist rows green** (spec Appendix A → the grid/zen/terminal-tab/panel clauses of *Screens/layout*, enumerated verbatim in Task 7 Step 2, plus the four rows Plans 04/06 explicitly deferred here); `./install.sh` green; one commit.

**Out of scope — do not build it here.** Every modal and every text input, so the `+` segment, the cog, the two statusbar chips, the session bar's `run script` button and the sidebar's modal actions all keep dispatching to their logged Plan 08 stubs. The upgrade flow, telemetry, quit paths and persistence beyond `grid_order`/`sidebar_width`/`ui_zoom` (Plan 09). tmux sidecar discovery/reattach (Plan 09). The screenshot sweep across grid n∈{1,2,3,5} × panel/zen (Plan 10 — this phase's checklist is a side-by-side human pass, not the scripted sweep). `gpui-component` and its durable pin stay a Plan 08 decision; this phase adds no text input.

**Architecture (new/changed files only):**

```
crates/grove-gpui/
  src/entities/
    workspace_state.rs    MODIFIED: the Plan 07 stub fields (`grid_view`, `zen`,
                          `focused_pane`) become real, joined by `tile_order`,
                          `grid_focused`, `grid_view_before_zen`,
                          `grid_view_before_terminal`, `term_panel_open`,
                          `term_panel_portion`; plus every pure transition
                          (enter/exit grid, toggle zen, tile zen, teardown
                          reconcile, terminal-tab toggle, panel toggle/resize).
    session_registry.rs   MODIFIED: per-worktree panel shells — a third
                          collection beside `order` and `home`, keyed by
                          worktree path, with an active index per path
                          (`src/app/terminals.rs:110-176`).
  src/grid.rs             NEW. The pure grid math ported from iced:
                          `grid_layout`, `grid_neighbor`, `session_grid_key`,
                          `reconcile_tile_order`, `swap_tiles`,
                          `grid_focus_after_kill`, `slide_progress`/`GRID_SLIDE`.
  src/views/
    grid.rs               NEW. The tile grid: columns-of-tiles layout, per-tile
                          header (reusing `session_header`), respond chip, num
                          hint, waiting border + scrim, drag/drop overlays, the
                          150ms draw-only slide.
    terminal_tab.rs       NEW. The home-terminal bar (`home_terminal_bar`,
                          `terminal.rs:420-485`) — restart + zen, no kill.
    term_panel.rs         NEW. The right-docked slide-over: tab strip, add/close,
                          collapse, the resize handle, the split.
    session_header.rs     MODIFIED: gains the tool cluster (run/term/zen/kill)
                          and a per-tile variant.
    appbar.rs             MODIFIED: `zen_attention_pill` (`appbar.rs:244-305`)
                          and the segmented grid combo's live branch.
    workspace.rs          MODIFIED: the four-screen render (`view/mod.rs:66-99`),
                          multi-`TerminalView` hosting, the real `chrome_visible`,
                          grid/zen key contexts, the new action handlers.
  src/keymap.rs           MODIFIED: `GridMove`/`GridSwap` gain data-carrying
                          actions + bindings (the `SelectSession` pattern), and
                          the Ctrl+Shift+←/→ panel step.
```

**Tech stack additions:** none. Pins unchanged.

## Global Constraints

- Branch: `gpui-rewrite`. Toolchain regime is **identical to Plans 03–06** and is not re-litigated:
  - grove-gpui builds/tests/clippy only via `PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 -p grove-gpui`.
  - Bare `cargo build` / `cargo test` (default-members, rustc 1.94.1) must keep working untouched for `grove`, `grove-core`, `grove-terminal`. Never run `--workspace`.
  - clippy for grove-gpui runs **`--no-deps`**: `cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings`.
  - `rustfmt --edition 2021` on **touched files only**.
- **Pins are law.** ZED_REV `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba`; alacritty fork `4c129667ce56611becdc82de6e28218c80e2e88f`. No `[patch]`, no `gpui-component`.
- **Constraint 3 — grove-core and the iced app are read-only.** No edits under `src/`, `crates/grove-core/`, or `crates/grove-terminal/`. Amendment protocol unchanged: if a genuinely **UI-free** helper must be exposed from grove-core, **STOP and report**; the orchestrator authorizes it. Do not edit grove-core on your own judgement.
  - **Expected outcome this phase: no amendment is needed.** Every helper this phase ports lives in the *iced* crate (`src/gui/metrics.rs`, `src/gui/launcher.rs`, `src/gui/update/shortcuts.rs`), which is read-only-as-oracle, not as a dependency — grove-gpui reimplements them with their tests, exactly as Plan 05 did for `path_basename`. From grove-core it needs only what it already uses: `Store::grid_order` (`storage.rs`), `storage::persist`, `Agent`, and the spawn surface Plan 05/06 already consume.
  - **Foreseen candidate spots** (report, do not act): (1) anything you want from `grove_core::session::Session` — still forbidden, it owns the vt100 parser; (2) `App::wt_terminals`/`ensure_wt_terminal` (`src/app/terminals.rs:110-176`) are iced-app types — port the *shape* into `SessionRegistry`, do not try to share them.
- **Behavior questions are answered by reading the iced code, never by guessing.** Canonical oracles for this phase, cited per task:
  - `src/gui/metrics.rs` — the layout authority: `TILE_HEAD_H`/`TILE_PTY_PAD_*` (:51-56), `TERM_PANEL_PORTION` and its MIN/MAX/STEP (:33-44), `term_portion_for_cursor` (:257-263), `pty_cols_for_fraction` (:302-321), `grid_layout` (:325-330), `grid_tile_cols` (:336-344), `grid_tile_rows_for_col` (:351-360), `grid_tile_size` (:367-375). Its tests (:558-607) are the acceptance suite for the ported half.
  - `src/gui/view/terminal.rs` — the rendering authority: `term_panel_resize_handle` (:78-87), `grid_workspace` (:90-179), `workspace` incl. the 100−portion/portion split (:181-229), `term_panel` (:234-316), `term_panel_tab` (:319-393), `terminal_workspace` (:398-414), `home_terminal_bar` (:420-485), `sess_bar` and its tool cluster (:487-620), `grid_tile` (:811-1175) — header/respond chip/num hint/drop-zone/drag-dim/waiting border/scrim, `focused_input_pane` (:1180-1186), `selection_pane` (:1192-1198).
  - `src/gui/update/layout.rs` — the transition authority: `on_toggle_zen` (:63-103), `on_toggle_grid_view` (:199-216), `enter_grid` (:222-252), `exit_grid` (:257-269), `reconcile_grid_after_teardown` (:276-306), `on_grid_drag_start/hover/end` (:308-342), `on_grid_tile_zen` (:344-356), `begin_grid_slide` (:363-377), `refresh_pty_viewport` (:394-471), `persist_grid_order` (:481-489), `adjust_term_panel_portion` (:533-542), and the panel divider trio `on_term_panel_drag_start/move/end` (:162-197 — note the 350ms double-click reset at :164-174).
  - `src/gui/update/shortcuts.rs` — `grid_swap_mods` (:25-32), the `GridMove`/`GridSwap` registry rows (:337-366), the display-only panel-resize row (:368-384), `screen_from_flags` (:387-392), the by-hand grid arm ahead of the registry lookup (:444-470), Enter→`ToggleZen` (:476), `terminal_toggle_decision` (:528-557), `should_sync_grid_focus` (:564-566), `grid_focus_after_kill` (:574-580), `grid_neighbor` (:581-627 — read the doc comment, it is the whole contract), `GRID_SLIDE`/`slide_progress` (:629-650).
  - `src/gui/launcher.rs:43-81` — `swap_tiles`, `session_grid_key`, `reconcile_tile_order` (+ their tests at :328-370).
  - `src/gui/update/pty_input.rs` — `reset_focused_pane` (:128-137), `panel_focused` (:139-143), `focus_pane` (:146-158), `focused_session`/`_mut` (:163-215), `active_wt_path` (:220-226), `term_panel_resize_delta` (:413-423).
  - `src/gui/update/sessions.rs` — `on_toggle_term_panel` (:62-88), `on_new_wt_terminal` (:179-187), `on_select_wt_terminal` (:189-196), `on_close_wt_terminal` (:198-208), `select_visible_session`'s grid branch (:396-419).
  - `src/gui/update/mod.rs` — `on_toggle_terminal` (:472-500), `set_grid_focus` (:1061-1068), `grid_move` (:1070-…), `leave_terminal_tab` (:1139-1141), `terminal_tab` (:1467-1469).
  - `src/gui/view/mod.rs:66-99` — the four-screen composition and the zen stack; `src/gui/view/appbar.rs:244-305` — `zen_attention_pill`; `:46-149` — the segmented combo's grid branch.
  - `src/app/terminals.rs:110-176` — the panel-shell collection shape.
- **Interfaces Plans 03–06 already shipped — consume them, do not re-derive:**
  - `entities::animation_clock::{tick, toast_pulse, dots, spinner_frame, cursor_visible, set_busy_inputs}` — **`toast_pulse` gets its first and only consumer here** (the tile scrim's 40-tick triangle wave, `terminal.rs:1097-1100`; Plan 06 recorded ambiguity 3 left it deliberately unconsumed for exactly this).
  - `entities::activity_store::{state_of, pulse, waiting_count, waiting_sessions, acknowledge}` — the tile's waiting border/respond chip/scrim and the zen pill are all reads of these; the 480ms task is **not** touched.
  - `views::session_header::{SessionHeaderData, session_header, is_in_progress_title, truncate_middle}` — Plan 06 built it "parameterized by session, not by the active session" precisely so the tile header reuses it. Extend it; do not fork it.
  - `views::terminal_view::TerminalView` and `terminal_element::TerminalElement` — a tile hosts an ordinary `TerminalView`. **`ZoomState::pty_dims` from the element's own `prepaint` bounds already sizes every PTY** (findings amendment 7, `zoom.rs:54-64`, `terminal_element.rs:117-130`).
  - `entities::workspace_state::{WorkspaceState, FocusedPane, acknowledge, visible_order, select_session, …}` — the single owner. Views read and call; they never mutate selection.
  - `keymap::{SHORTCUTS, GlobalShortcut, Screen, screen_from_flags, bindings, select_session_bindings, binding_for}`, `views::rows::state_glyph`, `icons::icon`, `theme.rs` token fns, `settings::SettingsState`, `zoom::ZoomState`, `fonts::{UI_FAMILY, MONO_FAMILY}`, `entities::toast::{ToastState, ToastKind}`.
- **Carried amendments (do not re-derive):**
  1. **`compute_pty_dims`'s grid and split arithmetic is NOT ported.** `grid_tile_cols` (`metrics.rs:336`), `grid_tile_rows_for_col` (:351) and `pty_cols_for_fraction` (:302) exist in iced only because iced computes PTY dims centrally in `refresh_pty_viewport` and pushes them down; gpui derives them from each element's post-layout bounds (findings amendment 7, already law since Plan 04). A grid tile and a panel shell therefore size themselves **for free** — there is no `refresh_pty_viewport` in grove-gpui and this phase must not introduce one. What survives is `grid_layout` (which decides how many columns exist, a layout fact, not a PTY fact) and `term_portion_for_cursor` (which maps a cursor to a percentage). The three superseded functions are used **as test oracles only**, per amendment 2.
  2. **The parity assertion replaces the ported arithmetic.** Add tests that feed a nominal window (1280×800, zoom 1.0) through the gpui layout constants and assert the resulting `(rows, cols)` matches the iced oracle formula within **±1 cell** — one for a 2/3/5-tile grid (`grid_tile_cols`/`grid_tile_rows_for_col`) and one for the 40% panel split (`pty_cols_for_fraction`). Reimplement the oracle formula *inside the test module* from the line references above; do not export it from production code. A larger divergence is a real layout bug: STOP and report rather than widening the tolerance.
  3. **`GridMove`/`GridSwap` follow the `SelectSession` pattern exactly.** Plan 03 deviation 3 left them the only unbound registry actions because gpui `actions!` are unit structs and these carry `(dx, dy)`. Do what `select_session_bindings()` already does: declare data-carrying action structs, generate one binding per direction × modifier set from the registry rows (`shortcuts.rs:337-366`, `grid_swap_mods` :25-32 — **on macOS the swap modifier is Shift *or* Alt**, elsewhere Alt), scope them to the `Grid` key context, and **delete the `only_the_grid_actions_remain_unbound` carve-out in `keymap.rs`'s test** (`keymap.rs:596-613`) so the drift guard now covers every row with no exceptions. That deletion is a required deliverable, not an optional cleanup.
  4. **Key contexts, not screen flags, do the scoping.** iced computes `screen_from_flags(chrome_visible, grid_view)` and filters by hand; gpui sets a `key_context` on the focused element's path (spec §5). Keep `screen_from_flags` — it is already ported and its "zen wins over grid" test is a real invariant — but use it only to *choose the context string* (`Screen::label()` already yields `workspace`/`grid`/`zen`/`terminal`). One context per screen on the root, matching what `keymap::contexts_for` already emits.
  5. **The grid's chrome is appbar + statusbar, no sidebar** (`view/mod.rs:66-79`). Grid entry does **not** hide the chrome, and every grid-entry path sets `chrome_visible = true` first — a chrome-less grid is a state `screen_from_flags` cannot name (`shortcuts.rs:222-227` and the `chromeless_grid_is_not_a_nameable_screen` test, which is already ported at `keymap.rs:712-716`). Zen is the only chrome-hiding screen.
  6. **The tile slide is a paint-time transform, not a layout change** (spec §4, `src/gui/slide.rs:1-8`). iced needed a whole custom `Widget` to translate drawing without perturbing layout; in gpui this is a transform/offset applied while painting the tile, driven by `slide_progress(start, now)` (`shortcuts.rs:645-650`, 150ms `EaseOutCubic`). Port `slide_progress` verbatim and TDD it (0 at start, 1.0 at ≥150ms, monotone, `EaseOutCubic`'s value at the midpoint). Reproduce the easing arithmetically — `iced::animation::Easing` is not available and must not be pulled in.
  7. **Multiple `TerminalView`s, one focus.** `Workspace` already memoizes one `TerminalView` per `SessionId` (`workspace.rs:389-419`); grid mode renders one per tile from that same map — **do not create a second view per session**, or the two would fight over `prepaint`'s `resize`. Exactly one tile's view holds the gpui focus handle at a time, and that is what `grid_focused` means. Clicking a tile (header, body or scrim) focuses it, sets `active_session`, and **acknowledges** (`layout.rs:308-321`). Focus-changing clicks still must not move the caret — `TerminalView::press_focused` (`terminal_view.rs:52-57,262-271,308-321`) already implements this and its comment ("with one session, the only focus transition a press can cause is…") must be updated now that there are many.
  8. **`FocusedPane` routing is gpui focus, not a bool consulted at read time.** iced has no focus system, so `focused_session()` branches on `panel_focused()` (`pty_input.rs:163-186`). In gpui the panel's `TerminalView` and the agent's each own a `FocusHandle` and keystrokes go where focus is. `WorkspaceState::focused_pane` survives as the **persisted intent** that decides which handle to focus on open/re-anchor (`reset_focused_pane`, `pty_input.rs:128-137`: opening the panel focuses `Panel`; changing the active session re-anchors the panel to a new worktree and so re-focuses it), and clicking a PTY updates it (`focus_pane`, :146-158). Keep the observable contract, including the fallback at `pty_input.rs:170-178`: a worktree with **no** panel shell routes to the agent rather than swallowing keystrokes.
- **Recorded ambiguities, resolved by reading the oracle:**
  1. **The tile header is `session_header` with a different chrome, not a second renderer.** `grid_tile`'s header (`terminal.rs:988-1018`) shows agent icon + agent label + project + branch, right-aligned respond chip / num hint / zen / kill, at `TILE_HEAD_H = 22px` — a denser variant of `sess_bar`'s identity row. Plan 06 built `session_header` "parameterized by session" for exactly this. Add a compact mode to `SessionHeaderData`/its renderer rather than duplicating the truncation, the in-progress 3-dot and the branchless-segment rule.
  2. **The session bar's tool cluster lands here.** Plan 06 built `session_header`'s *content* but not its right-hand buttons (`terminal.rs:592-620`: run script / term-panel toggle / zen / kill-with-confirm). Three of the four are this phase's own actions; `run script` stays a Plan 08 stub (the sidebar already stubs `RunScript`). The kill button reuses the existing two-step confirm that backs `CloseFocusedSession`.
  3. **`mod+t` never touches `chrome_visible`** (`update/mod.rs:472-475`) — in zen it is a pure content swap. And it is `Scope::Global`, reachable from all four screens; the already-ported `toggle_terminal_matches_on_every_screen` test pins that.
  4. **The panel resize keys are display-only in the registry** (`shortcuts.rs:368-384`) — Ctrl+Shift+←/→ is matched by hand (`term_panel_resize_delta`, `pty_input.rs:413-423`) and **only on `Screen::Workspace`**, so that closing the panel lets those keys fall through to the PTY. Bind them the same way: a context-scoped action on the workspace context only, never on grid/zen. The registry row stays display-only for the Plan 08 overlay.
  5. **Toasts get their first real producers here.** Plan 06 shipped `ToastState` with no caller, so checklist row 16 was unverifiable. The producers reachable in this phase are the spawn-failure errors: `"failed to start session: {e}"` (`sessions.rs:482`) and `"terminal failed: {e}"` (`src/app/terminals.rs:104-108`, which is also the panel-shell path). Wire those two; every remaining producer is a Plan 08/09 modal.
- No `git` commands until Task 7. Do not commit intermediate tasks. The orchestrator runs `./install.sh` and the commit.

---

### Task 1: The pure grid math (TDD, no gpui)

**Files:**
- Create: `crates/grove-gpui/src/grid.rs`
- Modify: `crates/grove-gpui/src/lib.rs`/`main.rs` module list

**Interfaces:**
- Produces: `grid_layout`, `grid_neighbor`, `session_grid_key`, `reconcile_tile_order`, `swap_tiles`, `grid_focus_after_kill`, `should_sync_grid_focus`, `GRID_SLIDE`, `slide_progress`, `slide_offsets` — every one pure, every one tested before it exists.

- [ ] **Step 1: Port the layout and neighbor math with their tests**

`metrics.rs:325-330` (`grid_layout`: `cols = ceil(sqrt(n)).clamp(1,4)`, `rows = ceil(n/cols).min(4)`) and `shortcuts.rs:581-627` (`grid_neighbor`). **Read `grid_neighbor`'s doc comment before writing a line** — the asymmetry is the contract: tiles are numbered row-major (`tile_idx = row * cols + col`) but *rendered* into per-column stacks that skip any index ≥ n, so vertical moves require the naive target to exist (no "nearest tile" fallback) while horizontal moves clamp the row **downward** to the largest row that has a tile in the target column. Port the comment verbatim.

Tests first, from `metrics.rs:558-572` (`grid_layout` for n = 1,2,3,4,5,6,7,9,10,16,20) plus the n=3 case the doc comment names explicitly: from tile 1 (the lone right-hand tile spanning both rows), a left move must land on tile 2, not tile 0.

- [ ] **Step 2: Port tile-order reconciliation and swapping**

`launcher.rs:43-81` — `swap_tiles`, `session_grid_key`, `reconcile_tile_order` (saved order first, unmatched live sessions appended in vector order, saved keys with no live match skipped, duplicate keys "first live match wins"). Their four tests are at `launcher.rs:328-370`; port all four. Plus `grid_focus_after_kill` (`shortcuts.rs:574-580`) and `should_sync_grid_focus` (:564-566) with their semantics intact.

**Deviation to record in the module doc:** iced keys `tile_order` by *index into `App::sessions`*, which is why `reconcile_grid_after_teardown` exists at all. grove-gpui has stable `SessionId`s (Plan 05 Task 2), so `tile_order: Vec<SessionId>` and the index-shifting hazard disappears — but reconciliation against `Store::grid_order`'s **string keys** stays, because that is the cross-restart identity. Keep `reconcile_tile_order` operating on keys; adapt only the return type.

- [ ] **Step 3: The slide (carried amendment 6)**

`GRID_SLIDE = 150ms` and `slide_progress` (`shortcuts.rs:629-650`), with `EaseOutCubic` written out arithmetically. Plus `slide_offsets(src, dst, n) -> [(idx, d_col, d_row); 2]`, the port of `begin_grid_slide` (`layout.rs:363-377`) — note it is called **after** `swap_tiles`, so `src`/`dst` are post-swap positions and each tile's offset points back where it came from.

TDD: `slide_progress(t0, t0) == 0.0`; `>= 150ms → 1.0`; monotone non-decreasing; the `EaseOutCubic` midpoint value. And `slide_offsets` for a horizontal swap in a 4-tile grid, a vertical one, and a diagonal one.

- [ ] **Step 4: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui grid 2>&1 | tail -30
```

---

### Task 2: `WorkspaceState` grows the four screens (TDD, pure transitions)

**Files:**
- Modify: `crates/grove-gpui/src/entities/workspace_state.rs`

**Interfaces:**
- Produces: real `grid_view`/`zen`/`chrome_visible`/`focused_pane` plus `tile_order`, `grid_focused`, `grid_view_before_zen`, `grid_view_before_terminal`, `term_panel_open`, `term_panel_portion`; and the transitions `toggle_grid`, `enter_grid`, `exit_grid`, `toggle_zen`, `tile_zen`, `set_grid_focus`, `grid_move`, `grid_swap`, `reconcile_after_teardown`, `toggle_terminal_tab`, `toggle_term_panel`, `adjust_term_panel_portion`, `set_term_panel_portion`, `focus_pane`, `reset_focused_pane`.

- [ ] **Step 1: The fields**

Replace the `// Plan 07 owns these` block (`workspace_state.rs:205-209`) with the real set, mirroring `src/gui/state.rs:109-170,282-289`. `chrome_visible` is the *inverse* of iced's naming confusion — iced stores `App::chrome_visible` and derives zen from it; grove-gpui already declared a `zen` field. **Pick `chrome_visible` as the stored truth and make `zen()` its negation**, because `screen_from_flags` (already ported) takes `chrome_visible` and every oracle line is written in those terms. Delete the `zen` field; keep the `zen()` accessor. Say so in the doc table at the top of the file (`:24`).

`term_panel_portion: u16` defaults to `TERM_PANEL_PORTION = 40` (`metrics.rs:38`); add `TERM_PANEL_PORTION_{MIN,MAX,STEP}` = 20/75/5 (:42-44) beside it.

- [ ] **Step 2: Grid entry/exit and zen (tests before bodies)**

Port, in order and with their comments: `enter_grid` (`layout.rs:222-252` — sets `chrome_visible = true` **first** per carried amendment 5, rebuilds `tile_order` from `Store::grid_order`, keeps the active session's tile focused if it has one else focuses the first, acknowledges, clears any drag), `exit_grid` (:257-269 — carries the focused tile into `active_session`, leaves the terminal tab, `reset_focused_pane` because the panel re-anchors to a new worktree, persists the order, clears the grid bookkeeping), `on_toggle_grid_view` (:199-216 — clears the selection, **clears `grid_view_before_zen`** because a manual toggle cancels the restore intent, leaves the terminal tab on entry), `on_toggle_zen` (:63-103 — all three branches: exiting restores the grid and re-enters it if `tile_order` was emptied while zenned; entering from the grid delegates to `tile_zen`; the empty-grid branch still drops out of grid so zen never stacks on a chrome-less grid; entering from the workspace just hides the chrome), `on_grid_tile_zen` (:344-356), `set_grid_focus` (`update/mod.rs:1061-1068` — a focus change clears the selection, which would otherwise paint and copy from the wrong session), and `reconcile_after_teardown` (`layout.rs:276-306`).

Mandatory tests (name them after the invariant, not the function):
- entering the grid from zen leaves `screen_from_flags(chrome_visible, grid_view) == Screen::Grid`, never `Zen` (the already-ported `chromeless_grid_is_not_a_nameable_screen` guard, now enforced on the transitions rather than just the classifier);
- zen entered from the grid and exited returns to the grid with the same `tile_order`, and to the **single-session workspace** when it was entered from there;
- a manual `mod+g` while `grid_view_before_zen` is set clears the intent, so a later zen-exit does not resurrect an empty grid;
- `exit_grid` carries `grid_focused` into `active_session`;
- teardown reconciliation drops dead ids, re-focuses the first tile when the focused one died, and falls back out of grid view when nothing is left.

- [ ] **Step 3: Directional move and swap**

`grid_move` (`update/mod.rs:1070-…`) and its swap sibling, over `grid::grid_neighbor`. Move: resolve the focused tile's *position* in `tile_order`, find the neighbor, focus it, leave the terminal tab, set `active_session`, acknowledge. Swap: `swap_tiles`, then record the slide (Task 1 Step 3), then persist the order — the focused **session** follows its tile, so `grid_focused` is unchanged while its position moves. Both no-op on an empty grid or a move off the edge.

- [ ] **Step 4: Terminal tab and panel transitions**

`terminal_toggle_decision` (`shortcuts.rs:528-557`) with `grid_view_before_terminal`: leaving the tab restores the grid **only** when the tab was entered from it; entering exits the grid first and remembers it. Then `on_toggle_terminal` (`update/mod.rs:472-500`) — including "first use with no terminals yet spawns one" (the spawn itself is the view's job; the transition just reports it) and **never touching `chrome_visible`** (recorded ambiguity 3).

Panel: `on_toggle_term_panel` (`sessions.rs:62-88`) — refuses to open with no active session to anchor a worktree, focuses `Panel` on open and `Agent` on close, clears the selection. `adjust_term_panel_portion` (`layout.rs:533-542`, clamped, no-op when unchanged) and `set_term_panel_portion` from `term_portion_for_cursor` (`metrics.rs:257-263`, ported verbatim with its test at `metrics.rs:482-501`). `focus_pane`/`reset_focused_pane` (`pty_input.rs:128-158`) — a `Panel` click only counts while the panel is open; a `Tile` origin is ignored (tile focus is `grid_focused`'s job).

- [ ] **Step 5: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui workspace_state 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 3: Keymap — the grid actions get bound, and the four contexts get scoped

**Files:**
- Modify: `crates/grove-gpui/src/keymap.rs`, `crates/grove-gpui/src/views/workspace.rs`

**Interfaces:**
- Produces: `GridMove`/`GridSwap` bindings, panel-resize binding, the `Grid`/`Zen`/`Terminal` key contexts on the root, and real handlers behind `ToggleGrid`/`ToggleZen`/`ToggleTerminal`/`NewHomeTerminal`.

- [ ] **Step 1: Data-carrying grid actions (carried amendment 3)**

Follow `select_session_bindings()` exactly. Generate from the registry rows at `shortcuts.rs:337-366`: h/j/k/l **and** the four arrow keys, × {move, swap}, where swap adds Alt everywhere and **also** accepts Shift on macOS (`grid_swap_mods`, :25-32). Scope every one to the `Grid` context. Then **delete the `only_the_grid_actions_remain_unbound` test's carve-out** (`keymap.rs:596-613`) and the `matches!(sc, GridMove | GridSwap) { continue }` skip in `every_actionable_row_produces_a_binding` (:575-579); both guards must now pass with no exceptions. Add the mapping test: each direction key yields the expected `(dx, dy)`, and the swap-modifier set is platform-correct.

- [ ] **Step 2: The panel-resize keys (recorded ambiguity 4)**

Ctrl+Shift+Left/Right → ±5, bound **only in the workspace context** so the panel-closed case falls through to the PTY. Port `term_panel_resize_delta`'s screen check as the context choice, and keep the registry row display-only.

- [ ] **Step 3: Contexts on the root (carried amendment 4)**

`Workspace::render` sets `key_context` from `screen_from_flags(chrome_visible, grid_view)` — but note `terminal_focused` is a *fourth* screen in the registry (`Screen::Terminal`) that `screen_from_flags` does not model; check whether the ported `screen_from_flags` and `contexts_for` already agree on that and **report the answer** rather than inventing a fifth state. Sub-contexts (the panel, a focused tile) hang off the focused element's own path, not the root.

- [ ] **Step 4: Real handlers**

Replace the `stub_action!` lines at `workspace.rs:517-525` for `ToggleGrid`, `ToggleZen`, `ToggleTerminal`, `NewHomeTerminal` with calls into Task 2's transitions (`NewSession`/`NewSessionInWorktree` stay Plan 08 stubs — they open the launcher). Add `on_action` arms for the two grid actions and the panel step. `select_visible_session`'s grid branch (`sessions.rs:396-410`: in grid view `mod+1..9` indexes `tile_order`, not the sidebar's visible order) goes in here too.

- [ ] **Step 5: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui keymap 2>&1 | tail -40
```

---

### Task 4: The grid view

**Files:**
- Create: `crates/grove-gpui/src/views/grid.rs`
- Modify: `crates/grove-gpui/src/views/{workspace,session_header,appbar,mod}.rs`

**Interfaces:**
- Produces: the tile grid, the tile header, the waiting scrim/chip/border, drag reorder + slide, and per-tile focus routing.

- [ ] **Step 1: The columns-of-tiles layout** (`terminal.rs:90-179`)

`grid_layout(n)` gives `(cols, rows)`; the render walks **columns**, and each column stacks only the tiles whose row-major index `row_idx * cols + col_idx` is `< n`. That is why a 3-session grid puts one full-height tile beside a 2-stack — reproduce it with flex (`flex_row` of `flex_col`s, each child `flex_1`), and the per-column height difference falls out for free, along with per-tile PTY dims (carried amendment 1). Inter-tile gaps are 1px of `BORDER_SOFT()` showing through the container background (`:165-173`). An empty `tile_order` renders the shared empty-workspace state.

**Do not add PTY resize code.** Assert instead: this is where amendment 2's grid parity test goes.

- [ ] **Step 2: The tile header** (recorded ambiguity 1, `terminal.rs:988-1018`)

`TILE_HEAD_H = 22px`, background `BG_HL()` when focused else `BG_STRIP()`, `BORDER_SOFT()` hairline beneath. Left: agent icon (11px) + agent label (`UI_BOLD` 10px `FG_DIM()`) + `·` + project + the branch segment, **skipped entirely when the branch is blank**. Right: respond chip, num hint, zen button, kill button (both 10px glyphs, 18×18, `FG_MUTE()`; kill turns `RED()` while armed). The zen button dispatches `tile_zen(id)`; the kill button is the existing two-step confirm. Press anywhere on the header starts a drag (Step 5).

- [ ] **Step 3: The num hint and the respond chip** (`terminal.rs:880-974`)

Num hint, tiles 0..9 only: the full chord (⌘ SVG + digit on macOS, `"{mod}+{n}"` text elsewhere) in a `BG()` chip with a `BORDER()` outline, `FG_DIM()` when focused else `FG_MUTE()`. **The modifier label comes from the registry**, never a literal.

Respond chip, only while this tile is `WaitingForInput`: alpha `1.0 - 0.35 * pulse` on `AMBER()`, background the same amber at `alpha * 0.08`, content `"respond · {chord}"` for tiles 0..9 and a bare `"respond"` beyond. TDD the label/alpha functions.

- [ ] **Step 4: The waiting border and the scrim** (`terminal.rs:1082-1155`)

Border: solid `AMBER()` 1.5px while waiting, `CYAN()` 1.5px while focused, none otherwise — **attention wins over focus**, and it does not blink.

Scrim: a full-tile overlay at `BG_STRIP()` alpha 0.92 (the theme's deepest surface — iced has no backdrop blur and gpui gets no special treatment here), centered `"N E E D S   A T T E N T I O N"` in `UI_BOLD` 20px with the letters spaced **manually** (the string literal is the spacing), pulsing alpha `0.7 + 0.3 * t` off the 40-tick triangle wave — **this is `animation_clock::toast_pulse`'s first consumer** (Plan 06 recorded ambiguity 3); consume it, do not recompute the wave. Sub-line: `"click to respond · {mod}+{n}"` for the first nine tiles, else `"click to respond"`, in `MONO_FAMILY` 10px `FG_MUTE()`. Clicking the scrim focuses/acknowledges the tile exactly like clicking its header.

- [ ] **Step 5: Drag reorder and the slide** (`layout.rs:308-342`, `terminal.rs:1032-1080,1157-1174`)

Press on a header → focus + acknowledge + arm the drag; entering any tile while armed sets the hover target (a bare enter with no armed drag is a no-op); release → if source ≠ target, `swap_tiles`, record the slide, persist `grid_order`. While dragging, the source tile takes a `BG()`-at-0.72 dim overlay and the hover target a `CYAN()` 1.5px inset with a 6%-alpha cyan wash. The slide is the paint-time offset from Task 1 Step 3 (carried amendment 6), cleared once `slide_progress >= 1.0`.

- [ ] **Step 6: Focus routing** (carried amendment 7 — the Plan 04 deferral lands here)

Reuse `Workspace`'s existing per-session `TerminalView` map; a tile hosts the same entity the single-session view would. Focusing a tile focuses that view's handle; `grid_focused` and the focused handle must never disagree — assert it. Update `TerminalView::press_focused`'s stale comment (`terminal_view.rs:52-57,268-270`). Keyboard input, scroll, selection and copy all follow gpui focus, which supersedes iced's `focused_session`/`selection_pane` branching (`terminal.rs:1180-1198`) — record that supersession in the module doc.

- [ ] **Step 7: The appbar's grid combo goes live**

Plan 06 built both shapes with the grid branch unreachable (carried amendment 7 there). Now `grid_view` is real: verify the segmented combo (`appbar.rs:46-123` — `+` magenta with left-rounded corners │ 1×14px `BORDER()` hairline │ `grid` cyan on `BG_HL()` with right-rounded corners, in a 5px-radius bordered container) renders in grid view and the lone 22×22 button outside it. The `+` stays a Plan 08 stub.

- [ ] **Step 8: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 5: Zen and the terminal tab

**Files:**
- Create: `crates/grove-gpui/src/views/terminal_tab.rs`
- Modify: `crates/grove-gpui/src/views/{workspace,appbar,session_header}.rs`

**Interfaces:**
- Produces: the chrome-hidden zen screen with its floating attention pill, the home-terminal tab and its bar, and the session bar's tool cluster.

- [ ] **Step 1: `chrome_visible` becomes real** (`view/mod.rs:66-99`)

Delete `workspace.rs:577-579`'s `let chrome_visible = true;` and read it from `WorkspaceState`. Chrome-visible: appbar / `row![sidebar, divider, body]` — **or the grid, which replaces the whole row including the sidebar** — / statusbar. Chrome-hidden (zen): the body alone, full-bleed, with the attention pill stacked over it when anything waits. The attention *dropdown* layer is gated on `chrome_visible` too (`view/mod.rs:101`).

Every piece of chrome that disappears returns its height to the terminal automatically (findings amendment 7) — **verify it visually in Task 7**; this is the first phase where chrome comes and goes at runtime.

- [ ] **Step 2: The zen attention pill** (`appbar.rs:244-305` — the Plan 06 deferral)

Top-right, 12px from the top and 12px from the right, over the terminal. A 999px-radius pill, `AMBER()` 1px border, amber at 8% background (14% hovered), containing a dot at alpha `1.0 - 0.4 * pulse` and the bare waiting **count**. It is not a dropdown — clicking jumps straight to the first waiting session (`JumpToWaitingSession`, already real since Plan 06), so there is no backdrop and nothing to dismiss.

- [ ] **Step 3: The home-terminal tab** (`terminal.rs:398-485`)

The tab body is the active home terminal's `TerminalView` (already memoized at `workspace.rs:397-408`) under a `SESSBAR_H` bar: a running/exited dot + label, a `vline`, the context title middle-truncated at 80 chars (defaulting to `~`), the literal `~` in `FG_MUTE()`, then **restart** and **zen** tool buttons. There is no kill — the terminal is permanent. `BG_STRIP()` background, `BORDER_SOFT()` hairline beneath. When there is no terminal at all, the empty-terminals state.

`NewHomeTerminal` and the restart both go through the registry's existing home-terminal surface (`session_registry.rs:288-322`); restart replaces the shell in its slot keeping the label (`src/app/terminals.rs:38-53`) and **only swaps once the replacement is live**, toasting on failure (recorded ambiguity 5).

- [ ] **Step 4: The session bar's tool cluster** (recorded ambiguity 2, `terminal.rs:592-620`)

Add to `session_header`: `run script` (Plan 08 stub, shown only when the project has a non-blank run script configured), the **term-panel toggle** (a *toggle*-styled button reflecting `term_panel_open`), **zen**, and **kill** with its two-step confirm label (`"kill"` → `"confirm kill"`). The zen button's tooltip flips to `"exit zen"` while the chrome is hidden — in zen the session bar is the only way back out by mouse.

- [ ] **Step 5: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 6: The worktree slide-over panel, its divider, and the first toasts

**Files:**
- Create: `crates/grove-gpui/src/views/term_panel.rs`
- Modify: `crates/grove-gpui/src/entities/session_registry.rs`, `crates/grove-gpui/src/views/workspace.rs`

**Interfaces:**
- Produces: per-worktree panel shells in the registry, the panel's tab strip and body, the split + draggable divider, Agent/Panel focus routing, and two real toast producers.

- [ ] **Step 1: Panel shells in the registry** (`src/app/terminals.rs:110-176`)

A third collection beside `order` and `home`: shells keyed by worktree path, with a per-path active index. Port the shape and its rules — `ensure` spawns the first shell on demand and selects something; `close` shifts the active index the way `close_home` already does; the collection may reach zero for a path. The shells are `Agent::Terminal` at the worktree root, native (not tmux — they are convenience shells, not agents that must survive a restart), so `attention::prepare` returns `None` and there is nothing to thread down (same argument as `sidebar.rs:297-301`).

TDD the index bookkeeping (add/select/close/close-active/close-last) without a PTY, exactly as Plan 05 did for the home terminals.

- [ ] **Step 2: The tab strip** (`terminal.rs:234-316,319-393`)

A `SESSBAR_H` `BG_STRIP()` strip: a horizontally scrollable row of tabs (scrollbar width 0), then a `＋` add button, then — pushed right — a `collapse-right` button that closes the whole panel. Each tab is icon-only (a running/exited dot, a `term` glyph, a `×`), because several shells share a worktree and names would not disambiguate them: active tabs get `BG_HL()` + a `CYAN()` 1px outline and a cyan glyph, inactive `FG_DIM()`, hover `BG_HOVER()`; the `×` turns `RED()` on hover. Below the strip: a `BORDER_SOFT()` hairline, then the active shell's `TerminalView`.

- [ ] **Step 3: The split and the divider** (`terminal.rs:181-229,78-87`, `layout.rs:162-197`)

When the panel is open **and** the active session has a worktree, the body becomes `row![session (100 − portion), divider, panel (portion)]` using proportional flex weights so the ratio is the single source of truth (iced's `FillPortion`). The divider is a `SIDEBAR_DIVIDER_W = 6px` grab zone around a `BORDER()` hairline, full height, with a horizontal-resize cursor.

Drag: press arms it, or — **within 350ms of the previous press** — resets the portion to the 40% default instead (the same double-click idiom the sidebar divider uses, and it must not also start a drag). Move maps the cursor to a percentage via `term_portion_for_cursor` (Task 2 Step 4). Release commits. The pointer events must be listened for at the root, not the 6px zone — reuse the pattern `sidebar::root_drag_listeners` already established (`workspace.rs:531`).

Keyboard: Ctrl+Shift+←/→ steps ±5 within 20..75 (Task 3 Step 2).

This is where amendment 2's `pty_cols_for_fraction` parity test goes.

- [ ] **Step 4: Agent/Panel focus routing** (carried amendment 8 — the second Plan 04 deferral)

Opening the panel focuses the panel's view (that is what the user just asked for); closing it, and changing the active session, re-anchor per `reset_focused_pane`. Clicking either PTY sets `focused_pane` to match. A worktree with **no** panel shell routes keystrokes to the agent rather than swallowing them (`pty_input.rs:170-178`) — test that fallback explicitly. The panel is hidden entirely in grid view (there is no single "active worktree" there); confirm that against `terminal.rs:181-186`'s ordering, where `grid_view` is checked before the panel split, and record the answer.

- [ ] **Step 5: The first toast producers** (recorded ambiguity 5)

Wire `ToastState::set_error` into the two spawn-failure paths reachable now: an agent session that fails to start (`"failed to start session: {e}"`, `sessions.rs:482`) and a shell that fails to start (`"terminal failed: {e}"`, `src/app/terminals.rs:104-108`) — the latter covers home terminals and panel shells alike. Checklist row 16's TTL behavior becomes verifiable by forcing a spawn failure; say how in the checklist.

- [ ] **Step 6: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 7: Verification and the manual parity checklist

**Files:**
- Modify: `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` (row 07 → done)

**Interfaces:**
- Produces: the phase's evidence.

- [ ] **Step 1: Full automated verification**

```bash
cd /home/gitfudge/dev/gitfudge0/grove
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 build -p grove-gpui 2>&1 | tail -5
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
GROVE_CONFIG_DIR=$(mktemp -d) GROVE_GPUI_SELFTEST=1 PATH="$HOME/.cargo/bin:$PATH" \
  cargo +1.95.0 run -p grove-gpui 2>&1 | tail -5
# the rest of the workspace, DEFAULT toolchain — must be untouched
rustc --version
cargo build 2>&1 | tail -5
cargo test 2>&1 | tail -10
git status --short src crates/grove-core crates/grove-terminal   # expect EMPTY
rustfmt --edition 2021 --check crates/grove-gpui/src/*.rs crates/grove-gpui/src/*/*.rs
```

Expected: everything green, the Plan 03 metric selftest still prints its `cell_w=7.5… OK` line, the keymap's two drift guards pass **with no grid carve-out**, and `git status` reports no changes at all under `src/`, `crates/grove-core/`, `crates/grove-terminal/`. Read the raw output.

- [ ] **Step 2: MANUAL — the spec Appendix A grid/zen/tab/panel rows (human, real desktop)**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui
# and, side by side, the installed iced build:
~/.local/bin/grove
```

Report each row pass/fail. **Do not claim any of these yourself.** Rows 1–9 are the grid/zen/terminal-tab/panel clauses of spec Appendix A → *Screens/layout*, verbatim and in order; 10–13 are the rows Plans 04 and 06 explicitly deferred to this phase.

1. **Grid ≤4×4, `cols = ceil(sqrt(n)).clamp(1,4)`.** n = 1,2,3,4,5,9,16 tile exactly as the iced build does, and a 17th session does not add a 17th tile.
2. **Short-column tiles fill height.** With 3 sessions, the lone right-hand tile spans the full workspace height — no empty cell beside it — and its PTY has roughly twice the rows of the stacked pair.
3. **Header drag-reorder with a 150ms slide.** Dragging a tile header onto another swaps them; the source dims while held, the target shows the cyan inset, and on release both tiles ease into place over ~150ms with their PTYs already correctly sized.
4. **Per-tile PTY resize on reorder.** After a swap between columns of different heights, both tiles' shells wrap at their new size immediately (run `tput lines; tput cols` in each).
5. **Order persisted by stable session key.** Reorder, quit, relaunch: the arrangement survives, and a session closed while the app was shut still leaves the rest in order.
6. **Zen chrome-hidden with grid/terminal restore bookkeeping.** Zen from the single-session workspace and back returns there; zen from a grid tile shows that one session and exiting restores the grid exactly as it was; `mod+g` while zenned cancels the restore; `mod+t` in zen swaps content **without** unhiding the chrome. The floating amber pill appears top-right in zen only while something waits, shows the count, and jumps to it on click.
7. **Terminal tab.** `mod+t` from the workspace, from the grid (which it leaves and later restores), and from zen; native PTY rooted at `~`; restart recovers an exited shell in place keeping its label; no kill button; `mod+shift+t` (or the registry's real `NewHomeTerminal` key) adds another.
8. **Worktree panel, 20–75% split.** The session bar's `term` toggle opens the right-docked panel for the active session's worktree; Ctrl+Shift+←/→ steps it 5% at a time and clamps at 20/75; dragging the divider tracks the cursor and double-clicking it resets to 40%; both PTYs rewrap at every settle. Multiple shells per worktree via `＋`, switchable and closable by tab, and the `collapse-right` button dismisses the panel from inside itself.
9. **Agent/Panel focus routing.** Opening the panel focuses the panel shell; clicking the agent PTY moves input back and clicking the panel returns it; switching sessions re-anchors the panel to the new worktree and refocuses it; a worktree whose panel has no shell routes typing to the agent rather than eating it; the focus-changing click does **not** move the caret.
10. **Grid-tile mouse routing (deferred from Plan 04).** Clicking any tile — header, body or scrim — focuses that tile, makes it the active session and clears its amber glyph; keystrokes, scroll, selection and copy all go to the focused tile only, and never to a neighbor; `mod+1..9` selects by the tile's own number hint.
11. **Tile waiting affordances (deferred from Plan 06).** A tile whose agent needs input shows the solid amber 1.5px border (winning over the focused cyan), the pulsing `respond · {mod}+{n}` chip in its header, and the full-tile "NEEDS ATTENTION" scrim pulsing on the same ~2.4s 40-tick wave as the iced build, side by side. Clicking the scrim responds to it.
12. **Tile headers (deferred from Plan 06).** Agent icon, agent label, project, branch (absent for branchless sessions — no orphan dot), the number hint chip with the registry's real modifier, and the zen/kill buttons with the two-step kill confirm.
13. **Toast with kind-dependent TTL (Plan 06 row 16, now producible).** Force a spawn failure (e.g. point a project at a directory that no longer exists, or make the shell unspawnable): the error toast appears in the statusbar and clears after 8s; a second failure replaces it immediately with a fresh full TTL.

Rows explicitly **deferred** and not checked here (record as deferred, not failed): every modal behind the `+`, the cog, the two statusbar chips and the session bar's `run script` button, plus `gpui-component` text inputs → **Plan 08**; the upgrade dot's real state, telemetry, quit paths and tmux sidecar reattach → **Plan 09**; the macOS dock badge/bounce → **Plan 10 on a macOS host**; the scripted screenshot sweep (grid n∈{1,2,3,5} × panel/zen × 3 zooms × 4 themes) and the measured idle-power comparison → **Plan 10**.

- [ ] **Step 3: `./install.sh`** — the orchestrator runs this.

```bash
./install.sh 2>&1 | tail -20
```

Expected: the release build + install of the **iced** `grove` binary still succeeds, untouched by this phase.

- [ ] **Step 4: Update the master plan and commit** — the orchestrator runs this.

Mark row 07 `done` in `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` with a one-line note recording: whether any grove-core amendment had to be authorized (expected: none), the result of amendment 2's two parity assertions (the actual cell deltas, not "passed"), whether `screen_from_flags` needed a `Screen::Terminal` answer (Task 3 Step 3), whether the panel is suppressed in grid view (Task 6 Step 4), that the keymap's grid carve-outs are deleted, and any Appendix A row that came back FAIL or MANUAL-deferred.

```bash
git add crates/grove-gpui docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md
git commit -m "feat(gpui): grid view, zen, terminal tab, worktree panel"
```

**Exit gate met when:** the Appendix A rows above are signed off by a human as pass or explicitly-deferred, the grid/slide/reconcile/portion unit tests and both layout parity assertions are green (raw output pasted), the keymap drift guards pass with **no** grid exception, grove-gpui builds/tests/clippy clean on 1.95, the iced app and both existing crates are provably untouched and still build on the default toolchain, and `./install.sh` is green.
