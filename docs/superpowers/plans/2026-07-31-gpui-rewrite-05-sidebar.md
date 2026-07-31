# gpui Rewrite Plan 05: Sidebar tree + WorkspaceState sync

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. This is **production** code: the workspace clippy denies apply (`unwrap_used`/`expect_used`), superpowers:test-driven-development governs every pure helper (tests before implementation, red before green), and superpowers:verification-before-completion governs every "done" claim — read raw command output, never a summary line. Also load the `gpui-development` skill before writing any gpui code; training-data gpui is stale and this rev is pinned.

**Goal:** Turn the one hardcoded session Plan 04 put on screen into **many sessions under one owner**, and render Grove's real left rail: a project → worktree → session tree with the 3-mode expand cycle, per-row hover actions, the 5s off-thread git suffix, the pinned TERMINALS section, the drag-resizable divider, and the data-carrying keyboard selection (`mod+1..9`, next/prev, jump-to-waiting, close-focused). Selection stops being a pair of drifting sync functions and becomes one entity — `WorkspaceState` — that every view reads and nobody else mutates. The two items Plan 04 deferred to this phase (per-project pinned **content** themes and the project-themes toggle invalidation) land at their markers.

Exit gate (master plan row 05): **Sidebar checklist rows green** (spec Appendix A → *Screens/layout*, sidebar clauses; enumerated verbatim in Task 7 Step 3); `./install.sh` green; one commit.

**Out of scope — do not build it here.** Grid/tiles, zen, the worktree panel, the terminal *tab* body, Agent/Panel focus routing (Plan 07). Appbar, statusbar, toasts, the dock badge, the 480ms activity classification task and the attention hook pipeline (Plan 06) — this phase defines the sidebar-facing interface for activity/attention and **stubs the data source**. Every modal, every text input, `gpui-component`, and therefore the AddProject/RemoveProject/Archive/AgentPicker/SessionLauncher flows a hover action would open (Plan 08) — hover actions dispatch to a logged stub, but the rows, their hit zones and their hover behavior are built for real here.

**Architecture (new/changed files only):**

```
crates/grove-gpui/
  src/entities/
    workspace_state.rs   NEW. THE single owner of selection: sessions map,
                         active_session, proj_idx/wt_idx, terminal_focused,
                         active_terminal, focused_pane/grid/zen fields (Plan 07
                         stubs). All transitions are methods here.
    project_tree.rs      NEW. projects (from SettingsState.store) + worktrees +
                         wt_cache with a generation guard; the 5s git-state poll.
    session_registry.rs  NEW. SessionId -> (SessionMeta, Entity<TerminalSession>)
                         plus the home-terminal vec. Spawn/kill/lookup.
  src/activity.rs        NEW. ActivityState + most_urgent, ported from
                         src/gui/activity.rs:30-36,201-222. Plan 06 fills the
                         classifier; this phase ships an Idle-only store.
  src/icons.rs           NEW. in-memory SVG AssetSource branch + `icon()` /
                         `spinner()` (see Constraint 5 — pulled forward from
                         Plan 06 because the sidebar cannot render without it).
  src/views/
    sidebar.rs           NEW. the tree view: header, uniform_list of flattened
                         rows, hover actions, agent-menu overlay, empty states,
                         docked TERMINALS header, divider drag.
    rows.rs              NEW. row renderers + the pure row helpers
                         (worktree_shows_branch, sanitize_ui_text, contexts).
    workspace.rs         MODIFIED: sidebar placeholder -> Sidebar; body renders
                         the ACTIVE session; data-carrying action dispatch.
    terminal_view.rs     MODIFIED: takes its session by handle instead of
                         spawning its own.
  src/terminal_element.rs MODIFIED: the `// Plan 05: project theme override`
                         marker becomes a real per-project theme resolution.
  src/entities/terminal_session.rs MODIFIED: `spawn` gains a cwd/agent/meta
                         argument; the `Plan 05` comments are resolved.
