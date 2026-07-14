//! Native desktop interface for Grove. The visual contract is `mockups/gui.html`.
//!
//! Module layout:
//! - [`state`]    — shared model: `Grove`, `Msg`, PTY cache types
//! - [`update`]   — `Grove::new`, subscriptions, and all `Msg` handling
//! - [`view`]     — `Grove::view` and the chrome it composes
//! - [`widgets`]  — small reusable widget primitives
//! - [`rows`]     — sidebar row builders (project / worktree / session)
//! - [`pty`]      — PTY canvas program + row-snapshot construction
//! - [`icons`]    — inline SVG sprite
//! - [`keys`]     — keyboard → PTY byte mapping
//! - [`drop`]     — dropped file paths → PTY text
//! - [`metrics`]  — layout constants
//! - [`palette`]  — color tokens
//! - [`slide`]    — draw-only translation wrapper for the grid slide animation

mod activity;
mod dock;
mod drop;
mod icons;
mod keys;
mod launcher;
mod metrics;
mod palette;
mod pty;
mod rows;
mod slide;
mod state;
mod update;
mod view;
mod widgets;

use anyhow::Result;
use iced::{Size, Task, Theme};
use state::{Grove, Msg};

pub fn run() -> Result<()> {
    // When launched from Finder/Launchpad/.desktop the process inherits a
    // minimal PATH; recover the user's login PATH before spawning any PTYs.
    crate::env_path::ensure_login_path();

    iced::application(
        || {
            // Fire a background update check ~3 s after startup so the UI is
            // fully rendered before the network round-trip begins.
            let launch_check = Task::perform(
                async {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                },
                |_| Msg::CheckForUpdates { manual: false },
            );
            // Seed the OS appearance immediately so "follow system" resolves
            // to the real mode on the first frame rather than waiting for the
            // next OS theme-change notification.
            let seed_system_theme = iced::system::theme().map(Msg::SystemThemeChanged);
            (Grove::new(), Task::batch([launch_check, seed_system_theme]))
        },
        Grove::update,
        Grove::view,
    )
    .title("grove")
    .theme(|_: &Grove| match crate::theme::current().kind {
        crate::theme::ThemeKind::Light => Theme::Light,
        crate::theme::ThemeKind::Dark => Theme::Dark,
    })
    .scale_factor(|state| state.ui_zoom)
    .subscription(Grove::subscription)
    .font(metrics::PLEX_SANS_REGULAR)
    .font(metrics::PLEX_SANS_BOLD)
    .font(metrics::MONO_REGULAR)
    .font(metrics::MONO_BOLD)
    .default_font(metrics::UI_FONT)
    .window_size(Size::new(1280.0, 800.0))
    .exit_on_close_request(false)
    .run()
    .map_err(|e: iced::Error| anyhow::anyhow!(e))
}
