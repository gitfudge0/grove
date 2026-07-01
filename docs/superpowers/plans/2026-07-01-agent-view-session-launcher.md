# Agent View — Session Launcher Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user launch a new agent session against any worktree of any project from inside Agent View, via a floating "+ New session" pill that opens a centered three-column (Project → Worktree → Agent) launcher modal.

**Architecture:** A new `Modal::SessionLauncher` variant carries the launcher's transient selection state (project/worktree/agent indices, focused column, skip-perms, label). All non-trivial selection/navigation logic lives in a new pure module `src/gui/launcher.rs` (no Iced deps) so it is unit-testable without spinning up the GUI. `update.rs` wires new `Msg` variants to that logic and to the existing `App::spawn_session`; `view.rs` renders the modal (mirroring `agent_picker_modal`) and the floating pill (added to `grid_workspace` via `stack![]`). Keyboard routing reuses the existing `handle_modal_key` dispatch plus a Cmd/Ctrl+N branch in the non-modal path.

**Tech Stack:** Rust, Iced (elm-style: `Grove` model, `Msg` enum, `update`/`view`), `git worktree` shell-outs via `crate::git`, existing modal/widget primitives in `src/gui/widgets.rs`.

## Global Constraints

- Rust edition/toolchain: use the repo's existing toolchain; every task must `cargo build` and `cargo test` clean (no new warnings).
- Follow existing conventions: `Msg` variants in `src/gui/state.rs`; `Modal` variants in `src/app.rs`; color tokens via `c::*()` helpers; modal chrome via `modal_panel` / `modal_list_row` / `modal_action` / `ModalBtn`; row height constant `crate::gui::metrics::ROW_H` (28.0).
- Agent list source: `app.available_agents` (installed-only), re-scanned via `App::refresh_available_agents()` when the launcher opens (same as the sidebar picker).
- Spawn convention: label defaults to project name for the main worktree, else the worktree path basename (`crate::app::path_basename`); skip-perms defaults to `App::skip_permissions_enabled()`; launch args come from `Agent::launch_args(skip_perms)`.
- On a successful spawn while `grid_view` is true, append the new session index to `tile_order` and set `grid_focused` (mirror `submit_agent_picker`, `src/gui/update.rs`).
- Close reuses the existing `Msg::ModalCancel` / `cancel_modal()` path.
- Commit after every green step. Every commit message ends with:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Final task bumps `version` in `Cargo.toml` (currently `0.26.1`).

---

## File Structure

| File | Change | Responsibility |
| --- | --- | --- |
| `src/gui/launcher.rs` | Create | Pure, Iced-free launcher logic + unit tests: `LauncherNav` newtype-ish helpers for column/selection clamping, `default_label`, `clamp`, project→worktree list assembly, breadcrumb text. This is the tested seam. |
| `src/gui/mod.rs` | Modify (~L16-28) | Declare `mod launcher;`. |
| `src/app.rs` | Modify (`Modal` enum ~L145-241) | Add `Modal::SessionLauncher { proj, wt, agent, col, skip_perms, label }`. |
| `src/gui/state.rs` | Modify (`Msg` enum, grid msgs ~L533-545) | Add the 8 new `Msg` variants. |
| `src/gui/update.rs` | Modify (handlers, `handle_modal_key` ~L1606, `handle_key` ~L1483) | `open_session_launcher`, per-Msg handlers, `launcher_start`, `Modal::SessionLauncher` key arm, Cmd/Ctrl+N open, `+ New worktree` round-trip. |
| `src/gui/view.rs` | Modify (`modal_layer` ~L1701, `grid_workspace` ~L809, new `session_launcher_modal`) | Render the three-column modal, footer breadcrumb + Start button, and the floating pill. |
| `Cargo.toml` | Modify (`version`) | Version bump. |

---

## Task 1: Launcher state + Msg scaffolding + pure logic (tested)

**Files:**
- Create: `src/gui/launcher.rs`
- Modify: `src/gui/mod.rs:16-28`
- Modify: `src/app.rs` (`Modal` enum, after the `AgentPicker` variant ~L196)
- Modify: `src/gui/state.rs` (`Msg` enum, after `GridTileZen(usize)` ~L544)
- Test: `src/gui/launcher.rs` (inline `#[cfg(test)] mod tests`, matching repo convention e.g. `src/gui/keys.rs`)

**Interfaces:**
- Consumes: `crate::git::Worktree { path, branch, mtime, is_main }`; `crate::app::path_basename(&str) -> String`; `crate::agent::Agent`.
- Produces (used by Tasks 2-6):
  - `pub fn clamp(v: usize, delta: i32, len: usize) -> usize`
  - `pub fn move_column(col: u8, delta: i32) -> u8`
  - `pub fn default_label(is_main: bool, project_name: &str, wt_path: &str) -> String`
  - `pub fn effective_label(typed: &str, is_main: bool, project_name: &str, wt_path: &str) -> String`
  - `pub fn breadcrumb(project_name: &str, branch: &str, agent_label: &str) -> String`
  - New `Modal::SessionLauncher { proj: usize, wt: usize, agent: usize, col: u8, skip_perms: bool, label: String }`
  - New `Msg` variants: `OpenSessionLauncher`, `LauncherSelectProject(usize)`, `LauncherSelectWorktree(usize)`, `LauncherSelectAgent(usize)`, `LauncherToggleSkip`, `LauncherLabelChanged(String)`, `LauncherNewWorktree`, `LauncherStart`.

- [ ] **Step 1: Write the failing test module (create `src/gui/launcher.rs`)**

