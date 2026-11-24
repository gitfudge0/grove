use super::*;
use crate::theme::{DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME};
use grove_core::storage::AppearancePreference;

struct FocusFixture {
    panel: Entity<SettingsPanel>,
    behind: FocusHandle,
}

impl Render for FocusFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(
                div()
                    .id("behind-settings")
                    .tab_index(0)
                    .track_focus(&self.behind)
                    .child("Behind"),
            )
            .when(self.panel.read(cx).is_open(), |root| {
                root.child(self.panel.clone())
            })
    }
}

#[gpui::test]
fn projects_settings_filters_workspace_and_returns_from_editor(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    cx.update(|cx| {
        let store = &mut cx.global_mut::<SettingsState>().store;
        store.workspaces.create("Other workspace").unwrap();
        store.workspaces.select(1);
        for (name, archived, workspace) in
            [("Atlas", false, 1), ("Old", true, 1), ("Other", false, 2)]
        {
            let path = format!("/qa/{name}");
            store.projects.push(grove_core::storage::Project {
                name: name.into(),
                path: path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived,
                worktree_dir: None,
            });
            store.project_workspaces.insert(path, workspace);
        }
    });
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(800.)));
    cx.update(|window, cx| panel.update(cx, |panel, cx| panel.open_projects(None, window, cx)));
    draw(cx);
    assert!(cx.debug_bounds("settings-project-row-0").is_some());
    assert!(cx.debug_bounds("settings-project-row-1").is_none());
    cx.update(|_, cx| {
        let runtime = panel.read(cx).runtime.clone();
        let tree = runtime.read(cx).tree.clone();
        let active = runtime.read(cx).state.read(cx).proj_idx();
        tree.update(cx, |tree, _| {
            tree.register_project_worktrees(
                0,
                active,
                vec![
                    grove_core::git::Worktree {
                        path: "/qa/Atlas".into(),
                        branch: "main".into(),
                        mtime: None,
                        is_main: true,
                    },
                    grove_core::git::Worktree {
                        path: "/qa/feature".into(),
                        branch: "feature".into(),
                        mtime: None,
                        is_main: false,
                    },
                ],
            );
        });
    });
    assert!(cx.debug_bounds("settings-project-search").is_none());
    let disclosure = cx.debug_bounds("settings-project-worktrees-0").unwrap();
    cx.simulate_click(disclosure.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(cx
        .debug_bounds("settings-project-worktree-list-0")
        .is_some());
    cx.update(|window, cx| {
        panel.read(cx).project_disclosure_focus["/qa/Atlas"]
            .clone()
            .focus(window, cx);
    });
    cx.simulate_keystrokes("space");
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("space").unwrap(),
    });
    draw(cx);
    assert!(cx
        .debug_bounds("settings-project-worktree-list-0")
        .is_none());
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.open_project_page(
                super::super::sidebar::projects::Page::Edit("/qa/Atlas".into()),
                window,
                cx,
            );
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(450.), gpui::px(305.)));
    cx.update(|window, _| window.set_rem_size(gpui::px(20.)));
    draw(cx);
    let heading = cx
        .debug_bounds("settings-heading")
        .expect("Settings heading");
    let picker = cx
        .debug_bounds("settings-category-picker")
        .expect("compact project category picker");
    let editor = cx
        .debug_bounds("project-panel")
        .expect("embedded project page");
    let back = cx
        .debug_bounds("project-panel-back")
        .expect("embedded project Back");
    let scroll = cx
        .debug_bounds("project-panel-scroll")
        .expect("embedded project scroll region");
    let footer = cx
        .debug_bounds("project-panel-footer")
        .expect("embedded project footer");
    assert!(heading.bottom() <= picker.top() && picker.bottom() <= editor.top());
    assert!(editor.top() <= back.top() && back.bottom() <= scroll.top());
    assert!(scroll.bottom() <= footer.top());
    let viewport = cx
        .debug_bounds("settings-panel")
        .expect("Settings viewport");
    assert!(
        footer.bottom() <= viewport.bottom() && viewport.bottom() <= gpui::px(305.),
        "footer {footer:?}, editor {editor:?}, scroll {scroll:?}, viewport {viewport:?}",
    );
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: scroll.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-120.))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    draw(cx);
    assert_eq!(cx.debug_bounds("settings-heading"), Some(heading));
    assert_eq!(cx.debug_bounds("settings-category-picker"), Some(picker));
    assert_eq!(cx.debug_bounds("project-panel-back"), Some(back));
    assert_eq!(cx.debug_bounds("project-panel-scroll"), Some(scroll));
    assert_eq!(cx.debug_bounds("project-panel-footer"), Some(footer));
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(cx.debug_bounds("settings-project-row-0").is_some());
    cx.update(|_, cx| assert_eq!(panel.read(cx).category, Category::Projects));
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| assert!(!panel.read(cx).is_open()));
}

fn setup(cx: &mut App) {
    gpui_component::init(cx);
    cx.set_global(SettingsState::new(Store::default()));
    cx.set_global(ZoomState::new(1.0));
    cx.set_global(crate::zoom::CurrentPtyDims::default());
    cx.set_global(ThemeState::new(
        false,
        DEFAULT_DARK_THEME.into(),
        DEFAULT_LIGHT_THEME.into(),
    ));
    c::sync_component_theme(cx);
}

fn draw(cx: &mut gpui::VisualTestContext) {
    // Settle the second pass if the actual Settings canvas changes breakpoint.
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
}

impl SettingsPanel {
    fn fixture_providers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::entities::provider_updates::ProviderRow;
        use grove_core::provider_updates::{Installer, ProviderStatus};
        for (index, scenario) in ["connected", "signed-out"].into_iter().enumerate() {
            self.provider_accounts[index]
                .update(cx, |account, cx| account.fixture(scenario, window, cx));
        }
        self.runtime
            .read(cx)
            .provider_updates
            .clone()
            .update(cx, |updates, cx| {
                updates.set_rows_for_test(
                    [Agent::Codex, Agent::Claude]
                        .into_iter()
                        .map(|agent| ProviderRow {
                            status: ProviderStatus {
                                agent,
                                installed: Some("1.0.0".into()),
                                latest: Some("1.1.0".into()),
                                executable: Some("/fixture/provider".into()),
                                error: None,
                                installer: if agent == Agent::Claude {
                                    Installer::ClaudeNative
                                } else {
                                    Installer::Npm {
                                        executable: "/fixture/npm".into(),
                                        prefix: "/fixture".into(),
                                        package: "@openai/codex".into(),
                                    }
                                },
                            },
                            checking: false,
                            updating: false,
                            updated: false,
                            update_verified: false,
                            last_success: Some(1),
                            error: None,
                        })
                        .collect(),
                    cx,
                );
            });
    }
}

