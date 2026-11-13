//! Last successful provider model lists. Workspace defaults and skills never enter this cache.
use crate::{
    codex::{Provider, SavedSession},
    composer_catalog::Scope,
};
use ring::digest::{Context, SHA256};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const VERSION: u8 = 1;
const MAX_MODELS: u64 = 4 * 1024 * 1024;
const MAX_ENTRIES: usize = 64;

#[derive(Clone)]
pub(crate) struct Key {
    fingerprint: String,
    path: PathBuf,
}
#[derive(Clone)]
struct Entry {
    models: Vec<Value>,
    refresh: bool,
}
#[derive(Default)]
struct Cache {
    entries: HashMap<String, Entry>,
    aliases: HashMap<Scope, String>,
    invalidated: std::collections::HashSet<String>,
}
fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}
#[derive(Serialize, Deserialize)]
struct Persisted {
    version: u8,
    fingerprint: String,
    models: Vec<Value>,
}

fn add(hash: &mut Context, bytes: &[u8]) {
    hash.update(&(bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}
fn file(hash: &mut Context, path: &Path) {
    add(hash, path.to_string_lossy().as_bytes());
    match fs_err::read(path) {
        Ok(bytes) => {
            add(hash, b"present");
            add(hash, &bytes);
        }
        Err(error) => add(hash, format!("{:?}", error.kind()).as_bytes()),
    }
}

fn catalog_references(hash: &mut Context, config: &toml::Value, cwd: &Path, config_parent: &Path) {
    match config {
        toml::Value::Table(table) => {
            for (name, value) in table {
                if let ("model_catalog_json", Some(path)) = (name.as_str(), value.as_str()) {
                    // Hash both plausible relative resolutions. Extra invalidation is safe;
                    // missing an input could reuse another route's model capabilities.
                    file(hash, &cwd.join(path));
                    if config_parent != cwd && !Path::new(path).is_absolute() {
                        file(hash, &config_parent.join(path));
                    }
                }
                catalog_references(hash, value, cwd, config_parent);
            }
        }
        toml::Value::Array(values) => {
            for value in values {
                catalog_references(hash, value, cwd, config_parent);
            }
        }
        _ => {}
    }
}

fn external_settings(hash: &mut Context, saved: &SavedSession, args: &[String]) {
    let cwd = Path::new(&saved.cwd);
    for (index, option) in args.iter().enumerate() {
        let settings = option.strip_prefix("--settings=").or_else(|| {
            (option == "--settings")
                .then(|| args.get(index + 1).map(String::as_str))
                .flatten()
        });
        if let Some(path) = settings.filter(|value| !value.trim_start().starts_with('{')) {
            file(hash, &cwd.join(path));
        }
        if let Ok(config) = option.parse::<toml::Value>() {
            catalog_references(hash, &config, cwd, cwd);
        }
    }
}

fn codex_config(hash: &mut Context, path: &Path, cwd: &str) {
    file(hash, path);
    if let Ok(text) = fs_err::read_to_string(path) {
        if let Ok(config) = text.parse::<toml::Value>() {
            let cwd = Path::new(cwd);
            catalog_references(hash, &config, cwd, path.parent().unwrap_or(cwd));
        }
    }
}

impl Key {
    /// Called on provider/background threads only: fingerprints credentials and configuration
    /// without keeping or serializing their contents. Relative overrides retain cwd isolation.
    pub(crate) fn new(saved: &SavedSession, args: &[String]) -> Option<Self> {
        if !Path::new(&saved.cwd).is_dir() {
            return None;
        }
        let mut hash = Context::new(&SHA256);
        add(&mut hash, &[VERSION]);
        add(&mut hash, saved.provider.label().as_bytes());
        let environment = crate::env_path::session_environment().map_or_else(
            || std::env::vars().collect::<Vec<_>>(),
            |values| values.to_vec(),
        );
        let env = |key: &str| {
            environment
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        };
        // Provider configuration, credentials and executable search paths affect model availability.
        // Desktop/session bookkeeping does not, and would defeat reuse after an app restart.
        let mut relevant: Vec<_> = environment
            .iter()
            .filter(|(key, _)| {
                matches!(
                    key.as_str(),
                    "HOME" | "USERPROFILE" | "PATH" | "XDG_CONFIG_HOME"
                ) || [
                    "CODEX_",
                    "OPENAI_",
                    "CLAUDE_",
                    "ANTHROPIC_",
                    "AWS_",
                    "AZURE_",
                    "GOOGLE_",
                ]
                .iter()
                .any(|prefix| key.starts_with(prefix))
            })
            .collect();
        relevant.sort();
        for (key, value) in relevant {
            add(&mut hash, key.as_bytes());
            add(&mut hash, value.as_bytes());
        }
        for root in &saved.roots {
            add(&mut hash, root.wt_path.as_bytes());
        }
        for arg in args {
            add(&mut hash, arg.as_bytes());
        }
        let mut normalized = saved.clone();
        if saved.provider == Provider::Codex {
            if let Ok((configs, _)) = crate::codex::launch_options(args) {
                if configs.is_empty() {
                    normalized.migrate_native_config();
                } else {
                    normalized.config_args = configs;
                    normalized.native_config_version = 1;
                }
            }
        }
        for arg in &normalized.config_args {
            add(&mut hash, arg.as_bytes());
        }
        add(&mut hash, &[normalized.native_config_version]);
        external_settings(&mut hash, saved, args);
        external_settings(&mut hash, saved, &normalized.config_args);
        // Overrides can name files or configuration relative to the working directory.
        let relative_overrides = normalized.config_args.iter().any(|arg| {
            !matches!(arg.as_str(), "-c" | "--config")
                && !arg.starts_with("approval_policy=")
                && !arg.starts_with("sandbox_mode=")
        });
        if relative_overrides
            || (saved.provider == Provider::Claude
                && args.iter().any(|arg| {
                    !matches!(arg.as_str(), "--dangerously-skip-permissions" | "--chrome")
                }))
        {
            add(&mut hash, saved.cwd.as_bytes());
        }
        let home = dirs::home_dir().unwrap_or_default();
        match saved.provider {
            Provider::Codex => {
                let root = env("CODEX_HOME").map_or_else(|| home.join(".codex"), PathBuf::from);
                codex_config(&mut hash, &root.join("config.toml"), &saved.cwd);
                file(&mut hash, &root.join("auth.json"));
                for parent in Path::new(&saved.cwd).ancestors() {
                    let config = parent.join(".codex/config.toml");
                    if config.exists() {
                        codex_config(&mut hash, &config, &saved.cwd);
                    }
                }
            }
            Provider::Claude => {
                let root =
                    env("CLAUDE_CONFIG_DIR").map_or_else(|| home.join(".claude"), PathBuf::from);
                // .claude.json also holds mutable UI/telemetry and per-project bookkeeping.
                // Hash only account and routing configuration, not startup-written timestamps.
                let user_config = home.join(".claude.json");
                if let Ok(bytes) = fs_err::read(&user_config) {
                    if let Ok(config) = serde_json::from_slice::<Value>(&bytes) {
                        for name in [
                            "oauthAccount",
                            "primaryApiKey",
                            "apiKeyHelper",
                            "env",
                            "model",
                        ] {
                            add(&mut hash, name.as_bytes());
                            add(&mut hash, config[name].to_string().as_bytes());
                        }
                    }
                }
                for path in [root.join("settings.json"), root.join(".credentials.json")] {
                    file(&mut hash, &path);
                }
                for parent in Path::new(&saved.cwd).ancestors() {
                    for name in [".claude/settings.json", ".claude/settings.local.json"] {
                        let config = parent.join(name);
                        if config.exists() {
                            file(&mut hash, &config);
                        }
                    }
                }
            }
        }
        let (binary, prefix) = saved.provider.agent().invocation();
        add(&mut hash, binary.as_bytes());
        for arg in prefix {
            add(&mut hash, arg.as_bytes());
            if let Ok(metadata) = fs_err::metadata(&arg) {
                add(&mut hash, &metadata.len().to_le_bytes());
                if let Ok(modified) = metadata.modified().and_then(|time| {
                    time.duration_since(std::time::UNIX_EPOCH)
                        .map_err(std::io::Error::other)
                }) {
                    add(&mut hash, &modified.as_nanos().to_le_bytes());
                }
            }
        }
        if let Ok(metadata) = fs_err::metadata(&binary) {
            add(&mut hash, &metadata.len().to_le_bytes());
            if let Ok(modified) = metadata.modified().and_then(|time| {
                time.duration_since(std::time::UNIX_EPOCH)
                    .map_err(std::io::Error::other)
            }) {
                add(&mut hash, &modified.as_nanos().to_le_bytes());
            }
        }
        let mut fingerprint = String::with_capacity(64);
        for byte in hash.finish().as_ref() {
            let _ = write!(&mut fingerprint, "{byte:02x}");
        }
        let directory = crate::storage::config_dir().ok()?.join("provider-models");
        let path = directory.join(format!("{fingerprint}.json"));
        let mut cached = cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cached.aliases.len() >= MAX_ENTRIES {
            cached.aliases.clear();
        }
        cached
            .aliases
            .insert(Scope::new(saved, args), fingerprint.clone());
        Some(Self { fingerprint, path })
    }
}

pub(crate) fn revision(saved: &SavedSession, args: &[String]) -> Option<String> {
    Key::new(saved, args).map(|key| key.fingerprint)
}

pub(crate) fn load(key: &Key) -> Option<Vec<Value>> {
    let mut cache = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.invalidated.contains(&key.fingerprint) {
        return None;
    }
    if let Some(entry) = cache.entries.get(&key.fingerprint) {
        return (!entry.refresh).then(|| entry.models.clone());
    }
    if fs_err::metadata(&key.path).ok()?.len() > MAX_MODELS {
        return None;
    }
    let persisted: Persisted = serde_json::from_slice(&fs_err::read(&key.path).ok()?).ok()?;
    if persisted.version != VERSION
        || persisted.fingerprint != key.fingerprint
        || persisted.models.is_empty()
    {
        return None;
    }
    let models = persisted.models;
    if cache.entries.len() >= MAX_ENTRIES {
        cache.entries.clear();
    }
    cache.entries.insert(
        key.fingerprint.clone(),
        Entry {
            models: models.clone(),
            refresh: false,
        },
    );
    Some(models)
}

pub(crate) fn cached(saved: &SavedSession, args: &[String]) -> Option<Vec<Value>> {
    let cache = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fingerprint = cache.aliases.get(&Scope::new(saved, args))?;
    if cache.invalidated.contains(fingerprint) {
        return None;
    }
    cache
        .entries
        .get(fingerprint)
        .filter(|entry| !entry.refresh)
        .map(|entry| entry.models.clone())
}

pub(crate) fn requires_refresh(saved: &SavedSession, args: &[String]) -> bool {
    let cache = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .aliases
        .get(&Scope::new(saved, args))
        .is_some_and(|fingerprint| cache.invalidated.contains(fingerprint))
}

pub(crate) fn invalidate(saved: &SavedSession, args: &[String]) {
    // Explicit refresh also works before disk hydration in a newly started app.
    let Some(key) = Key::new(saved, args) else {
        return;
    };
    let mut cache = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.invalidated.insert(key.fingerprint.clone());
    if let Some(entry) = cache.entries.get_mut(&key.fingerprint) {
        entry.refresh = true;
    }
}

pub(crate) fn remember(key: &Key, models: &[Value]) {
    if models.is_empty() {
        return;
    }
    {
        let mut cache = cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache.entries.len() >= MAX_ENTRIES && !cache.entries.contains_key(&key.fingerprint) {
            cache.entries.clear();
        }
        cache.invalidated.remove(&key.fingerprint);
        cache.entries.insert(
            key.fingerprint.clone(),
            Entry {
                models: models.to_vec(),
                refresh: false,
            },
        );
    }
    let saved = Persisted {
        version: VERSION,
        fingerprint: key.fingerprint.clone(),
        models: models.to_vec(),
    };
    if let (Some(parent), Ok(bytes)) = (key.path.parent(), serde_json::to_vec(&saved)) {
        if fs_err::create_dir_all(parent).is_ok() {
            let _ = crate::storage::write_atomic_private(&key.path, &bytes);
        }
    }
}

#[cfg(test)]
pub(crate) fn seed_memory(saved: &SavedSession, args: &[String], models: Vec<Value>) {
    let key = Key::new(saved, args).unwrap();
    let mut cache = cache().lock().unwrap();
    cache.invalidated.remove(&key.fingerprint);
    cache.entries.insert(
        key.fingerprint,
        Entry {
            models,
            refresh: false,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn saved(cwd: &Path, provider: Provider) -> SavedSession {
        SavedSession::empty(
            "catalog-test".into(),
            provider,
            "Test".into(),
            cwd.to_string_lossy().into_owned(),
        )
    }
    fn key(directory: &Path) -> Key {
        Key {
            fingerprint: directory.to_string_lossy().into_owned(),
            path: directory.join("models.json"),
        }
    }
    fn forget(key: &Key) {
        let mut cache = cache().lock().unwrap();
        cache.entries.remove(&key.fingerprint);
        cache.invalidated.remove(&key.fingerprint);
    }

    #[test]
    fn persisted_models_survive_memory_eviction_without_retaining_session_state() {
        let directory = tempfile::tempdir().unwrap();
        let key = key(directory.path());
        let models = vec![
            json!({"model":"catalog-model", "supportedReasoningEfforts":[{"reasoningEffort":"high"}]}),
        ];
        remember(&key, &models);
        forget(&key);
        assert_eq!(load(&key), Some(models));
        let persisted: Value = serde_json::from_slice(&fs_err::read(&key.path).unwrap()).unwrap();
        assert_eq!(persisted.as_object().unwrap().len(), 3);
        assert!(persisted.get("saved").is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs_err::metadata(&key.path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn refresh_does_not_destroy_last_success_and_only_success_releases_it() {
        let directory = tempfile::tempdir().unwrap();
        let key = key(directory.path());
        let models = vec![json!({"model":"old"})];
        remember(&key, &models);
        cache()
            .lock()
            .unwrap()
            .invalidated
            .insert(key.fingerprint.clone());
        assert!(load(&key).is_none());
        remember(&key, &[]); // A failure/empty catalog must not erase a usable list.
        assert!(load(&key).is_none());
        let disk: Persisted = serde_json::from_slice(&fs_err::read(&key.path).unwrap()).unwrap();
        assert_eq!(disk.models, models);
        let changed = vec![json!({"model":"new"})];
        remember(&key, &changed);
        assert_eq!(load(&key), Some(changed));
    }

    #[test]
    fn corrupt_or_wrong_version_disk_catalog_is_a_cache_miss() {
        let directory = tempfile::tempdir().unwrap();
        let key = key(directory.path());
        fs_err::write(&key.path, b"truncated{").unwrap();
        assert!(load(&key).is_none());
        fs_err::write(
            &key.path,
            serde_json::to_vec(&Persisted {
                version: VERSION + 1,
                fingerprint: key.fingerprint.clone(),
                models: vec![json!({"model":"wrong"})],
            })
            .unwrap(),
        )
        .unwrap();
        assert!(load(&key).is_none());
    }

    #[test]
    fn default_launch_models_share_across_empty_checkouts_but_not_provider_or_local_config() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        for provider in [Provider::Codex, Provider::Claude] {
            let args = provider.agent().launch_args_for_backend(true, false, false);
            let left = Key::new(&saved(a.path(), provider), &args)
                .unwrap()
                .fingerprint;
            let right = Key::new(&saved(b.path(), provider), &args)
                .unwrap()
                .fingerprint;
            assert_eq!(
                left, right,
                "default models are provider-scoped, not checkout-scoped"
            );
            let config = b.path().join(if provider == Provider::Codex {
                ".codex/config.toml"
            } else {
                ".claude/settings.json"
            });
            fs_err::create_dir_all(config.parent().unwrap()).unwrap();
            fs_err::write(
                &config,
                if provider == Provider::Codex {
                    "model_provider = 'gateway'"
                } else {
                    "{\"env\":{\"ANTHROPIC_BASE_URL\":\"https://gateway.invalid\"}}"
                },
            )
            .unwrap();
            let isolated = Key::new(&saved(b.path(), provider), &args)
                .unwrap()
                .fingerprint;
            assert_ne!(
                left, isolated,
                "workspace provider overrides cannot leak models"
            );
            fs_err::remove_file(config).unwrap();
        }
        assert_ne!(
            Key::new(&saved(a.path(), Provider::Codex), &[])
                .unwrap()
                .fingerprint,
            Key::new(&saved(a.path(), Provider::Claude), &[])
                .unwrap()
                .fingerprint
        );
    }

    #[test]
    fn skill_changes_refresh_workspace_revision_without_expiring_models() {
        let directory = tempfile::tempdir().unwrap();
        for provider in [Provider::Codex, Provider::Claude] {
            let saved = saved(directory.path(), provider);
            let before = Key::new(&saved, &[]).unwrap().fingerprint;
            let old_revision = crate::composer_catalog::revision(&saved, &[]);
            let path = directory.path().join(".agents/skills/test/SKILL.md");
            fs_err::create_dir_all(path.parent().unwrap()).unwrap();
            fs_err::write(&path, "---\nname: example\n---\nUpdated skill").unwrap();
            assert_eq!(before, Key::new(&saved, &[]).unwrap().fingerprint);
            assert_ne!(old_revision, crate::composer_catalog::revision(&saved, &[]));
            fs_err::remove_file(path).unwrap();
        }
    }

    #[test]
    fn referenced_settings_file_changes_invalidate_catalogs_and_workspace_revision() {
        let directory = tempfile::tempdir().unwrap();
        for provider in [Provider::Codex, Provider::Claude] {
            let saved = saved(directory.path(), provider);
            let path = directory.path().join("settings.json");
            let args = if provider == Provider::Claude {
                vec!["--settings".into(), "settings.json".into()]
            } else {
                vec!["-c".into(), "model_catalog_json='settings.json'".into()]
            };
            fs_err::write(&path, "first").unwrap();
            let before = Key::new(&saved, &args).unwrap().fingerprint;
            let revision = crate::composer_catalog::revision(&saved, &args);
            fs_err::write(&path, "second").unwrap();
            assert_ne!(before, Key::new(&saved, &args).unwrap().fingerprint);
            assert_ne!(revision, crate::composer_catalog::revision(&saved, &args));
        }
    }

    #[test]
    fn toml_catalog_references_support_quoted_keys_comments_and_profiles() {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path().join("checkout");
        let config_parent = directory.path().join("config");
        fs_err::create_dir_all(&cwd).unwrap();
        fs_err::create_dir_all(&config_parent).unwrap();
        let config_path = config_parent.join("config.toml");
        let relative = config_parent.join("models.json");
        let absolute = directory.path().join("absolute-models.json");
        let assignment = format!(
            "\"model_catalog_json\" = {} # managed catalog",
            serde_json::to_string(&absolute).unwrap()
        );
        for contents in [
            "\"model_catalog_json\" = 'models.json' # managed catalog".to_owned(),
            "[profiles.gateway]\n\"model_catalog_json\" = 'models.json' # managed catalog"
                .to_owned(),
        ] {
            fs_err::write(&config_path, contents).unwrap();
            fs_err::write(&relative, "first").unwrap();
            let fingerprint = || {
                let mut hash = Context::new(&SHA256);
                codex_config(&mut hash, &config_path, cwd.to_str().unwrap());
                hash.finish().as_ref().to_vec()
            };
            let before = fingerprint();
            fs_err::write(&relative, "second").unwrap();
            assert_ne!(
                before,
                fingerprint(),
                "relative config-parent catalog changes must invalidate"
            );
        }
        let saved = saved(&cwd, Provider::Codex);
        let args = vec!["-c".into(), assignment];
        fs_err::write(&absolute, "first").unwrap();
        let before = Key::new(&saved, &args).unwrap().fingerprint;
        fs_err::write(&absolute, "second").unwrap();
        assert_ne!(
            before,
            Key::new(&saved, &args).unwrap().fingerprint,
            "quoted CLI assignment with an inline comment must track the catalog"
        );
    }

    #[test]
    fn relative_configuration_overrides_keep_checkout_isolation() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        for provider in [Provider::Codex, Provider::Claude] {
            let args = if provider == Provider::Codex {
                vec!["-c".into(), "model_catalog_json='models.json'".into()]
            } else {
                vec!["--settings".into(), "settings.json".into()]
            };
            assert_ne!(
                Key::new(&saved(a.path(), provider), &args)
                    .unwrap()
                    .fingerprint,
                Key::new(&saved(b.path(), provider), &args)
                    .unwrap()
                    .fingerprint
            );
        }
    }
}
