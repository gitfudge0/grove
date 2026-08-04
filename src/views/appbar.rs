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

use crate::views::rpx;
use crate::views::tokens::*;
use std::rc::Rc;

use gpui::{div, prelude::*, px, AnyElement, App, MouseButton, MouseDownEvent, Window};

use crate::activity::ActivityState;
use crate::entities::session_registry::SessionId;
use crate::icons::icon;
use crate::keymap::{platform_mod_label, GlobalShortcut, SHORTCUTS};
use crate::theme as c;
use crate::views::components::{
    divider_h_strong, icon_btn, mono, seg_button_content, seg_group, status_dot, ui, SegSide,
};
use crate::views::rows::{path_basename, state_glyph};

/// App bar height (`src/gui/metrics.rs:15`).
pub const APPBAR_H: f32 = 44.0;

/// The attention dropdown panel's width. Per DESIGN.md §8.4 this is positional,
/// singular geometry — *the dropdown's* width — so it is a named const here
/// rather than a notch on a shared scale.
pub const ATTENTION_PANEL_W: f32 = 280.0;

/// The cog's box. One notch above [`CONTROL_H`] because the cog is the bar's
/// only always-present control and sits hard against the window's right edge:
/// at [`CONTROL_H`] its hover target clips against that edge, and the upgrade
/// dot it carries top-right would overhang the box. An optical/target
/// correction (§14 case 3), not a second control height.
const COG_BOX: f32 = 28.0;

/// The cog's glyph, sized to keep the same glyph-to-box ratio inside
/// [`COG_BOX`] that [`ICON_MD`] has inside [`CONTROL_H`]. Between [`ICON_MD`]
/// and [`ICON_LG`], for the same reason [`COG_BOX`] is off the control scale.
const COG_ICON: f32 = 15.0;

/// The waiting row's left accent bar. `rows.rs` draws the same 3px bar on its
/// own waiting rows; the two are independent local constants because §8.4 keeps
/// positional geometry in the module that owns the surface.
const ACCENT_BAR_W: f32 = 3.0;

/// One glyph segment's box in the `+` │ `grid` combo — see its use site.
const GLYPH_SEG_W: f32 = 26.0;

/// What a click on the window chrome asks the workspace to do. The chrome
/// never reaches into state itself (same contract as `rows::RowAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeAction {
    ToggleAttentionQueue,
    CloseAttentionQueue,
    /// A dropdown row: jump to that session (and close the dropdown).
    SelectWaiting(SessionId),
    /// The zen pill: straight to the first waiting session, no dropdown.
    JumpToWaiting,
    /// The grid/agent view.
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
    /// Whether a release is on offer — `upgrade_state::upgrade_available`,
    /// which is `matches!(state, Available(_))` and nothing else.
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
        .w(rpx(ctx.sidebar_width))
        .flex()
        .items_center()
        .px(rpx(SPACE_3XL))
        .child(ui("grove", TEXT_TITLE, c::MAGENTA()).font_weight(gpui::FontWeight::BOLD));

    let mut right = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_SM))
        .px(rpx(SPACE_3XL))
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
                .h(rpx(APPBAR_H))
                .w_full()
                .bg(c::BG_STRIP())
                .flex()
                .items_center()
                .child(brand)
                .child(div().flex_1())
                .child(right),
        )
        .child(divider_h_strong())
        .into_any_element()
}

