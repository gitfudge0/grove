//! Shared header control geometry and accessible naming.
use super::{motion, rpx, tokens::*};
use crate::theme as c;
use gpui::{div, prelude::*, AnimationExt as _, App, Div, MouseButton, Stateful};

/// Header layout shared by pages, compact viewers and anchored previews.
/// Owners retain their surface insets, source focus and action semantics.
pub fn content_header() -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
}

/// A title yields to fixed navigation and actions in the same header.
pub fn content_header_title(title: impl Into<gpui::SharedString>) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_size(rpx(TEXT_TITLE))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(title.into())
}

/// Bottom form actions stay together and wrap at the content column inset.
/// Add secondary actions first and the primary action last.
pub fn form_actions() -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(rpx(SPACE_LG))
}

/// Labeled return navigation uses the same complete dense recipe everywhere.
pub fn back_action(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    window: &gpui::Window,
    cx: &gpui::App,
) -> gpui_component::button::Button {
    dense_action_with_colors(
        id,
        label,
        gpui::transparent_black(),
        c::FG_DIM(),
        c::BG_HOVER(),
        window,
        cx,
    )
    .icon(gpui_component::Icon::default().path("icons/arrow-left.svg"))
}

/// Static shortcut/help text with catalog modifier glyphs. The complete original
/// text remains the accessible name; input bindings and terminal text are untouched.
pub fn shortcut_hint(label: impl Into<gpui::SharedString>) -> Stateful<Div> {
    let label = label.into();
    let mut row = div()
        .id(format!("shortcut-hint-{label}"))
        .flex()
        .flex_wrap()
        .items_center()
        .gap(rpx(SPACE_SM))
        .aria_label(
            label
                .replace('⌃', "Control+")
                .replace('⇧', "Shift+")
                .replace('⌘', "Command+"),
        );
    for part in shortcut_parts(&label) {
        row = match part {
            ShortcutPart::Text(text) => row.child(text),
            ShortcutPart::Icon(name) => row.child(crate::icons::icon(name, ICON_SM, c::FG_DIM())),
        };
    }
    row
}

#[derive(Debug, PartialEq)]
enum ShortcutPart {
    Text(String),
    Icon(&'static str),
}

fn shortcut_parts(label: &str) -> Vec<ShortcutPart> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut chars = label.chars().peekable();
    while let Some(ch) = chars.next() {
        let (word, glyph) = if ch.is_ascii_alphabetic() {
            let mut word = String::from(ch);
            while chars.peek().is_some_and(char::is_ascii_alphabetic) {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            let glyph = match word.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => Some("control"),
                "shift" => Some("shift"),
                "cmd" | "command" => Some("command"),
                _ => None,
            };
            (word, glyph)
        } else {
            (
                ch.to_string(),
                match ch {
                    '⌃' => Some("control"),
                    '⇧' => Some("shift"),
                    '⌘' => Some("command"),
                    _ => None,
                },
            )
        };
        if let Some(glyph) = glyph {
            if !text.is_empty() {
                parts.push(ShortcutPart::Text(std::mem::take(&mut text)));
            }
            parts.push(ShortcutPart::Icon(glyph));
            // Modifier adjacency and the shared gap replace the plus separator.
            if chars.peek() == Some(&'+') {
                chars.next();
            }
        } else {
            text.push_str(&word);
        }
    }
    if !text.is_empty() {
        parts.push(ShortcutPart::Text(text));
    }
    parts
}

const FORM_FIELD_H: f32 = FIELD_H;
pub(super) const FORM_FIELD_INSET: f32 = FIELD_INSET_X;
const FORM_FIELD_VALUE: f32 = FORM_VALUE_SIZE;

