# gpui Rewrite Plan 02: `crates/grove-terminal` + dual-parser golden harness

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. This is **production** code, not a spike: superpowers:test-driven-development is mandatory (tests before implementation, red before green), the workspace clippy denies apply, and superpowers:verification-before-completion governs every "done" claim.

**Goal:** Land `crates/grove-terminal` — a headless, gpui-free `alacritty_terminal` wrapper implementing the spec's parser API contract in **token space** — and prove it byte-for-byte equivalent to the in-tree `vt100` parser via a golden harness driven by real recorded PTY streams. Exit gate: **golden tests green against the vt100 oracle**, `./install.sh` green, one commit. No gpui code lands in this phase.

**Architecture:**

```
crates/grove-terminal/
  Cargo.toml            member of the MAIN workspace; deps: alacritty_terminal (zed fork,
                        pinned rev) + portable-pty. dev-deps: vt100 (oracle only).
                        NO gpui / gpui_platform / gpui-component anywhere.
  src/lib.rs            pub use of the model + cell types
  src/color.rs          TermColor { Default | Ansi(u8) | Rgb(u8,u8,u8) }
  src/cell.rs           Cell { text, fg: TermColor, bg: TermColor, bold }, Run, Snapshot
  src/term.rs           GroveTerm: Term<Listener> behind FairMutex + Processor; the API
                        contract (process/snapshot/tail_contents/selection_text/title/
                        bell_count/mouse_mode/encoding/app_cursor/cursor/display_offset/
                        scroll_to/resize/damage generation)
  src/pty.rs            portable-pty spawn + blocking reader thread -> channel (no async
                        runtime; the caller owns the executor)
  tests/fixtures/*.bin  committed recorded PTY byte streams (+ .meta.json: rows/cols/label)
  tests/golden.rs       dual-parser harness: feeds each fixture to vt100 AND GroveTerm,
                        asserts cell-by-cell equality
  tests/capture.rs      #[ignore]d capture helper that records new fixtures
```

`grove-terminal` is a **pure model crate**. Element/painting, theme resolution and the ANSI→token color table stay out of it — those are Plan 04. The crate emits `TermColor`; it never resolves a theme color. Nothing in `src/` (the iced app) is rewired to use it in this phase; the iced build keeps using `vt100` untouched.

**Tech Stack:** `alacritty_terminal` = `{ git = "https://github.com/zed-industries/alacritty", rev = "4c129667ce56611becdc82de6e28218c80e2e88f" }` (version `0.26.1-dev`), `portable-pty` 0.9, `vt100` 0.15 (dev-dependency, oracle). Toolchain: whatever the **default** system rustc is (currently 1.94.1) — see Task 1 Step 4, the STOP gate.

## Global Constraints

