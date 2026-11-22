use super::*;

#[test]
fn composer_markdown_styles_nested_unicode_source_without_overlapping_ranges() {
    let source =
        "🦀 **bold *é*** and ~~gone~~ and `code`\n\n# Heading\n\n[site](https://example.com)";
    let decorations = super::super::composer_markdown_decorations(source);
    let at = |text: &str| {
        let offset = source.find(text).unwrap();
        decorations
            .iter()
            .find(|d| d.range.contains(&offset))
            .unwrap()
            .style
    };
    assert_eq!(at("bold").font_weight, Some(gpui::FontWeight::BOLD));
    assert_eq!(at("é").font_weight, Some(gpui::FontWeight::BOLD));
    assert_eq!(at("é").font_style, Some(gpui::FontStyle::Italic));
    assert!(at("gone").strikethrough.is_some());
    assert!(at("code").background_color.is_some());
    assert_eq!(at("Heading").font_weight, Some(gpui::FontWeight::BOLD));
    assert!(at("site").underline.is_some());
    for pair in decorations.windows(2) {
        assert!(pair[0].range.end <= pair[1].range.start);
    }
    for decoration in decorations {
        assert!(source.get(decoration.range).is_some());
    }
    assert!(super::super::composer_markdown_decorations("**unfinished").is_empty());
}

#[test]
fn composer_markdown_projects_headings_lists_and_inline_syntax() {
    let source = "## Heading 2\n- A **bold** item with `code` and [link](https://example.com)\n1. Ordered 🦀";
    let replacements = super::super::composer_markdown_replacements(source);
    let mut rendered = String::new();
    let mut previous = 0;
    for replacement in &replacements {
        assert!(replacement.range.start >= previous);
        rendered.push_str(&source[previous..replacement.range.start]);
        rendered.push_str(&replacement.text);
        previous = replacement.range.end;
    }
    rendered.push_str(&source[previous..]);
    assert_eq!(
        rendered,
        "Heading 2\n• A bold item with code and link\n1. Ordered 🦀"
    );
    assert!(replacements.iter().any(|r| r.font_scale > 1.));
    assert!(super::super::composer_markdown_replacements("**unfinished").is_empty());
}

#[test]
fn fuzzy_completion_matches_skills_and_commands_for_both_providers() {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let mut state = snapshot();
        state.saved.provider = provider;
        state.skills = (0..10)
            .map(|i| json!({"name":format!("ship-pr-auto-{i}")}))
            .collect();
        state.skills.push(json!({"name":"shipau-exact"}));
        state
            .skills
            .push(json!({"name":"shipau-disabled", "enabled":false}));
        state.skills.push(json!({"name":"review"}));
        let prefix = if provider == codex::Provider::Claude {
            "/"
        } else {
            "$"
        };
        let choices = skill_choices(&state, "ShIpAu");
        assert_eq!(choices.len(), 8);
        assert_eq!(choices[0].0, format!("{prefix}shipau-exact"));
        assert!(choices
            .iter()
            .any(|(name, _)| name == &format!("{prefix}ship-pr-auto-0")));
        assert!(!choices.iter().any(|(name, _)| name.contains("disabled")));
        assert!(skill_choices(&state, "zzzz").is_empty());
        assert!(skill_choices(&state, "uapihs").is_empty());
        assert_eq!(skill_choices(&state, "dscch")[0].0, "$describe-changes");
        assert_eq!(slash_choices(&state, "CMP")[0].0, "/compact");
        assert_eq!(slash_choices(&state, "rvw")[0].0, "/review");
        assert_eq!(
            slash_choices(&state, "")
                .iter()
                .filter(|(name, _)| name == "/review")
                .count(),
            1
        );
        if provider == codex::Provider::Claude {
            assert_eq!(slash_choices(&state, "shipau")[0].0, "/shipau-exact");
        } else {
            assert!(slash_choices(&state, "shipau").is_empty());
        }
    }
}

#[test]
fn attachment_file_choices_search_nested_worktree_files() {
    let checkout = tempfile::tempdir().unwrap();
    let root = checkout.path();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    fs_err::create_dir_all(root.join("src/services/tasks")).unwrap();
    fs_err::write(root.join("src/services/tasks/add_task_service.js"), "").unwrap();
    fs_err::write(root.join("ship-pr-auto.md"), "").unwrap();
    assert!(file_choices(root.to_str().unwrap(), "shipau").contains(&"ship-pr-auto.md".into()));
    fs_err::write(root.join(".gitignore"), "ignored/\n").unwrap();
    fs_err::create_dir_all(root.join("ignored")).unwrap();
    fs_err::write(root.join("ignored/add_task.js"), "").unwrap();
    let cwd = root.to_str().unwrap();
    for query in [
        "add_task",
        "ADD_TASK",
        "adtsk",
        "src/services/tasks/add",
        "tasks/",
    ] {
        assert!(
            file_choices(cwd, query).contains(&"src/services/tasks/add_task_service.js".into()),
            "{query}"
        );
    }
    assert!(!file_choices(cwd, "add_task").contains(&"ignored/add_task.js".into()));
    assert!(file_choices(cwd, "no_such_file").is_empty());
    let other = tempfile::tempdir().unwrap();
    assert!(file_choices(other.path().to_str().unwrap(), "add_task").is_empty());
}

