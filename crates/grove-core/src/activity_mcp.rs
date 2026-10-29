//! Session-local reporting endpoint. It accepts metadata only and performs no
//! file, process, network, or permission actions on behalf of an agent.
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const HEADER_LIMIT: usize = 8 * 1024;
const BODY_LIMIT: usize = 16 * 1024;
const TASK_LIMIT: usize = 512;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

struct Recorded {
    task: Option<crate::tasks::Task>,
    status: crate::activity::StepStatus,
    _label: String,
}
#[derive(Default)]
struct Context {
    tasks: Vec<crate::tasks::Task>,
    turn_id: Option<String>,
    snapshot_id: Option<String>,
    overflow: bool,
    reports: HashMap<String, Recorded>,
}

pub(crate) struct Server {
    address: SocketAddr,
    token: String,
    stopping: Arc<AtomicBool>,
    tasks: Arc<Mutex<Context>>,
    thread: Option<JoinHandle<()>>,
    #[cfg(test)]
    last_error: Arc<Mutex<Option<String>>>,
}

impl Server {
    pub(crate) fn start() -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let token = crate::identity::random_id()?;
        let auth = format!("Bearer {token}");
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = stopping.clone();
        let tasks = Arc::new(Mutex::new(Context::default()));
        let context = tasks.clone();
        #[cfg(test)]
        let last_error = Arc::new(Mutex::new(None));
        #[cfg(test)]
        let diagnostic = last_error.clone();
        let thread = std::thread::Builder::new()
            .name("grove-activity-mcp".into())
            .spawn(move || {
                // One bounded request at a time: clients cannot create an unbounded
                // thread pool or keep a connection alive past its absolute deadline.
                while !stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, peer)) if peer.ip().is_loopback() => {
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            if let Err(error) = serve(&mut stream, address, &auth, &context) {
                                tracing::debug!(kind = ?error.kind(), "Activity MCP request ended before a response");
                                #[cfg(test)]
                                if let Ok(mut last) = diagnostic.lock() { *last = Some(format!("{:?}: {error}",error.kind())); }
                            }
                        }
                        Ok(_) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            address,
            token,
            stopping,
            tasks,
            thread: Some(thread),
            #[cfg(test)]
            last_error,
        })
    }
    pub(crate) fn set_context_items(
        &self,
        items: &[crate::codex::TranscriptItem],
        turn_id: Option<String>,
    ) {
        let list = crate::tasks::current(items);
        let snapshot_id = list
            .as_ref()
            .and_then(|list| items.get(list.item))
            .and_then(|item| item["id"].as_str())
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        let changed = self
            .tasks
            .lock()
            .is_ok_and(|context| context.turn_id != turn_id);
        let seed = if changed && turn_id.is_some() {
            crate::activity::steps(items)
                .into_iter()
                .filter(|step| step.turn_id.as_deref() == turn_id.as_deref())
                .take(TASK_LIMIT)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        self.update_context(list, snapshot_id, turn_id, seed);
    }
    #[cfg(test)]
    fn set_context(&self, list: Option<crate::tasks::TaskList>, turn_id: Option<String>) {
        self.update_context(
            list,
            Some("socket-test-snapshot".into()),
            turn_id,
            Vec::new(),
        );
    }
    fn update_context(
        &self,
        list: Option<crate::tasks::TaskList>,
        snapshot_id: Option<String>,
        turn_id: Option<String>,
        seed: Vec<crate::activity::Step>,
    ) {
        if let Ok(mut context) = self.tasks.lock() {
            if context.turn_id != turn_id {
                context.reports.clear();
                for step in seed {
                    context.reports.insert(
                        step.id,
                        Recorded {
                            task: step.task,
                            status: step.status,
                            _label: step.label,
                        },
                    );
                }
            }
            context.overflow = list
                .as_ref()
                .is_some_and(|list| list.tasks.len() > TASK_LIMIT);
            context.tasks = list.map_or_else(Vec::new, |list| {
                list.tasks.into_iter().take(TASK_LIMIT).collect()
            });
            context.turn_id = turn_id;
            context.snapshot_id = snapshot_id;
        }
    }
    pub(crate) fn url(&self) -> String {
        format!("http://{}/mcp", self.address)
    }
    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(50));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn response(stream: &mut TcpStream, status: &str, body: Option<Value>) -> std::io::Result<()> {
    let body = body.map_or_else(String::new, |body| body.to_string());
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}", body.len())
}
fn bad(stream: &mut TcpStream, status: &str) -> std::io::Result<()> {
    response(stream, status, None)?;
    stream.flush()?;
    // Closing a socket with an unread request body can reset TCP and discard
    // the HTTP error already written. Half-close first, then drain a bounded
    // amount so ordinary rejected SDK requests reliably receive their status.
    stream.shutdown(std::net::Shutdown::Write)?;
    stream.set_read_timeout(Some(Duration::from_millis(20)))?;
    let deadline = Instant::now() + Duration::from_millis(50);
    let mut remaining = BODY_LIMIT + HEADER_LIMIT;
    let mut buffer = [0; 1024];
    while remaining > 0 && Instant::now() < deadline {
        let size = remaining.min(buffer.len());
        match stream.read(&mut buffer[..size]) {
            Ok(0) | Err(_) => break,
            Ok(count) => remaining -= count,
        }
    }
    Ok(())
}
fn serve(
    stream: &mut TcpStream,
    address: SocketAddr,
    auth: &str,
    tasks: &Mutex<Context>,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    stream.set_write_timeout(Some(REQUEST_TIMEOUT))?;
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut request = Vec::new();
    let header_end = loop {
        if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
            break end + 4;
        }
        if request.len() >= HEADER_LIMIT {
            return bad(stream, "431 Request Header Fields Too Large");
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return bad(stream, "408 Request Timeout");
        }
        stream.set_read_timeout(Some(remaining))?;
        // Read a bounded chunk rather than one byte per syscall. Preserve body
        // bytes coalesced with the header; the absolute request deadline and
        // separate header/body limits still apply.
        let mut chunk = [0; 1024];
        let capacity = chunk.len().min(HEADER_LIMIT - request.len());
        let count = stream.read(&mut chunk[..capacity])?;
        if count == 0 {
            return bad(stream, "400 Bad Request");
        }
        request.extend_from_slice(&chunk[..count]);
    };
    let Ok(headers) = std::str::from_utf8(&request[..header_end]) else {
        return bad(stream, "400 Bad Request");
    };
    let mut lines = headers.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let mut first = first.split(' ');
    let method = first.next().unwrap_or_default();
    let path = first.next().unwrap_or_default();
    let version = first.next().unwrap_or_default();
    if first.next().is_some() || !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        return bad(stream, "400 Bad Request");
    }
    let mut length = None;
    let mut authorization = None;
    let mut host = None;
    let mut origin = false;
    let mut content_type = None;
    for line in lines.filter(|line| !line.is_empty()) {
        let Some((name, value)) = line.split_once(':') else {
            return bad(stream, "400 Bad Request");
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return bad(stream, "400 Bad Request");
            }
            length = value.parse::<usize>().ok();
            if length.is_none() {
                return bad(stream, "400 Bad Request");
            }
        } else if name.eq_ignore_ascii_case("authorization") {
            if authorization.is_some() {
                return bad(stream, "400 Bad Request");
            }
            authorization = Some(value);
        } else if name.eq_ignore_ascii_case("host") {
            if host.is_some() {
                return bad(stream, "400 Bad Request");
            }
            host = Some(value);
        } else if name.eq_ignore_ascii_case("origin") {
            origin = true;
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            return bad(stream, "400 Bad Request");
        } else if name.eq_ignore_ascii_case("content-type") {
            content_type = Some(value);
        }
    }
    // Strict host and no browser Origin stop DNS rebinding/browser callers;
    // the unpersisted bearer token isolates simultaneously running sessions.
    if host != Some(address.to_string().as_str()) || origin || authorization != Some(auth) {
        return bad(stream, "403 Forbidden");
    }
    if path != "/mcp" {
        return bad(stream, "404 Not Found");
    }
    if method != "POST" {
        return bad(stream, "405 Method Not Allowed");
    }
    if !content_type.is_some_and(|value| {
        value
            .split(';')
            .next()
            .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))
    }) {
        return bad(stream, "415 Unsupported Media Type");
    }
    let Some(length) = length else {
        return bad(stream, "411 Length Required");
    };
    if length > BODY_LIMIT {
        return bad(stream, "413 Content Too Large");
    }
    let mut offset = request.len().min(header_end + length);
    request.resize(header_end + length, 0);
    while offset < request.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return bad(stream, "408 Request Timeout");
        }
        stream.set_read_timeout(Some(remaining))?;
        let count = stream.read(&mut request[offset..])?;
        if count == 0 {
            return bad(stream, "400 Bad Request");
        }
        offset += count;
    }
    let Ok(message) = serde_json::from_slice::<Value>(&request[header_end..]) else {
        return bad(stream, "400 Bad Request");
    };
    let Some(method) = message["method"].as_str() else {
        return bad(stream, "400 Bad Request");
    };
    if message["jsonrpc"] != "2.0" {
        return bad(stream, "400 Bad Request");
    }
    let Some(id) = message.get("id") else {
        return response(stream, "202 Accepted", None);
    };
    let result = match method {
        "initialize" => {
            let version = match message["params"]["protocolVersion"].as_str() {
                Some("2024-11-05") => "2024-11-05",
                Some("2025-03-26") => "2025-03-26",
                _ => "2025-06-18",
            };
            json!({"protocolVersion":version,"capabilities":{"tools":{}},"serverInfo":{"name":"grove_activity","version":"1.0.0"}})
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools":[{
            "name":"report_activity",
            "description":"Report one concise meaningful action under a Grove task, separately from conversational commentary. Use a stable step id and the exact task id (or unique exact title) from your structured checklist; task_id may be empty only when no checklist exists. Report in_progress before work, completed only after verified success, failed on failure. Reuse the step id to update it. Current labels use present continuous (Running navigation checks); completed labels use past tense (Verified navigation behavior). No commands, paths, meta commentary or invented completion.",
            "inputSchema":{"type":"object","additionalProperties":false,"properties":{
                "id":{"type":"string","minLength":1,"maxLength":128},
                "task_id":{"type":"string","maxLength":1024},
                "status":{"type":"string","enum":["in_progress","completed","failed"]},
                "label":{"type":"string","minLength":1,"maxLength":100}
            },"required":["id","task_id","status","label"]},
            "annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}
        }]}),
        "tools/call" => {
            if message["params"]["name"] != "report_activity" {
                return response(
                    stream,
                    "200 OK",
                    Some(
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Unknown tool"}}),
                    ),
                );
            }
            let arguments = &message["params"]["arguments"];
            match crate::activity::validate_report(arguments)
                .and_then(|()| validate_task(arguments, tasks))
            {
                Ok((turn_id, owner)) => {
                    let accepted =
                        json!({"accepted":true,"report":arguments,"turn_id":turn_id,"owner":owner});
                    json!({"content":[{"type":"text","text":accepted.to_string()}],"structuredContent":accepted,"isError":false})
                }
                Err(error) => json!({"content":[{"type":"text","text":error}],"isError":true}),
            }
        }
        _ => {
            return response(
                stream,
                "200 OK",
                Some(
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}}),
                ),
            )
        }
    };
    response(
        stream,
        "200 OK",
        Some(json!({"jsonrpc":"2.0","id":id,"result":result})),
    )
}

