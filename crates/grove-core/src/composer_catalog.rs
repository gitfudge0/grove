//! Shared, bounded metadata-only provider discovery. No conversations are retained.
use crate::codex::{self, SavedSession, Snapshot};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::{Duration, Instant},
};

const TTL: Duration = Duration::from_mins(5);
const MAX_SCOPES: usize = 32;

/// Checkout configuration must never cross a provider or launch destination.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Scope {
    claude: bool,
    cwd: String,
    args: Vec<String>,
    config_args: Vec<String>,
    config_version: u8,
    roots: Vec<String>,
    environment: u64,
}
impl Scope {
    pub fn new(saved: &SavedSession, args: &[String]) -> Self {
        // Fingerprint exports without retaining or logging credential values.
        let mut environment = std::collections::hash_map::DefaultHasher::new();
        crate::env_path::session_environment().hash(&mut environment);
        std::env::var_os("GROVE_CONFIG_DIR").hash(&mut environment);
        Self {
            environment: environment.finish(),
            claude: saved.provider == codex::Provider::Claude,
            cwd: saved.cwd.clone(),
            args: args.to_vec(),
            config_args: saved.config_args.clone(),
            config_version: saved.native_config_version,
            roots: saved
                .roots
                .iter()
                .map(|root| root.wt_path.clone())
                .collect(),
        }
    }
}

enum State {
    Loading,
    Finished(Box<Result<Snapshot, String>>, Instant),
}
struct Entry {
    skills_revision: u64,
    state: Mutex<State>,
    changed: Condvar,
}
#[derive(Default)]
struct Cache(Mutex<HashMap<Scope, Arc<Entry>>>);
impl Cache {
    fn peek(&self, scope: &Scope) -> Option<Snapshot> {
        let entries = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = entries.get(scope)?;
        let state = entry
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*state {
            State::Finished(result, at) if at.elapsed() < TTL => {
                result.as_ref().as_ref().ok().cloned()
            }
            _ => None,
        }
    }
    fn invalidate(&self, scope: &Scope) {
        let mut entries = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // An explicit retry joins an existing handshake instead of starting another process.
        if entries.get(scope).is_some_and(|entry| {
            matches!(
                *entry
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                State::Finished(..)
            )
        }) {
            entries.remove(scope);
        }
    }
    fn discover(
        &self,
        scope: Scope,
        load: impl FnOnce() -> Result<Snapshot, String>,
    ) -> Result<Snapshot, String> {
        let skills_revision = skill_revision_with_args(&scope.cwd, &scope.args, &scope.config_args);
        let (entry, owner) = {
            let mut entries = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if entries.get(&scope).is_some_and(|entry| {
                match &*entry
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                {
                    State::Loading => entry.skills_revision != skills_revision,
                    State::Finished(result, at) => {
                        result.is_err()
                            || at.elapsed() >= TTL
                            || entry.skills_revision != skills_revision
                    }
                }
            }) {
                entries.remove(&scope);
            }
            if let Some(entry) = entries.get(&scope) {
                (entry.clone(), false)
            } else {
                // Bound finished scopes; in-flight scopes may temporarily exceed this cap.
                if entries.len() >= MAX_SCOPES {
                    let oldest = entries
                        .iter()
                        .filter_map(|(key, entry)| {
                            match &*entry
                                .state
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                            {
                                State::Finished(_, at) => Some((key.clone(), *at)),
                                State::Loading => None,
                            }
                        })
                        .min_by_key(|(_, at)| *at)
                        .map(|(key, _)| key);
                    if let Some(oldest) = oldest {
                        entries.remove(&oldest);
                    }
                }
                let entry = Arc::new(Entry {
                    skills_revision,
                    state: Mutex::new(State::Loading),
                    changed: Condvar::new(),
                });
                entries.insert(scope, entry.clone());
                (entry, true)
            }
        };
        if owner {
            let result = load();
            *entry
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                State::Finished(Box::new(result.clone()), Instant::now());
            entry.changed.notify_all();
            result
        } else {
            let mut state = entry
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                match &*state {
                    State::Finished(result, _) => return result.as_ref().clone(),
                    State::Loading => {
                        state = entry
                            .changed
                            .wait(state)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                }
            }
        }
    }
}
fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Cache::default)
}

