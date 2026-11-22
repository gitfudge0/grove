use super::*;

#[gpui::test]
fn draft_handle_commands_wake_without_advancing_timers(cx: &mut gpui::TestAppContext) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let session =
            cx.update(|cx| cx.new(|cx| CodexSession::draft(saved(provider), Vec::new(), cx)));
        cx.update(|cx| {
            let handle = session.read(cx).handle.clone();
            // More actions than wake slots must still all reach the draft.
            for index in 0..20 {
                handle
                    .send(Action::Draft(format!("draft {index}")))
                    .unwrap();
            }
            handle
                .send(Action::DraftImages(vec!["retained-image".into()]))
                .unwrap();
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(session.read(cx).state.saved.draft, "draft 19");
            assert_eq!(
                session.read(cx).state.saved.draft_images,
                vec!["retained-image"]
            );
            session
                .read(cx)
                .handle
                .send(Action::Draft("second wake".into()))
                .unwrap();
        });
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(session.read(cx).state.saved.draft, "second wake"));
    }
}

#[gpui::test]
fn dashboard_view_sends_ready_launch_as_first_turn(cx: &mut gpui::TestAppContext) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(crate::settings::SettingsState::new(
                grove_core::storage::Store::default(),
            ));
        });
        let (handle, actions) = Handle::channel();
        let (_updates, receive) = std::sync::mpsc::channel();
        let mut initial = Snapshot::new(saved(provider));
        initial.saved.draft = "First dashboard prompt".into();
        let mut session_entity = None;
        let (view, window_cx) = cx.add_window_view(|window, cx| {
            let session = cx.new(|cx| {
                let mut session = CodexSession::from_connection(
                    initial,
                    codex::Connection {
                        handle,
                        updates: receive.into(),
                    },
                    cx,
                );
                session.dashboard_launch = Some(DashboardLaunch::Starting);
                session
            });
            session_entity = Some(session.clone());
            crate::views::CodexView::new(session, window, cx)
        });
        window_cx
            .update(|window, cx| view.update(cx, |view, cx| view.launch_dashboard(window, cx)));
        let session = session_entity.unwrap();
        window_cx.update(|_, cx| {
            session.update(cx, |session, cx| {
                let mut ready = (*session.state).clone();
                ready.status = codex::Status::Idle;
                ready.saved.thread_id = Some("ready-thread".into());
                session.accept_snapshot(ready);
                assert_eq!(session.state.session_status(), "Working");
                cx.notify();
            });
        });
        window_cx.run_until_parked();
        let dispatched = actions.try_iter().collect::<Vec<_>>();
        assert_eq!(
            dispatched
                .iter()
                .filter(|action| matches!(action, Action::Submit { .. }))
                .count(),
            1,
            "{provider:?} must send, not enqueue"
        );
        assert_eq!(
            session.read_with(window_cx, |s, _| s.state.session_status()),
            "Working"
        );
    }
}