#[gpui::test]
fn providers_management_is_account_and_version_only_and_restores_focus(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.open_provider_updates(window, cx);
            panel.fixture_providers(window, cx);
        });
    });
    draw(cx);
    assert!(cx
        .debug_bounds("settings-provider-codex-management")
        .is_none());
    assert!(cx
        .debug_bounds("settings-provider-claude-management")
        .is_none());
    for agent in [Agent::Codex, Agent::Claude] {
        let manage = cx
            .debug_bounds(if agent == Agent::Codex {
                "settings-provider-codex-manage"
            } else {
                "settings-provider-claude-manage"
            })
            .unwrap();
        cx.simulate_click(manage.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx
            .debug_bounds(if agent == Agent::Codex {
                "settings-provider-codex-management"
            } else {
                "settings-provider-claude-management"
            })
            .is_some());
        assert!(cx.debug_bounds("connections-context").is_none());
        assert!(cx.debug_bounds("connections-server-0").is_none());
        assert!(cx.debug_bounds("connections-reconnect").is_none());
        cx.simulate_keystrokes("space");
        draw(cx);
        assert!(cx
            .debug_bounds(if agent == Agent::Codex {
                "settings-provider-codex-management"
            } else {
                "settings-provider-claude-management"
            })
            .is_none());
        cx.update(|window, cx| {
            assert!(
                panel.read(cx).provider_manage_focus[usize::from(agent != Agent::Codex)]
                    .is_focused(window)
            );
        });
    }
}

#[gpui::test]
fn provider_account_refresh_and_confirmation_are_keyboard_accessible(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.open_provider_updates(window, cx);
            panel.fixture_providers(window, cx);
            panel.managed_provider = Some(Agent::Codex);
        });
    });
    draw(cx);
    cx.update(|window, cx| {
        panel.read(cx).provider_accounts[0]
            .read(cx)
            .fixture_return_focus()
            .focus(window, cx);
    });
    cx.simulate_keystrokes("enter");
    draw(cx);
    cx.simulate_keystrokes("tab space");
    draw(cx);
    cx.update(|_, cx| {
        assert!(panel.read(cx).provider_accounts[0]
            .read(cx)
            .fixture_confirming());
    });
    cx.simulate_keystrokes("space");
    draw(cx);
    cx.update(|_, cx| {
        assert!(!panel.read(cx).provider_accounts[0]
            .read(cx)
            .fixture_confirming());
    });
}