fn validate_task(arguments: &Value, catalog: &Mutex<Context>) -> Result<(String, Value), String> {
    let mut context = catalog
        .lock()
        .map_err(|_| "Task context is unavailable; retry the report".to_owned())?;
    let turn_id = context
        .turn_id
        .clone()
        .ok_or("No live provider turn; this activity report has expired")?;
    if context.overflow {
        return Err(
            "The checklist exceeds the activity catalog limit; reduce it before reporting steps"
                .into(),
        );
    }
    let report: crate::activity::Report =
        serde_json::from_value(arguments.clone()).map_err(|error| error.to_string())?;
    let mut matches = context
        .tasks
        .iter()
        .filter(|task| task.id == report.task_id || task.step == report.task_id);
    let first = matches.next().cloned();
    let valid = (context.tasks.is_empty() && report.task_id.is_empty())
        || (first.is_some() && matches.next().is_none());
    if !valid {
        let available = context
            .tasks
            .iter()
            .take(16)
            .map(|task| {
                let id: String = task.id.chars().take(128).collect();
                let title: String = task.step.chars().take(160).collect();
                format!("{id}: {title}")
            })
            .collect::<Vec<_>>()
            .join("; ");
        return Err(if context.tasks.is_empty() {
            "No task checklist exists. Use an empty task_id, or first create the checklist.".into()
        } else {
            format!("task_id must match one exact checklist id or unique exact title. Available tasks: {available}")
        });
    }
    let owner = if let Some(task) = &first {
        let snapshot_id = context.snapshot_id.as_ref().ok_or(
            "Checklist snapshot identity is unavailable; refresh task state before reporting",
        )?;
        json!({"snapshot_id":snapshot_id,"id":task.id,"step":task.step})
    } else {
        Value::Null
    };
    if let Some(previous) = context.reports.get(&report.id) {
        let same_owner = match (&previous.task, &first) {
            (Some(old), Some(new)) => new.owns_activity(old),
            (None, None) => true,
            _ => false,
        };
        if !same_owner {
            return Err(
                "This step id already belongs to another task; use a new id for a new action"
                    .into(),
            );
        }
        if !crate::activity::valid_transition(previous.status, report.status) {
            return Err("A completed or failed step cannot be reopened or change outcome; use a new id for a retry".into());
        }
    } else if context.reports.len() >= TASK_LIMIT {
        return Err("This turn has reached the activity step limit".into());
    }
    context.reports.insert(
        report.id,
        Recorded {
            task: first,
            status: report.status,
            _label: report.label,
        },
    );
    Ok((turn_id, owner))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn call(server: &Server, auth: &str, headers: &str, body: &str) -> String {
        let started = Instant::now();
        let mut stream = TcpStream::connect(server.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        let request = format!("POST /mcp HTTP/1.1\r\nHost: {}\r\nAuthorization: {auth}\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\n\r\n{body}",server.address,body.len());
        // An HTTP server may reject headers before consuming the body. Read
        // its response even when that early rejection ends a request write.
        let write_error = stream.write_all(request.as_bytes()).err();
        let mut response = String::new();
        // A rejected body may leave unread bytes and cause a reset after the
        // complete HTTP error; preserve that response for assertions.
        let read_error = stream.read_to_string(&mut response).err();
        assert!(
            !response.is_empty(),
            "missing HTTP response; write={write_error:?}, read={read_error:?}, elapsed={:?}, server={:?}", started.elapsed(), server.last_error.lock().unwrap()
        );
        response
    }
    #[test]
    fn pending_action_finishes_after_activation_without_crossing_recreated_tasks() {
        use crate::codex::TranscriptItem;
        let server = Server::start().unwrap();
        let auth = format!("Bearer {}", server.token());
        let pending = json!({"id":"pending","type":"plan","plan":[{"id":"step-0","step":"Verify totals","status":"pending"}]});
        let active = json!({"id":"active","type":"plan","plan":[{"id":"step-0","step":"Verify totals","status":"inProgress"}]});
        let mut history: Vec<TranscriptItem> = vec![pending.into()];
        server.set_context_items(&history, Some("turn".into()));
        let request = |state: &str, label: &str| {
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"report_activity","arguments":{"id":"verify","task_id":"Verify totals","status":state,"label":label}}}).to_string()
        };
        assert!(call(
            &server,
            &auth,
            "",
            &request("in_progress", "Checking totals")
        )
        .contains("\"accepted\":true"));
        history.push(active.clone().into());
        server.set_context_items(&history, Some("turn".into()));
        assert!(
            call(&server, &auth, "", &request("completed", "Checked totals"))
                .contains("\"accepted\":true")
        );
        history.push(json!({"id":"removed","type":"plan","plan":[]}).into());
        history.push(active.into());
        server.set_context_items(&history, Some("turn".into()));
        assert!(
            call(&server, &auth, "", &request("completed", "Checked totals"))
                .contains("\"isError\":true")
        );
    }
    #[test]
    fn authenticated_mcp_records_only_valid_reports() {
        let server = Server::start().unwrap();
        let checklist =
            json!({"type":"plan","plan":[{"id":"verify","step":"Verify","status":"inProgress"}]});
        server.set_context(crate::tasks::current(&[checklist]), Some("turn".into()));
        let auth = format!("Bearer {}", server.token());
        let initialize = call(
            &server,
            &auth,
            "",
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        );
        assert!(initialize.starts_with("HTTP/1.1 200"));
        assert!(initialize.contains("2025-06-18"));
        let valid = call(
            &server,
            &auth,
            "",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"report_activity","arguments":{"id":"check","task_id":"Verify","status":"completed","label":"Verified behavior"}}}"#,
        );
        assert!(valid.contains("\"accepted\":true"));
        let unknown = call(
            &server,
            &auth,
            "",
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"report_activity","arguments":{"id":"other","task_id":"Unknown","status":"completed","label":"Verified behavior"}}}"#,
        );
        assert!(unknown.contains("\"isError\":true"));
        assert!(unknown.contains("verify: Verify"));
        let invalid = call(
            &server,
            &auth,
            "",
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"report_activity","arguments":{"id":"check"}}}"#,
        );
        assert!(invalid.contains("\"isError\":true"));
        assert!(!invalid.contains("\"accepted\":true"));
        assert!(call(&server, "Bearer wrong", "", "{}").starts_with("HTTP/1.1 403"));
    }
    #[test]
    fn task_context_rejects_expired_ambiguous_and_missing_owners() {
        let server = Server::start().unwrap();
        let auth = format!("Bearer {}", server.token());
        let report = |task: &str| {
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"report_activity","arguments":{"id":"action","task_id":task,"status":"in_progress","label":"Checking behavior"}}}).to_string()
        };
        let expired = call(&server, &auth, "", &report(""));
        assert!(expired.contains("expired"));
        assert!(!expired.contains("\"accepted\":true"));
        server.set_context(None, Some("turn".into()));
        assert!(call(&server, &auth, "", &report("")).contains("\"accepted\":true"));
        assert!(call(&server, &auth, "", &report("missing")).contains("\"isError\":true"));
        let checklist = json!({"type":"plan","plan":[
            {"id":"one","step":"Same","status":"inProgress"},
            {"id":"two","step":"Same","status":"inProgress"}
        ]});
        server.set_context(
            crate::tasks::current(&[checklist]),
            Some("parallel-turn".into()),
        );
        assert!(call(&server, &auth, "", &report("Same")).contains("\"isError\":true"));
        let explicit = call(&server, &auth, "", &report("two"));
        assert!(explicit.contains("\"accepted\":true"));
        assert!(explicit.contains("\"turn_id\":\"parallel-turn\""));
        assert!(call(&server, &auth, "", &report("")).contains("\"isError\":true"));
        server.set_context(None, None);
        assert!(call(&server, &auth, "", &report("")).contains("expired"));
    }

    #[test]
    fn accepted_reports_cannot_retarget_or_reverse_terminal_outcomes() {
        let server = Server::start().unwrap();
        let auth = format!("Bearer {}", server.token());
        let checklist = json!({"type":"plan","plan":[
            {"id":"one","step":"Inspect","status":"inProgress"},
            {"id":"two","step":"Verify","status":"inProgress"}
        ]});
        let set_context = |turn: &str| {
            server.set_context(
                crate::tasks::current(&[checklist.clone()]),
                Some(turn.into()),
            )
        };
        let report = |task: &str, status: &str| {
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"report_activity","arguments":{"id":"action","task_id":task,"status":status,"label":"Checked behavior"}}}).to_string()
        };
        set_context("turn");
        assert!(
            call(&server, &auth, "", &report("one", "in_progress")).contains("\"accepted\":true")
        );
        set_context("turn"); // Publishing an ordinary update retains ownership.
        let retarget = call(&server, &auth, "", &report("two", "completed"));
        assert!(retarget.contains("already belongs to another task"));
        assert!(!retarget.contains("\"accepted\":true"));
        assert!(call(&server, &auth, "", &report("one", "completed")).contains("\"accepted\":true"));
        assert!(call(&server, &auth, "", &report("one", "completed")).contains("\"accepted\":true"));
        for status in ["failed", "in_progress"] {
            let invalid = call(&server, &auth, "", &report("one", status));
            assert!(invalid.contains("cannot be reopened"));
            assert!(!invalid.contains("\"accepted\":true"));
        }
        set_context("next-turn"); // Step ids are scoped to their provider turn.
        assert!(
            call(&server, &auth, "", &report("two", "in_progress")).contains("\"accepted\":true")
        );
    }

    #[test]
    fn accepted_owner_is_bound_to_the_published_immutable_snapshot() {
        let server = Server::start().unwrap();
        let auth = format!("Bearer {}", server.token());
        let checklist = json!({"type":"plan","id":"snapshot-one","taskSnapshot":true,"plan":[
            {"id":"verify","step":"Verify behavior","status":"inProgress"}
        ]});
        let report = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"report_activity","arguments":{"id":"action","task_id":"verify","status":"in_progress","label":"Checking behavior"}}}"#;
        server.set_context_items(&[checklist.clone().into()], Some("turn".into()));
        let accepted = call(&server, &auth, "", report);
        let body: Value = serde_json::from_str(accepted.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            body["result"]["structuredContent"]["owner"],
            json!({"snapshot_id":"snapshot-one","id":"verify","step":"Verify behavior"})
        );
        let mut missing = checklist;
        missing.as_object_mut().unwrap().remove("id");
        server.set_context_items(&[missing.into()], Some("next-turn".into()));
        let rejected = call(&server, &auth, "", report);
        assert!(rejected.contains("snapshot identity is unavailable"));
        assert!(!rejected.contains("\"accepted\":true"));
    }

    #[test]
    fn reconnect_seeds_the_terminal_report_ledger_from_accepted_history() {
        let source = Server::start().unwrap();
        let checklist = json!({"type":"plan","id":"snapshot-one","taskSnapshot":true,"plan":[
            {"id":"verify","step":"Verify behavior","status":"inProgress"}
        ]});
        let arguments = json!({"id":"action","task_id":"verify","status":"completed","label":"Checked behavior"});
        let report = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"report_activity","arguments":arguments}});
        source.set_context_items(&[checklist.clone().into()], Some("turn".into()));
        let accepted = call(
            &source,
            &format!("Bearer {}", source.token()),
            "",
            &report.to_string(),
        );
        let body: Value = serde_json::from_str(accepted.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["result"]["structuredContent"]["accepted"], true);
        let item = json!({"type":"mcpToolCall","server":"grove_activity","tool":"report_activity","status":"completed","arguments":arguments,"result":body["result"]});
        drop(source);
        let restored = Server::start().unwrap();
        restored.set_context_items(&[checklist.into(), item.into()], Some("turn".into()));
        let mut reopened = report.clone();
        reopened["params"]["arguments"]["status"] = json!("in_progress");
        let invalid = call(
            &restored,
            &format!("Bearer {}", restored.token()),
            "",
            &reopened.to_string(),
        );
        assert!(invalid.contains("cannot be reopened"));
        assert!(!invalid.contains("\"accepted\":true"));
        let duplicate = call(
            &restored,
            &format!("Bearer {}", restored.token()),
            "",
            &report.to_string(),
        );
        assert!(duplicate.contains("\"accepted\":true"));
    }

    #[test]
    fn rejects_browser_duplicate_and_oversized_requests() {
        let server = Server::start().unwrap();
        let auth = format!("Bearer {}", server.token());
        assert!(call(&server, &auth, "Origin: http://example.com\r\n", "{}")
            .starts_with("HTTP/1.1 403"));
        assert!(call(&server, &auth, "Content-Length: 2\r\n", "{}").starts_with("HTTP/1.1 400"));
        assert!(call(&server, &auth, "", &"x".repeat(BODY_LIMIT + 1)).starts_with("HTTP/1.1 413"));
        let address = server.address;
        drop(server);
        assert!(TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_err());
    }
}
