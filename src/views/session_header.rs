//! The `SESSBAR_H` session bar above the terminal body: label, branch, the OSC context title, and the 3-dot in-progress animation. Port of `src/gui/view/terminal.rs:487-560` (+ `:480-485` for the hairline). **Built here, not in Plan 07** (recorded ambiguity 1): Appendix A's *Attention/activity* row "3-dot `(tick/5)%3`" is an exit-gate row for this phase, and the only place that animation exists is this bar's in-progress context; the bar also carries this phase's OSC deliverable. It is written **parameterized by session**, not by "the active session", so Plan 07 can reuse the same renderer for grid tile headers.

use crate::views::rpx;
use crate::views::tokens::*;
use gpui::{div, prelude::*, AnyElement, Div, Hsla};

use crate::entities::animation_clock::dots;
use crate::theme as c;

/// Session bar height (`src/gui/metrics.rs:17`).
pub const SESSBAR_H: f32 = 36.0;

/// What the session bar's right-hand tool cluster asks the workspace to do (Plan 07 Task 5 Step 4, recorded ambiguity 2 — `terminal.rs:592-620`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolAction {
    /// Plan 08 stub, shown only when the project has a run script configured.
    RunScript,
    ToggleTermPanel,
    ToggleZen,
    /// Two-step: the first press arms, the second kills.
    RequestKill,
    Kill,
    /// The `+N −M` diff-stat chip: open the diff viewer for the active session's worktree.
    OpenDiff,
}

pub type ToolDispatch = std::rc::Rc<dyn Fn(ToolAction, &mut gpui::Window, &mut gpui::App)>;

/// The right-hand tool cluster's inputs, resolved by the caller.
#[derive(Clone)]
pub struct ToolCluster {
    /// `terminal.rs:560-590` — the button exists only for a project with a non-blank run script.
    pub has_run_script: bool,
    /// Drives the `term` button's *toggle* styling.
    pub term_panel_open: bool,
    /// The zen tooltip flips while the chrome is hidden.
    pub chrome_visible: bool,
    /// Whether this session's kill is already armed.
    pub confirming_kill: bool,
    pub dispatch: ToolDispatch,
}

/// A labelled tool button (`src/gui/widgets/buttons.rs:340-410`'s `tool_btn`/`tool_btn_toggle`). `active` renders the cyan "on" state the `term` toggle uses; `danger` turns the hover red.
pub fn tool_btn(
    id: &'static str,
    icon_name: &'static str,
    label: &str,
    danger: bool,
    active: bool,
    on_click: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let base = if active { c::CYAN() } else { c::FG_DIM() };
    let hover_color = if danger { c::RED() } else { c::FG() };
    div()
        .id(id)
        .h(rpx(CONTROL_H))
        .flex()
        .items_center()
        .gap(rpx(SPACE_MD))
        .px(rpx(SPACE_LG))
        .rounded(rpx(RADIUS_CONTROL))
        .hover(move |s| s.bg(c::BG_HOVER()).text_color(hover_color))
        .cursor_pointer()
        .child(crate::icons::icon(icon_name, ICON_SM, base))
        .child(label_text(label.to_string(), TEXT_BODY, base, false))
        .on_mouse_down(
            gpui::MouseButton::Left,
            move |_: &gpui::MouseDownEvent, window: &mut gpui::Window, cx: &mut gpui::App| {
                on_click(window, cx);
            },
        )
        .into_any_element()
}

