//! Tool grouping, work periods, compaction, and live turn activity.

use super::*;

/// During a turn only its latest activity is visible, including between calls.
/// Completion restores the work disclosure's full history.
#[cfg(test)]
pub(super) fn tool_visible(state: &codex::Snapshot, index: usize) -> bool {
    if !state.status.busy() {
        return true;
    }
    let start = active_work_start(state);
    if index < start {
        return true;
    }
    state
        .saved
        .items
        .iter()
        .enumerate()
        .rev()
        .find(|(index, item)| *index >= start && is_tool_item(item))
        .is_some_and(|(current, _)| current == index)
}

/// New turns can start without a new user message (for example, manual compaction).
pub(super) fn active_work_start(state: &codex::Snapshot) -> usize {
    state
        .saved
        .work_periods
        .iter()
        .rev()
        .find(|period| period.end_item.is_none())
        .map_or_else(
            || {
                state
                    .saved
                    .items
                    .iter()
                    .rposition(|item| item["type"] == "userMessage")
                    .unwrap_or(0)
            },
            |period| period.start_item,
        )
}

#[cfg(test)]
pub(super) fn work_commentary(state: &codex::Snapshot, index: usize) -> bool {
    let item = &state.saved.items[index];
    if item["type"] != "agentMessage" {
        return false;
    }
    match item["phase"].as_str() {
        Some("commentary") => true,
        Some("final_answer") => false,
        _ => state.saved.items[index + 1..]
            .iter()
            .take_while(|item| item["type"] != "userMessage")
            .any(|item| item["type"] == "agentMessage" || is_tool_item(item)),
    }
}

pub(super) fn work_is_active_at(state: &codex::Snapshot, index: usize) -> bool {
    state.status.busy()
        && index >= active_work_start(state)
        && state
            .saved
            .work_periods
            .iter()
            .rev()
            .find(|period| period.start_item <= index)
            .is_none_or(|period| period.end_item.is_none())
}

pub(super) fn work_key_at(state: &codex::Snapshot, index: usize) -> String {
    let start = state.saved.items[..(index + 1).min(state.saved.items.len())]
        .iter()
        .rposition(|item| item["type"] == "userMessage")
        .unwrap_or(0);
    format!("work-{start}")
}

fn work_period_at(state: &codex::Snapshot, index: usize) -> Option<&codex::WorkPeriod> {
    state
        .saved
        .work_periods
        .iter()
        .rev()
        .find(|period| period.start_item <= index && period.end_item.is_none_or(|end| index < end))
}

/// Place work metadata after the user prompt, keeping the answer outside disclosure.
pub(super) fn work_header_at(
    state: &codex::Snapshot,
    index: usize,
) -> Option<(String, String, bool)> {
    if state
        .saved
        .items
        .get(index)
        .is_some_and(|item| item["type"] == "userMessage")
    {
        return None;
    }
    let period = work_period_at(state, index);
    let fallback_start = state.saved.items[..(index + 1).min(state.saved.items.len())]
        .iter()
        .rposition(|item| item["type"] == "userMessage")
        .unwrap_or(0);
    let start = period.map_or(fallback_start, |period| period.start_item);
    let active = work_is_active_at(state, index);
    let end = period
        .and_then(|period| period.end_item)
        .unwrap_or(state.saved.items.len());
    let first = (start..end)
        .find(|index| state.saved.items[*index]["type"] != "userMessage")
        .or_else(|| active.then_some(state.saved.items.len()))?;
    if first != index {
        return None;
    }
    let duration = if active {
        state.work_started.map(|start| start.elapsed().as_secs())
    } else {
        period.and_then(|period| period.duration_ms.map(|ms| ms / 1000))
    };
    Some((
        work_key_at(state, index),
        work_duration_label(active, duration),
        active,
    ))
}

pub(super) fn work_duration_label(active: bool, duration: Option<u64>) -> String {
    let verb = if active { "Working" } else { "Worked" };
    duration.map_or_else(
        || verb.into(),
        |seconds| {
            let minutes = seconds / 60;
            if minutes == 0 {
                format!("{verb} for {seconds}s")
            } else if minutes == 1 {
                format!("{verb} for {minutes}m {}s", seconds % 60)
            } else {
                format!("{verb} for {minutes}m")
            }
        },
    )
}

