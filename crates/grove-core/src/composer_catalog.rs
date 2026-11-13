//! Shared, bounded metadata-only provider discovery. No conversations are retained.
use crate::codex::{self, SavedSession, Snapshot};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::{Duration, Instant},
};

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

impl Scope {
    #[cfg(test)]
    fn saved(&self) -> SavedSession {
        let mut saved = SavedSession::empty(
            String::new(),
            if self.claude {
                codex::Provider::Claude
            } else {
                codex::Provider::Codex
            },
            String::new(),
            self.cwd.clone(),
        );
        saved.config_args.clone_from(&self.config_args);
        saved.native_config_version = self.config_version;
        saved.roots = self
            .roots
            .iter()
            .map(|path| crate::session_meta::ContextRoot {
                project: String::new(),
                wt_path: path.clone(),
            })
            .collect();
        saved
    }
}

/// One shared background fingerprint per scope and interval. No session state is retained.
/// Periodic fallback deliberately remains: it detects symlink targets and new directories
/// without platform-specific watcher gaps. Retry evicts this memo immediately.
const REVISION_INTERVAL: Duration = Duration::from_secs(5);
#[derive(Clone)]
pub struct DiscoveryInputs {
    scope: Scope,
    revision: u64,
    model_key: Option<crate::model_catalog::Key>,
    models_require_refresh: bool,
}
impl DiscoveryInputs {
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn models_require_refresh(&self) -> bool {
        self.models_require_refresh
    }
}
type RevisionEntry = Arc<Mutex<Option<(Instant, DiscoveryInputs)>>>;
#[derive(Default)]
struct Revisions(Mutex<HashMap<Scope, RevisionEntry>>);
impl Revisions {
    fn get(&self, scope: Scope, scan: impl FnOnce() -> DiscoveryInputs) -> DiscoveryInputs {
        let entry = {
            let mut entries = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if entries.len() >= MAX_SCOPES && !entries.contains_key(&scope) {
                // Never evict a scan in progress: readers must join its entry.
                let oldest = entries
                    .iter()
                    .filter_map(|(key, entry)| {
                        let state = entry.try_lock().ok()?;
                        Some((key.clone(), state.as_ref()?.0))
                    })
                    .min_by_key(|(_, at)| *at)
                    .map(|(key, _)| key);
                if let Some(oldest) = oldest {
                    entries.remove(&oldest);
                }
            }
            entries.entry(scope).or_default().clone()
        };
        // Serialize only matching scopes; unrelated checkouts never wait for this scan.
        let mut state = entry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, inputs)) = &*state {
            if at.elapsed() < REVISION_INTERVAL {
                return inputs.clone();
            }
        }
        let inputs = scan();
        *state = Some((Instant::now(), inputs.clone()));
        inputs
    }
    fn invalidate(&self, scope: &Scope) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(scope);
    }
}
fn revisions() -> &'static Revisions {
    static REVISIONS: OnceLock<Revisions> = OnceLock::new();
    REVISIONS.get_or_init(Revisions::default)
}
fn scan_inputs(saved: &SavedSession, args: &[String]) -> DiscoveryInputs {
    let model_key = crate::model_catalog::Key::new(saved, args);
    let mut revision = std::collections::hash_map::DefaultHasher::new();
    provider_skill_revision(saved.provider, &saved.cwd, args, &saved.config_args)
        .hash(&mut revision);
    let model_revision = crate::model_catalog::revision_for_key(model_key.as_ref());
    model_revision
        .as_ref()
        .map(|(fingerprint, _)| fingerprint)
        .hash(&mut revision);
    let models_require_refresh = model_revision.is_some_and(|(_, stale)| stale);
    DiscoveryInputs {
        scope: Scope::new(saved, args),
        revision: revision.finish(),
        model_key,
        models_require_refresh,
    }
}
/// Shared background-only input scan; never call from the foreground UI executor.
pub fn inputs(saved: &SavedSession, args: &[String]) -> DiscoveryInputs {
    revisions().get(Scope::new(saved, args), || scan_inputs(saved, args))
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
            State::Finished(result, _) => result.as_ref().as_ref().ok().cloned(),
            State::Loading => None,
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
    #[cfg(test)]
    fn discover(
        &self,
        scope: Scope,
        load: impl FnOnce() -> Result<Snapshot, String>,
    ) -> Result<Snapshot, String> {
        self.discover_at(scope.clone(), revision(&scope.saved(), &scope.args), load)
    }
    fn discover_at(
        &self,
        scope: Scope,
        skills_revision: u64,
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
                    State::Loading => entry.skills_revision != skills_revision,
                    State::Finished(result, _) => {
                        result.is_err() || entry.skills_revision != skills_revision
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
    if crate::model_catalog::requires_refresh(saved, args) {
        return None;
    }
    let mut snapshot = cache().peek(&Scope::new(saved, args))?;
    if let Some(models) = crate::model_catalog::cached(saved, args) {
        snapshot.models = models;
    }
    Some(snapshot)
}
pub fn invalidate(saved: &SavedSession, args: &[String]) {
    let scope = Scope::new(saved, args);
    revisions().invalidate(&scope);
    cache().invalidate(&scope);
    crate::model_catalog::invalidate(saved, args);
}

/// The UI can reuse remembered model rows without filesystem work or a provider process.
pub fn cached_models(saved: &SavedSession, args: &[String]) -> Option<Vec<serde_json::Value>> {
    crate::model_catalog::cached(saved, args)
}
/// Whether remembered rows need background refresh while remaining displayable.
pub fn models_require_refresh(saved: &SavedSession, args: &[String]) -> bool {
    crate::model_catalog::is_stale(saved, args)
}

/// Model browsing does not need checkout skills or configured defaults. Disk hydration is
/// deliberately on the caller's background thread; only a missing catalog starts discovery.
pub fn discover_models(
    saved: SavedSession,
    args: Vec<String>,
) -> Result<Vec<serde_json::Value>, String> {
    if let Some(models) = crate::model_catalog::Key::new(&saved, &args)
        .as_ref()
        .and_then(crate::model_catalog::load)
    {
        return Ok(models);
    }
    discover(saved, args).map(|snapshot| snapshot.models)
}

/// Background callers join one metadata handshake per scope. Failures remain retryable.
pub fn discover(saved: SavedSession, args: Vec<String>) -> Result<Snapshot, String> {
    let inputs = inputs(&saved, &args);
    discover_with_inputs(saved, args, inputs)
}
/// Reuse the watcher's input snapshot rather than scanning configuration twice.
pub fn discover_with_inputs(
    mut saved: SavedSession,
    args: Vec<String>,
    inputs: DiscoveryInputs,
) -> Result<Snapshot, String> {
    let inputs = if inputs.scope == Scope::new(&saved, &args) {
        inputs
    } else {
        self::inputs(&saved, &args)
    };
    if !std::path::Path::new(&saved.cwd).is_dir() {
        return Err("The selected checkout is no longer available.".into());
    }
    let scope = Scope::new(&saved, &args);
    let model_key = inputs.model_key;
    let current_models = model_key.as_ref().and_then(crate::model_catalog::load);
    if model_key.is_some() && current_models.is_none() {
        // Another workspace's ready snapshot cannot satisfy a missing/explicitly
        // invalidated model catalog. Keep coalescing an already-running handshake.
        cache().invalidate(&scope);
    }
    saved = metadata_request(saved);
    let mut snapshot = cache().discover_at(scope, inputs.revision, || {
        codex::discover_composer(saved, args)
    })?;
    if let Some(models) = model_key.as_ref().and_then(crate::model_catalog::load) {
        snapshot.models = models;
    }
    Ok(snapshot)
}

/// Background revision for all discovery inputs. Unlike model browsing, configured defaults
/// and skills remain workspace-specific and must be re-read when account/configuration changes.
pub fn revision(saved: &SavedSession, args: &[String]) -> u64 {
    scan_inputs(saved, args).revision
}

/// Fingerprint skill sources off the UI thread, following symlinked installations.
/// Ancestor roots are included because both CLIs discover project skills upwards.
pub fn skill_revision(cwd: &str) -> u64 {
    skill_revision_with_args(cwd, &[], &[])
}

pub fn skill_revision_with_args(cwd: &str, args: &[String], config_args: &[String]) -> u64 {
    // Legacy callers request the union; provider discovery uses only its own roots.
    let mut revision = std::collections::hash_map::DefaultHasher::new();
    provider_skill_revision(codex::Provider::Codex, cwd, args, config_args).hash(&mut revision);
    provider_skill_revision(codex::Provider::Claude, cwd, args, config_args).hash(&mut revision);
    revision.finish()
}
fn provider_skill_revision(
    provider: codex::Provider,
    cwd: &str,
    args: &[String],
    config_args: &[String],
) -> u64 {
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
    if provider == codex::Provider::Codex {
        let codex_home = configured("CODEX_HOME").unwrap_or_else(|| home.join(".codex"));
        roots.extend([
            codex_home.join("plugins/cache"),
            codex_home.join("config.toml"),
            codex_home.join("skills"),
        ]);
    } else {
        let claude = configured("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude"));
        roots.extend([
            claude.join("skills"),
            claude.join("commands"),
            claude.join("plugins/cache"),
            claude.join("plugins/installed_plugins.json"),
            claude.join("settings.json"),
        ]);
    }
    roots.push(home.join(".agents/skills"));
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
        let suffixes: &[&str] = if provider == codex::Provider::Codex {
            &[".codex/skills", ".agents/skills"]
        } else {
            &[
                ".agents/skills",
                ".claude/skills",
                ".claude/commands",
                ".claude/settings.json",
            ]
        };
        for suffix in suffixes {
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
    fn provider_skill_changes_do_not_invalidate_the_other_provider() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().to_str().unwrap();
        let codex_before = provider_skill_revision(codex::Provider::Codex, cwd, &[], &[]);
        let claude_before = provider_skill_revision(codex::Provider::Claude, cwd, &[], &[]);
        let skill = temp.path().join(".claude/skills/new/SKILL.md");
        fs_err::create_dir_all(skill.parent().unwrap()).unwrap();
        fs_err::write(&skill, "Claude skill").unwrap();
        assert_eq!(
            codex_before,
            provider_skill_revision(codex::Provider::Codex, cwd, &[], &[])
        );
        assert_ne!(
            claude_before,
            provider_skill_revision(codex::Provider::Claude, cwd, &[], &[])
        );
        let claude_after = provider_skill_revision(codex::Provider::Claude, cwd, &[], &[]);
        let skill = temp.path().join(".codex/skills/new/SKILL.md");
        fs_err::create_dir_all(skill.parent().unwrap()).unwrap();
        fs_err::write(&skill, "Codex skill").unwrap();
        assert_ne!(
            codex_before,
            provider_skill_revision(codex::Provider::Codex, cwd, &[], &[])
        );
        assert_eq!(
            claude_after,
            provider_skill_revision(codex::Provider::Claude, cwd, &[], &[])
        );
    }

    #[test]
    fn shared_revisions_coalesce_readers_expire_and_retry_immediately() {
        let revisions = Arc::new(Revisions::default());
        let scope = Scope::new(&saved(), &[]);
        let scans = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let revisions = revisions.clone();
                let scope = scope.clone();
                let scans = scans.clone();
                std::thread::spawn(move || {
                    revisions
                        .get(scope.clone(), || {
                            scans.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            DiscoveryInputs {
                                scope,
                                revision: 1,
                                model_key: None,
                                models_require_refresh: false,
                            }
                        })
                        .revision()
                })
            })
            .collect();
        for thread in threads {
            assert_eq!(thread.join().unwrap(), 1);
        }
        assert_eq!(scans.load(std::sync::atomic::Ordering::SeqCst), 1);
        revisions
            .0
            .lock()
            .unwrap()
            .get(&scope)
            .unwrap()
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .0 = Instant::now().checked_sub(REVISION_INTERVAL).unwrap();
        assert_eq!(
            revisions
                .get(scope.clone(), || DiscoveryInputs {
                    scope: scope.clone(),
                    revision: 2,
                    model_key: None,
                    models_require_refresh: false
                })
                .revision(),
            2
        );
        revisions.invalidate(&scope);
        assert_eq!(
            revisions
                .get(scope.clone(), || DiscoveryInputs {
                    scope,
                    revision: 3,
                    model_key: None,
                    models_require_refresh: false
                })
                .revision(),
            3
        );
    }

    #[test]
    fn unrelated_revision_scopes_do_not_wait_for_slow_scan() {
        let revisions = Arc::new(Revisions::default());
        let scope = Scope::new(&saved(), &[]);
        let mut other = scope.clone();
        other.cwd = "/other".into();
        let (started, start) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let owner = std::thread::spawn({
            let revisions = revisions.clone();
            move || {
                revisions.get(scope.clone(), || {
                    started.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(5)).unwrap();
                    DiscoveryInputs {
                        scope,
                        revision: 1,
                        model_key: None,
                        models_require_refresh: false,
                    }
                })
            }
        });
        start.recv_timeout(Duration::from_secs(5)).unwrap();
        let (finished, finish) = std::sync::mpsc::channel();
        let follower = std::thread::spawn(move || {
            revisions.get(other.clone(), || DiscoveryInputs {
                scope: other,
                revision: 2,
                model_key: None,
                models_require_refresh: false,
            });
            finished.send(()).unwrap();
        });
        let independent = finish.recv_timeout(Duration::from_secs(2));
        release.send(()).unwrap();
        owner.join().unwrap();
        follower.join().unwrap();
        assert!(independent.is_ok(), "unrelated scope waited for slow scan");
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
    fn ready_catalog_reuses_models_and_efforts_until_retry() {
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
                    finish
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    Ok(result())
                })
            }
        });
        loading
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
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
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
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
        // A unique unsupported launcher option both isolates the in-memory fixture and
        // fails before process spawn if a regression accidentally crosses the boundary.
        let args = vec![format!("--catalog-fixture={}", cwd.path().display())];
        let mut request = saved();
        request.cwd = cwd.path().to_string_lossy().into_owned();
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            request.provider = provider;
            let mut state = Snapshot::new(metadata_request(request.clone()));
            state.models = vec![serde_json::json!({
                "model":provider.label(),
                "supportedReasoningEfforts":[{"reasoningEffort":"high"}]
            })];
            crate::model_catalog::seed_memory(&request, &args, state.models.clone());
            cache()
                .discover(Scope::new(&request, &args), || Ok(state))
                .unwrap();
        }
        // A picker-selected model does not change scope; switching back also hits.
        for provider in [codex::Provider::Claude, codex::Provider::Codex] {
            request.provider = provider;
            request.composer.model = Some(provider.label().into());
            let ready =
                cached(&request, &args).expect("provider switch should be immediately ready");
            assert_eq!(ready.models[0]["model"], provider.label());
            assert_eq!(
                ready.models[0]["supportedReasoningEfforts"][0]["reasoningEffort"],
                "high"
            );
            assert_eq!(
                discover(request.clone(), args.clone()).unwrap().models,
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
