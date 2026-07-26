//! Root palette behavior: opening the launcher, live input/fuzzy-filtering,
//! activating a row (launch / drill in), the "+ new session" and
//! "switch to session" flows, per-row contextual actions (the Tab strip),
//! and building the root/typing row list (`palette_rows`).

use super::helpers::*;
use super::state::*;
use crate::app::Modal;
use crate::gui::add_project;
use crate::gui::state::{Grove, Msg as GMsg};
use crate::gui::update::scroll_to;
use crate::gui::view::launcher_palette_scrollable_id;
use grove_core::agent::Agent;
use iced::Task;

impl Grove {
    /// The worktrees backing launcher project `proj`: the live `app.worktrees`
    /// when it is the active project, else the cached list (loaded on demand).
    ///
    /// Borrows rather than cloning: this is called per rendered row, and the
    /// GUI redraws ~16×/s, so a deep clone of the worktree list per row was
    /// pure per-frame allocation.
    pub(in crate::gui) fn launcher_worktrees(&self, proj: usize) -> &[grove_core::git::Worktree] {
        if proj == self.app.proj_idx {
            &self.app.worktrees
        } else {
            self.wt_cache.get(&proj).map(Vec::as_slice).unwrap_or(&[])
        }
    }

    /// Open the command palette at root state (recents + actions). Warms the
    /// worktree cache for every project since the typing/browse-all list
    /// needs it, and refreshes `available_agents` for the same reason.
    pub(in crate::gui) fn open_session_launcher(&mut self) {
        self.app.refresh_available_agents();
        let n = self.app.store.projects.len();
        for i in 0..n {
            self.ensure_wt_cached(i);
        }
        self.set_modal(Modal::SessionLauncher);
        self.launcher = Some(LauncherState {
            input: String::new(),
            selected: 0,
            selected_identity: None,
            browse_all: false,
            options: None,
            switch: None,
            switch_identity: None,
            row_actions: None,
            settings: None,
        });
        self.set_palette_selected(0);
    }

    /// Start the session for the current options-state selection: Enter, or
    /// an agent row click (`Msg::SessionLauncher(Msg::OptionsPick)`, which sets the
    /// selection then calls this). No-op outside options state, or if the
    /// selection no longer resolves.
    pub(in crate::gui) fn launcher_start(&mut self) {
        let Some(LauncherState {
            options: Some(r), ..
        }) = self.launcher.clone()
        else {
            return;
        };
        self.launcher_launch(r.proj, r.wt, r.agent);
    }

    /// Spawn the session for `(proj, wt, agent_idx)`, close the palette, and
    /// (grid always open here) append it to `tile_order` and focus it. Shared
    /// by `launcher_activate`'s Recent/Combo case and `launcher_start`'s
    /// options path.
    pub(super) fn launcher_launch(&mut self, proj: usize, wt: usize, agent_idx: usize) {
        let Some(project) = self.app.store.projects.get(proj) else {
            return;
        };
        let pname = project.name.clone();
        let worktrees = self.launcher_worktrees(proj);
        let Some(w) = worktrees.get(wt).cloned() else {
            return;
        };
        let Some(ag) = self.app.available_agents.get(agent_idx).copied() else {
            return;
        };
        let label = crate::gui::launcher::default_label(w.is_main, &pname, &w.path);
        let args = ag.launch_args(self.app.skip_permissions_enabled());
        let before = self.session_keys();
        self.set_modal(Modal::None);
        self.launcher = None;
        // `at_end = true`: launcher sessions always land last in the sessions
        // vector so they appear at the end of the Agent View grid, even after a
        // tile_order rebuild (entering Agent View resets it to sessions order).
        let inserted =
            self.app
                .spawn_session(label, pname, w.path.clone(), ag, args, &w.path, true);
        self.resize_new_sessions(&before);
        if let Some(at) = inserted {
            self.leave_terminal_tab();
            if self.grid_view {
                crate::gui::launcher::insert_into_tile_order(&mut self.tile_order, at);
                self.persist_grid_order();
                self.set_grid_focus(Some(at));
                self.refresh_pty_viewport();
            }
        }
        self.rebuild_wt_cache();
    }

