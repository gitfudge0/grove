//! The startup sequence — the single ordered boot every later plan appends to.
//!
//! Ported from `src/app/mod.rs:176-215` and `src/gui/mod.rs:50-68`. The order
//! is load-bearing; each step says why.

use grove_core::storage::{self, Store};
use grove_core::theme;

use crate::settings::SettingsState;
use crate::theme::{ThemeState, DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME};
use crate::zoom::{ZoomState, ZOOM_DEFAULT, ZOOM_MAX, ZOOM_MIN};

/// Rewrites any of `store.theme`/`theme_dark`/`theme_light` that names a theme
/// `theme::by_name` can no longer resolve (a builtin dropped from a later
/// curated set, or a custom theme deleted outside the app) to
/// `DEFAULT_DARK_THEME`. Returns whether anything changed, so the caller only
/// re-persists when needed.
///
/// **Copied** from `src/app/theme_picker.rs:34-49` rather than hoisted into
/// grove-core: the spec reuses grove-core *unchanged*, so the glue lives on
/// the UI side of the line, in both UIs, until iced is deleted (Plan 10).
pub fn migrate_stale_theme_names(store: &mut Store) -> bool {
    let mut changed = false;
    for slot in [
        &mut store.theme,
        &mut store.theme_dark,
        &mut store.theme_light,
    ] {
        if let Some(name) = slot.as_deref() {
            if theme::by_name(name).is_none() {
                *slot = Some(DEFAULT_DARK_THEME.to_string());
                changed = true;
            }
        }
    }
    changed
}

/// Clamp a persisted zoom into the supported range (`src/app/mod.rs` reads
/// `store.ui_zoom` the same way).
pub fn resolve_zoom(store: &Store) -> f32 {
    store
        .ui_zoom
        .unwrap_or(ZOOM_DEFAULT)
        .clamp(ZOOM_MIN, ZOOM_MAX)
}

/// Runs the whole startup sequence and installs every global. Called once,
/// before the window opens.
pub fn boot(cx: &mut gpui::App) {
    // 1. When launched from Finder/Launchpad/.desktop the process inherits a
    //    minimal PATH; recover the login PATH before anything spawns
    //    (`src/gui/mod.rs:52-54`).
    grove_core::env_path::ensure_login_path();

    // 2. Stale attention-state GC, before any session id can be reused
    //    (`src/gui/update/mod.rs:90-93`).
    grove_core::attention::cleanup_stale_files();

    // 3. Settings. A failure here is genuinely unrecoverable — there is no UI
    //    to report into yet. The iced app hard-fails deliberately; this is the
    //    same decision without a panic, since `expect_used` is denied on
    //    production paths.
    let mut store = match storage::load() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("grove-gpui: could not load settings: {e}");
            eprintln!("grove-gpui: could not load settings: {e}");
            std::process::exit(1);
        }
    };

    // 4. User themes must exist before a persisted custom name is resolved,
    //    or a valid custom selection silently falls back on the first frame.
    let _ = theme::load_custom();

    // 5. One-time migration of dead theme names, so a stale name neither
    //    lingers in store.json nor leaves `theme::ACTIVE` on its static
    //    initializer (TOKYONIGHT, not DEFAULT_DARK_THEME) unnoticed.
    if migrate_stale_theme_names(&mut store) {
        storage::persist(&store);
    }

    // 6. Resolve the active theme. In follow-system mode the real OS
    //    appearance only arrives once a window exists, but that first frame
    //    still needs a concrete theme — seed from the saved dark theme and
    //    let `ThemeState::set_system_mode` correct it (`src/app/mod.rs:180-207`).
    let follow_system = store.theme_follow_system;
    let dark_name = store
        .theme_dark
        .clone()
        .unwrap_or_else(|| DEFAULT_DARK_THEME.to_string());
    let light_name = store
        .theme_light
        .clone()
        .unwrap_or_else(|| DEFAULT_LIGHT_THEME.to_string());
    if follow_system {
        if !theme::set_by_name(&dark_name) {
            theme::set_by_name(DEFAULT_DARK_THEME);
        }
    } else if let Some(name) = store.theme.as_deref() {
        if !theme::set_by_name(name) {
            theme::set_by_name(DEFAULT_DARK_THEME);
        }
    }

    // 7. Zoom, clamped — a hand-edited store.json must not be able to make the
    //    chrome unusable.
    let zoom = resolve_zoom(&store);

    // Plan 09: telemetry (`app_launched`, heartbeat) + the scrubbing panic hook.

    // 8. Globals, in dependency order.
    cx.set_global(SettingsState::new(store));
    cx.set_global(ThemeState::new(follow_system, dark_name, light_name));
    cx.set_global(ZoomState::new(zoom));
    cx.bind_keys(crate::keymap::bindings());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(theme: Option<&str>, dark: Option<&str>, light: Option<&str>) -> Store {
        Store {
            theme: theme.map(str::to_string),
            theme_dark: dark.map(str::to_string),
            theme_light: light.map(str::to_string),
            ..Store::default()
        }
    }

    #[test]
    fn migration_rewrites_only_unresolvable_names() {
        let mut s = store_with(Some("definitely-not-a-theme"), Some("tokyonight"), None);
        assert!(migrate_stale_theme_names(&mut s));
        assert_eq!(s.theme.as_deref(), Some(DEFAULT_DARK_THEME));
        assert_eq!(s.theme_dark.as_deref(), Some("tokyonight"));
        assert_eq!(s.theme_light, None);
    }

    #[test]
    fn migration_is_a_noop_when_everything_resolves() {
        let mut s = store_with(Some("tokyonight"), Some("tokyonight"), None);
        assert!(!migrate_stale_theme_names(&mut s));
        assert_eq!(s.theme.as_deref(), Some("tokyonight"));
    }

    #[test]
    fn zoom_is_clamped_and_defaults_to_one() {
        let mut s = Store::default();
        assert_eq!(resolve_zoom(&s), ZOOM_DEFAULT);
        s.ui_zoom = Some(99.0);
        assert_eq!(resolve_zoom(&s), ZOOM_MAX);
        s.ui_zoom = Some(0.01);
        assert_eq!(resolve_zoom(&s), ZOOM_MIN);
        s.ui_zoom = Some(1.4);
        assert_eq!(resolve_zoom(&s), 1.4);
    }
}
