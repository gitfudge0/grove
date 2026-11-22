//! Validated plan snapshots and imports from completed assistant Markdown.

use super::{
    assets::{asset, decoded_budget, read_bounded, BoundedReadError},
    plan_link_path, AssetSnapshot, Claim, Decision, Exhibit, ImportError, LayoutNode, PlanDocument,
    PlanReference, PlanRevision, PlanStore, ReviewState,
};
use crate::provider::SavedSession;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

impl PlanStore {
    fn remember_reference(
        &mut self,
        message_id: &str,
        source: &str,
        plan_id: &str,
        revision_id: &str,
    ) {
        self.references
            .retain(|r| r.message_id != message_id || r.source != source);
        self.references.push(PlanReference {
            message_id: message_id.into(),
            source: source.into(),
            plan_id: plan_id.into(),
            revision_id: revision_id.into(),
        });
    }
    pub fn reference(&self, message_id: &str, source: &str) -> Option<&PlanReference> {
        self.references
            .iter()
            .find(|r| r.message_id == message_id && r.source == source)
    }
    pub fn revision(&self, plan: &str, revision: &str) -> Option<&PlanRevision> {
        self.revisions
            .iter()
            .find(|r| r.document.plan_id == plan && r.document.revision_id == revision)
    }
    pub fn revision_mut(&mut self, plan: &str, revision: &str) -> Option<&mut PlanRevision> {
        self.revisions
            .iter_mut()
            .find(|r| r.document.plan_id == plan && r.document.revision_id == revision)
    }
    pub fn import(
        &mut self,
        message_id: &str,
        source: &str,
        cwd: &str,
        roots: &[PathBuf],
    ) -> Result<(), String> {
        let result: Result<(), BoundedReadError> = (|| {
            if self.references.len() >= 1000
                && !self
                    .references
                    .iter()
                    .any(|r| r.message_id == message_id && r.source == source)
            {
                return Err("Session plan reference limit reached".into());
            }
            let path = plan_link_path(Path::new(cwd), source, roots)?;
            if !source.ends_with(".grove-plan.json") {
                return Err("Expected .grove-plan.json".into());
            }
            let bytes = read_bounded(&path, 1024 * 1024)?;
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            validate_strings(&value, 0)?;
            let doc: PlanDocument = serde_json::from_value(value).map_err(|e| e.to_string())?;
            if doc.claims.is_empty() && doc.markdown.as_deref().is_none_or(|s| s.trim().is_empty())
            {
                return Err("Plan needs Markdown content or at least one behavior claim".into());
            }
            if let Some(markdown) = &doc.markdown {
                if markdown.trim().is_empty() || !doc.claims.is_empty() {
                    return Err("Use a nonblank Markdown body or behavior claims, not both".into());
                }
            }
            if let Some(presentation) = &doc.presentation {
                presentation.validate()?;
            }
            if doc.schema_version != 1 {
                return Err("Unsupported plan schema version".into());
            }
            if doc.plan_id.is_empty() || doc.revision_id.is_empty() || doc.title.is_empty() {
                return Err("Plan identity and title are required".into());
            }
            let mut ids = HashSet::new();
            let mut count = 0;
            let mut assets = Vec::new();
            validate_claims(
                &doc.claims,
                0,
                &mut count,
                &mut ids,
                &mut assets,
                path.parent().ok_or("Plan has no parent directory")?,
                roots,
            )?;
            for decision in &doc.decisions {
                validate_decision(decision, &mut ids)?;
            }
            if doc.all_decisions().len() > 200 {
                return Err("Decision count exceeded".into());
            }
            decoded_budget(&assets)?;
            if assets
                .iter()
                .map(|a: &AssetSnapshot| a.bytes.len())
                .sum::<usize>()
                > 20 * 1024 * 1024
            {
                return Err("Plan asset budget exceeded".into());
            }
            if let Some(old) = self.revision(&doc.plan_id, &doc.revision_id) {
                if old.document != doc || old.assets != assets {
                    return Err("Conflicting immutable plan revision".into());
                }
                self.remember_reference(message_id, source, &doc.plan_id, &doc.revision_id);
                return Ok(());
            }
            if self.revisions.len() >= 100 {
                return Err("Session plan revision limit reached".into());
            }
            self.validate_finalization(&doc)?;
            let mut review = ReviewState::default();
            if let Some(previous) = self
                .revisions
                .iter()
                .rev()
                .find(|r| r.document.plan_id == doc.plan_id)
            {
                let previous = previous
                    .document
                    .finalized_from
                    .as_ref()
                    .and_then(|from| self.revision(&doc.plan_id, &from.revision_id))
                    .unwrap_or(previous);
                for decision in doc.all_decisions() {
                    if previous
                        .document
                        .all_decisions()
                        .iter()
                        .any(|old| **old == *decision)
                    {
                        if let Some(value) = previous.review.answer_drafts.get(&decision.id) {
                            review
                                .answer_drafts
                                .insert(decision.id.clone(), value.clone());
                        }
                        if let Some(value) = previous.review.answers.get(&decision.id) {
                            review.answers.insert(decision.id.clone(), value.clone());
                        }
                        if let Some(value) = previous.review.own_answers.get(&decision.id) {
                            review
                                .own_answers
                                .insert(decision.id.clone(), value.clone());
                        }
                    }
                }
            }
            self.remember_reference(message_id, source, &doc.plan_id, &doc.revision_id);
            self.revisions.push(PlanRevision {
                approval_digest: None,
                message_id: message_id.into(),
                source: source.into(),
                document: doc,
                assets,
                review,
            });
            Ok(())
        })();
        self.errors
            .retain(|e| e.message_id != message_id || e.source != source);
        if let Err(error) = &result {
            self.errors.push(ImportError {
                message_id: message_id.into(),
                source: source.into(),
                error: error.message.clone(),
                size_limit: error.size_limit.clone(),
            });
            self.errors.truncate(100);
        }
        result.map_err(String::from)
    }
}
pub(super) fn validate_strings(value: &serde_json::Value, depth: usize) -> Result<(), String> {
    if depth > 48 {
        return Err("Plan nesting limit exceeded".into());
    }
    match value {
        serde_json::Value::String(s) if s.len() > 65536 => Err("Plan text exceeds 64 KiB".into()),
        serde_json::Value::Array(a) => {
            if a.len() > 1000 {
                return Err("Plan array limit exceeded".into());
            }
            for v in a {
                validate_strings(v, depth + 1)?;
            }
            Ok(())
        }
        serde_json::Value::Object(o) => {
            if o.len() > 1000 {
                return Err("Plan object limit exceeded".into());
            }
            for v in o.values() {
                validate_strings(v, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
fn unique(ids: &mut HashSet<String>, id: &str) -> Result<(), String> {
    if id.is_empty() || !ids.insert(id.to_owned()) {
        return Err("Plan target IDs must be nonempty and unique".into());
    }
    Ok(())
}
fn validate_decision(d: &Decision, ids: &mut HashSet<String>) -> Result<(), String> {
    unique(ids, &d.id)?;
    if d.prompt.trim().is_empty() {
        return Err("Decision prompt is required".into());
    }
    let mut options = HashSet::new();
    for option in &d.options {
        unique(&mut options, &option.id)?;
        if option.label.trim().is_empty() {
            return Err("Decision option label is required".into());
        }
    }
    if d.default.as_ref().is_some_and(|v| !options.contains(v)) {
        return Err("Unknown default decision option".into());
    }
    Ok(())
}
fn validate_claims(
    claims: &[Claim],
    depth: usize,
    count: &mut usize,
    ids: &mut HashSet<String>,
    assets: &mut Vec<AssetSnapshot>,
    base: &Path,
    roots: &[PathBuf],
) -> Result<(), BoundedReadError> {
    if depth >= 3 && !claims.is_empty() {
        return Err("Claim depth exceeded".into());
    }
    if claims.len() > 5 {
        return Err("At most five claims per level".into());
    }
    for c in claims {
        *count += 1;
        if *count > 200 {
            return Err("Claim count exceeded".into());
        }
        unique(ids, &c.id)?;
        for d in &c.decisions {
            validate_decision(d, ids)?;
        }
        if let Some(e) = &c.exhibit {
            validate_exhibit(e, 0, ids, assets, base, roots)?;
        }
        for l in &c.html_links {
            asset(&l.path, true, assets, base, roots)?;
        }
        validate_claims(&c.children, depth + 1, count, ids, assets, base, roots)?;
    }
    Ok(())
}
fn validate_exhibit(
    e: &Exhibit,
    depth: usize,
    ids: &mut HashSet<String>,
    assets: &mut Vec<AssetSnapshot>,
    base: &Path,
    roots: &[PathBuf],
) -> Result<(), BoundedReadError> {
    if depth > 12 {
        return Err("Exhibit depth exceeded".into());
    }
    match e {
        Exhibit::Image { path, .. } => asset(path, false, assets, base, roots)?,
        Exhibit::HtmlLink { path, .. } => asset(path, true, assets, base, roots)?,
        Exhibit::Calls { entries } => {
            for r in entries {
                unique(ids, &r.id)?;
            }
        }
        Exhibit::Layout { nodes } => validate_layout(nodes, depth, ids)?,
        Exhibit::StateMachine {
            states,
            initial,
            transitions,
        } => {
            let mut state_ids = HashSet::new();
            for s in states {
                unique(&mut state_ids, &s.id)?;
                unique(ids, &s.id)?;
                if let Some(screen) = &s.screen {
                    validate_exhibit(screen, depth + 1, ids, assets, base, roots)?;
                }
            }
            if !state_ids.contains(initial)
                || transitions
                    .iter()
                    .any(|t| !state_ids.contains(&t.from) || !state_ids.contains(&t.to))
            {
                return Err("Unknown state reference".into());
            }
        }
        _ => {}
    }
    Ok(())
}
fn validate_layout(
    nodes: &[LayoutNode],
    depth: usize,
    ids: &mut HashSet<String>,
) -> Result<(), String> {
    if depth > 12 {
        return Err("Layout depth exceeded".into());
    }
    for n in nodes {
        unique(ids, &n.id)?;
        if !["row", "column", "text", "surface", "control"].contains(&n.kind.as_str()) {
            return Err("Unsupported layout kind".into());
        }
        validate_layout(&n.children, depth + 1, ids)?;
    }
    Ok(())
}
/// Decode a Markdown plan destination exactly once. Asset paths are raw paths
/// and must continue to use `safe_path` directly. Keep the original destination
/// in stored references so retry and immutable snapshot identity remain stable.
/// Completed assistant messages only; never interpret tools, user messages or JSON fences.
pub fn import_completed(saved: &mut SavedSession) {
    let mut links = Vec::new();
    for item in &mut saved.items {
        if item["grovePlanImported"] == true {
            continue;
        }
        if !["agentMessage", "assistant"].contains(&item["type"].as_str().unwrap_or("")) {
            continue;
        }
        if item["status"].as_str().is_some_and(|s| s != "completed") {
            continue;
        }
        let id = item["id"].as_str().unwrap_or("").to_owned();
        if id.is_empty() {
            continue;
        }
        let text = item["text"].as_str().unwrap_or("");
        for source in markdown_links(text).into_iter().take(20) {
            if !saved
                .rich_plans
                .revisions
                .iter()
                .any(|r| r.message_id == id && r.source == source)
                && !saved
                    .rich_plans
                    .errors
                    .iter()
                    .any(|e| e.message_id == id && e.source == source)
            {
                links.push((id.clone(), source));
            }
        }
        item["grovePlanImported"] = serde_json::Value::Bool(true);
    }
    let mut roots: Vec<PathBuf> = saved
        .roots
        .iter()
        .map(|r| PathBuf::from(&r.wt_path))
        .collect();
    roots.push(PathBuf::from(&saved.cwd));
    // Older saved errors carried only this exact diagnostic. Re-read through
    // the guarded importer once to establish typed details instead of guessing
    // whether the oversized target was JSON or a linked asset.
    let legacy: Vec<_> = saved
        .rich_plans
        .errors
        .iter()
        .filter(|error| error.size_limit.is_none() && error.error == "Plan file exceeds size limit")
        .map(|error| (error.message_id.clone(), error.source.clone()))
        .collect();
    links.extend(legacy);
    links.sort();
    links.dedup();
    for (id, source) in links {
        let _ = saved.rich_plans.import(&id, &source, &saved.cwd, &roots);
    }
}
pub(super) fn markdown_links(text: &str) -> Vec<String> {
    use markdown::mdast::Node;
    if !text.contains(".grove-plan.json") {
        return Vec::new();
    }
    let Ok(root) = markdown::to_mdast(text, &markdown::ParseOptions::default()) else {
        return Vec::new();
    };
    // The same Markdown AST rules used by the transcript distinguish real links
    // from fenced/indented code, inline code, escaped examples and raw HTML.
    let mut definitions = HashMap::new();
    let mut stack = vec![&root];
    while let Some(node) = stack.pop() {
        if let Node::Definition(definition) = node {
            definitions
                .entry(&definition.identifier)
                .or_insert(&definition.url);
        }
        if let Some(children) = node.children() {
            stack.extend(children.iter().rev());
        }
    }
    let mut links = Vec::new();
    stack.push(&root);
    while let Some(node) = stack.pop() {
        let target = match node {
            Node::Link(link) => Some(&link.url),
            Node::LinkReference(link) => definitions.get(&link.identifier).copied(),
            _ => None,
        };
        if let Some(target) = target.filter(|target| target.ends_with(".grove-plan.json")) {
            links.push(target.clone());
            if links.len() == 20 {
                break;
            }
        }
        if let Some(children) = node.children() {
            stack.extend(children.iter().rev());
        }
    }
    links
}

pub fn retry_import(saved: &mut SavedSession, message: &str, source: &str) -> Result<(), String> {
    if !saved
        .rich_plans
        .errors
        .iter()
        .any(|e| e.message_id == message && e.source == source)
    {
        return Err("No failed plan import matches this message".into());
    }
    let mut roots: Vec<PathBuf> = saved
        .roots
        .iter()
        .map(|r| PathBuf::from(&r.wt_path))
        .collect();
    roots.push(PathBuf::from(&saved.cwd));
    saved.rich_plans.import(message, source, &saved.cwd, &roots)
}
