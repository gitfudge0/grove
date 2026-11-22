//! Self-contained transcript documents. A surface lease follows GPUI element
//! lifetime; the retained document follows session lifetime, never streaming text.

mod session;
#[cfg(test)]
use session::{browser_link_label, output_html_link};
mod native_surface;
#[cfg(all(test, target_os = "macos"))]
use native_surface::recovery_action;
#[cfg(test)]
use native_surface::{surface_is_selected, NativeElement};

mod document;

use document::{read_document, MAX_HTML_BYTES};

use super::status::{self, StatusKind};
use super::{components::transcript_action, rpx, tokens::*};
use crate::{
    platform::inline_html::{LoadStatus, NativeDocument},
    theme as c,
};
use gpui::{
    div, prelude::*, AnyElement, App, Bounds, Context, ElementId, Entity, EntityId, FocusHandle,
    GlobalElementId, LayoutId, Pixels, Point, ScrollHandle, SharedString, WeakEntity, Window,
};
use gpui_component::{
    text::{markdown_ast, MarkdownExtensions, MarkdownNode, MarkdownParseContext, TextView},
    Disableable, Selectable,
};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::OnceLock,
    time::Duration,
};

const CONTEXT_VEIL_OPACITY: f32 = 0.14;
const INLINE_HEIGHT: f32 = 360.0;
// Leave room for a useful title before the attachment actions wrap.
const ARTIFACT_IDENTITY_BASIS: f32 = 220.0;
// Bound adversarial or viewport-dependent reports within the transcript.
const REPORT_MAX_HEIGHT: f32 = INLINE_HEIGHT * 8.;
const REPORT_MARKER: &str = "<!-- grove:integrated-report -->";
const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const CSP: &str = "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval'; style-src 'unsafe-inline'; img-src data:; font-src data:; media-src data:; connect-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'";

/// Reports opt in with an exact marker; ordinary HTML keeps its own styling.
fn integrated_report(source: &str) -> bool {
    source.contains(REPORT_MARKER)
}

/// Host theme as CSS custom properties, replaced in place on every change so
/// appearance and zoom follow the app without reloading the document.
fn report_script(window: &Window, expanded: bool) -> String {
    fn color(value: gpui::Hsla) -> String {
        format!(
            "hsl({} {}% {}% / {})",
            value.h * 360.,
            value.s * 100.,
            value.l * 100.,
            value.a
        )
    }
    let px = |value: f32| f32::from(rpx(value).to_pixels(window.rem_size()));
    let css = format!(
        ":root{{--grove-bg:{};--grove-fg:{};--grove-muted:{};--grove-surface:{};\
         --grove-border:{};--grove-info:{};--grove-success:{};--grove-warning:{};\
         --grove-body:{}px;--grove-title:{}px;--grove-display:{}px;--grove-gap:{}px;\
         --grove-section:{}px;--grove-radius:{}px;--grove-font:'{}'}}\
         html{{background:var(--grove-bg);color:var(--grove-fg);font-family:var(--grove-font);\
         font-size:var(--grove-body);overflow:{}}}body{{margin:0;display:flow-root}}",
        color(c::BG()),
        color(c::FG()),
        color(c::FG_DIM()),
        color(c::BG_HOVER()),
        color(c::BORDER()),
        color(c::BLUE()),
        color(c::GREEN()),
        color(c::AMBER()),
        px(TEXT_BODY),
        px(TEXT_DISPLAY),
        px(TEXT_DISPLAY_LG),
        px(SPACE_LG),
        px(SPACE_20),
        px(RADIUS_GROUP),
        crate::fonts::UI_FAMILY,
        if expanded { "auto" } else { "hidden" },
    );
    format!(
        "(()=>{{let s=document.getElementById('grove-report-theme');if(!s){{s=document.createElement('style');s.id='grove-report-theme';document.head.append(s)}}s.textContent={};}})()",
        serde_json::Value::String(css)
    )
}

#[derive(Clone, Debug)]
struct DocumentBlock {
    source: String,
    markdown: String,
    offset: usize,
}

fn closed_fence(source: &str) -> bool {
    let mut lines = source.lines();
    let Some(open) = lines.next().map(str::trim_start) else {
        return false;
    };
    let Some(marker @ ('`' | '~')) = open.chars().next() else {
        return false;
    };
    let length = open.chars().take_while(|ch| *ch == marker).count();
    length >= 3
        && lines.last().is_some_and(|line| {
            let close = line.trim();
            close.len() >= length && close.chars().all(|ch| ch == marker)
        })
}

