//! The appbar: brand mark, attention pill, cog, and the anchored attention
//! dropdown. Free render functions, not a `Render` entity — the bar owns no
//! state, every input is already an entity `Workspace` holds and observes.

use crate::views::rpx;
use crate::views::tokens::*;
use std::rc::Rc;

use gpui::{div, prelude::*, px, AnyElement, App, MouseButton, MouseDownEvent, Window};

use crate::activity::ActivityState;
use crate::entities::session_registry::SessionId;
use crate::icons::icon;
use crate::keymap::{platform_mod_label, GlobalShortcut, SHORTCUTS};
use crate::theme as c;
use crate::views::components::{divider_h_strong, icon_btn, mono, status_dot, ui};
use crate::views::rows::{path_basename, state_glyph};

pub const APPBAR_H: f32 = 44.0;
pub const ATTENTION_PANEL_W: f32 = 280.0;

/// One notch above [`CONTROL_H`] — the cog sits hard against the window edge, so its hover target/upgrade dot need the extra room.
const COG_BOX: f32 = 28.0;
const COG_ICON: f32 = 15.0;
const ACCENT_BAR_W: f32 = 3.0;

/// The chrome never reaches into state itself (same contract as `rows::RowAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeAction {
    ToggleAttentionQueue,
    CloseAttentionQueue,
    SelectWaiting(SessionId),
    /// Straight to the first waiting session, no dropdown.
    JumpToWaiting,
    OpenSessionLauncher,
    OpenSettings,
    OpenShortcutOverlay,
}

pub type Dispatch = Rc<dyn Fn(ChromeAction, &mut Window, &mut App)>;

pub fn on_chrome(
    dispatch: &Dispatch,
    action: ChromeAction,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + use<> {
    let dispatch = Rc::clone(dispatch);
    move |_, window, cx| dispatch(action, window, cx)
}

#[derive(Clone, Debug)]
pub struct WaitingRow {
    pub id: SessionId,
    pub agent_label: &'static str,
    pub project: String,
    pub wt_path: String,
    pub state: ActivityState,
}

pub struct AppbarCtx {
    pub sidebar_width: f32,
    pub tick: u64,
    pub pulse: f32,
    /// Resolved once by the workspace and shared with the dropdown.
    pub waiting: Vec<WaitingRow>,
    pub upgrade_available: bool,
    pub dispatch: Dispatch,
}

#[must_use]
pub fn pill_label(waiting: usize) -> String {
    if waiting == 1 {
        "1 needs you".to_string()
    } else {
        format!("{waiting} need you")
    }
}

/// Never fully transparent, so layout cannot shift as it pulses.
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
        .px(rpx(SPACE_3XL));
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

/// Not a dropdown — no backdrop, nothing to dismiss. Clicking jumps straight to the first waiting session.
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
                // Without this, clicks bubbling through would also land in the pty underneath.
                cx.stop_propagation();
                dispatch(ChromeAction::CloseAttentionQueue, window, cx);
            }
        })
        .child(
            div()
                .absolute()
                // Bar height zooms; the hairline under it does not, so applied in separate units.
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

/// Key comes from the `SHORTCUTS` registry, never a literal.
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

    #[test]
    fn the_pill_label_switches_on_exactly_one() {
        assert_eq!(pill_label(1), "1 needs you");
        assert_eq!(pill_label(2), "2 need you");
        assert_eq!(pill_label(7), "7 need you");
        assert_eq!(pill_label(0), "0 need you");
    }

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

    #[test]
    fn the_hint_keys_come_from_the_shortcut_registry() {
        assert_eq!(shortcut_key(GlobalShortcut::JumpToWaitingSession, "?"), "'");
        assert_eq!(shortcut_key(GlobalShortcut::NewSession, "?"), "p");
        assert_eq!(shortcut_key(GlobalShortcut::ShortcutOverlay, "?"), "/");
    }
}