#[gpui::test]
fn dashboard_launch_never_publishes_ready_before_first_prompt(cx: &mut gpui::TestAppContext) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|cx| {
            let (handle, actions) = Handle::channel();
            let (_updates, receive) = std::sync::mpsc::channel();
            let initial = Snapshot::new(saved(provider));
            let session = cx.new(|cx| {
                CodexSession::from_connection(
                    initial.clone(),
                    codex::Connection {
                        handle,
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            session.update(cx, |session, cx| {
                session.dashboard_launch = Some(DashboardLaunch::Starting);
                let mut ready = initial;
                ready.status = codex::Status::Idle;
                ready.saved.thread_id = Some("ready-thread".into());
                session.accept_snapshot(ready.clone());
                assert_eq!(session.state.session_status(), "Working", "{provider:?}");
                assert!(session.dashboard_submission_ready());
                session
                    .queue_prompt("First prompt".into(), Vec::new(), true, cx)
                    .unwrap();
                assert!(actions
                    .try_iter()
                    .any(|action| matches!(action, Action::Submit { .. })));
                // A delayed idle snapshot must not undo the optimistic send state.
                session.accept_snapshot(ready.clone());
                assert_eq!(session.state.session_status(), "Working");
                assert!(!session.dashboard_submission_ready());
                let mut working = ready.clone();
                working.status = codex::Status::Working;
                session.accept_snapshot(working);
                session.accept_snapshot(ready.clone());
                assert_eq!(
                    session.state.session_status(),
                    "Ready",
                    "later idle is real"
                );
                for status in [
                    codex::Status::Failed,
                    codex::Status::Disconnected,
                    codex::Status::Interrupted,
                ] {
                    session.dashboard_launch = Some(DashboardLaunch::Starting);
                    let mut failed = ready.clone();
                    failed.status = status;
                    session.accept_snapshot(failed);
                    assert_eq!(session.state.status, status);
                    assert!(session.dashboard_launch.is_none());
                }
                session.dashboard_launch = Some(DashboardLaunch::Starting);
                session.accept_snapshot(ready);
                session.cancel_dashboard_title_waiting();
                assert_eq!(session.state.session_status(), "Ready");
            });
        });
    }
}

#[gpui::test]
fn restored_codex_and_claude_keep_metadata_without_starting_transport(
    cx: &mut gpui::TestAppContext,
) {
    let sessions = cx.update(|cx| {
        [codex::Provider::Codex, codex::Provider::Claude]
            .into_iter()
            .map(|provider| {
                let mut metadata = saved(provider);
                metadata.title = Some("Retained title".into());
                metadata.draft = "Retained draft".into();
                metadata.thread_id = Some("retained-thread".into());
                let expected = serde_json::to_value(&metadata).unwrap();
                let session = cx.new(|cx| CodexSession::restored(metadata, Vec::new(), cx));
                assert_eq!(
                    serde_json::to_value(&session.read(cx).state.saved).unwrap(),
                    expected
                );
                session
            })
            .collect::<Vec<_>>()
    });
    cx.run_until_parked();
    for session in sessions {
        session.read_with(cx, |session, _| {
            assert!(!session.has_transport());
            assert!(session.restored.is_some());
            assert!(session.restore_load.is_none());
            assert!(session.skill_watch.is_none());
            assert!(!session.catalog_loading);
            assert_eq!(session.state.status, codex::Status::Idle);
        });
    }
}

#[gpui::test]
fn dormant_codex_and_claude_validate_on_activation_and_reconnect(cx: &mut gpui::TestAppContext) {
    // Real actor threads wake the foreground receiver during this integration test.
    cx.executor().allow_parking();
    let _config = crate::settings::lock_test_config();
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let mut record = saved(provider);
        record.key = format!("dormant-validation-{}-{:?}", std::process::id(), provider);
        record.persist_recovery().unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = calls.clone();
        let validation = std::sync::Arc::new(move || {
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err("checkout no longer belongs to this project".into())
        });
        let session = cx.update(|cx| {
            cx.new(|cx| {
                CodexSession::restored_checked(
                    record.clone(),
                    vec!["--unsupported-grove-test-option".into()],
                    validation,
                    cx,
                )
            })
        });
        cx.run_until_parked();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        session.update(cx, CodexSession::ensure_connected);
        for attempt in 1..=2 {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                cx.run_until_parked();
                cx.executor().advance_clock(Duration::from_millis(50));
                if session.read_with(cx, |session, _| {
                    session.state.error.as_deref().is_some_and(|error| {
                        error.contains("checkout no longer belongs to this project")
                    })
                }) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "validation error was not surfaced for {provider:?}"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), attempt);
            session.read_with(cx, |session, _| {
                assert!(!session
                    .skill_startup_ready
                    .as_ref()
                    .unwrap()
                    .load(std::sync::atomic::Ordering::Acquire));
            });
            if attempt == 1 {
                session.update(cx, |session, cx| {
                    session.state.error = None;
                    session.send(Action::Reconnect, cx);
                });
            }
        }
        let handle = session.read_with(cx, |session, _| session.handle.clone());
        let (done, outcome) = handle.stop_archiving();
        cx.update(|_| drop(session));
        done.recv_timeout(Duration::from_secs(10)).unwrap();
        outcome
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        codex::finish_history_reopen(&record.key).unwrap();
    }
}