/// Agents often lead with the report marker, so a BOM and comments may precede
/// the doctype.
fn complete_document(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    let mut start = lower.trim_start_matches('\u{feff}').trim_start();
    while let Some(rest) = start.strip_prefix("<!--") {
        let Some(end) = rest.find("-->") else {
            return false;
        };
        start = rest[end + 3..].trim_start();
    }
    source.len() <= MAX_HTML_BYTES
        && (start.starts_with("<!doctype html") || start.starts_with("<html"))
        && lower.contains("</html>")
}

fn parse(node: &markdown_ast::Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
    let markdown_ast::Node::Code(code) = node else {
        return None;
    };
    // Untagged fences qualify too; the complete-document check still applies.
    if !code
        .lang
        .as_deref()
        .is_none_or(|lang| lang.is_empty() || lang.eq_ignore_ascii_case("html"))
    {
        return None;
    }
    let markdown = cx.node_source(node)?;
    if !closed_fence(markdown) || !complete_document(&code.value) {
        return None;
    }
    Some(
        MarkdownNode::new(
            "grove-html",
            DocumentBlock {
                source: code.value.clone(),
                markdown: markdown.to_owned(),
                offset: cx.offset() + code.position.as_ref()?.start.offset,
            },
        )
        .text(code.value.clone())
        .markdown(markdown.to_owned()),
    )
}

/// Data fonts stay inside the document CSP and work identically in WebKit/WebView2.
/// Encode once per process, never during a streaming transcript redraw.
fn document_font_css() -> &'static str {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    static CSS: OnceLock<String> = OnceLock::new();
    CSS.get_or_init(|| {
        let faces = [
            ("DMSans", crate::fonts::UI_FAMILY, "Regular", 400, "normal"),
            ("DMSans", crate::fonts::UI_FAMILY, "Medium", 500, "normal"),
            ("DMSans", crate::fonts::UI_FAMILY, "SemiBold", 600, "normal"),
            ("DMSans", crate::fonts::UI_FAMILY, "Bold", 700, "normal"),
            ("JetBrainsMono", crate::fonts::MONO_FAMILY, "Regular", 400, "normal"),
            ("JetBrainsMono", crate::fonts::MONO_FAMILY, "Medium", 500, "normal"),
            ("JetBrainsMono", crate::fonts::MONO_FAMILY, "SemiBold", 600, "normal"),
            ("JetBrainsMono", crate::fonts::MONO_FAMILY, "Bold", 700, "normal"),
            ("JetBrainsMono", crate::fonts::MONO_FAMILY, "Italic", 400, "italic"),
            ("JetBrainsMono", crate::fonts::MONO_FAMILY, "BoldItalic", 700, "italic"),
        ];
        let mut css: String = faces.into_iter().filter_map(|(file, family, name, weight, style)| {
            let path = format!("fonts/{file}-{name}.ttf");
            let font = crate::assets::Assets::get(&path)?;
            Some(format!("@font-face{{font-family:'{family}';font-style:{style};font-weight:{weight};font-display:swap;src:url(data:font/ttf;base64,{}) format('truetype')}}", STANDARD.encode(font.data)))
        }).collect();
        css.push_str("*::before,*::after{font-family:inherit!important}");
        css
    })
}

/// Normalize authored font choices while retaining semantic and explicitly authored
/// monospace text. Observe streaming/script changes without replacing the document.
fn document_font_script() -> &'static str {
    r#"(()=>{
