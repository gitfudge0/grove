# gpui Rewrite Plan 10 — Parity passes + delete vt100/iced

**Spec:** `docs/superpowers/specs/2026-07-31-gpui-rewrite-design.md` (Appendix A is the acceptance checklist; §1 non-goals says the iced code is deleted at the end of the branch; §2 is the final workspace layout)
**Master:** `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md`, row 10
**Branch:** `gpui-rewrite`
**Exit gate:** *Full Appendix A signed off; iced gone; `./install.sh` green.*

This is the last phase. It is the only phase that deletes things, and it is the
only phase with a **hard user gate in the middle**. It is organised as three
phases, in order, and the boundary between Phase B and Phase C is not
negotiable:

| Phase | What | Who |
|---|---|---|
| **A** | Pre-cutover parity fixes, measurements, screenshot sweep, consolidated sign-off doc | agents (Tasks 1–5) |
| **B** | **USER GATE** — the user reviews the screenshots, runs the manual checklists, signs off Appendix A | **the user, alone** (Task 6) |
| **C** | Delete iced + vt100, relocate the gpui app to `src/`, restore CI, clean grove-core, ship | agents (Tasks 7–8) |

> ## **NOTHING IS DELETED UNTIL THE USER EXPLICITLY APPROVES.**
>
> **No worker may start Task 7 on its own judgement, on a green test suite, on a
> "the checklists look fine" reading, or on another agent's report. The only
> thing that unlocks Phase C is the user saying so, in their own words, after
> Task 6. If you are a worker and you were handed Task 7 without being told the
> user signed off — STOP and ask the orchestrator.**

---

## Global constraints

1. **TDD where logic changes.** Task 1 changes layout math: test first, red, then
   fix. Tasks 2–5 produce tooling and documents (no product-logic change);
   Task 7's mechanical moves are verified by the existing suites, plus one new
   test for the re-based golden harness.
2. **The orchestrator runs `./install.sh` and every `git` command.** Workers
   never commit, never bump the master row, never run `install.sh`.
3. **grove-core amendment protocol — lifted for Task 7 Step 5 only.** Every
   phase so far has treated `crates/grove-core` as read-only. That freeze is
   released in this plan for exactly two things, both in Task 7:
   - deleting `crates/grove-core/src/session.rs` and its `vt100` dependency
     (the iced app is its only consumer — verified below), and
   - the 9 clippy-1.95 lints carried forward from Plan 03.
   Nothing else in grove-core may change. Any *behavioral* change to a
   surviving grove-core module is a STOP-and-report.
4. **Pins do not move.** ZED_REV `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba`,
   GPUI_COMPONENT_REV `88f102d13654fe25aa2fede076274b6b751a3704` (vendored),
   `alacritty_terminal` rev `4c129667…`. `grep -c '^name = "gpui"' Cargo.lock`
   must stay **1** through the whole plan. Deleting iced will move a lot of
   `Cargo.lock` lines; none of them may be a pinned rev.
5. **rustfmt only touched files, `--edition 2021`.** After Task 7 the workspace
   edition is still 2021 (only the vendored `gpui-component` crates are
   edition 2024, and they are workspace-excluded) — do not "upgrade" it.
6. **Behavior questions are answered by reading the iced code**, right up until
   Task 7 deletes it. Task 7 Step 1 is the last chance: after the delete, the
   oracle is gone and any unanswered parity question becomes unanswerable.
   If a question is open when Task 7 starts, STOP and report instead of
   guessing.
7. **Deferrals owed to this plan** (from the master table, gathered here so
   nothing is dropped):
   - tile-vs-single-session PTY padding difference (row 07 deviation 5) → **Task 1**
   - numeric idle-power measurement (spec §9 spike 5) → **Task 2**
   - the scripted screenshot sweep (spec §8.3) → **Task 3**
   - macOS dock badge/bounce and the mac-only ⌘ chords + ⌘-SVG substitution
     (rows 06/08) → **Task 5's sign-off doc, run by the user on a macOS host**
   - IME composition + the Wayland clipboard round-trip (findings §S2/§S4,
     rows 08/09) → **Task 5's sign-off doc**
   - the vt100 latent deep-scrollback panic noted in Plan 02's oracle → **Task 7
     Step 2** (it dies with vt100; record that it did, do not "fix" it)
   - golden fixtures kept as grove-terminal regression tests (spec §10.11) → **Task 7 Step 2**
   - CI `--workspace` restoration + toolchain unification → **Task 7 Step 6**
   - grove-core's 9 clippy-1.95 lints (Plan 03 carry-forward) → **Task 7 Step 5**
   - spec §2 `src/` relocation so the gpui app becomes THE app → **Task 7 Step 3**
8. **Toolchain, during Phases A and B:** unchanged from Plan 09 — the iced app
   and the core crates build on the default 1.94.1, grove-gpui with
   `PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 … -p grove-gpui`. Task 7 Step 6
   unifies them; before that step, do not touch `rust-toolchain.toml`.

---

## Facts established by reading the repo (do not re-derive, but do re-verify if something looks wrong)

- **`crates/grove-core/src/session.rs` is the iced app's session type, and it is
  the only thing in the workspace that pulls in `vt100` as a real dependency.**
  `grep -rn "grove_core::session" crates/grove-gpui/src` returns **doc comments
  only** — grove-gpui owns its own `entities/terminal_session.rs`
  (`TerminalSession`) over `grove-terminal`. Inside grove-core, only
  `attention.rs:25` and `session_meta.rs:2` touch it, and both import just
  `session::{Result, SessionError}` — the error types, not the parser. So
  removing `vt100` means: delete `session.rs`, rehome `Result`/`SessionError`,
  keep everything else.
