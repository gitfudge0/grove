//! One durable description per source, with small session-owned links to it.
//!
//! Results are private, atomic JSON files outside the checkout. Generation and
//! publication remain owned by `change_description`; persistence never changes
//! the repository or the PR.
use crate::change_description::{self, Description, Scope, Snapshot};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

const VERSION: u32 = 1;
const RESULT_LIMIT: u64 = 8 * 1024 * 1024;
const LINK_LIMIT: u64 = 64 * 1024;
pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub checkout: String,
    pub scope: Scope,
}
impl Source {
    pub fn normalized(&self) -> Result<Self> {
        let checkout = normalize_checkout(&self.checkout)?;
        let scope = match &self.scope {
            Scope::Local => Scope::Local,
            Scope::PullRequest { url } => {
                let (owner, repository, number) = change_description::pr_identity(url)?;
                Scope::PullRequest {
                    url: format!(
                        "https://github.com/{}/{}/pull/{number}",
                        owner.to_ascii_lowercase(),
                        repository.to_ascii_lowercase()
                    ),
                }
            }
        };
        Ok(Self { checkout, scope })
    }
    fn identity(&self) -> Result<String> {
        let source = self.normalized()?;
        Ok(match source.scope {
            Scope::Local => format!("local\0{}", source.checkout),
            Scope::PullRequest { url } => format!("pr\0{url}"),
        })
    }
}
impl From<&Description> for Source {
    fn from(value: &Description) -> Self {
        Self {
            checkout: value.snapshot.checkout.clone(),
            scope: value.snapshot.scope.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedResult {
    schema_version: u32,
    source_id: String,
    description: Description,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionLink {
    schema_version: u32,
    session_key: String,
    source: Source,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingJob {
    schema_version: u32,
    session_key: String,
    snapshot: Snapshot,
    generation: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerationClaim {
    schema_version: u32,
    source_id: String,
    session_key: String,
    generation: String,
}
impl Store {
    /// The supplied directory owns only change-description results and links.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn open() -> Result<Self> {
        Ok(Self::new(
            crate::storage::config_dir()
                .map_err(|error| format!("Could not locate saved descriptions: {error}"))?
                .join("change-descriptions"),
        ))
    }
    pub fn result_path(&self, source: &Source) -> Result<PathBuf> {
        Ok(self
            .root
            .join("sources")
            .join(format!("{}.json", hash(&source.identity()?))))
    }
    fn link_path(&self, session_key: &str) -> Result<PathBuf> {
        if session_key.trim().is_empty()
            || session_key.len() > 1024
            || session_key.chars().any(char::is_control)
        {
            return Err("Invalid description session identity".into());
        }
        Ok(self
            .root
            .join("sessions")
            .join(format!("{}.json", hash(session_key))))
    }
    fn pending_path(&self, session_key: &str) -> Result<PathBuf> {
        // Validate the session key using the same rules as its durable link.
        self.link_path(session_key)?;
        Ok(self
            .root
            .join("jobs")
            .join(format!("{}.json", hash(session_key))))
    }
    /// Save the immutable capture before a provider receives it. This job is
    /// recovery metadata, not a second description result or an automatic retry.
    pub fn save_pending(&self, session_key: &str, snapshot: &Snapshot) -> Result<()> {
        let source = Source {
            checkout: snapshot.checkout.clone(),
            scope: snapshot.scope.clone(),
        };
        self.with_source_lock(&source, || {
            let _ = self.pending_for_session(session_key)?;
            self.check_pending_source(session_key, snapshot)?;
            let generation = crate::identity::random_id()?;
            write(
                &self.pending_path(session_key)?,
                &PendingJob {
                    schema_version: VERSION,
                    session_key: session_key.into(),
                    snapshot: snapshot.clone(),
                    generation: generation.clone(),
                },
                RESULT_LIMIT,
            )?;
            write(
                &self.claim_path(&source)?,
                &GenerationClaim {
                    schema_version: VERSION,
                    source_id: source.identity()?,
                    session_key: session_key.into(),
                    generation,
                },
                LINK_LIMIT,
            )
        })
    }
    pub fn pending_generation(&self, session_key: &str) -> Result<Option<String>> {
        let Some(job) = read::<PendingJob>(&self.pending_path(session_key)?, RESULT_LIMIT)? else {
            return Ok(None);
        };
        let _ = self.pending_for_session(session_key)?;
        Ok(Some(job.generation))
    }
    fn claim_path(&self, source: &Source) -> Result<PathBuf> {
        Ok(self
            .root
            .join("claims")
            .join(format!("{}.json", hash(&source.identity()?))))
    }
    fn with_source_lock<T>(
        &self,
        source: &Source,
        action: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let directory = self.root.join("locks");
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        builder
            .create(&directory)
            .map_err(|error| format!("Could not create description lock directory: {error}"))?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let file = options
            .open(directory.join(format!("{}.lock", hash(&source.identity()?))))
            .map_err(|error| format!("Could not open description lock: {error}"))?;
        file.lock()
            .map_err(|error| format!("Could not lock description source: {error}"))?;
        let result = action();
        let unlocked = file
            .unlock()
            .map_err(|error| format!("Could not unlock description source: {error}"));
        match result {
            Err(error) => Err(error),
            Ok(value) => {
                unlocked?;
                Ok(value)
            }
        }
    }
    /// Import only the newest captured request for this canonical source. The
    /// source lock covers claim verification, atomic replacement and job cleanup.
    pub fn save_generation(
        &self,
        session_key: &str,
        description: &Description,
        generation: &str,
    ) -> Result<PathBuf> {
        let source = Source::from(description);
        self.with_source_lock(&source, || {
            let job = read::<PendingJob>(&self.pending_path(session_key)?, RESULT_LIMIT)?.ok_or("Description capture is no longer available")?;
            self.check_pending_source(session_key, &job.snapshot)?;
            if job.generation != generation { return Err("This generation was superseded by a newer request. The canonical result was preserved.".into()); }
            if job.snapshot != description.snapshot { return Err("Description does not match its captured request".into()); }
            let claim = read::<GenerationClaim>(&self.claim_path(&source)?, LINK_LIMIT)?.ok_or("Description generation claim is missing")?;
            if claim.schema_version != VERSION || claim.source_id != source.identity()? || claim.session_key != session_key || claim.generation != job.generation {
                return Err("A newer description generation superseded this request. The canonical result was preserved.".into());
            }
            let path = self.save_unlocked(description)?;
            self.clear_pending(session_key)?;
            Ok(path)
        })
    }
    /// Acknowledge terminal failure/cancellation without deleting a later job.
    /// Retired attempts cannot be imported after restart.
    pub fn retire_generation(&self, session_key: &str, generation: &str) -> Result<()> {
        let Some(snapshot) = self.pending_for_session(session_key)? else {
            return Ok(());
        };
        let source = Source {
            checkout: snapshot.checkout,
            scope: snapshot.scope,
        };
        self.with_source_lock(&source, || {
            let Some(job) = read::<PendingJob>(&self.pending_path(session_key)?, RESULT_LIMIT)?
            else {
                return Ok(());
            };
            if job.generation == generation {
                self.clear_pending(session_key)?;
            }
            Ok(())
        })
    }
    pub fn pending_for_session(&self, session_key: &str) -> Result<Option<Snapshot>> {
        let path = self.pending_path(session_key)?;
        let Some(job) = read::<PendingJob>(&path, RESULT_LIMIT)? else {
            return Ok(None);
        };
        if job.schema_version != VERSION || job.session_key != session_key {
            return Err("Saved description job has an unsupported schema or identity".into());
        }
        self.check_pending_source(session_key, &job.snapshot)?;
        Ok(Some(job.snapshot))
    }
    fn check_pending_source(&self, session_key: &str, snapshot: &Snapshot) -> Result<()> {
        let source = Source {
            checkout: snapshot.checkout.clone(),
            scope: snapshot.scope.clone(),
        }
        .normalized()?;
        let bound = self
            .source_for_session(session_key)?
            .ok_or("Description session has no saved source")?;
        if bound != source || snapshot.fingerprint.is_empty() || snapshot.files.len() > 1000 {
            return Err(
                "Saved description job does not match its session's captured source".into(),
            );
        }
        Ok(())
    }
    /// Call only for the acknowledged session job; callers own its generation
    /// identity so an older completion cannot clear a newer pending capture.
    pub fn clear_pending(&self, session_key: &str) -> Result<()> {
        let path = self.pending_path(session_key)?;
        let _ = self.pending_for_session(session_key)?;
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("Could not clear saved description job: {error}")),
        }
    }
    pub fn bind(&self, session_key: &str, source: &Source) -> Result<()> {
        let path = self.link_path(session_key)?;
        // An unreadable prior link is not permission to overwrite it.
        let _ = self.source_for_session(session_key)?;
        let link = SessionLink {
            schema_version: VERSION,
            session_key: session_key.into(),
            source: source.normalized()?,
        };
        write(&path, &link, LINK_LIMIT)
    }
    pub fn source_for_session(&self, session_key: &str) -> Result<Option<Source>> {
        let path = self.link_path(session_key)?;
        let Some(link) = read::<SessionLink>(&path, LINK_LIMIT)? else {
            return Ok(None);
        };
        if link.schema_version != VERSION || link.session_key != session_key {
            return Err(
                "Saved description session link has an unsupported schema or identity".into(),
            );
        }
        Ok(Some(link.source.normalized()?))
    }
    /// Publication is an explicit action against the current canonical result.
    /// Hold the source lock across preflight, remote update and local receipt so
    /// a parallel regeneration cannot replace the preview being confirmed.
    pub fn publish_current(
        &self,
        checkout: &Path,
        description: &Description,
        body: &str,
    ) -> Result<()> {
        let source = Source::from(description);
        self.with_source_lock(&source, || {
            if self.load(&source)?.as_ref() != Some(description) {
                return Err("The saved description changed. Reopen the latest description before updating the PR.".into());
            }
            if let Some(claim) = read::<GenerationClaim>(&self.claim_path(&source)?, LINK_LIMIT)? {
                if self.pending_for_session(&claim.session_key)?.is_some() {
                    return Err("Wait for the current description generation to finish before updating the PR.".into());
                }
            }
            change_description::publish(checkout, description, body)?;
            let mut updated = description.clone(); updated.snapshot.original_body = Some(body.into());
            self.save_unlocked(&updated).map_err(|error| format!("PR updated, but the local description could not be saved: {error}"))?;
            Ok(())
        })
    }
    pub fn load(&self, source: &Source) -> Result<Option<Description>> {
        let path = self.result_path(source)?;
        let Some(record) = read::<SavedResult>(&path, RESULT_LIMIT)? else {
            return Ok(None);
        };
        let expected = source.identity()?;
        if record.schema_version != VERSION
            || record.source_id != expected
            || Source::from(&record.description).identity()? != expected
        {
            return Err("Saved description has an unsupported schema or source identity".into());
        }
        validate_description(&record.description)?;
        Ok(Some(record.description))
    }
    /// Replace this source's single result. Failed validation/write leaves its
    /// previous file intact. Session generation imports use `save_generation`.
    pub fn save(&self, description: &Description) -> Result<PathBuf> {
        self.with_source_lock(&Source::from(description), || {
            self.save_unlocked(description)
        })
    }
    fn save_unlocked(&self, description: &Description) -> Result<PathBuf> {
        validate_description(description)?;
        let source = Source::from(description);
        let path = self.result_path(&source)?;
        // Preserve corruption for recovery instead of silently destroying it.
        let _ = self.load(&source)?;
        let record = SavedResult {
            schema_version: VERSION,
            source_id: source.identity()?,
            description: description.clone(),
        };
        write(&path, &record, RESULT_LIMIT)?;
        Ok(path)
    }
}

fn normalize_checkout(value: &str) -> Result<String> {
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err("Description checkout must be an absolute path".into());
    }
    // Retain a captured canonical path when its checkout has since disappeared.
    // Loading the saved result must still work so the reader can show stale data.
    match fs_err::canonicalize(path) {
        Ok(path) => Ok(path.to_string_lossy().into_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if path.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            }) {
                return Err(
                    "Unavailable description checkout must use its captured absolute path".into(),
                );
            }
            Ok(value.into())
        }
        Err(error) => Err(format!("Could not identify description checkout: {error}")),
    }
}
fn hash(text: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, text.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn read<T: DeserializeOwned>(path: &Path, limit: u64) -> Result<Option<T>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not read saved description: {error}")),
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("Could not inspect saved description: {error}"))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("Saved description is not a regular file or exceeds its size limit".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read saved description: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err("Saved description exceeds its size limit".into());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("Saved description is invalid; its file was preserved: {error}"))
}
fn write(path: &Path, value: &impl Serialize, limit: u64) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("Could not encode saved description: {error}"))?;
    if bytes.len() as u64 > limit {
        return Err("Description exceeds the durable storage limit".into());
    }
    let parent = path
        .parent()
        .ok_or("Description storage directory is unavailable")?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder
        .create(parent)
        .map_err(|error| format!("Could not create description storage: {error}"))?;
    crate::storage::write_atomic_private(path, &bytes)
        .map_err(|error| format!("Could not save description: {error}"))
}
fn validate_description(description: &Description) -> Result<()> {
    let prediction = serde_json::json!({
        "title": description.title,
        "types": description.types,
        "summary": description.summary,
        "files": description.files,
        "diagram": description.diagram,
    });
    // Reuse the generator's validation without exposing a second, weaker shape.
    change_description::validate(description.snapshot.clone(), prediction)?;
    if description.snapshot.fingerprint.is_empty() {
        return Err("Saved description has no captured snapshot identity".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn description(checkout: &Path, scope: Scope) -> Description {
        Description {
            snapshot: Snapshot {
                scope,
                fingerprint: "captured-index-and-file-state".into(),
                label: "Captured source".into(),
                head: "abc123".into(),
                base: "def456".into(),
                original_body: Some("Existing human PR context".into()),
                title: "Existing PR title".into(),
                checkout: checkout.to_string_lossy().into_owned(),
                files: vec![change_description::SnapshotFile {
                    path: "src/handler.rs".into(),
                    added: 1,
                    removed: 0,
                    patch: "@@ -1 +1,2 @@\n line\n+new line\n".into(),
                    omitted: None,
                }],
            },
            title: "Prevent duplicate webhook orders".into(),
            types: vec!["Bug fix".into()],
            summary: vec!["Deduplicate webhook deliveries before creating orders.".into()],
            files: vec![change_description::FileDescription {
                path: "src/handler.rs".into(),
                title: "Skip previously processed webhook delivery identifiers".into(),
                summary: vec!["Only create orders for new delivery IDs.".into()],
                category: "Behavior".into(),
            }],
            content: String::new(),
            diagram: None,
        }
    }
    fn pr(number: u32) -> Scope {
        Scope::PullRequest {
            url: format!("https://github.com/gitfudge0/grove/pull/{number}"),
        }
    }
    #[test]
    fn retired_generation_cannot_be_recovered_or_block_preserved_result() {
        let fixture = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let store = Store::new(fixture.path().to_path_buf());
        let result = description(checkout.path(), pr(17));
        let source = Source::from(&result);
        store.bind("session", &source).unwrap();
        store.save(&result).unwrap();
        store.save_pending("session", &result.snapshot).unwrap();
        let token = store.pending_generation("session").unwrap().unwrap();
        store.retire_generation("session", &token).unwrap();
        assert!(store.pending_for_session("session").unwrap().is_none());
        assert!(store.save_generation("session", &result, &token).is_err());
        assert_eq!(
            Store::new(fixture.path().to_path_buf())
                .load(&source)
                .unwrap(),
            Some(result)
        );
    }

    #[test]
    fn old_preview_and_active_generation_are_rejected_before_any_publication() {
        let fixture = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let store = Store::new(fixture.path().to_path_buf());
        let older = description(checkout.path(), pr(17));
        let mut current = older.clone();
        current.title = "New canonical result".into();
        store.save(&current).unwrap();
        assert!(store
            .publish_current(checkout.path(), &older, "edited body")
            .unwrap_err()
            .contains("saved description changed"));
        store.bind("active", &Source::from(&current)).unwrap();
        store.save_pending("active", &current.snapshot).unwrap();
        assert!(store
            .publish_current(checkout.path(), &current, "edited body")
            .unwrap_err()
            .contains("generation to finish"));
        assert_eq!(store.load(&Source::from(&current)).unwrap(), Some(current));
    }

    #[test]
    fn newer_generation_claim_preserves_the_single_canonical_result() {
        let fixture = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let store = Store::new(fixture.path().to_path_buf());
        let older = description(checkout.path(), pr(17));
        let mut newer = older.clone();
        newer.title = "Newest captured description".into();
        newer.snapshot.fingerprint = "new-snapshot".into();
        let source = Source::from(&older);
        for key in ["old", "new"] {
            store.bind(key, &source).unwrap();
        }
        let path = store.save(&older).unwrap();
        store.save_pending("old", &older.snapshot).unwrap();
        let old_token = store.pending_generation("old").unwrap().unwrap();
        store.save_pending("new", &newer.snapshot).unwrap();
        let new_token = store.pending_generation("new").unwrap().unwrap();
        assert!(store
            .save_generation("old", &older, &old_token)
            .unwrap_err()
            .contains("superseded"));
        assert_eq!(store.load(&source).unwrap(), Some(older.clone()));
        assert_eq!(
            store.save_generation("new", &newer, &new_token).unwrap(),
            path
        );
        assert_eq!(
            Store::new(fixture.path().to_path_buf())
                .load(&source)
                .unwrap(),
            Some(newer.clone())
        );
        assert!(store.pending_for_session("new").unwrap().is_none());
        assert!(store.save_generation("old", &older, &old_token).is_err());
        assert_eq!(store.load(&source).unwrap(), Some(newer.clone()));
        store.save_pending("new", &newer.snapshot).unwrap();
        let first_attempt = store.pending_generation("new").unwrap().unwrap();
        store.save_pending("new", &newer.snapshot).unwrap();
        assert_ne!(
            store.pending_generation("new").unwrap().unwrap(),
            first_attempt
        );
        assert!(store
            .save_generation("new", &newer, &first_attempt)
            .is_err());
        store.retire_generation("new", &first_attempt).unwrap();
        let latest = store.pending_generation("new").unwrap().unwrap();
        assert_eq!(store.save_generation("new", &newer, &latest).unwrap(), path);
        assert_eq!(
            std::fs::read_dir(fixture.path().join("sources"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn result_and_session_links_survive_reopening_without_duplicate_results() {
        let fixture = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let second_checkout = tempfile::tempdir().unwrap();
        let store = Store::new(fixture.path().join("descriptions"));
        let mut result = description(checkout.path(), pr(17));
        let source = Source::from(&result);
        store.bind("codex-session", &source).unwrap();
        let first_path = store.save(&result).unwrap();
        let bound = Source {
            checkout: second_checkout.path().to_string_lossy().into_owned(),
            scope: pr(17),
        };
        store.bind("claude-session", &bound).unwrap();
        result.title = "Avoid processing duplicate webhook deliveries".into();
        result.snapshot.head = "new-head".into();
        assert_eq!(store.save(&result).unwrap(), first_path);
        let reopened = Store::new(fixture.path().join("descriptions"));
        assert_eq!(
            reopened.source_for_session("codex-session").unwrap(),
            Some(source.normalized().unwrap())
        );
        assert_eq!(
            reopened.source_for_session("claude-session").unwrap(),
            Some(bound.normalized().unwrap())
        );
        assert_eq!(reopened.load(&bound).unwrap(), Some(result));
        assert_eq!(
            std::fs::read_dir(first_path.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
        assert!(reopened
            .source_for_session("other-session")
            .unwrap()
            .is_none());
    }
    #[test]
    fn canonical_identity_ignores_pr_spelling_session_checkout_and_snapshot_revision() {
        let root = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let other_checkout = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().into());
        let source = Source {
            checkout: checkout.path().to_string_lossy().into_owned(),
            scope: pr(17),
        };
        for url in [
            "https://github.com/GitFudge0/Grove/pull/017/",
            "https://GITHUB.COM/gitfudge0/grove/pull/17",
        ] {
            let variant = Source {
                checkout: other_checkout.path().to_string_lossy().into_owned(),
                scope: Scope::PullRequest { url: url.into() },
            };
            assert_eq!(
                store.result_path(&source).unwrap(),
                store.result_path(&variant).unwrap()
            );
        }
        let local = Source {
            checkout: source.checkout.clone(),
            scope: Scope::Local,
        };
        let other_local = Source {
            checkout: other_checkout.path().to_string_lossy().into_owned(),
            scope: Scope::Local,
        };
        assert_ne!(
            store.result_path(&local).unwrap(),
            store.result_path(&other_local).unwrap()
        );
        assert_ne!(
            store.result_path(&source).unwrap(),
            store.result_path(&local).unwrap()
        );
        let another_pr = Source {
            scope: pr(18),
            ..source.clone()
        };
        assert_ne!(
            store.result_path(&source).unwrap(),
            store.result_path(&another_pr).unwrap()
        );
        assert!(store
            .result_path(&Source {
                scope: Scope::PullRequest {
                    url: "https://evil.test/a/b/pull/17".into()
                },
                ..source
            })
            .is_err());
    }
    #[test]
    fn missing_checkout_keeps_saved_description_available_for_staleness_display() {
        let root = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().into());
        let result = description(checkout.path(), Scope::Local);
        let source = Source::from(&result);
        store.save(&result).unwrap();
        checkout.close().unwrap();
        assert_eq!(store.load(&source).unwrap(), Some(result));
    }
    #[test]
    fn corrupt_unknown_version_and_wrong_source_results_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().into());
        let result = description(checkout.path(), pr(17));
        let source = Source::from(&result);
        let path = store.save(&result).unwrap();
        let valid = std::fs::read(&path).unwrap();
        for bytes in [
            b"not json".to_vec(),
            {
                let mut record: serde_json::Value = serde_json::from_slice(&valid).unwrap();
                record["schema_version"] = 999.into();
                serde_json::to_vec(&record).unwrap()
            },
            {
                let mut record: serde_json::Value = serde_json::from_slice(&valid).unwrap();
                record["description"]["snapshot"]["scope"]["PullRequest"]["url"] =
                    "https://github.com/gitfudge0/grove/pull/18".into();
                serde_json::to_vec(&record).unwrap()
            },
        ] {
            std::fs::write(&path, &bytes).unwrap();
            assert!(store.load(&source).is_err());
            assert!(store.save(&result).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
    #[test]
    fn invalid_generation_never_replaces_the_last_successful_result() {
        let root = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().into());
        let result = description(checkout.path(), pr(17));
        store.save(&result).unwrap();
        let mut invalid = result.clone();
        invalid.files[0].path = "unsupplied-path.rs".into();
        assert!(store.save(&invalid).is_err());
        assert_eq!(store.load(&Source::from(&result)).unwrap(), Some(result));
    }
    #[test]
    fn pending_capture_survives_restart_until_explicitly_cleared() {
        let root = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().into());
        let result = description(checkout.path(), pr(17));
        let source = Source::from(&result);
        assert!(store.save_pending("session", &result.snapshot).is_err());
        store.bind("session", &source).unwrap();
        store.save_pending("session", &result.snapshot).unwrap();
        let reopened = Store::new(root.path().into());
        assert_eq!(
            reopened.pending_for_session("session").unwrap(),
            Some(result.snapshot.clone())
        );
        assert!(
            reopened.load(&source).unwrap().is_none(),
            "pending capture is not a second result"
        );
        let mut unrelated = result.snapshot.clone();
        unrelated.scope = pr(18);
        assert!(reopened.save_pending("session", &unrelated).is_err());
        assert_eq!(
            reopened.pending_for_session("session").unwrap(),
            Some(result.snapshot)
        );
        reopened.clear_pending("session").unwrap();
        reopened.clear_pending("session").unwrap();
        assert!(reopened.pending_for_session("session").unwrap().is_none());
        assert_eq!(
            reopened.source_for_session("session").unwrap(),
            Some(source.normalized().unwrap())
        );
    }
    #[test]
    fn hashed_links_and_bounded_reads_do_not_escape_or_replace_corruption() {
        let root = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().join("descriptions"));
        let source = Source {
            checkout: checkout.path().to_string_lossy().into_owned(),
            scope: pr(17),
        };
        let malicious_key = "../../outside/result";
        store.bind(malicious_key, &source).unwrap();
        let path = store.link_path(malicious_key).unwrap();
        assert_eq!(path.parent().unwrap(), store.root.join("sessions"));
        let corrupt = b"not a link";
        std::fs::write(&path, corrupt).unwrap();
        assert!(store.bind(malicious_key, &source).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), corrupt);
        let result_path = store.result_path(&source).unwrap();
        std::fs::create_dir_all(result_path.parent().unwrap()).unwrap();
        let large = std::fs::File::create(&result_path).unwrap();
        large.set_len(RESULT_LIMIT + 1).unwrap();
        assert!(store.load(&source).is_err());
        assert_eq!(
            std::fs::metadata(&result_path).unwrap().len(),
            RESULT_LIMIT + 1
        );
    }
    #[cfg(unix)]
    #[test]
    fn result_links_and_jobs_are_private_regular_files() {
        use std::os::unix::fs::{symlink, MetadataExt as _, PermissionsExt as _};
        let root = tempfile::tempdir().unwrap();
        let checkout = tempfile::tempdir().unwrap();
        let store = Store::new(root.path().join("descriptions"));
        let result = description(checkout.path(), pr(17));
        let source = Source::from(&result);
        store.bind("session", &source).unwrap();
        store.save_pending("session", &result.snapshot).unwrap();
        let path = store.save(&result).unwrap();
        for file in [
            &path,
            &store.link_path("session").unwrap(),
            &store.pending_path("session").unwrap(),
        ] {
            assert_eq!(std::fs::metadata(file).unwrap().mode() & 0o777, 0o600);
            assert_eq!(
                std::fs::metadata(file.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        let external = root.path().join("external.json");
        std::fs::write(&external, b"private external data").unwrap();
        std::fs::remove_file(&path).unwrap();
        symlink(&external, &path).unwrap();
        assert!(store.load(&source).is_err());
        assert!(store.save(&result).is_err());
        assert_eq!(std::fs::read(external).unwrap(), b"private external data");
    }
}