/// A ready snapshot contains only discovery metadata, never the caller's draft or preferences.
/// Callers copy the metadata fields they own into their existing session.
pub fn cached(saved: &SavedSession, args: &[String]) -> Option<Snapshot> {
    if !std::path::Path::new(&saved.cwd).is_dir() {
        return None;
    }
    cache().peek(&Scope::new(saved, args))
}
pub fn invalidate(saved: &SavedSession, args: &[String]) {
    cache().invalidate(&Scope::new(saved, args));
}

/// Background callers join one metadata handshake per scope. Failures remain retryable.
pub fn discover(mut saved: SavedSession, args: Vec<String>) -> Result<Snapshot, String> {
    if !std::path::Path::new(&saved.cwd).is_dir() {
        return Err("The selected checkout is no longer available.".into());
    }
    let scope = Scope::new(&saved, &args);
    saved = metadata_request(saved);
    cache().discover(scope, || codex::discover_composer(saved, args))
}

/// Fingerprint skill sources off the UI thread, following symlinked installations.
/// Ancestor roots are included because both CLIs discover project skills upwards.
pub fn skill_revision(cwd: &str) -> u64 {
    skill_revision_with_args(cwd, &[], &[])
}

pub fn skill_revision_with_args(cwd: &str, args: &[String], config_args: &[String]) -> u64 {
    use std::path::PathBuf;
    let mut roots = Vec::new();
    let home = dirs::home_dir().unwrap_or_default();
    let environment = crate::env_path::session_environment();
    let configured = |key: &str| -> Option<PathBuf> {
        environment
            .as_ref()
            .and_then(|vars| vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.into()))
            .or_else(|| std::env::var_os(key).map(PathBuf::from))
    };
    let codex_home = configured("CODEX_HOME").unwrap_or_else(|| home.join(".codex"));
    roots.extend([
        codex_home.join("plugins/cache"),
        codex_home.join("config.toml"),
    ]);
    roots.push(
        configured("CODEX_HOME")
            .unwrap_or_else(|| home.join(".codex"))
            .join("skills"),
    );
    roots.push(home.join(".agents/skills"));
    let claude = configured("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude"));
    roots.extend([
        claude.join("skills"),
        claude.join("commands"),
        claude.join("plugins/cache"),
        claude.join("plugins/installed_plugins.json"),
        claude.join("settings.json"),
    ]);
    for options in [args, config_args] {
        for (index, option) in options.iter().enumerate() {
            let plugin = option
                .strip_prefix("--plugin-dir=")
                .map(String::from)
                .or_else(|| {
                    (option == "--plugin-dir")
                        .then(|| options.get(index + 1).cloned())
                        .flatten()
                });
            if let Some(plugin) = plugin {
                roots.push(std::path::Path::new(cwd).join(plugin));
            }
        }
    }
    for ancestor in std::path::Path::new(cwd).ancestors() {
        for suffix in [
            ".codex/skills",
            ".agents/skills",
            ".claude/skills",
            ".claude/commands",
            ".claude/settings.json",
        ] {
            roots.push(ancestor.join(suffix));
        }
    }
    fingerprint_roots(&roots)
}

