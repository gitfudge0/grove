//! iced port of the grove TUI. The visual contract is `mockups/gui.html`.

use crate::agent::Agent;
use crate::app::App;
use crate::git::Worktree;
use crate::session::{Session, SessionStatus};
use anyhow::Result;
use iced::border::Radius;
use iced::keyboard::{self, key::Named, Key, Modifiers};
use iced::widget::{
    button, column, container, row, scrollable, svg, text, Column, Space,
};
use iced::{
    event, Background, Border, Color, Element, Event, Font, Length, Padding, Shadow, Size,
    Subscription, Task, Theme,
};
use std::cell::RefCell;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

// ──────────────────────────────────────────────────────────────────────────
// palette — converted from the mockup's oklch tokens
// ──────────────────────────────────────────────────────────────────────────
#[allow(dead_code)]
mod c {
    use iced::Color;
    pub const BG: Color = Color::from_rgb(0.110, 0.115, 0.150);
    pub const BG_RAIL: Color = Color::from_rgb(0.090, 0.095, 0.130);
    pub const BG_STRIP: Color = Color::from_rgb(0.075, 0.080, 0.115);
    pub const BG_HOVER: Color = Color::from_rgb(0.140, 0.150, 0.190);
    pub const BG_HL: Color = Color::from_rgb(0.180, 0.190, 0.260);
    pub const BORDER: Color = Color::from_rgb(0.190, 0.195, 0.230);
    pub const BORDER_SOFT: Color = Color::from_rgb(0.145, 0.150, 0.185);
    pub const FG: Color = Color::from_rgb(0.785, 0.805, 0.870);
    pub const FG_DIM: Color = Color::from_rgb(0.650, 0.670, 0.745);
    pub const FG_MUTE: Color = Color::from_rgb(0.460, 0.480, 0.560);
    pub const BLUE: Color = Color::from_rgb(0.478, 0.635, 0.969);
    pub const CYAN: Color = Color::from_rgb(0.490, 0.812, 1.000);
    pub const MAGENTA: Color = Color::from_rgb(0.733, 0.604, 0.969);
    pub const GREEN: Color = Color::from_rgb(0.620, 0.808, 0.416);
    pub const YELLOW: Color = Color::from_rgb(0.878, 0.686, 0.408);
    pub const RED: Color = Color::from_rgb(0.969, 0.463, 0.557);
}

const MONO_FONT: Font = Font::MONOSPACE;

const ROW_H: f32 = 28.0;
const RAIL_W: f32 = 320.0;
const APPBAR_H: f32 = 44.0;
const STATUS_H: f32 = 26.0;
const SESSBAR_H: f32 = 36.0;

/// Default PTY size on spawn — re-sized as the workspace area shrinks/grows.
const PTY_ROWS: u16 = 50;
const PTY_COLS: u16 = 160;

// ──────────────────────────────────────────────────────────────────────────
// entry
// ──────────────────────────────────────────────────────────────────────────

pub fn run() -> Result<()> {
    iced::application("grove", Grove::update, Grove::view)
        .theme(|_| Theme::Dark)
        .subscription(Grove::subscription)
        .window_size(Size::new(1280.0, 800.0))
        .run_with(|| (Grove::new(), Task::none()))
        .map_err(|e| anyhow::anyhow!(e))
}

pub struct Grove {
    app: App,
    collapsed: std::collections::HashSet<usize>,
    /// Cache of worktrees per project index. Refilled on project expand /
    /// session spawn/kill — never inside `view()`, since `git worktree list`
    /// is a subprocess and `view()` runs on every 33ms tick.
    wt_cache: std::collections::HashMap<usize, Vec<Worktree>>,
    /// Cached PTY screen snapshots, keyed by the `dirty` Arc's pointer
    /// (stable & unique per Session). Rebuilt only when the session's dirty
    /// flag was set since last build — so switching to a quiet session is
    /// free, and the per-frame parser lock is taken only for sessions that
    /// actually changed.
    pty_cache: RefCell<std::collections::HashMap<usize, PtyCacheEntry>>,
}

struct PtyCacheEntry {
    /// One row per terminal line. Each row is a run-list of styled segments.
    rows: Vec<Vec<StyledRun>>,
}

#[derive(Clone)]
struct StyledRun {
    text: String,
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Tick,
    BackendNative,
    BackendTmux,
    ProjectClicked(usize),
    WorktreeClicked { proj: usize, wt: usize },
    StartSession { proj: usize, wt: usize, agent: Agent },
    StartTerminal { proj: usize, wt: usize },
    SelectSession(usize),
    KillSession(usize),
    KeyPress(Key, Modifiers),
    AddProject,
    NoOp,
}

