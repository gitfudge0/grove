use super::*;

#[test]
fn claude_permissions_use_claude_modes_without_codex_sandbox_claims() {
    let mut state = snapshot();
    state.saved.provider = codex::Provider::Claude;
    state.permission_modes = [
        "default",
        "acceptEdits",
        "plan",
        "dontAsk",
        "bypassPermissions",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let (choices, _) = CodexView::menu_choices(&state, ComposerMenu::Access);
    assert!(choices.iter().any(|(value, _)| value == "default"));
    assert!(!choices
        .iter()
        .any(|(value, _)| value == "bypassPermissions"));
    assert!(!choices
        .iter()
        .any(|(value, label)| value == "workspace-write" || label.contains("sandbox")));
    for flag in [
        "--dangerously-skip-permissions",
        "--allow-dangerously-skip-permissions",
    ] {
        state.saved.config_args = vec![flag.into()];
        let (choices, _) = CodexView::menu_choices(&state, ComposerMenu::Access);
        assert!(choices
            .iter()
            .any(|(value, _)| value == "bypassPermissions"));
    }
}

#[gpui::test]
fn explicit_full_access_remains_reversible_for_codex_and_claude(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        cx.set_global(crate::settings::SettingsState::new(
            grove_core::storage::Store::default(),
        ));
    });
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        for restored in [false, true] {
            let (view, cx, actions, _) = fixture(cx);
            cx.update(|_, cx| {
                view.update(cx, |view, cx| {
                    view.session.update(cx, |session, cx| {
                        let access = if provider == codex::Provider::Codex {
                            "full-access"
                        } else {
                            "bypassPermissions"
                        };
                        session.state.saved.provider = provider;
                        session.state.saved.composer.access = Some(access.into());
                        if restored {
                            let saved = serde_json::from_value(
                                serde_json::to_value(&session.state.saved).unwrap(),
                            )
                            .unwrap();
                            session.state = Snapshot::new(saved).into();
                        }
                        // Restored sessions publish their connected state after startup.
                        session.state.status = Status::Idle;
                        session.state.access = Some(access.into());
                        session.state.resolved_access = Some(access.into());
                        session.state.permission_modes =
                            ["default", "acceptEdits", "bypassPermissions"]
                                .map(str::to_owned)
                                .to_vec();
                        cx.notify();
                    });
                });
            });
            for enabled in [false, true] {
                cx.update(|_, cx| {
                    cx.global_mut::<crate::settings::SettingsState>()
                        .store
                        .yolo_enabled = enabled;
                });
                draw(cx);
                assert!(
                    cx.debug_bounds("codex-yolo-access").is_none(),
                    "new-session Settings preferences must not relabel an existing session"
                );
            }
            draw(cx);
            assert!(
                cx.debug_bounds("codex-yolo-access").is_none(),
                "an explicit access choice must not turn into a Yolo launch"
            );
            let trigger = view.read_with(cx, |view, _| view.option_bounds[2].get());
            cx.simulate_click(trigger.center(), gpui::Modifiers::default());
            draw(cx);
            let popup = cx.debug_bounds("codex-options-popup").unwrap();
            // Both providers have three choices plus configured defaults.
            let first_row = gpui::point(popup.center().x, popup.top() + popup.size.height * 0.125);
            cx.simulate_click(first_row, gpui::Modifiers::default());
            draw(cx);
            let expected = if provider == codex::Provider::Codex {
                "read-only"
            } else {
                "default"
            };
            assert!(actions.try_iter().any(|action| matches!(action, Action::Configure { access: Some(access), .. } if access == expected)));
            assert!(view.read_with(cx, |view, _| view.popup.is_none()));
        }
    }
}