/// Transient file-drop feedback shared by every editable input. It overlays the
/// editor rather than replacing it, so drag exit/cancel leaves text and focus intact.
/// Drag feedback is immediate; no animation should lag behind the pointer.
pub fn file_drop_overlay() -> Div {
    div()
        .debug_selector(|| "input-file-drop-overlay".into())
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .gap(rpx(SPACE_SM))
        .rounded(rpx(RADIUS_CONTROL))
        .border_1()
        .border_dashed()
        .border_color(c::BORDER_STRONG())
        .bg(c::BG_RAIL())
        .text_color(c::FG_DIM())
        .text_size(rpx(TEXT_SMALL))
        .invisible()
        .cursor_default()
        .group_drag_over::<gpui::ExternalPaths>("input-file-drop", gpui::Styled::visible)
        .child(crate::icons::icon("paperclip", ICON_SM, c::FG_DIM()))
        .child(
            div()
                .debug_selector(|| "input-file-drop-label".into())
                .child("Drop files here"),
        )
}

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
}

/// Unavailable header action preserves geometry without pointer or keyboard activation.
pub fn disabled_header_control(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
) -> Stateful<Div> {
    inline_disclosure_frame(id, label)
        .cursor_default()
        .opacity(0.5)
        .group("disabled-control")
}

/// Header action with an explicitly requested text tooltip.
pub fn header_control_with_label_tooltip(id: &'static str, label: &'static str) -> Stateful<Div> {
    header_control(id, label).tooltip(move |window, cx| tooltip(label, window).build(window, cx))
}

/// Header action with an explicit tooltip and custom hover fill.
pub fn header_control_with_hover_tooltip(
    id: &'static str,
    label: &'static str,
    hover: gpui::Hsla,
) -> Stateful<Div> {
    header_control_with_hover(id, label, hover)
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
    inline_disclosure_frame(id, label).hover(move |style| style.bg(hover).text_color(c::FG()))
}

/// Inline disclosure: foreground-only pointer feedback; owner retains focus/open fills.
pub fn inline_disclosure(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
) -> Stateful<Div> {
    inline_disclosure_frame(id, label).hover(|style| style.text_color(c::FG()))
}

/// Inline disclosure that transfers pointer focus and supports normal keyboard activation.
/// Use when the disclosure owns subsequent keyboard input, rather than preserving editor focus.
pub fn focusable_inline_disclosure(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
) -> Stateful<Div> {
    inline_disclosure_base(id, label)
        .tab_index(0)
        .hover(|style| style.text_color(c::FG()))
}

fn inline_disclosure_frame(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
) -> Stateful<Div> {
    inline_disclosure_base(id, label).on_mouse_down(MouseButton::Left, |_, window, cx| {
        window.prevent_default();
        cx.stop_propagation();
    })
}

