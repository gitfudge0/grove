//! The design system's numeric scales, in design pixels (feed them to [`crate::views::rpx`]). See DESIGN.md for the rationale and the mapping rules; every new call site must use a token rather than a bare literal.

// File-level by design: this module is a design-token scale. A scale is declared in full so call sites pick a step rather than invent a literal; unused steps are what makes it a scale.
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
/// The smallest legible glyph — inline chips, footer hints, keycap glyphs. Below [`ICON_SM`]; anything smaller stops reading as a mark.
pub const ICON_XS: f32 = 10.0;
/// Row glyphs and menu items — the default list-density glyph, between [`ICON_XS`] and [`ICON_MD`].
pub const ICON_SM: f32 = 12.0;
/// Chrome glyphs: the appbar, the session header, the term panel and the settings rows. Between [`ICON_SM`] and [`ICON_LG`].
pub const ICON_MD: f32 = 14.0;
/// The largest chrome glyph — palette rows and empty-state marks, above [`ICON_MD`] and the ceiling for anything the user clicks.
pub const ICON_LG: f32 = 16.0;
/// Empty-state / onboarding display glyph only — never app chrome. This is a rule, not a note: the four tiers above are the whole chrome vocabulary.
pub const ICON_DISPLAY: f32 = 32.0;

// ── activity dot sizes ───────────────────────────────────────────────────
/// Row and tab activity dots — the denser of the two, for dots that sit inside a list row or a tab label.
pub const DOT_SM: f32 = 6.0;
/// Statusbar and appbar pill dots — one notch up from [`DOT_SM`], so a dot standing alone in a bar still reads at a glance.
pub const DOT_MD: f32 = 7.0;

// ── modal panel widths ───────────────────────────────────────────────────
/// Confirmations and single-question modals — one short paragraph, no list.
pub const MODAL_W_SM: f32 = 420.0;
/// The default: a form or a short list of rows, between [`MODAL_W_SM`] and [`MODAL_W_LG`].
pub const MODAL_W_MD: f32 = 480.0;
/// Modals whose rows carry a secondary column — a path, a hint, a trailing control — and would otherwise truncate at [`MODAL_W_MD`].
pub const MODAL_W_LG: f32 = 560.0;
/// The widest panel: the command palette and Settings, which host a scrolling results list rather than a form. Nothing goes above this.
pub const MODAL_W_XL: f32 = 640.0;

// ── corner radii ─────────────────────────────────────────────────────────
pub const RADIUS_CONTROL: f32 = 4.0;
pub const RADIUS_GROUP: f32 = 6.0;
pub const RADIUS_PANEL: f32 = 12.0;
pub const RADIUS_FULL: f32 = 999.0;
/// The theme-swatch chip's corner — below [`RADIUS_CONTROL`], because a swatch is a colour sample a few pixels wide and the control radius on a 10px chip reads as a circle. On the scale rather than a `theme_picker` local so the radius vocabulary (2 swatches · 4 controls/cards · 6 rows/groups/fields · 12 panel) is declared in one place (plan.md §3).
pub const SWATCH_RADIUS: f32 = 2.0;

// ── text-field geometry (plan.md §1, variant C1c "boxed + focus ring") ────
/// A boxed field's horizontal padding, both edges.
pub const FIELD_PX: f32 = SPACE_XL;
/// A boxed field's vertical padding, top and bottom. With a [`TEXT_BODY`] mono run plus the box's 1px border on each side this measures ~22px tall — the control tier one notch above [`CONTROL_H`], which is what a field needs to carry a border and a ring without crowding its own text.
pub const FIELD_PY: f32 = 2.0;

// ── panel drop-shadow geometry (plan.md §3) ────────────────────────────── Paired with `crate::theme::PANEL_SHADOW`, whose alpha forks the same way. A light theme's shadow is lighter *and* tighter: a long, soft shadow that reads as depth on a dark page reads as smudge on a bright one.

/// A panel shadow's vertical offset on dark themes.
pub const PANEL_SHADOW_Y: f32 = 12.0;
/// A panel shadow's blur radius on dark themes.
pub const PANEL_SHADOW_BLUR: f32 = 40.0;
/// [`PANEL_SHADOW_Y`] on light themes.
pub const PANEL_SHADOW_Y_LIGHT: f32 = 6.0;
/// [`PANEL_SHADOW_BLUR`] on light themes.
pub const PANEL_SHADOW_BLUR_LIGHT: f32 = 24.0;

