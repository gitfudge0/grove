//! Atomic recovery heads with a bounded, append-only transcript patch journal.
//! The head commits an exact byte boundary; an interrupted append cannot change
//! the previous recovery snapshot. Old inline JSON sessions remain readable.
use super::{SavedSession, TranscriptItems};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

#[derive(Clone, Serialize, Deserialize)]
struct Journal {
    version: u8,
    file: String,
    end: u64,
    #[serde(default)]
    metadata_prefix: bool,
}
#[derive(Serialize)]
struct Head<'a> {
    #[serde(flatten)]
    saved: &'a SavedSession,
    grove_transcript_journal: &'a Journal,
}
#[derive(Serialize, Deserialize)]
struct Record {
    len: usize,
    changes: Vec<Change>,
}
#[derive(Serialize, Deserialize)]
struct Change {
    item: usize,
    path: Vec<String>,
    #[serde(flatten)]
    edit: Edit,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
enum Edit {
    Set(Value),
    Append(String),
    Extend(Vec<Value>),
    Remove,
}
#[derive(Default)]
struct Writer {
    journal: Option<Journal>,
    previous: TranscriptItems,
    stamp: Option<(u64, SystemTime)>,
    live_bytes: u64,
    item_bytes: Vec<u64>,
}
// Bound retained snapshots. A slot under active use is never evicted, so a
// concurrent explicit save and background flush share the same writer lock.
type Writers = HashMap<PathBuf, Arc<Mutex<Writer>>>;
fn writer(path: &Path) -> Result<Arc<Mutex<Writer>>, String> {
    static WRITERS: OnceLock<Mutex<Writers>> = OnceLock::new();
    let mut writers = WRITERS
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|e| e.to_string())?;
    if !writers.contains_key(path) && writers.len() >= 8 {
        if let Some(key) = writers
            .iter()
            .find(|(_, value)| Arc::strong_count(value) == 1)
            .map(|(key, _)| key.clone())
        {
            writers.remove(&key);
        }
    }
    Ok(writers.entry(path.to_owned()).or_default().clone())
}
fn serialized_size(value: &impl Serialize) -> Result<u64, String> {
    #[derive(Default)]
    struct Count(u64);
    impl Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len() as u64);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count::default();
    serde_json::to_writer(&mut count, value).map_err(|error| error.to_string())?;
    Ok(count.0)
}
fn stamp(path: &Path) -> Option<(u64, SystemTime)> {
    let metadata = fs_err::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}