#[gpui::test]
fn composer_markdown_preserves_source_navigation_copy_and_ime_for_both_providers(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::EntityInputHandler as _;
    let (view, cx, _, _) = fixture(cx);
    let source = "## Heading 2\n- **é🦀** item with `code`\n1. Ordered\n\n```sh\nRAILS_ENV=production bin/rails runner 'puts 123'\n```\n\nAfter code";
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session
                    .update(cx, |session, _| session.state.saved.provider = provider);
                view.input.update(cx, |input, cx| {
                    input.set_value(source, window, cx);
                    input.set_selected_range(3..3, cx);
                    input.focus(window, cx);
                });
            });
        });
        draw(cx);
        let bounds = cx.debug_bounds("codex-composer-input").unwrap();
        let (heading, item) = cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    let heading = input.bounds_for_range(3..3, bounds, window, cx).unwrap();
                    let item_start = source.find('é').unwrap();
                    let utf16 = source[..item_start].encode_utf16().count();
                    let item = input
                        .bounds_for_range(utf16..utf16, bounds, window, cx)
                        .unwrap();
                    assert!(
                        item.top() > heading.top(),
                        "projected source offsets retain separate rows"
                    );
                    (heading, item)
                })
            })
        });
        cx.simulate_click(
            heading.origin + gpui::point(gpui::px(0.1), heading.size.height / 2.),
            gpui::Modifiers::default(),
        );
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.input.read(cx).cursor(),
                3,
                "clicking rendered heading begins after source hashes"
            );
        });
        cx.simulate_click(
            item.origin + gpui::point(gpui::px(0.1), item.size.height / 2.),
            gpui::Modifiers::default(),
        );
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.input.read(cx).cursor(),
                source.find('é').unwrap(),
                "clicking Unicode list content maps through hidden strong and bullet syntax"
            );
        });
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.input
                    .update(cx, |input, cx| input.set_selected_range(3..3, cx));
            });
        });
        draw(cx);
        cx.simulate_keystrokes("left");
        draw(cx);
        view.read_with(cx, |view, cx| assert_eq!(view.input.read(cx).cursor(), 2));
        cx.simulate_keystrokes("right");
        draw(cx);
        view.read_with(cx, |view, cx| assert_eq!(view.input.read(cx).cursor(), 3));
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-a cmd-c"
        } else {
            "ctrl-a ctrl-c"
        });
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some(source)
            );
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_selected_range(source.len()..source.len(), cx);
                    input.replace_and_mark_text_in_range(None, "日本", Some(2..2), window, cx);
                });
            });
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                format!("{source}日本")
            );
            assert_eq!(view.input.read(cx).cursor(), source.len() + "日本".len());
        });
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.replace_text_in_range(None, "日本語", window, cx);
                });
            });
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                format!("{source}日本語")
            );
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-z"
        } else {
            "ctrl-z"
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                source,
                "presentation does not add undo edits"
            );
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-shift-z"
        } else {
            "ctrl-y"
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                format!("{source}日本語")
            );
        });
    }
}

#[gpui::test]
fn composer_markdown_keeps_literal_draft_caret_and_picked_tokens(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    let source = "🦀 **bold** @src/main.rs\n`code`";
    let range = source.find('@').unwrap()..source.find('\n').unwrap();
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_value(source, window, cx);
                input.set_selected_range(4..4, cx);
            });
            let decoration = view.input.update(cx, |input, cx| {
                input.create_decorations_collection(
                    vec![TextDecoration::new(
                        range.clone(),
                        gpui::HighlightStyle {
                            underline: Some(gpui::UnderlineStyle::default()),
                            ..Default::default()
                        },
                    )],
                    cx,
                )
            });
            view.picked_tokens.push(ComposerToken {
                value: "@src/main.rs".into(),
                decoration,
            });
        });
    });
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.input.read(cx).value().as_ref(), source);
        assert_eq!(view.input.read(cx).cursor(), 4);
        assert!(!view.markdown_decorations.get_ranges(cx).is_empty());
        assert_eq!(view.picked_tokens[0].decoration.get_ranges(cx), vec![range]);
    });
}

