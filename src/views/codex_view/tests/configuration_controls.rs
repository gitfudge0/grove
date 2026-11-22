use super::*;

#[test]
fn claude_effort_uses_exact_resolved_catalog_model() {
    let mut state = snapshot();
    state.saved.provider = codex::Provider::Claude;
    state.model = None;
    state.resolved_model = Some("claude-opus-5-5".into());
    state.models = vec![
            json!({"model":"default", "resolvedModel":"claude-opus-5-5", "supportedReasoningEfforts":[{"reasoningEffort":"high"},{"reasoningEffort":"max"}]}),
            json!({"model":"haiku-legacy", "resolvedModel":"claude-haiku-legacy"}),
        ].into();
    let (choices, _) = CodexView::menu_choices(&state, ComposerMenu::Effort);
    assert!(choices.iter().any(|(value, _)| value == "high"));
    assert!(choices.iter().any(|(value, _)| value == "max"));
    state.model = Some("haiku-legacy".into());
    let (choices, _) = CodexView::menu_choices(&state, ComposerMenu::Effort);
    assert_eq!(
        choices,
        vec![("__configured".into(), "Restore configured defaults".into())]
    );
    state.model = Some("unknown-custom-model".into());
    assert!(effective_model(&state).is_none());
}

#[test]
fn yolo_identity_comes_from_saved_native_launch_options() {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let mut state = snapshot();
        state.saved.provider = provider;
        state.access = Some(
            if provider == codex::Provider::Codex {
                "full-access"
            } else {
                "bypassPermissions"
            }
            .into(),
        );
        state.resolved_access = state.access.clone();
        state.sandbox = Some(json!({"type":"dangerFullAccess"}));
        state.approval_policy = Some(json!("never"));
        assert!(!session_launched_with_yolo(&state));
        state.saved.config_args = provider
            .launch_options(&provider.agent().launch_args_for_backend(true, true, false))
            .unwrap()
            .0;
        let saved = serde_json::from_value(serde_json::to_value(&state.saved).unwrap()).unwrap();
        state = Snapshot::new(saved);
        assert!(session_launched_with_yolo(&state));
        state.access = Some("plan".into());
        state.resolved_access = Some("plan".into());
        assert!(
            session_launched_with_yolo(&state),
            "effective provider fallback does not rewrite launch identity"
        );
        assert!(!session_has_yolo_access(&state));
        state.saved.native_config_version = 0;
        assert!(
            !session_launched_with_yolo(&state),
            "legacy CLI bypass is not a native Yolo launch"
        );
    }
}

#[test]
fn yolo_readonly_requires_launch_provenance_and_effective_bypass() {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let mut state = snapshot();
        state.saved.provider = provider;
        state.saved.config_args = provider
            .launch_options(&provider.agent().launch_args_for_backend(true, true, false))
            .unwrap()
            .0;
        if provider == codex::Provider::Codex {
            state.resolved_access = Some("full-access".into());
            state.sandbox = Some(json!({"type":"dangerFullAccess"}));
            state.approval_policy = Some(json!("on-request"));
            assert!(!session_has_yolo_access(&state));
            state.approval_policy = Some(json!("never"));
            assert!(session_has_yolo_access(&state));
            state.sandbox = Some(json!({"type":"readOnly"}));
            assert!(!session_has_yolo_access(&state));
        } else {
            state.resolved_access = Some("default".into());
            assert!(!session_has_yolo_access(&state));
            state.resolved_access = Some("bypassPermissions".into());
            assert!(session_has_yolo_access(&state));
            state.access = Some("default".into());
            assert!(!session_has_yolo_access(&state));
        }
    }
}

#[gpui::test]
fn access_control_wraps_with_its_chevron_inside_composer(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    for (width, rem) in [(600.0, 16.0), (400.0, 16.0), (400.0, 20.0)] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0)));
        cx.update(|_, cx| {
            gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.sandbox = Some(json!({"type":"dangerFullAccess"}));
                    session.state.approval_policy = Some(json!("untrusted"));
                    cx.notify();
                });
            });
        });
        draw(cx);
        let composer = cx.debug_bounds("codex-composer").unwrap();
        let label = cx.debug_bounds("codex-access-summary").unwrap();
        let indicator = cx.debug_bounds("codex-access-indicator").unwrap();
        let control = view.read_with(cx, |view, _| view.option_bounds[2].get());
        let inset =
            cx.update(|window, _| rpx(SPACE_2XL).to_pixels(window.rem_size()) + gpui::px(1.));
        assert!(
            control.left() >= composer.left() + inset
                && control.right() <= composer.right() - inset
        );
        assert!(label.right() <= indicator.left());
        assert!(indicator.right() <= control.right());
        assert!(indicator.top() >= control.top() && indicator.bottom() <= control.bottom());
    }
}

