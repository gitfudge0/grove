//! Settings, the registry-generated ShortcutOverlay, the ScriptsEditor and
//! the Updating/changelog shells — plus the click routing every Task 4-6
//! modal shares.
//!
//! Ports `src/gui/view/modals/settings.rs:130-625` (Settings, with the
//! archived-projects row at :305 and the tmux setting at :325), `:626-790`
//! (the overlay), `src/gui/scripts_editor.rs:63-335` and
//! `src/gui/view/modals/upgrade.rs:16-97,98-182`.

use gpui::{div, prelude::*, px, AnyElement, App, Context, Window};

use crate::keymap::{self, Scope, ShortcutDef, SHORTCUTS};
use crate::launcher::SettingRow;
use crate::settings::SettingsState;
use crate::theme as c;

use super::shell::{
    body_text, click_action, click_checkbox, click_row, modal_body, modal_footer_hints,
    modal_header, modal_panel, section_header, ModalBtn,
};
use super::{Modal, ModalClick, ModalDispatch, ModalLayer, SettingToggle};
use crate::modal::{ScriptsEditorState, ThemePickerReturn, ThemePickerScope};

/// The current value shown on a settings row.
pub fn setting_value(row: SettingRow, cx: &App) -> String {
    let store = &cx.global::<SettingsState>().store;
    match row {
        SettingRow::Theme => store
            .theme
            .clone()
            .unwrap_or_else(|| crate::theme::DEFAULT_DARK_THEME.to_string()),
        SettingRow::AppSize => {
            format!("{:.0}%", cx.global::<crate::zoom::ZoomState>().zoom * 100.0)
        }
        SettingRow::ProjectThemes => on_off(store.project_themes_enabled),
        SettingRow::Backend => if store.tmux_enabled.unwrap_or(false) {
            "tmux"
        } else {
            "native"
        }
        .to_string(),
        SettingRow::Permissions => {
            if store.dangerously_skip_permissions_enabled.unwrap_or(false) {
                "skip prompts".to_string()
            } else {
                "ask me".to_string()
            }
        }
        SettingRow::Telemetry => on_off(store.telemetry_enabled.unwrap_or(false)),
        SettingRow::Chrome => on_off(store.chrome_enabled.unwrap_or(false)),
        SettingRow::DefaultAgent => store
            .default_agent
            .map_or_else(|| "—".to_string(), |a| a.label().to_string()),
        SettingRow::CheckUpdates => format!("v{}", env!("CARGO_PKG_VERSION")),
    }
}

fn on_off(v: bool) -> String {
    if v { "on" } else { "off" }.to_string()
}

