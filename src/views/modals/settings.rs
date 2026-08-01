//! Settings, the registry-generated ShortcutOverlay, the ScriptsEditor and
//! the Updating/changelog shells — plus the click routing every Task 4-6
//! modal shares.
//!
//! Ports `src/gui/view/modals/settings.rs:130-625` (Settings, with the
//! archived-projects row at :305 and the tmux setting at :325), `:626-790`
//! (the overlay), `src/gui/scripts_editor.rs:63-335` and
//! `src/gui/view/modals/upgrade.rs:16-97,98-182`.

use crate::views::rpx;
use gpui::{div, prelude::*, AnyElement, App, Context, Window};

use grove_core::agent::Agent;

use crate::entities::upgrade::Upgrade;
use crate::entities::upgrade_state::{ChangelogState, UpgradeState};
use crate::keymap::{self, Scope, ShortcutDef, SHORTCUTS};
use crate::launcher::SettingRow;
use crate::settings::SettingsState;
use crate::theme as c;

use super::shell::{
    body_text, caption, caption_promoted, click_action, click_checkbox, click_row, divider_h,
    flat_icon_btn, flat_text_btn, modal_body, modal_checkbox, modal_footer_hints, modal_footer_row,
    modal_header, modal_header_row, modal_panel, section_header, seg_button, seg_group, ModalBtn,
    OnToggle, SegSide,
};
use super::{Modal, ModalClick, ModalDispatch, ModalLayer, SettingToggle};
use crate::modal::{ScriptsEditorState, ThemePickerReturn, ThemePickerScope};
use crate::views::workspace::Workspace;
use crate::zoom::{ZoomState, ZOOM_DEFAULT, ZOOM_STEP};

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
        SettingRow::Telemetry => on_off(SettingsState::telemetry_enabled(store)),
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
            ModalClick::OpenChangelog => {
                // The changelog closes Settings on the way in and reopens it on
                // the way out; the round trip is already a passing state-machine
                // test, so this only supplies the data
                // (`src/gui/update/upgrade.rs:127-149`).
                self.upgrade.update(cx, Upgrade::fetch_changelog);
                self.open(
                    Modal::Changelog {
                        return_to_settings: true,
                    },
                    cx,
                );
            }
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
        if enabled {
            // The re-scan the workspace owns (`src/app/mod.rs:288-292`).
            cx.emit(super::ModalEvent::TmuxEnabled);
        }
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
                let enabled =
                    !SettingsState::telemetry_enabled(&cx.global::<SettingsState>().store);
                SettingsState::update(cx, |store| store.telemetry_enabled = Some(enabled));
                SettingsState::flush_now(cx);
                // Takes effect immediately, not at the next launch
                // (`src/app/mod.rs:339-344`).
                crate::telemetry::set_enabled(enabled);
                cx.notify();
            }
            SettingRow::Chrome => self.toggle_setting(SettingToggle::Chrome, cx),
            SettingRow::Backend => self.toggle_setting(SettingToggle::Tmux, cx),
            SettingRow::Permissions => self.toggle_setting(SettingToggle::SkipPermissions, cx),
            // A **manual** check, not a modal that claims to be one
            // (`src/gui/update/upgrade.rs:26-32`).
            SettingRow::CheckUpdates => self.upgrade.update(cx, |u, cx| u.check(true, cx)),
            // The app-size and default-agent panes are the Settings modal's
            // own rows.
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

/// One row of the Settings → Tools section (`src/gui/state.rs`'s `ToolStatus`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolStatus {
    pub agent: Agent,
    pub installed: bool,
    pub version: Option<String>,
    /// Drives the "Detecting…" placeholder while the off-thread scan runs.
    pub detecting: bool,
}

/// The tools shown, in display order. `Terminal` is omitted — always
/// available, no version (`src/gui/update/upgrade.rs:154-157`).
pub const SETTINGS_TOOLS: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::OpenCode];

/// The status text and whether it reads as a live value (a version) or as
/// muted status. Pure, so the three states are testable without a subprocess
/// (`src/gui/view/modals/settings.rs:441-452`).
#[must_use]
pub fn tool_status_text(st: &ToolStatus) -> (String, bool) {
    if st.detecting {
        ("Detecting…".to_string(), false)
    } else if !st.installed {
        ("Not installed".to_string(), false)
    } else {
        (
            st.version
                .clone()
                .unwrap_or_else(|| "installed".to_string()),
            true,
        )
    }
}