- `vt100` appears in three manifests: the root package (iced), grove-core
  (normal dep), grove-terminal (**dev**-dependency, oracle only).
- The root package `grove` (`Cargo.toml:106-155`) *is* the iced binary, and it
  carries `[package.metadata.bundle]` and the `assets/icon/*` paths that
  `install.sh` depends on.
- `install.sh` runs `cargo bundle --release` **at the repo root** and installs
  either the produced `.app`/`.deb` or `target/release/grove` +
  `~/.local/share/applications/grove.desktop`. Everything it does is keyed to
  the root package being named `grove` and producing a binary named `grove`.
- `.github/workflows/ci.yml` names `-p grove -p grove-core -p grove-terminal`
  in four places (lines 66, 71, 81, 85) and explains why grove-gpui is excluded
  (lines 59-64). CI reads the channel from `rust-toolchain.toml`, which pins
  **1.94.1**; `spikes/rust-toolchain.toml` pins 1.95.0.
- Iced's single-session PTY geometry comes from
  `src/gui/metrics.rs::compute_pty_dims`, which subtracts `PTY_PAD_W = 36` and
  `PTY_PAD_H = 28` (`metrics.rs:21-22`) on top of the chrome; grid tiles and
  panel shells use `TILE_PTY_PAD_W = 32` / `TILE_PTY_PAD_H = 24`
  (`metrics.rs:54-56`), which grove-gpui already mirrors as `px(16)`/`px(12)`
  per side (`views/grid.rs:222-223`, `views/term_panel.rs:115-116`). **The
  single-session body in grove-gpui is unpadded** — that is the row 07
  deviation 5 gap Task 1 closes.

---

## Task 1 (Phase A): the single-session PTY padding decision

**Files:**
- Modify: `crates/grove-gpui/src/views/workspace.rs` (the single-session body and the terminal-tab body)
- Modify: `crates/grove-gpui/src/views/terminal_tab.rs` if the tab body owns its own container

**Interfaces:** produces no new public API. Produces one parity test in
`views::workspace`'s test module, in the same shape as Plan 07's two exact
assertions (`views/grid.rs`, `views/term_panel.rs`), where the iced oracle
formula is reimplemented **locally in the test** and never exported.

- [ ] **Step 1: Read both oracles and decide, in writing, before editing**

Read `src/gui/metrics.rs:265-295` (`compute_pty_dims`) and
`src/gui/view/terminal.rs:181-200` (the single-session `column![sess_bar, pty]`)
and `:398-405` (the terminal tab). Then read the gpui side:
`views/workspace.rs::session_body` and the `terminal_focused` branch at
`workspace.rs:1664-1693`.

Record in your report, as three explicit answers:
1. Where iced's 36×28 actually lives — the container padding around `pty()`, or
   a fudge constant in `compute_pty_dims` with no matching container. If it is a
   fudge constant, the fix is a **container padding** in gpui of half of it per
   side; if it is real padding, port the same padding.
2. Whether the **terminal tab** shares that padding with the single-session
   body in iced (both call `self.pty(PtyPane::Agent, …)`, but the tab has its
   own bar).
3. Whether zen (chrome hidden) changes it. `compute_pty_dims` drops the chrome
   terms when `chrome_visible` is false but **keeps** `PTY_PAD_W`/`PTY_PAD_H` —
   confirm that and say so.

- [ ] **Step 2: Write the parity test FIRST (red)**

In `views/workspace.rs`'s test module, reimplement `compute_pty_dims` locally
(the oracle), then assert `(rows, cols)` equality against the gpui body's
post-padding bounds → `ZoomState::pty_dims`, for this matrix:

| window | zoom | sidebar | chrome |
|---|---|---|---|
| 1280×800 | 1.0 | 320 | visible |
| 1280×800 | 1.0 | 220 | visible |
| 1280×800 | 2.0 | 320 | visible |
| 1280×800 | 0.6 | 320 | visible |
| 1280×800 | 1.0 | 320 | **hidden (zen)** |

Compute the expected numbers **from the oracle in the test**, not from this
plan. (For orientation only, and not to be trusted: the first row works out to
`usable_w = 1280 − (320 + 6 + 36) = 918 → 122 cols`,
`usable_h = 800 − (44 + 26) − 36 − 28 = 666 → 39 rows`. If your oracle
disagrees with that, your oracle is right and this sentence is wrong.)

The assertion is **exact, delta 0**, exactly like Plan 07's — if you cannot make
it exact, STOP and report the residual rather than loosening the assertion to a
tolerance.

- [ ] **Step 3: Apply the padding, go green**

Add the container padding to the single-session body (and the terminal tab, per
Step 1 answer 2). Use named constants beside grid's `TILE_PTY_PAD_*`, e.g.
`PTY_PAD_W = 36.0` / `PTY_PAD_H = 28.0` applied as `.px(px(PTY_PAD_W / 2.0))` /
`.py(px(PTY_PAD_H / 2.0))`, with a comment citing `src/gui/metrics.rs:21-22`
**before that file is deleted** — the citation is the only record that survives.

- [ ] **Step 4: Anything else the checklists flagged as code-fixable**

Re-read the five checklist files' "Known gap" / "Deferred" sections
(`…-06-attention-checklist.md`, `…-08-modals-checklist.md`,
`…-09-system-checklist.md`, and Task 6 Step 3 / Task 7 Step 2 of plans 04, 05,
07). Anything that is a **code** gap rather than a *human-verification* gap gets
fixed here or is explicitly recorded as won't-fix with a reason. Known
candidates at plan-writing time:
- Plan 04 row 2's CJK `force_width` verdict — the helper's survival is a
  checklist outcome, so it may need deleting after Phase B, not before. **Leave
  it; note it in Task 5's doc as a decision the user makes at the gate.**
