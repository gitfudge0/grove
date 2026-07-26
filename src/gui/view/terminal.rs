//! The right-hand workspace: grid view, single-session view, the
//! worktree terminal slide-over panel, the home-terminal tab, and the
//! shared PTY canvas renderer.

use super::common::{is_in_progress_title, session_context_title};
use crate::gui::icons::icon;
use crate::gui::metrics::{
    CELL_H, CELL_W, MONO_FONT, SESSBAR_H, SIDEBAR_DIVIDER_W, UI_BOLD, UI_FONT,
};
use crate::gui::palette as c;
use crate::gui::pty::{rebuild_row_runs, PtyProgram};
use crate::gui::rows::single_line;
use crate::gui::state::GridDragMsg;
use crate::gui::state::{FocusedPane, Grove, Msg, PtyCacheEntry, PtyCell, PtyPane};
use crate::gui::update::platform_mod_label;
use crate::gui::widgets::{
    divider_h, divider_v, dot, empty_terminals_workspace, empty_workspace, icon_btn, tool_btn,
    tool_btn_toggle, vline,
};
use grove_core::git::Worktree;
use grove_core::session::{Session, SessionStatus};
use iced::border::Radius;
use iced::widget::{
    button, canvas as canvas_widget, column, container, row, scrollable, stack, text, Space,
};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

thread_local! {
    /// Per-frame memo of `pty_theme_for`, keyed by project name. Resolving a
    /// project's pinned theme scans the theme registry by name behind an
    /// `RwLock`; grid view repeats that for every tile, several of which
    /// usually share a project. Cleared at the top of every `view()` by
    /// [`reset_pty_theme_cache`], so it can never serve a stale theme across
    /// frames.
    static PTY_THEME_CACHE: RefCell<HashMap<String, grove_core::theme::Theme>> =
        RefCell::new(HashMap::new());
}

/// Drop the per-frame PTY-theme memo. Called once at the top of `view()`.
pub(super) fn reset_pty_theme_cache() {
    PTY_THEME_CACHE.with(|c| c.borrow_mut().clear());
}

