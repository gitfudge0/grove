mod app;
mod clipboard;
mod gui;
mod logging;
mod telemetry;

use anyhow::Result;

fn main() -> Result<()> {
    logging::init();
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };
        tracing::error!(message = %msg, "panic");
        // The message stays local: free text can carry user data (paths,
        // branch names, config values). Only the code location leaves the
        // machine, and even that gets its paths scrubbed first.
        let location = info.location().map_or_else(
            || "unknown".to_string(),
            |l| telemetry::scrub_paths(&format!("{}:{}:{}", l.file(), l.line(), l.column())),
        );
        telemetry::track_blocking("panic", vec![("location", location.into())]);
        prev_hook(info);
    }));
    gui::run()
}
