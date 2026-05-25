//! iced port of the grove TUI. The visual contract is `mockups/gui.html`.

use crate::agent::Agent;
use crate::app::{App, InputKind, Modal, Pane};
use crate::git::Worktree;
use crate::session::{Session, SessionStatus};
use anyhow::Result;
use iced::border::Radius;
use iced::keyboard::{self, key::Named, Key, Modifiers};
use iced::widget::canvas::{self, Frame, Geometry};
use iced::widget::{
    button, canvas as canvas_widget, column, container, row, scrollable, stack, svg, text, Column,
    Space,
};
use iced::{
    event, mouse, Background, Border, Color, Element, Event, Font, Length, Padding, Pixels, Point,
    Rectangle, Renderer, Shadow, Size, Subscription, Task, Theme,
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
const MONO_BOLD: Font = Font {
    weight: iced::font::Weight::Bold,
    ..Font::MONOSPACE
};

const ROW_H: f32 = 28.0;
const SUBTITLE_H: f32 = 14.0;
const RAIL_W: f32 = 320.0;
const APPBAR_H: f32 = 44.0;
const STATUS_H: f32 = 26.0;
const SESSBAR_H: f32 = 36.0;

/// Compute PTY dimensions from window pixel size. Subtracts the known fixed
/// chrome (rail, dividers, appbar, statusbar, sessbar, container padding) and
/// divides by the cell metrics to get rows × cols.
fn compute_pty_dims(win_w: f32, win_h: f32) -> (u16, u16) {
    // horizontal: rail (320) + divider (1) + pty container padding (18 × 2 = 36)
    let usable_w = win_w - RAIL_W - 1.0 - 36.0;
    // vertical: appbar (44) + statusbar (26) + sessbar (36) + pty padding (14 × 2 = 28)
    let usable_h = win_h - APPBAR_H - STATUS_H - SESSBAR_H - 28.0;
    let cols = (usable_w / CELL_W).max(10.0) as u16;
    let rows = (usable_h / CELL_H).max(4.0) as u16;
    (rows, cols)
}

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
    /// Current PTY dimensions derived from the window size. Updated on every
    /// `WindowResized` event and applied to sessions on spawn / select.
    pty_rows: u16,
    pty_cols: u16,
    /// Worktree whose split-start agent menu is open.
    open_agent_menu: Option<(usize, usize)>,
    /// Mouse-drag selection in the active session's PTY, in (row, col) cells.
    /// Tuple is (anchor, head) — un-normalized so we know which end is moving.
    pty_selection: Option<(PtyCell, PtyCell)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PtyCell {
    row: usize,
    col: usize,
}

struct PtyCacheEntry {
    /// One row per terminal line. Each row is a run-list of styled
    /// segments. Wrapped in `Arc` so the Canvas program can hold a cheap
    /// clone without copying ~8000 strings per frame.
    rows: Arc<Vec<Vec<StyledRun>>>,
    /// Iced canvas cache. The PTY draw skips entirely while the cache is
    /// warm — we `clear()` it only when `dirty` flips.
    cache: Arc<canvas::Cache>,
}

#[derive(Clone)]
struct StyledRun {
    text: String,
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
}

/// Hardcoded cell metrics for the MONOSPACE font at 12.5pt. iced doesn't
/// give us a cheap way to measure glyphs from outside a frame, so we pin
/// these — they only need to match the font roughly; the canvas just maps
/// (row, col) → pixel position.
const CELL_W: f32 = 7.6;
const CELL_H: f32 = 17.0;
const FONT_SIZE: f32 = 12.5;

#[derive(Debug, Clone)]
pub enum Msg {
    Tick,
    WindowResized(Size),
    BackendNative,
    BackendTmux,
    ProjectClicked(usize),
    WorktreeClicked {
        proj: usize,
        wt: usize,
    },
    StartSession {
        proj: usize,
        wt: usize,
        agent: Agent,
    },
    StartTerminal {
        proj: usize,
        wt: usize,
    },
    ToggleAgentMenu {
        proj: usize,
        wt: usize,
    },
    SelectSession(usize),
    KillSession(usize),
    KeyPress(Key, Modifiers),
    PtyMouseDown(f32, f32),
    PtyMouseDrag(f32, f32),
    PtyMouseUp,
    AddProject,
    AddWorktree {
        proj: usize,
    },
    DeleteWorktree {
        proj: usize,
        wt: usize,
    },
    ModalSubmit,
    ModalCancel,
    ModalConfirm(bool),
    ModalPickDir(String),
    ChooseTmux(bool),
    NoOp,
}

impl Grove {
    fn new() -> Self {
        // Compute initial PTY dimensions from the default window size (1280×800).
        // These are corrected on the first `WindowResized` event after startup.
        let (pty_rows, pty_cols) = compute_pty_dims(1280.0, 800.0);
        let mut app = App::new().expect("init app");
        // Resize any sessions discovered from a previous grove run so tmux
        // reports the correct terminal size immediately, not the INIT_ROWS/COLS
        // bootstrap values that `attach_existing` uses.
        for s in &mut app.sessions {
            s.resize(pty_rows, pty_cols);
        }
        let mut g = Self {
            app,
            collapsed: Default::default(),
            wt_cache: Default::default(),
            pty_cache: Default::default(),
            pty_rows,
            pty_cols,
            open_agent_menu: None,
            pty_selection: None,
        };
        // Prime the per-project worktree cache so `view()` never has to shell
        // out to `git worktree list` (it runs on every 33ms tick).
        let n = g.app.store.projects.len();
        for i in 0..n {
            g.ensure_wt_cached(i);
        }
        g
    }

    /// Switch the active project, saving the outgoing project's worktrees into
    /// `wt_cache` first so `tree_view` can still render its children while a
    /// different project is active.
    fn switch_active_project(&mut self, new_proj: usize) {
        if self.app.proj_idx != new_proj {
            let old = self.app.proj_idx;
            let wts = self.app.worktrees.clone();
            self.wt_cache.insert(old, wts);
            self.app.proj_idx = new_proj;
            self.app.refresh_worktrees();
            // Remove stale cache entry — live data is now in app.worktrees.
            self.wt_cache.remove(&new_proj);
        }
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

    /// Extract the text inside the current PTY selection from the cached
    /// styled rows of the active session. Returns `None` if there's no
    /// selection or no active session.
    fn selection_text(&self) -> Option<String> {
        let (a, h) = self.pty_selection?;
        let i = self.app.active_session?;
        let s = self.app.sessions.get(i)?;
        let key = Arc::as_ptr(&s.dirty) as usize;
        let map = self.pty_cache.borrow();
        let entry = map.get(&key)?;
        let rows = &entry.rows;
        if rows.is_empty() {
            return None;
        }
        let (r1, c1, r2, c2) = normalize_selection(a, h);
        let r1 = r1.min(rows.len() - 1);
        let r2 = r2.min(rows.len() - 1);
        let mut out = String::new();
        for r in r1..=r2 {
            let row = &rows[r];
            let row_text: String = row.iter().flat_map(|run| run.text.chars()).collect();
            let row_len = row_text.chars().count();
            let start = if r == r1 { c1 } else { 0 };
            let end = if r == r2 { c2.min(row_len) } else { row_len };
            let slice: String = row_text
                .chars()
                .skip(start)
                .take(end.saturating_sub(start))
                .collect();
            out.push_str(slice.trim_end());
            if r < r2 {
                out.push('\n');
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
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
                Event::Keyboard(keyboard::Event::KeyPressed {
                    modified_key,
                    modifiers,
                    ..
                }) => Some(Msg::KeyPress(modified_key, modifiers)),
                _ => None,
            }
        });
        let resize = iced::window::resize_events().map(|(_id, size)| Msg::WindowResized(size));
        Subscription::batch([tick, keys, resize])
    }

    fn update(&mut self, msg: Msg) -> Task<Msg> {
        match msg {
            Msg::Tick => {
                // No-op: `dirty` flags are consumed lazily by `pty()` when
                // it rebuilds a session's cached snapshot. Clearing them
                // here would force a full rebuild every tick.
            }
            Msg::WindowResized(size) => {
                let (rows, cols) = compute_pty_dims(size.width, size.height);
                self.pty_rows = rows;
                self.pty_cols = cols;
                if let Some(s) = self.app.active_session_mut() {
                    s.resize(rows, cols);
                }
            }
            Msg::BackendNative => {
                let _ = self.app.set_tmux_enabled(false);
            }
            Msg::BackendTmux => {
                let _ = self.app.set_tmux_enabled(true);
            }
            Msg::ChooseTmux(enabled) => {
                if let Err(e) = self.app.choose_tmux_enabled(enabled) {
                    self.app.modal = Modal::Message(format!("tmux setup failed: {e}"));
                }
            }
            Msg::ProjectClicked(i) => {
                self.open_agent_menu = None;
                if self.collapsed.contains(&i) {
                    self.collapsed.remove(&i);
                } else {
                    self.collapsed.insert(i);
                }
                self.switch_active_project(i);
                self.ensure_wt_cached(i);
            }
            Msg::WorktreeClicked { proj, wt } => {
                self.open_agent_menu = None;
                self.switch_active_project(proj);
                self.app.wt_idx = wt;
            }
            Msg::StartSession { proj, wt, agent } => {
                self.open_agent_menu = None;
                self.spawn(proj, wt, agent);
            }
            Msg::StartTerminal { proj, wt } => {
                self.open_agent_menu = None;
                self.spawn(proj, wt, Agent::Terminal);
            }
            Msg::ToggleAgentMenu { proj, wt } => {
                self.open_agent_menu = if self.open_agent_menu == Some((proj, wt)) {
                    None
                } else {
                    Some((proj, wt))
                };
            }
            Msg::SelectSession(i) => {
                self.open_agent_menu = None;
                if i < self.app.sessions.len() {
                    self.app.active_session = Some(i);
                    self.app.sessions[i].resize(self.pty_rows, self.pty_cols);
                }
            }
            Msg::KillSession(i) => {
                if i < self.app.sessions.len() {
                    let key = Arc::as_ptr(&self.app.sessions[i].dirty) as usize;
                    self.pty_cache.borrow_mut().remove(&key);
                    self.app.sessions[i].kill();
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
                if !matches!(self.app.modal, Modal::None) {
                    self.handle_modal_key(key, mods);
                    return Task::none();
                }
                // Ctrl+Shift+C copies the current PTY selection (if any) and
                // does NOT forward to the agent — this is the standard terminal
                // copy shortcut. Ctrl+C without shift still sends SIGINT.
                if mods.control() && mods.shift() {
                    if let Key::Character(s) = &key {
                        if s.eq_ignore_ascii_case("c") {
                            if let Some(text) = self.selection_text() {
                                crate::clipboard::copy(&text);
                            }
                            return Task::none();
                        }
                    }
                }
                if let Some(bytes) = key_to_bytes(&key, mods) {
                    if let Some(i) = self.app.active_session {
                        if let Some(s) = self.app.sessions.get_mut(i) {
                            s.send(&bytes);
                        }
                    }
                    self.pty_selection = None;
                }
            }
            Msg::PtyMouseDown(x, y) => {
                let cell = PtyCell {
                    row: (y / CELL_H).max(0.0) as usize,
                    col: (x / CELL_W).max(0.0) as usize,
                };
                self.pty_selection = Some((cell, cell));
            }
            Msg::PtyMouseDrag(x, y) => {
                let cell = PtyCell {
                    row: (y / CELL_H).max(0.0) as usize,
                    col: (x / CELL_W).max(0.0) as usize,
                };
                if let Some((a, _)) = self.pty_selection {
                    self.pty_selection = Some((a, cell));
                }
            }
            Msg::PtyMouseUp => {
                // Collapsed selection (single click, no drag) clears.
                if let Some((a, h)) = self.pty_selection {
                    if a == h {
                        self.pty_selection = None;
                    }
                }
            }
            Msg::AddProject => {
                self.open_agent_menu = None;
                self.app.focus_pane(Pane::Projects);
                self.app.start_add();
            }
            Msg::AddWorktree { proj } => {
                self.open_agent_menu = None;
                self.switch_active_project(proj);
                self.app.focus_pane(Pane::Worktrees);
                self.app.start_add();
            }
            Msg::DeleteWorktree { proj, wt } => {
                self.open_agent_menu = None;
                self.switch_active_project(proj);
                self.app.wt_idx = wt;
                self.app.focus_pane(Pane::Worktrees);
                self.app.start_delete();
            }
            Msg::ModalSubmit => self.submit_modal_input(),
            Msg::ModalCancel => self.cancel_modal(),
            Msg::ModalConfirm(yes) => self.submit_modal_confirm(yes),
            Msg::ModalPickDir(path) => {
                if let Modal::Input {
                    buffer,
                    kind,
                    dir_sel,
                    ..
                } = &mut self.app.modal
                {
                    if matches!(kind, InputKind::AddProjectPath) {
                        *buffer = format!("{path}/");
                        *dir_sel = 0;
                    }
                }
            }
            Msg::NoOp => {}
        }
        Task::none()
    }

    fn handle_modal_key(&mut self, key: Key, mods: Modifiers) {
        match &self.app.modal {
            Modal::Input { .. } => match key {
                Key::Named(Named::Escape) => self.cancel_modal(),
                Key::Named(Named::Enter) => self.submit_modal_input(),
                Key::Named(Named::ArrowDown) => self.app.input_dir_move(1),
                Key::Named(Named::ArrowUp) => self.app.input_dir_move(-1),
                Key::Named(Named::Tab) | Key::Named(Named::ArrowRight) => self.app.input_dir_pick(),
                Key::Named(Named::Backspace) => self.app.input_buffer_edit(|b| {
                    b.pop();
                }),
                Key::Named(Named::Space) if !mods.control() && !mods.alt() => {
                    self.app.input_buffer_edit(|b| b.push(' '));
                }
                Key::Character(s) => {
                    if mods.control() {
                        match s.as_str() {
                            "u" | "U" => self.app.input_buffer_edit(|b| b.clear()),
                            "c" | "C" => self.cancel_modal(),
                            _ => {}
                        }
                    } else if !mods.alt() {
                        self.app.input_buffer_edit(|b| b.push_str(&s));
                    }
                }
                _ => {}
            },
            Modal::Confirm { .. } => match key {
                Key::Named(Named::Escape) => self.submit_modal_confirm(false),
                Key::Named(Named::Enter) => self.submit_modal_confirm(true),
                Key::Character(s) => match s.as_str() {
                    "y" | "Y" => self.submit_modal_confirm(true),
                    "n" | "N" => self.submit_modal_confirm(false),
                    _ => {}
                },
                _ => {}
            },
            Modal::Message(_) => match key {
                Key::Named(Named::Escape) | Key::Named(Named::Enter) => self.cancel_modal(),
                Key::Character(s) if matches!(s.as_str(), "q" | "Q") => self.cancel_modal(),
                _ => {}
            },
            Modal::TmuxChoice => match key {
                Key::Named(Named::Enter) => {
                    if let Err(e) = self.app.choose_tmux_enabled(true) {
                        self.app.modal = Modal::Message(format!("tmux setup failed: {e}"));
                    }
                }
                Key::Named(Named::Escape) => {
                    if let Err(e) = self.app.choose_tmux_enabled(false) {
                        self.app.modal = Modal::Message(format!("tmux setup failed: {e}"));
                    }
                }
                Key::Character(s) => match s.as_str() {
                    "t" | "T" | "y" | "Y" => {
                        if let Err(e) = self.app.choose_tmux_enabled(true) {
                            self.app.modal = Modal::Message(format!("tmux setup failed: {e}"));
                        }
                    }
                    "n" | "N" => {
                        if let Err(e) = self.app.choose_tmux_enabled(false) {
                            self.app.modal = Modal::Message(format!("tmux setup failed: {e}"));
                        }
                    }
                    _ => {}
                },
                _ => {}
            },
            _ => {}
        }
    }

    fn submit_modal_input(&mut self) {
        if let Err(e) = self.app.submit_input() {
            self.app.modal = Modal::Message(format!("input failed: {e}"));
        }
        self.rebuild_wt_cache();
    }

    fn submit_modal_confirm(&mut self, yes: bool) {
        if let Err(e) = self.app.submit_confirm(yes) {
            self.app.modal = Modal::Message(format!("action failed: {e}"));
        }
        self.rebuild_wt_cache();
    }

    fn cancel_modal(&mut self) {
        self.app.modal = Modal::None;
    }

    fn rebuild_wt_cache(&mut self) {
        self.wt_cache.clear();
        let n = self.app.store.projects.len();
        if self.app.proj_idx >= n {
            self.app.proj_idx = n.saturating_sub(1);
        }
        self.app.refresh_worktrees();
        for i in 0..n {
            self.ensure_wt_cached(i);
        }
    }

    fn spawn(&mut self, proj: usize, wt: usize, agent: Agent) {
        self.switch_active_project(proj);
        self.app.wt_idx = wt;
        let pname = match self.app.store.projects.get(proj) {
            Some(p) => p.name.clone(),
            None => return,
        };
        let Some(w) = self.app.worktrees.get(wt).cloned() else {
            return;
        };
        let label = if w.is_main {
            pname.clone()
        } else {
            crate::app::path_basename(&w.path)
        };
        let args = agent.launch_args();
        let use_tmux = self.app.use_tmux();
        match Session::spawn(
            label,
            pname,
            w.path.clone(),
            agent,
            &args,
            &w.path,
            use_tmux,
        ) {
            Ok(mut s) => {
                s.resize(self.pty_rows, self.pty_cols);
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
            row![self.sidebar(), divider_v(c::BORDER), self.workspace()]
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
                background: Some(Background::Color(c::BG)),
                text_color: Some(c::FG),
                ..Default::default()
            })
            .into()
    }

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
            188.0 + (visible_matches as f32 * ROW_H)
        } else {
            164.0
        };
        let modal_w = if show_dirs { 640.0 } else { 480.0 };

        let input = container(
            row![
                text(buffer.to_string())
                    .font(MONO_FONT)
                    .size(13)
                    .color(c::FG)
                    .wrapping(iced::widget::text::Wrapping::None),
                container(Space::with_width(7))
                    .width(7)
                    .height(15)
                    .style(|_| container::Style {
                        background: Some(Background::Color(c::CYAN)),
                        ..Default::default()
                    }),
            ]
            .spacing(1)
            .align_y(iced::Alignment::Center),
        )
        .height(34)
        .width(Length::Fill)
        .align_y(iced::Alignment::Center)
        .padding(Padding::from([0, 10]))
        .clip(true)
        .style(|_| container::Style {
            background: Some(Background::Color(c::BG_STRIP)),
            border: Border {
                color: c::BORDER,
                width: 1.0,
                radius: Radius::from(5.0),
            },
            ..Default::default()
        });

        let mut body =
            column![text(title.to_string()).size(13).color(c::MAGENTA), input,].spacing(10);

        if show_dirs {
            let mut matches_col = Column::new()
                .spacing(0)
                .height(Length::Fixed(visible_matches as f32 * ROW_H));
            if entries.is_empty() {
                matches_col = matches_col.push(
                    container(text("no matches").size(12).color(c::FG_MUTE))
                        .height(ROW_H)
                        .align_y(iced::Alignment::Center),
                );
            } else {
                for (i, path) in entries.into_iter().take(6).enumerate() {
                    matches_col = matches_col.push(modal_dir_row(path, i == dir_sel));
                }
            }

            body = body
                .push(text("matches").size(11).color(c::FG_MUTE))
                .push(matches_col);
        }

        let hints = if show_dirs {
            modal_hints(&[
                ("enter", "submit"),
                ("up/down", "pick"),
                ("tab", "complete"),
                ("esc", "cancel"),
            ])
        } else {
            modal_hints(&[("enter", "submit"), ("esc", "cancel")])
        };

        body = body.push(Space::with_height(6)).push(
            row![
                hints,
                Space::with_width(Length::Fill),
                modal_action("cancel", false, Msg::ModalCancel),
                modal_action("submit", true, Msg::ModalSubmit),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        );

        modal_panel(body.into(), modal_w, modal_h, c::MAGENTA)
    }

    fn confirm_modal<'a>(
        &'a self,
        title: &'a str,
        prompt: &'a str,
        destructive: bool,
    ) -> Element<'a, Msg> {
        let accent = if destructive { c::RED } else { c::MAGENTA };
        let body = column![
            text(title.to_string()).size(12).color(accent),
            text(prompt.to_string())
                .size(13)
                .color(c::FG_DIM)
                .wrapping(iced::widget::text::Wrapping::Word),
            Space::with_height(6),
            row![
                text("Y confirm   N/Esc cancel").size(11).color(c::FG_MUTE),
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

        modal_panel(body.into(), 520.0, 190.0, accent)
    }

    fn message_modal<'a>(&'a self, message: &'a str) -> Element<'a, Msg> {
        let body = column![
            text("notice").size(12).color(c::CYAN),
            text(message.to_string())
                .size(13)
                .color(c::FG_DIM)
                .wrapping(iced::widget::text::Wrapping::Word),
            Space::with_height(6),
            row![
                text("Enter/Esc close").size(11).color(c::FG_MUTE),
                Space::with_width(Length::Fill),
                modal_action("close", true, Msg::ModalCancel),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        modal_panel(body.into(), 480.0, 170.0, c::CYAN)
    }

    fn tmux_choice_modal(&self) -> Element<'_, Msg> {
        let body = column![
            text("session backend").size(12).color(c::CYAN),
            text("Use tmux for new sessions? Existing sessions keep their current backend.")
                .size(13)
                .color(c::FG_DIM)
                .wrapping(iced::widget::text::Wrapping::Word),
            Space::with_height(6),
            row![
                text("T/Enter tmux   N/Esc native")
                    .size(11)
                    .color(c::FG_MUTE),
                Space::with_width(Length::Fill),
                modal_action("native", false, Msg::ChooseTmux(false)),
                modal_action("tmux", true, Msg::ChooseTmux(true)),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        modal_panel(body.into(), 520.0, 180.0, c::CYAN)
    }

    // ── appbar ──────────────────────────────────────────────────────────
    fn appbar(&self) -> Element<'_, Msg> {
        let brand = row![
            text("grove").font(MONO_BOLD).size(14.0).color(c::MAGENTA),
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

        let right = row![seg, icon_btn("cog", Msg::NoOp), icon_btn("help", Msg::NoOp),]
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

        let bar = container(inner)
            .height(APPBAR_H)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP)),
                ..Default::default()
            });

        column![bar, divider_h(c::BORDER)].into()
    }

    // ── sidebar ─────────────────────────────────────────────────────────
    fn sidebar(&self) -> Element<'_, Msg> {
        let tree = self.tree_view();
        let tree_area = container(scrollable(tree).height(Length::Fill))
            .height(Length::Fill)
            .padding(Padding {
                top: 6.0,
                bottom: 14.0,
                left: 0.0,
                right: 0.0,
            });
        let tree_layer: Element<'_, Msg> = match self.open_agent_menu_top() {
            Some((proj, wt, top)) => stack![tree_area, sidebar_agent_menu_overlay(proj, wt, top),]
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            None => tree_area.into(),
        };

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

        let stack = column![tree_layer, divider_h(c::BORDER_SOFT), add_proj,].height(Length::Fill);

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
                    col = col.push(worktree_row(pi, wi, &wname, &w.branch, active_wt, w.is_main));

                    for (si, s) in self.app.sessions.iter().enumerate() {
                        if s.wt_path == w.path {
                            let active = self.app.active_session == Some(si);
                            col = col.push(session_row(si, s, active));
                        }
                    }
                }
                col = col.push(add_worktree_row(pi));
            }
        }
        col.into()
    }

    fn open_agent_menu_top(&self) -> Option<(usize, usize, f32)> {
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
                    return Some((pi, wi, 6.0 + acc_y + ROW_H));
                }
                acc_y += ROW_H; // worktree row

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
            acc_y += ROW_H; // "+ new worktree" row at the end of each expanded project
        }

        None
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
            text(s.agent.label())
                .font(MONO_FONT)
                .size(12)
                .color(c::MAGENTA),
            text("·").color(c::FG_MUTE),
            text(s.project.clone())
                .font(MONO_FONT)
                .size(12)
                .color(c::BLUE),
            text("/").color(c::FG_MUTE),
            text(s.label.clone()).font(MONO_FONT).size(12).color(c::FG),
            text(format!("[{}]", s.branch))
                .font(MONO_FONT)
                .size(12)
                .color(c::FG_MUTE),
            Space::with_width(Length::Fill),
            text(s.wt_path.clone())
                .font(MONO_FONT)
                .size(12)
                .color(c::FG_MUTE),
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
        // PTY render uses a Canvas. We cache the styled-row snapshot AND
        // the `canvas::Cache` per session: switching to a quiet session
        // returns the cached geometry with zero draw work; switching to a
        // session that produced output re-snaps the rows and clears the
        // canvas cache, then draws once.
        let key = Arc::as_ptr(&s.dirty) as usize;
        let (rows, cache) = {
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
                cache: Arc::new(canvas::Cache::default()),
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
            }
            (Arc::clone(&entry.rows), Arc::clone(&entry.cache))
        };

        let rows_len = rows.len() as f32;
        let cols = rows
            .first()
            .map(|r| r.iter().map(|run| run.text.chars().count()).sum::<usize>())
            .unwrap_or(0) as f32;
        let program = PtyProgram {
            rows,
            cache,
            selection: self.pty_selection,
        };
        let body: Element<'_, Msg> = canvas_widget(program)
            .width(Length::Fixed((cols * CELL_W).max(CELL_W)))
            .height(Length::Fixed((rows_len * CELL_H).max(CELL_H)))
            .into();

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
                dot(if running > 0 { c::GREEN } else { c::FG_MUTE }),
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
    let count_color = if count > 0 { c::GREEN } else { c::FG_MUTE };
    let row_content = row![
        container(icon(twist, 10.0, c::FG_MUTE))
            .width(14)
            .center_y(Length::Fill),
        text(name.to_string()).size(13).color(c::FG),
        text(format!("● {count}"))
            .font(MONO_FONT)
            .size(11)
            .color(count_color),
        Space::with_width(Length::Fill),
    ]
    .spacing(6)
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
    is_main: bool,
) -> Element<'a, Msg> {
    // Split layout — action buttons are siblings of the left button, NOT
    // nested inside it. Nesting buttons inside a button causes both to fire
    // on a single click (iced 0.13 does not propagate captured-event status
    // through the outer button's on_event handler).
    let show_branch = branch != name;

    let label: Element<'a, Msg> = if show_branch {
        row![
            text(name.to_string())
                .size(13)
                .color(c::FG_DIM)
                .wrapping(iced::widget::text::Wrapping::None),
            text(format!(" · {branch}"))
                .font(MONO_FONT)
                .size(11)
                .color(c::FG_MUTE)
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(0)
        .align_y(iced::Alignment::Center)
        .into()
    } else {
        text(name.to_string())
            .size(13)
            .color(c::FG_DIM)
            .wrapping(iced::widget::text::Wrapping::None)
            .into()
    };

    // Left clickable area: chevron + name/branch, fills available space.
    let left_content = row![
        container(icon("chev-right", 10.0, c::FG_MUTE))
            .width(14)
            .center_y(Length::Fill),
        container(label).width(Length::Fill).clip(true),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center)
    .padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 28.0,
        right: 6.0,
    });

    let bg_opt = if active {
        Some(Background::Color(c::BG_HL))
    } else {
        None
    };
    let left_btn = button(
        container(left_content)
            .height(ROW_H)
            .width(Length::Fill)
            .align_y(iced::Alignment::Center),
    )
    .on_press(Msg::WorktreeClicked { proj, wt })
    .width(Length::Fill)
    .padding(0)
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered && !active {
                Some(Background::Color(c::BG_HOVER))
            } else {
                bg_opt
            },
            text_color: if active || hovered { c::FG } else { c::FG_DIM },
            border: Border::default(),
            shadow: Shadow::default(),
        }
    });

    // Right action buttons: siblings of left_btn, never nested inside it.
    let mut actions = row![split_start_button(proj, wt)]
        .spacing(6)
        .align_y(iced::Alignment::Center);
    if !is_main {
        actions = actions.push(delete_worktree_button(proj, wt));
    }
    let actions = actions.padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 0.0,
        right: 8.0,
    });

    let row_body = container(row![left_btn, actions].align_y(iced::Alignment::Center))
        .height(ROW_H)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: if active {
                Some(Background::Color(c::BG_HL))
            } else {
                None
            },
            ..Default::default()
        });

    row_body.into()
}