#[gpui::test]
fn restored_replacement_keeps_codex_and_claude_command_identity(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let record = saved(provider);
            let session = cx.new(|cx| CodexSession::restored(record.clone(), Vec::new(), cx));
            session.update(cx, |session, cx| {
                let restore = session.restored.take().unwrap();
                let stable = session.handle.clone();
                let (_updates, output) = grove_core::latest::channel();
                let replacement = CodexSession::from_connection(Snapshot::new(record), codex::Connection { handle: stable.clone(), updates: output }, cx);
                session.replace_restored(replacement);
                assert!(restore.connection.take_pending_actions().is_empty(), "replacing the view must not send Stop to its new actor");
                stable.send(Action::Draft("same command channel".into())).unwrap();
                assert!(matches!(restore.connection.take_pending_actions().as_slice(), [Action::Draft(text)] if text == "same command channel"));
            });
        }
    });
}

#[gpui::test]
fn restored_failure_rejects_unsent_requests_and_keeps_drafts_for_retry(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let session = cx.new(|cx| CodexSession::restored(saved(provider), Vec::new(), cx));
            session.update(cx, |session, _| {
                session.restored.as_mut().unwrap().loading = true;
                session
                    .handle
                    .send(Action::Draft("typed during hydration".into()))
                    .unwrap();
                let id = session
                    .handle
                    .submit("must not replay".into(), Vec::new())
                    .unwrap();
                session.restore_failed("temporary recovery failure".into());
                let restore = session.restored.take().unwrap();
                assert!(!restore.loading);
                assert!(restore.draft_changed);
                assert!(restore.connection.take_pending_actions().is_empty());
                assert_eq!(session.state.saved.draft, "typed during hydration");
                assert_eq!(session.state.rejected_submission, Some(id));
                assert_eq!(session.state.status, codex::Status::Failed);
            });
        }
    });
}

#[gpui::test]
fn closing_dormant_codex_and_claude_archives_before_activation(cx: &mut gpui::TestAppContext) {
    let _config = crate::settings::lock_test_config();
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        let key = format!(
            "dormant-close-{}-{}",
            if provider == codex::Provider::Codex {
                "codex"
            } else {
                "claude"
            },
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let mut record = saved(provider);
        record.key = key.clone();
        record.draft = "retained unsent draft".into();
        record.items.push(serde_json::json!({"id":"retained","type":"agentMessage","text":"retained full history"}));
        record.persist_recovery().unwrap();
        let mut metadata = record.clone();
        metadata.items.clear();
        let session = cx.update(|cx| {
            cx.new(|cx| {
                CodexSession::restored_checked(
                    metadata,
                    vec!["--unsupported-grove-test-option".into()],
                    std::sync::Arc::new(|| {
                        panic!("closing a dormant session must not validate or launch")
                    }),
                    cx,
                )
            })
        });
        let handle = session.read_with(cx, |session, _| session.handle.clone());
        let (done, outcome) = handle.stop_archiving();
        cx.update(|_| drop(session));
        cx.run_until_parked();
        done.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(
            outcome.recv_timeout(Duration::from_secs(10)).unwrap(),
            Ok(())
        );
        assert!(!codex::recovery_dir()
            .unwrap()
            .join(format!("{key}.json"))
            .exists());
        let archived = codex::load_history_session(&key).unwrap();
        assert_eq!(archived.items, record.items);
        assert_eq!(archived.draft, record.draft);
        codex::finish_history_reopen(&key).unwrap();
    }
}

