//! The startup sequence — the single ordered boot every later plan appends to. Order is load-bearing; each step says why.
//! Ported from `src/app/mod.rs:176-215` and `src/gui/mod.rs:50-68`.

use grove_core::storage::{self, Store};
use grove_core::theme;

use crate::settings::SettingsState;
use crate::theme::{ThemeState, DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME};
use crate::zoom::{ZoomState, ZOOM_DEFAULT, ZOOM_MAX, ZOOM_MIN};

/// Rewrites any unresolvable theme name to `DEFAULT_DARK_THEME`; returns whether anything changed.
/// Copied from `src/app/theme_picker.rs:34-49` rather than hoisted into grove-core, since grove-core stays UI-unaware until iced is deleted.
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

/// Clamps a persisted zoom into the supported range.
pub fn resolve_zoom(store: &Store) -> f32 {
    store
        .ui_zoom
        .unwrap_or(ZOOM_DEFAULT)
        .clamp(ZOOM_MIN, ZOOM_MAX)
}

pub fn boot(cx: &mut gpui::App) {
    // 1. Finder/Launchpad/.desktop launches inherit a minimal PATH; recover the login PATH before anything spawns.
    grove_core::env_path::ensure_login_path();

    // 2. Stale attention-state GC, before any session id can be reused.
    grove_core::attention::cleanup_stale_files();

    // 3. A settings-load failure is unrecoverable (no UI to report into yet); exit rather than panic since `expect_used` is denied on production paths.
    let mut store = match storage::load() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("grove-gpui: could not load settings: {e}");
            eprintln!("grove-gpui: could not load settings: {e}");
            std::process::exit(1);
        }
    };

    // 3a2. Adopts orphaned worktree dirs (metadata only) — must run before the session-meta repair below, which resolves ownership off the now-pinned dir.
    let adopted = storage::adopt_orphaned_worktree_dirs(&mut store.projects);
    if adopted > 0 {
        tracing::info!(adopted, "grove-gpui: adopted orphaned worktree directories");
        storage::persist(&store);
    }

    // 3b. Repairs session sidecars naming a now-gone project; must run before anything else reads a sidecar.
    let known_projects: Vec<(String, String)> = store
        .projects
        .iter()
        .map(|p| (p.name.clone(), p.path.clone()))
        .collect();
    let repaired = grove_core::session_meta::repair_stale_projects(&known_projects, |wt_path| {
        storage::project_for_worktree_path(&store.projects, wt_path).map(|(_, p)| p.name.clone())
    });
    if repaired > 0 {
        tracing::info!(repaired, "grove-gpui: repaired stale session-meta projects");
    }

    // 4. User themes must exist before a persisted custom name is resolved, or selection falls back on the first frame.
    let _ = theme::load_custom();

    // 5. One-time migration so a stale theme name doesn't linger unnoticed.
    if migrate_stale_theme_names(&mut store) {
        storage::persist(&store);
    }

    // 6. Follow-system mode's real OS appearance arrives only once a window exists; seed the dark theme for the first frame and let `set_system_mode` correct it.
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

    // 7. Clamped so a hand-edited store.json can't make the chrome unusable.
    let zoom = resolve_zoom(&store);

    // 7b. The stored preference gates the runtime atomic first, so `app_launched` can't transmit for an opted-out user.
    crate::telemetry::set_enabled(SettingsState::telemetry_enabled(&store));
    crate::telemetry::track(
        "app_launched",
        vec![
            (
                "theme",
                store
                    .theme
                    .clone()
                    .unwrap_or_else(|| "default".to_string())
                    .into(),
            ),
            ("project_count", (store.projects.len() as u64).into()),
            (
                "tmux_enabled",
                (grove_core::tmux::available() && store.tmux_enabled.unwrap_or(false)).into(),
            ),
        ],
    );
    crate::telemetry::start_heartbeat();

    // 8. In dependency order.
    cx.set_global(SettingsState::new(store));
    cx.set_global(ThemeState::new(follow_system, dark_name, light_name));
    cx.set_global(ZoomState::new(zoom));
    cx.set_global(crate::zoom::CurrentPtyDims::default());

    // 9. Must run before `keymap::bindings()`: a modal's Input binding and gpui-component's plain "Input" binding tie-break by registration order, and Grove must win to claim ←/→/Tab back from the caret.
    // Grove does NOT mount gpui-component's `Root` view, which binds ctrl-c to its own Copy action and would shadow the PTY's Ctrl+C.
    gpui_component::init(cx);
    // Must follow init: it installs the global this overwrites.
    crate::theme::sync_component_theme(cx);

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
