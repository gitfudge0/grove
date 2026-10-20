//! Lossless projection of Claude tools onto Grove's shared transcript rows.
use serde_json::{json, Value};

#[derive(Default)]
pub(crate) struct ClaudeTranscriptState {
    tasks: Vec<TaskEntry>,
}

/// One folded task: Claude's id, subject, normalised status and optional
/// present-continuous `activeForm` label.
struct TaskEntry {
    id: String,
    step: String,
    status: &'static str,
    active_form: Option<String>,
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
            "TaskCreate" | "TaskUpdate" | "TodoWrite" | "TaskList"
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
                        Some(TaskEntry {
                            id: format!("todo-{i}"),
                            step: t["content"].as_str()?.into(),
                            status: status(t["status"].as_str()),
                            active_form: active_form(&t["activeForm"]),
                        })
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
                if !self.tasks.iter().any(|task| task.id == id) {
                    self.tasks.push(TaskEntry {
                        id: id.into(),
                        step: subject.into(),
                        status: "pending",
                        active_form: active_form(&result["task"]["activeForm"])
                            .or_else(|| active_form(&args["activeForm"])),
                    });
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
                    self.tasks.retain(|task| task.id != id);
                } else if let Some(task) = self.tasks.iter_mut().find(|task| task.id == id) {
                    if let Some(text) = args["subject"].as_str() {
                        task.step = text.into();
                    }
                    if let Some(form) = active_form(&args["activeForm"]) {
                        task.active_form = Some(form);
                    }
                    if next.is_some() {
                        task.status = status(next);
                    }
                } else {
                    return;
                }
            }
            "TaskList" => {
                // Authoritative snapshot of the task store. Shape accepted
                // defensively: `{tasks:[{id, subject, status, activeForm?}]}`;
                // anything else (or an empty list) leaves state untouched.
                let Some(tasks) = result["tasks"].as_array().filter(|t| !t.is_empty()) else {
                    return;
                };
                let next: Vec<TaskEntry> = tasks
                    .iter()
                    .filter_map(|t| {
                        let id = match &t["id"] {
                            Value::String(id) => id.clone(),
                            Value::Number(id) => id.to_string(),
                            _ => return None,
                        };
                        let step = t["subject"].as_str().or(t["content"].as_str())?;
                        let previous = self.tasks.iter().find(|task| task.id == id);
                        Some(TaskEntry {
                            active_form: active_form(&t["activeForm"])
                                .or_else(|| previous.and_then(|task| task.active_form.clone())),
                            id,
                            step: step.into(),
                            status: status(t["status"].as_str()),
                        })
                    })
                    .collect();
                if next.is_empty() {
                    return;
                }
                self.tasks = next;
            }
            _ => return,
        }
        let plan: Vec<Value> = self
            .tasks
            .iter()
            .map(|task| {
                let mut entry = json!({"id":task.id,"step":task.step,"status":task.status});
                if let Some(form) = &task.active_form {
                    entry["activeForm"] = json!(form);
                }
                entry
            })
            .collect();
        item["text"] = json!(self
            .tasks
            .iter()
            .map(|task| format!(
                "- [{}] {}",
                if task.status == "completed" { "x" } else { " " },
                task.step
            ))
            .collect::<Vec<_>>()
            .join("\n"));
        item["plan"] = json!(plan);
        item["type"] = json!("plan");
    }
}

fn status(value: Option<&str>) -> &'static str {
    crate::tasks::normalize_status(value)
}

fn active_form(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|form| !form.is_empty())
        .map(str::to_owned)
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
    fn todo_write_keeps_active_form() {
        let mut state = ClaudeTranscriptState::default();
        let mut todos = tool(
            "TodoWrite",
            json!({"todos":[
                {"content":"Run tests","activeForm":"Running tests","status":"in_progress"},
                {"content":"Ship","status":"pending"}
            ]}),
        );
        state.apply_tool_result(&mut todos, &json!("ok"), None, false);
        assert_eq!(todos["plan"][0]["activeForm"], "Running tests");
        assert_eq!(todos["plan"][0]["status"], "inProgress");
        assert!(todos["plan"][1].get("activeForm").is_none());
        let list = crate::tasks::current(std::slice::from_ref(&todos)).unwrap();
        assert_eq!(list.now_label(), Some("Running tests"));
    }

    #[test]
    fn task_list_replaces_the_store_from_its_structured_result() {
        let mut state = ClaudeTranscriptState::default();
        let mut create = tool(
            "TaskCreate",
            json!({"subject":"stale","activeForm":"Doing stale"}),
        );
        state.apply_tool_result(
            &mut create,
            &json!("created"),
            Some(&json!({"task":{"id":"1","subject":"stale"}})),
            false,
        );
        assert_eq!(create["plan"][0]["activeForm"], "Doing stale");
        let mut list = tool("TaskList", json!({}));
        state.apply_tool_result(
            &mut list,
            &json!("listed"),
            Some(&json!({"tasks":[
                {"id":"1","subject":"stale","status":"completed"},
                {"id":2,"subject":"fresh","status":"in_progress"}
            ]})),
            false,
        );
        assert_eq!(list["type"], "plan");
        assert_eq!(list["plan"].as_array().unwrap().len(), 2);
        assert_eq!(list["plan"][0]["status"], "completed");
        assert_eq!(list["plan"][0]["activeForm"], "Doing stale");
        assert_eq!(list["plan"][1]["id"], "2");
        assert_eq!(list["plan"][1]["status"], "inProgress");
        let mut empty = tool("TaskList", json!({}));
        state.apply_tool_result(
            &mut empty,
            &json!("none"),
            Some(&json!({"tasks":[]})),
            false,
        );
        assert_eq!(empty["type"], "mcpToolCall");
        let mut unknown = tool("TaskList", json!({}));
        state.apply_tool_result(&mut unknown, &json!("text only"), None, false);
        assert_eq!(unknown["type"], "mcpToolCall");
        let mut items = vec![create, list];
        let mut restored = ClaudeTranscriptState::default();
        restored.restore(&mut items);
        assert_eq!(items[1]["type"], "plan");
        assert_eq!(items[1]["plan"][1]["step"], "fresh");
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
