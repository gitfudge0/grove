//! Workspace layout: zen/zoom, the sidebar and terminal-panel drag handles,
//! the agent grid (tile drag/zen), window resize, and the PTY-viewport/
//! grid-order helpers those all share.

use crate::gui::metrics::{
    clamp_sidebar_width, compute_pty_dims, pty_cols_for_fraction, term_portion_for_cursor,
    PTY_ZOOM_MAX, PTY_ZOOM_MIN, RAIL_W, TERM_PANEL_PORTION, TERM_PANEL_PORTION_MAX,
    TERM_PANEL_PORTION_MIN,
};
use crate::gui::state::{GridDrag, GridDragMsg, Grove, Msg, SidebarDrag};
use iced::Task;
use std::sync::atomic::Ordering;
use std::time::Duration;

impl Grove {
    /// Agent-grid tile drag-and-drop family dispatch (`Msg::GridDrag`).
    pub(super) fn on_grid_drag(&mut self, msg: GridDragMsg) -> Task<Msg> {
        match msg {
            GridDragMsg::Start(tile_idx) => return self.on_grid_drag_start(tile_idx),
            GridDragMsg::Hover(tile_idx) => self.on_grid_drag_hover(tile_idx),
            GridDragMsg::End => self.on_grid_drag_end(),
        }
        Task::none()
    }

    pub(super) fn on_animation_frame(&mut self) {
        if let Some(slide) = &self.anim.grid_slide {
            if super::shortcuts::slide_progress(slide.start, std::time::Instant::now()) >= 1.0 {
                self.anim.grid_slide = None;
            }
        }
    }

    pub(super) fn on_window_focus_changed(&mut self, focused: bool) -> Task<Msg> {
        self.window_focused = focused;
        // Regaining focus acknowledges the visible session.
        if focused {
            if let Some(i) = self.app.active_session {
                self.acknowledge_session(i);
            }
            // A window that stays idle+unfocused stops ticking (see
            // `subscription`), so a due update check would otherwise
            // stall until other activity resumes. Evaluate it here
            // too so refocus fires it promptly.
            if let Some(task) = self.maybe_check_updates_due() {
                return task;
            }
        }
        Task::none()
    }

    pub(super) fn on_window_resized(&mut self, size: iced::Size) {
        self.pty_layout.window_size = iced::Size::new(
            size.width * self.pty_layout.zoom,
            size.height * self.pty_layout.zoom,
        );
        // Keep the sidebar inside the window's bounds (it may now be too
        // wide for a shrunken window). `size` is already logical.
        self.sidebar_width = clamp_sidebar_width(self.sidebar_width, size.width);
        self.refresh_pty_viewport();
    }

    pub(super) fn on_toggle_zen(&mut self) -> Task<Msg> {
        if !self.app.chrome_visible {
            // Exiting zen.
            self.app.chrome_visible = true;
            if self.grid_view_before_zen {
                // Zen was entered from grid view: restore grid.
                self.grid_view = true;
                self.grid_view_before_zen = false;
                // Anything that emptied `tile_order` while zenned (a kill,
                // a grid toggle) would restore a blank grid with dead keys.
                if self.tile_order.is_empty() {
                    self.enter_grid();
                }
            }
            self.refresh_pty_viewport();
        } else if self.grid_view {
            // Entering zen from the grid: focus the selected tile so zen
            // shows that one session, matching the tile's zen button.
            if let Some(si) = self
                .grid_focused
                .or(self.app.active_session)
                .or_else(|| self.tile_order.first().copied())
            {
                self.on_grid_tile_zen(si);
                return Task::none();
            }
            // An empty grid has no tile to zen into. Still drop out of grid
            // the way `on_grid_tile_zen` does, so zen never stacks on top of
            // a chrome-less grid; exiting zen restores it.
            self.grid_view = false;
            self.grid_view_before_zen = true;
            self.app.chrome_visible = false;
            self.refresh_pty_viewport();
        } else {
            // Entering zen from the single-session workspace: the active
            // session is already focused, just hide the chrome.
            self.app.chrome_visible = false;
            self.refresh_pty_viewport();
        }
        Task::none()
    }

