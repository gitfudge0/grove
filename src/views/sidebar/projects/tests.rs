use super::*;
struct ButtonRecipeProbe;
impl Render for ButtonRecipeProbe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().flex_col().items_start().children([
            div()
                .debug_selector(|| "recipe-mini".into())
                .child(project_action(
                    "mini-probe".into(),
                    "Browse",
                    Some("icons/folder.svg"),
                    ProjectActionStyle::Mini,
                    window,
                    cx,
                )),
            div().debug_selector(|| "recipe-dense".into()).child(
                crate::views::components::transcript_action(
                    "dense-probe",
                    "Browse",
                    false,
                    window,
                    cx,
                )
                .icon(gpui_component::Icon::default().path("icons/folder.svg")),
            ),
            div()
                .debug_selector(|| "recipe-browse".into())
                .child(super::super::project_setup::browse_action(window, cx)),
            div()
                .debug_selector(|| "recipe-form".into())
                .child(project_action(
                    "form-probe".into(),
                    "Browse",
                    Some("icons/folder.svg"),
                    ProjectActionStyle::Primary,
                    window,
                    cx,
                )),
        ])
    }
}
#[test]
#[ignore = "requires macOS Metal renderer; writes project script editor screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn project_scripts_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        std::sync::Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        c::sync_component_theme(cx);
        cx.set_reduce_motion(true);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "Workflow fixture".into(),
                path: "/tmp/grove-script-editor-fixture".into(),
                archived: false,
                worktree_dir: None,
                scripts: ProjectScripts {
                    setup: Some("cargo fetch".into()),
                    entries: vec![
                        ProjectAction {
                            id: "dev".into(),
                            name: "Development server with a deliberately long descriptive name"
                                .into(),
                            kind: ProjectActionKind::Shell {
                                command: "npm run dev".into(),
                            },
                        },
                        ProjectAction {
                            id: "review".into(),
                            name: "PR review".into(),
                            kind: ProjectActionKind::Prompt {
                                template: "Review the changes carefully".into(),
                            },
                        },
                    ],
                    review_prompt_id: Some("review".into()),
                    ..Default::default()
                },
            }],
            ..Default::default()
        }));
    });
    for (width, rem, expanded) in [
        (768., 16., None),
        (1280., 16., Some(ScriptEditor::Action("review".into()))),
        (768., 20., Some(ScriptEditor::Action("dev".into()))),
        (420., 16., Some(ScriptEditor::Setup)),
        (420., 16., Some(ScriptEditor::Action("dev".into()))),
        (420., 16., Some(ScriptEditor::Action("review".into()))),
    ] {
        let state_name = match &expanded {
            None => "collapsed",
            Some(ScriptEditor::Setup) => "setup",
            Some(ScriptEditor::Action(id)) => id.as_str(),
            _ => "other",
        }
        .to_owned();
        let window = cx
            .open_window(
                gpui::size(gpui::px(width), gpui::px(1200.)),
                |window, cx| {
                    window.set_rem_size(gpui::px(rem));
                    let registry = cx.new(|_| SessionRegistry::new());
                    let state = cx.new(|cx| {
                        crate::entities::workspace_state::WorkspaceState::new(
                            &cx.global::<SettingsState>().store,
                            MODAL_W_LG,
                        )
                    });
                    let service = cx.new(|_| ProjectService::new(registry.clone(), state));
                    let panel = cx.new(|cx| {
                        ProjectPanel::new(
                            Page::Edit("/tmp/grove-script-editor-fixture".into()),
                            service,
                            registry,
                            window,
                            cx,
                        )
                    });
                    panel.update(cx, |panel, _| panel.expanded_script = expanded.clone());
                    cx.new(|cx| {
                        gpui_component::Root::new(panel, window, cx)
                            .bordered(false)
                            .bg(c::BG())
                    })
                },
            )
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            let _ = window.draw(cx);
        })
        .unwrap();
        let path = format!("/tmp/grove-project-register-{width:.0}-{rem:.0}-{state_name}.png");
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(&path)
            .unwrap();
        eprintln!("Project scripts native fixture: {path}");
        if width == 768. && rem == 16. && expanded.is_none() {
            // Setup's center in this fixed native fixture, observed in the rest capture.
            cx.update_window(window.into(), |_, window, cx| {
                window.dispatch_event(
                    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position: gpui::point(gpui::px(384.), gpui::px(344.)),
                        pressed_button: None,
                        modifiers: gpui::Modifiers::default(),
                    }),
                    cx,
                );
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
            let hover_path = path.replace(".png", "-hover.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&hover_path)
                .unwrap();
            eprintln!("Project scripts native hover fixture: {hover_path}");
        }
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
    }
}

