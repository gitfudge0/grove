//! Grove's domain layer (git worktrees, tmux, storage, themes, agents, self-upgrade); no UI-framework dependency, so the GUI/domain boundary is compiler-enforced.

pub mod agent;
pub mod attention;
pub mod claude_agents;
pub mod control;
pub mod diff;
pub mod env_path;
pub mod error;
pub mod git;
pub mod highlight;
pub mod multi_root;
pub mod release_highlights;
pub mod render_rows;
pub mod session_meta;
pub mod skill_install;
pub mod storage;
pub mod theme;
pub mod theme_file;
pub mod tmux;
pub mod upgrade;

pub mod control_state;