```rust
//! Pure selection / navigation logic for the Agent View session launcher.
//! Kept free of Iced so it can be unit-tested without a GUI. The launcher's
//! transient state lives in `crate::app::Modal::SessionLauncher`; these helpers
//! compute the next state and derived display strings.

/// Clamp `v + delta` into `[0, len)`. Saturates at both ends; returns 0 when
/// `len == 0` (an empty column has no valid selection).
pub fn clamp(v: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let max = len - 1;
    let next = (v as i64 + delta as i64).clamp(0, max as i64);
    next as usize
}

/// Move the focused column left (`delta < 0`) or right, clamped to `[0, 2]`
/// (0 = project, 1 = worktree, 2 = agent).
pub fn move_column(col: u8, delta: i32) -> u8 {
    let next = (col as i32 + delta).clamp(0, 2);
    next as u8
}

/// Default session label following Grove's spawn convention: the project name
/// for the main checkout, otherwise the worktree path basename.
pub fn default_label(is_main: bool, project_name: &str, wt_path: &str) -> String {
    if is_main {
        project_name.to_string()
    } else {
        crate::app::path_basename(wt_path)
    }
}

/// The label to spawn with: the trimmed typed label if non-empty, else the
/// default naming.
pub fn effective_label(typed: &str, is_main: bool, project_name: &str, wt_path: &str) -> String {
    let t = typed.trim();
    if t.is_empty() {
        default_label(is_main, project_name, wt_path)
    } else {
        t.to_string()
    }
}

/// Footer breadcrumb text: `project › branch › agent`.
pub fn breadcrumb(project_name: &str, branch: &str, agent_label: &str) -> String {
    format!("{project_name} › {branch} › {agent_label}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_saturates_and_handles_empty() {
        assert_eq!(clamp(0, -1, 3), 0);
        assert_eq!(clamp(2, 1, 3), 2);
        assert_eq!(clamp(1, 1, 3), 2);
        assert_eq!(clamp(1, -1, 3), 0);
        assert_eq!(clamp(0, 5, 0), 0); // empty column
        assert_eq!(clamp(5, 0, 3), 2); // stale index re-clamped
    }

    #[test]
    fn move_column_is_bounded() {
        assert_eq!(move_column(0, -1), 0);
        assert_eq!(move_column(0, 1), 1);
        assert_eq!(move_column(2, 1), 2);
        assert_eq!(move_column(1, -1), 0);
    }

    #[test]
    fn default_label_follows_spawn_convention() {
        assert_eq!(default_label(true, "grove", "/home/u/grove"), "grove");
        assert_eq!(
            default_label(false, "grove", "/home/u/grove/.wt/fix-scroll"),
            "fix-scroll"
        );
    }

    #[test]
    fn effective_label_prefers_typed_then_defaults() {
        assert_eq!(
            effective_label("  ", false, "grove", "/home/u/grove/.wt/fix"),
            "fix"
        );
        assert_eq!(
            effective_label("  my label  ", false, "grove", "/x/y"),
            "my label"
        );
        assert_eq!(effective_label("", true, "grove", "/home/u/grove"), "grove");
    }

    #[test]
    fn breadcrumb_joins_with_chevrons() {
        assert_eq!(breadcrumb("grove", "main", "claude"), "grove › main › claude");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails (module not wired yet)**

Run: `cargo test --lib gui::launcher`
Expected: FAIL — `error[E0583]: file not found for module` or `error: module launcher is not declared` until Step 3 (module not registered).

- [ ] **Step 3: Declare the module in `src/gui/mod.rs`**

Insert alphabetically among the `mod` lines (after `mod keys;` at ~L20):

```rust
mod launcher;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib gui::launcher`
Expected: PASS — `test result: ok. 5 passed`.

- [ ] **Step 5: Add the `Modal::SessionLauncher` variant in `src/app.rs`**

Insert directly after the `AgentPicker { .. }` variant (~L196), inside `pub enum Modal`:

```rust
    /// Agent View "+ New session" launcher: three Miller columns
    /// (project → worktree → agent) plus a skip-permissions toggle and an
    /// optional label. `proj` indexes `store.projects`; `wt` indexes the
    /// selected project's worktrees (`app.worktrees` when it is the active
    /// project, else `Grove::wt_cache[proj]`); `agent` indexes
    /// `available_agents`; `col` is the focused column (0=project 1=worktree
    /// 2=agent). Reachable only while the grid is open.
    SessionLauncher {
        proj: usize,
        wt: usize,
        agent: usize,
        col: u8,
        skip_perms: bool,
        label: String,
    },
```

- [ ] **Step 6: Add the new `Msg` variants in `src/gui/state.rs`**

Insert after `GridTileZen(usize),` (~L544), before the closing `}` of `pub enum Msg`:

```rust
    // ── Agent View session launcher ──────────────────────────────────────
    /// Open the launcher (pill click or Cmd/Ctrl+N while the grid is open).
    OpenSessionLauncher,
    /// Select the project at this index; resets worktree selection to 0 and
    /// lazily loads that project's worktrees.
    LauncherSelectProject(usize),
    /// Select the worktree at this index within the current project.
    LauncherSelectWorktree(usize),
    /// Select the agent at this index within `available_agents`.
    LauncherSelectAgent(usize),
    /// Toggle the skip-permission-prompts option for this launch.
    LauncherToggleSkip,
    /// Live edit of the optional label field.
    LauncherLabelChanged(String),
    /// "+ New worktree…" row: hand off to the worktree-name input flow.
    LauncherNewWorktree,
    /// Start the session with the current selection.
    LauncherStart,
```

- [ ] **Step 7: Verify it builds (handlers not yet wired — expect a non-exhaustive-match failure)**

Run: `cargo build 2>&1 | head -40`
Expected: FAIL — `error[E0004]: non-exhaustive patterns` in `src/gui/update.rs` (the `match msg`) and/or `src/gui/view.rs` (`modal_layer`) because the new `Msg`/`Modal` variants are unhandled. This confirms the enums compiled; Tasks 2 and 4 add the arms.

- [ ] **Step 8: Commit**

```bash
git add src/gui/launcher.rs src/gui/mod.rs src/app.rs src/gui/state.rs
git commit -m "feat(launcher): add SessionLauncher modal, Msgs, and pure logic module

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