#[gpui::test]
fn speed_keyboard_selection_preserves_composer_and_rejects_unavailable(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.models = vec![json!({"model":"speed-model", "serviceTiers":[{"id":"fast"}], "supportedReasoningEfforts":[]})].into();
                session.state.model = Some("speed-model".into());
                session.state.saved.draft_images = vec!["untouched attachment".into()];
            });
            view.input.update(cx, |input, cx| input.set_value("Keep my speed draft", window, cx));
            view.open_menu(ComposerMenu::Settings, window, cx);
            view.speed_focus[1].focus(window, cx);
        }));
    draw(cx);
    let _ = actions.try_iter().collect::<Vec<_>>();
    cx.simulate_keystrokes("space");
    draw(cx);
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::SetSpeed(codex::Speed::Fast))));
    cx.update(|_, cx| view.update(cx, |view, cx| view.send_speed(codex::Speed::Ultrafast, cx)));
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::SetSpeed(_))));
    cx.simulate_keystrokes("escape");
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert!(view.menu.is_none());
        assert_eq!(view.input.read(cx).value().as_ref(), "Keep my speed draft");
        assert_eq!(
            view.session.read(cx).state.saved.draft_images,
            ["untouched attachment"]
        );
    });
}

#[gpui::test]
fn speed_card_stays_above_composer_at_narrow_width_and_zoom(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    for (width, rem) in [(1024.0, 16.0), (400.0, 16.0), (400.0, 24.0)] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(720.0)));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                window.set_rem_size(gpui::px(rem));
                view.close_menu(cx);
                view.session.update(cx, |session, _| {
                    session.state.saved.composer.speed = Some(codex::Speed::Fast);
                    session.state.models.clear();
                });
            });
        });
        draw(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.open_menu(ComposerMenu::Settings, window, cx);
            });
        });
        draw(cx);
        let card = cx.debug_bounds("codex-options-popup").unwrap();
        let trigger = view.read_with(cx, |view, _| view.option_bounds[0].get());
        assert!(card.top() >= gpui::px(0.0), "card top {card:?}, rem={rem}");
        let gap = cx.update(|window, _| rpx(SPACE_LG).to_pixels(window.rem_size()));
        if card.size.height + gap <= trigger.top() {
            assert!(
                card.bottom() <= trigger.top(),
                "card stays above its source when it fits"
            );
        } else {
            assert!(
                card.bottom() <= gpui::px(720.0),
                "full-height card stays visible when it must overlap its source"
            );
        }
        assert!(
            card.left() >= gpui::px(0.0) && card.right() <= gpui::px(width),
            "card exceeds width {card:?}"
        );
        assert!(cx.debug_bounds("codex-speed-0").is_some());
    }
}

#[gpui::test]
fn tab_accepts_skill_and_marker_follows_edits_and_clears_on_change(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.skills = vec![json!({"name":"review","enabled":true})].into();
            });
            view.input.update(cx, |input, cx| {
                input.set_value("Use $rev", window, cx);
                input.set_selected_range(8..8, cx);
            });
            view.input.focus_handle(cx).focus(window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes("tab");
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.input.read(cx).value().as_ref(), "Use $review ");
        assert_eq!(view.picked_tokens.len(), 1);
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_selected_range(0..0, cx);
                input.insert("Please ", window, cx);
            });
        });
    });
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert_eq!(
            view.picked_tokens[0].decoration.get_ranges(cx),
            vec![11..18]
        );
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_selected_range(15..15, cx);
                input.insert("x", window, cx);
            });
        });
    });
    draw(cx);
    view.read_with(cx, |view, _| assert!(view.picked_tokens.is_empty()));
}

