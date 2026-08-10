//! The app-wide shared component library: keycaps, text helpers, icon
//! buttons, status dots, dividers and the modal chrome. Every view builds
//! from it, not just the modals. Ported from `src/gui/widgets/modal.rs`.
//!
//! Pure presentation: every helper takes plain data and returns an element, so
//! nothing here needs an entity or a `Context`. Line references in each doc
//! comment point at the iced original.

use crate::views::rpx;
use crate::views::tokens::*;
use gpui::{div, prelude::*, px, AnyElement, Div, Hsla, MouseButton, SharedString};

use crate::fonts::{MONO_FAMILY, UI_FAMILY};
use crate::theme as c;

use super::dispatch::{ModalClick, ModalDispatch};

/// The focus ring's width, drawn as a zero-blur outer shadow spread around a
/// focused [`field_box`] (plan.md §1). Single-consumer geometry, so per
/// DESIGN.md §14 it is a module constant here rather than a `tokens.rs` scale
/// entry: it is *the ring's* width, not a notch on a shared scale.
const FOCUS_RING_W: f32 = 2.0;

/// [`modal_header_slotted`]'s close-button id when a caller passes none. A
/// modal has exactly one close button, so the constant is unambiguous within
/// any one modal — which is the whole scope gpui's element ids are keyed on.
const DEFAULT_CLOSE_ID: &str = "modal-close";

/// The checkbox's box side. Single-consumer geometry, so per DESIGN.md §14 it
/// is a module constant here rather than a `tokens.rs` scale entry: it is
/// *the checkbox's* box, not a notch on a shared scale.
const CHECKBOX_BOX: f32 = 14.0;

