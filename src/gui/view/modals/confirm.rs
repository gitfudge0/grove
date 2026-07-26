//! Input/confirm/remove-project/message/teardown modals — the simple
//! prompt-shaped and progress-shaped dialogs (as opposed to the
//! richer settings/theme panels living in the sibling files).

use super::super::modal_input_id;
use crate::app::ConfirmKind;
use crate::gui::palette as c;
use crate::gui::state::{Grove, Msg, PtyPane};
use crate::gui::widgets::{
    divider_h, modal_action, modal_checkbox, modal_footer_hints, modal_header, modal_panel,
    ModalBtn,
};
use iced::border::Radius;
use iced::widget::{column, container, row, text, text_input, Space};
use iced::{Background, Border, Element, Length, Padding};

impl Grove {
    pub(super) fn input_modal<'a>(
        &'a self,
        title: &'a str,
        buffer: &'a str,
        note: Option<&'a str>,
    ) -> Element<'a, Msg> {
        let field = text_input("", buffer)
            .id(modal_input_id())
            .font(crate::gui::metrics::UI_FONT)
            .size(14)
            .padding(0)
            .on_input(Msg::InputPathChanged)
            .on_submit(Msg::ModalSubmit)
            .style(crate::gui::widgets::palette_input_style);

        let input_zone = container(
            row![
                crate::gui::icons::icon("git-branch", 16.0, c::FG_MUTE()),
                field
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([14, 16]));

        let mut buttons_zone = column![].spacing(8);
        if let Some(note) = note {
            buttons_zone = buttons_zone.push(text(note.to_string()).size(12).color(c::RED()));
        }
        buttons_zone = buttons_zone.push(
            row![
                Space::new().width(Length::Fill),
                modal_action("Cancel", ModalBtn::Plain, Msg::ModalCancel),
                modal_action("Submit", ModalBtn::Primary, Msg::ModalSubmit),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        );

        let body = column![
            modal_header(title, c::MAGENTA()),
            divider_h(c::BORDER_SOFT()),
            input_zone,
            divider_h(c::BORDER_SOFT()),
            container(buttons_zone).padding(Padding::from([12, 16])),
            divider_h(c::BORDER_SOFT()),
            modal_footer_hints(&[("⏎", "confirm"), ("esc", "cancel")]),
        ];

        modal_panel(body.into(), 480.0)
    }

    pub(super) fn confirm_modal<'a>(
        &'a self,
        title: &'a str,
        prompt: &'a str,
        destructive: bool,
        kind: &'a ConfirmKind,
    ) -> Element<'a, Msg> {
        let accent = if destructive { c::RED() } else { c::MAGENTA() };
        let confirm_label = match kind {
            ConfirmKind::Quit => "Quit",
            _ if destructive => "Remove",
            _ => "Confirm",
        };
        let confirm_label_lower = match kind {
            ConfirmKind::Quit => "quit",
            _ if destructive => "remove",
            _ => "confirm",
        };
        let body_zone = column![
            text(prompt.to_string())
                .size(13)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::Word),
            Space::new().height(8),
            row![
                Space::new().width(Length::Fill),
                modal_action("Cancel", ModalBtn::Plain, Msg::ModalConfirm(false)),
                modal_action(
                    confirm_label,
                    if destructive {
                        ModalBtn::Danger
                    } else {
                        ModalBtn::Primary
                    },
                    Msg::ModalConfirm(true)
                ),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        let footer = if destructive {
            modal_footer_hints(&[("y", confirm_label_lower), ("esc", "cancel")])
        } else {
            modal_footer_hints(&[("⏎", "confirm"), ("esc", "cancel")])
        };

        let body = column![
            modal_header(title, accent),
            divider_h(c::BORDER_SOFT()),
            container(body_zone).padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            footer,
        ];

        modal_panel(body.into(), 480.0)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn remove_project_modal<'a>(
        &'a self,
        name: &'a str,
        worktrees: &'a [String],
        also_remove: bool,
        in_progress: bool,
        done: usize,
        current: &'a str,
        errors: &'a [String],
    ) -> Element<'a, Msg> {
        use iced::widget::progress_bar;
        use iced::widget::progress_bar::Style as ProgressStyle;

        let accent = c::RED();
        let total = worktrees.len();
        let prompt = if total == 0 {
            format!("'{name}' will be unregistered from Grove. Files on disk stay put.")
        } else {
            format!(
                "'{name}' will be unregistered from Grove. Non-main worktrees stay on disk unless you opt in below."
            )
        };
        let session_note = "Running sessions for this project will be stopped.";

        let mut body = column![
            text(prompt)
                .size(13)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::Word),
            text(session_note)
                .size(12)
                .color(c::FG_MUTE())
                .wrapping(iced::widget::text::Wrapping::Word),
        ]
        .spacing(12);

        if total > 0 {
            let label = if total == 1 {
                "Delete 1 non-main worktree from disk".to_string()
            } else {
                format!("Delete {total} non-main worktrees from disk")
            };
            let cb = modal_checkbox(
                label,
                also_remove,
                c::RED(),
                if in_progress {
                    None
                } else {
                    Some(Msg::ToggleRemoveWorktrees)
                },
            );
            body = body
                .push(divider_h(c::BORDER_SOFT()))
                .push(Space::new().height(2))
                .push(cb);
        }

        if in_progress {
            let frac = if total == 0 {
                1.0
            } else {
                (done as f32 / total as f32).clamp(0.0, 1.0)
            };
            let status = if done >= total {
                "Finishing…".to_string()
            } else {
                format!("Removing {} of {}: {}", done + 1, total, current)
            };
            body = body
                .push(divider_h(c::BORDER_SOFT()))
                .push(Space::new().height(4))
                .push(
                    text(status)
                        .size(11)
                        .color(c::FG_MUTE())
                        .wrapping(iced::widget::text::Wrapping::None),
                )
                .push(
                    progress_bar(0.0..=1.0, frac)
                        .girth(6.0)
                        .style(|_| ProgressStyle {
                            background: Background::Color(c::BG_STRIP()),
                            bar: Background::Color(c::RED()),
                            border: Border {
                                color: c::BORDER(),
                                width: 1.0,
                                radius: Radius::from(4.0),
                            },
                        }),
                );
        } else {
            body = body.push(divider_h(c::BORDER_SOFT())).push(
                row![
                    Space::new().width(Length::Fill),
                    modal_action("Cancel", ModalBtn::Plain, Msg::ModalCancel),
                    modal_action("Remove", ModalBtn::Danger, Msg::ConfirmRemoveProject),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
        }

        if !errors.is_empty() {
            let summary = format!("{} worktree(s) failed to remove", errors.len());
            body = body.push(
                text(summary)
                    .size(11)
                    .color(c::RED())
                    .wrapping(iced::widget::text::Wrapping::Word),
            );
        }

        let mut panel_body = column![
            modal_header("Remove project", accent),
            divider_h(c::BORDER_SOFT()),
            container(body).padding(Padding::from([14, 16])),
        ];
        if !in_progress {
            panel_body = panel_body
                .push(divider_h(c::BORDER_SOFT()))
                .push(modal_footer_hints(&[
                    ("y", "remove"),
                    ("space", "toggle delete"),
                    ("esc", "cancel"),
                ]));
        }

        modal_panel(panel_body.into(), 520.0)
    }

    pub(super) fn message_modal<'a>(&'a self, message: &'a str) -> Element<'a, Msg> {
        let body_zone = column![
            text(message.to_string())
                .size(13)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::Word),
            Space::new().height(8),
            row![
                Space::new().width(Length::Fill),
                modal_action("Close", ModalBtn::Primary, Msg::ModalCancel),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        let body = column![
            modal_header("Notice", c::CYAN()),
            divider_h(c::BORDER_SOFT()),
            container(body_zone).padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            modal_footer_hints(&[("esc", "close")]),
        ];

        modal_panel(body.into(), 480.0)
    }

    pub(super) fn teardown_modal(&self) -> Element<'_, Msg> {
        use crate::app::TeardownStage;
        let td = match &self.app.teardown {
            Some(td) => td,
            None => return Space::new().width(0).into(),
        };
        let wt_name = crate::app::path_basename(&td.wt_path);
        let done = matches!(td.stage, TeardownStage::Done { .. });
        let running = matches!(td.stage, TeardownStage::RunningScript);

        let header = modal_header(&format!("Delete worktree / {wt_name}"), c::RED());

        let mut body = column![].spacing(12);

        // Embedded teardown-script PTY (read-only) while it runs / after it
        // exits, until removal completes and the session is dropped.
        if let Some(s) = &td.session {
            let pty = container(self.pty(PtyPane::Agent, s))
                .width(Length::Fill)
                .height(Length::Fixed(220.0))
                .style(|_| container::Style {
                    background: Some(Background::Color(c::BG())),
                    border: Border {
                        color: c::BORDER(),
                        width: 1.0,
                        radius: Radius::from(4.0),
                    },
                    ..Default::default()
                });
            body = body.push(pty);
        }

        body = body.push(
            text(td.message.clone())
                .size(13)
                .color(if done { c::FG_DIM() } else { c::FG_MUTE() })
                .wrapping(iced::widget::text::Wrapping::Word),
        );

        let buttons = if done {
            row![
                Space::new().width(Length::Fill),
                modal_action("Close", ModalBtn::Primary, Msg::ModalCancel),
            ]
        } else if running {
            // Let the user proceed without waiting for a hung teardown script.
            row![
                Space::new().width(Length::Fill),
                modal_action("Skip & remove", ModalBtn::Plain, Msg::ModalCancel),
            ]
        } else {
            row![Space::new().width(Length::Fill)]
        }
        .spacing(8)
        .align_y(iced::Alignment::Center);

        body = body.push(Space::new().height(4)).push(buttons);

        // Esc always dismisses here (`cancel_modal` gates by stage): skip &
        // remove while the teardown script runs, close once removal is done.
        // Mid-removal there's no dismissal (an in-flight `git worktree
        // remove` can't be safely interrupted), so the hint is omitted then.
        let footer = if done {
            Some(modal_footer_hints(&[("esc", "close")]))
        } else if running {
            Some(modal_footer_hints(&[("esc", "skip & remove")]))
        } else {
            None
        };

        let mut panel_body = column![
            header,
            divider_h(c::BORDER_SOFT()),
            container(body).padding(Padding::from([14, 16])),
        ];
        if let Some(footer) = footer {
            panel_body = panel_body.push(divider_h(c::BORDER_SOFT())).push(footer);
        }

        modal_panel(panel_body.into(), 560.0)
    }
}
