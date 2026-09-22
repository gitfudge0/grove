//! Focus and input ownership for a rendered PTY.
use crate::{
    entities::terminal_session::TerminalSession,
    terminal::{
        clipboard, keys,
        mouse::{self, AbsCell},
    },
    terminal_element::TerminalElement,
    zoom::ZoomState,
};
use gpui::{
    div, prelude::*, App, Bounds, Context, Entity, FocusHandle, Focusable, MouseButton, Pixels,
    Subscription, Window,
};
use std::{cell::Cell, rc::Rc};

pub struct TerminalView {
    session: Entity<TerminalSession>,
    focus: FocusHandle,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    selection: Option<(AbsCell, AbsCell)>,
    _subscription: Subscription,
}
impl TerminalView {
    pub fn new(session: Entity<TerminalSession>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&session, |_, _, cx| cx.notify());
        Self {
            session,
            focus: cx.focus_handle().tab_stop(true),
            bounds: Rc::default(),
            selection: None,
            _subscription: subscription,
        }
    }
    fn cell(&self, position: gpui::Point<Pixels>, cx: &App) -> Option<AbsCell> {
        let zoom = cx.global::<ZoomState>();
        let bounds = self.bounds.get();
        let term = self.session.read(cx);
        mouse::pixel_to_abs(
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
            zoom.cell_w(),
            zoom.cell_h(),
            usize::from(term.dims().0),
            term.display_offset(),
        )
    }
}
impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let zoom = cx.global::<ZoomState>().zoom;
        div()
            .id("terminal-view")
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    this.focus.focus(window, cx);
                    this.selection = this.cell(event.position, cx).map(|cell| (cell, cell));
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    if let (Some((anchor, _)), Some(head)) =
                        (this.selection, this.cell(event.position, cx))
                    {
                        this.selection = Some((anchor, head));
                        cx.notify();
                    }
                }
            }))
            .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                let dy = match event.delta {
                    gpui::ScrollDelta::Lines(p) => p.y,
                    gpui::ScrollDelta::Pixels(p) => {
                        f32::from(p.y) / cx.global::<ZoomState>().cell_h()
                    }
                };
                if dy.abs() >= 0.1 {
                    this.session.update(cx, |term, cx| {
                        term.scroll_lines(dy > 0., dy.abs().ceil() as usize);
                        cx.notify();
                    });
                }
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                let key = &event.keystroke;
                if let Some((up, amount)) = keys::keyboard_scroll_intent(key) {
                    this.session.update(cx, |term, cx| {
                        let lines = match amount {
                            keys::ScrollAmount::Page => term.scroll_page_lines(),
                            keys::ScrollAmount::All => mouse::SCROLLBACK_LINES,
                        };
                        term.scroll_lines(up, lines);
                        cx.notify();
                    });
                    cx.stop_propagation();
                    return;
                }
                if keys::is_copy_shortcut(key) {
                    if let Some((a, b)) = this.selection {
                        if let Some(text) =
                            this.session.update(cx, |term, _| term.selection_text(a, b))
                        {
                            clipboard::copy(&text);
                        }
                    }
                    cx.stop_propagation();
                    return;
                }
                let bytes = if keys::is_paste_shortcut(key) {
                    clipboard::paste().map(|text| clipboard::bracketed_paste(&text))
                } else {
                    keys::key_to_bytes(key, this.session.read(cx).app_cursor())
                };
                if let Some(bytes) = bytes {
                    this.selection = None;
                    this.session.update(cx, |term, cx| {
                        term.snap_to_bottom();
                        term.send(&bytes);
                        cx.notify();
                    });
                    cx.stop_propagation();
                }
            }))
            .child(TerminalElement::new(
                self.session.clone(),
                self.selection,
                self.focus.is_focused(window),
                zoom,
                self.bounds.clone(),
            ))
    }
}