    pub(super) fn on_sidebar_drag_start(&mut self) {
        // Double-click (two presses within 350ms) resets to the default.
        let now = std::time::Instant::now();
        let double = self
            .drag
            .last_divider_press
            .is_some_and(|t| now.duration_since(t) < Duration::from_millis(350));
        if double {
            self.drag.sidebar_drag = None;
            self.drag.last_divider_press = None;
            let logical_w = self.pty_layout.window_size.width / self.pty_layout.zoom;
            self.sidebar_width = clamp_sidebar_width(RAIL_W, logical_w);
            self.refresh_pty_viewport();
            self.persist_sidebar_width();
        } else {
            self.drag.last_divider_press = Some(now);
            self.drag.sidebar_drag = Some(SidebarDrag {
                grab_offset: None,
                start_width: self.sidebar_width,
            });
        }
    }

    pub(super) fn on_sidebar_drag_move(&mut self, cursor_x: f32) {
        if let Some(drag) = self.drag.sidebar_drag {
            // The sidebar's left edge is the window's left edge, so the
            // cursor x maps directly to width; the grab offset (set on
            // the first move) absorbs an off-edge press so width doesn't
            // jump. Both are logical px (iced scale_factor == ui_zoom).
            let offset = match drag.grab_offset {
                Some(o) => o,
                None => {
                    let o = self.sidebar_width - cursor_x;
                    self.drag.sidebar_drag = Some(SidebarDrag {
                        grab_offset: Some(o),
                        start_width: drag.start_width,
                    });
                    o
                }
            };
            let logical_w = self.pty_layout.window_size.width / self.pty_layout.zoom;
            self.sidebar_width = clamp_sidebar_width(cursor_x + offset, logical_w);
            // Visual width follows live; PTY grid is recomputed on end.
        }
    }

    pub(super) fn on_sidebar_drag_end(&mut self) {
        if let Some(drag) = self.drag.sidebar_drag.take() {
            // Skip the PTY resize + persist when the width didn't move
            // (a plain click rather than a drag).
            if (self.sidebar_width - drag.start_width).abs() >= 0.5 {
                self.refresh_pty_viewport();
                self.persist_sidebar_width();
            }
        }
    }

    pub(super) fn on_term_panel_drag_start(&mut self) {
        let now = std::time::Instant::now();
        let double = self
            .drag
            .last_term_divider_press
            .is_some_and(|t| now.duration_since(t) < Duration::from_millis(350));
        if double {
            self.drag.term_panel_dragging = false;
            self.drag.last_term_divider_press = None;
            if self.term_panel_portion != TERM_PANEL_PORTION {
                self.term_panel_portion = TERM_PANEL_PORTION;
                self.refresh_pty_viewport();
            }
        } else {
            self.drag.last_term_divider_press = Some(now);
            self.drag.term_panel_dragging = true;
        }
    }

    pub(super) fn on_term_panel_drag_move(&mut self, cursor_x: f32) {
        if self.drag.term_panel_dragging {
            let logical_w = self.pty_layout.window_size.width / self.pty_layout.zoom;
            // The split divider sits at the workspace edge, so the cursor
            // x maps directly to the panel's width share. Live update;
            // PTY columns are recomputed on release.
            self.term_panel_portion =
                term_portion_for_cursor(cursor_x, logical_w, self.sidebar_width);
        }
    }

    pub(super) fn on_term_panel_drag_end(&mut self) {
        if self.drag.term_panel_dragging {
            self.drag.term_panel_dragging = false;
            self.refresh_pty_viewport();
        }
    }

    pub(super) fn on_toggle_grid_view(&mut self) {
        self.grid_view = !self.grid_view;
        // Entering/leaving grid changes which pane can own a
        // selection — drop any stale one rather than mis-render it.
        self.pty_selection = None;
        // A manual grid toggle cancels the "restore grid on zen exit" intent;
        // leaving it set would later re-enter grid with no tiles built.
        self.grid_view_before_zen = false;
        if self.grid_view {
            // A home terminal is invisible behind the tiles, and would keep
            // stealing mod+w / keystrokes from the focused tile.
            self.leave_terminal_tab();
            self.enter_grid();
        } else {
            self.exit_grid();
        }
        self.refresh_pty_viewport();
    }