/// Non-grid: a lone 22×22 muted icon button (`appbar.rs:124-149`). Grid: the
/// segmented `+` │ hairline │ `grid` combo (`:46-123`). Both segments dispatch
/// to logged stubs (carried amendment 7).
fn view_control(ctx: &AppbarCtx) -> AnyElement {
    if !ctx.grid_view {
        let dispatch = Rc::clone(&ctx.dispatch);
        return icon_btn(
            "appbar-grid",
            "grid",
            CONTROL_H,
            CONTROL_H,
            ICON_SM,
            c::FG_MUTE(),
            c::BG_HOVER(),
            None,
            false,
            move |window, cx| dispatch(ChromeAction::ToggleGridView, window, cx),
        )
        .into_any_element();
    }
    // A fixed-size glyph box per segment: the shared `seg_button` padding is
    // sized for a text label, and this combo must stay exactly as tall as the
    // lone toggle it replaces (`appbar.rs:103-111`).
    let glyph = |name: &'static str, color| {
        div()
            // 26, not CONTROL_H: a square box would make the two-segment combo
            // narrower than the lone toggle it replaces (§14 case 3).
            .w(rpx(GLYPH_SEG_W))
            .h(rpx(CONTROL_H))
            .flex()
            .items_center()
            .justify_center()
            .child(icon(name, ICON_SM, color))
    };
    let d_plus = Rc::clone(&ctx.dispatch);
    let plus = seg_button_content(
        "appbar-plus",
        glyph("plus", c::MAGENTA()),
        false,
        SegSide::Left,
        false,
        Some(Box::new(move |window, cx| {
            d_plus(ChromeAction::OpenSessionLauncher, window, cx);
        })),
    );
    // A short, fixed-height hairline: a full-height one would stretch the combo
    // taller than the lone toggle (`appbar.rs:103-111`).
    let seg_divider = div().w(px(1.0)).h(rpx(14.0)).bg(c::BORDER());
    let d_grid = Rc::clone(&ctx.dispatch);
    let grid_seg = seg_button_content(
        "appbar-grid-seg",
        glyph("grid", c::CYAN()),
        true,
        SegSide::Right,
        false,
        Some(Box::new(move |window, cx| {
            d_grid(ChromeAction::ToggleGridView, window, cx);
        })),
    );
    seg_group(
        div()
            .flex()
            .items_center()
            .child(plus)
            .child(seg_divider)
            .child(grid_seg),
    )
    .into_any_element()
}

/// Cog → Settings, with the `GREEN()` upgrade dot overlaid top-right only
/// while an upgrade is available (`src/gui/view/appbar.rs:29`).
fn cog(ctx: &AppbarCtx) -> AnyElement {
    let dispatch = Rc::clone(&ctx.dispatch);
    icon_btn(
        "appbar-cog",
        "cog",
        COG_BOX,
        COG_BOX,
        COG_ICON,
        c::FG_DIM(),
        c::BG_HOVER(),
        None,
        false,
        move |window, cx| dispatch(ChromeAction::OpenSettings, window, cx),
    )
    .relative()
    .when(ctx.upgrade_available, |d| {
        d.child(
            status_dot(DOT_SM, c::GREEN())
                .absolute()
                .top(px(0.0))
                .right(px(0.0)),
        )
    })
    .into_any_element()
}

/// Rendered **only** while something waits (`appbar.rs:151-208`).
fn attention_pill(ctx: &AppbarCtx) -> AnyElement {
    let dot_color = c::alpha(c::AMBER(), pill_dot_alpha(ctx.pulse));
    let bg = c::alpha(c::AMBER(), 0.08);
    let bg_hover = c::alpha(c::AMBER(), 0.14);
    div()
        .id("appbar-attention-pill")
        .flex()
        .items_center()
        .gap(rpx(SPACE_MD))
        .px(rpx(SPACE_XL))
        .py(rpx(SPACE_SM))
        .rounded(rpx(RADIUS_FULL))
        .border_1()
        .border_color(c::AMBER())
        .bg(bg)
        .hover(move |s| s.bg(bg_hover))
        .cursor_pointer()
        .child(status_dot(DOT_MD, dot_color))
        .child(ui(pill_label(ctx.waiting.len()), TEXT_SMALL, c::AMBER()))
        .on_mouse_down(
            MouseButton::Left,
            on_chrome(&ctx.dispatch, ChromeAction::ToggleAttentionQueue),
        )
        .into_any_element()
}

