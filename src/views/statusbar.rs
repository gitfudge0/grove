//! The bottom status bar: running count, backend/theme labels, the `bypass`
//! chip, the toast slot, the palette/shortcuts hint chips and the version.
//!
//! Port of `src/gui/view/statusbar.rs:17-192`. Note the hairline is **above**
//! this bar, where the appbar's is below.
//!
//! Free render function rather than a `Render` entity, for the same reason as
//! [`crate::views::appbar`] — see its module docs.

use gpui::{div, prelude::*, px, AnyElement, Div, Hsla, MouseButton};

use crate::entities::toast::{Toast, ToastKind};
use crate::keymap::{platform_mod_label, GlobalShortcut};
use crate::theme as c;
use crate::views::appbar::{on_chrome, shortcut_key, ChromeAction, Dispatch};

/// Status bar height (`src/gui/metrics.rs:16`).
pub const STATUS_H: f32 = 26.0;

pub struct StatusbarCtx {
    pub running: usize,
    /// `tmux` or `native` (`statusbar.rs:31-35`).
    pub backend: &'static str,
    pub theme_name: String,
    pub skip_permissions: bool,
    pub toast: Option<Toast>,
    pub dispatch: Dispatch,
}

fn mono(content: impl Into<gpui::SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .font(gpui::font(crate::fonts::MONO_FAMILY))
        .text_size(px(size))
        .text_color(color)
        .child(content.into())
}

/// Keycap chip shell (`src/gui/widgets/modal.rs:44-58`): mono, 2px/6px
/// padding, radius 4, filled `BG_HL`.
fn keycap(inner: impl IntoElement) -> Div {
    div()
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .bg(c::BG_HL())
        .child(inner)
}

pub fn statusbar(ctx: &StatusbarCtx) -> AnyElement {
    let running_group = div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(div().size(px(7.0)).rounded_full().bg(if ctx.running > 0 {
            c::GREEN()
        } else {
            c::FG_MUTE()
        }))
        .child(mono(format!("{}", ctx.running), 10.0, c::FG_DIM()))
        .child(mono("RUNNING", 10.0, c::FG_MUTE()));

    let labelled = |label: &'static str, value: String| {
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(mono(label, 10.0, c::FG_MUTE()))
            .child(mono(value, 10.0, c::FG_DIM()))
    };

    let mut left = div()
        .flex()
        .items_center()
        .gap(px(14.0))
        .child(running_group)
        .child(labelled("BACKEND", ctx.backend.to_string()))
        .child(labelled("THEME", ctx.theme_name.clone()));
    if ctx.skip_permissions {
        left = left.child(keycap(mono("bypass", 10.0, c::YELLOW())));
    }

    // The third slot. No pulse, no overlay — recorded ambiguity 3.
    let toast: AnyElement = match ctx.toast.as_ref() {
        Some(t) => mono(
            t.message.clone(),
            10.0,
            match t.kind {
                ToastKind::Error => c::RED(),
                ToastKind::Info => c::GREEN(),
            },
        )
        .into_any_element(),
        None => div().into_any_element(),
    };

    let right = div()
        .flex()
        .items_center()
        .child(hint_chip(
            "statusbar-palette",
            shortcut_key(GlobalShortcut::NewSession, "p"),
            "palette",
            ChromeAction::OpenSessionLauncher,
            &ctx.dispatch,
        ))
        .child(div().w(px(14.0)))
        .child(hint_chip(
            "statusbar-shortcuts",
            shortcut_key(GlobalShortcut::ShortcutOverlay, "/"),
            "shortcuts",
            ChromeAction::OpenShortcutOverlay,
            &ctx.dispatch,
        ))
        .child(div().w(px(14.0)))
        .child(mono(
            format!("v{}", env!("CARGO_PKG_VERSION")),
            10.0,
            c::FG_MUTE(),
        ));

    div()
        .flex()
        .flex_col()
        .w_full()
        .h(px(STATUS_H))
        .child(div().h(px(1.0)).w_full().bg(c::BORDER_SOFT()))
        .child(
            div()
                .flex()
                .flex_1()
                .items_center()
                .w_full()
                .px(px(16.0))
                .bg(c::BG_STRIP())
                .child(left)
                .child(div().w(px(24.0)))
                .child(toast)
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
            .gap(px(1.0))
            .child(crate::icons::icon("command", 9.0, c::FG_DIM()))
            .child(mono(key, 10.0, c::FG_DIM()))
            .into_any_element()
    } else {
        mono(format!("{}+{key}", platform_mod_label()), 10.0, c::FG_DIM()).into_any_element()
    };
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_color(c::FG_MUTE())
        .hover(|s| s.text_color(c::FG()))
        .child(keycap(cap))
        .child(
            div()
                .font(gpui::font(crate::fonts::MONO_FAMILY))
                .text_size(px(10.0))
                .child(label),
        )
        .on_mouse_down(MouseButton::Left, on_chrome(dispatch, action))
        .into_any_element()
}
