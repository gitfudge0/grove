//! Shared header control geometry and accessible naming.
use super::{motion, rpx, tokens::*};
use crate::theme as c;
use gpui::{div, prelude::*, Div, MouseButton, Stateful};

const FORM_FIELD_H: f32 = 60.;
const FORM_FIELD_INSET: f32 = 14.;
const FORM_FIELD_VALUE: f32 = 16.;

/// Grove's compact, rem-scaled tooltip surface; overlay behavior remains component-owned.
pub fn tooltip(
    label: impl Into<gpui::SharedString>,
    window: &gpui::Window,
) -> gpui_component::tooltip::Tooltip {
    let label = label.into();
    tooltip_content(
        move |_, _| {
            div()
                .max_w(rpx(304.))
                .whitespace_normal()
                .child(label.clone())
        },
        window,
    )
}

/// Shared tooltip surface for structured previews as well as plain labels.
pub fn tooltip_content<E: gpui::IntoElement>(
    content: impl Fn(&mut gpui::Window, &mut gpui::App) -> E + 'static,
    window: &gpui::Window,
) -> gpui_component::tooltip::Tooltip {
    gpui_component::tooltip::Tooltip::element(content)
        .font_family(crate::fonts::UI_FAMILY)
        .text_size(rpx(TEXT_SMALL))
        .line_height(rpx(SPACE_3XL))
        .px(rpx(SPACE_LG))
        .py(rpx(5.))
        .max_w(rpx(320.))
        .rounded(rpx(RADIUS_GROUP))
        .bg(gpui::rgb(0x1b1b1b))
        .text_color(gpui::rgb(0xf7f7f8))
        .border_1()
        .border_color(gpui::rgb(0x34343a))
        .shadow(vec![gpui::BoxShadow {
            color: gpui::hsla(0., 0., 0., 0.36),
            offset: gpui::point(gpui::px(0.), rpx(2.).to_pixels(window.rem_size())),
            blur_radius: rpx(8.).to_pixels(window.rem_size()),
            spread_radius: gpui::px(0.),
            inset: false,
        }])
}

pub fn header_control(id: &'static str, label: &'static str) -> Stateful<Div> {
    header_control_with_hover(id, label, c::BG_HOVER())
}

/// Shared control geometry with a semantic hover fill for inverse and disabled actions.
pub fn header_control_with_hover(
    id: &'static str,
    label: &'static str,
    hover: gpui::Hsla,
) -> Stateful<Div> {
    header_control_frame(id, label, hover)
        .tooltip(move |window, cx| tooltip(label, window).build(window, cx))
}

/// Compact header control with a structured preview on the shared tooltip surface.
pub fn header_control_with_tooltip(
    id: &'static str,
    label: &'static str,
    preview: impl Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyView + 'static,
) -> Stateful<Div> {
    header_control_frame(id, label, c::BG_HOVER()).tooltip(preview)
}

fn header_control_frame(id: &'static str, label: &'static str, hover: gpui::Hsla) -> Stateful<Div> {
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
        .hover(move |s| s.bg(hover))
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
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
            el.child(motion::fast(
                div()
                    .id(gpui::SharedString::from(format!("{id}-error")))
                    .role(gpui::Role::Alert)
                    .text_size(rpx(TEXT_BODY))
                    .text_color(c::FORM_ERROR())
                    .child(error.to_string()),
                format!("form-error-{id}-{error}"),
                cx,
            ))
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
            field.child(motion::fast(
                div()
                    .id(gpui::SharedString::from(format!("{id}-error")))
                    .role(gpui::Role::Alert)
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FORM_ERROR())
                    .child(message.to_string()),
                format!("project-form-error-{id}-{message}"),
                cx,
            ))
        })
}