Note: the tree is intentionally not-yet-building between Task 1 and Task 2 (the match arms land in Task 2). This is a deliberate scaffolding boundary; the commit records the enum + tested-logic surface.

---

## Task 2: update.rs handlers — wire Msgs to state + spawn

**Files:**
- Modify: `src/gui/update.rs` — add the `match msg` arms (near the other grid/modal handlers) and helper methods `open_session_launcher`, `launcher_select_project`, `launcher_start`.
- Test: `src/gui/launcher.rs` (extend with one more pure helper + test for the "selected project's worktrees" resolution rule).

**Interfaces:**
- Consumes: `crate::gui::launcher::{clamp, effective_label}`; `App::refresh_available_agents()`; `App::skip_permissions_enabled()`; `Grove::ensure_wt_cached(usize)`; `App::spawn_session(label, project, wt_path, agent, args, cwd)`; `Agent::launch_args(bool)`; `crate::git::Worktree`.
- Produces: `Grove::launcher_worktrees(proj: usize) -> Vec<crate::git::Worktree>` (used by Task 4's render and by keyboard nav in Task 3).

- [ ] **Step 1: Write the failing test for the worktree-resolution rule**

Add to `src/gui/launcher.rs` (this is pure — it takes the two candidate slices, not `Grove`):

```rust
/// Resolve which worktree list backs a given launcher project column.
/// `is_active_project` selects `active` (the live `app.worktrees`); otherwise
/// the cached list is used. Returned as a slice reference so callers avoid a
/// clone in the hot render path.
pub fn worktrees_for<'a>(
    is_active_project: bool,
    active: &'a [crate::git::Worktree],
    cached: &'a [crate::git::Worktree],
) -> &'a [crate::git::Worktree] {
    if is_active_project {
        active
    } else {
        cached
    }
}
```

And in the `#[cfg(test)] mod tests`:

```rust
    fn wt(path: &str, is_main: bool) -> crate::git::Worktree {
        crate::git::Worktree {
            path: path.into(),
            branch: "b".into(),
            mtime: None,
            is_main,
        }
    }

    #[test]
    fn worktrees_for_picks_active_vs_cached() {
        let active = vec![wt("/a/main", true)];
        let cached = vec![wt("/b/main", true), wt("/b/.wt/x", false)];
        assert_eq!(worktrees_for(true, &active, &cached).len(), 1);
        assert_eq!(worktrees_for(false, &active, &cached).len(), 2);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib gui::launcher::tests::worktrees_for_picks_active_vs_cached`
Expected: FAIL — `cannot find function worktrees_for` (the build error from Task 1's unhandled arms is still present; this step confirms the new test symbol is missing). If the crate does not compile due to the Task 1 non-exhaustive match, proceed to Step 3 which resolves it, then re-run.

- [ ] **Step 3: Add the `launcher_worktrees` accessor and helper methods in `src/gui/update.rs`**

Add these methods to the `impl Grove` block that already holds `spawn`, `ensure_wt_cached`, `rebuild_wt_cache` (near ~L2086):

```rust
    /// The worktrees backing launcher project `proj`: the live `app.worktrees`
    /// when it is the active project, else the cached list (loaded on demand).
    pub(super) fn launcher_worktrees(&self, proj: usize) -> Vec<crate::git::Worktree> {
        if proj == self.app.proj_idx {
            self.app.worktrees.clone()
        } else {
            self.wt_cache.get(&proj).cloned().unwrap_or_default()
        }
    }

    /// Open the session launcher with a sensible default selection: the active
    /// project + worktree, agent index 0, skip-perms from the global default.
    fn open_session_launcher(&mut self) {
        if self.app.store.projects.is_empty() {
            return;
        }
        self.app.refresh_available_agents();
        let n = self.app.store.projects.len();
        for i in 0..n {
            self.ensure_wt_cached(i);
        }
        let proj = self.app.proj_idx.min(n - 1);
        let wt = self
            .app
            .wt_idx
            .min(self.launcher_worktrees(proj).len().saturating_sub(1));
        self.app.modal = crate::app::Modal::SessionLauncher {
            proj,
            wt,
            agent: 0,
            col: 0,
            skip_perms: self.app.skip_permissions_enabled(),
            label: String::new(),
        };
    }

    /// Select a launcher project: reset the worktree selection and ensure that
    /// project's worktrees are loaded.
    fn launcher_select_project(&mut self, index: usize) {
        let n = self.app.store.projects.len();
        if index >= n {
            return;
        }
        self.ensure_wt_cached(index);
        if let crate::app::Modal::SessionLauncher { proj, wt, col, .. } = &mut self.app.modal {
            *proj = index;
            *wt = 0;
            *col = 0;
        }
    }

    /// Start the selected session, then (grid always open here) append it to
    /// `tile_order` and focus it.
    fn launcher_start(&mut self) {
        let crate::app::Modal::SessionLauncher {
            proj,
            wt,
            agent,
            skip_perms,
            label,
            ..
        } = self.app.modal.clone()
        else {
            return;
        };
        let Some(project) = self.app.store.projects.get(proj) else {
            return;
        };
        let pname = project.name.clone();
        let worktrees = self.launcher_worktrees(proj);
        let Some(w) = worktrees.get(wt).cloned() else {
            return;
        };
        let Some(ag) = self.app.available_agents.get(agent).copied() else {
            return;
        };
        let label = crate::gui::launcher::effective_label(&label, w.is_main, &pname, &w.path);
        let args = ag.launch_args(skip_perms);
        let before = self.session_keys();
        self.app.modal = crate::app::Modal::None;
        self.app
            .spawn_session(label, pname, w.path.clone(), ag, args, &w.path);
        self.resize_new_sessions(&before);
        if self.grid_view && self.app.sessions.len() > before.len() {
            let si = self.app.sessions.len() - 1;
            self.tile_order.push(si);
            self.grid_focused = Some(si);
            self.refresh_pty_viewport();
        }
        self.rebuild_wt_cache();
    }
```

- [ ] **Step 4: Add the `match msg` arms in `src/gui/update.rs`**

Add after the `Msg::GridTileZen(si) => { .. }` arm (~L1034):

```rust
            Msg::OpenSessionLauncher => self.open_session_launcher(),
            Msg::LauncherSelectProject(i) => self.launcher_select_project(i),
            Msg::LauncherSelectWorktree(i) => {
                if let crate::app::Modal::SessionLauncher { wt, col, .. } = &mut self.app.modal {
                    *wt = i;
                    *col = 1;
                }
            }
            Msg::LauncherSelectAgent(i) => {
                let max = self.app.available_agents.len();
                if let crate::app::Modal::SessionLauncher { agent, col, .. } = &mut self.app.modal {
                    if i < max {
                        *agent = i;
                        *col = 2;
                    }
                }
            }
            Msg::LauncherToggleSkip => {
                if let crate::app::Modal::SessionLauncher { skip_perms, .. } = &mut self.app.modal {
                    *skip_perms = !*skip_perms;
                }
            }
            Msg::LauncherLabelChanged(s) => {
                if let crate::app::Modal::SessionLauncher { label, .. } = &mut self.app.modal {
                    *label = s;
                }
            }
            Msg::LauncherNewWorktree => self.launcher_new_worktree(),
            Msg::LauncherStart => self.launcher_start(),
```

Note: `launcher_new_worktree` is implemented in Task 6. For Tasks 2-5, add a temporary stub so the tree builds; Task 6 replaces its body:

```rust
    /// Placeholder until Task 6 implements the "+ New worktree…" round-trip.
    fn launcher_new_worktree(&mut self) {}
```

- [ ] **Step 5: Run the launcher tests to verify they pass**

Run: `cargo test --lib gui::launcher`
Expected: PASS — `test result: ok. 6 passed`.

- [ ] **Step 6: Confirm the crate builds (view still unhandled → expect the view match error only)**

Run: `cargo build 2>&1 | head -30`
Expected: FAIL — remaining `error[E0004]: non-exhaustive patterns` is now ONLY in `src/gui/view.rs` `modal_layer` (the `update.rs` match is now exhaustive). This confirms the handlers compiled.

- [ ] **Step 7: Commit**

```bash
git add src/gui/update.rs src/gui/launcher.rs
git commit -m "feat(launcher): wire launcher Msg handlers to state and spawn_session

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Keyboard routing — modal arm + Cmd/Ctrl+N open

**Files:**
- Modify: `src/gui/update.rs` — `handle_modal_key` (~L1606, add a `Modal::SessionLauncher` arm) and `handle_key` (~L1483, add a Cmd/Ctrl+N branch guarded by `grid_view`).
- Test: `src/gui/launcher.rs` — add a pure `nav_within_column` helper + test (the arrow-key state transition), keeping the tested seam.

**Interfaces:**
- Consumes: `crate::gui::launcher::{clamp, move_column}`; `Grove::launcher_worktrees`; `iced::keyboard::{Key, key::Named, Modifiers}`.
- Produces: `crate::gui::launcher::nav_within_column(col, proj, wt, agent, delta, proj_len, wt_len, agent_len) -> (usize, usize, usize)` — the new `(proj, wt, agent)` after an up/down move; on the project column it also resets `wt` to 0.

- [ ] **Step 1: Write the failing test for `nav_within_column`**

Add to `src/gui/launcher.rs`:

```rust
/// Compute `(proj, wt, agent)` after an up/down (`delta = ±1`) move in the
/// focused column. Moving within the project column resets `wt` to 0 (the
/// worktree list changes with the project). Lengths are clamped independently.
pub fn nav_within_column(
    col: u8,
    proj: usize,
    wt: usize,
    agent: usize,
    delta: i32,
    proj_len: usize,
    wt_len: usize,
    agent_len: usize,
) -> (usize, usize, usize) {
    match col {
        0 => (clamp(proj, delta, proj_len), 0, agent),
        1 => (proj, clamp(wt, delta, wt_len), agent),
        _ => (proj, wt, clamp(agent, delta, agent_len)),
    }
}
```

In the test module:

```rust
    #[test]
    fn nav_within_column_moves_focused_axis() {
        // project column: moving resets wt to 0
        assert_eq!(nav_within_column(0, 0, 3, 1, 1, 3, 5, 4), (1, 0, 1));
        // worktree column: only wt changes
        assert_eq!(nav_within_column(1, 1, 2, 1, 1, 3, 5, 4), (1, 3, 1));
        assert_eq!(nav_within_column(1, 1, 4, 1, 1, 3, 5, 4), (1, 4, 1)); // clamp top
        // agent column: only agent changes
        assert_eq!(nav_within_column(2, 1, 2, 0, -1, 3, 5, 4), (1, 2, 0)); // clamp bottom
        assert_eq!(nav_within_column(2, 1, 2, 1, 1, 3, 5, 4), (1, 2, 2));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib gui::launcher::tests::nav_within_column_moves_focused_axis`
Expected: FAIL — `cannot find function nav_within_column` (or the Task-2 view match error if still open — add the function then re-run).

- [ ] **Step 3: Add the `Modal::SessionLauncher` arm in `handle_modal_key`**

Insert in the `match &self.app.modal` inside `handle_modal_key` (~L1606), after the `Modal::AgentPicker { .. }` arm (~L1693):

```rust
            Modal::SessionLauncher {
                proj,
                wt,
                agent,
                col,
                ..
            } => {
                let (proj, wt, agent, col) = (*proj, *wt, *agent, *col);
                match key {
                    Key::Named(Named::Escape) => self.cancel_modal(),
                    Key::Named(Named::Enter) => self.launcher_start(),
                    Key::Named(Named::ArrowLeft) => {
                        if let Modal::SessionLauncher { col, .. } = &mut self.app.modal {
                            *col = crate::gui::launcher::move_column(*col, -1);
                        }
                    }
                    Key::Named(Named::ArrowRight) => {
                        if let Modal::SessionLauncher { col, .. } = &mut self.app.modal {
                            *col = crate::gui::launcher::move_column(*col, 1);
                        }
                    }
                    Key::Named(Named::ArrowDown) | Key::Named(Named::ArrowUp) => {
                        let delta = if matches!(key, Key::Named(Named::ArrowDown)) { 1 } else { -1 };
                        let proj_len = self.app.store.projects.len();
                        let wt_len = self.launcher_worktrees(proj).len();
                        let agent_len = self.app.available_agents.len();
                        let (np, nw, na) = crate::gui::launcher::nav_within_column(
                            col, proj, wt, agent, delta, proj_len, wt_len, agent_len,
                        );
                        // A project change reloads that project's worktrees.
                        if col == 0 && np != proj {
                            self.ensure_wt_cached(np);
                        }
                        if let Modal::SessionLauncher { proj, wt, agent, .. } = &mut self.app.modal {
                            *proj = np;
                            *wt = nw;
                            *agent = na;
                        }
                    }
                    _ => {}
                }
            }
```

Note: the label `text_input` owns text entry; when it is focused, iced marks the key event captured and the global subscription skips it (same pattern as `Modal::Input`), so typing into the label does not trigger these arrows.

- [ ] **Step 4: Add the Cmd/Ctrl+N open in `handle_key` (non-modal path)**

In `handle_key`, after the `if !matches!(self.app.modal, Modal::None) { return self.handle_modal_key(key, mods); }` early-return (~L1494) and inside the `if let Key::Character(s) = &key` block (~L1499), add as the first check inside that block:

```rust
            if is_new_session_shortcut(mods, s) && self.grid_view {
                return self.update(Msg::OpenSessionLauncher);
            }
```

Then add the free function near `is_copy_shortcut` (~L2354):

```rust
/// Returns true when the key event matches the "new session" shortcut.
/// macOS: Cmd+N (logo, no ctrl)  |  others: Ctrl+N.
fn is_new_session_shortcut(mods: Modifiers, s: &str) -> bool {
    if !s.eq_ignore_ascii_case("n") {
        return false;
    }
    #[cfg(target_os = "macos")]
    return mods.logo() && !mods.control();
    #[cfg(not(target_os = "macos"))]
    return mods.control();
}
```

- [ ] **Step 5: Run the launcher tests to verify they pass**

Run: `cargo test --lib gui::launcher`
Expected: PASS — `test result: ok. 7 passed`.

- [ ] **Step 6: Build (view match still open until Task 4)**

Run: `cargo build 2>&1 | head -20`
Expected: FAIL — only the `src/gui/view.rs` `modal_layer` non-exhaustive match remains. Keyboard code compiled.

- [ ] **Step 7: Commit**

```bash
git add src/gui/update.rs src/gui/launcher.rs
git commit -m "feat(launcher): keyboard routing (arrows/enter/esc + Cmd/Ctrl+N open)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: View — three-column launcher modal (manual verification)

**Files:**
- Modify: `src/gui/view.rs` — add `fn session_launcher_modal(...)` (mirroring `agent_picker_modal` ~L2373) and the `Modal::SessionLauncher` arm in `modal_layer` (~L1738).

**Interfaces:**
- Consumes: `crate::gui::launcher::breadcrumb`; `Grove::launcher_worktrees`; `self.app.store.projects`, `self.app.available_agents`; `modal_panel`, `modal_list_row`, `modal_action`, `ModalBtn`; `c::*()` tokens; `crate::app::path_basename`; `crate::gui::view::modal_input_id`.
- Produces: rendered launcher; consumes the Task-1 `Msg` variants via `on_press` / `on_input` / `on_toggle`.

- [ ] **Step 1: Add the `session_launcher_modal` render fn in `src/gui/view.rs`**

Add near `agent_picker_modal` (after it, ~L2437). The three columns each reuse `modal_list_row`; the agent column appends the skip toggle and the label `text_input`; the footer shows the breadcrumb + Start button.

```rust
    fn session_launcher_modal<'a>(
        &'a self,
        proj: usize,
        wt: usize,
        agent: usize,
        col: u8,
        skip_perms: bool,
        label: &'a str,
    ) -> Element<'a, Msg> {
        use iced::widget::{checkbox, text_input};

        // ── Column 1: projects ──────────────────────────────────────────
        let mut proj_list = Column::new().spacing(0);
        for (i, p) in self.app.store.projects.iter().enumerate() {
            let count = self.launcher_worktrees(i).len();
            let active = i == proj;
            let label_row = row![
                text(p.name.clone()).size(12).color(if active { c::FG() } else { c::FG_DIM() }),
                Space::with_width(Length::Fill),
                text(count.to_string()).size(11).color(c::FG_MUTE()),
            ]
            .align_y(iced::Alignment::Center);
            proj_list = proj_list.push(modal_list_row(label_row, active, Msg::LauncherSelectProject(i)));
        }

        // ── Column 2: worktrees ─────────────────────────────────────────
        let worktrees = self.launcher_worktrees(proj);
        let mut wt_list = Column::new().spacing(0);
        for (i, w) in worktrees.iter().enumerate() {
            let active = i == wt;
            let name = if w.branch.is_empty() {
                crate::app::path_basename(&w.path)
            } else {
                w.branch.clone()
            };
            let tag = if w.is_main { "main" } else { "" };
            let label_row = row![
                text(name).size(12).color(if active { c::FG() } else { c::FG_DIM() }),
                Space::with_width(Length::Fill),
                text(tag.to_string()).size(10).color(c::GREEN()),
            ]
            .align_y(iced::Alignment::Center);
            wt_list = wt_list.push(modal_list_row(label_row, active, Msg::LauncherSelectWorktree(i)));
        }
        // "+ New worktree…" affordance.
        let add_row = row![text("+ New worktree…").size(12).color(c::MAGENTA())]
            .align_y(iced::Alignment::Center);
        wt_list = wt_list.push(modal_list_row(add_row, false, Msg::LauncherNewWorktree));

        // ── Column 3: agents + options ──────────────────────────────────
        let mut agent_list = Column::new().spacing(0);
        for (i, ag) in self.app.available_agents.iter().enumerate() {
            let active = i == agent;
            let label_row = row![text(ag.label().to_string())
                .size(12)
                .color(if active { c::FG() } else { c::FG_DIM() })]
            .align_y(iced::Alignment::Center);
            agent_list = agent_list.push(modal_list_row(label_row, active, Msg::LauncherSelectAgent(i)));
        }
        let opts = column![
            checkbox("Skip permission prompts", skip_perms)
                .on_toggle(|_| Msg::LauncherToggleSkip)
                .size(15)
                .text_size(11),
            text_input("Optional label (defaults to worktree name)", label)
                .id(modal_input_id())
                .on_input(Msg::LauncherLabelChanged)
                .padding(6)
                .size(12),
        ]
        .spacing(8);
        let agent_col = column![agent_list, Space::with_height(8), opts].spacing(8);

        // Fixed-height columns so the modal keeps the mock's proportions.
        let col_h = Length::Fixed(300.0);
        let make_col = |title: &'static str, body: Element<'a, Msg>, focused: bool| {
            column![
                text(title).size(10).color(if focused { c::CYAN() } else { c::FG_MUTE() }),
                container(body).height(col_h).width(Length::Fill),
            ]
            .spacing(6)
            .width(Length::FillPortion(1))
        };
        let cols = row![
            make_col("PROJECT", proj_list.into(), col == 0),
            make_col("WORKTREE", wt_list.into(), col == 1),
            make_col("AGENT", agent_col.into(), col == 2),
        ]
        .spacing(12);

        // ── Footer: breadcrumb + Start ──────────────────────────────────
        let pname = self.app.store.projects.get(proj).map(|p| p.name.clone()).unwrap_or_default();
        let branch = worktrees
            .get(wt)
            .map(|w| if w.branch.is_empty() { crate::app::path_basename(&w.path) } else { w.branch.clone() })
            .unwrap_or_default();
        let ag_label = self.app.available_agents.get(agent).map(|a| a.label().to_string()).unwrap_or_default();
        let crumb = crate::gui::launcher::breadcrumb(&pname, &branch, &ag_label);
        let footer = row![
            text(crumb).size(12).color(c::FG_DIM()),
            Space::with_width(Length::Fill),
            text("←/→ columns · ↑/↓ move · ↵ start · esc").size(10).color(c::FG_MUTE()),
            modal_action("Start session", ModalBtn::Primary, Msg::LauncherStart),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        let body = column![
            text("New session").size(13).color(c::MAGENTA()),
            cols,
            Space::with_height(8),
            footer,
        ]
        .spacing(12);

        modal_panel(body.into(), 760.0, c::MAGENTA())
    }
```

- [ ] **Step 2: Add the `modal_layer` dispatch arm in `src/gui/view.rs`**

Insert after the `Modal::AgentPicker { .. } => self.agent_picker_modal(...)` arm (~L1742):

```rust
            Modal::SessionLauncher {
                proj,
                wt,
                agent,
                col,
                skip_perms,
                label,
            } => self.session_launcher_modal(*proj, *wt, *agent, *col, *skip_perms, label),
```

- [ ] **Step 3: Build to verify the crate now compiles clean**

Run: `cargo build 2>&1 | tail -5`
Expected: PASS — `Finished` with no errors and no new warnings. (If `checkbox`/`text_input` imports are already brought in at the top of `view.rs`, drop the local `use`; the plan's local `use` is scoped so it is harmless if the top-level import also exists — resolve any "unused import" warning by removing the local `use` line.)

- [ ] **Step 4: Run the full test suite**

Run: `cargo test`
Expected: PASS — all existing tests plus the 7 `gui::launcher` tests green.

- [ ] **Step 5: Manual verification**

Run: `cargo run`
- Add at least two projects (each with ≥1 worktree) so the columns are populated.
- Start ≥1 session, then toggle Agent View (grid).
- Press Cmd+N (macOS) / Ctrl+N (Linux). The launcher modal should appear centered, ~760px wide, with three columns Project / Worktree / Agent, matching `mock.html`.
- Observe: selecting a project (click) repopulates the worktree column and its count; selecting a worktree/agent highlights the row; the skip toggle flips; typing in the label field works; the footer breadcrumb reads `project › branch › agent` and updates live.
- Press `←/→` to move the focused column header highlight; `↑/↓` to move the selection; `Esc` closes.
- Do NOT click Start yet (spawn path verified in Step 6 of the manual check below and again in Task 5). Then click Start: a new tile should appear in the grid and receive focus.

- [ ] **Step 6: Commit**

```bash
git add src/gui/view.rs
git commit -m "feat(launcher): render three-column session launcher modal

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: View — floating "+ New session" pill (manual verification)

**Files:**
- Modify: `src/gui/view.rs` — wrap `grid_workspace`'s returned element in `stack![grid, pill]`, anchored bottom-right.

**Interfaces:**
- Consumes: `iced::widget::stack`; `modal_action` or a bespoke `button`; `Msg::OpenSessionLauncher`; `c::*()`.
- Produces: no new public symbol; the pill dispatches `Msg::OpenSessionLauncher`.

- [ ] **Step 1: Add the pill overlay in `grid_workspace`**

In `src/gui/view.rs` `grid_workspace` (~L809), replace the final `container(cols_row)...into()` return (~L851-858) with a stacked version. The grid container is bound to `grid`, and a bottom-right-anchored pill is stacked over it:

```rust
        let grid = container(cols_row)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BORDER_SOFT())),
                ..Default::default()
            });

        let pill = button(
            row![text("+ New session").size(12).color(c::FG())]
                .spacing(6)
                .align_y(iced::Alignment::Center),
        )
        .on_press(Msg::OpenSessionLauncher)
        .padding(Padding::from([9, 15]))
        .style(|_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: Some(Background::Color(if hovered { c::BG_HOVER() } else { c::BG_HL() })),
                text_color: c::FG(),
                border: Border {
                    color: c::MAGENTA(),
                    width: 1.0,
                    radius: Radius::from(22.0),
                },
                shadow: Shadow::default(),
            }
        });

        // Anchor the pill bottom-right without disturbing grid packing: wrap it
        // in a full-size container aligned to the bottom-right, then stack it
        // over the grid.
        let pill_layer = container(pill)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Bottom)
            .padding(18);

        stack![grid, pill_layer].into()
