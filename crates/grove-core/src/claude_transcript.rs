//! Lossless projection of Claude tools onto Grove's shared transcript rows.
use serde_json::{json, Value};

#[derive(Default)]
pub(crate) struct ClaudeTranscriptState {
    tasks: Vec<(String, String, String)>,
}

impl ClaudeTranscriptState {
    pub(crate) fn restore(&mut self, items: &mut [Value]) {
        self.tasks.clear();
        for item in items {
            self.normalize_tool_item(item);
            if item["status"] == "completed" {
                self.update_tasks(item);
            }
        }
    }

    pub(crate) fn normalize_tool_item(&mut self, item: &mut Value) {
        match item["tool"].as_str().unwrap_or_default() {
            "Edit" | "Write" => {
                item["type"] = json!("fileChange");
                let path = item["arguments"]["file_path"].clone();
                item["changes"] = if path.is_string() && item["status"] != "failed" {
                    json!([{"path":path,"kind":{"type":"update"}}])
                } else {
                    json!([])
                };
                if item["status"] == "completed" {
                    self.file_result(item);
                }
            }
            "WebSearch" | "WebFetch" => {
                item["type"] = json!("webSearch");
                let query = item["arguments"]["query"]
                    .as_str()
                    .or(item["arguments"]["url"].as_str())
                    .unwrap_or_default()
                    .to_owned();
                item["action"] = json!({"type":if item["tool"] == "WebFetch" {"openPage"} else {"search"},"query":query,"url":item["arguments"]["url"]});
            }
            "TaskCreate" | "TaskUpdate" | "TodoWrite"
                if item["status"] == "completed" && item["plan"].is_array() =>
            {
                item["type"] = json!("plan");
            }
            _ => {}
        }
    }

    pub(crate) fn apply_tool_result(
        &mut self,
        item: &mut Value,
        content: &Value,
        structured: Option<&Value>,
        is_error: bool,
    ) {
        let content = if let Some(text) = content.as_str() {
            json!([{"type":"text","text":text}])
        } else {
            content.clone()
        };
        item["result"] = json!({"content":content});
        if let Some(object) = item.as_object_mut() {
            object.remove("error");
            object.remove("detached");
            object.remove("structuredResult");
        }
        if let Some(value) = structured {
            item["structuredResult"] = value.clone();
        }
        item["status"] = json!(if is_error { "failed" } else { "completed" });
        if is_error {
            item["error"] = json!({"message":content.as_array().into_iter().flatten().filter_map(|v|v["text"].as_str()).collect::<Vec<_>>().join("\n")});
            // Input describes an intention, never evidence that a file changed.
            if item["type"] == "fileChange" {
                item["changes"] = json!([]);
            }
            return;
        }
        if structured.is_some_and(|value| value["detachedToolCall"] == true) {
            item["status"] = json!("inProgress");
            item["detached"] = json!(true);
            return;
        }
        self.normalize_tool_item(item);
        self.update_tasks(item);
    }

    fn file_result(&self, item: &mut Value) {
        let result = &item["structuredResult"];
        let Some(path) = result["filePath"]
            .as_str()
            .or(item["arguments"]["file_path"].as_str())
        else {
            return;
        };
        let mut diff = String::new();
        if let Some(hunks) = result["structuredPatch"].as_array() {
            for hunk in hunks {
                use std::fmt::Write;
                let _ = writeln!(
                    diff,
                    "@@ -{},{} +{},{} @@",
                    hunk["oldStart"], hunk["oldLines"], hunk["newStart"], hunk["newLines"]
                );
                for line in hunk["lines"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    diff.push_str(line);
                    diff.push('\n');
                }
            }
        }
        // A create result explicitly certifies the entire added content.
        if diff.is_empty() && result["type"] == "create" {
            if let Some(content) = result["content"].as_str() {
                for line in content.lines() {
                    diff.push('+');
                    diff.push_str(line);
                    diff.push('\n');
                }
            }
        }
        item["changes"] = json!([{"path":path,"kind":{"type":if result["type"] == "create" {"add"} else {"update"}},"diff":diff}]);
    }