/// Only parent commentary and tools belong to the work disclosure.
#[cfg(test)]
pub(super) fn work_has_parent_activity(state: &codex::Snapshot, index: usize) -> bool {
    let start = work_period_at(state, index).map_or_else(
        || {
            state.saved.items[..index.min(state.saved.items.len())]
                .iter()
                .rposition(|item| item["type"] == "userMessage")
                .unwrap_or(0)
        },
        |period| period.start_item,
    );
    let end = work_period_at(state, index)
        .and_then(|period| period.end_item)
        .unwrap_or(state.saved.items.len());
    (start.min(state.saved.items.len())..end.min(state.saved.items.len())).any(|index| {
        let item = &state.saved.items[index];
        (is_tool_item(item) && !tasks::is_checklist(item)) || work_commentary(state, index)
    })
}

pub(super) fn tool_expansion_key(item: &serde_json::Value, index: usize) -> String {
    format!(
        "tool-{}",
        item["id"]
            .as_str()
            .map_or_else(|| index.to_string(), str::to_owned)
    )
}

pub(super) fn tool_activity_label(
    label: String,
    index: usize,
    running: bool,
    cx: &App,
) -> gpui::AnyElement {
    shimmer_label(("codex-tool-shimmer", index).into(), label, running, cx)
}

pub(super) fn tool_is_running(state: &codex::Snapshot, index: usize) -> bool {
    if !matches!(
        state.status,
        codex::Status::Starting | codex::Status::Working
    ) {
        return false;
    }
    let Some(item) = state.saved.items.get(index) else {
        return false;
    };
    item["status"] == "inProgress"
        // Claude reasoning has no item status; only its current trailing block is active.
        || (state.saved.provider == codex::Provider::Claude
            && item.get("status").is_none()
            && item["type"] == "reasoning"
            && index + 1 == state.saved.items.len())
}

pub(super) fn is_tool_item(item: &serde_json::Value) -> bool {
    !grove_core::activity::is_internal(item)
        && !matches!(
            item["type"].as_str(),
            Some(
                "userMessage"
                    | "agentMessage"
                    | "plan"
                    | "providerNotice"
                    | "subAgentActivity"
                    | codex::COMPACTION_ITEM
            )
        )
}

/// Compaction is running: the provider said so, or its item has not completed yet.
pub(super) fn compacting(state: &codex::Snapshot) -> bool {
    state.compacting
        || state.saved.items.last().is_some_and(|item| {
            item["type"] == codex::COMPACTION_ITEM && item["status"] == "inProgress"
        })
}

/// The live activity line's text, or `None` when the session is not working.
pub(super) fn live_activity(state: &codex::Snapshot) -> Option<String> {
    if let Some(notice) = state
        .effective_notice()
        .filter(|notice| notice.kind == codex::SessionNoticeKind::Retrying)
    {
        return Some(format!("Retrying automatically · {}", notice.body));
    }
    let provider = state.saved.provider.label();
    if state.status == codex::Status::Starting && state.saved.provider == codex::Provider::Claude {
        return Some(format!("Connecting to {provider}…"));
    }
    if compacting(state) {
        return Some("Compacting context…".into());
    }
    if !state.status.busy()
        || (state
            .saved
            .items
            .last()
            .is_some_and(|item| is_tool_item(item))
            && state.questions.is_empty()
            && state.elicitations.is_empty()
            && state.approvals.is_empty())
    {
        return None;
    }
    // The active work disclosure already provides the turn's liveness signal.
    // Keep the initial fallback when no work disclosure has mounted yet.
    let current_start = state
        .saved
        .work_periods
        .last()
        .filter(|period| period.end_item.is_none())
        .map_or_else(
            || {
                state
                    .saved
                    .items
                    .iter()
                    .rposition(|item| item["type"] == "userMessage")
                    .unwrap_or(0)
            },
            |period| period.start_item,
        );
    let header_index = (current_start..state.saved.items.len())
        .find(|index| state.saved.items[*index]["type"] != "userMessage")
        .unwrap_or(state.saved.items.len());
    let has_work_header = work_header_at(state, header_index).is_some_and(|(_, _, active)| active);
    if state.questions.is_empty()
        && state.elicitations.is_empty()
        && state.approvals.is_empty()
        && has_work_header
        && !(state.saved.items.is_empty() && state.saved.thread_id.is_none())
    {
        return None;
    }
    Some(
        if !state.questions.is_empty() || !state.elicitations.is_empty() {
            "Waiting for your answers…".into()
        } else if !state.approvals.is_empty() {
            "Waiting for your permission…".into()
        } else {
            state.saved.items.last().map_or_else(
                || {
                    if state.saved.thread_id.is_none() {
                        format!("Connecting to {provider}…")
                    } else {
                        format!("{provider} is working…")
                    }
                },
                |item| match item["type"].as_str().unwrap_or("") {
                    "commandExecution" => {
                        format!("Running {}", item["command"].as_str().unwrap_or("command"))
                    }
                    "fileChange" => "Updating files…".into(),
                    "mcpToolCall" => "Using a connected tool…".into(),
                    "webSearch" => "Searching the web…".into(),
                    "reasoning" => "Thinking…".into(),
                    _ => format!("{provider} is working…"),
                },
            )
        },
    )
}

