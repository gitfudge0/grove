//! Inline SVG sprite. Avoids depending on system glyph fonts so the GUI
//! looks identical across platforms.

use super::state::Msg;
use iced::widget::svg;
use iced::{Color, Element};

pub fn icon<'a>(name: &str, size: f32, color: Color) -> Element<'a, Msg> {
    let s = svg_for(name);
    svg(svg::Handle::from_memory(s.into_bytes()))
        .width(size)
        .height(size)
        .style(move |_, _| svg::Style { color: Some(color) })
        .into()
}

fn svg_for(name: &str) -> String {
    let inner = match name {
        "plus" => r#"<path d="M8 3v10M3 8h10"/>"#,
        "close" => r#"<path d="M4 4l8 8M12 4l-8 8"/>"#,
        "play" => r#"<path d="M5 3.5l7 4.5-7 4.5z" fill="currentColor"/>"#,
        "chev-down" => r#"<path d="M4 6l4 4 4-4"/>"#,
        "chev-right" => r#"<path d="M6 4l4 4-4 4"/>"#,
        "cog" => {
            r#"<circle cx="8" cy="8" r="2"/><path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.5 3.5l1.4 1.4M11.1 11.1l1.4 1.4M3.5 12.5l1.4-1.4M11.1 4.9l1.4-1.4"/>"#
        }
        "help" => {
            r#"<circle cx="8" cy="8" r="6.2"/><path d="M6 6.2c0-1.1 0.9-2 2-2s2 0.9 2 2c0 1.1-2 1.4-2 2.6M8 11.6v0.2"/>"#
        }
        "search" => r#"<circle cx="7" cy="7" r="4.5"/><path d="M10.4 10.4l3 3"/>"#,
        "term" => {
            r#"<rect x="1.5" y="3" width="13" height="10" rx="1.5"/><path d="M4.5 7l2 1.5-2 1.5M8 10h3.5"/>"#
        }
        "more" => {
            r#"<circle cx="3.5" cy="8" r="1.2" fill="currentColor"/><circle cx="8" cy="8" r="1.2" fill="currentColor"/><circle cx="12.5" cy="8" r="1.2" fill="currentColor"/>"#
        }
        "split" => {
            r#"<rect x="1.5" y="2.5" width="13" height="11" rx="1.2"/><path d="M8 2.5v11"/>"#
        }
        "edit" => r#"<path d="M11.5 2.5l2 2L6 12l-2.5.5L4 10z"/>"#,        "trash" => {
            r#"<path d="M3 4.5h10M6 4.5V3a1 1 0 0 1 1-1h2a1 1 0 0 1 1 1v1.5M4.5 4.5l.5 8a1 1 0 0 0 1 .9h4a1 1 0 0 0 1-.9l.5-8"/>"#
        }
        _ => "",
    };
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">{inner}</svg>"#
    )
}