#[gpui::test]
fn provider_picker_dashboard_entry_commit_and_escape_preserve_draft(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(crate::settings::SettingsState::new(
            grove_core::storage::Store::default(),
        ));
    });
    let reference = codex::ReferenceDraft {
        id: "picker-reference".into(),
        thread_id: None,
        sources: Vec::new(),
        text: "Keep this quote".into(),
    };
    let mut state = snapshot();
    state.saved.thread_id = None;
    state.saved.cwd = "/missing-grove-picker-fixture".into();
    state.saved.draft = "Keep the prompt".into();
    state.saved.draft_images = vec!["attachment-fixture".into()];
    state.saved.draft_reference = Some(reference.clone());
    let mut retained = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let session = cx.new(|cx| CodexSession::draft(state.saved, Vec::new(), cx));
        session.update(cx, |session, _| {
            session.state.models =
                vec![json!({"model":"code-1","displayName":"Code model"})].into();
            session.state.model = Some("code-1".into());
        });
        let tray = cx.new(|_| {
            crate::views::sidebar::dashboard::DestinationTray::picker_test_tray(
                "/missing-grove-picker-fixture".into(),
            )
        });
        let chat = cx.new(|cx| {
            let mut view = CodexView::new(session, window, cx);
            view.attach_dashboard_tray(tray, cx);
            view
        });
        retained = Some(chat.clone());
        gpui_component::Root::new(chat, window, cx).bordered(false)
    });
    let view = retained.unwrap();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.open_menu(ComposerMenu::Settings, window, cx);
        });
    });
    draw(cx);
    let model = cx.debug_bounds("codex-card-effort-label").unwrap().center();
    cx.simulate_click(model, gpui::Modifiers::default());
    draw(cx);
    cx.update(|_, cx| {
        view.read(cx)
            .model_picker
            .clone()
            .unwrap()
            .update(cx, |picker, cx| {
                picker.seed_test_catalogs();
                cx.notify();
            });
    });
    draw(cx);
    let rail = cx.debug_bounds("model-provider-2").unwrap().center();
    cx.simulate_click(rail, gpui::Modifiers::default());
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert_eq!(
            view.session.read(cx).state.saved.provider,
            codex::Provider::Codex
        );
    });
    let row = cx.debug_bounds("model-picker-row-0").unwrap();
    cx.simulate_click(
        gpui::point(row.left() + gpui::px(35.), row.center().y),
        gpui::Modifiers::default(),
    );
    draw(cx);
    view.read_with(cx, |view, cx| {
        let state = &view.session.read(cx).state;
        assert_eq!(state.saved.provider, codex::Provider::Claude);
        assert_eq!(state.saved.composer.model.as_deref(), Some("claude-1"));
        assert!(view.menu == Some(ComposerMenu::Settings));
        assert_eq!(view.input.read(cx).value().as_ref(), "Keep the prompt");
        assert_eq!(state.saved.draft_images, ["attachment-fixture"]);
        assert_eq!(state.saved.draft_reference.as_ref(), Some(&reference));
        assert_eq!(state.saved.cwd, "/missing-grove-picker-fixture");
        assert!(view.session.read(cx).is_deferred());
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.model_from_card = true;
            view.open_menu(ComposerMenu::Model, window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes("escape");
    draw(cx);
    view.read_with(cx, |view, _| {
        assert!(view.menu == Some(ComposerMenu::Settings));
    });
    assert!(cx.update(|window, cx| view.read(cx).card_model_focus.is_focused(window)));
}

#[gpui::test]
fn composer_choices_dispatch_configuration_and_commands_without_prompts(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.open_menu(ComposerMenu::Model, window, cx);
            view.choose_option("supported-model", window, cx);
            view.input
                .update(cx, |input, cx| input.set_value("/access", window, cx));
            view.submit(window, cx);
            assert!(view.menu == Some(ComposerMenu::Access));
            assert!(view.input.read(cx).value().is_empty());
            view.choose_option("read-only", window, cx);
            view.session.update(cx, |session, _| {
                session.state.status = Status::Working;
                session.state.turn_id = Some("command-turn".into());
            });
            view.input
                .update(cx, |input, cx| input.set_value("/stop", window, cx));
            view.submit(window, cx);
        });
    });
    let actions = actions.try_iter().collect::<Vec<_>>();
    assert!(actions.iter().any(|action| matches!(action, Action::Configure { model: Some(model), .. } if model == "supported-model")));
    assert!(actions.iter().any(|action| matches!(action, Action::Configure { access: Some(access), .. } if access == "read-only")));
    assert!(actions
        .iter()
        .any(|action| matches!(action, Action::Interrupt)));
    assert!(!actions.iter().any(|action| matches!(
        action,
        Action::Submit { .. } | Action::SubmitWithMode { .. }
    )));
}

#[test]
fn composer_effort_stops_use_only_the_effective_catalog() {
    let mut state = snapshot();
    assert!(CodexView::effort_choices(&state).is_empty());
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        state.saved.provider = provider;
        state.model = Some("alias".into());
        state.resolved_model = Some("resolved".into());
        state.models = vec![
                json!({"model":"alias", "resolvedModel":"resolved", "supportedReasoningEfforts":[{"reasoningEffort":"high"}]}),
            ].into();
        assert_eq!(CodexView::effort_choices(&state), vec!["high"]);
        state.models[0]["supportedReasoningEfforts"] = json!([
            {"reasoningEffort":"low"}, {"reasoningEffort":"medium"}, {"reasoningEffort":"high"}
        ]);
        assert_eq!(
            CodexView::effort_choices(&state),
            vec!["low", "medium", "high"]
        );
        assert!(!CodexView::effort_choices(&state).contains(&"__configured".into()));
        state.models[0]["supportedReasoningEfforts"] = json!([]);
        assert!(CodexView::effort_choices(&state).is_empty());
    }
    assert_eq!(effort_stop(-1.0, 3), 0);
    assert_eq!(effort_stop(0.5, 3), 1);
    assert_eq!(effort_stop(2.0, 3), 2);
    assert_eq!(effort_key_index("right", 0, 1), None);
    assert_eq!(effort_key_index("end", 0, 3), Some(2));
    assert_eq!(effort_key_index("left", 0, 3), Some(0));
}

