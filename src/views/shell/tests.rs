mod session_switcher;

use super::*;

fn highlights_fixture() -> grove_core::release_highlights::ReleaseHighlights {
    use grove_core::release_highlights::{
        HighlightMedia, HighlightSlide, MediaKind, ReleaseHighlights,
    };
    ReleaseHighlights {
        version: "99.0.0".into(),
        enabled: false,
        title: "Test highlights".into(),
        slides: vec![HighlightSlide {
            id: "fixture".into(),
            title: "An example improvement".into(),
            description: "A fixture independent of release authoring.".into(),
            media: Some(HighlightMedia {
                kind: MediaKind::Image,
                src: "highlights/test-missing.png".into(),
                alt: "Test preview".into(),
                poster: None,
                captions: None,
                frame: None,
            }),
        }],
    }
}

#[gpui::test]
fn debug_highlights_replay_blocks_commands_without_marking_seen(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    let seen = cx.update(|_, cx| {
        cx.global::<crate::settings::SettingsState>()
            .store
            .seen_highlights_versions
            .clone()
    });
    for _ in 0..2 {
        cx.update(|window, cx| {
            shell.update(cx, |this, cx| {
                this.open_highlights_release(highlights_fixture(), window, cx);
            });
        });
        draw(cx);
        cx.update(|window, cx| {
            assert!(shell.read(cx).highlights.is_some());
            assert!(shell.read(cx).shortcut_blocked(cx));
            window.dispatch_action(Box::new(k::NewSession), cx);
            assert!(!shell.read(cx).launcher.read(cx).is_open());
        });
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|_, cx| {
            assert!(shell.read(cx).highlights.is_none());
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .seen_highlights_versions,
                seen
            );
        });
    }
}

#[gpui::test]
fn startup_highlights_wait_for_settings_backend_and_sidebar_panels(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .seen_highlights_versions
                .push(env!("CARGO_PKG_VERSION").into());
            this.highlights_startup_checked = false;
            this.backend_choice_open = true;
            this.check_startup_highlights(window, cx);
            assert!(!this.highlights_startup_checked);
            assert!(this.highlights.is_none());
            this.backend_choice_open = false;
            this.settings.update(cx, |panel, cx| panel.open(window, cx));
            this.check_startup_highlights(window, cx);
            assert!(!this.highlights_startup_checked);
            this.settings
                .update(cx, |panel, cx| panel.close(window, cx));
            this.sidebar
                .update(cx, |sidebar, cx| sidebar.open_archived_projects(window, cx));
            assert!(this.sidebar.read(cx).highlights_blocked());
            this.check_startup_highlights(window, cx);
            assert!(!this.highlights_startup_checked);
        });
    });
    draw(cx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        shell.update(cx, |this, cx| {
            assert!(!this.sidebar.read(cx).highlights_blocked());
            this.check_startup_highlights(window, cx);
            assert!(this.highlights_startup_checked);
            // Already seen installed releases remain closed after setup completes.
            assert!(this.highlights.is_none());
        });
    });
}

#[gpui::test]
fn zen_shows_one_terminal_and_restores_the_previous_view(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    cx.update(|_, cx| {
        let sidebar = shell.read(cx).sidebar.clone();
        sidebar.update(
            cx,
            crate::views::sidebar::Sidebar::select_project_canvas_for_test,
        );
    });
    draw(cx);
    cx.update(|window, cx| {
        let focus = shell.read(cx).focus.clone();
        focus.focus(window, cx);
        window.dispatch_action(Box::new(k::ToggleZen), cx);
    });
    draw(cx);
    assert!(cx.debug_bounds("app-header").is_some());
    assert!(cx.debug_bounds("statusbar").is_none());
    assert!(cx.debug_bounds("sidebar-rail").is_none());
    assert!(cx.debug_bounds("checkout-management").is_some());
    cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
    draw(cx);
    assert!(cx.debug_bounds("app-header").is_some());
    assert!(cx.debug_bounds("global-navigation").is_some());
    assert!(cx.debug_bounds("statusbar").is_none());
    assert!(cx.debug_bounds("global-navigation").is_some());
    let id = cx.update(|_, cx| {
        let registry = shell.read(cx).runtime.read(cx).registry.clone();
        registry.update(cx, |registry, cx| {
            let id = registry.insert_meta(
                "navigation".into(),
                "/grove-shell-navigation-test".into(),
                grove_core::agent::Agent::Terminal,
            );
            cx.notify();
            id
        })
    });
    draw(cx);
    cx.update(|window, cx| {
        let sidebar = shell.read(cx).sidebar.clone();
        sidebar.update(cx, |sidebar, cx| {
            sidebar.select_session_id(id, window, cx);
        });
        shell.update(cx, |shell, _| shell.backend_choice_open = true);
        window.dispatch_action(Box::new(k::ToggleZen), cx);
        assert!(!shell.read(cx).sidebar.read(cx).is_zen());
    });
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.cancel_backend_choice(window, cx);
        });
    });
    draw(cx);
    cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
    draw(cx);
    assert!(cx.debug_bounds("app-header").is_some());
    assert!(cx.debug_bounds("sidebar-rail").is_none());
    assert!(cx.debug_bounds("terminal-header-1").is_none());
    assert!(cx.debug_bounds("terminal-pane-1").is_some());
    cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
    draw(cx);
    assert!(cx.debug_bounds("app-header").is_some());
    assert!(cx.debug_bounds("global-navigation").is_some());
    assert!(cx.debug_bounds("global-navigation").is_some());
    assert!(cx.debug_bounds("terminal-header-1").is_some());
    cx.update(|window, cx| {
        let focus = shell.read(cx).focus.clone();
        focus.focus(window, cx);
        window.dispatch_action(Box::new(k::ToggleGrid), cx);
    });
    draw(cx);
    assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
    cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
    draw(cx);
    assert!(cx.debug_bounds("app-header").is_some());
    assert!(cx.debug_bounds("sidebar-rail").is_none());
    assert!(cx.debug_bounds("terminal-pane-1").is_some());
    cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
    draw(cx);
    assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
    assert!(cx.debug_bounds("app-header").is_some());
    assert!(cx.debug_bounds("global-navigation").is_some());
    cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
    draw(cx);
    cx.update(|_, cx| {
        let registry = shell.read(cx).runtime.read(cx).registry.clone();
        registry.update(cx, |registry, cx| {
            registry.remove(id);
            cx.notify();
        });
    });
    draw(cx);
    assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_zen()));
    assert!(cx.debug_bounds("session-grid-layout").is_none());
    assert!(cx.debug_bounds("projects-destination").is_none());
    cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
    draw(cx);
    assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
}

#[gpui::test]
fn empty_grid_zen_shows_one_overview_and_returns_to_grid(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    cx.update(|_, cx| {
        let sidebar = shell.read(cx).sidebar.clone();
        sidebar.update(
            cx,
            crate::views::sidebar::Sidebar::select_project_canvas_for_test,
        );
    });
    draw(cx);
    cx.update(|window, cx| {
        let focus = shell.read(cx).focus.clone();
        focus.focus(window, cx);
        window.dispatch_action(Box::new(k::ToggleGrid), cx);
    });
    draw(cx);
    assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
    cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
    draw(cx);
    assert!(cx.debug_bounds("app-header").is_some());
    assert!(cx.debug_bounds("checkout-management").is_some());
    assert!(cx.debug_bounds("session-grid-layout").is_none());
    cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
    draw(cx);
    assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
    assert!(cx.debug_bounds("app-header").is_some());
    assert!(cx.debug_bounds("global-navigation").is_some());
}