- Plan 06 row 16's "no toast producer" gap — **closed by Plan 07**; confirm and
  say so.
- Plan 08 row 12's Tools gap — **closed by Plan 09's scope addition**; confirm.

Do **not** invent new parity work here. If you find a genuine functional gap
that is not on any checklist, STOP and report it to the orchestrator; it is a
scope decision, not a worker decision.

- [ ] **Step 5: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -5
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -5
cargo test 2>&1 | tail -5   # default 1.94.1: iced + core crates still green
git status --short src crates/grove-core crates/grove-terminal   # must be EMPTY
```

---

## Task 2 (Phase A): idle-power measurement, both builds, same method as spike S1

**Files:**
- Create: `scripts/idle-power.sh`
- Create: `docs/superpowers/plans/2026-07-31-gpui-rewrite-10-idle-power.md`

**Interfaces:** a repeatable script anyone can re-run, plus a recorded table.
The script must not be a one-shot: the user will re-run it at the gate.

- [ ] **Step 1: Reproduce spike S1's method exactly**

Findings §S1 Step 5 / "Idle-power comparison": **`/proc/<pid>/stat` utime+stime
deltas over 60 s windows, window open and unfocused.** (`pidstat` is not
installed on this box; the /proc delta is the same measurement.) Do not
substitute `top`, `powertop`, or RAPL — a different method makes the numbers
incomparable to the spike's, which is the whole point.

The baseline table to reproduce and extend:

| build | %CPU over 60s (spike S1) |
|---|---|
| spike-term, release, blink on | 1.23 % |
| spike-term, release, `SPIKE_NO_BLINK=1` | 0.00 % |
| real Grove (`~/.local/bin/grove`, release) | 3.85 % / 3.55 % |

- [ ] **Step 2: Write `scripts/idle-power.sh`**

Contract: takes a pid (or a label + command), samples
`/proc/<pid>/stat` fields 14+15 (utime+stime, clock ticks) at t=0 and t=60,
divides by `getconf CLK_TCK`, prints `%CPU = 100 * ticks_delta / (CLK_TCK * 60)`.
Must:
- refuse to run if the pid is gone or if the window count changes mid-sample;
- print the sample window, the pid, the raw tick deltas and the derived %CPU
  (raw numbers are the evidence; the percentage is the summary);
- support `--windows N` to take N consecutive 60 s windows and print each;
- be `set -euo pipefail`, no bashisms beyond what `install.sh` already uses;
- state in a header comment that it is Linux-only, and that the macOS
  equivalent is `ps -o time= -p <pid>` sampled the same way (the user runs the
  macOS half by hand at the gate).

- [ ] **Step 3: Measure four scenarios, both builds, side by side**

Both binaries built `--release`, both windowed and **unfocused**, same window
size (1280×800), same theme, 3×60 s windows each:

| scenario | gpui | iced |
|---|---|---|
| A. one idle session, no PTY output, unfocused | | |
| B. same, plus one background agent actively working (streaming output) | | |
| C. grid view, n=5, all idle, unfocused | | |
| D. zen, single session, idle, unfocused | | |

Scenario A is the spec §4 "adaptive tick" claim (60 ms vs 1 s gating) and the
one that must not regress against iced. Scenario B is spec §9 spike 5's second
half ("busy background agent still classifies at ~480 ms and paints smoothly").

- [ ] **Step 4: Record the numbers side by side**

Write `…-10-idle-power.md`: the method (verbatim, one paragraph), the exact
commands run, the machine/session type (Wayland or X11 — say which, and note
that the other is untested unless you ran both), the raw tick deltas, the four
scenarios × two builds table, and a one-line verdict per scenario:
**gpui ≤ iced**, **gpui > iced**, or **inconclusive**.

**Do not editorialize a regression into a pass.** If gpui is worse in any
scenario, say so in bold and let the user decide at the gate — that is exactly
what the gate is for. A regression here is a legitimate reason for the user to
refuse Phase C.

---

## Task 3 (Phase A): the screenshot sweep

**Files:**
- Create: `scripts/screenshot-sweep.sh`
- Create: `docs/superpowers/plans/2026-07-31-gpui-rewrite-10-screenshot-sweep.md` (the index the user reads)
- Output (gitignored, not committed): `target/parity-shots/{gpui,iced}/<slug>.png`

**Interfaces:** produces a side-by-side capture set and an index document
pairing them. The user reviews the pairs; the agent never judges them.

- [ ] **Step 1: Decide the capture mechanism and record it**

gpui at ZED_REV exposes no screenshot API, and neither build has a scripted UI
driver. So the sweep is **external capture of a real window**, driven by the
operator:
- Wayland: `grim -g "$(slurp -o)"` or `grim -w <window>` if the compositor
  supports it; X11: `import -window <id>` (ImageMagick) or `xwd`.
- Probe which of these actually exist on this box **first**
  (`command -v grim slurp import xwd spectacle gnome-screenshot`) and write the
  script against what is there. If none exist, STOP and report — do not add a
  dependency without the orchestrator.
- Window geometry must be **identical** between the two builds for a pair to be
  comparable: pin 1280×800 and say in the script how (compositor rule, or a
  manual resize step the operator performs once).

Record the chosen mechanism at the top of the script and in the index doc.

- [ ] **Step 2: Enumerate the capture list from spec §8.3 + Appendix A**

Spec §8.3 asks for: *every screen/modal × 3 zooms × 4 representative themes +
follow-system flip × grid n∈{1,2,3,5} × panel open/zen*. Taken literally that is
a combinatorial explosion nobody will review. The sweep is therefore
**stratified**: the full screen/modal list at one baseline configuration, plus a
zoom/theme cross-section on a representative subset. Write the list into the
script as a table of `slug → how to reach it`, so the operator can work down it.

**Base configuration:** 1280×800, zoom 1.0, TokyoNight dark, chrome visible.

*Screens (base config), from Appendix A → Screens/layout:*

1. `workspace-empty-no-projects`
2. `workspace-empty-has-projects`
3. `workspace-single-session`
4. `sidebar-collapsed` / 5. `sidebar-sessions-only` / 6. `sidebar-all`
7. `sidebar-hover-actions` (worktree row hover strip)
8. `sidebar-agent-menu` (anchored overlay)
9. `sidebar-git-suffix` (dirty/ahead/behind visible)
10. `sidebar-archived-empty-state`
11. `sidebar-terminals-expanded` / 12. `sidebar-terminals-docked`
13. `grid-n1` / 14. `grid-n2` / 15. `grid-n3` / 16. `grid-n5`
17. `grid-drag-target` (source dimmed, target cyan inset)
18. `grid-tile-waiting` (amber border + scrim + respond chip)
19. `zen-single` / 20. `zen-attention-pill`
21. `terminal-tab` / 22. `terminal-tab-multiple`
23. `panel-20` / 24. `panel-40` / 25. `panel-75`
26. `panel-tabs-multi-shell`
27. `appbar-attention-pill` / 28. `attention-dropdown`
29. `statusbar-default` / 30. `statusbar-toast-info` / 31. `statusbar-toast-error`
32. `session-header-working` (spinner + 3-dot + "in progress")

*Modals (base config), from Appendix A → Modals — one shot each, in the
checklist's order:*

33. `modal-input` 34. `modal-confirm` 35. `modal-confirm-quit`
36. `modal-addproject-step1` 37. `modal-addproject-autocomplete`
38. `modal-addproject-step2-git-init`
39. `modal-removeproject` 40. `modal-removeproject-progress`
41. `modal-archiveproject` 42. `modal-archived-list`
43. `modal-message` 44. `modal-tmuxchoice` 45. `modal-agentpicker`
46. `modal-launcher-recents` 47. `modal-launcher-filtered`
48. `modal-launcher-row-actions` 49. `modal-launcher-switch-drill`
50. `modal-launcher-settings-drill` 51. `modal-launcher-theme-preview`
52. `modal-themepicker-dark` 53. `modal-themepicker-light`
54. `modal-themepicker-project-scope`
55. `modal-thememanager` 56. `modal-theme-editor`
57. `modal-settings-general` 58. `modal-settings-tools`
59. `modal-shortcutoverlay-workspace` 60. `modal-shortcutoverlay-grid`
61. `modal-teardown-running` 62. `modal-teardown-done`
63. `modal-scriptseditor` 64. `modal-updating` 65. `modal-changelog`
66. `onboarding-step1` 67. `onboarding-step2`

*Cross-section (the representative subset × the axes spec §8.3 names):*

- **Zooms** {0.6, 1.0, 2.0} × {`workspace-single-session`, `grid-n3`,
  `panel-40`, `modal-launcher-recents`, `modal-settings-general`} → 15 pairs
  (the 1.0 column is already captured above; capture 0.6 and 2.0).
- **Themes**, 4 representative — pick one dark, one light, one high-contrast,
  one custom `themes.json` theme, and **name the four in the index** ×
  {`workspace-single-session`, `grid-n3`, `modal-themepicker-dark`} → 12 pairs.
- **Follow-system flip**: `workspace-single-session` before and after an OS
  appearance change, both builds → 2 pairs. This one is a *pair of pairs*: the
  first frame after launch in follow-system mode is a named Appendix A
  behavior ("not a flash of the wrong one"), so capture the **first frame**.
- **Grid n∈{1,2,3,5} × panel open / zen**: already covered by 13–16, 19, 23–25;
  add `grid-n3-panel-open` if that combination is reachable (Plan 07 recorded
  that **the panel is suppressed in grid view, with no exception** — so if it
  is unreachable, record it as N/A with that citation rather than as a miss).

- [ ] **Step 3: Write `scripts/screenshot-sweep.sh`**

It drives the *operator*, not the app: for each slug it prints the slug, the
"how to reach it" instruction, waits for Enter, captures the focused window to
`target/parity-shots/$BUILD/$slug.png`, and moves on. `BUILD` is `gpui` or
`iced`, taken as `$1`. Support `--only <slug-prefix>` to resume, and skip slugs
whose PNG already exists unless `--force`. Add `target/parity-shots/` to
`.gitignore` — **PNGs are not committed**; they are review material.

- [ ] **Step 4: Run the sweep for both builds**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 build --release -p grove-gpui
./scripts/screenshot-sweep.sh gpui     # runs target/release/grove-gpui
./scripts/screenshot-sweep.sh iced     # runs ~/.local/bin/grove
```