impl Grove {
    fn new() -> Self {
        let app = App::new().expect("init app");
        let mut g = Self {
            app,
            collapsed: Default::default(),
            wt_cache: Default::default(),
            pty_cache: Default::default(),
        };
        // Prime the per-project worktree cache so `view()` never has to shell
        // out to `git worktree list` (it runs on every 33ms tick).
        let n = g.app.store.projects.len();
        for i in 0..n {
            g.ensure_wt_cached(i);
        }
        g
    }

    fn ensure_wt_cached(&mut self, proj: usize) {
        if proj == self.app.proj_idx || self.wt_cache.contains_key(&proj) {
            return;
        }
        if let Some(p) = self.app.store.projects.get(proj) {
            let wts = crate::git::list_worktrees(&p.path);
            self.wt_cache.insert(proj, wts);
        }
    }

    fn subscription(&self) -> Subscription<Msg> {
        let tick = iced::time::every(Duration::from_millis(60)).map(|_| Msg::Tick);
        // Only forward un-captured keys; widgets (search input) handle their own first.
        let keys = event::listen_with(|ev, status, _| {
            if status == event::Status::Captured {
                return None;
            }
            match ev {
                Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    Some(Msg::KeyPress(key, modifiers))
                }
                _ => None,
            }
        });
        Subscription::batch([tick, keys])
    }

    fn update(&mut self, msg: Msg) -> Task<Msg> {
        match msg {
            Msg::Tick => {
                // No-op: `dirty` flags are consumed lazily by `pty()` when
                // it rebuilds a session's cached snapshot. Clearing them
                // here would force a full rebuild every tick.
            }
            Msg::BackendNative => {
                let _ = self.app.set_tmux_enabled(false);
            }
            Msg::BackendTmux => {
                let _ = self.app.set_tmux_enabled(true);
            }
            Msg::ProjectClicked(i) => {
                if self.collapsed.contains(&i) {
                    self.collapsed.remove(&i);
                } else {
                    self.collapsed.insert(i);
                }
                if self.app.proj_idx != i {
                    self.app.proj_idx = i;
                    self.app.refresh_worktrees();
                    self.wt_cache.remove(&i);
                }
                self.ensure_wt_cached(i);
            }
            Msg::WorktreeClicked { proj, wt } => {
                if self.app.proj_idx != proj {
                    self.app.proj_idx = proj;
                    self.app.refresh_worktrees();
                }
                self.app.wt_idx = wt;
            }
            Msg::StartSession { proj, wt, agent } => self.spawn(proj, wt, agent),
            Msg::StartTerminal { proj, wt } => self.spawn(proj, wt, Agent::Terminal),
            Msg::SelectSession(i) => {
                if i < self.app.sessions.len() {
                    self.app.active_session = Some(i);
                }
            }
            Msg::KillSession(i) => {
                if i < self.app.sessions.len() {
                    let key = Arc::as_ptr(&self.app.sessions[i].dirty) as usize;
                    self.pty_cache.borrow_mut().remove(&key);
                    self.app.sessions.remove(i);
                    if let Some(a) = self.app.active_session {
                        if a == i {
                            self.app.active_session = None;
                        } else if a > i {
                            self.app.active_session = Some(a - 1);
                        }
                    }
                }
            }
            Msg::KeyPress(key, mods) => {
                if let Some(bytes) = key_to_bytes(&key, mods) {
                    if let Some(i) = self.app.active_session {
                        if let Some(s) = self.app.sessions.get_mut(i) {
                            s.send(&bytes);
                        }
                    }
                }
            }
            Msg::AddProject | Msg::NoOp => {}
        }
        Task::none()
    }

    fn spawn(&mut self, proj: usize, wt: usize, agent: Agent) {
        if self.app.proj_idx != proj {
            self.app.proj_idx = proj;
            self.app.refresh_worktrees();
        }
        self.app.wt_idx = wt;
        let pname = match self.app.store.projects.get(proj) {
            Some(p) => p.name.clone(),
            None => return,
        };
        let Some(w) = self.app.worktrees.get(wt).cloned() else { return };
        let label = if w.is_main {
            pname.clone()
        } else {
            crate::app::path_basename(&w.path)
        };
        let args = agent.launch_args();
        let use_tmux = self.app.use_tmux();
        match Session::spawn(label, pname, w.path.clone(), agent, &args, &w.path, use_tmux) {
            Ok(mut s) => {
                s.resize(PTY_ROWS, PTY_COLS);
                self.app.sessions.push(s);
                self.app.active_session = Some(self.app.sessions.len() - 1);
            }
            Err(e) => {
                self.app.status = format!("failed to start session: {e}");
            }
        }
    }

    fn view(&self) -> Element<'_, Msg> {
        let body = column![
            self.appbar(),
            row![self.sidebar(), self.workspace()]
                .height(Length::Fill)
                .width(Length::Fill),
            self.statusbar(),
        ]
        .width(Length::Fill)
        .height(Length::Fill);

        container(body)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG)),
                text_color: Some(c::FG),
                ..Default::default()
            })
            .into()
    }

    // ── appbar ──────────────────────────────────────────────────────────
    fn appbar(&self) -> Element<'_, Msg> {
        let brand = row![
            text("grove").font(MONO_FONT).size(14.0).color(c::MAGENTA),
            text("worktree launchpad for ai agents")
                .size(11.5)
                .color(c::FG_MUTE),
        ]
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
                color: c::BORDER,
                width: 1.0,
                radius: Radius::from(5.0),
            },
            ..Default::default()
        });

        let right = row![
            seg,
            icon_btn("cog", Msg::NoOp),
            icon_btn("help", Msg::NoOp),
        ]
        .spacing(4)
        .padding(Padding::from([0, 12]))
        .align_y(iced::Alignment::Center);

        let inner = row![
            container(brand).width(RAIL_W),
            Space::with_width(Length::Fill),
            right,
        ]
        .align_y(iced::Alignment::Center)
        .height(Length::Fill);

        container(inner)
            .height(APPBAR_H)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP)),
                ..Default::default()
            })
            .into()
    }

    // ── sidebar ─────────────────────────────────────────────────────────
    fn sidebar(&self) -> Element<'_, Msg> {
        let head = container(
            row![
                text("projects").size(11).color(c::FG_MUTE),
                Space::with_width(Length::Fill),
                icon_btn("plus", Msg::AddProject),
            ]
            .align_y(iced::Alignment::Center)
            .padding(Padding {
                top: 0.0,
                bottom: 0.0,
                left: 16.0,
                right: 12.0,
            }),
        )
        .height(36.0);

        let tree = self.tree_view();

        let add_proj = container(
            button(
                container(text("+ add project").size(12).color(c::FG_DIM))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            )
            .on_press(Msg::AddProject)
            .width(Length::Fill)
            .height(28.0)
            .style(|_, _| button::Style {
                background: None,
                text_color: c::FG_DIM,
                border: Border {
                    color: c::BORDER,
                    width: 1.0,
                    radius: Radius::from(5.0),
                },
                shadow: Shadow::default(),
            }),
        )
        .padding(Padding {
            top: 10.0,
            bottom: 10.0,
            left: 12.0,
            right: 12.0,
        });

        let stack = column![
            head,
            divider_h(c::BORDER_SOFT),
            container(scrollable(tree).height(Length::Fill))
                .height(Length::Fill)
                .padding(Padding {
                    top: 6.0,
                    bottom: 14.0,
                    left: 0.0,
                    right: 0.0,
                }),
            divider_h(c::BORDER_SOFT),
            add_proj,
        ]
        .height(Length::Fill);

        container(stack)
            .width(RAIL_W)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_RAIL)),
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
            .map(|(i, p)| (i, p.name.clone(), p.path.clone()))
            .collect();
        for (pi, pname, ppath) in projects {
            let expanded = !self.collapsed.contains(&pi);
            let count = self
                .app
                .sessions
                .iter()
                .filter(|s| s.project == pname)
                .count();
            col = col.push(project_row(pi, &pname, count, expanded));

            if expanded {
                let wts: &[Worktree] = if pi == self.app.proj_idx {
                    &self.app.worktrees
                } else {
                    self.wt_cache.get(&pi).map(|v| v.as_slice()).unwrap_or(&[])
                };
                let _ = ppath;
                for (wi, w) in wts.iter().enumerate() {
                    let wname = if w.is_main {
                        pname.clone()
                    } else {
                        crate::app::path_basename(&w.path)
                    };
                    let active_wt = pi == self.app.proj_idx && wi == self.app.wt_idx;
                    col = col.push(worktree_row(pi, wi, &wname, &w.branch, active_wt));

                    for (si, s) in self.app.sessions.iter().enumerate() {
                        if s.wt_path == w.path {
                            let active = self.app.active_session == Some(si);
                            col = col.push(session_row(si, s, active));
                        }
                    }
                }
            }
        }
        col.into()
    }

    // ── workspace ───────────────────────────────────────────────────────
    fn workspace(&self) -> Element<'_, Msg> {
        let inner: Element<'_, Msg> = match self.app.active_session {
            Some(i) if i < self.app.sessions.len() => column![
                self.sess_bar(&self.app.sessions[i]),
                self.pty(&self.app.sessions[i]),
            ]
            .height(Length::Fill)
            .into(),
            _ => empty_workspace().into(),
        };

        container(inner)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG)),
                ..Default::default()
            })
            .into()
    }

    fn sess_bar(&self, s: &Session) -> Element<'_, Msg> {
        let running = matches!(*s.status.lock().unwrap(), SessionStatus::Running);
        let (dot_color, label) = if running {
            (c::GREEN, "running")
        } else {
            (c::FG_MUTE, "exited")
        };

        let bar = row![
            dot(dot_color),
            text(label).size(11).color(dot_color),
            vline(),
            text(s.agent.label()).font(MONO_FONT).size(12).color(c::MAGENTA),
            text("·").color(c::BORDER),
            text(s.project.clone()).font(MONO_FONT).size(12).color(c::BLUE),
            text("/").color(c::BORDER),
            text(s.label.clone()).font(MONO_FONT).size(12).color(c::FG),
            text(format!("[{}]", s.branch)).font(MONO_FONT).size(12).color(c::FG_MUTE),
            Space::with_width(Length::Fill),
            text(s.wt_path.clone()).font(MONO_FONT).size(12).color(c::FG_MUTE),
            vline(),
            tool_btn("split", "split", false, Msg::NoOp),
            tool_btn("edit", "rename", false, Msg::NoOp),
            tool_btn(
                "trash",
                "kill",
                true,
                Msg::KillSession(self.app.active_session.unwrap_or(0)),
            ),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center)
        .height(Length::Fill)
        .padding(Padding::from([0, 18]));

        let bar_container = container(bar)
            .height(SESSBAR_H)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP)),
                ..Default::default()
            });

        column![bar_container, divider_h(c::BORDER_SOFT)].into()
    }

    fn pty(&self, s: &Session) -> Element<'_, Msg> {
        // Only re-snapshot when this session's PTY produced new output since
        // last build. Switching between two quiet sessions hits the cache
        // and never takes the parser lock — that's what makes the switch
        // feel instantaneous.
        let key = Arc::as_ptr(&s.dirty) as usize;
        let mut cache = self.pty_cache.borrow_mut();
        let entry = cache.entry(key);
        let needs_rebuild = match &entry {
            std::collections::hash_map::Entry::Occupied(_) => {
                s.dirty.swap(false, Ordering::Relaxed)
            }
            std::collections::hash_map::Entry::Vacant(_) => {
                s.dirty.store(false, Ordering::Relaxed);
                true
            }
        };
        let entry = entry.or_insert_with(|| PtyCacheEntry { rows: Vec::new() });
        if needs_rebuild {
            // Snapshot cells under the parser lock, then release it before
            // we touch any widget code. The PTY reader thread also wants
            // this lock to process incoming bytes — holding it across
            // widget construction is what froze the UI under heavy output.
            let parser = s.parser.lock().unwrap();
            let screen = parser.screen();
            let (h, w) = screen.size();
            entry.rows.clear();
            entry.rows.reserve(h as usize);
            for r in 0..h {
                entry.rows.push(rebuild_row_runs(screen, r, w));
            }
        }
        let body: Element<'_, Msg> = pty_widget_from_runs(&entry.rows);
        drop(cache);

        container(scrollable(body).width(Length::Fill).height(Length::Fill))
            .padding(Padding::from([14, 18]))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG)),
                ..Default::default()
            })
            .into()
    }

    // ── status bar ──────────────────────────────────────────────────────
    fn statusbar(&self) -> Element<'_, Msg> {
        let running = self.app.sessions.len();
        let backend = if self.app.use_tmux() { "tmux" } else { "native" };
        let theme_name = self.app.store.theme.clone().unwrap_or_else(|| "tokyonight".into());

        let left = row![
            row![
                dot(c::GREEN),
                text(format!("{running} running")).size(11).color(c::FG_DIM),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
            row![
                text("backend").size(11).color(c::FG_MUTE),
                text(backend).size(11).color(c::FG_DIM),
            ]
            .spacing(6),
            row![
                text("theme").size(11).color(c::FG_MUTE),
                text(theme_name).size(11).color(c::FG_DIM),
            ]
            .spacing(6),
        ]
        .spacing(14)
        .align_y(iced::Alignment::Center);

        let toast: Element<'_, Msg> = match &self.app.toast {
            Some(t) => text(t.message.clone()).size(11).color(c::GREEN).into(),
            None => Space::with_width(0).into(),
        };

        let right = row![text(format!("v{}", env!("CARGO_PKG_VERSION")))
            .size(11)
            .color(c::FG_DIM),];

        let bar = row![
            left,
            Space::with_width(24),
            toast,
            Space::with_width(Length::Fill),
            right,
        ]
        .padding(Padding::from([0, 14]))
        .align_y(iced::Alignment::Center)
        .height(Length::Fill);

        container(bar)
            .height(STATUS_H)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP)),
                ..Default::default()
            })
            .into()
    }
}

