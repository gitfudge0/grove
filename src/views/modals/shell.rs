//! The shared modal chrome, ported from `src/gui/widgets/modal.rs`.
//!
//! Pure presentation: every helper takes plain data and returns an element, so
//! nothing here needs an entity or a `Context`. Line references in each doc
//! comment point at the iced original.

// The chrome, the input wrapper and the archive/teardown helpers are built
// once here and consumed by Tasks 4-6 of gpui rewrite plan 08.
#![allow(dead_code)]

use gpui::{div, prelude::*, px, AnyElement, Div, Hsla, MouseButton, SharedString};

use crate::fonts::{MONO_FAMILY, UI_FAMILY};
use crate::theme as c;
use crate::views::rows::tracked;

use super::{ModalClick, ModalDispatch};

/// Panel corner radius (`modal.rs:29`).
const PANEL_RADIUS: f32 = 12.0;
/// The footer strip's bottom radius: the panel's 12px minus its 1px content
/// inset, so the strip hugs the inner corner without covering the border
/// (`modal.rs:117-120`).
const FOOTER_RADIUS: f32 = 11.0;

fn ui(content: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .font(gpui::font(UI_FAMILY))
        .text_size(px(size))
        .text_color(color)
        .child(content.into())
}

fn mono(content: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .font(gpui::font(MONO_FAMILY))
        .text_size(px(size))
        .text_color(color)
        .child(content.into())
}

/// Shared modal panel chrome — the same background/border/shadow language as
/// the command palette (`modal.rs:13-37`). `content` carries its own zone
/// padding, so the panel itself is unpadded apart from the 1px inset that
/// keeps the filled footer strip inside the border stroke.
pub fn modal_panel(width: f32, content: impl IntoElement) -> Div {
    div()
        .w(px(width))
        .p(px(1.0))
        .bg(c::BG_RAIL())
        .text_color(c::FG())
        .border_1()
        .border_color(c::BORDER())
        .rounded(px(PANEL_RADIUS))
        .shadow(vec![gpui::BoxShadow {
            color: gpui::hsla(0.0, 0.0, 0.0, 0.35),
            offset: gpui::point(px(0.0), px(12.0)),
            blur_radius: px(40.0),
            spread_radius: px(0.0),
            inset: false,
        }])
        .child(content)
}

/// Keycap chip shell: mono, 2px/6px padding, radius 4, filled `BG_HL`
/// (`modal.rs:44-58`). `inner` carries its own text color.
pub fn keycap(inner: impl IntoElement) -> Div {
    div()
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .bg(c::BG_HL())
        .child(inner)
}

/// A plain-label keycap ("⏎", "↑↓", "esc", "←→") in the given text color
/// (`modal.rs:60-72`).
pub fn keycap_text(label: impl Into<SharedString>, color: Hsla) -> Div {
    keycap(mono(label, 11.0, color))
}

/// A mono, uppercase, letter-tracked section label (`modal.rs:77-90`). gpui
/// has no letter-spacing either, so tracking is faked the same way — every
/// character joined with a U+2009 thin space (see [`tracked`]).
pub fn section_header(label: &str, top: f32, bottom: f32) -> Div {
    div()
        .pt(px(top))
        .pb(px(bottom))
        .pl(px(12.0))
        .child(mono(tracked(label), 10.0, c::FG_MUTE()))
}

/// One keycap + muted label pair in a footer hint strip, e.g. "[↑↓] navigate"
/// (`modal.rs:95-105`).
pub fn footer_hint(key: &'static str, label: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(keycap_text(key, c::FG_DIM()))
        .child(mono(label, 10.0, c::FG_MUTE()))
}

