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

mod activity;
mod dock;
mod drop;
mod icons;
mod keys;
mod metrics;
mod palette;
mod pty;
mod rows;
mod state;
mod update;
mod view;
mod widgets;

use anyhow::Result;
use iced::{Size, Task, Theme};
use state::Grove;

pub fn run() -> Result<()> {
    // When launched from Finder/Launchpad/.desktop the process inherits a
    // minimal PATH; recover the user's login PATH before spawning any PTYs.
    crate::env_path::ensure_login_path();

    iced::application("grove", Grove::update, Grove::view)
        .theme(|_| match crate::theme::current().kind {
            crate::theme::ThemeKind::Light => Theme::Light,
            crate::theme::ThemeKind::Dark => Theme::Dark,
        })
        .scale_factor(|state| state.ui_zoom as f64)
        .subscription(Grove::subscription)
        .font(metrics::PLEX_SANS_REGULAR)
        .font(metrics::PLEX_SANS_BOLD)
        .font(metrics::MONO_REGULAR)
        .font(metrics::MONO_BOLD)
        .default_font(metrics::UI_FONT)
        .window_size(Size::new(1280.0, 800.0))
        .run_with(|| (Grove::new(), Task::none()))
        .map_err(|e| anyhow::anyhow!(e))
}