// ──────────────────────────────────────────────────────────────────────────
// row builders
// ──────────────────────────────────────────────────────────────────────────

fn project_row<'a>(idx: usize, name: &str, count: usize, expanded: bool) -> Element<'a, Msg> {
    let twist = if expanded { "chev-down" } else { "chev-right" };
    let row_content = row![
        container(icon(twist, 10.0, c::FG_MUTE)).width(14).center_y(Length::Fill),
        text(name.to_string()).size(13).color(c::FG),
        Space::with_width(Length::Fill),
        row![
            dot(if count > 0 { c::GREEN } else { c::FG_MUTE }),
            text(format!("{count}"))
                .font(MONO_FONT)
                .size(11)
                .color(if count > 0 { c::GREEN } else { c::FG_MUTE }),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 12.0,
        right: 8.0,
    });

    clickable_row(row_content, ROW_H, false, Msg::ProjectClicked(idx))
}

fn worktree_row<'a>(
    proj: usize,
    wt: usize,
    name: &str,
    branch: &str,
    active: bool,
) -> Element<'a, Msg> {
    let row_content = row![
        container(icon("chev-right", 10.0, c::FG_MUTE)).width(14).center_y(Length::Fill),
        text(name.to_string()).size(13).color(c::FG_DIM),
        text(branch.to_string())
            .font(MONO_FONT)
            .size(10.5)
            .color(c::FG_MUTE),
        Space::with_width(Length::Fill),
        action_pill_icon("play", "start", Msg::StartSession { proj, wt, agent: Agent::Claude }),
        action_mini("term", Msg::StartTerminal { proj, wt }),
        action_mini("more", Msg::NoOp),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center)
    .padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 28.0,
        right: 8.0,
    });

    clickable_row(row_content, ROW_H, active, Msg::WorktreeClicked { proj, wt })
}

