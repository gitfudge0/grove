//! The startup sequence — the single ordered boot every later plan appends to. Order is load-bearing; each step says why.
//! Ported from `src/app/mod.rs:176-215` and `src/gui/mod.rs:50-68`.

use grove_core::storage::{self, AppearancePreference, Store};
use grove_core::theme;

use crate::settings::SettingsState;
use crate::theme::{ThemeState, DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME};
use crate::zoom::{ZoomState, ZOOM_DEFAULT, ZOOM_MAX, ZOOM_MIN};

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

    // 4. The two built-in terminal palettes are fixed. The OS mode arrives with the window.
    let appearance = store.appearance();
    let follow_system = appearance == AppearancePreference::System;
    let light = appearance == AppearancePreference::Light;
    theme::set_by_name(if light { DEFAULT_LIGHT_THEME } else { DEFAULT_DARK_THEME });

    // 7. Clamped so a hand-edited store.json can't make the chrome unusable.
    let zoom = resolve_zoom(&store);

    // 7b. The stored preference gates the runtime atomic first, so `app_launched` can't transmit for an opted-out user.
    crate::telemetry::set_enabled(SettingsState::telemetry_enabled(&store));
    crate::telemetry::track(
        "app_launched",
        vec![
            ("project_count", (store.projects.len() as u64).into()),
            (
                "tmux_enabled",
                (grove_core::tmux::available() && store.tmux_enabled.unwrap_or(false)).into(),
            ),
        ],
    );
    crate::telemetry::start_heartbeat();

    // 8. In dependency order.
    crate::theme::set_chrome_light(light);
    cx.set_global(SettingsState::new(store));
    cx.set_global(ThemeState::new(
        follow_system,
        DEFAULT_DARK_THEME.into(),
        DEFAULT_LIGHT_THEME.into(),
    ));
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

    #[test]
    fn legacy_theme_name_only_selects_light_or_dark() {
        let store = Store {
            theme: Some("catppuccin-latte".into()),
            ..Store::default()
        };
        assert_eq!(store.appearance(), AppearancePreference::Light);
        let store = Store {
            appearance: Some(AppearancePreference::Dark),
            ..store
        };
        assert_eq!(store.appearance(), AppearancePreference::Dark);
    }

}
