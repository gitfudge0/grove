use super::*;

#[gpui::test]
fn rewind_confirmation_blocks_background_and_preserves_composer(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(800.)));
    cx.update(|window, cx| view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| input.set_value("Unrelated composer draft", window, cx));
            view.session.update(cx, |session, cx| {
                session.state.edit_supported = true;
                session.state.saved.draft = "Unrelated composer draft".into();
                session.state.saved.items = vec![
                    json!({"id":"original-user", "type":"userMessage", "content":[{"type":"text", "text":"Original request"}]}),
                    json!({"id":"answer", "type":"agentMessage", "text":"Original answer"}),
                ].into();
                session.state.saved.edit_checkpoints.push(codex::EditCheckpoint {
                    start_item: 0, turn_id: Some("original-turn".into()), files: None,
                });
                cx.notify();
            });
        }));
    draw(cx);
    let edit = cx.debug_bounds("codex-edit-message-0").unwrap();
    cx.simulate_click(edit.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(cx.debug_bounds("codex-inline-editor").is_none());
    let tray = cx.debug_bounds("codex-edit-confirmation").unwrap();
    assert!((tray.right() - edit.right()).abs() < gpui::px(1.));
    cx.simulate_resize(gpui::size(gpui::px(420.), gpui::px(700.)));
    draw(cx);
    let moved_source = cx.debug_bounds("codex-edit-message-0").unwrap();
    let moved_tray = cx.debug_bounds("codex-edit-confirmation").unwrap();
    let overlay = cx.debug_bounds("codex-edit-confirmation-overlay").unwrap();
    assert!((moved_tray.right() - moved_source.right()).abs() < gpui::px(1.));
    assert!(moved_tray.top() >= overlay.top() && moved_tray.bottom() <= overlay.bottom());
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.items.push(
                    json!({"id":"later-answer", "type":"agentMessage", "text":"Incoming update"}),
                );
                cx.notify();
            });
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-edit-confirmation").is_some());
    let composer = cx.debug_bounds("codex-composer").unwrap();
    cx.simulate_click(composer.center(), gpui::Modifiers::default());
    cx.simulate_keystrokes("tab");
    cx.update(|window, cx| {
        view.read_with(cx, |view, cx| {
            assert!(view.edit_keep_focus.is_focused(window));
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                "Unrelated composer draft"
            );
        });
    });
    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(cx.debug_bounds("codex-edit-confirmation").is_none());
    assert!(cx.debug_bounds("codex-message-body-0").is_some());
    assert!(!actions.try_iter().any(|action| matches!(
        action,
        Action::RewindMessage { .. } | Action::PromptInline { .. }
    )));
    let source = cx.debug_bounds("codex-edit-message-0").unwrap();
    cx.simulate_click(source.center(), gpui::Modifiers::default());
    draw(cx);
    let keep = cx.debug_bounds("codex-keep-files").unwrap();
    cx.simulate_click(keep.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(actions.try_iter().any(|action| matches!(
        action,
        Action::RewindMessage {
            index: 0,
            restore_files: false,
            ..
        }
    )));
}

#[gpui::test]
fn successful_rewind_loads_composer_without_automatic_send(cx: &mut gpui::TestAppContext) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let (view, cx, actions, _) = fixture(cx);
        draw(cx);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    if provider == codex::Provider::Claude {
                        session.state.saved.thread_id = Some("claude-fork".into());
                    }
                    session.state.saved.draft = "Restored request".into();
                    session.state.edited_messages += 1;
                    cx.notify();
                });
            });
        });
        draw(cx);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.input.read(cx).value().as_ref(), "Restored request");
        });
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::PromptInline { .. } | Action::Prompt(_))));
    }
}