fn journal_path(dir: &Path, journal: &Journal, key: &str) -> Result<PathBuf, String> {
    if journal.version != 1
        || !journal.file.starts_with(&format!("{key}-"))
        || !journal.file.ends_with(".journal")
        || Path::new(&journal.file)
            .file_name()
            .and_then(|v| v.to_str())
            != Some(&journal.file)
    {
        return Err("Invalid transcript journal reference".into());
    }
    Ok(dir.join(&journal.file))
}
fn diff(item: usize, path: &mut Vec<String>, old: &Value, new: &Value, changes: &mut Vec<Change>) {
    if old == new {
        return;
    }
    match (old, new) {
        (Value::String(old), Value::String(new)) if new.starts_with(old) => {
            changes.push(Change {
                item,
                path: path.clone(),
                edit: Edit::Append(new[old.len()..].into()),
            });
        }
        (Value::Object(old), Value::Object(new)) => {
            for key in old.keys().filter(|key| !new.contains_key(*key)) {
                path.push(key.clone());
                changes.push(Change {
                    item,
                    path: path.clone(),
                    edit: Edit::Remove,
                });
                path.pop();
            }
            for (key, value) in new {
                path.push(key.clone());
                if let Some(previous) = old.get(key) {
                    diff(item, path, previous, value, changes);
                } else {
                    changes.push(Change {
                        item,
                        path: path.clone(),
                        edit: Edit::Set(value.clone()),
                    });
                }
                path.pop();
            }
        }
        (Value::Array(old), Value::Array(new)) if old.len() <= new.len() => {
            for (index, value) in new.iter().take(old.len()).enumerate() {
                path.push(index.to_string());
                diff(item, path, &old[index], value, changes);
                path.pop();
            }
            if new.len() > old.len() {
                changes.push(Change {
                    item,
                    path: path.clone(),
                    edit: Edit::Extend(new[old.len()..].to_vec()),
                });
            }
        }
        _ => changes.push(Change {
            item,
            path: path.clone(),
            edit: Edit::Set(new.clone()),
        }),
    }
}
fn apply_path(value: &mut Value, path: &[String], edit: Edit) -> Result<(), String> {
    if let Some((key, rest)) = path.split_first() {
        match value {
            Value::Object(map) if rest.is_empty() => match edit {
                Edit::Set(new) => {
                    map.insert(key.clone(), new);
                    Ok(())
                }
                Edit::Remove => map
                    .remove(key)
                    .map(|_| ())
                    .ok_or_else(|| "Invalid transcript removal".into()),
                edit => apply_path(
                    map.get_mut(key).ok_or("Invalid transcript patch path")?,
                    rest,
                    edit,
                ),
            },
            Value::Object(map) => apply_path(
                map.get_mut(key).ok_or("Invalid transcript patch path")?,
                rest,
                edit,
            ),
            Value::Array(array) => apply_path(
                key.parse::<usize>()
                    .ok()
                    .and_then(|index| array.get_mut(index))
                    .ok_or("Invalid transcript patch index")?,
                rest,
                edit,
            ),
            _ => Err("Invalid transcript patch path".into()),
        }
    } else {
        match edit {
            Edit::Set(new) => {
                *value = new;
                Ok(())
            }
            Edit::Append(suffix) => match value {
                Value::String(text) => {
                    text.push_str(&suffix);
                    Ok(())
                }
                _ => Err("Invalid transcript text patch".into()),
            },
            Edit::Extend(suffix) => match value {
                Value::Array(array) => {
                    array.extend(suffix);
                    Ok(())
                }
                _ => Err("Invalid transcript array patch".into()),
            },
            Edit::Remove => Err("Invalid transcript root removal".into()),
        }
    }
}
fn replay(items: &mut Vec<Value>, record: Record) -> Result<(), String> {
    items.truncate(record.len);
    // New rows must be introduced by a root Set, rather than allocating from an
    // untrusted length before validating the journal's actual payload.
    for change in record.changes {
        if change.item == items.len() && change.path.is_empty() {
            if let Edit::Set(value) = change.edit {
                items.push(value);
                continue;
            }
            return Err("Invalid transcript append".into());
        }
        let value = items
            .get_mut(change.item)
            .ok_or("Invalid transcript item index")?;
        apply_path(value, &change.path, change.edit)?;
    }
    if items.len() != record.len {
        return Err("Incomplete transcript journal".into());
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct Nested {
    claude_subagents: HashMap<String, super::SavedClaudeSubagent>,
    side_chats: HashMap<String, super::SavedSideChat>,
    rich_plans: crate::rich_plan::PlanStore,
}
#[derive(Serialize)]
struct BorrowedNested<'a> {
    claude_subagents: &'a HashMap<String, super::SavedClaudeSubagent>,
    side_chats: &'a HashMap<String, super::SavedSideChat>,
    rich_plans: &'a crate::rich_plan::PlanStore,
}
fn nested(saved: &SavedSession) -> BorrowedNested<'_> {
    BorrowedNested {
        claude_subagents: &saved.claude_subagents,
        side_chats: &saved.side_chats,
        rich_plans: &saved.rich_plans,
    }
}
fn journal_items(saved: &SavedSession) -> Result<TranscriptItems, String> {
    let metadata = serde_json::to_value(nested(saved)).map_err(|error| error.to_string())?;
    Ok(std::iter::once(super::TranscriptItem::from(metadata))
        .chain(saved.items.iter().cloned())
        .collect())
}
fn strip_head(saved: &SavedSession) -> SavedSession {
    // Construct the head explicitly so clearing nested payloads never first
    // clones their conversation text. New SavedSession fields must be handled
    // here at compile time rather than silently dropped from persistence.
    SavedSession {
        html_policy: saved.html_policy,
        visuals: saved.visuals.clone(),
        session_review: saved.session_review.clone(),
        claude_subagents: HashMap::new(),
        side_chats: HashMap::new(),
        rich_plans: crate::rich_plan::PlanStore::default(),
        claude_resume: saved.claude_resume.clone(),
        fork_origin: saved.fork_origin.clone(),
        edit_checkpoints: saved.edit_checkpoints.clone(),
        pending_edit: saved.pending_edit.clone(),
        rewind_drafts: saved.rewind_drafts.clone(),
        provider: saved.provider,
        key: saved.key.clone(),
        project: saved.project.clone(),
        cwd: saved.cwd.clone(),
        label: saved.label.clone(),
        title: saved.title.clone(),
        title_generation: saved.title_generation.clone(),
        roots: saved.roots.clone(),
        thread_id: saved.thread_id.clone(),
        items: TranscriptItems::default(),
        work_periods: saved.work_periods.clone(),
        draft: saved.draft.clone(),
        draft_images: saved.draft_images.clone(),
        draft_reference: saved.draft_reference.clone(),
        config_args: saved.config_args.clone(),
        native_config_version: saved.native_config_version,
        activity_reporting: saved.activity_reporting,
        composer: saved.composer.clone(),
        context: saved.context,
        context_details: saved.context_details.clone(),
    }
}