#[test]
fn description_conversations_present_commands_and_prose_without_transport_payloads() {
    use grove_core::change_description::Scope;
    let scope = Scope::PullRequest {
        url: "https://github.com/gitfudge0/grove/pull/17".into(),
    };
    let prompt="Describe ONLY the supplied immutable change snapshot. schema and captured diff\n\nGrove request identity: unique-attempt";
    assert_eq!(
        description_conversation_text(Some(&scope), "userMessage", prompt),
        "/describe-changes https://github.com/gitfudge0/grove/pull/17"
    );
    assert_eq!(
        description_conversation_text(None, "userMessage", prompt),
        prompt
    );
    let answer="Prepared the change summary.\n```grove-description\n{\"private-payload\":true}\n```\nMore explanation.";
    assert_eq!(
        description_conversation_text(Some(&scope), "agentMessage", answer),
        "Prepared the change summary.\nMore explanation."
    );
    assert_eq!(
        description_conversation_text(None, "agentMessage", answer),
        answer
    );
    assert_eq!(
        description_conversation_text(
            Some(&scope),
            "agentMessage",
            "Prepared.\n```grove-description\npartial"
        ),
        "Prepared."
    );
    let quoted = "```markdown\n```grove-description\nquoted source\n```";
    assert_eq!(
        description_conversation_text(Some(&scope), "agentMessage", quoted),
        quoted
    );
}

#[gpui::test]
fn pending_bubbles_use_submission_receipts_without_stealing_repeated_inputs(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| view.update(cx, |view, cx| {
            for provider in [codex::Provider::Codex, codex::Provider::Claude] {
                view.bubble_entrances.clear();
                let mut state = snapshot();
                state.saved.provider = provider;
                view.record_submission_bubble(41, "same".into(), vec![], false, false, cx);
                view.record_submission_bubble(42, "same".into(), vec![], false, false, cx);
                state.saved.items = vec![
                    json!({"id":"second","type":"userMessage","content":[{"text":"same"}]}),
                    json!({"id":"answer","type":"agentMessage","text":"Response"}),
                ].into();
                state.submission_items.insert(42, "second".into());
                state.accept_submission(42);
                view.sync_bubble_entrances(&state, true, cx.background_executor().now());
                assert_eq!(view.bubble_entrances.len(), 1, "unacknowledged identical input must not steal a receipt");
                assert_eq!(view.bubble_entrances[0].submission, 41);
                state.saved.items.insert(0, json!({"id":"first","type":"userMessage","content":[{"text":"provider normalized this input"}]}).into());
                state.submission_items.insert(41, "first".into());
                state.accept_submission(41);
                view.sync_bubble_entrances(&state, true, cx.background_executor().now());
                assert!(view.bubble_entrances.is_empty(), "receipt replaces temporary bubble despite text normalization");
            }
        }));
}

#[gpui::test]
#[cfg(unix)]
fn completed_shell_footer_reflows_without_markdown_change_for_both_providers(
    cx: &mut gpui::TestAppContext,
) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let (view, cx, _, _) = fixture(cx);
        cx.update(|_, cx| { cx.set_reduce_motion(true); view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    let mut state = snapshot(); state.saved.provider = provider; state.status = Status::Working;
                    state.saved.items = vec![json!({"id":"finishing-shell","type":"agentMessage","text":"```sh\nprintf x\n```","phase":"final_answer","status":"inProgress"})].into();
                    session.state = state.into(); cx.notify();
                });
            }); });
        for _ in 0..4 {
            draw(cx);
        }
        let before = cx.debug_bounds("codex-message-body-0").unwrap().size.height;
        let owner = view.read_with(cx, |view, _| {
            view.selection_views.values().next().unwrap().clone()
        });
        assert!(cx.debug_bounds("codex-run-code").is_none());
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.items[0]["status"] = json!("completed");
                    session.state.saved.items[0]["groveCompleted"] = json!(true);
                    session.state.status = Status::Done;
                    cx.notify();
                });
            });
        });
        for _ in 0..4 {
            draw(cx);
        }
        let after = cx.debug_bounds("codex-message-body-0").unwrap().size.height;
        let current = view.read_with(cx, |view, _| {
            view.selection_views.values().next().unwrap().clone()
        });
        assert_eq!(
            owner.entity_id(),
            current.entity_id(),
            "same Markdown owner must be retained"
        );
        assert!(
            cx.debug_bounds("codex-run-code").is_some(),
            "completed closed shell must expose Run"
        );
        assert!(after > before + gpui::px(10.), "metadata-only {provider:?} completion must make room for its footer: {before:?} -> {after:?}");
    }
}