#[gpui::test]
fn mixed_table_attachment_history_reuses_geometry_and_preserves_selection(
    cx: &mut gpui::TestAppContext,
) {
    fn frame(cx: &mut gpui::VisualTestContext) {
        // Measurement corrections are deferred to the next display frame.
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        cx.update(gpui::Window::simulate_next_frame);
        draw(cx);
    }
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|_, cx| view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    session.state.saved.thread_id = Some(format!("tables-{provider:?}"));
                    session.state.saved.items = (0..40).map(|i| json!({
                        "id":format!("table-{i}"), "type":if i % 4 == 0 {"userMessage"} else {"agentMessage"},
                        "content":[{"type":"text","text":"Seven days with an attached screenshot"},{"type":"image","url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jM1sAAAAASUVORK5CYII="}],
                        "text":format!("| Decision {i} | Answer |\n| --- | --- |\n| Default access with a long explanation | Member access with enough text to wrap at narrow widths |\n| Expiration | Seven days |"),
                        "phase":"final_answer", "groveCompleted":true,
                    })).collect();
                    session.state.status = Status::Idle;
                    cx.notify();
                });
                cx.notify();
            }));
        // Initialize session ownership before expanding the full-history fixture.
        draw(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                let items = view.session.read(cx).state.saved.items.clone();
                view.turn_history.expand_all_for_test(&items);
                cx.notify();
            });
        });
        for (width, rem) in [(1000., 16.), (420., 16.), (420., 22.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.)));
            cx.update(|_, cx| gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem));
            for _ in 0..4 {
                cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
                frame(cx);
            }
            measured_slot::MATERIALIZED.with(|count| count.set(0));
            cx.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
            frame(cx);
            let materialized = measured_slot::MATERIALIZED.with(Cell::get);
            assert!(materialized < 15,
                    "{provider:?} width={width} rem={rem}: settled frame materialized {materialized} rows");
            let extent = view.read_with(cx, |view, _| view.scroll.max_offset());
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.scroll.scroll_to_bottom();
                    cx.notify();
                });
            });
            frame(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(view.scroll.max_offset(), extent);
                assert_eq!(view.selection_views.len(), 40);
                assert!(view
                    .selection_views
                    .values()
                    .all(|owner| owner.read(cx).supports_layout_projection()));
            });
            // Selecting every retained owner forces the original layout path.
            // Its height and text must agree with the measured offscreen path.
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    for owner in view.selection_views.values() {
                        owner.update(cx, gpui_component::text::TextViewState::select_all);
                    }
                    cx.notify();
                });
            });
            frame(cx);
            view.read_with(cx, |view, cx| {
                assert_eq!(view.scroll.max_offset(), extent);
                assert!(view
                    .selection_views
                    .values()
                    .all(|owner| owner.read(cx).selected_text().contains("Seven days")));
            });
            // Discard selected view owners before measuring the next size.
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.selection_views.clear();
                    view.message_slots.clear();
                    cx.notify();
                });
            });
        }
    }
}

#[gpui::test]
fn completion_rows_remain_available_during_typing(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                view.session.update(cx, |session, _| {
                    session.state.saved.provider = provider;
                    session.state.skills = vec![json!({"name":"review","enabled":true})].into();
                });
                view.file_suggestions = vec!["src/main.rs".into()];
                view.file_lookup = Some("s".into());
                view.file_query = "s".into();
                for text in ["@sr", "@src", "/", "/a", "/ac", "$", "$r", "$re"] {
                    view.input.update(cx, |input, cx| {
                        input.set_value(text, window, cx);
                        input.set_selected_range(text.len()..text.len(), cx);
                    });
                    view.refresh_files(cx);
                    let state = &view.session.read(cx).state;
                    let (choices, _) = view
                        .suggestion_choices(state, cx)
                        .unwrap_or_else(|| panic!("missing tray for {provider:?}: {text}"));
                    assert!(!choices.is_empty(), "{provider:?}: {text}");
                    if text.starts_with('@') {
                        assert_eq!(choices[0].0, "@src/main.rs");
                    }
                }
                view.input
                    .update(cx, |input, cx| input.set_value("plain", window, cx));
                view.refresh_files(cx);
                assert!(view.file_suggestions.is_empty());
                assert!(view.file_lookup.is_none());
                assert!(view
                    .suggestion_choices(&view.session.read(cx).state, cx)
                    .is_none());
            }
        });
    });
}

#[gpui::test]
fn tab_accepts_slash_command_without_executing_until_enter(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_value("/acc", window, cx);
                input.set_selected_range(4..4, cx);
            });
            view.input.focus_handle(cx).focus(window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes("tab");
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.input.read(cx).value().as_ref(), "/access ");
        assert!(view.menu.is_none());
        assert_eq!(view.picked_tokens.len(), 1);
    });
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::Configure { .. } | Action::Submit { .. })));
    cx.simulate_keystrokes("enter");
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert!(view.menu == Some(ComposerMenu::Access));
        assert!(view.input.read(cx).value().is_empty());
    });
}