fn add_worktree_row<'a>(proj: usize) -> Element<'a, Msg> {
    let content = row![
        Space::with_width(28),
        text("+ new worktree").size(12).color(c::FG_MUTE),
        Space::with_width(Length::Fill),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center)
    .padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 16.0,
        right: 8.0,
    });
    button(
        container(content)
            .height(ROW_H)
            .width(Length::Fill)
            .align_y(iced::Alignment::Center),
    )
    .on_press(Msg::AddWorktree { proj })
    .width(Length::Fill)
    .padding(0)
    .style(|_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: if hovered {
                Some(Background::Color(c::BG_HOVER))
            } else {
                None
            },
            text_color: if hovered { c::FG_DIM } else { c::FG_MUTE },
            border: Border::default(),
            shadow: Shadow::default(),
        }
    })
    .into()
}

fn session_row<'a>(idx: usize, s: &Session, active: bool) -> Element<'a, Msg> {
    let running = matches!(*s.status.lock().unwrap(), SessionStatus::Running);
    let dot_color = if running { c::GREEN } else { c::FG_MUTE };
    let agent_color = if active { c::CYAN } else { c::FG };

    let subtitle = s
        .current_title()
        .filter(|t| !t.eq_ignore_ascii_case(&s.label) && !t.eq_ignore_ascii_case(s.agent.label()));

    // Agent label + session name fill the remaining space, clipping if too
    // long. The close button stays pinned to the right edge — same pattern
    // as worktree_row's `1fr auto` grid in the HTML mockup.
    let meta: Element<'a, Msg> = container(
        row![
            text(s.agent.label())
                .font(MONO_FONT)
                .size(12)
                .color(agent_color),
            text("·").size(11).color(c::FG_MUTE),
            text(s.label.clone())
                .font(MONO_FONT)
                .size(11)
                .color(c::FG_MUTE)
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center),
    )
    .width(Length::Fill)
    .clip(true)
    .into();

    let main_row: Element<'a, Msg> = row![
        Space::with_width(28),
        dot(dot_color),
        meta,
        action_mini("close", Msg::KillSession(idx)),
    ]
    .spacing(8)
    .align_y(iced::Alignment::Center)
    .padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: 16.0,
        right: 8.0,
    })
    .into();

    let row_h = if subtitle.is_some() {
        ROW_H + SUBTITLE_H
    } else {
        ROW_H
    };

    let row_content: Element<'a, Msg> = match subtitle {
        Some(t) => column![
            main_row,
            container(
                text(t)
                    .font(MONO_FONT)
                    .size(10)
                    .color(c::FG_MUTE)
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .padding(Padding {
                top: 0.0,
                bottom: 0.0,
                left: 52.0,
                right: 8.0,
            })
            .width(Length::Fill)
            .clip(true),
        ]
        .into(),
        None => main_row,
    };

    clickable_row(row_content, row_h, active, Msg::SelectSession(idx))
}