```

Note: `empty_workspace()` still returns early when `n == 0` (no tiles → no pill), matching the spec's "empty-state out of scope" non-goal.

- [ ] **Step 2: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: PASS — `Finished`, no new warnings. (If `stack!` / `Shadow` / `Padding` are not already imported at the top of `view.rs`, add them to the existing `use iced::widget::{...}` / `use iced::{...}` lines — verify against the current import block ~L25.)

- [ ] **Step 3: Run the test suite (regression check)**

Run: `cargo test`
Expected: PASS — unchanged from Task 4.

- [ ] **Step 4: Manual verification**

Run: `cargo run`
- Enter Agent View with ≥1 session.
- Confirm a rounded "+ New session" pill floats at the bottom-right of the grid, above the tiles, matching `mock.html`.
- Resize the window / add or kill sessions so the grid crosses a perfect-square boundary (e.g. 4→5 tiles): the tile layout must NOT reflow around the pill (the pill is in the overlay layer, not the tile flow).
- Click the pill → launcher opens.
- Click Start on a chosen project/worktree/agent → the launcher closes, a new tile appears and is focused.

- [ ] **Step 5: Commit**

```bash
git add src/gui/view.rs
git commit -m "feat(launcher): floating '+ New session' pill in Agent View

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: "+ New worktree…" round-trip

