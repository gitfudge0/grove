//! One live terminal: a PTY plus the `grove_terminal::GroveTerm` model that parses it.

use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use futures::channel::mpsc;
use futures::StreamExt as _;
use gpui::{Context, Task};
use grove_core::session_meta::{self, SessionMeta};
use grove_core::tmux;
use grove_terminal::{GroveTerm, MouseMode, PtyHandle, Snapshot};

use crate::entities::session_registry::SpawnTarget;
use crate::terminal::keys;
use crate::terminal::mouse::{self, AbsCell};
use crate::terminal_element::{TermScene, TermSceneKey};
use portable_pty::CommandBuilder;

/// Initial PTY size before the element's first `prepaint` reports real bounds (`crates/grove-core/src/session.rs:53-54`).
const INIT_ROWS: u16 = 24;
const INIT_COLS: u16 = 80;

fn output_age_at(last_output_at: Option<Instant>, now: Instant) -> Duration {
    last_output_at.map_or(Duration::MAX, |last_output_at| {
        now.saturating_duration_since(last_output_at)
    })
}

/// Tmux keeps its scrollback in copy-mode on the alternate screen, so grove's own scrollback is empty for it (`session.rs:667-705`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    Tmux { name: String },
    Native,
}

pub struct TerminalSession {
    term: GroveTerm,
    /// `None` only when no PTY could be spawned at all; the grid still renders empty rather than taking the window down.
    pty: Option<PtyHandle>,
    backend: Backend,
    rows: u16,
    cols: u16,
    last_damage_gen: u64,
    /// Whether tmux is parked in copy-mode from a scroll-up (`session.rs:617-622`).
    tmux_copy_mode: bool,
    /// Tmux renders copy-mode on an alternate screen, so GroveTerm's own
    /// display offset remains zero. Keep tmux's exact offset for selection
    /// coordinates and overlays.
    tmux_display_offset: usize,
    last_input_at: Option<Instant>,
    last_scroll_at: Option<Instant>,
    /// `None` until the PTY produces its first output.
    last_output_at: Option<Instant>,
    /// Latched once [`Self::alive`] observes the child reaped: `try_wait` must never be called again after it.
    exited: bool,
    /// `Some` implies `pty.is_none()`.
    spawn_error: Option<String>,
    /// Captured once at spawn — no cheap live handle to the pane's foreground process (`session.rs:511-522`).
    pane_pid: Option<u32>,
    /// The tmux session name a reattach is owed, taken by [`Self::attach_now`]. `Some` implies `pty.is_none()` and no reader.
    pending_attach: Option<String>,
    /// Stored here, not gpui element state — the element id embeds the tile slot index, so moving a tile would invalidate an element-keyed cache.
    scene_cache: Option<(TermSceneKey, Rc<TermScene>)>,
    /// Dropping the `Task` stops the reader, so this field *is* the reader.
    reader: Task<()>,
    _bundle: Option<grove_core::multi_root::SymlinkBundle>,
}

