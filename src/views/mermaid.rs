//! Native, offline Mermaid blocks for transcript Markdown.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
};

use gpui::{div, img, prelude::*, AnyElement, App, Image, ImageFormat, Window};
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
    let theme_generation = grove_core::theme::generation();
    // Gantt today markers and implicit start dates must not stay cached forever.
    let clock_generation = if diagram.source.lines().any(|line| line.trim() == "gantt") {
        now_millis() / 60_000
    } else {
        0
    };
    let mut launch = false;
    let result = {
        let Ok(mut cache) = CACHE.lock() else {
            return source_view(diagram, window);
        };
        if let Some(entry) = cache
            .iter()
            .find(|entry| entry.matches(&diagram.source, theme_generation, clock_generation))
        {
            entry.result.clone()
        } else if cache
            .iter()
            .filter(|entry| matches!(entry.result, Rendered::Pending))
            .count()
            < MAX_PENDING
        {
            if cache.len() >= MAX_CACHE_ENTRIES {
                if let Some(index) = cache
                    .iter()
                    .position(|entry| !matches!(entry.result, Rendered::Pending))
                {
                    cache.remove(index);
                }
            }
            cache.push_back(CacheEntry {
                source: diagram.source.clone(),
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
        let source = diagram.source.clone();
        let options = options();
        let task = cx.background_executor().spawn(async move {
            // A renderer panic must never take down the transcript.
            let result = std::panic::catch_unwind(|| rasterize(&source, options))
                .ok()
                .flatten()
                .map_or(Rendered::Failed, |(image, ratio, width)| {
                    Rendered::Ready(image, ratio, width)
                });
            if let Ok(mut cache) = CACHE.lock() {
                if let Some(entry) = cache
                    .iter_mut()
                    .find(|entry| entry.matches(&source, theme_generation, clock_generation))
                {
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
    match result {
        Rendered::Ready(image, ratio, width) => ready_view(diagram, image, ratio, width),
        Rendered::Pending | Rendered::Failed => source_view(diagram, window),
    }
}

fn ready_view(diagram: &Diagram, image: Arc<Image>, ratio: f32, width: f32) -> AnyElement {
    div()
        .id(("mermaid-diagram", diagram.offset))
        .debug_selector(|| "transcript-mermaid-chart".to_owned())
        .w_full()
        .min_w_0()
        .p(rpx(SPACE_2XL))
        .bg(c::FIELD_FILL())
        .rounded(rpx(RADIUS_GROUP))
        .role(gpui::Role::Image)
        .aria_label("Mermaid diagram")
        .aria_description(diagram.source.clone())
        .child(img(image).w_full().max_w(rpx(width)).aspect_ratio(ratio))
        .into_any_element()
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
    let scale = (MAX_RASTER_SIDE as f32 / size.width().max(size.height())).min(2.0);
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
        };
        assert!(entry.matches("flowchart LR\nA-->B", 7, 0));
        assert!(!entry.matches("flowchart LR\nA-->B", 8, 0));
        assert!(!entry.matches("flowchart LR\nA-->C", 7, 0));
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
    }
    impl Render for DiagramGallery {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w_full()
                .h_full()
                .p(rpx(SPACE_LG))
                .bg(c::BG())
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .child(ready_view(
                    &self.diagram,
                    self.image.clone(),
                    self.ratio,
                    self.width,
                ))
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
                let source="flowchart LR\nA[\"<b>Grove café 東京</b>\"]-->B{Ready?}\nB-->|Yes|C[\"<code>source</code>\"]\nB-->|No|D[Copy source]";
                let (image, ratio, natural_width) = rasterize(source, options()).unwrap();
                let window = cx
                    .open_window(gpui::size(gpui::px(width), gpui::px(400.)), |window, cx| {
                        let gallery = cx.new(|_| DiagramGallery {
                            image,
                            ratio,
                            width: natural_width,
                            diagram: Diagram {
                                source: source.to_owned(),
                                markdown: format!("```mermaid\n{source}\n```"),
                                offset: 0,
                            },
                        });
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
                let path = format!(
                    "/tmp/grove-pele-{}-{width:.0}-rem{rem:.0}.png",
                    if light { "light" } else { "dark" }
                );
                cx.capture_screenshot(window.into())
                    .unwrap()
                    .save(&path)
                    .unwrap();
                eprintln!("Native Pele fixture: {path}");
                cx.update_window(window.into(), |_, window, _| window.remove_window())
                    .unwrap();
            }
        }
    }
}
