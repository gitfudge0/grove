//! Native, offline Mermaid blocks for transcript Markdown.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
};

use gpui::{
    div, img, prelude::*, AnyElement, App, Context, FocusHandle, Image, ImageFormat, MouseButton,
    Pixels, Point, Render, Size, Window,
};
use gpui_component::text::{
    markdown_ast, MarkdownExtensions, MarkdownNode, MarkdownParseContext, TextView,
};
mod pele;

struct RenderOptions {
    palette: std::collections::HashMap<String, String>,
    now: u64,
}

use super::{rpx, tokens::*};
use crate::theme as c;

// Resource limits, not design dimensions. Oversized diagrams retain their source.
const MAX_SOURCE_BYTES: usize = 8 * 1024;
const MAX_STATEMENTS: usize = 100;
const MAX_RASTER_SIDE: u32 = 2048;
const MAX_CACHE_ENTRIES: usize = 24;
const MAX_DETAIL_ENTRIES: usize = 2;
const MAX_DETAIL_SIDE: u32 = 8192;
const MAX_DETAIL_PIXELS: f32 = 16_777_216.;
const VIEWPORT_MAX_H: f32 = 420.;
const VIEWPORT_MIN_H: f32 = 100.;
const MAX_ZOOM: f32 = 8.;
const ZOOM_STEP: f32 = 1.5;
const PAN_STEP: f32 = 40.;
const MAX_PENDING: usize = 2;

#[derive(Clone)]
struct Diagram {
    source: String,
    markdown: String,
    offset: usize,
}
#[derive(Clone)]
enum Rendered {
    Pending,
    Ready(Arc<Image>, f32, f32),
    Failed,
}
struct CacheEntry {
    source: String,
    theme_generation: u64,
    clock_generation: u64,
    result: Rendered,
    detail: bool,
}
impl CacheEntry {
    fn matches(&self, source: &str, theme_generation: u64, clock_generation: u64) -> bool {
        self.theme_generation == theme_generation
            && self.clock_generation == clock_generation
            && self.source == source
    }
}
static CACHE: Mutex<VecDeque<CacheEntry>> = Mutex::new(VecDeque::new());

pub(super) fn extensions() -> MarkdownExtensions {
    static EXTENSIONS: OnceLock<MarkdownExtensions> = OnceLock::new();
    EXTENSIONS
        .get_or_init(|| {
            MarkdownExtensions::default()
                .block_parser(parse)
                .block_renderer("grove-mermaid", render)
        })
        .clone()
}

fn parse(node: &markdown_ast::Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
    let markdown_ast::Node::Code(code) = node else {
        return None;
    };
    if !code.lang.as_deref()?.eq_ignore_ascii_case("mermaid") {
        return None;
    }
    let markdown = cx.node_source(node)?;
    // markdown-rs accepts an unfinished fence. Keep streaming source visible until
    // a matching closing fence arrives (including tilde and longer backtick fences).
    if !closed_fence(markdown) || !within_limits(&code.value) {
        return None;
    }
    Some(
        MarkdownNode::new(
            "grove-mermaid",
            Diagram {
                source: code.value.clone(),
                markdown: markdown.to_owned(),
                offset: cx.offset() + code.position.as_ref()?.start.offset,
            },
        )
        .text(code.value.clone())
        .markdown(markdown.to_owned()),
    )
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
            close.chars().count() >= length && close.chars().all(|ch| ch == marker)
        })
}

fn within_limits(source: &str) -> bool {
    !source.trim().is_empty() && source.len() <= MAX_SOURCE_BYTES
        && source.split(['\n', ';']).count() <= MAX_STATEMENTS
        // Chains on one line can otherwise hide thousands of nodes/edges.
        && source.matches(['[', '(', '{', '>']).count() <= MAX_STATEMENTS
}

fn render(node: &MarkdownNode, window: &mut Window, cx: &mut App) -> AnyElement {
    let Some(diagram) = node.data::<Diagram>() else {
        return div().into_any_element();
    };
    let result = rendered(&diagram.source, false, cx);
    match result {
        Rendered::Ready(image, ratio, width) => {
            ready_view(diagram, image, ratio, width, window, cx).into_any_element()
        }
        Rendered::Pending | Rendered::Failed => source_view(diagram, window),
    }
}

fn diagram_clock(source: &str) -> u64 {
    if source.lines().any(|line| line.trim() == "gantt") {
        now_millis() / 60_000
    } else {
        0
    }
}