    fn update_tasks(&mut self, item: &mut Value) {
        let args = &item["arguments"];
        let result = &item["structuredResult"];
        match item["tool"].as_str().unwrap_or_default() {
            "TodoWrite" => {
                let Some(todos) = args["todos"].as_array() else {
                    return;
                };
                self.tasks = todos
                    .iter()
                    .enumerate()
                    .filter_map(|(i, t)| {
                        Some((
                            format!("todo-{i}"),
                            t["content"].as_str()?.into(),
                            status(t["status"].as_str()).into(),
                        ))
                    })
                    .collect();
            }
            "TaskCreate" => {
                let Some(id) = result["task"]["id"].as_str() else {
                    return;
                };
                let Some(subject) = result["task"]["subject"]
                    .as_str()
                    .or(args["subject"].as_str())
                else {
                    return;
                };
                if !self.tasks.iter().any(|(key, _, _)| key == id) {
                    self.tasks
                        .push((id.into(), subject.into(), "pending".into()));
                }
            }
            "TaskUpdate" => {
                let Some(id) = result["taskId"]
                    .as_str()
                    .or(args["taskId"].as_str())
                    .or(args["id"].as_str())
                    .or(args["task_id"].as_str())
                else {
                    return;
                };
                if result["success"] == false {
                    return;
                }
                let next = args["status"]
                    .as_str()
                    .or(result["statusChange"]["to"].as_str());
                if next == Some("deleted") {
                    self.tasks.retain(|(key, _, _)| key != id);
                } else if let Some((_, subject, state)) =
                    self.tasks.iter_mut().find(|(key, _, _)| key == id)
                {
                    if let Some(text) = args["subject"].as_str() {
                        *subject = text.into();
                    }
                    if next.is_some() {
                        *state = status(next).into();
                    }
                } else {
                    return;
                }
            }
            _ => return,
        }
        let plan: Vec<Value> = self
            .tasks
            .iter()
            .map(|(id, step, status)| json!({"id":id,"step":step,"status":status}))
            .collect();
        item["text"] = json!(self
            .tasks
            .iter()
            .map(|(_, text, state)| format!(
                "- [{}] {}",
                if state == "completed" { "x" } else { " " },
                text
            ))
            .collect::<Vec<_>>()
            .join("\n"));
        item["plan"] = json!(plan);
        item["type"] = json!("plan");
    }
}

