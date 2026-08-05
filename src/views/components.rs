//! The app-wide shared component library: keycaps, text helpers, icon
//! buttons, status dots, dividers and the modal chrome. Every view builds
//! from it, not just the modals. Ported from `src/gui/widgets/modal.rs`.
//!
//! Pure presentation: every helper takes plain data and returns an element, so
//! nothing here needs an entity or a `Context`. Line references in each doc
//! comment point at the iced original.

// The chrome, the input wrapper and the archive/teardown helpers are built
// once here and consumed by Tasks 4-6 of gpui rewrite plan 08.
#![allow(dead_code)]

use crate::views::rpx;
use crate::views::tokens::*;
use gpui::{div, prelude::*, px, AnyElement, Div, Hsla, MouseButton, SharedString};

use crate::fonts::{MONO_FAMILY, UI_FAMILY};
use crate::theme as c;

use super::dispatch::{ModalClick, ModalDispatch};

/// The footer strip's bottom radius: [`RADIUS_PANEL`] minus the panel's 1px
/// content inset, so the strip hugs the inner corner without covering the
/// border (`modal.rs:117-120`).
const FOOTER_RADIUS: f32 = RADIUS_PANEL - 1.0;

/// The checkbox's box side. Single-consumer geometry, so per DESIGN.md §14 it
/// is a module constant here rather than a `tokens.rs` scale entry: it is
/// *the checkbox's* box, not a notch on a shared scale.
const CHECKBOX_BOX: f32 = 14.0;

/// [`vline`]'s height: tall enough to separate two [`CONTROL_H`] clusters
/// without drawing a full-height bar edge.
const VLINE_H: f32 = 16.0;

/// [`icon_slot`]'s fixed width — see that function's doc comment.
const ICON_SLOT_W: f32 = 24.0;

/// [`scrim_top_drop`]'s top inset. An optical correction, not a spacing notch:
/// the palette is read top-down, so it is dropped to roughly the upper third
/// of a typical window rather than centred — a centred palette makes the eye
/// travel down to the input and then back up to the first result. 80 is
/// carried over verbatim from the iced original
/// (`src/gui/view/modals/mod.rs:114-121`); that code recorded no derivation
/// for the exact figure, so it is preserved rather than re-derived.
const PALETTE_TOP_DROP: f32 = 80.0;

/// A UI-font text run. The single sans text primitive for the whole app —
/// every view that used to keep its own `ui`/`ui_text` copy calls this.
pub fn ui(content: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .font(gpui::font(UI_FAMILY))
        .text_size(rpx(size))
        .text_color(color)
        .child(content.into())
}

/// A mono-font text run. The single monospace text primitive for the whole
/// app — every view that used to keep its own `mono` copy calls this.
pub fn mono(content: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div()
        .font(gpui::font(MONO_FAMILY))
        .text_size(rpx(size))
        .text_color(color)
        .child(content.into())
}

/// Shared modal panel chrome — the same background/border/shadow language as
/// the command palette (`modal.rs:13-37`). `content` carries its own zone
/// padding, so the panel itself is unpadded apart from the 1px inset that
/// keeps the filled footer strip inside the border stroke.
pub fn modal_panel(width: f32, content: impl IntoElement) -> Div {
    div()
        .w(rpx(width))
        .p(px(1.0))
        .bg(c::BG_RAIL())
        .text_color(c::FG())
        .border_1()
        .border_color(c::BORDER())
        .rounded(rpx(RADIUS_PANEL))
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
    keycap_filled(c::BG_HL(), inner)
}

/// [`keycap`] with the fill as an axis. The chip shell is the same shape
/// wherever it appears, but the fill carries meaning: `BG_HL` for a literal
/// keycap, `BORDER_SOFT` for a neutral metadata chip (the rows' branch and
/// count chips) and an accent alpha for a live cue (the grid's number hint and
/// respond chip). Those forked the shell before this parameter existed.
pub fn keycap_filled(fill: Hsla, inner: impl IntoElement) -> Div {
    div()
        .px(rpx(SPACE_MD))
        .py(rpx(SPACE_XS))
        .rounded(rpx(RADIUS_CONTROL))
        .bg(fill)
        .child(inner)
}

/// A plain-label keycap ("⏎", "↑↓", "esc", "←→") in the given text color
/// (`modal.rs:60-72`).
pub fn keycap_text(label: impl Into<SharedString>, color: Hsla) -> Div {
    keycap(mono(label, TEXT_SMALL, color))
}

/// Letter-spaced section label. Neither iced nor gpui at this rev has
/// letter-spacing, so the characters are joined with U+2009 THIN SPACE exactly
/// as `src/gui/rows.rs:650-655` does.
#[must_use]
pub fn tracked(label: &str) -> String {
    label
        .chars()
        .map(String::from)
        .collect::<Vec<_>>()
        .join("\u{2009}")
}

