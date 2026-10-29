//! Opt-in catalog-only smoke: no prompt, conversation, tool approval or persisted session.
use grove_core::{
    codex::{Provider, SavedSession},
    composer_catalog,
};
use std::time::Instant;

fn main() -> Result<(), String> {
    if std::env::var_os("GROVE_CONFIG_DIR").is_none() {
        return Err("Set disposable GROVE_CONFIG_DIR".into());
    }
    let args: Vec<_> = std::env::args().collect();
    let provider = match args.get(1).map(String::as_str) {
        Some("codex") => Provider::Codex,
        Some("claude") => Provider::Claude,
        _ => return Err("Usage: native_catalog_smoke codex|claude EMPTY_CWD".into()),
    };
    let cwd = std::path::PathBuf::from(args.get(2).ok_or("Missing cwd")?)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if fs_err::read_dir(&cwd)
        .map_err(|error| error.to_string())?
        .next()
        .is_some()
    {
        return Err("Use empty disposable cwd".into());
    }
    grove_core::env_path::ensure_login_path();
    let mut saved: SavedSession = serde_json::from_value(serde_json::json!({
        "provider":provider, "key":grove_core::identity::random_id()?, "project":"Catalog smoke",
        "cwd":cwd, "label":"Smoke", "roots":[], "thread_id":null, "items":[], "native_config_version":1
    })).map_err(|error| error.to_string())?;
    let launch_args = provider.agent().launch_args_for_backend(true, false, false);
    let started = Instant::now();
    let cold = composer_catalog::discover(saved.clone(), launch_args.clone())?;
    let cold_ms = started.elapsed().as_secs_f64() * 1000.;
    if cold.models.is_empty() {
        return Err("Provider returned no models".into());
    }
    if cold.saved.thread_id.is_some() || !cold.saved.items.is_empty() {
        return Err("Discovery created conversation state".into());
    }
    let chosen = cold
        .models
        .iter()
        .find_map(|model| model["model"].as_str())
        .ok_or("No model IDs")?;
    saved.composer.model = Some(chosen.into());
    saved.composer.effort = Some("high".into());
    saved.draft = "Unsent smoke draft".into();
    let started = Instant::now();
    let cached = composer_catalog::cached(&saved, &launch_args)
        .ok_or("Ready metadata not immediately reusable")?;
    let warm = composer_catalog::discover(saved.clone(), launch_args)?;
    let warm_ms = started.elapsed().as_secs_f64() * 1000.;
    assert_eq!(cold.models, cached.models);
    assert_eq!(cold.models, warm.models);
    assert!(warm.saved.composer.model.is_none());
    assert!(warm.saved.draft.is_empty());
    assert_eq!(saved.draft, "Unsent smoke draft");
    let efforts: usize = warm
        .models
        .iter()
        .filter_map(|model| model["supportedReasoningEfforts"].as_array())
        .map(Vec::len)
        .sum();
    println!("provider={provider:?} cold_ms={cold_ms:.3} warm_lookup_and_discovery_ms={warm_ms:.3} models={} effort_options={efforts} conversation_created=false", warm.models.len());
    // This is one cold/warm observation, not a cross-revision performance benchmark.
    Ok(())
}
