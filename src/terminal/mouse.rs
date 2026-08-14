//! Pointer geometry: scroll accumulation, hit-testing, selection coordinates, overlay rects, mouse-report encoding. Ports `src/gui/pty.rs:110-380`, `pty_input.rs:235-295`, `session.rs:990-1039`.

use grove_terminal::MouseEncoding;

/// Lines moved per wheel notch when scrolling the terminal's own view (`crates/grove-core/src/session.rs:55-56`).
pub const SCROLL_STEP: usize = 3;

/// Max scrollback retained per session (`session.rs:57-58`).
pub const SCROLLBACK_LINES: usize = 5000;

/// Hardcoded, not a theme token — spec Appendix A pins this exact constant so it stays identical across all themes.
pub const SELECTION_RGBA: (f32, f32, f32, f32) = (0.40, 0.50, 0.78, 0.35);

/// A cell in absolute coordinates; `a_row` counts upward into history (larger = older), so a selection stays on its text while the view scrolls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AbsCell {
    pub a_row: usize,
    pub col: usize,
}

/// Banks sub-cell trackpad deltas and releases a whole cell at a time, so a gesture doesn't flood tmux with hundreds of notches.
#[derive(Debug, Default)]
pub struct ScrollAccum {
    accum: f32,
}

impl ScrollAccum {
    /// `cell_h` must be zoom-scaled (`ZoomState::cell_h()`), not a bare 17.0 — at zoom 2.0 the same gesture is worth half the notches.
    pub fn feed_pixels(&mut self, dy: f32, cell_h: f32) -> Option<(bool, usize)> {
        // A direction reversal drops the banked travel so the gesture responds immediately (`pty.rs:180-183`).
        if (self.accum > 0.0) != (dy > 0.0) {
            self.accum = 0.0;
        }
        self.accum += dy;
        if !cell_h.is_finite() || cell_h <= 0.0 {
            return None;
        }
        // Sub-threshold deltas emit nothing and stay banked (`pty.rs:185-187`).
        let notches = (self.accum.abs() / cell_h).floor();
        if notches < 1.0 {
            return None;
        }
        let up = self.accum > 0.0;
        // Subtract with `copysign` and keep the remainder — release every whole notch now, no lag on a fast flick (`pty.rs:188-189`).
        self.accum -= (notches * cell_h).copysign(self.accum);
        Some((up, notches as usize))
    }

    /// Resets the pixel bank so switching devices mid-gesture can't leak a partial line; `None` for `|dy| < 1.0` (`pty.rs:167-171`).
    pub fn feed_lines(&mut self, dy: f32) -> Option<bool> {
        self.accum = 0.0;
        if dy.abs() < 1.0 {
            return None;
        }
        Some(dy > 0.0)
    }
}

/// Element-local pixels → viewport cell, clamped at the origin (`pty_input.rs:290-295`, `pixel_to_cell`).
pub fn cell_at(x: f32, y: f32, cell_w: f32, cell_h: f32) -> (u16, u16) {
    let axis = |v: f32, cell: f32| -> u16 {
        if !cell.is_finite() || cell <= 0.0 {
            return 0;
        }
        let n = (v / cell).max(0.0);
        if n.is_nan() {
            return 0;
        }
        if n >= f32::from(u16::MAX) {
            return u16::MAX;
        }
        n as u16
    };
    (axis(x, cell_w), axis(y, cell_h))
}

/// Element-local pixels → absolute selection cell, clamped into `[sb, sb + h - 1]`; `None` for a zero-height grid (`pty_input.rs:242-256`).
pub fn pixel_to_abs(
    x: f32,
    y: f32,
    cell_w: f32,
    cell_h: f32,
    h: usize,
    sb: usize,
) -> Option<AbsCell> {
    if h == 0 {
        return None;
    }
    let (col, row) = cell_at(x, y, cell_w, cell_h);
    let row = (row as usize).min(h - 1);
    Some(AbsCell {
        a_row: sb + (h - 1 - row),
        col: col as usize,
    })
}

/// Order two selection endpoints as `(r1, c1, r2, c2)` (`pty.rs:374-380`). The compare is on the `(row, col)` tuple, with a swap when reversed.
pub fn normalize_selection(a: AbsCell, b: AbsCell) -> (usize, usize, usize, usize) {
    if (a.a_row, a.col) <= (b.a_row, b.col) {
        (a.a_row, a.col, b.a_row, b.col)
    } else {
        (b.a_row, b.col, a.a_row, a.col)
    }
}

/// A selection-overlay rectangle in element-local pixels: `(x, y, w, h)`.
pub type Rect = (f32, f32, f32, f32);

