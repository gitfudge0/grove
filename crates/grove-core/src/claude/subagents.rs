//! Claude child stream attribution and subagent lifecycle.

use super::*;
use crate::provider::SubagentStatus;

impl Actor {
    pub(super) fn register_subagent(&mut self, block: &Value) {
        self.register_subagent_from(block, None);
    }

    fn register_subagent_from(&mut self, block: &Value, parent: Option<&str>) {
        if !matches!(block["name"].as_str(), Some("Agent" | "Task")) {
            return;
        }
        let Some(id) = block["id"].as_str() else {
            return;
        };
        let root = self
            .state
            .saved
            .thread_id
            .as_deref()
            .unwrap_or(&self.state.saved.key);
        let owner = parent.unwrap_or(root);
        if owner == root || self.interaction_children.known(owner) {
            self.interaction_children.register(
                root,
                owner,
                id,
                block["input"]["name"]
                    .as_str()
                    .or(block["input"]["subagent_type"].as_str()),
                None,
            );
            self.interaction_children.start(id, None);
        }
        let input = &block["input"];
        let name = input["name"]
            .as_str()
            .or_else(|| input["subagent_type"].as_str());
        self.state.upsert_subagent(
            id,
            name,
            input["description"].as_str(),
            SubagentStatus::Working,
        );
        self.state.set_subagent_metadata(
            id,
            input["subagent_type"].as_str(),
            input["description"]
                .as_str()
                .filter(|text| !text.trim().is_empty())
                .or_else(|| input["prompt"].as_str()),
        );
    }

    pub(super) fn subagent_message(&mut self, parent: &str, value: &Value) {
        let canonical = self
            .subagent_tasks
            .get(parent)
            .cloned()
            .unwrap_or_else(|| parent.into());
        let parent = canonical.as_str();
        // Child frames must never reach the primary transcript, usage, or turn lifecycle.
        self.state
            .upsert_subagent(parent, None, None, SubagentStatus::Unknown);
        self.subagent_system_message(value);
        let child = self.child_transcripts.entry(parent.to_owned()).or_default();
        let previous = child.items.clone();
        child.message(value, &self.state.saved.cwd);
        for (index, item) in child.items.iter().enumerate() {
            if previous
                .get(index)
                .is_none_or(|old| !old.same_storage(item))
            {
                self.state.upsert_subagent_item(parent, item.clone());
            }
        }
        if value["type"] == "result" {
            if let Some(report) = value["result"].as_str() {
                self.subagent_report(parent, report);
            }
        }
        if value["type"] == "result" {
            if let Some(expired) = self.interaction_children.finish(parent, None) {
                self.state.expire_interactions(&expired);
                self.state.refresh_interaction_status();
            }
        }
        match value["type"].as_str() {
            Some("assistant") => {
                if let Some(blocks) = value["message"]["content"].as_array() {
                    for (index, block) in blocks.iter().enumerate() {
                        let key = value["message"]["id"]
                            .as_str()
                            .or_else(|| value["uuid"].as_str())
                            .map(|id| format!("{id}:{index}"));
                        self.subagent_block_with_key(parent, block, key.as_deref(), true);
                    }
                }
            }
            Some("stream_event") => {
                let event = &value["event"];
                if event["type"] == "content_block_start" {
                    self.subagent_block_with_key(parent, &event["content_block"], None, false);
                }
            }
            Some("user") => {
                if let Some(blocks) = value["message"]["content"].as_array() {
                    for block in blocks {
                        self.subagent_tool_result(block, value.get("tool_use_result"));
                    }
                }
            }
            Some("result") if !self.child_interrupted(parent) => self.state.upsert_subagent(
                parent,
                None,
                None,
                if value["is_error"] == true {
                    SubagentStatus::Failed
                } else {
                    SubagentStatus::Completed
                },
            ),
            _ => {}
        }
    }

    #[cfg(test)]
    pub(super) fn subagent_block(&mut self, parent: &str, block: &Value) {
        self.subagent_block_with_key(parent, block, None, true);
    }