// ── control heights ──────────────────────────────────────────────────────
/// Flat icon/text buttons and tile headers.
pub const CONTROL_H: f32 = 22.0;

// ── settings-row geometry (shared by App Settings and Project Settings) ─── A settings row is sized by padding around its content, on the same `RowDensity::Card` precedent DESIGN.md §9.1 already documents ("its height from its content rather than from padding") — there is no fixed row height. `ROW_MIN_H` is a floor only, expressed as `min_h`, never `h()`.

/// A settings row's horizontal padding, both edges.
pub const ROW_PX: f32 = SPACE_2XL;
/// A settings row's vertical padding, top and bottom.
pub const ROW_PY: f32 = SPACE_LG;
/// The gap between a row's label line and its sublabel line — the same notch [`SPACE_MD`] is everywhere else, named here because a settings row's vertical rhythm is a repeated call site, not a one-off `gap`.
pub const ROW_LINE_GAP: f32 = SPACE_MD;
/// A settings row's height FLOOR: [`CONTROL_H`] (22) plus [`ROW_PY`] (8) top+bottom, so a row containing a 22px control can never collapse smaller than it fits. A row's real height falls out of its content plus `ROW_PY` — a single-line row measures `ROW_MIN_H` (38); a label+sublabel row measures `ROW_PY + CONTROL_H + ROW_LINE_GAP + TEXT_SMALL + ROW_PY` (55) and a sublabel that wraps to two lines grows the row instead of overflowing it. This constant is never a fixed height and never an `h()`.
pub const ROW_MIN_H: f32 = CONTROL_H + ROW_PY * 2.0;

/// The scroll cap for **every** scrolling modal body — the one height at which a panel's body starts scrolling rather than growing past a laptop viewport. Settings was merely the first caller; a modal that scrolls a list, a changelog or a results column caps here too, because how tall a panel may grow is a property of the *window*, not of what a given modal happens to show. (The per-modal caps this replaced — a 360, a 420, a 452 — were three answers to one question, and their spread was drift, not intent.) Rows vary in height (they size by content, not a fixed row height), so this is not "N whole rows" — it is a cap on the body, expressed as a multiple of [`ROW_MIN_H`] so the clip line still lands near a row boundary rather than at an arbitrary pixel. It is a **maximum**, not a layout: real content may be shorter, or (per-row) taller, than the multiple suggests.
pub const MODAL_SCROLL_MAX_H: f32 = ROW_MIN_H * 12.0;

/// The diff viewer's file-list column floor width. The column itself is no longer fixed at this value — see [`crate::views::modals::diff_viewer::file_list_w`], which sizes it to its widest visible row, clamped between this floor and [`DIFF_FILE_LIST_MAX_FRAC`] of the window's width — but a column shorter than this would feel cramped even when every row is narrow, so it stays the minimum.
pub const DIFF_FILE_LIST_W: f32 = 240.0;

/// A draggable divider's hit-zone width — wider than the 1px rule it draws so the resize cursor has room to land. Shared by the sidebar/workspace divider and the diff viewer's file-list divider (`src/gui/metrics.rs:20`).
pub const DIVIDER_DRAG_HIT_W: f32 = 6.0;

/// Ceiling on the diff viewer's file-list column, as a fraction of the window's logical width. The column's width feeds [`crate::views::modals::diff_viewer::render`]'s body `content_w`, which [`crate::views::modals::diff_viewer::effective_mode`] gates [`DIFF_SPLIT_MIN_W`] on — without a ceiling, a long path in a deep tree could grow the file list wide enough to silently force unified mode by starving the body of width. See [`crate::views::modals::diff_viewer::file_list_w`].
pub const DIFF_FILE_LIST_MAX_FRAC: f32 = 0.4;

/// The diff viewer's inset from the window edge. The viewer is a viewport-filling surface (the same category as Onboarding) rather than a step on the `MODAL_W_*` scale — see `views::modals::diff_viewer::render`'s doc comment.
pub const DIFF_PANEL_INSET: f32 = SPACE_3XL;

/// The diff body's line-number gutter width, right-aligned.
pub const DIFF_GUTTER_W: f32 = 44.0;