```

**Tech stack additions:** none. No new dependencies, no new git revs. Pins unchanged.

## Global Constraints

- Branch: `gpui-rewrite`. Toolchain regime is **identical to Plans 03–04** and is not re-litigated:
  - grove-gpui builds/tests/clippy only via `PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 -p grove-gpui`.
  - Bare `cargo build` / `cargo test` (default-members, rustc 1.94.1) must keep working untouched for `grove`, `grove-core`, `grove-terminal`. Never run `--workspace`.
  - clippy for grove-gpui runs **`--no-deps`** (Plan 03 carry-forward): `cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings`.
  - `rustfmt --edition 2021` on **touched files only**.
- **Pins are law.** ZED_REV `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba`; alacritty fork `4c129667ce56611becdc82de6e28218c80e2e88f`. No `[patch]`, no `gpui-component`.
- **Constraint 3 — grove-core and the iced app are read-only.** No edits under `src/`, `crates/grove-core/`, or `crates/grove-terminal/`.
  - Plan 04's amendment protocol applies unchanged: if a genuinely **UI-free** helper must be exposed from grove-core to avoid duplicating domain logic, **STOP and report**; the orchestrator authorizes the amendment (this is exactly how `PtyHandle::take_receiver()` landed in Plan 04). Do not edit grove-core on your own judgement, and do not "share" a type by moving it.
  - **Foreseen candidate spots** (list them in your report if you hit them; none is pre-authorized):
    1. `crate::app::path_basename` (iced-side, `src/app/mod.rs`) is used by the sidebar for non-main worktree names. It is three lines of path arithmetic — **reimplement it in grove-gpui with tests**, do not move it.
    2. `grove_core::git::{is_repo, list_worktrees, worktree_git_state, git_state_suffix}` and `grove_core::storage::Store::{active_projects (storage.rs:174), archived_projects (:183), archived_count (:188)}` are already public and UI-free — **use them as-is**; needing nothing here is the expected outcome.
    3. If the sidebar needs a session's *agent kind* or *worktree path* in a shape `grove_core::agent::Agent` does not already provide, that is a sign grove-gpui's own `SessionMeta` (Task 2) is under-specified — fix `SessionMeta`, not grove-core.
- **Behavior questions are answered by reading the iced code, never by guessing.** Canonical oracles for this phase, cited per task:
  - `src/gui/view/sidebar.rs:1-471` — the whole view: `sidebar()` (:88), `tree_head()` (:141), `tree_view()` (:225), `visible_session_order`/`tree_session_order` (:386,:392), `open_agent_menu_top` (:421), the docked TERMINALS header (:114-129), the `is_repo` 5s memo (:26-54).
  - `src/gui/rows.rs` — `project_row` (:59), `worktree_shows_branch` (:249), `worktree_row_height` (:268), `worktree_row` (:295), `session_row` (:467), `terminal_row` (:575), `home_terminals_header` (:643), `cached_context`/`terminal_context`/`session_context` (:748,:778,:790), `sanitize_ui_text` (:809), `state_glyph` (:870).
  - `src/gui/activity.rs:30-36` (`ActivityState`), `:201-222` (`most_urgent`/`urgency_rank`).
  - `src/gui/update/sessions.rs:14-50` (collapse-cycle + project/worktree click), `:90-122` (home-terminal select/close), `:225-310` (session select/kill), `:365-405` (cycling), `:460-480` (spawn tail).
  - `src/gui/update/mod.rs:1121-1242` — `switch_active_project` (:1121), `leave_terminal_tab` (:1139), `sync_wt_to_session` (:1143), `sync_session_to_wt` (:1164), `worktrees_for_project` (:1185), `worktree_has_sessions` (:1195), `project_has_sessionful_worktree` (:1202), `apply_tree_expand` (:1213); plus `activity_state` (:710), `attention_pulse` (:719), `waiting_sessions` (:728), `maybe_poll_git_state` (:1251), `visible_worktree_paths` (:1308), `ensure_wt_cached` (:1326), `rebuild_wt_cache` (:1351).
  - `src/gui/update/layout.rs:105-160` — divider drag start/move/end incl. the 350ms double-click reset and the grab-offset; `:473` `persist_sidebar_width`.
  - `src/gui/metrics.rs:7-22` (ROW_H 28, RAIL_W 320, SIDEBAR_MIN_W 220, WORKSPACE_MIN_W 400, SESSBAR_H 36, SIDEBAR_DIVIDER_W 6), `:244-251` `clamp_sidebar_width`.
  - `src/app/theme_picker.rs:51-128` — `project_themes_enabled`, `set_project_themes_enabled`, `project_theme_override`; `src/gui/view/terminal.rs:33-73` — `pty_theme_for` and the per-frame memo.
  - `src/gui/widgets/primitives.rs:195-220` — `sidebar_empty_copy`'s two distinct empty states.
  - `crates/grove-core/src/storage.rs:71-75` (`Project::theme`), `:151-155` (`Store::project_themes_enabled`), `:174-189` (active/archived project iterators).
- **Interfaces Plans 03–04 already shipped — consume them, do not re-derive:**
  - `TerminalSession` (`entities/terminal_session.rs`) with `Backend::{Tmux{name}, Native}` and the full readout surface (`snapshot`, `cursor`, `display_offset`, `history_size`, `app_cursor`, `mouse_mode`, `encoding`, `input_age`, `scroll_age`, `send`, `resize`, `scroll`, `scroll_lines`, `scroll_page_lines`, `click`, `selection_text`, `dims`). Its `spawn(cx)` is the hardcoded single spawn this plan replaces.
  - `TerminalElement` + `views/terminal_view.rs` (`TerminalView::new(clock, cx)`), `terminal::colors::{resolve, resolve_pair}`, `terminal::{keys, mouse, drop, clipboard}`.
  - `keymap.rs`: `SHORTCUTS`, `GlobalShortcut` (incl. `SelectSession(usize)`, `NextSession`, `PrevSession`, `JumpToWaitingSession`, `CloseFocusedSession`, `ScrollHalfPage(bool)`), `actions!` list, `bindings()`, `binding_for` — which returns `None` for `SelectSession`/`GridMove`/`GridSwap` (Plan 03 deviation 3). **Task 6 is where the data-carrying half gets its dispatch.**
  - `settings::SettingsState` (Global; `store: grove_core::storage::Store`, `update()` with the 250ms debounce, `flush_now`), `theme.rs` token fns + `*_of(&Theme)` variants, `zoom::ZoomState`, `entities::animation_clock::{AnimationClock, spinner_frame, SPINNER_FRAMES, cursor_visible, dots}`, `fonts::{MONO_FAMILY, UI_FAMILY, CELL_W, CELL_H, FONT_SIZE}`, `assets::Assets`.
- **Carried amendments (do not re-derive):**
  1. **`WorkspaceState` is a gpui entity, not a Global.** Spec §2 lists it under `entities/` ("workspace_state (selection/focus/grid/zen/panel — single owner)") and spec §4 names it the single owner. Entity, because views must `cx.observe` it to repaint; globals do not notify. `SettingsState`/`ZoomState`/theme stay globals as Plan 03 built them.
  2. **The sidebar's row list is a `uniform_list`** over a **flattened** `Vec<TreeRow>` with a scroll handle stored in the view (gpui-development skill; findings' list guidance). Do not build a `Vec<div>` per frame — Grove trees reach hundreds of rows. Rows are *not* uniform in height in the iced build (`worktree_row_height` is `ROW_H` or `ROW_H + 14`, `rows.rs:268`); resolve this by giving `uniform_list` the **`ROW_H` = 28 uniform height** and rendering the branch chip *inside* the 28px row for worktrees that show a branch, **or**, if that visibly diverges from iced side-by-side, fall back to a plain `div` column inside a scrollable and record the reason. Decide by looking at both builds — and record the decision in the flattening module's doc comment and in your report. Whichever way it lands, `open_agent_menu_top`'s pixel math (`sidebar.rs:421-470`) must be recomputed from the *same* height function, or the overlay lands on the wrong row.
  3. **Attention/activity is stubbed, not faked.** `activity.rs` ports the `ActivityState` enum and `most_urgent` verbatim (they are pure), and exposes a `ActivityStore` entity whose `state_of(SessionId) -> ActivityState` returns `Idle` for everything and whose `pulse() -> f32` returns `0.0`. **Plan 06 fills both** (480ms classifier, hook state file, native poller, the 1s auto-reverse pulse). Every call site reads through the store — no view may branch on "attention isn't implemented yet". Mark the two stub bodies with `// Plan 06: data source` and nothing else.
  4. **The SVG icon `AssetSource` moves from Plan 06 to this phase.** `assets.rs`'s doc comment says "Plan 06 adds it", but the sidebar's chevrons, git glyph, plus button, `main` tag, hover-action icons and the 12-frame spinner are Appendix A sidebar content. Pull it forward: `src/gui/icons.rs` generates its SVGs as **strings in memory** (`svg_for(name)`, icons.rs:25-42) rather than shipping files, so the gpui side needs an in-memory branch in `AssetSource::load` keyed on a `grove-icon://<name>` style path — exactly what spec §6 prescribes ("existing single-color generated SVGs served from an in-memory `AssetSource`, tinted via `text_color`"). Update `assets.rs`'s comment to say Plan 05 landed it.
  5. **`sync_wt_to_session` / `sync_session_to_wt` are deleted, not ported.** Spec §4 names them the drift surface this phase removes. Their *observable outcomes* survive as `WorkspaceState` methods (Task 1); their bidirectional shape does not. Any place you are tempted to write "…and then sync back" is a bug.
  6. **`acknowledge_session` stays a synchronous call on every focus transition** (spec §4: "Attention is never event-driven"). This phase creates three such transitions — session row click, `SelectSession(n)`, next/prev cycling — and each must call `WorkspaceState::acknowledge` inline. In this phase `acknowledge` truncates nothing (Plan 06 owns the state file); it exists so Plan 06 has one call site set to fill, not five to find. Port the *call sites* now.
