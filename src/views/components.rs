//! Shared component library: keycaps, text, icon buttons, dividers, modal chrome. Ported from `src/gui/widgets/modal.rs`.

use crate::views::rpx;
use crate::views::tokens::*;
use gpui::{div, prelude::*, px, AnyElement, App, Div, Hsla, MouseButton, SharedString, Window};
use std::time::Duration;

use crate::fonts::{MONO_FAMILY, UI_FAMILY};
use crate::theme as c;

use super::dispatch::{ModalClick, ModalDispatch};

/// Double-click window for resetting a draggable divider's width (`src/gui/update/layout.rs:107-110`).
pub(crate) const DOUBLE_CLICK: Duration = Duration::from_millis(350);
/// Below this a divider release is a click, not a drag (`src/gui/update/layout.rs:159`).
pub(crate) const DRAG_EPSILON: f32 = 0.5;

/// In-progress divider drag state, shared by every draggable divider (ported from `src/gui/state.rs`'s `SidebarDrag`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct DividerDrag {
    /// Captured on first move, not press, so an off-edge grab doesn't jump the width (`layout.rs:137-147`).
    pub(crate) grab_offset: Option<f32>,
    pub(crate) start_width: f32,
}

const FOCUS_RING_W: f32 = 2.0;

/// [`modal_header_slotted`]'s close-button id when a caller passes none.
const DEFAULT_CLOSE_ID: &str = "modal-close";

const CHECKBOX_BOX: f32 = 14.0;

/// [`CHECKBOX_BOX`] less the 1px border on each side, so the mark fills the box without crossing the stroke.
const CHECKBOX_TICK: f32 = CHECKBOX_BOX - 2.0;

const VLINE_H: f32 = 16.0;

const ICON_SLOT_W: f32 = 24.0;

/// Palette top inset, carried over verbatim from the iced original (`src/gui/view/modals/mod.rs:114-121`); no derivation was recorded for the figure.
const PALETTE_TOP_DROP: f32 = 80.0;

/// The single sans text primitive for the whole app.
pub fn ui(content: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .font(gpui::font(UI_FAMILY))
        .text_size(rpx(size))
        .text_color(color)
        .child(content.into())
}

/// The single monospace text primitive for the whole app.
pub fn mono(content: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .font(gpui::font(MONO_FAMILY))
        .text_size(rpx(size))
        .text_color(color)
        .child(content.into())
}

/// Shared modal panel chrome, same background/border/shadow language as the command palette (`modal.rs:13-37`).
pub fn modal_panel(width: f32, content: impl IntoElement) -> Div {
    panel_surface(c::PANEL_SHADOW(), content).w(rpx(width))
}

/// Panel chrome shared by [`modal_panel`] and the diff viewer; `shadow` lets each caller still name its own themed [`crate::theme::PANEL_SHADOW`].
pub fn panel_surface(shadow: Hsla, content: impl IntoElement) -> Div {
    let (y, blur) = if c::is_dark() {
        (PANEL_SHADOW_Y, PANEL_SHADOW_BLUR)
    } else {
        (PANEL_SHADOW_Y_LIGHT, PANEL_SHADOW_BLUR_LIGHT)
    };
    div()
        .p(px(1.0))
        .bg(c::BG_RAIL())
        .text_color(c::FG())
        .border_1()
        .border_color(c::BORDER())
        .rounded(rpx(RADIUS_PANEL))
        .shadow(vec![gpui::BoxShadow {
            color: shadow,
            offset: gpui::point(px(0.0), px(y)),
            blur_radius: px(blur),
            spread_radius: px(0.0),
            inset: false,
        }])
        .child(content)
}

/// Keycap chip shell (`modal.rs:44-58`); `inner` carries its own text color.
pub fn keycap(inner: impl IntoElement) -> Div {
    keycap_filled(c::BG_HL(), inner)
}

/// [`keycap`] with the fill as an axis, so callers stop forking the chip shell to vary it.
pub fn keycap_filled(fill: Hsla, inner: impl IntoElement) -> Div {
    div()
        .px(rpx(SPACE_MD))
        .py(rpx(SPACE_XS))
        .rounded(rpx(RADIUS_CONTROL))
        .bg(fill)
        .child(inner)
}

