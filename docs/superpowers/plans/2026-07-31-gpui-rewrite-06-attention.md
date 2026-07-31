# gpui Rewrite Plan 06: Appbar, statusbar, attention, dock, toasts + the 480ms activity task

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. This is **production** code: the workspace clippy denies apply (`unwrap_used`/`expect_used`), superpowers:test-driven-development governs every pure helper (tests before implementation, red before green), and superpowers:verification-before-completion governs every "done" claim — read raw command output, never a summary line. Also load the `gpui-development` skill before writing any gpui code; training-data gpui is stale and this rev is pinned.

**Goal:** Make Grove *know what its agents are doing again.* Plan 05 shipped a sidebar whose activity glyphs render but never change state — every session classifies `Idle`, `JumpToWaitingSession` is a no-op, `acknowledge` has an empty body, session rows show no context, and there is no appbar, no statusbar and no toast. This phase turns the stub into the real thing: the zero-setup hook pipeline is wired at spawn, the 480ms classification task runs the full precedence chain (native `claude agents --json` poller > hook state file > screen-scrape `classify`), the `ActivityStore` becomes live, the amber pulse animates, the dock badge/bounce fires, OSC titles reach the rows and the session header, and the appbar + statusbar + toast chrome that surrounds the workspace gets built.

Exit gate (master plan row 06): **Attention checklist rows green on both platforms** (spec Appendix A → *Attention/activity*, enumerated verbatim in Task 7 Step 2, plus the appbar/statusbar clauses of *Screens/layout* and the two *System* clauses this phase owns); `./install.sh` green; one commit.

**Out of scope — do not build it here.** Grid tiles and their waiting-scrim/"respond" chip, zen and its floating attention pill, the worktree panel, the terminal tab, Agent/Panel focus routing (Plan 07) — the *session header bar* is built here (see Recorded ambiguity 1) but the per-tile header that reuses it is Plan 07. Every modal and every text input, so the cog button, the statusbar `palette`/`shortcuts` chips and the appbar `+` all dispatch to logged stubs (Plan 08). The upgrade flow behind the cog's green dot, telemetry, quit paths and persistence debounces beyond what already exists (Plan 09) — the green dot renders from a stubbed `UpgradeState::None`. tmux sidecar discovery/reattach (Plan 09); this phase must nevertheless behave correctly for a reattached session (`attention: None` → screen-scrape fallback), which is Task 4's explicit test.

**Architecture (new/changed files only):**

```
crates/grove-gpui/
  src/entities/
    session_registry.rs   MODIFIED: SpawnTarget/spawn wire grove-core's attention
                          hook pipeline (extra args + GROVE_STATE_FILE env);
                          SessionMeta gains `attention: Option<AttentionFiles>`
                          and `spawned_at`; kill cleans the files up.
    terminal_session.rs   MODIFIED: the *signal surface* the classifier needs —
                          title(), bell_count(), tail_contents(n), output_age(),
                          alive(), root_pid(), snap_to_bottom().
    activity_store.rs     NEW (or activity.rs grows it): Tracker map + the 480ms
                          background task + dock badge/bounce + the attention
                          pulse. Fills Plan 05's `// Plan 06: data source`.
    animation_clock.rs    MODIFIED: `set_busy_inputs` learns about the attention
                          pulse so the 60ms/1s gating matches iced.
    toast.rs              NEW. Toast + ToastKind + kind-dependent TTL task.
  src/activity.rs         MODIFIED: Signals, Tracker, classify, title_working,
                          matches_working, matches_waiting + the four timing
                          constants, ported verbatim with their tests.
  src/platform/mod.rs     NEW. platform-gated leaf module.
  src/platform/dock.rs    NEW. macOS objc dock badge + requestUserAttention,
                          no-ops elsewhere — ported as-is from src/gui/dock.rs.
  src/views/
    appbar.rs             NEW. brand, agent-view toggle combo, attention pill,
                          attention dropdown, cog (+ upgrade dot).
    statusbar.rs          NEW. running count, backend label, theme name, bypass
                          chip, toast slot, palette/shortcuts hint chips, version.
    session_header.rs     NEW. the SESSBAR_H session bar: label, branch, OSC
                          context title, the 3-dot in-progress animation.
    workspace.rs          MODIFIED: appbar above / statusbar below / session
                          header atop the body; the attention dropdown layer.
    rows.rs               MODIFIED: session-row context stops being None (OSC).
