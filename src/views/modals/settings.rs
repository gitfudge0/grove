//! Settings, ShortcutOverlay, ScriptsEditor and the Updating/changelog shells.
//! Ports `src/gui/view/modals/settings.rs:130-790`, `src/gui/scripts_editor.rs:63-335`, `src/gui/view/modals/upgrade.rs:16-182`.

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
    body_action, body_text, card, click_action, click_checkbox, click_row, divider_h,
    flat_icon_btn, flat_text_btn, keycap, modal_body, modal_checkbox, modal_footer,
    modal_footer_hints, modal_header_slotted, modal_header_slotted_custom, modal_panel, mono,
    row_sublabel, section_header, seg_button, seg_group, status_dot, status_dot_hollow,
    status_gutter, ui, ModalBtn, OnToggle, RowDensity, SegSide, SublabelTone,
};
use crate::views::workspace::Workspace;
use crate::zoom::{ZoomState, ZOOM_DEFAULT, ZOOM_STEP};

/// Fixed so rows don't reflow when the pill swaps for the button.
const TOOL_ACTION_W: f32 = 84.0;

const CHORD_COL_W: f32 = 150.0;
/// Wider than CHORD_COL_W: static chords (`cmd+c / cmd+v`) are longer than any registry chord.
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

    /// Same persistence as the TmuxChoice modal, but never closes Settings behind it.
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
            cx.emit(super::ModalEvent::TmuxEnabled);
        }
        cx.notify();
    }

    /// Toggles flip in place from the palette drill-in; enum rows open their own modal.
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
                crate::telemetry::set_enabled(enabled);
                cx.notify();
            }
            SettingRow::Chrome => self.toggle_setting(SettingToggle::Chrome, cx),
            SettingRow::Backend => self.toggle_setting(SettingToggle::Tmux, cx),
            SettingRow::Permissions => self.toggle_setting(SettingToggle::SkipPermissions, cx),
            SettingRow::CheckUpdates => self.upgrade.update(cx, |u, cx| u.check(true, cx)),
            SettingRow::AppSize | SettingRow::DefaultAgent => self.open(Modal::Settings, cx),
        }
    }

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

    /// Empty script buffers normalize to `None`; save failure shows a `Message` modal.
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
        // Case-insensitive collision check: RecentLaunch.project/grid_order are keyed by name, so a collision would silently merge two projects' recents/tile order.
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
        // Read outside the closure: the update closure is `move` with a local `old_name`, but sidecar/registry propagation below needs both names after the mutation.
        let renamed_from = cx
            .global::<SettingsState>()
            .store
            .projects
            .iter()
            .find(|p| p.path == path)
            .map(|p| p.name.clone())
            .filter(|old_name| !new_name.is_empty() && *old_name != new_name);
        let new_name_for_update = new_name.clone();
        SettingsState::update(cx, move |store| {
            let new_name = new_name_for_update;
            let Some(idx) = store.projects.iter().position(|p| p.path == path) else {
                return;
            };
            let old_name = store.projects[idx].name.clone();
            store.projects[idx].scripts.setup = setup;
            store.projects[idx].scripts.run = run;
            store.projects[idx].scripts.teardown = teardown;
            if new_name.is_empty() || new_name == old_name {
                return;
            }
            // Pin worktree_dir at the old name before it moves, or the rename orphans every existing worktree dir (storage.rs project_for_worktree_path rule 2).
            grove_core::storage::pin_worktree_dir_on_rename(&mut store.projects[idx], &old_name);
            store.projects[idx].name.clone_from(&new_name);
            // RecentLaunch.project/grid_order are keyed by name, not path — must migrate both or they orphan.
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
        // Propagate the rename to session metadata: persisted sidecars first, then the live registry, so a running app doesn't keep showing the old name.
        if let Some(old_name) = renamed_from {
            grove_core::session_meta::rename_project(&old_name, &new_name);
            self.registry
                .update(cx, |r, _| r.rename_project(&old_name, &new_name));
        }
        self.toast
            .update(cx, |t, cx| t.set_toast("scripts saved", cx));
        self.close(cx);
    }

    /// Enters rename mode without rebuilding fields, so the script buffers keep their in-progress text.
    pub(super) fn scripts_rename_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Modal::ScriptsEditor(st)) = self.slot.get_mut() {
            st.renaming = true;
        }
        if let Some(f) = self.fields.first() {
            f.focus_at_end(window, cx);
        }
        cx.notify();
    }

    /// Accepts the typed name locally only; nothing writes to disk until `save_scripts`, so Cancel/Esc still undoes it.
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

    /// Discards the typed name, reseeding from the store's current name (not `st.name`, which may have diverged).
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

    /// The `open_child` exception: the picker carries the editor's unsaved buffers through and hands them back on cancel.
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolStatus {
    pub agent: Agent,
    pub installed: bool,
    pub version: Option<String>,
    pub detecting: bool,
}