- Branch: `gpui-rewrite` (already exists; do not create a new one).
- **Pins are law.** ZED_REV `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba`, GPUI_COMPONENT_REV `88f102d13654fe25aa2fede076274b6b751a3704`, alacritty fork rev `4c129667ce56611becdc82de6e28218c80e2e88f`. Only the alacritty rev is actually consumed by this plan (no gpui dep here), but record it in the **root** `Cargo.toml` `[workspace.dependencies]` by exact rev, never a branch or version range. Never bump mid-phase.
- **Durable-pin question is out of scope here.** The findings doc's `[patch."https://github.com/zed-industries/zed"]` hazard (spikes point gpui at a GC-able `~/.cargo/git/checkouts` path) applies to *gpui-component*, which this plan does not depend on. Do **not** add any `[patch]` to the main workspace. If a resolution error mentioning `zed-industries/zed` appears, STOP — it means a gpui dep leaked in, which this phase forbids.
- vt100 stays in-tree (master-plan standing rule) as the oracle until Plan 10. Do not touch `crates/grove-core/src/session.rs`, `src/gui/pty.rs`, or `src/gui/activity.rs` in this phase — they are read-only references.
- TDD order is enforced by the task order: fixtures (Task 2) and the oracle harness (Task 3) exist and **fail red** before any of `src/term.rs`'s behavior is written (Tasks 4–6).
- Quality bars: `cargo clippy -p grove-terminal --all-features -- -D warnings` clean under the workspace `unwrap_used`/`expect_used` deny (production paths; `#[cfg(test)]` modules and `tests/` may unwrap freely, matching the existing CI split documented in the root `Cargo.toml`). `rustfmt --edition 2021` on touched files only, never crate-wide.
- Behavior questions are answered by reading the iced code, never by guessing. Canonical references: `crates/grove-core/src/session.rs:760-980` (snapshot, `tail_contents`, `current_title`, `bell_count`, `resize`, `selection_text`), `src/gui/pty.rs:374-421` (`normalize_selection`, `vt_color_opt`, `ansi_idx`), `src/gui/activity.rs` (fixture-shaped screen snippets).
- alacritty wiring is **reused verbatim** from findings §S1 Step 2 — do not re-derive it:
  ```rust
  use alacritty_terminal::{
      Term, event::EventListener, grid::Dimensions, sync::FairMutex,
      term::{Config, TermMode, test::TermSize}, vte::ansi::{Processor, StdSyncHandler},
  };
  let term = FairMutex::new(Term::new(Config { scrolling_history: 5000, ..Default::default() },
                                      &size, Listener));
  Processor::<StdSyncHandler>::new().advance(&mut *term.lock(), &chunk);
  // TermSize implements Dimensions (total_lines / screen_lines / columns)
  // damage: Term::damage(&mut self) -> TermDamage<'_>, Term::reset_damage(&mut self)
  // colors: vte::ansi::Color::{Named,Indexed,Spec}; NamedColor::Foreground/Background
  //         are the "default" cases == vt100::Color::Default
  // Flags::INVERSE swaps fg/bg; Flags::WIDE_CHAR_SPACER cells are skipped
  ```
