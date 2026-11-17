//! One live terminal: a PTY plus the `grove_terminal::GroveTerm` model that parses it.

use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc::Receiver, Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures::channel::mpsc;
use futures::StreamExt as _;
use gpui::{AppContext as _, Context, Task};
use grove_core::session_meta::{self, SessionMeta};
use grove_core::tmux;
use grove_terminal::{GroveTerm, MouseMode, PtyHandle, Snapshot};

use crate::entities::session_registry::SpawnTarget;
use crate::terminal::keys;
use crate::terminal::mouse::{self, AbsCell};
use crate::terminal_element::{GeomKey, TermScene, TermSceneKey};
use portable_pty::CommandBuilder;

/// Initial PTY size before the element's first `prepaint` reports real bounds (`crates/grove-core/src/session.rs:53-54`).
const INIT_ROWS: u16 = 24;
const INIT_COLS: u16 = 80;

fn tmux_side_effects_enabled() -> bool {
    !cfg!(test)
}

fn managed_worktree_terminal_sidecar(target: &SpawnTarget) -> bool {
    target.use_tmux
        && target.agent == grove_core::agent::Agent::Terminal
        && !target.project.is_empty()
}

fn output_age_at(last_output_at: Option<Instant>, now: Instant) -> Duration {
    last_output_at.map_or(Duration::MAX, |last_output_at| {
        now.saturating_duration_since(last_output_at)
    })
}

fn output_needs_notify(
    last_damage_gen: u64,
    damage_gen: u64,
    previous_title: Option<&str>,
    title: Option<&str>,
) -> bool {
    damage_gen != last_damage_gen || title != previous_title
}

/// Tmux keeps its scrollback in copy-mode on the alternate screen, so grove's own scrollback is empty for it (`session.rs:667-705`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    Tmux { name: String },
    Native,
}

struct PendingPty {
    result: Option<(PtyHandle, Backend, Option<u32>)>,
    cleanup_tmux: bool,
}

impl PendingPty {
    fn dispose(mut self) -> Result<(), String> {
        self.result.take().map_or(Ok(()), |(pty, backend, _)| {
            Self::cleanup(pty, backend, self.cleanup_tmux)
        })
    }

    fn cleanup(pty: PtyHandle, backend: Backend, cleanup_tmux: bool) -> Result<(), String> {
        // Stop the persistent session first, then wait for its embedded client.

        if cleanup_tmux {
            if let Backend::Tmux { name } = backend {
                tmux::with_lifecycle_lock(|| {
                    tmux::kill_session(&name);
                    session_meta::delete(&name);
                });
            }
        }
        pty.terminate_and_wait().map_err(|error| error.to_string())
    }
}
impl Drop for PendingPty {
    fn drop(&mut self) {
        let Some((pty, backend, _)) = self.result.take() else {
            return;
        };
        let cleanup_tmux = self.cleanup_tmux;
        // Abandoned application/launch ownership still cleans up off the foreground.
        let _ = std::thread::Builder::new()
            .name("grove-cancelled-pty".into())
            .spawn(move || Self::cleanup(pty, backend, cleanup_tmux));
    }
}

pub struct TerminalSession {
    term: Arc<Mutex<GroveTerm>>,
    reported_cwd: Option<String>,
    last_title: Option<String>,
    /// Launch directory only; the shell may change directory after startup.
    initial_cwd: Option<String>,
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
    exit_code: Option<u32>,
    script_session: bool,
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
    exit_watch: Option<Task<()>>,
    startup: Option<Task<()>>,
    starting: bool,
    startup_cancel: Arc<AtomicBool>,
    pending_input: Vec<u8>,
    scroll_work: Option<Task<()>>,
    scroll_delta: i64,
    bundle: Option<grove_core::multi_root::SymlinkBundle>,
}

impl TerminalSession {
    /// Spawn at an explicit target: tmux-backed when available, otherwise a plain PTY (`session.rs:222-279`).
    pub fn spawn(
        target: &SpawnTarget,
        extra_args: &[String],
        state_file: Option<&Path>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::spawn_checked(target, extra_args, state_file, Box::new(|| Ok(())), cx)
    }

    /// Resolve authoritative launch safety on the PTY worker before any process or sidecar exists.
    pub fn spawn_checked(
        target: &SpawnTarget,
        extra_args: &[String],
        state_file: Option<&Path>,
        validation: Box<dyn FnOnce() -> Result<(), String> + Send>,
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
        let mut this = Self::pending(rows, cols, Backend::Native, false);
        this.initial_cwd = Some(cwd);
        this.bundle = bundle;
        let target = target.clone();
        let extra_args = extra_args.to_vec();
        let state_file = state_file.map(Path::to_path_buf);
        let cancel = this.startup_cancel.clone();
        this.startup = Some(Self::start_pty(
            move || {
                grove_core::env_path::wait_for_initial_environment();
                check_startup_cancel(&cancel)?;
                validation()?;
                let result = if target.use_tmux && tmux_side_effects_enabled() {
                    spawn_tmux(
                        &target.cwd,
                        &target,
                        &extra_args,
                        state_file.as_deref(),
                        rows,
                        cols,
                        &cancel,
                    )
                    .or_else(|error| {
                        tracing::warn!("tmux unavailable ({error}); falling back to native PTY");
                        spawn_native(
                            &target.cwd,
                            &target,
                            &extra_args,
                            state_file.as_deref(),
                            rows,
                            cols,
                            &cancel,
                        )
                    })
                } else {
                    spawn_native(
                        &target.cwd,
                        &target,
                        &extra_args,
                        state_file.as_deref(),
                        rows,
                        cols,
                        &cancel,
                    )
                };
                result.map(|(pty, backend)| {
                    let pid = match &backend {
                        Backend::Tmux { name } => tmux::pane_pid(name),
                        Backend::Native => None,
                    };
                    (pty, backend, pid)
                })
            },
            true,
            cx,
        ));
        this
    }

