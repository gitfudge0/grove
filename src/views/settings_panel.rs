//! App-wide settings and shortcut reference, mounted in the main canvas.
mod project_creation;
mod projects;

mod shortcuts;

mod appearance;

mod providers;

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
    Pixels, ScrollHandle, SharedString, Subscription, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Disableable;
use grove_core::{
    agent::Agent,
    storage::{AfterSessionStart, Store},
};
use grove_core::{app_theme, theme::ThemeKind};
use std::{cell::Cell, collections::HashSet, rc::Rc};

const ENVIRONMENT_ACTION_W: f32 = 176.0;
const CATEGORY_RAIL_W: f32 = 172.0;
const SETTINGS_PAGE_W: f32 = MODAL_W_XL + SPACE_2XL;
const SETTINGS_COMPACT_W: f32 = 620.0;
const SETTINGS_HEADER_WIDE_H: f32 = 58.0;
const SETTINGS_HEADER_COMPACT_H: f32 = 50.0;
const SETTINGS_HEADER_WIDE_INSET: f32 = 38.0;
const SETTINGS_PAGE_TITLE_WIDE_SIZE: f32 = 26.0;
const SETTINGS_PAGE_TITLE_COMPACT_SIZE: f32 = 23.0;
const FOCUS_SCAN_LIMIT: usize = 128;
const THEME_ROW_H: f32 = 42.0;
const THEME_KIND_W: f32 = 36.0;
const THEME_SWATCH_RADIUS: f32 = 2.0;
const SHORTCUT_KEY_W: f32 = FIELD_LABEL_COL_W + SPACE_20 * 2.0;

