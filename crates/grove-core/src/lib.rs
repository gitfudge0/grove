//! Grove's domain layer: git worktrees, tmux sessions, on-disk storage,
//! themes, agent metadata, and self-upgrade — the pieces of Grove that have
//! nothing to do with drawing pixels. This crate carries no `iced`
//! dependency, so the GUI/domain boundary is enforced by the compiler
//! rather than by convention.

pub mod agent;
pub mod attention;
pub mod claude_agents;
pub mod env_path;
pub mod git;
pub mod session;
pub mod session_meta;
pub mod storage;
pub mod theme;
pub mod theme_file;
pub mod tmux;
pub mod upgrade;
