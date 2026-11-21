//! Isolated offscreen Settings captures for macOS visual acceptance.
use super::super::{Category, Page};
use super::{fixture_git_project, ProjectMenuTarget, RowAction};
use crate::{
    entities::provider_updates::ProviderRow,
    runtime::Runtime,
    settings::SettingsState,
    theme::{self as c, ThemeState},
    views::settings_panel::SettingsPanel,
    zoom::{CurrentPtyDims, ZoomState, REM_BASE},
};
use gpui::{
    prelude::*, px, AppContext, Context, HeadlessAppContext, Keystroke, Window, WindowHandle,
};
use grove_core::{
    agent::Agent,
    git::Worktree,
    provider_updates::{Installer, ProviderStatus},
    storage::{Project, ProjectScripts, Store},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

type CaptureWindow = WindowHandle<gpui_component::Root>;

#[test]
#[ignore = "requires the macOS Metal renderer and a task-owned GROVE_SETTINGS_QA_OUTPUT directory"]
fn settings_native_snapshots() {
    let output = PathBuf::from(
        std::env::var_os("GROVE_SETTINGS_QA_OUTPUT").expect("GROVE_SETTINGS_QA_OUTPUT"),
    );
    assert!(output.is_absolute());
    std::fs::create_dir_all(&output).unwrap();

    // Match the approved prototype's Settings panes, excluding document chrome and the mock note.
    for (width, height, suffix) in [(974., 748., "wide-dark"), (390., 767., "compact-dark")] {
        let mut store = Store::default();
        store.workspaces.rows[0].name = "grove".into();
        let root = Path::new("/Fixture");
        for name in ["grove", "docs", "skills", "atlas-web", "release-tools"] {
            add_project(&mut store, root, name, false);
        }
        for name in ["legacy-docs", "preview-kit"] {
            add_project(&mut store, root, name, true);
        }
        let (mut cx, window, panel) = open_settings(store, width, height, false, REM_BASE);
        capture(
            &mut cx,
            window,
            output.join(format!("general-{suffix}.png")),
        );

        let repo = "/Fixture/grove";
        let worktree = "/Fixture/worktrees/grove/settings-layout";
        let worktrees = [
            (repo, "main", true),
            (worktree, "settings-responsive-layout", false),
            (
                "/Fixture/worktrees/grove/terminal-startup",
                "terminal-startup",
                false,
            ),
        ]
        .map(|(path, branch, is_main)| Worktree {
            path: path.into(),
            branch: branch.into(),
            mtime: None,
            is_main,
        });
        cx.update_window(window.into(), |_, window, cx| {
            panel.update(cx, |panel, cx| {
                open_populated_projects(panel, repo, worktrees.to_vec(), window, cx);
            });
        })
        .unwrap();
        capture(
            &mut cx,
            window,
            output.join(format!("projects-{suffix}.png")),
        );

        let target = ProjectMenuTarget::worktree(repo, worktree);
        cx.update_window(window.into(), |_, window, cx| {
            let trigger = panel.read(cx).project_menu_focus[&target].clone();
            trigger.focus(window, cx);
        })
        .unwrap();
        settle(&mut cx, window);
        cx.update_window(window.into(), |_, window, cx| {
            assert!(window.dispatch_keystroke(Keystroke::parse("down").unwrap(), cx));
        })
        .unwrap();
        capture(
            &mut cx,
            window,
            output.join(format!("projects-menu-{suffix}.png")),
        );
        cx.update_window(window.into(), |_, window, cx| {
            let menu = panel.read(cx).project_menu.as_ref().unwrap();
            assert!(menu.target == target);
            assert!(menu
                .anchor_bounds
                .is_some_and(|bounds| bounds.size.width > px(0.)));
            assert!(window.dispatch_keystroke(Keystroke::parse("escape").unwrap(), cx));
        })
        .unwrap();
        settle(&mut cx, window);
        cx.update_window(window.into(), |_, _, cx| {
            panel.update(cx, |panel, cx| {
                assert!(panel.project_menu.is_none());
                panel.projects_archived = true;
                cx.notify();
            });
        })
        .unwrap();
        capture(
            &mut cx,
            window,
            output.join(format!("projects-archived-{suffix}.png")),
        );

        cx.update_window(window.into(), |_, window, cx| {
            panel.update(cx, |panel, cx| {
                panel.select_category(Category::Agents, cx);
                if width < 400. {
                    panel.toggle_category_menu(window, cx);
                }
            });
        })
        .unwrap();
        if width < 400. {
            capture(
                &mut cx,
                window,
                output.join("category-picker-compact-dark.png"),
            );
            cx.update_window(window.into(), |_, window, cx| {
                panel.update(cx, |panel, cx| panel.dismiss_category_menu(window, cx));
            })
            .unwrap();
        }
        capture(&mut cx, window, output.join(format!("agents-{suffix}.png")));
        cx.update_window(window.into(), |_, window, cx| {
            panel.update(cx, |panel, cx| {
                fixture_providers(panel, window, cx);
                // Render the native page without initiating live provider account refresh.
                panel.show(Page::Providers, cx);
            });
        })
        .unwrap();
        capture(
            &mut cx,
            window,
            output.join(format!("providers-{suffix}.png")),
        );
        close(&mut cx, window);
    }

    // Long paths and real Git metadata for the destructive confirmation state.
    let fixture = tempfile::Builder::new()
        .prefix("grove-settings-")
        .tempdir()
        .unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let repo = root.join("grove");
    let worktree = root.join("settings-layout");
    let other_worktree = root.join("terminal-startup");
    fixture_git_project(
        &repo,
        &[
            ("settings-responsive-layout", &worktree),
            ("terminal-startup", &other_worktree),
        ],
    );
    let repo = repo.to_str().unwrap();
    let worktree = worktree.to_str().unwrap();
    let mut store = Store::default();
    for name in ["grove", "docs", "skills", "atlas-web", "release-tools"] {
        add_project(&mut store, &root, name, false);
    }
    for (width, height, light, rem, suffix) in [
        (974., 748., false, REM_BASE, "wide-dark"),
        (450., 820., true, 20., "compact-light-zoom"),
        (450., 305., true, 20., "short-compact-light-zoom"),
    ] {
        let (mut cx, window, panel) = open_settings(store.clone(), width, height, light, rem);
        cx.update_window(window.into(), |_, window, cx| {
            panel.update(cx, |panel, cx| {
                open_populated_projects(
                    panel,
                    repo,
                    grove_core::git::list_worktrees_checked(repo).unwrap(),
                    window,
                    cx,
                );
            });
        })
        .unwrap();
        if suffix == "compact-light-zoom" {
            capture(
                &mut cx,
                window,
                output.join(format!("projects-{suffix}.png")),
            );
        }
        cx.update_window(window.into(), |_, window, cx| {
            panel.update(cx, |panel, cx| {
                panel.apply_project_row_action(
                    ProjectMenuTarget::worktree(repo, worktree),
                    RowAction::Delete,
                    window,
                    cx,
                );
                assert_eq!(panel.page, Page::Project);
            });
        })
        .unwrap();
        capture(
            &mut cx,
            window,
            output.join(format!("delete-worktree-{suffix}.png")),
        );
        if suffix == "compact-light-zoom" {
            cx.update_window(window.into(), |_, window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.back(window, cx);
                    panel.apply_project_row_action(
                        ProjectMenuTarget::project(repo),
                        RowAction::Remove,
                        window,
                        cx,
                    );
                    assert_eq!(panel.page, Page::Project);
                });
            })
            .unwrap();
            capture(
                &mut cx,
                window,
                output.join(format!("remove-project-keep-files-{suffix}.png")),
            );
        }
        close(&mut cx, window);
    }
}

