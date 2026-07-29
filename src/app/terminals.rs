use super::App;
use grove_core::agent::Agent;
use grove_core::session::Session;

impl App {
    /// Absolute path of the home directory, falling back to `/` if it can't be
    /// resolved. Used as the home terminal's working directory.
    fn home_dir() -> String {
        dirs::home_dir().map_or_else(|| "/".into(), |p| p.to_string_lossy().into_owned())
    }

    /// The home terminal the tab is currently showing, if any.
    pub(crate) fn active_home_terminal(&self) -> Option<&Session> {
        self.active_terminal
            .and_then(|i| self.home_terminals.get(i))
    }

    /// Ensure at least one home terminal exists (spawning the first on demand),
    /// resize them all to the current pane, and make sure something is selected.
    pub(crate) fn ensure_home_terminal(&mut self, rows: u16, cols: u16) {
        if self.home_terminals.is_empty() {
            self.spawn_home_terminal(rows, cols);
        } else {
            for s in &mut self.home_terminals {
                s.resize(rows, cols);
            }
        }
        if self.active_terminal.is_none() && !self.home_terminals.is_empty() {
            self.active_terminal = Some(0);
        }
    }

    /// Spawn an additional home terminal and focus it.
    pub(crate) fn new_home_terminal(&mut self, rows: u16, cols: u16) {
        self.spawn_home_terminal(rows, cols);
    }

    /// Replace the active terminal's shell in place with a fresh one at `~`,
    /// keeping its slot and label. Used to recover an exited terminal.
    pub(crate) fn restart_active_terminal(&mut self, rows: u16, cols: u16) {
        let Some(i) = self.active_terminal else {
            return;
        };
        if i >= self.home_terminals.len() {
            return;
        }
        let label = self.home_terminals[i].label.clone();
        // Only swap once the replacement is live: on spawn failure
        // `build_home_terminal` toasts and we keep the (usually exited)
        // terminal in place rather than leaving an empty slot.
        if let Some(s) = self.build_home_terminal(label, rows, cols) {
            let mut old = std::mem::replace(&mut self.home_terminals[i], s);
            old.kill();
        }
    }

    /// Close the terminal at `idx`. The terminal count may reach zero; the
    /// workspace shows an empty state and `ensure_home_terminal`/
    /// `Msg::NewHomeTerminal` spawn a fresh one on demand.
    pub(crate) fn close_home_terminal(&mut self, idx: usize) {
        if idx >= self.home_terminals.len() {
            return;
        }
        let mut s = self.home_terminals.remove(idx);
        s.kill();
        self.active_terminal = match self.active_terminal {
            Some(a) if a == idx => {
                if self.home_terminals.is_empty() {
                    None
                } else {
                    Some(idx.min(self.home_terminals.len() - 1))
                }
            }
            Some(a) if a > idx => Some(a - 1),
            other => other,
        };
    }

    fn spawn_home_terminal(&mut self, rows: u16, cols: u16) {
        self.home_terminal_seq += 1;
        let label = format!("terminal {}", self.home_terminal_seq);
        if let Some(s) = self.build_home_terminal(label, rows, cols) {
            self.home_terminals.push(s);
            self.active_terminal = Some(self.home_terminals.len() - 1);
        }
    }

    /// Build a native home-terminal session at `~`, sized to the pane. Always
    /// native: a local convenience shell, not a worktree-backed agent that
    /// needs to survive grove restarts via tmux. Returns `None` (and toasts) on
    /// spawn failure.
    fn build_home_terminal(&mut self, label: String, rows: u16, cols: u16) -> Option<Session> {
        let home = Self::home_dir();
        let args = Agent::Terminal.launch_args(false, false);
        match Session::spawn(
            label,
            String::new(),
            home.clone(),
            Agent::Terminal,
            &args,
            &home,
            false,
        ) {
            Ok(mut s) => {
                s.resize(rows, cols);
                Some(s)
            }
            Err(e) => {
                self.set_error_toast(format!("terminal failed: {e}"));
                None
            }
        }
    }

    // ── per-worktree terminal panel ────────────────────────────────────────

    /// The shells of the panel for `wt_path` (empty if none spawned yet).
    pub(crate) fn wt_terminals_for(&self, wt_path: &str) -> &[Session] {
        self.wt_terminals
            .get(wt_path)
            .map_or(&[][..], std::vec::Vec::as_slice)
    }