#[gpui::test]
fn saved_rewind_draft_handoff_blocks_stale_edits_until_acknowledged(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_value("Restored prompt", window, cx);
            });
            view.session.update(cx, |session, cx| {
                session
                    .state
                    .saved
                    .rewind_drafts
                    .push(codex::SavedComposerDraft {
                        id: "saved".into(),
                        text: "Existing draft".into(),
                        images: vec![],
                        reference: None,
                    });
                cx.notify();
            });
        });
    });
    draw(cx);
    for picker in [false, true] {
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.pending_image_pastes = usize::from(!picker);
                view.pending_attachment_picker = picker;
                cx.notify();
            });
        });
        draw(cx);
        let restore = cx.debug_bounds("codex-restore-rewind-draft").unwrap();
        cx.simulate_click(restore.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::RestoreRewindDraft { .. })));
    }
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.pending_image_pastes = 0;
            view.pending_attachment_picker = false;
            cx.notify();
        });
    });
    draw(cx);
    let restore = cx.debug_bounds("codex-restore-rewind-draft").unwrap();
    cx.simulate_click(restore.center(), gpui::Modifiers::default());
    draw(cx);
    assert!(actions
        .try_iter()
        .any(|action| matches!(action, Action::RestoreRewindDraft { id } if id == "saved")));
    let input = cx.debug_bounds("codex-composer-input").unwrap();
    cx.simulate_click(input.center(), gpui::Modifiers::default());
    cx.simulate_keystrokes("x");
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert!(view.edit_request_pending);
        assert_eq!(view.input.read(cx).value().as_ref(), "Restored prompt");
    });
    cx.update(|_, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, cx| {
                session.state.saved.draft = "Existing draft".into();
                session.state.rewind_attempts += 1;
                session.state.edited_messages += 1;
                cx.notify();
            });
        });
    });
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert!(!view.edit_request_pending);
        assert_eq!(view.input.read(cx).value().as_ref(), "Existing draft");
    });
}

#[gpui::test]
fn clear_waits_for_actor_ack_and_preserves_rejected_draft(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    for (status, compacting, expected) in [
        (Status::Starting, false, codex::CLEAR_BUSY),
        (Status::Working, false, codex::CLEAR_BUSY),
        (Status::Idle, true, codex::CLEAR_BUSY),
        (Status::Disconnected, false, codex::CLEAR_DISCONNECTED),
    ] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session.update(cx, |session, _| {
                    session.state.status = status;
                    session.state.compacting = compacting;
                });
                view.images = vec!["attached.png".into()];
                view.input
                    .update(cx, |input, cx| input.set_value("/clear", window, cx));
                view.submit(window, cx);
                assert_eq!(view.input.read(cx).value().as_ref(), "/clear");
                assert_eq!(view.images, vec!["attached.png"]);
                assert_eq!(view.session.read(cx).state.error.as_deref(), Some(expected));
            });
        });
    }
    assert!(!actions
        .try_iter()
        .any(|action| matches!(action, Action::ClearConversation)));
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.session.update(cx, |session, _| {
                session.state.status = Status::Idle;
                session.state.compacting = false;
            });
            view.submit(window, cx);
            assert_eq!(view.input.read(cx).value().as_ref(), "/clear");
            assert_eq!(view.images, vec!["attached.png"]);
        });
    });
    assert_eq!(
        actions
            .try_iter()
            .filter(|action| matches!(action, Action::ClearConversation))
            .count(),
        1
    );
    view.update(cx, |view, cx| {
        view.session.update(cx, |session, cx| {
            session.state.cleared_conversations += 1;
            cx.notify();
        });
    });
    draw(cx);
    view.read_with(cx, |view, cx| {
        assert!(view.input.read(cx).value().is_empty());
        assert!(view.images.is_empty());
    });
}

#[gpui::test]
fn next_reply_escape_and_stale_acceptance_preserve_draft(cx: &mut gpui::TestAppContext) {
    let (view, cx, actions, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                seed_next_reply(view, provider, "", "Check layouts.", window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.debug_bounds("next-reply-hint").is_none());
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                seed_next_reply(view, provider, "", "Check layouts.", window, cx);
                view.session
                    .update(cx, |session, _| session.state.accepted_prompts += 1);
                assert!(
                    !view.accept_next_reply(window, cx),
                    "Stale acceptance fails before render"
                );
                assert!(!view.move_next_reply(false, window, cx));
                assert!(view.input.read(cx).value().is_empty());
            });
        });
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, Action::Submit { .. } | Action::Prompt(_))));
    }
}

#[gpui::test]
fn composer_add_context_preserves_draft_and_never_configures_or_sends(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, actions, _) = fixture(cx);
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.session
                    .update(cx, |session, _| session.state.saved.provider = provider);
                view.input.update(cx, |input, cx| {
                    input.set_value("Keep this draft ", window, cx);
                });
                view.open_menu(ComposerMenu::Add, window, cx);
                view.choose_option("__reference", window, cx);
                assert_eq!(view.input.read(cx).value().as_ref(), "@Keep this draft ");
                assert!(view.menu.is_none());
                view.open_menu(ComposerMenu::Add, window, cx);
                view.choose_option("__commands", window, cx);
                assert_eq!(view.input.read(cx).value().as_ref(), "@ /Keep this draft ");
                view.open_menu(ComposerMenu::Settings, window, cx);
                assert!(view.menu == Some(ComposerMenu::Settings));
                view.close_menu(cx);
            });
        });
        assert!(actions
            .try_iter()
            .all(|action| !matches!(action, Action::Configure { .. } | Action::Prompt { .. })));
    }
}