    /// Build `tile_order` from the persisted order and pick the tile that
    /// takes keyboard focus. Shared by every path that shows the grid, so
    /// they can't drift (`mod+g`, the terminal toggle, the zen-exit restore).
    /// Does not set `grid_view` or reflow the PTYs — the caller owns both.
    pub(super) fn enter_grid(&mut self) {
        // Zen hides the chrome, but `mod+g` (and the terminal toggle's grid
        // restore) stay reachable there. A grid with no appbar or statusbar
        // isn't a screen `screen_from_flags` can even name — it reports Zen —
        // so showing the grid always ends zen rather than stacking the two.
        self.app.chrome_visible = true;
        let live_keys: Vec<String> = self
            .app
            .sessions
            .iter()
            .map(|s| crate::gui::launcher::session_grid_key(&s.project, &s.wt_path))
            .collect();
        self.tile_order =
            crate::gui::launcher::reconcile_tile_order(&live_keys, &self.app.store.grid_order);
        // Open with a focused tile so the directional shortcuts
        // (mod+hjkl to move focus, mod+alt+hjkl to move the tile)
        // work on the first keypress. Keep the active session's
        // tile if it has one — yanking focus elsewhere on entry
        // would be a surprise — otherwise focus the first tile.
        let focus = self
            .app
            .active_session
            .filter(|si| self.tile_order.contains(si))
            .or_else(|| self.tile_order.first().copied());
        self.grid_focused = focus;
        if let Some(si) = focus {
            self.app.active_session = Some(si);
            self.acknowledge_session(si);
        }
        self.drag.grid_drag = None;
    }

    /// Carry the focused tile into the single-session workspace and tear the
    /// grid bookkeeping down. Counterpart to [`Grove::enter_grid`]; likewise
    /// leaves `grid_view` and the PTY reflow to the caller.
    pub(super) fn exit_grid(&mut self) {
        if let Some(si) = self.grid_focused {
            self.app.active_session = Some(si);
            self.leave_terminal_tab();
            // The panel re-anchors to this session's worktree, so a stale
            // `Panel` focus would type into a different worktree's shell.
            self.reset_focused_pane();
        }
        self.persist_grid_order();
        self.tile_order.clear();
        self.grid_focused = None;
        self.drag.grid_drag = None;
    }

    /// Re-derive the grid's view of the session list after sessions were
    /// removed behind the GUI's back (project/worktree teardown mutates
    /// `app.sessions` directly and only fixes `active_session`). Without this
    /// `tile_order` keeps stale indices, so tiles render — and route
    /// keystrokes to — the wrong agent, and that order gets persisted.
    pub(in crate::gui) fn reconcile_grid_after_teardown(&mut self) {
        if !self.grid_view && !self.grid_view_before_zen {
            self.tile_order.clear();
            self.grid_focused = None;
            return;
        }
        let live_keys: Vec<String> = self
            .app
            .sessions
            .iter()
            .map(|s| crate::gui::launcher::session_grid_key(&s.project, &s.wt_path))
            .collect();
        self.tile_order =
            crate::gui::launcher::reconcile_tile_order(&live_keys, &self.app.store.grid_order);
        if self
            .grid_focused
            .is_none_or(|si| !self.tile_order.contains(&si))
        {
            self.set_grid_focus(self.tile_order.first().copied());
        }
        if self.app.active_session.is_none() {
            self.app.active_session = self.grid_focused;
        }
        if self.grid_view {
            if self.tile_order.is_empty() {
                // Nothing left to tile — fall back to the normal workspace.
                self.grid_view = false;
            }
            self.refresh_pty_viewport();
        }
    }

    pub(super) fn on_grid_drag_start(&mut self, tile_idx: usize) -> Task<Msg> {
        if tile_idx >= self.tile_order.len() {
            return Task::none();
        }
        let si = self.tile_order[tile_idx];
        self.set_grid_focus(Some(si));
        self.app.active_session = Some(si);
        self.acknowledge_session(si);
        self.drag.grid_drag = Some(GridDrag {
            source_idx: tile_idx,
            hover_idx: tile_idx,
        });
        Task::none()
    }

    pub(super) fn on_grid_drag_hover(&mut self, tile_idx: usize) {
        if let Some(drag) = &mut self.drag.grid_drag {
            drag.hover_idx = tile_idx;
        }
        // No-op when no drag is active (on_enter always fires).
    }

    pub(super) fn on_grid_drag_end(&mut self) {
        if let Some(drag) = self.drag.grid_drag.take() {
            let src = drag.source_idx;
            let dst = drag.hover_idx;
            if src != dst && src < self.tile_order.len() && dst < self.tile_order.len() {
                crate::gui::launcher::swap_tiles(&mut self.tile_order, src, dst);
                self.begin_grid_slide(src, dst);
                self.persist_grid_order();
                // Every tile between src and dst may have changed column, so re-size each tile's PTY to its new column height.
                self.refresh_pty_viewport();
            }
        }
    }

