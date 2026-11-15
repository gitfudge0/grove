//! Optional project appearance editing. Choices remain in the outer form draft.
use super::*;
use crate::views::components::{dense_action_with_colors, project_symbol};
use gpui_component::Selectable;
use grove_core::storage::identity::{ProjectIdentity, COLOR_COUNT, SYMBOLS};

const SYMBOL_CONTROL_SIZE: f32 = 28.;
const SYMBOL_PICKER_MAX_H: f32 = 224.;
const COLOR_NAMES: [&str; 6] = ["Teal", "Copper", "Slate", "Rose", "Olive", "Neutral"];

impl ProjectPanel {
    fn randomize_appearance(&mut self, cx: &mut Context<Self>) {
        if let Some(appearance) = &mut self.draft_appearance {
            match appearance.current().randomized() {
                Ok(identity) => {
                    appearance.custom = Some(identity);
                    self.error = None;
                }
                Err(error) => self.error = Some(error.into()),
            }
        }
        cx.notify();
    }

    pub(super) fn render_appearance(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let Some(appearance) = self.draft_appearance else {
            return div();
        };
        let current = appearance.current();
        let mut section = div()
            .debug_selector(|| "project-icon-section".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child("Project icon"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(rpx(SPACE_2XL))
                    .child(project_symbol(current, ICON_20))
                    .child(div().flex_1())
                    .child(
                        project_action(
                            "project-appearance-randomize".into(),
                            "Shuffle",
                            Some("icons/shuffle.svg"),
                            ProjectActionStyle::Mini,
                            window,
                            cx,
                        )
                        .debug_selector(|| "project-appearance-randomize".into())
                        .on_click(cx.listener(|this, _, _, cx| this.randomize_appearance(cx)))
                        .on_key_down(cx.listener(
                            |this, event: &gpui::KeyDownEvent, window, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                    && !event.is_held
                                {
                                    this.randomize_appearance(cx);
                                    window.prevent_default();
                                    cx.stop_propagation();
                                }
                            },
                        )),
                    ),
            );
        // Direct input feedback; no animation or asynchronous state to imply persistence.
        {
            let mut symbols = div()
                .id("project-symbol-choices")
                .debug_selector(|| "project-symbol-choices".into())
                .w_full()
                .max_h(rpx(SYMBOL_PICKER_MAX_H))
                .overflow_y_scrollbar()
                .flex()
                .flex_wrap()
                .gap(rpx(SPACE_SM));
            for (index, &(glyph, name)) in SYMBOLS.iter().enumerate() {
                let symbol = index as u8;
                let selected = current.symbol == symbol;
                symbols = symbols.child(
                    dense_action_with_colors(
                        ("project-symbol-choice", index),
                        name.to_string(),
                        if selected {
                            c::BG_HOVER()
                        } else {
                            gpui::transparent_black()
                        },
                        c::FG(),
                        c::BG_HOVER(),
                        window,
                        cx,
                    )
                    .icon_only()
                    .selected(selected)
                    .with_focus_handle(self.appearance_symbol_focus[index].clone())
                    .tooltip(name)
                    .p_0()
                    .border_1()
                    .border_color(if selected { c::FG() } else { c::BORDER_SOFT() })
                    .w(rpx(SYMBOL_CONTROL_SIZE))
                    .h(rpx(SYMBOL_CONTROL_SIZE))
                    .debug_selector(move || format!("project-symbol-choice-{index}"))
                    .icon(
                        gpui_component::Icon::default()
                            .path(format!("icons/{glyph}.svg"))
                            .text_color(c::PROJECT_IDENTITY(current.color)),
                    )
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                && !event.is_held
                            {
                                if let Some(a) = &mut this.draft_appearance {
                                    a.custom = Some(ProjectIdentity {
                                        symbol,
                                        ..a.current()
                                    });
                                }
                                window.prevent_default();
                                cx.stop_propagation();
                                cx.notify();
                            }
                        },
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(a) = &mut this.draft_appearance {
                            a.custom = Some(ProjectIdentity {
                                symbol,
                                ..a.current()
                            });
                        }
                        cx.notify();
                    })),
                );
            }
            let mut colors = div().flex().flex_wrap().gap(rpx(SPACE_SM));
            for color in 0..COLOR_COUNT {
                colors = colors.child(
                    crate::views::components::settings_control(
                        ("project-symbol-color", usize::from(color)),
                        COLOR_NAMES[usize::from(color)],
                        color == current.color,
                        true,
                    )
                    .child(COLOR_NAMES[usize::from(color)])
                    .track_focus(&self.appearance_color_focus[usize::from(color)])
                    .debug_selector(move || format!("project-symbol-color-{color}"))
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                && !event.is_held
                            {
                                if let Some(a) = &mut this.draft_appearance {
                                    a.custom = Some(ProjectIdentity {
                                        color,
                                        ..a.current()
                                    });
                                }
                                window.prevent_default();
                                cx.stop_propagation();
                                cx.notify();
                            }
                        },
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(a) = &mut this.draft_appearance {
                            a.custom = Some(ProjectIdentity {
                                color,
                                ..a.current()
                            });
                        }
                        cx.notify();
                    })),
                );
            }
            section = section.child(symbols).child(colors).child(
                project_action(
                    "project-appearance-automatic".into(),
                    "Reset icon",
                    Some("icons/reset.svg"),
                    ProjectActionStyle::Quiet,
                    window,
                    cx,
                )
                .debug_selector(|| "project-appearance-automatic".into())
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                        if let Some(a) = &mut this.draft_appearance {
                            a.custom = None;
                        }
                        window.prevent_default();
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(a) = &mut this.draft_appearance {
                        a.custom = None;
                    }
                    cx.notify();
                })),
            );
        }
        section
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn symbol_editor_keeps_choices_draft_and_preserves_name_at_narrow_zoom(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "Project with a long descriptive name".into(),
                    path: "/symbol-fixture".into(),
                    scripts: ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
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
                Page::Edit("/symbol-fixture".into()),
                service,
                registry,
                window,
                cx,
            )
        });
        cx.run_until_parked();
        let original = panel.read_with(cx, |p, _| p.draft_appearance.unwrap());
        for (width, rem) in [(768., 16.), (360., 16.), (360., 24.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(900.)));
            cx.update(|window, _| {
                window.set_rem_size(gpui::px(rem));
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let symbols = cx.debug_bounds("project-symbol-choices").unwrap();
            let choices = [
                "project-symbol-choice-0",
                "project-symbol-choice-1",
                "project-symbol-choice-2",
                "project-symbol-choice-3",
                "project-symbol-choice-4",
                "project-symbol-choice-5",
                "project-symbol-choice-6",
                "project-symbol-choice-7",
                "project-symbol-choice-8",
                "project-symbol-choice-9",
                "project-symbol-choice-10",
                "project-symbol-choice-11",
                "project-symbol-choice-12",
                "project-symbol-choice-13",
                "project-symbol-choice-14",
                "project-symbol-choice-15",
            ];
            assert_eq!(choices.len(), SYMBOLS.len());
            for selector in choices {
                assert!(cx.debug_bounds(selector).is_some());
            }
            assert!(symbols.left() >= gpui::px(0.) && symbols.right() <= gpui::px(width));
            let name = cx.debug_bounds("project-name").unwrap();
            assert!(name.bottom() <= symbols.top());
            let color = cx.debug_bounds("project-symbol-color-3").unwrap().center();
            cx.simulate_click(color, gpui::Modifiers::default());
            cx.run_until_parked();
            panel.read_with(cx, |p, cx| {
                assert_eq!(p.draft_appearance.unwrap().current().color, 3);
                assert_eq!(
                    cx.global::<SettingsState>()
                        .store
                        .project_appearance("/symbol-fixture"),
                    original
                );
                assert_eq!(
                    p.fields[0].read(cx).value().as_ref(),
                    "Project with a long descriptive name"
                );
            });
        }
        cx.simulate_resize(gpui::size(gpui::px(768.), gpui::px(1200.)));
        cx.update(|window, _| window.set_rem_size(gpui::px(16.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let before_randomize = panel.read_with(cx, |p, _| p.draft_appearance.unwrap().current());
        let randomize = cx.debug_bounds("project-appearance-randomize").unwrap();
        cx.simulate_click(randomize.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        panel.read_with(cx, |p, cx| {
            let next = p.draft_appearance.unwrap();
            assert_ne!(next.current().symbol, before_randomize.symbol);
            assert_ne!(next.current().color, before_randomize.color);
            assert_eq!(next.automatic, original.automatic);
            assert_eq!(
                cx.global::<SettingsState>()
                    .store
                    .project_appearance("/symbol-fixture"),
                original
            );
        });
        // A chooser value is keyboard activatable, and reset remains local until outer Save.
        let symbol = cx.debug_bounds("project-symbol-choice-0").unwrap().center();
        cx.simulate_click(symbol, gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            panel.update(cx, |p, cx| {
                p.appearance_symbol_focus[0].focus(window, cx);
                p.draft_appearance.as_mut().unwrap().custom = Some(ProjectIdentity {
                    symbol: 1,
                    ..original.current()
                });
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        panel.read_with(cx, |p, _| {
            assert_eq!(p.draft_appearance.unwrap().current().symbol, 0);
        });
        cx.simulate_resize(gpui::size(gpui::px(768.), gpui::px(1600.)));
        cx.update(|window, _| window.set_rem_size(gpui::px(16.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let reset = cx.debug_bounds("project-appearance-automatic").unwrap();
        cx.simulate_click(reset.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        panel.read_with(cx, |p, cx| {
            assert_eq!(p.draft_appearance.unwrap(), original);
            assert_eq!(
                cx.global::<SettingsState>()
                    .store
                    .project_appearance("/symbol-fixture"),
                original
            );
        });
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    #[gpui::test]
    fn symbol_save_is_atomic_on_disk_failure_and_reset(cx: &mut gpui::TestAppContext) {
        if std::env::var_os("GROVE_SYMBOL_SAVE_CHILD").is_none() {
            let root = tempfile::tempdir().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "views::sidebar::projects::appearance::persistence_tests::symbol_save_is_atomic_on_disk_failure_and_reset", "--nocapture"])
                .env("GROVE_SYMBOL_SAVE_CHILD", "1").env("GROVE_CONFIG_DIR", root.path()).env("GROVE_TEST_CONFIG_DIR", root.path()).output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let root = std::path::PathBuf::from(std::env::var("GROVE_CONFIG_DIR").unwrap());
        let path = root.join("repository");
        fs_err::create_dir_all(&path).unwrap();
        let path = path.to_string_lossy().into_owned();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "grove".into(),
                    path: path.clone(),
                    scripts: ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
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
            ProjectPanel::new(Page::Edit(path.clone()), service, registry, window, cx)
        });
        let original = panel.read_with(cx, |p, _| p.draft_appearance.unwrap());
        cx.update(|_, cx| {
            panel.update(cx, |p, cx| {
                p.draft_appearance.as_mut().unwrap().custom = Some(ProjectIdentity {
                    symbol: 4,
                    color: 3,
                });
                p.save(cx);
                assert!(p.error.is_none());
            });
        });
        let saved = grove_core::storage::load().unwrap();
        assert_eq!(
            saved.project_appearance(&path).current(),
            ProjectIdentity {
                symbol: 4,
                color: 3
            }
        );
        let config = grove_core::storage::config_path().unwrap();
        let saved_bytes = fs_err::read(&config).unwrap();
        fs_err::remove_file(&config).unwrap();
        fs_err::create_dir(&config).unwrap();
        cx.update(|_, cx| {
            panel.update(cx, |p, cx| {
                p.draft_appearance.as_mut().unwrap().custom = Some(ProjectIdentity {
                    symbol: 6,
                    color: 4,
                });
                p.save(cx);
                assert!(p.error.is_some());
                assert_eq!(
                    p.draft_appearance.unwrap().current(),
                    ProjectIdentity {
                        symbol: 6,
                        color: 4
                    }
                );
                assert_eq!(
                    cx.global::<SettingsState>()
                        .store
                        .project_appearance(&path)
                        .current(),
                    ProjectIdentity {
                        symbol: 4,
                        color: 3
                    }
                );
            });
        });
        fs_err::remove_dir(&config).unwrap();
        fs_err::write(&config, saved_bytes).unwrap();
        cx.update(|_, cx| {
            panel.update(cx, |p, cx| {
                p.draft_appearance.as_mut().unwrap().custom = None;
                p.save(cx);
                assert!(p.error.is_none());
            });
        });
        assert_eq!(
            grove_core::storage::load()
                .unwrap()
                .project_appearance(&path),
            original
        );
    }
}

#[cfg(test)]
mod native_qa {
    use super::*;
    const GALLERY_WIDTH: f32 = 300.;

    struct Gallery {
        panel: Entity<ProjectPanel>,
    }
    impl Render for Gallery {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let mut samples = div().flex().flex_wrap().gap(rpx(SPACE_LG));
            for (index, _) in SYMBOLS.iter().enumerate() {
                samples = samples.child(project_symbol(
                    ProjectIdentity {
                        symbol: index as u8,
                        color: (index % usize::from(COLOR_COUNT)) as u8,
                    },
                    ICON_20,
                ));
            }
            let mut projects = div().flex().flex_col().gap(rpx(SPACE_3XL));
            for (index, p) in cx
                .global::<SettingsState>()
                .store
                .projects
                .iter()
                .enumerate()
            {
                projects = projects.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .child(project_symbol(
                            cx.global::<SettingsState>()
                                .store
                                .project_appearance(&p.path)
                                .current(),
                            ICON_20,
                        ))
                        .child(p.name.clone())
                        .child(crate::icons::icon(
                            if index == 1 { "claude" } else { "codex" },
                            ICON_MD,
                            c::FG(),
                        )),
                );
            }
            div()
                .size_full()
                .bg(c::BG())
                .text_color(c::FG())
                .flex()
                .child(div().flex_1().min_w_0().h_full().child(self.panel.clone()))
                .child(
                    div()
                        .w(rpx(GALLERY_WIDTH))
                        .flex_shrink_0()
                        .p(rpx(SPACE_3XL))
                        .flex()
                        .flex_col()
                        .gap(rpx(SPACE_3XL))
                        .child("Automatic project identities")
                        .child(projects)
                        .child("16 native project symbols")
                        .child(samples)
                        .child(
                            div()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child("Static fixture · no provider connection"),
                        ),
                )
        }
    }

    #[test]
    #[ignore = "isolated native symbol QA; requires a display and GROVE_SYMBOL_QA_DIR"]
    fn symbol_native_gallery() {
        let output = std::env::var("GROVE_SYMBOL_QA_DIR").expect("task-owned QA directory");
        assert!(std::env::var_os("GROVE_CONFIG_DIR").is_some());
        let light = std::env::var_os("GROVE_SYMBOL_QA_LIGHT").is_some();
        let rem = if light { 20. } else { 16. };
        gpui_platform::application()
            .with_assets(crate::assets::Assets)
            .run(move |cx| {
                gpui_component::init(cx);
                crate::fonts::register(cx).unwrap();
                assert!(grove_core::theme::set_by_name(if light {
                    "grove-light"
                } else {
                    "grove-dark"
                }));
                let mut store = grove_core::storage::Store::default();
                for name in ["grove", "gateway", "grove-api"] {
                    store.projects.push(grove_core::storage::Project {
                        name: name.into(),
                        path: format!("/qa/{name}"),
                        scripts: ProjectScripts::default(),
                        archived: false,
                        worktree_dir: None,
                    });
                }
                cx.set_global(SettingsState::new(store));
                cx.set_reduce_motion(true);
                c::sync_component_theme(cx);
                cx.open_window(
                    gpui::WindowOptions {
                        app_id: Some("dev.grove.symbol-qa".into()),
                        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                            gpui::point(gpui::px(40.), gpui::px(40.)),
                            gpui::size(gpui::px(1100.), gpui::px(1000.)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        window.set_window_title("Grove Symbol QA");
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
                                Page::Edit("/qa/grove".into()),
                                service,
                                registry,
                                window,
                                cx,
                            )
                        });
                        let gallery = cx.new(|_| Gallery { panel });
                        cx.new(|cx| gpui_component::Root::new(gallery, window, cx))
                    },
                )
                .unwrap();
                cx.activate(true);
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(2))
                        .await;
                    fs_err::write(
                        std::path::Path::new(&output).join("ready"),
                        std::process::id().to_string(),
                    )
                    .unwrap();
                    cx.background_executor()
                        .timer(std::time::Duration::from_mins(10))
                        .await;
                    cx.update(|cx| cx.quit());
                })
                .detach();
            });
    }
}