/// The full-bleed footer strip: `BG_STRIP` fill, `[8, 16]` padding, bottom
/// corners rounded to stay flush with the panel's radius (`modal.rs:109-131`).
pub fn footer_container(content: impl IntoElement) -> Div {
    div()
        .w_full()
        .px(px(16.0))
        .py(px(8.0))
        .bg(c::BG_STRIP())
        .rounded_bl(px(FOOTER_RADIUS))
        .rounded_br(px(FOOTER_RADIUS))
        .child(content)
}

/// A footer strip of plain hints with the palette's 14px inter-hint spacing
/// (`modal.rs:135-146`).
pub fn modal_footer_hints(hints: &[(&'static str, &'static str)]) -> Div {
    let mut row = div().flex().items_center().gap(px(14.0));
    for (key, label) in hints {
        row = row.child(footer_hint(key, label));
    }
    footer_container(row)
}

/// [`footer_container`] for callers needing a fully custom row
/// (`modal.rs:148-154`).
pub fn modal_footer_row(content: impl IntoElement) -> Div {
    footer_container(content)
}

/// A modal header zone: `[14, 16]` padding around a size-13 title in `accent`
/// (`modal.rs:156-160`).
pub fn modal_header(title: impl Into<SharedString>, accent: Hsla) -> Div {
    modal_header_row(ui(title, 13.0, accent))
}

/// [`modal_header`] for callers needing more than a bare title in the header
/// zone, e.g. a title plus a right-aligned step counter (`modal.rs:162-171`).
pub fn modal_header_row(content: impl IntoElement) -> Div {
    div().w_full().px(px(16.0)).py(px(14.0)).child(content)
}

/// A checkbox's toggle handler. `None` renders the checkbox disabled.
pub type OnToggle = Box<dyn Fn(&mut gpui::Window, &mut gpui::App)>;

/// Visual weight of a modal footer button (`modal.rs:193-200`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalBtn {
    /// Dismiss / secondary action.
    Plain,
    /// Default affirmative action.
    Primary,
    /// Default affirmative action with destructive consequences.
    Danger,
}

impl ModalBtn {
    fn text_color(self) -> Hsla {
        match self {
            ModalBtn::Plain => c::FG_DIM(),
            ModalBtn::Primary => c::FG(),
            ModalBtn::Danger => c::RED(),
        }
    }

    fn border_color(self) -> Hsla {
        if self == ModalBtn::Danger {
            c::RED()
        } else {
            c::BORDER()
        }
    }

    /// `Plain` is unfilled; the two affirmative weights sit on `BG_HL`
    /// (`modal.rs:222-229`).
    fn bg(self) -> Hsla {
        if self == ModalBtn::Plain {
            c::BG()
        } else {
            c::BG_HL()
        }
    }
}

/// A modal footer button (`modal.rs:202-210`). `id` must be unique within the
/// modal — gpui bleeds hover state between duplicate ids.
pub fn modal_action(
    id: &'static str,
    label: impl Into<SharedString>,
    kind: ModalBtn,
    on_click: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<Div> {
    modal_action_sized(id, label, kind, 12.0, on_click)
}

/// [`modal_action`] wired straight to a [`ModalClick`], which is how every
/// ported modal's buttons are built.
pub fn click_action(
    id: &'static str,
    label: impl Into<SharedString>,
    kind: ModalBtn,
    dispatch: &ModalDispatch,
    click: ModalClick,
) -> gpui::Stateful<Div> {
    let dispatch = std::rc::Rc::clone(dispatch);
    modal_action(id, label, kind, move |window, cx| {
        dispatch(click.clone(), window, cx);
    })
}

/// A checkbox wired to a [`ModalClick`]. `enabled: false` renders it disabled.
pub fn click_checkbox(
    id: &'static str,
    label: impl Into<SharedString>,
    checked: bool,
    accent: Hsla,
    enabled: bool,
    dispatch: &ModalDispatch,
    click: ModalClick,
) -> AnyElement {
    let handler: Option<OnToggle> = if enabled {
        let dispatch = std::rc::Rc::clone(dispatch);
        Some(Box::new(move |window, cx| {
            dispatch(click.clone(), window, cx);
        }))
    } else {
        None
    };
    modal_checkbox(id, label, checked, accent, handler)
}

/// A clickable list row, the shape every modal list shares.
pub fn click_row(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    dispatch: &ModalDispatch,
    click: ModalClick,
    content: impl IntoElement,
) -> gpui::Stateful<Div> {
    let dispatch = std::rc::Rc::clone(dispatch);
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(8.0))
        .py(px(5.0))
        .rounded(px(4.0))
        .when(selected, |d| d.bg(c::BG_HL()))
        .hover(|s| s.bg(c::BG_HOVER()))
        .cursor_pointer()
        .child(content)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            dispatch(click.clone(), window, cx);
        })
}

