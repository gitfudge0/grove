//! ThemePicker and ThemeManager (list + paste-first editor).
//!
//! Ports `src/gui/view/modals/theme_picker.rs:17+`,
//! `src/gui/view/modals/theme_manager.rs:19,43` and
//! `src/gui/theme_manager_editor.rs`.
//!
//! **The live preview goes through the single stubbed hook** at
//! `crate::terminal_element::project_theme_override`'s `preview` argument
//! (carried decision 7) — there is no second theme-override path.

use crate::views::rpx;
use crate::views::tokens::*;
use gpui::{
    div, prelude::*, px, AnyElement, App, Context, Focusable as _, Hsla, SharedString, Window,
};
use grove_core::theme::{Theme, ThemeKind};

use crate::settings::SettingsState;
use crate::theme as c;

use super::{Modal, ModalClick, ModalDispatch, ModalLayer};
use crate::modal::{ThemePickerReturn, ThemePickerScope};
use crate::views::components::{
    body_action, body_text, caption, card, click_action, click_checkbox, click_row, divider_h,
    icon_btn, modal_body, modal_footer, modal_footer_hints, modal_header_with_close, modal_panel,
    mono, note_text, seg_button, seg_group, ui, ModalBtn, OnToggle, RowDensity, SegSide,
};

// ── local layout geometry (§8.4: geometry lives in the owning module) ─────

/// One palette swatch's box. Both preview strips — the picker row's 4-colour
/// glance and the manager row's 11-colour strip — share it, so a theme reads
/// the same size wherever it is previewed.
const SWATCH_SIZE: f32 = ICON_XS;

/// The manager row's swatch column. Deliberately narrower than the full
/// 11-swatch strip: the strip clips rather than pushing the row's action
/// icons out of alignment.
const SWATCH_COL_W: f32 = 90.0;

/// The fake rename caret's height. Derived from [`TEXT_BODY`] with a touch of
/// overshoot so the bar spans the glyph box rather than the x-height (§14
/// case 2 — derived geometry, not a scale value).
const CARET_H: f32 = TEXT_BODY * 1.1;

/// Vertical breathing room for an inline "nothing here" block inside a modal
/// body — one modal zone padding step above and below.
const EMPTY_STATE_PY: f32 = SPACE_3XL * 2.0;

/// A tooltip-carrying icon mini button for a `ThemeManager` row action
/// (edit/rename/duplicate/delete) — `src/gui/widgets/buttons.rs`'
/// `action_mini`/`action_mini_danger`, ported to gpui's icon sprite + the
/// built-in `.tooltip()` hover hint (`theme_manager.rs:192-218`).
fn action_mini(
    id: &'static str,
    icon_name: &'static str,
    hint: &'static str,
    danger: bool,
    dispatch: &ModalDispatch,
    click: ModalClick,
) -> AnyElement {
    let dispatch = std::rc::Rc::clone(dispatch);
    let color = if danger { c::RED() } else { c::FG_MUTE() };
    icon_btn(
        id,
        icon_name,
        CONTROL_H,
        CONTROL_H,
        ICON_SM,
        color,
        c::BG_HOVER(),
        None,
        false,
        move |window, cx| dispatch(click.clone(), window, cx),
    )
    .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(hint).build(window, cx))
    .into_any_element()
}

/// The 11-swatch palette strip previewing a whole theme, in
/// `grove_core::theme::FIELD_NAMES` order — `ThemeManager`'s row preview
/// (`theme_manager.rs:18-37`). Distinct from [`swatch`], which is the
/// 4-color glance `ThemePicker` rows use.
///
/// Every colour in this element belongs to the *previewed* theme, border
/// included (§4.4: never mix bare accessors with `_of` variants in one
/// element). The swatch fills are the theme's raw tier-1 fields by
/// construction — showing all eleven is the point of the strip.
fn swatch_strip(t: &Theme) -> impl IntoElement {
    let border = Hsla::from(c::border_of(t));
    let mut strip = div().flex().items_center().gap(rpx(SPACE_XS));
    for i in 0..grove_core::theme::FIELD_NAMES.len() {
        let color = Hsla::from(c::ic(t.field(i)));
        strip = strip.child(
            div()
                .size(rpx(SWATCH_SIZE))
                .rounded(rpx(SWATCH_RADIUS))
                .border_1()
                .border_color(border)
                .bg(color),
        );
    }
    strip
}