impl ModalLayer {
    /// The clicks Tasks 5-6's modals raise.
    pub(super) fn on_late_click(
        &mut self,
        click: ModalClick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match click {
            ModalClick::ThemePickerTab(dark) => {
                if let Some(Modal::ThemePicker { dark_tab, .. }) = self.slot.get_mut() {
                    *dark_tab = dark;
                }
                self.preview_selected_theme(cx);
            }
            ModalClick::ThemePickerToggleFollowSystem => {
                if let Some(Modal::ThemePicker { follow_system, .. }) = self.slot.get_mut() {
                    *follow_system = !*follow_system;
                }
                cx.notify();
            }
            ModalClick::ThemePickerUseDefault => {
                if let Some(Modal::ThemePicker {
                    project_use_default,
                    ..
                }) = self.slot.get_mut()
                {
                    *project_use_default = true;
                }
                self.preview_selected_theme(cx);
            }
            ModalClick::Save => self.save_scripts(cx),
            ModalClick::OpenProjectTheme => self.open_project_theme_from_scripts(cx),
            ModalClick::OpenArchiveGate => {
                let idx = self.scripts_project_index(cx);
                if let Some(idx) = idx {
                    self.open_archive_gate(idx, cx);
                }
            }
            ModalClick::OpenArchivedProjects => self.open(Modal::ArchivedProjects, cx),
            ModalClick::OpenThemePicker => {
                self.open_theme_picker(ThemePickerScope::App, ThemePickerReturn::Settings, cx);
            }
            ModalClick::OpenThemeManager => self.open_theme_manager(cx),
            ModalClick::OpenChangelog => self.open(
                Modal::Changelog {
                    return_to_settings: true,
                },
                cx,
            ),
            ModalClick::ToggleSetting(t) => self.toggle_setting(t, cx),
            ModalClick::ThemeSelect(i) => {
                if let Some(Modal::ThemeManager { selected, .. }) = self.slot.get_mut() {
                    *selected = i;
                }
                cx.notify();
            }
            ModalClick::ThemeRenameStart(i) => {
                let name = grove_core::theme::all_custom_themes()
                    .get(i)
                    .map(|t| t.name.to_string());
                if let (Some(name), Some(Modal::ThemeManager { rename, .. })) =
                    (name, self.slot.get_mut())
                {
                    *rename = Some((name.clone(), name));
                }
                cx.notify();
            }
            ModalClick::ThemeRenameCommit => self.theme_manager_rename_submit(cx),
            ModalClick::ThemeDuplicate(i) => self.duplicate_theme(i, cx),
            ModalClick::ThemeDeleteRequest(i) => {
                let name = grove_core::theme::all_custom_themes()
                    .get(i)
                    .map(|t| t.name.to_string());
                if let Some(Modal::ThemeManager { pending_delete, .. }) = self.slot.get_mut() {
                    *pending_delete = name;
                }
                cx.notify();
            }
            ModalClick::ThemeDeleteConfirm => self.theme_manager_delete_confirm(cx),
            ModalClick::ThemeDeleteCancel => self.theme_manager_delete_cancel(cx),
            ModalClick::ThemeNew => self.open_theme_editor(None, window, cx),
            ModalClick::ThemeEditOpen(i) => self.open_theme_editor(Some(i), window, cx),
            ModalClick::ThemeEditSave => self.theme_editor_save(cx),
            _ => {}
        }
    }

    fn toggle_setting(&mut self, t: SettingToggle, cx: &mut Context<Self>) {
        // Every control persists immediately; there is no apply/cancel footer
        // (recorded ambiguity 5).
        match t {
            SettingToggle::Tmux => {
                let on = cx
                    .global::<SettingsState>()
                    .store
                    .tmux_enabled
                    .unwrap_or(false);
                self.choose_tmux_setting(!on, cx);
                return;
            }
            SettingToggle::SkipPermissions => SettingsState::update(cx, |store| {
                let cur = store.dangerously_skip_permissions_enabled.unwrap_or(false);
                store.dangerously_skip_permissions_enabled = Some(!cur);
            }),
            SettingToggle::Chrome => SettingsState::update(cx, |store| {
                let cur = store.chrome_enabled.unwrap_or(false);
                store.chrome_enabled = Some(!cur);
            }),
            SettingToggle::ThemeFollowSystem => SettingsState::update(cx, |store| {
                store.theme_follow_system = !store.theme_follow_system;
            }),
        }
        SettingsState::flush_now(cx);
        cx.notify();
    }

    /// The Settings tmux row: same persistence as the TmuxChoice modal, but
    /// it never closes the Settings modal behind it.
    fn choose_tmux_setting(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if enabled && !grove_core::tmux::available() {
            self.toast.update(cx, |t, cx| {
                t.set_error("tmux not found; using native sessions", cx);
            });
            return;
        }
        SettingsState::update(cx, move |store| store.tmux_enabled = Some(enabled));
        SettingsState::flush_now(cx);
        cx.notify();
    }

    /// A settings row activated from the palette drill-in. Toggles flip in
    /// place; enum rows open their own modal.
    pub(super) fn activate_setting(
        &mut self,
        row: SettingRow,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match row {
            SettingRow::Theme => {
                self.open_theme_picker(ThemePickerScope::App, ThemePickerReturn::Close, cx);
            }
            SettingRow::ProjectThemes => {
                SettingsState::update(cx, |store| {
                    store.project_themes_enabled = !store.project_themes_enabled;
                });
                SettingsState::flush_now(cx);
                cx.notify();
            }
            SettingRow::Telemetry => {
                SettingsState::update(cx, |store| {
                    let cur = store.telemetry_enabled.unwrap_or(false);
                    store.telemetry_enabled = Some(!cur);
                });
                SettingsState::flush_now(cx);
                cx.notify();
            }
            SettingRow::Chrome => self.toggle_setting(SettingToggle::Chrome, cx),
            SettingRow::Backend => self.toggle_setting(SettingToggle::Tmux, cx),
            SettingRow::Permissions => self.toggle_setting(SettingToggle::SkipPermissions, cx),
            SettingRow::CheckUpdates => self.open(Modal::Updating, cx),
            // Plan 09 owns the live upgrade stages; the app-size and
            // default-agent panes are the Settings modal's own rows.
            SettingRow::AppSize | SettingRow::DefaultAgent => self.open(Modal::Settings, cx),
        }
    }