    fn pending(rows: u16, cols: u16, backend: Backend, script_session: bool) -> Self {
        Self {
            term: Arc::new(Mutex::new(GroveTerm::new(rows, cols))),
            reported_cwd: None,
            last_title: None,
            initial_cwd: None,
            pty: None,
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
            exit_code: None,
            script_session,
            spawn_error: None,
            pane_pid: None,
            pending_attach: None,
            scene_cache: None,
            reader: Task::ready(()),
            exit_watch: None,
            startup: None,
            starting: true,
            startup_cancel: Arc::new(AtomicBool::new(false)),
            pending_input: Vec::new(),
            scroll_work: None,
            scroll_delta: 0,
            bundle: None,
        }
    }

    fn start_pty(
        spawn: impl FnOnce() -> Result<(PtyHandle, Backend, Option<u32>), String> + Send + 'static,
        cleanup_tmux: bool,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        #[cfg(test)]
        {
            Self::start_pty_inner(spawn, cleanup_tmux, None, cx)
        }
        #[cfg(not(test))]
        {
            Self::start_pty_inner(spawn, cleanup_tmux, cx)
        }
    }

    fn start_pty_inner(
        spawn: impl FnOnce() -> Result<(PtyHandle, Backend, Option<u32>), String> + Send + 'static,
        cleanup_tmux: bool,
        #[cfg(test)] gate: Option<futures::channel::oneshot::Receiver<()>>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            let result = cx
                .background_spawn(async move {
                    spawn().map(|result| PendingPty {
                        result: Some(result),
                        cleanup_tmux,
                    })
                })
                .await;
            #[cfg(test)]
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            let _ = this.update(cx, |this, cx| {
                this.starting = false;
                this.startup = None;
                match result {
                    Ok(mut pending)
                        if !this.exited
                            && (!this.startup_cancel.load(Ordering::Acquire)
                                || this.script_session) =>
                    {
                        let Some((mut pty, backend, pid)) = pending.result.take() else {
                            return;
                        };
                        let _ = pty.resize(this.rows, this.cols);
                        let rx = pty.take_receiver();
                        this.pty = Some(pty);
                        this.backend = backend;
                        this.pane_pid = pid;
                        this.reader = Self::spawnreader(rx, this.term.clone(), cx);
                        let input = std::mem::take(&mut this.pending_input);
                        if this.startup_cancel.load(Ordering::Acquire) && this.script_session {
                            if let Err(error) = this.request_script_stop(cx) {
                                this.spawn_error = Some(error);
                            }
                        } else if !input.is_empty() {
                            this.send(&input);
                        }
                    }
                    Ok(pending) => {
                        // Keep startup pending until the unadopted process and any new
                        // tmux session are cleaned up. Destructive removal awaits this.
                        this.starting = true;
                        this.startup = Some(cx.spawn(async move |this, cx| {
                            let result =
                                cx.background_spawn(async move { pending.dispose() }).await;
                            let _ = this.update(cx, |this, cx| {
                                if let Err(error) = result {
                                    this.spawn_error =
                                        Some(format!("Terminal cleanup failed: {error}"));
                                }
                                this.starting = false;
                                this.startup = None;
                                this.reader_closed(cx);
                                cx.notify();
                            });
                        }));
                    }
                    Err(error) => {
                        if this.startup_cancel.load(Ordering::Acquire) {
                            this.reader_closed(cx);
                        } else {
                            this.spawn_error = Some(error);
                            this.exited = true;
                        }
                    }
                }
                cx.notify();
            });
        })
    }

    /// Re-attach to a tmux session a previous grove run created (`crates/grove-core/src/session.rs:327-347`).
    ///
    /// The actual attach is deferred to [`Self::attach_now`] — otherwise the wrong dims force a second SIGWINCH.
    pub fn attach_existing(name: &str, rows: u16, cols: u16, cx: &mut Context<Self>) -> Self {
        let _ = cx;
        let mut this = Self::pending(
            rows.max(1),
            cols.max(1),
            Backend::Tmux { name: name.into() },
            false,
        );
        this.pending_attach = Some(name.into());
        this.starting = false;
        this
    }

    /// Performs the deferred tmux attach at the caller's real dims; idempotent since the name is taken out of `pending_attach`.
    pub fn attach_now(&mut self, cx: &mut Context<Self>) {
        if !tmux_side_effects_enabled() {
            return;
        }
        let Some(name) = self.pending_attach.take() else {
            return;
        };
        self.starting = true;
        let (rows, cols) = (self.rows, self.cols);
        let cancel = self.startup_cancel.clone();
        self.startup = Some(Self::start_pty(
            move || {
                grove_core::env_path::wait_for_initial_environment();
                if !tmux::has_session(&name) {
                    return Err(format!("tmux session {name} is gone"));
                }
                tmux::configure_embedded_session(&name);
                check_startup_cancel(&cancel)?;
                let pty = grove_terminal::pty::spawn(tmux_attach_cmd(&name), rows, cols)
                    .map_err(|e| e.to_string())?;
                let pid = tmux::pane_pid(&name);
                Ok((pty, Backend::Tmux { name }, pid))
            },
            false,
            cx,
        ));
    }

    /// Only after startup/cancellation acknowledgment; the caller owns off-thread teardown.
    pub(crate) fn take_for_removal(&mut self) -> (Option<PtyHandle>, Backend) {
        self.startup_cancel.store(true, Ordering::Release);
        self.pending_attach = None;
        self.pending_input.clear();
        self.reader = Task::ready(());
        self.exit_watch = None;
        (self.pty.take(), self.backend.clone())
    }

    pub(crate) fn startup_cancellation(&self) -> Arc<AtomicBool> {
        self.startup_cancel.clone()
    }

    #[cfg(test)]
    pub(crate) fn pending_startup_for_test(
        gate: futures::channel::oneshot::Receiver<()>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut terminal = Self::pending(24, 80, Backend::Native, false);
        terminal.startup = Some(Self::start_pty_inner(
            || Err("Fixture startup does not launch a process".into()),
            false,
            Some(gate),
            cx,
        ));
        terminal
    }

    pub fn is_starting(&self) -> bool {
        self.starting
    }

    pub fn is_pending_attach(&self) -> bool {
        self.pending_attach.is_some()
    }

    /// Always native — a teardown script must die with the modal, never a tmux sidecar (`session.rs:288-321`).
    pub fn spawn_script(script: &str, cwd: &str, cx: &mut Context<Self>) -> Self {
        Self::spawn_script_checked(script, cwd, Box::new(|| Ok(())), cx)
    }

    pub fn spawn_script_checked(
        script: &str,
        cwd: &str,
        validation: Box<dyn FnOnce() -> Result<(), String> + Send>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::pending(INIT_ROWS, INIT_COLS, Backend::Native, true);
        this.initial_cwd = Some(cwd.into());
        let (script, cwd) = (script.to_owned(), cwd.to_owned());
        let cancel = this.startup_cancel.clone();
        this.startup = Some(Self::start_pty(
            move || {
                grove_core::env_path::wait_for_initial_environment();
                check_startup_cancel(&cancel)?;
                validation()?;
                let shell = grove_core::env_path::login_shell();
                let mut cmd = CommandBuilder::new(&shell);
                let shell_name = Path::new(&shell)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                cmd.arg(if matches!(shell_name.as_str(), "powershell" | "pwsh") {
                    "-Command"
                } else {
                    "-lc"
                });
                cmd.arg(script);
                apply_session_environment(
                    &mut cmd,
                    grove_core::env_path::session_environment().as_deref(),
                );
                cmd.cwd(cwd);
                cmd.env("TERM", "xterm-256color");
                cmd.env("LC_ALL", "en_US.UTF-8");
                check_startup_cancel(&cancel)?;
                grove_terminal::pty::spawn(cmd, INIT_ROWS, INIT_COLS)
                    .map(|pty| (pty, Backend::Native, None))
                    .map_err(|error| error.to_string())
            },
            false,
            cx,
        ));
        this
    }

    fn model(&self) -> MutexGuard<'_, GroveTerm> {
        self.term
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn spawnreader(
        rx: Option<Receiver<Vec<u8>>>,
        term: Arc<Mutex<GroveTerm>>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let Some(rx) = rx else {
            return Task::ready(());
        };
        if cfg!(test) {
            drop(rx);
            return Task::ready(());
        }
        // Parse on the bridge thread and queue only a bounded damage wake. The
        // grid mutex is held for at most one 64 KiB input chunk at a time.
        let (mut tx, mut wakes) = mpsc::channel::<()>(1);
        std::thread::Builder::new()
            .name("grove-gpui-pty-bridge".into())
            .spawn(move || {
                while let Ok(chunk) = rx.recv() {
                    term.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .process(&chunk);
                    if let Err(error) = tx.try_send(()) {
                        if error.is_disconnected() {
                            break;
                        }
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
                        while wakes.next().await.is_some() {
                            // Coalesce sustained output into at most one foreground
                            // update per display interval, without losing final EOF.
                            cx.background_executor()
                                .timer(Duration::from_millis(16))
                                .await;
                            while wakes.try_recv().is_ok() {}
                            if this.update(cx, Self::publish_output).is_err() {
                                break;
                            }
                        }
                        let _ = this.update(cx, Self::reader_closed);
                    })
                },
            )
    }

    fn publish_output(&mut self, cx: &mut Context<Self>) {
        self.last_output_at = Some(Instant::now());
        let (replies, generation, title, cwd) = {
            let mut term = self.model();
            (
                term.take_responses(),
                term.damage_generation(),
                term.title(),
                term.current_cwd().map(str::to_owned),
            )
        };
        if !replies.is_empty() {
            if let Some(pty) = self.pty.as_mut() {
                if let Err(e) = pty.write(&replies) {
                    tracing::debug!("grove-gpui: PTY reply write failed: {e}");
                }
            }
        }
        self.reported_cwd = cwd;
        if output_needs_notify(
            self.last_damage_gen,
            generation,
            self.last_title.as_deref(),
            title.as_deref(),
        ) {
            self.last_damage_gen = generation;
            self.last_title = title;
            cx.notify();
        }
    }

    #[cfg(test)]
    fn ingest(&mut self, chunks: &[Vec<u8>], cx: &mut Context<Self>) {
        for chunk in chunks {
            self.model().process(chunk);
        }
        self.publish_output(cx);
    }

    #[cfg(test)]
    pub(crate) fn native_for_test(cx: &mut Context<Self>) -> Self {
        let mut session = Self::attach_existing("inert-test-session", 24, 80, cx);
        session.backend = Backend::Native;
        session.pending_attach = None;
        session
    }

    #[cfg(test)]
    pub(crate) fn failed_for_test(cx: &mut Context<Self>) -> Self {
        let mut session = Self::native_for_test(cx);
        session.spawn_error = Some("Isolated terminal input fixture has no PTY".into());
        session
    }

    #[cfg(test)]
    pub(crate) fn last_input_for_test(&self) -> Option<Instant> {
        self.last_input_at
    }

    #[cfg(test)]
    pub(crate) fn ingest_for_test(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.ingest(&[bytes.to_vec()], cx);
    }

    /// Order is load-bearing: snap to live and leave copy-mode before the bytes go out (`session.rs:604-625`).
    pub fn send_checked(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.scroll_work.is_some() {
            return Err("Terminal scroll is in progress; no input sent".into());
        }
        self.last_input_at = Some(Instant::now());
        self.model().scroll_to(0);
        self.tmux_display_offset = 0;
        if self.tmux_copy_mode {
            if let Backend::Tmux { name } = &self.backend {
                if tmux_side_effects_enabled() {
                    tmux::cancel_copy_mode(name);
                }
            }
            self.tmux_copy_mode = false;
        }
        let pty = self
            .pty
            .as_mut()
            .ok_or("Session terminal is not attached; no input sent")?;
        pty.write(bytes).map_err(|error| {
            format!("Request outcome is uncertain; terminal input write failed: {error}")
        })
    }

    pub fn send(&mut self, bytes: &[u8]) {
        if self.starting || self.scroll_work.is_some() {
            self.scroll_delta = 0;
            self.last_input_at = Some(Instant::now());
            self.pending_input.extend_from_slice(bytes);
            return;
        }
        if let Err(error) = self.send_checked(bytes) {
            tracing::debug!("grove-gpui: {error}");
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
        self.model().scroll_to(0);
        self.model().resize(rows, cols);
        if let Some(pty) = self.pty.as_ref() {
            if let Err(e) = pty.resize(rows, cols) {
                tracing::debug!("grove-gpui: PTY resize failed: {e}");
            }
        }
    }

    /// Port of `session.rs:641-661`: with no mouse reporting the view scrolls, otherwise the notch is forwarded to the inner app.
    pub fn scroll(&mut self, up: bool, col: u16, row: u16, cx: &mut Context<Self>) {
        self.last_scroll_at = Some(Instant::now());
        if self.model().mouse_mode() == MouseMode::None {
            self.scroll_view(up, mouse::SCROLL_STEP, cx);
            return;
        }
        let (rows, cols) = self.model().size();
        // `mouse::cell_at` has no upper clamp, so clamp here to never encode a coordinate past the live grid.
        let col = col.min(cols.saturating_sub(1));
        let row = row.min(rows.saturating_sub(1));
        self.send_wheel_notch(up, col, row);
    }

    /// Goes through the notch flood cap — without it Shift+Home's full-scrollback jump would hang the PTY.
    pub fn scroll_lines(&mut self, up: bool, lines: usize, cx: &mut Context<Self>) {
        self.last_scroll_at = Some(Instant::now());
        if self.model().mouse_mode() == MouseMode::None {
            self.scroll_view(up, lines, cx);
            return;
        }
        let (col, row) = (self.cols / 2, self.rows / 2);
        for _ in 0..mouse::scroll_notch_count(lines) {
            self.send_wheel_notch(up, col, row);
        }
    }

    /// Selection drags scroll history even when the inner app owns wheel events.
    pub fn scroll_selection(&mut self, up: bool, lines: usize, cx: &mut Context<Self>) {
        self.last_scroll_at = Some(Instant::now());
        self.scroll_view(up, lines, cx);
    }

    pub fn scroll_page_lines(&self) -> usize {
        mouse::scroll_page_lines(self.rows)
    }

    fn send_wheel_notch(&mut self, up: bool, col: u16, row: u16) {
        // Wheel up is cb 64, wheel down cb 65 (`session.rs:666-669`).
        let cb: u32 = if up { 64 } else { 65 };
        let bytes = mouse::encode_mouse(self.model().encoding(), cb, col, row, true);
        self.send(&bytes);
    }

    /// Scroll the terminal's own view, ignoring the inner app's mouse mode (`session.rs:673-708`).
    fn scroll_view(&mut self, up: bool, lines: usize, cx: &mut Context<Self>) {
        match &self.backend {
            Backend::Tmux { .. } => {
                if tmux_side_effects_enabled() {
                    let delta = lines.min(mouse::SCROLLBACK_LINES) as i64;
                    self.scroll_delta =
                        self.scroll_delta
                            .saturating_add(if up { delta } else { -delta });
                    self.start_scroll(cx);
                } else if up {
                    self.tmux_copy_mode = true;
                }
            }
            Backend::Native => {
                let cur = self.model().display_offset();
                let target = if up {
                    (cur + lines).min(mouse::SCROLLBACK_LINES)
                } else {
                    cur.saturating_sub(lines)
                };
                self.model().scroll_to(target);
            }
        }
    }

    pub fn is_scroll_pending(&self) -> bool {
        self.scroll_work.is_some()
    }

    fn start_scroll(&mut self, cx: &mut Context<Self>) {
        if self.scroll_work.is_some() || self.scroll_delta == 0 {
            return;
        }
        let Backend::Tmux { name } = &self.backend else {
            return;
        };
        let name = name.clone();
        self.scroll_work = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            let delta = this
                .update(cx, |this, _| std::mem::take(&mut this.scroll_delta))
                .unwrap_or(0);
            let offset = if delta == 0 {
                None
            } else {
                cx.background_spawn(async move {
                    tmux::scroll(&name, delta > 0, delta.unsigned_abs() as usize)
                })
                .await
            };
            let _ = this.update(cx, |this, cx| {
                this.scroll_work = None;
                if let Some(offset) = offset {
                    this.tmux_display_offset = offset;
                    this.tmux_copy_mode = offset > 0;
                }
                let input = std::mem::take(&mut this.pending_input);
                if !input.is_empty() {
                    this.scroll_delta = 0;
                    this.send(&input);
                }
                this.start_scroll(cx);
                cx.notify();
            });
        }));
    }

    /// No-op while scrolled back, while the caret is hidden, or across rows.
    pub fn click(&mut self, col: u16, row: u16) {
        if self.model().display_offset() != 0 {
            return;
        }
        let (rows, cols) = self.model().size();
        let col = col.min(cols.saturating_sub(1));
        let row = row.min(rows.saturating_sub(1));

        if self.model().mouse_mode() != MouseMode::None {
            let encoding = self.model().encoding();
            let press = mouse::encode_mouse(encoding, 0, col, row, true);
            let release = mouse::encode_mouse(encoding, 0, col, row, false);
            self.send(&press);
            self.send(&release);
            return;
        }

        let (cur_row, cur_col, hidden) = self.model().cursor();
        if hidden || row != cur_row {
            return;
        }
        let bytes = keys::arrow_moves(cur_col, col, self.model().app_cursor());
        if !bytes.is_empty() {
            self.send(&bytes);
        }
    }

    /// Native selections use GroveTerm's oracle-backed history walk. Tmux
    /// selections read tmux's own history because its alternate-screen redraw
    /// only leaves the current viewport in GroveTerm.
    pub fn selection_text(&mut self, a: AbsCell, head: AbsCell) -> Option<String> {
        match &self.backend {
            Backend::Tmux { name } if tmux_side_effects_enabled() => tmux::selection_text(
                name,
                (a.a_row, a.col),
                (head.a_row, head.col),
                self.tmux_display_offset,
            ),
            Backend::Tmux { .. } => None,
            Backend::Native => self
                .model()
                .selection_text((a.a_row, a.col), (head.a_row, head.col)),
        }
    }

    /// Entire retained native buffer, including rows outside the viewport.
    /// Tmux owns its history separately, so it has no local full-buffer range.
    pub fn selection_all_bounds(&self) -> Option<(AbsCell, AbsCell)> {
        if !matches!(self.backend, Backend::Native) || self.rows == 0 || self.cols == 0 {
            return None;
        }
        Some((
            AbsCell {
                a_row: self.model().history_size() + usize::from(self.rows) - 1,
                col: 0,
            },
            AbsCell {
                a_row: 0,
                col: usize::from(self.cols) - 1,
            },
        ))
    }

    pub fn dims(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// Capture key, cursor and changed cells under one lock. Parsing can advance
    /// concurrently, but a painted frame must never mix two model generations.
    pub fn render_frame(
        &self,
        geom: GeomKey,
    ) -> (TermSceneKey, (u16, u16, bool), Option<Snapshot>) {
        let term = self.model();
        let display_offset = match self.backend {
            Backend::Tmux { .. } => self.tmux_display_offset,
            Backend::Native => term.display_offset(),
        };
        let key = TermSceneKey {
            geom,
            damage_gen: term.damage_generation(),
            display_offset,
        };
        let snapshot = self
            .scene_cache
            .as_ref()
            .is_none_or(|(previous, _)| *previous != key)
            .then(|| term.snapshot());
        (key, term.cursor(), snapshot)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.model().snapshot()
    }

    pub fn display_offset(&self) -> usize {
        match self.backend {
            Backend::Tmux { .. } => self.tmux_display_offset,
            Backend::Native => self.model().display_offset(),
        }
    }

    pub fn release_scene_cache(&mut self) {
        self.scene_cache = None;
    }

    pub fn scene_cache(&self) -> Option<&(TermSceneKey, Rc<TermScene>)> {
        self.scene_cache.as_ref()
    }

    pub fn set_scene_cache(&mut self, key: TermSceneKey, scene: Rc<TermScene>) {
        self.scene_cache = Some((key, scene));
    }

    pub fn app_cursor(&self) -> bool {
        self.model().app_cursor()
    }

    pub fn mouse_reporting(&self) -> bool {
        self.model().mouse_mode() != MouseMode::None
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
        self.model().title()
    }

    /// The directory passed to the spawned process, not its current directory.
    pub fn initial_cwd(&self) -> Option<&str> {
        self.initial_cwd.as_deref()
    }

    /// Last local directory the program reported through OSC 7. Absent until
    /// reported; it can lag if the shell does not emit OSC 7 after `cd`.
    pub fn current_cwd(&self) -> Option<&str> {
        self.reported_cwd.as_deref()
    }

    /// Cumulative BEL count (`term.rs:231`); the classifier diffs against what it has consumed.
    pub fn bell_count(&self) -> usize {
        self.model().bell_count()
    }

    /// `&mut self` because the model's own accessor needs it (`term.rs:299`).
    pub fn tail_contents(&mut self, n: usize) -> String {
        self.model().tail_contents(n)
    }

    pub fn output_age(&self) -> Duration {
        output_age_at(self.last_output_at, Instant::now())
    }

    /// `Some` means no PTY at all; toast producers read this since spawn always returns a session, never a `Result`.
    pub fn spawn_error(&self) -> Option<&str> {
        self.spawn_error.as_deref()
    }

    /// Request termination but retain the PTY until its actual exit is observed.
    pub(crate) fn request_script_stop(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if self.script_session && !self.exited {
            if self.starting {
                self.startup_cancel.store(true, Ordering::Release);
                self.pending_input.clear();
                return Ok(());
            }
            if let Some(pty) = &mut self.pty {
                pty.kill()
                    .map_err(|error| format!("Could not stop script: {error}"))?;
            }
            self.watch_exit(cx);
        }
        Ok(())
    }

    pub(crate) fn stop_script(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.request_script_stop(cx) {
            self.spawn_error = Some(error);
            cx.notify();
        }
    }

    fn watch_exit(&mut self, cx: &mut Context<Self>) {
        if self.exit_watch.is_some() {
            return;
        }
        self.exit_watch = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(25))
                .await;
            let done = this
                .update(cx, |this, cx| {
                    if this.exited {
                        this.exit_watch = None;
                        return true;
                    }
                    match this.pty.as_mut().map(PtyHandle::exit_code).transpose() {
                        Ok(Some(Some(code))) => {
                            this.exit_code = Some(code);
                            this.exit_watch = None;
                            this.reader_closed(cx);
                            true
                        }
                        Ok(None) => {
                            this.exit_watch = None;
                            this.reader_closed(cx);
                            true
                        }
                        Ok(Some(None)) => false,
                        Err(error) => {
                            this.exit_watch = None;
                            this.spawn_error =
                                Some(format!("Could not observe process exit: {error}"));
                            cx.notify();
                            true
                        }
                    }
                })
                .unwrap_or(true);
            if done {
                break;
            }
        }));
    }

    /// Latched PTY exit, available to renderers without polling the child.
    pub fn has_exited(&self) -> bool {
        self.exited
    }

    pub fn exit_code(&self) -> Option<u32> {
        self.exit_code
    }

    fn reader_closed(&mut self, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        self.exit_code = self.exit_code.or_else(|| {
            self.pty
                .as_mut()
                .and_then(|pty| pty.exit_code().ok().flatten())
        });
        if self.pty.is_some() && self.exit_code.is_none() {
            // Closing the PTY stream does not prove that its child has exited.
            self.watch_exit(cx);
            return;
        }
        self.exit_watch = None;
        self.exited = true;
        if self.script_session {
            let outcome = self.exit_code.map_or_else(
                || "\r\n[Terminal closed; process exit status unavailable]\r\n".into(),
                |code| format!("\r\n[Process exited with code {code}]\r\n"),
            );
            self.model().process(outcome.as_bytes());
        }
        cx.notify();
    }

    pub fn alive(&mut self) -> bool {
        if self.starting || self.pending_attach.is_some() {
            return !self.exited;
        }
        if self.exited {
            return false;
        }
        let Some(pty) = self.pty.as_mut() else {
            return false;
        };
        // A reaped child cannot come back: latch and never ask again.
        if let Some(code) = pty.exit_code().ok().flatten() {
            self.exit_code = Some(code);
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
        self.model().scroll_to(0);
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.startup_cancel.store(true, Ordering::Release);
    }
}