impl TerminalSession {
    /// Spawn at an explicit target: tmux-backed when available, otherwise a plain PTY (`session.rs:222-279`).
    pub fn spawn(
        target: &SpawnTarget,
        extra_args: &[String],
        state_file: Option<&Path>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Born at the live body dims so the first paint's resize is a no-op instead of a shrink-then-regrow storm.
        let dims = *cx.global::<crate::zoom::CurrentPtyDims>();
        let (rows, cols) = (dims.rows, dims.cols);
        let cwd = target.cwd.clone();
        let bundle = target
            .temp_bundle_path
            .as_ref()
            .and_then(|path| grove_core::multi_root::SymlinkBundle::from_path(path.into()));
        let mut spawn_error = None;
        let spawned = if target.use_tmux {
            match spawn_tmux(&cwd, target, extra_args, state_file, rows, cols) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(
                        "grove-gpui: tmux unavailable ({e}); falling back to a native PTY"
                    );
                    match spawn_native(&cwd, target, extra_args, state_file, rows, cols) {
                        Ok(v) => Some(v),
                        Err(e) => {
                            tracing::error!("grove-gpui: could not spawn a PTY: {e}");
                            spawn_error = Some(e.clone());
                            None
                        }
                    }
                }
            }
        } else {
            match spawn_native(&cwd, target, extra_args, state_file, rows, cols) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::error!("grove-gpui: could not spawn a PTY: {e}");
                    spawn_error = Some(e.clone());
                    None
                }
            }
        };
        let (mut pty, backend) = match spawned {
            Some((pty, backend)) => (Some(pty), backend),
            None => (None, Backend::Native),
        };
        let rx = pty.as_mut().and_then(PtyHandle::take_receiver);
        let pane_pid = match &backend {
            Backend::Tmux { name } => tmux::pane_pid(name),
            Backend::Native => None,
        };
        Self {
            term: GroveTerm::new(rows, cols),
            pty,
            backend,
            rows,
            cols,
            last_damage_gen: 0,
            tmux_copy_mode: false,
            tmux_display_offset: 0,
            last_input_at: None,
            last_scroll_at: None,
            last_output_at: None,
            exited: false,
            spawn_error,
            pane_pid,
            pending_attach: None,
            scene_cache: None,
            reader: Self::spawnreader(rx, cx),
            _bundle: bundle,
        }
    }

    /// Re-attach to a tmux session a previous grove run created (`crates/grove-core/src/session.rs:327-347`).
    ///
    /// The actual attach is deferred to [`Self::attach_now`] — otherwise the wrong dims force a second SIGWINCH.
    pub fn attach_existing(name: &str, rows: u16, cols: u16, cx: &mut Context<Self>) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        Self {
            term: GroveTerm::new(rows, cols),
            pty: None,
            backend: Backend::Tmux {
                name: name.to_string(),
            },
            rows,
            cols,
            last_damage_gen: 0,
            tmux_copy_mode: false,
            tmux_display_offset: 0,
            last_input_at: None,
            last_scroll_at: None,
            last_output_at: None,
            exited: false,
            spawn_error: None,
            pane_pid: None,
            pending_attach: Some(name.to_string()),
            scene_cache: None,
            reader: Self::spawnreader(None, cx),
            _bundle: None,
        }
    }

    /// Performs the deferred tmux attach at the caller's real dims; idempotent since the name is taken out of `pending_attach`.
    pub fn attach_now(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.pending_attach.take() else {
            return;
        };
        if !tmux::has_session(&name) {
            self.spawn_error = Some(format!("tmux session {name} is gone"));
            return;
        }
        tmux::configure_embedded_session(&name);
        let mut pty = match grove_terminal::pty::spawn(tmux_attach_cmd(&name), self.rows, self.cols)
        {
            Ok(p) => p,
            Err(e) => {
                self.spawn_error = Some(e.to_string());
                return;
            }
        };
        let rx = pty.take_receiver();
        self.pty = Some(pty);
        self.reader = Self::spawnreader(rx, cx);
        self.pane_pid = tmux::pane_pid(&name);
    }

    pub fn is_pending_attach(&self) -> bool {
        self.pending_attach.is_some()
    }

    /// Always native — a teardown script must die with the modal, never a tmux sidecar (`session.rs:288-321`).
    pub fn spawn_script(script: &str, cwd: &str, cx: &mut Context<Self>) -> Self {
        let shell = grove_core::env_path::login_shell();
        let mut cmd = CommandBuilder::new(&shell);
        #[cfg(windows)]
        cmd.arg("-Command");
        #[cfg(not(windows))]
        cmd.arg("-lc");
        cmd.arg(script);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("LC_ALL", "en_US.UTF-8");

        let mut spawn_error = None;
        let mut pty = match grove_terminal::pty::spawn(cmd, INIT_ROWS, INIT_COLS) {
            Ok(pty) => Some(pty),
            Err(e) => {
                tracing::error!(
                    "grove-gpui: could not spawn the lifecycle script (cwd={cwd:?}): {e}"
                );
                spawn_error = Some(e.to_string());
                None
            }
        };
        let rx = pty.as_mut().and_then(PtyHandle::take_receiver);
        Self {
            term: GroveTerm::new(INIT_ROWS, INIT_COLS),
            pty,
            backend: Backend::Native,
            rows: INIT_ROWS,
            cols: INIT_COLS,
            last_damage_gen: 0,
            tmux_copy_mode: false,
            tmux_display_offset: 0,
            last_input_at: None,
            last_scroll_at: None,
            last_output_at: None,
            exited: false,
            spawn_error,
            pane_pid: None,
            pending_attach: None,
            scene_cache: None,
            reader: Self::spawnreader(rx, cx),
            _bundle: None,
        }
    }

    /// A plain `std::thread`, not `background_executor().spawn` — the loop blocks for the session's whole life.
    fn spawnreader(rx: Option<Receiver<Vec<u8>>>, cx: &mut Context<Self>) -> Task<()> {
        let Some(rx) = rx else {
            return Task::ready(());
        };
        let (tx, mut chunks) = mpsc::unbounded::<Vec<u8>>();
        std::thread::Builder::new()
            .name("grove-gpui-pty-bridge".into())
            .spawn(move || {
                while let Ok(chunk) = rx.recv() {
                    if tx.unbounded_send(chunk).is_err() {
                        break;
                    }
                }
            })
            .map_or_else(
                |e| {
                    tracing::error!("grove-gpui: PTY bridge thread: {e}");
                    Task::ready(())
                },
                |_| {
                    cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
                        while let Some(chunk) = chunks.next().await {
                            // Coalesce everything already queued: a burst of output costs one repaint, not one per chunk.
                            let mut batch = vec![chunk];
                            while let Ok(more) = chunks.try_recv() {
                                batch.push(more);
                            }
                            if this
                                .update(cx, |this: &mut Self, cx| this.ingest(&batch, cx))
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                },
            )
    }

    /// Feeds chunks into the model and repaints only if the grid actually moved (damage-generation compare, not a redraw-every-chunk).
    fn ingest(&mut self, chunks: &[Vec<u8>], cx: &mut Context<Self>) {
        self.last_output_at = Some(Instant::now());
        for chunk in chunks {
            self.term.process(chunk);
        }
        // Protocol replies must go straight to the PTY, never through `send` (which snaps to live and cancels copy-mode).
        let replies = self.term.take_responses();
        if !replies.is_empty() {
            if let Some(pty) = self.pty.as_mut() {
                if let Err(e) = pty.write(&replies) {
                    tracing::debug!("grove-gpui: PTY reply write failed: {e}");
                }
            }
        }
        let generation = self.term.damage_generation();
        if generation != self.last_damage_gen {
            self.last_damage_gen = generation;
            cx.notify();
        }
    }

    /// Order is load-bearing: snap to live and leave copy-mode before the bytes go out (`session.rs:604-625`).
    pub fn send(&mut self, bytes: &[u8]) {
        self.last_input_at = Some(Instant::now());
        self.term.scroll_to(0);
        self.tmux_display_offset = 0;
        if self.tmux_copy_mode {
            if let Backend::Tmux { name } = &self.backend {
                tmux::cancel_copy_mode(name);
            }
            self.tmux_copy_mode = false;
        }
        if let Some(pty) = self.pty.as_mut() {
            if let Err(e) = pty.write(bytes) {
                tracing::debug!("grove-gpui: PTY write failed: {e}");
            }
        }
    }

    /// Both resizes are required: `GroveTerm` reflows the model, `PtyHandle::resize` sends the SIGWINCH (`session.rs:940-967`).
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        // Discount the resize-triggered repaint so the attention classifier doesn't read it as agent output.
        self.last_input_at = Some(Instant::now());
        self.rows = rows;
        self.cols = cols;
        self.term.scroll_to(0);
        self.term.resize(rows, cols);
        if let Some(pty) = self.pty.as_ref() {
            if let Err(e) = pty.resize(rows, cols) {
                tracing::debug!("grove-gpui: PTY resize failed: {e}");
            }
        }
    }

    /// Port of `session.rs:641-661`: with no mouse reporting the view scrolls, otherwise the notch is forwarded to the inner app.
    pub fn scroll(&mut self, up: bool, col: u16, row: u16) {
        self.last_scroll_at = Some(Instant::now());
        if self.term.mouse_mode() == MouseMode::None {
            self.scroll_view(up, mouse::SCROLL_STEP);
            return;
        }
        let (rows, cols) = self.term.size();
        // `mouse::cell_at` has no upper clamp, so clamp here to never encode a coordinate past the live grid.
        let col = col.min(cols.saturating_sub(1));
        let row = row.min(rows.saturating_sub(1));
        self.send_wheel_notch(up, col, row);
    }

    /// Goes through the notch flood cap — without it Shift+Home's full-scrollback jump would hang the PTY.
    pub fn scroll_lines(&mut self, up: bool, lines: usize) {
        self.last_scroll_at = Some(Instant::now());
        if self.term.mouse_mode() == MouseMode::None {
            self.scroll_view(up, lines);
            return;
        }
        let (col, row) = (self.cols / 2, self.rows / 2);
        for _ in 0..mouse::scroll_notch_count(lines) {
            self.send_wheel_notch(up, col, row);
        }
    }

    pub fn scroll_page_lines(&self) -> usize {
        mouse::scroll_page_lines(self.rows)
    }

    fn send_wheel_notch(&mut self, up: bool, col: u16, row: u16) {
        // Wheel up is cb 64, wheel down cb 65 (`session.rs:666-669`).
        let cb: u32 = if up { 64 } else { 65 };
        let bytes = mouse::encode_mouse(self.term.encoding(), cb, col, row, true);
        self.send(&bytes);
    }

    /// Scroll the terminal's own view, ignoring the inner app's mouse mode (`session.rs:673-708`).
    fn scroll_view(&mut self, up: bool, lines: usize) {
        match &self.backend {
            Backend::Tmux { name } => {
                // Grove's own scrollback is empty for tmux; drive copy-mode instead.
                let name = name.clone();
                if let Some(offset) = tmux::scroll(&name, up, lines) {
                    self.tmux_display_offset = offset;
                }
                if up {
                    self.tmux_copy_mode = true;
                } else if self.tmux_display_offset == 0 {
                    self.tmux_copy_mode = false;
                }
            }
            Backend::Native => {
                let cur = self.term.display_offset();
                let target = if up {
                    (cur + lines).min(mouse::SCROLLBACK_LINES)
                } else {
                    cur.saturating_sub(lines)
                };
                self.term.scroll_to(target);
            }
        }
    }

    /// No-op while scrolled back, while the caret is hidden, or across rows (`session.rs:758-794`).
    pub fn click(&mut self, col: u16, row: u16) {
        if self.term.display_offset() != 0 {
            return;
        }
        let (rows, cols) = self.term.size();
        let col = col.min(cols.saturating_sub(1));
        let row = row.min(rows.saturating_sub(1));

        if self.term.mouse_mode() != MouseMode::None {
            let encoding = self.term.encoding();
            let press = mouse::encode_mouse(encoding, 0, col, row, true);
            let release = mouse::encode_mouse(encoding, 0, col, row, false);
            self.send(&press);
            self.send(&release);
            return;
        }

        let (cur_row, cur_col, hidden) = self.term.cursor();
        if hidden || row != cur_row {
            return;
        }
        let bytes = keys::arrow_moves(cur_col, col, self.term.app_cursor());
        if !bytes.is_empty() {
            self.send(&bytes);
        }
    }

    /// Native selections use GroveTerm's oracle-backed history walk. Tmux
    /// selections read tmux's own history because its alternate-screen redraw
    /// only leaves the current viewport in GroveTerm.
    pub fn selection_text(&mut self, a: AbsCell, head: AbsCell) -> Option<String> {
        match &self.backend {
            Backend::Tmux { name } => tmux::selection_text(
                name,
                (a.a_row, a.col),
                (head.a_row, head.col),
                self.tmux_display_offset,
            ),
            Backend::Native => self
                .term
                .selection_text((a.a_row, a.col), (head.a_row, head.col)),
        }
    }

    pub fn dims(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.term.snapshot()
    }

    pub fn cursor(&self) -> (u16, u16, bool) {
        self.term.cursor()
    }

    pub fn display_offset(&self) -> usize {
        match self.backend {
            Backend::Tmux { .. } => self.tmux_display_offset,
            Backend::Native => self.term.display_offset(),
        }
    }

    pub fn damage_generation(&self) -> u64 {
        self.term.damage_generation()
    }

    pub fn scene_cache(&self) -> Option<&(TermSceneKey, Rc<TermScene>)> {
        self.scene_cache.as_ref()
    }

    pub fn set_scene_cache(&mut self, key: TermSceneKey, scene: Rc<TermScene>) {
        self.scene_cache = Some((key, scene));
    }

    pub fn app_cursor(&self) -> bool {
        self.term.app_cursor()
    }

    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    pub fn input_age(&self) -> Option<Duration> {
        self.last_input_at.map(|t| t.elapsed())
    }

    pub fn scroll_age(&self) -> Option<Duration> {
        self.last_scroll_at.map(|t| t.elapsed())
    }

    pub fn title(&self) -> Option<String> {
        self.term.title()
    }

    /// Cumulative BEL count (`term.rs:231`); the classifier diffs against what it has consumed.
    pub fn bell_count(&self) -> usize {
        self.term.bell_count()
    }

    /// `&mut self` because the model's own accessor needs it (`term.rs:299`).
    pub fn tail_contents(&mut self, n: usize) -> String {
        self.term.tail_contents(n)
    }

    pub fn output_age(&self) -> Duration {
        output_age_at(self.last_output_at, Instant::now())
    }

    /// `Some` means no PTY at all; toast producers read this since spawn always returns a session, never a `Result`.
    pub fn spawn_error(&self) -> Option<&str> {
        self.spawn_error.as_deref()
    }

    pub fn alive(&mut self) -> bool {
        if self.exited {
            return false;
        }
        let Some(pty) = self.pty.as_mut() else {
            return false;
        };
        // A reaped child cannot come back: latch and never ask again.
        if pty.try_wait().unwrap_or(false) {
            self.exited = true;
            return false;
        }
        true
    }

    /// `exited` gates this because `portable_pty` keeps reporting a reaped child's id.
    pub fn root_pid(&self) -> Option<u32> {
        match &self.backend {
            Backend::Native if self.exited => None,
            Backend::Native => self.pty.as_ref().and_then(PtyHandle::child_pid),
            Backend::Tmux { .. } => self.pane_pid,
        }
    }

    /// Deliberately unlike a manual `mod+j/k` switch (`sessions.rs:210-218`).
    pub fn snap_to_bottom(&mut self) {
        self.term.scroll_to(0);
    }
}

