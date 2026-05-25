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
}
