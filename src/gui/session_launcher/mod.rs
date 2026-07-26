//! Recents-first command palette / session launcher (`Modal::SessionLauncher`
//! marker / `Grove::launcher`). Batch A moved the transient presentation
//! state off `App::Modal` and onto `Grove`; batch B1 moves the update-side
//! logic and its `Msg` variants here too. `view.rs`'s render functions stay
//! where they are for now (batch B2).
//!
//! Depends on [`super::launcher`] (pure, Iced-free selection/fuzzy-match/
//! tile-order logic) but is itself the GUI-facing home for the palette's
//! state shape and behavior.
//!
//! Unlike `add_project`/`scripts_editor`/`theme_manager_editor`, this
//! module's methods are `impl Grove`, not free functions taking `&mut App`.
//! The palette isn't a self-contained wizard scoped to its own state plus
//! `App`: it reads and drives `Grove`-only state that also backs other
//! features entirely (`wt_cache`, `tile_order`/`grid_view`, `ui_zoom`,
//! `upgrade`/`upgrade_method`, `settings_tools`, `theme_manager_editor`) and
//! dispatches roughly twenty other `Msg` variants recursively through
//! `Grove::update`. See this batch's report for the full accounting; the
//! short version is that a true `&mut App`-only boundary isn't achievable
//! here without either threading a dozen extra parameters through every
//! call or duplicating Grove-only machinery, so the methods stay `impl
//! Grove`, physically relocated for organization rather than type-system
//! isolation.
//!
//! Once one 6000-line file, since split for navigability. Module layout:
//! - [`state`]       — `LauncherState`/`Msg`/pane and options/settings shapes
//! - [`palette`]      — open/input/activate/resolve, row-actions, switch-strip
//! - [`settings`]     — Settings drill-in root + backend/permissions/
//!   default-agent/app-size panes
//! - [`theme_panes`]  — app-scoped and project-scoped theme pane logic
//! - [`keys`]         — `handle_session_launcher_key`
//! - [`helpers`]      — pure free functions: scroll offsets, ranking,
//!   identity resolution, row builders
//! - [`view`]         — everything that renders the palette to an `Element`

mod helpers;
mod keys;
mod palette;
mod settings;
mod state;
#[cfg(test)]
mod tests;
mod theme_panes;
mod view;

// Re-exports for the handful of items `update.rs`/`view.rs`/
// `theme_manager_editor.rs` reach into by the same `session_launcher::…`
// path they used before the split. Everything else here is private to this
// directory.
pub(super) use helpers::{theme_editor_scroll_offset, update_available_actions, UpdateAction};
pub(super) use state::{project_theme_preview, LauncherState, Msg, SettingsPane};