#[gpui::test]
fn named_script_editor_renders_shell_and_prompt_entries(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "Workflow fixture".into(),
                path: "/tmp/grove-script-editor-fixture".into(),
                archived: false,
                worktree_dir: None,
                scripts: ProjectScripts {
                    entries: vec![
                        ProjectAction {
                            id: "dev".into(),
                            name: "Development server with a deliberately long descriptive name"
                                .into(),
                            kind: ProjectActionKind::Shell {
                                command: "npm run dev".into(),
                            },
                        },
                        ProjectAction {
                            id: "review".into(),
                            name: "PR review".into(),
                            kind: ProjectActionKind::Prompt {
                                template: "Review the changes carefully".into(),
                            },
                        },
                    ],
                    review_prompt_id: Some("review".into()),
                    ..Default::default()
                },
            }],
            ..Default::default()
        }));
    });
    let (panel, cx) = cx.add_window_view(|window, cx| {
        let registry = cx.new(|_| SessionRegistry::new());
        let state = cx.new(|cx| {
            crate::entities::workspace_state::WorkspaceState::new(
                &cx.global::<SettingsState>().store,
                MODAL_W_LG,
            )
        });
        let service = cx.new(|_| ProjectService::new(registry.clone(), state));
        ProjectPanel::new(
            Page::Edit("/tmp/grove-script-editor-fixture".into()),
            service,
            registry,
            window,
            cx,
        )
    });
    for width in [768., 1280.] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1200.)));
        draw(cx);
        assert!(cx.debug_bounds("Shell command").is_none());
        assert!(cx.debug_bounds("Prompt").is_none());
        for (id, label) in [("dev", "Shell command"), ("review", "Prompt")] {
            cx.update(|_, cx| {
                panel.update(cx, |panel, cx| {
                    panel.toggle_script(ScriptEditor::Action(id.into()), cx);
                });
            });
            draw(cx);
            let hint = cx.debug_bounds("script-execution-hint").unwrap();
            let name = cx.debug_bounds("script-entry-name").unwrap();
            let content = cx.debug_bounds(label).unwrap();
            assert!(
                hint.bottom() <= name.top(),
                "execution context precedes Name"
            );
            assert!(
                name.bottom() <= content.top(),
                "Name precedes script content"
            );
        }
        cx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.expanded_script = None;
                cx.notify();
            });
        });
        panel.read_with(cx, |panel, _| {
            assert_eq!(panel.script_entries.len(), 2);
        });
    }
    // Disclosure and type changes preserve the same unsaved input entities.
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.toggle_script(ScriptEditor::Action("review".into()), cx);
            let review = &panel.script_entries[1];
            review
                .2
                .update(cx, |input, cx| input.set_value("Review draft", window, cx));
            review.3.update(cx, |input, cx| {
                input.set_value("Unsaved {{pr_url}}", window, cx);
            });
            panel.toggle_script(ScriptEditor::Setup, cx);
            panel.fields[1].update(cx, |input, cx| {
                input.set_value("echo unsaved setup", window, cx);
            });
            panel.toggle_script(ScriptEditor::Action("review".into()), cx);
            assert_eq!(
                panel.expanded_script,
                Some(ScriptEditor::Action("review".into()))
            );
            assert_eq!(panel.script_entries[1].0, "review");
            assert_eq!(
                panel.script_entries[1].2.read(cx).value().as_ref(),
                "Review draft"
            );
            assert_eq!(
                panel.script_entries[1].3.read(cx).value().as_ref(),
                "Unsaved {{pr_url}}"
            );
            assert_eq!(
                panel.fields[1].read(cx).value().as_ref(),
                "echo unsaved setup"
            );
            panel.change_script("review", "shell", window, cx);
            assert_eq!(
                panel.script_entries[1].3.read(cx).value().as_ref(),
                "Unsaved {{pr_url}}"
            );
            panel.change_script("review", "prompt", window, cx);
            panel.change_script("review", "remove", window, cx);
            assert_eq!(panel.expanded_script, None);
            assert!(panel.script_focus["dev"].is_focused(window));
            panel.add_script(window, cx);
            let added = panel.script_entries.last().unwrap();
            assert_eq!(
                panel.expanded_script,
                Some(ScriptEditor::Action(added.0.clone()))
            );
            assert!(added.2.read(cx).focus_handle(cx).is_focused(window));
            let added_id = added.0.clone();
            let added_name = added.2.clone();
            assert!(
                added_name.read(cx).value().is_empty(),
                "New script is a placeholder, never draft content"
            );
            added_name.update(cx, |input, cx| input.set_value("Build", window, cx));
            panel.save(cx);
            assert!(matches!(&panel.script_error, Some((id, false, _)) if id == &added_id));
            added_name.update(cx, |input, cx| input.set_value("", window, cx));
            panel.save(cx);
            assert!(matches!(&panel.script_error, Some((id, true, _)) if id == &added_id));
            assert_eq!(
                panel.fields[1].read(cx).value().as_ref(),
                "echo unsaved setup"
            );
        });
    });
    draw(cx);
    // Keyboard disclosure activates the row once, without treating Space as page scrolling.
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| panel.script_focus["dev"].focus(window, cx));
    });
    cx.simulate_keystrokes("space");
    draw(cx);
    panel.read_with(cx, |panel, _| {
        assert_eq!(
            panel.expanded_script,
            Some(ScriptEditor::Action("dev".into()))
        );
    });
    cx.simulate_keystrokes("enter");
    draw(cx);
    panel.read_with(cx, |panel, _| assert_eq!(panel.expanded_script, None));
    for width in [320., 420., 768.] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(900.)));
        draw(cx);
        let field = cx.debug_bounds("project-name").unwrap();
        assert!(cx.debug_bounds("Project folder").is_none());
        let folder = cx.debug_bounds("project-icon-section").unwrap();
        assert!(
            folder.top() >= field.bottom(),
            "project icons must stay below the full-width name"
        );
        assert_eq!(field.left(), folder.left());
        assert_eq!(field.right(), folder.right());
        let actions = cx.debug_bounds("project-panel-actions").unwrap();
        assert_eq!(field.left(), actions.left());
        let cancel = cx.debug_bounds("project-panel-cancel-action").unwrap();
        assert_eq!(field.left(), cancel.left());
        assert_eq!(field.right(), actions.right());
        assert!(f32::from(folder.right()) <= width);
        let row = cx.debug_bounds("script-row-dev").unwrap();
        assert!(f32::from(row.left()) >= 0.);
        assert!(f32::from(row.right()) <= width);
    }
}

