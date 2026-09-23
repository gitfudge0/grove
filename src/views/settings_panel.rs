//! App-wide settings and shortcut reference, mounted by the shell as an overlay.
use super::{rpx, tokens::*};
use crate::{
    entities::upgrade_state::{ChangelogState, UpgradeState},
    keymap::{self, Scope, SHORTCUTS},
    runtime::Runtime,
    settings::SettingsState,
    theme::{self as c, ThemeState, DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME},
    zoom::{self, ZoomState, ZOOM_DEFAULT, ZOOM_STEP},
};
use gpui::{
    div, prelude::*, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    MouseButton, ScrollHandle, Subscription, Window,
};
use grove_core::{agent::Agent, storage::Store, theme::ThemeKind, upgrade::InstallMethod};
use std::collections::HashSet;

const FOCUS_SCAN_LIMIT: usize = 128;

/// The shell owns navigation to the existing archived-project panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsPanelEvent {
    Closed,
    OpenArchivedProjects,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Settings,
    Shortcuts,
    Changelog,
    Themes(ThemeKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Appearance {
    FollowSystem,
    Dark,
    Light,
}

fn appearance(store: &Store) -> Appearance {
    if store.theme_follow_system {
        Appearance::FollowSystem
    } else if store
        .theme
        .as_deref()
        .and_then(grove_core::theme::by_name)
        .is_some_and(|theme| theme.kind == ThemeKind::Light)
    {
        Appearance::Light
    } else {
        Appearance::Dark
    }
}

fn theme_for(mode: Appearance, store: &Store) -> String {
    match mode {
        Appearance::FollowSystem | Appearance::Dark => store
            .theme_dark
            .clone()
            .unwrap_or_else(|| DEFAULT_DARK_THEME.into()),
        Appearance::Light => store
            .theme_light
            .clone()
            .unwrap_or_else(|| DEFAULT_LIGHT_THEME.into()),
    }
}

fn edited_theme_is_active(
    store: &Store,
    system_mode: gpui::WindowAppearance,
    kind: ThemeKind,
) -> bool {
    if store.theme_follow_system {
        let system_kind = match system_mode {
            gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight => {
                ThemeKind::Light
            }
            gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark => ThemeKind::Dark,
        };
        kind == system_kind
    } else {
        (kind == ThemeKind::Light) == (appearance(store) == Appearance::Light)
    }
}

fn save_named_theme(store: &mut Store, kind: ThemeKind, name: &str, active: bool) {
    match kind {
        ThemeKind::Dark => store.theme_dark = Some(name.into()),
        ThemeKind::Light => store.theme_light = Some(name.into()),
    }
    if active && !store.theme_follow_system {
        store.theme = Some(name.into());
    }
}

/// The reference lists only actions the replacement shell handles. The numbered
/// row is display-only in `SHORTCUTS`, but `shell_bindings` generates its keys.
fn handled_shortcuts() -> Vec<(usize, &'static keymap::ShortcutDef)> {
    let bound: HashSet<String> = keymap::shell_bindings()
        .iter()
        .map(|binding| binding.action().name().to_string())
        .collect();
    SHORTCUTS
        .iter()
        .enumerate()
        .filter(|(_, shortcut)| !shortcut.display_keys.is_empty())
        .filter(|(_, shortcut)| match shortcut.action {
            Some(action) => {
                let name = format!("{action:?}");
                bound.iter().any(|bound| bound.ends_with(&name))
            }
            None if shortcut.display_keys == "1–9" => {
                bound.iter().any(|bound| bound.ends_with("SelectSession"))
            }
            None => false,
        })
        .collect()
}

fn panel_width(viewport: f32) -> f32 {
    MODAL_W_XL.min((viewport - SPACE_LG * 2.0).max(0.0))
}

pub struct SettingsPanel {
    runtime: Entity<Runtime>,
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    open: bool,
    page: Page,
    error: Option<String>,
    scroll: ScrollHandle,
    _settings_observer: Subscription,
    _upgrade_observer: Subscription,
}

impl EventEmitter<SettingsPanelEvent> for SettingsPanel {}

impl SettingsPanel {
    pub fn new(runtime: Entity<Runtime>, cx: &mut Context<Self>) -> Self {
        let upgrade = runtime.read(cx).upgrade.clone();
        Self {
            runtime,
            focus: cx.focus_handle(),
            return_focus: None,
            open: false,
            page: Page::Settings,
            error: None,
            scroll: ScrollHandle::new(),
            _settings_observer: cx.observe_global::<SettingsState>(|_, cx| cx.notify()),
            _upgrade_observer: cx.observe(&upgrade, |_, _, cx| cx.notify()),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            return;
        }
        self.return_focus = window.focused(cx);
        self.open = true;
        self.page = Page::Settings;
        self.error = None;
        self.scroll.set_offset(gpui::Point::default());
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub fn open_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open(window, cx);
        self.show(Page::Shortcuts, cx);
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.page = Page::Settings;
        self.error = None;
        if let Some(focus) = self.return_focus.take() {
            focus.focus(window, cx);
        }
        cx.emit(SettingsPanelEvent::Closed);
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut Store)) -> bool {
        let ((), result) = SettingsState::update_and_flush_checked(cx, edit);
        match result {
            Ok(()) => {
                self.error = None;
                cx.notify();
                true
            }
            Err(error) => {
                self.error = Some(format!("Could not save settings: {error}"));
                cx.notify();
                false
            }
        }
    }

    fn set_appearance(&mut self, mode: Appearance, cx: &mut Context<Self>) {
        let name = theme_for(mode, &cx.global::<SettingsState>().store);
        if grove_core::theme::by_name(&name).is_none() {
            self.error = Some(format!("Theme {name} is unavailable."));
            cx.notify();
            return;
        }
        if !self.save(cx, |store| {
            store.theme_follow_system = mode == Appearance::FollowSystem;
            if mode != Appearance::FollowSystem {
                store.theme = Some(name.clone());
            }
        }) {
            return;
        }
        cx.update_global::<ThemeState, _>(|state, _| {
            state.follow_system = mode == Appearance::FollowSystem;
        });
        if mode == Appearance::FollowSystem {
            let system_light = matches!(
                cx.global::<ThemeState>().system_mode,
                gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight
            );
            c::set_chrome_light(system_light);
            ThemeState::apply_system_theme(cx);
        } else {
            c::set_chrome_light(mode == Appearance::Light);
            ThemeState::set_by_name(cx, &name);
        }
        cx.refresh_windows();
    }

    fn set_named_theme(&mut self, kind: ThemeKind, name: String, cx: &mut Context<Self>) {
        if grove_core::theme::by_name(&name).is_none_or(|theme| theme.kind != kind) {
            self.error = Some(format!("Theme {name} is unavailable."));
            cx.notify();
            return;
        }
        let store = &cx.global::<SettingsState>().store;
        let follows_system = store.theme_follow_system;
        let active = edited_theme_is_active(store, cx.global::<ThemeState>().system_mode, kind);
        if !self.save(cx, |store| save_named_theme(store, kind, &name, active)) {
            return;
        }
        cx.update_global::<ThemeState, _>(|state, _| match kind {
            ThemeKind::Dark => state.dark_name.clone_from(&name),
            ThemeKind::Light => state.light_name.clone_from(&name),
        });
        if !active {
            self.page = Page::Settings;
            cx.notify();
            return;
        }
        if follows_system {
            ThemeState::apply_system_theme(cx);
            c::set_chrome_light(matches!(
                cx.global::<ThemeState>().system_mode,
                gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight
            ));
        } else {
            c::set_chrome_light(kind == ThemeKind::Light);
            ThemeState::set_by_name(cx, &name);
        }
        self.page = Page::Settings;
        cx.refresh_windows();
    }

    fn set_zoom(&mut self, delta: f32, cx: &mut Context<Self>) {
        let current = cx.global::<ZoomState>().zoom;
        let next = if delta == 0.0 {
            ZOOM_DEFAULT
        } else {
            zoom::snap(current + delta)
        };
        if next != current && self.save(cx, |store| store.ui_zoom = Some(next)) {
            cx.update_global::<ZoomState, _>(|state, _| state.zoom = next);
            cx.refresh_windows();
        }
    }

    fn set_backend(&mut self, tmux: bool, cx: &mut Context<Self>) {
        if tmux && !grove_core::tmux::available() {
            self.error = Some("tmux is not installed; native sessions remain selected.".into());
            cx.notify();
            return;
        }
        if self.save(cx, |store| store.tmux_enabled = Some(tmux)) && tmux {
            self.runtime.update(cx, Runtime::discover_tmux_sessions);
        }
    }

    fn set_default_agent(&mut self, agent: Agent, cx: &mut Context<Self>) {
        if !agent.available() {
            self.error = Some(format!("{} is not installed.", agent.label()));
            cx.notify();
            return;
        }
        self.save(cx, |store| store.default_agent = Some(agent));
    }

    fn toggle_permissions(&mut self, cx: &mut Context<Self>) {
        self.save(cx, |store| {
            let on = store.dangerously_skip_permissions_enabled.unwrap_or(false);
            store.dangerously_skip_permissions_enabled = Some(!on);
        });
    }

    fn toggle_chrome(&mut self, cx: &mut Context<Self>) {
        self.save(cx, |store| {
            store.chrome_enabled = Some(!store.chrome_enabled.unwrap_or(false));
        });
    }

    fn toggle_telemetry(&mut self, cx: &mut Context<Self>) {
        let enabled = !SettingsState::telemetry_enabled(&cx.global::<SettingsState>().store);
        if self.save(cx, |store| store.telemetry_enabled = Some(enabled)) {
            crate::telemetry::set_enabled(enabled);
        }
    }

    fn open_archived(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
        cx.emit(SettingsPanelEvent::OpenArchivedProjects);
    }

    fn show(&mut self, page: Page, cx: &mut Context<Self>) {
        if page == Page::Changelog {
            let upgrade = self.runtime.read(cx).upgrade.clone();
            upgrade.update(cx, crate::entities::upgrade::Upgrade::fetch_changelog);
        }
        self.page = page;
        self.error = None;
        self.scroll.set_offset(gpui::Point::default());
        cx.notify();
    }

    fn key(&mut self, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" if self.page != Page::Settings => self.show(Page::Settings, cx),
            "escape" => self.close(window, cx),
            "tab" => {
                for _ in 0..FOCUS_SCAN_LIMIT {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    if self.focus.contains_focused(window, cx) {
                        break;
                    }
                }
                if !self.focus.contains_focused(window, cx) {
                    self.focus.focus(window, cx);
                }
            }
            _ => return,
        }
        window.prevent_default();
        cx.stop_propagation();
    }

    fn button(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<gpui::SharedString>,
        selected: bool,
        enabled: bool,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .tab_index(if enabled { 0 } else { -1 })
            .aria_label(label.into())
            .px(rpx(SPACE_2XL))
            .h(rpx(CONTROL_H + SPACE_LG))
            .flex()
            .items_center()
            .rounded(rpx(RADIUS_CONTROL))
            .bg(if selected {
                c::BG_HOVER()
            } else {
                c::FIELD_FILL()
            })
            .border_1()
            .border_color(if selected {
                c::CYAN()
            } else {
                c::BORDER_SOFT()
            })
            .text_size(rpx(TEXT_BODY))
            .text_color(if enabled { c::FG() } else { c::FG_DIM() })
            .when(!enabled, |el| el.opacity(OPACITY_DISABLED))
    }

    fn section(label: &'static str) -> gpui::Div {
        div()
            .text_size(rpx(TEXT_MICRO))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c::FG_DIM())
            .child(label)
    }

    fn setting_row(
        label: &'static str,
        detail: impl Into<gpui::SharedString>,
        control: impl IntoElement,
    ) -> gpui::Div {
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(rpx(SPACE_LG))
            .py(rpx(SPACE_LG))
            .border_b_1()
            .border_color(c::BORDER_SOFT())
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .text_size(rpx(TEXT_BODY))
                            .text_color(c::FG())
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(detail.into()),
                    ),
            )
            .child(control)
    }

    fn settings_body(&self, cx: &mut Context<Self>) -> gpui::Div {
        let store = &cx.global::<SettingsState>().store;
        let mode = appearance(store);
        let zoom = cx.global::<ZoomState>().zoom;
        let tmux_on = store.tmux_enabled.unwrap_or(false);
        let tmux_available = grove_core::tmux::available();
        let skip = store.dangerously_skip_permissions_enabled.unwrap_or(false);
        let chrome = store.chrome_enabled.unwrap_or(false);
        let telemetry = SettingsState::telemetry_enabled(store);
        let archived = store.archived_count();
        let mut body = div().flex().flex_col().gap(rpx(SPACE_3XL));

        let themes = div()
            .flex()
            .flex_wrap()
            .gap(rpx(SPACE_SM))
            .child(
                self.button(
                    "settings-theme-system",
                    "System",
                    mode == Appearance::FollowSystem,
                    true,
                )
                .child("System")
                .on_click(
                    cx.listener(|this, _, _, cx| this.set_appearance(Appearance::FollowSystem, cx)),
                ),
            )
            .child(
                self.button(
                    "settings-theme-dark",
                    "Dark",
                    mode == Appearance::Dark,
                    true,
                )
                .child("Dark")
                .on_click(cx.listener(|this, _, _, cx| this.set_appearance(Appearance::Dark, cx))),
            )
            .child(
                self.button(
                    "settings-theme-light",
                    "Light",
                    mode == Appearance::Light,
                    true,
                )
                .child("Light")
                .on_click(cx.listener(|this, _, _, cx| this.set_appearance(Appearance::Light, cx))),
            );
        let zoom_control = div()
            .flex()
            .gap(rpx(SPACE_SM))
            .child(
                self.button(
                    "settings-zoom-out",
                    "Zoom out",
                    false,
                    zoom > zoom::ZOOM_MIN,
                )
                .child("−")
                .when(zoom > zoom::ZOOM_MIN, |el| {
                    el.on_click(cx.listener(|this, _, _, cx| this.set_zoom(-ZOOM_STEP, cx)))
                }),
            )
            .child(
                self.button("settings-zoom-reset", "Reset zoom", false, true)
                    .child(format!("{:.0}%", zoom * 100.0))
                    .on_click(cx.listener(|this, _, _, cx| this.set_zoom(0.0, cx))),
            )
            .child(
                self.button("settings-zoom-in", "Zoom in", false, zoom < zoom::ZOOM_MAX)
                    .child("+")
                    .when(zoom < zoom::ZOOM_MAX, |el| {
                        el.on_click(cx.listener(|this, _, _, cx| this.set_zoom(ZOOM_STEP, cx)))
                    }),
            );
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .child(Self::section("APPEARANCE"))
                .child(Self::setting_row(
                    "App theme",
                    "One theme across Grove and all terminals",
                    themes,
                ))
                .child(Self::setting_row(
                    "Dark palette",
                    theme_for(Appearance::Dark, store),
                    self.button("settings-dark-palette", "Choose dark theme", false, true)
                        .child("Choose")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show(Page::Themes(ThemeKind::Dark), cx);
                        })),
                ))
                .child(Self::setting_row(
                    "Light palette",
                    theme_for(Appearance::Light, store),
                    self.button("settings-light-palette", "Choose light theme", false, true)
                        .child("Choose")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show(Page::Themes(ThemeKind::Light), cx);
                        })),
                ))
                .child(Self::setting_row(
                    "App size",
                    "Scale the interface",
                    zoom_control,
                )),
        );

        let backend = div()
            .flex()
            .gap(rpx(SPACE_SM))
            .child(
                self.button("settings-backend-native", "Native backend", !tmux_on, true)
                    .child("Native")
                    .on_click(cx.listener(|this, _, _, cx| this.set_backend(false, cx))),
            )
            .child(
                self.button(
                    "settings-backend-tmux",
                    "Tmux backend",
                    tmux_on,
                    tmux_available,
                )
                .child("Tmux")
                .when(tmux_available, |el| {
                    el.on_click(cx.listener(|this, _, _, cx| this.set_backend(true, cx)))
                }),
            );
        let permissions = self
            .button(
                "settings-permissions",
                "Toggle permission prompts",
                skip,
                true,
            )
            .child(if skip { "Skip prompts" } else { "Ask me" })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_permissions(cx)));
        let browser = self
            .button("settings-chrome", "Toggle Claude in Chrome", chrome, true)
            .child(if chrome { "On" } else { "Off" })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_chrome(cx)));
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .child(Self::section("AGENTS & TERMINAL"))
                .child(Self::setting_row(
                    "Backend",
                    if tmux_available {
                        "New sessions use this backend"
                    } else {
                        "tmux is unavailable on this system"
                    },
                    backend,
                ))
                .child(Self::setting_row(
                    "Permissions",
                    "Skip lets agents run commands without asking",
                    permissions,
                ))
                .child(Self::setting_row(
                    "Claude in Chrome",
                    "Let Claude read and control Chrome tabs",
                    browser,
                )),
        );

        let mut agents = div().flex().flex_wrap().gap(rpx(SPACE_SM));
        for agent in Agent::ALL {
            let available = agent.available();
            let selected = store.default_agent == Some(agent);
            let label = if available {
                agent.label().to_string()
            } else {
                format!("{} (unavailable)", agent.label())
            };
            agents = agents.child(
                self.button(
                    format!("settings-agent-{}", agent.label()),
                    label.clone(),
                    selected,
                    available,
                )
                .child(label)
                .when(available, |el| {
                    el.on_click(
                        cx.listener(move |this, _, _, cx| this.set_default_agent(agent, cx)),
                    )
                }),
            );
        }
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .child(Self::section("DEFAULT AGENT"))
                .child(Self::setting_row(
                    "New sessions",
                    "Unavailable agents cannot be selected",
                    agents,
                )),
        );

        let archived_button = self
            .button("settings-archived", "Open archived projects", false, true)
            .child(format!("Open ({archived})"))
            .on_click(cx.listener(|this, _, window, cx| this.open_archived(window, cx)));
        let telemetry_button = self
            .button(
                "settings-telemetry",
                "Toggle anonymous usage data",
                telemetry,
                true,
            )
            .child(if telemetry { "On" } else { "Off" })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_telemetry(cx)));
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .child(Self::section("DATA & PROJECTS"))
                .child(Self::setting_row(
                    "Archived projects",
                    "Restore or remove project registrations",
                    archived_button,
                ))
                .child(Self::setting_row(
                    "Share anonymous usage data",
                    "Usage and diagnostics",
                    telemetry_button,
                )),
        );

        let upgrade = self.runtime.read(cx).upgrade.clone();
        let upgrade = upgrade.read(cx);
        let (status, checking) = match upgrade.state() {
            UpgradeState::Idle => ("Not checked yet".to_string(), false),
            UpgradeState::Checking => ("Checking…".to_string(), true),
            UpgradeState::UpToDate => ("Up to date".to_string(), false),
            UpgradeState::Available(release) => {
                (format!("Update available: {}", release.tag), false)
            }
            UpgradeState::Error(error) => (format!("Check failed: {error}"), false),
            UpgradeState::Updating(_) => ("Updating…".to_string(), true),
            UpgradeState::Updated => ("Update installed".to_string(), false),
            UpgradeState::UpdateFailed(error) => (format!("Update failed: {error}"), false),
        };
        let can_update =
            upgrade.available().is_some() && upgrade.method() != InstallMethod::Unknown;
        let has_update = upgrade.available().is_some();
        let mut actions = div().flex().flex_wrap().gap(rpx(SPACE_SM));
        if has_update {
            actions = actions.child(
                self.button("settings-update-now", "Update now", false, can_update)
                    .child("Update now")
                    .when(can_update, |el| {
                        el.on_click(cx.listener(|this, _, _, cx| {
                            let upgrade = this.runtime.read(cx).upgrade.clone();
                            upgrade.update(cx, crate::entities::upgrade::Upgrade::start_update);
                        }))
                    }),
            );
            actions = actions.child(
                self.button("settings-update-skip", "Skip this version", false, true)
                    .child("Skip version")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let upgrade = this.runtime.read(cx).upgrade.clone();
                        upgrade.update(cx, crate::entities::upgrade::Upgrade::skip);
                    })),
            );
            actions = actions.child(
                self.button("settings-update-copy-url", "Copy release URL", false, true)
                    .child("Copy URL")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let upgrade = this.runtime.read(cx).upgrade.clone();
                        let url = upgrade
                            .read(cx)
                            .available()
                            .map(|release| release.html_url.clone());
                        if let Some(url) = url {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(url));
                        }
                    })),
            );
        }
        actions = actions
            .child(
                self.button(
                    "settings-update-check",
                    "Check for updates",
                    false,
                    !checking,
                )
                .child("Check now")
                .when(!checking, |el| {
                    el.on_click(cx.listener(|this, _, _, cx| {
                        let upgrade = this.runtime.read(cx).upgrade.clone();
                        upgrade.update(cx, |upgrade, cx| upgrade.check(true, cx));
                    }))
                }),
            )
            .child(
                self.button("settings-changelog", "View changelog", false, true)
                    .child("Changelog")
                    .on_click(cx.listener(|this, _, _, cx| this.show(Page::Changelog, cx))),
            );
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .child(Self::section("UPDATES"))
                .child(Self::setting_row(
                    "Grove version",
                    format!("v{} · {status}", env!("CARGO_PKG_VERSION")),
                    actions,
                )),
        );
        body.child(
            div()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .child(Self::section("KEYBOARD"))
                .child(Self::setting_row(
                    "Shortcuts",
                    "Commands for the current view",
                    self.button("settings-shortcuts", "View shortcuts", false, true)
                        .child("View shortcuts")
                        .on_click(cx.listener(|this, _, _, cx| this.show(Page::Shortcuts, cx))),
                )),
        )
    }

    fn shortcuts_body(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut body = div().flex().flex_col().gap(rpx(SPACE_SM));
        for (index, shortcut) in handled_shortcuts() {
            let scopes = shortcut
                .scopes
                .iter()
                .map(|scope| match scope {
                    Scope::Global => "Global".to_string(),
                    Scope::Screen(screen) => screen.label().to_string(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let chord = if shortcut.literal {
                shortcut.display_keys.to_string()
            } else if shortcut.requires_alt {
                format!(
                    "{}{}",
                    keymap::alt_chord_prefix().replace('-', "+"),
                    shortcut.display_keys
                )
            } else {
                format!("{}+{}", keymap::platform_mod_label(), shortcut.display_keys)
            };
            body = body.child(
                div()
                    .id(format!("settings-shortcut-{index}"))
                    .flex()
                    .flex_wrap()
                    .gap(rpx(SPACE_LG))
                    .py(rpx(SPACE_SM))
                    .child(
                        div()
                            .min_w(rpx(135.0))
                            .font_family(crate::fonts::MONO_FAMILY)
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::CYAN())
                            .child(chord),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(rpx(TEXT_BODY))
                            .text_color(c::FG())
                            .child(shortcut.description),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child(scopes),
                    ),
            );
        }
        let _ = cx;
        body
    }

    fn changelog_body(&self, cx: &mut Context<Self>) -> gpui::Div {
        let upgrade = self.runtime.read(cx).upgrade.clone();
        let upgrade = upgrade.read(cx);
        let mut body = div().flex().flex_col().gap(rpx(SPACE_2XL));
        match upgrade.changelog() {
            ChangelogState::Idle | ChangelogState::Loading => {
                body = body.child("Loading changelog…");
            }
            ChangelogState::Error(error) => {
                body = body.child(format!("Could not load changelog: {error}"));
            }
            ChangelogState::Loaded(notes) if notes.is_empty() => {
                body = body.child("No release notes available.");
            }
            ChangelogState::Loaded(notes) => {
                for (index, note) in notes.iter().enumerate() {
                    body = body.child(
                        div()
                            .id(format!("settings-release-{index}"))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_SM))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(c::FG())
                                    .child(note.tag.clone()),
                            )
                            .child(div().text_color(c::FG_DIM()).child(note.body.clone())),
                    );
                }
            }
        }
        body.text_size(rpx(TEXT_BODY))
    }

    fn themes_body(&self, kind: ThemeKind, cx: &mut Context<Self>) -> gpui::Div {
        let selected = match kind {
            ThemeKind::Dark => theme_for(Appearance::Dark, &cx.global::<SettingsState>().store),
            ThemeKind::Light => theme_for(Appearance::Light, &cx.global::<SettingsState>().store),
        };
        let mut body = div().flex().flex_col().gap(rpx(SPACE_SM));
        for (index, theme) in grove_core::theme::selectable_themes_of(kind)
            .into_iter()
            .enumerate()
        {
            let name = theme.name.to_string();
            let is_selected = selected == name;
            body = body.child(
                self.button(
                    format!("settings-theme-choice-{index}"),
                    name.clone(),
                    is_selected,
                    true,
                )
                .child(name.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_named_theme(kind, name.clone(), cx);
                })),
            );
        }
        body
    }
}