#[gpui::test]
fn tab_accepts_file_at_caret_and_preserves_surrounding_text(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.file_query = "src".into();
            view.file_lookup = Some("src".into());
            view.file_suggestions = vec!["src/main.rs".into()];
            view.input.update(cx, |input, cx| {
                input.set_value("Read @src later", window, cx);
                input.set_selected_range(9..9, cx);
            });
            view.input.focus_handle(cx).focus(window, cx);
        });
    });
    draw(cx);
    cx.simulate_keystrokes("tab");
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert_eq!(
            view.input.read(cx).value().as_ref(),
            "Read @src/main.rs later"
        );
        assert_eq!(view.input.read(cx).cursor(), 18);
        assert_eq!(view.picked_tokens.len(), 1);
        assert_eq!(view.picked_tokens[0].decoration.get_ranges(cx), vec![5..17]);
    });
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::Submit { .. })));
}

#[gpui::test]
fn sent_bubble_entrance_is_local_once_and_respects_reduced_motion(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            let mut state = snapshot();
            state.saved.items =
                vec![json!({"id":"history","type":"userMessage","content":[{"text":"hello"}]})]
                    .into();
            view.sync_bubble_entrances(&state, false, cx.background_executor().now());
            assert!(view.animating_bubbles.is_empty());
            view.bubble_entrances.push(BubbleEntrance {
                submission: 1,
                existing: HashSet::from(["history".into()]),
                thread: state.saved.thread_id.clone(),
                text: "hello".into(),
                optimistic: None,
            });
            view.sync_bubble_entrances(&state, false, cx.background_executor().now());
            assert!(view.animating_bubbles.is_empty());
            state
                .saved
                .items
                .push(json!({"id":"fresh","type":"userMessage","content":[{"text":"hello"}]}));
            view.sync_bubble_entrances(&state, false, cx.background_executor().now());
            assert!(view.bubble_entrances.is_empty());
            let clock = view.animating_bubbles["fresh"].1.clone();
            view.sync_bubble_entrances(&state, false, cx.background_executor().now());
            assert!(Rc::ptr_eq(&clock, &view.animating_bubbles["fresh"].1));
            clock.set(true);
            view.sync_bubble_entrances(&state, false, cx.background_executor().now());
            assert!(view.animating_bubbles.is_empty());
            view.sync_bubble_entrances(&state, false, cx.background_executor().now());
            assert!(view.animating_bubbles.is_empty());
            view.bubble_entrances.push(BubbleEntrance {
                submission: 2,
                existing: HashSet::new(),
                thread: state.saved.thread_id.clone(),
                text: "hello".into(),
                optimistic: None,
            });
            view.sync_bubble_entrances(&state, true, cx.background_executor().now());
            assert!(view.bubble_entrances.is_empty());
            assert!(view.animating_bubbles.is_empty());
            view.sync_bubble_entrances(&state, false, cx.background_executor().now());
            assert!(view.animating_bubbles.is_empty());
        });
    });
}

#[gpui::test]
fn user_messages_are_narrower_and_right_aligned(cx: &mut gpui::TestAppContext) {
    let (_view, cx, _, updates) = fixture(cx);
    let mut state = snapshot();
    state.saved.items = vec![
            json!({"id":"user","type":"userMessage","content":[{"type":"text","text":"Please inspect this worktree."}]}),
            json!({"id":"agent","type":"agentMessage","text":"I can inspect this worktree."}),
        ].into();
    updates.send(state).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    let user = cx.debug_bounds("codex-message-row-0").unwrap();
    let agent = cx.debug_bounds("codex-message-row-1").unwrap();
    assert!(user.size.width < agent.size.width);
    assert!(user.origin.x > agent.origin.x);
    assert_eq!(user.right(), agent.right());
}

#[test]
fn context_meter_turns_amber_at_the_threshold_and_details_tokens() {
    let usage = |used| codex::ContextUsage {
        used,
        window: 200_000,
    };
    assert_eq!(context_tone(usage(76_000)), (38, c::FG_MUTE()));
    assert_eq!(context_tone(usage(158_000)).1, c::FG_MUTE());
    assert_eq!(context_tone(usage(159_000)), (80, c::AMBER()));
    assert_eq!(context_tone(usage(250_000)), (100, c::AMBER()));
    assert_eq!(context_detail(usage(76_000)), "76k / 200k tokens");
}