#[gpui::test]
fn accepted_prompt_clears_composer_once_and_preserves_next_draft(cx: &mut gpui::TestAppContext) {
    let (view, cx, _actions, updates) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input
                .update(cx, |input, cx| input.set_value("first", window, cx));
            view.submit(window, cx);
        });
    });
    draw(cx);
    assert!(cx.debug_bounds("codex-message-row-0").is_some());
    let mut accepted = snapshot();
    accepted.status = Status::Working;
    accepted.accepted_submission = view.read_with(cx, |view, _| view.submitted_ack);
    accepted.turn_id = Some("turn-ui".into());
    updates.send(accepted).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(view.read_with(cx, |view, cx| view.input.read(cx).value().is_empty()));
    assert!(cx.debug_bounds("codex-message-row-0").is_some());
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input
                .update(cx, |input, cx| input.set_value("next draft", window, cx));
        });
    });
    draw(cx);
    assert_eq!(
        view.read_with(cx, |view, cx| view.input.read(cx).value().to_string()),
        "next draft"
    );
}

#[gpui::test]
fn codex_and_claude_file_links_preserve_drafts_and_return_to_transcript(
    cx: &mut gpui::TestAppContext,
) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.rs");
        fs_err::write(&path, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();
        let (view, cx, actions, updates) = fixture(cx);
        let mut response = snapshot();
        response.saved.provider = provider;
        response.saved.cwd = root.path().display().to_string();
        response.saved.draft = "Keep this file-viewing draft".into();
        response.saved.items.push(serde_json::json!({"type":"agentMessage","id":"file-answer","text":"[main.rs:2](main.rs:2)"}));
        updates.send(response).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        // Test fixture snapshots update the session; a user's existing draft remains separately owned.
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.input.update(cx, |input, cx| {
                    input.set_value("Keep this file-viewing draft", window, cx);
                });
            });
        });
        draw(cx);
        let link = cx
            .debug_bounds("transcript-file-link")
            .expect("local file link");
        cx.simulate_click(link.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        draw(cx);
        assert!(cx.debug_bounds("file-viewer").is_some(), "{provider:?}");
        assert!(
            cx.debug_bounds("codex-composer").is_none(),
            "hidden composer must not remain focusable"
        );
        view.read_with(cx, |view, cx| {
            let browser = view.file_browser.read(cx);
            assert!(browser.visible);
            assert_eq!(browser.target.as_ref().unwrap().line, Some(2));
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                "Keep this file-viewing draft"
            );
        });
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(
            cx.debug_bounds("file-viewer").is_none(),
            "Back restores transcript"
        );
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                "Keep this file-viewing draft"
            );
        });
        assert!(!actions.try_iter().any(|action| matches!(
            action,
            Action::Prompt(_) | Action::Submit { .. } | Action::PromptWithImages { .. }
        )));
        assert!(cx.opened_url().is_none());
    }
}

#[gpui::test]
fn codex_selection_reply_pointer_capture_preserves_draft_and_sends_canonical_quote(
    cx: &mut gpui::TestAppContext,
) {
    selection_reply_pointer_fixture(cx, codex::Provider::Codex);
}

#[gpui::test]
fn claude_selection_reply_pointer_capture_preserves_draft_and_sends_canonical_quote(
    cx: &mut gpui::TestAppContext,
) {
    selection_reply_pointer_fixture(cx, codex::Provider::Claude);
}

