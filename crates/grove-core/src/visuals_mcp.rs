//! A separate authenticated visual service; activity reporting remains metadata-only.
use crate::visuals::{Input, Reference};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
const HEADER_LIMIT: usize = 8 * 1024;
const BODY_LIMIT: usize = crate::visuals::DOCUMENT_LIMIT * 2;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
#[derive(Default)]
struct Context {
    session: String,
    turn: Option<String>,
    cwd: PathBuf,
    roots: Vec<PathBuf>,
    host_reads: bool,
    turn_cancel: Arc<AtomicBool>,
    calls: Vec<(String, String, Value)>,
    committed: Vec<Reference>,
    references: Vec<Reference>,
    history_len: usize,
    transcript_revision: Option<u64>,
    recovery_items: Vec<String>,
    recovery: Option<(String, Vec<String>)>,
}
pub(crate) struct Server {
    address: SocketAddr,
    token: String,
    stopping: Arc<AtomicBool>,
    context: Arc<Mutex<Context>>,
    thread: Option<JoinHandle<()>>,
}
impl Server {
    pub(crate) fn start() -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        let token = crate::identity::random_id()?;
        let auth = format!("Bearer {token}");
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = stopping.clone();
        let context = Arc::new(Mutex::new(Context::default()));
        let ctx = context.clone();
        let thread = std::thread::Builder::new()
            .name("grove-visuals-mcp".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let recovery = ctx.lock().ok().and_then(|mut ctx| ctx.recovery.take());
                    if let Some((session, items)) = recovery {
                        let recovered = crate::visuals::recover_events(&session, &items);
                        if let Ok(mut ctx) = ctx.lock() {
                            if ctx.session == session {
                                ctx.committed.extend(recovered);
                            }
                        }
                    }
                    match listener.accept() {
                        Ok((mut stream, peer)) if peer.ip().is_loopback() => {
                            let _ = serve(&mut stream, address, &auth, &ctx, &stop);
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
            context,
            thread: Some(thread),
        })
    }
    pub(crate) fn url(&self) -> String {
        format!("http://{}/mcp", self.address)
    }
    pub(crate) fn token(&self) -> &str {
        &self.token
    }
    pub(crate) fn sync(&self, state: &mut crate::provider::Snapshot, side_chat: bool) -> bool {
        let Ok(mut ctx) = self.context.lock() else {
            return false;
        };
        let mut changed = false;
        for reference in ctx.committed.drain(..) {
            if state
                .saved
                .items
                .iter()
                .any(|i| i["id"] == reference.item_id)
                && !state
                    .saved
                    .visuals
                    .iter()
                    .any(|r| r.publication_id == reference.publication_id)
            {
                state.saved.visuals.push(reference);
                changed = true;
            }
        }
        if ctx.history_len != state.saved.items.len() || ctx.session != state.saved.key {
            let missing: Vec<String> = state
                .saved
                .items
                .iter()
                .filter(|item| {
                    visual_tool(item, state.saved.provider) == Some("render_html")
                        && !state.saved.visuals.iter().any(|r| item["id"] == r.item_id)
                })
                .filter_map(|i| i["id"].as_str().map(str::to_owned))
                .collect();
            if !missing.is_empty() && ctx.recovery_items != missing {
                ctx.recovery = Some((state.saved.key.clone(), missing.clone()));
            }
            ctx.recovery_items = missing;
            ctx.history_len = state.saved.items.len();
        }
        if ctx.references != state.saved.visuals {
            ctx.references.clone_from(&state.saved.visuals);
        }
        ctx.session.clone_from(&state.saved.key);
        ctx.cwd = PathBuf::from(&state.saved.cwd);
        ctx.roots = state
            .saved
            .roots
            .iter()
            .map(|r| PathBuf::from(&r.wt_path))
            .collect();
        let cwd = ctx.cwd.clone();
        ctx.roots.push(cwd);
        // Only acknowledged effective bypass allows host-side path reads. No inferred approvals.
        ctx.host_reads = match state.saved.provider {
            crate::provider::Provider::Codex => {
                state
                    .sandbox
                    .as_ref()
                    .is_some_and(|v| v["type"] == "dangerFullAccess")
                    && state.approval_policy.as_ref().is_some_and(|v| v == "never")
            }
            crate::provider::Provider::Claude => {
                state.resolved_access.as_deref() == Some("bypassPermissions")
            }
        };
        let turn = if side_chat || !state.status.busy() {
            None
        } else {
            state.turn_id.clone()
        };
        let turn_changed = ctx.turn != turn;
        if turn_changed {
            ctx.turn_cancel.store(true, Ordering::Relaxed);
            ctx.turn_cancel = Arc::new(AtomicBool::new(false));
        }
        ctx.turn = turn;
        let revision = state.saved.items.revision();
        if turn_changed || ctx.transcript_revision != Some(revision) {
            ctx.calls.clear();
            if ctx.turn.is_some() {
                let start = state.saved.work_periods.last().map_or(0, |p| p.start_item);
                for item in state.saved.items.iter().skip(start) {
                    let Some(name) = visual_tool(item, state.saved.provider) else {
                        continue;
                    };
                    if let Some(id) = item["id"].as_str() {
                        let args = if let Some(s) = item["arguments"].as_str() {
                            serde_json::from_str(s).unwrap_or(Value::Null)
                        } else {
                            item["arguments"].clone()
                        };
                        ctx.calls.push((id.into(), name.into(), args));
                    }
                }
            }
            ctx.transcript_revision = Some(revision);
        }
        changed
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
pub(crate) fn visual_tool(
    item: &Value,
    provider: crate::provider::Provider,
) -> Option<&'static str> {
    if item["type"] != "mcpToolCall" {
        return None;
    }
    match provider {
        crate::provider::Provider::Codex if item["server"] == "grove_visuals" => {
            match item["tool"].as_str()? {
                "render_html" => Some("render_html"),
                "preview_html" => Some("preview_html"),
                _ => None,
            }
        }
        crate::provider::Provider::Claude if item["server"] == "Claude Code" => {
            match item["tool"].as_str()? {
                "mcp__grove_visuals__render_html" => Some("render_html"),
                "mcp__grove_visuals__preview_html" => Some("preview_html"),
                _ => None,
            }
        }
        _ => None,
    }
}

fn tools() -> Value {
    let schema = json!({"type":"object","additionalProperties":false,"properties":{
        "request_id":{"type":"string","minLength":1,"maxLength":128,"description":"Unique stable ID for this invocation; reuse only when retrying it."},
        "html":{"type":"string","maxLength":crate::visuals::DOCUMENT_LIMIT},"path":{"type":"string","maxLength":4096},
        "title":{"type":"string","minLength":1,"maxLength":200},"visual_id":{"type":"string"},
        "presentation":{"type":"string","enum":["inline","dashboard","canvas","document"],"description":"Authoring intent: small visual, responsive dashboard, framed canvas, or long document. Every publication is an artifact shown as a compact transcript row; open its dedicated viewer to interact."},
        "preview_region":{"type":"object","additionalProperties":false,"required":["x","y","width","height"],"properties":{"x":{"type":"integer","minimum":0,"maximum":1000000},"y":{"type":"integer","minimum":0,"maximum":1000000},"width":{"type":"integer","minimum":1,"maximum":100000},"height":{"type":"integer","minimum":1,"maximum":100000}}},
        "width":{"type":"integer","minimum":240,"maximum":1600},"height":{"type":"integer","minimum":80,"maximum":2000}
    },"required":["request_id","title"],"oneOf":[{"required":["html"],"not":{"required":["path"]}},{"required":["path"],"not":{"required":["html"]}}]});
    json!({"tools":[
        {"name":"render_html","description":"Publish a durable offline HTML artifact beside this tool event. Pass exactly one of complete html or an authorized absolute local HTML path. Inline CSS/JS/fonts/SVG. Local PNG/JPEG/WebP assets require authorized host reads. Use a new request_id for each publication; visual_id explicitly creates a new revision. Set presentation and width/height for the document layout viewport, independent of the single-line transcript artifact row. preview_region selects a document scroll target or a canvas world frame; canvas authors handle the grove:initial-view DOM event detail.frame to fit their camera. A saved reference does not claim successful native loading. Child-agent calls without root ownership are unsupported.","inputSchema":schema,"annotations":{"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}},
        {"name":"preview_html","description":"Check a complete offline HTML document with Grove's native webview. Returns PNG, dimensions, source digest and available bounded diagnostics; publishes nothing. Use width 360 for narrow and 728 for desktop. An unavailable capture backend does not prevent render_html publication.","inputSchema":schema,"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}}
    ]})
}
fn call(
    name: &str,
    args: &Value,
    context: &Mutex<Context>,
    stop: &AtomicBool,
) -> Result<Value, String> {
    if !matches!(name, "render_html" | "preview_html") {
        return Err("Unknown visual tool".into());
    }
    let input: Input =
        serde_json::from_value(args.clone()).map_err(|e| format!("Invalid visual input: {e}"))?;
    if !input
        .request_id
        .as_ref()
        .is_some_and(|s| !s.is_empty() && s.len() <= 128)
    {
        return Err("Provide a stable request_id of 1–128 bytes".into());
    }
    // The provider may deliver its item-start event after forwarding the MCP request.
    let deadline = Instant::now() + Duration::from_secs(4);
    let (session, turn, item, cwd, roots, reads) = loop {
        if stop.load(Ordering::Relaxed) {
            return Err("Visual session closed".into());
        }
        {
            let ctx = context.lock().map_err(|_| "Visual context unavailable")?;
            let candidates: Vec<_> = ctx
                .calls
                .iter()
                .filter(|(_, tool, observed)| tool == name && observed == args)
                .collect();
            if let (Some(turn), [candidate]) = (&ctx.turn, candidates.as_slice()) {
                break (
                    ctx.session.clone(),
                    turn.clone(),
                    candidate.0.clone(),
                    ctx.cwd.clone(),
                    ctx.roots.clone(),
                    ctx.host_reads,
                );
            }
            if candidates.len() > 1 {
                return Err(
                    "Ambiguous tool ownership; use a fresh request_id for each call".into(),
                );
            }
        }
        if Instant::now() >= deadline {
            return Err("No matching root tool event in an active turn; child-agent or stale calls are unsupported".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let html = crate::visuals::prepare(&input, &cwd, &roots, reads)?;
    let ctx = context.lock().map_err(|_| "Visual context unavailable")?;
    if stop.load(Ordering::Relaxed)
        || ctx.turn.as_deref() != Some(&turn)
        || ctx.session != session
        || ctx.host_reads != reads
        || !ctx
            .calls
            .iter()
            .any(|(id, tool, observed)| id == &item && tool == name && observed == args)
    {
        return Err("Visual call expired before commit".into());
    }
    if name == "preview_html" {
        let turn_cancel = ctx.turn_cancel.clone();
        drop(ctx);
        let capture = crate::visual_preview::capture(
            html,
            input.presentation_options().width,
            input.presentation_options().height,
            stop,
            &turn_cancel,
        )?;
        return Ok(capture);
    }
    let turn_cancel = ctx.turn_cancel.clone();
    let references = ctx.references.clone();
    drop(ctx);
    let reference = crate::visuals::publish_cancellable(
        &crate::visuals::directory()?,
        (&session, &item, &turn),
        &input,
        html,
        &references,
        || {
            if stop.load(Ordering::SeqCst) || turn_cancel.load(Ordering::SeqCst) {
                return true;
            }
            context.lock().map_or(true, |ctx| {
                ctx.session != session
                    || ctx.turn.as_deref() != Some(&turn)
                    || ctx.host_reads != reads
                    || ctx.cwd != cwd
                    || ctx.roots != roots
                    || !ctx
                        .calls
                        .iter()
                        .any(|(id, tool, observed)| id == &item && tool == name && observed == args)
            })
        },
    )?;
    if let Ok(mut ctx) = context.lock() {
        if ctx.session == session {
            ctx.committed.push(reference.clone());
        }
    }
    Ok(
        json!({"visual":reference,"message":"Saved immutable artifact; open it from the transcript."}),
    )
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
    context: &Mutex<Context>,
    stop: &AtomicBool,
) -> std::io::Result<()> {
    // BSD accepts inherit the listener's nonblocking mode, unlike Linux.
    // The parser uses bounded blocking reads with one absolute request deadline.
    stream.set_nonblocking(false)?;
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
            json!({"protocolVersion":message["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),"capabilities":{"tools":{}},"serverInfo":{"name":"grove_visuals","version":"1.0.0"}})
        }
        "ping" => json!({}),
        "tools/list" => tools(),
        "tools/call" => match call(
            message["params"]["name"].as_str().unwrap_or_default(),
            &message["params"]["arguments"],
            context,
            stop,
        ) {
            Ok(mut result) => {
                let png = result.as_object_mut().and_then(|r| r.remove("png"));
                let mut content = vec![json!({"type":"text","text":result.to_string()})];
                if let Some(png) = png.and_then(|v| v.as_str().map(str::to_owned)) {
                    content.push(json!({"type":"image","mimeType":"image/png","data":png}));
                }
                json!({"content":content,"structuredContent":result,"isError":false})
            }
            Err(error) => json!({"content":[{"type":"text","text":error}],"isError":true}),
        },
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schemas_require_explicit_sources_and_stable_requests() {
        let tools = tools();
        for tool in tools["tools"].as_array().unwrap() {
            assert_eq!(
                tool["inputSchema"]["required"],
                json!(["request_id", "title"])
            );
            assert_eq!(tool["inputSchema"]["oneOf"].as_array().unwrap().len(), 2);
            assert_eq!(tool["annotations"]["openWorldHint"], false);
        }
    }
    #[test]
    fn context_requires_active_root_tool_and_effective_permissions() {
        use crate::provider::{Provider, SavedSession, Snapshot, Status};
        for provider in [Provider::Codex, Provider::Claude] {
            let server = Server::start().unwrap();
            let saved = SavedSession::empty("test".into(), provider, "test".into(), "/tmp".into());
            let mut state = Snapshot::new(saved);
            state.status = Status::Working;
            state.turn_id = Some("turn".into());
            state.saved.items.push(json!({"id":"tool","type":"mcpToolCall","server":if provider == Provider::Claude {"Claude Code"} else {"grove_visuals"},"tool":if provider == Provider::Claude {"mcp__grove_visuals__render_html"} else {"render_html"},"arguments":{"request_id":"one"},"status":"inProgress"}));
            server.sync(&mut state, false);
            {
                let ctx = server.context.lock().unwrap();
                assert_eq!(ctx.calls.len(), 1);
                assert!(!ctx.host_reads);
                assert_eq!(ctx.turn.as_deref(), Some("turn"));
            }
            let cancel = server.context.lock().unwrap().turn_cancel.clone();
            state.status = Status::Done;
            server.sync(&mut state, false);
            assert!(server.context.lock().unwrap().calls.is_empty());
            assert!(cancel.load(Ordering::Relaxed));
            state.status = Status::Working;
            server.sync(&mut state, true);
            assert!(server.context.lock().unwrap().turn.is_none());
        }
    }
    #[test]
    fn transport_rejects_browser_and_foreign_bearer_without_executing() {
        let server = Server::start().unwrap();
        for headers in [
            "Authorization: Bearer wrong\r\n".to_string(),
            format!(
                "Authorization: Bearer {}\r\nOrigin: http://evil.test\r\n",
                server.token
            ),
        ] {
            let mut stream = TcpStream::connect(server.address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            write!(stream, "POST /mcp HTTP/1.1\r\nHost: {}\r\n{}Content-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}",server.address,headers).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            assert!(response.starts_with("HTTP/1.1 403"));
        }
    }
}

#[cfg(test)]
mod provenance_tests {
    use super::*;
    use crate::provider::Provider;
    #[test]
    fn only_exact_provider_tool_events_can_own_publications() {
        let codex = json!({"type":"mcpToolCall","server":"grove_visuals","tool":"render_html"});
        let claude = json!({"type":"mcpToolCall","server":"Claude Code","tool":"mcp__grove_visuals__render_html"});
        assert_eq!(visual_tool(&codex, Provider::Codex), Some("render_html"));
        assert_eq!(visual_tool(&claude, Provider::Claude), Some("render_html"));
        assert_eq!(visual_tool(&codex, Provider::Claude), None);
        assert_eq!(visual_tool(&claude, Provider::Codex), None);
        let mut forged = codex;
        forged["type"] = json!("userMessage");
        assert_eq!(visual_tool(&forged, Provider::Codex), None);
        let mut bare_claude = claude;
        bare_claude["tool"] = json!("render_html");
        assert_eq!(visual_tool(&bare_claude, Provider::Claude), None);
    }
}