#[gpui::test]
fn closing_missing_dormant_recovery_reports_archival_error(cx: &mut gpui::TestAppContext) {
    let _config = crate::settings::lock_test_config();
    let mut record = saved(codex::Provider::Codex);
    record.key = format!(
        "missing-dormant-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let session = cx.update(|cx| cx.new(|cx| CodexSession::restored(record, Vec::new(), cx)));
    let handle = session.read_with(cx, |session, _| session.handle.clone());
    let (done, outcome) = handle.stop_archiving();
    cx.update(|_| drop(session));
    cx.run_until_parked();
    done.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(outcome
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap_err()
        .contains("Could not close restored session"));
}

#[gpui::test]
fn history_snapshot_has_no_transport_or_catalog_for_codex_and_claude(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let record = saved(provider);
            let expected = serde_json::to_value(&record).unwrap();
            let session = cx.new(|cx| CodexSession::history(record, cx));
            let session = session.read(cx);
            assert_eq!(
                serde_json::to_value(&session.state.saved).unwrap(),
                expected
            );
            assert_eq!(session.state.status, codex::Status::Done);
            assert!(!session.catalog_loading);
            assert!(session.deferred.is_none());
            assert!(session.handle.send(Action::Reconnect).is_err());
        }
    });
}

#[gpui::test]
fn dashboard_title_remains_pending_through_startup_and_hands_off_or_fails(
    cx: &mut gpui::TestAppContext,
) {
    for provider in [codex::Provider::Codex, codex::Provider::Claude] {
        cx.update(|cx| {
            let (handle, _actions) = Handle::channel();
            let (_updates, receive) = std::sync::mpsc::channel();
            let mut starting = Snapshot::new(saved(provider));
            starting.status = codex::Status::Starting;
            let session = cx.new(|cx| {
                CodexSession::from_connection(
                    starting.clone(),
                    codex::Connection {
                        handle,
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            session.update(cx, |session, _| {
                session.dashboard_title_waiting = Some("First prompt".into());
                session.accept_snapshot(starting.clone());
                assert!(session.state.title_generation_pending);
                assert_eq!(session.state.saved.title.as_deref(), Some("First prompt"));
                let mut failed = starting.clone();
                failed.status = codex::Status::Disconnected;
                session.accept_snapshot(failed);
                assert!(!session.state.title_generation_pending);
                assert!(session.dashboard_title_waiting.is_none());
                session.dashboard_title_waiting = Some("First prompt".into());
                let mut actor_pending = starting.clone();
                actor_pending.title_generation_pending = true;
                session.accept_snapshot(actor_pending.clone());
                assert!(session.state.title_generation_pending);
                assert!(session.dashboard_title_waiting.is_some());
                let mut rejected = actor_pending.clone();
                rejected.rejected_submission = Some(42);
                rejected.title_generation_pending = false;
                session.accept_snapshot(rejected);
                assert!(!session.state.title_generation_pending);
                assert!(session.dashboard_title_waiting.is_none());
                session.dashboard_title_waiting = Some("First prompt".into());
                session.accept_snapshot(starting.clone());
                session.cancel_dashboard_title_waiting();
                assert!(!session.state.title_generation_pending);
                session.dashboard_title_waiting = Some("First prompt".into());
                let mut actor_pending = starting;
                actor_pending.title_generation_pending = true;
                actor_pending.saved.title = Some("First prompt".into());
                session.accept_snapshot(actor_pending.clone());
                assert!(session.state.title_generation_pending);
                assert!(session.dashboard_title_waiting.is_none());
                actor_pending.title_generation_pending = false;
                session.accept_snapshot(actor_pending);
                assert!(!session.state.title_generation_pending);
            });
        });
    }
}

fn saved(provider: codex::Provider) -> SavedSession {
    SavedSession {
        html_policy: grove_core::visuals::HtmlPolicy::Legacy,
        visuals: Vec::new(),
        claude_subagents: std::collections::HashMap::default(),
        session_review: grove_core::codex::SessionReview::default(),
        rich_plans: grove_core::rich_plan::PlanStore::default(),
        side_chats: std::collections::HashMap::default(),
        fork_origin: None,
        claude_resume: None,
        edit_checkpoints: Vec::new(),
        pending_edit: None,
        rewind_drafts: Vec::new(),
        provider,
        key: "dashboard-deferred-test".into(),
        project: "before".into(),
        cwd: "/unused".into(),
        label: provider.label().into(),
        title: None,
        title_generation: None,
        roots: Vec::new(),
        thread_id: None,
        items: Vec::new().into(),
        work_periods: Vec::new(),
        draft: String::new(),
        draft_images: Vec::new(),
        draft_reference: None,
        config_args: Vec::new(),
        native_config_version: 1,
        activity_reporting: false,
        composer: codex::ComposerSettings::default(),
        context: None,
        context_details: None,
    }
}

#[gpui::test]
fn refreshed_skills_survive_queued_snapshots_until_actor_acknowledges(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let (handle, _actions) = Handle::channel();
            let (_updates, receive) = std::sync::mpsc::channel();
            let mut snapshot = Snapshot::new(saved(provider));
            snapshot.saved.draft = "keep typing".into();
            snapshot.saved.thread_id = Some("same-thread".into());
            let session = cx.new(|cx| {
                CodexSession::from_connection(
                    snapshot.clone(),
                    codex::Connection {
                        handle,
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            session.update(cx, |session, _| {
                let skills: Vec<serde_json::Value> = vec![serde_json::json!({"name":"new-skill"})];
                session.refreshed_skills = Some(skills.clone());
                session.accept_snapshot(snapshot.clone());
                assert_eq!(session.state.skills, skills);
                assert_eq!(session.state.saved.draft, "keep typing");
                assert_eq!(
                    session.state.saved.thread_id.as_deref(),
                    Some("same-thread")
                );
                assert!(session.refreshed_skills.is_some());
                snapshot.skills = skills.into();
                session.accept_snapshot(snapshot.clone());
                assert!(session.refreshed_skills.is_none());
                snapshot.skills.clear();
                session.accept_snapshot(snapshot.clone());
                assert!(session.state.skills.is_empty());
            });
        }
    });
}

#[gpui::test]
fn deferred_title_edits_preserve_manual_ownership_and_composer(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let session = cx.new(|cx| CodexSession::draft(saved(provider), Vec::new(), cx));
            session.update(cx, |session, cx| {
                session.send(Action::Draft("Keep my draft".into()), cx);
                session.send(Action::SetTitle("  My session title  ".into()), cx);
                assert_eq!(
                    session.state.saved.title.as_deref(),
                    Some("My session title")
                );
                assert!(session.state.saved.title_generation.is_some());
                assert_eq!(session.state.saved.draft, "Keep my draft");
                assert!(session.is_deferred());
                assert!(!session.state.title_generation_pending);
                session.send(Action::SetTitle("  ".into()), cx);
                assert_eq!(
                    session.state.saved.title.as_deref(),
                    Some("My session title")
                );
                let restored = serde_json::from_value::<SavedSession>(
                    serde_json::to_value(&session.state.saved).unwrap(),
                )
                .unwrap();
                assert_eq!(restored.title.as_deref(), Some("My session title"));
                assert_eq!(restored.provider, provider);
            });
        }
    });
}

#[gpui::test]
fn draft_model_selection_keeps_ready_efforts_without_another_handshake(
    cx: &mut gpui::TestAppContext,
) {
    // Scope fingerprints the test config path. Initialize it before another
    // parallel UI fixture can change the environment between the two reads.
    let _config = crate::settings::lock_test_config();
    let checkout = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            // Start outside a real checkout so this fixture never launches a CLI.
            let session = cx.new(|cx| CodexSession::draft(saved(provider), Vec::new(), cx));
            session.update(cx, |session, cx| {
                session.state.saved.cwd = checkout.path().to_string_lossy().into_owned();
                session.catalog_scope = Some(Scope::new(&session.state.saved, &[]));
                session.catalog_loading = false;
                session.catalog_error = None;
                session.state.models = vec![
                    serde_json::json!({"model":"first", "supportedReasoningEfforts":[{"reasoningEffort":"low"}]}),
                    serde_json::json!({"model":"second", "supportedReasoningEfforts":[{"reasoningEffort":"high"},{"reasoningEffort":"max"}]}),
                ].into();
                session.state.skills = vec![serde_json::json!({"name":"checkout-skill"})].into();
                session.state.resolved_access = Some("read-only".into());
                session.state.saved.draft = "keep my draft".into();
                session.state.effort = Some("low".into());
                session.state.saved.composer.effort = Some("low".into());
                let generation = session.catalog_generation;
                session.select_draft_model(provider, "second".into(), Vec::new(), cx).unwrap();
                assert_eq!(session.catalog_generation, generation);
                assert!(!session.catalog_loading);
                assert!(session.catalog_error.is_none());
                assert_eq!(session.state.models[1]["supportedReasoningEfforts"][1]["reasoningEffort"], "max");
                assert_eq!(session.state.skills[0]["name"], "checkout-skill");
                assert_eq!(session.state.resolved_access.as_deref(), Some("read-only"));
                assert_eq!(session.state.saved.composer.model.as_deref(), Some("second"));
                assert!(session.state.saved.composer.effort.is_none());
                assert_eq!(session.state.saved.draft, "keep my draft");
            });
        }
    });
}

