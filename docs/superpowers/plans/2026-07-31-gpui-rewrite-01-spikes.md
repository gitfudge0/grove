# gpui Rewrite Plan 01: Spikes

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** De-risk the four unknowns of the gpui port with throwaway code, producing a committed findings doc and locked dependency revs before any production code is written.

**Architecture:** A separate `spikes/` cargo workspace at the repo root (not a member of the main workspace, so it never affects the main build or lints). One small binary per spike. Everything in `spikes/` is throwaway: quality bars (TDD, clippy denies) do NOT apply here — measurement and findings are the deliverable. Findings land in `docs/superpowers/specs/2026-07-31-gpui-spike-findings.md`.

**Tech Stack:** gpui (git rev of `zed-industries/zed`), gpui-component (git rev), alacritty_terminal, portable-pty 0.9 (already in grove-core).

## Global Constraints

- Branch: `gpui-rewrite`, created off `main` before Task 1.
- `spikes/` is its own workspace; add `spikes/` to the root `.gitignore`? NO — spikes are committed (they're evidence), but excluded from the root workspace via its own `[workspace]` table.
- Pin gpui and gpui-component by exact rev in `spikes/Cargo.toml`; record both revs in the findings doc. gpui-component's expected gpui rev is authoritative: read gpui-component's `Cargo.toml` first and use THAT zed rev for everything.
- Terminal parity targets from spec: cell 7.5×17 @ 12.5pt "BlexMono Nerd Font Mono" (bundled at `fonts/` — verify actual path with `fd -e ttf` before Task 2), 533ms cursor blink, pixel-scroll accumulation crossing 17.0, ANSI→token mapping per `src/gui/pty.rs:390-421`.
- Zoom range 0.6–2.0; PTY dims math oracle: `src/gui/metrics.rs:265-295`.
- Each spike ends by appending its findings section + committing; never leave a spike half-recorded.

---

### Task 1: Spike workspace scaffold + rev lock

**Files:**
- Create: `spikes/Cargo.toml` (workspace with members `term`, `inputs`, `zoom`, `platform`)
- Create: `docs/superpowers/specs/2026-07-31-gpui-spike-findings.md` (header + empty sections)
- Modify: none in main workspace

**Interfaces:**
- Produces: the pinned `ZED_REV` and `GPUI_COMPONENT_REV` recorded in the findings doc header — every later task and every later phase plan uses these exact revs.

- [ ] **Step 1: Create branch**

```bash
git checkout -b gpui-rewrite
```

- [ ] **Step 2: Determine lockstep revs**

```bash
git ls-remote https://github.com/longbridge/gpui-component HEAD
# clone shallow to read its Cargo.toml gpui rev:
git clone --depth 1 https://github.com/longbridge/gpui-component /tmp/claude-1000/-home-gitfudge-dev-gitfudge0-grove/*/scratchpad/gpui-component 2>/dev/null || true
grep -A2 'gpui' <that clone>/Cargo.toml
```
Expected: a `gpui = { git = "https://github.com/zed-industries/zed", rev = "<ZED_REV>" }` (or branch) line. If it tracks a branch, resolve to the current commit with `git ls-remote`. Record both revs.

- [ ] **Step 3: Write the spike workspace**

```toml
# spikes/Cargo.toml
[workspace]
members = ["term", "inputs", "zoom", "platform"]
resolver = "2"

[workspace.dependencies]
gpui = { git = "https://github.com/zed-industries/zed", rev = "<ZED_REV>" }
gpui-component = { git = "https://github.com/longbridge/gpui-component", rev = "<GPUI_COMPONENT_REV>" }
alacritty_terminal = "0.25"          # use latest; record actual resolved version
portable-pty = "0.9"
```
(Adjust alacritty_terminal version to whatever Zed's own Cargo.lock uses at ZED_REV — check `crates/terminal/Cargo.toml` in the zed clone — and record it.)

- [ ] **Step 4: Create findings doc skeleton**

```markdown
# gpui Spike Findings — 2026-07-31
ZED_REV: <rev>   GPUI_COMPONENT_REV: <rev>   alacritty_terminal: <ver>
## S1 Terminal element   ## S2 Text inputs   ## S3 Zoom   ## S4 Linux platform
## Go/No-go
```

- [ ] **Step 5: Verify the workspace builds empty and commit**

```bash
mkdir -p spikes/term/src spikes/inputs/src spikes/zoom/src spikes/platform/src
# minimal main.rs + Cargo.toml per member (fn main() {})
cd spikes && cargo build 2>&1 | tail -5
```
Expected: `Finished` (first build compiles gpui — takes several minutes).

```bash
git add spikes docs/superpowers/specs/2026-07-31-gpui-spike-findings.md
git commit -m "spike: workspace scaffold, gpui rev lock"
```

---

### Task 2: Spike S1 — TerminalElement end-to-end (riskiest)

**Files:**
- Create: `spikes/term/src/main.rs` (single-file spike binary)

**Interfaces:**
- Consumes: pinned revs from Task 1.
- Produces: findings §S1 — measured em advance at 12.5pt, CJK/Nerd-glyph fallback verdict, scroll-delta behavior, damage-repaint verdict, reflow-suppression verdict, and the shaped-line/paint_quad API signatures actually used (later Plan 04 copies these verbatim).

- [ ] **Step 1: Window + bundled font + metric assertion**

Build a gpui app that: registers the bundled Grove mono font via an `AssetSource`, opens a window, and in the first render measures the font's advance width:

```rust
// sketch — adapt to actual TextSystem API found via the zed clone's rustdoc
let ts = window.text_system();
let run = TextRun { len: 1, font: mono_font.clone(), color: white.into(), ..default() };
let line = ts.shape_line("M".into(), px(12.5), &[run], None);
eprintln!("advance = {:?}", line.width); // TARGET: 7.5px
```
Record: exact API names/signatures used and the measured value. If ≠7.5 @12.5pt, find the pt size or font that yields 7.5 (or record that Plan 03 must derive CELL_W from measurement instead of hardcoding).

- [ ] **Step 2: PTY + alacritty grid, running claude in tmux**

Spawn `tmux new-session -A -s grove-spike` via portable-pty; feed output into `alacritty_terminal::Term` with `Config { scrolling_history: 5000, .. }`. Run `claude` inside it manually.
Record: whether resize reflows lines (resize window narrower, check whether old rows rewrap) and what config/API suppresses reflow. This is the spec's hardest parity requirement — if reflow cannot be suppressed, record the exact observable difference for a user decision.

- [ ] **Step 3: Custom Element painting the grid**

Implement `Element` (`request_layout`/`prepaint`/`paint`) painting: merged bg quads (`window.paint_quad(fill(bounds, color))`), text runs via `shape_line().paint()` at fixed 7.5px columns, block cursor quad. Colors via the ANSI→token table copied from `src/gui/pty.rs:390-421` against the TokyoNight default palette (hardcode the 11 token colors from `crates/grove-core/src/theme.rs` defaults).
Record: visual verdict vs the real Grove side-by-side (glyph alignment, CJK/Nerd glyphs without a `mono_covers`-style hack, bold rendering).

- [ ] **Step 4: Input path**

Wire: keystrokes→PTY bytes (crib the table shape from `src/gui/keys.rs`), scroll wheel (log `ScrollDelta` variants from a real trackpad AND a wheel mouse; implement the 17px pixel-accumulation), mouse click/drag reporting to tmux (SGR encode).
Record: delta variants observed per device, whether accumulation feels identical to Grove.

- [ ] **Step 5: Damage-driven repaint + idle cost**

Repaint only when the alacritty damage generation changes (`cx.notify` from the PTY reader thread via a channel task); cursor blink via a 533ms timer. With the spike idle and unfocused, measure CPU for 60s (`pidstat -p <pid> 1 60` or `top -b`).
Record: idle %CPU vs real Grove idle (measure Grove the same way), and the notify/timer pattern used.

- [ ] **Step 6: Record findings + commit**

Fill findings §S1 completely (every "Record:" above), including a PASS/FAIL against: metrics reproducible, reflow suppressible, fallback OK without cmap hack, damage repaint viable, idle cost ≈ Grove.

```bash
git add spikes/term docs/superpowers/specs/2026-07-31-gpui-spike-findings.md
git commit -m "spike: S1 terminal element findings"
```

---

### Task 3: Spike S2 — text inputs (gpui-component)

**Files:**
- Create: `spikes/inputs/src/main.rs`

**Interfaces:**
- Produces: findings §S2 — verdict "gpui-component" vs "hand-roll", plus the component API names used (Plan 08 consumes these).

- [ ] **Step 1: Single-line input behaviors**

Window with one gpui-component `Input` styled as the palette search. Verify and record each: focus on open; Escape reaches an app-level handler while input is focused (the palette-close contract, `src/gui/update/mod.rs` should_forward Escape carve-out); ←/→ usable for app navigation when input is empty vs editing; Cmd/Ctrl-chords do NOT insert characters (the ModifiersChanged suppression contract); move-cursor-to-end API exists; IME composition (type an accented char via compose key); clipboard cut/copy/paste.

- [ ] **Step 2: Multiline editor ×3**

Three multiline editors in one view (the scripts-editor shape, `src/gui/scripts_editor.rs:31-33`): Tab/click focus between them, independent scroll, paste multi-line text, select-all/copy.
Record: quality verdict, any missing behavior that would need patching/wrapping.

- [ ] **Step 3: Record findings + commit**

Findings §S2 with per-behavior PASS/FAIL table and the final recommendation.

```bash
git add spikes/inputs docs/superpowers/specs/2026-07-31-gpui-spike-findings.md
git commit -m "spike: S2 text input findings"
```

---

### Task 4: Spike S3 — two-scope rem zoom

**Files:**
- Create: `spikes/zoom/src/main.rs`

**Interfaces:**
- Consumes: S1's terminal element (copy the file — spikes may share by copy, not by crate dep).
- Produces: findings §S3 — the exact rem-scoping mechanism (`WithRemSize` or current equivalent) and the zoomed PTY-dims formula (Plan 03 consumes).

- [ ] **Step 1: Chrome + content scopes**

A window with fake chrome (a rems-styled sidebar div) and the S1 terminal element in a content scope. Bind keys to step zoom 0.6→2.0 (0.1 steps). Chrome and terminal must scale together; verify text stays crisp (no bitmap scaling) at 0.6, 1.0, 1.37, 2.0.

- [ ] **Step 2: PTY dims under zoom**

Recompute rows/cols from the zoomed cell size on every zoom step and resize the PTY; assert against the oracle math in `src/gui/metrics.rs:265-295` for the same logical window sizes (print both, compare at 3 zoom levels).
Record: formula used, any rounding divergence from the oracle.

- [ ] **Step 3: Record findings + commit**

```bash
git add spikes/zoom docs/superpowers/specs/2026-07-31-gpui-spike-findings.md
git commit -m "spike: S3 zoom findings"
```

---

### Task 5: Spike S4 — Linux platform matrix

**Files:**
- Create: `spikes/platform/src/main.rs` (window + drop target + clipboard + close-interception)

**Interfaces:**
- Produces: findings §S4 — Wayland/X11 verdicts (Plans 03/09 consume: drag-drop approach, quit-path approach).

- [ ] **Step 1: Window basics on both display servers**

Run under Wayland, then force X11 (`WAYLAND_DISPLAY= cargo run`, or gpui's backend env var — record which). Check: 1280×800 initial size honored, title set, decorations, resize, focus/blur events received (needed for attention acknowledge-on-refocus).

- [ ] **Step 2: Close-request interception**

Register the should-close callback returning `false` once then `true`; verify the window survives the first close click. Record the exact API.

- [ ] **Step 3: File drag-drop on Wayland**

Drag a file from the file manager onto the window on Wayland AND X11. Record: does gpui deliver a file-path event first-party? If not on Wayland, confirm the `wl-paste type text/uri-list` fallback (`src/gui/drop.rs` contract) still works from the spike.

- [ ] **Step 4: Clipboard**

Write+read via gpui's clipboard API; cross-check paste into/out of a normal terminal. Record whether arboard is still needed (OSC52 path is framework-free regardless).

- [ ] **Step 5: Record findings + commit**

```bash
git add spikes/platform docs/superpowers/specs/2026-07-31-gpui-spike-findings.md
git commit -m "spike: S4 platform findings"
```

---

### Task 6: Go/No-go synthesis

**Files:**
- Modify: `docs/superpowers/specs/2026-07-31-gpui-spike-findings.md` (Go/No-go section)
- Modify: `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` (mark Plan 01 done; note any spec amendments)

**Interfaces:**
- Produces: GO / NO-GO / GO-WITH-AMENDMENTS decision for the whole rewrite; list of spec deltas (e.g. "reflow not suppressible — user decision needed", "CELL_W measured not hardcoded").

- [ ] **Step 1: Write the synthesis**

For each of S1–S4: one-line verdict. Then: any spec Appendix-A row that spikes proved cannot be met as written, with the proposed amendment. Then the locked revs restated.

- [ ] **Step 2: Idle-power comparison table** (S1 Step 5 data + Grove baseline) — include raw numbers.

- [ ] **Step 3: Commit and stop**

```bash
git add -A docs/superpowers
git commit -m "spike: go/no-go synthesis"
```
**STOP: the user reviews the findings doc and makes the go/no-go call before Plan 02 is written.**
