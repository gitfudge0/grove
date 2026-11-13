//! Provider-neutral session state, commands, persistence, and interaction contracts.
//! Native transports live in `codex` and `claude`; persisted formats stay compatible.
pub use crate::transcript_items::{TranscriptItem, TranscriptItems};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::{
    borrow::Borrow,
    collections::{HashMap, HashSet},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::Instant,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComposerSettings {
    #[serde(default)]
    pub reset_to_configured: bool,
    #[serde(default)]
    pub reset_model_effort_only: bool,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub access: Option<String>,
    /// None preserves a legacy conversation's inherited provider configuration.
    #[serde(default)]
    pub speed: Option<Speed>,
}
fn legacy_composer_settings() -> ComposerSettings {
    ComposerSettings {
        speed: None,
        ..Default::default()
    }
}
impl Default for ComposerSettings {
    fn default() -> Self {
        Self {
            reset_to_configured: false,
            reset_model_effort_only: false,
            model: None,
            effort: None,
            access: None,
            speed: Some(Speed::Standard),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Speed {
    Standard,
    Fast,
    Ultrafast,
}
impl Speed {
    pub fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Fast => "Fast",
            Self::Ultrafast => "Ultrafast",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    Codex,
    Claude,
}
impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
        }
    }
    pub fn agent(self) -> crate::agent::Agent {
        match self {
            Self::Codex => crate::agent::Agent::Codex,
            Self::Claude => crate::agent::Agent::Claude,
        }
    }
    pub fn launch_options(self, args: &[String]) -> Result<(Vec<String>, Option<String>), String> {
        match self {
            Self::Codex => crate::codex::launch_options(args),
            Self::Claude => crate::claude::launch_options(args),
        }
    }
}
/// A locally observed provider turn; item bounds are half-open transcript indices.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkPeriod {
    pub start_item: usize,
    pub end_item: Option<usize>,
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub subagents: Vec<Subagent>,
}
/// Observed child work only; absence of provider data is never completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Working,
    Completed,
    Failed,
    Interrupted,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Subagent {
    pub id: String,
    pub name: String,
    pub activity: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub task: Option<String>,
    pub status: SubagentStatus,
}
/// Provider-visible observations only; deliberately excluded from saved sessions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentActivityKind {
    Tool,
    Commentary,
}
#[derive(Clone, Debug)]
pub struct SubagentActivityEvent {
    pub id: String,
    pub text: String,
    pub kind: SubagentActivityKind,
    pub observed_at: Instant,
}
#[derive(Clone, Debug, Default)]
pub struct SubagentActivity {
    /// Full provider-visible transcript items, in chronological order.
    pub items: TranscriptItems,
    pub events: Vec<SubagentActivityEvent>,
    pub observed_at: Option<Instant>,
    pub refreshing: bool,
    pub refresh_error: bool,
    pub truncated: bool,
    poll_cursor: Option<(u64, u64)>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentTranscriptAvailability {
    Awaiting,
    Live,
    Restored,
    Unavailable,
}
/// Canonical provider-visible Claude child data; monotonic observation clocks
/// and private reasoning are deliberately not persisted.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedClaudeSubagent {
    #[serde(default)]
    pub items: TranscriptItems,
    #[serde(default)]
    pub task_ids: Vec<String>,
    #[serde(default)]
    pub run_id: Option<String>,
}
fn activity_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}
fn bounded_excerpt(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
/// Latest observation per child in this session, including terminal tombstones.
/// Work-period closure describes the parent and does not complete its children.
pub fn active_subagents(periods: &[WorkPeriod]) -> Vec<Subagent> {
    let mut latest: Vec<Subagent> = Vec::new();
    for agent in periods.iter().flat_map(|period| &period.subagents) {
        if let Some(previous) = latest.iter_mut().find(|old| old.id == agent.id) {
            *previous = agent.clone();
        } else {
            latest.push(agent.clone());
        }
    }
    latest.retain(|agent| {
        matches!(
            agent.status,
            SubagentStatus::Working | SubagentStatus::Unknown
        )
    });
    latest
}

fn update_subagent_metadata(agent: &mut Subagent, role: Option<&str>, task: Option<&str>) {
    for (target, observed) in [(&mut agent.role, role), (&mut agent.task, task)] {
        if let Some(text) = observed.filter(|text| !text.trim().is_empty()) {
            *target = Some(text.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }
}

fn collab_status(status: Option<&str>) -> SubagentStatus {
    match status {
        Some("pendingInit" | "running" | "inProgress") => SubagentStatus::Working,
        Some("completed") => SubagentStatus::Completed,
        Some("interrupted") => SubagentStatus::Interrupted,
        Some("errored" | "failed") => SubagentStatus::Failed,
        _ => SubagentStatus::Unknown,
    }
}
fn subagent_activity(item: &Value) -> Option<String> {
    let text = match item["type"].as_str()? {
        "commandExecution" => item["command"].as_str(),
        "mcpToolCall" => item["progress"].as_str().or_else(|| item["tool"].as_str()),
        "agentMessage" => item["text"].as_str(),
        "fileChange" => return Some("Editing files".into()),
        "webSearch" => item["query"].as_str(),
        "reasoning" => return Some("Thinking".into()),
        _ => None,
    }?;
    (!text.trim().is_empty()).then(|| text.into())
}
pub(crate) fn record_subagent(
    period: &mut WorkPeriod,
    id: &str,
    name: Option<&str>,
    activity: Option<&str>,
    status: SubagentStatus,
) {
    if id.is_empty() {
        return;
    }
    let agent = if let Some(index) = period.subagents.iter().position(|a| a.id == id) {
        &mut period.subagents[index]
    } else {
        period.subagents.push(Subagent {
            id: id.into(),
            name: format!("Agent {}", period.subagents.len() + 1),
            activity: "Activity unavailable".into(),
            role: None,
            task: None,
            status,
        });
        let index = period.subagents.len() - 1;
        &mut period.subagents[index]
    };
    if let Some(name) = name.filter(|s| !s.trim().is_empty()) {
        agent.name = name.trim().into();
    }
    if let Some(activity) = activity.filter(|s| !s.trim().is_empty()) {
        agent.activity = activity
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(240)
            .collect();
    }
    if status != SubagentStatus::Unknown
        && !(status == SubagentStatus::Working
            && matches!(
                agent.status,
                SubagentStatus::Completed | SubagentStatus::Failed | SubagentStatus::Interrupted
            ))
    {
        agent.status = status;
    }
}
/// Immutable selection captured from eligible transcript messages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceSource {
    pub message_id: String,
    pub role: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceDraft {
    pub id: String,
    pub thread_id: Option<String>,
    pub sources: Vec<ReferenceSource>,
    pub text: String,
}
impl ReferenceDraft {
    pub(crate) fn is_canonical_prompt(text: &str) -> bool {
        let Some((encoded, reply)) = text
            .strip_prefix("Referenced transcript selection (quoted data):\n")
            .and_then(|body| body.split_once("\n\nUser reply:\n"))
        else {
            return false;
        };
        serde_json::from_str::<Self>(encoded).is_ok_and(|reference| {
            !reference.id.is_empty()
                && !reference.sources.is_empty()
                && reference.serialize(reply) == text
        })
    }
    /// JSON encodes the literal quote and metadata so quoted delimiters, slash
    /// commands and markup remain ordinary prompt data.
    pub fn serialize(&self, text: &str) -> String {
        format!(
            "Referenced transcript selection (quoted data):\n{}\n\nUser reply:\n{}",
            serde_json::json!(self),
            text
        )
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedSideChat {
    /// Provider state for the independent resumable side conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Box<SavedSession>>,
    #[serde(default)]
    pub draft: String,
    #[serde(default)]
    pub messages: Vec<(String, String)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedSession {
    /// App-owned review receipt; independent of focus/unread acknowledgment.
    #[serde(default)]
    pub session_review: SessionReview,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub claude_subagents: HashMap<String, SavedClaudeSubagent>,
    #[serde(default)]
    pub rich_plans: crate::rich_plan::PlanStore,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub side_chats: HashMap<String, SavedSideChat>,
    #[serde(default)]
    pub claude_resume: Option<ClaudeResume>,
    #[serde(default)]
    pub fork_origin: Option<ForkOrigin>,
    #[serde(default)]
    pub edit_checkpoints: Vec<EditCheckpoint>,
    #[serde(default)]
    pub pending_edit: Option<PendingEdit>,
    #[serde(default)]
    pub rewind_drafts: Vec<SavedComposerDraft>,
    #[serde(default)]
    pub provider: Provider,
    pub key: String,
    pub project: String,
    pub cwd: String,
    pub label: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub title_generation: Option<crate::session_title::Generation>,
    pub roots: Vec<crate::session_meta::ContextRoot>,
    pub thread_id: Option<String>,
    pub items: TranscriptItems,
    #[serde(default)]
    pub work_periods: Vec<WorkPeriod>,
    #[serde(default)]
    pub draft: String,
    #[serde(default)]
    pub draft_images: Vec<String>,
    #[serde(default)]
    pub draft_reference: Option<ReferenceDraft>,
    #[serde(default)]
    pub config_args: Vec<String>,
    /// Version 1 marks startup options supplied specifically for a native session.
    /// Missing/zero predates CLI/native isolation and requires reconnect migration.
    #[serde(default)]
    pub native_config_version: u8,
    #[serde(default)]
    pub activity_reporting: bool,
    #[serde(default = "legacy_composer_settings")]
    pub composer: ComposerSettings,
    /// Last known context-window occupancy, so a reopened session shows its meter.
    #[serde(default)]
    pub context: Option<ContextUsage>,
    /// Optional attribution for the exact latest context measurement.
    #[serde(default)]
    pub context_details: Option<ContextDetails>,
}
/// A monotonic receipt protects newer results from stale review actions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionReview {
    /// Exclusive transcript boundary acknowledged by the user.
    #[serde(default)]
    pub reviewed_through: usize,
    /// Provider-authored brief, independent of subsequent conversational replies.
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub sequence: usize,
    #[serde(default)]
    pub last_completed_period: Option<usize>,
    #[serde(default)]
    pub pending: Option<ReviewCompletion>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewCompletion {
    pub id: usize,
    pub period: usize,
}
/// Durable startup recipe and title hold for a duplicated conversation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ForkOrigin {
    pub source_thread: Option<String>,
    pub last_turn: Option<String>,
    pub users: usize,
}

/// A Claude fork is not persisted by the CLI until its first prompt. Retain the
/// exact launch recipe across that window so reconnect cannot lose prior context.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClaudeResume {
    pub original_session: Option<String>,
    pub assistant_uuid: Option<String>,
    pub human_uuid: String,
    pub fork_id: String,
}
/// Workspace state immediately before a provider turn began.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditCheckpoint {
    pub start_item: usize,
    pub turn_id: Option<String>,
    #[serde(default)]
    pub files: Option<crate::checkpoint::Checkpoint>,
}
/// Durable recovery journal: no provider request is replayed automatically.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingEdit {
    pub target: usize,
    pub turn_id: String,
    pub text: String,
    pub images: Vec<String>,
    #[serde(default)]
    pub recovery: Option<crate::checkpoint::Checkpoint>,
    /// Rewind-first handoff, distinct from the legacy inline editor.
    #[serde(default)]
    pub rewind: bool,
    #[serde(default)]
    pub composer_draft: Option<(String, Vec<String>)>,
    #[serde(default)]
    pub claude_fork_id: Option<String>,
}
/// A complete composer retained separately when rewinding a conversation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedComposerDraft {
    pub id: String,
    pub text: String,
    pub images: Vec<String>,
    pub reference: Option<ReferenceDraft>,
}
impl SavedSession {
    /// Copy a completed prefix. A user boundary excludes that message and restores its input.
    pub fn fork_at(&self, boundary: usize, restore_user: bool) -> Result<Self, String> {
        if boundary > self.items.len() || self.pending_edit.is_some() {
            return Err("This conversation boundary is unavailable.".into());
        }
        let user = self
            .items
            .get(boundary)
            .filter(|item| restore_user && item["type"] == "userMessage");
        let last_user = self
            .items
            .iter()
            .take(boundary)
            .rposition(|item| item["type"] == "userMessage");
        let last_turn = last_user
            .and_then(|index| {
                self.edit_checkpoints.iter().find(|cp| {
                    self.items
                        .iter()
                        .enumerate()
                        .skip(cp.start_item)
                        .find(|(_, item)| item["type"] == "userMessage")
                        .is_some_and(|(at, _)| at == index)
                })
            })
            .and_then(|cp| cp.turn_id.clone());
        if self.provider == Provider::Codex && last_user.is_some() && last_turn.is_none() {
            return Err("No provider turn boundary is available for this message.".into());
        }
        if last_user.is_some() && self.thread_id.is_none() {
            return Err("The source provider conversation is unavailable.".into());
        }
        let mut fork = self.clone();
        let hex = crate::identity::random_id()?;
        let id = format!(
            "{}-{}-4{}-8{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[13..16],
            &hex[17..20],
            &hex[20..]
        );
        fork.key = format!("fork-{id}");
        fork.title = Some(format!(
            "Forked · {}",
            self.title.as_deref().unwrap_or(&self.label)
        ));
        fork.title_generation = None;
        fork.thread_id = None;
        fork.claude_resume = None;
        fork.items.truncate(boundary);
        fork.draft = user.map_or_else(String::new, |item| item_text(item));
        fork.draft_images = user
            .and_then(|item| item["content"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|part| {
                if part["type"] == "image" {
                    part["url"].as_str().map(str::to_owned)
                } else if part["type"] == "localImage" {
                    part["path"].as_str().map(str::to_owned)
                } else {
                    None
                }
            })
            .collect();
        fork.draft_reference = None;
        fork.session_review = SessionReview::default();
        fork.side_chats.clear();
        fork.rewind_drafts.clear();
        fork.context = None;
        fork.context_details = None;
        fork.rich_plans.retain_messages(&fork.items);
        fork.edit_checkpoints.retain(|cp| cp.start_item < boundary);
        fork.work_periods
            .retain(|period| period.start_item < boundary);
        for period in &mut fork.work_periods {
            period.end_item = Some(period.end_item.unwrap_or(boundary).min(boundary));
        }
        fork.retain_claude_subagents();
        if self.provider == Provider::Claude && last_user.is_some() {
            let anchor = fork
                .items
                .iter()
                .rev()
                .find_map(|item| item["claudeAssistantUuid"].as_str())
                .ok_or("No Claude assistant boundary is available for this response.")?;
            fork.claude_resume = Some(ClaudeResume {
                original_session: self
                    .claude_resume
                    .as_ref()
                    .and_then(|resume| resume.original_session.clone())
                    .or_else(|| self.thread_id.clone()),
                assistant_uuid: Some(anchor.into()),
                human_uuid: String::new(),
                fork_id: id.clone(),
            });
            fork.thread_id = Some(id);
        }
        fork.fork_origin = Some(ForkOrigin {
            source_thread: if self.provider == Provider::Codex && last_user.is_some() {
                self.thread_id.clone()
            } else {
                None
            },
            last_turn,
            users: fork
                .items
                .iter()
                .filter(|item| item["type"] == "userMessage")
                .count(),
        });
        Ok(fork)
    }

    pub(crate) fn finish_rewind_draft(&mut self, text: String, images: Vec<String>) {
        if !self.draft.is_empty() || !self.draft_images.is_empty() || self.draft_reference.is_some()
        {
            let id = format!(
                "rewind-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                self.rewind_drafts.len()
            );
            self.rewind_drafts.push(SavedComposerDraft {
                id,
                text: std::mem::take(&mut self.draft),
                images: std::mem::take(&mut self.draft_images),
                reference: self.draft_reference.take(),
            });
        }
        self.draft_reference = None;
        if ReferenceDraft::is_canonical_prompt(&text) {
            let parsed = text
                .strip_prefix("Referenced transcript selection (quoted data):\n")
                .and_then(|body| body.split_once("\n\nUser reply:\n"));
            if let Some((encoded, reply)) = parsed {
                self.draft_reference = serde_json::from_str(encoded).ok();
                self.draft = reply.to_owned();
            } else {
                self.draft = text;
            }
        } else {
            self.draft = text;
        }
        self.draft_images = images;
    }
    pub(crate) fn restore_rewind_draft(&mut self, id: &str) -> Result<(), String> {
        let index = self
            .rewind_drafts
            .iter()
            .position(|draft| draft.id == id)
            .ok_or("This saved draft is no longer available.")?;
        let mut draft = self.rewind_drafts.remove(index);
        std::mem::swap(&mut self.draft, &mut draft.text);
        std::mem::swap(&mut self.draft_images, &mut draft.images);
        std::mem::swap(&mut self.draft_reference, &mut draft.reference);
        if !draft.text.is_empty() || !draft.images.is_empty() || draft.reference.is_some() {
            self.rewind_drafts.insert(index, draft);
        }
        Ok(())
    }

    pub(crate) fn retain_claude_subagents(&mut self) {
        let children: HashSet<&str> = self
            .work_periods
            .iter()
            .flat_map(|period| &period.subagents)
            .map(|agent| agent.id.as_str())
            .collect();
        self.claude_subagents.retain(|id, child| {
            child.items.retain(|item| {
                !matches!(
                    item["type"].as_str(),
                    Some("reasoning" | "reasoningMessage")
                )
            });
            children.contains(id.as_str())
        });
    }
    /// Run only when reconnecting/restoring, never against a live provider.
    pub(crate) fn migrate_native_config(&mut self) {
        if self.native_config_version != 0 {
            return;
        }
        match self.provider {
            Provider::Claude => self
                .config_args
                .retain(|arg| arg != "--dangerously-skip-permissions"),
            Provider::Codex => {
                // This exact pair was emitted by the old global CLI preference.
                // Do not erase individual custom settings or composer selections.
                let approval = "approval_policy=\"never\"";
                let sandbox = "sandbox_mode=\"danger-full-access\"";
                let has = |value: &str| {
                    self.config_args.windows(2).any(|pair| {
                        matches!(pair[0].as_str(), "-c" | "--config") && pair[1] == value
                    })
                };
                let bundle = has(approval) && has(sandbox);
                let mut migrated = Vec::new();
                let mut i = 0;
                while i < self.config_args.len() {
                    if self.config_args[i] == "--dangerously-bypass-approvals-and-sandbox" {
                        i += 1;
                    } else if bundle
                        && matches!(self.config_args[i].as_str(), "-c" | "--config")
                        && self
                            .config_args
                            .get(i + 1)
                            .is_some_and(|v| v == approval || v == sandbox)
                    {
                        i += 2;
                    } else {
                        migrated.push(self.config_args[i].clone());
                        i += 1;
                    }
                }
                self.config_args = migrated;
            }
        }
        self.native_config_version = 1;
    }

    pub fn empty(key: String, provider: Provider, project: String, cwd: String) -> Self {
        Self {
            session_review: crate::provider::SessionReview::default(),
            rich_plans: crate::rich_plan::PlanStore::default(),
            side_chats: std::collections::HashMap::default(),
            fork_origin: None,
            claude_resume: None,
            claude_subagents: std::collections::HashMap::default(),
            edit_checkpoints: Vec::new(),
            pending_edit: None,
            rewind_drafts: Vec::new(),
            provider,
            composer: ComposerSettings::default(),
            context: None,
            context_details: None,
            key,
            project,
            cwd,
            label: provider.label().into(),
            title: None,
            title_generation: None,
            roots: Vec::new(),
            thread_id: None,
            items: TranscriptItems::default(),
            work_periods: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
            activity_reporting: false,
        }
    }

    /// Start an independent conversation with the current provider preferences.
    #[must_use]
    pub fn independent_conversation(&self, key: String) -> Self {
        let mut saved = self.fresh_conversation();
        saved.key = key;
        saved.rewind_drafts.clear();
        saved.session_review = SessionReview::default();
        saved
    }

    /// Persist a host-created native conversation before starting its transport.
    pub fn persist_recovery(&self) -> Result<(), String> {
        persist(self)
    }

    pub(crate) fn fresh_conversation(&self) -> Self {
        let mut saved = self.clone();
        saved.thread_id = None;
        saved.claude_resume = None;
        saved.fork_origin = None;
        saved.edit_checkpoints.clear();
        saved.pending_edit = None;
        saved.title = None;
        saved.title_generation = None;
        saved.items.clear();
        saved.claude_subagents.clear();
        saved.rich_plans = crate::rich_plan::PlanStore::default();
        saved.side_chats.clear();
        saved.work_periods.clear();
        saved.session_review.pending = None;
        saved.session_review.last_completed_period = None;
        saved.session_review.reviewed_through = 0;
        saved.session_review.summary.clear();
        saved.draft.clear();
        saved.draft_images.clear();
        saved.draft_reference = None;
        saved.context = None;
        saved.context_details = None;
        saved
    }
    /// Consume the accepted composer copy, preserving any newer text/attachment pair.
    pub(crate) fn clear_submitted_draft(&mut self, text: &str, images: &[String]) {
        let canonical = self
            .draft_reference
            .as_ref()
            .map(|reference| reference.serialize(&self.draft));
        if canonical.as_deref().unwrap_or(&self.draft) == text && self.draft_images == images {
            self.draft.clear();
            self.draft_images.clear();
            self.draft_reference = None;
        }
    }
}

/// Provider-neutral context-window occupancy in tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsage {
    pub used: u64,
    pub window: u64,
}
impl ContextUsage {
    /// Whole percent of the window in use, capped at 100.
    pub fn percent(self) -> u32 {
        if self.window == 0 {
            return 0;
        }
        ((self.used as f64 / self.window as f64 * 100.0).round() as u32).min(100)
    }
}
/// Attribution is optional: a missing breakdown must never be reconstructed from spend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextDetails {
    /// Request accounting is not content attribution or context-window composition.
    #[serde(default)]
    pub accounting: Option<ContextTokenAccounting>,
    pub model: Option<String>,
    /// Local receipt time, in Unix seconds; persisted measurements are last-known data.
    pub measured_at: u64,
    pub source: ContextMeasurement,
    pub categories: Vec<ContextCategory>,
    pub auto_compact_enabled: Option<bool>,
    /// Provider-reported threshold, in tokens; absent when not supplied.
    pub auto_compact_threshold: Option<u64>,
}
/// Provider-reported accounting; cached/reasoning counts overlap input/output totals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextTokenAccounting {
    pub last: Option<ContextTokenCounters>,
    pub total: Option<ContextTokenCounters>,
}
/// Missing fields stay unknown, while an explicit zero remains a known count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextTokenCounters {
    pub total_tokens: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
}
impl ContextTokenCounters {
    fn from_provider_value(value: &Value) -> Option<Self> {
        let counters = Self {
            total_tokens: value["totalTokens"].as_u64(),
            input_tokens: value["inputTokens"].as_u64(),
            cached_input_tokens: value["cachedInputTokens"].as_u64(),
            cache_write_input_tokens: value["cacheWriteInputTokens"].as_u64(),
            output_tokens: value["outputTokens"].as_u64(),
            reasoning_output_tokens: value["reasoningOutputTokens"].as_u64(),
        };
        (counters != Self::default()).then_some(counters)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextMeasurement {
    ProviderUsage,
    Full,
    Estimate,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCategory {
    pub id: String,
    pub label: String,
    pub tokens: u64,
    pub kind: ContextCategoryKind,
    pub entries: Vec<ContextEntry>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextCategoryKind {
    Used,
    Free,
    Buffer,
    Deferred,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextEntry {
    pub label: String,
    pub tokens: u64,
}
fn context_receipt_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
/// Inline error when clearing is requested while provider work is active.
pub const CLEAR_BUSY: &str =
    "Wait for the current turn to finish before clearing the conversation.";
/// Inline error when clearing is requested without a live provider process.
pub const CLEAR_DISCONNECTED: &str = "Reconnect before clearing the conversation.";

/// Inline error when compaction is requested while a turn is active.
pub const COMPACT_BUSY: &str = "Wait for the current turn to finish before compacting.";
/// Inline error when compaction is requested before any conversation exists.
pub const NOTHING_TO_COMPACT: &str = "There's nothing to compact yet.";
/// Inline error when compaction is requested without a live provider process.
pub const COMPACT_DISCONNECTED: &str = "Reconnect before compacting.";
/// The provider-neutral transcript item type for a completed or running compaction.
pub const COMPACTION_ITEM: &str = "contextCompaction";
/// Compact token count: `950`, `4.2k`, `182k`, `1.2M`.
pub fn compact_tokens(tokens: u64) -> String {
    fn trimmed(value: f64, suffix: &str) -> String {
        let text = format!("{value:.1}");
        format!("{}{suffix}", text.strip_suffix(".0").unwrap_or(&text))
    }
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=9_949 => trimmed(tokens as f64 / 1_000.0, "k"),
        9_950..=999_499 => format!("{}k", (tokens as f64 / 1_000.0).round() as u64),
        _ => trimmed(tokens as f64 / 1_000_000.0, "M"),
    }
}
/// "Context compacted · auto · 182k → 24k"; the trigger and figures appear only when known.
pub fn compaction_label(item: &Value) -> String {
    let mut label = String::from("Context compacted");
    if let Some(trigger) = item["trigger"].as_str() {
        let _ = write!(label, " · {trigger}");
    }
    if let (Some(before), Some(after)) = (item["preTokens"].as_u64(), item["postTokens"].as_u64()) {
        let _ = write!(
            label,
            " · {} \u{2192} {}",
            compact_tokens(before),
            compact_tokens(after)
        );
    }
    label
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmissionMode {
    Send,
    Steer,
    Queue,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedPrompt {
    pub id: u64,
    pub text: String,
    pub images: Vec<String>,
}
/// A live provider request. Answers are deliberately never persisted in SavedSession.
#[derive(Clone, Debug)]
pub struct QuestionRequest {
    pub id: Value,
    pub questions: Vec<Question>,
    pub title: Option<String>,
    pub(crate) claude_input: Option<Value>,
    pub(crate) source: QuestionSource,
    pub(crate) instance: u64,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum QuestionSource {
    ServerRequest,
    AsyncMessage,
}
#[derive(Clone, Debug)]
pub struct Question {
    pub id: String,
    pub header: String,
    pub text: String,
    pub options: Vec<QuestionOption>,
    pub multiple: bool,
    pub allow_custom: bool,
    pub secret: bool,
    pub kind: QuestionKind,
    pub placeholder: Option<String>,
    pub description: Option<String>,
}
#[derive(Clone, Debug)]
pub enum QuestionKind {
    Choice,
    Text,
    Number(QuestionNumber),
}
#[derive(Clone, Debug)]
pub struct QuestionNumber {
    pub min: f64,
    pub max: f64,
    pub step: Option<f64>,
    pub default_value: Option<f64>,
    pub unit: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub preview: Option<String>,
}
pub type QuestionAnswers = HashMap<String, Vec<String>>;
impl QuestionRequest {
    /// Async message questions remain live after their originating turn ends.
    pub fn turn_bound(&self) -> bool {
        self.source != QuestionSource::AsyncMessage
    }

    // Async questions are assistant-message metadata, not pending JSON-RPC requests.
    // Their answers enter the conversation as ordinary user input.
    pub(crate) fn from_async_message(item: &Value) -> Option<Self> {
        if item["type"] != "agentMessage"
            || item["delivery"] != "async"
            || item["groveQuestionResolved"] == true
        {
            return None;
        }
        let id = item["id"].as_str()?.to_owned();
        let rows = item["questions"].as_array()?;
        let normalized = rows.iter().enumerate().map(|(index, row)| {
            json!({"id":index.to_string(),"question":row["title"],"isOther":true,
                "options":row["options"].as_array().map(|options| options.iter().map(|label| json!({"label":label})).collect::<Vec<_>>())})
        }).collect::<Vec<_>>();
        let mut request = Self::parse(json!(id), &json!({"questions":normalized}), false).ok()?;
        request.source = QuestionSource::AsyncMessage;
        Some(request)
    }
    pub(crate) fn answer_text(&self, answers: &QuestionAnswers) -> String {
        self.questions
            .iter()
            .map(|question| format!("{}\n{}", question.text, answers[&question.id].join(", ")))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
    /// The ordinary user message emitted when answering an asynchronous question.
    /// Never retain secret answers for transcript deduplication.
    pub fn answer_echo_text(&self, answers: &QuestionAnswers) -> Option<String> {
        (self.source == QuestionSource::AsyncMessage
            && self.questions.iter().all(|question| !question.secret))
        .then(|| self.answer_text(answers))
    }
    /// Decode the app-server's structured input request without retaining answers.
    pub fn from_codex(id: Value, params: &Value) -> Result<Self, String> {
        Self::parse(id, params, false)
    }
    pub(crate) fn parse(id: Value, input: &Value, claude: bool) -> Result<Self, String> {
        if !(id.is_string() || id.is_number()) {
            return Err("Invalid question request identity".into());
        }
        let rows = input["questions"]
            .as_array()
            .filter(|rows| !rows.is_empty())
            .ok_or("The question request contains no questions")?;
        let mut questions = Vec::new();
        for row in rows {
            let text = row["question"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or("The question text is missing")?
                .to_owned();
            let key = if claude {
                Some(text.as_str())
            } else {
                row["id"].as_str()
            }
            .filter(|s| !s.trim().is_empty())
            .ok_or("The question identity is missing")?
            .to_owned();
            if questions.iter().any(|q: &Question| q.id == key) {
                return Err("The question request contains duplicate identities".into());
            }
            let options: Vec<QuestionOption> = if row["options"].is_null() {
                Vec::new()
            } else {
                serde_json::from_value(row["options"].clone())
                    .map_err(|_| "Invalid question choices")?
            };
            for (index, option) in options.iter().enumerate() {
                if option.label.trim().is_empty()
                    || options[..index].iter().any(|o| o.label == option.label)
                {
                    return Err("The question choices need unique nonempty labels".into());
                }
            }
            let kind = if claude {
                match row["kind"].as_str().unwrap_or("choice") {
                    "choice" => QuestionKind::Choice,
                    "text" => QuestionKind::Text,
                    "number" => {
                        let min = row["min"]
                            .as_f64()
                            .filter(|value| value.is_finite())
                            .ok_or("The numeric question minimum is missing")?;
                        let max = row["max"]
                            .as_f64()
                            .filter(|value| value.is_finite())
                            .ok_or("The numeric question maximum is missing")?;
                        let number_field = |key: &str| -> Result<Option<f64>, String> {
                            match row.get(key) {
                                None => Ok(None),
                                Some(value) => value
                                    .as_f64()
                                    .filter(|number| number.is_finite())
                                    .map(Some)
                                    .ok_or_else(|| {
                                        format!("The numeric question {key} must be a number")
                                    }),
                            }
                        };
                        let step = number_field("step")?;
                        let default_value = number_field("defaultValue")?;
                        if min >= max
                            || step.is_some_and(|value| !value.is_finite() || value <= 0.0)
                            || default_value.is_some_and(|value| {
                                !value.is_finite() || value < min || value > max
                            })
                        {
                            return Err("The numeric question constraints are invalid".into());
                        }
                        QuestionKind::Number(QuestionNumber {
                            min,
                            max,
                            step,
                            default_value,
                            unit: row["unit"].as_str().map(str::to_owned),
                        })
                    }
                    _ => return Err("Unsupported question kind".into()),
                }
            } else if options.is_empty() {
                QuestionKind::Text
            } else {
                QuestionKind::Choice
            };
            if !matches!(kind, QuestionKind::Choice)
                && (!options.is_empty() || row["multiSelect"] == true)
            {
                return Err("Text and number questions cannot have choices".into());
            }
            questions.push(Question {
                id: key,
                header: row["header"].as_str().unwrap_or_default().to_owned(),
                text,
                allow_custom: claude
                    || row["isOther"].as_bool().unwrap_or(false)
                    || options.is_empty(),
                multiple: claude && row["multiSelect"].as_bool().unwrap_or(false),
                secret: row["isSecret"].as_bool().unwrap_or(false),
                options,
                kind,
                placeholder: row["placeholder"].as_str().map(str::to_owned),
                description: row["description"].as_str().map(str::to_owned),
            });
        }
        Ok(Self {
            id,
            questions,
            title: input["title"].as_str().map(str::to_owned),
            claude_input: claude.then(|| input.clone()),
            source: QuestionSource::ServerRequest,
            instance: {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },
        })
    }
    pub fn validate(&self, answers: &QuestionAnswers) -> Result<(), String> {
        if answers.len() != self.questions.len() {
            return Err("Answer every question before submitting".into());
        }
        for question in &self.questions {
            let values = answers
                .get(&question.id)
                .ok_or("Answer every question before submitting")?;
            if values.is_empty()
                || (!question.multiple && values.len() != 1)
                || values.iter().enumerate().any(|(i, value)| {
                    value.trim().is_empty()
                        || values[..i].contains(value)
                        || (!question.allow_custom
                            && !question.options.iter().any(|o| o.label == *value))
                })
            {
                return Err("Choose or enter a valid answer for every question".into());
            }
            if let QuestionKind::Number(number) = &question.kind {
                let text = values[0].trim();
                let unsigned = text.strip_prefix('-').unwrap_or(text);
                let mut pieces = unsigned.split('.');
                let whole = pieces.next().unwrap_or_default();
                let fraction = pieces.next();
                let decimal = !whole.is_empty()
                    && whole.bytes().all(|byte| byte.is_ascii_digit())
                    && fraction.is_none_or(|part| {
                        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                    })
                    && pieces.next().is_none();
                let valid = decimal
                    && text.parse::<f64>().ok().is_some_and(|value| {
                        value.is_finite() && value >= number.min && value <= number.max
                    });
                if !valid {
                    return Err(format!(
                        "Enter a number between {} and {}",
                        number.min, number.max
                    ));
                }
            }
        }
        Ok(())
    }
    pub(crate) fn codex_response(&self, answers: QuestionAnswers) -> Value {
        let answers: serde_json::Map<String, Value> = answers
            .into_iter()
            .map(|(key, answers)| (key, json!({"answers":answers})))
            .collect();
        json!({"answers":answers})
    }
    pub(crate) fn claude_response(&self, answers: QuestionAnswers) -> Value {
        let mut input = self.claude_input.clone().unwrap_or_else(|| json!({}));
        input["answers"] = answers
            .into_iter()
            .map(|(key, values)| (key, Value::String(values.join(", "))))
            .collect::<serde_json::Map<String, Value>>()
            .into();
        json!({"behavior":"allow","updatedInput":input})
    }
}
/// Ephemeral successful answer delivery; deliberately contains no answer content.
#[derive(Clone, Debug)]
pub struct QuestionAnswerAck {
    pub sequence: u64,
    pub id: Value,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
}
/// Provider-supplied recovery metadata. Raw messages remain available verbatim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionNoticeKind {
    Retrying,
    Stopped,
    UsageLimit,
}
#[derive(Clone, Debug)]
pub struct SessionNotice {
    pub kind: SessionNoticeKind,
    pub provider: &'static str,
    pub body: String,
    pub reset_at: Option<i64>,
    pub usage_url: Option<&'static str>,
}
/// Native operation support represented by this snapshot, not permission to act.
/// Codex steering is part of its transport; editing follows observed history mode.
/// Claude editing uses its native fork support and steering follows live state.
/// Callers must also enforce connection, turn and recovery availability.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub edit_messages: bool,
    pub redirect_turns: bool,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Explicit reconnect attempt, while the underlying runtime is Starting.
    pub reconnecting: bool,
    restored_subagents: HashSet<String>,
    pub saved: SavedSession,
    /// A title request is running; never persisted across reconnects.
    pub title_generation_pending: bool,
    pub subagent_activity: HashMap<String, SubagentActivity>,
    pub subagent_activity_evicted: HashSet<String>,
    pub status: Status,
    pub turn_id: Option<String>,
    pub work_started: Option<Instant>,
    pub approvals: Vec<Value>,
    pub interaction_owners: HashMap<String, crate::interaction::InteractionOwner>,
    pub interaction_epoch: u64,
    pub(crate) retired_interactions: HashSet<String>,
    pub questions: Vec<QuestionRequest>,
    pub question_answer_acks: Vec<QuestionAnswerAck>,
    pub elicitations: Vec<crate::elicitation::ElicitationRequest>,
    pub error: Option<String>,
    pub notice: Option<SessionNotice>,
    rate_limit_reset: Option<i64>,
    pub(crate) mcp_statuses: HashMap<String, (String, String, bool)>,
    pub sandbox: Option<Value>,
    pub approval_policy: Option<Value>,
    /// Modes advertised by the installed Claude CLI, not assumed SDK capabilities.
    pub permission_modes: Vec<String>,
    pub models: Vec<Value>,
    pub skills: Vec<Value>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub access: Option<String>,
    pub resolved_model: Option<String>,
    pub resolved_effort: Option<String>,
    pub resolved_access: Option<String>,
    /// Provider-reported state only; selecting a preference is not an acknowledgement.
    pub reported_speed: Option<String>,
    pub speed_disabled_reason: Option<String>,
    /// Monotonic local acknowledgement; only a successful turn/start response advances it.
    pub accepted_prompts: u64,
    pub cleared_conversations: u64,
    pub edited_messages: u64,
    /// Acknowledges rewind/draft-swap dispatch, including rejected attempts.
    pub rewind_attempts: u64,
    pub edit_supported: bool,
    pub can_redirect: bool,
    pub accepted_submission: Option<u64>,
    /// Session-local receipts survive coalesced snapshots and out-of-order turn replies.
    pub accepted_submissions: std::collections::HashSet<u64>,
    pub rejected_submission: Option<u64>,
    pub queued_prompts: Vec<QueuedPrompt>,
    pub queue_edit: Option<u64>,
    pub queue_sending: Vec<u64>,
    pub queue_steering: Vec<u64>,
    pub queue_uncertain: Vec<u64>,
    /// The provider is compacting the conversation context right now.
    pub compacting: bool,
    pub context_query_pending: bool,
    /// A context count was received on this connection, not only restored from disk.
    pub context_measurement_live: bool,
    pub context_query_error: Option<String>,
    /// Reject context queries whose inputs changed while counting.
    pub(crate) context_revision: u64,
    /// Grove asked for the next compaction, so it is labelled manual.
    pub(crate) compact_requested: bool,
    /// Latest context occupancy even when the window is still unknown.
    pub(crate) context_tokens: Option<u64>,
    pub(crate) settled_turns: Vec<String>,
    subagent_restart_calls: Vec<String>,
    pub(crate) subagent_observation_generation: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Starting,
    Idle,
    Working,
    Waiting,
    Done,
    Interrupted,
    Failed,
    Disconnected,
}
impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::Idle => "Idle",
            Self::Working => "Working",
            Self::Waiting => "Needs you",
            Self::Done => "Done",
            Self::Interrupted => "Interrupted",
            Self::Failed => "Failed",
            Self::Disconnected => "Disconnected",
        }
    }
    pub fn busy(self) -> bool {
        matches!(self, Self::Starting | Self::Working | Self::Waiting)
    }
}
pub(crate) fn next_interaction_epoch() -> u64 {
    static EPOCH: AtomicU64 = AtomicU64::new(1);
    EPOCH.fetch_add(1, Ordering::Relaxed)
}
impl Snapshot {
    /// Includes requests accepted locally but not yet acknowledged by the provider.
    pub fn provider_work_active(&self) -> bool {
        self.status.busy() || self.turn_id.is_some() || self.compacting || self.compact_requested
    }

    /// Revision of the inputs to a context measurement. UI consumers can coalesce
    /// one optional query per observed revision instead of counting on redraw.
    pub fn context_measurement_revision(&self) -> u64 {
        self.context_revision
    }
    /// Read capabilities from the same live state used by transport action gates.
    /// Keeping this derived avoids a second cache becoming stale on reconnect.
    pub fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            edit_messages: self.edit_supported,
            redirect_turns: self.saved.provider == Provider::Codex || self.can_redirect,
        }
    }

    /// Shared active-turn availability, including provider-specific steering readiness.
    pub fn can_redirect_active_turn(&self) -> bool {
        matches!(self.status, Status::Working | Status::Waiting)
            && self.turn_id.is_some()
            && !self.compacting
            && self.saved.pending_edit.is_none()
            && self.capabilities().redirect_turns
    }

    /// Only the first title request animates its prompt-derived placeholder.
    pub fn initial_title_pending(&self) -> bool {
        self.title_generation_pending
            && self
                .saved
                .title_generation
                .as_ref()
                .is_none_or(|generation| !generation.generated)
    }

    pub fn accept_submission(&mut self, id: u64) {
        self.accepted_submission = Some(id);
        self.accepted_submissions.insert(id);
    }

    pub fn submission_accepted(&self, id: u64) -> bool {
        self.accepted_submission == Some(id) || self.accepted_submissions.contains(&id)
    }

    /// Pending successful result identity. This is deliberately not an unread flag.
    pub fn review_completion(&self) -> Option<usize> {
        if self.reconnecting || !matches!(self.status, Status::Done | Status::Idle) {
            return None;
        }
        let receipt = self.saved.session_review.pending.as_ref()?;
        let period = self.saved.work_periods.get(receipt.period)?;
        (receipt.period + 1 == self.saved.work_periods.len()
            && period
                .end_item
                .is_some_and(|end| end <= self.saved.items.len()))
        .then_some(receipt.id)
    }
    pub fn mark_reviewed(&mut self, work_period: usize) -> Result<(), String> {
        if self.plan_ready() {
            return Err("Review the proposed plan before marking this result reviewed".into());
        }
        if self.review_completion() != Some(work_period) {
            return Err("This review belongs to a previous result".into());
        }
        let reviewed_through = self
            .saved
            .session_review
            .pending
            .as_ref()
            .and_then(|receipt| self.saved.work_periods.get(receipt.period))
            .and_then(|period| period.end_item)
            .ok_or("This review belongs to a previous result")?;
        self.saved.session_review.reviewed_through = reviewed_through;
        self.saved.session_review.summary.clear();
        self.saved.session_review.pending = None;
        Ok(())
    }
    /// Called only after a provider's successful foreground completion boundary.
    pub(crate) fn completed_for_review(&mut self) {
        let Some(period) = self.saved.work_periods.len().checked_sub(1) else {
            return;
        };
        let Some(end_item) = self.saved.work_periods[period].end_item else {
            return;
        };
        if self.saved.session_review.last_completed_period == Some(period) {
            return;
        }
        let work = &self.saved.work_periods[period];
        for item in &self.saved.items[work.start_item..end_item] {
            if item["type"] == "agentMessage" && item["phase"] != "commentary" {
                let text = item_text(item);
                if let Some(brief) = crate::review_brief::extract(&text) {
                    brief.clone_into(&mut self.saved.session_review.summary);
                }
            }
        }
        self.saved.session_review.last_completed_period = Some(period);
        self.saved.session_review.sequence = self.saved.session_review.sequence.saturating_add(1);
        self.saved.session_review.pending = Some(ReviewCompletion {
            id: self.saved.session_review.sequence,
            period,
        });
    }
    pub fn plan_ready(&self) -> bool {
        if self.review_completion().is_none() {
            return false;
        }
        let Some(receipt) = self.saved.session_review.pending.as_ref() else {
            return false;
        };
        let Some(period) = self.saved.work_periods.get(receipt.period) else {
            return false;
        };
        let mut latest = HashSet::new();
        self.saved.rich_plans.revisions.iter().rev().any(|plan| {
            if !latest.insert(&plan.document.plan_id) {
                return false;
            }
            let actionable = matches!(
                plan.review.delivery,
                crate::rich_plan::ResponseDelivery::Draft
                    | crate::rich_plan::ResponseDelivery::Failed { .. }
            ) && matches!(
                plan.review.approval,
                crate::rich_plan::ResponseDelivery::Draft
                    | crate::rich_plan::ResponseDelivery::Failed { .. }
            );
            actionable
                && self.saved.items.iter().enumerate().any(|(index, item)| {
                    index >= period.start_item
                        && index < period.end_item.unwrap_or(0)
                        && item["id"].as_str() == Some(plan.message_id.as_str())
                })
        })
    }
    /// Waiting is proven by an open parent wait tool, never by a child roster.
    pub fn waiting_on_work(&self) -> bool {
        if self.status != Status::Working {
            return false;
        }
        if self.saved.provider == Provider::Claude
            && self.turn_id.as_deref() == Some("awaiting-session-idle")
        {
            return true;
        }
        let Some(period) = self.saved.work_periods.last() else {
            return false;
        };
        self.saved.items.iter().skip(period.start_item).any(|item| {
            if item["status"] != "inProgress" {
                return false;
            }
            let tool = item["tool"].as_str().unwrap_or_default();
            match item["type"].as_str() {
                Some("collabAgentToolCall") => matches!(tool, "wait" | "waitAgent" | "waitAgents"),
                Some("mcpToolCall") => {
                    tool == "mcp__collaboration__wait_agent"
                        || (tool == "wait_agent" && item["server"] == "collaboration")
                        || (tool == "TaskOutput"
                            && item["server"] == "Claude Code"
                            && item["arguments"]["block"] == true)
                }
                _ => false,
            }
        })
    }
    /// One semantic projection for all session navigation surfaces.
    pub fn session_status(&self) -> &'static str {
        if self.reconnecting && self.status == Status::Starting {
            return "Reconnecting";
        }
        if !self.approvals.is_empty() {
            return "Approval needed";
        }
        if !self.questions.is_empty() || !self.elicitations.is_empty() {
            return "Answer needed";
        }
        if matches!(
            self.status,
            Status::Failed | Status::Interrupted | Status::Done
        ) && self
            .effective_notice()
            .is_some_and(|notice| notice.kind == SessionNoticeKind::UsageLimit)
        {
            return "Usage limit";
        }
        if self.waiting_on_work() {
            return "Waiting";
        }
        if self.plan_ready() {
            return "Plan ready";
        }
        if self.review_completion().is_some() {
            return "Review";
        }
        match self.status {
            Status::Idle | Status::Done => "Ready",
            status => status.label(),
        }
    }
    pub fn subagent_transcript_availability(&self, id: &str) -> SubagentTranscriptAvailability {
        if let Some(activity) = self
            .subagent_activity
            .get(id)
            .filter(|activity| !activity.items.is_empty())
        {
            return if activity.observed_at.is_some() {
                SubagentTranscriptAvailability::Live
            } else {
                SubagentTranscriptAvailability::Restored
            };
        }
        if self.restored_subagents.contains(id)
            || (self.saved.provider == Provider::Claude && self.subagent_terminal(id))
        {
            SubagentTranscriptAvailability::Unavailable
        } else {
            SubagentTranscriptAvailability::Awaiting
        }
    }
    /// Reset the raw error together with its provider recovery metadata.
    pub fn clear_error(&mut self) {
        self.error = None;
        self.notice = None;
    }
    /// Direct callers can still clear/replace `error`; never reuse stale metadata.
    pub fn effective_notice(&self) -> Option<&SessionNotice> {
        self.notice
            .as_ref()
            .filter(|notice| self.error.as_deref() == Some(notice.body.as_str()))
    }
    pub(crate) fn set_provider_error(
        &mut self,
        provider: &'static str,
        body: String,
        retrying: bool,
        code: Option<&str>,
        reset_at: Option<i64>,
    ) {
        let preceding = self
            .effective_notice()
            .filter(|notice| notice.provider == provider)
            .cloned();
        let usage = matches!(code, Some("usageLimitExceeded" | "usage_limit_reached"))
            || body.to_lowercase().contains("you’ve hit your usage limit")
            || body.to_lowercase().contains("you've hit your usage limit")
            || (matches!(code, Some("rate_limit_error" | "rate_limit"))
                && preceding
                    .as_ref()
                    .is_some_and(|notice| notice.kind == SessionNoticeKind::UsageLimit));
        let reset_at = reset_at
            .or_else(|| {
                if usage || matches!(code, Some("rate_limit_error" | "rate_limit")) || retrying {
                    preceding.as_ref().and_then(|notice| notice.reset_at)
                } else {
                    None
                }
            })
            .or_else(|| {
                if usage && provider == "Codex" {
                    self.rate_limit_reset
                } else {
                    None
                }
            });
        self.notice = Some(SessionNotice {
            kind: if retrying {
                SessionNoticeKind::Retrying
            } else if usage {
                SessionNoticeKind::UsageLimit
            } else {
                SessionNoticeKind::Stopped
            },
            provider,
            body: body.clone(),
            reset_at,
            usage_url: if usage {
                match provider {
                    "Codex" => Some("https://chatgpt.com/settings/usage"),
                    "Claude" => Some("https://claude.ai/settings/usage"),
                    _ => None,
                }
            } else {
                None
            },
        });
        self.error = Some(body);
    }
    pub(crate) fn clear_retry_notice(&mut self) {
        if self
            .effective_notice()
            .is_some_and(|notice| notice.kind == SessionNoticeKind::Retrying)
        {
            self.clear_error();
        }
    }
    pub(crate) fn provider_warning(&mut self, title: &str, text: &str) {
        if self.saved.items.last().is_some_and(|item| {
            item["type"] == "providerNotice"
                && item["title"] == title
                && item["text"] == text
                && item["severity"] == "warning"
        }) {
            return;
        }
        self.saved.items.push(json!({"id":format!("provider-notice-{}", self.saved.items.len()), "type":"providerNotice", "title":title, "text":text, "severity":"warning"}));
    }

    /// MCP health is independent of the model turn. Keep diagnostics in the
    /// existing literal transcript notice, never in the fatal session error.
    pub(crate) fn mcp_server_status(
        &mut self,
        name: &str,
        status: &str,
        error: Option<&str>,
        needs_auth: bool,
    ) {
        let detail = error.unwrap_or_default();
        let current = (status.to_owned(), detail.to_owned(), needs_auth);
        if self.mcp_statuses.get(name) == Some(&current) {
            return;
        }
        self.mcp_statuses.insert(name.to_owned(), current);
        if !matches!(status, "failed" | "needs-auth") {
            return;
        }
        let cli = match self.saved.provider {
            Provider::Codex => "Codex",
            Provider::Claude => "Claude Code",
        };
        let action = if needs_auth || status == "needs-auth" {
            format!("Authenticate this MCP server with the {cli} CLI, then reconnect this conversation.")
        } else {
            format!(
                "Check this server with the {cli} CLI, then reconnect this conversation to retry."
            )
        };
        let text = if detail.is_empty() {
            action
        } else {
            format!("{detail}\n\n{action}")
        };
        self.provider_warning(&format!("MCP server {name} is unavailable"), &text);
    }

    pub(crate) fn retire_interaction(&mut self, id: &Value) {
        self.interaction_owners.remove(&id.to_string());
        self.retired_interactions.insert(id.to_string());
    }

    pub fn interaction_agent(&self, id: &Value) -> Option<&str> {
        self.interaction_owners
            .get(&id.to_string())
            .and_then(|owner| owner.agent.as_deref())
    }
    pub(crate) fn expire_interactions(&mut self, thread: &str) {
        let ids: Vec<_> = self
            .interaction_owners
            .iter()
            .filter(|(_, owner)| owner.thread == thread)
            .map(|(id, _)| id.clone())
            .collect();
        self.approvals
            .retain(|a| !ids.contains(&a["id"].to_string()));
        self.questions.retain(|q| !ids.contains(&q.id.to_string()));
        self.elicitations
            .retain(|r| !ids.contains(&r.id.to_string()));
        self.retired_interactions.extend(ids);
        self.interaction_owners
            .retain(|_, owner| owner.thread != thread);
    }
    pub(crate) fn refresh_interaction_status(&mut self) {
        if !self.approvals.is_empty() || !self.questions.is_empty() || !self.elicitations.is_empty()
        {
            self.status = Status::Waiting;
        } else if self.status == Status::Waiting
            || (self.status == Status::Working && self.turn_id.is_none())
        {
            self.status = if self.turn_id.is_some() {
                Status::Working
            } else {
                Status::Idle
            };
        }
    }

    /// Keep effective sandbox and approvals separate: an unrestricted filesystem
    /// does not imply that the provider will stop asking for approval.
    pub fn configured_access_summary(&self) -> String {
        let sandbox = match self.sandbox.as_ref().and_then(|v| v["type"].as_str()) {
            Some("dangerFullAccess") => "No sandbox",
            Some("readOnly") => "Read only",
            Some("workspaceWrite") => "Workspace access",
            Some("externalSandbox") => "External sandbox",
            _ => return "Configured access".into(),
        };
        let approvals = match self.approval_policy.as_ref().and_then(Value::as_str) {
            Some("never") => "never ask",
            Some("on-request") => "ask on request",
            Some("on-failure") => "ask on failure",
            Some("untrusted") => "ask for untrusted commands",
            _ => "configured approvals",
        };
        format!("{sandbox} · {approvals}")
    }

    /// Exact catalog entry for the currently selected or provider-resolved model.
    pub fn speed_model(&self) -> Option<&Value> {
        let selected = self.model.as_deref().or(self.resolved_model.as_deref());
        self.models.iter().find(|m| {
            selected.is_some_and(|value| {
                m["model"].as_str() == Some(value)
                    || m["id"].as_str() == Some(value)
                    || m["resolvedModel"].as_str() == Some(value)
            })
        })
    }
    pub fn codex_speed_tier(&self, speed: Speed) -> Option<String> {
        if speed == Speed::Standard {
            return Some("default".into());
        }
        let model = self.speed_model()?;
        let matches = |id: &str| match speed {
            Speed::Fast => matches!(id, "fast" | "priority"),
            Speed::Ultrafast => id == "ultrafast",
            Speed::Standard => false,
        };
        model["serviceTiers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tier| tier["id"].as_str())
            .find(|id| matches(id))
            .or_else(|| {
                model["additionalSpeedTiers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .find(|id| matches(id))
            })
            .map(str::to_owned)
    }
    pub fn speed_unavailable(&self, speed: Speed) -> Option<&'static str> {
        if speed == Speed::Standard {
            return None;
        }
        match self.saved.provider {
            Provider::Codex if self.codex_speed_tier(speed).is_some() => None,
            Provider::Codex => Some("Not advertised for this model or account by Codex."),
            Provider::Claude if speed == Speed::Ultrafast => {
                Some("Claude Code does not offer Ultrafast.")
            }
            Provider::Claude
                if self
                    .speed_model()
                    .is_some_and(|m| m["supportsFastMode"] == true) =>
            {
                None
            }
            Provider::Claude => {
                Some("Fast requires a supported Opus model and an eligible account.")
            }
        }
    }

    pub(crate) fn question_answer_ack(&self, id: Value) -> QuestionAnswerAck {
        QuestionAnswerAck {
            sequence: self
                .question_answer_acks
                .last()
                .map_or(1, |ack| ack.sequence + 1),
            id: id.clone(),
            thread_id: self
                .interaction_owners
                .get(&id.to_string())
                .map(|owner| owner.thread.clone())
                .or_else(|| self.saved.thread_id.clone()),
            turn_id: self
                .interaction_owners
                .get(&id.to_string())
                .and_then(|owner| owner.turn.clone())
                .or_else(|| self.turn_id.clone()),
        }
    }

    pub(crate) fn mark_queue_uncertain(&mut self) {
        for id in &self.queue_sending {
            self.saved.rich_plans.uncertain_delivery(*id);
            if !self.queue_uncertain.contains(id) {
                self.queue_uncertain.push(*id);
            }
        }
    }
    pub(crate) fn dismiss_uncertain_queued(&mut self, id: u64) -> Result<(), String> {
        if !self.queue_uncertain.contains(&id) {
            return Err("This queued input has no unconfirmed delivery".into());
        }
        self.queued_prompts.retain(|p| p.id != id);
        self.queue_sending.retain(|p| *p != id);
        self.queue_steering.retain(|p| *p != id);
        self.queue_uncertain.retain(|p| *p != id);
        Ok(())
    }

    fn rewind_checkpoint(&self, index: usize) -> Result<&EditCheckpoint, String> {
        if !self.edit_supported {
            return Err("Editing requires a conversation with rewind support. Older legacy conversations cannot be edited.".into());
        }
        if self.saved.provider == Provider::Claude {
            let item = self
                .saved
                .items
                .get(index)
                .ok_or("This message is no longer available.")?;
            if item["claudeHumanUuid"].as_str().is_none()
                || (self.saved.items[..index].iter().any(|item| {
                    matches!(item["type"].as_str(), Some("userMessage" | "agentMessage"))
                }) && item["claudePreviousAssistantUuid"].as_str().is_none())
            {
                return Err("No Claude conversation anchor is available. Only newly captured turns can be edited.".into());
            }
        }
        if self.saved.pending_edit.is_some() {
            return Err("An interrupted edit needs recovery. Reconnect to reconcile the conversation before continuing.".into());
        }
        if self.status.busy()
            || self.turn_id.is_some()
            || self.compacting
            || !self.queued_prompts.is_empty()
        {
            return Err("Finish the active turn and remove queued messages before editing.".into());
        }
        if self.status == Status::Disconnected {
            return Err("Reconnect before editing a message.".into());
        }
        if self
            .saved
            .items
            .get(index)
            .is_none_or(|item| item["type"] != "userMessage")
        {
            return Err("This message is no longer available.".into());
        }
        self.saved.edit_checkpoints.iter().find(|checkpoint| {
            checkpoint.turn_id.is_some() && self.saved.items.iter().enumerate()
                .skip(checkpoint.start_item).find(|(_, item)| item["type"] == "userMessage")
                .is_some_and(|(first, _)| first == index)
        }).ok_or_else(|| "No conversation checkpoint is available for this message. Only captured turn boundaries can be rewound.".into())
    }
    pub fn rewind_target(&self, index: usize) -> Result<EditCheckpoint, String> {
        self.rewind_checkpoint(index).cloned()
    }
    pub fn rewind_files_unavailable(&self, index: usize) -> Option<String> {
        let target = match self.rewind_checkpoint(index) {
            Ok(target) => target,
            Err(error) => return Some(error),
        };
        let Some(files) = &target.files else {
            return Some("No workspace checkpoint is available for this message. You can rewind and keep the files.".into());
        };
        let mut current = vec![PathBuf::from(&self.saved.cwd)];
        current.extend(
            self.saved
                .roots
                .iter()
                .map(|root| PathBuf::from(&root.wt_path)),
        );
        let mut current = match crate::checkpoint::repository_roots(&current) {
            Ok(roots) => roots,
            Err(error) => return Some(error),
        };
        let mut captured = files.roots();
        current.sort();
        captured.sort();
        if current != captured {
            return Some("The session’s worktrees changed after this checkpoint. You can rewind and keep the files.".into());
        }
        if let Err(error) = files.validate_linked_worktrees() {
            return Some(error);
        }
        match workspace_recovery_blocked(&current, Some(&self.saved.key)) {
            Ok(true) => return Some("Another conversation has unresolved workspace recovery in these worktrees. You can rewind and keep the files.".into()),
            Err(error) => return Some(error),
            Ok(false) => {}
        }
        None
    }
    pub fn editable_message(&self, index: usize) -> Result<&EditCheckpoint, String> {
        let checkpoint = self.rewind_checkpoint(index)?;
        checkpoint
            .files
            .as_ref()
            .ok_or("No workspace checkpoint is available for this message.")?;
        Ok(checkpoint)
    }
    pub fn queue_editable(&self, id: u64) -> bool {
        self.queued_prompts.iter().any(|p| p.id == id)
            && !self.saved.rich_plans.is_delivery(id)
            && !self.queue_sending.contains(&id)
            && !self.queue_uncertain.contains(&id)
    }
    pub(crate) fn begin_queue_edit(&mut self, id: u64) -> Result<(), String> {
        if !self.queue_editable(id) || self.queue_edit.is_some_and(|held| held != id) {
            return Err("This queued input cannot be edited right now".into());
        }
        self.queue_edit = Some(id);
        Ok(())
    }
    pub(crate) fn save_queue_edit(
        &mut self,
        id: u64,
        text: String,
        images: Vec<String>,
    ) -> Result<(), String> {
        if self.queue_edit != Some(id) || !self.queue_editable(id) {
            return Err("This queued input is no longer being edited".into());
        }
        validate_prompt(&text, &images)?;
        let prompt = self
            .queued_prompts
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or("This queued input is no longer being edited")?;
        prompt.text = text;
        prompt.images = images;
        self.queue_edit = None;
        Ok(())
    }
    pub(crate) fn remove_queued(&mut self, id: u64) -> Result<(), String> {
        if self.queue_sending.contains(&id) || self.queue_uncertain.contains(&id) {
            return Err("This queued input is already being sent".into());
        }
        self.queued_prompts.retain(|p| p.id != id);
        self.saved.rich_plans.cancel_delivery(id);
        if self.queue_edit == Some(id) {
            self.queue_edit = None;
        }
        Ok(())
    }

    pub fn new(mut saved: SavedSession) -> Self {
        if saved.title.is_none() {
            saved.title = first_prompt_title(&saved.items);
        }
        // Older sessions saved activity items without adding their children to the roster.
        for (index, item) in saved.items.iter().enumerate() {
            if item["type"] != "subAgentActivity" || item["kind"] != "started" {
                continue;
            }
            let Some(id) = item["agentThreadId"].as_str() else {
                continue;
            };
            if saved.thread_id.as_deref() == Some(id) {
                continue;
            }
            let period = saved
                .work_periods
                .iter()
                .rposition(|period| {
                    period.start_item <= index && period.end_item.is_none_or(|end| index < end)
                })
                .unwrap_or_else(|| {
                    let start = saved.items[..index]
                        .iter()
                        .rposition(|item| item["type"] == "userMessage")
                        .unwrap_or(0);
                    let end = saved.items[index + 1..]
                        .iter()
                        .position(|item| item["type"] == "userMessage")
                        .map_or(saved.items.len(), |offset| index + 1 + offset);
                    saved.work_periods.push(WorkPeriod {
                        start_item: start,
                        end_item: Some(end),
                        duration_ms: None,
                        subagents: Vec::new(),
                    });
                    saved.work_periods.len() - 1
                });
            // Legacy activity supplies a fallback roster only. Its task path
            // must not replace the provider's subsequently observed nickname.
            if saved.work_periods[period]
                .subagents
                .iter()
                .any(|agent| agent.id == id)
            {
                continue;
            }
            record_subagent(
                &mut saved.work_periods[period],
                id,
                item["agentPath"]
                    .as_str()
                    .and_then(|path| path.rsplit('/').next()),
                None,
                SubagentStatus::Unknown,
            );
        }
        if saved.provider == Provider::Claude {
            // Older snapshots can contain shell tasks in the child roster. Only
            // discard entries whose primary transcript proves they are tools;
            // missing transcript data is not evidence that a child is invalid.
            let tools: HashSet<&str> = saved
                .items
                .iter()
                .filter_map(|item| {
                    let non_agent = item["type"] == "commandExecution"
                        || (item["type"] == "mcpToolCall"
                            && item["tool"]
                                .as_str()
                                .is_some_and(|tool| !matches!(tool, "Agent" | "Task")));
                    non_agent.then(|| item["id"].as_str()).flatten()
                })
                .collect();
            for period in &mut saved.work_periods {
                period
                    .subagents
                    .retain(|agent| !tools.contains(agent.id.as_str()));
            }
        }
        // Monotonic clocks cannot survive recovery. Preserve bounds without inventing time.
        for period in &mut saved.work_periods {
            if period.end_item.is_none() {
                period.end_item = Some(saved.items.len());
            }
            // A restored child remains visible, but is not known to still be running.
            for agent in &mut period.subagents {
                if agent.status == SubagentStatus::Working {
                    agent.status = SubagentStatus::Unknown;
                }
            }
        }
        saved.rich_plans.recover_delivery();
        saved.retain_claude_subagents();
        let restored_subagents = saved
            .work_periods
            .iter()
            .flat_map(|period| &period.subagents)
            .map(|agent| agent.id.clone())
            .collect();
        let subagent_activity = saved
            .claude_subagents
            .iter()
            .filter(|(_, child)| !child.items.is_empty())
            .map(|(id, child)| {
                (
                    id.clone(),
                    SubagentActivity {
                        items: child
                            .items
                            .iter()
                            .filter(|item| {
                                !matches!(
                                    item["type"].as_str(),
                                    Some("reasoning" | "reasoningMessage")
                                )
                            })
                            .cloned()
                            .collect(),
                        ..Default::default()
                    },
                )
            })
            .collect();
        let composer = saved.composer.clone();
        let context_tokens = saved.context.map(|usage| usage.used);
        Self {
            saved,
            title_generation_pending: false,
            reconnecting: false,
            status: Status::Starting,
            turn_id: None,
            work_started: None,
            approvals: Vec::new(),
            interaction_owners: HashMap::new(),
            interaction_epoch: next_interaction_epoch(),
            retired_interactions: HashSet::new(),
            questions: Vec::new(),
            question_answer_acks: Vec::new(),
            elicitations: Vec::new(),
            error: None,
            notice: None,
            rate_limit_reset: None,
            mcp_statuses: HashMap::new(),
            sandbox: None,
            approval_policy: None,
            permission_modes: Vec::new(),
            models: Vec::new(),
            skills: Vec::new(),
            model: composer.model,
            effort: composer.effort,
            access: composer.access,
            resolved_model: None,
            resolved_effort: None,
            resolved_access: None,
            reported_speed: None,
            speed_disabled_reason: None,
            accepted_prompts: 0,
            cleared_conversations: 0,
            edited_messages: 0,
            rewind_attempts: 0,
            edit_supported: false,
            can_redirect: false,
            accepted_submission: None,
            accepted_submissions: HashSet::default(),
            rejected_submission: None,
            queued_prompts: Vec::new(),
            queue_edit: None,
            queue_sending: Vec::new(),
            queue_steering: Vec::new(),
            queue_uncertain: Vec::new(),
            compacting: false,
            context_query_pending: false,
            context_measurement_live: false,
            context_query_error: None,
            context_revision: 0,
            compact_requested: false,
            context_tokens,
            settled_turns: Vec::new(),
            subagent_restart_calls: Vec::new(),
            subagent_observation_generation: 0,
            restored_subagents,
            subagent_activity,
            subagent_activity_evicted: HashSet::new(),
        }
    }
    pub(crate) fn begin_work(&mut self) {
        if self.work_started.is_some() {
            return;
        }
        self.work_started = Some(Instant::now());
        self.saved.session_review.pending = None;
        self.saved.work_periods.push(WorkPeriod {
            start_item: self.saved.items.len(),
            end_item: None,
            duration_ms: None,
            subagents: Vec::new(),
        });
    }
    pub(crate) fn subagent_terminal(&self, id: &str) -> bool {
        self.saved
            .work_periods
            .iter()
            .rev()
            .flat_map(|p| p.subagents.iter().rev())
            .find(|a| a.id == id)
            .is_some_and(|a| {
                matches!(
                    a.status,
                    SubagentStatus::Completed
                        | SubagentStatus::Failed
                        | SubagentStatus::Interrupted
                )
            })
    }
    pub(crate) fn ensure_subagent_activity(&mut self, id: &str) {
        if !self.subagent_activity.contains_key(id) && self.subagent_activity.len() >= 100 {
            if let Some(oldest) = self
                .subagent_activity
                .iter()
                .filter(|(_, activity)| activity.items.is_empty())
                .min_by_key(|(id, a)| (!self.subagent_terminal(id), a.observed_at))
                .map(|(id, _)| id.clone())
            {
                self.subagent_activity.remove(&oldest);
                if self.subagent_activity_evicted.len() >= 100 {
                    self.subagent_activity_evicted.clear();
                }
                self.subagent_activity_evicted.insert(oldest);
            }
        }
        let evicted = self.subagent_activity_evicted.contains(id);
        self.subagent_activity
            .entry(id.into())
            .or_default()
            .truncated |= evicted;
        self.enforce_subagent_activity_budget();
    }
    pub(crate) fn upsert_subagent_item(&mut self, id: &str, item: impl Into<TranscriptItem>) {
        let item = item.into();
        if matches!(
            item["type"].as_str(),
            Some("reasoning" | "reasoningMessage")
        ) || self.subagent_period(id).is_none()
        {
            return;
        }
        if self.saved.provider == Provider::Claude {
            let items = &mut self
                .saved
                .claude_subagents
                .entry(id.into())
                .or_default()
                .items;
            if let Some(existing) = items
                .iter_mut()
                .find(|old| !item["id"].is_null() && old["id"] == item["id"])
            {
                *existing = item.clone();
            } else {
                items.push(item.clone());
            }
        }
        self.ensure_subagent_activity(id);
        let Some(activity) = self.subagent_activity.get_mut(id) else {
            return;
        };
        let items = &mut activity.items;
        if let Some(existing) = items
            .iter_mut()
            .find(|old| !item["id"].is_null() && old["id"] == item["id"])
        {
            if *existing == item {
                return;
            }
            *existing = item;
        } else {
            items.push(item);
        }
        activity.observed_at = Some(Instant::now());
    }

    /// An isolated projection lets the native transcript reuse its normal renderer.
    #[must_use]
    pub fn subagent_snapshot(&self, id: &str) -> Self {
        let mut saved = self.saved.clone();
        saved.items = self
            .subagent_activity
            .get(id)
            .map(|a| a.items.clone())
            .unwrap_or_default();
        saved.thread_id = Some(id.into());
        saved.work_periods.clear();
        saved.session_review.pending = None;
        saved.session_review.last_completed_period = None;
        saved.session_review.reviewed_through = 0;
        saved.session_review.summary.clear();
        saved.claude_subagents.clear();
        saved.rich_plans = crate::rich_plan::PlanStore::default();
        saved.draft.clear();
        saved.draft_images.clear();
        saved.draft_reference = None;
        let mut state = Self::new(saved);
        state.status = if self.subagent_terminal(id)
            || self.subagent_transcript_availability(id) == SubagentTranscriptAvailability::Restored
        {
            Status::Idle
        } else {
            Status::Working
        };
        state.work_started = self.work_started;
        state
    }

    pub(crate) fn observe_subagent(
        &mut self,
        id: &str,
        key: &str,
        kind: SubagentActivityKind,
        text: &str,
    ) {
        if id.len() > 1024 || self.subagent_period(id).is_none() || text.trim().is_empty() {
            return;
        }
        self.ensure_subagent_activity(id);
        let Some(activity) = self.subagent_activity.get_mut(id) else {
            return;
        };
        let key = format!("{:016x}", activity_hash(key));
        activity.truncated |= text.len() > 1024;
        let text = bounded_excerpt(text, 1024);
        let now = Instant::now();
        if let Some(event) = activity.events.iter_mut().find(|event| event.id == key) {
            if event.text == text {
                return;
            }
            event.text = text;
            event.observed_at = now;
        } else {
            activity.events.push(SubagentActivityEvent {
                id: key,
                text,
                kind,
                observed_at: now,
            });
        }
        activity.observed_at = Some(now);
        while activity.events.len() > 20
            || activity
                .events
                .iter()
                .map(|e| e.text.len() + e.id.len())
                .sum::<usize>()
                > 16 * 1024
        {
            activity.events.remove(0);
            activity.truncated = true;
        }
        self.enforce_subagent_activity_budget();
    }
    pub(crate) fn subagent_activity_bytes(&self) -> usize {
        self.subagent_activity
            .iter()
            .map(|(id, a)| {
                id.len()
                    + a.events
                        .iter()
                        .map(|e| e.id.len() + e.text.len())
                        .sum::<usize>()
            })
            .sum::<usize>()
            + self
                .subagent_activity_evicted
                .iter()
                .map(String::len)
                .sum::<usize>()
    }
    fn enforce_subagent_activity_budget(&mut self) {
        while self.subagent_activity_bytes() > 1024 * 1024 {
            let oldest = self
                .subagent_activity
                .iter()
                .filter(|(_, a)| !a.events.is_empty())
                .min_by_key(|(id, a)| (!self.subagent_terminal(id), a.events[0].observed_at))
                .map(|(id, _)| id.clone());
            let Some(oldest) = oldest else {
                break;
            };
            let Some(activity) = self.subagent_activity.get_mut(&oldest) else {
                break;
            };
            activity.events.remove(0);
            activity.truncated = true;
        }
    }
    pub(crate) fn upsert_subagent(
        &mut self,
        id: &str,
        name: Option<&str>,
        activity: Option<&str>,
        status: SubagentStatus,
    ) {
        let existing = self.subagent_period(id);
        let target = existing.or_else(|| {
            self.saved
                .work_periods
                .iter()
                .rposition(|p| p.end_item.is_none())
        });
        if let Some(period) = target.and_then(|index| self.saved.work_periods.get_mut(index)) {
            record_subagent(period, id, name, activity, status);
        }
    }
    pub(crate) fn set_subagent_metadata(
        &mut self,
        id: &str,
        role: Option<&str>,
        task: Option<&str>,
    ) {
        if let Some(period) = self.subagent_period(id) {
            if let Some(agent) = self.saved.work_periods[period]
                .subagents
                .iter_mut()
                .find(|agent| agent.id == id)
            {
                update_subagent_metadata(agent, role, task);
            }
        }
    }
    pub(crate) fn subagent_period(&self, id: &str) -> Option<usize> {
        self.saved
            .work_periods
            .iter()
            .rposition(|p| p.subagents.iter().any(|a| a.id == id))
    }
    /// Collab call status describes the call, not the child. Only agentsStates is authoritative.
    pub(crate) fn record_collab(&mut self, item: &Value, period_index: usize) {
        let restart = matches!(
            item["tool"].as_str(),
            Some("resumeAgent" | "followupTask" | "sendInput")
        ) && item["id"]
            .as_str()
            .is_some_and(|id| !self.subagent_restart_calls.iter().any(|old| old == id))
            && item["receiverThreadIds"]
                .as_array()
                .is_some_and(|receivers| {
                    receivers.iter().filter_map(Value::as_str).any(|receiver| {
                        collab_status(item["agentsStates"][receiver]["status"].as_str())
                            == SubagentStatus::Working
                    })
                });
        if restart {
            self.subagent_observation_generation =
                self.subagent_observation_generation.wrapping_add(1);
            if let Some(id) = item["id"].as_str() {
                self.subagent_restart_calls.push(id.into());
            }
        }
        if restart {
            if let Some(receivers) = item["receiverThreadIds"].as_array() {
                for id in receivers
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|id| id.len() <= 1024)
                {
                    self.ensure_subagent_activity(id);
                }
            }
        }
        let Some(period) = self.saved.work_periods.get_mut(period_index) else {
            return;
        };
        let Some(receivers) = item["receiverThreadIds"].as_array() else {
            return;
        };
        for receiver in receivers.iter().filter_map(Value::as_str) {
            if self.saved.thread_id.as_deref() == Some(receiver) {
                continue;
            }
            let state = &item["agentsStates"][receiver];
            let status = collab_status(state["status"].as_str());
            let activity = state["message"].as_str().or_else(|| {
                (!period.subagents.iter().any(|a| a.id == receiver))
                    .then(|| item["prompt"].as_str())
                    .flatten()
            });
            record_subagent(period, receiver, None, activity, status);
            if let Some(agent) = period
                .subagents
                .iter_mut()
                .find(|agent| agent.id == receiver)
            {
                update_subagent_metadata(
                    agent,
                    item["agentRole"].as_str(),
                    item["prompt"].as_str(),
                );
            }
            if restart && status == SubagentStatus::Working {
                let Some(observation) = self.subagent_activity.get_mut(receiver) else {
                    continue;
                };
                observation.refreshing = false;
                observation.poll_cursor = None;
                if let Some(agent) = period.subagents.iter_mut().find(|a| a.id == receiver) {
                    agent.status = status;
                }
            }
            if state["status"].as_str().is_some() && status == SubagentStatus::Unknown {
                if let Some(agent) = period.subagents.iter_mut().find(|a| a.id == receiver) {
                    agent.status = status;
                }
            }
        }
    }
    pub(crate) fn apply_subagent_notification(&mut self, message: &Value, period: usize) -> bool {
        let p = &message["params"];
        let Some(id) = p["threadId"].as_str() else {
            return false;
        };
        let method = message["method"].as_str().unwrap_or_default();
        let mut transcript_changed = false;
        if p["item"]["type"] != "reasoning" && !method.starts_with("item/reasoning") {
            let mut child = self.subagent_snapshot(id);
            child.saved.work_periods.clear();
            child.apply(message);
            self.ensure_subagent_activity(id);
            if let Some(activity) = self.subagent_activity.get_mut(id) {
                if activity.items != child.saved.items {
                    transcript_changed = true;
                    activity.items = child.saved.items;
                    activity.observed_at = Some(Instant::now());
                }
            }
        }
        if matches!(method, "item/started" | "item/completed")
            && p["item"]["type"] == "collabAgentToolCall"
        {
            self.record_collab(&p["item"], period);
        }
        if matches!(method, "item/started" | "item/completed")
            && p["item"]["type"] == "subAgentActivity"
            && p["item"]["kind"] == "started"
        {
            if let Some(child) = p["item"]["agentThreadId"].as_str() {
                record_subagent(
                    &mut self.saved.work_periods[period],
                    child,
                    p["item"]["agentPath"]
                        .as_str()
                        .and_then(|path| path.rsplit('/').next()),
                    None,
                    SubagentStatus::Working,
                );
            }
        }
        let activity = match method {
            "item/started" | "item/completed" => subagent_activity(&p["item"]),
            "item/mcpToolCall/progress" => p["message"].as_str().map(str::to_owned),
            // Text deltas do not constitute a complete activity label.
            _ => None,
        };
        let status = match method {
            "turn/started" => SubagentStatus::Working,
            "turn/completed" => collab_status(p["turn"]["status"].as_str()),
            _ => SubagentStatus::Unknown,
        };
        if activity.is_none() && status == SubagentStatus::Unknown && method != "turn/completed" {
            return transcript_changed;
        }
        if let Some(text) = activity
            .as_deref()
            .filter(|_| p["item"]["type"] != "reasoning")
        {
            let kind = if p["item"]["type"] == "agentMessage" {
                SubagentActivityKind::Commentary
            } else {
                SubagentActivityKind::Tool
            };
            let key = format!(
                "{}:{}",
                p["turnId"].as_str().unwrap_or_default(),
                p["item"]["id"]
                    .as_str()
                    .or_else(|| p["itemId"].as_str())
                    .unwrap_or(method)
            );
            self.observe_subagent(id, &key, kind, text);
        }
        let work = &mut self.saved.work_periods[period];
        record_subagent(work, id, None, activity.as_deref(), status);
        if matches!(method, "turn/started" | "turn/completed") {
            if let Some(agent) = work.subagents.iter_mut().find(|a| a.id == id) {
                agent.status = status;
            }
        }
        true
    }
    /// Read-only polling observes child activity without resuming or taking control of its thread.
    pub(crate) fn apply_subagent_read(&mut self, id: &str, period_start: usize, thread: &Value) {
        let Some(period) =
            self.saved.work_periods.iter().rposition(|p| {
                p.start_item == period_start && p.subagents.iter().any(|a| a.id == id)
            })
        else {
            return;
        };
        if thread["id"].as_str() != Some(id) || self.subagent_period(id) != Some(period) {
            return;
        }
        let name = thread["agentNickname"]
            .as_str()
            .or_else(|| thread["agentRole"].as_str());
        record_subagent(
            &mut self.saved.work_periods[period],
            id,
            name,
            None,
            SubagentStatus::Unknown,
        );
        self.set_subagent_metadata(id, thread["agentRole"].as_str(), None);
        if let Some(turns) = thread["turns"].as_array() {
            let mut chronological = Vec::new();
            for turn in turns {
                let inherited = turn["id"].as_str().is_some_and(|turn| {
                    self.turn_id.as_deref() == Some(turn)
                        || self.settled_turns.iter().any(|old| old == turn)
                        || self
                            .saved
                            .edit_checkpoints
                            .iter()
                            .any(|checkpoint| checkpoint.turn_id.as_deref() == Some(turn))
                });
                if inherited {
                    continue;
                }
                if let Some(items) = turn["items"].as_array() {
                    for item in items {
                        if item["type"] != "reasoning"
                            && !self
                                .saved
                                .items
                                .iter()
                                .any(|parent| !item["id"].is_null() && parent["id"] == item["id"])
                        {
                            chronological.push(item.clone());
                        }
                    }
                }
            }
            if !chronological.is_empty() {
                self.ensure_subagent_activity(id);
                if let Some(activity) = self.subagent_activity.get_mut(id) {
                    // A read can introduce older history after live notifications.
                    // Keep the read's provider order and append only unseen live items.
                    for item in std::mem::take(&mut activity.items) {
                        if !chronological.iter().any(|old| old["id"] == item["id"]) {
                            chronological.push(item);
                        }
                    }
                    activity.items = chronological.into();
                }
            }
        }
        let Some(turn) = thread["turns"].as_array().and_then(|turns| turns.last()) else {
            return;
        };
        // Forked child history can initially end at the parent's turn. Never report
        // those inherited tools or the parent's completion as this child's work.
        if turn["id"].as_str().is_some_and(|turn| {
            self.turn_id.as_deref() == Some(turn)
                || self.settled_turns.iter().any(|old| old == turn)
                || self
                    .saved
                    .edit_checkpoints
                    .iter()
                    .any(|checkpoint| checkpoint.turn_id.as_deref() == Some(turn))
        }) {
            return;
        }
        if let Some(items) = turn["items"].as_array() {
            for item in items {
                // Fork history may include parent items. Sender identity keeps those out of the roster.
                if item["type"] == "collabAgentToolCall"
                    && item["senderThreadId"].as_str() == Some(id)
                {
                    self.record_collab(item, period);
                }
            }
            let turn_hash = activity_hash(turn["id"].as_str().unwrap_or_default());
            let cursor = self.subagent_activity.get(id).and_then(|a| a.poll_cursor);
            let item_key = |index: usize, item: &Value| {
                item["id"]
                    .as_str()
                    .map_or_else(|| index.to_string(), str::to_owned)
            };
            let start = cursor
                .filter(|(old_turn, _)| *old_turn == turn_hash)
                .and_then(|(_, old_item)| {
                    items
                        .iter()
                        .enumerate()
                        .position(|(i, item)| activity_hash(&item_key(i, item)) == old_item)
                })
                .unwrap_or_else(|| items.len().saturating_sub(20));
            // Retained tools may keep changing after later items appear. Revisit
            // bounded recent history without resurrecting evicted observations.
            let revisit_start = start.min(items.len().saturating_sub(20));
            for (index, item) in items.iter().enumerate().skip(revisit_start) {
                if item["type"] == "reasoning" {
                    continue;
                }
                if let Some(text) = subagent_activity(item) {
                    let key = format!(
                        "{}:{}",
                        turn["id"].as_str().unwrap_or_default(),
                        item_key(index, item)
                    );
                    if index < start {
                        let event_id = format!("{:016x}", activity_hash(&key));
                        if !self.subagent_activity.get(id).is_some_and(|activity| {
                            activity.events.iter().any(|event| event.id == event_id)
                        }) {
                            continue;
                        }
                    }
                    let kind = if item["type"] == "agentMessage" {
                        SubagentActivityKind::Commentary
                    } else {
                        SubagentActivityKind::Tool
                    };
                    self.observe_subagent(id, &key, kind, &text);
                }
            }
            if let Some((index, item)) = items.iter().enumerate().next_back() {
                if let Some(activity) = self.subagent_activity.get_mut(id) {
                    activity.poll_cursor = Some((turn_hash, activity_hash(&item_key(index, item))));
                }
            }
            if let Some(activity) = items.iter().rev().find_map(subagent_activity) {
                record_subagent(
                    &mut self.saved.work_periods[period],
                    id,
                    None,
                    Some(&activity),
                    SubagentStatus::Unknown,
                );
            }
        }
        let status = collab_status(turn["status"].as_str());
        if turn["status"].as_str().is_some() {
            if let Some(agent) = self.saved.work_periods[period]
                .subagents
                .iter_mut()
                .find(|a| a.id == id)
            {
                agent.status = status;
            }
        }
    }
    pub(crate) fn finish_work(&mut self) {
        let Some(started) = self.work_started.take() else {
            return;
        };
        if let Some(period) = self
            .saved
            .work_periods
            .last_mut()
            .filter(|p| p.end_item.is_none())
        {
            period.end_item = Some(self.saved.items.len());
            period.duration_ms = Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64);
        }
    }
    pub(crate) fn resolve_async_question(&mut self, id: &Value) {
        if let Some(item) = self.saved.items.iter_mut().find(|item| item["id"] == *id) {
            item["groveQuestionResolved"] = json!(true);
        }
        self.questions.retain(|question| question.id != *id);
    }
    pub(crate) fn restore_async_questions(&mut self) {
        let start = self
            .saved
            .items
            .iter()
            .rposition(|item| item["type"] == "userMessage")
            .map_or(0, |index| index + 1);
        for (index, item) in self.saved.items.iter_mut().enumerate() {
            if index < start && item["groveQuestionPending"] != true {
                continue;
            }
            if let Some(question) = QuestionRequest::from_async_message(item) {
                item["groveQuestionPending"] = json!(true);
                if !self.questions.iter().any(|old| old.id == question.id) {
                    self.questions.push(question);
                }
            }
        }
        if !self.questions.is_empty() {
            self.status = Status::Waiting;
        }
    }
    pub fn transcript(&self) -> String {
        self.saved
            .items
            .iter()
            .map(|item| item_text(item))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
    pub fn apply(&mut self, message: &Value) -> bool {
        let p = &message["params"];
        if message["method"] == "mcpServer/startupStatus/updated" {
            if p["threadId"]
                .as_str()
                .is_some_and(|id| Some(id) != self.saved.thread_id.as_deref())
            {
                return false;
            }
            if let (Some(name), Some(status)) = (p["name"].as_str(), p["status"].as_str()) {
                self.mcp_server_status(
                    name,
                    status,
                    p["error"].as_str(),
                    p["failureReason"] == "reauthenticationRequired",
                );
                return true;
            }
            return false;
        }
        if let Some(period) = p["threadId"]
            .as_str()
            .and_then(|id| self.subagent_period(id))
        {
            return self.apply_subagent_notification(message, period);
        }
        // Account/configuration notifications may have no thread association.
        if p["threadId"].is_null() {
            match message["method"].as_str() {
                Some("warning" | "configWarning") => {
                    let text = p["message"]
                        .as_str()
                        .or(p["details"].as_str())
                        .unwrap_or("Codex reported a warning");
                    self.provider_warning(p["summary"].as_str().unwrap_or("Codex warning"), text);
                    return true;
                }
                // Occupancy is not evidence that a request was rejected.
                Some("account/rateLimits/updated") => {
                    let limits = &p["rateLimits"];
                    self.rate_limit_reset = ["primary", "secondary"]
                        .iter()
                        .filter_map(|key| {
                            let window = &limits[*key];
                            (window["usedPercent"].as_f64().unwrap_or(0.0) >= 100.0)
                                .then(|| window["resetsAt"].as_i64())
                                .flatten()
                        })
                        .max();
                    if self.effective_notice().is_some_and(|notice| {
                        notice.provider == "Codex"
                            && notice.kind == SessionNoticeKind::UsageLimit
                            && notice.reset_at != self.rate_limit_reset
                    }) {
                        if let Some(notice) = self.notice.as_mut() {
                            notice.reset_at = self.rate_limit_reset;
                            return true;
                        }
                    }
                    return false;
                }
                _ => {}
            }
        }
        if p["threadId"].as_str() != self.saved.thread_id.as_deref()
            || self.saved.thread_id.is_none()
        {
            return false;
        }
        if let Some(turn) = p["turnId"].as_str() {
            if self.turn_id.as_deref().is_some_and(|active| active != turn) {
                let late_command = self.settled_turns.iter().any(|old| old == turn)
                    && matches!(
                        message["method"].as_str(),
                        Some("item/commandExecution/outputDelta" | "item/completed")
                    )
                    && self.saved.items.iter().any(|item| {
                        item["type"] == "commandExecution"
                            && item["id"]
                                == if message["method"] == "item/completed" {
                                    p["item"]["id"].clone()
                                } else {
                                    p["itemId"].clone()
                                }
                    });
                if !late_command {
                    return false;
                }
            }
        }
        if message["method"] == "turn/completed"
            && self
                .turn_id
                .as_deref()
                .is_some_and(|active| p["turn"]["id"].as_str() != Some(active))
        {
            return false;
        }
        match message["method"].as_str().unwrap_or_default() {
            "warning" | "configWarning" => {
                let text = p["message"]
                    .as_str()
                    .or(p["details"].as_str())
                    .unwrap_or("Codex reported a warning");
                self.provider_warning(p["summary"].as_str().unwrap_or("Codex warning"), text);
            }
            "thread/name/updated" => {
                self.set_provider_title(p["threadName"].as_str());
            }
            "turn/started" => {
                if self
                    .turn_id
                    .as_deref()
                    .is_some_and(|active| p["turn"]["id"].as_str() != Some(active))
                {
                    return false;
                }
                if p["turn"]["id"]
                    .as_str()
                    .is_some_and(|id| self.settled_turns.iter().any(|old| old == id))
                {
                    return false;
                }
                self.begin_work();
                self.turn_id = p["turn"]["id"].as_str().map(str::to_owned);
                self.status = if self.approvals.is_empty()
                    && self.questions.is_empty()
                    && self.elicitations.is_empty()
                {
                    Status::Working
                } else {
                    Status::Waiting
                };
                self.clear_error();
            }
            "turn/completed" => {
                // Completion carries its identity inside `turn`, unlike item
                // events' top-level turnId. A late result must never finish the
                // newer period, or recreate Review after rewind/reconnect.
                if self.turn_id.is_none() || p["turn"]["id"].as_str() != self.turn_id.as_deref() {
                    return false;
                }
                self.finish_work();
                if let Some(turn) = p["turn"]["id"].as_str() {
                    self.settled_turns.push(turn.to_owned());
                    if self.settled_turns.len() > 32 {
                        self.settled_turns.remove(0);
                    }
                }
                self.turn_id = None;
                self.compacting = false;
                self.compact_requested = false;
                let parent = self.saved.thread_id.as_deref();
                let owners = &self.interaction_owners;
                let child = |id: &Value| {
                    owners
                        .get(&id.to_string())
                        .is_some_and(|owner| Some(owner.thread.as_str()) != parent)
                };
                self.approvals.retain(|a| child(&a["id"]));
                self.questions
                    .retain(|q| q.source == QuestionSource::AsyncMessage || child(&q.id));
                self.elicitations
                    .retain(|r| r.turn_id.is_none() || child(&r.id));
                self.interaction_owners
                    .retain(|_, owner| Some(owner.thread.as_str()) != parent);
                self.status = match p["turn"]["status"].as_str() {
                    Some("failed") => Status::Failed,
                    Some("interrupted") => Status::Interrupted,
                    _ => Status::Done,
                };
                if p["turn"]["status"] == "completed" {
                    self.completed_for_review();
                }
                if !self.elicitations.is_empty()
                    || !self.questions.is_empty()
                    || !self.approvals.is_empty()
                {
                    self.status = Status::Waiting;
                }
                self.error = p["turn"]["error"]["message"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| {
                        (p["turn"]["status"] == "failed")
                            .then(|| "Codex could not complete this request.".to_owned())
                    });
                if let Some(body) = self.error.clone() {
                    self.set_provider_error(
                        "Codex",
                        body,
                        false,
                        p["turn"]["error"]["codexErrorInfo"].as_str(),
                        None,
                    );
                }
            }
            "serverRequest/resolved" => {
                self.approvals
                    .retain(|approval| approval["id"] != p["requestId"]);
                self.questions
                    .retain(|request| request.id != p["requestId"]);
                self.elicitations
                    .retain(|request| request.id != p["requestId"]);
                if self.status == Status::Waiting
                    && self.approvals.is_empty()
                    && self.questions.is_empty()
                    && self.elicitations.is_empty()
                {
                    self.status = Status::Working;
                }
            }
            "error" => {
                self.set_provider_error(
                    "Codex",
                    p["error"]["message"]
                        .as_str()
                        .unwrap_or("Codex reported an error")
                        .into(),
                    p["willRetry"] == true,
                    p["error"]["codexErrorInfo"]
                        .as_str()
                        .or(p["error"]["code"].as_str()),
                    None,
                );
                if p["willRetry"] != true {
                    self.status = Status::Failed;
                }
            }
            "item/started" | "item/completed" if p["item"]["type"] == COMPACTION_ITEM => {
                let running = message["method"] == "item/started";
                let manual = running && std::mem::take(&mut self.compact_requested);
                self.compacting = running;
                if running {
                    if let Some(details) = self.saved.context_details.as_mut() {
                        details.accounting = None;
                    }
                }
                self.upsert_compaction(
                    &p["item"]["id"],
                    running,
                    Some(if manual { "manual" } else { "auto" }),
                    None,
                );
            }
            "thread/tokenUsage/updated" => {
                let last = &p["tokenUsage"]["last"];
                let used = last["totalTokens"].as_u64().or_else(|| {
                    Some(last["inputTokens"].as_u64()? + last["outputTokens"].as_u64().unwrap_or(0))
                });
                if used.is_none() {
                    return false;
                }
                self.record_context(used, p["tokenUsage"]["modelContextWindow"].as_u64());
                if let Some(details) = self.saved.context_details.as_mut() {
                    details.accounting = Some(ContextTokenAccounting {
                        last: ContextTokenCounters::from_provider_value(last),
                        total: ContextTokenCounters::from_provider_value(&p["tokenUsage"]["total"]),
                    });
                }
            }
            "item/started" | "item/completed" => {
                if p["item"]["type"] == "agentMessage" {
                    self.clear_retry_notice();
                }
                let mut item = p["item"].clone();
                if item["type"] == "subAgentActivity" && item["kind"] == "started" {
                    if let Some(id) = item["agentThreadId"]
                        .as_str()
                        .filter(|id| self.saved.thread_id.as_deref() != Some(*id))
                    {
                        self.upsert_subagent(
                            id,
                            item["agentPath"]
                                .as_str()
                                .and_then(|path| path.rsplit('/').next()),
                            None,
                            SubagentStatus::Working,
                        );
                    }
                }
                if item["type"] == "collabAgentToolCall" {
                    if let Some(period) = self
                        .saved
                        .work_periods
                        .iter()
                        .rposition(|p| p.end_item.is_none())
                    {
                        self.record_collab(&item, period);
                    }
                }
                stamp_message_received(
                    &mut item,
                    self.saved
                        .items
                        .iter()
                        .find(|old| old["id"] == p["item"]["id"])
                        .map(AsRef::as_ref),
                );
                if let Some(old) = self.saved.items.iter().find(|old| old["id"] == item["id"]) {
                    for marker in [
                        "groveQuestionResolved",
                        "groveQuestionPending",
                        "groveTaskOwnershipUnavailable",
                    ] {
                        if old[marker] == true {
                            item[marker] = json!(true);
                        }
                    }
                }
                if message["method"] == "item/completed" {
                    if item["type"] == "agentMessage" {
                        item["groveCompleted"] = json!(true);
                    }
                    if let Some(question) = QuestionRequest::from_async_message(&item) {
                        item["groveQuestionPending"] = json!(true);
                        if !self.questions.iter().any(|q| q.id == question.id) {
                            self.questions.push(question);
                            self.status = Status::Waiting;
                        }
                    }
                }
                if self.saved.title.is_none() && item["type"] == "userMessage" {
                    self.saved.title = prompt_title(&item_text(&item));
                }
                if let Some(at) = self
                    .saved
                    .items
                    .iter()
                    .position(|old| old["id"] == item["id"])
                {
                    self.saved.items[at] = item.into();
                } else {
                    self.saved.items.push(item);
                }
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                let at = self.item_index(&p["itemId"], "reasoning");
                let summary = message["method"] == "item/reasoning/summaryTextDelta";
                let key = if summary { "summary" } else { "content" };
                let index = p[if summary {
                    "summaryIndex"
                } else {
                    "contentIndex"
                }]
                .as_u64()
                .unwrap_or(0) as usize;
                // Bound malformed indexes before allocation.
                if index > 1024 {
                    return false;
                }
                let item = &mut self.saved.items[at];
                if !item[key].is_array() {
                    item[key] = json!([]);
                }
                if let Some(parts) = item[key].as_array_mut() {
                    while parts.len() <= index {
                        parts.push(json!(""));
                    }
                    parts[index] = json!(format!(
                        "{}{}",
                        parts[index].as_str().unwrap_or_default(),
                        p["delta"].as_str().unwrap_or_default()
                    ));
                }
            }
            "item/mcpToolCall/progress" => {
                let at = self.item_index(&p["itemId"], "mcpToolCall");
                self.saved.items[at]["progress"] = p["message"].clone();
            }
            "item/fileChange/patchUpdated" => {
                let at = self.item_index(&p["itemId"], "fileChange");
                self.saved.items[at]["changes"] = p["changes"].clone();
            }
            "turn/plan/updated" => {
                let id = json!(format!(
                    "grove-plan-{}",
                    p["turnId"].as_str().unwrap_or_default()
                ));
                // Every update is an immutable chronological snapshot. Replacing the
                // turn's first plan retroactively moves progress prose to the wrong task.
                let base_id = id.as_str().unwrap_or_default().to_owned();
                let mut id = id;
                let mut sequence = self.saved.items.len();
                while self.saved.items.iter().any(|item| item["id"] == id) {
                    id = json!(format!("{base_id}-{sequence}"));
                    sequence += 1;
                }
                let at = self.item_index(&id, "plan");
                self.saved.items[at]["taskSnapshot"] = json!(true);
                let explanation = p["explanation"].as_str().unwrap_or_default();
                let mut text = explanation.to_owned();
                // The structured checklist read by `crate::tasks`; `text` stays
                // for transcripts and sessions saved before this field existed.
                let plan: Vec<Value> = p["plan"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(i, step)| {
                        json!({
                            "id": format!("step-{i}"),
                            "step": step["step"].as_str().unwrap_or_default(),
                            "status": crate::tasks::normalize_status(step["status"].as_str()),
                        })
                    })
                    .collect();
                self.saved.items[at]["plan"] = json!(plan);
                self.saved.items[at]["explanation"] = if explanation.trim().is_empty() {
                    Value::Null
                } else {
                    json!(explanation)
                };
                if let Some(steps) = p["plan"].as_array() {
                    for step in steps {
                        let _ = write!(
                            text,
                            "\n- [{}] {}",
                            if step["status"] == "completed" {
                                "x"
                            } else {
                                " "
                            },
                            step["step"].as_str().unwrap_or_default()
                        );
                    }
                }
                self.saved.items[at]["text"] = json!(text);
            }
            "item/agentMessage/delta" | "item/commandExecution/outputDelta" | "item/plan/delta" => {
                if message["method"] == "item/agentMessage/delta" {
                    self.clear_retry_notice();
                }
                let method = message["method"].as_str().unwrap_or_default();
                let output = method.contains("outputDelta");
                let at=self.saved.items.iter().position(|old|old["id"]==p["itemId"]).unwrap_or_else(|| {let mut item = json!({"id":p["itemId"],"type":if output{"commandExecution"}else if method.contains("plan"){"plan"}else{"agentMessage"}});stamp_message_received(&mut item, None);self.saved.items.push(item);self.saved.items.len()-1});
                let key = if output { "aggregatedOutput" } else { "text" };
                let item = &mut self.saved.items[at];
                let text = format!(
                    "{}{}",
                    item[key].as_str().unwrap_or_default(),
                    p["delta"].as_str().unwrap_or_default()
                );
                item[key] = json!(text);
            }
            _ => return false,
        }
        true
    }
    fn item_index(&mut self, id: &Value, kind: &str) -> usize {
        if let Some(at) = self.saved.items.iter().position(|item| item["id"] == *id) {
            return at;
        }
        let mut item = json!({"id":id,"type":kind,"status":"inProgress"});
        stamp_message_received(&mut item, None);
        self.saved.items.push(item);
        self.saved.items.len() - 1
    }
    /// Record the provider's latest context occupancy. A known `used` also completes the
    /// newest compaction divider that is still waiting for its post-compaction figure.
    pub(crate) fn record_context(&mut self, used: Option<u64>, window: Option<u64>) {
        if used.is_some() {
            self.context_measurement_live = true;
        }
        if used.is_some()
            || window.is_some_and(|window| {
                self.saved
                    .context
                    .is_some_and(|usage| usage.window != window)
            })
        {
            self.context_revision = self.context_revision.wrapping_add(1);
            self.saved.context_details = Some(ContextDetails {
                accounting: None,
                model: self.resolved_model.clone(),
                measured_at: context_receipt_time(),
                source: ContextMeasurement::ProviderUsage,
                categories: Vec::new(),
                auto_compact_enabled: None,
                auto_compact_threshold: None,
            });
        }
        if let Some(used) = used {
            self.context_tokens = Some(used);
            if let Some(item) = self
                .saved
                .items
                .iter_mut()
                .rev()
                .find(|item| item["type"] == COMPACTION_ITEM)
            {
                if item.get("postTokens") == Some(&Value::Null) {
                    item["postTokens"] = json!(used);
                }
            }
        }
        let window = window
            .filter(|window| *window > 0)
            .or(self.saved.context.map(|usage| usage.window));
        if let (Some(used), Some(window)) = (self.context_tokens, window) {
            self.saved.context = Some(ContextUsage { used, window });
        }
    }
    /// Upsert a normalized compaction divider. Grove-owned metadata (trigger and token
    /// figures) survives provider replacements of the same item.
    pub(crate) fn upsert_compaction(
        &mut self,
        id: &Value,
        running: bool,
        trigger: Option<&str>,
        before: Option<u64>,
    ) {
        let at = self.saved.items.iter().position(|old| old["id"] == *id);
        let old = at.map(|at| self.saved.items[at].clone());
        let (trigger, before, after) = match &old {
            Some(old) => (
                old["trigger"].as_str().map(str::to_owned),
                old["preTokens"].as_u64(),
                old["postTokens"].clone(),
            ),
            None => (
                trigger.map(str::to_owned),
                before.or(self.context_tokens),
                Value::Null,
            ),
        };
        let item = json!({
            "id": id,
            "type": COMPACTION_ITEM,
            "status": if running { "inProgress" } else { "completed" },
            "trigger": trigger,
            "preTokens": before,
            "postTokens": after,
        });
        match at {
            Some(at) => self.saved.items[at] = item.into(),
            None => self.saved.items.push(item),
        }
    }
    pub(crate) fn set_provider_title(&mut self, name: Option<&str>) {
        if self.saved.fork_origin.is_some() {
            return;
        }
        if self
            .saved
            .title_generation
            .as_ref()
            .is_some_and(|generation| generation.generated)
        {
            return;
        }
        self.saved.title = name
            .filter(|name| !name.trim().is_empty())
            .map(|name| name.trim().to_owned())
            .or_else(|| first_prompt_title(&self.saved.items));
    }
    pub(crate) fn fail(&mut self, error: String) {
        self.finish_work();
        self.error = Some(error);
        self.status = Status::Disconnected;
        self.reconnecting = false;
        self.turn_id = None;
        self.compacting = false;
        self.compact_requested = false;
        self.context_query_pending = false;
        self.context_measurement_live = false;
        self.context_revision = self.context_revision.wrapping_add(1);
        self.approvals.clear();
        self.questions.clear();
        self.elicitations.clear();
    }
}
pub(crate) fn prompt_title(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(80).collect())
}
/// Whether a transcript holds any conversation that compaction could shorten.
pub fn has_conversation<T: Borrow<Value>>(items: &[T]) -> bool {
    items.iter().any(|item| {
        item.borrow()["type"] == "userMessage" || item.borrow()["type"] == "agentMessage"
    })
}
fn first_prompt_title<T: Borrow<Value>>(items: &[T]) -> Option<String> {
    items
        .iter()
        .map(Borrow::<Value>::borrow)
        .filter(|item| item["type"] == "userMessage")
        .find_map(|item| prompt_title(&item_text(item)))
}
/// Provider history omits Grove's checklist notifications. Reinsert retained
/// snapshots between surviving message IDs so resume cannot erase attribution.
pub(crate) fn restore_task_snapshots<T: std::borrow::Borrow<Value>>(
    saved: &[T],
    items: Vec<Value>,
) -> Vec<Value> {
    // Newly discovered history has no observed checklist boundary: task
    // notifications may have happened while Grove was disconnected. Persist
    // that provenance so a later resume cannot turn it into a known owner.
    let items: Vec<_> = items
        .into_iter()
        .map(|mut item| {
            if item["type"] == "agentMessage" {
                let known = saved
                    .iter()
                    .map(|saved| -> &Value { saved.borrow() })
                    .find(|old| old["id"] == item["id"]);
                if known.is_none_or(|old| old["groveTaskOwnershipUnavailable"] == true) {
                    item["groveTaskOwnershipUnavailable"] = json!(true);
                }
            }
            item
        })
        .collect();
    let positions: std::collections::HashMap<_, _> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| Some((item["id"].as_str()?.to_owned(), index)))
        .collect();
    let mut slots: Vec<Vec<Value>> = vec![Vec::new(); items.len() + 1];
    let mut last_slot = 0;
    for (index, item) in saved.iter().enumerate() {
        let item = item.borrow();
        if !crate::tasks::is_checklist(item)
            || !item["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("grove-plan-"))
            || positions.contains_key(item["id"].as_str().unwrap_or_default())
        {
            continue;
        }
        let next = saved[index + 1..]
            .iter()
            .find_map(|item| positions.get(item.borrow()["id"].as_str()?).copied());
        let previous = saved[..index].iter().rev().find_map(|item| {
            positions
                .get(item.borrow()["id"].as_str()?)
                .map(|index| index + 1)
        });
        let mut snapshot = item.clone();
        let slot = next.or(previous).unwrap_or(items.len());
        // A rewritten or unanchored history cannot establish old ownership.
        if slot < last_slot || (next.is_none() && previous.is_none()) {
            snapshot["taskSnapshot"] = json!(false);
        }
        let slot = slot.max(last_slot);
        slots[slot].push(snapshot);
        last_slot = slot;
    }
    let mut merged = Vec::with_capacity(items.len() + saved.len());
    for (index, item) in items.into_iter().enumerate() {
        merged.append(&mut slots[index]);
        merged.push(item);
    }
    if let Some(last) = slots.last_mut() {
        merged.append(last);
    }
    merged
}