    fn subagent_block_with_key(
        &mut self,
        parent: &str,
        block: &Value,
        event_key: Option<&str>,
        observe: bool,
    ) {
        if observe {
            if let Some(text) = match block["type"].as_str() {
                Some("tool_use") => block["name"]
                    .as_str()
                    .map(|name| format!("Using {name}\n{}", block["input"])),
                Some("text") => block["text"].as_str().map(str::to_owned),
                _ => None,
            } {
                let kind = if block["type"] == "text" {
                    crate::provider::SubagentActivityKind::Commentary
                } else {
                    crate::provider::SubagentActivityKind::Tool
                };
                let key = block["id"]
                    .as_str()
                    .or(event_key)
                    .map_or_else(|| format!("text:{text}"), str::to_owned);
                self.state.observe_subagent(parent, &key, kind, &text);
            }
        }
        match block["type"].as_str() {
            Some("tool_use") => {
                self.register_subagent_from(block, Some(parent));
                if let Some(tool) = block["name"].as_str() {
                    self.state.upsert_subagent(
                        parent,
                        None,
                        Some(&format!("Using {tool}")),
                        SubagentStatus::Unknown,
                    );
                }
            }
            Some("text") => {
                if let Some(text) = block["text"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                {
                    self.state
                        .upsert_subagent(parent, None, Some(text), SubagentStatus::Unknown);
                }
            }
            Some("thinking") => {
                self.state
                    .upsert_subagent(parent, None, Some("Thinking"), SubagentStatus::Unknown);
            }
            _ => {}
        }
    }

    fn subagent_report(&mut self, id: &str, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        let child = self.child_transcripts.entry(id.into()).or_default();
        child.report(text);
        for item in &child.items {
            self.state.upsert_subagent_item(id, item.clone());
        }
        self.state.observe_subagent(
            id,
            &format!("child-final-report:{text}"),
            crate::provider::SubagentActivityKind::Commentary,
            text,
        );
    }

    pub(super) fn subagent_tool_result(&mut self, block: &Value, structured: Option<&Value>) {
        if structured
            .is_some_and(|result| result["isAsync"] == true || result["status"] == "async_launched")
        {
            return;
        }
        let Some(id) = block["tool_use_id"].as_str() else {
            return;
        };
        if !self
            .state
            .saved
            .work_periods
            .iter()
            .any(|period| period.subagents.iter().any(|agent| agent.id == id))
        {
            return;
        }
        if let Some(report) =
            structured.and_then(|result| result["handbackReport"]["text"].as_str())
        {
            self.subagent_report(id, report);
        }
        if let Some(expired) = self.interaction_children.finish(id, None) {
            self.state.expire_interactions(&expired);
            self.state.refresh_interaction_status();
        }
        if self.child_interrupted(id) {
            return;
        }
        self.state.upsert_subagent(
            id,
            None,
            None,
            if block["is_error"] == true {
                SubagentStatus::Failed
            } else {
                SubagentStatus::Completed
            },
        );
    }

    fn child_interrupted(&self, id: &str) -> bool {
        self.state
            .saved
            .work_periods
            .iter()
            .flat_map(|period| &period.subagents)
            .any(|agent| agent.id == id && agent.status == SubagentStatus::Interrupted)
    }

    pub(super) fn subagent_system_message(&mut self, value: &Value) {
        if value["type"] != "system" {
            return;
        }
        let task = value["task_id"].as_str();
        let observed_id = value["tool_use_id"]
            .as_str()
            .or_else(|| task.and_then(|task| self.subagent_tasks.get(task).map(String::as_str)))
            .or(task)
            .map(str::to_owned);
        if let Some(id) = &observed_id {
            let known = self
                .state
                .saved
                .work_periods
                .iter()
                .any(|period| period.subagents.iter().any(|agent| agent.id == *id));
            if known || value["task_type"] == "local_agent" || value["subagent_type"].is_string() {
                let child = self
                    .state
                    .saved
                    .claude_subagents
                    .entry(id.clone())
                    .or_default();
                if let Some(run) = value["run_id"].as_str() {
                    // A Claude Agent tool invocation has one run. A delayed
                    // lifecycle frame must not settle a different observed run.
                    if child
                        .run_id
                        .as_deref()
                        .is_some_and(|current| current != run)
                    {
                        return;
                    }
                    child.run_id = Some(run.into());
                }
                if let Some(task) = task {
                    if !child.task_ids.iter().any(|old| old == task) {
                        child.task_ids.push(task.into());
                    }
                }
            }
        }
        if let Some(task) = task {
            let root = self
                .state
                .saved
                .thread_id
                .as_deref()
                .unwrap_or(&self.state.saved.key);
            if value["subtype"] == "task_started"
                && (value["task_type"] == "local_agent"
                    || value["subagent_type"].is_string()
                    || value["tool_use_id"]
                        .as_str()
                        .is_some_and(|id| self.interaction_children.known(id)))
            {
                let parent = value["parent_task_id"].as_str().unwrap_or(root);
                if self.interaction_children.claude_task(
                    root,
                    parent,
                    task,
                    value["tool_use_id"].as_str(),
                    value["subagent_type"].as_str(),
                ) {
                    self.interaction_children
                        .start(task, value["run_id"].as_str());
                }
            }
            if value["subtype"] == "task_notification"
                || (value["subtype"] == "task_updated"
                    && matches!(
                        value["patch"]["status"].as_str(),
                        Some("completed" | "failed" | "killed")
                    ))
            {
                if let Some(expired) = self
                    .interaction_children
                    .finish(task, value["run_id"].as_str())
                {
                    self.state.expire_interactions(&expired);
                    self.state.refresh_interaction_status();
                }
            }
        }
        if let (Some(task), Some(tool)) = (task, value["tool_use_id"].as_str()) {
            self.subagent_tasks.insert(task.to_owned(), tool.to_owned());
        }
        let Some(id) = value["tool_use_id"]
            .as_str()
            .or_else(|| task.and_then(|task| self.subagent_tasks.get(task).map(String::as_str)))
            .or(task)
            .map(str::to_owned)
        else {
            return;
        };
        let known = self
            .state
            .saved
            .work_periods
            .iter()
            .any(|period| period.subagents.iter().any(|agent| agent.id == id));
        if !known
            && value["task_type"] != "local_agent"
            && value["subagent_type"].as_str().is_none()
        {
            return;
        }
        let mut status = match value["subtype"].as_str() {
            Some("task_started") => SubagentStatus::Working,
            Some("task_progress") => SubagentStatus::Unknown,
            Some("task_notification" | "task_updated") => match value["status"]
                .as_str()
                .or(value["patch"]["status"].as_str())
            {
                Some("completed") => SubagentStatus::Completed,
                Some("failed") => SubagentStatus::Failed,
                Some("stopped" | "killed" | "cancelled" | "canceled" | "interrupted") => {
                    SubagentStatus::Interrupted
                }
                _ => SubagentStatus::Unknown,
            },
            _ => return,
        };
        if self.child_interrupted(&id) {
            status = SubagentStatus::Interrupted;
        }
        let activity = value["summary"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| value["description"].as_str());
        self.state
            .upsert_subagent(&id, value["subagent_type"].as_str(), activity, status);
        self.state.set_subagent_metadata(
            &id,
            value["subagent_type"].as_str(),
            (value["subtype"] == "task_started")
                .then(|| value["description"].as_str())
                .flatten(),
        );
        if value["subtype"] == "task_notification" {
            if let Some(report) = value["handback_report"]["text"]
                .as_str()
                .or(value["handbackReport"]["text"].as_str())
            {
                self.subagent_report(&id, report);
            }
        }
    }
}

#[derive(Default)]
pub(super) struct ChildTranscript {
    transcript: crate::claude_transcript::ClaudeTranscriptState,
    pub(super) items: provider::TranscriptItems,
    message_id: String,
    blocks: HashMap<u64, String>,
}
impl ChildTranscript {
    pub(super) fn restore(items: provider::TranscriptItems) -> Self {
        let mut child = Self {
            items,
            ..Self::default()
        };
        child.transcript.restore(&mut child.items);
        child
    }