impl ModalLayer {
    /// Mark every Tools row as detecting and dispatch the off-thread
    /// availability + version scan (`detect_tools_task`,
    /// `src/gui/update/upgrade.rs:158-191`). `--version` is a short
    /// subprocess, but running three of them on the UI thread is still three
    /// too many.
    pub(super) fn detect_tools(&mut self, cx: &mut Context<Self>) {
        self.tools = SETTINGS_TOOLS
            .iter()
            .map(|&agent| ToolStatus {
                agent,
                installed: false,
                version: None,
                detecting: true,
            })
            .collect();
        cx.notify();
        let scan = cx.background_spawn(async {
            SETTINGS_TOOLS
                .iter()
                .map(|&agent| {
                    let installed = agent.available();
                    let version = if installed { agent.version() } else { None };
                    ToolStatus {
                        agent,
                        installed,
                        version,
                        detecting: false,
                    }
                })
                .collect::<Vec<_>>()
        });
        self.tools_task = Some(cx.spawn(async move |this, cx| {
            let tools = scan.await;
            let _ = this.update(cx, |this, cx| {
                this.tools = tools;
                cx.notify();
            });
        }));
    }

    /// Copy the offered release's URL and raise the oracle's toast
    /// (`src/gui/update/upgrade.rs:77-82`).
    pub(super) fn copy_release_url(&mut self, cx: &mut Context<Self>) {
        let Some(url) = self
            .upgrade
            .read(cx)
            .available()
            .map(|r| r.html_url.clone())
        else {
            return;
        };
        crate::terminal::clipboard::copy(&url);
        self.toast
            .update(cx, |t, cx| t.set_toast("release url copied", cx));
    }
}

pub fn render(layer: &ModalLayer, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    match layer.slot().get() {
        Some(Modal::Settings) => settings_modal(layer, dispatch, cx),
        Some(Modal::ShortcutOverlay) => shortcut_overlay(layer.state.read(cx).screen()),
        Some(Modal::ScriptsEditor(st)) => scripts_editor(layer, st, dispatch, cx),
        Some(Modal::Updating) => updating_modal(layer, dispatch, cx),
        Some(Modal::Changelog { .. }) => changelog_modal(layer, dispatch, cx),
        _ => div().into_any_element(),
    }
}

/// A drill-in settings row: label, value, and a trailing chevron when
/// `chevron` is set (App theme / Archived projects — the rows that open
/// another modal; `src/gui/view/modals/settings.rs:230-243,303-316`).
fn settings_row(
    id: &'static str,
    label: &'static str,
    value: String,
    chevron: bool,
    dispatch: &ModalDispatch,
    click: ModalClick,
) -> impl IntoElement {
    let mut row = div()
        .flex()
        .items_center()
        .gap(rpx(8.0))
        .w_full()
        .child(
            div()
                .flex_1()
                .text_size(rpx(12.0))
                .text_color(c::FG())
                .child(label),
        )
        .child(
            div()
                .text_size(rpx(12.0))
                .text_color(c::FG_DIM())
                .child(value),
        );
    if chevron {
        row = row.child(crate::icons::icon("chev-right", 12.0, c::FG_MUTE()));
    }
    click_row(id, false, dispatch, click, row)
}

/// A checkbox row wired to a raw persist-and-repaint closure rather than a
/// [`ModalClick`] — for the two boolean rows (`ProjectThemes`, `Telemetry`)
/// whose toggle path lives in [`ModalLayer::activate_setting`] but has no
/// [`SettingToggle`] variant of its own. `cx.refresh_windows()` stands in for
/// the entity's own `cx.notify()`, which this closure's `&mut App` has no
/// access to.
fn raw_checkbox(
    id: &'static str,
    label: &'static str,
    checked: bool,
    accent: gpui::Hsla,
    on_toggle: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let handler: OnToggle = Box::new(on_toggle);
    modal_checkbox(id, label, checked, accent, Some(handler))
}