**Files:**
- Modify: `src/gui/update.rs` — replace the `launcher_new_worktree` stub with the round-trip; add a re-open hook after worktree creation.

**Interfaces:**
- Consumes: `App::start_add()` behavior for `InputKind::AddWorktreeName` (`src/app.rs:537-548`); the existing worktree-creation path (`submit_input` → `create_worktree`, `src/app.rs:1835`); `Grove::rebuild_wt_cache`.
- Produces: `Grove::pending_launcher_proj: Option<usize>` on the `Grove` model (state.rs) — remembers which project to re-open the launcher into after a worktree is created.

Design note: the launcher and the worktree-name input are both `Modal` variants, so only one is visible at a time. Primary approach: stash the target project in a new `Grove::pending_launcher_proj` field, open the standard worktree-name `Modal::Input`, and after `submit_modal_input` creates the worktree, re-open the launcher with the newest worktree selected. Fallback (Step 6) if the round-trip is fiddly: skip the auto-reopen and simply leave the freshly-created worktree's session running (the existing `create_worktree` behavior), documented as an acceptable degradation.

- [ ] **Step 1: Add the `pending_launcher_proj` field in `src/gui/state.rs`**

In `pub struct Grove` (after `grid_drag` ~L146), add:

```rust
    /// Set while the worktree-name input was opened from the session launcher;
    /// on successful worktree creation the launcher re-opens into this project.
    pub pending_launcher_proj: Option<usize>,
```