#[gpui::test]
fn provider_actions_fit_native_widths_and_zoom(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.open_provider_updates(window, cx);
            panel.fixture_providers(window, cx);
            panel.managed_provider = Some(Agent::Codex);
        });
    });
    for width in [900., 500., 320.] {
        for rem in [16., 24.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1200.)));
            cx.update(|window, cx| {
                window.set_rem_size(gpui::px(rem));
                gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
            });
            draw(cx);
            let body = cx.debug_bounds("settings-body").unwrap();
            for name in [
                "settings-provider-codex-actions",
                "provider-account-actions",
                "settings-provider-codex-version-actions",
            ] {
                let action = cx.debug_bounds(name).unwrap();
                assert!(
                    action.left() >= body.left() && action.right() <= body.right(),
                    "{name} outside body at {width}/{rem}: {action:?}, {body:?}"
                );
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "captures isolated Connections screens with fixture state"]
fn connections_review_linux_capture() {
    let output = std::path::PathBuf::from(
        std::env::var_os("GROVE_CONNECTIONS_QA_OUTPUT").expect("task-owned capture directory"),
    );
    assert!(
        std::env::var_os("GROVE_CONFIG_DIR").is_some(),
        "isolated configuration required"
    );
    fs_err::create_dir_all(&output).unwrap();
    gpui_platform::application()
        .with_assets(crate::assets::Assets)
        .run(move |cx| {
            setup(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
            let mut retained = None;
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        app_id: Some("dev.grove.connections-review-qa".into()),
                        kind: gpui::WindowKind::PopUp,
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                            gpui::point(gpui::px(40.), gpui::px(40.)),
                            gpui::size(gpui::px(900.), gpui::px(900.)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        window.set_window_title("Grove Connections review QA");
                        let runtime = cx.new(Runtime::new);
                        let panel = cx.new(|cx| SettingsPanel::new(runtime, cx));
                        panel.update(cx, |panel, cx| {
                            panel.open(window, cx);
                            panel.category = Category::Agents;
                            panel.page = Page::Connections;
                        });
                        retained = Some(panel.clone());
                        cx.new(|cx| gpui_component::Root::new(panel, window, cx))
                    },
                )
                .unwrap();
            let panel = retained.unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                for (theme, rem, width) in [
                    ("grove-dark", 16., 900.),
                    ("grove-light", 16., 900.),
                    ("grove-dark", 24., 500.),
                ] {
                    for scenario in [
                        "connected",
                        "signed-out",
                        "external",
                        "waiting",
                        "failed",
                        "confirm",
                        "contexts",
                    ] {
                        cx.update(|cx| {
                            grove_core::theme::set_by_name(theme);
                            c::sync_component_theme(cx);
                            gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                        });
                        cx.update_window(window.into(), |_, window, cx| {
                            window.resize(gpui::size(gpui::px(width), gpui::px(900.)));
                            window.set_rem_size(gpui::px(rem));
                            panel.update(cx, |panel, cx| {
                                panel.scroll.set_offset(gpui::Point::default());
                                panel.connections.update(cx, |connections, cx| {
                                    connections.fixture(scenario, window, cx);
                                });
                                cx.notify();
                            });
                            window.refresh();
                        })
                        .unwrap();
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(200))
                            .await;
                        if let Some(helper) = std::env::var_os("GROVE_QA_EXPOSE_HELPER") {
                            for _ in 0..3 {
                                assert!(std::process::Command::new(&helper)
                                    .arg("Grove Connections review QA")
                                    .status()
                                    .unwrap()
                                    .success());
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(100))
                                    .await;
                            }
                        }
                        let path = output
                            .join(format!("connections-{scenario}-{theme}-{rem}-{width}.png"));
                        assert!(std::process::Command::new("import")
                            .arg("-window")
                            .arg("Grove Connections review QA")
                            .arg(&path)
                            .status()
                            .unwrap()
                            .success());
                        if scenario == "waiting" || width <= 500. {
                            cx.update(|cx| {
                                panel.update(cx, |panel, cx| {
                                    panel
                                        .scroll
                                        .set_offset(gpui::point(gpui::px(0.), gpui::px(-10000.)));
                                    cx.notify();
                                });
                            });
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(100))
                                .await;
                            if let Some(helper) = std::env::var_os("GROVE_QA_EXPOSE_HELPER") {
                                for _ in 0..3 {
                                    assert!(std::process::Command::new(&helper)
                                        .arg("Grove Connections review QA")
                                        .status()
                                        .unwrap()
                                        .success());
                                    cx.background_executor()
                                        .timer(std::time::Duration::from_millis(100))
                                        .await;
                                }
                            }
                            let path = output.join(format!(
                                "connections-{scenario}-bottom-{theme}-{rem}-{width}.png"
                            ));
                            assert!(std::process::Command::new("import")
                                .arg("-window")
                                .arg("Grove Connections review QA")
                                .arg(path)
                                .status()
                                .unwrap()
                                .success());
                        }
                    }
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "captures isolated Providers screens with fixture state"]
fn providers_review_linux_capture() {
    let output = std::path::PathBuf::from(
        std::env::var_os("GROVE_PROVIDERS_QA_OUTPUT").expect("task-owned capture directory"),
    );
    assert!(
        std::env::var_os("GROVE_CONFIG_DIR").is_some(),
        "isolated configuration required"
    );
    fs_err::create_dir_all(&output).unwrap();
    gpui_platform::application()
        .with_assets(crate::assets::Assets)
        .run(move |cx| {
            setup(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
            let mut retained = None;
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        app_id: Some("dev.grove.providers-review-qa".into()),
                        kind: gpui::WindowKind::PopUp,
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                            gpui::point(gpui::px(40.), gpui::px(40.)),
                            gpui::size(gpui::px(900.), gpui::px(900.)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        window.set_window_title("Grove Providers review QA");
                        let runtime = cx.new(Runtime::new);
                        let panel = cx.new(|cx| SettingsPanel::new(runtime, cx));
                        panel.update(cx, |panel, cx| {
                            panel.open(window, cx);
                            panel.category = Category::Agents;
                            panel.page = Page::Providers;
                        });
                        retained = Some(panel.clone());
                        cx.new(|cx| gpui_component::Root::new(panel, window, cx))
                    },
                )
                .unwrap();
            let panel = retained.unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                for (theme, rem, width) in [
                    ("grove-dark", 16., 900.),
                    ("grove-light", 16., 900.),
                    ("grove-dark", 24., 500.),
                ] {
                    for scenario in [
                        "collapsed",
                        "codex",
                        "claude",
                        "browser",
                        "waiting",
                        "confirm",
                        "external",
                    ] {
                        cx.update(|cx| {
                            grove_core::theme::set_by_name(theme);
                            c::sync_component_theme(cx);
                            gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                        });
                        cx.update_window(window.into(), |_, window, cx| {
                            window.resize(gpui::size(gpui::px(width), gpui::px(900.)));
                            window.set_rem_size(gpui::px(rem));
                            panel.update(cx, |panel, cx| {
                                panel.scroll.set_offset(gpui::Point::default());
                                panel.fixture_providers(window, cx);
                                panel.managed_provider = match scenario {
                                    "collapsed" => None,
                                    "claude" | "external" => Some(Agent::Claude),
                                    _ => Some(Agent::Codex),
                                };
                                let index =
                                    usize::from(panel.managed_provider == Some(Agent::Claude));
                                if ["browser", "waiting", "confirm", "external"].contains(&scenario)
                                {
                                    panel.provider_accounts[index].update(cx, |account, cx| {
                                        account.fixture(scenario, window, cx);
                                    });
                                }
                                cx.notify();
                            });
                            window.refresh();
                        })
                        .unwrap();
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(200))
                            .await;
                        if let Some(helper) = std::env::var_os("GROVE_QA_EXPOSE_HELPER") {
                            for _ in 0..3 {
                                assert!(std::process::Command::new(&helper)
                                    .arg("Grove Providers review QA")
                                    .status()
                                    .unwrap()
                                    .success());
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(100))
                                    .await;
                            }
                        }
                        let path =
                            output.join(format!("providers-{scenario}-{theme}-{rem}-{width}.png"));
                        assert!(std::process::Command::new("import")
                            .arg("-window")
                            .arg("Grove Providers review QA")
                            .arg(&path)
                            .status()
                            .unwrap()
                            .success());
                        if scenario == "waiting" || width <= 500. {
                            cx.update(|cx| {
                                panel.update(cx, |panel, cx| {
                                    panel
                                        .scroll
                                        .set_offset(gpui::point(gpui::px(0.), gpui::px(-10000.)));
                                    cx.notify();
                                });
                            });
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(100))
                                .await;
                            if let Some(helper) = std::env::var_os("GROVE_QA_EXPOSE_HELPER") {
                                for _ in 0..3 {
                                    assert!(std::process::Command::new(&helper)
                                        .arg("Grove Providers review QA")
                                        .status()
                                        .unwrap()
                                        .success());
                                    cx.background_executor()
                                        .timer(std::time::Duration::from_millis(100))
                                        .await;
                                }
                            }
                            let path = output.join(format!(
                                "providers-{scenario}-bottom-{theme}-{rem}-{width}.png"
                            ));
                            assert!(std::process::Command::new("import")
                                .arg("-window")
                                .arg("Grove Providers review QA")
                                .arg(path)
                                .status()
                                .unwrap()
                                .success());
                        }
                    }
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "captures isolated Settings pages with fixture state"]
fn settings_review_linux_capture() {
    let output = std::path::PathBuf::from(
        std::env::var_os("GROVE_SETTINGS_QA_OUTPUT").expect("task-owned capture directory"),
    );
    assert!(
        std::env::var_os("GROVE_CONFIG_DIR").is_some(),
        "isolated configuration required"
    );
    fs_err::create_dir_all(&output).unwrap();
    gpui_platform::application().with_assets(crate::assets::Assets).run(move |cx| {
            setup(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
            let mut retained = None;
            let window = cx.open_window(gpui::WindowOptions {
                app_id: Some("dev.grove.settings-review-qa".into()),
                window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                    gpui::point(gpui::px(40.), gpui::px(40.)), gpui::size(gpui::px(900.), gpui::px(850.))))),
                ..Default::default()
            }, |window, cx| {
                window.set_window_title("Grove Settings review QA");
                let runtime = cx.new(Runtime::new);
                runtime.read(cx).upgrade.clone().update(cx, |upgrade, cx| upgrade.set_fixture(UpgradeState::Idle, ChangelogState::Idle, cx));
                let panel = cx.new(|cx| SettingsPanel::new(runtime, cx));
                panel.update(cx, |panel, cx| panel.open(window, cx));
                retained = Some(panel.clone());
                cx.new(|cx| gpui_component::Root::new(panel, window, cx))
            }).unwrap();
            let panel = retained.unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let cases = [
                    ("general", Category::General, Page::Settings),
                    ("general-open", Category::General, Page::Settings),
                    ("agents", Category::Agents, Page::Settings),
                    ("terminal", Category::Terminal, Page::Settings),
                    ("privacy", Category::Privacy, Page::Settings),
                    ("about", Category::About, Page::Settings),
                    ("themes", Category::General, Page::Themes),
                    ("themes-notices", Category::General, Page::Themes),
                    ("archived", Category::General, Page::Archived),
                    ("shortcuts", Category::General, Page::Shortcuts),
                    ("providers", Category::Agents, Page::Providers),
                    ("providers-error", Category::Agents, Page::Providers),
                    ("changelog", Category::About, Page::Changelog),
                    ("changelog-error", Category::About, Page::Changelog),
                    ("terminal-working", Category::Terminal, Page::Settings),
                    ("terminal-confirmed", Category::Terminal, Page::Settings),
                    ("terminal-error", Category::Terminal, Page::Settings),
                ];
                for (theme, rem, width) in [("grove-dark", 16., 900.), ("grove-light", 16., 900.), ("grove-dark", 24., 500.), ("grove-dark", 24., 320.)] {
                    for (name, category, page) in cases {
                        cx.update(|cx| {
                            grove_core::theme::set_by_name(theme);
                            c::sync_component_theme(cx);
                            gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                        });
                        cx.update_window(window.into(), |_, window, cx| {
                            window.resize(gpui::size(gpui::px(width), gpui::px(850.)));
                            window.set_rem_size(gpui::px(rem));
                            panel.update(cx, |panel, cx| {
                                cx.global_mut::<SettingsState>().store.after_session_start = if name == "general-open" {
                                    AfterSessionStart::OpenSession
                                } else {
                                    AfterSessionStart::StayOnDashboard
                                };
                                panel.select_category(category, cx);
                                panel.show_theme_notices = name == "themes-notices";
                                if page == Page::Changelog {
                                    panel.page = page; // Fixture content; never fetch the network.
                                } else if page != Page::Settings {
                                    panel.open_nested(page, window, cx);
                                }
                                let runtime = panel.runtime.read(cx);
                                let upgrade = runtime.upgrade.clone();
                                let environment = runtime.shell_environment.clone();
                                let release = grove_core::upgrade::Release {
                                    version: semver::Version::new(99,0,0), tag: "v99.0.0".into(), html_url: "https://example.invalid/release".into(), body: String::new(), dmg_url: None, dmg_sha256_url: None, target_commitish: "main".into()
                                };
                                let notes = vec![grove_core::upgrade::ReleaseNote {
                                    tag: "Example release".into(), name: String::new(), date: String::new(), body: "## Improvements\n\n- Clearer update status\n- Better recovery\n\n[Release details](https://example.invalid/release)\n\n```rust\nlet recovered = true;\n```".into()
                                }];
                                upgrade.update(cx, |upgrade, cx| {
                                    upgrade.set_fixture(UpgradeState::Skipped(release), ChangelogState::Loaded(notes), cx);
                                    if name == "changelog-error" { upgrade.set_fixture(UpgradeState::Idle, ChangelogState::Error("Network unavailable".into()), cx); }
                                });
                                environment.update(cx, |environment, cx| environment.set_fixture(match name {
                                    "terminal-working" => RefreshState::Refreshing,
                                    "terminal-confirmed" => RefreshState::Refreshed,
                                    "terminal-error" => RefreshState::Error,
                                    _ => RefreshState::Idle,
                                }, cx));
                                if page == Page::Providers {
                                    let providers = panel.runtime.read(cx).provider_updates.clone();
                                    let rows = if name == "providers-error" {
                                        use crate::entities::provider_updates::ProviderRow;
                                        use grove_core::provider_updates::{Installer, ProviderStatus};
                                        panel.provider_details.insert(Agent::Claude.label());
                                        vec![ProviderRow {
                                            status: ProviderStatus {
                                                agent: Agent::Claude, installed: Some("1.0.0".into()), latest: None,
                                                executable: Some("/fixture/claude".into()), error: None,
                                                installer: Installer::ClaudeNative,
                                            }, checking: false, updating: false, updated: false, update_verified: false,
                                            last_success: Some(1), error: Some("Permission denied while updating the provider executable. Example diagnostic only.".into()),
                                        }]
                                    } else { Vec::new() };
                                    providers.update(cx, |providers, cx| providers.set_rows_for_test(rows, cx));
                                }
                                if name == "privacy" {
                                    SettingsState::update(cx, |store| store.telemetry_enabled = Some(true));
                                }
                                cx.notify();
                            });
                            window.refresh();
                        }).unwrap();
                        cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
                        if name == "themes-notices" || (name.starts_with("terminal-") && width <= 500.) {
                            cx.update(|cx| panel.update(cx, |panel, cx| {
                                panel.scroll.set_offset(gpui::point(gpui::px(0.), gpui::px(-10000.)));
                                cx.notify();
                            }));
                            cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                        }
                        let path = output.join(format!("{name}-{theme}-{rem}-{width}.png"));
                        assert!(std::process::Command::new("import").arg("-window").arg("Grove Settings review QA").arg(&path).status().unwrap().success());
                        eprintln!("Settings fixture capture: {}", path.display());
                    }
                }
                cx.update(|cx| cx.quit());
            }).detach();
        });
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens an isolated native X11 Settings window"]
fn settings_header_linux_capture() {
    assert!(
        std::env::var_os("GROVE_CONFIG_DIR").is_some(),
        "use an isolated config directory"
    );
    gpui_platform::application()
        .with_assets(crate::assets::Assets)
        .run(|cx| {
            setup(cx);
            crate::fonts::register(cx).unwrap();
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                            gpui::point(gpui::px(40.), gpui::px(40.)),
                            gpui::size(gpui::px(600.), gpui::px(650.)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        window.set_window_title("Grove header pattern QA");
                        let runtime = cx.new(Runtime::new);
                        let panel = cx.new(|cx| SettingsPanel::new(runtime, cx));
                        panel.update(cx, |panel, cx| {
                            panel.open(window, cx);
                            panel.open_nested(Page::Shortcuts, window, cx);
                        });
                        cx.new(|cx| gpui_component::Root::new(panel, window, cx))
                    },
                )
                .unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                for (theme, rem) in [
                    ("grove-dark", 16.),
                    ("grove-light", 16.),
                    ("grove-dark", 24.),
                ] {
                    cx.update(|cx| {
                        grove_core::theme::set_by_name(theme);
                        c::sync_component_theme(cx);
                        gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                    });
                    cx.update_window(window.into(), |_, window, _| {
                        window.set_rem_size(gpui::px(rem));
                        window.refresh();
                    })
                    .unwrap();
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(600))
                        .await;
                    let output = format!("/tmp/grove-settings-header-{theme}-{rem}.png");
                    assert!(std::process::Command::new("import")
                        .args(["-window", "Grove header pattern QA", &output])
                        .status()
                        .unwrap()
                        .success());
                    eprintln!("Native Settings header capture: {output}");
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens an isolated native X11 Settings window"]
fn settings_button_icons_linux_capture() {
    assert!(
        std::env::var_os("GROVE_CONFIG_DIR").is_some(),
        "use an isolated config directory"
    );
    gpui_platform::application()
        .with_assets(crate::assets::Assets)
        .run(|cx| {
            setup(cx);
            crate::fonts::register(cx).unwrap();
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                            gpui::point(gpui::px(40.), gpui::px(40.)),
                            gpui::size(gpui::px(600.), gpui::px(650.)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        window.set_window_title("Grove button icons QA");
                        let runtime = cx.new(Runtime::new);
                        let panel = cx.new(|cx| SettingsPanel::new(runtime, cx));
                        panel.update(cx, |panel, cx| {
                            panel.open(window, cx);
                        });
                        cx.new(|cx| gpui_component::Root::new(panel, window, cx))
                    },
                )
                .unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                for (theme, rem) in [
                    ("grove-dark", 16.),
                    ("grove-light", 16.),
                    ("grove-dark", 24.),
                ] {
                    cx.update(|cx| {
                        grove_core::theme::set_by_name(theme);
                        c::sync_component_theme(cx);
                        gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                    });
                    cx.update_window(window.into(), |_, window, _| {
                        window.set_rem_size(gpui::px(rem));
                        window.refresh();
                    })
                    .unwrap();
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(600))
                        .await;
                    let output = format!("/tmp/grove-settings-buttons-{theme}-{rem}.png");
                    assert!(std::process::Command::new("import")
                        .args(["-window", "Grove button icons QA", &output])
                        .status()
                        .unwrap()
                        .success());
                    eprintln!("Native Settings button capture: {output}");
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}

#[gpui::test]
fn connections_auth_completion_restores_owned_focus_without_stealing_other_focus(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.open(window, cx);
            panel.page = Page::Connections;
        });
    });
    for target in [0, 1, 2] {
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.connections.update(cx, |connections, cx| {
                    connections.fixture("waiting", window, cx);
                });
            });
        });
        draw(cx);
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.connections.update(cx, |connections, cx| {
                    let focus = if target == 2 {
                        connections.fixture_copy_focus()
                    } else {
                        connections.fixture_auth_focus(target == 0, cx)
                    };
                    focus.focus(window, cx);
                    connections.fixture_finish(cx);
                });
            });
        });
        draw(cx);
        cx.update(|window, cx| {
            assert!(panel
                .read(cx)
                .connections
                .read(cx)
                .fixture_return_focus()
                .is_focused(window));
        });
    }
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.connections.update(cx, |connections, cx| {
                connections.fixture("waiting", window, cx);
            });
            panel.focus.focus(window, cx);
        });
    });
    draw(cx);
    cx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.connections.update(
                cx,
                super::super::connections_panel::ConnectionsPanel::fixture_finish,
            );
        });
    });
    draw(cx);
    cx.update(|window, cx| assert!(panel.read(cx).focus.is_focused(window)));
}

