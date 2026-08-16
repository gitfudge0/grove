//! The design system's numeric scales, in design pixels (feed them to [`crate::views::rpx`]). See DESIGN.md; every call site must use a token, never a bare literal.

#![allow(dead_code)]

pub const SPACE_XS: f32 = 2.0;
pub const SPACE_SM: f32 = 4.0;
pub const SPACE_MD: f32 = 6.0;
pub const SPACE_LG: f32 = 8.0;
pub const SPACE_XL: f32 = 10.0;
pub const SPACE_2XL: f32 = 12.0;
pub const SPACE_3XL: f32 = 16.0;

pub const TEXT_MICRO: f32 = 10.0;
pub const TEXT_SMALL: f32 = 11.0;
pub const TEXT_BODY: f32 = 12.0;
pub const TEXT_TITLE: f32 = 13.0;
/// Empty-state / onboarding display type only — never app chrome.
pub const TEXT_DISPLAY: f32 = 20.0;
pub const TEXT_DISPLAY_LG: f32 = 32.0;

/// Smallest legible glyph — chips, footer hints, keycaps.
pub const ICON_XS: f32 = 10.0;
/// Default list-density glyph: rows, menu items.
pub const ICON_SM: f32 = 12.0;
/// Chrome glyphs: appbar, session header, term panel, settings rows.
pub const ICON_MD: f32 = 14.0;
/// Largest chrome glyph and the ceiling for anything clickable.
pub const ICON_LG: f32 = 16.0;
/// Empty-state / onboarding display glyph only — never app chrome.
pub const ICON_DISPLAY: f32 = 32.0;

/// Row and tab activity dots, denser than [`DOT_MD`].
pub const DOT_SM: f32 = 6.0;
/// Statusbar/appbar pill dots, standing alone.
pub const DOT_MD: f32 = 7.0;

/// Confirmations and single-question modals.
pub const MODAL_W_SM: f32 = 420.0;
/// The default: a form or short list of rows.
pub const MODAL_W_MD: f32 = 480.0;
/// Rows with a secondary column (path, hint, trailing control).
pub const MODAL_W_LG: f32 = 560.0;
/// The widest panel: command palette and Settings. Nothing above this.
pub const MODAL_W_XL: f32 = 640.0;

pub const RADIUS_CONTROL: f32 = 4.0;
pub const RADIUS_GROUP: f32 = 6.0;
pub const RADIUS_PANEL: f32 = 12.0;
pub const RADIUS_FULL: f32 = 999.0;
/// The theme-swatch corner, below [`RADIUS_CONTROL`] — a 10px chip at that radius reads as a circle.
pub const SWATCH_RADIUS: f32 = 2.0;

/// A field's vertical padding — the field paints nothing, so this only sets the
/// text's line box, keeping it at ~22px, one notch above [`CONTROL_H`], so a row
/// hosting a field is the same height as one hosting a value.
pub const FIELD_PY: f32 = 3.0;

/// A panel shadow's vertical offset on dark themes; paired with `crate::theme::PANEL_SHADOW`.
pub const PANEL_SHADOW_Y: f32 = 12.0;
/// A panel shadow's blur radius on dark themes.
pub const PANEL_SHADOW_BLUR: f32 = 40.0;
/// [`PANEL_SHADOW_Y`] on light themes (lighter and tighter than dark).
pub const PANEL_SHADOW_Y_LIGHT: f32 = 6.0;
/// [`PANEL_SHADOW_BLUR`] on light themes.
pub const PANEL_SHADOW_BLUR_LIGHT: f32 = 24.0;

/// Flat icon/text buttons and tile headers.
pub const CONTROL_H: f32 = 22.0;

/// A settings row's horizontal padding, both edges.
pub const ROW_PX: f32 = SPACE_2XL;
/// A settings row's vertical padding, top and bottom.
pub const ROW_PY: f32 = SPACE_LG;
/// Gap between a row's label and sublabel line.
pub const ROW_LINE_GAP: f32 = SPACE_MD;
/// Floor only (`min_h`, never `h()`) — a row's real height grows from its content.
pub const ROW_MIN_H: f32 = CONTROL_H + ROW_PY * 2.0;

/// The scroll cap for every scrolling modal body, a property of the window, not of any one modal.
pub const MODAL_SCROLL_MAX_H: f32 = ROW_MIN_H * 12.0;

/// The diff file-list column's floor width; real width is clamped between this and [`DIFF_FILE_LIST_MAX_FRAC`].
pub const DIFF_FILE_LIST_W: f32 = 240.0;

/// A draggable divider's hit-zone, wider than the 1px rule it draws (`src/gui/metrics.rs:20`).
pub const DIVIDER_DRAG_HIT_W: f32 = 6.0;

/// Ceiling on the diff file-list column so a deep path can't starve the body width into unified mode.
pub const DIFF_FILE_LIST_MAX_FRAC: f32 = 0.4;

/// The diff viewer's inset from the window edge — a viewport-filling surface, not a `MODAL_W_*` step.
pub const DIFF_PANEL_INSET: f32 = SPACE_3XL;

/// The diff body's line-number gutter width, right-aligned.
pub const DIFF_GUTTER_W: f32 = 44.0;

/// Below this window width, split mode is disabled and unified is forced.
pub const DIFF_SPLIT_MIN_W: f32 = 900.0;

/// A diff body line's fixed height, so split-mode fillers and unified rows/headers stay uniform for `uniform_list`.
pub const DIFF_BODY_LINE_H: f32 = TEXT_SMALL + SPACE_SM;

/// The settings-row label column's fixed width, wide enough for the longest label ("Teardown").
pub const FIELD_LABEL_COL_W: f32 = 92.0;

/// The leading status-dot column, reserved so a row's label starts at the same x with or without a dot.
pub const STATUS_DOT_COL_W: f32 = DOT_MD + SPACE_SM * 2.0;

/// The square box of a header/card-head icon button.
pub const ICON_BTN_W: f32 = 28.0;
/// The narrower icon button used in-row.
pub const ICON_BTN_W_SM: f32 = 24.0;
/// The stepper buttons inside the App-size segmented group.
pub const STEPPER_BTN_W: f32 = 20.0;

/// The needs-input accent bar; overlaid (not `border_l`) so it never shifts content.
pub const ATTENTION_BAR_W: f32 = 3.0;

/// A session card's headline line; carries a control, so it takes [`CONTROL_H`].
pub const CARD_LINE_H: f32 = CONTROL_H;
/// A session card's secondary line height (worktree/status, and meta line).
pub const CARD_LINE_SM_H: f32 = TEXT_SMALL + SPACE_SM;
/// Number of [`CARD_LINE_SM_H`] lines under the headline.
pub const CARD_SM_LINES: f32 = 2.0;
/// A session card's rendered height, arithmetic on its real parts — this is what `TreeRow::height` returns.
pub const SESSION_CARD_H: f32 = SPACE_LG * 2.0
    + CARD_LINE_H
    + ROW_LINE_GAP * CARD_SM_LINES
    + CARD_LINE_SM_H * CARD_SM_LINES
    + 1.0 * 2.0;

/// The Settings cards' label indent: card hairline (1px) + row inset (`SPACE_2XL`) = 13px.
pub const CARD_LABEL_INDENT: f32 = SPACE_2XL + 1.0;
