//! Grid and Zen transitions, tile focus, and keyboard and pointer resizing.

use super::*;

impl WorkspaceState {
    /// Shared by every path that shows the grid (`mod+g`, terminal toggle, zen-exit restore) so they can't drift; does not set `grid_view` — the caller owns it (`layout.rs:222-252`).
    pub fn enter_grid(&mut self, live: &[LiveTile], saved: &[String]) {
        self.chrome_visible = true;
        let keys: Vec<String> = live.iter().map(|t| t.key.clone()).collect();
        self.tile_order = crate::grid::reconcile_tile_order(&keys, saved)
            .into_iter()
            .filter_map(|i| live.get(i).map(|t| t.id))
            .collect();
        let focus = self
            .active_session
            .filter(|id| self.tile_order.contains(id))
            .or_else(|| self.tile_order.first().copied());
        self.grid_focused = focus;
        if let Some(id) = focus {
            self.set_active_session(Some(id));
            self.acknowledge(id);
        }
        self.grid_pointer_drag = None;
        self.grid_resize_mode = None;
        self.grid_sizing.ensure_topology(self.tile_order.len());
    }

    /// Counterpart to [`Self::enter_grid`]; likewise leaves `grid_view` to the caller (`layout.rs:257-269`).
    pub fn exit_grid(&mut self) {
        if let Some(id) = self.grid_focused {
            self.set_active_session(Some(id));
            self.leave_terminal_tab();
            // The panel re-anchors to this session's worktree, so a stale `Panel` focus would type into a different worktree's shell.
            self.reset_focused_pane();
        }
        self.pending_grid_persist = Some(self.tile_order.clone());
        self.tile_order.clear();
        self.grid_focused = None;
        self.grid_pointer_drag = None;
        self.grid_resize_mode = None;
    }

    /// `mod+g`. Port of `on_toggle_grid_view` (`layout.rs:199-216`).
    pub fn toggle_grid(&mut self, live: &[LiveTile], saved: &[String]) {
        self.grid_view = !self.grid_view;
        self.grid_view_before_zen = false;
        if self.grid_view {
            self.leave_terminal_tab();
            self.enter_grid(live, saved);
        } else {
            self.exit_grid();
        }
    }

    /// Port of `on_toggle_zen` (`layout.rs:63-103`), all four branches.
    pub fn toggle_zen(&mut self, live: &[LiveTile], saved: &[String]) {
        if !self.chrome_visible {
            self.chrome_visible = true;
            if self.grid_view_before_zen {
                self.grid_view = true;
                self.grid_view_before_zen = false;
                if self.tile_order.is_empty() {
                    self.enter_grid(live, saved);
                }
            }
        } else if self.grid_view {
            if let Some(id) = self
                .grid_focused
                .or(self.active_session)
                .or_else(|| self.tile_order.first().copied())
            {
                self.tile_zen(id);
                return;
            }
            self.grid_view = false;
            self.grid_view_before_zen = true;
            self.chrome_visible = false;
            self.grid_pointer_drag = None;
            self.grid_resize_mode = None;
        } else {
            self.chrome_visible = false;
        }
    }

    /// A tile's own zen button (`layout.rs:344-356`).
    pub fn tile_zen(&mut self, id: SessionId) {
        self.set_active_session(Some(id));
        self.leave_terminal_tab();
        self.grid_focused = Some(id);
        self.acknowledge(id);
        self.grid_view = false;
        self.grid_view_before_zen = true;
        self.chrome_visible = false;
        self.grid_pointer_drag = None;
        self.grid_resize_mode = None;
    }

    /// Point `grid_focused` at a (possibly different) tile (`update/mod.rs:1061-1068`; the selection half is the view's).
    pub fn set_grid_focus(&mut self, focus: Option<SessionId>) {
        if self.grid_focused != focus {
            // The resize context is defined relative to the focused tile. Exiting on a focus
            // change is less surprising than leaving its selected split pointed at the old tile.
            self.grid_resize_mode = None;
        }
        self.grid_focused = focus;
    }

