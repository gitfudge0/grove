//! Exact request boundaries shared by retained change-analysis conversations.
use crate::{
    change_description::Snapshot,
    change_result_store::{PendingRequest, StoredResult},
    codex::{Snapshot as Session, Status},
};
pub trait Workflow: StoredResult {
    const NAME: &'static str;
    const LABEL: &'static str;
    const FENCE: &'static str;
    fn prompt(snapshot: &Snapshot, input: &str) -> Result<String, String>;
    fn parse(snapshot: Snapshot, input: &str, text: &str) -> Result<Self, String>;
}
/// Only the final response between this exact request and the next user turn is eligible.
pub fn completed_text(
    state: &Session,
    request: &PendingRequest,
    start_item: usize,
    recovered: bool,
) -> Option<Result<String, String>> {
    let index = state
        .saved
        .items
        .iter()
        .enumerate()
        .skip(start_item)
        .rev()
        .find(|(_, item)| {
            item["type"] == "userMessage"
                && item["content"].as_array().is_some_and(|parts| {
                    parts
                        .iter()
                        .any(|part| part["text"].as_str() == Some(request.prompt.as_str()))
                })
        })
        .map(|(i, _)| i);
    let complete = index.is_some_and(|i| {
        state
            .saved
            .work_periods
            .iter()
            .any(|p| p.start_item <= i && p.end_item.is_some_and(|end| end > i))
    });
    if !complete && (state.status.busy() || state.turn_id.is_some()) {
        return None;
    }
    let recovered_idle = recovered
        && state.saved.thread_id.is_some()
        && matches!(state.status, Status::Idle | Status::Done);
    if !complete && !recovered_idle && state.error.is_none() && state.status != Status::Disconnected
    {
        return None;
    }
    if !complete {
        if let Some(error) = &state.error {
            return Some(Err(error.clone()));
        }
    }
    let Some(index) = index else {
        return Some(Err(
            "Analysis was interrupted before its captured request completed; regenerate explicitly"
                .into(),
        ));
    };
    let end = state
        .saved
        .items
        .iter()
        .enumerate()
        .skip(index + 1)
        .find(|(_, item)| item["type"] == "userMessage")
        .map_or(state.saved.items.len(), |(i, _)| i);
    let text = state
        .saved
        .items
        .iter()
        .take(end)
        .skip(index + 1)
        .rev()
        .find(|item| item["type"] == "agentMessage" && item["phase"].as_str() != Some("commentary"))
        .and_then(|item| item["text"].as_str())
        .unwrap_or("");
    Some(Ok(text.to_owned()))
}
/// Accept raw JSON or exactly one unambiguous named fenced JSON result.
pub fn json_payload(text: &str, fence: &str) -> Result<serde_json::Value, String> {
    if text.len() > 2 * 1024 * 1024 {
        return Err("Analysis response exceeds its import limit".into());
    }
    let text = text.trim();
    if text.starts_with('{') {
        return serde_json::from_str(text).map_err(|_| "Analysis returned invalid JSON".into());
    }
    let marker = format!("```{fence}");
    let mut result = None;
    let mut current = None::<String>;
    let mut other = false;
    for line in text.lines() {
        let m = line.trim();
        if let Some(content) = &mut current {
            if m == "```" {
                result = current.take();
            } else if m.starts_with("```") {
                return Err("Analysis result contains an invalid fence".into());
            } else {
                content.push_str(line);
                content.push('\n');
            }
        } else if other {
            if m == "```" {
                other = false;
            }
        } else if m == marker {
            if result.is_some() {
                return Err("Analysis response contains duplicate result blocks".into());
            }
            current = Some(String::new());
        } else if m.starts_with(&marker) {
            return Err("Analysis result marker is invalid".into());
        } else if m.starts_with("```") {
            other = true;
        }
    }
    if current.is_some() {
        return Err("Analysis result block is incomplete".into());
    }
    serde_json::from_str(&result.ok_or("Analysis response has no complete result block")?)
        .map_err(|_| "Analysis returned invalid JSON".into())
}