const familyFor=value=>/monospace|JetBrains Mono|Courier|Consolas|Menlo|Monaco|BlexMono/i.test(value)?'JetBrains Mono':'DM Sans';
const originals=new WeakMap();
const normalize=root=>{
 const nodes=root.nodeType===1?[root,...root.querySelectorAll('*')]:[...root.querySelectorAll('*')];
 for(const el of nodes){
  const current=el.style.getPropertyValue('font-family');
  if(!originals.has(el)||(current!=='"DM Sans"'&&current!=='"JetBrains Mono"'))originals.set(el,[current,el.style.getPropertyPriority('font-family')]);
  const [value,priority]=originals.get(el);
  if(value)el.style.setProperty('font-family',value,priority);else el.style.removeProperty('font-family');
 }
 // Read before writing: children must retain their authored inherited monospace role.
 const choices=nodes.map(el=>{
  const family=getComputedStyle(el).fontFamily;
  const mono=el.closest('pre,code,kbd,samp,tt')!==null||familyFor(family)==='JetBrains Mono';
  return [el,mono?'JetBrains Mono':'DM Sans'];
 });
 for(const [el,family] of choices)if(el.style.getPropertyValue('font-family')!==`"${family}"`||el.style.getPropertyPriority('font-family')!=='important')el.style.setProperty('font-family',`"${family}"`,'important');
};
const start=()=>{
 normalize(document);
 const observer=new MutationObserver(records=>{
  observer.disconnect();
  for(const record of records){
   if(record.type==='childList'){
    if(record.target.nodeType===1&&record.target.matches('style'))normalize(document);
    else for(const node of record.addedNodes)if(node.nodeType===1)normalize(node);
   }
   if(record.type==='attributes')normalize(record.target);
  }
  observe();
 });
 const observe=()=>observer.observe(document.documentElement,{subtree:true,childList:true,attributes:true,attributeFilter:['style','class']});
 observe();
};
if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',start,{once:true});else start();
})()"#
}

/// App preference reaches ordinary HTML as well as integrated reports. CSS
/// suppresses author animation/transition effects; matchMedia and an event let
/// scripts react without replacing the document or losing input state.
fn preference_script(reduced: bool) -> String {
    format!(
        r"(()=>{{
const reduced={reduced};
if(!window.__groveMotion){{
 const native=window.matchMedia.bind(window), queries=new Map();
 const state=window.__groveMotion={{reduced:false,queries}};
 const rewrite=query=>state.reduced?query.replace(/\(prefers-reduced-motion:\s*(reduce|no-preference)\)/ig,(_,value)=>value.toLowerCase()==='reduce'?'(min-width: 0px)':'(width < 0px)'):query;
 window.matchMedia=query=>{{
  query=String(query);
  if(!/\(prefers-reduced-motion:\s*(reduce|no-preference)\)/i.test(query))return native(query);
  if(queries.has(query))return queries.get(query).result;
  const result=new EventTarget();let current=native(rewrite(query));result.onchange=null;
  Object.defineProperties(result,{{media:{{value:query}},matches:{{get:()=>current.matches}}}});
  const notify=()=>{{const e=new MediaQueryListEvent('change',{{matches:result.matches,media:query}});result.dispatchEvent(e);result.onchange?.(e)}};
  const refresh=()=>{{const before=current.matches;current.removeEventListener('change',notify);current=native(rewrite(query));current.addEventListener('change',notify);if(before!==current.matches)notify()}};
  result.addListener=listener=>result.addEventListener('change',listener);
  result.removeListener=listener=>result.removeEventListener('change',listener);
  current.addEventListener('change',notify);queries.set(query,{{result,refresh}});return result;
 }};
}}
const state=window.__groveMotion,changed=state.reduced!==reduced;state.reduced=reduced;
let style=document.getElementById('grove-motion');if(!style){{style=document.createElement('style');style.id='grove-motion';document.head.append(style)}}
style.textContent=reduced?'*,*::before,*::after{{animation:none!important;transition:none!important;scroll-behavior:auto!important}}':'';
document.documentElement.dataset.groveReduceMotion=String(reduced);
if(reduced)document.getAnimations().forEach(animation=>animation.cancel());
if(changed){{for(const query of state.queries.values())query.refresh();window.dispatchEvent(new CustomEvent('grove:preferences',{{detail:{{reduceMotion:reduced}}}}))}}
}})()"
    )
}

fn document_preferences_script(
    window: &Window,
    report: bool,
    expanded: bool,
    reduced: bool,
) -> String {
    let mut script = preference_script(reduced);
    if report {
        script.push(';');
        script.push_str(&report_script(window, expanded));
    }
    script
}

fn sandboxed(source: &str, reduced: bool) -> String {
    // The first policy is parsed before ANY author bytes, including malformed
    // heads, base tags and author policies. Subsequent CSP policies only tighten it.
    format!(
        "<!doctype html><meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\"><style>{}</style><script>{}</script><script>{}</script>{source}",
        document_font_css(), preference_script(reduced), document_font_script()
    )
}

