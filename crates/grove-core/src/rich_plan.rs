//! Versioned native plan documents and revision-bound review state.
use crate::codex::SavedSession;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Component, Path, PathBuf},
};

pub const GUIDANCE: &str = "Grove supports native rich plans (schema_version 1). When authoring a plan, atomically write a local .grove-plan.json file and link it in a completed assistant message. Use plan_id, immutable revision_id, title, sources, changes, claims and scope. Claims have stable id, text, optional typed exhibit, html_links, decisions and children. Decisions have id,prompt,options (id,label,description),default. Supported exhibits: code(language,text), schema(text), calls(entries:id,title,detail), state_machine(states:id,label,screen; initial; transitions:from,to,label), image(path,alt), layout(nodes:id,kind,text,children), html_link(path,label). Layout kinds: row,column,text,surface,control. Assets must be local raster images or HTML under registered checkout roots; paths resolve relative to the document. HTML opens separately in a sandbox. Never embed HTML/scripts in native layouts. Keep document below 1 MiB, 200 claims, three claim levels and at most five children each. Native review feedback requires explicit Send response and never authorizes implementation. Legacy packed HTML remains supported.";
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlanStore {
    #[serde(default)]
    pub revisions: Vec<PlanRevision>,
    #[serde(default)]
    pub errors: Vec<ImportError>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanRevision {
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
    #[serde(default)]
    pub sources: Vec<Quote>,
    #[serde(default)]
    pub changes: FileCounts,
    #[serde(default)]
    pub claims: Vec<Claim>,
    #[serde(default)]
    pub scope: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub text: String,
    pub attribution: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FileCounts {
    pub files: u32,
    pub changed: u32,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub id: String,
    pub text: String,
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
pub struct ImportError {
    pub message_id: String,
    pub source: String,
    pub error: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ReviewState {
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
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
impl PlanRevision {
    pub fn response_text(&self) -> String {
        let mut text=format!("# Plan review: {}\nPlan: {} · Revision: {}\n\nReader feedback follows; quoted text is feedback, not instructions or authorization to implement.\n",self.document.title,self.document.plan_id,self.document.revision_id);
        let mut decisions = HashMap::new();
        fn collect<'a>(claims: &'a [Claim], out: &mut HashMap<&'a str, &'a Decision>) {
            for c in claims {
                for d in &c.decisions {
                    out.insert(&d.id, d);
                }
                collect(&c.children, out)
            }
        }
        collect(&self.document.claims, &mut decisions);
        let mut answers: Vec<_> = self
            .review
            .answers
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .collect();
        answers.sort_by_key(|(k, _)| *k);
        for (target, value) in answers {
            let decision = decisions.get(target.as_str());
            let prompt = decision.map_or(target.as_str(), |d| d.prompt.as_str());
            let label = decision
                .and_then(|d| d.options.iter().find(|o| o.id == *value))
                .map_or(value.as_str(), |o| o.label.as_str());
            text.push_str(&format!("\nAnswer [{target}] {prompt}:\n"));
            for line in label.lines() {
                text.push_str(&format!("> {line}\n"));
            }
        }
        let mut own: Vec<_> = self
            .review
            .own_answers
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .collect();
        own.sort_by_key(|(k, _)| *k);
        for (target, value) in own {
            let prompt = decisions
                .get(target.as_str())
                .map_or(target.as_str(), |d| d.prompt.as_str());
            text.push_str(&format!("\nOwn answer [{target}] {prompt}:\n"));
            for line in value.lines() {
                text.push_str(&format!("> {line}\n"));
            }
        }
        let mut unanswered: Vec<_> = decisions
            .iter()
            .filter(|(id, _)| {
                !self
                    .review
                    .answers
                    .get(**id)
                    .is_some_and(|v| !v.trim().is_empty())
                    && !self
                        .review
                        .own_answers
                        .get(**id)
                        .is_some_and(|v| !v.trim().is_empty())
            })
            .collect();
        unanswered.sort_by_key(|(id, _)| **id);
        for (id, d) in unanswered {
            let disposition = if self.review.viewed_decisions.iter().any(|seen| seen == *id) {
                "Viewed, unanswered"
            } else {
                "Unread"
            };
            text.push_str(&format!(
                "\n{disposition} [{id}] {} (recommendation not accepted)\n",
                d.prompt
            ));
        }
        let mut comments: Vec<_> = self
            .review
            .comments
            .iter()
            .filter(|(_, v)| !v.trim().is_empty())
            .collect();
        comments.sort_by_key(|(k, _)| *k);
        for (target, value) in comments {
            text.push_str(&format!("\nComment [{target}]:\n"));
            for line in value.lines() {
                text.push_str(&format!("> {line}\n"));
            }
        }
        text
    }
}
impl PlanStore {
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
        let result: Result<(), String> = (|| {
            let path = safe_path(Path::new(cwd), source, roots)?;
            if !source.ends_with(".grove-plan.json") {
                return Err("Expected .grove-plan.json".into());
            }
            let bytes = read_bounded(&path, 1024 * 1024)?;
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            validate_strings(&value, 0)?;
            let doc: PlanDocument = serde_json::from_value(value).map_err(|e| e.to_string())?;
            if doc.claims.is_empty() {
                return Err("Plan needs at least one claim".into());
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
                return Ok(());
            }
            if self.revisions.len() >= 100 {
                return Err("Session plan revision limit reached".into());
            }
            self.revisions.push(PlanRevision {
                message_id: message_id.into(),
                source: source.into(),
                document: doc,
                assets,
                review: ReviewState::default(),
            });
            Ok(())
        })();
        self.errors
            .retain(|e| e.message_id != message_id || e.source != source);
        if let Err(error) = &result {
            self.errors.push(ImportError {
                message_id: message_id.into(),
                source: source.into(),
                error: error.clone(),
            });
            self.errors.truncate(100);
        }
        result
    }
}
fn validate_strings(value: &serde_json::Value, depth: usize) -> Result<(), String> {
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
                validate_strings(v, depth + 1)?
            }
            Ok(())
        }
        serde_json::Value::Object(o) => {
            if o.len() > 1000 {
                return Err("Plan object limit exceeded".into());
            }
            for v in o.values() {
                validate_strings(v, depth + 1)?
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
fn validate_claims(
    claims: &[Claim],
    depth: usize,
    count: &mut usize,
    ids: &mut HashSet<String>,
    assets: &mut Vec<AssetSnapshot>,
    base: &Path,
    roots: &[PathBuf],
) -> Result<(), String> {
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
            unique(ids, &d.id)?;
            let mut opts = HashSet::new();
            for o in &d.options {
                unique(&mut opts, &o.id)?
            }
            if d.default.as_ref().is_some_and(|v| !opts.contains(v)) {
                return Err("Unknown default decision option".into());
            }
        }
        if let Some(e) = &c.exhibit {
            validate_exhibit(e, 0, ids, assets, base, roots)?
        }
        for l in &c.html_links {
            asset(&l.path, true, assets, base, roots)?
        }
        validate_claims(&c.children, depth + 1, count, ids, assets, base, roots)?
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
) -> Result<(), String> {
    if depth > 12 {
        return Err("Exhibit depth exceeded".into());
    }
    match e {
        Exhibit::Image { path, .. } => asset(path, false, assets, base, roots)?,
        Exhibit::HtmlLink { path, .. } => asset(path, true, assets, base, roots)?,
        Exhibit::Calls { entries } => {
            for r in entries {
                unique(ids, &r.id)?
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
                    validate_exhibit(screen, depth + 1, ids, assets, base, roots)?
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
        validate_layout(&n.children, depth + 1, ids)?
    }
    Ok(())
}
fn safe_path(base: &Path, source: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    if source.contains("://")
        || source.contains('\0')
        || Path::new(source)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err("Plan paths must be local without traversal".into());
    }
    let path = base
        .join(source)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !roots
        .iter()
        .filter_map(|r| r.canonicalize().ok())
        .any(|r| path.starts_with(r))
    {
        return Err("Plan path escapes registered roots".into());
    }
    if !path.is_file() {
        return Err("Plan asset must be a regular file".into());
    }
    Ok(path)
}
fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    if !f.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Not a regular file".into());
    }
    let mut bytes = Vec::new();
    f.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("Plan file exceeds size limit".into());
    }
    Ok(bytes)
}
fn asset(
    source: &str,
    html: bool,
    assets: &mut Vec<AssetSnapshot>,
    base: &Path,
    roots: &[PathBuf],
) -> Result<(), String> {
    if let Some(old) = assets.iter().find(|a| a.path == source) {
        if html != (old.mime == "text/html") {
            return Err("Asset used as both image and HTML".into());
        }
        return Ok(());
    }
    if assets.len() >= 100 {
        return Err("Plan asset count exceeded".into());
    }
    let path = safe_path(base, source, roots)?;
    let remaining =
        (20usize * 1024 * 1024).saturating_sub(assets.iter().map(|a| a.bytes.len()).sum::<usize>());
    let bytes = read_bounded(
        &path,
        remaining.min(if html { 1024 * 1024 } else { 8 * 1024 * 1024 }),
    )?;
    let mime = if html {
        if !source.ends_with(".html") || std::str::from_utf8(&bytes).is_err() {
            return Err("Expected UTF-8 HTML asset".into());
        }
        "text/html"
    } else {
        raster_mime(&bytes)?
    };
    assets.push(AssetSnapshot {
        path: source.into(),
        mime: mime.into(),
        bytes: std::sync::Arc::new(bytes),
    });
    decoded_budget(assets)?;
    Ok(())
}
fn raster_mime(b: &[u8]) -> Result<&'static str, String> {
    let format = image::guess_format(b).map_err(|_| "Invalid raster image")?;
    let mime = match format {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::WebP => "image/webp",
        _ => return Err("Supported images are PNG, JPEG and WebP".into()),
    };
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(b), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(20 * 1024 * 1024);
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|e| format!("Invalid or oversized raster image: {e}"))?;
    if u64::from(decoded.width()) * u64::from(decoded.height()) * 4 > 20 * 1024 * 1024 {
        return Err("Decoded image exceeds 20 MiB".into());
    }
    Ok(mime)
}
fn decoded_budget(assets: &[AssetSnapshot]) -> Result<(), String> {
    let mut total = 0u64;
    for asset in assets.iter().filter(|a| a.mime.starts_with("image/")) {
        let reader = image::ImageReader::new(std::io::Cursor::new(asset.bytes.as_slice()))
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        let (w, h) = reader.into_dimensions().map_err(|e| e.to_string())?;
        total = total.saturating_add(u64::from(w) * u64::from(h) * 4);
        if total > 20 * 1024 * 1024 {
            return Err("Decoded plan image budget exceeds 20 MiB".into());
        }
    }
    Ok(())
}

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
                links.push((id.to_owned(), source));
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
    for (id, source) in links {
        let _ = saved.rich_plans.import(&id, &source, &saved.cwd, &roots);
    }
}
fn markdown_links(text: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut fence: Option<&str> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let marker = &trimmed[..3];
            if fence == Some(marker) {
                fence = None
            } else if fence.is_none() {
                fence = Some(marker)
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        for tail in line.split("](").skip(1) {
            if let Some((path, _)) = tail.split_once(')') {
                let path = path.trim().trim_start_matches('<').trim_end_matches('>');
                if path.ends_with(".grove-plan.json") {
                    links.push(path.to_owned());
                    if links.len() == 20 {
                        return links;
                    }
                }
            }
        }
    }
    links
}

