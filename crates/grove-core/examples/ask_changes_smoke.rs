//! Read-only live-provider smoke; run with an explicitly supplied disposable checkout.
use grove_core::{
    change_description::Scope, change_questions, codex::Provider, commit_agent::CommitControl,
};
fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    let path = args
        .get(1)
        .ok_or("Usage: ask_changes_smoke <checkout> <codex|claude> <question>")?;
    let provider = match args.get(2).map(String::as_str) {
        Some("codex") => Provider::Codex,
        Some("claude") => Provider::Claude,
        _ => return Err("Choose codex or claude".into()),
    };
    let question = args.get(3).ok_or("Provide a question")?;
    let answer = change_questions::generate(
        std::path::Path::new(path),
        &Scope::Local,
        question,
        provider,
        &CommitControl::default(),
    )?;
    println!("{}", answer.markdown());
    Ok(())
}
