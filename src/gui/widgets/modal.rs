//! Modal chrome: panel, header, footer, keycaps, and modal controls.

use super::primitives::tracked;
use crate::gui::metrics::{MONO_FONT, UI_FONT};
use crate::gui::palette as c;
use crate::gui::state::Msg;
use iced::border::Radius;
use iced::widget::{button, container, row, text, text_input};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};
/// Shared modal panel chrome — the same background/border/shadow language as
/// the command palette. `content` carries its own zone padding (header /
/// divider / body / divider / footer), so the panel itself is unpadded.
pub(in crate::gui) fn modal_panel<'a, M: 'a>(
    content: Element<'a, M>,
    width: f32,
) -> Element<'a, M> {
    container(content)
        .width(width)
        // 1px inset so children (notably the filled footer strip) sit inside
        // the border stroke instead of painting over it.
        .padding(1.0)
        .style(move |_| container::Style {
            background: Some(Background::Color(c::BG_RAIL())),
            text_color: Some(c::FG()),
            border: Border {
                color: c::BORDER(),
                width: 1.0,
                radius: Radius::from(12.0),
            },
            shadow: Shadow {
                color: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
                offset: iced::Vector::new(0.0, 12.0),
                blur_radius: 40.0,
            },
            ..Default::default()
        })
        .into()
}

/// Keycap chip shell shared by every modal footer hint, the palette's ⌘T/digit
/// chips, and the ⏎ chips on active rows: mono, 2px/6px padding, radius 4,
/// filled `BG_HL` background. `inner` carries its own text color so callers
/// can pick the muted "quiet digit" shade vs. the regular hint shade.
pub(in crate::gui) fn keycap<'a, M: 'a>(inner: Element<'a, M>) -> Element<'a, M> {
    container(inner)
        .padding(Padding::from([2, 6]))
        .style(|_| container::Style {
            background: Some(Background::Color(c::BG_HL())),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(4.0),
            },
            ..Default::default()
        })
        .into()
}

/// A plain-label keycap (e.g. "⏎", "↑↓", "esc", "←→") in the given text color.
pub(in crate::gui) fn keycap_text<'a, M: 'a>(
    label: impl Into<String>,
    color: Color,
) -> Element<'a, M> {
    keycap(
        text(label.into())
            .font(MONO_FONT)
            .size(11)
            .color(color)
            .into(),
    )
}
/// A mono, uppercase, letter-tracked section label ("RECENT", "ACTIONS",
/// "OPEN WITH") used by both the command palette and modal lists. Iced has no
/// letter-spacing property, so tracking is faked by joining every character
/// with a U+2009 thin space (confirmed present in the bundled BlexMono Nerd
/// Font's `cmap`). `top`/`bottom` are the caller's margin above/below.
pub(in crate::gui) fn section_header<'a>(label: &str, top: f32, bottom: f32) -> Element<'a, Msg> {
    container(
        text(tracked(label))
            .font(MONO_FONT)
            .size(10)
            .color(c::FG_MUTE()),
    )
    .padding(Padding {
        top,
        bottom,
        left: 12.0,
        right: 0.0,
    })
    .into()
}

/// One `keycap` + muted label pair in a modal's footer hint strip (e.g.
/// "[↑↓] navigate").
pub(in crate::gui) fn footer_hint<'a>(key: &'static str, label: &'static str) -> Element<'a, Msg> {
    row![
        keycap_text(key, c::FG_DIM()),
        text(label).font(MONO_FONT).size(10).color(c::FG_MUTE()),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center)
    .into()
}

/// The palette's full-bleed footer strip: `BG_STRIP` fill, `[8, 16]` padding,
/// bottom corners rounded to stay flush with the panel's own 12px radius
/// (containers don't clip children, so the footer must carry its own bottom
/// radius rather than relying on the panel's).
pub(in crate::gui) fn footer_container<'a, M: 'a>(content: Element<'a, M>) -> Element<'a, M> {
    container(content)
        .padding(Padding::from([8, 16]))
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(c::BG_STRIP())),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                // 11 = the panel's 12px radius minus its 1px content inset,
                // so the strip hugs the inner corner without covering the
                // panel border.
                radius: Radius {
                    top_left: 0.0,
                    top_right: 0.0,
                    bottom_left: 11.0,
                    bottom_right: 11.0,
                },
            },
            ..Default::default()
        })
        .into()
}

/// Convenience wrapper around [`footer_container`] for the common case of a
/// row of `footer_hint`s with the palette's 14px inter-hint spacing.
pub(in crate::gui) fn modal_footer_hints<'a>(
    hints: &[(&'static str, &'static str)],
) -> Element<'a, Msg> {
    let mut r = row![].spacing(14);
    for (key, label) in hints {
        r = r.push(footer_hint(key, label));
    }
    footer_container(r.into())
}

