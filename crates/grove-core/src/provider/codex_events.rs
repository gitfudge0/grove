//! Codex notification projection into the shared session snapshot.

use super::{
    append_json_text, item_text, prompt_title, stamp_message_received, ContextTokenAccounting,
    ContextTokenCounters, QuestionRequest, QuestionSource, SessionNoticeKind, Snapshot, Status,
    SubagentStatus, COMPACTION_ITEM,
};
use serde_json::{json, Value};
use std::fmt::Write as _;

impl Snapshot {
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
                    let usage = crate::account_usage::codex(p, crate::account_usage::now());
                    let usage_changed = usage != self.account_usage;
                    self.account_usage = usage;
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
                    return usage_changed;
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
                if item["type"] == "userMessage" {
                    if let Some(turn) = p["turnId"].as_str() {
                        item["groveTurnId"] = json!(turn);
                    } else if let Some(old) =
                        self.saved.items.iter().find(|old| old["id"] == item["id"])
                    {
                        item["groveTurnId"] = old["groveTurnId"].clone();
                    }
                }
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
                    append_json_text(&mut parts[index], p["delta"].as_str().unwrap_or_default());
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
                let at = self
                    .saved
                    .items
                    .iter()
                    .rposition(|old| old["id"] == p["itemId"])
                    .unwrap_or_else(|| {
                        let kind = if output {
                            "commandExecution"
                        } else if method.contains("plan") {
                            "plan"
                        } else {
                            "agentMessage"
                        };
                        let mut item = json!({"id":p["itemId"],"type":kind});
                        stamp_message_received(&mut item, None);
                        self.saved.items.push(item);
                        self.saved.items.len() - 1
                    });
                let key = if output { "aggregatedOutput" } else { "text" };
                let item = &mut self.saved.items[at];
                append_json_text(&mut item[key], p["delta"].as_str().unwrap_or_default());
            }
            _ => return false,
        }
        true
    }
    fn item_index(&mut self, id: &Value, kind: &str) -> usize {
        if let Some(at) = self.saved.items.iter().rposition(|item| item["id"] == *id) {
            return at;
        }
        let mut item = json!({"id":id,"type":kind,"status":"inProgress"});
        stamp_message_received(&mut item, None);
        self.saved.items.push(item);
        self.saved.items.len() - 1
    }
}
