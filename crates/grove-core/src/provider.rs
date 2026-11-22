//! Provider-neutral session state, commands, persistence, and interaction contracts.
//! Native transports live in `codex` and `claude`; legacy inline recovery files migrate on save.
mod codex_events;
mod commands;
mod persistence;
mod questions;
mod recovery;
mod saved_session;
mod subagents;

pub(crate) use commands::startup_submission;
pub use commands::{
    deferred_connection, Action, Connection, DeferredConnection, Handle, StartupValidation,
};
#[cfg(test)]
pub(crate) use recovery::{
    archive_closed_in_dirs, load_from_dir, persist_in_dir, recover_saved_for_test,
    workspace_recovery_blocked_in_dirs,
};
pub(crate) use recovery::{archive_closed_session, persist};
pub use recovery::{
    finish_history_reopen, load_active_session, load_history_session, load_saved_session,
    load_session_history, load_session_history_metadata, load_sessions, load_sessions_metadata,
    prepare_history_reopen, recovery_dir, session_history_dir, workspace_recovery_blocked,
    Recovery,
};
pub use saved_session::{
    ClaudeResume, EditCheckpoint, ForkOrigin, PendingEdit, ReferenceDraft, ReferenceSource,
    ReviewCompletion, SavedComposerDraft, SavedSession, SavedSideChat, SessionReview,
};
pub(crate) use subagents::record_subagent;
pub use subagents::{
    active_subagents, SavedClaudeSubagent, Subagent, SubagentActivity, SubagentActivityEvent,
    SubagentActivityKind, SubagentStatus, SubagentTranscriptAvailability, WorkPeriod,
};

pub(crate) use persistence::Background as BackgroundPersistence;
pub(crate) use questions::QuestionSource;
pub use questions::{
    Question, QuestionAnswerAck, QuestionAnswers, QuestionKind, QuestionNumber, QuestionOption,
    QuestionRequest,
};
/// Append provider text in place; malformed/missing text retains legacy empty-prefix behavior.
pub(crate) fn append_json_text(value: &mut Value, suffix: &str) {
    if let Value::String(text) = value {
        text.push_str(suffix);
    } else {
        *value = Value::String(suffix.into());
    }
}

pub use crate::transcript_items::{SharedValues, TranscriptItem, TranscriptItems};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::{
    borrow::Borrow,
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
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
    /// Transient account allowance; never serialized with conversation context.
    pub account_usage: Option<crate::account_usage::AccountUsage>,
    pub(crate) mcp_statuses: HashMap<String, (String, String, bool)>,
    pub sandbox: Option<Value>,
    pub approval_policy: Option<Value>,
    /// Modes advertised by the installed Claude CLI, not assumed SDK capabilities.
    pub permission_modes: Vec<String>,
    pub models: SharedValues,
    pub skills: SharedValues,
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
    /// Runtime-only ownership receipts; provider IDs never cross session connections.
    pub submission_items: HashMap<u64, String>,
    pub submission_turns: HashMap<u64, String>,
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

    /// During provider work only response anchors in a closed work period can fork.
    pub fn can_fork_at(&self, boundary: usize, restore_user: bool) -> bool {
        if boundary > self.saved.items.len() || self.saved.pending_edit.is_some() {
            return false;
        }
        if !self.provider_work_active() {
            return true;
        }
        if restore_user || boundary == 0 || self.status == Status::Starting || self.reconnecting {
            return false;
        }
        self.saved
            .work_periods
            .iter()
            .rev()
            .find(|period| period.start_item < boundary)
            .is_some_and(|period| period.end_item.is_some_and(|end| boundary <= end))
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

    /// Resolve an acknowledged input without comparing provider-normalized content.
    /// Codex identifies a turn's initial input by turn ID; Claude acknowledges the
    /// exact input UUID. Steers without per-input identity retain the UI fallback.
    pub fn submission_item_id(&self, submission: u64) -> Option<&str> {
        if let Some(id) = self.submission_items.get(&submission) {
            return Some(id);
        }
        let turn = self.submission_turns.get(&submission)?;
        self.saved.items.iter().find_map(|item| {
            (item["type"] == "userMessage" && item["groveTurnId"].as_str() == Some(turn))
                .then(|| item["id"].as_str())
                .flatten()
        })
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
            account_usage: None,
            mcp_statuses: HashMap::new(),
            sandbox: None,
            approval_policy: None,
            permission_modes: Vec::new(),
            models: SharedValues::default(),
            skills: SharedValues::default(),
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
            submission_items: HashMap::default(),
            submission_turns: HashMap::default(),
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
/// Fresh, repeatable startup validation. Native actors invoke it only on their
/// worker thread, before the first process and each explicit reconnect.
#[cfg(test)]
mod independent_conversation_tests;
