//! Settings, the registry-generated ShortcutOverlay, the ScriptsEditor and
//! the Updating/changelog shells — plus the click routing every Task 4-6
//! modal shares.
//!
//! Ports `src/gui/view/modals/settings.rs:130-625` (Settings, with the
//! archived-projects row at :305 and the tmux setting at :325), `:626-790`
//! (the overlay), `src/gui/scripts_editor.rs:63-335` and
//! `src/gui/view/modals/upgrade.rs:16-97,98-182`.

use crate::views::rpx;
use crate::views::tokens::*;
use gpui::{div, prelude::*, AnyElement, App, Context, Div, Focusable, Window};

use grove_core::agent::Agent;

use crate::entities::upgrade::Upgrade;
use crate::entities::upgrade_state::{ChangelogState, UpgradeState};
use crate::keymap::{self, Scope, ShortcutDef, SHORTCUTS};
use crate::launcher::SettingRow;
use crate::settings::SettingsState;
use crate::theme as c;

use super::{Modal, ModalClick, ModalDispatch, ModalLayer, SettingToggle};
use crate::modal::{ScriptsEditorState, ThemePickerReturn, ThemePickerScope};
use crate::views::components::{
    body_text, card, click_action, click_checkbox, click_row, divider_h, field_underline,
    flat_icon_btn, flat_text_btn, flat_text_btn_tinted, keycap, modal_body, modal_checkbox,
    modal_footer_hints, modal_footer_row, modal_header, modal_header_row, modal_panel, mono,
    row_sublabel, section_header, seg_button, seg_group, status_dot, status_gutter, ui, ModalBtn,
    OnToggle, RowDensity, SegSide, SublabelTone,
};
use crate::views::workspace::Workspace;
use crate::zoom::{ZoomState, ZOOM_DEFAULT, ZOOM_STEP};

// ── local layout geometry (§8.4: layout constants live in the owning module) ──
//
// ROW_PX, ROW_PY, ROW_LINE_GAP, ROW_MIN_H, MODAL_SCROLL_MAX_H,
// FIELD_LABEL_COL_W, STATUS_DOT_COL_W, ICON_BTN_W, ICON_BTN_W_SM,
// STEPPER_BTN_W and CARD_LABEL_INDENT now live in `tokens.rs` (via the
// `use ... tokens::*` above) — both modals in this file share them.

/// The changelog release list.
const CHANGELOG_SCROLL_MAX_H: f32 = 420.0;

/// The trailing "Set default" / "Default" column in a Tools row. Fixed so the
/// rows do not reflow when the pill swaps for the button (§13).
const TOOL_ACTION_W: f32 = 84.0;

