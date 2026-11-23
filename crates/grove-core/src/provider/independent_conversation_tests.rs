use super::*;
use std::sync::{mpsc, Arc};
#[test]
fn command_wake_happens_after_enqueue_and_clones_retain_it() {
    let observed = Arc::new(AtomicU64::new(0));
    let wakes = observed.clone();
    let (handle, receiver) = Handle::channel_with_wake(Arc::new(move || {
        wakes.fetch_add(1, Ordering::Relaxed);
    }));
    handle.clone().send(Action::Draft("queued".into())).unwrap();
    assert_eq!(observed.load(Ordering::Relaxed), 1);
    assert!(matches!(receiver.try_recv().unwrap(), Action::Draft(text) if text == "queued"));
    drop(receiver);
    assert!(handle.send(Action::Draft("disconnected".into())).is_err());
    assert_eq!(
        observed.load(Ordering::Relaxed),
        1,
        "failed enqueue must not wake"
    );
}

#[test]
fn deferred_wake_retargets_cloned_handles_without_losing_queued_actions() {
    let wakes = Arc::new(AtomicU64::new(0));
    let observed = wakes.clone();
    let deferred = deferred_connection(Arc::new(move || {
        observed.fetch_add(1, Ordering::Relaxed);
    }));
    let handle = deferred.handle.clone();
    handle
        .send(Action::Draft("before activation".into()))
        .unwrap();
    assert_eq!(wakes.load(Ordering::Relaxed), 1);
    assert!(
        matches!(deferred.receiver.try_recv().unwrap(), Action::Draft(text) if text == "before activation")
    );
    deferred.thread.set(std::thread::current()).unwrap();
    handle
        .send(Action::Draft("after activation".into()))
        .unwrap();
    assert_eq!(wakes.load(Ordering::Relaxed), 1);
    assert!(
        matches!(deferred.receiver.try_recv().unwrap(), Action::Draft(text) if text == "after activation")
    );
    // Consume the actor wake token so this test does not perturb later tests.
    std::thread::park_timeout(std::time::Duration::ZERO);
}
#[test]
fn abandoned_deferred_connection_reports_failed_archive_without_startup() {
    let deferred = deferred_connection(Arc::new(|| {}));
    let handle = deferred.handle.clone();
    let (done, outcome) = handle.stop_archiving();
    deferred.reject_pending("recovery unavailable".into());
    done.recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    assert_eq!(outcome.recv().unwrap(), Err("recovery unavailable".into()));
    assert!(handle.send(Action::Draft("closed".into())).is_err());
}
#[test]
fn stopping_deferred_codex_and_claude_preserves_data_without_startup() {
    for provider in [Provider::Codex, Provider::Claude] {
        let key = format!(
            "test-deferred-stop-{}-{}",
            if provider == Provider::Codex {
                "codex"
            } else {
                "claude"
            },
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let mut saved = SavedSession::empty(
            key.clone(),
            provider,
            "test".into(),
            "/missing-grove-fixture-directory".into(),
        );
        saved.draft = "preserved before activation".into();
        saved
            .items
            .push(json!({"id":"retained","type":"agentMessage","text":"retained history"}));
        let deferred = deferred_connection(Arc::new(|| {}));
        let done = deferred.handle.stop_completion(false);
        let connection = crate::codex::connect_deferred(
            saved,
            vec!["--unsupported-grove-fixture-option".into()],
            deferred,
        );
        done.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let snapshot = connection.updates.try_recv().unwrap();
        assert!(
            snapshot.error.is_none(),
            "startup must be skipped: {:?}",
            snapshot.error
        );
        let path = recovery_dir().unwrap().join(format!("{key}.json"));
        let disk = load_saved_session(&path).unwrap();
        assert_eq!(disk.draft, "preserved before activation");
        assert_eq!(disk.items[0]["text"], "retained history");
        persistence::remove(&path).unwrap();
    }
}
fn checked_snapshot(connection: &Connection, predicate: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while Instant::now() < deadline {
        while let Ok(snapshot) = connection.updates.try_recv() {
            if predicate(&snapshot) {
                return snapshot;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("checked startup snapshot did not arrive");
}

#[test]
fn checked_startup_codex_and_claude_reject_without_cli_and_revalidate_reconnect() {
    for provider in [Provider::Codex, Provider::Claude] {
        let key = format!(
            "checked-{}-{}",
            if provider == Provider::Codex {
                "codex"
            } else {
                "claude"
            },
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let mut saved = SavedSession::empty(
            key.clone(),
            provider,
            "test".into(),
            "/missing-checked-fixture".into(),
        );
        saved.draft = "retained draft".into();
        saved.title = Some("retained title".into());
        saved.items.push(json!({"id":"retained-user","type":"userMessage","content":[{"type":"text","text":"existing prompt"}]}));
        let attempts = Arc::new(AtomicU64::new(0));
        let seen = attempts.clone();
        let (worker, observed) = mpsc::channel();
        let validation = Arc::new(move || {
            let attempt = seen.fetch_add(1, Ordering::Relaxed) + 1;
            let _ = worker.send(std::thread::current().id());
            Err(format!("fresh checkout validation {attempt}"))
        });
        let connection = crate::codex::connect_checked(
            saved,
            vec!["--unsupported-grove-test-option".into()],
            validation,
        );
        assert_ne!(
            observed
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            std::thread::current().id(),
            "validation must run off the caller thread"
        );
        let id = connection
            .handle
            .submit("never replay this request".into(), Vec::new())
            .unwrap();
        let failed = checked_snapshot(&connection, |snapshot| {
            snapshot.rejected_submission == Some(id)
        });
        // Startup rejection retains the provider's disconnected/reconnect contract.
        assert_eq!(failed.status, Status::Disconnected);
        assert_eq!(failed.saved.draft, "retained draft");
        assert_eq!(failed.saved.title.as_deref(), Some("retained title"));
        assert_eq!(failed.saved.items.len(), 1);
        assert_eq!(failed.saved.items[0]["id"], "retained-user");
        assert!(
            failed.saved.title_generation.is_none(),
            "a failed guard must also suppress title CLI jobs"
        );
        connection
            .handle
            .send(Action::Draft("edited after failure".into()))
            .unwrap();
        connection.handle.send(Action::Reconnect).unwrap();
        observed
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let retried = checked_snapshot(&connection, |snapshot| {
            snapshot
                .error
                .as_deref()
                .is_some_and(|error| error.contains("validation 2"))
        });
        assert_eq!(retried.status, Status::Disconnected);
        assert_eq!(retried.saved.draft, "edited after failure");
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
        connection
            .handle
            .stop_completion(false)
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let path = recovery_dir().unwrap().join(format!("{key}.json"));
        assert_eq!(
            load_saved_session(&path).unwrap().draft,
            "edited after failure"
        );
        persistence::remove(&path).unwrap();
    }
}

#[test]
fn checked_startup_stop_during_validation_never_spawns_codex_or_claude() {
    for provider in [Provider::Codex, Provider::Claude] {
        let key = format!(
            "checked-stop-{}-{}",
            if provider == Provider::Codex {
                "codex"
            } else {
                "claude"
            },
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let saved = SavedSession::empty(
            key.clone(),
            provider,
            "test".into(),
            "/missing-checked-fixture".into(),
        );
        let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let gate = release.clone();
        let (started, entered) = mpsc::channel();
        let validation = Arc::new(move || {
            let _ = started.send(());
            let mut ready = gate.0.lock().unwrap();
            while !*ready {
                ready = gate.1.wait(ready).unwrap();
            }
            Ok(())
        });
        let connection = crate::codex::connect_checked(
            saved,
            vec!["--unsupported-grove-test-option".into()],
            validation,
        );
        entered
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let done = connection.handle.stop_completion(false);
        *release.0.lock().unwrap() = true;
        release.1.notify_one();
        done.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let stopped = connection.updates.try_recv().unwrap();
        assert!(stopped
            .error
            .as_deref()
            .is_some_and(|error| error.contains("closing") || error.contains("stopping")));
        let path = recovery_dir().unwrap().join(format!("{key}.json"));
        assert!(load_saved_session(&path).is_ok());
        persistence::remove(&path).unwrap();
    }
}

#[test]
fn text_append_retains_unicode_and_empty_prefix_behavior() {
    let mut value = json!(null);
    append_json_text(&mut value, "工具 ");
    append_json_text(&mut value, "🦀");
    assert_eq!(value, json!("工具 🦀"));
    append_json_text(&mut value, "");
    assert_eq!(value, json!("工具 🦀"));
}
#[test]
#[ignore = "manual synthetic JSON streaming append baseline"]
fn json_stream_append_benchmark() {
    let delta = "工具输出 🦀 ".repeat(4);
    for sample in 0..3 {
        let mut legacy = json!("");
        let start = Instant::now();
        for _ in 0..4096 {
            legacy = json!(format!("{}{}", legacy.as_str().unwrap(), delta));
        }
        let legacy_ms = start.elapsed().as_secs_f64() * 1000.0;
        let mut value = json!("");
        let start = Instant::now();
        for _ in 0..4096 {
            append_json_text(&mut value, &delta);
        }
        let append_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(legacy, value);
        println!("json_stream_append sample={sample} legacy_ms={legacy_ms:.3} append_ms={append_ms:.3} bytes={}", value.as_str().unwrap().len());
    }
}

#[test]
fn new_session_preserves_preferences_and_clears_conversation_ownership() {
    for provider in [Provider::Codex, Provider::Claude] {
        let mut source = SavedSession::empty(
            "source".into(),
            provider,
            "project".into(),
            "/checkout".into(),
        );
        source.thread_id = Some("old-thread".into());
        source.composer.model = Some("selected-model".into());
        source.composer.effort = Some("medium".into());
        source.config_args = vec!["permission-settings".into()];
        source.draft = "untouched original draft".into();
        source.items.push(json!({"id":"old","type":"userMessage","content":[{"type":"text","text":"private old conversation"}]}));
        source.session_review.summary = "old receipt".into();
        let fresh = source.independent_conversation("new-session".into());
        assert_eq!(fresh.key, "new-session");
        assert_eq!(fresh.provider, provider);
        assert_eq!(fresh.composer.model, source.composer.model);
        assert_eq!(fresh.config_args, source.config_args);
        assert!(fresh.thread_id.is_none());
        assert!(fresh.items.is_empty());
        assert!(fresh.draft.is_empty());
        assert!(fresh.session_review.summary.is_empty());
        assert_eq!(source.draft, "untouched original draft");
        assert_eq!(source.items.len(), 1);
    }
}