#[gpui::test]
fn after_session_start_choices_update_saved_preference(cx: &mut gpui::TestAppContext) {
    struct NavigationHost(Entity<SettingsPanel>);
    impl Render for NavigationHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .on_key_down(super::super::shell::traverse_unhandled_tab)
                .child(self.0.clone())
        }
    }
    let _disk = crate::settings::lock_test_config();
    cx.update(setup);
    let mut panel = None;
    let (_, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        let entity = cx.new(|cx| SettingsPanel::new(runtime, cx));
        panel = Some(entity.clone());
        NavigationHost(entity)
    });
    let panel = panel.unwrap();
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.open(window, cx);
            panel.select_category(Category::Agents, cx);
        });
    });
    draw(cx);
    for (id, expected) in [
        (
            "settings-after-session-start-open",
            AfterSessionStart::OpenSession,
        ),
        (
            "settings-after-session-start-stay",
            AfterSessionStart::StayOnDashboard,
        ),
    ] {
        let bounds = cx.debug_bounds(id).unwrap();
        cx.simulate_click(bounds.center(), gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<SettingsState>().store.after_session_start,
                expected
            );
        });
    }
    // Traverse through the app's actual unhandled-Tab route, not direct focus.
    cx.update(|window, cx| {
        panel.read(cx).after_session_start_focus[0]
            .clone()
            .focus(window, cx);
    });
    cx.simulate_keystrokes("tab");
    draw(cx);
    cx.update(|window, cx| {
        assert!(panel.read(cx).after_session_start_focus[1].is_focused(window));
    });
    cx.simulate_keystrokes("shift-tab");
    draw(cx);
    cx.update(|window, cx| {
        assert!(panel.read(cx).after_session_start_focus[0].is_focused(window));
    });
    for (index, key, expected) in [
        (1, "enter", AfterSessionStart::OpenSession),
        (0, "space", AfterSessionStart::StayOnDashboard),
    ] {
        cx.update(|window, cx| {
            panel.read(cx).after_session_start_focus[index]
                .clone()
                .focus(window, cx);
        });
        cx.simulate_keystrokes(key);
        draw(cx);
        cx.update(|window, cx| {
            assert_eq!(
                cx.global::<SettingsState>().store.after_session_start,
                expected
            );
            assert!(panel.read(cx).after_session_start_focus[index].is_focused(window));
        });
    }
}