#[gpui::test]
fn panel_script_entry_point_reveals_new_name_after_many_entries(cx: &mut gpui::TestAppContext) {
    let path = "/tmp/grove-many-scripts";
    let entries = (0..15)
        .map(|index| ProjectAction {
            id: format!("saved-{index}"),
            name: format!("Saved {index}"),
            kind: ProjectActionKind::Shell {
                command: "echo saved".into(),
            },
        })
        .collect();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "Many scripts".into(),
                path: path.into(),
                archived: false,
                worktree_dir: None,
                scripts: ProjectScripts {
                    entries,
                    ..Default::default()
                },
            }],
            ..Default::default()
        }));
    });
    let (panel, cx) = cx.add_window_view(|window, cx| {
        let registry = cx.new(|_| SessionRegistry::new());
        let state = cx.new(|cx| {
            crate::entities::workspace_state::WorkspaceState::new(
                &cx.global::<SettingsState>().store,
                MODAL_W_LG,
            )
        });
        let service = cx.new(|_| ProjectService::new(registry.clone(), state));
        let mut panel = ProjectPanel::new(Page::Edit(path.into()), service, registry, window, cx);
        panel.open_scripts(true, window, cx);
        panel
    });
    cx.simulate_resize(gpui::size(gpui::px(768.), gpui::px(720.)));
    draw(cx);
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    draw(cx);
    let name = cx.debug_bounds("script-entry-name").unwrap();
    let scroll = cx.debug_bounds("project-panel-scroll").unwrap();
    assert!(
        name.top() >= scroll.top() && name.bottom() <= scroll.bottom(),
        "new script name must be visible after existing scripts: {name:?} in {scroll:?}"
    );
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            let entry = panel.script_entries.last().unwrap();
            assert!(!entry.1, "Add starts a Shell entry");
            assert!(entry.2.read(cx).focus_handle(cx).is_focused(window));
            assert_eq!(
                cx.global::<SettingsState>().store.projects[0]
                    .scripts
                    .entries
                    .len(),
                15
            );
        });
    });
}