#[gpui::test]
fn next_reply_tab_accepts_whole_reply_for_codex_and_claude(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    let proposal = "Check keyboard navigation.";
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                seed_next_reply(view, provider, "", proposal, window, cx);
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("next-reply-hint").is_some());
        cx.simulate_input("Something else");
        draw(cx);
        assert!(cx.debug_bounds("next-reply-hint").is_none());
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-a backspace"
        } else {
            "ctrl-a backspace"
        });
        draw(cx);
        assert!(cx.debug_bounds("next-reply-hint").is_some());
        cx.simulate_keystrokes("tab");
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), proposal);
            assert_eq!(
                view.input.read(cx).selected_range(),
                proposal.len()..proposal.len()
            );
        });
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Submit { .. } | Action::Prompt(_))));
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-z"
        } else {
            "ctrl-z"
        });
        draw(cx);
        assert_eq!(
            view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
            ""
        );
        assert!(
            cx.debug_bounds("next-reply-hint").is_some(),
            "Undoing acceptance restores the candidate until Send"
        );
    }
}

#[gpui::test]
fn shimmer_moves_bright_glyph_band_and_keeps_unicode_boundaries(cx: &mut gpui::TestAppContext) {
    let _restore_theme = c::ActiveThemeGuard::capture();
    let (_, cx, _, _) = fixture(cx);
    for theme in ["grove-dark", "grove-light"] {
        grove_core::theme::set_by_name(theme);
        cx.update(|_, cx| c::sync_component_theme(cx));
        cx.update(|_, _| {
            let label = "5 tool calls · Running…";
            let early = crate::views::components::shimmer_highlights(label, 0.25);
            let later = crate::views::components::shimmer_highlights(label, 0.75);
            assert_eq!(early.len(), label.chars().count());
            assert!(early
                .iter()
                .all(|(range, _)| label.is_char_boundary(range.start)
                    && label.is_char_boundary(range.end)));
            // Emphasis moves toward FG; on light themes that is darker.
            let direction = (c::FG().l - c::FG_DIM().l).signum();
            let brightest = |runs: &[(std::ops::Range<usize>, gpui::HighlightStyle)]| {
                runs.iter()
                    .enumerate()
                    .max_by(|(_, first), (_, second)| {
                        (first.1.color.unwrap().l * direction)
                            .total_cmp(&(second.1.color.unwrap().l * direction))
                    })
                    .unwrap()
                    .0
            };
            assert!(brightest(&early) < brightest(&later));
            assert_ne!(
                early[0].1.color,
                early[early.len() / 2].1.color,
                "shimmer varies glyph colors rather than pulsing the whole line"
            );
        });
    }
}

#[gpui::test]
fn stream_reveal_preserves_history_and_reveals_only_appended_text(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, _) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.last_content = None; // Model mounting an existing saved transcript.
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Working;
                session.state.saved.items =
                    vec![json!({"id":"history","type":"agentMessage","text":"Saved history"})]
                        .into();
                cx.notify();
            });
        });
    });
    draw(cx);
    view.read_with(cx, |view, _| {
        assert_eq!(view.stream_text["history"].1, "Saved history".len());
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.status = Status::Working;
                session.state.saved.items[0]["text"] =
                    json!("Saved history with a Unicode continuation 界界界界界界界界界界界界");
                cx.notify();
            });
            let _ = view.render(window, cx);
            let (text, shown) = &view.stream_text["history"];
            assert!(*shown >= "Saved history".len());
            assert!(
                *shown < text.len(),
                "status={:?}, reduced={}, shown={shown}",
                view.session.read(cx).state.status,
                cx.reduce_motion()
            );
            assert!(text.is_char_boundary(*shown));
        });
    });
    draw(cx);
    for _ in 0..80 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(16));
        draw(cx);
    }
    view.read_with(cx, |view, _| {
        let (text, shown) = &view.stream_text["history"];
        assert_eq!(*shown, text.len());
    });
}

