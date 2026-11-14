//! App-wide settings and shortcut reference, mounted in the main canvas.
use super::components;
use super::status::{self, StatusKind};
use super::{motion, rpx, tokens::*};
use crate::{
    entities::{
        shell_environment::{RefreshState, ShellEnvironment},
        upgrade_state::{restart_available, ChangelogState, UpgradeState},
    },
    keymap::{self, Scope, SHORTCUTS},
    runtime::Runtime,
    settings::SettingsState,
    theme::{self as c, ThemeState},
    zoom::{self, ZoomState, ZOOM_DEFAULT, ZOOM_STEP},
};
use gpui::{
    div, prelude::*, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    ScrollHandle, Subscription, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::Disableable;
use grove_core::{
    agent::Agent,
    storage::{AfterSessionStart, Store},
};
use grove_core::{app_theme, theme::ThemeKind};
use std::collections::HashSet;

const ENVIRONMENT_ACTION_W: f32 = 176.0;
const FOCUS_SCAN_LIMIT: usize = 128;
const THEME_ROW_H: f32 = 42.0;
const THEME_KIND_W: f32 = 36.0;
const THEME_SWATCH_RADIUS: f32 = 2.0;
const SHORTCUT_KEY_W: f32 = FIELD_LABEL_COL_W + SPACE_20 * 2.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Category {
    #[default]
    General,
    Agents,
    Terminal,
    Privacy,
    About,
}

impl Category {
    const ALL: [Self; 5] = [
        Self::General,
        Self::Agents,
        Self::Terminal,
        Self::Privacy,
        Self::About,
    ];

    fn icon(self) -> &'static str {
        match self {
            Self::General => "cog",
            Self::Agents => "brain",
            Self::Terminal => "terminal",
            Self::Privacy => "lock",
            Self::About => "status-info",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Agents => "Agents",
            Self::Terminal => "Terminal",
            Self::Privacy => "Privacy",
            Self::About => "About",
        }
    }
}

fn agent_display_label(agent: Agent) -> &'static str {
    match agent {
        Agent::Codex => "Codex",
        Agent::Claude => "Claude Code",
        Agent::OpenCode => "OpenCode",
        Agent::Terminal => "Terminal",
    }
}

fn runs_agents_in_terminal(store: &Store) -> bool {
    !store.native_agents_enabled
}

/// Settings navigation actions handled by the shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsPanelEvent {
    Closed,
    OpenHighlights { debug: bool },
    ConnectionsChanged(grove_core::provider::Provider),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Settings,
    Shortcuts,
    Changelog,
    Themes,
    Archived,
    Providers,
    Connections,
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

pub struct SettingsPanel {
    runtime: Entity<Runtime>,
    focus: FocusHandle,
    after_session_start_focus: [FocusHandle; 2],
    return_focus: Option<FocusHandle>,
    open: bool,
    page: Page,
    category: Category,
    nested_return_focus: Option<FocusHandle>,
    standalone_shortcuts: bool,
    error: Option<String>,
    scroll: ScrollHandle,
    archived_panel: Option<Entity<super::sidebar::projects::ProjectPanel>>,
    archived_subscription: Option<Subscription>,
    theme_search: Option<Entity<InputState>>,
    theme_search_subscription: Option<Subscription>,
    theme_filter: Option<ThemeKind>,
    theme_retry: Option<&'static str>,
    theme_focus: Vec<FocusHandle>,
    theme_anchors: Vec<gpui::ScrollAnchor>,
    show_theme_notices: bool,
    _settings_observer: Subscription,
    _upgrade_observer: Subscription,
    _environment_observer: Subscription,
    _provider_observer: Subscription,
    changelog_markdown: Vec<(String, String, Entity<gpui_component::text::TextViewState>)>,
    provider_accounts: [Entity<super::connections_panel::ConnectionsPanel>; 2],
    _account_observers: Vec<Subscription>,
    _account_subscriptions: Vec<Subscription>,
    managed_provider: Option<Agent>,
    provider_manage_focus: [FocusHandle; 3],
    provider_details: HashSet<&'static str>,
    connections: Entity<super::connections_panel::ConnectionsPanel>,
    connections_pending_focus: bool,
    _connections_subscription: Subscription,
}

impl EventEmitter<SettingsPanelEvent> for SettingsPanel {}

impl SettingsPanel {
    pub fn new(runtime: Entity<Runtime>, cx: &mut Context<Self>) -> Self {
        let upgrade = runtime.read(cx).upgrade.clone();
        let shell_environment = runtime.read(cx).shell_environment.clone();
        let provider_updates = runtime.read(cx).provider_updates.clone();
        let scroll = ScrollHandle::new();
        let connections =
            cx.new(|cx| super::connections_panel::ConnectionsPanel::new(runtime.clone(), cx));
        let connections_subscription = cx.subscribe(
            &connections,
            |_, _, event: &super::connections_panel::ConnectionChanged, cx| {
                cx.emit(SettingsPanelEvent::ConnectionsChanged(event.0));
            },
        );
        let provider_accounts = [
            grove_core::provider::Provider::Codex,
            grove_core::provider::Provider::Claude,
        ]
        .map(|provider| {
            cx.new(|cx| {
                super::connections_panel::ConnectionsPanel::account_panel(
                    runtime.clone(),
                    provider,
                    cx,
                )
            })
        });
        let account_observers = provider_accounts
            .iter()
            .map(|account| cx.observe(account, |_, _, cx| cx.notify()))
            .collect();
        let account_subscriptions = provider_accounts
            .iter()
            .map(|account| {
                cx.subscribe(
                    account,
                    |_, _, event: &super::connections_panel::ConnectionChanged, cx| {
                        cx.emit(SettingsPanelEvent::ConnectionsChanged(event.0));
                    },
                )
            })
            .collect();
        Self {
            runtime,
            provider_accounts,
            _account_observers: account_observers,
            _account_subscriptions: account_subscriptions,
            managed_provider: None,
            provider_manage_focus: [
                cx.focus_handle().tab_stop(true),
                cx.focus_handle().tab_stop(true),
                cx.focus_handle().tab_stop(true),
            ],
            connections,
            _connections_subscription: connections_subscription,
            connections_pending_focus: false,
            focus: cx.focus_handle(),
            after_session_start_focus: [
                cx.focus_handle().tab_stop(true),
                cx.focus_handle().tab_stop(true),
            ],
            return_focus: None,
            open: false,
            page: Page::Settings,
            category: Category::General,
            nested_return_focus: None,
            standalone_shortcuts: false,
            error: None,
            scroll: scroll.clone(),
            theme_anchors: app_theme::CATALOG
                .iter()
                .map(|_| gpui::ScrollAnchor::for_handle(scroll.clone()))
                .collect(),
            show_theme_notices: false,
            archived_panel: None,
            archived_subscription: None,
            theme_search: None,
            theme_search_subscription: None,
            theme_filter: None,
            theme_retry: None,
            theme_focus: app_theme::CATALOG
                .iter()
                .map(|_| cx.focus_handle().tab_stop(true))
                .collect(),
            _settings_observer: cx.observe_global::<SettingsState>(|_, cx| cx.notify()),
            _upgrade_observer: cx.observe(&upgrade, |_, _, cx| cx.notify()),
            _environment_observer: cx.observe(&shell_environment, |_, _, cx| cx.notify()),
            _provider_observer: cx.observe(&provider_updates, |_, _, cx| cx.notify()),
            changelog_markdown: Vec::new(),
            provider_details: HashSet::new(),
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
        self.standalone_shortcuts = false;
        self.nested_return_focus = None;
        self.error = None;
        self.scroll.set_offset(gpui::Point::default());
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn open_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open(window, cx);
        self.select_category(Category::About, cx);
    }

    pub(crate) fn open_provider_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open(window, cx);
        self.select_category(Category::Agents, cx);
        self.open_nested(Page::Providers, window, cx);
    }

    pub(crate) fn request_connections(
        &mut self,
        provider: grove_core::provider::Provider,
        cwd: String,
        args: Vec<String>,
        source_focus: Option<FocusHandle>,
        cx: &mut Context<Self>,
    ) {
        if !self.open {
            self.return_focus = source_focus;
        }
        self.open = true;
        self.category = Category::Agents;
        self.page = Page::Connections;
        self.connections_pending_focus = true;
        self.scroll.set_offset(gpui::Point::default());
        self.connections.update(cx, |panel, cx| {
            panel.select_context(provider, cwd, args, cx);
        });
        cx.notify();
    }