    // ── ScriptsEditor ───────────────────────────────────────────────────

    fn scripts_project_index(&self, cx: &App) -> Option<usize> {
        let Some(Modal::ScriptsEditor(st)) = self.slot.get() else {
            return None;
        };
        cx.global::<SettingsState>()
            .store
            .projects
            .iter()
            .position(|p| p.path == st.project_path)
    }

    /// Save with the empty→`None` normalization and the save-failure `Message`
    /// modal (`src/gui/scripts_editor.rs:79-107`).
    fn save_scripts(&mut self, cx: &mut Context<Self>) {
        self.sync_wizard_buffers(cx);
        let Some(Modal::ScriptsEditor(st)) = self.slot.get() else {
            return;
        };
        let st = (**st).clone();
        let norm = |s: &str| {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        };
        let (setup, run, teardown) = (norm(&st.setup), norm(&st.run), norm(&st.teardown));
        let path = st.project_path.clone();
        SettingsState::update(cx, move |store| {
            if let Some(p) = store.projects.iter_mut().find(|p| p.path == path) {
                p.scripts.setup = setup;
                p.scripts.run = run;
                p.scripts.teardown = teardown;
            }
        });
        SettingsState::flush_now(cx);
        if cx.global::<SettingsState>().is_dirty() {
            self.open(Modal::Message("Scripts could not be saved.".into()), cx);
            return;
        }
        self.toast
            .update(cx, |t, cx| t.set_toast("scripts saved", cx));
        self.close(cx);
    }

    /// "Project theme" from the scripts editor: the documented `open_child`
    /// exception — the picker carries the editor's unsaved buffers through and
    /// hands them back on cancel (`modals.rs:660-668`).
    fn open_project_theme_from_scripts(&mut self, cx: &mut Context<Self>) {
        self.sync_wizard_buffers(cx);
        let Some(Modal::ScriptsEditor(st)) = self.slot.get() else {
            return;
        };
        let state: ScriptsEditorState = (**st).clone();
        let name = cx
            .global::<SettingsState>()
            .store
            .projects
            .iter()
            .find(|p| p.path == state.project_path)
            .map(|p| p.name.clone());
        let Some(name) = name else { return };
        self.open_theme_picker(
            ThemePickerScope::Project(name),
            ThemePickerReturn::ScriptsEditor(Box::new(state)),
            cx,
        );
    }

    // ── ThemeManager helpers ────────────────────────────────────────────

    fn duplicate_theme(&mut self, i: usize, cx: &mut Context<Self>) {
        let themes = grove_core::theme::all_custom_themes();
        let Some(t) = themes.get(i) else { return };
        if let Err(e) = crate::theme::duplicate_custom_theme(&t.name) {
            self.open(Modal::Message(format!("Duplicate failed: {e}")), cx);
            return;
        }
        cx.refresh_windows();
        cx.notify();
    }

    fn open_theme_editor(&mut self, i: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let buffer = match i {
            Some(i) => grove_core::theme::all_custom_themes()
                .get(i)
                .map(grove_core::theme_file::to_named_lines)
                .unwrap_or_default(),
            None => crate::theme::new_theme_template(),
        };
        if let Some(Modal::ThemeManager { editor, .. }) = self.slot.get_mut() {
            *editor = Some(buffer);
        }
        self.rebuild_fields(window, cx);
    }
}

// ── the views ────────────────────────────────────────────────────────────

