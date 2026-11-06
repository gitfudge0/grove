//! Opt-in live, read-only structured generation with isolated prompt settings.
//! cargo run -p grove-core --example custom_prompts_smoke -- codex|claude
use grove_core::{
    codex::Provider,
    prompts::{self, PromptKind},
    storage,
};
use std::{path::Path, process::Command};
fn git(path: &Path, args: &[&str]) -> Result<String, String> {
    let result = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&result.stdout).into_owned())
}
fn main() -> Result<(), String> {
    let provider = match std::env::args().nth(1).as_deref() {
        Some("codex") => Provider::Codex,
        Some("claude") => Provider::Claude,
        _ => return Err("Usage: custom_prompts_smoke codex|claude".into()),
    };
    let config = tempfile::tempdir().map_err(|e| e.to_string())?;
    std::env::set_var(storage::CONFIG_DIR_ENV, config.path());
    let fixture = tempfile::tempdir().map_err(|e| e.to_string())?;
    let path = fixture.path();
    git(path, &["init", "-b", "main"])?;
    git(path, &["config", "user.name", "Grove Smoke"])?;
    git(path, &["config", "user.email", "smoke@example.test"])?;
    fs_err::write(
        path.join("handler.ts"),
        "export const shouldProcess = true;\n",
    )
    .map_err(|e| e.to_string())?;
    git(path, &["add", "."])?;
    git(path, &["commit", "-m", "Fixture"])?;
    fs_err::write(
        path.join("handler.ts"),
        "export const shouldProcess = !seen;\n",
    )
    .map_err(|e| e.to_string())?;
    let before = git(path, &["status", "--porcelain=v1"])?;
    let head = git(path, &["rev-parse", "HEAD"])?;
    let index = git(path, &["ls-files", "--stage"])?;
    let patch = git(path, &["diff", "--", "handler.ts"])?;
    let mut store = storage::Store::default();
    for kind in [PromptKind::Commit, PromptKind::PullRequest] {
        store.prompt_overrides.insert(kind.key().into(),
            "Write in English. Begin the subject or title with the exact marker GROVE-SMOKE: followed by a concise behavior description. Use a concise body.".into());
    }
    storage::save(&store).map_err(|e| e.to_string())?;
    for kind in [PromptKind::Commit, PromptKind::PullRequest] {
        let result = prompts::generate_snapshot_text(
            kind,
            provider,
            &format!("<change-data>\n{patch}\n</change-data>"),
        )?;
        let title = result[if kind == PromptKind::Commit {
            "subject"
        } else {
            "title"
        }]
        .as_str()
        .ok_or("Missing title")?;
        if !title.starts_with("GROVE-SMOKE:") || !result["body"].is_string() {
            return Err(format!("Customized {kind:?} writing was not followed"));
        }
        println!("PASS {provider:?} {kind:?}: saved custom instructions reached isolated structured generation");
    }
    if before != git(path, &["status", "--porcelain=v1"])?
        || head != git(path, &["rev-parse", "HEAD"])?
        || index != git(path, &["ls-files", "--stage"])?
    {
        return Err("Structured generation changed the fixture repository".into());
    }
    println!("PASS HEAD, index and working state unchanged; actual commit and GitHub browser handoff not exercised");
    Ok(())
}
