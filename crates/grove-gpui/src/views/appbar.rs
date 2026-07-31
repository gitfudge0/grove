//! The appbar: brand mark, agent-view control, attention pill, cog — plus the
//! anchored attention dropdown that the pill toggles.
//!
//! Port of `src/gui/view/appbar.rs:20-237` (the bar) and `:310-437` (the
//! dropdown). `zen_attention_pill` (`:244-305`) is Plan 07.
//!
//! **Deviation from the plan's "Produces: `Appbar`".** These are free render
//! functions, not a `Render` entity, exactly like [`crate::views::rows`]: the
//! bar owns no state — every input is already an entity the
//! [`crate::views::workspace::Workspace`] holds and observes — so an entity
//! here would add a second observer graph over the same data with nothing to
//! store in it. Clicks travel back out through [`ChromeAction`], the same
//! `Rc<dyn Fn>` dispatch idiom `rows::RowCtx` uses.

use std::rc::Rc;

use gpui::{div, prelude::*, px, AnyElement, App, Hsla, MouseButton, MouseDownEvent, Window};

use crate::activity::ActivityState;
use crate::entities::session_registry::SessionId;
use crate::icons::icon;
use crate::keymap::{platform_mod_label, GlobalShortcut, SHORTCUTS};
use crate::theme as c;
use crate::views::rows::{path_basename, state_glyph, ui_text};

/// App bar height (`src/gui/metrics.rs:15`).
pub const APPBAR_H: f32 = 44.0;

/// What a click on the window chrome asks the workspace to do. The chrome
/// never reaches into state itself (same contract as `rows::RowAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeAction {
    ToggleAttentionQueue,
    CloseAttentionQueue,
    /// A dropdown row: jump to that session (and close the dropdown).
    SelectWaiting(SessionId),
    /// Plan 07 — the grid/agent view.
    ToggleGridView,
    /// Plan 08 — the session launcher (the `+` segment and the `palette` chip).
    OpenSessionLauncher,
    /// Plan 08 — Settings, behind the cog.
    OpenSettings,
    /// Plan 08 — the shortcut overlay.
    OpenShortcutOverlay,
}

pub type Dispatch = Rc<dyn Fn(ChromeAction, &mut Window, &mut App)>;

