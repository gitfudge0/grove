//! Sidebar and canvas confirmation surfaces and their input gates.

use super::*;

impl Sidebar {
    pub(in crate::views) fn canvas_confirmation(
        &self,
        id: SessionId,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.canvas_close_anchor != Some(id) || !self.confirmation_open() {
            return None;
        }
        // Zen hides the header that records tile bounds, so its saved bounds are stale.
        // Anchor the tray at the top inset of the full-screen pane instead.
        let bounds = if self.is_zen() {
            None
        } else {
            Some(self.canvas_bounds.get(&id)?.get())
        };
        let registry = self.runtime.read(cx).registry.read(cx);
        let (message, action) = if self.pending_home_close == Some(id) {
            let meta = registry
                .home_terminals()
                .iter()
                .find(|meta| meta.id == id)?;
            (
                format!(
                    "Close {}? Its shell and running commands will stop. Files remain on disk.",
                    meta.label
                ),
                Action::ConfirmHome(id),
            )
        } else {
            let meta = registry.meta(id)?;
            (
                format!(
                    "Close {} in {}? Its process will stop. The worktree stays on disk.",
                    meta.label, meta.project
                ),
                Action::ConfirmClose(id),
            )
        };
        let session_close = matches!(&action, Action::ConfirmClose(_));
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let gutter = if session_close { SPACE_3XL } else { SPACE_LG };
        let max_width = if session_close {
            SESSION_CLOSE_W
        } else {
            MODAL_W_SM
        };
        let available_width =
            (f32::from(window.viewport_size().width) / scale - gutter * 2.0).max(0.0);
        let width = bounds
            .map_or(max_width, |bounds| f32::from(bounds.size.width) / scale)
            .clamp(SIDEBAR_W, max_width)
            .min(available_width);
        let position = bounds.map_or_else(
            || gpui::point(gpui::px(gutter * scale), gpui::px(gutter * scale)),
            |bounds| gpui::point(bounds.left(), bounds.bottom()),
        );
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position_mode(gpui::AnchoredPositionMode::Window)
                    .position(position)
                    .snap_to_window_with_margin(gpui::px(gutter * scale))
                    .child(
                        div()
                            .id("canvas-confirmation")
                            .debug_selector(|| "canvas-confirmation".into())
                            .w(rpx(width))
                            .occlude()
                            .child(self.confirmation(&message, action, cx)),
                    ),
            )
            .into_any_element(),
        )
    }

    pub(in crate::views) fn action_source_bounds(&self) -> gpui::Bounds<gpui::Pixels> {
        self.action_source_bounds.get()
    }

    pub(in crate::views) fn edit_confirmation_open(&self, cx: &App) -> bool {
        self.codex_views
            .values()
            .any(|view| view.read(cx).edit_confirmation_open())
    }

    pub fn confirmation_open(&self) -> bool {
        self.project_decision
            || self.pending_close.is_some()
            || self.pending_home_close.is_some()
            || self.pending_remove.is_some()
            || self.pending_worktree_removal.is_some()
            || self.checkout_confirmation_open()
    }

    pub(super) fn confirmation(
        &self,
        label: &str,
        action: Action,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let session_close = matches!(&action, Action::ConfirmClose(_));
        let wrap_context = |text: &str| {
            text.chars()
                .map(|character| character.to_string())
                .collect::<Vec<_>>()
                .join("\u{200b}")
        };
        let session_meta = match &action {
            Action::ConfirmClose(id) => self.runtime.read(cx).registry.read(cx).meta(*id).cloned(),
            _ => None,
        };
        let verb = match &action {
            Action::ConfirmClose(_) => "Close session",
            Action::ConfirmHome(_) => "Close terminal",
            #[cfg(test)]
            Action::ConfirmRemove(_) => "Remove project",
            _ => "Confirm",
        };
        let confirm_button = self
            .control("confirm-close", verb, action, cx)
            .debug_selector(|| "confirm-close".into())
            .w_auto()
            .px(rpx(SPACE_LG))
            .text_color(c::RED())
            .when(!session_close, |button| {
                button.child(icon("check", ICON_XS, c::RED()))
            })
            .when(session_close, |button| {
                button
                    .text_size(rpx(TEXT_SMALL))
                    .min_h(rpx(CONTROL_H))
                    .border_1()
                    .border_color(c::alpha(c::RED(), 0.35))
                    .bg(c::alpha(c::RED(), 0.1))
                    .gap(rpx(SPACE_SM))
                    .child(icon("check", ICON_XS, c::RED()))
            })
            .child(verb);
        let cancel_button = self
            .control("cancel-close", "Cancel", Action::Cancel, cx)
            .debug_selector(|| "cancel-close".into())
            .w_auto()
            .px(rpx(SPACE_LG))
            .when(!session_close, |button| {
                button.child(icon("close", ICON_XS, c::FG_DIM()))
            })
            .when(session_close, |button| {
                button
                    .text_size(rpx(TEXT_SMALL))
                    .min_h(rpx(CONTROL_H))
                    .border_1()
                    .border_color(c::BORDER())
                    .bg(c::SURFACE_RAISED())
                    .gap(rpx(SPACE_SM))
                    .child(icon("close", ICON_XS, c::FG_DIM()))
            })
            .child("Cancel");
        let action_row = div()
            .flex()
            .min_w_0()
            .when(session_close, gpui::Styled::justify_end)
            .flex_wrap()
            .when(!session_close, |row| row.mt(rpx(SPACE_SM)))
            .gap(rpx(if session_close { SPACE_MD } else { SPACE_LG }));
        let action_row = if session_close {
            action_row.child(cancel_button).child(confirm_button)
        } else {
            action_row.child(confirm_button).child(cancel_button)
        };
        let confirmation = div()
            .id("sidebar-confirmation")
            .debug_selector(|| "sidebar-confirmation".into())
            .role(gpui::Role::Dialog)
            .aria_label(verb)
            .aria_description(label.to_string())
            .text_color(c::FG())
            .flex()
            .flex_col()
            .gap(rpx(if session_close { SPACE_MD } else { SPACE_2XL }))
            .p(rpx(if session_close { SPACE_2XL } else { SPACE_3XL }))
            .m(rpx(SPACE_SM))
            .rounded(rpx(RADIUS_GROUP))
            .min_w_0()
            .whitespace_normal()
            .border_1()
            .border_color(c::BORDER())
            .bg(c::SURFACE_RAISED())
            .child(if let Some(meta) = session_meta {
                div()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .text_size(rpx(TEXT_TITLE))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(format!("Close {}?", wrap_context(&meta.label))),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(wrap_context(&meta.project)),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_BODY))
                            .text_color(c::FG_DIM())
                            .child("Its process will stop. The worktree stays on disk."),
                    )
                    .into_any_element()
            } else {
                div()
                    .child(label.replace('/', "/\u{200b}"))
                    .into_any_element()
            })
            .child(action_row);
        motion::slow(confirmation, format!("sidebar-confirmation-{verb}"), cx)
    }

    /// Keep source-attached close dialogs mounted independently of the row's
    /// visibility, including responsive navigation collapse and list filtering.
    pub(super) fn navigation_close_confirmation(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.canvas_close_anchor.is_some() {
            return None;
        }
        let (id, message, action, source) = if let Some(id) = self.pending_close {
            let registry = self.runtime.read(cx).registry.read(cx);
            let meta = registry.meta(id)?;
            (
                id,
                format!(
                    "Close {} in {}? Its process will stop. The worktree stays on disk.",
                    meta.label, meta.project
                ),
                Action::ConfirmClose(id),
                self.session_close_bounds.get(&id)?.get(),
            )
        } else if let Some(id) = self.pending_home_close {
            let registry = self.runtime.read(cx).registry.read(cx);
            let meta = registry
                .home_terminals()
                .iter()
                .find(|meta| meta.id == id)?;
            (
                id,
                format!(
                    "Close {}? Its shell and running commands will stop. Files remain on disk.",
                    meta.label
                ),
                Action::ConfirmHome(id),
                self.home_close_bounds.get(&id)?.get(),
            )
        } else {
            return None;
        };
        let popup = self.confirmation_popup(&message, action, source, window, cx);
        Some(
            div()
                .id(("session-close-popup", id.raw()))
                .absolute()
                .debug_selector(move || format!("session-close-popup-{}", id.raw()))
                .child(gpui::deferred(popup))
                .into_any_element(),
        )
    }

    fn confirmation_popup(
        &self,
        label: &str,
        action: Action,
        trigger: gpui::Bounds<gpui::Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let preferred_width = if matches!(&action, Action::ConfirmClose(_)) {
            SESSION_CLOSE_W
        } else {
            SIDEBAR_W
        };
        let width = preferred_width
            .min((f32::from(window.viewport_size().width) / scale - SPACE_LG * 2.0).max(0.0));
        gpui::anchored()
            .position_mode(gpui::AnchoredPositionMode::Window)
            .position(gpui::point(trigger.right(), trigger.top()))
            .snap_to_window_with_margin(gpui::px(SPACE_LG * scale))
            .child(
                div()
                    .w(rpx(width))
                    .child(self.confirmation(label, action, cx)),
            )
            .into_any_element()
    }
}
