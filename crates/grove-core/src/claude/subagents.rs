//! Claude child stream attribution and subagent lifecycle.

use super::*;

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
