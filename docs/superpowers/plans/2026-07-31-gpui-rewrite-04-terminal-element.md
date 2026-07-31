# gpui Rewrite Plan 04: TerminalElement + single-session workspace + full input path

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. This is **production** code: the workspace clippy denies apply (`unwrap_used`/`expect_used`), superpowers:test-driven-development governs every pure helper (tests before implementation, red before green), and superpowers:verification-before-completion governs every "done" claim — read raw command output, never a summary line. Also load the `gpui-development` skill before writing any gpui code; training-data gpui is stale and this rev is pinned.

**Goal:** Put **one real terminal session** on screen in the gpui shell and make every input path through it work. This is the phase that burns in the core: a `TerminalSession` entity owning a `grove_terminal::GroveTerm` fed by a real PTY (tmux attach, exactly as iced Grove spawns it), a custom `TerminalElement` painting the grid (merged bg quads → shaped runs → selection overlay → blinking cursor), ANSI→theme-token color resolution at paint time, and the complete input path: the keyboard→PTY byte table, scroll with pixel accumulation and mouse-report forwarding, SGR/X10 mouse encoding, click-to-move-caret, selection in absolute scrollback-stable coordinates with clipboard copy, bracketed paste, and file drop.

Exit gate (master plan row 04): **spec §Terminal checklist rows green; keyboard byte-table test green**; `./install.sh` green; one commit.

**Out of scope — do not build it here.** Multiple sessions, the sidebar, the session registry, grid/tiles, the worktree panel, the terminal tab, per-project pinned content themes, the project-themes toggle, and Agent/Panel focus routing are Plans 05–07. This phase renders **one hardcoded session** in the body region Plan 03 left as a placeholder. Modals, text inputs and `gpui-component` stay forbidden (Plan 08 owns the durable-pin decision).

**Architecture (new/changed files only):**

```
crates/grove-gpui/
  Cargo.toml               + grove-terminal (path), + arboard (clipboard parity)
  src/entities/
    terminal_session.rs    NEW. TerminalSession entity: PtyHandle + GroveTerm + the
                           reader task (damage-gated cx.notify), send()/scroll()/click()/
                           resize(), selection state, tmux copy-mode bookkeeping.
                           This is grove-gpui's OWN session type — see Constraint 3.
  src/terminal/
    mod.rs                 NEW.
    colors.rs              NEW. TermColor + ANSI index -> theme token resolution
                           (port of src/gui/pty.rs:382-421), inverse-swap semantics.
    keys.rs                NEW. gpui Keystroke -> PTY bytes (port of src/gui/keys.rs)
                           + the copy/paste/scroll-chord predicates.
    mouse.rs               NEW. cell_at, scroll accumulation, SGR/X10 encode,
                           selection normalization/geometry.
    drop.rs                NEW. wl-paste URI-list fallback + shell-escaped drop text
                           (port of src/gui/drop.rs).
  src/terminal_element.rs  NEW. the custom Element (spec §2 layout puts it at crate root).
  src/views/
    terminal_view.rs       NEW. focusable div wrapper: key/scroll/mouse handlers +
                           key_context "Terminal", hosting TerminalElement.
    workspace.rs           MODIFIED: body placeholder -> TerminalView.
```

**Tech stack additions:** `grove-terminal` (path dep, landed in Plan 02), `arboard` (already a workspace dependency of the iced app — reuse the workspace entry, same version). No new git dependencies. Pins unchanged.

## Global Constraints

- Branch: `gpui-rewrite`. Toolchain regime is **identical to Plan 03** and is not re-litigated:
  - grove-gpui builds/tests/clippy only via `PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 -p grove-gpui`.
  - Bare `cargo build` / `cargo test` (default-members, rustc 1.94.1) must keep working untouched for `grove`, `grove-core`, `grove-terminal`. Never run `--workspace`.
  - clippy for grove-gpui runs **`--no-deps`** (Plan 03 carry-forward: clippy 1.95 raises 9 new lints in grove-core, which is off-limits until Plan 10):
    `cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings`
  - `rustfmt --edition 2021` on **touched files only**.
- **Pins are law.** ZED_REV `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba`; alacritty fork `4c129667ce56611becdc82de6e28218c80e2e88f`. No `[patch]`, no `gpui-component`.
- **Constraint 3 — grove-core and the iced app are read-only.** No edits under `src/`, `crates/grove-core/`, or `crates/grove-terminal/` in this phase.
  - **Recorded contradiction, resolved here:** spec §3 says the parser API is "the trait grove-core's `Session` swaps its parser behind", but Plans 02 and 03 both forbid touching `crates/grove-core/src/session.rs`, and the master plan's standing rule keeps vt100 in-tree as the golden oracle until Plan 10. **Resolution for this phase:** grove-gpui defines its **own** `TerminalSession` entity over `grove_terminal::{GroveTerm, spawn}`. `grove_core::session::Session` (vt100-backed) is untouched and unused by grove-gpui. The trait-swap in grove-core, if it ever happens, is a Plan 10 concern when iced is deleted. Do not edit grove-core to "share" the type; if you feel the urge, STOP and report.
  - What grove-gpui **does** reuse from grove-core, because it is genuinely UI-free and must not be duplicated: `grove_core::tmux::{SOCKET, make_name, next_free_n, new_session, configure_embedded_session, scroll, cancel_copy_mode, kill_session, pane_pid, has_session, available}`, `grove_core::session_meta::write`, `grove_core::agent::Agent::{program, invocation}`, `grove_core::attention::prepare`, `grove_core::env_path::login_shell`. Read `crates/grove-core/src/session.rs:177-236` (`spawn_tmux`) and `:349-384` (`attach_tmux`) and reproduce the **command construction** — `tmux -L <SOCKET> -u attach-session -t =<name>`, `TERM=xterm-256color`, `LC_ALL=en_US.UTF-8` — verbatim in behavior, calling those helpers rather than re-implementing them. The `-u` flag is load-bearing (session.rs:362-367 explains why: without it tmux downgrades box-drawing to ACS).
