//! The bottom status bar: running count, backend/theme labels, the `bypass`
//! chip, the toast slot, the palette/shortcuts hint chips and the version.
//!
//! Port of `src/gui/view/statusbar.rs:17-192`. Note the hairline is **above**
//! this bar, where the appbar's is below.
//!
//! Free render function rather than a `Render` entity, for the same reason as
//! [`crate::views::appbar`] — see its module docs.

use crate::views::rpx;
use crate::views::tokens::*;
use gpui::{div, prelude::*, AnyElement, MouseButton};

use crate::entities::toast::{Toast, ToastKind};
use crate::keymap::{platform_mod_label, GlobalShortcut};
use crate::theme as c;
use crate::views::appbar::{on_chrome, shortcut_key, ChromeAction, Dispatch};
use crate::views::components::{divider_h, footer_hint_flat, keycap, mono, status_dot};

/// Status bar height (`src/gui/metrics.rs:16`).
pub const STATUS_H: f32 = 26.0;

pub struct StatusbarCtx {
    pub running: usize,
    /// `tmux` or `native` (`statusbar.rs:31-35`).
    pub backend: &'static str,
    pub theme_name: String,
    pub skip_permissions: bool,
    pub toast: Option<Toast>,
    /// Present only while the transient grid-resize key context owns plain direction keys.
    pub grid_resize_hint: Option<String>,
    pub dispatch: Dispatch,
}

pub fn statusbar(ctx: &StatusbarCtx) -> AnyElement {
    let running_group = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_MD))
        .child(status_dot(
            DOT_MD,
            if ctx.running > 0 {
                c::GREEN()
            } else {
                c::FG_MUTE()
            },
        ))
        .child(mono(format!("{}", ctx.running), TEXT_MICRO, c::FG_DIM()))
        .child(mono("RUNNING", TEXT_MICRO, c::FG_MUTE()));

    let labelled = |label: &'static str, value: String| {
        div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_MD))
            .child(mono(label, TEXT_MICRO, c::FG_MUTE()))
            .child(mono(value, TEXT_MICRO, c::FG_DIM()))
    };

    let mut left = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_3XL))
        .child(running_group)
        .child(labelled("BACKEND", ctx.backend.to_string()))
        .child(labelled("THEME", ctx.theme_name.clone()));
    if ctx.skip_permissions {
        left = left.child(keycap(mono("bypass", TEXT_MICRO, c::YELLOW())));
    }

    // The third slot. No pulse, no overlay — recorded ambiguity 3.
    //
    // Kind is carried by a glyph as well as a colour (§2.3, §12): red-vs-green
    // mono text in the same slot is otherwise indistinguishable to a viewer who
    // cannot separate the two hues. The sprite table has no warning triangle, so
    // Error takes `close` — the X mark, the nearest "this did not work" shape —
    // and Info takes `check`, matching the Done glyph in §12's table.
    let toast: AnyElement = match ctx.toast.as_ref() {
        Some(t) => {
            let (glyph, tint) = match t.kind {
                ToastKind::Error => ("close", c::RED()),
                ToastKind::Info => ("check", c::GREEN()),
            };
            div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_SM))
                .child(crate::icons::icon(glyph, ICON_XS, tint))
                .child(mono(t.message.clone(), TEXT_MICRO, tint))
                .into_any_element()
        }
        None => div().into_any_element(),
    };

    let right = if let Some(target) = ctx.grid_resize_hint.as_ref() {
        div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_3XL))
            .child(mono("RESIZE", TEXT_MICRO, c::CYAN()))
            .child(mono(target.clone(), TEXT_MICRO, c::FG_DIM()))
            .child(footer_hint_flat("←↓↑→ / hjkl", "5%"))
            .child(footer_hint_flat("shift", "1%"))
            .child(footer_hint_flat("enter / esc", "done"))
    } else {
        div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_3XL))
            .child(hint_chip(
                "statusbar-palette",
                shortcut_key(GlobalShortcut::NewSession, "p"),
                "palette",
                ChromeAction::OpenSessionLauncher,
                &ctx.dispatch,
            ))
            .child(hint_chip(
                "statusbar-shortcuts",
                shortcut_key(GlobalShortcut::ShortcutOverlay, "/"),
                "shortcuts",
                ChromeAction::OpenShortcutOverlay,
                &ctx.dispatch,
            ))
            .child(mono(
                format!("v{}", env!("CARGO_PKG_VERSION")),
                TEXT_MICRO,
                c::FG_MUTE(),
            ))
    };

    div()
        .flex()
        .flex_col()
        .w_full()
        .h(rpx(STATUS_H))
        .child(divider_h())
        .child(
            div()
                .flex()
                .flex_1()
                .items_center()
                .w_full()
                .px(rpx(SPACE_3XL))
                .bg(c::BG_STRIP())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_3XL))
                        .child(left)
                        .child(toast),
                )
                .child(div().flex_1())
                .child(right),
        )
        .into_any_element()
}

/// A keycap chip (⌘+key icon on macOS, `"{mod}+{key}"` text elsewhere) plus a
/// muted label, `FG()` on hover (`statusbar.rs:100-140`). Both chips dispatch
/// to Plan 08 stubs.
fn hint_chip(
    id: &'static str,
    key: &'static str,
    label: &'static str,
    action: ChromeAction,
    dispatch: &Dispatch,
) -> AnyElement {
    let cap: AnyElement = if cfg!(target_os = "macos") {
        div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_XS))
            .child(crate::icons::icon("command", ICON_XS, c::FG_DIM()))
            .child(mono(key, TEXT_MICRO, c::FG_DIM()))
            .into_any_element()
    } else {
        mono(
            format!("{}+{key}", platform_mod_label()),
            TEXT_MICRO,
            c::FG_DIM(),
        )
        .into_any_element()
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(rpx(SPACE_MD))
        .text_color(c::FG_MUTE())
        .hover(|s| s.text_color(c::FG()))
        .cursor_pointer()
        .child(keycap(cap))
        .child(
            // Deliberately *not* `mono`: the chip's hover recolor lives on the
            // row, so this label must inherit its color rather than pin one.
            div()
                .font(gpui::font(crate::fonts::MONO_FAMILY))
                .text_size(rpx(TEXT_MICRO))
                .child(label),
        )
        .on_mouse_down(MouseButton::Left, on_chrome(dispatch, action))
        .into_any_element()
}
