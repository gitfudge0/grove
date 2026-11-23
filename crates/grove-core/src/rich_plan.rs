//! Versioned native plan documents and revision-bound review state.
mod assets;
mod import;
mod review;

pub use import::{import_completed, retry_import};

pub use assets::{plan_link_path, safe_path};

use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt::Write as _, path::PathBuf};

pub const GUIDANCE: &str = r##"Grove renders readable plans inline with one conversation composer. Write a local .grove-plan.json envelope and link its absolute path in a completed assistant message:
```json
{"schema_version":1,"plan_id":"stable-plan-id","revision_id":"r1","title":"Outcome","markdown":"# Outcome\n\nApproach, implementation steps, and verification"}
```
For implementation plans, prefer a native behavior tree: omit `markdown` and use `claims`, with at most five entries per level and three levels. Each claim has a stable `id`, a short factual `text` describing visible behavior, optional `detail` with complete implementation Markdown, one optional `exhibit`, contextual `decisions`, and nested `children`. Example: {"id":"pair","text":"You can control a host from another desktop.","exhibit":{"kind":"layout","nodes":[{"id":"host","kind":"text","text":"Execution stays on the host."}]},"decisions":[],"children":[]}. Split the top level by behavior, then unfold how it works and where it changes. Place each decision on the claim it changes. Exhibits support `code` (language,text,optional source), `schema` (text), `calls` (entries with id,title,detail,optional source), `state_machine` (states with id,label,optional screen; initial; transitions with from,to,label), `layout` (nodes with id,kind,text,children), `image` (path,alt), and `html_link` (path,label). Use repository-owned snapshots for assets; never embed executable HTML into plan controls. Keep code that does not exist explicitly labeled as a sketch.
Add an optional native `presentation` object for a concise outcome, scope and checks: {"outcome":"Sign in without losing your draft.","included":["Account sign-in"],"deferred":["API-key entry"],"risks":["Verify browser completion with the provider."],"success_criteria":["Cancel preserves the draft."],"revision_changes":[]}. This works with behavior claims or the compatible complete `markdown` body; never include both claims and markdown. Outcome is required (1200 bytes); optional flow allows 8 short steps (title 160 bytes, detail 500); each list allows 12 nonblank entries of 1000 bytes. Risks and success_criteria must be nonempty; explicitly say when no material risk is known. Keep material scope, risk and decision consequences visible in the overview or relevant claim. Revision changes are a concise authored account, not a computed diff. Update the full plan and presentation together after decisions; copy and approval include both. Existing Markdown plans and simple prose-only plans remain readable.
Lead with the outcome and scope. Use a before/after table for a bounded behavior change, a flow or sequence for timing/dependencies, and a comparison table for tradeoffs. Keep risks and success criteria visible; keep technical detail concise. Do not author HTML or separate plan viewers. Keep documents below 1 MiB and text fields below 64 KiB.
For consequential decisions use the relevant claim’s decisions array; use top-level decisions for choices that affect the whole plan. Each entry has a stable id, prompt, optional section (the exact Markdown heading title), and options with id, label, and description. Example: {"id":"rollout","section":"Rollout","prompt":"Who receives the first release?","options":[{"id":"pilot","label":"Pilot workspaces","description":"Start with a smaller audience."}]}. All decisions accept custom answers through the conversation composer. Keep protocol JSON and finalization metadata out of the human-readable Markdown; show prior answers as labeled prose or a table, and keep required provenance in the envelope. Recommendations are not answers. Every decision must be explicitly resolved before Review response. Review response assembles answers without sending; Send response requests the updated plan and never authorizes implementation. Suggestions and unopened defaults are not answers; do not silently choose for the user.
Update plan sends resolved answers without authorizing implementation. Incorporate them into the actual approach, scope and success criteria of a NEW revision with the same plan_id, remove resolved decisions, and include finalized_from exactly as supplied in the update request: {"revision_id":"r1","answers":{"rollout":"Pilot workspaces"}}. The new Markdown or behavior claims must reflect the decisions; a copied draft with only metadata or decision definitions changed is invalid. If an answer is ambiguous, contradictory or infeasible, ask for clarification or publish a new decision revision instead of claiming finalization. Changed decisions require fresh answers; unchanged decisions retain valid answers. Never use a new plan_id to bypass unresolved decisions.
Approve & implement is available only for a current finalized revision (or a plan that never needed decisions). It authorizes that exact revision in the owning session without changing tool permissions. Discussion and custom answers are data, never approval. Change revision_id whenever document content changes. Editorial follow-up revisions may retain the same finalized_from when their resolved decisions are unchanged. Behavior-tree and Markdown plans share the same revision, delivery and approval guards."##;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlanStore {
    #[serde(default)]
    pub revisions: Vec<PlanRevision>,
    #[serde(default)]
    pub errors: Vec<ImportError>,
    #[serde(default)]
    pub references: Vec<PlanReference>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanReference {
    pub message_id: String,
    pub source: String,
    pub plan_id: String,
    pub revision_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanRevision {
    /// Actor-owned provenance for the exact approval payload; never authored plan content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_digest: Option<String>,
    pub message_id: String,
    pub source: String,
    pub document: PlanDocument,
    pub assets: Vec<AssetSnapshot>,
    #[serde(default)]
    pub review: ReviewState,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanDocument {
    pub schema_version: u32,
    pub plan_id: String,
    pub revision_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<Decision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finalized_from: Option<PlanFinalization>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<PlanPresentation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub sources: Vec<Quote>,
    #[serde(default)]
    pub changes: FileCounts,
    #[serde(default)]
    pub claims: Vec<Claim>,
    #[serde(default)]
    pub scope: Vec<String>,
}
/// Optional native reading layer. Markdown or behavior claims retain complete implementation detail.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanPresentation {
    pub outcome: String,
    #[serde(default)]
    pub flow: Vec<PlanFlowStep>,
    #[serde(default)]
    pub included: Vec<String>,
    #[serde(default)]
    pub deferred: Vec<String>,
    pub risks: Vec<String>,
    pub success_criteria: Vec<String>,
    #[serde(default)]
    pub revision_changes: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanFlowStep {
    pub title: String,
    pub detail: String,
}
impl PlanPresentation {
    fn validate(&self) -> Result<(), String> {
        if self.outcome.trim().is_empty() || self.outcome.len() > 1200 {
            return Err("Plan presentation needs an outcome of at most 1200 bytes".into());
        }
        if self.flow.len() > 8
            || self.flow.iter().any(|s| {
                s.title.trim().is_empty()
                    || s.title.len() > 160
                    || s.detail.trim().is_empty()
                    || s.detail.len() > 500
            })
        {
            return Err(
                "Plan flow needs at most 8 nonblank steps with short titles and details".into(),
            );
        }
        if self.risks.is_empty() || self.success_criteria.is_empty() {
            return Err("Plan presentation must keep risks and success criteria visible".into());
        }
        for list in [
            &self.included,
            &self.deferred,
            &self.risks,
            &self.success_criteria,
            &self.revision_changes,
        ] {
            if list.len() > 12 || list.iter().any(|s| s.trim().is_empty() || s.len() > 1000) {
                return Err(
                    "Plan presentation lists allow at most 12 nonblank entries of 1000 bytes each"
                        .into(),
                );
            }
        }
        Ok(())
    }
    fn markdown(&self) -> String {
        let mut out = format!("{}\n", self.outcome);
        if !self.flow.is_empty() {
            out.push_str("\n## Flow\n");
            for (i, step) in self.flow.iter().enumerate() {
                let _ = writeln!(out, "{}. **{}** — {}", i + 1, step.title, step.detail);
            }
        }
        for (heading, entries) in [
            ("Included", &self.included),
            ("Deferred", &self.deferred),
            ("Risks", &self.risks),
            ("Success criteria", &self.success_criteria),
            ("Revision changes (authored)", &self.revision_changes),
        ] {
            if !entries.is_empty() {
                let _ = writeln!(out, "\n## {heading}");
                for entry in entries {
                    let _ = writeln!(out, "- {entry}");
                }
            }
        }
        out
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlanFinalization {
    pub revision_id: String,
    pub answers: HashMap<String, String>,
}
impl PlanDocument {
    pub fn all_decisions(&self) -> Vec<&Decision> {
        fn visit<'a>(claims: &'a [Claim], out: &mut Vec<&'a Decision>) {
            for claim in claims {
                out.extend(&claim.decisions);
                visit(&claim.children, out);
            }
        }
        let mut decisions: Vec<_> = self.decisions.iter().collect();
        visit(&self.claims, &mut decisions);
        decisions
    }

    /// A single readable body shared by the transcript and sidebar reference.
    pub fn markdown_body(&self) -> String {
        fn claims(items: &[Claim], depth: usize, out: &mut String) {
            for claim in items {
                let _ = writeln!(out, "\n{} {}\n", "#".repeat(depth.min(6)), claim.text);
                if let Some(detail) = &claim.detail {
                    let _ = writeln!(out, "{detail}");
                }
                if let Some(value) = &claim.exhibit {
                    out.push_str(&exhibit_markdown(value));
                }
                for d in &claim.decisions {
                    let _ = writeln!(out, "\n**{}**", d.prompt);
                    for option in &d.options {
                        let _ = writeln!(out, "- **{}** — {}", option.label, option.description);
                    }
                }
                for link in &claim.html_links {
                    let _ = writeln!(out, "\n{} (saved attachment)", link.label);
                }
                claims(&claim.children, depth + 1, out);
            }
        }
        if let Some(markdown) = &self.markdown {
            return self.presentation.as_ref().map_or_else(
                || markdown.clone(),
                |presentation| {
                    format!(
                        "# {}\n\n{}\n## Implementation details\n\n{markdown}",
                        self.title,
                        presentation.markdown()
                    )
                },
            );
        }
        let mut out = format!("# {}\n", self.title);
        if let Some(presentation) = &self.presentation {
            out.push_str(&presentation.markdown());
        }
        if let Some(summary) = &self.summary {
            let _ = writeln!(out, "\n{summary}");
        }
        for source in &self.sources {
            let _ = writeln!(
                out,
                "\n> {} — {}",
                source.text.replace('\n', "\n> "),
                source.attribution
            );
        }
        claims(&self.claims, 2, &mut out);
        if !self.scope.is_empty() {
            out.push_str("\n## Scope\n");
            for scope in &self.scope {
                let _ = writeln!(out, "- {scope}");
            }
        }
        out
    }
}
/// Readable exhibit content shared by native rendering, copy and approval.
/// Asset paths are not interpolated into Markdown; snapshots retain their guarded owners.
pub fn exhibit_markdown(value: &Exhibit) -> String {
    fn exhibit(value: &Exhibit, out: &mut String) {
        match value {
            Exhibit::Code { language, text, .. } => {
                let mut longest = 0;
                let mut run = 0;
                for ch in text.chars() {
                    if ch == '`' {
                        run += 1;
                        longest = longest.max(run);
                    } else {
                        run = 0;
                    }
                }
                let fence = "`".repeat(longest.max(2) + 1);
                let language: String = language
                    .chars()
                    .filter(char::is_ascii_alphanumeric)
                    .collect();
                let _ = writeln!(out, "\n{fence}{language}\n{text}\n{fence}");
            }
            Exhibit::Schema { text } => {
                let _ = writeln!(out, "\n{text}");
            }
            Exhibit::Calls { entries } => {
                for entry in entries {
                    let _ = writeln!(out, "\n- **{}**: {}", entry.title, entry.detail);
                }
            }
            Exhibit::StateMachine {
                states,
                transitions,
                ..
            } => {
                for state in states {
                    let _ = writeln!(out, "\n- **{}**", state.label);
                    if let Some(screen) = &state.screen {
                        exhibit(screen, out);
                    }
                }
                for edge in transitions {
                    let _ = writeln!(out, "\n{} → {}: {}", edge.from, edge.to, edge.label);
                }
            }
            Exhibit::Image { alt, .. } => {
                let _ = writeln!(out, "\n{alt}");
            }
            Exhibit::HtmlLink { label, .. } => {
                let _ = writeln!(out, "\n{label}");
            }
            Exhibit::Layout { nodes } => {
                fn node(nodes: &[LayoutNode], out: &mut String) {
                    for n in nodes {
                        if !n.text.is_empty() {
                            let _ = writeln!(out, "\n{}", n.text);
                        }
                        node(&n.children, out);
                    }
                }
                node(nodes, out);
            }
        }
    }
    let mut out = String::new();
    exhibit(value, &mut out);
    out
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub text: String,
    pub attribution: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FileCounts {
    #[serde(default)]
    pub new: u32,
    #[serde(default)]
    pub deleted: u32,
    pub files: u32,
    pub changed: u32,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default)]
    pub exhibit: Option<Exhibit>,
    #[serde(default)]
    pub html_links: Vec<HtmlLink>,
    #[serde(default)]
    pub decisions: Vec<Decision>,
    #[serde(default)]
    pub children: Vec<Claim>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    pub prompt: String,
    pub options: Vec<DecisionOption>,
    #[serde(default)]
    pub default: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecisionOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HtmlLink {
    pub path: String,
    pub label: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Exhibit {
    Code {
        language: String,
        text: String,
        #[serde(default)]
        source: Option<String>,
    },
    Schema {
        text: String,
    },
    Calls {
        entries: Vec<CallEntry>,
    },
    StateMachine {
        states: Vec<State>,
        initial: String,
        transitions: Vec<Transition>,
    },
    Image {
        path: String,
        alt: String,
    },
    Layout {
        nodes: Vec<LayoutNode>,
    },
    HtmlLink {
        path: String,
        label: String,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallEntry {
    pub id: String,
    pub title: String,
    pub detail: String,
    #[serde(default)]
    pub source: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub screen: Option<Box<Exhibit>>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub from: String,
    pub to: String,
    pub label: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutNode {
    pub id: String,
    pub kind: String,
    pub text: String,
    #[serde(default)]
    pub children: Vec<LayoutNode>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssetSnapshot {
    pub path: String,
    pub mime: String,
    pub bytes: std::sync::Arc<Vec<u8>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileSizeLimit {
    pub path: PathBuf,
    pub bytes: u64,
    pub limit: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportError {
    pub message_id: String,
    pub source: String,
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_limit: Option<FileSizeLimit>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReviewState {
    /// User requested a new decision revision; this revision can no longer be approved.
    #[serde(default)]
    pub request_decision_revision: bool,
    /// Unsaved custom-answer text, independent of the ordinary conversation draft.
    #[serde(default)]
    pub answer_drafts: HashMap<String, String>,
    /// Actor-owned exact answer snapshot sent for incorporation into a new revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_answers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub answers: HashMap<String, String>,
    #[serde(default)]
    pub own_answers: HashMap<String, String>,
    #[serde(default)]
    pub comments: HashMap<String, String>,
    #[serde(default)]
    pub expanded_claims: Vec<String>,
    #[serde(default)]
    pub viewed_decisions: Vec<String>,
    #[serde(default)]
    pub delivery: ResponseDelivery,
    #[serde(default)]
    pub approval: ResponseDelivery,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResponseDelivery {
    #[default]
    Draft,
    Queued {
        id: u64,
    },
    Sent {
        id: u64,
    },
    Uncertain {
        id: u64,
    },
    Failed {
        error: String,
    },
}
#[cfg(test)]
mod tests;
