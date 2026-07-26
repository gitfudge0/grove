//! The first-run onboarding wizard's view. `Grove::view()` short-circuits to
//! `onboarding_view` while `Modal::Onboarding` is active — see the top of
//! `view.rs`'s `view()`. Pure code motion out of `view.rs`; state and `Msg`
//! for onboarding still live on `App::Modal::Onboarding`.

use super::icons::icon;
use super::metrics::{ROW_H, UI_BOLD, UI_FONT};
use super::palette as c;
use super::state::{Grove, Msg};
use super::view::{cap, git_on_path, input_field_style, modal_input_id, modal_name_id};
use super::widgets::{dot, modal_action, modal_list_row, skip_perms_seg, ModalBtn};
use crate::app::OnboardStep;
use crate::gui::state::OnboardingMsg;
use iced::border::Radius;
use iced::widget::{column, container, row, text, text_input, Column, Row, Space};
use iced::{Background, Border, Color, Element, Length, Padding};

impl Grove {
    /// The first-run onboarding wizard: a full-viewport page (no modal
    /// chrome, no sidebar/statusbar/scrim behind it) that walks the user
    /// through four steps in grove's own quiet visual language. `view()`
    /// returns this directly while `Modal::Onboarding` is active, bypassing
    /// the modal layer entirely (see the top of `view()`).
    ///
    /// Layout: one hard left axis. Rail, wordmark/tagline, bullets,
    /// headings, descriptions, labels and inputs all sit flush to the left
    /// edge of a fixed 560px column, which is itself horizontally centered
    /// in the viewport. The column sits slightly above true center via
    /// proportional spacers (44/56 split) rather than `center_y`.
    // One arg over the limit; splitting into a param struct would obscure more than it clarifies.
    // Also slated for extraction into its own module, at which point this will be revisited.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn onboarding_view<'a>(
        &'a self,
        step: OnboardStep,
        path: &'a str,
        dir_sel: usize,
        name: Option<&'a str>,
        note: Option<&'a str>,
        agent_sel: usize,
        perms_skip: bool,
    ) -> Element<'a, Msg> {
        use iced::Alignment::Center;

        // Entrance animation: eases 0 → 1 over `.quick()` (200ms, `EaseOut`)
        // whenever the step changes (and on first show). Drives a fade
        // (text/dot alpha) and an 8px settle (top padding on the centered
        // column) — see `Grove::onb_step_anim`.
        let t = self
            .anim
            .onb_step_anim
            .interpolate(0.0_f32, 1.0_f32, std::time::Instant::now());
        let slide_pad = 8.0 * (1.0 - t);
        let fg = Color { a: t, ..c::FG() };
        let fg_dim = Color {
            a: t,
            ..c::FG_DIM()
        };

        // ── progress rail ───────────────────────────────────────────────────
        let mut rail = Row::new().spacing(10).align_y(Center);
        for &s in OnboardStep::flow() {
            let (dotc, txtc) = if s == step {
                (
                    Color {
                        a: t,
                        ..c::MAGENTA()
                    },
                    c::FG(),
                )
            } else if s.index_in() < step.index_in() {
                (c::MAGENTA(), c::FG_DIM())
            } else {
                (c::BORDER(), c::FG_MUTE())
            };
            rail = rail.push(
                row![dot(dotc), text(s.label()).size(10).color(txtc)]
                    .spacing(5)
                    .align_y(Center),
            );
        }

        // ── step body ────────────────────────────────────────────────────────
        let body: Element<'_, Msg> = match step {
            OnboardStep::Welcome => column![
                row![
                    icon("grid", 32.0, Color { a: t, ..c::CYAN() }),
                    text("grove").size(32).font(UI_BOLD).color(fg),
                ]
                .spacing(10)
                .align_y(Center),
                text("a worktree launchpad for AI coding agents")
                    .size(15)
                    .color(fg_dim),
                Space::new().height(20),
                onboard_point(
                    "Sessions are the unit of work",
                    "Every agent you spawn lives in a managed session that survives navigation; switch between them in two keystrokes.",
                ),
                onboard_point(
                    "Worktrees, not branches",
                    "Grove treats Git worktrees as a first-class primitive: create, list, and run agents inside them.",
                ),
                onboard_point(
                    "Quiet and keyboard-first",
                    "The app stays out of the way so terminal output stays primary. This takes about a minute.",
                ),
            ]
            .spacing(10)
            .into(),

            OnboardStep::Environment => {
                let mut list = Column::new().spacing(6);
                let rows = [
                    (git_on_path(), false, "Git", "Version control"),
                    (
                        grove_core::agent::Agent::Claude.available(),
                        false,
                        "Claude",
                        "Claude Code",
                    ),
                    (
                        grove_core::agent::Agent::Codex.available(),
                        false,
                        "Codex",
                        "Codex CLI",
                    ),
                    (
                        grove_core::agent::Agent::OpenCode.available(),
                        false,
                        "OpenCode",
                        "OpenCode CLI",
                    ),
                    (
                        self.app.tmux_available,
                        true,
                        "tmux",
                        "Persists sessions across restarts",
                    ),
                ];
                for (found, optional, n, meta) in rows {
                    list = list.push(onboard_env_row(found, optional, n, meta));
                }
                column![
                    text("Environment").size(20).color(fg),
                    text("Grove spawns agents from your PATH; it doesn't install or authenticate them. Only Git is required to get going.")
                        .size(13)
                        .color(fg_dim)
                        .wrapping(iced::widget::text::Wrapping::Word),
                    Space::new().height(4),
                    list,
                ]
                .spacing(10)
                .into()
            }

            OnboardStep::Project => {
                let path_input = text_input("~/code/my-repo", path)
                    .id(modal_input_id())
                    .font(UI_FONT)
                    .size(14)
                    .padding(Padding::from([8, 12]))
                    .on_input(|v| Msg::Onboarding(OnboardingMsg::PathChanged(v)))
                    .on_submit(Msg::Onboarding(OnboardingMsg::Next))
                    .style(input_field_style);

                let browse = modal_action(
                    if self.picker_open {
                        "Waiting…"
                    } else {
                        "Browse…"
                    },
                    ModalBtn::Plain,
                    Msg::AddProjectBrowse,
                );

                let mut col = column![
                    text("Add your first project").size(20).color(fg),
                    text("Point Grove at a Git repository, or any plain folder for ad-hoc sessions.")
                        .size(13)
                        .color(fg_dim)
                        .wrapping(iced::widget::text::Wrapping::Word),
                    // iced has no letter-spacing; the gaps are literal
                    // characters (single space between letters, three
                    // between words) — copied verbatim from the mock.
                    text("R E P O S I T O R Y   O R   F O L D E R")
                        .size(11)
                        .color(c::FG_MUTE()),
                    row![path_input, browse]
                        .spacing(8)
                        .align_y(Center),
                ]
                .spacing(8);

                // Matches appear only once the user starts typing; an empty
                // field would list the cwd's directories as noise.
                if !path.trim().is_empty() {
                    col = col
                        .push(text("M A T C H E S").size(11).color(c::FG_MUTE()))
                        .push(self.dir_matches(path, dir_sel, 5, |v| Msg::Onboarding(OnboardingMsg::PickDir(v))));
                }

                if let Some(name) = name {
                    let name_input = text_input("project name", name)
                        .id(modal_name_id())
                        .font(UI_FONT)
                        .size(14)
                        .padding(Padding::from([8, 12]))
                        .on_input(|v| Msg::Onboarding(OnboardingMsg::NameChanged(v)))
                        .on_submit(Msg::Onboarding(OnboardingMsg::Next))
                        .style(input_field_style);
                    col = col
                        .push(text("N A M E").size(11).color(c::FG_MUTE()))
                        .push(name_input);
                }

                if let Some(note) = note {
                    col = col.push(text(note.to_string()).size(12).color(c::RED()));
                }
                col = col.push(
                    text("Tab to complete · ↑↓ to select · Enter to continue · Or skip setup")
                        .size(11)
                        .color(c::FG_MUTE()),
                );
                col.into()
            }

            OnboardStep::Session => {
                let mut col = column![text("Start your first session").size(20).color(fg),]
                    .spacing(8);

                match self.app.store.projects.last() {
                    Some(p) => {
                        col = col.push(
                            text(format!("Launch an agent inside {}.", p.name))
                                .size(13)
                                .color(fg_dim)
                                .wrapping(iced::widget::text::Wrapping::Word),
                        );
                        let mut list = Column::new().spacing(0);
                        for (i, agent) in self.app.available_agents.iter().enumerate() {
                            let active = i == agent_sel;
                            list = list.push(modal_list_row(
                                text(cap(agent.label()))
                                    .size(13)
                                    .color(if active { c::FG() } else { c::FG_DIM() }),
                                active,
                                Msg::Onboarding(OnboardingMsg::AgentSelect(i)),
                            ));
                        }
                        let list_h = (self.app.available_agents.len().max(1) as f32) * ROW_H;
                        col = col.push(
                            container(list)
                                .width(Length::Fill)
                                .height(Length::Fixed(list_h))
                                .style(|_| container::Style {
                                    background: Some(Background::Color(c::BG_STRIP())),
                                    border: Border {
                                        color: c::BORDER(),
                                        width: 1.0,
                                        radius: Radius::from(4.0),
                                    },
                                    ..Default::default()
                                }),
                        );
                    }
                    None => {
                        col = col.push(
                            text("No project added. You can add one any time from the sidebar. Finish to start using Grove.")
                                .size(13)
                                .color(fg_dim)
                                .wrapping(iced::widget::text::Wrapping::Word),
                        );
                    }
                }
                col = col
                    .push(Space::new().height(4))
                    .push(
                        row![
                            text("P E R M I S S I O N S").size(11).color(c::FG_MUTE()),
                            Space::new().width(20),
                            skip_perms_seg(
                                perms_skip,
                                Msg::Onboarding(OnboardingMsg::PermsSelect(true)),
                                Msg::Onboarding(OnboardingMsg::PermsSelect(false))
                            ),
                        ]
                        .align_y(Center),
                    )
                    .push(
                        text(if perms_skip {
                            "Skip: agents run any command without asking"
                        } else {
                            "Safe: agents ask before running commands"
                        })
                        .size(11)
                        .color(if perms_skip { c::YELLOW() } else { c::FG_MUTE() }),
                    );
                col.into()
            }
        };

        // ── footer ────────────────────────────────────────────────────────────
        let next_label = match step {
            OnboardStep::Welcome => "Get started",
            OnboardStep::Session => "Launch session",
            _ => "Continue",
        };
        let count = format!("{} / {}", step.index_in() + 1, OnboardStep::flow().len());
        let mut footer = row![
            text(count).size(12).color(c::FG_MUTE()),
            Space::new().width(Length::Fill),
            modal_action(
                "Skip setup",
                ModalBtn::Plain,
                Msg::Onboarding(OnboardingMsg::Skip)
            ),
        ]
        .spacing(8)
        .align_y(Center);
        if step.prev().is_some() {
            footer = footer.push(modal_action(
                "Back",
                ModalBtn::Plain,
                Msg::Onboarding(OnboardingMsg::Back),
            ));
        }
        footer = footer.push(modal_action(
            next_label,
            ModalBtn::Primary,
            Msg::Onboarding(OnboardingMsg::Next),
        ));

        // Small top-left wordmark — the wizard's only persistent chrome.
        // Distinct from the (larger, centered) wordmark the Welcome step's
        // `body` renders as part of its own content.
        let brand = row![
            icon("grid", 15.0, c::CYAN()),
            text("grove").font(UI_BOLD).size(14).color(c::MAGENTA()),
        ]
        .spacing(8)
        .align_y(Center);

        // One hard left axis: rail and body both sit flush to the left edge
        // of a fixed 560px column. The column itself is horizontally
        // centered in the viewport, but nothing inside it is — no centered
        // text anywhere in the content.
        let content = column![rail, container(body).width(Length::Fixed(560.0))]
            .width(Length::Fixed(560.0))
            .align_x(iced::Alignment::Start)
            .spacing(22)
            .padding(Padding {
                top: slide_pad,
                ..Padding::ZERO
            });

        // Vertical bias: the column sits slightly above true center (~44%
        // from the top) via a proportional 44/56 spacer split, rather than
        // `center_y`.
        let centered = container(
            column![
                Space::new().height(Length::FillPortion(44)),
                content,
                Space::new().height(Length::FillPortion(56)),
            ]
            .width(Length::Fill)
            .align_x(Center),
        )
        .width(Length::Fill)
        .height(Length::Fill);

        column![
            container(brand).padding(Padding::from([16, 20])),
            centered,
            container(footer)
                .width(Length::Fill)
                .padding(Padding::from([16, 20])),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }
}

