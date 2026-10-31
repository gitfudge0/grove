//! Transient app notifications; no permanent footer or reserved content height.
use gpui::{div, prelude::*, App};

use super::{
    rpx,
    status::{self, StatusKind},
    tokens::*,
};
use crate::{
    entities::toast::{Toast, ToastKind},
    theme as c,
};

pub(super) fn render(toast: Toast, cx: &mut App) -> gpui::AnyElement {
    let kind = if toast.kind == ToastKind::Error {
        StatusKind::Failed
    } else {
        StatusKind::Info
    };
    div()
        .absolute()
        .left(rpx(SPACE_3XL))
        .right(rpx(SPACE_3XL))
        .bottom(rpx(SPACE_3XL))
        .flex()
        .justify_end()
        .child(
            div()
                .id("app-toast")
                .debug_selector(|| "app-toast".into())
                .role(if toast.kind == ToastKind::Error {
                    gpui::Role::Alert
                } else {
                    gpui::Role::Status
                })
                .aria_label(toast.message.clone())
                .w_full()
                .max_w(rpx(MODAL_W_SM))
                .min_w_0()
                .flex()
                .items_start()
                .gap(rpx(SPACE_LG))
                .p(rpx(SPACE_2XL))
                .rounded(rpx(RADIUS_PANEL))
                .border_1()
                .border_color(c::BORDER())
                .bg(c::BG())
                .font_family(crate::fonts::UI_FAMILY)
                .text_size(rpx(TEXT_SMALL))
                .line_height(rpx(SPACE_3XL))
                .text_color(c::FG())
                .child(div().flex_shrink_0().child(status::mark(
                    "app-toast-mark",
                    kind,
                    ICON_SM,
                    cx,
                )))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_normal()
                        .child(toast.message),
                ),
        )
        .into_any_element()
}