/// The DARK/LIGHT kind badge next to a `ThemeManager` row's name
/// (`theme_manager.rs:145-160`).
fn kind_badge(kind: ThemeKind) -> impl IntoElement {
    let label: SharedString = match kind {
        ThemeKind::Dark => "DARK".into(),
        ThemeKind::Light => "LIGHT".into(),
    };
    div()
        .px(rpx(SPACE_SM))
        .py(rpx(SPACE_XS))
        .rounded(rpx(RADIUS_CONTROL))
        .border_1()
        .border_color(c::BORDER())
        .bg(c::BG_HL())
        // DARK/LIGHT reads as a token, not language (§5.2).
        .child(mono(label, TEXT_MICRO, c::FG_MUTE()))
}

/// The live project-theme preview an open picker is driving, if any.
///
/// This is the ONE hook `terminal_element` consults (`terminal_element.rs:156`
/// no longer says "Plan 08 will"): `Some(inner)` wins outright over the
/// persisted pin, and `inner == None` means "preview the global theme",
/// matching `project_use_default`.
#[derive(Clone, Default)]
pub struct ThemePreview {
    /// Project name being previewed, and the theme to show for it.
    pub project: Option<(String, Option<Theme>)>,
    /// App-scope preview: the whole window shows this theme until the picker
    /// commits or cancels.
    // Written by the picker and read back only by `views::modals`'
    // `#[cfg(test)]` "preview global is clear" assertion; the render path reads
    // the global through `ThemePreview::for_project`.
    #[allow(dead_code)]
    pub app: Option<Theme>,
}

impl gpui::Global for ThemePreview {}

impl ThemePreview {
    /// What `terminal_element` should pass as `preview` for `project_name`.
    pub fn for_project(cx: &App, project_name: &str) -> Option<Option<Theme>> {
        let p = cx.try_global::<ThemePreview>()?;
        let (name, theme) = p.project.as_ref()?;
        (name == project_name).then(|| theme.clone())
    }

    fn set(cx: &mut App, preview: ThemePreview) {
        cx.set_global(preview);
        cx.refresh_windows();
    }

    fn clear(cx: &mut App) {
        cx.set_global(ThemePreview::default());
        cx.refresh_windows();
    }
}

/// Every theme of `kind` a user can pick from, in the stable order every
/// selection surface agrees on: builtins first, then custom.
pub fn selectable(kind: ThemeKind) -> Vec<Theme> {
    grove_core::theme::selectable_themes_of(kind)
}

impl ModalLayer {
    /// Open the picker. `return_to` decides where cancelling lands.
    pub fn open_theme_picker(
        &mut self,
        scope: ThemePickerScope,
        return_to: ThemePickerReturn,
        cx: &mut Context<Self>,
    ) {
        let store = &cx.global::<SettingsState>().store;
        let original = store
            .theme
            .clone()
            .unwrap_or_else(|| crate::theme::DEFAULT_DARK_THEME.to_string());
        let follow_system = store.theme_follow_system;
        let project_use_default = match &scope {
            ThemePickerScope::App => false,
            ThemePickerScope::Project(name) => store
                .projects
                .iter()
                .find(|p| &p.name == name)
                .is_none_or(|p| p.theme.is_none()),
        };
        let sel_dark = selectable(ThemeKind::Dark)
            .iter()
            .position(|t| t.name == original)
            .unwrap_or(0);
        let sel_light = selectable(ThemeKind::Light)
            .iter()
            .position(|t| t.name == original)
            .unwrap_or(0);
        self.open(
            Modal::ThemePicker {
                sel_dark,
                sel_light,
                dark_tab: true,
                original,
                follow_system,
                scope,
                project_use_default,
                return_to,
            },
            cx,
        );
        self.preview_selected_theme(cx);
    }

    /// A click on a theme row: select it and live-preview, but don't commit
    /// (Enter/save still does that).
    pub(super) fn theme_picker_click(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(Modal::ThemePicker {
            sel_dark,
            sel_light,
            dark_tab,
            ..
        }) = self.slot.get_mut()
        {
            let sel = if *dark_tab { sel_dark } else { sel_light };
            *sel = i;
        }
        // delta 0 reuses move's clamp + "clears follow-system / use-default" + preview
        self.theme_picker_move(0, cx);
    }