/// The sessions rail's `+N` / `-N` / `clean` stat chip; label is signed so state survives greyscale.
pub fn diff_chip(label: impl Into<SharedString>, color: Hsla) -> Div {
    keycap_filled(c::BORDER_SOFT(), mono(label, TEXT_MICRO, color)).flex_none()
}

/// A plain-label keycap ("⏎", "↑↓", "esc", "←→") in the given text color (`modal.rs:60-72`).
pub fn keycap_text(label: impl Into<SharedString>, color: Hsla) -> Div {
    keycap(mono(label, TEXT_SMALL, color))
}

/// [`keycap_text`] with no chip fill — a filled chip per hint read as competing buttons.
pub fn keycap_text_flat(label: impl Into<SharedString>, color: Hsla) -> Div {
    keycap_filled(gpui::transparent_black(), mono(label, TEXT_SMALL, color))
}

/// Fakes letter-spacing by joining characters with U+2009 THIN SPACE (`src/gui/rows.rs:650-655`); gpui has no real letter-spacing.
#[must_use]
pub fn tracked(label: &str) -> String {
    label
        .chars()
        .map(String::from)
        .collect::<Vec<_>>()
        .join("\u{2009}")
}

/// A mono, uppercase section label (`modal.rs:77-90`); `indent` varies because a card-flush label and a list-inset label differ.
pub fn section_header(label: &str, indent: f32, top: f32, bottom: f32) -> Div {
    div()
        .pt(rpx(top))
        .pb(rpx(bottom))
        .pl(rpx(indent))
        .child(mono(label, TEXT_MICRO, c::FG_MUTE()))
}

/// One flat-keycap + muted label pair, the statusbar footer's hint shape.
pub fn footer_hint_flat(key: &'static str, label: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_MD))
        .child(keycap_text_flat(key, c::FG_DIM()))
        .child(mono(label, TEXT_MICRO, c::FG_MUTE()))
}

/// The footer zone: a 1px [`divider_h`] over a transparent row. The divider is drawn here, not by the caller — a footer without its rule is not a footer.
pub fn footer_container(content: impl IntoElement) -> Div {
    div().w_full().flex().flex_col().child(divider_h()).child(
        div()
            .w_full()
            .px(rpx(SPACE_3XL))
            .py(rpx(SPACE_LG))
            .child(content),
    )
}

/// The footer for every modal: hints left, buttons right, `buttons` ordered secondary then affirmative.
pub fn modal_footer(hints: &[(&'static str, &'static str)], buttons: Vec<AnyElement>) -> Div {
    let mut row = div().flex().items_center();

    if !hints.is_empty() {
        let mut group = div().flex().items_center().gap(rpx(SPACE_3XL));
        for (key, label) in hints {
            group = group.child(footer_hint_flat(key, label));
        }
        row = row.child(group);
    }

    row = row.child(div().flex_1());

    if !buttons.is_empty() {
        row = row.child(
            div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_LG))
                .children(buttons),
        );
    }

    footer_container(row)
}

/// A footer of plain hints and no buttons.
pub fn modal_footer_hints(hints: &[(&'static str, &'static str)]) -> Div {
    modal_footer(hints, Vec::new())
}

/// `dispatch: None` renders no close button (a blocking-progress state with nothing to cancel). `id` must be unique within the modal.
pub fn modal_header_slotted(
    id: Option<&'static str>,
    title: impl Into<SharedString>,
    accent: Hsla,
    meta: Option<AnyElement>,
    subtitle: Option<AnyElement>,
    dispatch: Option<&ModalDispatch>,
) -> Div {
    modal_header_slotted_custom(
        id,
        ui(title, TEXT_TITLE, accent).flex_1().into_any_element(),
        meta,
        subtitle,
        dispatch,
    )
}

/// [`modal_header_slotted`] for callers whose title zone is not a plain string (e.g. a rename-capable title with live input controls).
pub fn modal_header_slotted_custom(
    id: Option<&'static str>,
    title_content: AnyElement,
    meta: Option<AnyElement>,
    subtitle: Option<AnyElement>,
    dispatch: Option<&ModalDispatch>,
) -> Div {
    let close = dispatch.map(|dispatch| {
        let dispatch = std::rc::Rc::clone(dispatch);
        flat_icon_btn(
            id.unwrap_or(DEFAULT_CLOSE_ID),
            "close",
            ICON_BTN_W,
            ICON_MD,
            move |window, cx| dispatch(ModalClick::Cancel, window, cx),
        )
    });

    modal_header_row(
        div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_MD))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_XL))
                    .w_full()
                    .child(title_content)
                    .children(meta)
                    .children(close),
            )
            .children(subtitle),
    )
}

