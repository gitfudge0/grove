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

// ── settings-row geometry (shared by App Settings and Project Settings) ───
//
// A settings row is sized by padding around its content, on the same
// `RowDensity::Card` precedent DESIGN.md §9.1 already documents ("its height
// from its content rather than from padding") — there is no fixed row
// height. `ROW_MIN_H` is a floor only, expressed as `min_h`, never `h()`.

/// A settings row's horizontal padding, both edges.
pub const ROW_PX: f32 = SPACE_2XL;
/// A settings row's vertical padding, top and bottom.
pub const ROW_PY: f32 = SPACE_LG;
/// The gap between a row's label line and its sublabel line — the same notch
/// [`SPACE_MD`] is everywhere else, named here because a settings row's
/// vertical rhythm is a repeated call site, not a one-off `gap`.
pub const ROW_LINE_GAP: f32 = SPACE_MD;
/// A settings row's height FLOOR: [`CONTROL_H`] (22) plus [`ROW_PY`] (8)
/// top+bottom, so a row containing a 22px control can never collapse smaller
/// than it fits. A row's real height falls out of its content plus
/// `ROW_PY` — a single-line row measures `ROW_MIN_H` (38); a label+sublabel
/// row measures `ROW_PY + CONTROL_H + ROW_LINE_GAP + TEXT_SMALL + ROW_PY` (55)
/// and a sublabel that wraps to two lines grows the row instead of
/// overflowing it. This constant is never a fixed height and never an `h()`.
pub const ROW_MIN_H: f32 = CONTROL_H + ROW_PY * 2.0;

/// Both settings modals' body scrolls before the panel outgrows a laptop
/// viewport. Rows vary in height now (they size by content, not a fixed
/// row height), so this is no longer "N whole rows" — it is a cap on the
/// body, expressed as a multiple of [`ROW_MIN_H`] so the clip line still
/// lands near a row boundary rather than at an arbitrary pixel. It is a
/// **maximum**, not a layout: real content may be shorter or (per-row)
/// taller than this multiple would suggest.
pub const MODAL_SCROLL_MAX_H: f32 = ROW_MIN_H * 12.0;

/// The settings-row label column's fixed width, wide enough for the longest
/// label ("Teardown") without wasting width on the row's dominant flex-1
/// control. Shared by every `setting_row_field`, so the three script rows'
/// (and any future field row's) labels align.
pub const FIELD_LABEL_COL_W: f32 = 92.0;

/// The leading status-dot column reserved on every settings row: [`DOT_MD`]
/// plus a little breathing room on each side, so a row's label starts at the
/// same x whether or not that row happens to carry a status dot.
pub const STATUS_DOT_COL_W: f32 = DOT_MD + SPACE_SM * 2.0;

/// The square box of a header/card-head icon button (the modal close button,
/// the header pencil, the Tools card's refresh icon).
pub const ICON_BTN_W: f32 = 28.0;
/// The narrower icon button used in-row (the footer's small refresh icon, the
/// Project Settings rename check/discard pair).
pub const ICON_BTN_W_SM: f32 = 24.0;
/// The stepper buttons inside the App-size segmented group.
pub const STEPPER_BTN_W: f32 = 20.0;

/// The Settings cards' label indent: flush with the card's left edge plus the
/// row's own inset. `card()`'s hairline `border_1()` (1px) plus a row's own
/// `px(SPACE_2XL)` (12px) = 13px from the card's outer edge. Tracks the row's
/// horizontal padding — if that padding moves off `SPACE_2XL`, this must move
/// with it or the label drifts 1px out of true against the rows it names.
pub const CARD_LABEL_INDENT: f32 = SPACE_2XL + 1.0;
