mod agent;
mod app;
mod attention;
mod claude_agents;
mod clipboard;
mod env_path;
mod git;
mod gui;
mod session;
mod session_meta;
mod storage;
mod telemetry;
mod theme;
mod theme_file;
mod tmux;
mod upgrade;

use anyhow::Result;

fn main() -> Result<()> {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };
        telemetry::track_blocking("panic", vec![("message", msg.into())]);
        prev_hook(info);
    }));
    gui::run()
}