/// [`modal_header_slotted`] plus a trailing close icon button dispatching [`ModalClick::Cancel`]. `id` must be unique within the modal.
pub fn modal_header_with_close(
    id: &'static str,
    title: impl Into<SharedString>,
    accent: Hsla,
    dispatch: &ModalDispatch,
) -> Div {
    modal_header_slotted(Some(id), title, accent, None, None, Some(dispatch))
}

/// [`modal_header_slotted`]'s internal row shell (`modal.rs:162-171`).
fn modal_header_row(content: impl IntoElement) -> Div {
    div()
        .w_full()
        .px(rpx(SPACE_3XL))
        .py(rpx(SPACE_3XL))
        .child(content)
}

/// A checkbox's toggle handler. `None` renders the checkbox disabled.
pub type OnToggle = Box<dyn Fn(&mut gpui::Window, &mut gpui::App)>;

/// Visual weight of a modal footer button (`modal.rs:193-200`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalBtn {
    Plain,
    Primary,
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
        match self {
            ModalBtn::Danger => c::RED(),
            ModalBtn::Plain | ModalBtn::Primary => c::BORDER(),
        }
    }

    /// No current weight moves on this axis; kept because [`modal_action_sized`] reads it unconditionally.
    fn hover_border_color(self) -> Option<Hsla> {
        None
    }

    /// `Plain` is unfilled; the affirmative weights sit on `BG_HL` (`modal.rs:222-229`).
    fn bg(self) -> Hsla {
        if self == ModalBtn::Plain {
            c::BG()
        } else {
            c::BG_HL()
        }
    }
}

/// A modal footer button (`modal.rs:202-210`). `id` must be unique within the modal — gpui bleeds hover state between duplicate ids.
pub fn modal_action(
    id: &'static str,
    label: impl Into<SharedString>,
    kind: ModalBtn,
    on_click: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<Div> {
    modal_action_sized(id, label, kind, TEXT_BODY, on_click)
}

/// [`modal_action`] wired straight to a [`ModalClick`].
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

/// `enabled: false` keeps the box geometry but drops the handler structurally, never just `opacity()` (which still eats the click).
pub fn click_action_enabled(
    id: &'static str,
    label: impl Into<SharedString>,
    kind: ModalBtn,
    enabled: bool,
    dispatch: &ModalDispatch,
    click: ModalClick,
) -> AnyElement {
    if enabled {
        return click_action(id, label, kind, dispatch, click).into_any_element();
    }
    div()
        .px(rpx(SPACE_2XL))
        .py(rpx(SPACE_MD))
        .rounded(rpx(RADIUS_CONTROL))
        .border_1()
        .border_color(c::BORDER_SOFT())
        .bg(c::BG())
        .child(ui(label, TEXT_BODY, c::FG_MUTE()))
        .into_any_element()
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

/// How roomy a [`click_row`] is, and how its fill is shaped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowDensity {
    Manager,
    /// A row inside a bordered [`card`]: full-bleed square fill, since a rounded fill inset from the card edge reads as a floating second surface.
    Card,
}

impl RowDensity {
    fn px(self) -> f32 {
        match self {
            RowDensity::Manager | RowDensity::Card => SPACE_XL,
        }
    }

    fn py(self) -> Option<f32> {
        match self {
            RowDensity::Manager => Some(SPACE_MD),
            RowDensity::Card => None,
        }
    }

    fn radius(self) -> Option<f32> {
        match self {
            RowDensity::Manager => Some(RADIUS_GROUP),
            RowDensity::Card => None,
        }
    }
}

/// A clickable list row, the shape every modal list shares; [`click_row_on`] pre-wired to [`ModalClick`].
pub fn click_row(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    density: RowDensity,
    dispatch: &ModalDispatch,
    click: ModalClick,
    content: impl IntoElement,
) -> gpui::Stateful<Div> {
    let dispatch = std::rc::Rc::clone(dispatch);
    click_row_on(
        id,
        selected,
        density,
        move |window, cx| dispatch(click.clone(), window, cx),
        content,
    )
}

/// [`click_row`] with the click as a plain callback rather than a [`ModalClick`].
pub fn click_row_on(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    density: RowDensity,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    content: impl IntoElement,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .px(rpx(density.px()))
        .map(|d| match density.py() {
            Some(py) => d.py(rpx(py)),
            None => d.w_full(),
        })
        .map(|d| match density.radius() {
            Some(r) => d.rounded(rpx(r)),
            None => d,
        })
        .when(selected, |d| d.bg(c::BG_HL()))
        .hover(|s| s.bg(c::BG_HOVER()))
        .cursor_pointer()
        .child(content)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(window, cx);
        })
}

