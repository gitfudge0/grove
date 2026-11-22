//! Persisted session data, forks, and composer draft ownership.

use super::{
    item_text, legacy_composer_settings, persist, ComposerSettings, ContextDetails, ContextUsage,
    Provider, SavedClaudeSubagent, TranscriptItems, WorkPeriod,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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
    #[serde(default)]
    pub html_policy: crate::visuals::HtmlPolicy,
    #[serde(default)]
    pub visuals: Vec<crate::visuals::Reference>,
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
            html_policy: crate::visuals::HtmlPolicy::Explicit,
            visuals: Vec::new(),
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
        saved.visuals.clear();
        saved.html_policy = crate::visuals::HtmlPolicy::Explicit;
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