#[test]
fn completion_trigger_steps_past_multibyte_whitespace() {
    // U+00A0 is whitespace but two bytes wide; the token starts after all of it.
    let text = "see\u{a0}@src next";
    let cursor = "see\u{a0}@src".len();
    assert_eq!(completion_trigger(text, cursor), Some((5..9, '@', "src")));
    let text = "Image 1\u{a0} We";
    assert_eq!(completion_trigger(text, "Image 1\u{a0}".len()), None);
}

#[gpui::test]
fn busy_shortcuts_queue_and_steer_preserve_acknowledged_draft(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Working;
                session.state.turn_id = Some("active-turn".into());
                cx.notify();
            });
            view.input
                .update(cx, |input, cx| input.set_value("follow up", window, cx));
            view.input.focus_handle(cx).focus(window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes("enter");
    draw(cx);
    let queue_id = view.read_with(cx, |view, _| view.submitted_ack.unwrap());
    let queued_actions = actions.try_iter().collect::<Vec<_>>();
    assert!(
        queued_actions.iter().any(|action| matches!(action,
            Action::SubmitWithMode { id, text, mode: SubmissionMode::Queue, .. }
                if *id == queue_id && text.trim() == "follow up")),
        "{}",
        queued_actions
            .iter()
            .map(|action| match action {
                Action::SubmitWithMode { id, text, mode, .. } =>
                    format!("mode={mode:?} id={id} text={text:?}"),
                Action::Submit { id, text, .. } => format!("send id={id} text={text:?}"),
                _ => "other".to_owned(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    );
    assert_eq!(
        view.read_with(cx, |view, cx| view.input.read(cx).value().trim().to_owned()),
        "follow up"
    );
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.accepted_submission = Some(queue_id);
                session.state.queued_prompts.push(codex::QueuedPrompt {
                    id: queue_id,
                    text: "follow up".into(),
                    images: Vec::new(),
                });
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(view.read_with(cx, |view, cx| view.input.read(cx).value().is_empty()));
    assert!(cx.debug_bounds("codex-queued-prompts").is_some());
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_value("change direction", window, cx);
            });
            view.input.focus_handle(cx).focus(window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes("secondary-enter");
    draw(cx);
    let steer_id = view.read_with(cx, |view, _| view.submitted_ack.unwrap());
    assert!(actions.try_iter().any(|action| matches!(action,
            Action::SubmitWithMode { id, mode: SubmissionMode::Steer, .. } if id == steer_id)));
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.rejected_submission = Some(steer_id);
                cx.notify();
            });
        });
    });
    draw(cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view.input.read(cx).value().trim().to_owned()),
        "change direction"
    );
}

#[gpui::test]
fn next_reply_completion_and_submission_ack_in_one_frame(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            seed_next_reply(
                view,
                codex::Provider::Codex,
                "Fix draft preservation.",
                "Add regression coverage.",
                window,
                cx,
            );
            view.next_reply_seen = None;
            view.next_reply_request = None;
            view.submitted = Some("Fix draft preservation.".into());
            view.submitted_ack = Some(42);
            view.session.update(cx, |session, _| {
                session.state.accepted_submission = Some(42);
            });
            view.next_reply_generate = |_, _| Ok(vec!["Add regression coverage.".into()]);
        });
    });
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert!(view.submitted.is_none());
        assert!(view.input.read(cx).value().is_empty());
        assert!(
            view.next_reply_request.is_some(),
            "coalesced completion must start generation after clearing the submitted draft"
        );
    });
}

