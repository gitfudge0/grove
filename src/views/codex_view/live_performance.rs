// Explicit opt-in provider/structural-UI acceptance; not a presented-frame test.
// Run the `live_transcript_` filter with --ignored --nocapture --test-threads=1.
// Each case starts one owned authenticated CLI and consumes one model turn.
mod live_transcript_acceptance {
    use super::*;
    use std::time::{Duration, Instant};

    struct IsolatedConfig(Option<std::ffi::OsString>);
    impl IsolatedConfig {
        fn set(path: &std::path::Path) -> Self {
            let previous = std::env::var_os("GROVE_CONFIG_DIR");
            std::env::set_var("GROVE_CONFIG_DIR", path);
            Self(previous)
        }
    }
    impl Drop for IsolatedConfig {
        fn drop(&mut self) {
            match &self.0 {
                Some(previous) => std::env::set_var("GROVE_CONFIG_DIR", previous),
                None => std::env::remove_var("GROVE_CONFIG_DIR"),
            }
        }
    }

    struct OwnedTransport(Option<codex::Handle>);
    impl OwnedTransport {
        fn stop(&mut self) -> Result<(), String> {
            if let Some(handle) = self.0.take() {
                handle
                    .stop_completion(true)
                    .recv_timeout(Duration::from_secs(15))
                    .map_err(|_| {
                        "Owned live provider transport did not stop within 15 seconds".into()
                    })
            } else {
                Ok(())
            }
        }
    }
    impl Drop for OwnedTransport {
        fn drop(&mut self) {
            let _ = self.stop();
        }
    }

