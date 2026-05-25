//! PTY-canvas rendering: row-snapshot building, the `canvas::Program`
//! implementation, selection painting, and vt100 → iced colour mapping.

use super::metrics::{CELL_H, CELL_W, FONT_SIZE, MONO_FONT};
use super::palette as c;
use super::state::{Msg, PtyCell, StyledRun};
use iced::widget::canvas::{self, Frame, Geometry};
use iced::{mouse, Color, Font, Pixels, Point, Rectangle, Renderer, Size, Theme};
use std::sync::Arc;

/// Build a single row's styled runs directly from the vt100 screen.
pub fn rebuild_row_runs(screen: &vt100::Screen, row: u16, cols: u16) -> Vec<StyledRun> {
    let mut runs: Vec<StyledRun> = Vec::new();
    let mut buf = String::new();
    let mut cur_fg: Option<Color> = None;
    let mut cur_bg: Option<Color> = None;
    let mut cur_bold = false;
    let mut started = false;

    for col in 0..cols {
        let (ch, fg, bg, bold) = match screen.cell(row, col) {
            Some(cell) => {
                let ch = cell.contents().chars().next().unwrap_or(' ');
                let mut fg = vt_color_opt(cell.fgcolor());
                let mut bg = vt_color_opt(cell.bgcolor());
                if cell.inverse() {
                    std::mem::swap(&mut fg, &mut bg);
                    if fg.is_none() {
                        fg = Some(c::BG());
                    }
                    if bg.is_none() {
                        bg = Some(c::FG());
                    }
                }
                (ch, fg, bg, cell.bold())
            }
            None => (' ', None, None, false),
        };
        if !started || fg != cur_fg || bg != cur_bg || bold != cur_bold {
            if !buf.is_empty() {
                runs.push(StyledRun {
                    text: std::mem::take(&mut buf),
                    fg: cur_fg,
                    bg: cur_bg,
                    bold: cur_bold,
                });
            }
            cur_fg = fg;
            cur_bg = bg;
            cur_bold = bold;
            started = true;
        }
        buf.push(ch);
    }
    if !buf.is_empty() {
        runs.push(StyledRun {
            text: buf,
            fg: cur_fg,
            bg: cur_bg,
            bold: cur_bold,
        });
    }
    runs
}

/// Custom `canvas::Program` that paints PTY cells directly using
/// `fill_rectangle` and `fill_text`. Bypasses iced's text layout pipeline.
pub struct PtyProgram {
    pub rows: Arc<Vec<Vec<StyledRun>>>,
    pub cache: Arc<canvas::Cache>,
    pub selection: Option<(PtyCell, PtyCell)>,
    /// Terminal cursor position (row, col). `None` when the running program
    /// has hidden the cursor (e.g. vim, htop manage their own cursor).
    pub cursor: Option<(u16, u16)>,
    /// Whether the cursor should be visible in this frame (drives blinking).
    pub cursor_visible: bool,
}

#[derive(Default)]
pub struct PtyProgramState {
    dragging: bool,
}

impl canvas::Program<Msg> for PtyProgram {
    type State = PtyProgramState;

    fn update(
        &self,
        state: &mut PtyProgramState,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Msg>) {
        let local = |cursor: mouse::Cursor| -> Option<Point> {
            cursor.position_in(bounds).or_else(|| {
                cursor.position().map(|p| {
                    Point::new(
                        (p.x - bounds.x).clamp(0.0, bounds.width.max(0.0)),
                        (p.y - bounds.y).clamp(0.0, bounds.height.max(0.0)),
                    )
                })
            })
        };
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(p) = cursor.position_in(bounds) {
                    state.dragging = true;
                    return (
                        canvas::event::Status::Captured,
                        Some(Msg::PtyMouseDown(p.x, p.y)),
                    );
                }
                (canvas::event::Status::Ignored, None)
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => {
                if let Some(p) = local(cursor) {
                    return (
                        canvas::event::Status::Captured,
                        Some(Msg::PtyMouseDrag(p.x, p.y)),
                    );
                }
                (canvas::event::Status::Ignored, None)
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.dragging =>
            {
                state.dragging = false;
                (canvas::event::Status::Captured, Some(Msg::PtyMouseUp))
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if let Some(p) = cursor.position_in(bounds) {
                    let dy = match delta {
                        mouse::ScrollDelta::Lines { y, .. } => y,
                        mouse::ScrollDelta::Pixels { y, .. } => y,
                    };
                    if dy.abs() < f32::EPSILON {
                        return (canvas::event::Status::Ignored, None);
                    }
                    return (
                        canvas::event::Status::Captured,
                        Some(Msg::PtyScroll {
                            up: dy > 0.0,
                            x: p.x,
                            y: p.y,
                        }),
                    );
                }
                (canvas::event::Status::Ignored, None)
            }
            _ => (canvas::event::Status::Ignored, None),
        }
    }