    pub(super) fn theme_picker_move(&mut self, delta: i32, cx: &mut Context<Self>) {
        if let Some(Modal::ThemePicker {
            sel_dark,
            sel_light,
            dark_tab,
            follow_system,
            scope,
            project_use_default,
            ..
        }) = self.slot.get_mut()
        {
            let kind = if *dark_tab {
                ThemeKind::Dark
            } else {
                ThemeKind::Light
            };
            let len = selectable(kind).len();
            let sel = if *dark_tab { sel_dark } else { sel_light };
            *sel = crate::launcher::cycle(*sel, delta, len);
            match scope {
                ThemePickerScope::App => {
                    // Picking a concrete theme in app scope clears "follow system".
                    *follow_system = false;
                }
                ThemePickerScope::Project(_) => {
                    // Picking a concrete theme clears "Default (follow app)".
                    *project_use_default = false;
                }
            }
        }
        self.preview_selected_theme(cx);
    }

    pub(super) fn theme_picker_switch_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(Modal::ThemePicker { dark_tab, .. }) = self.slot.get_mut() {
            *dark_tab = !*dark_tab;
        }
        self.preview_selected_theme(cx);
    }

    /// The single live-preview driver. Both the picker and the launcher's
    /// theme pane call this; there is no second override path.
    pub(super) fn preview_selected_theme(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::ThemePicker {
            sel_dark,
            sel_light,
            dark_tab,
            follow_system,
            scope,
            project_use_default,
            ..
        }) = self.slot.get()
        else {
            return;
        };
        let kind = if *dark_tab {
            ThemeKind::Dark
        } else {
            ThemeKind::Light
        };
        let sel = if *dark_tab { *sel_dark } else { *sel_light };
        let theme = selectable(kind).get(sel).cloned();
        match scope {
            ThemePickerScope::App => {
                // While "follow system" is checked, keep the preview showing
                // the resolved system theme rather than snapping to the
                // tab's list selection (which would visually contradict the
                // still-checked checkbox).
                if *follow_system {
                    crate::theme::ThemeState::apply_system_theme(cx);
                } else if let Some(t) = &theme {
                    crate::theme::ThemeState::set_by_name(cx, &t.name);
                }
                ThemePreview::set(
                    cx,
                    ThemePreview {
                        project: None,
                        app: theme,
                    },
                );
            }
            ThemePickerScope::Project(name) => {
                let inner = if *project_use_default { None } else { theme };
                ThemePreview::set(
                    cx,
                    ThemePreview {
                        project: Some((name.clone(), inner)),
                        app: None,
                    },
                );
            }
        }
        cx.notify();
    }

    /// Enter: commit. App scope pins the theme (or `theme_follow_system`);
    /// project scope pins or clears `Project::theme`.
    pub(super) fn theme_picker_submit(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::ThemePicker {
            sel_dark,
            sel_light,
            dark_tab,
            follow_system,
            scope,
            project_use_default,
            ..
        }) = self.slot.get()
        else {
            return;
        };
        let kind = if *dark_tab {
            ThemeKind::Dark
        } else {
            ThemeKind::Light
        };
        let sel = if *dark_tab { *sel_dark } else { *sel_light };
        let name = selectable(kind).get(sel).map(|t| t.name.to_string());
        let (follow_system, use_default, scope) =
            (*follow_system, *project_use_default, scope.clone());
        match scope {
            ThemePickerScope::App => {
                let n = name.clone();
                SettingsState::update(cx, move |store| {
                    store.theme_follow_system = follow_system;
                    if !follow_system {
                        store.theme.clone_from(&n);
                        match kind {
                            ThemeKind::Dark => store.theme_dark = n,
                            ThemeKind::Light => store.theme_light = n,
                        }
                    }
                });
                let store = &cx.global::<SettingsState>().store;
                let dark_name = store
                    .theme_dark
                    .clone()
                    .unwrap_or_else(|| crate::theme::DEFAULT_DARK_THEME.to_string());
                let light_name = store
                    .theme_light
                    .clone()
                    .unwrap_or_else(|| crate::theme::DEFAULT_LIGHT_THEME.to_string());
                cx.update_global::<crate::theme::ThemeState, _>(|state, _| {
                    state.follow_system = follow_system;
                    state.dark_name = dark_name;
                    state.light_name = light_name;
                });
                if follow_system {
                    crate::theme::ThemeState::apply_system_theme(cx);
                } else if let Some(n) = &name {
                    crate::theme::ThemeState::set_by_name(cx, n);
                }
            }
            ThemePickerScope::Project(project) => {
                let n = name.clone();
                SettingsState::update(cx, move |store| {
                    if let Some(p) = store.projects.iter_mut().find(|p| p.name == project) {
                        p.theme = if use_default { None } else { n };
                    }
                });
            }
        }
        SettingsState::flush_now(cx);
        ThemePreview::clear(cx);
        // Commit **consumes** `original`: the picker leaves through
        // `ModalLayer::cancel`, which restores `original` on the way out, and
        // the theme just pinned above is precisely the one that must survive.
        // Emptying it here is what makes that restore a no-op on this path —
        // see [`Self::restore_theme_before_leaving`].
        if let Some(Modal::ThemePicker { original, .. }) = self.slot.get_mut() {
            original.clear();
        }
        self.cancel(cx);
    }

    /// Undo the live preview on the way out of a `ThemePicker`: restore
    /// `original` and drop the [`ThemePreview`] global
    /// (`src/app/modal.rs:74-94`). Does **not** leave the modal — that is
    /// [`ModalLayer::cancel`]'s job, and this runs as its first step, while
    /// the slot still holds the picker.
    ///
    /// `original` is consumed, mirroring how `ModalSlot::cancel` consumes
    /// `return_to` on the same exit: an empty `original` means "there is
    /// nothing to go back to", which is exactly the state
    /// [`Self::theme_picker_submit`] leaves behind once it has pinned the new
    /// theme. Self-guarding — a no-op unless a `ThemePicker` is open.
    pub(super) fn restore_theme_before_leaving(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::ThemePicker { original, .. }) = self.slot.get_mut() else {
            return;
        };
        let original = std::mem::take(original);
        if original.is_empty() {
            return;
        }
        crate::theme::ThemeState::set_by_name(cx, &original);
        ThemePreview::clear(cx);
    }

    // ── ThemeManager ────────────────────────────────────────────────────

    pub fn open_theme_manager(&mut self, cx: &mut Context<Self>) {
        self.open(
            Modal::ThemeManager {
                selected: 0,
                rename: None,
                rename_error: None,
                pending_delete: None,
                editor: None,
            },
            cx,
        );
    }

    pub(super) fn theme_manager_move(&mut self, delta: i32, cx: &mut Context<Self>) {
        let len = grove_core::theme::all_custom_themes().len();
        if let Some(Modal::ThemeManager { selected, .. }) = self.slot.get_mut() {
            *selected = crate::launcher::cycle(*selected, delta, len);
        }
        cx.notify();
    }

    pub(super) fn theme_manager_delete_confirm(&mut self, cx: &mut Context<Self>) {
        let name = match self.slot.get() {
            Some(Modal::ThemeManager { pending_delete, .. }) => pending_delete.clone(),
            _ => None,
        };
        if let Some(name) = name {
            if let Err(e) = crate::theme::delete_custom_theme(&name) {
                self.open(Modal::Message(format!("Delete failed: {e}")), cx);
                return;
            }
            // Editing a theme invalidates the PTY render path
            // (`modals.rs:214-219`).
            cx.refresh_windows();
        }
        if let Some(Modal::ThemeManager { pending_delete, .. }) = self.slot.get_mut() {
            *pending_delete = None;
        }
        cx.notify();
    }

    pub(super) fn theme_manager_delete_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(Modal::ThemeManager { pending_delete, .. }) = self.slot.get_mut() {
            *pending_delete = None;
        }
        cx.notify();
    }

    pub(super) fn theme_manager_rename_submit(&mut self, cx: &mut Context<Self>) {
        let pair = match self.slot.get() {
            Some(Modal::ThemeManager { rename, .. }) => rename.clone(),
            _ => None,
        };
        let Some((from, to)) = pair else { return };
        if grove_core::theme::by_name(&to).is_some() {
            if let Some(Modal::ThemeManager { rename_error, .. }) = self.slot.get_mut() {
                *rename_error = Some(format!("'{to}' already exists"));
            }
            cx.notify();
            return;
        }
        if let Err(e) = crate::theme::rename_custom_theme(&from, &to) {
            if let Some(Modal::ThemeManager { rename_error, .. }) = self.slot.get_mut() {
                *rename_error = Some(e);
            }
            cx.notify();
            return;
        }
        if let Some(Modal::ThemeManager {
            rename,
            rename_error,
            ..
        }) = self.slot.get_mut()
        {
            *rename = None;
            *rename_error = None;
        }
        cx.refresh_windows();
        cx.notify();
    }

    pub(super) fn theme_manager_rename_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(Modal::ThemeManager {
            rename,
            rename_error,
            ..
        }) = self.slot.get_mut()
        {
            *rename = None;
            *rename_error = None;
        }
        cx.notify();
    }

    /// The paste-first editor sub-view's Save.
    pub(super) fn theme_editor_save(&mut self, cx: &mut Context<Self>) {
        self.sync_wizard_buffers(cx);
        let buffer = match self.slot.get() {
            Some(Modal::ThemeManager { editor, .. }) => editor.clone(),
            _ => None,
        };
        let Some(buffer) = buffer else { return };
        match crate::theme::save_custom_theme_json(&buffer) {
            Ok(()) => {
                if let Some(Modal::ThemeManager { editor, .. }) = self.slot.get_mut() {
                    *editor = None;
                }
                // Re-colors the terminal immediately.
                cx.refresh_windows();
                cx.notify();
            }
            Err(e) => self.open(Modal::Message(format!("Theme save failed: {e}")), cx),
        }
    }
}