/// Local receipt time, recorded only at the live provider boundary, never on history load.
pub(crate) fn stamp_message_received(item: &mut Value, previous: Option<&Value>) {
    if !matches!(item["type"].as_str(), Some("userMessage" | "agentMessage")) {
        return;
    }
    if let Some(previous) = previous {
        if let Some(timestamp) = previous.get("groveReceivedAt") {
            item["groveReceivedAt"] = timestamp.clone();
        }
        return;
    }
    if item.get("groveReceivedAt").is_none() {
        item["groveReceivedAt"] = json!(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64);
    }
}

pub fn item_text(item: &Value) -> String {
    if crate::activity::is_internal(item) {
        return String::new();
    }
    match item["type"].as_str().unwrap_or_default() {
        "userMessage" => item["content"]
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p["text"].as_str())
                    .filter(|text| !crate::review_brief::is_context(text))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
        "agentMessage" | "plan" | "providerNotice" => {
            item["text"].as_str().unwrap_or_default().into()
        }
        COMPACTION_ITEM => compaction_label(item),
        "reasoning" => ["summary", "content"]
            .iter()
            .filter_map(|key| item[*key].as_array())
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        "mcpToolCall" => {
            let mut text = format!(
                "{} / {}",
                item["server"].as_str().unwrap_or("Tool"),
                item["tool"].as_str().unwrap_or_default()
            );
            if let Some(progress) = item["progress"].as_str() {
                text.push('\n');
                text.push_str(progress);
            }
            if let Some(content) = item["result"]["content"].as_array() {
                for part in content {
                    if let Some(value) = part["text"].as_str() {
                        text.push('\n');
                        text.push_str(value);
                    }
                }
            }
            if let Some(error) = item["error"]["message"].as_str() {
                text.push('\n');
                text.push_str(error);
            }
            text
        }
        "webSearch" => format!(
            "Search: {}",
            item["action"]["query"]
                .as_str()
                .or(item["query"].as_str())
                .unwrap_or("Searching the web")
        ),
        "commandExecution" => format!(
            "{}\n{}",
            item["command"].as_str().unwrap_or_default(),
            item["aggregatedOutput"].as_str().unwrap_or_default()
        ),
        "fileChange" => item["changes"]
            .as_array()
            .map(|changes| {
                changes
                    .iter()
                    .map(|change| {
                        format!(
                            "{}\n{}",
                            change["path"].as_str().unwrap_or_default(),
                            change["diff"].as_str().unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .unwrap_or_default(),
        _ => item["text"].as_str().map_or_else(
            || {
                format!(
                    "{}: {}",
                    item["type"].as_str().unwrap_or("Tool"),
                    item["status"].as_str().unwrap_or("output unavailable")
                )
            },
            str::to_owned,
        ),
    }
}

/// Validate before accepting a draft or invoking a provider. Only embedded image data is
/// accepted: URLs must never trigger incidental network/file reads from a paste.
pub fn validate_images(images: &[String]) -> Result<(), String> {
    if images.len() > 10 || images.iter().map(String::len).sum::<usize>() > 28 * 1024 * 1024 {
        return Err("Attach at most 10 images, up to 20 MiB in total.".into());
    }
    let mut total = 0;
    for image in images {
        let data = [
            "data:image/png;base64,",
            "data:image/jpeg;base64,",
            "data:image/webp;base64,",
            "data:image/gif;base64,",
        ]
        .iter()
        .find_map(|prefix| image.strip_prefix(prefix))
        .ok_or("Paste a PNG, JPEG, WebP or GIF image")?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| "The pasted image data is invalid.")?;
        total += decoded.len();
        if decoded.is_empty() || total > 20 * 1024 * 1024 {
            return Err("Attach at most 10 images, up to 20 MiB in total.".into());
        }
    }
    Ok(())
}
pub fn validate_prompt(text: &str, images: &[String]) -> Result<(), String> {
    validate_images(images)?;
    if (text.trim().is_empty() && images.is_empty()) || text.len() > 512 * 1024 {
        return Err("Enter a message or paste an image (text limit: 512 KiB).".into());
    }
    Ok(())
}

pub(crate) fn prompt_input(text: &str, images: Vec<String>) -> Vec<Value> {
    let mut input = Vec::new();
    if !text.trim().is_empty() {
        input.push(json!({"type":"text","text":text,"text_elements":[]}));
    }
    input.extend(
        images
            .into_iter()
            .map(|url| json!({"type":"image","url":url})),
    );
    input
}
pub enum Action {
    /// Metadata-only refresh; never reconnects or alters the conversation.
    UpdateSkills(Vec<Value>),
    /// Replace advertised model metadata without reconnecting or configuring the provider.
    UpdateModels(Vec<Value>),
    /// Explicitly chosen title; automatic generation must preserve it.
    SetTitle(String),
    /// Regenerate from bounded conversation context and resume automatic naming.
    RegenerateTitle,
    /// A receipt for the exact completed result displayed by the native UI.
    MarkReviewed {
        work_period: usize,
    },
    /// UI responses are bound to the connection that rendered the interaction.
    Interaction {
        epoch: u64,
        action: Box<Action>,
    },
    UpdatePlanReview {
        plan_id: String,
        revision_id: String,
        review: Box<crate::rich_plan::ReviewState>,
    },
    SendPlanResponse {
        plan_id: String,
        revision_id: String,
    },
    ApprovePlan {
        plan_id: String,
        revision_id: String,
    },
    RetryPlanImport {
        message_id: String,
        source: String,
    },
    /// Start a fresh provider conversation while retaining the Grove session.
    ClearConversation,
    EditMessage {
        index: usize,
        message_id: String,
    },
    RewindMessage {
        index: usize,
        message_id: String,
        restore_files: bool,
    },
    RewindMessageReserved {
        index: usize,
        message_id: String,
        reservation: crate::checkpoint::RestoreReservation,
    },
    RestoreRewindDraft {
        id: String,
    },
    EditMessageInline {
        index: usize,
        message_id: String,
    },
    PromptInline {
        text: String,
        images: Vec<String>,
    },
    ResetConfiguration,
    /// Restore configured model and effort without resetting effective access.
    ResetModelEffort,
    SetSpeed(Speed),
    Configure {
        model: Option<String>,
        effort: Option<String>,
        access: Option<String>,
    },
    /// An explicit project action enters the native queue without consuming the composer.
    ProjectPrompt {
        id: u64,
        text: String,
    },
    /// Activity reply owns a separate draft; steer is bound to the observed turn.
    ActivityPrompt {
        id: u64,
        text: String,
        mode: SubmissionMode,
        expected_turn: Option<String>,
    },
    SubmitWithMode {
        id: u64,
        text: String,
        images: Vec<String>,
        mode: SubmissionMode,
    },
    RemoveQueued(u64),
    BeginQueuedEdit(u64),
    SaveQueuedEdit {
        id: u64,
        text: String,
        images: Vec<String>,
    },
    CancelQueuedEdit(u64),
    SteerQueued(u64),
    DismissUncertainQueued(u64),
    ResumeQueue,
    Submit {
        id: u64,
        text: String,
        images: Vec<String>,
    },
    Prompt(String),
    PromptWithImages {
        text: String,
        images: Vec<String>,
    },
    DraftImages(Vec<String>),
    DraftReference(Option<ReferenceDraft>),
    /// Compact the conversation context (`/compact`); rejected while a turn is active.
    Compact,
    /// Request supported provider context attribution without submitting a prompt.
    RefreshContext,
    Interrupt,
    Approval(Value, bool),
    RespondApproval {
        id: Value,
        choice: String,
        input: Option<Value>,
        feedback: Option<String>,
    },
    RespondElicitation {
        id: Value,
        action: String,
        content: Option<Value>,
    },
    AnswerQuestions(Value, Option<QuestionAnswers>),
    Reconnect,
    Draft(String),
    SideChat {
        checkout: String,
        chat: SavedSideChat,
    },
    SideChatDraft {
        checkout: String,
        draft: String,
    },
    Metadata {
        project: String,
        roots: Vec<crate::session_meta::ContextRoot>,
    },
    Stop {
        forget: bool,
        done: mpsc::Sender<()>,
        archive_outcome: Option<mpsc::Sender<Result<(), String>>>,
    },
}
#[derive(Clone)]
pub struct Handle(
    pub(crate) mpsc::Sender<Action>,
    pub(crate) Arc<AtomicU32>,
    pub(crate) Arc<AtomicBool>,
    pub(crate) Arc<AtomicU64>,
    Option<Arc<dyn Fn() + Send + Sync>>,
);
impl Handle {
    /// Create the actor command channel independently of process startup.
    /// Also permits embedding hosts to exercise the UI with a deterministic protocol peer.
    pub fn channel() -> (Self, mpsc::Receiver<Action>) {
        let (send, receive) = mpsc::channel();
        (
            Self(
                send,
                Arc::new(AtomicU32::new(0)),
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicU64::new(1)),
                None,
            ),
            receive,
        )
    }
    /// Wake an embedding host after each command is queued. The callback must be
    /// nonblocking; hosts may coalesce notifications and drain the command queue.
    pub fn channel_with_wake(wake: Arc<dyn Fn() + Send + Sync>) -> (Self, mpsc::Receiver<Action>) {
        let (mut handle, receiver) = Self::channel();
        handle.4 = Some(wake);
        (handle, receiver)
    }
    pub fn submit(&self, text: String, images: Vec<String>) -> Result<u64, String> {
        let id = self.3.fetch_add(1, Ordering::Relaxed);
        self.send(Action::Submit { id, text, images })?;
        Ok(id)
    }
    pub fn project_prompt(&self, text: String) -> Result<u64, String> {
        let id = self.3.fetch_add(1, Ordering::Relaxed);
        self.send(Action::ProjectPrompt { id, text })?;
        Ok(id)
    }
    pub fn activity_prompt(
        &self,
        text: String,
        mode: SubmissionMode,
        expected_turn: Option<String>,
    ) -> Result<u64, String> {
        let id = self.3.fetch_add(1, Ordering::Relaxed);
        self.send(Action::ActivityPrompt {
            id,
            text,
            mode,
            expected_turn,
        })?;
        Ok(id)
    }
    pub fn submit_with_mode(
        &self,
        text: String,
        images: Vec<String>,
        mode: SubmissionMode,
    ) -> Result<u64, String> {
        if mode == SubmissionMode::Send {
            return self.submit(text, images);
        }
        let id = self.3.fetch_add(1, Ordering::Relaxed);
        self.send(Action::SubmitWithMode {
            id,
            text,
            images,
            mode,
        })?;
        Ok(id)
    }
    pub fn send(&self, action: Action) -> Result<(), String> {
        self.0
            .send(action)
            .map_err(|_| "Provider session is disconnected".to_owned())?;
        if let Some(wake) = &self.4 {
            wake();
        }
        Ok(())
    }
    pub fn stop_completion(&self, forget: bool) -> mpsc::Receiver<()> {
        self.stop_with_outcome(forget, None)
    }
    pub fn stop_archiving(&self) -> (mpsc::Receiver<()>, mpsc::Receiver<Result<(), String>>) {
        let (send, receive) = mpsc::channel();
        (self.stop_with_outcome(true, Some(send)), receive)
    }
    fn stop_with_outcome(
        &self,
        forget: bool,
        archive_outcome: Option<mpsc::Sender<Result<(), String>>>,
    ) -> mpsc::Receiver<()> {
        self.2.store(true, Ordering::SeqCst);
        let pid = self.1.swap(0, Ordering::SeqCst);
        #[cfg(not(unix))]
        let _ = pid;
        // Process termination is immediate even if the UI still holds this entity.
        #[cfg(unix)]
        if pid != 0 {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let (done, receive) = mpsc::channel();
        let _ = self.send(Action::Stop {
            forget,
            done,
            archive_outcome,
        });
        receive
    }
    pub fn stop(&self, forget: bool) {
        self.stop_completion(forget);
    }
}
pub struct Connection {
    pub handle: Handle,
    pub updates: crate::latest::Receiver<Snapshot>,
}

pub fn recovery_dir() -> Result<PathBuf, String> {
    crate::storage::config_dir()
        .map(|p| p.join("codex-sessions"))
        .map_err(|e| e.to_string())
}
/// Closed native sessions live separately from startup recovery.
pub fn session_history_dir() -> Result<PathBuf, String> {
    crate::storage::config_dir()
        .map(|p| p.join("session-history"))
        .map_err(|e| e.to_string())
}

/// Unresolved filesystem recovery remains an owner even after its actor closes.
/// Only an explicit reconnect/reopen may exclude its own journal identity.
pub fn workspace_recovery_blocked(
    paths: &[PathBuf],
    excluding_owner: Option<&str>,
) -> Result<bool, String> {
    workspace_recovery_blocked_in_dirs(
        paths,
        excluding_owner,
        &[recovery_dir()?, session_history_dir()?],
    )
}
pub(crate) fn workspace_recovery_blocked_in_dirs(
    paths: &[PathBuf],
    excluding_owner: Option<&str>,
    dirs: &[PathBuf],
) -> Result<bool, String> {
    #[derive(Deserialize)]
    struct Summary {
        #[serde(default)]
        pending_edit: Option<PendingSummary>,
    }
    #[derive(Deserialize)]
    struct PendingSummary {
        #[serde(default)]
        recovery: Option<crate::checkpoint::Checkpoint>,
    }
    let paths = paths
        .iter()
        .map(|path| path.canonicalize().unwrap_or_else(|_| path.clone()))
        .collect::<Vec<_>>();
    for dir in dirs {
        let entries = match fs_err::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "Workspace recovery ownership could not be checked: {error}"
                ));
            }
        };
        for entry in entries {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            if excluding_owner
                .is_some_and(|owner| path.file_stem().and_then(|stem| stem.to_str()) == Some(owner))
            {
                continue;
            }
            let file = match fs_err::File::open(&path) {
                Ok(file) => file,
                // Atomic archival/recovery can remove an entry after read_dir.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!(
                        "Workspace recovery journal could not be read: {error}"
                    ));
                }
            };
            let summary: Summary =
                serde_json::from_reader(std::io::BufReader::new(file)).map_err(|e| {
                    format!(
                        "Workspace recovery journal {} needs attention: {e}",
                        path.display()
                    )
                })?;
            if let Some(recovery) = summary.pending_edit.and_then(|pending| pending.recovery) {
                if recovery.roots().iter().any(|root| {
                    paths
                        .iter()
                        .any(|path| root.starts_with(path) || path.starts_with(root))
                }) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

pub fn load_session_history() -> Result<Recovery, String> {
    let dir = session_history_dir()?;
    if !dir.exists() {
        return Ok(Recovery {
            sessions: Vec::new(),
            warnings: Vec::new(),
        });
    }
    let mut history = load_from_dir(&dir)?;
    history.sessions.sort_by_cached_key(|saved| {
        std::cmp::Reverse(
            fs_err::metadata(dir.join(format!("{}.json", saved.key)))
                .and_then(|metadata| metadata.modified())
                .ok(),
        )
    });
    Ok(history)
}

/// Persist recovery before an archived record is adopted by the live registry.
pub fn prepare_history_reopen(saved: &SavedSession) -> Result<(), String> {
    persist(saved)
}

/// Called only after the live registry owns the reopened session.
pub fn finish_history_reopen(key: &str) -> Result<(), String> {
    if !valid_key(key) {
        return Err("Invalid native session history key".into());
    }
    let path = session_history_dir()?.join(format!("{key}.json"));
    match fs_err::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn archive_closed_session(saved: &SavedSession) -> Result<(), String> {
    archive_closed_in_dirs(saved, &recovery_dir()?, &session_history_dir()?)
}

pub(crate) fn archive_closed_in_dirs(
    saved: &SavedSession,
    active: &std::path::Path,
    history: &std::path::Path,
) -> Result<(), String> {
    // The archive is durable before removing the only startup recovery record.
    persist_in_dir(saved, history)?;
    match fs_err::remove_file(active.join(format!("{}.json", saved.key))) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}
fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() < 128
        && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
pub struct Recovery {
    pub sessions: Vec<SavedSession>,
    pub warnings: Vec<String>,
}
pub fn load_sessions() -> Result<Recovery, String> {
    let dir = recovery_dir()?;
    if !dir.exists() {
        return Ok(Recovery {
            sessions: Vec::new(),
            warnings: Vec::new(),
        });
    }
    load_from_dir(&dir)
}
pub(crate) fn load_from_dir(dir: &std::path::Path) -> Result<Recovery, String> {
    let mut recovery = Recovery {
        sessions: Vec::new(),
        warnings: Vec::new(),
    };
    for entry in fs_err::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let parsed = fs_err::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<SavedSession>(&bytes).map_err(|e| e.to_string())
            });
        match parsed {
            Ok(session)
                if valid_key(&session.key)
                    && path.file_stem().and_then(|x| x.to_str()) == Some(&session.key) =>
            {
                recovery.sessions.push(session);
            }
            _ => recovery.warnings.push(format!(
                "Could not restore native session {}. The recovery file was preserved.",
                path.display()
            )),
        }
    }
    Ok(recovery)
}