/// [`modal_action`] with an explicit text size, for spots where the default 12px button reads too loud (`modal.rs:212-254`).
pub fn modal_action_sized(
    id: &'static str,
    label: impl Into<SharedString>,
    kind: ModalBtn,
    size: f32,
    on_click: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .px(rpx(SPACE_2XL))
        .py(rpx(SPACE_MD))
        .rounded(rpx(RADIUS_CONTROL))
        .border_1()
        .border_color(kind.border_color())
        .bg(kind.bg())
        .hover(move |s| {
            let s = s.bg(c::BG_HOVER());
            match kind.hover_border_color() {
                Some(border) => s.border_color(border),
                None => s,
            }
        })
        .cursor_pointer()
        .child(ui(label, size, kind.text_color()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(window, cx);
        })
}

/// Modal checkbox in the shared themed style; `accent` colors the tick and checked border. `on_toggle: None` renders it disabled (`modal.rs:256-...`).
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
        .size(rpx(CHECKBOX_BOX))
        .rounded(rpx(RADIUS_CONTROL))
        .border_1()
        .border_color(border_color)
        .bg(if checked { c::BG_HL() } else { c::BG() })
        .flex()
        .items_center()
        .justify_center();
    if checked {
        // Tick is a sprite, not a text run: bundled fonts have no U+2713 glyph.
        box_ = box_.child(crate::icons::icon(
            "check",
            CHECKBOX_TICK,
            if disabled { c::FG_MUTE() } else { accent },
        ));
    }

    let row = div()
        .id(id)
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .child(box_)
        .child(ui(
            label,
            TEXT_BODY,
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

/// The full-bleed scrim centered modals sit on (`src/gui/view/modals/mod.rs:139-149`); an opaque wash since gpui has no backdrop blur.
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

/// The palette's variant of [`scrim`]: top-dropped rather than centered (`src/gui/view/modals/mod.rs:114-121`).
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
        .pt(rpx(PALETTE_TOP_DROP))
        .child(content)
}

/// A modal's body zone: shared `SPACE_3XL` padding on all four sides.
pub fn modal_body(content: impl IntoElement) -> Div {
    div()
        .w_full()
        .px(rpx(SPACE_3XL))
        .pt(rpx(SPACE_3XL))
        .pb(rpx(SPACE_3XL))
        .flex()
        .flex_col()
        .gap(rpx(SPACE_XL))
        .child(content)
}

pub fn body_text(content: impl Into<SharedString>) -> Div {
    ui(content, TEXT_BODY, c::FG_DIM())
}

/// An inline validation note, cleared on the next edit (`src/app/modal.rs:10-12`).
pub fn note_text(content: impl Into<SharedString>) -> Div {
    ui(content, TEXT_SMALL, c::RED())
}

/// A full-bleed 1px horizontal rule in the soft border tone (`src/gui/widgets/primitives.rs:54-62`, `divider_h`).
pub fn divider_h() -> Div {
    divider_h_toned(c::BORDER_SOFT())
}

/// [`divider_h`] at full `BORDER` strength, for rules separating structural zones rather than sections within one panel.
pub fn divider_h_strong() -> Div {
    divider_h_toned(c::BORDER())
}

/// The shared hairline shell behind [`divider_h`] and [`divider_h_strong`].
pub fn divider_h_toned(tone: Hsla) -> Div {
    div().w_full().h(px(1.0)).bg(tone)
}

/// [`divider_h`]'s axis flipped, for a body split into side-by-side columns.
pub fn divider_v() -> Div {
    div().flex_none().h_full().w(px(1.0)).bg(c::BORDER_SOFT())
}

/// A muted, indented one-liner shown under a section header or row (`src/gui/view/modals/settings.rs:141-145`).
pub fn caption(content: impl Into<SharedString>) -> Div {
    div()
        .px(rpx(SPACE_XL))
        .child(ui(content, TEXT_SMALL, c::FG_MUTE()))
}

/// Which of a [`row_sublabel`]'s two caption tones to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SublabelTone {
    Normal,
    /// Reserved for safety-relevant text (skip-permissions and friends): `FG_DIM` rather than `FG_MUTE`.
    Safety,
}