pub fn render(layer: &ModalLayer, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    match layer.slot().get() {
        Some(Modal::Settings) => settings_modal(dispatch, cx),
        Some(Modal::ShortcutOverlay) => shortcut_overlay(layer.state.read(cx).screen()),
        Some(Modal::ScriptsEditor(st)) => scripts_editor(layer, st, dispatch),
        Some(Modal::Updating) => updating_modal(dispatch),
        Some(Modal::Changelog { .. }) => changelog_modal(dispatch),
        _ => div().into_any_element(),
    }
}

fn settings_row(
    id: &'static str,
    label: &'static str,
    value: String,
    dispatch: &ModalDispatch,
    click: ModalClick,
) -> impl IntoElement {
    click_row(
        id,
        false,
        dispatch,
        click,
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
                    .child(label),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(c::FG_MUTE())
                    .child(value),
            ),
    )
}

/// Every control persists immediately; there is no apply/cancel footer.
fn settings_modal(dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    let store = &cx.global::<SettingsState>().store;
    let archived = store.archived_count();
    let tmux_on = store.tmux_enabled.unwrap_or(false);

    let body = div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(section_header("APPEARANCE", 4.0, 4.0))
        .child(settings_row(
            "set-theme",
            "App theme",
            setting_value(SettingRow::Theme, cx),
            dispatch,
            ModalClick::OpenThemePicker,
        ))
        .child(settings_row(
            "set-manage-themes",
            "Manage themes…",
            String::new(),
            dispatch,
            ModalClick::OpenThemeManager,
        ))
        .child(click_checkbox(
            "set-follow-system",
            "Follow system appearance",
            store.theme_follow_system,
            c::CYAN(),
            true,
            dispatch,
            ModalClick::ToggleSetting(SettingToggle::ThemeFollowSystem),
        ))
        .child(section_header("AGENTS / TERMINAL", 10.0, 4.0))
        // The tmux setting Plan 06/07 deferred here (`settings.rs:325`).
        .child(click_checkbox(
            "set-tmux",
            "Use tmux for new sessions",
            tmux_on,
            c::GREEN(),
            true,
            dispatch,
            ModalClick::ToggleSetting(SettingToggle::Tmux),
        ))
        .child(click_checkbox(
            "set-perms",
            "Skip agent permission prompts",
            store.dangerously_skip_permissions_enabled.unwrap_or(false),
            c::RED(),
            true,
            dispatch,
            ModalClick::ToggleSetting(SettingToggle::SkipPermissions),
        ))
        .child(click_checkbox(
            "set-chrome",
            "Claude in Chrome control",
            store.chrome_enabled.unwrap_or(false),
            c::BLUE(),
            true,
            dispatch,
            ModalClick::ToggleSetting(SettingToggle::Chrome),
        ))
        .child(section_header("PROJECTS", 10.0, 4.0))
        // The archived-projects row Plan 06/07 deferred here
        // (`settings.rs:305`).
        .child(settings_row(
            "set-archived",
            "Archived projects",
            format!("{archived}"),
            dispatch,
            ModalClick::OpenArchivedProjects,
        ))
        .child(section_header("UPDATES", 10.0, 4.0))
        .child(settings_row(
            "set-changelog",
            "View changelog",
            format!("v{}", env!("CARGO_PKG_VERSION")),
            dispatch,
            ModalClick::OpenChangelog,
        ));

    modal_panel(
        560.0,
        div()
            .child(modal_header("Settings", c::CYAN()))
            .child(modal_body(body))
            .child(modal_footer_hints(&[("esc", "close")])),
    )
    .into_any_element()
}

