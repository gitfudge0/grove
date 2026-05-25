//! `Grove::view` and the chrome it composes (appbar, sidebar, workspace,
//! statusbar, modal layer). Pure rendering — no state mutation.

use super::metrics::{
    APPBAR_H, CELL_H, CELL_W, MONO_BOLD, MONO_FONT, RAIL_W, ROW_H, SESSBAR_H, STATUS_H, SUBTITLE_H,
};
use super::palette as c;
use super::pty::{rebuild_row_runs, PtyProgram};
use super::rows::{project_row, session_row, worktree_row};
use super::state::{Grove, Msg, PtyCacheEntry};
use super::widgets::{
    divider_h, divider_v, dot, empty_workspace, icon_btn, modal_action, modal_dir_row, modal_panel,
    seg_button, sidebar_agent_menu_overlay, tool_btn, vline,
};
use crate::app::{InputKind, Modal};
use crate::git::Worktree;
use crate::session::{Session, SessionStatus};
use iced::border::Radius;
use iced::widget::{
    button, canvas as canvas_widget, column, container, row, scrollable, stack, text, Column, Space,
};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};
use std::sync::atomic::Ordering;

/// Stable id for the theme-picker scrollable, used to scroll the active
/// selection into view from `update`.
pub fn theme_picker_scrollable_id() -> scrollable::Id {
    scrollable::Id::new("theme-picker-list")
}
use std::sync::Arc;

fn session_context_title(s: &Session) -> Option<String> {
    s.current_title()
        .filter(|t| !t.eq_ignore_ascii_case(&s.label) && !t.eq_ignore_ascii_case(s.agent.label()))
}

fn is_in_progress_title(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    lower.contains("in progress") || lower.contains("in-progress") || lower.contains("in_progress")
}

impl Grove {
    pub fn view(&self) -> Element<'_, Msg> {
        let body = column![
            self.appbar(),
            row![self.sidebar(), divider_v(c::BORDER()), self.workspace()]
                .height(Length::Fill)
                .width(Length::Fill),
            self.statusbar(),
        ]
        .width(Length::Fill)
        .height(Length::Fill);