/// Create the persistent tmux session and attach an embedded client (`session.rs:177-236` create+sidecar, `:349-384` attach).
fn spawn_tmux(
    cwd: &str,
    target: &SpawnTarget,
    extra_args: &[String],
    state_file: Option<&Path>,
    rows: u16,
    cols: u16,
) -> Result<(PtyHandle, Backend), String> {
    if !tmux::available() {
        return Err("tmux not on PATH".to_string());
    }
    let agent = target.agent;
    let n = tmux::next_free_n(cwd, agent);
    let name = tmux::make_name(cwd, agent, n);
    let env: Vec<(String, String)> = state_file
        .map(|p| {
            vec![(
                grove_core::attention::STATE_FILE_ENV.to_string(),
                p.display().to_string(),
            )]
        })
        .unwrap_or_default();
    let mut env = env;
    if let Some(path) = &target.temp_bundle_path {
        env.push(("GROVE_MULTI_ROOT".into(), path.clone()));
    }
    let agent_args = target
        .args
        .iter()
        .cloned()
        .chain(extra_args.iter().cloned())
        .collect::<Vec<_>>();
    let (program, invocation_args) = agent.session_invocation(&agent_args);
    tmux::new_session(&name, cwd, rows, cols, &program, &invocation_args, &env)
        .map_err(|e| e.to_string())?;
    // Without the sidecar the session can't be rediscovered after a restart; kill rather than orphan it (`session.rs:213-227`).
    if let Err(e) = session_meta::write(
        &name,
        &SessionMeta {
            wt_path: cwd.to_string(),
            project: target.project.clone(),
            label: target.label.clone(),
            agent,
            context_roots: target.context_roots.clone(),
            temp_bundle_path: target.temp_bundle_path.clone(),
        },
    ) {
        tmux::kill_session(&name);
        return Err(format!("session metadata: {e}"));
    }
    tmux::configure_embedded_session(&name);

    match grove_terminal::pty::spawn(tmux_attach_cmd(&name), rows, cols) {
        Ok(pty) => Ok((pty, Backend::Tmux { name })),
        Err(e) => {
            tmux::kill_session(&name);
            Err(e.to_string())
        }
    }
}