fn add_project(store: &mut Store, root: &Path, name: &str, archived: bool) {
    store.projects.push(Project {
        name: name.into(),
        path: root.join(name).to_str().unwrap().to_owned(),
        scripts: ProjectScripts::default(),
        archived,
        worktree_dir: None,
    });
}

fn open_settings(
    mut store: Store,
    width: f32,
    height: f32,
    light: bool,
    rem: f32,
) -> (
    HeadlessAppContext,
    CaptureWindow,
    gpui::Entity<SettingsPanel>,
) {
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        cx.set_reduce_motion(true);
        let theme = if light {
            c::DEFAULT_LIGHT_THEME
        } else {
            c::DEFAULT_DARK_THEME
        };
        assert!(grove_core::theme::set_by_name(theme));
        cx.set_global(ThemeState::new(
            false,
            c::DEFAULT_DARK_THEME.into(),
            c::DEFAULT_LIGHT_THEME.into(),
        ));
        c::sync_component_theme(cx);
        gpui_component::Theme::global_mut(cx).font_size = px(rem);
        cx.set_global(ZoomState::new(rem / REM_BASE));
        cx.set_global(CurrentPtyDims::default());
        store.theme = Some(theme.into());
        cx.set_global(SettingsState::new(store));
    });
    let panel = cx.new(|cx| {
        let runtime = cx.new(Runtime::new);
        SettingsPanel::new(runtime, cx)
    });
    let view = panel.clone();
    let window = cx
        .open_window(gpui::size(px(width), px(height)), move |window, cx| {
            view.update(cx, |panel, cx| panel.open(window, cx));
            cx.new(|cx| {
                gpui_component::Root::new(view, window, cx)
                    .bordered(false)
                    .font_family(crate::fonts::UI_FAMILY)
                    .bg(c::BG())
                    .text_color(c::FG())
            })
        })
        .unwrap();
    (cx, window, panel)
}