fn check_startup_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Terminal launch cancelled".into())
    } else {
        Ok(())
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
    cancel: &AtomicBool,
) -> Result<(PtyHandle, Backend), String> {
    tmux::with_lifecycle_lock(|| {
        spawn_tmux_locked(cwd, target, extra_args, state_file, rows, cols, cancel)
    })
}

fn spawn_tmux_locked(
    cwd: &str,
    target: &SpawnTarget,
    extra_args: &[String],
    state_file: Option<&Path>,
    rows: u16,
    cols: u16,
    cancel: &AtomicBool,
) -> Result<(PtyHandle, Backend), String> {
    check_startup_cancel(cancel)?;
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
    env.push((
        "GROVE_CONFIG_DIR".into(),
        grove_core::storage::config_dir()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned(),
    ));
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
    check_startup_cancel(cancel)?;
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
            managed_worktree_terminal: managed_worktree_terminal_sidecar(target),
            context_roots: target.context_roots.clone(),
            temp_bundle_path: target.temp_bundle_path.clone(),
        },
    ) {
        tmux::kill_session(&name);
        return Err(format!("session metadata: {e}"));
    }
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

/// A complete snapshot removes exports deleted since app startup. Session
/// overrides are applied afterward by every caller.
fn apply_session_environment(cmd: &mut CommandBuilder, env: Option<&[(String, String)]>) {
    if let Some(env) = env {
        cmd.env_clear();
        for (key, value) in env {
            cmd.env(key, value);
        }
    }
}