fn rendered(source: &str, detail: bool, cx: &mut App) -> Rendered {
    let theme_generation = grove_core::theme::generation();
    // Gantt today markers and implicit start dates must not stay cached forever.
    let clock_generation = diagram_clock(source);
    let mut launch = false;
    let result = {
        let Ok(mut cache) = CACHE.lock() else {
            return Rendered::Failed;
        };
        if let Some(entry) = cache.iter().find(|entry| {
            entry.detail == detail && entry.matches(source, theme_generation, clock_generation)
        }) {
            entry.result.clone()
        } else if cache
            .iter()
            .filter(|entry| matches!(entry.result, Rendered::Pending))
            .count()
            < MAX_PENDING
        {
            if detail && cache.iter().filter(|entry| entry.detail).count() >= MAX_DETAIL_ENTRIES {
                if let Some(index) = cache
                    .iter()
                    .position(|entry| entry.detail && !matches!(entry.result, Rendered::Pending))
                {
                    cache.remove(index);
                } else {
                    return Rendered::Pending;
                }
            }
            if cache.len() >= MAX_CACHE_ENTRIES {
                if let Some(index) = cache
                    .iter()
                    .position(|entry| !matches!(entry.result, Rendered::Pending))
                {
                    cache.remove(index);
                }
            }
            cache.push_back(CacheEntry {
                source: source.to_owned(),
                detail,
                theme_generation,
                clock_generation,
                result: Rendered::Pending,
            });
            launch = true;
            Rendered::Pending
        } else {
            Rendered::Failed
        }
    };
    if launch {
        let source = source.to_owned();
        let options = options();
        let task = cx.background_executor().spawn(async move {
            // A renderer panic must never take down the transcript.
            let result = std::panic::catch_unwind(|| {
                if detail {
                    rasterize_with_detail(&source, options, true)
                } else {
                    rasterize(&source, options)
                }
            })
            .ok()
            .flatten()
            .map_or(Rendered::Failed, |(image, ratio, width)| {
                Rendered::Ready(image, ratio, width)
            });
            if let Ok(mut cache) = CACHE.lock() {
                if let Some(entry) = cache.iter_mut().find(|entry| {
                    entry.detail == detail
                        && entry.matches(&source, theme_generation, clock_generation)
                }) {
                    entry.result = result;
                }
            }
        });
        cx.spawn(async move |cx| {
            task.await;
            cx.update(App::refresh_windows);
        })
        .detach();
    }
    result
}

