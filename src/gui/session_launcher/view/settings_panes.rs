//! The Settings drill-in's states, one function per `SettingsPane`: the
//! grouped root list, the app/project Theme sub-panes, and the three fixed
//! pickers (Backend, Permissions, Default agent).
//!
//! Like `panes.rs`, each takes the modal's partially-built `body` column and
//! returns it with its list zone, divider and footer appended.

use super::super::helpers::{
    project_theme_pane_rows, theme_pane_custom_rows, theme_pane_row_is_custom, theme_pane_rows,
};
use super::super::state::{LauncherSettings, Msg, SettingsPane};
use super::panes::danger_caption;
use crate::gui::icons::icon;
use crate::gui::metrics::UI_FONT;
use crate::gui::palette as c;
use crate::gui::state::ThemeManagerMsg;
use crate::gui::state::{Grove, Msg as GMsg, UpgradeState};
use crate::gui::update::SettingRow;
use crate::gui::view::{
    cap, footer_mod_hint, highlighted_line, launcher_settings_scrollable_id,
    launcher_theme_scrollable_id,
};
use crate::gui::widgets::{
    divider_h, footer_container, footer_hint, ghost_scrollable, launcher_row, modal_list_row_sized,
    section_header, seg_button, slot_badge, SegSide, PALETTE_ROW_H,
};
use iced::border::Radius;
use iced::widget::{button, column, container, row, text, Column, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};