impl PlanStore {
    pub fn is_delivery(&self, id: u64) -> bool {
        self.revisions.iter().any(|r|matches!(r.review.delivery,ResponseDelivery::Queued{id:held}|ResponseDelivery::Sent{id:held}|ResponseDelivery::Uncertain{id:held} if held==id))
    }
    pub fn update_review(
        &mut self,
        plan: &str,
        revision: &str,
        mut review: ReviewState,
    ) -> Result<(), String> {
        let r = self
            .revision_mut(plan, revision)
            .ok_or("Unknown plan revision")?;
        if !matches!(
            r.review.delivery,
            ResponseDelivery::Draft | ResponseDelivery::Failed { .. }
        ) {
            review.answers = r.review.answers.clone();
            review.own_answers = r.review.own_answers.clone();
            review.comments = r.review.comments.clone();
        }
        review.answers.retain(|_, value| !value.trim().is_empty());
        review
            .own_answers
            .retain(|_, value| !value.trim().is_empty());
        review.comments.retain(|_, value| !value.trim().is_empty());
        let value = serde_json::to_value(&review).map_err(|e| e.to_string())?;
        validate_strings(&value, 0)?;
        if serde_json::to_vec(&value).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
            return Err("Review exceeds 1 MiB".into());
        }
        // Delivery is actor-owned; editing cannot turn a confirmed send into another send.
        review.delivery = r.review.delivery.clone();
        r.review = review;
        Ok(())
    }
    pub fn prepare_delivery(
        &mut self,
        plan: &str,
        revision: &str,
    ) -> Result<Option<(u64, String)>, String> {
        let r = self
            .revision_mut(plan, revision)
            .ok_or("Unknown plan revision")?;
        if !matches!(
            r.review.delivery,
            ResponseDelivery::Draft | ResponseDelivery::Failed { .. }
        ) {
            return Ok(None);
        }
        let text = r.response_text();
        let digest = ring::digest::digest(
            &ring::digest::SHA256,
            format!("{plan}\0{revision}").as_bytes(),
        );
        let mut id_bytes = [0u8; 8];
        id_bytes.copy_from_slice(&digest.as_ref()[..8]);
        let id = u64::from_be_bytes(id_bytes) | (1 << 63);
        r.review.delivery = ResponseDelivery::Queued { id };
        Ok(Some((id, text)))
    }
    pub fn confirm_delivery(&mut self, id: u64) {
        for r in &mut self.revisions {
            if matches!(r.review.delivery,ResponseDelivery::Queued{id:held}|ResponseDelivery::Uncertain{id:held} if held==id)
            {
                r.review.delivery = ResponseDelivery::Sent { id }
            }
        }
    }
    pub fn uncertain_delivery(&mut self, id: u64) {
        for r in &mut self.revisions {
            if matches!(r.review.delivery,ResponseDelivery::Queued{id:held} if held==id) {
                r.review.delivery = ResponseDelivery::Uncertain { id }
            }
        }
    }
    pub fn fail_delivery(&mut self, id: u64, error: &str) {
        for r in &mut self.revisions {
            if matches!(r.review.delivery,ResponseDelivery::Queued{id:held} if held==id) {
                r.review.delivery = ResponseDelivery::Failed {
                    error: error.into(),
                }
            }
        }
    }
    pub fn cancel_delivery(&mut self, id: u64) {
        for r in &mut self.revisions {
            if matches!(r.review.delivery,ResponseDelivery::Queued{id:held} if held==id) {
                r.review.delivery = ResponseDelivery::Draft
            }
        }
    }
    pub fn retain_messages(&mut self, items: &[serde_json::Value]) {
        self.revisions.retain(|r| {
            items
                .iter()
                .any(|item| item["id"].as_str() == Some(&r.message_id))
        });
        self.errors.retain(|e| {
            items
                .iter()
                .any(|item| item["id"].as_str() == Some(&e.message_id))
        });
    }
    pub fn recover_delivery(&mut self) {
        for r in &mut self.revisions {
            if let ResponseDelivery::Queued { id } = r.review.delivery {
                r.review.delivery = ResponseDelivery::Uncertain { id }
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    fn document() -> PlanDocument {
        serde_json::from_value(serde_json::json!({"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"Review me","claims":[{"id":"claim","text":"A claim","decisions":[{"id":"choice","prompt":"Where?","options":[{"id":"inline","label":"Inline card"}],"default":"inline"}]}]})).unwrap()
    }
    fn fixture() -> (tempfile::TempDir, PlanStore) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("plan.grove-plan.json"),
            serde_json::to_vec(&document()).unwrap(),
        )
        .unwrap();
        (dir, PlanStore::default())
    }
    fn import(store: &mut PlanStore, dir: &Path) -> Result<(), String> {
        store.import(
            "message",
            "plan.grove-plan.json",
            dir.to_str().unwrap(),
            &[dir.to_owned()],
        )
    }
    #[test]
    fn snapshots_and_reviews_survive_source_removal_and_revision_conflicts() {
        let (dir, mut store) = fixture();
        import(&mut store, dir.path()).unwrap();
        let mut review = ReviewState::default();
        review.comments.insert("claim".into(), "Keep this".into());
        store.update_review("plan", "r1", review).unwrap();
        let mut conflicting = document();
        conflicting.title = "Changed same identity".into();
        std::fs::write(
            dir.path().join("plan.grove-plan.json"),
            serde_json::to_vec(&conflicting).unwrap(),
        )
        .unwrap();
        assert!(import(&mut store, dir.path()).is_err());
        assert_eq!(store.revisions.len(), 1);
        assert_eq!(store.revisions[0].document.title, "Review me");
        assert_eq!(store.errors.len(), 1);
        conflicting.revision_id = "r2".into();
        std::fs::write(
            dir.path().join("plan.grove-plan.json"),
            serde_json::to_vec(&conflicting).unwrap(),
        )
        .unwrap();
        import(&mut store, dir.path()).unwrap();
        std::fs::remove_file(dir.path().join("plan.grove-plan.json")).unwrap();
        let restored: PlanStore =
            serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
        assert_eq!(restored.revisions[0].review.comments["claim"], "Keep this");
        assert!(restored.revisions[1].review.comments.is_empty());
        assert_eq!(
            restored.revisions[1].document.title,
            "Changed same identity"
        );
    }
    #[test]
    fn delivery_journal_suppresses_duplicate_click_restart_and_confirmed_retry() {
        let (dir, mut store) = fixture();
        import(&mut store, dir.path()).unwrap();
        let mut review = ReviewState::default();
        review.answers.insert("choice".into(), "inline".into());
        store.update_review("plan", "r1", review).unwrap();
        let (id, text) = store.prepare_delivery("plan", "r1").unwrap().unwrap();
        assert!(text.contains("Where?"));
        assert!(text.contains("Inline card"));
        assert!(store.prepare_delivery("plan", "r1").unwrap().is_none());
        let mut spoof = ReviewState::default();
        spoof
            .comments
            .insert("claim".into(), "changed after send".into());
        store.update_review("plan", "r1", spoof).unwrap();
        assert_eq!(store.revisions[0].response_text(), text);
        let mut restart: PlanStore =
            serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
        restart.recover_delivery();
        assert!(
            matches!(restart.revisions[0].review.delivery,ResponseDelivery::Uncertain{id:held} if held==id)
        );
        assert!(restart.prepare_delivery("plan", "r1").unwrap().is_none());
        restart.confirm_delivery(id);
        assert!(restart.prepare_delivery("plan", "r1").unwrap().is_none());
        store.fail_delivery(id, "definitive rejection before side effect");
        assert_eq!(store.prepare_delivery("plan", "r1").unwrap().unwrap().0, id);
    }
    #[test]
    fn own_answers_unread_and_viewed_remain_distinct() {
        let (dir, mut store) = fixture();
        import(&mut store, dir.path()).unwrap();
        let r = &mut store.revisions[0];
        assert!(r.response_text().contains("Unread [choice]"));
        r.review.viewed_decisions.push("choice".into());
        assert!(r.response_text().contains("Viewed, unanswered"));
        r.review
            .own_answers
            .insert("choice".into(), "inline".into());
        let text = r.response_text();
        assert!(text.contains("Own answer [choice] Where?:\n> inline"));
        assert!(!text.contains("Inline card"));
    }
    #[test]
    fn rejects_traversal_symlink_escape_invalid_version_and_duplicate_ids() {
        let (dir, mut store) = fixture();
        assert!(store
            .import(
                "m",
                "../plan.grove-plan.json",
                dir.path().to_str().unwrap(),
                &[dir.path().to_owned()]
            )
            .is_err());
        let mut doc = document();
        doc.schema_version = 2;
        std::fs::write(
            dir.path().join("plan.grove-plan.json"),
            serde_json::to_vec(&doc).unwrap(),
        )
        .unwrap();
        assert!(import(&mut store, dir.path()).is_err());
        doc.schema_version = 1;
        doc.claims.push(doc.claims[0].clone());
        std::fs::write(
            dir.path().join("plan.grove-plan.json"),
            serde_json::to_vec(&doc).unwrap(),
        )
        .unwrap();
        assert!(import(&mut store, dir.path()).is_err());
        assert!(store.revisions.is_empty());
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(
                outside.path().join("outside.grove-plan.json"),
                serde_json::to_vec(&document()).unwrap(),
            )
            .unwrap();
            std::os::unix::fs::symlink(
                outside.path().join("outside.grove-plan.json"),
                dir.path().join("escape.grove-plan.json"),
            )
            .unwrap();
            assert!(store
                .import(
                    "m",
                    "escape.grove-plan.json",
                    dir.path().to_str().unwrap(),
                    &[dir.path().to_owned()]
                )
                .is_err());
        }
    }
    #[test]
    fn malformed_images_never_become_snapshots_and_html_is_immutable() {
        let (dir, mut store) = fixture();
        let mut doc = document();
        doc.claims[0].exhibit = Some(Exhibit::Image {
            path: "bad.png".into(),
            alt: "bad".into(),
        });
        std::fs::write(dir.path().join("bad.png"), b"\x89PNG\r\n\x1a\n").unwrap();
        std::fs::write(
            dir.path().join("plan.grove-plan.json"),
            serde_json::to_vec(&doc).unwrap(),
        )
        .unwrap();
        assert!(import(&mut store, dir.path()).is_err());
        assert!(store.revisions.is_empty());
        doc.claims[0].exhibit = Some(Exhibit::HtmlLink {
            path: "exhibit.html".into(),
            label: "Open".into(),
        });
        std::fs::write(dir.path().join("exhibit.html"), b"<p>saved</p>").unwrap();
        std::fs::write(
            dir.path().join("plan.grove-plan.json"),
            serde_json::to_vec(&doc).unwrap(),
        )
        .unwrap();
        import(&mut store, dir.path()).unwrap();
        std::fs::write(dir.path().join("exhibit.html"), b"<p>changed</p>").unwrap();
        assert!(import(&mut store, dir.path()).is_err());
        assert_eq!(
            store.revisions[0].assets[0].bytes.as_slice(),
            b"<p>saved</p>"
        );
    }
    #[test]
    fn imported_links_only_come_from_completed_assistant_messages() {
        let (dir, _) = fixture();
        let mut saved:SavedSession=serde_json::from_value(serde_json::json!({"key":"test","cwd":dir.path(),"project":"test","label":"test","roots":[],"thread_id":null,"items":[{"id":"user","type":"userMessage","text":"[plan](plan.grove-plan.json)"},{"id":"stream","type":"agentMessage","status":"inProgress","text":"[plan](plan.grove-plan.json)"}]})).unwrap();
        import_completed(&mut saved);
        assert!(saved.rich_plans.revisions.is_empty());
        saved.items[1]["status"] = serde_json::json!("completed");
        import_completed(&mut saved);
        assert_eq!(saved.rich_plans.revisions.len(), 1);
        assert_eq!(saved.rich_plans.revisions[0].message_id, "stream");
        import_completed(&mut saved);
        assert_eq!(saved.rich_plans.revisions.len(), 1);
    }
    #[test]
    fn canceled_queue_feedback_can_be_reviewed_and_sent_again_without_old_payload() {
        let (dir, mut store) = fixture();
        import(&mut store, dir.path()).unwrap();
        let (id, _) = store.prepare_delivery("plan", "r1").unwrap().unwrap();
        assert!(store.is_delivery(id));
        store.cancel_delivery(id);
        assert!(!store.is_delivery(id));
        let mut review = ReviewState::default();
        review
            .comments
            .insert("claim".into(), "Changed after cancellation".into());
        store.update_review("plan", "r1", review).unwrap();
        let (new_id, text) = store.prepare_delivery("plan", "r1").unwrap().unwrap();
        assert_eq!(id, new_id);
        assert!(text.contains("Changed after cancellation"));
    }
    #[test]
    fn repeated_revision_links_are_not_reopened_and_fenced_links_are_not_imported() {
        let (dir, _) = fixture();
        let mut saved:SavedSession=serde_json::from_value(serde_json::json!({"key":"test","cwd":dir.path(),"project":"test","label":"test","roots":[],"thread_id":null,"items":[{"id":"m1","type":"agentMessage","text":"[plan](plan.grove-plan.json)"},{"id":"fenced","type":"agentMessage","text":"```json\n[plan](missing.grove-plan.json)\n```"}]})).unwrap();
        import_completed(&mut saved);
        assert_eq!(saved.rich_plans.revisions.len(), 1);
        assert!(saved.rich_plans.errors.is_empty());
        saved.items.push(serde_json::json!({"id":"m2","type":"agentMessage","text":"[same](plan.grove-plan.json)"}));
        import_completed(&mut saved);
        std::fs::remove_file(dir.path().join("plan.grove-plan.json")).unwrap();
        import_completed(&mut saved);
        assert!(saved.rich_plans.errors.is_empty());
        assert_eq!(saved.rich_plans.revisions.len(), 1);
        saved.items.retain(|i| i["id"] != "m1");
        saved.rich_plans.retain_messages(&saved.items);
        assert!(saved.rich_plans.revisions.is_empty());
    }
}
