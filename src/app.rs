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
    // GTK owns WebKit's asynchronous loading, input and IPC, including while a
    // retained document is hidden. Dispatch it on GPUI's foreground thread.
    #[cfg(all(target_os = "linux", not(test)))]
    cx.spawn(async move |cx| loop {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(16))
            .await;
        cx.update(crate::platform::inline_html::NativeDocument::pump);
    })
    .detach();

    // 1. Finder/Launchpad/.desktop launches inherit a minimal PATH; recover exported shell variables once before anything spawns.
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

    // 4. Resolve the standalone theme; legacy system appearance is seeded by the window.
    let appearance = store.appearance();
    let follow_system = store
        .selected_theme
        .as_deref()
        .and_then(grove_core::app_theme::by_id)
        .is_none()
        && appearance == AppearancePreference::System;
    let light = appearance == AppearancePreference::Light;
    theme::set_by_name(store.selected_theme_id(light));
    if store.selected_theme.is_none() && !follow_system {
        let id = store.selected_theme_id(light).to_string();
        let mut migrated = store.clone();
        migrated.selected_theme = Some(id);
        match storage::save(&migrated) {
            Ok(()) => store = migrated,
            Err(error) => tracing::warn!(%error, "could not save legacy theme migration"),
        }
    }

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
    cx.set_global(SettingsState::new(store));
    cx.set_global(ThemeState::new(
        follow_system,
        DEFAULT_DARK_THEME.into(),
        DEFAULT_LIGHT_THEME.into(),
    ));
    cx.set_global(ZoomState::new(zoom));
    cx.set_global(crate::zoom::CurrentPtyDims::default());

    // 9. Must run before `keymap::bindings()`: a modal's Input binding and gpui-component's plain "Input" binding tie-break by registration order, and Grove must win to claim ←/→/Tab back from the caret.
    // The component Root supplies native text selection. Copy bindings are scoped
    // to TextView/Input so terminal Ctrl+C still reaches the PTY.
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
