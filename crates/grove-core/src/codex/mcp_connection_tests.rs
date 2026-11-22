use super::*;

#[test]
fn mcp_startup_failure_is_visible_without_failing_the_conversation() {
    let mut state = Snapshot::new(serde_json::from_value(json!({"key":"mcp-test","project":"test","cwd":"/tmp","label":"MCP","roots":[],"thread_id":"mcp-thread","items":[]})).unwrap());
    state.status = Status::Idle;
    let event = json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"mcp-thread","name":"jira","status":"failed","error":"handshake timed out","failureReason":"reauthenticationRequired"}});
    assert!(state.apply(&event));
    assert_eq!(state.status, Status::Idle);
    assert!(state.error.is_none());
    let warning = state.saved.items.last().unwrap();
    assert_eq!(warning["type"], "providerNotice");
    assert!(warning["text"]
        .as_str()
        .unwrap()
        .contains("handshake timed out"));
    assert!(warning["text"].as_str().unwrap().contains("Authenticate"));
    let count = state.saved.items.len();
    state
        .saved
        .items
        .push(json!({"type":"agentMessage","text":"still working"}));
    state.apply(&event);
    assert_eq!(state.saved.items.len(), count + 1);
    let mut foreign = event.clone();
    foreign["params"]["threadId"] = json!("other-thread");
    foreign["params"]["name"] = json!("foreign");
    assert!(!state.apply(&foreign));
    assert_eq!(state.saved.items.len(), count + 1);
}