/// `Terminal` is omitted: always available, no version.
pub const SETTINGS_TOOLS: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::OpenCode];

/// Whether the text reads as a live value (a version) or as muted status.
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
    /// Marks every row as detecting and runs the availability+version scan off-thread — three subprocesses is still too many for the UI thread.
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
        Some(Modal::ShortcutOverlay) => shortcut_overlay(layer.state.read(cx).screen(), dispatch),
        Some(Modal::ScriptsEditor(st)) => scripts_editor(layer, st, dispatch, window, cx),
        Some(Modal::Updating) => updating_modal(layer, dispatch, cx),
        Some(Modal::Changelog { .. }) => changelog_modal(layer, dispatch, cx),
        _ => div().into_any_element(),
    }
}

/// Returns the row's inner content only; callers add the padding/height that make it a row.
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

    // items_start deliberately: a tall sublabel must not drag cross-axis alignment; status_gutter centers its own mark independently.
    div()
        .flex()
        .items_start()
        .w_full()
        .child(status_gutter(status))
        .child(col)
}

/// Non-interactive: the control itself carries interaction, so the row box stays inert rather than a second hit target.
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

/// A drill-in row: the whole row is the hit target because the row *is* the control.
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

/// The three lifecycle-script rows: an underline-style input stacked below a title line, rather than the two-column gutter layout other rows use.
fn setting_row_field(
    label: &'static str,
    sublabel: Option<(&'static str, SublabelTone)>,
    input: AnyElement,
) -> Div {
    let mut title_line =
        div()
            .flex()
            .items_baseline()
            .gap(rpx(SPACE_SM))
            .child(ui(label, TEXT_BODY, c::FG()));
    if let Some((sub, tone)) = sublabel {
        title_line = title_line.child(row_sublabel(sub, tone));
    }

    let col = div()
        .flex()
        .flex_col()
        .gap(rpx(ROW_LINE_GAP))
        .w_full()
        .child(title_line)
        .child(input);

    div().w_full().px(rpx(ROW_PX)).py(rpx(ROW_PY)).child(col)
}

fn settings_card_block(label: &'static str, body: Div) -> Div {
    div()
        .flex()
        .flex_col()
        .child(section_header(label, CARD_LABEL_INDENT, 0.0, SPACE_MD))
        .child(body)
}

/// For the two boolean rows with no `SettingToggle` variant of their own; `cx.refresh_windows()` stands in for `cx.notify()`, unavailable to this closure's `&mut App`.
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

    let current_ver = env!("CARGO_PKG_VERSION");
    let header = modal_header_slotted(
        Some("set-close"),
        "Settings",
        c::MAGENTA(),
        Some(mono(format!("v{current_ver}"), TEXT_SMALL, c::FG_DIM()).into_any_element()),
        None,
        Some(dispatch),
    );

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

    // Archived projects shows "0" rather than being hidden — a row that appears only after the first archive is undiscoverable when needed.
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
                        crate::telemetry::set_enabled(enabled);
                        cx.refresh_windows();
                    },
                )),
            )
            .into_any_element(),
        ]),
    );

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

    let updates_row = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_XL))
        .child(update_status_line(layer, cx))
        .child(body_action(
            "set-updates-refresh",
            "Check for updates",
            c::CYAN(),
            dispatch,
            ModalClick::CheckUpdates,
        ))
        .child(ui("Changes save automatically.", TEXT_SMALL, c::FG_MUTE()))
        .into_any_element();

    let mut body_zone = div()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_XL))
        .child(scroll_body)
        .child(updates_row);
    body_zone = body_zone.children(update_actions(layer, dispatch, cx));

    let footer = modal_footer(
        &[("esc", "close")],
        vec![
            flat_text_btn("set-changelog", "View changelog", TEXT_BODY, SPACE_LG, {
                let dispatch = std::rc::Rc::clone(dispatch);
                move |window, cx| dispatch(ModalClick::OpenChangelog, window, cx)
            })
            .into_any_element(),
        ],
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

/// The status dot carries state by shape as well as colour so it survives grayscale.
fn tool_row(st: &ToolStatus, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    let (status, is_value) = tool_status_text(st);
    let label_color = if st.installed { c::FG() } else { c::FG_DIM() };
    let status_color = if is_value { c::FG_DIM() } else { c::FG_MUTE() };
    let is_default = cx.global::<SettingsState>().store.default_agent == Some(st.agent);
    let dot = if st.installed {
        status_dot(DOT_MD, c::GREEN())
    } else {
        status_dot_hollow(DOT_MD, c::FG_MUTE())
    };
    let mut row = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .w_full()
        .px(rpx(ROW_PX))
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
        slot.child(body_action(
            "set-default-agent",
            "Set default",
            c::CYAN(),
            dispatch,
            ModalClick::SetDefaultAgent(st.agent),
        ))
    } else {
        slot
    };
    row = row.child(slot);
    row.into_any_element()
}