    /// `update/mod.rs:1052-1056`.
    pub fn sync_grid_focus(&mut self) {
        if crate::grid::should_sync_grid_focus(self.grid_view, self.grid_view_before_zen) {
            self.set_grid_focus(self.active_session);
        }
    }

    /// Grid-only; no-ops if there's nothing to focus or the move would fall off the edge of the tile layout (`update/mod.rs:1071-1094`).
    pub fn grid_move(&mut self, dx: i32, dy: i32) {
        if self.tile_order.is_empty() || self.grid_resize_mode.is_some() {
            return;
        }
        self.leave_terminal_tab();
        let cur = self
            .grid_focused
            .and_then(|id| self.tile_order.iter().position(|&x| x == id));
        let Some(pos) = cur else {
            let id = self.tile_order[0];
            self.set_active_session(Some(id));
            self.sync_grid_focus();
            self.acknowledge(id);
            return;
        };
        let Some(target) = crate::grid::grid_neighbor(pos, self.tile_order.len(), dx, dy) else {
            return;
        };
        let id = self.tile_order[target];
        self.set_active_session(Some(id));
        self.sync_grid_focus();
        self.acknowledge(id);
    }

    /// Leaves `grid_focused`/`active_session` untouched — both hold a session, not a tile-order position, so focus stays on the same session after its tile moves (`update/mod.rs:1102-1116`).
    pub fn grid_swap(&mut self, dx: i32, dy: i32) {
        if self.grid_resize_mode.is_some() {
            return;
        }
        let Some(pos) = self
            .grid_focused
            .and_then(|id| self.tile_order.iter().position(|&x| x == id))
        else {
            return;
        };
        let Some(target) = crate::grid::grid_neighbor(pos, self.tile_order.len(), dx, dy) else {
            return;
        };
        crate::grid::swap_tiles(&mut self.tile_order, pos, target);
        self.grid_slide = Some(GridSlide {
            tiles: crate::grid::slide_offsets(pos, target, self.tile_order.len()),
            start: std::time::Instant::now(),
        });
        self.pending_grid_persist = Some(self.tile_order.clone());
    }

    /// A press on a tile: focus it, make it active, acknowledge, and arm the drag (`layout.rs:308-321`).
    pub fn grid_drag_start(&mut self, tile_idx: usize) {
        let Some(&id) = self.tile_order.get(tile_idx) else {
            return;
        };
        self.set_grid_focus(Some(id));
        self.set_active_session(Some(id));
        self.acknowledge(id);
        self.grid_resize_mode = None;
        self.grid_pointer_drag = Some(GridPointerDrag::Reorder(GridDrag {
            source_idx: tile_idx,
            hover_idx: tile_idx,
        }));
    }

    pub fn grid_focus_tile(&mut self, tile_idx: usize) {
        let Some(&id) = self.tile_order.get(tile_idx) else {
            return;
        };
        self.set_grid_focus(Some(id));
        self.set_active_session(Some(id));
        self.acknowledge(id);
    }

    /// A no-op when no drag is armed — the enter event fires regardless (`layout.rs:323-328`).
    pub fn grid_drag_hover(&mut self, tile_idx: usize) {
        if let Some(GridPointerDrag::Reorder(drag)) = self.grid_pointer_drag.as_mut() {
            drag.hover_idx = tile_idx;
        }
    }

    /// Releases the one pointer owner. Returns true only when a reorder changed the persisted order.
    pub fn grid_pointer_end(&mut self) -> bool {
        let Some(pointer) = self.grid_pointer_drag.take() else {
            return false;
        };
        let GridPointerDrag::Reorder(drag) = pointer else {
            return false;
        };
        let (src, dst) = (drag.source_idx, drag.hover_idx);
        if src == dst || src >= self.tile_order.len() || dst >= self.tile_order.len() {
            return false;
        }
        crate::grid::swap_tiles(&mut self.tile_order, src, dst);
        self.grid_slide = Some(GridSlide {
            tiles: crate::grid::slide_offsets(src, dst, self.tile_order.len()),
            start: std::time::Instant::now(),
        });
        self.pending_grid_persist = Some(self.tile_order.clone());
        true
    }

