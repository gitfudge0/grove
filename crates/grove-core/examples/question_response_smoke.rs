//! Opt-in production-actor question/answer smoke. No native widgets are exercised.
//! Run with isolated GROVE_CONFIG_DIR and an empty disposable working directory.
use grove_core::codex::{self, Action, Connection, SavedSession, Snapshot, Status};
use std::time::{Duration, Instant};

struct StopOnDrop(codex::Handle);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let _ = self
            .0
            .stop_completion(false)
            .recv_timeout(Duration::from_secs(5));
    }
}
fn wait(connection: &Connection, ready: impl Fn(&Snapshot) -> bool) -> Result<Snapshot, String> {
    let deadline = Instant::now() + Duration::from_mins(2);
    loop {
        let state = connection
            .updates
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|e| e.to_string())?;
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        if matches!(state.status, Status::Disconnected | Status::Failed) {
            return Err(state.status.label().into());
        }
        if !state.approvals.is_empty() || !state.elicitations.is_empty() {
            return Err("Unexpected permission or elicitation; no response granted".into());
        }
        if ready(&state) {
            return Ok(state);
        }
    }
}
fn main() -> Result<(), String> {
    if std::env::var_os("GROVE_CONFIG_DIR").is_none() {
        return Err("Set isolated GROVE_CONFIG_DIR".into());
    }
    let cwd = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("Supply empty disposable cwd")?,
    )
    .canonicalize()
    .map_err(|e| e.to_string())?;
    if fs_err::read_dir(&cwd)
        .map_err(|e| e.to_string())?
        .next()
        .is_some()
    {
        return Err("Use an empty disposable cwd".into());
    }
    grove_core::env_path::ensure_login_path();
    let saved: SavedSession=serde_json::from_value(serde_json::json!({"provider":"codex","key":grove_core::identity::random_id()?,"project":"Question response smoke","cwd":cwd,"label":"Question response smoke","roots":[],"items":[],"draft":"Preserve unrelated draft"})).map_err(|e|e.to_string())?;
    let connection = codex::connect(saved, Vec::new());
    let _cleanup = StopOnDrop(connection.handle.clone());
    wait(&connection, |s| s.status == Status::Idle)?;
    connection.handle.send(Action::Prompt("This is an explicit question/answer transport acceptance check. Do not read or change files or run shell commands. Use your available native question tool to ask exactly one single-choice question: Which release audience? Options Pilot and Everyone. Prefer request_user_input_async if available. Wait for the answer. Once answered, reply with exactly QUESTION_SMOKE_CONFIRMED and the selected audience. Do not replace the native question with prose.".into()))?;
    let state = wait(&connection, |s| !s.questions.is_empty())?;
    let request = state.questions.first().ok_or("No question received")?;
    let mut answers = std::collections::HashMap::new();
    for question in &request.questions {
        let value = question
            .options
            .iter()
            .find(|o| o.label.contains("Pilot"))
            .or_else(|| question.options.first())
            .map(|o| o.label.clone())
            .ok_or("Provider did not offer choices")?;
        answers.insert(question.id.clone(), vec![value]);
    }
    request.validate(&answers)?;
    connection
        .handle
        .send(Action::Draft("Preserve unrelated draft".into()))?;
    wait(&connection, |s| s.saved.draft == "Preserve unrelated draft")?;
    println!(
        "Observed live Codex question with {} offered choices",
        request.questions[0].options.len()
    );
    let id = request.id.clone();
    connection
        .handle
        .send(Action::AnswerQuestions(id.clone(), Some(answers)))?;
    let ack = wait(&connection, |s| {
        s.question_answer_acks.iter().any(|a| a.id == id)
    })?;
    if ack.saved.draft != "Preserve unrelated draft" {
        return Err("Unrelated draft changed".into());
    }
    println!("PASS live Codex production actor acknowledged response and preserved unrelated draft. Native widget behavior is not covered.");
    Ok(())
}
