//! PTY-canvas rendering: row-snapshot building, the `canvas::Program`
//! implementation, selection painting, and vt100 → iced colour mapping.

use super::metrics::{mono_covers, CELL_H, CELL_W, FONT_SIZE, MONO_FONT};
use super::palette as c;
use super::state::{Msg, PtyCell, PtyPane, StyledRun};
use iced::widget::canvas::{self, Frame, Geometry};
use iced::{mouse, Color, Font, Pixels, Point, Rectangle, Renderer, Size, Theme};
use std::sync::Arc;

/// Build a single row's styled runs directly from the vt100 screen.
///
/// `theme` resolves the colors used for this row: normally the global active
/// theme, but a PTY belonging to a project with a pinned "Project theme"
/// (see `Store::project_themes_enabled` / `Project::theme`) passes that
/// theme instead, so terminal *content* renders in it while app chrome stays
/// on the global theme.
pub fn rebuild_row_runs(
    screen: &vt100::Screen,
    row: u16,
    cols: u16,
    theme: &grove_core::theme::Theme,
) -> Vec<StyledRun> {
    let mut runs: Vec<StyledRun> = Vec::new();
    let mut buf = String::new();
    let mut cur_fg: Option<Color> = None;
    let mut cur_bg: Option<Color> = None;
    let mut cur_bold = false;
    let mut started = false;

    for col in 0..cols {
        let (ch, fg, bg, bold) = match screen.cell(row, col) {
            Some(cell) => {
                // `contents()` allocates a `String` per cell; blank cells are
                // the overwhelming majority of a terminal grid, so skip it for
                // them entirely.
                let ch = if cell.has_contents() {
                    cell.contents().chars().next().unwrap_or(' ')
                } else {
                    ' '
                };
                let mut fg = vt_color_opt(cell.fgcolor(), theme);
                let mut bg = vt_color_opt(cell.bgcolor(), theme);
                if cell.inverse() {
                    std::mem::swap(&mut fg, &mut bg);
                    if fg.is_none() {
                        fg = Some(c::bg_of(theme));
                    }
                    if bg.is_none() {
                        bg = Some(c::fg_of(theme));
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
    /// Which on-screen PTY this canvas is, so mouse messages carry their origin
    /// pane and `update` can route input/selection to the right session.
    pub pane: PtyPane,
    pub rows: Arc<Vec<Vec<StyledRun>>>,
    pub cache: Arc<canvas::Cache>,
    /// Separate cache for the blinking cursor block. Kept apart from `cache`
    /// so the ~2 Hz blink doesn't invalidate the screen geometry, and cleared
    /// by `pty()` only when `(cursor, cursor_visible)` actually changes — so a
    /// steady cursor draws no `Frame` at all.
    pub cursor_cache: Arc<canvas::Cache>,
    pub selection: Option<(PtyCell, PtyCell)>,
    /// Terminal cursor position (row, col). `None` when the running program
    /// has hidden the cursor (e.g. vim, htop manage their own cursor).
    pub cursor: Option<(u16, u16)>,
    /// Whether the cursor should be visible in this frame (drives blinking).
    pub cursor_visible: bool,
    /// Fallback text color for cells with no explicit fg (vt100 "default"),
    /// and the cursor block color. Resolved once per frame in `pty()` from
    /// either the global active theme or this session's project override.
    pub default_fg: Color,
    pub cursor_color: Color,
}

#[derive(Default)]
pub struct PtyProgramState {
    dragging: bool,
    /// Accumulated sub-pixel scroll delta from trackpad smooth-scroll. We
    /// only emit a wheel event each time this crosses `CELL_H`, so tmux (and
    /// other inner apps that opt into mouse reporting) don't get flooded
    /// with hundreds of wheel notches per gesture.
    scroll_accum: f32,
}

impl canvas::Program<Msg> for PtyProgram {
    type State = PtyProgramState;

    fn update(
        &self,
        state: &mut PtyProgramState,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Msg>> {
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
                    return Some(canvas::Action::publish(Msg::PtyMouseDown(
                        self.pane, p.x, p.y,
                    )));
                }
                None
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => {
                if let Some(p) = local(cursor) {
                    return Some(canvas::Action::publish(Msg::PtyMouseDrag(
                        self.pane, p.x, p.y,
                    )));
                }
                None
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.dragging =>
            {
                state.dragging = false;
                Some(canvas::Action::publish(Msg::PtyMouseUp))
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let p = cursor.position_in(bounds)?;
                match *delta {
                    mouse::ScrollDelta::Lines { y, .. } => {
                        state.scroll_accum = 0.0;
                        if y.abs() < 1.0 {
                            return Some(canvas::Action::capture());
                        }
                        Some(canvas::Action::publish(Msg::PtyScroll {
                            pane: self.pane,
                            up: y > 0.0,
                            x: p.x,
                            y: p.y,
                        }))
                    }
                    mouse::ScrollDelta::Pixels { y, .. } => {
                        if (state.scroll_accum > 0.0) != (y > 0.0) {
                            state.scroll_accum = 0.0;
                        }
                        state.scroll_accum += y;
                        let step = CELL_H;
                        if state.scroll_accum.abs() < step {
                            return Some(canvas::Action::capture());
                        }
                        let up = state.scroll_accum > 0.0;
                        state.scroll_accum -= step.copysign(state.scroll_accum);
                        Some(canvas::Action::publish(Msg::PtyScroll {
                            pane: self.pane,
                            up,
                            x: p.x,
                            y: p.y,
                        }))
                    }
                }
            }
            _ => None,
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
                        // ASCII is one char per byte, and `mono_covers` is
                        // true for all of it — the common case skips both the
                        // char-count scan and the segmentation below.
                        let ascii = run.text.is_ascii();
                        let n = if ascii {
                            run.text.len()
                        } else {
                            run.text.chars().count()
                        };
                        let x = col_i as f32 * CELL_W;
                        let w = n as f32 * CELL_W;
                        if let Some(bg) = run.bg {
                            frame.fill_rectangle(Point::new(x, y), Size::new(w, CELL_H), bg);
                        }
                        let font = if run.bold { bold_font } else { MONO_FONT };
                        let color = run.fg.unwrap_or(self.default_fg);
                        // Split the run into segments of consecutive characters
                        // the bundled mono font covers vs. doesn't. Covered
                        // segments keep fast basic shaping; uncovered ones use
                        // advanced shaping so cosmic-text falls back to a system
                        // font instead of painting tofu. Each segment is drawn
                        // at its own column, so the monospace grid never drifts.
                        let segs: Vec<(usize, String, bool)> = if ascii {
                            vec![(col_i, run.text.clone(), true)]
                        } else {
                            let mut segs: Vec<(usize, String, bool)> = Vec::new();
                            let mut idx = col_i;
                            for ch in run.text.chars() {
                                let covered = mono_covers(ch);
                                match segs.last_mut() {
                                    Some((_, s, c)) if *c == covered => s.push(ch),
                                    _ => segs.push((idx, String::from(ch), covered)),
                                }
                                idx += 1;
                            }
                            segs
                        };
                        for (start, content, covered) in segs {
                            frame.fill_text(canvas::Text {
                                content,
                                position: Point::new(start as f32 * CELL_W, y),
                                max_width: f32::INFINITY,
                                color,
                                size: Pixels(FONT_SIZE),
                                line_height: iced::widget::text::LineHeight::Absolute(Pixels(
                                    CELL_H,
                                )),
                                font,
                                align_x: iced::advanced::text::Alignment::Left,
                                align_y: iced::alignment::Vertical::Top,
                                shaping: if covered {
                                    iced::widget::text::Shaping::Basic
                                } else {
                                    iced::widget::text::Shaping::Advanced
                                },
                            });
                        }
                        col_i += n;
                    }
                }
            });
        let mut out = vec![geom];
        if let Some((a, h)) = self.selection {
            let cols = self
                .rows
                .first()
                .map(|r| {
                    r.iter()
                        .map(|run| {
                            if run.text.is_ascii() {
                                run.text.len()
                            } else {
                                run.text.chars().count()
                            }
                        })
                        .sum::<usize>()
                })
                .unwrap_or(0);
            let rows = self.rows.len();
            let mut overlay = Frame::new(renderer, bounds.size());
            paint_selection(&mut overlay, a, h, rows, cols);
            out.push(overlay.into_geometry());
        }
        if self.cursor_visible {
            if let Some((crow, ccol)) = self.cursor {
                out.push(
                    self.cursor_cache
                        .draw(renderer, bounds.size(), |frame: &mut Frame| {
                            frame.fill_rectangle(
                                Point::new(ccol as f32 * CELL_W, crow as f32 * CELL_H),
                                Size::new(CELL_W, CELL_H),
                                self.cursor_color,
                            );
                        }),
                );
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

fn vt_color_opt(c: vt100::Color, theme: &grove_core::theme::Theme) -> Option<Color> {
    match c {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(ansi_idx(i, theme)),
        vt100::Color::Rgb(r, g, b) => Some(Color::from_rgb8(r, g, b)),
    }
}

fn ansi_idx(i: u8, theme: &grove_core::theme::Theme) -> Color {
    match i {
        0 => c::bg_strip_of(theme),
        1 | 9 => c::red_of(theme),
        2 | 10 => c::green_of(theme),
        3 | 11 => c::yellow_of(theme),
        4 | 12 => c::blue_of(theme),
        5 | 13 => c::magenta_of(theme),
        6 | 14 => c::cyan_of(theme),
        7 | 15 => c::fg_of(theme),
        8 => c::fg_mute_of(theme),
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
