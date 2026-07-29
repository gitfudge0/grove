//! Rendering for the Settings drill-in's leaf panes: a single settings row's
//! content (`setting_row_content`), the App-size row's live zoom stepper
//! (`appsize_stepper_row_content`), the Check-for-updates row's expanded
//! actions strip (`update_actions_strip`), and the drill-in's cue-chip/
//! placeholder text per pane (`settings_pane_cue`/`settings_pane_placeholder`).

use super::super::helpers::{update_available_actions, UpdateAction};
use super::super::state::{Msg, SettingsPane};
use crate::gui::icons::icon;
use crate::gui::metrics::UI_FONT;
use crate::gui::palette as c;
use crate::gui::state::{Grove, Msg as GMsg, UpgradeState};
use crate::gui::update::SettingRow;
use crate::gui::view::highlighted_line;
use crate::gui::widgets::{control_btn_sized, control_icon_btn};
use iced::border::Radius;
use iced::widget::{button, container, row, text, Space};
use iced::{Background, Border, Element, Length, Padding, Shadow};

impl Grove {
    /// Row content shared by a root-mode `PaletteRow::Setting` match and a
    /// Settings-drill-in row: icon slot + label (cyan fuzzy-highlighted
    /// against `input`) + right-aligned live value, plus a trailing chevron
    /// on the rows that drill into a deeper level (the two toggles flip in
    /// place, so they go without).
    pub(super) fn setting_row_content<'a>(
        &'a self,
        s: SettingRow,
        input: &str,
    ) -> Element<'a, GMsg> {
        let label = s.label();
        let value = self.setting_value(s);
        let m = (!input.is_empty())
            .then(|| crate::gui::launcher::fuzzy_match_indices(input, label, &value, s.section()));
        let label_ranges: &[(usize, usize)] = m.as_ref().map_or(&[][..], |m| m.project.as_slice());
        let label_el = highlighted_line(label, label_ranges, c::FG(), UI_FONT, 13.0);

        let icon_slot: Element<'a, GMsg> = if s.is_toggle() {
            // A non-interactive checkbox glyph modeled on
            // `modal_checkbox`'s checked-state border/background colors
            // (`settings_modal`'s own toggle rows) — the whole row is
            // already the click target here, so this isn't the real
            // `checkbox` widget, just its visual idiom.
            let checked = value == "On";
            let box_el = container(if checked {
                icon("check", 10.0, c::MAGENTA())
            } else {
                Space::new().width(10.0).height(10.0).into()
            })
            .width(14.0)
            .height(14.0)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::Alignment::Center)
            .style(move |_| container::Style {
                background: Some(Background::Color(if checked {
                    c::BG_HL()
                } else {
                    c::BG()
                })),
                border: Border {
                    color: if checked { c::MAGENTA() } else { c::BORDER() },
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                ..Default::default()
            });
            container(box_el)
                .width(24.0)
                .align_x(iced::alignment::Horizontal::Center)
                .into()
        } else {
            container(icon(s.icon_name(), 16.0, c::FG_MUTE()))
                .width(24.0)
                .align_x(iced::alignment::Horizontal::Center)
                .into()
        };

        // Async status renders inline in the value slot (E2): CheckUpdates
        // mirrors `settings_modal`'s status line — a spinner while checking,
        // green once a release is known to be available. Every other state
        // (and every other setting) keeps the plain FG_DIM value.
        let value_el: Element<'a, GMsg> = if s == SettingRow::CheckUpdates {
            match &self.upgrade {
                UpgradeState::Checking => row![
                    crate::gui::icons::spinner(11.0, c::FG_MUTE(), self.anim.blink_tick),
                    Space::new().width(6),
                    text(value).size(12).color(c::FG_MUTE()),
                ]
                .align_y(iced::Alignment::Center)
                .into(),
                UpgradeState::Available(_) => text(value).size(12).color(c::GREEN()).into(),
                _ => text(value).size(12).color(c::FG_DIM()).into(),
            }
        } else {
            text(value).size(12).color(c::FG_DIM()).into()
        };

        let mut content = row![
            icon_slot,
            label_el,
            Space::new().width(Length::Fill),
            value_el,
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        // The chevron promises a deeper level — toggles flip in place and
        // never open one, so they don't get it.
        if !s.is_toggle() {
            content =
                content
                    .push(Space::new().width(8))
                    .push(icon("chev-right", 12.0, c::FG_MUTE()));
        }
        content.into()
    }

    /// The App-size row's value slot while `LauncherSettings::resizing` is
    /// set (D4): the same live zoom stepper trio `settings_modal` uses
    /// (view.rs's own `app_size_row`), instead of `setting_row_content`'s
    /// usual right-aligned value + chevron.
    pub(super) fn appsize_stepper_row_content(&self) -> Element<'_, GMsg> {
        let icon_slot = container(icon(SettingRow::AppSize.icon_name(), 16.0, c::FG_MUTE()))
            .width(24.0)
            .align_x(iced::alignment::Horizontal::Center);
        let stepper = container(
            row![
                control_icon_btn("minus", GMsg::ZoomOut, 20.0, 13.0),
                control_btn_sized(
                    format!("{:.0}%", self.pty_layout.zoom * 100.0),
                    GMsg::ZoomReset,
                    12,
                    2
                ),
                control_icon_btn("plus", GMsg::ZoomIn, 20.0, 13.0),
            ]
            .spacing(0)
            .align_y(iced::Alignment::Center),
        )
        .style(|_| container::Style {
            border: Border {
                color: c::BORDER(),
                width: 1.0,
                radius: Radius::from(6.0),
            },
            ..Default::default()
        });
        row![
            icon_slot,
            text(SettingRow::AppSize.label()).size(13).color(c::FG()),
            Space::new().width(Length::Fill),
            stepper,
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into()
    }

    /// The update-available actions strip expanded under the drill-in's
    /// Check-for-updates row (E3): the same pill-button treatment
    /// `settings_modal`'s update-actions row uses (`modal_action_sized`'s
    /// visual language), laid out horizontally, plus a cyan selection ring
    /// on the keyboard-selected action (←→/Tab move it — the palette rows'
    /// ↑↓ stay reserved for the list cursor). The action list comes from
    /// `update_available_actions` so the render and the keyboard nav can
    /// never disagree about what index N runs.
    pub(super) fn update_actions_strip(&self, sel: usize) -> Element<'_, GMsg> {
        let method_unknown = matches!(
            self.upgrade_method,
            grove_core::upgrade::InstallMethod::Unknown
        );
        let mut strip = row![].spacing(8).align_y(iced::Alignment::Center);
        for (i, action) in update_available_actions(method_unknown)
            .into_iter()
            .enumerate()
        {
            let active = i == sel;
            let primary = matches!(action, UpdateAction::UpdateNow);
            strip = strip.push(
                button(text(action.label()).size(11))
                    .on_press(GMsg::SessionLauncher(Msg::UpdateActionPick(i)))
                    .padding(Padding::from([5, 12]))
                    .style(move |_, status| {
                        let hovered = matches!(status, button::Status::Hovered);
                        let bg = if active {
                            c::SEL_TINT_SOFT()
                        } else if hovered {
                            c::BG_HOVER()
                        } else if primary {
                            c::BG_HL()
                        } else {
                            c::BG()
                        };
                        button::Style {
                            background: Some(Background::Color(bg)),
                            text_color: if active || primary {
                                c::FG()
                            } else {
                                c::FG_DIM()
                            },
                            border: Border {
                                color: if active { c::SEL_RING() } else { c::BORDER() },
                                width: 1.0,
                                radius: Radius::from(4.0),
                            },
                            shadow: Shadow::default(),
                            snap: false,
                        }
                    }),
            );
        }
        container(strip)
            .padding(Padding {
                top: 4.0,
                bottom: 4.0,
                left: 12.0,
                right: 12.0,
            })
            .width(Length::Fill)
            .into()
    }
}

