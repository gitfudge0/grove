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

/// Continuously rotating arc — the Working/loading indicator. `tick` is the
/// GUI `blink_tick`; the arc advances 12° per tick for a smooth spin without
/// any glyph-font dependency or discrete frame array.
pub fn spinner<'a>(size: f32, color: Color, tick: u32) -> Element<'a, Msg> {
    let deg = (tick.wrapping_mul(12) % 360) as f32;
    // Open ~270° arc, gapped at the top-left, so the rotation is visible.
    let inner = format!(
        r#"<path d="M8 1.5a6.5 6.5 0 1 1-4.6 1.9" transform="rotate({deg} 8 8)"/>"#
    );
    let s = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">{inner}</svg>"#
    );
    svg(svg::Handle::from_memory(s.into_bytes()))
        .width(size)
        .height(size)
        .style(move |_, _| svg::Style { color: Some(color) })
        .into()
}

fn svg_for(name: &str) -> String {
    let inner = match name {
        "plus" => r#"<path d="M8 3v10M3 8h10"/>"#,
        "minus" => r#"<path d="M3 8h10"/>"#,
        "close" => r#"<path d="M4 4l8 8M12 4l-8 8"/>"#,
        "check" => r#"<path d="M3 8l4 4 6-7"/>"#,
        "play" => r#"<path d="M4.5 3.5l8 4.5-8 4.5z" fill="currentColor" stroke="none"/>"#,
        "chev-down" => r#"<path d="M4 6l4 4 4-4"/>"#,
        "chev-right" => r#"<path d="M6 4l4 4-4 4"/>"#,
        // Two stacked open chevrons + label hatches — "everything is open".
        "expand-all" => {
            r#"<path d="M3 4l2 2 2-2"/><path d="M3 10l2 2 2-2"/><path d="M9 5h4M9 11h4"/>"#
        }
        // Two stacked closed chevrons + label hatches — "everything is closed".
        "collapse-all" => {
            r#"<path d="M4 3l2 2-2 2"/><path d="M4 9l2 2-2 2"/><path d="M9 5h4M9 11h4"/>"#
        }
        "cog" => {
            r#"<circle cx="8" cy="8" r="2"/><path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.5 3.5l1.4 1.4M11.1 11.1l1.4 1.4M3.5 12.5l1.4-1.4M11.1 4.9l1.4-1.4"/>"#
        }
        "search" => r#"<circle cx="7" cy="7" r="4.5"/><path d="M10.4 10.4l3 3"/>"#,
        // Activity status glyphs.
        "question" => {
            r#"<path d="M5.8 6a2.2 2.2 0 1 1 3.2 2c-.8.5-1 .8-1 1.6"/><circle cx="8" cy="12.2" r="0.5" fill="currentColor" stroke="none"/>"#
        }
        "dot" => r#"<circle cx="8" cy="8" r="2" fill="currentColor" stroke="none"/>"#,
        "ring" => r#"<circle cx="8" cy="8" r="3.5"/>"#,
        "term" => {
            r#"<rect x="1.5" y="3" width="13" height="10" rx="1.5"/><path d="M4.5 7l2 1.5-2 1.5M8 10h3.5"/>"#
        }
        "more" => {
            r#"<circle cx="3.5" cy="8" r="1.2" fill="currentColor"/><circle cx="8" cy="8" r="1.2" fill="currentColor"/><circle cx="12.5" cy="8" r="1.2" fill="currentColor"/>"#
        }
        "split" => {
            r#"<rect x="1.5" y="2.5" width="13" height="11" rx="1.2"/><path d="M8 2.5v11"/>"#
        }
        "zen" => r#"<path d="M5 2.5H2.5V5M11 2.5h2.5V5M5 13.5H2.5V11M11 13.5h2.5V11"/>"#,
        // Circular arrow — restart / relaunch.
        "restart" => r#"<path d="M13 8a5 5 0 1 1-1.7-3.75M13 2.5V5h-2.5"/>"#,
        "edit" => r#"<path d="M11.5 2.5l2 2L6 12l-2.5.5L4 10z"/>"#,
        "trash" => {
            r#"<path d="M3 4.5h10M6 4.5V3a1 1 0 0 1 1-1h2a1 1 0 0 1 1 1v1.5M4.5 4.5l.5 8a1 1 0 0 0 1 .9h4a1 1 0 0 0 1-.9l.5-8"/>"#
        }
        // Claude wordmark glyph (Anthropic). Source: Bootstrap Icons.
        "claude" => {
            r#"<path fill="currentColor" stroke="none" d="m3.127 10.604 3.135-1.76.053-.153-.053-.085H6.11l-.525-.032-1.791-.048-1.554-.065-1.505-.08-.38-.081L0 7.832l.036-.234.32-.214.455.04 1.009.069 1.513.105 1.097.064 1.626.17h.259l.036-.105-.089-.065-.068-.064-1.566-1.062-1.695-1.121-.887-.646-.48-.327-.243-.306-.104-.67.435-.48.585.04.15.04.593.456 1.267.981 1.654 1.218.242.202.097-.068.012-.049-.109-.181-.9-1.626-.96-1.655-.428-.686-.113-.411a2 2 0 0 1-.068-.484l.496-.674L4.446 0l.662.089.279.242.411.94.666 1.48 1.033 2.014.302.597.162.553.06.17h.105v-.097l.085-1.134.157-1.392.154-1.792.052-.504.25-.605.497-.327.387.186.319.456-.045.294-.19 1.23-.37 1.93-.243 1.29h.142l.161-.16.654-.868 1.097-1.372.484-.545.565-.601.363-.287h.686l.505.751-.226.775-.707.895-.585.759-.839 1.13-.524.904.048.072.125-.012 1.897-.403 1.024-.186 1.223-.21.553.258.06.263-.218.536-1.307.323-1.533.307-2.284.54-.028.02.032.04 1.029.098.44.024h1.077l2.005.15.525.346.315.424-.053.323-.807.411-3.631-.863-.872-.218h-.12v.073l.726.71 1.331 1.202 1.667 1.55.084.383-.214.302-.226-.032-1.464-1.101-.565-.497-1.28-1.077h-.084v.113l.295.432 1.557 2.34.08.718-.112.234-.404.141-.444-.08-.911-1.28-.94-1.44-.759-1.291-.093.053-.448 4.821-.21.246-.484.186-.403-.307-.214-.496.214-.98.258-1.28.21-1.016.19-1.263.112-.42-.008-.028-.092.012-.953 1.307-1.448 1.957-1.146 1.227-.274.109-.477-.247.045-.44.266-.39 1.586-2.018.956-1.25.617-.723-.004-.105h-.036l-4.212 2.736-.75.096-.324-.302.04-.496.154-.162 1.267-.871z"/>"#
        }
        // OpenAI knot mark (used as the Codex icon). Source: Bootstrap Icons.
        "codex" => {
            r#"<path fill="currentColor" stroke="none" d="M14.949 6.547a3.94 3.94 0 0 0-.348-3.273 4.11 4.11 0 0 0-4.4-1.934A4.1 4.1 0 0 0 8.423.2 4.15 4.15 0 0 0 6.305.086a4.1 4.1 0 0 0-1.891.948 4.04 4.04 0 0 0-1.158 1.753 4.1 4.1 0 0 0-1.563.679A4 4 0 0 0 .554 4.72a3.99 3.99 0 0 0 .502 4.731 3.94 3.94 0 0 0 .346 3.274 4.11 4.11 0 0 0 4.402 1.933c.382.425.852.764 1.377.995.526.231 1.095.35 1.67.346 1.78.002 3.358-1.132 3.901-2.804a4.1 4.1 0 0 0 1.563-.68 4 4 0 0 0 1.14-1.253 3.99 3.99 0 0 0-.506-4.716m-6.097 8.406a3.05 3.05 0 0 1-1.945-.694l.096-.054 3.23-1.838a.53.53 0 0 0 .265-.455v-4.49l1.366.778q.02.011.025.035v3.722c-.003 1.653-1.361 2.992-3.037 2.996m-6.53-2.75a2.95 2.95 0 0 1-.36-2.01l.095.057L5.29 12.09a.53.53 0 0 0 .527 0l3.949-2.246v1.555a.05.05 0 0 1-.022.041L6.473 13.3c-1.454.826-3.311.335-4.15-1.098m-.85-6.94A3.02 3.02 0 0 1 3.07 3.949v3.785a.51.51 0 0 0 .262.451l3.93 2.237-1.366.779a.05.05 0 0 1-.048 0L2.585 9.342a2.98 2.98 0 0 1-1.113-4.094zm11.216 2.571L8.747 5.576l1.362-.776a.05.05 0 0 1 .048 0l3.265 1.86a3 3 0 0 1 1.173 1.207 2.96 2.96 0 0 1-.27 3.2 3.05 3.05 0 0 1-1.36.997V8.279a.52.52 0 0 0-.276-.445m1.36-2.015-.097-.057-3.226-1.855a.53.53 0 0 0-.53 0L6.249 6.153V4.598a.04.04 0 0 1 .019-.04L9.533 2.7a3.07 3.07 0 0 1 3.257.139c.474.325.843.778 1.066 1.303.223.526.289 1.103.191 1.664zM5.503 8.575 4.139 7.8a.05.05 0 0 1-.026-.037V4.049c0-.57.166-1.127.476-1.607s.752-.864 1.275-1.105a3.08 3.08 0 0 1 3.234.41l-.096.054-3.23 1.838a.53.53 0 0 0-.265.455zm.742-1.577 1.758-1 1.762 1v2l-1.755 1-1.762-1z"/>"#
        }
        // OpenCode favicon shape — outer terminal frame with an inset block.
        // Source: opencode.ai/favicon.svg, scaled from 512 → 16.
        "opencode" => {
            r#"<rect x="4" y="3" width="8" height="10" rx="0.5"/><rect x="6" y="5" width="4" height="6" fill="currentColor" stroke="none"/>"#
        }
        _ => "",
    };
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">{inner}</svg>"#
    )
}