/// The second line under a row's label; no indent of its own since the row's label column already owns the left edge.
pub fn row_sublabel(content: impl Into<SharedString>, tone: SublabelTone) -> Div {
    ui(
        content,
        TEXT_SMALL,
        match tone {
            SublabelTone::Normal => c::FG_MUTE(),
            SublabelTone::Safety => c::FG_DIM(),
        },
    )
}

/// `rows` are separated by full-bleed [`divider_h`] rules and carry their own padding, so a row's fill can reach the card's inner edge.
pub fn card(rows: Vec<AnyElement>) -> Div {
    let last = rows.len().saturating_sub(1);
    let mut card = div()
        .rounded(rpx(RADIUS_CONTROL))
        .border_1()
        .border_color(c::BORDER())
        .bg(c::BG_STRIP())
        .flex()
        .flex_col();
    for (i, row) in rows.into_iter().enumerate() {
        card = card.child(row);
        if i != last {
            card = card.child(divider_h());
        }
    }
    card
}

/// A flat, borderless icon button in a fixed-width hoverable box (`src/gui/widgets/buttons.rs:419-451`, `icon_btn`).
pub fn flat_icon_btn(
    id: impl Into<gpui::ElementId>,
    name: &'static str,
    box_w: f32,
    icon_size: f32,
    on_click: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<Div> {
    icon_btn(
        id,
        name,
        box_w,
        CONTROL_H,
        icon_size,
        c::FG_DIM(),
        c::BG_HOVER(),
        None,
        false,
        on_click,
    )
}

/// Parameters, not chained calls, because gpui's `hover` cannot be called twice on one element.
#[allow(clippy::too_many_arguments)]
pub fn icon_btn(
    id: impl Into<gpui::ElementId>,
    name: &'static str,
    box_w: f32,
    box_h: f32,
    icon_size: f32,
    color: Hsla,
    hover_bg: Hsla,
    hover_fg: Option<Hsla>,
    hover_ring: bool,
    on_click: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .w(rpx(box_w))
        .h(rpx(box_h))
        .flex()
        .items_center()
        .justify_center()
        .rounded(rpx(RADIUS_CONTROL))
        .text_color(color)
        .when(hover_ring, |d| {
            d.border_1().border_color(gpui::transparent_black())
        })
        .hover(move |s| {
            let s = s.bg(hover_bg);
            let s = match hover_fg {
                Some(fg) => s.text_color(fg),
                None => s,
            };
            if hover_ring {
                s.border_color(c::BORDER_SOFT())
            } else {
                s
            }
        })
        .cursor_pointer()
        .child(crate::icons::icon(name, icon_size, color))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            // Without this, the press also reaches the row underneath and clears the two-step kill this click just armed.
            cx.stop_propagation();
            on_click(window, cx);
        })
}

pub fn status_dot(size: f32, color: Hsla) -> Div {
    div().size(rpx(size)).rounded_full().bg(color)
}

/// A 1px ring instead of a fill, so present-vs-absent survives greyscale.
pub fn status_dot_hollow(size: f32, color: Hsla) -> Div {
    div()
        .size(rpx(size))
        .rounded_full()
        .border_1()
        .border_color(color)
}

/// A 1px vertical hairline separating clusters inside a bar (`src/gui/widgets/primitives.rs`'s `vline`).
pub fn vline() -> Div {
    div().w(px(1.0)).h(rpx(VLINE_H)).bg(c::BORDER())
}