/// The floating zen pill (`src/gui/view/appbar.rs:244-305`) — the Plan 06
/// deferral. Top-right over the terminal, 12px from each edge; it is **not** a
/// dropdown, so there is no backdrop and nothing to dismiss: clicking jumps
/// straight to the first waiting session.
pub fn zen_attention_pill(ctx: &AppbarCtx) -> AnyElement {
    let dot_color = c::alpha(c::AMBER(), pill_dot_alpha(ctx.pulse));
    let bg = c::alpha(c::AMBER(), 0.08);
    let bg_hover = c::alpha(c::AMBER(), 0.14);
    let pill = div()
        .id("zen-attention-pill")
        .flex()
        .items_center()
        .gap(rpx(SPACE_MD))
        .px(rpx(SPACE_LG))
        .py(rpx(SPACE_XS))
        .rounded(rpx(RADIUS_FULL))
        .border_1()
        .border_color(c::AMBER())
        .bg(bg)
        .hover(move |s| s.bg(bg_hover))
        .cursor_pointer()
        .child(status_dot(DOT_SM, dot_color))
        // The bare count, not the appbar pill's "{n} need you" copy.
        .child(ui(ctx.waiting.len().to_string(), TEXT_SMALL, c::AMBER()))
        .on_mouse_down(
            MouseButton::Left,
            on_chrome(&ctx.dispatch, ChromeAction::JumpToWaiting),
        );

    div()
        .absolute()
        .top(rpx(SPACE_2XL))
        .right(rpx(SPACE_2XL))
        .child(pill)
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
        .w(rpx(ATTENTION_PANEL_W))
        .flex()
        .flex_col()
        .bg(c::BG_STRIP())
        .rounded(rpx(RADIUS_GROUP))
        .border_1()
        .border_color(c::BORDER())
        .overflow_hidden()
        .child(rows)
        .child(divider_h_strong())
        .child(
            div()
                .w_full()
                .pl(rpx(SPACE_2XL))
                .pr(rpx(SPACE_XL))
                .py(rpx(SPACE_MD))
                .child(footer_hint()),
        );

    div()
        .id("attention-dropdown-layer")
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .on_mouse_down(MouseButton::Left, {
            let dispatch = Rc::clone(&ctx.dispatch);
            move |_, window, cx| {
                // This layer covers the whole terminal body; without stopping
                // here every click on it (rows included, they bubble through)
                // would also land in the pty underneath.
                cx.stop_propagation();
                dispatch(ChromeAction::CloseAttentionQueue, window, cx);
            }
        })
        .child(
            div()
                .absolute()
                // The bar's height zooms; the hairline under it does not
                // (§6.3), so the two are applied in their own units rather
                // than folded into one rems value.
                .top(rpx(APPBAR_H))
                .mt(px(1.0))
                .right(rpx(SPACE_3XL))
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
        .gap(rpx(SPACE_LG))
        .pl(rpx(SPACE_2XL))
        .pr(rpx(SPACE_XL))
        .py(rpx(SPACE_MD))
        .hover(|s| s.bg(c::BG_HOVER()))
        .cursor_pointer()
        .child(state_glyph(row.state, ctx.tick, ctx.pulse))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_XS))
                .child(ui(row.agent_label, TEXT_SMALL, c::FG()))
                .child(mono(subtitle, TEXT_MICRO, c::FG_MUTE())),
        )
        // 3px amber left accent bar, stacked over the row — same idiom as the
        // waiting sidebar row.
        .child(
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .w(rpx(ACCENT_BAR_W))
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
            .gap(rpx(SPACE_XS))
            .child(icon("command", ICON_XS, c::FG_MUTE()))
            .child(mono(key.to_string(), TEXT_MICRO, c::FG_MUTE()))
            .child(ui(" jump to next", TEXT_MICRO, c::FG_MUTE()))
            .into_any_element();
    }
    ui(
        format!("{}+{key} jump to next", platform_mod_label()),
        TEXT_MICRO,
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