/// Full port of `Session::spawn_native` (`crates/grove-core/src/session.rs:238-279`); grove's no-tmux path runs the agent, not a bare login shell.
fn spawn_native(
    cwd: &str,
    target: &SpawnTarget,
    extra_args: &[String],
    state_file: Option<&Path>,
    rows: u16,
    cols: u16,
    cancel: &AtomicBool,
) -> Result<(PtyHandle, Backend), String> {
    check_startup_cancel(cancel)?;
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
    apply_session_environment(
        &mut cmd,
        grove_core::env_path::session_environment().as_deref(),
    );
    cmd.env(
        "GROVE_CONFIG_DIR",
        grove_core::storage::config_dir().map_err(|e| e.to_string())?,
    );
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
    check_startup_cancel(cancel)?;
    grove_terminal::pty::spawn(cmd, rows, cols)
        .map(|pty| (pty, Backend::Native))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use gpui::AppContext as _;
    use grove_core::agent::Agent;

    use super::{output_age_at, output_needs_notify};

    #[test]
    fn session_environment_replaces_stale_exports_before_session_overrides() {
        let mut cmd = super::CommandBuilder::new("/bin/sh");
        cmd.env("GROVE_TEST_REMOVED", "old");
        let env = vec![
            ("GROVE_TEST_EXPORT".into(), "new".into()),
            ("TERM".into(), "old-term".into()),
        ];
        super::apply_session_environment(&mut cmd, Some(&env));
        cmd.env("TERM", "xterm-256color");
        cmd.env("GROVE_MULTI_ROOT", "/session/bundle");
        assert!(cmd.get_env("GROVE_TEST_REMOVED").is_none());
        assert_eq!(
            cmd.get_env("GROVE_TEST_EXPORT"),
            Some(std::ffi::OsStr::new("new"))
        );
        assert_eq!(
            cmd.get_env("TERM"),
            Some(std::ffi::OsStr::new("xterm-256color"))
        );
        assert_eq!(
            cmd.get_env("GROVE_MULTI_ROOT"),
            Some(std::ffi::OsStr::new("/session/bundle"))
        );
    }

    #[test]
    fn root_binary_tests_do_not_contact_tmux_from_terminal_sessions() {
        assert!(!super::tmux_side_effects_enabled());
    }

    #[test]
    fn managed_worktree_terminal_sidecar_marker_is_explicit() {
        let mut target = crate::entities::session_registry::SpawnTarget::home("Terminal 1".into());
        target.project = "project".into();
        target.use_tmux = true;
        assert!(super::managed_worktree_terminal_sidecar(&target));
        target.agent = Agent::Claude;
        assert!(!super::managed_worktree_terminal_sidecar(&target));
        target.agent = Agent::Terminal;
        target.use_tmux = false;
        assert!(!super::managed_worktree_terminal_sidecar(&target));
    }

    #[gpui::test]
    fn pending_test_reattach_stays_pending_without_contacting_tmux(cx: &mut gpui::TestAppContext) {
        let session = cx
            .new(|cx| super::TerminalSession::attach_existing("grove__unpainted_test", 24, 80, cx));
        session.update(cx, super::TerminalSession::attach_now);
        assert!(session.read_with(cx, |session, _| session.is_pending_attach()));
        assert!(session.read_with(cx, |session, _| session.spawn_error().is_none()));
    }

    #[gpui::test]
    fn checked_send_rejects_unattached_terminal_instead_of_claiming_input(
        cx: &mut gpui::TestAppContext,
    ) {
        let session = cx.new(|cx| {
            super::TerminalSession::attach_existing("grove__unattached_input_test", 24, 80, cx)
        });
        let result = session.update(cx, |session, _| session.send_checked(b"literal input\n"));
        assert!(result.unwrap_err().contains("no input sent"));
        assert!(session.read_with(cx, |session, _| session.is_pending_attach()));
    }

    #[gpui::test]
    fn large_paste_is_preserved_while_terminal_startup_is_pending(cx: &mut gpui::TestAppContext) {
        let session =
            cx.new(|_| super::TerminalSession::pending(24, 80, super::Backend::Native, false));
        let paste = b"complete pasted command\n".repeat(8_000);
        session.update(cx, |session, _| session.send(&paste));
        assert_eq!(
            session.read_with(cx, |session, _| session.pending_input.clone()),
            paste
        );
    }

    #[cfg(unix)]
    #[gpui::test]
    fn rejected_background_validation_never_starts_a_terminal_or_script(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| cx.set_global(crate::zoom::CurrentPtyDims::default()));
        let mut target = crate::entities::session_registry::SpawnTarget::home("validation".into());
        target.cwd = directory.path().to_str().unwrap().into();
        target.use_tmux = false;
        let terminal = cx.new(|cx| {
            super::TerminalSession::spawn_checked(
                &target,
                &[],
                None,
                Box::new(|| Err("worktree removed".into())),
                cx,
            )
        });
        let script = cx.new(|cx| {
            super::TerminalSession::spawn_script_checked(
                "printf executed > forbidden.txt",
                directory.path().to_str().unwrap(),
                Box::new(|| Err("worktree removed".into())),
                cx,
            )
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        for session in [terminal, script] {
            while session.read_with(cx, |session, _| session.is_starting()) {
                assert!(std::time::Instant::now() < deadline);
                cx.run_until_parked();
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(
                session.read_with(cx, |session, _| session.spawn_error().map(str::to_owned)),
                Some("worktree removed".into())
            );
            assert!(session.read_with(cx, |session, _| session.pty.is_none()));
        }
        assert!(!directory.path().join("forbidden.txt").exists());
    }

    #[cfg(target_os = "linux")]
    #[gpui::test]
    fn removal_waits_for_already_spawned_unadopted_child_cleanup(cx: &mut gpui::TestAppContext) {
        use crate::entities::session_registry::{SessionMeta, SessionRegistry};
        use std::sync::mpsc;
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path().to_owned();
        let (entered, started) = mpsc::channel();
        let (release, gate) = futures::channel::oneshot::channel();
        let terminal = cx.new(|cx| {
            let mut terminal =
                super::TerminalSession::pending(24, 80, super::Backend::Native, false);
            terminal.startup = Some(super::TerminalSession::start_pty_inner(
                move || {
                    let mut command = portable_pty::CommandBuilder::new("/bin/sh");
                    command.args(["-c", "exec sleep 30"]);
                    command.cwd(cwd);
                    let pty = grove_terminal::pty::spawn(command, 24, 80).unwrap();
                    entered.send(pty.child_pid().unwrap()).unwrap();
                    Ok((pty, super::Backend::Native, None))
                },
                true,
                Some(gate),
                cx,
            ));
            terminal
        });
        let registry = cx.new(|_| SessionRegistry::new());
        registry.update(cx, |registry, _| {
            let id = registry.next_home_id();
            let path = directory.path().to_str().unwrap().to_string();
            registry.push_wt_shell(
                &path,
                SessionMeta {
                    id,
                    project: "test".into(),
                    wt_path: path.clone(),
                    agent: Agent::Terminal,
                    context_roots: Vec::new(),
                    temp_bundle_path: None,
                    label: "panel shell".into(),
                    restored_title: None,
                    spawned_at: Instant::now(),
                    attention: None,
                    tmux: false,
                    tmux_name: None,
                },
                Some(terminal.clone()),
            );
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let pid = loop {
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(10));
            if let Ok(pid) = started.try_recv() {
                break pid;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        };
        let process = std::path::PathBuf::from(format!("/proc/{pid}"));
        assert!(process.exists());
        registry.update(cx, |registry, cx| {
            registry.remove_worktree_shells_matching_background(|_| true, cx);
        });
        cx.run_until_parked();
        assert!(registry.read_with(cx, |registry, _| registry.has_pending_session_cleanup()));
        assert!(
            process.exists(),
            "removal cannot acknowledge while the startup worker owns a live child"
        );
        release.send(()).unwrap();
        while registry.read_with(cx, |registry, _| registry.has_pending_session_cleanup()) {
            assert!(Instant::now() < deadline);
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(10));
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(registry.read_with(cx, |registry, _| registry.session_cleanup_error().is_none()));
        assert!(
            !process.exists(),
            "destructive removal is released only after the child is reaped"
        );
        assert!(terminal.read_with(cx, |terminal, _| terminal.has_exited()));
    }

    #[cfg(unix)]
    #[gpui::test]
    fn cancelling_pending_script_waits_for_ack_and_never_executes(cx: &mut gpui::TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let session = cx.new(|cx| {
            super::TerminalSession::spawn_script(
                "printf executed > cancelled.txt",
                directory.path().to_str().unwrap(),
                cx,
            )
        });
        session
            .update(cx, super::TerminalSession::request_script_stop)
            .unwrap();
        assert!(session.read_with(cx, |session, _| session.is_starting()));
        assert!(
            !session.read_with(cx, |session, _| session.has_exited()),
            "cancellation requires worker acknowledgement"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !session.read_with(cx, |session, _| session.has_exited()) {
            assert!(std::time::Instant::now() < deadline);
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!directory.path().join("cancelled.txt").exists());
        assert!(session.read_with(cx, |session, _| session.spawn_error().is_none()));
    }

    #[cfg(unix)]
    #[gpui::test]
    fn requested_script_stop_waits_for_observed_process_exit(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;
        let session = cx.new(|cx| super::TerminalSession::spawn_script("sleep 30", "/tmp", cx));
        assert!(session.read_with(cx, |session, _| session.spawn_error().is_none()));
        assert!(!session.read_with(cx, |session, _| session.has_exited()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while session.read_with(cx, |session, _| session.starting)
            && std::time::Instant::now() < deadline
        {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(session.read_with(cx, |session, _| session.pty.is_some()));
        session
            .update(cx, super::TerminalSession::request_script_stop)
            .unwrap();
        // The request cannot synthesize an exit; the PTY remains until EOF/exit polling.
        assert!(!session.read_with(cx, |session, _| session.has_exited()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !session.read_with(cx, |session, _| session.has_exited())
            && std::time::Instant::now() < deadline
        {
            cx.run_until_parked();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(10));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let exited = session.read_with(cx, |session, _| session.has_exited());
        session.update(cx, super::TerminalSession::stop_script);
        assert!(
            exited,
            "PTY termination was not observed within five seconds"
        );
    }

    #[cfg(unix)]
    #[gpui::test]
    fn reader_eof_keeps_a_live_script_stoppable_until_confirmed_exit(
        cx: &mut gpui::TestAppContext,
    ) {
        let mut command = portable_pty::CommandBuilder::new("/bin/sh");
        command.args(["-c", "exec sleep 30"]);
        let pty = grove_terminal::pty::spawn(command, 24, 80).unwrap();
        let session = cx.new(|_| {
            let mut session = super::TerminalSession::pending(24, 80, super::Backend::Native, true);
            session.starting = false;
            session.pty = Some(pty);
            session
        });
        session.update(cx, super::TerminalSession::reader_closed);
        assert!(
            !session.read_with(cx, |session, _| session.has_exited()),
            "stream EOF cannot acknowledge a still-running child"
        );
        session
            .update(cx, super::TerminalSession::request_script_stop)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !session.read_with(cx, |session, _| session.has_exited()) {
            assert!(Instant::now() < deadline);
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(25));
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(session.read_with(cx, |session, _| session.exit_code().is_some()));
    }

    #[gpui::test]
    fn reader_eof_latches_exit_and_notifies_observers(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;
        let session = cx.new(super::TerminalSession::native_for_test);
        assert!(!session.read_with(cx, |session, _| session.has_exited()));
        let notified = std::rc::Rc::new(std::cell::Cell::new(false));
        let seen = notified.clone();
        let _observer = cx.update(|cx| cx.observe(&session, move |_, _| seen.set(true)));
        session.update(cx, super::TerminalSession::reader_closed);
        cx.run_until_parked();
        assert!(session.read_with(cx, |session, _| session.has_exited()));
        assert!(notified.get());
    }

    #[gpui::test]
    fn osc7_updates_live_cwd_without_changing_launch_cwd(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;
        let session = cx.new(super::TerminalSession::native_for_test);
        session.update(cx, |session, _| session.initial_cwd = Some("/".into()));
        assert_eq!(
            session.read_with(cx, |session, _| session.initial_cwd().map(str::to_owned)),
            Some("/".to_string())
        );
        assert_eq!(
            session.read_with(cx, |session, _| session.current_cwd().map(str::to_owned)),
            None
        );
        let notified = std::rc::Rc::new(std::cell::Cell::new(false));
        let seen = notified.clone();
        let _observer = cx.update(|cx| cx.observe(&session, move |_, _| seen.set(true)));
        session.update(cx, |session, cx| {
            session.ingest(&[b"\x1b]7;file:///tmp/reported\x07".to_vec()], cx);
        });
        cx.run_until_parked();
        assert_eq!(
            session.read_with(cx, |session, _| session.current_cwd().map(str::to_owned)),
            Some("/tmp/reported".to_string())
        );
        assert_eq!(
            session.read_with(cx, |session, _| session.initial_cwd().map(str::to_owned)),
            Some("/".to_string())
        );
        assert!(notified.get());
    }

    #[gpui::test]
    fn osc_title_change_notifies_observers(cx: &mut gpui::TestAppContext) {
        let session = cx.new(super::TerminalSession::native_for_test);
        // Consume the terminal's initial full damage before observing title-only output.
        session.update(cx, |session, cx| session.ingest(&[Vec::new()], cx));
        let notifications = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = notifications.clone();
        let _observer = cx.update(|cx| cx.observe(&session, move |_, _| seen.set(seen.get() + 1)));
        session.update(cx, |session, cx| {
            session.ingest(&[b"\x1b]2;Fix build\x07".to_vec()], cx);
        });
        cx.run_until_parked();
        assert_eq!(notifications.get(), 1);
        assert_eq!(
            session.read_with(cx, |session, _| session.title()),
            Some("Fix build".into())
        );
    }

    #[test]
    fn title_only_change_requires_notification_without_grid_damage() {
        assert!(output_needs_notify(7, 7, None, Some("Fix build")));
        assert!(output_needs_notify(
            7,
            7,
            Some("Fix build"),
            Some("Fix tests")
        ));
        assert!(!output_needs_notify(
            7,
            7,
            Some("Fix build"),
            Some("Fix build")
        ));
        assert!(!output_needs_notify(7, 7, None, None));
    }

    #[test]
    fn output_is_stale_until_the_pty_produces_bytes() {
        let now = Instant::now();
        assert_eq!(output_age_at(None, now), Duration::MAX);
        let earlier = now
            .checked_sub(Duration::from_secs(2))
            .expect("test clock supports a two-second interval");
        assert_eq!(output_age_at(Some(earlier), now), Duration::from_secs(2));
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