/// A mono, uppercase section label (`modal.rs:77-90`). Previously faked
/// letter-spacing via [`tracked`]'s thin-space joins; that read as "split-out"
/// text rather than tracking, so the label now renders plain.
pub fn section_header(label: &str, top: f32, bottom: f32) -> Div {
    div()
        .pt(rpx(top))
        .pb(rpx(bottom))
        .pl(rpx(SPACE_2XL))
        .child(mono(label, TEXT_MICRO, c::FG_MUTE()))
}

/// One keycap + muted label pair in a footer hint strip, e.g. "[↑↓] navigate"
/// (`modal.rs:95-105`).
pub fn footer_hint(key: &'static str, label: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_MD))
        .child(keycap_text(key, c::FG_DIM()))
        .child(mono(label, TEXT_MICRO, c::FG_MUTE()))
}

/// The full-bleed footer strip: `BG_STRIP` fill, `[8, 16]` padding, bottom
/// corners rounded to stay flush with the panel's radius (`modal.rs:109-131`).
pub fn footer_container(content: impl IntoElement) -> Div {
    div()
        .w_full()
        .px(rpx(SPACE_3XL))
        .py(rpx(SPACE_LG))
        .bg(c::BG_STRIP())
        .rounded_bl(rpx(FOOTER_RADIUS))
        .rounded_br(rpx(FOOTER_RADIUS))
        .child(content)
}

/// A footer strip of plain hints with the palette's 14px inter-hint spacing
/// (`modal.rs:135-146`).
pub fn modal_footer_hints(hints: &[(&'static str, &'static str)]) -> Div {
    let mut row = div().flex().items_center().gap(rpx(SPACE_3XL));
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
    modal_header_row(ui(title, TEXT_TITLE, accent))
}

/// [`modal_header`] plus a trailing close icon button dispatching
/// [`ModalClick::Cancel`] — the shape the theme manager and the archived-project
/// list each used to fork. Same token contract as [`modal_header`]: `px/py
/// SPACE_3XL` around a [`TEXT_TITLE`] title in `accent`.
///
/// `id` must be unique within the modal (gpui bleeds hover state between
/// duplicate ids).
pub fn modal_header_with_close(
    id: &'static str,
    title: impl Into<SharedString>,
    accent: Hsla,
    dispatch: &ModalDispatch,
) -> Div {
    let dispatch = std::rc::Rc::clone(dispatch);
    modal_header_row(
        div()
            .flex()
            .items_center()
            .w_full()
            .child(div().flex_1().child(ui(title, TEXT_TITLE, accent)))
            .child(icon_btn(
                id,
                "close",
                CONTROL_H,
                CONTROL_H,
                ICON_SM,
                c::FG_MUTE(),
                c::BG_HOVER(),
                None,
                false,
                move |window, cx| dispatch(ModalClick::Cancel, window, cx),
            )),
    )
}

/// [`modal_header`] for callers needing more than a bare title in the header
/// zone, e.g. a title plus a right-aligned step counter (`modal.rs:162-171`).
pub fn modal_header_row(content: impl IntoElement) -> Div {
    div()
        .w_full()
        .px(rpx(SPACE_3XL))
        .py(rpx(SPACE_3XL))
        .child(content)
}

/// A checkbox's toggle handler. `None` renders the checkbox disabled.
pub type OnToggle = Box<dyn Fn(&mut gpui::Window, &mut gpui::App)>;

/// [`ModalBtn::Accent`]'s rest-border alpha: the accent is present but held
/// back at rest so the full-strength hover border reads as a change. Carried
/// over from the Add-project hero button, which hand-rolled this tint before
/// the weight existed.
const ACCENT_BORDER_REST_ALPHA: f32 = 0.45;

/// Visual weight of a modal footer button (`modal.rs:193-200`).
///
/// | Weight | Text | Border | Hover border | Fill |
/// |---|---|---|---|---|
/// | `Plain` | `FG_DIM` | `BORDER` | unchanged | `BG` (unfilled) |
/// | `Primary` | `FG` | `BORDER` | unchanged | `BG_HL` |
/// | `Danger` | `RED` | `RED` | unchanged | `BG_HL` |
/// | `Accent` | `FG` | `MAGENTA` α0.45 | `MAGENTA` | `BG_HL` |
///
/// Every weight also takes the shared `BG_HOVER` fill on hover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalBtn {
    /// Dismiss / secondary action.
    Plain,
    /// Default affirmative action.
    Primary,
    /// Default affirmative action with destructive consequences.
    Danger,
    /// Affirmative action with emphasis — the accent-bordered hero button that
    /// starts something new (the Add-project wizard's folder browser), sharing
    /// the appbar `+`'s accent role.
    Accent,
}