/// One bulleted value-prop line on the welcome step: a magenta mark, a bold
/// lead, and a muted explanation that wraps.
fn onboard_point<'a>(lead: &'a str, body: &'a str) -> Element<'a, Msg> {
    row![
        // A drawn marker, not a glyph: the bundled fonts have no U+25xx box
        // characters, so a text bullet renders as tofu. Nudged down to sit on
        // the lead line's baseline.
        container(dot(c::MAGENTA())).padding(Padding {
            top: 6.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }),
        column![
            text(lead).size(14).color(c::FG()),
            text(body)
                .size(13)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::Word),
        ]
        .spacing(2),
    ]
    .spacing(10)
    .into()
}

/// One detected-tool row on the environment step: a status dot, the tool name,
/// a muted description, and a right-aligned found/missing/optional tag.
fn onboard_env_row<'a>(
    found: bool,
    optional: bool,
    name: &'a str,
    meta: &'a str,
) -> Element<'a, Msg> {
    let (dotc, tag, tagc) = if found {
        (c::GREEN(), "Found", c::GREEN())
    } else if optional {
        (c::AMBER(), "Optional", c::AMBER())
    } else {
        (c::FG_MUTE(), "Missing", c::FG_MUTE())
    };
    container(
        row![
            dot(dotc),
            text(name.to_string()).size(13).color(c::FG()),
            text(meta.to_string()).size(12).color(c::FG_MUTE()),
            Space::new().width(Length::Fill),
            text(tag).size(11).color(tagc),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([8, 12]))
    .style(|_| container::Style {
        background: Some(Background::Color(c::BG_STRIP())),
        border: Border {
            color: c::BORDER(),
            width: 1.0,
            radius: Radius::from(4.0),
        },
        ..Default::default()
    })
    .into()
}