        let content: Element<'_, Msg> = if matches!(self.app.modal, Modal::None) {
            body.into()
        } else {
            stack![body, self.modal_layer()]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };

        container(content)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG())),
                text_color: Some(c::FG()),
                ..Default::default()
            })
            .into()
    }

    // ── appbar ────────────────────────────────────────────────────────────
    fn appbar(&self) -> Element<'_, Msg> {
        let brand = row![text("grove").font(MONO_BOLD).size(14).color(c::MAGENTA()),]
            .spacing(8)
            .padding(Padding::from([0, 16]))
            .align_y(iced::Alignment::Center);

        let seg = container(
            row![
                seg_button("native", !self.app.use_tmux(), Msg::BackendNative),
                seg_button("tmux", self.app.use_tmux(), Msg::BackendTmux),
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

        let right = row![seg, icon_btn("cog", Msg::OpenThemePicker),]
            .spacing(4)
            .padding(Padding::from([0, 16]))
            .align_y(iced::Alignment::Center);

        let inner = row![
            container(brand).width(RAIL_W),
            Space::with_width(Length::Fill),
            right,
        ]
        .align_y(iced::Alignment::Center)
        .height(Length::Fill);

        let bar = container(inner)
            .height(APPBAR_H)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP())),
                ..Default::default()
            });

        column![bar, divider_h(c::BORDER())].into()
    }

    // ── sidebar ───────────────────────────────────────────────────────────
    fn sidebar(&self) -> Element<'_, Msg> {
        let tree = self.tree_view();
        let tree_area = container(scrollable(tree).height(Length::Fill))
            .height(Length::Fill)
            .padding(Padding {
                top: 8.0,
                bottom: 12.0,
                left: 0.0,
                right: 0.0,
            });
        let tree_layer: Element<'_, Msg> = match self.open_agent_menu_top() {
            Some((proj, wt, top, is_main)) => stack![
                tree_area,
                sidebar_agent_menu_overlay(proj, wt, top, is_main),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
            None => tree_area.into(),
        };

        let add_proj = container(
            button(
                container(text("+ add project").size(12).color(c::FG_DIM()))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            )
            .on_press(Msg::AddProject)
            .width(Length::Fill)
            .height(28.0)
            .style(|_, _| button::Style {
                background: None,
                text_color: c::FG_DIM(),
                border: Border {
                    color: c::BORDER(),
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                shadow: Shadow::default(),
            }),
        )
        .padding(Padding::from([12, 12]));

        let stack_col =
            column![tree_layer, divider_h(c::BORDER_SOFT()), add_proj,].height(Length::Fill);

        container(stack_col)
            .width(RAIL_W)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_RAIL())),
                ..Default::default()
            })
            .into()
    }

    fn tree_view(&self) -> Element<'_, Msg> {
        let mut col: Column<'_, Msg> = Column::new();
        let projects: Vec<_> = self
            .app
            .store
            .projects
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p.name.clone()))
            .collect();
        for (pi, pname) in projects {
            let expanded = !self.collapsed.contains(&pi);
            let count = self
                .app
                .sessions
                .iter()
                .filter(|s| s.project == pname)
                .count();
            col = col.push(project_row(pi, &pname, count, expanded));

            if !expanded {
                continue;
            }
            let wts: &[Worktree] = if pi == self.app.proj_idx {
                &self.app.worktrees
            } else {
                self.wt_cache.get(&pi).map(|v| v.as_slice()).unwrap_or(&[])
            };
            for (wi, w) in wts.iter().enumerate() {
                let wname = if w.is_main {
                    pname.clone()
                } else {
                    crate::app::path_basename(&w.path)
                };
                let active_wt = pi == self.app.proj_idx && wi == self.app.wt_idx;
                col = col.push(worktree_row(
                    pi, wi, &wname, &w.branch, active_wt, w.is_main,
                ));

                for (si, s) in self.app.sessions.iter().enumerate() {
                    if s.wt_path == w.path {
                        let active = self.app.active_session == Some(si);
                        let pending_kill = self.pending_kill == Some(si);
                        col = col.push(session_row(si, s, active, pending_kill));
                    }
                }
            }
        }
        col.into()
    }

    /// Find the y-pixel offset of the open agent menu, if any, so the overlay
    /// can be positioned. Walks the tree in the same order `tree_view` does.
    fn open_agent_menu_top(&self) -> Option<(usize, usize, f32, bool)> {
        let (open_proj, open_wt) = self.open_agent_menu?;
        let mut acc_y: f32 = 0.0;

        for (pi, pname) in self
            .app
            .store
            .projects
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p.name.as_str()))
        {
            acc_y += ROW_H; // project row
            if self.collapsed.contains(&pi) {
                continue;
            }

            let wts: &[Worktree] = if pi == self.app.proj_idx {
                &self.app.worktrees
            } else {
                self.wt_cache.get(&pi).map(|v| v.as_slice()).unwrap_or(&[])
            };

            for (wi, w) in wts.iter().enumerate() {
                if pi == open_proj && wi == open_wt {
                    return Some((pi, wi, 6.0 + acc_y + ROW_H, w.is_main));
                }
                acc_y += ROW_H;

                for s in &self.app.sessions {
                    if s.project == pname && s.wt_path == w.path {
                        let has_sub = s
                            .current_title()
                            .filter(|t| {
                                !t.eq_ignore_ascii_case(&s.label)
                                    && !t.eq_ignore_ascii_case(s.agent.label())
                            })
                            .is_some();
                        acc_y += ROW_H + if has_sub { SUBTITLE_H } else { 0.0 };
                    }
                }
            }
        }

        None
    }

    // ── workspace ─────────────────────────────────────────────────────────
    fn workspace(&self) -> Element<'_, Msg> {
        let inner: Element<'_, Msg> = match self.app.active_session {
            Some(i) if i < self.app.sessions.len() => column![
                self.sess_bar(&self.app.sessions[i]),
                self.pty(&self.app.sessions[i]),
            ]
            .height(Length::Fill)
            .into(),
            _ => empty_workspace(),
        };

        container(inner)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG())),
                ..Default::default()
            })
            .into()
    }

    fn sess_bar(&self, s: &Session) -> Element<'_, Msg> {
        let running = matches!(*s.status.lock().unwrap(), SessionStatus::Running);
        let (dot_color, label) = if running {
            (c::GREEN(), "running")
        } else {
            (c::FG_MUTE(), "exited")
        };
        let context = session_context_title(s);
        let show_progress = running
            && context
                .as_deref()
                .map(is_in_progress_title)
                .unwrap_or(false);
        let sess_text = |content: String, color: Color| {
            text(content)
                .font(MONO_FONT)
                .size(12)
                .line_height(1.0)
                .height(18)
                .align_y(iced::alignment::Vertical::Center)
                .color(color)
        };

        let status: Element<'_, Msg> =
            row![dot(dot_color), sess_text(label.to_string(), dot_color),]
                .spacing(6)
                .align_y(iced::Alignment::Center)
                .into();

        let mut identity = row![
            sess_text(s.agent.label().to_string(), c::MAGENTA()),
            sess_text("·".to_string(), c::FG_MUTE()),
            sess_text(s.project.clone(), c::BLUE()),
            sess_text("/".to_string(), c::FG_MUTE()),
            sess_text(s.label.clone(), c::FG()),
            sess_text(format!("[{}]", s.branch), c::FG_MUTE()),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        if let Some(title) = context {
            let session_context: Element<'_, Msg> = if show_progress {
                let phase = ((self.blink_tick / 5) % 3) as usize;
                let step_dot = |i| dot(if i == phase { c::GREEN() } else { c::FG_MUTE() });
                row![
                    step_dot(0),
                    step_dot(1),
                    step_dot(2),
                    sess_text("in progress".to_string(), c::GREEN()),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center)
                .into()
            } else {
                sess_text(title, c::FG_MUTE()).into()
            };
            identity = identity
                .push(sess_text("·".to_string(), c::FG_MUTE()))
                .push(session_context);
        }

        let bar = row![
            status,
            vline(),
            container(identity).width(Length::Fill).clip(true),
            sess_text(s.wt_path.clone(), c::FG_MUTE()),
            vline(),
            tool_btn(
                "trash",
                "kill",
                true,
                Msg::KillSession(self.app.active_session.unwrap_or(0)),
            ),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center)
        .height(Length::Fill)
        .padding(Padding::from([0, 16]));

        let bar_container = container(bar)
            .height(SESSBAR_H)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP())),
                ..Default::default()
            });

        column![bar_container, divider_h(c::BORDER_SOFT())].into()
    }

    fn pty(&self, s: &Session) -> Element<'_, Msg> {
        // Per-session row snapshot + canvas cache. Switching to a quiet
        // session returns the cached geometry with zero draw work; switching
        // to a session that produced output re-snaps the rows and clears the
        // canvas cache, then draws once.
        let key = Arc::as_ptr(&s.dirty) as usize;
        let (rows, cache, cursor_pos) = {
            let mut map = self.pty_cache.borrow_mut();
            let entry = map.entry(key);
            let needs_rebuild = match &entry {
                std::collections::hash_map::Entry::Occupied(_) => {
                    s.dirty.swap(false, Ordering::Relaxed)
                }
                std::collections::hash_map::Entry::Vacant(_) => {
                    s.dirty.store(false, Ordering::Relaxed);
                    true
                }
            };
            let entry = entry.or_insert_with(|| PtyCacheEntry {
                rows: Arc::new(Vec::new()),
                cache: Arc::new(iced::widget::canvas::Cache::default()),
                cursor_pos: None,
            });
            if needs_rebuild {
                let parser = s.parser.lock().unwrap();
                let screen = parser.screen();
                let (h, w) = screen.size();
                let mut new_rows = Vec::with_capacity(h as usize);
                for r in 0..h {
                    new_rows.push(rebuild_row_runs(screen, r, w));
                }
                entry.rows = Arc::new(new_rows);
                entry.cache.clear();
                entry.cursor_pos = if screen.hide_cursor() {
                    None
                } else {
                    Some(screen.cursor_position())
                };
            }
            (
                Arc::clone(&entry.rows),
                Arc::clone(&entry.cache),
                entry.cursor_pos,
            )
        };

        let rows_len = rows.len() as f32;
        let cols = rows
            .first()
            .map(|r| r.iter().map(|run| run.text.chars().count()).sum::<usize>())
            .unwrap_or(0) as f32;
        // Cursor blinks at ~500 ms on / 500 ms off (tick interval = 60 ms,
        // so 8–9 ticks per half-period; use mod 16 with threshold 8).
        let cursor_visible = self.blink_tick % 16 < 8;
        let program = PtyProgram {
            rows,
            cache,
            selection: self.pty_selection,
            cursor: cursor_pos,
            cursor_visible,
        };
        let body: Element<'_, Msg> = canvas_widget(program)
            .width(Length::Fixed((cols * CELL_W).max(CELL_W)))
            .height(Length::Fixed((rows_len * CELL_H).max(CELL_H)))
            .into();

        container(scrollable(body).width(Length::Fill).height(Length::Fill))
            .padding(Padding::from([12, 16]))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG())),
                ..Default::default()
            })
            .into()
    }

    // ── status bar ────────────────────────────────────────────────────────
    fn statusbar(&self) -> Element<'_, Msg> {
        let running = self
            .app
            .sessions
            .iter()
            .filter(|s| matches!(*s.status.lock().unwrap(), SessionStatus::Running))
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

        let left = row![
            row![
                dot(if running > 0 {
                    c::GREEN()
                } else {
                    c::FG_MUTE()
                }),
                text(format!("{running} running"))
                    .size(11)
                    .color(c::FG_DIM()),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
            row![
                text("backend").size(11).color(c::FG_MUTE()),
                text(backend).size(11).color(c::FG_DIM()),
            ]
            .spacing(6),
            row![
                text("theme").size(11).color(c::FG_MUTE()),
                text(theme_name).size(11).color(c::FG_DIM()),
            ]
            .spacing(6),
        ]
        .spacing(16)
        .align_y(iced::Alignment::Center);

        let toast: Element<'_, Msg> = match &self.app.toast {
            Some(t) => text(t.message.clone()).size(11).color(c::GREEN()).into(),
            None => Space::with_width(0).into(),
        };

        let right = row![text(format!("v{}", env!("CARGO_PKG_VERSION")))
            .size(11)
            .color(c::FG_DIM()),];

        let bar = row![
            left,
            Space::with_width(24),
            toast,
            Space::with_width(Length::Fill),
            right,
        ]
        .padding(Padding::from([0, 16]))
        .align_y(iced::Alignment::Center)
        .height(Length::Fill);

        container(bar)
            .height(STATUS_H)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP())),
                ..Default::default()
            })
            .into()
    }

    // ── modal layer ───────────────────────────────────────────────────────
    fn modal_layer(&self) -> Element<'_, Msg> {
        let panel: Element<'_, Msg> = match &self.app.modal {
            Modal::Input {
                title,
                buffer,
                kind,
                dir_sel,
            } => self.input_modal(title, buffer, kind, *dir_sel),
            Modal::Confirm {
                title,
                prompt,
                destructive,
                ..
            } => self.confirm_modal(title, prompt, *destructive),
            Modal::Message(message) => self.message_modal(message),
            Modal::TmuxChoice => self.tmux_choice_modal(),
            Modal::ThemePicker {
                sel_dark,
                sel_light,
                tab,
                ..
            } => self.theme_picker_modal(*sel_dark, *sel_light, *tab),
            _ => Space::with_width(0).into(),
        };

        container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.16))),
                ..Default::default()
            })
            .into()
    }

    fn input_modal<'a>(
        &'a self,
        title: &'a str,
        buffer: &'a str,
        kind: &'a InputKind,
        dir_sel: usize,
    ) -> Element<'a, Msg> {
        let show_dirs = matches!(kind, InputKind::AddProjectPath);
        let entries = if show_dirs {
            crate::app::list_dirs(buffer)
        } else {
            Vec::new()
        };
        let visible_matches = if show_dirs {
            entries.len().clamp(1, 6)
        } else {
            0
        };
        let modal_h = if show_dirs {
            192.0 + (visible_matches as f32 * ROW_H)
        } else {
            180.0
        };
        let modal_w = if show_dirs { 640.0 } else { 480.0 };

        let input = container(
            row![
                text(buffer.to_string())
                    .font(MONO_FONT)
                    .size(13)
                    .color(c::FG())
                    .wrapping(iced::widget::text::Wrapping::None),
                container(Space::with_width(7))
                    .width(7)
                    .height(15)
                    .style(|_| container::Style {
                        background: Some(Background::Color(c::CYAN())),
                        ..Default::default()
                    }),
            ]
            .spacing(1)
            .align_y(iced::Alignment::Center),
        )
        .height(36)
        .width(Length::Fill)
        .align_y(iced::Alignment::Center)
        .padding(Padding::from([0, 12]))
        .clip(true)
        .style(|_| container::Style {
            background: Some(Background::Color(c::BG_STRIP())),
            border: Border {
                color: c::BORDER(),
                width: 1.0,
                radius: Radius::from(4.0),
            },
            ..Default::default()
        });

        let mut body =
            column![text(title.to_string()).size(13).color(c::MAGENTA()), input,].spacing(12);

        if show_dirs {
            let mut matches_col = Column::new()
                .spacing(0)
                .height(Length::Fixed(visible_matches as f32 * ROW_H));
            if entries.is_empty() {
                matches_col = matches_col.push(
                    container(text("no matches").size(12).color(c::FG_MUTE()))
                        .height(ROW_H)
                        .align_y(iced::Alignment::Center),
                );
            } else {
                for (i, path) in entries.into_iter().take(6).enumerate() {
                    matches_col = matches_col.push(modal_dir_row(path, i == dir_sel));
                }
            }

            body = body
                .push(text("matches").size(11).color(c::FG_MUTE()))
                .push(matches_col);
        }

        body = body.push(Space::with_height(8)).push(
            row![
                Space::with_width(Length::Fill),
                modal_action("cancel", false, Msg::ModalCancel),
                modal_action("submit", true, Msg::ModalSubmit),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        );

        modal_panel(body.into(), modal_w, modal_h, c::MAGENTA())
    }

    fn confirm_modal<'a>(
        &'a self,
        title: &'a str,
        prompt: &'a str,
        destructive: bool,
    ) -> Element<'a, Msg> {
        let accent = if destructive { c::RED() } else { c::MAGENTA() };
        let body = column![
            text(title.to_string()).size(13).color(accent),
            text(prompt.to_string())
                .size(13)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::Word),
            Space::with_height(8),
            row![
                Space::with_width(Length::Fill),
                modal_action("cancel", false, Msg::ModalConfirm(false)),
                modal_action(
                    if destructive { "remove" } else { "confirm" },
                    true,
                    Msg::ModalConfirm(true)
                ),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        modal_panel(body.into(), 480.0, 180.0, accent)
    }

    fn message_modal<'a>(&'a self, message: &'a str) -> Element<'a, Msg> {
        let body = column![
            text("notice").size(13).color(c::CYAN()),
            text(message.to_string())
                .size(13)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::Word),
            Space::with_height(8),
            row![
                Space::with_width(Length::Fill),
                modal_action("close", true, Msg::ModalCancel),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        modal_panel(body.into(), 480.0, 180.0, c::CYAN())
    }

    fn tmux_choice_modal(&self) -> Element<'_, Msg> {
        let body = column![
            text("session backend").size(13).color(c::CYAN()),
            text("Use tmux for new sessions? Existing sessions keep their current backend.")
                .size(13)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::Word),
            Space::with_height(8),
            row![
                Space::with_width(Length::Fill),
                modal_action("native", false, Msg::ChooseTmux(false)),
                modal_action("tmux", true, Msg::ChooseTmux(true)),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        modal_panel(body.into(), 480.0, 180.0, c::CYAN())
    }

    fn theme_picker_modal(
        &self,
        sel_dark: usize,
        sel_light: usize,
        tab: crate::theme::ThemeKind,
    ) -> Element<'_, Msg> {
        let themes = crate::theme::themes_of(tab);
        let sel = match tab {
            crate::theme::ThemeKind::Dark => sel_dark,
            crate::theme::ThemeKind::Light => sel_light,
        };

        let tab_pill = |label: &'static str, active: bool, msg: Msg| -> Element<'_, Msg> {
            button(text(label).size(11))
                .on_press(msg)
                .padding(Padding::from([4, 12]))
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
                        text_color: if active { c::MAGENTA() } else { c::FG_MUTE() },
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: Radius::from(3.0),
                        },
                        shadow: Shadow::default(),
                    }
                })
                .into()
        };

        let tabs = row![
            tab_pill(
                "Dark",
                matches!(tab, crate::theme::ThemeKind::Dark),
                Msg::ThemePickerSwitchTab,
            ),
            tab_pill(
                "Light",
                matches!(tab, crate::theme::ThemeKind::Light),
                Msg::ThemePickerSwitchTab,
            ),
        ]
        .spacing(6);

        let mut list = Column::new().spacing(0);
        for (i, th) in themes.iter().enumerate() {
            let active = i == sel;
            let name = th.name.to_string();
            list = list.push(
                button(
                    container(text(name).size(12).color(if active {
                        c::FG()
                    } else {
                        c::FG_DIM()
                    }))
                    .width(Length::Fill)
                    .center_y(ROW_H)
                    .padding(Padding::from([0, 10])),
                )
                .on_press(Msg::ThemePickerSelect(i))
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
                        border: Border::default(),
                        shadow: Shadow::default(),
                    }
                }),
            );
        }

        let list_h = (themes.len().min(12) as f32) * ROW_H;
        let scroller = container(scrollable(list).id(theme_picker_scrollable_id()))
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
            });

        let body = column![
            text("theme").size(13).color(c::MAGENTA()),
            tabs,
            scroller,
            Space::with_height(4),
            row![
                Space::with_width(Length::Fill),
                modal_action("cancel", false, Msg::ThemePickerCancel),
                modal_action("apply", true, Msg::ThemePickerSubmit),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        let modal_h = 140.0 + list_h;
        modal_panel(body.into(), 460.0, modal_h, c::MAGENTA())
    }
}