impl ModalBtn {
    fn text_color(self) -> Hsla {
        match self {
            ModalBtn::Plain => c::FG_DIM(),
            ModalBtn::Primary | ModalBtn::Accent => c::FG(),
            ModalBtn::Danger => c::RED(),
        }
    }

    fn border_color(self) -> Hsla {
        match self {
            ModalBtn::Danger => c::RED(),
            ModalBtn::Accent => Hsla {
                a: ACCENT_BORDER_REST_ALPHA,
                ..c::MAGENTA()
            },
            ModalBtn::Plain | ModalBtn::Primary => c::BORDER(),
        }
    }

    /// The border on hover. `Accent` is the only weight that moves on this
    /// axis; it must be applied inside the component because gpui's `hover`
    /// refuses a second call on the same element (see [`icon_btn`]).
    fn hover_border_color(self) -> Option<Hsla> {
        match self {
            ModalBtn::Accent => Some(c::MAGENTA()),
            _ => None,
        }
    }

    /// `Plain` is unfilled; the affirmative weights sit on `BG_HL`
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
    modal_action_sized(id, label, kind, TEXT_BODY, on_click)
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
        .gap(rpx(SPACE_LG))
        .px(rpx(SPACE_LG))
        .py(rpx(SPACE_SM))
        .rounded(rpx(RADIUS_CONTROL))
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
        .size(rpx(CHECKBOX_BOX))
        .rounded(rpx(RADIUS_CONTROL))
        .border_1()
        .border_color(border_color)
        .bg(if checked { c::BG_HL() } else { c::BG() })
        .flex()
        .items_center()
        .justify_center();
    if checked {
        box_ = box_.child(mono(
            "✓",
            TEXT_MICRO,
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
        .pt(rpx(PALETTE_TOP_DROP))
        .child(content)
}

/// A modal's body zone: the 16px side padding every ported modal shares, with
/// the caller choosing the vertical rhythm.
pub fn modal_body(content: impl IntoElement) -> Div {
    div()
        .w_full()
        .px(rpx(SPACE_3XL))
        .pb(rpx(SPACE_3XL))
        .flex()
        .flex_col()
        .gap(rpx(SPACE_XL))
        .child(content)
}

/// Body prose at the modals' shared 12px/`FG_DIM` weight.
pub fn body_text(content: impl Into<SharedString>) -> Div {
    ui(content, TEXT_BODY, c::FG_DIM())
}

/// An inline validation note, shown in red under a field and cleared on the
/// next edit (`src/app/modal.rs:10-12`).
pub fn note_text(content: impl Into<SharedString>) -> Div {
    ui(content, TEXT_SMALL, c::RED())
}

/// A full-bleed 1px horizontal rule in the soft border tone, used between
/// Settings' sections and around its header/footer zones
/// (`src/gui/widgets/primitives.rs:54-62`, `divider_h`).
pub fn divider_h() -> Div {
    divider_h_toned(c::BORDER_SOFT())
}

/// [`divider_h`] at full `BORDER` strength, for rules that separate *structural*
/// zones rather than sections inside one panel — the appbar's bottom edge and
/// the rules bounding the chrome bars (§7.2's two tones).
///
/// This is a second function rather than a tone parameter on [`divider_h`]
/// because §7.2 admits exactly two tones: a two-inhabitant enum would earn
/// nothing over a name, and the soft case — 13 of the call sites — stays a
/// zero-argument call.
pub fn divider_h_strong() -> Div {
    divider_h_toned(c::BORDER())
}

/// The shared hairline shell behind [`divider_h`] and [`divider_h_strong`],
/// for the rare rule that needs a tone neither name covers.
pub fn divider_h_toned(tone: Hsla) -> Div {
    div().w_full().h(px(1.0)).bg(tone)
}

/// A muted, indented one-liner shown under a section header or row to explain
/// what a control does (`src/gui/view/modals/settings.rs:141-145`).
pub fn caption(content: impl Into<SharedString>) -> Div {
    div()
        .px(rpx(SPACE_XL))
        .child(ui(content, TEXT_SMALL, c::FG_MUTE()))
}

/// One shade up from [`caption`] — reserved for safety-relevant captions,
/// e.g. skip-permissions (`src/gui/view/modals/settings.rs:148-152`).
pub fn caption_promoted(content: impl Into<SharedString>) -> Div {
    div()
        .px(rpx(SPACE_XL))
        .child(ui(content, TEXT_SMALL, c::FG_DIM()))
}

/// A flat, borderless icon button in a fixed-width hoverable box — the zoom
/// `-`/`+` glyphs, the header close icon and the Tools/Updates refresh icons
/// all share this shape (`src/gui/widgets/buttons.rs:419-451`, `icon_btn`).
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

/// [`flat_icon_btn`] with every visual axis the app's icon buttons actually
/// vary on exposed. The chrome's icon buttons differ only in box size, glyph
/// size, rest tint and hover treatment, so they all land here rather than each
/// keeping a private copy of the same `div()` chain.
///
/// `hover_fg` recolors the *container's* text on hover (the sidebar/rows
/// buttons set it; the glyph itself is tinted by `color`, since `Svg` paints
/// with its own `text_color`). `hover_ring` adds the transparent-at-rest,
/// `BORDER_SOFT`-on-hover outline the sidebar row tools use.
///
/// gpui's `hover` refuses to be called twice on one element
/// (`div.rs:805` `debug_assert!`), which is why these are parameters rather
/// than something a call site could chain on afterwards.
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
            on_click(window, cx);
        })
}