    fn assistant_block_id(
        &self,
        message: &str,
        index: usize,
        block: &Value,
        claimed: &[String],
    ) -> String {
        let fallback = format!("{message}-{index}");
        let kind = match block["type"].as_str() {
            Some("text") => "agentMessage",
            _ => return fallback,
        };
        if self.message_id != message {
            return fallback;
        }
        let text = block[if kind == "reasoning" {
            "thinking"
        } else {
            "text"
        }]
        .as_str()
        .unwrap_or_default();
        let mut candidates = self
            .blocks
            .iter()
            .filter_map(|(stream_index, id)| {
                if claimed.contains(id) {
                    return None;
                }
                self.items
                    .iter()
                    .find(|item| item["id"] == *id && item["type"] == kind)
                    .map(|item| (*stream_index, id, item))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(stream_index, _, _)| *stream_index);
        let content = |item: &Value| -> String {
            if kind == "reasoning" {
                item["content"][0].as_str().unwrap_or_default().to_owned()
            } else {
                item["text"].as_str().unwrap_or_default().to_owned()
            }
        };
        if let Some((_, id, _)) = candidates.iter().find(|(_, _, item)| content(item) == text) {
            return (*id).clone();
        }
        if let Some((_, id, _)) = candidates.iter().find(|(_, _, item)| {
            let streamed = content(item);
            !streamed.is_empty() && (text.starts_with(&streamed) || streamed.starts_with(text))
        }) {
            return (*id).clone();
        }
        if let Some((_, id, _)) = candidates
            .iter()
            .find(|(stream_index, _, _)| *stream_index == index as u64)
        {
            return (*id).clone();
        }
        if candidates.len() == 1 {
            return candidates[0].1.clone();
        }
        fallback
    }
    fn upsert(&mut self, item: Value) {
        if let Some(old) = self.items.iter_mut().find(|old| old["id"] == item["id"]) {
            *old = item.into();
        } else {
            self.items.push(item);
        }
    }
    fn report(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if let Some(item) = self
            .items
            .iter_mut()
            .rev()
            .find(|item| item["type"] == "agentMessage" && item["text"].as_str() == Some(text))
        {
            item["phase"] = json!("final_answer");
        } else {
            self.upsert(json!({"id":format!("child-final-report-{}", self.items.len()),"type":"agentMessage","phase":"final_answer","text":text}));
        }
    }
    fn block(&mut self, id: &str, block: &Value, cwd: &str) {
        match block["type"].as_str() {
            Some("text") => {
                self.upsert(json!({"id":id,"type":"agentMessage","text":block["text"]}));
            }
            Some("tool_use") => {
                let id = block["id"].as_str().unwrap_or(id);
                let mut item = if block["name"] == "Bash" {
                    json!({"id":id,"type":"commandExecution","command":block["input"]["command"],"cwd":cwd,"status":"inProgress","aggregatedOutput":""})
                } else {
                    json!({"id":id,"type":"mcpToolCall","server":"Claude Code","tool":block["name"],"arguments":block["input"],"status":"inProgress"})
                };
                if let Some(old) = self.items.iter().find(|old| old["id"] == id) {
                    for key in [
                        "status",
                        "result",
                        "error",
                        "aggregatedOutput",
                        "progress",
                        "structuredResult",
                        "plan",
                        "text",
                        "detached",
                    ] {
                        if let Some(value) = old.get(key) {
                            item[key] = value.clone();
                        }
                    }
                }
                self.transcript.normalize_tool_item(&mut item);
                self.upsert(item);
            }
            _ => {} // Private reasoning is never a child transcript message.
        }
    }
    pub(super) fn message(&mut self, value: &Value, cwd: &str) {
        match value["type"].as_str() {
            Some("assistant") => {
                let message = value["message"]["id"]
                    .as_str()
                    .or(value["uuid"].as_str())
                    .unwrap_or(&self.message_id)
                    .to_owned();
                if let Some(blocks) = value["message"]["content"].as_array() {
                    let mut claimed = Vec::new();
                    for (index, block) in blocks.iter().enumerate() {
                        let id = self.assistant_block_id(&message, index, block, &claimed);
                        self.block(&id, block, cwd);
                        claimed.push(id);
                    }
                }
            }
            Some("stream_event") => {
                let event = &value["event"];
                let index = event["index"].as_u64().unwrap_or(0);
                match event["type"].as_str() {
                    Some("message_start") => {
                        self.message_id = event["message"]["id"]
                            .as_str()
                            .unwrap_or("child-stream")
                            .into();
                        self.blocks.clear();
                    }
                    Some("content_block_start") => {
                        let block = &event["content_block"];
                        let id = block["id"]
                            .as_str()
                            .map_or_else(|| format!("{}-{index}", self.message_id), str::to_owned);
                        self.block(&id, block, cwd);
                        self.blocks.insert(index, id);
                    }
                    Some("content_block_delta") => {
                        if let Some(text) = event["delta"]["text"].as_str() {
                            if let Some(item) = self.blocks.get(&index).and_then(|id| {
                                self.items.iter_mut().rev().find(|item| item["id"] == *id)
                            }) {
                                provider::append_json_text(&mut item["text"], text);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some("user") => {
                let content = &value["message"]["content"];
                let blocks = content.as_array().cloned().unwrap_or_else(|| {
                    content
                        .as_str()
                        .map(|text| vec![json!({"type":"text","text":text})])
                        .unwrap_or_default()
                });
                let visible: Vec<Value> = blocks
                    .iter()
                    .filter(|block| {
                        block["type"] != "tool_result"
                            && block["type"] != "thinking"
                            && block["type"] != "redacted_thinking"
                    })
                    .cloned()
                    .collect();
                if !visible.is_empty() {
                    let id = value["uuid"]
                        .as_str()
                        .or(value["message"]["id"].as_str())
                        .map_or_else(|| format!("child-user-{}", self.items.len()), str::to_owned);
                    self.upsert(json!({"id":id,"type":"userMessage","content":visible}));
                }
                for block in blocks.iter().filter(|block| block["type"] == "tool_result") {
                    if let Some(item) = self
                        .items
                        .iter_mut()
                        .find(|item| item["id"] == block["tool_use_id"])
                    {
                        self.transcript.apply_tool_result(
                            item,
                            &block["content"],
                            value.get("tool_use_result"),
                            block["is_error"] == true,
                        );
                        if item["type"] == "commandExecution" {
                            item["aggregatedOutput"] =
                                json!(block["content"].as_str().map_or_else(
                                    || block["content"]
                                        .as_array()
                                        .into_iter()
                                        .flatten()
                                        .filter_map(|part| part["text"].as_str())
                                        .collect::<Vec<_>>()
                                        .join("\n"),
                                    str::to_owned
                                ));
                        }
                    }
                }
            }
            Some("tool_progress") => {
                if let Some(item) = self
                    .items
                    .iter_mut()
                    .find(|item| item["id"] == value["tool_use_id"])
                {
                    item["progress"] =
                        json!(format!("Running for {}s", value["elapsed_time_seconds"]));
                }
            }
            Some("result") => {
                if let Some(text) = value["result"].as_str() {
                    self.report(text);
                }

                for item in &mut self.items {
                    if item["status"] == "inProgress" && item["detached"] != true {
                        item["status"] = json!(if value["is_error"] == true {
                            "failed"
                        } else {
                            "completed"
                        });
                    }
                }
            }
            _ => {}
        }
    }
}
