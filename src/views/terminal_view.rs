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
    Subscription, Task, Window,
};
use gpui_component::input::{Copy, Paste, SelectAll};
use std::{cell::Cell, rc::Rc};

pub struct TerminalView {
    session: Entity<TerminalSession>,
    focus: FocusHandle,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    selection: Option<(AbsCell, AbsCell)>,
    selecting: bool,
    pending_click: Option<(u16, u16)>,
    autoscroll_task: Option<Task<()>>,
    autoscrolling: bool,
    _subscription: Subscription,
}
impl TerminalView {
    /// Native Edit menu availability for the terminal that owns keyboard focus.
    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn terminal_menu_state(&self, window: &Window, cx: &App) -> Option<(bool, bool)> {
        self.focus.contains_focused(window, cx).then(|| {
            (
                self.selection.is_some(),
                self.session.read(cx).selection_all_bounds().is_some(),
            )
        })
    }

    pub fn new(session: Entity<TerminalSession>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&session, |_, _, cx| cx.notify());
        Self {
            session,
            focus: cx.focus_handle().tab_stop(true),
            bounds: Rc::default(),
            selection: None,
            selecting: false,
            pending_click: None,
            autoscroll_task: None,
            autoscrolling: false,
            _subscription: subscription,
        }
    }
    fn stop_autoscroll(&mut self) {
        self.autoscrolling = false;
        self.autoscroll_task = None;
    }

    fn stop_drag(&mut self) {
        self.selecting = false;
        self.pending_click = None;
        self.stop_autoscroll();
    }

    fn scroll_direction(&self, window: &Window, cx: &App) -> Option<bool> {
        let position = window.mouse_position();
        let bounds = self.bounds.get();
        mouse::drag_scroll_direction(
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            cx.global::<ZoomState>().cell_h(),
        )
    }

    fn sync_autoscroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || self.scroll_direction(window, cx).is_none() {
            self.stop_autoscroll();
            return;
        }
        self.pending_click = None;
        if self.autoscrolling {
            return;
        }
        self.autoscrolling = true;
        self.autoscroll_task = Some(cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor()
                .timer(crate::entities::animation_clock::FAST)
                .await;
            let keep_scrolling = this
                .update_in(cx, |this, window, cx| {
                    let keep_scrolling = this.tick_autoscroll(window, cx);
                    // Leave the completed Task owned until the next pointer event;
                    // dropping the current task from its own update would cancel it.
                    this.autoscrolling = keep_scrolling;
                    keep_scrolling
                })
                .unwrap_or(false);
            if !keep_scrolling {
                break;
            }
        }));
    }

    fn tick_autoscroll(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        if !self.selecting || !self.focus.is_focused(window) || !window.is_window_active() {
            self.selecting = false;
            self.pending_click = None;
            return false;
        }
        let Some(up) = self.scroll_direction(window, cx) else {
            return false;
        };
        let Some(selection) = self.selection else {
            return false;
        };
        let before = self.session.read(cx).display_offset();
        self.session
            .update(cx, |term, _| term.scroll_selection(up, mouse::SCROLL_STEP));
        let term = self.session.read(cx);
        let offset = term.display_offset();
        if offset == before {
            return false;
        }
        let zoom = cx.global::<ZoomState>();
        let bounds = self.bounds.get();
        let position = window.mouse_position();
        let x = f32::from(position.x - bounds.origin.x).clamp(
            0.0,
            f32::from(term.dims().1.saturating_sub(1)) * zoom.cell_w(),
        );
        self.selection = mouse::extend_selection_after_scroll(
            selection,
            x,
            f32::from(position.y - bounds.origin.y),
            zoom.cell_w(),
            zoom.cell_h(),
            usize::from(term.dims().0),
            offset,
        );
        cx.notify();
        true
    }

    fn drag_move(
        &mut self,
        event: &gpui::MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.selecting {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.stop_drag();
            return;
        }
        if let (Some((anchor, _)), Some(head)) = (self.selection, self.cell(event.position, cx)) {
            if anchor != head {
                self.pending_click = None;
            }
            self.selection = Some((anchor, head));
            cx.notify();
        }
        self.sync_autoscroll(window, cx);
    }

    fn cell(&self, position: gpui::Point<Pixels>, cx: &App) -> Option<AbsCell> {
        let zoom = cx.global::<ZoomState>();
        let bounds = self.bounds.get();
        let term = self.session.read(cx);
        mouse::pixel_to_abs(
            f32::from(position.x - bounds.origin.x).clamp(
                0.0,
                f32::from(term.dims().1.saturating_sub(1)) * zoom.cell_w(),
            ),
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
        if self.selecting && !self.focus.is_focused(window) {
            self.stop_drag();
        }
        div()
            .id("terminal-view")
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Copy, _, cx| {
                let text = this.selection.and_then(|(a, b)| {
                    this.session.update(cx, |term, _| term.selection_text(a, b))
                });
                if let Some(text) = text {
                    clipboard::copy(&text);
                } else {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|this, _: &Paste, _, cx| {
                let Some(text) = clipboard::paste() else {
                    cx.propagate();
                    return;
                };
                this.stop_drag();
                this.selection = None;
                this.session.update(cx, |term, cx| {
                    term.snap_to_bottom();
                    term.send(&clipboard::bracketed_paste(&text));
                    cx.notify();
                });
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                let Some(bounds) = this.session.read(cx).selection_all_bounds() else {
                    cx.propagate();
                    return;
                };
                this.stop_drag();
                this.selection = Some(bounds);
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    this.focus.focus(window, cx);
                    this.stop_drag();
                    this.selecting = true;
                    this.selection = this.cell(event.position, cx).map(|cell| (cell, cell));
                    // Wait until release to distinguish a TUI click from a text drag.
                    this.pending_click = if this.session.read(cx).mouse_reporting() {
                        let zoom = cx.global::<ZoomState>();
                        let bounds = this.bounds.get();
                        Some(mouse::cell_at(
                            f32::from(event.position.x - bounds.origin.x),
                            f32::from(event.position.y - bounds.origin.y),
                            zoom.cell_w(),
                            zoom.cell_h(),
                        ))
                    } else {
                        None
                    };
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseUpEvent, _, cx| {
                    this.selecting = false;
                    this.stop_autoscroll();
                    if let Some((col, row)) = this.pending_click.take() {
                        if this.bounds.get().contains(&event.position) {
                            this.selection = None;
                            this.session.update(cx, |term, cx| {
                                term.click(col, row);
                                cx.notify();
                            });
                            cx.notify();
                        }
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.stop_drag();
                }),
            )
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
                this.stop_drag();
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
            // Capture only a drag owned by this terminal, including pointer
            // moves outside its hitbox. The canvas paints no pixels or hitbox.
            .child(
                gpui::canvas(|_, _, _| (), {
                    let listener = cx.listener(Self::drag_move);
                    move |_, (), window, _| {
                        window.on_mouse_event(
                            move |event: &gpui::MouseMoveEvent, phase, window, cx| {
                                if phase == gpui::DispatchPhase::Capture {
                                    listener(event, window, cx);
                                }
                            },
                        );
                    }
                })
                .absolute(),
            )
            .child(TerminalElement::new(
                self.session.clone(),
                self.selection,
                self.focus.is_focused(window),
                zoom,
                self.bounds.clone(),
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px, Modifiers};
    use std::fmt::Write as _;

    fn fixture(
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<TerminalView>, &mut gpui::VisualTestContext) {
        cx.update(|cx| {
            cx.set_global(ZoomState::new(1.0));
            cx.set_global(crate::theme::ThemeState::new(
                false,
                "tokyonight".into(),
                "tokyonight-day".into(),
            ));
        });
        let session = cx.new(TerminalSession::native_for_test);
        let (view, cx) = cx.add_window_view(|_, cx| TerminalView::new(session, cx));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.activate_window();
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            view.session.update(cx, |term, cx| {
                term.ingest_for_test(b"Claude output\x1b[?1000h\x1b[?1006h", cx);
            });
        });
        (view, cx)
    }

    #[gpui::test]
    fn mouse_reporting_drag_selects_without_sending_an_app_click(cx: &mut gpui::TestAppContext) {
        let (view, cx) = fixture(cx);
        let before = view.read_with(cx, |view, cx| view.session.read(cx).last_input_for_test());
        let start = point(px(2.0), px(2.0));
        let end = point(px(crate::fonts::CELL_W * 5.0), px(2.0));
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        view.update(cx, |view, cx| {
            let (anchor, head) = view
                .selection
                .expect("drag must select even with app mouse reporting");
            assert_eq!(anchor.col, 0);
            assert_eq!(head.col, 5);
            assert_eq!(
                view.session.read(cx).last_input_for_test(),
                before,
                "selection must not click or type into the app"
            );
            assert_eq!(
                view.session
                    .update(cx, |term, _| term.selection_text(anchor, head)),
                Some("Claude".into())
            );
        });
    }

    #[gpui::test]
    fn mouse_reporting_stationary_click_still_reaches_the_app(cx: &mut gpui::TestAppContext) {
        let (view, cx) = fixture(cx);
        let before = view.read_with(cx, |view, cx| view.session.read(cx).last_input_for_test());
        let position = point(px(2.0), px(2.0));
        cx.simulate_mouse_down(position, MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, cx| {
            assert_eq!(view.session.read(cx).last_input_for_test(), before);
        });
        cx.simulate_mouse_up(position, MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, cx| {
            assert!(view.selection.is_none());
            assert_ne!(view.session.read(cx).last_input_for_test(), before);
        });
    }

    #[gpui::test]
    fn returning_to_drag_anchor_does_not_turn_selection_into_an_app_click(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = fixture(cx);
        let before = view.read_with(cx, |view, cx| view.session.read(cx).last_input_for_test());
        let start = point(px(2.0), px(2.0));
        let end = point(px(crate::fonts::CELL_W * 5.0), px(2.0));
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(start, MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, cx| {
            assert!(view.selection.is_some());
            assert_eq!(view.session.read(cx).last_input_for_test(), before);
        });
    }

    #[gpui::test]
    fn release_outside_cancels_app_click(cx: &mut gpui::TestAppContext) {
        let (view, cx) = fixture(cx);
        let before = view.read_with(cx, |view, cx| view.session.read(cx).last_input_for_test());
        let start = point(px(2.0), px(2.0));
        let outside = point(px(-2.0), px(-2.0));
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        view.read_with(cx, |view, cx| {
            assert!(!view.selecting);
            assert!(view.pending_click.is_none());
            assert_eq!(view.session.read(cx).last_input_for_test(), before);
        });
    }

    #[gpui::test]
    fn ordinary_terminal_drag_still_selects_without_input(cx: &mut gpui::TestAppContext) {
        let (view, cx) = fixture(cx);
        view.update(cx, |view, cx| {
            view.session.update(cx, |term, cx| {
                term.ingest_for_test(b"\x1b[?1000l", cx);
            });
        });
        let before = view.read_with(cx, |view, cx| view.session.read(cx).last_input_for_test());
        let start = point(px(2.0), px(2.0));
        let end = point(px(crate::fonts::CELL_W * 5.0), px(2.0));
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        view.update(cx, |view, cx| {
            let (anchor, head) = view.selection.unwrap();
            assert_eq!(
                view.session
                    .update(cx, |term, _| term.selection_text(anchor, head)),
                Some("Claude".into())
            );
            assert_eq!(view.session.read(cx).last_input_for_test(), before);
        });
    }

    fn history_fixture(
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<TerminalView>, &mut gpui::VisualTestContext, usize) {
        let (view, cx) = fixture(cx);
        let total = view.read_with(cx, |view, cx| usize::from(view.session.read(cx).dims().0)) + 40;
        let mut output = String::from("\x1b[2J\x1b[H");
        for row in 0..total {
            writeln!(output, "row{row:03}\r").unwrap();
        }
        view.update(cx, |view, cx| {
            view.session
                .update(cx, |term, cx| term.ingest_for_test(output.as_bytes(), cx));
        });
        (view, cx, total)
    }

    fn advance_drag(cx: &mut gpui::VisualTestContext) {
        cx.executor()
            .advance_clock(crate::entities::animation_clock::FAST);
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn native_select_all_includes_retained_history_without_sending_input(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, total) = history_fixture(cx);
        let before = view.read_with(cx, |view, cx| view.session.read(cx).last_input_for_test());
        cx.update(|window, cx| {
            let focus = view.read(cx).focus.clone();
            focus.focus(window, cx);
        });
        cx.update(|window, cx| {
            window.dispatch_action(Box::new(SelectAll), cx);
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            let (anchor, head) = view
                .selection
                .expect("Select All must select the native buffer");
            let text = view
                .session
                .update(cx, |term, _| term.selection_text(anchor, head))
                .unwrap();
            // Clearing the viewport retains its earlier output in scrollback.
            let expected = std::iter::once("Claude output".to_string())
                .chain((0..total).map(|row| format!("row{row:03}")))
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(text, expected);
            assert_eq!(view.session.read(cx).last_input_for_test(), before);
        });
    }

    #[gpui::test]
    fn native_edit_actions_preserve_terminal_control_c(cx: &mut gpui::TestAppContext) {
        let (view, cx) = fixture(cx);
        cx.update(|window, cx| {
            let focus = view.read(cx).focus.clone();
            focus.focus(window, cx);
            assert_eq!(
                view.read(cx).terminal_menu_state(window, cx),
                Some((false, true))
            );
            window.dispatch_action(Box::new(Copy), cx);
        });
        cx.run_until_parked();
        let before = view.read_with(cx, |view, cx| view.session.read(cx).last_input_for_test());
        cx.simulate_keystrokes("ctrl-c");
        view.read_with(cx, |view, cx| {
            assert_ne!(view.session.read(cx).last_input_for_test(), before);
        });
    }

    #[gpui::test]
    fn stationary_edge_drag_scrolls_history_and_extends_selection(cx: &mut gpui::TestAppContext) {
        let (view, cx, total) = history_fixture(cx);
        let bounds = view.read_with(cx, |view, _| view.bounds.get());
        let start = point(px(crate::fonts::CELL_W * 5.0), bounds.center().y);
        let top = point(px(2.0), px(crate::fonts::CELL_H / 2.0));
        let before = view.read_with(cx, |view, cx| view.session.read(cx).last_input_for_test());
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        let anchor = view.read_with(cx, |view, _| view.selection.unwrap().0);
        cx.simulate_mouse_move(top, MouseButton::Left, Modifiers::default());
        advance_drag(cx);
        let first_offset = view.read_with(cx, |view, cx| view.session.read(cx).display_offset());
        assert!(
            first_offset > 0,
            "holding a selection at the top must scroll history even with app mouse reporting"
        );
        advance_drag(cx);
        view.update(cx, |view, cx| {
            let offset = view.session.read(cx).display_offset();
            assert!(
                offset > first_offset,
                "stationary edge drag must keep scrolling"
            );
            let (actual_anchor, head) = view.selection.unwrap();
            assert_eq!(actual_anchor, anchor);
            assert_eq!(
                head.a_row,
                offset + usize::from(view.session.read(cx).dims().0) - 1
            );
            let expected = (total - head.a_row..=total - anchor.a_row)
                .map(|row| format!("row{row:03}"))
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(
                view.session
                    .update(cx, |term, _| term.selection_text(anchor, head)),
                Some(expected)
            );
            assert_eq!(
                view.session.read(cx).last_input_for_test(),
                before,
                "drag scroll must not inject wheel events into the app"
            );
        });
    }

    #[gpui::test]
    fn downward_edge_drag_extends_to_live_rows_and_release_outside_stops_it(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx, total) = history_fixture(cx);
        view.update(cx, |view, cx| {
            view.session
                .update(cx, |term, _| term.scroll_selection(true, 12));
        });
        let bounds = view.read_with(cx, |view, _| view.bounds.get());
        let start = point(px(2.0), bounds.center().y);
        let bottom = point(
            px(crate::fonts::CELL_W * 5.0),
            bounds.bottom() + px(crate::fonts::CELL_H),
        );
        let before = view.read_with(cx, |view, cx| view.session.read(cx).last_input_for_test());
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        let anchor = view.read_with(cx, |view, _| view.selection.unwrap().0);
        cx.simulate_mouse_move(bottom, MouseButton::Left, Modifiers::default());
        advance_drag(cx);
        advance_drag(cx);
        let offset = view.update(cx, |view, cx| {
            let offset = view.session.read(cx).display_offset();
            assert!(
                offset < 12,
                "drag below the viewport must scroll toward live rows"
            );
            let (actual_anchor, head) = view.selection.unwrap();
            assert_eq!(actual_anchor, anchor);
            assert_eq!(head.a_row, offset);
            let expected = (total - anchor.a_row..=total - head.a_row)
                .map(|row| format!("row{row:03}"))
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(
                view.session
                    .update(cx, |term, _| term.selection_text(anchor, head)),
                Some(expected)
            );
            assert_eq!(view.session.read(cx).last_input_for_test(), before);
            offset
        });
        cx.simulate_mouse_up(bottom, MouseButton::Left, Modifiers::default());
        advance_drag(cx);
        advance_drag(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.session.read(cx).display_offset(), offset);
            assert!(!view.selecting);
            assert!(!view.autoscrolling);
        });
    }

    #[gpui::test]
    fn moving_away_from_edge_stops_autoscroll(cx: &mut gpui::TestAppContext) {
        let (view, cx, _) = history_fixture(cx);
        let bounds = view.read_with(cx, |view, _| view.bounds.get());
        let center = bounds.center();
        let top = point(center.x, px(crate::fonts::CELL_H / 2.0));
        cx.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(top, MouseButton::Left, Modifiers::default());
        advance_drag(cx);
        let offset = view.read_with(cx, |view, cx| view.session.read(cx).display_offset());
        assert!(offset > 0);
        cx.simulate_mouse_move(center, MouseButton::Left, Modifiers::default());
        advance_drag(cx);
        advance_drag(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.session.read(cx).display_offset(), offset);
            assert!(!view.autoscrolling);
        });
        cx.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
    }
}