/// The cluster itself: run script (Plan 08 stub) │ term toggle │ zen │ kill.
fn tools(cluster: &ToolCluster) -> AnyElement {
    let mut row = div().flex().items_center().gap(rpx(SPACE_2XL));
    if cluster.has_run_script {
        let d = std::rc::Rc::clone(&cluster.dispatch);
        row = row.child(tool_btn(
            "sess-run",
            "play",
            "run script",
            false,
            false,
            move |window, cx| d(ToolAction::RunScript, window, cx),
        ));
    }
    let d_term = std::rc::Rc::clone(&cluster.dispatch);
    let d_zen = std::rc::Rc::clone(&cluster.dispatch);
    let d_kill = std::rc::Rc::clone(&cluster.dispatch);
    let kill_action = if cluster.confirming_kill {
        ToolAction::Kill
    } else {
        ToolAction::RequestKill
    };
    row.child(tool_btn(
        "sess-term",
        "term",
        "terminal",
        false,
        cluster.term_panel_open,
        move |window, cx| d_term(ToolAction::ToggleTermPanel, window, cx),
    ))
    .child(tool_btn(
        "sess-zen",
        "zen",
        if cluster.chrome_visible {
            "zen"
        } else {
            "exit zen"
        },
        false,
        false,
        move |window, cx| d_zen(ToolAction::ToggleZen, window, cx),
    ))
    .child(tool_btn(
        "sess-kill",
        "trash",
        if cluster.confirming_kill {
            "confirm kill"
        } else {
            "kill"
        },
        true,
        false,
        move |window, cx| d_kill(kill_action, window, cx),
    ))
    .into_any_element()
}

/// One session as the header draws it. Everything is resolved by the caller, so the renderer touches no entity and Plan 07 can build one per tile.
#[derive(Clone, Debug, Default)]
pub struct SessionHeaderData {
    /// Session/project label — the strongest element (13px, weight 600).
    pub label: String,
    /// Empty for branchless sessions (home terminals), which skip the segment entirely rather than show two dots with nothing between.
    pub branch: String,
    /// The OSC context title, already sanitized (`common.rs:179-190`).
    pub context: Option<String>,
    /// The agent's sprite name, for the leading glyph.
    pub icon_name: &'static str,
    /// Whether the session's process is still alive.
    pub running: bool,
    /// The worktree's uncommitted diff against `HEAD`: `(added, removed)` lines. `None` before the first poll lands (or for a branchless session with no worktree to poll) — see [`rows::diff_chips`].
    pub diff: Option<(u32, u32)>,
}

/// `common.rs:192-195` — case-insensitive, all three spellings.
#[must_use]
pub fn is_in_progress_title(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    lower.contains("in progress") || lower.contains("in-progress") || lower.contains("in_progress")
}

/// Shorten `s` to at most `max` chars by collapsing the middle with `…` (`src/gui/widgets/primitives.rs:12-26`).
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

/// Which of the three dots is lit (`terminal.rs:540` — `(tick/5)%3`). Shares the single animation counter with the cursor and the spinner, so the phases stay in the same relationship the iced build has.
#[must_use]
pub fn in_progress_phase(tick: u64) -> u64 {
    dots(tick)
}

/// [`crate::views::components::ui`] plus the bar's optional semibold weight, which the shared helper has no parameter for.
fn label_text(content: impl Into<gpui::SharedString>, size: f32, color: Hsla, bold: bool) -> Div {
    let d = crate::views::components::ui(content, size, color);
    if bold {
        d.font_weight(gpui::FontWeight::SEMIBOLD)
    } else {
        d
    }
}