#[gpui::test]
fn shell_shortcut_steers_only_from_main_input_and_preserves_zen_elsewhere(
    cx: &mut gpui::TestAppContext,
) {
    struct ShellProbe {
        chat: Entity<CodexView>,
        zen_toggles: usize,
        outside: FocusHandle,
    }
    impl Render for ShellProbe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(self.chat.clone())
                .child(div().track_focus(&self.outside))
                .on_action(cx.listener(|this, _: &crate::keymap::ToggleZen, _, _| {
                    this.zen_toggles += 1;
                }))
        }
    }
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.bind_keys(crate::keymap::shell_bindings());
    });
    let (handle, actions) = Handle::channel();
    let (_updates, receive) = mpsc::channel();
    let mut chat = None;
    let (shell, cx) = cx.add_window_view(|window, cx| {
        let session = cx.new(|cx| {
            CodexSession::from_connection(
                snapshot(),
                Connection {
                    handle,
                    updates: receive.into(),
                },
                cx,
            )
        });
        let view = cx.new(|cx| CodexView::new(session, window, cx));
        chat = Some(view.clone());
        ShellProbe {
            chat: view,
            zen_toggles: 0,
            outside: cx.focus_handle(),
        }
    });
    let view = chat.unwrap();
    let shell_enter = format!("{}enter", crate::keymap::platform_mod_prefix());
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.status = Status::Working;
                session.state.turn_id = Some("active-turn".into());
            });
            view.input
                .update(cx, |input, cx| input.set_value("steer this", window, cx));
            view.input.focus_handle(cx).focus(window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes(&shell_enter);
    draw(cx);
    assert!(actions.try_iter().any(|action| matches!(
        action,
        Action::SubmitWithMode {
            mode: SubmissionMode::Steer,
            ..
        }
    )));
    assert_eq!(shell.read_with(cx, |shell, _| shell.zen_toggles), 0);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.submitted = None;
            view.submitted_ack = None;
            view.input
                .update(cx, |input, cx| input.set_value("line", window, cx));
            view.input.focus_handle(cx).focus(window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes("shift-enter");
    draw(cx);
    assert!(view.read_with(cx, |view, cx| view.input.read(cx).value().contains('\n')));
    assert!(!actions.try_iter().any(|action| matches!(
        action,
        Action::SubmitWithMode { .. } | Action::Submit { .. }
    )));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.status = Status::Idle;
                session.state.turn_id = None;
            });
            view.input
                .update(cx, |input, cx| input.set_value("send this", window, cx));
        });
    });
    draw(cx);
    cx.simulate_keystrokes(&shell_enter);
    draw(cx);
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::Submit { .. })));
    assert_eq!(shell.read_with(cx, |shell, _| shell.zen_toggles), 0);
    cx.update(|window, cx| {
        let focus = view.read(cx).option_focus[0].clone();
        focus.focus(window, cx);
    });
    draw(cx);
    cx.simulate_keystrokes(&shell_enter);
    draw(cx);
    assert_eq!(shell.read_with(cx, |shell, _| shell.zen_toggles), 1);
    cx.update(|window, cx| {
        let focus = shell.read(cx).outside.clone();
        focus.focus(window, cx);
    });
    draw(cx);
    cx.simulate_keystrokes(&shell_enter);
    draw(cx);
    assert_eq!(shell.read_with(cx, |shell, _| shell.zen_toggles), 2);
}

#[gpui::test]
fn removing_images_removes_and_renumbers_inline_references(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.images = vec!["first".into(), "second".into()];
            view.input.update(cx, |input, cx| {
                input.set_value("Compare Image 1 with Image 2", window, cx);
            });
            view.restore_image_tokens(window, cx);
            assert_eq!(view.picked_tokens.len(), 2);
            view.remove_image(0, window, cx);
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                "Compare  with Image 1"
            );
            assert_eq!(view.picked_tokens.len(), 1);
            view.remove_image(0, window, cx);
            assert_eq!(view.input.read(cx).value().as_ref(), "Compare  with ");
            assert!(view.picked_tokens.is_empty());
            assert!(view.images.is_empty());
        });
    });
}

#[gpui::test]
fn attachment_squares_clear_the_input_and_tool_text_aligns_with_messages(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.expanded.insert("work-0".into());
            for (width, height) in [(344, 666), (1320, 500)] {
                let mut bytes = std::io::Cursor::new(Vec::new());
                image::DynamicImage::new_rgba8(width, height)
                    .write_to(&mut bytes, image::ImageFormat::Png)
                    .unwrap();
                let url = format!(
                    "data:image/png;base64,{}",
                    STANDARD.encode(bytes.into_inner())
                );
                view.previews
                    .insert(url.clone(), image_from_url(&url).unwrap());
                view.images.push(url);
            }
            view.session.update(cx, |session, cx| {
                session.state.saved.items = vec![
                    json!({"id":"a","type":"commandExecution","command":"pwd"}),
                    json!({"id":"b","type":"agentMessage","text":"Ready"}),
                ]
                .into();
                cx.notify();
            });
            cx.notify();
        });
    });
    for width in [600.0, 1280.0] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0)));
        draw(cx);
        let edge = cx.update(|window, _| rpx(IMAGE_PREVIEW_EDGE).to_pixels(window.rem_size()));
        let attachments = cx.debug_bounds("codex-attachments").unwrap();
        let input = cx.debug_bounds("codex-composer-input").unwrap();
        let composer = cx.debug_bounds("codex-composer").unwrap();
        let hints = cx.debug_bounds("codex-composer-hints").unwrap();
        assert!(
            hints.top() >= composer.bottom(),
            "instructions belong outside the border"
        );
        assert!(
            attachments.top() > composer.top() && input.top() > attachments.bottom(),
            "attachments need clear space above and below"
        );
        for selector in ["codex-image-0", "codex-image-1"] {
            let image = cx.debug_bounds(selector).unwrap();
            assert_eq!(image.size.width, edge);
            assert_eq!(image.size.height, edge);
            assert!(image.bottom() <= input.top(), "attachment overlaps input");
            assert!(image.left() >= attachments.left() && image.right() <= attachments.right());
        }
        assert_eq!(attachments.size.height, edge);
        let icon = cx.debug_bounds("codex-tool-icon-0").unwrap();
        let label = cx.debug_bounds("codex-tool-row-label-0").unwrap();
        let body = cx.debug_bounds("codex-message-body-1").unwrap();
        assert_eq!(
            icon.left(),
            body.left(),
            "tool icon must begin at the transcript rail"
        );
        assert!(
            label.left() > icon.right(),
            "tool label follows its icon with a gap"
        );
    }
}