/// The chord column in the shortcut overlay.
const CHORD_COL_W: f32 = 150.0;
/// The static rows' chord column: the same grid, one gutter wider, because
/// their literal chords (`cmd+c / cmd+v`) are longer than any registry chord.
const STATIC_CHORD_COL_W: f32 = CHORD_COL_W + SPACE_XL * 2.0;

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
            ModalClick::ScriptsRenameStart => self.scripts_rename_start(window, cx),
            ModalClick::ScriptsRenameCommit => self.scripts_rename_commit(window, cx),
            ModalClick::ScriptsRenameCancel => self.scripts_rename_cancel(window, cx),
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
    pub(super) fn save_scripts(&mut self, cx: &mut Context<Self>) {
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
        let new_name = st.name.trim().to_string();
        // Reject a rename that collides (case-insensitively) with another
        // project's name: `RecentLaunch.project` and `grid_order` are keyed
        // by name (see below), so two projects sharing a name would silently
        // merge each other's recents and saved tile order.
        if !new_name.is_empty() {
            let collides = cx
                .global::<SettingsState>()
                .store
                .projects
                .iter()
                .any(|p| p.path != path && p.name.eq_ignore_ascii_case(&new_name));
            if collides {
                self.open(
                    Modal::Message(format!("A project named \"{new_name}\" already exists.")),
                    cx,
                );
                return;
            }
        }
        SettingsState::update(cx, move |store| {
            let Some(idx) = store.projects.iter().position(|p| p.path == path) else {
                return;
            };
            let old_name = store.projects[idx].name.clone();
            store.projects[idx].scripts.setup = setup;
            store.projects[idx].scripts.run = run;
            store.projects[idx].scripts.teardown = teardown;
            // Empty or unchanged: leave the persisted name untouched (no error
            // UI — see the module's Task 4 note). `path` stays the identity
            // key either way.
            if new_name.is_empty() || new_name == old_name {
                return;
            }
            store.projects[idx].name.clone_from(&new_name);
            // `RecentLaunch.project` and `grid_order` are keyed by project
            // NAME (not path — storage.rs:58, storage.rs:132), so a rename
            // must migrate both or the palette's recents list and the grid's
            // saved tile order silently orphan themselves.
            for r in &mut store.recent_launches {
                if r.project == old_name {
                    r.project.clone_from(&new_name);
                }
            }
            let old_prefix = format!("{old_name}::");
            let new_prefix = format!("{new_name}::");
            for key in &mut store.grid_order {
                if let Some(rest) = key.strip_prefix(&old_prefix) {
                    *key = format!("{new_prefix}{rest}");
                }
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

    /// Pencil: enter rename mode and hand the caret to field 0 (the name).
    /// Fields are not rebuilt — `renaming` only changes which one the header
    /// renders — so the three script buffers keep their in-progress text.
    pub(super) fn scripts_rename_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Modal::ScriptsEditor(st)) = self.slot.get_mut() {
            st.renaming = true;
        }
        if let Some(f) = self.fields.first() {
            f.focus_at_end(window, cx);
        }
        cx.notify();
    }

    /// Check: accept the typed name **locally** into `st.name` — the header
    /// switches back to display mode showing it, but nothing is written to
    /// disk until the modal's own Save (`save_scripts` above), so Cancel/Esc
    /// on the modal still undoes it in one story.
    pub(super) fn scripts_rename_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_wizard_buffers(cx);
        if let Some(Modal::ScriptsEditor(st)) = self.slot.get_mut() {
            st.renaming = false;
        }
        if let Some(f) = self.fields.get(1) {
            f.focus_at_end(window, cx);
        }
        cx.notify();
    }

    /// X: discard the typed name, reseeding field 0 from the store's current
    /// `Project::name` (not `st.name`, which the buffer may have already
    /// diverged from), and leave rename mode.
    pub(super) fn scripts_rename_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current_name = match self.slot.get() {
            Some(Modal::ScriptsEditor(st)) => cx
                .global::<SettingsState>()
                .store
                .projects
                .iter()
                .find(|p| p.path == st.project_path)
                .map(|p| p.name.clone()),
            _ => None,
        };
        if let Some(name) = current_name {
            if let Some(f) = self.fields.first() {
                f.set_value(&name, window, cx);
            }
            if let Some(Modal::ScriptsEditor(st)) = self.slot.get_mut() {
                st.name = name;
            }
        }
        if let Some(Modal::ScriptsEditor(st)) = self.slot.get_mut() {
            st.renaming = false;
        }
        if let Some(f) = self.fields.get(1) {
            f.focus_at_end(window, cx);
        }
        cx.notify();
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

pub fn render(
    layer: &ModalLayer,
    dispatch: &ModalDispatch,
    window: &gpui::Window,
    cx: &App,
) -> AnyElement {
    match layer.slot().get() {
        Some(Modal::Settings) => settings_modal(layer, dispatch, cx),
        Some(Modal::ShortcutOverlay) => shortcut_overlay(layer.state.read(cx).screen()),
        Some(Modal::ScriptsEditor(st)) => scripts_editor(layer, st, dispatch, window, cx),
        Some(Modal::Updating) => updating_modal(layer, dispatch, cx),
        Some(Modal::Changelog { .. }) => changelog_modal(layer, dispatch, cx),
        _ => div().into_any_element(),
    }
}

/// The shared grid every Settings row is laid out on: a growing label column,
/// the row's own control cluster (control, then the drill-in chevron if any),
/// and — when a sublabel is present — a second line below the label+control
/// line that never affects the control's vertical position.
///
/// Returns the row's inner content only; [`setting_row_static`] and
/// [`setting_row_link`] add the padding and height that make it a row, since
/// the clickable case gets those from [`click_row`]'s own box.
fn setting_row_grid(
    label: &'static str,
    sublabel: Option<(&'static str, SublabelTone)>,
    control: Option<AnyElement>,
    chevron: bool,
    status: Option<AnyElement>,
) -> Div {
    let mut cluster = div().flex().items_center().gap(rpx(SPACE_MD));
    cluster = cluster.children(control);
    if chevron {
        cluster = cluster.child(crate::icons::icon("chev-right", ICON_SM, c::FG_MUTE()));
    }

    let label_line = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .child(ui(label, TEXT_BODY, c::FG()).flex_1())
        .child(cluster);

    let mut col = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(rpx(ROW_LINE_GAP))
        .child(label_line);
    if let Some((sub, tone)) = sublabel {
        col = col.child(row_sublabel(sub, tone));
    }

    // `items_start`, deliberately: a tall sublabel must not drag the row's
    // cross-axis alignment around. [`status_gutter`] is fixed at `CONTROL_H`
    // and centres its own mark inside that height, which is what actually
    // puts the mark on the label line — not this container's alignment.
    div()
        .flex()
        .items_start()
        .w_full()
        .child(status_gutter(status))
        .child(col)
}

/// A non-interactive row — the control itself (checkbox, segmented group,
/// stepper) carries the interaction, so the row box stays inert rather than
/// gaining a second, larger hit target over the same setting. Every row leads
/// with a [`status_gutter`] (empty here — no static row carries a status
/// mark) so labels align with the rows that do. Sized by [`ROW_PX`]/[`ROW_PY`]
/// padding around its content, with [`ROW_MIN_H`] as a floor only — never a
/// fixed height (§9.1's `RowDensity::Card` precedent, applied to every
/// settings row rather than treating them as the exception).
fn setting_row_static(
    label: &'static str,
    sublabel: Option<(&'static str, SublabelTone)>,
    control: Option<AnyElement>,
) -> Div {
    div()
        .flex()
        .items_center()
        .px(rpx(ROW_PX))
        .py(rpx(ROW_PY))
        .min_h(rpx(ROW_MIN_H))
        .child(setting_row_grid(label, sublabel, control, false, None))
}

/// A drill-in row: the whole row is the hit target because the row *is* the
/// control (App theme / Manage themes… / Archived projects). `RowDensity::Card`
/// gives it the card's square, full-bleed hover fill rather than a rounded
/// rect floating inside the card; its `px`/`py` are restated at [`ROW_PX`]/
/// [`ROW_PY`] (the row padding contract) since `RowDensity::Card`'s own
/// defaults — `SPACE_XL` px, no py, content decides height — are the shared
/// shape other `Card`/`Manager` rows outside Settings still want.
fn setting_row_link(
    id: &'static str,
    label: &'static str,
    value: Option<String>,
    dispatch: &ModalDispatch,
    click: ModalClick,
) -> gpui::Stateful<Div> {
    let value = value.map(|v| mono(v, TEXT_BODY, c::FG_DIM()).into_any_element());
    click_row(
        id,
        false,
        RowDensity::Card,
        dispatch,
        click,
        setting_row_grid(label, None, value, true, None),
    )
    .min_h(rpx(ROW_MIN_H))
    .px(rpx(ROW_PX))
    .py(rpx(ROW_PY))
}

/// A settings row carrying a borderless [`field_underline`] input rather than
/// a segmented control or checkbox — the three lifecycle-script rows. On the
/// same grid [`setting_row_grid`] draws (leading [`status_gutter`], [`ROW_PX`]/
/// [`ROW_PY`] padding, [`ROW_MIN_H`] as a floor), but with the label pinned to
/// [`FIELD_LABEL_COL_W`] rather than flexing, so three fields' inputs all
/// start at the same x. The input column is `flex_1().min_w_0()` so a long
/// value shrinks and clips at the field's own right edge (via
/// [`field_underline`]'s `overflow_hidden`) rather than overflowing into the
/// card's border.
fn setting_row_field(
    label: &'static str,
    sublabel: Option<(&'static str, SublabelTone)>,
    status: Option<AnyElement>,
    input: AnyElement,
) -> Div {
    let field_line = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .child(
            div()
                .w(rpx(FIELD_LABEL_COL_W))
                .flex_shrink_0()
                .child(ui(label, TEXT_BODY, c::FG())),
        )
        .child(div().flex_1().min_w_0().child(input));

    let mut col = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(rpx(ROW_LINE_GAP))
        .child(field_line);
    if let Some((sub, tone)) = sublabel {
        col = col.child(row_sublabel(sub, tone));
    }

    div()
        .flex()
        .items_start()
        .w_full()
        .px(rpx(ROW_PX))
        .py(rpx(ROW_PY))
        .min_h(rpx(ROW_MIN_H))
        .child(status_gutter(status))
        .child(col)
}

/// A [`card`] plus the small uppercase label that names it. The label is
/// [`section_header`] at [`CARD_LABEL_INDENT`] — it used to be a local `ui()`
/// fork, which put a sans run where §5.2 wants mono.
fn settings_card_block(label: &'static str, body: Div) -> Div {
    div()
        .flex()
        .flex_col()
        .child(section_header(label, CARD_LABEL_INDENT, 0.0, SPACE_MD))
        .child(body)
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

    // ── header: title + close only — the save story moved to the footer's
    // left cluster (§16a) ───────────────────────────────────────────────────
    let header = modal_header_row(
        div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_XL))
            .child(ui("Settings", TEXT_TITLE, c::MAGENTA()).flex_1())
            .child(flat_icon_btn("set-close", "close", ICON_BTN_W, ICON_MD, {
                let dispatch = std::rc::Rc::clone(dispatch);
                move |window, cx| dispatch(ModalClick::Cancel, window, cx)
            })),
    );

    // ── appearance ───────────────────────────────────────────────────────
    let zoom_pct = format!("{:.0}%", cx.global::<ZoomState>().zoom * 100.0);
    let app_size_stepper = seg_group(
        div()
            .flex()
            .items_center()
            .child(flat_icon_btn(
                "set-zoom-out",
                "minus",
                STEPPER_BTN_W,
                ICON_MD,
                |_, cx| {
                    Workspace::set_zoom(cx.global::<ZoomState>().zoom - ZOOM_STEP, cx);
                },
            ))
            .child(flat_text_btn(
                "set-zoom-reset",
                zoom_pct,
                TEXT_BODY,
                SPACE_XS,
                |_, cx| {
                    Workspace::set_zoom(ZOOM_DEFAULT, cx);
                },
            ))
            .child(flat_icon_btn(
                "set-zoom-in",
                "plus",
                STEPPER_BTN_W,
                ICON_MD,
                |_, cx| {
                    Workspace::set_zoom(cx.global::<ZoomState>().zoom + ZOOM_STEP, cx);
                },
            )),
    );

    let appearance = settings_card_block(
        "APPEARANCE",
        card(vec![
            setting_row_link(
                "set-theme",
                "App theme",
                Some(setting_value(SettingRow::Theme, cx)),
                dispatch,
                ModalClick::OpenThemePicker,
            )
            .into_any_element(),
            // Not in the iced original (`settings_iced.rs`) — the gpui-only
            // entry point into `ThemeManager` (custom-theme CRUD), kept here
            // since nothing else in Settings opens it.
            setting_row_link(
                "set-manage-themes",
                "Manage themes…",
                None,
                dispatch,
                ModalClick::OpenThemeManager,
            )
            .into_any_element(),
            setting_row_static("App size", None, Some(app_size_stepper.into_any_element()))
                .into_any_element(),
            setting_row_static(
                "Follow system appearance",
                None,
                Some(click_checkbox(
                    "set-follow-system",
                    "",
                    store.theme_follow_system,
                    c::CYAN(),
                    true,
                    dispatch,
                    ModalClick::ToggleSetting(SettingToggle::ThemeFollowSystem),
                )),
            )
            .into_any_element(),
            setting_row_static(
                "Project themes",
                Some((
                    "Let each project pin its PTYs to a specific theme",
                    SublabelTone::Normal,
                )),
                Some(raw_checkbox(
                    "set-project-themes",
                    "",
                    store.project_themes_enabled,
                    c::MAGENTA(),
                    |_, cx| {
                        SettingsState::update(cx, |store| {
                            store.project_themes_enabled = !store.project_themes_enabled;
                        });
                        SettingsState::flush_now(cx);
                        cx.refresh_windows();
                    },
                )),
            )
            .into_any_element(),
        ]),
    );

    // ── agents / terminal ────────────────────────────────────────────────
    let backend_seg = seg_group(
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
    );

    let perms_seg = seg_group(
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
    );

    let agents_terminal = settings_card_block(
        "AGENTS & TERMINAL",
        card(vec![
            setting_row_static("Backend", None, Some(backend_seg.into_any_element()))
                .into_any_element(),
            setting_row_static(
                "Permissions",
                Some((
                    "Skip lets agents run any command without asking.",
                    SublabelTone::Safety,
                )),
                Some(perms_seg.into_any_element()),
            )
            .into_any_element(),
            setting_row_static(
                "Claude in Chrome",
                Some((
                    "Lets Claude read and control your Chrome tabs.",
                    SublabelTone::Normal,
                )),
                Some(click_checkbox(
                    "set-chrome",
                    "",
                    store.chrome_enabled.unwrap_or(false),
                    c::BLUE(),
                    true,
                    dispatch,
                    ModalClick::ToggleSetting(SettingToggle::Chrome),
                )),
            )
            .into_any_element(),
        ]),
    );

    // ── tools ────────────────────────────────────────────────────────────
    // The refresh action rides the card's own header strip rather than the
    // micro-label above it, so it reads as belonging to the detected-agent
    // list it reloads.
    let tools_head = div()
        .flex()
        .items_center()
        .justify_end()
        .px(rpx(ROW_PX))
        .min_h(rpx(ROW_MIN_H))
        .border_b_1()
        .border_color(c::BORDER_SOFT())
        .child(flat_icon_btn(
            "set-tools-refresh",
            "restart",
            ICON_BTN_W,
            ICON_MD,
            {
                let dispatch = std::rc::Rc::clone(dispatch);
                move |window, cx| dispatch(ModalClick::RefreshTools, window, cx)
            },
        ));
    let mut tools_rows: Vec<AnyElement> = vec![tools_head.into_any_element()];
    tools_rows.extend(layer.tools.iter().map(|st| tool_row(st, dispatch, cx)));
    let tools_section = settings_card_block("TOOLS", card(tools_rows));

    // ── data & projects ──────────────────────────────────────────────────
    // Archived projects is shown with "0" rather than hidden when nothing is
    // archived (see the iced original's comment at `settings.rs:299-302`): a
    // row that only appears after the first archive makes the feature
    // undiscoverable at exactly the moment the user needs it.
    let data_projects = settings_card_block(
        "DATA & PROJECTS",
        card(vec![
            setting_row_link(
                "set-archived",
                "Archived projects",
                Some(format!("{archived}")),
                dispatch,
                ModalClick::OpenArchivedProjects,
            )
            .into_any_element(),
            setting_row_static(
                "Share anonymous usage data",
                None,
                Some(raw_checkbox(
                    "set-telemetry",
                    "",
                    SettingsState::telemetry_enabled(store),
                    c::MAGENTA(),
                    |_, cx| {
                        let enabled =
                            !SettingsState::telemetry_enabled(&cx.global::<SettingsState>().store);
                        SettingsState::update(cx, move |store| {
                            store.telemetry_enabled = Some(enabled);
                        });
                        SettingsState::flush_now(cx);
                        // Takes effect immediately, not at the next launch
                        // (`src/app/mod.rs:339-344`).
                        crate::telemetry::set_enabled(enabled);
                        cx.refresh_windows();
                    },
                )),
            )
            .into_any_element(),
        ]),
    );

    // ── the scrolling body: cards above the updates strip ─────────────────
    let sections = div()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_3XL))
        .child(appearance)
        .child(agents_terminal)
        .child(tools_section)
        .child(data_projects);

    let scroll_body = div()
        .id("settings-scroll")
        .max_h(rpx(MODAL_SCROLL_MAX_H))
        .overflow_y_scroll()
        .child(sections);

    let mut body_zone = div()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_XL))
        .child(scroll_body);
    body_zone = body_zone.children(update_actions(layer, dispatch, cx));

    // ── footer: left cluster = context (version/status/refresh, plus the
    // save story that used to live in the header — §16a) and no destructive
    // action here; spacer; right cluster = the esc hint immediately left of
    // the one button (§16). No rule above this strip — `footer_container`'s
    // `BG_STRIP` fill is already the zone edge (§7). ───────────────────────
    let current_ver = env!("CARGO_PKG_VERSION");
    let footer = modal_footer_row(
        div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_XL))
            .child(mono(format!("v{current_ver}"), TEXT_SMALL, c::FG_DIM()))
            .child(update_status_line(layer, cx))
            .child(flat_icon_btn(
                "set-updates-refresh",
                "restart",
                ICON_BTN_W_SM,
                ICON_SM,
                {
                    let dispatch = std::rc::Rc::clone(dispatch);
                    move |window, cx| dispatch(ModalClick::CheckUpdates, window, cx)
                },
            ))
            .child(ui("Changes save automatically.", TEXT_SMALL, c::FG_MUTE()))
            .child(div().flex_1())
            .child(crate::views::components::footer_hint("esc", "close"))
            .child(flat_text_btn(
                "set-changelog",
                "View changelog",
                TEXT_BODY,
                SPACE_LG,
                {
                    let dispatch = std::rc::Rc::clone(dispatch);
                    move |window, cx| dispatch(ModalClick::OpenChangelog, window, cx)
                },
            )),
    );

    modal_panel(
        MODAL_W_XL,
        div()
            .child(header)
            .child(divider_h())
            .child(modal_body(body_zone))
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
    // Installed reads as a filled dot, missing as a hollow ring — the shape
    // difference is what survives grayscale (§2.3).
    let dot = if st.installed {
        status_dot(DOT_MD, c::GREEN())
    } else {
        status_dot(DOT_MD, gpui::transparent_black())
            .border_1()
            .border_color(c::FG_MUTE())
    };
    let mut row = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .w_full()
        .px(rpx(ROW_PX))
        // `ROW_PY` like every other row in every card. A denser roster inside
        // the same card grammar is the drift the unification removed: two
        // vertical rhythms in one panel read as a mistake, not as a density.
        .py(rpx(ROW_PY))
        .min_h(rpx(ROW_MIN_H))
        .child(status_gutter(Some(dot.into_any_element())))
        .child(ui(cap(st.agent.label()), TEXT_BODY, label_color).flex_1())
        .child(mono(status, TEXT_BODY, status_color));
    let slot = div()
        .w(rpx(TOOL_ACTION_W))
        .flex()
        .items_center()
        .justify_end();
    let slot = if is_default {
        slot.child(keycap(mono("Default", TEXT_MICRO, c::FG_DIM())))
    } else if st.installed {
        slot.child(click_action(
            "set-default-agent",
            "Set default",
            ModalBtn::Plain,
            dispatch,
            ModalClick::SetDefaultAgent(st.agent),
        ))
    } else {
        slot
    };
    row = row.child(slot);
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
    ui(text, TEXT_SMALL, color)
        .px(rpx(SPACE_LG))
        .py(rpx(SPACE_SM))
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
    let mut row = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .px(rpx(SPACE_LG));
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
            // Release-note prose, so sans (§5.2) — matching the full changelog
            // list, which renders the same field with `ui`.
            ui(preview, TEXT_SMALL, c::FG_MUTE())
                .px(rpx(SPACE_LG))
                .pt(rpx(SPACE_SM))
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
            .gap(rpx(SPACE_XL))
            .py(rpx(SPACE_SM))
            .child(
                div()
                    .w(rpx(CHORD_COL_W))
                    .child(crate::views::components::keycap_text(
                        chord_label(d),
                        c::FG_DIM(),
                    )),
            )
            .child(ui(d.description, TEXT_BODY, c::FG_DIM()).flex_1())
    };

    let mut body = div().flex().flex_col();
    if grouped {
        body = body.child(section_header("GLOBAL", SPACE_2XL, SPACE_SM, SPACE_SM));
        for d in visible.iter().filter(|d| d.scopes.contains(&Scope::Global)) {
            body = body.child(row(d));
        }
        body = body.child(section_header(
            screen.label(),
            SPACE_2XL,
            SPACE_XL,
            SPACE_SM,
        ));
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
        .child(section_header("EDITING", SPACE_2XL, SPACE_XL, SPACE_SM))
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
        MODAL_W_XL,
        div()
            .child(modal_header("Keyboard shortcuts", c::MAGENTA()))
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
        .gap(rpx(SPACE_XL))
        .py(rpx(SPACE_SM))
        .child(
            div()
                .w(rpx(STATIC_CHORD_COL_W))
                .child(crate::views::components::keycap_text(
                    keys.to_string(),
                    c::FG_DIM(),
                )),
        )
        .child(ui(label, TEXT_BODY, c::FG_DIM()).flex_1())
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

/// Project Settings — the "editable header" layout, unified onto the same
/// row/card grammar App Settings uses (see the module doc and the
/// modal-unification spec). The project name doubles as the header title (no
/// separate NAME section, no `Project Settings — {name}` title); the path
/// sits under it as a mono second line; the theme row and the three lifecycle
/// buffers each become `settings_card_block`s on the shared row grid; and the
/// footer's left cluster carries the destructive action, mirroring
/// `settings_modal`'s footer contract.
fn scripts_editor(
    layer: &ModalLayer,
    st: &ScriptsEditorState,
    dispatch: &ModalDispatch,
    window: &gpui::Window,
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
    // §14: the pinned value is FG_DIM whether or not a theme is pinned — the
    // wording ("Default (follow app)" versus a theme name) is already the
    // carrier, so this is the only accented value in either modal and it
    // loses its CYAN tint.
    let value_color = c::FG_DIM();

    // ── Header: the project name IS the header title. Display mode is a
    // MAGENTA TEXT_TITLE run plus a header-tier pencil button; rename is a
    // deliberate, reversible sub-state (`st.renaming`, mirroring
    // `Modal::ThemeManager`'s `rename`) entered by the pencil and left by
    // check (accept, locally) or X (discard) — save/cancel/discard all still
    // flow through `sync_wizard_buffers`/the modal's own Save, so this is a
    // rendering change plus the explicit affordance, not a behavior change.
    // A close button sits at the far right, and the path is a mono TEXT_SMALL
    // second line — the same shape App Settings' header now has (title +
    // subtitle + close). ──
    let name_field = layer.fields.first();
    let name_focused = st.renaming
        && name_field.is_some_and(|f| f.state().read(cx).focus_handle(cx).is_focused(window));
    // The same focused/BORDER_SOFT rule the lifecycle rows use, not a
    // dedicated always-on rule, so toggling pencil↔check never reflows the
    // header (DESIGN.md §2.4) — both modes measure to the same line height.
    let title_rule = if name_focused {
        c::MAGENTA()
    } else {
        c::BORDER_SOFT()
    };

    let title_el: AnyElement = match (st.renaming, name_field) {
        (true, Some(f)) => div()
            .id("se-title-input")
            .flex_1()
            .min_w_0()
            .border_b_1()
            .border_color(title_rule)
            .font(gpui::font(crate::fonts::UI_FAMILY))
            .text_size(rpx(TEXT_TITLE))
            .text_color(c::MAGENTA())
            .child(
                gpui_component::input::Input::new(f.state())
                    .appearance(false)
                    // `Input` applies its own `input_px`/`input_py` (10px/8px
                    // at the default `Size::Medium`) regardless of
                    // `.appearance(false)` — that inset, not the surrounding
                    // divs, was what broke the title's left edge against the
                    // rest of the panel. Zeroed here rather than compensated
                    // for elsewhere.
                    .pl(gpui::px(0.0))
                    .pr(gpui::px(0.0))
                    .py(gpui::px(0.0))
                    .w_full(),
            )
            .into_any_element(),
        _ => ui(project_name.clone(), TEXT_TITLE, c::MAGENTA())
            .flex_1()
            .min_w_0()
            .into_any_element(),
    };

    let title_controls: AnyElement = if st.renaming {
        div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_XS))
            .child(flat_icon_btn(
                "se-name-accept",
                "check",
                ICON_BTN_W,
                ICON_MD,
                {
                    let dispatch = std::rc::Rc::clone(dispatch);
                    move |window, cx| dispatch(ModalClick::ScriptsRenameCommit, window, cx)
                },
            ))
            .child(flat_icon_btn(
                "se-name-discard",
                "close",
                ICON_BTN_W,
                ICON_MD,
                {
                    let dispatch = std::rc::Rc::clone(dispatch);
                    move |window, cx| dispatch(ModalClick::ScriptsRenameCancel, window, cx)
                },
            ))
            .into_any_element()
    } else {
        flat_icon_btn("se-name-edit", "edit", ICON_BTN_W, ICON_MD, {
            let dispatch = std::rc::Rc::clone(dispatch);
            move |window, cx| dispatch(ModalClick::ScriptsRenameStart, window, cx)
        })
        .into_any_element()
    };

    let close_btn = flat_icon_btn("se-close", "close", ICON_BTN_W, ICON_MD, {
        let dispatch = std::rc::Rc::clone(dispatch);
        move |window, cx| dispatch(ModalClick::Cancel, window, cx)
    });

    let title_row = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_SM))
        .w_full()
        .child(title_el)
        .child(title_controls)
        .child(close_btn);

    let header_content = div()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_XS))
        .w_full()
        .child(title_row)
        .child(mono(st.project_path.clone(), TEXT_SMALL, c::FG_MUTE()));
    let header = modal_header_row(header_content);

    // ── Project theme: a settings_card_block on the shared row grid. Enabled
    // is a setting_row_link (the same drill-in shape "App theme" uses);
    // disabled drops the chevron and the interaction, moving FG_MUTE and the
    // reason to a sublabel rather than an opacity fade (§11/§14).
    let theme_row: AnyElement = if themes_enabled {
        setting_row_link(
            "se-theme-row",
            "Project theme",
            Some(value_text),
            dispatch,
            ModalClick::OpenProjectTheme,
        )
        .into_any_element()
    } else {
        setting_row_grid(
            "Project theme",
            Some(("Enable Project themes in Settings", SublabelTone::Normal)),
            Some(mono(value_text, TEXT_BODY, value_color).into_any_element()),
            false,
            None,
        )
        .px(rpx(ROW_PX))
        .py(rpx(ROW_PY))
        .min_h(rpx(ROW_MIN_H))
        .into_any_element()
    };

    let project_theme_section = settings_card_block("PROJECT THEME", card(vec![theme_row]));

    // ── Lifecycle scripts: three divider-separated setting_row_fields inside
    // a settings_card_block, each carrying its description as a row_sublabel
    // rather than a hover-only tooltip — a sublabel is visible without a
    // pointer and does not duplicate an existing component (§13). The three
    // buffers are genuine `ModalInput::single_line` fields (they used to be
    // `multi_line` textareas) — see `views/modals/mod.rs`'s `ScriptsEditor`
    // arm for why a `multi_line` buffer squeezed into one row broke typing.
    let script_row = |i: usize, label: &'static str, desc: &'static str| -> AnyElement {
        let Some(f) = layer.fields.get(i) else {
            return div().into_any_element();
        };
        let focused = f.state().read(cx).focus_handle(cx).is_focused(window);
        let has_value = !f.value(cx).trim().is_empty();
        let status: AnyElement = if has_value {
            status_dot(DOT_SM, c::GREEN()).into_any_element()
        } else {
            div()
                .size(rpx(DOT_SM))
                .rounded_full()
                .border_1()
                .border_color(c::FG_MUTE())
                .into_any_element()
        };

        let input = field_underline(focused)
            .id(("se-script-row", i as u64))
            .child(
                gpui_component::input::Input::new(f.state())
                    .appearance(false)
                    // Zero `Input`'s own `input_px`/`input_py` inset (see the
                    // title field's comment above) so the mono text sits
                    // flush against the field's bottom rule rather than
                    // floating inside a hidden box.
                    .pl(gpui::px(0.0))
                    .pr(gpui::px(0.0))
                    .py(gpui::px(0.0))
                    .w_full(),
            )
            .into_any_element();

        setting_row_field(
            label,
            Some((desc, SublabelTone::Normal)),
            Some(status),
            input,
        )
        .into_any_element()
    };

    let lifecycle_rows = vec![
        script_row(
            1,
            "Setup",
            "Runs once when a worktree is created, inside its directory.",
        ),
        script_row(
            2,
            "Run",
            "Runs on demand from the play button, in an interactive terminal tab.",
        ),
        script_row(
            3,
            "Teardown",
            "Runs before a worktree is deleted, while it still exists.",
        ),
    ];

    let lifecycle_section = settings_card_block("LIFECYCLE SCRIPTS", card(lifecycle_rows));
    let lifecycle_caption = ui(
        "Shared by every worktree of this project, run via $SHELL -lc. Blank disables the step.",
        TEXT_SMALL,
        c::FG_MUTE(),
    );

    // ── the scrolling body: the same MODAL_SCROLL_MAX_H cap App Settings
    // uses, so a project with a long theme list still fits a laptop
    // viewport. ─────────────────────────────────────────────────────────────
    let sections = div()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_3XL))
        .child(project_theme_section)
        .child(lifecycle_section)
        .child(lifecycle_caption);

    let scroll_body = div()
        .id("scripts-editor-scroll")
        .max_h(rpx(MODAL_SCROLL_MAX_H))
        .overflow_y_scroll()
        .child(sections);

    // ── Footer: left cluster = the destructive action (Archive project, now
    // a flat_text_btn_tinted rather than a bare `ui()` run with a raw
    // `on_mouse_down`); spacer; right cluster = the esc hint immediately left
    // of the Cancel/Save pair (§16). The two save models stay different —
    // this modal keeps its explicit Cancel/Save rather than adopting App
    // Settings' autosave. ───────────────────────────────────────────────────
    let footer = modal_footer_row(
        div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_LG))
            .child(flat_text_btn_tinted(
                "se-archive",
                "Archive project",
                TEXT_BODY,
                SPACE_SM,
                c::RED(),
                {
                    let dispatch = std::rc::Rc::clone(dispatch);
                    move |window, cx| dispatch(ModalClick::OpenArchiveGate, window, cx)
                },
            ))
            .child(div().flex_1())
            .child(crate::views::components::footer_hint("esc", "discard"))
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
            )),
    );

    modal_panel(
        MODAL_W_LG,
        div()
            .child(header)
            .child(divider_h())
            .child(modal_body(scroll_body))
            .child(footer),
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
                .gap(rpx(SPACE_XL))
                .child(crate::icons::spinner(ICON_LG, c::FG_DIM(), tick))
                .child(body_text(label))
                .into_any_element()
        }
        UpgradeState::Updated => div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .child(body_text("Update installed. Restart Grove to apply"))
            .child(
                div()
                    .flex()
                    .gap(rpx(SPACE_LG))
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
            .gap(rpx(SPACE_MD))
            .child(body_text("Update failed"))
            // `UpgradeError`'s own `Display`, deliberately unchanged
            // (recorded ambiguity 7).
            .child(ui(e.clone(), TEXT_SMALL, c::FG_MUTE()))
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
    modal_panel(MODAL_W_SM, panel).into_any_element()
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
                .gap(rpx(SPACE_XL))
                .child(crate::icons::spinner(ICON_LG, c::FG_DIM(), tick))
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
            let mut list = div().flex().flex_col().gap(rpx(SPACE_3XL));
            for n in notes {
                let mut head = div().flex().items_center().gap(rpx(SPACE_LG)).child(
                    mono(n.tag.clone(), TEXT_TITLE, c::FG()).font_weight(gpui::FontWeight::BOLD),
                );
                if !n.name.is_empty() && n.name != n.tag {
                    head = head.child(ui(n.name.clone(), TEXT_TITLE, c::FG_DIM()));
                }
                head = head.child(div().flex_1());
                if !n.date.is_empty() {
                    head = head.child(mono(n.date.clone(), TEXT_SMALL, c::FG_MUTE()));
                }
                list = list.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(rpx(SPACE_SM))
                        .child(head)
                        .child(ui(
                            grove_core::upgrade::clean_markdown(&n.body),
                            TEXT_BODY,
                            c::FG_MUTE(),
                        )),
                );
            }
            div()
                .id("changelog-scroll")
                .max_h(rpx(CHANGELOG_SCROLL_MAX_H))
                .overflow_y_scroll()
                .child(list)
                .into_any_element()
        }
    };

    modal_panel(
        MODAL_W_LG,
        div()
            .child(modal_header("Changelog", c::MAGENTA()))
            .child(modal_body(
                div()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_LG))
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