fn init(cx: &mut App) {
    gpui_component::init(cx);
    // Shell navigation checks measure stable endpoints; motion has owner-level coverage.
    cx.set_reduce_motion(true);
    let project = grove_core::storage::Project {
        name: "navigation".into(),
        path: "/grove-shell-navigation-test".into(),
        scripts: grove_core::storage::ProjectScripts::default(),
        archived: false,
        worktree_dir: None,
    };
    cx.set_global(crate::settings::SettingsState::new(
        grove_core::storage::Store {
            projects: vec![project],
            tmux_enabled: Some(false),
            ..grove_core::storage::Store::default()
        },
    ));
    cx.set_global(crate::zoom::ZoomState::new(1.0));
    cx.set_global(crate::zoom::CurrentPtyDims::default());
    cx.set_global(ThemeState::new(
        false,
        "tokyonight".into(),
        "tokyonight-day".into(),
    ));
}

fn init_projects(cx: &mut App) {
    init(cx);
    cx.global_mut::<crate::settings::SettingsState>()
        .store
        .default_landing_page = grove_core::storage::LandingPage::Projects;
}

fn draw(cx: &mut gpui::VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

#[gpui::test]
fn new_session_opens_main_screen_preserving_draft(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    let composer = cx.debug_bounds("codex-composer-input").unwrap();
    cx.simulate_click(composer.center(), gpui::Modifiers::default());
    cx.simulate_input("Keep my New session draft");
    cx.update(|window, cx| shell.update(cx, |shell, cx| shell.open_new_session(window, cx)));
    draw(cx);
    assert!(cx.debug_bounds("dashboard-summary").is_some());
    assert!(cx.debug_bounds("worktree-launcher-panel").is_none());
    cx.update(|_, cx| {
        let shell = shell.read(cx);
        assert_eq!(
            shell
                .sidebar
                .read(cx)
                .dashboard_draft_for_test(cx)
                .as_deref(),
            Some("Keep my New session draft")
        );
        assert_eq!(shell.runtime.read(cx).registry.read(cx).len(), 0);
    });
}

#[gpui::test]
fn new_session_landing_opens_main_screen(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        init(cx);
        cx.global_mut::<crate::settings::SettingsState>()
            .store
            .default_landing_page = grove_core::storage::LandingPage::NewSession;
    });
    let (_, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    assert!(cx.debug_bounds("worktree-launcher-panel").is_none());
    assert!(cx.debug_bounds("dashboard-summary").is_some());
}

#[gpui::test]
fn numbered_destinations_route_and_respect_blocking_overlays(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    cx.update(|cx| cx.bind_keys(k::shell_bindings()));
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    for (key, destination) in [("2", "history-list-scroll"), ("1", "dashboard-page")] {
        cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
        });
        cx.simulate_keystrokes(&format!("{}{key}", k::platform_mod_prefix()));
        draw(cx);
        assert!(
            cx.debug_bounds(destination).is_some(),
            "{key} must open {destination}"
        );
    }
    cx.simulate_keystrokes("escape escape");
    draw(cx);
    cx.update(|window, cx| window.dispatch_action(Box::new(k::Settings), cx));
    draw(cx);
    cx.simulate_keystrokes(&format!("{}2", k::platform_mod_prefix()));
    draw(cx);
    assert!(cx.update(|_, cx| shell.read(cx).settings.read(cx).is_open()));
    assert!(cx.debug_bounds("projects-destination").is_none());
    assert!(cx.debug_bounds("statusbar").is_none());
    cx.update(|window, cx| {
        shell
            .read(cx)
            .settings
            .clone()
            .update(cx, |settings, cx| settings.close(window, cx));
    });
    draw(cx);
    cx.update(|window, cx| {
        let launcher = shell.read(cx).launcher.clone();
        launcher.update(cx, |launcher, cx| launcher.open(window, cx));
    });
    draw(cx);
    cx.simulate_keystrokes(&format!("{}3", k::platform_mod_prefix()));
    draw(cx);
    assert!(cx.update(|_, cx| shell.read(cx).launcher.read(cx).is_open()));
    assert!(cx.debug_bounds("session-history-page").is_none());
}

#[gpui::test]
fn backend_choice_waits_for_launch_then_back_restores_shortcuts(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        init(cx);
        cx.global_mut::<crate::settings::SettingsState>()
            .store
            .tmux_enabled = None;
    });
    let (shell, cx) = cx.add_window_view(Shell::new);
    assert!(!cx.update(|_, cx| shell.read(cx).backend_choice_open));
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.backend_choice_open = true;
            cx.notify();
        });
        // A requested choice keeps keyboard input inside the dialog.
        let handle = shell.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        assert_eq!(
            window.focused(cx),
            Some(shell.read(cx).backend_choice_focus.clone())
        );
    });
    draw(cx);
    assert!(cx.debug_bounds("backend-choice-dialog").is_some());
    assert!(cx.debug_bounds("backend-choice-option-0").is_some());
    assert!(cx.debug_bounds("backend-choice-option-1").is_some());
    cx.simulate_keystrokes("tab");
    draw(cx);
    cx.update(|_, cx| assert_eq!(shell.read(cx).backend_choice_index, 1));
    cx.update(|window, cx| {
        assert!(shell.read(cx).backend_choice_open);
        assert!(shell.read(cx).shortcut_blocked(cx));
        window.dispatch_action(Box::new(k::NewSession), cx);
        assert!(!shell.read(cx).launcher.read(cx).is_open());
    });
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|_, cx| {
        assert!(!shell.read(cx).backend_choice_open);
        assert!(!shell.read(cx).shortcut_blocked(cx));
    });
}

#[gpui::test]
fn selecting_backend_does_not_start_or_save_before_confirmation(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        init(cx);
        cx.global_mut::<crate::settings::SettingsState>()
            .store
            .tmux_enabled = None;
    });
    let (shell, cx) = cx.add_window_view(Shell::new);
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| {
            shell.backend_choice_open = true;
            shell.backend_choice_focus.focus(window, cx);
            cx.notify();
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("backend-choice-start").is_some());
    let tmux = cx.debug_bounds("backend-choice-option-1").unwrap().center();
    cx.simulate_mouse_down(tmux, MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_up(tmux, MouseButton::Left, gpui::Modifiers::default());
    draw(cx);
    cx.update(|_, cx| {
        let shell = shell.read(cx);
        assert!(shell.backend_choice_open);
        assert!(shell.backend_selected_tmux);
        assert_eq!(
            cx.global::<crate::settings::SettingsState>()
                .store
                .tmux_enabled,
            None
        );
        assert!(shell.runtime.read(cx).registry.read(cx).is_empty());
    });
}

