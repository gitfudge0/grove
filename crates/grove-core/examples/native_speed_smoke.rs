//! Opt-in production-transport speed smoke. Uses disposable Grove config/cwd;
//! authenticated provider CLIs retain credential ownership. Never grants tools.
use grove_core::codex::{self, Action, Provider, SavedSession, Snapshot, Speed, Status};
use std::time::{Duration, Instant};
fn wait(c: &codex::Connection, predicate: impl Fn(&Snapshot) -> bool) -> Result<Snapshot, String> {
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut last = "no snapshots".to_owned();
    loop {
        let s = c
            .updates
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|e| format!("Provider wait: {e}; last={last}"))?;
        last = format!("status={:?} requested={:?} accepted={} accepted_submission={:?} queued={:?} sending={:?} turn={:?} reported={:?}", s.status,s.saved.composer.speed,s.accepted_prompts,s.accepted_submission,s.queued_prompts.iter().map(|p|p.id).collect::<Vec<_>>(),s.queue_sending,s.turn_id,s.reported_speed);
        if matches!(s.status, Status::Failed | Status::Disconnected) || s.error.is_some() {
            return Err(format!(
                "Provider status={:?}, error={:?}",
                s.status, s.error
            ));
        }
        if !s.approvals.is_empty() {
            return Err("Unexpected approval; smoke does not grant tools".into());
        }
        if predicate(&s) {
            return Ok(s);
        }
    }
}
struct Cleanup(codex::Handle);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self
            .0
            .stop_completion(true)
            .recv_timeout(Duration::from_secs(10));
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
        _ => {
            return Err(
                "Usage: native_speed_smoke codex|claude EMPTY_CWD [MODEL] [fast|ultrafast]".into(),
            )
        }
    };
    let cwd = std::path::PathBuf::from(args.get(2).ok_or("Missing cwd")?)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if fs_err::read_dir(&cwd)
        .map_err(|e| e.to_string())?
        .next()
        .is_some()
    {
        return Err("Use empty disposable cwd".into());
    }
    grove_core::env_path::ensure_login_path();
    let mut saved: SavedSession = serde_json::from_value(serde_json::json!({"provider":provider,"key":grove_core::identity::random_id()?,"project":"Speed smoke","cwd":cwd,"label":"Smoke","roots":[],"thread_id":null,"items":[]})).map_err(|e|e.to_string())?;
    saved.composer = codex::ComposerSettings::default();
    saved.composer.model = args.get(3).filter(|m| m.as_str() != "default").cloned();
    saved.composer.effort = Some("low".into());
    saved.draft = "Preserve this unsent draft".into();
    let c = codex::connect(saved, Vec::new());
    let _cleanup = Cleanup(c.handle.clone());
    let s = wait(&c, |s| s.status == Status::Idle && !s.models.is_empty())?;
    let accelerated = if args.get(4).is_some_and(|a| a == "ultrafast") {
        Speed::Ultrafast
    } else {
        Speed::Fast
    };
    println!(
        "provider={provider:?} model={:?} fast_available={:?} catalog={:?}",
        s.model.as_ref().or(s.resolved_model.as_ref()),
        s.speed_unavailable(accelerated),
        s.speed_model()
    );
    if let Some(reason) = s.speed_unavailable(accelerated) {
        return Err(format!("UNSUPPORTED: {reason}"));
    }
    let key = s.saved.key.clone();
    for (index, speed) in [Speed::Standard, accelerated, Speed::Standard]
        .into_iter()
        .enumerate()
    {
        c.handle.send(Action::SetSpeed(speed))?;
        println!("turn={} selected={speed:?}; submitting", index + 1);
        let submission = c
            .handle
            .project_prompt("Reply exactly OK. Do not use tools.".into())?;
        let s = wait(&c, |s| {
            s.accepted_submission == Some(submission)
                && s.saved.composer.speed == Some(speed)
                && !s.status.busy()
                && s.turn_id.is_none()
                && s.queued_prompts.is_empty()
        })?;
        assert_eq!(s.saved.draft, "Preserve this unsent draft");
        assert_eq!(s.saved.composer.speed, Some(speed));
        if provider == Provider::Claude
            && speed == Speed::Fast
            && s.reported_speed.as_deref() != Some("on")
        {
            let known_restriction = matches!(
                s.speed_disabled_reason.as_deref(),
                Some(
                    "preference"
                        | "extra_usage_disabled"
                        | "free"
                        | "not_first_party"
                        | "disabled_by_env"
                        | "model_not_allowed"
                )
            );
            if (s.reported_speed.as_deref() == Some("off") && known_restriction)
                || s.reported_speed.as_deref() == Some("cooldown")
            {
                println!("KNOWN_UNAVAILABLE provider=Claude state={:?} reason={:?}; request/turn/persistence checks continue, accelerated inference is unverified", s.reported_speed,s.speed_disabled_reason);
            } else {
                return Err(format!(
                    "Unexpected Fast activation state={:?}, reason={:?}",
                    s.reported_speed, s.speed_disabled_reason
                ));
            }
        }
        if provider == Provider::Claude
            && speed == Speed::Standard
            && s.reported_speed.as_deref() == Some("on")
        {
            return Err("Claude reported Fast after explicit Standard selection".into());
        }
        let recovered: SavedSession =
            serde_json::from_value(serde_json::to_value(&s.saved).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        assert_eq!(recovered.composer.speed, Some(speed));
        println!("turn={} requested={speed:?} provider_report={:?} disabled_reason={:?} draft_preserved=true serialized=true", index+1, s.reported_speed, s.speed_disabled_reason);
    }
    c.handle
        .stop_completion(false)
        .recv_timeout(Duration::from_secs(10))
        .map_err(|e| e.to_string())?;
    let recovered = codex::load_sessions()?
        .sessions
        .into_iter()
        .find(|s| s.key == key)
        .ok_or("Saved speed smoke record missing on disk")?;
    assert_eq!(recovered.composer.speed, Some(Speed::Standard));
    assert_eq!(recovered.draft, "Preserve this unsent draft");
    println!("disk_persistence=true requested=Standard draft_preserved=true");
    let resumed = codex::connect(recovered, Vec::new());
    let _resume_cleanup = Cleanup(resumed.handle.clone());
    let resumed_state = wait(&resumed, |s| s.status == Status::Idle)?;
    assert_eq!(resumed_state.saved.composer.speed, Some(Speed::Standard));
    println!("reconnect=true preference=Standard");
    Ok(())
}