/// Every control persists immediately; there is no apply/cancel footer.
fn settings_modal(layer: &ModalLayer, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    let store = &cx.global::<SettingsState>().store;
    let archived = store.archived_count();
    let tmux_on = store.tmux_enabled.unwrap_or(false);
    let skip_perms_on = store.dangerously_skip_permissions_enabled.unwrap_or(false);

    // ── header ───────────────────────────────────────────────────────────
    let header = modal_header_row(
        div()
            .flex()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .text_size(rpx(13.0))
                    .text_color(c::CYAN())
                    .child("Settings"),
            )
            .child(
                div()
                    .text_size(rpx(11.0))
                    .text_color(c::FG_MUTE())
                    .child("Changes save automatically."),
            )
            .child(div().w(rpx(10.0)))
            .child(flat_icon_btn("set-close", "close", 28.0, 15.0, {
                let dispatch = std::rc::Rc::clone(dispatch);
                move |window, cx| dispatch(ModalClick::Cancel, window, cx)
            })),
    );

    // ── appearance ───────────────────────────────────────────────────────
    let zoom_pct = format!("{:.0}%", cx.global::<ZoomState>().zoom * 100.0);
    let app_size_row = div()
        .flex()
        .items_center()
        .h(rpx(28.0))
        .px(rpx(10.0))
        .child(
            div()
                .flex_1()
                .text_size(rpx(12.0))
                .text_color(c::FG())
                .child("App size"),
        )
        .child(seg_group(
            div()
                .flex()
                .items_center()
                .child(flat_icon_btn(
                    "set-zoom-out",
                    "minus",
                    20.0,
                    13.0,
                    |_, cx| {
                        Workspace::set_zoom(cx.global::<ZoomState>().zoom - ZOOM_STEP, cx);
                    },
                ))
                .child(flat_text_btn(
                    "set-zoom-reset",
                    zoom_pct,
                    12.0,
                    2.0,
                    |_, cx| {
                        Workspace::set_zoom(ZOOM_DEFAULT, cx);
                    },
                ))
                .child(flat_icon_btn("set-zoom-in", "plus", 20.0, 13.0, |_, cx| {
                    Workspace::set_zoom(cx.global::<ZoomState>().zoom + ZOOM_STEP, cx);
                })),
        ));

    let appearance = div()
        .flex()
        .flex_col()
        .gap(rpx(4.0))
        .child(section_header("APPEARANCE", 4.0, 4.0))
        .child(settings_row(
            "set-theme",
            "App theme",
            setting_value(SettingRow::Theme, cx),
            true,
            dispatch,
            ModalClick::OpenThemePicker,
        ))
        // Not in the iced original (`settings_iced.rs`) — the gpui-only entry
        // point into `ThemeManager` (custom-theme CRUD), kept here since
        // nothing else in Settings opens it.
        .child(settings_row(
            "set-manage-themes",
            "Manage themes…",
            String::new(),
            true,
            dispatch,
            ModalClick::OpenThemeManager,
        ))
        .child(app_size_row)
        .child(div().px(rpx(10.0)).child(raw_checkbox(
            "set-project-themes",
            "Project themes",
            store.project_themes_enabled,
            c::MAGENTA(),
            |_, cx| {
                SettingsState::update(cx, |store| {
                    store.project_themes_enabled = !store.project_themes_enabled;
                });
                SettingsState::flush_now(cx);
                cx.refresh_windows();
            },
        )))
        .child(caption("Let each project pin its PTYs to a specific theme"))
        .child(div().px(rpx(10.0)).child(click_checkbox(
            "set-follow-system",
            "Follow system appearance",
            store.theme_follow_system,
            c::CYAN(),
            true,
            dispatch,
            ModalClick::ToggleSetting(SettingToggle::ThemeFollowSystem),
        )));

    // ── projects ─────────────────────────────────────────────────────────
    // Shown with "0" rather than hidden when nothing is archived (see the
    // iced original's comment at `settings.rs:299-302`): a row that only
    // appears after the first archive makes the feature undiscoverable at
    // exactly the moment the user needs to know where their project went.
    let projects = div()
        .flex()
        .flex_col()
        .gap(rpx(4.0))
        .child(section_header("PROJECTS", 10.0, 4.0))
        .child(settings_row(
            "set-archived",
            "Archived projects",
            format!("{archived}"),
            true,
            dispatch,
            ModalClick::OpenArchivedProjects,
        ));

    // ── agents / terminal ────────────────────────────────────────────────
    let backend_row = div()
        .flex()
        .items_center()
        .h(rpx(28.0))
        .px(rpx(10.0))
        .child(
            div()
                .flex_1()
                .text_size(rpx(12.0))
                .text_color(c::FG())
                .child("Backend"),
        )
        .child(seg_group(
            div()
                .flex()
                .items_center()
                .child(seg_button(
                    "set-backend-native",
                    "Native",
                    !tmux_on,
                    SegSide::Left,
                    false,
                    tmux_on.then(|| -> OnToggle {
                        let dispatch = std::rc::Rc::clone(dispatch);
                        Box::new(move |window, cx| {
                            dispatch(ModalClick::ToggleSetting(SettingToggle::Tmux), window, cx);
                        })
                    }),
                ))
                .child(seg_button(
                    "set-backend-tmux",
                    "Tmux",
                    tmux_on,
                    SegSide::Right,
                    false,
                    (!tmux_on).then(|| -> OnToggle {
                        let dispatch = std::rc::Rc::clone(dispatch);
                        Box::new(move |window, cx| {
                            dispatch(ModalClick::ToggleSetting(SettingToggle::Tmux), window, cx);
                        })
                    }),
                )),
        ));

    let perms_row = div()
        .flex()
        .items_center()
        .h(rpx(28.0))
        .px(rpx(10.0))
        .child(
            div()
                .flex_1()
                .text_size(rpx(12.0))
                .text_color(c::FG())
                .child("Permissions"),
        )
        .child(seg_group(
            div()
                .flex()
                .items_center()
                .child(seg_button(
                    "set-perms-skip",
                    "Skip",
                    skip_perms_on,
                    SegSide::Left,
                    true,
                    (!skip_perms_on).then(|| -> OnToggle {
                        let dispatch = std::rc::Rc::clone(dispatch);
                        Box::new(move |window, cx| {
                            dispatch(
                                ModalClick::ToggleSetting(SettingToggle::SkipPermissions),
                                window,
                                cx,
                            );
                        })
                    }),
                ))
                .child(seg_button(
                    "set-perms-safe",
                    "Safe",
                    !skip_perms_on,
                    SegSide::Right,
                    false,
                    skip_perms_on.then(|| -> OnToggle {
                        let dispatch = std::rc::Rc::clone(dispatch);
                        Box::new(move |window, cx| {
                            dispatch(
                                ModalClick::ToggleSetting(SettingToggle::SkipPermissions),
                                window,
                                cx,
                            );
                        })
                    }),
                )),
        ));

    let agents_terminal = div()
        .flex()
        .flex_col()
        .gap(rpx(4.0))
        .child(section_header("AGENTS / TERMINAL", 10.0, 4.0))
        .child(backend_row)
        .child(perms_row)
        .child(caption_promoted(
            "Skip lets agents run any command without asking.",
        ))
        .child(div().px(rpx(10.0)).child(click_checkbox(
            "set-chrome",
            "Claude in Chrome",
            store.chrome_enabled.unwrap_or(false),
            c::BLUE(),
            true,
            dispatch,
            ModalClick::ToggleSetting(SettingToggle::Chrome),
        )))
        .child(caption_promoted(
            "Lets Claude read and control your Chrome tabs.",
        ))
        .child(div().px(rpx(10.0)).child(raw_checkbox(
            "set-telemetry",
            "Share anonymous usage data",
            SettingsState::telemetry_enabled(store),
            c::MAGENTA(),
            |_, cx| {
                let enabled =
                    !SettingsState::telemetry_enabled(&cx.global::<SettingsState>().store);
                SettingsState::update(cx, move |store| store.telemetry_enabled = Some(enabled));
                SettingsState::flush_now(cx);
                // Takes effect immediately, not at the next launch
                // (`src/app/mod.rs:339-344`).
                crate::telemetry::set_enabled(enabled);
                cx.refresh_windows();
            },
        )));

    // ── tools ────────────────────────────────────────────────────────────
    let tools_header = div()
        .flex()
        .items_center()
        .pr(rpx(10.0))
        .child(div().flex_1().child(section_header("TOOLS", 10.0, 4.0)))
        .child(flat_icon_btn("set-tools-refresh", "restart", 28.0, 15.0, {
            let dispatch = std::rc::Rc::clone(dispatch);
            move |window, cx| dispatch(ModalClick::RefreshTools, window, cx)
        }));
    let tools_section = div()
        .flex()
        .flex_col()
        .gap(rpx(4.0))
        .child(tools_header)
        .children(layer.tools.iter().map(|st| tool_row(st, dispatch, cx)));

    // ── the scrolling body: sections above the updates strip ───────────────
    let sections = div()
        .flex()
        .flex_col()
        .gap(rpx(8.0))
        .child(appearance)
        .child(divider_h())
        .child(projects)
        .child(divider_h())
        .child(agents_terminal)
        .child(divider_h())
        .child(tools_section);

    let scroll_body = div()
        .id("settings-scroll")
        .max_h(rpx(420.0))
        .overflow_y_scroll()
        .child(sections);

    let mut body_zone = div().flex().flex_col().gap(rpx(10.0)).child(scroll_body);
    body_zone = body_zone.children(update_actions(layer, dispatch, cx));

    // ── footer: the version/status strip merges into the shared chrome,
    // with an [esc] close hint trailing on the right
    // (`src/gui/view/modals/settings.rs:589-609`). ─────────────────────────
    let current_ver = env!("CARGO_PKG_VERSION");
    let footer = modal_footer_row(
        div()
            .flex()
            .items_center()
            .gap(rpx(10.0))
            .child(
                div()
                    .text_size(rpx(11.0))
                    .text_color(c::FG_DIM())
                    .child(format!("v{current_ver}")),
            )
            .child(update_status_line(layer, cx))
            .child(flat_icon_btn(
                "set-updates-refresh",
                "restart",
                24.0,
                12.0,
                {
                    let dispatch = std::rc::Rc::clone(dispatch);
                    move |window, cx| dispatch(ModalClick::CheckUpdates, window, cx)
                },
            ))
            .child(div().flex_1())
            .child(click_action(
                "set-changelog",
                "View changelog",
                ModalBtn::Plain,
                dispatch,
                ModalClick::OpenChangelog,
            ))
            .child(div().w(rpx(10.0)))
            .child(super::shell::footer_hint("esc", "close")),
    );

    modal_panel(
        580.0,
        div()
            .child(header)
            .child(divider_h())
            .child(modal_body(body_zone))
            .child(divider_h())
            .child(footer),
    )
    .into_any_element()
}

