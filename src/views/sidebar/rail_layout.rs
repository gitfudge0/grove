//! Sidebar rail geometry and resize gestures.

use super::*;

impl Sidebar {
    pub(super) fn logical_window_width(window: &Window) -> f32 {
        f32::from(window.viewport_size().width)
            / (f32::from(window.rem_size()) / crate::zoom::REM_BASE)
    }

    pub(crate) fn is_collapsed(&self, cx: &App) -> bool {
        cx.global::<SettingsState>().store.sidebar_collapsed
    }

    pub(crate) fn navigation_collapsed(&self, window: &Window, cx: &App) -> bool {
        let width = Self::logical_window_width(window);
        self.is_collapsed(cx)
            || (width < self.context_width(window, cx) + MODAL_W_LG
                && self.navigation_width_override != Some(width))
    }

    pub(crate) fn rail_width(&self, window: &Window, cx: &App) -> f32 {
        if self.navigation_collapsed(window, cx) {
            SIDEBAR_COLLAPSED_W
        } else {
            self.context_width(window, cx)
        }
    }

    pub(super) fn context_width(&self, window: &Window, cx: &App) -> f32 {
        let preferred = self.drag.map_or_else(
            || {
                cx.global::<SettingsState>()
                    .store
                    .sidebar_width
                    .unwrap_or(SIDEBAR_W)
            },
            |drag| drag.width,
        );
        effective_rail_width(preferred, Self::logical_window_width(window))
    }

    pub(super) fn divider_press(
        &mut self,
        event: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.prevent_default();
        cx.stop_propagation();
        if self.is_collapsed(cx) {
            return;
        }
        if event.click_count == 2 {
            self.drag = None;
            SettingsState::update(cx, |store| store.sidebar_width = Some(SIDEBAR_W));
            cx.notify();
            return;
        }
        let width = self.context_width(window, cx);
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        self.drag = Some(SidebarDrag {
            start_width: width,
            width,
            grab_offset: width - f32::from(event.position.x) / scale,
        });
    }

    pub(super) fn divider_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        if !event.dragging() {
            self.drag = None;
            return;
        }
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let cursor = f32::from(event.position.x) / scale;
        let width = effective_rail_width(
            cursor + drag.grab_offset,
            Self::logical_window_width(window),
        );
        if (drag.width - width).abs() > f32::EPSILON {
            drag.width = width;
            cx.notify();
        }
        cx.stop_propagation();
    }

    pub(super) fn divider_release(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if (drag.width - drag.start_width).abs() >= SIDEBAR_DRAG_EPSILON {
            SettingsState::update(cx, |store| store.sidebar_width = Some(drag.width));
        }
        cx.notify();
    }
}