/// One rect for a single row, else up to three; caller converts absolute rows to viewport rows first (`pty.rs:332-372`).
pub fn selection_rects(
    a: AbsCell,
    head: AbsCell,
    rows: usize,
    cols: usize,
    cell_w: f32,
    cell_h: f32,
) -> Vec<Rect> {
    if rows == 0 || cols == 0 {
        return Vec::new();
    }
    let (r1, c1, r2, c2) = normalize_selection(a, head);
    let r1 = r1.min(rows - 1);
    let r2 = r2.min(rows - 1);
    let c1 = c1.min(cols);
    let c2 = c2.min(cols);

    if r1 == r2 {
        let w = (c2.saturating_sub(c1)).max(1) as f32 * cell_w;
        return vec![(c1 as f32 * cell_w, r1 as f32 * cell_h, w, cell_h)];
    }

    let row_w = cols as f32 * cell_w;
    let x1 = c1 as f32 * cell_w;
    let mut out = vec![(x1, r1 as f32 * cell_h, (row_w - x1).max(cell_w), cell_h)];
    if r2 > r1 + 1 {
        out.push((
            0.0,
            (r1 + 1) as f32 * cell_h,
            row_w,
            (r2 - r1 - 1) as f32 * cell_h,
        ));
    }
    let w2 = c2 as f32 * cell_w;
    if w2 > 0.0 {
        out.push((0.0, r2 as f32 * cell_h, w2, cell_h));
    }
    out
}

/// Encode one mouse report at a 0-based, pane-relative cell (`session.rs:1003-1039`). `cb` is the button/wheel code; `press` picks the press vs release form.
pub fn encode_mouse(encoding: MouseEncoding, cb: u32, col: u16, row: u16, press: bool) -> Vec<u8> {
    match encoding {
        MouseEncoding::Sgr => format!(
            "\x1b[<{};{};{}{}",
            cb,
            col + 1,
            row + 1,
            if press { 'M' } else { 'm' }
        )
        .into_bytes(),
        // X10 tops out at coordinate 223; past that, emit nothing rather than a wrong position (parity, not a bug).
        _ => {
            if col >= 223 || row >= 223 {
                return Vec::new();
            }
            let enc = |v: u32| -> u8 { (32 + v) as u8 };
            let button = if press { cb } else { 3 };
            vec![
                0x1b,
                b'[',
                b'M',
                enc(button),
                enc(u32::from(col) + 1),
                enc(u32::from(row) + 1),
            ]
        }
    }
}

/// One notch per [`SCROLL_STEP`] lines, capped at 200 so a Shift+Home/End full-scrollback jump can't hang (`session.rs:995-997`).
pub fn scroll_notch_count(lines: usize) -> usize {
    lines.div_ceil(SCROLL_STEP).min(200)
}