/// File links keep the app's existing absolute/file URL interpretation; relative
/// paths resolve against the session checkout. Query/fragment never enter disk IO.
fn local_html_url(target: &str, cwd: &Path) -> Option<gpui::http_client::Url> {
    use gpui::http_client::Url;
    if target.starts_with("//") {
        return None;
    }
    let url = if target.starts_with('/') {
        Url::parse(&format!("file://{target}")).ok()?
    } else if target.starts_with("file:") {
        Url::parse(target).ok()?
    } else if Url::parse(target).is_ok() {
        return None;
    } else {
        Url::from_directory_path(cwd).ok()?.join(target).ok()?
    };
    if url.scheme() != "file" {
        return None;
    }
    let path = url.to_file_path().ok()?;
    let extension = path.extension()?.to_str()?;
    (extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("htm")).then_some(url)
}

fn local_html_path(target: &str, cwd: &Path) -> Option<PathBuf> {
    local_html_url(target, cwd)?.to_file_path().ok()
}

pub(super) fn recovery_path(path: &Path, cwd: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    let mut roots = roots.to_vec();
    roots.push(cwd.to_owned());
    grove_core::rich_plan::safe_path(cwd, &path.to_string_lossy(), &roots).ok()
}

fn artifact_size(bytes: u64, rejected_limit: Option<u64>) -> String {
    let value = bytes as f64 / (1024. * 1024.);
    if rejected_limit.is_some_and(|limit| {
        bytes > limit && format!("{value:.1}") == format!("{:.1}", limit as f64 / (1024. * 1024.))
    }) {
        return format!("{bytes} bytes");
    }
    if bytes.is_multiple_of(1024 * 1024) {
        format!("{value:.0} MiB")
    } else {
        format!("{value:.1} MiB")
    }
}

pub(super) fn oversized_card(
    title: &str,
    limit: &grove_core::rich_plan::FileSizeLimit,
    description: &str,
    actions: Vec<AnyElement>,
    _cx: &App,
) -> gpui::Div {
    div()
        .debug_selector(|| "oversized-artifact-card".into())
        .w_full()
        .min_w_0()
        .flex()
        .items_start()
        .gap(rpx(SPACE_LG))
        .p(rpx(SPACE_2XL))
        .bg(c::BG_RAIL())
        .border_1()
        .border_color(c::BORDER())
        .rounded(rpx(RADIUS_GROUP))
        .child(crate::icons::icon("status-warning", ICON_MD, c::AMBER()).flex_shrink_0())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_SM))
                .child(
                    div()
                        .text_size(rpx(TEXT_BODY))
                        .text_color(c::FG())
                        .child(title.to_owned()),
                )
                .child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(format!(
                            "{} · {} · limit {}",
                            limit.path.file_name().unwrap_or_default().to_string_lossy(),
                            artifact_size(limit.bytes, Some(limit.limit)),
                            artifact_size(limit.limit, None)
                        )),
                )
                .child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG())
                        .child(description.to_owned()),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(rpx(SPACE_SM))
                        .children(actions),
                ),
        )
}

#[derive(Default)]
pub(super) struct HtmlUi {
    expanded: Option<WeakEntity<HtmlArtifact>>,
    owner: Option<EntityId>,
    selected: Option<EntityId>,
    blocked: bool,
    exclusion: Option<Bounds<Pixels>>,
}
impl gpui::Global for HtmlUi {}

pub(super) fn set_context(selected: Option<EntityId>, blocked: bool, cx: &mut App) {
    if !cx.has_global::<HtmlUi>() {
        cx.set_global(HtmlUi::default());
    }
    let ui = cx.global_mut::<HtmlUi>();
    if ui.blocked != blocked || ui.selected != selected {
        NativeDocument::hide_all();
    }
    if ui.owner.is_some() && ui.owner != selected {
        ui.expanded = None;
        ui.owner = None;
    }
    if ui.selected != selected {
        ui.exclusion = None;
    }
    ui.selected = selected;
    ui.blocked = blocked;
}

