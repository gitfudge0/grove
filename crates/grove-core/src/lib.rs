//! Grove's domain layer (git worktrees, tmux, storage, themes, agents, self-upgrade); no UI-framework dependency, so the GUI/domain boundary is compiler-enforced.

pub mod agent;
pub mod attention;
pub mod claude_agents;
pub mod diff;
pub mod env_path;
pub mod error;
pub mod git;
pub mod highlight;
pub mod render_rows;
pub mod session_meta;
pub mod storage;
pub mod theme;
pub mod theme_file;
pub mod tmux;
pub mod upgrade;