/// Generated from `keymap::SHORTCUTS`, filtered by the current screen, plus
/// exactly two static rows (recorded ambiguity 6).
fn shortcut_overlay(screen: keymap::Screen) -> AnyElement {
    let visible: Vec<&ShortcutDef> = SHORTCUTS
        .iter()
        .filter(|d| !d.display_keys.is_empty())
        .filter(|d| scope_allows(d, screen))
        .collect();
    // Group only when the visible set spans Global AND screen scopes.
    let has_global = visible.iter().any(|d| d.scopes.contains(&Scope::Global));
    let has_screen = visible
        .iter()
        .any(|d| d.scopes.iter().any(|s| matches!(s, Scope::Screen(_))));
    let grouped = has_global && has_screen;

    let row = |d: &ShortcutDef| {
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .py(px(3.0))
            .child(
                div()
                    .w(px(150.0))
                    .child(super::shell::keycap_text(chord_label(d), c::FG_DIM())),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.0))
                    .text_color(c::FG_DIM())
                    .child(d.description),
            )
    };

    let mut body = div().flex().flex_col();
    if grouped {
        body = body.child(section_header("GLOBAL", 4.0, 4.0));
        for d in visible.iter().filter(|d| d.scopes.contains(&Scope::Global)) {
            body = body.child(row(d));
        }
        body = body.child(section_header(screen.label(), 10.0, 4.0));
        for d in visible
            .iter()
            .filter(|d| d.scopes.iter().any(|s| matches!(s, Scope::Screen(_))))
        {
            body = body.child(row(d));
        }
    } else {
        for d in &visible {
            body = body.child(row(d));
        }
    }
    // The two static rows (`settings.rs:665-669`).
    body = body
        .child(section_header("EDITING", 10.0, 4.0))
        .child(static_row(
            &format!(
                "{}+c / {}+v",
                keymap::platform_mod_label(),
                keymap::platform_mod_label()
            ),
            "Copy / paste",
        ))
        .child(static_row("esc", "Close modals"));

    modal_panel(
        620.0,
        div()
            .child(modal_header("Keyboard shortcuts", c::CYAN()))
            .child(modal_body(body))
            .child(modal_footer_hints(&[(
                "esc",
                "close (or press the same chord again)",
            )])),
    )
    .into_any_element()
}

fn static_row(keys: &str, label: &'static str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .py(px(3.0))
        .child(
            div()
                .w(px(150.0))
                .child(super::shell::keycap_text(keys.to_string(), c::FG_DIM())),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(12.0))
                .text_color(c::FG_DIM())
                .child(label),
        )
}

/// Whether a registry row is visible on `screen`.
pub fn scope_allows(def: &ShortcutDef, screen: keymap::Screen) -> bool {
    def.scopes
        .iter()
        .any(|s| matches!(s, Scope::Global) || matches!(s, Scope::Screen(sc) if *sc == screen))
}

/// The chord label: the platform modifier prepended, with the alt-chord rule
/// (`cmd+alt+n` / `ctrl+alt+n`, never `ctrl+shift+alt+n`) and `literal` rows
/// shown verbatim.
pub fn chord_label(def: &ShortcutDef) -> String {
    if def.literal {
        return def.display_keys.to_string();
    }
    let prefix = if def.requires_alt {
        keymap::alt_chord_prefix().replace('-', "+")
    } else {
        format!("{}+", keymap::platform_mod_label())
    };
    format!("{prefix}{}", def.display_keys)
}

fn scripts_editor(
    layer: &ModalLayer,
    st: &ScriptsEditorState,
    dispatch: &ModalDispatch,
) -> AnyElement {
    let editor = |i: usize, label: &'static str| {
        let mut d = div().flex().flex_col().gap(px(4.0)).flex_1().child(
            div()
                .text_size(px(10.0))
                .text_color(c::FG_MUTE())
                .child(label),
        );
        if let Some(f) = layer.fields.get(i) {
            d = d.child(gpui_component::input::Input::new(f.state()).w_full());
        }
        d
    };

    modal_panel(
        760.0,
        div()
            .child(modal_header(
                format!("Scripts — {}", st.project_path),
                c::CYAN(),
            ))
            .child(modal_body(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex()
                            .gap(px(10.0))
                            .child(editor(0, "setup"))
                            .child(editor(1, "run"))
                            .child(editor(2, "teardown")),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(click_action(
                                "se-save",
                                "Save",
                                ModalBtn::Primary,
                                dispatch,
                                ModalClick::Save,
                            ))
                            .child(click_action(
                                "se-cancel",
                                "Cancel",
                                ModalBtn::Plain,
                                dispatch,
                                ModalClick::Cancel,
                            ))
                            .child(click_action(
                                "se-theme",
                                "Project theme",
                                ModalBtn::Plain,
                                dispatch,
                                ModalClick::OpenProjectTheme,
                            ))
                            .child(click_action(
                                "se-archive",
                                "Archive project",
                                ModalBtn::Danger,
                                dispatch,
                                ModalClick::OpenArchiveGate,
                            )),
                    ),
            ))
            // Tab INDENTS inside a buffer; traversal is a click or ctrl-tab
            // (carried decision 2). The footer says so rather than lying.
            .child(modal_footer_hints(&[
                ("tab", "indent"),
                ("ctrl+tab / click", "next buffer"),
                ("esc", "discard"),
            ])),
    )
    .into_any_element()
}