// The native host has a single exclusion rectangle. Intersect first so an
// unrelated sidebar tooltip cannot enlarge a panel's hole in this document.
fn merge_exclusions(
    first: Option<Bounds<Pixels>>,
    second: Option<Bounds<Pixels>>,
    clip: Bounds<Pixels>,
) -> Option<Bounds<Pixels>> {
    [first, second]
        .into_iter()
        .flatten()
        .filter_map(|bounds| {
            let bounds = bounds.intersect(&clip);
            (bounds.size.width > Pixels::ZERO && bounds.size.height > Pixels::ZERO)
                .then_some(bounds)
        })
        .reduce(|a, b| {
            Bounds::from_corners(
                a.origin.min(&b.origin),
                a.bottom_right().max(&b.bottom_right()),
            )
        })
}

// Populated while rendering the preview, before any native surface is painted.
pub(super) fn exclude_native_html(bounds: Option<Bounds<Pixels>>, cx: &mut App) {
    if !cx.has_global::<HtmlUi>() {
        cx.set_global(HtmlUi::default());
    }
    cx.global_mut::<HtmlUi>().exclusion = bounds;
}

#[cfg(test)]
pub(super) fn native_exclusion(cx: &App) -> Option<Bounds<Pixels>> {
    cx.try_global::<HtmlUi>().and_then(|ui| ui.exclusion)
}

fn expanded(artifact: &Entity<HtmlArtifact>, cx: &App) -> bool {
    cx.try_global::<HtmlUi>()
        .and_then(|ui| ui.expanded.as_ref())
        .is_some_and(|weak| weak.entity_id() == artifact.entity_id())
}

pub(super) fn dismiss_expanded(window: &mut Window, cx: &mut App) -> bool {
    let Some(ui) = cx.try_global::<HtmlUi>() else {
        return false;
    };
    if ui.blocked {
        return false;
    }
    let Some(artifact) = ui.expanded.as_ref().and_then(gpui::WeakEntity::upgrade) else {
        return false;
    };
    artifact.update(cx, |artifact, cx| artifact.toggle_expand(window, cx));
    true
}

pub(super) fn render_expanded(_window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let artifact = cx.try_global::<HtmlUi>()?.expanded.as_ref()?.upgrade()?;
    let blocked = cx.global::<HtmlUi>().blocked;
    if blocked {
        return None;
    }
    Some(
        div()
            .id("html-expanded-overlay")
            .debug_selector(|| "html-expanded-overlay".into())
            .absolute()
            .inset_0()
            .occlude()
            .bg(c::alpha(c::SCRIM(), CONTEXT_VEIL_OPACITY))
            .p(rpx(SPACE_3XL))
            .child(div().size_full().min_h_0().child(artifact))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .into_any_element(),
    )
}

pub(super) struct OpenLocalFile(pub super::file_browser::FileTarget);
impl gpui::EventEmitter<OpenLocalFile> for HtmlSession {}

pub(super) struct HtmlSession {
    legacy: bool,
    artifacts: HashMap<String, Entity<HtmlArtifact>>,
    extensions: HashMap<String, MarkdownExtensions>,
    file_focus: HashMap<(String, String), FocusHandle>,
    browser_errors: HashMap<(String, String), String>,
    opened: HashMap<String, Vec<String>>,
    cwd: PathBuf,
    roots: Vec<PathBuf>,
    scroll: ScrollHandle,
    paused: Rc<Cell<bool>>,
    auto: AutoOpen,
    identity: Option<(Option<String>, u64)>,
}

#[derive(Default)]
struct AutoOpen {
    observed: bool,
    completed_messages: HashSet<String>,
}
impl AutoOpen {
    fn observe(
        &mut self,
        busy: bool,
        done: bool,
        _turn: Option<&str>,
        final_message: Option<(&str, &str)>,
    ) -> Option<(String, String)> {
        // Establish a restoration baseline once. Subsequent successful terminal
        // snapshots identify newly completed output even if a fast run's busy
        // notification was coalesced before this view could render it.
        if !std::mem::replace(&mut self.observed, true) {
            if !busy {
                if let Some((id, _)) = final_message {
                    self.completed_messages.insert(id.to_owned());
                }
            }
            return None;
        }
        if busy || !done {
            return None;
        }
        let (id, text) = final_message?;
        self.completed_messages
            .insert(id.to_owned())
            .then(|| (id.to_owned(), text.to_owned()))
    }
}