#[test]
fn model_label_native_layout_preserves_full_text_at_every_zoom() {
    use gpui::AppContext as _;
    struct Probe {
        label: &'static str,
        zoom: f32,
        layout: Option<gpui::TextLayout>,
    }
    impl Render for Probe {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            window.set_rem_size(gpui::px(crate::zoom::REM_BASE * self.zoom));
            let text = gpui::StyledText::new(self.label);
            self.layout = Some(text.layout().clone());
            div()
                .font_family(crate::fonts::UI_FAMILY)
                .w(gpui::px(250.))
                .flex()
                .child(full_model_label(text))
        }
    }
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
    });
    let mut probe = None;
    let window = cx
        .open_window(gpui::size(gpui::px(900.), gpui::px(600.)), |window, cx| {
            let child = cx.new(|_| Probe {
                label: "",
                zoom: 1.,
                layout: None,
            });
            probe = Some(child.clone());
            cx.new(|cx| gpui_component::Root::new(child, window, cx).bordered(false))
        })
        .unwrap();
    let probe = probe.unwrap();
    for zoom in [0.6, 0.7, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 2.0] {
        for label in [
            "GPT-6.1 Terra",
            "GPT-6.1-Sol · Medium · Fast",
            "Claude Opus · High · Standard",
        ] {
            probe.update(&mut cx, |probe, cx| {
                probe.label = label;
                probe.zoom = zoom;
                cx.notify();
            });
            cx.update_window(window.into(), |_, window, cx| {
                window.simulate_next_frame(cx);
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.update(|cx| {
                assert_eq!(
                    probe.read(cx).layout.as_ref().unwrap().text(),
                    label,
                    "full shaped text at {zoom}x"
                );
            });
        }
    }
    cx.update_window(window.into(), |_, window, _| window.remove_window())
        .unwrap();
}

#[gpui::test]
fn composer_popup_rise_releases_ownership_and_cancels_stale_cleanup(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            cx.set_reduce_motion(false);
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.saved.provider = provider;
                    session.state.permission_modes = vec!["default".into(), "plan".into()];
                });
                view.open_menu(ComposerMenu::Access, window, cx);
            });
        });
        draw(cx);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.close_menu(cx);
                assert!(view.menu.is_none());
                assert!(view.popup.is_none());
                assert!(view.closing_popup.is_some());
                view.open_menu(ComposerMenu::Add, window, cx);
                assert!(view.closing_popup.is_none());
            });
        });
        cx.executor()
            .advance_clock(crate::views::motion::PopupMotion::exit_duration() * 2);
        draw(cx);
        view.read_with(cx, |view, _| {
            assert!(
                view.menu == Some(ComposerMenu::Add),
                "old cleanup cannot close a reopened popup"
            );
        });
        cx.update(|_, cx| view.update(cx, super::super::CodexView::close_menu));
        draw(cx);
        cx.executor()
            .advance_clock(crate::views::motion::PopupMotion::exit_duration() * 2);
        draw(cx);
        view.read_with(cx, |view, _| assert!(view.closing_popup.is_none()));
        cx.update(|window, cx| {
            cx.set_reduce_motion(true);
            view.update(cx, |view, cx| {
                view.open_menu(ComposerMenu::Settings, window, cx);
                view.close_menu(cx);
                assert!(
                    view.closing_popup.is_none(),
                    "reduced motion closes immediately"
                );
            });
        });
    }
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::Configure { .. } | Action::Submit { .. })));
}

#[gpui::test]
fn model_labels_fit_across_zoom_for_both_providers(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        for zoom in [0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.25, 1.5, 2.0] {
            cx.update(|window, cx| {
                    window.set_rem_size(gpui::px(crate::zoom::REM_BASE * zoom));
                    view.update(cx, |view, cx| {
                        view.session.update(cx, |session, cx| {
                            session.state.saved.provider = provider;
                            session.state.models = vec![json!({"model":"test", "displayName":"GPT-6.1 Terra", "supportedReasoningEfforts":[{"reasoningEffort":"medium"}]})].into();
                            session.state.model = Some("test".into());
                            session.state.effort = Some("medium".into());
                            cx.notify();
                        });
                        view.open_menu(ComposerMenu::Settings, window, cx);
                    });
                });
            draw(cx);
            let bounds = cx.debug_bounds("codex-card-model-label").unwrap();
            cx.update(|window, _| {
                let label = "GPT-6.1 Terra";
                let mut run = window.text_style().to_run(label.len());
                run.font.family = crate::fonts::UI_FAMILY.into();
                let required = truncation_safe_label_width(
                    label,
                    rpx(TEXT_SMALL).to_pixels(window.rem_size()),
                    &run,
                    window.text_system(),
                );
                assert!(
                    bounds.size.width + gpui::px(0.5) >= required,
                    "{provider:?} at {zoom}"
                );
            });
        }
    }
}