- **Recorded ambiguity, resolved by reading the oracle:** Appendix A says "archived-projects row". The iced sidebar has **no** archived-projects row — `tree_view` iterates `store.active_projects()` (`sidebar.rs:242`, `storage.rs:174`) and archived projects surface only as the second of `sidebar_empty_copy`'s two empty states ("All projects archived" / "Restore one from Settings → Archived projects.", `widgets/primitives.rs:200-216`); the actual list lives in the Settings → ArchivedProjects modal, which is Plan 08. **This phase satisfies the row by shipping the `active_projects` filter plus both empty-state copies**, and Task 7's checklist states it that way.
- No `git` commands until Task 7. Do not commit intermediate tasks. The orchestrator runs `./install.sh` and the commit.

---

### Task 1: `WorkspaceState` — the single owner (TDD, pure transitions)

**Files:**
- Create: `crates/grove-gpui/src/entities/workspace_state.rs`
- Modify: `crates/grove-gpui/src/entities/mod.rs`

**Interfaces:**
- Produces: `WorkspaceState`, a gpui `Entity` owning **all** selection state, with every transition as a method. Tasks 2–7 and Plans 06–09 read it; nothing else mutates it.

- [ ] **Step 1: Read the oracle before writing anything**

`src/app/mod.rs:78-146` (the `App` struct — note `proj_idx`:82, `wt_idx`:83, `active_session`:86, `home_terminals`:93, `active_terminal`:95, `chrome_visible`:120) and `src/gui/state.rs` (the `Grove` struct — `terminal_focused`:108, `collapsed`:50, `collapsed_wt`:53, `tree_expand`:56, `terminals_collapsed`:118, `sidebar_width`:154, `hovered_wt`:99), then the transition functions listed in Global Constraints (`update/mod.rs:1121-1242`, `update/sessions.rs:14-50`). **There is no `WorkspaceState` type in the iced code** — it is this rewrite's consolidation of state split across two structs and mutated from a dozen `update` handlers. Write down, in the module doc comment, which iced field each of your fields replaces and at which line.

- [ ] **Step 2: The shape**

```rust
pub struct WorkspaceState {
    // selection — spec §4's single-owner set
    active_session: Option<SessionId>,      // App::active_session (app/mod.rs:86)
    proj_idx: usize,                        // App::proj_idx (:82)
    wt_idx: usize,                          // App::wt_idx (:83)
    terminal_focused: bool,                 // Grove::terminal_focused (state.rs:108)
    active_terminal: Option<usize>,         // App::active_terminal (:95)
    // tree presentation — Grove::{collapsed, collapsed_wt, tree_expand, terminals_collapsed}
    collapsed: HashSet<usize>,
    collapsed_wt: HashSet<(usize, usize)>,
    tree_expand: TreeExpand,
    terminals_collapsed: bool,
    // transient row affordances — Grove::{hovered_wt, open_agent_menu, pending_kill*}
    hovered_wt: Option<(usize, usize)>,
    open_agent_menu: Option<(usize, usize)>,
    pending_kill: Option<SessionId>,
    pending_kill_terminal: Option<usize>,
    // layout
    sidebar_width: f32,                     // Grove::sidebar_width (state.rs:154)
    // Plan 07 owns these; declared so the single-owner rule is not violated later.
    focused_pane: FocusedPane,
    grid_view: bool,
    zen: bool,
}
```

