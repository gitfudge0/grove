mod composition;
mod configuration;
mod interaction;
mod plan_delivery;
mod recovery;
mod session;
mod subagents;
mod transport;

use super::*;
use super::{context::context_measurement, permissions::permission_modes_from_help};
use crate::provider::{SubagentStatus, SubmissionMode};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

fn actor() -> Actor {
    let saved:SavedSession=serde_json::from_value(json!({"key":"claude-test","provider":"claude","project":"test","cwd":"/tmp","label":"Claude","roots":[],"thread_id":null,"items":[]})).unwrap();
    actor_with_saved(saved)
}
fn actor_with_saved(saved: SavedSession) -> Actor {
    let (_, input) = mpsc::channel();
    let (updates, _) = crate::latest::channel();
    Actor::new(
        saved,
        Vec::new(),
        input,
        updates,
        Arc::new(AtomicU32::new(0)),
        Arc::new(AtomicBool::new(false)),
    )
}

#[test]
#[cfg(unix)]
fn user_message_admission_holds_lease_until_authoritative_idle() {
    const CHILD: &str = "GROVE_TEST_CLAUDE_AUTH_LEASE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "claude::tests::user_message_admission_holds_lease_until_authoritative_idle",
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
        assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed; 0 failed"));
        return;
    }
    let reservation = crate::connections::reserve_named_for_test("Claude Code").unwrap();
    let mut actor = actor();
    let mut reader = transport(&mut actor);
    assert!(actor
        .write(json!({"type":"user","message":{"content":"blocked"}}))
        .is_err());
    drop(reservation);
    actor
        .write(json!({"type":"user","message":{"content":"allowed"}}))
        .unwrap();
    assert_eq!(read(&mut reader)["message"]["content"], "allowed");
    assert!(
        crate::connections::reserve_named_for_test("Claude Code").is_err(),
        "wire admission blocks auth before any published UI snapshot"
    );
    actor.state.status = Status::Done;
    actor.state.turn_id = None;
    actor.tracks_session_state = true;
    actor.session_idle = false;
    actor.publish();
    assert!(
        crate::connections::reserve_named_for_test("Claude Code").is_err(),
        "a root result does not replace authoritative session idle"
    );
    actor.session_idle = true;
    actor.publish();
    assert!(crate::connections::reserve_named_for_test("Claude Code").is_ok());
    actor
        .write(json!({"type":"user","message":{"content":"another"}}))
        .unwrap();
    assert!(crate::connections::reserve_named_for_test("Claude Code").is_err());
    actor.process.take();
    actor.publish();
    assert!(
        crate::connections::reserve_named_for_test("Claude Code").is_ok(),
        "reaped process releases admission"
    );
}

