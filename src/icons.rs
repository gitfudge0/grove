//! The in-memory SVG icon sprite (spec §6), served to gpui through
//! [`crate::assets::Assets`] and tinted with `text_color`.
//!
//! Pulled forward from Plan 06 by carried amendment 4: the sidebar's chevrons,
//! git glyph, plus button, `main` tag, hover-action icons and the 12-frame
//! spinner are Appendix A sidebar content, so they cannot wait.
//!
//! `svg_for` is `src/gui/icons.rs:88-198` ported verbatim — a pure
//! `&str -> String`, so the two front ends draw the same shapes. Nothing is
//! shipped as a file: [`Assets::load`](crate::assets::Assets) answers
//! `icons/<name>.svg` and `icons/spinner-<frame>.svg` straight out of this
//! table. The color is **not** baked into the SVG text (`stroke="currentColor"`
//! / `fill="currentColor"`), so one path serves every tint.
//!
//! **Verified at ZED_REV `1a246ef`:** `Svg` has no `color` method; it paints
//! with `style.text.color` (`crates/gpui/src/elements/svg.rs:110`), so the tint
//! is `Styled::text_color`.

// `icon`/`spinner` are consumed by Task 5's row renderers.
#![allow(dead_code)]

use crate::views::rpx;
use gpui::{svg, Hsla, SharedString, Styled as _, Svg};

use crate::entities::animation_clock::{spinner_frame, SPINNER_FRAMES};

/// A square, tinted icon by sprite name.
pub fn icon(name: &str, size: f32, color: Hsla) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(rpx(size))
        .text_color(color)
}

/// The rotating Working/loading arc. `tick` is the animation clock; the arc
/// advances one of [`SPINNER_FRAMES`] fixed steps every 3 ticks — parity with
/// `src/gui/icons.rs:136-138`.
pub fn spinner(size: f32, color: Hsla, tick: u64) -> Svg {
    let frame = spinner_frame(tick);
    svg()
        .path(SharedString::from(format!("icons/spinner-{frame}.svg")))
        .size(rpx(size))
        .text_color(color)
}

/// The SVG text for `path`, or `None` when `path` is not an icon path. This is
/// the whole of grove-gpui's in-memory asset branch.
pub fn svg_for_path(path: &str) -> Option<String> {
    let name = path.strip_prefix("icons/")?.strip_suffix(".svg")?;
    if let Some(frame) = name.strip_prefix("spinner-") {
        let frame: u64 = frame.parse().ok()?;
        if frame >= SPINNER_FRAMES {
            return None;
        }
        // Open ~270 degree arc, gapped at the top-left, so the rotation is
        // visible (`src/gui/icons.rs:125-129`).
        let deg = frame * (360 / SPINNER_FRAMES);
        return Some(wrap_svg(&format!(
            r#"<path d="M8 1.5a6.5 6.5 0 1 1-4.6 1.9" transform="rotate({deg} 8 8)"/>"#
        )));
    }
    Some(svg_for(name))
}