`SessionId` is grove-gpui's own stable key (Task 2), **not** a `Vec` index: the iced build's `Option<usize>` index-shifting dance (`update/sessions.rs:270-280`, `:109-113`) exists only because sessions live in a `Vec`, and reproducing it here would import the exact class of bug this rewrite removes. Home terminals keep a positional index because their row order *is* their identity (`app/terminals.rs:61-84`).

`TreeExpand` is ported verbatim from `src/gui/state.rs:27-44`, cycle and all (`Collapsed → SessionsOnly → All → Collapsed`).

- [ ] **Step 3: TDD the transitions — tests first, red before green**

These are pure functions over the struct plus a borrowed tree/registry snapshot; they must be testable without a gpui `App`. Design them as `&mut self` methods taking `&TreeSnapshot` (projects, per-project worktrees, sessions-by-worktree), so the tests construct fixtures directly. Each test cites its oracle line:

| behavior | oracle |
|---|---|
| `select_session(id)`: sets `active_session`, clears `terminal_focused`, clears both pending-kill arms, calls `acknowledge`, and moves `proj_idx`/`wt_idx` to the owning project+worktree | sessions.rs:225-246 + mod.rs:1143-1156 (`sync_wt_to_session`'s outcome) |
| `select_worktree(p, w)`: sets `proj_idx`/`wt_idx`, toggles that worktree's collapse, and re-points `active_session` at the **first** session in that worktree — leaving it alone when it is already there, clearing it to `None` when the worktree has none | sessions.rs:35-50 + mod.rs:1164-1183 (`sync_session_to_wt`'s outcome) |
| `select_project(p)`: toggles that project's collapse, switches the active project | sessions.rs:22-33 + mod.rs:1121-1130 |
| `select_home_terminal(i)`: bounds-checked, sets `terminal_focused`, clears both kill arms | sessions.rs:90-103 |
| `close_home_terminal(i)`: shifts `pending_kill_terminal` across the removal (`==i → None`, `>i → i-1`) | sessions.rs:109-113 |
| `cycle_session(next: bool)`: walks `visible_session_order` (Task 4), wraps, and from the terminal tab returns to the last agent session first | sessions.rs:365-405 |
| `apply_tree_expand()`: `All` clears both sets; `Collapsed` collapses every **active** project (archived skipped — the sets are keyed on TRUE indices); `SessionsOnly` collapses projects with no sessionful worktree and worktrees with no sessions | mod.rs:1213-1242 |
| `toggle_collapse_all()`: advances `tree_expand` then applies it, clearing the agent menu and both kill arms | sessions.rs:14-20 |
| removing a session clears `active_session` iff it was the removed one, and otherwise leaves it pointing at the *same* session | sessions.rs:270-280 (the index dance this design removes) |

Also assert the negative that names the phase: **no method both writes `active_session` and re-reads it to fix up `wt_idx` in a second pass** — one direction only, per spec §4.

- [ ] **Step 4: Implement, then wire the entity**

`WorkspaceState::new(store: &Store)` seeds `sidebar_width` from `store.sidebar_width` clamped by Task 5's `clamp_sidebar_width`, `tree_expand` from its default, and everything else empty. Mutations call `cx.notify()`; the entity is created in `Workspace::new` and handed to `Sidebar`.

`acknowledge(&mut self, id: SessionId)` is a real method with a `// Plan 06: truncates the attention state file` body comment and no other content. It is called from every focus transition above (carried amendment 6).

- [ ] **Step 5: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui workspace_state 2>&1 | tail -30
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 2: The session registry — many `TerminalSession`s with metadata

**Files:**
- Create: `crates/grove-gpui/src/entities/session_registry.rs`
- Modify: `crates/grove-gpui/src/entities/terminal_session.rs`, `crates/grove-gpui/src/views/terminal_view.rs`

**Interfaces:**
- Produces: `SessionId` (opaque, stable, monotonic), `SessionMeta { id, project: String, wt_path: String, agent: Agent, label: String, spawned_at }`, and `SessionRegistry` — a gpui entity holding `IndexMap<SessionId, (SessionMeta, Entity<TerminalSession>)>` plus `home_terminals: Vec<(SessionMeta, Entity<TerminalSession>)>` and `home_terminal_seq`. `TerminalSession::spawn` gains an explicit target.

- [ ] **Step 1: Read the oracle**

`src/app/mod.rs:85-112` (why `home_terminals` and `wt_terminals` live *outside* `sessions` — they must never appear as tree/activity rows; the comment at :87-92 is the contract), `src/app/spawn.rs:120-210`, `src/app/terminals.rs:14-84`, and grove-gpui's own `entities/terminal_session.rs:76-190` (`spawn`, `spawn_tmux`) — which currently hardcodes one target and honours `GROVE_GPUI_SESSION_CWD`.

- [ ] **Step 2: Generalize `TerminalSession::spawn`**

`spawn(target: SpawnTarget, cx) -> Self` where `SpawnTarget { cwd: String, agent: Agent, tmux_name_hint: Option<String> }`. The tmux command construction (`tmux -L <SOCKET> -u attach-session -t =<name>`, `TERM=xterm-256color`, `LC_ALL=en_US.UTF-8`) and the native-shell fallback are already correct — **do not touch them**, only the argument plumbing. `GROVE_GPUI_SESSION_CWD` stays as the manual-checklist escape hatch but is now only the *default* when the registry has no projects.

Home terminals are `SpawnTarget { cwd: home_dir(), agent: Agent::Terminal, .. }` and go in the `home_terminals` vec, never the map. Spec's "pinned TERMINALS section (always ≥1 home terminal)" is enforced here, mirroring `app/terminals.rs:21-30,61-84`: the section lazily spawns its first shell, and closing the last one **immediately respawns a fresh shell**.

