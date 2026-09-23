//! Shared header control geometry and accessible naming.
use super::{rpx, tokens::*};
use crate::theme as c;
use gpui::{div, prelude::*, Div, MouseButton, Stateful};

const FORM_FIELD_H: f32 = 60.;
const FORM_FIELD_INSET: f32 = 14.;
const FORM_FIELD_VALUE: f32 = 16.;

pub fn header_control(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label)
        .size(rpx(CHROME_CONTROL_H))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(rpx(RADIUS_CONTROL))
        .hover(|s| s.bg(c::BG_HOVER()))
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .tooltip(move |window, cx| {
            gpui_component::tooltip::Tooltip::new(label)
                .bg(c::BG_STRIP())
                .text_color(c::FG())
                .build(window, cx)
        })
}

/// A compound form well with an embedded label and error attached to its field.
pub fn form_field(
    id: &'static str,
    label: &'static str,
    input: &gpui::Entity<gpui_component::input::InputState>,
    error: Option<&str>,
    window: &gpui::Window,
    cx: &gpui::App,
) -> impl IntoElement {
    use gpui::Focusable as _;
    let fill = if input.read(cx).focus_handle(cx).is_focused(window) {
        c::BG_HOVER()
    } else {
        c::FIELD_FILL()
    };
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(gpui::Role::Group)
        .aria_label(label)
        .when_some(error, |el, error| el.aria_description(error.to_string()))
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_SM))
        .child(
            div()
                .h(rpx(FORM_FIELD_H))
                .flex_shrink_0()
                .min_w_0()
                .px(rpx(FORM_FIELD_INSET))
                .flex()
                .flex_col()
                .justify_center()
                .gap(rpx(SPACE_XS))
                .rounded(rpx(RADIUS_PANEL))
                .bg(fill)
                .border_1()
                .border_color(if error.is_some() {
                    c::FORM_ERROR()
                } else {
                    fill
                })
                .child(
                    div()
                        .text_size(rpx(TEXT_BODY))
                        .text_color(c::FG_DIM())
                        .child(label),
                )
                .child(
                    gpui_component::input::Input::new(input)
                        .aria_label(label)
                        .when_some(error, |input, error| {
                            input.aria_description(error.to_string())
                        })
                        .appearance(false)
                        .bordered(false)
                        .focus_bordered(false)
                        .text_size(rpx(FORM_FIELD_VALUE))
                        .text_color(c::FG())
                        .p_0(),
                ),
        )
        .when_some(error, |el, error| {
            el.child(
                div()
                    .id(gpui::SharedString::from(format!("{id}-error")))
                    .role(gpui::Role::Alert)
                    .text_size(rpx(TEXT_BODY))
                    .text_color(c::FORM_ERROR())
                    .child(error.to_string()),
            )
        })
}

/// Project-flow field: the neutral outline and compact value match the project canvas.
pub fn project_field_well(focused: bool, error: bool) -> Div {
    div()
        .h(rpx(FORM_FIELD_H))
        .flex_shrink_0()
        .min_w_0()
        .px(rpx(FORM_FIELD_INSET))
        .flex()
        .flex_col()
        .justify_center()
        .gap(rpx(SPACE_XS))
        .rounded(rpx(RADIUS_PANEL))
        .bg(if focused {
            c::BG_HOVER()
        } else {
            c::FIELD_FILL()
        })
        .border_1()
        .border_color(if error {
            c::FORM_ERROR()
        } else if focused {
            c::BORDER_STRONG()
        } else {
            c::BORDER_SOFT()
        })
}

/// An editable project field with the same well as read-only project values.
pub fn project_form_field(
    id: &'static str,
    label: &'static str,
    input: &gpui::Entity<gpui_component::input::InputState>,
    error: Option<&str>,
    window: &gpui::Window,
    cx: &gpui::App,
) -> impl IntoElement {
    use gpui::Focusable as _;
    let focused = input.read(cx).focus_handle(cx).is_focused(window);
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(gpui::Role::Group)
        .aria_label(label)
        .when_some(error, |field, message| {
            field.aria_description(message.to_string())
        })
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_SM))
        .child(
            project_field_well(focused, error.is_some())
                .child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(label),
                )
                .child(
                    gpui_component::input::Input::new(input)
                        .aria_label(label)
                        .when_some(error, |input, message| {
                            input.aria_description(message.to_string())
                        })
                        .appearance(false)
                        .bordered(false)
                        .focus_bordered(false)
                        .font_family(crate::fonts::UI_FAMILY)
                        .text_size(rpx(14.))
                        .text_color(c::FG())
                        .p_0(),
                ),
        )
        .when_some(error, |field, message| {
            field.child(
                div()
                    .id(gpui::SharedString::from(format!("{id}-error")))
                    .role(gpui::Role::Alert)
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FORM_ERROR())
                    .child(message.to_string()),
            )
        })
}

pub fn form_action(
    id: &'static str,
    label: &'static str,
    primary: bool,
    window: &gpui::Window,
    cx: &gpui::App,
) -> gpui_component::button::Button {
    use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants};
    let (fill, foreground, hover) = if primary {
        (c::FG(), c::BG(), c::FG_DIM())
    } else {
        (c::FIELD_FILL(), c::FG(), c::BG_HOVER())
    };
    Button::new(id)
        .label(label)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(fill)
                .foreground(foreground)
                .hover(hover)
                .active(hover)
                .shadow(false),
        )
        .rounded(gpui::px(
            RADIUS_PANEL * f32::from(window.rem_size()) / crate::zoom::REM_BASE,
        ))
}