#[gpui::test]
fn draft_model_switch_preserves_content_and_resets_provider_configuration(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        for (before, after) in [
            (codex::Provider::Codex, codex::Provider::Claude),
            (codex::Provider::Claude, codex::Provider::Codex),
        ] {
            let session = cx.new(|cx| CodexSession::draft(saved(before), Vec::new(), cx));
            session.update(cx, |session, cx| {
                let reference = codex::ReferenceDraft {
                    id: "quote".into(),
                    thread_id: Some("source".into()),
                    sources: Vec::new(),
                    text: "keep this quote".into(),
                };
                session.send(Action::DraftImages(vec!["image-data".into()]), cx);
                session.send(Action::DraftReference(Some(reference.clone())), cx);
                session.send(
                    Action::Configure {
                        model: Some("old-model".into()),
                        effort: Some("high".into()),
                        access: Some("full-access".into()),
                    },
                    cx,
                );
                session.send(Action::SetSpeed(codex::Speed::Ultrafast), cx);
                session
                    .handle
                    .send(Action::Draft("newest prompt".into()))
                    .unwrap();
                let generation = session.catalog_generation;
                session
                    .select_draft_model(after, "chosen-model".into(), Vec::new(), cx)
                    .unwrap();
                assert!(session.is_deferred());
                assert_eq!(session.state.saved.provider, after);
                assert_eq!(session.state.saved.label, after.label());
                assert_eq!(session.state.saved.project, "before");
                assert_eq!(session.state.saved.cwd, "/unused");
                assert_eq!(session.state.saved.draft, "newest prompt");
                assert_eq!(session.state.saved.draft_images, vec!["image-data"]);
                assert_eq!(session.state.saved.draft_reference, Some(reference));
                assert_eq!(
                    session.state.saved.composer.model.as_deref(),
                    Some("chosen-model")
                );
                assert!(session.state.saved.composer.effort.is_none());
                assert!(session.state.saved.composer.access.is_none());
                assert_eq!(
                    session.state.saved.composer.speed,
                    Some(codex::Speed::Standard)
                );
                assert_ne!(session.catalog_generation, generation);
                assert!(session.state.saved.thread_id.is_none());
            });
        }
    });
}

