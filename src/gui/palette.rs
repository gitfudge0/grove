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

fn ic(c: theme::Color) -> Color {
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
    matches!(theme::current().kind, theme::ThemeKind::Dark)
}

fn base_bg() -> Color {
    ic(theme::current().bg)
}
fn base_fg() -> Color {
    ic(theme::current().fg)
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
    mix(base_bg(), ic(theme::current().bg_highlight), 0.55)
}

/// Active / selected row.
pub fn BG_HL() -> Color {
    ic(theme::current().bg_highlight)
}

pub fn BORDER() -> Color {
    mix(base_bg(), base_fg(), 0.16)
}
pub fn BORDER_SOFT() -> Color {
    mix(base_bg(), base_fg(), 0.07)
}

// ── text ─────────────────────────────────────────────────────────────────

pub fn FG() -> Color {
    base_fg()
}
pub fn FG_DIM() -> Color {
    ic(theme::current().fg_dark)
}
pub fn FG_MUTE() -> Color {
    ic(theme::current().comment)
}

// ── accents ──────────────────────────────────────────────────────────────

pub fn BLUE() -> Color {
    ic(theme::current().blue)
}
pub fn CYAN() -> Color {
    ic(theme::current().cyan)
}
pub fn MAGENTA() -> Color {
    ic(theme::current().magenta)
}
pub fn GREEN() -> Color {
    ic(theme::current().green)
}
pub fn YELLOW() -> Color {
    ic(theme::current().yellow)
}
pub fn RED() -> Color {
    ic(theme::current().red)
}