/// A small filled dot — the running/activity indicator the status bar, the
/// terminal tab bar, the sidebar rollups and the attention pills all draw.
pub fn status_dot(size: f32, color: Hsla) -> Div {
    div().size(rpx(size)).rounded_full().bg(color)
}

/// A 1px vertical hairline separating clusters inside a bar
/// (`src/gui/widgets/primitives.rs`'s `vline`).
pub fn vline() -> Div {
    div().w(px(1.0)).h(rpx(VLINE_H)).bg(c::BORDER())
}

/// A flat, borderless text button in the same 22px-tall shape as
/// [`flat_icon_btn`] — used for the zoom percentage's reset label
/// (`src/gui/widgets/buttons.rs:455-495`, `control_btn_sized`).
pub fn flat_text_btn(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    text_size: f32,
    h_padding: f32,
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
        .child(mono(label, text_size, c::FG_DIM()))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(window, cx);
        })
}

/// Which edge of a joined segmented-control group a [`seg_button`] sits at —
/// only the group's outer corners round (`src/gui/widgets/buttons.rs:11-18`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegSide {
    Left,
    Right,
}

/// One segment of a two-way segmented control (Backend Native|Tmux,
/// Permissions Skip|Safe, the theme picker's Dark|Light). `on_click: None`
/// renders the segment inert — used for the side that is already active, so
/// clicking it can never toggle the control back off
/// (`src/gui/widgets/buttons.rs:20-100`, `seg_button`/`seg_button_danger`).
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
        div().px(rpx(SPACE_2XL)).py(rpx(SPACE_SM)).child(mono(
            label,
            TEXT_SMALL,
            seg_text_color(active, danger),
        )),
        active,
        side,
        danger,
        on_click,
    )
}

/// The text tint of a [`seg_button`]'s label.
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

/// [`seg_button`]'s shell — fill, outer-corner rounding, hover and click —
/// around an arbitrary child, for segments carrying a glyph rather than a
/// label (the appbar's `+` │ `grid` combo). `content` owns its own padding.
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

/// The bordered wrapper a joined segmented-control group sits in — 1px
/// `BORDER`, radius 6, no internal gap between segments
/// (`src/gui/widgets/buttons.rs:119-127`, `skip_perms_seg`'s container).
pub fn seg_group(content: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .border_1()
        .border_color(c::BORDER())
        .rounded(rpx(RADIUS_GROUP))
        .child(content)
}

/// Command palette row height — taller than the shared 28px [`ROW_H`]-style
/// modal row, per the palette redesign (`src/gui/widgets/rows.rs:73`,
/// `PALETTE_ROW_H`).
pub const PALETTE_ROW_H: f32 = 54.0;

/// A fixed 24px icon slot so titles line up across rows regardless of glyph
/// width (`src/gui/widgets/rows.rs:27-51`, `palette_agent_content`'s
/// `icon_slot`).
pub fn icon_slot(name: &str, size: f32, color: Hsla) -> Div {
    div()
        .w(rpx(ICON_SLOT_W))
        .flex()
        .items_center()
        .justify_center()
        .child(crate::icons::icon(name, size, color))
}

/// The cue chip shown in the palette's leading glyph slot when a drill-in
/// (Switch to session, Settings) replaces the search icon: mono, cyan text
/// over a soft cyan tint (`src/gui/session_launcher/view/mod.rs:44-59`).
pub fn cue_chip(label: impl Into<SharedString>) -> Div {
    div()
        .px(rpx(SPACE_MD))
        .py(rpx(SPACE_XS))
        .rounded(rpx(RADIUS_CONTROL))
        .bg(c::SEL_TINT_SOFT())
        .child(mono(label, TEXT_MICRO, c::CYAN()))
}

/// A palette results row: [`PALETTE_ROW_H`]-tall, radius 6, 12px horizontal
/// padding. `selected` gets the cyan tint + ring the iced original paints as
/// a gradient (`src/gui/widgets/rows.rs:88-141`, `launcher_row`) — flattened
/// here to a solid [`crate::theme::SEL_TINT_SOFT`] fill, since gpui's `Div`
/// has no gradient-background primitive at this rev.
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