    /// Shared landing for `Msg::SessionLauncher(Msg::InputChanged)` and
    /// `Msg::SessionLauncher(Msg::InputPasted)`: writes the new query into `input`,
    /// resets the pieces of state a query edit invalidates (row-actions
    /// strip, resize mode, filtered-list cursors), and rescrolls whichever
    /// sub-view is showing. Split out so the paste path can reach this
    /// without going through `Msg::InputChanged`'s `global_mods` guard,
    /// which would otherwise swallow the paste too.
    pub(in crate::gui) fn launcher_input_changed(&mut self, s: String) -> Task<GMsg> {
        // The switch-to-session drill-in filters live by `input` (same idiom
        // as OPEN WITH's agent list, which also keeps its own state open
        // while the query underneath changes) — resolved by identity (which
        // session, not which position) before the mutable borrow below, same
        // principle as `resolve_selected` for the main list: a query edit
        // can reorder/drop rows in the filtered list, so re-anchoring by
        // position alone (the old `clamp`-based behavior) could land the
        // cursor on a different session than the one highlighted.
        let switch_open = matches!(
            &self.launcher,
            Some(LauncherState {
                switch: Some(_),
                ..
            })
        );
        let new_switch_pos = switch_open.then(|| self.resolve_switch_position(&s));
        if let Some(LauncherState {
            input,
            row_actions,
            settings,
            ..
        }) = self.launcher_modal_mut()
        {
            *input = s;
            // `row_actions` is pinned to a specific (proj, wt_path)
            // resolved from the root/typing list; once that list is
            // re-derived for the new query, the row it was anchored
            // to may no longer be rendered (or may have moved) —
            // collapse the strip rather than risk it going stale.
            *row_actions = None;
            // Settings drill-in stays open across a query edit (only
            // Esc backs out). Only the panes that actually filter on
            // the query (Root, Theme) reset their cursor — the input
            // stays focused in every pane, so a stray keystroke in
            // Backend/Permissions/DefaultAgent must not snap their
            // (unfiltered) cursor to 0. The update-actions strip
            // collapses for the same reason `row_actions` does: the
            // row it hangs under may no longer be rendered. Typing
            // also leaves App-size resize mode — the user is
            // searching now, and the filtered list may not even
            // contain the App-size row anymore.
            if let Some(st) = settings {
                if matches!(
                    st.pane,
                    SettingsPane::Root
                        | SettingsPane::Theme { .. }
                        | SettingsPane::ProjectTheme { .. }
                ) {
                    st.selected = 0;
                    st.update_actions = None;
                    st.resizing = false;
                }
            }
        }
        // Root/typed list cursor snapped to 0 for the new query —
        // capture its identity too (see `set_palette_selected`).
        self.set_palette_selected(0);
        if let Some(pos) = new_switch_pos {
            self.set_switch_selected(pos);
        }
        // Theme sub-pane / drill-in Root: the new query reshapes
        // the list and the cursor just snapped to 0 — scroll the
        // list back with it.
        if let Some(LauncherState {
            settings: Some(ls), ..
        }) = self.launcher_modal()
        {
            return match ls.pane {
                SettingsPane::Theme { .. } | SettingsPane::ProjectTheme { .. } => {
                    self.scroll_launcher_theme_to_selection()
                }
                SettingsPane::Root => self.scroll_launcher_settings_to_selection(),
                _ => Task::none(),
            };
        }
        // Root/typed list: the query edit just reshaped the list and
        // snapped the cursor to 0 above — scroll it back with it,
        // same as the Settings drill-in's Root pane.
        self.scroll_launcher_palette_to_selection()
    }

