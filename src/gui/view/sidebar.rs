//! The project/worktree/session tree sidebar: header, collapse/expand,
//! per-row rendering, docked terminals-collapsed header, and the visible
//! session-order helpers `mod+1..9` and the agent-menu overlay rely on.

use crate::gui::icons::icon;
use crate::gui::metrics::{ROW_H, SESSBAR_H, SIDEBAR_DIVIDER_W};
use crate::gui::palette as c;
use crate::gui::rows::{
    project_row, session_row, worktree_row, ProjectRowProps, SessionRowProps, WorktreeRowProps,
};
use crate::gui::state::{Grove, Msg};
use crate::gui::widgets::{
    divider_h, divider_v, ghost_scrollable, section_header, sidebar_agent_menu_overlay,
};
use grove_core::git::Worktree;
use grove_core::session::{Session, SessionStatus};
use iced::border::Radius;
use iced::widget::{button, column, container, row, stack, Column, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long a memoized `git::is_repo` answer stays good. `tree_view` runs on
/// every redraw (~16×/s), and `is_repo` stats the filesystem, so calling it
/// straight from `view()` meant a syscall per project per frame. A project
/// gaining/losing its `.git` is rare and not something the app drives, so a
/// short TTL is enough to keep the glyph honest without the per-frame cost.
const IS_REPO_TTL: Duration = Duration::from_secs(5);

thread_local! {
    /// Path → (answer, when it was taken). Single-threaded view-only cache,
    /// same idiom as the other `RefCell` render caches.
    static IS_REPO_CACHE: RefCell<HashMap<String, (bool, Instant)>> =
        RefCell::new(HashMap::new());
}

/// `git::is_repo` memoized for `IS_REPO_TTL` (see above).
fn is_repo_cached(path: &str) -> bool {
    IS_REPO_CACHE.with(|cache| {
        let mut map = cache.borrow_mut();
        let now = Instant::now();
        if let Some((answer, at)) = map.get(path) {
            if now.duration_since(*at) < IS_REPO_TTL {
                return *answer;
            }
        }
        let answer = grove_core::git::is_repo(path);
        map.insert(path.to_string(), (answer, now));
        answer
    })
}

impl Grove {
    /// Whether any home terminal has a live process running — the signal
    /// behind the "TERMINALS" header's collapsed-state activity dot. Shared
    /// by the docked (collapsed) and inline (expanded, forced `false` — see
    /// call site) header renders so the scan isn't duplicated.
    fn home_terminals_activity(&self) -> bool {
        self.app.home_terminals.iter().any(|s| {
            matches!(
                *s.status
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                SessionStatus::Running
            )
        })
    }

    /// The draggable divider between the sidebar and the workspace. A 1px line
    /// centered in a `SIDEBAR_DIVIDER_W`-wide hit zone, with a resize cursor on
    /// hover. The press starts a drag; cursor moves and the release are tracked
    /// by a global subscription (see `Grove::subscription`).
    pub(super) fn sidebar_resize_handle(&self) -> Element<'_, Msg> {
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
    pub(super) fn sidebar(&self) -> Element<'_, Msg> {
        let tree_head = self.tree_head();
        let content: Element<'_, Msg> = self.tree_view();
        let tree_area = container(ghost_scrollable(content).height(Length::Fill))
            .height(Length::Fill)
            .padding(Padding {
                top: 8.0,
                bottom: 12.0,
                left: 0.0,
                right: 0.0,
            });
        let agent_menu_top = self.open_agent_menu_top();
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

        // When the TERMINALS section is collapsed, its header+rows no longer
        // render inside the scrollable tree — dock a standalone copy of the
        // header at the very bottom instead so it's always reachable.
        let docked_terminals: Option<Element<'_, Msg>> = if self.terminals_collapsed {
            Some(crate::gui::rows::home_terminals_header(
                false,
                self.app.home_terminals.len(),
                self.home_terminals_activity(),
            ))
        } else {
            None
        };

        let mut stack_col =
            column![tree_head, divider_h(c::BORDER_SOFT()), tree_layer,].height(Length::Fill);
        if let Some(docked) = docked_terminals {
            stack_col = stack_col.push(divider_h(c::BORDER_SOFT()));
            stack_col = stack_col.push(docked);
        }

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

        let add_btn = button(
            container(icon("plus", 12.0, c::FG_MUTE()))
                .center_x(22)
                .center_y(22),
        )
        .on_press(Msg::AddProject(crate::gui::add_project::Msg::Open))
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

        // Tree is always active now, so the collapse-all toggle is always shown.
        let right_tools: Element<'_, Msg> =
            container(row![add_btn, toggle].align_y(iced::Alignment::Center))
                .height(Length::Fill)
                .align_y(iced::Alignment::Center)
                .into();

        let section_label = section_header("PROJECTS", 0.0, 0.0);

        container(
            row![section_label, Space::new().width(Length::Fill), right_tools,]
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
        // One pass over the session list up front. Previously every project
        // row rescanned all sessions for its count and roll-up, and every
        // worktree row rescanned them twice more — O(projects × worktrees ×
        // sessions) per frame.
        let mut by_proj: HashMap<&str, Vec<&Session>> = HashMap::new();
        let mut by_wt: HashMap<&str, Vec<(usize, &Session)>> = HashMap::new();
        for (si, s) in self.app.sessions.iter().enumerate() {
            by_proj.entry(s.project.as_str()).or_default().push(s);
            by_wt.entry(s.wt_path.as_str()).or_default().push((si, s));
        }
        // The git-status map is read once per worktree row; locking it per row
        // meant one mutex acquisition per worktree per frame.
        let git_states = self.git_state.lock().ok();
        let pulse = self.attention_pulse();
        for (pi, p) in self.app.store.projects.iter().enumerate() {
            let (pname, ppath) = (p.name.as_str(), p.path.as_str());
            let expanded = !self.collapsed.contains(&pi);
            let is_git = is_repo_cached(ppath);
            let proj_sessions = by_proj.get(pname).map_or(&[][..], std::vec::Vec::as_slice);
            let count = proj_sessions.len();
            // Collapsed projects surface the most urgent descendant state as
            // a trailing glyph; expanded parents show nothing extra.
            let proj_rollup = if !expanded {
                crate::gui::activity::most_urgent(
                    proj_sessions.iter().map(|s| self.activity_state(s)),
                )
            } else {
                None
            };
            col = col.push(project_row(ProjectRowProps {
                idx: pi,
                name: pname,
                count,
                expanded,
                is_git,
                rollup: proj_rollup,
                tick: self.anim.blink_tick,
                pulse,
            }));

            if !expanded {
                continue;
            }
            let wts: &[Worktree] = if pi == self.app.proj_idx {
                &self.app.worktrees
            } else {
                self.wt_cache
                    .get(&pi)
                    .map_or(&[][..], std::vec::Vec::as_slice)
            };
            let has_run = p
                .scripts
                .run
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty());
            for (wi, w) in wts.iter().enumerate() {
                let wname: Cow<'_, str> = if w.is_main {
                    Cow::Borrowed(pname)
                } else {
                    Cow::Owned(crate::app::path_basename(&w.path))
                };
                let wt_sessions = by_wt
                    .get(w.path.as_str())
                    .map_or(&[][..], std::vec::Vec::as_slice);
                let active_wt = pi == self.app.proj_idx && wi == self.app.wt_idx;
                let hovered = self.hovered_wt == Some((pi, wi));
                let wt_expanded = !self.collapsed_wt.contains(&(pi, wi));
                // Same roll-up rule as projects: only when collapsed.
                let wt_rollup = if !wt_expanded {
                    crate::gui::activity::most_urgent(
                        wt_sessions.iter().map(|(_, s)| self.activity_state(s)),
                    )
                } else {
                    None
                };
                let git_suffix = git_states
                    .as_ref()
                    .and_then(|g| g.get(&w.path).and_then(grove_core::git::git_state_suffix));
                let wt_el = worktree_row(WorktreeRowProps {
                    proj: pi,
                    wt: wi,
                    name: &wname,
                    branch: &w.branch,
                    active: active_wt,
                    is_main: w.is_main,
                    is_git,
                    hovered,
                    expanded: wt_expanded,
                    has_run,
                    rollup: wt_rollup,
                    tick: self.anim.blink_tick,
                    pulse,
                    available: &self.app.available_agents,
                    git_suffix,
                });
                col = col.push(
                    iced::widget::mouse_area(wt_el)
                        .on_enter(Msg::HoverWorktree(Some((pi, wi))))
                        .on_exit(Msg::HoverWorktree(None)),
                );

                if !wt_expanded {
                    continue;
                }
                for &(si, s) in wt_sessions {
                    // The tree and the pinned terminals now render
                    // simultaneously, so a session must not show the
                    // "active" highlight while the workspace is actually
                    // showing a home terminal.
                    let active = !self.terminal_focused && self.app.active_session == Some(si);
                    let pending_kill = self.pending_kill == Some(si);
                    col = col.push(session_row(SessionRowProps {
                        idx: si,
                        session: s,
                        wt_name: &wname,
                        active,
                        pending_kill,
                        state: self.activity_state(s),
                        tick: self.anim.blink_tick,
                        pulse,
                    }));
                }
            }
        }

        if !self.terminals_collapsed {
            // Expanded: every terminal already renders its own row below, so
            // the header's activity dot (a "something's running in here" cue
            // for the *collapsed* state) would be redundant — always off.
            col = col.push(divider_h(c::BORDER_SOFT()));
            col = col.push(crate::gui::rows::home_terminals_header(
                true,
                self.app.home_terminals.len(),
                false,
            ));
            for (i, s) in self.app.home_terminals.iter().enumerate() {
                let active = self.terminal_focused && self.app.active_terminal == Some(i);
                let pending_kill = self.pending_kill_terminal == Some(i);
                col = col.push(crate::gui::rows::terminal_row(i, s, active, pending_kill));
            }
        }

        col.into()
    }

    /// Session indices in the order `tree_view` renders them, honoring
    /// collapse state. Kept as a separate method (identical to
    /// `tree_session_order`) because `mod+1..9` calls it by this name.
    pub fn visible_session_order(&self) -> Vec<usize> {
        self.tree_session_order()
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
                self.wt_cache
                    .get(&pi)
                    .map_or(&[][..], std::vec::Vec::as_slice)
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
                self.wt_cache
                    .get(&pi)
                    .map_or(&[][..], std::vec::Vec::as_slice)
            };

            for (wi, w) in wts.iter().enumerate() {
                let wname = if w.is_main {
                    pname.to_string()
                } else {
                    crate::app::path_basename(&w.path)
                };
                let show_branch =
                    crate::gui::rows::worktree_shows_branch(w.is_main, &w.branch, &wname);
                let wt_h = crate::gui::rows::worktree_row_height(show_branch);
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
}