#[gpui::test]
fn user_message_attachments_are_small_squares_below_text(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            let mut content = vec![json!({"type":"text", "text":"Review these screenshots"})];
            for (width, height) in [(344, 666), (1320, 500)] {
                let mut bytes = std::io::Cursor::new(Vec::new());
                image::DynamicImage::new_rgba8(width, height)
                    .write_to(&mut bytes, image::ImageFormat::Png)
                    .unwrap();
                let url = format!(
                    "data:image/png;base64,{}",
                    STANDARD.encode(bytes.into_inner())
                );
                content.push(json!({"type":"image", "url":url}));
            }
            view.session.update(cx, |session, cx| {
                session.state.saved.items =
                    vec![json!({"id":"screenshots", "type":"userMessage", "content":content})]
                        .into();
                cx.notify();
            });
            cx.notify();
        });
    });
    for width in [320.0, 1280.0] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0)));
        draw(cx);
        let edge = cx.update(|window, _| rpx(IMAGE_PREVIEW_EDGE).to_pixels(window.rem_size()));
        let body = cx.debug_bounds("codex-message-body-0").unwrap();
        let attachments = cx.debug_bounds("codex-message-attachments-0").unwrap();
        let label = cx.debug_bounds("codex-message-attachment-count-0").unwrap();
        let row = cx.debug_bounds("codex-message-images-0").unwrap();
        assert!(body.bottom() <= attachments.top());
        assert!(label.bottom() <= row.top());
        for selector in ["codex-message-image-0-0", "codex-message-image-0-1"] {
            let image = cx.debug_bounds(selector).unwrap();
            assert_eq!(image.size.width, edge);
            assert_eq!(image.size.height, edge);
            assert!(image.left() >= row.left() && image.right() <= row.right());
            assert!(image.top() >= row.top() && image.bottom() <= row.bottom());
        }
    }
}

#[gpui::test]
fn composer_pastes_text_and_image_and_submits_image_only(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.read(cx).input.focus_handle(cx).focus(window, cx);
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("pasted context".into()));
        window.dispatch_action(Box::new(gpui_component::input::Paste), cx);
    });
    draw(cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
        "pasted context"
    );
    let png = test_image(image::ImageFormat::Png);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input
                .update(cx, |input, cx| input.set_value("", window, cx));
        });
        cx.write_to_clipboard(gpui::ClipboardItem::new_image(&gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            png,
        )));
        window.dispatch_action(Box::new(gpui_component::input::Paste), cx);
    });
    draw(cx);
    assert_eq!(view.read_with(cx, |view, _| view.images.len()), 1);
    assert!(cx.debug_bounds("codex-attachments").is_some());
    cx.simulate_keystrokes("enter");
    draw(cx);
    let sent: Vec<_> = actions
        .try_iter()
        .filter_map(|action| {
            if let Action::Submit { text, images, .. } = action {
                Some((text, images))
            } else {
                None
            }
        })
        .collect();
    assert!(
        sent.iter().any(|(text, images)| text.trim() == "Image 1"
            && images.len() == 1
            && images[0].starts_with("data:image/png;base64,")),
        "sent={sent:?}; error={:?}; input={:?}; focused={}",
        view.read_with(cx, |view, cx| view.session.read(cx).state.error.clone()),
        view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
        cx.update(|window, cx| view.read(cx).input.focus_handle(cx).is_focused(window))
    );
}

#[gpui::test]
fn composer_markdown_newline_keeps_source_undo_and_send_for_both_providers(
    cx: &mut gpui::TestAppContext,
) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let (view, cx, actions, _updates) = fixture(cx);
        let source = "> 9. [x] é🦀";
        let continued = "> 9. [x] é🦀\n> 10. [ ] ";
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session
                    .update(cx, |session, _| session.state.saved.provider = provider);
                view.input.update(cx, |input, cx| {
                    input.set_value(source, window, cx);
                    input.set_selected_range(source.len()..source.len(), cx);
                    input.focus(window, cx);
                });
            });
        });
        draw(cx);
        cx.simulate_keystrokes("shift-enter");
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), continued);
        });
        // The fixture exposes the provider command channel without running
        // its reducer; verify the actual draft command at that boundary.
        let emitted: Vec<_> = actions.try_iter().collect();
        assert!(emitted
            .iter()
            .any(|action| matches!(action, Action::Draft(text) if text == continued)));
        assert!(!emitted
            .iter()
            .any(|action| matches!(action, Action::Submit { .. })));
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-z"
        } else {
            "ctrl-z"
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), source);
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-shift-z"
        } else {
            "ctrl-y"
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), continued);
        });
        cx.simulate_keystrokes("enter");
        draw(cx);
        let sent: Vec<_> = actions.try_iter().collect();
        assert!(
            sent.iter()
                .any(|action| matches!(action, Action::Submit { text, .. } if text == continued)),
            "provider={provider:?}; actions={:?}; state={:?}; error={:?}",
            sent.iter()
                .map(|action| match action {
                    Action::Submit { text, .. } => format!("Submit({text:?})"),
                    Action::Draft(text) => format!("Draft({text:?})"),
                    _ => format!("{:?}", std::mem::discriminant(action)),
                })
                .collect::<Vec<_>>(),
            view.read_with(cx, |view, cx| view.session.read(cx).state.status),
            view.read_with(cx, |view, cx| view.session.read(cx).state.error.clone())
        );
    }
}

