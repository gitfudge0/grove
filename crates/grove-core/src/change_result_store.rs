//! Durable typed analysis results, sharing the description store's private I/O and locks.
use crate::change_description::Snapshot;
pub use crate::change_description_store::Source;
use crate::change_description_store::{read, write, Store as Files};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{marker::PhantomData, path::PathBuf};
const VERSION: u32 = 1;
const LIMIT: u64 = 8 * 1024 * 1024;
const LINK_LIMIT: u64 = 64 * 1024;
pub type Result<T> = std::result::Result<T, String>;
pub trait StoredResult:
    Clone + Serialize + DeserializeOwned + PartialEq + Send + Sync + 'static
{
    const DIRECTORY: &'static str;
    fn snapshot(&self) -> &Snapshot;
    fn input(&self) -> &str;
    fn validate(&self) -> Result<()>;
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PendingRequest {
    pub snapshot: Snapshot,
    pub input: String,
    pub prompt: String,
    pub generation: String,
}
#[derive(Clone, Debug)]
pub struct Store<T> {
    root: PathBuf,
    files: Files,
    marker: PhantomData<T>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Link {
    schema_version: u32,
    session_key: String,
    source: Source,
    /// Last captured input is recovery data, never authority to replay a job.
    #[serde(default)]
    input: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    schema_version: u32,
    session_key: String,
    request: PendingRequest,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Claim {
    schema_version: u32,
    source_id: String,
    session_key: String,
    generation: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record<T> {
    schema_version: u32,
    source_id: String,
    result: T,
}
fn hash(text: &str) -> String {
    use std::fmt::Write as _;
    ring::digest::digest(&ring::digest::SHA256, text.as_bytes())
        .as_ref()
        .iter()
        .fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}
fn source(snapshot: &Snapshot) -> Source {
    Source {
        checkout: snapshot.checkout.clone(),
        scope: snapshot.scope.clone(),
    }
}
fn key_valid(key: &str) -> Result<()> {
    if key.trim().is_empty() || key.len() > 1024 || key.chars().any(char::is_control) {
        Err("Invalid analysis session identity".into())
    } else {
        Ok(())
    }
}
impl<T: StoredResult> Store<T> {
    pub fn new(root: PathBuf) -> Self {
        Self {
            files: Files::new(root.clone()),
            root,
            marker: PhantomData,
        }
    }
    pub fn open() -> Result<Self> {
        if T::DIRECTORY.is_empty()
            || std::path::Path::new(T::DIRECTORY)
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            || T::DIRECTORY.contains('/')
            || T::DIRECTORY.contains('\\')
        {
            return Err("Invalid analysis storage namespace".into());
        }
        Ok(Self::new(
            crate::storage::config_dir()
                .map_err(|e| e.to_string())?
                .join(T::DIRECTORY),
        ))
    }
    pub fn result_path(&self, source: &Source) -> Result<PathBuf> {
        Ok(self
            .root
            .join("sources")
            .join(format!("{}.json", hash(&source.identity()?))))
    }
    fn path(&self, category: &str, key: &str) -> Result<PathBuf> {
        key_valid(key)?;
        Ok(self.root.join(category).join(format!("{}.json", hash(key))))
    }
    fn claim_path(&self, source: &Source) -> Result<PathBuf> {
        Ok(self
            .root
            .join("claims")
            .join(format!("{}.json", hash(&source.identity()?))))
    }
    pub fn bind(&self, key: &str, source: &Source) -> Result<()> {
        let source = source.normalized()?;
        self.files.with_source_lock(&source, || {
            let previous = self.session_link(key)?;
            let input = previous
                .filter(|link| link.source == source)
                .and_then(|link| link.input);
            write(
                &self.path("sessions", key)?,
                &Link {
                    schema_version: VERSION,
                    session_key: key.into(),
                    source: source.clone(),
                    input,
                },
                LINK_LIMIT,
            )
        })
    }
    fn session_link(&self, key: &str) -> Result<Option<Link>> {
        let Some(mut link) = read::<Link>(&self.path("sessions", key)?, LINK_LIMIT)? else {
            return Ok(None);
        };
        if link.schema_version != VERSION
            || link.session_key != key
            || link
                .input
                .as_ref()
                .is_some_and(|input| input.len() > 64 * 1024)
        {
            return Err("Invalid analysis session link".into());
        }
        link.source = link.source.normalized()?;
        Ok(Some(link))
    }
    pub fn source_for_session(&self, key: &str) -> Result<Option<Source>> {
        Ok(self.session_link(key)?.map(|link| link.source))
    }
    /// The most recently captured input survives completion/retirement. It does
    /// not recreate a pending request or authorize a new provider submission.
    pub fn input_for_session(&self, key: &str) -> Result<Option<String>> {
        Ok(self.session_link(key)?.and_then(|link| link.input))
    }
    fn validate_request(&self, key: &str, request: &PendingRequest) -> Result<()> {
        let expected = self
            .source_for_session(key)?
            .ok_or("Analysis session has no saved source")?;
        if source(&request.snapshot).normalized()? != expected
            || request.snapshot.fingerprint.is_empty()
            || request.snapshot.files.len() > 1000
            || request.generation.is_empty()
            || request.generation.len() > 1024
            || request.generation.chars().any(char::is_control)
            || request.prompt.trim().is_empty()
            || request.prompt.len() > 2 * 1024 * 1024
            || request.input.len() > 64 * 1024
        {
            return Err("Analysis request does not match its captured source or limits".into());
        }
        Ok(())
    }
    pub fn save_pending(&self, key: &str, request: &PendingRequest) -> Result<()> {
        let source = source(&request.snapshot);
        self.files.with_source_lock(&source, || {
            self.validate_request(key, request)?;
            let _ = self.pending_for_session(key)?;
            let mut link = self
                .session_link(key)?
                .ok_or("Analysis session has no saved source")?;
            link.input = Some(request.input.clone());
            write(&self.path("sessions", key)?, &link, LINK_LIMIT)?;
            write(
                &self.path("jobs", key)?,
                &Job {
                    schema_version: VERSION,
                    session_key: key.into(),
                    request: request.clone(),
                },
                LIMIT,
            )?;
            write(
                &self.claim_path(&source)?,
                &Claim {
                    schema_version: VERSION,
                    source_id: source.identity()?,
                    session_key: key.into(),
                    generation: request.generation.clone(),
                },
                LINK_LIMIT,
            )
        })
    }
    pub fn pending_for_session(&self, key: &str) -> Result<Option<PendingRequest>> {
        let Some(job) = read::<Job>(&self.path("jobs", key)?, LIMIT)? else {
            return Ok(None);
        };
        if job.schema_version != VERSION || job.session_key != key {
            return Err("Invalid saved analysis request identity".into());
        }
        self.validate_request(key, &job.request)?;
        Ok(Some(job.request))
    }
    pub fn load(&self, source: &Source) -> Result<Option<T>> {
        let Some(record) = read::<Record<T>>(&self.result_path(source)?, LIMIT)? else {
            return Ok(None);
        };
        if record.schema_version != VERSION
            || record.source_id != source.identity()?
            || crate::change_result_store::source(record.result.snapshot()).identity()?
                != source.identity()?
        {
            return Err("Invalid saved analysis result identity".into());
        }
        record.result.validate()?;
        if record.result.snapshot().fingerprint.is_empty() {
            return Err("Saved analysis has no snapshot identity".into());
        }
        Ok(Some(record.result))
    }
    pub fn save_generation(&self, key: &str, result: &T, generation: &str) -> Result<PathBuf> {
        let source = source(result.snapshot());
        self.files.with_source_lock(&source,||{
            result.validate()?;
            let job=self.pending_for_session(key)?.ok_or("Analysis request was retired")?;
            let claim=read::<Claim>(&self.claim_path(&source)?,LINK_LIMIT)?.ok_or("Analysis generation claim is missing")?;
            if job.generation!=generation || result.snapshot()!=&job.snapshot || result.input()!=job.input || claim.schema_version!=VERSION || claim.source_id!=source.identity()? || claim.session_key!=key || claim.generation!=generation {return Err("A newer analysis request superseded this result; the saved result was preserved".into())}
            let _=self.load(&source)?;
            let path=self.result_path(&source)?;
            write(&path,&Record{schema_version:VERSION,source_id:source.identity()?,result},LIMIT)?;
            self.clear(key)?;
            Ok(path)
        })
    }
    fn clear(&self, key: &str) -> Result<()> {
        match fs_err::remove_file(self.path("jobs", key)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Could not retire analysis request: {e}")),
        }
    }
    pub fn retire_generation(&self, key: &str, generation: &str) -> Result<()> {
        let Some(job) = self.pending_for_session(key)? else {
            return Ok(());
        };
        self.files.with_source_lock(&source(&job.snapshot), || {
            if self
                .pending_for_session(key)?
                .is_some_and(|job| job.generation == generation)
            {
                self.clear(key)?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change_description::{Scope, SnapshotFile};
    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct TestResult {
        snapshot: Snapshot,
        input: String,
        text: String,
    }
    impl StoredResult for TestResult {
        const DIRECTORY: &'static str = "test-analysis";
        fn snapshot(&self) -> &Snapshot {
            &self.snapshot
        }
        fn input(&self) -> &str {
            &self.input
        }
        fn validate(&self) -> Result<()> {
            if self.text.is_empty() {
                Err("Invalid result".into())
            } else {
                Ok(())
            }
        }
    }
    fn setup() -> (tempfile::TempDir, Store<TestResult>, PendingRequest) {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = Snapshot {
            checkout: dir.path().to_string_lossy().into_owned(),
            scope: Scope::Local,
            fingerprint: "snapshot".into(),
            label: "Local".into(),
            head: "head".into(),
            base: "head".into(),
            original_body: None,
            title: String::new(),
            files: vec![SnapshotFile {
                path: "a.rs".into(),
                added: 1,
                removed: 0,
                patch: "@@ -0,0 +1 @@\n+new".into(),
                omitted: None,
            }],
        };
        let store = Store::<TestResult>::new(dir.path().join("private"));
        let job = PendingRequest {
            snapshot,
            input: "Review focus: correctness".into(),
            prompt: "Exact customized original prompt\nwith request identity".into(),
            generation: "generation-one".into(),
        };
        (dir, store, job)
    }
    fn result(job: &PendingRequest) -> TestResult {
        TestResult {
            snapshot: job.snapshot.clone(),
            input: job.input.clone(),
            text: "Grounded review".into(),
        }
    }
    #[test]
    fn input_and_exact_prompt_survive_restart_and_invalid_results_preserve_canonical() {
        let (dir, store, job) = setup();
        let src = source(&job.snapshot);
        store.bind("session", &src).unwrap();
        store.save_pending("session", &job).unwrap();
        let reopened = Store::<TestResult>::new(dir.path().join("private"));
        assert_eq!(
            reopened.pending_for_session("session").unwrap(),
            Some(job.clone())
        );
        let answer = result(&job);
        let file = reopened
            .save_generation("session", &answer, &job.generation)
            .unwrap();
        assert_eq!(reopened.load(&src).unwrap(), Some(answer.clone()));
        assert!(reopened.pending_for_session("session").unwrap().is_none());
        let original = fs_err::read(&file).unwrap();
        let mut next = job.clone();
        next.generation = "generation-two".into();
        reopened.save_pending("session", &next).unwrap();
        let mut invalid = answer;
        invalid.text.clear();
        assert!(reopened
            .save_generation("session", &invalid, &next.generation)
            .is_err());
        assert_eq!(fs_err::read(file).unwrap(), original);
    }
    #[test]
    fn source_claim_rejects_older_sessions_and_retirement_never_clears_newer_request() {
        let (_dir, store, one) = setup();
        let src = source(&one.snapshot);
        store.bind("older", &src).unwrap();
        store.bind("newer", &src).unwrap();
        store.save_pending("older", &one).unwrap();
        let mut two = one.clone();
        two.generation = "second".into();
        two.input = "Review focus: data loss".into();
        two.prompt = "Second exact prompt".into();
        store.save_pending("newer", &two).unwrap();
        assert!(store
            .save_generation("older", &result(&one), &one.generation)
            .is_err());
        store.retire_generation("older", &one.generation).unwrap();
        assert_eq!(
            store.pending_for_session("newer").unwrap(),
            Some(two.clone())
        );
        store
            .save_generation("newer", &result(&two), &two.generation)
            .unwrap();
        assert_eq!(store.load(&src).unwrap(), Some(result(&two)));
        store.save_pending("newer", &one).unwrap();
        store.save_pending("newer", &two).unwrap();
        store.retire_generation("newer", &one.generation).unwrap();
        assert_eq!(store.pending_for_session("newer").unwrap(), Some(two));
    }
    #[test]
    fn request_and_result_must_match_bound_snapshot_and_input() {
        let (_dir, store, job) = setup();
        store.bind("session", &source(&job.snapshot)).unwrap();
        store.save_pending("session", &job).unwrap();
        let mut wrong = result(&job);
        wrong.input = "Wrong review focus".into();
        assert!(store
            .save_generation("session", &wrong, &job.generation)
            .is_err());
        wrong = result(&job);
        wrong.snapshot.fingerprint = "different".into();
        assert!(store
            .save_generation("session", &wrong, &job.generation)
            .is_err());
        let mut invalid = job.clone();
        invalid.prompt = " ".into();
        assert!(store.save_pending("session", &invalid).is_err());
        invalid = job.clone();
        invalid.snapshot.files.clear();
        invalid.snapshot.fingerprint.clear();
        assert!(store.save_pending("session", &invalid).is_err());
    }
    #[test]
    fn corrupt_or_oversized_canonical_files_are_preserved() {
        let (_dir, store, job) = setup();
        let src = source(&job.snapshot);
        store.bind("session", &src).unwrap();
        store.save_pending("session", &job).unwrap();
        let file = store
            .save_generation("session", &result(&job), &job.generation)
            .unwrap();
        fs_err::write(&file, "broken").unwrap();
        assert!(store.load(&src).is_err());
        assert_eq!(fs_err::read_to_string(&file).unwrap(), "broken");
        fs_err::write(&file, vec![b' '; LIMIT as usize + 1]).unwrap();
        assert!(store.load(&src).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn result_is_private_and_symbolic_links_are_rejected() {
        use std::os::unix::{fs::symlink, fs::MetadataExt as _};
        let (dir, store, job) = setup();
        let src = source(&job.snapshot);
        store.bind("session", &src).unwrap();
        store.save_pending("session", &job).unwrap();
        let file = store
            .save_generation("session", &result(&job), &job.generation)
            .unwrap();
        assert_eq!(fs_err::metadata(&file).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            fs_err::metadata(file.parent().unwrap()).unwrap().mode() & 0o777,
            0o700
        );
        fs_err::remove_file(&file).unwrap();
        let target = dir.path().join("other.json");
        fs_err::write(&target, "{}").unwrap();
        symlink(&target, &file).unwrap();
        assert!(store.load(&src).is_err());
        assert_eq!(fs_err::read_to_string(&target).unwrap(), "{}");
    }
    #[test]
    fn captured_input_survives_retirement_rebinding_and_second_restart_without_replay() {
        let (dir, store, job) = setup();
        let source = source(&job.snapshot);
        store.bind("session", &source).unwrap();
        store.save_pending("session", &job).unwrap();
        // No provider transcript or canonical result exists for this captured request.
        store.retire_generation("session", &job.generation).unwrap();
        let reopened = Store::<TestResult>::new(dir.path().join("private"));
        assert_eq!(
            reopened.input_for_session("session").unwrap(),
            Some(job.input.clone())
        );
        assert!(reopened.pending_for_session("session").unwrap().is_none());
        assert!(reopened.load(&source).unwrap().is_none());
        reopened.bind("session", &source).unwrap();
        let second = Store::<TestResult>::new(dir.path().join("private"));
        assert_eq!(
            second.input_for_session("session").unwrap(),
            Some(job.input.clone())
        );
        assert!(second.pending_for_session("session").unwrap().is_none());
        second.save_pending("session", &job).unwrap();
        second
            .save_generation("session", &result(&job), &job.generation)
            .unwrap();
        assert_eq!(
            second.input_for_session("session").unwrap(),
            Some(job.input)
        );
        assert!(second.pending_for_session("session").unwrap().is_none());
    }
    #[test]
    fn old_links_load_without_input_and_corrupt_input_link_is_preserved() {
        let (_dir, store, job) = setup();
        let source = source(&job.snapshot);
        store.bind("session", &source).unwrap();
        let path = store.path("sessions", "session").unwrap();
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs_err::read(&path).unwrap()).unwrap();
        legacy.as_object_mut().unwrap().remove("input");
        fs_err::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(
            store.source_for_session("session").unwrap(),
            Some(source.clone())
        );
        assert_eq!(store.input_for_session("session").unwrap(), None);
        fs_err::write(&path, "corrupt link").unwrap();
        assert!(store.input_for_session("session").is_err());
        assert!(store.bind("session", &source).is_err());
        assert!(store.save_pending("session", &job).is_err());
        assert_eq!(fs_err::read_to_string(&path).unwrap(), "corrupt link");
        fs_err::write(&path, vec![b' '; LINK_LIMIT as usize + 1]).unwrap();
        assert!(store.input_for_session("session").is_err());
        assert_eq!(fs_err::metadata(&path).unwrap().len(), LINK_LIMIT + 1);
    }
}
