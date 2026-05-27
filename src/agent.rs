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

    pub fn program(self) -> String {
        match self {
            Agent::Claude => "claude".into(),
            Agent::Codex => "codex".into(),
            Agent::OpenCode => "opencode".into(),
            // The user's login shell, falling back to a POSIX shell.
            Agent::Terminal => std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
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
                let name = self.label(); // label() == binary name for all non-Terminal variants
                std::env::var_os("PATH")
                    .is_some_and(|paths| {
                        std::env::split_paths(&paths)
                            .any(|dir| is_executable(dir.join(name)))
                    })
            }
        }
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