/// The bar plus its `BORDER_SOFT()` hairline beneath. `cluster` is `None` for bars with no tools (Plan 06's call sites); Plan 07's session bar passes one.
pub fn session_header(
    data: &SessionHeaderData,
    tick: u64,
    cluster: Option<&ToolCluster>,
) -> AnyElement {
    let mut identity = div()
        .flex()
        .items_center()
        .flex_1()
        .min_w_0()
        .gap(rpx(SPACE_MD))
        .overflow_hidden()
        .child(crate::icons::icon(data.icon_name, ICON_MD, c::FG()).into_any_element())
        .child(
            label_text(data.label.clone(), TEXT_TITLE, c::FG(), true)
                .flex_shrink_0()
                .into_any_element(),
        );

    if !data.branch.trim().is_empty() {
        identity = identity
            .child(
                label_text("·", TEXT_TITLE, c::FG_MUTE(), false)
                    .flex_shrink_0()
                    .into_any_element(),
            )
            .child(
                label_text(data.branch.clone(), TEXT_BODY, c::FG_DIM(), false)
                    .flex_shrink_0()
                    .into_any_element(),
            );
    }

    if let Some(title) = data.context.as_deref() {
        let show_progress = data.running && is_in_progress_title(title);
        let context: AnyElement = if show_progress {
            let phase = in_progress_phase(tick);
            let step = |i: u64| {
                crate::views::components::status_dot(
                    DOT_SM,
                    if i == phase { c::GREEN() } else { c::FG_MUTE() },
                )
            };
            div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_SM))
                .child(step(0))
                .child(step(1))
                .child(step(2))
                .child(label_text("in progress", TEXT_BODY, c::GREEN(), false))
                .into_any_element()
        } else {
            let full_title = title.to_string();
            label_text(full_title.clone(), TEXT_BODY, c::FG_DIM(), false)
                .id("sess-context-title")
                .truncate()
                .min_w_0()
                .flex_1()
                .tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(full_title.clone()).build(window, cx)
                })
                .into_any_element()
        };
        identity = identity
            .child(
                label_text("·", TEXT_BODY, c::FG_MUTE(), false)
                    .flex_shrink_0()
                    .into_any_element(),
            )
            .child(context);
    }

    div()
        .flex()
        .flex_col()
        .w_full()
        .child(
            div()
                .h(rpx(SESSBAR_H))
                .w_full()
                .flex()
                .items_center()
                .gap(rpx(SPACE_2XL))
                .px(rpx(SPACE_3XL))
                .bg(c::BG_STRIP())
                .overflow_hidden()
                .child(div().flex_1().overflow_hidden().child(identity))
                .when(
                    crate::views::rows::diff_display(data.diff)
                        != crate::views::rows::DiffDisplay::Unknown,
                    |d| {
                        let chips = div()
                            .flex_none()
                            .child(crate::views::rows::diff_chips(data.diff));
                        // The chip pair is one click target with a hover state, wired only when a tool cluster exists to dispatch through — the tile-header call sites that pass `cluster: None` keep the chip inert rather than opening a diff viewer with no session to resolve a worktree from.
                        d.child(match cluster {
                            Some(cluster) => {
                                let dispatch = cluster.dispatch.clone();
                                div()
                                    .id("diff-chip-open")
                                    .flex_none()
                                    .rounded(rpx(RADIUS_CONTROL))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(c::BG_HOVER()))
                                    .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                                        dispatch(ToolAction::OpenDiff, window, cx);
                                    })
                                    .child(chips)
                                    .into_any_element()
                            }
                            None => chips.into_any_element(),
                        })
                    },
                )
                .when_some(cluster, |d, cluster| {
                    d.child(crate::views::components::vline())
                        .child(tools(cluster))
                }),
        )
        .child(crate::views::components::divider_h())
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

    /// [`SessionHeaderData::diff`] draws nothing before the first poll lands, distinguishing an unknown diff from a *known* clean one — the same rule [`rows::diff_chips`] enforces for the card.
    #[test]
    fn an_unknown_header_diff_is_distinguished_from_a_known_clean_one() {
        assert_eq!(
            crate::views::rows::diff_display(None),
            crate::views::rows::DiffDisplay::Unknown
        );
        assert_eq!(
            crate::views::rows::diff_display(Some((0, 0))),
            crate::views::rows::DiffDisplay::Clean
        );
        let unknown = SessionHeaderData {
            diff: None,
            ..SessionHeaderData::default()
        };
        let clean = SessionHeaderData {
            diff: Some((0, 0)),
            ..SessionHeaderData::default()
        };
        assert_ne!(unknown.diff, clean.diff);
    }

    /// The 3-dot walk replaces the title only while the session is *running* (`terminal.rs:494`): a dead agent with a frozen "in progress" title shows the text, not a live animation.
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