fn inline_disclosure_base(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
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
        .group("neutral-control")
        .text_color(c::FG_DIM())
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
                        .file_drop_overlay(crate::views::components::file_drop_overlay())
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

/// Compact search field for navigation lists. The well owns its border and focus ring.
pub fn search_field_well(focused: bool) -> Div {
    compact_field_well(focused)
}

/// Compact single-line field for constrained chrome. The well owns its border and fill.
pub fn compact_field_well(focused: bool) -> Div {
    div()
        .h(rpx(ROW_MIN_H))
        .flex_shrink_0()
        .min_w_0()
        .px(rpx(SPACE_XL))
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .rounded(rpx(RADIUS_GROUP))
        .bg(c::FIELD_FILL())
        .border_1()
        .border_color(if focused {
            c::SEL_RING()
        } else {
            c::BORDER_SOFT()
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
        (c::FIELD_FILL(), c::FG_DIM(), c::BG_HOVER())
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

/// Shared main-conversation input surface. Compact owners may reduce its
/// padding while keeping the same surface, seam and corner treatment.
pub fn composer_surface() -> Div {
    div()
        .flex_shrink_0()
        .w_full()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_LG))
        .px(rpx(SPACE_2XL))
        .pt(rpx(SPACE_LG))
        .pb(rpx(SPACE_2XL))
        .rounded(rpx(RADIUS_PANEL))
        .border_1()
        .border_color(c::BORDER_SOFT())
        .bg(c::BG_RAIL())
}

/// Retarget from the currently painted spread, so rapid focus reversals do not snap.
struct ComposerSpread {
    focused: bool,
    from: f32,
    started: std::time::Instant,
}
impl ComposerSpread {
    fn target(focused: bool) -> f32 {
        if focused {
            COMPOSER_GLOW_SPREAD
        } else {
            COMPOSER_GLOW_REST_SPREAD
        }
    }
    fn new(focused: bool, now: std::time::Instant) -> Self {
        Self {
            focused,
            from: Self::target(focused),
            started: now,
        }
    }
    fn sample(&self, now: std::time::Instant) -> (f32, bool) {
        let target = Self::target(self.focused);
        let elapsed = now.saturating_duration_since(self.started);
        let duration = std::time::Duration::from_millis(MOTION_SLOW_MS);
        if elapsed >= duration || (self.from - target).abs() < f32::EPSILON {
            return (target, false);
        }
        if elapsed.is_zero() {
            return (self.from, true);
        }
        let progress = motion::ease_standard(elapsed.as_secs_f32() / duration.as_secs_f32());
        (self.from + (target - self.from) * progress, true)
    }
    fn update(
        &mut self,
        focused: bool,
        reduce_motion: bool,
        now: std::time::Instant,
    ) -> (f32, bool) {
        if reduce_motion || self.focused != focused {
            self.from = if reduce_motion {
                Self::target(focused)
            } else {
                self.sample(now).0
            };
            self.focused = focused;
            self.started = now;
        }
        self.sample(now)
    }
}

#[cfg(test)]
mod composer_spread_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn spread_reverses_from_current_value_and_stops_at_rest() {
        let start = Instant::now();
        let duration = Duration::from_millis(MOTION_SLOW_MS);
        let mut state = ComposerSpread::new(false, start);
        assert_eq!(state.sample(start), (COMPOSER_GLOW_REST_SPREAD, false));
        assert_eq!(
            state.update(true, false, start),
            (COMPOSER_GLOW_REST_SPREAD, true)
        );
        let reversal = start + duration / 4;
        let midway = state.sample(reversal).0;
        assert!(midway > COMPOSER_GLOW_REST_SPREAD && midway < COMPOSER_GLOW_SPREAD);
        assert!((state.update(false, false, reversal).0 - midway).abs() < 0.0001);
        let retreat = state.sample(reversal + duration / 4).0;
        assert!(retreat < midway && retreat > COMPOSER_GLOW_REST_SPREAD);
        assert_eq!(
            state.sample(reversal + duration),
            (COMPOSER_GLOW_REST_SPREAD, false)
        );
        // Ordinary redraws must not restart the transition clock.
        state.update(true, false, reversal + duration);
        state.update(true, false, reversal + duration + duration / 2);
        assert_eq!(
            state.sample(reversal + duration * 2),
            (COMPOSER_GLOW_SPREAD, false)
        );
    }

    #[test]
    fn reduced_motion_interrupts_spread_without_more_frames() {
        let start = Instant::now();
        let mut state = ComposerSpread::new(false, start);
        state.update(true, false, start);
        assert_eq!(
            state.update(true, true, start + Duration::from_millis(50)),
            (COMPOSER_GLOW_SPREAD, false)
        );
        assert_eq!(
            state.update(false, true, start + Duration::from_millis(60)),
            (COMPOSER_GLOW_REST_SPREAD, false)
        );
        assert_eq!(
            state.update(false, false, start + Duration::from_millis(70)),
            (COMPOSER_GLOW_REST_SPREAD, false)
        );
    }
}

/// Paint the focus light behind a measured writing surface, outside any
/// scrolling/animation clip owned by the surface's immediate container.
pub fn composer_focus_glow(
    bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    focused: bool,
    enabled: bool,
    id: impl Into<gpui::ElementId>,
    window: &mut gpui::Window,
    cx: &mut App,
) -> gpui::AnyElement {
    if !enabled || !c::composer_focus_glow_enabled() {
        return div().into_any_element();
    }
    let id = id.into();
    let now = std::time::Instant::now();
    let reduced = cx.reduce_motion();
    let state = window.use_keyed_state(id.clone(), cx, |_, _| ComposerSpread::new(focused, now));
    let (spread, moving) = state.update(cx, |state, _| state.update(focused, reduced, now));
    if moving {
        window.request_animation_frame();
    }
    let light = gpui::canvas(
        |_, _, _| {},
        move |_, (), window, _| {
            let bounds = bounds.get();
            if bounds.size.width > gpui::px(0.) {
                window.paint_drop_shadows(
                    bounds,
                    gpui::Corners::all(rpx(RADIUS_PANEL).to_pixels(window.rem_size())),
                    &[composer_focus_shadow(window, spread)],
                );
            }
        },
    )
    .absolute()
    .size_full();
    // Div owns opacity propagation in GPUI; Canvas does not apply it itself.
    let glow = div().absolute().size_full().child(light);
    if !focused {
        return glow.opacity(COMPOSER_GLOW_MIN_OPACITY).into_any_element();
    }
    if cx.reduce_motion() {
        return glow.into_any_element();
    }
    glow.with_animation(
        id,
        gpui::Animation::new(COMPOSER_GLOW_PERIOD).repeat(),
        |glow, progress| {
            // Cosine keeps both turnarounds and the repeat seam smooth. Only
            // the underlay pulses: the writing surface and text stay steady.
            let wave = (1.0 - (progress * std::f32::consts::TAU).cos()) * 0.5;
            glow.opacity(COMPOSER_GLOW_MIN_OPACITY + (1.0 - COMPOSER_GLOW_MIN_OPACITY) * wave)
        },
    )
    .into_any_element()
}

/// Shared compact light, also painted at the conversation level to avoid the
/// composer's scrolling rail clipping the ambient falloff.
fn composer_focus_shadow(window: &gpui::Window, spread: f32) -> gpui::BoxShadow {
    gpui::BoxShadow {
        color: c::COMPOSER_FOCUS_GLOW(),
        offset: gpui::point(
            gpui::px(0.),
            rpx(COMPOSER_GLOW_OFFSET_Y).to_pixels(window.rem_size()),
        ),
        blur_radius: rpx(COMPOSER_GLOW_BLUR).to_pixels(window.rem_size()),
        spread_radius: rpx(spread).to_pixels(window.rem_size()),
        inset: false,
    }
}

/// Existing checkout script register geometry, shared without changing its owner.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct CheckoutRegisterRecipe {
    pub height: f32,
    pub padding_x: f32,
    pub gap: f32,
    pub icon_slot: f32,
    pub icon_size: f32,
    pub text_size: f32,
    pub padding_token: &'static str,
    pub gap_token: &'static str,
    pub text_token: &'static str,
    pub line_height_policy: &'static str,
}
pub const CHECKOUT_REGISTER_RECIPE: CheckoutRegisterRecipe = CheckoutRegisterRecipe {
    height: CONTROL_H + SPACE_SM,
    padding_x: SPACE_2XL,
    gap: SPACE_SM,
    icon_slot: ICON_LG,
    icon_size: ICON_SM,
    text_size: TEXT_BODY,
    padding_token: "space_2xl",
    gap_token: "space_sm",
    text_token: "text_body",
    line_height_policy: "inherit-native-context",
};
pub fn checkout_register_frame() -> Div {
    let r = CHECKOUT_REGISTER_RECIPE;
    div()
        .w_full()
        .min_w_0()
        .h(rpx(r.height))
        .px(rpx(r.padding_x))
        .flex()
        .items_center()
        .gap(rpx(r.gap))
}
pub fn checkout_register_leading() -> Div {
    div()
        .w(rpx(CHECKOUT_REGISTER_RECIPE.icon_slot))
        .flex_shrink_0()
}
pub fn checkout_register_label() -> Div {
    div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_size(rpx(CHECKOUT_REGISTER_RECIPE.text_size))
}