- [ ] **Step 3: The registry, TDD where it is pure**

Tests first for the parts that need no PTY: id monotonicity and stability across removals; `sessions_in_worktree(path)` returning insertion order; `by_project(name)`; the home-terminal close/shift/respawn rule; `label` sequencing (`terminal 1`, `terminal 2`, … — `app/mod.rs:96-101`, the label is internal and stripped from the displayed title). The spawning paths are exercised by Task 7's manual checklist, not by unit tests.

- [ ] **Step 4: `TerminalView` takes its session**

`TerminalView::new(session: Entity<TerminalSession>, clock, cx)`. The view no longer spawns anything. Its selection state, drag state and the `pty_press_focused` flag stay per-view — **and the flag becomes load-bearing now** (`terminal_view.rs:52` says exactly this): a click that switches focus from the sidebar or another session must not move the caret.

- [ ] **Step 5: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -30
```

---

### Task 3: Icons, activity stub, and the pure row helpers (TDD)

**Files:**
- Create: `crates/grove-gpui/src/icons.rs`, `crates/grove-gpui/src/activity.rs`, `crates/grove-gpui/src/views/rows.rs`
- Modify: `crates/grove-gpui/src/assets.rs`

**Interfaces:**
- Produces: `icons::{icon(name, size, color), spinner(size, color, tick)}`; `activity::{ActivityState, most_urgent, ActivityStore}`; and the pure row helpers `worktree_shows_branch`, `row_height`, `sanitize_ui_text`, `session_context`, `terminal_context`, `sidebar_empty_copy`, `path_basename`.

- [ ] **Step 1: The in-memory icon `AssetSource` (carried amendment 4)**

Port `src/gui/icons.rs`'s `svg_for(name)` sprite table verbatim (it is a pure `&str -> String`), then extend `assets::Assets::load` with a branch that answers `icons/<name>.svg` from that table and `icons/spinner-<frame>.svg` for the 12 pre-rotated frames (`icons.rs:45-70`; `SPINNER_FRAMES` already exists in `animation_clock.rs:71`). `icon()` is `svg().path(..).size(..).text_color(color)` per spec §6 and the gpui-development skill's AssetSource pattern — **the skill flags the tint method as unverified at this rev; check the `Svg` struct and record which of `text_color`/`color` is real.** `spinner(tick)` selects the frame with `animation_clock::spinner_frame(tick)` (every 3 ticks — parity).

Update `assets.rs`'s "Plan 06 adds it" doc comment to record that Plan 05 did.

- [ ] **Step 2: `activity.rs` — pure port plus the stub store**

Port `ActivityState` (`src/gui/activity.rs:30-36`) and `most_urgent`/`urgency_rank` (`:201-222`) with their tests: waiting(3) > working(2) > done(1); Idle/Exited contribute nothing; an empty iterator is `None`. **Do not port `classify`, `Signals`, `Tracker` or any of the timing constants — Plan 06 owns them.** `ActivityStore` is the stub from carried amendment 3.

- [ ] **Step 3: The pure row helpers, tests first**

Port with their existing iced tests where they have them:
- `worktree_shows_branch(is_main, branch, name)` (`rows.rs:249`) — `!is_main && branch != name && !branch.is_empty()`.
- `row_height(show_branch)` (`rows.rs:268`) — `ROW_H` or `ROW_H + 14.0`. This is the function carried amendment 2 forces `uniform_list` and the agent-menu overlay math to share.
- `sanitize_ui_text` (`rows.rs:809`) and `remove_all_ci` (`:895`) — UTF-8-safe, case-insensitive removal; port the boundary tests.
- `session_context` / `terminal_context` (`rows.rs:778-808`) — the contextual title shown on a row, with the internal `terminal N` label stripped.
- `sidebar_empty_copy(total_projects, active_count)` (`widgets/primitives.rs:195-220`) with its `empty_and_all_archived_pick_distinct_copy` test (`:407-419`) — the two states must stay textually distinct.
- `path_basename` — reimplemented per Constraint 3 candidate 1, with tests for a trailing slash, a root path, and a non-UTF-8-ish path.

- [ ] **Step 4: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -30
```

---

### Task 4: Tree flattening + the git poll (TDD, the phase's densest pure logic)

**Files:**
- Create: `crates/grove-gpui/src/entities/project_tree.rs`
- Modify: `crates/grove-gpui/src/views/rows.rs` (flattening lives beside the row renderers it feeds)

**Interfaces:**
- Produces: `TreeRow` (the flattened row enum), `flatten(tree, workspace_state, registry, activity) -> Vec<TreeRow>`, `visible_session_order() -> Vec<SessionId>`, `ProjectTree` (worktrees + `wt_cache` + generation), and the 5s off-thread git-state poll.

- [ ] **Step 1: TDD `flatten` — this is the sidebar's whole shape**

```rust
pub enum TreeRow {
    Project { idx: usize, name: String, count: usize, expanded: bool, is_git: bool,
              rollup: Option<ActivityState> },
    Worktree { proj: usize, wt: usize, name: String, branch: String, is_main: bool,
               active: bool, expanded: bool, has_run: bool, rollup: Option<ActivityState>,
               git_suffix: Option<String> },
    Session { id: SessionId, active: bool, pending_kill: bool, state: ActivityState },
    Empty { title: &'static str, subtitle: &'static str },
    TerminalsHeader { expanded: bool, count: usize, activity_dot: bool },
    Terminal { idx: usize, active: bool, pending_kill: bool },
}
```