fn source_view(markdown: &str, id: usize, window: &Window) -> AnyElement {
    TextView::markdown(("html-source", id), markdown.to_owned())
        .style(super::codex_view::markdown_style(window))
        .selectable(true)
        .into_any_element()
}

fn inline_card(artifact: Entity<HtmlArtifact>, _window: &mut Window, cx: &mut App) -> AnyElement {
    if expanded(&artifact, cx) {
        div()
            .h(rpx(INLINE_HEIGHT))
            .w_full()
            .p(rpx(SPACE_2XL))
            .text_color(c::FG_DIM())
            .child("HTML document expanded · use Return to conversation above")
            .into_any_element()
    } else {
        artifact.into_any_element()
    }
}

#[derive(Clone)]
struct Version {
    active: String,
    markdown: String,
    pending: Option<(String, String)>,
}
impl Version {
    fn offer(&mut self, source: String, markdown: String) -> bool {
        let pending = (source != self.active).then_some((source, markdown));
        if self.pending == pending {
            return false;
        }
        self.pending = pending;
        true
    }
    fn adopt(&mut self) -> bool {
        let Some((source, markdown)) = self.pending.take() else {
            return false;
        };
        self.active = source;
        self.markdown = markdown;
        true
    }
}

struct Surface {
    initialized: Cell<bool>,
    native: NativeDocument,
    generation: Cell<u64>,
}
struct SurfaceLease {
    surface: Rc<Surface>,
    generation: u64,
}
impl Drop for SurfaceLease {
    fn drop(&mut self) {
        if self.surface.generation.get() == self.generation {
            self.surface.native.retire_frame();
        }
    }
}