impl Grove {
    /// Dispatch on the drill-in's current pane.
    pub(super) fn settings_body<'a>(
        &'a self,
        body: Column<'a, GMsg>,
        input: &'a str,
        ls: &'a LauncherSettings,
    ) -> Column<'a, GMsg> {
        match &ls.pane {
            SettingsPane::Root => self.settings_root_pane(body, input, ls),
            SettingsPane::Theme {
                kind,
                follow_system,
                ..
            } => self.settings_theme_pane(body, input, ls, *kind, *follow_system),
            SettingsPane::ProjectTheme {
                proj,
                kind,
                preview,
            } => self.settings_project_theme_pane(body, input, ls, *proj, *kind, preview.as_ref()),
            SettingsPane::Backend => self.settings_backend_pane(body, ls),
            SettingsPane::Permissions => self.settings_permissions_pane(body, ls),
            SettingsPane::DefaultAgent => self.settings_default_agent_pane(body, ls),
        }
    }

    /// Settings drill-in root: every `SettingRow`, grouped
    /// under its 4 section headers (C1 in the palette
    /// redesign mock), fuzzy-filtered by `input` — headers
    /// for a section with zero remaining rows are dropped
    /// (C2). While `resizing` (D4), the App-size row's value
    /// slot swaps for the live zoom stepper.
    fn settings_root_pane<'a>(
        &'a self,
        mut body: Column<'a, GMsg>,
        input: &'a str,
        ls: &'a LauncherSettings,
    ) -> Column<'a, GMsg> {
        let rows = self.settings_rows_filtered(input);
        let list_zone: Element<'a, GMsg> = if rows.is_empty() {
            container(text("No matching settings").size(12).color(c::FG_MUTE()))
                .padding(Padding::from([30, 16]))
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .into()
        } else {
            let mut list = Column::new().spacing(2);
            let mut printed_section: Option<&'static str> = None;
            for (i, s) in rows.iter().enumerate() {
                let section = s.section();
                if printed_section != Some(section) {
                    let top = if printed_section.is_none() { 0.0 } else { 12.0 };
                    list = list.push(section_header(section, top, 6.0));
                    printed_section = Some(section);
                }
                let active = i == ls.selected;
                let content: Element<'a, GMsg> = if ls.resizing && *s == SettingRow::AppSize {
                    self.appsize_stepper_row_content()
                } else {
                    self.setting_row_content(*s, input)
                };
                list = list.push(launcher_row(
                    content,
                    active,
                    true,
                    GMsg::SessionLauncher(Msg::SettingActivate(i)),
                    PALETTE_ROW_H,
                ));
                // Danger settings warn inline before you ever
                // change them (B3, same string E1's pane
                // promotes) — only under the selected row, so
                // the list doesn't permanently grow a caption.
                if active && *s == SettingRow::Permissions {
                    list = list.push(danger_caption());
                }
                // Update-available actions expand in place under
                // the CheckUpdates row (E3). Guarded on the live
                // upgrade state, not just the flag: SkipVersion
                // (or a background re-check) can invalidate the
                // strip while it's open.
                if *s == SettingRow::CheckUpdates {
                    if let (Some(strip_sel), UpgradeState::Available(_)) =
                        (ls.update_actions, &self.upgrade)
                    {
                        list = list.push(self.update_actions_strip(strip_sel));
                    }
                }
            }
            container(
                ghost_scrollable(list)
                    .id(launcher_settings_scrollable_id())
                    .height(Length::Shrink),
            )
            .padding(8)
            .max_height(380.0)
            .width(Length::Fill)
            .into()
        };
        body = body.push(list_zone);
        body = body.push(divider_h(c::BORDER_SOFT()));
        let footer_row: Element<'a, GMsg> = if rows.is_empty() {
            // Nothing to choose or change (E4) — only the way
            // back is worth hinting.
            row![footer_hint("esc", "back")]
                .spacing(14)
                .align_y(iced::Alignment::Center)
                .into()
        } else if ls.resizing {
            row![
                footer_hint("←/→", "adjust"),
                footer_hint("0", "reset"),
                footer_hint("⏎", "done"),
                footer_hint("esc", "done"),
            ]
            .spacing(14)
            .align_y(iced::Alignment::Center)
            .into()
        } else if ls.update_actions.is_some() {
            row![
                footer_hint("←→", "choose"),
                footer_hint("⏎", "run"),
                footer_hint("esc", "back"),
            ]
            .spacing(14)
            .align_y(iced::Alignment::Center)
            .into()
        } else {
            row![
                footer_hint("↑↓", "choose"),
                footer_hint("⏎", "change"),
                footer_hint("esc", "back"),
                Space::new().width(Length::Fill),
                text("Changes save automatically.")
                    .size(11)
                    .color(c::FG_MUTE()),
            ]
            .spacing(14)
            .align_y(iced::Alignment::Center)
            .into()
        };
        body.push(footer_container(footer_row))
    }

    /// Theme sub-pane (D1): pinned context row + Dark/Light/
    /// System mode row above a fuzzy-filtered, live-previewing
    /// theme list — see `Grove::theme_pane_select`/
    /// `theme_pane_set_kind`/`theme_pane_set_system`.
    fn settings_theme_pane<'a>(
        &'a self,
        mut body: Column<'a, GMsg>,
        input: &'a str,
        ls: &'a LauncherSettings,
        kind: grove_core::theme::ThemeKind,
        follow_system: bool,
    ) -> Column<'a, GMsg> {
        let context_row = container(
            row![
                text("App theme").size(13).color(c::FG()),
                Space::new().width(Length::Fill),
                text(grove_core::theme::current().name.to_string())
                    .size(12)
                    .color(c::FG_DIM()),
            ]
            .align_y(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .height(PALETTE_ROW_H)
        .padding(Padding::from([0.0, 12.0]))
        .align_y(iced::Alignment::Center)
        .style(|_| container::Style {
            background: Some(Background::Color(c::BG_HL())),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(6.0),
            },
            ..Default::default()
        });

        let mode_seg = container(
            row![
                seg_button(
                    "Dark",
                    !follow_system && kind == grove_core::theme::ThemeKind::Dark,
                    SegSide::Left,
                    GMsg::SessionLauncher(Msg::ThemePaneDark),
                ),
                seg_button(
                    "Light",
                    !follow_system && kind == grove_core::theme::ThemeKind::Light,
                    SegSide::Mid,
                    GMsg::SessionLauncher(Msg::ThemePaneLight),
                ),
                seg_button(
                    "System",
                    follow_system,
                    SegSide::Right,
                    GMsg::SessionLauncher(Msg::ThemePaneSystem),
                ),
            ]
            .spacing(0),
        )
        .style(|_| container::Style {
            border: Border {
                color: c::BORDER(),
                width: 1.0,
                radius: Radius::from(6.0),
            },
            ..Default::default()
        });
        let mode_row = container(
            row![
                text("Mode").size(11).color(c::FG_MUTE()),
                Space::new().width(Length::Fill),
                mode_seg,
            ]
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([8, 12]));

        let list_filter: &str = input;
        let builtin_rows = theme_pane_rows(kind, list_filter);
        let custom_rows = theme_pane_custom_rows(kind, list_filter);
        let n_builtin = builtin_rows.len();
        let current_name = grove_core::theme::current().name.to_string();

        let theme_list: Element<'a, GMsg> = if n_builtin == 0 && custom_rows.is_empty() {
            container(text("No matching themes").size(12).color(c::FG_MUTE()))
                .padding(Padding::from([30, 16]))
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .into()
        } else {
            let mut list = Column::new().spacing(2);
            let theme_row =
                |i: usize, t: &grove_core::theme::Theme, active: bool| -> Element<'a, GMsg> {
                    let m = (!list_filter.is_empty()).then(|| {
                        crate::gui::launcher::fuzzy_match_indices(list_filter, &t.name, "", "")
                    });
                    let ranges: &[(usize, usize)] =
                        m.as_ref().map(|m| m.project.as_slice()).unwrap_or(&[]);
                    let label_el = highlighted_line(&t.name, ranges, c::FG(), UI_FONT, 13.0);
                    let mut content = row![label_el]
                        .spacing(8)
                        .align_y(iced::Alignment::Center)
                        .push(Space::new().width(Length::Fill));
                    if t.name == current_name {
                        content = content.push(icon("check", 12.0, c::CYAN()));
                    }
                    launcher_row(
                        content,
                        active,
                        true,
                        GMsg::SessionLauncher(Msg::ThemePaneSelect(i)),
                        36.0,
                    )
                };
            for (i, t) in builtin_rows.iter().enumerate() {
                list = list.push(theme_row(i, t, i == ls.selected));
            }
            list = list.push(section_header("CUSTOM", 12.0, 6.0));
            if custom_rows.is_empty() {
                list = list.push(
                    container(
                        text("No custom themes yet — create one or paste a palette.")
                            .size(11)
                            .color(c::FG_MUTE()),
                    )
                    .padding(Padding::from([8, 12])),
                );
            } else {
                for (j, t) in custom_rows.iter().enumerate() {
                    let i = n_builtin + j;
                    list = list.push(theme_row(i, t, i == ls.selected));
                }
            }
            // "Manage themes…" (D2+): opens `Modal::ThemeManager`
            // for rename/duplicate/delete/new — the palette's own
            // Theme pane only browses and previews now.
            list = list.push(modal_list_row_sized(
                row![
                    text("Manage themes…").size(12).color(c::FG_DIM()),
                    Space::new().width(Length::Fill),
                    footer_mod_hint("m", "manage"),
                ]
                .align_y(iced::Alignment::Center),
                false,
                GMsg::ThemeManager(ThemeManagerMsg::Open),
                32.0,
                6.0,
                12.0,
            ));
            container(
                ghost_scrollable(list)
                    .id(launcher_theme_scrollable_id())
                    .height(Length::Shrink),
            )
            .max_height(280.0)
            .width(Length::Fill)
            .into()
        };

        body = body.push(
            container(column![context_row, mode_row, theme_list].spacing(0))
                .padding(8)
                .width(Length::Fill),
        );
        body = body.push(divider_h(c::BORDER_SOFT()));
        let selected_is_custom = theme_pane_row_is_custom(kind, list_filter, ls.selected);
        let footer_row: Element<'a, GMsg> = if selected_is_custom {
            row![
                footer_hint("⏎", "apply"),
                footer_mod_hint("e", "edit"),
                footer_mod_hint("m", "manage themes"),
            ]
            .spacing(14)
            .align_y(iced::Alignment::Center)
            .into()
        } else {
            row![
                footer_hint("↑↓", "preview"),
                footer_hint("tab", "mode"),
                footer_hint("⏎", "apply"),
                footer_mod_hint("m", "manage themes"),
                footer_hint("esc", "back"),
            ]
            .spacing(14)
            .align_y(iced::Alignment::Center)
            .into()
        };
        body.push(footer_container(footer_row))
    }

    /// Project theme sub-pane: same shape as the app Theme
    /// pane above, minus the System segment (a project
    /// override is always a concrete pick or "Use app
    /// theme") — see `Grove::theme_pane_select`/
    /// `theme_pane_set_kind` (Project scope arm).
    fn settings_project_theme_pane<'a>(
        &'a self,
        mut body: Column<'a, GMsg>,
        input: &'a str,
        ls: &'a LauncherSettings,
        proj: usize,
        kind: grove_core::theme::ThemeKind,
        preview: Option<&'a grove_core::theme::Theme>,
    ) -> Column<'a, GMsg> {
        let proj_name = self
            .app
            .store
            .projects
            .get(proj)
            .map(|p| p.name.as_str())
            .unwrap_or("(project removed)");
        let context_row = container(
            row![
                text("Project theme").size(13).color(c::FG()),
                Space::new().width(Length::Fill),
                text(proj_name).size(12).color(c::FG_DIM()),
            ]
            .align_y(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .height(PALETTE_ROW_H)
        .padding(Padding::from([0.0, 12.0]))
        .align_y(iced::Alignment::Center)
        .style(|_| container::Style {
            background: Some(Background::Color(c::BG_HL())),
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(6.0),
            },
            ..Default::default()
        });

        let mode_seg = container(
            row![
                seg_button(
                    "Dark",
                    kind == grove_core::theme::ThemeKind::Dark,
                    SegSide::Left,
                    GMsg::SessionLauncher(Msg::ThemePaneDark),
                ),
                seg_button(
                    "Light",
                    kind == grove_core::theme::ThemeKind::Light,
                    SegSide::Right,
                    GMsg::SessionLauncher(Msg::ThemePaneLight),
                ),
            ]
            .spacing(0),
        )
        .style(|_| container::Style {
            border: Border {
                color: c::BORDER(),
                width: 1.0,
                radius: Radius::from(6.0),
            },
            ..Default::default()
        });
        let mode_row = container(
            row![
                text("Mode").size(11).color(c::FG_MUTE()),
                Space::new().width(Length::Fill),
                mode_seg,
            ]
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([8, 12]));

        let rows = project_theme_pane_rows(kind, input);
        let theme_list: Element<'a, GMsg> = if rows.is_empty() {
            container(text("No matching themes").size(12).color(c::FG_MUTE()))
                .padding(Padding::from([30, 16]))
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .into()
        } else {
            let mut list = Column::new().spacing(2);
            for (i, row_theme) in rows.iter().enumerate() {
                let active = i == ls.selected;
                let is_current =
                    row_theme.as_ref().map(|t| t.name.as_ref()) == preview.map(|t| t.name.as_ref());
                let content: Element<'a, GMsg> = match row_theme {
                    Some(t) => {
                        let m = (!input.is_empty()).then(|| {
                            crate::gui::launcher::fuzzy_match_indices(input, &t.name, "", "")
                        });
                        let ranges: &[(usize, usize)] =
                            m.as_ref().map(|m| m.project.as_slice()).unwrap_or(&[]);
                        let label_el = highlighted_line(&t.name, ranges, c::FG(), UI_FONT, 13.0);
                        let mut c = row![label_el]
                            .spacing(8)
                            .align_y(iced::Alignment::Center)
                            .push(Space::new().width(Length::Fill));
                        if is_current {
                            c = c.push(icon("check", 12.0, c::CYAN()));
                        }
                        c.into()
                    }
                    None => {
                        let mut c = row![text("Use app theme").size(13).color(c::FG_MUTE())]
                            .spacing(8)
                            .align_y(iced::Alignment::Center)
                            .push(Space::new().width(Length::Fill));
                        if is_current {
                            c = c.push(icon("check", 12.0, c::CYAN()));
                        }
                        c.into()
                    }
                };
                list = list.push(launcher_row(
                    content,
                    active,
                    true,
                    GMsg::SessionLauncher(Msg::ThemePaneSelect(i)),
                    36.0,
                ));
            }
            container(
                ghost_scrollable(list)
                    .id(launcher_theme_scrollable_id())
                    .height(Length::Shrink),
            )
            .max_height(280.0)
            .width(Length::Fill)
            .into()
        };

        body = body.push(
            container(column![context_row, mode_row, theme_list].spacing(0))
                .padding(8)
                .width(Length::Fill),
        );
        body = body.push(divider_h(c::BORDER_SOFT()));
        let footer_row: Element<'a, GMsg> = if rows.is_empty() {
            row![footer_hint("esc", "back")]
                .spacing(14)
                .align_y(iced::Alignment::Center)
                .into()
        } else {
            row![
                footer_hint("↑↓", "preview"),
                footer_hint("tab", "dark/light"),
                footer_hint("⏎", "apply"),
                footer_hint("esc", "back"),
            ]
            .spacing(14)
            .align_y(iced::Alignment::Center)
            .into()
        };
        body.push(footer_container(footer_row))
    }

    /// Binary enum picker (D2): no filtering, 2 fixed rows.
    fn settings_backend_pane<'a>(
        &'a self,
        mut body: Column<'a, GMsg>,
        ls: &'a LauncherSettings,
    ) -> Column<'a, GMsg> {
        let tmux_on = self.app.use_tmux();
        let current = if tmux_on { 1 } else { 0 };
        let rows: [(&str, &str); 2] = [
            ("Native", "spawn PTYs directly"),
            ("Tmux", "sessions survive restarts"),
        ];
        let mut list = Column::new().spacing(2);
        for (i, (label, desc)) in rows.iter().enumerate() {
            let active = i == ls.selected;
            let mut content = row![
                text(*label).size(13).color(c::FG()),
                text(format!("— {desc}")).size(11).color(c::FG_MUTE()),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center)
            .push(Space::new().width(Length::Fill));
            if i == current {
                content = content.push(icon("check", 12.0, c::CYAN()));
            }
            list = list.push(launcher_row(
                content,
                active,
                true,
                GMsg::SessionLauncher(Msg::SettingsPaneActivate(i)),
                PALETTE_ROW_H,
            ));
        }
        let note = container(
            text("Applies to new sessions; running sessions keep their backend.")
                .size(11)
                .color(c::FG_MUTE()),
        )
        .padding(Padding::from([6, 12]));
        body = body.push(
            container(column![list, note].spacing(0))
                .padding(8)
                .width(Length::Fill),
        );
        body = body.push(divider_h(c::BORDER_SOFT()));
        body.push(footer_container(
            row![
                footer_hint("↑↓", "choose"),
                footer_hint("⏎", "apply"),
                footer_hint("esc", "back"),
            ]
            .spacing(14)
            .align_y(iced::Alignment::Center)
            .into(),
        ))
    }

    /// Skip permissions confirms first (E1): no filtering, 2
    /// fixed rows; the highlighted Skip row promotes to a red
    /// wash + a warning caption instead of the usual cyan
    /// selection tint.
    fn settings_permissions_pane<'a>(
        &'a self,
        mut body: Column<'a, GMsg>,
        ls: &'a LauncherSettings,
    ) -> Column<'a, GMsg> {
        let skip_on = self.app.skip_permissions_enabled();
        let current = if skip_on { 1 } else { 0 };
        let rows: [(&str, &str); 2] = [
            ("Ask", "agents ask before running commands"),
            ("Skip", "run any command without asking"),
        ];
        let mut list = Column::new().spacing(2);
        for (i, (label, desc)) in rows.iter().enumerate() {
            let active = i == ls.selected;
            let is_skip = i == 1;
            let danger = is_skip && active;
            let label_color = if danger { c::RED() } else { c::FG() };
            let mut content = row![
                text(*label).size(13).color(label_color),
                text(format!("— {desc}")).size(11).color(c::FG_MUTE()),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center)
            .push(Space::new().width(Length::Fill));
            if i == current {
                content = content.push(icon("check", 12.0, c::CYAN()));
            }
            let msg = GMsg::SessionLauncher(Msg::SettingsPaneActivate(i));
            let row_el: Element<'a, GMsg> = if danger {
                button(
                    container(content)
                        .width(Length::Fill)
                        .height(PALETTE_ROW_H)
                        .align_y(iced::Alignment::Center)
                        .padding(Padding::from([0.0, 12.0])),
                )
                .on_press(msg)
                .width(Length::Fill)
                .padding(0)
                .style(|_, _| button::Style {
                    background: Some(Background::Color(c::RED_WASH())),
                    text_color: c::FG(),
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: Radius::from(6.0),
                    },
                    shadow: Shadow::default(),
                    snap: false,
                })
                .into()
            } else {
                launcher_row(content, active, true, msg, PALETTE_ROW_H)
            };
            list = list.push(row_el);
        }
        let mut pane_col = column![list].spacing(0);
        if ls.selected == 1 {
            pane_col = pane_col.push(
                container(
                    text("Skip lets agents run any command without asking.")
                        .size(11)
                        .color(c::FG_DIM()),
                )
                .padding(Padding::from([6, 12])),
            );
        }
        body = body.push(container(pane_col).padding(8).width(Length::Fill));
        body = body.push(divider_h(c::BORDER_SOFT()));
        body.push(footer_container(
            row![
                footer_hint("↑↓", "choose"),
                footer_hint("⏎", "confirm"),
                footer_hint("esc", "back"),
            ]
            .spacing(14)
            .align_y(iced::Alignment::Center)
            .into(),
        ))
    }

    /// Default agent picker (D3): mirrors OPEN WITH's list —
    /// uninstalled tools are visible but inert (see
    /// `Grove::default_agent_pane_row_installed`).
    fn settings_default_agent_pane<'a>(
        &'a self,
        mut body: Column<'a, GMsg>,
        ls: &'a LauncherSettings,
    ) -> Column<'a, GMsg> {
        let mut list = Column::new().spacing(2);
        for (i, &agent) in grove_core::agent::Agent::ALL.iter().enumerate() {
            let active = i == ls.selected;
            let installed = self.default_agent_pane_row_installed(agent);
            let is_default = self.app.store.default_agent == Some(agent);
            let label_color = if installed { c::FG() } else { c::FG_MUTE() };
            let icon_color = if active { c::YELLOW() } else { c::FG_MUTE() };
            let icon_slot = container(icon(agent.icon_name(), 16.0, icon_color))
                .width(24.0)
                .align_x(iced::alignment::Horizontal::Center);
            let status_text =
                if agent == grove_core::agent::Agent::Terminal || self.settings_tools.is_empty() {
                    None
                } else {
                    self.settings_tools
                        .iter()
                        .find(|t| t.agent == agent)
                        .map(|st| {
                            if st.detecting {
                                ("Detecting…".to_string(), c::FG_MUTE())
                            } else if !st.installed {
                                ("Not installed".to_string(), c::FG_MUTE())
                            } else {
                                (
                                    st.version
                                        .clone()
                                        .unwrap_or_else(|| "installed".to_string()),
                                    c::FG_DIM(),
                                )
                            }
                        })
                };
            let mut content = row![
                icon_slot,
                text(cap(agent.label())).size(13).color(label_color),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .push(Space::new().width(Length::Fill));
            if let Some((text_s, color)) = status_text {
                content = content.push(text(text_s).size(12).color(color));
                content = content.push(Space::new().width(10));
            }
            if is_default {
                content = content.push(slot_badge("Default"));
                content = content.push(Space::new().width(6));
                content = content.push(icon("check", 12.0, c::CYAN()));
            }
            list = list.push(launcher_row(
                content,
                active,
                true,
                GMsg::SessionLauncher(Msg::SettingsPaneActivate(i)),
                36.0,
            ));
        }
        body = body.push(container(list).padding(8).width(Length::Fill));
        body = body.push(divider_h(c::BORDER_SOFT()));
        body.push(footer_container(
            row![
                footer_hint("↑↓", "choose"),
                footer_hint("⏎", "set default"),
                footer_hint("esc", "back"),
            ]
            .spacing(14)
            .align_y(iced::Alignment::Center)
            .into(),
        ))
    }
}