fn window_toggle_id() -> &'static str {
    if cfg!(target_os = "macos") {
        "window-fullscreen"
    } else {
        "window-maximize"
    }
}
#[gpui::test]
fn header_controls_follow_platform_order_without_overlap(cx: &mut gpui::TestAppContext) {
    cx.update(init_projects);
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    let close = cx.debug_bounds("window-close").unwrap();
    let minimize = cx.debug_bounds("window-minimize").unwrap();
    let toggle = cx.debug_bounds(window_toggle_id()).unwrap();
    let sidebar = cx.debug_bounds("sidebar-toggle").unwrap();
    let header = cx.debug_bounds("app-header").unwrap();
    if cfg!(target_os = "macos") {
        assert!(close.right() <= minimize.left());
        assert!(minimize.right() <= toggle.left());
        assert!(toggle.right() <= sidebar.left());
        assert!(cx.debug_bounds("window-maximize").is_none());
    } else {
        assert!(sidebar.right() <= minimize.left());
        assert!(minimize.right() <= toggle.left());
        assert!(toggle.right() <= close.left());
        let window_header = cx.debug_bounds("window-header").unwrap();
        assert!(close.left() >= header.right());
        assert!((f32::from(window_header.right() - close.right()) - SPACE_LG).abs() <= 1.);
        assert_eq!(
            window_header.right(),
            cx.update(|window, _| window.viewport_size().width)
        );
        assert!(cx.debug_bounds("window-fullscreen").is_none());
    }
    let ids: &[&str] = if cfg!(target_os = "macos") {
        &["window-close", "window-minimize", "window-fullscreen"]
    } else {
        &[
            "sidebar-toggle",
            "window-minimize",
            "window-maximize",
            "window-close",
        ]
    };
    cx.update(|window, cx| {
        shell.read(cx).header_focus[ids[0]]
            .clone()
            .focus(window, cx);
    });
    for id in &ids[1..] {
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| {
            assert!(shell.read(cx).header_focus[id].is_focused(window), "{id}");
        });
    }
    for id in ids[..ids.len() - 1].iter().rev() {
        cx.simulate_keystrokes("shift-tab");
        draw(cx);
        cx.update(|window, cx| {
            assert!(shell.read(cx).header_focus[id].is_focused(window), "{id}");
        });
    }
}
#[gpui::test]
fn header_controls_support_keyboard_and_restore_settings_source(cx: &mut gpui::TestAppContext) {
    cx.update(init_projects);
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    // TestWindow implements fullscreen but not minimize/zoom. Exercise the
    // shared keyboard handler through the sidebar button on other targets;
    // real maximize/minimize needs native platform evidence.
    if cfg!(target_os = "macos") {
        cx.update(|window, cx| {
            shell.read(cx).header_focus["window-fullscreen"]
                .clone()
                .focus(window, cx);
        });
        cx.simulate_keystrokes("enter");
        draw(cx);
        cx.update(|window, _| assert!(window.is_fullscreen()));
        cx.simulate_keystrokes("space");
        draw(cx);
        cx.update(|window, _| assert!(!window.is_fullscreen()));
    } else {
        let initial = cx.update(|window, cx| {
            shell.read(cx).header_focus["sidebar-toggle"]
                .clone()
                .focus(window, cx);
            cx.global::<crate::settings::SettingsState>()
                .store
                .sidebar_collapsed
        });
        cx.simulate_keystrokes("enter");
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_collapsed,
                !initial
            );
        });
        // Collapsing navigation deliberately restores content focus.
        cx.update(|window, cx| {
            shell.read(cx).header_focus["sidebar-toggle"]
                .clone()
                .focus(window, cx);
        });
        cx.simulate_keystrokes("space");
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_collapsed,
                initial
            );
        });
    }
    let settings = cx.debug_bounds("sidebar-settings").unwrap();
    cx.simulate_mouse_down(
        settings.center(),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    let source = cx.update(|window, cx| window.focused(cx).expect("settings source focus"));
    cx.simulate_mouse_up(
        settings.center(),
        MouseButton::Left,
        gpui::Modifiers::default(),
    );
    draw(cx);
    assert!(cx.debug_bounds("settings-panel").is_some());
    let collapsed = cx.update(|_, cx| {
        cx.global::<crate::settings::SettingsState>()
            .store
            .sidebar_collapsed
    });
    let toggle = cx.debug_bounds("sidebar-toggle").unwrap();
    cx.simulate_click(toggle.center(), gpui::Modifiers::default());
    draw(cx);
    cx.update(|_, cx| {
        assert_eq!(
            cx.global::<crate::settings::SettingsState>()
                .store
                .sidebar_collapsed,
            collapsed
        );
    });
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        assert!(!shell.read(cx).settings.read(cx).is_open());
        assert!(source.is_focused(window));
    });
}

#[gpui::test]
fn sidebar_collapse_shortcut_toggles_and_respects_blocking(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        init_projects(cx);
        cx.bind_keys(k::shell_bindings());
        cx.global_mut::<crate::settings::SettingsState>()
            .store
            .sidebar_width = Some(300.0);
    });
    let (shell, cx) = cx.add_window_view(Shell::new);
    cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
    draw(cx);
    let shortcut = k::SHORTCUTS
        .iter()
        .find(|def| def.action == Some(k::GlobalShortcut::ToggleSidebar))
        .expect("sidebar shortcut registry row");
    let chord = k::keystrokes_for(shortcut).remove(0);
    cx.update(|window, cx| {
        let focus = shell.read(cx).focus.clone();
        focus.focus(window, cx);
    });
    cx.simulate_keystrokes(&chord);
    draw(cx);
    assert!(cx.debug_bounds("sidebar-rail").is_none());
    assert!(cx.debug_bounds("global-navigation").is_none());
    assert!(cx.debug_bounds("app-header").is_some());
    assert!(cx.debug_bounds("sidebar-settings").is_none());
    assert!(cx.debug_bounds("window-close").is_some());
    assert!(cx.debug_bounds("sidebar-divider").is_none());
    cx.update(|_, cx| {
        let settings = &cx.global::<crate::settings::SettingsState>().store;
        assert!(settings.sidebar_collapsed);
        assert_eq!(settings.sidebar_width, Some(300.0));
    });
    cx.simulate_keystrokes(&chord);
    draw(cx);
    assert_rail_alignment(cx, 300.0);
    cx.update(|window, cx| {
        assert!(
            !cx.global::<crate::settings::SettingsState>()
                .store
                .sidebar_collapsed
        );
        let settings = shell.read(cx).settings.clone();
        settings.update(cx, |settings, cx| settings.open(window, cx));
        assert!(shell.read(cx).shortcut_blocked(cx));
    });
    draw(cx);
    cx.simulate_keystrokes(&chord);
    draw(cx);
    cx.update(|window, cx| {
        assert!(
            !cx.global::<crate::settings::SettingsState>()
                .store
                .sidebar_collapsed
        );
        window.dispatch_action(Box::new(k::ToggleSidebar), cx);
        assert!(
            !cx.global::<crate::settings::SettingsState>()
                .store
                .sidebar_collapsed
        );
        assert_eq!(
            cx.global::<crate::settings::SettingsState>()
                .store
                .sidebar_width,
            Some(300.0)
        );
    });
}

#[gpui::test]
fn sidebar_collapse_restores_width_and_header_controls(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        init_projects(cx);
        cx.global_mut::<crate::settings::SettingsState>()
            .store
            .sidebar_width = Some(300.);
    });
    let (_, cx) = cx.add_window_view(Shell::new);
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    draw(cx);
    assert_rail_alignment(cx, 300.);
    for _ in 0..2 {
        let toggle = cx.debug_bounds("sidebar-toggle").unwrap();
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("global-navigation").is_none());
        assert!(cx.debug_bounds("app-header").is_some());
        assert_eq!(
            f32::from(cx.debug_bounds("sidebar-canvas").unwrap().left()),
            0.
        );
        for id in [
            "window-close",
            "window-minimize",
            window_toggle_id(),
            "sidebar-toggle",
        ] {
            assert!(cx.debug_bounds(id).is_some(), "{id}");
        }
        let toggle = cx.debug_bounds("sidebar-toggle").unwrap();
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        draw(cx);
        assert_rail_alignment(cx, 300.);
    }
    cx.update(|_, cx| {
        assert_eq!(
            cx.global::<crate::settings::SettingsState>()
                .store
                .sidebar_width,
            Some(300.)
        );
    });
}

#[gpui::test]
fn global_home_control_opens_composer_while_settings_open(cx: &mut gpui::TestAppContext) {
    cx.update(init_projects);
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    cx.update(|window, cx| {
        shell
            .read(cx)
            .sidebar
            .clone()
            .update(cx, |sidebar, cx| sidebar.toggle_grid(window, cx));
        shell
            .read(cx)
            .settings
            .clone()
            .update(cx, |settings, cx| settings.open(window, cx));
    });
    draw(cx);
    assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
    assert!(cx.debug_bounds("sidebar-rail").is_none());
    let projects = cx.debug_bounds("global-nav-home").unwrap();
    cx.simulate_click(projects.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(
        cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()),
        "Home preserves the preferred session view"
    );
    assert!(cx.debug_bounds("dashboard-page").is_some());
    assert!(!cx.update(|_, cx| shell.read(cx).settings.read(cx).is_open()));
    assert_rail_alignment(cx, 260.);
}