    /// The active shell of the panel for `wt_path`, if any.
    pub(crate) fn active_wt_terminal(&self, wt_path: &str) -> Option<&Session> {
        let i = *self.wt_active_terminal.get(wt_path)?;
        self.wt_terminals.get(wt_path)?.get(i)
    }

    /// Active shell index within the panel for `wt_path`.
    pub(crate) fn active_wt_terminal_idx(&self, wt_path: &str) -> Option<usize> {
        self.wt_active_terminal.get(wt_path).copied()
    }

    /// Ensure the panel for `wt_path` has at least one shell, resize them all,
    /// and select something. Mirrors `ensure_home_terminal` but rooted in the
    /// worktree rather than `~`.
    pub(crate) fn ensure_wt_terminal(&mut self, wt_path: &str, rows: u16, cols: u16) {
        match self.wt_terminals.get_mut(wt_path) {
            Some(v) if !v.is_empty() => {
                for s in v {
                    s.resize(rows, cols);
                }
            }
            // Missing or present-but-empty: spawn the first shell (which also
            // sets the active index).
            _ => self.spawn_wt_terminal(wt_path, rows, cols),
        }
        if !self.wt_active_terminal.contains_key(wt_path)
            && !self.wt_terminals_for(wt_path).is_empty()
        {
            self.wt_active_terminal.insert(wt_path.to_string(), 0);
        }
    }

    /// Spawn an additional panel shell for `wt_path` and focus it.
    pub(crate) fn new_wt_terminal(&mut self, wt_path: &str, rows: u16, cols: u16) {
        self.spawn_wt_terminal(wt_path, rows, cols);
    }

    /// Focus the panel shell at `idx` for `wt_path`.
    pub(crate) fn select_wt_terminal(&mut self, wt_path: &str, idx: usize, rows: u16, cols: u16) {
        if let Some(v) = self.wt_terminals.get_mut(wt_path) {
            if idx < v.len() {
                v[idx].resize(rows, cols);
                self.wt_active_terminal.insert(wt_path.to_string(), idx);
            }
        }
    }

    /// Close the panel shell at `idx` for `wt_path`. Unlike the home terminal
    /// this does *not* respawn when the last one closes — an empty panel is a
    /// valid state (the panel shows its empty/start affordance).
    pub(crate) fn close_wt_terminal(&mut self, wt_path: &str, idx: usize) {
        let Some(v) = self.wt_terminals.get_mut(wt_path) else {
            return;
        };
        if idx >= v.len() {
            return;
        }
        let mut s = v.remove(idx);
        s.kill();
        let new_active = match self.wt_active_terminal.get(wt_path).copied() {
            Some(a) if a == idx => {
                if v.is_empty() {
                    None
                } else {
                    Some(idx.min(v.len() - 1))
                }
            }
            Some(a) if a > idx => Some(a - 1),
            other => other,
        };
        match new_active {
            Some(a) => {
                self.wt_active_terminal.insert(wt_path.to_string(), a);
            }
            None => {
                self.wt_active_terminal.remove(wt_path);
            }
        }
    }

    /// Kill and drop every panel shell for `wt_path`. Called when the owning
    /// worktree/session is removed so no orphaned shells survive.
    pub(crate) fn kill_wt_terminals(&mut self, wt_path: &str) {
        if let Some(mut v) = self.wt_terminals.remove(wt_path) {
            for s in &mut v {
                s.kill();
            }
        }
        self.wt_active_terminal.remove(wt_path);
    }

    fn spawn_wt_terminal(&mut self, wt_path: &str, rows: u16, cols: u16) {
        self.wt_terminal_seq += 1;
        let label = format!("wt-terminal {}", self.wt_terminal_seq);
        let args = Agent::Terminal.launch_args(false, false);
        match Session::spawn(
            label,
            String::new(),
            wt_path.to_string(),
            Agent::Terminal,
            &args,
            wt_path,
            false,
        ) {
            Ok(mut s) => {
                s.resize(rows, cols);
                let v = self.wt_terminals.entry(wt_path.to_string()).or_default();
                v.push(s);
                self.wt_active_terminal
                    .insert(wt_path.to_string(), v.len() - 1);
            }
            Err(e) => {
                self.set_error_toast(format!("terminal failed: {e}"));
            }
        }
    }
}
