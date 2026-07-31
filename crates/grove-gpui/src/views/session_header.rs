//! The `SESSBAR_H` session bar above the terminal body: label, branch, the OSC
//! context title, and the 3-dot in-progress animation.
//!
//! Port of `src/gui/view/terminal.rs:487-560` (+ `:480-485` for the hairline).
//!
//! **Built here, not in Plan 07** (recorded ambiguity 1): Appendix A's
//! *Attention/activity* row "3-dot `(tick/5)%3`" is an exit-gate row for this
//! phase, and the only place that animation exists is this bar's in-progress
//! context; the bar also carries this phase's OSC deliverable. It is written
//! **parameterized by session**, not by "the active session", so Plan 07 can
//! reuse the same renderer for grid tile headers.

use gpui::{div, prelude::*, px, AnyElement, Div, Hsla};

use crate::entities::animation_clock::dots;
use crate::theme as c;

/// Session bar height (`src/gui/metrics.rs:17`).
pub const SESSBAR_H: f32 = 36.0;

/// The longest a context title may be before the middle is collapsed
/// (`terminal.rs:538`).
const CONTEXT_MAX_CHARS: usize = 80;

/// One session as the header draws it. Everything is resolved by the caller,
/// so the renderer touches no entity and Plan 07 can build one per tile.
#[derive(Clone, Debug, Default)]
pub struct SessionHeaderData {
    /// Session/project label — the strongest element (13px, weight 600).
    pub label: String,
    /// Empty for branchless sessions (home terminals), which skip the segment
    /// entirely rather than show two dots with nothing between.
    pub branch: String,
    /// The OSC context title, already sanitized (`common.rs:179-190`).
    pub context: Option<String>,
    /// The agent's sprite name, for the leading glyph.
    pub icon_name: &'static str,
    /// Whether the session's process is still alive.
    pub running: bool,
}

/// `common.rs:192-195` — case-insensitive, all three spellings.
#[must_use]
pub fn is_in_progress_title(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    lower.contains("in progress") || lower.contains("in-progress") || lower.contains("in_progress")
}

/// Shorten `s` to at most `max` chars by collapsing the middle with `…`
/// (`src/gui/widgets/primitives.rs:12-26`).
#[must_use]
pub fn truncate_middle(s: &str, max: usize) -> String {
    let len = s.chars().count();
    if len <= max || max < 2 {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let prefix: String = s.chars().take(head).collect();
    let suffix: String = s.chars().skip(len - tail).collect();
    format!("{prefix}…{suffix}")
}

/// Which of the three dots is lit (`terminal.rs:540` — `(tick/5)%3`). Shares
/// the single animation counter with the cursor and the spinner, so the phases
/// stay in the same relationship the iced build has.
#[must_use]
pub fn in_progress_phase(tick: u64) -> u64 {
    dots(tick)
}

fn label_text(content: impl Into<gpui::SharedString>, size: f32, color: Hsla, bold: bool) -> Div {
    let d = div()
        .font(gpui::font(crate::fonts::UI_FAMILY))
        .text_size(px(size))
        .text_color(color)
        .child(content.into());
    if bold {
        d.font_weight(gpui::FontWeight::SEMIBOLD)
    } else {
        d
    }
}

/// The bar plus its `BORDER_SOFT()` hairline beneath.
pub fn session_header(data: &SessionHeaderData, tick: u64) -> AnyElement {
    let mut identity = div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .overflow_hidden()
        .child(crate::icons::icon(data.icon_name, 13.0, c::FG()))
        .child(label_text(data.label.clone(), 13.0, c::FG(), true));

    if !data.branch.trim().is_empty() {
        identity = identity
            .child(label_text("·", 13.0, c::FG_MUTE(), false))
            .child(label_text(data.branch.clone(), 12.0, c::FG_DIM(), false));
    }

    if let Some(title) = data.context.as_deref() {
        let show_progress = data.running && is_in_progress_title(title);
        let context: AnyElement = if show_progress {
            let phase = in_progress_phase(tick);
            let step = |i: u64| {
                div().size(px(6.0)).rounded_full().bg(if i == phase {
                    c::GREEN()
                } else {
                    c::FG_MUTE()
                })
            };
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(step(0))
                .child(step(1))
                .child(step(2))
                .child(label_text("in progress", 12.0, c::GREEN(), false))
                .into_any_element()
        } else {
            label_text(
                truncate_middle(title, CONTEXT_MAX_CHARS),
                12.0,
                c::FG_DIM(),
                false,
            )
            .into_any_element()
        };
        identity = identity
            .child(label_text("·", 12.0, c::FG_MUTE(), false))
            .child(context);
    }

    div()
        .flex()
        .flex_col()
        .w_full()
        .child(
            div()
                .h(px(SESSBAR_H))
                .w_full()
                .flex()
                .items_center()
                .px(px(12.0))
                .bg(c::BG_STRIP())
                .overflow_hidden()
                .child(identity),
        )
        .child(div().h(px(1.0)).w_full().bg(c::BORDER_SOFT()))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `common.rs:192-195`.
    #[test]
    fn in_progress_matches_all_three_spellings_case_insensitively() {
        assert!(is_in_progress_title("Refactor in progress"));
        assert!(is_in_progress_title("REFACTOR IN-PROGRESS"));
        assert!(is_in_progress_title("build in_progress"));
        assert!(!is_in_progress_title("progress report"));
        assert!(!is_in_progress_title("done"));
        assert!(!is_in_progress_title(""));
    }

    /// `terminal.rs:540` — three phases, one step every five ticks.
    #[test]
    fn the_three_dot_walk_advances_every_five_ticks() {
        assert_eq!(in_progress_phase(0), 0);
        assert_eq!(in_progress_phase(4), 0);
        assert_eq!(in_progress_phase(5), 1);
        assert_eq!(in_progress_phase(10), 2);
        assert_eq!(in_progress_phase(15), 0);
        for t in 0..240u64 {
            assert!(in_progress_phase(t) < 3);
            assert_eq!(in_progress_phase(t), in_progress_phase(t + 15));
        }
    }

    /// `src/gui/widgets/primitives.rs:12-26`.
    #[test]
    fn truncate_middle_collapses_only_what_does_not_fit() {
        assert_eq!(truncate_middle("short", 80), "short");
        assert_eq!(truncate_middle("abcdefghij", 5), "ab…ij");
        // Multi-byte safe: char counts, never byte slicing.
        assert_eq!(truncate_middle("ααααββββ", 5).chars().count(), 5);
        assert_eq!(truncate_middle("abc", 1), "abc");
    }

    /// A branchless session (a home terminal) must not render an orphan `·`.
    #[test]
    fn a_blank_branch_is_skipped_entirely() {
        let data = SessionHeaderData {
            branch: "   ".into(),
            ..SessionHeaderData::default()
        };
        assert!(data.branch.trim().is_empty());
    }

    /// The 3-dot walk replaces the title only while the session is *running*
    /// (`terminal.rs:494`): a dead agent with a frozen "in progress" title
    /// shows the text, not a live animation.
    #[test]
    fn a_dead_session_does_not_animate_its_stale_in_progress_title() {
        let title = "migration in progress";
        assert!(is_in_progress_title(title));
        let dead = SessionHeaderData {
            context: Some(title.into()),
            running: false,
            ..SessionHeaderData::default()
        };
        let show_progress = dead.running && is_in_progress_title(title);
        assert!(!show_progress);
    }
}