fn cap(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

fn update_status_line(layer: &ModalLayer, cx: &App) -> AnyElement {
    let (text, color) = match layer.upgrade.read(cx).state() {
        UpgradeState::Idle => ("Not checked yet".to_string(), c::FG_MUTE()),
        UpgradeState::Checking => ("Checking…".to_string(), c::FG_MUTE()),
        UpgradeState::UpToDate => ("Up to date".to_string(), c::FG_DIM()),
        UpgradeState::Error(e) => (format!("Check failed: {e}"), c::FG_MUTE()),
        UpgradeState::Available(r) => (format!("Update available: {}", r.tag), c::GREEN()),
        _ => ("Updating…".to_string(), c::FG_DIM()),
    };
    ui(text, TEXT_SMALL, color)
        .px(rpx(SPACE_LG))
        .py(rpx(SPACE_SM))
        .into_any_element()
}

/// Shown only when a release is on offer. `Update now` is withheld when `InstallMethod::Unknown` (cannot self-apply).
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
        row = row.child(body_action(
            "up-now",
            "Update now",
            c::CYAN(),
            dispatch,
            ModalClick::StartUpdate,
        ));
    }
    row = row
        .child(body_action(
            "up-skip",
            "Skip this version",
            c::CYAN(),
            dispatch,
            ModalClick::SkipVersion,
        ))
        .child(body_action(
            "up-copy",
            "Copy URL",
            c::CYAN(),
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
            ui(preview, TEXT_SMALL, c::FG_MUTE())
                .px(rpx(SPACE_LG))
                .pt(rpx(SPACE_SM))
                .into_any_element(),
        );
    }
    out
}