    /// Move the root/typed list's selection cursor to `idx` and capture that
    /// row's identity in the same step (`PaletteRowIdentity`). Every write
    /// site for `SessionLauncher::selected` routes through this — reads
    /// `input`/`browse_all` off the *current* modal state, so callers must
    /// apply any other field changes (new `input`, flipped `browse_all`,
    /// …) first. No-op outside `SessionLauncher`.
    pub(super) fn set_palette_selected(&mut self, idx: usize) {
        let (input, browse_all) = match self.launcher_modal() {
            Some(LauncherState {
                input, browse_all, ..
            }) => (input.clone(), *browse_all),
            _ => return,
        };
        let rows = self.palette_rows(&input, browse_all);
        let identity = rows.get(idx).map(row_identity);
        if let Some(LauncherState {
            selected,
            selected_identity,
            ..
        }) = self.launcher_modal_mut()
        {
            *selected = idx;
            *selected_identity = identity;
        }
    }

    /// The keyboard Enter/Tab activation path's index resolution: match the
    /// modal's `selected_identity` against `rows` (see
    /// `resolve_row_by_identity`) instead of trusting `selected` directly —
    /// this is what actually closes the staleness window `set_palette_
    /// selected` opens up: a background list change between the last
    /// keypress that moved the cursor and this one can't make Enter/Tab fire
    /// a different row than the one highlighted on screen. Mouse clicks
    /// (`Msg::SessionLauncher(Msg::Activate(i))`) name their row directly and skip this —
    /// the click event itself is already exact.
    pub(super) fn resolve_selected(&self, rows: &[PaletteRow]) -> Option<usize> {
        let (selected, identity) = match self.launcher_modal() {
            Some(LauncherState {
                selected,
                selected_identity,
                ..
            }) => (*selected, selected_identity.clone()),
            _ => return None,
        };
        resolve_row_by_identity(rows, &identity, selected)
    }

    /// Activate the row at `i` in the currently-rendered root/typing/
    /// browse-all list: launch a `Recent`/`Combo` row directly, or run the
    /// effect of an action row.
    pub(in crate::gui) fn launcher_activate(&mut self, i: usize) -> Task<GMsg> {
        let (input, browse_all) = match self.launcher_modal() {
            Some(LauncherState {
                input, browse_all, ..
            }) => (input.clone(), *browse_all),
            _ => return Task::none(),
        };
        let rows = self.palette_rows(&input, browse_all);
        let Some(row) = rows.get(i) else {
            return Task::none();
        };
        match row {
            PaletteRow::Recent { proj, wt_path, .. } | PaletteRow::Combo { proj, wt_path, .. } => {
                let proj = *proj;
                let agent = match row {
                    PaletteRow::Recent { agent, .. } | PaletteRow::Combo { agent, .. } => *agent,
                    _ => unreachable!(),
                };
                let Some(wt) = self
                    .launcher_worktrees(proj)
                    .iter()
                    .position(|w| &w.path == wt_path)
                else {
                    return Task::none();
                };
                let Some(agent_idx) = self.app.available_agents.iter().position(|a| *a == agent)
                else {
                    return Task::none();
                };
                self.launcher_launch(proj, wt, agent_idx);
                Task::none()
            }
            PaletteRow::NewSession => {
                if let Some(LauncherState { browse_all, .. }) = self.launcher_modal_mut() {
                    *browse_all = true;
                }
                self.set_palette_selected(0);
                Task::none()
            }
            PaletteRow::TerminalHome => {
                self.on_new_home_terminal();
                self.terminal_focused = true;
                self.set_modal(Modal::None);
                Task::none()
            }
            PaletteRow::TerminalWt => {
                let task = if !self.term_panel_open {
                    self.on_toggle_term_panel()
                } else {
                    self.on_new_wt_terminal();
                    Task::none()
                };
                self.set_modal(Modal::None);
                task
            }
            // `Msg::AddProject(Open)`'s own handler clears every child-state
            // field (including the palette) before opening the wizard, so
            // this doesn't need to pre-clear anything itself.
            PaletteRow::AddProject => self.on_add_project(add_project::Msg::Open),
            PaletteRow::SwitchToSession => {
                // Inert outside zen (row still shows, muted, with a "zen
                // only" hint — see `palette_row_view`): swallow the Enter,
                // keep the palette open, no state change.
                if self.switch_to_session_active() {
                    self.launcher_enter_switch();
                }
                Task::none()
            }
            PaletteRow::Settings => self.open_settings_drill_in(),
            PaletteRow::Setting(s) => {
                let s = *s;
                let task = self.activate_setting(s);
                self.reselect_typed_setting(s, &input, browse_all, i);
                task
            }
            PaletteRow::ReloadThemes => self.reload_themes(),
        }
    }