/// A flat, borderless text button in the same 22px-tall shape as [`flat_icon_btn`] (`src/gui/widgets/buttons.rs:455-495`, `control_btn_sized`).
pub fn flat_text_btn(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    text_size: f32,
    h_padding: f32,
    on_click: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<Div> {
    // Delegates to `flat_text_btn_tinted`, which is the one that writes
    // `.h(rpx(CONTROL_H))` — see its body for the §8.1 declaration this
    // function shares.
    flat_text_btn_tinted(id, label, text_size, h_padding, c::FG_DIM(), on_click)
}

/// [`flat_text_btn`] with the text colour as an axis (e.g. a low-emphasis destructive "Archive project" action).
pub fn flat_text_btn_tinted(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    text_size: f32,
    h_padding: f32,
    color: Hsla,
    on_click: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(rpx(CONTROL_H))
        .px(rpx(h_padding))
        .flex()
        .items_center()
        .justify_center()
        .rounded(rpx(RADIUS_CONTROL))
        .hover(|s| s.bg(c::BG_HOVER()))
        .cursor_pointer()
        .child(mono(label, text_size, color))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(window, cx);
        })
}

/// A boxed shell with a `BoxShadow` focus ring, since gpui's `Div` has no outline primitive.
///
/// **Caller contract** (still load bearing). The wrapped `Input` must be
/// built with its own insets zeroed and its width claimed, verbatim:
///
/// ```ignore
/// field_box(focused).child(
///     Input::new(state)
///         .appearance(false)
///         .pl(px(0.0))
///         .pr(px(0.0))
///         .py(px(0.0))
///         .w_full(),
/// )
/// ```
///
/// `Input` applies its own padding regardless of `.appearance(false)` — leave it unzeroed and the field's left edge goes out of true.
///
/// [`BG_STRIP`]: crate::theme::BG_STRIP
/// [`BORDER_SOFT`]: crate::theme::BORDER_SOFT
/// [`FOCUS_RING`]: crate::theme::FOCUS_RING
pub fn field_box(focused: bool) -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .overflow_hidden()
        .px(rpx(FIELD_PX))
        .py(rpx(FIELD_PY))
        .rounded(rpx(RADIUS_GROUP))
        .bg(c::BG_STRIP())
        .border_1()
        .border_color(if focused {
            c::MAGENTA()
        } else {
            c::BORDER_SOFT()
        })
        .when(focused, |d| {
            d.shadow(vec![gpui::BoxShadow {
                color: c::FOCUS_RING(),
                offset: gpui::point(px(0.0), px(0.0)),
                blur_radius: px(0.0),
                spread_radius: px(FOCUS_RING_W),
                inset: false,
            }])
        })
        .font(gpui::font(MONO_FAMILY))
        .text_size(rpx(TEXT_BODY))
}

/// Never bordered — a bordered button in a body would compete with the footer's affirmative action. `id` must be unique within the modal.
pub fn body_action(
    id: &'static str,
    label: impl Into<SharedString>,
    tone: Hsla,
    dispatch: &ModalDispatch,
    click: ModalClick,
) -> gpui::Stateful<Div> {
    let dispatch = std::rc::Rc::clone(dispatch);
    // Pinned to content size/leading edge: a column parent would otherwise stretch this to a full-bleed centred band.
    flat_text_btn_tinted(id, label, TEXT_BODY, SPACE_MD, tone, move |window, cx| {
        dispatch(click.clone(), window, cx);
    })
    .flex_none()
    .self_start()
    .justify_start()
}

/// Fixed width so labels align whether or not the row carries a status; height matches the row's first line, not its overall height.
pub fn status_gutter(dot: Option<AnyElement>) -> Div {
    div()
        .w(rpx(STATUS_DOT_COL_W))
        .h(rpx(CONTROL_H))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .children(dot)
}

/// Which edge of a joined segmented-control group a [`seg_button`] sits at (`src/gui/widgets/buttons.rs:11-18`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegSide {
    Left,
    Right,
}

/// `on_click: None` renders the segment inert, used for the already-active side so it can never toggle off.
pub fn seg_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    side: SegSide,
    danger: bool,
    on_click: Option<OnToggle>,
) -> AnyElement {
    seg_button_content(
        id,
        // Sized by CONTROL_H, not py, so a segment matches every other in-row control's height.
        div()
            .h(rpx(CONTROL_H))
            .px(rpx(SPACE_2XL))
            .flex()
            .items_center()
            .justify_center()
            .child(mono(label, TEXT_SMALL, seg_text_color(active, danger))),
        active,
        side,
        danger,
        on_click,
    )
}

pub fn seg_text_color(active: bool, danger: bool) -> Hsla {
    if active {
        if danger {
            c::RED()
        } else {
            c::FG()
        }
    } else {
        c::FG_DIM()
    }
}