/// Generated from `keymap::SHORTCUTS`, filtered by the current screen, plus two static rows.
fn shortcut_overlay(screen: keymap::Screen, dispatch: &ModalDispatch) -> AnyElement {
    let visible: Vec<&ShortcutDef> = SHORTCUTS
        .iter()
        .filter(|d| !d.display_keys.is_empty())
        .filter(|d| scope_allows(d, screen))
        .collect();
    // Group only when the visible set spans both Global and Screen scopes.
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

    let header = modal_header_slotted(
        Some("so-close"),
        "Keyboard shortcuts",
        c::MAGENTA(),
        None,
        None,
        Some(dispatch),
    );

    let scroll_body = div()
        .id("shortcut-overlay-scroll")
        .max_h(rpx(MODAL_SCROLL_MAX_H))
        .overflow_y_scroll()
        .child(body);

    modal_panel(
        MODAL_W_XL,
        div()
            .child(header)
            .child(divider_h())
            .child(modal_body(scroll_body))
            .child(modal_footer_hints(&[("esc", "close")])),
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

/// Alt-chord rule: `cmd+alt+n`/`ctrl+alt+n`, never `ctrl+shift+alt+n`; `literal` rows show verbatim.
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

/// The project name doubles as the header title (no separate NAME section); the path is a mono second line.
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
    // While Project themes is off, show "Default" rather than a stale pinned name that would look active.
    let pinned_name = if themes_enabled {
        project.and_then(|p| p.theme.as_deref())
    } else {
        None
    };
    let value_text = pinned_name.unwrap_or("Default (follow app)").to_string();
    // The wording already carries the distinction, so this value stays FG_DIM regardless of pin state.
    let value_color = c::FG_DIM();

    let name_field = layer.fields.first();
    let name_focused = st.renaming
        && name_field.is_some_and(|f| f.state().read(cx).focus_handle(cx).is_focused(window));
    // Same focused/BORDER_SOFT rule the lifecycle rows use so toggling pencil<->check never reflows the header.
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
                    // Input's own input_px/input_py inset survives `.appearance(false)` and breaks the title's left edge; zeroed here.
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

    let title_row = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_SM))
        .flex_1()
        .min_w_0()
        .child(title_el)
        .child(title_controls)
        .into_any_element();

    // The rename-capable title can't ride modal_header_slotted's plain-SharedString slot, so it uses title_content instead.
    let header = modal_header_slotted_custom(
        None,
        title_row,
        None,
        Some(mono(st.project_path.clone(), TEXT_SMALL, c::FG_MUTE()).into_any_element()),
        Some(dispatch),
    );

    // Disabled drops the chevron/interaction, moving the reason to a sublabel rather than an opacity fade.
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

    // These are single_line fields, not multi_line — a multi_line buffer squeezed into one row broke typing (see ScriptsEditor arm in views/modals/mod.rs).
    let script_row = |i: usize, label: &'static str, desc: &'static str| -> AnyElement {
        let Some(f) = layer.fields.get(i) else {
            return div().into_any_element();
        };
        let focused = f.state().read(cx).focus_handle(cx).is_focused(window);

        let input = div()
            .id(("se-script-row", i as u64))
            .w_full()
            .min_w_0()
            .border_b_1()
            .border_color(if focused {
                c::MAGENTA()
            } else {
                c::BORDER_SOFT()
            })
            .font(gpui::font(crate::fonts::MONO_FAMILY))
            .text_size(rpx(TEXT_BODY))
            .child(
                gpui_component::input::Input::new(f.state())
                    .appearance(false)
                    // Zeroed like the title field above, so text sits flush against the bottom rule.
                    .pl(gpui::px(0.0))
                    .pr(gpui::px(0.0))
                    .py(gpui::px(0.0))
                    .w_full(),
            )
            .into_any_element();

        setting_row_field(label, Some((desc, SublabelTone::Normal)), input).into_any_element()
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

    let archive_action = body_action(
        "se-archive",
        "Archive project",
        c::RED(),
        dispatch,
        ModalClick::OpenArchiveGate,
    );

    let sections = div()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_3XL))
        .child(project_theme_section)
        .child(lifecycle_section)
        .child(lifecycle_caption)
        .child(archive_action);

    let scroll_body = div()
        .id("scripts-editor-scroll")
        .max_h(rpx(MODAL_SCROLL_MAX_H))
        .overflow_y_scroll()
        .child(sections);

    // This modal keeps explicit Cancel/Save rather than App Settings' autosave; hint reads "cancel" to agree with the button.
    let footer = modal_footer(
        &[("esc", "cancel")],
        vec![
            click_action(
                "se-cancel",
                "Cancel",
                ModalBtn::Plain,
                dispatch,
                ModalClick::Cancel,
            )
            .into_any_element(),
            click_action(
                "se-save",
                "Save",
                ModalBtn::Primary,
                dispatch,
                ModalClick::Save,
            )
            .into_any_element(),
        ],
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

/// Escape is genuinely refused while a stage is in flight; the footer only offers a hint once the apply has landed.
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
        UpgradeState::Updated => {
            body_text("Update installed. Restart Grove to apply").into_any_element()
        }
        UpgradeState::UpdateFailed(e) => div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_MD))
            .child(body_text("Update failed"))
            .child(ui(e.clone(), TEXT_SMALL, c::FG_MUTE()))
            .into_any_element(),
        _ => div().child(body_text("Updating…")).into_any_element(),
    };

    // The one exception to "every panel gets a close X": Updating(_) refuses Escape, so a close X there would be dead.
    let header = match &state {
        UpgradeState::Updating(_) => {
            modal_header_slotted(None, "Updating Grove", c::MAGENTA(), None, None, None)
        }
        _ => modal_header_slotted(
            Some("up-header-close"),
            "Updating Grove",
            c::MAGENTA(),
            None,
            None,
            Some(dispatch),
        ),
    };

    let panel = div()
        .child(header)
        .child(divider_h())
        .child(modal_body(body));
    let panel = match &state {
        UpgradeState::Updating(_) => panel,
        UpgradeState::Updated => panel.child(modal_footer(
            &[("esc", "close")],
            vec![
                click_action(
                    "up-later",
                    "Later",
                    ModalBtn::Plain,
                    dispatch,
                    ModalClick::Cancel,
                )
                .into_any_element(),
                click_action(
                    "up-restart",
                    "Restart",
                    ModalBtn::Primary,
                    dispatch,
                    ModalClick::RestartApp,
                )
                .into_any_element(),
            ],
        )),
        UpgradeState::UpdateFailed(_) => panel.child(modal_footer(
            &[("esc", "close")],
            vec![click_action(
                "up-close",
                "Close",
                ModalBtn::Primary,
                dispatch,
                ModalClick::Cancel,
            )
            .into_any_element()],
        )),
        _ => panel.child(modal_footer_hints(&[("esc", "close")])),
    };
    modal_panel(MODAL_W_SM, panel).into_any_element()
}

/// Overlays Settings and returns to it on dismiss; renders `ChangelogState`'s three states.
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
                .max_h(rpx(MODAL_SCROLL_MAX_H))
                .overflow_y_scroll()
                .child(list)
                .into_any_element()
        }
    };

    modal_panel(
        MODAL_W_LG,
        div()
            .child(modal_header_slotted(
                Some("cl-close"),
                "Changelog",
                c::MAGENTA(),
                None,
                None,
                Some(dispatch),
            ))
            .child(divider_h())
            .child(modal_body(body))
            .child(modal_footer(
                &[("esc", "back")],
                vec![click_action(
                    "cl-back",
                    "Back",
                    ModalBtn::Primary,
                    dispatch,
                    ModalClick::Cancel,
                )
                .into_any_element()],
            )),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tools_list_omits_the_plain_terminal() {
        assert_eq!(
            SETTINGS_TOOLS,
            [Agent::Claude, Agent::Codex, Agent::OpenCode]
        );
        assert!(!SETTINGS_TOOLS.contains(&Agent::Terminal));
    }

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

    /// Drift guard: every registry row with a display label appears on at least one screen.
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
