//! Grove-owned Codex app-server sessions. All process and disk I/O runs on an actor thread.
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ComposerSettings {
    #[serde(default)]
    pub reset_to_configured: bool,
    #[serde(default)]
    pub reset_model_effort_only: bool,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub access: Option<String>,
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
            Self::Codex => launch_options(args),
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
    pub items: Vec<Value>,
    pub events: Vec<SubagentActivityEvent>,
    pub observed_at: Option<Instant>,
    pub refreshing: bool,
    pub refresh_error: bool,
    pub truncated: bool,
    poll_cursor: Option<(u64, u64)>,
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
fn record_subagent(
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
                SubagentStatus::Completed | SubagentStatus::Failed
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
    fn is_canonical_prompt(text: &str) -> bool {
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedSession {
    #[serde(default)]
    pub rich_plans: crate::rich_plan::PlanStore,
    #[serde(default)]
    pub claude_resume: Option<ClaudeResume>,
    #[serde(default)]
    pub edit_checkpoints: Vec<EditCheckpoint>,
    #[serde(default)]
    pub pending_edit: Option<PendingEdit>,
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
    pub items: Vec<Value>,
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
    pub composer: ComposerSettings,
    /// Last known context-window occupancy, so a reopened session shows its meter.
    #[serde(default)]
    pub context: Option<ContextUsage>,
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
    pub files: crate::checkpoint::Checkpoint,
}
/// Durable recovery journal: no provider request is replayed automatically.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingEdit {
    pub target: usize,
    pub turn_id: String,
    pub text: String,
    pub images: Vec<String>,
    pub recovery: crate::checkpoint::Checkpoint,
    #[serde(default)]
    pub composer_draft: Option<(String, Vec<String>)>,
    #[serde(default)]
    pub claude_fork_id: Option<String>,
}
impl SavedSession {
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

    pub(crate) fn fresh_conversation(&self) -> Self {
        let mut saved = self.clone();
        saved.thread_id = None;
        saved.claude_resume = None;
        saved.edit_checkpoints.clear();
        saved.pending_edit = None;
        saved.title = None;
        saved.title_generation = None;
        saved.items.clear();
        saved.rich_plans = crate::rich_plan::PlanStore::default();
        saved.work_periods.clear();
        saved.draft.clear();
        saved.draft_images.clear();
        saved.draft_reference = None;
        saved.context = None;
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
    source: QuestionSource,
    instance: u64,
}
#[derive(Clone, Debug, PartialEq)]
enum QuestionSource {
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
    fn from_async_message(item: &Value) -> Option<Self> {
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
    fn answer_text(&self, answers: &QuestionAnswers) -> String {
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
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub saved: SavedSession,
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
    /// Monotonic local acknowledgement; only a successful turn/start response advances it.
    pub accepted_prompts: u64,
    pub cleared_conversations: u64,
    pub edited_messages: u64,
    pub edit_supported: bool,
    pub can_redirect: bool,
    pub accepted_submission: Option<u64>,
    pub rejected_submission: Option<u64>,
    pub queued_prompts: Vec<QueuedPrompt>,
    pub queue_edit: Option<u64>,
    pub queue_sending: Vec<u64>,
    pub queue_steering: Vec<u64>,
    pub queue_uncertain: Vec<u64>,
    /// The provider is compacting the conversation context right now.
    pub compacting: bool,
    /// Grove asked for the next compaction, so it is labelled manual.
    pub(crate) compact_requested: bool,
    /// Latest context occupancy even when the window is still unknown.
    pub(crate) context_tokens: Option<u64>,
    settled_turns: Vec<String>,
    subagent_restart_calls: Vec<String>,
    subagent_observation_generation: u64,
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

    pub fn editable_message(&self, index: usize) -> Result<&EditCheckpoint, String> {
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
                || (index > 0 && item["claudePreviousAssistantUuid"].as_str().is_none())
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
        }).ok_or_else(|| "No workspace checkpoint is available for this message. Only new turns with a saved checkpoint can be edited.".into())
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
        let composer = saved.composer.clone();
        let context_tokens = saved.context.map(|usage| usage.used);
        Self {
            saved,
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
            accepted_prompts: 0,
            cleared_conversations: 0,
            edited_messages: 0,
            edit_supported: false,
            can_redirect: false,
            accepted_submission: None,
            rejected_submission: None,
            queued_prompts: Vec::new(),
            queue_edit: None,
            queue_sending: Vec::new(),
            queue_steering: Vec::new(),
            queue_uncertain: Vec::new(),
            compacting: false,
            compact_requested: false,
            context_tokens,
            settled_turns: Vec::new(),
            subagent_restart_calls: Vec::new(),
            subagent_observation_generation: 0,
            subagent_activity: HashMap::new(),
            subagent_activity_evicted: HashSet::new(),
        }
    }
    pub(crate) fn begin_work(&mut self) {
        if self.work_started.is_some() {
            return;
        }
        self.work_started = Some(Instant::now());
        self.saved.work_periods.push(WorkPeriod {
            start_item: self.saved.items.len(),
            end_item: None,
            duration_ms: None,
            subagents: Vec::new(),
        });
    }
    fn subagent_terminal(&self, id: &str) -> bool {
        self.saved
            .work_periods
            .iter()
            .rev()
            .flat_map(|p| p.subagents.iter().rev())
            .find(|a| a.id == id)
            .is_some_and(|a| matches!(a.status, SubagentStatus::Completed | SubagentStatus::Failed))
    }
    fn ensure_subagent_activity(&mut self, id: &str) {
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
    pub(crate) fn upsert_subagent_item(&mut self, id: &str, item: Value) {
        if item["type"] == "reasoning" || self.subagent_period(id).is_none() {
            return;
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
        saved.rich_plans = crate::rich_plan::PlanStore::default();
        saved.draft.clear();
        saved.draft_images.clear();
        saved.draft_reference = None;
        let mut state = Self::new(saved);
        state.status = if self.subagent_terminal(id) {
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
    fn subagent_activity_bytes(&self) -> usize {
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
    fn subagent_period(&self, id: &str) -> Option<usize> {
        self.saved
            .work_periods
            .iter()
            .rposition(|p| p.subagents.iter().any(|a| a.id == id))
    }
    /// Collab call status describes the call, not the child. Only agentsStates is authoritative.
    fn record_collab(&mut self, item: &Value, period_index: usize) {
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
    fn apply_subagent_notification(&mut self, message: &Value, period: usize) -> bool {
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
    fn apply_subagent_read(&mut self, id: &str, period_start: usize, thread: &Value) {
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
                    activity.items = chronological;
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
    fn resolve_async_question(&mut self, id: &Value) {
        if let Some(item) = self.saved.items.iter_mut().find(|item| item["id"] == *id) {
            item["groveQuestionResolved"] = json!(true);
        }
        self.questions.retain(|question| question.id != *id);
    }
    fn restore_async_questions(&mut self) {
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
            .map(item_text)
            .collect::<Vec<_>>()
            .join("\n\n")
    }
    pub fn apply(&mut self, message: &Value) -> bool {
        let p = &message["params"];
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
            }
            "item/started" | "item/completed" => {
                if p["item"]["type"] == "agentMessage" {
                    self.clear_retry_notice();
                }
                let mut item = p["item"].clone();
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
                        .find(|old| old["id"] == p["item"]["id"]),
                );
                if let Some(old) = self.saved.items.iter().find(|old| old["id"] == item["id"]) {
                    for marker in ["groveQuestionResolved", "groveQuestionPending"] {
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
                    self.saved.items[at] = item;
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
                let at = self.item_index(&id, "plan");
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
            Some(at) => self.saved.items[at] = item,
            None => self.saved.items.push(item),
        }
    }
    pub(crate) fn set_provider_title(&mut self, name: Option<&str>) {
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
    fn fail(&mut self, error: String) {
        self.finish_work();
        self.error = Some(error);
        self.status = Status::Disconnected;
        self.turn_id = None;
        self.compacting = false;
        self.compact_requested = false;
        self.approvals.clear();
        self.questions.clear();
        self.elicitations.clear();
    }
}
fn prompt_title(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(80).collect())
}
/// Whether a transcript holds any conversation that compaction could shorten.
pub fn has_conversation(items: &[Value]) -> bool {
    items
        .iter()
        .any(|item| item["type"] == "userMessage" || item["type"] == "agentMessage")
}
fn first_prompt_title(items: &[Value]) -> Option<String> {
    items
        .iter()
        .filter(|item| item["type"] == "userMessage")
        .find_map(|item| prompt_title(&item_text(item)))
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
    match item["type"].as_str().unwrap_or_default() {
        "userMessage" => item["content"]
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p["text"].as_str())
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

fn prompt_input(text: &str, images: Vec<String>) -> Vec<Value> {
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
    /// UI responses are bound to the connection that rendered the interaction.
    Interaction {
        epoch: u64,
        action: Box<Action>,
    },
    UpdatePlanReview {
        plan_id: String,
        revision_id: String,
        review: crate::rich_plan::ReviewState,
    },
    SendPlanResponse {
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
    Metadata {
        project: String,
        roots: Vec<crate::session_meta::ContextRoot>,
    },
    Stop {
        forget: bool,
        done: mpsc::Sender<()>,
    },
}
#[derive(Clone)]
pub struct Handle(
    pub(crate) mpsc::Sender<Action>,
    pub(crate) Arc<AtomicU32>,
    pub(crate) Arc<AtomicBool>,
    pub(crate) Arc<AtomicU64>,
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
            ),
            receive,
        )
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
            .map_err(|_| "Codex session is disconnected".into())
    }
    pub fn stop_completion(&self, forget: bool) -> mpsc::Receiver<()> {
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
        let _ = self.send(Action::Stop { forget, done });
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
fn load_from_dir(dir: &std::path::Path) -> Result<Recovery, String> {
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
        return Err("Invalid Codex recovery key".into());
    }
    persist_in_dir(saved, &recovery_dir()?)
}
fn persist_in_dir(saved: &SavedSession, dir: &std::path::Path) -> Result<(), String> {
    if !valid_key(&saved.key) {
        return Err("Invalid Codex recovery key".into());
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
    serde_json::to_writer(&mut file, saved).map_err(|e| e.to_string())?;
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

/// Translate terminal launcher arguments without changing configured permissions or model.
/// Explicit legacy permission bypass is mapped only when requested by the user.
pub fn launch_options(args: &[String]) -> Result<(Vec<String>, Option<String>), String> {
    let mut configs = Vec::new();
    let mut prompt = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--no-alt-screen" => {}
            "-c" | "--config" => {
                i += 1;
                let value = args.get(i).ok_or("Missing Codex configuration value")?;
                if !value.starts_with("tui.") {
                    configs.extend(["-c".into(), value.clone()]);
                }
            }
            "--add-dir" => {
                i += 1;
                args.get(i).ok_or("Missing Codex additional directory")?;
            }
            "--dangerously-bypass-approvals-and-sandbox" => configs.extend([
                "-c".into(),
                "approval_policy=\"never\"".into(),
                "-c".into(),
                "sandbox_mode=\"danger-full-access\"".into(),
            ]),
            flag if flag.starts_with('-') => {
                return Err(format!(
                    "Native Codex does not support launcher option {flag}"
                ));
            }
            text => {
                if prompt.is_some() {
                    return Err("Multiple Codex launch prompts are unsupported".into());
                }
                prompt = Some(text.into());
            }
        }
        i += 1;
    }
    Ok((configs, prompt))
}

pub fn connect(saved: SavedSession, args: Vec<String>) -> Connection {
    if saved.provider == Provider::Claude {
        return crate::claude::connect(saved, args);
    }
    let (handle, receive) = Handle::channel();
    let (updates, output) = crate::latest::channel();
    let actor_pid = handle.1.clone();
    let actor_stopping = handle.2.clone();
    std::thread::spawn(move || {
        let mut actor = Actor::new(saved, receive, updates, args);
        actor.pid = actor_pid;
        actor.stopping = actor_stopping;
        actor.run();
    });
    Connection {
        handle,
        updates: output,
    }
}
struct Process {
    child: Child,
    input: ChildStdin,
    lines: mpsc::Receiver<Result<Value, String>>,
}
impl Drop for Process {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn spawn(args: &[String], cwd: &str) -> Result<Process, String> {
    let mut argv = vec!["app-server".into(), "--stdio".into()];
    argv.extend_from_slice(args);
    let (program, argv) = crate::agent::Agent::Codex.session_invocation(&argv);
    let mut cmd = Command::new(program);
    cmd.args(argv)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(env) = crate::env_path::session_environment() {
        cmd.env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    }
    cmd.env(
        "GROVE_CONFIG_DIR",
        crate::storage::config_dir().map_err(|e| e.to_string())?,
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| {
        format!("Cannot launch Codex. Install the Codex CLI and sign in with `codex login`: {e}")
    })?;
    let input = child.stdin.take().ok_or("Codex stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("Codex stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("Codex stderr unavailable")?;
    // Keep a bounded, session-private diagnostic tail for an actionable exit error.
    // Never write provider output to global logs.
    let diagnostics = Arc::new(Mutex::new(String::new()));
    let diagnostic_writer = diagnostics.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else {
                break;
            };
            if let Ok(mut tail) = diagnostic_writer.lock() {
                tail.push_str(&line);
                tail.push('\n');
                if tail.len() > 4096 {
                    let mut at = tail.len() - 4096;
                    while !tail.is_char_boundary(at) {
                        at += 1;
                    }
                    tail.drain(..at);
                }
            }
        }
    });
    let (send, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let value = line.map_err(|e| e.to_string()).and_then(|line| {
                serde_json::from_str(&line)
                    .map_err(|e| format!("Invalid Codex protocol message: {e}"))
            });
            if send.send(value).is_err() {
                return;
            }
        }
        let tail = diagnostics
            .lock()
            .map(|tail| tail.trim().to_owned())
            .unwrap_or_default();
        let detail = if tail.is_empty() {
            String::new()
        } else {
            format!("\n{tail}")
        };
        let _ = send.send(Err(format!(
            "Codex exited. Check CLI authentication/configuration, then reconnect.{detail}"
        )));
    });
    Ok(Process {
        child,
        input,
        lines,
    })
}
#[derive(Clone)]
enum Pending {
    Initialize,
    Models,
    ChildRead {
        thread: String,
        period_start: usize,
        generation: u64,
    },
    Skills,
    Config,
    Thread,
    Turn {
        submission: Option<u64>,
        text: String,
        images: Vec<String>,
    },
    InlineTurn,
    Steer {
        submission: u64,
        turn: String,
        draft: QueuedPrompt,
        from_queue: bool,
    },
    QueuedTurn {
        id: u64,
    },
    AsyncAnswer {
        question_id: Value,
        request_instance: u64,
        ack: QuestionAnswerAck,
    },
    Compact,
    ChildInterrupt,
    EditMessage,
    RecoverEdit,
    Interrupt {
        attempt: u8,
        turn: String,
    },
}
struct Request {
    kind: Pending,
    deadline: Instant,
}
struct Actor {
    state: Snapshot,
    actions: mpsc::Receiver<Action>,
    updates: crate::latest::Sender<Snapshot>,
    process: Option<Process>,
    pending: HashMap<u64, Request>,
    seq: u64,
    args: Vec<String>,
    initial: Option<String>,
    interrupt_retry: Option<(Instant, u8, String)>,
    queue_ready: bool,
    queue_paused: bool,
    configured_turn: Value,
    developer_instructions: String,
    reset_configuration: bool,
    dirty: bool,
    pid: Arc<AtomicU32>,
    stopping: Arc<AtomicBool>,
    last_save: Instant,
    last_child_poll: Instant,
    child_read_failures: HashMap<String, u8>,
    interaction_children: crate::interaction::Children,
    needs_save: bool,
    title_generator: crate::session_title::Generator,
}
impl Actor {
    fn new(
        saved: SavedSession,
        actions: mpsc::Receiver<Action>,
        updates: crate::latest::Sender<Snapshot>,
        args: Vec<String>,
    ) -> Self {
        let reset_configuration = saved.composer.reset_to_configured;
        Self {
            state: Snapshot::new(saved),
            actions,
            updates,
            process: None,
            pending: HashMap::new(),
            seq: 0,
            args,
            initial: None,
            interrupt_retry: None,
            queue_ready: false,
            queue_paused: false,
            configured_turn: json!({}),
            developer_instructions: String::new(),
            reset_configuration,
            dirty: true,
            pid: Arc::new(AtomicU32::new(0)),
            stopping: Arc::new(AtomicBool::new(false)),
            last_save: Instant::now(),
            last_child_poll: Instant::now(),
            child_read_failures: HashMap::new(),
            interaction_children: crate::interaction::Children::default(),
            needs_save: false,
            title_generator: crate::session_title::Generator::default(),
        }
    }
    fn save(&mut self) {
        if let Err(e) = persist(&self.state.saved) {
            self.state.error = Some(format!("Could not save Codex session: {e}"));
            let _ = self.updates.send(self.state.clone());
        }
        self.last_save = Instant::now();
        self.needs_save = false;
    }
    fn emit(&mut self) {
        self.needs_save = true;
        let _ = self.updates.send(self.state.clone());
        self.dirty = false;
    }
    fn write(&mut self, value: &Value) -> Result<(), String> {
        let p = self.process.as_mut().ok_or("Codex is disconnected")?;
        serde_json::to_writer(&mut p.input, value).map_err(|e| e.to_string())?;
        p.input.write_all(b"\n").map_err(|e| e.to_string())?;
        p.input.flush().map_err(|e| e.to_string())
    }
    fn request(&mut self, method: &str, params: Value, kind: Pending) -> Result<(), String> {
        self.seq += 1;
        self.write(&json!({"id":self.seq,"method":method,"params":params}))?;
        self.pending.insert(
            self.seq,
            Request {
                kind,
                deadline: Instant::now() + Duration::from_secs(30),
            },
        );
        Ok(())
    }
    fn start(&mut self) -> Result<(), String> {
        if self.stopping.load(Ordering::SeqCst) {
            return Err("Codex session is closing".into());
        }
        let (mut args, initial) = launch_options(&self.args)?;
        if args.is_empty() {
            self.state.saved.migrate_native_config();
            args.clone_from(&self.state.saved.config_args);
        } else {
            self.state.saved.config_args.clone_from(&args);
            self.state.saved.native_config_version = 1;
        }
        if self.state.saved.thread_id.is_none() {
            self.initial = initial;
        }
        self.state.mark_queue_uncertain();
        self.queue_ready = false;
        self.queue_paused = !self.state.queued_prompts.is_empty();
        self.pending.clear();
        self.state.models.clear();
        self.state.skills.clear();
        self.interrupt_retry = None;
        self.interaction_children = crate::interaction::Children::default();
        self.state.interaction_owners.clear();
        self.state.retired_interactions.clear();
        self.state.interaction_epoch = next_interaction_epoch();
        self.state.approvals.clear();
        self.state.questions.clear();
        self.state.elicitations.clear();
        self.state.turn_id = None;
        self.state.status = Status::Starting;
        self.state.clear_error();
        self.process = Some(spawn(&args, &self.state.saved.cwd)?);
        if let Some(process) = &self.process {
            self.pid.store(process.child.id(), Ordering::SeqCst);
        }
        if self.stopping.load(Ordering::SeqCst) {
            self.pid.store(0, Ordering::SeqCst);
            self.process = None;
            return Err("Codex session is closing".into());
        }
        self.request("initialize",json!({"clientInfo":{"name":"grove","title":"Grove","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),Pending::Initialize)
    }
    fn start_thread(&mut self, writable_roots: Option<Vec<Value>>) -> Result<(), String> {
        // Codex disables its native plan tool by default. Instructions alone do
        // not register it; enable it for both new and resumed Grove threads.
        let mut params = json!({"cwd": self.state.saved.cwd, "config": {
            "tools.update_plan.enabled": true
        }});
        params["developerInstructions"] = json!(format!(
            "{}{}{}\n\n{}\n\n{}\n\n{}",
            self.developer_instructions,
            if self.developer_instructions.is_empty() {
                ""
            } else {
                "\n\n"
            },
            crate::agent_guidance::WORKFLOW_GUIDANCE,
            crate::tasks::CODEX_GUIDANCE,
            crate::reports::REPORT_GUIDANCE,
            crate::rich_plan::GUIDANCE
        ));
        if let Some(roots) = writable_roots {
            params["config"]["sandbox_workspace_write.writable_roots"] = json!(roots);
        }
        let method = if let Some(thread) = &self.state.saved.thread_id {
            params["threadId"] = json!(thread);
            "thread/resume"
        } else {
            params["historyMode"] = json!("paginated");
            "thread/start"
        };
        self.request(method, params, Pending::Thread)
    }
    fn input(&self, text: &str, images: Vec<String>) -> Vec<Value> {
        let mut input = prompt_input(text, images);
        for skill in &self.state.skills {
            let Some(name) = skill["name"].as_str() else {
                continue;
            };
            if skill["enabled"] == false {
                continue;
            }
            let command_text = text
                .strip_prefix("Referenced transcript selection (quoted data):\n")
                .and_then(|body| body.split_once("\n\nUser reply:\n").map(|(_, reply)| reply))
                .unwrap_or(text);
            if command_text
                .split_whitespace()
                .any(|word| word.trim_end_matches([',', '.', ';', ')']) == format!("${name}"))
            {
                if let Some(path) = skill["path"].as_str() {
                    input.push(json!({"type":"skill","name":name,"path":path}));
                }
            }
        }
        input
    }
    fn turn_params(&self, input: Vec<Value>) -> Value {
        let mut params = if self.reset_configuration {
            if self.state.saved.composer.reset_model_effort_only {
                // Omitted permission fields retain the provider's exact effective
                // policies, including settings richer than Grove's access modes.
                let mut configured = json!({});
                for key in ["model", "effort"] {
                    if let Some(value) = self.configured_turn.get(key) {
                        configured[key] = value.clone();
                    }
                }
                configured
            } else {
                self.configured_turn.clone()
            }
        } else {
            json!({})
        };
        params["threadId"] = json!(self.state.saved.thread_id);
        params["input"] = json!(input);
        if let Some(model) = &self.state.model {
            params["model"] = json!(model);
        }
        if let Some(effort) = &self.state.effort {
            params["effort"] = json!(effort);
        }
        if let Some(access) = &self.state.access {
            params["approvalPolicy"] = json!(if access == "full-access" {
                "never"
            } else {
                "on-request"
            });
            params["sandboxPolicy"] = match access.as_str() {
                "full-access" => json!({"type":"dangerFullAccess"}),
                "read-only" => json!({"type":"readOnly"}),
                _ => {
                    json!({"type":"workspaceWrite","writableRoots":self.state.saved.roots.iter().map(|root| &root.wt_path).collect::<Vec<_>>()})
                }
            };
        }
        params
    }
    fn capture_edit_checkpoint(&mut self) {
        if !self.state.edit_supported {
            return;
        }
        let mut roots = vec![PathBuf::from(&self.state.saved.cwd)];
        roots.extend(
            self.state
                .saved
                .roots
                .iter()
                .map(|root| PathBuf::from(&root.wt_path)),
        );
        roots.sort();
        roots.dedup();
        // Checkpoint errors never prevent ordinary chat; Edit explains unavailable turns.
        if let Ok(files) = crate::checkpoint::repository_roots(&roots)
            .and_then(|roots| crate::checkpoint::capture(&roots))
        {
            self.state
                .saved
                .edit_checkpoints
                .retain(|record| record.turn_id.is_some());
            self.state.saved.edit_checkpoints.push(EditCheckpoint {
                start_item: self.state.saved.items.len(),
                turn_id: None,
                files,
            });
        }
    }
    fn bind_edit_checkpoint(&mut self, result: &Value) {
        if let Some(turn) = result["turn"]["id"].as_str() {
            if let Some(record) = self
                .state
                .saved
                .edit_checkpoints
                .last_mut()
                .filter(|record| record.turn_id.is_none())
            {
                record.turn_id = Some(turn.to_owned());
            }
        }
    }
    fn edit_message(&mut self, index: usize, message_id: String) -> Result<(), String> {
        self.edit_message_with_draft(index, message_id, None)
    }
    fn edit_message_with_draft(
        &mut self,
        index: usize,
        message_id: String,
        composer_draft: Option<(String, Vec<String>)>,
    ) -> Result<(), String> {
        let record = self.state.editable_message(index)?.clone();
        if self.process.is_none() || !self.pending.is_empty() {
            return Err("Wait for the provider to finish reconnecting before editing.".into());
        }
        let item = &self.state.saved.items[index];
        if item["id"].as_str().unwrap_or("") != message_id {
            return Err("This message changed; select Edit again.".into());
        }
        let text = item_text(item);
        let images = item["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|part| {
                part["url"]
                    .as_str()
                    .filter(|_| part["type"] == "image")
                    .map(str::to_owned)
            })
            .collect();
        let mut roots = vec![PathBuf::from(&self.state.saved.cwd)];
        roots.extend(
            self.state
                .saved
                .roots
                .iter()
                .map(|root| PathBuf::from(&root.wt_path)),
        );
        roots.sort();
        roots.dedup();
        let roots = crate::checkpoint::repository_roots(&roots)?;
        let recovery = crate::checkpoint::capture(&roots)?;
        let turn_id = record.turn_id.ok_or("Missing turn identity")?;
        self.state.saved.pending_edit = Some(PendingEdit {
            target: record.start_item,
            turn_id: turn_id.clone(),
            text,
            images,
            recovery,
            composer_draft,
            claude_fork_id: None,
        });
        // Persist recovery before the first filesystem mutation or provider request.
        persist(&self.state.saved)?;
        if let Err(error) = record.files.restore() {
            self.recover_failed_edit()?;
            return Err(error);
        }
        if let Err(error) = self.request(
            "thread/revert",
            json!({"threadId":self.state.saved.thread_id,"beforeTurnId":turn_id}),
            Pending::EditMessage,
        ) {
            // A write or flush error does not prove the provider rejected the request.
            // Keep the durable journal and reconcile history after reconnecting.
            self.pid.store(0, Ordering::SeqCst);
            self.process = None;
            self.state.status = Status::Disconnected;
            return Err(format!(
                "Edit delivery is uncertain. Reconnect to recover the workspace and conversation: {error}"
            ));
        }
        self.state.status = Status::Working;
        self.state.clear_error();
        Ok(())
    }
    fn recover_failed_edit(&mut self) -> Result<(), String> {
        if let Some(edit) = &self.state.saved.pending_edit {
            edit.recovery.restore().map_err(|error| {
                format!("Edit failed and workspace recovery needs attention: {error}")
            })?;
        }
        let mut saved = self.state.saved.clone();
        saved.pending_edit = None;
        persist(&saved)?;
        self.state.saved = saved;
        self.state.status = Status::Idle;
        Ok(())
    }
    fn finish_edit(&mut self) -> Result<(), String> {
        let mut saved = self.state.saved.clone();
        let edit = saved
            .pending_edit
            .take()
            .ok_or("Missing edit recovery journal")?;
        saved.items.truncate(edit.target);
        saved.edit_checkpoints.retain(|record| {
            record.start_item < edit.target && record.turn_id.as_deref() != Some(&edit.turn_id)
        });
        saved
            .work_periods
            .retain(|period| period.start_item < edit.target);
        saved.rich_plans.retain_messages(&saved.items);
        let (text, images) = edit.composer_draft.unwrap_or((edit.text, edit.images));
        saved.draft = text;
        saved.draft_images = images;
        saved.context = None;
        persist(&saved)?;
        self.state.saved = saved;
        self.state.context_tokens = None;
        self.state.turn_id = None;
        self.state.status = Status::Idle;
        self.state.edited_messages += 1;
        Ok(())
    }
    fn prompt(&mut self, text: String) -> Result<(), String> {
        self.prompt_with_images(text, Vec::new(), None)
    }
    fn prompt_with_images(
        &mut self,
        text: String,
        images: Vec<String>,
        submission: Option<u64>,
    ) -> Result<(), String> {
        if self.state.status.busy()
            || self.state.turn_id.is_some()
            || self.pending.values().any(|r| {
                matches!(
                    r.kind,
                    Pending::Turn { .. }
                        | Pending::InlineTurn
                        | Pending::QueuedTurn { .. }
                        | Pending::Compact
                        | Pending::AsyncAnswer { .. }
                )
            })
        {
            return Err(
                "Wait for the current turn, or interrupt it before sending another prompt.".into(),
            );
        }
        validate_prompt(&text, &images)?;
        let thread = self
            .state
            .saved
            .thread_id
            .clone()
            .ok_or("Reconnect before sending a prompt")?;
        if self.process.is_none() {
            return Err("Reconnect before sending a prompt".into());
        }
        let reference = self
            .state
            .saved
            .draft_reference
            .as_ref()
            .is_some_and(|reference| reference.serialize(&self.state.saved.draft) == text);
        if !reference {
            self.state.saved.draft.clone_from(&text);
            self.state.saved.draft_images.clone_from(&images);
        }
        let pending = Pending::Turn {
            submission,
            text: text.clone(),
            images: images.clone(),
        };
        let _ = thread;
        self.capture_edit_checkpoint();
        self.request(
            "turn/start",
            self.turn_params(self.input(&text, images)),
            pending,
        )?;
        self.state.status = Status::Working;
        self.state.clear_error();
        Ok(())
    }
    fn submit_mode(
        &mut self,
        id: u64,
        text: String,
        images: Vec<String>,
        mode: SubmissionMode,
    ) -> Result<(), String> {
        validate_prompt(&text, &images)?;
        if self.process.is_none()
            || self.state.saved.thread_id.is_none()
            || self.state.status == Status::Disconnected
        {
            return Err("Reconnect before sending a prompt".into());
        }
        match mode {
            SubmissionMode::Send => {
                self.prompt_with_images(text, images, Some(id))?;
            }
            SubmissionMode::Steer => {
                let turn = self
                    .state
                    .turn_id
                    .clone()
                    .filter(|_| self.state.status.busy())
                    .ok_or("No active Codex turn to steer")?;
                let input = self.input(&text, images.clone());
                let draft = QueuedPrompt { id, text, images };
                self.state.queued_prompts.push(draft.clone());
                self.state.queue_sending.push(id);
                self.state.queue_steering.push(id);
                self.request("turn/steer", json!({"threadId":self.state.saved.thread_id,"expectedTurnId":turn,"input":input}), Pending::Steer { submission: id, turn, draft, from_queue: false }).inspect_err(|_| { self.queue_paused = true; self.queue_ready = false; self.state.mark_queue_uncertain(); })?;
                self.state.clear_error();
            }
            SubmissionMode::Queue => {
                self.state.saved.clear_submitted_draft(&text, &images);
                self.state
                    .queued_prompts
                    .push(QueuedPrompt { id, text, images });
                self.state.accepted_submission = Some(id);
                // An idle, healthy session can begin an explicitly queued submission.
                if !self.state.status.busy() && self.state.turn_id.is_none() && !self.queue_paused {
                    self.queue_ready = true;
                    if let Err(error) = self.dispatch_queue() {
                        self.queue_ready = false;
                        self.queue_paused = true;
                        self.state.error = Some(error);
                    }
                }
            }
        }
        Ok(())
    }
    /// Ask Codex to compact the current thread. The app-server runs compaction as its own
    /// turn, and starting it during an active turn would interrupt that turn.
    fn compact(&mut self) -> Result<(), String> {
        if self.process.is_none() || self.state.status == Status::Disconnected {
            return Err(COMPACT_DISCONNECTED.into());
        }
        if self.state.status.busy()
            || self.state.turn_id.is_some()
            || self.pending.values().any(|r| {
                matches!(
                    r.kind,
                    Pending::Turn { .. }
                        | Pending::InlineTurn
                        | Pending::QueuedTurn { .. }
                        | Pending::Steer { .. }
                        | Pending::Compact
                        | Pending::AsyncAnswer { .. }
                )
            })
        {
            return Err(COMPACT_BUSY.into());
        }
        let thread = self
            .state
            .saved
            .thread_id
            .clone()
            .filter(|_| has_conversation(&self.state.saved.items))
            .ok_or(NOTHING_TO_COMPACT)?;
        self.request(
            "thread/compact/start",
            json!({ "threadId": thread }),
            Pending::Compact,
        )?;
        self.state.compact_requested = true;
        self.state.clear_error();
        Ok(())
    }
    fn dispatch_queue(&mut self) -> Result<(), String> {
        if !self.queue_ready
            || self.queue_paused
            || !self.state.queue_sending.is_empty()
            || self.state.turn_id.is_some()
            || self.state.status.busy()
            || self.pending.values().any(|r| {
                matches!(
                    r.kind,
                    Pending::Turn { .. }
                        | Pending::InlineTurn
                        | Pending::QueuedTurn { .. }
                        | Pending::Steer { .. }
                        | Pending::Compact
                        | Pending::AsyncAnswer { .. }
                )
            })
        {
            return Ok(());
        }
        let Some(prompt) = self.state.queued_prompts.first().cloned() else {
            return Ok(());
        };
        if self.state.queue_edit == Some(prompt.id) || self.state.queue_sending.contains(&prompt.id)
        {
            return Ok(());
        }
        self.state.queue_sending.push(prompt.id);
        self.capture_edit_checkpoint();
        self.request(
            "turn/start",
            self.turn_params(self.input(&prompt.text, prompt.images)),
            Pending::QueuedTurn { id: prompt.id },
        )
        .inspect_err(|_| {
            self.state.mark_queue_uncertain();
        })?;
        self.queue_ready = false;
        self.state.status = Status::Working;
        self.state.clear_error();
        Ok(())
    }
    fn clear_conversation(
        &mut self,
        persist_saved: impl FnOnce(&SavedSession) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.state.status.busy()
            || self.state.turn_id.is_some()
            || self.state.compacting
            || self.pending.values().any(|r| {
                matches!(
                    r.kind,
                    Pending::Turn { .. }
                        | Pending::InlineTurn
                        | Pending::QueuedTurn { .. }
                        | Pending::Steer { .. }
                        | Pending::Compact
                        | Pending::AsyncAnswer { .. }
                        | Pending::Interrupt { .. }
                )
            })
        {
            return Err(CLEAR_BUSY.into());
        }
        if self.process.is_none() {
            return Err(CLEAR_DISCONNECTED.into());
        }
        // Persist the empty identity before publishing it or replacing the transport.
        // A crash during restart must never resume the conversation being cleared.
        let args = launch_options(&self.args)?.0;
        let saved = self.state.saved.fresh_conversation();
        persist_saved(&saved).map_err(|e| format!("Could not clear conversation: {e}"))?;
        self.pid.store(0, Ordering::SeqCst);
        self.process = None;
        self.pending.clear();
        self.interrupt_retry = None;
        self.queue_ready = false;
        self.queue_paused = false;
        self.initial = None;
        self.args = args;
        let cleared = self.state.cleared_conversations + 1;
        self.state = Snapshot::new(saved);
        self.state.cleared_conversations = cleared;
        Ok(())
    }
    fn send_plan_response(
        &mut self,
        plan_id: &str,
        revision_id: &str,
        persist_saved: impl Fn(&SavedSession) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.process.is_none() || self.state.saved.thread_id.is_none() {
            return Err("Reconnect before sending plan feedback".into());
        }
        if let Some((id, text)) = self
            .state
            .saved
            .rich_plans
            .prepare_delivery(plan_id, revision_id)?
        {
            // Journal before any provider write. Recovery never replays an ambiguous send.
            if let Err(error) = persist_saved(&self.state.saved) {
                self.state.saved.rich_plans.fail_delivery(id, &error);
                return Err(error);
            }
            self.state.queued_prompts.push(QueuedPrompt {
                id,
                text,
                images: Vec::new(),
            });
            if !self.state.status.busy() && self.state.turn_id.is_none() && !self.queue_paused {
                self.queue_ready = true;
                if let Err(error) = self.dispatch_queue() {
                    self.state.saved.rich_plans.uncertain_delivery(id);
                    persist_saved(&self.state.saved)?;
                    return Err(error);
                }
            }
        }
        Ok(())
    }
    fn action(&mut self, action: Action) -> Result<bool, String> {
        if self.state.saved.pending_edit.is_some()
            && !matches!(
                action,
                Action::Reconnect
                    | Action::Stop { .. }
                    | Action::Draft(_)
                    | Action::DraftImages(_)
                    | Action::DraftReference(_)
            )
        {
            if let Action::Submit { id, .. }
            | Action::SubmitWithMode { id, .. }
            | Action::ProjectPrompt { id, .. } = &action
            {
                self.state.rejected_submission = Some(*id);
            }
            return Err("An interrupted edit needs recovery. Reconnect before continuing.".into());
        }
        match action {
            Action::Interaction { epoch, action } => {
                if epoch != self.state.interaction_epoch {
                    return Err("This interaction belongs to a previous connection".into());
                }
                return self.action(*action);
            }
            Action::UpdatePlanReview {
                plan_id,
                revision_id,
                review,
            } => {
                self.state
                    .saved
                    .rich_plans
                    .update_review(&plan_id, &revision_id, review)?;
                persist(&self.state.saved)?;
            }
            Action::RetryPlanImport { message_id, source } => {
                crate::rich_plan::retry_import(&mut self.state.saved, &message_id, &source)?;
                persist(&self.state.saved)?;
            }
            Action::SendPlanResponse {
                plan_id,
                revision_id,
            } => self.send_plan_response(&plan_id, &revision_id, persist)?,
            Action::EditMessage { index, message_id } => self.edit_message(index, message_id)?,
            Action::EditMessageInline { index, message_id } => {
                let draft = (
                    self.state.saved.draft.clone(),
                    self.state.saved.draft_images.clone(),
                );
                self.edit_message_with_draft(index, message_id, Some(draft))?;
            }
            Action::PromptInline { text, images } => {
                validate_prompt(&text, &images)?;
                if self.state.status.busy()
                    || self.state.turn_id.is_some()
                    || !self.pending.is_empty()
                {
                    return Err("Wait for the provider before sending the edited message.".into());
                }
                if self.process.is_none() || self.state.saved.thread_id.is_none() {
                    return Err("Reconnect before sending the edited message.".into());
                }
                self.capture_edit_checkpoint();
                self.request(
                    "turn/start",
                    self.turn_params(self.input(&text, images)),
                    Pending::InlineTurn,
                )?;
                self.state.status = Status::Working;
                self.state.clear_error();
            }
            Action::ClearConversation => {
                self.clear_conversation(persist)?;
                if let Err(error) = self.start() {
                    self.state.fail(error);
                    self.pid.store(0, Ordering::SeqCst);
                    self.process = None;
                }
            }
            Action::ResetModelEffort => {
                self.state.model = None;
                self.state.effort = None;
                self.state.saved.composer = ComposerSettings {
                    reset_to_configured: true,
                    reset_model_effort_only: true,
                    access: self.state.access.clone(),
                    ..Default::default()
                };
                self.state.resolved_model =
                    self.configured_turn["model"].as_str().map(str::to_owned);
                self.state.resolved_effort =
                    self.configured_turn["effort"].as_str().map(str::to_owned);
                self.reset_configuration = true;
            }
            Action::ResetConfiguration => {
                self.state.model = None;
                self.state.effort = None;
                self.state.access = None;
                self.state.saved.composer = ComposerSettings {
                    reset_to_configured: true,
                    ..Default::default()
                };
                self.state.resolved_model =
                    self.configured_turn["model"].as_str().map(str::to_owned);
                self.state.resolved_effort =
                    self.configured_turn["effort"].as_str().map(str::to_owned);
                self.state.resolved_access =
                    match self.configured_turn["sandboxPolicy"]["type"].as_str() {
                        Some("dangerFullAccess") => Some("full-access".into()),
                        Some("readOnly") => Some("read-only".into()),
                        Some("workspaceWrite") => Some("workspace-write".into()),
                        _ => None,
                    };
                self.state.sandbox = self.configured_turn.get("sandboxPolicy").cloned();
                self.state.approval_policy = self.configured_turn.get("approvalPolicy").cloned();
                self.reset_configuration = true;
            }
            Action::Configure {
                model,
                effort,
                access,
            } => {
                if let Some(value) = &access {
                    if !["full-access", "read-only", "workspace-write"].contains(&value.as_str()) {
                        return Err("Unknown access mode".into());
                    }
                }
                if let Some(value) = &effort {
                    if ![
                        "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
                    ]
                    .contains(&value.as_str())
                    {
                        return Err("Unknown reasoning effort".into());
                    }
                }
                if let Some(model) = model {
                    // App-server effort overrides survive subsequent turns. Send the new
                    // model's default explicitly so a previous model's effort cannot leak.
                    self.state.effort = self
                        .state
                        .models
                        .iter()
                        .find(|entry| {
                            entry["model"].as_str() == Some(model.as_str())
                                || entry["id"].as_str() == Some(model.as_str())
                        })
                        .and_then(|entry| entry["defaultReasoningEffort"].as_str())
                        .map(str::to_owned);
                    self.state.model = Some(model);
                }
                if let Some(effort) = effort {
                    self.state.effort = Some(effort);
                }
                if let Some(access) = access {
                    self.state.access = Some(access);
                }
                self.state.saved.composer = ComposerSettings {
                    reset_to_configured: self.reset_configuration,
                    reset_model_effort_only: self.state.saved.composer.reset_model_effort_only,
                    model: self.state.model.clone(),
                    effort: self.state.effort.clone(),
                    access: self.state.access.clone(),
                };
            }
            Action::DismissUncertainQueued(id) => {
                self.state.dismiss_uncertain_queued(id)?;
                self.queue_paused = true;
                self.queue_ready = false;
            }
            Action::ResumeQueue => {
                if !self.state.queue_uncertain.is_empty() {
                    return Err(
                        "Inspect and dismiss unconfirmed deliveries before resuming queued inputs"
                            .into(),
                    );
                }
                if self.state.queued_prompts.is_empty() {
                    return Err("There are no queued prompts to send.".into());
                }
                if self.state.status.busy() || self.state.turn_id.is_some() || self.state.compacting
                {
                    return Err("Wait for the current turn before resuming queued inputs".into());
                }
                if self.process.is_none() {
                    return Err("Reconnect before resuming queued inputs".into());
                }
                self.queue_paused = false;
                self.queue_ready = true;
                self.dispatch_queue()?;
            }
            Action::BeginQueuedEdit(id) => self.state.begin_queue_edit(id)?,
            Action::SaveQueuedEdit { id, text, images } => {
                self.state.save_queue_edit(id, text, images)?;
                self.dispatch_queue()?;
            }
            Action::CancelQueuedEdit(id) => {
                if self.state.queue_edit == Some(id) {
                    self.state.queue_edit = None;
                }
                self.dispatch_queue()?;
            }
            Action::RemoveQueued(id) => {
                self.state.remove_queued(id)?;
                self.dispatch_queue()?;
            }
            Action::SteerQueued(id) => {
                if !self.state.queue_editable(id) || self.state.queue_edit == Some(id) {
                    return Err("This queued input cannot be steered right now".into());
                }
                let turn = self
                    .state
                    .turn_id
                    .clone()
                    .filter(|_| self.state.status.busy())
                    .ok_or("No active Codex turn to steer")?;
                let draft = self
                    .state
                    .queued_prompts
                    .iter()
                    .find(|p| p.id == id)
                    .ok_or("This queued input cannot be steered right now")?
                    .clone();
                let input = self.input(&draft.text, draft.images.clone());
                if self.process.is_none() {
                    return Err("Reconnect before steering a queued input".into());
                }
                self.state.queue_sending.push(id);
                self.state.queue_steering.push(id);
                self.request("turn/steer", json!({"threadId":self.state.saved.thread_id,"expectedTurnId":turn,"input":input}), Pending::Steer { submission: id, turn, draft, from_queue: true }).inspect_err(|_| { self.queue_paused = true; self.queue_ready = false; self.state.mark_queue_uncertain(); })?;
                self.state.clear_error();
            }
            Action::ProjectPrompt { id, text } => {
                let draft = self.state.saved.draft.clone();
                let images = self.state.saved.draft_images.clone();
                let reference = self.state.saved.draft_reference.clone();
                let result = self.submit_mode(id, text, Vec::new(), SubmissionMode::Queue);
                self.state.saved.draft = draft;
                self.state.saved.draft_images = images;
                self.state.saved.draft_reference = reference;
                if let Err(error) = result {
                    self.state.rejected_submission = Some(id);
                    return Err(error);
                }
            }
            Action::SubmitWithMode {
                id,
                text,
                images,
                mode,
            } => {
                if let Err(error) = self.submit_mode(id, text, images, mode) {
                    self.state.rejected_submission = Some(id);
                    return Err(error);
                }
            }
            Action::Stop { forget, done } => {
                self.pid.store(0, Ordering::SeqCst);
                self.process = None;
                if forget {
                    if let Ok(dir) = recovery_dir() {
                        let _ =
                            fs_err::remove_file(dir.join(format!("{}.json", self.state.saved.key)));
                    }
                } else {
                    self.save();
                }
                let _ = done.send(());
                return Ok(false);
            }
            Action::Reconnect => {
                if self.state.status.busy() {
                    return Err("Interrupt the current turn before reconnecting".into());
                }
                self.pid.store(0, Ordering::SeqCst);
                self.process = None;
                if let Err(error) = self.start() {
                    self.state.fail(error);
                    self.pid.store(0, Ordering::SeqCst);
                    self.process = None;
                }
            }
            Action::Prompt(text) => self.prompt(text)?,
            Action::Compact => self.compact()?,
            Action::PromptWithImages { text, images } => {
                self.prompt_with_images(text, images, None)?;
            }
            Action::Submit { id, text, images } => {
                if let Err(error) = self.prompt_with_images(text, images, Some(id)) {
                    self.state.rejected_submission = Some(id);
                    return Err(error);
                }
            }
            Action::DraftReference(reference) => {
                if let Some(reference) = &reference {
                    validate_prompt(
                        &reference.serialize(&self.state.saved.draft),
                        &self.state.saved.draft_images,
                    )?;
                }
                self.state.saved.draft_reference = reference;
            }
            Action::DraftImages(images) => {
                validate_images(&images)?;
                if let Some(edit) = &mut self.state.saved.pending_edit {
                    if let Some((_, draft_images)) = &mut edit.composer_draft {
                        draft_images.clone_from(&images);
                    }
                }
                self.state.saved.draft_images = images;
            }
            Action::Draft(text) => {
                if let Some(edit) = &mut self.state.saved.pending_edit {
                    if let Some((draft, _)) = &mut edit.composer_draft {
                        draft.clone_from(&text);
                    }
                }
                self.state.saved.draft = text;
            }
            Action::Metadata { project, roots } => {
                self.state.saved.project = project;
                self.state.saved.roots = roots;
            }
            Action::Interrupt => {
                self.queue_paused = true;
                self.queue_ready = false;
                let turn = self
                    .state
                    .turn_id
                    .clone()
                    .ok_or("No active Codex turn to interrupt")?;
                self.interrupt(turn, 0)?;
                self.state.questions.clear();
                self.state.elicitations.clear();
                self.state.approvals.clear();
            }
            Action::AnswerQuestions(id, answers) => {
                let at = self
                    .state
                    .questions
                    .iter()
                    .position(|q| q.id == id)
                    .ok_or("This question request has expired")?;
                let request = &self.state.questions[at];
                if let Some(answers) = &answers {
                    request.validate(answers)?;
                }
                if request.source == QuestionSource::AsyncMessage {
                    if self.pending.values().any(|pending| matches!(&pending.kind, Pending::AsyncAnswer { request_instance, .. } if *request_instance == request.instance)) {
                        return Err("This answer is already being submitted".into());
                    }
                    if let Some(answers) = answers {
                        let text = request.answer_text(&answers);
                        self.request(
                            "turn/start",
                            self.turn_params(self.input(&text, Vec::new())),
                            Pending::AsyncAnswer {
                                question_id: id.clone(),
                                request_instance: request.instance,
                                ack: self.state.question_answer_ack(id),
                            },
                        )?;
                        self.state.clear_error();
                    } else {
                        self.state.resolve_async_question(&id);
                        self.state.status = if !self.state.questions.is_empty()
                            || !self.state.approvals.is_empty()
                            || !self.state.elicitations.is_empty()
                        {
                            Status::Waiting
                        } else if self.state.turn_id.is_some() {
                            Status::Working
                        } else {
                            Status::Done
                        };
                    }
                    return Ok(true);
                }
                let answered = answers.is_some();
                let response = request.codex_response(answers.unwrap_or_default());
                self.write(&json!({"id":id,"result":response}))?;
                if answered {
                    let ack = self.state.question_answer_ack(id);
                    self.state.question_answer_acks.push(ack);
                }
                let resolved = self.state.questions[at].id.clone();
                self.state.retire_interaction(&resolved);
                self.state.questions.remove(at);
                self.state.status = if self.state.approvals.is_empty()
                    && self.state.questions.is_empty()
                    && self.state.elicitations.is_empty()
                {
                    Status::Working
                } else {
                    Status::Waiting
                };
            }
            Action::Approval(id, accept) => {
                return self.action(Action::RespondApproval {
                    id,
                    choice: if accept { "allow_once" } else { "deny" }.into(),
                    input: None,
                    feedback: None,
                });
            }
            Action::RespondApproval {
                id,
                choice,
                input,
                feedback,
            } => {
                let at = self
                    .state
                    .approvals
                    .iter()
                    .position(|a| a["id"] == id)
                    .ok_or("This approval has expired")?;
                let result = crate::controls::approval_response(
                    &self.state.approvals[at],
                    &choice,
                    input,
                    feedback,
                )?;
                let owner = self.state.interaction_owners.get(&id.to_string()).cloned();
                let child = owner.as_ref().is_some_and(|owner| owner.agent.is_some());
                let permission_cancel = choice == "cancel"
                    && self.state.approvals[at]["method"] == "item/permissions/requestApproval";
                self.write(&json!({"id":id,"result":result}))?;
                let resolved = self.state.approvals[at]["id"].clone();
                self.state.retire_interaction(&resolved);
                self.state.approvals.remove(at);
                if choice == "cancel" && child {
                    if let Some(owner) = &owner {
                        self.state.expire_interactions(&owner.thread);
                    }
                }
                if choice == "cancel" && !child {
                    self.queue_paused = true;
                    self.queue_ready = false;
                    self.state.approvals.clear();
                    self.state.questions.clear();
                    self.state.elicitations.clear();
                }
                if permission_cancel {
                    if child {
                        if let Some(owner) = owner {
                            if let Some(turn) = owner.turn {
                                self.request(
                                    "turn/interrupt",
                                    json!({"threadId":owner.thread,"turnId":turn}),
                                    Pending::ChildInterrupt,
                                )?;
                            }
                        }
                    } else if let Some(turn) = self.state.turn_id.clone() {
                        self.interrupt(turn, 0)?;
                    }
                }
                self.state.status = if self.state.approvals.is_empty()
                    && self.state.questions.is_empty()
                    && self.state.elicitations.is_empty()
                {
                    Status::Working
                } else {
                    Status::Waiting
                };
            }
            Action::RespondElicitation {
                id,
                action,
                content,
            } => {
                let at = self
                    .state
                    .elicitations
                    .iter()
                    .position(|r| r.id == id)
                    .ok_or("This request has expired")?;
                if !matches!(action.as_str(), "accept" | "decline" | "cancel") {
                    return Err("Invalid elicitation action".into());
                }
                if action == "accept" {
                    self.state.elicitations[at]
                        .validate_content(content.as_ref().unwrap_or(&Value::Null))?;
                }
                let content = if action == "accept" {
                    match &self.state.elicitations[at].mode {
                        crate::elicitation::ElicitationMode::Form(form) => Some(
                            form.validate(
                                content
                                    .as_ref()
                                    .and_then(Value::as_object)
                                    .ok_or("Form response must be an object")?,
                            )
                            .map_err(|_| "Invalid form content")?,
                        ),
                        crate::elicitation::ElicitationMode::Url { .. } => None,
                    }
                } else {
                    None
                };
                let result = if action == "accept" && content.is_some() {
                    json!({"action":action,"content":content})
                } else {
                    json!({"action":action})
                };
                self.write(&json!({"id":id,"result":result}))?;
                let resolved = self.state.elicitations[at].id.clone();
                self.state.retire_interaction(&resolved);
                self.state.elicitations.remove(at);
                self.state.status = if self.state.approvals.is_empty()
                    && self.state.questions.is_empty()
                    && self.state.elicitations.is_empty()
                {
                    if self.state.turn_id.is_some() {
                        Status::Working
                    } else {
                        Status::Idle
                    }
                } else {
                    Status::Waiting
                };
            }
        }
        self.state.refresh_interaction_status();
        Ok(true)
    }
    fn interrupt(&mut self, turn: String, attempt: u8) -> Result<(), String> {
        self.request(
            "turn/interrupt",
            json!({"threadId":self.state.saved.thread_id,"turnId":turn}),
            Pending::Interrupt { attempt, turn },
        )
    }
    fn interaction_owner(
        &self,
        p: &Value,
        thread_bound: bool,
    ) -> Option<crate::interaction::InteractionOwner> {
        let thread = p["threadId"].as_str()?;
        if Some(thread) == self.state.saved.thread_id.as_deref() {
            let turn = p["turnId"].as_str();
            if !(thread_bound && turn.is_none())
                && (self.state.turn_id.is_none() || turn != self.state.turn_id.as_deref())
            {
                return None;
            }
            return Some(crate::interaction::InteractionOwner {
                thread: thread.into(),
                turn: turn.map(str::to_owned),
                agent: None,
            });
        }
        if !thread_bound && p["turnId"].as_str().is_none() {
            return None;
        }
        // Provider child approvals and questions belong to one active child
        // turn. Never attach an unscoped request to whichever turn is current.
        p["turnId"].as_str()?;
        self.interaction_children
            .owner(thread, p["turnId"].as_str())
    }
    fn message(&mut self, m: Value) -> Result<(), String> {
        if m.get("method").is_some() {
            if let Some(id) = m.get("id") {
                if self
                    .state
                    .approvals
                    .iter()
                    .any(|approval| approval["id"] == *id)
                {
                    return Ok(());
                }
                if m["method"] == "item/tool/requestUserInput" {
                    if self.state.questions.iter().any(|q| q.id == *id) {
                        return Ok(());
                    }
                    let owner = self.interaction_owner(&m["params"], false);
                    let current = owner.is_some();
                    match QuestionRequest::parse(id.clone(), &m["params"], false) {
                        Ok(request) if current => {
                            if let Some(owner) = owner { self.state.interaction_owners.insert(id.to_string(),owner); }
                            self.state.questions.push(request);
                            self.state.status = Status::Waiting;
                        }
                        _ => self.write(&json!({"id":id,"error":{"code":-32602,"message":"Invalid or expired question request"}}))?,
                    }
                    return Ok(());
                }
                let method = m["method"].as_str().unwrap_or_default();
                let p = &m["params"];
                let owner = self.interaction_owner(p, false);
                let active = owner.is_some();
                match method {
                    "mcpServer/elicitation/request" => {
                        if self.state.elicitations.iter().any(|r|r.id==*id){return Ok(());}
                        let elicitation_owner=self.interaction_owner(p,true);let current=elicitation_owner.is_some();
                        match crate::elicitation::ElicitationRequest::parse_codex(id.clone(),p) {
                            Ok(request) if current => { if let Some(owner)=elicitation_owner { self.state.interaction_owners.insert(id.to_string(),owner); }self.state.elicitations.push(request);self.state.status=Status::Waiting; }
                            _=>self.write(&json!({"id":id,"error":{"code":-32602,"message":"Invalid, unsupported or expired elicitation"}}))?,
                        }
                    }
                    "item/commandExecution/requestApproval"|"item/fileChange/requestApproval"|"item/permissions/requestApproval" if active => {
                        if method=="item/commandExecution/requestApproval" && !p["kind"].is_null() && !matches!(p["kind"].as_str(),Some("command"|"writeStdin")) {
                            self.write(&json!({"id":id,"error":{"code":-32602,"message":"Unknown command approval kind"}}))?;
                        }else{let mut approval=m.clone();if let Some(owner)=owner {if let Some(agent)=&owner.agent {approval["groveAgent"]=json!(agent);}self.state.interaction_owners.insert(id.to_string(),owner); }self.state.approvals.push(approval);self.state.status=Status::Waiting;}
                    }
                    "execCommandApproval"|"applyPatchApproval" if self.state.turn_id.is_some() && p["conversationId"].as_str()==self.state.saved.thread_id.as_deref()=> {
                        self.state.approvals.push(m);self.state.status=Status::Waiting;
                    }
                    "item/tool/call" => self.write(&json!({"id":id,"result":{"success":false,"contentItems":[{"type":"inputText","text":"Grove has no registered executor for this client-side dynamic tool. Configure a supported MCP tool instead."}]}}))?,
                    "account/chatgptAuthTokens/refresh"|"attestation/generate" => {
                        self.write(&json!({"id":id,"error":{"code":-32601,"message":"Grove uses CLI-managed authentication and cannot issue host credentials or attestation. Sign in with the agent CLI and reconnect."}}))?;
                        self.state.error=Some("Codex requested host-managed credentials. Sign in with the Codex CLI and reconnect.".into());
                    }
                    _=>{
                        self.write(&json!({"id":id,"error":{"code":-32601,"message":"Unsupported or expired Grove request"}}))?;
                        self.state.error=Some(format!("Codex requested unsupported interaction: {method}. Interrupt or use a terminal session."));
                    }
                }
            } else {
                let active = self.state.turn_id.clone();
                let completed = m["method"] == "turn/completed"
                    && active.is_some()
                    && m["params"]["turn"]["id"].as_str() == active.as_deref()
                    && m["params"]["threadId"].as_str() == self.state.saved.thread_id.as_deref();
                if let Some(root) = self.state.saved.thread_id.as_deref() {
                    if let Some(expired) = self.interaction_children.codex_event(
                        root,
                        self.state.turn_id.as_deref(),
                        &m,
                    ) {
                        self.state.expire_interactions(&expired);
                    }
                }
                if m["method"] == "serverRequest/resolved" {
                    let id = &m["params"]["requestId"];
                    if self
                        .state
                        .interaction_owners
                        .get(&id.to_string())
                        .is_some_and(|owner| {
                            Some(owner.thread.as_str()) == m["params"]["threadId"].as_str()
                        })
                    {
                        self.state.approvals.retain(|a| a["id"] != *id);
                        self.state.questions.retain(|q| q.id != *id);
                        self.state.elicitations.retain(|r| r.id != *id);
                        self.state.retire_interaction(id);
                    }
                    self.state.refresh_interaction_status();
                    return Ok(());
                }
                if m["params"]["item"]["type"] == "subAgentActivity"
                    && m["params"]["item"]["kind"] == "started"
                {
                    if let Some(id) = m["params"]["item"]["agentThreadId"]
                        .as_str()
                        .filter(|id| self.interaction_children.known(id))
                    {
                        self.state.upsert_subagent(
                            id,
                            m["params"]["item"]["agentPath"]
                                .as_str()
                                .and_then(|path| path.rsplit('/').next()),
                            None,
                            SubagentStatus::Working,
                        );
                    }
                }
                self.state.apply(&m);
                self.state.refresh_interaction_status();
                if completed && m["params"]["turn"]["status"] == "completed" {
                    crate::rich_plan::import_completed(&mut self.state.saved);
                }
                if completed {
                    self.queue_ready = m["params"]["turn"]["status"] == "completed";
                    if !self.queue_ready {
                        self.queue_paused = true;
                    }
                    self.dispatch_queue()?;
                }
                if self.state.status == Status::Failed {
                    self.queue_paused = true;
                    self.queue_ready = false;
                }
            }
            return Ok(());
        }
        let Some(id) = m["id"].as_u64() else {
            return Ok(());
        };
        let Some(request) = self.pending.remove(&id) else {
            return Ok(());
        };
        if let Some(error) = m.get("error") {
            if matches!(request.kind, Pending::ChildInterrupt) {
                self.state.error = Some(
                    error["message"]
                        .as_str()
                        .unwrap_or("Could not interrupt the child turn")
                        .into(),
                );
                return Ok(());
            }
            if let Pending::ChildRead {
                thread, generation, ..
            } = &request.kind
            {
                if self.state.subagent_observation_generation != *generation {
                    return Ok(());
                }
                if let Some(a) = self.state.subagent_activity.get_mut(thread) {
                    a.refreshing = false;
                    a.refresh_error = true;
                }
                let failures = self.child_read_failures.entry(thread.clone()).or_default();
                *failures = failures.saturating_add(1);
                return Ok(());
            }
            // Optional catalogs must never fail or pause an otherwise healthy session.
            if matches!(
                request.kind,
                Pending::Models | Pending::Skills | Pending::ChildRead { .. }
            ) {
                return Ok(());
            }

            if matches!(request.kind, Pending::EditMessage) {
                self.recover_failed_edit()?;
                self.state.error = Some(
                    error["message"]
                        .as_str()
                        .unwrap_or("Codex could not restore the conversation")
                        .into(),
                );
                return Ok(());
            }
            if let Pending::Turn { text, images, .. } = &request.kind {
                // A queued UI clear may arrive before the provider rejects the
                // start. Recover the submitted draft, preserving newer edits
                // and independently owned quote state.
                if self.state.saved.draft.is_empty()
                    && self.state.saved.draft_images.is_empty()
                    && self.state.saved.draft_reference.is_none()
                    && !ReferenceDraft::is_canonical_prompt(text)
                {
                    self.state.saved.draft.clone_from(text);
                    self.state.saved.draft_images.clone_from(images);
                }
            }
            if let Pending::Turn {
                submission: Some(id),
                ..
            } = &request.kind
            {
                self.state.rejected_submission = Some(*id);
            }
            if matches!(request.kind, Pending::Compact) {
                self.state.compact_requested = false;
                self.state.error = Some(
                    error["message"]
                        .as_str()
                        .unwrap_or("Codex could not compact the conversation")
                        .into(),
                );
                return Ok(());
            }
            if let Pending::QueuedTurn { id } = &request.kind {
                let rich = self.state.saved.rich_plans.revisions.iter().any(|r|matches!(r.review.delivery, crate::rich_plan::ResponseDelivery::Queued{id:held} if held==*id));
                if rich {
                    self.state.saved.rich_plans.fail_delivery(
                        *id,
                        error["message"]
                            .as_str()
                            .unwrap_or("Provider rejected plan feedback"),
                    );
                    self.state.queued_prompts.retain(|p| p.id != *id);
                }
                self.state.queue_sending.retain(|sending| sending != id);
            }
            if let Pending::Steer {
                submission,
                from_queue,
                ..
            } = request.kind
            {
                self.state.queue_sending.retain(|id| *id != submission);
                self.state.queue_steering.retain(|id| *id != submission);
                if !from_queue {
                    self.state.queued_prompts.retain(|p| p.id != submission);
                }
                self.queue_paused = true;
                self.queue_ready = false;
                self.state.rejected_submission = Some(submission);
                self.state.error = Some(
                    error["message"]
                        .as_str()
                        .unwrap_or("Codex rejected steering")
                        .into(),
                );
                return Ok(());
            }
            if matches!(request.kind, Pending::AsyncAnswer { .. }) {
                self.state.error = Some(
                    error["message"]
                        .as_str()
                        .unwrap_or("Codex rejected the answer")
                        .into(),
                );
                self.state.status = Status::Waiting;
                return Ok(());
            }
            self.queue_paused = true;
            self.queue_ready = false;
            let message = error["message"]
                .as_str()
                .unwrap_or("Codex request failed")
                .to_owned();
            if let Pending::Interrupt { attempt, turn } = request.kind {
                if attempt < 4
                    && message.contains("no active turn to interrupt")
                    && self.state.turn_id.as_deref() == Some(&turn)
                {
                    self.interrupt_retry = Some((
                        Instant::now() + Duration::from_millis(150),
                        attempt + 1,
                        turn,
                    ));
                    return Ok(());
                }
            }
            self.state.error = Some(message);
            if !matches!(self.state.status, Status::Done | Status::Interrupted) {
                self.state.status = Status::Failed;
            }
            return Ok(());
        }
        let result = &m["result"];
        match request.kind {
            Pending::Initialize => {
                self.write(&json!({"method":"initialized","params":{}}))?;
                self.request("model/list", json!({"limit":100}), Pending::Models)?;
                self.request(
                    "skills/list",
                    json!({"cwds":[self.state.saved.cwd]}),
                    Pending::Skills,
                )?;
                self.request(
                    "config/read",
                    json!({"cwd":self.state.saved.cwd,"includeLayers":false}),
                    Pending::Config,
                )?;
            }
            Pending::Models => {
                self.state
                    .models
                    .extend(result["data"].as_array().cloned().unwrap_or_default());
                if self.state.resolved_model.is_none() {
                    if let Some(model) = self
                        .state
                        .models
                        .iter()
                        .find(|model| model["isDefault"] == true)
                    {
                        self.state.resolved_model = model["model"].as_str().map(str::to_owned);
                        self.state.resolved_effort =
                            model["defaultReasoningEffort"].as_str().map(str::to_owned);
                    }
                }
                if let Some(cursor) = result["nextCursor"].as_str() {
                    self.request(
                        "model/list",
                        json!({"limit":100,"cursor":cursor}),
                        Pending::Models,
                    )?;
                }
            }
            Pending::Skills => {
                self.state.skills = result["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| entry["skills"].as_array())
                    .flatten()
                    .cloned()
                    .collect();
            }
            Pending::Config => {
                result["config"]["developer_instructions"]
                    .as_str()
                    .unwrap_or_default()
                    .clone_into(&mut self.developer_instructions);
                if let Some(model) = result["config"]["model"].as_str() {
                    self.state.resolved_model = Some(model.into());
                }
                if let Some(effort) = result["config"]["model_reasoning_effort"].as_str() {
                    self.state.resolved_effort = Some(effort.into());
                }
                if let Some(model) = &self.state.resolved_model {
                    self.configured_turn["model"] = json!(model);
                }
                if let Some(effort) = &self.state.resolved_effort {
                    self.configured_turn["effort"] = json!(effort);
                }

                if let Some(policy) = result["config"].get("approval_policy") {
                    if !policy.is_null() {
                        self.configured_turn["approvalPolicy"] = policy.clone();
                    }
                }
                let configured_sandbox = match result["config"]["sandbox_mode"].as_str() {
                    Some("read-only") => Some(json!({"type":"readOnly"})),
                    Some("danger-full-access") => Some(json!({"type":"dangerFullAccess"})),
                    Some("workspace-write") => Some(json!({"type":"workspaceWrite",
                        "writableRoots":self.state.saved.roots.iter().map(|root| &root.wt_path).collect::<Vec<_>>(),
                        "networkAccess":result["config"]["sandbox_workspace_write"]["network_access"].as_bool().unwrap_or(false),
                        "excludeTmpdirEnvVar":result["config"]["sandbox_workspace_write"]["exclude_tmpdir_env_var"].as_bool().unwrap_or(false),
                        "excludeSlashTmp":result["config"]["sandbox_workspace_write"]["exclude_slash_tmp"].as_bool().unwrap_or(false)})),
                    _ => None,
                };
                if let Some(policy) = configured_sandbox {
                    self.configured_turn["sandboxPolicy"] = policy;
                }
                let mut roots = result["config"]["sandbox_workspace_write"]["writable_roots"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                for root in &self.state.saved.roots {
                    if root.wt_path != self.state.saved.cwd
                        && !roots
                            .iter()
                            .any(|value| value.as_str() == Some(&root.wt_path))
                    {
                        roots.push(json!(root.wt_path));
                    }
                }
                if self.configured_turn["sandboxPolicy"]["type"] == "workspaceWrite" {
                    self.configured_turn["sandboxPolicy"]["writableRoots"] = json!(roots);
                }
                self.start_thread(Some(roots))?;
            }
            Pending::Thread => {
                if let Some(model) = result["model"].as_str() {
                    self.state.resolved_model = Some(model.into());
                    if self.configured_turn.get("model").is_none() {
                        self.configured_turn["model"] = json!(model);
                    }
                }
                if let Some(effort) = result["reasoningEffort"].as_str() {
                    self.state.resolved_effort = Some(effort.into());
                    if self.configured_turn.get("effort").is_none() {
                        self.configured_turn["effort"] = json!(effort);
                    }
                }
                self.state.resolved_access = match result["sandbox"]["type"].as_str() {
                    Some("dangerFullAccess") => Some("full-access".into()),
                    Some("readOnly") => Some("read-only".into()),
                    Some("workspaceWrite") => Some("workspace-write".into()),
                    _ => None,
                };
                if let Some(sandbox) = result.get("sandbox") {
                    if !sandbox.is_null() {
                        self.configured_turn["sandboxPolicy"] = sandbox.clone();
                    }
                }
                if let Some(approval) = result.get("approvalPolicy") {
                    if !approval.is_null() {
                        self.configured_turn["approvalPolicy"] = approval.clone();
                    }
                }
                self.state.sandbox = result.get("sandbox").cloned();
                self.state.approval_policy = result.get("approvalPolicy").cloned();
                if result["sandbox"]["type"] == "workspaceWrite" {
                    for root in &self.state.saved.roots {
                        if root.wt_path != self.state.saved.cwd
                            && !result["sandbox"]["writableRoots"]
                                .as_array()
                                .is_some_and(|roots| {
                                    roots
                                        .iter()
                                        .any(|value| value.as_str() == Some(&root.wt_path))
                                })
                        {
                            return Err(format!(
                                "Codex did not honor writable context root {}. No prompt was submitted.",
                                root.wt_path
                            ));
                        }
                    }
                }
                self.state.edit_supported = result["thread"]["historyMode"] == "paginated";
                self.state.saved.thread_id = result["thread"]["id"].as_str().map(str::to_owned);
                if self.state.saved.thread_id.is_none() {
                    return Err("Codex returned no thread identity".into());
                }
                if self.state.saved.pending_edit.is_some() {
                    self.request("thread/turns/list", json!({"threadId":self.state.saved.thread_id,"itemsView":"notLoaded","limit":100,"sortDirection":"desc"}), Pending::RecoverEdit)?;
                    self.state.status = Status::Starting;
                    return Ok(());
                }
                if let Some(turns) = result["thread"]["turns"].as_array() {
                    let items = turns
                        .iter()
                        .filter_map(|t| t["items"].as_array())
                        .flatten()
                        .cloned()
                        .collect::<Vec<_>>();
                    if !items.is_empty() {
                        // History carries no Grove metadata; keep known compaction figures.
                        let items = items
                            .into_iter()
                            .map(|mut item| {
                                if let Some(old) = self
                                    .state
                                    .saved
                                    .items
                                    .iter()
                                    .find(|old| old["id"] == item["id"])
                                {
                                    stamp_message_received(&mut item, Some(old));
                                    for marker in ["groveQuestionResolved", "groveQuestionPending"]
                                    {
                                        if old[marker] == true {
                                            item[marker] = json!(true);
                                        }
                                    }
                                }
                                if item["type"] != COMPACTION_ITEM {
                                    return item;
                                }
                                let mut known = self
                                    .state
                                    .saved
                                    .items
                                    .iter()
                                    .find(|old| old["id"] == item["id"])
                                    .cloned()
                                    .unwrap_or(item);
                                known["status"] = json!("completed");
                                known
                            })
                            .collect();
                        // Transcript offsets remain valid only when restored items keep
                        // the saved prefix. A provider rewrite must not misattribute timing.
                        let items: Vec<Value> = items;
                        if self.state.saved.items.len() > items.len()
                            || self
                                .state
                                .saved
                                .items
                                .iter()
                                .zip(&items)
                                .any(|(old, new)| old["id"] != new["id"])
                        {
                            self.state.saved.work_periods.clear();
                            self.state.saved.edit_checkpoints.clear();
                            self.state.work_started = None;
                        }
                        self.state.saved.items = items;
                    }
                }
                self.state
                    .set_provider_title(result["thread"]["name"].as_str());
                self.state.status = Status::Idle;
                if let Some(turns) = result["thread"]["turns"].as_array() {
                    if let Some(turn) = turns
                        .iter()
                        .rev()
                        .find(|turn| turn["status"] == "inProgress")
                    {
                        if result["thread"]["status"]["type"] == "active" {
                            self.state.turn_id = turn["id"].as_str().map(str::to_owned);
                            self.state.status = Status::Working;
                        } else {
                            self.state.turn_id = None;
                            self.state.status = Status::Interrupted;
                            self.state.error = Some("The previous turn stopped when Codex disconnected. Inspect the conversation before sending another prompt.".into());
                        }
                    }
                }
                self.state.restore_async_questions();
                if let Some(prompt) = self.initial.take() {
                    self.prompt(prompt)?;
                }
            }
            Pending::AsyncAnswer {
                question_id,
                request_instance,
                mut ack,
            } => {
                // A cancelled or expired request cannot become an answered receipt.
                if self
                    .state
                    .questions
                    .iter()
                    .any(|request| request.instance == request_instance)
                {
                    ack.sequence = self
                        .state
                        .question_answer_acks
                        .last()
                        .map_or(1, |old| old.sequence + 1);
                    self.state.question_answer_acks.push(ack);
                    self.state.resolve_async_question(&question_id);
                }
                if let Some(turn) = result["turn"]["id"]
                    .as_str()
                    .filter(|id| !self.state.settled_turns.iter().any(|old| old == id))
                {
                    self.state.turn_id = Some(turn.to_owned());
                }
                self.state.status = if !self.state.questions.is_empty()
                    || !self.state.approvals.is_empty()
                    || !self.state.elicitations.is_empty()
                {
                    Status::Waiting
                } else if self.state.turn_id.is_some() {
                    Status::Working
                } else {
                    Status::Done
                };
            }
            Pending::RecoverEdit => {
                let edit = self
                    .state
                    .saved
                    .pending_edit
                    .clone()
                    .ok_or("Missing edit recovery journal")?;
                let turns = result["data"].as_array().ok_or(
                    "Codex returned no history page for edit recovery; reconnect to retry.",
                )?;
                if turns
                    .iter()
                    .any(|turn| turn["id"].as_str() == Some(&edit.turn_id))
                {
                    self.recover_failed_edit()?;
                    self.state.error = Some("The interrupted edit was cancelled; the previous workspace and conversation were recovered.".into());
                } else if let Some(cursor) = result["nextCursor"].as_str() {
                    self.request("thread/turns/list", json!({"threadId":self.state.saved.thread_id,"itemsView":"notLoaded","limit":100,"sortDirection":"desc","cursor":cursor}), Pending::RecoverEdit)?;
                } else {
                    let record = self
                        .state
                        .saved
                        .edit_checkpoints
                        .iter()
                        .find(|record| record.turn_id.as_deref() == Some(&edit.turn_id))
                        .ok_or("The edit checkpoint is missing; recovery remains blocked.")?;
                    record.files.restore()?;
                    self.finish_edit()?;
                    self.state.error = Some(
                        "The interrupted edit completed; the restored message is ready to edit."
                            .into(),
                    );
                }
            }
            Pending::EditMessage => {
                if result["thread"]["id"].as_str() != self.state.saved.thread_id.as_deref() {
                    return Err("Codex returned an unexpected edit acknowledgement. Reconnect to reconcile the conversation; the workspace recovery was preserved.".into());
                }
                self.finish_edit()?;
            }
            Pending::InlineTurn => {
                self.bind_edit_checkpoint(result);
                self.state.accepted_prompts = self.state.accepted_prompts.saturating_add(1);
                if self.state.status == Status::Working {
                    self.state.turn_id = result["turn"]["id"].as_str().map(str::to_owned);
                }
            }
            Pending::Turn {
                submission,
                text,
                images,
            } => {
                self.bind_edit_checkpoint(result);
                if let Some(submission) = submission {
                    self.state.accepted_submission = Some(submission);
                }
                self.state.accepted_prompts = self.state.accepted_prompts.saturating_add(1);
                if self.state.saved.title.is_none() {
                    self.state.saved.title = prompt_title(&text);
                }
                self.state.saved.clear_submitted_draft(&text, &images);
                if self.state.status == Status::Working {
                    self.state.turn_id = result["turn"]["id"].as_str().map(str::to_owned);
                }
            }
            Pending::Steer {
                submission,
                turn,
                draft,
                from_queue,
            } => {
                if result["turnId"].as_str() != Some(turn.as_str()) {
                    self.state.mark_queue_uncertain();
                    self.queue_paused = true;
                    self.queue_ready = false;
                    self.state.rejected_submission = Some(submission);
                    self.state.error = Some("Codex returned an unexpected steering turn".into());
                } else {
                    if !from_queue {
                        self.state
                            .saved
                            .clear_submitted_draft(&draft.text, &draft.images);
                        self.state.accepted_submission = Some(submission);
                    }
                    self.state.queued_prompts.retain(|p| p.id != submission);
                    self.state.queue_sending.retain(|id| *id != submission);
                    self.state.queue_steering.retain(|id| *id != submission);
                    self.state.queue_uncertain.retain(|id| *id != submission);
                }
            }
            Pending::QueuedTurn { id } => {
                self.bind_edit_checkpoint(result);
                let Some(turn) = result["turn"]["id"].as_str().filter(|id| !id.is_empty()) else {
                    self.queue_paused = true;
                    self.queue_ready = false;
                    self.state.mark_queue_uncertain();
                    self.state.error = Some("Codex returned no queued turn identity; inspect the conversation before resuming".into());
                    if self.state.turn_id.is_none() {
                        self.state.status = Status::Failed;
                    }
                    return Ok(());
                };
                if self
                    .state
                    .turn_id
                    .as_deref()
                    .is_some_and(|active| active != turn)
                {
                    self.queue_paused = true;
                    self.queue_ready = false;
                    self.state.mark_queue_uncertain();
                    self.state.error =
                        Some("Codex returned an unexpected queued turn identity".into());
                    return Ok(());
                }
                self.state.saved.rich_plans.confirm_delivery(id);
                self.state.queued_prompts.retain(|p| p.id != id);
                self.state.queue_sending.retain(|sending| *sending != id);
                self.state.queue_uncertain.retain(|sending| *sending != id);
                if self.state.status == Status::Working {
                    self.state.turn_id = result["turn"]["id"]
                        .as_str()
                        .filter(|id| !self.state.settled_turns.iter().any(|old| old == id))
                        .map(str::to_owned);
                }
            }
            Pending::ChildRead {
                thread,
                period_start,
                generation,
            } => {
                if self.state.subagent_observation_generation != generation {
                    return Ok(());
                }
                if let Some(a) = self.state.subagent_activity.get_mut(&thread) {
                    a.refreshing = false;
                    a.refresh_error = false;
                }
                self.child_read_failures.remove(&thread);
                self.state
                    .apply_subagent_read(&thread, period_start, &result["thread"]);
            }
            Pending::Interrupt { .. } | Pending::Compact | Pending::ChildInterrupt => {}
        }
        self.dispatch_queue()?;
        Ok(())
    }
    fn expire_subagent_reads(&mut self) {
        let expired: Vec<_> = self
            .pending
            .iter()
            .filter_map(|(id, request)| match &request.kind {
                Pending::ChildRead { thread, .. } if request.deadline < Instant::now() => Some((
                    *id,
                    thread.clone(),
                    match &request.kind {
                        Pending::ChildRead { generation, .. } => *generation,
                        _ => 0,
                    },
                )),
                _ => None,
            })
            .collect();
        for (id, thread, generation) in expired {
            self.pending.remove(&id);
            if self.state.subagent_observation_generation != generation {
                continue;
            }
            if let Some(a) = self.state.subagent_activity.get_mut(&thread) {
                a.refreshing = false;
                a.refresh_error = true;
            }
            let failures = self.child_read_failures.entry(thread).or_default();
            *failures = failures.saturating_add(1);
        }
    }
    fn poll_subagents(&mut self) {
        if self.process.is_none() || self.last_child_poll.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_child_poll = Instant::now();
        // Optional reads must never hold up the parent or accumulate timed-out requests.

        let children: Vec<_> = self
            .state
            .saved
            .work_periods
            .iter()
            .flat_map(|period| {
                period
                    .subagents
                    .iter()
                    .filter(|agent| {
                        matches!(
                            agent.status,
                            SubagentStatus::Working | SubagentStatus::Unknown
                        )
                    })
                    .map(|agent| (agent.id.clone(), period.start_item))
            })
            .collect();
        for (thread, period_start) in children {
            if thread.len() > 1024 {
                continue;
            }
            if self.child_read_failures.get(&thread).copied().unwrap_or(0) >= 3 {
                continue;
            }
            if self
                .state
                .subagent_period(&thread)
                .is_none_or(|index| self.state.saved.work_periods[index].start_item != period_start)
            {
                continue;
            }
            if self.pending.values().any(|request| matches!(&request.kind, Pending::ChildRead { thread: pending, .. } if pending == &thread)) { continue; }
            let generation = self.state.subagent_observation_generation;
            self.state.ensure_subagent_activity(&thread);
            let Some(activity) = self.state.subagent_activity.get_mut(&thread) else {
                continue;
            };
            activity.refreshing = true;
            let _ = self.request(
                "thread/read",
                json!({"threadId":thread,"includeTurns":true}),
                Pending::ChildRead {
                    thread,
                    period_start,
                    generation,
                },
            );
        }
    }
    fn reject_pending_submission(&mut self) {
        self.state.mark_queue_uncertain();
        self.queue_paused = true;
        self.queue_ready = false;
        if let Some(id) = self
            .pending
            .values()
            .find_map(|request| match request.kind {
                Pending::Turn {
                    submission: Some(id),
                    ..
                }
                | Pending::Steer { submission: id, .. } => Some(id),
                _ => None,
            })
        {
            self.state.rejected_submission = Some(id);
        }
    }
    fn run(&mut self) {
        if let Err(e) = self.start() {
            self.state.fail(e);
        }
        self.emit();
        loop {
            match self.actions.recv_timeout(Duration::from_millis(40)) {
                Ok(action) => {
                    match self.action(action) {
                        Ok(false) => break,
                        Ok(true) => {}
                        Err(e) => self.state.error = Some(e),
                    }
                    self.dirty = true;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.pid.store(0, Ordering::SeqCst);
                    self.process = None;
                    self.save();
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            loop {
                let message = self.process.as_ref().and_then(|p| p.lines.try_recv().ok());
                let Some(message) = message else {
                    break;
                };
                let outcome = message.and_then(|m| self.message(m));
                if let Err(e) = outcome {
                    self.reject_pending_submission();
                    self.state.fail(e);
                    self.pid.store(0, Ordering::SeqCst);
                    self.process = None;
                    self.pending.clear();
                    self.interrupt_retry = None;
                }
                self.dirty = true;
            }
            if self.title_generator.tick(
                &mut self.state.saved,
                self.state.status,
                self.stopping.load(Ordering::SeqCst),
            ) {
                self.dirty = true;
            }
            self.expire_subagent_reads();
            self.poll_subagents();
            if let Some((when, attempt, turn)) = self.interrupt_retry.clone() {
                if Instant::now() >= when {
                    self.interrupt_retry = None;
                    if self.state.turn_id.as_deref() == Some(&turn) {
                        if let Err(e) = self.interrupt(turn, attempt) {
                            self.state.error = Some(e);
                        }
                        self.dirty = true;
                    }
                }
            }
            let expired_steers: Vec<_> = self
                .pending
                .iter()
                .filter_map(|(id, r)| {
                    if Instant::now() > r.deadline && matches!(r.kind, Pending::Steer { .. }) {
                        Some(*id)
                    } else {
                        None
                    }
                })
                .collect();
            for id in expired_steers {
                if let Some(Request {
                    kind: Pending::Steer { submission, .. },
                    ..
                }) = self.pending.remove(&id)
                {
                    self.queue_paused = true;
                    self.queue_ready = false;
                    self.state.rejected_submission = Some(submission);
                    self.state.mark_queue_uncertain();
                    self.state.error = Some("Steering acknowledgement timed out; inspect the conversation before retrying".into());
                    self.dirty = true;
                }
            }
            self.pending.retain(|_, request| {
                !(Instant::now() > request.deadline
                    && matches!(request.kind, Pending::Models | Pending::Skills))
            });
            if self.pending.values().any(|r| Instant::now() > r.deadline) {
                self.reject_pending_submission();
                self.state.fail("Codex request timed out. Reconnect to recover the thread; the failed request is never replayed automatically.".into());
                self.pid.store(0, Ordering::SeqCst);
                self.process = None;
                self.pending.clear();
                self.dirty = true;
            }
            if self.dirty {
                self.emit();
            }
            if self.needs_save && self.last_save.elapsed() >= Duration::from_millis(250) {
                self.save();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn answer_echo_text_is_only_for_nonsecret_async_questions() {
        let params = serde_json::json!({"questions":[{"id":"scope", "question":"Which scope?"}]});
        let mut request =
            super::QuestionRequest::from_codex(serde_json::json!("ask"), &params).unwrap();
        let answers = std::collections::HashMap::from([("scope".into(), vec!["All".into()])]);
        assert_eq!(request.answer_echo_text(&answers), None);
        request.source = super::QuestionSource::AsyncMessage;
        assert_eq!(
            request.answer_echo_text(&answers).as_deref(),
            Some("Which scope?\nAll")
        );
        request.questions[0].secret = true;
        assert_eq!(request.answer_echo_text(&answers), None);
    }

    use super::*;
    fn saved() -> SavedSession {
        SavedSession {
            rich_plans: crate::rich_plan::PlanStore::default(),
            claude_resume: None,
            edit_checkpoints: Vec::new(),
            pending_edit: None,
            provider: Provider::Codex,
            key: "test-native-1".into(),
            project: "Project".into(),
            cwd: "/unused".into(),
            label: "Codex".into(),
            title: None,
            title_generation: None,
            roots: Vec::new(),
            thread_id: Some("thread-1".into()),
            items: Vec::new(),
            work_periods: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
            composer: ComposerSettings::default(),
            context: None,
        }
    }
    fn reference(id: &str) -> ReferenceDraft {
        ReferenceDraft {
            id: id.into(),
            thread_id: Some("thread-1".into()),
            sources: vec![ReferenceSource {
                message_id: "message-1".into(),
                role: "assistant".into(),
            }],
            text: "/compact $skill literal\n```rust\n<User reply:>\n```".into(),
        }
    }
    #[test]
    fn reference_draft_roundtrip_legacy_and_acceptance_identity() {
        let mut saved = saved();
        let mut legacy = serde_json::to_value(&saved).unwrap();
        legacy.as_object_mut().unwrap().remove("draft_reference");
        assert!(serde_json::from_value::<SavedSession>(legacy)
            .unwrap()
            .draft_reference
            .is_none());
        saved.draft = "explain".into();
        saved.draft_reference = Some(reference("old"));
        let canonical = saved
            .draft_reference
            .as_ref()
            .unwrap()
            .serialize(&saved.draft);
        let encoded = canonical.lines().nth(1).unwrap();
        assert_eq!(
            serde_json::from_str::<ReferenceDraft>(encoded).unwrap(),
            reference("old")
        );
        let restored: SavedSession =
            serde_json::from_value(serde_json::to_value(&saved).unwrap()).unwrap();
        assert_eq!(restored.draft_reference, saved.draft_reference);
        saved.draft_reference = Some(reference("new"));
        saved.clear_submitted_draft(&canonical, &[]);
        assert_eq!(saved.draft, "explain");
        assert_eq!(saved.draft_reference, Some(reference("new")));
        let canonical = saved
            .draft_reference
            .as_ref()
            .unwrap()
            .serialize(&saved.draft);
        saved.clear_submitted_draft(&canonical, &[]);
        assert!(saved.draft.is_empty());
        assert!(saved.draft_reference.is_none());
        saved.draft_reference = Some(reference("fresh"));
        assert!(saved.fresh_conversation().draft_reference.is_none());
    }
    #[test]
    #[cfg(unix)]
    fn reference_codex_dispatch_literal_once_and_late_acceptance_preserves_replacement() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        let mut actor = actor();
        let mut child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let input = child.stdin.take().unwrap();
        let (_, lines) = mpsc::channel();
        actor.process = Some(Process {
            child,
            input,
            lines,
        });
        actor.state.status = Status::Idle;
        actor.state.turn_id = None;
        actor.state.saved.draft = "explain".into();
        actor.state.saved.draft_reference = Some(reference("old"));
        actor.state.skills = vec![json!({"name":"skill","path":"/tmp/skill"})];
        let canonical = reference("old").serialize("explain");
        actor
            .action(Action::Submit {
                id: 91,
                text: canonical.clone(),
                images: vec![],
            })
            .unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let sent: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(sent["params"]["input"][0]["text"], canonical);
        assert_eq!(sent["params"]["input"].as_array().unwrap().len(), 1);
        assert_eq!(actor.state.saved.draft, "explain");
        actor
            .action(Action::DraftReference(Some(reference("new"))))
            .unwrap();
        actor
            .message(json!({"id":1,"result":{"turn":{"id":"turn-new"}}}))
            .unwrap();
        assert_eq!(actor.state.accepted_submission, Some(91));
        assert_eq!(actor.state.saved.draft_reference, Some(reference("new")));
        assert_eq!(actor.state.saved.draft, "explain");
    }
    #[test]
    #[cfg(unix)]
    fn reference_attached_after_plain_submit_preserves_newer_draft_on_acceptance() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.state.status = Status::Idle;
        actor.state.turn_id = None;
        actor.state.saved.draft = "original plain message".into();
        actor
            .action(Action::Submit {
                id: 91,
                text: "original plain message".into(),
                images: vec![],
            })
            .unwrap();
        actor
            .action(Action::Draft("new unsent text".into()))
            .unwrap();
        let images = vec!["data:image/png;base64,aGVsbG8=".into()];
        actor.action(Action::DraftImages(images.clone())).unwrap();
        actor
            .action(Action::DraftReference(Some(reference("new"))))
            .unwrap();
        actor
            .message(json!({"id":1,"result":{"turn":{"id":"accepted-turn"}}}))
            .unwrap();
        assert_eq!(actor.state.accepted_submission, Some(91));
        assert_eq!(actor.state.saved.draft, "new unsent text");
        assert_eq!(actor.state.saved.draft_images, images);
        assert_eq!(actor.state.saved.draft_reference, Some(reference("new")));
    }
    #[test]
    #[cfg(unix)]
    fn reference_attached_after_plain_submit_survives_rejection_without_restoring_old_text() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.state.status = Status::Idle;
        actor.state.turn_id = None;
        actor
            .action(Action::Submit {
                id: 91,
                text: "old submitted text".into(),
                images: vec![],
            })
            .unwrap();
        actor.action(Action::Draft(String::new())).unwrap();
        actor
            .action(Action::DraftReference(Some(reference("new"))))
            .unwrap();
        actor
            .message(json!({"id":1,"error":{"message":"rejected"}}))
            .unwrap();
        assert_eq!(actor.state.rejected_submission, Some(91));
        assert!(actor.state.saved.draft.is_empty());
        assert_eq!(actor.state.saved.draft_reference, Some(reference("new")));
    }
    #[test]
    #[cfg(unix)]
    fn reference_prefix_looking_ordinary_prompt_restores_after_rejection() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.state.status = Status::Idle;
        actor.state.turn_id = None;
        let text = "Referenced transcript selection (quoted data):\nnot JSON\n\nUser reply:\nexplain this format";
        actor
            .action(Action::Submit {
                id: 91,
                text: text.into(),
                images: vec![],
            })
            .unwrap();
        actor.action(Action::Draft(String::new())).unwrap();
        actor
            .message(json!({"id":1,"error":{"message":"rejected"}}))
            .unwrap();
        assert_eq!(actor.state.saved.draft, text);
        assert!(actor.state.saved.draft_reference.is_none());
        assert!(ReferenceDraft::is_canonical_prompt(
            &reference("valid").serialize("reply")
        ));
        let mut invalid = reference("");
        assert!(!ReferenceDraft::is_canonical_prompt(
            &invalid.serialize("reply")
        ));
        invalid.id = "valid".into();
        invalid.sources.clear();
        assert!(!ReferenceDraft::is_canonical_prompt(
            &invalid.serialize("reply")
        ));
    }
    #[test]
    #[cfg(unix)]
    fn reference_queue_and_steer_snapshots_are_immutable_and_rejections_retain_draft() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.state.saved.draft_reference = Some(reference("old"));
        let canonical = reference("old").serialize("");
        actor
            .submit_mode(91, canonical.clone(), vec![], SubmissionMode::Steer)
            .unwrap();
        actor
            .message(json!({"id":1,"error":{"message":"rejected"}}))
            .unwrap();
        assert_eq!(actor.state.saved.draft_reference, Some(reference("old")));
        actor
            .submit_mode(92, canonical.clone(), vec![], SubmissionMode::Queue)
            .unwrap();
        assert!(actor.state.saved.draft_reference.is_none());
        assert_eq!(actor.state.queued_prompts.last().unwrap().text, canonical);
        actor
            .action(Action::DraftReference(Some(reference("new"))))
            .unwrap();
        assert_eq!(
            actor.state.queued_prompts.last().unwrap().text,
            reference("old").serialize("")
        );
    }
    #[test]
    fn fresh_yolo_options_survive_saved_session_resume_for_both_providers() {
        for provider in [Provider::Codex, Provider::Claude] {
            let args = provider.agent().launch_args_for_backend(true, true, false);
            let options = if provider == Provider::Codex {
                launch_options(&args).unwrap().0
            } else {
                crate::claude::launch_options(&args).unwrap().0
            };
            assert!(!options.is_empty());
            let mut session = saved();
            session.provider = provider;
            session.native_config_version = 1;
            session.config_args = options.clone();
            session.composer.access = Some(
                if provider == Provider::Codex {
                    "full-access"
                } else {
                    "bypassPermissions"
                }
                .into(),
            );
            let mut recovered = recover_saved_for_test(&session);
            recovered.migrate_native_config();
            assert_eq!(recovered.config_args, options);
            assert_eq!(recovered.composer.access, session.composer.access);
        }
    }
    #[test]
    fn legacy_native_bypass_migration_preserves_explicit_access_and_custom_options() {
        for provider in [Provider::Codex, Provider::Claude] {
            let mut legacy = serde_json::to_value(saved()).unwrap();
            legacy
                .as_object_mut()
                .unwrap()
                .remove("native_config_version");
            let mut session: SavedSession = serde_json::from_value(legacy).unwrap();
            session.provider = provider;
            session.composer.access = Some("full-access".into());
            session.config_args = if provider == Provider::Codex {
                vec![
                    "-c",
                    "approval_policy=\"never\"",
                    "-c",
                    "sandbox_mode=\"danger-full-access\"",
                    "-c",
                    "model=\"custom\"",
                ]
            } else {
                vec![
                    "--dangerously-skip-permissions",
                    "--chrome",
                    "--allowedTools",
                    "Read",
                ]
            }
            .into_iter()
            .map(str::to_owned)
            .collect();
            let explicit = session.config_args.clone();
            let mut native = session.clone();
            native.native_config_version = 1;
            native.migrate_native_config();
            assert_eq!(native.config_args, explicit);
            session.migrate_native_config();
            assert_eq!(session.composer.access.as_deref(), Some("full-access"));
            assert_eq!(
                session.config_args,
                if provider == Provider::Codex {
                    vec!["-c", "model=\"custom\""]
                } else {
                    vec!["--chrome", "--allowedTools", "Read"]
                }
            );
            let mut recovered: SavedSession =
                serde_json::from_value(serde_json::to_value(&session).unwrap()).unwrap();
            recovered.migrate_native_config();
            assert_eq!(recovered.config_args, session.config_args);
            assert_eq!(recovered.native_config_version, 1);
        }
        let mut custom = saved();
        custom.native_config_version = 0;
        custom.config_args = vec!["-c".into(), "approval_policy=\"never\"".into()];
        let expected = custom.config_args.clone();
        custom.migrate_native_config();
        assert_eq!(custom.config_args, expected);
    }
    #[test]
    fn effective_codex_access_separates_sandbox_approvals_and_preserves_richer_reset() {
        let mut actor = actor();
        let sandbox = json!({"type":"workspaceWrite","writableRoots":["/custom"],"networkAccess":false,"excludeSlashTmp":true,"readOnlyAccess":{"type":"restricted","readableRoots":["/read"]}});
        actor.state.sandbox = Some(json!({"type":"dangerFullAccess"}));
        actor.state.approval_policy = Some(json!("on-request"));
        assert_eq!(
            actor.state.configured_access_summary(),
            "No sandbox · ask on request"
        );
        actor.configured_turn =
            json!({"sandboxPolicy":sandbox,"approvalPolicy":{"reject":{"sandbox_approval":true}}});
        actor
            .action(Action::Configure {
                model: None,
                effort: None,
                access: Some("full-access".into()),
            })
            .unwrap();
        actor.action(Action::ResetConfiguration).unwrap();
        assert_eq!(actor.turn_params(vec![])["sandboxPolicy"], sandbox);
        assert_eq!(actor.state.sandbox, Some(sandbox));
        assert_eq!(
            actor.state.approval_policy,
            Some(json!({"reject":{"sandbox_approval":true}}))
        );
        assert_eq!(
            actor.state.configured_access_summary(),
            "Workspace access · configured approvals"
        );
    }
    #[test]
    fn work_periods_follow_turn_boundaries_and_recover_without_inventing_time() {
        let mut state = state();
        let start = json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}});
        state.apply(&start);
        state.apply(&start);
        assert_eq!(state.saved.work_periods.len(), 1);
        state.work_started = Some(Instant::now().checked_sub(Duration::from_secs(3)).unwrap());
        state
            .saved
            .items
            .push(json!({"id":"answer", "type":"agentMessage", "text":"Done"}));
        state.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"other","status":"completed"}}}));
        assert!(state.work_started.is_some());
        state.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"interrupted"}}}));
        let period = &state.saved.work_periods[0];
        assert_eq!(period.start_item, 0);
        assert_eq!(period.end_item, Some(1));
        assert!(period.duration_ms.unwrap() >= 3000);
        assert!(state.work_started.is_none());
        let mut saved = state.saved;
        saved.work_periods.push(WorkPeriod {
            start_item: 1,
            end_item: None,
            duration_ms: None,
            subagents: Vec::new(),
        });
        let recovered = Snapshot::new(saved);
        assert_eq!(recovered.saved.work_periods[1].end_item, Some(1));
        assert_eq!(recovered.saved.work_periods[1].duration_ms, None);
        assert!(recovered.work_started.is_none());
        assert!(recovered.saved.fresh_conversation().work_periods.is_empty());
    }

    #[test]
    fn active_subagents_keep_latest_observations_and_terminal_tombstones() {
        let agent = |id: &str, status: SubagentStatus| Subagent {
            id: id.into(),
            name: id.into(),
            role: None,
            task: None,
            activity: "Observed".into(),
            status,
        };
        let periods = vec![
            WorkPeriod {
                start_item: 0,
                end_item: Some(1),
                duration_ms: None,
                subagents: vec![
                    agent("done", SubagentStatus::Working),
                    agent("live", SubagentStatus::Working),
                ],
            },
            WorkPeriod {
                start_item: 1,
                end_item: Some(2),
                duration_ms: None,
                subagents: vec![
                    agent("done", SubagentStatus::Completed),
                    agent("live", SubagentStatus::Unknown),
                    agent("failed", SubagentStatus::Failed),
                ],
            },
        ];
        let active = active_subagents(&periods);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "live");
        assert_eq!(active[0].status, SubagentStatus::Unknown);
        let restored: Subagent = serde_json::from_value(
            json!({"id":"old", "name":"Agent 1", "activity":"Reading", "status":"working"}),
        )
        .unwrap();
        assert!(restored.role.is_none());
        assert!(restored.task.is_none());
        let mut saved = state().saved;
        saved.work_periods = periods;
        saved.work_periods[1].subagents[1].status = SubagentStatus::Working;
        let recovered = Snapshot::new(saved);
        let active = active_subagents(&recovered.saved.work_periods);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].status, SubagentStatus::Unknown);
        assert!(recovered.saved.fresh_conversation().work_periods.is_empty());
    }

    #[test]
    fn subagent_collab_counts_children_not_calls_and_isolates_parent_state() {
        let mut state = state();
        state.begin_work();
        let item = json!({"id":"spawn","type":"collabAgentToolCall","tool":"spawnAgent","status":"completed","senderThreadId":"thread-1","receiverThreadIds":["child-1"],"prompt":"Inspect the parser","agentRole":"explorer","agentsStates":{"child-1":{"status":"running","message":null}}});
        let event = json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":item}});
        assert!(state.apply(&event));
        assert!(state.apply(&event));
        assert_eq!(state.saved.work_periods[0].subagents.len(), 1);
        assert_eq!(
            state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Working
        );
        assert_eq!(
            state.saved.work_periods[0].subagents[0].activity,
            "Inspect the parser"
        );
        assert_eq!(
            state.saved.work_periods[0].subagents[0].task.as_deref(),
            Some("Inspect the parser")
        );
        assert_eq!(
            state.saved.work_periods[0].subagents[0].role.as_deref(),
            Some("explorer")
        );
        let parent_items = state.saved.items.clone();
        assert!(!state.apply(&json!({"method":"thread/tokenUsage/updated","params":{"threadId":"child-1","tokenUsage":{"last":{"totalTokens":999},"modelContextWindow":1000}}})));
        assert_eq!(state.context_tokens, None);
        assert_eq!(state.status, Status::Working);
        assert_eq!(state.turn_id.as_deref(), Some("turn-1"));
        assert_eq!(state.saved.items, parent_items);
        assert!(state.apply(&json!({"method":"item/started","params":{"threadId":"child-1","turnId":"child-turn","item":{"id":"cmd","type":"commandExecution","command":"cargo test parser"}}})));
        assert_eq!(
            state.saved.work_periods[0].subagents[0].activity,
            "cargo test parser"
        );
        assert!(state.apply(&json!({"method":"turn/completed","params":{"threadId":"child-1","turn":{"id":"child-turn","status":"failed"}}})));
        assert_eq!(
            state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Failed
        );
        state.apply(&event); // A duplicate spawn does not resurrect the failed worker.
        assert_eq!(
            state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Failed
        );
        assert_eq!(state.status, Status::Working);
        let resume = json!({"id":"resume-child", "tool":"resumeAgent", "receiverThreadIds":["child-1"],"agentsStates":{"child-1":{"status":"running"}}});
        let mut empty_started = resume.clone();
        empty_started["agentsStates"] = json!({});
        state.record_collab(&empty_started, 0);
        assert_eq!(
            state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Failed
        );
        state.record_collab(&resume, 0);
        assert_eq!(
            state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Working
        );
        state.apply(&json!({"method":"turn/completed","params":{"threadId":"child-1","turn":{"id":"child-turn-2","status":"completed"}}}));
        state.record_collab(&resume, 0);
        assert_eq!(
            state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Completed
        );
    }
    #[test]
    fn subagent_reads_find_nested_children_and_preserve_turn_history() {
        let mut state = state();
        state.begin_work();
        state.upsert_subagent("child", None, Some("Inspect"), SubagentStatus::Working);
        let read = json!({"id":"child","agentNickname":"Ada","agentRole":"reviewer","turns":[{"id":"child-turn","status":"inProgress","items":[{"id":"nested","type":"collabAgentToolCall","tool":"spawnAgent","status":"completed","senderThreadId":"child","receiverThreadIds":["nested"],"agentsStates":{"nested":{"status":"running"}}},{"id":"cmd","type":"commandExecution","command":"rg parser"}]}]});
        state.apply_subagent_read("child", 0, &read);
        state.apply_subagent_read("child", 0, &read);
        assert_eq!(state.saved.work_periods[0].subagents.len(), 2);
        assert_eq!(state.saved.work_periods[0].subagents[0].name, "Ada");
        assert_eq!(
            state.saved.work_periods[0].subagents[0].role.as_deref(),
            Some("reviewer")
        );
        assert_eq!(
            state.saved.work_periods[0].subagents[0].activity,
            "rg parser"
        );
        state
            .saved
            .items
            .push(json!({"id":"answer","type":"agentMessage","text":"Done"}));
        state.finish_work();
        // Parent completion is not child completion.
        assert_eq!(
            state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Working
        );
        state.begin_work();
        state.apply_subagent_read(
            "child",
            0,
            &json!({"id":"child","turns":[{"id":"child-turn","status":"completed","items":[]}]}),
        );
        assert_eq!(
            state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Completed
        );
        assert!(state.saved.work_periods[1].subagents.is_empty());
        let round_trip: SavedSession =
            serde_json::from_value(serde_json::to_value(&state.saved).unwrap()).unwrap();
        assert_eq!(round_trip.work_periods[0].subagents.len(), 2);
        let old: WorkPeriod =
            serde_json::from_value(json!({"start_item":0,"end_item":1,"duration_ms":2})).unwrap();
        assert!(old.subagents.is_empty());
    }
    #[test]
    fn reused_subagent_belongs_to_new_work_and_old_read_cannot_rewrite_history() {
        let mut state = state();
        state.begin_work();
        state.upsert_subagent(
            "child",
            Some("Ada"),
            Some("First task"),
            SubagentStatus::Completed,
        );
        state
            .saved
            .items
            .push(json!({"id":"answer","type":"agentMessage","text":"Done"}));
        state.finish_work();
        state.begin_work();
        state.record_collab(&json!({"receiverThreadIds":["child"],"tool":"followupTask","agentsStates":{"child":{"status":"running","message":"Second task"}}}), 1);
        assert_eq!(
            state.saved.work_periods[0].subagents[0].status,
            SubagentStatus::Completed
        );
        assert_eq!(
            state.saved.work_periods[1].subagents[0].status,
            SubagentStatus::Working
        );
        state.apply_subagent_read("child", 0, &json!({"id":"child","agentNickname":"Wrong","turns":[{"id":"new-child-turn","status":"inProgress","items":[]}]}));
        assert_eq!(state.saved.work_periods[0].subagents[0].name, "Ada");
        // An inherited parent turn is not this child's completion or activity.
        state.apply_subagent_read("child", 1, &json!({"id":"child","turns":[{"id":"turn-1","status":"completed","items":[{"type":"commandExecution","command":"parent command"}]}]}));
        assert_eq!(
            state.saved.work_periods[1].subagents[0].activity,
            "Second task"
        );
        assert_eq!(
            state.saved.work_periods[1].subagents[0].status,
            SubagentStatus::Working
        );
    }
    #[test]
    fn subagent_recent_activity_bounds_dedup_and_poll_delta() {
        let mut state = state();
        state.begin_work();
        state.upsert_subagent("child", None, None, SubagentStatus::Working);
        for i in 0..40 {
            state.observe_subagent(
                "child",
                &format!("item-{i}"),
                SubagentActivityKind::Tool,
                &"é".repeat(900),
            );
        }
        let activity = &state.subagent_activity["child"];
        assert!(activity.truncated);
        assert!(activity.events.len() <= 20);
        assert!(activity.events.iter().all(|e| e.text.len() <= 1024));
        assert!(
            activity
                .events
                .iter()
                .map(|e| e.id.len() + e.text.len())
                .sum::<usize>()
                <= 16 * 1024
        );
        let thread = json!({"id":"child","turns":[{"id":"child-turn","status":"inProgress","items":[{"id":"a","type":"agentMessage","text":"First"},{"id":"b","type":"commandExecution","command":"cargo check"}]}]});
        state.apply_subagent_read("child", 0, &thread);
        let before = state.subagent_activity["child"].events.clone();
        state.apply_subagent_read("child", 0, &thread);
        let after = &state.subagent_activity["child"].events;
        assert_eq!(before.len(), after.len());
        assert_eq!(
            before.last().unwrap().observed_at,
            after.last().unwrap().observed_at
        );
        assert_eq!(after.last().unwrap().kind, SubagentActivityKind::Tool);
        state.apply_subagent_read("child", 0, &json!({"id":"child","turns":[{"id":"turn-1","items":[{"id":"p","type":"agentMessage","text":"Parent secret"}]}]}));
        assert!(!state.subagent_activity["child"]
            .events
            .iter()
            .any(|e| e.text == "Parent secret"));
    }
    #[test]
    fn subagent_recent_activity_poll_refreshes_earlier_retained_tool() {
        let mut state = state();
        state.begin_work();
        state.upsert_subagent("child", None, None, SubagentStatus::Working);
        let mut thread = json!({"id":"child","turns":[{"id":"child-turn","status":"inProgress","items":[{"id":"a","type":"mcpToolCall","tool":"read","progress":"Opening source"},{"id":"b","type":"commandExecution","command":"cargo check"}]}]});
        state.apply_subagent_read("child", 0, &thread);
        let original_b = state.subagent_activity["child"].events[1].observed_at;
        thread["turns"][0]["items"][0]["progress"] = json!("Reading source");
        state.apply_subagent_read("child", 0, &thread);
        let activity = &state.subagent_activity["child"];
        assert_eq!(activity.events.len(), 2);
        assert_eq!(activity.events[0].text, "Reading source");
        assert_eq!(activity.events[1].observed_at, original_b);
        let observed = activity.observed_at;
        let times: Vec<_> = activity.events.iter().map(|e| e.observed_at).collect();
        state.apply_subagent_read("child", 0, &thread);
        assert_eq!(state.subagent_activity["child"].observed_at, observed);
        assert_eq!(
            state.subagent_activity["child"]
                .events
                .iter()
                .map(|e| e.observed_at)
                .collect::<Vec<_>>(),
            times
        );
    }
    #[test]
    fn subagent_recent_activity_session_bound_and_reasoning_exclusion() {
        let mut state = state();
        state.begin_work();
        for i in 0..120 {
            let id = format!("child-{i}");
            state.upsert_subagent(&id, None, None, SubagentStatus::Working);
            for j in 0..20 {
                state.observe_subagent(
                    &id,
                    &format!("item-{j}"),
                    SubagentActivityKind::Commentary,
                    &"x".repeat(1024),
                );
            }
        }
        assert_eq!(state.subagent_activity.len(), 100);
        assert!(state.subagent_activity_bytes() <= 1024 * 1024);
        assert!(
            state
                .subagent_activity
                .values()
                .flat_map(|a| &a.events)
                .map(|e| e.id.len() + e.text.len())
                .sum::<usize>()
                <= 1024 * 1024
        );
        state.apply_subagent_notification(&json!({"method":"item/completed","params":{"threadId":"child-119","item":{"id":"reason","type":"reasoning","text":"private"}}}), 0);
        assert!(!state.subagent_activity["child-119"]
            .events
            .iter()
            .any(|e| e.text.contains("private")));
    }
    #[test]
    fn subagent_recent_activity_replacement_and_terminal_eviction() {
        let mut state = state();
        state.begin_work();
        state.upsert_subagent("child", None, None, SubagentStatus::Working);
        for i in 0..20 {
            state.observe_subagent(
                "child",
                &format!("item-{i}"),
                SubagentActivityKind::Tool,
                "small",
            );
        }
        for i in 0..20 {
            state.observe_subagent(
                "child",
                &format!("item-{i}"),
                SubagentActivityKind::Tool,
                &"x".repeat(1024),
            );
        }
        assert!(
            state.subagent_activity["child"]
                .events
                .iter()
                .map(|e| e.id.len() + e.text.len())
                .sum::<usize>()
                <= 16 * 1024
        );
        state.upsert_subagent("terminal", None, None, SubagentStatus::Completed);
        state.observe_subagent(
            "terminal",
            "event",
            SubagentActivityKind::Commentary,
            "Done",
        );
        for i in 0..99 {
            let id = format!("new-{i}");
            state.upsert_subagent(&id, None, None, SubagentStatus::Working);
            state.observe_subagent(&id, "event", SubagentActivityKind::Tool, "Read");
        }
        assert!(!state.subagent_activity.contains_key("terminal"));
        assert!(state.subagent_activity_evicted.contains("terminal"));
        assert!(state.subagent_activity.contains_key("child"));
    }
    #[test]
    fn subagent_recent_activity_rejects_stale_generation_success_and_failure() {
        let mut actor = actor();
        actor.state.begin_work();
        actor
            .state
            .upsert_subagent("child", None, None, SubagentStatus::Working);
        actor.state.ensure_subagent_activity("child");
        actor.state.subagent_observation_generation = 1;
        for id in 1..=2 {
            actor.pending.insert(
                id,
                Request {
                    kind: Pending::ChildRead {
                        thread: "child".into(),
                        period_start: 0,
                        generation: 0,
                    },
                    deadline: Instant::now(),
                },
            );
        }
        actor.message(json!({"id":1,"result":{"thread":{"id":"child","turns":[{"id":"old-child-turn","status":"completed","items":[{"type":"agentMessage","text":"Stale"}]}]}}})).unwrap();
        actor
            .message(json!({"id":2,"error":{"message":"Stale failure"}}))
            .unwrap();
        assert!(actor.state.subagent_activity["child"].events.is_empty());
        assert!(!actor.state.subagent_activity["child"].refresh_error);
        assert!(actor.child_read_failures.is_empty());
        assert_eq!(
            actor.state.saved.work_periods.last().unwrap().subagents[0].status,
            SubagentStatus::Working
        );
    }
    #[test]
    fn optional_subagent_read_errors_do_not_fail_parent_or_pause_queue() {
        let mut actor = actor();
        actor.queue_ready = true;
        actor.pending.insert(
            1,
            Request {
                kind: Pending::ChildRead {
                    thread: "child".into(),
                    period_start: 0,
                    generation: 0,
                },
                deadline: Instant::now(),
            },
        );
        actor
            .message(json!({"id":1,"error":{"message":"Thread not found"}}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Working);
        assert!(actor.state.error.is_none());
        assert!(actor.queue_ready);
        assert!(!actor.queue_paused);
        actor.pending.insert(
            2,
            Request {
                kind: Pending::ChildRead {
                    thread: "child".into(),
                    period_start: 0,
                    generation: 0,
                },
                deadline: Instant::now().checked_sub(Duration::from_secs(1)).unwrap(),
            },
        );
        actor.expire_subagent_reads();
        assert!(actor.pending.is_empty());
        assert_eq!(actor.state.status, Status::Working);
        assert_eq!(actor.child_read_failures.get("child"), Some(&2));
    }
    #[test]
    fn old_recovery_defaults_to_codex_and_claude_round_trips() {
        let mut record = serde_json::to_value(saved()).unwrap();
        record.as_object_mut().unwrap().remove("provider");
        let restored: SavedSession = serde_json::from_value(record).unwrap();
        assert_eq!(restored.provider, Provider::Codex);
        let mut claude = restored;
        claude.provider = Provider::Claude;
        let restored: SavedSession =
            serde_json::from_slice(&serde_json::to_vec(&claude).unwrap()).unwrap();
        assert_eq!(restored.provider, Provider::Claude);
        assert_eq!(restored.provider.agent(), crate::agent::Agent::Claude);
    }
    #[test]
    fn child_read_orders_older_history_before_live_items() {
        let mut state = state();
        state.begin_work();
        state.upsert_subagent("child", None, None, SubagentStatus::Working);
        state.upsert_subagent_item("child", json!({"id":"b","type":"agentMessage","text":"B"}));
        state.upsert_subagent_item("child", json!({"id":"c","type":"agentMessage","text":"C"}));
        state.apply_subagent_read("child", 0, &json!({"id":"child","turns":[{"id":"child-turn","status":"inProgress","items":[{"id":"a","type":"agentMessage","text":"A"},{"id":"b","type":"agentMessage","text":"B"}]}]}));
        assert_eq!(
            state.subagent_activity["child"]
                .items
                .iter()
                .map(|item| item["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn child_transcript_retains_complete_items_and_streams_without_parent_mutation() {
        let mut state = state();
        state.begin_work();
        state.upsert_subagent("child", Some("Ada"), None, SubagentStatus::Working);
        let parent = state.saved.items.clone();
        let text = "full visible commentary ".repeat(150);
        let items: Vec<_> = (0..30)
            .map(|i| json!({"id":format!("item-{i}"),"type":"agentMessage","text":text}))
            .collect();
        state.apply_subagent_read("child", 0, &json!({"id":"child","turns":[{"id":"child-turn","status":"inProgress","items":items}]}));
        assert_eq!(state.subagent_activity["child"].items.len(), 30);
        assert_eq!(state.subagent_activity["child"].items[0]["text"], text);
        state.apply(&json!({"method":"item/started","params":{"threadId":"child","turnId":"child-turn","item":{"id":"cmd","type":"commandExecution","command":"pwd","status":"inProgress"}}}));
        assert!(state.apply(&json!({"method":"item/commandExecution/outputDelta","params":{"threadId":"child","turnId":"child-turn","itemId":"cmd","delta":"full output"}})));
        let projection = state.subagent_snapshot("child");
        assert_eq!(
            projection.saved.items.last().unwrap()["aggregatedOutput"],
            "full output"
        );
        assert_eq!(projection.saved.items.len(), 31);
        assert_eq!(state.saved.items, parent);
        assert!(projection.subagent_activity.is_empty());
    }

    fn state() -> Snapshot {
        let mut state = Snapshot::new(saved());
        state.status = Status::Working;
        state.turn_id = Some("turn-1".into());
        state
    }
    #[test]
    fn provider_notices_preserve_retry_recovery_and_raw_error_ownership() {
        let mut state = state();
        state.apply(&json!({"method":"error","params":{"threadId":"thread-1","turnId":"turn-1","error":{"message":"Connection interrupted"},"willRetry":true}}));
        assert_eq!(state.status, Status::Working);
        assert_eq!(
            state.effective_notice().unwrap().kind,
            SessionNoticeKind::Retrying
        );
        state.apply(&json!({"method":"item/commandExecution/outputDelta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"tool","delta":"Local output"}}));
        assert_eq!(
            state.effective_notice().unwrap().kind,
            SessionNoticeKind::Retrying
        );
        state.apply(&json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"message","delta":"Recovered"}}));
        assert!(state.error.is_none());
        state.apply(&json!({"method":"account/rateLimits/updated","params":{"rateLimits":{"primary":{"usedPercent":100,"resetsAt":1234}}}}));
        assert!(state.error.is_none());
        state.apply(&json!({"method":"error","params":{"threadId":"thread-1","error":{"message":"Limit reached","codexErrorInfo":"usageLimitExceeded"},"willRetry":false}}));
        assert_eq!(state.effective_notice().unwrap().reset_at, Some(1234));
        assert!(state.apply(&json!({"method":"account/rateLimits/updated","params":{"rateLimits":{"primary":{"usedPercent":100,"resetsAt":5678}}}})));
        assert_eq!(state.effective_notice().unwrap().reset_at, Some(5678));
        assert_eq!(
            state.effective_notice().unwrap().kind,
            SessionNoticeKind::UsageLimit
        );
        state.error = Some("A different error".into());
        assert!(state.effective_notice().is_none());
    }
    #[test]
    fn provider_notice_does_not_revive_after_clear_and_identical_raw_error() {
        let mut state = state();
        state.set_provider_error("Codex", "Connection interrupted".into(), true, None, None);
        state.clear_error();
        state.error = Some("Connection interrupted".into());
        assert!(state.effective_notice().is_none());
    }
    #[test]
    fn provider_warning_is_historical_and_foreign_errors_are_ignored() {
        let mut state = state();
        assert!(state.apply(&json!({"method":"configWarning","params":{"summary":"Configuration warning","details":"Unsupported setting"}})));
        assert_eq!(state.saved.items.last().unwrap()["type"], "providerNotice");
        let count = state.saved.items.len();
        state.apply(&json!({"method":"configWarning","params":{"summary":"Configuration warning","details":"Unsupported setting"}}));
        assert_eq!(state.saved.items.len(), count);
        assert!(state.error.is_none());
        assert_eq!(state.status, Status::Working);
        assert!(!state.apply(&json!({"method":"error","params":{"threadId":"foreign","error":{"message":"Not ours"}}})));
        assert!(state.error.is_none());
    }
    fn actor() -> Actor {
        let (_, actions) = mpsc::channel();
        let (updates, _) = crate::latest::channel();
        let mut actor = Actor::new(saved(), actions, updates, Vec::new());
        actor.state = state();
        actor
    }
    #[test]
    #[cfg(unix)]
    fn project_prompt_preserves_composer_and_uses_native_queue() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.state.status = Status::Working;
        actor.state.turn_id = Some("busy".into());
        actor.state.saved.draft = "same text".into();
        actor.state.saved.draft_reference = Some(reference("preserved"));
        actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
        actor
            .action(Action::ProjectPrompt {
                id: 41,
                text: "same text".into(),
            })
            .unwrap();
        assert_eq!(actor.state.queued_prompts[0].text, "same text");
        assert_eq!(actor.state.saved.draft, "same text");
        assert_eq!(
            actor.state.saved.draft_reference,
            Some(reference("preserved"))
        );
        assert_eq!(actor.state.saved.draft_images.len(), 1);
        assert!(actor.state.queue_sending.is_empty());
        actor.state.turn_id = None;
        actor.state.status = Status::Idle;
        actor.queue_ready = true;
        actor.dispatch_queue().unwrap();
        assert_eq!(actor.state.queue_sending, vec![41]);
        assert_eq!(actor.state.saved.draft, "same text");
        assert_eq!(
            actor.state.saved.draft_reference,
            Some(reference("preserved"))
        );
        assert_eq!(actor.state.saved.draft_images.len(), 1);
    }
    #[test]
    #[cfg(unix)]
    fn uncertain_delivery_requires_explicit_dismissal_then_resume() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor
            .submit_mode(41, "unknown".into(), Vec::new(), SubmissionMode::Queue)
            .unwrap();
        actor
            .submit_mode(42, "next".into(), Vec::new(), SubmissionMode::Queue)
            .unwrap();
        actor.action(Action::SteerQueued(41)).unwrap();
        actor
            .message(json!({"id":1,"result":{"turnId":"wrong"}}))
            .unwrap();
        assert_eq!(actor.state.queue_uncertain, vec![41]);
        actor.state.turn_id = None;
        actor.state.status = Status::Idle;
        assert!(actor.action(Action::ResumeQueue).is_err());
        assert!(actor.action(Action::BeginQueuedEdit(41)).is_err());
        assert!(actor.action(Action::RemoveQueued(41)).is_err());
        assert!(actor.action(Action::DismissUncertainQueued(42)).is_err());
        actor.action(Action::DismissUncertainQueued(41)).unwrap();
        assert!(actor.queue_paused);
        assert!(!actor.queue_ready);
        assert!(actor.pending.is_empty());
        assert_eq!(actor.state.queued_prompts[0].id, 42);
        actor.action(Action::ResumeQueue).unwrap();
        assert_eq!(actor.state.queue_sending, vec![42]);
    }

    #[test]
    fn queue_edit_preserves_identity_position_and_attachments() {
        let mut actor = actor();
        actor.state.queued_prompts = vec![
            QueuedPrompt {
                id: 41,
                text: "first".into(),
                images: Vec::new(),
            },
            QueuedPrompt {
                id: 42,
                text: "second".into(),
                images: Vec::new(),
            },
        ];
        actor.action(Action::BeginQueuedEdit(42)).unwrap();
        assert_eq!(actor.state.queue_edit, Some(42));
        assert!(actor.action(Action::BeginQueuedEdit(41)).is_err());
        assert!(actor
            .action(Action::SaveQueuedEdit {
                id: 41,
                text: "wrong".into(),
                images: Vec::new()
            })
            .is_err());
        assert!(actor
            .action(Action::SaveQueuedEdit {
                id: 42,
                text: String::new(),
                images: Vec::new()
            })
            .is_err());
        assert_eq!(actor.state.queue_edit, Some(42));
        let images = vec!["data:image/png;base64,aGVsbG8=".into()];
        actor
            .action(Action::SaveQueuedEdit {
                id: 42,
                text: "revised".into(),
                images: images.clone(),
            })
            .unwrap();
        assert_eq!(actor.state.queue_edit, None);
        assert_eq!(actor.state.queued_prompts[0].id, 41);
        assert_eq!(
            actor.state.queued_prompts[1],
            QueuedPrompt {
                id: 42,
                text: "revised".into(),
                images
            }
        );
        actor.action(Action::BeginQueuedEdit(42)).unwrap();
        actor.action(Action::CancelQueuedEdit(42)).unwrap();
        assert_eq!(actor.state.queued_prompts[1].text, "revised");
    }

    #[test]
    #[cfg(unix)]
    fn held_queue_head_blocks_fifo_until_released() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor
            .submit_mode(41, "first".into(), Vec::new(), SubmissionMode::Queue)
            .unwrap();
        actor
            .submit_mode(42, "second".into(), Vec::new(), SubmissionMode::Queue)
            .unwrap();
        actor.action(Action::BeginQueuedEdit(41)).unwrap();
        actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
        assert!(actor.pending.is_empty());
        actor.action(Action::CancelQueuedEdit(41)).unwrap();
        assert_eq!(actor.state.queue_sending, vec![41]);
        assert!(actor.action(Action::BeginQueuedEdit(41)).is_err());
        assert!(actor.action(Action::RemoveQueued(41)).is_err());
        // Editing a later row does not prevent the earlier request's acknowledgement.
        actor.action(Action::BeginQueuedEdit(42)).unwrap();
        actor
            .message(json!({"id":1,"result":{"turn":{"id":"turn-2"}}}))
            .unwrap();
        assert_eq!(actor.state.queued_prompts[0].id, 42);
        assert_eq!(actor.state.queue_edit, Some(42));
        assert!(actor.state.queue_sending.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn queued_steering_is_atomic_and_preserves_row_on_rejection_or_uncertainty() {
        for response in [
            json!({"id":1,"error":{"message":"rejected"}}),
            json!({"id":1,"result":{"turnId":"wrong"}}),
            json!({"id":1,"result":{"turnId":"turn-1"}}),
        ] {
            let mut actor = actor();
            actor.process = Some(fixture_process());
            actor.state.saved.draft = "unrelated draft".into();
            actor
                .submit_mode(41, "queued".into(), Vec::new(), SubmissionMode::Queue)
                .unwrap();
            actor.action(Action::SteerQueued(41)).unwrap();
            assert_eq!(actor.state.queued_prompts.len(), 1);
            assert_eq!(actor.state.queue_sending, vec![41]);
            assert!(actor.action(Action::SteerQueued(41)).is_err());
            assert!(actor.action(Action::RemoveQueued(41)).is_err());
            actor.message(response.clone()).unwrap();
            assert_eq!(actor.state.saved.draft, "unrelated draft");
            if response["result"]["turnId"] == "turn-1" {
                assert!(actor.state.queued_prompts.is_empty());
                assert!(actor.state.queue_sending.is_empty());
            } else {
                assert_eq!(actor.state.queued_prompts[0].id, 41);
                assert_eq!(
                    actor.state.queue_editable(41),
                    response.get("error").is_some()
                );
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn pending_steer_blocks_fifo_after_turn_completes_before_ack() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor
            .submit_mode(41, "first".into(), Vec::new(), SubmissionMode::Queue)
            .unwrap();
        actor
            .submit_mode(42, "steering".into(), Vec::new(), SubmissionMode::Queue)
            .unwrap();
        actor.action(Action::SteerQueued(42)).unwrap();
        assert_eq!(actor.state.queue_steering, vec![42]);
        actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
        assert_eq!(actor.pending.len(), 1);
        assert_eq!(actor.state.queue_sending, vec![42]);
        actor
            .message(json!({"id":1,"result":{"turnId":"turn-1"}}))
            .unwrap();
        assert_eq!(actor.pending.len(), 1);
        assert_eq!(actor.state.queue_sending, vec![41]);
        assert!(actor.state.queue_steering.is_empty());
        assert_eq!(actor.state.queued_prompts.len(), 1);
        assert_eq!(actor.state.queued_prompts[0].id, 41);
    }

    #[test]
    #[cfg(unix)]
    fn composer_steer_has_visible_locked_row_until_acknowledged() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.state.saved.draft = "steer".into();
        actor
            .submit_mode(41, "steer".into(), Vec::new(), SubmissionMode::Steer)
            .unwrap();
        assert_eq!(actor.state.queued_prompts[0].id, 41);
        assert!(!actor.state.queue_editable(41));
        actor
            .message(json!({"id":1,"result":{"turnId":"turn-1"}}))
            .unwrap();
        assert!(actor.state.queued_prompts.is_empty());
        assert!(actor.state.queue_sending.is_empty());
        assert!(actor.state.saved.draft.is_empty());
    }

    #[test]
    fn composer_overrides_are_explicit_and_native() {
        let mut actor = actor();
        let defaults = actor.turn_params(Vec::new());
        assert!(defaults.get("model").is_none());
        assert!(defaults.get("effort").is_none());
        assert!(defaults.get("approvalPolicy").is_none());
        assert!(defaults.get("sandboxPolicy").is_none());
        actor
            .action(Action::Configure {
                model: Some("test-model".into()),
                effort: Some("low".into()),
                access: Some("read-only".into()),
            })
            .unwrap();
        let params = actor.turn_params(Vec::new());
        assert_eq!(params["model"], "test-model");
        assert_eq!(params["effort"], "low");
        assert_eq!(params["sandboxPolicy"]["type"], "readOnly");
        assert_eq!(params["approvalPolicy"], "on-request");
        actor
            .action(Action::Configure {
                model: None,
                effort: None,
                access: Some("full-access".into()),
            })
            .unwrap();
        let params = actor.turn_params(Vec::new());
        assert_eq!(params["sandboxPolicy"]["type"], "dangerFullAccess");
        assert_eq!(params["approvalPolicy"], "never");
        assert!(actor
            .action(Action::Configure {
                model: None,
                effort: None,
                access: Some("invalid".into())
            })
            .is_err());
    }
    #[test]
    fn live_catalog_ultra_effort_is_sent_and_persisted() {
        let mut actor = actor();
        // Codex CLI 0.163.0 model/list advertises these stops for GPT-6.1 Sol.
        actor.state.models = vec![json!({
            "id":"gpt-6.1-sol", "model":"gpt-6.1-sol",
            "defaultReasoningEffort":"medium",
            "supportedReasoningEfforts":[
                {"reasoningEffort":"low"}, {"reasoningEffort":"medium"},
                {"reasoningEffort":"high"}, {"reasoningEffort":"xhigh"},
                {"reasoningEffort":"max"}, {"reasoningEffort":"ultra"}
            ]
        })];
        actor
            .action(Action::Configure {
                model: Some("gpt-6.1-sol".into()),
                effort: Some("ultra".into()),
                access: None,
            })
            .unwrap();
        assert_eq!(actor.turn_params(vec![])["effort"], "ultra");
        let recovered =
            serde_json::from_value(serde_json::to_value(&actor.state.saved).unwrap()).unwrap();
        let (_, input) = mpsc::channel();
        let (updates, _) = crate::latest::channel();
        let mut restored = Actor::new(recovered, input, updates, Vec::new());
        assert_eq!(restored.turn_params(vec![])["effort"], "ultra");
        assert!(restored
            .action(Action::Configure {
                model: None,
                effort: Some("bogus".into()),
                access: None,
            })
            .is_err());
        assert_eq!(restored.turn_params(vec![])["effort"], "ultra");
    }
    #[test]
    fn changing_models_explicitly_replaces_previous_effort_with_catalog_default() {
        let mut actor = actor();
        actor.state.models =
            vec![json!({"id":"new-id","model":"new-model","defaultReasoningEffort":"low"})];
        actor.state.effort = Some("xhigh".into());
        actor
            .action(Action::Configure {
                model: Some("new-model".into()),
                effort: None,
                access: None,
            })
            .unwrap();
        assert_eq!(actor.turn_params(vec![])["effort"], "low");
        assert_eq!(actor.state.saved.composer.effort.as_deref(), Some("low"));
        actor
            .action(Action::Configure {
                model: Some("new-model".into()),
                effort: Some("medium".into()),
                access: None,
            })
            .unwrap();
        assert_eq!(actor.turn_params(vec![])["effort"], "medium");
    }
    #[test]
    fn scoped_model_effort_reset_preserves_access_and_survives_recovery() {
        for explicit in [false, true] {
            let mut actor = actor();
            actor.configured_turn = json!({"model":"configured","effort":"medium","sandboxPolicy":{"type":"readOnly"},"approvalPolicy":"on-request"});
            let effective_access = if explicit {
                "full-access"
            } else {
                "workspace-write"
            };
            actor.state.resolved_access = Some(effective_access.into());
            let inherited_sandbox = json!({"type":"workspaceWrite","writableRoots":["/custom"],"networkAccess":true,"excludeTmpdirEnvVar":true});
            actor.state.sandbox = Some(inherited_sandbox.clone());
            actor
                .action(Action::Configure {
                    model: Some("selected".into()),
                    effort: Some("high".into()),
                    access: explicit.then(|| "full-access".into()),
                })
                .unwrap();
            actor.action(Action::ResetModelEffort).unwrap();
            assert_eq!(
                actor.state.resolved_access.as_deref(),
                Some(effective_access)
            );
            assert_eq!(actor.state.sandbox, Some(inherited_sandbox));
            let params = actor.turn_params(vec![]);
            assert_eq!(params["model"], "configured");
            assert_eq!(params["effort"], "medium");
            if explicit {
                assert_eq!(params["sandboxPolicy"]["type"], "dangerFullAccess");
                assert_eq!(params["approvalPolicy"], "never");
            } else {
                assert!(params.get("sandboxPolicy").is_none());
                assert!(params.get("approvalPolicy").is_none());
                assert!(actor.state.saved.composer.access.is_none());
            }
            let recovered =
                serde_json::from_value(serde_json::to_value(&actor.state.saved).unwrap()).unwrap();
            let (_, input) = mpsc::channel();
            let (updates, _) = crate::latest::channel();
            let mut restored = Actor::new(recovered, input, updates, Vec::new());
            restored.configured_turn = actor.configured_turn.clone();
            assert!(restored.state.saved.composer.reset_model_effort_only);
            assert_eq!(restored.turn_params(vec![]), params);
            restored
                .action(Action::Configure {
                    model: None,
                    effort: Some("low".into()),
                    access: None,
                })
                .unwrap();
            let params = restored.turn_params(vec![]);
            assert!(restored.state.saved.composer.reset_model_effort_only);
            assert_eq!(params["model"], "configured");
            assert_eq!(params["effort"], "low");
            if explicit {
                assert_eq!(params["sandboxPolicy"]["type"], "dangerFullAccess");
            } else {
                assert!(params.get("sandboxPolicy").is_none());
                assert!(params.get("approvalPolicy").is_none());
            }
            restored.action(Action::ResetConfiguration).unwrap();
            assert!(!restored.state.saved.composer.reset_model_effort_only);
            assert_eq!(
                restored.turn_params(vec![])["sandboxPolicy"]["type"],
                "readOnly"
            );
        }
    }
    #[test]
    fn composer_settings_survive_recovery_and_reset_restores_protocol_defaults() {
        let mut actor = actor();
        actor.configured_turn = json!({"model":"configured","effort":"medium","sandboxPolicy":{"type":"readOnly"},"approvalPolicy":"on-request"});
        actor
            .action(Action::Configure {
                model: Some("selected".into()),
                effort: Some("high".into()),
                access: Some("full-access".into()),
            })
            .unwrap();
        let recovered: SavedSession =
            serde_json::from_value(serde_json::to_value(&actor.state.saved).unwrap()).unwrap();
        let state = Snapshot::new(recovered);
        assert_eq!(state.model.as_deref(), Some("selected"));
        assert_eq!(state.effort.as_deref(), Some("high"));
        assert_eq!(state.access.as_deref(), Some("full-access"));
        actor.action(Action::ResetConfiguration).unwrap();
        let params = actor.turn_params(vec![]);
        assert_eq!(params["model"], "configured");
        assert_eq!(params["effort"], "medium");
        assert_eq!(params["sandboxPolicy"]["type"], "readOnly");
        assert_eq!(params["approvalPolicy"], "on-request");
        assert!(actor.state.saved.composer.model.is_none());
        assert_eq!(actor.state.resolved_model.as_deref(), Some("configured"));
        assert_eq!(actor.state.resolved_effort.as_deref(), Some("medium"));
        assert_eq!(actor.state.resolved_access.as_deref(), Some("read-only"));
    }
    #[test]
    fn skill_mentions_use_catalog_paths_and_optional_catalog_errors_are_nonfatal() {
        let mut actor = actor();
        actor.state.skills = vec![
            json!({"name":"review","path":"/skills/review/SKILL.md","enabled":true}),
            json!({"name":"disabled","path":"/disabled","enabled":false}),
        ];
        let input = actor.input("Use $review, then $disabled and $reviewer", vec![]);
        assert_eq!(input.len(), 2);
        assert_eq!(
            input[1],
            json!({"type":"skill","name":"review","path":"/skills/review/SKILL.md"})
        );
        actor.pending.insert(
            99,
            Request {
                kind: Pending::Models,
                deadline: Instant::now(),
            },
        );
        actor
            .message(json!({"id":99,"error":{"message":"unsupported"}}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Working);
        assert!(actor.state.error.is_none());
    }
    #[cfg(unix)]
    fn fixture_process() -> Process {
        use std::os::unix::process::CommandExt;
        let mut child = Command::new("sh")
            .args(["-c", "cat >/dev/null"])
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let (_, lines) = mpsc::channel();
        Process {
            child,
            input,
            lines,
        }
    }
    #[test]
    #[cfg(unix)]
    fn accepted_steering_clears_only_submitted_draft_after_recovery() {
        let sent_images = vec!["data:image/png;base64,aGVsbG8=".to_owned()];
        for newer in [
            None,
            Some(("new unsent draft", sent_images.clone())),
            Some(("sent", Vec::new())),
        ] {
            let mut actor = actor();
            actor.process = Some(fixture_process());
            actor.state.saved.draft = "sent".into();
            actor.state.saved.draft_images = sent_images.clone();
            actor
                .submit_mode(
                    42,
                    "sent".into(),
                    sent_images.clone(),
                    SubmissionMode::Steer,
                )
                .unwrap();
            let awaiting = recover_saved_for_test(&actor.state.saved);
            assert_eq!(awaiting.draft, "sent");
            assert_eq!(awaiting.draft_images, sent_images);
            if let Some((text, images)) = &newer {
                actor.action(Action::Draft((*text).into())).unwrap();
                actor.action(Action::DraftImages(images.clone())).unwrap();
            }
            actor
                .message(json!({"id":1,"result":{"turnId":"turn-1"}}))
                .unwrap();
            assert_eq!(actor.state.accepted_submission, Some(42));
            let recovered = recover_saved_for_test(&actor.state.saved);
            let (text, images) = newer.unwrap_or(("", Vec::new()));
            assert_eq!(recovered.draft, text);
            assert_eq!(recovered.draft_images, images);
        }
    }

    #[test]
    #[cfg(unix)]
    fn queued_acceptance_clears_only_matching_composer_after_recovery() {
        let images = vec!["data:image/png;base64,aGVsbG8=".to_owned()];
        for draft in ["sent", "new unsent draft"] {
            let mut actor = actor();
            actor.process = Some(fixture_process());
            actor.state.saved.draft = draft.into();
            actor.state.saved.draft_images = images.clone();
            actor
                .submit_mode(42, "sent".into(), images.clone(), SubmissionMode::Queue)
                .unwrap();
            assert_eq!(actor.state.accepted_submission, Some(42));
            assert_eq!(actor.state.queued_prompts.len(), 1);
            assert_eq!(actor.state.queued_prompts[0].text, "sent");
            assert_eq!(actor.state.queued_prompts[0].images, images);
            let recovered = recover_saved_for_test(&actor.state.saved);
            assert_eq!(recovered.draft, if draft == "sent" { "" } else { draft });
            assert_eq!(
                recovered.draft_images,
                if draft == "sent" {
                    Vec::new()
                } else {
                    images.clone()
                }
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn rejected_steering_preserves_retry_draft_after_recovery() {
        let images = vec!["data:image/png;base64,aGVsbG8=".to_owned()];
        for response in [
            json!({"id":1,"error":{"message":"no active turn"}}),
            json!({"id":1,"result":{"turnId":"wrong-turn"}}),
        ] {
            let mut actor = actor();
            actor.process = Some(fixture_process());
            actor.state.saved.draft = "retry".into();
            actor.state.saved.draft_images = images.clone();
            actor
                .submit_mode(42, "retry".into(), images.clone(), SubmissionMode::Steer)
                .unwrap();
            actor.message(response).unwrap();
            assert_eq!(actor.state.rejected_submission, Some(42));
            let recovered = recover_saved_for_test(&actor.state.saved);
            assert_eq!(recovered.draft, "retry");
            assert_eq!(recovered.draft_images, images);
        }
    }

    #[test]
    #[cfg(unix)]
    fn queue_is_fifo_and_waits_for_exact_completion_and_ack() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor
            .submit_mode(41, "first".into(), Vec::new(), SubmissionMode::Queue)
            .unwrap();
        actor
            .submit_mode(42, "second".into(), Vec::new(), SubmissionMode::Queue)
            .unwrap();
        assert_eq!(actor.state.accepted_submission, Some(42));
        actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"wrong","status":"completed"}}})).unwrap();
        assert!(actor.pending.is_empty());
        actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
        assert_eq!(actor.pending.len(), 1);
        assert_eq!(actor.state.queued_prompts.len(), 2);
        actor.message(json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-2"}}})).unwrap();
        actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-2","status":"completed"}}})).unwrap();
        assert_eq!(actor.pending.len(), 1);
        actor.state.saved.draft = "independent draft".into();
        actor
            .message(json!({"id":1,"result":{"turn":{"id":"turn-2"}}}))
            .unwrap();
        assert_eq!(actor.state.queued_prompts[0].id, 42);
        assert_eq!(actor.state.saved.draft, "independent draft");
        assert_eq!(actor.pending.len(), 1);
        actor
            .message(json!({"id":1,"result":{"turn":{"id":"turn-2"}}}))
            .unwrap();
        assert_eq!(actor.state.queued_prompts.len(), 1);
    }
    #[test]
    #[cfg(unix)]
    fn malformed_queued_ack_and_steer_failure_keep_queue_paused() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor
            .submit_mode(41, "retained".into(), Vec::new(), SubmissionMode::Queue)
            .unwrap();
        actor
            .submit_mode(42, "steer".into(), Vec::new(), SubmissionMode::Steer)
            .unwrap();
        actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
        assert!(actor.queue_ready);
        assert_eq!(actor.pending.len(), 1);
        actor
            .message(json!({"id":1,"error":{"message":"too late"}}))
            .unwrap();
        assert!(actor.queue_paused);
        assert!(!actor.queue_ready);
        assert!(actor.pending.is_empty());
        actor.action(Action::ResumeQueue).unwrap();
        actor.message(json!({"id":2,"result":{}})).unwrap();
        assert_eq!(actor.state.queued_prompts[0].text, "retained");
        assert!(actor.queue_paused);
        assert_eq!(actor.state.status, Status::Failed);
        actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
        assert!(actor.pending.is_empty());
    }
    #[test]
    #[cfg(unix)]
    fn interrupted_queue_requires_explicit_resume_and_survives_rejection() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor
            .submit_mode(41, "keep me".into(), Vec::new(), SubmissionMode::Queue)
            .unwrap();
        actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"interrupted"}}})).unwrap();
        assert!(actor.pending.is_empty());
        actor.action(Action::ResumeQueue).unwrap();
        actor
            .message(json!({"id":1,"error":{"message":"rejected"}}))
            .unwrap();
        assert_eq!(actor.state.queued_prompts[0].text, "keep me");
        assert!(actor.pending.is_empty());
        actor.action(Action::RemoveQueued(41)).unwrap();
        assert!(actor.state.queued_prompts.is_empty());
    }
    #[test]
    #[cfg(unix)]
    fn steer_rejection_preserves_active_turn_and_approval() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.state.approvals.push(json!({"id":99}));
        actor
            .submit_mode(7, "adjust".into(), Vec::new(), SubmissionMode::Steer)
            .unwrap();
        assert!(
            matches!(&actor.pending[&1].kind, Pending::Steer { submission: 7, turn, .. } if turn == "turn-1")
        );
        assert_eq!(actor.state.accepted_submission, None);
        actor
            .message(json!({"id":1,"error":{"message":"no active turn"}}))
            .unwrap();
        assert_eq!(actor.state.rejected_submission, Some(7));
        assert!(actor.queue_paused);
        assert!(!actor.queue_ready);
        assert_eq!(actor.state.status, Status::Working);
        assert_eq!(actor.state.turn_id.as_deref(), Some("turn-1"));
        assert_eq!(actor.state.approvals.len(), 1);
        actor
            .submit_mode(8, "adjust again".into(), Vec::new(), SubmissionMode::Steer)
            .unwrap();
        assert_eq!(actor.state.error, None);
        actor
            .message(json!({"id":2,"result":{"turnId":"turn-1"}}))
            .unwrap();
        assert_eq!(actor.state.accepted_submission, Some(8));
        assert_eq!(actor.state.error, None);
    }
    #[test]
    fn rejected_submission_is_exact_and_survives_retry_until_new_outcome() {
        let mut actor = actor();
        actor.state.saved.draft = "Retry my draft".into();
        actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
        // The actor may reject after the UI queued against a stale idle snapshot.
        assert!(actor
            .action(Action::Submit {
                id: 41,
                text: "Retry my draft".into(),
                images: actor.state.saved.draft_images.clone()
            })
            .is_err());
        assert_eq!(actor.state.rejected_submission, Some(41));
        assert_eq!(actor.state.saved.draft, "Retry my draft");
        assert_eq!(actor.state.saved.draft_images.len(), 1);
        let stale_failure = actor.state.clone();
        actor.pending.insert(
            10,
            Request {
                kind: Pending::Turn {
                    submission: Some(42),
                    text: actor.state.saved.draft.clone(),
                    images: actor.state.saved.draft_images.clone(),
                },
                deadline: Instant::now(),
            },
        );
        // Queued/retrying state keeps the prior outcome; it cannot match token42.
        assert_eq!(stale_failure.rejected_submission, Some(41));
        assert_ne!(actor.state.rejected_submission, Some(42));
        actor
            .message(json!({"id":10,"error":{"message":"image temporarily unavailable"}}))
            .unwrap();
        assert_eq!(actor.state.rejected_submission, Some(42));
        assert_eq!(actor.state.saved.draft, "Retry my draft");
        assert_eq!(actor.state.saved.draft_images.len(), 1);
        actor.pending.insert(
            11,
            Request {
                kind: Pending::Turn {
                    submission: Some(43),
                    text: actor.state.saved.draft.clone(),
                    images: actor.state.saved.draft_images.clone(),
                },
                deadline: Instant::now(),
            },
        );
        assert_eq!(actor.state.rejected_submission, Some(42));
        actor
            .message(json!({"id":11,"result":{"turn":{"id":"retried"}}}))
            .unwrap();
        assert_eq!(actor.state.accepted_submission, Some(43));
        assert_eq!(actor.state.rejected_submission, Some(42));
        assert!(actor.state.saved.draft.is_empty());
        assert!(actor.state.saved.draft_images.is_empty());
    }
    #[test]
    fn unrelated_turn_ack_cannot_acknowledge_a_ui_submission() {
        let mut actor = actor();
        actor.state.saved.draft = "UI draft".into();
        actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
        actor.pending.insert(
            1,
            Request {
                kind: Pending::Turn {
                    submission: None,
                    text: actor.state.saved.draft.clone(),
                    images: actor.state.saved.draft_images.clone(),
                },
                deadline: Instant::now(),
            },
        );
        actor
            .message(json!({"id":1,"result":{"turn":{"id":"cli-turn"}}}))
            .unwrap();
        assert_eq!(actor.state.accepted_submission, None);
        actor.pending.insert(
            2,
            Request {
                kind: Pending::Turn {
                    submission: Some(41),
                    text: actor.state.saved.draft.clone(),
                    images: actor.state.saved.draft_images.clone(),
                },
                deadline: Instant::now(),
            },
        );
        actor.state.saved.draft = "UI draft".into();
        actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
        actor
            .message(json!({"id":2,"error":{"message":"busy"}}))
            .unwrap();
        assert_ne!(actor.state.accepted_submission, Some(41));
        assert_eq!(actor.state.saved.draft, "UI draft");
        assert_eq!(actor.state.saved.draft_images.len(), 1);
        actor.pending.insert(
            3,
            Request {
                kind: Pending::Turn {
                    submission: Some(42),
                    text: actor.state.saved.draft.clone(),
                    images: actor.state.saved.draft_images.clone(),
                },
                deadline: Instant::now(),
            },
        );
        actor
            .message(json!({"id":3,"result":{"turn":{"id":"ui-turn"}}}))
            .unwrap();
        assert_eq!(actor.state.accepted_submission, Some(42));
        assert_ne!(actor.state.accepted_submission, Some(41));
        // An untracked CLI response must not erase the UI acknowledgement before
        // the main thread coalesces queued snapshots.
        actor.pending.insert(
            4,
            Request {
                kind: Pending::Turn {
                    submission: None,
                    text: actor.state.saved.draft.clone(),
                    images: actor.state.saved.draft_images.clone(),
                },
                deadline: Instant::now(),
            },
        );
        actor
            .message(json!({"id":4,"result":{"turn":{"id":"later-cli-turn"}}}))
            .unwrap();
        assert_eq!(actor.state.accepted_submission, Some(42));
    }
    #[test]
    fn prompt_acknowledgement_changes_only_after_successful_turn_response() {
        let mut actor = actor();
        let before_submission = actor.state.clone();
        actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
        actor.pending.insert(
            8,
            Request {
                kind: Pending::Turn {
                    submission: None,
                    text: actor.state.saved.draft.clone(),
                    images: actor.state.saved.draft_images.clone(),
                },
                deadline: Instant::now(),
            },
        );
        actor.message(json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}})).unwrap();
        assert_eq!(actor.state.accepted_prompts, 0);
        assert_eq!(before_submission.accepted_prompts, 0);
        actor
            .message(json!({"id":8,"error":{"message":"image rejected"}}))
            .unwrap();
        assert_eq!(actor.state.accepted_prompts, 0);
        assert_eq!(actor.state.saved.draft_images.len(), 1);
        actor.pending.insert(
            9,
            Request {
                kind: Pending::Turn {
                    submission: None,
                    text: actor.state.saved.draft.clone(),
                    images: actor.state.saved.draft_images.clone(),
                },
                deadline: Instant::now(),
            },
        );
        actor
            .message(json!({"id":9,"result":{"turn":{"id":"turn-accepted"}}}))
            .unwrap();
        assert_eq!(actor.state.accepted_prompts, 1);
        assert!(actor.state.saved.draft_images.is_empty());
        // Duplicate/unmatched responses cannot acknowledge another submission.
        actor
            .message(json!({"id":9,"result":{"turn":{"id":"turn-accepted"}}}))
            .unwrap();
        assert_eq!(actor.state.accepted_prompts, 1);
    }
    #[test]
    fn pasted_images_validate_and_survive_rejected_turn_until_ack() {
        assert!(validate_prompt("", &["data:image/png;base64,aGVsbG8=".into()]).is_ok());
        assert!(validate_prompt("", &[]).is_err());
        assert!(validate_images(&["file:///private/image.png".into()]).is_err());
        assert!(validate_images(&["https://example.test/image.png".into()]).is_err());
        assert!(validate_images(&["data:image/png;base64,INVALID!".into()]).is_err());
        assert!(validate_images(&["data:image/png;base64,a=AA".into()]).is_err());
        let oversized =
            base64::engine::general_purpose::STANDARD.encode(vec![0; 20 * 1024 * 1024 + 1]);
        assert!(validate_images(&[format!("data:image/png;base64,{oversized}")]).is_err());
        let mut actor = actor();
        actor.state.saved.draft = "Keep draft".into();
        actor.state.saved.draft_images = vec!["data:image/png;base64,aGVsbG8=".into()];
        actor.pending.insert(
            9,
            Request {
                kind: Pending::Turn {
                    submission: None,
                    text: actor.state.saved.draft.clone(),
                    images: actor.state.saved.draft_images.clone(),
                },
                deadline: Instant::now(),
            },
        );
        actor
            .message(json!({"id":9,"error":{"message":"model unavailable"}}))
            .unwrap();
        assert_eq!(actor.state.saved.draft, "Keep draft");
        assert_eq!(actor.state.saved.draft_images.len(), 1);
        actor.pending.insert(
            10,
            Request {
                kind: Pending::Turn {
                    submission: None,
                    text: actor.state.saved.draft.clone(),
                    images: actor.state.saved.draft_images.clone(),
                },
                deadline: Instant::now(),
            },
        );
        actor
            .message(json!({"id":10,"result":{"turn":{"id":"accepted"}}}))
            .unwrap();
        assert!(actor.state.saved.draft.is_empty());
        assert!(actor.state.saved.draft_images.is_empty());
    }
    #[test]
    fn live_reasoning_tools_and_plan_updates_remain_readable() {
        let mut state = state();
        for delta in ["Check ", "repository"] {
            assert!(state.apply(&json!({"method":"item/reasoning/summaryTextDelta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"reason","summaryIndex":0,"delta":delta}})));
        }
        assert!(state.transcript().contains("Check repository"));
        assert!(state.apply(&json!({"method":"item/mcpToolCall/progress","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"tool","message":"Fetching documentation"}})));
        assert!(state.transcript().contains("Fetching documentation"));
        assert!(state.apply(&json!({"method":"turn/plan/updated","params":{"threadId":"thread-1","turnId":"turn-1","plan":[{"step":"Inspect repository","status":"completed"},{"step":"Implement","status":"inProgress"}]}})));
        assert!(state.transcript().contains("[x] Inspect repository"));
        assert!(state.transcript().contains("[ ] Implement"));
    }
    #[test]
    fn plan_updates_store_a_structured_checklist() {
        let mut state = state();
        assert!(state.apply(&json!({"method":"turn/plan/updated","params":{"threadId":"thread-1","turnId":"turn-1","explanation":"Fix the bug","plan":[{"step":"Inspect","status":"completed"},{"step":"Implement","status":"in_progress"},{"step":"Test","status":"pending"}]}})));
        let item = state
            .saved
            .items
            .iter()
            .find(|item| item["id"] == "grove-plan-turn-1")
            .unwrap();
        assert_eq!(item["type"], "plan");
        assert_eq!(item["explanation"], "Fix the bug");
        assert_eq!(
            item["plan"],
            json!([
                {"id":"step-0","step":"Inspect","status":"completed"},
                {"id":"step-1","step":"Implement","status":"inProgress"},
                {"id":"step-2","step":"Test","status":"pending"}
            ])
        );
        assert!(item["text"].as_str().unwrap().contains("- [x] Inspect"));
        assert!(state.apply(&json!({"method":"turn/plan/updated","params":{"threadId":"thread-1","turnId":"turn-1","plan":[{"step":"Inspect","status":"completed"},{"step":"Implement","status":"inProgress"}]}})));
        let item = state
            .saved
            .items
            .iter()
            .find(|item| item["id"] == "grove-plan-turn-1")
            .unwrap();
        assert!(item["explanation"].is_null());
        assert_eq!(item["plan"][1]["status"], "inProgress");
        let list = crate::tasks::current(&state.saved.items).unwrap();
        assert_eq!(list.done(), 1);
        assert_eq!(list.now_label(), Some("Implement"));
    }
    #[test]
    fn settled_background_commands_keep_streaming_during_next_turn() {
        let mut state = state();
        state.apply(&json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1","item":{"id":"background","type":"commandExecution","command":"sleep 1; echo done","aggregatedOutput":""}}}));
        state.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}}));
        state.apply(&json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-2"}}}));
        assert!(state.apply(&json!({"method":"item/commandExecution/outputDelta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"background","delta":"done"}})));
        assert!(state.transcript().contains("done"));
        assert!(!state.apply(&json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"stale-message","delta":"stale"}})));
        assert_eq!(state.turn_id.as_deref(), Some("turn-2"));
        assert_eq!(state.status, Status::Working);
    }
    #[test]
    fn routes_only_matching_thread_and_turn() {
        let mut state = state();
        assert!(!state.apply(&json!({"method":"turn/completed","params":{"threadId":"other","turn":{"id":"turn-1","status":"completed"}}})));
        assert!(!state.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"old-turn","status":"completed"}}})));
        assert_eq!(state.status, Status::Working);
        assert_eq!(state.turn_id.as_deref(), Some("turn-1"));
    }
    #[test]
    fn completed_item_replaces_stream_without_duplicate_text() {
        let mut state = state();
        for delta in ["Hello ", "world"] {
            assert!(state.apply(&json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","delta":delta}})));
        }
        state.apply(&json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"id":"item-1","type":"agentMessage","text":"Hello world"}}}));
        assert_eq!(
            state
                .saved
                .items
                .iter()
                .find(|item| item["id"] == "item-1")
                .unwrap()["groveCompleted"],
            true
        );
        assert_eq!(state.saved.items.len(), 1);
        assert_eq!(state.transcript(), "Hello world");
    }
    #[test]
    fn command_output_stream_merges_final_output() {
        let mut state = state();
        state.apply(&json!({"method":"item/commandExecution/outputDelta","params":{"threadId":"thread-1","turnId":"turn-1","itemId":"cmd","delta":"output"}}));
        state.apply(&json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"id":"cmd","type":"commandExecution","command":"pwd","aggregatedOutput":"output"}}}));
        assert_eq!(state.saved.items.len(), 1);
        assert_eq!(state.transcript(), "pwd\noutput");
    }
    #[test]
    fn completed_turn_expires_approvals_and_clears_active_turn() {
        let mut state = state();
        state.approvals.push(json!({"id":1}));
        state.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"interrupted"}}}));
        assert_eq!(state.status, Status::Interrupted);
        assert!(state.approvals.is_empty());
        assert!(state.turn_id.is_none());
    }
    #[test]
    fn disconnect_invalidates_all_permissions() {
        let mut state = state();
        state.approvals.push(json!({"id":1}));
        state.fail("lost connection".into());
        assert_eq!(state.status, Status::Disconnected);
        assert!(state.approvals.is_empty());
        assert!(state.turn_id.is_none());
    }
    #[test]
    fn failed_state_with_active_turn_cannot_send_duplicate() {
        let mut actor = actor();
        actor.state.status = Status::Failed;
        assert!(actor
            .prompt("second".into())
            .unwrap_err()
            .contains("current turn"));
    }
    #[test]
    fn pending_start_blocks_second_prompt_without_turn_identity() {
        let mut actor = actor();
        actor.state.status = Status::Idle;
        actor.state.turn_id = None;
        actor.pending.insert(
            1,
            Request {
                kind: Pending::Turn {
                    submission: None,
                    text: actor.state.saved.draft.clone(),
                    images: actor.state.saved.draft_images.clone(),
                },
                deadline: Instant::now(),
            },
        );
        assert!(actor.prompt("second".into()).is_err());
    }
    #[test]
    fn expired_approval_is_rejected() {
        let mut actor = actor();
        assert!(actor
            .action(Action::Approval(json!("old"), true))
            .unwrap_err()
            .contains("expired"));
    }
    #[cfg(unix)]
    #[test]
    fn approval_routes_exact_turn_and_unsupported_request_fails_closed() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.message(json!({"id":7,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","turnId":"old"}})).unwrap();
        assert!(actor.state.approvals.is_empty());
        actor.message(json!({"id":8,"method":"item/tool/requestUserInput","params":{"threadId":"thread-1","turnId":"turn-1"}})).unwrap();
        assert!(actor.state.approvals.is_empty());
        assert!(actor
            .state
            .error
            .as_deref()
            .unwrap()
            .contains("unsupported"));
        actor.message(json!({"id":9,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","turnId":"turn-1"}})).unwrap();
        assert_eq!(actor.state.approvals.len(), 1);
        actor.action(Action::Approval(json!(9), false)).unwrap();
        assert!(actor.state.approvals.is_empty());
        assert_eq!(actor.state.status, Status::Working);
    }
    #[test]
    fn child_approval_requires_its_exact_turn_and_survives_parent_completion() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.interaction_children.register(
            "thread-1",
            "thread-1",
            "child-1",
            Some("Reviewer"),
            None,
        );
        actor
            .interaction_children
            .start("child-1", Some("child-turn"));
        actor.message(json!({"id":70,"method":"item/commandExecution/requestApproval","params":{"threadId":"child-1"}})).unwrap();
        assert!(actor.state.approvals.is_empty());
        actor.message(json!({"id":71,"method":"item/commandExecution/requestApproval","params":{"threadId":"child-1","turnId":"stale-turn"}})).unwrap();
        assert!(actor.state.approvals.is_empty());
        actor.message(json!({"id":72,"method":"item/commandExecution/requestApproval","params":{"threadId":"child-1","turnId":"child-turn"}})).unwrap();
        assert_eq!(actor.state.approvals.len(), 1);
        assert_eq!(
            actor.state.interaction_agent(&json!(72)),
            Some("Reviewer · child-1")
        );
        actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
        assert_eq!(actor.state.approvals.len(), 1);
        assert_eq!(actor.state.status, Status::Waiting);
        actor.message(json!({"method":"turn/completed","params":{"threadId":"child-1","turn":{"id":"child-turn","status":"completed"}}})).unwrap();
        assert!(actor.state.approvals.is_empty());
        assert!(actor.state.interaction_owners.is_empty());
        assert_eq!(actor.state.status, Status::Idle);
    }
    fn async_question_message() -> Value {
        // Recorded request_user_input_async AgentMessage, exposed by app-server as
        // camelCase ThreadItem metadata (Codex 0.163.0 schema).
        json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{
            "type":"agentMessage","id":"async-question","text":"Which changes?\n- All\n- Composer",
            "phase":"final_answer","delivery":"async","questions":[
                {"title":"Which changes?","options":["All","Composer"]},
                {"title":"Any details?","options":null}
            ]
        }}})
    }
    #[test]
    fn async_question_metadata_survives_completion_and_history_without_duplicates() {
        let mut state = state();
        let message = async_question_message();
        state.apply(&message);
        state.apply(&message);
        assert_eq!(state.questions.len(), 1);
        let request = &state.questions[0];
        assert_eq!(request.questions.len(), 2);
        assert_eq!(request.questions[0].options[1].label, "Composer");
        assert!(request
            .questions
            .iter()
            .all(|question| question.allow_custom));
        assert!(request.questions[1].options.is_empty());
        state.apply(&json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}}));
        assert_eq!(state.status, Status::Waiting);
        assert_eq!(state.questions.len(), 1);
        state.questions.clear();
        state.restore_async_questions();
        assert_eq!(state.questions.len(), 1);
        state.resolve_async_question(&json!("async-question"));
        state.apply(&message);
        state.restore_async_questions();
        assert!(state.questions.is_empty());
    }
    #[test]
    fn async_questions_reject_invalid_metadata_and_survive_unrelated_user_reply() {
        let mut state = state();
        let mut message = async_question_message();
        message["params"]["item"]["questions"][0]["options"] = json!(["All", "All"]);
        state.apply(&message);
        assert!(state.questions.is_empty());
        state.apply(&async_question_message());
        state.apply(&json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"type":"userMessage","id":"reply","content":[{"type":"text","text":"All"}]}}}));
        assert_eq!(state.questions.len(), 1);
        state.questions.clear();
        state.restore_async_questions();
        state.apply(&async_question_message());
        assert_eq!(state.questions.len(), 1);
    }
    #[cfg(unix)]
    #[test]
    fn async_question_answers_use_user_input_preserve_draft_and_retry() {
        use std::os::unix::process::CommandExt;
        let mut actor = actor();
        let mut child = Command::new("cat")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let input = child.stdin.take().unwrap();
        let (_, lines) = mpsc::channel();
        actor.process = Some(Process {
            child,
            input,
            lines,
        });
        actor.state.saved.draft = "Unsent draft".into();
        actor.state.saved.draft_images = vec!["draft.png".into()];
        actor.message(async_question_message()).unwrap();
        let answers = HashMap::from([
            ("0".into(), vec!["All".into()]),
            ("1".into(), vec!["Keep it tidy".into()]),
        ]);
        actor
            .action(Action::AnswerQuestions(
                json!("async-question"),
                Some(answers.clone()),
            ))
            .unwrap();
        assert!(actor
            .action(Action::AnswerQuestions(
                json!("async-question"),
                Some(answers.clone())
            ))
            .is_err());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let sent: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(sent["method"], "turn/start");
        assert_eq!(sent["params"]["threadId"], "thread-1");
        assert_eq!(
            sent["params"]["input"][0]["text"],
            "Which changes?\nAll\n\nAny details?\nKeep it tidy"
        );
        assert_eq!(
            actor.state.questions.len(),
            1,
            "retain until acknowledgement"
        );
        // An echoed user message can arrive before the request acknowledgement.
        // It must neither dismiss this card nor turn a rejection into data loss.
        actor.message(json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"type":"userMessage","id":"echo","content":[{"type":"text","text":"Which changes?\nAll"}]}}})).unwrap();
        actor
            .message(json!({"id":sent["id"],"error":{"message":"Try again"}}))
            .unwrap();
        assert_eq!(actor.state.status, Status::Waiting);
        assert_eq!(actor.state.questions.len(), 1);
        assert!(actor.state.question_answer_acks.is_empty());
        actor.state.turn_id = None;
        actor
            .action(Action::AnswerQuestions(
                json!("async-question"),
                Some(answers),
            ))
            .unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let retry: Value = serde_json::from_str(&line).unwrap();
        actor
            .message(json!({"id":retry["id"],"result":{"turn":{"id":"turn-2"}}}))
            .unwrap();
        assert!(actor.state.questions.is_empty());
        assert_eq!(actor.state.turn_id.as_deref(), Some("turn-2"));
        assert_eq!(actor.state.question_answer_acks.len(), 1);
        let ack = &actor.state.question_answer_acks[0];
        assert_eq!(ack.id, json!("async-question"));
        assert_eq!(ack.thread_id.as_deref(), Some("thread-1"));
        assert_eq!(
            ack.turn_id, None,
            "capture submission scope, not acknowledged turn"
        );
        assert_eq!(actor.state.saved.draft, "Unsent draft");
        assert_eq!(actor.state.saved.draft_images, vec!["draft.png"]);
        actor.state.turn_id = Some("turn-1".into());
        let mut new_question = async_question_message();
        new_question["params"]["item"]["id"] = json!("dismiss");
        actor.message(new_question.clone()).unwrap();
        actor
            .action(Action::AnswerQuestions(json!("dismiss"), None))
            .unwrap();
        actor.message(new_question).unwrap();
        assert!(actor.state.questions.is_empty());
        assert!(
            actor.pending.is_empty(),
            "dismissal must not send a provider RPC"
        );
        let mut cancelled = async_question_message();
        cancelled["params"]["item"]["id"] = json!("cancelled-in-flight");
        actor.message(cancelled).unwrap();
        actor
            .action(Action::AnswerQuestions(
                json!("cancelled-in-flight"),
                Some(HashMap::from([
                    ("0".into(), vec!["All".into()]),
                    ("1".into(), vec!["Details".into()]),
                ])),
            ))
            .unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let submitted: Value = serde_json::from_str(&line).unwrap();
        actor
            .state
            .resolve_async_question(&json!("cancelled-in-flight"));
        // A provider can reuse its wire id for a new request before the old reply arrives.
        let mut replacement_item = async_question_message()["params"]["item"].clone();
        replacement_item["id"] = json!("cancelled-in-flight");
        let replacement = QuestionRequest::from_async_message(&replacement_item).unwrap();
        assert!(!replacement.turn_bound());
        actor.state.questions.push(replacement);
        actor
            .message(json!({"id":submitted["id"],"result":{"turn":{"id":"turn-3"}}}))
            .unwrap();
        assert_eq!(
            actor.state.question_answer_acks.len(),
            1,
            "late success after cancellation is not an answer receipt"
        );
        assert_eq!(
            actor.state.questions.len(),
            1,
            "late success must not remove the replacement request"
        );
        assert_eq!(actor.state.questions[0].id, json!("cancelled-in-flight"));
    }
    #[cfg(unix)]
    #[test]
    fn question_requests_roundtrip_grouped_answers_and_expire() {
        use std::os::unix::process::CommandExt;
        let mut actor = actor();
        let mut child = Command::new("cat")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let input = child.stdin.take().unwrap();
        let (_, lines) = mpsc::channel();
        actor.process = Some(Process {
            child,
            input,
            lines,
        });
        let request = json!({"id":81,"method":"item/tool/requestUserInput","params":{
        "threadId":"thread-1","turnId":"turn-1","itemId":"tool","isBlocking":true,
        "questions":[
            {"id":"size","header":"Size","question":"Which size?","options":[{"label":"Small","description":"Compact"}]},
            {"id":"token","header":"Token","question":"Enter token","isSecret":true,"options":null}
        ]}});
        actor.message(request.clone()).unwrap();
        actor.message(request).unwrap();
        assert_eq!(actor.state.questions.len(), 1);
        assert_eq!(actor.state.status, Status::Waiting);
        assert!(actor.state.approvals.is_empty());
        assert!(actor
            .action(Action::AnswerQuestions(
                json!(81),
                Some(HashMap::from([("size".into(), vec!["Small".into()])]))
            ))
            .is_err());
        assert_eq!(actor.state.questions.len(), 1);
        let answers = HashMap::from([
            ("size".into(), vec!["Small".into()]),
            ("token".into(), vec!["private-token".into()]),
        ]);
        actor
            .action(Action::AnswerQuestions(json!(81), Some(answers)))
            .unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap(),
            json!({"id":81,"result":{"answers":{"size":{"answers":["Small"]},"token":{"answers":["private-token"]}}}})
        );
        assert!(!serde_json::to_string(&actor.state.saved)
            .unwrap()
            .contains("private-token"));
        assert_eq!(actor.state.question_answer_acks.len(), 1);
        assert_eq!(actor.state.question_answer_acks[0].id, json!(81));
        assert_eq!(actor.state.question_answer_acks[0].sequence, 1);
        assert!(!format!("{:?}", actor.state.question_answer_acks).contains("private-token"));
        assert!(!serde_json::to_string(&actor.state.saved)
            .unwrap()
            .contains("question_answer_acks"));
        assert!(actor.state.questions.is_empty());
        assert!(actor
            .action(Action::AnswerQuestions(json!(81), None))
            .is_err());
        actor.message(json!({"id":82,"method":"item/tool/requestUserInput","params":{"threadId":"thread-1","turnId":"turn-1","questions":[{"id":"q","question":"Continue?"}]}})).unwrap();
        actor
            .action(Action::AnswerQuestions(json!(82), None))
            .unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap(),
            json!({"id":82,"result":{"answers":{}}})
        );
        assert_eq!(
            actor.state.question_answer_acks.len(),
            1,
            "cancellation is not an answer"
        );
        actor.message(json!({"id":83,"method":"item/tool/requestUserInput","params":{"threadId":"thread-1","turnId":"turn-1","questions":[{"id":"q","question":"Continue?"}]}})).unwrap();
        actor.message(json!({"method":"serverRequest/resolved","params":{"threadId":"thread-1","requestId":83}})).unwrap();
        assert!(actor
            .action(Action::AnswerQuestions(json!(83), None))
            .is_err());
        actor.message(json!({"id":84,"method":"item/tool/requestUserInput","params":{"threadId":"thread-1","turnId":"turn-1","questions":[{"id":"q","question":"Continue?"}]}})).unwrap();
        actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
        assert!(actor.state.questions.is_empty());
        assert!(actor
            .action(Action::AnswerQuestions(json!(84), None))
            .is_err());
    }
    #[test]
    fn failed_question_transport_does_not_acknowledge_or_remove_request() {
        let mut actor = actor();
        actor.state.questions.push(
            QuestionRequest::from_codex(
                json!(91),
                &json!({"questions":[{"id":"q","question":"Name?"}]}),
            )
            .unwrap(),
        );
        assert!(actor
            .action(Action::AnswerQuestions(
                json!(91),
                Some(HashMap::from([("q".into(), vec!["Alice".into()]),]))
            ))
            .is_err());
        assert!(actor.state.question_answer_acks.is_empty());
        assert_eq!(actor.state.questions.len(), 1);
        assert!(actor.state.questions[0].turn_bound());
    }
    #[test]
    fn question_acknowledgements_are_ephemeral_across_restart_and_new_conversation() {
        let mut snapshot = state();
        snapshot
            .question_answer_acks
            .push(snapshot.question_answer_ack(json!(91)));
        let serialized = serde_json::to_value(&snapshot.saved).unwrap();
        assert!(serialized.get("question_answer_acks").is_none());
        let restored: SavedSession = serde_json::from_value(serialized).unwrap();
        assert!(Snapshot::new(restored).question_answer_acks.is_empty());
        assert!(Snapshot::new(snapshot.saved.fresh_conversation())
            .question_answer_acks
            .is_empty());
    }
    #[test]
    fn question_validation_rejects_ambiguous_ids_and_unoffered_answers() {
        for questions in [
            json!([]),
            json!([{"id":"","question":"q"}]),
            json!([{"id":"q","question":"q"},{"id":"q","question":"other"}]),
            json!([{"id":"q","question":"q","options":[{"label":"A"},{"label":"A"}]}]),
        ] {
            assert!(
                QuestionRequest::from_codex(json!(1), &json!({"questions":questions})).is_err()
            );
        }
        let request = QuestionRequest::from_codex(
            json!(1),
            &json!({"questions":[{"id":"q","question":"q","options":[{"label":"A"}]}]}),
        )
        .unwrap();
        assert!(request
            .validate(&HashMap::from([("q".into(), vec!["unoffered".into()])]))
            .is_err());
        assert!(request
            .validate(&HashMap::from([("q".into(), vec!["A".into(), "A".into()])]))
            .is_err());
    }
    #[test]
    fn early_interrupt_error_retries_but_has_fixed_bound() {
        let mut actor = actor();
        actor.pending.insert(
            1,
            Request {
                kind: Pending::Interrupt {
                    attempt: 0,
                    turn: "turn-1".into(),
                },
                deadline: Instant::now(),
            },
        );
        actor
            .message(json!({"id":1,"error":{"message":"no active turn to interrupt"}}))
            .unwrap();
        assert_eq!(actor.interrupt_retry.as_ref().unwrap().1, 1);
        actor.interrupt_retry = None;
        actor.pending.insert(
            2,
            Request {
                kind: Pending::Interrupt {
                    attempt: 4,
                    turn: "turn-1".into(),
                },
                deadline: Instant::now(),
            },
        );
        actor
            .message(json!({"id":2,"error":{"message":"no active turn to interrupt"}}))
            .unwrap();
        assert!(actor.interrupt_retry.is_none());
        assert!(actor.state.error.is_some());
    }
    #[cfg(unix)]
    #[test]
    fn resume_in_progress_preserves_active_turn() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.pending.insert(
            1,
            Request {
                kind: Pending::Thread,
                deadline: Instant::now(),
            },
        );
        actor.message(json!({"id":1,"result":{"thread":{"id":"thread-1","status":{"type":"active","activeFlags":[]},"turns":[{"id":"resumed-turn","status":"inProgress","items":[]}]}}})).unwrap();
        assert_eq!(actor.state.turn_id.as_deref(), Some("resumed-turn"));
        assert_eq!(actor.state.status, Status::Working);
        assert!(actor.prompt("duplicate".into()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn failed_prompt_preserves_draft_and_is_never_replayed() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.state.status = Status::Idle;
        actor.state.turn_id = None;
        actor.prompt("keep this text".into()).unwrap();
        actor.action(Action::Draft(String::new())).unwrap();
        actor
            .message(json!({"id":1,"error":{"message":"provider rejected prompt"}}))
            .unwrap();
        assert_eq!(actor.state.saved.draft, "keep this text");
        assert!(actor.initial.is_none());
        assert!(actor.pending.is_empty());
    }
    #[test]
    fn launch_preserves_defaults_and_only_maps_explicit_permission_bypass() {
        assert_eq!(launch_options(&[]).unwrap(), (Vec::new(), None));
        let args = crate::agent::Agent::Codex.launch_args(false, false);
        assert!(launch_options(&args).unwrap().0.is_empty());
        let args = crate::agent::Agent::Codex.launch_args(true, false);
        let (args, _) = launch_options(&args).unwrap();
        assert!(args.contains(&"sandbox_mode=\"danger-full-access\"".into()));
        assert!(args.contains(&"approval_policy=\"never\"".into()));
        assert!(launch_options(&["--add-dir".into(), "/extra".into()]).is_ok());
        assert!(launch_options(&["--add-dir".into()]).is_err());
    }
    #[test]
    fn recovery_is_private_atomic_and_isolates_corrupt_files() {
        let dir = tempfile::tempdir().unwrap();
        let saved = saved();
        persist_in_dir(&saved, dir.path()).unwrap();
        fs_err::write(dir.path().join("broken.json"), "{").unwrap();
        let result = load_from_dir(dir.path()).unwrap();
        assert_eq!(result.sessions.len(), 1);
        assert_eq!(result.warnings.len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs_err::metadata(dir.path().join("test-native-1.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn unsafe_recovery_key_cannot_escape_directory() {
        let dir = tempfile::tempdir().unwrap();
        let mut saved = saved();
        saved.key = "../escape".into();
        assert!(persist_in_dir(&saved, dir.path()).is_err());
    }
    #[test]
    fn old_recovery_without_new_optional_fields_is_compatible() {
        let mut value = serde_json::to_value(saved()).unwrap();
        for key in ["draft", "config_args", "title", "context", "work_periods"] {
            value.as_object_mut().unwrap().remove(key);
        }
        let restored: SavedSession = serde_json::from_value(value).unwrap();
        assert!(restored.draft.is_empty());
        assert!(restored.config_args.is_empty());
        assert!(restored.context.is_none());
        assert!(restored.work_periods.is_empty());
    }
    #[test]
    fn stop_before_spawn_prevents_process_creation() {
        let mut actor = actor();
        actor.stopping.store(true, Ordering::SeqCst);
        assert!(actor.start().unwrap_err().contains("closing"));
        assert!(actor.process.is_none());
    }
    #[cfg(unix)]
    #[test]
    fn stop_terminates_process_even_with_retained_handle_clones() {
        let mut process = fixture_process();
        let (handle, _actions) = Handle::channel();
        handle.1.store(process.child.id(), Ordering::SeqCst);
        let retained = handle.clone();
        handle.stop(false);
        assert!(retained.2.load(Ordering::SeqCst));
        assert!(!process.child.wait().unwrap().success());
    }
    #[test]
    fn reconnect_start_failure_is_retryable_disconnected_state() {
        let mut actor = actor();
        actor.state.status = Status::Failed;
        actor.state.turn_id = None;
        actor.args = vec!["--unsupported".into()];
        actor.action(Action::Reconnect).unwrap();
        assert_eq!(actor.state.status, Status::Disconnected);
        assert!(actor.process.is_none());
        assert!(actor.state.error.is_some());
    }
    #[test]
    fn resumed_history_retains_known_receipt_time_and_leaves_unknown_time_absent() {
        let mut actor = actor();
        actor.state.saved.items =
            vec![json!({"id":"known", "type":"userMessage", "groveReceivedAt":123})];
        actor.pending.insert(
            1,
            Request {
                kind: Pending::Thread,
                deadline: Instant::now(),
            },
        );
        actor.message(json!({"id":1,"result":{"thread":{"id":"thread-1","turns":[{"id":"old-turn","status":"completed","items":[
            {"id":"known","type":"userMessage","content":[]},
            {"id":"unknown","type":"agentMessage","text":"historical response"}
        ]}]}}})).unwrap();
        assert_eq!(actor.state.saved.items[0]["groveReceivedAt"], 123);
        assert!(actor.state.saved.items[1].get("groveReceivedAt").is_none());
    }

    #[test]
    fn historical_unfinished_turn_is_not_a_live_process_after_resume() {
        let mut actor = actor();
        actor.pending.insert(
            1,
            Request {
                kind: Pending::Thread,
                deadline: Instant::now(),
            },
        );
        actor.message(json!({"id":1,"result":{"thread":{"id":"thread-1","status":{"type":"idle"},"turns":[{"id":"stale","status":"inProgress","items":[]}]}}})).unwrap();
        assert_eq!(actor.state.status, Status::Interrupted);
        assert!(actor.state.turn_id.is_none());
    }
    #[test]
    fn resolved_server_request_expires_approval_immediately() {
        let mut state = state();
        state.status = Status::Waiting;
        state.approvals.push(json!({"id":7}));
        state.apply(&json!({"method":"serverRequest/resolved","params":{"threadId":"thread-1","requestId":7}}));
        assert!(state.approvals.is_empty());
        assert_eq!(state.status, Status::Working);
    }
    #[test]
    fn provider_title_overrides_prompt_fallback_and_survives_recovery() {
        let mut state = state();
        state.apply(&json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"id":"user-1","type":"userMessage","content":[{"type":"text","text":"Fix session search\nPreserve keyboard navigation"}]}}}));
        assert_eq!(state.saved.title.as_deref(), Some("Fix session search"));
        state.apply(&json!({"method":"thread/name/updated","params":{"threadId":"thread-1","threadName":"Search keyboard navigation"}}));
        assert_eq!(
            state.saved.title.as_deref(),
            Some("Search keyboard navigation")
        );
        let restored: SavedSession =
            serde_json::from_slice(&serde_json::to_vec(&state.saved).unwrap()).unwrap();
        assert_eq!(restored.title, state.saved.title);
        assert!(!state.apply(&json!({"method":"thread/name/updated","params":{"threadId":"other","threadName":"Wrong session"}})));
        assert_eq!(
            state.saved.title.as_deref(),
            Some("Search keyboard navigation")
        );
        state.apply(&json!({"method":"thread/name/updated","params":{"threadId":"thread-1"}}));
        assert_eq!(state.saved.title.as_deref(), Some("Fix session search"));
    }
    #[test]
    fn old_history_derives_a_bounded_unicode_prompt_title() {
        let mut saved = saved();
        saved.items.push(json!({"id":"user","type":"userMessage","content":[{"type":"text","text":format!("\n{}\nmore", "界".repeat(100))}]}));
        let state = Snapshot::new(saved);
        assert_eq!(state.saved.title.as_ref().unwrap().chars().count(), 80);
        assert_eq!(state.saved.title.as_deref(), Some("界".repeat(80).as_str()));
    }
    #[test]
    fn resumed_thread_uses_provider_name_without_changing_stable_identity() {
        let mut actor = actor();
        actor.pending.insert(
            1,
            Request {
                kind: Pending::Thread,
                deadline: Instant::now(),
            },
        );
        actor.message(json!({"id":1,"result":{"thread":{"id":"thread-1","name":"Provider restored title","turns":[]}}})).unwrap();
        assert_eq!(
            actor.state.saved.title.as_deref(),
            Some("Provider restored title")
        );
        assert_eq!(actor.state.saved.key, "test-native-1");
        assert_eq!(actor.state.saved.label, "Codex");
    }
    #[cfg(unix)]
    #[test]
    fn additional_controls_roundtrip_exact_wire_and_lifecycle() {
        use std::os::unix::process::CommandExt;
        fn read(reader: &mut impl BufRead) -> Value {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        }
        let mut actor = actor();
        let mut child = Command::new("cat")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let input = child.stdin.take().unwrap();
        let (_, lines) = mpsc::channel();
        actor.process = Some(Process {
            child,
            input,
            lines,
        });
        actor.message(json!({"id":901,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","turnId":"turn-1","kind":"writeStdin","command":"yes","proposedExecpolicyAmendment":["echo"]}})).unwrap();
        assert!(actor
            .action(Action::RespondApproval {
                id: json!(901),
                choice: "network_0".into(),
                input: None,
                feedback: None
            })
            .is_err());
        actor
            .action(Action::RespondApproval {
                id: json!(901),
                choice: "execpolicy".into(),
                input: None,
                feedback: None,
            })
            .unwrap();
        assert_eq!(
            read(&mut reader),
            json!({"id":901,"result":{"decision":{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["echo"]}}}})
        );
        actor.message(json!({"id":902,"method":"item/permissions/requestApproval","params":{"threadId":"thread-1","turnId":"turn-1","permissions":{"network":{"enabled":true}}}})).unwrap();
        actor
            .action(Action::RespondApproval {
                id: json!(902),
                choice: "permission_session".into(),
                input: None,
                feedback: None,
            })
            .unwrap();
        assert_eq!(
            read(&mut reader),
            json!({"id":902,"result":{"permissions":{"network":{"enabled":true}},"scope":"session"}})
        );
        let request = json!({"id":903,"method":"mcpServer/elicitation/request","params":{"threadId":"thread-1","turnId":null,"serverName":"server","mode":"form","message":"Count?","requestedSchema":{"type":"object","properties":{"count":{"type":"integer","minimum":1}},"required":["count"]}}});
        actor.message(request.clone()).unwrap();
        actor.message(request.clone()).unwrap();
        assert_eq!(actor.state.elicitations.len(), 1);
        actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})).unwrap();
        assert_eq!(actor.state.status, Status::Waiting);
        assert_eq!(actor.state.elicitations.len(), 1);
        assert!(actor
            .action(Action::RespondElicitation {
                id: json!(903),
                action: "accept".into(),
                content: Some(json!({"count":0}))
            })
            .is_err());
        actor
            .action(Action::RespondElicitation {
                id: json!(903),
                action: "accept".into(),
                content: Some(json!({"count":2})),
            })
            .unwrap();
        assert_eq!(
            read(&mut reader),
            json!({"id":903,"result":{"action":"accept","content":{"count":2}}})
        );
        assert!(actor
            .action(Action::RespondElicitation {
                id: json!(903),
                action: "accept".into(),
                content: Some(json!({"count":2}))
            })
            .is_err());
        actor.message(json!({"id":907,"method":"mcpServer/elicitation/request","params":{"threadId":"thread-1","serverName":"server","mode":"url","message":"Authorize","elicitationId":"flow","url":"https://example.com/auth"}})).unwrap();
        actor
            .action(Action::RespondElicitation {
                id: json!(907),
                action: "accept".into(),
                content: None,
            })
            .unwrap();
        assert_eq!(
            read(&mut reader),
            json!({"id":907,"result":{"action":"accept"}})
        );
        actor.message(request).unwrap();
        actor.state.fail("Disconnected".into());
        assert!(actor.state.elicitations.is_empty());
        actor
            .message(json!({"id":904,"method":"item/tool/call","params":{"tool":"unregistered"}}))
            .unwrap();
        let response = read(&mut reader);
        assert_eq!(response["id"], 904);
        assert_eq!(response["result"]["success"], false);
        for (id, method) in [
            (905, "account/chatgptAuthTokens/refresh"),
            (906, "attestation/generate"),
        ] {
            actor
                .message(json!({"id":id,"method":method,"params":{}}))
                .unwrap();
            assert_eq!(read(&mut reader)["error"]["code"], -32601);
        }
    }
    fn usage(turn: &str, used: u64, window: Option<u64>) -> Value {
        json!({"method":"thread/tokenUsage/updated","params":{"threadId":"thread-1","turnId":turn,
            "tokenUsage":{"total":{"totalTokens":used * 3,"inputTokens":used * 3,"outputTokens":0},
            "last":{"totalTokens":used,"inputTokens":used - 5,"cachedInputTokens":0,"outputTokens":5,
            "reasoningOutputTokens":0,"cacheWriteInputTokens":0},"modelContextWindow":window}}})
    }
    fn idle_actor() -> (Actor, BufReader<std::process::ChildStdout>) {
        use std::os::unix::process::CommandExt;
        let mut actor = actor();
        actor.state.status = Status::Done;
        actor.state.turn_id = None;
        actor.state.saved.items =
            vec![json!({"id":"u1","type":"userMessage","content":[{"type":"text","text":"hi"}]})];
        let mut child = Command::new("cat")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let reader = BufReader::new(child.stdout.take().unwrap());
        let input = child.stdin.take().unwrap();
        let (_, lines) = mpsc::channel();
        actor.process = Some(Process {
            child,
            input,
            lines,
        });
        (actor, reader)
    }
    fn read_line(reader: &mut impl BufRead) -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    #[test]
    fn grove_workflow_guidance_preserves_config_in_start_and_resume_requests() {
        for resume in [false, true] {
            for custom in [None, Some("Follow the team's instructions")] {
                let (mut actor, mut reader) = idle_actor();
                if !resume {
                    actor.state.saved.thread_id = None;
                }
                actor
                    .request("config/read", json!({}), Pending::Config)
                    .unwrap();
                let config_request = read_line(&mut reader);
                actor.message(json!({"id":config_request["id"],"result":{"config":{"developer_instructions":custom}}})).unwrap();
                let request = read_line(&mut reader);
                assert_eq!(
                    request["method"],
                    if resume {
                        "thread/resume"
                    } else {
                        "thread/start"
                    }
                );
                let guidance = request["params"]["developerInstructions"].as_str().unwrap();
                assert!(guidance.contains("You are running inside Grove."));
                assert!(
                    guidance.contains("even when a skill prescribes a different format or handoff")
                );
                assert!(guidance.contains("Markdown link to the .grove-plan.json artifact"));
                assert!(guidance.contains("Grove's Review response and Send response controls"));
                assert!(guidance.contains("does not override explicit user preferences"));
                assert!(guidance.contains("use update_plan"));
                assert!(guidance.contains("Keep step statuses current"));
                assert!(guidance.contains(crate::reports::REPORT_GUIDANCE));
                assert!(guidance.ends_with(crate::rich_plan::GUIDANCE));
                if let Some(custom) = custom {
                    assert!(guidance.starts_with(&format!("{custom}\n\n")));
                }
            }
        }
    }
    #[test]
    fn native_plan_tool_is_enabled_for_new_and_resumed_threads_with_roots() {
        for resume in [false, true] {
            for roots in [None, Some(vec![json!("/tmp/grove-worktree")])] {
                let (mut actor, mut reader) = idle_actor();
                if !resume {
                    actor.state.saved.thread_id = None;
                }
                actor.start_thread(roots.clone()).unwrap();
                let request = read_line(&mut reader);
                assert_eq!(
                    request["method"],
                    if resume {
                        "thread/resume"
                    } else {
                        "thread/start"
                    }
                );
                assert_eq!(
                    request["params"]["config"]["tools.update_plan.enabled"],
                    true
                );
                match roots {
                    Some(roots) => assert_eq!(
                        request["params"]["config"]["sandbox_workspace_write.writable_roots"],
                        json!(roots)
                    ),
                    None => assert!(request["params"]["config"]
                        .get("sandbox_workspace_write.writable_roots")
                        .is_none()),
                }
            }
        }
    }
    #[test]
    fn clear_conversation_persists_fresh_identity_and_fences_old_transport() {
        let (mut actor, _) = idle_actor();
        actor.args = vec![
            "-c".into(),
            "model=chosen".into(),
            "old launch prompt".into(),
        ];
        actor.state.saved.draft = "old draft".into();
        actor.state.saved.draft_images = vec!["old image".into()];
        actor.state.saved.context = Some(ContextUsage {
            used: 99,
            window: 100,
        });
        actor.state.saved.composer.model = Some("chosen".into());
        let before = actor.state.saved.clone();
        let dir = tempfile::tempdir().unwrap();
        persist_in_dir(&before, dir.path()).unwrap();
        actor
            .clear_conversation(|saved| persist_in_dir(saved, dir.path()))
            .unwrap();
        let restored = load_from_dir(dir.path()).unwrap().sessions.remove(0);
        assert_eq!(restored.key, before.key);
        assert_eq!(restored.cwd, before.cwd);
        assert_eq!(restored.composer.model, before.composer.model);
        assert!(restored.thread_id.is_none());
        assert!(restored.items.is_empty());
        assert!(restored.title.is_none());
        assert!(restored.context.is_none());
        assert!(restored.draft.is_empty());
        assert!(restored.draft_images.is_empty());
        assert!(actor.process.is_none());
        assert!(actor.pending.is_empty());
        assert_eq!(actor.state.cleared_conversations, 1);
        assert_eq!(launch_options(&actor.args).unwrap().1, None);
        actor.message(json!({"method":"item/completed","params":{"threadId":"thread-1","item":{"id":"late","type":"agentMessage","text":"old"}}})).unwrap();
        assert!(actor.state.saved.items.is_empty());
        // Use a fresh fake transport and inspect the actual provider boundary.
        let mut child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let input = child.stdin.take().unwrap();
        let (_, lines) = mpsc::channel();
        actor.process = Some(Process {
            child,
            input,
            lines,
        });
        actor.start_thread(None).unwrap();
        let request = read_line(&mut reader);
        assert_eq!(request["method"], "thread/start");
        assert!(request["params"].get("threadId").is_none());
        actor
            .message(
                json!({"id":request["id"],"result":{"thread":{"id":"fresh-thread","turns":[]}}}),
            )
            .unwrap();
        actor.prompt("new question".into()).unwrap();
        let turn = read_line(&mut reader);
        assert_eq!(turn["method"], "turn/start");
        assert_eq!(turn["params"]["threadId"], "fresh-thread");
        assert_eq!(turn["params"]["input"][0]["text"], "new question");
    }
    #[test]
    fn clear_conversation_rejects_busy_disconnected_and_save_failure_without_mutation() {
        let mut actor = actor();
        assert_eq!(
            actor.action(Action::ClearConversation).unwrap_err(),
            CLEAR_BUSY
        );
        actor.state.status = Status::Done;
        actor.state.turn_id = None;
        assert_eq!(
            actor.action(Action::ClearConversation).unwrap_err(),
            CLEAR_DISCONNECTED
        );
        let (mut actor, _) = idle_actor();
        let old = actor.state.saved.thread_id.clone();
        assert!(actor
            .clear_conversation(|_| Err("disk full".into()))
            .unwrap_err()
            .contains("disk full"));
        assert_eq!(actor.state.saved.thread_id, old);
        assert!(actor.process.is_some());
        assert_eq!(actor.state.cleared_conversations, 0);
    }
    #[test]
    fn token_usage_tracks_last_turn_context_and_survives_recovery() {
        let mut state = state();
        assert!(state.apply(&usage("turn-1", 76_000, Some(200_000))));
        assert_eq!(
            state.saved.context,
            Some(ContextUsage {
                used: 76_000,
                window: 200_000
            })
        );
        assert_eq!(state.saved.context.unwrap().percent(), 38);
        // A missing window keeps the last known one; cumulative totals are never used.
        assert!(state.apply(&usage("turn-1", 90_000, None)));
        assert_eq!(state.saved.context.unwrap().used, 90_000);
        assert_eq!(state.saved.context.unwrap().window, 200_000);
        let recovered: SavedSession =
            serde_json::from_value(serde_json::to_value(&state.saved).unwrap()).unwrap();
        assert_eq!(Snapshot::new(recovered).saved.context, state.saved.context);
        // An unknown window leaves the meter unknown.
        let mut fresh = Snapshot::new(saved());
        fresh.apply(&usage("turn-1", 10_000, None));
        assert!(fresh.saved.context.is_none());
        assert_eq!(fresh.context_tokens, Some(10_000));
    }
    #[test]
    fn compact_tokens_and_labels_are_compact() {
        assert_eq!(compact_tokens(950), "950");
        assert_eq!(compact_tokens(4_200), "4.2k");
        assert_eq!(compact_tokens(5_000), "5k");
        assert_eq!(compact_tokens(24_400), "24k");
        assert_eq!(compact_tokens(182_000), "182k");
        assert_eq!(compact_tokens(999_700), "1M");
        assert_eq!(compact_tokens(1_260_000), "1.3M");
        let item = json!({"type":COMPACTION_ITEM,"trigger":"auto","preTokens":182_000,"postTokens":24_000});
        assert_eq!(
            compaction_label(&item),
            "Context compacted \u{b7} auto \u{b7} 182k \u{2192} 24k"
        );
        assert_eq!(item_text(&item), compaction_label(&item));
        let pending = json!({"type":COMPACTION_ITEM,"trigger":"manual","preTokens":182_000,"postTokens":null});
        assert_eq!(
            compaction_label(&pending),
            "Context compacted \u{b7} manual"
        );
        assert_eq!(
            ContextUsage {
                used: 300,
                window: 200
            }
            .percent(),
            100
        );
    }
    #[test]
    #[cfg(unix)]
    fn manual_compaction_requests_thread_compact_and_normalizes_its_divider() {
        let (mut actor, mut reader) = idle_actor();
        actor
            .message(usage("turn-0", 182_000, Some(475_000)))
            .unwrap();
        actor.action(Action::Compact).unwrap();
        assert_eq!(
            read_line(&mut reader),
            json!({"id":1,"method":"thread/compact/start","params":{"threadId":"thread-1"}})
        );
        // While the request is pending, ordinary prompts and a second compaction wait.
        assert!(actor.prompt("next".into()).is_err());
        assert_eq!(actor.action(Action::Compact).unwrap_err(), COMPACT_BUSY);
        actor.message(json!({"id":1,"result":{}})).unwrap();
        // Observed codex-cli 0.163.0 sequence: a fresh turn wraps the compaction item.
        actor.message(json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"compact-turn","status":"inProgress"}}})).unwrap();
        actor.message(json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"compact-turn","item":{"type":"contextCompaction","id":"c1"}}})).unwrap();
        assert!(actor.state.compacting);
        assert_eq!(actor.state.status, Status::Working);
        assert_eq!(actor.state.saved.items[1]["status"], "inProgress");
        assert_ne!(
            item_text(&actor.state.saved.items[1]),
            "contextCompaction: inProgress"
        );
        let mut post = usage("compact-turn", 24_000, Some(475_000));
        post["params"]["tokenUsage"]["last"]["inputTokens"] = json!(0);
        post["params"]["tokenUsage"]["last"]["outputTokens"] = json!(0);
        actor.message(post).unwrap();
        actor.message(json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"compact-turn","item":{"type":"contextCompaction","id":"c1"}}})).unwrap();
        actor.message(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"compact-turn","status":"completed"}}})).unwrap();
        assert!(!actor.state.compacting);
        assert_eq!(actor.state.status, Status::Done);
        let divider = &actor.state.saved.items[1];
        assert_eq!(divider["type"], COMPACTION_ITEM);
        assert_eq!(divider["status"], "completed");
        assert_eq!(
            compaction_label(divider),
            "Context compacted \u{b7} manual \u{b7} 182k \u{2192} 24k"
        );
        assert_eq!(actor.state.saved.context.unwrap().used, 24_000);
        // Resumed history carries no Grove metadata; the known divider survives it.
        actor.pending.insert(
            9,
            Request {
                kind: Pending::Thread,
                deadline: Instant::now() + Duration::from_secs(30),
            },
        );
        actor.message(json!({"id":9,"result":{"thread":{"id":"thread-1","turns":[{"id":"t","status":"completed","items":[
            {"id":"u1","type":"userMessage","content":[{"type":"text","text":"hi"}]},
            {"id":"c1","type":"contextCompaction"},
            {"id":"old","type":"contextCompaction"}
        ]}]}}})).unwrap();
        assert_eq!(
            compaction_label(&actor.state.saved.items[1]),
            "Context compacted \u{b7} manual \u{b7} 182k \u{2192} 24k"
        );
        assert_eq!(item_text(&actor.state.saved.items[2]), "Context compacted");
    }
    #[test]
    fn automatic_compaction_is_labelled_auto_and_waits_for_post_usage() {
        let mut state = state();
        state.apply(&usage("turn-1", 150_000, Some(200_000)));
        state.apply(&json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1","item":{"type":"contextCompaction","id":"auto"}}}));
        assert!(state.compacting);
        state.apply(&json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"type":"contextCompaction","id":"auto"}}}));
        assert!(!state.compacting);
        let divider = state.saved.items.last().unwrap().clone();
        assert_eq!(compaction_label(&divider), "Context compacted \u{b7} auto");
        state.apply(&usage("turn-1", 30_000, Some(200_000)));
        assert_eq!(
            compaction_label(state.saved.items.last().unwrap()),
            "Context compacted \u{b7} auto \u{b7} 150k \u{2192} 30k"
        );
        // Only the waiting figure is filled; later usage never rewrites it.
        state.apply(&usage("turn-1", 40_000, Some(200_000)));
        assert_eq!(state.saved.items.last().unwrap()["postTokens"], 30_000);
    }
    #[test]
    #[cfg(unix)]
    fn compaction_is_rejected_while_busy_or_empty_and_failure_is_inline() {
        let (mut actor, mut reader) = idle_actor();
        actor.state.status = Status::Working;
        actor.state.turn_id = Some("turn-1".into());
        assert_eq!(actor.action(Action::Compact).unwrap_err(), COMPACT_BUSY);
        assert!(actor.pending.is_empty());
        actor.state.status = Status::Done;
        actor.state.turn_id = None;
        actor.state.saved.items.clear();
        assert_eq!(
            actor.action(Action::Compact).unwrap_err(),
            NOTHING_TO_COMPACT
        );
        actor.state.saved.thread_id = None;
        assert_eq!(
            actor.action(Action::Compact).unwrap_err(),
            NOTHING_TO_COMPACT
        );
        actor.state.saved.thread_id = Some("thread-1".into());
        actor.state.saved.items = vec![json!({"id":"a","type":"agentMessage","text":"hello"})];
        actor.action(Action::Compact).unwrap();
        let sent = read_line(&mut reader);
        actor
            .message(json!({"id":sent["id"],"error":{"code":-32600,"message":"no thread"}}))
            .unwrap();
        assert_eq!(actor.state.error.as_deref(), Some("no thread"));
        assert_eq!(actor.state.status, Status::Done);
        assert!(!actor.state.compact_requested);
        actor.process = None;
        assert_eq!(
            actor.action(Action::Compact).unwrap_err(),
            COMPACT_DISCONNECTED
        );
    }
    #[cfg(unix)]
    struct EditFixture {
        actor: Actor,
        repo: tempfile::TempDir,
        journal: PathBuf,
    }
    #[cfg(unix)]
    impl Drop for EditFixture {
        fn drop(&mut self) {
            let _ = fs_err::remove_file(&self.journal);
        }
    }
    #[cfg(unix)]
    fn edit_fixture() -> EditFixture {
        let repo = tempfile::tempdir().unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .arg(repo.path())
            .status()
            .unwrap()
            .success());
        let root = repo.path().canonicalize().unwrap();
        let file = root.join("work");
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.state.status = Status::Idle;
        actor.state.turn_id = None;
        actor.state.edit_supported = true;
        actor.state.saved.cwd = root.to_string_lossy().into_owned();
        actor.state.saved.key = format!(
            "test-edit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let journal = recovery_dir()
            .unwrap()
            .join(format!("{}.json", actor.state.saved.key));
        for (start, turn, text) in [
            (0, "earlier-turn", "initial"),
            (2, "target-turn", "before target"),
            (4, "later-turn", "after target"),
        ] {
            fs_err::write(&file, text).unwrap();
            actor.state.saved.edit_checkpoints.push(EditCheckpoint {
                start_item: start,
                turn_id: Some(turn.into()),
                files: crate::checkpoint::capture(std::slice::from_ref(&root)).unwrap(),
            });
        }
        fs_err::write(&file, "latest workspace").unwrap();
        actor.state.saved.items = vec![
            json!({"id":"earlier","type":"userMessage","content":[{"type":"text","text":"earlier prompt"}]}),
            json!({"id":"earlier-answer","type":"agentMessage","text":"earlier answer"}),
            json!({"id":"target","type":"userMessage","content":[{"type":"text","text":"target prompt"},{"type":"image","url":"data:image/png;base64,aGVsbG8="}]}),
            json!({"id":"target-answer","type":"agentMessage","text":"target answer"}),
            json!({"id":"later","type":"userMessage","content":[{"type":"text","text":"later prompt"}]}),
        ];
        EditFixture {
            actor,
            repo,
            journal,
        }
    }
    #[cfg(unix)]
    fn edit_pending(actor: &Actor) -> u64 {
        *actor.pending.keys().next().unwrap()
    }
    #[cfg(unix)]
    fn edit_disk(fixture: &EditFixture) -> SavedSession {
        serde_json::from_slice(&fs_err::read(&fixture.journal).unwrap()).unwrap()
    }
    #[test]
    #[cfg(unix)]
    fn edit_older_message_restores_pre_turn_then_retains_earlier_history_and_populates_draft() {
        let mut fixture = edit_fixture();
        let earlier = fixture.actor.state.saved.items[..2].to_vec();
        fixture.actor.edit_message(2, "target".into()).unwrap();
        assert_eq!(
            fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
            "before target"
        );
        assert_eq!(fixture.actor.state.saved.items.len(), 5);
        assert!(edit_disk(&fixture).pending_edit.is_some());
        let id = edit_pending(&fixture.actor);
        fixture
            .actor
            .message(json!({"id":id,"result":{"thread":{"id":"thread-1"}}}))
            .unwrap();
        assert_eq!(fixture.actor.state.saved.items, earlier);
        assert_eq!(fixture.actor.state.saved.draft, "target prompt");
        assert_eq!(
            fixture.actor.state.saved.draft_images,
            vec!["data:image/png;base64,aGVsbG8="]
        );
        assert_eq!(fixture.actor.state.saved.edit_checkpoints.len(), 1);
        assert_eq!(
            fixture.actor.state.saved.edit_checkpoints[0]
                .turn_id
                .as_deref(),
            Some("earlier-turn")
        );
        assert!(edit_disk(&fixture).pending_edit.is_none());
        assert_eq!(edit_disk(&fixture).items, earlier);
        assert_eq!(fixture.actor.state.status, Status::Idle);
    }
    #[test]
    #[cfg(unix)]
    fn inline_edit_preserves_composer_through_restore_and_submission_ack() {
        let mut fixture = edit_fixture();
        let image = "data:image/png;base64,aGVsbG8=".to_owned();
        fixture.actor.state.saved.draft = "Unrelated draft".into();
        fixture.actor.state.saved.draft_images = vec![image.clone()];
        fixture
            .actor
            .action(Action::EditMessageInline {
                index: 2,
                message_id: "target".into(),
            })
            .unwrap();
        assert_eq!(
            edit_disk(&fixture)
                .pending_edit
                .unwrap()
                .composer_draft
                .unwrap()
                .0,
            "Unrelated draft"
        );
        fixture
            .actor
            .action(Action::Draft("Newer composer draft".into()))
            .unwrap();
        let id = edit_pending(&fixture.actor);
        fixture
            .actor
            .message(json!({"id":id,"result":{"thread":{"id":"thread-1"}}}))
            .unwrap();
        assert_eq!(fixture.actor.state.saved.draft, "Newer composer draft");
        assert_eq!(fixture.actor.state.saved.draft_images, vec![image.clone()]);
        fixture
            .actor
            .action(Action::PromptInline {
                text: "Edited target".into(),
                images: vec![image.clone()],
            })
            .unwrap();
        fixture
            .actor
            .action(Action::Draft("Typing during send".into()))
            .unwrap();
        fixture
            .actor
            .action(Action::DraftImages(Vec::new()))
            .unwrap();
        let id = edit_pending(&fixture.actor);
        fixture
            .actor
            .message(json!({"id":id,"result":{"turn":{"id":"edited-turn","status":"inProgress"}}}))
            .unwrap();
        assert_eq!(fixture.actor.state.saved.draft, "Typing during send");
        assert!(fixture.actor.state.saved.draft_images.is_empty());
        assert_eq!(fixture.actor.state.accepted_prompts, 1);
        assert_eq!(fixture.actor.state.saved.items.len(), 2);
    }
    #[test]
    #[cfg(unix)]
    fn inline_edit_failure_preserves_composer_and_original_history() {
        let mut fixture = edit_fixture();
        let items = fixture.actor.state.saved.items.clone();
        fixture.actor.state.saved.draft = "Keep composer".into();
        fixture
            .actor
            .action(Action::EditMessageInline {
                index: 2,
                message_id: "target".into(),
            })
            .unwrap();
        let id = edit_pending(&fixture.actor);
        fixture
            .actor
            .message(json!({"id":id,"error":{"message":"Rejected restore"}}))
            .unwrap();
        assert_eq!(fixture.actor.state.saved.draft, "Keep composer");
        assert_eq!(fixture.actor.state.saved.items, items);
        assert_eq!(
            fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
            "latest workspace"
        );
    }
    #[test]
    #[cfg(unix)]
    fn edit_provider_failure_recovers_current_workspace_and_complete_conversation() {
        let mut fixture = edit_fixture();
        let items = fixture.actor.state.saved.items.clone();
        fixture.actor.edit_message(2, "target".into()).unwrap();
        let id = edit_pending(&fixture.actor);
        fixture
            .actor
            .message(json!({"id":id,"error":{"message":"revert refused"}}))
            .unwrap();
        assert_eq!(
            fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
            "latest workspace"
        );
        assert_eq!(fixture.actor.state.saved.items, items);
        assert_eq!(fixture.actor.state.saved.edit_checkpoints.len(), 3);
        assert!(fixture.actor.state.saved.pending_edit.is_none());
        assert!(edit_disk(&fixture).pending_edit.is_none());
        assert_eq!(fixture.actor.state.error.as_deref(), Some("revert refused"));
    }
    #[test]
    #[cfg(unix)]
    fn edit_busy_queued_and_stale_message_guards_leave_workspace_and_history_untouched() {
        let mut fixture = edit_fixture();
        let items = fixture.actor.state.saved.items.clone();
        fixture.actor.state.status = Status::Working;
        assert!(fixture.actor.edit_message(2, "target".into()).is_err());
        fixture.actor.state.status = Status::Idle;
        fixture.actor.state.queued_prompts.push(QueuedPrompt {
            id: 77,
            text: "queued".into(),
            images: Vec::new(),
        });
        assert!(fixture.actor.edit_message(2, "target".into()).is_err());
        fixture.actor.state.queued_prompts.clear();
        assert!(fixture.actor.edit_message(2, "stale-id".into()).is_err());
        assert_eq!(fixture.actor.state.saved.items, items);
        assert!(fixture.actor.state.saved.pending_edit.is_none());
        assert!(fixture.actor.pending.is_empty());
        assert!(!fixture.journal.exists());
        assert_eq!(
            fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
            "latest workspace"
        );
    }
    #[test]
    #[cfg(unix)]
    fn edit_reconnect_recovers_durable_journal_only_after_paginated_provider_history() {
        for target_present in [true, false] {
            let mut fixture = edit_fixture();
            let original = fixture.actor.state.saved.items.clone();
            fixture.actor.edit_message(2, "target".into()).unwrap();
            let saved = edit_disk(&fixture);
            fixture.actor.state = Snapshot::new(saved);
            fixture.actor.pending.clear();
            fixture.actor.pending.insert(
                900,
                Request {
                    kind: Pending::Thread,
                    deadline: Instant::now(),
                },
            );
            fixture
                .actor
                .message(json!({"id":900,"result":{"thread":{"id":"thread-1","turns":[]}}}))
                .unwrap();
            assert!(fixture.actor.state.saved.pending_edit.is_some());
            let first_page = edit_pending(&fixture.actor);
            fixture.actor.message(json!({"id":first_page,"result":{"data":[{"id":"newer-turn"}],"nextCursor":"older-page"}})).unwrap();
            assert!(fixture.actor.state.saved.pending_edit.is_some());
            assert_eq!(fixture.actor.state.saved.items, original);
            let second_page = edit_pending(&fixture.actor);
            let turns = if target_present {
                json!([{"id":"target-turn"}])
            } else {
                json!([{"id":"earlier-turn"}])
            };
            fixture
                .actor
                .message(json!({"id":second_page,"result":{"data":turns,"nextCursor":null}}))
                .unwrap();
            assert!(fixture.actor.state.saved.pending_edit.is_none());
            assert!(edit_disk(&fixture).pending_edit.is_none());
            if target_present {
                assert_eq!(fixture.actor.state.saved.items, original);
                assert_eq!(
                    fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
                    "latest workspace"
                );
            } else {
                assert_eq!(fixture.actor.state.saved.items, original[..2]);
                assert_eq!(fixture.actor.state.saved.draft, "target prompt");
                assert_eq!(
                    fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
                    "before target"
                );
            }
        }
    }
    #[test]
    #[cfg(unix)]
    fn edit_transport_failure_keeps_durable_journal_and_history_for_reconnect() {
        let mut fixture = edit_fixture();
        let items = fixture.actor.state.saved.items.clone();
        let process = fixture.actor.process.as_mut().unwrap();
        // Reap the entire fixture process group so the pipe has no remaining reader.
        unsafe {
            libc::kill(-(process.child.id() as i32), libc::SIGKILL);
        }
        process.child.wait().unwrap();
        let error = fixture.actor.edit_message(2, "target".into()).unwrap_err();
        assert!(error.contains("uncertain"), "{error}");
        assert_eq!(fixture.actor.state.status, Status::Disconnected);
        assert!(fixture.actor.process.is_none());
        assert_eq!(fixture.actor.state.saved.items, items);
        assert!(fixture.actor.state.saved.pending_edit.is_some());
        let durable = edit_disk(&fixture);
        assert!(durable.pending_edit.is_some());
        assert_eq!(durable.items, items);
        assert_eq!(
            fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
            "before target"
        );
        assert_eq!(fixture.actor.state.edited_messages, 0);
        durable.pending_edit.unwrap().recovery.restore().unwrap();
        assert_eq!(
            fs_err::read_to_string(fixture.repo.path().join("work")).unwrap(),
            "latest workspace"
        );
    }

    #[test]
    fn edit_capability_tracks_paginated_and_legacy_thread_responses() {
        let mut actor = actor();
        actor.state.turn_id = None;
        actor.state.saved.items = vec![
            json!({"id":"sent","type":"userMessage","content":[{"type":"text","text":"sent prompt"}]}),
        ];
        for (id, history_mode, supported) in [
            (901, None, false),
            (902, Some("paginated"), true),
            (903, Some("legacy"), false),
        ] {
            actor.pending.insert(
                id,
                Request {
                    kind: Pending::Thread,
                    deadline: Instant::now(),
                },
            );
            let mut thread = json!({"id":"thread-1","turns":[]});
            if let Some(mode) = history_mode {
                thread["historyMode"] = json!(mode);
            }
            actor
                .message(json!({"id":id,"result":{"thread":thread}}))
                .unwrap();
            assert_eq!(actor.state.edit_supported, supported);
            let error = actor.state.editable_message(0).unwrap_err();
            if supported {
                assert!(error.contains("checkpoint"), "{error}");
            } else {
                assert!(error.contains("rewind support"), "{error}");
            }
        }
    }
    #[test]
    #[cfg(unix)]
    fn native_plan_actor_journals_before_dispatch_and_never_duplicates_feedback() {
        let mut actor = actor();
        actor.process = Some(fixture_process());
        actor.state.saved.rich_plans = serde_json::from_value(json!({"revisions":[{"message_id":"plan-message","source":"plan.grove-plan.json","document":{"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"Native","claims":[{"id":"claim","text":"Bounded"}]},"assets":[]}]})).unwrap();
        actor.state.saved.draft = "independent composer".into();

        assert!(actor
            .send_plan_response("plan", "r1", |_| Err("disk full".into()))
            .is_err());
        assert!(actor.state.queued_prompts.is_empty());
        assert!(actor.pending.is_empty());
        let journal = std::cell::RefCell::new(Vec::new());
        for _ in 0..2 {
            actor
                .send_plan_response("plan", "r1", |saved| {
                    journal
                        .borrow_mut()
                        .push(serde_json::to_vec(saved).unwrap());
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(journal.borrow().len(), 1);
        assert_eq!(actor.state.queued_prompts.len(), 1);
        assert!(actor.pending.is_empty());
        let id = actor.state.queued_prompts[0].id;
        assert!(!actor.state.queue_editable(id));
        assert!(actor.state.begin_queue_edit(id).is_err());
        actor.state.remove_queued(id).unwrap();
        assert!(matches!(
            actor.state.saved.rich_plans.revisions[0].review.delivery,
            crate::rich_plan::ResponseDelivery::Draft
        ));
        actor.send_plan_response("plan", "r1", |_| Ok(())).unwrap();
        actor.state.status = Status::Idle;
        actor.state.turn_id = None;
        actor.queue_ready = true;
        actor.dispatch_queue().unwrap();
        assert_eq!(actor.pending.len(), 1);
        actor
            .message(json!({"id":actor.seq,"result":{"turn":{"id":"feedback-turn"}}}))
            .unwrap();
        assert!(matches!(
            actor.state.saved.rich_plans.revisions[0].review.delivery,
            crate::rich_plan::ResponseDelivery::Sent { .. }
        ));
        let seq = actor.seq;
        actor
            .send_plan_response("plan", "r1", |_| {
                panic!("confirmed feedback must not be journaled again")
            })
            .unwrap();
        assert_eq!(actor.seq, seq);
        assert_eq!(actor.state.saved.draft, "independent composer");
        let mut restarted = Snapshot::new(
            serde_json::from_slice(&serde_json::to_vec(&actor.state.saved).unwrap()).unwrap(),
        );
        assert!(restarted
            .saved
            .rich_plans
            .prepare_delivery("plan", "r1")
            .unwrap()
            .is_none());
    }
}

#[cfg(test)]
mod received_time_tests {
    use super::*;

    #[test]
    fn receipt_time_survives_streaming_without_backfilling_history() {
        let mut item = json!({"id":"reply", "type":"agentMessage", "text":"first"});
        stamp_message_received(&mut item, None);
        let timestamp = item["groveReceivedAt"].as_u64().unwrap();
        assert!(timestamp > 0);
        let mut replacement = json!({"id":"reply", "type":"agentMessage", "text":"complete"});
        stamp_message_received(&mut replacement, Some(&item));
        assert_eq!(replacement["groveReceivedAt"], timestamp);
        let historical = json!({"id":"old", "type":"userMessage"});
        let mut replacement = historical.clone();
        stamp_message_received(&mut replacement, Some(&historical));
        assert!(replacement.get("groveReceivedAt").is_none());
    }
}
