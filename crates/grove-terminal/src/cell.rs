//! The cell/snapshot vocabulary the model emits. Deliberately narrow: italic, underline, dim and strikethrough are dropped, matching the iced renderer.

use crate::color::TermColor;

/// `inverse` carried rather than pre-applied, so the golden harness's fg/bg swap helper can't drift from the model.
/// `Hash` is load-bearing for the renderer's per-row scene memo — must stay in lockstep with `PartialEq`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Cell {
    pub c: char,
    pub fg: TermColor,
    pub bg: TermColor,
    pub bold: bool,
    pub inverse: bool,
}

/// A full visible-grid readout: row-major, exactly `rows * cols` cells.
///
/// Wide characters occupy the lead cell; the trailing `WIDE_CHAR_SPACER` cell
/// is emitted blank so the grid stays rectangular (matching vt100's layout).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub rows: u16,
    pub cols: u16,
    pub cells: Vec<Cell>,
}

impl Snapshot {
    pub fn cell(&self, row: u16, col: u16) -> Option<&Cell> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        self.cells
            .get(row as usize * self.cols as usize + col as usize)
    }
}