#[gpui::test]
fn composer_markdown_newline_preserves_literal_paste_and_selection(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    for (source, range, expected) in [
        ("## Heading", 10..10, "## Heading\n"),
        ("```\n  - literal", 15..15, "```\n  - literal\n  "),
        ("- é🦀 tail", 8..8, "- é🦀\n-  tail"),
        ("- é🦀 tail", 2..8, "- \n tail"),
        ("> - [ ] ", 8..8, "> "),
    ] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value(source, window, cx);
                    input.set_selected_range(range.clone(), cx);
                    input.focus(window, cx);
                });
            });
        });
        draw(cx);
        cx.simulate_keystrokes("shift-enter");
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), expected, "{source:?}");
        });
    }
    let pasted = "- first\n- second\n";
    cx.update(|window, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(pasted.into()));
        view.update(cx, |view, cx| {
            view.input
                .update(cx, |input, cx| input.set_value("", window, cx));
        });
    });
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-v"
    } else {
        "ctrl-v"
    });
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.input.read(cx).value().as_ref(), pasted);
    });
}

#[test]
fn selection_reference_projection_preserves_literal_quote_and_rejects_partial_envelopes() {
    let quote = codex::ReferenceDraft {
        id: "reference".into(),
        thread_id: Some("thread".into()),
        sources: vec![codex::ReferenceSource {
            message_id: "source".into(),
            role: "Assistant".into(),
        }],
        text: "  /clear\n```rust\n<literal>\n\nUser reply:\n```".into(),
    };
    let canonical = quote.serialize("my reply");
    let (restored, reply) = reference_prompt(&canonical).unwrap();
    assert_eq!(restored, quote);
    assert_eq!(reply, "my reply");
    assert!(reference_prompt_copy(&canonical).contains(&quote.text));
    assert!(reference_prompt_markdown(&canonical).contains(&quote.text));
    assert!(!reference_prompt_copy(&canonical).contains("message_id"));
    let partial = "Referenced transcript selection (quoted data):\n{}\n\nUser reply:\nreply";
    assert_eq!(reference_prompt_copy(partial), partial);
}

#[gpui::test]
fn selection_reference_queue_editor_saves_its_quote_and_restores_main_reference(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    let quote = |id: &str| codex::ReferenceDraft {
        id: id.into(),
        thread_id: Some("thread-ui".into()),
        sources: vec![codex::ReferenceSource {
            message_id: "source".into(),
            role: "Assistant".into(),
        }],
        text: format!("literal {id} quote"),
    };
    let main = quote("main");
    let queued = quote("queued");
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input
                .update(cx, |input, cx| input.set_value("main draft", window, cx));
            view.set_reference(Some(main.clone()), cx);
            view.session.update(cx, |session, _| {
                session.state.queued_prompts.push(codex::QueuedPrompt {
                    id: 81,
                    text: queued.serialize("queued reply"),
                    images: Vec::new(),
                });
            });
            view.begin_queue_edit(81, cx);
            view.session
                .update(cx, |session, _| session.state.queue_edit = Some(81));
        });
    });
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.input.read(cx).value().as_ref(), "queued reply");
        assert_eq!(
            view.queue_editor.as_ref().unwrap().reference.as_ref(),
            Some(&queued)
        );
        assert_eq!(view.reference.as_ref(), Some(&main));
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input
                .update(cx, |input, cx| input.set_value("edited reply", window, cx));
            view.save_queue_edit(cx);
        });
    });
    assert!(actions.try_iter().any(|action| matches!(action, Action::SaveQueuedEdit { id:81, text, .. } if text == queued.serialize("edited reply"))));
    cx.update(|window, cx| view.update(cx, |view, cx| view.restore_queue_draft(window, cx)));
    view.read_with(cx, |view, cx| {
        assert_eq!(view.input.read(cx).value().as_ref(), "main draft");
        assert_eq!(view.reference.as_ref(), Some(&main));
    });
}

