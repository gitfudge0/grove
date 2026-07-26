//! Selectable list rows shared by modals and the session launcher.

use crate::gui::metrics::ROW_H;
use crate::gui::palette as c;
use iced::border::Radius;
use iced::widget::{button, container, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};
/// One selectable row inside a modal list (agent picker, theme picker,
/// directory matches). Shared so every list uses the same active/hover
/// treatment: active rows get `bg_highlight`, hovered rows `bg_hover`. Fixed
/// at the shared 28px `ROW_H` with square corners — see
/// [`modal_list_row_sized`] for the command palette's taller, rounded rows.
pub(in crate::gui) fn modal_list_row<'a, M: Clone + 'a>(
    label: impl Into<Element<'a, M>>,
    active: bool,
    msg: M,
) -> Element<'a, M> {
    modal_list_row_sized(label, active, msg, ROW_H, 0.0, 10.0)
}

/// Generalized form of [`modal_list_row`]: same active/hover fill logic
/// (`bg_highlight` / `bg_hover`), but with caller-chosen height, corner
/// radius, and horizontal padding. The command palette uses this directly
/// for its 44px main rows and 36px single-line action rows, both with a 6px
/// radius (`modal_list_row` itself keeps 0 radius so every other list's
/// square-cornered rows are unaffected).
pub(in crate::gui) fn modal_list_row_sized<'a, M: Clone + 'a>(
    label: impl Into<Element<'a, M>>,
    active: bool,
    msg: M,
    height: f32,
    radius: f32,
    pad_x: f32,
) -> Element<'a, M> {
    button(
        container(label)
            .width(Length::Fill)
            .height(height)
            .align_y(iced::Alignment::Center)
            .padding(Padding::from([0.0, pad_x]))
            .clip(true),
    )
    .on_press(msg)
    .width(Length::Fill)
    .padding(0)
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if active {
                Some(Background::Color(c::BG_HL()))
            } else if hovered {
                Some(Background::Color(c::BG_HOVER()))
            } else {
                None
            },
            text_color: if active { c::FG() } else { c::FG_DIM() },
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(radius),
            },
            shadow: Shadow::default(),
            snap: false,
        }
    })
    .into()
}

/// Command palette row height (recents/combo rows) — taller than the shared
/// 28px `ROW_H` used elsewhere, per the palette redesign. Kept local to this
/// module rather than added to `metrics.rs`, which stays a global constant
/// other lists depend on.
pub(in crate::gui) const PALETTE_ROW_H: f32 = 44.0;
const PALETTE_ROW_RADIUS: f32 = 6.0;
const PALETTE_ROW_PAD_X: f32 = 12.0;

/// A selectable row inside the session launcher's palette.
///
/// - `active && focused` (the selected row, keyboard-driven) gets the
///   prominent treatment — a cyan-tinted gradient fill, a cyan ring, a left
///   accent bar, and bright text.
/// - `active && !focused` keeps a quiet `bg_highlight` fill with dimmed text.
/// - hovered / idle rows match the shared modal treatment.
///
/// `height` lets callers pick their row size: recents/combo rows use
/// [`PALETTE_ROW_H`] (44px), the options-state agent list uses 36px. Corners
/// are always [`PALETTE_ROW_RADIUS`] (6px).
pub(in crate::gui) fn launcher_row<'a, M: Clone + 'a>(
    label: impl Into<Element<'a, M>>,
    active: bool,
    focused: bool,
    msg: M,
    height: f32,
) -> Element<'a, M> {
    use iced::gradient::{self, Gradient};
    use iced::Radians;

    // Resting/idle rows match the shared modal treatment, just taller and
    // rounded to match the palette's row idiom.
    if !(active && focused) {
        return modal_list_row_sized(
            label,
            active,
            msg,
            height,
            PALETTE_ROW_RADIUS,
            PALETTE_ROW_PAD_X,
        );
    }

    // The selected, focused row: cyan gradient fill + ring.
    let btn = button(
        container(label)
            .width(Length::Fill)
            .height(height)
            .align_y(iced::Alignment::Center)
            .padding(Padding::from([0.0, PALETTE_ROW_PAD_X]))
            .clip(true),
    )
    .on_press(msg)
    .width(Length::Fill)
    .padding(0)
    .style(|_, _| {
        // Horizontal cyan tint, brighter at the accent-bar edge.
        let g = gradient::Linear::new(Radians(std::f32::consts::FRAC_PI_2))
            .add_stop(0.0, c::SEL_TINT_STRONG())
            .add_stop(1.0, c::SEL_TINT_SOFT());
        button::Style {
            background: Some(Background::Gradient(Gradient::Linear(g))),
            text_color: c::FG(),
            border: Border {
                color: c::SEL_RING(),
                width: 1.0,
                radius: Radius::from(PALETTE_ROW_RADIUS),
            },
            shadow: Shadow::default(),
            snap: false,
        }
    });

    // Left accent bar, overlaid so it doesn't shift row content.
    let bar = container(
        container(Space::new().width(3))
            .width(3)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::CYAN())),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius::from(2.0),
                },
                ..Default::default()
            }),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(iced::Alignment::Start)
    .align_y(iced::Alignment::Center)
    .padding(Padding::from([6, 0]));

    iced::widget::stack![btn, bar].into()
}

pub(in crate::gui) fn clickable_row<'a, M: Clone + 'a>(
    content: impl Into<Element<'a, M>>,
    height: f32,
    active: bool,
    on_press: M,
) -> Element<'a, M> {
    let bg = if active {
        Some(Background::Color(c::BG_HL()))
    } else {
        None
    };
    button(
        container(content.into())
            .height(height)
            .width(Length::Fill)
            .align_y(iced::Alignment::Center),
    )
    .on_press(on_press)
    .width(Length::Fill)
    .padding(0)
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered && !active {
                Some(Background::Color(c::BG_HOVER()))
            } else {
                bg
            },
            text_color: if active || hovered {
                c::FG()
            } else {
                c::FG_DIM()
            },
            border: Border::default(),
            shadow: Shadow::default(),
            snap: false,
        }
    })
    .into()
}