/// [`seg_button`]'s shell around an arbitrary child, for segments carrying a glyph rather than a label. `content` owns its own padding.
pub fn seg_button_content(
    id: impl Into<gpui::ElementId>,
    content: impl IntoElement,
    active: bool,
    side: SegSide,
    danger: bool,
    on_click: Option<OnToggle>,
) -> AnyElement {
    let mut d = div()
        .id(id)
        .when(active, |d| {
            d.bg(if danger { c::RED_WASH() } else { c::BG_HL() })
        })
        .map(|d| match side {
            SegSide::Left => d
                .rounded_tl(rpx(RADIUS_CONTROL))
                .rounded_bl(rpx(RADIUS_CONTROL)),
            SegSide::Right => d
                .rounded_tr(rpx(RADIUS_CONTROL))
                .rounded_br(rpx(RADIUS_CONTROL)),
        })
        .child(content);
    if let Some(f) = on_click {
        d = d
            .cursor_pointer()
            .hover(|s| s.bg(c::BG_HOVER()))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| f(window, cx));
    }
    d.into_any_element()
}

/// The bordered wrapper a joined segmented-control group sits in (`src/gui/widgets/buttons.rs:119-127`, `skip_perms_seg`'s container).
pub fn seg_group(content: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .border_1()
        .border_color(c::BORDER())
        .rounded(rpx(RADIUS_GROUP))
        .child(content)
}

/// A tiny `Render` entity for one line of hint text; gpui's `.tooltip(builder)` needs an `AnyView` built fresh per hover, not a bare element.
struct HintTooltip {
    text: SharedString,
}

impl Render for HintTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .px(rpx(SPACE_LG))
            .py(rpx(SPACE_SM))
            .rounded(rpx(RADIUS_CONTROL))
            .bg(c::BG_HL())
            .border_1()
            .border_color(c::BORDER())
            .child(ui(self.text.clone(), TEXT_MICRO, c::FG_DIM()))
    }
}

/// A `.tooltip(..)` builder showing one line of `text` in the shared [`HintTooltip`] shape.
pub fn hint_tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView + 'static {
    let text = text.into();
    move |_window, cx| cx.new(|_| HintTooltip { text: text.clone() }).into()
}

/// Command palette row height (`src/gui/widgets/rows.rs:73`, `PALETTE_ROW_H`).
pub const PALETTE_ROW_H: f32 = 54.0;

/// A fixed 24px icon slot so titles line up across rows regardless of glyph width (`src/gui/widgets/rows.rs:27-51`, `palette_agent_content`'s `icon_slot`).
pub fn icon_slot(name: &str, size: f32, color: Hsla) -> Div {
    div()
        .w(rpx(ICON_SLOT_W))
        .flex()
        .items_center()
        .justify_center()
        .child(crate::icons::icon(name, size, color))
}

/// The cue chip shown in the palette's leading glyph slot when a drill-in replaces the search icon (`src/gui/session_launcher/view/mod.rs:44-59`).
pub fn cue_chip(label: impl Into<SharedString>) -> Div {
    div()
        .px(rpx(SPACE_MD))
        .py(rpx(SPACE_XS))
        .rounded(rpx(RADIUS_CONTROL))
        .bg(c::SEL_TINT_SOFT())
        .child(mono(label, TEXT_MICRO, c::CYAN()))
}

/// `selected`'s tint is a solid flattening of the iced original's gradient — gpui's `Div` has no gradient-background primitive.
pub fn palette_row(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    dispatch: &ModalDispatch,
    click: ModalClick,
    content: impl IntoElement,
) -> gpui::Stateful<Div> {
    let dispatch = std::rc::Rc::clone(dispatch);
    div()
        .id(id)
        .w_full()
        .h(rpx(PALETTE_ROW_H))
        .px(rpx(SPACE_2XL))
        .rounded(rpx(RADIUS_GROUP))
        .flex()
        .items_center()
        .when(selected, |d| {
            d.bg(c::SEL_TINT_SOFT())
                .border_1()
                .border_color(c::SEL_RING())
        })
        .when(!selected, |d| d.hover(|s| s.bg(c::BG_HOVER())))
        .cursor_pointer()
        .child(content)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            dispatch(click.clone(), window, cx);
        })
}