impl Focusable for SettingsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = f32::from(window.rem_size()) / zoom::REM_BASE;
        let viewport_w = f32::from(window.viewport_size().width) / scale;
        let viewport_h = f32::from(window.viewport_size().height) / scale;
        let width = panel_width(viewport_w);
        let height = (viewport_h - SPACE_LG * 2.0).max(0.0);
        let title = match self.page {
            Page::Settings => "Settings",
            Page::Shortcuts => "Shortcuts",
            Page::Changelog => "Changelog",
            Page::Themes(ThemeKind::Dark) => "Dark theme",
            Page::Themes(ThemeKind::Light) => "Light theme",
        };
        let body = match self.page {
            Page::Settings => self.settings_body(cx),
            Page::Shortcuts => self.shortcuts_body(cx),
            Page::Changelog => self.changelog_body(cx),
            Page::Themes(kind) => self.themes_body(kind, cx),
        };
        div()
            .id("settings-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .bg(c::SCRIM())
            .flex()
            .items_center()
            .justify_center()
            .capture_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close(window, cx)),
            )
            .child(
                div()
                    .id("settings-panel")
                    .debug_selector(|| "settings-panel".into())
                    .track_focus(&self.focus)
                    .w(rpx(width))
                    .max_h(rpx(height))
                    .flex()
                    .flex_col()
                    .rounded(rpx(RADIUS_PANEL))
                    .border_1()
                    .border_color(c::BORDER())
                    .bg(c::SURFACE_RAISED())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(rpx(SPACE_LG))
                            .p(rpx(SPACE_3XL))
                            .child(
                                div()
                                    .text_size(rpx(TEXT_TITLE))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(c::FG())
                                    .child(title),
                            )
                            .child(
                                self.button(
                                    "settings-close",
                                    if self.page == Page::Settings {
                                        "Close settings"
                                    } else {
                                        "Back to settings"
                                    },
                                    false,
                                    true,
                                )
                                .child(if self.page == Page::Settings {
                                    "Close"
                                } else {
                                    "Back"
                                })
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        if this.page == Page::Settings {
                                            this.close(window, cx);
                                        } else {
                                            this.show(Page::Settings, cx);
                                        }
                                    },
                                )),
                            ),
                    )
                    .child(
                        div()
                            .id("settings-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .px(rpx(SPACE_3XL))
                            .pb(rpx(SPACE_3XL))
                            .child(body),
                    )
                    .when_some(self.error.clone(), |panel, error| {
                        panel.child(
                            div()
                                .id("settings-error")
                                .role(gpui::Role::Alert)
                                .px(rpx(SPACE_3XL))
                                .py(rpx(SPACE_LG))
                                .text_size(rpx(TEXT_BODY))
                                .text_color(c::FORM_ERROR())
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .border_t_1()
                            .border_color(c::BORDER_SOFT())
                            .px(rpx(SPACE_3XL))
                            .py(rpx(SPACE_LG))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child("Tab to move · Esc to close · Changes save automatically"),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FocusFixture {
        panel: Entity<SettingsPanel>,
        behind: FocusHandle,
    }

    impl Render for FocusFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(
                    div()
                        .id("behind-settings")
                        .tab_index(0)
                        .track_focus(&self.behind)
                        .child("Behind"),
                )
                .when(self.panel.read(cx).is_open(), |root| {
                    root.child(self.panel.clone())
                })
        }
    }

    fn setup(cx: &mut App) {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(Store::default()));
        cx.set_global(ZoomState::new(1.0));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(ThemeState::new(
            false,
            DEFAULT_DARK_THEME.into(),
            DEFAULT_LIGHT_THEME.into(),
        ));
        c::sync_component_theme(cx);
    }

    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    #[test]
    fn appearance_uses_one_app_theme_and_preserves_mode_names() {
        let mut store = Store {
            theme_dark: Some("tokyonight-storm".into()),
            theme_light: Some("tokyonight-day".into()),
            theme_follow_system: true,
            ..Store::default()
        };
        assert_eq!(appearance(&store), Appearance::FollowSystem);
        assert_eq!(theme_for(Appearance::Dark, &store), "tokyonight-storm");
        assert_eq!(theme_for(Appearance::Light, &store), "tokyonight-day");
        store.theme_follow_system = false;
        store.theme = Some("tokyonight-day".into());
        assert_eq!(appearance(&store), Appearance::Light);
    }

    #[test]
    fn editing_inactive_palette_preserves_current_appearance() {
        let mut store = Store {
            theme: Some(DEFAULT_DARK_THEME.into()),
            theme_dark: Some(DEFAULT_DARK_THEME.into()),
            theme_light: Some(DEFAULT_LIGHT_THEME.into()),
            ..Store::default()
        };
        assert!(!edited_theme_is_active(
            &store,
            gpui::WindowAppearance::Dark,
            ThemeKind::Light
        ));
        save_named_theme(&mut store, ThemeKind::Light, "catppuccin-latte", false);
        assert_eq!(store.theme.as_deref(), Some(DEFAULT_DARK_THEME));
        assert_eq!(store.theme_light.as_deref(), Some("catppuccin-latte"));
        assert_eq!(appearance(&store), Appearance::Dark);

        assert!(edited_theme_is_active(
            &store,
            gpui::WindowAppearance::Dark,
            ThemeKind::Dark
        ));
        save_named_theme(&mut store, ThemeKind::Dark, "tokyonight", true);
        assert_eq!(store.theme.as_deref(), Some("tokyonight"));
        assert_eq!(store.theme_dark.as_deref(), Some("tokyonight"));
    }

    #[test]
    fn follow_system_edits_only_active_palette_without_changing_explicit_theme() {
        let mut store = Store {
            theme_follow_system: true,
            theme: Some(DEFAULT_DARK_THEME.into()),
            ..Store::default()
        };
        assert!(edited_theme_is_active(
            &store,
            gpui::WindowAppearance::Light,
            ThemeKind::Light
        ));
        assert!(!edited_theme_is_active(
            &store,
            gpui::WindowAppearance::Light,
            ThemeKind::Dark
        ));
        save_named_theme(&mut store, ThemeKind::Light, DEFAULT_LIGHT_THEME, true);
        save_named_theme(&mut store, ThemeKind::Dark, DEFAULT_DARK_THEME, false);
        assert_eq!(store.theme.as_deref(), Some(DEFAULT_DARK_THEME));
        assert_eq!(store.theme_light.as_deref(), Some(DEFAULT_LIGHT_THEME));
        assert_eq!(store.theme_dark.as_deref(), Some(DEFAULT_DARK_THEME));
        assert_eq!(appearance(&store), Appearance::FollowSystem);
        assert!(edited_theme_is_active(
            &store,
            gpui::WindowAppearance::Dark,
            ThemeKind::Dark
        ));
    }

    #[test]
    fn shortcut_reference_excludes_unhandled_grid_and_zen_actions() {
        let visible = handled_shortcuts();
        assert!(visible
            .iter()
            .any(|(_, def)| def.description == "New session"));
        assert!(visible
            .iter()
            .any(|(_, def)| def.description == "Select nth session"));
        assert!(!visible
            .iter()
            .any(|(_, def)| def.description == "Resize grid"));
        assert!(!visible
            .iter()
            .any(|(_, def)| def.description == "Toggle zen mode"));
        assert!(!visible
            .iter()
            .any(|(_, def)| def.description == "Move focus in grid"));
    }

    #[test]
    fn narrow_panel_stays_inside_320px_viewport() {
        assert_eq!(panel_width(320.0), 304.0);
        assert_eq!(panel_width(100.0), 84.0);
        assert_eq!(panel_width(1000.0), MODAL_W_XL);
    }

    #[gpui::test]
    fn shortcuts_escape_and_narrow_layout_restore_focus(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        let prior = cx.update(|window, cx| {
            let prior = cx.focus_handle();
            prior.focus(window, cx);
            panel.update(cx, |panel, cx| panel.open_shortcuts(window, cx));
            prior
        });
        cx.simulate_resize(gpui::size(gpui::px(320.0), gpui::px(200.0)));
        draw(cx);
        let bounds = cx.debug_bounds("settings-panel").expect("settings panel");
        assert!(f32::from(bounds.size.width) <= 320.0);
        assert!(f32::from(bounds.size.height) <= 200.0);
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(panel.read(cx).page, Page::Settings);
            assert!(panel.read(cx).is_open());
        });
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| {
            assert!(!panel.read(cx).is_open());
            assert!(prior.is_focused(window));
        });
    }

    #[gpui::test]
    fn tab_and_shift_tab_stay_inside_open_panel(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (fixture, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            FocusFixture {
                panel: cx.new(|cx| SettingsPanel::new(runtime, cx)),
                behind: cx.focus_handle().tab_stop(true),
            }
        });
        let (panel, behind) = fixture.read_with(cx, |fixture, _| {
            (fixture.panel.clone(), fixture.behind.clone())
        });
        cx.update(|window, cx| {
            behind.focus(window, cx);
            panel.update(cx, |panel, cx| panel.open_shortcuts(window, cx));
        });
        draw(cx);
        for chord in ["tab", "tab", "shift-tab", "shift-tab"] {
            cx.simulate_keystrokes(chord);
            draw(cx);
            cx.update(|window, cx| {
                assert!(!behind.is_focused(window), "{chord} escaped the overlay");
                assert!(panel.read(cx).focus.contains_focused(window, cx));
            });
        }
    }
}