fn clickable_row<'a>(
    content: impl Into<Element<'a, Msg>>,
    height: f32,
    active: bool,
    on_press: Msg,
) -> Element<'a, Msg> {
    let bg = if active {
        Some(Background::Color(c::BG_HL))
    } else {
        None
    };
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
            text_color: if active || hovered { c::FG } else { c::FG_DIM },
            border: Border::default(),
            shadow: Shadow::default(),
        }
    })
    .into()
}

fn modal_panel<'a>(
    content: Element<'a, Msg>,
    width: f32,
    height: f32,
    accent: Color,
) -> Element<'a, Msg> {
    container(content)
        .width(width)
        .height(height)
        .padding(Padding::from([16, 18]))
        .style(move |_| container::Style {
            background: Some(Background::Color(c::BG)),
            text_color: Some(c::FG),
            border: Border {
                color: accent,
                width: 1.0,
                radius: Radius::from(5.0),
            },
            ..Default::default()
        })
        .into()
}

fn modal_action<'a>(label: &'static str, primary: bool, msg: Msg) -> Element<'a, Msg> {
    button(text(label).size(11.5))
        .on_press(msg)
        .padding(Padding::from([6, 12]))
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            let bg = if primary {
                if hovered {
                    c::BG_HOVER
                } else {
                    c::BG_HL
                }
            } else if hovered {
                c::BG_HOVER
            } else {
                c::BG
            };
            button::Style {
                background: Some(Background::Color(bg)),
                text_color: if primary { c::FG } else { c::FG_DIM },
                border: Border {
                    color: c::BORDER,
                    width: 1.0,
                    radius: Radius::from(5.0),
                },
                shadow: Shadow::default(),
            }
        })
        .into()
}