#[gpui::test]
fn draft_model_selection_rejects_running_session_without_changing_identity(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        let session =
            cx.new(|cx| CodexSession::draft(saved(codex::Provider::Codex), Vec::new(), cx));
        session.update(cx, |session, cx| {
            // Simulate activation's invariant without opening a provider process.
            session.deferred = None;
            session.state.saved.draft = "keep my task".into();
            let generation = session.catalog_generation;
            let error = session
                .select_draft_model(codex::Provider::Claude, "other".into(), Vec::new(), cx)
                .unwrap_err();
            assert!(error.contains("cannot switch providers"));
            assert_eq!(session.state.saved.provider, codex::Provider::Codex);
            assert_eq!(session.state.saved.draft, "keep my task");
            assert!(session.state.saved.composer.model.is_none());
            assert_eq!(session.catalog_generation, generation);
        });
    });
}

#[gpui::test]
fn draft_activation_rejects_pending_failed_and_unadvertised_models(cx: &mut gpui::TestAppContext) {
    let checkout = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let session = cx.new(|cx| CodexSession::draft(saved(provider), Vec::new(), cx));
            session.update(cx, |session, cx| {
                let cwd = checkout.path().to_string_lossy().into_owned();
                session.state.saved.cwd.clone_from(&cwd);
                session.state.saved.draft = "retained task".into();
                session.catalog_error = None;
                session.catalog_loading = true;
                let error = session
                    .activate("project".into(), cwd.clone(), Vec::new(), cx)
                    .unwrap_err();
                assert!(error.contains("finish loading"));
                session.catalog_loading = false;
                session.catalog_error = Some("Provider metadata failed".into());
                let error = session
                    .activate("project".into(), cwd.clone(), Vec::new(), cx)
                    .unwrap_err();
                assert_eq!(error, "Provider metadata failed");
                session.catalog_error = None;
                session
                    .handle
                    .send(Action::Configure {
                        model: Some("missing-model".into()),
                        effort: None,
                        access: None,
                    })
                    .unwrap();
                session.state.models = vec![serde_json::json!({"model":"available-model"})].into();
                let error = session
                    .activate("project".into(), cwd, Vec::new(), cx)
                    .unwrap_err();
                assert!(error.contains("no longer advertised"));
                assert!(session.is_deferred());
                assert_eq!(session.state.saved.provider, provider);
                assert_eq!(session.state.saved.draft, "retained task");
                // Ready defaults need no explicit model. An advertised explicit model is valid.
                session.state.saved.composer.model = None;
                assert!(session.validate_draft_catalog().is_ok());
                session.state.saved.composer.model = Some("available-model".into());
                assert!(session.validate_draft_catalog().is_ok());
            });
        }
    });
}

