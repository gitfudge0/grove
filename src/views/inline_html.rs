//! Self-contained transcript documents. A surface lease follows GPUI element
//! lifetime; the retained document follows session lifetime, never streaming text.
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
    io::Read,
    path::{Path, PathBuf},
    rc::Rc,
    sync::OnceLock,
    time::Duration,
};

const CONTEXT_VEIL_OPACITY: f32 = 0.14;
const INLINE_HEIGHT: f32 = 360.0;
// Bound adversarial or viewport-dependent reports within the transcript.
const REPORT_MAX_HEIGHT: f32 = INLINE_HEIGHT * 8.;
const REPORT_MARKER: &str = "<!-- grove:integrated-report -->";
const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
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
         --grove-section:{}px;--grove-radius:{}px;--grove-font:'{}',system-ui,sans-serif}}\
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

fn complete_document(source: &str) -> bool {
    let lower = source.trim().to_ascii_lowercase();
    source.len() <= MAX_HTML_BYTES
        && (lower.starts_with("<!doctype html") || lower.starts_with("<html"))
        && lower.contains("</html>")
}

fn parse(node: &markdown_ast::Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
    let markdown_ast::Node::Code(code) = node else {
        return None;
    };
    if !code.lang.as_deref()?.eq_ignore_ascii_case("html") {
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
        [("Regular", 400), ("Medium", 500), ("SemiBold", 600), ("Bold", 700)]
            .into_iter().filter_map(|(name, weight)| {
                let font = crate::assets::Assets::get(&format!("fonts/Geist-{name}.ttf"))?;
                Some(format!("@font-face{{font-family:'{}';font-style:normal;font-weight:{weight};font-display:swap;src:url(data:font/ttf;base64,{}) format('truetype')}}", crate::fonts::UI_FAMILY, STANDARD.encode(font.data)))
            }).collect()
    })
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