impl Grove {
    /// The theme a PTY belonging to `project` renders its *content* in: the
    /// project's pinned theme (or the launcher's live preview of one) if any,
    /// otherwise the global active theme. App chrome is unaffected — it always
    /// uses `c::*` against the global theme. Memoized per frame; see
    /// [`PTY_THEME_CACHE`].
    fn pty_theme_for(&self, project: &str) -> grove_core::theme::Theme {
        PTY_THEME_CACHE.with(|cache| {
            if let Some(t) = cache.borrow().get(project) {
                return t.clone();
            }
            let launcher_preview = crate::gui::session_launcher::project_theme_preview(
                &self.launcher,
                &self.app.store.projects,
                project,
            );
            let theme = self
                .app
                .project_theme_override(project, launcher_preview)
                .unwrap_or_else(grove_core::theme::current);
            cache
                .borrow_mut()
                .insert(project.to_string(), theme.clone());
            theme
        })
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
        use crate::gui::metrics::grid_layout;

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
                let mut el: Element<'_, Msg> = if si < self.app.sessions.len() {
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
                // Draw-only slide for a tile that just swapped places: ease
                // its rendered position back from where it came from, in
                // grid cells, to zero. Layout is untouched (see slide.rs).
                if let Some(slide) = &self.anim.grid_slide {
                    if let Some(&(_, d_col, d_row)) =
                        slide.tiles.iter().find(|(idx, _, _)| *idx == tile_idx)
                    {
                        let t = crate::gui::update::slide_progress(
                            slide.start,
                            std::time::Instant::now(),
                        );
                        if t < 1.0 {
                            let (tile_w, tile_h) = crate::gui::metrics::grid_tile_size(
                                self.pty_layout.window_size.width,
                                self.pty_layout.window_size.height,
                                self.pty_layout.zoom,
                                n,
                            );
                            let remaining = 1.0 - t;
                            // ponytail: uses the nominal equal-cell tile size, so a
                            // horizontal swap between columns of unequal tile
                            // heights (ragged grid) is approximate — it settles
                            // exactly at t=1. Upgrade path is a real per-tile rect
                            // calc if it ever reads wrong.
                            let offset = iced::Vector::new(
                                d_col as f32 * (tile_w + 1.0) * remaining,
                                d_row as f32 * (tile_h + 1.0) * remaining,
                            );
                            el = crate::gui::slide::slide(el, offset);
                        }
                    }
                }
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

    pub(super) fn workspace(&self) -> Element<'_, Msg> {
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

        // Collapse the whole slide-over (same effect as the header term
        // toggle), so the panel is always dismissable from itself. Uses a
        // distinct "collapse-right" glyph (rather than the per-tab ×) so the
        // two affordances don't read as the same action, plus a tooltip to
        // disambiguate at a glance.
        let close_panel = Self::hint(
            icon_btn("collapse-right", Msg::ToggleTermPanel),
            "collapse panel",
        );

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

        let close_btn = button(
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
        let close = container(Self::hint(close_btn, "close shell")).padding(Padding {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 2.0,
        });

        // Tabs are identified by a terminal icon (status conveyed by the dot
        // and the active highlight), not a textual name — cleaner when several
        // shells share a worktree. Spacing widened so the dot / icon / × read
        // as distinct controls rather than one blob.
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
            None => empty_terminals_workspace(),
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

    fn sess_bar(&self, si: usize, s: &Session) -> Element<'_, Msg> {
        let running = matches!(
            *s.status.lock().unwrap_or_else(|e| e.into_inner()),
            SessionStatus::Running
        );
        let context = session_context_title(s);
        let show_progress = running
            && context
                .as_deref()
                .map(is_in_progress_title)
                .unwrap_or(false);
        // Visual hierarchy: session/project label is the strongest (13px,
        // weight-600, FG); the branch and context title are secondary
        // (12px, FG_DIM).
        let sess_text_sized = |content: String, size: f32, color: Color, bold: bool| {
            let t = text(content)
                .font(UI_FONT)
                .size(size)
                .line_height(1.0)
                .align_y(iced::alignment::Vertical::Center)
                .color(color);
            if bold {
                t.font(iced::Font {
                    weight: iced::font::Weight::Semibold,
                    ..UI_FONT
                })
            } else {
                t
            }
        };
        // Force single-line rendering (see rows::single_line docs): iced 0.13's
        // text widget ignores wrapping::None, so long labels word-wrap to a
        // second line inside the outer clip(true) container unless each text
        // is itself clipped to exactly one line height.
        let single = |content: String, size: f32, color: Color, bold: bool| -> Element<'_, Msg> {
            single_line(sess_text_sized(content, size, color, bold), size)
        };

        let mut identity = row![
            icon(s.agent.icon_name(), 13.0, c::FG()),
            single(s.label.clone(), 13.0, c::FG(), true),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);
        // Branchless sessions (e.g. host terminals) skip the branch segment
        // entirely — otherwise the header shows two dots with nothing between.
        if !s.branch.trim().is_empty() {
            identity = identity
                .push(single("·".to_string(), 13.0, c::FG_MUTE(), false))
                .push(single(s.branch.clone(), 12.0, c::FG_DIM(), false));
        }

        if let Some(title) = context {
            let title = crate::gui::widgets::truncate_middle(&title, 80);
            let session_context: Element<'_, Msg> = if show_progress {
                let phase = ((self.anim.blink_tick / 5) % 3) as usize;
                let step_dot = |i| dot(if i == phase { c::GREEN() } else { c::FG_MUTE() });
                row![
                    step_dot(0),
                    step_dot(1),
                    step_dot(2),
                    single("in progress".to_string(), 12.0, c::GREEN(), false),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center)
                .into()
            } else {
                single(title, 12.0, c::FG_DIM(), false)
            };
            identity = identity
                .push(single("·".to_string(), 12.0, c::FG_MUTE(), false))
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
            container(identity).width(Length::Fill).clip(true),
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

    pub(super) fn pty(&self, pane: PtyPane, s: &Session) -> Element<'_, Msg> {
        // Per-session row snapshot + canvas cache. Switching to a quiet
        // session returns the cached geometry with zero draw work; switching
        // to a session that produced output re-snaps the rows and clears the
        // canvas cache, then draws once.
        // The `dirty` Arc's address is the cache key. A dropped session can
        // free that address and a newly spawned one reuse it — safe only
        // because every session add/remove (incl. home-terminal new/close/
        // restart) fully clears this cache, so no stale entry can alias.
        // Resolve once per tile per frame: a pinned "Project theme" makes
        // this PTY's *content* (fill, default fg, cursor, ANSI 0-15) render
        // in that theme instead of the global one. App chrome (header,
        // borders, rail, appbar) is untouched — it always uses `c::*`
        // against the global active theme.
        let pty_theme = self.pty_theme_for(&s.project);

        // Cursor blinks at ~500 ms on / 500 ms off (tick interval = 60 ms,
        // so 8–9 ticks per half-period; use mod 16 with threshold 8).
        let cursor_visible = self.anim.blink_tick % 16 < 8;

        let key = Arc::as_ptr(&s.dirty) as usize;
        let (rows, cache, cursor_cache, cursor_pos) = {
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
            // These Arcs are for cheap intra-frame refcounting inside a
            // `RefCell`-guarded, single-threaded view cache — never shared
            // across threads — so `arc_with_non_send_sync` is a false
            // positive here.
            #[allow(clippy::arc_with_non_send_sync)]
            let entry = entry.or_insert_with(|| PtyCacheEntry {
                rows: Arc::new(Vec::new()),
                cache: Arc::new(iced::widget::canvas::Cache::default()),
                cursor_pos: None,
                cursor_cache: Arc::new(iced::widget::canvas::Cache::default()),
                cursor_key: (None, false),
            });
            if needs_rebuild {
                let parser = s.parser.lock().unwrap_or_else(|e| e.into_inner());
                let screen = parser.screen();
                let (h, w) = screen.size();
                let mut new_rows = Vec::with_capacity(h as usize);
                for r in 0..h {
                    new_rows.push(rebuild_row_runs(screen, r, w, &pty_theme));
                }
                entry.rows = Arc::new(new_rows);
                entry.cache.clear();
                entry.cursor_pos = if screen.hide_cursor() {
                    None
                } else {
                    Some(screen.cursor_position())
                };
            }
            // The cursor block lives in its own cache so a blink — which flips
            // twice a second — doesn't rebuild the whole screen's geometry,
            // and so a steady cursor costs nothing at all.
            let cursor_key = (entry.cursor_pos, cursor_visible);
            if entry.cursor_key != cursor_key {
                entry.cursor_key = cursor_key;
                entry.cursor_cache.clear();
            }
            (
                Arc::clone(&entry.rows),
                Arc::clone(&entry.cache),
                Arc::clone(&entry.cursor_cache),
                entry.cursor_pos,
            )
        };

        let rows_len = rows.len() as f32;
        let cols = rows
            .first()
            .map(|r| r.iter().map(|run| run.text.chars().count()).sum::<usize>())
            .unwrap_or(0) as f32;
        // Translate the scrollback-stable selection into the current viewport.
        // Each endpoint clamps to the visible window; a selection entirely off
        // one edge isn't painted. The selection lives in the pane that owns
        // it (Agent/Panel, or the focused tile in grid view), so only paint it
        // there — otherwise a selection in one pane would mis-render against
        // another's grid.
        let selection = if pane == self.selection_pane() {
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
            let to_vr = |c: &crate::gui::state::AbsCell| (h - 1) - (c.a_row as isize - sb);
            let (ra, rb) = (to_vr(&a), to_vr(&b));
            if (ra < 0 && rb < 0) || (ra > h - 1 && rb > h - 1) {
                return None;
            }
            let cell = |c: &crate::gui::state::AbsCell, r: isize| PtyCell {
                row: r.clamp(0, h - 1) as usize,
                col: c.col,
            };
            Some((cell(&a, ra), cell(&b, rb)))
        });
        let program = PtyProgram {
            pane,
            rows,
            cache,
            cursor_cache,
            selection,
            cursor: cursor_pos,
            cursor_visible,
            default_fg: c::fg_of(&pty_theme),
            cursor_color: c::fg_of(&pty_theme),
        };
        let body: Element<'_, Msg> = canvas_widget(program)
            .width(Length::Fixed((cols * CELL_W).max(CELL_W)))
            .height(Length::Fixed((rows_len * CELL_H).max(CELL_H)))
            .into();

