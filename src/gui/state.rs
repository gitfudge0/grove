//! Shared types: the top-level `Grove` model, the `Msg` enum dispatched by
//! iced, and PTY-render caching primitives.

use crate::agent::Agent;
use crate::app::App;
use crate::git::Worktree;
use iced::keyboard::{Key, Modifiers};
use iced::widget::canvas;
use iced::{Color, Size};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Top-level iced application state.
pub struct Grove {
    pub app: App,
    pub collapsed: HashSet<usize>,
    /// Cache of worktrees per project index. Refilled on project expand /
    /// session spawn/kill — never inside `view()`, since `git worktree list`
    /// is a subprocess and `view()` runs on every 33ms tick.
    pub wt_cache: HashMap<usize, Vec<Worktree>>,
    /// Cached PTY screen snapshots, keyed by the `dirty` Arc's pointer
    /// (stable & unique per Session). Rebuilt only when the session's dirty
    /// flag was set since last build — so switching to a quiet session is
    /// free, and the per-frame parser lock is taken only for sessions that
    /// actually changed.
    pub pty_cache: RefCell<HashMap<usize, PtyCacheEntry>>,
    /// Current PTY dimensions derived from window size. Updated on every
    /// `WindowResized` event and applied to sessions on spawn / select.
    pub pty_rows: u16,
    pub pty_cols: u16,
    /// Worktree whose split-start agent menu is open.
    pub open_agent_menu: Option<(usize, usize)>,
    /// Mouse-drag selection in the active session's PTY, in (row, col) cells.
    /// Un-normalized so we know which end is moving.
    pub pty_selection: Option<(PtyCell, PtyCell)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PtyCell {
    pub row: usize,
    pub col: usize,
}

pub struct PtyCacheEntry {
    /// One row per terminal line. Each row is a run-list of styled segments.
    /// Wrapped in `Arc` so the Canvas program can hold a cheap clone without
    /// copying ~8000 strings per frame.
    pub rows: Arc<Vec<Vec<StyledRun>>>,
    /// Iced canvas cache. The PTY draw skips entirely while warm — we
    /// `clear()` it only when `dirty` flips.
    pub cache: Arc<canvas::Cache>,
}

#[derive(Clone)]
pub struct StyledRun {
    pub text: String,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Tick,
    WindowResized(Size),
    BackendNative,
    BackendTmux,
    ProjectClicked(usize),
    WorktreeClicked { proj: usize, wt: usize },
    StartSession { proj: usize, wt: usize, agent: Agent },
    StartTerminal { proj: usize, wt: usize },
    ToggleAgentMenu { proj: usize, wt: usize },
    SelectSession(usize),
    KillSession(usize),
    KeyPress(Key, Modifiers),
    PtyMouseDown(f32, f32),
    PtyMouseDrag(f32, f32),
    PtyMouseUp,
    AddProject,
    AddWorktree { proj: usize },
    DeleteWorktree { proj: usize, wt: usize },
    ModalSubmit,
    ModalCancel,
    ModalConfirm(bool),
    ModalPickDir(String),
    ChooseTmux(bool),
    NoOp,
}

#[derive(Clone, Copy)]
pub enum SplitStartSegment {
    Left,
    Middle,
    Right,
}