// ── the views ────────────────────────────────────────────────────────────

pub fn render(
    layer: &ModalLayer,
    dispatch: &ModalDispatch,
    window: &Window,
    cx: &App,
) -> AnyElement {
    match layer.slot().get() {
        Some(Modal::ThemePicker { .. }) => picker(layer, dispatch, cx),
        Some(Modal::ThemeManager { .. }) => manager(layer, dispatch, window, cx),
        _ => div().into_any_element(),
    }
}

/// The picker row's 4-colour glance. Like [`swatch_strip`], every colour here
/// is the *previewed* theme's — `_of(theme)` throughout, never a bare
/// accessor (§4.4).
fn swatch(t: &Theme) -> impl IntoElement {
    div().flex().items_center().gap(rpx(SPACE_SM)).children(
        [c::bg_of(t), c::fg_of(t), c::blue_of(t), c::green_of(t)].map(|col| {
            div()
                .size(rpx(SWATCH_SIZE))
                .rounded(rpx(SWATCH_RADIUS))
                .bg(gpui::Hsla::from(col))
        }),
    )
}

fn picker(layer: &ModalLayer, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    let Some(Modal::ThemePicker {
        sel_dark,
        sel_light,
        dark_tab,
        follow_system,
        scope,
        project_use_default,
        ..
    }) = layer.slot().get()
    else {
        return div().into_any_element();
    };
    let _ = cx;
    let kind = if *dark_tab {
        ThemeKind::Dark
    } else {
        ThemeKind::Light
    };
    let sel = if *dark_tab { *sel_dark } else { *sel_light };
    let themes = selectable(kind);

    let tabs = seg_group(
        div()
            .flex()
            .items_center()
            .child(seg_button(
                "tp-dark",
                "Dark",
                *dark_tab,
                SegSide::Left,
                false,
                (!*dark_tab).then(|| -> OnToggle {
                    let dispatch = std::rc::Rc::clone(dispatch);
                    Box::new(move |window, cx| {
                        dispatch(ModalClick::ThemePickerTab(true), window, cx);
                    })
                }),
            ))
            .child(seg_button(
                "tp-light",
                "Light",
                !*dark_tab,
                SegSide::Right,
                false,
                (*dark_tab).then(|| -> OnToggle {
                    let dispatch = std::rc::Rc::clone(dispatch);
                    Box::new(move |window, cx| {
                        dispatch(ModalClick::ThemePickerTab(false), window, cx);
                    })
                }),
            )),
    );

    // Shared `click_row`s (§9.1.1's `RowDensity::Card` shape, the same one
    // `setting_row_link` in `settings.rs` uses) sitting inside a `card()`.
    let mut rows: Vec<AnyElement> = Vec::new();
    // Where the selected theme lands in `rows`; the project scope prepends a
    // "Default (follow app)" row, so it isn't `sel`.
    let mut selected_row: Option<usize> = None;
    if matches!(scope, ThemePickerScope::Project(_)) {
        rows.push(
            theme_row(
                "tp-default",
                *project_use_default,
                dispatch,
                ModalClick::ThemePickerUseDefault,
                mono(
                    "Default (follow app)",
                    TEXT_BODY,
                    if *project_use_default {
                        c::FG()
                    } else {
                        c::FG_DIM()
                    },
                ),
            )
            .into_any_element(),
        );
    }
    for (i, t) in themes.iter().enumerate() {
        let active = i == sel && !*project_use_default;
        if i == sel {
            selected_row = Some(rows.len());
        }
        let content = div()
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .child(mono(
                t.name.to_string(),
                TEXT_BODY,
                if active { c::FG() } else { c::FG_DIM() },
            ))
            .child(swatch(t));
        rows.push(
            theme_row(
                gpui::SharedString::from(format!("tp-{i}")),
                active,
                dispatch,
                ModalClick::SelectRow(i),
                content,
            )
            .into_any_element(),
        );
    }
    // Retires the hard-coded 8-row window: the whole list renders and the
    // shared MODAL_SCROLL_MAX_H cap scrolls it, matching the manager list's
    // own scroll container below. The `card` itself is the scroll container
    // rather than a child of one, because `scroll_to_item` addresses the
    // tracked element's *direct* children — and those are the card's rows.
    // `card` interleaves a divider after every row but the last, so row `k`
    // sits at child `2k`.
    if let Some(k) = selected_row {
        layer.scroll_list_to(usize::from(*dark_tab), sel, k * 2);
    }
    let list = card(rows)
        .id("theme-picker-list")
        .max_h(rpx(MODAL_SCROLL_MAX_H))
        .overflow_y_scroll()
        .track_scroll(&layer.list_scroll);

    let title = match scope {
        ThemePickerScope::App => "Theme".to_string(),
        ThemePickerScope::Project(p) => format!("Project theme — {p}"),
    };

    let mut body = div().flex().flex_col().gap(rpx(SPACE_XL));
    if matches!(scope, ThemePickerScope::App) {
        body = body.child(click_checkbox(
            "tp-follow",
            "Follow system appearance",
            *follow_system,
            c::MAGENTA(),
            true,
            dispatch,
            ModalClick::ThemePickerToggleFollowSystem,
        ));
    }
    body = body.child(tabs).child(list);

    modal_panel(
        MODAL_W_LG,
        div()
            .child(modal_header_with_close(
                "tp-close",
                title,
                c::MAGENTA(),
                dispatch,
            ))
            .child(divider_h())
            .child(modal_body(body))
            .child(modal_footer(
                &[("↑↓", "select"), ("⏎", "apply"), ("esc", "cancel")],
                vec![
                    click_action(
                        "tp-cancel",
                        "Cancel",
                        ModalBtn::Plain,
                        dispatch,
                        ModalClick::Cancel,
                    )
                    .into_any_element(),
                    click_action(
                        "tp-apply",
                        "Apply",
                        ModalBtn::Primary,
                        dispatch,
                        ModalClick::ThemePickerApply,
                    )
                    .into_any_element(),
                ],
            )),
    )
    .into_any_element()
}