/// Like [`footer_container`], but for callers that need a fully custom row
/// (e.g. version/update status mixed with a trailing footer keycap hint)
/// instead of a plain list of `footer_hint`s.
pub(in crate::gui) fn modal_footer_row<'a, M: 'a>(content: Element<'a, M>) -> Element<'a, M> {
    footer_container(content)
}

/// A modal header zone: `[14, 16]` padding holding a size-13 title in
/// `accent`. Callers with extra header content (step counter, close button)
/// should build the row themselves and wrap it with [`modal_header_row`]
/// instead, to keep the same padding.
pub(in crate::gui) fn modal_header<'a>(title: &str, accent: Color) -> Element<'a, Msg> {
    modal_header_row(text(title.to_string()).size(13).color(accent).into())
}

/// Like [`modal_header`], but for callers that need more than a bare title in
/// the header zone (e.g. a title plus a right-aligned step counter).
pub(in crate::gui) fn modal_header_row<'a, M: 'a>(row_content: Element<'a, M>) -> Element<'a, M> {
    container(row_content)
        .padding(Padding::from([14, 16]))
        .width(Length::Fill)
        .into()
}

/// Borderless, transparent-background `text_input` style for a modal's hero
/// input zone (the zone's own container carries no visible field chrome —
/// just a leading icon and the typed text). Shared by the session launcher
/// and the worktree-name input modal.
pub(in crate::gui) fn palette_input_style(
    _t: &iced::Theme,
    _status: text_input::Status,
) -> text_input::Style {
    text_input::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: Radius::from(0.0),
        },
        icon: c::FG_MUTE(),
        placeholder: c::FG_MUTE(),
        value: c::FG(),
        selection: c::CYAN(),
    }
}

/// Visual weight of a modal footer button.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::gui) enum ModalBtn {
    /// Dismiss / secondary action.
    Plain,
    /// Default affirmative action.
    Primary,
    /// Default affirmative action with destructive consequences.
    Danger,
}

pub(in crate::gui) fn modal_action<'a, M: Clone + 'a>(
    label: &'static str,
    kind: ModalBtn,
    msg: M,
) -> Element<'a, M> {
    modal_action_sized(label, kind, 12, msg)
}

/// [`modal_action`] with an explicit text size, for spots (e.g. a demoted
/// footer strip) where the default 12px button reads too loud.
pub(in crate::gui) fn modal_action_sized<'a, M: Clone + 'a>(
    label: &'static str,
    kind: ModalBtn,
    size: u16,
    msg: M,
) -> Element<'a, M> {
    button(text(label).size(iced::Pixels::from(size as f32)))
        .on_press(msg)
        .padding(Padding::from([6, 12]))
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            let filled = !matches!(kind, ModalBtn::Plain);
            let bg = if hovered {
                c::BG_HOVER()
            } else if filled {
                c::BG_HL()
            } else {
                c::BG()
            };
            button::Style {
                background: Some(Background::Color(bg)),
                text_color: match kind {
                    ModalBtn::Plain => c::FG_DIM(),
                    ModalBtn::Primary => c::FG(),
                    ModalBtn::Danger => c::RED(),
                },
                border: Border {
                    color: if matches!(kind, ModalBtn::Danger) {
                        c::RED()
                    } else {
                        c::BORDER()
                    },
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                shadow: Shadow::default(),
                snap: false,
            }
        })
        .into()
}

/// Modal checkbox in the shared themed style; `accent` colors the tick and
/// the checked border. `on_toggle: None` renders it disabled.
pub(in crate::gui) fn modal_checkbox<'a, M: Clone + 'a>(
    label: String,
    checked: bool,
    accent: Color,
    on_toggle: Option<fn(bool) -> M>,
) -> Element<'a, M> {
    use iced::widget::checkbox;
    use iced::widget::checkbox::{Status as CheckboxStatus, Style as CheckboxStyle};
    checkbox(checked)
        .label(label)
        .on_toggle_maybe(on_toggle)
        .size(14)
        .spacing(8)
        .text_size(12)
        .font(UI_FONT)
        .style(move |_, status| {
            let (checked, disabled, hovered) = match status {
                CheckboxStatus::Active { is_checked } => (is_checked, false, false),
                CheckboxStatus::Hovered { is_checked } => (is_checked, false, true),
                CheckboxStatus::Disabled { is_checked } => (is_checked, true, false),
            };
            let border_color = if checked {
                accent
            } else if hovered {
                c::FG_DIM()
            } else {
                c::BORDER()
            };
            CheckboxStyle {
                background: Background::Color(if checked {
                    c::BG_HL()
                } else if hovered {
                    c::BG_HOVER()
                } else {
                    c::BG()
                }),
                icon_color: if disabled { c::FG_MUTE() } else { accent },
                border: Border {
                    color: border_color,
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                text_color: Some(if disabled { c::FG_MUTE() } else { c::FG_DIM() }),
            }
        })
        .into()
}
