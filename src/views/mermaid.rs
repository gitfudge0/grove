//! Native, offline Mermaid blocks for transcript Markdown.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
};

use gpui::{div, img, prelude::*, AnyElement, App, Image, ImageFormat, Window};
use gpui_component::text::{
    markdown_ast, MarkdownExtensions, MarkdownNode, MarkdownParseContext, TextView,
};
use mermaid_rs_renderer::{RenderOptions, Theme};

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
    result: Rendered,
}
impl CacheEntry {
    fn matches(&self, source: &str, theme_generation: u64) -> bool {
        self.theme_generation == theme_generation && self.source == source
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
    let mut launch = false;
    let result = {
        let Ok(mut cache) = CACHE.lock() else {
            return source_view(diagram, window);
        };
        if let Some(entry) = cache
            .iter()
            .find(|entry| entry.matches(&diagram.source, theme_generation))
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
                    .find(|entry| entry.matches(&source, theme_generation))
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
        Rendered::Ready(image, ratio, width) => div()
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
            .into_any_element(),
        Rendered::Pending | Rendered::Failed => source_view(diagram, window),
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

fn options() -> RenderOptions {
    let fill = css(c::FIELD_FILL());
    let text = css(c::FG());
    let border = css(c::BORDER());
    let line = css(c::FG_DIM());
    let mut theme = Theme::modern();
    crate::fonts::UI_FAMILY.clone_into(&mut theme.font_family);
    theme.font_size = TEXT_BODY;
    theme.background.clone_from(&fill);
    theme.primary_color = css(c::BG_HOVER());
    theme.primary_text_color.clone_from(&text);
    theme.primary_border_color.clone_from(&border);
    theme.line_color.clone_from(&line);
    theme.secondary_color.clone_from(&fill);
    theme.tertiary_color.clone_from(&fill);
    theme.edge_label_background.clone_from(&fill);
    theme.cluster_background.clone_from(&fill);
    theme.cluster_border.clone_from(&border);
    theme.sequence_actor_fill.clone_from(&fill);
    theme.sequence_actor_border.clone_from(&border);
    theme.sequence_actor_line = line;
    theme.sequence_note_fill.clone_from(&fill);
    theme.sequence_note_border.clone_from(&border);
    theme.sequence_activation_fill = css(c::BG_HOVER());
    theme.sequence_activation_border.clone_from(&border);
    theme.text_color.clone_from(&text);
    theme.git_colors = std::array::from_fn(|_| css(c::BG_HOVER()));
    theme.git_inv_colors = std::array::from_fn(|_| text.clone());
    theme.git_branch_label_colors = std::array::from_fn(|_| text.clone());
    theme.git_commit_label_color.clone_from(&text);
    theme.git_commit_label_background.clone_from(&fill);
    theme.git_tag_label_color.clone_from(&text);
    theme.git_tag_label_background.clone_from(&fill);
    theme.git_tag_label_border.clone_from(&border);
    theme.pie_colors = std::array::from_fn(|index| {
        css(if index % 2 == 0 {
            c::BG_HOVER()
        } else {
            c::BORDER()
        })
    });
    theme.pie_title_text_size = TEXT_BODY;
    theme.pie_section_text_size = TEXT_BODY;
    theme.pie_legend_text_size = TEXT_BODY;
    theme.pie_title_text_color.clone_from(&text);
    theme.pie_section_text_color.clone_from(&text);
    theme.pie_legend_text_color = text;
    theme.pie_stroke_color.clone_from(&border);
    theme.pie_outer_stroke_color = border;
    RenderOptions {
        theme,
        ..RenderOptions::default()
    }
}

fn rasterize(source: &str, options: RenderOptions) -> Option<(Arc<Image>, f32, f32)> {
    use resvg::{tiny_skia, usvg};
    let svg = mermaid_rs_renderer::render_strict(source, options).ok()?;
    let mut svg_options = usvg::Options {
        font_family: crate::fonts::UI_FAMILY.to_owned(),
        ..usvg::Options::default()
    };
    if let Some(font) = crate::assets::Assets::get("fonts/Geist-Regular.ttf") {
        svg_options
            .fontdb_mut()
            .load_font_data(font.data.into_owned());
    }
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
            result: Rendered::Pending,
        };
        assert!(entry.matches("flowchart LR\nA-->B", 7));
        assert!(!entry.matches("flowchart LR\nA-->B", 8));
        assert!(!entry.matches("flowchart LR\nA-->C", 7));
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