pub(crate) fn persist(saved: &SavedSession) -> Result<(), String> {
    if !valid_key(&saved.key) {
        return Err("Invalid provider recovery key".into());
    }
    persist_in_dir(saved, &recovery_dir()?)
}
pub(crate) fn persist_in_dir(saved: &SavedSession, dir: &std::path::Path) -> Result<(), String> {
    if !valid_key(&saved.key) {
        return Err("Invalid provider recovery key".into());
    }
    fs_err::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.json", saved.key));
    let mut file = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    {
        // JSON emits many small writes for keys and escaped tool output. Buffer
        // those writes, then propagate flush errors before syncing or replacing
        // the recovery file so atomicity and durability stay unchanged.
        let mut writer = std::io::BufWriter::with_capacity(64 * 1024, file.as_file_mut());
        serde_json::to_writer(&mut writer, saved).map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
    }
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    fs_err::File::open(dir)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn recover_saved_for_test(saved: &SavedSession) -> SavedSession {
    let dir = tempfile::tempdir().unwrap();
    persist_in_dir(saved, dir.path()).unwrap();
    load_from_dir(dir.path()).unwrap().sessions.remove(0)
}

#[cfg(test)]
mod independent_conversation_tests {
    use super::*;
    #[test]
    fn command_wake_happens_after_enqueue_and_clones_retain_it() {
        let observed = Arc::new(AtomicU64::new(0));
        let wakes = observed.clone();
        let (handle, receiver) = Handle::channel_with_wake(Arc::new(move || {
            wakes.fetch_add(1, Ordering::Relaxed);
        }));
        handle.clone().send(Action::Draft("queued".into())).unwrap();
        assert_eq!(observed.load(Ordering::Relaxed), 1);
        assert!(matches!(receiver.try_recv().unwrap(), Action::Draft(text) if text == "queued"));
        drop(receiver);
        assert!(handle.send(Action::Draft("disconnected".into())).is_err());
        assert_eq!(
            observed.load(Ordering::Relaxed),
            1,
            "failed enqueue must not wake"
        );
    }

    #[test]
    fn new_session_preserves_preferences_and_clears_conversation_ownership() {
        for provider in [Provider::Codex, Provider::Claude] {
            let mut source = SavedSession::empty(
                "source".into(),
                provider,
                "project".into(),
                "/checkout".into(),
            );
            source.thread_id = Some("old-thread".into());
            source.composer.model = Some("selected-model".into());
            source.composer.effort = Some("medium".into());
            source.config_args = vec!["permission-settings".into()];
            source.draft = "untouched original draft".into();
            source.items.push(json!({"id":"old","type":"userMessage","content":[{"type":"text","text":"private old conversation"}]}));
            source.session_review.summary = "old receipt".into();
            let fresh = source.independent_conversation("new-session".into());
            assert_eq!(fresh.key, "new-session");
            assert_eq!(fresh.provider, provider);
            assert_eq!(fresh.composer.model, source.composer.model);
            assert_eq!(fresh.config_args, source.config_args);
            assert!(fresh.thread_id.is_none());
            assert!(fresh.items.is_empty());
            assert!(fresh.draft.is_empty());
            assert!(fresh.session_review.summary.is_empty());
            assert_eq!(source.draft, "untouched original draft");
            assert_eq!(source.items.len(), 1);
        }
    }
}
