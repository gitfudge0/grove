//! Provider-neutral subagent transcripts, observation and retention.

use super::*;

impl Snapshot {
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
        // Avoid mutable transcript access (and a revision bump) for duplicate
        // child notifications. Both durable and live child rows share item Arcs.
        let identified = !item["id"].is_null();
        let same_live = identified
            && self.subagent_activity.get(id).is_some_and(|activity| {
                activity
                    .items
                    .iter()
                    .any(|old| old["id"] == item["id"] && old == &item)
            });
        let same_saved = self.saved.provider != Provider::Claude
            || (identified
                && self.saved.claude_subagents.get(id).is_some_and(|child| {
                    child
                        .items
                        .iter()
                        .any(|old| old["id"] == item["id"] && old == &item)
                }));
        if same_live && same_saved {
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
        saved.visuals.clear();
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
            let parent_ids: HashSet<&str> = self
                .saved
                .items
                .iter()
                .filter_map(|item| item["id"].as_str())
                .collect();
            let existing: HashMap<String, TranscriptItem> = self
                .subagent_activity
                .get(id)
                .into_iter()
                .flat_map(|activity| activity.items.iter())
                .filter_map(|item| item["id"].as_str().map(|id| (id.to_owned(), item.clone())))
                .collect();
            let mut chronological: Vec<TranscriptItem> = Vec::new();
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
                            && !item["id"]
                                .as_str()
                                .is_some_and(|id| parent_ids.contains(id))
                        {
                            chronological.push(
                                item["id"]
                                    .as_str()
                                    .and_then(|id| existing.get(id))
                                    .filter(|old| old.as_ref() == item)
                                    .cloned()
                                    .unwrap_or_else(|| item.clone().into()),
                            );
                        }
                    }
                }
            }
            if !chronological.is_empty() {
                self.ensure_subagent_activity(id);
                if let Some(activity) = self.subagent_activity.get_mut(id) {
                    // A read can introduce older history after live notifications.
                    // Keep the read's provider order and append only unseen live items.
                    let ids: HashSet<String> = chronological
                        .iter()
                        .filter_map(|item| item["id"].as_str().map(str::to_owned))
                        .collect();
                    for item in &activity.items {
                        if !item["id"].as_str().is_some_and(|id| ids.contains(id)) {
                            chronological.push(item.clone());
                        }
                    }
                    if activity.items != chronological {
                        activity.items = chronological.into_iter().collect();
                    }
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
}