#[gpui::test]
fn removed_preferences_preserve_saved_launch_values(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    cx.update(|cx| {
        SettingsState::update(cx, |store| {
            store.default_landing_page = grove_core::storage::LandingPage::History;
            store.default_agent = Some(Agent::Codex);
        });
    });
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.update(|window, cx| panel.update(cx, |panel, cx| panel.open(window, cx)));
    draw(cx);
    assert!(cx.debug_bounds("settings-landing-history").is_none());
    assert!(cx.debug_bounds("settings-archived-projects").is_none());
    assert!(cx
        .debug_bounds("settings-after-session-start-stay")
        .is_none());
    assert!(cx.debug_bounds("settings-shortcuts").is_some());
    cx.update(|_, cx| {
        panel.update(cx, |panel, cx| panel.select_category(Category::Agents, cx));
    });
    draw(cx);
    assert!(cx.debug_bounds("settings-agent-codex").is_none());
    cx.update(|_, cx| {
        assert_eq!(
            cx.global::<SettingsState>().store.default_agent,
            Some(Agent::Codex)
        );
        assert_eq!(
            cx.global::<SettingsState>().store.default_landing_page,
            grove_core::storage::LandingPage::History
        );
    });
}

#[gpui::test]
fn refresh_receipts_fit_narrow_zoomed_settings(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.simulate_resize(gpui::size(gpui::px(320.), gpui::px(600.)));
    cx.update(|window, cx| {
        window.set_rem_size(gpui::px(24.));
        panel.update(cx, |panel, cx| {
            panel.open(window, cx);
            panel.select_category(Category::Terminal, cx);
        });
    });
    let mut action_bounds = None;
    for state in [
        RefreshState::Idle,
        RefreshState::Refreshing,
        RefreshState::Refreshed,
        RefreshState::Error,
    ] {
        cx.update(|_, cx| {
            panel
                .read(cx)
                .runtime
                .read(cx)
                .shell_environment
                .clone()
                .update(cx, |environment, cx| environment.set_fixture(state, cx));
        });
        draw(cx);
        let action = cx.debug_bounds("settings-shell-environment").unwrap();
        let body = cx.debug_bounds("settings-body").unwrap();
        assert!(
            action.left() >= body.left() && action.right() <= body.right(),
            "{state:?}: {action:?} outside {body:?}"
        );
        if let Some(previous) = action_bounds {
            assert_eq!(action, previous, "receipt moved action slot");
        }
        action_bounds = Some(action);
    }
}

#[gpui::test]
fn save_notes_only_appear_on_editable_pages(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.update(|window, cx| panel.update(cx, |panel, cx| panel.open(window, cx)));
    draw(cx);
    assert!(cx.debug_bounds("settings-save-note").is_some());
    for page in [
        Page::Shortcuts,
        Page::Changelog,
        Page::Providers,
        Page::Settings,
    ] {
        cx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.page = page;
                panel.category = Category::About;
                panel
                    .runtime
                    .read(cx)
                    .upgrade
                    .clone()
                    .update(cx, |upgrade, cx| {
                        upgrade.set_fixture(
                            UpgradeState::Idle,
                            ChangelogState::Error("Offline".into()),
                            cx,
                        );
                    });
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("settings-save-note").is_none(), "{page:?}");
    }
}

