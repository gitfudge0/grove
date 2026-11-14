//! Immutable, offline visual publications. File bytes never enter transcript JSON.
use base64::{engine::general_purpose::STANDARD, Engine};
use html5ever::{parse_document, serialize, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom, SerializableHandle};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const DOCUMENT_LIMIT: usize = 2 * 1024 * 1024;
pub const BUNDLE_LIMIT: usize = 10 * 1024 * 1024;
const ASSET_LIMIT: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HtmlPolicy {
    #[default]
    Legacy,
    Explicit,
}

/// Layout intent defines agent preview dimensions independently of the transcript attachment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presentation {
    Inline,
    #[default]
    Dashboard,
    Canvas,
    Document,
}
impl Presentation {
    pub fn viewport(self) -> (u32, u32) {
        match self {
            Self::Inline => (728, 320),
            Self::Dashboard => (1200, 640),
            Self::Canvas => (1200, 640),
            Self::Document => (800, 480),
        }
    }
}
/// A document scroll target, or world-space frame for a cooperating canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationOptions {
    #[serde(default)]
    pub mode: Presentation,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_region: Option<PreviewRegion>,
}
impl Default for PresentationOptions {
    fn default() -> Self {
        let (width, height) = Presentation::default().viewport();
        Self {
            mode: Presentation::default(),
            width,
            height,
            preview_region: None,
        }
    }
}
impl Input {
    pub fn presentation_options(&self) -> PresentationOptions {
        let mode = self.presentation.unwrap_or_default();
        let (width, height) = mode.viewport();
        PresentationOptions {
            mode,
            width: self.width.unwrap_or(width),
            height: self.height.unwrap_or(height),
            preview_region: self.preview_region,
        }
    }
}
impl PresentationOptions {
    /// Run once after layout, not on theme/zoom updates, to preserve the reader's camera.
    pub fn initial_view_script(&self) -> String {
        let region = serde_json::to_string(&self.preview_region).unwrap_or_default();
        format!("(()=>{{globalThis.__groveInitialViewReady=false;const frame={region};const apply=()=>{{const ready=()=>requestAnimationFrame(()=>requestAnimationFrame(()=>{{if(frame)window.scrollTo(frame.x,frame.y);window.dispatchEvent(new CustomEvent('grove:initial-view',{{detail:{{frame}}}}));globalThis.__groveInitialViewReady=true;}}));if(document.readyState==='complete')ready();else addEventListener('load',ready,{{once:true}});}};addEventListener('grove:activate-view',apply,{{once:true}});}})();")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub scope: String,
    pub publication_id: String,
    pub visual_id: String,
    pub revision_id: String,
    pub source_digest: String,
    pub title: String,
    pub item_id: String,
    pub turn_id: String,
    #[serde(default)]
    pub presentation: PresentationOptions,
}
#[derive(Serialize, Deserialize)]
struct Bundle {
    version: u8,
    reference: Reference,
    html: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub request_id: Option<String>,
    pub html: Option<String>,
    pub path: Option<String>,
    pub title: String,
    pub visual_id: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub presentation: Option<Presentation>,
    pub preview_region: Option<PreviewRegion>,
}

pub fn digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let hash = ring::digest::digest(&ring::digest::SHA256, bytes);
    let mut encoded = String::with_capacity(64);
    for byte in hash.as_ref() {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 15) as usize] as char);
    }
    encoded
}
fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
pub fn directory() -> Result<PathBuf, String> {
    crate::storage::config_dir()
        .map(|p| p.join("visuals-v1"))
        .map_err(|e| e.to_string())
}
fn bundle_path(root: &Path, scope: &str, id: &str) -> Result<PathBuf, String> {
    if !valid_id(scope) || !valid_id(id) {
        return Err("Invalid saved visual identity".into());
    }
    Ok(root.join(scope).join(format!("{id}.json")))
}
pub fn read(reference: &Reference) -> Result<String, String> {
    read_in(&directory()?, reference)
}
fn read_in(root: &Path, reference: &Reference) -> Result<String, String> {
    let bundle = read_bundle(&bundle_path(
        root,
        &reference.scope,
        &reference.publication_id,
    )?)?;
    if bundle.version != 1
        || bundle.reference != *reference
        || digest(bundle.html.as_bytes()) != reference.source_digest
    {
        return Err("Saved visual integrity check failed".into());
    }
    Ok(bundle.html)
}
fn read_bundle(path: &Path) -> Result<Bundle, String> {
    let file = fs_err::File::open(path).map_err(|e| format!("Saved visual unavailable: {e}"))?;
    let limit = BUNDLE_LIMIT * 6 + 8192;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err("Saved visual bundle exceeds its limit or is not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("Saved visual bundle exceeds its limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Saved visual bundle is corrupt".into())
}

/// One atomic file is both the snapshot and retry journal. No success precedes fsync.
pub fn publish(
    root: &Path,
    session: &str,
    item: &str,
    turn: &str,
    input: &Input,
    html: String,
) -> Result<Reference, String> {
    publish_with_references(root, session, item, turn, input, html, &[])
}

pub(crate) fn publish_with_references(
    root: &Path,
    session: &str,
    item: &str,
    turn: &str,
    input: &Input,
    html: String,
    references: &[Reference],
) -> Result<Reference, String> {
    publish_cancellable(root, (session, item, turn), input, html, references, || {
        false
    })
}

pub(crate) fn publish_cancellable(
    root: &Path,
    owner: (&str, &str, &str),
    input: &Input,
    html: String,
    references: &[Reference],
    cancelled: impl Fn() -> bool,
) -> Result<Reference, String> {
    validate_presentation(input)?;
    let (session, item, turn) = owner;
    if cancelled() {
        return Err("Visual publication cancelled before commit".into());
    }
    if html.len() > BUNDLE_LIMIT {
        return Err("Visual bundle exceeds 10 MiB".into());
    }
    let scope = digest(session.as_bytes());
    let publication_id = digest(format!("{session}\0{turn}\0{item}").as_bytes());
    let path = bundle_path(root, &scope, &publication_id)?;
    let source_digest = digest(html.as_bytes());
    if path.exists() {
        let old = read_bundle(&path)?;
        if old.reference.source_digest != source_digest
            || old.reference.title != input.title
            || old.reference.presentation != input.presentation_options()
        {
            return Err("This tool event already published different content".into());
        }
        read_in(root, &old.reference)?;
        return Ok(old.reference);
    }
    let visual_id = match &input.visual_id {
        Some(id) if valid_id(id) => {
            let exists = references
                .iter()
                .any(|reference| reference.visual_id == *id)
                || fs_err::read_dir(root.join(&scope))
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                    .any(|entry| {
                        read_bundle(&entry.path())
                            .ok()
                            .is_some_and(|b| b.reference.visual_id == *id)
                    });
            if !exists {
                return Err("Unknown visual ID in this conversation".into());
            }
            id.clone()
        }
        Some(_) => return Err("Invalid visual ID".into()),
        None => publication_id.clone(),
    };
    let reference = Reference {
        scope,
        publication_id: publication_id.clone(),
        visual_id,
        revision_id: publication_id,
        source_digest,
        title: input.title.clone(),
        item_id: item.into(),
        turn_id: turn.into(),
        presentation: input.presentation_options(),
    };
    let parent = path.parent().ok_or("Invalid visual storage path")?;
    fs_err::create_dir_all(parent).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs_err::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let mut temp = tempfile::Builder::new()
        .prefix(".grove-visual-stage-")
        .tempfile_in(parent)
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(
        temp.as_file_mut(),
        &Bundle {
            version: 1,
            reference: reference.clone(),
            html,
        },
    )
    .map_err(|e| e.to_string())?;
    temp.flush()
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    if cancelled() {
        return Err("Visual publication cancelled before commit".into());
    }
    temp.persist_noclobber(&path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    fs_err::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(reference)
}
/// Reconcile a durable publication whose MCP acknowledgement or session save was interrupted.
/// Only existing tool anchors qualify; removed history cannot resurrect a publication.
pub fn recover(saved: &mut crate::provider::SavedSession) {
    let Ok(root) = directory() else {
        return;
    };
    recover_in(saved, &root);
}
/// Recovery on the visual worker after provider history restoration.
pub(crate) fn recover_events(session: &str, items: &[String]) -> Vec<Reference> {
    let mut saved = crate::provider::SavedSession::empty(
        session.into(),
        crate::provider::Provider::Codex,
        String::new(),
        String::new(),
    );
    for id in items {
        saved.items.push(serde_json::json!({"id":id,"type":"mcpToolCall","server":"grove_visuals","tool":"render_html"}));
    }
    recover(&mut saved);
    saved.visuals
}
fn recover_in(saved: &mut crate::provider::SavedSession, root: &Path) {
    let Ok(entries) = fs_err::read_dir(root.join(digest(saved.key.as_bytes()))) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let Ok(bundle) = read_bundle(&entry.path()) else {
            continue;
        };
        if bundle.version == 1
            && bundle.reference.scope == digest(saved.key.as_bytes())
            && saved.items.iter().any(|i| {
                i["id"] == bundle.reference.item_id
                    && crate::visuals_mcp::visual_tool(i, saved.provider) == Some("render_html")
            })
            && !saved
                .visuals
                .iter()
                .any(|r| r.publication_id == bundle.reference.publication_id)
        {
            saved.visuals.push(bundle.reference);
        }
    }
}

fn read_local(path: &Path, roots: &[PathBuf], limit: usize) -> Result<Vec<u8>, String> {
    if path.to_string_lossy().starts_with("\\\\") || path.to_string_lossy().starts_with("//") {
        return Err("Network paths are unsupported".into());
    }
    let target = path
        .canonicalize()
        .map_err(|e| format!("Cannot resolve {}: {e}", path.display()))?;
    let allowed_roots: Vec<PathBuf> = roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .collect();
    if !allowed_roots.iter().any(|root| target.starts_with(root)) {
        return Err("Visual file must be within a registered conversation root".into());
    }
    let before = fs_err::metadata(&target).map_err(|e| e.to_string())?;
    if !before.is_file() || before.len() > limit as u64 {
        return Err(format!(
            "Visual file must be a regular file at most {limit} bytes"
        ));
    }
    let mut options = fs_err::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use fs_err::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use fs_err::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options.open(&target).map_err(|e| e.to_string())?;
    let after = file.metadata().map_err(|e| e.to_string())?;
    #[cfg(unix)]
    verify_unix_handle(file.file(), &allowed_roots)?;
    #[cfg(windows)]
    verify_windows_handle(file.file(), &target)?;
    if !after.is_file() {
        return Err("Visual input is not a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != after.dev() || before.ino() != after.ino() || after.nlink() != 1 {
            return Err("Visual file changed while opening or has multiple links".into());
        }
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err(format!("Visual input exceeds {limit} bytes"));
    }
    Ok(bytes)
}

fn validate_presentation(input: &Input) -> Result<(), String> {
    if input.width.is_some_and(|v| !(240..=1600).contains(&v))
        || input.height.is_some_and(|v| !(80..=2000).contains(&v))
    {
        return Err("Viewport width must be 240–1600 and height 80–2000".into());
    }
    if input.preview_region.is_some_and(|r| {
        r.width == 0
            || r.height == 0
            || r.width > 100_000
            || r.height > 100_000
            || r.x > 1_000_000
            || r.y > 1_000_000
    }) {
        return Err("Preview region requires positive bounded dimensions and coordinates".into());
    }
    Ok(())
}

pub fn prepare(
    input: &Input,
    cwd: &Path,
    roots: &[PathBuf],
    host_reads: bool,
) -> Result<String, String> {
    if input.title.trim().is_empty() || input.title.len() > 200 {
        return Err("Provide a title of 1–200 bytes".into());
    }
    validate_presentation(input)?;
    let (source, base) = match (&input.html, &input.path) {
        (Some(html), None) => (html.clone(), cwd.to_path_buf()),
        (None, Some(path)) if host_reads => {
            let path = Path::new(path);
            if !path.is_absolute() || path.to_string_lossy().starts_with("\\\\") {
                return Err("Use an absolute local HTML path".into());
            }
            if !path
                .extension()
                .and_then(|v| v.to_str())
                .is_some_and(|v| v.eq_ignore_ascii_case("html") || v.eq_ignore_ascii_case("htm"))
            {
                return Err("Visual path must end in .html or .htm".into());
            }
            let bytes = read_local(path, roots, DOCUMENT_LIMIT)?;
            (
                String::from_utf8(bytes).map_err(|_| "HTML must be UTF-8")?,
                path.parent().ok_or("Invalid HTML path")?.into(),
            )
        }
        (None, Some(_)) => {
            return Err(
                "Direct host reads are not authorized; pass inline HTML with data assets".into(),
            )
        }
        _ => return Err("Pass exactly one of html or path".into()),
    };
    let lower = source.trim_start().to_ascii_lowercase();
    if source.len() > DOCUMENT_LIMIT
        || !(lower.starts_with("<!doctype html") || lower.starts_with("<html"))
        || !lower.contains("</html>")
    {
        return Err("Provide a complete HTML document at most 2 MiB".into());
    }
    let dom = parse_document(RcDom::default(), html5ever::ParseOpts::default()).one(source);
    let mut assets = 0;
    let mut bytes = 0;
    visit(
        &dom.document,
        &base,
        roots,
        host_reads,
        &mut assets,
        &mut bytes,
        0,
    )?;
    let mut output = Vec::new();
    serialize(
        &mut output,
        &SerializableHandle::from(dom.document),
        html5ever::serialize::SerializeOpts::default(),
    )
    .map_err(|e| e.to_string())?;
    if output.len() > BUNDLE_LIMIT {
        return Err("Visual bundle exceeds 10 MiB".into());
    }
    let mut html = String::from_utf8(output).map_err(|e| e.to_string())?;
    if !html.contains("<!-- grove:integrated-report -->") {
        html.insert_str(0, "<!-- grove:integrated-report -->\n");
    }
    html.push_str("<script>");
    html.push_str(&input.presentation_options().initial_view_script());
    html.push_str("</script>");
    if html.len() > BUNDLE_LIMIT {
        return Err("Visual bundle exceeds 10 MiB".into());
    }
    Ok(html)
}
fn validate_css(css: &str) -> Result<(), String> {
    // Escaped URL tokens can hide dependencies; require authors to use plain inline CSS.
    let lower = css.to_ascii_lowercase();
    if lower.contains("@import")
        || lower.contains('\\')
        || lower.contains("url(")
            && !lower
                .split("url(")
                .skip(1)
                .all(|v| v.trim_start_matches([' ', '\'', '"']).starts_with("data:"))
    {
        return Err(
            "Inline CSS dependencies as data URLs; imports and escaped CSS are unsupported".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(html: &str) -> Input {
        Input {
            request_id: None,
            html: Some(html.into()),
            path: None,
            title: "Chart".into(),
            visual_id: None,
            width: None,
            height: None,
            presentation: None,
            preview_region: None,
        }
    }
    #[test]
    fn presentation_is_durable_and_retry_identity_includes_layout() {
        let root = tempfile::tempdir().unwrap();
        let mut i = input("<!doctype html><html><body>Canvas</body></html>");
        i.presentation = Some(Presentation::Canvas);
        i.width = Some(1440);
        i.preview_region = Some(PreviewRegion {
            x: 800,
            y: 400,
            width: 600,
            height: 400,
        });
        let source = prepare(&i, root.path(), &[], false).unwrap();
        assert!(source.contains("grove:initial-view"));
        let reference =
            publish(root.path(), "session", "tool", "turn", &i, source.clone()).unwrap();
        assert_eq!(reference.presentation.mode, Presentation::Canvas);
        assert_eq!(reference.presentation.width, 1440);
        assert_eq!(read_in(root.path(), &reference).unwrap(), source);
        assert_eq!(
            publish(root.path(), "session", "tool", "turn", &i, source.clone()).unwrap(),
            reference
        );
        i.width = Some(800);
        assert!(publish(root.path(), "session", "tool", "turn", &i, source).is_err());
        i.preview_region.as_mut().unwrap().width = 0;
        assert!(prepare(&i, root.path(), &[], false).is_err());
    }
    #[test]
    fn old_references_get_a_bounded_dashboard_presentation() {
        let reference: Reference = serde_json::from_value(serde_json::json!({"scope":"a","publication_id":"b","visual_id":"c","revision_id":"d","source_digest":"e","title":"Old","item_id":"tool","turn_id":"turn"})).unwrap();
        assert_eq!(reference.presentation, PresentationOptions::default());
        for mode in [
            Presentation::Inline,
            Presentation::Dashboard,
            Presentation::Canvas,
            Presentation::Document,
        ] {
            let mut i = input("<html><body>Sample</body></html>");
            i.presentation = Some(mode);
            assert_eq!(
                (
                    i.presentation_options().width,
                    i.presentation_options().height
                ),
                mode.viewport()
            );
        }
    }
    #[test]
    fn immutable_retry_restart_and_corruption() {
        let root = tempfile::tempdir().unwrap();
        let i = input("<!doctype html><html><body>First</body></html>");
        let html = prepare(&i, root.path(), &[], false).unwrap();
        let r = publish(root.path(), "session", "tool", "turn", &i, html.clone()).unwrap();
        assert_eq!(
            publish(root.path(), "session", "tool", "turn", &i, html.clone()).unwrap(),
            r
        );
        assert_eq!(read_in(root.path(), &r).unwrap(), html);
        assert!(publish(root.path(), "session", "tool", "turn", &i, "changed".into()).is_err());
        fs_err::write(
            bundle_path(root.path(), &r.scope, &r.publication_id).unwrap(),
            "bad",
        )
        .unwrap();
        assert!(read_in(root.path(), &r).is_err());
    }
    #[test]
    fn validation_and_access_do_not_depend_on_provider_approvals() {
        let root = tempfile::tempdir().unwrap();
        for html in [
            "<b>fragment</b>",
            "<html><script src='https://example.com/x.js'></script></html>",
            "<html><link href='local.css'></html>",
            "<html><img src='../secret.png'></html>",
            "<html><style>@import 'local.css';</style></html>",
        ] {
            assert!(
                prepare(&input(html), root.path(), &[], false).is_err(),
                "{html}"
            );
        }
        let file = root.path().join("page.html");
        fs_err::write(&file, "<html>saved</html>").unwrap();
        let mut i = input("");
        i.html = None;
        i.path = Some(file.display().to_string());
        assert!(prepare(&i, root.path(), &[root.path().into()], false).is_err());
        let html = prepare(&i, root.path(), &[root.path().into()], true).unwrap();
        let r = publish(root.path(), "session", "tool", "turn", &i, html.clone()).unwrap();
        fs_err::remove_file(file).unwrap();
        assert_eq!(read_in(root.path(), &r).unwrap(), html);
    }
    #[test]
    fn revisions_are_new_snapshots() {
        let root = tempfile::tempdir().unwrap();
        let i = input("<html>one</html>");
        let r = publish(
            root.path(),
            "session",
            "a",
            "turn",
            &i,
            i.html.clone().unwrap(),
        )
        .unwrap();
        let mut next = input("<html>two</html>");
        next.visual_id = Some(r.visual_id.clone());
        let second = publish(
            root.path(),
            "session",
            "b",
            "turn",
            &next,
            next.html.clone().unwrap(),
        )
        .unwrap();
        assert_eq!(second.visual_id, r.visual_id);
        assert_ne!(second.revision_id, r.revision_id);
        assert_eq!(read_in(root.path(), &r).unwrap(), "<html>one</html>");
    }
}

/// Conservative mark/sweep: every saved JSON record, archive and recovery journal
/// is a root. Unknown/unreadable records abort sweeping. Recent orphan commits
/// remain retry journals for seven days; existing sessions retain all their journals.
pub fn cleanup() -> Result<(), String> {
    let config = crate::storage::config_dir().map_err(|e| e.to_string())?;
    cleanup_in(&config, std::time::Duration::from_hours(168))
}
fn cleanup_in(config: &Path, age: std::time::Duration) -> Result<(), String> {
    let root = config.join("visuals-v1");
    if !root.exists() {
        return Ok(());
    }
    let mut sessions = HashSet::new();
    let mut references = HashSet::new();
    let mut pending = vec![config.to_path_buf()];
    let mut files = 0;
    let mut total = 0;
    while let Some(dir) = pending.pop() {
        for entry in fs_err::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path == root {
                continue;
            }
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_symlink() {
                return Err("Visual cleanup skipped: unresolved configuration symlink".into());
            }
            if kind.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "json") {
                files += 1;
                total += entry.metadata().map_err(|e| e.to_string())?.len();
                if files > 25000 || total > 128 * 1024 * 1024 {
                    return Err("Visual cleanup scan limit reached; bundles retained".into());
                }
                let value = serde_json::from_slice(&fs_err::read(path).map_err(|e| e.to_string())?)
                    .map_err(|_| "Visual cleanup skipped: unreadable JSON root")?;
                mark(&value, &mut sessions, &mut references);
            }
        }
    }
    for scope in fs_err::read_dir(&root)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
    {
        if !scope.file_type().map_err(|e| e.to_string())?.is_dir() {
            continue;
        }
        let scope_name = scope.file_name().to_string_lossy().to_string();
        for entry in fs_err::read_dir(scope.path())
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let metadata = entry.metadata().map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_file()
                || metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_none_or(|elapsed| elapsed < age)
            {
                continue;
            }
            let temporary = name.starts_with(".grove-visual-stage-");
            let unreferenced = name.strip_suffix(".json").is_some_and(|id| {
                valid_id(id)
                    && !sessions.contains(&scope_name)
                    && !references.contains(&(scope_name.clone(), id.into()))
            });
            if temporary || unreferenced {
                fs_err::remove_file(entry.path()).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    fn input(html: &str) -> Input {
        Input {
            request_id: Some("test".into()),
            html: Some(html.into()),
            path: None,
            title: "Saved visual".into(),
            visual_id: None,
            width: None,
            height: None,
            presentation: None,
            preview_region: None,
        }
    }
    #[test]
    fn policies_default_old_sessions_and_preserve_forks() {
        use crate::provider::{Provider, SavedSession};
        let new = SavedSession::empty("new".into(), Provider::Codex, "test".into(), "/tmp".into());
        assert_eq!(new.html_policy, HtmlPolicy::Explicit);
        let mut encoded = serde_json::to_value(&new).unwrap();
        encoded.as_object_mut().unwrap().remove("html_policy");
        let old: SavedSession = serde_json::from_value(encoded).unwrap();
        assert_eq!(old.html_policy, HtmlPolicy::Legacy);
        assert_eq!(old.fresh_conversation().html_policy, HtmlPolicy::Explicit);
        assert_eq!(
            old.fork_at(0, false).unwrap().html_policy,
            HtmlPolicy::Legacy
        );
    }
    #[test]
    fn embeds_real_raster_bytes_and_survives_asset_deletion() {
        let root = tempfile::tempdir().unwrap();
        let asset = root.path().join("pixel.png");
        image::DynamicImage::new_rgb8(2, 2).save(&asset).unwrap();
        let i = input("<html><body><img src='pixel.png'></body></html>");
        let html = prepare(&i, root.path(), &[root.path().into()], true).unwrap();
        assert!(html.contains("data:image/png;base64,"));
        let r = publish(root.path(), "session", "tool", "turn", &i, html.clone()).unwrap();
        fs_err::remove_file(asset).unwrap();
        assert_eq!(read_in(root.path(), &r).unwrap(), html);
        let fake = root.path().join("secret.png");
        fs_err::write(fake, "not an image").unwrap();
        assert!(prepare(
            &input("<html><img src='secret.png'></html>"),
            root.path(),
            &[root.path().into()],
            true
        )
        .is_err());
    }
    #[cfg(unix)]
    #[test]
    fn outside_symlinks_fifos_hardlinks_and_limits_fail_closed() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("page.html");
        fs_err::write(&file, "<html>outside</html>").unwrap();
        let link = root.path().join("link.html");
        symlink(&file, &link).unwrap();
        assert!(read_local(&link, &[root.path().into()], DOCUMENT_LIMIT).is_err());
        let inside = root.path().join("inside.html");
        fs_err::write(&inside, "<html>inside</html>").unwrap();
        fs_err::hard_link(&inside, root.path().join("hard.html")).unwrap();
        assert!(read_local(&inside, &[root.path().into()], DOCUMENT_LIMIT).is_err());
        let fifo = root.path().join("pipe.html");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        assert!(read_local(&fifo, &[root.path().into()], DOCUMENT_LIMIT).is_err());
        assert!(prepare(
            &input(&format!("<html>{}</html>", "x".repeat(DOCUMENT_LIMIT))),
            root.path(),
            &[],
            false
        )
        .is_err());
    }
    #[test]
    fn failed_storage_publishes_no_reference() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("not-a-directory");
        fs_err::write(&file, "x").unwrap();
        let i = input("<html>saved</html>");
        assert!(publish(
            &file,
            "session",
            "tool",
            "turn",
            &i,
            i.html.clone().unwrap()
        )
        .is_err());
    }
    #[test]
    fn cleanup_preserves_fork_refs_and_active_session_journals() {
        let config = tempfile::tempdir().unwrap();
        let root = config.path().join("visuals-v1");
        let i = input("<html>saved</html>");
        let r = publish(
            &root,
            "original",
            "tool",
            "turn",
            &i,
            i.html.clone().unwrap(),
        )
        .unwrap();
        fs_err::write(
            config.path().join("fork.json"),
            serde_json::to_vec(&serde_json::json!({"key":"fork","visuals":[r]})).unwrap(),
        )
        .unwrap();
        cleanup_in(config.path(), std::time::Duration::ZERO).unwrap();
        assert!(read_in(&root, &r).is_ok());
        fs_err::write(config.path().join("fork.json"), "{\"key\":\"original\"}").unwrap();
        cleanup_in(config.path(), std::time::Duration::ZERO).unwrap();
        assert!(read_in(&root, &r).is_ok());
        fs_err::remove_file(config.path().join("fork.json")).unwrap();
        cleanup_in(config.path(), std::time::Duration::ZERO).unwrap();
        assert!(read_in(&root, &r).is_err());
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn verify_windows_handle(file: &std::fs::File, target: &Path) -> Result<(), String> {
    use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
    if file
        .metadata()
        .map_err(|e| e.to_string())?
        .file_attributes()
        & 0x400
        != 0
    {
        return Err("Reparse-point visual input is unsupported".into());
    }
    let mut name = vec![0u16; 32768];
    let length = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            name.as_mut_ptr(),
            name.len() as u32,
            0,
        )
    } as usize;
    if length == 0 || length >= name.len() {
        return Err("Could not verify opened visual file".into());
    }
    use std::os::windows::ffi::OsStringExt;
    let opened = PathBuf::from(std::ffi::OsString::from_wide(&name[..length]));
    if opened != target {
        return Err("Visual path changed while opening".into());
    }
    Ok(())
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn lost_acknowledgement_reconciles_only_existing_source_events() {
        use crate::provider::{Provider, SavedSession};
        let root = tempfile::tempdir().unwrap();
        let input: Input = serde_json::from_value(
            serde_json::json!({"html":"<html>durable</html>","title":"Durable"}),
        )
        .unwrap();
        let reference = publish(
            root.path(),
            "session",
            "tool",
            "turn",
            &input,
            input.html.clone().unwrap(),
        )
        .unwrap();
        let mut saved = SavedSession::empty(
            "session".into(),
            Provider::Codex,
            "test".into(),
            "/tmp".into(),
        );
        saved.items.push(serde_json::json!({"id":"tool","type":"mcpToolCall","server":"grove_visuals","tool":"render_html"}));
        recover_in(&mut saved, root.path());
        recover_in(&mut saved, root.path());
        assert_eq!(saved.visuals, vec![reference.clone()]);
        let fork = saved.fork_at(1, false).unwrap();
        assert_eq!(fork.visuals, vec![reference.clone()]);
        assert_eq!(
            read_in(root.path(), &fork.visuals[0]).unwrap(),
            "<html>durable</html>"
        );
        saved.items.clear();
        saved.visuals.clear();
        recover_in(&mut saved, root.path());
        assert!(
            saved.visuals.is_empty(),
            "removed anchors must never be resurrected"
        );
    }
}

fn visit(
    node: &Handle,
    base: &Path,
    roots: &[PathBuf],
    host_reads: bool,
    assets: &mut usize,
    bytes: &mut usize,
    depth: usize,
) -> Result<(), String> {
    if depth > 128 {
        return Err("Visual markup exceeds the nesting limit".into());
    }
    if let NodeData::Element { name, attrs, .. } = &node.data {
        let tag = name.local.as_ref();
        if matches!(
            tag,
            "base" | "iframe" | "frame" | "object" | "embed" | "link" | "template"
        ) {
            return Err(format!(
                "Inline all dependencies; {tag} elements are unsupported"
            ));
        }
        for attr in attrs.borrow_mut().iter_mut() {
            let key = attr.name.local.as_ref();
            let value = attr.value.to_string();
            if key == "style" {
                validate_css(&value)?;
            }
            if key == "srcset" {
                return Err("Use a single embedded img src instead of srcset".into());
            }
            if matches!(key, "src" | "poster" | "data" | "background")
                || (key == "href" && tag != "a")
            {
                if value.starts_with("data:") {
                    *bytes += value.len();
                } else if value.starts_with('#') || value.is_empty() {
                    continue;
                } else if tag == "img"
                    && key == "src"
                    && host_reads
                    && !value.contains("://")
                    && !value.starts_with("//")
                {
                    *assets += 1;
                    if *assets > ASSET_LIMIT {
                        return Err("A visual may embed at most 64 local images".into());
                    }
                    let data = read_local(&base.join(&value), roots, DOCUMENT_LIMIT)?;
                    let format = image::guess_format(&data)
                        .map_err(|_| "Local asset is not a supported raster image")?;
                    let mut reader = image::ImageReader::new(std::io::Cursor::new(&data))
                        .with_guessed_format()
                        .map_err(|e| e.to_string())?;
                    let mut limits = image::Limits::default();
                    limits.max_alloc = Some(16 * 1024 * 1024);
                    limits.max_image_width = Some(8192);
                    limits.max_image_height = Some(8192);
                    reader.limits(limits);
                    reader
                        .decode()
                        .map_err(|_| "Local raster image is corrupt or exceeds decoding limits")?;

                    let mime = match format {
                        image::ImageFormat::Png => "image/png",
                        image::ImageFormat::Jpeg => "image/jpeg",
                        image::ImageFormat::WebP => "image/webp",
                        _ => {
                            return Err(
                                "Use PNG, JPEG or WebP local images; inline other assets".into()
                            )
                        }
                    };
                    let uri = format!("data:{mime};base64,{}", STANDARD.encode(data));
                    *bytes += uri.len();
                    attr.value = uri.into();
                } else {
                    return Err(format!("Unsupported dependency {key}; inline it as data or authorize a registered local raster image"));
                }
                if *bytes > BUNDLE_LIMIT {
                    return Err("Visual bundle exceeds 10 MiB".into());
                }
            }
        }
        if tag == "style" {
            for child in node.children.borrow().iter() {
                if let NodeData::Text { contents } = &child.data {
                    validate_css(&contents.borrow())?;
                }
            }
        }
    }
    for child in node.children.borrow().iter() {
        visit(child, base, roots, host_reads, assets, bytes, depth + 1)?;
    }
    Ok(())
}

fn mark(
    value: &serde_json::Value,
    sessions: &mut HashSet<String>,
    refs: &mut HashSet<(String, String)>,
) {
    if let Some(key) = value["key"].as_str() {
        sessions.insert(digest(key.as_bytes()));
    }
    if let (Some(scope), Some(id)) = (value["scope"].as_str(), value["publication_id"].as_str()) {
        refs.insert((scope.into(), id.into()));
    }
    match value {
        serde_json::Value::Object(map) => {
            for v in map.values() {
                mark(v, sessions, refs);
            }
        }
        serde_json::Value::Array(list) => {
            for v in list {
                mark(v, sessions, refs);
            }
        }
        _ => {}
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn verify_unix_handle(file: &std::fs::File, roots: &[PathBuf]) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    #[cfg(target_os = "linux")]
    let opened = fs_err::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    let opened = {
        use std::os::unix::ffi::OsStrExt;
        let mut name = [0u8; libc::PATH_MAX as usize];
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, name.as_mut_ptr()) } != 0 {
            return Err("Could not verify opened visual file".into());
        }
        let end = name
            .iter()
            .position(|byte| *byte == 0)
            .ok_or("Invalid opened visual path")?;
        PathBuf::from(std::ffi::OsStr::from_bytes(&name[..end]))
    };
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        // The kernel path names the opened inode. Resolving it again could follow
        // a replacement symlink and accidentally validate a different file.
        if !opened.is_absolute() || !roots.iter().any(|root| opened.starts_with(root)) {
            return Err("Opened visual file escaped its registered roots".into());
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (file, roots);
        Err("Opened-file verification is unsupported on this platform".into())
    }
}