#[gpui::test]
fn context_workspace_header_opens_picker_and_escape_preserves_destination(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(init_projects);
    let (_, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    let trigger = cx.debug_bounds("header-workspace").unwrap();
    let rail = cx.debug_bounds("global-navigation").unwrap();
    assert!(rail.contains(&trigger.center()));
    cx.simulate_click(trigger.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(cx.debug_bounds("workspace-popup").is_some());
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(cx.debug_bounds("workspace-popup").is_none());
    assert_eq!(cx.debug_bounds("global-navigation").unwrap(), rail);
    assert!(cx.debug_bounds("global-nav-history").is_some());
}

#[gpui::test]
fn home_content_respects_platform_window_header(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (_, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    let rail = cx.debug_bounds("global-navigation").unwrap();
    let canvas = cx.debug_bounds("sidebar-canvas").unwrap();
    let header = cx.debug_bounds("app-header").unwrap();
    assert_eq!(rail.top(), header.top());
    assert_eq!(
        canvas.top(),
        if cfg!(target_os = "macos") {
            header.top()
        } else {
            header.bottom()
        }
    );
    assert_eq!(canvas.bottom(), rail.bottom());
    assert!(header.right() <= canvas.left());
    assert_eq!(canvas.left(), rail.right());
    let workspace = cx
        .debug_bounds("header-workspace")
        .expect("shared workspace access");
    assert!(rail.contains(&workspace.center()));
    cx.simulate_click(workspace.center(), gpui::Modifiers::default());
    draw(cx);
    let popup = cx
        .debug_bounds("workspace-popup")
        .expect("workspace picker");
    assert!((f32::from(popup.left() - workspace.left())).abs() <= SPACE_LG);
    assert!(popup.top() >= workspace.bottom());
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(cx.debug_bounds("workspace-popup").is_none());
    for selector in [
        "window-close",
        "window-minimize",
        window_toggle_id(),
        "sidebar-toggle",
        "sidebar-settings",
    ] {
        assert!(cx.debug_bounds(selector).is_some(), "{selector}");
    }
    assert!(cx.debug_bounds("context-workspace").is_none());
    assert!(cx.debug_bounds("global-nav-workspace").is_none());
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn mac_titlebar_drag_area_spans_empty_top_space(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    for (width, zoom, collapsed) in [
        (1280., 1.0, false),
        (1280., 1.5, false),
        (1280., 1.0, true),
        (500., 1.5, true),
    ] {
        cx.update(|_, cx| {
            cx.set_global(crate::zoom::ZoomState::new(zoom));
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .sidebar_collapsed = collapsed;
        });
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(600.)));
        draw(cx);

        let drag = cx
            .debug_bounds("mac-titlebar-drag-region")
            .expect("macOS window drag area");
        let header = cx.debug_bounds("app-header").expect("app header");
        assert_eq!(drag.top(), header.top());
        assert!(drag.left() > header.left());
        assert!(drag.size.width >= gpui::px(CHROME_CONTROL_H * 3.0 * zoom));
        assert!(drag.size.height >= gpui::px(SPACE_SM * zoom));
        for selector in [
            "window-close",
            "window-minimize",
            window_toggle_id(),
            "header-workspace",
            "sidebar-toggle",
            "sidebar-divider",
        ] {
            if let Some(control) = cx.debug_bounds(selector) {
                assert!(!drag.intersects(&control), "{selector} overlaps {drag:?}");
            }
        }
        if let Some(rail) = cx.debug_bounds("global-navigation") {
            assert!(!collapsed);
            assert!(drag.right() < rail.right());
            assert!(drag.contains(&gpui::point(rail.center().x, drag.center().y)));
        } else {
            assert!(collapsed);
            let toggle = cx.debug_bounds("sidebar-toggle").expect("sidebar toggle");
            assert!(drag.right() <= toggle.right());
        }
    }

    let sidebar = cx.update(|_, cx| shell.read(cx).sidebar.clone());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.toggle_zen(window, cx));
    });
    draw(cx);
    assert!(cx.update(|_, cx| sidebar.read(cx).is_zen()));
    assert!(cx.debug_bounds("mac-titlebar-drag-region").is_none());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.toggle_zen(window, cx);
            sidebar.toggle_grid(window, cx);
        });
    });
    draw(cx);
    assert!(cx.update(|_, cx| sidebar.read(cx).is_grid()));
    assert!(cx.debug_bounds("mac-titlebar-drag-region").is_none());
}

fn reopen_sidebar_if_fitted(cx: &mut gpui::VisualTestContext) {
    if cx.debug_bounds("global-navigation").is_none() {
        let toggle = cx
            .debug_bounds("sidebar-toggle")
            .expect("sidebar reopen control");
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        draw(cx);
    }
}

fn assert_rail_alignment(cx: &mut gpui::VisualTestContext, expected: f32) {
    let rail = cx
        .debug_bounds("global-navigation")
        .expect("single sidebar");
    let divider = cx.debug_bounds("sidebar-divider").expect("resize hit zone");
    let canvas = cx.debug_bounds("sidebar-canvas").expect("canvas");
    let header = cx.debug_bounds("app-header").expect("window header");
    assert!((f32::from(rail.size.width) - expected).abs() <= 1.0);
    assert_eq!(rail.left(), gpui::px(0.));
    assert_eq!(rail.top(), header.top());
    assert_eq!(
        canvas.top(),
        if cfg!(target_os = "macos") {
            header.top()
        } else {
            header.bottom()
        }
    );
    assert_eq!(canvas.bottom(), rail.bottom());
    assert_eq!(canvas.left(), rail.right());
    assert!((f32::from(divider.right() - rail.right())).abs() <= 1.0);
    assert!(divider.left() >= rail.left());
}