/// One Tools row: a status dot that carries its state by **shape** as well as
/// colour (filled for installed, hollow for missing, so it survives
/// grayscale), the agent, its version, and the default-agent selector
/// (`src/gui/view/modals/settings.rs:396-482`).
fn tool_row(st: &ToolStatus, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    let (status, is_value) = tool_status_text(st);
    let label_color = if st.installed { c::FG() } else { c::FG_DIM() };
    let status_color = if is_value { c::FG_DIM() } else { c::FG_MUTE() };
    let is_default = cx.global::<SettingsState>().store.default_agent == Some(st.agent);
    let dot = div().w(rpx(7.0)).h(rpx(7.0)).rounded(rpx(3.5)).map(|d| {
        if st.installed {
            d.bg(c::GREEN())
        } else {
            d.border_1().border_color(c::FG_MUTE())
        }
    });
    let mut row = div()
        .flex()
        .items_center()
        .gap(rpx(8.0))
        .w_full()
        .px(rpx(8.0))
        .py(rpx(5.0))
        .child(dot)
        .child(
            div()
                .flex_1()
                .text_size(rpx(12.0))
                .text_color(label_color)
                .child(cap(st.agent.label())),
        )
        .child(
            div()
                .text_size(rpx(12.0))
                .text_color(status_color)
                .child(status),
        );
    if is_default {
        row = row.child(
            div()
                .text_size(rpx(10.0))
                .text_color(c::CYAN())
                .child("Default"),
        );
    } else if st.installed {
        row = row.child(click_action(
            "set-default-agent",
            "Set default",
            ModalBtn::Plain,
            dispatch,
            ModalClick::SetDefaultAgent(st.agent),
        ));
    }
    row.into_any_element()
}

