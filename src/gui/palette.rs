//! GUI color tokens, derived live from the active [`crate::theme`].
//!
//! The shared theme exposes a flat `Theme` with `bg / bg_highlight / fg /
//! fg_dark / comment` plus six accents. The GUI uses a richer surface
//! vocabulary (rail, strip, hover, two border weights), so the missing tokens
//! are synthesized by blending the base theme colors at fixed ratios.
//!
//! All accessors read [`crate::theme::current()`] on each call, so swapping
//! themes at runtime takes effect on the next frame.

#![allow(non_snake_case, dead_code)]

use crate::theme;
use iced::Color;

/// Public so the theme editor (`view.rs`) can render arbitrary draft-theme
/// swatches directly, without going through `theme::current()`.
pub fn ic(c: theme::Color) -> Color {
    match c {
        theme::Color::Rgb(r, g, b) => {
            Color::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
        }
    }
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color::from_rgb(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
    )
}

fn is_dark() -> bool {
    theme::with_current(|t| matches!(t.kind, theme::ThemeKind::Dark))
}

fn base_bg() -> Color {
    theme::with_current(|t| ic(t.bg))
}
fn base_fg() -> Color {
    theme::with_current(|t| ic(t.fg))
}

// ── surfaces ─────────────────────────────────────────────────────────────

pub fn BG() -> Color {
    base_bg()
}

/// Rail / sidebar — slightly darker than BG on dark themes, slightly
/// off-white on light themes.
pub fn BG_RAIL() -> Color {
    let bg = base_bg();
    if is_dark() {
        mix(bg, Color::BLACK, 0.18)
    } else {
        mix(bg, Color::BLACK, 0.04)
    }
}

/// Outer strip / chrome edge — darker than rail.
pub fn BG_STRIP() -> Color {
    let bg = base_bg();
    if is_dark() {
        mix(bg, Color::BLACK, 0.32)
    } else {
        mix(bg, Color::BLACK, 0.08)
    }
}

/// Hover surface — partway between bg and bg_highlight.
pub fn BG_HOVER() -> Color {
    mix(base_bg(), theme::with_current(|t| ic(t.bg_highlight)), 0.55)
}

/// Active / selected row.
pub fn BG_HL() -> Color {
    theme::with_current(|t| ic(t.bg_highlight))
}

pub fn BORDER() -> Color {
    mix(base_bg(), base_fg(), 0.16)
}
pub fn BORDER_SOFT() -> Color {
    mix(base_bg(), base_fg(), 0.07)
}

// ── overlays ─────────────────────────────────────────────────────────────

/// Modal scrim: a translucent wash derived from the theme rather than a
/// fixed black. Dark themes dim toward black; light themes dim toward the
/// foreground so the wash stays visible on near-white backgrounds.
pub fn SCRIM() -> Color {
    let base = if is_dark() {
        mix(base_bg(), Color::BLACK, 0.9)
    } else {
        mix(base_bg(), base_fg(), 0.9)
    };
    Color { a: 0.16, ..base }
}

// ── text ─────────────────────────────────────────────────────────────────

pub fn FG() -> Color {
    base_fg()
}
pub fn FG_DIM() -> Color {
    theme::with_current(|t| ic(t.fg_dark))
}
pub fn FG_MUTE() -> Color {
    theme::with_current(|t| ic(t.comment))
}

// ── accents ──────────────────────────────────────────────────────────────

pub fn BLUE() -> Color {
    theme::with_current(|t| ic(t.blue))
}
pub fn CYAN() -> Color {
    theme::with_current(|t| ic(t.cyan))
}
pub fn MAGENTA() -> Color {
    theme::with_current(|t| ic(t.magenta))
}
pub fn GREEN() -> Color {
    theme::with_current(|t| ic(t.green))
}
/// Attention amber — the "needs input" accent. Warmer than YELLOW so it
/// reads as a call to action next to green/working.
pub fn AMBER() -> Color {
    theme::with_current(|t| mix(ic(t.yellow), ic(t.red), 0.25))
}
pub fn YELLOW() -> Color {
    theme::with_current(|t| ic(t.yellow))
}
pub fn RED() -> Color {
    theme::with_current(|t| ic(t.red))
}

