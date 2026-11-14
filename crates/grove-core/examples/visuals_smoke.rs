//! Live transport smoke. Uses only inline offline HTML in a disposable session.
//! Run with a disposable GROVE_CONFIG_DIR and an empty cwd. No approvals are answered.
use grove_core::{
    codex::{self, Action, Provider, SavedSession, Snapshot, Status},
    visuals,
};
use std::time::{Duration, Instant};
fn wait(connection: &codex::Connection, done: bool) -> Result<Snapshot, String> {
    let deadline = Instant::now() + Duration::from_secs(150);
    loop {
        let s = connection
            .updates
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|e| e.to_string())?;
        if !s.approvals.is_empty() {
            return Err(format!("Provider requested approval: {:?}", s.approvals));
        }
        if matches!(s.status, Status::Failed | Status::Disconnected) {
            return Err(format!("Provider failed: {:?}", s.error));
        }
        if (!done && s.status == Status::Idle) || (done && s.status == Status::Done) {
            return Ok(s);
        }
    }
}
fn main() -> Result<(), String> {
    if std::env::var_os("GROVE_CONFIG_DIR").is_none() {
        return Err("Use a disposable GROVE_CONFIG_DIR".into());
    }
    let args: Vec<_> = std::env::args().collect();
    let provider = match args.get(1).map(String::as_str) {
        Some("codex") => Provider::Codex,
        Some("claude") => Provider::Claude,
        _ => return Err("Usage: visuals_smoke codex|claude EMPTY_CWD".into()),
    };
    let cwd = std::path::PathBuf::from(args.get(2).ok_or("Missing cwd")?)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if fs_err::read_dir(&cwd)
        .map_err(|e| e.to_string())?
        .next()
        .is_some()
    {
        return Err("Use an empty disposable cwd".into());
    }
    let saved = SavedSession::empty(
        grove_core::identity::random_id()?,
        provider,
        "Visual smoke".into(),
        cwd.display().to_string(),
    );
    let c = codex::connect(saved, Vec::new());
    let result = (|| {
        wait(&c, false)?;
        c.handle.send(Action::Prompt("This is a bounded tool transport smoke. Do not read or write workspace files, run shell commands or delegate. Call grove_visuals.preview_html once with inline html <!doctype html><html><body><h1>Smoke chart</h1><input value=\"retained\"></body></html>, title Visual smoke, request_id smoke-preview, width 360, height 560. If native preview is unavailable report that honestly and continue. Then call grove_visuals.render_html with exactly the same html, title Visual smoke and request_id smoke-publish. Finish with a short plain-text receipt. Do not use HTML fences or file links as a substitute for these calls.".into()))?;
        let s = wait(&c, true)?;
        let path = std::env::var("GROVE_CONFIG_DIR").map_err(|e| e.to_string())?;
        fs_err::write(
            std::path::Path::new(&path).join("smoke-snapshot.json"),
            serde_json::to_vec_pretty(&s.saved).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if s.saved.visuals.len() != 1 {
            return Err(format!(
                "Expected one publication, got {}. See smoke-snapshot.json",
                s.saved.visuals.len()
            ));
        }
        let r = &s.saved.visuals[0];
        let html = visuals::read(r)?;
        if !html.contains("Smoke chart") {
            return Err("Published snapshot lost content".into());
        }
        let restored = Snapshot::new(s.saved.clone());
        if restored.saved.visuals != s.saved.visuals {
            return Err("Reload changed saved publications".into());
        }
        println!("{provider:?}: live publication and persisted reload passed; preview unavailable without UI event loop");
        Ok(())
    })();
    let _ = c
        .handle
        .stop_completion(false)
        .recv_timeout(Duration::from_secs(10));
    result
}