    /// Clears pointer ownership without committing a reorder. Used when a move arrives without
    /// the left button held, which means the platform release happened outside our listener path.
    pub fn grid_pointer_cancel(&mut self) -> bool {
        self.grid_pointer_drag.take().is_some()
    }

    /// Enters the transient keyboard context without changing tile focus or order.
    pub fn enter_grid_resize_mode(&mut self) -> bool {
        if !self.grid_view || self.tile_order.len() < 2 {
            return false;
        }
        self.grid_sizing.ensure_topology(self.tile_order.len());
        let selected = self
            .grid_focused
            .and_then(|id| {
                self.tile_order
                    .iter()
                    .position(|candidate| *candidate == id)
            })
            .and_then(|tile| {
                [(1, 0), (-1, 0), (0, 1), (0, -1)]
                    .into_iter()
                    .find_map(|(dx, dy)| {
                        crate::grid::boundary_adjacent_to_tile(tile, self.tile_order.len(), dx, dy)
                    })
            });
        self.grid_pointer_drag = None;
        self.grid_resize_mode = Some(GridResizeMode { selected });
        true
    }

    pub fn exit_grid_resize_mode(&mut self) {
        self.grid_resize_mode = None;
    }

    /// Steps the boundary in the requested direction. The caller supplies an axis-specific floor
    /// derived from the current viewport and terminal metrics.
    pub fn grid_resize_step(
        &mut self,
        dx: i32,
        dy: i32,
        percentage_points: f32,
        minimum_weight: f32,
    ) -> bool {
        if self.grid_resize_mode.is_none() || !percentage_points.is_finite() {
            return false;
        }
        let Some(tile) = self.grid_focused.and_then(|id| {
            self.tile_order
                .iter()
                .position(|candidate| *candidate == id)
        }) else {
            return false;
        };
        let Some(boundary) =
            crate::grid::boundary_adjacent_to_tile(tile, self.tile_order.len(), dx, dy)
        else {
            return false;
        };
        let selection_changed = self
            .grid_resize_mode
            .is_some_and(|mode| mode.selected != Some(boundary));
        if let Some(mode) = self.grid_resize_mode.as_mut() {
            mode.selected = Some(boundary);
        }
        let direction = if dx != 0 { dx } else { dy };
        let signed_step = if direction < 0 {
            -percentage_points
        } else {
            percentage_points
        };
        let delta = signed_step / 100.0;
        let Some(weights) = self.grid_sizing.weights_mut(boundary) else {
            return false;
        };
        crate::grid::transfer_pair(weights, boundary.boundary, delta, minimum_weight)
            || selection_changed
    }

    /// Starts a mouse resize from the current pair. Updates always refer back to this snapshot,
    /// which prevents accumulated error and jump-back after dragging against a clamp.
    pub fn grid_resize_drag_start(
        &mut self,
        boundary: GridBoundary,
        coordinate: f32,
        span_px: f32,
        minimum_weight: f32,
    ) -> bool {
        if !coordinate.is_finite() || !span_px.is_finite() || span_px <= 0.0 {
            return false;
        }
        self.grid_sizing.ensure_topology(self.tile_order.len());
        let Some(weights) = self.grid_sizing.weights(boundary) else {
            return false;
        };
        let Some(&before) = weights.get(boundary.boundary) else {
            return false;
        };
        let Some(&after) = weights.get(boundary.boundary + 1) else {
            return false;
        };
        self.grid_pointer_drag = Some(GridPointerDrag::Resize(GridResizeDrag {
            boundary,
            start_coordinate: coordinate,
            span_px,
            start_weights: (before, after),
            minimum_weight,
        }));
        true
    }

