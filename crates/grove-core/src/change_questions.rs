//! Tool-free questions about one immutable change snapshot; no repository writes.
use crate::{
    change_description::{self, Scope, Snapshot},
    codex::Provider,
    commit_agent::{self, CommitControl},
};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fmt::Write as _, path::Path};

const SCHEMA: &str = r#"{"type":"object","properties":{"answer":{"type":"string"},"citations":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"side":{"type":"string","enum":["old","new"]},"start":{"type":"integer"},"end":{"type":"integer"},"explanation":{"type":"string"}},"required":["path","side","start","end","explanation"],"additionalProperties":false}},"examined_files":{"type":"array","items":{"type":"string"}},"limitations":{"type":"array","items":{"type":"string"}}},"required":["answer","citations","examined_files","limitations"],"additionalProperties":false}"#;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Old,
    New,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Citation {
    pub path: String,
    pub side: Side,
    pub start: u32,
    pub end: u32,
    pub explanation: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Answer {
    pub snapshot: Snapshot,
    pub question: String,
    pub answer: String,
    pub citations: Vec<Citation>,
    pub examined_files: Vec<String>,
    pub limitations: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prediction {
    answer: String,
    citations: Vec<Citation>,
    examined_files: Vec<String>,
    limitations: Vec<String>,
}
impl Answer {
    pub fn markdown(&self) -> String {
        let mut text = format!(
            "## Question\n\n{}\n\n## Answer\n\n{}\n",
            self.question, self.answer
        );
        if !self.citations.is_empty() {
            text.push_str("\n## Captured references\n\n");
            for cite in &self.citations {
                let _ = writeln!(
                    text,
                    "- `{}` ({} lines {}–{}): {}",
                    cite.path.replace('`', "'"),
                    if cite.side == Side::New { "new" } else { "old" },
                    cite.start,
                    cite.end,
                    cite.explanation
                );
            }
        }
        text.push_str("\n## Coverage and limits\n\n");
        let _ = writeln!(
            text,
            "{} of {} changed files examined. Snapshot: {}. Tests were not executed by Ask.",
            self.examined_files.len(),
            self.snapshot.files.len(),
            self.snapshot.label
        );
        for file in &self.snapshot.files {
            if !self.examined_files.contains(&file.path) {
                let _ = writeln!(
                    text,
                    "- `{}`: {}.",
                    file.path.replace('`', "'"),
                    file.omitted
                        .as_deref()
                        .unwrap_or("Not examined for this answer")
                );
            }
        }
        for limit in &self.limitations {
            let _ = writeln!(text, "- {limit}");
        }
        text
    }
}
fn text_valid(text: &str, max: usize) -> bool {
    !text.trim().is_empty()
        && text.len() <= max
        && !text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
}
/// Return exact old/new line numbers carried by valid unified hunk bodies.
pub fn captured_lines(patch: &str, side: &Side) -> HashSet<u32> {
    let mut lines = HashSet::new();
    let mut old = 0u32;
    let mut new = 0u32;
    let mut old_left = 0u32;
    let mut new_left = 0u32;
    for line in patch.lines() {
        if line.starts_with("@@ ") {
            old_left = 0;
            new_left = 0;
            let mut words = line.split_whitespace();
            words.next();
            let parse = |word: Option<&str>, prefix: char| -> Option<(u32, u32)> {
                let word = word?.strip_prefix(prefix)?;
                let (start, count) = word.split_once(',').unwrap_or((word, "1"));
                Some((start.parse().ok()?, count.parse().ok()?))
            };
            if let (Some((o, oc)), Some((n, nc))) =
                (parse(words.next(), '-'), parse(words.next(), '+'))
            {
                old = o;
                new = n;
                old_left = oc;
                new_left = nc;
            }
            continue;
        }
        let (uses_old, uses_new) = match line.as_bytes().first() {
            Some(b' ') => (true, true),
            Some(b'-') => (true, false),
            Some(b'+') => (false, true),
            Some(b'\\') => continue,
            _ => {
                old_left = 0;
                new_left = 0;
                continue;
            }
        };
        if (uses_old && old_left == 0) || (uses_new && new_left == 0) {
            old_left = 0;
            new_left = 0;
            continue;
        }
        if uses_old {
            if *side == Side::Old {
                lines.insert(old);
            }
            old = old.saturating_add(1);
            old_left -= 1;
        }
        if uses_new {
            if *side == Side::New {
                lines.insert(new);
            }
            new = new.saturating_add(1);
            new_left -= 1;
        }
    }
    lines
}
fn validate(
    snapshot: Snapshot,
    question: String,
    value: serde_json::Value,
) -> Result<Answer, String> {
    let p: Prediction = serde_json::from_value(value)
        .map_err(|_| "Ask returned invalid structured output. Retry the question.".to_string())?;
    if !text_valid(&p.answer, 20000)
        || p.citations.len() > 20
        || p.limitations.len() > 12
        || p.limitations.iter().any(|s| !text_valid(s, 1000))
    {
        return Err("Ask returned an invalid answer or limitations. Retry the question.".into());
    }
    let complete: HashSet<_> = snapshot
        .files
        .iter()
        .filter(|f| f.omitted.is_none() && !f.patch.trim().is_empty())
        .map(|f| f.path.as_str())
        .collect();
    let mut seen = HashSet::new();
    if p.examined_files.len() > snapshot.files.len()
        || p.examined_files
            .iter()
            .any(|path| !complete.contains(path.as_str()) || !seen.insert(path))
    {
        return Err("Ask claimed an unknown, omitted or duplicate file was examined.".into());
    }
    for cite in &p.citations {
        let Some(file) = snapshot
            .files
            .iter()
            .find(|f| f.path == cite.path && complete.contains(f.path.as_str()))
        else {
            return Err("Ask cited an unavailable file. Retry the question.".into());
        };
        if !p.examined_files.contains(&cite.path)
            || cite.start == 0
            || cite.end < cite.start
            || cite.end - cite.start > 80
            || !text_valid(&cite.explanation, 1000)
        {
            return Err("Ask returned an invalid captured reference.".into());
        }
        let lines = captured_lines(&file.patch, &cite.side);
        if !(cite.start..=cite.end).all(|n| lines.contains(&n)) {
            return Err("Ask cited lines outside the captured patch. Retry the question.".into());
        }
    }
    Ok(Answer {
        snapshot,
        question,
        answer: p.answer,
        citations: p.citations,
        examined_files: p.examined_files,
        limitations: p.limitations,
    })
}
pub fn generate(
    path: &Path,
    scope: &Scope,
    question: &str,
    provider: Provider,
    control: &CommitControl,
) -> Result<Answer, String> {
    if !text_valid(question, 4000) {
        return Err("Enter a question of at most 4,000 bytes.".into());
    }
    let snapshot = change_description::prepare_with_control(path, scope, Some(control))?;
    generate_snapshot(snapshot, question, provider, control)
}
pub fn generate_snapshot(
    snapshot: Snapshot,
    question: &str,
    provider: Provider,
    control: &CommitControl,
) -> Result<Answer, String> {
    if !text_valid(question, 4000) {
        return Err("Enter a question of at most 4,000 bytes.".into());
    }
    if control.canceled() {
        return Err("Ask canceled".into());
    }
    let prompt = generation_prompt(
        &snapshot,
        question,
        "Return the required JSON object and nothing else.",
    )?;
    let value = commit_agent::generate_json(provider, &prompt, SCHEMA, control).map_err(|e| {
        e.replace("commit message", "change answer")
            .replace("commit model", "Ask model")
            .replace("Commit canceled", "Ask canceled")
            .replace("Commit assistant", "Ask assistant")
    })?;
    if control.canceled() {
        return Err("Ask canceled".into());
    }
    validate(snapshot, question.into(), value)
}

pub const RESULT_FENCE: &str = "grove-ask";
fn generation_prompt(snapshot: &Snapshot, question: &str, output: &str) -> Result<String, String> {
    if !text_valid(question, 4000) {
        return Err("Enter a question of at most 4,000 bytes.".into());
    }
    let available: Vec<_> = snapshot
        .files
        .iter()
        .filter(|f| f.omitted.is_none() && !f.patch.trim().is_empty())
        .collect();
    if available.is_empty() {
        return Err("No complete text patches are available in this scope. Choose another scope or inspect the diff.".into());
    }
    let data=serde_json::to_string(&serde_json::json!({"scope":snapshot.label,"previous_title":snapshot.title,"files":available})).map_err(|_|"Could not encode patches")?;
    let question = serde_json::to_string(question).map_err(|_| "Could not encode question")?;
    let context=format!("{output}\n\nRequired result schema: {SCHEMA}\nanswer is supported Markdown without invented file/line links; references belong in citations. Citations must name exact supplied complete-patch paths and contiguous source lines actually present in unified-diff hunks. side old identifies old/deleted lines; new identifies new/added lines. start/end are inclusive source line numbers, not patch indexes. examined_files lists only complete patches actually examined for this question. limitations discloses missing context and uncertainty; do not assert tests or runtime behavior were verified. Do not put unrelated findings in the answer.\n\n<user-question>\n{question}\n</user-question>\n\n<change-data>\n{data}\n</change-data>");
    Ok(crate::prompts::compose(
        crate::prompts::PromptKind::AskChanges,
        &context,
    ))
}
pub fn conversation_prompt(snapshot: &Snapshot, question: &str) -> Result<String, String> {
    generation_prompt(snapshot,question,"Give a short completion sentence, then exactly one closed ```grove-ask fenced block containing the required JSON result. Do not show the captured schema/diff again. Ordinary conversation follow-ups after this request are not new canonical answer generations.")
}
pub fn from_conversation(snapshot: Snapshot, question: &str, text: &str) -> Result<Answer, String> {
    if text.len() > 2 * 1024 * 1024 {
        return Err("Ask response exceeds the import limit".into());
    }
    let text = text.trim();
    let payload = if text.starts_with('{') {
        text.to_owned()
    } else {
        let mut result = None;
        let mut current = None::<String>;
        let mut other = false;
        for line in text.lines() {
            let marker = line.trim();
            if let Some(content) = &mut current {
                if marker == "```" {
                    result = current.take();
                } else if marker.starts_with("```") {
                    return Err("Ask response has an invalid result block".into());
                } else {
                    content.push_str(line);
                    content.push('\n');
                }
            } else if other {
                if marker == "```" {
                    other = false;
                }
            } else if marker == "```grove-ask" {
                if result.is_some() {
                    return Err("Ask response has multiple result blocks".into());
                }
                current = Some(String::new());
            } else if marker.starts_with("```grove-ask") {
                return Err("Ask response has an invalid result marker".into());
            } else if marker.starts_with("```") {
                other = true;
            }
        }
        if current.is_some() {
            return Err("Ask response has an incomplete result block".into());
        }
        result.ok_or("Ask response has no complete result block")?
    };
    validate(
        snapshot,
        question.into(),
        serde_json::from_str(&payload).map_err(|_| "Ask response has invalid JSON")?,
    )
}
impl crate::change_result_store::StoredResult for Answer {
    const DIRECTORY: &'static str = "change-answers";
    fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    fn input(&self) -> &str {
        &self.question
    }
    fn validate(&self) -> Result<(), String> {
        if !text_valid(&self.question, 4000) {
            return Err("Saved answer has an invalid question".into());
        }
        let value = serde_json::json!({"answer":self.answer,"citations":self.citations,"examined_files":self.examined_files,"limitations":self.limitations});
        validate(self.snapshot.clone(), self.question.clone(), value).map(|_| ())
    }
}
impl crate::conversation_result::Workflow for Answer {
    const NAME: &'static str = "ask";
    const LABEL: &'static str = "answer";
    const FENCE: &'static str = RESULT_FENCE;
    fn prompt(snapshot: &Snapshot, input: &str) -> Result<String, String> {
        conversation_prompt(snapshot, input)
    }
    fn parse(snapshot: Snapshot, input: &str, text: &str) -> Result<Self, String> {
        from_conversation(snapshot, input, text)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> Snapshot {
        Snapshot {
            scope: Scope::Local,
            fingerprint: "identity".into(),
            label: "local".into(),
            head: "head".into(),
            files: vec![
                change_description::SnapshotFile {
                    path: "src/a.rs".into(),
                    added: 1,
                    removed: 1,
                    patch: "@@ -10,2 +20,2 @@\n keep\n-old\n+new\n".into(),
                    omitted: None,
                },
                change_description::SnapshotFile {
                    path: "image.png".into(),
                    added: 0,
                    removed: 0,
                    patch: "".into(),
                    omitted: Some("Binary".into()),
                },
            ],
            original_body: None,
            title: "".into(),
            base: "".into(),
            checkout: "/fixture".into(),
        }
    }
    fn prediction() -> serde_json::Value {
        serde_json::json!({"answer":"The value changes from old to new.","citations":[{"path":"src/a.rs","side":"new","start":21,"end":21,"explanation":"New value."}],"examined_files":["src/a.rs"],"limitations":["Caller behavior is not present."]})
    }
    #[test]
    fn validates_captured_references_and_discloses_omissions() {
        let a = validate(snapshot(), "What changes?".into(), prediction()).unwrap();
        assert!(a.markdown().contains("1 of 2 changed files"));
        assert!(a.markdown().contains("Binary"));
        assert!(a.markdown().contains("Tests were not executed"));
    }
    #[test]
    fn rejects_invented_or_omitted_files_and_uncaptured_lines() {
        for (key, value) in [
            ("path", serde_json::json!("image.png")),
            ("path", serde_json::json!("../outside.rs")),
            ("start", serde_json::json!(22)),
            ("end", serde_json::json!(25)),
            ("side", serde_json::json!("unknown")),
        ] {
            let mut p = prediction();
            p["citations"][0][key] = value;
            assert!(validate(snapshot(), "?".into(), p).is_err());
        }
        let mut p = prediction();
        p["examined_files"] = serde_json::json!(["image.png"]);
        assert!(validate(snapshot(), "?".into(), p).is_err());
    }
    #[test]
    fn old_new_ranges_and_hunk_gaps_are_not_interchangeable() {
        let p="--- a/a\n+++ b/a\n@@ -10,2 +20,2 @@\n keep\n-old\n+new\n@@ -40 +50 @@\n-next\n+after\n";
        assert_eq!(captured_lines(p, &Side::Old), HashSet::from([10, 11, 40]));
        assert_eq!(captured_lines(p, &Side::New), HashSet::from([20, 21, 50]));
        assert!(captured_lines("@@ -1 +1 @@\n+a\n+overflow", &Side::New) == HashSet::from([1]));
    }
    #[test]
    fn accepts_deleted_source_lines_but_rejects_cross_hunk_ranges_and_invalid_schema() {
        let mut value = prediction();
        value["citations"][0]["side"] = serde_json::json!("old");
        value["citations"][0]["start"] = serde_json::json!(11);
        value["citations"][0]["end"] = serde_json::json!(11);
        assert!(validate(snapshot(), "What was removed?".into(), value).is_ok());
        let mut source = snapshot();
        source.files[0]
            .patch
            .push_str("@@ -40 +50 @@\n-next\n+after\n");
        let mut value = prediction();
        value["citations"][0]["end"] = serde_json::json!(50);
        assert!(validate(source, "?".into(), value).is_err());
        let mut value = prediction();
        value["answer"] = serde_json::json!("");
        assert!(validate(snapshot(), "?".into(), value).is_err());
        let mut value = prediction();
        value["unexpected"] = serde_json::json!(true);
        assert!(validate(snapshot(), "?".into(), value).is_err());
    }
    #[test]
    fn refuses_empty_question_and_pre_canceled_generation_without_provider_calls() {
        let c = CommitControl::default();
        c.cancel();
        assert!(generate_snapshot(snapshot(), "Why?", Provider::Codex, &c)
            .unwrap_err()
            .contains("canceled"));
        assert!(
            generate_snapshot(snapshot(), "", Provider::Claude, &CommitControl::default()).is_err()
        );
    }
}