fn decode(path: &Path, bytes: &[u8]) -> Result<(SavedSession, Option<Journal>), String> {
    #[derive(Deserialize)]
    struct Reference {
        grove_transcript_journal: Option<Journal>,
    }
    let reference: Reference = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let mut saved: SavedSession = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if let Some(journal) = &reference.grove_transcript_journal {
        if !saved.items.is_empty() {
            return Err("Ambiguous transcript recovery head".into());
        }
        let file = fs_err::File::open(journal_path(
            path.parent().ok_or("Missing session directory")?,
            journal,
            &saved.key,
        )?)
        .map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() < journal.end {
            return Err("Truncated transcript journal".into());
        }
        let reader = std::io::BufReader::new(file.take(journal.end));
        let mut items = Vec::new();
        for record in serde_json::Deserializer::from_reader(reader).into_iter::<Record>() {
            replay(&mut items, record.map_err(|e| e.to_string())?)?;
        }
        if journal.metadata_prefix {
            if items.is_empty() {
                return Err("Missing transcript metadata prefix".into());
            }
            let metadata: Nested =
                serde_json::from_value(items.remove(0)).map_err(|error| error.to_string())?;
            saved.claude_subagents = metadata.claude_subagents;
            saved.side_chats = metadata.side_chats;
            saved.rich_plans = metadata.rich_plans;
        }
        saved.items = items.into();
    }
    Ok((saved, reference.grove_transcript_journal))
}
pub(super) fn load(path: &Path) -> Result<SavedSession, String> {
    let writer = writer(path)?;
    let _guard = writer.lock().map_err(|e| e.to_string())?;
    let bytes = fs_err::read(path).map_err(|e| e.to_string())?;
    match decode(path, &bytes) {
        Ok((saved, _)) => Ok(saved),
        Err(error) => {
            let current = fs_err::read(path).map_err(|e| e.to_string())?;
            if current == bytes {
                Err(error)
            } else {
                decode(path, &current).map(|(saved, _)| saved)
            }
        }
    }
}
pub(super) fn remove(path: &Path) -> Result<(), String> {
    let writer = writer(path)?;
    let mut writer = writer.lock().map_err(|error| error.to_string())?;
    let bytes = match fs_err::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let journal = value
        .get("grove_transcript_journal")
        .map(|value| {
            serde_json::from_value::<Journal>(value.clone()).map_err(|error| error.to_string())
        })
        .transpose()?;
    let journal_path = journal
        .as_ref()
        .map(|journal| {
            journal_path(
                path.parent().ok_or("Missing session directory")?,
                journal,
                value["key"].as_str().ok_or("Missing session key")?,
            )
        })
        .transpose()?;
    fs_err::remove_file(path).map_err(|error| error.to_string())?;
    *writer = Writer::default();
    if let Some(path) = journal_path {
        let _ = fs_err::remove_file(path);
    }
    Ok(())
}
fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let dir = path.parent().ok_or("Missing session directory")?;
    let mut file = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    {
        let mut buffer = std::io::BufWriter::with_capacity(64 * 1024, file.as_file_mut());
        serde_json::to_writer(&mut buffer, value).map_err(|e| e.to_string())?;
        buffer.flush().map_err(|e| e.to_string())?;
    }
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    fs_err::File::open(dir)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}
pub(super) fn persist(saved: &SavedSession, dir: &Path) -> Result<(), String> {
    persist_with_commit(saved, dir, || Ok(()))
}
fn persist_with_commit(
    saved: &SavedSession,
    dir: &Path,
    before_commit: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    fs_err::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.json", saved.key));
    let items = journal_items(saved)?;
    let writer = writer(&path)?;
    let mut writer = writer.lock().map_err(|e| e.to_string())?;
    if writer.stamp != stamp(&path) || writer.journal.is_none() {
        *writer = Writer::default();
        if path.exists() {
            let (previous, journal) =
                decode(&path, &fs_err::read(&path).map_err(|e| e.to_string())?)?;
            let previous_items = journal_items(&previous)?;
            writer.item_bytes = previous_items
                .iter()
                .map(serialized_size)
                .collect::<Result<_, _>>()?;
            writer.live_bytes = writer.item_bytes.iter().sum();
            writer.previous = previous_items;
            writer.journal = journal;
        }
    }
    let compact = items.len() < writer.previous.len()
        || writer.journal.as_ref().is_none_or(|journal| {
            !journal.metadata_prefix
                || journal.end > writer.live_bytes.saturating_mul(3).max(4 * 1024 * 1024)
        });
    let old_journal = writer.journal.clone();
    let (mut file, mut journal) = if compact {
        let file = tempfile::Builder::new()
            .prefix(&format!("{}-", saved.key))
            .suffix(".journal")
            .tempfile_in(dir)
            .map_err(|e| e.to_string())?;
        let (file, journal_path) = file.keep().map_err(|e| e.to_string())?;
        let file = fs_err::File::from_parts(file, journal_path.clone());
        let journal = Journal {
            version: 1,
            file: journal_path
                .file_name()
                .ok_or("Invalid journal name")?
                .to_string_lossy()
                .into_owned(),
            end: 0,
            metadata_prefix: true,
        };
        (file, journal)
    } else {
        let journal = writer.journal.clone().ok_or("Missing transcript journal")?;
        let mut file = fs_err::OpenOptions::new()
            .write(true)
            .open(journal_path(dir, &journal, &saved.key)?)
            .map_err(|e| e.to_string())?;
        // Discard an uncommitted append from a failed/interrupted head write.
        file.set_len(journal.end).map_err(|e| e.to_string())?;
        file.seek(SeekFrom::Start(journal.end))
            .map_err(|e| e.to_string())?;
        (file, journal)
    };
    let mut changes = Vec::new();
    let mut item_bytes = writer.item_bytes.clone();
    item_bytes.resize(items.len(), 0);
    item_bytes.truncate(items.len());
    for (index, item) in items.iter().enumerate() {
        let old = if compact {
            None
        } else {
            writer.previous.get(index)
        };
        if let Some(old) = old {
            if old.same_storage(item) {
                continue;
            }
            diff(index, &mut Vec::new(), old, item, &mut changes);
            item_bytes[index] = serialized_size(item)?;
        } else {
            item_bytes[index] = serialized_size(item)?;
            changes.push(Change {
                item: index,
                path: Vec::new(),
                edit: Edit::Set(item.as_ref().clone()),
            });
        }
    }
    let changed = compact || !changes.is_empty() || writer.previous.len() != items.len();
    if changed {
        let mut buffer = std::io::BufWriter::with_capacity(64 * 1024, &mut file);
        serde_json::to_writer(
            &mut buffer,
            &Record {
                len: items.len(),
                changes,
            },
        )
        .map_err(|e| e.to_string())?;
        buffer.write_all(b"\n").map_err(|e| e.to_string())?;
        buffer.flush().map_err(|e| e.to_string())?;
        drop(buffer);
        journal.end = file.stream_position().map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    let head = strip_head(saved);
    before_commit()?;
    atomic_json(
        &path,
        &Head {
            saved: &head,
            grove_transcript_journal: &journal,
        },
    )?;
    // Only the successfully committed head may become the next diff base.
    writer.previous = items.clone();
    writer.live_bytes = item_bytes.iter().sum();
    writer.item_bytes = item_bytes;
    writer.journal = Some(journal.clone());
    writer.stamp = stamp(&path);
    if compact {
        if let Some(old) = old_journal {
            if old.file != journal.file {
                let _ = fs_err::remove_file(dir.join(old.file));
            }
        }
    }
    Ok(())
}

/// One coalescing disk worker per live actor. Routine flushes never block provider
/// ingestion; explicit saves and actor shutdown are durable completion barriers.
type PendingState = Arc<(Mutex<Pending>, std::sync::Condvar)>;
type DiskWorker = (PendingState, std::thread::JoinHandle<()>);
#[derive(Default)]
pub(crate) struct Background {
    worker: Option<DiskWorker>,
    last: Option<(u64, Vec<u8>)>,
    #[cfg(test)]
    directory: Option<PathBuf>,
}
#[derive(Default)]
struct Pending {
    saved: Option<(u64, SavedSession)>,
    requested: u64,
    completed: u64,
    error: Option<String>,
    shutdown: bool,
}
impl Background {
    fn shared(&mut self) -> PendingState {
        #[cfg(test)]
        let directory = self.directory.clone();
        self.worker
            .get_or_insert_with(|| {
                let shared = Arc::new((Mutex::new(Pending::default()), std::sync::Condvar::new()));
                let worker = shared.clone();
                let join = std::thread::spawn(move || loop {
                    let (lock, wake) = &*worker;
                    let mut pending = lock
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    while pending.saved.is_none() && !pending.shutdown {
                        pending = wake
                            .wait(pending)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                    let Some((generation, saved)) = pending.saved.take() else {
                        break;
                    };
                    drop(pending);
                    #[cfg(not(test))]
                    let result = super::persist(&saved);
                    #[cfg(test)]
                    let result = match &directory {
                        Some(directory) => persist(&saved, directory),
                        None => super::persist(&saved),
                    };
                    let mut pending = lock
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    pending.completed = generation;
                    pending.error = result.err();
                    wake.notify_all();
                });
                (shared, join)
            })
            .0
            .clone()
    }
    pub(crate) fn schedule(&mut self, saved: &SavedSession) -> Result<(), String> {
        let head = strip_head(saved);
        let metadata = serde_json::to_vec(&(head, nested(saved))).map_err(|e| e.to_string())?;
        let stamp = (saved.items.revision(), metadata);
        if self.last.as_ref() == Some(&stamp) {
            return Ok(());
        }
        self.enqueue(saved)?;
        self.last = Some(stamp);
        Ok(())
    }
    fn enqueue(&mut self, saved: &SavedSession) -> Result<u64, String> {
        let shared = self.shared();
        let mut pending = shared.0.lock().map_err(|e| e.to_string())?;
        pending.requested += 1;
        let generation = pending.requested;
        pending.saved = Some((generation, saved.clone()));
        shared.1.notify_one();
        Ok(generation)
    }
    pub(crate) fn drain(&mut self) -> Result<(), String> {
        self.last = None;
        let Some((shared, _)) = &self.worker else {
            return Ok(());
        };
        let mut pending = shared.0.lock().map_err(|e| e.to_string())?;
        let generation = pending.requested;
        while pending.completed < generation {
            pending = shared.1.wait(pending).map_err(|e| e.to_string())?;
        }
        match pending.error.take() {
            Some(error) => {
                self.last = None;
                Err(error)
            }
            None => Ok(()),
        }
    }
    pub(crate) fn flush(&mut self, saved: &SavedSession) -> Result<(), String> {
        self.last = None;
        let generation = self.enqueue(saved)?;
        let shared = self.shared();
        let mut pending = shared.0.lock().map_err(|e| e.to_string())?;
        while pending.completed < generation {
            pending = shared.1.wait(pending).map_err(|e| e.to_string())?;
        }
        match pending.error.take() {
            Some(error) => {
                self.last = None;
                Err(error)
            }
            None => Ok(()),
        }
    }
    pub(crate) fn take_error(&mut self) -> Option<String> {
        let error = self.worker.as_ref()?.0 .0.lock().ok()?.error.take();
        if error.is_some() {
            self.last = None;
        }
        error
    }
}
impl Drop for Background {
    fn drop(&mut self) {
        if let Some((shared, join)) = self.worker.take() {
            if let Ok(mut pending) = shared.0.lock() {
                pending.shutdown = true;
                shared.1.notify_one();
            }
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn saved() -> SavedSession {
        serde_json::from_value(json!({"key":"journal-test","project":"test","cwd":"/unused","label":"test","roots":[],"thread_id":null,"items":[{"id":"a","type":"agentMessage","text":"hello","nested":["reason"]}]})).unwrap()
    }
    fn head_path(dir: &Path) -> PathBuf {
        dir.join("journal-test.json")
    }
    fn journal(dir: &Path) -> Journal {
        serde_json::from_value(
            serde_json::from_slice::<Value>(&fs_err::read(head_path(dir)).unwrap()).unwrap()
                ["grove_transcript_journal"]
                .clone(),
        )
        .unwrap()
    }
    fn force_compaction(dir: &Path) {
        let mut head: Value =
            serde_json::from_slice(&fs_err::read(head_path(dir)).unwrap()).unwrap();
        let journal = journal(dir);
        let path = journal_path(dir, &journal, "journal-test").unwrap();
        let mut file = fs_err::OpenOptions::new().append(true).open(path).unwrap();
        file.write_all(&vec![b' '; 5_000_000]).unwrap();
        head["grove_transcript_journal"]["end"] = serde_json::json!(file.metadata().unwrap().len());
        atomic_json(&head_path(dir), &head).unwrap();
    }
    fn assert_saved(actual: &SavedSession, expected: &SavedSession) {
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }
    #[test]
    fn legacy_migration_patch_roundtrip_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let mut saved = saved();
        fs_err::write(head_path(dir.path()), serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
        persist(&saved, dir.path()).unwrap();
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
        saved.items[0]["text"] = json!("hello 世界");
        saved.items[0]["nested"][0] = json!("reason extended");
        saved
            .items
            .push(json!({"id":"b","type":"commandExecution","aggregatedOutput":"output"}));
        persist(&saved, dir.path()).unwrap();
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
        saved.items[0].as_object_mut().unwrap().remove("nested");
        saved.items[0]["text"] = json!("replacement");
        saved.items.truncate(1);
        persist(&saved, dir.path()).unwrap();
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
        saved.items.clear();
        persist(&saved, dir.path()).unwrap();
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
    }
    #[test]
    fn uncommitted_tail_is_ignored_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let mut saved = saved();
        persist(&saved, dir.path()).unwrap();
        let committed = journal(dir.path());
        let path = journal_path(dir.path(), &committed, &saved.key).unwrap();
        fs_err::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{incomplete append")
            .unwrap();
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
        saved.items[0]["text"] = json!("hello next");
        persist(&saved, dir.path()).unwrap();
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
        let current = journal(dir.path());
        fs_err::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(current.end - 1)
            .unwrap();
        assert!(load(&head_path(dir.path())).is_err());
    }
    #[test]
    fn metadata_and_stream_append_write_only_changed_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let mut saved = saved();
        saved.items[0]["text"] = json!("x".repeat(1_000_000));
        persist(&saved, dir.path()).unwrap();
        let before = journal(dir.path());
        saved.draft = "draft-only".into();
        persist(&saved, dir.path()).unwrap();
        assert_eq!(journal(dir.path()).end, before.end);
        assert!(fs_err::metadata(head_path(dir.path())).unwrap().len() < 10_000);
        saved.items[0]["text"].as_str().unwrap();
        if let Value::String(text) = &mut saved.items[0]["text"] {
            text.push_str(" streamed suffix");
        }
        persist(&saved, dir.path()).unwrap();
        assert!(journal(dir.path()).end - before.end < 300);
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
    }
    #[test]
    fn nested_transcripts_use_metadata_prefix_and_suffix_patches() {
        let dir = tempfile::tempdir().unwrap();
        let mut saved = saved();
        saved.claude_subagents.insert("child".into(), super::super::SavedClaudeSubagent { items: vec![json!({"id":"child-message","type":"agentMessage","text":"x".repeat(1_000_000)})].into(), ..Default::default() });
        saved.side_chats.insert(
            "checkout".into(),
            super::super::SavedSideChat {
                messages: vec![("assistant".into(), "y".repeat(1_000_000))],
                ..Default::default()
            },
        );
        persist(&saved, dir.path()).unwrap();
        let before = journal(dir.path());
        assert!(fs_err::metadata(head_path(dir.path())).unwrap().len() < 10_000);
        if let Value::String(text) =
            &mut saved.claude_subagents.get_mut("child").unwrap().items[0]["text"]
        {
            text.push_str(" child suffix");
        }
        saved.side_chats.get_mut("checkout").unwrap().messages[0]
            .1
            .push_str(" side suffix");
        persist(&saved, dir.path()).unwrap();
        assert!(journal(dir.path()).end - before.end < 1_000);
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
        saved
            .side_chats
            .get_mut("checkout")
            .unwrap()
            .messages
            .push(("user".into(), "new prompt".into()));
        persist(&saved, dir.path()).unwrap();
        assert!(journal(dir.path()).end - before.end < 2_000);
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
        saved.claude_subagents.insert(
            "second-child".into(),
            super::super::SavedClaudeSubagent::default(),
        );
        persist(&saved, dir.path()).unwrap();
        assert!(journal(dir.path()).end - before.end < 3_000);
        saved.claude_subagents.remove("second-child");
        persist(&saved, dir.path()).unwrap();
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
    }
    #[test]
    fn journal_reference_cannot_escape_and_files_are_private() {
        let dir = tempfile::tempdir().unwrap();
        let saved = saved();
        persist(&saved, dir.path()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let journal = journal(dir.path());
            for path in [
                head_path(dir.path()),
                journal_path(dir.path(), &journal, &saved.key).unwrap(),
            ] {
                assert_eq!(
                    fs_err::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
        let mut head: Value =
            serde_json::from_slice(&fs_err::read(head_path(dir.path())).unwrap()).unwrap();
        head["grove_transcript_journal"]["file"] = json!("journal-test-../../outside.journal");
        fs_err::write(head_path(dir.path()), serde_json::to_vec(&head).unwrap()).unwrap();
        assert!(load(&head_path(dir.path())).is_err());
    }
    #[test]
    fn interrupted_compaction_keeps_previous_head_readable() {
        let dir = tempfile::tempdir().unwrap();
        let mut saved = saved();
        persist(&saved, dir.path()).unwrap();
        let old_head = fs_err::read(head_path(dir.path())).unwrap();
        let old = journal(dir.path());
        // Construct and sync a replacement journal without committing its head.
        let orphan = dir.path().join("journal-test-orphan.journal");
        fs_err::write(&orphan, b"incomplete replacement").unwrap();
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
        // Force compaction at a real committed boundary, then ensure both old
        // and new snapshots remain isolated across the head replacement.
        force_compaction(dir.path());
        saved.items[0]["text"] = json!("new transcript");
        persist(&saved, dir.path()).unwrap();
        assert_ne!(journal(dir.path()).file, old.file);
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
        assert_ne!(fs_err::read(head_path(dir.path())).unwrap(), old_head);
    }
    #[test]
    fn failed_head_commit_preserves_previous_transcript_and_retry() {
        let dir = tempfile::tempdir().unwrap();
        let mut saved = saved();
        persist(&saved, dir.path()).unwrap();
        let previous = saved.clone();
        let head = fs_err::read(head_path(dir.path())).unwrap();
        saved.items[0]["text"] = json!("hello uncommitted");
        assert!(
            persist_with_commit(&saved, dir.path(), || Err("injected disk failure".into()))
                .is_err()
        );
        assert_eq!(fs_err::read(head_path(dir.path())).unwrap(), head);
        assert_saved(&load(&head_path(dir.path())).unwrap(), &previous);
        saved.items[0]["text"] = json!("hello committed retry");
        persist(&saved, dir.path()).unwrap();
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
        let old = journal(dir.path());
        force_compaction(dir.path());
        saved.items.clear();
        assert!(persist_with_commit(
            &saved,
            dir.path(),
            || Err("injected compact failure".into())
        )
        .is_err());
        assert_eq!(journal(dir.path()).file, old.file);
        assert!(!load(&head_path(dir.path())).unwrap().items.is_empty());
        persist(&saved, dir.path()).unwrap();
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
    }
    #[test]
    fn background_flush_and_drop_preserve_latest_snapshot_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut background = Background::default();
        background.directory = Some(dir.path().to_owned());
        let mut saved = saved();
        for index in 0..20 {
            saved.draft = format!("draft {index}");
            background.schedule(&saved).unwrap();
        }
        background.flush(&saved).unwrap();
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
        saved.draft = "final queued draft".into();
        background.schedule(&saved).unwrap();
        drop(background);
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
        let bad = dir.path().join("not-a-directory");
        fs_err::write(&bad, "blocked").unwrap();
        let mut background = Background::default();
        background.directory = Some(bad.clone());
        assert!(background.flush(&saved).is_err());
        fs_err::remove_file(&bad).unwrap();
        background.flush(&saved).unwrap();
        assert_saved(&load(&head_path(&bad)).unwrap(), &saved);
    }
    #[test]
    #[ignore = "manual synthetic persistence baseline and changed-byte benchmark"]
    fn journal_persistence_benchmark() {
        let dir = tempfile::tempdir().unwrap();
        let mut saved = saved();
        for index in 0..1_000 {
            saved.items.push(json!({"id":format!("tool-{index}"),"type":"commandExecution","aggregatedOutput":"output\n".repeat(2_000)}));
        }
        let baseline = std::time::Instant::now();
        for _ in 0..5 {
            atomic_json(&dir.path().join("legacy.json"), &saved).unwrap();
        }
        let baseline_ms = baseline.elapsed().as_secs_f64() * 1000.0 / 5.0;
        persist(&saved, dir.path()).unwrap();
        let start = std::time::Instant::now();
        for index in 0..5 {
            saved.draft = format!("draft {index}");
            persist(&saved, dir.path()).unwrap();
        }
        let draft_ms = start.elapsed().as_secs_f64() * 1000.0 / 5.0;
        let before = journal(dir.path()).end;
        let start = std::time::Instant::now();
        for _ in 0..5 {
            if let Value::String(text) = &mut saved.items[0]["text"] {
                text.push_str(" continued");
            }
            persist(&saved, dir.path()).unwrap();
        }
        let append_ms = start.elapsed().as_secs_f64() * 1000.0 / 5.0;
        println!("journal_persistence baseline_ms={baseline_ms:.3} draft_ms={draft_ms:.3} append_ms={append_ms:.3} journal_append_bytes={}", journal(dir.path()).end-before);
        assert_saved(&load(&head_path(dir.path())).unwrap(), &saved);
    }
}