    // Provider diagnostics can contain paths or account details. Keep only
    // known categories in acceptance output, never raw CLI error payloads.
    fn failure_category(state: &codex::Snapshot) -> &'static str {
        let error = state
            .error
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if error.contains("401")
            || error.contains("unauthorized")
            || error.contains("authentication")
            || error.contains("not logged")
            || error.contains("oauth")
            || error.contains("api key")
        {
            "authentication"
        } else if error.contains("credit") || error.contains("balance") || error.contains("billing")
        {
            "billing_or_credit"
        } else if error.contains("429")
            || error.contains("rate limit")
            || error.contains("usage limit")
        {
            "rate_or_usage_limit"
        } else if error.contains("403")
            || error.contains("permission")
            || error.contains("forbidden")
        {
            "permission_denied"
        } else if error.contains("model") {
            "model_unavailable_or_configuration"
        } else if error.contains("network")
            || error.contains("connect")
            || error.contains("502")
            || error.contains("503")
            || error.contains("timeout")
        {
            "network_or_service"
        } else if state.error.is_some() {
            "unclassified_provider_error"
        } else {
            "terminal_state_without_error"
        }
    }

    fn await_ready(
        connection: &codex::Connection,
        provider: codex::Provider,
    ) -> Result<codex::Snapshot, String> {
        let deadline = Instant::now() + Duration::from_secs(75);
        while Instant::now() < deadline {
            match connection.updates.recv_timeout(Duration::from_millis(250)) {
                Ok(state) => {
                    if state.error.is_some()
                        || matches!(
                            state.status,
                            codex::Status::Failed | codex::Status::Disconnected
                        )
                    {
                        return Err(format!(
                            "{} startup failed category={} status={:?}",
                            provider.label(),
                            failure_category(&state),
                            state.status
                        ));
                    }
                    if state.status == codex::Status::Idle
                        && (provider == codex::Provider::Claude || state.saved.thread_id.is_some())
                    {
                        return Ok(state);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => {
                    return Err("Live provider state channel disconnected during startup".into())
                }
            }
        }
        Err(format!(
            "{} startup timed out after 75 seconds",
            provider.label()
        ))
    }

    fn live_transcript(provider: codex::Provider, cx: &mut gpui::TestAppContext) {
        // This explicitly opted-in test exercises real I/O on actor threads.
        // Use the pinned scheduler's supported nondeterministic-I/O mode for
        // this context, rather than reporting an otherwise successful transport
        // wake as deterministic-test corruption at end_test.
        cx.executor().allow_parking();
        let output =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".grove-artifacts/issue45");
        fs_err::create_dir_all(&output).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("live-transcript-")
            .tempdir_in(output)
            .unwrap();
        let checkout = directory.path().join("checkout");
        fs_err::create_dir(&checkout).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&checkout)
            .status()
            .unwrap()
            .success());
        let _environment = IsolatedConfig::set(&directory.path().join("config"));
        let mut saved = snapshot().saved;
        saved.provider = provider;
        saved.key = grove_core::identity::random_id().unwrap();
        saved.cwd = checkout
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        saved.project = "Live transcript QA".into();
        saved.thread_id = None;
        saved.native_config_version = 1;
        saved.activity_reporting = false;
        saved.items.clear();
        grove_core::session_title::set_manual_title(&mut saved, "Live transcript QA");
        let args = provider.agent().launch_args_for_backend(true, false, false);
        let connection = codex::connect(saved, args);
        let handle = connection.handle.clone();
        let mut owned = OwnedTransport(Some(handle.clone()));
        let initial = await_ready(&connection, provider).unwrap();
        cx.update(|cx| {
            gpui_component::init(cx);
            if cx.try_global::<crate::settings::SettingsState>().is_none() {
                cx.set_global(crate::settings::SettingsState::new(
                    grove_core::storage::Store::default(),
                ));
            }
            cx.set_reduce_motion(true);
        });
        let mut retained = None;
        let (_, visual) = cx.add_window_view(|window, cx| {
            let session = cx.new(|cx| CodexSession::from_connection(initial, connection, cx));
            let chat = cx.new(|cx| CodexView::new(session, window, cx));
            chat.update(cx, |view, _| {
                view.next_reply_generate = |_, _| Ok(Vec::new());
            });
            retained = Some(chat.clone());
            gpui_component::Root::new(chat, window, cx).bordered(false)
        });
        let view = retained.unwrap();
        let prompt = "Do not use tools or inspect files. Return only Markdown: a heading `# Live transcript verification`, sixteen numbered lines each containing a different short explanation of why preserving a conversation draft matters, and finally a fenced rust code block containing exactly fn main() { println!(\"live-stream-complete\"); }. Include no other text.";
        let submission = handle.submit(prompt.into(), Vec::new()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(150);
        let mut stream_revisions = std::collections::HashSet::new();
        let mut rendered_stream = false;
        let mut draft_set = false;
        let mut completed = false;
        while Instant::now() < deadline {
            // Real actor threads wake the UI. Only GPUI's timer clock is advanced;
            // provider snapshots and streaming text are never synthesized.
            std::thread::sleep(Duration::from_millis(20));
            visual.executor().advance_clock(Duration::from_millis(50));
            visual.update(gpui::Window::simulate_next_frame);
            draw(visual);
            let observation = view.read_with(visual, |view, cx| {
                let state = &view.session.read(cx).state;
                let prose = state.saved.items.iter().any(|item| {
                    item["type"] == "agentMessage"
                        && !item["text"].as_str().unwrap_or_default().is_empty()
                });
                let owner_text = state.saved.items.iter().enumerate().any(|(index, item)| {
                    if item["type"] != "agentMessage" {
                        return false;
                    }
                    let text = transcript_item_text(item);
                    let message = item["id"]
                        .as_str()
                        .map_or_else(|| format!("message-{index}"), str::to_owned);
                    let owner_key = format!(
                        "{}:{message}",
                        state.saved.thread_id.as_deref().unwrap_or_default()
                    );
                    !text.is_empty()
                        && view
                            .selection_views
                            .get(&owner_key)
                            .is_some_and(|owner| owner.read(cx).source().as_ref() == text)
                });
                (
                    state.status.busy(),
                    state.accepted_submission == Some(submission),
                    state.saved.items.revision(),
                    prose,
                    owner_text,
                    state.session_status() == "Review",
                    state.error.is_some(),
                    matches!(
                        state.status,
                        codex::Status::Failed
                            | codex::Status::Interrupted
                            | codex::Status::Disconnected
                    ),
                    failure_category(state),
                    state.status,
                )
            });
            assert!(
                !observation.6 && !observation.7,
                "{} live turn failed category={} status={:?}",
                provider.label(),
                observation.8,
                observation.9
            );
            if observation.1 && observation.0 && !draft_set {
                visual.update(|window, cx| {
                    view.update(cx, |view, cx| {
                        view.input.update(cx, |input, cx| {
                            input.replace_all("Retained live transcript draft", window, cx);
                        });
                    });
                });
                draft_set = true;
            }
            if observation.0 && observation.3 {
                stream_revisions.insert(observation.2);
                rendered_stream |= observation.4;
            }
            if observation.1 && observation.5 {
                completed = true;
                break;
            }
        }
        assert!(
            completed,
            "{} live turn timed out after 150 seconds",
            provider.label()
        );
        assert!(
            !stream_revisions.is_empty() && rendered_stream,
            "{} did not expose streaming prose through the real transport and rendered text owner",
            provider.label()
        );
        assert!(draft_set, "No in-progress draft was created");
        // Let the asynchronous tail parser and input persistence settle.
        for _ in 0..8 {
            visual.executor().advance_clock(Duration::from_millis(50));
            visual.update(gpui::Window::simulate_next_frame);
            draw(visual);
        }
        view.read_with(visual, |view, cx| {
            let state = &view.session.read(cx).state;
            assert_eq!(state.session_status(), "Review");
            assert_eq!(state.saved.draft, "Retained live transcript draft");
            assert_eq!(
                view.input.read(cx).value().as_ref(),
                "Retained live transcript draft"
            );
            assert!(
                view.selection_views.values().any(|owner| {
                    let owner = owner.read(cx);
                    owner.source().contains("live-stream-complete")
                        && !owner.code_blocks().is_empty()
                }),
                "Completed real Markdown/code must reach the rendered TextView owner"
            );
        });
        assert!(
            visual.debug_bounds("session-review").is_some(),
            "Live completion must render the Review surface"
        );
        println!("LIVE_TRANSCRIPT provider={} observed_stream_revisions={} rendered_stream=true final_code=true review=true draft_preserved=true scope=live_transport_and_structural_gpui_only", provider.label(), stream_revisions.len());
        owned.stop().unwrap();
    }

    #[gpui::test]
    #[ignore = "One live authenticated Codex turn; run alone with --test-threads=1"]
    fn live_transcript_codex_streaming_reaches_rendered_owner(cx: &mut gpui::TestAppContext) {
        live_transcript(codex::Provider::Codex, cx);
    }

    #[gpui::test]
    #[ignore = "One live authenticated Claude turn; run alone with --test-threads=1"]
    fn live_transcript_claude_streaming_reaches_rendered_owner(cx: &mut gpui::TestAppContext) {
        live_transcript(codex::Provider::Claude, cx);
    }
}
