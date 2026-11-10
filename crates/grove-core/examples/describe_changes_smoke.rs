//! Explicit read-only provider smoke in an isolated temporary Git repository.
//! Run: cargo run -p grove-core --example describe_changes_smoke -- codex|claude [PR-URL]
use grove_core::{
    change_description::{self, Scope},
    codex::Provider,
    commit_agent::CommitControl,
};
use std::{path::Path, process::Command};
fn git(path: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("Fixture Git operation failed".into());
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}
fn main() -> Result<(), String> {
    let provider = match std::env::args().nth(1).as_deref() {
        Some("codex") => Provider::Codex,
        Some("claude") => Provider::Claude,
        _ => return Err("Usage: describe_changes_smoke codex|claude [PR-URL]".into()),
    };
    let fixture = tempfile::tempdir().map_err(|e| e.to_string())?;
    let path = fixture.path();
    git(path, &["init", "-b", "main"])?;
    git(path, &["config", "user.name", "Grove Smoke"])?;
    git(path, &["config", "user.email", "grove-smoke@example.test"])?;
    fs_err::write(
        path.join("handler.ts"),
        "export function shouldProcess(seen: boolean) { return true; }\n",
    )
    .map_err(|e| e.to_string())?;
    git(path, &["add", "."])?;
    git(path, &["commit", "-m", "Fixture"])?;
    fs_err::write(
        path.join("handler.ts"),
        "export function shouldProcess(seen: boolean) { return !seen; }\n",
    )
    .map_err(|e| e.to_string())?;
    let scope = std::env::args()
        .nth(2)
        .map_or(Scope::Local, |url| Scope::PullRequest { url });
    let before = git(path, &["status", "--porcelain=v1"])?;
    let head = git(path, &["rev-parse", "HEAD"])?;
    let index = git(path, &["ls-files", "--stage"])?;
    let result =
        change_description::generate(path, &scope, provider, &CommitControl::default(), |phase| {
            eprintln!("{phase:?}");
        })?;
    if result.files.is_empty()
        || (scope == Scope::Local
            && (result.files.len() != 1 || result.files[0].path != "handler.ts"))
        || result.snapshot.scope != scope
        || result.summary.is_empty()
        || !change_description::is_current(path, &result)?
    {
        return Err("Description coverage or freshness failed".into());
    }
    if git(path, &["status", "--porcelain=v1"])? != before
        || git(path, &["rev-parse", "HEAD"])? != head
        || git(path, &["ls-files", "--stage"])? != index
    {
        return Err("Generation changed the temporary repository".into());
    }
    println!("PASS {provider:?}: {} described files; snapshot stayed current; HEAD, index, and working state unchanged", result.files.len());
    Ok(())
}
