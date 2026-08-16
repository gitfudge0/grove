//! ThemePicker and ThemeManager (list + paste-first editor). Ports `src/gui/view/modals/theme_{picker,manager}.rs`
//! and `src/gui/theme_manager_editor.rs`. The live preview goes through one stubbed hook (carried decision 7).

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

/// Shared by both preview strips so a theme reads the same size wherever it is previewed.
const SWATCH_SIZE: f32 = ICON_XS;

/// Deliberately narrower than the full 11-swatch strip: it clips rather than pushing the row's icons out of alignment.
const SWATCH_COL_W: f32 = 90.0;

const CARET_H: f32 = TEXT_BODY * 1.1;

const EMPTY_STATE_PY: f32 = SPACE_3XL * 2.0;

/// Ported to gpui's icon sprite + the built-in `.tooltip()` hint (`theme_manager.rs:192-218`).
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

/// `ThemeManager`'s row preview (`theme_manager.rs:18-37`); distinct from [`swatch`]'s 4-color glance.
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
        .child(mono(label, TEXT_MICRO, c::FG_MUTE()))
}

/// The ONE hook `terminal_element` consults: `Some(inner)` wins over the persisted pin; `None` previews the global.
#[derive(Clone, Default)]
pub struct ThemePreview {
    pub project: Option<(String, Option<Theme>)>,
    // Written by the picker; read back only by a `#[cfg(test)]` assertion (render reads through `for_project`).
    #[allow(dead_code)]
    pub app: Option<Theme>,
}

impl gpui::Global for ThemePreview {}

impl ThemePreview {
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

/// Builtins first, then custom, in the order every selection surface agrees on.
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

    /// Selects and live-previews; doesn't commit (Enter/save still does that).
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
        // delta 0 reuses move's clamp + follow-system/use-default clear + preview.
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
                    *follow_system = false;
                }
                ThemePickerScope::Project(_) => {
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

    /// The single live-preview driver; no second override path exists.
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
                // While "follow system" is checked, keep showing the resolved system theme, not the tab's list selection.
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
        // Commit consumes `original`, making `restore_theme_before_leaving`'s restore a no-op on this path.
        if let Some(Modal::ThemePicker { original, .. }) = self.slot.get_mut() {
            original.clear();
        }
        self.cancel(cx);
    }

    /// Does not leave the modal itself — that's [`ModalLayer::cancel`]'s job; this runs as its first step.
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
            // Editing a theme invalidates the PTY render path (`modals.rs:214-219`).
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
                cx.refresh_windows();
                cx.notify();
            }
            Err(e) => self.open(Modal::Message(format!("Theme save failed: {e}")), cx),
        }
    }
}

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

    let mut rows: Vec<AnyElement> = Vec::new();
    // Not `sel`: the project scope prepends a "Default (follow app)" row.
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
    // `card` interleaves a divider after every row but the last, so row `k` sits at child `2k`.
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

fn theme_row(
    id: impl Into<gpui::ElementId>,
    active: bool,
    dispatch: &ModalDispatch,
    click: ModalClick,
    content: impl IntoElement,
) -> gpui::Stateful<gpui::Div> {
    click_row(id, active, RowDensity::CardPadded, dispatch, click, content)
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
        // The one sanctioned multiline exception to `field_box`'s single-line contract: a 14-row JSON buffer needs its own bordered box.
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
                // Tab indents inside a multiline buffer (carried decision 2).
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

    // Swaps the whole panel for a confirm dialog, matching every other destructive confirmation (`theme_manager.rs:55-86`).
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
                // No real `Input` field exists for this row; falls back to a styled buffer + caret glyph (`theme_manager.rs:106-141`).
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
                            // No mouse-driven rename-cancel `ModalClick` exists; re-seeding via `ThemeRenameStart` produces the same result.
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

    // Settings modal's header shape: every row action here persists immediately, so no Cancel/Save footer is needed.
    let header = modal_header_with_close("tm-close", "Manage themes", c::MAGENTA(), dispatch);

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