pub(super) fn settings_page_title_size(compact: bool) -> f32 {
    if compact {
        SETTINGS_PAGE_TITLE_COMPACT_SIZE
    } else {
        SETTINGS_PAGE_TITLE_WIDE_SIZE
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Category {
    #[default]
    General,
    Agents,
    Projects,
    Terminal,
    Privacy,
    About,
}

impl Category {
    const ALL: [Self; 6] = [
        Self::General,
        Self::Agents,
        Self::Projects,
        Self::Terminal,
        Self::Privacy,
        Self::About,
    ];

    fn icon(self) -> &'static str {
        match self {
            Self::General => "cog",
            Self::Agents => "brain",
            Self::Projects => "folder",
            Self::Terminal => "terminal",
            Self::Privacy => "lock",
            Self::About => "status-info",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Agents => "Agents",
            Self::Projects => "Projects",
            Self::Terminal => "Terminal",
            Self::Privacy => "Privacy",
            Self::About => "About",
        }
    }
    fn description(self) -> Option<&'static str> {
        match self {
            Self::General => Some("Appearance and keyboard shortcuts"),
            Self::Agents => Some("Provider accounts, new sessions, and access"),
            Self::Projects => None,
            Self::Terminal => Some("Terminal backend and shell environment"),
            Self::Privacy => Some("Usage data and diagnostics"),
            Self::About => Some("Grove version, updates, and release notes"),
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
    UseCheckout { root: String, path: String },
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
    Project,
    ProjectCreation,
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
    canvas_width: Rc<Cell<Option<Pixels>>>,
    category_rail_focus: [FocusHandle; Category::ALL.len()],
    category_rail_scroll: ScrollHandle,
    category_trigger_focus: FocusHandle,
    category_trigger_bounds: Rc<Cell<gpui::Bounds<Pixels>>>,
    category_menu: Option<Entity<PopupMenu>>,
    category_menu_subscription: Option<Subscription>,
    nested_return_focus: Option<FocusHandle>,
    standalone_shortcuts: bool,
    error: Option<String>,
    scroll: ScrollHandle,
    creation_panel: Option<project_creation::CreationPanel>,
    creation_subscription: Option<Subscription>,
    creation_observer: Option<Subscription>,
    creation_from_shortcut: bool,
    archived_panel: Option<Entity<super::sidebar::projects::ProjectPanel>>,
    archived_subscription: Option<Subscription>,
    archived_observer: Option<Subscription>,
    expanded_projects: HashSet<String>,
    project_disclosure_focus: std::collections::HashMap<String, FocusHandle>,
    add_project_focus: FocusHandle,
    project_reveal: Option<projects::ProjectReveal>,
    project_inventory: std::collections::HashMap<String, projects::WorktreeInventory>,
    _project_events: Subscription,
    project_menu: Option<projects::ProjectRowMenu>,
    project_menu_focus: std::collections::HashMap<projects::ProjectMenuTarget, FocusHandle>,
    project_action_return: Option<projects::ProjectActionReturn>,
    _project_tree_observer: Subscription,
    projects_archived: bool,
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
            runtime: runtime.clone(),
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
            canvas_width: Rc::new(Cell::new(None)),
            category_rail_focus: Category::ALL.map(|_| cx.focus_handle().tab_stop(true)),
            category_rail_scroll: ScrollHandle::new(),
            category_trigger_focus: cx.focus_handle().tab_stop(true),
            category_trigger_bounds: Rc::new(Cell::new(gpui::Bounds::default())),
            category_menu: None,
            category_menu_subscription: None,
            nested_return_focus: None,
            standalone_shortcuts: false,
            error: None,
            scroll: scroll.clone(),
            theme_anchors: app_theme::CATALOG
                .iter()
                .map(|_| gpui::ScrollAnchor::for_handle(scroll.clone()))
                .collect(),
            show_theme_notices: false,
            expanded_projects: HashSet::new(),
            project_disclosure_focus: std::collections::HashMap::new(),
            add_project_focus: cx.focus_handle().tab_stop(true),
            project_reveal: None,
            project_inventory: std::collections::HashMap::new(),
            _project_events: {
                use crate::project_service::ProjectEvent;
                let service = runtime.read(cx).projects.clone();
                cx.subscribe(&service, |this, _, event, cx| match event {
                    ProjectEvent::TreeInvalidated
                    | ProjectEvent::WorktreeAdded { .. }
                    | ProjectEvent::WorktreeRemoved { .. }
                    | ProjectEvent::ProjectRemoved { .. } => this.refresh_expanded_projects(cx),
                    _ => {}
                })
            },
            project_menu: None,
            project_menu_focus: std::collections::HashMap::new(),
            project_action_return: None,
            _project_tree_observer: {
                let tree = runtime.read(cx).tree.clone();
                cx.observe(&tree, |_, _, cx| cx.notify())
            },
            projects_archived: false,
            creation_panel: None,
            creation_subscription: None,
            creation_observer: None,
            creation_from_shortcut: false,
            archived_panel: None,
            archived_subscription: None,
            archived_observer: None,
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

    pub(crate) fn open_projects(
        &mut self,
        path: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project_operation_pending(cx) {
            return;
        }
        self.open(window, cx);
        self.select_category(Category::Projects, cx);
        if let Some(path) = path {
            self.open_project_page(super::sidebar::projects::Page::Edit(path), window, cx);
        }
    }

    fn open_project_page(
        &mut self,
        page: super::sidebar::projects::Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.nested_return_focus = window.focused(cx);
        let compact = self.compact_layout(window);
        let runtime = self.runtime.read(cx);
        let (service, registry) = (runtime.projects.clone(), runtime.registry.clone());
        let panel = cx.new(|cx| {
            super::sidebar::projects::ProjectPanel::new(page, service, registry, window, cx)
                .in_settings(compact)
        });
        self.archived_subscription =
            Some(
                cx.subscribe_in(&panel, window, |this, _, event, window, cx| {
                    use super::sidebar::projects::ProjectPanelEvent;
                    match event {
                        ProjectPanelEvent::Closed | ProjectPanelEvent::Selected(_) => {
                            this.back(window, cx);
                        }
                        ProjectPanelEvent::Saved(name) => {
                            this.back(window, cx);
                            this.project_receipt(format!("Saved {name} settings"), cx);
                        }
                        ProjectPanelEvent::Archived => {
                            this.back(window, cx);
                            this.project_receipt("Archived project", cx);
                        }
                        ProjectPanelEvent::Moved {
                            name, workspace, ..
                        } => {
                            this.back(window, cx);
                            this.project_receipt(format!("Moved {name} to {workspace}"), cx);
                        }
                        ProjectPanelEvent::Decision(_) => {}
                    }
                }),
            );
        self.archived_observer = Some(cx.observe(&panel, |_, _, cx| cx.notify()));
        self.archived_panel = Some(panel);
        self.show(Page::Project, cx);
    }

    pub(crate) fn open_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open(window, cx);
        self.select_category(Category::About, cx);
    }

    pub(crate) fn open_provider_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_operation_pending(cx) {
            return;
        }
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
        if self.project_operation_pending(cx) {
            return;
        }
        if !self.open {
            self.return_focus = source_focus;
        }
        self.open = true;
        self.select_category(Category::Agents, cx);
        self.page = Page::Connections;
        self.connections_pending_focus = true;
        self.scroll.set_offset(gpui::Point::default());
        self.connections.update(cx, |panel, cx| {
            panel.select_context(provider, cwd, args, cx);
        });
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open || self.project_operation_pending(cx) {
            return;
        }
        self.dismiss_projects_menu(window, cx);
        self.category_menu = None;
        self.category_menu_subscription = None;
        self.project_action_return = None;
        self.open = false;
        self.clear_creation();
        self.project_reveal = None;
        self.project_inventory.clear();
        self.archived_panel = None;
        self.archived_subscription = None;
        self.archived_observer = None;
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
        if self.project_operation_pending(cx) {
            return;
        }
        self.clear_creation();
        self.archived_panel = None;
        self.archived_subscription = None;
        self.archived_observer = None;
        self.project_menu = None;
        self.project_action_return = None;
        self.category_menu = None;
        self.category_menu_subscription = None;
        self.category = category;
        self.nested_return_focus = None;
        self.standalone_shortcuts = false;
        self.show(Page::Settings, cx);
        if category == Category::Projects {
            self.refresh_expanded_projects(cx);
        }
    }

    fn open_nested(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.standalone_shortcuts = false;
        self.nested_return_focus = window.focused(cx);
        if page == Page::Archived {
            let compact = self.compact_layout(window);
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
                .in_settings(compact)
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
            self.archived_observer = Some(cx.observe(&panel, |_, _, cx| cx.notify()));
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
        if self.project_operation_pending(cx) {
            return;
        }
        self.clear_creation();
        self.archived_panel = None;
        self.archived_subscription = None;
        self.archived_observer = None;
        self.standalone_shortcuts = false;
        self.show(Page::Settings, cx);
        if let Some(focus) = self.nested_return_focus.take() {
            focus.focus(window, cx);
        }
        self.restore_project_action_focus(window, cx);
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
        if self.category_menu.is_some() {
            if matches!(event.keystroke.key.as_str(), "escape" | "tab") {
                self.dismiss_category_menu(window, cx);
                window.prevent_default();
                cx.stop_propagation();
            }
            return;
        }
        if self.handle_projects_menu_key(event, window, cx) {
            return;
        }
        if let Some(current) = self
            .category_rail_focus
            .iter()
            .position(|focus| focus.is_focused(window))
        {
            let count = Category::ALL.len();
            let next = match event.keystroke.key.as_str() {
                "up" => Some((current + count - 1) % count),
                "down" => Some((current + 1) % count),
                "home" => Some(0),
                "end" => Some(count - 1),
                _ => None,
            };
            if let Some(next) = next {
                self.category_rail_focus[next].focus(window, cx);
                self.category_rail_scroll.scroll_to_item(next);
                window.prevent_default();
                cx.stop_propagation();
                return;
            }
        }
        if event.keystroke.key == "escape"
            && self
                .archived_panel
                .as_ref()
                .is_some_and(|panel| panel.focus_handle(cx).contains_focused(window, cx))
        {
            // Project editing owns its nested decisions and their cancellation.
            return;
        }
        if matches!(
            &self.creation_panel,
            Some(project_creation::CreationPanel::Project(panel))
                if panel.read(cx).owns_settings_key(&event.keystroke, window, cx)
        ) {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" if self.page == Page::ProjectCreation => self.cancel_creation(window, cx),
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
                if let Some(index) = self
                    .category_rail_focus
                    .iter()
                    .position(|focus| focus.is_focused(window))
                {
                    self.category_rail_scroll.scroll_to_item(index);
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
            .border_b_1()
            .border_color(c::BORDER_SOFT())
            .text_size(rpx(TEXT_SMALL))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c::FG())
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
            .border_b_1()
            .border_color(c::BORDER_SOFT())
            .child(
                div()
                    .min_w(rpx(FIELD_LABEL_COL_W))
                    .max_w_full()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .text_size(rpx(TEXT_BODY))
                            .font_weight(FontWeight::SEMIBOLD)
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

    fn settings_body(&self, window: &Window, compact: bool, cx: &mut Context<Self>) -> gpui::Div {
        if self.category == Category::Projects {
            return self.projects_body(window, compact, cx);
        }
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
            .child(crate::icons::icon("chev-right", ICON_XS, c::FG_DIM()))
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
            .child(Self::setting_row("Shell environment", if cfg!(windows) { "Shell environment refresh is unavailable on Windows." } else { "Refresh the environment for new sessions." }, environment_control));

        body = body.when(self.category == Category::Agents, |body| body.child(
            div().flex().flex_col()
                .child(Self::section("Provider tools"))
                .child(Self::setting_row("Providers", "Sign in to Codex or Claude Code and manage provider updates.",
                    self.button("settings-providers", "Manage providers", false, true)
                        .child(crate::icons::icon("arrow-right", ICON_SM, c::FG_DIM()))
                        .child("Manage providers")
                        .on_click(cx.listener(|this, _, window, cx| this.open_nested(Page::Providers, window, cx)))))
                .child(Self::section("Sessions"))
                .child(Self::setting_row(
                    "After starting a session",
                    "Choose where you go after sending your first message",
                    after_session_start_control,
                ))
                .child(Self::section("New sessions"))
                .child(Self::setting_row("Run agents in terminal", "Use the Codex and Claude Code command-line interfaces instead of Grove’s conversation view. Applies to new sessions.", terminal_agents_control))
                .when(cfg!(windows), |body| body.child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child("Windows supports terminal agents only.")))
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
                    )),
            )
        })
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

impl SettingsPanel {
    pub(super) fn project_operation_pending(&self, cx: &App) -> bool {
        self.archived_panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).is_pending(cx))
            || self
                .creation_panel
                .as_ref()
                .is_some_and(|panel| panel.is_pending(cx))
    }

    fn dismiss_category_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.category_menu.take().is_some() {
            self.category_menu_subscription = None;
            self.category_trigger_focus.focus(window, cx);
            cx.notify();
        }
    }

    fn toggle_category_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_operation_pending(cx) {
            return;
        }
        self.dismiss_projects_menu(window, cx);
        if self.category_menu.is_some() {
            self.dismiss_category_menu(window, cx);
            return;
        }

        let owner = cx.entity().downgrade();
        let selected = self.category;
        let popup = PopupMenu::build(window, cx, move |mut menu, window, _| {
            let margin = rpx(SPACE_LG).to_pixels(window.rem_size());
            menu = components::dropdown_menu(menu, window)
                .min_w(rpx(CATEGORY_RAIL_W).to_pixels(window.rem_size()))
                .max_h(window.viewport_size().height - margin * 2.)
                .scrollable(true);
            for category in Category::ALL {
                let owner = owner.clone();
                menu = menu.item(
                    PopupMenuItem::new(category.label())
                        .icon(
                            gpui_component::Icon::default()
                                .path(format!("icons/{}.svg", category.icon())),
                        )
                        .checked(selected == category)
                        .on_click(move |_, window, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                this.dismiss_category_menu(window, cx);
                                this.select_category(category, cx);
                            });
                        }),
                );
            }
            menu
        });
        self.category_menu_subscription = Some(cx.subscribe_in(
            &popup,
            window,
            |this, _, _: &gpui::DismissEvent, window, cx| {
                this.dismiss_category_menu(window, cx);
            },
        ));
        popup.focus_handle(cx).focus(window, cx);
        self.category_menu = Some(popup);
        cx.notify();
    }

    fn category_navigation(
        &self,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let enabled = !self.project_operation_pending(cx);
        let main = div()
            .id("settings-category-navigation")
            .debug_selector(|| "settings-category-navigation".into())
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_color(c::BORDER_SOFT());
        if compact {
            let bounds = self.category_trigger_bounds.clone();
            return main
                .w_full()
                .border_b_1()
                .px(rpx(SPACE_3XL))
                .py(rpx(SPACE_XL))
                .child(
                    self.button(
                        "settings-category-picker",
                        format!("Settings category: {}", self.category.label()),
                        self.category_menu.is_some(),
                        enabled,
                    )
                    .debug_selector(|| "settings-category-picker".into())
                    .track_focus(&self.category_trigger_focus)
                    .aria_expanded(self.category_menu.is_some())
                    .w_full()
                    .h(rpx(CONTROL_H + SPACE_2XL))
                    .justify_start()
                    .text_size(rpx(TEXT_BODY))
                    .text_color(if enabled { c::FG() } else { c::FG_DIM() })
                    .child(
                        gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                            .absolute()
                            .inset_0(),
                    )
                    .child(crate::icons::icon(
                        self.category.icon(),
                        ICON_MD,
                        c::FG_DIM(),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(self.category.label()),
                    )
                    .child(crate::icons::icon("chev-down", ICON_SM, c::FG_DIM()))
                    .when(enabled, |trigger| {
                        trigger.on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_category_menu(window, cx);
                        }))
                    }),
                );
        }

        main.w(rpx(CATEGORY_RAIL_W))
            .min_h_0()
            .border_r_1()
            .overflow_y_scroll()
            .track_scroll(&self.category_rail_scroll)
            .pt(rpx(SPACE_20 + SPACE_XS))
            .px(rpx(SPACE_2XL + SPACE_XS))
            .pb(rpx(SPACE_LG))
            .gap(rpx(SPACE_SM))
            .children(
                Category::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, category)| {
                        let selected = self.category == category;
                        div()
                            .id(format!("settings-category-{}", category.label()))
                            .debug_selector(move || {
                                format!("settings-category-{}", category.label())
                            })
                            .role(gpui::Role::Button)
                            .aria_label(category.label())
                            .aria_selected(selected)
                            .w_full()
                            .h(rpx(CONTROL_H + SPACE_2XL))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .rounded(rpx(RADIUS_GROUP))
                            .border_1()
                            .border_color(gpui::transparent_black())
                            .px(rpx(SPACE_LG))
                            .gap(rpx(SPACE_LG))
                            .text_size(rpx(TEXT_BODY))
                            .font_weight(if selected {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .text_color(if selected { c::FG() } else { c::FG_DIM() })
                            .when(selected, |row| row.bg(c::BG_HOVER()))
                            .child(crate::icons::icon(
                                category.icon(),
                                ICON_MD,
                                if selected { c::FG() } else { c::FG_DIM() },
                            ))
                            .child(category.label())
                            .when(enabled, |row| {
                                row.track_focus(&self.category_rail_focus[index])
                                    .cursor_pointer()
                                    .hover(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
                                    .focus_visible(|style| {
                                        style
                                            .bg(c::FIELD_FILL())
                                            .border_1()
                                            .border_color(c::BORDER_STRONG())
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.dismiss_projects_menu(window, cx);
                                        this.select_category(category, cx);
                                    }))
                            })
                    }),
            )
    }

    fn page_description(&self, cx: &App) -> Option<SharedString> {
        if self.page == Page::Settings && self.category == Category::Projects {
            let workspaces = &cx.global::<SettingsState>().store.workspaces;
            let workspace = workspaces.name(workspaces.active);
            return Some(format!("Project registrations in the {workspace} workspace").into());
        }
        let description = match self.page {
            Page::Settings => self.category.description(),
            Page::Themes => Some("Theme changes apply to the entire app."),
            Page::Shortcuts => Some("Commands for the current view"),
            Page::Providers
            | Page::Changelog
            | Page::Connections
            | Page::Archived
            | Page::Project
            | Page::ProjectCreation => None,
        };
        description.map(SharedString::from)
    }

    fn compact_layout(&self, window: &Window) -> bool {
        let width = self
            .canvas_width
            .get()
            .unwrap_or_else(|| window.viewport_size().width);
        width < rpx(SETTINGS_COMPACT_W).to_pixels(window.rem_size())
    }
}

