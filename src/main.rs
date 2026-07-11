mod agent;
mod app;
mod attention;
mod clipboard;
mod env_path;
mod git;
mod gui;
mod session;
mod session_meta;
mod storage;
mod theme;
mod tmux;
mod upgrade;

use anyhow::Result;

fn main() -> Result<()> {
    gui::run()
}
