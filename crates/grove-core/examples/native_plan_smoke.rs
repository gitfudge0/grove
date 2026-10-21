//! Live provider-boundary rich-plan smoke. Requires disposable cwd/config and installed authenticated CLIs.
use grove_core::{
    codex::{self, Action, Connection, Provider, SavedSession, Snapshot, Status},
    rich_plan::{ResponseDelivery, ReviewState},
};
use std::time::{Duration, Instant};
fn wait(
    connection: &Connection,
    predicate: impl Fn(&Snapshot) -> bool,
) -> Result<Snapshot, String> {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Provider smoke timed out".into());
        }
        let s = connection
            .updates
            .recv_timeout(remaining)
            .map_err(|e| e.to_string())?;
        if matches!(s.status, Status::Failed | Status::Disconnected) {
            return Err(s.error.unwrap_or_else(|| s.status.label().into()));
        }
        if predicate(&s) {
            return Ok(s);
        }
    }
}
fn main() -> Result<(), String> {
    if std::env::var_os("GROVE_CONFIG_DIR").is_none() {
        return Err("Set disposable GROVE_CONFIG_DIR".into());
    }
    let args: Vec<_> = std::env::args().collect();
    let provider = match args.get(1).map(String::as_str) {
        Some("codex") => Provider::Codex,
        Some("claude") => Provider::Claude,
        _ => return Err("Usage: native_plan_smoke codex|claude EMPTY_DISPOSABLE_CWD".into()),
    };
    let cwd = std::path::PathBuf::from(args.get(2).ok_or("Missing disposable cwd")?)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if std::fs::read_dir(&cwd)
        .map_err(|e| e.to_string())?
        .next()
        .is_some()
    {
        return Err("Use an empty disposable cwd".into());
    }
    grove_core::env_path::ensure_login_path();
    let key = grove_core::identity::random_id()?;
    let saved:SavedSession=serde_json::from_value(serde_json::json!({"provider":provider,"key":key,"project":"Native plan smoke","cwd":cwd,"label":"Smoke","roots":[],"thread_id":null,"items":[]})).map_err(|e|e.to_string())?;
    let connection = codex::connect(saved, Vec::new());
    let ready = wait(&connection, |s| s.status == Status::Idle)?;
    let thread = ready.saved.thread_id.clone();
    let prompt="This is a bounded native-plan review smoke test. Only write the following schema-v1 JSON document atomically to smoke.grove-plan.json in this disposable working directory, then reply with a Markdown link to it. Do not implement anything or access other directories. JSON: {\"schema_version\":1,\"plan_id\":\"smoke\",\"revision_id\":\"r1\",\"title\":\"Native plan smoke\",\"claims\":[{\"id\":\"claim\",\"text\":\"Keep this smoke bounded\",\"decisions\":[{\"id\":\"choice\",\"prompt\":\"Review choice\",\"options\":[{\"id\":\"keep\",\"label\":\"Keep scope\"}],\"default\":\"keep\"}]}]}";
    connection.handle.send(Action::Prompt(prompt.into()))?;
    let imported = wait(&connection, |s| {
        !s.rich_plan_empty() && s.status == Status::Done
    })?;
    let thread = thread.or(imported.saved.thread_id.clone());
    println!(
        "PASS {:?} generated and linked plan imported by production actor in {:?}",
        provider, thread
    );
    connection.handle.send(Action::Prompt("Make the same plan revision r2, preserving all claim/decision IDs. Atomically rewrite smoke.grove-plan.json and link it. Do not implement anything.".into()))?;
    let revised = wait(&connection, |s| {
        s.status == Status::Done && s.saved.rich_plans.revisions.len() == 2
    })?;
    if revised.saved.rich_plans.revisions[0].document.revision_id != "r1"
        || revised.saved.rich_plans.revisions[1].document.revision_id != "r2"
    {
        return Err("Revision snapshots lost".into());
    }
    println!(
        "PASS {:?} immutable r1 retained while r2 imported",
        provider
    );
    let mut review = ReviewState::default();
    review.answers.insert("choice".into(), "keep".into());
    review.comments.insert(
        "claim".into(),
        "NATIVE_REVIEW_MARIGOLD: acknowledge this review only; no implementation.".into(),
    );
    connection.handle.send(Action::Draft(
        "Unrelated composer draft must survive".into(),
    ))?;
    connection.handle.send(Action::UpdatePlanReview {
        plan_id: "smoke".into(),
        revision_id: "r2".into(),
        review,
    })?;
    connection.handle.send(Action::SendPlanResponse {
        plan_id: "smoke".into(),
        revision_id: "r2".into(),
    })?;
    connection.handle.send(Action::SendPlanResponse {
        plan_id: "smoke".into(),
        revision_id: "r2".into(),
    })?;
    let delivered = wait(&connection, |s| {
        s.status == Status::Done
            && s.saved
                .rich_plans
                .revisions
                .iter()
                .any(|r| matches!(r.review.delivery, ResponseDelivery::Sent { .. }))
    })?;
    if delivered.saved.thread_id != thread {
        return Err("Feedback changed provider session".into());
    }
    if delivered.saved.draft != "Unrelated composer draft must survive" {
        return Err("Feedback overwrote composer draft".into());
    }
    let count = delivered
        .saved
        .items
        .iter()
        .filter(|item| {
            item["type"] == "userMessage"
                && codex::item_text(item).contains("NATIVE_REVIEW_MARIGOLD")
        })
        .count();
    if count != 1 {
        return Err(format!(
            "Expected exactly one delivered response, saw {count}"
        ));
    }
    println!("PASS {:?} explicit same-session feedback, duplicate click suppression, preserved composer draft",provider);
    connection
        .handle
        .stop_completion(false)
        .recv_timeout(Duration::from_secs(10))
        .map_err(|e| e.to_string())?;
    let recovered = codex::load_sessions()?
        .sessions
        .into_iter()
        .find(|s| s.key == key)
        .ok_or("Missing persisted smoke session")?;
    if !matches!(
        recovered.rich_plans.revisions[1].review.delivery,
        ResponseDelivery::Sent { .. }
    ) {
        return Err("Confirmed delivery was not persisted".into());
    }
    println!("PASS persisted rich plan and confirmed delivery after stop");
    Ok(())
}
trait RichPlanEmpty {
    fn rich_plan_empty(&self) -> bool;
}
impl RichPlanEmpty for Snapshot {
    fn rich_plan_empty(&self) -> bool {
        self.saved.rich_plans.revisions.is_empty()
    }
}