    fn mouse_interaction(
        &self,
        _state: &PtyProgramState,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        _state: &PtyProgramState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<Renderer>> {
        let bold_font = Font {
            weight: iced::font::Weight::Bold,
            ..MONO_FONT
        };
        let geom = self
            .cache
            .draw(renderer, bounds.size(), |frame: &mut Frame| {
                for (r_i, row) in self.rows.iter().enumerate() {
                    let y = r_i as f32 * CELL_H;
                    let mut col_i: usize = 0;
                    for run in row {
                        let n = run.text.chars().count();
                        let x = col_i as f32 * CELL_W;
                        let w = n as f32 * CELL_W;
                        if let Some(bg) = run.bg {
                            frame.fill_rectangle(Point::new(x, y), Size::new(w, CELL_H), bg);
                        }
                        frame.fill_text(canvas::Text {
                            content: run.text.clone(),
                            position: Point::new(x, y),
                            color: run.fg.unwrap_or(c::FG()),
                            size: Pixels(FONT_SIZE),
                            line_height: iced::widget::text::LineHeight::Absolute(Pixels(CELL_H)),
                            font: if run.bold { bold_font } else { MONO_FONT },
                            horizontal_alignment: iced::alignment::Horizontal::Left,
                            vertical_alignment: iced::alignment::Vertical::Top,
                            shaping: iced::widget::text::Shaping::Advanced,
                        });
                        col_i += n;
                    }
                }
            });
        let mut out = vec![geom];
        if let Some((a, h)) = self.selection {
            let cols = self
                .rows
                .first()
                .map(|r| r.iter().map(|run| run.text.chars().count()).sum::<usize>())
                .unwrap_or(0);
            let rows = self.rows.len();
            let mut overlay = Frame::new(renderer, bounds.size());
            paint_selection(&mut overlay, a, h, rows, cols);
            out.push(overlay.into_geometry());
        }
        if self.cursor_visible {
            if let Some((crow, ccol)) = self.cursor {
                let mut cursor_frame = Frame::new(renderer, bounds.size());
                let x = ccol as f32 * CELL_W;
                let y = crow as f32 * CELL_H;
                cursor_frame.fill_rectangle(
                    Point::new(x, y),
                    Size::new(CELL_W, CELL_H),
                    c::FG(),
                );
                out.push(cursor_frame.into_geometry());
            }
        }
        out
    }
}

fn paint_selection(frame: &mut Frame, a: PtyCell, h: PtyCell, rows: usize, cols: usize) {
    if rows == 0 || cols == 0 {
        return;
    }
    let (r1, c1, r2, c2) = normalize_selection(a, h);
    let r1 = r1.min(rows - 1);
    let r2 = r2.min(rows - 1);
    let c1 = c1.min(cols);
    let c2 = c2.min(cols);
    let color = Color {
        r: 0.40,
        g: 0.50,
        b: 0.78,
        a: 0.35,
    };
    if r1 == r2 {
        let x = c1 as f32 * CELL_W;
        let y = r1 as f32 * CELL_H;
        let w = ((c2.saturating_sub(c1)).max(1)) as f32 * CELL_W;
        frame.fill_rectangle(Point::new(x, y), Size::new(w, CELL_H), color);
        return;
    }
    let row_w = cols as f32 * CELL_W;
    let x1 = c1 as f32 * CELL_W;
    let y1 = r1 as f32 * CELL_H;
    frame.fill_rectangle(
        Point::new(x1, y1),
        Size::new((row_w - x1).max(CELL_W), CELL_H),
        color,
    );
    if r2 > r1 + 1 {
        let ym = (r1 + 1) as f32 * CELL_H;
        let hm = (r2 - r1 - 1) as f32 * CELL_H;
        frame.fill_rectangle(Point::new(0.0, ym), Size::new(row_w, hm), color);
    }
    let y2 = r2 as f32 * CELL_H;
    let w2 = c2 as f32 * CELL_W;
    if w2 > 0.0 {
        frame.fill_rectangle(Point::new(0.0, y2), Size::new(w2, CELL_H), color);
    }
}

pub fn normalize_selection(a: PtyCell, b: PtyCell) -> (usize, usize, usize, usize) {
    if (a.row, a.col) <= (b.row, b.col) {
        (a.row, a.col, b.row, b.col)
    } else {
        (b.row, b.col, a.row, a.col)
    }
}

fn vt_color_opt(c: vt100::Color) -> Option<Color> {
    match c {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(ansi_idx(i)),
        vt100::Color::Rgb(r, g, b) => Some(Color::from_rgb8(r, g, b)),
    }
}

fn ansi_idx(i: u8) -> Color {
    match i {
        0 => c::BG_STRIP(),
        1 | 9 => c::RED(),
        2 | 10 => c::GREEN(),
        3 | 11 => c::YELLOW(),
        4 | 12 => c::BLUE(),
        5 | 13 => c::MAGENTA(),
        6 | 14 => c::CYAN(),
        7 | 15 => c::FG(),
        8 => c::FG_MUTE(),
        16..=231 => {
            // 6×6×6 cube
            let n = i - 16;
            let r = n / 36;
            let g = (n % 36) / 6;
            let b = n % 6;
            let v = |x: u8| -> u8 {
                if x == 0 {
                    0
                } else {
                    55 + 40 * x
                }
            };
            Color::from_rgb8(v(r), v(g), v(b))
        }
        232..=255 => {
            let v = 8 + 10 * (i - 232);
            Color::from_rgb8(v, v, v)
        }
    }
}
