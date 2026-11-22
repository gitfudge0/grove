mod composition;
mod configuration;
mod interaction;
mod plan_delivery;
mod recovery;
mod session;
mod subagents;
mod transport;

use super::*;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

fn saved() -> SavedSession {
    SavedSession {
        html_policy: crate::visuals::HtmlPolicy::Legacy,
        visuals: Vec::new(),
        session_review: SessionReview::default(),
        claude_subagents: HashMap::default(),
        rich_plans: crate::rich_plan::PlanStore::default(),
        side_chats: std::collections::HashMap::default(),
        fork_origin: None,
        claude_resume: None,
        edit_checkpoints: Vec::new(),
        pending_edit: None,
        rewind_drafts: Vec::new(),
        provider: Provider::Codex,
        key: "test-native-1".into(),
        project: "Project".into(),
        cwd: "/unused".into(),
        label: "Codex".into(),
        title: None,
        title_generation: None,
        roots: Vec::new(),
        thread_id: Some("thread-1".into()),
        items: Vec::new().into(),
        work_periods: Vec::new(),
        draft: String::new(),
        draft_images: Vec::new(),
        draft_reference: None,
        config_args: Vec::new(),
        native_config_version: 1,
        activity_reporting: false,
        composer: ComposerSettings::default(),
        context: None,
        context_details: None,
    }
}

/// Real-session clone/update CPU timings; excludes rendering, I/O and provider latency.

/// Compares the previous deep-Vec item copy with an existing prose-row edit.
/// Measures transcript item storage only, excluding other Snapshot metadata.

/// Includes full recovery-file serialization, atomic replacement and durability syncs.

fn reference(id: &str) -> ReferenceDraft {
    ReferenceDraft {
        id: id.into(),
        thread_id: Some("thread-1".into()),
        sources: vec![ReferenceSource {
            message_id: "message-1".into(),
            role: "assistant".into(),
        }],
        text: "/compact $skill literal\n```rust\n<User reply:>\n```".into(),
    }
}

fn state() -> Snapshot {
    let mut state = Snapshot::new(saved());
    state.status = Status::Working;
    state.turn_id = Some("turn-1".into());
    state
}

fn actor() -> Actor {
    let (_, actions) = mpsc::channel();
    let (updates, _) = crate::latest::channel();
    let mut actor = Actor::new(saved(), actions, updates, Vec::new());
    actor.state = state();
    actor
}

#[test]
#[cfg(unix)]
fn turn_start_admission_refuses_auth_before_writing() {
    // Process isolation keeps this actual global reservation from affecting
    // unrelated concurrently running provider fixture tests.
    const CHILD: &str = "GROVE_TEST_AUTH_ADMISSION_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "codex::tests::turn_start_admission_refuses_auth_before_writing",
                "--exact",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            String::from_utf8_lossy(&result.stdout).contains("1 passed; 0 failed"),
            "isolated admission fixture must execute one test"
        );
        return;
    }
    let reservation = crate::connections::reserve_named_for_test("Codex").unwrap();
    let mut actor = actor();
    actor.process = Some(fixture_process());
    let seq = actor.seq;
    let pending = actor.pending.len();
    let result = actor.request(
        "turn/start",
        json!({"threadId":"fixture","input":[]}),
        Pending::InlineTurn,
    );
    assert!(result.is_err());
    assert_eq!(actor.seq, seq);
    assert_eq!(actor.pending.len(), pending);
    actor
        .request(
            "thread/read",
            json!({"threadId":"fixture"}),
            Pending::Compact,
        )
        .unwrap();
    assert_eq!(actor.seq, seq + 1);
    drop(reservation);
    actor
        .request(
            "turn/start",
            json!({"threadId":"fixture","input":[]}),
            Pending::InlineTurn,
        )
        .unwrap();
    assert!(
        crate::connections::reserve_named_for_test("Codex").is_err(),
        "admitted model work must block credential changes before its UI snapshot arrives"
    );
    actor.state.status = Status::Idle;
    actor.state.turn_id = None;
    actor.pending.clear();
    actor.emit();
    assert!(
        crate::connections::reserve_named_for_test("Codex").is_ok(),
        "authoritative idle releases model admission"
    );
    actor
        .request(
            "turn/start",
            json!({"threadId":"fixture","input":[]}),
            Pending::InlineTurn,
        )
        .unwrap();
    actor.process.take();
    actor.emit();
    assert!(
        crate::connections::reserve_named_for_test("Codex").is_ok(),
        "owned process cleanup releases model admission"
    );
}

#[cfg(unix)]
fn fixture_process() -> Process {
    use std::os::unix::process::CommandExt;
    let mut child = Command::new("sh")
        .args(["-c", "cat >/dev/null"])
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let input = child.stdin.take().unwrap();
    let (_, lines) = mpsc::channel();
    Process {
        activity: crate::activity_mcp::Server::start().unwrap(),
        visuals: crate::visuals_mcp::Server::start().unwrap(),
        child,
        input,
        lines,
        capture: crate::provider_diagnostics::Capture::default(),
        ownership: None,
    }
}