    pub fn grid_resize_drag_update(&mut self, x: f32, y: f32) -> bool {
        let Some(GridPointerDrag::Resize(drag)) = self.grid_pointer_drag else {
            return false;
        };
        let coordinate = match drag.boundary.axis {
            GridAxis::Columns => x,
            GridAxis::Rows => y,
        };
        if !coordinate.is_finite() {
            return false;
        }
        let Some(weights) = self.grid_sizing.weights_mut(drag.boundary) else {
            return false;
        };
        let Some(right) = drag.boundary.boundary.checked_add(1) else {
            return false;
        };
        if right >= weights.len() {
            return false;
        }
        weights[drag.boundary.boundary] = drag.start_weights.0;
        weights[right] = drag.start_weights.1;
        crate::grid::transfer_pair(
            weights,
            drag.boundary.boundary,
            (coordinate - drag.start_coordinate) / drag.span_px,
            drag.minimum_weight,
        )
    }

    pub fn reset_grid_boundary(&mut self, boundary: GridBoundary) -> bool {
        self.grid_pointer_drag = None;
        let Some(weights) = self.grid_sizing.weights_mut(boundary) else {
            return false;
        };
        crate::grid::reset_pair(weights, boundary.boundary)
    }

    #[must_use]
    pub fn grid_resize_label(&self) -> Option<String> {
        let mode = self.grid_resize_mode?;
        Some(match mode.selected {
            Some(GridBoundary {
                axis: GridAxis::Columns,
                boundary,
                ..
            }) => format!("columns {} / {}", boundary + 1, boundary + 2),
            Some(GridBoundary {
                axis: GridAxis::Rows,
                boundary,
                column: Some(column),
            }) => format!(
                "column {} rows {} / {}",
                column + 1,
                boundary + 1,
                boundary + 2
            ),
            Some(GridBoundary {
                axis: GridAxis::Rows,
                column: None,
                ..
            })
            | None => "choose split".to_string(),
        })
    }

    /// Re-derive the grid's view of the session list after sessions were removed behind the GUI's back (`layout.rs:276-306`).
    pub fn reconcile_after_teardown(&mut self, live: &[LiveTile], saved: &[String]) {
        if !self.grid_view && !self.grid_view_before_zen {
            self.tile_order.clear();
            self.grid_focused = None;
            self.grid_pointer_drag = None;
            self.grid_resize_mode = None;
            return;
        }
        let keys: Vec<String> = live.iter().map(|t| t.key.clone()).collect();
        self.tile_order = crate::grid::reconcile_tile_order(&keys, saved)
            .into_iter()
            .filter_map(|i| live.get(i).map(|t| t.id))
            .collect();
        if self
            .grid_focused
            .is_none_or(|id| !self.tile_order.contains(&id))
        {
            self.set_grid_focus(self.tile_order.first().copied());
        }
        if self.active_session.is_none() {
            self.set_active_session(self.grid_focused);
        }
        if self.grid_sizing.ensure_topology(self.tile_order.len()) {
            self.grid_pointer_drag = None;
            self.grid_resize_mode = None;
        }
        if self.grid_view && self.tile_order.is_empty() {
            self.grid_view = false;
        }
    }

    /// `mod+1..9` inside the grid indexes `tile_order` rather than the sidebar's visible order (`sessions.rs:396-405`); `n` is 0-based.
    pub fn select_tile_by_index(&mut self, n: usize) {
        let Some(&id) = self.tile_order.get(n) else {
            return;
        };
        self.set_active_session(Some(id));
        self.leave_terminal_tab();
        self.sync_grid_focus();
        self.acknowledge(id);
    }

    /// Shared with [`Self::toggle_terminal_tab`]'s enter branch (`update/mod.rs:1008-1013`): leaves the grid so a freshly spawned terminal is visible instead of drawn behind the tiles.
    pub fn exit_grid_for_terminal(&mut self) {
        if self.grid_view {
            self.grid_view_before_terminal = true;
            self.grid_view = false;
            self.exit_grid();
        }
    }
}