#[gpui::test]
fn composer_effort_card_commits_on_release_and_preserves_draft(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.update(|window, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.models = vec![json!({"model":"test", "displayName":"Test model", "defaultReasoningEffort":"medium", "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"medium"},{"reasoningEffort":"high"}]})].into();
                session.state.model = Some("test".into());
                session.state.effort = Some("medium".into());
                cx.notify();
            });
            view.input.update(cx, |input, cx| input.set_value("Keep my draft", window, cx));
            view.open_menu(ComposerMenu::Settings, window, cx);
        }));
    draw(cx);
    let summary = cx.debug_bounds("codex-model-effort-summary").unwrap();
    let card_model = cx.debug_bounds("codex-card-model-label").unwrap();
    cx.update(|window, _| {
        let label = "Test model · Medium";
        let mut run = window.text_style().to_run(label.len());
        run.font.family = crate::fonts::UI_FAMILY.into();
        let required = truncation_safe_label_width(
            label,
            rpx(TEXT_SMALL).to_pixels(window.rem_size()),
            &run,
            window.text_system(),
        );
        assert!(
            summary.size.width >= required,
            "ordinary model and effort labels must not ellipsize prematurely"
        );
        let model = "Test model";
        let mut run = window.text_style().to_run(model.len());
        run.font.family = crate::fonts::UI_FAMILY.into();
        let required = truncation_safe_label_width(
            model,
            rpx(TEXT_SMALL).to_pixels(window.rem_size()),
            &run,
            window.text_system(),
        );
        assert!(
            card_model.size.width >= required,
            "the card model name must fit before its chevron"
        );
    });
    let trigger = view.read_with(cx, |view, _| view.option_bounds[0].get());
    let card = cx.debug_bounds("codex-options-popup").unwrap();
    assert!(card.bottom() <= trigger.top());
    assert!((trigger.top() - card.bottom() - gpui::px(SPACE_LG)).abs() < gpui::px(1.0));
    let bounds = view.read_with(cx, |view, _| view.effort_bounds.get());
    let end = gpui::point(bounds.right(), bounds.center().y);
    let _ = actions.try_iter().collect::<Vec<_>>();
    cx.simulate_mouse_down(end, gpui::MouseButton::Left, gpui::Modifiers::default());
    draw(cx);
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::Configure { .. })));
    cx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::default());
    draw(cx);
    assert!(actions.try_iter().any(|action| matches!(action, Action::Configure { effort: Some(effort), model: None, access: None } if effort == "high")));
    cx.simulate_keystrokes("home right");
    draw(cx);
    let values: Vec<_> = actions
        .try_iter()
        .filter_map(|action| match action {
            Action::Configure { effort, .. } => effort,
            _ => None,
        })
        .collect();
    assert_eq!(values, ["low", "medium"]);
    let reset = cx.debug_bounds("codex-model-effort-reset").unwrap();
    cx.simulate_click(reset.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::ResetModelEffort)));
    cx.simulate_keystrokes("escape");
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert!(view.menu.is_none());
        assert_eq!(view.input.read(cx).value().as_ref(), "Keep my draft");
    });
}