fn session_row<'a>(idx: usize, s: &Session, active: bool) -> Element<'a, Msg> {
    let running = matches!(*s.status.lock().unwrap(), SessionStatus::Running);
    let dot_color = if running { c::GREEN } else { c::FG_MUTE };
    let agent_color = if active { c::CYAN } else { c::FG };

    let row_content = row![
        Space::with_width(28),
        dot(dot_color),
        text(s.agent.label())
            .font(MONO_FONT)
            .size(12)
            .color(agent_color),
        text(s.label.clone()).size(11).color(c::FG_MUTE),
        Space::with_width(Length::Fill),
        action_mini("close", Msg::KillSession(idx)),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 16.0,
        right: 8.0,
    });

    clickable_row(row_content, ROW_H, active, Msg::SelectSession(idx))
}

fn clickable_row<'a>(
    content: impl Into<Element<'a, Msg>>,
    height: f32,
    active: bool,
    on_press: Msg,
) -> Element<'a, Msg> {
    let bg = if active { Some(Background::Color(c::BG_HL)) } else { None };
    let text_color = if active { c::FG } else { c::FG_DIM };
    button(
        container(content.into())
            .height(height)
            .width(Length::Fill)
            .align_y(iced::Alignment::Center),
    )
    .on_press(on_press)
    .width(Length::Fill)
    .padding(0)
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered && !active {
                Some(Background::Color(c::BG_HOVER))
            } else {
                bg
            },
            text_color,
            border: Border::default(),
            shadow: Shadow::default(),
        }
    })
    .into()
}