Initialize it in `Grove::new` (in `src/gui/update.rs`, near `grid_view: false,` ~L82):

```rust
            pending_launcher_proj: None,
```

- [ ] **Step 2: Implement `launcher_new_worktree` (replace the Task-2 stub)**

```rust
    /// "+ New worktree…": remember the launcher's project, switch to it, and
    /// open Grove's standard worktree-name input. After creation,
    /// `submit_modal_input` re-opens the launcher (see `reopen_launcher`).
    fn launcher_new_worktree(&mut self) {
        let crate::app::Modal::SessionLauncher { proj, .. } = self.app.modal else {
            return;
        };
        if proj >= self.app.store.projects.len() {
            return;
        }
        self.pending_launcher_proj = Some(proj);
        self.switch_active_project(proj);
        // Mirror the sidebar "add worktree" entry point.
        self.app.focus = crate::app::Pane::Worktrees;
        self.app.start_add();
    }

    /// Re-open the launcher after a worktree was created from it. Selects the
    /// newly-created worktree (the last non-main entry) in the stashed project.
    fn reopen_launcher(&mut self) {
        let Some(proj) = self.pending_launcher_proj.take() else {
            return;
        };
        if proj >= self.app.store.projects.len() {
            return;
        }
        self.app.refresh_available_agents();
        self.ensure_wt_cached(proj);
        let worktrees = self.launcher_worktrees(proj);
        // The newest worktree is the last entry (git lists main first).
        let wt = worktrees.len().saturating_sub(1);
        self.app.modal = crate::app::Modal::SessionLauncher {
            proj,
            wt,
            agent: 0,
            col: 1,
            skip_perms: self.app.skip_permissions_enabled(),
            label: String::new(),
        };
    }
```