/// Authoritative geometry shared by the native builder and reference exporter.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct DenseActionRecipe {
    pub height: f32,
    pub padding_x: f32,
    pub text_size: f32,
    pub radius: f32,
    pub gap: f32,
    pub icon_size: f32,
    pub vendor_size: DenseActionSize,
    pub disabled_opacity: f32,
    pub loading_opacity: f32,
    pub loading_icon: &'static str,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DenseActionSize {
    XSmall,
}
impl DenseActionSize {
    fn native(self) -> gpui_component::Size {
        match self {
            Self::XSmall => gpui_component::Size::XSmall,
        }
    }
}

pub const DENSE_ACTION_RECIPE: DenseActionRecipe = DenseActionRecipe {
    height: CONTROL_H,
    padding_x: SPACE_LG,
    text_size: TEXT_SMALL,
    radius: RADIUS_CONTROL,
    gap: SPACE_SM,
    icon_size: ICON_SM,
    vendor_size: DenseActionSize::XSmall,
    disabled_opacity: gpui_component::button::Button::CUSTOM_DISABLED_OPACITY,
    loading_opacity: gpui_component::button::Button::LOADING_OPACITY,
    loading_icon: "status-working",
};

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
        Sizable,
    };
    let recipe = DENSE_ACTION_RECIPE;
    let neutral = foreground == c::FG() || foreground == c::FG_DIM();
    let rest_foreground = if neutral { c::FG_DIM() } else { foreground };
    Button::new(id)
        .group(if neutral {
            "neutral-control"
        } else {
            "semantic-control"
        })
        .label(label)
        .loading_icon(
            gpui_component::Icon::default().path(format!("icons/{}.svg", recipe.loading_icon)),
        )
        .with_size(recipe.vendor_size.native())
        .h(rpx(recipe.height))
        .px(rpx(recipe.padding_x))
        .text_size(rpx(recipe.text_size))
        .custom(
            ButtonCustomVariant::new(cx)
                .color(fill)
                .foreground(rest_foreground)
                .hover_foreground(if neutral { c::FG() } else { foreground })
                .hover(if neutral { c::BG_HOVER() } else { hover })
                .active(if neutral { c::BG_HOVER() } else { hover })
                .shadow(false),
        )
        .rounded(rpx(recipe.radius).to_pixels(window.rem_size()))
}