fn status(value: Option<&str>) -> &'static str {
    match value {
        Some("completed") => "completed",
        Some("in_progress") => "inProgress",
        _ => "pending",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tool(name: &str, arguments: Value) -> Value {
        json!({"id":"call","type":"mcpToolCall","tool":name,"arguments":arguments,"status":"inProgress"})
    }
    #[test]
    fn file_changes_require_success_and_use_recorded_patch() {
        let mut state = ClaudeTranscriptState::default();
        let mut item = tool(
            "Edit",
            json!({"file_path":"a","old_string":"old","new_string":"new"}),
        );
        state.normalize_tool_item(&mut item);
        assert!(item["changes"][0]["diff"].is_null());
        state.apply_tool_result(&mut item,&json!("done"),Some(&json!({"filePath":"a","structuredPatch":[{"oldStart":1,"oldLines":1,"newStart":1,"newLines":1,"lines":["-old","+new"]}]})),false);
        assert_eq!(item["changes"][0]["diff"], "@@ -1,1 +1,1 @@\n-old\n+new\n");
        assert_eq!(item["arguments"]["old_string"], "old");
        state.apply_tool_result(&mut item, &json!("permission denied"), None, true);
        state.normalize_tool_item(&mut item);
        assert_eq!(item["changes"], json!([]));
        assert_eq!(item["status"], "failed");
    }
    #[test]
    fn creates_use_confirmed_content_and_preserve_results() {
        let mut state = ClaudeTranscriptState::default();
        let mut item = tool("Write", json!({"file_path":"new","content":"intent"}));
        state.apply_tool_result(
            &mut item,
            &json!("created"),
            Some(&json!({"type":"create","filePath":"new","content":"actual\n"})),
            false,
        );
        assert_eq!(item["changes"][0]["kind"]["type"], "add");
        assert_eq!(item["changes"][0]["diff"], "+actual\n");
        assert_eq!(item["result"]["content"][0]["text"], "created");
    }
    #[test]
    fn search_keeps_query_links_and_tool_details() {
        let mut state = ClaudeTranscriptState::default();
        let mut item = tool(
            "WebSearch",
            json!({"query":"docs","allowed_domains":["example.com"]}),
        );
        state.normalize_tool_item(&mut item);
        state.apply_tool_result(
            &mut item,
            &json!("source text"),
            Some(&json!({"results":[{"title":"docs","url":"https://example.com"}]})),
            false,
        );
        assert_eq!(item["action"]["query"], "docs");
        assert_eq!(
            item["structuredResult"]["results"][0]["url"],
            "https://example.com"
        );
        assert_eq!(item["arguments"]["allowed_domains"][0], "example.com");
        let mut fetch = tool(
            "WebFetch",
            json!({"url":"https://example.com","prompt":"summarize"}),
        );
        state.normalize_tool_item(&mut fetch);
        assert_eq!(fetch["action"]["query"], "https://example.com");
    }
    #[test]
    fn task_ids_survive_replay_and_failed_updates_do_not_complete() {
        let mut state = ClaudeTranscriptState::default();
        let mut create = tool("TaskCreate", json!({"subject":"implement"}));
        state.apply_tool_result(
            &mut create,
            &json!("created"),
            Some(&json!({"task":{"id":"9","subject":"implement"}})),
            false,
        );
        let mut failed = tool("TaskUpdate", json!({"taskId":"9","status":"completed"}));
        state.apply_tool_result(&mut failed, &json!("denied"), None, true);
        let mut items = vec![create, failed];
        let mut restored = ClaudeTranscriptState::default();
        restored.restore(&mut items);
        let mut update = tool("TaskUpdate", json!({"taskId":"9","status":"in_progress"}));
        restored.apply_tool_result(
            &mut update,
            &json!("updated"),
            Some(&json!({"success":true,"taskId":"9"})),
            false,
        );
        assert_eq!(update["plan"][0]["status"], "inProgress");
        assert_eq!(update["plan"][0]["step"], "implement");
        let mut unknown = tool("TaskUpdate", json!({"taskId":"1","status":"completed"}));
        restored.apply_tool_result(&mut unknown, &json!("updated"), None, false);
        assert_eq!(unknown["type"], "mcpToolCall");
    }
    #[test]
    fn task_aliases_delete_and_todo_replacement_preserve_actual_state() {
        let mut state = ClaudeTranscriptState::default();
        let mut create = tool("TaskCreate", json!({"subject":"original"}));
        state.apply_tool_result(
            &mut create,
            &json!("created"),
            Some(&json!({"task":{"id":"7"}})),
            false,
        );
        let mut failed = tool(
            "TaskUpdate",
            json!({"task_id":"7","subject":"never applied"}),
        );
        state.apply_tool_result(&mut failed, &json!("failed"), None, true);
        let mut alias = tool("TaskUpdate", json!({"id":"7","status":"completed"}));
        state.apply_tool_result(&mut alias, &json!("updated"), None, false);
        assert_eq!(alias["plan"][0]["step"], "original");
        assert_eq!(alias["plan"][0]["status"], "completed");
        alias["type"] = json!("mcpToolCall");
        state.normalize_tool_item(&mut alias);
        assert_eq!(alias["type"], "plan");
        let mut delete = tool("TaskUpdate", json!({"task_id":"7","status":"deleted"}));
        state.apply_tool_result(&mut delete, &json!("deleted"), None, false);
        assert_eq!(delete["plan"], json!([]));
        let mut todos = tool(
            "TodoWrite",
            json!({"todos":[{"content":"fresh","status":"in_progress"}]}),
        );
        state.apply_tool_result(&mut todos, &json!("updated"), None, false);
        assert_eq!(todos["plan"].as_array().unwrap().len(), 1);
        assert_eq!(todos["plan"][0]["step"], "fresh");
    }

    #[test]
    fn empty_streamed_arguments_can_be_completed_without_losing_result() {
        let mut state = ClaudeTranscriptState::default();
        let mut item = tool("WebSearch", json!({}));
        state.normalize_tool_item(&mut item);
        state.apply_tool_result(&mut item, &json!("answer"), None, false);
        item["arguments"] = json!({"query":"final input"});
        state.normalize_tool_item(&mut item);
        assert_eq!(item["id"], "call");
        assert_eq!(item["action"]["query"], "final input");
        assert_eq!(item["result"]["content"][0]["text"], "answer");
        assert_eq!(item["status"], "completed");
    }
}
