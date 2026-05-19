use anyhow::Result;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::stdout;
use std::process::Command;

/// Suspend the TUI, run a foreground command (inheriting stdio), then restore.
pub fn run_inline<F: FnOnce() -> Result<()>>(f: F) -> Result<()> {
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    let res = f();
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    res
}

pub fn run_claude_inline(cwd: &str, args: &[&str]) -> Result<()> {
    let status = Command::new("claude").args(args).current_dir(cwd).status()?;
    if !status.success() {
        anyhow::bail!("claude exited with {:?}", status.code());
    }
    Ok(())
}