#[gpui::test]
fn sidebar_drag_clamps_persists_on_release_and_survives_reconstruction(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(init_projects);
    let (_, cx) = cx.add_window_view(Shell::new);
    cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
    draw(cx);
    assert_rail_alignment(cx, 260.0);
    let edge = cx.debug_bounds("sidebar-divider").unwrap().center();
    cx.simulate_mouse_down(edge, MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_up(edge, MouseButton::Left, gpui::Modifiers::default());
    cx.update(|_, cx| {
        assert_eq!(
            cx.global::<crate::settings::SettingsState>()
                .store
                .sidebar_width,
            None
        );
    });

    cx.simulate_mouse_down(edge, MouseButton::Left, gpui::Modifiers::default());
    let far_right = gpui::point(gpui::px(1100.0), edge.y);
    cx.simulate_mouse_move(far_right, MouseButton::Left, gpui::Modifiers::default());
    draw(cx);
    assert_rail_alignment(cx, 640.0);
    cx.update(|_, cx| {
        assert_eq!(
            cx.global::<crate::settings::SettingsState>()
                .store
                .sidebar_width,
            None
        );
    });
    cx.simulate_mouse_up(far_right, MouseButton::Left, gpui::Modifiers::default());
    draw(cx);
    cx.update(|_, cx| {
        assert_eq!(
            cx.global::<crate::settings::SettingsState>()
                .store
                .sidebar_width,
            Some(640.0)
        );
    });

    let edge = cx.debug_bounds("sidebar-divider").unwrap().center();
    cx.simulate_mouse_down(edge, MouseButton::Left, gpui::Modifiers::default());
    let far_left = gpui::point(gpui::px(30.0), edge.y);
    cx.simulate_mouse_move(far_left, MouseButton::Left, gpui::Modifiers::default());
    draw(cx);
    assert_rail_alignment(cx, 220.0);
    cx.simulate_mouse_up(far_left, MouseButton::Left, gpui::Modifiers::default());
    draw(cx);
    cx.update(|_, cx| {
        assert_eq!(
            cx.global::<crate::settings::SettingsState>()
                .store
                .sidebar_width,
            Some(220.0)
        );
    });

    let (_, cx) = cx.add_window_view(Shell::new);
    cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
    draw(cx);
    assert_rail_alignment(cx, 220.0);
    let home = cx.debug_bounds("global-nav-home").unwrap();
    cx.simulate_click(home.center(), gpui::Modifiers::default());
    draw(cx);
    assert_rail_alignment(cx, 220.0);
    let projects = cx.debug_bounds("global-nav-history").unwrap();
    cx.simulate_click(projects.center(), gpui::Modifiers::default());
    draw(cx);
    assert_rail_alignment(cx, 220.0);
}

#[gpui::test]
fn sidebar_narrow_cap_and_double_click_reset_keep_saved_preference(cx: &mut gpui::TestAppContext) {
    cx.update(init_projects);
    cx.update(|cx| {
        cx.global_mut::<crate::settings::SettingsState>()
            .store
            .sidebar_width = Some(500.0);
    });
    let (_, cx) = cx.add_window_view(Shell::new);
    cx.simulate_resize(gpui::size(gpui::px(500.0), gpui::px(800.0)));
    draw(cx);
    reopen_sidebar_if_fitted(cx);
    assert_rail_alignment(cx, 200.0);
    cx.update(|_, cx| {
        assert_eq!(
            cx.global::<crate::settings::SettingsState>()
                .store
                .sidebar_width,
            Some(500.0)
        );
    });
    cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
    draw(cx);
    assert_rail_alignment(cx, 500.0);

    cx.update(|_, cx| cx.global_mut::<crate::zoom::ZoomState>().zoom = 1.5);
    cx.simulate_resize(gpui::size(gpui::px(1500.0), gpui::px(800.0)));
    draw(cx);
    reopen_sidebar_if_fitted(cx);
    assert_rail_alignment(cx, 750.0);
    let edge = cx.debug_bounds("sidebar-divider").unwrap().center();
    cx.simulate_mouse_down(edge, MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_up(edge, MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_event(gpui::MouseDownEvent {
        position: edge,
        modifiers: gpui::Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(gpui::MouseUpEvent {
        position: edge,
        modifiers: gpui::Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
    });
    draw(cx);
    cx.update(|_, cx| {
        assert_eq!(
            cx.global::<crate::settings::SettingsState>()
                .store
                .sidebar_width,
            Some(260.0)
        );
    });
    assert_rail_alignment(cx, 390.0);
}

#[gpui::test]
fn header_grid_control_toggles_and_respects_blocking(cx: &mut gpui::TestAppContext) {
    cx.update(init_projects);
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    let grid = cx.debug_bounds("sidebar-grid").unwrap();
    let settings = cx.debug_bounds("sidebar-settings").unwrap();
    let rail = cx.debug_bounds("global-navigation").unwrap();
    assert!(rail.contains(&grid.center()));
    assert!(rail.contains(&settings.center()));
    assert!(grid.bottom() < settings.top());
    cx.simulate_click(grid.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
    cx.update(|window, cx| assert!(window.focused(cx).is_some()));
    cx.simulate_keystrokes("enter");
    draw(cx);
    assert!(!cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
    cx.simulate_click(settings.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(cx.update(|_, cx| shell.read(cx).settings.read(cx).is_open()));
    cx.simulate_click(grid.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(!cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()),
            "Grid navigation changed while Settings was open: settings_open={}, shell_blocked={}, sidebar_navigation_available={}",
            cx.update(|_, cx| shell.read(cx).settings.read(cx).is_open()),
            cx.update(|_, cx| shell.read(cx).shortcut_blocked(cx)),
            cx.update(|_, cx| shell.read(cx).sidebar.read(cx).navigation_available()));
    assert!(cx.update(|_, cx| shell.read(cx).settings.read(cx).is_open()));
}

#[gpui::test]
fn settings_stays_in_sidebar_footer_across_destination_changes(cx: &mut gpui::TestAppContext) {
    cx.update(init_projects);
    let (_, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    for destination in ["global-nav-home", "global-nav-history"] {
        let target = cx.debug_bounds(destination).expect("destination");
        cx.simulate_click(target.center(), gpui::Modifiers::default());
        draw(cx);
        let header = cx.debug_bounds("app-header").unwrap();
        let settings = cx.debug_bounds("sidebar-settings").unwrap();
        let rail = cx.debug_bounds("global-navigation").unwrap();
        assert!(rail.contains(&settings.center()));
        assert!(settings.top() > header.bottom());
        assert_eq!(
            cx.debug_bounds("sidebar-canvas").unwrap().top(),
            if cfg!(target_os = "macos") {
                header.top()
            } else {
                header.bottom()
            }
        );
    }
}

#[gpui::test]
fn ordinary_sidebar_controls_traverse_forward_and_backward(cx: &mut gpui::TestAppContext) {
    cx.update(init_projects);
    let (_, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    let project = cx
        .debug_bounds("global-nav-history")
        .expect("History navigation")
        .center();
    cx.simulate_mouse_down(project, MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_up(project, MouseButton::Left, gpui::Modifiers::default());
    draw(cx);
    let initial = cx.update(|window, cx| window.focused(cx).expect("project focus"));
    cx.simulate_keystrokes("tab");
    draw(cx);
    let next = cx.update(|window, cx| window.focused(cx).expect("next control"));
    assert_ne!(initial, next);
    cx.simulate_keystrokes("shift-tab");
    draw(cx);
    cx.update(|window, cx| assert_eq!(window.focused(cx), Some(initial)));
}

#[gpui::test]
fn window_header_and_picker_remain_stable_in_fullscreen(cx: &mut gpui::TestAppContext) {
    cx.update(init_projects);
    let (_, cx) = cx.add_window_view(Shell::new);
    for zoom in [1.0, 1.5] {
        cx.update(|_, cx| cx.set_global(crate::zoom::ZoomState::new(zoom)));
        for width in [1280., 500., 320.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(600.)));
            draw(cx);
            reopen_sidebar_if_fitted(cx);
            let mut previous = None;
            for fullscreen in [false, true, false] {
                cx.update(|window, _| {
                    if window.is_fullscreen() != fullscreen {
                        window.toggle_fullscreen();
                    }
                });
                draw(cx);
                let header = cx.debug_bounds("app-header").unwrap();
                let rail = cx.debug_bounds("global-navigation").unwrap();
                let canvas = cx.debug_bounds("sidebar-canvas").unwrap();
                let picker = cx.debug_bounds("header-workspace");
                let settings = cx.debug_bounds("sidebar-settings").unwrap();
                let grid = cx.debug_bounds("sidebar-grid").unwrap();
                assert_eq!(rail.top(), header.top());
                assert_eq!(
                    canvas.top(),
                    if cfg!(target_os = "macos") {
                        header.top()
                    } else {
                        header.bottom()
                    }
                );
                assert_eq!(canvas.bottom(), rail.bottom());
                assert!(header.right() <= canvas.left());
                if width / zoom >= 400. {
                    assert!(header.contains(&picker.expect("workspace picker").center()));
                } else {
                    assert!(
                        picker.is_none(),
                        "compact header reserves its fixed controls"
                    );
                }
                assert!(rail.contains(&settings.center()),
                        "Settings is outside the sidebar at {width}px/{zoom}x: rail={rail:?}, settings={settings:?}");
                assert!(rail.contains(&grid.center()));
                assert!(grid.bottom() < settings.top());
                for selector in [
                    "window-close",
                    "window-minimize",
                    window_toggle_id(),
                    "sidebar-toggle",
                ] {
                    assert!(cx.debug_bounds(selector).is_some(), "{selector}");
                }
                if !cfg!(target_os = "macos") {
                    let bar = cx.debug_bounds("window-header").unwrap();
                    let close = cx.debug_bounds("window-close").unwrap();
                    let minimize = cx.debug_bounds("window-minimize").unwrap();
                    let sidebar = cx.debug_bounds("sidebar-toggle").unwrap();
                    assert_eq!(bar.right(), gpui::px(width));
                    assert!((f32::from(bar.right() - close.right()) - SPACE_LG * zoom).abs() <= 1.);
                    assert!(minimize.left() >= sidebar.right());
                    assert!(canvas.top() >= bar.bottom());
                }
                let geometry = (header, rail, canvas, picker, settings, grid);
                if let Some(prior) = previous {
                    assert_eq!(
                        geometry, prior,
                        "fullscreen shifted header at {width}px/{zoom}x"
                    );
                }
                previous = Some(geometry);
            }
        }
    }
}

#[gpui::test]
fn sidebar_workspace_control_traverses_and_activates(cx: &mut gpui::TestAppContext) {
    cx.update(init_projects);
    let (_, cx) = cx.add_window_view(Shell::new);
    for zoom in [1.0, 1.5] {
        cx.update(|_, cx| cx.set_global(crate::zoom::ZoomState::new(zoom)));
        for width in [1280., 500., 320.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(600.)));
            draw(cx);
            reopen_sidebar_if_fitted(cx);
            let rail = cx.debug_bounds("global-navigation").unwrap();
            let control = cx.debug_bounds("header-workspace");
            if width / zoom >= 400. {
                let control = control.expect("workspace picker");
                assert!(rail.contains(&control.center()));
                assert!(control.bottom() <= cx.debug_bounds("global-nav-home").unwrap().top());
            } else {
                assert!(control.is_none());
            }
        }
    }
    cx.update(|_, cx| cx.set_global(crate::zoom::ZoomState::new(1.0)));
    cx.simulate_resize(gpui::size(gpui::px(500.), gpui::px(600.)));
    draw(cx);
    reopen_sidebar_if_fitted(cx);
    let trigger = cx.debug_bounds("header-workspace").unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(cx.debug_bounds("workspace-popup").is_some());
    cx.simulate_keystrokes("escape");
    draw(cx);
    let source_focus = cx.update(|window, cx| window.focused(cx).expect("workspace trigger focus"));
    cx.simulate_keystrokes("tab");
    draw(cx);
    cx.update(|window, cx| assert_ne!(window.focused(cx), Some(source_focus.clone())));
    cx.simulate_keystrokes("shift-tab");
    draw(cx);
    cx.update(|window, cx| assert_eq!(window.focused(cx), Some(source_focus.clone())));
    for key in ["enter", "space"] {
        cx.simulate_keystrokes(key);
        draw(cx);
        let popup = cx.debug_bounds("workspace-popup").expect("workspace popup");
        assert_eq!(popup.left(), trigger.left());
        assert!(popup.top() >= trigger.bottom());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.debug_bounds("workspace-popup").is_none());
        cx.update(|window, cx| assert_eq!(window.focused(cx), Some(source_focus.clone())));
    }
}

#[gpui::test]
fn worktree_inputs_tab_in_order_then_reach_cancel(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        init(cx);
        cx.global_mut::<crate::settings::SettingsState>()
            .store
            .projects[0]
            .path = env!("CARGO_MANIFEST_DIR").into();
    });
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    cx.update(|window, cx| {
        let sidebar = shell.read(cx).sidebar.clone();
        sidebar.update(cx, |sidebar, cx| {
            sidebar.manage_project_checkouts(env!("CARGO_MANIFEST_DIR"), window, cx);
        });
    });
    draw(cx);
    let add = cx.debug_bounds("checkouts-add").expect("New worktree");
    cx.simulate_click(add.center(), gpui::Modifiers::default());
    draw(cx);
    let mut fields = Vec::new();
    for selector in [
        "worktree-name-field",
        "worktree-branch-field",
        "worktree-base-field",
    ] {
        let bounds = cx.debug_bounds(selector).expect("worktree field");
        let point = gpui::point(bounds.center().x, bounds.bottom() - gpui::px(SPACE_LG));
        cx.simulate_mouse_down(point, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(point, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        fields.push(cx.update(|window, cx| window.focused(cx).expect("input focus")));
    }
    assert_ne!(fields[0], fields[1]);
    assert_ne!(fields[1], fields[2]);
    cx.update(|window, cx| fields[0].focus(window, cx));
    for expected in &fields[1..] {
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| assert_eq!(window.focused(cx), Some(expected.clone())));
    }
    cx.simulate_keystrokes("shift-tab");
    draw(cx);
    cx.update(|window, cx| assert_eq!(window.focused(cx), Some(fields[1].clone())));
    cx.simulate_keystrokes("tab");
    draw(cx);
    cx.update(|window, cx| assert_eq!(window.focused(cx), Some(fields[2].clone())));
    cx.simulate_keystrokes("tab");
    draw(cx);
    cx.update(|window, cx| {
        assert_ne!(
            window.focused(cx),
            Some(fields[2].clone()),
            "Tab leaves base for Cancel"
        );
    });
    cx.simulate_keystrokes("enter");
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
    });
    draw(cx);
    assert!(
        cx.debug_bounds("worktree-name-field").is_none(),
        "Cancel closes the worktree editor; disabled Create is skipped"
    );
}

#[gpui::test]
fn footer_is_absent_and_transient_toast_does_not_reserve_layout(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    cx.simulate_resize(gpui::size(
        gpui::px(crate::WINDOW_MIN_W),
        gpui::px(crate::WINDOW_MIN_H),
    ));
    draw(cx);
    assert!(cx.debug_bounds("statusbar").is_none());
    let canvas = cx.debug_bounds("sidebar-canvas").unwrap();
    assert_eq!(f32::from(canvas.bottom()), crate::WINDOW_MIN_H);
    cx.update(|_, cx| {
        shell
            .read(cx)
            .runtime
            .read(cx)
            .toast
            .clone()
            .update(cx, |toast, cx| toast.set_error("Example failure", cx));
    });
    draw(cx);
    let toast = cx.debug_bounds("app-toast").expect("error toast");
    assert!(f32::from(toast.right()) <= crate::WINDOW_MIN_W);
    assert!(f32::from(toast.left()) >= 0.);
    assert_eq!(cx.debug_bounds("sidebar-canvas").unwrap(), canvas);
    cx.executor()
        .advance_clock(crate::entities::toast::Toast::ttl(
            crate::entities::toast::ToastKind::Error,
        ));
    draw(cx);
    assert!(cx.debug_bounds("app-toast").is_none());
}

#[gpui::test]
fn canvas_session_scope_changes_before_sidebar_render(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    cx.update(|_, cx| {
        let sidebar = shell.read(cx).sidebar.clone();
        let registry = shell.read(cx).runtime.read(cx).registry.clone();
        let id = registry.update(cx, |registry, cx| {
            let id = registry.insert_meta(
                "navigation".into(),
                "/grove-shell-navigation-test".into(),
                grove_core::agent::Agent::Terminal,
            );
            cx.notify();
            id
        });
        assert_eq!(
            sidebar.read(cx).active_canvas_sessions(cx),
            vec![(id, false)]
        );
        cx.update_global::<crate::settings::SettingsState, _>(|settings, _| {
            settings.store.workspaces.create("Other").unwrap();
        });
        // No draw or artificial entity notification between changing the
        // active workspace and the canvas's scope read.
        assert!(sidebar.read(cx).active_canvas_sessions(cx).is_empty());
        cx.update_global::<crate::settings::SettingsState, _>(|settings, _| {
            settings.store.workspaces.select(1);
        });
        assert_eq!(
            sidebar.read(cx).active_canvas_sessions(cx),
            vec![(id, false)]
        );
    });
}

struct TerminalFixture {
    terminal: Entity<super::super::terminal_view::TerminalView>,
    session: Entity<crate::entities::terminal_session::TerminalSession>,
}

impl Render for TerminalFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .on_key_down(traverse_unhandled_tab)
            .child(div().id("other-control").tab_index(0).child("Other"))
            .child(self.terminal.clone())
    }
}

#[gpui::test]
fn terminal_consumes_tab_before_shell_traversal(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (fixture, cx) = cx.add_window_view(|_, cx| {
        // An inert failed session exercises real terminal input routing
        // without starting a process or depending on asynchronous launch.
        let session = cx.new(crate::entities::terminal_session::TerminalSession::failed_for_test);
        assert!(
            session.read(cx).spawn_error().is_some(),
            "Input fixture must have no PTY"
        );
        let terminal =
            cx.new(|cx| super::super::terminal_view::TerminalView::new(session.clone(), cx));
        TerminalFixture { terminal, session }
    });
    draw(cx);
    cx.update(|window, cx| {
        let fixture = fixture.read(cx);
        fixture.terminal.focus_handle(cx).focus(window, cx);
    });
    let before_key = std::time::Instant::now();
    cx.simulate_keystrokes("tab");
    draw(cx);
    cx.update(|window, cx| {
        let fixture = fixture.read(cx);
        assert!(fixture.terminal.focus_handle(cx).is_focused(window));
        assert!(
            fixture
                .session
                .read(cx)
                .input_age()
                .is_some_and(|age| age <= before_key.elapsed()),
            "Tab reached TerminalSession::send"
        );
    });
    let before_control_c = std::time::Instant::now();
    cx.simulate_keystrokes("ctrl-c");
    draw(cx);
    cx.update(|window, cx| {
        let fixture = fixture.read(cx);
        assert!(fixture.terminal.focus_handle(cx).is_focused(window));
        assert!(
            fixture
                .session
                .read(cx)
                .input_age()
                .is_some_and(|age| age <= before_control_c.elapsed()),
            "Ctrl+C reached TerminalSession::send"
        );
    });
}

#[gpui::test]
fn cross_project_section_is_hidden_in_sidebar(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    cx.update(|_, cx| {
        let runtime = shell.read(cx).runtime.clone();
        runtime
            .read(cx)
            .registry
            .clone()
            .update(cx, |registry, cx| {
                registry.insert_meta_with_context(
                    "navigation".into(),
                    "/grove-shell-navigation-test".into(),
                    Agent::Claude,
                    vec![grove_core::session_meta::ContextRoot {
                        project: "WEB".into(),
                        wt_path: "/web/main".into(),
                    }],
                    None,
                );
                cx.notify();
            });
        runtime.read(cx).tree.clone().update(cx, |tree, cx| {
            tree.set_active_worktrees(
                0,
                vec![grove_core::git::Worktree {
                    path: "/grove-shell-navigation-test".into(),
                    branch: "main".into(),
                    mtime: None,
                    is_main: true,
                }],
            );
            cx.notify();
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("multi-project-sessions").is_none());
    assert!(cx.debug_bounds("multi-project-anchor").is_none());
}

#[gpui::test]
fn removed_header_launcher_preserves_selected_canvas_alignment(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let path = env!("CARGO_MANIFEST_DIR").to_string();
    cx.update(|cx| {
        let store = &mut cx.global_mut::<crate::settings::SettingsState>().store;
        store.projects[0].name = "navigation".into();
        store.projects[0].path = path.clone();
        store.recent_launches = vec![grove_core::storage::RecentLaunch {
            project: "navigation".into(),
            wt_path: path.clone(),
            agent: grove_core::agent::Agent::Terminal,
        }];
    });
    let (shell, cx) = cx.add_window_view(Shell::new);
    for width in [1280.0, 320.0] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(640.0)));
        draw(cx);
        assert!(cx.debug_bounds("header-new-session").is_none());
    }

    // Exercise the selected canvas with a registered session whose invalid
    // script cannot start a PTY reader. A live reader wakes GPUI's test
    // scheduler from another thread and makes this visual test nondeterministic.
    let selected = cx.update(|window, cx| {
        let session = cx.new(|cx| {
            crate::entities::terminal_session::TerminalSession::spawn_script("\0", &path, cx)
        });
        let registry = shell.read(cx).runtime.read(cx).registry.clone();
        let selected = registry.update(cx, |registry, cx| {
            let id = registry.insert_meta(
                "navigation".into(),
                path.clone(),
                grove_core::agent::Agent::Terminal,
            );
            registry.attach(id, session, None);
            cx.notify();
            id
        });
        let sidebar = shell.read(cx).sidebar.clone();
        sidebar.update(cx, |sidebar, cx| {
            sidebar.select_session_id(selected, window, cx);
        });
        selected
    });
    draw(cx);
    cx.update(|_, cx| {
        let shell = shell.read(cx);
        assert_eq!(shell.sidebar.read(cx).selected_session(), Some(selected));
        assert_eq!(
            shell.runtime.read(cx).state.read(cx).active_session(),
            Some(selected)
        );
    });
    let selector: &'static str =
        Box::leak(format!("terminal-header-{}", selected.raw()).into_boxed_str());
    let terminal_header = cx.debug_bounds(selector).expect("selected canvas header");
    let canvas = cx.debug_bounds("sidebar-canvas").expect("canvas");
    assert_eq!(terminal_header.left(), canvas.left());
    assert_eq!(terminal_header.right(), canvas.right());
}

#[gpui::test]
fn collapsed_close_shortcut_confirms_from_terminal_focus(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    cx.update(|cx| {
        cx.bind_keys(k::shell_bindings());
        cx.global_mut::<crate::settings::SettingsState>()
            .store
            .sidebar_collapsed = false;
    });
    let (shell, cx) = cx.add_window_view(Shell::new);
    let selected = cx.update(|_, cx| {
        let registry = shell.read(cx).runtime.read(cx).registry.clone();
        let terminal = cx.new(|cx| {
            crate::entities::terminal_session::TerminalSession::spawn_script(
                "\0",
                "/grove-shell-navigation-test",
                cx,
            )
        });
        registry.update(cx, |registry, cx| {
            let id = registry.insert_meta(
                "navigation".into(),
                "/grove-shell-navigation-test".into(),
                Agent::Terminal,
            );
            registry.attach(id, terminal, None);
            cx.notify();
            id
        })
    });
    cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
    draw(cx);
    cx.update(|window, cx| {
        shell.read(cx).sidebar.clone().update(cx, |sidebar, cx| {
            sidebar.select_session_id(selected, window, cx);
        });
    });
    draw(cx);
    let prior = cx.update(|window, cx| {
        let prior = shell
            .read(cx)
            .sidebar
            .read(cx)
            .canvas_terminal_focus(selected, false, cx)
            .expect("selected terminal");
        prior.focus(window, cx);
        prior
    });
    let rail = cx.debug_bounds("global-navigation").unwrap();
    cx.simulate_keystrokes(&format!("{}w", k::platform_mod_prefix()));
    draw(cx);
    assert!(cx.debug_bounds("sidebar-confirmation").is_some());
    assert_eq!(cx.debug_bounds("global-navigation").unwrap(), rail);
    assert!(cx.debug_bounds("canvas-confirmation").is_none());
    let cancel = cx.debug_bounds("cancel-close").unwrap();
    let confirm = cx.debug_bounds("confirm-close").unwrap();
    assert_eq!(cancel.top(), confirm.top());
    assert!(cancel.right() <= confirm.left());
    assert!(f32::from(cancel.size.height) >= 24.0);
    assert!(f32::from(confirm.size.height) >= 24.0);
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        assert!(prior.is_focused(window));
        assert!(!shell.read(cx).sidebar.read(cx).confirmation_open());
        assert!(shell
            .read(cx)
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .meta(selected)
            .is_some());
    });
    cx.update(|window, cx| prior.focus(window, cx));
    cx.simulate_keystrokes(&format!("{}w", k::platform_mod_prefix()));
    draw(cx);
    assert!(cx.debug_bounds("sidebar-confirmation").is_some());
    for zoom in [1.0, 1.5] {
        cx.update(|_, cx| cx.set_global(crate::zoom::ZoomState::new(zoom)));
        cx.simulate_resize(gpui::size(gpui::px(420.0), gpui::px(640.0)));
        draw(cx);
        let popup = cx.debug_bounds("sidebar-confirmation").unwrap();
        assert!(popup.left() >= gpui::px(0.0));
        assert!(popup.right() <= gpui::px(420.0));
        let cancel = cx.debug_bounds("cancel-close").unwrap();
        let confirm = cx.debug_bounds("confirm-close").unwrap();
        assert_eq!(cancel.top(), confirm.top());
        assert!(cancel.left() >= popup.left());
        assert!(confirm.right() <= popup.right());
    }
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, _| assert!(prior.is_focused(window)));
}