```

**Tech stack additions:** none for Linux. **macOS only:** `objc = "0.2"` (already a dependency of the iced `grove` crate — reuse the exact version, target-gated `[target.'cfg(target_os = "macos")'.dependencies]`). Pins unchanged.

## Global Constraints

- Branch: `gpui-rewrite`. Toolchain regime is **identical to Plans 03–05** and is not re-litigated:
  - grove-gpui builds/tests/clippy only via `PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 -p grove-gpui`.
  - Bare `cargo build` / `cargo test` (default-members, rustc 1.94.1) must keep working untouched for `grove`, `grove-core`, `grove-terminal`. Never run `--workspace`.
  - clippy for grove-gpui runs **`--no-deps`**: `cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings`.
  - `rustfmt --edition 2021` on **touched files only**.
- **Pins are law.** ZED_REV `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba`; alacritty fork `4c129667ce56611becdc82de6e28218c80e2e88f`. No `[patch]`, no `gpui-component`.
- **Constraint 3 — grove-core and the iced app are read-only.** No edits under `src/`, `crates/grove-core/`, or `crates/grove-terminal/`. Plan 04's amendment protocol is unchanged: if a genuinely **UI-free** helper must be exposed from grove-core, **STOP and report**; the orchestrator authorizes it. Do not edit grove-core on your own judgement.
  - **Expected outcome this phase: no amendment is needed.** The entire attention pipeline already lives in grove-core and is already public and UI-free — this is the single most reusable file in the tree. Audited surface, use as-is:
    - `grove_core::attention` (`crates/grove-core/src/attention.rs`): `STATE_FILE_ENV` (:30), `AttentionState` (:36), `AttentionFiles` (:45), `cleanup_stale_files()` (:122 — the startup GC, "delete only files whose `{pid}-` prefix names a dead process", :147), `prepare(agent, session_id)` (:166 — returns `(extra CLI args, AttentionFiles)`; `None` for OpenCode/Terminal/Windows), `parse_state` (:319), `read_state` (:339 — with the mtime+len cache and the 4 KiB tail read), `acknowledge` (:386 — **truncates**, does not delete, so hooks keep appending), `cleanup(&AttentionFiles)` (:403). Its 20 unit tests stay grove-core's; **do not re-test grove-core's logic in grove-gpui.**
    - `grove_core::claude_agents::{Poller, NativeStatus}` (`claude_agents.rs:54,83`): `Poller::new()` (:93) starts its own background thread, `set_wanted(bool)` (:162) parks it, `status_for(root_pid, wt_path)` (:172) answers from a snapshot with a 5s staleness cut-off and a 3-failure permanent-disable. It is a plain `Arc`-backed struct with no UI types — **hold one on the `ActivityStore` for the process lifetime and call it exactly as `tick.rs:124,191-195` does.**
    - `grove_core::tmux::{new_session (tmux.rs:122, takes `args`/`env` slices), pane_pid (:245)}`.
  - **Foreseen candidate spots** (report them if you hit them; none is pre-authorized): (1) anything you want from `grove_core::session::Session` — that type owns an iced-era vt100 parser and **must not be used**; its *methods* (`attention_state` :575, `acknowledge_attention` :583, `root_pid` :511, `bell_count` :933, `current_title` :873) are the specification for Task 2's grove-gpui equivalents, not code to call. (2) `src/gui/dock.rs` is iced-side but has zero iced types — **copy it into `platform/dock.rs`**, do not move it (the iced build still needs it until Plan 10).
- **Behavior questions are answered by reading the iced code, never by guessing.** Canonical oracles for this phase, cited per task:
  - `crates/grove-core/src/attention.rs` — the whole hook pipeline (module doc :1-22 is the architecture statement).
  - `crates/grove-core/src/session.rs:155-175` (id allocated *before* spawn so the state file can be keyed to it), `:222-238` (tmux: extra args appended to the agent's args, `GROVE_STATE_FILE` passed through tmux's env slice), `:240-279` (native: args + `cmd.env`), `:511-522` (`root_pid`), `:530-535` (kill → `attention::cleanup`), `:575-586` (`attention_state`/`acknowledge_attention`), `:873` (`current_title`), `:933` (`bell_count`).
  - `src/gui/update/tick.rs:104-287` — **`refresh_activity`, the whole phase in one function**: the `every 8th tick` gate (:44-47), the `set_wanted` guard (:120-124), the focused rule (:128), bell consume + backwards resync (:133-142), the lazy `tail()` closure (:155-161), `scrolling`/`interacting` (:163-168), the `Signals` build incl. `title` (:169-180), the native-poll arm (:196-218), the hook-state-file arm (:228-243), `was_working` maintenance (:245-255), `newly_waiting` edge (:256-260), tracker pruning (:264), badge diffing (:267-275), pulse start/stop (:276-283), bounce (:284-286).
  - `src/gui/activity.rs:1-222` — constants (:11-27), `ActivityState` (:30), `Signals` (:39-63), `Tracker`+`acknowledge` (:66-100), `classify` (:104-136), `title_working` (:150-159), `matches_working` (:163-171), `matches_waiting` (:179-197). Its **51 unit tests (:224-622)** are the port's acceptance suite.
  - `src/gui/dock.rs:1-63` — badge + `requestUserAttention:` (`NSInformationalRequest` = 10).
  - `src/gui/view/appbar.rs:20-437` — `appbar()` (:20), the segmented `+`/grid combo (:46-149), the attention pill (:151-208), `attention_dropdown` (:310-437); `zen_attention_pill` (:244-305) is **Plan 07**.
  - `src/gui/view/statusbar.rs:17-192` — running count, backend/theme labels, `bypass` chip, toast slot, hint chips, version.
  - `src/app/mod.rs:26-52` — `ToastKind`, `Toast::ttl` (Info 4s / Error 8s), `expired_at`; `:149-160` `set_toast`/`set_error`.
  - `src/gui/update/mod.rs:246-252` (`attention_animation` — 1000ms, `EaseInOut`, auto-reverse, repeat-forever, parked at `false`), `:355-385` (the 60ms/1s cadence gate and the `animating` term), `:619-627` (`ToggleAttentionQueue`/`CloseAttentionQueue`), `:699-707` (`acknowledge_session`), `:710-716` (`activity_state`), `:719-726` (`attention_pulse`), `:728-740` (`waiting_sessions`).
  - `src/gui/update/sessions.rs:210-223` (`on_jump_to_waiting_session` — **snaps to bottom first**, then selects), `:229` (selecting closes the dropdown).
  - `src/gui/update/layout.rs:34-49` (`on_window_focus_changed` — refocus acknowledges the active session).
  - `src/gui/view/common.rs:179-195` (`session_context_title`, `is_in_progress_title`), `src/gui/view/terminal.rs:487-560` (the session bar and its `(tick/5)%3` 3-dot), `:1096-1100` (the 40-tick toast/scrim triangle wave — **the scrim itself is Plan 07**), `src/gui/rows.rs:870-892` (`state_glyph`).
- **Interfaces Plans 03–05 already shipped — consume them, do not re-derive:**
  - `activity::{ActivityState, most_urgent}` (ported verbatim in Plan 05) and `ActivityStore::{state_of, pulse}` — the two `// Plan 06: data source` stub bodies this phase fills. **The signature set does not change**; every Plan 05 call site must keep compiling untouched.
  - `WorkspaceState::acknowledge(SessionId)` with its `// Plan 06: truncates the attention state file` body comment, already called from every focus transition (Plan 05 carried amendment 6). **Fill the body; do not add call sites — find them.** Window-refocus is the one transition Plan 05 could not create (there was no window-focus observer yet); Task 4 adds it.
  - `SessionRegistry`, `SessionId`, `SessionMeta`, `SpawnTarget`, `visible_session_order()` (the attention queue's order — `flatten`-derived, Plan 05 Task 4 Step 2).
  - `entities::animation_clock::{AnimationClock, tick, is_fast, cadence, spinner_frame, SPINNER_FRAMES, cursor_visible, dots, toast_pulse}` — `dots` and `toast_pulse` were ported in Plan 03 and **get their first consumers here**.
  - `icons::{icon, spinner}` (Plan 05 pulled the in-memory SVG `AssetSource` forward), `views::rows::state_glyph`, `theme.rs` token fns, `settings::SettingsState`, `zoom::ZoomState`, `fonts::{UI_FAMILY, MONO_FAMILY}`, `keymap::{SHORTCUTS, GlobalShortcut, binding_for}`.
- **Carried amendments (do not re-derive):**
  1. **The 480ms task is a foreground `cx.spawn` loop with a `Timer`, not a background thread.** It reads and writes entities (`ActivityStore`, `SessionRegistry`, `WorkspaceState`) and calls `cx.notify()`, all of which require the foreground executor. Model it on `AnimationClock`'s own loop (`animation_clock.rs:90-108`). The *blocking* work it triggers is already off-thread inside grove-core: `claude_agents::Poller` owns its own thread; `attention::read_state` is a `stat` plus, only on change, a 4 KiB read. **Do not** wrap `read_state` in a background task — that would reorder signals against the snapshot they were captured with, and spec §4 pins the snapshot semantics ("a synchronously captured `(sessions meta, active_session, window_focused)` snapshot").
  2. **480ms is a period, not a tick multiple.** iced derives it as "every 8th 60ms tick" (`tick.rs:44-47`) because it had exactly one timer. gpui has per-concern tasks (spec §4, "Tick decomposition"), so this is `Timer::after(Duration::from_millis(480))` in its own loop. The observable cadence is identical; **do not** couple it to `AnimationClock::tick`. Record this in the task's doc comment — a reader who greps for `% 8` must find the explanation.
  3. **`Tracker` state lives in the `ActivityStore`, keyed by `SessionId`, and is pruned every pass.** Port `tick.rs:264`'s `retain` — a killed session's tracker must not keep its `WaitingForInput` in the badge count. `SessionId` is stable (Plan 05 Task 2), so the iced build's index-shifting has no analogue here.
  4. **The attention pulse is `with_animation`-free.** Spec §4 maps it to `with_animation`, but that helper animates *an element being rendered*, and this pulse is a scalar read by six call sites (sidebar glyph, appbar pill dot, dropdown glyphs, and Plan 07's tile scrim/zen pill) across two views. Implement `ActivityStore::pulse() -> f32` as a **triangle wave over `Instant`** reproducing `attention_animation`'s observable output — 1000ms half-period, `EaseInOut`, auto-reverse, so a full round trip is ~2s and the value sweeps `0.0 → 1.0 → 0.0` — and return a **constant `0.0` while nothing waits** (`update/mod.rs:719-726` guarantees exactly this, so call sites interpolate unconditionally). TDD the easing: `pulse_at(t0) == 0.0`, `pulse_at(t0 + 1000ms) == 1.0` (within 1e-3), `pulse_at(t0 + 2000ms) == 0.0`, monotone on each half, and `EaseInOut`'s midpoint `pulse_at(t0 + 500ms) == 0.5`. Record the deviation from spec §4 in the module doc.
  5. **The pulse must drive the frame clock, or it will not animate.** `AnimationClock::is_fast`'s `animating` input (`animation_clock.rs:31`, mirroring `update/mod.rs:367-370`) has no source today. Wire `waiting_count > 0` into it via `set_busy_inputs`. Assert the idle-power contract in a test: no sessions waiting + unfocused + no dirty PTYs ⇒ `is_fast == false`.
  6. **Dock is Linux-first by *absence*, not by porting.** `src/gui/dock.rs` is `#[cfg(target_os = "macos")]` with explicit no-op stubs off macOS (`:59-63`) — there is no Wayland/X11 badge API in Grove today, and spec §7 lists the dock badge/bounce under **macOS**. So on Linux the correct, verifiable behavior is **nothing renders and nothing bounces**, on both Wayland and X11, while the waiting *count* still drives the appbar pill. Copy the file verbatim (incl. the `#![allow(unexpected_cfgs)]` and both SAFETY comments), swap "iced drives the GUI on the main thread" for "gpui drives the GUI on the main thread" in the two SAFETY blocks, and gate the `objc` dependency to macOS. **Do not invent a Linux badge.** Task 7 records the macOS badge/bounce rows as MANUAL-on-macOS.
  7. **The appbar's grid toggle and `+` are built but inert.** The segmented combo's *appearance* is conditional on `grid_view` (`appbar.rs:46`), which `WorkspaceState` already carries as a Plan 07 stub field. Render both shapes, dispatch `ToggleGridView` to a logged stub, and check the appearance rows against `grid_view == false` only. Same for the cog (Plan 08) and its upgrade dot (Plan 09, stubbed off).

- **Recorded ambiguities, resolved by reading the oracle:**
  1. **The session header bar (`sess_bar`, `view/terminal.rs:487`) is built in this phase**, not Plan 07. Appendix A's *Attention/activity* row "3-dot `(tick/5)%3`" is an exit-gate row, and the only place that animation exists is the session bar's in-progress context (`terminal.rs:539-551`). The bar also carries the OSC context title, which is this phase's OSC deliverable. Plan 07 reuses the same renderer for grid tile headers; it is written here parameterized by session, not by "the active session".
  2. **"Both platforms" in the master-plan exit gate means Wayland and X11**, per spec §7's Linux row and the Plan 01 Linux matrix spike. macOS is a *deferred verification host*, not a deferred implementation: the dock code ships here and its two rows are recorded MANUAL/macOS for Plan 10's parity pass. State this explicitly in the checklist rather than marking them green from a Linux box.
  3. **Toasts have no floating widget.** Appendix A says "statusbar … toast with kind-dependent TTL", and the iced toast is a `text` in the statusbar row (`statusbar.rs:84-97`), not an overlay. Build it as the statusbar's third slot. `toast_pulse` (`animation_clock.rs:66`) belongs to Plan 07's tile scrim (`terminal.rs:1098`), **not** to the statusbar toast, which does not pulse — leave it unconsumed and say so.
- No `git` commands until Task 7. Do not commit intermediate tasks. The orchestrator runs `./install.sh` and the commit.

---

### Task 1: Wire the zero-setup hook pipeline at spawn

**Files:**
- Modify: `crates/grove-gpui/src/entities/session_registry.rs`, `crates/grove-gpui/src/entities/terminal_session.rs`, `crates/grove-gpui/src/main.rs` (or wherever startup runs before the first spawn)

**Interfaces:**
- Produces: every agent session spawned by grove-gpui carries its Claude `--settings` / Codex `-c notify=` args and a `GROVE_STATE_FILE` env var; `SessionMeta` carries the resulting `AttentionFiles`; startup GCs stale files; kill cleans up.

- [ ] **Step 1: Read the oracle, then read what grove-gpui actually does today**

`crates/grove-core/src/attention.rs:1-22,110-126,157-226` and `session.rs:155-175,222-238,240-279,530-535`. Then read `terminal_session.rs`'s `spawn_tmux`/`spawn_native`: **the agent's own args and env are hardcoded empty** — `tmux::new_session(&name, cwd, INIT_ROWS, INIT_COLS, &agent.program(), &[], &[])` passes `&[]` for both `args` and `env`, and `spawn_native` spawns the login shell with no agent invocation at all. That is the gap this task closes for the attention path. Write down in your report whether `spawn_native` also needs `Agent::invocation()` plumbing (`session.rs:248-252`) or whether Plan 05 deliberately left native agent sessions to a later phase — **do not silently expand scope**; if the native path spawns a bare shell for an agent target, fix only the attention plumbing and report the rest.

- [ ] **Step 2: `SessionMeta` carries the files; the registry owns their lifecycle**

`SessionMeta` gains `attention: Option<grove_core::attention::AttentionFiles>` and `spawned_at: Instant`. `SpawnTarget` is unchanged. In the registry's spawn path, **before** the PTY exists (mirroring `session.rs:158-162`'s ordering comment):

```rust
let attention = grove_core::attention::prepare(target.agent, id.0); // id.0: the SessionId's u64
```

then thread `(extra_args, files)` down: tmux gets `extra_args` appended to the agent's args and `[(STATE_FILE_ENV, state_file)]` as its env slice (`tmux::new_session`'s `args`/`env` parameters, `tmux.rs:122-130`); native gets `cmd.arg(..)` per extra arg plus `cmd.env(STATE_FILE_ENV, ..)`. Killing or dropping a session calls `attention::cleanup(&files)` (`session.rs:530-535`).

