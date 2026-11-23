//! Grid dimensions, resize state and pointer gesture types.

use crate::grid::{GridAxis, GridBoundary};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridDrag {
    pub source_idx: usize,
    pub hover_idx: usize,
}

/// Session-only proportions for the current column-first topology.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridSizing {
    tile_count: usize,
    topology: (usize, usize),
    column_weights: Vec<f32>,
    row_weights: Vec<Vec<f32>>,
}

impl GridSizing {
    #[must_use]
    pub(super) fn equal(tile_count: usize) -> Self {
        if tile_count == 0 {
            return Self::default();
        }
        let (cols, rows) = crate::grid::grid_layout(tile_count);
        let row_weights = (0..cols)
            .map(|column| {
                let count = (column..tile_count).step_by(cols).count();
                crate::grid::equal_weights(count)
            })
            .collect();
        Self {
            tile_count,
            topology: (cols, rows),
            column_weights: crate::grid::equal_weights(cols),
            row_weights,
        }
    }

    pub(super) fn ensure_topology(&mut self, tile_count: usize) -> bool {
        let next = if tile_count == 0 {
            (0, 0)
        } else {
            crate::grid::grid_layout(tile_count)
        };
        let row_shape: Vec<usize> = if tile_count == 0 {
            Vec::new()
        } else {
            (0..next.0)
                .map(|column| (column..tile_count).step_by(next.0).count())
                .collect()
        };
        let current_shape: Vec<usize> = self.row_weights.iter().map(Vec::len).collect();
        if self.tile_count == tile_count && self.topology == next && current_shape == row_shape {
            return false;
        }
        *self = Self::equal(tile_count);
        true
    }

    #[must_use]
    pub fn column_weights(&self) -> &[f32] {
        &self.column_weights
    }

    #[must_use]
    pub fn row_weights(&self, column: usize) -> &[f32] {
        self.row_weights.get(column).map_or(&[], Vec::as_slice)
    }

    pub(super) fn weights(&self, boundary: GridBoundary) -> Option<&[f32]> {
        match boundary.axis {
            GridAxis::Columns => Some(&self.column_weights),
            GridAxis::Rows => self.row_weights.get(boundary.column?),
        }
        .map(Vec::as_slice)
    }

    pub(super) fn weights_mut(&mut self, boundary: GridBoundary) -> Option<&mut Vec<f32>> {
        match boundary.axis {
            GridAxis::Columns => Some(&mut self.column_weights),
            GridAxis::Rows => self.row_weights.get_mut(boundary.column?),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GridResizeMode {
    pub selected: Option<GridBoundary>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridResizeDrag {
    pub boundary: GridBoundary,
    pub start_coordinate: f32,
    pub span_px: f32,
    pub start_weights: (f32, f32),
    pub minimum_weight: f32,
}

/// Exactly one grid gesture may own the root pointer listeners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GridPointerDrag {
    Reorder(GridDrag),
    Resize(GridResizeDrag),
}

#[derive(Clone, Copy, Debug)]
pub struct GridSlide {
    pub tiles: [(usize, i32, i32); 2],
    pub start: std::time::Instant,
}