fn ready_view(
    diagram: &Diagram,
    image: Arc<Image>,
    ratio: f32,
    width: f32,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<DiagramViewer> {
    // use_keyed_state is scoped by the enclosing TextView's element identity,
    // so identical source/offsets in separate conversations never share a camera.
    let source = diagram.source.clone();
    let viewer = window.use_keyed_state(("mermaid-viewer", diagram.offset), cx, |_, cx| {
        DiagramViewer {
            source: source.clone(),
            image: image.clone(),
            detail_image: None,
            ratio,
            width,
            zoom: 1.,
            pan: gpui::point(gpui::px(0.), gpui::px(0.)),
            drag: None,
            viewport: gpui::size(gpui::px(0.), gpui::px(0.)),
            #[cfg(test)]
            viewport_bounds: gpui::Bounds::default(),
            focus: cx.focus_handle(),
        }
    });
    viewer.update(cx, |viewer, _| {
        if viewer.source != source {
            viewer.zoom = 1.;
            viewer.detail_image = None;
            viewer.pan = gpui::point(gpui::px(0.), gpui::px(0.));
            viewer.drag = None;
        }
        viewer.source = source;
        viewer.image = image;
        viewer.ratio = ratio;
        viewer.width = width;
    });
    viewer
}

struct DiagramViewer {
    source: String,
    image: Arc<Image>,
    // A mounted viewer retains its resolved detail independently of the bounded
    // shared cache, so three zoomed charts cannot repeatedly evict one another.
    detail_image: Option<(u64, u64, Arc<Image>)>,
    ratio: f32,
    width: f32,
    zoom: f32,
    pan: Point<Pixels>,
    drag: Option<Point<Pixels>>,
    viewport: Size<Pixels>,
    #[cfg(test)]
    viewport_bounds: gpui::Bounds<Pixels>,
    focus: FocusHandle,
}
impl DiagramViewer {
    fn image_size(&self) -> Size<Pixels> {
        let fit_width = self.viewport.width.min(self.viewport.height * self.ratio);
        gpui::size(fit_width * self.zoom, fit_width / self.ratio * self.zoom)
    }
    fn clamp_pan(&mut self) {
        let size = self.image_size();
        let x = ((size.width - self.viewport.width) / 2.).max(gpui::px(0.));
        let y = ((size.height - self.viewport.height) / 2.).max(gpui::px(0.));
        self.pan.x = self.pan.x.clamp(-x, x);
        self.pan.y = self.pan.y.clamp(-y, y);
    }
    fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        let next = zoom.clamp(1., MAX_ZOOM);
        self.pan *= next / self.zoom;
        self.zoom = next;
        self.clamp_pan();
        self.drag = None;
        cx.notify();
    }
    fn control_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        zoom: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
            if zoom.clamp(1., MAX_ZOOM) != self.zoom {
                self.set_zoom(zoom, cx);
            }
            window.prevent_default();
            cx.stop_propagation();
        }
    }
    fn key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.platform
            || event.keystroke.modifiers.control
            || event.keystroke.modifiers.alt
        {
            return;
        }
        match event.keystroke.key.as_str() {
            "+" | "=" => self.set_zoom(self.zoom * ZOOM_STEP, cx),
            "-" => self.set_zoom(self.zoom / ZOOM_STEP, cx),
            "0" => self.set_zoom(1., cx),
            "left" | "right" | "up" | "down" if self.zoom > 1. => {
                let step = rpx(PAN_STEP).to_pixels(window.rem_size());
                match event.keystroke.key.as_str() {
                    "left" => self.pan.x += step,
                    "right" => self.pan.x -= step,
                    "up" => self.pan.y += step,
                    _ => self.pan.y -= step,
                }
                self.clamp_pan();
                cx.notify();
            }
            _ => return,
        }
        window.prevent_default();
        cx.stop_propagation();
    }
    fn release(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        }
    }
}
impl Render for DiagramViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use super::components;
        use crate::icons::icon;
        use gpui_component::Disableable as _;
        self.clamp_pan();
        let theme = grove_core::theme::generation();
        let clock = diagram_clock(&self.source);
        if self
            .detail_image
            .as_ref()
            .is_some_and(|(generation, minute, _)| *generation != theme || *minute != clock)
        {
            self.detail_image = None;
        }
        let image = if self.zoom > 1. {
            if self.detail_image.is_none() {
                if let Rendered::Ready(image, _, _) = rendered(&self.source, true, cx) {
                    self.detail_image = Some((theme, clock, image));
                }
            }
            self.detail_image
                .as_ref()
                .map_or_else(|| self.image.clone(), |(_, _, image)| image.clone())
        } else {
            self.image.clone()
        };
        let size = self.image_size();
        let origin = gpui::point(
            (self.viewport.width - size.width) / 2.,
            (self.viewport.height - size.height) / 2.,
        ) + self.pan;
        let min_height = if rpx(self.width).to_pixels(window.rem_size()) > self.viewport.width {
            VIEWPORT_MIN_H
        } else {
            VIEWPORT_MIN_H.min(self.width / self.ratio)
        };
        let height = (self.viewport.width / self.ratio).clamp(
            rpx(min_height).to_pixels(window.rem_size()),
            rpx(VIEWPORT_MAX_H).to_pixels(window.rem_size()),
        );
        let viewer = cx.entity().downgrade();
        div().id("mermaid-diagram")
            .debug_selector(|| "transcript-mermaid-chart".to_owned())
            .w_full().min_w_0().p(rpx(SPACE_2XL)).bg(c::FIELD_FILL()).rounded(rpx(RADIUS_GROUP))
            .flex().flex_col().gap(rpx(SPACE_SM))
            .child(div().flex().items_center().gap(rpx(SPACE_SM))
                .child(components::form_action("mermaid-zoom-out", "Zoom out diagram", false, window, cx)
                    .icon_only().tooltip("Zoom out diagram")
                    .child(icon("minus", ICON_SM, c::FG()))
                    .disabled(self.zoom <= 1.)
                    .focus_visible(|s| s.bg(c::BG_HOVER()))
                    .on_key_down(cx.listener(|this, event, window, cx| this.control_key(event, this.zoom / ZOOM_STEP, window, cx)))
                    .on_click(cx.listener(|this, _, _, cx| this.set_zoom(this.zoom / ZOOM_STEP, cx))))
                .child(components::form_action("mermaid-zoom-in", "Zoom in diagram", false, window, cx)
                    .icon_only().tooltip("Zoom in diagram")
                    .child(icon("plus", ICON_SM, c::FG()))
                    .disabled(self.zoom >= MAX_ZOOM)
                    .focus_visible(|s| s.bg(c::BG_HOVER()))
                    .on_key_down(cx.listener(|this, event, window, cx| this.control_key(event, this.zoom * ZOOM_STEP, window, cx)))
                    .on_click(cx.listener(|this, _, _, cx| this.set_zoom(this.zoom * ZOOM_STEP, cx))))
                .child(components::form_action("mermaid-fit", "Reset diagram to fit", false, window, cx)
                    .icon_only().child(icon("reset", ICON_SM, c::FG())).child("Fit").gap(rpx(SPACE_SM))
                    .focus_visible(|s| s.bg(c::BG_HOVER()))
                    .on_key_down(cx.listener(|this, event, window, cx| this.control_key(event, 1., window, cx)))
                    .on_click(cx.listener(|this, _, _, cx| this.set_zoom(1., cx))))
                .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child(format!("{:.0}%", self.zoom * 100.))))
            .child(div().id("mermaid-viewport")
                .debug_selector(|| "mermaid-viewport".to_owned())
                .relative().w_full().min_w_0().max_w(rpx(self.width)).h(height)
                .overflow_hidden().track_focus(&self.focus).tab_index(0).tab_stop(true)
                .role(gpui::Role::Image).aria_label("Mermaid diagram")
                .aria_description(format!("{}. Zoom with plus or minus, reset with 0. Drag or use arrow keys to pan when zoomed.", self.source))
                .tooltip(|window, cx| components::tooltip("Drag or arrow keys to pan · + / − to zoom · 0 to fit", window).build(window, cx))
                .focus_visible(|s| s.bg(c::BG_HOVER()))
                .when(self.zoom > 1., |el| el.cursor(gpui::CursorStyle::OpenHand))
                .when(self.drag.is_some(), |el| el.cursor(gpui::CursorStyle::ClosedHand))
                .child(gpui::canvas(move |bounds, _, cx| {
                    let _ = viewer.update(cx, |this, cx| {
                        #[cfg(test)] { this.viewport_bounds = bounds; }
                        if this.viewport != bounds.size { this.viewport = bounds.size; this.clamp_pan(); cx.notify(); }
                    });
                }, |_, (), _, _| {}).absolute().size_full())
                .child(img(image).absolute().left(origin.x).top(origin.y).w(size.width).h(size.height))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    window.prevent_default(); cx.stop_propagation();
                    if this.zoom > 1. { this.drag = Some(event.position); cx.notify(); }
                }))
                .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                    if let Some(last) = this.drag {
                        if event.pressed_button == Some(MouseButton::Left) {
                            this.pan += event.position - last; this.drag = Some(event.position); this.clamp_pan(); cx.stop_propagation(); cx.notify();
                        } else { this.release(cx); }
                    }
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.release(cx)))
                .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.release(cx)))
                .on_key_down(cx.listener(Self::key_down)))
    }
}

