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
                    State::Loading => false,
                    State::Finished(result, at) => result.is_err() || at.elapsed() >= TTL,
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
