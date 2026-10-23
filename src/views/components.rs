//! Shared header control geometry and accessible naming.
use super::{motion, rpx, tokens::*};
use crate::theme as c;
use gpui::{div, prelude::*, AnimationExt as _, App, Div, MouseButton, Stateful};

const FORM_FIELD_H: f32 = FIELD_H;
pub(super) const FORM_FIELD_INSET: f32 = FIELD_INSET_X;
const FORM_FIELD_VALUE: f32 = FORM_VALUE_SIZE;

/// Preferred width for a truncating label whose trailing control stays adjacent.
pub fn truncation_safe_label_width(
    label: &str,
    font_size: gpui::Pixels,
    run: &gpui::TextRun,
    text_system: &gpui::WindowTextSystem,
) -> gpui::Pixels {
    let shaped_width = text_system
        .shape_line(
            label.to_owned().into(),
            font_size,
            std::slice::from_ref(run),
            None,
        )
        .width()
        .ceil();
    // GPUI truncates using separate glyph advances, which can exceed a kerned
    // word's intrinsic width. Reserve that budget before flex layout shrinks it.
    let font_id = text_system.resolve_font(&run.font);
    let glyph_width: gpui::Pixels = label
        .chars()
        .map(|ch| text_system.layout_width(font_id, font_size, ch))
        .sum();
    shaped_width.max(glyph_width.floor())
}