#[test]
fn terminal_agent_choice_inverts_existing_native_preference() {
    let mut store = Store {
        native_agents_enabled: true,
        ..Store::default()
    };
    assert!(!runs_agents_in_terminal(&store));
    store.native_agents_enabled = false;
    assert!(runs_agents_in_terminal(&store));
}

#[test]
fn appearance_prefers_saved_mode_over_legacy_theme() {
    let mut store = Store {
        theme: Some("catppuccin-latte".into()),
        ..Store::default()
    };
    assert_eq!(store.selected_theme_id(false), "grove-light");
    store.appearance = Some(AppearancePreference::Dark);
    assert_eq!(store.selected_theme_id(false), "grove-dark");
    store.appearance = Some(AppearancePreference::System);
    assert_eq!(store.selected_theme_id(true), "grove-light");
}

#[test]
fn shortcut_reference_includes_zen_but_excludes_unhandled_grid_actions() {
    let visible = handled_shortcuts();
    assert!(visible
        .iter()
        .any(|(_, def)| def.description == "New session"));
    assert!(visible
        .iter()
        .any(|(_, def)| def.description == "Select nth session"));
    assert!(!visible
        .iter()
        .any(|(_, def)| def.description == "Resize grid"));
    assert!(visible
        .iter()
        .any(|(_, def)| def.description == "Toggle zen mode"));
    assert!(!visible
        .iter()
        .any(|(_, def)| def.description == "Move focus in grid"));
}

#[gpui::test]
fn theme_navigation_search_immediate_save_and_retry(cx: &mut gpui::TestAppContext) {
    struct RestoreFile(std::path::PathBuf, std::path::PathBuf);
    impl Drop for RestoreFile {
        fn drop(&mut self) {
            let _ = fs_err::remove_dir(&self.0);
            let _ = fs_err::rename(&self.1, &self.0);
        }
    }
    let _disk = crate::settings::lock_test_config();
    let _lock = c::ACTIVE_THEME_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _restore = c::ActiveThemeGuard::capture();
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.open(window, cx);
            panel.open_nested(Page::Themes, window, cx);
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(900.0), gpui::px(900.0)));
    cx.update(|window, _| window.set_rem_size(gpui::px(crate::zoom::REM_BASE * 1.1)));
    draw(cx);
    let baseline = cx
        .debug_bounds("theme-kind-grove-dark")
        .expect("Grove kind column");
    for (kind_selector, name_selector) in [
        (
            "theme-kind-catppuccin-macchiato",
            "theme-name-catppuccin-macchiato",
        ),
        ("theme-kind-tokyo-night", "theme-name-tokyo-night"),
    ] {
        let kind = cx.debug_bounds(kind_selector).expect("kind column");
        let name = cx.debug_bounds(name_selector).expect("name column");
        assert!(
            (f32::from(kind.origin.x - baseline.origin.x)).abs() < 0.1,
            "kind columns must align"
        );
        assert!(
            name.right() < kind.left(),
            "name must yield to trailing metadata"
        );
    }
    cx.simulate_input("frappe");
    draw(cx);
    cx.simulate_keystrokes("down");
    draw(cx);
    cx.update(|window, cx| {
        assert!(panel.read(cx).theme_focus[4].is_focused(window));
        assert!(
            cx.global::<SettingsState>().store.selected_theme.is_none(),
            "focus must not select"
        );
    });
    cx.simulate_keystrokes("enter");
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
    });
    draw(cx);
    cx.update(|_, cx| {
        assert_eq!(
            cx.global::<SettingsState>().store.selected_theme.as_deref(),
            Some("catppuccin-frappe")
        );
        assert_eq!(grove_core::theme::current().name, "catppuccin-frappe");
        assert_eq!(
            grove_core::storage::load()
                .unwrap()
                .selected_theme
                .as_deref(),
            Some("catppuccin-frappe")
        );
    });
    cx.update(|window, cx| {
        let input = panel.read(cx).theme_search.clone().unwrap();
        input.update(cx, |input, cx| input.set_value("catppuccin", window, cx));
    });
    draw(cx);
    // Force a real atomic-save failure without changing process environment.
    let path = grove_core::storage::config_path().unwrap();
    let backup = path.with_extension("theme-test-backup");
    fs_err::rename(&path, &backup).unwrap();
    fs_err::create_dir(&path).unwrap();
    let restore_file = RestoreFile(path.clone(), backup.clone());
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.select_theme("catppuccin-mocha", window, cx);
        });
    });
    cx.update(|_, cx| {
        assert_eq!(
            panel.read(cx).error.as_deref(),
            Some("Couldn’t save Catppuccin Mocha. Catppuccin Frappé is still active. Try again.")
        );
        assert_eq!(panel.read(cx).theme_retry, Some("catppuccin-mocha"));
        assert_eq!(grove_core::theme::current().name, "catppuccin-frappe");
        assert_eq!(
            cx.global::<SettingsState>().store.selected_theme.as_deref(),
            Some("catppuccin-frappe")
        );
    });
    draw(cx);
    for (kind_selector, text_selector) in [
        (
            "theme-kind-catppuccin-latte",
            "theme-kind-text-catppuccin-latte",
        ),
        (
            "theme-kind-catppuccin-frappe",
            "theme-kind-text-catppuccin-frappe",
        ),
        (
            "theme-kind-catppuccin-macchiato",
            "theme-kind-text-catppuccin-macchiato",
        ),
        (
            "theme-kind-catppuccin-mocha",
            "theme-kind-text-catppuccin-mocha",
        ),
    ] {
        let column = cx
            .debug_bounds(kind_selector)
            .expect("filtered kind column");
        let text = cx
            .debug_bounds(text_selector)
            .expect("intrinsic text layout bounds");
        assert!(
            (f32::from(column.right() - text.right())).abs() < 0.1,
            "metadata text must occupy column trailing edge after search and save error"
        );
    }
    drop(restore_file);
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.select_theme("catppuccin-mocha", window, cx);
        });
    });
    cx.update(|window, cx| {
        assert!(panel.read(cx).error.is_none());
        assert!(panel.read(cx).theme_retry.is_none());
        assert!(panel.read(cx).theme_focus[6].is_focused(window));
        assert_eq!(
            cx.global::<ThemeState>().generation,
            grove_core::theme::generation()
        );
    });
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| assert_eq!(panel.read(cx).page, Page::Settings));
}

#[gpui::test]
fn shortcuts_escape_closes_page_and_restores_focus(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    let prior = cx.update(|window, cx| {
        let prior = cx.focus_handle();
        prior.focus(window, cx);
        panel.update(cx, |panel, cx| panel.open_shortcuts(window, cx));
        prior
    });
    cx.simulate_resize(gpui::size(gpui::px(320.0), gpui::px(200.0)));
    draw(cx);
    let bounds = cx.debug_bounds("settings-panel").expect("settings panel");
    assert_eq!(f32::from(bounds.size.width), 320.0);
    assert_eq!(f32::from(bounds.size.height), 200.0);
    assert!(cx.debug_bounds("settings-overlay").is_none());
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        assert!(!panel.read(cx).is_open());
        assert!(prior.is_focused(window));
    });
}

