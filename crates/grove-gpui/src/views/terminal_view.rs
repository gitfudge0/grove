//! The focusable wrapper around [`TerminalElement`]: every input path into the
//! terminal lives here.
//!
//! Dispatch order in `on_key_down` is the **observable contract**, mirroring
//! `src/gui/update/mod.rs:780-880`:
//!
//! 1. any key press kills the selection and any in-progress drag,
//! 2. copy shortcut → clipboard, consumed,
//! 3. paste shortcut → file-URI paths, else bracketed paste, consumed,
//! 4. `keyboard_scroll_intent` → `scroll_lines`, consumed,
//! 5. otherwise `key_to_bytes` → `send`.
//!
//! Actions declared in Plan 03's keymap take precedence automatically through
//! gpui's key-context dispatch — that is the entire point of replacing iced's
//! `should_forward` carve-outs, so `should_forward`, `MODAL_OPEN` and
//! `PALETTE_OPEN` are deliberately **not** ported.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    div, prelude::*, App, Bounds, Context, Entity, ExternalPaths, FocusHandle, Focusable,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollDelta, ScrollWheelEvent, Window,
};

use crate::entities::animation_clock::{self, AnimationClock};
use crate::entities::terminal_session::TerminalSession;
use crate::terminal::keys::{self, ScrollAmount};
use crate::terminal::mouse::{self, AbsCell, ScrollAccum};
use crate::terminal::{clipboard, drop as file_drop};
use crate::terminal_element::TerminalElement;
use crate::zoom::ZoomState;

/// An in-progress selection drag (`src/gui/state`'s `PtyDrag`).
#[derive(Clone, Copy, Debug)]
struct PtyDrag {
    last_x: f32,
    last_y: f32,
    view_h_px: f32,
}