impl Focusable for SettingsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.category == Category::Projects {
            let paths: Vec<_> = cx
                .global::<SettingsState>()
                .store
                .projects
                .iter()
                .map(|p| p.path.clone())
                .collect();
            self.project_disclosure_focus
                .retain(|path, _| paths.contains(path));
            for path in paths {
                self.project_disclosure_focus
                    .entry(path)
                    .or_insert_with(|| cx.focus_handle().tab_stop(true));
            }
        }
        self.sync_project_action_focus(window, cx);
        if self.connections_pending_focus {
            self.connections_pending_focus = false;
            self.focus.focus(window, cx);
        }

        let compact = self.compact_layout(window);
        if let Some(panel) = &self.creation_panel {
            panel.update_layout(compact, cx);
        }
        if let Some(panel) = &self.archived_panel {
            panel.update(cx, |panel, cx| panel.update_settings_layout(compact, cx));
        }
        let observed_width = self.canvas_width.clone();
        let owner = cx.entity().downgrade();
        let measure = gpui::canvas(
            move |bounds, window, cx| {
                observed_width.set(Some(bounds.size.width));
                let next_compact =
                    bounds.size.width < rpx(SETTINGS_COMPACT_W).to_pixels(window.rem_size());
                if compact != next_compact {
                    window.defer(cx, move |window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.dismiss_category_menu(window, cx);
                            if next_compact
                                && this
                                    .category_rail_focus
                                    .iter()
                                    .any(|focus| focus.is_focused(window))
                            {
                                this.category_trigger_focus.focus(window, cx);
                            } else if !next_compact
                                && this.category_trigger_focus.is_focused(window)
                            {
                                this.category_rail_focus[this.category as usize].focus(window, cx);
                            }
                            cx.notify();
                        });
                    });
                }
            },
            |_, (), _, _| {},
        )
        .absolute()
        .inset_0();

        let enabled = !self.project_operation_pending(cx);
        let header = components::content_header()
            .id("settings-heading")
            .debug_selector(|| "settings-heading".into())
            .h(rpx(if compact {
                SETTINGS_HEADER_COMPACT_H
            } else {
                SETTINGS_HEADER_WIDE_H
            }))
            .px(rpx(if compact {
                SPACE_3XL
            } else {
                SETTINGS_HEADER_WIDE_INSET
            }))
            .border_b_1()
            .border_color(c::BORDER_SOFT())
            .child(crate::icons::icon("cog", ICON_LG, c::FG_DIM()))
            .child(
                components::content_header_title("Settings")
                    .debug_selector(|| "settings-heading-title".into()),
            )
            .child(
                components::header_control_with_label_tooltip("settings-close", "Close settings")
                    .debug_selector(|| "settings-close".into())
                    .child(crate::icons::icon("close", ICON_MD, c::FG_DIM()))
                    .when(!enabled, |control| {
                        control.opacity(OPACITY_DISABLED).cursor_default()
                    })
                    .when(enabled, |control| {
                        control
                            .tab_index(0)
                            .focus_visible(|style| style.bg(c::BG_HOVER()))
                            .on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))
                    }),
            );
        let page = div()
            .flex()
            .flex_col()
            .w_full()
            .max_w(rpx(SETTINGS_PAGE_W))
            .min_w_0()
            .min_h_0();
        let page = if let Some(panel) = &self.creation_panel {
            use project_creation::CreationPanel;
            let panel = match panel {
                CreationPanel::Project(panel) => gpui::AnyView::from(panel.clone()),
                CreationPanel::Worktree(panel) => gpui::AnyView::from(panel.clone()),
            };
            page.child(
                div().flex().flex_shrink_0().pt(rpx(SPACE_LG)).child(
                    components::back_action("settings-project-creation-back", "Back", window, cx)
                        .disabled(!enabled)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.cancel_creation(window, cx)),
                        ),
                ),
            )
            .child(
                div()
                    .relative()
                    .min_h_0()
                    .flex_1()
                    .child(div().absolute().inset_0().child(panel)),
            )
        } else if matches!(self.page, Page::Archived | Page::Project) {
            page.child(
                div()
                    .id("settings-archived-page")
                    .relative()
                    .min_h_0()
                    .flex_1()
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .when_some(self.archived_panel.clone(), gpui::ParentElement::child),
                    ),
            )
        } else {
            let (title, body) = match self.page {
                Page::Settings => (
                    self.category.label(),
                    self.settings_body(window, compact, cx),
                ),
                Page::Shortcuts => ("Shortcuts", self.shortcuts_body(cx)),
                Page::Changelog => ("Changelog", self.changelog_body(window, cx)),
                Page::Themes => ("Themes", self.themes_body(cx)),
                Page::Providers => ("Providers", self.providers_body(window, cx)),
                Page::Connections => ("Connections", div().child(self.connections.clone())),
                Page::Archived | Page::Project | Page::ProjectCreation => unreachable!(),
            };
            let description = self.page_description(cx);
            let title_size = settings_page_title_size(compact);
            let description_gap = if self.page == Page::Settings {
                if self.category == Category::Projects {
                    if compact {
                        SPACE_20
                    } else {
                        SPACE_20 + SPACE_LG
                    }
                } else {
                    SPACE_LG
                }
            } else {
                SPACE_3XL
            };
            page.when(self.page != Page::Settings, |column| {
                column.child(
                    div().flex().flex_shrink_0().pt(rpx(SPACE_LG)).child(
                        components::back_action(
                            "settings-back",
                            format!("Back to {}", self.category.label()),
                            window,
                            cx,
                        )
                        .debug_selector(|| "settings-back".into())
                        .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
                    ),
                )
            })
            .when_some(self.error.clone(), |column, error| {
                column.child(motion::fast(
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
                            .pt(rpx(if self.page != Page::Settings {
                                SPACE_LG
                            } else if compact {
                                SPACE_20
                            } else {
                                SPACE_20 + SPACE_LG
                            }))
                            .pb(rpx(SPACE_20))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .flex_wrap()
                                    .gap(rpx(SPACE_LG))
                                    .child(
                                        div()
                                            .id("settings-page-title")
                                            .debug_selector(|| "settings-page-title".into())
                                            .text_size(rpx(title_size))
                                            .line_height(rpx(title_size + SPACE_SM))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(c::FG())
                                            .child(title),
                                    )
                                    .when(self.page == Page::Providers, |heading| {
                                        heading.child(self.check_all_providers_action(window, cx))
                                    }),
                            )
                            .when_some(description, |body, description| {
                                body.child(
                                    div()
                                        .pt(rpx(SPACE_SM))
                                        .pb(rpx(description_gap))
                                        .text_size(rpx(TEXT_TITLE))
                                        .line_height(rpx(TEXT_TITLE + SPACE_MD))
                                        .text_color(c::FG_DIM())
                                        .child(description),
                                )
                            })
                            .child(body)
                            .when(
                                self.page == Page::Themes
                                    || self.page == Page::Settings
                                        && !matches!(
                                            self.category,
                                            Category::About | Category::Projects
                                        ),
                                |body| {
                                    body.child(
                                        div()
                                            .id("settings-save-note")
                                            .debug_selector(|| "settings-save-note".into())
                                            .pt(rpx(SPACE_3XL))
                                            .pb(rpx(SPACE_LG))
                                            .flex()
                                            .items_center()
                                            .gap(rpx(SPACE_SM))
                                            .text_size(rpx(TEXT_MICRO))
                                            .text_color(c::FG_DIM())
                                            .child(crate::icons::icon(
                                                "check",
                                                ICON_SM,
                                                c::FG_DIM(),
                                            ))
                                            .child("Changes save automatically"),
                                    )
                                },
                            ),
                    ),
            )
        };

        div()
            .id("settings-panel")
            .debug_selector(|| "settings-panel".into())
            .size_full()
            .min_w_0()
            .min_h_0()
            .relative()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(Self::key))
            .child(measure)
            .child(
                div()
                    .id("settings-content")
                    .debug_selector(|| "settings-content".into())
                    .w_full()
                    .h_full()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(
                        div()
                            .flex()
                            .when(compact, gpui::Styled::flex_col)
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .when(!self.standalone_shortcuts, |columns| {
                                columns.child(self.category_navigation(compact, cx))
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .min_w_0()
                                    .min_h_0()
                                    .justify_center()
                                    .px(rpx(if compact { SPACE_3XL } else { SPACE_20 }))
                                    .child(page),
                            ),
                    ),
            )
            .when_some(self.category_menu.clone(), |panel, popup| {
                let bounds = self.category_trigger_bounds.get();
                panel.child(gpui::deferred(
                    gpui::anchored()
                        .position_mode(gpui::AnchoredPositionMode::Window)
                        .position(gpui::point(bounds.left(), bounds.bottom()))
                        .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
                        .child(popup),
                ))
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests;