    pub(super) fn on_grid_tile_zen(&mut self, si: usize) {
        self.app.active_session = Some(si);
        self.leave_terminal_tab();
        self.grid_focused = Some(si);
        self.acknowledge_session(si);
        // Switching workspace shape invalidates any tile selection.
        self.pty_selection = None;
        // Temporarily exit grid so zen has a single-session workspace.
        self.grid_view = false;
        self.grid_view_before_zen = true;
        self.app.chrome_visible = false;
        self.refresh_pty_viewport();
    }

    /// Records a slide animation for the two tiles that just swapped places
    /// in `tile_order`, so `grid_workspace` can translate their drawing back
    /// toward where they came from and ease it out to zero. Must be called
    /// AFTER `swap_tiles`, so `src`/`dst` are the tile-order indices the two
    /// tiles now occupy (post-swap).
    pub(super) fn begin_grid_slide(&mut self, src: usize, dst: usize) {
        let n = self.tile_order.len();
        let (cols, _) = crate::gui::metrics::grid_layout(n);
        let cols = cols.max(1);
        let cell = |i: usize| ((i % cols) as i32, (i / cols) as i32);
        let (src_col, src_row) = cell(src);
        let (dst_col, dst_row) = cell(dst);
        self.anim.grid_slide = Some(crate::gui::state::GridSlide {
            tiles: [
                (dst, src_col - dst_col, src_row - dst_row),
                (src, dst_col - src_col, dst_row - src_row),
            ],
            start: std::time::Instant::now(),
        });
    }

    pub(in crate::gui) fn invalidate_pty_render_cache(&mut self) {
        self.pty_cache.borrow_mut().clear();
        for s in &self.app.sessions {
            s.dirty.store(true, Ordering::Relaxed);
        }
        for s in &self.app.home_terminals {
            s.dirty.store(true, Ordering::Relaxed);
        }
        for v in self.app.wt_terminals.values() {
            for s in v {
                s.dirty.store(true, Ordering::Relaxed);
            }
        }
    }

    pub(in crate::gui) fn refresh_pty_viewport(&mut self) {
        if self.grid_view {
            let total = self.tile_order.len();
            let n = total.max(1);
            let (grid_cols, _) = crate::gui::metrics::grid_layout(n);
            // All columns are equal width, so the cell width is uniform.
            let tile_cols = crate::gui::metrics::grid_tile_cols(
                self.pty_layout.window_size.width,
                self.pty_layout.zoom,
                n,
            );
            // Height is per-column: a tile's PTY rows depend on how many tiles
            // share its column (column `p % grid_cols` for tile-order slot `p`),
            // so the lone tile in a short column fills the full workspace height.
            for (p, &si) in self.tile_order.iter().enumerate() {
                let col = p % grid_cols;
                let tiles_in_col = (total - 1 - col) / grid_cols + 1;
                let tile_rows = crate::gui::metrics::grid_tile_rows_for_col(
                    self.pty_layout.window_size.height,
                    self.pty_layout.zoom,
                    tiles_in_col,
                );
                if let Some(s) = self.app.sessions.get_mut(si) {
                    s.resize(tile_rows, tile_cols);
                }
            }
            self.invalidate_pty_render_cache();
            return;
        }
        let (rows, cols) = compute_pty_dims(
            self.pty_layout.window_size.width,
            self.pty_layout.window_size.height,
            self.pty_layout.zoom,
            self.app.chrome_visible,
            self.sidebar_width,
        );
        self.pty_layout.rows = rows;
        self.pty_layout.cols = cols;
        // When the slide-over panel is open the workspace splits 65/35, so the
        // agent PTY and the panel PTY each see a narrower width than the full
        // workspace. Compute both so every shell wraps at its rendered width.
        let (sess_cols, panel_cols) = if self.term_panel_open {
            let panel = self.term_panel_portion as f32 / 100.0;
            (
                pty_cols_for_fraction(
                    self.pty_layout.window_size.width,
                    self.pty_layout.zoom,
                    self.app.chrome_visible,
                    1.0 - panel,
                    self.sidebar_width,
                ),
                pty_cols_for_fraction(
                    self.pty_layout.window_size.width,
                    self.pty_layout.zoom,
                    self.app.chrome_visible,
                    panel,
                    self.sidebar_width,
                ),
            )
        } else {
            (cols, cols)
        };
        self.pty_layout.sess_cols = sess_cols;
        self.pty_layout.panel_cols = panel_cols;
        for s in &mut self.app.sessions {
            s.resize(rows, sess_cols);
        }
        // Home terminals live on their own full-width tab, never beside the panel.
        for s in &mut self.app.home_terminals {
            s.resize(rows, cols);
        }
        for v in self.app.wt_terminals.values_mut() {
            for s in v {
                s.resize(rows, panel_cols);
            }
        }
        self.invalidate_pty_render_cache();
    }