pub fn form_action(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
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

/// Compact transcript actions; quiet actions share their surrounding surface.
pub fn transcript_action(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    primary: bool,
    window: &gpui::Window,
    cx: &gpui::App,
) -> gpui_component::button::Button {
    use gpui_component::{
        button::{Button, ButtonCustomVariant, ButtonVariants},
        Sizable, Size,
    };
    let (fill, foreground, hover) = if primary {
        (c::FG(), c::BG(), c::FG_DIM())
    } else {
        (c::BG_RAIL(), c::FG_DIM(), c::BG_HOVER())
    };
    Button::new(id)
        .label(label)
        .with_size(Size::XSmall)
        .h(rpx(CONTROL_H))
        .px(rpx(SPACE_LG))
        .text_size(rpx(TEXT_SMALL))
        .custom(
            ButtonCustomVariant::new(cx)
                .color(fill)
                .foreground(foreground)
                .hover(hover)
                .active(hover)
                .shadow(false),
        )
        .rounded(rpx(RADIUS_CONTROL).to_pixels(window.rem_size()))
}

/// Neutral selectable row for native question forms. The owner supplies interaction.
pub fn question_choice(
    id: impl Into<gpui::ElementId>,
    label: String,
    description: String,
    multiple: bool,
    selected: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(if multiple {
            gpui::Role::CheckBox
        } else {
            gpui::Role::RadioButton
        })
        .aria_label(label.clone())
        .aria_selected(selected)
        .when(multiple, |row| {
            row.aria_toggled(if selected {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
        })
        .tab_index(0)
        .w_full()
        .min_w_0()
        .flex()
        .items_start()
        .gap(rpx(SPACE_LG))
        .p(rpx(SPACE_LG))
        .rounded(rpx(RADIUS_CONTROL))
        .bg(if selected { c::BG_HL() } else { c::BG_RAIL() })
        .hover(|style| style.bg(c::BG_HOVER()))
        .focus_visible(|style| style.bg(c::BG_HOVER()).border_color(c::BORDER_STRONG()))
        .border_1()
        .border_color(c::BORDER_SOFT())
        .child(
            div()
                .size(rpx(ICON_MD))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(c::FG_DIM())
                .when(multiple, |shape| shape.rounded(rpx(RADIUS_CONTROL)))
                .when(!multiple, gpui::Styled::rounded_full)
                .when(selected, |shape| {
                    if multiple {
                        shape.child(crate::icons::icon("check", ICON_SM, c::FG()))
                    } else {
                        shape.child(div().size(rpx(SPACE_MD)).rounded_full().bg(c::FG()))
                    }
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_XS))
                .text_size(rpx(TEXT_BODY))
                .child(label)
                .when(!description.is_empty(), |row| {
                    row.child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(description),
                    )
                }),
        )
}

/// Expandable question-list row, retaining neutral open and keyboard focus states.
pub fn question_disclosure(
    id: impl Into<gpui::ElementId>,
    label: String,
    summary: String,
    expanded: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label.clone())
        .aria_expanded(expanded)
        .aria_description(if expanded {
            "Expanded. Activate to collapse"
        } else {
            "Collapsed. Activate to expand"
        })
        .tab_index(0)
        .tab_stop(true)
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .px(rpx(SPACE_3XL))
        .py(rpx(SPACE_2XL))
        .border_t_1()
        .border_color(c::BORDER_SOFT())
        .when(expanded, |row| row.bg(c::FIELD_FILL()))
        .hover(|row| row.bg(c::BG_HOVER()))
        .focus_visible(|row| row.bg(c::BG_HOVER()))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_XS))
                .child(div().child(label))
                .child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(summary),
                ),
        )
        .child(crate::icons::icon(
            if expanded { "chev-down" } else { "chev-right" },
            ICON_SM,
            c::FG_DIM(),
        ))
}

/// Neutral Settings action and choice geometry. Selection never changes its border.
pub fn settings_control(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    selected: bool,
    enabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(gpui::Role::Button)
        .tab_index(if enabled { 0 } else { -1 })
        .aria_label(label.into())
        .aria_selected(selected)
        .px(rpx(SPACE_LG))
        .h(rpx(CONTROL_H))
        .flex()
        .items_center()
        .rounded(rpx(RADIUS_CONTROL))
        .bg(if selected {
            c::BG_HOVER()
        } else {
            c::FIELD_FILL()
        })
        .border_1()
        .border_color(c::BORDER_SOFT())
        .text_size(rpx(TEXT_SMALL))
        .text_color(if enabled { c::FG() } else { c::FG_DIM() })
        .when(enabled, |el| {
            el.cursor_pointer()
                .hover(|style| style.bg(c::BG_HOVER()))
                .focus_visible(|style| style.bg(c::BG_HOVER()))
        })
        .when(!enabled, |el| el.opacity(OPACITY_DISABLED))
}

/// How an in-progress task's mark reads, from the owning session's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskPhase {
    /// The agent is working: the mark spins.
    Running,
    /// The agent is blocked on the user (approval or question).
    Waiting,
    /// The turn was interrupted or failed with the step unfinished.
    Halted,
    /// A newer turn began without publishing a list; everything reads muted.
    Stale,
}

// Remove once the task-list strip and sidebar consume these shared primitives.
/// The leading mark for one task row, at [`ICON_SM`]. `tick` drives the
/// Running spinner (pass `0` under reduced motion for a static arc).
pub fn task_mark(
    status: grove_core::tasks::TaskStatus,
    phase: TaskPhase,
    tick: u64,
) -> impl IntoElement {
    use grove_core::tasks::TaskStatus;
    let icon = crate::icons::icon;
    match (status, phase) {
        (TaskStatus::Completed, TaskPhase::Stale) => icon("check", ICON_SM, c::FG_MUTE()),
        (TaskStatus::Completed, _) => icon("check", ICON_SM, c::GREEN()),
        (TaskStatus::Pending, _) => icon("task-pending", ICON_SM, c::FG_MUTE()),
        (TaskStatus::InProgress, TaskPhase::Running) => {
            crate::icons::spinner(ICON_SM, c::BLUE(), tick)
        }
        (TaskStatus::InProgress, TaskPhase::Waiting) => icon("task-waiting", ICON_SM, c::AMBER()),
        (TaskStatus::InProgress, TaskPhase::Halted) => icon("stop", ICON_SM, c::FG_DIM()),
        (TaskStatus::InProgress, TaskPhase::Stale) => {
            crate::icons::spinner(ICON_SM, c::FG_MUTE(), 0)
        }
    }
}

// Remove once the task-list strip and sidebar consume these shared primitives.
/// A thin done/total progress track `width` design pixels wide; the fill turns
/// green once every task is done.
pub fn task_meter(done: usize, total: usize, width: f32) -> impl IntoElement {
    let fraction = if total == 0 {
        0.
    } else {
        done.min(total) as f32 / total as f32
    };
    let all_done = total > 0 && done >= total;
    div()
        .w(rpx(width))
        .h(rpx(SPACE_SM))
        .flex_shrink_0()
        .rounded(rpx(RADIUS_FULL))
        // BORDER equals the BG_HOVER row fill; the track must survive
        // selected and hovered sidebar rows.
        .bg(c::BORDER_STRONG())
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(gpui::relative(fraction))
                .rounded(rpx(RADIUS_FULL))
                .bg(if all_done { c::GREEN() } else { c::FG_DIM() }),
        )
}