#[gpui::test]
fn selection_reference_requires_current_native_message_owners_and_preserves_order(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, updates) = fixture(cx);
    let mut response = snapshot();
    response.saved.items = vec![
            serde_json::json!({"type":"userMessage", "id":"user-source", "content":[{"type":"text","text":"user passage"}]}),
            serde_json::json!({"type":"agentMessage", "id":"assistant-source", "text":"assistant passage"}),
        ].into();
    updates.send(response).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            let state = view.session.read(cx).state.shared();
            let owner = |id: &str| view.selection_views[&format!("thread-ui:{id}")].entity_id();
            let mut selection = TextSelectionSnapshot {
                text: "user passage\nassistant passage".into(),
                spans: vec![
                    gpui_component::text::TextSelectionSpan {
                        owner: owner("user-source"),
                        text: "user passage".into(),
                    },
                    gpui_component::text::TextSelectionSpan {
                        owner: owner("assistant-source"),
                        text: "assistant passage".into(),
                    },
                ],
                anchor: gpui::point(gpui::px(0.), gpui::px(0.)),
                endpoint: gpui::point(gpui::px(1.), gpui::px(1.)),
                is_selecting: false,
            };
            let quote = view.selection_reference(&selection, &state).unwrap();
            assert_eq!(
                quote
                    .sources
                    .iter()
                    .map(|s| s.message_id.as_str())
                    .collect::<Vec<_>>(),
                ["user-source", "assistant-source"]
            );
            selection.spans[1].owner = view.input.entity_id();
            assert!(
                view.selection_reference(&selection, &state).is_none(),
                "composer/tool/HTML owners cannot qualify"
            );
            selection.spans.pop();
            let mut switched = (*state).clone();
            switched.saved.thread_id = Some("another-thread".into());
            assert!(
                view.selection_reference(&selection, &switched).is_none(),
                "old thread owners cannot qualify"
            );
            let mut removed = (*state).clone();
            removed.saved.items.clear();
            assert!(
                view.selection_reference(&selection, &removed).is_none(),
                "removed sources cannot qualify"
            );
        });
    });
}

#[gpui::test]
fn selection_reference_replacement_remove_and_late_accept_preserve_newer_draft(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, updates) = fixture_with_navigation(cx, true);
    let quote = |id: &str| codex::ReferenceDraft {
        id: id.into(),
        thread_id: Some("thread-ui".into()),
        sources: vec![codex::ReferenceSource {
            message_id: "source".into(),
            role: "Assistant".into(),
        }],
        text: "/clear\ncode <literal>".into(),
    };
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input
                .update(cx, |input, cx| input.set_value("preserved", window, cx));
            view.set_reference(Some(quote("old")), cx);
            view.submitted_reference = Some("old".into());
            view.submitted_ack = Some(7);
            view.submitted = Some("preserved".into());
            view.images = vec!["same-image".into()];
            view.submitted_images.clone_from(&view.images);
            view.set_reference(Some(quote("new")), cx);
            view.input.focus_handle(cx).focus(window, cx);
        });
    });
    let mut accepted = snapshot();
    accepted.accepted_submission = Some(7);
    updates.send(accepted).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert_eq!(view.reference.as_ref().unwrap().id, "new");
        assert_eq!(view.input.read(cx).value().as_ref(), "preserved");
        assert_eq!(view.images, vec!["same-image"]);
    });
    assert!(cx.debug_bounds("codex-remove-reference").is_some());
    let mut focused = false;
    for _ in 0..20 {
        cx.simulate_keystrokes("tab");
        draw(cx);
        focused = cx.update(|window, cx| view.read(cx).reference_remove_focus.is_focused(window));
        if focused {
            break;
        }
    }
    assert!(focused, "Remove reference must be reachable by Tab");
    cx.simulate_keystrokes("enter");
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert!(view.reference.is_none());
        assert_eq!(view.input.read(cx).value().as_ref(), "preserved");
        assert_eq!(view.images, vec!["same-image"]);
    });
    assert!(cx.update(|window, cx| view.read(cx).input.focus_handle(cx).is_focused(window)));
}

#[gpui::test]
fn screenshot_tiff_paste_is_prepared_before_prompt_and_normalized_to_png(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    let tiff = test_image(image::ImageFormat::Tiff);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_value("inspect screenshot", window, cx);
            });
            view.paste_images(
                vec![gpui::Image::from_bytes(gpui::ImageFormat::Tiff, tiff)],
                window,
                cx,
            );
            assert_eq!(view.pending_image_pastes, 1);
            view.submit(window, cx);
        });
    });
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::Submit { .. })));
    draw(cx);
    assert_eq!(view.read_with(cx, |view, _| view.pending_image_pastes), 0);
    assert!(view.read_with(cx, |view, _| {
        view.images[0].starts_with("data:image/png;base64,")
    }));
    cx.update(|window, cx| view.update(cx, |view, cx| view.submit(window, cx)));
    assert!(actions.try_iter().any(|action| matches!(action, Action::Submit {text,images,..} if text.trim()=="Image 1 inspect screenshot" && images.len()==1 && images[0].starts_with("data:image/png;base64,"))));
}