fn open_populated_projects(
    panel: &mut SettingsPanel,
    repo: &str,
    worktrees: Vec<Worktree>,
    window: &mut Window,
    cx: &mut Context<SettingsPanel>,
) {
    panel.open_projects(None, window, cx);
    panel.expanded_projects.insert(repo.into());
    let runtime = panel.runtime.read(cx);
    let active = runtime.state.read(cx).proj_idx();
    let tree = runtime.tree.clone();
    tree.update(cx, |tree, cx| {
        tree.register_project_worktrees(0, active, worktrees);
        cx.notify();
    });
}

fn fixture_providers(
    panel: &mut SettingsPanel,
    window: &mut Window,
    cx: &mut Context<SettingsPanel>,
) {
    for (index, scenario) in ["connected", "signed-out"].into_iter().enumerate() {
        panel.provider_accounts[index]
            .update(cx, |account, cx| account.fixture(scenario, window, cx));
    }
    panel
        .runtime
        .read(cx)
        .provider_updates
        .clone()
        .update(cx, |updates, cx| {
            let rows = [
                (
                    Agent::Codex,
                    "1.0.0",
                    "1.0.0",
                    Installer::External {
                        guidance: "Managed outside Grove.".into(),
                    },
                ),
                (Agent::Claude, "1.0.0", "1.1.0", Installer::ClaudeNative),
            ];
            updates.set_rows_for_test(
                rows.into_iter()
                    .map(|(agent, installed, latest, installer)| ProviderRow {
                        status: ProviderStatus {
                            agent,
                            installed: Some(installed.into()),
                            latest: Some(latest.into()),
                            executable: Some("/Fixture/provider".into()),
                            error: None,
                            installer,
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

fn settle(cx: &mut HeadlessAppContext, window: CaptureWindow) {
    for _ in 0..3 {
        cx.advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        })
        .unwrap();
    }
}

fn capture(cx: &mut HeadlessAppContext, window: CaptureWindow, path: PathBuf) {
    settle(cx, window);
    cx.capture_screenshot(window.into())
        .unwrap()
        .save(&path)
        .unwrap();
    eprintln!("Native Settings capture: {}", path.display());
}

fn close(cx: &mut HeadlessAppContext, window: CaptureWindow) {
    cx.update_window(window.into(), |_, window, _| window.remove_window())
        .unwrap();
}