- **Behavior questions are answered by reading the iced code, never by guessing.** Canonical oracles for this phase, cited per task:
  - `src/gui/keys.rs:5-45` — the key→bytes table (this plan's exit-gate test ports it row for row).
  - `src/gui/pty.rs:18-82` (run building + inverse), `:216-330` (paint order), `:332-372` (selection overlay geometry + the rgba), `:374-421` (`normalize_selection`, `vt_color_opt`, `ansi_idx`).
  - `src/gui/pty.rs:110-201` — scroll accumulation and the mouse/drag state machine.
  - `src/gui/update/pty_input.rs:10-294` — press/drag/up semantics, `pty_press_focused`, `pixel_to_abs`, `pty_view_geom`, drag autoscroll, `keyboard_scroll_intent`, `is_copy_shortcut`/`is_paste_shortcut`, `escape_should_dismiss`.
  - `src/gui/update/mod.rs:806-843` — copy/paste handling, bracketed paste, the wl-paste file-URI fallback.
  - `src/gui/update/sessions.rs:321-346` — file-drop behavior.
  - `crates/grove-core/src/session.rs:595-794` (`snap_to_bottom`, `send`, `scroll`, `scroll_view`, `scroll_lines`, `scroll_page_lines`, `click`), `:940-967` (`resize`), `:990-1060` (`scroll_notch_count`, `encode_mouse`, `arrow_moves`), `:53-58` (`INIT_ROWS`/`INIT_COLS`/`SCROLL_STEP`/`SCROLLBACK_LINES`).
  - `src/gui/metrics.rs:24-32,46-49` — cell metrics and zoom bounds.
- **Carried amendments (do not re-derive):**
  1. `Element` trait shape at this rev (findings §S1 Step 3) — `RequestLayoutState = ()`, `PrepaintState`, and the two extra params (`Option<&GlobalElementId>`, `Option<&InspectorElementId>`) on `request_layout`/`prepaint`/`paint`, plus `id()` and `source_location()`. `request_layout` uses `Style::default()` with `size.{width,height} = relative(1.).into()` and `window.request_layout(style, [], cx)`.
  2. Paint APIs, verbatim (findings §S1 Step 1): `ShapedLine::paint(&self, origin: Point<Pixels>, line_height: Pixels, align: TextAlign, align_width: Option<Pixels>, window: &mut Window, cx: &mut App) -> Result<()>`; `gpui::fill(bounds, background) -> PaintQuad` + `Window::paint_quad(quad)`; `TextSystem::shape_line(text: SharedString, font_size: Pixels, runs: &[TextRun], force_width: Option<Pixels>) -> ShapedLine`. **All shaping happens in `prepaint`** (it needs `&mut Window`); `paint` only emits quads and paints already-shaped lines.
  3. **Per-run anchoring is mandatory** (findings §S1 Step 1 CJK finding): every run is painted at `origin.x + col * cell_w`, never by concatenating advances, so a width mismatch inside one run cannot drift the next. CJK shapes to 1.333 cells out of the system fallback face instead of 2 — see Task 3 Step 5 for the `force_width` attempt and its guarded fallback.
  4. Damage-gated repaint (findings §S1 Step 5), verbatim shape:
     ```rust
     proc.advance(&mut *t, &chunk);
     let dirty = match t.damage() { TermDamage::Full => true,
                                    TermDamage::Partial(mut it) => it.next().is_some() };
     t.reset_damage();
     if dirty { this.update(cx, |_this, cx| cx.notify())?; }
     ```
     …except that in this phase `process()`/damage live **inside `GroveTerm`** (Plan 02 Task 4 Step 2 already bumps a damage generation); the entity compares the generation and calls `cx.notify()` only when it moved. No polling timer on the data path.
  5. Cursor blink is **not** a private 533ms timer here. Plan 03 landed `AnimationClock` with `cursor_visible(tick) = tick % 16 < 8`; the terminal view observes the clock and reads that accessor. The findings' 533ms figure describes the spike's standalone loop — the *formula* is the parity contract (Plan 03 Task 5 Step 3). Do not add a second timer.
  6. `ScrollDelta` has exactly two variants at this rev: `Pixels(Point<Pixels>)` and `Lines(Point<f32>)`. Pixel accumulation threshold is **`CELL_H * zoom`**, i.e. `ZoomState::cell_h()` — not the bare 17.0 from the spike (the spike had no zoom).
  7. PTY dims come from the element's own post-layout bounds: `cols = (bounds.size.width / cell_w).floor().max(1.0)`, `rows = (bounds.size.height / cell_h).floor().max(1.0)`. **`compute_pty_dims`'s chrome-subtraction arithmetic is superseded** (Plan 03 Task 6 Step 1) — do not port `src/gui/metrics.rs:265-295`.
  8. Zoom is the single `window.set_rem_size(px(16.0 * zoom))` call in `Workspace::render` plus Rust-side `cell_w()`/`cell_h()`/`font_size()` multiplication in the element. **`WithRemSize` does not exist at this rev**; the spec §6 "two rem scopes" sentence is superseded by Plan 03's amendment 4. Recorded contradiction, already resolved — do not reintroduce it.
- **Parity beats the spike where they disagree.** Two spike behaviors in findings §S1 Step 4 are *not* what iced Grove does, and iced wins:
  - **Modified arrows.** The spike emits the CSI-modifier form `\x1b[1;{1+shift+2*alt+4*ctrl}{A-D}`. `src/gui/keys.rs` does **not**: it emits the plain `\x1b[A..D`, with an ESC prefix when Alt is held, and ignores Shift/Ctrl on arrows entirely. Port keys.rs. A comment must say the spike's richer form was rejected for parity.
  - **App-cursor (DECCKM) key encoding.** keys.rs is DECCKM-*unaware* for keypresses — arrows are always CSI. DECCKM only affects the **click-to-move-caret** synthesis (`session.rs:1046` `arrow_moves`, SS3 `\x1bO{C,D}`). The byte-table test therefore covers app-cursor variants **on the `arrow_moves` path only**, and asserts explicitly that a plain Up keypress is `\x1b[A` in *both* cursor modes. Do not "fix" this.
  - **`ScrollDelta::Lines` with `|y| < 1.0` is swallowed** (`src/gui/pty.rs:167-171`), not passed through as the spike did.
- No `git` commands until Task 6. Do not commit intermediate tasks. The orchestrator runs `./install.sh` and the commit.

---

### Task 1: `TerminalSession` entity — PTY, GroveTerm, damage-gated repaint, resize

**Files:**
- Create: `crates/grove-gpui/src/entities/terminal_session.rs`
- Modify: `crates/grove-gpui/src/entities/mod.rs`, `crates/grove-gpui/Cargo.toml`, `crates/grove-gpui/src/zoom.rs` (PTY-dims helper only)

**Interfaces:**
- Produces: `TerminalSession` — a gpui `Entity` owning `PtyHandle` + `GroveTerm`, exposing `send(&[u8])`, `resize(rows, cols)`, `snapshot()`, `cursor()`, `display_offset()`, `scroll(up, col, row)`, `scroll_lines(up, lines)`, `click(col, row)`, `selection_text_abs(..)`, `mouse_mode()`/`encoding()`/`app_cursor()`. Tasks 3–5 consume it; Plan 05 replaces the hardcoded spawn with a registry.

- [ ] **Step 1: Read the two oracles side by side before writing anything**

`crates/grove-core/src/session.rs:149-236,349-394` (spawn/attach/launch_pty) and `crates/grove-terminal/src/pty.rs` + `src/term.rs` (what Plan 02 actually shipped). **Read grove-terminal's real API first** — this plan describes it from Plan 02's Interfaces sections, and the shipped signatures are the authority. In particular confirm: whether `Cell` carries an `inverse` field or whether `GroveTerm::snapshot()` already applied the inverse swap (Plan 02 Task 3 Step 2 specified a *shared swap helper applied inside the dump*, so the swap is likely already baked into the emitted `fg`/`bg`). Record the answer in your report — Task 2 Step 3 branches on it.

- [ ] **Step 2: Add the dependencies**

`crates/grove-gpui/Cargo.toml`: `grove-terminal = { path = "../grove-terminal" }` and `arboard.workspace = true` (reuse the existing workspace entry the iced app uses; do **not** introduce a second version). Rebuild to confirm nothing in the pin graph moved:

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 build -p grove-gpui 2>&1 | tail -5
git diff --stat Cargo.lock
```

Expected: `Finished`, and `Cargo.lock` changes only by adding grove-terminal/arboard edges. If any pinned git rev moved, STOP and report.

- [ ] **Step 3: TDD the PTY-dims helper (pure)**

On `ZoomState` (Plan 03 already records the formula as a doc comment on `cell_w()`):

```rust
/// Logical PTY grid for an element laid out at `bounds` — findings amendment 7.
/// `compute_pty_dims`'s chrome subtraction is superseded by gpui layout.
pub fn pty_dims(&self, width_px: f32, height_px: f32) -> (u16, u16);  // (rows, cols)
```

Tests first: 1280×800 at zoom 1.0 → (47, 170); a zero/NaN/negative bound clamps to (1, 1); zoom 2.0 halves both counts (floor, not round); zoom 0.6 at a bound that lands on a fractional cell still floors; results never exceed `u16::MAX`. Red, then implement `(px / cell).floor().max(1.0) as u16`.

- [ ] **Step 4: The entity**

```rust
pub struct TerminalSession {
    term: grove_terminal::GroveTerm,
    pty: grove_terminal::PtyHandle,
    backend: Backend,                  // Tmux { name: String } | Native
    rows: u16, cols: u16,
    last_damage_gen: u64,
    tmux_copy_mode: bool,              // session.rs:617-622 bookkeeping
    last_input_at: Option<Instant>,    // session.rs:604-605 (attention plumbing is Plan 06)
    last_scroll_at: Option<Instant>,
    _reader: gpui::Task<()>,
}
```

Spawn is `TerminalSession::spawn_tmux(cwd, cx)` for this phase: `grove_core::tmux::available()` → `next_free_n`/`make_name`/`new_session` → `session_meta::write` → `configure_embedded_session` → `grove_terminal::pty::spawn(CommandBuilder::new("tmux") …)` with the argument list and env copied from `session.rs:359-373`. Honour `GROVE_GPUI_SESSION_CWD` (default: the repo root / `$PWD`) so the manual checklist can point it anywhere. If tmux is unavailable, fall back to a native `login_shell()` PTY and log it — the element must still render (this is the escape hatch that keeps the visual checklist runnable on a box without tmux).

`unwrap_used`/`expect_used` are denied: every lock/spawn failure is `let Ok(..) = .. else { .. }` or `?` into a `tracing::error!`, never a panic.

- [ ] **Step 5: The reader task and damage-gated notify**

Plan 02's `PtyHandle` hands back a `std::sync::mpsc::Receiver<Vec<u8>>` (it deliberately picks no executor). Bridge it: a `cx.background_executor().spawn` blocking-recv loop forwarding into a `futures::channel::mpsc::unbounded`, consumed by a foreground `cx.spawn` loop that calls `term.process(&chunk)` and then:

```rust
let gen = self.term.damage_generation();
if gen != self.last_damage_gen { self.last_damage_gen = gen; cx.notify(); }
```

Keep the `Task` alive in the entity (`_reader`). **No timer on this path** — a silent PTY must cost zero wakeups (findings §S1 Step 5 measured 0.00% idle with blink off). If the bridge shape is impossible with the shipped `PtyHandle`, report it rather than adding a poll.

- [ ] **Step 6: `send` and `resize`, ported semantically**

`send(&mut self, bytes)` reproduces `session.rs:604-625` exactly and in order: stamp `last_input_at`; **snap scrollback to 0** (typing returns to the live screen); if `tmux_copy_mode`, `tmux::cancel_copy_mode(name)` **once** and clear the flag; then write + flush. Getting the order wrong makes keystrokes get eaten as copy-mode commands.

`resize(rows, cols)` reproduces `session.rs:940-967`: clamp ≥1, no-op when unchanged, snap scrollback to 0 first, then `GroveTerm::resize` **and** `PtyHandle::resize` (the OS-level `TIOCSWINSZ`) — both, or the inner app never learns.

- [ ] **Step 7: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -20
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 2: ANSI → theme-token color resolution (TDD, pure)

**Files:**
- Create: `crates/grove-gpui/src/terminal/mod.rs`, `crates/grove-gpui/src/terminal/colors.rs`

**Interfaces:**
- Produces: `resolve(c: TermColor, theme: &grove_core::theme::Theme) -> Option<Hsla>` and `resolve_pair(fg, bg, inverse, theme) -> (Hsla, Option<Hsla>)` — the only place ANSI indices become colors. `None` background means "paint no quad"; `None` foreground means "use the default fg token".

- [ ] **Step 1: Tests first — the ANSI table, row for row**

Port `src/gui/pty.rs:390-421` as a table test against Plan 03's `theme.rs` tokens, citing the source line beside each row:

| index | token | line |
|---|---|---|
| 0 | `BG_STRIP` | pty.rs:392 |
| 1, 9 | `RED` | :393 |
| 2, 10 | `GREEN` | :394 |
| 3, 11 | `YELLOW` | :395 |
| 4, 12 | `BLUE` | :396 |
| 5, 13 | `MAGENTA` | :397 |
| 6, 14 | `CYAN` | :398 |
| 7, 15 | `FG` | :399 |
| 8 | `FG_MUTE` | :400 |
| 16..=231 | cube: `n=i-16; r=n/36; g=(n%36)/6; b=n%6; v(x)= if x==0 {0} else {55+40x}` | :401-415 |
| 232..=255 | gray: `v = 8 + 10*(i-232)` | :416-419 |

Assert exhaustively over **all 256 indices** (a loop, not spot checks), including the boundaries 16/231/232/255, and that bright variants map to the *same* token as their base (`1` and `9` are equal). Also: `TermColor::Rgb(r,g,b)` bypasses the theme entirely; `TermColor::Default` → `None`.

- [ ] **Step 2: Implement**

`resolve` is a line-for-line port. Cube/gray values are computed in `u8` then converted through `gpui::Rgba` → `Hsla`, matching Plan 03's `ic()` conversion path (no HSL-space arithmetic).

- [ ] **Step 3: Inverse-swap semantics**

`src/gui/pty.rs:44-52` is the contract: swap fg/bg, and **after** the swap, a `None` fg becomes `bg_of(theme)` and a `None` bg becomes `fg_of(theme)`. This "theme-default fill" is what makes an inverse-video cell readable instead of transparent.

Where the swap lives depends on Task 1 Step 1's finding:
- If `GroveTerm::snapshot()` already swapped (likely — Plan 02 baked it into the shared dump helper), then `resolve_pair` must **not** swap again; it only applies the default-fill rule, and the `inverse` argument is whatever the cell type still exposes (possibly nothing). Write the test that proves no double-swap: a cell that was inverse must render fg=bg-token, bg=fg-token, **not** back to normal.
- If `Cell` carries `inverse` unswapped, `resolve_pair` does both.

Either way there must be exactly **one** swap in the pipeline, and a test that pins it.

- [ ] **Step 4: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui terminal::colors 2>&1 | tail -20
```

---

### Task 3: `TerminalElement` — layout, prepaint, paint

**Files:**
- Create: `crates/grove-gpui/src/terminal_element.rs`
- Modify: `crates/grove-gpui/src/main.rs` (module wiring)

**Interfaces:**
- Produces: `TerminalElement { session: Entity<TerminalSession>, selection: Option<(AbsCell, AbsCell)>, cursor_visible: bool, zoom: f32 }` implementing `Element` + `IntoElement`. Paints the grid; owns no input.

- [ ] **Step 1: The trait skeleton** (shape verbatim from carried amendment 1)

```rust
pub struct PrepaintState {
    bg_quads: Vec<PaintQuad>,
    runs: Vec<(Point<Pixels>, ShapedLine)>,   // origin already anchored at col*cell_w
    selection_quads: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
    dims: (u16, u16),
}
```

`request_layout` returns `relative(1.)` in both axes. `prepaint` receives `bounds` and does **all** the work; `paint` only replays quads and calls `ShapedLine::paint` (which needs `line_height` = `px(cell_h)`, `TextAlign::Left`, `align_width: None`).

- [ ] **Step 2: Resize on the way through prepaint**

In `prepaint`, compute `(rows, cols) = zoom.pty_dims(bounds.size.width.into(), bounds.size.height.into())` and, when it differs from the session's current dims, call `TerminalSession::resize`. This is the single place PTY dims are decided — a zoom change and a window resize both land here, so no separate zoom→resize wiring is needed. Guard against re-entrancy: resize, then read the snapshot (never the reverse), so the painted frame matches the new dims.

- [ ] **Step 3: Paint order** (findings §S1 Step 3, and `src/gui/pty.rs:216-330`)

1. One full-bounds quad in `BG()` (or the default bg token).
2. **Merged bg quads**: walk each row coalescing adjacent cells with an equal `Option<Hsla>` background into one `fill`. Cells whose background resolves to `None` emit **no quad**.
3. **Text runs**: coalesce adjacent non-blank cells with equal `(fg, bold)` into one `shape_line`, anchored at `origin.x + col * cell_w`, `origin.y + row * cell_h`. Blank cells are skipped entirely (a mostly-empty screen costs almost nothing). Bold is `gpui::font(MONO_FAMILY).bold()` — same family, weight-selected, no second family name.
4. **Selection overlay** — Task 5 fills this in; leave the field and paint it here.
5. **Cursor** — `fill(Bounds::new(point(col*cell_w, row*cell_h), size(cell_w, cell_h)), FG())`, gated on `cursor_visible` **and** the terminal's not-hidden flag, positioned via `GroveTerm::cursor()` (Plan 02 already folds `display_offset` in, so it stays put when scrolled back).

- [ ] **Step 4: Colors and the theme read**

Every cell goes through Task 2's `resolve*`. The theme is read fresh per paint through grove-core's `theme::with_current` snapshot (Plan 03 Task 3 Step 1: an atomic-load generation, not a lock), so a theme swap takes effect next frame with no invalidation bookkeeping. **Per-project pinned content themes are Plan 05** (they need the project tree); leave a `// Plan 05: project theme override` marker at the exact call site where the `&Theme` is chosen, and a comment that app chrome stays on the global theme regardless.

- [ ] **Step 5: CJK — per-run anchoring plus a guarded `force_width` attempt**

Per-run anchoring (Step 3) is the mitigation already proven in the spike and is **non-negotiable**. On top of it, attempt the width fix behind one small helper:

```rust
/// Wide chars fall back to a system CJK face at ~1.33 cells instead of 2
/// (findings §S1 Step 1). `shape_line`'s `force_width` is the obvious fix but
/// was untested at spike time. Returns None when the run needs no forcing.
fn forced_width(run_text: &str, cell_w: f32) -> Option<Pixels>;
```

Pass it as `shape_line(.., force_width)` **only** for runs containing non-ASCII wide characters; ASCII runs pass `None` so the fast path is untouched. Unit-test `forced_width` itself (ASCII → `None`; a run with one wide char → `Some(n_cells * cell_w)` using the terminal's own width accounting, not `str::chars().count()`).

**If `force_width` does not exist on `shape_line` at the pinned rev, or forcing visibly distorts the glyph:** delete the call, keep per-run anchoring alone, and record the outcome in a doc comment on the helper *and* in your report. Both outcomes are acceptable; silently guessing is not.

- [ ] **Step 6: Verify it compiles and the pure helpers are green**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -20
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

Paint correctness is not unit-testable here — it is verified by Task 6's visual checklist. Do not claim it green from a compile.

---

### Task 4: The keyboard byte table (the named exit gate) and key routing

**Files:**
- Create: `crates/grove-gpui/src/terminal/keys.rs`
- Create: `crates/grove-gpui/src/views/terminal_view.rs` (key handling half)

**Interfaces:**
- Produces: `key_to_bytes(keystroke: &gpui::Keystroke, app_cursor: bool) -> Option<Vec<u8>>`, the copy/paste/scroll-chord predicates, and the `on_key_down` handler. **This task's test is the master-plan exit gate "keyboard byte-table test green".**

- [ ] **Step 1: Write the table test FIRST, row for row, with citations**

A headless `#[cfg(test)]` module in `keys.rs`. gpui gives a *named* key string plus a separate `key_char` (findings §S1 Step 4), so the table is keyed on `Keystroke { key, key_char, modifiers }`. Every row cites its `src/gui/keys.rs` line:

| gpui keystroke | expected bytes | oracle |
|---|---|---|
| `enter` | `\r` | keys.rs:25 |
| `tab` | `\t` | :26 |
| `backspace` | `\x7f` | :27 |
| `escape` | `\x1b` | :28 |
| `space` | `' '` | :29 |
| `up`/`down`/`right`/`left` | `\x1b[A` / `\x1b[B` / `\x1b[C` / `\x1b[D` | :30-33 |
| `home` / `end` | `\x1b[H` / `\x1b[F` | :34-35 |
| `pageup` / `pagedown` | `\x1b[5~` / `\x1b[6~` | :36-37 |
| `delete` / `insert` | `\x1b[3~` / `\x1b[2~` | :38-39 |
| any other named key | `None` | :40 |
| plain char `a`, `Z` | its UTF-8 bytes | :21 |
| `ctrl-a` … `ctrl-z` | `1` … `26` (fold to uppercase, `-0x40`, `& 0x1f`) | :16-19 |
| `ctrl-c` (and `ctrl-C`) | `\x03` | keys.rs:69-74 |
| `ctrl-space` | `\x00` | :18-19 arithmetic over `' '`… see Step 2 |
| `ctrl-é` (non-ASCII) | `None` | :14-17, test :85 |
| `alt-<anything>` | ESC (`0x1b`) prefixed to the unmodified sequence | :7-9, test :96 |
| `alt-escape` | `\x1b\x1b` (the spec's "Alt+Escape reaches the PTY as ESC ESC") | :7-9 + :28 |

Plus the **app-cursor rows**, which belong to `arrow_moves` (`session.rs:1046-1060`), not to keypresses:

| call | expected |
|---|---|
| `arrow_moves(cur=5, target=8, app_cursor=false)` | `\x1b[C` ×3 |
| `arrow_moves(cur=8, target=5, app_cursor=true)` | `\x1bOD` ×3 |
| `arrow_moves(cur=4, target=4, _)` | empty |
| `key_to_bytes(up, app_cursor=true)` | **`\x1b[A`** — DECCKM must NOT change keypress encoding |

The last row is load-bearing: it pins the parity decision from Global Constraints against the spike's richer form. Add a matching negative row asserting `shift-up` and `ctrl-up` are **also** `\x1b[A` (no CSI-modifier form).

Run red:

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui terminal::keys 2>&1 | tail -30
```

- [ ] **Step 2: Implement `key_to_bytes`**

Port `src/gui/keys.rs:5-45`. Two gpui-shape adaptations, each commented:
- iced's `Key::Character(s)` ≈ gpui's `key` when it is a single printable char; the fallback for anything else typed is `key_char`. Prefer `key` for the Ctrl arithmetic (so a Linux Ctrl+V, whose `key_char` is the control char `\u{16}`, folds correctly — the same hazard `src/gui/update/mod.rs:804-806` documents on the iced side) and `key_char` for plain text emission.
- `modifiers.platform` (Super on Linux, Cmd on macOS) is **dropped**: app chords never reach the PTY (findings §S1 Step 4). Return `None` when it is set.

`arrow_moves` is ported into the same module (it is PTY-byte synthesis, not session state) with its "horizontal only — Up/Down mean history recall at a shell prompt" comment intact.

- [ ] **Step 3: The chord predicates**

Port with their tests: `is_copy_shortcut` / `is_paste_shortcut` (`pty_input.rs:427-449` — macOS Cmd+C/V without Ctrl, elsewhere Ctrl+Shift+C/V; plain Ctrl+V is deliberately left to the PTY) and `keyboard_scroll_intent` (`pty_input.rs:400-411` — Shift+PageUp/PageDown → `Page`, Shift+Home/End → `All`, and `None` when Ctrl/Alt/Super is also held or Shift is absent). Port the existing iced tests for these verbatim in intent (`pty_input.rs:512-590`).

- [ ] **Step 4: Wire `on_key_down` in `terminal_view.rs`**

Handler shape from findings §S1 Step 4: a `div().track_focus(&self.focus).key_context("Terminal")` with `.on_key_down(cx.listener(..))`. Dispatch order — **this order is the observable contract**, mirroring `src/gui/update/mod.rs:780-880`:

1. Any key press **kills the selection and any in-progress drag** (spec: "keypress kills selection+drag").
2. Copy shortcut → `arboard` copy of `selection_text()`; consume.
3. Paste shortcut → the wl-paste file-URI path first (Task 5 Step 5), else bracketed paste (Task 5 Step 4); consume.
4. `keyboard_scroll_intent` → `scroll_lines`; consume.
5. Otherwise `key_to_bytes` → `session.send(bytes)`.

Actions declared in Plan 03's keymap take precedence automatically via gpui's key-context dispatch — that is the whole point of replacing iced's `should_forward` carve-outs, so **do not port `should_forward`, `MODAL_OPEN` or `PALETTE_OPEN`**. Escape's carve-out and the two-step confirm-kill arming are modal/registry behavior owned by Plan 08; leave a `// Plan 08` marker where `escape_should_dismiss` would slot in. In this phase Escape simply reaches the PTY, which is `escape_should_dismiss`'s documented `false` branch and therefore correct.

- [ ] **Step 5: Green**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -30
```

Paste the raw byte-table test lines into your report. **This is the named exit gate; a summary line is not acceptable evidence.**

---

### Task 5: Scroll, mouse, selection, clipboard, drop

**Files:**
- Create: `crates/grove-gpui/src/terminal/mouse.rs`, `crates/grove-gpui/src/terminal/drop.rs`
- Modify: `crates/grove-gpui/src/views/terminal_view.rs`

**Interfaces:**
- Produces: the scroll accumulator, `cell_at`, `pixel_to_abs`, `normalize_selection`, selection-overlay geometry, `encode_mouse`, drag autoscroll, clipboard copy/paste, and file drop.

- [ ] **Step 1: TDD the scroll accumulator** (`src/gui/pty.rs:164-197`)

```rust
pub struct ScrollAccum { accum: f32 }
impl ScrollAccum {
    /// Returns the number of notches to emit and their direction.
    pub fn feed_pixels(&mut self, dy: f32, cell_h: f32) -> Option<(bool, usize)>;
    pub fn feed_lines(&mut self, dy: f32) -> Option<bool>;   // resets accum; None when |dy| < 1.0
}
```

Tests before implementation, each citing the oracle line:
- Sub-threshold pixel deltas accumulate and emit nothing (`:185-187`).
- Crossing the threshold emits one notch and **subtracts** `cell_h` with `copysign`, keeping the remainder (`:188-189`) — feeding `cell_h * 2.5` in one event must not silently drop 1.5 cells of travel.
- A **direction reversal resets the accumulator to 0** before adding (`:180-183`).
- `feed_lines` resets `accum` to 0 (so a device switch cannot leak a partial line) and returns `None` when `|y| < 1.0` (`:167-171`) — the spike's pass-through is rejected.
- The threshold is `cell_h`, i.e. zoom-scaled: at zoom 2.0 a gesture produces half as many notches.

- [ ] **Step 2: Scroll routing** (`session.rs:641-661,720-738`)

`on_scroll_wheel` → accumulator → per notch, `TerminalSession::scroll(up, col, row)` where `(col,row)` is the cell under the pointer (`cell_at` = position ÷ (cell_w, cell_h), clamped). `scroll` reproduces the dispatch exactly:
- `mouse_mode() == None` → scroll the **view**: for a tmux backend, `tmux::scroll(name, up, SCROLL_STEP)` and set `tmux_copy_mode = true` when scrolling up (the agent is on the alt screen, so grove's own scrollback is empty); for native, `GroveTerm::scroll_to` clamped to `SCROLLBACK_LINES` (5000).
- otherwise → forward wheel notches to the inner app: `cb = 64` (up) / `65` (down) through `encode_mouse`.

`scroll_lines` (the keyboard chords) mirrors it, sending at the viewport center `(cols/2, rows/2)` with `scroll_notch_count(lines) = lines.div_ceil(SCROLL_STEP).min(200)` — port the **200-notch flood cap** and unit-test it (`session.rs:995-997`). `scroll_page_lines()` is `rows - 1`, falling back to 20 (`:743-749`).

- [ ] **Step 3: Mouse encoding, press/drag/release, click-to-caret**

Port `encode_mouse` (`session.rs:1003-1039`) with its tests: SGR `\x1b[<{cb};{col+1};{row+1}{M|m}`; X10/UTF-8 as a 6-byte packet with `32+v` encoding, release button code 3, and **empty output past coordinate 223** (the limit is a parity behavior, not a bug). Drive it off grove-terminal's `MouseMode`/`MouseEncoding` enums (Plan 02 Task 6 Step 2) — do not reach for vt100's.

Press/drag/release semantics from `pty_input.rs:10-126`:
- Press: clear any selection, anchor `(cell, cell)` in **absolute** coords, start a drag recording `last_x/last_y` and the view height in px.
- Drag: extend the head to `pixel_to_abs(x, y)`.
- Release: if head == anchor (no drag), clear the selection and treat it as click-to-move-caret — **but only when scrollback is 0** (`pty_input.rs:113-121`; clicking history must be inert). `TerminalSession::click` then reproduces `session.rs:758-794`: clamp col/row; if mouse reporting is on, send press+release at that cell; else bail when the cursor is hidden, bail when the click row ≠ the caret's row (Up/Down would recall history), and otherwise synthesize `arrow_moves` with the DECCKM-correct prefix.
- The **focus-changing click doesn't move the caret** rule (`pty_press_focused`, `pty_input.rs:36-39,104-108`): with one session there is only the window-focus transition to guard, so implement it as "a press that gave this element focus swallows its own release". Keep the flag and the comment — Plan 05/07 make it load-bearing again.

- [ ] **Step 4: Selection — absolute coords, overlay, extraction, autoscroll**

`AbsCell { a_row, col }` where larger `a_row` is older (`pty_input.rs:244-256`): `a_row = scrollback + (h - 1 - viewport_row)`. That is what makes a selection survive scrolling. `pixel_to_abs` clamps the row into the visible window and returns `None` for a zero-height grid.

`normalize_selection` is the `(row, col)` tuple compare with swap (`pty.rs:374-380`). Overlay geometry is `pty.rs:332-372`: single-row rect (min width 1 cell), else first-row-to-EOL + full middle block + last-row-from-BOL, in `rgba(0.40, 0.50, 0.78, 0.35)` — **that exact constant**, hardcoded, not a theme token (Appendix A pins it). Unit-test the three shapes' rect lists.

Extraction is `GroveTerm::selection_text` (Plan 02 Task 6 Step 1), which already carries `clean_selection`'s trailing-whitespace/blank-line trimming — verify that in Plan 02's tests rather than re-implementing it; if it does not, report before writing a second copy.

Drag autoscroll (`pty_input.rs:261-285`) hangs off the **AnimationClock** tick, not a new timer: while a drag is held and the pointer is within one cell of the top/bottom edge, scroll one step and extend the head over the revealed line — only if the scroll actually moved the view.

- [ ] **Step 5: Clipboard and file drop**

Copy: `arboard` (`src/gui/update/mod.rs:810-812`). OSC52 is part of the same clipboard story on the iced side — check `src/clipboard.rs` and port whatever it does, or record its absence.

Paste (`mod.rs:815-842`), in order: try `drop::clipboard_paths()` (the `wl-paste --no-newline --type text/uri-list` fallback for Wayland's missing DnD) and, if non-empty, type each path as a drop would; otherwise bracketed paste — normalize `\r\n` → `\r` and `\n` → `\r`, wrap in `\x1b[200~` … `\x1b[201~`. Either way, clear the selection.

Port `src/gui/drop.rs` wholesale into `terminal/drop.rs`: `parse_uri_list`, `percent_decode`, `shell_escape`, `dropped_path_text` (path + **one trailing space**), with its tests. File drop with no modal open types the escaped path into the focused session and clears the selection (`sessions.rs:336-341`); the modal-aware branches are Plan 08.

- [ ] **Step 6: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 6: Workspace wiring, verification, and the manual parity checklist

**Files:**
- Modify: `crates/grove-gpui/src/views/workspace.rs`, `crates/grove-gpui/src/main.rs`
- Modify: `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` (row 04 → done)

**Interfaces:**
- Produces: a running Grove-gpui window with one live terminal, all input paths wired; the phase's evidence.

- [ ] **Step 1: Replace the body placeholder**

`Workspace::render` keeps its `window.set_rem_size(px(16.0 * zoom))` call and its sidebar/appbar/statusbar placeholders, but the `flex_1()` body becomes the `TerminalView`. Delete the Plan 03 debug text (theme name / zoom / tick) — Plan 03 Task 7 Step 2 said this phase deletes it. Spawn the session once in `Workspace::new`; focus the terminal on the first frame so keystrokes land without a click.

Pinch-to-zoom (deferred to this phase by Plan 03 Task 6 Step 2) belongs here: a `ScrollWheelEvent` with the platform zoom modifier adjusts `ZoomState` instead of scrolling, clamped `[0.6, 2.0]`, and arms the 250ms settings debounce. If the pinned rev exposes no distinguishable pinch/magnify event, implement modifier+wheel only and record that in your report.

- [ ] **Step 2: Full automated verification**

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

Expected: everything green, the Plan 03 metric selftest still prints its `cell_w=7.5… OK` line, and **`git status` reports no changes at all** under `src/`, `crates/grove-core/`, `crates/grove-terminal/`. Read the raw output.

- [ ] **Step 3: MANUAL — the spec Appendix A **Terminal** checklist (human, real desktop)**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui
# and, side by side, the installed iced build:
~/.local/bin/grove
```

Report each row pass/fail. **Do not claim any of these yourself** — they are the exit gate and they need eyes. Rows are quoted from spec Appendix A → *Terminal*:

1. **Metrics/glyphs.** `CELL 7.5×17 @ 12.5pt BlexMono`. Long lines stay column-aligned to the right edge; box-drawing and Nerd/powerline glyphs occupy exactly one cell; bold renders as the bundled Bold face. Side-by-side vs iced Grove on the same `claude` session.
2. **CJK.** A wide char under-fills its two-cell slot (findings §S1). Judge whether the `force_width` attempt (Task 3 Step 5) improved it, made it worse, or was unavailable — and record the verdict, because it decides whether the helper survives.
3. **ANSI→token map.** Run a 256-color test script (`for i in $(seq 0 255)`) in both builds and compare: index 0 = bg_strip, 1|9 = red … 8 = fg_mute, the cube and grayscale ramps. Then flip themes (Plan 03's chords) and confirm terminal content re-colors on the next frame.
4. **Inverse.** Something using reverse video (`printf '\e[7mINVERSE\e[0m'`, a tmux status line, `less` search highlight) shows the theme-default fill, not transparent or double-swapped text.
5. **Cursor.** Block cursor, blinking on the AnimationClock phase (`%16<8`), matching iced's rhythm; hidden when the inner app hides it (`vim`, `htop`); parked correctly when scrolled back.
6. **Selection.** Drag-select across several rows: overlay is `rgba(.40,.50,.78,.35)`; scroll while selected and the highlight **stays on the same text**; copy (Ctrl+Shift+C / Cmd+C) yields trailing-whitespace-cleaned text; a keypress kills the selection and the drag; dragging to the top/bottom edge auto-scrolls and extends.
7. **Click-to-caret.** Click mid-line at a shell prompt: caret moves horizontally only; clicking a different row does nothing; clicking while scrolled back does nothing; the click that focuses the window does not move the caret; inside an app with mouse reporting the click is forwarded instead.
8. **Scroll feel.** Trackpad (pixel deltas) vs wheel (line deltas) both feel identical to iced Grove; a fast flick does not flood tmux; reversing direction mid-gesture responds immediately; scrolling up in tmux enters copy-mode and the next keystroke leaves it exactly once.
9. **Keyboard scrollback.** Shift+PageUp/PageDown, Shift+Home/End; the Home/End full-scrollback jump is capped and does not hang; typing snaps back to the bottom.
10. **Input bytes.** Ctrl+C interrupts; Alt chords reach the app as ESC-prefixed; Alt+Escape arrives as ESC ESC; arrows work in `vim` and at a readline prompt in both cursor modes.
11. **Paste and drop.** Bracketed paste of a multi-line block arrives as one paste with `\r` line endings; on Wayland, "Copy" a file in a file manager then paste → the shell-escaped path plus a trailing space; drag-drop a file (X11/macOS) → the same.
12. **Resize.** Resize the window and change zoom across `[0.6, 2.0]`: the grid re-dims, the inner app reflows to the new size, and nothing clips or overlaps.
13. **Idle cost.** Leave the window open and unfocused for 60s; CPU should sit near the spike's release figure (1.23%) and comfortably under iced Grove's ~3.6%.

Rows explicitly **deferred** and not checked here (record them as deferred, not failed): per-project pinned content themes and the project-themes toggle → Plan 05; mouse-report forwarding *inside grid tiles* and Agent/Panel focus routing → Plan 07; the two-step confirm-kill and Escape-despite-capture carve-outs → Plan 08.

- [ ] **Step 4: `./install.sh`** — the orchestrator runs this.

```bash
./install.sh 2>&1 | tail -20
```

Expected: the release build + install of the **iced** `grove` binary still succeeds, untouched by this phase.

- [ ] **Step 5: Update the master plan and commit** — the orchestrator runs this.

Mark row 04 `done` in `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` with a one-line note recording: the CJK `force_width` verdict, whether `GroveTerm::snapshot()` pre-applies the inverse swap, the pinch-zoom event availability, and any Appendix A row that came back FAIL.

```bash
git add crates/grove-gpui Cargo.toml Cargo.lock \
        docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md
git commit -m "feat(gpui): terminal element, single live session, full input path"
```

**Exit gate met when:** the keyboard byte-table test is green (raw output pasted), the spec Appendix A Terminal rows above are signed off by a human as pass or explicitly-deferred, grove-gpui builds/tests/clippy clean on 1.95, the iced app and both existing crates are provably untouched and still build on the default toolchain, and `./install.sh` is green.