#[gpui::test]
fn nested_shortcuts_after_direct_shortcuts_return_to_general(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.open_shortcuts(window, cx);
            panel.back(window, cx);
            panel.open_nested(Page::Shortcuts, window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|_, cx| {
        assert!(panel.read(cx).is_open());
        assert_eq!(panel.read(cx).page, Page::Settings);
        assert_eq!(panel.read(cx).category, Category::General);
    });
}

#[gpui::test]
fn category_navigation_activates_with_keyboard_and_resets_scroll(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.update(|window, cx| panel.update(cx, |panel, cx| panel.open(window, cx)));
    draw(cx);
    // The Close control precedes the category rail, where arrows move between categories.
    for chord in ["tab", "tab", "down", "enter"] {
        cx.simulate_keystrokes(chord);
        draw(cx);
    }
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
    });
    draw(cx);
    cx.update(|_, cx| {
        assert_eq!(panel.read(cx).category, Category::Agents);
        assert_eq!(panel.read(cx).page, Page::Settings);
        assert_eq!(panel.read(cx).scroll.offset(), gpui::Point::default());
    });
}

#[gpui::test]
fn settings_header_spans_the_canvas_at_zoom_and_short_heights(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    for (zoom, width, height) in [
        (1.0, 1280.0, 800.0),
        (1.1, 500.0, 200.0),
        (1.5, 600.0, 600.0),
    ] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
        cx.update(|window, _| window.set_rem_size(gpui::px(crate::zoom::REM_BASE * zoom)));
        for page in [Page::Settings, Page::Providers, Page::Themes] {
            cx.update(|window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.open(window, cx);
                    if page == Page::Settings {
                        panel.show(page, cx);
                    } else {
                        panel.open_nested(page, window, cx);
                    }
                });
            });
            draw(cx);
            let canvas = cx.debug_bounds("settings-panel").expect("Settings canvas");
            let heading = cx
                .debug_bounds("settings-heading")
                .expect("settings heading");
            assert_eq!(heading.top(), canvas.top());
            assert_eq!(heading.left(), canvas.left());
            assert_eq!(heading.right(), canvas.right());
            for selector in ["settings-close", "settings-back"] {
                if selector == "settings-back" && page == Page::Settings {
                    continue;
                }
                let control = cx.debug_bounds(selector).expect("settings header control");
                assert!(
                    control.top() > canvas.top(),
                    "{selector} must stay inset from the canvas top at {zoom}x"
                );
                assert!(
                    control.bottom() <= gpui::px(height),
                    "{selector} clips outside short window at {zoom}x"
                );
            }
            let title = cx
                .debug_bounds("settings-heading-title")
                .expect("settings title");
            let close = cx.debug_bounds("settings-close").unwrap();
            assert!(title.right() <= close.left(), "title must yield to Close");
            if page != Page::Settings {
                let back = cx.debug_bounds("settings-back").unwrap();
                assert!(
                    back.top() >= heading.bottom(),
                    "Back stays under the Settings header"
                );
                assert!(back.right() <= gpui::px(width));
            }
            let scroll = cx
                .debug_bounds("settings-scroll")
                .expect("settings body viewport");
            assert!(scroll.top() >= heading.bottom());
            cx.update(|_, cx| {
                panel
                    .read(cx)
                    .scroll
                    .set_offset(gpui::point(gpui::px(0.0), gpui::px(-120.0)));
            });
            draw(cx);
            assert_eq!(
                cx.debug_bounds("settings-heading"),
                Some(heading),
                "body scrolling moved page header"
            );
        }
    }
}

#[gpui::test]
fn settings_body_scrolls_while_heading_and_category_picker_stay_reachable(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.update(|window, cx| panel.update(cx, |panel, cx| panel.open(window, cx)));
    cx.simulate_resize(gpui::size(gpui::px(320.0), gpui::px(200.0)));
    draw(cx);

    let scroll = cx.debug_bounds("settings-scroll").expect("scroll viewport");
    let heading = cx.debug_bounds("settings-heading").expect("local heading");
    let picker = cx
        .debug_bounds("settings-category-navigation")
        .expect("compact category navigation");
    assert!(cx.debug_bounds("settings-category-picker").is_some());
    let close = cx.debug_bounds("settings-close").expect("close");
    assert_eq!(picker.left(), heading.left());
    assert_eq!(picker.right(), heading.right());
    let picker_control = cx
        .debug_bounds("settings-category-picker")
        .expect("compact category picker");
    let body = cx.debug_bounds("settings-body").expect("body");
    assert_eq!(picker_control.left(), body.left());
    assert_eq!(picker_control.right(), body.right());
    assert_eq!(picker.bottom(), scroll.top());
    assert!(scroll.bottom() <= gpui::px(200.));
    assert!(close.top() >= heading.top() && close.bottom() <= heading.bottom());
    assert!(cx.debug_bounds("settings-footer").is_none());
    assert!(panel.read_with(cx, |panel, _| panel.scroll.max_offset().y) > gpui::px(0.));

    cx.simulate_event(gpui::ScrollWheelEvent {
        position: scroll.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-120.))),
        modifiers: gpui::Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    draw(cx);
    assert!(panel.read_with(cx, |panel, _| panel.scroll.offset().y) < gpui::px(0.));
    assert_eq!(cx.debug_bounds("settings-heading"), Some(heading));
    assert_eq!(
        cx.debug_bounds("settings-category-navigation"),
        Some(picker)
    );
    assert_eq!(cx.debug_bounds("settings-close"), Some(close));
    cx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.error = Some("Could not save settings.".into());
            cx.notify();
        });
    });
    draw(cx);
    let error = cx
        .debug_bounds("settings-error")
        .expect("visible save error");
    let scroll = cx.debug_bounds("settings-scroll").expect("bounded scroll");
    assert!(error.top() >= picker.bottom());
    assert_eq!(error.bottom(), scroll.top());
    assert!(error.bottom() < gpui::px(200.));
    assert_eq!(cx.debug_bounds("settings-close"), Some(close));
}

#[gpui::test]
fn desktop_settings_anchors_the_rail_and_centers_the_body(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.update(|window, cx| panel.update(cx, |panel, cx| panel.open(window, cx)));
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    draw(cx);
    let content = cx.debug_bounds("settings-content").expect("content column");
    let heading = cx.debug_bounds("settings-heading").expect("heading");
    let rail = cx
        .debug_bounds("settings-category-navigation")
        .expect("wide category navigation");
    assert!(cx.debug_bounds("settings-category-picker").is_none());
    let body = cx.debug_bounds("settings-body").expect("body");
    let general = cx
        .debug_bounds("settings-category-General")
        .expect("General category");
    let agents = cx
        .debug_bounds("settings-category-Agents")
        .expect("Agents category");
    assert_eq!(content.size.width, gpui::px(1280.));
    assert_eq!(heading.left(), rail.left());
    assert_eq!(heading.right(), content.right());
    assert_eq!(rail.size.width, gpui::px(CATEGORY_RAIL_W));
    assert_eq!(heading.bottom(), rail.top());
    assert_eq!(rail.bottom(), content.bottom());
    assert_eq!(body.size.width, gpui::px(SETTINGS_PAGE_W));
    assert_eq!(body.left() - rail.right(), heading.right() - body.right());
    assert_eq!(general.left(), agents.left());
    assert!(general.left() > rail.left());
    assert!(general.right() < rail.right());
    assert!(general.bottom() < agents.top());
}