/// Stable transport marker, independent of editable writing instructions.
pub fn compose_request(name: &str, display: &str, generation: &str, body: &str) -> String {
    format!(
        "Grove captured {name} request.\n{}\n\n{body}",
        serde_json::json!({"display":display,"generation":generation})
    )
}
/// Presentation only: callers must limit this filter to their linked workflow sessions.
pub fn display_text(name: &str, fence: &str, kind: &str, text: &str) -> String {
    if matches!(kind, "userMessage" | "user") {
        let marker = format!("Grove captured {name} request.\n");
        if let Some(rest) = text.strip_prefix(&marker) {
            if let Some(line) = rest.lines().next().filter(|line| line.len() <= 64 * 1024) {
                if let Ok(meta) = serde_json::from_str::<serde_json::Value>(line) {
                    if let Some(display) = meta["display"].as_str().filter(|s| s.len() <= 64 * 1024)
                    {
                        return display.to_owned();
                    }
                }
            }
        }
        return text.into();
    }
    if !matches!(kind, "agentMessage" | "assistant") {
        return text.into();
    }
    let marker = format!("```{fence}");
    let mut hiding = false;
    let mut other = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let m = line.trim();
        if hiding {
            if m == "```" {
                hiding = false;
            }
            continue;
        }
        if other {
            out.push(line);
            if m == "```" {
                other = false;
            }
            continue;
        }
        if m == marker {
            hiding = true;
            continue;
        }
        if m.starts_with("```") {
            other = true;
        }
        out.push(line);
    }
    out.join("\n").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        change_description::Scope,
        codex::{Provider, SavedSession, WorkPeriod},
    };
    use serde_json::json;
    fn request() -> PendingRequest {
        PendingRequest {
            snapshot: Snapshot {
                checkout: "/fixture".into(),
                scope: Scope::Local,
                fingerprint: "id".into(),
                label: "Local".into(),
                head: "head".into(),
                base: "head".into(),
                files: vec![],
                title: String::new(),
                original_body: None,
            },
            input: "Earlier question".into(),
            prompt: compose_request(
                "ask",
                "/ask local Earlier question",
                "generation-one",
                "Original custom writing instructions",
            ),
            generation: "generation-one".into(),
        }
    }
    #[test]
    fn completed_response_is_bound_to_exact_request_before_followup() {
        for provider in [Provider::Codex, Provider::Claude] {
            let job = request();
            let mut state = Session::new(SavedSession::empty(
                "session".into(),
                provider,
                String::new(),
                "/fixture".into(),
            ));
            state.status = Status::Done;
            state.saved.thread_id = Some("thread".into());
            state.saved.items=vec![json!({"type":"userMessage","content":[{"text":job.prompt}]}),json!({"type":"agentMessage","phase":"commentary","text":"Working"}),json!({"type":"agentMessage","phase":"final_answer","text":"Exact result"}),json!({"type":"userMessage","content":[{"text":"Follow up"}]}),json!({"type":"agentMessage","phase":"final_answer","text":"Unrelated later answer"})].into();
            state.saved.work_periods.push(WorkPeriod {
                start_item: 0,
                end_item: Some(3),
                duration_ms: None,
                subagents: vec![],
            });
            assert_eq!(
                completed_text(&state, &job, 0, false),
                Some(Ok("Exact result".into()))
            );
            let mut other = job;
            other.prompt.push_str(" changed prompt");
            assert!(completed_text(&state, &other, 0, true).unwrap().is_err());
        }
    }
    #[test]
    fn recovered_uncertain_request_is_never_a_new_submission() {
        let job = request();
        let mut state = Session::new(SavedSession::empty(
            "session".into(),
            Provider::Codex,
            String::new(),
            "/fixture".into(),
        ));
        state.status = Status::Working;
        state.turn_id = Some("active".into());
        assert_eq!(completed_text(&state, &job, 0, true), None);
        state.status = Status::Idle;
        state.turn_id = None;
        state.saved.thread_id = Some("thread".into());
        assert!(completed_text(&state, &job, 0, true).unwrap().is_err());
    }
    #[test]
    fn named_json_rejects_duplicate_partial_or_quoted_blocks() {
        assert_eq!(
            json_payload(
                "Summary\n```grove-review\n{\"ok\":true}\n```",
                "grove-review"
            )
            .unwrap(),
            json!({"ok":true})
        );
        for text in [
            "```grove-review\n{}",
            "```grove-review\n{}\n```\n```grove-review\n{}\n```",
            "```text\n```grove-review\n{}\n```\n```",
        ] {
            assert!(json_payload(text, "grove-review").is_err());
        }
    }
    #[test]
    fn display_is_per_request_and_independent_of_prompt_customization() {
        let old = compose_request(
            "ask",
            "/ask local Earlier question",
            "old",
            "French custom instructions",
        );
        let new = compose_request(
            "ask",
            "/ask pr Later question",
            "new",
            "Different instructions",
        );
        assert_eq!(
            display_text("ask", "grove-ask", "userMessage", &old),
            "/ask local Earlier question"
        );
        assert_eq!(
            display_text("ask", "grove-ask", "userMessage", &new),
            "/ask pr Later question"
        );
        assert_eq!(
            display_text("review", "grove-review", "userMessage", &old),
            old
        );
        let response =
            "Useful answer\n```rust\nlet x = 1;\n```\n```grove-ask\n{}\n```\nMore explanation";
        assert_eq!(
            display_text("ask", "grove-ask", "agentMessage", response),
            "Useful answer\n```rust\nlet x = 1;\n```\nMore explanation"
        );
    }
}