Note: `self.app.focus` / `crate::app::Pane` — confirm the exact path when implementing (`Pane` is defined in `src/app.rs`; `start_add` reads `self.focus`).

- [ ] **Step 3: Call `reopen_launcher` from `submit_modal_input`**

In `submit_modal_input` (`src/gui/update.rs` ~L1755), after `self.rebuild_wt_cache();` at the end of the method (~L1766), add:

```rust
        // If the worktree-name input was launched from the session launcher,
        // and a new worktree/session was actually created, re-open the launcher.
        if self.pending_launcher_proj.is_some() {
            if matches!(self.app.modal, Modal::None) {
                self.reopen_launcher();
            } else {
                // Creation was interrupted (invalid name, init-git confirm, or
                // validation note re-showed the input): keep the target parked
                // so a later successful submit still re-opens the launcher.
            }
        }
```

Note: `create_worktree` spawns a session for the new worktree (existing behavior). That session already lands in `tile_order` via the `submit_modal_input` grid-append (~L1762). Re-opening the launcher afterward lets the user optionally start an additional agent against the same new worktree; that is consistent with the spec's round-trip intent.

- [ ] **Step 4: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: PASS — `Finished`, no new warnings.

- [ ] **Step 5: Run the test suite**

Run: `cargo test`
Expected: PASS — unchanged.