#[cfg(test)]
mod review_regressions {
    use super::*;
    fn input(html: &str) -> Input {
        serde_json::from_value(serde_json::json!({"html":html,"title":"Snapshot"})).unwrap()
    }
    #[cfg(unix)]
    #[test]
    fn opened_descriptor_must_still_belong_to_a_registered_root() {
        let root = tempfile::tempdir().unwrap();
        let inside = tempfile::NamedTempFile::new_in(root.path()).unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let roots = [root.path().canonicalize().unwrap()];
        assert!(verify_unix_handle(inside.as_file(), &roots).is_ok());
        // This is the handle produced when an ancestor is swapped after canonicalization.
        assert!(verify_unix_handle(outside.as_file(), &roots).is_err());
    }
    #[test]
    fn forks_can_revise_inherited_visuals_without_mutating_the_parent() {
        let root = tempfile::tempdir().unwrap();
        let first = input("<html>Parent</html>");
        let reference = publish(
            root.path(),
            "parent",
            "tool",
            "turn",
            &first,
            first.html.clone().unwrap(),
        )
        .unwrap();
        let mut next = input("<html>Fork</html>");
        next.visual_id = Some(reference.visual_id.clone());
        assert!(publish(
            root.path(),
            "unrelated",
            "tool",
            "turn",
            &next,
            next.html.clone().unwrap()
        )
        .is_err());
        let fork = publish_with_references(
            root.path(),
            "fork",
            "new-tool",
            "new-turn",
            &next,
            next.html.clone().unwrap(),
            std::slice::from_ref(&reference),
        )
        .unwrap();
        assert_eq!(fork.visual_id, reference.visual_id);
        assert_ne!(fork.scope, reference.scope);
        assert_eq!(
            read_in(root.path(), &reference).unwrap(),
            "<html>Parent</html>"
        );
        assert_eq!(read_in(root.path(), &fork).unwrap(), "<html>Fork</html>");
    }
    #[test]
    fn cancellation_at_commit_leaves_no_effect_and_the_same_request_can_retry() {
        let root = tempfile::tempdir().unwrap();
        let input = input("<html>Cancelled</html>");
        let checks = std::cell::Cell::new(0);
        let result = publish_cancellable(
            root.path(),
            ("session", "tool", "turn"),
            &input,
            input.html.clone().unwrap(),
            &[],
            || {
                checks.set(checks.get() + 1);
                checks.get() >= 2
            },
        );
        assert!(result.is_err());
        assert_eq!(checks.get(), 2);
        assert_eq!(
            fs_err::read_dir(root.path().join(digest(b"session")))
                .unwrap()
                .count(),
            0
        );
        let reference = publish(
            root.path(),
            "session",
            "tool",
            "turn",
            &input,
            input.html.clone().unwrap(),
        )
        .unwrap();
        assert_eq!(
            read_in(root.path(), &reference).unwrap(),
            "<html>Cancelled</html>"
        );
        assert_eq!(
            fs_err::read_dir(root.path().join(digest(b"session")))
                .unwrap()
                .count(),
            1
        );
    }
}