fn sandboxed(source: &str, reduced: bool) -> String {
    // The first policy is parsed before ANY author bytes, including malformed
    // heads, base tags and author policies. Subsequent CSP policies only tighten it.
    format!(
        "<!doctype html><meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\"><style>{}</style><script>{}</script>{source}",
        document_font_css(), preference_script(reduced)
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

fn read_document(path: &Path) -> Result<String, String> {
    if !fs_err::metadata(path)
        .map_err(|e| format!("Couldn’t read HTML: {e}"))?
        .is_file()
    {
        return Err("HTML target must be a regular file".into());
    }
    let mut options = fs_err::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use fs_err::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("Couldn’t read HTML: {e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("HTML target must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take((MAX_HTML_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_HTML_BYTES {
        return Err("HTML document exceeds the 2 MiB preview limit".into());
    }
    String::from_utf8(bytes).map_err(|_| "HTML preview requires UTF-8 text".into())
}

#[derive(Default)]
pub(super) struct HtmlUi {
    expanded: Option<WeakEntity<HtmlArtifact>>,
    owner: Option<EntityId>,
    selected: Option<EntityId>,
    blocked: bool,
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
    ui.selected = selected;
    ui.blocked = blocked;
}

fn expanded(artifact: &Entity<HtmlArtifact>, cx: &App) -> bool {
    cx.try_global::<HtmlUi>()
        .and_then(|ui| ui.expanded.as_ref())
        .is_some_and(|weak| weak.entity_id() == artifact.entity_id())
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

pub(super) struct HtmlSession {
    artifacts: HashMap<String, Entity<HtmlArtifact>>,
    native_plans: HashMap<(String, String), WeakEntity<super::native_plan::NativePlan>>,
    extensions: HashMap<String, MarkdownExtensions>,
    opened: HashMap<String, Vec<String>>,
    cwd: PathBuf,
    scroll: ScrollHandle,
    paused: Rc<Cell<bool>>,
    browser_errors: HashMap<String, String>,
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

impl HtmlSession {
    #[cfg(test)]
    pub(super) fn artifact_count(&self) -> usize {
        self.artifacts.len()
    }
    #[cfg(test)]
    pub(super) fn opened_count(&self) -> usize {
        self.opened.values().map(Vec::len).sum()
    }
    #[cfg(test)]
    pub(super) fn artifact_ids(&self) -> HashSet<EntityId> {
        self.artifacts.values().map(Entity::entity_id).collect()
    }
    #[cfg(test)]
    pub(super) fn artifact_owners_match(&self, owner: EntityId, cx: &App) -> bool {
        self.artifacts
            .values()
            .all(|artifact| artifact.read(cx).owner == owner)
    }
    pub(super) fn new(cwd: PathBuf, scroll: ScrollHandle) -> Self {
        Self {
            artifacts: HashMap::new(),
            native_plans: HashMap::new(),
            extensions: HashMap::new(),
            opened: HashMap::new(),
            cwd,
            scroll,
            paused: Rc::default(),
            browser_errors: HashMap::new(),
            auto: AutoOpen::default(),
            identity: None,
        }
    }
    pub(super) fn sync(
        &mut self,
        state: &grove_core::codex::Snapshot,
        cwd: PathBuf,
        paused: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let identity = (state.saved.thread_id.clone(), state.cleared_conversations);
        if self
            .identity
            .as_ref()
            .is_some_and(|previous| previous != &identity)
        {
            self.artifacts.clear();
            self.extensions.clear();
            self.opened.clear();
            self.browser_errors.clear();
            self.auto = AutoOpen::default();
            if cx
                .try_global::<HtmlUi>()
                .is_some_and(|ui| ui.owner == Some(cx.entity_id()))
            {
                let ui = cx.global_mut::<HtmlUi>();
                ui.expanded = None;
                ui.owner = None;
                NativeDocument::hide_all();
                cx.refresh_windows();
            }
        }
        self.identity = Some(identity);
        self.cwd = cwd;
        if self.paused.replace(paused) != paused {
            for artifact in self.artifacts.values() {
                artifact.update(cx, |_, cx| cx.notify());
            }
        }
        let final_message = state
            .saved
            .items
            .iter()
            .rev()
            .find(|item| item["type"] == "agentMessage");
        let final_text = final_message.map(grove_core::codex::item_text);
        let final_item = final_message
            .and_then(|item| item["id"].as_str())
            .zip(final_text.as_deref());
        if let Some((id, text)) = self.auto.observe(
            state.status.busy(),
            state.status == grove_core::codex::Status::Done,
            state.turn_id.as_deref(),
            final_item,
        ) {
            if cx
                .try_global::<HtmlUi>()
                .is_none_or(|ui| ui.expanded.is_none())
            {
                if let Some(target) = output_html_link(&text, &self.cwd) {
                    self.open_link(&id, &target, window, cx);
                }
            }
        }
    }
    pub(super) fn bind_native_plans(
        &mut self,
        plans: HashMap<(String, String), WeakEntity<super::native_plan::NativePlan>>,
    ) {
        self.native_plans = plans;
    }

    pub(super) fn extensions(
        &mut self,
        message: &str,
        cx: &mut Context<Self>,
    ) -> MarkdownExtensions {
        if let Some(extensions) = self.extensions.get(message) {
            return extensions.clone();
        }
        let session = cx.entity().downgrade();
        let owner = message.to_owned();
        let links = session.clone();
        let link_owner = owner.clone();
        let extensions = super::mermaid::extensions()
            .block_parser(parse)
            .block_renderer("grove-html", move |node, window, cx| {
                let Some(block) = node.data::<DocumentBlock>() else {
                    return div().into_any_element();
                };
                session
                    .update(cx, |session, cx| {
                        let key = format!("fence:{owner}:{}", block.offset);
                        let session_owner = cx.entity().downgrade();
                        let artifact = session
                            .artifacts
                            .entry(key)
                            .or_insert_with(|| {
                                cx.new(|cx| {
                                    HtmlArtifact::new(
                                        block.source.clone(),
                                        block.markdown.clone(),
                                        None,
                                        session_owner,
                                        session.paused.clone(),
                                        session.scroll.clone(),
                                        cx,
                                    )
                                })
                            })
                            .clone();
                        artifact.update(cx, |artifact, cx| {
                            if artifact
                                .version
                                .offer(block.source.clone(), block.markdown.clone())
                            {
                                cx.notify();
                            }
                        });
                        inline_card(artifact, window, cx)
                    })
                    .unwrap_or_else(|_| source_view(&block.markdown, block.offset, window))
            })
            .link_renderer(move |target, label, window, cx| {
                links
                    .update(cx, |session, cx| {
                        if target.ends_with(".grove-plan.json") {
                            let plan = session
                                .native_plans
                                .get(&(link_owner.clone(), target.to_owned()))
                                .cloned();
                            return Some(
                                transcript_action(
                                    SharedString::from(format!("native-link-{target}")),
                                    label.to_owned(),
                                    false,
                                    window,
                                    cx,
                                )
                                .icon(
                                    gpui_component::Icon::default()
                                        .path("icons/external-link.svg")
                                        .size(rpx(ICON_SM)),
                                )
                                .disabled(plan.is_none())
                                .on_click(move |_, window, cx| {
                                    if let Some(plan) = &plan {
                                        let _ = plan.update(cx, |plan, cx| plan.reveal(window, cx));
                                    }
                                    cx.stop_propagation();
                                })
                                .into_any_element(),
                            );
                        }
                        local_html_path(target, &session.cwd)?;
                        let open = cx.entity().downgrade();
                        let message = link_owner.clone();
                        let target = target.to_owned();
                        let browser = local_html_url(&target, &session.cwd)?.to_string();
                        let browser_session = cx.entity().downgrade();
                        let browser_message = message.clone();
                        if browser_link_label(label) {
                            return Some(
                                div()
                                    .debug_selector(|| "html-browser-link-only".into())
                                    .child(
                                        transcript_action(
                                            SharedString::from(format!(
                                                "html-browser-{message}-{target}"
                                            )),
                                            label.to_owned(),
                                            false,
                                            window,
                                            cx,
                                        )
                                        .icon(
                                            gpui_component::Icon::default()
                                                .path("icons/external-link.svg")
                                                .size(rpx(ICON_SM)),
                                        )
                                        .on_click(
                                            move |_, _, cx| {
                                                cx.stop_propagation();
                                                let _ =
                                                    browser_session.update(cx, |session, cx| {
                                                        session.open_browser(
                                                            browser_message.clone(),
                                                            browser.clone(),
                                                            cx,
                                                        );
                                                    });
                                            },
                                        ),
                                    )
                                    .into_any_element(),
                            );
                        }
                        let key = format!("html-link-{message}-{target}");
                        Some(
                            div()
                                .id(SharedString::from(key.clone()))
                                .debug_selector(|| "html-preview-link".into())
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_XS))
                                .child(
                                    transcript_action(
                                        SharedString::from(format!("{key}-preview")),
                                        label.to_owned(),
                                        false,
                                        window,
                                        cx,
                                    )
                                    .icon(
                                        gpui_component::Icon::default()
                                            .path("icons/zen.svg")
                                            .size(rpx(ICON_SM)),
                                    )
                                    .on_click(
                                        move |_, window, cx| {
                                            cx.stop_propagation();
                                            let _ = open.update(cx, |session, cx| {
                                                session.open_link(&message, &target, window, cx);
                                            });
                                        },
                                    ),
                                )
                                .child(
                                    div().debug_selector(|| "html-open-browser".into()).child(
                                        transcript_action(
                                            SharedString::from(format!("{key}-browser")),
                                            "Open in browser",
                                            false,
                                            window,
                                            cx,
                                        )
                                        .icon_only()
                                        .child(crate::icons::icon(
                                            "external-link",
                                            ICON_MD,
                                            c::FG_DIM(),
                                        ))
                                        .tooltip("Open in browser")
                                        .on_click(
                                            move |_, _, cx| {
                                                cx.stop_propagation();
                                                let _ =
                                                    browser_session.update(cx, |session, cx| {
                                                        session.open_browser(
                                                            browser_message.clone(),
                                                            browser.clone(),
                                                            cx,
                                                        );
                                                    });
                                            },
                                        ),
                                    ),
                                )
                                .into_any_element(),
                        )
                    })
                    .ok()
                    .flatten()
            });
        self.extensions
            .insert(message.to_owned(), extensions.clone());
        extensions
    }

    /// Open an explicitly selected immutable plan asset in the existing sandbox.
    pub(super) fn open_snapshot(
        &mut self,
        key: String,
        source: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = cx.entity().downgrade();
        let artifact = self
            .artifacts
            .entry(key)
            .or_insert_with(|| {
                cx.new(|cx| {
                    HtmlArtifact::new(
                        source,
                        String::new(),
                        None,
                        owner,
                        self.paused.clone(),
                        self.scroll.clone(),
                        cx,
                    )
                })
            })
            .clone();
        let return_focus = window.focused(cx);
        // Expansion notifies sibling artifacts through their owning session.
        // Release this session update before that callback can borrow it again.
        window.defer(cx, move |window, cx| {
            artifact.update(cx, |artifact, cx| {
                artifact.external_return_focus = return_focus;
                if !expanded(&cx.entity(), cx) {
                    artifact.toggle_expand(window, cx);
                }
            });
        });
        cx.notify();
    }

    fn open_link(
        &mut self,
        message: &str,
        target: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = local_html_path(target, &self.cwd) else {
            return;
        };
        // A reference in a later reply owns a new source snapshot and native view.
        // Reopening this reply must never reread a successfully captured file.
        let key = format!("link:{message}:{}", path.display());
        let owner = cx.entity().downgrade();
        self.artifacts.entry(key.clone()).or_insert_with(|| {
            cx.new(|cx| {
                HtmlArtifact::new(
                    String::new(),
                    String::new(),
                    Some(path),
                    owner,
                    self.paused.clone(),
                    self.scroll.clone(),
                    cx,
                )
            })
        });
        if let Some(artifact) = self.artifacts.get(&key) {
            artifact.update(cx, |artifact, cx| {
                artifact.reveal = true;
                cx.notify();
            });
        }
        let opened = self.opened.entry(message.to_owned()).or_default();
        if !opened.contains(&key) {
            opened.push(key);
        }
        cx.notify();
    }
    fn open_browser(&mut self, message: String, url: String, cx: &mut Context<Self>) {
        self.browser_errors.remove(&message);
        cx.spawn(async move |this, cx| {
            let result = crate::platform::inline_html::open_in_browser(url).await;
            if let Err(error) = result {
                let _ = this.update(cx, |this, cx| {
                    this.browser_errors
                        .insert(message, format!("Couldn’t open in browser: {error}"));
                    cx.notify();
                });
            }
        })
        .detach();
    }
    pub(super) fn linked(
        &self,
        message: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        let mut cards: Vec<_> = self
            .opened
            .get(message)
            .into_iter()
            .flatten()
            .filter_map(|key| self.artifacts.get(key))
            .map(|artifact| inline_card(artifact.clone(), window, cx))
            .collect();
        if let Some(error) = self.browser_errors.get(message) {
            cards.push(
                div()
                    .p(rpx(SPACE_LG))
                    .text_color(c::RED())
                    .child(error.clone())
                    .into_any_element(),
            );
        }
        cards
    }
}

fn browser_link_label(label: &str) -> bool {
    label.trim().eq_ignore_ascii_case("Open in browser")
}

fn output_html_link(text: &str, cwd: &Path) -> Option<String> {
    fn find(node: &markdown_ast::Node, cwd: &Path, links: &mut Vec<String>) {
        if let markdown_ast::Node::Link(link) = node {
            let label = link
                .children
                .iter()
                .map(markdown_ast::Node::to_string)
                .collect::<String>();
            if !browser_link_label(&label) && local_html_path(&link.url, cwd).is_some() {
                links.push(link.url.clone());
            }
        }
        if let Some(children) = node.children() {
            for child in children {
                find(child, cwd, links);
            }
        }
    }
    let root = markdown::to_mdast(text, &markdown::ParseOptions::gfm()).ok()?;
    let mut links = Vec::new();
    find(&root, cwd, &mut links);
    links.pop()
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
    version: Version,
    path: Option<PathBuf>,
    owner: EntityId,
    session: WeakEntity<HtmlSession>,
    paused: Rc<Cell<bool>>,
    surface: Option<Rc<Surface>>,
    error: Option<String>,
    loading_file: bool,
    source: bool,
    started: Option<std::time::Instant>,
    last_status: Option<LoadStatus>,
    poll: Option<gpui::Task<()>>,
    scroll: ScrollHandle,
    return_offset: Option<Point<Pixels>>,
    expand_focus: FocusHandle,
    return_focus: FocusHandle,
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
            loading_file: false,
            source: false,
            started: None,
            last_status: None,
            poll: None,
            scroll,
            return_offset: None,
            expand_focus: cx.focus_handle().tab_stop(true),
            return_focus: cx.focus_handle().tab_stop(true),
            external_return_focus: None,
        };
        if artifact.path.is_some() {
            artifact.read_file(cx);
        } else {
            artifact.load(cx);
        }
        artifact
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
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        self.surface = None;
        self.report = integrated_report(&self.version.active);
        self.report_height = INLINE_HEIGHT;
        self.report_theme.clear();
        self.error = None;
        self.poll = None;
        self.last_status = None;
        self.started = None;
        if self.version.active.len() > MAX_HTML_BYTES {
            self.error = Some("HTML document exceeds the preview limit".into());
            return;
        }
        let Some(native) =
            NativeDocument::new(&sandboxed(&self.version.active, cx.reduce_motion()))
        else {
            self.error = Some("Native HTML preview is unavailable".into());
            return;
        };
        self.surface = Some(Rc::new(Surface {
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
            self.return_focus.focus(window, cx);
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

/// Icon treatment for document toolbar actions, retaining shared button behavior.
fn document_action(
    id: &'static str,
    label: &'static str,
    glyph: &'static str,
    window: &Window,
    cx: &App,
) -> gpui_component::button::Button {
    transcript_action(id, label, false, window, cx)
        .icon_only()
        .tooltip(label)
        .w(rpx(CONTROL_H))
        .px_0()
        .child(crate::icons::icon(glyph, ICON_MD, c::FG_DIM()))
}

impl Render for HtmlArtifact {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.error.is_none() {
            if let Some(surface) = &self.surface {
                if let Err(error) = surface.native.prepare(window) {
                    self.error = Some(error);
                    self.surface = None;
                    self.poll = None;
                } else {
                    // Queued documents may remain offscreen indefinitely. Start
                    // the loading timeout only once a live window initializes it.
                    self.started.get_or_insert_with(std::time::Instant::now);
                }
            }
        }
        let expanded = expanded(&cx.entity(), cx);
        let report = self.report;
        if self.last_status == Some(LoadStatus::Ready) {
            let mut script = preference_script(cx.reduce_motion());
            if report {
                script.push_str(&report_script(window, expanded));
            }
            if script != self.report_theme {
                if let Some(surface) = &self.surface {
                    surface.native.report_theme(&script, report && !expanded);
                    self.report_theme = script;
                }
            }
        }
        if self.reveal && !expanded {
            self.reveal = false;
            self.anchor.scroll_to(window, cx);
        }
        let title = self
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("HTML document")
            .to_owned();
        let mut header = div()
            .debug_selector(|| "html-document-toolbar".into())
            .flex()
            .items_center()
            .gap(rpx(SPACE_SM))
            .when(!report || expanded, |header| {
                header
                    .p(rpx(SPACE_LG))
                    .border_b_1()
                    .border_color(c::BORDER())
            })
            .flex_wrap()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(rpx(TEXT_SMALL))
                    .when(!report || expanded, |label| label.child(title)),
            )
            .child(
                document_action("html-preview", "Preview", "unified", window, cx)
                    .selected(!self.source)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.source = false;
                        cx.notify();
                    })),
            )
            .child(
                document_action("html-source", "Source", "terminal", window, cx)
                    .selected(self.source)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.source = true;
                        cx.notify();
                    })),
            );
        if self.version.pending.is_some() {
            header = header.child(
                document_action(
                    "html-new-version",
                    "Load new version",
                    "restore",
                    window,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.version.adopt() {
                        this.load(cx);
                    }
                    cx.notify();
                })),
            );
        }
        if self.path.is_some() {
            header = header.child(
                document_action(
                    "html-check-version",
                    "Check for updates",
                    "restart",
                    window,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.read_file(cx))),
            );
        }
        if report && self.path.is_none() {
            header = header.child(
                document_action("html-reload", "Reload preview", "restart", window, cx).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.load(cx);
                        cx.notify();
                    }),
                ),
            );
        }
        header = header.child(
            document_action(
                "html-expand",
                if expanded {
                    "Return to conversation"
                } else {
                    "Expand"
                },
                if expanded { "arrow-left" } else { "zen" },
                window,
                cx,
            )
            .with_focus_handle(if expanded {
                self.return_focus.clone()
            } else {
                self.expand_focus.clone()
            })
            .on_click(cx.listener(Self::toggle_expand_click)),
        );
        let version_notice = self.version.pending.is_some().then(|| div().p(rpx(SPACE_LG)).text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child("A new version is available. Load new version resets this document’s edits, tab and scroll."));
        let mut body = div()
            .id("html-document-body")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden();
        if self.loading_file {
            body = body.child(div().p(rpx(SPACE_2XL)).child("Reading HTML document…"));
        } else if self.source || self.error.is_some() {
            body = body.overflow_y_scroll();
            if let Some(error) = &self.error {
                body = body
                    .child(
                        div()
                            .p(rpx(SPACE_LG))
                            .text_color(c::FG_DIM())
                            .child(error.clone()),
                    )
                    .child(
                        transcript_action("html-retry", "Retry preview", false, window, cx)
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/restart.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.version.active.is_empty() && this.path.is_some() {
                                    this.read_file(cx);
                                } else {
                                    this.load(cx);
                                }
                                cx.notify();
                            })),
                    );
            }
            body = body.child(source_view(&self.version.markdown, 0, window));
        } else if let Some(surface) = &self.surface {
            if surface.native.status() == LoadStatus::Loading {
                body = body.child(div().p(rpx(SPACE_2XL)).child("Loading preview…"));
            }
            body = body.child(NativeElement {
                artifact: cx.entity_id(),
                surface: surface.clone(),
                paused: self.paused.clone(),
            });
        }
        div()
            .id("html-artifact")
            .debug_selector(|| "html-artifact".into())
            .w_full()
            .min_w_0()
            .min_h_0()
            .when(expanded, gpui::Styled::h_full)
            .when(!expanded, |card| {
                card.h(if report && !self.source && self.error.is_none() {
                    // Measured CSS pixels are logical pixels; the cap follows UI zoom.
                    let cap = rpx(REPORT_MAX_HEIGHT).to_pixels(window.rem_size());
                    gpui::px(self.report_height).min(cap)
                } else {
                    rpx(INLINE_HEIGHT).to_pixels(window.rem_size())
                })
            })
            .flex()
            .flex_col()
            .bg(if report && !expanded {
                c::BG()
            } else {
                c::BG_RAIL()
            })
            .when(!report || expanded, |card| {
                card.border_1()
                    .border_color(c::BORDER())
                    .rounded(rpx(RADIUS_GROUP))
            })
            .overflow_hidden()
            .anchor_scroll((!expanded).then(|| self.anchor.clone()))
            .when(expanded, |card| {
                card.child(header)
                    .when_some(version_notice, gpui::ParentElement::child)
            })
            .child(body)
    }
}
impl HtmlArtifact {
    fn toggle_expand_click(
        &mut self,
        _: &gpui::ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_expand(window, cx);
    }
}