#[gpui::test]
fn codex_and_claude_all_local_file_types_open_viewer_including_external_outputs(
    cx: &mut gpui::TestAppContext,
) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let checkout = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs_err::write(
            checkout.path().join("launch.html"),
            "<!doctype html>\n<title>Launch</title>\n",
        )
        .unwrap();
        fs_err::write(
            checkout.path().join("review.grove-plan.json"),
            "{\n  \"schema_version\": 1\n}\n",
        )
        .unwrap();
        let external = outside.path().join("output notes.md");
        fs_err::write(
            &external,
            "# Outside output\n\nSelected explicitly from the transcript.\n",
        )
        .unwrap();
        let (view, cx, _, updates) = fixture(cx);
        for (label, target, line, expected_text) in [
            (
                "Launch checklist",
                "launch.html:2".to_owned(),
                2,
                "<title>Launch</title>",
            ),
            (
                "Launch preview",
                "launch.html#L2".to_owned(),
                2,
                "<title>Launch</title>",
            ),
            (
                "Review plan",
                "review.grove-plan.json:2:3".to_owned(),
                2,
                "schema_version",
            ),
            (
                "Output notes",
                format!(
                    "{}#L3",
                    gpui::http_client::Url::from_file_path(&external).unwrap()
                ),
                3,
                "Selected explicitly",
            ),
        ] {
            let mut response = snapshot();
            response.saved.provider = provider;
            response.saved.cwd = checkout.path().display().to_string();
            response.saved.items.push(serde_json::json!({"type":"agentMessage","id":"file-output","text":format!("[{label}](<{target}>)")}));
            updates.send(response).unwrap();
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(40));
            draw(cx);
            let link = cx
                .debug_bounds("transcript-file-link")
                .expect("all local file extensions use the viewer link");
            assert_eq!(
                cx.debug_bounds("transcript-browser-link").is_some(),
                target.starts_with("launch.html"),
                "{provider:?}: local HTML retains an independent browser action",
            );
            cx.simulate_click(link.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            draw(cx);
            assert!(
                cx.debug_bounds("file-viewer").is_some(),
                "{provider:?}: {target}"
            );
            view.read_with(cx, |view, cx| {
                let browser = view.file_browser.read(cx);
                assert_eq!(browser.target.as_ref().unwrap().line, Some(line));
                assert!(browser.loaded_text().unwrap().contains(expected_text));
            });
            assert!(
                cx.opened_url().is_none(),
                "file link must not open externally"
            );
            cx.simulate_keystrokes("escape");
            draw(cx);
            assert!(cx.debug_bounds("file-viewer").is_none());
        }
    }
}

#[gpui::test]
fn native_markdown_drag_selects_and_copies_live_response(cx: &mut gpui::TestAppContext) {
    let (_view, cx, _actions, updates) = fixture(cx);
    let mut response = snapshot();
    response.saved.items.push(serde_json::json!({"type":"agentMessage", "id":"answer", "text":"quick selection value\n\n## Details\n\n- formatted list\n\n```rust\n    let selected = true;\n```"}));
    updates.send(response.clone()).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    let body = cx
        .debug_bounds("codex-message-body-0")
        .expect("rendered Markdown body");
    let start = body.origin + gpui::point(gpui::px(1.), gpui::px(8.));
    let end = body.origin + gpui::point(gpui::px(110.), gpui::px(8.));
    cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(
        end,
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    cx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::default());
    draw(cx);
    let selected = cx.update(gpui_component::WindowExt::selected_text);
    assert!(
        !selected.trim().is_empty(),
        "drag must highlight actual output text"
    );
    #[cfg(target_os = "macos")]
    cx.simulate_keystrokes("cmd-c");
    #[cfg(not(target_os = "macos"))]
    cx.simulate_keystrokes("ctrl-c");
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text())),
        Some(selected.clone())
    );
    response.saved.items[0]["text"] = serde_json::json!(
            "quick selection value\n\n## Details\n\n- formatted list\n\n```rust\n    let selected = true;\n```\n\nMore live text."
        );
    updates.send(response).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(
        cx.debug_bounds("codex-message-body-0").unwrap().size.height > body.size.height,
        "streamed Markdown must actually parse and grow before checking selection"
    );
    assert_eq!(
        cx.update(gpui_component::WindowExt::selected_text),
        selected,
        "streaming must preserve existing selection"
    );
}