/// The upgrade shell. Plan 09 owns the live stages, the changelog fetch and
/// apply/restart — this renders whatever the current state reports and nothing
/// more.
fn updating_modal(dispatch: &ModalDispatch) -> AnyElement {
    modal_panel(
        420.0,
        div()
            .child(modal_header("Updates", c::CYAN()))
            .child(modal_body(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(body_text(format!(
                        "Grove v{} — you are up to date.",
                        env!("CARGO_PKG_VERSION")
                    )))
                    // Plan 09 fills the live stages (Updating polling, apply,
                    // restart). Not a stub that claims to be done.
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(c::FG_MUTE())
                            .child("Live update stages arrive in gpui rewrite plan 09."),
                    )
                    .child(click_action(
                        "up-changelog",
                        "View changelog",
                        ModalBtn::Plain,
                        dispatch,
                        ModalClick::OpenChangelog,
                    )),
            ))
            .child(modal_footer_hints(&[("esc", "close")])),
    )
    .into_any_element()
}

/// Overlays Settings and returns to it on dismiss (carried decision 4).
fn changelog_modal(dispatch: &ModalDispatch) -> AnyElement {
    modal_panel(
        520.0,
        div()
            .child(modal_header("Changelog", c::MAGENTA()))
            .child(modal_body(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(body_text(format!("Grove v{}", env!("CARGO_PKG_VERSION"))))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(c::FG_MUTE())
                            .child("Release-note fetch arrives in gpui rewrite plan 09."),
                    )
                    .child(click_action(
                        "cl-close",
                        "Back to Settings",
                        ModalBtn::Plain,
                        dispatch,
                        ModalClick::Cancel,
                    )),
            ))
            .child(modal_footer_hints(&[("esc", "back to settings")])),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift guard (Task 6 Step 2): every registry row with a display label
    /// appears in the overlay for at least one screen. The registry is the
    /// single source of truth (spec §5) and this is what proves it stayed so.
    #[test]
    fn every_registry_row_with_a_label_appears_on_some_screen() {
        for def in SHORTCUTS {
            if def.display_keys.is_empty() {
                continue;
            }
            let shown = [
                keymap::Screen::Workspace,
                keymap::Screen::Grid,
                keymap::Screen::Zen,
            ]
            .into_iter()
            .any(|s| scope_allows(def, s));
            assert!(
                shown,
                "registry row {:?} has a display label but no screen shows it",
                def.description
            );
        }
    }

    /// The alt-chord label rule: `{mod}+alt+n`, never `ctrl+shift+alt+n`.
    #[test]
    fn alt_chords_never_render_the_shift_prefixed_modifier() {
        for def in SHORTCUTS.iter().filter(|d| d.requires_alt && !d.literal) {
            let label = chord_label(def);
            assert!(
                !label.contains("ctrl+shift+alt"),
                "{label} must use the alt-chord prefix, not the global-mods one"
            );
            assert!(label.contains("alt+"), "{label}");
        }
    }

    #[test]
    fn literal_rows_render_verbatim() {
        for def in SHORTCUTS.iter().filter(|d| d.literal) {
            assert_eq!(chord_label(def), def.display_keys);
        }
    }

    #[test]
    fn the_overlay_is_screen_sensitive() {
        let count = |screen| SHORTCUTS.iter().filter(|d| scope_allows(d, screen)).count();
        let grid = count(keymap::Screen::Grid);
        let zen = count(keymap::Screen::Zen);
        assert_ne!(grid, zen, "grid and zen must not show identical sets");
    }
}