- [ ] **Step 6: Manual verification (with documented fallback)**

Run: `cargo run`
- Open the launcher in Agent View, select a git-backed project, click "+ New worktree…".
- The standard worktree-name input appears. Enter a valid name and press Enter.
- Expected primary behavior: the worktree is created (existing flow spawns its session), then the launcher re-opens with the new worktree pre-selected in column 2.
- Fallback path (accept if the re-open proves fiddly / flaky): comment out the `reopen_launcher` call added in Step 3 and instead leave `pending_launcher_proj = None` after creation (`self.pending_launcher_proj = None;`). The new worktree's session still appears as a tile; the user re-opens the launcher manually with Cmd/Ctrl+N. Document this choice in the commit body if taken.

- [ ] **Step 7: Commit**

```bash
git add src/gui/state.rs src/gui/update.rs
git commit -m "feat(launcher): '+ New worktree' round-trip re-opens launcher

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Version bump

**Files:**
- Modify: `Cargo.toml` (`version`).

- [ ] **Step 1: Bump the version**

Current value is `version = "0.26.1"`. This is a user-facing feature (new modal + entry point), so bump the minor version. Change the `[package]` `version` field:

```toml
version = "0.27.0"
```

- [ ] **Step 2: Build (updates `Cargo.lock`)**

Run: `cargo build 2>&1 | tail -3`
Expected: PASS — `Finished`; `Cargo.lock` now records `grove 0.27.0`.

- [ ] **Step 3: Run the full suite one last time**

Run: `cargo test`
Expected: PASS — all tests green, including `gui::launcher` (7 tests).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to 0.27.0 for session launcher

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**1. Spec coverage:**
- Floating "+ New session" pill, bottom-right, above tiles, no reflow → Task 5 (Steps 1, 4).
- Cmd/Ctrl+N open while grid active → Task 3 (Step 4).
- Three Miller columns Project → Worktree → Agent → Task 4 (Step 1).
- Worktree column shows branch + main/running tags + "+ New worktree…" row → Task 4 (Step 1) renders branch + `main` tag + the add row. (Running/dirty dots: the mock shows them; `Worktree` carries no session/dirty flag, so a `running` tag would require cross-referencing `app.sessions[*].wt_path`. Deferred as cosmetic — noted here; not a launch-behavior gap.)
- Agent column with skip toggle + optional label → Task 4 (Step 1).
- Footer breadcrumb + Start button + keyboard hint → Task 4 (Step 1) + `breadcrumb` from Task 1.
- Keyboard: ←/→ columns, ↑/↓ move, Enter start, Esc close → Task 3 (Step 3).
- Spawn via `spawn_session` + append to `tile_order` + focus → Task 2 (`launcher_start`).
- Agents from `available_agents`, scanned on open → Task 2 (`open_session_launcher` calls `refresh_available_agents`).
- All-project worktrees loaded lazily via `wt_cache` / `app.worktrees` → Task 2 (`open_session_launcher` pre-caches; `launcher_select_project` / nav call `ensure_wt_cached`).
- Label default convention when empty → Task 1 (`effective_label`), used in Task 2.
- Skip default = `skip_permissions_enabled()` → Task 2.
- "+ New worktree…" round-trip with fallback → Task 6.
- Version bump → Task 7.
- Default selection = active project/worktree + agent 0 → Task 2 (`open_session_launcher`).

**2. Placeholder scan:** No "TBD"/"add error handling"/"similar to Task N" left. The `launcher_new_worktree` stub in Task 2 Step 4 is explicitly labeled temporary and replaced in full in Task 6 Step 2. Every code step shows complete code. The one intentional "does not build between Task 1 and Task 2" boundary is called out with the exact expected compiler error.

**3. Type consistency:** `Modal::SessionLauncher { proj, wt, agent, col, skip_perms, label }` field names are identical across app.rs (Task 1), all `update.rs` handlers (Tasks 2, 3, 6), and view.rs (Task 4). `Msg` variant names match between state.rs (Task 1) and their `on_press`/handlers. Pure fns (`clamp`, `move_column`, `default_label`, `effective_label`, `breadcrumb`, `worktrees_for`, `nav_within_column`) keep identical signatures where referenced. `launcher_worktrees` returns `Vec<Worktree>` consistently. `pending_launcher_proj: Option<usize>` declared (Task 6 Step 1) and used (Steps 2-3).

Open follow-ups (non-blocking, noted for the implementer): (a) the worktree `running`/dirty indicator is cosmetic and deferred; (b) confirm `crate::app::Pane` / `self.app.focus` path in Task 6 Step 2 against current source; (c) if `view.rs` already imports `checkbox`/`text_input`/`stack`/`Shadow`/`Padding`, drop the local `use` lines to avoid unused-import warnings.
