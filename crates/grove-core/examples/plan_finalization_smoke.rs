//! Opt-in live plan-decision finalization through the production Codex/Claude actor.
//! Requires authenticated CLIs, an empty disposable cwd and GROVE_CONFIG_DIR.
//! It generates plan artifacts only; it never sends implementation approval.
use grove_core::{
    codex::{self, Action, Connection, Provider, SavedSession, Snapshot, Status},
    rich_plan::{ResponseDelivery, ReviewState},
};
use std::time::{Duration, Instant};

fn wait(
    connection: &Connection,
    predicate: impl Fn(&Snapshot) -> bool,
) -> Result<Snapshot, String> {
    let deadline = Instant::now() + Duration::from_mins(3);
    let mut last = "no snapshots".to_owned();
    loop {
        let snapshot = connection
            .updates
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|error| format!("Provider wait ended ({error}): {last}"))?;
        last = format!(
            "status={}, items={}, plans={}, import_errors={}, error_present={}",
            snapshot.status.label(),
            snapshot.saved.items.len(),
            snapshot.saved.rich_plans.revisions.len(),
            snapshot.saved.rich_plans.errors.len(),
            snapshot.error.is_some()
        );
        if !snapshot.approvals.is_empty() {
            return Err(format!(
                "Provider requested approval; none was granted: {last}"
            ));
        }
        if matches!(snapshot.status, Status::Disconnected | Status::Failed) {
            return Err(format!("Provider failed: {last}"));
        }
        if snapshot.status == Status::Done && !snapshot.saved.rich_plans.errors.is_empty() {
            return Err(format!(
                "Plan import failed: {:?}",
                snapshot
                    .saved
                    .rich_plans
                    .errors
                    .iter()
                    .map(|e| &e.error)
                    .collect::<Vec<_>>()
            ));
        }
        if predicate(&snapshot) {
            return Ok(snapshot);
        }
    }
}