// ──────────────────────────────────────────────────────────────────────────
// small widgets
// ──────────────────────────────────────────────────────────────────────────

fn empty_workspace<'a>() -> Element<'a, Msg> {
    container(
        column![
            text("no session selected").size(14).color(c::FG_DIM),
            text("click a worktree's start button to spawn an agent")
                .size(12)
                .color(c::FG_MUTE),
        ]
        .spacing(8)
        .align_x(iced::Alignment::Center),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .width(Length::Fill)
    .height(Length::Fill)
    .style(|_| container::Style {
        background: Some(Background::Color(c::BG)),
        ..Default::default()
    })
    .into()
}

fn dot<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::with_width(7))
        .width(7)
        .height(7)
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            border: Border {
                color,
                width: 0.0,
                radius: Radius::from(3.5),
            },
            ..Default::default()
        })
        .into()
}

fn vline<'a>() -> Element<'a, Msg> {
    container(Space::with_width(1))
        .width(1)
        .height(18)
        .style(|_| container::Style {
            background: Some(Background::Color(c::BORDER)),
            ..Default::default()
        })
        .into()
}

fn divider_h<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::with_height(1))
        .width(Length::Fill)
        .height(1)
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

fn seg_button<'a>(label: &str, active: bool, msg: Msg) -> Element<'a, Msg> {
    button(text(label.to_string()).size(12))
        .on_press(msg)
        .padding(Padding::from([4, 10]))
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: if active {
                    Some(Background::Color(c::BG_HL))
                } else if hovered {
                    Some(Background::Color(c::BG_HOVER))
                } else {
                    None
                },
                text_color: if active { c::FG } else { c::FG_DIM },
                border: Border::default(),
                shadow: Shadow::default(),
            }
        })
        .into()
}

