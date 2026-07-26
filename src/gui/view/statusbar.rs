//! The bottom status bar: running-session count, backend/theme labels,
//! toast, and the palette/shortcuts hint buttons.

use crate::gui::icons::icon;
use crate::gui::metrics::{MONO_FONT, STATUS_H};
use crate::gui::palette as c;
use crate::gui::session_launcher;
use crate::gui::state::{Grove, Msg};
use crate::gui::update::{platform_mod_label, GlobalShortcut, SHORTCUTS};
use crate::gui::widgets::{divider_h, dot, keycap};
use grove_core::session::SessionStatus;
use iced::widget::{button, column, container, row, text, Space};
use iced::{Background, Element, Length, Padding};

impl Grove {
    // ── status bar ────────────────────────────────────────────────────────
    pub(super) fn statusbar(&self) -> Element<'_, Msg> {
        let running = self
            .app
            .sessions
            .iter()
            .filter(|s| {
                matches!(
                    *s.status.lock().unwrap_or_else(|e| e.into_inner()),
                    SessionStatus::Running
                )
            })
            .count();
        let backend = if self.app.use_tmux() {
            "tmux"
        } else {
            "native"
        };
        let theme_name = self
            .app
            .store
            .theme
            .clone()
            .unwrap_or_else(|| "tokyonight".into());

        let mut left = row![
            row![
                dot(if running > 0 {
                    c::GREEN()
                } else {
                    c::FG_MUTE()
                }),
                text(format!("{running}"))
                    .font(MONO_FONT)
                    .size(10)
                    .color(c::FG_DIM()),
                text("RUNNING").font(MONO_FONT).size(10).color(c::FG_MUTE()),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
            row![
                text("BACKEND").font(MONO_FONT).size(10).color(c::FG_MUTE()),
                text(backend).font(MONO_FONT).size(10).color(c::FG_DIM()),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
            row![
                text("THEME").font(MONO_FONT).size(10).color(c::FG_MUTE()),
                text(theme_name).font(MONO_FONT).size(10).color(c::FG_DIM()),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(14)
        .align_y(iced::Alignment::Center);

        if self.app.skip_permissions_enabled() {
            left = left.push(keycap(
                text("bypass")
                    .font(MONO_FONT)
                    .size(10)
                    .color(c::YELLOW())
                    .into(),
            ));
        }

        let toast: Element<'_, Msg> = match &self.app.toast {
            Some(t) => {
                let color = match t.kind {
                    crate::app::ToastKind::Error => c::RED(),
                    crate::app::ToastKind::Info => c::GREEN(),
                };
                text(t.message.clone())
                    .font(MONO_FONT)
                    .size(10)
                    .color(color)
                    .into()
            }
            None => Space::new().width(0).into(),
        };

        let modifier = platform_mod_label();
        // Build a footer-hint style button: a keycap chip (mod icon + key on
        // macOS, "{mod}+{key}" text elsewhere) followed by a muted mono
        // label, matching the palette footer's `footer_hint` chrome — but
        // wrapped in a button since these still need `on_press`.
        let hint_button = |key: &str, label: &'static str, msg: Msg| -> Element<'_, Msg> {
            let keycap_content: Element<'_, Msg> = if cfg!(target_os = "macos") {
                row![
                    icon("command", 9.0, c::FG_DIM()),
                    text(key.to_string())
                        .font(MONO_FONT)
                        .size(10)
                        .color(c::FG_DIM()),
                ]
                .spacing(1)
                .align_y(iced::Alignment::Center)
                .into()
            } else {
                text(format!("{modifier}+{key}"))
                    .font(MONO_FONT)
                    .size(10)
                    .color(c::FG_DIM())
                    .into()
            };
            let content = row![keycap(keycap_content), text(label).font(MONO_FONT).size(10),]
                .spacing(6)
                .align_y(iced::Alignment::Center);
            button(content)
                .padding(0)
                .on_press(msg)
                .style(|_, status| button::Style {
                    background: None,
                    text_color: if matches!(status, button::Status::Hovered) {
                        c::FG()
                    } else {
                        c::FG_MUTE()
                    },
                    ..Default::default()
                })
                .into()
        };

        let overlay_key = SHORTCUTS
            .iter()
            .find(|d| d.action == Some(GlobalShortcut::ShortcutOverlay))
            .map(|d| d.display_keys)
            .unwrap_or("/");
        let shortcuts_chip = hint_button(overlay_key, "shortcuts", Msg::OpenShortcutOverlay);

        let palette_key = SHORTCUTS
            .iter()
            .find(|d| d.action == Some(GlobalShortcut::NewSession))
            .map(|d| d.display_keys)
            .unwrap_or("p");
        let palette_chip = hint_button(
            palette_key,
            "palette",
            Msg::SessionLauncher(session_launcher::Msg::Open),
        );

        let right = row![
            palette_chip,
            Space::new().width(14),
            shortcuts_chip,
            Space::new().width(14),
            text(format!("v{}", env!("CARGO_PKG_VERSION")))
                .font(MONO_FONT)
                .size(10)
                .color(c::FG_MUTE()),
        ]
        .align_y(iced::Alignment::Center);

        let bar = row![
            left,
            Space::new().width(24),
            toast,
            Space::new().width(Length::Fill),
            right,
        ]
        .padding(Padding::from([0, 16]))
        .align_y(iced::Alignment::Center)
        .height(Length::Fill);

        let bar_container = container(bar)
            .height(STATUS_H - 1.0)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP())),
                ..Default::default()
            });

        column![divider_h(c::BORDER_SOFT()), bar_container]
            .width(Length::Fill)
            .height(STATUS_H)
            .into()
    }
}