#[gpui::test]
fn draft_catalog_ignores_superseded_results_and_rejects_disappeared_selection(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        let session =
            cx.new(|cx| CodexSession::draft(saved(codex::Provider::Codex), Vec::new(), cx));
        session.update(cx, |session, cx| {
            let old_generation = session.catalog_generation;
            session
                .select_draft_model(
                    codex::Provider::Claude,
                    "chosen-model".into(),
                    Vec::new(),
                    cx,
                )
                .unwrap();
            session.catalog_loading = true;
            session.catalog_error = None;
            let mut stale = Snapshot::new(saved(codex::Provider::Codex));
            stale.models = vec![serde_json::json!({"model":"stale-codex-model"})].into();
            session.complete_catalog(old_generation, Ok(stale), cx);
            assert!(session.catalog_loading);
            assert!(session.state.models.is_empty());
            assert_eq!(session.state.saved.provider, codex::Provider::Claude);
            session.complete_catalog(old_generation, Err("old failure".into()), cx);
            assert!(session.catalog_error.is_none());
            let mut current = Snapshot::new(saved(codex::Provider::Claude));
            current.models = vec![serde_json::json!({"model":"different-model"})].into();
            session.complete_catalog(session.catalog_generation, Ok(current), cx);
            assert!(!session.catalog_loading);
            assert!(session
                .catalog_error
                .as_deref()
                .unwrap()
                .contains("no longer advertised"));
            assert_eq!(
                session.state.saved.composer.model.as_deref(),
                Some("chosen-model")
            );
        });
    });
}