Tests before implementation, each citing `src/gui/view/sidebar.rs:225-381`:
- Order is projects (active only, TRUE indices preserved) → their worktrees → each worktree's sessions, exactly as `tree_view` pushes them.
- A collapsed project emits **no** worktree or session rows (`:269-271`); a collapsed worktree emits no session rows (`:330-332`).
- **Roll-up glyphs appear only on collapsed parents** (`:251-257`, `:296-303`) — an expanded parent's `rollup` is always `None`, even when a descendant is waiting.
- Worktrees come from `app.worktrees` for the active project and from `wt_cache` otherwise (`:272-278`); a cache miss yields **no** worktree rows, not a panic.
- The non-main worktree name is `path_basename(w.path)`; the main worktree's name is the project name (`:285-289`).
- A session row is `active` only when `!terminal_focused && active_session == Some(id)` (`:338` — the comment there is the contract: a session must not look active while a home terminal is on screen).
- The empty state is emitted iff no active project produced a row, choosing between the two copies by `(store.projects.len(), active_count)` (`:357-361`).
- The TERMINALS section: when expanded, a divider + header (activity dot forced **off**, `:363-372`) + one row per home terminal, `active` iff `terminal_focused && active_terminal == Some(i)` (`:374`); when collapsed, the section emits nothing here and the header is docked separately by the view (`:114-129`) with the activity dot **on** iff any home terminal is `Running` (`:61-70`).

- [ ] **Step 2: TDD `visible_session_order`**

`src/gui/view/sidebar.rs:386-417`: the same walk, sessions only, honouring both collapse sets. It is `mod+1..9`'s index space **and** the attention queue's order (`update/mod.rs:728-739`), so derive it from `flatten`'s output rather than writing a second walk — and assert that equivalence in a test. Cover: a collapsed project hides its sessions from the numbering; the order is stable across an unrelated project's collapse toggle.

- [ ] **Step 3: `ProjectTree` — worktrees, cache, generation**

Port `worktrees_for_project` (`mod.rs:1185-1193`), `ensure_wt_cached` (`:1326-1334`), `rebuild_wt_cache` + the generation guard (`:1351`, and its doc comment explaining *why* the generation exists — a sweep launched before an add/remove/archive must not be folded in), and `switch_active_project`'s cache hand-off (`:1121-1130`). The off-thread sweep becomes a `cx.background_executor().spawn` whose result is applied through the entity, dropped when the generation moved. `is_repo` keeps its 5s memo (`sidebar.rs:26-54`) — it is a per-frame `stat` otherwise.

- [ ] **Step 4: The 5s git-state poll**

Port `maybe_poll_git_state` (`mod.rs:1251-1303`) and `visible_worktree_paths` (`:1308-1324`) as an independent background task (spec §4: "git 5s poll (in-flight guard)" becomes its own task, not a tick branch). Keep all three behaviors: the 5s throttle, the in-flight compare-exchange guard that **skips** rather than overlaps, and the failure semantics — a `None` from `worktree_git_state` **drops** the cached entry rather than showing stale data. Only worktrees of non-collapsed **active** projects are polled. `git_state_suffix` renders the dirty/ahead/behind text.

- [ ] **Step 5: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 5: The `Sidebar` view — rows, hover actions, divider

**Files:**
- Create: `crates/grove-gpui/src/views/sidebar.rs`
- Modify: `crates/grove-gpui/src/views/rows.rs` (the renderers), `crates/grove-gpui/src/views/mod.rs`

**Interfaces:**
- Produces: `Sidebar` — a gpui view rendering the header, the `uniform_list` tree, the agent-menu overlay, the docked TERMINALS header, and the draggable divider.

- [ ] **Step 1: The header** (`src/gui/view/sidebar.rs:141-223`)

`SESSBAR_H`-tall bar: the `PROJECTS` section label (letter-spaced by hand — iced has no letter-spacing and neither does this rev; the iced build joins chars with `\u{2009}` at `rows.rs:650-655`, so match that), a `plus` button (opens AddProject — Plan 08 stub), and the collapse-cycle button whose **glyph shows the *next* action** (`:143-147`: `SessionsOnly → "expand-sessions"`, `All → "expand-all"`, `Collapsed → "collapse-all"`). Hover styling is `BG_HOVER()` background + `FG()` text, otherwise transparent + `FG_MUTE()`.

- [ ] **Step 2: The row renderers** (`src/gui/rows.rs`)

One function per `TreeRow` variant, reading only from the flattened row (no lookups back into state — that is what made the iced version O(projects × worktrees × sessions) per frame, see `sidebar.rs:227-237`). Port, per row: the indent ladder, the twisty chevron, the session-count badge, the git glyph, the `main` tag (`rows.rs:257-265`, sharing its slot with the hover icons so they never compete for width), the branch chip, the git suffix, the context text, and `state_glyph` (`rows.rs:870-892`): Working = spinner on the clock tick, WaitingForInput = `question` in `AMBER()` **dimmed** by the pulse (`alpha = 1.0 - 0.45 * pulse`, never hidden, so layout stays stable), Done = `check`, Idle = `dot`, Exited = `ring` — all in a fixed 14px slot.

- [ ] **Step 3: Hover actions and the agent-menu overlay**

`hovered_wt` (`WorkspaceState`) drives the worktree row's action strip: spawn-agent icons (from `available_agents`), the run-script icon (only when `has_run`), add/delete worktree. Session rows carry their own kill affordance with the **two-step confirm** arming (`pending_kill`) — the *arming state* is sidebar state and lands here; the keyboard half of two-step confirm-kill (Escape disarms) is Plan 08's keyboard matrix. Every action that would open a modal dispatches to a logged stub naming its plan; every action that mutates selection or the registry is wired for real.

The agent-menu overlay is an absolutely-positioned layer over the list (`sidebar.rs:99-109`), positioned by the recomputed `open_agent_menu_top` walk (`:421-470`) using Task 3's `row_height` — carried amendment 2. Unit-test the offset walk against a fixture tree.

- [ ] **Step 4: `uniform_list` + the docked TERMINALS header**

