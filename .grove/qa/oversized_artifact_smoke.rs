//! Live provider-boundary rich-plan smoke. Requires disposable cwd/config and installed authenticated CLIs.
use grove_core::{
    codex::{self, Action, Connection, Provider, SavedSession, Snapshot, Status},
    
};
use std::time::{Duration, Instant};
fn diagnostic(s: &Snapshot) -> String {
    let mut approvals = std::collections::BTreeMap::new();
    for approval in &s.approvals {
        let kind = match approval["method"].as_str() {
            Some("item/commandExecution/requestApproval") => "command_execution",
            Some("item/fileChange/requestApproval") => "file_change",
            Some("execCommandApproval") => "legacy_command",
            Some("applyPatchApproval") => "legacy_patch",
            _ => "other",
        };
        *approvals.entry(kind).or_insert(0usize) += 1;
    }
    let mut items = std::collections::BTreeMap::new();
    for item in &s.saved.items {
        let kind = match item["type"].as_str() {
            Some("userMessage") => "user",
            Some("agentMessage") => "assistant",
            Some("commandExecution") => "command",
            Some("fileChange") => "file_change",
            Some("mcpToolCall") => "tool",
            _ => "other",
        };
        let status = match item["status"].as_str() {
            Some("inProgress") => "in_progress",
            Some("completed") => "completed",
            Some("failed") => "failed",
            _ => "unspecified",
        };
        *items.entry((kind, status)).or_insert(0usize) += 1;
    }
    format!("status={}, approvals={} {:?}, questions={}, items={:?}, imported_revisions={}, import_errors={}",s.status.label(),s.approvals.len(),approvals,s.questions.len(),items,s.saved.rich_plans.revisions.len(),s.saved.rich_plans.errors.len())
}
fn wait(
    connection: &Connection,
    predicate: impl Fn(&Snapshot) -> bool,
) -> Result<Snapshot, String> {
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut last = "no snapshot received".to_owned();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!("Provider smoke timed out: {last}"));
        }
        let s = connection
            .updates
            .recv_timeout(remaining)
            .map_err(|error| format!("Provider smoke receive ended ({error}): {last}"))?;
        last = diagnostic(&s);
        if !s.approvals.is_empty() {
            return Err(format!(
                "Provider smoke is waiting for approval; no approval was submitted: {last}"
            ));
        }
        if matches!(s.status, Status::Failed | Status::Disconnected) {
            return Err(format!("Provider smoke failed: {last}"));
        }
        if predicate(&s) {
            return Ok(s);
        }
    }
}
/// Ensure every early error stops this smoke's own actor/process and flushes recovery.
struct StopOnDrop(Option<codex::Handle>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            if let Err(error) = handle
                .stop_completion(false)
                .recv_timeout(Duration::from_secs(10))
            {
                eprintln!("Smoke cleanup did not confirm actor stop: {error}");
            }
        }
    }
}
fn main() -> Result<(), String> {
    if std::env::var_os("GROVE_CONFIG_DIR").is_none() { return Err("Set disposable GROVE_CONFIG_DIR".into()); }
    let args: Vec<_> = std::env::args().collect();
    let provider = match args.get(1).map(String::as_str) { Some("codex") => Provider::Codex, Some("claude") => Provider::Claude, _ => return Err("Usage: oversized_artifact_smoke codex|claude EMPTY_DISPOSABLE_CWD".into()) };
    let cwd = std::path::PathBuf::from(args.get(2).ok_or("Missing cwd")?).canonicalize().map_err(|e| e.to_string())?;
    if fs_err::read_dir(&cwd).map_err(|e|e.to_string())?.next().is_some() { return Err("Use empty disposable cwd".into()); }
    let plan = cwd.join("oversized.grove-plan.json");
    let html = cwd.join("oversized.html");
    let valid = r#"{"schema_version":1,"plan_id":"size-smoke","revision_id":"r1","title":"Size recovery smoke","claims":[{"id":"claim","text":"Safe read-only fixture"}]}"#;
    fs_err::write(&plan, format!("{valid}{}", " ".repeat(1024*1024))).map_err(|e|e.to_string())?;
    fs_err::write(&html, format!("<!doctype html><html><body>Safe fixture<!--{}--></body></html>", "x".repeat(2*1024*1024))).map_err(|e|e.to_string())?;
    let plan_size = fs_err::metadata(&plan).map_err(|e|e.to_string())?.len();
    grove_core::env_path::ensure_login_path();
    let key = grove_core::identity::random_id()?;
    let saved:SavedSession=serde_json::from_value(serde_json::json!({"provider":provider,"key":key,"project":"Oversized artifact smoke","cwd":cwd,"label":"Size smoke","roots":[],"thread_id":null,"items":[]})).map_err(|e|e.to_string())?;
    let connection = codex::connect(saved, Vec::new());
    let mut cleanup = StopOnDrop(Some(connection.handle.clone()));
    wait(&connection, |s| s.status == Status::Idle)?;
    let prompt = format!("Reply with exactly these two Markdown links, using the paths literally. Do not call tools, read, write, edit, or execute anything. [Plan]({})\n[HTML]({})",plan.display(),html.display());
    connection.handle.send(Action::Prompt(prompt))?;
    let failed = wait(&connection, |s| s.status == Status::Done && !s.saved.rich_plans.errors.is_empty())?;
    let error = failed.saved.rich_plans.errors.iter().find(|e|e.size_limit.is_some()).ok_or("No structured size error")?.clone();
    let size = error.size_limit.as_ref().unwrap();
    if size.bytes != plan_size || size.limit != 1024*1024 || size.path != plan { return Err(format!("Unexpected size metadata: {size:?}")); }
    let html_link = failed.saved.items.iter().filter(|i|i["type"]=="agentMessage").any(|i|codex::item_text(i).contains(html.to_str().unwrap()));
    if !html_link { return Err("Actual provider response omitted HTML link".into()); }
    println!("PASS {provider:?}: completed live provider response imported plan link as structured size error ({} bytes, limit {}); actual HTML link present. HTML renderer is not exercised by this actor harness.",size.bytes,size.limit);
    fs_err::write(&plan, valid).map_err(|e|e.to_string())?;
    connection.handle.send(Action::RetryPlanImport { message_id:error.message_id, source:error.source })?;
    let recovered = wait(&connection, |s| s.saved.rich_plans.errors.is_empty() && s.saved.rich_plans.revisions.len()==1)?;
    if recovered.saved.rich_plans.revisions[0].document.plan_id != "size-smoke" { return Err("Wrong recovered plan".into()); }
    println!("PASS {provider:?}: external shrink followed by production RetryPlanImport clears size error and imports original reference.");
    connection.handle.stop_completion(false).recv_timeout(Duration::from_secs(10)).map_err(|e|e.to_string())?;
    cleanup.0 = None;
    Ok(())
}