/// Capitalize an agent label for display (`cap`, `view/modals/settings.rs`).
fn cap(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

/// The Updates status line — the same six sentences iced shows
/// (`src/gui/view/modals/settings.rs:505-526`).
fn update_status_line(layer: &ModalLayer, cx: &App) -> AnyElement {
    let (text, color) = match layer.upgrade.read(cx).state() {
        UpgradeState::Idle => ("Not checked yet".to_string(), c::FG_MUTE()),
        UpgradeState::Checking => ("Checking…".to_string(), c::FG_MUTE()),
        UpgradeState::UpToDate => ("Up to date".to_string(), c::FG_DIM()),
        UpgradeState::Error(e) => (format!("Check failed: {e}"), c::FG_MUTE()),
        UpgradeState::Available(r) => (format!("Update available: {}", r.tag), c::GREEN()),
        // Updating/Updated/UpdateFailed live in the progress modal.
        _ => ("Updating…".to_string(), c::FG_DIM()),
    };
    div()
        .px(rpx(8.0))
        .py(rpx(4.0))
        .text_size(rpx(11.0))
        .text_color(color)
        .child(text)
        .into_any_element()
}

/// Update / Skip / Copy URL, plus the release-note preview — shown only when a
/// release is on offer. `Update now` is withheld for an unclassifiable install
/// (`InstallMethod::Unknown` cannot self-apply), matching the palette's
/// `update_available_actions` guard.
fn update_actions(layer: &ModalLayer, dispatch: &ModalDispatch, cx: &App) -> Vec<AnyElement> {
    let upgrade = layer.upgrade.read(cx);
    let Some(release) = upgrade.available() else {
        return Vec::new();
    };
    let mut row = div().flex().items_center().gap(rpx(8.0)).px(rpx(8.0));
    if upgrade.method() != grove_core::upgrade::InstallMethod::Unknown {
        row = row.child(click_action(
            "up-now",
            "Update now",
            ModalBtn::Primary,
            dispatch,
            ModalClick::StartUpdate,
        ));
    }
    row = row
        .child(click_action(
            "up-skip",
            "Skip this version",
            ModalBtn::Plain,
            dispatch,
            ModalClick::SkipVersion,
        ))
        .child(click_action(
            "up-copy",
            "Copy URL",
            ModalBtn::Plain,
            dispatch,
            ModalClick::CopyReleaseUrl,
        ));
    let mut out = vec![row.into_any_element()];
    if !release.body.is_empty() {
        let preview: String = release
            .body
            .lines()
            .take(6)
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .take(300)
            .collect();
        out.push(
            div()
                .px(rpx(8.0))
                .pt(rpx(4.0))
                .text_size(rpx(11.0))
                .text_color(c::FG_MUTE())
                .child(preview)
                .into_any_element(),
        );
    }
    out
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
            .gap(rpx(10.0))
            .py(rpx(3.0))
            .child(
                div()
                    .w(rpx(150.0))
                    .child(super::shell::keycap_text(chord_label(d), c::FG_DIM())),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(rpx(12.0))
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
        .gap(rpx(10.0))
        .py(rpx(3.0))
        .child(
            div()
                .w(rpx(150.0))
                .child(super::shell::keycap_text(keys.to_string(), c::FG_DIM())),
        )
        .child(
            div()
                .flex_1()
                .text_size(rpx(12.0))
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
    cx: &App,
) -> AnyElement {
    let store = &cx.global::<SettingsState>().store;
    let project = store.projects.iter().find(|p| p.path == st.project_path);
    let project_name = project.map_or_else(|| st.project_path.clone(), |p| p.name.clone());
    let themes_enabled = store.project_themes_enabled;
    // While Project themes is off nothing actually applies the pin, so the
    // displayed value must show "Default" rather than the stale pinned name
    // (which would otherwise look active when it isn't).
    let pinned_name = if themes_enabled {
        project.and_then(|p| p.theme.as_deref())
    } else {
        None
    };
    let value_text = pinned_name.unwrap_or("Default (follow app)").to_string();
    let value_color = if pinned_name.is_some() {
        c::CYAN()
    } else {
        c::FG_DIM()
    };

    let theme_row_content = div()
        .flex()
        .items_center()
        .gap(rpx(8.0))
        .w_full()
        .child(
            div()
                .flex_1()
                .text_size(rpx(12.0))
                .text_color(if themes_enabled {
                    c::FG()
                } else {
                    c::FG_MUTE()
                })
                .child("Project theme"),
        )
        .child(
            div()
                .text_size(rpx(12.0))
                .text_color(value_color)
                .child(value_text),
        )
        .child(crate::icons::icon("chev-right", 12.0, c::FG_MUTE()));

    let theme_row: AnyElement = if themes_enabled {
        click_row(
            "se-theme-row",
            false,
            dispatch,
            ModalClick::OpenProjectTheme,
            theme_row_content,
        )
        .into_any_element()
    } else {
        div()
            .flex()
            .items_center()
            .px(rpx(8.0))
            .py(rpx(5.0))
            .child(theme_row_content)
            .into_any_element()
    };

    let theme_caption = if themes_enabled {
        "Pin every PTY in this project to a specific theme"
    } else {
        "Enable Project themes in Settings to use this"
    };

    let project_theme_section = div()
        .flex()
        .flex_col()
        .gap(rpx(4.0))
        .child(section_header("PROJECT THEME", 0.0, 0.0))
        .child(theme_row)
        .child(caption(theme_caption));

    let field = |i: usize, label: &'static str, desc: &'static str| {
        let mut d = div()
            .flex()
            .flex_col()
            .gap(rpx(5.0))
            .w_full()
            .child(div().text_size(rpx(12.0)).text_color(c::FG()).child(label))
            .child(
                div()
                    .text_size(rpx(11.0))
                    .text_color(c::FG_MUTE())
                    .child(desc),
            );
        if let Some(f) = layer.fields.get(i) {
            d = d.child(gpui_component::input::Input::new(f.state()).w_full());
        }
        d
    };

    let fields = div()
        .flex()
        .flex_col()
        .gap(rpx(16.0))
        .child(field(
            0,
            "Setup",
            "Runs once when a new worktree is created, inside the new worktree's directory. \
             Use it to install dependencies, copy ignored env files, or start the services \
             an agent needs before you begin working.",
        ))
        .child(field(
            1,
            "Run",
            "Runs on demand when you press the play button (worktree row or session header). \
             It opens an interactive terminal tab, so it suits dev servers, test watchers, \
             or any command you want to watch and interact with.",
        ))
        .child(field(
            2,
            "Teardown",
            "Runs when you delete the worktree, before it is removed from disk. Use it to \
             stop services, tear down databases, or clean up anything setup created. \
             Deletion proceeds once it exits.",
        ));

    let scroll_area = div()
        .id("scripts-editor-scroll")
        .max_h(rpx(480.0))
        .overflow_y_scroll()
        .child(fields);

    let lifecycle_section = div()
        .flex()
        .flex_col()
        .gap(rpx(4.0))
        .child(section_header("LIFECYCLE SCRIPTS", 0.0, 0.0))
        .child(caption(
            "Shell snippets shared by every worktree of this project, run via $SHELL -lc. \
             Leave a field blank to disable that step.",
        ))
        .child(scroll_area);

    let footer_row = div()
        .flex()
        .items_center()
        .gap(rpx(8.0))
        .child(click_action(
            "se-archive",
            "Archive project",
            ModalBtn::Danger,
            dispatch,
            ModalClick::OpenArchiveGate,
        ))
        .child(div().flex_1())
        .child(click_action(
            "se-cancel",
            "Cancel",
            ModalBtn::Plain,
            dispatch,
            ModalClick::Cancel,
        ))
        .child(click_action(
            "se-save",
            "Save",
            ModalBtn::Primary,
            dispatch,
            ModalClick::Save,
        ));

    modal_panel(
        560.0,
        div()
            .child(modal_header(
                format!("Project Settings — {project_name}"),
                c::CYAN(),
            ))
            .child(modal_body(
                div()
                    .flex()
                    .flex_col()
                    .gap(rpx(12.0))
                    .child(project_theme_section)
                    .child(lifecycle_section)
                    .child(footer_row),
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

/// The apply-in-progress modal. Escape is genuinely refused while a stage is
/// in flight (`escape_closes`), so the footer only offers a hint once the
/// apply has landed — exactly as iced does
/// (`src/gui/view/modals/upgrade.rs:16-97`).
fn updating_modal(layer: &ModalLayer, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    use grove_core::upgrade::Stage;

    let state = layer.upgrade.read(cx).state().clone();
    let body = match &state {
        UpgradeState::Updating(stage) => {
            let label = match stage {
                Stage::Downloading => "Downloading…",
                Stage::Building => "Building…",
                Stage::Installing => "Installing…",
                Stage::Done => "Finishing…",
            };
            let tick = layer.clock.read(cx).tick();
            div()
                .flex()
                .items_center()
                .gap(rpx(10.0))
                .child(crate::icons::spinner(16.0, c::FG_DIM(), tick))
                .child(body_text(label))
                .into_any_element()
        }
        UpgradeState::Updated => div()
            .flex()
            .flex_col()
            .gap(rpx(8.0))
            .child(body_text("Update installed. Restart Grove to apply"))
            .child(
                div()
                    .flex()
                    .gap(rpx(8.0))
                    .child(click_action(
                        "up-restart",
                        "Restart",
                        ModalBtn::Primary,
                        dispatch,
                        ModalClick::RestartApp,
                    ))
                    .child(click_action(
                        "up-later",
                        "Later",
                        ModalBtn::Plain,
                        dispatch,
                        ModalClick::Cancel,
                    )),
            )
            .into_any_element(),
        UpgradeState::UpdateFailed(e) => div()
            .flex()
            .flex_col()
            .gap(rpx(6.0))
            .child(body_text("Update failed"))
            // `UpgradeError`'s own `Display`, deliberately unchanged
            // (recorded ambiguity 7).
            .child(
                div()
                    .text_size(rpx(11.0))
                    .text_color(c::FG_MUTE())
                    .child(e.clone()),
            )
            .child(click_action(
                "up-close",
                "Close",
                ModalBtn::Plain,
                dispatch,
                ModalClick::Cancel,
            ))
            .into_any_element(),
        _ => div().child(body_text("Updating…")).into_any_element(),
    };

    let panel = div()
        .child(modal_header("Updating Grove", c::MAGENTA()))
        .child(modal_body(body));
    let panel = match &state {
        // No hint while it runs: the key is refused, and a footer that says
        // otherwise would be a lie.
        UpgradeState::Updating(_) => panel,
        UpgradeState::Updated => panel.child(modal_footer_hints(&[("esc", "later")])),
        _ => panel.child(modal_footer_hints(&[("esc", "close")])),
    };
    modal_panel(420.0, panel).into_any_element()
}

/// Overlays Settings and returns to it on dismiss (carried decision 4). The
/// round trip is the state machine's; this renders `ChangelogState`'s three
/// states (`src/gui/view/modals/upgrade.rs:98-182`).
fn changelog_modal(layer: &ModalLayer, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    let body = match layer.upgrade.read(cx).changelog() {
        ChangelogState::Idle | ChangelogState::Loading => {
            let tick = layer.clock.read(cx).tick();
            div()
                .flex()
                .items_center()
                .gap(rpx(10.0))
                .child(crate::icons::spinner(16.0, c::FG_DIM(), tick))
                .child(body_text("Loading…"))
                .into_any_element()
        }
        ChangelogState::Error(e) => div()
            .child(body_text(format!("Couldn't load changelog: {e}")))
            .into_any_element(),
        ChangelogState::Loaded(notes) if notes.is_empty() => div()
            .child(body_text("No releases yet."))
            .into_any_element(),
        ChangelogState::Loaded(notes) => {
            let mut list = div().flex().flex_col().gap(rpx(18.0));
            for n in notes {
                let mut head = div().flex().items_center().gap(rpx(8.0)).child(
                    div()
                        .text_size(rpx(13.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(c::FG())
                        .child(n.tag.clone()),
                );
                if !n.name.is_empty() && n.name != n.tag {
                    head = head.child(
                        div()
                            .text_size(rpx(13.0))
                            .text_color(c::FG_DIM())
                            .child(n.name.clone()),
                    );
                }
                head = head.child(div().flex_1());
                if !n.date.is_empty() {
                    head = head.child(
                        div()
                            .text_size(rpx(11.0))
                            .text_color(c::FG_MUTE())
                            .child(n.date.clone()),
                    );
                }
                list = list.child(
                    div().flex().flex_col().gap(rpx(4.0)).child(head).child(
                        div()
                            .text_size(rpx(12.0))
                            .text_color(c::FG_MUTE())
                            .child(grove_core::upgrade::clean_markdown(&n.body)),
                    ),
                );
            }
            div()
                .id("changelog-scroll")
                .max_h(rpx(420.0))
                .overflow_y_scroll()
                .child(list)
                .into_any_element()
        }
    };

    modal_panel(
        520.0,
        div()
            .child(modal_header("Changelog", c::MAGENTA()))
            .child(modal_body(
                div()
                    .flex()
                    .flex_col()
                    .gap(rpx(8.0))
                    .child(body)
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

    /// `Terminal` is not a Tools row: it is always available and has no
    /// version (`src/gui/update/upgrade.rs:154-157`).
    #[test]
    fn the_tools_list_omits_the_plain_terminal() {
        assert_eq!(
            SETTINGS_TOOLS,
            [Agent::Claude, Agent::Codex, Agent::OpenCode]
        );
        assert!(!SETTINGS_TOOLS.contains(&Agent::Terminal));
    }

    /// The three Tools states, and which of them reads as a live value rather
    /// than as muted status.
    #[test]
    fn a_tool_row_reports_detecting_then_missing_or_its_version() {
        let base = ToolStatus {
            agent: Agent::Claude,
            installed: false,
            version: None,
            detecting: true,
        };
        assert_eq!(tool_status_text(&base), ("Detecting…".to_string(), false));

        let missing = ToolStatus {
            detecting: false,
            ..base.clone()
        };
        assert_eq!(
            tool_status_text(&missing),
            ("Not installed".to_string(), false)
        );

        let installed = ToolStatus {
            installed: true,
            detecting: false,
            version: Some("1.2.3".to_string()),
            ..base.clone()
        };
        assert_eq!(tool_status_text(&installed), ("1.2.3".to_string(), true));

        // Installed but version-less still reads as installed, never blank.
        let versionless = ToolStatus {
            installed: true,
            detecting: false,
            version: None,
            ..base
        };
        assert_eq!(
            tool_status_text(&versionless),
            ("installed".to_string(), true)
        );
    }

    #[test]
    fn agent_labels_are_capitalized_for_display() {
        assert_eq!(cap("claude"), "Claude");
        assert_eq!(cap(""), "");
    }

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