        // While the split is live, tint the focused PTY's top edge so it's clear
        // which terminal will receive keystrokes. Suppressed when the panel is
        // closed (only one PTY is interactive then).
        let focused = self.term_panel_open && pane == self.focused_input_pane();
        container(
            scrollable(body)
                .width(Length::Fill)
                .height(Length::Fill)
                .direction(scrollable::Direction::Vertical(
                    scrollable::Scrollbar::new().width(0).scroller_width(0),
                )),
        )
        .padding(Padding::from([12, 16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Color(c::bg_of(&pty_theme))),
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
        use crate::gui::metrics::TILE_HEAD_H;

        let focused = self.grid_focused == Some(si);
        let is_drag_src = self
            .drag
            .grid_drag
            .as_ref()
            .is_some_and(|d| d.source_idx == tile_order_idx);
        let is_drop_zone = self
            .drag
            .grid_drag
            .as_ref()
            .is_some_and(|d| d.hover_idx == tile_order_idx && d.source_idx != tile_order_idx);

        // ── tile header ────────────────────────────────────────────────
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
        // Waiting-for-input: drives both the header's "respond" chip below and
        // (later) the tile border. Attention wins over the focused-cyan border.
        use crate::gui::activity::ActivityState;
        let waiting = matches!(self.activity_state(s), ActivityState::WaitingForInput);

        // "respond" chip: only shown while this tile is waiting for input.
        // Pulses via `attention_pulse` so it stays visible without demanding
        // constant attention. Placed left of `num_hint` in the header.
        let respond_chip: Element<'_, Msg> = if waiting {
            let a = 1.0 - 0.35 * self.attention_pulse();
            let amber = Color { a, ..c::AMBER() };
            let amber_bg = Color {
                a: a * 0.08,
                ..c::AMBER()
            };
            let inner: Element<'_, Msg> = if tile_order_idx >= 9 {
                text("respond").font(MONO_FONT).size(9).color(amber).into()
            } else {
                let n = tile_order_idx + 1;
                let chord: Element<'_, Msg> = if cfg!(target_os = "macos") {
                    row![
                        icon("command", 9.0, amber),
                        text(n.to_string()).font(MONO_FONT).size(9).color(amber),
                    ]
                    .spacing(1)
                    .align_y(iced::Alignment::Center)
                    .into()
                } else {
                    text(format!("{}+{}", platform_mod_label(), n))
                        .font(MONO_FONT)
                        .size(9)
                        .color(amber)
                        .into()
                };
                row![
                    text("respond · ").font(MONO_FONT).size(9).color(amber),
                    chord,
                ]
                .spacing(1)
                .align_y(iced::Alignment::Center)
                .into()
            };
            container(inner)
                .padding(Padding::from([1, 4]))
                .style(move |_| container::Style {
                    background: Some(Background::Color(amber_bg)),
                    border: Border {
                        color: amber,
                        width: 1.0,
                        radius: 3.0.into(),
                    },
                    ..Default::default()
                })
                .into()
        } else {
            Space::new().width(0).into()
        };

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
        // Branchless sessions (e.g. host terminals) skip the branch segment
        // entirely — otherwise the header shows a trailing dot with nothing after.
        let branch_seg: Element<'_, Msg> = if s.branch.trim().is_empty() {
            Space::new().width(0).into()
        } else {
            row![
                text("·").size(10).color(c::FG_MUTE()),
                text(s.branch.clone()).size(10).color(c::FG_MUTE()),
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center)
            .into()
        };
        let header_row = row![
            icon(s.agent.icon_name(), 11.0, c::FG_DIM()),
            text(s.agent.label())
                .font(UI_BOLD)
                .size(10)
                .color(c::FG_DIM()),
            text("·").size(10).color(c::FG_MUTE()),
            text(s.project.clone()).size(10).color(c::FG_MUTE()),
            branch_seg,
            Space::new().width(Length::Fill),
            respond_chip,
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
        .on_press(Msg::GridDrag(GridDragMsg::Start(tile_order_idx)));

        // ── tile body (header + PTY) ───────────────────────────────────
        let tile_body: Element<'_, Msg> = column![
            header,
            divider_h(c::BORDER_SOFT()),
            // Reuse the existing pty() renderer with PtyPane::Tile(si).
            // Selection paints here when this tile is `grid_focused` — see
            // `selection_pane`.
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
        let (border_color, border_width) = if waiting {
            (c::AMBER(), 1.5f32)
        } else if focused {
            (c::CYAN(), 1.5f32)
        } else {
            (Color::TRANSPARENT, 0.0)
        };

        // Full-tile scrim overlay when waiting for input. Layered on top of
        // the tile-header "respond" chip above and the appbar "needs you"
        // pill elsewhere — this doesn't replace either, it's the third and
        // most attention-grabbing signal for a tile that needs a response.
        let with_scrim: Element<'_, Msg> = if waiting {
            // Opacity pulse (~2.4s): 40-tick triangle wave, alpha 0.7..1.0.
            let phase = (self.anim.blink_tick % 40) as f32;
            let t = (phase - 20.0).abs() / 20.0;
            let text_alpha = 0.7 + 0.3 * t;
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
                    text(sub_line).font(MONO_FONT).size(10).color(c::FG_MUTE()),
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

            // Wrap in mouse_area so clicking the scrim focuses/acknowledges
            // the tile, same as clicking the header elsewhere on the tile.
            let clickable_scrim: Element<'_, Msg> = iced::widget::mouse_area(scrim_content)
                .on_press(Msg::GridDrag(GridDragMsg::Start(tile_order_idx)))
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
        .on_enter(Msg::GridDrag(GridDragMsg::Hover(tile_order_idx)))
        .into()
    }

    /// The pane that currently owns keyboard/scroll/selection input. Mirrors the
    /// routing logic in `focused_session*`: the panel only wins while it is open
    /// *and* `focused_pane` selects it; otherwise input belongs to the agent.
    pub(in crate::gui) fn focused_input_pane(&self) -> PtyPane {
        if self.term_panel_open && matches!(self.focused_pane, FocusedPane::Panel) {
            PtyPane::Panel
        } else {
            PtyPane::Agent
        }
    }

    /// The pane that currently owns `pty_selection` — like
    /// `focused_input_pane`, but grid-view-aware: while grid view is showing,
    /// the focused tile owns any selection instead of the (unrendered) Agent
    /// pane.
    pub(in crate::gui) fn selection_pane(&self) -> PtyPane {
        if self.grid_view {
            self.grid_focused
                .map(PtyPane::Tile)
                .unwrap_or(PtyPane::Agent)
        } else {
            self.focused_input_pane()
        }
    }
}