fn icon_btn<'a>(name: &'static str, msg: Msg) -> Element<'a, Msg> {
    button(
        container(icon(name, 15.0, c::FG_DIM))
            .center_x(28)
            .center_y(28),
    )
    .on_press(msg)
    .padding(0)
    .style(|_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered { Some(Background::Color(c::BG_HOVER)) } else { None },
            text_color: c::FG_DIM,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(5.0),
            },
            shadow: Shadow::default(),
        }
    })
    .into()
}

fn action_pill_icon<'a>(icon_name: &'static str, label: &str, msg: Msg) -> Element<'a, Msg> {
    let label_owned = label.to_string();
    button(
        container(
            row![
                icon(icon_name, 9.0, c::GREEN),
                text(label_owned).size(11).color(c::FG_DIM),
            ]
            .spacing(5)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([0, 8]))
        .center_y(22),
    )
    .on_press(msg)
    .padding(0)
    .style(|_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: Some(Background::Color(if hovered { c::BG_HOVER } else { c::BG })),
            text_color: if hovered { c::FG } else { c::FG_DIM },
            border: Border {
                color: c::BORDER,
                width: 1.0,
                radius: Radius::from(4.0),
            },
            shadow: Shadow::default(),
        }
    })
    .into()
}

fn action_mini<'a>(icon_name: &'static str, msg: Msg) -> Element<'a, Msg> {
    button(
        container(icon(icon_name, 12.0, c::FG_MUTE))
            .center_x(22)
            .center_y(22),
    )
    .on_press(msg)
    .padding(0)
    .style(|_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered { Some(Background::Color(c::BG_HOVER)) } else { None },
            text_color: if hovered { c::FG } else { c::FG_MUTE },
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(4.0),
            },
            shadow: Shadow::default(),
        }
    })
    .into()
}

fn tool_btn<'a>(icon_name: &'static str, label: &str, danger: bool, msg: Msg) -> Element<'a, Msg> {
    let label_owned = label.to_string();
    button(
        container(
            row![
                icon(icon_name, 12.0, c::FG_DIM),
                text(label_owned).size(11.5).color(c::FG_DIM),
            ]
            .spacing(5)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([0, 8]))
        .center_y(22),
    )
    .on_press(msg)
    .padding(0)
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        let color = if hovered {
            if danger { c::RED } else { c::FG }
        } else {
            c::FG_DIM
        };
        button::Style {
            background: if hovered { Some(Background::Color(c::BG_HOVER)) } else { None },
            text_color: color,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: Radius::from(4.0),
            },
            shadow: Shadow::default(),
        }
    })
    .into()
}