    /// Enter the "switch to session" drill-in: selects the first row and
    /// clears the search input so the full session list is visible
    /// immediately, rather than still filtered by whatever root/typing query
    /// was active (e.g. "swi", which is how the row itself was found).
    /// Typing afterward re-filters as normal (`Msg::SessionLauncher(Msg::InputChanged)`).
    /// Esc backs out without restoring that query — same as OPEN WITH, which
    /// doesn't touch `input` at all going in or coming out.
    pub(in crate::gui) fn launcher_enter_switch(&mut self) {
        if let Some(LauncherState { input, .. }) = self.launcher_modal_mut() {
            input.clear();
        }
        self.set_switch_selected(0);
    }

    /// Scroll the root/typed palette list so the selected row is centered —
    /// the un-drilled-in list's counterpart to `scroll_launcher_settings_
    /// to_selection`. No-op whenever a sub-state (options, switch drill-in,
    /// settings drill-in, or the row-actions strip) is showing instead of
    /// this list — see the `else` branch in `view.rs` that renders it.
    pub(super) fn scroll_launcher_palette_to_selection(&self) -> Task<GMsg> {
        use iced::widget::scrollable::AbsoluteOffset;
        let Some(LauncherState {
            input,
            selected,
            browse_all,
            options: None,
            switch: None,
            row_actions: None,
            settings: None,
            ..
        }) = self.launcher_modal()
        else {
            return Task::none();
        };
        let rows = self.palette_rows(input, *browse_all);
        let zero_projects = self.app.store.projects.is_empty();
        let root_mode = input.is_empty() && !*browse_all && !zero_projects;
        let y = palette_scroll_offset(&rows, *selected, root_mode);
        scroll_to(
            launcher_palette_scrollable_id(),
            AbsoluteOffset { x: 0.0, y },
        )
    }

    /// Tab in root/typing state: if the row at `i` is a `Recent`/`Combo`,
    /// reveal its inline contextual-action strip (Launch session…/Delete
    /// worktree); if it's `SwitchToSession`, open the switch-to-session
    /// drill-in directly (Tab behaves the same as Enter there); if it's a
    /// `Setting`/`Settings` row, Tab also mirrors Enter — enum settings
    /// extend into their sub-pane, toggles flip in place. Any other row
    /// is a no-op, same as before.
    pub(super) fn launcher_enter_row_actions(
        &mut self,
        i: usize,
        input: &str,
        browse_all: bool,
    ) -> Task<GMsg> {
        let rows = self.palette_rows(input, browse_all);
        let Some(row) = rows.get(i) else {
            return Task::none();
        };
        match row {
            PaletteRow::Recent {
                proj,
                wt_path,
                agent,
                ..
            }
            | PaletteRow::Combo {
                proj,
                wt_path,
                agent,
                ..
            } => {
                let ra = RowActionsState {
                    proj: *proj,
                    wt_path: wt_path.clone(),
                    agent: *agent,
                    action: 0,
                };
                if let Some(LauncherState { row_actions, .. }) = self.launcher_modal_mut() {
                    *row_actions = Some(ra);
                }
            }
            PaletteRow::SwitchToSession => {
                // Tab is inert outside zen too — same gate as Enter.
                if self.switch_to_session_active() {
                    self.launcher_enter_switch();
                }
            }
            PaletteRow::Settings => return self.open_settings_drill_in(),
            PaletteRow::Setting(s) => {
                let s = *s;
                let task = self.activate_setting(s);
                self.reselect_typed_setting(s, input, browse_all, i);
                return task;
            }
            _ => {}
        }
        Task::none()
    }