/// A centred transcript divider between hairline rules, e.g. "Context compacted · auto".
pub(super) fn compaction_divider(index: usize, label: String) -> impl IntoElement {
    let rule = || {
        div()
            .flex_1()
            .min_w_0()
            .h(gpui::px(1.))
            .bg(c::BORDER_SOFT())
    };
    div()
        .id(("codex-compaction", index))
        .debug_selector(move || format!("codex-compaction-{index}"))
        .w_full()
        .max_w(rpx(CHAT_MAX_W))
        .flex()
        .items_center()
        .gap(rpx(SPACE_2XL))
        .text_size(rpx(TEXT_SMALL))
        .text_color(c::FG_DIM())
        .aria_label(label.clone())
        .child(rule())
        .child(div().flex_shrink_0().child(label))
        .child(rule())
}

/// Choose the operation identity independently of whether its output is expanded.
pub(super) fn tool_activity_icon(item: &serde_json::Value) -> &'static str {
    if tool_failed(item) {
        return "message-error";
    }
    match item["type"].as_str().unwrap_or_default() {
        "reasoning" => "brain",
        "commandExecution" => "terminal",
        "fileChange" => "edit",
        "webSearch" => "search",
        "plan" => "list",
        _ => {
            let action = item["tool"]
                .as_str()
                .unwrap_or_default()
                .rsplit("__")
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            match action.as_str() {
                "bash" | "shell" | "exec_command" | "write_stdin" => "terminal",
                "read" | "read_file" => "file",
                "write" | "write_file" | "edit" | "multiedit" | "apply_patch" => "edit",
                "search" | "grep" | "glob" | "websearch" => "search",
                "fetch" | "webfetch" => "external-link",
                "think" | "thought" | "thinking" => "brain",
                "wait" | "wait_agent" | "sleep" => "status-limited",
                _ => "tool",
            }
        }
    }
}

#[cfg(test)]
#[test]
fn tool_icons_distinguish_codex_and_claude_operations() {
    use serde_json::json;
    // Normalized Codex items and Claude tool names use the same renderer.
    for (item, expected) in [
        (json!({"type":"commandExecution"}), "terminal"),
        (json!({"type":"fileChange"}), "edit"),
        (json!({"type":"webSearch"}), "search"),
        (json!({"type":"reasoning"}), "brain"),
        (json!({"type":"mcpToolCall", "tool":"Bash"}), "terminal"),
        (json!({"type":"mcpToolCall", "tool":"Read"}), "file"),
        (json!({"type":"mcpToolCall", "tool":"Edit"}), "edit"),
        (
            json!({"type":"mcpToolCall", "tool":"mcp__docs__search"}),
            "search",
        ),
        (
            json!({"type":"mcpToolCall", "tool":"custom_integration"}),
            "tool",
        ),
        (
            json!({"type":"mcpToolCall", "tool":"Read", "is_error":true}),
            "message-error",
        ),
    ] {
        assert_eq!(tool_activity_icon(&item), expected, "{item}");
    }
}

pub(super) fn tool_failed(item: &serde_json::Value) -> bool {
    item["status"] == "failed"
        || item["is_error"] == true
        || item["exitCode"].as_i64().is_some_and(|code| code != 0)
        || item.get("error").is_some_and(|error| !error.is_null())
}