/// Grove's compact, rem-scaled tooltip surface; overlay behavior remains component-owned.
pub fn tooltip(
    label: impl Into<gpui::SharedString>,
    window: &gpui::Window,
) -> gpui_component::tooltip::Tooltip {
    let label = label.into();
    tooltip_content(
        move |_, _| {
            div()
                .max_w(rpx(TOOLTIP_TEXT_MAX_W))
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
        .py(rpx(TOOLTIP_PADDING_Y))
        .max_w(rpx(TOOLTIP_MAX_W))
        .rounded(rpx(RADIUS_GROUP))
        .bg(c::TOOLTIP_BG())
        .text_color(c::TOOLTIP_FG())
        .border_1()
        .border_color(c::TOOLTIP_BORDER())
        .shadow(vec![gpui::BoxShadow {
            color: c::TOOLTIP_SHADOW(),
            offset: gpui::point(gpui::px(0.), rpx(SPACE_XS).to_pixels(window.rem_size())),
            blur_radius: rpx(SPACE_LG).to_pixels(window.rem_size()),
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

pub(super) fn header_control_frame(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    hover: gpui::Hsla,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(gpui::Role::Button)
        .cursor_pointer()
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
        .gap_0()
        .when(error.is_some(), |field| {
            field
                .rounded(rpx(RADIUS_PANEL))
                .overflow_hidden()
                .border_1()
                .border_color(c::FORM_ERROR())
        })
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
                    c::BORDER_SOFT()
                })
                .child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .line_height(rpx(SPACE_3XL))
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
                        .line_height(rpx(FORM_VALUE_LINE_H))
                        .text_color(c::FG())
                        .p_0(),
                ),
        )
        .when_some(error, |el, error| {
            el.child(motion::fast(
                div()
                    .id(gpui::SharedString::from(format!("{id}-error")))
                    .role(gpui::Role::Alert)
                    .px(rpx(FIELD_INSET_X))
                    .py(rpx(SPACE_LG))
                    .bg(c::ERROR_WASH())
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
    form_field(id, label, input, error, window, cx)
}

pub fn form_action(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    primary: bool,
    window: &gpui::Window,
    cx: &gpui::App,
) -> gpui_component::button::Button {
    let (fill, foreground, hover) = if primary {
        (c::FG(), c::BG(), c::FG_DIM())
    } else {
        (c::FIELD_FILL(), c::FG(), c::BG_HOVER())
    };
    dense_action_with_colors(id, label, fill, foreground, hover, window, cx)
}

/// Compact transcript actions; quiet actions share their surrounding surface.
pub fn transcript_action(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    primary: bool,
    window: &gpui::Window,
    cx: &gpui::App,
) -> gpui_component::button::Button {
    let (fill, foreground, hover) = if primary {
        (c::FG(), c::BG(), c::FG_DIM())
    } else {
        (c::BG_RAIL(), c::FG_DIM(), c::BG_HOVER())
    };
    dense_action_with_colors(id, label, fill, foreground, hover, window, cx)
}

/// Full dense action recipe, including the vendor's inner label/icon/gap tier.
/// Custom semantic colors do not create a new geometry variant.
pub fn dense_action_with_colors(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    fill: gpui::Hsla,
    foreground: gpui::Hsla,
    hover: gpui::Hsla,
    window: &gpui::Window,
    cx: &gpui::App,
) -> gpui_component::button::Button {
    use gpui_component::{
        button::{Button, ButtonCustomVariant, ButtonVariants},
        Sizable, Size,
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
        .cursor_pointer()
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
        .cursor_pointer()
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
        .gap(rpx(SPACE_SM))
        .px(rpx(SPACE_3XL))
        .py(rpx(SPACE_2XL))
        .border_t_1()
        .border_color(c::BORDER_SOFT())
        .when(expanded, |row| row.bg(c::FIELD_FILL()))
        .hover(|row| row.bg(c::BG_HOVER()))
        .focus_visible(|row| row.bg(c::BG_HOVER()))
        .child(crate::icons::icon(
            if expanded { "chev-down" } else { "chev-right" },
            ICON_SM,
            c::FG_DIM(),
        ))
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
        .gap(rpx(SPACE_SM))
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
        .when(!enabled, |el| el.cursor_default().opacity(OPACITY_DISABLED))
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use gpui::AssetSource as _;

    #[test]
    fn activity_labels_fit_without_premature_truncation() {
        let platform = gpui_platform::current_platform(true);
        let text_system = std::sync::Arc::new(gpui::TextSystem::new(platform.text_system()));
        let font = crate::assets::Assets
            .load("fonts/Geist-Regular.ttf")
            .unwrap()
            .unwrap();
        text_system.add_fonts(vec![font]).unwrap();
        let window_text_system = gpui::WindowTextSystem::new(text_system.clone());
        let style = gpui::TextStyle {
            font_family: crate::fonts::UI_FAMILY.into(),
            ..Default::default()
        };
        let mut original_waiting_failures = 0;
        for zoom in [0.9, 1.0, 1.1, 1.2, 1.5, 2.0] {
            for label in ["Waiting…", "Using js…", "Reading a long source file…"] {
                let size = gpui::px(TEXT_SMALL * zoom);
                let run = style.to_run(label.len());
                let width = truncation_safe_label_width(label, size, &run, &window_text_system);
                let original_width = window_text_system
                    .shape_line(
                        label.to_owned().into(),
                        size,
                        std::slice::from_ref(&run),
                        None,
                    )
                    .width();
                let mut wrapper = text_system.line_wrapper(run.font, size);
                if label == "Waiting…"
                    && wrapper
                        .should_truncate_line(label, original_width, "…", gpui::TruncateFrom::End)
                        .is_some()
                {
                    original_waiting_failures += 1;
                }
                assert_eq!(
                    wrapper.should_truncate_line(label, width, "…", gpui::TruncateFrom::End),
                    None,
                    "{label} truncates despite available space at {zoom}x ({width:?})"
                );
                assert!(
                    wrapper
                        .should_truncate_line(label, width / 2., "…", gpui::TruncateFrom::End)
                        .is_some(),
                    "labels still truncate when their available width is constrained"
                );
            }
        }
        assert!(
            original_waiting_failures > 0,
            "the original content-sized Waiting… label must reproduce premature truncation"
        );
    }
}

const TOOL_SHIMMER_PERIOD: std::time::Duration = std::time::Duration::from_millis(1600);
const TOOL_SHIMMER_WIDTH: f32 = 0.2;

/// A label whose glyphs carry a moving highlight while `running`; static with reduced motion.
pub fn shimmer_label(
    id: gpui::ElementId,
    label: String,
    running: bool,
    cx: &App,
) -> gpui::AnyElement {
    if !running || cx.reduce_motion() {
        return div().child(label).into_any_element();
    }
    div()
        .with_animation(
            id,
            gpui::Animation::new(TOOL_SHIMMER_PERIOD).repeat(),
            move |element, progress| {
                element.child(
                    gpui::StyledText::new(label.clone())
                        .with_highlights(shimmer_highlights(&label, progress)),
                )
            },
        )
        .into_any_element()
}

pub(super) fn shimmer_highlights(
    text: &str,
    progress: f32,
) -> Vec<(std::ops::Range<usize>, gpui::HighlightStyle)> {
    let count = text.chars().count().max(1) as f32;
    let center = progress * (1.0 + TOOL_SHIMMER_WIDTH * 2.0) - TOOL_SHIMMER_WIDTH;
    let dim = c::FG_DIM();
    let bright = c::FG();
    text.char_indices()
        .enumerate()
        .map(|(index, (start, character))| {
            let strength =
                (1.0 - ((index as f32 / count - center).abs() / TOOL_SHIMMER_WIDTH)).max(0.0);
            let color = gpui::Hsla {
                h: dim.h + (bright.h - dim.h) * strength,
                s: dim.s + (bright.s - dim.s) * strength,
                l: dim.l + (bright.l - dim.l) * strength,
                a: dim.a + (bright.a - dim.a) * strength,
            };
            (
                start..start + character.len_utf8(),
                gpui::HighlightStyle {
                    color: Some(color),
                    ..Default::default()
                },
            )
        })
        .collect()
}

/// Shared visual switch track; the owning control supplies its accessible role and input.
pub fn switch_track(enabled: bool) -> Div {
    div()
        .w(rpx(SWITCH_W))
        .h(rpx(SWITCH_H))
        .flex_shrink_0()
        .p(rpx(SWITCH_INSET))
        .rounded_full()
        .flex()
        .items_center()
        .bg(if enabled {
            c::SWITCH_ON()
        } else {
            c::BORDER_STRONG()
        })
        .when(enabled, gpui::Styled::justify_end)
        .child(
            div()
                .size(rpx(SWITCH_THUMB))
                .rounded_full()
                .bg(c::SWITCH_THUMB()),
        )
}

/// Literal selectable readonly text; HTML escaping prevents paths becoming markup.
pub fn readonly_value(id: impl Into<gpui::ElementId>, value: String) -> impl IntoElement {
    let escaped = value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    gpui_component::text::TextView::html(id, escaped)
        .selectable(true)
        .font_family(crate::fonts::UI_FAMILY)
        .text_size(rpx(FORM_VALUE_SIZE))
        .line_height(rpx(FORM_VALUE_LINE_H))
        .text_color(c::FG())
        .min_w_0()
}

const TOOLTIP_TEXT_MAX_W: f32 = 304.0;
const TOOLTIP_PADDING_Y: f32 = 5.0;
const TOOLTIP_MAX_W: f32 = 320.0;

/// Join a dynamic field well to its semantic validation in normal document flow.
pub fn attached_validation(
    id: impl Into<gpui::ElementId>,
    well: impl IntoElement,
    error: Option<&str>,
) -> impl IntoElement {
    div()
        .id(id)
        .role(gpui::Role::Group)
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_0()
        .when_some(error, |field, message| {
            field
                .aria_description(message.to_owned())
                .rounded(rpx(RADIUS_PANEL))
                .overflow_hidden()
                .border_1()
                .border_color(c::FORM_ERROR())
        })
        .child(well)
        .when_some(error, |field, message| {
            field.child(
                div()
                    .id("attached-validation-message")
                    .role(gpui::Role::Alert)
                    .px(rpx(FIELD_INSET_X))
                    .py(rpx(SPACE_LG))
                    .bg(c::ERROR_WASH())
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FORM_ERROR())
                    .child(message.to_owned()),
            )
        })
}

#[cfg(test)]
mod action_recipe_tests {
    use super::*;
    struct ActionProbe {
        rem: f32,
        disabled: bool,
        loading: bool,
    }
    impl gpui::Render for ActionProbe {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            use gpui_component::Disableable;
            window.set_rem_size(gpui::px(self.rem));
            div()
                .flex()
                .flex_col()
                .items_start()
                .child(
                    settings_control(
                        "settings-gap-probe",
                        "Retry media preview",
                        false,
                        !self.disabled,
                    )
                    .child(
                        div()
                            .debug_selector(|| "settings-probe-glyph".into())
                            .child(crate::icons::icon("reset", ICON_SM, c::FG_DIM())),
                    )
                    .child(
                        div()
                            .debug_selector(|| "settings-probe-label".into())
                            .child("Retry"),
                    ),
                )
                .child(
                    transcript_action("dense-gap-probe", "Next lines", false, window, cx)
                        // Keep the label measurable while exercising the real vendor icon slot.
                        .icon_only()
                        .icon(gpui_component::Icon::default().path("icons/arrow-right.svg"))
                        .debug_selector(|| "dense-probe-button".into())
                        .disabled(self.disabled)
                        .loading(self.loading)
                        .child(
                            div()
                                .debug_selector(|| "dense-probe-label".into())
                                .child("Next lines"),
                        ),
                )
        }
    }
    #[gpui::test]
    fn text_icon_actions_keep_leading_gap_at_zoom_and_inactive_states(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_component::init);
        for rem in [crate::zoom::REM_BASE, 20.0] {
            let mut rest_label_offset = None;
            for (disabled, loading) in [(false, false), (true, false), (false, true)] {
                let (_, cx) = cx.add_window_view(|_, _| ActionProbe {
                    rem,
                    disabled,
                    loading,
                });
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                let scale = rem / crate::zoom::REM_BASE;
                let glyph = cx.debug_bounds("settings-probe-glyph").unwrap();
                let label = cx.debug_bounds("settings-probe-label").unwrap();
                assert_eq!(f32::from(label.left() - glyph.right()), SPACE_SM * scale);
                assert_eq!(f32::from(glyph.size.width), ICON_SM * scale);
                let button = cx.debug_bounds("dense-probe-button").unwrap();
                let label = cx.debug_bounds("dense-probe-label").unwrap();
                // Dense custom variants have no border. Their natural-width content
                // starts after shared padding, then the vendor's actual icon slot.
                let label_offset = f32::from(label.left() - button.left());
                assert_eq!(
                    label_offset - (SPACE_LG + ICON_SM) * scale,
                    SPACE_SM * scale
                );
                assert_eq!(
                    *rest_label_offset.get_or_insert(label_offset),
                    label_offset,
                    "disabled/loading must not move the label"
                );
            }
        }
    }
}