pub struct TerminalView {
    session: Entity<TerminalSession>,
    focus: FocusHandle,
    clock: Entity<AnimationClock>,
    selection: Option<(AbsCell, AbsCell)>,
    drag: Option<PtyDrag>,
    /// `pty_press_focused` (`pty_input.rs:36-39,104-108`): the press that gave
    /// this element focus swallows its own release, so refocusing never moves
    /// the caret (a second click does). With one session only the window-focus
    /// transition can set it — Plan 05/07 make it load-bearing again.
    press_focused: bool,
    scroll: ScrollAccum,
    /// The element's post-layout bounds, published by `prepaint` so pointer
    /// events (which arrive in window coordinates) can be made element-local.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    _observers: Vec<gpui::Subscription>,
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalView {
    pub fn new(clock: Entity<AnimationClock>, cx: &mut Context<Self>) -> Self {
        let session = cx.new(TerminalSession::spawn);
        let observers = vec![
            // The terminal repaints when its model changes…
            cx.observe(&session, |_, _, cx| cx.notify()),
            // …and on every clock tick, which drives both the cursor blink and
            // the drag autoscroll. No second timer (carried amendment 5).
            cx.observe(&clock, |this: &mut Self, _, cx| {
                this.tick_drag_autoscroll(cx);
                cx.notify();
            }),
        ];
        Self {
            session,
            focus: cx.focus_handle(),
            clock,
            selection: None,
            drag: None,
            press_focused: false,
            scroll: ScrollAccum::default(),
            bounds: Rc::new(Cell::new(Bounds::default())),
            _observers: observers,
        }
    }

    // ── helpers ──────────────────────────────────────────────────────────

    /// Window coordinates → element-local pixels, clamped into the element
    /// (`src/gui/pty.rs:132-141`'s `local`).
    fn local(&self, position: Point<Pixels>) -> (f32, f32) {
        let bounds = self.bounds.get();
        let x = f32::from(position.x - bounds.origin.x);
        let y = f32::from(position.y - bounds.origin.y);
        (
            x.clamp(0.0, f32::from(bounds.size.width).max(0.0)),
            y.clamp(0.0, f32::from(bounds.size.height).max(0.0)),
        )
    }

    fn cell_metrics(cx: &App) -> (f32, f32) {
        let zoom = ZoomState::new(cx.global::<ZoomState>().zoom);
        (zoom.cell_w(), zoom.cell_h())
    }

    /// `(viewport rows, scrollback offset)` — `pty_view_geom`
    /// (`pty_input.rs:235-241`).
    fn view_geom(&self, cx: &App) -> (usize, usize) {
        let session = self.session.read(cx);
        (usize::from(session.dims().0), session.display_offset())
    }

    fn pixel_to_abs(&self, x: f32, y: f32, cx: &App) -> Option<AbsCell> {
        let (cell_w, cell_h) = Self::cell_metrics(cx);
        let (h, sb) = self.view_geom(cx);
        mouse::pixel_to_abs(x, y, cell_w, cell_h, h, sb)
    }

    fn clear_selection(&mut self) {
        self.selection = None;
        self.drag = None;
    }

    // ── keyboard ─────────────────────────────────────────────────────────

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;

        // 1. Any key press kills the selection and any in-progress drag. The
        //    copy path below still needs what was selected, so it is taken
        //    rather than merely dropped.
        let selected = self.selection.take();
        self.drag = None;

        // 2. Copy.
        if keys::is_copy_shortcut(keystroke) {
            if let Some(text) = selected.and_then(|(a, head)| {
                self.session
                    .update(cx, |session, _| session.selection_text(a, head))
            }) {
                clipboard::copy(&text);
            }
            cx.notify();
            return;
        }

        // 3. Paste. Wayland has no native file drag-and-drop (a winit gap), so
        //    a clipboard holding file URIs — "Copy" from a file manager — types
        //    their paths exactly as a drop would, and only otherwise falls
        //    through to a text paste.
        if keys::is_paste_shortcut(keystroke) {
            let paths = file_drop::clipboard_paths();
            if !paths.is_empty() {
                self.session.update(cx, |session, _| {
                    for path in &paths {
                        session.send(file_drop::dropped_path_text(path).as_bytes());
                    }
                });
            } else if let Some(text) = clipboard::paste() {
                let bytes = clipboard::bracketed_paste(&text);
                self.session.update(cx, |session, _| session.send(&bytes));
            }
            cx.notify();
            return;
        }

        // 4. Keyboard scrollback chords.
        if let Some((up, amount)) = keys::keyboard_scroll_intent(keystroke) {
            self.session.update(cx, |session, cx| {
                let lines = match amount {
                    ScrollAmount::Page => session.scroll_page_lines(),
                    ScrollAmount::All => mouse::SCROLLBACK_LINES,
                };
                session.scroll_lines(up, lines);
                cx.notify();
            });
            return;
        }

        // 5. Straight to the PTY.
        //
        // Plan 08 owns Escape's carve-out (`escape_should_dismiss`) and the
        // two-step confirm-kill arming; with no modal and nothing armed in this
        // phase, Escape simply reaches the PTY — which is exactly that
        // function's documented `false` branch, so this is already correct.
        let app_cursor = self.session.read(cx).app_cursor();
        if let Some(bytes) = keys::key_to_bytes(keystroke, app_cursor) {
            self.session.update(cx, |session, cx| {
                session.send(&bytes);
                cx.notify();
            });
        }
    }

    // ── scroll ───────────────────────────────────────────────────────────

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Pinch-to-zoom: the pinned rev's `ScrollDelta` has exactly two
        // variants (`Pixels` / `Lines`) and there is **no distinguishable
        // pinch/magnify event**, so this is modifier+wheel only — recorded in
        // the Plan 04 report.
        if event.modifiers.platform || event.modifiers.control {
            let dy = match event.delta {
                ScrollDelta::Pixels(p) => f32::from(p.y),
                ScrollDelta::Lines(p) => p.y,
            };
            if dy != 0.0 {
                let step = if dy > 0.0 {
                    crate::zoom::ZOOM_STEP
                } else {
                    -crate::zoom::ZOOM_STEP
                };
                crate::views::workspace::Workspace::set_zoom(
                    cx.global::<ZoomState>().zoom + step,
                    cx,
                );
            }
            return;
        }

        let (cell_w, cell_h) = Self::cell_metrics(cx);
        let (x, y) = self.local(event.position);
        let (col, row) = mouse::cell_at(x, y, cell_w, cell_h);

