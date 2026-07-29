//! Settings + tools: the project-settings delegate, tmux-choice and
//! agent-picker prompts, the main Settings modal, and the keyboard-shortcut
//! overlay.

use super::super::common::cap;
use crate::gui::icons::icon;
use crate::gui::metrics::{MONO_FONT, ROW_H};
use crate::gui::palette as c;
use crate::gui::session_launcher;
use crate::gui::state::{Grove, Msg, UpgradeState};
use crate::gui::state::{ThemePickerMsg, UpgradeMsg};
use crate::gui::update::{platform_mod_label, Scope, ShortcutDef, SHORTCUTS};
use crate::gui::widgets::{
    control_btn_sized, control_icon_btn, divider_h, dot, footer_hint, ghost_scrollable, icon_btn,
    keycap, keycap_text, launcher_row, modal_action, modal_action_sized, modal_checkbox,
    modal_footer_hints, modal_footer_row, modal_header, modal_header_row, modal_list_row,
    modal_panel, section_header, seg_button, skip_perms_seg, ModalBtn, SegSide,
};
use iced::border::Radius;
use iced::widget::{button, column, container, row, text, Column, Row, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};

impl Grove {
    /// Delegates to the extracted scripts-editor module's view, mapping its
    /// child `Msg` into the parent's.
    pub(super) fn project_settings_modal(&self) -> Element<'_, Msg> {
        crate::gui::scripts_editor::view(self)
    }

    pub(super) fn tmux_choice_modal(&self) -> Element<'_, Msg> {
        let body_zone = column![
            text("Use tmux for new sessions? Existing sessions keep their current backend.")
                .size(13)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::Word),
            Space::new().height(8),
            row![
                Space::new().width(Length::Fill),
                modal_action("Native", ModalBtn::Plain, Msg::ChooseTmux(false)),
                modal_action("Tmux", ModalBtn::Primary, Msg::ChooseTmux(true)),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        let body = column![
            modal_header("Session backend", c::CYAN()),
            divider_h(c::BORDER_SOFT()),
            container(body_zone).padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            modal_footer_hints(&[("⏎", "tmux"), ("n", "native"), ("esc", "close")]),
        ];

        modal_panel(body.into(), 480.0)
    }

    pub(super) fn agent_picker_modal<'a>(
        &'a self,
        project: &'a str,
        wt_path: &'a str,
        sel: usize,
    ) -> Element<'a, Msg> {
        const AGENT_ROW_H: f32 = 32.0;

        let wt_name = crate::app::path_basename(wt_path);
        let title = if project.is_empty() {
            format!("Start session / {wt_name}")
        } else {
            format!("Start session / {project} / {wt_name}")
        };

        let mut list = Column::new().spacing(2);
        for (i, agent) in self.app.available_agents.iter().enumerate() {
            let active = i == sel;
            let is_default = self.app.store.default_agent == Some(*agent);
            let icon_color = if active { c::YELLOW() } else { c::FG_MUTE() };
            let icon_slot = container(icon(agent.icon_name(), 16.0, icon_color))
                .width(24.0)
                .align_x(iced::alignment::Horizontal::Center);
            let label = row![
                icon_slot,
                text(cap(agent.label()))
                    .size(12)
                    .color(if active { c::FG() } else { c::FG_DIM() }),
                Space::new().width(Length::Fill),
                text(if is_default { "Default" } else { "" })
                    .size(11)
                    .color(c::FG_MUTE()),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center);

            list = list.push(launcher_row(
                label,
                active,
                true,
                Msg::AgentPickerSelect(i),
                AGENT_ROW_H,
            ));
        }

        let list_zone = container(list).padding(8).width(Length::Fill);

        let body_zone = column![
            list_zone,
            Space::new().height(8),
            row![
                modal_action("Default", ModalBtn::Plain, Msg::AgentPickerToggleDefault),
                Space::new().width(Length::Fill),
                modal_action("Cancel", ModalBtn::Plain, Msg::ModalCancel),
                modal_action("Launch", ModalBtn::Primary, Msg::AgentPickerSubmit),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        let body = column![
            modal_header(&title, c::MAGENTA()),
            divider_h(c::BORDER_SOFT()),
            container(body_zone).padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            modal_footer_hints(&[("↑↓", "choose"), ("⏎", "launch"), ("esc", "cancel")]),
        ];

        modal_panel(body.into(), 500.0)
    }

    pub(super) fn settings_modal(&self) -> Element<'_, Msg> {
        // The "Default" badge and "Set default" button share an identical
        // footprint (fixed width, same padding/radius) so the action-cell
        // column stays aligned regardless of which state a row is in.
        const SLOT_W: f32 = 84.0;

        use iced::Alignment::Center;

        // A muted, indented one-liner used under section headers and rows to
        // explain what a control does (throwaway caption: 11 · regular ·
        // fg-mute).
        let caption = |s: &'static str| -> Element<'_, Msg> {
            container(text(s).size(11).color(c::FG_MUTE()))
                .padding(Padding::from([0, 10]))
                .into()
        };
        // One shade up from a throwaway caption — reserved for the single
        // safety-relevant caption (skip-permissions).
        let caption_promoted = |s: &'static str| -> Element<'_, Msg> {
            container(text(s).size(11).color(c::FG_DIM()))
                .padding(Padding::from([0, 10]))
                .into()
        };
        let slot_badge = |label: &'static str| -> Element<'_, Msg> {
            container(
                text(label)
                    .size(11)
                    .color(c::FG())
                    .align_x(iced::alignment::Horizontal::Center)
                    .width(Length::Fill),
            )
            .width(Length::Fixed(SLOT_W))
            .padding(Padding::from([4, 12]))
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_HL())),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                ..Default::default()
            })
            .into()
        };
        let slot_action = |label: &'static str, msg: Msg| -> Element<'_, Msg> {
            button(
                text(label)
                    .size(11)
                    .align_x(iced::alignment::Horizontal::Center)
                    .width(Length::Fill),
            )
            .on_press(msg)
            .width(Length::Fixed(SLOT_W))
            .padding(Padding::from([4, 12]))
            .style(|_, status| {
                let hovered = matches!(status, button::Status::Hovered);
                button::Style {
                    background: if hovered {
                        Some(Background::Color(c::BG_HOVER()))
                    } else {
                        Some(Background::Color(c::BG()))
                    },
                    text_color: c::FG_DIM(),
                    border: Border {
                        color: c::BORDER(),
                        width: 1.0,
                        radius: Radius::from(4.0),
                    },
                    shadow: Shadow::default(),
                    snap: false,
                }
            })
            .into()
        };
        // Missing tools reserve the same fixed-width, same-padding footprint
        // as a real slot but render nothing, so the column of badges/buttons
        // above and below it doesn't shift.
        let slot_none = || -> Element<'_, Msg> {
            container(Space::new().width(Length::Fill))
                .width(Length::Fixed(SLOT_W))
                .padding(Padding::from([4, 12]))
                .into()
        };

        // ── header ─────────────────────────────────────────────────────────
        let header = modal_header_row(
            row![
                text("Settings").size(13).color(c::MAGENTA()),
                Space::new().width(Length::Fill),
                text("Changes save automatically.")
                    .size(11)
                    .color(c::FG_MUTE()),
                Space::new().width(10),
                icon_btn("close", Msg::ModalCancel),
            ]
            .align_y(Center)
            .into(),
        );

        // ── appearance ───────────────────────────────────────────────────────
        let theme_row = modal_list_row(
            row![
                text("App theme").size(12).color(c::FG()),
                Space::new().width(Length::Fill),
                text(grove_core::theme::current().name.to_string())
                    .size(12)
                    .color(c::FG_DIM()),
                Space::new().width(8),
                icon("chev-right", 12.0, c::FG_MUTE()),
            ]
            .align_y(Center),
            false,
            Msg::ThemePicker(ThemePickerMsg::Open),
        );

        let zoom = container(
            row![
                control_icon_btn("minus", Msg::ZoomOut, 20.0, 13.0),
                control_btn_sized(
                    format!("{:.0}%", self.pty_layout.zoom * 100.0),
                    Msg::ZoomReset,
                    12,
                    2
                ),
                control_icon_btn("plus", Msg::ZoomIn, 20.0, 13.0),
            ]
            .spacing(0)
            .align_y(Center),
        )
        .style(|_| container::Style {
            border: Border {
                color: c::BORDER(),
                width: 1.0,
                radius: Radius::from(6.0),
            },
            ..Default::default()
        });
        let app_size_row = container(
            row![
                text("App size").size(12).color(c::FG()),
                Space::new().width(Length::Fill),
                zoom,
            ]
            .align_y(Center),
        )
        .height(ROW_H)
        .padding(Padding::from([0, 10]));

        let project_themes_row = container(modal_checkbox(
            "Project themes".into(),
            self.app.project_themes_enabled(),
            c::MAGENTA(),
            Some(Msg::ProjectThemesToggle),
        ))
        .height(ROW_H)
        .align_y(Center)
        .padding(Padding::from([0, 10]));

        let appearance = column![
            section_header("APPEARANCE", 0.0, 0.0),
            Space::new().height(2),
            theme_row,
            app_size_row,
            project_themes_row,
            caption("Let each project pin its PTYs to a specific theme"),
        ]
        .spacing(4);

        // ── agents / terminal ────────────────────────────────────────────
        let tmux_on = self.app.use_tmux();
        let backend_seg = container(
            row![
                seg_button("Native", !tmux_on, SegSide::Left, Msg::BackendNative),
                seg_button("Tmux", tmux_on, SegSide::Right, Msg::BackendTmux),
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
        let backend_row = container(
            row![
                text("Backend").size(12).color(c::FG()),
                Space::new().width(Length::Fill),
                backend_seg,
            ]
            .align_y(Center),
        )
        .height(ROW_H)
        .padding(Padding::from([0, 10]));

        let skip_perms_on = self.app.skip_permissions_enabled();
        let skip_perms_row = container(
            row![
                text("Permissions").size(12).color(c::FG()),
                Space::new().width(Length::Fill),
                skip_perms_seg(
                    skip_perms_on,
                    Msg::SkipPermissionsEnable,
                    Msg::SkipPermissionsDisable
                ),
            ]
            .align_y(Center),
        )
        .height(ROW_H)
        .padding(Padding::from([0, 10]));

        let chrome_row = container(modal_checkbox(
            "Claude in Chrome".into(),
            self.app.chrome_enabled(),
            c::CYAN(),
            Some(Msg::ChromeToggle),
        ))
        .height(ROW_H)
        .align_y(Center)
        .padding(Padding::from([0, 10]));

        let telemetry_row = container(modal_checkbox(
            "Share anonymous usage data".into(),
            self.app.telemetry_enabled(),
            c::MAGENTA(),
            Some(Msg::TelemetryToggle),
        ))
        .height(ROW_H)
        .align_y(Center)
        .padding(Padding::from([0, 10]));

        let agents_terminal = column![
            section_header("AGENTS / TERMINAL", 0.0, 0.0),
            Space::new().height(2),
            backend_row,
            skip_perms_row,
            caption_promoted("Skip lets agents run any command without asking."),
            chrome_row,
            caption_promoted("Lets Claude read and control your Chrome tabs."),
            telemetry_row,
        ]
        .spacing(4);

        // ── tools ─────────────────────────────────────────────────────────
        let tools_header = container(
            row![
                section_header("TOOLS", 0.0, 0.0),
                Space::new().width(Length::Fill),
                icon_btn("restart", Msg::RefreshTools),
            ]
            .align_y(Center),
        )
        .padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: 0.0,
            right: 10.0,
        });

        let mut tools = Column::new().spacing(0);
        for st in &self.settings_tools {
            // Install state is carried by shape as well as color so it survives
            // grayscale: a filled ● (green) for installed, a hollow ○ (muted)
            // for missing — both at the app's 7px status-dot diameter.
            let status_dot: Element<'_, Msg> = if st.installed {
                dot(c::GREEN())
            } else {
                container(Space::new().width(7))
                    .width(7)
                    .height(7)
                    .style(|_| container::Style {
                        border: Border {
                            color: c::FG_MUTE(),
                            width: 1.0,
                            radius: Radius::from(3.5),
                        },
                        ..Default::default()
                    })
                    .into()
            };
            // Missing tools recede: dim the label and mute the status. Present
            // tools keep full-strength labels; version numbers read as data —
            // status text stays FG_MUTE (not FG_DIM, which is reserved for
            // live values like version strings).
            let (status, status_color) = if st.detecting {
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
            };
            let label_color = if st.installed { c::FG() } else { c::FG_DIM() };
            let agent_label = cap(st.agent.label());
            let is_default = self.app.store.default_agent == Some(st.agent);
            let selector: Element<'_, Msg> = if is_default {
                // The chosen default reads as a selected control (filled
                // highlight), not a category tag — magenta stays reserved for
                // the modal's identity accent.
                slot_badge("Default")
            } else if st.installed {
                slot_action("Set default", Msg::SetDefaultAgent(st.agent))
            } else {
                slot_none()
            };
            let row = container(
                row![
                    status_dot,
                    Space::new().width(8),
                    icon(st.agent.icon_name(), 14.0, label_color),
                    Space::new().width(8),
                    text(agent_label).size(12).color(label_color),
                    Space::new().width(Length::Fill),
                    text(status).size(12).color(status_color),
                    Space::new().width(16),
                    selector,
                ]
                .align_y(Center),
            )
            .height(ROW_H)
            .padding(Padding::from([0, 10]));
            tools = tools.push(row);
        }

        let tools_section = column![tools_header, Space::new().height(2), tools].spacing(4);

        // ── body (scrolls once content exceeds the cap) ─────────────────────
        let sections = column![
            appearance,
            divider_h(c::BORDER_SOFT()),
            agents_terminal,
            divider_h(c::BORDER_SOFT()),
            tools_section,
        ]
        .spacing(8);

        let scroll_cap = (self.pty_layout.window_size.height - 220.0).max(160.0);
        let scroll_body = container(ghost_scrollable(sections)).max_height(scroll_cap);

        // ── updates — the version/status strip merges into the shared
        // footer chrome below; update-available actions and the release
        // notes preview stay in the body, right under the scroll area. ────
        let current_ver = env!("CARGO_PKG_VERSION");
        let status_line: Element<'_, Msg> = match &self.upgrade {
            UpgradeState::Idle => text("Not checked yet").size(11).color(c::FG_MUTE()).into(),
            UpgradeState::Checking => row![
                crate::gui::icons::spinner(11.0, c::FG_MUTE(), self.anim.blink_tick),
                Space::new().width(6),
                text("Checking…").size(11).color(c::FG_MUTE()),
            ]
            .align_y(Center)
            .into(),
            UpgradeState::UpToDate => text("Up to date").size(11).color(c::FG_DIM()).into(),
            UpgradeState::Error(e) => text(format!("Check failed: {e}"))
                .size(11)
                .color(c::FG_MUTE())
                .into(),
            UpgradeState::Available(r) => text(format!("Update available: {}", r.tag))
                .size(11)
                .color(c::GREEN())
                .into(),
            // Updating/Updated/UpdateFailed are shown in the progress modal.
            _ => text("Updating…").size(11).color(c::FG_DIM()).into(),
        };
        let refresh: Element<'_, Msg> = if matches!(self.upgrade, UpgradeState::Checking) {
            container(crate::gui::icons::spinner(
                12.0,
                c::FG_MUTE(),
                self.anim.blink_tick,
            ))
            .into()
        } else {
            icon_btn(
                "restart",
                Msg::Upgrade(UpgradeMsg::CheckForUpdates { manual: true }),
            )
        };

        let mut extra = column![].spacing(4);
        if let UpgradeState::Available(r) = &self.upgrade {
            let mut actions = row![].spacing(8).align_y(Center);
            // Same action set, same order, same "Unknown install method"
            // guard the palette's update-actions strip uses — both derive
            // from `update_available_actions` so they can't drift. Only the
            // rendering (full-size `modal_action_sized` buttons here vs. the
            // palette's compact keyboard-driven strip) differs.
            let method_unknown = matches!(
                self.upgrade_method,
                grove_core::upgrade::InstallMethod::Unknown
            );
            for action in session_launcher::update_available_actions(method_unknown) {
                // Exhaustive match (no wildcard) so a new `UpdateAction`
                // variant fails to compile here instead of silently being
                // skipped by this row.
                let style = match action {
                    session_launcher::UpdateAction::UpdateNow => ModalBtn::Primary,
                    session_launcher::UpdateAction::SkipVersion
                    | session_launcher::UpdateAction::CopyUrl => ModalBtn::Plain,
                };
                actions = actions.push(modal_action_sized(
                    action.label(),
                    style,
                    11,
                    action.to_msg(),
                ));
            }
            extra = extra.push(Space::new().height(2)).push(actions);

            if !r.body.is_empty() {
                let truncated: String = r
                    .body
                    .lines()
                    .take(6)
                    .collect::<Vec<_>>()
                    .join("\n")
                    .chars()
                    .take(300)
                    .collect();
                extra = extra
                    .push(Space::new().height(4))
                    .push(text(truncated).size(11).color(c::FG_MUTE()));
            }
        }

        let body_zone = column![scroll_body, extra].spacing(10);

        // The version/status strip merges into the shared footer chrome,
        // with an [esc] close hint trailing on the right.
        let footer = modal_footer_row(
            row![
                text(format!("v{current_ver}")).size(11).color(c::FG_DIM()),
                status_line,
                refresh,
                Space::new().width(Length::Fill),
                modal_action_sized(
                    "View changelog",
                    ModalBtn::Plain,
                    11,
                    Msg::Upgrade(UpgradeMsg::OpenChangelog)
                ),
                Space::new().width(10),
                footer_hint("esc", "close"),
            ]
            .spacing(10)
            .align_y(Center)
            .into(),
        );

        let body = column![
            header,
            divider_h(c::BORDER_SOFT()),
            container(body_zone).padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            footer,
        ];

        modal_panel(body.into(), 580.0)
    }

    /// Two-column keyboard-shortcut reference (mod+/). On macOS the ⌘ is
    /// rendered as the SVG `command` icon (the bundled fonts have no
    /// U+2318 glyph); elsewhere key labels stay plain text via
    /// `platform_mod_label()`.
    pub(super) fn shortcut_overlay_modal(&self) -> Element<'_, Msg> {
        let m = platform_mod_label();
        // Alt-chord rows layer Alt on top of the platform modifier instead of
        // using it plain, e.g. "cmd+alt+n" / "ctrl+alt+n" (never
        // "ctrl+shift+alt+n" — see `requires_alt` on `ShortcutDef`).
        let alt_m = if cfg!(target_os = "macos") {
            "cmd+alt"
        } else {
            "ctrl+alt"
        };
        let key_label = |d: &ShortcutDef| {
            if d.literal {
                // Already the complete chord text (e.g. the terminal-panel
                // resize, which is Ctrl+Shift on every platform, not `mod`).
                d.display_keys.to_string()
            } else if d.requires_alt {
                format!("{alt_m}+{}", d.display_keys)
            } else {
                format!("{m}+{}", d.display_keys)
            }
        };
        let screen = self.current_screen();

        // Registry entries visible on this screen: Global or matching current screen.
        let visible: Vec<&ShortcutDef> = SHORTCUTS
            .iter()
            .filter(|d| crate::gui::update::scope_allows(d.scopes, screen))
            .collect();

        // Does the visible set span more than one scope? (Global vs current-screen)
        let has_global = visible.iter().any(|d| d.scopes.contains(&Scope::Global));
        let has_screen = visible
            .iter()
            .any(|d| d.scopes.contains(&Scope::Screen(screen)));
        let grouped = has_global && has_screen;

        // Static display-only rows the behavioral registry deliberately omits.
        let static_rows: [(String, &'static str); 2] = [
            (format!("{m}+c / {m}+v"), "Copy / paste in session"),
            ("esc".into(), "Close modals"),
        ];

        // Render a key-chord string as an Element, swapping any "cmd"
        // occurrence for the SVG ⌘ icon on macOS (with the "+" right after
        // it dropped, e.g. "cmd+alt+n" -> ⌘ "alt+n", "cmd+c / cmd+v" ->
        // ⌘ "c / " ⌘ "v"). Non-mac and literal chords (no "cmd" substring)
        // render unchanged as plain text.
        let chord_keys = |keys: &str| -> Element<'_, Msg> {
            if !cfg!(target_os = "macos") || !keys.contains("cmd") {
                return keycap_text(keys.to_string(), c::FG_DIM());
            }
            let mut parts = keys.split("cmd");
            let mut els: Vec<Element<'_, Msg>> = Vec::new();
            if let Some(first) = parts.next() {
                if !first.is_empty() {
                    els.push(
                        text(first.to_string())
                            .font(MONO_FONT)
                            .size(11)
                            .color(c::FG_DIM())
                            .into(),
                    );
                }
            }
            for part in parts {
                els.push(icon("command", 10.0, c::FG_DIM()));
                let rest = part.strip_prefix('+').unwrap_or(part);
                if !rest.is_empty() {
                    els.push(
                        text(rest.to_string())
                            .font(MONO_FONT)
                            .size(11)
                            .color(c::FG_DIM())
                            .into(),
                    );
                }
            }
            keycap(
                Row::with_children(els)
                    .spacing(1)
                    .align_y(iced::Alignment::Center)
                    .into(),
            )
        };

        let make_row = |keys: String, desc: &'static str| {
            row![
                container(chord_keys(&keys)).width(Length::Fixed(170.0)),
                text(desc).size(11).color(c::FG_DIM()),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center)
        };

        // Split a flat list of (keys, desc) rows into the two-column layout.
        let two_columns = |rows: Vec<(String, &'static str)>| {
            let mut cols = row![].spacing(24);
            if rows.is_empty() {
                return cols; // chunks(0) would panic on an empty list
            }
            let half = rows.len().div_ceil(2);
            for chunk in rows.chunks(half) {
                let mut col = Column::new().spacing(6);
                for (keys, desc) in chunk {
                    col = col.push(make_row(keys.clone(), desc));
                }
                cols = cols.push(col.width(Length::FillPortion(1)));
            }
            cols
        };

        let mut body = column![].spacing(12);

        if grouped {
            // Global section: registry Global rows + the static copy/paste/esc rows.
            let mut global_rows: Vec<(String, &'static str)> = visible
                .iter()
                .filter(|d| d.scopes.contains(&Scope::Global))
                .map(|d| (key_label(d), d.description))
                .collect();
            for (keys, desc) in &static_rows {
                global_rows.push((keys.clone(), desc));
            }
            // Screen section: registry rows scoped to the current screen.
            let screen_rows: Vec<(String, &'static str)> = visible
                .iter()
                .filter(|d| d.scopes.contains(&Scope::Screen(screen)))
                .map(|d| (key_label(d), d.description))
                .collect();

            if !global_rows.is_empty() {
                body = body.push(section_header("GLOBAL", 0.0, 0.0));
                body = body.push(two_columns(global_rows));
            }
            if !screen_rows.is_empty() {
                body = body.push(section_header(&screen.label().to_uppercase(), 0.0, 0.0));
                body = body.push(two_columns(screen_rows));
            }
        } else {
            // Single scope (all-Global today): render a flat, headerless list, one
            // shortcut per row, derived straight from the registry. (The old
            // hand-authored overlay combined a couple of related pairs onto single
            // lines; we keep the registry as the sole source of order and text
            // rather than re-introducing a parallel display layout.)
            let mut rows: Vec<(String, &'static str)> = visible
                .iter()
                .map(|d| (key_label(d), d.description))
                .collect();
            for (keys, desc) in &static_rows {
                rows.push((keys.clone(), desc));
            }
            body = body.push(two_columns(rows));
        }

        let panel_body = column![
            modal_header("Keyboard shortcuts", c::MAGENTA()),
            divider_h(c::BORDER_SOFT()),
            container(body).padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            modal_footer_hints(&[("esc", "close")]),
        ];

        modal_panel(panel_body.into(), 640.0)
    }
}
