//! The root view: themed placeholder chrome at Grove's real dimensions.
//!
//! Plans 04-07 replace these placeholders region by region. Every dimension
//! comes from a named constant carrying its `src/gui/metrics.rs` line.

use gpui::{div, prelude::*, px, rems, App, Context, Entity, FocusHandle, Focusable, Window};

use crate::entities::animation_clock::AnimationClock;
use crate::keymap;
use crate::settings::SettingsState;
use crate::theme as c;
use crate::zoom::{self, ZoomState};

/// Default sidebar width (`src/gui/metrics.rs:9`).
const RAIL_W: f32 = 320.0;
/// Divider/grab-handle between sidebar and workspace (`src/gui/metrics.rs:20`).
const SIDEBAR_DIVIDER_W: f32 = 6.0;
/// App bar height (`src/gui/metrics.rs:15`).
const APPBAR_H: f32 = 44.0;
/// Status bar height (`src/gui/metrics.rs:16`).
const STATUS_H: f32 = 26.0;

/// Chrome is authored in `rems` so a single `set_rem_size` scales all of it.
fn r(px_at_1x: f32) -> gpui::Rems {
    rems(px_at_1x / zoom::REM_BASE)
}

pub struct Workspace {
    focus: FocusHandle,
    clock: Entity<AnimationClock>,
    _clock_observer: gpui::Subscription,
}

impl Focusable for Workspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Workspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let clock = cx.new(AnimationClock::new);
        // Observing the clock is what proves the tick drives repaints end to
        // end: the body's counter only advances because of this.
        let observer = cx.observe(&clock, |_, _, cx| cx.notify());
        Self {
            focus: cx.focus_handle(),
            clock,
            _clock_observer: observer,
        }
    }

    /// Applies a new zoom level: state, the debounced persist, repaint.
    fn set_zoom(zoom_value: f32, cx: &mut App) {
        let snapped = zoom::snap(zoom_value);
        if cx.global::<ZoomState>().zoom == snapped {
            return;
        }
        cx.global_mut::<ZoomState>().zoom = snapped;
        SettingsState::update(cx, |s| s.ui_zoom = Some(snapped));
        cx.refresh_windows();
    }

    fn zoom_in(_: &keymap::ZoomIn, _: &mut Window, cx: &mut App) {
        Self::set_zoom(cx.global::<ZoomState>().zoom + zoom::ZOOM_STEP, cx);
    }

    fn zoom_out(_: &keymap::ZoomOut, _: &mut Window, cx: &mut App) {
        Self::set_zoom(cx.global::<ZoomState>().zoom - zoom::ZOOM_STEP, cx);
    }

    fn zoom_reset(_: &keymap::ZoomReset, _: &mut Window, cx: &mut App) {
        Self::set_zoom(zoom::ZOOM_DEFAULT, cx);
    }
}

/// Logs and does nothing. Each stub names the plan that implements it.
macro_rules! stub_action {
    ($div:expr, $action:ty, $plan:literal) => {
        $div.on_action(|_: &$action, _: &mut Window, _: &mut App| {
            tracing::debug!(concat!(
                stringify!($action),
                ": not implemented yet — ",
                $plan
            ));
        })
    };
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The single zoom application point. `WithRemSize` does not exist at
        // this rev; `Window::with_rem_size` is for scoped overrides.
        let zoom_value = cx.global::<ZoomState>().zoom;
        window.set_rem_size(px(zoom::REM_BASE * zoom_value));

        let tick = self.clock.read(cx).tick();
        let theme_name = grove_core::theme::with_current(|t| t.name.to_string());
        let body_text = format!("grove — theme {theme_name} · zoom {zoom_value:.1}× · tick {tick}");

        let root = div()
            .track_focus(&self.focus)
            .key_context("Workspace")
            .on_action(Self::zoom_in)
            .on_action(Self::zoom_out)
            .on_action(Self::zoom_reset);
        let root = stub_action!(root, keymap::NewSession, "Plan 07");
        let root = stub_action!(root, keymap::NewSessionInWorktree, "Plan 07");
        let root = stub_action!(root, keymap::SwitchSession, "Plan 08");
        let root = stub_action!(root, keymap::NextSession, "Plan 05");
        let root = stub_action!(root, keymap::PrevSession, "Plan 05");
        let root = stub_action!(root, keymap::ToggleGrid, "Plan 07");
        let root = stub_action!(root, keymap::ToggleZen, "Plan 06");
        let root = stub_action!(root, keymap::Settings, "Plan 08");
        let root = stub_action!(root, keymap::ShortcutOverlay, "Plan 08");
        let root = stub_action!(root, keymap::CloseFocusedSession, "Plan 05");
        let root = stub_action!(root, keymap::ToggleTerminal, "Plan 07");
        let root = stub_action!(root, keymap::NewHomeTerminal, "Plan 07");
        let root = stub_action!(root, keymap::JumpToWaitingSession, "Plan 05");
        let root = stub_action!(root, keymap::ScrollHalfPageUp, "Plan 04");
        let root = stub_action!(root, keymap::ScrollHalfPageDown, "Plan 04");

        root.flex()
            .flex_row()
            .size_full()
            .bg(c::BG())
            .text_color(c::FG())
            // Sidebar placeholder (Plan 05).
            .child(div().w(r(RAIL_W)).h_full().bg(c::BG_RAIL()))
            // Sidebar divider (Plan 05).
            .child(div().w(r(SIDEBAR_DIVIDER_W)).h_full().bg(c::BORDER()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .h_full()
                    // App bar placeholder (Plan 06).
                    .child(div().h(r(APPBAR_H)).w_full().bg(c::BG_STRIP()))
                    .child(
                        // Body. The text exists purely so a human can see
                        // theme, zoom and clock working in one glance —
                        // Plan 04 deletes it.
                        div()
                            .flex()
                            .flex_1()
                            .w_full()
                            .items_center()
                            .justify_center()
                            .bg(c::BG())
                            .text_color(c::FG_DIM())
                            .child(body_text),
                    )
                    // Status bar placeholder (Plan 06).
                    .child(div().h(r(STATUS_H)).w_full().bg(c::BG_STRIP())),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rems_are_derived_from_the_pixel_constants() {
        assert_eq!(r(RAIL_W).0, 320.0 / 16.0);
        assert_eq!(r(APPBAR_H).0, 44.0 / 16.0);
        assert_eq!(r(STATUS_H).0, 26.0 / 16.0);
        assert_eq!(r(SIDEBAR_DIVIDER_W).0, 6.0 / 16.0);
    }
}
