//! `Grove::view` and the chrome it composes (appbar, sidebar, workspace,
//! statusbar, modal layer). Pure rendering — no state mutation.

use super::icons::icon;
use super::metrics::{
    APPBAR_H, CELL_H, CELL_W, MONO_FONT, ROW_H, SESSBAR_H, SIDEBAR_DIVIDER_W, STATUS_H, UI_BOLD,
    UI_FONT,
};
use super::palette as c;
use super::pty::{rebuild_row_runs, PtyProgram};
use super::rows::{
    activity_group_header, project_row, session_activity_row, session_row, single_line,
    truncate_ellipsis, worktree_activity_row, worktree_row,
};
use super::state::{
    FocusedPane, Grove, Msg, PtyCacheEntry, PtyCell, PtyPane, SidebarView, UpgradeState,
};
use super::update::{platform_mod_label, GlobalShortcut, Scope, ShortcutDef, SHORTCUTS};
use super::widgets::{
    control_btn_sized, control_icon_btn, divider_h, divider_v, dot, empty_workspace, footer_btn,
    icon_btn, launcher_row, modal_action, modal_checkbox, modal_list_row, modal_panel, seg_button,
    sidebar_agent_menu_overlay, tool_btn, tool_btn_toggle, truncate_middle, vline, ModalBtn,
    SegSide,
};
use crate::app::{AddProjectStep, ConfirmKind, GitProbe, Modal, OnboardStep};
use crate::git::Worktree;
use crate::session::{Session, SessionStatus};
use iced::border::Radius;
use iced::widget::{
    button, canvas as canvas_widget, column, container, row, scrollable, stack, text, text_input,
    Column, Id, Row, Space,
};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};
use std::sync::atomic::Ordering;

/// Stable id for the add-project / add-worktree primary text input, used to
/// focus it from `update` when the modal opens.
pub fn modal_input_id() -> Id {
    Id::new("modal-input-primary")
}

/// Stable id for the add-project details-step name field, used to focus it
/// when the modal advances to step 2.
pub fn modal_name_id() -> Id {
    Id::new("modal-input-name")
}

/// Shared `text_input` styling for modal fields: strip background, themed
/// border, cyan caret/selection. Focus brightens the border.
fn input_field_style(_t: &iced::Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: Background::Color(c::BG_STRIP()),
        border: Border {
            color: if focused { c::MAGENTA() } else { c::BORDER() },
            width: 1.0,
            radius: Radius::from(4.0),
        },
        icon: c::FG_MUTE(),
        placeholder: c::FG_MUTE(),
        value: c::FG(),
        selection: c::CYAN(),
    }
}