#[gpui::test]
fn new_session_shortcut_focuses_dashboard_after_closing_session_without_click(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(init);
    cx.update(|cx| cx.bind_keys(k::shell_bindings()));
    let (shell, cx) = cx.add_window_view(Shell::new);
    let selected = cx.update(|_, cx| {
        let registry = shell.read(cx).runtime.read(cx).registry.clone();
        let terminal = cx.new(|cx| {
            crate::entities::terminal_session::TerminalSession::spawn_script(
                "\0",
                "/grove-shell-navigation-test",
                cx,
            )
        });
        registry.update(cx, |registry, cx| {
            let id = registry.insert_meta(
                "navigation".into(),
                "/grove-shell-navigation-test".into(),
                Agent::Terminal,
            );
            registry.attach(id, terminal, None);
            cx.notify();
            id
        })
    });
    draw(cx);
    cx.update(|window, cx| {
        shell.read(cx).sidebar.clone().update(cx, |sidebar, cx| {
            sidebar.select_session_id(selected, window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes(&format!("{}w", k::platform_mod_prefix()));
    draw(cx);
    assert!(cx.debug_bounds("sidebar-confirmation").is_some());
    cx.simulate_keystrokes("tab enter");
    draw(cx);
    cx.update(|_, cx| {
        let shell = shell.read(cx);
        assert!(shell
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .meta(selected)
            .is_none());
        assert!(!shell.sidebar.read(cx).confirmation_open());
        assert_eq!(shell.sidebar.read(cx).selected_session(), None);
    });
    cx.simulate_keystrokes(&format!("{}p", k::platform_mod_prefix()));
    draw(cx);
    assert!(cx.debug_bounds("dashboard-summary").is_some());
    assert!(cx.debug_bounds("worktree-launcher-panel").is_none());
    cx.simulate_input("Continue after closing");
    draw(cx);
    assert_eq!(
        cx.update(|_, cx| shell.read(cx).sidebar.read(cx).dashboard_draft_for_test(cx)),
        Some("Continue after closing".into())
    );
}

#[gpui::test]
fn settings_shortcut_opens_panel_and_escape_restores_focus(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    cx.update(|cx| cx.bind_keys(k::shell_bindings()));
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    let prior = cx.update(|window, cx| {
        let focus = shell.read(cx).focus.clone();
        focus.focus(window, cx);
        focus
    });
    cx.simulate_keystrokes(&format!("{},", k::platform_mod_prefix()));
    draw(cx);
    assert!(cx.debug_bounds("settings-panel").is_some());
    let canvas = cx.debug_bounds("sidebar-canvas").expect("sidebar canvas");
    let settings = cx.debug_bounds("settings-panel").expect("settings page");
    assert_eq!(settings, canvas);
    assert!(cx.debug_bounds("settings-overlay").is_none());
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        assert!(prior.is_focused(window));
        assert!(!shell.read(cx).settings.read(cx).is_open());
    });
}

#[gpui::test]
fn settings_uses_canvas_from_grid_and_zen(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    cx.update(|window, cx| {
        let focus = shell.read(cx).focus.clone();
        focus.focus(window, cx);
        window.dispatch_action(Box::new(k::ToggleGrid), cx);
    });
    draw(cx);
    cx.update(|window, cx| window.dispatch_action(Box::new(k::Settings), cx));
    draw(cx);
    assert!(cx.debug_bounds("sidebar-rail").is_none());
    assert!(cx.debug_bounds("global-navigation").is_some());
    assert_eq!(
        cx.debug_bounds("settings-panel"),
        cx.debug_bounds("sidebar-canvas")
    );
    cx.simulate_resize(gpui::size(gpui::px(500.0), gpui::px(640.0)));
    draw(cx);
    assert!(cx.debug_bounds("sidebar-rail").is_none());
    assert_eq!(
        cx.debug_bounds("settings-panel"),
        cx.debug_bounds("sidebar-canvas")
    );
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
    cx.simulate_resize(gpui::size(gpui::px(1920.0), gpui::px(1080.0)));
    draw(cx);
    cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
    draw(cx);
    assert!(cx.debug_bounds("app-header").is_some());
    cx.update(|window, cx| window.dispatch_action(Box::new(k::Settings), cx));
    draw(cx);
    assert!(cx.debug_bounds("app-header").is_some());
    assert!(cx.debug_bounds("global-navigation").is_some());
    assert!(cx.debug_bounds("sidebar-rail").is_none());
    assert!(cx.debug_bounds("global-navigation").is_some());
    assert_eq!(
        cx.debug_bounds("settings-panel"),
        cx.debug_bounds("sidebar-canvas")
    );
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_zen()));
    assert!(cx.debug_bounds("app-header").is_some());
}

#[gpui::test]
fn settings_control_opens_panel(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    let point = cx.debug_bounds("sidebar-settings").unwrap().center();
    cx.simulate_mouse_down(point, MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_up(point, MouseButton::Left, gpui::Modifiers::default());
    draw(cx);
    cx.update(|_, cx| assert!(shell.read(cx).settings.read(cx).is_open()));
}

#[gpui::test]
fn palette_setting_command_opens_settings_panel(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    cx.update(|window, cx| {
        let launcher = shell.read(cx).launcher.clone();
        launcher.update(cx, |launcher, cx| launcher.open(window, cx));
    });
    draw(cx);
    cx.simulate_input("appearance");
    draw(cx);
    assert!(cx.debug_bounds("launcher-row-0").is_some());
    cx.simulate_keystrokes("enter");
    draw(cx);
    cx.update(|_, cx| {
        assert!(!shell.read(cx).launcher.read(cx).is_open());
        assert!(shell.read(cx).settings.read(cx).is_open());
    });
}

#[gpui::test]
fn zoom_shortcut_persists_the_same_value_as_the_settings_panel(cx: &mut gpui::TestAppContext) {
    cx.update(init);
    let (shell, cx) = cx.add_window_view(Shell::new);
    draw(cx);
    cx.update(|window, cx| {
        let focus = shell.read(cx).focus.clone();
        focus.focus(window, cx);
        window.dispatch_action(Box::new(k::ZoomIn), cx);
    });
    cx.update(|_, cx| {
        let zoom = cx.global::<crate::zoom::ZoomState>().zoom;
        assert_eq!(zoom, 1.1);
        assert_eq!(
            cx.global::<crate::settings::SettingsState>().store.ui_zoom,
            Some(zoom)
        );
    });
}