**`prepare` is keyed on grove-gpui's `SessionId`, not grove-core's `NEXT_SESSION_ID`** — grove-gpui never constructs a `grove_core::session::Session`. The state-file name is therefore `{our pid}-{our SessionId}.state`, which is exactly the invariant the GC and the cross-run collision argument rely on (attention.rs:110-121): the pid prefix is what makes it safe, not the id's provenance. Say this in the doc comment.

- [ ] **Step 3: Startup GC**

Call `grove_core::attention::cleanup_stale_files()` **once at startup, before any session spawns** (attention.rs:110-112). It is pure garbage collection of dead-pid files; surviving tmux agents from a previous run keep appending to orphan paths that nobody reads and get collected next startup (attention.rs:113-121). Put it beside the existing startup wiring in `main.rs`/`app.rs` with a comment naming the pid-prefix invariant.

- [ ] **Step 4: Fill `WorkspaceState::acknowledge`**

Replace the `// Plan 06: truncates the attention state file` body with the real one, mirroring `update/mod.rs:699-707`: look up the session, call `ActivityStore::acknowledge(id)` (Task 4 — the `Tracker::acknowledge` half) **and** `grove_core::attention::acknowledge(&files.state_file)` (the file half). Both halves, always — the iced comment at :697-698 explains why the file must be truncated too: a stale `needs-you` would resurface the moment the user looks away.