If a slug is unreachable in one build, capture the other and record the slug as
**MISSING in <build>** in the index — that is a finding, not a skip.

- [ ] **Step 5: Write the index document**

`…-10-screenshot-sweep.md`: the capture mechanism, the base configuration, the
four theme names chosen, and a table `slug | gpui | iced | reviewed ☐ | verdict`
with relative paths to both PNGs. **Every verdict cell starts empty.** An agent
filling one in is the same violation as an agent signing a checklist row.

---

## Task 4 (Phase A): freeze the golden dumps while vt100 is still alive

**Files:**
- Create: `crates/grove-terminal/tests/expected/<fixture>__<case>.dump` (committed text)
- Modify: `crates/grove-terminal/tests/common/mod.rs` (a `ScreenDump` text serializer + a bless path)
- Modify: `crates/grove-terminal/tests/golden.rs`, `crates/grove-terminal/tests/divergence.rs`

**Why this is in Phase A, before the gate:** the vt100 oracle is what gives the
frozen dumps their authority. Once Task 7 deletes vt100 the dumps can only ever
be re-blessed from `GroveTerm` itself, which is circular. Freezing them **now**,
from the oracle, is what turns spec §10.11's "keep golden fixtures as
grove-terminal regression tests" into a real regression test rather than a
self-snapshot. This task changes no product code and is safe to land pre-gate.