/// The checkbox tick's glyph size: [`CHECKBOX_BOX`] less the box's 1px border
/// on each side, so the mark fills the box without crossing the stroke. A
/// derived value in the `FOOTER_RADIUS` sense (§7.3, §14's second literal
/// case) — it moves with the box, it is never chosen independently. It lands
/// on [`ICON_SM`], the list-density glyph tier.
const CHECKBOX_TICK: f32 = CHECKBOX_BOX - 2.0;

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
/// The shadow's colour is [`crate::theme::PANEL_SHADOW`] and its geometry the
/// `PANEL_SHADOW_*` tokens, forked light/dark by [`crate::theme::is_dark`] —
/// both were a hard-coded `rgba(0,0,0,.35) 0 12px 40px` here, which is the one
/// piece of panel chrome that never tracked a theme swap (plan.md §3).
pub fn modal_panel(width: f32, content: impl IntoElement) -> Div {
    let (y, blur) = if c::is_dark() {
        (PANEL_SHADOW_Y, PANEL_SHADOW_BLUR)
    } else {
        (PANEL_SHADOW_Y_LIGHT, PANEL_SHADOW_BLUR_LIGHT)
    };
    div()
        .w(rpx(width))
        .p(px(1.0))
        .bg(c::BG_RAIL())
        .text_color(c::FG())
        .border_1()
        .border_color(c::BORDER())
        .rounded(rpx(RADIUS_PANEL))
        .shadow(vec![gpui::BoxShadow {
            color: c::PANEL_SHADOW(),
            offset: gpui::point(px(0.0), px(y)),
            blur_radius: px(blur),
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

/// [`keycap_text`] with no chip behind it — the statusbar footer's keycap
/// weight (plan.md §2). A filled chip per hint turned a left-aligned hint row
/// into a row of buttons competing with the actual button group on the right;
/// the glyph alone still reads as a key because the label beside it names the
/// action. Same shell (so padding and rhythm match a filled keycap), fill only
/// dropped.
pub fn keycap_text_flat(label: impl Into<SharedString>, color: Hsla) -> Div {
    keycap_filled(gpui::transparent_black(), mono(label, TEXT_SMALL, color))
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
///
/// `indent` is the label's left inset. It is an axis rather than a fixed
/// `SPACE_2XL` because a label naming a bordered [`card`] sits flush with the
/// card's edge, while a label over a flat list is inset to the list's text
/// column.
pub fn section_header(label: &str, indent: f32, top: f32, bottom: f32) -> Div {
    div()
        .pt(rpx(top))
        .pb(rpx(bottom))
        .pl(rpx(indent))
        .child(mono(label, TEXT_MICRO, c::FG_MUTE()))
}

/// One flat-keycap + muted label pair — the statusbar footer's hint shape
/// (plan.md §2). The bordered-keycap weight it replaced (a filled chip per
/// hint) retired with the old `BG_STRIP`-filled footer.
pub fn footer_hint_flat(key: &'static str, label: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_MD))
        .child(keycap_text_flat(key, c::FG_DIM()))
        .child(mono(label, TEXT_MICRO, c::FG_MUTE()))
}

/// The footer zone — variant C2g "statusbar" (plan.md §2). A 1px
/// [`divider_h`] over a **transparent** `py SPACE_LG` row sitting directly on
/// the panel's rail, replacing the old `BG_STRIP` strip.
///
/// The strip was the last surface in the app that fenced off a zone with a
/// fill rather than a rule, and a filled band under a modal read as a second
/// panel rather than that panel's last row. Dropping the fill also retired
/// the old inner-corner radius: with nothing painted in the corner there is
/// no inner radius to keep flush, so the panel's own [`RADIUS_PANEL`] is the
/// only corner left.
///
/// The divider is drawn **here**, not by the caller: a footer without its rule
/// is not a footer. Callers that still emit their own `divider_h()` above a
/// footer are the migration's work.
pub fn footer_container(content: impl IntoElement) -> Div {
    div().w_full().flex().flex_col().child(divider_h()).child(
        div()
            .w_full()
            .px(rpx(SPACE_3XL))
            .py(rpx(SPACE_LG))
            .child(content),
    )
}

/// **The** footer for every modal — plan.md §2's variant C2g "statusbar", in
/// one function: hints at the left edge, a spacer, buttons at the right.
///
/// One vocabulary everywhere. A hints-only modal (the launcher, the shortcut
/// overlay, the archived-project list) is *this* row minus the button group,
/// not a second component — which is why [`modal_footer_hints`] is now a call
/// to this rather than a layout of its own.
///
/// **The left slot is gone.** It held a caption, a flat destructive action, a
/// version string and — in one case — a Primary button, which is four different
/// things wearing one slot, and the Primary was a §9.1.1 violation outright.
/// Each of those relocates into the body as a [`body_action`] or into the
/// header's meta slot; nothing lands back here.
///
/// Hints are flat-keycap ([`footer_hint_flat`]) and left-aligned because the
/// footer now reads as a statusbar: the row's low-emphasis end is where the eye
/// starts, and the affirmative action is where it ends. Button order inside
/// `buttons` is the caller's — always secondary(`Plain`) then
/// affirmative(`Primary`/`Danger`). An empty `hints` or `buttons` contributes no
/// group at all, so a footer never carries an empty flex box whose gap counts.
pub fn modal_footer(hints: &[(&'static str, &'static str)], buttons: Vec<AnyElement>) -> Div {
    let mut row = div().flex().items_center();

    if !hints.is_empty() {
        let mut group = div().flex().items_center().gap(rpx(SPACE_3XL));
        for (key, label) in hints {
            group = group.child(footer_hint_flat(key, label));
        }
        row = row.child(group);
    }

    // Pushes the button group to the right edge whatever the hints measure —
    // no per-caller alignment, and no reflow when a hint's copy changes.
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

/// A footer of plain hints and no buttons — [`modal_footer`]'s second case,
/// spelled as a name because it is how a third of the modals end.
pub fn modal_footer_hints(hints: &[(&'static str, &'static str)]) -> Div {
    modal_footer(hints, Vec::new())
}

/// **The** modal header — plan.md §3's "one header component with optional
/// slots". Every header in the app is this function with a different set of
/// slots filled; the wizard's step counter, Settings' version line and
/// ScriptsEditor's subtitle were three forks of one shape.
///
/// - `title` / `accent` — always present.
/// - `meta` — the right-aligned metadata slot: a step counter ("Step 2 of 3"),
///   a version string, a status chip. Sits left of the close button.
/// - `subtitle` — a second line under the title, inside the same header zone.
/// - `dispatch` — `Some` renders the trailing close button, wired to
///   [`ModalClick::Cancel`]. `None` renders no close, which is how a
///   blocking-progress state (a teardown mid-run, an update installing) says
///   "there is nothing to cancel" *structurally* rather than by drawing a
///   button that does nothing.
/// - `id` — the close button's element id, which must be unique within the
///   modal (gpui bleeds hover state between duplicate ids). Only read when
///   `dispatch` is `Some`.
///
/// The close button sits at the **header icon tier** — an [`ICON_BTN_W`] (28)
/// box around an [`ICON_MD`] (14) glyph — which DESIGN.md §9.1.1 names verbatim
/// ("`flat_icon_btn` at the header icon tier (28 + `ICON_MD`)").
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

/// [`modal_header_slotted`] for callers whose title zone is not a plain
/// string — e.g. `ScriptsEditor`'s rename-capable title, which swaps the
/// static title for a live `Input` plus pencil/check/discard controls. Same
/// row shape (title content + `meta` + close, with `subtitle` stacked below)
/// as the string version, which delegates here with its title pre-rendered
/// as a `ui()` run.
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

/// [`modal_header_slotted`] plus a trailing close icon button dispatching
/// [`ModalClick::Cancel`] — the shape the theme manager and the archived-project
/// list each used to fork. Same token contract as [`modal_header_slotted`]:
/// `px/py SPACE_3XL` around a [`TEXT_TITLE`] title in `accent`.
///
/// The close button sits at the **header icon tier** — an [`ICON_BTN_W`] (28)
/// box around an [`ICON_MD`] (14) glyph — which DESIGN.md §9.1.1 names
/// verbatim ("`flat_icon_btn` at the header icon tier (28 + `ICON_MD`)"). It
/// was a bare [`icon_btn`] at [`CONTROL_H`] (22) + [`ICON_SM`] before the
/// modal-consistency sweep; that was drift toward the in-row control tier
/// (§8.1), not a second header tier.
///
/// `id` must be unique within the modal (gpui bleeds hover state between
/// duplicate ids).
pub fn modal_header_with_close(
    id: &'static str,
    title: impl Into<SharedString>,
    accent: Hsla,
    dispatch: &ModalDispatch,
) -> Div {
    modal_header_slotted(Some(id), title, accent, None, None, Some(dispatch))
}

/// [`modal_header_slotted`]'s internal row shell for callers needing more than
/// a bare title in the header zone, e.g. a title plus a right-aligned step
/// counter (`modal.rs:162-171`).
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
///
/// | Weight | Text | Border | Hover border | Fill |
/// |---|---|---|---|---|
/// | `Plain` | `FG_DIM` | `BORDER` | unchanged | `BG` (unfilled) |
/// | `Primary` | `FG` | `BORDER` | unchanged | `BG_HL` |
/// | `Danger` | `RED` | `RED` | unchanged | `BG_HL` |
///
/// Every weight also takes the shared `BG_HOVER` fill on hover.
///
/// The bordered `Accent` hero weight (the Add-project wizard's "Browse…"
/// button) retired with plan.md §3's in-body button sweep: "no bordered
/// buttons inside bodies" turned that hero button into a flat tinted
/// [`body_action`], leaving no caller for a fourth weight.
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
        match self {
            ModalBtn::Danger => c::RED(),
            ModalBtn::Plain | ModalBtn::Primary => c::BORDER(),
        }
    }

    /// The border on hover. No current weight moves on this axis; kept as a
    /// seam (rather than deleted outright) because [`modal_action_sized`]
    /// reads it unconditionally and a future weight is the likeliest way this
    /// gets used again.
    fn hover_border_color(self) -> Option<Hsla> {
        None
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

/// [`click_action`] with an enabled axis — the button shape for a footer's
/// primary action that cannot fire yet (an empty required field, a form with
/// nothing changed to save).
///
/// `enabled: false` keeps the box geometry (`px SPACE_2XL` / `py SPACE_MD`,
/// [`RADIUS_CONTROL`], 1px border) so the footer does not reflow when the
/// action becomes available (§2.4), and drops *everything* interactive:
/// `BORDER_SOFT` instead of the weight's border, the unfilled `BG`, a
/// [`FG_MUTE`](crate::theme::FG_MUTE) label, no `cursor_pointer`, no hover and
/// no `on_mouse_down` at all. Per DESIGN.md §10.1 the handler is *structurally*
/// absent rather than guarded by a boolean the paint path could forget, and per
/// §9.1.1 disabled is `FG_MUTE` plus a dropped handler and **never**
/// `opacity()` — a half-transparent button still paints a hover and still eats
/// the click.
///
/// Mirrors [`click_checkbox`]'s parameter order (`.., enabled, dispatch,
/// click`), which is the house shape for a dispatch-wired control with a
/// disabled state.
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

/// How roomy a [`click_row`] is, and how its fill is shaped. This is the axis
/// DESIGN.md §9.2 recorded as missing when `theme_picker`'s `manager_row`
/// forked the row shape; it is an enum rather than three padding parameters so
/// a call site picks a *density*, not a set of numbers.
///
/// | Variant | `px` | `py` | Corners | Fill |
/// |---|---|---|---|---|
/// | `Manager` | `SPACE_XL` | `SPACE_MD` | `RADIUS_GROUP` | inset, rounded |
/// | `Card` | `SPACE_XL` | from content | square | full-bleed |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowDensity {
    /// The theme manager's row, which carries a name, a badge, an
    /// eleven-swatch strip and four action buttons on one line.
    Manager,
    /// A row inside a hairline-bordered [`card`]. It takes its height from its
    /// content rather than from padding, and its hover fill is full-bleed and
    /// square: a rounded fill inset from the card's own edge would read as a
    /// second, floating surface inside the card.
    Card,
}

impl RowDensity {
    /// Horizontal padding.
    fn px(self) -> f32 {
        match self {
            RowDensity::Manager | RowDensity::Card => SPACE_XL,
        }
    }

    /// Vertical padding, or `None` where the row's content sets its height.
    fn py(self) -> Option<f32> {
        match self {
            RowDensity::Manager => Some(SPACE_MD),
            RowDensity::Card => None,
        }
    }

    /// Corner radius, or `None` for the square full-bleed fill.
    fn radius(self) -> Option<f32> {
        match self {
            RowDensity::Manager => Some(RADIUS_GROUP),
            RowDensity::Card => None,
        }
    }
}

/// A clickable list row, the shape every modal list shares. `density` picks
/// how much room it gives its content — see [`RowDensity`].
pub fn click_row(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    density: RowDensity,
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
        .px(rpx(density.px()))
        .map(|d| match density.py() {
            Some(py) => d.py(rpx(py)),
            // A `Card` row spans its card, so its fill must reach both edges.
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
        // The tick is a sprite, not a text run: §9.3 owns every mark in the
        // app, and the bundled fonts have no U+2713 — a literal "✓" fell back
        // to a stand-in glyph.
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

/// A modal's body zone: the `SPACE_3XL` padding every ported modal shares on
/// all four sides, with the caller choosing the rhythm *between* its children.
/// The top edge is the same notch as the sides and the bottom because a zone
/// pads uniformly (§6.1) — a header divider above it is a rule, not padding,
/// and the body used to sit flush against it.
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

/// Which of §9.1's two caption tones a [`row_sublabel`] takes. Named rather
/// than a `bool` per §14 rule 3's spirit: `SublabelTone::Safety` says at the
/// call site what `true` would not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SublabelTone {
    /// The default explanatory sublabel — [`caption`]'s `FG_MUTE`.
    Normal,
    /// One shade up, reserved for safety-relevant text (skip-permissions and
    /// friends) — `FG_DIM` rather than [`caption`]'s `FG_MUTE`.
    Safety,
}

/// The second line under a row's label. This is [`caption`] /
/// [`caption_promoted`]'s two-tone axis in the shape a row needs: no indent of
/// its own, because the row's label column already owns the left edge.
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

/// A filled, hairline-bordered card whose `rows` are separated by full-bleed
/// [`divider_h`] rules rather than by whitespace — the shape Settings' sections
/// and the project-settings theme block both draw. Same token set as a control
/// surface: `RADIUS_CONTROL`, 1px `BORDER`, `BG_STRIP` (§7.1, §7.2).
///
/// Rows carry their own padding; the card contributes none, so a row's fill can
/// reach the card's inner edge (see [`RowDensity::Card`]).
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

/// [`status_dot`]'s "absent / not installed" counterpart: the same circle at
/// the same size, drawn as a 1px ring instead of a fill.
///
/// The pair is a §2.3 case — colour is never the sole carrier of state. Filled
/// versus hollow is a *shape* difference, so present-versus-absent survives
/// greyscale, a colour-blind reader and a dimmed display, where two tints of
/// the same disc would not. Which is also why the ring keeps the full 1px
/// hairline in the state's own colour rather than a washed-out fill (§7.2).
pub fn status_dot_hollow(size: f32, color: Hsla) -> Div {
    div()
        .size(rpx(size))
        .rounded_full()
        .border_1()
        .border_color(color)
}

/// A 1px vertical hairline separating clusters inside a bar
/// (`src/gui/widgets/primitives.rs`'s `vline`).
pub fn vline() -> Div {
    div().w(px(1.0)).h(rpx(VLINE_H)).bg(c::BORDER())
}

/// A flat, borderless text button in the same 22px-tall shape as
/// [`flat_icon_btn`] — used for the zoom percentage's reset label
/// (`src/gui/widgets/buttons.rs:455-495`, `control_btn_sized`). Delegates to
/// [`flat_text_btn_tinted`] at `FG_DIM`, the weight every existing call site
/// wants, so none of them had to change when the colour axis was added.
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

/// [`flat_text_btn`] with the text colour as an axis, so a low-emphasis
/// destructive action ("Archive project") has a component to be instead of a
/// bare `ui()` run with a raw `on_mouse_down` and no button shape.
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

/// **The** app-wide text field — plan.md §1's variant C1c, "boxed + focus
/// ring", which replaces the old bare-bottom-rule underline field.
///
/// A full-width box: `px FIELD_PX` / `py FIELD_PY` (~32px tall around a
/// [`TEXT_BODY`] mono run), [`RADIUS_GROUP`] corners, a [`BG_STRIP`] fill and a
/// 1px [`BORDER_SOFT`] hairline. Focused, the hairline goes full-strength
/// `MAGENTA` and a [`FOCUS_RING_W`] ring in [`FOCUS_RING`] is drawn *outside*
/// it.
///
/// An underline could only say "focused" by recolouring one edge, which on a
/// bare panel left a field with no resting shape at all — an input the user had
/// to find by clicking. The box gives it one, and the ring keeps keyboard focus
/// legible in the single magenta language §3 settles on.
///
/// The ring is a zero-blur, [`FOCUS_RING_W`]-spread outer `BoxShadow` because
/// gpui's `Div` at this rev has no outline primitive — `border_2` would grow
/// the box and reflow the row on focus, and a wrapper div would need the same
/// radius maintained in two places. It is a ring in every respect but the API
/// it is spelled with.
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
/// `Input` applies its own `input_px`/`input_py` (10px/8px at the default
/// `Size::Medium`) **regardless of `.appearance(false)`** — `appearance` drops
/// the border and the fill, not the padding. That inset, not the surrounding
/// divs, is what breaks a field's left edge out of true against the rest of the
/// panel if left unzeroed, and `w_full()` is what stops the field from
/// collapsing to its content inside this shell's `min_w_0` flex row.
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

/// A flat tinted text action **inside** a modal body — plan.md §2/§3's
/// replacement for both the retired footer left slot (AgentPicker's "Default",
/// ThemeManager's "+ New theme", ScriptsEditor's "Archive project") and the
/// bordered in-body buttons §3 forbids ("Change", "Kill all sessions",
/// onboarding's "Browse…").
///
/// A bordered button inside a body reads as a second call to action competing
/// with the footer's affirmative one; flat tinted text reads as what it is — an
/// action available here, subordinate to the panel's own. `tone` carries the
/// action's flavour (`CYAN` for a neutral in-body action, `RED` for a
/// destructive one), which is the only axis these ever varied on.
///
/// [`flat_text_btn_tinted`] in the [`click_action`] shape: same shell, wired
/// straight to a [`ModalClick`] rather than to a bare closure. `id` must be
/// unique within the modal.
pub fn body_action(
    id: &'static str,
    label: impl Into<SharedString>,
    tone: Hsla,
    dispatch: &ModalDispatch,
    click: ModalClick,
) -> gpui::Stateful<Div> {
    let dispatch = std::rc::Rc::clone(dispatch);
    // `flat_text_btn_tinted`'s row-only users get shrink-to-content for free
    // from a horizontal flex parent; a column parent stretches children to
    // full width by default instead, which reads as a full-bleed band with a
    // centred label rather than a subordinate flat action. Pin this variant
    // to its content size and the leading edge so it reads correctly in
    // either axis.
    flat_text_btn_tinted(id, label, TEXT_BODY, SPACE_MD, tone, move |window, cx| {
        dispatch(click.clone(), window, cx);
    })
    .flex_none()
    .self_start()
    .justify_start()
}

/// A fixed [`STATUS_DOT_COL_W`] column holding an optional status mark.
/// Reserved on every settings row so labels align whether or not the row
/// carries a status — the same principle [`icon_slot`] applies to a fixed
/// glyph slot ("a fixed slot so titles align regardless of glyph width"),
/// applied here to the row grid instead of a palette row's leading icon.
///
/// Fixed at [`CONTROL_H`] tall and centred *inside that height*, not inside
/// the row's overall height: a row's outer container is `items_start` (so a
/// tall sublabel does not drag the whole row's cross-axis alignment around),
/// which pins this gutter's top edge to the row's first line. Matching that
/// line's own height — [`CONTROL_H`], the height of every in-row control —
/// is what puts the mark's centre on the label line rather than the row's
/// top edge (too high) or the row's overall centre (between the label and a
/// sublabel, which is worse).
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
        // Sized by [`CONTROL_H`] rather than by vertical padding, so a segment
        // is the same 22px as every other in-row control (§8.1) — the same
        // shape `flat_text_btn` uses. `py` would stack on top of the fixed
        // height, so there is none.
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