struct HtmlArtifact {
    publication: Option<grove_core::visuals::Reference>,
    publication_failed: bool,
    version: Version,
    path: Option<PathBuf>,
    owner: EntityId,
    session: WeakEntity<HtmlSession>,
    paused: Rc<Cell<bool>>,
    surface: Option<Rc<Surface>>,
    error: Option<String>,
    size_limit: Option<grove_core::rich_plan::FileSizeLimit>,
    recovery_error: Option<String>,
    loading_file: bool,
    source: bool,
    started: Option<std::time::Instant>,
    last_status: Option<LoadStatus>,
    poll: Option<gpui::Task<()>>,
    scroll: ScrollHandle,
    return_offset: Option<Point<Pixels>>,
    expand_focus: FocusHandle,
    return_focus: FocusHandle,
    expanded_focus: FocusHandle,
    external_return_focus: Option<FocusHandle>,
    anchor: gpui::ScrollAnchor,
    reveal: bool,
    report: bool,
    report_height: f32,
    report_theme: String,
}
impl HtmlArtifact {
    fn new(
        source: String,
        markdown: String,
        path: Option<PathBuf>,
        owner: WeakEntity<HtmlSession>,
        paused: Rc<Cell<bool>>,
        scroll: ScrollHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let anchor = gpui::ScrollAnchor::for_handle(scroll.clone());
        let mut artifact = Self {
            publication: None,
            publication_failed: false,
            anchor,
            reveal: false,
            report: false,
            report_height: INLINE_HEIGHT,
            report_theme: String::new(),
            version: Version {
                active: source,
                markdown,
                pending: None,
            },
            path,
            owner: owner.entity_id(),
            session: owner,
            paused,
            surface: None,
            error: None,
            size_limit: None,
            recovery_error: None,
            loading_file: false,
            source: false,
            started: None,
            last_status: None,
            poll: None,
            scroll,
            return_offset: None,
            expand_focus: cx.focus_handle().tab_stop(true),
            return_focus: cx.focus_handle().tab_stop(true),
            expanded_focus: cx.focus_handle(),
            external_return_focus: None,
        };
        if artifact.path.is_none() && artifact.version.active.is_empty() {
            // Published bytes load asynchronously; do not create an empty native document.
        } else if artifact.path.is_some() {
            artifact.read_file(cx);
        } else {
            artifact.load(cx);
        }
        artifact
    }
    fn read_publication(&mut self, cx: &mut Context<Self>) {
        let Some(reference) = self.publication.clone() else {
            return;
        };
        if self.loading_file {
            return;
        }
        self.surface = None;
        self.error = None;
        self.loading_file = true;
        let read = cx
            .background_executor()
            .spawn(async move { grove_core::visuals::read(&reference) });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |this, cx| {
                this.loading_file = false;
                match result {
                    Ok(source) => {
                        this.version.markdown = format!("```html\n{source}\n```");
                        this.version.active = source;
                        this.load(cx);
                    }
                    Err(error) => {
                        this.publication_failed = true;
                        this.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn read_file(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if self.loading_file {
            return;
        }
        self.loading_file = true;
        self.error = None;
        self.size_limit = None;
        let read = cx
            .background_executor()
            .spawn(async move { read_document(&path) });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |this, cx| {
                this.loading_file = false;
                match result {
                    Ok(source) if this.version.active.is_empty() => {
                        this.version.markdown = format!("```html\n{source}\n```");
                        this.version.active = source;
                        this.load(cx);
                    }
                    Ok(source) => {
                        let markdown = format!("```html\n{source}\n```");
                        this.version.offer(source, markdown);
                    }
                    Err(error) => {
                        this.size_limit = error.size_limit;
                        this.error = Some(error.message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        self.surface = None;
        self.size_limit = None;
        self.report = integrated_report(&self.version.active);
        self.report_height = INLINE_HEIGHT;
        self.report_theme.clear();
        self.error = None;
        self.poll = None;
        self.last_status = None;
        self.started = None;
        if self.version.active.len()
            > if self.publication.is_some() {
                grove_core::visuals::BUNDLE_LIMIT
            } else {
                MAX_HTML_BYTES
            }
        {
            self.error = Some("HTML document exceeds the preview limit".into());
            return;
        }
        if self.publication.is_some() && !expanded(&cx.entity(), cx) {
            return;
        }
        let Some(native) =
            NativeDocument::new(&sandboxed(&self.version.active, cx.reduce_motion()))
        else {
            self.error = Some("Native HTML preview is unavailable".into());
            return;
        };
        self.surface = Some(Rc::new(Surface {
            initialized: Cell::new(false),
            native,
            generation: Cell::new(0),
        }));
        self.poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            let Ok(alive) = this.update(cx, |this, cx| {
                let status = this.surface.as_ref().map(|surface| surface.native.status());
                if status == Some(LoadStatus::Failed) {
                    this.error = Some(
                        this.surface
                            .as_ref()
                            .and_then(|surface| surface.native.failure_reason())
                            .unwrap_or_else(|| "Couldn’t load this preview".into()),
                    );
                    this.surface = None;
                }
                if status == Some(LoadStatus::Loading)
                    && this
                        .started
                        .is_some_and(|time| time.elapsed() >= LOAD_TIMEOUT)
                {
                    this.error = Some("Preview loading timed out".into());
                    this.surface = None;
                }
                if this.report && status == Some(LoadStatus::Ready) {
                    if let Some(height) =
                        this.surface.as_ref().and_then(|s| s.native.report_height())
                    {
                        if (height - this.report_height).abs() >= 1. {
                            this.report_height = height;
                            cx.notify();
                        }
                    }
                }
                if status != this.last_status {
                    this.last_status = status;
                    cx.notify();
                }
                this.surface.is_some()
            }) else {
                break;
            };
            if !alive {
                break;
            }
        }));
    }
    fn toggle_expand(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !cx.has_global::<HtmlUi>() {
            cx.set_global(HtmlUi::default());
        }
        NativeDocument::hide_all();
        if expanded(&cx.entity(), cx) {
            cx.global_mut::<HtmlUi>().expanded = None;
            cx.global_mut::<HtmlUi>().owner = None;
            if let Some(offset) = self.return_offset.take() {
                self.scroll.set_offset(offset);
            }
            self.external_return_focus
                .take()
                .unwrap_or_else(|| self.expand_focus.clone())
                .focus(window, cx);
        } else {
            self.return_offset = Some(self.scroll.offset());
            let entity = cx.entity().downgrade();
            let ui = cx.global_mut::<HtmlUi>();
            ui.expanded = Some(entity);
            ui.owner = Some(self.owner);
            if self.publication.is_some()
                && self.surface.is_none()
                && !self.loading_file
                && !self.publication_failed
            {
                self.load(cx);
            }
            self.expanded_focus.focus(window, cx);
            // The expanded header is deferred and has no focus path until paint.
            let owner = cx.entity().downgrade();
            window.on_next_frame(move |window, cx| {
                let _ = owner.update(cx, |this, cx| {
                    if expanded(&cx.entity(), cx) && !cx.global::<HtmlUi>().blocked {
                        NativeDocument::focus_host(window);
                        this.expanded_focus.focus(window, cx);
                    }
                });
            });
        }
        let current = cx.entity_id();
        let _ = self.session.update(cx, |session, cx| {
            for artifact in session
                .artifacts
                .values()
                .filter(|artifact| artifact.entity_id() != current)
            {
                artifact.update(cx, |_, cx| cx.notify());
            }
        });
        cx.notify();
        cx.refresh_windows();
    }
}

#[cfg(test)]
mod tests;

// A capture owns an independent native document, never a live transcript surface.
struct PendingCapture {
    request: grove_core::visual_preview::Request,
    native: crate::platform::visual_capture::Capture,
    started: std::time::Instant,
}
thread_local! {
    static CAPTURE: std::cell::RefCell<Option<PendingCapture>> = const { std::cell::RefCell::new(None) };
}
pub(crate) fn capture_pending() -> bool {
    CAPTURE.with(|slot| slot.borrow().is_some())
}

pub(crate) fn pump_capture(window: &Window, cx: &App) {
    use base64::Engine;
    use std::sync::atomic::Ordering;
    CAPTURE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            if let Some(request) = grove_core::visual_preview::next() {
                if request.cancelled.load(Ordering::Relaxed) { return; }
                // Same CSP, fonts, theme and reduced-motion policy as the displayed document.
                let diagnostics = r"<script>(()=>{const messages=[];Object.defineProperty(globalThis,'__groveCaptureConsole',{value:messages});for(const level of ['log','warn','error']){const original=console[level];console[level]=(...args)=>{if(messages.length<32)messages.push({level,text:args.map(x=>{try{return String(x)}catch{return '[unreadable]'}}).join(' ').slice(0,512)});original.apply(console,args)}}addEventListener('error',e=>{if(messages.length<32)messages.push({level:'error',text:String(e.message).slice(0,512)})})})();</script>";
                let theme = document_preferences_script(window, true, false, cx.reduce_motion()).replace("</script", "<\\/script");
                // WebView2 installs its CSS viewport asynchronously after navigation.
                // Its capture backend activates the initial frame after that override.
                let activation = if cfg!(target_os = "windows") { "" } else {
                    "(()=>{const activate=()=>window.dispatchEvent(new Event('grove:activate-view'));if(document.readyState==='complete')activate();else addEventListener('load',activate,{once:true});})();"
                };
                let source = format!("{}<script>{theme};{activation}</script>", sandboxed(&format!("{diagnostics}{}", request.html), cx.reduce_motion()));
                match crate::platform::visual_capture::Capture::new(&source, request.width, request.height) {
                    Ok(native) => *slot = Some(PendingCapture { request, native, started: std::time::Instant::now() }),
                    Err(error) => { let _ = request.reply.send(Err(error)); },
                }
            }
        }
        let Some(pending) = slot.as_ref() else { return; };
        if pending.request.cancelled.load(Ordering::Relaxed) || pending.started.elapsed() > Duration::from_secs(18) {
            let _ = pending.request.reply.send(Err("Native capture cancelled or timed out waiting for load/fonts/layout".into()));
            slot.take(); return;
        }
        // Give layout and paint a bounded settling interval after native creation.
        if pending.started.elapsed() < Duration::from_millis(250) { return; }
        if let Some(result) = pending.native.poll() {
            let result = result.map(|(png, metadata)| {
                let pixel_width = png.get(16..20).and_then(|b| b.try_into().ok()).map(u32::from_be_bytes);
                let pixel_height = png.get(20..24).and_then(|b| b.try_into().ok()).map(u32::from_be_bytes);
                serde_json::json!({
                    "png":base64::engine::general_purpose::STANDARD.encode(png),
                    "width":pending.request.width,"capturedHeight":pending.request.height,
                    "pixelWidth":pixel_width,"pixelHeight":pixel_height,
                    "contentHeight":metadata["contentHeight"],"consoleMessages":metadata["consoleMessages"],
                    "source_digest":pending.request.digest
                })
            });
            let _ = pending.request.reply.send(result);
            slot.take();
        }
    });
}