fn async_question_message() -> Value {
    // Recorded request_user_input_async AgentMessage, exposed by app-server as
    // camelCase ThreadItem metadata (Codex 0.163.0 schema).
    json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{
        "type":"agentMessage","id":"async-question","text":"Which changes?\n- All\n- Composer",
        "phase":"final_answer","delivery":"async","questions":[
            {"title":"Which changes?","options":["All","Composer"]},
            {"title":"Any details?","options":null}
        ]
    }}})
}

fn usage(turn: &str, used: u64, window: Option<u64>) -> Value {
    json!({"method":"thread/tokenUsage/updated","params":{"threadId":"thread-1","turnId":turn,
        "tokenUsage":{"total":{"totalTokens":used * 3,"inputTokens":used * 3,"outputTokens":0},
        "last":{"totalTokens":used,"inputTokens":used - 5,"cachedInputTokens":0,"outputTokens":5,
        "reasoningOutputTokens":0,"cacheWriteInputTokens":0},"modelContextWindow":window}}})
}
fn idle_actor() -> (Actor, BufReader<std::process::ChildStdout>) {
    use std::os::unix::process::CommandExt;
    let mut actor = actor();
    actor.state.status = Status::Done;
    actor.state.turn_id = None;
    actor.state.saved.items =
        vec![json!({"id":"u1","type":"userMessage","content":[{"type":"text","text":"hi"}]})]
            .into();
    let mut child = Command::new("cat")
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let reader = BufReader::new(child.stdout.take().unwrap());
    let input = child.stdin.take().unwrap();
    let (_, lines) = mpsc::channel();
    actor.process = Some(Process {
        activity: crate::activity_mcp::Server::start().unwrap(),
        visuals: crate::visuals_mcp::Server::start().unwrap(),
        child,
        input,
        lines,
        capture: crate::provider_diagnostics::Capture::default(),
        ownership: None,
    });
    (actor, reader)
}
fn read_line(reader: &mut impl BufRead) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

#[cfg(unix)]
struct EditFixture {
    actor: Actor,
    repo: tempfile::TempDir,
    journal: PathBuf,
    // Reads/writes use the process-wide config override. Keep it stable
    // against storage/session-meta environment tests for this fixture.
    _config_lock: std::sync::MutexGuard<'static, ()>,
}
#[cfg(unix)]
impl Drop for EditFixture {
    fn drop(&mut self) {
        let _ = fs_err::remove_file(&self.journal);
    }
}
#[cfg(unix)]
fn edit_fixture() -> EditFixture {
    let config_lock = crate::storage::tests::CONFIG_DIR_ENV_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let repo = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .arg(repo.path())
        .status()
        .unwrap()
        .success());
    let root = repo.path().canonicalize().unwrap();
    let file = root.join("work");
    let mut actor = actor();
    actor.process = Some(fixture_process());
    actor.state.status = Status::Idle;
    actor.state.turn_id = None;
    actor.state.edit_supported = true;
    actor.state.saved.cwd = root.to_string_lossy().into_owned();
    actor.state.saved.key = format!(
        "test-edit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let journal = recovery_dir()
        .unwrap()
        .join(format!("{}.json", actor.state.saved.key));
    for (start, turn, text) in [
        (0, "earlier-turn", "initial"),
        (2, "target-turn", "before target"),
        (4, "later-turn", "after target"),
    ] {
        fs_err::write(&file, text).unwrap();
        actor.state.saved.edit_checkpoints.push(EditCheckpoint {
            start_item: start,
            turn_id: Some(turn.into()),
            files: Some(crate::checkpoint::capture(std::slice::from_ref(&root)).unwrap()),
        });
    }
    fs_err::write(&file, "latest workspace").unwrap();
    actor.state.saved.items = vec![
        json!({"id":"earlier","type":"userMessage","content":[{"type":"text","text":"earlier prompt"}]}),
        json!({"id":"earlier-answer","type":"agentMessage","text":"earlier answer"}),
        json!({"id":"target","type":"userMessage","content":[{"type":"text","text":"target prompt"},{"type":"image","url":"data:image/png;base64,aGVsbG8="}]}),
        json!({"id":"target-answer","type":"agentMessage","text":"target answer"}),
        json!({"id":"later","type":"userMessage","content":[{"type":"text","text":"later prompt"}]}),
    ].into();
    EditFixture {
        actor,
        repo,
        journal,
        _config_lock: config_lock,
    }
}
#[cfg(unix)]
fn edit_pending(actor: &Actor) -> u64 {
    *actor.pending.keys().next().unwrap()
}
#[cfg(unix)]
fn edit_disk(fixture: &EditFixture) -> SavedSession {
    load_saved_session(&fixture.journal).unwrap()
}