/// Page size for Shift+PageUp/PageDown: the viewport height minus one line of overlap, falling back to 20 for a degenerate viewport (`session.rs:743-749`).
pub fn scroll_page_lines(rows: u16) -> usize {
    if rows > 1 {
        usize::from(rows - 1)
    } else {
        20
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::fonts::{CELL_H, CELL_W};

    #[test]
    fn sub_threshold_pixels_accumulate_silently() {
        let mut a = ScrollAccum::default();
        assert_eq!(a.feed_pixels(5.0, CELL_H), None);
        assert_eq!(a.feed_pixels(5.0, CELL_H), None);
        // 16 banked, still under one 17px cell.
        assert_eq!(a.feed_pixels(6.0, CELL_H), None);
        // 16 + 2 = 18 crosses it, and 1px stays banked.
        assert_eq!(a.feed_pixels(2.0, CELL_H), Some((true, 1)));
        assert_eq!(a.feed_pixels(15.0, CELL_H), None);
        assert_eq!(a.feed_pixels(1.0, CELL_H), Some((true, 1)));
    }

    #[test]
    fn crossing_the_threshold_keeps_the_remainder() {
        // Feeding 2.5 cells in one event must not drop 1.5 cells of travel.
        let mut a = ScrollAccum::default();
        assert_eq!(a.feed_pixels(CELL_H * 2.5, CELL_H), Some((true, 2)));
        // 0.5 cells remain banked: another 0.5 completes the third notch.
        assert_eq!(a.feed_pixels(CELL_H * 0.5, CELL_H), Some((true, 1)));
        assert_eq!(a.feed_pixels(0.0, CELL_H), None);
    }

    #[test]
    fn downward_travel_reports_down() {
        let mut a = ScrollAccum::default();
        assert_eq!(a.feed_pixels(-CELL_H * 1.2, CELL_H), Some((false, 1)));
    }

    #[test]
    fn a_direction_reversal_resets_the_bank() {
        // Reversing must respond immediately, not first pay off the opposite direction.
        let mut a = ScrollAccum::default();
        assert_eq!(a.feed_pixels(CELL_H * 0.9, CELL_H), None);
        assert_eq!(a.feed_pixels(-CELL_H * 0.9, CELL_H), None);
        // Only 0.1 more cell is needed, proving the 0.9 up was discarded.
        assert_eq!(a.feed_pixels(-CELL_H * 0.2, CELL_H), Some((false, 1)));
    }

    #[test]
    fn line_deltas_reset_the_pixel_bank() {
        let mut a = ScrollAccum::default();
        assert_eq!(a.feed_pixels(CELL_H * 0.9, CELL_H), None);
        assert_eq!(a.feed_lines(1.0), Some(true));
        // The banked 0.9 cell is gone: a fresh 0.9 must not complete a notch.
        assert_eq!(a.feed_pixels(CELL_H * 0.9, CELL_H), None);
    }

    #[test]
    fn tiny_line_deltas_are_swallowed() {
        // The spike passed these through; iced does not.
        let mut a = ScrollAccum::default();
        assert_eq!(a.feed_lines(0.5), None);
        assert_eq!(a.feed_lines(-0.99), None);
        assert_eq!(a.feed_lines(-1.0), Some(false));
    }

    #[test]
    fn the_threshold_is_zoom_scaled() {
        // At zoom 2.0 the same gesture is worth half the notches.
        let gesture = CELL_H * 4.0;
        let mut at_1x = ScrollAccum::default();
        let mut at_2x = ScrollAccum::default();
        assert_eq!(at_1x.feed_pixels(gesture, CELL_H), Some((true, 4)));
        assert_eq!(at_2x.feed_pixels(gesture, CELL_H * 2.0), Some((true, 2)));
    }

    #[test]
    fn cell_at_floors_and_clamps_at_the_origin() {
        assert_eq!(cell_at(0.0, 0.0, CELL_W, CELL_H), (0, 0));
        assert_eq!(cell_at(CELL_W * 3.9, CELL_H * 2.1, CELL_W, CELL_H), (3, 2));
        assert_eq!(cell_at(-50.0, -50.0, CELL_W, CELL_H), (0, 0));
    }

    #[test]
    fn abs_rows_count_upward_into_history() {
        // Bottom row of a 24-row viewport with no scrollback is a_row 0.
        let bottom = pixel_to_abs(0.0, CELL_H * 23.0, CELL_W, CELL_H, 24, 0);
        assert_eq!(bottom, Some(AbsCell { a_row: 0, col: 0 }));
        let top = pixel_to_abs(0.0, 0.0, CELL_W, CELL_H, 24, 0);
        assert_eq!(top, Some(AbsCell { a_row: 23, col: 0 }));
        // Scrolled back 10 lines, the same pixel is 10 rows older.
        let scrolled = pixel_to_abs(0.0, 0.0, CELL_W, CELL_H, 24, 10);
        assert_eq!(scrolled, Some(AbsCell { a_row: 33, col: 0 }));
    }

    #[test]
    fn pixel_to_abs_clamps_below_the_viewport_and_rejects_empty_grids() {
        let below = pixel_to_abs(0.0, CELL_H * 999.0, CELL_W, CELL_H, 24, 0);
        assert_eq!(below, Some(AbsCell { a_row: 0, col: 0 }));
        assert_eq!(pixel_to_abs(0.0, 0.0, CELL_W, CELL_H, 0, 0), None);
    }

    #[test]
    fn normalize_swaps_reversed_endpoints() {
        let a = AbsCell { a_row: 5, col: 2 };
        let b = AbsCell { a_row: 3, col: 9 };
        assert_eq!(normalize_selection(a, b), (3, 9, 5, 2));
        assert_eq!(normalize_selection(b, a), (3, 9, 5, 2));
        // Same row: the column breaks the tie.
        let c = AbsCell { a_row: 3, col: 1 };
        assert_eq!(normalize_selection(b, c), (3, 1, 3, 9));
    }

    #[test]
    fn single_row_selection_is_one_rect() {
        let rects = selection_rects(
            AbsCell { a_row: 2, col: 3 },
            AbsCell { a_row: 2, col: 7 },
            24,
            80,
            CELL_W,
            CELL_H,
        );
        assert_eq!(
            rects,
            vec![(3.0 * CELL_W, 2.0 * CELL_H, 4.0 * CELL_W, CELL_H)]
        );
    }

    #[test]
    fn a_zero_width_selection_still_shows_one_cell() {
        let cell = AbsCell { a_row: 0, col: 5 };
        let rects = selection_rects(cell, cell, 24, 80, CELL_W, CELL_H);
        assert_eq!(rects, vec![(5.0 * CELL_W, 0.0, CELL_W, CELL_H)]);
    }

    #[test]
    fn two_row_selection_has_no_middle_block() {
        let rects = selection_rects(
            AbsCell { a_row: 1, col: 70 },
            AbsCell { a_row: 2, col: 4 },
            24,
            80,
            CELL_W,
            CELL_H,
        );
        assert_eq!(
            rects,
            vec![
                // first row, from col 70 to end of line
                (70.0 * CELL_W, CELL_H, 10.0 * CELL_W, CELL_H),
                // last row, from beginning of line to col 4
                (0.0, 2.0 * CELL_H, 4.0 * CELL_W, CELL_H),
            ]
        );
    }

    #[test]
    fn three_plus_row_selection_adds_the_full_middle_block() {
        let rects = selection_rects(
            AbsCell { a_row: 1, col: 10 },
            AbsCell { a_row: 5, col: 2 },
            24,
            80,
            CELL_W,
            CELL_H,
        );
        assert_eq!(
            rects,
            vec![
                (10.0 * CELL_W, CELL_H, 70.0 * CELL_W, CELL_H),
                (0.0, 2.0 * CELL_H, 80.0 * CELL_W, 3.0 * CELL_H),
                (0.0, 5.0 * CELL_H, 2.0 * CELL_W, CELL_H),
            ]
        );
    }

    #[test]
    fn an_empty_grid_paints_no_overlay() {
        let cell = AbsCell { a_row: 0, col: 0 };
        assert!(selection_rects(cell, cell, 0, 80, CELL_W, CELL_H).is_empty());
        assert!(selection_rects(cell, cell, 24, 0, CELL_W, CELL_H).is_empty());
    }

    #[test]
    fn sgr_encodes_one_based_coordinates() {
        assert_eq!(
            encode_mouse(MouseEncoding::Sgr, 0, 4, 9, true),
            b"\x1b[<0;5;10M".to_vec()
        );
        assert_eq!(
            encode_mouse(MouseEncoding::Sgr, 0, 4, 9, false),
            b"\x1b[<0;5;10m".to_vec()
        );
        // Wheel up / down are cb 64 / 65.
        assert_eq!(
            encode_mouse(MouseEncoding::Sgr, 64, 0, 0, true),
            b"\x1b[<64;1;1M".to_vec()
        );
    }

    #[test]
    fn x10_is_a_six_byte_packet_with_32_offsets() {
        assert_eq!(
            encode_mouse(MouseEncoding::Default, 0, 4, 9, true),
            vec![0x1b, b'[', b'M', 32, 32 + 5, 32 + 10]
        );
        assert_eq!(
            encode_mouse(MouseEncoding::Utf8, 0, 4, 9, true),
            vec![0x1b, b'[', b'M', 32, 32 + 5, 32 + 10]
        );
    }

    #[test]
    fn x10_release_uses_button_code_three() {
        assert_eq!(
            encode_mouse(MouseEncoding::Default, 0, 0, 0, false),
            vec![0x1b, b'[', b'M', 32 + 3, 33, 33]
        );
    }

    #[test]
    fn x10_emits_nothing_past_coordinate_223() {
        // Parity behavior, not a bug.
        assert!(encode_mouse(MouseEncoding::Default, 0, 223, 0, true).is_empty());
        assert!(encode_mouse(MouseEncoding::Default, 0, 0, 223, true).is_empty());
        assert!(!encode_mouse(MouseEncoding::Default, 0, 222, 222, true).is_empty());
        // SGR has no such limit.
        assert!(!encode_mouse(MouseEncoding::Sgr, 0, 500, 500, true).is_empty());
    }

    #[test]
    fn notch_count_rounds_up_and_caps_at_200() {
        assert_eq!(scroll_notch_count(0), 0);
        assert_eq!(scroll_notch_count(1), 1);
        assert_eq!(scroll_notch_count(3), 1);
        assert_eq!(scroll_notch_count(4), 2);
        // The Shift+Home full-scrollback jump must not hang the PTY.
        assert_eq!(scroll_notch_count(SCROLLBACK_LINES), 200);
        assert_eq!(scroll_notch_count(usize::MAX), 200);
    }

    #[test]
    fn page_size_leaves_one_line_of_overlap() {
        assert_eq!(scroll_page_lines(24), 23);
        assert_eq!(scroll_page_lines(1), 20);
        assert_eq!(scroll_page_lines(0), 20);
    }
}
