//! The design system's numeric scales, in design pixels (feed them to
//! [`crate::views::rpx`]). See DESIGN.md for the rationale and the mapping
//! rules; every new call site must use a token rather than a bare literal.

// Not every token has a caller yet.
#![allow(dead_code)]

// ── spacing: gaps, padding, margins ──────────────────────────────────────
pub const SPACE_XS: f32 = 2.0;
pub const SPACE_SM: f32 = 4.0;
pub const SPACE_MD: f32 = 6.0;
pub const SPACE_LG: f32 = 8.0;
pub const SPACE_XL: f32 = 10.0;
pub const SPACE_2XL: f32 = 12.0;
pub const SPACE_3XL: f32 = 16.0;

// ── type sizes ───────────────────────────────────────────────────────────
pub const TEXT_MICRO: f32 = 10.0;
pub const TEXT_SMALL: f32 = 11.0;
pub const TEXT_BODY: f32 = 12.0;
pub const TEXT_TITLE: f32 = 13.0;
/// Empty-state / onboarding display type only — never app chrome.
pub const TEXT_DISPLAY: f32 = 20.0;
pub const TEXT_DISPLAY_LG: f32 = 32.0;

// ── icon glyph sizes ─────────────────────────────────────────────────────
/// The smallest legible glyph — inline chips, footer hints, keycap glyphs.
/// Below [`ICON_SM`]; anything smaller stops reading as a mark.
pub const ICON_XS: f32 = 10.0;
/// Row glyphs and menu items — the default list-density glyph, between
/// [`ICON_XS`] and [`ICON_MD`].
pub const ICON_SM: f32 = 12.0;
/// Chrome glyphs: the appbar, the session header, the term panel and the
/// settings rows. Between [`ICON_SM`] and [`ICON_LG`].
pub const ICON_MD: f32 = 14.0;
/// The largest chrome glyph — palette rows and empty-state marks, above
/// [`ICON_MD`] and the ceiling for anything the user clicks.
pub const ICON_LG: f32 = 16.0;
/// Empty-state / onboarding display glyph only — never app chrome. This is a
/// rule, not a note: the four tiers above are the whole chrome vocabulary.
pub const ICON_DISPLAY: f32 = 32.0;

// ── activity dot sizes ───────────────────────────────────────────────────
/// Row and tab activity dots — the denser of the two, for dots that sit
/// inside a list row or a tab label.
pub const DOT_SM: f32 = 6.0;
/// Statusbar and appbar pill dots — one notch up from [`DOT_SM`], so a dot
/// standing alone in a bar still reads at a glance.
pub const DOT_MD: f32 = 7.0;

// ── modal panel widths ───────────────────────────────────────────────────
/// Confirmations and single-question modals — one short paragraph, no list.
pub const MODAL_W_SM: f32 = 420.0;
/// The default: a form or a short list of rows, between [`MODAL_W_SM`] and
/// [`MODAL_W_LG`].
pub const MODAL_W_MD: f32 = 480.0;
/// Modals whose rows carry a secondary column — a path, a hint, a trailing
/// control — and would otherwise truncate at [`MODAL_W_MD`].
pub const MODAL_W_LG: f32 = 560.0;
/// The Project Settings panel: an editable-title header plus a compact
/// lifecycle-scripts table, between [`MODAL_W_LG`] and [`MODAL_W_XL`].
pub const MODAL_W_LG2: f32 = 600.0;
/// The widest panel: the command palette and Settings, which host a scrolling
/// results list rather than a form. Nothing goes above this.
pub const MODAL_W_XL: f32 = 640.0;

// ── corner radii ─────────────────────────────────────────────────────────
pub const RADIUS_CONTROL: f32 = 4.0;
pub const RADIUS_GROUP: f32 = 6.0;
pub const RADIUS_PANEL: f32 = 12.0;
pub const RADIUS_FULL: f32 = 999.0;

// ── control heights ──────────────────────────────────────────────────────
/// Flat icon/text buttons and tile headers.
pub const CONTROL_H: f32 = 22.0;
