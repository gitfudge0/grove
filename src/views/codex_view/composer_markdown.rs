//! Native composer styling that preserves the original Markdown source.

use super::*;

/// Paint Markdown source without replacing text or changing editor mode. Leaf
/// spans are ordered and disjoint, so nested emphasis composes correctly with
/// independent picked-file/image decorations and UTF-8 cursor offsets.
pub(in crate::views) fn composer_markdown_decorations(text: &str) -> Vec<TextDecoration> {
    use markdown::mdast::Node;
    fn visit(node: &Node, mut style: gpui::HighlightStyle, out: &mut Vec<TextDecoration>) {
        match node {
            Node::Strong(_) | Node::Heading(_) => {
                style.font_weight = Some(gpui::FontWeight::BOLD);
            }
            Node::Emphasis(_) => style.font_style = Some(gpui::FontStyle::Italic),
            Node::Delete(_) => {
                style.strikethrough = Some(gpui::StrikethroughStyle {
                    thickness: gpui::px(1.),
                    color: None,
                });
            }
            Node::Link(_) | Node::LinkReference(_) => {
                style.color = Some(c::FG_DIM());
                style.underline = Some(gpui::UnderlineStyle {
                    thickness: gpui::px(1.),
                    ..Default::default()
                });
            }
            Node::InlineCode(_) => {
                // Match TextView inline code: neutral backplate, medium weight, no outline.
                style.background_color = Some(c::MENU_HOVER());
                style.font_weight = Some(gpui::FontWeight::MEDIUM);
            }
            Node::Code(_) => {
                style.background_color = Some(c::CODE_FILL());
                style.color = Some(c::FG());
                style.font_weight = Some(gpui::FontWeight::NORMAL);
            }
            _ => {}
        }
        if let Some(children) = node.children() {
            for child in children {
                visit(child, style, out);
            }
        } else if style != gpui::HighlightStyle::default() {
            if let Some(position) = node.position() {
                let range = position.start.offset..position.end.offset;
                if !range.is_empty() {
                    let decoration = TextDecoration::new(range, style);
                    out.push(if matches!(node, Node::Code(_) | Node::InlineCode(_)) {
                        decoration.font_family(crate::fonts::MONO_FAMILY)
                    } else {
                        decoration
                    });
                }
            }
        }
    }
    let mut decorations = Vec::new();
    if let Ok(root) = markdown::to_mdast(text, &markdown::ParseOptions::gfm()) {
        visit(&root, gpui::HighlightStyle::default(), &mut decorations);
    }
    decorations
}

/// Keep Markdown in the input model while projecting syntax into native text.
pub(in crate::views) fn composer_markdown_replacements(text: &str) -> Vec<TextReplacement> {
    use markdown::mdast::Node;
    fn hide(range: std::ops::Range<usize>, out: &mut Vec<TextReplacement>) {
        if !range.is_empty() {
            out.push(TextReplacement {
                range,
                text: String::new(),
                font_scale: 1.,
            });
        }
    }
    fn visit(node: &Node, text: &str, out: &mut Vec<TextReplacement>) {
        let Some(position) = node.position() else {
            return;
        };
        let range = position.start.offset..position.end.offset;
        match node {
            Node::Heading(heading) => {
                if let Some(first) = heading.children.first().and_then(Node::position) {
                    let prefix = range.start..first.start.offset;
                    // ATX headings; setext underlines remain literal until supported.
                    if text
                        .get(prefix.clone())
                        .is_some_and(|s| s.trim_start().starts_with('#'))
                    {
                        out.push(TextReplacement {
                            range: prefix,
                            text: String::new(),
                            font_scale: match heading.depth {
                                1 => 1.25,
                                2 => 1.125,
                                _ => 1.,
                            },
                        });
                        if let Some(last) = heading.children.last().and_then(Node::position) {
                            hide(last.end.offset..range.end, out);
                        }
                    }
                }
            }
            Node::ListItem(_) => {
                let line = text[range.clone()].lines().next().unwrap_or("");
                let marker_end = line.find([' ', '\t']).unwrap_or(0);
                let marker = &line[..marker_end];
                if matches!(marker, "-" | "*" | "+") {
                    out.push(TextReplacement {
                        range: range.start..range.start + marker_end,
                        text: "•".into(),
                        font_scale: 1.,
                    });
                }
            }
            Node::Strong(_)
            | Node::Emphasis(_)
            | Node::Delete(_)
            | Node::Link(_)
            | Node::LinkReference(_) => {
                if let Some(children) = node.children() {
                    if let (Some(first), Some(last)) = (
                        children.first().and_then(Node::position),
                        children.last().and_then(Node::position),
                    ) {
                        hide(range.start..first.start.offset, out);
                        hide(last.end.offset..range.end, out);
                    }
                }
            }
            Node::InlineCode(_) => {
                let source = &text[range.clone()];
                let ticks = source.bytes().take_while(|b| *b == b'`').count();
                if ticks > 0 && source.len() >= ticks * 2 {
                    hide(range.start..range.start + ticks, out);
                    hide(range.end - ticks..range.end, out);
                }
            }
            _ => {}
        }
        if let Some(children) = node.children() {
            for child in children {
                visit(child, text, out);
            }
        }
    }
    let mut replacements = Vec::new();
    if let Ok(root) = markdown::to_mdast(text, &markdown::ParseOptions::gfm()) {
        visit(&root, text, &mut replacements);
    }
    replacements.sort_by_key(|r| (r.range.start, r.range.end));
    replacements
}
