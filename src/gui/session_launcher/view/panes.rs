//! The palette's two non-settings states, one function each: the default
//! recents/typing/browse-all list (`root_pane`) and the "switch to session"
//! drill-in (`switch_pane`).
//!
//! Each takes the modal's partially-built `body` column (input zone + divider
//! already pushed) and returns it with its own list zone, divider and footer
//! appended — the shell in `super` owns everything around that.

use super::super::state::{Msg, PaletteRow, RowActionsState, SwitchRow};
use crate::gui::icons::icon;
use crate::gui::metrics::{MONO_FONT, UI_FONT};
use crate::gui::palette as c;
use crate::gui::state::{Grove, Msg as GMsg};
use crate::gui::update::SettingRow;
use crate::gui::view::{cap, launcher_palette_scrollable_id};
use crate::gui::widgets::{
    divider_h, footer_container, footer_hint, ghost_scrollable, keycap_text, launcher_row,
    section_header, PALETTE_ROW_H,
};
use iced::border::Radius;
use iced::widget::{column, container, row, stack, text, Column, Space};
use iced::{Background, Border, Color, Element, Length, Padding};

/// The inline warning under a *selected* Permissions row (B3) — the
/// same string E1's Permissions pane promotes, one shade up from a
/// throwaway caption (11 · FG_DIM), left-padded past the 24px icon
/// slot so it aligns with the row's label column. Shared by the
/// drill-in Root list and the root/typing direct-match list.
pub(super) fn danger_caption<'a>() -> Element<'a, GMsg> {
    container(
        text("Skip lets agents run any command without asking.")
            .size(11)
            .color(c::FG_DIM()),
    )
    .padding(Padding {
        top: 4.0,
        bottom: 2.0,
        left: 44.0,
        right: 12.0,
    })
    .into()
}