#[gpui::test]
fn compact_settings_menu_dismisses_to_its_trigger_and_switches_categories(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.simulate_resize(gpui::size(gpui::px(500.), gpui::px(400.)));
    cx.update(|window, cx| panel.update(cx, |panel, cx| panel.open(window, cx)));
    draw(cx);
    let picker = cx
        .debug_bounds("settings-category-picker")
        .expect("compact picker");
    cx.simulate_click(picker.center(), gpui::Modifiers::default());
    draw(cx);
    cx.update(|_, cx| assert!(panel.read(cx).category_menu.is_some()));
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        assert!(panel.read(cx).is_open());
        assert!(panel.read(cx).category_menu.is_none());
        assert!(panel.read(cx).category_trigger_focus.is_focused(window));
    });

    cx.simulate_click(picker.center(), gpui::Modifiers::default());
    draw(cx);
    cx.simulate_keystrokes("down down enter");
    draw(cx);
    cx.update(|window, cx| {
        assert_eq!(panel.read(cx).category, Category::Agents);
        assert!(panel.read(cx).category_menu.is_none());
        assert!(panel.read(cx).category_trigger_focus.is_focused(window));
        assert_eq!(panel.read(cx).scroll.offset(), gpui::Point::default());
    });
}

#[gpui::test]
fn provider_notice_opens_agents_nested_page_and_escape_returns(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| panel.open_provider_updates(window, cx));
        assert_eq!(panel.read(cx).page, Page::Providers);
        assert_eq!(panel.read(cx).category, Category::Agents);
    });
    draw(cx);
    assert!(
        cx.debug_bounds("settings-save-note").is_none(),
        "Provider versions has no save-preference footer"
    );
    cx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.toggle_provider_details(Agent::Codex, cx);
        });
        let updates = panel.read(cx).runtime.read(cx).provider_updates.clone();
        updates.update(cx, |_, cx| cx.notify());
    });
    draw(cx);
    cx.update(|_, cx| {
        assert!(
            panel
                .read(cx)
                .provider_details
                .contains(Agent::Codex.label()),
            "Refresh preserves disclosure by provider identity"
        );
    });
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|_, cx| {
        assert!(panel.read(cx).is_open());
        assert_eq!(panel.read(cx).page, Page::Settings);
        assert_eq!(panel.read(cx).category, Category::Agents);
    });
}

#[gpui::test]
fn provider_rows_keep_codex_and_claude_actions_compact_and_errors_keyboard_accessible(
    cx: &mut gpui::TestAppContext,
) {
    use crate::entities::provider_updates::ProviderRow;
    use grove_core::provider_updates::{Installer, ProviderStatus};
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| panel.open_provider_updates(window, cx));
        let updates = panel.read(cx).runtime.read(cx).provider_updates.clone();
        updates.update(cx, |updates, cx| {
            updates.set_rows_for_test(
                [Agent::Codex, Agent::Claude]
                    .into_iter()
                    .map(|agent| ProviderRow {
                        status: ProviderStatus {
                            agent,
                            installed: Some("1.0.0".into()),
                            latest: Some("1.1.0".into()),
                            executable: Some("/fixture/provider".into()),
                            error: None,
                            installer: if agent == Agent::Claude {
                                Installer::ClaudeNative
                            } else {
                                Installer::Npm {
                                    executable: "/fixture/npm".into(),
                                    prefix: "/fixture".into(),
                                    package: "@openai/codex".into(),
                                }
                            },
                        },
                        checking: false,
                        updating: false,
                        updated: false,
                        update_verified: false,
                        last_success: Some(1),
                        error: if agent == Agent::Claude {
                            Some("Permission denied".into())
                        } else {
                            None
                        },
                    })
                    .collect(),
                cx,
            );
        });
    });
    draw(cx);
    let codex = cx
        .debug_bounds("settings-provider-codex-update")
        .expect("Codex Update");
    let claude = cx
        .debug_bounds("settings-provider-claude-update")
        .expect("Claude Retry");
    assert_eq!(
        codex.right(),
        claude.right(),
        "Both providers reserve the same action edge"
    );
    assert_eq!(codex.size.height, claude.size.height);
    let manage = cx.debug_bounds("settings-provider-claude-manage").unwrap();
    cx.simulate_click(manage.center(), gpui::Modifiers::default());
    draw(cx);
    let details = cx
        .debug_bounds("settings-provider-claude-details")
        .expect("error disclosure");
    cx.simulate_click(details.center(), gpui::Modifiers::default());
    draw(cx);
    cx.update(|_, cx| {
        assert!(panel
            .read(cx)
            .provider_details
            .contains(Agent::Claude.label()));
    });
    cx.simulate_keystrokes("space");
    draw(cx);
    cx.update(|_, cx| {
        assert!(!panel
            .read(cx)
            .provider_details
            .contains(Agent::Claude.label()));
    });
    assert!(
        cx.debug_bounds("settings-provider-codex-details").is_none(),
        "No routine disclosure"
    );
}

#[gpui::test]
fn provider_page_back_restores_source_focus(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.open(window, cx);
            panel.select_category(Category::Agents, cx);
            let source = cx.focus_handle().tab_stop(true);
            source.focus(window, cx);
            panel.open_nested(Page::Providers, window, cx);
            assert!(panel
                .nested_return_focus
                .as_ref()
                .is_some_and(|focus| focus == &source));
            panel.back(window, cx);
            assert!(source.is_focused(window));
            assert_eq!(panel.category, Category::Agents);
        });
    });
}

#[gpui::test]
fn nested_settings_page_escape_returns_to_settings(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (panel, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.open(window, cx);
            panel.select_category(Category::About, cx);
            panel.open_nested(Page::Changelog, window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|_, cx| {
        assert!(panel.read(cx).is_open());
        assert_eq!(panel.read(cx).page, Page::Settings);
        assert_eq!(panel.read(cx).category, Category::About);
    });
}

#[gpui::test]
fn tab_and_shift_tab_stay_inside_open_panel(cx: &mut gpui::TestAppContext) {
    cx.update(setup);
    let (fixture, cx) = cx.add_window_view(|_, cx| {
        let runtime = cx.new(Runtime::new);
        FocusFixture {
            panel: cx.new(|cx| SettingsPanel::new(runtime, cx)),
            behind: cx.focus_handle().tab_stop(true),
        }
    });
    let (panel, behind) = fixture.read_with(cx, |fixture, _| {
        (fixture.panel.clone(), fixture.behind.clone())
    });
    cx.update(|window, cx| {
        behind.focus(window, cx);
        panel.update(cx, |panel, cx| panel.open_shortcuts(window, cx));
    });
    draw(cx);
    for chord in ["tab", "tab", "shift-tab", "shift-tab"] {
        cx.simulate_keystrokes(chord);
        draw(cx);
        cx.update(|window, cx| {
            assert!(!behind.is_focused(window), "{chord} escaped the page");
            assert!(panel.read(cx).focus.contains_focused(window, cx));
        });
    }
}
