use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Agent {
    Claude,
    Codex,
    OpenCode,
    Terminal,
}

impl Agent {
    pub const ALL: [Agent; 4] = [
        Agent::Claude,
        Agent::Codex,
        Agent::OpenCode,
        Agent::Terminal,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::OpenCode => "opencode",
            Agent::Terminal => "terminal",
        }
    }

    /// Name of the inline SVG sprite (see `gui::icons`) representing this
    /// agent. `Terminal` reuses the generic terminal glyph.
    pub fn icon_name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::OpenCode => "opencode",
            Agent::Terminal => "term",
        }
    }

    pub fn program(self) -> String {
        match self {
            Agent::Claude | Agent::Codex | Agent::OpenCode => self.binary_name().into(),
            // The user's login shell (validated), falling back to a POSIX shell.
            Agent::Terminal => crate::env_path::login_shell(),
        }
    }

    /// Executable name to look up on `$PATH`. `Terminal` has no static name
    /// (resolved at runtime via `$SHELL`) so callers must guard against it.
    fn binary_name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::OpenCode => "opencode",
            Agent::Terminal => "",
        }
    }

    pub fn launch_args(self) -> Vec<String> {
        match self {
            Agent::Claude => vec!["--dangerously-skip-permissions".into()],
            Agent::Codex => vec![],
            Agent::OpenCode => vec![],
            Agent::Terminal => vec![],
        }
    }

    /// Returns true if the binary for this agent can be found on `$PATH` and
    /// has at least one execute bit set. `Terminal` is always available (it
    /// resolves via `$SHELL`). Returns `false` — never panics — when `$PATH`
    /// is unset.
    ///
    /// # Platform
    /// The execute-bit check uses `std::os::unix::fs::PermissionsExt` on Unix.
    /// On other targets it falls back to `is_file()`.
    pub fn available(self) -> bool {
        match self {
            Agent::Terminal => true,
            Agent::Claude | Agent::Codex | Agent::OpenCode => {
                let name = self.binary_name();
                std::env::var_os("PATH").is_some_and(|paths| {
                    std::env::split_paths(&paths).any(|dir| is_executable(dir.join(name)))
                })
            }
        }
    }

    /// Runs `<program> --version` and returns the trimmed first non-empty line
    /// of stdout — robust across the three CLIs' differing formats. Returns
    /// `None` if the agent has no static binary (`Terminal`), the command fails
    /// to spawn or run, or it yields no usable output; callers then fall back to
    /// displaying "installed". This shells out, so callers should run it off the
    /// UI thread.
    pub fn version(self) -> Option<String> {
        if matches!(self, Agent::Terminal) {
            return None;
        }
        let name = self.binary_name();
        if name.is_empty() {
            return None;
        }
        let output = std::process::Command::new(name)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string)
    }
}

/// Returns true if `path` is a regular file with at least one execute bit set.
/// Falls back to `is_file()` on non-Unix targets.
fn is_executable(path: std::path::PathBuf) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(&path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}
