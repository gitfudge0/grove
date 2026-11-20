//! Isolated offscreen Settings captures for macOS visual acceptance.
use super::{fixture_git_project, ProjectMenuTarget, RowAction};
use crate::{
    runtime::Runtime,
    settings::SettingsState,
    theme::{self as c, ThemeState},
    views::settings_panel::SettingsPanel,
    zoom::{CurrentPtyDims, ZoomState, REM_BASE},
};
use gpui::{prelude::*, px, AppContext, HeadlessAppContext, WindowHandle};
use grove_core::storage::{Project, ProjectScripts, Store};
use std::{path::PathBuf, sync::Arc, time::Duration};

#[test]
#[ignore = "requires the macOS Metal renderer and a task-owned GROVE_SETTINGS_QA_OUTPUT directory"]
fn settings_native_snapshots() {
    let output = PathBuf::from(
        std::env::var_os("GROVE_SETTINGS_QA_OUTPUT").expect("GROVE_SETTINGS_QA_OUTPUT"),
    );
    assert!(output.is_absolute());
    std::fs::create_dir_all(&output).unwrap();
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

    for (light, rem, width) in [(false, 16., 992.), (true, 20., 450.)] {
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
            let mut store = Store {
                theme: Some(theme.into()),
                ..Store::default()
            };
            for name in ["grove", "docs", "skills", "atlas-web", "release-tools"] {
                store.projects.push(Project {
                    name: name.into(),
                    path: root.join(name).to_str().unwrap().to_owned(),
                    scripts: ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                });
            }
            cx.set_global(SettingsState::new(store));
        });
        let panel = cx.new(|cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        let view = panel.clone();
        let window = cx
            .open_window(gpui::size(px(width), px(820.)), move |window, cx| {
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
        if !light {
            capture(&mut cx, window, output.join("general-wide-dark.png"));
        }
        cx.update_window(window.into(), |_, window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open_projects(None, window, cx);
                panel.expanded_projects.insert(repo.into());
                let runtime = panel.runtime.read(cx);
                let active = runtime.state.read(cx).proj_idx();
                let tree = runtime.tree.clone();
                tree.update(cx, |tree, cx| {
                    tree.register_project_worktrees(
                        0,
                        active,
                        grove_core::git::list_worktrees_checked(repo).unwrap(),
                    );
                    cx.notify();
                });
            });
        })
        .unwrap();
        let filename = if light {
            "projects-compact-light-zoom.png"
        } else {
            "projects-wide-dark.png"
        };
        capture(&mut cx, window, output.join(filename));
        let target = ProjectMenuTarget::worktree(repo, worktree);
        if !light {
            cx.update_window(window.into(), |_, window, cx| {
                panel.update(cx, |panel, cx| {
                    // Test the real menu paint at a fixture anchor near the row.
                    // The regular GPUI tests exercise the route from the measured trigger.
                    let bounds = gpui::Bounds {
                        origin: gpui::point(px(width - 116.), px(248.)),
                        size: gpui::size(px(24.), px(24.)),
                    };
                    panel.open_project_row_menu(target.clone(), bounds, window, cx);
                    // The fixture positions its offscreen menu explicitly.
                    // Interaction coverage compares an actual trigger’s bounds.
                    panel.project_menu.as_mut().unwrap().anchor_bounds = None;
                });
            })
            .unwrap();
            capture(&mut cx, window, output.join("projects-menu-wide-dark.png"));
        }
        cx.update_window(window.into(), |_, window, cx| {
            panel.update(cx, |panel, cx| {
                panel.apply_project_row_action(target, RowAction::Delete, window, cx);
            });
        })
        .unwrap();
        let filename = if light {
            "delete-worktree-compact-light-zoom.png"
        } else {
            "delete-worktree-wide-dark.png"
        };
        capture(&mut cx, window, output.join(filename));
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
    }
}

fn capture(cx: &mut HeadlessAppContext, window: WindowHandle<gpui_component::Root>, path: PathBuf) {
    for _ in 0..3 {
        cx.advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            let _ = window.draw(cx);
        })
        .unwrap();
    }
    cx.capture_screenshot(window.into())
        .unwrap()
        .save(&path)
        .unwrap();
    eprintln!("Native Settings capture: {}", path.display());
}