Do **not** add call sites. Plan 05 already placed them (session row click, `SelectSession(n)`, next/prev cycling); Task 4 adds the window-refocus one. Grep for `acknowledge(` and list every call site in your report.

- [ ] **Step 5: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -30
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

Spawn paths are not unit-tested (they need a PTY); Task 7 Step 2 row 2 verifies the generated `--settings` file end-to-end by inspecting `~/.config/grove/attention/`.

---

### Task 2: The signal surface on `TerminalSession`

**Files:**
- Modify: `crates/grove-gpui/src/entities/terminal_session.rs`

**Interfaces:**
- Produces: `title()`, `bell_count()`, `tail_contents(n)`, `output_age()`, `alive()`, `root_pid()`, `snap_to_bottom()` — everything Task 4's `Signals` build needs, and nothing more.

- [ ] **Step 1: Pass-throughs from `GroveTerm`**

`term.rs` already has `title()` (:220), `bell_count()` (:231) and `tail_contents(n)` (:299) — Plan 02 built them precisely for this. Expose them as thin methods. `tail_contents` takes `&mut self` on the term; keep grove-gpui's signature `&mut self` rather than adding interior mutability.

- [ ] **Step 2: `last_output_at` — the field `ingest` never stamped**

`TerminalSession::ingest` currently folds damage and notifies; it does **not** record when output last arrived. Add `last_output_at: Instant` (initialized at spawn, exactly like `session.rs:432`) stamped in `ingest`, and `output_age() -> Duration` as `now.saturating_duration_since(..)` (`tick.rs:145-149`). `last_input_at`/`last_scroll_at` already exist with the right semantics (Plan 04 stamped them "so Plan 06's plumbing matches `session.rs:604-605,642` from the start") — `input_age()`/`scroll_age()` are already public. Use them; do not re-stamp.

- [ ] **Step 3: `alive()` and `root_pid()`**

- `alive()`: `PtyHandle::try_wait()` (`pty.rs:145`) — the equivalent of `SessionStatus::Running`. It takes `&mut self`; cache the result on the session (`exited: bool`) once it flips, since a reaped child cannot come back, and never call `try_wait` after that. A session with **no** PTY (spawn failed, `pty: None`) is **not** alive.
- `root_pid()`: port `session.rs:511-522` exactly — `Backend::Native` reads the live child pid; `Backend::Tmux { name }` returns the pane pid **captured once at spawn** via `grove_core::tmux::pane_pid(name)` (:245), because there is no cheap live handle. Capture it in `spawn` (and store `None` on failure), do not shell out per classification pass.

- [ ] **Step 4: `snap_to_bottom()`**