/// Starts a Claude turn that has a running Bash tool, then steers it.
fn steered_turn(reader: &mut BufReader<std::process::ChildStdout>, actor: &mut Actor) -> Value {
    actor.initialized = true;
    actor.state.status = Status::Idle;
    actor.submit(1, "build it".into(), vec![]).unwrap();
    let prompt = read(reader);
    actor
        .message(json!({"type":"user","uuid":prompt["uuid"],"message":{"role":"user","content":"build it"}}))
        .unwrap();
    actor.message(json!({"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"bash","name":"Bash","input":{"command":"sleep 5"}}]}})).unwrap();
    actor
        .action(Action::SubmitWithMode {
            id: 2,
            text: "use the other branch".into(),
            images: vec![],
            mode: SubmissionMode::Steer,
        })
        .unwrap();
    let steer = read(reader);
    assert_eq!(steer["priority"], "next");
    assert!(actor.state.status.busy());
    steer
}

fn steer_acknowledged(actor: &Actor) -> bool {
    actor.in_flight.is_none()
        && actor.state.accepted_submission == Some(2)
        && actor.state.saved.items.iter().any(|item| {
            item["type"] == "userMessage" && provider::item_text(item) == "use the other branch"
        })
}

fn assert_steer_turn_finishes(actor: &mut Actor) {
    actor.message(json!({"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"Switched."}]}})).unwrap();
    assert!(actor.state.status.busy());
    actor.message(json!({"type":"result","subtype":"success","terminal_reason":"completed","is_error":false,"queued_turn_count":0})).unwrap();
    assert_eq!(actor.state.status, Status::Done);
    assert!(actor.state.turn_id.is_none());
    assert!(actor.state.error.is_none());
    assert!(actor.state.rejected_submission.is_none());
    assert!(!actor.redirect_pending);
}

fn transport(actor: &mut Actor) -> BufReader<std::process::ChildStdout> {
    let mut command = Command::new("cat");
    command.stdin(Stdio::piped()).stdout(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().unwrap();
    let input = child.stdin.take().unwrap();
    let output = BufReader::new(child.stdout.take().unwrap());
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
    output
}
#[test]

fn activity_reply_queue_and_steer_preserve_identical_main_draft() {
    let mut actor = actor();
    let _reader = transport(&mut actor);
    actor.initialized = true;
    actor.state.status = Status::Working;
    actor.state.turn_id = Some("busy".into());
    actor.state.saved.draft = "same text".into();
    actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
    actor.state.saved.draft_reference = Some(provider::ReferenceDraft {
        id: "reference".into(),
        thread_id: Some("source-thread".into()),
        sources: vec![provider::ReferenceSource {
            message_id: "source-message".into(),
            role: "assistant".into(),
        }],
        text: "quoted selection".into(),
    });
    let original_reference = actor.state.saved.draft_reference.clone();
    actor
        .action(Action::ActivityPrompt {
            id: 41,
            text: "same text".into(),
            mode: SubmissionMode::Queue,
            expected_turn: Some("busy".into()),
        })
        .unwrap();
    assert_eq!(actor.state.accepted_submission, Some(41));
    assert_eq!(actor.state.saved.draft, "same text");
    actor
        .action(Action::ActivityPrompt {
            id: 42,
            text: "same text".into(),
            mode: SubmissionMode::Steer,
            expected_turn: Some("busy".into()),
        })
        .unwrap();
    let uuid = actor.input_uuid.clone().unwrap();
    actor.acknowledge_input(&json!(uuid));
    assert_eq!(actor.state.accepted_submission, Some(42));
    assert_eq!(actor.state.saved.draft, "same text");
    assert_eq!(actor.state.saved.draft_images.len(), 1);
    assert_eq!(actor.state.saved.draft_reference, original_reference);
    assert!(!actor.state.queued_prompts.iter().any(|p| p.id == 42));
}

fn read(reader: &mut impl BufRead) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn rich_context_fixture() -> Value {
    json!({"totalTokens":128000,"maxTokens":200000,"model":"claude-test", "isAutoCompactEnabled":true,
        "categories":[
            {"id":"messages","name":"Messages","tokens":94000,"kind":"used"},
            {"id":"memory_files","name":"Memory files","tokens":6000,"kind":"used"},
            {"id":"mcp_tools","name":"MCP tools","tokens":14000,"kind":"used"},
            {"id":"skills","name":"Skills","tokens":2000,"kind":"used"},
            {"id":"custom_agents","name":"Agents","tokens":12000,"kind":"used"},
            {"id":"free_space","name":"Free space","tokens":62000,"kind":"free"},
            {"id":"autocompact_buffer","name":"Auto-compaction buffer","tokens":10000,"kind":"buffer"},
            {"id":"future_tools","name":"Future tools","tokens":5000,"kind":"deferred"}],
        "memoryFiles":[{"path":"CLAUDE.md","tokens":6000}],
        "mcpTools":[{"name":"search","serverName":"github","tokens":14000}],
        "skills":{"skillFrontmatter":[{"name":"grove-ui","tokens":2000}]},
        "agents":[{"agentType":"reviewer","tokens":12000}],
        "messageBreakdown":{"toolResultTokens":68000,"assistantMessageTokens":20000,"userMessageTokens":6000,
            "toolCallsByType":[{"name":"Read","callTokens":1000,"resultTokens":67000}],
            "attachmentsByType":[{"name":"image","tokens":20}]}})
}