        let notches = match event.delta {
            ScrollDelta::Pixels(p) => self.scroll.feed_pixels(f32::from(p.y), cell_h),
            ScrollDelta::Lines(p) => self.scroll.feed_lines(p.y).map(|up| (up, 1)),
        };
        let Some((up, count)) = notches else { return };
        self.session.update(cx, |session, cx| {
            for _ in 0..count {
                session.scroll(up, col, row);
            }
            cx.notify();
        });
    }

    // ── pointer ──────────────────────────────────────────────────────────

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // With one session, the only focus transition a press can cause is the
        // window/element one.
        self.press_focused = !self.focus.is_focused(window);
        window.focus(&self.focus, cx);

        self.selection = None;
        let (x, y) = self.local(event.position);
        if let Some(cell) = self.pixel_to_abs(x, y, cx) {
            let (_, cell_h) = Self::cell_metrics(cx);
            let (h, _) = self.view_geom(cx);
            self.selection = Some((cell, cell));
            self.drag = Some(PtyDrag {
                last_x: x,
                last_y: y,
                view_h_px: h as f32 * cell_h,
            });
        }
        cx.notify();
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.drag.is_none() {
            return;
        }
        let (x, y) = self.local(event.position);
        if let Some(d) = self.drag.as_mut() {
            d.last_x = x;
            d.last_y = y;
        }
        if let (Some(cell), Some((anchor, _))) = (self.pixel_to_abs(x, y, cx), self.selection) {
            self.selection = Some((anchor, cell));
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, _event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.drag = None;
        let press_focused = std::mem::take(&mut self.press_focused);
        let Some((a, head)) = self.selection else {
            return;
        };
        if a != head {
            // A real drag: keep the selection.
            return;
        }
        self.selection = None;
        if press_focused {
            // The press that focused this element is focus-only; swallow its
            // release so refocusing never moves the caret.
            cx.notify();
            return;
        }
        // No drag happened — click-to-move-caret, but only while scrolled to
        // the live screen: clicking history must be inert
        // (`pty_input.rs:113-121`).
        let (h, sb) = self.view_geom(cx);
        if sb == 0 && h > 0 {
            let row = u16::try_from((h - 1).saturating_sub(a.a_row)).unwrap_or(u16::MAX);
            let col = u16::try_from(a.col).unwrap_or(u16::MAX);
            self.session
                .update(cx, |session, _| session.click(col, row));
        }
        cx.notify();
    }

    /// `pty_input.rs:261-285`. Hangs off the **AnimationClock** tick, not a new
    /// timer: while a drag is held with the pointer within one cell of the
    /// top/bottom edge, scroll one step and extend the head over the revealed
    /// line — but only if the scroll actually moved the view.
    fn tick_drag_autoscroll(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.drag else { return };
        let (_, cell_h) = Self::cell_metrics(cx);
        let up = if d.last_y <= cell_h {
            true
        } else if d.last_y >= d.view_h_px - cell_h {
            false
        } else {
            return;
        };
        let before = self.view_geom(cx).1;
        self.session
            .update(cx, |session, _| session.scroll(up, 0, 0));
        if self.view_geom(cx).1 == before {
            return;
        }
        if let (Some(cell), Some((anchor, _))) =
            (self.pixel_to_abs(d.last_x, d.last_y, cx), self.selection)
        {
            self.selection = Some((anchor, cell));
        }
    }

    // ── file drop ────────────────────────────────────────────────────────

    /// A dropped file types its shell-escaped path plus one trailing space into
    /// the session and clears the selection (`sessions.rs:336-341`). The
    /// modal-aware branches of `on_file_dropped` are Plan 08.
    fn on_drop(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        self.session.update(cx, |session, _| {
            for path in paths.paths() {
                session.send(file_drop::dropped_path_text(path).as_bytes());
            }
        });
        self.clear_selection();
        cx.notify();
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let zoom = cx.global::<ZoomState>().zoom;
        let cursor_visible = animation_clock::cursor_visible(self.clock.read(cx).tick());

        div()
            .track_focus(&self.focus)
            .key_context("Terminal")
            .size_full()
            .overflow_hidden()
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_drop(
                cx.listener(|this: &mut Self, paths: &ExternalPaths, _window, cx| {
                    this.on_drop(paths, cx);
                }),
            )
            .child(TerminalElement::new(
                self.session.clone(),
                self.selection,
                cursor_visible,
                zoom,
                Rc::clone(&self.bounds),
            ))
    }
}