fn source_view(diagram: &Diagram, window: &Window) -> AnyElement {
    // No extensions on this inner view: fallback remains selectable and copyable.
    TextView::markdown(("mermaid-source", diagram.offset), diagram.markdown.clone())
        .style(super::codex_view::markdown_style(window))
        .selectable(true)
        .into_any_element()
}

fn css(color: gpui::Hsla) -> String {
    let rgba: gpui::Rgba = color.into();
    format!("#{:06x}", u32::from(rgba) >> 8)
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().try_into().unwrap_or(u64::MAX)
        })
}

fn options() -> RenderOptions {
    let mut palette = std::collections::HashMap::from([
        ("--pele-bg".to_owned(), css(c::FIELD_FILL())),
        ("--pele-fg".to_owned(), css(c::FG())),
        ("--pele-muted".to_owned(), css(c::FG_DIM())),
        ("--pele-line".to_owned(), css(c::FG_DIM())),
        ("--pele-surface".to_owned(), css(c::BG_HOVER())),
        ("--pele-surface-alt".to_owned(), css(c::FIELD_FILL())),
        ("--pele-border".to_owned(), css(c::BORDER())),
        ("--pele-accent".to_owned(), css(c::FG_DIM())),
        ("--pele-font".to_owned(), crate::fonts::UI_FAMILY.to_owned()),
        (
            "--pele-font-mono".to_owned(),
            crate::fonts::MONO_FAMILY.to_owned(),
        ),
        ("--pele-bold-weight".to_owned(), "bold".to_owned()),
    ]);
    for index in 1..=8 {
        palette.insert(
            format!("--pele-series-{index}"),
            css(if index % 2 == 0 {
                c::BORDER()
            } else {
                c::BG_HOVER()
            }),
        );
    }
    RenderOptions {
        palette,
        now: now_millis(),
    }
}