`on_jump_to_waiting_session` (`sessions.rs:210-218`) snaps the target to the live screen *before* selecting it, deliberately unlike a manual `mod+j/k` switch. If Plan 04's scroll code already has a "snap to bottom" path (`scroll_to(0)`), expose it under this name; do not write a second one.

- [ ] **Step 5: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -30
```

---

### Task 3: Port the classifier (TDD, pure — the phase's correctness core)

**Files:**
- Modify: `crates/grove-gpui/src/activity.rs`

**Interfaces:**
- Produces: `WORKING_RECENT`/`TITLE_STALE`/`SCROLL_QUIET`/`INPUT_QUIET`, `Signals`, `Tracker` (+ `acknowledge`), `classify`, and the three private pattern predicates — all pure, all testable without gpui.

- [ ] **Step 1: Port the code verbatim, comments included**

`src/gui/activity.rs:11-27` (the four constants — every one of their doc comments encodes a decision: `INPUT_QUIET` tracking `WORKING_RECENT` on purpose, `TITLE_STALE`'s hung-agent reasoning), `:39-100` (`Signals`, `Tracker`, `Tracker::acknowledge`), `:104-136` (`classify`), `:150-197` (`title_working`, `matches_working`, `matches_waiting`). `ActivityState` and `most_urgent` are already there from Plan 05 — do not duplicate them, and **do not change their definitions**.

The ordering inside `classify` is the entire contract and must not be "cleaned up": waiting evidence is computed first and gates the title check (`:117-121`), so a working title can never mask `WaitingForInput`; the title asserts `Working` only while `output_age < TITLE_STALE`; recency is discounted by `scrolling`/`interacting`; `waiting` is only *returned* after the output-quiet cut; `Done` requires `was_working && agent != Terminal`. Keep the comment block at `:106-116` verbatim — it is the justification.

`title_working`'s braille frames (`\u{2802} `, `\u{2810} `, with the trailing space) and the "`✳` proves nothing" note are version-pinned facts about Claude 2.1.173 — port the doc comment with them.

- [ ] **Step 2: Port all 51 tests (tests first — write them, watch them fail against an empty `classify`, then paste the code in)**

`src/gui/activity.rs:224-622`. Every test transfers unchanged except the `Agent` import path. Groups, all mandatory: generic rules (:248-278), scroll suppression (:280-302), interaction suppression (:304-359, incl. `interaction_does_not_mask_waiting`), bell handling (:361-390, incl. `focused_session_never_waiting`), per-agent screen fixtures (:392-487, incl. `claude_prose_question_is_not_waiting` — the one that stopped spurious dock bounces — and `terminal_never_done`), the structured title signal (:489-568, incl. `screen_waiting_beats_working_title`, `stale_working_title_does_not_assert_working`, `claude_static_asterisk_title_is_no_answer`), and tracker acknowledgment (:595-622). `most_urgent`'s four tests are already ported.

**A test you cannot make pass is a port error, not a bad test.** If one genuinely cannot hold under the new architecture, STOP and report rather than editing the assertion.

- [ ] **Step 3: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui activity 2>&1 | tail -40
```

Expect ~55 tests in this module. Paste the raw count.

---

### Task 4: The 480ms task — live `ActivityStore`, dock, pulse