> **grove-terminal amendment:** this task touches `crates/grove-terminal/tests/`
> only — never `src/`. The Constraint-3 freeze on grove-terminal's *source*
> still holds.

- [ ] **Step 1: Decide and record the assertion form**

**The chosen form is: committed, human-readable expected-dump files, generated
from the vt100 oracle in this task, compared byte-for-byte thereafter.**

Rejected alternatives, with reasons — write these into the module doc of
`tests/common/mod.rs`, because the reasoning is the only thing that stops a
future reader from "simplifying" it back:
- *Keep vt100 as a dev-dependency forever.* Rejected: spec §1 and the master
  standing rules say vt100 leaves at Plan 10, and a dev-dep still pins a parser
  the product no longer uses.
- *Bless the dumps from `GroveTerm` after the delete.* Rejected: circular. The
  test would then assert only "alacritty behaves the way alacritty behaved
  yesterday", which is a change-detector, not a parity test. It cannot detect a
  regression that was already present at bless time.
- *Assert hand-written invariants instead of dumps.* Rejected: the fixtures'
  value is exactly their cell-by-cell density; summarizing them throws away the
  coverage Plan 02 paid for.

- [ ] **Step 2: Serialize `ScreenDump` deterministically**

Text, not bincode/JSON — a diff must be reviewable in a PR. One line per cell
run: `r{row} c{col} "{text}" fg={TermColor} bg={TermColor} bold={bool}`, then a
trailer for cursor `(row,col,visible)`, title, bell count, display offset, and
the `tail_contents(n)` / `selection_text` probe outputs the existing harness
already compares. Blank-cell normalization and the INVERSE swap stay exactly
where they are (`common::normalize_cell_text`, `common::apply_inverse`) so the
frozen text is the *same* neutral dump the two parsers already agree on.

- [ ] **Step 3: Bless every case from the oracle**

For every fixture in `tests/fixtures/` × every `CHUNK_SIZES` entry × the
`RESIZE_SCRIPT` cases the current `golden.rs` runs, write
`tests/expected/<fixture>__<case>.dump` **from `common::oracle`**, not from
`GroveTerm`. Gate it behind `GROVE_TERM_BLESS=1` so it is never regenerated by
accident:

```bash
GROVE_TERM_BLESS=1 cargo test -p grove-terminal --test golden
```

For the two **asserted divergences** pinned by `divergence.rs` — the primary
screen reflowing on resize, and alacritty retaining an `ED 2`-cleared screen in
scrollback — the oracle is by definition wrong. Bless those cases from
`GroveTerm`, and name each such file `…__DIVERGENCE.dump` with a header comment
inside it citing the master-plan row 02 text. A reader must never have to guess
which side a file came from.

- [ ] **Step 4: Re-point the tests at the files, with vt100 still present**

`golden.rs` now asserts `dump(GroveTerm) == read(expected file)`. Keep the
oracle comparison **alive as a third assertion in this task** — model vs oracle
vs file, all three agreeing — so the freeze is proven correct while vt100 is
still there. Task 7 Step 2 deletes only the oracle leg.

Add one test that fails if `tests/expected/` has a file with no matching case or
a case with no matching file — the drift guard that stops a fixture from
silently losing its assertion.

- [ ] **Step 5: Verify**

```bash
cargo test -p grove-terminal 2>&1 | tail -5     # default 1.94.1
git status --short crates/grove-terminal/src     # must be EMPTY
```

Report the number of `.dump` files written and their total size.

---

## Task 5 (Phase A): the consolidated sign-off document

**Files:**
- Create: `docs/superpowers/plans/2026-07-31-gpui-rewrite-10-signoff.md`

**Interfaces:** ONE file listing **every** outstanding manual row from every
phase checklist, with the command to run each. This is the document the user
works down in Phase B, and it is the thing "Full Appendix A signed off" means.

- [ ] **Step 1: Gather every row from every source**

Six sources, in phase order. Copy each row **verbatim** — do not summarize, do
not renumber within a source, do not "improve" the wording. A row the user
cannot match back to its origin checklist is a row they cannot trust.

| source | file / location | rows |
|---|---|---|
| Plan 04 — Terminal | `…-04-terminal-element.md`, Task 6 Step 3 | 13 |
| Plan 05 — Sidebar | `…-05-sidebar.md`, Task 7 Step 2 | 11 |
| Plan 06 — Attention/activity | `…-06-attention-checklist.md` | 17 (× Wayland **and** X11) |
| Plan 07 — Grid/zen/tab/panel | `…-07-grid-zen.md`, Task 7 Step 2 | 13 |
| Plan 08 — Modals | `…-08-modals-checklist.md` | 23 |
| Plan 09 — System | `…-09-system-checklist.md` | 10 + row A |
| | **carried subtotal** | **88** |

Plus the six rows this plan itself owes, which exist nowhere else:

