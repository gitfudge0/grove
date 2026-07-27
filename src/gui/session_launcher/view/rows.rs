//! Row rendering shared by the palette's results list: the results-row
//! dispatch (`palette_row_view`), the agent icon+label+subtitle idiom
//! reused across `Recent`/`Combo`/options-context rows (`palette_agent_content`),
//! and the Tab-revealed per-row actions strip (`palette_row_actions_strip`).

use super::super::state::{Msg, PaletteRow};
use crate::gui::icons::icon;
use crate::gui::metrics::{MONO_FONT, UI_FONT};
use crate::gui::palette as c;
use crate::gui::state::{Grove, Msg as GMsg};
use crate::gui::view::{cap, digit_label, highlighted_line, mod_key_chip};
use crate::gui::widgets::{keycap_text, launcher_row, modal_list_row_sized, PALETTE_ROW_H};
use iced::border::Radius;
use iced::widget::{button, column, container, row, text, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};

/// One agent icon button in the row-actions strip's agent bar (mock F): a
/// 26px rounded square, ringed yellow when it's the selected agent.
const AGENT_BTN: f32 = 26.0;
/// The strip's "Launch session…" row grows to fit the agent bar; every
/// other strip row keeps the flat 30px.
const STRIP_ROW_H: f32 = 30.0;
const STRIP_LAUNCH_ROW_H: f32 = 42.0;