struct StopOnDrop(Option<codex::Handle>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            let _ = handle
                .stop_completion(false)
                .recv_timeout(Duration::from_secs(10));
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
        _ => return Err("Usage: plan_finalization_smoke codex|claude EMPTY_DISPOSABLE_CWD".into()),
    };
    let cwd = std::path::PathBuf::from(args.get(2).ok_or("Missing cwd")?)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if fs_err::read_dir(&cwd)
        .map_err(|error| error.to_string())?
        .next()
        .is_some()
    {
        return Err("Use an empty disposable cwd".into());
    }
    grove_core::env_path::ensure_login_path();
    let key = grove_core::identity::random_id()?;
    let mut saved: SavedSession = serde_json::from_value(serde_json::json!({
        "provider":provider,"key":key,"project":"Plan finalization smoke","cwd":cwd,
        "label":"Plan finalization smoke","roots":[],"thread_id":null,"items":[]
    }))
    .map_err(|error| error.to_string())?;
    if provider == Provider::Codex {
        saved.config_args = vec!["-c".into(), "sandbox_mode=\"workspace-write\"".into()];
    }
    let connection = codex::connect(saved, Vec::new());
    let mut cleanup = StopOnDrop(Some(connection.handle.clone()));
    wait(&connection, |s| s.status == Status::Idle)?;
    let prompt = r##"This is a bounded plan-finalization smoke. Only create and revise smoke.grove-plan.json in this disposable cwd; do not access other directories or implement the hypothetical feature. Write this JSON document exactly and reply with its absolute Markdown file link. The next user response will resolve the decisions; then incorporate the answers into a new revision and the required finalized_from metadata. Preserve and update presentation, including revision_changes, scope and success_criteria to reflect both answers. JSON: {"schema_version":1,"plan_id":"finalization-smoke","revision_id":"r1","title":"Export report plan","presentation":{"outcome":"Create a bounded export report.","flow":[{"title":"Export","detail":"Generate report"},{"title":"Retain","detail":"Apply the chosen retention policy"}],"included":["Report export"],"deferred":["Background scheduling"],"risks":["Expired exports must not remain accessible"],"success_criteria":["Verify the chosen platform and retention policy"],"revision_changes":[]},"markdown":"# Export report plan\n\nCreate an export report after its platform and retention decisions are finalized.\n\n## Platform\nSelect the initial target platform.\n\n## Retention\nChoose the export retention policy.\n\n## Verification\nVerify the resolved platform and retention behavior. This smoke only updates the plan; do not implement the feature.","decisions":[{"id":"platform","section":"Platform","prompt":"Which platform should the first version support?","options":[{"id":"linux","label":"Linux","description":"Start with Linux"},{"id":"all","label":"All desktop platforms","description":"Support all desktop platforms"}]},{"id":"retention","section":"Retention","prompt":"How long should exports be retained?","options":[{"id":"day","label":"One day","description":"Delete after one day"},{"id":"forever","label":"Until manually deleted","description":"Keep until the user removes them"}]}]}"##;
    connection.handle.send(Action::Prompt(prompt.into()))?;
    let imported = wait(&connection, |s| {
        s.status == Status::Done && s.saved.rich_plans.revisions.len() == 1
    })?;
    let presentation = imported.saved.rich_plans.revisions[0]
        .document
        .presentation
        .as_ref()
        .ok_or("Native presentation was not imported")?;
    if presentation.flow.len() != 2 {
        return Err("Native flow steps were not preserved".into());
    }
    let thread = imported.saved.thread_id.clone();
    let initial = imported
        .saved
        .rich_plans
        .revision("finalization-smoke", "r1")
        .ok_or("Missing initial plan")?;
    if initial.document.all_decisions().len() != 2 {
        return Err("Provider did not generate the two requested decisions".into());
    }
    let mut check = imported.saved.rich_plans.clone();
    if check.prepare_approval("finalization-smoke", "r1").is_ok() {
        return Err("Unresolved decision plan was approvable".into());
    }
    println!("PASS {provider:?}: live provider generated two-decision plan, production import succeeded; unresolved approval rejected locally.");
    let custom = "Keep exports for 7 days, then delete them.";
    let mut review = ReviewState::default();
    review.answers.insert("platform".into(), "linux".into());
    review.own_answers.insert("retention".into(), custom.into());
    review.comments.insert("plan".into(), "Only update smoke.grove-plan.json with a finalized revision and link it. Do not implement anything.".into());
    check.update_review("finalization-smoke", "r1", review.clone())?;
    if check.prepare_approval("finalization-smoke", "r1").is_ok() {
        return Err("Answered but not incorporated decisions were approvable".into());
    }
    let draft = "Unrelated composer draft must survive plan finalization";
    connection.handle.send(Action::Draft(draft.into()))?;
    connection.handle.send(Action::UpdatePlanReview {
        plan_id: "finalization-smoke".into(),
        revision_id: "r1".into(),
        review: Box::new(review),
    })?;
    for _ in 0..2 {
        connection.handle.send(Action::SendPlanResponse {
            plan_id: "finalization-smoke".into(),
            revision_id: "r1".into(),
        })?;
    }
    let finalized = wait(&connection, |s| {
        s.status == Status::Done && s.saved.rich_plans.revisions.len() == 2
    })?;
    let latest = finalized
        .saved
        .rich_plans
        .revisions
        .last()
        .ok_or("Missing finalized revision")?;
    let presentation = latest
        .document
        .presentation
        .as_ref()
        .ok_or("Finalized native presentation missing")?;
    if presentation.revision_changes.is_empty()
        || !latest
            .document
            .markdown_body()
            .contains(&presentation.outcome)
    {
        return Err("Finalized overview changes or complete copy missing".into());
    }
    let provenance = latest
        .document
        .finalized_from
        .as_ref()
        .ok_or("Missing finalization provenance")?;
    if provenance.revision_id != "r1"
        || provenance.answers.get("platform").map(String::as_str) != Some("Linux")
        || provenance.answers.get("retention").map(String::as_str) != Some(custom)
    {
        return Err(
            "Finalized revision did not preserve the exact selected and custom answers".into(),
        );
    }
    if !latest.document.all_decisions().is_empty() || latest.document.revision_id == "r1" {
        return Err(
            "Finalized plan did not replace unresolved decisions with a new revision".into(),
        );
    }
    let body = latest.document.markdown_body();
    if !body.contains("Linux") || !(body.contains('7') || body.to_lowercase().contains("seven")) {
        return Err("Finalized Markdown did not incorporate platform and custom retention".into());
    }
    if finalized.saved.thread_id != thread || finalized.saved.draft != draft {
        return Err("Finalization changed session identity or composer draft".into());
    }
    let old = finalized
        .saved
        .rich_plans
        .revision("finalization-smoke", "r1")
        .ok_or("Initial revision lost")?;
    if !matches!(old.review.delivery, ResponseDelivery::Sent { .. }) {
        return Err("Finalization request lacks confirmed delivery".into());
    }
    let count = finalized
        .saved
        .items
        .iter()
        .filter(|item| item["type"] == "userMessage" && codex::item_text(item).contains(custom))
        .count();
    if count != 1 {
        return Err(format!("Expected one decision delivery, got {count}"));
    }
    let mut approval_check = finalized.saved.rich_plans.clone();
    if approval_check
        .prepare_approval("finalization-smoke", &latest.document.revision_id)?
        .is_none()
    {
        return Err("Finalized revision was not approvable in cloned store".into());
    }
    if approval_check
        .prepare_approval("finalization-smoke", "r1")
        .is_ok()
    {
        return Err("Superseded decision revision became approvable".into());
    }
    println!("PASS {provider:?}: live same-session update incorporated selected/custom answers; exactly one request delivered despite duplicate action; draft and immutable initial revision retained. Finalized approval readiness checked on a clone only; no approval sent.");
    connection
        .handle
        .stop_completion(false)
        .recv_timeout(Duration::from_secs(10))
        .map_err(|error| error.to_string())?;
    cleanup.0 = None;
    let recovered = codex::load_sessions()?
        .sessions
        .into_iter()
        .find(|s| s.key == key)
        .ok_or("Missing saved session")?;
    if recovered.rich_plans.revisions.len() != 2 || recovered.draft != draft {
        return Err("Finalized revisions or composer draft did not survive persistence".into());
    }
    let mut recovered_check = recovered.rich_plans;
    recovered_check
        .prepare_approval("finalization-smoke", &latest.document.revision_id)?
        .ok_or("Recovered finalized plan not approvable")?;
    println!("PASS {provider:?}: finalized plan and readiness survived persisted session reload. Native widgets and implementation delivery were not exercised.");
    Ok(())
}