#[gpui::test]
fn transcript_selection_copies_rich_text_and_markdown_for_both_providers(
    cx: &mut gpui::TestAppContext,
) {
    thread_local! {
        static RICH: std::cell::RefCell<Option<(String, String)>> = const { std::cell::RefCell::new(None) };
    }
    const SOURCE: &str = "> - [a link](https://example.com) and `make`\n\nOutside the selection";
    const MARKDOWN: &str = "> - [a link](https://example.com) and `make`";

    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let (_view, cx, _actions, updates) = fixture(cx);
        cx.update(|_, cx| {
            gpui_component::text::set_rich_clipboard_writer(cx, |html, text| {
                RICH.with_borrow_mut(|captured| *captured = Some((html.into(), text.into())));
                true
            });
        });
        let mut response = snapshot();
        response.saved.provider = provider;
        response.saved.items.push(serde_json::json!({
            "type": "agentMessage", "id": "formatted-answer", "text": SOURCE, "groveCompleted": true
        }));
        updates.send(response).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        let body = cx.debug_bounds("codex-message-body-0").unwrap();
        let top_line = body.top() + gpui::px(8.);
        let start = gpui::point(body.left() + gpui::px(1.), top_line);
        let end = gpui::point(body.right() - gpui::px(1.), top_line);
        cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            gpui::point(body.left() + gpui::px(55.), top_line),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_move(
            end,
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::default());
        draw(cx);

        let selected = cx.update(gpui_component::WindowExt::selected_text);
        assert_eq!(selected.trim(), "a link and make", "{provider:?}");
        #[cfg(target_os = "macos")]
        cx.simulate_keystrokes("cmd-c");
        #[cfg(not(target_os = "macos"))]
        cx.simulate_keystrokes("ctrl-c");
        let (html, plain) = RICH.with_borrow_mut(Option::take).unwrap();
        assert_eq!(plain, selected);
        assert!(
            html.contains("<blockquote>") && html.contains("<ul>"),
            "{html}"
        );
        assert!(
            html.contains("<a href=\"https://example.com\">a link</a>"),
            "{html}"
        );
        assert!(html.contains("<code>make</code>"), "{html}");
        assert!(!html.contains("Outside the selection"), "{html}");

        #[cfg(target_os = "macos")]
        cx.simulate_keystrokes("cmd-shift-c");
        #[cfg(not(target_os = "macos"))]
        cx.simulate_keystrokes("ctrl-shift-c");
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text()))
                .as_deref(),
            Some(MARKDOWN)
        );

        // Overwrite the test clipboard so the mouse path proves that it copied.
        cx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("old".into())));
        let copy = cx.debug_bounds("codex-selection-copy-markdown").unwrap();
        cx.simulate_click(copy.center(), gpui::Modifiers::default());
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text()))
                .as_deref(),
            Some(MARKDOWN)
        );
        assert_eq!(
            cx.update(gpui_component::WindowExt::selected_text),
            selected
        );
    }
}

#[gpui::test]
fn response_footer_copies_collapsed_prose_and_survives_streaming(cx: &mut gpui::TestAppContext) {
    let (view, cx, _, updates) = fixture(cx);
    let mut state = snapshot();
    state.saved.items = vec![
            serde_json::json!({"type":"agentMessage", "id":"first", "phase":"commentary", "text":"Hidden commentary"}),
            serde_json::json!({"type":"agentMessage", "id":"answer", "phase":"final_answer", "text":"Visible answer"}),
            serde_json::json!({"type":"commandExecution", "id":"tail", "status":"completed", "command":"true"}),
        ].into();
    updates.send(state.clone()).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(cx.debug_bounds("codex-message-row-0").is_none());
    let footer = cx.debug_bounds("codex-response-footer").unwrap();
    let body = cx.debug_bounds("codex-message-row-1").unwrap();
    assert!(footer.origin.y >= body.bottom());
    let copy = cx.debug_bounds("codex-copy-response").unwrap().center();
    cx.simulate_click(copy, gpui::Modifiers::default());
    let key = view.read_with(cx, |view, _| {
        view.copied_message.as_ref().unwrap().0.clone()
    });
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text())),
        Some("Hidden commentary\n\nVisible answer".into())
    );
    state.saved.items.push(serde_json::json!({"type":"agentMessage", "id":"resumed", "phase":"final_answer", "text":"Resumed answer"}));
    updates.send(state).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert_eq!(
        view.read_with(cx, |view, _| view
            .copied_message
            .as_ref()
            .unwrap()
            .0
            .clone()),
        key
    );
    let copy = cx.debug_bounds("codex-copy-response").unwrap().center();
    cx.simulate_click(copy, gpui::Modifiers::default());
    assert_eq!(
        cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text())),
        Some("Hidden commentary\n\nVisible answer\n\nResumed answer".into())
    );
}