impl Grove {
    /// Default state: recents + actions on an empty input, every
    /// project×worktree combo fuzzy-filtered once you type or browse all.
    pub(super) fn root_pane<'a>(
        &'a self,
        mut body: Column<'a, GMsg>,
        input: &'a str,
        selected: usize,
        browse_all: bool,
        row_actions: Option<&'a RowActionsState>,
    ) -> Column<'a, GMsg> {
        let rows = self.palette_rows(input, browse_all);
        // Must match `palette_rows`' guard: all-archived reads as empty.
        let zero_projects = self.app.store.active_projects().next().is_none();
        let root_mode = input.is_empty() && !browse_all && !zero_projects;

        let list_zone: Element<'a, GMsg> = if rows.is_empty() {
            container(text("No matches").size(12).color(c::FG_MUTE()))
                .padding(Padding::from([30, 16]))
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .into()
        } else {
            let mut list = Column::new().spacing(2);
            let mut printed_recent = false;
            let mut printed_actions = false;
            // Typed list only: settings matches sort first (see
            // `palette_rows`), and their presence labels the two groups —
            // SETTINGS above, SESSIONS below (B2 in the palette redesign
            // mock). Sessions and actions each always get a header now
            // (previously SESSIONS/ACTIONS only appeared when a settings
            // match was also present).
            let has_settings = rows.iter().any(|r| matches!(r, PaletteRow::Setting(_)));
            let mut printed_settings = false;
            let mut printed_sessions = false;
            // Root-only: when the recents list is empty, `palette_rows`
            // falls back to a worktree listing grouped by project — the
            // presence of a `Combo` row in root state is the signal (root
            // state otherwise only ever holds `Recent` + action rows).
            let mut last_wt_project: Option<usize> = None;
            // Typed/browse-all only: `palette_rows` already reordered
            // session rows into per-project runs above this same
            // threshold — recompute it here (never trust a flag) so the
            // headers can't disagree with the actual row order.
            let session_project_order: Vec<usize> = {
                let mut seen = Vec::new();
                for r in &rows {
                    if let PaletteRow::Recent { proj, .. } | PaletteRow::Combo { proj, .. } = r {
                        if !seen.contains(proj) {
                            seen.push(*proj);
                        }
                    }
                }
                seen
            };
            let session_row_count = rows
                .iter()
                .filter(|r| matches!(r, PaletteRow::Recent { .. } | PaletteRow::Combo { .. }))
                .count();
            let grouped_by_project =
                !root_mode && (session_project_order.len() > 2 || session_row_count > 10);
            let mut last_grouped_project: Option<usize> = None;
            for (i, row) in rows.iter().enumerate() {
                if root_mode {
                    match row {
                        PaletteRow::Recent { .. } => {
                            if !printed_recent {
                                list = list.push(section_header("RECENT", 0.0, 6.0));
                                printed_recent = true;
                            }
                        }
                        PaletteRow::Combo { proj, .. } => {
                            if last_wt_project != Some(*proj) {
                                let top = if i == 0 { 0.0 } else { 12.0 };
                                let pname = self
                                    .app
                                    .store
                                    .projects
                                    .get(*proj)
                                    .map(|p| p.name.to_uppercase())
                                    .unwrap_or_default();
                                list = list.push(section_header(
                                    &format!("{pname} — WORKTREES"),
                                    top,
                                    6.0,
                                ));
                                last_wt_project = Some(*proj);
                            }
                        }
                        _ => {
                            if !printed_actions {
                                let top = if printed_recent || last_wt_project.is_some() {
                                    12.0
                                } else {
                                    0.0
                                };
                                list = list.push(section_header("ACTIONS", top, 6.0));
                                printed_actions = true;
                            }
                        }
                    }
                } else {
                    let is_setting = matches!(row, PaletteRow::Setting(_));
                    let is_session =
                        matches!(row, PaletteRow::Recent { .. } | PaletteRow::Combo { .. });
                    if has_settings && is_setting && !printed_settings {
                        list = list.push(section_header("SETTINGS", 0.0, 6.0));
                        printed_settings = true;
                    } else if is_session && !printed_sessions {
                        let top = if printed_settings { 12.0 } else { 0.0 };
                        list = list.push(section_header("SESSIONS", top, 6.0));
                        printed_sessions = true;
                    } else if !is_setting && !is_session && !printed_actions {
                        let top = if printed_sessions || printed_settings {
                            12.0
                        } else {
                            0.0
                        };
                        list = list.push(section_header("ACTIONS", top, 6.0));
                        printed_actions = true;
                    }
                    if grouped_by_project && is_session {
                        let proj = match row {
                            PaletteRow::Recent { proj, .. } | PaletteRow::Combo { proj, .. } => {
                                *proj
                            }
                            _ => unreachable!(),
                        };
                        if last_grouped_project != Some(proj) {
                            let pname = self
                                .app
                                .store
                                .projects
                                .get(proj)
                                .map(|p| p.name.to_uppercase())
                                .unwrap_or_default();
                            let top = if last_grouped_project.is_none() {
                                0.0
                            } else {
                                8.0
                            };
                            list = list.push(section_header(&pname, top, 4.0));
                            last_grouped_project = Some(proj);
                        }
                    }
                }
                list = list.push(self.palette_row_view(i, row, i == selected, input, root_mode));
                // Danger settings warn inline in the direct-match list
                // too (B3), before the user ever drills in.
                if i == selected && matches!(row, PaletteRow::Setting(SettingRow::Permissions)) {
                    list = list.push(danger_caption());
                }
                let row_identity = match row {
                    PaletteRow::Recent {
                        proj,
                        wt_path,
                        agent,
                        ..
                    }
                    | PaletteRow::Combo {
                        proj,
                        wt_path,
                        agent,
                        ..
                    } => Some((*proj, wt_path.as_str(), *agent)),
                    _ => None,
                };
                if let (Some((rp, rw, rag)), Some(ra)) = (row_identity, row_actions) {
                    if rp == ra.proj && rw == ra.wt_path && rag == ra.agent {
                        let is_main = self
                            .launcher_worktrees(rp)
                            .iter()
                            .find(|w| w.path == rw)
                            .is_some_and(|w| w.is_main);
                        list = list.push(self.palette_row_actions_strip(
                            ra.proj,
                            ra.action,
                            ra.agent_sel,
                            is_main,
                        ));
                    }
                }
            }
            container(
                ghost_scrollable(list)
                    .id(launcher_palette_scrollable_id())
                    .height(Length::Shrink),
            )
            .padding(8)
            .max_height(380.0)
            .width(Length::Fill)
            .into()
        };
        body = body.push(list_zone);
        body = body.push(divider_h(c::BORDER_SOFT()));
        body = body.push(if let Some(ra) = row_actions {
            // Row-actions strip open: the footer reflects that sub-state
            // directly rather than the underlying highlighted row — ⏎
            // runs the selected strip action (e.g. "Delete worktree"),
            // not "open/launch". ←→ only mean "agent" on the strip's
            // "Launch session…" row, which hosts the agent icon bar, so
            // the hint only shows there.
            let mut hints = row![footer_hint("↑↓", "action")].spacing(14);
            if ra.action == 0 {
                hints = hints.push(footer_hint("←→", "agent"));
                hints = hints.push(footer_hint("⏎", "launch"));
            } else {
                hints = hints.push(footer_hint("⏎", "run"));
            }
            hints = hints.push(footer_hint("esc", "back"));
            footer_container(hints.into())
        } else {
            // Recent/Combo (project/worktree) rows expose the
            // tab->actions strip; settings rows get their own ⏎ verb
            // (toggle / open) with no tab hint at all, since tab just
            // mirrors enter there; every other row keeps the plain
            // tab->options hint.
            let highlighted = rows.get(selected);
            let highlighted_is_row = matches!(
                highlighted,
                Some(PaletteRow::Recent { .. } | PaletteRow::Combo { .. })
            );
            let setting_enter_label: Option<&'static str> = match highlighted {
                Some(PaletteRow::Setting(s)) => Some(if s.is_toggle() { "toggle" } else { "open" }),
                Some(PaletteRow::Settings) => Some("open"),
                _ => None,
            };
            // Row-highlighted footer orders tab before ⏎ ("navigate · tab
            // actions · ⏎ open/launch · close"); every other state keeps
            // ⏎ before tab ("navigate · ⏎ launch · tab options · close")
            // — matches the palette redesign mock's D1 vs. D2/D3 ordering.
            let mid: Element<'a, GMsg> = if let Some(enter_label) = setting_enter_label {
                row![footer_hint("⏎", enter_label)].spacing(14).into()
            } else if highlighted_is_row {
                row![
                    footer_hint("tab", "actions"),
                    footer_hint("⏎", "open/launch"),
                ]
                .spacing(14)
                .into()
            } else {
                row![footer_hint("⏎", "launch"), footer_hint("tab", "options"),]
                    .spacing(14)
                    .into()
            };
            footer_container(
                row![
                    footer_hint("↑↓", "navigate"),
                    mid,
                    footer_hint("esc", "close"),
                ]
                .spacing(14)
                .into(),
            )
        });
        body
    }

    /// "Switch to session" drill-in: every active session across
    /// every project/worktree, then every home terminal under its own
    /// TERMINALS header. Waiting sessions keep the sidebar's
    /// amber tint/left bar; the currently-focused session/terminal's icon
    /// renders yellow (same idiom as the recents' active-row accent).
    pub(super) fn switch_pane<'a>(
        &'a self,
        mut body: Column<'a, GMsg>,
        input: &'a str,
        sel: usize,
    ) -> Column<'a, GMsg> {
        let rows = self.switch_rows(input);
        let list_zone: Element<'a, GMsg> = if rows.is_empty() {
            container(text("No matching sessions").size(12).color(c::FG_MUTE()))
                .padding(Padding::from([30, 16]))
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .into()
        } else {
            let mut list = Column::new().spacing(2);
            // Each group labels itself the first time it appears, so a
            // query that filters one of them away drops its header too.
            let mut printed_sessions = false;
            let mut printed_terminals = false;
            for (i, &switch_row) in rows.iter().enumerate() {
                let highlighted = i == sel;
                let si = match switch_row {
                    SwitchRow::Session(si) => si,
                    SwitchRow::Terminal(ti) => {
                        let Some(t) = self.app.home_terminals.get(ti) else {
                            continue;
                        };
                        if !printed_terminals {
                            let top = if i == 0 { 0.0 } else { 12.0 };
                            list = list.push(section_header("TERMINALS", top, 6.0));
                            printed_terminals = true;
                        }
                        let is_active =
                            self.terminal_focused && self.app.active_terminal == Some(ti);
                        let icon_color = if is_active { c::YELLOW() } else { c::FG_MUTE() };
                        let title_color = if highlighted { c::FG() } else { c::FG_DIM() };
                        let icon_slot = container(icon("term", 16.0, icon_color))
                            .width(24.0)
                            .align_x(iced::alignment::Horizontal::Center);
                        let mut content = row![
                            icon_slot,
                            column![
                                text(t.label.clone())
                                    .font(UI_FONT)
                                    .size(13)
                                    .color(title_color),
                                text("home terminal")
                                    .font(MONO_FONT)
                                    .size(10.5)
                                    .color(c::FG_MUTE()),
                            ]
                            .spacing(2),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center);
                        if highlighted {
                            content = content
                                .push(Space::new().width(Length::Fill))
                                .push(keycap_text("⏎", c::FG_DIM()));
                        }
                        list = list.push(launcher_row(
                            content,
                            highlighted,
                            true,
                            GMsg::SessionLauncher(Msg::SwitchTerminalPick(ti)),
                            PALETTE_ROW_H,
                        ));
                        continue;
                    }
                };
                let Some(s) = self.app.sessions.get(si) else {
                    continue;
                };
                if !printed_sessions {
                    list = list.push(section_header("SESSIONS", 0.0, 6.0));
                    printed_sessions = true;
                }
                let waiting = matches!(
                    self.activity_state(s),
                    crate::gui::activity::ActivityState::WaitingForInput
                );
                let is_active = self.app.active_session == Some(si);
                let icon_color = if is_active { c::YELLOW() } else { c::FG_MUTE() };
                let label = if waiting {
                    format!("{} (waiting)", cap(s.agent.label()))
                } else {
                    cap(s.agent.label())
                };
                let subtitle = format!("{} / {}", s.project, crate::app::path_basename(&s.wt_path));
                let icon_slot = container(icon(s.agent.icon_name(), 16.0, icon_color))
                    .width(24.0)
                    .align_x(iced::alignment::Horizontal::Center);
                let title_color = if highlighted { c::FG() } else { c::FG_DIM() };
                let mut content = row![
                    icon_slot,
                    column![
                        text(label).font(UI_FONT).size(13).color(title_color),
                        text(subtitle)
                            .font(MONO_FONT)
                            .size(10.5)
                            .color(c::FG_MUTE()),
                    ]
                    .spacing(2),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                if highlighted {
                    content = content
                        .push(Space::new().width(Length::Fill))
                        .push(keycap_text("⏎", c::FG_DIM()));
                }
                let row_el = launcher_row(
                    content,
                    highlighted,
                    true,
                    GMsg::SessionLauncher(Msg::SwitchSessionPick(si)),
                    PALETTE_ROW_H,
                );
                // Waiting sessions keep the sidebar's amber tint + 3px
                // left accent bar, same idiom as `rows.rs`'s waiting row.
                let row_el = if waiting {
                    let tint = Color {
                        a: 0.12,
                        ..c::AMBER()
                    };
                    let bar: Element<'a, GMsg> = container(
                        container(Space::new().width(3.0))
                            .width(3.0)
                            .height(Length::Fill)
                            .style(|_| container::Style {
                                background: Some(Background::Color(c::AMBER())),
                                ..Default::default()
                            }),
                    )
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::Alignment::Start)
                    .into();
                    container(stack![row_el, bar])
                        .height(PALETTE_ROW_H)
                        .width(Length::Fill)
                        .style(move |_| container::Style {
                            background: Some(Background::Color(tint)),
                            border: Border {
                                color: Color::TRANSPARENT,
                                width: 0.0,
                                radius: Radius::from(6.0),
                            },
                            ..Default::default()
                        })
                        .into()
                } else {
                    row_el
                };
                list = list.push(row_el);
            }
            container(ghost_scrollable(list).height(Length::Shrink))
                .padding(8)
                .max_height(380.0)
                .width(Length::Fill)
                .into()
        };
        body = body.push(list_zone);
        body = body.push(divider_h(c::BORDER_SOFT()));
        body = body.push(footer_container(
            row![
                footer_hint("↑↓", "choose"),
                footer_hint("⏎", "switch"),
                footer_hint("esc", "back"),
            ]
            .spacing(14)
            .into(),
        ));
        body
    }
}