**Files:**
- Create: `crates/grove-gpui/src/entities/activity_store.rs` (or grow `activity.rs`'s `ActivityStore` — pick one and say which), `crates/grove-gpui/src/platform/mod.rs`, `crates/grove-gpui/src/platform/dock.rs`
- Modify: `crates/grove-gpui/src/entities/animation_clock.rs`, `crates/grove-gpui/src/views/workspace.rs`, `crates/grove-gpui/Cargo.toml`

**Interfaces:**
- Produces: `ActivityStore` with a live `state_of`, a real `pulse()`, `waiting_sessions()`, `acknowledge(id)`, and the 480ms loop; `platform::dock::{set_badge, request_attention}`.

- [ ] **Step 1: The store's shape and the pulse (TDD the pure parts first)**

```rust
pub struct ActivityStore {
    trackers: HashMap<SessionId, Tracker>,
    waiting: usize,               // cached count; drives the badge + the pulse gate
    pulse_since: Option<Instant>, // Some(t) while waiting > 0 — the pulse's phase origin
    last_badge: usize,            // `Grove::last_badge` (state.rs:151) — diff before calling the dock
    poller: grove_core::claude_agents::Poller,
    _task: Task<()>,
}
```

TDD, no gpui needed (make the phase math a free function over `Instant`s):
- `pulse_at(None, _) == 0.0` (nothing waiting → constant, per carried amendment 4).
- The `EaseInOut` triangle wave: 0.0 at 0ms, 0.5 at 500ms, 1.0 at 1000ms, 0.5 at 1500ms, 0.0 at 2000ms, and 1.0 again at 3000ms (repeat-forever, auto-reverse — `update/mod.rs:246-252`), each within 1e-3; monotone increasing on `[0,1000ms]`.
- Badge diffing: `set_badge` is called only when the count *changes* (`tick.rs:272-275`).
- Bounce edge: `request_attention` fires only on a session **entering** `WaitingForInput` while the **window is unfocused** (`tick.rs:256-260,284-286`) — never on a session that was already waiting, never while focused.

Model these as pure functions (`pulse_at`, `badge_transition(prev, next)`, `should_bounce(newly_waiting, window_focused)`) so they are unit-testable, then call them from the loop.

- [ ] **Step 2: `platform/dock.rs` (carried amendment 6)**

Copy `src/gui/dock.rs:1-63` verbatim into `crates/grove-gpui/src/platform/dock.rs`, keeping `#![allow(unexpected_cfgs)]`, both SAFETY comments (updating "iced drives the GUI on the main thread" → "gpui drives the GUI on the main thread"), the `NS_INFORMATIONAL_REQUEST = 10` constant, the null-check, and the `null_mut()`-clears-the-badge contract. Add to `Cargo.toml`:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc = "0.2"   # same version the iced crate already uses
```

Off macOS both functions are no-ops. **Do not add a Linux implementation.** The module doc must say so and name spec §7.

- [ ] **Step 3: The 480ms loop (carried amendments 1 and 2)**

A `cx.spawn` loop on the store entity: `Timer::after(480ms)`, then one classification pass, forever. Its doc comment explains why it is a period rather than iced's "every 8th tick" and why `read_state` stays on the foreground.

One pass, ported from `tick.rs:111-287` in order:

1. Snapshot synchronously: the registry's sessions (id, agent, meta, entity handle), `WorkspaceState::active_session`, and `window_focused` (spec §4).
2. `poller.set_wanted(any live Claude session)` (`tick.rs:120-124`).
3. Per session:
   - `focused = active_session == Some(id) && window_focused` (`:128`).
   - Bell: `bells < seen` ⇒ **resync** `seen = bells` (the parser was reset — never go silent forever); `bells > seen` ⇒ `seen = bells` and, **only if unfocused**, `bell_pending = true` (`:133-142`).
   - Build `Signals` from Task 2's surface: `alive`, `output_age`, `bell_pending`, `was_working`, `focused`, `scrolling = scroll_age() < SCROLL_QUIET`, `interacting = input_age() < INPUT_QUIET`, `title = alive.then(|| title()).flatten()` (`:144-180`).
   - `tail` stays **lazy** (a closure, `:155-161`): scraping 15 rows takes the term lock, and the two higher-precedence signals discard it for most sessions.
   - Precedence: native poll (`alive && agent == Claude` → `poller.status_for(root_pid, wt_path)`) → `Busy⇒Working`, `Waiting⇒WaitingForInput` **unless focused, then Working**, `Idle⇒Done if was_working else Idle` (`:196-218`). Else the hook state file (`attention::read_state(&files.state_file)`) → `NeedsYou⇒WaitingForInput` unless focused (then `Working`), `Done⇒Done`, `Working⇒Working`, and **`!alive` short-circuits to `classify` before any of it** so a stale `working` from a killed agent still reads `Exited` (`:228-243`). Else `classify(agent, &tail(), &sig)`.
   - `was_working |= (state == Working)`; `!alive` clears `was_working` and `bell_pending`; `focused` clears `bell_pending` continuously ("watching it = continuously acknowledged", `:245-255`).
   - Record the `newly_waiting` edge (`:256-260`).
4. Prune trackers to live ids (`:264`, carried amendment 3).
5. Recount `waiting`; badge-diff; start/stop the pulse (`pulse_since = Some(now)` on 0→n, `None` on n→0 — `:276-283`); bounce if `newly_waiting && !window_focused`.
6. `cx.notify()` only when something observable changed (any state, the waiting count, or an active pulse) — an all-`Idle` pass on a quiet app must not repaint.

- [ ] **Step 4: Window focus, refocus-acknowledge, and the frame clock**

- Observe window activation (gpui's window-activation observer; the skill has the current API — **check it, do not guess**) into `window_focused`. On **regaining** focus, acknowledge the active session immediately (`layout.rs:36-41`), synchronously — spec §4's "attention is never event-driven" applies to *classification*, not to acknowledgment.
- Feed `waiting > 0` into `AnimationClock::set_busy_inputs`'s `animating` term (carried amendment 5), and test the idle-power contract: nothing waiting + unfocused + no dirty PTYs ⇒ `is_fast() == false`.

- [ ] **Step 5: `JumpToWaitingSession` becomes real, and `waiting_sessions()` gets its order**

`ActivityStore::waiting_sessions()` filters `WorkspaceState::visible_session_order()` (Plan 05 Task 4 Step 2) by `state_of(id) == WaitingForInput` — **tree order, not HashMap order** (`update/mod.rs:728-740`); it is the queue the pill, the dropdown and `mod+'` all share, resolved **once per frame** and passed down (`view/mod.rs:58-61` — the iced build had three call sites recomputing it). Plan 05's stub-gated `JumpToWaitingSession` now: first waiting session → `snap_to_bottom()` (Task 2) → `select_session` (which acknowledges) → close the dropdown (`sessions.rs:210-223,229`).

- [ ] **Step 6: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 5: The appbar — pill, dropdown, and the rest of the top strip

**Files:**
- Create: `crates/grove-gpui/src/views/appbar.rs`
- Modify: `crates/grove-gpui/src/views/workspace.rs`, `crates/grove-gpui/src/views/mod.rs`

**Interfaces:**
- Produces: `Appbar` — brand, agent-view control, attention pill, cog; plus the anchored attention dropdown as a workspace layer.

- [ ] **Step 1: The bar** (`appbar.rs:20-44,210-237`)

`APPBAR_H`-tall `BG_STRIP()` strip with a `BORDER()` hairline beneath. Left: `grove` in `UI_BOLD` 14px `MAGENTA()`, inside a container **exactly `sidebar_width` wide** (`:221`) so the brand sits over the rail. Right cluster: view control, optional pill, cog — 4px spacing, 16px horizontal padding, vertically centered.

- [ ] **Step 2: The view control and the cog** (carried amendment 7)

Non-grid (today's only reachable state): a lone 22×22 icon button, `grid` glyph in `FG_MUTE()`, `BG_HOVER()` on hover, 4px radius (`:124-149`). Grid: the segmented combo — `+` (magenta, left corners rounded) │ 1px×14px `BORDER()` hairline │ `grid` (cyan, `BG_HL()` background, right corners rounded), all inside a 5px-radius bordered container (`:46-123`). Both segments dispatch to logged stubs (`+` → Plan 08 SessionLauncher, grid → Plan 07). Cog → Plan 08 Settings stub, with the `GREEN()` dot overlaid top-right only when upgrade state is `Available` — stubbed `None` this phase (Plan 09).

- [ ] **Step 3: The attention pill** (`:151-208`)

Rendered **only** while `waiting` is non-empty. A pill (999px radius, `AMBER()` 1px border, `AMBER()` at 8% background, 14% on hover) containing a dot whose alpha is `1.0 - 0.4 * pulse` and the label `"1 needs you"` / `"{n} need you"` (`:162-166` — the singular/plural switch is exact copy; TDD the label function). Click toggles the dropdown.

- [ ] **Step 4: The dropdown** (`:310-437`)

A 280px-wide `BG_STRIP()` panel, 6px radius, `BORDER()`, anchored under the appbar (`APPBAR_H + 1` from the top, 16px from the right), over a full-window transparent backdrop that dismisses on click (same idiom as Plan 05's agent-menu overlay). One row per waiting session: `state_glyph(state, tick, pulse)` + agent label (11px `FG()`) over `"{project} / {basename(wt_path)}"` (10px `MONO_FAMILY` `FG_MUTE()`), a **3px `AMBER()` left accent bar** stacked over the row, `BG_HOVER()` on hover, click selects the session. Footer: a hairline then the `mod+'` jump hint, rendered per-platform (`⌘` icon + `'` on macOS, `"{mod}+' jump to next"` elsewhere) — the key comes from the `SHORTCUTS` registry row for `JumpToWaitingSession`, never a literal.

Selecting a session closes the dropdown (`sessions.rs:229`); so does `CloseAttentionQueue` and opening any modal (`update/mod.rs:795-801`).

- [ ] **Step 5: The session header bar** (recorded ambiguity 1)

`crates/grove-gpui/src/views/session_header.rs`: a `SESSBAR_H` (36px) `BG_STRIP()` bar above the terminal body with a `BORDER_SOFT()` hairline beneath (`terminal.rs:487-560,480-485`). Contents: session/project label (13px, weight 600, `FG()`), then — only when the branch is non-empty — a `·` separator and the branch (12px `FG_DIM()`), then the OSC context title (12px `FG_DIM()`, middle-truncated at 80 chars). When the session is running **and** the context title matches `is_in_progress_title` (`common.rs:192-195`: case-insensitive `in progress` / `in-progress` / `in_progress`), the title is replaced by the **3-dot animation**: three dots, the one at `dots(tick)` = `(tick/5)%3` in `GREEN()`, the others `FG_MUTE()`, followed by `"in progress"` in `GREEN()` (`terminal.rs:539-551`). TDD `is_in_progress_title` and the phase selection.

Parameterize the renderer by session — Plan 07 reuses it for grid tile headers.

- [ ] **Step 6: OSC titles reach the rows** (Plan 05 deviation 5 closes here)

`session_context_title` (`common.rs:179-190`): take the term's `title()`, drop it when it equals the session's internal label or the agent label (case-insensitive), then run it through Plan 05's `sanitize_ui_text` (OSC titles routinely start with emoji/box-drawing the UI font cannot render — the sidebar and the header must apply the same filter). Wire it into Plan 05's `session_context`/`terminal_context` so sidebar session rows stop showing `None`, and into the dropdown and the header. The iced `cached_context` memo (`rows.rs:748`) exists because the sanitize ran per frame per row; port it **only** if a profile shows it matters — otherwise record the omission at the call site, as Plan 05 did for the PTY-theme memo.

- [ ] **Step 7: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

Appearance is not unit-testable — Task 7 owns it. Do not claim it green from a compile.

---

### Task 6: The statusbar and toasts

**Files:**
- Create: `crates/grove-gpui/src/views/statusbar.rs`, `crates/grove-gpui/src/entities/toast.rs`
- Modify: `crates/grove-gpui/src/views/workspace.rs`

**Interfaces:**
- Produces: `Statusbar`; `Toast`/`ToastKind`/`ToastState` with a kind-dependent TTL task and `set_toast`/`set_error`.

- [ ] **Step 1: Toast state (TDD)** (`src/app/mod.rs:26-52,149-160`)

Port `ToastKind { Info, Error }`, `Toast { message, kind, created }`, `Toast::ttl` (Info 4s, Error 8s) and `expired_at(now)` with their existing tests (`app/mod.rs:721-757`). Expiry becomes its **own task** (spec §4's tick decomposition): setting a toast spawns a `Timer::after(ttl)` that clears it if it is still the same toast; a newer toast supersedes an older one and restarts the timer. Test that superseding does not let the old timer clear the new toast — the iced build got this free from polling `expired_at`, a timer does not.

`toast_pulse` stays unconsumed (recorded ambiguity 3); leave its doc comment pointing at Plan 07.

- [ ] **Step 2: The bar** (`statusbar.rs:17-192`)

`STATUS_H`-tall `BG_STRIP()` strip with a `BORDER_SOFT()` hairline **above** it (note: appbar's hairline is below, this one above), 16px horizontal padding. Left group, 14px spacing, all 10px `MONO_FAMILY`:
- a dot (`GREEN()` if any session is Running, else `FG_MUTE()`), the running count in `FG_DIM()`, the label `RUNNING` in `FG_MUTE()`;
- `BACKEND` + `tmux`/`native`;
- `THEME` + the store's theme name (default `tokyonight`);
- when skip-permissions is on, a `bypass` keycap chip in `YELLOW()`.

Then 24px, the toast slot (`RED()` for Error, `GREEN()` for Info, empty otherwise), then flexible space. Right: the `palette` chip, 14px, the `shortcuts` chip, 14px, `v{CARGO_PKG_VERSION}` in `FG_MUTE()`. Each chip is a keycap (⌘+key icon on macOS, `"{mod}+{key}"` text elsewhere) plus a muted label, with the key pulled from the `SHORTCUTS` registry rows for `NewSession` and `ShortcutOverlay` — **never a literal**, and `FG()` on hover. Both chips dispatch to Plan 08 stubs.

- [ ] **Step 3: Assemble the chrome**

`Workspace::render` becomes `column![appbar, row![sidebar, divider, body], statusbar]`, with the body itself `column![session_header, terminal_view]`, and the attention dropdown pushed as a layer above everything when open. The waiting queue is resolved **once** here and handed to the appbar and the dropdown (Task 4 Step 5). Zen's chrome-hidden branch and its floating pill stay Plan 07 — leave the `chrome_visible` field read but always true, with a `// Plan 07` comment.

Whatever the chrome costs in height must come out of the terminal's available rows; the element derives its dims from its own bounds in `prepaint` (Plan 04 amendment 7), so this needs no PTY-dim wiring — **verify it visually in Task 7 anyway**, since this is the first phase where chrome takes vertical space.

- [ ] **Step 4: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 7: Verification and the manual parity checklist

**Files:**
- Modify: `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` (row 06 → done)

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

Expected: everything green, the Plan 03 metric selftest still prints its `cell_w=7.5… OK` line, the activity module reports ~55 tests, and **`git status` reports no changes at all** under `src/`, `crates/grove-core/`, `crates/grove-terminal/`. Read the raw output.

- [ ] **Step 2: MANUAL — the spec Appendix A **Attention/activity** rows, on Wayland *and* X11 (human, real desktop)**

```bash
# Wayland, then X11 — run both:
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui
WAYLAND_DISPLAY= PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui   # forces X11
# and, side by side, the installed iced build:
~/.local/bin/grove
```

Report each row pass/fail **per backend**. **Do not claim any of these yourself.** Rows 1–12 are spec Appendix A → *Attention/activity*, verbatim and in order; 13–17 are the appbar/statusbar clauses of *Screens/layout* plus the two *System* clauses this phase owns.

1. **480ms cadence.** A background agent's state changes land within ~half a second, and the cadence is 480ms whether the window is focused or not (it is its own timer, not the frame clock).
2. **Precedence: native poller > hook state file > screen-scrape.** With a live Claude session: `~/.config/grove/attention/{pid}-{id}.state` exists and is 0600, the generated `{pid}-{id}.claude-settings.json` is valid JSON declaring Notification/Stop/UserPromptSubmit, a Codex session gets `-c notify=[…]` and no settings file, and an OpenCode/plain-terminal session gets neither. Killing the `claude agents --json` support (or using an agent without it) demotes cleanly to the hook file, and a session with no state file at all still classifies from the screen.
3. **Focused never `WaitingForInput`.** A permission prompt on the *visible* session shows Working/Done, never the amber waiting glyph; leaving it and coming back re-checks.
4. **Bell diff with backwards-reset resync.** A BEL on an unfocused session flags waiting once; a decorative BEL during active output does not; and a session whose parser resets does not go permanently bell-deaf.
5. **Scroll/input quiet windows.** Scrolling or typing in a Done session does not flip it back to Working (3s / 2s windows), but a genuinely working agent still shows Working throughout because its marker/title is visible.
6. **OSC-title working marker with 60s staleness.** A running Claude turn shows Working from its braille title even with a quiet PTY; a hung agent with a frozen title stops asserting Working after 60s; `✳` alone never asserts anything.
7. **Done only for non-Terminal agents.** A home terminal never shows the green check after you type in it; an agent that finished a turn does.
8. **Acknowledge on focus/refocus truncates the state file.** Selecting a waiting session clears its glyph *and* empties its `.state` file (`wc -c` it); re-focusing the window acknowledges the visible session the same way; the file still **exists** afterwards (truncated, not deleted) and later hooks still land.
9. **Dock badge + one bounce per enter-while-unfocused.** **Linux (both backends): MANUAL-verified as a no-op** — nothing renders anywhere, nothing bounces, and the waiting count still drives the appbar pill correctly. **macOS: MANUAL-on-macOS, deferred to Plan 10's parity pass** (see recorded ambiguity 2) — the code ships here; badge = waiting count, cleared at zero, exactly one bounce per session entering waiting while unfocused, none while focused.
10. **Amber pulse, 1s auto-reverse.** The sidebar waiting glyph and the appbar pill dot dim and brighten in lockstep on a ~2s round trip, never disappearing (layout must not shift), and the pulse **stops completely** when the last waiting session is acknowledged.
11. **12-frame spinner every 3 ticks.** A working session's sidebar spinner turns at the same rate as the iced build's, side by side.
12. **3-dot `(tick/5)%3`.** A session whose OSC title says "in progress" shows the three-dot walk plus the green "in progress" label in the session header, at iced's rate.
13. **Appbar.** Brand over the rail at the current sidebar width; the lone grid toggle in non-grid view; the cog; the attention pill appears only while something waits, reads "1 needs you" / "n need you", and toggles the dropdown.
14. **Attention dropdown.** Anchored under the appbar's right edge, 280px wide, one row per waiting session in **tree order**, each with its glyph, agent label, `project / worktree` subtitle and 3px amber accent; clicking a row jumps to that session **and snaps it to the bottom**; the backdrop dismisses; the footer shows the real `mod+'` binding; `mod+'` itself cycles to the next waiting session.
15. **Statusbar.** Running count and its dot, `BACKEND tmux|native`, `THEME <name>`, the `bypass` chip when enabled, the version, and the palette/shortcuts chips showing the registry's real keys.
16. **Toast with kind-dependent TTL.** An info toast clears after 4s, an error after 8s, a newer toast replaces an older one immediately and gets its own full TTL.
17. **System: attention stale-file GC at startup + idle power.** Killing Grove mid-session leaves `.state` files; the next start deletes exactly the dead-pid ones and leaves a concurrently-running Grove's alone. With nothing waiting, the window unfocused and no PTY output, the app is at the 1s cadence (no busy loop) — check CPU with `top`; with an agent working it classifies at ~480ms and paints smoothly.

Rows explicitly **deferred** and not checked here (record as deferred, not failed): the grid tile waiting-scrim and its 40-tick pulse, the tile "respond" chip, the zen floating attention pill, per-tile session headers → **Plan 07**; every modal behind the cog, the `+`, and the two statusbar chips, plus `gpui-component` text inputs → **Plan 08**; the upgrade dot's real state, telemetry, quit paths and tmux sidecar reattach discovery → **Plan 09**; the macOS dock badge/bounce rows (9) → **Plan 10 on a macOS host**; the screenshot sweep and the measured idle-power comparison → **Plan 10**.

- [ ] **Step 3: `./install.sh`** — the orchestrator runs this.

```bash
./install.sh 2>&1 | tail -20
```

Expected: the release build + install of the **iced** `grove` binary still succeeds, untouched by this phase.

- [ ] **Step 4: Update the master plan and commit** — the orchestrator runs this.

Mark row 06 `done` in `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` with a one-line note recording: whether any grove-core amendment had to be authorized (expected: none), the gpui window-activation API actually used, whether `cached_context`'s memo was ported or omitted, the state of Task 1 Step 1's `spawn_native` finding, and any Appendix A row that came back FAIL or MANUAL-deferred.

```bash
git add crates/grove-gpui docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md
git commit -m "feat(gpui): attention pipeline, 480ms classifier, appbar/statusbar/toasts"
```

**Exit gate met when:** the spec Appendix A Attention rows above are signed off by a human as pass or explicitly-deferred **on both Wayland and X11**, the classifier's ported test suite and the pulse/badge/bounce/TTL unit tests are green (raw output pasted), grove-gpui builds/tests/clippy clean on 1.95, the iced app and both existing crates are provably untouched and still build on the default toolchain, and `./install.sh` is green.