fn surface_is_selected(artifact: EntityId, expanded: Option<EntityId>) -> bool {
    expanded.is_none_or(|selected| selected == artifact)
}

struct NativeElement {
    artifact: EntityId,
    surface: Rc<Surface>,
    paused: Rc<Cell<bool>>,
}
impl IntoElement for NativeElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl gpui::Element for NativeElement {
    type RequestLayoutState = ();
    type PrepaintState = Bounds<Pixels>;
    fn id(&self) -> Option<ElementId> {
        Some(ElementId::NamedInteger(
            "html-native-surface".into(),
            self.artifact.as_u64(),
        ))
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = gpui::Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = gpui::relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        window: &mut Window,
        _: &mut App,
    ) -> Bounds<Pixels> {
        bounds.intersect(&window.content_mask().bounds)
    }
    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        clip: &mut Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let selected = cx
            .try_global::<HtmlUi>()
            .and_then(|ui| ui.expanded.as_ref())
            .map(|entity| entity.entity_id());
        let blocked = !surface_is_selected(self.artifact, selected)
            || self.paused.get()
            || cx.try_global::<HtmlUi>().is_some_and(|ui| ui.blocked)
            || gpui_component::Root::try_read(window, cx)
                .is_some_and(|root| root.has_visible_overlay(cx));
        if blocked
            || self.surface.native.status() != LoadStatus::Ready
            || clip.size.width <= Pixels::ZERO
            || clip.size.height <= Pixels::ZERO
        {
            self.surface.native.hide();
            return;
        }
        let generation = self.surface.generation.get().wrapping_add(1);
        self.surface.generation.set(generation);
        self.surface.native.show(bounds, *clip, window);
        let Some(id) = id else {
            self.surface.native.hide();
            return;
        };
        // Element arena objects are cleared AFTER presenting the current frame.
        // Keep the lease in GPUI's frame-owned state, not on the temporary
        // element or its request/prepaint data. It survives arena.clear and is
        // dropped only when a following frame omits this keyed surface.
        window.with_element_state(id, |previous: Option<SurfaceLease>, _| {
            drop(previous); // generation advanced above; cannot hide the new paint
            (
                (),
                SurfaceLease {
                    surface: self.surface.clone(),
                    generation,
                },
            )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integrated_reports_require_the_exact_marker() {
        assert!(!integrated_report("<html>ordinary document</html>"));
        assert!(!integrated_report("<html>grove:integrated-report</html>"));
        assert!(integrated_report(&format!("<html>{REPORT_MARKER}</html>")));
    }

    #[gpui::test]
    fn native_html_surface_lease_survives_arena_clear_and_retires_when_omitted(
        cx: &mut gpui::TestAppContext,
    ) {
        struct SurfaceFixture {
            surface: Rc<Surface>,
            mounted: bool,
        }
        impl Render for SurfaceFixture {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div().size_full().when(self.mounted, |root| {
                    root.child(NativeElement {
                        artifact: cx.entity_id(),
                        surface: self.surface.clone(),
                        paused: Rc::new(Cell::new(false)),
                    })
                })
            }
        }
        let visible = Rc::new(Cell::new(false));
        let probe = visible.clone();
        let (fixture, cx) = cx.add_window_view(|_, _| SurfaceFixture {
            surface: Rc::new(Surface {
                native: NativeDocument::with_visibility_probe(probe),
                generation: Cell::new(0),
            }),
            mounted: true,
        });
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(
            visible.get(),
            "clearing the just-presented GPUI arena must not retire its visible native surface"
        );
        cx.update(|window, cx| {
            fixture.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear();
        });
        assert!(
            visible.get(),
            "replacement frames must preserve the surface after their arena clears"
        );
        cx.update(|window, cx| {
            fixture.update(cx, |fixture, cx| {
                fixture.mounted = false;
                cx.notify();
            });
            window.draw(cx).clear();
        });
        assert!(
            !visible.get(),
            "omission must retire the prior frame-owned lease"
        );
        cx.update(|window, cx| {
            fixture.update(cx, |fixture, cx| {
                fixture.mounted = true;
                cx.notify();
            });
            window.draw(cx).clear();
        });
        assert!(
            visible.get(),
            "the retained document can attach again after omission"
        );
    }

    #[gpui::test]
    fn expansion_selects_only_one_surface_and_return_restores_scroll(
        cx: &mut gpui::TestAppContext,
    ) {
        struct Empty;
        impl Render for Empty {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
            }
        }
        cx.update(gpui_component::init);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let session = cx.new(|_| HtmlSession::new(PathBuf::from("/tmp"), ScrollHandle::new()));
            let owner = session.downgrade();
            let paused = Rc::new(Cell::new(false));
            let a = cx.new(|cx| {
                HtmlArtifact::new(
                    "<html>A</html>".into(),
                    "A".into(),
                    None,
                    owner.clone(),
                    paused.clone(),
                    ScrollHandle::new(),
                    cx,
                )
            });
            let b = cx.new(|cx| {
                HtmlArtifact::new(
                    "<html>B</html>".into(),
                    "B".into(),
                    None,
                    owner,
                    paused,
                    ScrollHandle::new(),
                    cx,
                )
            });
            set_context(Some(session.entity_id()), false, cx);
            assert!(surface_is_selected(a.entity_id(), None));
            assert!(surface_is_selected(b.entity_id(), None));
            a.update(cx, |a, cx| a.toggle_expand(window, cx));
            let selected = cx
                .global::<HtmlUi>()
                .expanded
                .as_ref()
                .map(|entity| entity.entity_id());
            assert!(surface_is_selected(a.entity_id(), selected));
            assert!(!surface_is_selected(b.entity_id(), selected));
            a.update(cx, |a, cx| {
                assert_eq!(
                    a.owner,
                    session.entity_id(),
                    "fenced artifact owner must be the session"
                );
                a.return_offset = Some(gpui::point(gpui::px(0.), gpui::px(-120.)));
                a.toggle_expand(window, cx);
                assert_eq!(a.scroll.offset().y, gpui::px(-120.));
                assert!(a.expand_focus.is_focused(window));
            });
            assert!(cx.global::<HtmlUi>().expanded.is_none());
            a.update(cx, |a, cx| a.toggle_expand(window, cx));
            set_context(None, false, cx);
            assert!(
                cx.global::<HtmlUi>().expanded.is_none(),
                "switching session dismisses expansion"
            );
            Empty
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn snapshot_open_releases_owner_before_expansion_and_restores_source_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        struct Source {
            focus: FocusHandle,
        }
        impl Render for Source {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().track_focus(&self.focus).child("Native plan source")
            }
        }
        cx.update(gpui_component::init);
        let mut owner = None;
        let mut source_focus = None;
        let (_, cx) = cx.add_window_view(|_, cx| {
            let session = cx.new(|_| HtmlSession::new(PathBuf::from("/tmp"), ScrollHandle::new()));
            set_context(Some(session.entity_id()), false, cx);
            let focus = cx.focus_handle();
            source_focus = Some(focus.clone());
            owner = Some(session);
            Source { focus }
        });
        let owner = owner.unwrap();
        let source_focus = source_focus.unwrap();
        let source = "<html><body>Immutable plan exhibit</body></html>";
        cx.update(|window, cx| {
            source_focus.focus(window, cx);
            owner.update(cx, |session, cx| {
                session.open_snapshot("saved-plan-asset".into(), source.into(), window, cx);
                assert!(cx.global::<HtmlUi>().expanded.is_none());
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let artifact = owner.read(cx).artifacts["saved-plan-asset"].clone();
            assert!(expanded(&artifact, cx));
            assert_eq!(artifact.read(cx).owner, owner.entity_id());
            assert_eq!(artifact.read(cx).version.active, source);
            artifact.update(cx, |artifact, cx| artifact.toggle_expand(window, cx));
            assert!(cx.global::<HtmlUi>().expanded.is_none());
            assert!(source_focus.is_focused(window));
            assert_eq!(owner.read(cx).artifacts.len(), 1);
            owner.update(cx, |session, cx| {
                session.open_snapshot(
                    "saved-plan-asset".into(),
                    "changed source".into(),
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let artifact = owner.read(cx).artifacts["saved-plan-asset"].clone();
            assert!(expanded(&artifact, cx));
            assert_eq!(artifact.read(cx).version.active, source);
            assert_eq!(owner.read(cx).artifacts.len(), 1);
            artifact.update(cx, |artifact, cx| artifact.toggle_expand(window, cx));
            assert!(source_focus.is_focused(window));
        });
    }

    #[gpui::test]
    fn file_references_in_later_replies_capture_independent_source(cx: &mut gpui::TestAppContext) {
        struct Empty;
        impl Render for Empty {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
            }
        }
        cx.update(gpui_component::init);
        let checkout = tempfile::tempdir().unwrap();
        let path = checkout.path().join("same.html");
        fs_err::write(&path, "<html>First version</html>").unwrap();
        let mut session = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let owner = cx.new(|_| HtmlSession::new(checkout.path().into(), ScrollHandle::new()));
            owner.update(cx, |session, cx| {
                session.open_link("first", "same.html", window, cx);
            });
            session = Some(owner);
            Empty
        });
        let session = session.unwrap();
        cx.run_until_parked();
        cx.update(|_, cx| {
            let first = session.read(cx).opened["first"][0].clone();
            assert_eq!(
                session.read(cx).artifacts[&first].read(cx).version.active,
                "<html>First version</html>"
            );
        });
        fs_err::write(&path, "<html>Updated version</html>").unwrap();
        cx.update(|window, cx| {
            session.update(cx, |session, cx| {
                session.open_link("second", "same.html", window, cx);
                session.open_link("first", "same.html", window, cx);
                session.open_link("second", "same.html", window, cx);
                assert_eq!(session.opened_count(), 2);
                assert_eq!(session.artifact_ids().len(), 2);
            });
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let session = session.read(cx);
            let first = &session.artifacts[&session.opened["first"][0]];
            let second = &session.artifacts[&session.opened["second"][0]];
            assert_ne!(first.entity_id(), second.entity_id());
            assert_eq!(first.read(cx).version.active, "<html>First version</html>");
            assert!(first.read(cx).version.pending.is_none());
            assert_eq!(
                second.read(cx).version.active,
                "<html>Updated version</html>"
            );
        });
    }

    #[gpui::test]
    fn inline_documents_omit_toolbar_and_expanded_documents_retain_controls(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(gpui_component::init);
        let mut owner = None;
        let (artifact, cx) = cx.add_window_view(|_, cx| {
            let session = cx.new(|_| HtmlSession::new(PathBuf::from("/tmp"), ScrollHandle::new()));
            owner = Some(session.clone());
            HtmlArtifact::new(
                "<html>Ordinary</html>".into(),
                String::new(),
                None,
                session.downgrade(),
                Rc::default(),
                ScrollHandle::new(),
                cx,
            )
        });
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("html-document-toolbar").is_none());
        cx.update(|window, cx| {
            artifact.update(cx, |artifact, cx| {
                artifact.report = true;
                cx.notify();
            });
            window.draw(cx).clear();
        });
        assert!(cx.debug_bounds("html-document-toolbar").is_none());
        cx.update(|window, cx| {
            artifact.update(cx, |artifact, cx| artifact.toggle_expand(window, cx));
            window.draw(cx).clear();
        });
        assert!(cx.debug_bounds("html-document-toolbar").is_some());
        drop(owner);
    }