// Compact monograms are secondary to the adjacent full project name.
const PROJECT_INITIALS_TEXT_SIZE: f32 = 9.0;

/// Compact project identity using the project picker's capital-initial rule.
pub fn project_initials_badge(name: &str) -> Div {
    let initials = name
        .split_whitespace()
        .take(2)
        .filter_map(|word| word.chars().next())
        .flat_map(char::to_uppercase)
        .collect::<String>();
    div()
        .size(rpx(ICON_20))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(c::BORDER_STRONG())
        .bg(c::BG_HOVER())
        .text_size(rpx(PROJECT_INITIALS_TEXT_SIZE))
        .text_color(c::FG())
        .child(initials)
}

/// Dense disclosure with foreground-only pointer/open feedback. Keyboard focus
/// retains a separate ring; geometry stays owned by the dense action recipe.
pub fn dense_disclosure_action(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    window: &gpui::Window,
    cx: &gpui::App,
) -> gpui_component::button::Button {
    use gpui_component::button::{ButtonCustomVariant, ButtonVariants};
    dense_action_with_colors(
        id,
        label,
        gpui::transparent_black(),
        c::FG_DIM(),
        gpui::transparent_black(),
        window,
        cx,
    )
    .custom(
        ButtonCustomVariant::new(cx)
            .color(gpui::transparent_black())
            .foreground(c::FG_DIM())
            .hover(gpui::transparent_black())
            .hover_foreground(c::FG())
            .active(gpui::transparent_black())
            .shadow(false),
    )
    .focus_visible(|style| style.border_1().border_color(c::SEL_RING()))
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
        .group("neutral-control")
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
        .text_color(c::FG_DIM())
        .bg(if selected { c::BG_HL() } else { c::BG_RAIL() })
        .when(selected, |row| row.text_color(c::FG()))
        .hover(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
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
                            .text_color(if selected { c::FG() } else { c::FG_DIM() })
                            .group_hover("neutral-control", |text| text.text_color(c::FG()))
                            .child(description),
                    )
                }),
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
        .text_color(if selected { c::FG() } else { c::FG_DIM() })
        .when(enabled, |el| {
            el.cursor_pointer()
                .group("neutral-control")
                .hover(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
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
        (TaskStatus::Completed, TaskPhase::Stale) => {
            icon("status-success", ICON_SM, c::STATUS_SUCCESS())
        }
        (TaskStatus::Completed, _) => icon("status-success", ICON_SM, c::STATUS_SUCCESS()),
        (TaskStatus::Pending, _) => icon("task-pending", ICON_SM, c::STATUS_NEUTRAL()),
        (TaskStatus::InProgress, TaskPhase::Running) => {
            crate::icons::spinner(ICON_SM, c::STATUS_INFO(), tick)
        }
        (TaskStatus::InProgress, TaskPhase::Waiting) => {
            icon("status-attention", ICON_SM, c::STATUS_WARNING())
        }
        (TaskStatus::InProgress, TaskPhase::Halted) => {
            icon("status-stopped", ICON_SM, c::STATUS_NEUTRAL())
        }
        (TaskStatus::InProgress, TaskPhase::Stale) => {
            crate::icons::spinner(ICON_SM, c::STATUS_NEUTRAL(), 0)
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

    #[gpui::test]
    fn readonly_html_updates_preserve_full_path_and_literal_copy(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;
        use gpui_component::text::TextViewState;
        cx.update(gpui_component::init);
        let state = cx.update(|cx| cx.new(|cx| TextViewState::html("/tmp/project/", cx)));
        cx.run_until_parked();
        state.update(cx, |state, cx| {
            state.set_text_incremental("/tmp/project/work&lt;&amp;&gt;.tree", cx);
        });
        cx.run_until_parked();
        state.update(cx, |state, cx| {
            assert_eq!(
                state.source().as_str(),
                "/tmp/project/work&lt;&amp;&gt;.tree"
            );
            state.select_all(cx);
            assert_eq!(
                state.selected_text().trim_end_matches('\n'),
                "/tmp/project/work<&>.tree"
            );
        });
        // The source remains identical on an ordinary rerender, preserving selection.
        state.update(cx, |state, cx| {
            state.set_text_incremental("/tmp/project/work&lt;&amp;&gt;.tree", cx);
        });
        cx.run_until_parked();
        assert_eq!(
            state
                .read_with(cx, |state, _| state.selected_text())
                .trim_end_matches('\n'),
            "/tmp/project/work<&>.tree"
        );
        state.update(cx, |state, cx| {
            state.set_text_incremental("/tmp/other&amp;checkout", cx);
        });
        cx.run_until_parked();
        state.update(cx, |state, cx| {
            state.select_all(cx);
            assert_eq!(
                state.selected_text().trim_end_matches('\n'),
                "/tmp/other&checkout"
            );
        });
    }

    #[test]
    fn activity_labels_fit_without_premature_truncation() {
        let platform = gpui_platform::current_platform(true);
        let text_system = std::sync::Arc::new(gpui::TextSystem::new(platform.text_system()));
        let font = crate::assets::Assets
            .load("fonts/DMSans-Regular.ttf")
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
    // The animated spans own explicit colors, so changing inherited text color
    // cannot brighten them on hover. Keep the animated text in layout and swap
    // its paint for a foreground copy while the owning neutral control is hovered.
    let hover_label = label.clone();
    div()
        .relative()
        .child(
            div()
                .group_hover("neutral-control", |style| style.opacity(0.0))
                .with_animation(
                    id,
                    gpui::Animation::new(TOOL_SHIMMER_PERIOD).repeat(),
                    move |element, progress| {
                        element.child(
                            gpui::StyledText::new(label.clone())
                                .with_highlights(shimmer_highlights(&label, progress)),
                        )
                    },
                ),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .text_color(c::FG())
                .opacity(0.0)
                .group_hover("neutral-control", |style| style.opacity(1.0))
                .child(hover_label),
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

/// Shared grouped preference choices, matching Settings' existing choice recipe.
pub fn preference_choices() -> Div {
    div()
        .flex()
        .flex_wrap()
        .flex_shrink_0()
        .gap(rpx(SPACE_XS))
        .p(rpx(SPACE_XS))
        .border_1()
        .border_color(c::BORDER_SOFT())
        .rounded(rpx(RADIUS_CONTROL))
}

/// Explicit Off/On actions. Re-selecting a choice sets that value again rather than toggling.
pub fn boolean_choices<T: 'static>(
    id: &'static str,
    label: &'static str,
    value: bool,
    enabled: bool,
    cx: &gpui::Context<T>,
    on_change: impl Fn(&mut T, bool, &mut gpui::Window, &mut gpui::Context<T>) + Clone + 'static,
) -> Div {
    let mut group = preference_choices();
    for (selected_value, text) in [(false, "Off"), (true, "On")] {
        let click = on_change.clone();
        let key = on_change.clone();
        group = group.child(
            settings_control(
                format!("{id}-{selected_value}"),
                format!("{label} {text}"),
                value == selected_value,
                enabled,
            )
            .border_0()
            .bg(if value == selected_value {
                c::BG_HOVER()
            } else {
                c::BG()
            })
            .child(text)
            .when(enabled, |control| {
                control
                    .on_click(cx.listener(move |this, _, window, cx| {
                        click(this, selected_value, window, cx);
                    }))
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                && !event.is_held
                            {
                                key(this, selected_value, window, cx);
                                window.prevent_default();
                                cx.stop_propagation();
                            }
                        },
                    ))
            }),
        );
    }
    group
}

/// Literal selectable readonly text; HTML escaping prevents paths becoming markup.
pub fn readonly_value(
    id: impl Into<gpui::ElementId>,
    value: String,
) -> gpui_component::text::TextView {
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
                    .text_color(c::STATUS_ERROR())
                    .flex()
                    .items_start()
                    .gap(rpx(SPACE_SM))
                    .child(div().flex_shrink_0().child(crate::icons::icon(
                        "status-failed",
                        ICON_SM,
                        c::STATUS_ERROR(),
                    )))
                    .child(div().min_w_0().child(message.to_owned())),
            )
        })
}