impl Grove {
    /// Icon (in a fixed 24px slot, so titles align across rows regardless of
    /// icon glyph width) + agent label + mono-muted "project / worktree"
    /// subtitle — the visual idiom of the `Recent`/`Combo` rows (same idiom
    /// as `attention_dropdown`).
    /// `agent_ranges`/`subtitle_ranges` are typing-state fuzzy-match char
    /// ranges to render cyan (pass `&[]` where nothing should highlight, e.g.
    /// root-state `Recent` rows). `trailing`, if given, right-aligns after a
    /// filling gap — the row's ⌘-digit or ⏎ keycap.
    pub(super) fn palette_agent_content<'a>(
        &'a self,
        agent: grove_core::agent::Agent,
        subtitle: String,
        agent_ranges: &[(usize, usize)],
        subtitle_ranges: &[(usize, usize)],
        trailing: Option<Element<'a, GMsg>>,
        active: bool,
    ) -> Element<'a, GMsg> {
        let title = cap(agent.label());
        let title_el = highlighted_line(&title, agent_ranges, c::FG(), UI_FONT, 13.0);
        let subtitle_el =
            highlighted_line(&subtitle, subtitle_ranges, c::FG_MUTE(), MONO_FONT, 10.5);
        // The agent glyph lights up yellow on the selected row; resting
        // rows keep it muted.
        let icon_color = if active { c::YELLOW() } else { c::FG_MUTE() };
        let icon_slot = container(icon(agent.icon_name(), 16.0, icon_color))
            .width(24.0)
            .align_x(iced::alignment::Horizontal::Center);

        let mut content = row![icon_slot, column![title_el, subtitle_el].spacing(2)]
            .spacing(8)
            .align_y(iced::Alignment::Center);
        if let Some(t) = trailing {
            content = content.push(Space::new().width(Length::Fill)).push(t);
        }
        content.into()
    }
    /// Render one row of the root/typing/browse-all list. `input` recomputes
    /// the typing-state fuzzy-match highlight ranges for `Combo` rows
    /// (root-state `Recent` rows never highlight, since the query is empty
    /// there); `root_mode` gates the ⌘-digit chip on `Recent` rows — hidden
    /// while typing/browsing, per the redesign. Every row, active or not,
    /// swaps its natural trailing chip (digit / ⌘T) for a ⏎ keycap when it's
    /// the current selection.
    pub(super) fn palette_row_view<'a>(
        &'a self,
        i: usize,
        row: &PaletteRow,
        active: bool,
        input: &str,
        root_mode: bool,
    ) -> Element<'a, GMsg> {
        let enter_chip = || keycap_text("⏎", c::FG_DIM());
        // Action rows share the session rows' 24px icon rail so titles align.
        let icon_slot = |name: &'static str, color: Color| {
            container(icon(name, 16.0, color))
                .width(24.0)
                .align_x(iced::alignment::Horizontal::Center)
        };
        match row {
            PaletteRow::Recent {
                proj,
                wt_path,
                agent: _,
            }
            | PaletteRow::Combo {
                proj,
                wt_path,
                agent: _,
            } => {
                let pname = self
                    .app
                    .store
                    .projects
                    .get(*proj)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                let wt_name = self
                    .launcher_worktrees(*proj)
                    .iter()
                    .find(|w| &w.path == wt_path)
                    .map_or_else(
                        || crate::app::path_basename(wt_path),
                        |w| {
                            if w.branch.is_empty() {
                                crate::app::path_basename(&w.path)
                            } else {
                                w.branch.clone()
                            }
                        },
                    );
                let subtitle = format!("{pname} / {wt_name}");
                let is_recent = matches!(row, PaletteRow::Recent { .. });

                // While the row-actions strip is open on this row, its
                // agent bar (←→) retitles the row, so the row and the
                // launch it's about to perform can't disagree — and the
                // fuzzy-highlight ranges are computed against the shown
                // label, not the row's original one.
                let shown_agent = self.palette_row_agent(row);
                let m = (!input.is_empty()).then(|| {
                    crate::gui::launcher::fuzzy_match_indices(
                        input,
                        &pname,
                        &wt_name,
                        shown_agent.label(),
                    )
                });
                let agent_ranges: &[(usize, usize)] =
                    m.as_ref().map_or(&[][..], |m| m.agent.as_slice());
                // The subtitle is "{pname} / {wt_name}"; the worktree match's
                // ranges (computed against `wt_name` alone) need shifting by
                // that prefix's char length to land in the right place.
                let prefix_len = pname.chars().count() + 3;
                let subtitle_ranges: Vec<(usize, usize)> = m
                    .as_ref()
                    .map(|m| {
                        m.project
                            .iter()
                            .copied()
                            .chain(
                                m.worktree
                                    .iter()
                                    .map(|(s, e)| (s + prefix_len, e + prefix_len)),
                            )
                            .collect()
                    })
                    .unwrap_or_default();

                let trailing = if active {
                    Some(enter_chip())
                } else if is_recent && root_mode {
                    digit_label(i).map(|d| mod_key_chip(d, c::FG_MUTE()))
                } else if !is_recent && root_mode {
                    // Root's no-recents worktree-listing fallback (the only
                    // place a `Combo` row renders in root state): a
                    // persistent teaching hint since there's no recency
                    // digit to show instead.
                    Some(
                        text("↵ starts a session")
                            .font(MONO_FONT)
                            .size(10)
                            .color(c::FG_MUTE())
                            .into(),
                    )
                } else {
                    None
                };

                launcher_row(
                    self.palette_agent_content(
                        shown_agent,
                        subtitle,
                        agent_ranges,
                        &subtitle_ranges,
                        trailing,
                        active,
                    ),
                    active,
                    true,
                    GMsg::SessionLauncher(Msg::Activate(i)),
                    PALETTE_ROW_H,
                )
            }
            PaletteRow::NewSession => {
                let mut content = row![
                    icon_slot("plus", c::MAGENTA()),
                    text("New session…").size(13).color(c::MAGENTA()),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                if active {
                    content = content
                        .push(Space::new().width(Length::Fill))
                        .push(enter_chip());
                }
                modal_list_row_sized(
                    content,
                    active,
                    GMsg::SessionLauncher(Msg::Activate(i)),
                    36.0,
                    6.0,
                    12.0,
                )
            }
            PaletteRow::TerminalHome => {
                let content = row![
                    icon_slot("term", c::FG_MUTE()),
                    text("Terminal at ~").size(13).color(if active {
                        c::FG()
                    } else {
                        c::FG_DIM()
                    }),
                    Space::new().width(Length::Fill),
                    if active {
                        enter_chip()
                    } else {
                        mod_key_chip("t", c::FG_DIM())
                    },
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                modal_list_row_sized(
                    content,
                    active,
                    GMsg::SessionLauncher(Msg::Activate(i)),
                    36.0,
                    6.0,
                    12.0,
                )
            }
            PaletteRow::TerminalWt => {
                let label = self
                    .app
                    .active_session
                    .and_then(|si| self.app.sessions.get(si))
                    .map_or_else(
                        || "Terminal in worktree".to_string(),
                        |s| {
                            format!(
                                "Terminal in {}/{}",
                                s.project,
                                crate::app::path_basename(&s.wt_path)
                            )
                        },
                    );
                let mut content = row![
                    icon_slot("term", c::FG_MUTE()),
                    text(label)
                        .size(13)
                        .color(if active { c::FG() } else { c::FG_DIM() }),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                if active {
                    content = content
                        .push(Space::new().width(Length::Fill))
                        .push(enter_chip());
                }
                modal_list_row_sized(
                    content,
                    active,
                    GMsg::SessionLauncher(Msg::Activate(i)),
                    36.0,
                    6.0,
                    12.0,
                )
            }
            PaletteRow::AddProject => {
                let mut content = row![
                    icon_slot("plus", c::MAGENTA()),
                    text("Add project…").size(13).color(c::MAGENTA()),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                if active {
                    content = content
                        .push(Space::new().width(Length::Fill))
                        .push(enter_chip());
                }
                modal_list_row_sized(
                    content,
                    active,
                    GMsg::SessionLauncher(Msg::Activate(i)),
                    36.0,
                    6.0,
                    12.0,
                )
            }
            PaletteRow::SwitchToSession => {
                // Neutral FG (never magenta — that's reserved for create
                // actions): a "swap sessions" idiom via the restart glyph,
                // plus a tab-hint chip and chevron drill-in affordance.
                // Outside zen the row is visible but inert — forced muted
                // regardless of keyboard highlight, with an "in zen · ⌘⏎"
                // hint (telling the user how to actually reach it) in place
                // of the tab/chevron affordance; Enter/Tab on it are
                // swallowed (see `launcher_activate`/
                // `launcher_enter_row_actions`).
                let switchable = self.switch_to_session_active();
                let label_color = if !switchable {
                    c::FG_MUTE()
                } else if active {
                    c::FG()
                } else {
                    c::FG_DIM()
                };
                let icon_color = if switchable {
                    c::FG_DIM()
                } else {
                    c::FG_MUTE()
                };
                let mut content = row![
                    icon_slot("restart", icon_color),
                    text("Switch to session…").size(13).color(label_color),
                    Space::new().width(Length::Fill),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                if !switchable {
                    content = content.push(
                        text("in zen · ⌘⏎")
                            .font(MONO_FONT)
                            .size(10.5)
                            .color(c::FG_MUTE()),
                    );
                } else if active {
                    content = content.push(enter_chip());
                } else {
                    content = content.push(keycap_text("tab", c::FG_MUTE())).push(icon(
                        "chev-right",
                        12.0,
                        c::FG_MUTE(),
                    ));
                }
                modal_list_row_sized(
                    content,
                    active,
                    GMsg::SessionLauncher(Msg::Activate(i)),
                    36.0,
                    6.0,
                    12.0,
                )
            }
            PaletteRow::Settings => {
                // Unlike the other ACTIONS rows, this one shows no ⏎ chip
                // when selected — Tab (not Enter, though Enter also works;
                // see `launcher_activate`) is the primary gesture into the
                // drill-in, so the selected row surfaces a "tab" keycap
                // instead (B1 in the palette redesign mock).
                let mut content = row![
                    icon_slot("cog", c::FG_MUTE()),
                    text("Settings…")
                        .size(13)
                        .color(if active { c::FG() } else { c::FG_DIM() }),
                    Space::new().width(Length::Fill),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                if active {
                    content = content.push(keycap_text("tab", c::FG_DIM()));
                }
                modal_list_row_sized(
                    content,
                    active,
                    GMsg::SessionLauncher(Msg::Activate(i)),
                    36.0,
                    6.0,
                    12.0,
                )
            }
            PaletteRow::Setting(s) => {
                let content = self.setting_row_content(*s, input);
                launcher_row(
                    content,
                    active,
                    true,
                    GMsg::SessionLauncher(Msg::Activate(i)),
                    PALETTE_ROW_H,
                )
            }
            PaletteRow::ReloadThemes => {
                let mut content = row![
                    icon_slot("restart", c::FG_MUTE()),
                    text("Reload themes").size(13).color(if active {
                        c::FG()
                    } else {
                        c::FG_DIM()
                    }),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                if active {
                    content = content
                        .push(Space::new().width(Length::Fill))
                        .push(enter_chip());
                }
                modal_list_row_sized(
                    content,
                    active,
                    GMsg::SessionLauncher(Msg::Activate(i)),
                    36.0,
                    6.0,
                    12.0,
                )
            }
        }
    }

    /// The strip's primary "Launch session…" row (mock F2/F3): the plain
    /// magenta play/label pair on the left, then — right-aligned — the
    /// ringed agent's own label and a horizontal bar of one small icon
    /// button per installed agent. ⏎ launches with the ringed one; clicking
    /// any button launches with that one outright (`RowActionAgentLaunch`),
    /// and clicking the row's own background launches with whatever is
    /// currently ringed. Nested buttons are fine here: iced's `button`
    /// forwards the event to its content first and bails out when the
    /// content captured it, so an icon button always wins over the row
    /// underneath it.
    fn strip_launch_row(&self, active: bool, agent_sel: usize) -> Element<'_, GMsg> {
        let mut bar = row![].spacing(6).align_y(iced::Alignment::Center);
        for (i, ag) in self.app.available_agents.iter().enumerate() {
            let on = i == agent_sel;
            let glyph_color = if on { c::YELLOW() } else { c::FG_MUTE() };
            bar = bar.push(
                button(
                    container(icon(ag.icon_name(), 15.0, glyph_color))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_x(iced::alignment::Horizontal::Center)
                        .align_y(iced::Alignment::Center),
                )
                .width(AGENT_BTN)
                .height(AGENT_BTN)
                .padding(0)
                .on_press(GMsg::SessionLauncher(Msg::RowActionAgentLaunch(i)))
                .style(move |_, status| {
                    let hovered = matches!(status, button::Status::Hovered);
                    button::Style {
                        background: Some(Background::Color(if on {
                            c::BG_HL()
                        } else if hovered {
                            c::BG_HOVER()
                        } else {
                            c::BG_RAIL()
                        })),
                        text_color: glyph_color,
                        border: Border {
                            color: if on { c::YELLOW() } else { c::BORDER_SOFT() },
                            width: 1.0,
                            radius: Radius::from(6.0),
                        },
                        shadow: Shadow::default(),
                        snap: false,
                    }
                }),
            );
        }
        let sel_label = self
            .app
            .available_agents
            .get(agent_sel)
            .map_or("", |ag| ag.label());
        let content = row![
            container(icon("play", 13.0, c::MAGENTA()))
                .width(20.0)
                .align_x(iced::alignment::Horizontal::Center),
            text("Launch session…").size(12).color(c::MAGENTA()),
            Space::new().width(Length::Fill),
            text(sel_label).font(MONO_FONT).size(12).color(c::FG_DIM()),
            bar,
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        modal_list_row_sized(
            content,
            active,
            GMsg::SessionLauncher(Msg::RowActionPick(0)),
            STRIP_LAUNCH_ROW_H,
            4.0,
            12.0,
        )
    }

    /// The inline contextual-action strip revealed by Tab under a
    /// highlighted `Recent`/`Combo` row: "Launch session…" (magenta, play,
    /// hosting the agent icon bar) and "Delete worktree" (red, trash).
    /// `action` is the currently-selected action within the strip;
    /// `agent_sel` is the ringed agent in the bar (index into
    /// `App::available_agents`), which ⏎ on action 0 launches with.
    /// `is_main` selects the second action:
    /// the project's default/base checkout can't be deleted (`start_delete`
    /// bounces it to a "can't remove the project's main checkout" message),
    /// so its strip offers "Create worktree…" there instead of "Delete
    /// worktree". `0.0` left/right padding here is deliberate — the strip
    /// must render exactly as wide as the highlighted row card above it, and
    /// `modal_list_row_sized`'s own row buttons are already `Length::Fill`
    /// with their own internal `pad_x`, so any outer horizontal padding here
    /// would inset the strip relative to that row. Any configured lifecycle
    /// scripts (setup/run/teardown) are appended after the theme row, via
    /// `row_action_scripts`.
    pub(super) fn palette_row_actions_strip(
        &self,
        proj: usize,
        action: usize,
        agent_sel: usize,
        is_main: bool,
    ) -> Element<'_, GMsg> {
        let icon_slot = |name: &'static str, color: Color| {
            container(icon(name, 13.0, color))
                .width(20.0)
                .align_x(iced::alignment::Horizontal::Center)
        };
        let action_row = |idx: usize, name: &'static str, label: &'static str, color: Color| {
            let active = idx == action;
            let content = row![icon_slot(name, color), text(label).size(12).color(color),]
                .spacing(8)
                .align_y(iced::Alignment::Center);
            modal_list_row_sized(
                content,
                active,
                GMsg::SessionLauncher(Msg::RowActionPick(idx)),
                STRIP_ROW_H,
                4.0,
                12.0,
            )
        };
        let second = if is_main {
            action_row(1, "plus", "Create worktree…", c::MAGENTA())
        } else {
            action_row(1, "trash", "Delete worktree", c::RED())
        };
        let mut rows = column![self.strip_launch_row(action == 0, agent_sel), second].spacing(1);
        if self.app.project_themes_enabled() {
            // "contrast" mirrors `SettingRow::Theme::icon_name()` — the app
            // theme row's own icon, reused here since this is the same idea
            // scoped to one project.
            rows = rows.push(action_row(2, "contrast", "Project theme…", c::CYAN()));
        }
        let base = if self.app.project_themes_enabled() {
            3
        } else {
            2
        };
        for (i, (kind, _)) in self.row_action_scripts(proj).into_iter().enumerate() {
            let (label, color) = match kind {
                "setup" => ("Setup script", c::GREEN()),
                "run" => ("Run script", c::CYAN()),
                "teardown" => ("Teardown script", c::AMBER()),
                _ => continue,
            };
            rows = rows.push(action_row(base + i, "play", label, color));
        }
        container(rows)
            .padding(Padding {
                top: 0.0,
                bottom: 4.0,
                left: 0.0,
                right: 0.0,
            })
            .width(Length::Fill)
            .into()
    }
}