    /// Enter the "Launch session…" flow for `(proj, wt_path)`: the full OPEN
    /// WITH agent-picker state (`Modal::SessionLauncher::options`). Used by
    /// the row-actions strip's "Launch session…" action (action `0`); `origin`
    /// is that strip's state, restored on Esc from options.
    pub(super) fn launcher_open_options_for(
        &mut self,
        proj: usize,
        wt_path: String,
        origin: RowActionsState,
    ) {
        let Some(wt) = self
            .launcher_worktrees(proj)
            .iter()
            .position(|w| w.path == wt_path)
        else {
            return;
        };
        let pname = self
            .app
            .store
            .projects
            .get(proj)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let agent = self
            .app
            .store
            .recent_launches
            .iter()
            .find(|r| r.project == pname && r.wt_path == wt_path)
            .map(|r| r.agent)
            .unwrap_or_else(|| {
                self.app
                    .available_agents
                    .first()
                    .copied()
                    .unwrap_or(Agent::Terminal)
            });
        let agent_idx = self
            .app
            .available_agents
            .iter()
            .position(|a| *a == agent)
            .unwrap_or(0);
        if let Some(LauncherState {
            options,
            row_actions,
            ..
        }) = self.launcher_modal_mut()
        {
            *options = Some(LauncherOptions {
                proj,
                wt,
                agent: agent_idx,
                origin,
            });
            *row_actions = None;
        }
    }