fn rasterize(source: &str, options: RenderOptions) -> Option<(Arc<Image>, f32, f32)> {
    rasterize_with_detail(source, options, false)
}

fn rasterize_with_detail(
    source: &str,
    options: RenderOptions,
    detail: bool,
) -> Option<(Arc<Image>, f32, f32)> {
    use resvg::{tiny_skia, usvg};
    if !within_limits(source) {
        return None;
    }
    let svg = pele::render(source, TEXT_BODY, &options.palette, options.now)?;
    let mut svg_options = usvg::Options {
        font_family: crate::fonts::UI_FAMILY.to_owned(),
        ..usvg::Options::default()
    };
    svg_options.fontdb = pele::fonts();
    // Never resolve file, network or embedded image references from transcript data.
    svg_options.image_href_resolver = usvg::ImageHrefResolver {
        resolve_data: Box::new(|_, _, _| None),
        resolve_string: Box::new(|_, _| None),
    };
    let tree = usvg::Tree::from_str(&svg, &svg_options).ok()?;
    let size = tree.size();
    let scale = if detail {
        (MAX_DETAIL_SIDE as f32 / size.width().max(size.height()))
            .min((MAX_DETAIL_PIXELS / (size.width() * size.height())).sqrt())
            .min(4.)
    } else {
        (MAX_RASTER_SIDE as f32 / size.width().max(size.height())).min(2.0)
    };
    let width = (size.width() * scale).ceil() as u32;
    let height = (size.height() * scale).ceil() as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let bytes = pixmap.encode_png().ok()?;
    Some((
        Arc::new(Image::from_bytes(ImageFormat::Png, bytes)),
        size.width() / size.height(),
        width as f32 / 2.0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_changes_invalidate_diagrams_even_within_one_appearance() {
        let entry = CacheEntry {
            source: "flowchart LR\nA-->B".to_owned(),
            theme_generation: 7,
            clock_generation: 0,
            result: Rendered::Pending,
            detail: false,
        };
        assert!(entry.matches("flowchart LR\nA-->B", 7, 0));
        assert!(!entry.matches("flowchart LR\nA-->B", 8, 0));
        assert!(!entry.matches("flowchart LR\nA-->C", 7, 0));
    }

    #[test]
    fn zoom_requests_more_detail_without_unbounded_rasters() {
        let source = format!(
            "flowchart LR\n{}",
            (0..18)
                .map(|i| format!("A{i}[Stage {i}] --> A{}", i + 1))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let (preview, _, _) = rasterize(&source, options()).unwrap();
        let (detail, _, _) = rasterize_with_detail(&source, options(), true).unwrap();
        let preview = image::load_from_memory(preview.bytes()).unwrap();
        let detail = image::load_from_memory(detail.bytes()).unwrap();
        assert!(detail.width() > preview.width() * 2);
        assert!(detail.width() <= MAX_DETAIL_SIDE && detail.height() <= MAX_DETAIL_SIDE);
        assert!(
            detail.width() as f32 * detail.height() as f32
                <= MAX_DETAIL_PIXELS + MAX_DETAIL_SIDE as f32 * 2.
        );
    }

    #[test]
    fn only_complete_matching_fences_render() {
        for source in [
            "```mermaid\nflowchart LR\nA-->B\n```",
            "~~~~mermaid\nA-->B\n~~~~~",
            "  ```mermaid\nA-->B\n  ```",
        ] {
            assert!(closed_fence(source), "{source}");
        }
        for source in [
            "```mermaid",
            "```mermaid\nA-->B",
            "````mermaid\nA-->B\n```",
            "```mermaid\nA-->B\n~~~",
            "```mermaid\nA-->B\n```extra",
        ] {
            assert!(!closed_fence(source), "{source}");
        }
    }

    #[test]
    fn resource_limits_include_dense_single_line_diagrams() {
        assert!(within_limits("flowchart LR\nA-->B"));
        assert!(!within_limits(""));
        assert!(!within_limits(&"x".repeat(MAX_SOURCE_BYTES + 1)));
        assert!(!within_limits(&"A-->".repeat(MAX_STATEMENTS + 1)));
        assert!(!within_limits(&"A;".repeat(MAX_STATEMENTS + 1)));
    }

    #[test]
    fn common_diagram_families_rasterize_and_errors_fall_back() {
        for source in [
            "flowchart LR\nA[Start] --> B{Ready}\nB -->|Yes| C[Done]",
            "sequenceDiagram\nAlice->>Bob: Hello\nBob-->>Alice: Hi",
            "classDiagram\nAnimal <|-- Duck\nAnimal : +int age",
            "stateDiagram-v2\n[*] --> Ready\nReady --> [*]",
            "erDiagram\nCUSTOMER ||--o{ ORDER : places",
            "pie title Work\n\"Done\" : 3\n\"Pending\" : 2",
            "gantt\n dateFormat YYYY-MM-DD\n section Build\n Task :2026-10-24, 2d",
            "mindmap\n  root((Grove))\n    Diagrams\n    Source",
            "flowchart LR\nsubgraph One\nsubgraph Two\nA[Hello]-->B[World]\nend\nend",
        ] {
            let (image, ratio, width) = rasterize(source, options())
                .unwrap_or_else(|| panic!("Could not rasterize: {source}"));
            assert!(ratio.is_finite() && ratio > 0.0);
            assert!(width > 0.0 && width <= MAX_RASTER_SIDE as f32 / 2.0);
            let decoded = image::load_from_memory(image.bytes()).unwrap();
            assert!(decoded.width() <= MAX_RASTER_SIDE && decoded.height() <= MAX_RASTER_SIDE);
            assert!(decoded.to_rgba8().pixels().any(|pixel| pixel[3] > 0));
        }
        assert!(rasterize("this is not a diagram", options()).is_none());
        assert!(rasterize("flowchart LR\nsubgraph MissingEnd\nA --> B", options()).is_none());
    }
}

#[cfg(all(test, target_os = "macos"))]
mod native_snapshots {
    use super::*;
    use gpui::{Context, Render};
    struct DiagramGallery {
        image: Arc<Image>,
        ratio: f32,
        width: f32,
        diagram: Diagram,
        viewer: Option<gpui::Entity<DiagramViewer>>,
        sibling: Option<gpui::Entity<DiagramViewer>>,
        wheel: std::rc::Rc<std::cell::Cell<usize>>,
    }
    impl Render for DiagramGallery {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let viewer = window.with_element_namespace("first-message", |window| {
                ready_view(
                    &self.diagram,
                    self.image.clone(),
                    self.ratio,
                    self.width,
                    window,
                    cx,
                )
            });
            let sibling = window.with_element_namespace("second-message", |window| {
                ready_view(
                    &self.diagram,
                    self.image.clone(),
                    self.ratio,
                    self.width,
                    window,
                    cx,
                )
            });
            self.viewer = Some(viewer.clone());
            self.sibling = Some(sibling);
            let wheel = self.wheel.clone();
            div()
                .id("transcript-fixture")
                .on_key_down(super::super::shell::traverse_unhandled_tab)
                .on_scroll_wheel(move |_, _, _| wheel.set(wheel.get() + 1))
                .w_full()
                .h_full()
                .p(rpx(SPACE_LG))
                .bg(c::BG())
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .child(viewer)
                .child(source_view(
                    &Diagram {
                        source: "flowchart LR\nA-->".to_owned(),
                        markdown: "```mermaid\nflowchart LR\nA-->".to_owned(),
                        offset: 1,
                    },
                    window,
                ))
        }
    }
    fn dispatch(
        cx: &mut gpui::HeadlessAppContext,
        window: gpui::AnyWindowHandle,
        event: gpui::PlatformInput,
    ) {
        cx.update_window(window, |_, window, cx| {
            window.dispatch_event(event, cx);
        })
        .unwrap();
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update_window(window, |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
        }
        cx.run_until_parked();
    }
    fn click(
        cx: &mut gpui::HeadlessAppContext,
        window: gpui::AnyWindowHandle,
        position: Point<Pixels>,
    ) {
        dispatch(
            cx,
            window,
            gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                position,
                button: MouseButton::Left,
                modifiers: gpui::Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
        );
        dispatch(
            cx,
            window,
            gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                position,
                button: MouseButton::Left,
                modifiers: gpui::Modifiers::default(),
                click_count: 1,
            }),
        );
    }
    fn key(cx: &mut gpui::HeadlessAppContext, window: gpui::AnyWindowHandle, key: &str) {
        dispatch(
            cx,
            window,
            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse(key).unwrap(),
                is_held: false,
                prefer_character_input: false,
            }),
        );
    }

    #[test]
    #[ignore = "requires macOS Metal renderer; writes native Pele fixture screenshots to /tmp"]
    fn pele_native_snapshots() {
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
        });
        for light in [false, true] {
            for (width, rem) in [(760., 16.), (360., 20.)] {
                cx.update(|cx| {
                    grove_core::theme::set(
                        grove_core::theme::by_name(if light {
                            "grove-light"
                        } else {
                            "grove-dark"
                        })
                        .unwrap(),
                    );
                    c::sync_component_theme(cx);
                    gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                });
                let source="flowchart LR\nA[Understand request]-->B[Inspect existing behavior]-->C[Choose approach]-->D[Implement change]-->E[Verify interactions]-->F[Review result]-->G[Ready to ship]";
                let (image, ratio, natural_width) = rasterize(source, options()).unwrap();
                let gallery_slot = std::rc::Rc::new(std::cell::RefCell::new(None));
                let gallery_out = gallery_slot.clone();
                let window = cx
                    .open_window(gpui::size(gpui::px(width), gpui::px(400.)), |window, cx| {
                        let gallery = cx.new(|_| DiagramGallery {
                            image,
                            ratio,
                            width: natural_width,
                            viewer: None,
                            sibling: None,
                            wheel: std::rc::Rc::default(),
                            diagram: Diagram {
                                source: source.to_owned(),
                                markdown: format!("```mermaid\n{source}\n```"),
                                offset: 0,
                            },
                        });
                        *gallery_out.borrow_mut() = Some(gallery.clone());
                        cx.new(|cx| {
                            gpui_component::Root::new(gallery, window, cx)
                                .bordered(false)
                                .bg(c::BG())
                        })
                    })
                    .unwrap();
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    let _ = window.draw(cx);
                })
                .unwrap();
                for _ in 0..3 {
                    cx.run_until_parked();
                    cx.update_window(window.into(), |_, window, cx| {
                        let _ = window.draw(cx);
                    })
                    .unwrap();
                }
                let path = format!(
                    "/tmp/grove-pele-{}-{width:.0}-rem{rem:.0}.png",
                    if light { "light" } else { "dark" }
                );
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Native Pele fixture: {path}");
                let gallery = gallery_slot.borrow().as_ref().unwrap().clone();
                let (viewer, sibling, wheel) = cx.update(|cx| {
                    let gallery = gallery.read(cx);
                    (
                        gallery.viewer.clone().unwrap(),
                        gallery.sibling.clone().unwrap(),
                        gallery.wheel.clone(),
                    )
                });
                assert_ne!(
                    viewer.entity_id(),
                    sibling.entity_id(),
                    "equal offsets in different message scopes must own different cameras"
                );
                let original = cx.update(|cx| viewer.read(cx).viewport_bounds);
                assert!(original.size.width > gpui::px(0.) && original.size.height > gpui::px(0.));
                // Activate the actual native zoom button, not a direct camera mutation.
                let scale = rem / 16.;
                let plus = gpui::point(
                    original.origin.x + gpui::px(46. * scale),
                    original.origin.y - gpui::px(16. * scale),
                );
                dispatch(
                    &mut cx,
                    window.into(),
                    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position: plus,
                        pressed_button: None,
                        modifiers: gpui::Modifiers::default(),
                    }),
                );
                click(&mut cx, window.into(), plus);
                cx.update(|cx| assert!(viewer.read(cx).zoom > 1., "native plus button must zoom"));
                // Focus the viewport; keyboard zoom and pan route to its owner.
                click(&mut cx, window.into(), original.center());
                for _ in 0..4 {
                    key(&mut cx, window.into(), "=");
                }
                key(&mut cx, window.into(), "right");
                cx.update(|cx| {
                    let view = viewer.read(cx);
                    assert!(
                        view.zoom > 4.,
                        "zoom={} bounds={:?}",
                        view.zoom,
                        view.viewport_bounds
                    );
                    assert!(view.pan.x < gpui::px(0.));
                    assert_eq!(
                        view.viewport_bounds, original,
                        "zoom must retain transcript geometry"
                    );
                    assert_eq!(
                        sibling.read(cx).zoom,
                        1.,
                        "other message camera must remain fitted"
                    );
                });
                key(&mut cx, window.into(), "down");
                cx.update(|cx| {
                    let view = viewer.read(cx);
                    if view.image_size().height > view.viewport.height {
                        assert!(
                            view.pan.y < gpui::px(0.),
                            "down arrow must pan vertically overflowing content"
                        );
                    } else {
                        assert_eq!(
                            view.pan.y,
                            gpui::px(0.),
                            "vertical pan clamps when content fits"
                        );
                    }
                });
                let before = cx.update(|cx| viewer.read(cx).pan);
                let start = original.center();
                dispatch(
                    &mut cx,
                    window.into(),
                    gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                        position: start,
                        button: MouseButton::Left,
                        modifiers: gpui::Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    }),
                );
                dispatch(
                    &mut cx,
                    window.into(),
                    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position: start + gpui::point(gpui::px(35.), gpui::px(0.)),
                        pressed_button: Some(MouseButton::Left),
                        modifiers: gpui::Modifiers::default(),
                    }),
                );
                cx.update(|cx| {
                    assert_ne!(viewer.read(cx).pan, before, "native drag must move camera");
                });
                dispatch(
                    &mut cx,
                    window.into(),
                    gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                        position: gpui::point(gpui::px(1.), gpui::px(1.)),
                        button: MouseButton::Left,
                        modifiers: gpui::Modifiers::default(),
                        click_count: 1,
                    }),
                );
                cx.update(|cx| {
                    assert!(
                        viewer.read(cx).drag.is_none(),
                        "release outside must end panning"
                    );
                });
                let wheel_before = wheel.get();
                dispatch(
                    &mut cx,
                    window.into(),
                    gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                        position: start,
                        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-30.))),
                        modifiers: gpui::Modifiers::default(),
                        touch_phase: gpui::TouchPhase::Moved,
                    }),
                );
                assert!(
                    wheel.get() > wheel_before,
                    "ordinary wheel must reach the transcript parent"
                );
                let held_detail = cx.update(|cx| {
                    viewer
                        .read(cx)
                        .detail_image
                        .as_ref()
                        .expect("zoom resolves a higher-resolution raster")
                        .2
                        .clone()
                });
                CACHE.lock().unwrap().retain(|entry| !entry.detail);
                cx.update(|cx| viewer.update(cx, |_, cx| cx.notify()));
                cx.update_window(window.into(), |_, window, cx| {
                    let _ = window.draw(cx);
                })
                .unwrap();
                cx.run_until_parked();
                cx.update(|cx| {
                    assert!(
                        Arc::ptr_eq(
                            &held_detail,
                            &viewer.read(cx).detail_image.as_ref().unwrap().2
                        ),
                        "mounted viewer must retain detail after shared cache eviction"
                    );
                });
                assert!(
                    !CACHE.lock().unwrap().iter().any(|entry| entry.detail),
                    "cache eviction must not request another raster for retained detail"
                );
                let zoom_path = path.replace(".png", "-zoom-pan.png");
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&zoom_path)
                    .unwrap();
                eprintln!("Native zoom/pan fixture: {zoom_path}");
                key(&mut cx, window.into(), "0");
                cx.update(|cx| {
                    let view = viewer.read(cx);
                    assert_eq!(view.zoom, 1.);
                    assert_eq!(view.pan, gpui::point(gpui::px(0.), gpui::px(0.)));
                    assert_eq!(view.viewport_bounds, original);
                });
                key(&mut cx, window.into(), "=");
                key(&mut cx, window.into(), "shift-tab");
                key(&mut cx, window.into(), "enter");
                cx.update(|cx| {
                    assert_eq!(
                        viewer.read(cx).zoom,
                        1.,
                        "Fit must be reachable and activatable from keyboard"
                    );
                });
                click(&mut cx, window.into(), plus);
                let fit = gpui::point(
                    original.origin.x + gpui::px(86. * scale),
                    original.origin.y - gpui::px(16. * scale),
                );
                click(&mut cx, window.into(), fit);
                cx.update(|cx| {
                    assert_eq!(
                        viewer.read(cx).zoom,
                        1.,
                        "native Fit click must reset camera"
                    );
                });
                eprintln!("PASS native zoom click, focused keyboard zoom/arrows/reset, pointer pan, outside release, wheel propagation, equal-offset isolation: {width} rem{rem} light={light}");
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }
}