#[gpui::test]
#[cfg(unix)]
fn codex_and_claude_snippet_run_stays_inline_and_preserves_draft(cx: &mut gpui::TestAppContext) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let root = std::env::temp_dir().join(format!(
            "grove-snippet-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs_err::create_dir_all(&root).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&root)
            .status()
            .unwrap()
            .success());
        let path = root.canonicalize().unwrap().to_string_lossy().into_owned();
        let (view, cx, _actions, updates) = fixture(cx);
        let runtime = cx.update(|window, cx| {
            cx.set_global(crate::settings::SettingsState::new(
                grove_core::storage::Store {
                    projects: vec![grove_core::storage::Project {
                        name: "Snippet fixture".into(),
                        path: path.clone(),
                        scripts: grove_core::storage::ProjectScripts::default(),
                        archived: false,
                        worktree_dir: None,
                    }],
                    ..Default::default()
                },
            ));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.));
            cx.set_global(c::ThemeState::new(
                false,
                "grove-dark".into(),
                "grove-light".into(),
            ));
            let runtime = cx.new(crate::runtime::Runtime::new);
            view.update(cx, |view, cx| {
                view.set_script_runtime(runtime.clone(), cx);
                view.input
                    .update(cx, |input, cx| input.set_value("unsent draft", window, cx));
            });
            runtime
        });
        let code = "printf '%s\n' ' first ' > snippet-output.txt\nprintf '%s\n' 'second' >> snippet-output.txt\npwd > snippet-cwd.txt";
        let mut response = snapshot();
        response.saved.provider = provider;
        response.saved.cwd = path.clone();
        let indented = code
            .lines()
            .map(|line| format!("  {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        response.saved.items.push(json!({"type":"agentMessage","id":"shell-answer","text":format!("- Run the command.\n\n  ```sh\n{indented}\n  ```\n\nThe next paragraph stays below the card."),"groveCompleted":true}));
        updates.send(response).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(40));
        draw(cx);
        let run = cx.debug_bounds("codex-run-code").unwrap().center();
        // Dispatch the native click without polling the deterministic executor:
        // a real PTY must not wake TestDispatcher from its bridge thread.
        cx.update(|window, cx| {
            use gpui::InputEvent as _;
            window.dispatch_event(
                gpui::MouseDownEvent {
                    position: run,
                    button: gpui::MouseButton::Left,
                    modifiers: gpui::Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                gpui::MouseUpEvent {
                    position: run,
                    button: gpui::MouseButton::Left,
                    modifiers: gpui::Modifiers::default(),
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
        });
        cx.update(|_, cx| {
            let registry = runtime.read(cx).registry.read(cx);
            let sessions = registry.all();
            assert!(
                sessions.is_empty(),
                "Inline scripts must not navigate to a new session"
            );
            assert!(runtime.read(cx).state.read(cx).active_session().is_none());
            let snippet = view.read(cx).snippets.values().next().unwrap();
            assert_eq!(snippet.runs.len(), 1);
            assert!(!snippet.runs[0].expanded);
            assert_eq!(
                snippet.runs[0].terminal.read(cx).initial_cwd(),
                Some(path.as_str())
            );
            assert_eq!(
                view.read(cx).input.read(cx).value().as_ref(),
                "unsent draft"
            );
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while fs_err::read_to_string(root.join("snippet-cwd.txt"))
            .ok()
            .is_none_or(|cwd| cwd.trim() != path)
        {
            cx.run_until_parked();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            fs_err::read_to_string(root.join("snippet-output.txt")).unwrap(),
            " first \nsecond\n"
        );
        assert_eq!(
            fs_err::read_to_string(root.join("snippet-cwd.txt"))
                .unwrap()
                .trim(),
            path
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            let exited = cx.update(|_, cx| {
                let terminal = view.read(cx).snippets.values().next().unwrap().runs[0]
                    .terminal
                    .clone();
                terminal.update(cx, |terminal, cx| {
                    let alive = terminal.alive();
                    cx.notify();
                    !alive
                })
            });
            if exited {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        draw(cx);
        assert!(
            cx.debug_bounds("snippet-terminal-0").is_none(),
            "Output starts collapsed"
        );
        let show = cx.debug_bounds("snippet-output-0").unwrap();
        let body = cx.debug_bounds("codex-message-body-0").unwrap();
        assert!(
            show.bottom() < body.bottom(),
            "The list footer must fit above the next paragraph"
        );
        assert!(cx.debug_bounds("snippet-reference-0").is_none());
        let toolbar = cx.debug_bounds("snippet-options").unwrap();
        cx.simulate_click(toolbar.center(), gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| {
            assert!(view
                .read(cx)
                .snippets
                .values()
                .next()
                .unwrap()
                .options_open())
        });
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            let borrowed = view.read(cx);
            let snippet = borrowed.snippets.values().next().unwrap();
            assert!(!snippet.options_open());
            assert!(snippet.options_focused(window));
        });
        cx.simulate_click(show.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("snippet-terminal-0").is_some());
        cx.simulate_keystrokes("space");
        draw(cx);
        assert!(cx.debug_bounds("snippet-terminal-0").is_none());
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(cx.debug_bounds("snippet-terminal-0").is_some());
        let reference_action = cx.debug_bounds("snippet-reference-0").unwrap();
        assert!(
            show.bottom() <= reference_action.top(),
            "Reference action belongs inside output"
        );
        cx.simulate_click(reference_action.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("codex-reference-preview").is_some());
        cx.update(|_, cx| {
            let view = view.read(cx);
            assert_eq!(view.input.read(cx).value().as_ref(), "unsent draft");
            let reference = view.reference.as_ref().unwrap();
            assert!(reference.text.contains(&path));
            assert!(reference.text.contains(code));
            assert!(reference.text.contains("exit 0"));
        });
        fs_err::remove_dir_all(root).unwrap();
    }
}