/// `-u` forces UTF-8 output; without it tmux downgrades box-drawing to ACS/DEC escapes (`session.rs:362-367`).
fn tmux_attach_cmd(name: &str) -> CommandBuilder {
    let mut cmd = CommandBuilder::new("tmux");
    cmd.arg("-L");
    cmd.arg(tmux::SOCKET);
    cmd.arg("-u");
    cmd.arg("attach-session");
    cmd.arg("-t");
    cmd.arg(format!("={name}"));
    cmd.env("TERM", "xterm-256color");
    cmd.env("LC_ALL", "en_US.UTF-8");
    cmd
}

/// Full port of `Session::spawn_native` (`crates/grove-core/src/session.rs:238-279`); grove's no-tmux path runs the agent, not a bare login shell.
fn spawn_native(
    cwd: &str,
    target: &SpawnTarget,
    extra_args: &[String],
    state_file: Option<&Path>,
    rows: u16,
    cols: u16,
) -> Result<(PtyHandle, Backend), String> {
    let agent_args = target
        .args
        .iter()
        .cloned()
        .chain(extra_args.iter().cloned())
        .collect::<Vec<_>>();
    let (program, prefix_args) = target.agent.session_invocation(&agent_args);
    let mut cmd = CommandBuilder::new(program);
    for a in prefix_args {
        cmd.arg(a);
    }
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    cmd.env("LC_ALL", "en_US.UTF-8");
    if let Some(path) = &target.temp_bundle_path {
        cmd.env("GROVE_MULTI_ROOT", path);
    }
    if let Some(path) = state_file {
        cmd.env(
            grove_core::attention::STATE_FILE_ENV,
            path.display().to_string(),
        );
    }
    grove_terminal::pty::spawn(cmd, rows, cols)
        .map(|pty| (pty, Backend::Native))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use grove_core::agent::Agent;

    use super::output_age_at;

    #[test]
    fn output_is_stale_until_the_pty_produces_bytes() {
        let now = Instant::now();
        assert_eq!(output_age_at(None, now), Duration::MAX);
        assert_eq!(
            output_age_at(Some(now - Duration::from_secs(2)), now),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn a_terminal_target_still_invokes_the_login_shell_with_no_flags() {
        let (program, prefix) = Agent::Terminal.invocation();
        assert_eq!(program, grove_core::env_path::login_shell());
        assert!(prefix.is_empty());
        assert!(Agent::Terminal.launch_args(true, true).is_empty());
    }

    #[test]
    fn the_permission_and_chrome_settings_produce_claudes_flags() {
        assert!(Agent::Claude.launch_args(false, false).is_empty());
        assert_eq!(
            Agent::Claude.launch_args(true, true),
            vec![
                "--dangerously-skip-permissions".to_string(),
                "--chrome".to_string()
            ]
        );
    }
}