    pub fn open_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open(window, cx);
        self.standalone_shortcuts = true;
        self.show(Page::Shortcuts, cx);
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.page = Page::Settings;
        self.standalone_shortcuts = false;
        self.nested_return_focus = None;
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
                tracing::warn!(%error, "could not save settings");
                self.error = Some(format!("Could not save settings: {error}"));
                cx.notify();
                false
            }
        }
    }

    fn select_theme(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.save(cx, |store| {
            store.selected_theme = Some(id.to_string());
            if let Some(item) = app_theme::by_id(id) {
                store.appearance = Some(if item.kind == ThemeKind::Light {
                    grove_core::storage::AppearancePreference::Light
                } else {
                    grove_core::storage::AppearancePreference::Dark
                });
            }
        }) {
            let requested = app_theme::by_id(id).map_or("the theme", |item| item.label);
            let active = grove_core::theme::current();
            let current =
                app_theme::by_id(&active.name).map_or("The previous theme", |item| item.label);
            self.error = Some(format!(
                "Couldn’t save {requested}. {current} is still active. Try again."
            ));
            self.theme_retry = Some(id);
            return;
        }
        self.theme_retry = None;
        if let Some(item) = app_theme::by_id(id) {
            let query = self
                .theme_search
                .as_ref()
                .map(|input| input.read(cx).value().to_string())
                .unwrap_or_default();
            if !item.matches(&query, self.theme_filter) {
                self.theme_filter = None;
                if let Some(input) = &self.theme_search {
                    input.update(cx, |input, cx| input.set_value("", window, cx));
                }
            }
        }
        cx.update_global::<ThemeState, _>(|state, _| state.follow_system = false);
        ThemeState::set_by_name(cx, id);
        if let Some(index) = app_theme::CATALOG.iter().position(|item| item.id == id) {
            self.theme_focus[index].focus(window, cx);
        }
    }

    fn themes_body(&self, cx: &mut Context<Self>) -> gpui::Div {
        let query = self
            .theme_search
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        let active = grove_core::theme::current();
        let mut body = div().flex().flex_col().gap(rpx(SPACE_LG));
        if let Some(input) = &self.theme_search {
            body = body.child(
                super::components::project_field_well(false, false).child(
                    Input::new(input)
                        .file_drop_overlay(crate::views::components::file_drop_overlay())
                        .aria_label("Search themes")
                        .appearance(false)
                        .bordered(false)
                        .focus_bordered(false)
                        .p_0()
                        .text_size(rpx(FORM_VALUE_SIZE))
                        .text_color(c::FG()),
                ),
            );
        }
        let mut filters = Self::choices();
        for (id, label, kind) in [
            ("all", "All", None),
            ("light", "Light", Some(ThemeKind::Light)),
            ("dark", "Dark", Some(ThemeKind::Dark)),
        ] {
            filters = filters.child(
                self.choice(
                    format!("theme-filter-{id}"),
                    label,
                    self.theme_filter == kind,
                    true,
                )
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.theme_filter = kind;
                    this.scroll.set_offset(gpui::Point::default());
                    cx.notify();
                })),
            );
        }
        body = body.child(filters);
        if let Some(id) = self.theme_retry {
            body = body.child(
                div().flex().child(
                    self.button("theme-save-retry", "Retry saving theme", false, true)
                        .child(crate::icons::icon("refresh", ICON_MD, c::FG()))
                        .child("Retry")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_theme(id, window, cx);
                        })),
                ),
            );
        }
        let mut count = 0;
        let mut list = div()
            .id("theme-list")
            .role(gpui::Role::RadioGroup)
            .aria_label("Application theme")
            .flex()
            .flex_col()
            .w_full()
            .rounded(rpx(RADIUS_PANEL))
            .border_1()
            .border_color(c::BORDER())
            .overflow_hidden();
        for (index, item) in app_theme::CATALOG
            .iter()
            .enumerate()
            .filter(|(_, item)| item.matches(&query, self.theme_filter))
        {
            count += 1;
            let palette = item.terminal();
            let selected = active.name == item.id;
            let mut swatches = div().flex().flex_shrink_0().gap(rpx(SPACE_XS));
            for color in c::swatches(&palette) {
                swatches = swatches.child(
                    div()
                        .size(rpx(ICON_XS))
                        .rounded(rpx(THEME_SWATCH_RADIUS))
                        .border_1()
                        .border_color(c::BORDER_SOFT())
                        .bg(color),
                );
            }
            let kind = if item.kind == ThemeKind::Light {
                "Light"
            } else {
                "Dark"
            };
            list = list.child(
                self.choice(format!("theme-{}", item.id), item.label, selected, true)
                    .role(gpui::Role::RadioButton)
                    .aria_label(format!("{}, {} theme", item.label, kind))
                    .aria_toggled(if selected {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .track_focus(&self.theme_focus[index])
                    .anchor_scroll(Some(self.theme_anchors[index].clone()))
                    .w_full()
                    .h(rpx(THEME_ROW_H))
                    .rounded_none()
                    .when(count > 1, |row| {
                        row.border_t_1().border_color(c::BORDER_SOFT())
                    })
                    .gap(rpx(SPACE_LG))
                    .child(swatches)
                    .child(
                        div()
                            .id(format!("theme-name-{}", item.id))
                            .debug_selector(move || format!("theme-name-{}", item.id))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(item.label),
                    )
                    .child(
                        div()
                            .id(format!("theme-kind-{}", item.id))
                            .debug_selector(move || format!("theme-kind-{}", item.id))
                            .w(rpx(THEME_KIND_W))
                            .flex_shrink_0()
                            .flex()
                            .justify_end()
                            .items_center()
                            .whitespace_nowrap()
                            .text_color(c::FG_DIM())
                            .child(
                                div()
                                    .id(format!("theme-kind-text-{}", item.id))
                                    .debug_selector(move || format!("theme-kind-text-{}", item.id))
                                    .flex_shrink_0()
                                    .whitespace_nowrap()
                                    .child(kind),
                            ),
                    )
                    .child(div().w(rpx(ICON_MD)).flex_shrink_0().when(selected, |el| {
                        el.child(crate::icons::icon("check", ICON_MD, c::FG()))
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_theme(item.id, window, cx);
                    })),
            );
        }
        if count > 0 {
            body = body.child(list);
        }
        if count == 0 {
            body = body.child(
                div()
                    .py(rpx(SPACE_20))
                    .text_color(c::FG_DIM())
                    .child("No themes found. Try another search or choose All."),
            );
        }
        body = body.child(
            Self::disclosure(
                "theme-notices",
                "Theme palette acknowledgements",
                self.show_theme_notices,
            )
            .child(crate::icons::icon(
                if self.show_theme_notices {
                    "chev-down"
                } else {
                    "chev-right"
                },
                ICON_SM,
                c::FG_DIM(),
            ))
            .child("Theme palette acknowledgements")
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_theme_notices = !this.show_theme_notices;
                cx.notify();
            })),
        );
        if self.show_theme_notices {
            body = body.child(super::components::readonly_value(
                "theme-notices-text",
                include_str!("../../assets/themes/NOTICES.txt").to_string(),
            ));
        }
        body
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
            crate::platform::text_preferences::sync_component_size(cx);
            cx.refresh_windows();
        }
    }

    fn set_backend(&mut self, tmux: bool, cx: &mut Context<Self>) {
        if tmux && !grove_core::tmux::available() {
            self.error = Some("tmux is not installed; built-in sessions remain selected.".into());
            cx.notify();
            return;
        }
        if self.save(cx, |store| store.tmux_enabled = Some(tmux)) && tmux {
            self.runtime.update(cx, Runtime::discover_tmux_sessions);
        }
    }

    fn set_yolo(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.save(cx, |store| {
            store.yolo_enabled = enabled;
        });
    }

    fn set_chrome(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.save(cx, |store| store.chrome_enabled = Some(enabled));
    }

    fn set_agents_in_terminal(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.save(cx, |store| store.native_agents_enabled = !enabled);
    }

    fn set_telemetry(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.save(cx, |store| store.telemetry_enabled = Some(enabled)) {
            crate::telemetry::set_enabled(enabled);
        }
    }

    fn set_after_session_start(&mut self, preference: AfterSessionStart, cx: &mut Context<Self>) {
        self.save(cx, |store| store.after_session_start = preference);
    }

    fn set_reduce_motion(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.save(cx, |store| store.reduce_motion = Some(enabled)) {
            cx.set_reduce_motion(enabled);
        }
    }

    fn select_category(&mut self, category: Category, cx: &mut Context<Self>) {
        self.category = category;
        self.nested_return_focus = None;
        self.standalone_shortcuts = false;
        self.show(Page::Settings, cx);
    }

    fn open_nested(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.standalone_shortcuts = false;
        self.nested_return_focus = window.focused(cx);
        if page == Page::Archived {
            let runtime = self.runtime.read(cx);
            let (service, registry) = (runtime.projects.clone(), runtime.registry.clone());
            let panel = cx.new(|cx| {
                super::sidebar::projects::ProjectPanel::new(
                    super::sidebar::projects::Page::Archived,
                    service,
                    registry,
                    window,
                    cx,
                )
                .in_settings()
            });
            self.archived_subscription =
                Some(
                    cx.subscribe_in(&panel, window, |this, _, event, window, cx| {
                        use super::sidebar::projects::ProjectPanelEvent;
                        if matches!(event, ProjectPanelEvent::Closed) {
                            this.back(window, cx);
                        }
                    }),
                );
            self.archived_panel = Some(panel);
        }
        if page == Page::Providers {
            for account in self.provider_accounts.clone() {
                account.update(cx, |panel, cx| panel.refresh(false, cx));
            }
        }
        if page == Page::Connections {
            self.connections
                .update(cx, |panel, cx| panel.refresh(false, cx));
        }
        if page == Page::Themes {
            if self.theme_search.is_none() {
                let input =
                    cx.new(|cx| InputState::new(window, cx).placeholder("e.g. Nord or Rosé Pine"));
                self.theme_search_subscription =
                    Some(cx.subscribe(&input, |this, _, event, cx| {
                        if matches!(event, InputEvent::Change) {
                            this.scroll.set_offset(gpui::Point::default());
                            cx.notify();
                        }
                    }));
                self.theme_search = Some(input);
            }
            self.theme_filter = None;
            self.theme_retry = None;
        }
        self.show(page, cx);
        if page == Page::Themes {
            if let Some(input) = &self.theme_search {
                input.read(cx).focus_handle(cx).focus(window, cx);
            }
        } else if page == Page::Archived {
            if let Some(panel) = &self.archived_panel {
                panel.focus_handle(cx).focus(window, cx);
            }
        } else {
            self.focus.focus(window, cx);
        }
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.archived_panel = None;
        self.archived_subscription = None;
        self.standalone_shortcuts = false;
        self.show(Page::Settings, cx);
        if let Some(focus) = self.nested_return_focus.take() {
            focus.focus(window, cx);
        }
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
            "escape" if self.page == Page::Shortcuts && self.standalone_shortcuts => {
                self.close(window, cx);
            }
            "escape" if self.page != Page::Settings => self.back(window, cx),
            "escape" => self.close(window, cx),
            "up" | "down" if self.page == Page::Themes => {
                let query = self
                    .theme_search
                    .as_ref()
                    .map(|input| input.read(cx).value().to_string())
                    .unwrap_or_default();
                let indices: Vec<_> = app_theme::CATALOG
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| item.matches(&query, self.theme_filter))
                    .map(|(i, _)| i)
                    .collect();
                if !indices.is_empty() {
                    let current = indices
                        .iter()
                        .position(|i| self.theme_focus[*i].is_focused(window));
                    let next = match (current, event.keystroke.key.as_str()) {
                        (Some(i), "up") => i.saturating_sub(1),
                        (Some(i), _) => (i + 1).min(indices.len() - 1),
                        (None, "up") => indices.len() - 1,
                        _ => 0,
                    };
                    self.theme_focus[indices[next]].focus(window, cx);
                    self.theme_anchors[indices[next]].scroll_to(window, cx);
                }
            }
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
                if self.page == Page::Themes {
                    if let Some(index) = self
                        .theme_focus
                        .iter()
                        .position(|focus| focus.is_focused(window))
                    {
                        self.theme_anchors[index].scroll_to(window, cx);
                    }
                }
            }
            _ => return,
        }
        window.prevent_default();
        cx.stop_propagation();
    }

    fn toggle_provider_details(&mut self, agent: Agent, cx: &mut Context<Self>) {
        if !self.provider_details.insert(agent.label()) {
            self.provider_details.remove(agent.label());
        }
        cx.notify();
    }

    fn toggle_managed_provider(
        &mut self,
        agent: Agent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .provider_accounts
            .iter()
            .any(|panel| panel.read(cx).busy())
        {
            return;
        }
        // Disclosure is immediate input/focus layout feedback; focus moves with the current layout.
        self.managed_provider = if self.managed_provider == Some(agent) {
            None
        } else {
            Some(agent)
        };
        self.provider_manage_focus[match agent {
            Agent::Codex => 0,
            Agent::Claude => 1,
            _ => 2,
        }]
        .focus(window, cx);
        cx.notify();
    }

    fn providers_body(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let mut body = div().w_full().min_w_0().flex().flex_col();
        let mut version_body = self
            .managed_provider
            .map(|agent| self.provider_version_body(agent, window, cx));
        let updates = self.runtime.read(cx).provider_updates.read(cx);
        let busy = self
            .provider_accounts
            .iter()
            .any(|panel| panel.read(cx).busy());
        for (index, agent) in [Agent::Codex, Agent::Claude, Agent::OpenCode]
            .into_iter()
            .enumerate()
        {
            if agent == Agent::OpenCode
                && !updates.rows().iter().any(|row| {
                    row.status.agent == agent
                        && (row.status.executable.is_some()
                            || row.status.update_available()
                            || row.error.is_some())
                })
            {
                continue;
            }
            let account = self.provider_accounts.get(index).cloned();
            let (kind, label) = account
                .as_ref()
                .map_or((StatusKind::Idle, "Version only"), |account| {
                    account.read(cx).summary()
                });
            let can_login = account
                .as_ref()
                .is_some_and(|account| account.read(cx).can_login());
            let expanded = self.managed_provider == Some(agent);
            let id = format!("settings-provider-{}", agent.label());
            let update = updates.rows().iter().find(|row| row.status.agent == agent);
            let mut actions = components::form_actions()
                .w_auto()
                .max_w_full()
                .flex_shrink_1()
                .id(format!("{id}-actions"))
                .debug_selector({
                    let id = id.clone();
                    move || format!("{id}-actions")
                });
            if can_login {
                actions = actions.child(
                    components::form_action(format!("{id}-login"), "Sign in", false, window, cx)
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/external-link.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .disabled(busy)
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                if !busy
                                    && matches!(event.keystroke.key.as_str(), "enter" | "space")
                                    && !event.is_held
                                    && !event.keystroke.modifiers.control
                                    && !event.keystroke.modifiers.alt
                                    && !event.keystroke.modifiers.platform
                                {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    this.managed_provider = Some(agent);
                                    this.provider_accounts[index]
                                        .update(cx, |panel, cx| panel.login(window, cx));
                                    cx.notify();
                                }
                            },
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.managed_provider = Some(agent);
                            this.provider_accounts[index]
                                .update(cx, |panel, cx| panel.login(window, cx));
                            cx.notify();
                        })),
                );
            }
            if let Some(row) = update
                .filter(|row| row.updating || row.status.update_available() || row.error.is_some())
            {
                let enabled = !busy
                    && !updates.checking()
                    && !updates.rows().iter().any(|row| row.updating)
                    && row.status.can_update();
                actions = actions.child(
                    components::form_action(
                        format!("{id}-update"),
                        if row.updating {
                            "Updating…"
                        } else if row.error.is_some() {
                            "Retry"
                        } else {
                            "Update"
                        },
                        false,
                        window,
                        cx,
                    )
                    .debug_selector({
                        let id = id.clone();
                        move || format!("{id}-update")
                    })
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/refresh.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .disabled(!enabled)
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if enabled
                                && matches!(event.keystroke.key.as_str(), "enter" | "space")
                                && !event.is_held
                                && !event.keystroke.modifiers.control
                                && !event.keystroke.modifiers.alt
                                && !event.keystroke.modifiers.platform
                            {
                                window.prevent_default();
                                cx.stop_propagation();
                                this.runtime
                                    .read(cx)
                                    .provider_updates
                                    .clone()
                                    .update(cx, |updates, cx| updates.start_update(agent, cx));
                            }
                        },
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.runtime
                            .read(cx)
                            .provider_updates
                            .clone()
                            .update(cx, |updates, cx| updates.start_update(agent, cx));
                    })),
                );
            }
            actions = actions.child(
                components::form_action(
                    format!("{id}-manage"),
                    if expanded { "Less" } else { "Manage" },
                    false,
                    window,
                    cx,
                )
                .debug_selector({
                    let id = id.clone();
                    move || format!("{id}-manage")
                })
                .with_focus_handle(self.provider_manage_focus[index].clone())
                .aria_expanded(expanded)
                .icon(
                    gpui_component::Icon::default()
                        .path(if expanded {
                            "icons/chev-down.svg"
                        } else {
                            "icons/chev-right.svg"
                        })
                        .size(rpx(ICON_SM)),
                )
                .disabled(busy)
                .on_key_down(
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if !busy
                            && matches!(event.keystroke.key.as_str(), "enter" | "space")
                            && !event.is_held
                            && !event.keystroke.modifiers.control
                            && !event.keystroke.modifiers.alt
                            && !event.keystroke.modifiers.platform
                        {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.toggle_managed_provider(agent, window, cx);
                        }
                    }),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_managed_provider(agent, window, cx);
                })),
            );
            let mut card = div()
                .id(id.clone())
                .w_full()
                .min_w_0()
                .border_t_1()
                .border_color(c::BORDER_SOFT())
                .py(rpx(SPACE_2XL))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .child(
                            div()
                                .flex_1()
                                .min_w(rpx(FIELD_LABEL_COL_W))
                                .flex()
                                .gap(rpx(SPACE_SM))
                                .child(crate::icons::icon(agent.icon_name(), ICON_SM, c::FG()))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(rpx(SPACE_XS))
                                        .child(
                                            div()
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(agent_display_label(agent)),
                                        )
                                        .child(status::label(
                                            format!("{id}-account-status"),
                                            kind,
                                            label,
                                            ICON_SM,
                                            cx,
                                        )),
                                ),
                        )
                        .child(actions),
                );
            if expanded {
                card = card.child(
                    div()
                        .id(format!("{id}-management"))
                        .debug_selector({
                            let id = id.clone();
                            move || format!("{id}-management")
                        })
                        .w_full()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(rpx(SPACE_2XL))
                        .pt(rpx(SPACE_2XL))
                        .when_some(account, gpui::ParentElement::child)
                        .when_some(version_body.take(), gpui::ParentElement::child),
                );
            } else if let Some(error) = update.and_then(|row| row.error.as_ref()) {
                card = card.child(status::label(
                    format!("{id}-update-error"),
                    StatusKind::Failed,
                    error.clone(),
                    ICON_SM,
                    cx,
                ));
            }
            body = body.child(card);
        }
        body.border_b_1()
            .border_color(c::BORDER_SOFT())
            .text_size(rpx(TEXT_BODY))
    }

    fn provider_version_body(
        &self,
        selected: Agent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let updates = self.runtime.read(cx).provider_updates.clone();
        let state = updates.read(cx);
        let row = state.rows().iter().find(|row| row.status.agent == selected);
        let busy = state.checking()
            || state.rows().iter().any(|row| row.updating)
            || self
                .provider_accounts
                .iter()
                .any(|account| account.read(cx).busy());
        let id = format!("settings-provider-{}", selected.label());
        let mut actions = components::form_actions()
            .w_auto()
            .max_w_full()
            .flex_shrink_1()
            .id(format!("{id}-version-actions"))
            .debug_selector({
                let id = id.clone();
                move || format!("{id}-version-actions")
            })
            .child(
                components::form_action(
                    format!("{id}-check"),
                    if state.checking() {
                        "Checking…"
                    } else {
                        "Check for updates"
                    },
                    false,
                    window,
                    cx,
                )
                .icon(
                    gpui_component::Icon::default()
                        .path("icons/refresh.svg")
                        .size(rpx(ICON_SM)),
                )
                .disabled(busy)
                .on_key_down(
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if !busy
                            && matches!(event.keystroke.key.as_str(), "enter" | "space")
                            && !event.is_held
                            && !event.keystroke.modifiers.control
                            && !event.keystroke.modifiers.alt
                            && !event.keystroke.modifiers.platform
                        {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.runtime.read(cx).provider_updates.clone().update(
                                cx,
                                crate::entities::provider_updates::ProviderUpdates::check,
                            );
                        }
                    }),
                )
                .when(!busy, |el| {
                    el.on_click(cx.listener(|this, _, _, cx| {
                        this.runtime.read(cx).provider_updates.clone().update(
                            cx,
                            crate::entities::provider_updates::ProviderUpdates::check,
                        );
                    }))
                }),
            );
        let versions =
            row.map_or_else(
                || "Version not checked".to_string(),
                |row| {
                    let installed = row.status.installed.as_deref().unwrap_or(
                        if row.status.executable.is_some() {
                            "Version unavailable"
                        } else {
                            "Not installed"
                        },
                    );
                    if row.status.update_available()
                        && !matches!(
                            row.status.installer,
                            grove_core::provider_updates::Installer::ClaudeNative
                        )
                    {
                        format!(
                            "{installed} → {}",
                            row.status.latest.as_deref().unwrap_or_default()
                        )
                    } else {
                        installed.into()
                    }
                },
            );
        if let Some(row) = row {
            if matches!(
                row.status.installer,
                grove_core::provider_updates::Installer::ClaudeNative
            ) && !row.status.update_available()
            {
                let enabled = !busy && row.status.can_update();
                actions = actions.child(
                    components::form_action(
                        format!("{id}-native-update"),
                        "Update",
                        false,
                        window,
                        cx,
                    )
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/refresh.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .disabled(!enabled)
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if enabled
                                && matches!(event.keystroke.key.as_str(), "enter" | "space")
                                && !event.is_held
                                && !event.keystroke.modifiers.control
                                && !event.keystroke.modifiers.alt
                                && !event.keystroke.modifiers.platform
                            {
                                window.prevent_default();
                                cx.stop_propagation();
                                this.runtime
                                    .read(cx)
                                    .provider_updates
                                    .clone()
                                    .update(cx, |updates, cx| updates.start_update(selected, cx));
                            }
                        },
                    ))
                    .when(enabled, |el| {
                        el.on_click(cx.listener(move |this, _, _, cx| {
                            this.runtime
                                .read(cx)
                                .provider_updates
                                .clone()
                                .update(cx, |updates, cx| updates.start_update(selected, cx));
                        }))
                    }),
                );
            }
        }
        let mut body = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .pt(rpx(SPACE_2XL))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(
                        div()
                            .flex_1()
                            .min_w(rpx(FIELD_LABEL_COL_W))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_SM))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Version · This machine"),
                            )
                            .child(
                                components::readonly_value(format!("{id}-versions"), versions)
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM()),
                            ),
                    )
                    .child(actions),
            );
        if let Some(row) = row {
            if let Some(error) = &row.error {
                let expanded = self.provider_details.contains(selected.label());
                body = body.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(rpx(SPACE_SM))
                        .child(status::label(
                            format!("{id}-error-summary"),
                            StatusKind::Failed,
                            if row.status.error.is_some() {
                                "Could not check."
                            } else {
                                "Update failed."
                            },
                            ICON_SM,
                            cx,
                        ))
                        .child(
                            Self::disclosure(
                                format!("{id}-details"),
                                format!("Error details for {}", agent_display_label(selected)),
                                expanded,
                            )
                            .debug_selector({
                                let id = id.clone();
                                move || format!("{id}-details")
                            })
                            .child(crate::icons::icon(
                                if expanded { "chev-down" } else { "chev-right" },
                                ICON_SM,
                                c::FG_DIM(),
                            ))
                            .child("Details")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_provider_details(selected, cx);
                            }))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                        window.prevent_default();
                                        cx.stop_propagation();
                                        this.toggle_provider_details(selected, cx);
                                    }
                                },
                            )),
                        ),
                );
                if expanded {
                    body = body.child(
                        components::readonly_value(format!("{id}-error"), error.clone())
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM()),
                    );
                }
            } else if row.updated
                || row.update_verified
                || (!matches!(
                    row.status.installer,
                    grove_core::provider_updates::Installer::ClaudeNative
                ) && !row.checking
                    && row.status.installed.is_some()
                    && row.status.latest.is_some()
                    && !row.status.update_available())
            {
                body = body.child(status::label(
                    format!("{id}-current"),
                    StatusKind::Success,
                    if row.updated { "Updated" } else { "Up to date" },
                    ICON_SM,
                    cx,
                ));
            } else if matches!(
                row.status.installer,
                grove_core::provider_updates::Installer::ClaudeNative
            ) {
                body = body.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("The native installer checks its configured update channel."),
                );
            } else if row.status.update_available() && !row.status.can_update() {
                body = body.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("Update unavailable"),
                );
            }
        }
        body
    }

    fn disclosure(
        id: impl Into<gpui::ElementId>,
        label: impl Into<gpui::SharedString>,
        expanded: bool,
    ) -> gpui::Stateful<gpui::Div> {
        super::components::focusable_inline_disclosure(id, label)
            .tab_index(0)
            .aria_expanded(expanded)
            .w_auto()
            .self_start()
            .h(rpx(CONTROL_H))
            .px(rpx(SPACE_LG))
            .gap(rpx(SPACE_SM))
            .text_size(rpx(TEXT_SMALL))
            .focus_visible(|style| style.bg(c::BG_HOVER()))
    }

    fn button(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<gpui::SharedString>,
        selected: bool,
        enabled: bool,
    ) -> gpui::Stateful<gpui::Div> {
        super::components::settings_control(id, label, selected, enabled)
    }

    fn choice(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<gpui::SharedString>,
        selected: bool,
        enabled: bool,
    ) -> gpui::Stateful<gpui::Div> {
        self.button(id, label, selected, enabled)
            .border_0()
            .bg(if selected { c::BG_HOVER() } else { c::BG() })
    }

    fn choices() -> gpui::Div {
        super::components::preference_choices()
    }

    fn section(label: &'static str) -> gpui::Div {
        div()
            .pt(rpx(SPACE_20))
            .pb(rpx(SPACE_SM))
            .text_size(rpx(TEXT_SMALL))
            .font_weight(FontWeight::MEDIUM)
            .text_color(c::FG_DIM())
            .child(label)
    }

    fn setting_row(
        label: impl IntoElement,
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
            .gap(rpx(SPACE_3XL))
            .py(rpx(SPACE_XL))
            .px(rpx(SPACE_2XL))
            .border_b_1()
            .border_color(c::BORDER_SOFT())
            .child(
                div()
                    .min_w(rpx(FIELD_LABEL_COL_W))
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .text_size(rpx(TEXT_BODY))
                            .font_weight(FontWeight::MEDIUM)
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

    fn settings_body(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let store = &cx.global::<SettingsState>().store;
        let active_theme = grove_core::theme::current();
        let theme_label = app_theme::by_id(&active_theme.name).map_or("Grove", |item| item.label);
        let zoom = cx.global::<ZoomState>().zoom;
        let tmux_on = store.tmux_enabled.unwrap_or(false);
        let tmux_available = grove_core::tmux::available();
        let skip = store.yolo_enabled;
        let chrome = store.chrome_enabled.unwrap_or(false);
        let terminal_agents = cfg!(windows) || runs_agents_in_terminal(store);
        let telemetry = SettingsState::telemetry_enabled(store);
        let reduce_motion = store.reduce_motion.unwrap_or(false);
        let after_session_start = store.after_session_start;
        let mut body = div().flex().flex_col();

        let mut theme_swatches = div().flex().gap(rpx(SPACE_XS));
        for color in c::swatches(&active_theme) {
            theme_swatches = theme_swatches.child(
                div()
                    .size(rpx(ICON_SM))
                    .rounded(rpx(THEME_SWATCH_RADIUS))
                    .bg(color),
            );
        }
        let themes = self
            .button(
                "settings-theme",
                format!("Choose theme, current theme: {theme_label}"),
                false,
                true,
            )
            .child(crate::icons::icon("contrast", ICON_SM, c::FG_DIM()))
            .child(theme_swatches)
            .child(theme_label)
            .on_click(
                cx.listener(|this, _, window, cx| this.open_nested(Page::Themes, window, cx)),
            );
        let zoom_control = Self::choices()
            .child(
                self.choice(
                    "settings-zoom-out",
                    "Zoom out",
                    false,
                    zoom > zoom::ZOOM_MIN,
                )
                .child(crate::icons::icon("minus", ICON_MD, c::FG()))
                .when(zoom > zoom::ZOOM_MIN, |el| {
                    el.on_click(cx.listener(|this, _, _, cx| this.set_zoom(-ZOOM_STEP, cx)))
                }),
            )
            .child(
                self.choice("settings-zoom-reset", "Reset zoom", false, true)
                    .child(format!("{:.0}%", zoom * 100.0))
                    .on_click(cx.listener(|this, _, _, cx| this.set_zoom(0.0, cx))),
            )
            .child(
                self.choice("settings-zoom-in", "Zoom in", false, zoom < zoom::ZOOM_MAX)
                    .child(crate::icons::icon("plus", ICON_MD, c::FG()))
                    .when(zoom < zoom::ZOOM_MAX, |el| {
                        el.on_click(cx.listener(|this, _, _, cx| this.set_zoom(ZOOM_STEP, cx)))
                    }),
            );
        let reduce_motion_control = super::components::boolean_choices(
            "settings-reduce-motion",
            "Reduce motion",
            reduce_motion,
            true,
            cx,
            |this, enabled, _, cx| this.set_reduce_motion(enabled, cx),
        );
        let mut after_session_start_control = Self::choices();
        for (index, (id, label, preference)) in [
            (
                "stay",
                "Stay on Dashboard",
                AfterSessionStart::StayOnDashboard,
            ),
            ("open", "Open session", AfterSessionStart::OpenSession),
        ]
        .into_iter()
        .enumerate()
        {
            after_session_start_control = after_session_start_control.child(
                self.choice(
                    format!("settings-after-session-start-{id}"),
                    label,
                    after_session_start == preference,
                    true,
                )
                .debug_selector(move || format!("settings-after-session-start-{id}"))
                .track_focus(&self.after_session_start_focus[index])
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.after_session_start_focus[index].focus(window, cx);
                    this.set_after_session_start(preference, cx);
                }))
                .on_key_down(cx.listener(
                    move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if this.after_session_start_focus[index].is_focused(window)
                            && matches!(event.keystroke.key.as_str(), "enter" | "space")
                            && !event.is_held
                            && !event.keystroke.modifiers.control
                            && !event.keystroke.modifiers.alt
                            && !event.keystroke.modifiers.platform
                        {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.set_after_session_start(preference, cx);
                        }
                    },
                )),
            );
        }
        body = body.when(self.category == Category::General, |body| {
            body.child(
                div()
                    .flex()
                    .flex_col()
                    .child(Self::section("Appearance"))
                    .child(Self::setting_row(
                        "Theme",
                        "Choose a theme for the entire app",
                        themes,
                    ))
                    .child(Self::setting_row(
                        "App size",
                        "Scale the interface",
                        zoom_control,
                    ))
                    .child(Self::setting_row(
                        "Reduce motion",
                        "Skip decorative interface transitions",
                        reduce_motion_control,
                    ))
                    .child(Self::section("Sessions"))
                    .child(Self::setting_row(
                        "After starting a session",
                        "Choose where you go after sending your first message",
                        after_session_start_control,
                    )),
            )
        });

        let backend = Self::choices()
            .child(
                self.choice(
                    "settings-backend-native",
                    "Built-in backend",
                    !tmux_on,
                    true,
                )
                .child("Built-in")
                .on_click(cx.listener(|this, _, _, cx| this.set_backend(false, cx))),
            )
            .child(
                self.choice(
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
        let permissions = super::components::boolean_choices(
            "settings-yolo",
            "Yolo mode",
            skip,
            true,
            cx,
            |this, enabled, _, cx| this.set_yolo(enabled, cx),
        );
        let mut browser = Self::choices();
        let mut terminal_agents_control = Self::choices();
        for (enabled, label) in [(false, "Off"), (true, "On")] {
            browser = browser.child(
                self.choice(
                    format!("settings-chrome-{enabled}"),
                    format!("Claude in Chrome {label}"),
                    chrome == enabled,
                    true,
                )
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.set_chrome(enabled, cx))),
            );
            terminal_agents_control = terminal_agents_control.child(
                self.choice(
                    format!("settings-terminal-agents-{enabled}"),
                    format!("Run agents in terminal {label}"),
                    terminal_agents == enabled,
                    !cfg!(windows),
                )
                .child(label)
                .when(!cfg!(windows), |el| {
                    el.on_click(
                        cx.listener(move |this, _, _, cx| this.set_agents_in_terminal(enabled, cx)),
                    )
                }),
            );
        }
        let environment = self.runtime.read(cx).shell_environment.clone();
        let environment_state = environment.read(cx).state();
        let refreshing = environment_state == RefreshState::Refreshing;
        let refresh_enabled = !cfg!(windows) && !refreshing;
        let confirmation = environment.read(cx).confirmation();
        let refresh_label = match environment_state {
            RefreshState::Refreshing => "Refreshing…",
            RefreshState::Refreshed => "Refreshed",
            RefreshState::Error => "Retry refresh",
            RefreshState::Idle => "Refresh now",
        };
        let environment_control = div()
            .flex()
            .flex_col()
            .w(rpx(ENVIRONMENT_ACTION_W))
            .max_w_full()
            .min_w_0()
            .items_end()
            .gap(rpx(SPACE_SM))
            .child(
                self.button(
                    "settings-shell-environment",
                    "Refresh shell environment",
                    false,
                    refresh_enabled,
                )
                .debug_selector(|| "settings-shell-environment".into())
                .w_full()
                .justify_start()
                .child(if refreshing {
                    status::mark(
                        "settings-environment-working",
                        StatusKind::Working,
                        ICON_SM,
                        cx,
                    )
                } else if let Some((generation, started)) = confirmation {
                    status::drawn_check(
                        "settings-environment-confirmed",
                        generation,
                        started,
                        ICON_SM,
                        cx,
                    )
                } else {
                    crate::icons::icon("refresh", ICON_SM, c::FG_DIM()).into_any_element()
                })
                .child(refresh_label)
                .when(refresh_enabled, |el| {
                    el.on_click(cx.listener(|this, _, _, cx| {
                        this.runtime
                            .read(cx)
                            .shell_environment
                            .clone()
                            .update(cx, ShellEnvironment::refresh);
                    }))
                }),
            )
            .when(environment_state == RefreshState::Error, |el| {
                el.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("Refresh failed. Previous environment retained."),
                )
            });
        let terminal_body = div().flex().flex_col()
            .child(Self::section("New terminal sessions"))
            .child(Self::setting_row("Terminal backend", if tmux_available { "Existing sessions keep their current backend." } else { "Tmux is not installed. Built-in remains available." }, backend))
            .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child("Tmux keeps terminal processes running after Grove exits. Built-in terminals end when Grove exits. Grove’s conversation view has separate recovery."))
            .child(Self::section("Environment"))
            .child(Self::setting_row("Shell environment", if cfg!(windows) { "Shell environment refresh is unavailable on Windows." } else { "Refreshes every 10 minutes. Changes apply to new sessions." }, environment_control));

        body = body.when(self.category == Category::Agents, |body| body.child(
            div().flex().flex_col()
                .child(Self::section("New sessions"))
                .child(Self::setting_row("Run agents in terminal", "Use the Codex and Claude Code command-line interfaces instead of Grove’s conversation view. Applies to new sessions.", terminal_agents_control))
                .when(cfg!(windows), |body| body.child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child("Windows supports terminal agents only.")))
                .child(Self::section("Provider tools"))
                .child(Self::setting_row("Providers", "Manage accounts and provider updates.",
                    self.button("settings-providers", "Providers", false, true)
                        .child(crate::icons::icon("arrow-right", ICON_SM, c::FG_DIM()))
                        .child("Providers")
                        .on_click(cx.listener(|this, _, window, cx| this.open_nested(Page::Providers, window, cx)))))
                .child(Self::section("Agent access"))
                .child(Self::setting_row("Yolo mode", "Launch new CLI and native sessions with bypass permissions. When off, agents use their defaults. OpenCode uses its own permissions.", permissions))
                .child(Self::section("Browser access"))
                .child(Self::setting_row("Claude in Chrome", "Let Claude read and control Chrome tabs. Applies to new Claude sessions.", browser))
        ));
        body = body.when(self.category == Category::Terminal, |body| {
            body.child(terminal_body)
        });

        let telemetry_button = super::components::boolean_choices(
            "settings-telemetry",
            "Share anonymous usage data",
            telemetry,
            true,
            cx,
            |this, enabled, _, cx| this.set_telemetry(enabled, cx),
        );
        body = body.when(self.category == Category::Privacy, |body| body.child(
            div().flex().flex_col()
                .child(Self::section("Usage & diagnostics"))
                .when(crate::telemetry::disabled_by_environment(), |body| body.child(
                    div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child(format!(
                        "Collection is disabled by GROVE_TELEMETRY. Your saved preference is {}.", if telemetry { "On" } else { "Off" }
                    ))
                ))
                .child(Self::setting_row("Share anonymous usage data", "Help improve Grove with usage events and diagnostics.", telemetry_button))
                .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child("Usage events include app and session activity, version, OS, diagnostic error types, and scrubbed panic locations. Project names, file paths, Git data, prompts, and conversation or terminal content are excluded. GROVE_TELEMETRY=off also disables collection."))
        ));

        let upgrade = self.runtime.read(cx).upgrade.clone();
        let upgrade = upgrade.read(cx);
        let (status, checking) = match upgrade.state() {
            UpgradeState::Idle => ("Not checked yet".to_string(), false),
            UpgradeState::Checking => ("Checking…".to_string(), true),
            UpgradeState::UpToDate => ("Up to date".to_string(), false),
            UpgradeState::Skipped(release) => (format!("Version {} skipped", release.tag), false),
            UpgradeState::Available(release) => {
                (format!("Update available: {}", release.tag), false)
            }
            UpgradeState::Error(error) => (format!("Check failed: {error}"), false),
            UpgradeState::Updating(_) => ("Updating…".to_string(), true),
            UpgradeState::Updated => ("Update installed".to_string(), false),
            UpgradeState::UpdateFailed(error) => (format!("Update failed: {error}"), false),
        };
        let status_kind = match upgrade.state() {
            UpgradeState::Idle => StatusKind::Unknown,
            UpgradeState::Skipped(_) => StatusKind::Stopped,
            UpgradeState::Checking | UpgradeState::Updating(_) => StatusKind::Working,
            UpgradeState::UpToDate | UpgradeState::Updated => StatusKind::Success,
            UpgradeState::Available(_) => StatusKind::Warning,
            UpgradeState::Error(_) | UpgradeState::UpdateFailed(_) => StatusKind::Failed,
        };
        let can_update = upgrade.can_update();
        let has_update = upgrade.available().is_some();
        let can_restart = restart_available(upgrade.state());
        let mut actions = div().flex().flex_wrap().gap(rpx(SPACE_SM));
        if matches!(upgrade.state(), UpgradeState::Skipped(_)) {
            actions = actions.child(
                self.button(
                    "settings-update-reconsider",
                    "Reconsider skipped version",
                    false,
                    true,
                )
                .child(crate::icons::icon("restore", ICON_SM, c::FG_DIM()))
                .child("Reconsider version")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.runtime
                        .read(cx)
                        .upgrade
                        .clone()
                        .update(cx, crate::entities::upgrade::Upgrade::reconsider);
                })),
            );
        }
        if can_restart {
            actions = actions.child(
                super::components::form_action(
                    "settings-update-restart",
                    "Restart app",
                    true,
                    window,
                    cx,
                )
                .icon(gpui_component::Icon::default().path("icons/restart.svg"))
                .on_click(cx.listener(|this, _, _, cx| {
                    if restart_available(this.runtime.read(cx).upgrade.read(cx).state()) {
                        this.runtime
                            .update(cx, |runtime, cx| runtime.request_exit(true, cx));
                    }
                })),
            );
        }
        if has_update {
            actions = actions.child(
                super::components::form_action(
                    "settings-update-now",
                    "Update now",
                    true,
                    window,
                    cx,
                )
                .icon(gpui_component::Icon::default().path("icons/arrow-up.svg"))
                .disabled(!can_update)
                .when(can_update, |el| {
                    el.on_click(cx.listener(|this, _, _, cx| {
                        let upgrade = this.runtime.read(cx).upgrade.clone();
                        upgrade.update(cx, crate::entities::upgrade::Upgrade::start_update);
                    }))
                }),
            );
            actions = actions.child(
                self.button("settings-update-skip", "Skip this version", false, true)
                    .child(crate::icons::icon("arrow-right", ICON_SM, c::FG_DIM()))
                    .child("Skip version")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let upgrade = this.runtime.read(cx).upgrade.clone();
                        upgrade.update(cx, crate::entities::upgrade::Upgrade::skip);
                    })),
            );
            actions = actions.child(
                self.button("settings-update-copy-url", "Copy release URL", false, true)
                    .child(crate::icons::icon("copy", ICON_SM, c::FG_DIM()))
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
                    !checking && !can_restart,
                )
                .child(crate::icons::icon("refresh", ICON_SM, c::FG_DIM()))
                .child("Check now")
                .when(!checking && !can_restart, |el| {
                    el.on_click(cx.listener(|this, _, _, cx| {
                        let upgrade = this.runtime.read(cx).upgrade.clone();
                        upgrade.update(cx, |upgrade, cx| upgrade.check(true, cx));
                    }))
                }),
            )
            .child(
                self.button("settings-changelog", "View changelog", false, true)
                    .child(crate::icons::icon("list", ICON_SM, c::FG_DIM()))
                    .child("Changelog")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_nested(Page::Changelog, window, cx);
                    })),
            );
        body = body.when(self.category == Category::About, |body| {
            body.child(
                div()
                    .flex()
                    .flex_col()
                    .child(Self::section("Grove"))
                    .child(
                        status::label(
                            "settings-upgrade-status",
                            status_kind,
                            status.clone(),
                            ICON_SM,
                            cx,
                        )
                        .text_size(rpx(TEXT_SMALL)),
                    )
                    .child(Self::setting_row(
                        "Grove version",
                        format!("v{}", env!("CARGO_PKG_VERSION")),
                        actions,
                    )),
            )
        });
        body.when(self.category == Category::General, |body| {
            body.child(
                div()
                    .flex()
                    .flex_col()
                    .child(Self::section("Keyboard"))
                    .child(Self::setting_row(
                        "Shortcuts",
                        "Commands for the current view",
                        self.button("settings-shortcuts", "View shortcuts", false, true)
                            .debug_selector(|| "settings-shortcuts".into())
                            .child(crate::icons::icon("command", ICON_SM, c::FG_DIM()))
                            .child("View shortcuts")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_nested(Page::Shortcuts, window, cx);
                            })),
                    ))
                    .child(Self::setting_row(
                        "Archived projects",
                        "Restore or remove archived project registrations",
                        self.button(
                            "settings-archived-projects",
                            "Archived projects",
                            false,
                            true,
                        )
                        .debug_selector(|| "settings-archived-projects".into())
                        .child(crate::icons::icon("archive", ICON_SM, c::FG_DIM()))
                        .child("Manage")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_nested(Page::Archived, window, cx);
                        })),
                    )),
            )
        })
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
                            .min_w(rpx(SHORTCUT_KEY_W))
                            .font_family(crate::fonts::UI_FAMILY)
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(super::components::shortcut_hint(chord)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(rpx(FIELD_LABEL_COL_W))
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
        body.child(Self::section("GROVE’S CONVERSATION VIEW"))
            .child(Self::setting_row(
                "Enter",
                "Send when idle; queue input while an agent turn is running.",
                div(),
            ))
            .child(Self::setting_row(
                super::components::shortcut_hint("Shift+Enter"),
                "Insert a new line.",
                div(),
            ))
            .child(Self::setting_row(
                "Steer current turn",
                "Codex and Claude Code, while an agent turn is running.",
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .child(super::components::shortcut_hint(format!(
                        "{}+Enter",
                        keymap::platform_mod_label()
                    ))),
            ))
    }

    fn changelog_body(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let upgrade = self.runtime.read(cx).upgrade.clone();
        let upgrade = upgrade.read(cx);
        let mut body = div().flex().flex_col().gap(rpx(SPACE_2XL));
        if super::release_highlights::selected(false).is_some() {
            body = body.child(
                self.button("settings-view-highlights", "View highlights", false, true)
                    .child(crate::icons::icon("sparkle", ICON_SM, c::FG_DIM()))
                    .child("View highlights")
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(SettingsPanelEvent::OpenHighlights { debug: false });
                    })),
            );
        }
        if cfg!(debug_assertions) && super::release_highlights::selected(true).is_some() {
            body = body.child(
                self.button(
                    "settings-preview-highlights",
                    "Preview release highlights",
                    false,
                    true,
                )
                .child(crate::icons::icon("play", ICON_SM, c::FG_DIM()))
                .child("Preview release highlights")
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.emit(SettingsPanelEvent::OpenHighlights { debug: true });
                })),
            );
        }
        match upgrade.changelog() {
            ChangelogState::Idle | ChangelogState::Loading => {
                body = body.child(status::label(
                    "changelog-loading",
                    StatusKind::Working,
                    "Loading changelog…",
                    ICON_SM,
                    cx,
                ));
            }
            ChangelogState::Error(error) => {
                body = body
                    .child(status::label(
                        "changelog-error",
                        StatusKind::Failed,
                        format!("Could not load changelog: {error}"),
                        ICON_SM,
                        cx,
                    ))
                    .child(
                        self.button(
                            "settings-changelog-retry",
                            "Retry loading changelog",
                            false,
                            true,
                        )
                        .self_start()
                        .child(crate::icons::icon("refresh", ICON_SM, c::FG_DIM()))
                        .child("Retry loading")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.runtime
                                .read(cx)
                                .upgrade
                                .clone()
                                .update(cx, crate::entities::upgrade::Upgrade::fetch_changelog);
                        })),
                    );
            }
            ChangelogState::Loaded(notes) if notes.is_empty() => {
                body = body.child("No release notes available.");
            }
            ChangelogState::Loaded(_) => {}
        }
        let notes = upgrade.changelog_notes().to_vec();
        if self.changelog_markdown.len() != notes.len()
            || self
                .changelog_markdown
                .iter()
                .zip(&notes)
                .any(|((tag, body, _), note)| tag != &note.tag || body != &note.body)
        {
            self.changelog_markdown = notes
                .iter()
                .map(|note| {
                    (
                        note.tag.clone(),
                        note.body.clone(),
                        cx.new(|cx| gpui_component::text::TextViewState::markdown(&note.body, cx)),
                    )
                })
                .collect();
        }
        for (index, (tag, _, markdown)) in self.changelog_markdown.iter().enumerate() {
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
                            .child(tag.clone()),
                    )
                    .child(
                        gpui_component::text::TextView::new(markdown)
                            .selectable(true)
                            .style(super::codex_view::markdown_style(window)),
                    ),
            );
        }
        body.text_size(rpx(TEXT_BODY))
    }
}