/// Stable id for the theme-picker scrollable, used to scroll the active
/// selection into view from `update`.
pub fn theme_picker_scrollable_id() -> Id {
    Id::new("theme-picker-list")
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
            let workspace_row: Element<'_, Msg> = if self.grid_view {
                // Grid mode: sidebar is hidden, workspace fills the full width.
                self.workspace()
            } else {
                row![
                    self.sidebar(),
                    self.sidebar_resize_handle(),
                    self.workspace()
                ]
                .height(Length::Fill)
                .width(Length::Fill)
                .into()
            };
            column![
                self.appbar(),
                container(workspace_row)
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

        let content: Element<'_, Msg> =
            if matches!(self.app.modal, Modal::None) && !self.show_changelog {
                body.into()
            } else {
                let mut layers = stack![body];
                if !matches!(self.app.modal, Modal::None) {
                    layers = layers.push(self.modal_layer());
                }
                if self.show_changelog {
                    layers = layers.push(self.changelog_modal());
                }
                layers.width(Length::Fill).height(Length::Fill).into()
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

        // App size, theme, and terminal backend now live in the Settings modal;
        // the appbar's right cluster is just the cog entry point.
        let cog = icon_btn("cog", Msg::OpenSettings);
        let cog: Element<'_, Msg> = if matches!(self.upgrade, UpgradeState::Available(_)) {
            stack![
                cog,
                container(dot(c::GREEN()))
                    .align_x(iced::alignment::Horizontal::Right)
                    .align_y(iced::alignment::Vertical::Top)
                    .width(Length::Fill)
                    .height(Length::Fill),
            ]
            .into()
        } else {
            cog
        };
        // Agent-view toggle. In agent (grid) view it grows a "+" session-launcher
        // segment on its left, forming a single segmented combo; on every other
        // screen it is a lone muted button. The combo replaces the floating "+"
        // FAB that used to hover over the grid.
        let view_control: Element<'_, Msg> = if self.grid_view {
            let plus = button(
                container(icon("plus", 13.0, c::MAGENTA()))
                    .center_x(26)
                    .center_y(22),
            )
            .on_press(Msg::OpenSessionLauncher)
            .padding(0)
            .style(|_, status| button::Style {
                background: if matches!(status, button::Status::Hovered) {
                    Some(Background::Color(c::BG_HOVER()))
                } else {
                    None
                },
                text_color: c::MAGENTA(),
                // Round the left corners only — the right edge butts the grid seg.
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius {
                        top_left: 4.0,
                        top_right: 0.0,
                        bottom_right: 0.0,
                        bottom_left: 4.0,
                    },
                },
                shadow: Shadow::default(),
                snap: false,
            });
            let grid_seg = button(
                container(icon("grid", 13.0, c::CYAN()))
                    .center_x(26)
                    .center_y(22),
            )
            .on_press(Msg::ToggleGridView)
            .padding(0)
            .style(|_, status| button::Style {
                background: Some(Background::Color(
                    if matches!(status, button::Status::Hovered) {
                        c::BG_HOVER()
                    } else {
                        c::BG_HL()
                    },
                )),
                text_color: c::CYAN(),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius {
                        top_left: 0.0,
                        top_right: 4.0,
                        bottom_right: 4.0,
                        bottom_left: 0.0,
                    },
                },
                shadow: Shadow::default(),
                snap: false,
            });
            // A short, fixed-height hairline between the segments. Using a
            // Fill-height divider here would inherit the appbar's full height and
            // stretch the combo taller than the lone toggle button.
            let seg_divider = container(Space::new().width(1))
                .width(1)
                .height(Length::Fixed(14.0))
                .style(|_| container::Style {
                    background: Some(Background::Color(c::BORDER())),
                    ..Default::default()
                });
            container(row![plus, seg_divider, grid_seg].align_y(iced::Alignment::Center))
                .style(|_| container::Style {
                    border: Border {
                        color: c::BORDER(),
                        width: 1.0,
                        radius: Radius::from(5.0),
                    },
                    ..Default::default()
                })
                .into()
        } else {
            let grid_color = c::FG_MUTE();
            button(
                container(icon("grid", 13.0, grid_color))
                    .center_x(22)
                    .center_y(22),
            )
            .on_press(Msg::ToggleGridView)
            .padding(0)
            .style(move |_, status| button::Style {
                background: if matches!(status, button::Status::Hovered) {
                    Some(Background::Color(c::BG_HOVER()))
                } else {
                    None
                },
                text_color: grid_color,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius::from(4.0),
                },
                shadow: Shadow::default(),
                snap: false,
            })
            .into()
        };

        let right = row![view_control, cog]
            .spacing(4)
            .padding(Padding::from([0, 16]))
            .align_y(iced::Alignment::Center);

        let inner = row![
            container(brand).width(self.sidebar_width),
            Space::new().width(Length::Fill),
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
                sidebar_agent_menu_overlay(proj, wt, top, is_main, &self.app.available_agents),
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
        // Glyph shows the *next* action the cycle button will take.
        let glyph = match self.tree_expand.next() {
            crate::gui::state::TreeExpand::SessionsOnly => "expand-sessions",
            crate::gui::state::TreeExpand::All => "expand-all",
            crate::gui::state::TreeExpand::Collapsed => "collapse-all",
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
                snap: false,
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
            Space::new().width(Length::Fixed(0.0)).into()
        };

        container(
            row![
                container(pillset)
                    .height(Length::Fill)
                    .align_y(iced::Alignment::Center),
                Space::new().width(Length::Fill),
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
            .map(|(i, p)| (i, p.name.clone(), p.path.clone()))
            .collect();
        for (pi, pname, ppath) in projects {
            let expanded = !self.collapsed.contains(&pi);
            let is_git = crate::git::is_repo(&ppath);
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
                is_git,
                proj_rollup,
                self.blink_tick,
                self.attention_pulse(),
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
                    p.scripts
                        .run
                        .as_deref()
                        .is_some_and(|s| !s.trim().is_empty())
                });
                let git_suffix = self
                    .git_state
                    .lock()
                    .ok()
                    .and_then(|g| g.get(&w.path).and_then(crate::git::git_state_suffix));
                let wt_el = worktree_row(
                    pi,
                    wi,
                    &wname,
                    &w.branch,
                    active_wt,
                    w.is_main,
                    is_git,
                    hovered,
                    wt_expanded,
                    has_run,
                    wt_rollup,
                    self.blink_tick,
                    self.attention_pulse(),
                    &self.app.available_agents,
                    git_suffix,
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
                            self.attention_pulse(),
                        ));
                    }
                }
            }
        }
        col.into()
    }

    /// Session indices grouped by liveness for the activity view, in the exact
    /// top-to-bottom order they render: `(waiting, running, idle)`. Idle folds
    /// in exited sessions. Shared with keyboard navigation so `mod+1..9` maps to
    /// the same visual order the sidebar shows.
    fn activity_session_groups(&self) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
        use super::activity::ActivityState;
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
        (waiting, running, idle)
    }

    /// Session indices in the order they appear in the sidebar for the current
    /// `sidebar_view`, honoring collapse state in the tree. Drives `mod+1..9`
    /// while the agent grid is closed so the shortcut follows what's on screen.
    pub fn visible_session_order(&self) -> Vec<usize> {
        match self.sidebar_view {
            SidebarView::Activity => {
                let (mut order, running, idle) = self.activity_session_groups();
                order.extend(running);
                order.extend(idle);
                order
            }
            SidebarView::Tree => self.tree_session_order(),
            // The terminal sidebar lists no agent sessions; fall back to raw
            // session order so the shortcut still targets something sane.
            SidebarView::Terminal => (0..self.app.sessions.len()).collect(),
        }
    }

    /// Session indices in the top-to-bottom order `tree_view` renders them,
    /// skipping sessions hidden under a collapsed project or worktree.
    fn tree_session_order(&self) -> Vec<usize> {
        let mut order = Vec::new();
        for (pi, _p) in self.app.store.projects.iter().enumerate() {
            if self.collapsed.contains(&pi) {
                continue;
            }
            let wts: &[Worktree] = if pi == self.app.proj_idx {
                &self.app.worktrees
            } else {
                self.wt_cache.get(&pi).map(|v| v.as_slice()).unwrap_or(&[])
            };
            for (wi, w) in wts.iter().enumerate() {
                if self.collapsed_wt.contains(&(pi, wi)) {
                    continue;
                }
                for (si, s) in self.app.sessions.iter().enumerate() {
                    if s.wt_path == w.path {
                        order.push(si);
                    }
                }
            }
        }
        order
    }

    /// Flat activity-stream rendering of every session across every project /
    /// worktree, grouped by liveness (`running` / `idle` / `worktrees · no
    /// sessions`).
    fn activity_view(&self) -> Element<'_, Msg> {
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

        let (waiting, running, idle) = self.activity_session_groups();

        // All worktrees across all projects, listed `project / worktree`.
        // The row shows the session count and lets the user spawn new
        // sessions inline regardless of whether sessions already exist.
        let mut worktree_rows: Vec<(usize, usize, String, String, bool, bool, usize)> = Vec::new();
        for (pi, p) in self.app.store.projects.iter().enumerate() {
            let is_git = crate::git::is_repo(&p.path);
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
                worktree_rows.push((pi, wi, p.name.clone(), wname, w.is_main, is_git, count));
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
                col =
                    col.push(self.activity_row_wrapped(si, &session_wnames[si], now, &project_idx));
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
            for (pi, wi, pname, wname, is_main, is_git, count) in worktree_rows {
                let hovered = self.hovered_wt == Some((pi, wi));
                let row_el = worktree_activity_row(
                    pi,
                    wi,
                    &pname,
                    &wname,
                    is_main,
                    is_git,
                    count,
                    hovered,
                    &self.app.available_agents,
                );
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
            self.attention_pulse(),
            &self.app.available_agents,
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
    fn grid_workspace(&self) -> Element<'_, Msg> {
        use super::metrics::grid_layout;

        let n = self.tile_order.len();
        if n == 0 {
            return empty_workspace();
        }
        let (grid_cols, grid_rows) = grid_layout(n);

        // Lay out columns-of-tiles (not rows-of-tiles): each column stacks only
        // the tiles it actually has, so a column left with a single tile (e.g.
        // the odd one out in a 2×2 grid holding 3 sessions) spans the full
        // workspace height instead of leaving an empty cell beside it.
        let mut cols_row = row![].spacing(1).height(Length::Fill);
        for col_idx in 0..grid_cols {
            let mut col_el = column![]
                .spacing(1)
                .width(Length::Fill)
                .height(Length::Fill);
            for row_idx in 0..grid_rows {
                let tile_idx = row_idx * grid_cols + col_idx;
                if tile_idx >= n {
                    continue;
                }
                let si = self.tile_order[tile_idx];
                let el: Element<'_, Msg> = if si < self.app.sessions.len() {
                    self.grid_tile(tile_idx, si, &self.app.sessions[si])
                } else {
                    // Stale index: render blank until KillSession prunes tile_order.
                    container(Space::new().width(Length::Fill).height(Length::Fill))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(|_| container::Style {
                            background: Some(Background::Color(c::BG())),
                            ..Default::default()
                        })
                        .into()
                };
                col_el = col_el.push(el);
            }
            cols_row = cols_row.push(col_el);
        }

        // Inter-tile gaps: set the container background to BORDER_SOFT;
        // 1px spacing in column/row lets that background show through.
        let grid = container(cols_row)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BORDER_SOFT())),
                ..Default::default()
            });

        // The session launcher is opened from the "+" segment of the agent-view
        // combo in the appbar (see `appbar`); the grid workspace itself is just
        // the tile grid with no floating action button.
        grid.into()
    }

    fn workspace(&self) -> Element<'_, Msg> {
        if self.grid_view {
            return self.grid_workspace();
        }
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
                    snap: false,
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
                snap: false,
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
                    snap: false,
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
                wts.iter()
                    .position(|w| w.path == s.wt_path)
                    .map(|wi| (pi, wi))
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
            _ => Space::new().width(0).into(),
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

    fn grid_tile(&self, tile_order_idx: usize, si: usize, s: &Session) -> Element<'_, Msg> {
        use super::metrics::TILE_HEAD_H;

        let focused = self.grid_focused == Some(si);
        let is_drag_src = self
            .grid_drag
            .as_ref()
            .map_or(false, |d| d.source_idx == tile_order_idx);
        let is_drop_zone = self.grid_drag.as_ref().map_or(false, |d| {
            d.hover_idx == tile_order_idx && d.source_idx != tile_order_idx
        });

        // ── tile header ────────────────────────────────────────────────
        let running = matches!(
            *s.status.lock().unwrap_or_else(|e| e.into_inner()),
            SessionStatus::Running
        );
        let dot_color = if running { c::GREEN() } else { c::FG_MUTE() };
        let tile_btn = |icon_name, msg| {
            button(
                container(icon(icon_name, 10.0, c::FG_MUTE()))
                    .center_x(18)
                    .center_y(18),
            )
            .on_press(msg)
            .padding(0)
            .style(|_, _| button::Style {
                background: None,
                text_color: c::FG_MUTE(),
                border: Border::default(),
                shadow: Shadow::default(),
                snap: false,
            })
        };
        let confirming_kill = self.pending_kill == Some(si);
        let kill_btn = button(
            container(icon(
                "trash",
                10.0,
                if confirming_kill {
                    c::RED()
                } else {
                    c::FG_MUTE()
                },
            ))
            .center_x(18)
            .center_y(18),
        )
        .on_press(if confirming_kill {
            Msg::KillSession(si)
        } else {
            Msg::RequestKillSession(si)
        })
        .padding(0)
        .style(move |_, _| button::Style {
            background: None,
            text_color: if confirming_kill {
                c::RED()
            } else {
                c::FG_MUTE()
            },
            border: Border::default(),
            shadow: Shadow::default(),
            snap: false,
        });
        // Shortcut-number hint: the first 9 tiles (tile_order positions 0..9)
        // are reachable via the platform modifier + 1..9 (see
        // select_visible_session). Show the full chord so the key is
        // unambiguous. On macOS we render the ⌘ glyph as an SVG icon (the
        // bundled font has no U+2318); elsewhere the modifier is spelled out as
        // text via platform_mod_label(), matching the overlay.
        let num_hint: Element<'_, Msg> = if tile_order_idx < 9 {
            let n = tile_order_idx + 1;
            let hint_color = if focused { c::FG_DIM() } else { c::FG_MUTE() };
            let inner: Element<'_, Msg> = if cfg!(target_os = "macos") {
                row![
                    icon("command", 9.0, hint_color),
                    text(n.to_string())
                        .font(MONO_FONT)
                        .size(9)
                        .color(hint_color),
                ]
                .spacing(1)
                .align_y(iced::Alignment::Center)
                .into()
            } else {
                text(format!("{}+{}", platform_mod_label(), n))
                    .font(MONO_FONT)
                    .size(9)
                    .color(hint_color)
                    .into()
            };
            container(inner)
                .padding(Padding::from([1, 4]))
                .style(|_| container::Style {
                    background: Some(Background::Color(c::BG())),
                    border: Border {
                        color: c::BORDER(),
                        width: 1.0,
                        radius: 3.0.into(),
                    },
                    ..Default::default()
                })
                .into()
        } else {
            Space::new().width(0).into()
        };
        let header_row = row![
            dot(dot_color),
            text(s.agent.label())
                .font(UI_BOLD)
                .size(10)
                .color(c::FG_DIM()),
            text("·").size(10).color(c::FG_MUTE()),
            text(s.project.clone()).size(10).color(c::FG_MUTE()),
            text("·").size(10).color(c::FG_MUTE()),
            text(s.branch.clone()).size(10).color(c::FG_MUTE()),
            Space::new().width(Length::Fill),
            num_hint,
            tile_btn("zen", Msg::GridTileZen(si)),
            kill_btn,
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center)
        .padding(Padding::from([0, 6]));

        let header_bg = if focused { c::BG_HL() } else { c::BG_STRIP() };
        let header = iced::widget::mouse_area(
            container(header_row)
                .height(TILE_HEAD_H)
                .align_y(iced::Alignment::Center)
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(Background::Color(header_bg)),
                    ..Default::default()
                }),
        )
        .on_press(Msg::GridDragStart(tile_order_idx));

        // ── tile body (header + PTY) ───────────────────────────────────
        let tile_body: Element<'_, Msg> = column![
            header,
            divider_h(c::BORDER_SOFT()),
            // Reuse the existing pty() renderer with PtyPane::Tile(si).
            // Selection returns None automatically (focused_input_pane won't
            // match PtyPane::Tile) — that's the intentional no-selection policy.
            self.pty(PtyPane::Tile(si), s),
        ]
        .height(Length::Fill)
        .into();

        // Drop-zone overlay: cyan inset when this tile is the drag target.
        let with_drop: Element<'_, Msg> = if is_drop_zone {
            stack![
                tile_body,
                container(Space::new().width(Length::Fill).height(Length::Fill))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|_| container::Style {
                        border: Border {
                            color: c::CYAN(),
                            width: 1.5,
                            radius: Radius::from(0.0),
                        },
                        background: Some(Background::Color(Color {
                            a: 0.06,
                            ..c::CYAN()
                        })),
                        ..Default::default()
                    }),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            tile_body
        };

        // Drag-source dim: semi-transparent BG overlay to show "lifted" state.
        let with_dim: Element<'_, Msg> = if is_drag_src {
            stack![
                with_drop,
                container(Space::new().width(Length::Fill).height(Length::Fill))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|_| {
                        let mut bg = c::BG();
                        bg.a = 0.72;
                        container::Style {
                            background: Some(Background::Color(bg)),
                            ..Default::default()
                        }
                    }),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            with_drop
        };

        // Waiting-for-input: solid amber 1.5px border (no blink).
        // Overrides the focused-cyan border — attention wins.
        use super::activity::ActivityState;
        let waiting = matches!(self.activity_state(s), ActivityState::WaitingForInput);
        let (border_color, border_width) = if waiting {
            (c::AMBER(), 1.5f32)
        } else if focused {
            (c::CYAN(), 1.5f32)
        } else {
            (Color::TRANSPARENT, 0.0)
        };

        // Full-tile scrim overlay when waiting for input.
        let with_scrim: Element<'_, Msg> = if waiting {
            // Opacity pulse (~2s round trip): alpha eases between 1.0 and
            // 0.7, driven by the attention `Animation`.
            let text_alpha = 1.0 - 0.3 * self.attention_pulse();
            let amber_pulsed = Color {
                a: text_alpha,
                ..c::AMBER()
            };

            let sub_line: String = if tile_order_idx < 9 {
                format!(
                    "click to respond · {}+{}",
                    platform_mod_label(),
                    tile_order_idx + 1
                )
            } else {
                "click to respond".to_string()
            };

            let scrim_content: Element<'_, Msg> = container(
                column![
                    text("N E E D S   A T T E N T I O N")
                        .font(UI_BOLD)
                        .size(20)
                        .color(amber_pulsed),
                    text(sub_line).font(UI_FONT).size(10).color(c::FG_MUTE()),
                ]
                .spacing(8)
                .align_x(iced::Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::Alignment::Center)
            .align_y(iced::Alignment::Center)
            .style(|_| container::Style {
                // Darker theme-derived scrim: BG_STRIP is the theme's deepest
                // surface, so the wash tracks the active theme (iced has no
                // backdrop blur, so opacity does the softening).
                background: Some(Background::Color(Color {
                    a: 0.92,
                    ..c::BG_STRIP()
                })),
                ..Default::default()
            })
            .into();

            // Wrap in mouse_area so clicking the scrim focuses/acknowledges the tile.
            let focus_msg = Msg::GridDragStart(tile_order_idx);
            let clickable_scrim: Element<'_, Msg> = iced::widget::mouse_area(scrim_content)
                .on_press(focus_msg)
                .into();

            stack![with_dim, clickable_scrim]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            with_dim
        };

        // on_enter fires even while a button is held — the GridDragHover handler
        // ignores it when no drag is active.
        iced::widget::mouse_area(
            container(with_scrim)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(Background::Color(c::BG())),
                    border: Border {
                        color: border_color,
                        width: border_width,
                        radius: Radius::from(0.0),
                    },
                    ..Default::default()
                }),
        )
        .on_enter(Msg::GridDragHover(tile_order_idx))
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

        let mut left = row![
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

        if self.app.skip_permissions_enabled() {
            let mut chip_border = c::YELLOW();
            chip_border.a = 0.45;
            left = left.push(
                container(text("bypass").size(10).color(c::YELLOW()))
                    .padding(Padding::from([1, 6]))
                    .style(move |_| container::Style {
                        border: Border {
                            color: chip_border,
                            width: 1.0,
                            radius: Radius::from(3.0),
                        },
                        ..Default::default()
                    }),
            );
        }

        let toast: Element<'_, Msg> = match &self.app.toast {
            Some(t) => {
                let color = match t.kind {
                    crate::app::ToastKind::Error => c::RED(),
                    crate::app::ToastKind::Info => c::GREEN(),
                };
                text(t.message.clone()).size(11).color(color).into()
            }
            None => Space::new().width(0).into(),
        };

        let modifier = platform_mod_label();
        // Pull the key label from the registry (single source of truth).
        let overlay_key = SHORTCUTS
            .iter()
            .find(|d| d.action == Some(GlobalShortcut::ShortcutOverlay))
            .map(|d| d.display_keys)
            .unwrap_or("/");
        let shortcuts_chip = button(
            text(format!("{modifier}+{overlay_key}  shortcuts"))
                .size(11)
                .color(c::FG_DIM()),
        )
        .padding(Padding::from([0, 6]))
        .on_press(Msg::OpenShortcutOverlay)
        .style(|_, status| button::Style {
            background: None,
            text_color: if matches!(status, button::Status::Hovered) {
                c::FG()
            } else {
                c::FG_DIM()
            },
            ..Default::default()
        });

        let right = row![
            shortcuts_chip,
            Space::new().width(12),
            text(format!("v{}", env!("CARGO_PKG_VERSION")))
                .size(11)
                .color(c::FG_DIM()),
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
                note,
            } => self.input_modal(title, buffer, note.as_deref()),
            Modal::Confirm {
                title,
                prompt,
                destructive,
                kind,
            } => self.confirm_modal(title, prompt, *destructive, kind),
            Modal::AddProject {
                step,
                path,
                dir_sel,
                name,
                git,
                init_git,
                note,
            } => {
                self.add_project_modal(*step, path, *dir_sel, name, git, *init_git, note.as_deref())
            }
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
                follow_system,
                ..
            } => self.theme_picker_modal(*sel_dark, *sel_light, *tab, *follow_system),
            Modal::Settings => self.settings_modal(),
            Modal::ShortcutOverlay => self.shortcut_overlay_modal(),
            Modal::Updating => self.updating_modal(),
            Modal::Teardown => self.teardown_modal(),
            Modal::ScriptsEditor => self.scripts_editor_modal(),
            Modal::Onboarding {
                step,
                path,
                dir_sel,
                name,
                note,
                tab,
                sel_dark,
                sel_light,
                agent_sel,
                backend_tmux,
                perms_skip,
                ..
            } => self.onboarding_modal(
                *step,
                path,
                *dir_sel,
                name.as_deref(),
                note.as_deref(),
                *tab,
                *sel_dark,
                *sel_light,
                *agent_sel,
                *backend_tmux,
                *perms_skip,
            ),
            Modal::SessionLauncher {
                proj,
                wt,
                agent,
                col,
            } => self.session_launcher_modal(*proj, *wt, *agent, *col),
            _ => Space::new().width(0).into(),
        };

        container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::SCRIM())),
                ..Default::default()
            })
            .into()
    }

    fn input_modal<'a>(
        &'a self,
        title: &'a str,
        buffer: &'a str,
        note: Option<&'a str>,
    ) -> Element<'a, Msg> {
        let field = text_input("", buffer)
            .id(modal_input_id())
            .font(UI_FONT)
            .size(13)
            .padding(Padding::from([8, 12]))
            .on_input(Msg::InputPathChanged)
            .on_submit(Msg::ModalSubmit)
            .style(input_field_style);

        let mut body =
            column![text(title.to_string()).size(13).color(c::MAGENTA()), field].spacing(12);

        if let Some(note) = note {
            body = body.push(text(note.to_string()).size(12).color(c::RED()));
        }

        body = body
            .push(
                text("enter to confirm · esc to cancel")
                    .size(11)
                    .color(c::FG_MUTE()),
            )
            .push(Space::new().height(4))
            .push(
                row![
                    Space::new().width(Length::Fill),
                    modal_action("cancel", ModalBtn::Plain, Msg::ModalCancel),
                    modal_action("submit", ModalBtn::Primary, Msg::ModalSubmit),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );

        modal_panel(body.into(), 480.0, c::MAGENTA())
    }

    /// The windowed directory-match list shared by the add-project pick step
    /// and the onboarding project step: up to `window` rows that scroll to
    /// keep the selection visible, with muted "↑N/↓N more" hints when entries
    /// sit above or below the window. Results are memoized in `dir_cache`
    /// because `view()` runs every tick.
    fn dir_matches(
        &self,
        buffer: &str,
        dir_sel: usize,
        window: usize,
        on_pick: fn(String) -> Msg,
    ) -> Element<'_, Msg> {
        let entries = {
            let mut cache = self.dir_cache.borrow_mut();
            match cache.as_ref() {
                Some((k, v)) if k == buffer => v.clone(),
                _ => {
                    let v = crate::app::list_dirs(buffer);
                    *cache = Some((buffer.to_string(), v.clone()));
                    v
                }
            }
        };
        let total = entries.len();
        let shown = total.min(window);
        // Scroll the window so dir_sel stays visible.
        let start = dir_sel
            .saturating_sub(window - 1)
            .min(total.saturating_sub(window));
        let above = start;
        let below = total.saturating_sub(start + shown);
        let rows =
            shown + usize::from(above > 0) + usize::from(below > 0) + usize::from(total == 0);
        let mut matches_col = Column::new()
            .spacing(0)
            .height(Length::Fixed(rows.max(1) as f32 * ROW_H));
        if entries.is_empty() {
            matches_col = matches_col.push(
                container(text("no matches").size(12).color(c::FG_MUTE()))
                    .height(ROW_H)
                    .padding(Padding::from([0, 10]))
                    .align_y(iced::Alignment::Center),
            );
        } else {
            let more = |n: usize, arrow: char| {
                container(
                    text(format!("{arrow}{n} more"))
                        .size(11)
                        .color(c::FG_MUTE()),
                )
                .height(ROW_H)
                .padding(Padding::from([0, 10]))
                .align_y(iced::Alignment::Center)
            };
            if above > 0 {
                matches_col = matches_col.push(more(above, '↑'));
            }
            for (i, path) in entries.into_iter().skip(start).take(shown).enumerate() {
                let active = start + i == dir_sel;
                // Rows show just the directory name — the buffer above already
                // carries the parent path, and full paths wrap illegibly.
                let label = format!("{}/", crate::app::path_basename(&path));
                matches_col = matches_col.push(modal_list_row(
                    text(label)
                        .font(UI_FONT)
                        .size(12)
                        .color(if active { c::FG() } else { c::FG_DIM() })
                        .wrapping(iced::widget::text::Wrapping::None),
                    active,
                    on_pick(path),
                ));
            }
            if below > 0 {
                matches_col = matches_col.push(more(below, '↓'));
            }
        }
        container(matches_col)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP())),
                border: Border {
                    color: c::BORDER_SOFT(),
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                ..Default::default()
            })
            .into()
    }

    /// The two-step add-project modal: pick a folder (native picker, drop, or
    /// typed path), then confirm the details with the git probe inline.
    #[allow(clippy::too_many_arguments)]
    fn add_project_modal<'a>(
        &'a self,
        step: AddProjectStep,
        path: &'a str,
        dir_sel: usize,
        name: &'a str,
        git: &'a GitProbe,
        init_git: bool,
        note: Option<&'a str>,
    ) -> Element<'a, Msg> {
        let accent = c::MAGENTA();
        let step_no = match step {
            AddProjectStep::PickSource => 1,
            AddProjectStep::Details => 2,
        };
        let header = row![
            text("add project").size(13).color(accent),
            Space::new().width(Length::Fill),
            text(format!("step {step_no} of 2"))
                .size(11)
                .color(c::FG_MUTE()),
        ]
        .align_y(iced::Alignment::Center);

        let mut body = column![header].spacing(12);

        match step {
            AddProjectStep::PickSource => {
                // Hero action: a full-width primary Browse button with the
                // drop affordance as its caption.
                let accent_soft = Color { a: 0.45, ..accent };
                let browse = button(
                    container(
                        text(if self.picker_open {
                            "waiting for the folder picker…"
                        } else {
                            "browse for folder…"
                        })
                        .size(13),
                    )
                    .width(Length::Fill)
                    .align_x(iced::Alignment::Center),
                )
                .on_press(Msg::AddProjectBrowse)
                .width(Length::Fill)
                .padding(Padding::from([10, 12]))
                .style(move |_, status| {
                    let hovered = matches!(status, button::Status::Hovered);
                    button::Style {
                        background: Some(Background::Color(if hovered {
                            c::BG_HOVER()
                        } else {
                            c::BG_HL()
                        })),
                        text_color: c::FG(),
                        border: Border {
                            color: if hovered { accent } else { accent_soft },
                            width: 1.0,
                            radius: Radius::from(5.0),
                        },
                        shadow: Shadow::default(),
                        snap: false,
                    }
                });
                let drop_hint = container(
                    text("or drop a folder anywhere in this window")
                        .size(11)
                        .color(c::FG_MUTE()),
                )
                .width(Length::Fill)
                .align_x(iced::Alignment::Center);

                let or_divider = row![
                    container(divider_h(c::BORDER_SOFT())).width(Length::Fill),
                    text("or type a path").size(11).color(c::FG_MUTE()),
                    container(divider_h(c::BORDER_SOFT())).width(Length::Fill),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center);

                let path_input = text_input("~/code/my-repo", path)
                    .id(modal_input_id())
                    .font(UI_FONT)
                    .size(13)
                    .padding(Padding::from([8, 12]))
                    .on_input(Msg::AddProjectPathChanged)
                    .on_submit(Msg::AddProjectChooseTyped)
                    .style(input_field_style);

                body = body
                    .push(Space::new().height(2))
                    .push(browse)
                    .push(drop_hint)
                    .push(Space::new().height(2))
                    .push(or_divider)
                    .push(path_input)
                    .push(self.dir_matches(path, dir_sel, 6, Msg::ModalPickDir));

                if let Some(note) = note {
                    body = body.push(text(note.to_string()).size(12).color(c::RED()));
                }
                body = body
                    .push(
                        text("tab complete · ↑↓ select · enter continue · esc cancel")
                            .size(11)
                            .color(c::FG_MUTE()),
                    )
                    .push(Space::new().height(4))
                    .push(
                        row![
                            Space::new().width(Length::Fill),
                            modal_action("cancel", ModalBtn::Plain, Msg::ModalCancel),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                    );
            }
            AddProjectStep::Details => {
                let chip = container(
                    row![
                        icon("folder", 14.0, c::FG_DIM()),
                        text(path.to_string())
                            .size(12)
                            .color(c::FG())
                            .wrapping(iced::widget::text::Wrapping::None),
                        Space::new().width(Length::Fill),
                        modal_action("change", ModalBtn::Plain, Msg::AddProjectChangeSource),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                )
                .width(Length::Fill)
                .padding(Padding::from([6, 10]))
                .style(|_| container::Style {
                    background: Some(Background::Color(c::BG_STRIP())),
                    border: Border {
                        color: c::BORDER(),
                        width: 1.0,
                        radius: Radius::from(4.0),
                    },
                    ..Default::default()
                });

                let badge: Element<'a, Msg> = match git {
                    GitProbe::Repo { branch } => row![
                        icon("git", 14.0, c::GREEN()),
                        text(format!("git repository · branch {branch}"))
                            .size(12)
                            .color(c::GREEN()),
                    ]
                    .spacing(7)
                    .align_y(iced::Alignment::Center)
                    .into(),
                    GitProbe::NotRepo => row![
                        icon("no-git", 14.0, c::AMBER()),
                        text("not a git repository").size(12).color(c::AMBER()),
                    ]
                    .spacing(7)
                    .align_y(iced::Alignment::Center)
                    .into(),
                };

                // The placeholder is the default (folder basename); typing
                // overrides it without having to clear pre-filled text.
                let default_name = crate::app::path_basename(path);
                let name_input = text_input(&default_name, name)
                    .id(modal_name_id())
                    .font(UI_FONT)
                    .size(13)
                    .padding(Padding::from([8, 12]))
                    .on_input(Msg::AddProjectNameChanged)
                    .on_submit(Msg::AddProjectSubmit)
                    .style(input_field_style);

                body = body
                    .push(text("folder").size(11).color(c::FG_MUTE()))
                    .push(chip)
                    .push(badge)
                    .push(
                        row![
                            text("name").size(11).color(c::FG_MUTE()),
                            Space::new().width(Length::Fill),
                            text(format!("empty uses '{default_name}'"))
                                .size(11)
                                .color(c::FG_MUTE()),
                        ]
                        .align_y(iced::Alignment::Center),
                    )
                    .push(name_input);

                if matches!(git, GitProbe::NotRepo) {
                    body = body.push(modal_checkbox(
                        "initialize git repository".into(),
                        init_git,
                        accent,
                        Some(Msg::AddProjectToggleInitGit),
                    ));
                    if !init_git {
                        body = body.push(
                            text("sessions will run directly in the project folder, no worktrees")
                                .size(11)
                                .color(c::FG_MUTE()),
                        );
                    }
                }
                if let Some(note) = note {
                    body = body.push(text(note.to_string()).size(12).color(c::RED()));
                }
                body = body
                    .push(text("enter add · esc back").size(11).color(c::FG_MUTE()))
                    .push(Space::new().height(4))
                    .push(
                        row![
                            Space::new().width(Length::Fill),
                            modal_action("cancel", ModalBtn::Plain, Msg::ModalCancel),
                            modal_action("add project", ModalBtn::Primary, Msg::AddProjectSubmit),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                    );
            }
        }

        modal_panel(body.into(), 640.0, accent)
    }

    fn confirm_modal<'a>(
        &'a self,
        title: &'a str,
        prompt: &'a str,
        destructive: bool,
        kind: &'a ConfirmKind,
    ) -> Element<'a, Msg> {
        let accent = if destructive { c::RED() } else { c::MAGENTA() };
        let confirm_label = match kind {
            ConfirmKind::Quit => "quit",
            _ if destructive => "remove",
            _ => "confirm",
        };
        let body = column![
            text(title.to_string()).size(13).color(accent),
            text(prompt.to_string())
                .size(13)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::Word),
            Space::new().height(8),
            row![
                Space::new().width(Length::Fill),
                modal_action("cancel", ModalBtn::Plain, Msg::ModalConfirm(false)),
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
        use iced::widget::progress_bar;
        use iced::widget::progress_bar::Style as ProgressStyle;

        let accent = c::RED();
        let total = worktrees.len();
        let prompt = if total == 0 {
            format!("'{name}' will be unregistered from grove. files on disk stay put.")
        } else {
            format!(
                "'{name}' will be unregistered from grove. non-main worktrees stay on disk unless you opt in below."
            )
        };
        let session_note = "running sessions for this project will be stopped.";

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
                "delete 1 non-main worktree from disk".to_string()
            } else {
                format!("delete {total} non-main worktrees from disk")
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
            body = body.push(Space::new().height(2)).push(cb);
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
                format!("removing {} of {}: {}", done + 1, total, current)
            };
            body = body
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
            body = body
                .push(Space::new().height(4))
                .push(
                    text("press y to remove · space toggles delete-from-disk · esc cancels")
                        .size(11)
                        .color(c::FG_MUTE()),
                )
                .push(
                    row![
                        Space::new().width(Length::Fill),
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
            Space::new().height(8),
            row![
                Space::new().width(Length::Fill),
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
            None => return Space::new().width(0).into(),
        };
        let wt_name = crate::app::path_basename(&td.wt_path);
        let done = matches!(td.stage, TeardownStage::Done { .. });
        let running = matches!(td.stage, TeardownStage::RunningScript);

        let mut body = column![text(format!("delete worktree / {wt_name}"))
            .size(13)
            .color(c::RED()),]
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
                Space::new().width(Length::Fill),
                modal_action("close", ModalBtn::Primary, Msg::ModalCancel),
            ]
        } else if running {
            // Let the user proceed without waiting for a hung teardown script.
            row![
                Space::new().width(Length::Fill),
                modal_action("skip & remove", ModalBtn::Plain, Msg::ModalCancel),
            ]
        } else {
            row![Space::new().width(Length::Fill)]
        }
        .spacing(8)
        .align_y(iced::Alignment::Center);

        body = body.push(Space::new().height(4)).push(buttons);
        modal_panel(body.into(), 560.0, c::RED())
    }

    fn scripts_editor_modal(&self) -> Element<'_, Msg> {
        use super::state::ScriptField;
        let ed = match &self.scripts_editor {
            Some(ed) => ed,
            None => return Space::new().width(0).into(),
        };

        let field = |label: &str, desc: &str, placeholder: &str, content, which: ScriptField| {
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
                        Status::Focused { .. } => c::CYAN(),
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
                "runs once when a new worktree is created, inside the new worktree's directory. \
                 use it to install dependencies, copy ignored env files, or start the services \
                 an agent needs before you begin working.",
                "npm install",
                &ed.setup,
                ScriptField::Setup,
            ),
            field(
                "run",
                "runs on demand when you press the play button (worktree row or session header). \
                 it opens an interactive terminal tab, so it suits dev servers, test watchers, \
                 or any command you want to watch and interact with.",
                "npm run dev",
                &ed.run,
                ScriptField::Run,
            ),
            field(
                "teardown",
                "runs when you delete the worktree, before it is removed from disk. use it to \
                 stop services, tear down databases, or clean up anything setup created. \
                 deletion proceeds once it exits.",
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
                background: Background::Color(Color::TRANSPARENT),
                border: Border::default(),
            },
        };
        let scroll_area = container(
            scrollable(fields)
                .height(Length::Shrink)
                .direction(Direction::Vertical(
                    Scrollbar::new().width(0).scroller_width(0),
                ))
                .style(move |theme, status| iced::widget::scrollable::Style {
                    container: container::Style::default(),
                    vertical_rail: invisible_rail,
                    horizontal_rail: invisible_rail,
                    gap: None,
                    ..iced::widget::scrollable::default(theme, status)
                }),
        )
        .max_height(480.0);

        let body = column![
            text(format!("scripts / {}", ed.project_name))
                .size(13)
                .color(c::CYAN()),
            text("shell snippets shared by every worktree of this project, run via $SHELL -lc. leave a field blank to disable that step.")
                .size(11)
                .color(c::FG_MUTE())
                .wrapping(iced::widget::text::Wrapping::Word),
            scroll_area,
            row![
                Space::new().width(Length::Fill),
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
            text("use tmux for new sessions? existing sessions keep their current backend.")
                .size(13)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::Word),
            Space::new().height(8),
            row![
                Space::new().width(Length::Fill),
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
                Space::new().width(Length::Fill),
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
            Space::new().height(8),
            row![
                modal_action("default", ModalBtn::Plain, Msg::AgentPickerToggleDefault),
                Space::new().width(Length::Fill),
                modal_action("cancel", ModalBtn::Plain, Msg::ModalCancel),
                modal_action("launch", ModalBtn::Primary, Msg::AgentPickerSubmit),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        modal_panel(body.into(), 500.0, c::MAGENTA())
    }

    /// Agent View "+ New session" launcher: three Miller columns (project →
    /// worktree → agent) and a footer breadcrumb + Start button. See
    /// `mock.html` for the approved visual source of truth.
    fn session_launcher_modal<'a>(
        &'a self,
        proj: usize,
        wt: usize,
        agent: usize,
        col: u8,
    ) -> Element<'a, Msg> {
        // ── Column 1: projects ──────────────────────────────────────────
        let proj_focused = col == 0;
        let mut proj_list = Column::new().spacing(0);
        for (i, p) in self.app.store.projects.iter().enumerate() {
            let count = self.launcher_worktrees(i).len();
            let active = i == proj;
            let bright = active && proj_focused;
            let label_row = row![
                text(p.name.clone())
                    .size(12)
                    .color(if bright { c::FG() } else { c::FG_DIM() }),
                Space::new().width(Length::Fill),
                text(count.to_string()).size(11).color(c::FG_MUTE()),
            ]
            .align_y(iced::Alignment::Center);
            proj_list = proj_list.push(launcher_row(
                label_row,
                active,
                proj_focused,
                Msg::LauncherSelectProject(i),
            ));
        }

        // ── Column 2: worktrees ─────────────────────────────────────────
        let worktrees = self.launcher_worktrees(proj);
        let wt_focused = col == 1;
        let mut wt_list = Column::new().spacing(0);
        for (i, w) in worktrees.iter().enumerate() {
            let active = i == wt;
            let bright = active && wt_focused;
            let name = if w.branch.is_empty() {
                crate::app::path_basename(&w.path)
            } else {
                w.branch.clone()
            };
            let tag = if w.is_main { "main" } else { "" };
            let name_el = single_line(
                text(truncate_ellipsis(&name, 28))
                    .size(12)
                    .color(if bright { c::FG() } else { c::FG_DIM() })
                    .wrapping(iced::widget::text::Wrapping::None),
                12.0,
            );
            let label_row = row![
                container(name_el).width(Length::Fill).clip(true),
                text(tag.to_string()).size(10).color(c::GREEN()),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center);
            wt_list = wt_list.push(launcher_row(
                label_row,
                active,
                wt_focused,
                Msg::LauncherSelectWorktree(i),
            ));
        }
        // "+ New worktree…" affordance.
        let add_row = row![text("+ new worktree…").size(12).color(c::MAGENTA())]
            .align_y(iced::Alignment::Center);
        wt_list = wt_list.push(modal_list_row(add_row, false, Msg::LauncherNewWorktree));

        // ── Column 3: agents + options ──────────────────────────────────
        let agent_focused = col == 2;
        let mut agent_list = Column::new().spacing(0);
        for (i, ag) in self.app.available_agents.iter().enumerate() {
            let active = i == agent;
            let bright = active && agent_focused;
            let is_default = self.app.store.default_agent == Some(*ag);
            let label_row = row![
                text(ag.label().to_string()).size(12).color(if bright {
                    c::FG()
                } else {
                    c::FG_DIM()
                }),
                Space::new().width(Length::Fill),
                text(if is_default { "default" } else { "" })
                    .size(11)
                    .color(c::FG_MUTE()),
            ]
            .align_y(iced::Alignment::Center);
            agent_list = agent_list.push(launcher_row(
                label_row,
                active,
                agent_focused,
                Msg::LauncherSelectAgent(i),
            ));
        }
        let agent_col = agent_list;

        // Fixed-height columns so the modal keeps the mock's proportions.
        let col_h = Length::Fixed(300.0);
        let make_col = |title: &'static str, body: Element<'a, Msg>, focused: bool| {
            column![
                text(title)
                    .size(10)
                    .color(if focused { c::CYAN() } else { c::FG_MUTE() }),
                container(body).height(col_h).width(Length::Fill),
            ]
            .spacing(6)
            .width(Length::FillPortion(1))
        };
        let cols = row![
            make_col("PROJECT", proj_list.into(), col == 0),
            make_col("WORKTREE", wt_list.into(), col == 1),
            make_col("AGENT", agent_col.into(), col == 2),
        ]
        .spacing(12);

        // ── Footer: breadcrumb + Start ──────────────────────────────────
        let pname = self
            .app
            .store
            .projects
            .get(proj)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let branch = worktrees
            .get(wt)
            .map(|w| {
                if w.branch.is_empty() {
                    crate::app::path_basename(&w.path)
                } else {
                    w.branch.clone()
                }
            })
            .unwrap_or_default();
        let ag_label = self
            .app
            .available_agents
            .get(agent)
            .map(|a| a.label().to_string())
            .unwrap_or_default();
        let crumb = crate::gui::launcher::breadcrumb(&pname, &branch, &ag_label);
        let crumb_el = single_line(
            text(truncate_middle(&crumb, 60))
                .size(12)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::None),
            12.0,
        );
        let footer = row![
            container(crumb_el).width(Length::Fill).clip(true),
            text("←/→ or h/l columns · ↑/↓ or j/k move · space default · enter start · esc")
                .size(10)
                .color(c::FG_MUTE()),
            modal_action("start session", ModalBtn::Primary, Msg::LauncherStart),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);

        // Header: title on the left, close (✕) button on the right.
        let close_btn = button(
            container(icon("close", 12.0, c::FG_MUTE()))
                .center_x(20)
                .center_y(20),
        )
        .on_press(Msg::ModalCancel)
        .padding(0)
        .style(|_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: None,
                text_color: if hovered { c::FG() } else { c::FG_MUTE() },
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius::from(3.0),
                },
                shadow: Shadow::default(),
                snap: false,
            }
        });
        let header = row![
            text("new session").size(13).color(c::MAGENTA()),
            Space::new().width(Length::Fill),
            close_btn,
        ]
        .align_y(iced::Alignment::Center);

        let body = column![header, cols, Space::new().height(8), footer,].spacing(12);

        modal_panel(body.into(), 760.0, c::MAGENTA())
    }

    fn settings_modal(&self) -> Element<'_, Msg> {
        use iced::Alignment::Center;

        // A muted, indented one-liner used under section headers and rows to
        // explain what a control does.
        let caption = |s: &'static str| -> Element<'_, Msg> {
            container(text(s).size(11).color(c::FG_MUTE()))
                .padding(Padding::from([0, 10]))
                .into()
        };

        // ── header ─────────────────────────────────────────────────────────
        let header = row![
            text("settings").size(13).color(c::MAGENTA()),
            Space::new().width(Length::Fill),
            icon_btn("close", Msg::ModalCancel),
        ]
        .align_y(Center);

        // ── appearance ───────────────────────────────────────────────────────
        let theme_row = modal_list_row(
            row![
                text("app theme").size(12).color(c::FG()),
                Space::new().width(Length::Fill),
                text(crate::theme::current().name.to_string())
                    .size(12)
                    .color(c::FG_DIM()),
                Space::new().width(8),
                icon("chev-right", 12.0, c::FG_MUTE()),
            ]
            .align_y(Center),
            false,
            Msg::OpenThemePicker,
        );

        let zoom = container(
            row![
                control_icon_btn("minus", Msg::ZoomOut, 20.0, 13.0),
                control_btn_sized(
                    format!("{:.0}%", self.ui_zoom * 100.0),
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
                text("app size").size(12).color(c::FG()),
                Space::new().width(Length::Fill),
                zoom,
            ]
            .align_y(Center),
        )
        .height(ROW_H)
        .padding(Padding::from([0, 10]));

        // ── terminal ──────────────────────────────────────────────────────
        let tmux_on = self.app.use_tmux();
        let backend_seg = container(
            row![
                seg_button("native", !tmux_on, SegSide::Left, Msg::BackendNative),
                seg_button("tmux", tmux_on, SegSide::Right, Msg::BackendTmux),
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
                text("backend").size(12).color(c::FG()),
                Space::new().width(Length::Fill),
                backend_seg,
            ]
            .align_y(Center),
        )
        .height(ROW_H)
        .padding(Padding::from([0, 10]));
        let backend_caption = container(
            text(if self.app.tmux_available {
                "applies to new sessions only · tmux: detected"
            } else {
                "applies to new sessions only · tmux not found"
            })
            .size(11)
            .color(c::FG_MUTE()),
        )
        .padding(Padding::from([0, 10]));

        let skip_perms_on = self.app.skip_permissions_enabled();
        let skip_perms_seg = container(
            row![
                seg_button(
                    "skip",
                    skip_perms_on,
                    SegSide::Left,
                    Msg::SkipPermissionsEnable
                ),
                seg_button(
                    "safe",
                    !skip_perms_on,
                    SegSide::Right,
                    Msg::SkipPermissionsDisable
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
        let skip_perms_row = container(
            row![
                text("permissions").size(12).color(c::FG()),
                Space::new().width(Length::Fill),
                skip_perms_seg,
            ]
            .align_y(Center),
        )
        .height(ROW_H)
        .padding(Padding::from([0, 10]));
        let skip_perms_caption = container(
            text("applies to new claude/codex sessions only. skip means agents run any command without asking.")
                .size(11)
                .color(c::FG_MUTE()),
        )
        .padding(Padding::from([0, 10]));

        // ── tools ─────────────────────────────────────────────────────────
        let tools_header = container(
            row![
                text("TOOLS").font(UI_BOLD).size(11).color(c::FG_MUTE()),
                Space::new().width(Length::Fill),
                icon_btn("restart", Msg::RefreshTools),
            ]
            .align_y(Center),
        )
        .padding(Padding::from([0, 10]));

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
            // tools keep full-strength labels; version numbers read as data.
            let (status, status_color) = if st.detecting {
                ("detecting…".to_string(), c::FG_MUTE())
            } else if !st.installed {
                ("not installed".to_string(), c::FG_MUTE())
            } else {
                (
                    st.version
                        .clone()
                        .unwrap_or_else(|| "installed".to_string()),
                    c::FG_DIM(),
                )
            };
            let label_color = if st.installed { c::FG() } else { c::FG_DIM() };
            let is_default = self.app.store.default_agent == Some(st.agent);
            let selector: Element<'_, Msg> = if is_default {
                // The chosen default reads as a selected control (filled
                // highlight), not a category tag — magenta stays reserved for
                // the modal's identity accent.
                container(text("default").size(12).color(c::FG()))
                    .padding(Padding::from([4, 12]))
                    .style(|_| container::Style {
                        background: Some(Background::Color(c::BG_HL())),
                        border: Border {
                            radius: Radius::from(4.0),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .into()
            } else if st.installed {
                modal_action(
                    "set default",
                    ModalBtn::Plain,
                    Msg::SetDefaultAgent(st.agent),
                )
            } else {
                Space::new().width(0).into()
            };
            // A fixed-width, right-aligned action cell keeps the badge and the
            // buttons in one column, so the version strings to their left also
            // align — even on missing rows, where the cell stays reserved.
            let action_cell = container(selector)
                .width(Length::Fixed(108.0))
                .align_x(iced::alignment::Horizontal::Right);
            let row = container(
                row![
                    status_dot,
                    Space::new().width(8),
                    icon(st.agent.icon_name(), 14.0, label_color),
                    Space::new().width(8),
                    text(st.agent.label()).size(12).color(label_color),
                    Space::new().width(Length::Fill),
                    text(status).size(12).color(status_color),
                    Space::new().width(16),
                    action_cell,
                ]
                .align_y(Center),
            )
            .height(ROW_H)
            .padding(Padding::from([0, 10]));
            tools = tools.push(row);
        }
        let tools_caption = container(
            text("the default launches for new worktrees.")
                .size(11)
                .color(c::FG_MUTE()),
        )
        .padding(Padding::from([0, 10]));

        // Each section groups its eyebrow, description, and rows tightly; the
        // outer column spaces the groups apart so the hierarchy reads as
        // section → controls rather than one undifferentiated list.
        let eyebrow = |label: &'static str| -> Element<'_, Msg> {
            container(text(label).font(UI_BOLD).size(11).color(c::FG_MUTE()))
                .padding(Padding::from([0, 10]))
                .into()
        };

        let head = column![header, caption("changes save automatically."),].spacing(3);

        let appearance = column![
            eyebrow("APPEARANCE"),
            caption("theme colors and how large the interface renders."),
            Space::new().height(2),
            theme_row,
            app_size_row,
        ]
        .spacing(4);

        let terminal = column![
            eyebrow("TERMINAL"),
            backend_row,
            backend_caption,
            skip_perms_row,
            skip_perms_caption,
        ]
        .spacing(4);

        let tools_section = column![
            tools_header,
            caption("coding agents grove can launch. versions read from each cli."),
            Space::new().height(2),
            tools,
            tools_caption,
        ]
        .spacing(4);

        // ── updates ──────────────────────────────────────────────────────────
        let current_ver = env!("CARGO_PKG_VERSION");
        let status_line: Element<'_, Msg> = match &self.upgrade {
            UpgradeState::Idle => text("not checked yet").size(12).color(c::FG_MUTE()).into(),
            UpgradeState::Checking => row![
                super::icons::spinner(12.0, c::FG_MUTE(), self.blink_tick),
                Space::new().width(8),
                text("checking…").size(12).color(c::FG_MUTE()),
            ]
            .align_y(Center)
            .into(),
            UpgradeState::UpToDate => text("up to date").size(12).color(c::FG_DIM()).into(),
            UpgradeState::Error(e) => text(format!("check failed: {e}"))
                .size(12)
                .color(c::FG_MUTE())
                .into(),
            UpgradeState::Available(r) => text(format!("update available: {}", r.tag))
                .size(12)
                .color(c::GREEN())
                .into(),
            // Updating/Updated/UpdateFailed are shown in the progress modal.
            _ => text("updating…").size(12).color(c::FG_DIM()).into(),
        };

        let updates_header = container(
            row![
                text("UPDATES").font(UI_BOLD).size(11).color(c::FG_MUTE()),
                Space::new().width(Length::Fill),
                if matches!(self.upgrade, UpgradeState::Checking) {
                    container(super::icons::spinner(13.0, c::FG_MUTE(), self.blink_tick)).into()
                } else {
                    icon_btn("restart", Msg::CheckForUpdates { manual: true })
                },
            ]
            .align_y(Center),
        )
        .padding(Padding::from([0, 10]));

        let current_row = container(
            row![
                text("current version").size(12).color(c::FG()),
                Space::new().width(Length::Fill),
                text(format!("v{current_ver}")).size(12).color(c::FG_DIM()),
            ]
            .align_y(Center),
        )
        .height(ROW_H)
        .padding(Padding::from([0, 10]));

        let status_row = container(
            row![
                text("status").size(12).color(c::FG()),
                Space::new().width(Length::Fill),
                status_line,
            ]
            .align_y(Center),
        )
        .height(ROW_H)
        .padding(Padding::from([0, 10]));

        let mut updates_col = column![updates_header, current_row, status_row].spacing(4);

        if let UpgradeState::Available(r) = &self.upgrade {
            let mut actions = row![].spacing(8).align_y(Center);
            // Hide "update now" for Unknown installs (notify-only).
            if !matches!(self.upgrade_method, crate::upgrade::InstallMethod::Unknown) {
                actions = actions.push(modal_action(
                    "update now",
                    ModalBtn::Primary,
                    Msg::StartUpdate,
                ));
            }
            actions = actions.push(modal_action(
                "skip this version",
                ModalBtn::Plain,
                Msg::SkipVersion,
            ));
            // No opener crate exists in this codebase; offer the URL as a
            // clipboard action instead of dead text.
            actions = actions.push(modal_action(
                "copy url",
                ModalBtn::Plain,
                Msg::CopyReleaseUrl,
            ));

            let action_row = container(actions).padding(Padding::from([4, 10]));
            updates_col = updates_col.push(action_row);

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
                let notes_row = container(text(truncated).size(11).color(c::FG_MUTE()))
                    .padding(Padding::from([2, 10]));
                updates_col = updates_col.push(notes_row);
            }
        }

        let changelog_row = container(modal_action(
            "view changelog",
            ModalBtn::Plain,
            Msg::OpenChangelog,
        ))
        .padding(Padding::from([4, 10]));
        updates_col = updates_col.push(changelog_row);

        let updates_section = updates_col;

        let body = column![head, appearance, terminal, tools_section, updates_section].spacing(16);

        modal_panel(body.into(), 580.0, c::MAGENTA())
    }

    /// Two-column keyboard-shortcut reference (mod+/). Text-only key labels:
    /// the bundled fonts have no modifier-symbol glyphs.
    fn shortcut_overlay_modal(&self) -> Element<'_, Msg> {
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
            .filter(|d| super::update::scope_allows(d.scopes, screen))
            .collect();

        // Does the visible set span more than one scope? (Global vs current-screen)
        let has_global = visible.iter().any(|d| d.scopes.contains(&Scope::Global));
        let has_screen = visible
            .iter()
            .any(|d| d.scopes.contains(&Scope::Screen(screen)));
        let grouped = has_global && has_screen;

        // Static display-only rows the behavioral registry deliberately omits.
        let static_rows: [(String, &'static str); 2] = [
            (format!("{m}+c / {m}+v"), "copy / paste in session"),
            ("esc".into(), "close modals"),
        ];

        let make_row = |keys: String, desc: &'static str| {
            row![
                container(text(keys).size(11).color(c::CYAN())).width(Length::Fixed(170.0)),
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

        let mut body = column![text("keyboard shortcuts").size(13).color(c::MAGENTA())].spacing(12);

        if grouped {
            // Global section: registry Global rows + the static copy/paste/esc rows.
            let mut global_rows: Vec<(String, &'static str)> = visible
                .iter()
                .filter(|d| d.scopes.contains(&Scope::Global))
                .map(|d| (key_label(*d), d.description))
                .collect();
            for (keys, desc) in static_rows.iter() {
                global_rows.push((keys.clone(), desc));
            }
            // Screen section: registry rows scoped to the current screen.
            let screen_rows: Vec<(String, &'static str)> = visible
                .iter()
                .filter(|d| d.scopes.contains(&Scope::Screen(screen)))
                .map(|d| (key_label(*d), d.description))
                .collect();

            if !global_rows.is_empty() {
                body = body.push(text("global").size(11).color(c::FG_MUTE()));
                body = body.push(two_columns(global_rows));
            }
            if !screen_rows.is_empty() {
                body = body.push(text(screen.label()).size(11).color(c::FG_MUTE()));
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
                .map(|d| (key_label(*d), d.description))
                .collect();
            for (keys, desc) in static_rows.iter() {
                rows.push((keys.clone(), desc));
            }
            body = body.push(two_columns(rows));
        }

        body = body
            .push(Space::new().height(4))
            .push(text("esc to close").size(11).color(c::FG_MUTE()));

        modal_panel(body.into(), 640.0, c::MAGENTA())
    }

    fn updating_modal(&self) -> Element<'_, Msg> {
        use iced::Alignment::Center;

        let header = text("updating grove").size(13).color(c::MAGENTA());

        let body: Element<'_, Msg> = match &self.upgrade {
            UpgradeState::Updating(stage) => {
                let label = match stage {
                    crate::upgrade::Stage::Downloading => "downloading…",
                    crate::upgrade::Stage::Building => "building…",
                    crate::upgrade::Stage::Installing => "installing…",
                    crate::upgrade::Stage::Done => "finishing…",
                };
                row![
                    super::icons::spinner(16.0, c::FG_DIM(), self.blink_tick),
                    Space::new().width(10),
                    text(label).size(12).color(c::FG()),
                ]
                .align_y(Center)
                .into()
            }
            UpgradeState::Updated => column![
                text("update installed. restart grove to apply")
                    .size(12)
                    .color(c::FG()),
                Space::new().height(10),
                row![
                    modal_action("restart", ModalBtn::Primary, Msg::RestartApp),
                    Space::new().width(8),
                    modal_action("later", ModalBtn::Plain, Msg::ModalCancel),
                ]
                .align_y(Center),
            ]
            .into(),
            UpgradeState::UpdateFailed(e) => column![
                text("update failed").size(12).color(c::FG()),
                Space::new().height(6),
                text(e.clone()).size(11).color(c::FG_MUTE()),
                Space::new().height(10),
                modal_action("close", ModalBtn::Plain, Msg::ModalCancel),
            ]
            .into(),
            _ => text("updating…").size(12).color(c::FG_DIM()).into(),
        };

        let content = column![header, Space::new().height(12), body].spacing(0);
        modal_panel(content.into(), 420.0, c::MAGENTA())
    }

    fn theme_picker_modal(
        &self,
        sel_dark: usize,
        sel_light: usize,
        tab: crate::theme::ThemeKind,
        follow_system: bool,
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

        let system_row = modal_checkbox(
            "follow system appearance".into(),
            follow_system,
            c::MAGENTA(),
            Some(Msg::ThemePickerToggleSystem),
        );

        let body = column![
            text("theme").size(13).color(c::MAGENTA()),
            system_row,
            tabs,
            scroller,
            Space::new().height(8),
            row![
                Space::new().width(Length::Fill),
                modal_action("cancel", ModalBtn::Plain, Msg::ThemePickerCancel),
                modal_action("apply", ModalBtn::Primary, Msg::ThemePickerSubmit),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        modal_panel(body.into(), 460.0, c::MAGENTA())
    }

    /// The first-run onboarding wizard. A single modal that walks the user
    /// through the flow's steps (five, or six when tmux is detected) in
    /// grove's own quiet chrome — no SaaS-wizard flourishes, just the same
    /// modal vocabulary every other surface uses.
    #[allow(clippy::too_many_arguments)]
    fn onboarding_modal<'a>(
        &'a self,
        step: OnboardStep,
        path: &'a str,
        dir_sel: usize,
        name: Option<&'a str>,
        note: Option<&'a str>,
        tab: crate::theme::ThemeKind,
        sel_dark: usize,
        sel_light: usize,
        agent_sel: usize,
        backend_tmux: bool,
        perms_skip: bool,
    ) -> Element<'a, Msg> {
        use iced::Alignment::Center;

        // ── progress rail ───────────────────────────────────────────────────
        let tmux = self.app.tmux_available;
        let mut rail = Row::new().spacing(10).align_y(Center);
        for &s in OnboardStep::flow(tmux) {
            let (dotc, txtc) = if s == step {
                (c::MAGENTA(), c::FG())
            } else if s.index_in(tmux) < step.index_in(tmux) {
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
                text("welcome to grove").size(19).color(c::FG()),
                text("a worktree launchpad for ai coding agents")
                    .size(13)
                    .color(c::FG_DIM()),
                Space::new().height(6),
                onboard_point(
                    "sessions are the unit of work",
                    "every agent you spawn lives in a managed session that survives navigation; switch between them in two keystrokes.",
                ),
                onboard_point(
                    "worktrees, not branches",
                    "grove treats git worktrees as a first-class primitive: create, list, and run agents inside them.",
                ),
                onboard_point(
                    "quiet and keyboard-first",
                    "the app stays out of the way so terminal output stays primary. this takes about a minute.",
                ),
            ]
            .spacing(10)
            .into(),

            OnboardStep::Environment => {
                let mut list = Column::new().spacing(6);
                let rows = [
                    (on_path("git"), false, "git", "version control"),
                    (
                        crate::agent::Agent::Claude.available(),
                        false,
                        "claude",
                        "claude code",
                    ),
                    (
                        crate::agent::Agent::Codex.available(),
                        false,
                        "codex",
                        "codex cli",
                    ),
                    (
                        crate::agent::Agent::OpenCode.available(),
                        false,
                        "opencode",
                        "opencode cli",
                    ),
                    (
                        self.app.tmux_available,
                        true,
                        "tmux",
                        "persists sessions across restarts",
                    ),
                ];
                for (found, optional, n, meta) in rows {
                    list = list.push(onboard_env_row(found, optional, n, meta));
                }
                column![
                    text("environment").size(16).color(c::FG()),
                    text("grove spawns agents from your PATH; it doesn't install or authenticate them. only git is required to get going.")
                        .size(12)
                        .color(c::FG_DIM())
                        .wrapping(iced::widget::text::Wrapping::Word),
                    Space::new().height(4),
                    list,
                ]
                .spacing(10)
                .into()
            }

            OnboardStep::Backend => {
                let seg = container(
                    row![
                        seg_button(
                            "native",
                            !backend_tmux,
                            SegSide::Left,
                            Msg::OnbBackendSelect(false)
                        ),
                        seg_button(
                            "tmux",
                            backend_tmux,
                            SegSide::Right,
                            Msg::OnbBackendSelect(true)
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
                column![
                    text("session backend").size(16).color(c::FG()),
                    text("use tmux for new sessions? existing sessions keep their current backend.")
                        .size(12)
                        .color(c::FG_DIM())
                        .wrapping(iced::widget::text::Wrapping::Word),
                    text("native sessions end when grove quits; tmux sessions survive restarts")
                        .size(12)
                        .color(c::FG_DIM())
                        .wrapping(iced::widget::text::Wrapping::Word),
                    Space::new().height(4),
                    seg,
                ]
                .spacing(10)
                .into()
            }

            OnboardStep::Project => {
                let path_input = text_input("~/code/my-repo", path)
                    .id(modal_input_id())
                    .font(UI_FONT)
                    .size(13)
                    .padding(Padding::from([8, 12]))
                    .on_input(Msg::OnbPathChanged)
                    .on_submit(Msg::OnbNext)
                    .style(input_field_style);

                let browse = modal_action(
                    if self.picker_open {
                        "waiting…"
                    } else {
                        "browse…"
                    },
                    ModalBtn::Plain,
                    Msg::AddProjectBrowse,
                );

                let mut col = column![
                    text("add your first project").size(16).color(c::FG()),
                    text("point grove at a git repository, or any plain folder for ad-hoc sessions.")
                        .size(12)
                        .color(c::FG_DIM())
                        .wrapping(iced::widget::text::Wrapping::Word),
                    text("repository or folder").size(11).color(c::FG_MUTE()),
                    row![path_input, browse]
                        .spacing(8)
                        .align_y(iced::Alignment::Center),
                ]
                .spacing(8);

                // Matches appear only once the user starts typing; an empty
                // field would list the cwd's directories as noise.
                if !path.trim().is_empty() {
                    col = col
                        .push(text("matches").size(11).color(c::FG_MUTE()))
                        .push(self.dir_matches(path, dir_sel, 5, Msg::OnbPickDir));
                }

                if let Some(name) = name {
                    let name_input = text_input("project name", name)
                        .id(modal_name_id())
                        .font(UI_FONT)
                        .size(13)
                        .padding(Padding::from([8, 12]))
                        .on_input(Msg::OnbNameChanged)
                        .on_submit(Msg::OnbNext)
                        .style(input_field_style);
                    col = col
                        .push(text("name").size(11).color(c::FG_MUTE()))
                        .push(name_input);
                }

                if let Some(note) = note {
                    col = col.push(text(note.to_string()).size(12).color(c::RED()));
                }
                col = col.push(
                    text("tab complete · ↑↓ select · enter continue · or skip setup")
                        .size(11)
                        .color(c::FG_MUTE()),
                );
                col.into()
            }

            OnboardStep::Theme => {
                let themes = crate::theme::themes_of(tab);
                let sel = match tab {
                    crate::theme::ThemeKind::Dark => sel_dark,
                    crate::theme::ThemeKind::Light => sel_light,
                };
                let tabs = container(
                    row![
                        seg_button(
                            "dark",
                            matches!(tab, crate::theme::ThemeKind::Dark),
                            SegSide::Left,
                            Msg::OnbThemeTab,
                        ),
                        seg_button(
                            "light",
                            matches!(tab, crate::theme::ThemeKind::Light),
                            SegSide::Right,
                            Msg::OnbThemeTab,
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
                    list = list.push(modal_list_row(
                        text(th.name.to_string())
                            .size(12)
                            .color(if active { c::FG() } else { c::FG_DIM() }),
                        active,
                        Msg::OnbThemeSelect(i),
                    ));
                }
                let list_h = (themes.len().min(7) as f32) * ROW_H;
                let scroller = container(scrollable(list))
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

                column![
                    text("pick a theme").size(16).color(c::FG()),
                    text("37 colorways, painted by semantic role so every screen reads correctly. change it any time in settings.")
                        .size(12)
                        .color(c::FG_DIM())
                        .wrapping(iced::widget::text::Wrapping::Word),
                    tabs,
                    scroller,
                ]
                .spacing(10)
                .into()
            }

            OnboardStep::Session => {
                let mut col = column![
                    text("start your first session").size(16).color(c::FG()),
                ]
                .spacing(8);

                match self.app.store.projects.last() {
                    Some(p) => {
                        col = col.push(
                            text(format!("launch an agent inside {}.", p.name))
                                .size(12)
                                .color(c::FG_DIM())
                                .wrapping(iced::widget::text::Wrapping::Word),
                        );
                        let mut list = Column::new().spacing(0);
                        for (i, agent) in self.app.available_agents.iter().enumerate() {
                            let active = i == agent_sel;
                            list = list.push(modal_list_row(
                                text(agent.label().to_string())
                                    .size(12)
                                    .color(if active { c::FG() } else { c::FG_DIM() }),
                                active,
                                Msg::OnbAgentSelect(i),
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
                            text("no project added. you can add one any time from the sidebar. finish to start using grove.")
                                .size(12)
                                .color(c::FG_DIM())
                                .wrapping(iced::widget::text::Wrapping::Word),
                        );
                    }
                }
                let perms_seg = container(
                    row![
                        seg_button("safe", !perms_skip, SegSide::Left, Msg::OnbPermsSelect(false)),
                        seg_button("skip", perms_skip, SegSide::Right, Msg::OnbPermsSelect(true)),
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
                col = col
                    .push(Space::new().height(4))
                    .push(
                        row![
                            text("permissions").size(11).color(c::FG_MUTE()),
                            Space::new().width(Length::Fill),
                            perms_seg,
                        ]
                        .align_y(iced::Alignment::Center),
                    )
                    .push(
                        text(if perms_skip {
                            "skip: agents run any command without asking"
                        } else {
                            "safe: agents ask before running commands"
                        })
                        .size(11)
                        .color(if perms_skip { c::YELLOW() } else { c::FG_MUTE() }),
                    );
                col.into()
            }
        };

        // ── footer ────────────────────────────────────────────────────────────
        let next_label = match step {
            OnboardStep::Welcome => "get started",
            OnboardStep::Session => "launch session",
            _ => "continue",
        };
        let count = format!(
            "{} / {}",
            step.index_in(tmux) + 1,
            OnboardStep::flow(tmux).len()
        );
        let mut footer = row![
            text(count).size(11).color(c::FG_MUTE()),
            Space::new().width(Length::Fill),
            modal_action("skip setup", ModalBtn::Plain, Msg::OnbSkip),
        ]
        .spacing(8)
        .align_y(Center);
        if step.prev(tmux).is_some() {
            footer = footer.push(modal_action("back", ModalBtn::Plain, Msg::OnbBack));
        }
        footer = footer.push(modal_action(next_label, ModalBtn::Primary, Msg::OnbNext));

        let content = column![
            rail,
            container(body)
                .width(Length::Fill)
                .height(Length::Fixed(300.0)),
            footer,
        ]
        .spacing(14);

        modal_panel(content.into(), 600.0, c::MAGENTA())
    }

    // ── changelog modal ───────────────────────────────────────────────────

    fn changelog_modal(&self) -> Element<'_, Msg> {
        use super::state::ChangelogState;
        use iced::Alignment::Center;

        let header = row![
            text("changelog").size(13).color(c::MAGENTA()),
            Space::new().width(Length::Fill),
            icon_btn("close", Msg::CloseChangelog),
        ]
        .align_y(Center);

        let inner: Element<'_, Msg> = match &self.changelog {
            ChangelogState::Idle | ChangelogState::Loading => row![
                super::icons::spinner(16.0, c::FG_DIM(), self.blink_tick),
                Space::new().width(10),
                text("loading\u{2026}").size(12).color(c::FG_MUTE()),
            ]
            .align_y(Center)
            .into(),
            ChangelogState::Error(e) => text(format!("couldn't load changelog: {e}"))
                .size(12)
                .color(c::FG_MUTE())
                .into(),
            ChangelogState::Loaded(notes) if notes.is_empty() => {
                text("no releases yet.").size(12).color(c::FG_MUTE()).into()
            }
            ChangelogState::Loaded(notes) => {
                let mut list = Column::new().spacing(18);
                for n in notes {
                    let mut head = row![text(n.tag.clone()).size(13).font(UI_BOLD).color(c::FG()),]
                        .spacing(8)
                        .align_y(Center);
                    if !n.name.is_empty() && n.name != n.tag {
                        head = head.push(text(n.name.clone()).size(13).color(c::FG_DIM()));
                    }
                    if !n.date.is_empty() {
                        head = head.push(Space::new().width(Length::Fill));
                        head = head.push(text(n.date.clone()).size(11).color(c::FG_MUTE()));
                    }
                    let body_text = crate::upgrade::clean_markdown(&n.body);
                    let entry = column![
                        head,
                        Space::new().height(4),
                        text(body_text).size(12).color(c::FG_MUTE()),
                    ]
                    .spacing(0);
                    list = list.push(entry);
                }
                // Right padding leaves a gap between the text and the
                // scrollbar so they don't crowd each other.
                scrollable(container(list).padding(Padding {
                    top: 0.0,
                    right: 12.0,
                    bottom: 0.0,
                    left: 0.0,
                }))
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
            }
        };

        let body = column![
            header,
            Space::new().height(12),
            container(inner)
                .width(Length::Fill)
                .height(Length::Fixed(420.0)),
        ]
        .spacing(0);

        let panel = modal_panel(body.into(), 600.0, c::MAGENTA());

        // Centered overlay on a dim backdrop, matching the settings modal.
        container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::SCRIM())),
                ..Default::default()
            })
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
            top: 5.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }),
        column![
            text(lead).size(12).color(c::FG()),
            text(body)
                .size(11)
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
        (c::GREEN(), "found", c::GREEN())
    } else if optional {
        (c::AMBER(), "optional", c::AMBER())
    } else {
        (c::FG_MUTE(), "missing", c::FG_MUTE())
    };
    container(
        row![
            dot(dotc),
            text(name.to_string()).size(12).color(c::FG()),
            text(meta.to_string()).size(11).color(c::FG_MUTE()),
            Space::new().width(Length::Fill),
            text(tag).size(10).color(tagc),
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

/// Cheap PATH scan for a bare binary name — used to report `git`/`tmux`
/// presence on the onboarding environment step without shelling out.
fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let p = dir.join(bin);
            std::fs::metadata(&p).map(|m| m.is_file()).unwrap_or(false)
        })
    })
}