/// Quiet activity rows describe the invocation; output stays behind disclosure.
pub(super) fn tool_activity_summary(item: &serde_json::Value, running: bool) -> String {
    if item["type"] == "reasoning" {
        return "Thinking…".into();
    }
    if item["type"] == "commandExecution" {
        let command = item["command"].as_str().unwrap_or("command");
        return format!(
            "{} {}",
            if running {
                "Running"
            } else if tool_failed(item) {
                "Command failed ·"
            } else {
                "Ran"
            },
            command.split_whitespace().collect::<Vec<_>>().join(" ")
        ) + if running { "…" } else { "" };
    }
    // Waiting tools keep one label after completion; their raw identity stays
    // in the disclosure, like every other tool's arguments and output.
    if item["tool"].as_str().is_some_and(|tool| {
        matches!(
            tool.rsplit("__")
                .next()
                .unwrap_or(tool)
                .to_ascii_lowercase()
                .as_str(),
            "wait" | "wait_agent"
        )
    }) {
        return "Waiting…".into();
    }
    if !running {
        return if tool_failed(item) {
            format!("Tool failed · {}", tool_summary(item))
        } else {
            tool_summary(item)
        };
    }
    match item["type"].as_str().unwrap_or_default() {
        "fileChange" => "Updating files…".into(),
        "webSearch" => "Searching the web…".into(),
        _ => {
            let tool = item["tool"].as_str().unwrap_or_default();
            // Namespaced MCP tools retain their full identity in the disclosure.
            let action = tool
                .rsplit("__")
                .next()
                .unwrap_or(tool)
                .to_ascii_lowercase();
            match action.as_str() {
                "sleep" => "Sleeping…".into(),
                "wait" | "wait_agent" => "Waiting…".into(),
                "think" | "thought" | "thinking" => "Thinking…".into(),
                "read" => "Reading…".into(),
                "write" => "Writing…".into(),
                "edit" | "multiedit" => "Editing…".into(),
                "search" | "grep" | "glob" | "websearch" => "Searching…".into(),
                "fetch" | "webfetch" => "Fetching…".into(),
                _ if !tool.is_empty() => format!("Using {tool}…"),
                _ => "Working…".into(),
            }
        }
    }
}

pub(super) fn tool_summary(item: &serde_json::Value) -> String {
    let summary = match item["type"].as_str().unwrap_or_default() {
        "commandExecution" => format!(
            "Command · {}",
            item["command"].as_str().unwrap_or("Starting command…")
        ),
        "fileChange" => {
            let paths = item["changes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|change| change["path"].as_str())
                .collect::<Vec<_>>();
            if paths.is_empty() {
                "File changes".into()
            } else {
                format!("File changes · {}", paths.join(", "))
            }
        }
        "mcpToolCall" => format!(
            "Tool · {} / {}",
            item["server"].as_str().unwrap_or("Connected tool"),
            item["tool"].as_str().unwrap_or("Starting…")
        ),
        "webSearch" => format!(
            "Web search · {}",
            item["action"]["query"]
                .as_str()
                .or(item["query"].as_str())
                .unwrap_or("Searching…")
        ),
        "reasoning" => "Thinking…".into(),
        _ => item["tool"]
            .as_str()
            .or(item["type"].as_str())
            .unwrap_or("Tool call")
            .to_owned(),
    };
    // Keep multiline commands and queries in the disclosure's single summary row.
    summary.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn tool_detail(item: &serde_json::Value) -> String {
    let mut text = if item["type"] == "plan" && item["tool"].is_string() {
        String::new()
    } else {
        codex::item_text(item)
    };
    if matches!(
        item["type"].as_str(),
        Some("fileChange" | "webSearch" | "plan")
    ) {
        if let Some(content) = item["result"]["content"].as_array() {
            for result in content.iter().filter_map(|block| block["text"].as_str()) {
                if !result.is_empty() && !text.contains(result) {
                    text.push_str("\n\n");
                    text.push_str(result);
                }
            }
        }
        if let Some(error) = item["error"]
            .as_str()
            .or_else(|| item["error"]["message"].as_str())
        {
            if !text.contains(error) {
                text.push_str("\n\n");
                text.push_str(error);
            }
        }
        if let Some(arguments) = item.get("arguments") {
            return format!(
                "Arguments:\n{}\n\n{}",
                serde_json::to_string_pretty(arguments).unwrap_or_default(),
                text.trim()
            );
        }
    }
    match item["type"].as_str().unwrap_or_default() {
        "mcpToolCall" => {
            let arguments = item.get("arguments").or_else(|| item.get("input"));
            match arguments {
                Some(arguments) => format!(
                    "Arguments:\n{}\n\n{text}",
                    serde_json::to_string_pretty(arguments).unwrap_or_default()
                ),
                None => text,
            }
        }
        "commandExecution" | "fileChange" | "reasoning" | "webSearch" | "plan" => text,
        _ => serde_json::to_string_pretty(item).unwrap_or(text),
    }
}