/// A 16% wash of RED over BG — the active fill for a danger-flavored
/// segmented control (e.g. "skip permissions"), distinct from the neutral
/// `BG_HL()` used by ordinary active segments.
pub fn RED_WASH() -> Color {
    mix(RED(), BG(), 0.84)
}

// ── selection (focused Miller column) ──────────────────────────────────────
// The launcher's active column marks its selected row with a cyan-tinted
// gradient fill, a cyan ring, and a left accent bar. Derived from the theme's
// cyan so the treatment tracks theme swaps.

/// Stronger end of the selected-row gradient (left edge).
pub fn SEL_TINT_STRONG() -> Color {
    Color { a: 0.22, ..CYAN() }
}
/// Softer end of the selected-row gradient (right edge).
pub fn SEL_TINT_SOFT() -> Color {
    Color { a: 0.10, ..CYAN() }
}
/// Ring outlining the selected row in the focused column.
pub fn SEL_RING() -> Color {
    Color { a: 0.5, ..CYAN() }
}

// ── theme-parameterized variants ────────────────────────────────────────────
// Used to render PTY *content* (background fill, default fg, cursor, ANSI
// 0-15) under a per-project "Project theme" override, decoupled from the
// global `theme::current()` that the `c::*` accessors above read. App chrome
// (tile header, borders, rail, appbar) always uses the plain accessors above
// and is unaffected by a project's pinned theme.

fn is_dark_of(t: &theme::Theme) -> bool {
    matches!(t.kind, theme::ThemeKind::Dark)
}

pub fn bg_of(t: &theme::Theme) -> Color {
    ic(t.bg)
}
pub fn fg_of(t: &theme::Theme) -> Color {
    ic(t.fg)
}
pub fn fg_mute_of(t: &theme::Theme) -> Color {
    ic(t.comment)
}
pub fn blue_of(t: &theme::Theme) -> Color {
    ic(t.blue)
}
pub fn cyan_of(t: &theme::Theme) -> Color {
    ic(t.cyan)
}
pub fn magenta_of(t: &theme::Theme) -> Color {
    ic(t.magenta)
}
pub fn green_of(t: &theme::Theme) -> Color {
    ic(t.green)
}
pub fn yellow_of(t: &theme::Theme) -> Color {
    ic(t.yellow)
}
pub fn red_of(t: &theme::Theme) -> Color {
    ic(t.red)
}

/// Themed variant of `BG_STRIP` — used for ANSI color 0 inside PTY content
/// rendered under a per-project override theme.
pub fn bg_strip_of(t: &theme::Theme) -> Color {
    let bg = ic(t.bg);
    if is_dark_of(t) {
        mix(bg, Color::BLACK, 0.32)
    } else {
        mix(bg, Color::BLACK, 0.08)
    }
}

/// Themed variant of `BG_HL` — the theme editor's "derived — not editable"
/// strip synthesizes these from a draft `Theme` that isn't (and may never
/// be) the active theme, so it needs the same blends as `BG_HL`/`BORDER`/
/// `BG_HOVER`/`SEL_RING` parameterized rather than reading `theme::current()`.
pub fn bg_hl_of(t: &theme::Theme) -> Color {
    ic(t.bg_highlight)
}
/// Themed variant of `BG_HOVER`.
pub fn bg_hover_of(t: &theme::Theme) -> Color {
    mix(bg_of(t), bg_hl_of(t), 0.55)
}
/// Themed variant of `BORDER`.
pub fn border_of(t: &theme::Theme) -> Color {
    mix(bg_of(t), fg_of(t), 0.16)
}
/// Themed variant of `SEL_RING`.
pub fn sel_ring_of(t: &theme::Theme) -> Color {
    Color {
        a: 0.5,
        ..cyan_of(t)
    }
}
