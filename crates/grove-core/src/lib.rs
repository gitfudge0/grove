//! Grove's domain layer (git worktrees, tmux, storage, themes, agents, self-upgrade); no UI-framework dependency, so the GUI/domain boundary is compiler-enforced.

pub mod agent;
pub mod attention;
pub mod checkout_git;
pub mod checkpoint;
pub mod claude;
pub mod claude_agents;
mod claude_transcript;
pub mod codex;
pub mod controls;
pub mod diff;
pub mod elicitation;
pub mod env_path;
pub mod error;
pub mod git;
pub mod highlight;
pub mod identity;
pub mod latest;
pub mod multi_root;
pub mod release_highlights;
pub mod render_rows;
pub mod session_meta;
pub mod storage;
pub mod tasks;
pub mod theme;
pub mod theme_file;
pub mod tmux;
pub mod upgrade;