/// Below this window width, split mode is disabled and unified is forced.
pub const DIFF_SPLIT_MIN_W: f32 = 900.0;

/// A diff body line's fixed height. In split mode it keeps a filler row (a half-empty pair's inert side) and a real code row measuring the same so the two columns stay row-aligned; in unified mode the same height is what makes every row — line rows and hunk headers alike — uniform, which is what lets the body be a `uniform_list`. Same shape as [`CARD_LINE_SM_H`]: [`TEXT_SMALL`], the run it carries, plus [`SPACE_SM`] of leading so the line box is never clipped by its own text.
pub const DIFF_BODY_LINE_H: f32 = TEXT_SMALL + SPACE_SM;

/// The settings-row label column's fixed width, wide enough for the longest label ("Teardown") without wasting width on the row's dominant flex-1 control. Shared by every `setting_row_field`, so the three script rows' (and any future field row's) labels align.
pub const FIELD_LABEL_COL_W: f32 = 92.0;

/// The leading status-dot column reserved on every settings row: [`DOT_MD`] plus a little breathing room on each side, so a row's label starts at the same x whether or not that row happens to carry a status dot.
pub const STATUS_DOT_COL_W: f32 = DOT_MD + SPACE_SM * 2.0;

/// The square box of a header/card-head icon button (the modal close button, the header pencil, the Tools card's refresh icon).
pub const ICON_BTN_W: f32 = 28.0;
/// The narrower icon button used in-row (the footer's small refresh icon, the Project Settings rename check/discard pair).
pub const ICON_BTN_W_SM: f32 = 24.0;
/// The stepper buttons inside the App-size segmented group.
pub const STEPPER_BTN_W: f32 = 20.0;

// ── the attention accent bar ─────────────────────────────────────────────
/// Width of the accent bar overlaid on a row (or a sessions-rail card) that needs input. Overlaid rather than a `border_l` so it never shifts the content it marks. Two call sites — the tree's session row and the rail's session card — which is why it lives on the scale rather than in `rows.rs`. (`src/views/appbar.rs` draws the same 3px bar as its own visual idea.)
pub const ATTENTION_BAR_W: f32 = 3.0;

// ── the sessions rail's card (mock D11 "diff-stat") ────────────────────── The card is three text lines inside one `RowDensity::Card` row. Every line declares a height, because [`crate::views::rows::TreeRow::height`] is the single height source for the list and it cannot measure text — a line that sized itself from its content would let the two disagree.

/// A session card's **headline** line: the agent glyph, the session title and the in-row kill button. It carries a control, so it is a control line and takes [`CONTROL_H`] — not a type-derived height.
pub const CARD_LINE_H: f32 = CONTROL_H;
/// A session card's **secondary** line height, used twice: the worktree + status line, and the `project · agent · elapsed` + diff-stat meta line. [`TEXT_SMALL`], the tallest run on either, plus [`SPACE_SM`] of leading so an 11px line box is never clipped by its own box.
pub const CARD_LINE_SM_H: f32 = TEXT_SMALL + SPACE_SM;
/// How many [`CARD_LINE_SM_H`] lines sit under the headline: the worktree line and the meta line. Named so the height arithmetic below stays a statement about the card's parts rather than a bare multiplier.
pub const CARD_SM_LINES: f32 = 2.0;
/// A session card's rendered height: [`SPACE_LG`] above and below the three lines, [`ROW_LINE_GAP`] between each pair, plus the card's own 1px hairline on the top and bottom edges. Every term is a token — this is the number `TreeRow::height` returns, so it must be arithmetic on the card's real parts rather than a measured or guessed figure.
pub const SESSION_CARD_H: f32 = SPACE_LG * 2.0
    + CARD_LINE_H
    + ROW_LINE_GAP * CARD_SM_LINES
    + CARD_LINE_SM_H * CARD_SM_LINES
    + 1.0 * 2.0;

/// The Settings cards' label indent: flush with the card's left edge plus the row's own inset. `card()`'s hairline `border_1()` (1px) plus a row's own `px(SPACE_2XL)` (12px) = 13px from the card's outer edge. Tracks the row's horizontal padding — if that padding moves off `SPACE_2XL`, this must move with it or the label drifts 1px out of true against the rows it names.
pub const CARD_LABEL_INDENT: f32 = SPACE_2XL + 1.0;