fn modal_hints<'a>(pairs: &[(&'static str, &'static str)]) -> Element<'a, Msg> {
    let mut hints = iced::widget::Row::new()
        .spacing(8)
        .align_y(iced::Alignment::Center);
    for (key, label) in pairs {
        hints = hints.push(
            row![
                text(*key).font(MONO_FONT).size(11).color(c::YELLOW),
                text(*label).size(11).color(c::FG_MUTE),
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center),
        );
    }
    hints.into()
}

fn modal_dir_row<'a>(path: String, active: bool) -> Element<'a, Msg> {
    let msg_path = path.clone();
    button(
        container(
            text(path)
                .font(MONO_FONT)
                .size(12)
                .color(if active { c::FG } else { c::CYAN })
                .wrapping(iced::widget::text::Wrapping::None),
        )
        .height(ROW_H)
        .width(Length::Fill)
        .align_y(iced::Alignment::Center)
        .padding(Padding::from([0, 8]))
        .clip(true),
    )
    .on_press(Msg::ModalPickDir(msg_path))
    .width(Length::Fill)
    .padding(0)
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
            text_color: if active || hovered { c::FG } else { c::CYAN },
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

fn divider_v<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::with_width(1))
        .width(1)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

fn seg_button<'a>(label: &str, active: bool, msg: Msg) -> Element<'a, Msg> {
    button(text(label.to_string()).size(11))
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
            background: if hovered {
                Some(Background::Color(c::BG_HOVER))
            } else {
                None
            },
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

fn split_start_button<'a>(proj: usize, wt: usize) -> Element<'a, Msg> {
    let launch = button(
        container(icon("play", 9.0, c::GREEN))
            .center_x(28)
            .center_y(22),
    )
    .on_press(Msg::StartSession {
        proj,
        wt,
        agent: Agent::Claude,
    })
    .padding(0)
    .style(split_start_style(SplitStartSegment::Left));

    let terminal = button(
        container(icon("term", 12.0, c::FG_MUTE))
            .center_x(28)
            .center_y(22),
    )
    .on_press(Msg::StartTerminal { proj, wt })
    .padding(0)
    .style(split_start_style(SplitStartSegment::Middle));

    let menu = button(
        container(icon("more", 12.0, c::FG_MUTE))
            .center_x(22)
            .center_y(22),
    )
    .on_press(Msg::ToggleAgentMenu { proj, wt })
    .padding(0)
    .style(split_start_style(SplitStartSegment::Right));

    row![launch, terminal, menu]
        .spacing(0)
        .align_y(iced::Alignment::Center)
        .into()
}

fn delete_worktree_button<'a>(proj: usize, wt: usize) -> Element<'a, Msg> {
    button(
        container(icon("trash", 11.0, c::RED))
            .center_x(24)
            .center_y(22),
    )
    .on_press(Msg::DeleteWorktree { proj, wt })
    .padding(0)
    .style(|_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: Some(Background::Color(if hovered {
                c::BG_HOVER
            } else {
                c::BG
            })),
            text_color: c::RED,
            border: Border {
                color: if hovered { c::RED } else { c::BORDER },
                width: 1.0,
                radius: Radius::from(4.0),
            },
            shadow: Shadow::default(),
        }
    })
    .into()
}