// ──────────────────────────────────────────────────────────────────────────
// icons: inline svg sprite so we don't depend on system glyph fonts
// ──────────────────────────────────────────────────────────────────────────

fn icon<'a>(name: &str, size: f32, color: Color) -> Element<'a, Msg> {
    let s = svg_for(name);
    svg(svg::Handle::from_memory(s.into_bytes()))
        .width(size)
        .height(size)
        .style(move |_, _| svg::Style { color: Some(color) })
        .into()
}

fn svg_for(name: &str) -> String {
    let inner = match name {
        "plus" => r#"<path d="M8 3v10M3 8h10"/>"#,
        "close" => r#"<path d="M4 4l8 8M12 4l-8 8"/>"#,
        "play" => r#"<path d="M5 3.5l7 4.5-7 4.5z" fill="currentColor"/>"#,
        "chev-down" => r#"<path d="M4 6l4 4 4-4"/>"#,
        "chev-right" => r#"<path d="M6 4l4 4-4 4"/>"#,
        "cog" => r#"<circle cx="8" cy="8" r="2"/><path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.5 3.5l1.4 1.4M11.1 11.1l1.4 1.4M3.5 12.5l1.4-1.4M11.1 4.9l1.4-1.4"/>"#,
        "help" => r#"<circle cx="8" cy="8" r="6.2"/><path d="M6 6.2c0-1.1 0.9-2 2-2s2 0.9 2 2c0 1.1-2 1.4-2 2.6M8 11.6v0.2"/>"#,
        "search" => r#"<circle cx="7" cy="7" r="4.5"/><path d="M10.4 10.4l3 3"/>"#,
        "term" => r#"<rect x="1.5" y="3" width="13" height="10" rx="1.5"/><path d="M4.5 7l2 1.5-2 1.5M8 10h3.5"/>"#,
        "more" => r#"<circle cx="3.5" cy="8" r="1.2" fill="currentColor"/><circle cx="8" cy="8" r="1.2" fill="currentColor"/><circle cx="12.5" cy="8" r="1.2" fill="currentColor"/>"#,
        "split" => r#"<rect x="1.5" y="2.5" width="13" height="11" rx="1.2"/><path d="M8 2.5v11"/>"#,
        "edit" => r#"<path d="M11.5 2.5l2 2L6 12l-2.5.5L4 10z"/>"#,
        "trash" => r#"<path d="M3 4.5h10M6 4.5V3a1 1 0 0 1 1-1h2a1 1 0 0 1 1 1v1.5M4.5 4.5l.5 8a1 1 0 0 0 1 .9h4a1 1 0 0 0 1-.9l.5-8"/>"#,
        _ => "",
    };
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">{inner}</svg>"#
    )
}

// ──────────────────────────────────────────────────────────────────────────
// PTY cell rendering with vt100 colors
// ──────────────────────────────────────────────────────────────────────────

/// Build a single row's styled runs directly from the vt100 screen. Skips
/// the intermediate `CellSnap` allocation per cell that the old path used.
fn rebuild_row_runs(screen: &vt100::Screen, row: u16, cols: u16) -> Vec<StyledRun> {
    let mut runs: Vec<StyledRun> = Vec::new();
    let mut buf = String::new();
    let mut cur_fg: Option<Color> = None;
    let mut cur_bg: Option<Color> = None;
    let mut cur_bold = false;
    let mut started = false;

    for col in 0..cols {
        let (ch, fg, bg, bold) = match screen.cell(row, col) {
            Some(cell) => {
                let ch = cell.contents().chars().next().unwrap_or(' ');
                let mut fg = vt_color_opt(cell.fgcolor());
                let mut bg = vt_color_opt(cell.bgcolor());
                if cell.inverse() {
                    std::mem::swap(&mut fg, &mut bg);
                    if fg.is_none() {
                        fg = Some(c::BG);
                    }
                    if bg.is_none() {
                        bg = Some(c::FG);
                    }
                }
                (ch, fg, bg, cell.bold())
            }
            None => (' ', None, None, false),
        };
        if !started || fg != cur_fg || bg != cur_bg || bold != cur_bold {
            if !buf.is_empty() {
                runs.push(StyledRun {
                    text: std::mem::take(&mut buf),
                    fg: cur_fg,
                    bg: cur_bg,
                    bold: cur_bold,
                });
            }
            cur_fg = fg;
            cur_bg = bg;
            cur_bold = bold;
            started = true;
        }
        buf.push(ch);
    }
    if !buf.is_empty() {
        runs.push(StyledRun { text: buf, fg: cur_fg, bg: cur_bg, bold: cur_bold });
    }
    runs
}

