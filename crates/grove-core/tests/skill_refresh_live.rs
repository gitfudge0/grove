//! Opt-in acceptance against installed provider CLIs; creates temporary skills.
use grove_core::{
    codex::{Provider, SavedSession},
    composer_catalog,
};

#[test]
#[ignore = "Launches installed authenticated Codex and Claude metadata discovery"]
fn installed_skills_refresh_without_restart() {
    let mut failures = Vec::new();
    for provider in [Provider::Codex, Provider::Claude] {
        let outcome = check(provider);
        println!("{}: {outcome:?}", provider.label());
        if let Err(error) = outcome {
            failures.push(format!("{}: {error}", provider.label()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn check(provider: Provider) -> Result<(), String> {
    let checkout = tempfile::tempdir().map_err(|e| e.to_string())?;
    let cwd = checkout.path().to_string_lossy().into_owned();
    let saved: SavedSession = serde_json::from_value(serde_json::json!({
        "provider": if provider == Provider::Codex { "codex" } else { "claude" },
        "key":"skill-refresh-live", "project":"Skill refresh QA", "cwd":cwd,
        "label":provider.label(), "roots":[], "thread_id":null, "items":[]
    }))
    .map_err(|e| e.to_string())?;
    let args = provider.agent().launch_args_for_backend(true, true, false);
    let initial = composer_catalog::discover(saved.clone(), args.clone())?;
    let home = dirs::home_dir().ok_or("Home unavailable")?;
    let (local_root, global_root) = match provider {
        Provider::Codex => (
            checkout.path().join(".agents/skills"),
            std::env::var_os("CODEX_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| home.join(".codex"))
                .join("skills"),
        ),
        Provider::Claude => (
            checkout.path().join(".claude/skills"),
            std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| home.join(".claude"))
                .join("skills"),
        ),
    };
    std::fs::create_dir_all(&local_root).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&global_root).map_err(|e| e.to_string())?;
    let local = tempfile::Builder::new()
        .prefix("grove-live-local-")
        .tempdir_in(local_root)
        .map_err(|e| e.to_string())?;
    let global = tempfile::Builder::new()
        .prefix("grove-live-global-")
        .tempdir_in(global_root)
        .map_err(|e| e.to_string())?;
    let names: Vec<_> = [&local, &global]
        .iter()
        .map(|dir| {
            dir.path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    for (dir, name) in [&local, &global].iter().zip(&names) {
        std::fs::write(dir.path().join("SKILL.md"), format!("---\nname: {name}\ndescription: Temporary Grove discovery verification skill.\n---\nReturn the word verified.\n")).map_err(|e| e.to_string())?;
        assert!(!initial.skills.iter().any(|skill| skill["name"] == *name));
    }
    let refreshed = composer_catalog::discover(saved.clone(), args.clone())?;
    let missing: Vec<_> = names
        .iter()
        .filter(|name| !refreshed.skills.iter().any(|skill| skill["name"] == **name))
        .cloned()
        .collect();
    drop(local);
    drop(global);
    if !missing.is_empty() {
        return Err(format!("Installed skills absent: {missing:?}"));
    }
    let removed = composer_catalog::discover(saved, args)?;
    for name in names {
        if removed.skills.iter().any(|skill| skill["name"] == name) {
            return Err(format!("Removed skill remains: {name}"));
        }
    }
    Ok(())
}