#[gpui::test]
fn composer_effort_motion_tracks_pointer_and_settles(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    let advance = |cx: &mut gpui::VisualTestContext, millis| {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(millis));
        draw(cx);
    };
    let aligned = |cx: &mut gpui::VisualTestContext| {
        let thumb = cx.debug_bounds("codex-effort-thumb").unwrap();
        let fill = cx.debug_bounds("codex-effort-fill").unwrap();
        let slider = cx.debug_bounds("codex-effort-slider").unwrap();
        assert_eq!(
            thumb.size.width, thumb.size.height,
            "effort thumb must remain round"
        );
        assert!(
            thumb.left() >= slider.left() && thumb.right() <= slider.right(),
            "the complete effort thumb must stay inside its slider: {thumb:?}, {slider:?}"
        );
        assert!(
            (fill.right() - thumb.right()).abs() < gpui::px(1.0),
            "the rounded fill cap must stay covered by the moving thumb"
        );
        thumb.center().x
    };
    for (provider, supported) in [
        (codex::Provider::Codex, vec!["low", "medium", "high"]),
        (codex::Provider::Claude, vec!["low", "max"]),
    ] {
        cx.update(|window, cx| {
                cx.set_reduce_motion(false);
                view.update(cx, |view, cx| {
                    view.close_menu(cx);
                    view.session.update(cx, |session, cx| {
                        session.state.saved.provider = provider;
                        session.state.model = Some("motion-model".into());
                        session.state.effort = Some("low".into());
                        session.state.models = vec![
                            json!({"model":"motion-model", "supportedReasoningEfforts":supported.iter().map(|effort| json!({"reasoningEffort":effort})).collect::<Vec<_>>()}),
                            json!({"model":"fixed-model", "supportedReasoningEfforts":[{"reasoningEffort":"high"}]}),
                        ].into();
                        cx.notify();
                    });
                    view.open_menu(ComposerMenu::Settings, window, cx);
                });
            });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                CodexView::effort_choices(&view.session.read(cx).state),
                supported
            );
        });
        let path = view.read_with(cx, |view, _| view.effort_bounds.get());
        let start = gpui::point(path.left(), path.center().y);
        let end = gpui::point(path.right(), path.center().y);
        let before = aligned(cx);
        let _ = actions.try_iter().collect::<Vec<_>>();
        cx.simulate_click(end, gpui::Modifiers::default());
        draw(cx);
        advance(cx, 16);
        let intermediate = aligned(cx);
        assert!(
            intermediate > before && intermediate < path.right(),
            "click must render motion between supported stops instead of jumping"
        );
        assert_eq!(actions.try_iter().filter(|action| matches!(action, Action::Configure { effort: Some(effort), model: None, access: None } if effort == supported.last().unwrap())).count(), 1);
        // Reverse a moving click: its first frame must start where it was displayed.
        cx.simulate_click(start, gpui::Modifiers::default());
        draw(cx);
        assert!(
            (aligned(cx) - intermediate).abs() < gpui::px(1.0),
            "retargeting must not jump to the old semantic stop"
        );
        advance(cx, 16);
        assert!(aligned(cx) < intermediate && aligned(cx) > path.left());
        advance(cx, MOTION_BASE_MS);
        assert!((aligned(cx) - path.left()).abs() < gpui::px(1.0));
        let _ = actions.try_iter().collect::<Vec<_>>();
        // Move between stops, not onto a dot. Capture must work despite the card's occlusion.
        let between = gpui::point(path.left() + path.size.width * 0.35, path.center().y);
        cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            between,
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        draw(cx);
        advance(cx, 16);
        assert!(
            aligned(cx) > path.left() && aligned(cx) < between.x,
            "dragging must move responsively toward the continuous pointer position"
        );
        advance(cx, MOTION_FAST_MS);
        assert!(
            (aligned(cx) - between.x).abs() < gpui::px(1.0),
            "thumb must follow the pointer between discrete stops"
        );
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Configure { .. })));
        cx.simulate_mouse_up(between, gpui::MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        let index = effort_stop(0.35, supported.len());
        let writes: Vec<_> = actions
            .try_iter()
            .filter_map(|action| match action {
                Action::Configure { effort, .. } => effort,
                _ => None,
            })
            .collect();
        assert_eq!(writes, [supported[index]]);
        advance(cx, MOTION_BASE_MS);
        let stop_x = path.left() + path.size.width * effort_fraction(index, supported.len());
        assert!(
            (aligned(cx) - stop_x).abs() < gpui::px(1.0),
            "release must settle onto the committed supported stop"
        );
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Configure { .. })));
        // Capture continues outside both the track and the card.
        let outside = gpui::point(
            path.right() + gpui::px(ICON_BTN_W),
            path.top() - gpui::px(ICON_BTN_W),
        );
        cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            outside,
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        draw(cx);
        advance(cx, MOTION_FAST_MS);
        assert!((aligned(cx) - path.right()).abs() < gpui::px(1.0));
        cx.simulate_mouse_up(outside, gpui::MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert_eq!(actions.try_iter().filter(|action| matches!(action, Action::Configure { effort: Some(effort), .. } if effort == supported.last().unwrap())).count(), 1);
        // Enabling Reduce motion midway snaps to the target and stops scheduling frames.
        cx.simulate_click(start, gpui::Modifiers::default());
        draw(cx);
        advance(cx, 16);
        assert!(aligned(cx) > path.left());
        cx.update(|_, cx| cx.set_reduce_motion(true));
        draw(cx);
        assert!((aligned(cx) - path.left()).abs() < gpui::px(1.0));
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
        draw(cx);
        // Other component callbacks can remain queued. The slider schedules
        // its frames by notifying this view; observe that boundary directly.
        let frame_notifications = Rc::new(Cell::new(0));
        let observed = frame_notifications.clone();
        let frame_watch = cx.update(|_, cx| {
            cx.observe(&view, move |_, _| {
                observed.set(observed.get() + 1);
            })
        });
        for _ in 0..3 {
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
            });
            draw(cx);
        }
        assert_eq!(
            frame_notifications.get(),
            0,
            "reduced slider motion must not request view animation frames"
        );
        drop(frame_watch);
        view.read_with(cx, |view, _| {
            let visual = view.effort_visual.as_ref().unwrap();
            assert!(visual.started.is_none());
            assert_eq!(visual.from, visual.target);
        });
        let _ = actions.try_iter().collect::<Vec<_>>();
        cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            between,
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        draw(cx);
        assert!((aligned(cx) - between.x).abs() < gpui::px(1.0));
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Configure { .. })));
        cx.simulate_mouse_up(between, gpui::MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert!((aligned(cx) - stop_x).abs() < gpui::px(1.0));
        assert_eq!(
            actions
                .try_iter()
                .filter(|action| matches!(action, Action::Configure { .. }))
                .count(),
            1
        );
        // Changing selected models must replace the range, including fixed and absent catalogs.
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.model = Some("fixed-model".into());
                    session.state.effort = Some("high".into());
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!((aligned(cx) - path.center().x).abs() < gpui::px(1.0));
        cx.simulate_click(end, gpui::Modifiers::default());
        draw(cx);
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Configure { .. })));
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.model = Some("unknown-model".into());
                    cx.notify();
                });
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("codex-effort-thumb").is_none());
    }
}