impl Focusable for SettingsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.connections_pending_focus {
            self.connections_pending_focus = false;
            self.focus.focus(window, cx);
        }
        if self.page == Page::Archived {
            return div()
                .id("settings-archived-page")
                .size_full()
                .min_h_0()
                .flex()
                .flex_col()
                .track_focus(&self.focus)
                .when_some(self.archived_panel.clone(), gpui::ParentElement::child)
                .into_any_element();
        }
        let title = match self.page {
            Page::Settings => "Settings",
            Page::Shortcuts => "Shortcuts",
            Page::Changelog => "Changelog",
            Page::Themes => "Themes",
            Page::Providers => "Providers",
            Page::Connections => "Connections",
            Page::Archived => "Archived projects",
        };
        let body = match self.page {
            Page::Settings => self.settings_body(window, cx),
            Page::Shortcuts => self.shortcuts_body(cx),
            Page::Changelog => self.changelog_body(window, cx),
            Page::Themes => self.themes_body(cx),
            Page::Providers => self.providers_body(window, cx),
            Page::Connections => div().child(self.connections.clone()),
            Page::Archived => {
                div().when_some(self.archived_panel.clone(), gpui::ParentElement::child)
            }
        };
        div()
            .id("settings-panel")
            .debug_selector(|| "settings-panel".into())
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .px(rpx(SPACE_20))
            .pt(rpx(SPACE_20))
            .pb(rpx(SPACE_20))
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::key))
            .child(
                div()
                    .id("settings-content")
                    .debug_selector(|| "settings-content".into())
                    .w_full()
                    .max_w(rpx(MODAL_W_XL))
                    .h_full()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        super::components::content_header()
                            .id("settings-heading")
                            .debug_selector(|| "settings-heading".into())
                            .when(self.page != Page::Settings, |header| {
                                header.child(
                                    super::components::back_action(
                                        "settings-back",
                                        "Back to settings",
                                        window,
                                        cx,
                                    )
                                    .debug_selector(|| "settings-back".into())
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.back(window, cx)),
                                    ),
                                )
                            })
                            .child(
                                super::components::content_header_title(title)
                                    .debug_selector(|| "settings-heading-title".into()),
                            )
                            .child(
                                super::components::header_control_with_label_tooltip(
                                    "settings-close",
                                    "Close settings",
                                )
                                .tab_index(0)
                                .focus_visible(|style| style.bg(c::BG_HOVER()))
                                .debug_selector(|| "settings-close".into())
                                .child(crate::icons::icon("close", ICON_MD, c::FG_DIM()))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.close(window, cx)),
                                ),
                            ),
                    )
                    .when(self.page == Page::Settings, |region| {
                        region.child(
                            div()
                                .id("settings-category-navigation")
                                .debug_selector(|| "settings-category-navigation".into())
                                .w_full()
                                .flex_shrink_0()
                                .flex()
                                .mt(rpx(SPACE_3XL))
                                .overflow_x_scroll()
                                .border_b_1()
                                .border_color(c::BORDER_SOFT())
                                .children(Category::ALL.into_iter().map(|category| {
                                    let selected = self.category == category;
                                    div()
                                        .id(format!("settings-category-{}", category.label()))
                                        .role(gpui::Role::Button)
                                        .aria_label(category.label())
                                        .aria_selected(selected)
                                        .tab_index(0)
                                        .flex_shrink_0()
                                        .px(rpx(SPACE_2XL))
                                        .py(rpx(SPACE_LG))
                                        .border_b_2()
                                        .border_color(if selected { c::FG_DIM() } else { c::BG() })
                                        .text_size(rpx(TEXT_SMALL))
                                        .font_weight(if selected {
                                            FontWeight::MEDIUM
                                        } else {
                                            FontWeight::NORMAL
                                        })
                                        .text_color(if selected { c::FG() } else { c::FG_DIM() })
                                        .cursor_pointer()
                                        .hover(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
                                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                                        .debug_selector(move || {
                                            format!("settings-category-{}", category.label())
                                        })
                                        .flex()
                                        .items_center()
                                        .gap(rpx(SPACE_SM))
                                        .child(crate::icons::icon(
                                            category.icon(),
                                            ICON_SM,
                                            c::FG_DIM(),
                                        ))
                                        .child(category.label())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.select_category(category, cx);
                                        }))
                                })),
                        )
                    })
                    .when_some(self.error.clone(), |region, error| {
                        region.child(motion::fast(
                            div()
                                .id("settings-error")
                                .debug_selector(|| "settings-error".into())
                                .role(gpui::Role::Alert)
                                .flex_shrink_0()
                                .w_full()
                                .py(rpx(SPACE_LG))
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(StatusKind::Failed.color())
                                .child(status::label(
                                    "settings-error-status",
                                    StatusKind::Failed,
                                    error.clone(),
                                    ICON_SM,
                                    cx,
                                )),
                            format!("settings-error-{error}"),
                            cx,
                        ))
                    })
                    .child(
                        div()
                            .id("settings-scroll")
                            .debug_selector(|| "settings-scroll".into())
                            .w_full()
                            .min_w_0()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .child(
                                div()
                                    .id("settings-body")
                                    .debug_selector(|| "settings-body".into())
                                    .w_full()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .when(self.page != Page::Settings, |body| {
                                        body.pt(rpx(SPACE_20))
                                    })
                                    .child(body)
                                    .when(
                                        self.page == Page::Themes
                                            || self.page == Page::Settings
                                                && self.category != Category::About,
                                        |body| {
                                            body.child(
                                                div()
                                                    .id("settings-save-note")
                                                    .debug_selector(|| "settings-save-note".into())
                                                    .pt(rpx(SPACE_3XL))
                                                    .pb(rpx(SPACE_LG))
                                                    .text_size(rpx(TEXT_MICRO))
                                                    .text_color(c::FG_DIM())
                                                    .child("Changes save automatically"),
                                            )
                                        },
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME};
    use grove_core::storage::AppearancePreference;

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

    impl SettingsPanel {
        fn fixture_providers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            use crate::entities::provider_updates::ProviderRow;
            use grove_core::provider_updates::{Installer, ProviderStatus};
            for (index, scenario) in ["connected", "signed-out"].into_iter().enumerate() {
                self.provider_accounts[index]
                    .update(cx, |account, cx| account.fixture(scenario, window, cx));
            }
            self.runtime
                .read(cx)
                .provider_updates
                .clone()
                .update(cx, |updates, cx| {
                    updates.set_rows_for_test(
                        [Agent::Codex, Agent::Claude]
                            .into_iter()
                            .map(|agent| ProviderRow {
                                status: ProviderStatus {
                                    agent,
                                    installed: Some("1.0.0".into()),
                                    latest: Some("1.1.0".into()),
                                    executable: Some("/fixture/provider".into()),
                                    error: None,
                                    installer: if agent == Agent::Claude {
                                        Installer::ClaudeNative
                                    } else {
                                        Installer::Npm {
                                            executable: "/fixture/npm".into(),
                                            prefix: "/fixture".into(),
                                            package: "@openai/codex".into(),
                                        }
                                    },
                                },
                                checking: false,
                                updating: false,
                                updated: false,
                                update_verified: false,
                                last_success: Some(1),
                                error: None,
                            })
                            .collect(),
                        cx,
                    );
                });
        }
    }

    #[gpui::test]
    fn providers_management_is_account_and_version_only_and_restores_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open_provider_updates(window, cx);
                panel.fixture_providers(window, cx);
            });
        });
        draw(cx);
        assert!(cx
            .debug_bounds("settings-provider-codex-management")
            .is_none());
        assert!(cx
            .debug_bounds("settings-provider-claude-management")
            .is_none());
        for agent in [Agent::Codex, Agent::Claude] {
            let manage = cx
                .debug_bounds(if agent == Agent::Codex {
                    "settings-provider-codex-manage"
                } else {
                    "settings-provider-claude-manage"
                })
                .unwrap();
            cx.simulate_click(manage.center(), gpui::Modifiers::default());
            draw(cx);
            assert!(cx
                .debug_bounds(if agent == Agent::Codex {
                    "settings-provider-codex-management"
                } else {
                    "settings-provider-claude-management"
                })
                .is_some());
            assert!(cx.debug_bounds("connections-context").is_none());
            assert!(cx.debug_bounds("connections-server-0").is_none());
            assert!(cx.debug_bounds("connections-reconnect").is_none());
            cx.simulate_keystrokes("space");
            draw(cx);
            assert!(cx
                .debug_bounds(if agent == Agent::Codex {
                    "settings-provider-codex-management"
                } else {
                    "settings-provider-claude-management"
                })
                .is_none());
            cx.update(|window, cx| {
                assert!(
                    panel.read(cx).provider_manage_focus[usize::from(agent != Agent::Codex)]
                        .is_focused(window)
                );
            });
        }
    }

    #[gpui::test]
    fn provider_account_refresh_and_confirmation_are_keyboard_accessible(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open_provider_updates(window, cx);
                panel.fixture_providers(window, cx);
                panel.managed_provider = Some(Agent::Codex);
            });
        });
        draw(cx);
        cx.update(|window, cx| {
            panel.read(cx).provider_accounts[0]
                .read(cx)
                .fixture_return_focus()
                .focus(window, cx);
        });
        cx.simulate_keystrokes("enter");
        draw(cx);
        cx.simulate_keystrokes("tab space");
        draw(cx);
        cx.update(|_, cx| {
            assert!(panel.read(cx).provider_accounts[0]
                .read(cx)
                .fixture_confirming());
        });
        cx.simulate_keystrokes("tab space");
        draw(cx);
        cx.update(|_, cx| {
            assert!(!panel.read(cx).provider_accounts[0]
                .read(cx)
                .fixture_confirming());
        });
    }

    #[gpui::test]
    fn provider_actions_fit_native_widths_and_zoom(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open_provider_updates(window, cx);
                panel.fixture_providers(window, cx);
                panel.managed_provider = Some(Agent::Codex);
            });
        });
        for width in [900., 500., 320.] {
            for rem in [16., 24.] {
                cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1200.)));
                cx.update(|window, cx| {
                    window.set_rem_size(gpui::px(rem));
                    gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                });
                draw(cx);
                let body = cx.debug_bounds("settings-body").unwrap();
                for name in [
                    "settings-provider-codex-actions",
                    "provider-account-actions",
                    "settings-provider-codex-version-actions",
                ] {
                    let action = cx.debug_bounds(name).unwrap();
                    assert!(
                        action.left() >= body.left() && action.right() <= body.right(),
                        "{name} outside body at {width}/{rem}: {action:?}, {body:?}"
                    );
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "captures isolated Connections screens with fixture state"]
    fn connections_review_linux_capture() {
        let output = std::path::PathBuf::from(
            std::env::var_os("GROVE_CONNECTIONS_QA_OUTPUT").expect("task-owned capture directory"),
        );
        assert!(
            std::env::var_os("GROVE_CONFIG_DIR").is_some(),
            "isolated configuration required"
        );
        fs_err::create_dir_all(&output).unwrap();
        gpui_platform::application()
            .with_assets(crate::assets::Assets)
            .run(move |cx| {
                setup(cx);
                crate::fonts::register(cx).unwrap();
                cx.set_reduce_motion(true);
                let mut retained = None;
                let window = cx
                    .open_window(
                        gpui::WindowOptions {
                            app_id: Some("dev.grove.connections-review-qa".into()),
                            kind: gpui::WindowKind::PopUp,
                            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                                gpui::point(gpui::px(40.), gpui::px(40.)),
                                gpui::size(gpui::px(900.), gpui::px(900.)),
                            ))),
                            ..Default::default()
                        },
                        |window, cx| {
                            window.set_window_title("Grove Connections review QA");
                            let runtime = cx.new(Runtime::new);
                            let panel = cx.new(|cx| SettingsPanel::new(runtime, cx));
                            panel.update(cx, |panel, cx| {
                                panel.open(window, cx);
                                panel.category = Category::Agents;
                                panel.page = Page::Connections;
                            });
                            retained = Some(panel.clone());
                            cx.new(|cx| gpui_component::Root::new(panel, window, cx))
                        },
                    )
                    .unwrap();
                let panel = retained.unwrap();
                cx.activate(true);
                cx.spawn(async move |cx| {
                    for (theme, rem, width) in [
                        ("grove-dark", 16., 900.),
                        ("grove-light", 16., 900.),
                        ("grove-dark", 24., 500.),
                    ] {
                        for scenario in [
                            "connected",
                            "signed-out",
                            "external",
                            "waiting",
                            "failed",
                            "confirm",
                            "contexts",
                        ] {
                            cx.update(|cx| {
                                grove_core::theme::set_by_name(theme);
                                c::sync_component_theme(cx);
                                gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                            });
                            cx.update_window(window.into(), |_, window, cx| {
                                window.resize(gpui::size(gpui::px(width), gpui::px(900.)));
                                window.set_rem_size(gpui::px(rem));
                                panel.update(cx, |panel, cx| {
                                    panel.scroll.set_offset(gpui::Point::default());
                                    panel.connections.update(cx, |connections, cx| {
                                        connections.fixture(scenario, window, cx);
                                    });
                                    cx.notify();
                                });
                                window.refresh();
                            })
                            .unwrap();
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(200))
                                .await;
                            if let Some(helper) = std::env::var_os("GROVE_QA_EXPOSE_HELPER") {
                                for _ in 0..3 {
                                    assert!(std::process::Command::new(&helper)
                                        .arg("Grove Connections review QA")
                                        .status()
                                        .unwrap()
                                        .success());
                                    cx.background_executor()
                                        .timer(std::time::Duration::from_millis(100))
                                        .await;
                                }
                            }
                            let path = output
                                .join(format!("connections-{scenario}-{theme}-{rem}-{width}.png"));
                            assert!(std::process::Command::new("import")
                                .arg("-window")
                                .arg("Grove Connections review QA")
                                .arg(&path)
                                .status()
                                .unwrap()
                                .success());
                            if scenario == "waiting" || width <= 500. {
                                cx.update(|cx| {
                                    panel.update(cx, |panel, cx| {
                                        panel.scroll.set_offset(gpui::point(
                                            gpui::px(0.),
                                            gpui::px(-10000.),
                                        ));
                                        cx.notify();
                                    });
                                });
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(100))
                                    .await;
                                if let Some(helper) = std::env::var_os("GROVE_QA_EXPOSE_HELPER") {
                                    for _ in 0..3 {
                                        assert!(std::process::Command::new(&helper)
                                            .arg("Grove Connections review QA")
                                            .status()
                                            .unwrap()
                                            .success());
                                        cx.background_executor()
                                            .timer(std::time::Duration::from_millis(100))
                                            .await;
                                    }
                                }
                                let path = output.join(format!(
                                    "connections-{scenario}-bottom-{theme}-{rem}-{width}.png"
                                ));
                                assert!(std::process::Command::new("import")
                                    .arg("-window")
                                    .arg("Grove Connections review QA")
                                    .arg(path)
                                    .status()
                                    .unwrap()
                                    .success());
                            }
                        }
                    }
                    cx.update(|cx| cx.quit());
                })
                .detach();
            });
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "captures isolated Providers screens with fixture state"]
    fn providers_review_linux_capture() {
        let output = std::path::PathBuf::from(
            std::env::var_os("GROVE_PROVIDERS_QA_OUTPUT").expect("task-owned capture directory"),
        );
        assert!(
            std::env::var_os("GROVE_CONFIG_DIR").is_some(),
            "isolated configuration required"
        );
        fs_err::create_dir_all(&output).unwrap();
        gpui_platform::application()
            .with_assets(crate::assets::Assets)
            .run(move |cx| {
                setup(cx);
                crate::fonts::register(cx).unwrap();
                cx.set_reduce_motion(true);
                let mut retained = None;
                let window = cx
                    .open_window(
                        gpui::WindowOptions {
                            app_id: Some("dev.grove.providers-review-qa".into()),
                            kind: gpui::WindowKind::PopUp,
                            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                                gpui::point(gpui::px(40.), gpui::px(40.)),
                                gpui::size(gpui::px(900.), gpui::px(900.)),
                            ))),
                            ..Default::default()
                        },
                        |window, cx| {
                            window.set_window_title("Grove Providers review QA");
                            let runtime = cx.new(Runtime::new);
                            let panel = cx.new(|cx| SettingsPanel::new(runtime, cx));
                            panel.update(cx, |panel, cx| {
                                panel.open(window, cx);
                                panel.category = Category::Agents;
                                panel.page = Page::Providers;
                            });
                            retained = Some(panel.clone());
                            cx.new(|cx| gpui_component::Root::new(panel, window, cx))
                        },
                    )
                    .unwrap();
                let panel = retained.unwrap();
                cx.activate(true);
                cx.spawn(async move |cx| {
                    for (theme, rem, width) in [
                        ("grove-dark", 16., 900.),
                        ("grove-light", 16., 900.),
                        ("grove-dark", 24., 500.),
                    ] {
                        for scenario in [
                            "collapsed",
                            "codex",
                            "claude",
                            "waiting",
                            "confirm",
                            "external",
                        ] {
                            cx.update(|cx| {
                                grove_core::theme::set_by_name(theme);
                                c::sync_component_theme(cx);
                                gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                            });
                            cx.update_window(window.into(), |_, window, cx| {
                                window.resize(gpui::size(gpui::px(width), gpui::px(900.)));
                                window.set_rem_size(gpui::px(rem));
                                panel.update(cx, |panel, cx| {
                                    panel.scroll.set_offset(gpui::Point::default());
                                    panel.fixture_providers(window, cx);
                                    panel.managed_provider = match scenario {
                                        "collapsed" => None,
                                        "claude" | "external" => Some(Agent::Claude),
                                        _ => Some(Agent::Codex),
                                    };
                                    let index =
                                        usize::from(panel.managed_provider == Some(Agent::Claude));
                                    if ["waiting", "confirm", "external"].contains(&scenario) {
                                        panel.provider_accounts[index].update(cx, |account, cx| {
                                            account.fixture(scenario, window, cx);
                                        });
                                    }
                                    cx.notify();
                                });
                                window.refresh();
                            })
                            .unwrap();
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(200))
                                .await;
                            if let Some(helper) = std::env::var_os("GROVE_QA_EXPOSE_HELPER") {
                                for _ in 0..3 {
                                    assert!(std::process::Command::new(&helper)
                                        .arg("Grove Providers review QA")
                                        .status()
                                        .unwrap()
                                        .success());
                                    cx.background_executor()
                                        .timer(std::time::Duration::from_millis(100))
                                        .await;
                                }
                            }
                            let path = output
                                .join(format!("providers-{scenario}-{theme}-{rem}-{width}.png"));
                            assert!(std::process::Command::new("import")
                                .arg("-window")
                                .arg("Grove Providers review QA")
                                .arg(&path)
                                .status()
                                .unwrap()
                                .success());
                            if scenario == "waiting" || width <= 500. {
                                cx.update(|cx| {
                                    panel.update(cx, |panel, cx| {
                                        panel.scroll.set_offset(gpui::point(
                                            gpui::px(0.),
                                            gpui::px(-10000.),
                                        ));
                                        cx.notify();
                                    });
                                });
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(100))
                                    .await;
                                if let Some(helper) = std::env::var_os("GROVE_QA_EXPOSE_HELPER") {
                                    for _ in 0..3 {
                                        assert!(std::process::Command::new(&helper)
                                            .arg("Grove Providers review QA")
                                            .status()
                                            .unwrap()
                                            .success());
                                        cx.background_executor()
                                            .timer(std::time::Duration::from_millis(100))
                                            .await;
                                    }
                                }
                                let path = output.join(format!(
                                    "providers-{scenario}-bottom-{theme}-{rem}-{width}.png"
                                ));
                                assert!(std::process::Command::new("import")
                                    .arg("-window")
                                    .arg("Grove Providers review QA")
                                    .arg(path)
                                    .status()
                                    .unwrap()
                                    .success());
                            }
                        }
                    }
                    cx.update(|cx| cx.quit());
                })
                .detach();
            });
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "captures isolated Settings pages with fixture state"]
    fn settings_review_linux_capture() {
        let output = std::path::PathBuf::from(
            std::env::var_os("GROVE_SETTINGS_QA_OUTPUT").expect("task-owned capture directory"),
        );
        assert!(
            std::env::var_os("GROVE_CONFIG_DIR").is_some(),
            "isolated configuration required"
        );
        fs_err::create_dir_all(&output).unwrap();
        gpui_platform::application().with_assets(crate::assets::Assets).run(move |cx| {
            setup(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
            let mut retained = None;
            let window = cx.open_window(gpui::WindowOptions {
                app_id: Some("dev.grove.settings-review-qa".into()),
                window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                    gpui::point(gpui::px(40.), gpui::px(40.)), gpui::size(gpui::px(900.), gpui::px(850.))))),
                ..Default::default()
            }, |window, cx| {
                window.set_window_title("Grove Settings review QA");
                let runtime = cx.new(Runtime::new);
                runtime.read(cx).upgrade.clone().update(cx, |upgrade, cx| upgrade.set_fixture(UpgradeState::Idle, ChangelogState::Idle, cx));
                let panel = cx.new(|cx| SettingsPanel::new(runtime, cx));
                panel.update(cx, |panel, cx| panel.open(window, cx));
                retained = Some(panel.clone());
                cx.new(|cx| gpui_component::Root::new(panel, window, cx))
            }).unwrap();
            let panel = retained.unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let cases = [
                    ("general", Category::General, Page::Settings),
                    ("general-open", Category::General, Page::Settings),
                    ("agents", Category::Agents, Page::Settings),
                    ("terminal", Category::Terminal, Page::Settings),
                    ("privacy", Category::Privacy, Page::Settings),
                    ("about", Category::About, Page::Settings),
                    ("themes", Category::General, Page::Themes),
                    ("themes-notices", Category::General, Page::Themes),
                    ("archived", Category::General, Page::Archived),
                    ("shortcuts", Category::General, Page::Shortcuts),
                    ("providers", Category::Agents, Page::Providers),
                    ("providers-error", Category::Agents, Page::Providers),
                    ("changelog", Category::About, Page::Changelog),
                    ("changelog-error", Category::About, Page::Changelog),
                    ("terminal-working", Category::Terminal, Page::Settings),
                    ("terminal-confirmed", Category::Terminal, Page::Settings),
                    ("terminal-error", Category::Terminal, Page::Settings),
                ];
                for (theme, rem, width) in [("grove-dark", 16., 900.), ("grove-light", 16., 900.), ("grove-dark", 24., 500.), ("grove-dark", 24., 320.)] {
                    for (name, category, page) in cases {
                        cx.update(|cx| {
                            grove_core::theme::set_by_name(theme);
                            c::sync_component_theme(cx);
                            gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                        });
                        cx.update_window(window.into(), |_, window, cx| {
                            window.resize(gpui::size(gpui::px(width), gpui::px(850.)));
                            window.set_rem_size(gpui::px(rem));
                            panel.update(cx, |panel, cx| {
                                cx.global_mut::<SettingsState>().store.after_session_start = if name == "general-open" {
                                    AfterSessionStart::OpenSession
                                } else {
                                    AfterSessionStart::StayOnDashboard
                                };
                                panel.select_category(category, cx);
                                panel.show_theme_notices = name == "themes-notices";
                                if page == Page::Changelog {
                                    panel.page = page; // Fixture content; never fetch the network.
                                } else if page != Page::Settings {
                                    panel.open_nested(page, window, cx);
                                }
                                let runtime = panel.runtime.read(cx);
                                let upgrade = runtime.upgrade.clone();
                                let environment = runtime.shell_environment.clone();
                                let release = grove_core::upgrade::Release {
                                    version: semver::Version::new(99,0,0), tag: "v99.0.0".into(), html_url: "https://example.invalid/release".into(), body: String::new(), dmg_url: None, dmg_sha256_url: None, target_commitish: "main".into()
                                };
                                let notes = vec![grove_core::upgrade::ReleaseNote {
                                    tag: "Example release".into(), name: String::new(), date: String::new(), body: "## Improvements\n\n- Clearer update status\n- Better recovery\n\n[Release details](https://example.invalid/release)\n\n```rust\nlet recovered = true;\n```".into()
                                }];
                                upgrade.update(cx, |upgrade, cx| {
                                    upgrade.set_fixture(UpgradeState::Skipped(release), ChangelogState::Loaded(notes), cx);
                                    if name == "changelog-error" { upgrade.set_fixture(UpgradeState::Idle, ChangelogState::Error("Network unavailable".into()), cx); }
                                });
                                environment.update(cx, |environment, cx| environment.set_fixture(match name {
                                    "terminal-working" => RefreshState::Refreshing,
                                    "terminal-confirmed" => RefreshState::Refreshed,
                                    "terminal-error" => RefreshState::Error,
                                    _ => RefreshState::Idle,
                                }, cx));
                                if page == Page::Providers {
                                    let providers = panel.runtime.read(cx).provider_updates.clone();
                                    let rows = if name == "providers-error" {
                                        use crate::entities::provider_updates::ProviderRow;
                                        use grove_core::provider_updates::{Installer, ProviderStatus};
                                        panel.provider_details.insert(Agent::Claude.label());
                                        vec![ProviderRow {
                                            status: ProviderStatus {
                                                agent: Agent::Claude, installed: Some("1.0.0".into()), latest: None,
                                                executable: Some("/fixture/claude".into()), error: None,
                                                installer: Installer::ClaudeNative,
                                            }, checking: false, updating: false, updated: false, update_verified: false,
                                            last_success: Some(1), error: Some("Permission denied while updating the provider executable. Example diagnostic only.".into()),
                                        }]
                                    } else { Vec::new() };
                                    providers.update(cx, |providers, cx| providers.set_rows_for_test(rows, cx));
                                }
                                if name == "privacy" {
                                    SettingsState::update(cx, |store| store.telemetry_enabled = Some(true));
                                }
                                cx.notify();
                            });
                            window.refresh();
                        }).unwrap();
                        cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
                        if name == "themes-notices" || (name.starts_with("terminal-") && width <= 500.) {
                            cx.update(|cx| panel.update(cx, |panel, cx| {
                                panel.scroll.set_offset(gpui::point(gpui::px(0.), gpui::px(-10000.)));
                                cx.notify();
                            }));
                            cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                        }
                        let path = output.join(format!("{name}-{theme}-{rem}-{width}.png"));
                        assert!(std::process::Command::new("import").arg("-window").arg("Grove Settings review QA").arg(&path).status().unwrap().success());
                        eprintln!("Settings fixture capture: {}", path.display());
                    }
                }
                cx.update(|cx| cx.quit());
            }).detach();
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "opens an isolated native X11 Settings window"]
    fn settings_header_linux_capture() {
        assert!(
            std::env::var_os("GROVE_CONFIG_DIR").is_some(),
            "use an isolated config directory"
        );
        gpui_platform::application()
            .with_assets(crate::assets::Assets)
            .run(|cx| {
                setup(cx);
                crate::fonts::register(cx).unwrap();
                let window = cx
                    .open_window(
                        gpui::WindowOptions {
                            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                                gpui::point(gpui::px(40.), gpui::px(40.)),
                                gpui::size(gpui::px(600.), gpui::px(650.)),
                            ))),
                            ..Default::default()
                        },
                        |window, cx| {
                            window.set_window_title("Grove header pattern QA");
                            let runtime = cx.new(Runtime::new);
                            let panel = cx.new(|cx| SettingsPanel::new(runtime, cx));
                            panel.update(cx, |panel, cx| {
                                panel.open(window, cx);
                                panel.open_nested(Page::Shortcuts, window, cx);
                            });
                            cx.new(|cx| gpui_component::Root::new(panel, window, cx))
                        },
                    )
                    .unwrap();
                cx.activate(true);
                cx.spawn(async move |cx| {
                    for (theme, rem) in [
                        ("grove-dark", 16.),
                        ("grove-light", 16.),
                        ("grove-dark", 24.),
                    ] {
                        cx.update(|cx| {
                            grove_core::theme::set_by_name(theme);
                            c::sync_component_theme(cx);
                            gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                        });
                        cx.update_window(window.into(), |_, window, _| {
                            window.set_rem_size(gpui::px(rem));
                            window.refresh();
                        })
                        .unwrap();
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(600))
                            .await;
                        let output = format!("/tmp/grove-settings-header-{theme}-{rem}.png");
                        assert!(std::process::Command::new("import")
                            .args(["-window", "Grove header pattern QA", &output])
                            .status()
                            .unwrap()
                            .success());
                        eprintln!("Native Settings header capture: {output}");
                    }
                    cx.update(|cx| cx.quit());
                })
                .detach();
            });
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "opens an isolated native X11 Settings window"]
    fn settings_button_icons_linux_capture() {
        assert!(
            std::env::var_os("GROVE_CONFIG_DIR").is_some(),
            "use an isolated config directory"
        );
        gpui_platform::application()
            .with_assets(crate::assets::Assets)
            .run(|cx| {
                setup(cx);
                crate::fonts::register(cx).unwrap();
                let window = cx
                    .open_window(
                        gpui::WindowOptions {
                            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                                gpui::point(gpui::px(40.), gpui::px(40.)),
                                gpui::size(gpui::px(600.), gpui::px(650.)),
                            ))),
                            ..Default::default()
                        },
                        |window, cx| {
                            window.set_window_title("Grove button icons QA");
                            let runtime = cx.new(Runtime::new);
                            let panel = cx.new(|cx| SettingsPanel::new(runtime, cx));
                            panel.update(cx, |panel, cx| {
                                panel.open(window, cx);
                            });
                            cx.new(|cx| gpui_component::Root::new(panel, window, cx))
                        },
                    )
                    .unwrap();
                cx.activate(true);
                cx.spawn(async move |cx| {
                    for (theme, rem) in [
                        ("grove-dark", 16.),
                        ("grove-light", 16.),
                        ("grove-dark", 24.),
                    ] {
                        cx.update(|cx| {
                            grove_core::theme::set_by_name(theme);
                            c::sync_component_theme(cx);
                            gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
                        });
                        cx.update_window(window.into(), |_, window, _| {
                            window.set_rem_size(gpui::px(rem));
                            window.refresh();
                        })
                        .unwrap();
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(600))
                            .await;
                        let output = format!("/tmp/grove-settings-buttons-{theme}-{rem}.png");
                        assert!(std::process::Command::new("import")
                            .args(["-window", "Grove button icons QA", &output])
                            .status()
                            .unwrap()
                            .success());
                        eprintln!("Native Settings button capture: {output}");
                    }
                    cx.update(|cx| cx.quit());
                })
                .detach();
            });
    }

    #[gpui::test]
    fn connections_auth_completion_restores_owned_focus_without_stealing_other_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open(window, cx);
                panel.page = Page::Connections;
            });
        });
        for target in [0, 1, 2] {
            cx.update(|window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.connections.update(cx, |connections, cx| {
                        connections.fixture("waiting", window, cx);
                    });
                });
            });
            draw(cx);
            cx.update(|window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.connections.update(cx, |connections, cx| {
                        let focus = if target == 2 {
                            connections.fixture_copy_focus()
                        } else {
                            connections.fixture_auth_focus(target == 0, cx)
                        };
                        focus.focus(window, cx);
                        connections.fixture_finish(cx);
                    });
                });
            });
            draw(cx);
            cx.update(|window, cx| {
                assert!(panel
                    .read(cx)
                    .connections
                    .read(cx)
                    .fixture_return_focus()
                    .is_focused(window));
            });
        }
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.connections.update(cx, |connections, cx| {
                    connections.fixture("waiting", window, cx);
                });
                panel.focus.focus(window, cx);
            });
        });
        draw(cx);
        cx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.connections.update(
                    cx,
                    super::super::connections_panel::ConnectionsPanel::fixture_finish,
                );
            });
        });
        draw(cx);
        cx.update(|window, cx| assert!(panel.read(cx).focus.is_focused(window)));
    }

    #[gpui::test]
    fn after_session_start_choices_update_saved_preference(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        struct NavigationHost(Entity<SettingsPanel>);
        impl Render for NavigationHost {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .on_key_down(super::super::shell::traverse_unhandled_tab)
                    .child(self.0.clone())
            }
        }
        let mut panel = None;
        let (_, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            let entity = cx.new(|cx| SettingsPanel::new(runtime, cx));
            panel = Some(entity.clone());
            NavigationHost(entity)
        });
        let panel = panel.unwrap();
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.open(window, cx)));
        draw(cx);
        for (id, expected) in [
            (
                "settings-after-session-start-open",
                AfterSessionStart::OpenSession,
            ),
            (
                "settings-after-session-start-stay",
                AfterSessionStart::StayOnDashboard,
            ),
        ] {
            let bounds = cx.debug_bounds(id).unwrap();
            cx.simulate_click(bounds.center(), gpui::Modifiers::default());
            draw(cx);
            cx.update(|_, cx| {
                assert_eq!(
                    cx.global::<SettingsState>().store.after_session_start,
                    expected
                )
            });
        }
        // Traverse through the app's actual unhandled-Tab route, not direct focus.
        cx.update(|window, cx| {
            panel.read(cx).after_session_start_focus[0]
                .clone()
                .focus(window, cx)
        });
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| {
            assert!(panel.read(cx).after_session_start_focus[1].is_focused(window))
        });
        cx.simulate_keystrokes("shift-tab");
        draw(cx);
        cx.update(|window, cx| {
            assert!(panel.read(cx).after_session_start_focus[0].is_focused(window))
        });
        for (index, key, expected) in [
            (1, "enter", AfterSessionStart::OpenSession),
            (0, "space", AfterSessionStart::StayOnDashboard),
        ] {
            cx.update(|window, cx| {
                panel.read(cx).after_session_start_focus[index]
                    .clone()
                    .focus(window, cx)
            });
            cx.simulate_keystrokes(key);
            draw(cx);
            cx.update(|window, cx| {
                assert_eq!(
                    cx.global::<SettingsState>().store.after_session_start,
                    expected
                );
                assert!(panel.read(cx).after_session_start_focus[index].is_focused(window));
            });
        }
    }

    #[gpui::test]
    fn removed_preferences_preserve_saved_launch_values(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        cx.update(|cx| {
            SettingsState::update(cx, |store| {
                store.default_landing_page = grove_core::storage::LandingPage::History;
                store.default_agent = Some(Agent::Codex);
            });
        });
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.open(window, cx)));
        draw(cx);
        assert!(cx.debug_bounds("settings-landing-history").is_none());
        let archive = cx.debug_bounds("settings-archived-projects").unwrap();
        let shortcuts = cx.debug_bounds("settings-shortcuts").unwrap();
        assert!(archive.top() > shortcuts.bottom());
        cx.update(|_, cx| {
            panel.update(cx, |panel, cx| panel.select_category(Category::Agents, cx));
        });
        draw(cx);
        assert!(cx.debug_bounds("settings-agent-codex").is_none());
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<SettingsState>().store.default_agent,
                Some(Agent::Codex)
            );
            assert_eq!(
                cx.global::<SettingsState>().store.default_landing_page,
                grove_core::storage::LandingPage::History
            );
        });
    }

    #[gpui::test]
    fn refresh_receipts_fit_narrow_zoomed_settings(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(320.), gpui::px(600.)));
        cx.update(|window, cx| {
            window.set_rem_size(gpui::px(24.));
            panel.update(cx, |panel, cx| {
                panel.open(window, cx);
                panel.select_category(Category::Terminal, cx);
            });
        });
        let mut action_bounds = None;
        for state in [
            RefreshState::Idle,
            RefreshState::Refreshing,
            RefreshState::Refreshed,
            RefreshState::Error,
        ] {
            cx.update(|_, cx| {
                panel
                    .read(cx)
                    .runtime
                    .read(cx)
                    .shell_environment
                    .clone()
                    .update(cx, |environment, cx| environment.set_fixture(state, cx));
            });
            draw(cx);
            let action = cx.debug_bounds("settings-shell-environment").unwrap();
            let body = cx.debug_bounds("settings-body").unwrap();
            assert!(
                action.left() >= body.left() && action.right() <= body.right(),
                "{state:?}: {action:?} outside {body:?}"
            );
            if let Some(previous) = action_bounds {
                assert_eq!(action, previous, "receipt moved action slot");
            }
            action_bounds = Some(action);
        }
    }

    #[gpui::test]
    fn save_notes_only_appear_on_editable_pages(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.open(window, cx)));
        draw(cx);
        assert!(cx.debug_bounds("settings-save-note").is_some());
        for page in [
            Page::Shortcuts,
            Page::Changelog,
            Page::Providers,
            Page::Settings,
        ] {
            cx.update(|_, cx| {
                panel.update(cx, |panel, cx| {
                    panel.page = page;
                    panel.category = Category::About;
                    panel
                        .runtime
                        .read(cx)
                        .upgrade
                        .clone()
                        .update(cx, |upgrade, cx| {
                            upgrade.set_fixture(
                                UpgradeState::Idle,
                                ChangelogState::Error("Offline".into()),
                                cx,
                            );
                        });
                    cx.notify();
                });
            });
            draw(cx);
            assert!(cx.debug_bounds("settings-save-note").is_none(), "{page:?}");
        }
    }

    #[test]
    fn terminal_agent_choice_inverts_existing_native_preference() {
        let mut store = Store {
            native_agents_enabled: true,
            ..Store::default()
        };
        assert!(!runs_agents_in_terminal(&store));
        store.native_agents_enabled = false;
        assert!(runs_agents_in_terminal(&store));
    }

    #[test]
    fn appearance_prefers_saved_mode_over_legacy_theme() {
        let mut store = Store {
            theme: Some("catppuccin-latte".into()),
            ..Store::default()
        };
        assert_eq!(store.selected_theme_id(false), "grove-light");
        store.appearance = Some(AppearancePreference::Dark);
        assert_eq!(store.selected_theme_id(false), "grove-dark");
        store.appearance = Some(AppearancePreference::System);
        assert_eq!(store.selected_theme_id(true), "grove-light");
    }

    #[test]
    fn shortcut_reference_includes_zen_but_excludes_unhandled_grid_actions() {
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
        assert!(visible
            .iter()
            .any(|(_, def)| def.description == "Toggle zen mode"));
        assert!(!visible
            .iter()
            .any(|(_, def)| def.description == "Move focus in grid"));
    }

    #[gpui::test]
    fn theme_navigation_search_immediate_save_and_retry(cx: &mut gpui::TestAppContext) {
        struct RestoreFile(std::path::PathBuf, std::path::PathBuf);
        impl Drop for RestoreFile {
            fn drop(&mut self) {
                let _ = fs_err::remove_dir(&self.0);
                let _ = fs_err::rename(&self.1, &self.0);
            }
        }
        let _disk = crate::settings::lock_test_config();
        let _lock = c::ACTIVE_THEME_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _restore = c::ActiveThemeGuard::capture();
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open(window, cx);
                panel.open_nested(Page::Themes, window, cx);
            });
        });
        cx.simulate_resize(gpui::size(gpui::px(900.0), gpui::px(900.0)));
        cx.update(|window, _| window.set_rem_size(gpui::px(crate::zoom::REM_BASE * 1.1)));
        draw(cx);
        let baseline = cx
            .debug_bounds("theme-kind-grove-dark")
            .expect("Grove kind column");
        for (kind_selector, name_selector) in [
            (
                "theme-kind-catppuccin-macchiato",
                "theme-name-catppuccin-macchiato",
            ),
            ("theme-kind-tokyo-night", "theme-name-tokyo-night"),
        ] {
            let kind = cx.debug_bounds(kind_selector).expect("kind column");
            let name = cx.debug_bounds(name_selector).expect("name column");
            assert!(
                (f32::from(kind.origin.x - baseline.origin.x)).abs() < 0.1,
                "kind columns must align"
            );
            assert!(
                name.right() < kind.left(),
                "name must yield to trailing metadata"
            );
        }
        cx.simulate_input("frappe");
        draw(cx);
        cx.simulate_keystrokes("down");
        draw(cx);
        cx.update(|window, cx| {
            assert!(panel.read(cx).theme_focus[4].is_focused(window));
            assert!(
                cx.global::<SettingsState>().store.selected_theme.is_none(),
                "focus must not select"
            );
        });
        cx.simulate_keystrokes("enter");
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<SettingsState>().store.selected_theme.as_deref(),
                Some("catppuccin-frappe")
            );
            assert_eq!(grove_core::theme::current().name, "catppuccin-frappe");
            assert_eq!(
                grove_core::storage::load()
                    .unwrap()
                    .selected_theme
                    .as_deref(),
                Some("catppuccin-frappe")
            );
        });
        cx.update(|window, cx| {
            let input = panel.read(cx).theme_search.clone().unwrap();
            input.update(cx, |input, cx| input.set_value("catppuccin", window, cx));
        });
        draw(cx);
        // Force a real atomic-save failure without changing process environment.
        let path = grove_core::storage::config_path().unwrap();
        let backup = path.with_extension("theme-test-backup");
        fs_err::rename(&path, &backup).unwrap();
        fs_err::create_dir(&path).unwrap();
        let restore_file = RestoreFile(path.clone(), backup.clone());
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.select_theme("catppuccin-mocha", window, cx);
            });
        });
        cx.update(|_, cx| {
            assert_eq!(
                panel.read(cx).error.as_deref(),
                Some(
                    "Couldn’t save Catppuccin Mocha. Catppuccin Frappé is still active. Try again."
                )
            );
            assert_eq!(panel.read(cx).theme_retry, Some("catppuccin-mocha"));
            assert_eq!(grove_core::theme::current().name, "catppuccin-frappe");
            assert_eq!(
                cx.global::<SettingsState>().store.selected_theme.as_deref(),
                Some("catppuccin-frappe")
            );
        });
        draw(cx);
        for (kind_selector, text_selector) in [
            (
                "theme-kind-catppuccin-latte",
                "theme-kind-text-catppuccin-latte",
            ),
            (
                "theme-kind-catppuccin-frappe",
                "theme-kind-text-catppuccin-frappe",
            ),
            (
                "theme-kind-catppuccin-macchiato",
                "theme-kind-text-catppuccin-macchiato",
            ),
            (
                "theme-kind-catppuccin-mocha",
                "theme-kind-text-catppuccin-mocha",
            ),
        ] {
            let column = cx
                .debug_bounds(kind_selector)
                .expect("filtered kind column");
            let text = cx
                .debug_bounds(text_selector)
                .expect("intrinsic text layout bounds");
            assert!(
                (f32::from(column.right() - text.right())).abs() < 0.1,
                "metadata text must occupy column trailing edge after search and save error"
            );
        }
        drop(restore_file);
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.select_theme("catppuccin-mocha", window, cx);
            });
        });
        cx.update(|window, cx| {
            assert!(panel.read(cx).error.is_none());
            assert!(panel.read(cx).theme_retry.is_none());
            assert!(panel.read(cx).theme_focus[6].is_focused(window));
            assert_eq!(
                cx.global::<ThemeState>().generation,
                grove_core::theme::generation()
            );
        });
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| assert_eq!(panel.read(cx).page, Page::Settings));
    }

    #[gpui::test]
    fn shortcuts_escape_closes_page_and_restores_focus(cx: &mut gpui::TestAppContext) {
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
        assert_eq!(f32::from(bounds.size.width), 320.0);
        assert_eq!(f32::from(bounds.size.height), 200.0);
        assert!(cx.debug_bounds("settings-overlay").is_none());
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            assert!(!panel.read(cx).is_open());
            assert!(prior.is_focused(window));
        });
    }

    #[gpui::test]
    fn nested_shortcuts_after_direct_shortcuts_return_to_general(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open_shortcuts(window, cx);
                panel.back(window, cx);
                panel.open_nested(Page::Shortcuts, window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|_, cx| {
            assert!(panel.read(cx).is_open());
            assert_eq!(panel.read(cx).page, Page::Settings);
            assert_eq!(panel.read(cx).category, Category::General);
        });
    }

    #[gpui::test]
    fn category_navigation_activates_with_keyboard_and_resets_scroll(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.open(window, cx)));
        draw(cx);
        // The category choices are the first keyboard stops, in displayed order.
        for chord in ["tab", "tab", "tab", "enter"] {
            cx.simulate_keystrokes(chord);
            draw(cx);
        }
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(panel.read(cx).category, Category::Agents);
            assert_eq!(panel.read(cx).page, Page::Settings);
            assert_eq!(panel.read(cx).scroll.offset(), gpui::Point::default());
        });
    }

    #[gpui::test]
    fn settings_headers_start_at_content_inset_at_zoom_and_short_heights(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        for (zoom, width, height) in [
            (1.0, 1280.0, 800.0),
            (1.1, 500.0, 200.0),
            (1.5, 600.0, 600.0),
        ] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
            cx.update(|window, _| window.set_rem_size(gpui::px(crate::zoom::REM_BASE * zoom)));
            for page in [Page::Settings, Page::Providers, Page::Themes] {
                cx.update(|window, cx| {
                    panel.update(cx, |panel, cx| {
                        panel.open(window, cx);
                        if page == Page::Settings {
                            panel.show(page, cx);
                        } else {
                            panel.open_nested(page, window, cx);
                        }
                    });
                });
                draw(cx);
                let chrome_bottom = gpui::px(SPACE_20 * zoom);
                let heading = cx
                    .debug_bounds("settings-heading")
                    .expect("settings heading");
                assert!(heading.top() >= chrome_bottom, "{page:?} at {zoom}x: heading {heading:?} precedes content inset ending at {chrome_bottom:?}");
                for selector in ["settings-close", "settings-back"] {
                    if selector == "settings-back" && page == Page::Settings {
                        continue;
                    }
                    let control = cx.debug_bounds(selector).expect("settings header control");
                    assert!(
                        control.top() >= chrome_bottom,
                        "{selector} precedes content inset at {zoom}x"
                    );
                    assert!(
                        control.bottom() <= gpui::px(height),
                        "{selector} clips outside short window at {zoom}x"
                    );
                }
                let title = cx
                    .debug_bounds("settings-heading-title")
                    .expect("settings title");
                let close = cx.debug_bounds("settings-close").unwrap();
                assert!(title.right() <= close.left(), "title must yield to Close");
                if page != Page::Settings {
                    let back = cx.debug_bounds("settings-back").unwrap();
                    assert!(back.right() <= title.left(), "Back must precede the title");
                }
                let scroll = cx
                    .debug_bounds("settings-scroll")
                    .expect("settings body viewport");
                assert!(scroll.top() >= heading.bottom());
                cx.update(|_, cx| {
                    panel
                        .read(cx)
                        .scroll
                        .set_offset(gpui::point(gpui::px(0.0), gpui::px(-120.0)));
                });
                draw(cx);
                assert_eq!(
                    cx.debug_bounds("settings-heading"),
                    Some(heading),
                    "body scrolling moved page header"
                );
            }
        }
    }

    #[gpui::test]
    fn settings_body_scrolls_while_heading_and_tabs_stay_reachable(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.open(window, cx)));
        cx.simulate_resize(gpui::size(gpui::px(320.0), gpui::px(200.0)));
        draw(cx);

        let scroll = cx.debug_bounds("settings-scroll").expect("scroll viewport");
        let heading = cx.debug_bounds("settings-heading").expect("local heading");
        let tabs = cx
            .debug_bounds("settings-category-navigation")
            .expect("tabs");
        let close = cx.debug_bounds("settings-close").expect("close");
        assert_eq!(tabs.bottom(), scroll.top());
        assert!(scroll.bottom() <= gpui::px(200.));
        assert!(close.top() >= heading.top() && close.bottom() <= heading.bottom());
        assert!(cx.debug_bounds("settings-footer").is_none());
        assert!(panel.read_with(cx, |panel, _| panel.scroll.max_offset().y) > gpui::px(0.));

        cx.simulate_event(gpui::ScrollWheelEvent {
            position: scroll.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-120.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        draw(cx);
        assert!(panel.read_with(cx, |panel, _| panel.scroll.offset().y) < gpui::px(0.));
        assert_eq!(cx.debug_bounds("settings-heading"), Some(heading));
        assert_eq!(cx.debug_bounds("settings-category-navigation"), Some(tabs));
        assert_eq!(cx.debug_bounds("settings-close"), Some(close));
        cx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.error = Some("Could not save settings.".into());
                cx.notify();
            });
        });
        draw(cx);
        let error = cx
            .debug_bounds("settings-error")
            .expect("visible save error");
        let scroll = cx.debug_bounds("settings-scroll").expect("bounded scroll");
        assert!(error.top() >= tabs.bottom());
        assert_eq!(error.bottom(), scroll.top());
        assert!(error.bottom() < gpui::px(200.));
        assert_eq!(cx.debug_bounds("settings-close"), Some(close));
    }

    #[gpui::test]
    fn desktop_tabs_and_body_share_one_centered_content_column(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.update(|window, cx| panel.update(cx, |panel, cx| panel.open(window, cx)));
        cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
        draw(cx);
        let content = cx.debug_bounds("settings-content").expect("content column");
        let heading = cx.debug_bounds("settings-heading").expect("heading");
        let tabs = cx
            .debug_bounds("settings-category-navigation")
            .expect("tabs");
        let body = cx.debug_bounds("settings-body").expect("body");
        let general = cx
            .debug_bounds("settings-category-General")
            .expect("General tab");
        let agents = cx
            .debug_bounds("settings-category-Agents")
            .expect("Agents tab");
        assert_eq!(content.size.width, gpui::px(MODAL_W_XL));
        assert_eq!(content.center().x, gpui::px(640.));
        assert_eq!(heading.left(), body.left());
        assert_eq!(tabs.left(), body.left());
        assert_eq!(tabs.size.width, body.size.width);
        assert_eq!(general.top(), agents.top());
        assert_eq!(agents.left(), general.right());
        assert!(tabs.bottom() <= body.top());
    }

    #[gpui::test]
    fn provider_notice_opens_agents_nested_page_and_escape_returns(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| panel.open_provider_updates(window, cx));
            assert_eq!(panel.read(cx).page, Page::Providers);
            assert_eq!(panel.read(cx).category, Category::Agents);
        });
        draw(cx);
        assert!(
            cx.debug_bounds("settings-save-note").is_none(),
            "Provider versions has no save-preference footer"
        );
        cx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.toggle_provider_details(Agent::Codex, cx);
            });
            let updates = panel.read(cx).runtime.read(cx).provider_updates.clone();
            updates.update(cx, |_, cx| cx.notify());
        });
        draw(cx);
        cx.update(|_, cx| {
            assert!(
                panel
                    .read(cx)
                    .provider_details
                    .contains(Agent::Codex.label()),
                "Refresh preserves disclosure by provider identity"
            );
        });
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|_, cx| {
            assert!(panel.read(cx).is_open());
            assert_eq!(panel.read(cx).page, Page::Settings);
            assert_eq!(panel.read(cx).category, Category::Agents);
        });
    }

    #[gpui::test]
    fn provider_rows_keep_codex_and_claude_actions_compact_and_errors_keyboard_accessible(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::entities::provider_updates::ProviderRow;
        use grove_core::provider_updates::{Installer, ProviderStatus};
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| panel.open_provider_updates(window, cx));
            let updates = panel.read(cx).runtime.read(cx).provider_updates.clone();
            updates.update(cx, |updates, cx| {
                updates.set_rows_for_test(
                    [Agent::Codex, Agent::Claude]
                        .into_iter()
                        .map(|agent| ProviderRow {
                            status: ProviderStatus {
                                agent,
                                installed: Some("1.0.0".into()),
                                latest: Some("1.1.0".into()),
                                executable: Some("/fixture/provider".into()),
                                error: None,
                                installer: if agent == Agent::Claude {
                                    Installer::ClaudeNative
                                } else {
                                    Installer::Npm {
                                        executable: "/fixture/npm".into(),
                                        prefix: "/fixture".into(),
                                        package: "@openai/codex".into(),
                                    }
                                },
                            },
                            checking: false,
                            updating: false,
                            updated: false,
                            update_verified: false,
                            last_success: Some(1),
                            error: if agent == Agent::Claude {
                                Some("Permission denied".into())
                            } else {
                                None
                            },
                        })
                        .collect(),
                    cx,
                );
            });
        });
        draw(cx);
        let codex = cx
            .debug_bounds("settings-provider-codex-update")
            .expect("Codex Update");
        let claude = cx
            .debug_bounds("settings-provider-claude-update")
            .expect("Claude Retry");
        assert_eq!(
            codex.right(),
            claude.right(),
            "Both providers reserve the same action edge"
        );
        assert_eq!(codex.size.height, claude.size.height);
        let manage = cx.debug_bounds("settings-provider-claude-manage").unwrap();
        cx.simulate_click(manage.center(), gpui::Modifiers::default());
        draw(cx);
        let details = cx
            .debug_bounds("settings-provider-claude-details")
            .expect("error disclosure");
        cx.simulate_click(details.center(), gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| {
            assert!(panel
                .read(cx)
                .provider_details
                .contains(Agent::Claude.label()));
        });
        cx.simulate_keystrokes("space");
        draw(cx);
        cx.update(|_, cx| {
            assert!(!panel
                .read(cx)
                .provider_details
                .contains(Agent::Claude.label()));
        });
        assert!(
            cx.debug_bounds("settings-provider-codex-details").is_none(),
            "No routine disclosure"
        );
    }

    #[gpui::test]
    fn provider_page_back_restores_source_focus(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open(window, cx);
                panel.select_category(Category::Agents, cx);
                let source = cx.focus_handle().tab_stop(true);
                source.focus(window, cx);
                panel.open_nested(Page::Providers, window, cx);
                assert!(panel
                    .nested_return_focus
                    .as_ref()
                    .is_some_and(|focus| focus == &source));
                panel.back(window, cx);
                assert!(source.is_focused(window));
                assert_eq!(panel.category, Category::Agents);
            });
        });
    }

    #[gpui::test]
    fn nested_settings_page_escape_returns_to_settings(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open(window, cx);
                panel.select_category(Category::About, cx);
                panel.open_nested(Page::Changelog, window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|_, cx| {
            assert!(panel.read(cx).is_open());
            assert_eq!(panel.read(cx).page, Page::Settings);
            assert_eq!(panel.read(cx).category, Category::About);
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
                assert!(!behind.is_focused(window), "{chord} escaped the page");
                assert!(panel.read(cx).focus.contains_focused(window, cx));
            });
        }
    }
}