fn wrap_svg(inner: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">{inner}</svg>"#
    )
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
        // Chevron pointing into a vertical bar — "collapse panel to the right".
        "collapse-right" => r#"<path d="M6 4l4 4-4 4"/><path d="M13 3v10"/>"#,
        // Two stacked open chevrons + label hatches — "everything is open".
        "expand-all" => {
            r#"<path d="M3 4l2 2 2-2"/><path d="M3 10l2 2 2-2"/><path d="M9 5h4M9 11h4"/>"#
        }
        // One open + one closed chevron + label hatches — "expand only the
        // ones that matter" (sessions-only, the middle cycle state).
        "expand-sessions" => {
            r#"<path d="M3 4l2 2 2-2"/><path d="M4 9l2 2-2 2"/><path d="M9 5h4M9 11h4"/>"#
        }
        // Two stacked closed chevrons + label hatches — "everything is closed".
        "collapse-all" => {
            r#"<path d="M4 3l2 2-2 2"/><path d="M4 9l2 2-2 2"/><path d="M9 5h4M9 11h4"/>"#
        }
        "cog" => {
            r#"<path fill="currentColor" stroke="none" fill-rule="evenodd" d="M8 4.754a3.246 3.246 0 1 0 0 6.492 3.246 3.246 0 0 0 0-6.492M5.754 8a2.246 2.246 0 1 1 4.492 0 2.246 2.246 0 0 1-4.492 0"/><path fill="currentColor" stroke="none" fill-rule="evenodd" d="M9.796 1.343c-.527-1.79-3.065-1.79-3.592 0l-.094.319a.873.873 0 0 1-1.255.52l-.292-.16c-1.64-.892-3.433.902-2.54 2.541l.159.292a.873.873 0 0 1-.52 1.255l-.319.094c-1.79.527-1.79 3.065 0 3.592l.319.094a.873.873 0 0 1 .52 1.255l-.16.292c-.892 1.64.901 3.434 2.541 2.54l.292-.159a.873.873 0 0 1 1.255.52l.094.319c.527 1.79 3.065 1.79 3.592 0l.094-.319a.873.873 0 0 1 1.255-.52l.292.16c1.64.893 3.434-.902 2.54-2.541l-.159-.292a.873.873 0 0 1 .52-1.255l.319-.094c1.79-.527 1.79-3.065 0-3.592l-.319-.094a.873.873 0 0 1-.52-1.255l.16-.292c.893-1.64-.902-3.433-2.541-2.54l-.292.159a.873.873 0 0 1-1.255-.52zm-2.633.283c.246-.835 1.428-.835 1.674 0l.094.319a1.873 1.873 0 0 0 2.693 1.115l.291-.16c.764-.415 1.6.42 1.184 1.185l-.159.292a1.873 1.873 0 0 0 1.116 2.692l.318.094c.835.246.835 1.428 0 1.674l-.319.094a1.873 1.873 0 0 0-1.115 2.693l.16.291c.415.764-.42 1.6-1.185 1.184l-.291-.159a1.873 1.873 0 0 0-2.693 1.116l-.094.318c-.246.835-1.428.835-1.674 0l-.094-.319a1.873 1.873 0 0 0-2.692-1.115l-.292.16c-.764.415-1.6-.42-1.184-1.185l.159-.291A1.873 1.873 0 0 0 1.945 8.93l-.319-.094c-.835-.246-.835-1.428 0-1.674l.319-.094A1.873 1.873 0 0 0 3.06 4.376l-.16-.292c-.415-.764.42-1.6 1.185-1.184l.292.159a1.873 1.873 0 0 0 2.692-1.115z"/>"#
        }
        "search" => r#"<circle cx="7" cy="7" r="4.5"/><path d="M10.4 10.4l3 3"/>"#,
        // Half-filled circle — light/dark theme toggle.
        "contrast" => {
            r#"<circle cx="8" cy="8" r="6"/><path d="M8 2a6 6 0 0 0 0 12z" fill="currentColor" stroke="none"/>"#
        }
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
        "grid" => {
            // 2×2 filled squares — the agent-view grid icon.
            r#"<g fill="currentColor" stroke="none"><rect x="2" y="2" width="5" height="5" rx="1"/><rect x="9" y="2" width="5" height="5" rx="1"/><rect x="2" y="9" width="5" height="5" rx="1"/><rect x="9" y="9" width="5" height="5" rx="1"/></g>"#
        }
        // Circular arrow — restart / relaunch.
        "restart" => r#"<path d="M13 8a5 5 0 1 1-1.7-3.75M13 2.5V5h-2.5"/>"#,
        // Counter-clockwise arrow with a corner tick — "restore" (un-archive).
        // Distinct from `restart`, which spins the other way and reads as
        // "relaunch this session".
        "restore" => r#"<path d="M3.2 8a4.8 4.8 0 1 0 1.5-3.5"/><path d="M2.6 2.6v3.1h3.1"/>"#,
        "edit" => r#"<path d="M11.5 2.5l2 2L6 12l-2.5.5L4 10z"/>"#,
        // I-beam text cursor — "rename" (distinct from the "edit" pencil).
        "rename" => r#"<path d="M5 3h6M8 3v10M5 13h6"/>"#,
        // Two overlapping sheets — "duplicate".
        "duplicate" => {
            r#"<rect x="3" y="5.5" width="8" height="8" rx="1"/><path d="M6 5.5V3.5a1 1 0 0 1 1-1h5a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1h-2"/>"#
        }
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
        // Git branch — two nodes on a trunk plus a branch node.
        "git-branch" => {
            r#"<circle cx="4.5" cy="3.5" r="1.5"/><circle cx="4.5" cy="12.5" r="1.5"/><circle cx="11.5" cy="3.5" r="1.5"/><path d="M11.5 5v1.5a3 3 0 0 1-3 3H4.5M4.5 5v6"/>"#
        }
        // Generic git commit — a node on a horizontal line.
        "git" => r#"<circle cx="8" cy="8" r="2.5"/><path d="M1.5 8h4M10.5 8h4"/>"#,
        // Git branch with a slash through it — git disabled.
        "no-git" => {
            r#"<circle cx="4.5" cy="3.5" r="1.5"/><circle cx="4.5" cy="12.5" r="1.5"/><circle cx="11.5" cy="3.5" r="1.5"/><path d="M11.5 5v1.5a3 3 0 0 1-3 3H4.5M4.5 5v6"/><path d="M2.5 13.5l11-11"/>"#
        }
        // Simple folder outline.
        "folder" => {
            r#"<path d="M2 4.5a1 1 0 0 1 1-1h3l1.2 1.5H13a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z"/>"#
        }
        // Open folder outline.
        "folder-open" => {
            r#"<path d="M2 12V4.5a1 1 0 0 1 1-1h3l1.2 1.5H12a1 1 0 0 1 1 1V7"/><path d="M2 12l1.7-4.2a1 1 0 0 1 .93-.63h9.1a.6.6 0 0 1 .56.82L14 12a1 1 0 0 1-.93.63H3a1 1 0 0 1-1-.63z"/>"#
        }
        // ⌘ command / place-of-interest mark — macOS shortcut modifier.
        // Source: Bootstrap Icons (16×16 grid, fill-based like claude/codex).
        "command" => {
            r#"<path fill="currentColor" stroke="none" d="M3.5 2A1.5 1.5 0 0 1 5 3.5V5H3.5a1.5 1.5 0 1 1 0-3M6 5V3.5A2.5 2.5 0 1 0 3.5 6H5v4H3.5A2.5 2.5 0 1 0 6 12.5V11h4v1.5a2.5 2.5 0 1 0 2.5-2.5H11V6h1.5A2.5 2.5 0 1 0 10 3.5V5zm4 1v4H6V6zm1-1V3.5A1.5 1.5 0 1 1 12.5 5zm0 6h1.5a1.5 1.5 0 1 1-1.5 1.5zm-6 0v1.5A1.5 1.5 0 1 1 3.5 11zM5 5H3.5A1.5 1.5 0 0 1 5 3.5z"/>"#
        }
        // Four-point sparkle — initialize / new.
        "sparkle" => {
            r#"<path d="M8 2.5c.6 2.6 1.9 3.9 4.5 4.5C9.9 7.6 8.6 8.9 8 11.5 7.4 8.9 6.1 7.6 3.5 7 6.1 6.4 7.4 5.1 8 2.5z" fill="currentColor" stroke="none"/>"#
        }
        _ => "",
    };
    wrap_svg(inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sidebar_glyph_has_a_shape() {
        // The names the sidebar rows and header actually ask for.
        for name in [
            "plus",
            "chev-down",
            "chev-right",
            "collapse-all",
            "expand-all",
            "expand-sessions",
            "git",
            "no-git",
            "play",
            "term",
            "more",
            "trash",
            "close",
            "check",
            "dot",
            "ring",
            "question",
            "restart",
        ] {
            let svg = svg_for(name);
            assert!(svg.starts_with("<svg"), "{name} is not an svg");
            assert!(
                svg.len() > wrap_svg("").len(),
                "{name} has an empty sprite body"
            );
        }
    }

    #[test]
    fn an_unknown_name_degrades_to_an_empty_svg_rather_than_a_panic() {
        assert_eq!(svg_for("no-such-icon"), wrap_svg(""));
    }

    #[test]
    fn asset_paths_resolve_only_under_the_icons_prefix() {
        assert!(svg_for_path("icons/plus.svg").is_some());
        assert!(svg_for_path("fonts/BlexMono.ttf").is_none());
        assert!(svg_for_path("icons/plus").is_none());
        assert!(svg_for_path("plus.svg").is_none());
    }

    #[test]
    fn all_twelve_spinner_frames_resolve_and_differ() {
        let frames: Vec<String> = (0..SPINNER_FRAMES)
            .map(|f| {
                let Some(svg) = svg_for_path(&format!("icons/spinner-{f}.svg")) else {
                    unreachable!("frame {f} must resolve")
                };
                svg
            })
            .collect();
        assert_eq!(frames.len(), 12);
        // 360/12 = 30 degrees per step, so no two frames share a transform.
        for f in 1..frames.len() {
            assert_ne!(frames[0], frames[f]);
        }
        assert!(svg_for_path(&format!("icons/spinner-{SPINNER_FRAMES}.svg")).is_none());
        assert!(svg_for_path("icons/spinner-x.svg").is_none());
    }

    /// Parity with `src/gui/icons.rs:136-138`: one step every 3 ticks.
    #[test]
    fn the_spinner_advances_every_three_ticks() {
        assert_eq!(spinner_frame(0), spinner_frame(2));
        assert_ne!(spinner_frame(2), spinner_frame(3));
    }
}
