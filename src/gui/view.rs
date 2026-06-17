//! `Grove::view` and the chrome it composes (appbar, sidebar, workspace,
//! statusbar, modal layer). Pure rendering — no state mutation.

use super::icons::icon;
use super::metrics::{
    APPBAR_H, CELL_H, CELL_W, ROW_H, SESSBAR_H, SIDEBAR_DIVIDER_W, STATUS_H, UI_BOLD, UI_FONT,
};
use super::palette as c;
use super::pty::{rebuild_row_runs, PtyProgram};
use super::rows::{
    activity_group_header, project_row, session_activity_row,
    session_row, worktree_activity_row, worktree_row,
};
use super::state::{FocusedPane, Grove, Msg, PtyCacheEntry, PtyCell, PtyPane, SidebarView};
use super::widgets::{
    control_btn_sized, control_icon_btn, divider_h, divider_v, dot, empty_workspace, footer_btn,
    icon_btn, modal_action,
    modal_dir_row, modal_list_row, modal_panel, seg_button, sidebar_agent_menu_overlay, tool_btn,
    tool_btn_toggle, vline, ModalBtn, SegSide,
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
    let raw = s.current_title()?;
    if raw.eq_ignore_ascii_case(&s.label) || raw.eq_ignore_ascii_case(s.agent.label()) {
        return None;
    }
    // OSC titles often start with emoji or box-drawing characters that the UI
    // font (IBM Plex Sans) can't render — strip them so the sess_bar never
    // shows a tofu box. The sidebar applies the same filter.
    super::rows::sanitize_ui_text(&raw)
}

fn is_in_progress_title(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    lower.contains("in progress") || lower.contains("in-progress") || lower.contains("in_progress")
}