#[gpui::test]
fn composer_effort_motion_reconciles_async_reset_and_acknowledged_updates(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.model = Some("same-model".into());
                session.state.effort = Some("high".into());
                session.state.resolved_effort = Some("low".into());
                session.state.models = vec![json!({"model":"same-model", "defaultReasoningEffort":"low", "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"}]})].into();
                cx.notify();
            });
            view.open_menu(ComposerMenu::Settings, window, cx);
        }));
    draw(cx);
    let path = view.read_with(cx, |view, _| view.effort_bounds.get());
    let assert_stop = |cx: &mut gpui::VisualTestContext, expected| {
        let thumb = cx.debug_bounds("codex-effort-thumb").unwrap();
        let fill = cx.debug_bounds("codex-effort-fill").unwrap();
        assert!(
            (thumb.center().x - expected).abs() < gpui::px(1.0),
            "thumb must match the effective effort after asynchronous updates"
        );
        assert!((fill.right() - thumb.right()).abs() < gpui::px(1.0));
    };
    assert_stop(cx, path.right());
    let _ = actions.try_iter().collect::<Vec<_>>();
    let reset = cx.debug_bounds("codex-model-effort-reset").unwrap();
    cx.simulate_click(reset.center(), gpui::Modifiers::default());
    // send() queues the action; this redraw still sees the pre-reset High snapshot.
    draw(cx);
    assert_stop(cx, path.right());
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::ResetModelEffort)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.model = None;
                session.state.resolved_model = Some("same-model".into());
                session.state.effort = None;
                session.state.resolved_effort = Some("low".into());
                cx.notify();
            });
        });
    });
    draw(cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(MOTION_BASE_MS));
    draw(cx);
    assert_stop(cx, path.left());
    view.read_with(cx, |view, cx| {
        let state = &view.session.read(cx).state;
        let choices = CodexView::effort_choices(state);
        assert_eq!(view.effort_index(state, &choices), Some(0));
        assert_eq!(effort_label(&choices[0]), "Low");
    });
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::Configure { .. })));
    // An unsolicited effort change for the same model must also move the thumb.
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.effort = Some("high".into());
                cx.notify();
            });
        });
    });
    draw(cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(MOTION_BASE_MS));
    draw(cx);
    assert_stop(cx, path.right());
    // Once a local choice is acknowledged, it must not mask later provider updates.
    cx.simulate_click(
        gpui::point(path.left(), path.center().y),
        gpui::Modifiers::default(),
    );
    draw(cx);
    assert_eq!(actions.try_iter().filter(|action| matches!(action, Action::Configure { effort: Some(effort), .. } if effort == "low")).count(), 1);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.effort = Some("low".into());
                cx.notify();
            });
        });
    });
    draw(cx);
    view.read_with(cx, |view, _| assert!(view.effort_pending.is_none()));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.effort = Some("high".into());
                cx.notify();
            });
        });
    });
    draw(cx);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(MOTION_BASE_MS));
    draw(cx);
    assert_stop(cx, path.right());
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::Configure { .. })));
}

#[gpui::test]
fn composer_effort_drag_cancels_on_escape_and_catalog_or_session_change(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.model = Some("test".into());
                session.state.models = vec![json!({"model":"test", "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"}]})].into();
                cx.notify();
            });
            view.open_menu(ComposerMenu::Settings, window, cx);
        }));
    draw(cx);
    let position = view.read_with(cx, |view, _| view.effort_bounds.get().center());
    let _ = actions.try_iter().collect::<Vec<_>>();
    cx.simulate_mouse_down(
        position,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    draw(cx);
    cx.simulate_keystrokes("escape");
    view.read_with(cx, |view, _| {
        assert!(view.effort_visual.is_none());
        assert!(view.effort_drag.is_none());
    });
    cx.simulate_mouse_up(
        position,
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    draw(cx);
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::Configure { .. })));
    for change_session in [false, true] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.models[0]["supportedReasoningEfforts"] =
                        json!([{"reasoningEffort":"low"},{"reasoningEffort":"high"}]);
                    cx.notify();
                });
                view.open_menu(ComposerMenu::Settings, window, cx);
            });
        });
        draw(cx);
        let position = view.read_with(cx, |view, _| view.effort_bounds.get().center());
        cx.simulate_mouse_down(
            position,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        draw(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    if change_session {
                        session.state.saved.key = "different-session".into();
                    } else {
                        session.state.models[0]["supportedReasoningEfforts"] =
                            json!([{ "reasoningEffort":"low" }]);
                    }
                    cx.notify();
                });
            });
        });
        draw(cx);
        cx.simulate_mouse_up(
            position,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        draw(cx);
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Configure { .. })));
    }
}