#[gpui::test]
fn all_action_contexts_share_complete_small_recipe(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);
    let (_, cx) = cx.add_window_view(|_, _| ButtonRecipeProbe);
    draw(cx);
    let mini = cx.debug_bounds("recipe-mini").unwrap();
    let dense = cx.debug_bounds("recipe-dense").unwrap();
    let form = cx.debug_bounds("recipe-form").unwrap();
    let browse = cx.debug_bounds("recipe-browse").unwrap();
    assert_eq!(
        browse.size, dense.size,
        "actual Browse builder must share full dense geometry"
    );
    assert_eq!(
        mini.size, dense.size,
        "Mini must share label/icon/gap/padding geometry with dense actions"
    );
    assert_eq!(f32::from(mini.size.height), CONTROL_H);
    assert_eq!(
        form.size, dense.size,
        "form actions must share the smaller label, icon, gap and padding recipe"
    );
}
fn draw(cx: &mut gpui::VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}
#[gpui::test]
fn project_move_picker_cancel_keyboard_and_persistence(cx: &mut gpui::TestAppContext) {
    if std::env::var_os("GROVE_MOVE_PANEL_TEST_CHILD").is_none() {
        let root = std::env::temp_dir().join(format!(
            "grove-move-panel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs_err::create_dir_all(&root).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "views::sidebar::projects::tests::project_move_picker_cancel_keyboard_and_persistence", "--nocapture"])
                .env("GROVE_MOVE_PANEL_TEST_CHILD", "1").env("GROVE_CONFIG_DIR", &root).env("GROVE_TEST_CONFIG_DIR", &root).output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let path = format!("{}/repository", std::env::var("GROVE_CONFIG_DIR").unwrap());
    fs_err::create_dir_all(&path).unwrap();
    cx.update(|cx| {
        gpui_component::init(cx);
        let mut store = grove_core::storage::Store::default();
        store.projects.push(grove_core::storage::Project {
            name: "Move example".into(),
            path: path.clone(),
            scripts: ProjectScripts::default(),
            archived: false,
            worktree_dir: None,
        });
        store.assign_project_to_active_workspace(&path);
        cx.set_global(SettingsState::new(store));
    });
    let (panel, cx) = cx.add_window_view(|window, cx| {
        let registry = cx.new(|_| SessionRegistry::new());
        let state = cx.new(|cx| {
            crate::entities::workspace_state::WorkspaceState::new(
                &cx.global::<SettingsState>().store,
                MODAL_W_LG,
            )
        });
        let service = cx.new(|_| ProjectService::new(registry.clone(), state));
        ProjectPanel::new(Page::Move(path.clone()), service, registry, window, cx)
    });
    draw(cx);
    assert!(cx.debug_bounds("project-move-destination-2").is_none());
    cx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.move_project(cx);
            assert_eq!(
                cx.global::<SettingsState>()
                    .store
                    .project_workspace_id(&path),
                1
            );
            assert!(panel.error.is_none());
            cx.global_mut::<SettingsState>()
                .store
                .workspaces
                .create("A destination workspace with a very long name for geometry checks")
                .unwrap();
            cx.global_mut::<SettingsState>().store.workspaces.select(1);
        });
    });
    draw(cx);
    for (width, height) in [(320., 480.), (768., 560.), (1280., 800.)] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
        draw(cx);
        let bounds = cx.debug_bounds("project-move-destination-2").unwrap();
        assert!(f32::from(bounds.left()) >= 0.);
        assert!(f32::from(bounds.right()) <= width);
        let footer = cx.debug_bounds("project-panel-footer").unwrap();
        assert!(f32::from(footer.bottom()) <= height);
    }
    cx.simulate_resize(gpui::size(gpui::px(768.), gpui::px(560.)));
    draw(cx);
    cx.simulate_keystrokes("tab enter");
    draw(cx);
    assert_eq!(panel.read_with(cx, |panel, _| panel.destination), Some(2));
    assert!(!panel.read_with(cx, |panel, _| panel.open_destination));
    cx.simulate_keystrokes("tab tab space");
    draw(cx);
    assert!(panel.read_with(cx, |panel, _| panel.open_destination));
    cx.simulate_keystrokes("shift-tab space");
    draw(cx);
    assert!(!panel.read_with(cx, |panel, _| panel.open_destination));
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert_eq!(
        cx.read(|cx| cx
            .global::<SettingsState>()
            .store
            .project_workspace_id(&path)),
        1
    );
    // A standalone panel isn't removed by its parent here. Reopen focus, then activate
    // destination and explicit submit via the same keyboard route as the real panel.
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.destination = None;
            panel.focus.focus(window, cx);
            cx.notify();
        });
    });
    draw(cx);
    cx.simulate_keystrokes("tab space");
    draw(cx);
    assert_eq!(panel.read_with(cx, |panel, _| panel.destination), Some(2));
    cx.simulate_keystrokes("tab tab tab tab enter");
    draw(cx);
    assert!(panel.read_with(cx, |panel, _| panel.error.is_none()));
    cx.read(|cx| {
        let store = &cx.global::<SettingsState>().store;
        assert_eq!(store.project_workspace_id(&path), 2);
        assert_eq!(store.workspaces.active, 1);
        assert_eq!(store.projects[0].path, path);
    });
    assert_eq!(
        grove_core::storage::load()
            .unwrap()
            .project_workspace_id(&path),
        2
    );
    cx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.destination = Some(1);
            panel.open_destination = true;
            cx.global_mut::<SettingsState>().store.workspaces.select(2);
            panel.move_project(cx);
            assert!(panel.error.is_none());
            assert_eq!(
                cx.global::<SettingsState>()
                    .store
                    .project_workspace_id(&path),
                1
            );
            assert_eq!(cx.global::<SettingsState>().store.workspaces.active, 1);
        });
    });
    // A vanished choice is revalidated at submission and leaves the panel retryable.
    cx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel.destination = Some(9999);
            panel.move_project(cx);
            assert!(panel.error.is_some());
            assert_eq!(
                cx.global::<SettingsState>()
                    .store
                    .project_workspace_id(&path),
                1
            );
        });
    });
}
#[gpui::test]
fn project_editor_archive_restore_and_removal_keep_path_identity(cx: &mut gpui::TestAppContext) {
    // Run persistence against a child-process-only config: no test changes the parent environment.
    if std::env::var_os("GROVE_PROJECT_PANEL_TEST_CHILD").is_none() {
        let root = std::env::temp_dir().join(format!(
            "grove-project-panel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs_err::create_dir_all(&root).unwrap();
        let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","views::sidebar::projects::tests::project_editor_archive_restore_and_removal_keep_path_identity","--nocapture"]).env("GROVE_PROJECT_PANEL_TEST_CHILD","1").env("GROVE_CONFIG_DIR",&root).env("GROVE_TEST_CONFIG_DIR", &root).output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let path = std::env::var("GROVE_CONFIG_DIR").unwrap() + "/repository";
    fs_err::create_dir_all(&path).unwrap();
    let path = fs_err::canonicalize(&path)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "old".into(),
                path: path.clone(),
                scripts: ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..grove_core::storage::Store::default()
        }));
    });
    let (panel, cx) = cx.add_window_view(|window, cx| {
        let registry = cx.new(|_| SessionRegistry::new());
        let state = cx.new(|cx| {
            crate::entities::workspace_state::WorkspaceState::new(
                &cx.global::<SettingsState>().store,
                MODAL_W_LG,
            )
        });
        let service = cx.new(|_| ProjectService::new(registry.clone(), state));
        ProjectPanel::new(Page::Edit(path.clone()), service, registry, window, cx)
    });
    draw(cx);
    for (width, height) in [(768., 560.), (1280., 800.)] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
        draw(cx);
        let field = cx.debug_bounds("project-name").unwrap();
        assert!(cx.debug_bounds("Project folder").is_none());
        let folder = cx.debug_bounds("project-icon-section").unwrap();
        let scroll = cx.debug_bounds("project-panel-scroll").unwrap();
        let footer = cx.debug_bounds("project-panel-footer").unwrap();
        assert!(f32::from(field.left()) >= 0.);
        assert!(f32::from(field.right()) <= width);
        assert!(folder.top() >= field.bottom());
        assert_eq!(field.left(), folder.left());
        assert_eq!(field.right(), folder.right());
        let actions = cx.debug_bounds("project-panel-actions").unwrap();
        assert_eq!(field.left(), actions.left());
        let cancel = cx.debug_bounds("project-panel-cancel-action").unwrap();
        assert_eq!(field.left(), cancel.left());
        assert_eq!(field.right(), actions.right());
        assert!(f32::from(folder.right()) <= width);
        assert!(f32::from(scroll.top()) < f32::from(footer.top()));
        assert!(f32::from(footer.bottom()) <= height);
    }
    cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                for (field, value) in
                    panel
                        .fields
                        .iter()
                        .zip(["renamed", "echo setup", "echo run", "echo teardown"])
                {
                    field.update(cx, |input, cx| input.set_value(value, window, cx));
                }
                let name = cx.new(|cx| InputState::new(window,cx).default_value("Run"));
                let body = cx.new(|cx| InputState::new(window,cx).multi_line(true).default_value("echo run"));
                panel.script_entries = vec![("test-run".into(),false,name,body)];
                panel.fields[0].update(cx, |input, cx| input.set_value("", window, cx));
                panel.save(cx);
                assert!(panel.error.is_some());
                assert_eq!(panel.fields[1].read(cx).value().as_ref(), "echo setup");
                let mut collision = cx.global::<SettingsState>().store.projects[0].clone();
                collision.name = "taken".into();
                collision.path = format!("{path}/other");
                cx.global_mut::<SettingsState>()
                    .store
                    .projects
                    .push(collision);
                panel.fields[0].update(cx, |input, cx| input.set_value("taken", window, cx));
                panel.save(cx);
                assert!(panel.error.is_some());
                assert_eq!(panel.fields[1].read(cx).value().as_ref(), "echo setup");
                cx.global_mut::<SettingsState>().store.projects.pop();
                panel.fields[0].update(cx, |input, cx| input.set_value("renamed", window, cx));
                panel.save(cx);
                assert!(panel.error.is_none());
                let project = &cx.global::<SettingsState>().store.projects[0];
                assert_eq!(project.name, "renamed");
                assert_eq!(project.scripts.setup.as_deref(), Some("echo setup"));
                assert!(matches!(&project.scripts.entries[0].kind, ProjectActionKind::Shell { command } if command == "echo run"));
                assert_eq!(project.scripts.teardown.as_deref(), Some("echo teardown"));
                panel.registry.update(cx, |registry, _| {
                    registry.insert_meta(
                        "renamed".into(),
                        path.clone(),
                        grove_core::agent::Agent::Codex,
                    );
                });
                panel.decide(Decision::Archive, window, cx);
                panel.archive(cx);
                assert!(panel.error.is_some());
                assert!(!cx.global::<SettingsState>().store.projects[0].archived);
                panel
                    .service
                    .update(cx, |service, cx| {
                        service.kill_sessions_for_project_path(&path, cx)
                    })
                    .unwrap();
                panel.error = None;
                panel.archive(cx);
                assert!(panel.error.is_none());
                assert!(cx.global::<SettingsState>().store.projects[0].archived);
                panel
                    .service
                    .update(cx, |service, cx| {
                        service.restore_archived_by_path(&path, cx)
                    })
                    .unwrap();
                assert!(!cx.global::<SettingsState>().store.projects[0].archived);
                assert!(matches!(&cx.global::<SettingsState>().store.projects[0].scripts.entries[0].kind, ProjectActionKind::Shell { command } if command == "echo run"));
                assert_eq!(cx.global::<SettingsState>().store.projects[0].scripts.entries[0].id,"test-run");
                panel
                    .service
                    .update(cx, |service, cx| service.archive_project_by_path(&path, cx))
                    .unwrap();
                panel.decide(Decision::Delete(path.clone()), window, cx);
                panel.cancel_decision(window, cx);
                assert_eq!(cx.global::<SettingsState>().store.projects.len(), 1);
                panel
                    .service
                    .update(cx, |service, cx| service.delete_archived_by_path(&path, cx))
                    .unwrap();
                assert!(cx.global::<SettingsState>().store.projects.is_empty());
                assert!(std::path::Path::new(&path).exists());
                panel
                    .service
                    .update(cx, |service, cx| {
                        service.register_project_in_workspace(
                            "remove-me".into(),
                            path.clone(),
                            1,
                            cx,
                        )
                    })
                    .unwrap();
                panel.page = Page::Remove(path.clone());
                panel.reviewed_worktrees = Some(ProjectWorktreeReview::default());
                panel.discovery_done = true;
                panel.delete_worktrees = false;
                panel.remove(cx);
                assert!(panel.started);
            });
        });
    let wait_for_removal = |cx: &mut gpui::VisualTestContext| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            if panel.read_with(cx, |panel, cx| {
                panel
                    .service
                    .read(cx)
                    .removal_status(&path)
                    .unwrap()
                    .finished
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "project removal did not finish"
            );
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(20));
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    };
    wait_for_removal(cx);
    cx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            let status = panel.service.read(cx).removal_status(&path).unwrap();
            assert!(status.finished && status.unregistered);
            assert!(status.errors.is_empty());
            assert!(std::path::Path::new(&path).exists());
            panel.remove(cx);
            assert!(
                panel
                    .service
                    .read(cx)
                    .removal_status(&path)
                    .unwrap()
                    .finished
            );
        });
    });
    cx.update(|_, cx| {
        panel.update(cx, |panel, cx| {
            panel
                .service
                .update(cx, |service, cx| {
                    service.register_project_in_workspace("error-test".into(), path.clone(), 1, cx)
                })
                .unwrap();
            panel.started = false;
            panel.reviewed_worktrees = None;
            panel.error = Some("Could not discover worktrees.".into());
            panel.remove(cx);
            assert!(panel.started);
        });
    });
    wait_for_removal(cx);
    draw(cx);
    panel.read_with(cx, |panel, cx| {
        let status = panel.service.read(cx).removal_status(&path).unwrap();
        assert!(status.finished);
        assert!(status.errors.is_empty());
        assert!(status.unregistered);
    });
    assert!(cx.debug_bounds("project-panel").is_some());
}