fn fingerprint_roots(roots: &[std::path::PathBuf]) -> u64 {
    fn visit(
        path: &std::path::Path,
        hash: &mut impl Hasher,
        visited: &mut std::collections::HashSet<std::path::PathBuf>,
        depth: usize,
    ) {
        if depth > 32 {
            return;
        }
        path.hash(hash);
        let Ok(real) = path.canonicalize() else {
            return;
        };
        if !visited.insert(real.clone()) {
            return;
        }
        if real.is_dir() {
            let definition = real.join("SKILL.md");
            if definition.is_file() {
                if let Ok(contents) = fs_err::read(definition) {
                    contents.hash(hash);
                }
                return;
            }
            if let Ok(entries) = fs_err::read_dir(&real) {
                let mut entries: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
                entries.sort();
                for entry in entries {
                    if !entry.file_name().is_some_and(|name| {
                        name == ".git" || name == "node_modules" || name == "target"
                    }) || entry.join("SKILL.md").is_file()
                    {
                        visit(&entry, hash, visited, depth + 1);
                    }
                }
            }
        } else if real
            .extension()
            .is_some_and(|ext| ext == "md" || ext == "json" || ext == "toml")
        {
            if let Ok(contents) = fs_err::read(real) {
                contents.hash(hash);
            }
        }
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let mut visited = std::collections::HashSet::new();
    for root in roots {
        visit(root, &mut hash, &mut visited, 0);
    }
    hash.finish()
}

fn metadata_request(mut saved: SavedSession) -> SavedSession {
    saved.thread_id = None;
    saved.claude_subagents.clear();
    saved.rich_plans = crate::rich_plan::PlanStore::default();
    saved.edit_checkpoints.clear();
    saved.pending_edit = None;
    saved.title = None;
    saved.title_generation = None;
    saved.context = None;
    saved.claude_resume = None;
    saved.items.clear();
    saved.work_periods.clear();
    saved.draft.clear();
    saved.draft_images.clear();
    saved.draft_reference = None;
    saved.composer = codex::ComposerSettings::default();
    saved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> SavedSession {
        serde_json::from_value(serde_json::json!({
            "provider":"codex", "key":"catalog-test", "project":"Project",
            "cwd":"/unused", "label":"Codex", "roots":[], "thread_id":null, "items":[]
        }))
        .unwrap()
    }
    fn result() -> Snapshot {
        let mut state = Snapshot::new(saved());
        state.models = vec![
            serde_json::json!({"model":"advertised", "supportedReasoningEfforts":[{"reasoningEffort":"high"}]}),
        ];
        state
    }

    #[test]
    fn installed_edited_and_removed_skills_expire_warm_catalog_for_both_providers() {
        let temp = tempfile::tempdir().unwrap();
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let cache = Cache::default();
            let mut saved = saved();
            saved.provider = provider;
            saved.cwd = temp.path().to_string_lossy().into_owned();
            let scope = Scope::new(&saved, &[]);
            cache.discover(scope.clone(), || Ok(result())).unwrap();
            let definition = temp.path().join(".agents/skills/new-skill/SKILL.md");
            fs_err::create_dir_all(definition.parent().unwrap()).unwrap();
            fs_err::write(&definition, "---\nname: new-skill\n---\nFirst").unwrap();
            let mut changed = result();
            changed.skills = vec![serde_json::json!({"name":"new-skill"})];
            assert_eq!(
                cache
                    .discover(scope.clone(), || Ok(changed))
                    .unwrap()
                    .skills
                    .len(),
                1
            );
            fs_err::write(&definition, "---\nname: renamed-skill\n---\nSecond").unwrap();
            let mut changed = result();
            changed.skills = vec![serde_json::json!({"name":"renamed-skill"})];
            assert_eq!(
                cache
                    .discover(scope.clone(), || Ok(changed))
                    .unwrap()
                    .skills[0]["name"],
                "renamed-skill"
            );
            fs_err::remove_file(definition).unwrap();
            assert!(cache
                .discover(scope, || Ok(result()))
                .unwrap()
                .skills
                .is_empty());
        }
    }

    #[test]
    fn skill_names_matching_asset_directories_are_still_discovered() {
        let temp = tempfile::tempdir().unwrap();
        let roots = [temp.path().to_path_buf()];
        let before = fingerprint_roots(&roots);
        let skill = temp.path().join("target/SKILL.md");
        fs_err::create_dir_all(skill.parent().unwrap()).unwrap();
        fs_err::write(skill, "---\nname: target\n---").unwrap();
        assert_ne!(before, fingerprint_roots(&roots));
    }

    #[test]
    fn explicit_plugin_installation_refreshes_source_revision() {
        let temp = tempfile::tempdir().unwrap();
        let plugin = temp.path().join("plugin");
        let args = vec!["--plugin-dir".into(), plugin.to_string_lossy().into_owned()];
        let cwd = temp.path().to_str().unwrap();
        let before = skill_revision_with_args(cwd, &args, &[]);
        let skill = plugin.join("skills/new/SKILL.md");
        fs_err::create_dir_all(skill.parent().unwrap()).unwrap();
        fs_err::write(&skill, "---\nname: plugin-skill\n---").unwrap();
        assert_ne!(before, skill_revision_with_args(cwd, &args, &[]));
        fs_err::remove_file(skill).unwrap();
        assert_ne!(before, skill_revision_with_args(cwd, &args, &[])); // directory install remains visible
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_global_skill_definitions_and_cycles_are_detected() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let root = temp.path().join("skills");
        fs_err::create_dir_all(&source).unwrap();
        fs_err::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        let before = fingerprint_roots(std::slice::from_ref(&root));
        fs_err::write(source.join("SKILL.md"), "first").unwrap();
        std::os::unix::fs::symlink(&source, root.join("linked")).unwrap();
        let installed = fingerprint_roots(std::slice::from_ref(&root));
        assert_ne!(before, installed);
        fs_err::write(source.join("SKILL.md"), "other").unwrap();
        assert_ne!(installed, fingerprint_roots(std::slice::from_ref(&root)));
        fs_err::remove_file(root.join("linked")).unwrap();
        assert_eq!(before, fingerprint_roots(&[root]));
    }

    #[test]
    fn ready_catalog_reuses_models_and_efforts_until_expiry_or_retry() {
        let cache = Cache::default();
        let scope = Scope::new(&saved(), &[]);
        cache.discover(scope.clone(), || Ok(result())).unwrap();
        let state = cache
            .discover(scope.clone(), || panic!("warm catalog spawned a provider"))
            .unwrap();
        assert_eq!(
            state.models[0]["supportedReasoningEfforts"][0]["reasoningEffort"],
            "high"
        );
        let entry = cache.0.lock().unwrap().get(&scope).unwrap().clone();
        *entry.state.lock().unwrap() = State::Finished(
            Box::new(Ok(result())),
            Instant::now().checked_sub(TTL).unwrap(),
        );
        assert!(cache.peek(&scope).is_none());
        let mut changed = result();
        changed.models.clear();
        assert!(cache
            .discover(scope.clone(), || Ok(changed))
            .unwrap()
            .models
            .is_empty());
        cache.invalidate(&scope);
        assert!(cache.peek(&scope).is_none());
        assert!(!cache
            .discover(scope, || Ok(result()))
            .unwrap()
            .models
            .is_empty());
    }

    #[test]
    fn overlapping_discovery_joins_one_handshake() {
        let cache = Arc::new(Cache::default());
        let scope = Scope::new(&saved(), &[]);
        let (started, loading) = std::sync::mpsc::channel();
        let (release, finish) = std::sync::mpsc::channel();
        let owner = std::thread::spawn({
            let (cache, scope) = (cache.clone(), scope.clone());
            move || {
                cache.discover(scope, || {
                    started.send(()).unwrap();
                    finish.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(result())
                })
            }
        });
        loading.recv_timeout(Duration::from_secs(5)).unwrap();
        // Retry must not evict the in-flight owner and duplicate its process.
        cache.invalidate(&scope);
        let follower = std::thread::spawn({
            let (cache, scope) = (cache.clone(), scope.clone());
            move || {
                cache.discover(scope, || {
                    panic!("overlapping discovery started another provider")
                })
            }
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let joined = loop {
            // Map + owner + follower prove the follower acquired this very entry.
            let joined = cache
                .0
                .lock()
                .unwrap()
                .get(&scope)
                .is_some_and(|entry| Arc::strong_count(entry) >= 3);
            if joined || Instant::now() >= deadline {
                break joined;
            }
            std::thread::yield_now();
        };
        release.send(()).unwrap();
        assert!(joined, "follower never joined the in-flight handshake");
        assert_eq!(owner.join().unwrap().unwrap().models, result().models);
        assert_eq!(follower.join().unwrap().unwrap().models, result().models);
    }

    #[test]
    fn provider_switch_reuses_only_its_ready_catalog_and_efforts() {
        let cwd = tempfile::tempdir().unwrap();
        let mut request = saved();
        request.cwd = cwd.path().to_string_lossy().into_owned();
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            request.provider = provider;
            let mut state = Snapshot::new(metadata_request(request.clone()));
            state.models = vec![serde_json::json!({
                "model":provider.label(),
                "supportedReasoningEfforts":[{"reasoningEffort":"high"}]
            })];
            cache()
                .discover(Scope::new(&request, &[]), || Ok(state))
                .unwrap();
        }
        // A picker-selected model does not change scope; switching back also hits.
        for provider in [codex::Provider::Claude, codex::Provider::Codex] {
            request.provider = provider;
            request.composer.model = Some(provider.label().into());
            let ready = cached(&request, &[]).expect("provider switch should be immediately ready");
            assert_eq!(ready.models[0]["model"], provider.label());
            assert_eq!(
                ready.models[0]["supportedReasoningEfforts"][0]["reasoningEffort"],
                "high"
            );
            assert_eq!(
                discover(request.clone(), Vec::new()).unwrap().models,
                ready.models
            );
        }
    }

    #[test]
    fn discovery_failure_can_be_retried() {
        let cache = Cache::default();
        let scope = Scope::new(&saved(), &[]);
        assert!(cache
            .discover(scope.clone(), || Err("offline".into()))
            .is_err());
        assert!(cache.peek(&scope).is_none());
        assert!(cache.discover(scope, || Ok(result())).is_ok());
    }

    #[test]
    fn scope_isolates_destinations_and_configuration_but_not_selection() {
        let original = saved();
        let scope = Scope::new(&original, &[]);
        let mut selected = original.clone();
        selected.composer.model = Some("chosen".into());
        selected.composer.effort = Some("high".into());
        selected.draft = "private draft".into();
        assert_eq!(Scope::new(&selected, &[]), scope);
        selected.cwd = "/other".into();
        assert_ne!(Scope::new(&selected, &[]), scope);
        selected = original.clone();
        selected.provider = codex::Provider::Claude;
        assert_ne!(Scope::new(&selected, &[]), scope);
        selected = original.clone();
        selected.config_args.push("--config".into());
        assert_ne!(Scope::new(&selected, &[]), scope);
        assert_ne!(Scope::new(&original, &["--different".into()]), scope);
        selected = original.clone();
        selected.roots.push(crate::session_meta::ContextRoot {
            project: "Other".into(),
            wt_path: "/other".into(),
        });
        assert_ne!(Scope::new(&selected, &[]), scope);
        selected = original.clone();
        selected.native_config_version = selected.native_config_version.wrapping_add(1);
        assert_ne!(Scope::new(&selected, &[]), scope);
    }

    #[test]
    fn shared_metadata_does_not_retain_draft_or_selected_preferences() {
        let mut request = saved();
        request.thread_id = Some("conversation".into());
        request.draft = "private draft".into();
        request.draft_images = vec!["image".into()];
        request.composer.model = Some("selected".into());
        request.composer.effort = Some("high".into());
        let request = metadata_request(request);
        assert!(request.thread_id.is_none());
        assert!(request.draft.is_empty());
        assert!(request.draft_images.is_empty());
        assert!(request.composer.model.is_none());
        assert!(request.composer.effort.is_none());
    }

    #[test]
    fn removed_checkout_cannot_reuse_a_ready_catalog() {
        let cwd = tempfile::tempdir().unwrap();
        let mut request = saved();
        request.cwd = cwd.path().to_string_lossy().into_owned();
        let scope = Scope::new(&request, &[]);
        cache().discover(scope, || Ok(result())).unwrap();
        assert!(cached(&request, &[]).is_some());
        drop(cwd);
        assert!(cached(&request, &[]).is_none());
        assert!(discover(request, Vec::new())
            .unwrap_err()
            .contains("no longer available"));
    }
}