#[gpui::test]
fn codex_and_claude_access_popup_pointer_selection_dispatches_configuration(
    cx: &mut gpui::TestAppContext,
) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let (view, cx, actions, _) = fixture(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    session.state.permission_modes = ["default", "acceptEdits", "plan"]
                        .map(str::to_owned)
                        .to_vec();
                    cx.notify();
                });
            });
        });
        draw(cx);
        for rejected in [false, true] {
            if rejected {
                cx.update(|_, cx| {
                    view.update(cx, |view, cx| {
                        view.session.update(cx, |session, cx| {
                            session.state.error = Some("mode unavailable".into());
                            cx.notify();
                        });
                    });
                });
                draw(cx);
            }
            let trigger = view.read_with(cx, |view, _| view.option_bounds[2].get());
            cx.simulate_mouse_move(trigger.center(), None, gpui::Modifiers::default());
            draw(cx);
            cx.executor()
                .advance_clock(std::time::Duration::from_secs(1));
            draw(cx);
            cx.simulate_click(trigger.center(), gpui::Modifiers::default());
            draw(cx);
            let popup = cx.debug_bounds("codex-options-popup").unwrap();
            // Both fixtures have four rows; use the rendered menu's second row.
            let second_row = gpui::point(popup.center().x, popup.top() + popup.size.height * 0.375);
            cx.simulate_mouse_move(second_row, None, gpui::Modifiers::default());
            draw(cx);
            cx.simulate_mouse_down(
                second_row,
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            draw(cx);
            cx.simulate_mouse_up(
                second_row,
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            draw(cx);
            let expected = if provider == codex::Provider::Claude {
                "acceptEdits"
            } else {
                "workspace-write"
            };
            assert!(actions.try_iter().any(|action| matches!(action, Action::Configure { access: Some(access), model: None, effort: None } if access == expected)));
            assert!(view.read_with(cx, |view, _| view.popup.is_none()));
        }
    }
}

#[gpui::test]
fn option_popup_is_anchored_without_resizing_composer_and_handles_keyboard_and_outside_click(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    draw(cx);
    let input = cx.debug_bounds("codex-composer-input").unwrap();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.open_menu(ComposerMenu::Access, window, cx);
        });
    });
    draw(cx);
    let popup = cx.debug_bounds("codex-options-popup").unwrap();
    assert_eq!(cx.debug_bounds("codex-composer-input").unwrap(), input);
    let trigger = view.read_with(cx, |view, _| view.option_bounds[2].get());
    assert!(popup.bottom() <= trigger.top());
    cx.simulate_keystrokes("down enter");
    draw(cx);
    assert!(actions.try_iter().any(|action| matches!(action, Action::Configure { access: Some(access), .. } if access == "read-only")));
    assert!(view.read_with(cx, |view, _| view.popup.is_none()));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.open_menu(ComposerMenu::Access, window, cx);
        });
    });
    draw(cx);
    // Pointer navigation remains actionable without a selected fill. Keyboard
    // navigation uses the same index that the native menu paints as selected.
    let menu_bounds = cx.debug_bounds("popup-menu-item-Integer(0)").unwrap();
    cx.simulate_mouse_move(menu_bounds.center(), None, gpui::Modifiers::default());
    draw(cx);
    let menu = view.read_with(cx, |view, _| view.popup.clone().unwrap());
    cx.update(|_, cx| assert_eq!(menu.read(cx).test_highlighted_index(), None));
    cx.simulate_keystrokes("down");
    draw(cx);
    cx.update(|_, cx| assert!(menu.read(cx).test_highlighted_index().is_some()));
    // Moving off the menu must not clear the keyboard-selected row.
    cx.simulate_mouse_move(
        gpui::point(gpui::px(10.), gpui::px(10.)),
        None,
        gpui::Modifiers::default(),
    );
    draw(cx);
    cx.update(|_, cx| assert!(menu.read(cx).test_highlighted_index().is_some()));
    cx.simulate_mouse_move(menu_bounds.center(), None, gpui::Modifiers::default());
    draw(cx);
    cx.update(|_, cx| assert_eq!(menu.read(cx).test_highlighted_index(), None));
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.popup.is_none()));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.open_menu(ComposerMenu::Access, window, cx);
        });
    });
    draw(cx);
    cx.simulate_mouse_down(
        gpui::point(gpui::px(10.), gpui::px(10.)),
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.simulate_mouse_up(
        gpui::point(gpui::px(10.), gpui::px(10.)),
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    draw(cx);
    assert!(view.read_with(cx, |view, _| view.popup.is_none()));
}