#[gpui::test]
fn draft_catalog_selection_recovers_on_reset_but_discovery_failure_does_not(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let session = cx.new(|cx| CodexSession::draft(saved(provider), Vec::new(), cx));
            session.update(cx, |session, cx| {
                for action in [
                    Action::ResetModelEffort,
                    Action::ResetConfiguration,
                    Action::Configure {
                        model: Some("available".into()),
                        effort: None,
                        access: None,
                    },
                ] {
                    session.state.model = Some("removed".into());
                    session.state.saved.composer.model = Some("removed".into());
                    let mut catalog = Snapshot::new(saved(provider));
                    catalog.models = vec![serde_json::json!({"model":"available"})].into();
                    session.complete_catalog(session.catalog_generation, Ok(catalog), cx);
                    assert!(session.catalog_error.is_some());
                    assert!(session.state.error.is_some());
                    session.send(action, cx);
                    assert!(session.catalog_error.is_none());
                    assert!(session.state.error.is_none());
                    assert!(session.validate_draft_catalog().is_ok());
                }
                for action in [Action::ResetModelEffort, Action::ResetConfiguration] {
                    session.complete_catalog(
                        session.catalog_generation,
                        Err("provider discovery failed".into()),
                        cx,
                    );
                    session.send(action, cx);
                    assert_eq!(
                        session.catalog_error.as_deref(),
                        Some("provider discovery failed")
                    );
                    assert_eq!(
                        session.state.error.as_deref(),
                        Some("provider discovery failed")
                    );
                    assert!(session.validate_draft_catalog().is_err());
                }
            });
        }
    });
}

#[gpui::test]
fn deferred_composer_preserves_provider_options_and_rejects_stale_checkout(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|cx| {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let session = cx.new(|cx| CodexSession::draft(saved(provider), Vec::new(), cx));
            session.update(cx, |session, cx| {
                session.send(Action::Draft("retained task".into()), cx);
                session.send(
                    Action::DraftImages(vec!["data:image/png;base64,fixture".into()]),
                    cx,
                );
                session.send(
                    Action::Configure {
                        model: Some("selected-model".into()),
                        effort: Some("high".into()),
                        access: Some("full-access".into()),
                    },
                    cx,
                );
                // The composer also sends directly through Handle. Activation must
                // flush those commands before reading the launch draft.
                session
                    .handle
                    .send(Action::Draft("latest direct draft".into()))
                    .unwrap();
                session.drain_deferred(cx);
                session.state.skills =
                    vec![serde_json::json!({"name":"old-checkout-skill"})].into();
                session.state.resolved_model = Some("old-checkout-model".into());
                session.state.resolved_access = Some("full-access".into());
                session.refresh_catalog(
                    "/nonexistent/grove-dashboard-checkout".into(),
                    Vec::new(),
                    cx,
                );
                assert!(session.state.skills.is_empty());
                assert!(session.state.resolved_model.is_none());
                assert!(session.state.resolved_access.is_none());
                let error = session
                    .activate(
                        "chosen project".into(),
                        "/nonexistent/grove-dashboard-checkout".into(),
                        Vec::new(),
                        cx,
                    )
                    .unwrap_err();
                assert!(error.contains("checkout"));
                assert!(session.is_deferred());
                assert_eq!(session.state.saved.project, "before");
                assert_eq!(session.state.saved.draft, "latest direct draft");
                assert_eq!(session.state.saved.draft_images.len(), 1);
                assert_eq!(session.state.saved.provider, provider);
                assert_eq!(
                    session.state.saved.composer.model.as_deref(),
                    Some("selected-model")
                );
                assert_eq!(session.state.saved.composer.effort.as_deref(), Some("high"));
                assert_eq!(
                    session.state.saved.composer.access.as_deref(),
                    Some("full-access")
                );
                assert!(session.state.saved.thread_id.is_none());
            });
        }
    });
}