/// A picker-list row: the shared [`click_row`] in the same [`RowDensity::Card`]
/// shape `setting_row_link` (`settings.rs`) uses, sitting inside a [`card`].
/// `content` carries the theme's name (mono, §5.2) plus the trailing
/// [`swatch`] glance, or just the name for the "Default (follow app)" row.
fn theme_row(
    id: impl Into<gpui::ElementId>,
    active: bool,
    dispatch: &ModalDispatch,
    click: ModalClick,
    content: impl IntoElement,
) -> gpui::Stateful<gpui::Div> {
    click_row(id, active, RowDensity::Card, dispatch, click, content)
        .min_h(rpx(ROW_MIN_H))
        .px(rpx(ROW_PX))
        .py(rpx(ROW_PY))
}

fn manager(layer: &ModalLayer, dispatch: &ModalDispatch, window: &Window, cx: &App) -> AnyElement {
    let Some(Modal::ThemeManager {
        selected,
        rename,
        rename_error,
        pending_delete,
        editor,
    }) = layer.slot().get()
    else {
        return div().into_any_element();
    };

    // The editor sub-view wins over the list (`modals.rs:186-228`).
    if editor.is_some() {
        // The ONE sanctioned multiline exception to `field_box`'s single-line
        // contract (see its doc comment): a 14-row JSON buffer can't fit a
        // `FIELD_PY`-padded box sized for one line, so this field keeps its
        // own bordered box instead. It still owes the app's zeroed-inset half
        // of that contract — `.pl/.pr/.py` zeroed on the wrapped `Input` so
        // its own padding doesn't double up with this box's — via the same
        // five calls `field_box`'s callers make.
        //
        // The other half of the contract is honoured too: a focus-reactive
        // border, `c::MAGENTA()` focused / `c::BORDER()` at rest, the way
        // every other field in the app behaves.
        let field = layer.fields.first().map(|f| {
            let focused = f.state().read(cx).focus_handle(cx).is_focused(window);
            div()
                .w_full()
                .px(rpx(SPACE_XL))
                .py(rpx(SPACE_MD))
                .rounded(rpx(RADIUS_GROUP))
                .bg(c::BG())
                .border_1()
                .border_color(if focused { c::MAGENTA() } else { c::BORDER() })
                .child(
                    gpui_component::input::Input::new(f.state())
                        .appearance(false)
                        .pl(px(0.0))
                        .pr(px(0.0))
                        .py(px(0.0))
                        .w_full(),
                )
        });
        return modal_panel(
            MODAL_W_LG,
            div()
                .child(modal_header_with_close(
                    "tm-editor-close",
                    "Theme editor",
                    c::MAGENTA(),
                    dispatch,
                ))
                .child(divider_h())
                .child(modal_body(
                    div()
                        .flex()
                        .flex_col()
                        .gap(rpx(SPACE_LG))
                        .child(body_text(
                            "Paste a theme JSON object, or edit the one below.",
                        ))
                        .children(field),
                ))
                // Tab INDENTS inside a multiline buffer (carried decision 2);
                // the footer says so rather than pretending it traverses.
                .child(modal_footer(
                    &[("tab", "indent"), ("esc", "back")],
                    vec![
                        click_action(
                            "tm-editor-cancel",
                            "Cancel",
                            ModalBtn::Plain,
                            dispatch,
                            ModalClick::Cancel,
                        )
                        .into_any_element(),
                        click_action(
                            "tm-save",
                            "Save",
                            ModalBtn::Primary,
                            dispatch,
                            ModalClick::ThemeEditSave,
                        )
                        .into_any_element(),
                    ],
                )),
        )
        .into_any_element();
    }

    // A pending delete swaps the whole panel for a confirm-shaped dialog
    // (header/body/footer) rather than an inline row, matching every other
    // destructive confirmation in the app (`theme_manager.rs:55-86`). Key
    // handling (y/esc) is unaffected — it lives in `crate::modal`.
    if let Some(name) = pending_delete {
        let body_zone = div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .child(body_text(format!("Delete theme \"{name}\"?")))
            .child(caption("This cannot be undone."));
        return modal_panel(
            MODAL_W_LG,
            div()
                .child(modal_header_with_close(
                    "tm-del-close",
                    "Delete theme",
                    c::RED(),
                    dispatch,
                ))
                .child(divider_h())
                .child(modal_body(body_zone))
                .child(modal_footer(
                    &[("y", "delete"), ("esc", "cancel")],
                    vec![
                        click_action(
                            "tm-del-cancel",
                            "Cancel",
                            ModalBtn::Plain,
                            dispatch,
                            ModalClick::ThemeDeleteCancel,
                        )
                        .into_any_element(),
                        click_action(
                            "tm-del-confirm",
                            "Delete",
                            ModalBtn::Danger,
                            dispatch,
                            ModalClick::ThemeDeleteConfirm,
                        )
                        .into_any_element(),
                    ],
                )),
        )
        .into_any_element();
    }

    let themes = grove_core::theme::all_custom_themes();
    let list_content: AnyElement = if themes.is_empty() {
        div()
            .w_full()
            .px(rpx(SPACE_3XL))
            .py(rpx(EMPTY_STATE_PY))
            .flex()
            .items_center()
            .justify_center()
            .child(ui(
                "No custom themes yet — create one or paste a palette.",
                TEXT_BODY,
                c::FG_MUTE(),
            ))
            .into_any_element()
    } else {
        let mut list = div().flex().flex_col().gap(rpx(SPACE_SM));
        for (i, t) in themes.iter().enumerate() {
            let is_renaming = rename.as_ref().is_some_and(|(from, _)| *from == t.name);
            let row_el: AnyElement = if is_renaming {
                let buf = rename
                    .as_ref()
                    .map_or_else(|| t.name.to_string(), |(_, buf)| buf.clone());
                // A real gpui-component `Input` needs a field `mod.rs` owns
                // (`build_fields` has no ThemeManager-rename arm and this
                // file cannot add one), so this falls back to a styled
                // editable-looking buffer row: the live rename buffer plus a
                // blinking-caret glyph, in the same selected-tint container
                // the iced original uses (`theme_manager.rs:106-141`).
                let mut col = div().flex().flex_col().gap(rpx(SPACE_SM)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_MD))
                        .w_full()
                        .child(
                            div()
                                .flex_1()
                                .px(rpx(SPACE_LG))
                                .py(rpx(SPACE_SM))
                                .rounded(rpx(RADIUS_CONTROL))
                                .bg(c::BG())
                                .border_1()
                                .border_color(c::BORDER())
                                .flex()
                                .items_center()
                                // The rename buffer holds a theme name — a
                                // token, so mono (§5.2).
                                .child(mono(buf, TEXT_BODY, c::FG()))
                                .child(
                                    div()
                                        .w(px(1.0))
                                        .h(rpx(CARET_H))
                                        .ml(rpx(SPACE_XS))
                                        .bg(c::FG_DIM()),
                                ),
                        )
                        .child(body_action(
                            "tm-rename-save",
                            "Save",
                            c::CYAN(),
                            dispatch,
                            ModalClick::ThemeRenameCommit,
                        ))
                        .child(body_action(
                            "tm-rename-cancel",
                            "Cancel",
                            c::CYAN(),
                            dispatch,
                            // No `ModalClick` variant exists for a
                            // mouse-driven rename-cancel (only
                            // `ModalAction::ThemeManagerRenameCancel`,
                            // reached from the keyboard verdict table in
                            // `crate::modal`, which `ModalDispatch`
                            // cannot invoke) — `mod.rs` is read-only so
                            // one cannot be added here. `ThemeRenameStart`
                            // re-seeds `rename` to `(name, name)`, which
                            // is what Cancel would produce anyway since
                            // this fallback buffer never diverges from
                            // the original name (no live-typing dispatch
                            // exists either). Escape still cancels
                            // correctly via the keyboard path.
                            ModalClick::ThemeRenameStart(i),
                        )),
                );
                if let Some(err) = rename_error {
                    col = col.child(note_text(err.clone()));
                }
                div()
                    .w_full()
                    .px(rpx(SPACE_XL))
                    .py(rpx(SPACE_MD))
                    .rounded(rpx(RADIUS_GROUP))
                    .bg(c::SEL_TINT_STRONG())
                    .border_1()
                    .border_color(c::SEL_RING())
                    .child(col)
                    .into_any_element()
            } else {
                let active = i == *selected;
                let name_zone = div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_MD))
                    .overflow_hidden()
                    // A theme name is a token, not language (§5.2).
                    .child(mono(
                        t.name.to_string(),
                        TEXT_TITLE,
                        if active { c::FG() } else { c::FG_DIM() },
                    ))
                    .child(kind_badge(t.kind));
                let icons = div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_XS))
                    .child(action_mini(
                        "tm-edit",
                        "edit",
                        "edit",
                        false,
                        dispatch,
                        ModalClick::ThemeEditOpen(i),
                    ))
                    .child(action_mini(
                        "tm-rename",
                        "rename",
                        "rename",
                        false,
                        dispatch,
                        ModalClick::ThemeRenameStart(i),
                    ))
                    .child(action_mini(
                        "tm-dup",
                        "duplicate",
                        "duplicate",
                        false,
                        dispatch,
                        ModalClick::ThemeDuplicate(i),
                    ))
                    .child(action_mini(
                        "tm-del",
                        "trash",
                        "delete",
                        true,
                        dispatch,
                        ModalClick::ThemeDeleteRequest(i),
                    ));
                // The manager row's looser shape is `click_row`'s
                // `RowDensity::Manager`: it carries a name, a badge, an
                // eleven-swatch strip and four action buttons on one line.
                click_row(
                    gpui::SharedString::from(format!("tm-{i}")),
                    active,
                    RowDensity::Manager,
                    dispatch,
                    ModalClick::ThemeSelect(i),
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_XL))
                        .w_full()
                        .child(name_zone)
                        .child(
                            div()
                                .w(rpx(SWATCH_COL_W))
                                .overflow_hidden()
                                .child(swatch_strip(t)),
                        )
                        .child(icons),
                )
                .into_any_element()
            };
            list = list.child(row_el);
        }
        div()
            .id("theme-manager-list")
            .max_h(rpx(MODAL_SCROLL_MAX_H))
            .overflow_y_scroll()
            .w_full()
            .child(list)
            .into_any_element()
    };

    // Bare title + a close icon button — the Settings modal's header shape —
    // since like Settings every row action here persists immediately, there
    // is no unsaved state a Cancel/Save footer would guard
    // (`theme_manager.rs:214-222`).
    let header = modal_header_with_close("tm-close", "Manage themes", c::MAGENTA(), dispatch);

    // The footer's left slot is retired (plan.md §2): "+ New theme" moves
    // into the body as a flat magenta `body_action` at the foot of the list,
    // rather than the Primary button the old left slot forced it to be —
    // that was already a §9.1.1 contract violation, since a footer's left
    // cluster is low-emphasis by definition.
    let new_theme_btn = body_action(
        "tm-new",
        "+ New theme",
        c::MAGENTA(),
        dispatch,
        ModalClick::ThemeNew,
    );

    modal_panel(
        MODAL_W_LG,
        div()
            .child(header)
            .child(divider_h())
            .child(modal_body(
                div()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_LG))
                    .child(list_content)
                    .child(new_theme_btn),
            ))
            .child(modal_footer_hints(&[("↑↓", "select"), ("esc", "close")])),
    )
    .into_any_element()
}