/// Render the cached rows as 50 small `rich_text` widgets stacked in a
/// column. One giant `rich_text` is tempting (fewer widgets) but iced has
/// to reshape the entire block on every keypress, which makes input feel
/// laggy. Many small rows let iced amortize layout per row.
fn pty_widget_from_runs<'a>(rows: &[Vec<StyledRun>]) -> Element<'a, Msg> {
    use iced::widget::span;
    let bold_font = Font {
        weight: iced::font::Weight::Bold,
        ..MONO_FONT
    };
    let row_elems: Vec<Element<'a, Msg>> = rows
        .iter()
        .map(|runs| {
            let mut spans: Vec<iced::advanced::text::Span<'a, Msg, Font>> =
                Vec::with_capacity(runs.len().max(1));
            for run in runs {
                let mut sp = span(run.text.clone())
                    .font(if run.bold { bold_font } else { MONO_FONT })
                    .size(12.5);
                if let Some(c) = run.fg {
                    sp = sp.color(c);
                }
                if let Some(c) = run.bg {
                    sp = sp.background(c);
                }
                spans.push(sp);
            }
            if spans.is_empty() {
                spans.push(span(" ").font(MONO_FONT).size(12.5));
            }
            iced::widget::rich_text(spans).into()
        })
        .collect();
    column(row_elems).spacing(0).into()
}

fn vt_color_opt(c: vt100::Color) -> Option<Color> {
    match c {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(ansi_idx(i)),
        vt100::Color::Rgb(r, g, b) => Some(Color::from_rgb8(r, g, b)),
    }
}

fn ansi_idx(i: u8) -> Color {
    match i {
        0 => c::BG_STRIP,
        1 => c::RED,
        2 => c::GREEN,
        3 => c::YELLOW,
        4 => c::BLUE,
        5 => c::MAGENTA,
        6 => c::CYAN,
        7 => c::FG,
        8 => c::FG_MUTE,
        9 => c::RED,
        10 => c::GREEN,
        11 => c::YELLOW,
        12 => c::BLUE,
        13 => c::MAGENTA,
        14 => c::CYAN,
        15 => c::FG,
        16..=231 => {
            // 6×6×6 cube
            let n = i - 16;
            let r = n / 36;
            let g = (n % 36) / 6;
            let b = n % 6;
            let v = |x: u8| -> u8 {
                if x == 0 {
                    0
                } else {
                    55 + 40 * x
                }
            };
            Color::from_rgb8(v(r), v(g), v(b))
        }
        232..=255 => {
            let v = 8 + 10 * (i - 232);
            Color::from_rgb8(v, v, v)
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// keyboard → PTY byte mapping
// ──────────────────────────────────────────────────────────────────────────

fn key_to_bytes(key: &Key, mods: Modifiers) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    if mods.alt() {
        out.push(0x1b);
    }
    match key {
        Key::Character(s) => {
            if mods.control() {
                if let Some(ch) = s.chars().next() {
                    let b = (ch.to_ascii_uppercase() as u8).wrapping_sub(0x40);
                    out.push(b & 0x1f);
                } else {
                    return None;
                }
            } else {
                out.extend_from_slice(s.as_bytes());
            }
        }
        Key::Named(n) => match n {
            Named::Enter => out.push(b'\r'),
            Named::Tab => out.push(b'\t'),
            Named::Backspace => out.push(0x7f),
            Named::Escape => out.push(0x1b),
            Named::Space => out.push(b' '),
            Named::ArrowUp => out.extend_from_slice(b"\x1b[A"),
            Named::ArrowDown => out.extend_from_slice(b"\x1b[B"),
            Named::ArrowRight => out.extend_from_slice(b"\x1b[C"),
            Named::ArrowLeft => out.extend_from_slice(b"\x1b[D"),
            Named::Home => out.extend_from_slice(b"\x1b[H"),
            Named::End => out.extend_from_slice(b"\x1b[F"),
            Named::PageUp => out.extend_from_slice(b"\x1b[5~"),
            Named::PageDown => out.extend_from_slice(b"\x1b[6~"),
            Named::Delete => out.extend_from_slice(b"\x1b[3~"),
            Named::Insert => out.extend_from_slice(b"\x1b[2~"),
            _ => return None,
        },
        _ => return None,
    }
    Some(out)
}