| # | Plan 10 row | Host |
|---|---|---|
| P1 | **macOS dock badge + one bounce per enter-while-unfocused** (Plan 06 row 9's macOS half) | macOS |
| P2 | **macOS ⌘ chords + the ⌘-SVG substitution in ShortcutOverlay** (Plan 08 row 13's deferral), incl. the Cmd+Opt+H collision workaround and Cmd-chord suppression in the palette (spec §5, §7) | macOS |
| P3 | **IME composition** inside every text-heavy modal field (findings §S2) — compose a CJK string in the launcher, the add-project path field, and a multiline editor; the preedit renders in place and commits once | Linux + macOS |
| P4 | **Wayland clipboard round-trip** (findings §S4) — copy from the terminal into another Wayland app and back; the `wl-paste` file-URI fallback; OSC 52 from inside tmux | Wayland |
| P5 | **Idle-power numbers accepted** — read `…-10-idle-power.md` and accept or reject each of the four scenarios | Linux |
| P6 | **Screenshot sweep reviewed** — work down `…-10-screenshot-sweep.md` and fill every verdict cell | any |

**Total: 94 rows** (88 carried + 6 new), with the 17 Plan 06 rows each needing
two platform columns.

- [ ] **Step 2: Structure the document so it is actually runnable**

Header block with the two commands, once, at the top:

```bash
cd /home/gitfudge/dev/gitfudge0/grove
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui   # Wayland
WAYLAND_DISPLAY= PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui   # X11
~/.local/bin/grove                                                        # iced, side by side
```

Then one section per source, each row a table line:
`| origin | # | row (verbatim) | PASS/FAIL/DEFER | notes |`, where `origin` is
e.g. `08/14` (plan 08, row 14) so a failure is traceable in one hop. Carry each
source's own "explicitly deferred" list into a short note under its section, so
the user does not re-litigate a deferral that a later phase already closed —
and where a later phase *did* close one, say so (Plan 06 row 16's toast
producer → closed by Plan 07; Plan 08 row 12's Tools gap → closed by Plan 09).

Add a short **open decisions** section at the end, for the things the gate
decides rather than verifies:
- **CJK `force_width` helper** (Plan 04 row 2): keep or delete, based on how it
  actually looks. Record the user's answer; Task 7 acts on it.
- Any Task 1 Step 4 won't-fix, listed with its reason.

- [ ] **Step 3: State the gate, in the document, in bold**

The document must open with the same sentence this plan opens with: **nothing is
deleted until the user explicitly approves**, and the sign-off is the approval.
It must also say that **no agent may fill in a result cell** — every cell is the
user's, and an agent-filled cell invalidates the sign-off.

- [ ] **Step 4: Sanity-check the count**

Assert in the document's header that it contains 94 rows, and that the section
subtotals are 13 / 11 / 17 / 13 / 23 / 11 / 6. If your gathered count differs
from 94, **the count in this plan is not authoritative — the checklists are**.
Report the discrepancy and the corrected number rather than dropping or
inventing rows to hit 94.

---

## Task 6 (Phase B): **THE USER GATE**

> **This task is not performed by an agent. There is no worker for this task.**
> The orchestrator hands the user four artifacts and waits.

**Artifacts handed over:**
1. `…-10-signoff.md` — 94 rows to work down
2. `…-10-screenshot-sweep.md` + `target/parity-shots/{gpui,iced}/` — the pairs
3. `…-10-idle-power.md` — the four scenarios, both builds
4. `scripts/idle-power.sh`, `scripts/screenshot-sweep.sh` — re-runnable

**What the user must do, exactly:**

- [ ] **1. Review the screenshot pairs.** Work down the sweep index; for each
  pair, mark the verdict cell. "Pixel-familiar, not pixel-perfect" is the spec's
  own bar (§1) — a different anti-aliasing is a pass, a missing element or a
  wrong layout is a fail.
- [ ] **2. Run the 88 carried checklist rows**, side by side against
  `~/.local/bin/grove`, on **Wayland and X11** for the Plan 06 section. Mark
  PASS / FAIL / DEFER.
- [ ] **3. Run rows P1–P2 on a macOS host** — dock badge/bounce, ⌘ chords, the
  ⌘-SVG substitution, the Cmd+Opt+H workaround. These are the only rows that
  cannot be run on this machine at all. If no macOS host is available, mark them
  **DEFER** and say so explicitly — Phase C may then proceed **only** if the
  user accepts shipping with macOS unverified, and that acceptance must be
  written down.
- [ ] **4. Run rows P3–P4** — IME composition and the Wayland clipboard
  round-trip.
- [ ] **5. Read `…-10-idle-power.md`** and accept or reject each scenario (row
  P5). A rejected scenario means Phase C waits.
- [ ] **6. Answer the open decisions** — the CJK `force_width` helper's fate,
  and any Task 1 won't-fix.
- [ ] **7. Say so.** Explicitly, in your own words: *the sign-off is complete
  and Phase C may proceed.* Anything less — silence, "looks good", a green test
  run, an agent's summary — is not approval.

**If any row FAILs:** Phase C does not start. The failure comes back as new
work (a fix task in this plan, or a new plan), the affected rows are re-run, and
the gate is re-presented. **Deleting the oracle while a parity row is red
destroys the only thing that can diagnose it.**

---

## Task 7 (Phase C): the cutover — **REQUIRES TASK 6 APPROVAL**

> **Do not start this task without the user's explicit approval from Task 6.**
> Confirm with the orchestrator that it exists before you touch a file.

**Files:** the largest blast radius in the whole rewrite. Delete `src/` (73
files), `crates/grove-core/src/session.rs`, `crates/grove-gpui/` (moved, not
lost), and rewrite `Cargo.toml`, `rust-toolchain.toml`, `.github/workflows/ci.yml`,
`install.sh`.

**Order matters.** Each step leaves the tree in a state the next step can
verify. Do not batch them.

- [ ] **Step 1: Last look at the oracle**

Before deleting anything, confirm every open question is closed:
- Task 5's open-decisions section has the user's answers.
- No checklist row is FAIL.
- `grep -rn "src/gui\|src/app" crates/grove-gpui/src | wc -l` — these are the
  file:line citations pointing at code that is about to vanish. They stay (they
  are provenance, and git history keeps them resolvable), but **read the list**
  and confirm none of them is a TODO that still needs the oracle. Report the
  count.

- [ ] **Step 2: Delete vt100 and re-base the golden harness**

1. `crates/grove-terminal/tests/golden.rs` + `divergence.rs`: drop the oracle
   leg of every assertion (Task 4 Step 4 kept three-way agreement; now it is
   model vs frozen file). Delete `tests/common/oracle.rs` and its `mod`
   declaration.
2. Remove `vt100.workspace = true` from `crates/grove-terminal/Cargo.toml`'s
   `[dev-dependencies]`.
3. **Record, in `tests/common/mod.rs`'s module doc, that the vt100 latent
   deep-scrollback panic (vt100 0.15.2 `grid.rs:125`, `visible_rows` computing
   `rows_len - scrollback_offset` unguarded, documented in the deleted
   `oracle.rs`) leaves the tree with vt100 and needs no further action.** That
   note is the only surviving trace of a real bug the harness had to work
   around; do not let it disappear silently.
4. `crates/grove-core`: delete `src/session.rs`, drop `pub mod session;` from
   `lib.rs`, and rehome `Result`/`SessionError` — the only two items
   `attention.rs:25` and `session_meta.rs:2` import from it. Move them into a
   new `crates/grove-core/src/error.rs` (or into `session_meta.rs` if that reads
   better after the move — decide by which produces the smaller diff and say
   which you chose). Remove `vt100.workspace = true` from grove-core's manifest.
5. Remove `vt100 = "0.15"` from `[workspace.dependencies]`.

```bash
grep -rn "vt100" --include='*.rs' --include='*.toml' . | grep -v '^./target' | grep -v '^./vendor'
# expected: only the two provenance comments (grove-terminal tests, tmux.rs prose)
cargo test -p grove-terminal -p grove-core 2>&1 | tail -5
```

- [ ] **Step 3: Delete the iced app and relocate grove-gpui to `src/`**

**Mechanism — promote grove-gpui *into* the existing root `grove` package.**
Not a crate rename in `crates/`, not a virtual workspace root. Concretely:

1. `git rm -r src/` (the iced app, 73 files) and drop the iced-only deps from
   the root `[dependencies]`: `iced`, `rfd`, `tokio`, `vt100`, and the
   `[dev-dependencies] smol_str`.
2. Move `crates/grove-gpui/src/` → `src/`.
3. Merge `crates/grove-gpui/Cargo.toml`'s `[dependencies]`,
   `[target.'cfg(target_os = "macos")'.dependencies]` and `[dev-dependencies]`
   into the root package's corresponding sections. Keep `[[bin]] name = "grove"`,
   `path = "src/main.rs"`.
4. Delete `crates/grove-gpui/`; drop it from `[workspace] members` and from
   `default-members` (which then becomes redundant — remove the key and its
   comment entirely).
5. Set `rust-version = "1.95"` in `[workspace.package]` and delete
   grove-gpui's now-homeless `rust-version` comment.
6. Keep `[workspace] exclude` for the three vendored crates, unchanged.

**Why this mechanism and not the alternative:** the alternative — renaming
`grove-gpui` → `grove` in place under `crates/` and making the workspace root
virtual — breaks three things at once. `install.sh` runs `cargo bundle` **at the
repo root**, which requires the root to be a real package; `[package.metadata.bundle]`
carries `assets/icon/*` paths that are resolved relative to the manifest, so
they would all have to move or be re-pathed; and the two names would collide
(`grove` at root and `grove` in `crates/`) for as long as both exist, forcing an
atomic rename with no verifiable intermediate state. Promoting into the root
package keeps the package name, version 0.45.0, the bundle metadata, the icon
paths, the binary name `grove`, `~/.local/bin/grove`, the `.desktop` entry,
`cargo-deny`'s wildcard-path allowance and `cargo machete`'s view all valid —
the only things that change are **file paths and a dependency list**, both of
which the compiler checks. It also matches spec §2's layout literally: `src/` is
the gpui app, `crates/` holds grove-core and grove-terminal.

Fix up whatever the move breaks: `crates/grove-core` → `crates/grove-core` path
deps become `crates/…` from the root (they already are, since the root package
already depends on grove-core that way); grove-gpui's `rust-embed` asset paths
(`include-exclude` on `assets/fonts`) are now root-relative — verify the font
still embeds, because a silently-empty `AssetSource` is a runtime failure, not a
compile error.

```bash
cargo build --release 2>&1 | tail -5
GROVE_GPUI_SELFTEST=1 ./target/release/grove   # the Plan 03 metric assertion: cell_w ≈ 7.5
```

The selftest passing is the proof the font embed survived the move.

- [ ] **Step 4: Unify the toolchain**

Root `rust-toolchain.toml` → `channel = "1.95.0"`, components `["rustfmt",
"clippy"]`, keeping its existing "pinned, not stable" comment (the reasoning is
unchanged; only the number moves). Update the comment to say *why* it moved:
zed at ZED_REV uses `std::hint::cold_path`. `spikes/rust-toolchain.toml` is now
redundant but harmless — leave it; the spikes are throwaway scaffolding and
deleting them is not this plan's job.

- [ ] **Step 5: Clean grove-core's 9 clippy-1.95 lints** *(the grove-core
      amendment protocol is lifted for exactly this)*

```bash
cargo clippy -p grove-core --all-targets -- -D warnings 2>&1 | tail -40
```

Plan 03 named `map_unwrap_or` and `duration_suboptimal_units` among 9. Enumerate
what clippy 1.95 actually reports now that `session.rs` is gone — the count may
be lower, since some of them may have been in the deleted file. **Fix them as
lints, not as refactors:** `map_unwrap_or` → `map_or`, `duration_suboptimal_units`
→ `Duration::from_hours`/`from_mins` (Plan 09 already hit this one in
grove-gpui). Any lint that cannot be fixed without changing behavior gets an
`#[allow]` with a one-line reason, not a behavioral change. Report the before
count, the after count, and every `#[allow]` added.

`cargo test -p grove-core` must be green before and after, with the same test
count (minus whatever `session.rs` owned).

- [ ] **Step 6: Restore CI to `--workspace` and unify it**

`.github/workflows/ci.yml`:
- Replace all four `-p grove -p grove-core -p grove-terminal` invocations
  (lines 66, 71, 81, 85) with `--workspace`. The vendored gpui-component crates
  are workspace-**excluded**, so `--workspace` still does not lint them.
- Delete the lines 59-64 comment block explaining grove-gpui's exclusion, and
  replace it with a one-liner noting the vendored crates are excluded and why.
- **Linux build deps change**: the apt list installs iced/GTK prerequisites
  (`libgtk-3-dev libxdo-dev libayatana-appindicator3-dev`). gpui on Linux needs
  the Wayland/X11/wgpu stack instead. Derive the real list from what
  `cargo build` needed on this box (check `crates/grove-gpui`'s build history or
  just build in a clean container if one is available) — do **not** guess.
  `librsvg2-dev libssl-dev pkg-config dpkg` stay (bundling + TLS). If you cannot
  determine the list with confidence, say so and leave the existing list plus
  your best additions, flagged in your report as unverified — a red CI on a
  branch is recoverable, a silently-wrong dep list is not.
- The `Install Rust toolchain` step needs no change: it already reads the
  channel from `rust-toolchain.toml`, which Step 4 moved to 1.95.0.
- `macos-latest` in the matrix now builds the gpui app on macOS. Expect this to
  be the riskiest CI change; it cannot be verified locally.

- [ ] **Step 7: Update `install.sh`**

Read it end to end first. Most of it needs **no** change, because Step 3 kept
the package and binary named `grove`. Check and fix only:
- the "GUI (iced) + bundling prerequisites" comment in CI is Step 6's; here,
  check whether `install.sh` mentions iced anywhere (it does not today, but
  re-grep);
- `target/release/grove` in the Linux fallback (line 105) — still correct;
- `[package.metadata.bundle]` icon paths — unchanged by Step 3, but confirm
  `cargo bundle` still finds them from the root;
- whether the gpui build needs any env (`PATH`, toolchain) that the script does
  not set. It should not, now that `rust-toolchain.toml` pins 1.95.0.

If `install.sh` genuinely needs no edit, **say so explicitly in your report**
rather than editing it to look like work was done.

- [ ] **Step 8: Full verification (worker half)**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings -A clippy::unwrap_used -A clippy::expect_used
cargo clippy --workspace -- -D warnings
cargo test --workspace 2>&1 | tail -10
cargo test --doc --workspace 2>&1 | tail -5
grep -c '^name = "gpui"' Cargo.lock          # must be 1
grep -rn "iced" --include='*.rs' --include='*.toml' . | grep -v '^./target' | grep -v '^./vendor' | grep -v docs/
typos
cargo machete
```

Report every number. A test-count *drop* is expected (the iced app's tests go
with it) — report the before and after and account for the difference. An
unexplained drop is a STOP.

---

## Task 8 (Phase C): ship

- [ ] **Step 1: `./install.sh`** — the orchestrator runs this, reads the raw
  output, and launches the installed binary. This is the exit gate's third
  clause. A worker's claim that it passed is not evidence.

- [ ] **Step 2: Smoke the installed build** — launch `grove` from the desktop
  launcher (not a shell, so the login-PATH recovery path is exercised), spawn a
  session, confirm the sidebar, a modal and the terminal all work. This is not a
  re-run of the sign-off; it is proof the *installed* artifact is the one that
  was signed off.

- [ ] **Step 3: Update the master plan row 10** — the orchestrator writes the
  Status cell: the sign-off outcome (how many rows PASS / DEFER, and which),
  the relocation mechanism and why, the golden-test assertion form, the final
  test count, the clippy-lint count cleaned, the CI dep-list change and whether
  it is verified, and every deferral that is shipping unverified (macOS rows, if
  any).

- [ ] **Step 4: Commit** — the orchestrator, one commit, `Co-Authored-By` trailer.

---

## What this plan deliberately does NOT do

- **It does not fix parity failures found at the gate.** Those become new work.
  This plan's job is to *find* them, present them, and then delete the oracle
  only once the user says there are none left worth keeping it for.
- **It does not delete `spikes/`.** Throwaway scaffolding, out of scope.
- **It does not touch `vendor/gpui-component/`.** Vendored third-party source.
- **It does not bump any pin**, including to "the latest gpui" now that the
  branch is landing. That is a separate, post-merge decision.
- **It does not redesign anything.** Spec §1: UX redesigns are deferred until
  after the port lands. A screenshot pair where gpui looks *better* is still a
  parity question, not an improvement to keep silently.