/// Bind a chrome action to a mouse-down handler. Shared with
/// [`crate::views::statusbar`].
pub fn on_chrome(
    dispatch: &Dispatch,
    action: ChromeAction,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + use<> {
    let dispatch = Rc::clone(dispatch);
    move |_, window, cx| dispatch(action, window, cx)
}

/// One waiting session as the dropdown draws it, resolved once by the
/// workspace so the row renderer never touches an entity.
#[derive(Clone, Debug)]
pub struct WaitingRow {
    pub id: SessionId,
    pub agent_label: &'static str,
    pub project: String,
    pub wt_path: String,
    pub state: ActivityState,
}

/// Everything the bar draws, resolved once per frame.
pub struct AppbarCtx {
    pub sidebar_width: f32,
    pub tick: u64,
    pub pulse: f32,
    /// The attention queue in tree order, resolved **once** by the workspace
    /// and shared with the dropdown (`view/mod.rs:58-61` — the iced build had
    /// three call sites recomputing it).
    pub waiting: Vec<WaitingRow>,
    /// Plan 07 stub field; the combo's *appearance* is conditional on it
    /// (`appbar.rs:46`) and only the `false` shape is reachable this phase.
    pub grid_view: bool,
    /// Plan 09 stubs this to `false`; the cog's green dot renders off.
    pub upgrade_available: bool,
    pub dispatch: Dispatch,
}

/// `"1 needs you"` / `"{n} need you"` (`appbar.rs:162-166`). Exact copy.
#[must_use]
pub fn pill_label(waiting: usize) -> String {
    if waiting == 1 {
        "1 needs you".to_string()
    } else {
        format!("{waiting} need you")
    }
}

/// The pill dot's alpha (`appbar.rs:155`): never fully transparent, so the
/// layout cannot shift as it pulses.
#[must_use]
pub fn pill_dot_alpha(pulse: f32) -> f32 {
    0.4f32.mul_add(-pulse, 1.0)
}

/// `APPBAR_H`-tall strip with a `BORDER()` hairline beneath it.
pub fn appbar(ctx: &AppbarCtx) -> AnyElement {
    let brand = div()
        .w(px(ctx.sidebar_width))
        .flex()
        .items_center()
        .px(px(16.0))
        .child(
            div()
                .font(gpui::font(crate::fonts::UI_FAMILY))
                .font_weight(gpui::FontWeight::BOLD)
                .text_size(px(14.0))
                .text_color(c::MAGENTA())
                .child("grove"),
        );

    let mut right = div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .px(px(16.0))
        .child(view_control(ctx));
    if !ctx.waiting.is_empty() {
        right = right.child(attention_pill(ctx));
    }
    right = right.child(cog(ctx));

    div()
        .flex()
        .flex_col()
        .w_full()
        .child(
            div()
                .h(px(APPBAR_H))
                .w_full()
                .bg(c::BG_STRIP())
                .flex()
                .items_center()
                .child(brand)
                .child(div().flex_1())
                .child(right),
        )
        .child(div().h(px(1.0)).w_full().bg(c::BORDER()))
        .into_any_element()
}

/// Non-grid: a lone 22×22 muted icon button (`appbar.rs:124-149`). Grid: the
/// segmented `+` │ hairline │ `grid` combo (`:46-123`). Both segments dispatch
/// to logged stubs (carried amendment 7).
fn view_control(ctx: &AppbarCtx) -> AnyElement {
    if !ctx.grid_view {
        return div()
            .id("appbar-grid")
            .size(px(22.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .hover(|s| s.bg(c::BG_HOVER()))
            .child(icon("grid", 13.0, c::FG_MUTE()))
            .on_mouse_down(
                MouseButton::Left,
                on_chrome(&ctx.dispatch, ChromeAction::ToggleGridView),
            )
            .into_any_element();
    }
    let plus = div()
        .id("appbar-plus")
        .w(px(26.0))
        .h(px(22.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_l(px(4.0))
        .hover(|s| s.bg(c::BG_HOVER()))
        .child(icon("plus", 13.0, c::MAGENTA()))
        .on_mouse_down(
            MouseButton::Left,
            on_chrome(&ctx.dispatch, ChromeAction::OpenSessionLauncher),
        );
    // A short, fixed-height hairline: a full-height one would stretch the combo
    // taller than the lone toggle (`appbar.rs:103-111`).
    let seg_divider = div().w(px(1.0)).h(px(14.0)).bg(c::BORDER());
    let grid_seg = div()
        .id("appbar-grid-seg")
        .w(px(26.0))
        .h(px(22.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_r(px(4.0))
        .bg(c::BG_HL())
        .hover(|s| s.bg(c::BG_HOVER()))
        .child(icon("grid", 13.0, c::CYAN()))
        .on_mouse_down(
            MouseButton::Left,
            on_chrome(&ctx.dispatch, ChromeAction::ToggleGridView),
        );
    div()
        .flex()
        .items_center()
        .rounded(px(5.0))
        .border_1()
        .border_color(c::BORDER())
        .child(plus)
        .child(seg_divider)
        .child(grid_seg)
        .into_any_element()
}

/// Cog → Plan 08 Settings, with the `GREEN()` upgrade dot overlaid top-right
/// only while an upgrade is available — stubbed off this phase (Plan 09).
fn cog(ctx: &AppbarCtx) -> AnyElement {
    div()
        .id("appbar-cog")
        .relative()
        .size(px(22.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .hover(|s| s.bg(c::BG_HOVER()))
        .child(icon("cog", 13.0, c::FG_MUTE()))
        .when(ctx.upgrade_available, |d| {
            d.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .right(px(0.0))
                    .size(px(6.0))
                    .rounded_full()
                    .bg(c::GREEN()),
            )
        })
        .on_mouse_down(
            MouseButton::Left,
            on_chrome(&ctx.dispatch, ChromeAction::OpenSettings),
        )
        .into_any_element()
}

/// Rendered **only** while something waits (`appbar.rs:151-208`).
fn attention_pill(ctx: &AppbarCtx) -> AnyElement {
    let dot_color = Hsla {
        a: pill_dot_alpha(ctx.pulse),
        ..c::AMBER()
    };
    let bg = Hsla {
        a: 0.08,
        ..c::AMBER()
    };
    let bg_hover = Hsla {
        a: 0.14,
        ..c::AMBER()
    };
    div()
        .id("appbar-attention-pill")
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(10.0))
        .py(px(4.0))
        .rounded(px(999.0))
        .border_1()
        .border_color(c::AMBER())
        .bg(bg)
        .hover(move |s| s.bg(bg_hover))
        .child(div().size(px(6.0)).rounded_full().bg(dot_color))
        .child(ui_text(pill_label(ctx.waiting.len()), 11.0, c::AMBER()))
        .on_mouse_down(
            MouseButton::Left,
            on_chrome(&ctx.dispatch, ChromeAction::ToggleAttentionQueue),
        )
        .into_any_element()
}

// ── the dropdown ─────────────────────────────────────────────────────────

/// The full-window layer: a transparent backdrop that dismisses on click, with
/// the 280px panel anchored under the appbar's right edge
/// (`appbar.rs:310-437`).
pub fn attention_dropdown(ctx: &AppbarCtx) -> AnyElement {
    let mut rows = div().flex().flex_col();
    for row in &ctx.waiting {
        rows = rows.child(dropdown_row(row, ctx));
    }

    let panel = div()
        .w(px(280.0))
        .flex()
        .flex_col()
        .bg(c::BG_STRIP())
        .rounded(px(6.0))
        .border_1()
        .border_color(c::BORDER())
        .overflow_hidden()
        .child(rows)
        .child(div().h(px(1.0)).w_full().bg(c::BORDER()))
        .child(
            div()
                .w_full()
                .pl(px(12.0))
                .pr(px(10.0))
                .py(px(6.0))
                .child(footer_hint()),
        );

    div()
        .id("attention-dropdown-layer")
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .on_mouse_down(
            MouseButton::Left,
            on_chrome(&ctx.dispatch, ChromeAction::CloseAttentionQueue),
        )
        .child(
            div()
                .absolute()
                .top(px(APPBAR_H + 1.0))
                .right(px(16.0))
                .child(panel),
        )
        .into_any_element()
}

fn dropdown_row(row: &WaitingRow, ctx: &AppbarCtx) -> AnyElement {
    let subtitle = format!("{} / {}", row.project, path_basename(&row.wt_path));
    div()
        .id(gpui::SharedString::from(format!(
            "attention-row-{}",
            row.id.raw()
        )))
        .relative()
        .w_full()
        .flex()
        .items_center()
        .gap(px(8.0))
        .pl(px(12.0))
        .pr(px(10.0))
        .py(px(6.0))
        .hover(|s| s.bg(c::BG_HOVER()))
        .child(state_glyph(row.state, ctx.tick, ctx.pulse))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(ui_text(row.agent_label, 11.0, c::FG()))
                .child(
                    div()
                        .font(gpui::font(crate::fonts::MONO_FAMILY))
                        .text_size(px(10.0))
                        .text_color(c::FG_MUTE())
                        .child(subtitle),
                ),
        )
        // 3px amber left accent bar, stacked over the row — same idiom as the
        // waiting sidebar row.
        .child(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .w(px(3.0))
                .h_full()
                .bg(c::AMBER()),
        )
        .on_mouse_down(
            MouseButton::Left,
            on_chrome(&ctx.dispatch, ChromeAction::SelectWaiting(row.id)),
        )
        .into_any_element()
}

/// The `mod+'` jump hint, per platform. The key comes from the `SHORTCUTS`
/// registry row for `JumpToWaitingSession` — **never** a literal.
fn footer_hint() -> AnyElement {
    let key = shortcut_key(GlobalShortcut::JumpToWaitingSession, "'");
    if cfg!(target_os = "macos") {
        return div()
            .flex()
            .items_center()
            .gap(px(1.0))
            .child(icon("command", 10.0, c::FG_MUTE()))
            .child(
                div()
                    .font(gpui::font(crate::fonts::MONO_FAMILY))
                    .text_size(px(10.0))
                    .text_color(c::FG_MUTE())
                    .child(key.to_string()),
            )
            .child(ui_text(" jump to next", 10.0, c::FG_MUTE()))
            .into_any_element();
    }
    ui_text(
        format!("{}+{key} jump to next", platform_mod_label()),
        10.0,
        c::FG_MUTE(),
    )
    .into_any_element()
}

/// The display key the registry records for an action, never a literal
/// (`statusbar.rs:150-158`, `appbar.rs:377-395`).
#[must_use]
pub fn shortcut_key(action: GlobalShortcut, fallback: &'static str) -> &'static str {
    SHORTCUTS
        .iter()
        .find(|d| d.action == Some(action))
        .map_or(fallback, |d| d.display_keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `appbar.rs:162-166` — exact copy, singular and plural.
    #[test]
    fn the_pill_label_switches_on_exactly_one() {
        assert_eq!(pill_label(1), "1 needs you");
        assert_eq!(pill_label(2), "2 need you");
        assert_eq!(pill_label(7), "7 need you");
        // Never rendered at zero (the pill is omitted), but it must not lie.
        assert_eq!(pill_label(0), "0 need you");
    }

    /// `appbar.rs:155` — the dot dims to 0.6 but never vanishes, so the pill
    /// cannot change size as it pulses.
    #[test]
    fn the_pill_dot_dims_without_disappearing() {
        assert!((pill_dot_alpha(0.0) - 1.0).abs() < 1e-6);
        assert!((pill_dot_alpha(0.5) - 0.8).abs() < 1e-6);
        assert!((pill_dot_alpha(1.0) - 0.6).abs() < 1e-6);
        for i in 0..=100 {
            let a = pill_dot_alpha(i as f32 / 100.0);
            assert!((0.6..=1.0).contains(&a), "{a}");
        }
    }

    /// The registry is the single source of the displayed keys.
    #[test]
    fn the_hint_keys_come_from_the_shortcut_registry() {
        assert_eq!(shortcut_key(GlobalShortcut::JumpToWaitingSession, "?"), "'");
        assert_eq!(shortcut_key(GlobalShortcut::NewSession, "?"), "p");
        assert_eq!(shortcut_key(GlobalShortcut::ShortcutOverlay, "?"), "/");
    }
}