#[derive(Clone, Copy)]
enum SplitStartSegment {
    Left,
    Middle,
    Right,
}

fn split_start_style(
    segment: SplitStartSegment,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        let radius = match segment {
            SplitStartSegment::Left => Radius::default().left(4.0),
            SplitStartSegment::Middle => Radius::default(),
            SplitStartSegment::Right => Radius::default().right(4.0),
        };
        button::Style {
            background: Some(Background::Color(if hovered { c::BG_HOVER } else { c::BG })),
            text_color: if hovered { c::FG } else { c::FG_DIM },
            border: Border {
                color: c::BORDER,
                width: 1.0,
                radius,
            },
            shadow: Shadow::default(),
        }
    }
}

fn sidebar_agent_menu_overlay<'a>(proj: usize, wt: usize, top: f32) -> Element<'a, Msg> {
    container(
        column![
            Space::with_height(top),
            row![Space::with_width(Length::Fill), agent_menu(proj, wt)].padding(Padding {
                top: 0.0,
                bottom: 0.0,
                left: 0.0,
                right: 8.0,
            }),
            Space::with_height(Length::Fill),
        ]
        .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn agent_menu<'a>(proj: usize, wt: usize) -> Element<'a, Msg> {
    let item = |agent: Agent| {
        button(
            container(
                text(agent.label())
                    .font(MONO_FONT)
                    .size(11)
                    .color(c::FG_DIM),
            )
            .width(Length::Fill)
            .center_y(24)
            .padding(Padding::from([0, 8])),
        )
        .on_press(Msg::StartSession { proj, wt, agent })
        .width(Length::Fill)
        .padding(0)
        .style(|_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: if hovered {
                    Some(Background::Color(c::BG_HOVER))
                } else {
                    None
                },
                text_color: if hovered { c::FG } else { c::FG_DIM },
                border: Border::default(),
                shadow: Shadow::default(),
            }
        })
    };

    container(column![item(Agent::Codex), item(Agent::OpenCode)].spacing(0))
        .width(96)
        .padding(Padding::from([3, 0]))
        .style(|_| container::Style {
            background: Some(Background::Color(c::BG)),
            border: Border {
                color: c::BORDER,
                width: 1.0,
                radius: Radius::from(4.0),
            },
            ..Default::default()
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
            background: if hovered {
                Some(Background::Color(c::BG_HOVER))
            } else {
                None
            },
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
            if danger {
                c::RED
            } else {
                c::FG
            }
        } else {
            c::FG_DIM
        };
        button::Style {
            background: if hovered {
                Some(Background::Color(c::BG_HOVER))
            } else {
                None
            },
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
        "cog" => {
            r#"<circle cx="8" cy="8" r="2"/><path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.5 3.5l1.4 1.4M11.1 11.1l1.4 1.4M3.5 12.5l1.4-1.4M11.1 4.9l1.4-1.4"/>"#
        }
        "help" => {
            r#"<circle cx="8" cy="8" r="6.2"/><path d="M6 6.2c0-1.1 0.9-2 2-2s2 0.9 2 2c0 1.1-2 1.4-2 2.6M8 11.6v0.2"/>"#
        }
        "search" => r#"<circle cx="7" cy="7" r="4.5"/><path d="M10.4 10.4l3 3"/>"#,
        "term" => {
            r#"<rect x="1.5" y="3" width="13" height="10" rx="1.5"/><path d="M4.5 7l2 1.5-2 1.5M8 10h3.5"/>"#
        }
        "more" => {
            r#"<circle cx="3.5" cy="8" r="1.2" fill="currentColor"/><circle cx="8" cy="8" r="1.2" fill="currentColor"/><circle cx="12.5" cy="8" r="1.2" fill="currentColor"/>"#
        }
        "split" => {
            r#"<rect x="1.5" y="2.5" width="13" height="11" rx="1.2"/><path d="M8 2.5v11"/>"#
        }
        "edit" => r#"<path d="M11.5 2.5l2 2L6 12l-2.5.5L4 10z"/>"#,
        "trash" => {
            r#"<path d="M3 4.5h10M6 4.5V3a1 1 0 0 1 1-1h2a1 1 0 0 1 1 1v1.5M4.5 4.5l.5 8a1 1 0 0 0 1 .9h4a1 1 0 0 0 1-.9l.5-8"/>"#
        }
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
        runs.push(StyledRun {
            text: buf,
            fg: cur_fg,
            bg: cur_bg,
            bold: cur_bold,
        });
    }
    runs
}

/// Custom `canvas::Program` that paints PTY cells directly using
/// `fill_rectangle` and `fill_text`. Bypasses iced's text layout pipeline
/// entirely — switching sessions just calls `cache.clear()` and one frame
/// re-paint, no per-row rich_text shaping.
struct PtyProgram {
    rows: Arc<Vec<Vec<StyledRun>>>,
    cache: Arc<canvas::Cache>,
    selection: Option<(PtyCell, PtyCell)>,
}

#[derive(Default)]
struct PtyProgramState {
    dragging: bool,
}

impl canvas::Program<Msg> for PtyProgram {
    type State = PtyProgramState;

    fn update(
        &self,
        state: &mut PtyProgramState,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Msg>) {
        let local = |cursor: mouse::Cursor| -> Option<Point> {
            cursor.position_in(bounds).or_else(|| {
                cursor.position().map(|p| {
                    Point::new(
                        (p.x - bounds.x).clamp(0.0, bounds.width.max(0.0)),
                        (p.y - bounds.y).clamp(0.0, bounds.height.max(0.0)),
                    )
                })
            })
        };
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(p) = cursor.position_in(bounds) {
                    state.dragging = true;
                    return (
                        canvas::event::Status::Captured,
                        Some(Msg::PtyMouseDown(p.x, p.y)),
                    );
                }
                (canvas::event::Status::Ignored, None)
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => {
                if let Some(p) = local(cursor) {
                    return (
                        canvas::event::Status::Captured,
                        Some(Msg::PtyMouseDrag(p.x, p.y)),
                    );
                }
                (canvas::event::Status::Ignored, None)
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.dragging =>
            {
                state.dragging = false;
                (canvas::event::Status::Captured, Some(Msg::PtyMouseUp))
            }
            _ => (canvas::event::Status::Ignored, None),
        }
    }

    fn mouse_interaction(
        &self,
        _state: &PtyProgramState,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        _state: &PtyProgramState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<Renderer>> {
        let bold_font = Font {
            weight: iced::font::Weight::Bold,
            ..MONO_FONT
        };
        let geom = self
            .cache
            .draw(renderer, bounds.size(), |frame: &mut Frame| {
                for (r_i, row) in self.rows.iter().enumerate() {
                    let y = r_i as f32 * CELL_H;
                    let mut col_i: usize = 0;
                    for run in row {
                        let n = run.text.chars().count();
                        let x = col_i as f32 * CELL_W;
                        let w = n as f32 * CELL_W;
                        if let Some(bg) = run.bg {
                            frame.fill_rectangle(Point::new(x, y), Size::new(w, CELL_H), bg);
                        }
                        frame.fill_text(canvas::Text {
                            content: run.text.clone(),
                            position: Point::new(x, y),
                            color: run.fg.unwrap_or(c::FG),
                            size: Pixels(FONT_SIZE),
                            line_height: iced::widget::text::LineHeight::Absolute(Pixels(CELL_H)),
                            font: if run.bold { bold_font } else { MONO_FONT },
                            horizontal_alignment: iced::alignment::Horizontal::Left,
                            vertical_alignment: iced::alignment::Vertical::Top,
                            shaping: iced::widget::text::Shaping::Advanced,
                        });
                        col_i += n;
                    }
                }
            });
        let mut out = vec![geom];
        if let Some((a, h)) = self.selection {
            let cols = self
                .rows
                .first()
                .map(|r| r.iter().map(|run| run.text.chars().count()).sum::<usize>())
                .unwrap_or(0);
            let rows = self.rows.len();
            let mut overlay = Frame::new(renderer, bounds.size());
            paint_selection(&mut overlay, a, h, rows, cols);
            out.push(overlay.into_geometry());
        }
        out
    }
}

fn paint_selection(frame: &mut Frame, a: PtyCell, h: PtyCell, rows: usize, cols: usize) {
    if rows == 0 || cols == 0 {
        return;
    }
    let (r1, c1, r2, c2) = normalize_selection(a, h);
    let r1 = r1.min(rows - 1);
    let r2 = r2.min(rows - 1);
    let c1 = c1.min(cols);
    let c2 = c2.min(cols);
    let color = Color {
        r: 0.40,
        g: 0.50,
        b: 0.78,
        a: 0.35,
    };
    if r1 == r2 {
        let x = c1 as f32 * CELL_W;
        let y = r1 as f32 * CELL_H;
        let w = ((c2.saturating_sub(c1)).max(1)) as f32 * CELL_W;
        frame.fill_rectangle(Point::new(x, y), Size::new(w, CELL_H), color);
        return;
    }
    let row_w = cols as f32 * CELL_W;
    let x1 = c1 as f32 * CELL_W;
    let y1 = r1 as f32 * CELL_H;
    frame.fill_rectangle(
        Point::new(x1, y1),
        Size::new((row_w - x1).max(CELL_W), CELL_H),
        color,
    );
    if r2 > r1 + 1 {
        let ym = (r1 + 1) as f32 * CELL_H;
        let hm = (r2 - r1 - 1) as f32 * CELL_H;
        frame.fill_rectangle(Point::new(0.0, ym), Size::new(row_w, hm), color);
    }
    let y2 = r2 as f32 * CELL_H;
    let w2 = c2 as f32 * CELL_W;
    if w2 > 0.0 {
        frame.fill_rectangle(Point::new(0.0, y2), Size::new(w2, CELL_H), color);
    }
}

fn normalize_selection(a: PtyCell, b: PtyCell) -> (usize, usize, usize, usize) {
    if (a.row, a.col) <= (b.row, b.col) {
        (a.row, a.col, b.row, b.col)
    } else {
        (b.row, b.col, a.row, a.col)
    }
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
