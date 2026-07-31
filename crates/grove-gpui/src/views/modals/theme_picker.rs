//! ThemePicker and ThemeManager (list + paste-first editor).
//!
//! Ports `src/gui/view/modals/theme_picker.rs:17+`,
//! `src/gui/view/modals/theme_manager.rs:19,43` and
//! `src/gui/theme_manager_editor.rs`.
//!
//! **The live preview goes through the single stubbed hook** at
//! `crate::terminal_element::project_theme_override`'s `preview` argument
//! (carried decision 7) — there is no second theme-override path.

use gpui::{div, prelude::*, px, AnyElement, App, Context};
use grove_core::theme::{Theme, ThemeKind};

use crate::settings::SettingsState;
use crate::theme as c;

use super::shell::{
    body_text, click_action, click_checkbox, click_row, modal_body, modal_footer_hints,
    modal_header, modal_panel, note_text, ModalBtn,
};
use super::{Modal, ModalClick, ModalDispatch, ModalLayer};
use crate::modal::{ThemePickerReturn, ThemePickerScope};

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

    pub(super) fn theme_picker_move(&mut self, delta: i32, cx: &mut Context<Self>) {
        if let Some(Modal::ThemePicker {
            sel_dark,
            sel_light,
            dark_tab,
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
            // Picking a concrete theme clears "Default (follow app)".
            *project_use_default = false;
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
                if let Some(t) = &theme {
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
                    if follow_system {
                        store.theme_dark = n;
                    } else {
                        store.theme = n;
                    }
                });
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
        self.cancel(cx);
    }

    /// Escape: restore `original` before leaving (`src/app/modal.rs:74-94`).
    pub(super) fn theme_picker_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(Modal::ThemePicker { original, .. }) = self.slot.get() {
            let original = original.clone();
            crate::theme::ThemeState::set_by_name(cx, &original);
        }
        ThemePreview::clear(cx);
        self.cancel(cx);
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

pub fn render(layer: &ModalLayer, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    match layer.slot().get() {
        Some(Modal::ThemePicker { .. }) => picker(layer, dispatch, cx),
        Some(Modal::ThemeManager { .. }) => manager(layer, dispatch),
        _ => div().into_any_element(),
    }
}

fn swatch(t: &Theme) -> impl IntoElement {
    div().flex().items_center().gap(px(3.0)).children(
        [c::bg_of(t), c::fg_of(t), c::blue_of(t), c::green_of(t)].map(|col| {
            div()
                .size(px(9.0))
                .rounded(px(2.0))
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
    let offset = crate::launcher::scroll_offset_for(0, sel, 8, themes.len());

    let tabs = div()
        .flex()
        .gap(px(6.0))
        .child(click_action(
            "tp-dark",
            "Dark",
            if *dark_tab {
                ModalBtn::Primary
            } else {
                ModalBtn::Plain
            },
            dispatch,
            ModalClick::ThemePickerTab(true),
        ))
        .child(click_action(
            "tp-light",
            "Light",
            if *dark_tab {
                ModalBtn::Plain
            } else {
                ModalBtn::Primary
            },
            dispatch,
            ModalClick::ThemePickerTab(false),
        ));

    let mut list = div().flex().flex_col().gap(px(2.0));
    if matches!(scope, ThemePickerScope::Project(_)) {
        list = list.child(click_row(
            "tp-default",
            *project_use_default,
            dispatch,
            ModalClick::ThemePickerUseDefault,
            body_text("Default (follow app)"),
        ));
    }
    for (i, t) in themes.iter().enumerate().skip(offset).take(8) {
        list = list.child(click_row(
            gpui::SharedString::from(format!("tp-{i}")),
            i == sel && !*project_use_default,
            dispatch,
            ModalClick::SelectRow(i),
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .w_full()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.0))
                        .text_color(c::FG_DIM())
                        .child(t.name.to_string()),
                )
                .child(swatch(t)),
        ));
    }

    let title = match scope {
        ThemePickerScope::App => "App theme".to_string(),
        ThemePickerScope::Project(p) => format!("Project theme — {p}"),
    };

    modal_panel(
        480.0,
        div()
            .child(modal_header(title, c::MAGENTA()))
            .child(modal_body(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(tabs)
                    .child(list)
                    .child(click_checkbox(
                        "tp-follow",
                        "Follow system appearance",
                        *follow_system,
                        c::CYAN(),
                        matches!(scope, ThemePickerScope::App),
                        dispatch,
                        ModalClick::ThemePickerToggleFollowSystem,
                    )),
            ))
            .child(modal_footer_hints(&[
                ("↑↓ / jk", "browse"),
                ("tab / hl", "dark/light"),
                ("⏎", "apply"),
                ("esc", "restore"),
            ])),
    )
    .into_any_element()
}

fn manager(layer: &ModalLayer, dispatch: &ModalDispatch) -> AnyElement {
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
        let field = layer.fields.first().map(|f| {
            div()
                .w_full()
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .bg(c::BG())
                .border_1()
                .border_color(c::BORDER())
                .child(gpui_component::input::Input::new(f.state()).w_full())
        });
        return modal_panel(
            620.0,
            div()
                .child(modal_header("Theme editor", c::MAGENTA()))
                .child(modal_body(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(body_text(
                            "Paste a theme JSON object, or edit the one below.",
                        ))
                        .children(field)
                        .child(div().flex().gap(px(8.0)).child(click_action(
                            "tm-save",
                            "Save",
                            ModalBtn::Primary,
                            dispatch,
                            ModalClick::ThemeEditSave,
                        ))),
                ))
                // Tab INDENTS inside a multiline buffer (carried decision 2);
                // the footer says so rather than pretending it traverses.
                .child(modal_footer_hints(&[("tab", "indent"), ("esc", "back")])),
        )
        .into_any_element();
    }

    let themes = grove_core::theme::all_custom_themes();
    let mut list = div().flex().flex_col().gap(px(2.0));
    if themes.is_empty() {
        list = list.child(body_text("No custom themes yet."));
    }
    for (i, t) in themes.iter().enumerate() {
        let is_renaming = rename.as_ref().is_some_and(|(from, _)| *from == t.name);
        let is_pending = pending_delete.as_deref() == Some(t.name.as_ref());
        list = list.child(click_row(
            gpui::SharedString::from(format!("tm-{i}")),
            i == *selected,
            dispatch,
            ModalClick::ThemeSelect(i),
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .w_full()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.0))
                        .text_color(if is_pending { c::RED() } else { c::FG_DIM() })
                        .child(if is_renaming {
                            rename
                                .as_ref()
                                .map_or_else(|| t.name.to_string(), |(_, buf)| buf.clone())
                        } else {
                            t.name.to_string()
                        }),
                )
                .child(swatch(t))
                .child(click_action(
                    "tm-rename",
                    "rename",
                    ModalBtn::Plain,
                    dispatch,
                    ModalClick::ThemeRenameStart(i),
                ))
                .child(click_action(
                    "tm-dup",
                    "duplicate",
                    ModalBtn::Plain,
                    dispatch,
                    ModalClick::ThemeDuplicate(i),
                ))
                .child(click_action(
                    "tm-edit",
                    "edit",
                    ModalBtn::Plain,
                    dispatch,
                    ModalClick::ThemeEditOpen(i),
                ))
                .child(click_action(
                    "tm-del",
                    "delete",
                    ModalBtn::Danger,
                    dispatch,
                    ModalClick::ThemeDeleteRequest(i),
                )),
        ));
    }

    let mut body = div().flex().flex_col().gap(px(10.0)).child(list);
    if let Some(err) = rename_error {
        body = body.child(note_text(err.clone()));
    }
    if let Some(name) = pending_delete {
        body = body.child(note_text(format!("Delete '{name}'?  y / n")));
    }
    body = body.child(div().flex().gap(px(8.0)).child(click_action(
        "tm-new",
        "New theme",
        ModalBtn::Primary,
        dispatch,
        ModalClick::ThemeNew,
    )));

    modal_panel(
        620.0,
        div()
            .child(modal_header("Manage themes", c::MAGENTA()))
            .child(modal_body(body))
            .child(modal_footer_hints(&[("↑↓", "browse"), ("esc", "close")])),
    )
    .into_any_element()
}