`uniform_list("sidebar-tree", rows.len(), ..)` with the scroll handle stored on `Sidebar` and `.track_scroll(handle)` (gpui-development skill). When `terminals_collapsed`, the header renders **outside** the scroll area, pinned at the bottom above a `BORDER_SOFT` divider, with the activity dot on iff any home terminal is running (`sidebar.rs:114-129`, `:61-70`). Background is `BG_RAIL()`; the tree area keeps its 8px top / 12px bottom padding (`:93-98`).

- [ ] **Step 5: The divider — drag, clamp, double-click, persist**

Port `src/gui/update/layout.rs:105-160` and `src/gui/metrics.rs:244-251`:
- TDD `clamp_sidebar_width(width, logical_win_w)` first: lower bound `SIDEBAR_MIN_W` (220); upper bound `min(win/2, win - WORKSPACE_MIN_W)` floored at 220; the upper bound wins when the window is narrow. Cover a 1280 window (→ 640 cap), an 800 window (→ 400), a 500 window (→ 220 both ways).
- Press: a second press within **350ms** resets the width to `RAIL_W` (320, clamped) and persists; otherwise it starts a drag capturing `start_width`.
- Move: the grab offset is captured on the **first move** (`sidebar_width - cursor_x`) so an off-edge press does not jump the width; visual width follows live.
- Release: persist and resize PTYs only when the width actually moved (`>= 0.5px`) — a plain click must not write to disk.
- Persist through `SettingsState::update(|s| s.sidebar_width = Some(w))` (the 250ms debounce replaces the iced tick-debounce). The divider is a 6px hit zone around a 1px `BORDER()` line with a horizontal-resize cursor.

Terminal PTY re-dimensioning needs no wiring: the element derives its dims from its own bounds in `prepaint` (Plan 04 amendment 7), so a width change re-dims on the next frame.

- [ ] **Step 6: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

Row *appearance* is not unit-testable — Task 7's checklist owns it. Do not claim it green from a compile.

---

### Task 6: Data-carrying actions + per-project content themes

**Files:**
- Modify: `crates/grove-gpui/src/views/workspace.rs`, `crates/grove-gpui/src/keymap.rs`, `crates/grove-gpui/src/terminal_element.rs`, `crates/grove-gpui/src/views/terminal_view.rs`

**Interfaces:**
- Produces: real dispatch for `SelectSession(1..9)`, `NextSession`, `PrevSession`, `JumpToWaitingSession`, `CloseFocusedSession`, `ScrollHalfPage(bool)`; and `project_theme_override` resolution at the `// Plan 05` marker.

- [ ] **Step 1: The data-carrying actions** (Plan 03 deviation 3)

`binding_for` returns `None` for `SelectSession(_)`/`GridMove`/`GridSwap` (`keymap.rs:497-521`) because `actions!` generates unit structs. Give `SelectSession` a payload action — a `#[derive(Action)]`-style struct carrying `index: usize`, registered with nine bindings (`mod+1` … `mod+9`) generated from the registry's display-only `1–9` row, so the registry stays the single source of truth. `GridMove`/`GridSwap` stay unbound (Plan 07). Assert in a test that after this phase the only registry rows without a binding are the grid ones.

Dispatch, all on `WorkspaceState` and all citing their oracle:
- `SelectSession(n)` → `visible_session_order()[n-1]`, if present → `select_session` (which acknowledges). Out of range is a no-op, not a clamp.
- `NextSession`/`PrevSession` → `cycle_session` (Task 1; `sessions.rs:365-405`).
- `JumpToWaitingSession` → the first `WaitingForInput` in `visible_session_order` (`update/mod.rs:728-739`). With Plan 06's classifier stubbed to Idle this is a no-op today — **that is correct, not broken**; the wiring is what this phase owes. Note it in the checklist as stub-gated.
- `CloseFocusedSession` → the two-step confirm arming on the focused session/terminal (`sessions.rs:105-122` for terminals).
- `ScrollHalfPage(up)` → `TerminalSession::scroll_lines(up, scroll_page_lines()/2)` on the **focused** session.

Delete the corresponding `stub_action!` lines from `workspace.rs:128-139` as each becomes real; leave the Plan 06/07/08 ones.

- [ ] **Step 2: The workspace body follows the selection**

`Workspace::render` shows the `TerminalView` for `WorkspaceState`'s active session — or the active home terminal when `terminal_focused` — instead of the single hardcoded one. Views are cached per `SessionId` so switching does not respawn. `cx.observe(&workspace_state)` drives the repaint. Grid/zen remain absent (Plan 07).

- [ ] **Step 3: Per-project pinned content themes** (deferred here from Plan 04)

At `terminal_element.rs:135-142` — the `// Plan 05: project theme override` marker — replace the unconditional `grove_core::theme::with_current` with the resolution ported from `src/app/theme_picker.rs:65-128` and `src/gui/view/terminal.rs:48-73`:

1. **Live preview wins outright** when a project-scoped theme picker is open for this project — Plan 08 owns the picker, so leave a `// Plan 08: launcher/picker live preview` hook taking `Option<Option<Theme>>` and passing `None` today. The hook's shape is load-bearing: `Some(None)` means "preview the global theme", which is *not* the same as `None` ("no preview").
2. `if !store.project_themes_enabled { return None }` — the universal toggle.
3. Otherwise the project's pinned `Project::theme` name resolved via `theme::by_name`; an unresolvable name falls back to the global theme.

**App chrome always stays on the global theme regardless** (`storage.rs:151-155`) — assert this by keeping every `c::*` call site untouched, and say so in the doc comment.

TDD `project_theme_override(store, project_name, preview) -> Option<Theme>` as a pure function: toggle off → `None` even with a pin; pin set + toggle on → the pinned theme; unknown pin name → `None`; `Some(None)` preview → `None` (global) even when a pin exists; `Some(Some(t))` preview → `t` even when the toggle is **off** (the preview path bypasses the toggle — `theme_picker.rs:111-118` puts the preview check *before* the toggle check; this ordering is the parity contract).