#[cfg(test)]
mod action_recipe_tests {
    use super::*;
    struct HeaderLayoutProbe;
    impl gpui::Render for HeaderLayoutProbe {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(
                    content_header()
                        .debug_selector(|| "layout-header".into())
                        .child(
                            div()
                                .w(rpx(CONTROL_H))
                                .h(rpx(CONTROL_H))
                                .flex_shrink_0()
                                .debug_selector(|| "layout-back".into()),
                        )
                        .child(
                            content_header_title(
                                "A long document title that must yield to navigation and actions",
                            )
                            .debug_selector(|| "layout-title".into()),
                        )
                        .child(
                            div()
                                .w(rpx(CONTROL_H))
                                .h(rpx(CONTROL_H))
                                .flex_shrink_0()
                                .debug_selector(|| "layout-close".into()),
                        ),
                )
                .child(
                    form_actions()
                        .debug_selector(|| "layout-actions".into())
                        .child(
                            div()
                                .w(rpx(TEXTAREA_H))
                                .h(rpx(CONTROL_H))
                                .flex_shrink_0()
                                .debug_selector(|| "layout-secondary".into()),
                        )
                        .child(
                            div()
                                .w(rpx(TEXTAREA_H))
                                .h(rpx(CONTROL_H))
                                .flex_shrink_0()
                                .debug_selector(|| "layout-primary".into()),
                        ),
                )
        }
    }

    #[gpui::test]
    fn header_slots_stay_visible_and_form_actions_wrap(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (_, cx) = cx.add_window_view(|_, _| HeaderLayoutProbe);
        for (width, rem) in [(500., 16.), (250., 16.), (360., 24.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(400.)));
            cx.update(|window, cx| {
                window.set_rem_size(gpui::px(rem));
                let _ = window.draw(cx);
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let header = cx.debug_bounds("layout-header").unwrap();
            let back = cx.debug_bounds("layout-back").unwrap();
            let title = cx.debug_bounds("layout-title").unwrap();
            let close = cx.debug_bounds("layout-close").unwrap();
            assert!(back.left() >= header.left() && back.right() <= title.left());
            assert!(title.right() <= close.left() && close.right() <= header.right());
            let actions = cx.debug_bounds("layout-actions").unwrap();
            let secondary = cx.debug_bounds("layout-secondary").unwrap();
            let primary = cx.debug_bounds("layout-primary").unwrap();
            assert_eq!(secondary.left(), actions.left());
            if width < (TEXTAREA_H * 2. + SPACE_LG) * rem / crate::zoom::REM_BASE {
                assert!(primary.top() >= secondary.bottom());
                assert_eq!(primary.left(), actions.left());
            } else {
                assert_eq!(primary.top(), secondary.top());
                assert!(primary.left() >= secondary.right());
            }
        }
    }

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

/// Shared compact dropdown geometry; menu owners retain focus and activation.
pub fn dropdown_menu(
    menu: gpui_component::menu::PopupMenu,
    window: &gpui::Window,
) -> gpui_component::menu::PopupMenu {
    menu.menu_style(gpui_component::menu::MenuStyle {
        row_height: rpx(MENU_ROW_H).to_pixels(window.rem_size()),
        inset: rpx(SPACE_LG).to_pixels(window.rem_size()),
        padding: rpx(SPACE_MD).to_pixels(window.rem_size()),
        gap: rpx(SPACE_LG).to_pixels(window.rem_size()),
        radius: rpx(RADIUS_CHROME).to_pixels(window.rem_size()),
        font_size: rpx(TEXT_BODY).to_pixels(window.rem_size()),
    })
}

/// Visual row recipe for custom dropdowns with application-owned keyboard routing.
pub fn dropdown_row(id: impl Into<gpui::ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .role(gpui::Role::MenuItem)
        .cursor_pointer()
        .h(rpx(MENU_ROW_H))
        .flex_shrink_0()
        .text_size(rpx(TEXT_BODY))
        .text_color(c::FG_DIM())
        .group("neutral-control")
        .font_weight(gpui::FontWeight::MEDIUM)
        .px(rpx(SPACE_LG))
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .rounded(rpx(RADIUS_CHROME))
}

#[cfg(test)]
mod shortcut_hint_tests {
    use super::{
        shortcut_parts,
        ShortcutPart::{Icon, Text},
    };

    #[test]
    fn modifiers_preserve_keys_prose_and_plus_keys() {
        assert_eq!(
            shortcut_parts("Ctrl+Shift++"),
            vec![Icon("control"), Icon("shift"), Text("+".into())]
        );
        assert_eq!(
            shortcut_parts("⌃⇧1"),
            vec![Icon("control"), Icon("shift"), Text("1".into())]
        );
        assert_eq!(
            shortcut_parts("Shift+Enter for a new line"),
            vec![Icon("shift"), Text("Enter for a new line".into())]
        );
        assert_eq!(
            shortcut_parts("shift fine · enter/esc done"),
            vec![Icon("shift"), Text(" fine · enter/esc done".into())]
        );
        assert_eq!(
            shortcut_parts("Shifted controls"),
            vec![Text("Shifted controls".into())]
        );
        assert_eq!(
            shortcut_parts("cmd+alt+1–9"),
            vec![Icon("command"), Text("alt+1–9".into())]
        );
    }
}