/// The Settings drill-in's leading cue-chip label for the current pane —
/// shared by the input zone's chip and (indirectly, via the same match
/// shape) nothing else, kept as a free function since it needs no `self`.
pub(super) fn settings_pane_cue(pane: &SettingsPane) -> &'static str {
    match pane {
        SettingsPane::Root => "SETTINGS",
        SettingsPane::Theme { .. } => "THEME",
        SettingsPane::Backend => "BACKEND",
        SettingsPane::Permissions => "PERMISSIONS",
        SettingsPane::DefaultAgent => "DEFAULT AGENT",
        SettingsPane::ProjectTheme { .. } => "PROJECT THEME",
    }
}

/// The Settings drill-in's search-field placeholder for the current pane.
/// Root and Theme actually filter on it; Backend/Permissions/DefaultAgent
/// show it but ignore what's typed (see `handle_modal_key`'s settings
/// branch) — their lists are short and fixed, nothing to filter.
pub(super) fn settings_pane_placeholder(pane: &SettingsPane) -> &'static str {
    match pane {
        SettingsPane::Root => "Search settings…",
        SettingsPane::Theme { .. } => "Search themes…",
        SettingsPane::Backend | SettingsPane::Permissions | SettingsPane::DefaultAgent => "Search…",
        SettingsPane::ProjectTheme { .. } => "Search themes…",
    }
}