- **Reflow (carry from findings §S1 Step 2):** the spec §3 says "reflow-on-resize is suppressed"; the spike proved **there is no config knob** — `Term::resize` hardcodes `!is_alt`, so the primary screen always reflows and the alt screen never does. This plan does **not** patch alacritty. It encodes the constraint as a *test*: the resize-storm fixtures run on the alt screen (tmux's actual regime) where vt100 and alacritty agree, and a separate `#[test]` documents the primary-screen divergence as a known, asserted difference. Recorded in Task 5 Step 3.
- **CJK (carry from findings §S1 Step 1):** wide chars shape to 1.33 cells, not 2. That is a *painting* artifact, out of scope here — but the model must still emit wide chars in the vt100-compatible cell layout (char in the lead cell, `WIDE_CHAR_SPACER` skipped), which the golden tests check.
- No `git` commands until Task 6. Do not commit intermediate tasks.

---

### Task 1: Crate scaffold, workspace membership, and the toolchain gate

**Files:**
- Create: `crates/grove-terminal/Cargo.toml`, `crates/grove-terminal/src/lib.rs` (stub)
- Modify: root `Cargo.toml` (`members`, `[workspace.dependencies]`)

**Interfaces:**
- Produces: a buildable empty `grove-terminal` on the **default toolchain**, and the pinned `alacritty_terminal` workspace dependency every later task consumes.

- [ ] **Step 1: Add the crate to the workspace**

Root `Cargo.toml`: `members = ["crates/grove-core", "crates/grove-terminal"]`, and under `[workspace.dependencies]`:

```toml
alacritty_terminal = { git = "https://github.com/zed-industries/alacritty", rev = "4c129667ce56611becdc82de6e28218c80e2e88f" }
portable-pty = "0.9"
```

(`portable-pty` is currently a direct `"0.9"` in `crates/grove-core/Cargo.toml`; hoisting it to the workspace table and switching grove-core to `portable-pty.workspace = true` keeps one version — do that, it is the workspace convention already used for `vt100`/`serde_json`/etc.)

- [ ] **Step 2: Write `crates/grove-terminal/Cargo.toml`**

Mirror `crates/grove-core/Cargo.toml`'s conventions exactly (`publish = false`, `version.workspace`, `edition.workspace`, `rust-version.workspace`, `license.workspace`, `[lints] workspace = true`):

```toml
[dependencies]
alacritty_terminal.workspace = true
portable-pty.workspace = true

[dev-dependencies]
vt100.workspace = true      # ORACLE ONLY — never a normal dependency
serde_json.workspace = true # fixture .meta.json
```

- [ ] **Step 3: Stub `src/lib.rs`**

```rust
//! Headless terminal model for Grove: an `alacritty_terminal` wrapper emitting
//! token-space cells. Contains no gpui types and no theme resolution.
#![forbid(unsafe_code)]
```

- [ ] **Step 4: STOP GATE — build under the DEFAULT toolchain**

The main workspace has **no** `rust-toolchain.toml` and pins `rust-version = "1.94"`. The spikes only ever compiled `alacritty_terminal` under a user-local rustup 1.95.0 (findings §Build status). Verify explicitly:

```bash
cd /home/gitfudge/dev/gitfudge0/grove
rustc --version                       # record the exact version in your report
cargo build -p grove-terminal 2>&1 | tail -20
```

Expected: `Finished`.

**If it fails to compile on the default toolchain (e.g. E0658 or any "requires newer rustc"), STOP AND REPORT to the orchestrator.** Do **not** add a `rust-toolchain.toml` to the main workspace, do not pin a toolchain, do not vendor, do not downgrade the alacritty rev — pinning the whole product's toolchain is a user-level decision. Report: the rustc version, the exact error, and which crate in the dependency graph raised it.

- [ ] **Step 5: Verify nothing else broke**

```bash
cargo build 2>&1 | tail -5
cargo clippy -p grove-terminal --all-targets -- -D warnings 2>&1 | tail -5
```

Expected: `Finished` and no clippy output. Do not commit yet.

---

### Task 2: Fixture capture — real recorded PTY byte streams

**Files:**
- Create: `crates/grove-terminal/tests/capture.rs` (`#[ignore]`d recorder)
- Create: `crates/grove-terminal/tests/fixtures/*.bin` + `*.meta.json` (committed)
- Create: `crates/grove-terminal/tests/fixtures/README.md` (how each was recorded)

**Interfaces:**
- Produces: the fixture corpus + `Fixture { bytes, rows, cols, label, alt_screen }` loader that Tasks 3–6 consume. Nothing here asserts parser behavior yet.

- [ ] **Step 1: Define the fixture format**

A fixture is a raw byte dump of everything read off a PTY master, plus a sidecar:

```json
{ "label": "claude-tmux-session", "rows": 34, "cols": 120,
  "alt_screen": true, "recorded": "2026-07-31", "how": "tmux new-session -A -s cap; claude; typed 2 prompts" }
```

Write the loader as `fn load_all() -> Vec<Fixture>` in a shared `tests/common/mod.rs` (globs `tests/fixtures/*.bin`, parses the sidecar with `serde_json`). Fixtures are raw bytes — never text — so escape sequences survive round-tripping.

- [ ] **Step 2: Write the recorder**

`tests/capture.rs`, a single `#[test] #[ignore]` that reads env vars so a human can record any command:

```rust
// GROVE_CAPTURE_CMD="tmux new-session -A -s cap" GROVE_CAPTURE_LABEL=claude-tmux \
//   GROVE_CAPTURE_SECS=45 cargo test -p grove-terminal --test capture -- --ignored --nocapture
```

It spawns via `portable_pty::native_pty_system().openpty(PtySize{rows, cols, ..})` + `CommandBuilder`, forwards stdin, and appends every read chunk to `tests/fixtures/<label>.bin` while also writing the sidecar. Reuse the reader-thread shape from findings §S1 Step 2 (blocking `std::thread` + `read()`), no async.

- [ ] **Step 3: Record the required corpus** (human-in-the-loop; run each capture, then eyeball the `.bin` sizes)

Required labels, minimum one fixture each:
1. `claude-tmux` — `claude` running inside tmux, ≥2 prompts and one long streaming response.
2. `codex-tmux` — `codex` inside tmux, one turn.
3. `tmux-bare` — plain tmux with a shell, status bar redraws, a window split.
4. `vim` — `vim` opening a file, `:set number`, scroll, `:q` (heavy alt-screen + cursor addressing).
5. `resize-storm` — inside tmux, repeatedly resize the terminal narrower/wider ≥20 times while output is streaming. This is the alt-screen resize case.
6. `resize-storm-primary` — the same but on a **bare shell** (no tmux), i.e. the primary screen. Recorded specifically so the known-divergence test in Task 5 Step 3 has evidence.
7. `sgr-torture` — a synthetic stream (no capture needed; generate in code) covering: all 16 ANSI named colors fg+bg, the 6×6×6 cube boundary indices (16, 231), grayscale ramp bounds (232, 255), truecolor `38;2;r;g;b`, bold on/off, `INVERSE` on/off, `\x07` bells ×3, OSC 0/1/2 title sets, CJK + Nerd Font glyphs, and a `\x1b[?1049h`/`l` alt-screen toggle.
8. Reuse: transcribe the per-agent screen snippets already in `src/gui/activity.rs:392-620` into a `activity-snippets.bin` fixture (they are plain text screens; prefix each with a clear + cursor-home so they parse deterministically). These make the `tail_contents` equivalence test in Task 5 meaningful against the classifier's real inputs.

Keep each fixture under ~2 MB; truncate long captures rather than committing megabytes.

- [ ] **Step 4: Sanity test the loader (first real test, and it must pass)**

```rust
#[test] fn every_fixture_loads_and_is_nonempty() { /* assert count >= 8, bytes non-empty, rows/cols > 0 */ }
```

```bash
cargo test -p grove-terminal --test golden 2>&1 | tail -20   # or wherever the loader test lands
```

- [ ] **Step 5: Document**

`tests/fixtures/README.md`: one line per fixture — label, what it exercises, the exact capture command. Anyone re-recording later must be able to reproduce it.

---

### Task 3: The dual-parser oracle harness (RED — no implementation yet)

**Files:**
- Create: `crates/grove-terminal/tests/common/oracle.rs` (vt100 side)
- Create: `crates/grove-terminal/tests/golden.rs` (the comparison tests)
- Create: `crates/grove-terminal/src/cell.rs`, `src/color.rs` (types only — the *shape* the tests compile against)

**Interfaces:**
- Produces: `ScreenDump` — the canonical comparison value both parsers must produce — and a full suite of failing tests. Tasks 4–6 turn them green without editing them.

- [ ] **Step 1: Define the neutral comparison value**

```rust
// tests/common/mod.rs
#[derive(Debug, PartialEq, Eq)]
pub struct ScreenDump {
    pub rows: u16, pub cols: u16,
    pub cells: Vec<CellDump>,          // row-major, rows*cols entries
    pub cursor: (u16, u16),            // (row, col)
    pub cursor_hidden: bool,
    pub title: Option<String>,
    pub bell_count: usize,
    pub display_offset: usize,
    pub app_cursor: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub struct CellDump { pub text: String, pub fg: TermColor, pub bg: TermColor, pub bold: bool }
```

`TermColor` is the **production** type from `grove_terminal::TermColor` (`Default | Ansi(u8) | Rgb(u8, u8, u8)`, `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`) — the oracle converts *into* it, so the test asserts on one shared vocabulary. Write `src/color.rs` and `src/cell.rs` now (types + derives only, no behavior).

- [ ] **Step 2: Write the vt100 oracle**

`fn oracle_dump(bytes: &[u8], rows: u16, cols: u16) -> ScreenDump` using `vt100::Parser::new(rows, cols, 5000)`, then per cell:
- `text`: `cell.contents()` (empty string for blanks — normalize `""` and `" "` to `" "` on **both** sides, once, in a shared `normalize_cell_text` helper; document why in a comment).
- `fg`/`bg`: map `vt100::Color::{Default, Idx(i), Rgb(r,g,b)}` → `TermColor::{Default, Ansi(i), Rgb(r,g,b)}`. This is exactly `vt_color_opt`/`ansi_idx`'s *input* domain from `src/gui/pty.rs:382-421` with the theme lookup removed — the point of token space.
- `bold`: `cell.bold()`.
- **INVERSE**: vt100 exposes `cell.inverse()`; alacritty has `Flags::INVERSE`. Do **not** pre-swap in either dump — record inverse as a swap applied identically by both sides *inside the dump function* (fg/bg exchanged, `Default` mapping to `Default` on both sides). One helper, called by both, so the semantics cannot drift.
- `cursor`, `cursor_hidden`, `title`, `bell_count` (`screen().audible_bell_count()`), `display_offset` (`screen().scrollback()`), `app_cursor` (`screen().application_cursor()`).

- [ ] **Step 3: Write the failing golden tests**

In `tests/golden.rs`, one `#[test]` per axis, each looping over every fixture:

1. `golden_cells_match` — full `ScreenDump.cells` equality. On mismatch, print the **first** differing `(row, col)` with both sides and a ±3-row text rendering of each; a 4000-cell `assert_eq!` diff is unreadable otherwise.
2. `golden_cursor_matches`
3. `golden_title_matches`
4. `golden_bell_count_matches`
5. `golden_tail_contents_matches` — `n` ∈ {1, 5, 20, 60}.
6. `golden_after_resize_matches` — for each fixture, feed bytes, then resize through a script of sizes, then compare. **Alt-screen fixtures only** in this test (see Task 5 Step 3).
7. `golden_chunking_invariance` — the same fixture fed as one blob vs. split at 1/7/64/4096-byte boundaries must produce identical `GroveTerm` dumps (guards against a stateful-parser bug at chunk edges).
8. `golden_selection_text_matches` — a fixed set of `(start_abs, end_abs)` rectangles per fixture.

- [ ] **Step 4: Confirm RED for the right reason**

```bash
cargo test -p grove-terminal 2>&1 | tail -30
```

Expected: compile error or failures because `GroveTerm` does not exist / is unimplemented — **not** because the oracle side panics. If the oracle panics, fix the oracle now; a broken oracle silently invalidates everything downstream. Record the exact failure lines in your report.

---

### Task 4: `GroveTerm` core — parse, snapshot, cursor, title, bells

**Files:**
- Create: `crates/grove-terminal/src/term.rs`
- Modify: `crates/grove-terminal/src/lib.rs` (re-exports)

**Interfaces:**
- Consumes: the harness from Task 3 (do not edit `tests/`).
- Produces: `GroveTerm` with `new/process/snapshot/cursor/title/bell_count/app_cursor` — turns golden tests 1–4 and 7 green.

- [ ] **Step 1: Construct the term** (wiring verbatim from Global Constraints)

```rust
pub struct GroveTerm { /* FairMutex<Term<GroveListener>>, Processor<StdSyncHandler>,
                          bells: usize, damage_gen: u64, rows: u16, cols: u16 */ }
impl GroveTerm { pub fn new(rows: u16, cols: u16) -> Self }
```

`GroveListener` implements `EventListener`; its `send_event` counts `Event::Bell` into a **monotonic** counter (spec: "own monotonic counter off `Event::Bell`") and captures `Event::Title`/`ResetTitle`. Note: alacritty reports the title via events, unlike vt100 which exposes `screen().title()` — the listener is the only place it can come from, so keep it behind an `Arc<Mutex<..>>` the `GroveTerm` reads. `unwrap_used` is denied: use `if let Ok(g) = m.lock()` / `.ok()?` throughout, matching `session.rs`'s style (`let Ok(mut p) = self.parser.lock() else { return String::new() };`).

- [ ] **Step 2: `process(&mut self, bytes: &[u8])`**

`self.processor.advance(&mut *term, bytes)`, then read `Term::damage()`; if `TermDamage::Full` or any `Partial` item exists, bump `damage_gen`; then `reset_damage()`. This is the findings §S1 Step 5 pattern minus the gpui `cx.notify`.

- [ ] **Step 3: `snapshot()` → token-space cells**

Walk the visible grid (`grid.display_offset()`-relative), producing `Cell { text, fg: TermColor, bg: TermColor, bold }`:
- map `vte::ansi::Color::{Named, Indexed, Spec}` → `TermColor`. `NamedColor::Foreground`/`Background` → `TermColor::Default`; the 16 `NamedColor` ANSI entries → `TermColor::Ansi(0..=15)` (keep bright variants at their 8..=15 indices — `ansi_idx` folds `1|9`, `2|10`, … to the same theme token, so the *index* must survive to Plan 04 unfolded); `Indexed(i)` → `Ansi(i)`; `Spec(Rgb{r,g,b})` → `Rgb(r,g,b)`.
- `Flags::BOLD` → `bold`. `Flags::INVERSE` → the shared swap semantics (Task 3 Step 2).
- Skip `Flags::WIDE_CHAR_SPACER` cells; the lead cell carries the wide char.
- Italic/underline/dim/strikethrough are **deliberately dropped** (spec §3, explicit parity decision) — add a comment saying so, so nobody "fixes" it later.

Adjacent-run coalescing is a *painting* concern (Plan 04) — `snapshot()` returns cells, not runs. Only add a `Run` type if the golden harness needs one; it does not.

- [ ] **Step 4: `cursor()`, `title()`, `bell_count()`, `app_cursor()`**

`cursor()` returns `(row, col, hidden)` where hidden is `!term.mode().contains(TermMode::SHOW_CURSOR)`; positioning uses `grid.cursor.point.line + grid.display_offset()` (findings §S1 Step 3) so a scrolled-back view matches vt100. `title()` trims and returns `None` when empty — copy the exact semantics from `session.rs:872-880`. `app_cursor()` is `TermMode::APP_CURSOR`.

- [ ] **Step 5: Green tests 1, 2, 3, 4, 7**

```bash
cargo test -p grove-terminal --test golden 2>&1 | tail -40
```

Expected: `golden_cells_match`, `golden_cursor_matches`, `golden_title_matches`, `golden_bell_count_matches`, `golden_chunking_invariance` all pass. Tests 5, 6, 8 still fail (unimplemented) — that is correct at this point. Paste the raw pass/fail lines into your report; do not summarize.

---

### Task 5: Scrollback, resize, and `tail_contents`

**Files:**
- Modify: `crates/grove-terminal/src/term.rs`

**Interfaces:**
- Produces: `tail_contents/resize/display_offset/scroll_to` — turns golden tests 5 and 6 green, and lands the asserted primary-screen reflow divergence.

- [ ] **Step 1: `display_offset()` / `scroll_to(n)`**

`display_offset()` = `grid.display_offset()`. `scroll_to(n)` uses `term.scroll_display(Scroll::Delta(..))` / `Scroll::Top`/`Bottom` to land on an absolute offset, clamped to `scrolling_history` (5000), mirroring `session.rs:690-705`'s cap-at-configured-scrollback behavior.

- [ ] **Step 2: `tail_contents(n)`**

Port `crates/grove-core/src/session.rs:882-928` **semantically, not literally** — read it first, then reproduce: temporarily zero the scroll offset so a scrolled-back user doesn't feed stale markers to the classifier; read a `2n`-row window off the bottom; trim trailing blank lines; if the window came back with fewer than `n` real lines and the window was clipped, fall back to the whole grid; restore the original offset. The doc comment must say it exists to feed `src/gui/activity.rs`'s classifier.

- [ ] **Step 3: `resize(rows, cols)` + the reflow divergence**

Port `session.rs:945-975`: clamp to ≥1, no-op when unchanged, and **snap the scrollback offset to 0 first** (the vt100 comment says vt100 doesn't clamp on resize; alacritty may, but snap unconditionally so both parsers see the same starting state). Then `term.resize(TermSize::new(cols, rows))`.

Then:
- Make `golden_after_resize_matches` pass over the alt-screen fixtures (`tmux-bare`, `vim`, `resize-storm`, `claude-tmux`, `codex-tmux`).
- Add one **new** test, `primary_screen_reflow_is_a_known_divergence`, over `resize-storm-primary`: assert the two parsers **differ**, and assert the specific shape (alacritty rewraps, vt100 does not) with a doc comment citing findings §S1 Step 2 (`term/mod.rs:677`, `self.grid.resize(!is_alt, ..)`, no config knob) and the spec §3 sentence it contradicts. An asserted, documented divergence is the deliverable — not a fix.

- [ ] **Step 4: Verify**

```bash
cargo test -p grove-terminal 2>&1 | tail -40
```

Expected: tests 5 and 6 green, `primary_screen_reflow_is_a_known_divergence` green, test 8 still red.

---

### Task 6: Selection, mouse/encoding surface, and phase close-out

**Files:**
- Modify: `crates/grove-terminal/src/term.rs`, `src/lib.rs`
- Create: `crates/grove-terminal/src/pty.rs`
- Modify: `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` (row 02 → done)

**Interfaces:**
- Produces: the complete API contract from spec §3, all golden tests green, `./install.sh` green, one commit. Plan 03 consumes `grove_terminal::{GroveTerm, TermColor, Cell}`.

- [ ] **Step 1: `selection_text(abs_start..abs_end)`**

Grove's selection is a **plain cell rectangle in absolute (scrollback-inclusive) coordinates** — alacritty's semantic `Selection` is explicitly NOT used (spec §3). Reimplement over the grid, matching `session.rs:800-868`'s extraction and `src/gui/pty.rs:374-381`'s `normalize_selection` ordering ((row, col) tuple compare, swap if reversed). Turn `golden_selection_text_matches` green.

- [ ] **Step 2: `mouse_mode()` / `encoding()`**

Return crate-local enums (`MouseMode { None, Normal, Button, Any }`, `MouseEncoding { Default, Sgr, Utf8 }`) derived from `TermMode::{MOUSE_REPORT_CLICK, MOUSE_DRAG, MOUSE_MOTION, SGR_MOUSE, UTF8_MOUSE}`. Do **not** re-export `vt100`'s `MouseProtocolMode`/`MouseProtocolEncoding` (`session.rs:8`) — the whole point is that grove-terminal owns its vocabulary. Add a unit test asserting the mapping against the `sgr-torture` fixture's mode toggles, plus a `#[cfg(test)]` cross-check that the vt100 oracle reports the equivalent mode for the same bytes.

- [ ] **Step 3: `src/pty.rs` — spawn + reader thread**

`pub fn spawn(cmd: CommandBuilder, rows, cols) -> Result<PtyHandle>` where `PtyHandle` owns the master, a `Box<dyn Write + Send>` writer, and a blocking reader `std::thread` pushing `Vec<u8>` chunks into a `std::sync::mpsc::Receiver` (**not** `futures` — grove-terminal must not pick an executor; Plan 03 bridges the receiver into `cx.spawn`). Wiring per findings §S1 Step 2. Errors via `std::io::Error`/a small `thiserror` enum — no `unwrap`/`expect` anywhere.

- [ ] **Step 4: Full green + quality gates**

```bash
cd /home/gitfudge/dev/gitfudge0/grove
cargo test -p grove-terminal 2>&1 | tail -30
cargo test 2>&1 | tail -20
cargo clippy -p grove-terminal --all-targets -- -D warnings 2>&1 | tail -20
rustfmt --edition 2021 --check crates/grove-terminal/src/*.rs crates/grove-terminal/tests/*.rs crates/grove-terminal/tests/common/*.rs
```

Every golden test must pass. The whole-workspace `cargo test` must be unaffected (the iced app was not touched). Per superpowers:verification-before-completion, read the raw output — do not claim green off a summary line.

- [ ] **Step 5: `./install.sh`**

```bash
./install.sh 2>&1 | tail -20
```

Expected: release build + install succeeds (several minutes). Project rule; non-negotiable.

- [ ] **Step 6: Update the master plan and commit**

Mark row 02 `done` in `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md`, adding a one-line note for the primary-screen reflow divergence (spec §3 amendment) and the observed rustc version from Task 1 Step 4.

```bash
git add crates/grove-terminal Cargo.toml crates/grove-core/Cargo.toml Cargo.lock \
        docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md
git commit -m "feat(terminal): grove-terminal crate + dual-parser golden harness"
```

**Exit gate met when:** golden tests green against the vt100 oracle, `./install.sh` green, no gpui dependency anywhere in the main workspace, iced app untouched and still building.