    #[test]
    fn explicit_browser_links_are_not_auto_previewed() {
        let cwd = Path::new("/tmp/project");
        assert!(output_html_link("[Open in browser](large.html)", cwd).is_none());
        assert!(output_html_link("[**Open in browser**](large.html)", cwd).is_none());
        assert!(output_html_link("[  OPEN IN BROWSER  ](large.html)", cwd).is_none());
        assert_eq!(
            output_html_link(
                "[Small mock](small.html) [Open in browser](large.html)",
                cwd
            ),
            Some("small.html".into())
        );
    }

    #[cfg(unix)]
    #[test]
    fn fifo_and_symlink_to_fifo_are_rejected_without_opening() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("out.html");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        let link = dir.path().join("linked.html");
        symlink(&fifo, &link).unwrap();
        assert!(read_document(&fifo).unwrap_err().contains("regular file"));
        assert!(read_document(&link).unwrap_err().contains("regular file"));
        assert!(read_document(dir.path())
            .unwrap_err()
            .contains("regular file"));
    }

    #[test]
    fn complete_fences_and_documents_only() {
        for source in [
            "```html\n<html></html>\n```",
            "~~~~html\n<!doctype html><html></html>\n~~~~~",
        ] {
            assert!(closed_fence(source));
        }
        for source in [
            "```html\n<html></html>",
            "````html\n<html></html>\n```",
            "```html\n<html></html>\n~~~",
            "```html\n<html></html>\n```bad",
        ] {
            assert!(!closed_fence(source));
        }
        assert!(complete_document(
            "<!DOCTYPE html><html><body></body></html>"
        ));
        assert!(!complete_document("<div>fragment</div>"));
        assert!(!complete_document("<html><body>streaming"));
        assert!(!complete_document(&format!(
            "<html>{}</html>",
            "x".repeat(MAX_HTML_BYTES)
        )));
    }
    #[test]
    fn fast_completion_opens_without_a_rendered_busy_snapshot() {
        for restored_done in [false, true] {
            let mut auto = AutoOpen::default();
            assert!(auto
                .observe(
                    false,
                    restored_done,
                    None,
                    Some(("restored", "[old](old.html)"))
                )
                .is_none());
            assert!(auto
                .observe(false, true, None, Some(("restored", "[old](old.html)")))
                .is_none());
            assert_eq!(
                auto.observe(false, true, None, Some(("new-final", "[new](new.html)"))),
                Some(("new-final".into(), "[new](new.html)".into()))
            );
            assert!(auto
                .observe(false, true, None, Some(("new-final", "[new](new.html)")))
                .is_none());
        }
    }

    #[test]
    fn previous_turn_output_is_not_reopened_after_a_run_with_no_new_answer() {
        let mut auto = AutoOpen::default();
        auto.observe(false, true, None, Some(("old", "[old](old.html)")));
        auto.observe(true, false, Some("next"), Some(("old", "[old](old.html)")));
        assert!(auto
            .observe(false, true, None, Some(("old", "[old](old.html)")))
            .is_none());
    }
    #[test]
    fn browser_url_keeps_fragment_and_query_while_read_path_does_not() {
        let cwd = Path::new("/tmp/project");
        assert_eq!(
            local_html_url("preview.html?theme=dark#tasks", cwd)
                .unwrap()
                .as_str(),
            "file:///tmp/project/preview.html?theme=dark#tasks"
        );
        assert_eq!(
            local_html_path("preview.html?theme=dark#tasks", cwd).unwrap(),
            PathBuf::from("/tmp/project/preview.html")
        );
    }
    #[test]
    fn new_versions_require_explicit_adoption() {
        let mut version = Version {
            active: "first".into(),
            markdown: "one".into(),
            pending: None,
        };
        version.offer("second".into(), "two".into());
        assert_eq!(version.active, "first");
        version.offer("second".into(), "two".into());
        assert_eq!(version.active, "first");
        assert!(version.adopt());
        assert_eq!(version.active, "second");
        assert!(!version.adopt());
        version.offer("second".into(), "two".into());
        assert!(version.pending.is_none());
    }
    #[test]
    fn local_link_resolution_keeps_browser_and_disk_targets_distinct() {
        let cwd = Path::new("/tmp/project");
        for target in [
            "https://example.com/a.html",
            "//example.com/a.html",
            "file://remote/tmp/a.html",
            "mailto:a.html",
            "a.rs",
        ] {
            assert!(local_html_path(target, cwd).is_none(), "{target}");
        }
        for (target, expected) in [
            ("file:///tmp/a.htm#section", "/tmp/a.htm"),
            ("/tmp/A%20B.html#x", "/tmp/A B.html"),
            ("./out/demo.HTML", "/tmp/project/out/demo.HTML"),
            ("../demo.html", "/tmp/demo.html"),
        ] {
            assert_eq!(local_html_path(target, cwd), Some(PathBuf::from(expected)));
        }
    }
    #[test]
    fn auto_open_needs_observed_completed_run_and_deduplicates() {
        let mut auto = AutoOpen::default();
        assert!(auto
            .observe(false, true, None, Some(("old", "old")))
            .is_none());
        assert!(auto.observe(true, false, Some("t1"), None).is_none());
        assert!(auto
            .observe(true, false, Some("t1"), Some(("new", "partial")))
            .is_none());
        assert_eq!(
            auto.observe(false, true, None, Some(("new", "final"))),
            Some(("new".into(), "final".into()))
        );
        assert!(auto
            .observe(false, true, None, Some(("new", "final")))
            .is_none());
        auto.observe(true, false, Some("t2"), None);
        assert!(auto
            .observe(false, false, None, Some(("failed", "text")))
            .is_none());
    }
    #[test]
    fn output_detection_uses_markdown_links_not_tool_mentions() {
        let cwd = Path::new("/tmp");
        assert_eq!(
            output_html_link("Done: [Preview](out.html)", cwd),
            Some("out.html".into())
        );
        assert!(
            output_html_link("tool wrote out.html\n```sh\necho '[x](out.html)'\n```", cwd)
                .is_none()
        );
        assert!(output_html_link("[site](https://example.com/out.html)", cwd).is_none());
    }
    #[test]
    fn sandbox_policy_precedes_author_bytes_and_blocks_nested_and_transport_loads() {
        let hostile = "<html><head><meta http-equiv='Content-Security-Policy' content='default-src *'></head><body><iframe srcdoc=\"<script>fetch('https://example.com')</script>\"></iframe></body></html>";
        let html = sandboxed(hostile, false);
        assert!(html.find(CSP).unwrap() < html.find(hostile).unwrap());
        for directive in [
            "connect-src 'none'",
            "frame-src 'none'",
            "worker-src 'none'",
            "object-src 'none'",
            "form-action 'none'",
            "base-uri 'none'",
        ] {
            assert!(CSP.contains(directive));
        }
    }
    #[test]
    fn file_reads_reject_invalid_utf8_and_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.html");
        fs_err::write(&path, "<html>ok</html>").unwrap();
        assert_eq!(read_document(&path).unwrap(), "<html>ok</html>");
        fs_err::write(&path, [255]).unwrap();
        assert!(read_document(&path).is_err());
        fs_err::write(&path, vec![b'x'; MAX_HTML_BYTES + 1]).unwrap();
        assert!(read_document(&path).is_err());
        assert!(read_document(&dir.path().join("missing.html")).is_err());
    }
}