impl Grove {
    pub fn view(&self) -> Element<'_, Msg> {
        let body = if self.app.chrome_visible {
            column![
                self.appbar(),
                row![self.sidebar(), self.sidebar_resize_handle(), self.workspace()]
                    .height(Length::Fill)
                    .width(Length::Fill),
                self.statusbar(),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
        } else {
            column![self.workspace()]
                .width(Length::Fill)
                .height(Length::Fill)
        };

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
        let brand = row![text("grove").font(UI_BOLD).size(14).color(c::MAGENTA()),]
            .spacing(8)
            .padding(Padding::from([0, 16]))
            .align_y(iced::Alignment::Center);

        let seg = container(
            row![
                seg_button(
                    "native",
                    !self.app.use_tmux(),
                    SegSide::Left,
                    Msg::BackendNative
                ),
                seg_button(
                    "tmux",
                    self.app.use_tmux(),
                    SegSide::Right,
                    Msg::BackendTmux
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

        // `- <num> +` reads as one control: the three buttons sit flush (zero
        // row spacing, tight per-button padding), with outer horizontal padding
        // that becomes the margin to the left of `-` and the right of `+`,
        // separating the unit from the segment buttons and the theme toggle.
        let zoom = container(
            row![
                control_icon_btn("minus", Msg::ZoomOut, 20.0, 13.0),
                control_btn_sized(format!("{:.0}%", self.ui_zoom * 100.0), Msg::ZoomReset, 12, 2),
                control_icon_btn("plus", Msg::ZoomIn, 20.0, 13.0),
            ]
            .spacing(0)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([0, 10]));

        let right = row![
            seg,
            zoom,
            icon_btn("contrast", Msg::OpenThemePicker),
        ]
        .spacing(4)
        .padding(Padding::from([0, 16]))
        .align_y(iced::Alignment::Center);

        let inner = row![
            container(brand).width(self.sidebar_width),
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

    /// The draggable divider between the sidebar and the workspace. A 1px line
    /// centered in a `SIDEBAR_DIVIDER_W`-wide hit zone, with a resize cursor on
    /// hover. The press starts a drag; cursor moves and the release are tracked
    /// by a global subscription (see `Grove::subscription`).
    fn sidebar_resize_handle(&self) -> Element<'_, Msg> {
        iced::widget::mouse_area(
            container(divider_v(c::BORDER()))
                .height(Length::Fill)
                .center_x(SIDEBAR_DIVIDER_W),
        )
        .on_press(Msg::SidebarDragStart)
        .interaction(iced::mouse::Interaction::ResizingHorizontally)
        .into()
    }

    // ── sidebar ───────────────────────────────────────────────────────────
    fn sidebar(&self) -> Element<'_, Msg> {
        let tree_head = self.tree_head();
        let content: Element<'_, Msg> = match self.sidebar_view {
            SidebarView::Tree => self.tree_view(),
            SidebarView::Activity => self.activity_view(),
            SidebarView::Terminal => self.terminal_sidebar(),
        };
        let tree_area = container(scrollable(content).height(Length::Fill))
            .height(Length::Fill)
            .padding(Padding {
                top: 8.0,
                bottom: 12.0,
                left: 0.0,
                right: 0.0,
            });
        let agent_menu_top = if matches!(self.sidebar_view, SidebarView::Tree) {
            self.open_agent_menu_top()
        } else {
            None
        };
        let tree_layer: Element<'_, Msg> = match agent_menu_top {
            Some((proj, wt, top, is_main)) => stack![
                tree_area,
                sidebar_agent_menu_overlay(proj, wt, top, is_main),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
            None => tree_area.into(),
        };

        // The footer is view-specific: project-oriented views get "+ add
        // project"; the terminal tab gets "+ new terminal".
        let footer: Element<'_, Msg> = if matches!(self.sidebar_view, SidebarView::Terminal) {
            footer_btn("+ new terminal", Msg::NewHomeTerminal)
        } else {
            footer_btn("+ add project", Msg::AddProject)
        };
        let stack_col = column![
            tree_head,
            divider_h(c::BORDER_SOFT()),
            tree_layer,
            divider_h(c::BORDER_SOFT()),
            footer,
        ]
        .height(Length::Fill);

        container(stack_col)
            .width(self.sidebar_width)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_RAIL())),
                ..Default::default()
            })
            .into()
    }

    fn tree_head(&self) -> Element<'_, Msg> {
        let collapsed = self.is_collapsed_to_sessionful_worktrees();
        let glyph = if collapsed {
            "expand-all"
        } else {
            "collapse-all"
        };
        let toggle = button(
            container(icon(glyph, 13.0, c::FG_MUTE()))
                .center_x(22)
                .center_y(22),
        )
        .on_press(Msg::ToggleCollapseAll)
        .padding(0)
        .style(|_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: if hovered {
                    Some(Background::Color(c::BG_HOVER()))
                } else {
                    None
                },
                text_color: if hovered { c::FG() } else { c::FG_MUTE() },
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius::from(4.0),
                },
                shadow: Shadow::default(),
            }
        });

        let activity_active = matches!(self.sidebar_view, SidebarView::Activity);
        let tree_active = matches!(self.sidebar_view, SidebarView::Tree);
        let terminal_active = matches!(self.sidebar_view, SidebarView::Terminal);
        let pillset = container(
            row![
                seg_button(
                    "activity",
                    activity_active,
                    SegSide::Left,
                    Msg::SidebarSetView(SidebarView::Activity),
                ),
                seg_button(
                    "tree",
                    tree_active,
                    SegSide::Middle,
                    Msg::SidebarSetView(SidebarView::Tree),
                ),
                seg_button(
                    "terminal",
                    terminal_active,
                    SegSide::Right,
                    Msg::SidebarSetView(SidebarView::Terminal),
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

        // The collapse-all toggle only belongs to tree mode.
        let right_tools: Element<'_, Msg> = if tree_active {
            container(toggle)
                .height(Length::Fill)
                .align_y(iced::Alignment::Center)
                .into()
        } else {
            Space::with_width(Length::Fixed(0.0)).into()
        };

        container(
            row![
                container(pillset)
                    .height(Length::Fill)
                    .align_y(iced::Alignment::Center),
                Space::with_width(Length::Fill),
                right_tools,
            ]
            .align_y(iced::Alignment::Center)
            .height(Length::Fill)
            .padding(Padding {
                top: 0.0,
                bottom: 0.0,
                left: 14.0,
                right: 8.0,
            }),
        )
        .height(SESSBAR_H)
        .width(Length::Fill)
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
            // Collapsed projects surface the most urgent descendant state as
            // a trailing glyph; expanded parents show nothing extra.
            let proj_rollup = if !expanded {
                super::activity::most_urgent(
                    self.app
                        .sessions
                        .iter()
                        .filter(|s| s.project == pname)
                        .map(|s| self.activity_state(s)),
                )
            } else {
                None
            };
            col = col.push(project_row(
                pi,
                &pname,
                count,
                expanded,
                proj_rollup,
                self.blink_tick,
            ));

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
                let hovered = self.hovered_wt == Some((pi, wi));
                let wt_expanded = !self.collapsed_wt.contains(&(pi, wi));
                // Same roll-up rule as projects: only when collapsed.
                let wt_rollup = if !wt_expanded {
                    super::activity::most_urgent(
                        self.app
                            .sessions
                            .iter()
                            .filter(|s| s.wt_path == w.path)
                            .map(|s| self.activity_state(s)),
                    )
                } else {
                    None
                };
                let has_run = self.app.store.projects.get(pi).is_some_and(|p| {
                    p.scripts.run.as_deref().is_some_and(|s| !s.trim().is_empty())
                });
                let wt_el = worktree_row(
                    pi,
                    wi,
                    &wname,
                    &w.branch,
                    active_wt,
                    w.is_main,
                    hovered,
                    wt_expanded,
                    has_run,
                    wt_rollup,
                    self.blink_tick,
                );
                col = col.push(
                    iced::widget::mouse_area(wt_el)
                        .on_enter(Msg::HoverWorktree(Some((pi, wi))))
                        .on_exit(Msg::HoverWorktree(None)),
                );

                if !wt_expanded {
                    continue;
                }
                for (si, s) in self.app.sessions.iter().enumerate() {
                    if s.wt_path == w.path {
                        let active = self.app.active_session == Some(si);
                        let pending_kill = self.pending_kill == Some(si);
                        col = col.push(session_row(
                            si,
                            s,
                            &wname,
                            active,
                            pending_kill,
                            self.activity_state(s),
                            self.blink_tick,
                        ));
                    }
                }
            }
        }
        col.into()
    }

    /// Flat activity-stream rendering of every session across every project /
    /// worktree, grouped by liveness (`running` / `idle` / `worktrees · no
    /// sessions`).
    fn activity_view(&self) -> Element<'_, Msg> {
        use super::activity::ActivityState;
        let now = std::time::Instant::now();

        // Pre-compute lookups used by both the grouping pass and the row
        // renderer below — folded once per frame instead of O(N) per row.
        let project_idx: std::collections::HashMap<&str, usize> = self
            .app
            .store
            .projects
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name.as_str(), i))
            .collect();
        let wt_paths_with_sessions: std::collections::HashSet<&str> = self
            .app
            .sessions
            .iter()
            .map(|s| s.wt_path.as_str())
            .collect();
        let mut session_count_by_wt: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for s in &self.app.sessions {
            *session_count_by_wt.entry(s.wt_path.as_str()).or_insert(0) += 1;
        }
        // Resolved worktree display name per session, indexed by session index.
        let session_wnames: Vec<String> = self
            .app
            .sessions
            .iter()
            .map(|s| self.resolve_session_wname(s, &project_idx))
            .collect();

        let mut waiting: Vec<usize> = Vec::new();
        let mut running: Vec<usize> = Vec::new();
        let mut idle: Vec<(usize, std::time::Instant)> = Vec::new();
        let mut exited: Vec<(usize, std::time::Instant)> = Vec::new();
        for (i, s) in self.app.sessions.iter().enumerate() {
            let t = *s.last_output_at.lock().unwrap_or_else(|e| e.into_inner());
            match self.activity_state(s) {
                ActivityState::WaitingForInput => waiting.push(i),
                ActivityState::Working => running.push(i),
                ActivityState::Done | ActivityState::Idle => idle.push((i, t)),
                ActivityState::Exited => exited.push((i, t)),
            }
        }
        // Exited sessions live under "idle" — they're not running, not "live".
        idle.extend(exited);
        // Waiting/working sessions sort by creation order (newest first) —
        // sorting by `last_output_at` made the list reorder on every PTY read,
        // since a live agent updates its timestamp many times per second.
        // Why: idle/exited timestamps are frozen by definition, so sorting
        // those by recency is stable; running ones aren't. The timestamps were
        // snapshotted above so the sort doesn't re-lock per comparison.
        waiting.sort_by_key(|i| std::cmp::Reverse(*i));
        running.sort_by_key(|i| std::cmp::Reverse(*i));
        idle.sort_by_key(|&(_, t)| std::cmp::Reverse(t));
        let idle: Vec<usize> = idle.into_iter().map(|(i, _)| i).collect();

        // All worktrees across all projects, listed `project / worktree`.
        // The row shows the session count and lets the user spawn new
        // sessions inline regardless of whether sessions already exist.
        let mut worktree_rows: Vec<(usize, usize, String, String, bool, usize)> = Vec::new();
        for (pi, p) in self.app.store.projects.iter().enumerate() {
            let wts: &[Worktree] = if pi == self.app.proj_idx {
                &self.app.worktrees
            } else {
                self.wt_cache.get(&pi).map(|v| v.as_slice()).unwrap_or(&[])
            };
            for (wi, w) in wts.iter().enumerate() {
                let wname = if w.is_main {
                    p.name.clone()
                } else {
                    crate::app::path_basename(&w.path)
                };
                let count = session_count_by_wt
                    .get(w.path.as_str())
                    .copied()
                    .unwrap_or(0);
                worktree_rows.push((pi, wi, p.name.clone(), wname, w.is_main, count));
            }
        }
        let _ = wt_paths_with_sessions; // retained above for symmetry; not used now

        let no_sessions_expanded = self
            .activity_no_sessions_expanded
            .unwrap_or(!worktree_rows.is_empty());

        let mut col: Column<'_, Msg> = Column::new().spacing(0);

        // "waiting" is an attention group: shown on top, hidden when empty
        // (unlike the always-visible running/idle scaffolding).
        if !waiting.is_empty() {
            col = col.push(activity_group_header("waiting", waiting.len(), true, None));
            for si in waiting {
                col = col.push(self.activity_row_wrapped(si, &session_wnames[si], now, &project_idx));
            }
        }

        col = col.push(activity_group_header("running", running.len(), true, None));
        if running.is_empty() {
            col = col.push(self.activity_empty_hint("no live sessions"));
        }
        for si in running {
            col = col.push(self.activity_row_wrapped(si, &session_wnames[si], now, &project_idx));
        }

        col = col.push(activity_group_header("idle", idle.len(), true, None));
        if idle.is_empty() {
            col = col.push(self.activity_empty_hint("nothing paused"));
        }
        for si in idle {
            col = col.push(self.activity_row_wrapped(si, &session_wnames[si], now, &project_idx));
        }

        col = col.push(activity_group_header(
            "worktrees",
            worktree_rows.len(),
            no_sessions_expanded,
            Some(Msg::ToggleActivityNoSessionsGroup),
        ));
        if no_sessions_expanded {
            for (pi, wi, pname, wname, is_main, count) in worktree_rows {
                let hovered = self.hovered_wt == Some((pi, wi));
                let row_el = worktree_activity_row(pi, wi, &pname, &wname, is_main, count, hovered);
                col = col.push(
                    iced::widget::mouse_area(row_el)
                        .on_enter(Msg::HoverWorktree(Some((pi, wi))))
                        .on_exit(Msg::HoverWorktree(None)),
                );
            }
        }

        col.into()
    }

    /// Build one activity-stream session row and wrap it in a `mouse_area` so
    /// hovering reveals the inline spawn chips (mirrors how `tree_view` wraps
    /// worktree rows for `HoverWorktree`).
    fn activity_row_wrapped<'a>(
        &'a self,
        si: usize,
        wname: &str,
        now: std::time::Instant,
        project_idx: &std::collections::HashMap<&str, usize>,
    ) -> Element<'a, Msg> {
        let s = &self.app.sessions[si];
        let active = self.app.active_session == Some(si);
        let pending_kill = self.pending_kill == Some(si);
        let t = *s.last_output_at.lock().unwrap_or_else(|e| e.into_inner());
        let last = Some(now.saturating_duration_since(t));
        let hovered = self.hovered_activity_row == Some(si);
        let coords = self.resolve_session_wt_coords(s, project_idx);
        let row_el = session_activity_row(
            si,
            s,
            &s.project,
            wname,
            active,
            pending_kill,
            last,
            hovered,
            coords,
            self.activity_state(s),
            self.blink_tick,
        );
        iced::widget::mouse_area(row_el)
            .on_enter(Msg::HoverActivityRow(Some(si)))
            .on_exit(Msg::HoverActivityRow(None))
            .into()
    }

    /// Resolve a session's `(project, worktree)` indices for the spawn chips.
    /// Returns `None` when the worktree list for that project isn't cached
    /// (e.g. a collapsed, never-expanded project) — the row then falls back to
    /// showing the relative time with no spawn affordance.
    fn resolve_session_wt_coords(
        &self,
        s: &Session,
        project_idx: &std::collections::HashMap<&str, usize>,
    ) -> Option<(usize, usize)> {
        let &pi = project_idx.get(s.project.as_str())?;
        let wts: &[Worktree] = if pi == self.app.proj_idx {
            &self.app.worktrees
        } else {
            self.wt_cache.get(&pi).map(|v| v.as_slice())?
        };
        let wi = wts.iter().position(|w| w.path == s.wt_path)?;
        Some((pi, wi))
    }

    /// Resolve a session's worktree display name using a pre-built project
    /// name → index map, so we avoid a linear scan over `store.projects` per
    /// session row in `activity_view`.
    fn resolve_session_wname(
        &self,
        s: &Session,
        project_idx: &std::collections::HashMap<&str, usize>,
    ) -> String {
        let Some(&pi) = project_idx.get(s.project.as_str()) else {
            return crate::app::path_basename(&s.wt_path);
        };
        let wts: &[Worktree] = if pi == self.app.proj_idx {
            &self.app.worktrees
        } else {
            self.wt_cache.get(&pi).map(|v| v.as_slice()).unwrap_or(&[])
        };
        let pname = &self.app.store.projects[pi].name;
        wts.iter()
            .find(|w| w.path == s.wt_path)
            .map(|w| {
                if w.is_main {
                    pname.clone()
                } else {
                    crate::app::path_basename(&w.path)
                }
            })
            .unwrap_or_else(|| crate::app::path_basename(&s.wt_path))
    }

    fn activity_empty_hint<'a>(&self, label: &'a str) -> Element<'a, Msg> {
        container(
            text(label.to_string())
                .font(UI_FONT)
                .size(11)
                .color(c::FG_MUTE()),
        )
        .height(ROW_H)
        .width(Length::Fill)
        .align_y(iced::Alignment::Center)
        // 12px row padding + 14px dot column + 8px spacing = 34, so the hint
        // text lines up with the activity rows' titles.
        .padding(Padding::from([0, 34]))
        .into()
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
                let wname = if w.is_main {
                    pname.to_string()
                } else {
                    crate::app::path_basename(&w.path)
                };
                let show_branch = super::rows::worktree_shows_branch(w.is_main, &w.branch, &wname);
                let wt_h = super::rows::worktree_row_height(show_branch);
                if pi == open_proj && wi == open_wt {
                    return Some((pi, wi, 6.0 + acc_y + wt_h, w.is_main));
                }
                acc_y += wt_h;

                if self.collapsed_wt.contains(&(pi, wi)) {
                    continue;
                }
                for s in &self.app.sessions {
                    if s.project == pname && s.wt_path == w.path {
                        acc_y += ROW_H;
                    }
                }
            }
        }

        None
    }

    /// The draggable divider between the session view and the right-docked
    /// terminal panel. Mirrors `sidebar_resize_handle` but drives the panel's
    /// percentage split (`term_panel_portion`) instead of a pixel width.
    fn term_panel_resize_handle(&self) -> Element<'_, Msg> {
        iced::widget::mouse_area(
            container(divider_v(c::BORDER()))
                .height(Length::Fill)
                .center_x(SIDEBAR_DIVIDER_W),
        )
        .on_press(Msg::TermPanelDragStart)
        .interaction(iced::mouse::Interaction::ResizingHorizontally)
        .into()
    }

    // ── workspace ─────────────────────────────────────────────────────────
    fn workspace(&self) -> Element<'_, Msg> {
        if self.terminal_tab() {
            return self.terminal_workspace();
        }
        let left: Element<'_, Msg> = match self.app.active_session {
            Some(i) if i < self.app.sessions.len() => column![
                self.sess_bar(i, &self.app.sessions[i]),
                self.pty(PtyPane::Agent, &self.app.sessions[i]),
            ]
            .height(Length::Fill)
            .into(),
            _ => empty_workspace(),
        };

        // When the slide-over panel is open and a session is active, split the
        // workspace: session view on the left (filling remaining space), the
        // worktree terminal panel docked full-height on the right (~46%).
        let inner: Element<'_, Msg> = if self.term_panel_open && self.active_wt_path().is_some() {
            row![
                container(left)
                    .width(Length::FillPortion(100 - self.term_panel_portion))
                    .height(Length::Fill),
                self.term_panel_resize_handle(),
                container(self.term_panel())
                    .width(Length::FillPortion(self.term_panel_portion))
                    .height(Length::Fill),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            left
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

    /// The right-docked terminal slide-over for the active session's worktree:
    /// a thin tab strip (one tab per shell + a `＋` add) above the active
    /// shell's PTY. Reuses the shared `pty()` renderer.
    fn term_panel(&self) -> Element<'_, Msg> {
        let Some(wt) = self.active_wt_path() else {
            return empty_workspace();
        };
        let shells = self.app.wt_terminals_for(&wt);
        let active_idx = self.app.active_wt_terminal_idx(&wt);

        // Tab strip: a small mono tab per shell with a running/exited dot and a
        // × close, plus a ＋ to add a new shell.
        let mut tabs = row![].spacing(6).align_y(iced::Alignment::Center);
        for (i, s) in shells.iter().enumerate() {
            tabs = tabs.push(self.term_panel_tab(i, s, active_idx == Some(i)));
        }
        tabs = tabs.push(
            button(
                container(icon("plus", 13.0, c::FG_DIM()))
                    .center_x(22)
                    .center_y(22),
            )
            .on_press(Msg::NewWtTerminal)
            .padding(0)
            .style(|_, status| {
                let hovered = matches!(status, button::Status::Hovered);
                button::Style {
                    background: if hovered {
                        Some(Background::Color(c::BG_HOVER()))
                    } else {
                        None
                    },
                    text_color: if hovered { c::FG() } else { c::FG_DIM() },
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: Radius::from(4.0),
                    },
                    shadow: Shadow::default(),
                }
            }),
        );

        // Close the whole slide-over (same effect as the header term toggle),
        // so the panel is always dismissable from itself.
        let close_panel = icon_btn("close", Msg::ToggleTermPanel);

        let strip = container(
            row![
                container(
                    scrollable(tabs).direction(scrollable::Direction::Horizontal(
                        scrollable::Scrollbar::new().width(0).scroller_width(0)
                    ))
                )
                .width(Length::Fill)
                .clip(true),
                close_panel,
            ]
            .align_y(iced::Alignment::Center)
            .height(Length::Fill)
            .padding(Padding::from([0, 10])),
        )
        .height(SESSBAR_H)
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(c::BG_STRIP())),
            ..Default::default()
        });

        let surface: Element<'_, Msg> = match self.app.active_wt_terminal(&wt) {
            Some(s) => self.pty(PtyPane::Panel, s),
            None => empty_workspace(),
        };

        column![strip, divider_h(c::BORDER_SOFT()), surface]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// A single tab in the terminal panel's tab strip.
    fn term_panel_tab<'a>(&self, idx: usize, s: &Session, active: bool) -> Element<'a, Msg> {
        let running = matches!(
            *s.status.lock().unwrap_or_else(|e| e.into_inner()),
            SessionStatus::Running
        );
        let dot_color = if running { c::GREEN() } else { c::FG_MUTE() };
        let name_color = if active { c::CYAN() } else { c::FG_DIM() };

        let close = button(
            container(icon("close", 11.0, c::FG_MUTE()))
                .center_x(16)
                .center_y(18),
        )
        .on_press(Msg::CloseWtTerminal(idx))
        .padding(0)
        .style(|_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: None,
                text_color: if hovered { c::RED() } else { c::FG_MUTE() },
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius::from(3.0),
                },
                shadow: Shadow::default(),
            }
        });

        // Tabs are identified by a terminal icon (status conveyed by the dot
        // and the active highlight), not a textual name — cleaner when several
        // shells share a worktree.
        let label = row![dot(dot_color), icon("term", 13.0, name_color), close,]
            .spacing(6)
            .align_y(iced::Alignment::Center);

        button(container(label).padding(Padding::from([0, 8])).center_y(24))
            .on_press(Msg::SelectWtTerminal(idx))
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
                    text_color: name_color,
                    border: Border {
                        color: if active {
                            c::CYAN()
                        } else {
                            Color::TRANSPARENT
                        },
                        width: if active { 1.0 } else { 0.0 },
                        radius: Radius::from(4.0),
                    },
                    shadow: Shadow::default(),
                }
            })
            .into()
    }

    /// Workspace for the persistent home-terminal tab: a status bar with a
    /// restart control above the home shell's PTY. Shows a spawn-failure hint
    /// if the shell could never be started.
    fn terminal_workspace(&self) -> Element<'_, Msg> {
        let inner: Element<'_, Msg> = match self.app.active_home_terminal() {
            Some(s) => column![self.home_terminal_bar(s), self.pty(PtyPane::Agent, s)]
                .height(Length::Fill)
                .into(),
            None => empty_workspace(),
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

    /// Status bar for the home terminal. Unlike `sess_bar` there is no kill
    /// action — the terminal is permanent — only a restart that relaunches the
    /// shell at `~`. When the shell has exited the restart button is the
    /// affordance the user reaches for.
    fn home_terminal_bar(&self, s: &Session) -> Element<'_, Msg> {
        let running = matches!(
            *s.status.lock().unwrap_or_else(|e| e.into_inner()),
            SessionStatus::Running
        );
        let (dot_color, label) = if running {
            (c::GREEN(), "running")
        } else {
            (c::FG_MUTE(), "exited")
        };
        let bar_text = |content: String, color: Color| {
            text(content)
                .font(UI_FONT)
                .size(12)
                .line_height(1.0)
                .height(18)
                .align_y(iced::alignment::Vertical::Center)
                .color(color)
        };

        let status: Element<'_, Msg> = row![dot(dot_color), bar_text(label.to_string(), dot_color)]
            .spacing(6)
            .align_y(iced::Alignment::Center)
            .into();

        let ctx = crate::gui::rows::terminal_context(s).unwrap_or_else(|| "~".to_string());
        let ctx = crate::gui::widgets::truncate_middle(&ctx, 80);
        let identity = row![bar_text(ctx, c::FG())]
            .spacing(6)
            .align_y(iced::Alignment::Center);

        let bar = row![
            status,
            vline(),
            container(identity).width(Length::Fill).clip(true),
            bar_text("~".to_string(), c::FG_MUTE()),
            vline(),
            tool_btn("restart", "restart", false, Msg::RestartHomeTerminal),
            tool_btn(
                "zen",
                if self.app.chrome_visible {
                    "zen"
                } else {
                    "exit zen"
                },
                false,
                Msg::ToggleZen,
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

    /// Sidebar body for the terminal tab — one row per home terminal, showing
    /// its label and contextual title (the shell's OSC window title, e.g. the
    /// current directory or running command). The active terminal is
    /// highlighted; the close affordance is hidden when only one remains so the
    /// tab always keeps a shell.
    fn terminal_sidebar(&self) -> Element<'_, Msg> {
        let mut col: Column<'_, Msg> = Column::new();
        let show_close = self.app.home_terminals.len() > 1;
        for (i, s) in self.app.home_terminals.iter().enumerate() {
            let active = self.app.active_terminal == Some(i);
            col = col.push(crate::gui::rows::terminal_row(i, s, active, show_close));
        }
        col.into()
    }

    fn sess_bar(&self, si: usize, s: &Session) -> Element<'_, Msg> {
        let running = matches!(
            *s.status.lock().unwrap_or_else(|e| e.into_inner()),
            SessionStatus::Running
        );
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
                .font(UI_FONT)
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
            sess_text(s.label.clone(), c::FG()),
            sess_text("·".to_string(), c::FG_MUTE()),
            sess_text(s.branch.clone(), c::FG_MUTE()),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        if let Some(title) = context {
            let title = crate::gui::widgets::truncate_middle(&title, 80);
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

        // Resolve the session's (project, worktree) indices so the run button
        // can target the right worktree, and only show it when the project has
        // a run script configured.
        let coords = self
            .app
            .store
            .projects
            .iter()
            .position(|p| p.name == s.project)
            .and_then(|pi| {
                let wts: &[Worktree] = if pi == self.app.proj_idx {
                    &self.app.worktrees
                } else {
                    self.wt_cache.get(&pi).map(|v| v.as_slice()).unwrap_or(&[])
                };
                wts.iter().position(|w| w.path == s.wt_path).map(|wi| (pi, wi))
            });
        let run_btn: Element<'_, Msg> = match coords {
            Some((proj, wt))
                if self.app.store.projects[proj]
                    .scripts
                    .run
                    .as_deref()
                    .is_some_and(|s| !s.trim().is_empty()) =>
            {
                tool_btn("play", "run script", false, Msg::RunScript { proj, wt })
            }
            _ => Space::with_width(0).into(),
        };

        let bar = row![
            status,
            vline(),
            container(identity).width(Length::Fill).clip(true),
            sess_text(s.wt_path.clone(), c::FG_MUTE()),
            vline(),
            run_btn,
            tool_btn_toggle(
                "term",
                "terminal",
                false,
                self.term_panel_open,
                Msg::ToggleTermPanel
            ),
            tool_btn(
                "zen",
                if self.app.chrome_visible {
                    "zen"
                } else {
                    "exit zen"
                },
                false,
                Msg::ToggleZen,
            ),
            tool_btn(
                "trash",
                if self.pending_kill == Some(si) {
                    "confirm kill"
                } else {
                    "kill"
                },
                true,
                // Two-step confirm, targeting the session this bar renders —
                // never a fallback index.
                if self.pending_kill == Some(si) {
                    Msg::KillSession(si)
                } else {
                    Msg::RequestKillSession(si)
                },
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

    fn pty(&self, pane: PtyPane, s: &Session) -> Element<'_, Msg> {
        // Per-session row snapshot + canvas cache. Switching to a quiet
        // session returns the cached geometry with zero draw work; switching
        // to a session that produced output re-snaps the rows and clears the
        // canvas cache, then draws once.
        // The `dirty` Arc's address is the cache key. A dropped session can
        // free that address and a newly spawned one reuse it — safe only
        // because every session add/remove (incl. home-terminal new/close/
        // restart) fully clears this cache, so no stale entry can alias.
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
                let parser = s.parser.lock().unwrap_or_else(|e| e.into_inner());
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
        // Translate the scrollback-stable selection into the current viewport.
        // Each endpoint clamps to the visible window; a selection entirely off
        // one edge isn't painted. The selection lives in the *focused* session,
        // so only paint it on the PTY that currently owns input — otherwise a
        // selection in one pane would mis-render against the other's grid.
        let selection = if pane == self.focused_input_pane() {
            self.pty_selection
        } else {
            None
        }
        .and_then(|(a, b)| {
            let (h, sb) = {
                let p = s.parser.lock().ok()?;
                (
                    p.screen().size().0 as isize,
                    p.screen().scrollback() as isize,
                )
            };
            if h == 0 {
                return None;
            }
            let to_vr = |c: &super::state::AbsCell| (h - 1) - (c.a_row as isize - sb);
            let (ra, rb) = (to_vr(&a), to_vr(&b));
            if (ra < 0 && rb < 0) || (ra > h - 1 && rb > h - 1) {
                return None;
            }
            let cell = |c: &super::state::AbsCell, r: isize| PtyCell {
                row: r.clamp(0, h - 1) as usize,
                col: c.col,
            };
            Some((cell(&a, ra), cell(&b, rb)))
        });
        let program = PtyProgram {
            pane,
            rows,
            cache,
            selection,
            cursor: cursor_pos,
            cursor_visible,
        };
        let body: Element<'_, Msg> = canvas_widget(program)
            .width(Length::Fixed((cols * CELL_W).max(CELL_W)))
            .height(Length::Fixed((rows_len * CELL_H).max(CELL_H)))
            .into();

        // While the split is live, tint the focused PTY's top edge so it's clear
        // which terminal will receive keystrokes. Suppressed when the panel is
        // closed (only one PTY is interactive then).
        let focused = self.term_panel_open && pane == self.focused_input_pane();
        container(scrollable(body).width(Length::Fill).height(Length::Fill))
            .padding(Padding::from([12, 16]))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| container::Style {
                background: Some(Background::Color(c::BG())),
                border: if focused {
                    Border {
                        color: c::CYAN(),
                        width: 1.0,
                        radius: Radius::from(0.0),
                    }
                } else {
                    Border::default()
                },
                ..Default::default()
            })
            .into()
    }

    /// The pane that currently owns keyboard/scroll/selection input. Mirrors the
    /// routing logic in `focused_session*`: the panel only wins while it is open
    /// *and* `focused_pane` selects it; otherwise input belongs to the agent.
    pub(super) fn focused_input_pane(&self) -> PtyPane {
        if self.term_panel_open && matches!(self.focused_pane, FocusedPane::Panel) {
            PtyPane::Panel
        } else {
            PtyPane::Agent
        }
    }

    // ── status bar ────────────────────────────────────────────────────────
    fn statusbar(&self) -> Element<'_, Msg> {
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
            Modal::RemoveProject {
                name,
                worktrees,
                also_remove_worktrees,
                in_progress,
                done,
                current,
                errors,
                ..
            } => self.remove_project_modal(
                name,
                worktrees,
                *also_remove_worktrees,
                *in_progress,
                *done,
                current,
                errors,
            ),
            Modal::Message(message) => self.message_modal(message),
            Modal::TmuxChoice => self.tmux_choice_modal(),
            Modal::AgentPicker {
                project,
                wt_path,
                sel,
            } => self.agent_picker_modal(project, wt_path, *sel),
            Modal::ThemePicker {
                sel_dark,
                sel_light,
                tab,
                ..
            } => self.theme_picker_modal(*sel_dark, *sel_light, *tab),
            Modal::Teardown => self.teardown_modal(),
            Modal::ScriptsEditor => self.scripts_editor_modal(),
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
            let mut cache = self.dir_cache.borrow_mut();
            match cache.as_ref() {
                Some((k, v)) if k == buffer => v.clone(),
                _ => {
                    let v = crate::app::list_dirs(buffer);
                    *cache = Some((buffer.to_string(), v.clone()));
                    v
                }
            }
        } else {
            Vec::new()
        };
        let visible_matches = if show_dirs {
            entries.len().clamp(1, 6)
        } else {
            0
        };
        let modal_w = if show_dirs { 640.0 } else { 480.0 };

        let input = container(
            row![
                text(buffer.to_string())
                    .font(UI_FONT)
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
                modal_action("cancel", ModalBtn::Plain, Msg::ModalCancel),
                modal_action("submit", ModalBtn::Primary, Msg::ModalSubmit),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        );

        modal_panel(body.into(), modal_w, c::MAGENTA())
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
                modal_action("cancel", ModalBtn::Plain, Msg::ModalConfirm(false)),
                modal_action(
                    if destructive { "remove" } else { "confirm" },
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

        modal_panel(body.into(), 480.0, accent)
    }

    #[allow(clippy::too_many_arguments)]
    fn remove_project_modal<'a>(
        &'a self,
        name: &'a str,
        worktrees: &'a [String],
        also_remove: bool,
        in_progress: bool,
        done: usize,
        current: &'a str,
        errors: &'a [String],
    ) -> Element<'a, Msg> {
        use iced::widget::checkbox;
        use iced::widget::checkbox::{Status as CheckboxStatus, Style as CheckboxStyle};
        use iced::widget::progress_bar;
        use iced::widget::progress_bar::Style as ProgressStyle;

        let accent = c::RED();
        let total = worktrees.len();
        let prompt = if total == 0 {
            format!("'{name}' will be unregistered from grove. Files on disk stay put.")
        } else {
            format!(
                "'{name}' will be unregistered from grove. Non-main worktrees stay on disk unless you opt in below."
            )
        };
        let session_note = "Running sessions for this project will be stopped.";

        let mut body = column![
            text("remove project").size(13).color(accent),
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
            let cb = checkbox(label, also_remove)
                .on_toggle_maybe(if in_progress {
                    None
                } else {
                    Some(Msg::ToggleRemoveWorktrees)
                })
                .size(14)
                .spacing(8)
                .text_size(12)
                .font(UI_FONT)
                .style(|_, status| {
                    let (checked, disabled, hovered) = match status {
                        CheckboxStatus::Active { is_checked } => (is_checked, false, false),
                        CheckboxStatus::Hovered { is_checked } => (is_checked, false, true),
                        CheckboxStatus::Disabled { is_checked } => (is_checked, true, false),
                    };
                    let border_color = if checked {
                        c::RED()
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
                        icon_color: if disabled { c::FG_MUTE() } else { c::RED() },
                        border: Border {
                            color: border_color,
                            width: 1.0,
                            radius: Radius::from(4.0),
                        },
                        text_color: Some(if disabled { c::FG_MUTE() } else { c::FG_DIM() }),
                    }
                });
            body = body.push(Space::with_height(2)).push(cb);
        }

        if in_progress {
            let frac = if total == 0 {
                1.0
            } else {
                (done as f32 / total as f32).clamp(0.0, 1.0)
            };
            let status = if done >= total {
                "finishing…".to_string()
            } else {
                format!("Removing {} of {}: {}", done + 1, total, current)
            };
            body = body
                .push(Space::with_height(4))
                .push(
                    text(status)
                        .size(11)
                        .color(c::FG_MUTE())
                        .wrapping(iced::widget::text::Wrapping::None),
                )
                .push(
                    progress_bar(0.0..=1.0, frac)
                        .height(6.0)
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
            body = body.push(Space::with_height(8)).push(
                row![
                    Space::with_width(Length::Fill),
                    modal_action("cancel", ModalBtn::Plain, Msg::ModalCancel),
                    modal_action("remove", ModalBtn::Danger, Msg::ConfirmRemoveProject),
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

        modal_panel(body.into(), 520.0, accent)
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
                modal_action("close", ModalBtn::Primary, Msg::ModalCancel),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        modal_panel(body.into(), 480.0, c::CYAN())
    }

    fn teardown_modal(&self) -> Element<'_, Msg> {
        use crate::app::TeardownStage;
        let td = match &self.app.teardown {
            Some(td) => td,
            None => return Space::with_width(0).into(),
        };
        let wt_name = crate::app::path_basename(&td.wt_path);
        let done = matches!(td.stage, TeardownStage::Done { .. });
        let running = matches!(td.stage, TeardownStage::RunningScript);

        let mut body = column![
            text(format!("delete worktree / {wt_name}"))
                .size(13)
                .color(c::RED()),
        ]
        .spacing(12);

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
                Space::with_width(Length::Fill),
                modal_action("close", ModalBtn::Primary, Msg::ModalCancel),
            ]
        } else if running {
            // Let the user proceed without waiting for a hung teardown script.
            row![
                Space::with_width(Length::Fill),
                modal_action("skip & remove", ModalBtn::Plain, Msg::ModalCancel),
            ]
        } else {
            row![Space::with_width(Length::Fill)]
        }
        .spacing(8)
        .align_y(iced::Alignment::Center);

        body = body.push(Space::with_height(4)).push(buttons);
        modal_panel(body.into(), 560.0, c::RED())
    }

    fn scripts_editor_modal(&self) -> Element<'_, Msg> {
        use super::state::ScriptField;
        let ed = match &self.scripts_editor {
            Some(ed) => ed,
            None => return Space::with_width(0).into(),
        };

        let field = |label: &str,
                     desc: &str,
                     placeholder: &str,
                     content,
                     which: ScriptField| {
            // Shrink height grows the editor with its content (Iced sizes a
            // Shrink text_editor to its measured line count), so it never
            // scrolls internally — the outer scroll area absorbs any overflow.
            let editor = iced::widget::text_editor(content)
                .height(Length::Shrink)
                .font(iced::Font::MONOSPACE)
                .size(12)
                .padding(8)
                .placeholder(placeholder.to_string())
                .style(|_, status| {
                    use iced::widget::text_editor::Status;
                    // Cyan border on focus mirrors the modal accent and tells the
                    // user which field has keyboard focus without relying on color
                    // alone (the caret and selection move with it too).
                    let border_color = match status {
                        Status::Focused => c::CYAN(),
                        Status::Hovered => c::BORDER(),
                        _ => c::BORDER_SOFT(),
                    };
                    iced::widget::text_editor::Style {
                        background: Background::Color(c::BG_STRIP()),
                        border: Border {
                            color: border_color,
                            width: 1.0,
                            radius: Radius::from(4.0),
                        },
                        icon: c::FG_MUTE(),
                        placeholder: c::FG_MUTE(),
                        value: c::FG(),
                        selection: c::BG_HL(),
                    }
                })
                .on_action(move |a| Msg::ScriptsEditorAction(which, a));
            column![
                text(label.to_string()).size(12).color(c::FG()),
                text(desc.to_string())
                    .size(11)
                    .color(c::FG_MUTE())
                    .wrapping(iced::widget::text::Wrapping::Word),
                editor,
            ]
            .spacing(5)
        };

        let fields = column![
            field(
                "setup",
                "Runs once when a new worktree is created, inside the new worktree's directory. \
                 Use it to install dependencies, copy ignored env files, or start the services \
                 an agent needs before you begin working.",
                "npm install",
                &ed.setup,
                ScriptField::Setup,
            ),
            field(
                "run",
                "Runs on demand when you press the play button (worktree row or session header). \
                 It opens an interactive terminal tab, so it suits dev servers, test watchers, \
                 or any command you want to watch and interact with.",
                "npm run dev",
                &ed.run,
                ScriptField::Run,
            ),
            field(
                "teardown",
                "Runs when you delete the worktree, before it is removed from disk. Use it to \
                 stop services, tear down databases, or clean up anything setup created. \
                 Deletion proceeds once it exits.",
                "docker compose down",
                &ed.teardown,
                ScriptField::Teardown,
            ),
        ]
        .spacing(16);

        // The fields size to their content (min-height) and only scroll once
        // they exceed `max_height` — so on a tall enough window no scrollbar
        // appears at all. The scrollbar itself is invisible (zero-width,
        // transparent): scrolling still works via wheel/trackpad, but nothing
        // is drawn over the editors.
        use iced::widget::scrollable::{Direction, Rail, Scrollbar, Scroller};
        let invisible_rail = Rail {
            background: None,
            border: Border::default(),
            scroller: Scroller {
                color: Color::TRANSPARENT,
                border: Border::default(),
            },
        };
        let scroll_area = container(
            scrollable(fields)
                .height(Length::Shrink)
                .direction(Direction::Vertical(
                    Scrollbar::new().width(0).scroller_width(0),
                ))
                .style(move |_, _| iced::widget::scrollable::Style {
                    container: container::Style::default(),
                    vertical_rail: invisible_rail,
                    horizontal_rail: invisible_rail,
                    gap: None,
                }),
        )
        .max_height(480.0);

        let body = column![
            text(format!("scripts / {}", ed.project_name))
                .size(13)
                .color(c::CYAN()),
            text("Shell snippets shared by every worktree of this project, run via $SHELL -lc. Leave a field blank to disable that step.")
                .size(11)
                .color(c::FG_MUTE())
                .wrapping(iced::widget::text::Wrapping::Word),
            scroll_area,
            row![
                Space::with_width(Length::Fill),
                modal_action("cancel", ModalBtn::Plain, Msg::ScriptsEditorCancel),
                modal_action("save", ModalBtn::Primary, Msg::ScriptsEditorSave),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        modal_panel(body.into(), 560.0, c::CYAN())
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
                modal_action("native", ModalBtn::Plain, Msg::ChooseTmux(false)),
                modal_action("tmux", ModalBtn::Primary, Msg::ChooseTmux(true)),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        modal_panel(body.into(), 480.0, c::CYAN())
    }

    fn agent_picker_modal<'a>(
        &'a self,
        project: &'a str,
        wt_path: &'a str,
        sel: usize,
    ) -> Element<'a, Msg> {
        let wt_name = crate::app::path_basename(wt_path);
        let title = if project.is_empty() {
            format!("start session / {wt_name}")
        } else {
            format!("start session / {project} / {wt_name}")
        };

        let mut list = Column::new().spacing(0);
        for (i, agent) in self.app.available_agents.iter().enumerate() {
            let active = i == sel;
            let is_default = self.app.store.default_agent == Some(*agent);
            let label = row![
                text(agent.label().to_string()).size(12).color(if active {
                    c::FG()
                } else {
                    c::FG_DIM()
                }),
                Space::with_width(Length::Fill),
                text(if is_default { "default" } else { "" })
                    .size(11)
                    .color(c::FG_MUTE()),
            ]
            .align_y(iced::Alignment::Center);

            list = list.push(modal_list_row(label, active, Msg::AgentPickerSelect(i)));
        }

        let list_h = (self.app.available_agents.len() as f32) * ROW_H;
        let list_box = container(list)
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
            text(title).size(13).color(c::MAGENTA()),
            list_box,
            Space::with_height(8),
            row![
                modal_action("default", ModalBtn::Plain, Msg::AgentPickerToggleDefault),
                Space::with_width(Length::Fill),
                modal_action("cancel", ModalBtn::Plain, Msg::ModalCancel),
                modal_action("launch", ModalBtn::Primary, Msg::AgentPickerSubmit),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        modal_panel(body.into(), 500.0, c::MAGENTA())
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

        // Same segmented control as the appbar backend switch and the sidebar
        // view switch — one vocabulary for "choose one of N".
        let tabs = container(
            row![
                seg_button(
                    "dark",
                    matches!(tab, crate::theme::ThemeKind::Dark),
                    SegSide::Left,
                    Msg::ThemePickerSwitchTab,
                ),
                seg_button(
                    "light",
                    matches!(tab, crate::theme::ThemeKind::Light),
                    SegSide::Right,
                    Msg::ThemePickerSwitchTab,
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

        let mut list = Column::new().spacing(0);
        for (i, th) in themes.iter().enumerate() {
            let active = i == sel;
            let name = th.name.to_string();
            list = list.push(modal_list_row(
                text(name)
                    .size(12)
                    .color(if active { c::FG() } else { c::FG_DIM() }),
                active,
                Msg::ThemePickerSelect(i),
            ));
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
            Space::with_height(8),
            row![
                Space::with_width(Length::Fill),
                modal_action("cancel", ModalBtn::Plain, Msg::ThemePickerCancel),
                modal_action("apply", ModalBtn::Primary, Msg::ThemePickerSubmit),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        modal_panel(body.into(), 460.0, c::MAGENTA())
    }
}
