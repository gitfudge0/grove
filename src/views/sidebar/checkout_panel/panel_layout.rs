//! Responsive panel placement, visibility, and transition geometry.

use super::{motion, rpx, Duration, Instant, Selection, Sidebar, CARD_WIDTH};
use crate::views::tokens::{APPBAR_H, MODAL_W_LG, MOTION_BASE_MS, SPACE_20, SPACE_3XL};
use gpui::prelude::{InteractiveElement, IntoElement, ParentElement, Styled};
use gpui::{div, AnyElement, App, Context, Window};

impl Sidebar {
    pub(in crate::views::sidebar) fn toggle_session_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let width = Self::logical_window_width(window);
        let auto_hidden = width - self.rail_width(window, cx) < CARD_WIDTH + MODAL_W_LG
            && self.checkout_panel.width_override != Some(width);
        let was_visible = !self.checkout_panel.hidden && !auto_hidden;
        self.checkout_panel.width_override = Some(width);
        if self.files_active && self.files_owner == self.selection {
            self.files_active = false;
            self.checkout_panel.hidden = false;
            self.checkout_panel.expanded = true;
            cx.notify();
            return;
        }
        if self.checkout_panel.side_chat_owner.take().is_some() {
            self.checkout_panel.hidden = false;
            self.checkout_panel.expanded = true;
            self.start_panel_motion(was_visible, true, cx);
            cx.notify();
            return;
        }
        self.checkout_panel.hidden = was_visible;
        if self.checkout_panel.hidden {
            self.dismiss_agent_peek(window, cx);
            self.dismiss_plan_peek(window, cx);
            self.dismiss_script_peek(window, cx);
            self.checkout_panel.popup = None;
            self.checkout_panel.popup_subscription = None;
        }
        self.start_panel_motion(was_visible, !self.checkout_panel.hidden, cx);
        cx.notify();
    }

    pub(in crate::views::sidebar) fn start_panel_motion(
        &mut self,
        was_visible: bool,
        visible: bool,
        cx: &App,
    ) {
        if cx.reduce_motion() {
            self.checkout_panel.panel_motion = None;
            return;
        }
        let from = self
            .panel_motion_width()
            .unwrap_or(if was_visible { 1. } else { 0. });
        self.checkout_panel.panel_motion_revision =
            self.checkout_panel.panel_motion_revision.wrapping_add(1);
        self.checkout_panel.panel_motion = Some((
            Instant::now(),
            from,
            if visible { 1. } else { 0. },
            self.checkout_panel.panel_motion_revision,
        ));
    }

    pub(super) fn panel_motion_width(&self) -> Option<f32> {
        self.checkout_panel
            .panel_motion
            .map(|(started, from, to, _)| {
                let progress = (started.elapsed().as_secs_f32() / (MOTION_BASE_MS as f32 / 1000.))
                    .clamp(0., 1.);
                from + (to - from) * motion::ease_standard(progress)
            })
    }

    #[cfg(test)]
    pub(in crate::views::sidebar) fn hold_panel_motion_at_start(&mut self) {
        if let Some((started, _, _, _)) = self.checkout_panel.panel_motion.as_mut() {
            // Draw the exact first frame independently of machine/render latency.
            *started = Instant::now() + Duration::from_secs(1);
        }
    }

    pub(in crate::views::sidebar) fn narrow_panel_entrance<
        E: IntoElement + gpui::Styled + 'static,
    >(
        &self,
        panel: E,
        cx: &App,
    ) -> AnyElement {
        match self.checkout_panel.panel_motion {
            Some((started, _, _, id))
                if started.elapsed() < Duration::from_millis(MOTION_BASE_MS) =>
            {
                motion::fast(panel, format!("narrow-session-panel-enter-{id}"), cx)
            }
            _ => panel.into_any_element(),
        }
    }

    pub(super) fn wide_panel_layout(
        &mut self,
        body: AnyElement,
        panel: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut width = 1.;
        let mut revision = None;
        if !cx.reduce_motion() {
            if let Some((started, _, _, id)) = self.checkout_panel.panel_motion {
                if started.elapsed() < Duration::from_millis(MOTION_BASE_MS) {
                    width = self.panel_motion_width().unwrap_or(1.);
                    revision = Some(id);
                    window.request_animation_frame();
                } else {
                    self.checkout_panel.panel_motion = None;
                }
            }
        } else {
            self.checkout_panel.panel_motion = None;
        }
        let panel = if let Some(id) = revision {
            motion::fast(
                div().size_full().child(panel),
                format!("session-panel-enter-{id}"),
                cx,
            )
        } else {
            panel
        };
        let panel_width = if !self.dashboard_open
            && self.checkout_panel.side_chat_owner.is_some()
            && self.checkout_panel.side_chat_owner == self.selection
        {
            super::super::side_chat::SIDE_CHAT_WIDTH
        } else if self.checkout_panel.hidden && self.checkout_panel.panel_last_wide_width > 0. {
            self.checkout_panel.panel_last_wide_width
        } else {
            CARD_WIDTH
        };
        if !self.checkout_panel.hidden {
            self.checkout_panel.panel_last_wide_width = panel_width;
        }
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .relative()
            .flex()
            .child(div().flex_1().min_w_0().min_h_0().child(body))
            .child(
                div()
                    .w(rpx(panel_width * width))
                    .h_full()
                    .flex_shrink_0()
                    .overflow_hidden()
                    .child(panel),
            )
            .into_any_element()
    }

    pub(in crate::views::sidebar) fn render_checkout_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = self.render_checkout_container_body(window, cx);
        if !self.checkout_panel.toggle_available {
            return body;
        }
        let side_chat_available = !self.dashboard_open
            && match self.selection {
                Some(Selection::Session(id) | Selection::Home(id)) => self
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .codex(id)
                    .is_some_and(|session| !session.read(cx).is_deferred()),
                _ => false,
            };
        let controls = self.side_container_controls(side_chat_available, window, cx);
        // This container survives Home/session navigation and panel dismissal.
        // Keep the toolbar outside the animated body so reopening stays reachable.
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .relative()
            .child(body)
            .child(
                div()
                    .id("side-container-controls")
                    .debug_selector(|| "side-container-controls".into())
                    .absolute()
                    .top(rpx(SPACE_3XL))
                    .right_0()
                    .h(rpx(APPBAR_H))
                    .px(rpx(SPACE_20))
                    .flex()
                    .items_center()
                    .child(controls),
            )
            .into_any_element()
    }
}