- [ ] **Step 4: Toggle invalidation**

The iced build memoizes the per-PTY theme for one frame (`view/terminal.rs:33-46`, `reset_pty_theme_cache` at the top of `view()`) and explicitly invalidates on cancel/submit. gpui needs **neither**: the resolution is a cheap read of a `Store` field plus a name lookup, done fresh in `prepaint`, so flipping `project_themes_enabled` re-colors on the next frame with no bookkeeping. **Do not port the cache.** Record this deliberate omission in a comment at the call site — a future reader will look for it.

- [ ] **Step 5: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -40
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets --no-deps -- -D warnings 2>&1 | tail -20
```

---

### Task 7: Verification and the manual parity checklist

**Files:**
- Modify: `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` (row 05 → done)

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

Expected: everything green, the Plan 03 metric selftest still prints its `cell_w=7.5… OK` line, and **`git status` reports no changes at all** under `src/`, `crates/grove-core/`, `crates/grove-terminal/`. Read the raw output.

- [ ] **Step 2: MANUAL — the spec Appendix A **sidebar** rows (human, real desktop)**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui
# and, side by side, the installed iced build:
~/.local/bin/grove
```

Report each row pass/fail. **Do not claim any of these yourself.** Rows are the sidebar clauses of spec Appendix A → *Screens/layout*, plus the two Terminal-section rows deferred into this phase and the one Shortcuts row:

1. **Tree shape.** `sidebar project→worktree→session tree` — same order, indent ladder, chevrons, session-count badges, git glyph, `main` tag, branch chip, context text, and the same active-row highlight rules (a session never looks active while a home terminal is on screen).
2. **3-mode expand cycle.** The header button cycles Collapsed → SessionsOnly → All, its glyph always previews the **next** action, and `SessionsOnly` collapses exactly the projects/worktrees with no sessions. Manual per-row toggles are fully overridden by a cycle press.
3. **Per-row hover actions.** Hovering a worktree reveals the spawn/run/add/delete strip in the `main`-tag slot with no layout shift; the agent menu opens anchored to the correct row at any scroll position and any collapse state; session rows arm and disarm their two-step kill.
4. **Git suffix.** dirty/ahead/behind text appears within ~5s of a change, on off-thread polls only (no UI stutter), for visible worktrees only, and **disappears** rather than going stale when `git` fails or the repo goes away.
5. **Archived-projects row.** Archived projects are absent from the tree (TRUE indices preserved for the rest); archiving every project shows the "All projects archived / Restore one from Settings → Archived projects." copy, and having no projects at all shows the *other* copy. (See the recorded ambiguity in Global Constraints — the archived *list* is Plan 08.)
6. **Divider.** Drag-resizable within `220 .. min(win/2, win−400)`; the width does not jump on an off-edge grab; a **350ms** double-click resets to 320; the width survives a restart; the terminal re-dims to the new width; a plain click writes nothing.
7. **Pinned TERMINALS section.** Always ≥1 home terminal; expanded shows header + rows with the header dot **off**; collapsed docks the header at the bottom with the dot **on** iff a shell is running; closing the last terminal respawns one.
8. **Selection and keyboard nav.** `mod+1..9` selects the nth **visible** session (numbering follows collapse state); next/prev cycle in tree order and wrap; selecting a session moves the sidebar highlight and the workspace body together with no visible two-step; `mod+w`-style close arms the confirm.
9. **Per-project pinned content themes** (deferred from Plan 04). With Project themes **on** and a project pinned to a different theme, that project's session content re-colors while app chrome stays on the global theme; sessions of unpinned projects are unaffected.
10. **Project-themes toggle invalidates** (deferred from Plan 04). Flipping the toggle in the store re-colors pinned projects' content on the next frame — no restart, no stale frame.
11. **Scroll behavior.** A tree taller than the viewport scrolls smoothly, keeps its scroll position across a repaint/selection change, and the docked TERMINALS header stays pinned outside the scroll area.

Rows explicitly **deferred** and not checked here (record them as deferred, not failed): live attention/activity glyph *content* — the spinner, the amber pulse and the roll-ups render but every session classifies Idle until Plan 06's 480ms task, so `JumpToWaitingSession` is a no-op today; the appbar attention pill/dropdown → Plan 06; grid/zen/worktree panel/terminal tab and Agent/Panel focus routing → Plan 07; every modal a hover action would open (AddProject, RemoveProject, Archive, AgentPicker, SessionLauncher, ThemePicker incl. the project-theme live preview hook) and the keyboard matrix's Escape/confirm-kill carve-outs → Plan 08.

- [ ] **Step 3: `./install.sh`** — the orchestrator runs this.

```bash
./install.sh 2>&1 | tail -20
```

Expected: the release build + install of the **iced** `grove` binary still succeeds, untouched by this phase.

- [ ] **Step 4: Update the master plan and commit** — the orchestrator runs this.

Mark row 05 `done` in `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` with a one-line note recording: the `uniform_list` vs plain-column decision from carried amendment 2, whether `Svg` tints via `text_color` or `color` at this rev, any grove-core amendment that had to be authorized, and any Appendix A row that came back FAIL.

```bash
git add crates/grove-gpui docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md
git commit -m "feat(gpui): sidebar tree, session registry, WorkspaceState single owner"
```

**Exit gate met when:** the spec Appendix A sidebar rows above are signed off by a human as pass or explicitly-deferred, the flattening/selection/clamp/theme-override unit tests are green (raw output pasted), grove-gpui builds/tests/clippy clean on 1.95, the iced app and both existing crates are provably untouched and still build on the default toolchain, and `./install.sh` is green.