    /// Build the current root or typing/browse-all row list for the command
    /// palette. Root (`input` empty, `!browse_all`): up to 6 valid recents
    /// (skipping any whose project name or worktree path no longer resolves)
    /// plus action rows. Zero projects: only `AddProject` + `TerminalHome`.
    /// Typing/browse-all: every `(proj, wt)` combo across every project,
    /// fuzzy-filtered by `input`.
    pub(super) fn palette_rows(&self, input: &str, browse_all: bool) -> Vec<PaletteRow> {
        if self.app.store.projects.is_empty() {
            return vec![PaletteRow::AddProject, PaletteRow::TerminalHome];
        }
        if input.is_empty() && !browse_all {
            let mut rows = Vec::new();
            for r in self.app.store.recent_launches.iter().take(6) {
                let Some(proj) = self
                    .app
                    .store
                    .projects
                    .iter()
                    .position(|p| p.name == r.project)
                else {
                    continue;
                };
                if !self
                    .launcher_worktrees(proj)
                    .iter()
                    .any(|w| w.path == r.wt_path)
                {
                    continue;
                }
                rows.push(PaletteRow::Recent {
                    proj,
                    wt_path: r.wt_path.clone(),
                    agent: r.agent,
                });
            }
            // No valid recents (fresh install, or every recent's project/
            // worktree since disappeared) but at least one project exists:
            // fall back to listing worktrees directly, active project first,
            // so root state is never just the bare action rows. Capped at 6
            // total, same as the recents list it replaces. `view.rs` renders
            // a "{PROJECT} — WORKTREES" header per project run (detected by
            // the same "root_mode but rows contain a Combo" signal used
            // here) and a persistent "starts a session" row hint in place of
            // the recents list's per-row digit accelerator.
            if rows.is_empty() && !self.app.available_agents.is_empty() {
                let proj_order =
                    root_project_order(self.app.store.projects.len(), self.app.proj_idx);
                let mut count = 0;
                'projects: for proj in proj_order {
                    let Some(p) = self.app.store.projects.get(proj) else {
                        continue;
                    };
                    for w in self.launcher_worktrees(proj) {
                        if count >= 6 {
                            break 'projects;
                        }
                        let agent = self
                            .app
                            .store
                            .recent_launches
                            .iter()
                            .find(|r| r.project == p.name && r.wt_path == w.path)
                            .map(|r| r.agent)
                            .unwrap_or(self.app.available_agents[0]);
                        rows.push(PaletteRow::Combo {
                            proj,
                            wt_path: w.path.clone(),
                            agent,
                        });
                        count += 1;
                    }
                }
            }
            rows.push(PaletteRow::NewSession);
            rows.push(PaletteRow::TerminalHome);
            if self.app.active_session.is_some() && !self.terminal_focused {
                rows.push(PaletteRow::TerminalWt);
            }
            rows.push(PaletteRow::AddProject);
            if self.switch_to_session_row_visible() {
                rows.push(PaletteRow::SwitchToSession);
            }
            rows.push(PaletteRow::Settings);
            rows
        } else {
            if self.app.available_agents.is_empty() {
                return Vec::new();
            }
            let mut rows = Vec::new();
            // Direct settings matches (name/value/section) go first, so the
            // typing-state SETTINGS section (see `view.rs`'s header logic)
            // prints above SESSIONS — B2 in the palette redesign mock.
            if !browse_all {
                for s in self.settings_rows_filtered(input) {
                    rows.push(PaletteRow::Setting(s));
                }
            }
            // Score every (project, worktree) combo, keep only the ones that
            // match at all, then rank by score desc with a recency tiebreak:
            // a combo whose (project, wt_path) sits earlier in
            // `recent_launches` wins a tie (`sort_by` is stable, so combos
            // absent from recents — tied at `usize::MAX` — keep their
            // relative store order).
            let mut scored: Vec<(u32, usize, PaletteRow)> = Vec::new();
            for (proj, p) in self.app.store.projects.iter().enumerate() {
                for w in self.launcher_worktrees(proj) {
                    let name = if w.branch.is_empty() {
                        crate::app::path_basename(&w.path)
                    } else {
                        w.branch.clone()
                    };
                    let agent = self
                        .app
                        .store
                        .recent_launches
                        .iter()
                        .find(|r| r.project == p.name && r.wt_path == w.path)
                        .map(|r| r.agent)
                        .unwrap_or(self.app.available_agents[0]);
                    let Some(score) =
                        crate::gui::launcher::fuzzy_score(input, &p.name, &name, agent.label())
                    else {
                        continue;
                    };
                    let recency = self
                        .app
                        .store
                        .recent_launches
                        .iter()
                        .position(|r| r.project == p.name && r.wt_path == w.path)
                        .unwrap_or(usize::MAX);
                    scored.push((
                        score,
                        recency,
                        PaletteRow::Combo {
                            proj,
                            wt_path: w.path.clone(),
                            agent,
                        },
                    ));
                }
            }
            rows.extend(rank_and_group_combos(scored));
            // Typing (not the "+ new session…" browse-all list): the ACTIONS
            // rows stay reachable by keyword, e.g. "swi" -> "Switch to
            // session…" (D5 in the palette redesign mock).
            if !browse_all {
                if crate::gui::launcher::fuzzy_match(input, "add project", "", "") {
                    rows.push(PaletteRow::AddProject);
                }
                if self.switch_to_session_row_visible()
                    && crate::gui::launcher::fuzzy_match(input, "switch to session", "", "")
                {
                    rows.push(PaletteRow::SwitchToSession);
                }
                if crate::gui::launcher::fuzzy_match(input, "settings", "", "") {
                    rows.push(PaletteRow::Settings);
                }
                if crate::gui::launcher::fuzzy_match(input, "reload themes", "", "") {
                    rows.push(PaletteRow::ReloadThemes);
                }
            }
            rows
        }
    }

    /// Whether `PaletteRow::SwitchToSession` should be *rendered* at all
    /// (root and typed lists alike): just needs a session to switch to.
    /// Outside zen the row still shows, but inert — see
    /// `switch_to_session_active`.
    pub(super) fn switch_to_session_row_visible(&self) -> bool {
        // At least one session you could actually switch *to* — the active
        // one is hidden from the drill-in list, so it doesn't count.
        (0..self.app.sessions.len()).any(|i| self.app.active_session != Some(i))
    }

    /// Whether `PaletteRow::SwitchToSession` is *actionable*: also requires
    /// zen (`!chrome_visible` — the flag `Msg::ToggleZen`/mod+enter flips).
    /// Outside zen the workspace/grid already shows every session, so
    /// opening the drill-in would be redundant there; the row still renders
    /// (muted, "in zen · ⌘⏎") but Enter/Tab on it are no-ops.
    pub(in crate::gui) fn switch_to_session_active(&self) -> bool {
        self.switch_to_session_row_visible() && !self.app.chrome_visible
    }

    /// Active sessions across every project/worktree, for the "switch to
    /// session" drill-in list: every index into `App::sessions` (in their
    /// existing display order) fuzzy-filtered by `input` against the
    /// session's project/worktree/agent — same matching used by the
    /// typing-state Combo list. Empty `input` matches everything.
    pub(super) fn switch_session_rows(&self, input: &str) -> Vec<usize> {
        (0..self.app.sessions.len())
            // Switching to the session already on screen is a no-op; hide it.
            .filter(|&i| self.app.active_session != Some(i))
            .filter(|&i| {
                let s = &self.app.sessions[i];
                let wt_name = crate::app::path_basename(&s.wt_path);
                crate::gui::launcher::fuzzy_match(input, &s.project, &wt_name, s.agent.label())
            })
            .collect()
    }

    /// Switch focus to `App::sessions[si]` and close the palette. Shared by
    /// the drill-in's Enter key and row-click paths.
    pub(in crate::gui) fn launcher_switch_to(&mut self, si: usize) -> Task<GMsg> {
        self.set_modal(Modal::None);
        self.on_select_session(si);
        Task::none()
    }

    /// Move the "switch to session" drill-in's cursor to position `idx`
    /// within `switch_session_rows(&input)` and capture that row's session
    /// `id` in the same step — same principle as `set_palette_selected`,
    /// applied to this drill-in's own list. Reads `input` off the *current*
    /// modal state, so callers must clear/update it first.
    pub(super) fn set_switch_selected(&mut self, idx: usize) {
        let input = match self.launcher_modal() {
            Some(LauncherState { input, .. }) => input.clone(),
            _ => return,
        };
        let rows = self.switch_session_rows(&input);
        let identity = rows
            .get(idx)
            .and_then(|&si| self.app.sessions.get(si))
            .map(|s| s.id);
        if let Some(LauncherState {
            switch,
            switch_identity,
            ..
        }) = self.launcher_modal_mut()
        {
            *switch = Some(idx);
            *switch_identity = identity;
        }
    }

    /// The position (within a freshly filtered `switch_session_rows(input)`)
    /// of the session at `switch_identity` — used to re-anchor the drill-in
    /// cursor after `input` itself changes (a query edit can reorder/drop
    /// rows in the filtered list, so clamping the raw position, the old
    /// behavior, could land on a different session than the one
    /// highlighted). Defaults to the top of the list when there's no
    /// identity yet, or that session no longer matches the new query.
    pub(super) fn resolve_switch_position(&self, input: &str) -> usize {
        let identity = match self.launcher_modal() {
            Some(LauncherState {
                switch_identity, ..
            }) => *switch_identity,
            _ => None,
        };
        let rows = self.switch_session_rows(input);
        match identity {
            Some(id) => rows
                .iter()
                .position(|&si| self.app.sessions.get(si).map(|s| s.id) == Some(id))
                .unwrap_or(0),
            None => 0,
        }
    }

    /// Resolve the switch drill-in's Enter target: the `App::sessions` index
    /// whose `id` matches `switch_identity`, re-derived against a fresh
    /// `switch_session_rows` rather than trusting the drill-in's raw cursor
    /// position — the same staleness hazard `resolve_selected` closes for
    /// the main list (a session closing/reordering between two keystrokes
    /// can't make Enter switch to a different session than the one
    /// highlighted). `Session::id` is the identity here rather than
    /// `PaletteRowIdentity`, since a switch-drill-in row already *is* a
    /// session, with its own stable, never-reused id (see
    /// `crate::gui::launcher::session_grid_key` for the analogous
    /// project+path key used elsewhere — an id is simpler and available
    /// here).
    pub(super) fn resolve_switch_selected(&self, input: &str) -> Option<usize> {
        let (sel, identity) = match self.launcher_modal() {
            Some(LauncherState {
                switch: Some(sel),
                switch_identity,
                ..
            }) => (*sel, *switch_identity),
            _ => return None,
        };
        let rows = self.switch_session_rows(input);
        match identity {
            Some(id) => rows
                .into_iter()
                .find(|&si| self.app.sessions.get(si).map(|s| s.id) == Some(id)),
            None => rows.get(sel).copied(),
        }
    }

    /// Run the row-actions strip action `action` for the `(proj, wt_path)`
    /// identity it's pinned to: `0` enters the existing OPEN WITH agent-picker
    /// ("Launch session…"); `1` closes the palette and routes through the
    /// sidebar's own delete-worktree confirmation flow (`Msg::DeleteWorktree`,
    /// the same message the sidebar trash icon sends — reusing it means the
    /// existing teardown-script/confirm modal applies here too, rather than
    /// duplicating that flow in the palette). Resolving straight from the
    /// identity (rather than re-deriving `palette_rows()` and indexing into
    /// it) means a query/list change since the strip was opened can't make
    /// this act on the wrong row.
    /// The project's configured lifecycle scripts, in fixed order
    /// setup/run/teardown, skipping any that are unset or blank. Indices
    /// into this vec are the row-actions strip's script actions, offset by
    /// the strip's base action count — used by the view, the keyboard-nav
    /// clamp, and the run handler so all three stay in sync.
    pub(crate) fn row_action_scripts(&self, proj: usize) -> Vec<(&'static str, String)> {
        let Some(p) = self.app.store.projects.get(proj) else {
            return Vec::new();
        };
        [
            ("setup", &p.scripts.setup),
            ("run", &p.scripts.run),
            ("teardown", &p.scripts.teardown),
        ]
        .into_iter()
        .filter_map(|(kind, script)| {
            let s = script.as_deref()?.trim();
            if s.is_empty() {
                None
            } else {
                Some((kind, s.to_string()))
            }
        })
        .collect()
    }

    pub(in crate::gui) fn launcher_run_row_action(
        &mut self,
        proj: usize,
        wt_path: String,
        agent: grove_core::agent::Agent,
        action: usize,
    ) -> Task<GMsg> {
        if action == 0 {
            let origin = RowActionsState {
                proj,
                wt_path: wt_path.clone(),
                agent,
                action: 0,
            };
            self.launcher_open_options_for(proj, wt_path, origin);
            return Task::none();
        }
        if action == 2 && self.app.project_themes_enabled() {
            return self.enter_project_theme_pane(proj);
        }
        let base = if self.app.project_themes_enabled() {
            3
        } else {
            2
        };
        if action >= base {
            let scripts = self.row_action_scripts(proj);
            let Some((kind, script)) = scripts.get(action - base) else {
                return Task::none();
            };
            let Some(pname) = self.app.store.projects.get(proj).map(|p| p.name.clone()) else {
                return Task::none();
            };
            self.set_modal(Modal::None);
            self.app.spawn_script_session(kind, pname, wt_path, script);
            return Task::none();
        }
        let Some((wt, is_main)) = self
            .launcher_worktrees(proj)
            .iter()
            .position(|w| w.path == wt_path)
            .and_then(|i| self.launcher_worktrees(proj).get(i).map(|w| (i, w.is_main)))
        else {
            return Task::none();
        };
        // The strip's second action is worktree-dependent: the project's
        // default/base checkout can't be removed (`App::start_delete` bounces
        // it to a "can't remove the project's main checkout" message), so its
        // strip offers "Create worktree…" there instead of "Delete worktree".
        if is_main {
            self.set_modal(Modal::None);
            return self.on_add_worktree(proj);
        }
        self.set_modal(Modal::None);
        self.on_delete_worktree(proj, wt);
        Task::none()
    }
}
