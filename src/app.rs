//! The startup sequence — the single ordered boot every later plan appends to. Order is load-bearing; each step says why.
//! Ported from `src/app/mod.rs:176-215` and `src/gui/mod.rs:50-68`.

use futures::FutureExt as _;
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

#[derive(Clone)]
struct StartupMaintenance(futures::future::Shared<futures::future::BoxFuture<'static, ()>>);
impl gpui::Global for StartupMaintenance {}

/// Await only on asynchronous restore/spawn paths. Tests without boot have no
/// maintenance dependency.
pub(crate) fn startup_ready(
    cx: &gpui::App,
) -> impl std::future::Future<Output = ()> + Send + 'static {
    let ready = cx
        .try_global::<StartupMaintenance>()
        .map(|ready| ready.0.clone());
    async move {
        if let Some(ready) = ready {
            ready.await;
        }
    }
}

pub fn boot(cx: &mut gpui::App) {
    cx.background_executor()
        .spawn(async {
            let _ = grove_core::visuals::cleanup();
        })
        .detach();
    let (wake, mut captures) = futures::channel::mpsc::channel(1);
    let wake = std::sync::Mutex::new(wake);
    grove_core::visual_preview::set_wake(std::sync::Arc::new(move || {
        let _ = wake
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .try_send(());
    }));
    grove_core::visual_preview::enable();
    cx.spawn(async move |cx| {
        use futures::StreamExt as _;
        while captures.next().await.is_some() {
            loop {
                let pending = cx.update(|cx| {
                    cx.windows().first().copied().is_some_and(|handle| {
                        handle
                            .update(cx, |_, window, cx| {
                                crate::views::inline_html::pump_capture(window, cx);
                                crate::views::inline_html::capture_pending()
                                    || grove_core::visual_preview::has_pending()
                            })
                            .unwrap_or(false)
                    })
                }) || grove_core::visual_preview::has_pending();
                if !pending {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(32))
                    .await;
            }
        }
    })
    .detach();

    // GTK owns WebKit's asynchronous loading, input and IPC, including while a
    // retained document is hidden. Dispatch it on GPUI's foreground thread.
    #[cfg(all(target_os = "linux", not(test)))]
    cx.spawn(async move |cx| loop {
        let wait = cx.update(|_| crate::platform::inline_html::prepare_event_wait());
        cx.background_executor()
            .spawn(async move {
                crate::platform::inline_html::wait_events(wait);
            })
            .await;
        cx.update(|cx| {
            crate::platform::inline_html::finish_event_wait();
            crate::platform::text_preferences::poll(cx);
            crate::platform::inline_html::NativeDocument::pump(cx);
        });
    })
    .detach();

    // GUI shell capture must not delay first paint. Session/PTY workers gate
    // executable resolution on this immutable environment snapshot.
    grove_core::env_path::prepare_initial_environment();
    cx.background_executor()
        .spawn(async {
            grove_core::env_path::initialize_login_environment();
        })
        .detach();

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

    // Adoption and sidecar repair preserve their ordering in one worker.
    // Restore awaits application of its result, while first paint proceeds.
    let original_projects = store.projects.clone();
    let mut projects = original_projects.clone();
    let (ready, receive) = futures::channel::oneshot::channel();
    cx.set_global(StartupMaintenance(receive.map(|_| ()).boxed().shared()));
    let maintenance = cx.background_executor().spawn(async move {
        let adopted = storage::adopt_orphaned_worktree_dirs(&mut projects);
        let known: Vec<_> = projects
            .iter()
            .map(|project| (project.name.clone(), project.path.clone()))
            .collect();
        let repaired = grove_core::session_meta::repair_stale_projects(&known, |path| {
            storage::project_for_worktree_path(&projects, path)
                .map(|(_, project)| project.name.clone())
        });
        (projects, adopted, repaired)
    });
    cx.spawn(async move |cx| {
        let (projects, adopted, repaired) = maintenance.await;
        cx.update(|cx| {
            if adopted > 0 {
                SettingsState::update(cx, |store| {
                    for project in &mut store.projects {
                        if let Some((old, updated)) = original_projects
                            .iter()
                            .zip(&projects)
                            .find(|(old, _)| old.path == project.path)
                        {
                            // Preserve an edit made while maintenance ran.
                            if project.name == old.name && project.worktree_dir == old.worktree_dir
                            {
                                project.worktree_dir.clone_from(&updated.worktree_dir);
                            }
                        }
                    }
                });
                tracing::info!(adopted, "adopted orphaned worktree directories");
            }
            if repaired > 0 {
                tracing::info!(repaired, "repaired stale session projects");
            }
        });
        let _ = ready.send(());
    })
    .detach();

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
    if crate::telemetry::enabled() {
        let project_count = store.projects.len() as u64;
        let tmux_enabled = store.tmux_enabled.unwrap_or(false);
        cx.background_executor()
            .spawn(async move {
                grove_core::env_path::wait_for_initial_environment();
                crate::telemetry::track(
                    "app_launched",
                    vec![
                        ("project_count", project_count.into()),
                        (
                            "tmux_enabled",
                            (tmux_enabled && grove_core::tmux::available()).into(),
                        ),
                    ],
                );
            })
            .detach();
    }
    crate::telemetry::start_heartbeat();

    // 8. In dependency order.
    cx.set_global(SettingsState::new(store));
    cx.set_global(ThemeState::new(
        follow_system,
        DEFAULT_DARK_THEME.into(),
        DEFAULT_LIGHT_THEME.into(),
    ));
    cx.set_global(
        ZoomState::new(zoom).with_text_scale(crate::platform::text_preferences::initial_scale()),
    );
    cx.set_global(crate::zoom::CurrentPtyDims::default());

    // 9. Must run before `keymap::bindings()`: a modal's Input binding and gpui-component's plain "Input" binding tie-break by registration order, and Grove must win to claim ←/→/Tab back from the caret.
    // The component Root supplies native text selection. Copy bindings are scoped
    // to TextView/Input so terminal Ctrl+C still reaches the PTY.
    gpui_component::init(cx);
    gpui_component::text::set_rich_clipboard_writer(cx, crate::platform::clipboard::copy_html);
    gpui_component::kbd::Kbd::set_content_renderer(
        |stroke| {
            use gpui::IntoElement as _;
            crate::views::components::shortcut_hint(gpui_component::kbd::Kbd::format(stroke))
                .into_any_element()
        },
        cx,
    );
    // Must follow init: it installs the global this overwrites.
    crate::theme::sync_component_theme(cx);
    crate::platform::text_preferences::sync_component_size(cx);

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
