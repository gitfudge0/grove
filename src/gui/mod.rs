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
//! - [`add_project`] — two-step add-project wizard (`Modal::AddProject`)
//! - [`onboarding`] — first-run onboarding wizard view
//! - [`palette`]  — color tokens
//! - [`scripts_editor`] — per-project lifecycle-scripts editor
//! - [`session_launcher`] — command palette / session launcher (`Modal::SessionLauncher`): state,
//!   behavior, keys, and view, split across its own directory (see that module's doc comment)
//! - [`slide`]    — draw-only translation wrapper for the grid slide animation
//! - [`theme_manager_editor`] — `Modal::ThemeManager`'s EDITOR sub-view

mod activity;
mod add_project;
mod dock;
mod drop;
mod icons;
mod keys;
pub(crate) mod launcher;
mod metrics;
mod onboarding;
mod palette;
mod pty;
mod rows;
mod scripts_editor;
mod session_launcher;
mod slide;
mod state;
mod theme_manager_editor;
pub(crate) mod update;
mod view;
mod widgets;

use crate::gui::state::UpgradeMsg;
use anyhow::Result;
use iced::{Size, Task, Theme};
use state::{Grove, Msg};

pub fn run() -> Result<()> {
    // When launched from Finder/Launchpad/.desktop the process inherits a
    // minimal PATH; recover the user's login PATH before spawning any PTYs.
    grove_core::env_path::ensure_login_path();

    iced::application(
        || {
            // Fire a background update check ~3 s after startup so the UI is
            // fully rendered before the network round-trip begins.
            let launch_check = Task::perform(
                async {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                },
                |()| Msg::Upgrade(UpgradeMsg::CheckForUpdates { manual: false }),
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
    .theme(|_: &Grove| match grove_core::theme::current().kind {
        grove_core::theme::ThemeKind::Light => Theme::Light,
        grove_core::theme::ThemeKind::Dark => Theme::Dark,
    })
    .scale_factor(|state| state.pty_layout.zoom)
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
