use super::*;
#[test]
#[ignore = "requires native headless GPU renderer; writes isolated Dashboard screenshots to /tmp"]
fn dashboard_native_snapshots() {
    use gpui::AppContext as _;
    let repo = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "Fixture",
        ],
    ] {
        assert!(std::process::Command::new("git")
            .current_dir(repo.path())
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    let path = repo.path().to_string_lossy().into_owned();
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        std::sync::Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(true);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        let mut store = grove_core::storage::Store::default();
        store.projects.push(grove_core::storage::Project {
            name: "Dashboard visual fixture".into(),
            path: path.clone(),
            scripts: grove_core::storage::ProjectScripts::default(),
            archived: false,
            worktree_dir: None,
        });
        store.assign_project_to_active_workspace(&path);
        cx.set_global(SettingsState::new(store));
        grove_core::theme::set(grove_core::theme::by_name("grove-dark").unwrap());
        c::sync_component_theme(cx);
    });
    for (state, width) in [
        ("activity-projectless-codex", 1100.),
        ("activity-projectless-claude-narrow", 760.),
        ("activity-codex", 1100.),
        ("activity-claude-narrow", 760.),
        ("activity-codex-history", 1100.),
        ("activity-claude-narrow-history", 760.),
        ("normal", 1100.),
        ("narrow", 560.),
        ("worktree-menu", 1100.),
        ("project-menu", 1100.),
        ("narrow-worktree-menu", 560.),
        ("narrow-project-menu", 560.),
        ("hover", 1100.),
        ("worktree-hover", 1100.),
        ("light", 1100.),
        ("light-hover", 1100.),
    ] {
        cx.update(|cx| {
            let theme = if state.starts_with("light") {
                "grove-light"
            } else {
                "grove-dark"
            };
            grove_core::theme::set(grove_core::theme::by_name(theme).unwrap());
            c::sync_component_theme(cx);
        });
        let mut sidebar = None;
        let window = cx
            .open_window(gpui::size(gpui::px(width), gpui::px(760.)), |window, cx| {
                let runtime = cx.new(crate::runtime::Runtime::new);
                let view = cx.new(|cx| Sidebar::new(runtime, window, cx));
                sidebar = Some(view.clone());
                cx.new(|cx| {
                    gpui_component::Root::new(view, window, cx)
                        .bordered(false)
                        .bg(c::BG())
                })
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            sidebar.as_ref().unwrap().update(cx, |sidebar, cx| {
                sidebar.open_dashboard(window, cx);
                assert!(sidebar.dashboard_open);
                assert!(sidebar.dashboard.is_some());
                if state.starts_with("activity-") {
                    let provider = if state.contains("claude") {
                        grove_core::codex::Provider::Claude
                    } else {
                        grove_core::codex::Provider::Codex
                    };
                    if state.contains("projectless") {
                        sidebar
                            .dashboard
                            .as_ref()
                            .unwrap()
                            .session
                            .update(cx, |s, _| {
                                s.state.saved.project.clear();
                                s.state.saved.cwd = dashboard_home_dir();
                            });
                    }
                    let id = seed_activity_fixture(sidebar, provider, window, cx);
                    if state.ends_with("history") {
                        sidebar
                            .dashboard_timelines
                            .toggle((sidebar.active_workspace, id));
                    }
                }
            });
            let _ = window.draw(cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            let _ = window.draw(cx);
        })
        .unwrap();
        if state.ends_with("hover") {
            let idx = usize::from(state == "worktree-hover");
            cx.update_window(window.into(), |_, window, cx| {
                let tray = sidebar
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .dashboard
                    .as_ref()
                    .unwrap()
                    .tray
                    .clone();
                let position = tray.read(cx).bounds[idx].get().center();
                window.dispatch_event(
                    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position,
                        pressed_button: None,
                        modifiers: gpui::Modifiers::default(),
                    }),
                    cx,
                );
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
                let tray = &sidebar
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .dashboard
                    .as_ref()
                    .unwrap()
                    .tray;
                assert!(
                    tray.read(cx).hovered[idx],
                    "native pointer must enter selector"
                );
            })
            .unwrap();
        }
        if state.ends_with("menu") {
            cx.update_window(window.into(), |_, window, cx| {
                let tray = sidebar
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .dashboard
                    .as_ref()
                    .unwrap()
                    .tray
                    .clone();
                tray.update(cx, |tray, cx| {
                    tray.open(state.ends_with("project-menu"), window, cx);
                });
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
        }
        let output = format!("/tmp/grove-dashboard-{state}.png");
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(&output)
            .unwrap();
        eprintln!("Native Dashboard fixture: {output}");
    }
}