/// [`modal_action`] with an explicit text size, for spots where the default
/// 12px button reads too loud (`modal.rs:212-254`).
pub fn modal_action_sized(
    id: &'static str,
    label: impl Into<SharedString>,
    kind: ModalBtn,
    size: f32,
    on_click: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(kind.border_color())
        .bg(kind.bg())
        .hover(|s| s.bg(c::BG_HOVER()))
        .cursor_pointer()
        .child(ui(label, size, kind.text_color()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(window, cx);
        })
}

/// Modal checkbox in the shared themed style; `accent` colors the tick and the
/// checked border. `on_toggle: None` renders it disabled
/// (`modal.rs:256-...`).
pub fn modal_checkbox(
    id: &'static str,
    label: impl Into<SharedString>,
    checked: bool,
    accent: Hsla,
    on_toggle: Option<OnToggle>,
) -> AnyElement {
    let disabled = on_toggle.is_none();
    let border_color = if checked { accent } else { c::BORDER() };
    let mut box_ = div()
        .size(px(14.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(border_color)
        .bg(if checked { c::BG_HL() } else { c::BG() })
        .flex()
        .items_center()
        .justify_center();
    if checked {
        box_ = box_.child(mono(
            "✓",
            10.0,
            if disabled { c::FG_MUTE() } else { accent },
        ));
    }

    let row = div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(box_)
        .child(ui(
            label,
            12.0,
            if disabled { c::FG_MUTE() } else { c::FG_DIM() },
        ));

    match on_toggle {
        None => row.into_any_element(),
        Some(f) => row
            .cursor_pointer()
            .hover(|s| s.opacity(0.85))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| f(window, cx))
            .into_any_element(),
    }
}

/// The full-bleed scrim the centered modals sit on
/// (`src/gui/view/modals/mod.rs:139-149`). gpui has no backdrop blur either,
/// so this is an opaque theme-derived wash, exactly as iced's `SCRIM()` is.
pub fn scrim(content: impl IntoElement) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(c::SCRIM())
        .flex()
        .items_center()
        .justify_center()
        .child(content)
}

/// The palette's variant of [`scrim`]: top-dropped rather than centered
/// (`src/gui/view/modals/mod.rs:114-121`).
pub fn scrim_top_drop(content: impl IntoElement) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(c::SCRIM())
        .flex()
        .flex_col()
        .items_center()
        .pt(px(80.0))
        .child(content)
}

/// A modal's body zone: the 16px side padding every ported modal shares, with
/// the caller choosing the vertical rhythm.
pub fn modal_body(content: impl IntoElement) -> Div {
    div()
        .w_full()
        .px(px(16.0))
        .pb(px(14.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(content)
}

/// Body prose at the modals' shared 12px/`FG_DIM` weight.
pub fn body_text(content: impl Into<SharedString>) -> Div {
    ui(content, 12.0, c::FG_DIM())
}

/// An inline validation note, shown in red under a field and cleared on the
/// next edit (`src/app/modal.rs:10-12`).
pub fn note_text(content: impl Into<SharedString>) -> Div {
    ui(content, 11.0, c::RED())
}