    pub(super) fn persist_sidebar_width(&mut self) {
        self.app.store.sidebar_width = Some(self.sidebar_width);
        grove_core::storage::persist(&self.app.store);
    }

    /// Save the current `tile_order` to `Store::grid_order` (mapped through
    /// each tile's stable session key) so Agent View reopens in the same
    /// arrangement, including across app restarts.
    pub(in crate::gui) fn persist_grid_order(&mut self) {
        self.app.store.grid_order = self
            .tile_order
            .iter()
            .filter_map(|&si| self.app.sessions.get(si))
            .map(|s| crate::gui::launcher::session_grid_key(&s.project, &s.wt_path))
            .collect();
        grove_core::storage::persist(&self.app.store);
    }

    pub(super) fn adjust_ui_zoom(&mut self, delta: f32) {
        self.set_ui_zoom(self.pty_layout.zoom + delta);
    }

    pub(super) fn set_ui_zoom(&mut self, zoom: f32) {
        let clamped = zoom.clamp(PTY_ZOOM_MIN, PTY_ZOOM_MAX);
        let snapped = ((clamped * 10.0).round() / 10.0).clamp(PTY_ZOOM_MIN, PTY_ZOOM_MAX);
        if (snapped - self.pty_layout.zoom).abs() < f32::EPSILON {
            return;
        }
        self.pty_layout.zoom = snapped;
        self.refresh_pty_viewport();
        self.app.store.ui_zoom = Some(snapped);
        // Debounced: `Msg::Tick` performs the actual `storage::save` once
        // `ZOOM_SAVE_QUIET_TICKS` ticks pass without another zoom change.
        // `Msg::ZoomIn`/`ZoomOut` fire per wheel-tick/held-keypress, so
        // writing (and renaming) a file on every one of those on the UI
        // thread is wasteful for a value that's purely cosmetic.
        //
        // ponytail: a crash between the last zoom change and the debounced
        // flush loses that one zoom setting. Acceptable — it's a cosmetic
        // preference, not data. `flush_ui_zoom_save` also runs on every
        // process-terminating path (close-request, quit-confirm, restart)
        // so a normal exit doesn't lose it.
        self.pty_layout.zoom_save_countdown = Some(super::ZOOM_SAVE_QUIET_TICKS);
    }

    /// Write out a pending debounced `ui_zoom` change immediately, if any.
    /// Called from the `Msg::Tick` handler once the quiet period elapses,
    /// and from every process-terminating path (`Msg::CloseRequested`,
    /// `ConfirmKind::Quit`, `Msg::Upgrade(UpgradeMsg::RestartApp)`) so exiting mid-pinch, or
    /// restarting after a self-update, doesn't drop the last value.
    pub(super) fn flush_ui_zoom_save(&mut self) {
        if self.pty_layout.zoom_save_countdown.is_some() {
            self.pty_layout.zoom_save_countdown = None;
            grove_core::storage::persist(&self.app.store);
        }
    }

    /// Grow (`delta > 0`) or shrink the terminal panel by `delta` percent of the
    /// workspace, clamped to `[TERM_PANEL_PORTION_MIN, TERM_PANEL_PORTION_MAX]`,
    /// then reflow every PTY to its new width.
    pub(super) fn adjust_term_panel_portion(&mut self, delta: i16) {
        let next = (self.term_panel_portion as i16 + delta)
            .clamp(TERM_PANEL_PORTION_MIN as i16, TERM_PANEL_PORTION_MAX as i16)
            as u16;
        if next == self.term_panel_portion {
            return;
        }
        self.term_panel_portion = next;
        self.refresh_pty_viewport();
    }
}
