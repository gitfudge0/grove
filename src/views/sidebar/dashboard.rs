//! Dashboard destination state; the composer remains the shared native editor.

mod destination_tray;

mod launch_scope;
use launch_scope::{dashboard_launch_args, scope_roots, validate_scope};

use super::*;
use crate::entities::codex_session::CodexSession;
use crate::views::codex_view::{CodexView, CodexViewEvent, CHAT_MAX_W};
use gpui_component::menu::{PopupMenu, PopupMenuItem};

#[cfg(test)]
mod scope_tests;
mod worktree_scope;
use worktree_scope::{ScopePicker, ScopePickerEvent, ScopeTarget};

const NO_PROJECT: usize = usize::MAX;
const WORKTREE_CACHE_FRESHNESS: std::time::Duration = std::time::Duration::from_secs(10);

fn fresh_dashboard_provider(
    codex_available: Option<bool>,
    claude_available: Option<bool>,
) -> grove_core::codex::Provider {
    if codex_available == Some(false) && claude_available == Some(true) {
        grove_core::codex::Provider::Claude
    } else {
        grove_core::codex::Provider::Codex
    }
}

fn detect_dashboard_provider() -> grove_core::codex::Provider {
    fresh_dashboard_provider(
        Agent::Codex.executable_available(),
        Agent::Claude.executable_available(),
    )
}

#[derive(Default)]
struct WorktreeDiscovery {
    scope: Option<(usize, String, u64)>,
    fetched_at: Option<std::time::Instant>,
    in_flight: Option<u64>,
}
impl WorktreeDiscovery {
    fn reusable(&self, scope: &(usize, String, u64), generation: u64, has_rows: bool) -> bool {
        self.scope.as_ref() == Some(scope)
            && (self.in_flight == Some(generation)
                || (has_rows
                    && self
                        .fetched_at
                        .is_some_and(|at| at.elapsed() < WORKTREE_CACHE_FRESHNESS)))
    }
}

fn dashboard_home_dir() -> String {
    dirs::home_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

const DASHBOARD_COMPOSER_TOP_FRACTION: f32 = 0.31;

/// Preferences for a fresh dashboard draft after adopting the previous one.
struct DashboardDefaults {
    project: usize,
    path: String,
    provider: grove_core::codex::Provider,
    composer: grove_core::codex::ComposerSettings,
    config_args: Vec<String>,
    worktrees: Option<Vec<(String, String)>>,
    handoffs: Vec<Entity<CodexSession>>,
    scope: Vec<ScopeTarget>,
    viewport_height: gpui::Pixels,
}
pub(super) struct Dashboard {
    pub session: Entity<CodexSession>,
    pub view: Entity<CodexView>,
    pub tray: Entity<DestinationTray>,
    provider_pending: bool,
    viewport_height: std::rc::Rc<std::cell::Cell<gpui::Pixels>>,
    _events: Vec<gpui::Subscription>,
}
pub(crate) enum DestinationEvent {
    Project(usize, String),
    Worktree(String),
    NewWorktree,
    NewProject,
    ManageProjects,
    ManageWorktrees,
    Home,
    RefreshWorktrees,
    AcrossWorktrees,
    ScopeChanged,
}
pub(crate) struct DestinationTray {
    projects: Vec<(usize, String, String)>,
    scope: Vec<ScopeTarget>,
    scope_picker: Option<Entity<ScopePicker>>,
    scope_events: Option<gpui::Subscription>,
    worktrees: Vec<(String, String)>,
    loading: bool,
    generation: u64,
    error: Option<String>,
    preserve_on_refresh: bool,
    worktree_discovery: WorktreeDiscovery,
    project: usize,
    pub(super) path: String,
    popup: Option<Entity<gpui_component::menu::PopupMenu>>,
    popup_events: Option<gpui::Subscription>,
    popup_motion: motion::PopupMotion,
    closing_popup: Option<(
        Entity<gpui_component::menu::PopupMenu>,
        gpui::Point<gpui::Pixels>,
        bool,
    )>,
    popup_generation: u64,
    hovered: [bool; 2],
    open_selector: Option<usize>,
    position: gpui::Point<gpui::Pixels>,
    bounds: [std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>; 2],
}
impl Sidebar {
    pub(crate) fn open_dashboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dashboard_timelines.clear_entrances();
        self.open_dashboard_with_defaults(None, window, cx);
    }
    #[cfg(test)]
    pub(crate) fn dashboard_destination_for_test(&self, cx: &App) -> Option<(usize, String)> {
        self.dashboard.as_ref().map(|dashboard| {
            let tray = dashboard.tray.read(cx);
            (tray.project, tray.path.clone())
        })
    }
    /// Select a launch destination without replacing the deferred composer or its draft.
    pub(crate) fn open_dashboard_for_project(
        &mut self,
        project: usize,
        expected_path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.open_dashboard_for_checkout(project, expected_path, expected_path, window, cx)
    }
    pub(super) fn open_dashboard_for_checkout(
        &mut self,
        project: usize,
        expected_path: &str,
        checkout_path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(path) = cx
            .global::<SettingsState>()
            .store
            .workspace_projects(self.active_workspace)
            .find(|(idx, value)| *idx == project && value.path == expected_path)
            .map(|(_, value)| value.path.clone())
        else {
            return false;
        };
        self.open_destination(super::global_navigation::Destination::Home, window, cx);
        if let Some(tray) = self
            .dashboard
            .as_ref()
            .map(|dashboard| dashboard.tray.clone())
        {
            tray.update(cx, |tray, cx| {
                tray.scope.clear();
                tray.project = project;
                tray.path = if checkout_path == expected_path {
                    path
                } else {
                    checkout_path.to_owned()
                };
                cx.notify();
            });
            self.refresh_dashboard_worktrees(cx);
            if checkout_path != expected_path {
                tray.update(cx, |tray, cx| {
                    if !tray.worktrees.iter().any(|(path, _)| path == checkout_path) {
                        let name = std::path::Path::new(checkout_path).file_name().map_or_else(
                            || checkout_path.to_owned(),
                            |name| name.to_string_lossy().into_owned(),
                        );
                        tray.worktrees.push((checkout_path.to_owned(), name));
                    }
                    cx.notify();
                });
            }
            self.refresh_dashboard_catalog(cx);
            if let Some(dashboard) = &self.dashboard {
                dashboard.view.focus_handle(cx).focus(window, cx);
            }
        }
        cx.notify();
        true
    }
    fn open_dashboard_with_defaults(
        &mut self,
        defaults: Option<DashboardDefaults>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.destination = super::global_navigation::Destination::Home;
        self.dismiss_project_panel_for_navigation();
        if let Some(settings) = &self.settings_panel {
            settings.update(cx, |settings, cx| settings.close(window, cx));
        }
        if self.dashboard.is_none() {
            let projects: Vec<_> = cx
                .global::<SettingsState>()
                .store
                .workspace_projects(self.active_workspace)
                .map(|(i, p)| (i, p.name.clone(), p.path.clone()))
                .collect();
            let selected = defaults
                .as_ref()
                .and_then(|defaults| projects.iter().find(|(idx, _, _)| *idx == defaults.project))
                .or_else(|| projects.first());
            let idx = defaults
                .as_ref()
                .filter(|d| d.project == NO_PROJECT)
                .map_or_else(|| selected.map_or(NO_PROJECT, |p| p.0), |_| NO_PROJECT);
            let project = cx
                .global::<SettingsState>()
                .store
                .projects
                .get(idx)
                .cloned();
            let project_path = project
                .as_ref()
                .map_or_else(dashboard_home_dir, |p| p.path.clone());
            let provider_pending =
                defaults.is_none() && grove_core::env_path::initial_environment_pending();
            let provider = defaults.as_ref().map_or_else(
                || {
                    if provider_pending {
                        grove_core::codex::Provider::Codex
                    } else {
                        detect_dashboard_provider()
                    }
                },
                |defaults| defaults.provider,
            );
            let checkout = defaults
                .as_ref()
                .filter(|defaults| defaults.project == idx)
                .map_or_else(|| project_path.clone(), |defaults| defaults.path.clone());
            let key = match grove_core::identity::random_id() {
                Ok(key) => key,
                Err(error) => {
                    self.content_error =
                        Some(format!("Could not create session identity: {error}"));
                    self.dashboard_open = true;
                    cx.notify();
                    return;
                }
            };
            let saved = grove_core::codex::SavedSession {
                html_policy: grove_core::visuals::HtmlPolicy::Explicit,
                visuals: Vec::new(),
                session_review: grove_core::codex::SessionReview::default(),
                rich_plans: grove_core::rich_plan::PlanStore::default(),
                side_chats: std::collections::HashMap::default(),
                fork_origin: None,
                claude_resume: None,
                claude_subagents: HashMap::default(),
                edit_checkpoints: Vec::new(),
                pending_edit: None,
                rewind_drafts: Vec::new(),
                provider,
                composer: defaults
                    .as_ref()
                    .map(|defaults| defaults.composer.clone())
                    .unwrap_or_default(),
                context: None,
                context_details: None,
                key,
                project: project.map(|p| p.name).unwrap_or_default(),
                cwd: checkout.clone(),
                label: provider.label().into(),
                title: None,
                title_generation: None,
                roots: defaults
                    .as_ref()
                    .map(|d| scope_roots(&d.scope))
                    .unwrap_or_default(),
                thread_id: None,
                items: Vec::new().into(),
                work_periods: Vec::new(),
                draft: String::new(),
                draft_images: Vec::new(),
                draft_reference: None,
                config_args: defaults
                    .as_ref()
                    .map(|defaults| defaults.config_args.clone())
                    .unwrap_or_default(),
                native_config_version: 1,
                activity_reporting: false,
            };
            let session = if provider_pending {
                cx.new(|cx| CodexSession::draft_pending(saved, cx))
            } else {
                let scope = defaults.as_ref().map_or(&[][..], |d| d.scope.as_slice());
                let args = dashboard_launch_args(provider, scope, &saved.cwd, cx);
                cx.new(|cx| CodexSession::draft(saved, args, cx))
            };
            let tray = cx.new(|_| {
                let mut tray = DestinationTray::new(projects, idx, checkout);
                if let Some(worktrees) = defaults
                    .as_ref()
                    .filter(|d| d.project == idx && d.path == tray.path)
                    .and_then(|d| d.worktrees.clone())
                {
                    tray.worktrees = worktrees;
                    tray.preserve_on_refresh = true;
                }
                tray.scope = defaults
                    .as_ref()
                    .map(|d| d.scope.clone())
                    .unwrap_or_default();
                tray
            });
            let view = cx.new(|cx| CodexView::new(session.clone(), window, cx));
            view.update(cx, |view, cx| {
                view.attach_dashboard_tray(tray.clone(), cx);
                if let Some(defaults) = &defaults {
                    for handoff in &defaults.handoffs {
                        view.track_dashboard_handoff(handoff.clone(), cx);
                    }
                }
            });
            let events = vec![
                cx.subscribe_in(&view, window, |this, source, event, window, cx| {
                    if !this.dashboard_open
                        || this
                            .dashboard
                            .as_ref()
                            .is_none_or(|draft| &draft.view != source)
                    {
                        return;
                    }
                    match event {
                        CodexViewEvent::OpenConnections(provider, cwd, args, focus) => {
                            if let Some(settings) = &this.settings_panel {
                                settings.update(cx, |panel, cx| {
                                    panel.request_connections(
                                        *provider,
                                        cwd.clone(),
                                        args.clone(),
                                        focus.clone(),
                                        cx,
                                    );
                                });
                            }
                            cx.notify();
                        }
                        CodexViewEvent::DashboardSubmit { open_transcript } => {
                            this.launch_dashboard(*open_transcript, window, cx);
                        }
                        CodexViewEvent::ReviewChanges(preference, focus) => {
                            this.invoke_review_skill(preference.clone(), focus.clone(), cx);
                        }
                        CodexViewEvent::DescribeChanges(preference, focus) => {
                            this.invoke_description_skill(preference.clone(), focus.clone(), cx);
                        }
                        _ => {}
                    }
                }),
                cx.subscribe_in(&tray, window, |this, _, event, window, cx| {
                    match event {
                        DestinationEvent::AcrossWorktrees => {
                            cx.defer_in(window, |this, window, cx| {
                                if let Some(d) = &this.dashboard {
                                    d.tray.update(cx, |tray, cx| tray.open_scope(window, cx));
                                }
                            });
                        }
                        DestinationEvent::ScopeChanged => {
                            this.refresh_dashboard_worktrees(cx);
                            this.refresh_dashboard_catalog(cx);
                            if let Some(dashboard) = &this.dashboard {
                                dashboard.view.focus_handle(cx).focus(window, cx);
                            }
                        }
                        DestinationEvent::NewProject => {
                            cx.defer_in(window, Self::add_project);
                        }
                        DestinationEvent::ManageProjects => {
                            cx.defer_in(window, |this, window, cx| {
                                if let Some(settings) = &this.settings_panel {
                                    settings.update(cx, |settings, cx| {
                                        settings.open_projects(None, window, cx);
                                    });
                                }
                            });
                        }
                        DestinationEvent::ManageWorktrees => {
                            let path = this.dashboard.as_ref().and_then(|d| {
                                let tray = d.tray.read(cx);
                                tray.projects
                                    .iter()
                                    .find(|p| p.0 == tray.project)
                                    .map(|p| p.2.clone())
                            });
                            cx.defer_in(window, move |this, window, cx| {
                                if let Some(settings) = &this.settings_panel {
                                    settings.update(cx, |settings, cx| {
                                        settings.open_project_worktrees(
                                            path.as_deref(),
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            });
                        }
                        DestinationEvent::Home => {
                            if let Some(d) = &this.dashboard {
                                d.tray.update(cx, |t, cx| {
                                    t.scope.clear();
                                    t.project = NO_PROJECT;
                                    t.path = dashboard_home_dir();
                                    t.generation = t.generation.wrapping_add(1);
                                    t.worktrees.clear();
                                    t.loading = false;
                                    t.error = None;
                                    cx.notify();
                                });
                            }
                            this.refresh_dashboard_catalog(cx);
                        }
                        DestinationEvent::Project(idx, expected_path) => {
                            let Some(path) = cx
                                .global::<SettingsState>()
                                .store
                                .workspace_projects(this.active_workspace)
                                .find(|(i, p)| i == idx && p.path == *expected_path)
                                .map(|(_, p)| p.path.clone())
                            else {
                                return;
                            };
                            if let Some(d) = &this.dashboard {
                                d.tray.update(cx, |t, cx| {
                                    t.scope.clear();
                                    t.project = *idx;
                                    t.path = path;
                                    cx.notify();
                                });
                            }
                            this.refresh_dashboard_worktrees(cx);
                            this.refresh_dashboard_catalog(cx);
                        }
                        DestinationEvent::Worktree(path) => {
                            if let Some(d) = &this.dashboard {
                                d.tray.update(cx, |t, cx| {
                                    t.scope.clear();
                                    t.path.clone_from(path);
                                    cx.notify();
                                });
                            }
                            this.refresh_dashboard_catalog(cx);
                        }
                        DestinationEvent::RefreshWorktrees => {
                            this.force_refresh_dashboard_worktrees(cx);
                        }
                        DestinationEvent::NewWorktree => {
                            let project = this.dashboard.as_ref().and_then(|dashboard| {
                                let tray = dashboard.tray.read(cx);
                                tray.projects
                                    .iter()
                                    .find(|entry| entry.0 == tray.project)
                                    .map(|(_, _, path)| path.clone())
                            });
                            if let Some(path) = project {
                                cx.defer_in(window, move |this, window, cx| {
                                    this.act(super::Action::NewWorktree(path), window, cx);
                                });
                            }
                        }
                    }
                    cx.notify();
                }),
            ];
            self.dashboard = Some(Dashboard {
                session,
                view,
                tray,
                provider_pending,
                viewport_height: std::rc::Rc::new(std::cell::Cell::new(
                    defaults
                        .as_ref()
                        .map_or(gpui::px(0.), |d| d.viewport_height),
                )),
                _events: events,
            });
            if provider_pending {
                self.resolve_dashboard_provider(window, cx);
            }
        }
        self.refresh_dashboard_projects(cx);
        self.refresh_dashboard_worktrees(cx);
        self.dashboard_open = true;
        self.enter_dashboard_timeline_scope(cx);
        if let Some(d) = &self.dashboard {
            let scope = self
                .dashboard_timelines
                .return_focus
                .take()
                .filter(|(workspace, _)| *workspace == self.active_workspace);
            if let Some((workspace, id)) = scope {
                self.dashboard_timelines
                    .session_focus(workspace, id, cx)
                    .focus(window, cx);
                if let Some(anchor) = self
                    .dashboard_timelines
                    .row_anchors
                    .get(&(workspace, id))
                    .cloned()
                {
                    let owner = cx.entity().downgrade();
                    window.on_next_frame(move |window, cx| {
                        if owner.upgrade().is_some_and(|sidebar| {
                            let sidebar = sidebar.read(cx);
                            sidebar.dashboard_open && sidebar.active_workspace == workspace
                        }) {
                            anchor.scroll_to(window, cx);
                        }
                    });
                }
            } else {
                d.view.focus_handle(cx).focus(window, cx);
            }
        }
        cx.notify();
    }

    fn resolve_dashboard_provider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dashboard) = &self.dashboard else {
            return;
        };
        let session = dashboard.session.downgrade();
        cx.spawn_in(window, async move |this, cx| {
            let provider = cx
                .background_spawn(async {
                    grove_core::env_path::wait_for_initial_environment();
                    detect_dashboard_provider()
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(dashboard) = &mut this.dashboard else {
                    return;
                };
                if session.upgrade().as_ref() != Some(&dashboard.session) {
                    return;
                }
                dashboard.provider_pending = false;
                let state = &dashboard.session.read(cx).state;
                if !dashboard.session.read(cx).is_deferred() || state.saved.composer.model.is_some()
                {
                    return;
                }
                dashboard.session.update(cx, |session, _| {
                    session.state.saved.provider = provider;
                    session.state.saved.label = provider.label().into();
                });
                dashboard.view.update(cx, |view, cx| {
                    view.update_provider_placeholder(provider, window, cx);
                });
                this.refresh_dashboard_catalog(cx);
            });
        })
        .detach();
    }
    /// Reconcile by repository path: removal or reordering must not retarget a draft.
    pub(super) fn refresh_dashboard_projects(&mut self, cx: &mut Context<Self>) {
        let Some(dashboard) = &self.dashboard else {
            return;
        };
        let projects: Vec<_> = cx
            .global::<SettingsState>()
            .store
            .workspace_projects(self.active_workspace)
            .map(|(idx, p)| (idx, p.name.clone(), p.path.clone()))
            .collect();
        let tray = dashboard.tray.clone();
        if tray.read(cx).projects == projects {
            return;
        }
        let old = tray.read(cx);
        let root = old
            .projects
            .iter()
            .find(|p| p.0 == old.project)
            .map(|p| p.2.clone());
        let selected = root
            .as_ref()
            .and_then(|root| projects.iter().find(|p| &p.2 == root))
            .map_or(NO_PROJECT, |p| p.0);
        let removed = old.project != NO_PROJECT && selected == NO_PROJECT;
        tray.update(cx, |tray, cx| {
            tray.projects = projects;
            for target in &mut tray.scope {
                if let Some((idx, name, _)) = tray
                    .projects
                    .iter()
                    .find(|(_, _, path)| *path == target.project_path)
                {
                    target.project = *idx;
                    target.project_name.clone_from(name);
                }
            }
            tray.project = selected;
            if removed && tray.scope.is_empty() {
                tray.path = dashboard_home_dir();
                tray.generation = tray.generation.wrapping_add(1);
                tray.worktrees.clear();
                tray.loading = false;
                tray.error = None;
            }
            cx.notify();
        });
        self.refresh_dashboard_worktrees(cx);
        self.refresh_dashboard_catalog(cx);
    }

    fn launch_dashboard(
        &mut self,
        open_transcript: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(d) = &self.dashboard else {
            return;
        };
        let (idx, path, scope) = {
            let t = d.tray.read(cx);
            (t.project, t.path.clone(), t.scope.clone())
        };
        let provider = d.session.read(cx).state.saved.provider;
        if !scope.is_empty() {
            let projects = cx
                .global::<SettingsState>()
                .store
                .workspace_projects(self.active_workspace)
                .map(|(idx, project)| (idx, project.name.clone(), project.path.clone()))
                .collect::<Vec<_>>();
            if let Err(error) = validate_scope(&scope, &path, &projects) {
                self.content_error = Some(error);
                cx.notify();
                return;
            }
            let service = self.runtime.read(cx).projects.read(cx);
            if scope.iter().any(|root| {
                service.is_removing(&root.project_path) || service.is_worktree_removing(&root.path)
            }) {
                self.content_error = Some(
                    "Wait for project or worktree removal to finish before starting this session."
                        .into(),
                );
                cx.notify();
                return;
            }
        }
        let Some((name, path, _)) = (if idx == NO_PROJECT {
            let home = dashboard_home_dir();
            (path == home && std::path::Path::new(&home).is_dir()).then_some((
                String::new(),
                home,
                provider.agent(),
            ))
        } else {
            self.launch_project_name(idx, &path, cx)
                .map(|name| (name, path, provider.agent()))
        }) else {
            self.content_error = Some("Select an available project and worktree.".into());
            cx.notify();
            return;
        };
        if self
            .runtime
            .read(cx)
            .provider_updates
            .read(cx)
            .is_updating(provider.agent())
            || grove_core::connections::credentials_busy(provider.agent())
        {
            self.content_error = Some(
                "This provider is updating or changing credentials. Wait for it to finish.".into(),
            );
            cx.notify();
            return;
        }
        let args = dashboard_launch_args(provider, &scope, &path, cx);
        let Some(d) = self.dashboard.as_ref() else {
            return;
        };
        if let Err(error) = d.session.update(cx, |s, cx| {
            s.state.saved.roots = scope_roots(&scope);
            s.activate(name, path, args, cx)
        }) {
            if d.session.read(cx).catalog_missing_executable.is_none() {
                self.content_error = Some(error);
            }
            cx.notify();
            return;
        }
        let session = d.session.clone();
        let view = d.view.clone();
        self.adopt_dashboard_launch(idx, session, view, open_transcript, window, cx);
    }
    fn adopt_dashboard_launch(
        &mut self,
        project: usize,
        session: Entity<CodexSession>,
        view: Entity<CodexView>,
        open_transcript: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let defaults = {
            let saved = &session.read(cx).state.saved;
            let worktrees = self.dashboard.as_ref().and_then(|dashboard| {
                let tray = dashboard.tray.read(cx);
                (tray.project == project
                    && tray.path == saved.cwd
                    && !tray.loading
                    && tray.error.is_none()
                    && tray.worktrees.iter().any(|(path, _)| path == &tray.path))
                .then(|| tray.worktrees.clone())
            });
            DashboardDefaults {
                project,
                path: saved.cwd.clone(),
                provider: saved.provider,
                composer: saved.composer.clone(),
                config_args: saved.config_args.clone(),
                worktrees,
                scope: self
                    .dashboard
                    .as_ref()
                    .map(|d| d.tray.read(cx).scope.clone())
                    .unwrap_or_default(),
                handoffs: {
                    let mut handoffs = view.read(cx).dashboard_handoff_sessions(cx);
                    handoffs.push(session.clone());
                    handoffs
                },
                viewport_height: self
                    .dashboard
                    .as_ref()
                    .map_or(gpui::px(0.), |d| d.viewport_height.get()),
            }
        };
        let id = self.runtime.update(cx, |runtime, cx| {
            runtime.adopt_dashboard_session(session, cx)
        });
        view.update(cx, |view, cx| view.launch_dashboard(window, cx));
        self.observe_session_view_events(id, &view, cx);
        self.codex_views.insert(id, view);
        self.dashboard = None;
        self.open_dashboard_with_defaults(Some(defaults), window, cx);
        self.dashboard_timelines
            .adopt_session(self.active_workspace, id, cx.reduce_motion());
        if open_transcript
            || cx.global::<SettingsState>().store.after_session_start
                == grove_core::storage::AfterSessionStart::OpenSession
        {
            // Adoption and first-send ownership are identical in both modes. Navigation
            // is immediate, so later provider readiness cannot steal focus back.
            self.select_session_id(id, window, cx);
        }
    }
    pub(super) fn refresh_dashboard_catalog(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &self.dashboard else {
            return;
        };
        if d.provider_pending {
            return;
        }
        let path = d.tray.read(cx).path.clone();
        let idx = d.tray.read(cx).project;
        let scope = d.tray.read(cx).scope.clone();
        let name = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let provider = d.session.read(cx).state.saved.provider;
        let args = dashboard_launch_args(provider, &scope, &path, cx);
        d.session.update(cx, |s, cx| {
            s.state.saved.cwd.clone_from(&path);
            s.state.saved.project = name;
            s.state.saved.roots = scope_roots(&scope);
            s.refresh_catalog(path, args, cx);
            cx.notify();
        });
    }
    pub(super) fn refresh_dashboard_worktrees(&mut self, cx: &mut Context<Self>) {
        self.refresh_dashboard_worktrees_inner(false, cx);
    }
    pub(super) fn force_refresh_dashboard_worktrees(&mut self, cx: &mut Context<Self>) {
        self.refresh_dashboard_worktrees_inner(true, cx);
    }
    fn refresh_dashboard_worktrees_inner(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(d) = &self.dashboard else {
            return;
        };
        let idx = d.tray.read(cx).project;
        let Some(project) = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .cloned()
        else {
            return;
        };
        let tray = d.tray.clone();
        let root = project.path;
        let scope = (idx, root.clone(), self.active_workspace);
        if !force
            && tray.read(cx).worktree_discovery.reusable(
                &scope,
                tray.read(cx).generation,
                !tray.read(cx).worktrees.is_empty(),
            )
        {
            return;
        }
        let generation = tray.update(cx, |t, cx| {
            t.generation = t.generation.wrapping_add(1);
            let preserve = std::mem::take(&mut t.preserve_on_refresh)
                || (t.worktree_discovery.scope.as_ref() == Some(&scope) && !t.worktrees.is_empty());
            t.worktree_discovery.scope = Some(scope);
            t.worktree_discovery.in_flight = Some(t.generation);
            t.worktree_discovery.fetched_at = None;
            t.loading = !preserve;
            t.error = None;
            if !preserve {
                t.worktrees = vec![(root.clone(), "Main worktree".into())];
            }
            cx.notify();
            t.generation
        });
        let runtime = self.runtime.clone();
        let expected_root = root.clone();
        let workspace = self.active_workspace;
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    grove_core::git::list_worktrees_checked(&root).map_err(|e| e.to_string())
                })
                .await;
            tray.update(cx, |t, cx| {
                // A completed request cannot remain joinable after navigation.
                // Compare its marker first: navigation may invalidate generation
                // without starting a replacement, but newer requests keep theirs.
                if t.worktree_discovery.in_flight == Some(generation) {
                    t.worktree_discovery.in_flight = None;
                    if t.generation == generation {
                        t.loading = false;
                    }
                    cx.notify();
                }
                if t.generation != generation {
                    return;
                }
                let store = &cx.global::<SettingsState>().store;
                if t.project != idx
                    || store.workspaces.active != workspace
                    || !store
                        .workspace_projects(workspace)
                        .any(|(i, p)| i == idx && p.path == expected_root)
                {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(rows) => {
                        t.worktree_discovery.fetched_at = Some(std::time::Instant::now());
                        runtime.update(cx, |runtime, cx| {
                            let active = runtime.state.read(cx).proj_idx();
                            runtime.tree.update(cx, |tree, cx| {
                                tree.register_project_worktrees(idx, active, rows.clone());
                                cx.notify();
                            });
                        });
                        t.worktrees = rows
                            .into_iter()
                            .map(|w| {
                                let name = if w.branch.is_empty() {
                                    std::path::Path::new(&w.path)
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned()
                                } else {
                                    w.branch
                                };
                                (w.path, name)
                            })
                            .collect();
                    }
                    Err(e) => t.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn dashboard_checkout(&self, cx: &App) -> Option<(usize, String)> {
        let tray = self.dashboard.as_ref()?.tray.read(cx);
        cx.global::<SettingsState>()
            .store
            .workspace_projects(self.active_workspace)
            .find(|(idx, project)| *idx == tray.project && !project.archived)
            .map(|(idx, _)| (idx, tray.path.clone()))
    }

    pub(super) fn render_dashboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(d) = self.dashboard.as_ref() else {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(rpx(SPACE_3XL))
                .child("Add a project to start a session")
                .child(
                    super::super::components::form_action(
                        "dashboard-add-project",
                        "Add project",
                        false,
                        window,
                        cx,
                    )
                    .icon(gpui_component::Icon::default().path("icons/plus.svg"))
                    .on_click(cx.listener(|this, _, window, cx| this.add_project(window, cx))),
                )
                .into_any_element();
        };
        let dashboard_composer = d.view.clone();
        let viewport_height = d.viewport_height.clone();
        let measured_height = viewport_height.get();
        let composer_top = if measured_height > gpui::px(0.) {
            measured_height
        } else {
            window.viewport_size().height
        } * DASHBOARD_COMPOSER_TOP_FRACTION
            - rpx(CONTROL_H + SPACE_MD).to_pixels(window.rem_size());
        let owner = cx.entity().entity_id();
        let registry = self.runtime.read(cx).registry.read(cx);
        let activity = self.runtime.read(cx).activity.read(cx);
        let sessions: Vec<_> = registry
            .all()
            .iter()
            .filter(|meta| {
                meta.project.is_empty()
                    || self
                        .snapshot
                        .projects
                        .iter()
                        .any(|project| project.sessions.contains(&meta.id))
            })
            .map(|meta| {
                let (status, _) = self.status(meta, cx);
                let title = session_display_title(
                    meta,
                    registry
                        .session(meta.id)
                        .and_then(|session| session.read(cx).title()),
                );
                let since = activity.since_of(meta.id).unwrap_or(meta.spawned_at);
                (meta.clone(), status, title, since)
            })
            .collect();
        let grouping_entries = sessions
            .iter()
            .map(|(meta, status, _, since)| super::SessionListEntry {
                id: meta.id,
                state: activity.state_of(meta.id),
                status,
                since: Some(*since),
                active: activity.active_of(meta.id),
            })
            .collect::<Vec<_>>();
        let grouping_entries = self
            .dashboard_timelines
            .review_holds
            .entries(self.active_workspace, grouping_entries);
        let groups = super::group_sessions_for_list(&grouping_entries, Instant::now());
        self.dashboard_timelines.review_order = groups
            .iter()
            .find(|(section, _)| *section == "Review")
            .map(|(_, indices)| indices.iter().map(|index| sessions[*index].0.id).collect())
            .unwrap_or_default();
        let summary = div()
            .id("dashboard-summary")
            .debug_selector(|| "dashboard-summary".into())
            .w_full()
            .max_w(rpx(CHAT_MAX_W))
            .text_center()
            .text_size(rpx(TEXT_DISPLAY_LG))
            .font_weight(gpui::FontWeight::MEDIUM)
            .child("Ship fast. Break faster.");
        let mut recent = div()
            .id("dashboard-recent")
            .debug_selector(|| "dashboard-recent".into())
            .w_full()
            .max_w(rpx(CHAT_MAX_W))
            .border_1()
            .border_color(c::BORDER_SOFT())
            .rounded(rpx(RADIUS_PANEL))
            .p(rpx(SPACE_2XL))
            .flex()
            .flex_col();
        if sessions.is_empty() {
            recent = recent.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child("Your sessions will appear here."),
            );
        } else {
            for (group_index, (section, indices)) in groups.into_iter().enumerate() {
                let workspace = self.active_workspace;
                let mut category = div()
                    .id(gpui::SharedString::from(format!(
                        "dashboard-category-{section}"
                    )))
                    .w_full()
                    .flex()
                    .flex_col()
                    .when(section == "Review", |category| {
                        category.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            this.hover_review_category(workspace, *hovered, cx);
                        }))
                    });
                category = category.child(
                    div()
                        .id(gpui::SharedString::from(format!(
                            "dashboard-section-{section}"
                        )))
                        .debug_selector(move || format!("dashboard-section-{section}"))
                        .when(group_index > 0, |heading| heading.pt(rpx(SPACE_2XL)))
                        .pb(rpx(SPACE_SM))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(format!("{section} · {}", indices.len())),
                );
                for index in indices {
                    let (meta, status, title, since) = sessions[index].clone();
                    let id = meta.id;
                    let age = session_age_label(since, Instant::now());
                    let label = format!(
                        "{title} · {} · {} · {status} · {age} · {}",
                        meta.project,
                        meta.agent.label(),
                        meta.wt_path
                    );
                    let structured =
                        self.runtime
                            .read(cx)
                            .registry
                            .read(cx)
                            .codex(id)
                            .map(|session| {
                                let state = &session.read(cx).state;
                                (
                                    state.status,
                                    state.review_completion(),
                                    state.saved.provider,
                                    super::super::codex_view::composer_model_name(state),
                                    state
                                        .effort
                                        .as_deref()
                                        .filter(|effort| !effort.trim().is_empty())
                                        .or(state
                                            .resolved_effort
                                            .as_deref()
                                            .filter(|effort| !effort.trim().is_empty()))
                                        .or(state
                                            .saved
                                            .composer
                                            .effort
                                            .as_deref()
                                            .filter(|effort| !effort.trim().is_empty()))
                                        .map(str::to_owned),
                                )
                            });
                    if let Some((structured_status, review_completion, provider, model, effort)) =
                        structured
                    {
                        let worktree = self
                            .snapshot
                            .projects
                            .iter()
                            .flat_map(|project| &project.worktrees)
                            .find(|worktree| worktree.path == meta.wt_path);
                        let worktree = worktree.map_or_else(
                            || {
                                let main = cx.global::<SettingsState>().store.projects.iter().any(
                                    |project| {
                                        project.name == meta.project
                                            && crate::paths::normalize_wt_path(&project.path)
                                                == crate::paths::normalize_wt_path(&meta.wt_path)
                                    },
                                );
                                if main {
                                    sidebar_worktree_name("", true).to_owned()
                                } else {
                                    std::path::Path::new(&meta.wt_path).file_name().map_or_else(
                                        || meta.wt_path.clone(),
                                        |name| name.to_string_lossy().into_owned(),
                                    )
                                }
                            },
                            |worktree| {
                                sidebar_worktree_name(&worktree.name, worktree.is_main).to_owned()
                            },
                        );
                        let worktree = if meta.context_roots.len() > 1 {
                            super::multi_project::worktree_names(&meta, &self.snapshot)
                        } else {
                            worktree
                        };
                        let metadata = super::dashboard_timeline::RowMetadata {
                            project: if let Some(names) = super::multi_project::project_names(&meta)
                            {
                                names
                            } else if meta.project.is_empty() {
                                "No project".into()
                            } else {
                                meta.project.clone()
                            },
                            worktree,
                            provider,
                            model,
                            effort,
                            context: label,
                            status: structured_status,
                            display_status: status,
                            review_completion,
                            age,
                        };
                        category = category.child(
                            self.render_dashboard_progress(id, &title, &metadata, window, cx),
                        );
                        continue;
                    }
                    let row_focus =
                        self.dashboard_timelines
                            .session_focus(self.active_workspace, id, cx);
                    let mut terminal_row = div().w_full().child(
                        self.row(
                            format!("dashboard-session-{}", id.raw()),
                            label,
                            false,
                            Action::Select(Selection::Session(id)),
                            cx,
                        )
                        .track_focus(&row_focus)
                        .focus_visible(|row| row.bg(c::BG_HOVER()))
                        .debug_selector(move || format!("dashboard-terminal-session-{}", id.raw()))
                        .h_auto()
                        .min_h(rpx(ROW_H))
                        .py(rpx(SPACE_SM))
                        .border_t_1()
                        .border_color(c::BORDER_SOFT())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(rpx(TEXT_BODY))
                                .child(single_line_session_title(&title)),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(super::super::status::session_kind(status).color())
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .child(session_status_icon_sized(id, status, ICON_SM, cx))
                                .child(status),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(age),
                        ),
                    );
                    if matches!(status, "Review" | "Review · unread") {
                        if let Some(completion) =
                            self.runtime.read(cx).activity.read(cx).terminal_review(id)
                        {
                            terminal_row = terminal_row.child(
                                div().pb(rpx(SPACE_LG)).child(
                                    self.control(
                                        ("dashboard-terminal-mark-reviewed", id.raw()),
                                        "Mark reviewed",
                                        Action::MarkTerminalReviewed(id, completion),
                                        cx,
                                    )
                                    .w_auto()
                                    .px(rpx(SPACE_LG))
                                    .gap(rpx(SPACE_SM))
                                    .text_size(rpx(TEXT_SMALL))
                                    .child(icon("check", ICON_SM, c::FG_DIM()))
                                    .child("Mark reviewed"),
                                ),
                            );
                        }
                    }
                    let terminal_row = self.track_review_hold(
                        (self.active_workspace, id),
                        terminal_row.into_any_element(),
                        window,
                        cx,
                    );
                    category = category.child(terminal_row);
                }
                recent = recent.child(category);
            }
        }
        div()
            .id("dashboard-page")
            .debug_selector(|| "dashboard-page".into())
            .size_full()
            .relative()
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        if viewport_height.replace(bounds.size.height) != bounds.size.height {
                            cx.notify(owner);
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                div()
                    .id("dashboard-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.dashboard_timelines.scroll)
                    .child(
                        div()
                            .w_full()
                            .min_h_full()
                            .flex()
                            .flex_col()
                            .when_some(
                                self.checkout_panel.narrow_content.take(),
                                gpui::ParentElement::child,
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_h(composer_top)
                                    .flex_shrink_0()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_end()
                                    .px(rpx(SPACE_20))
                                    .pt(rpx(SPACE_3XL))
                                    .pb(rpx(SPACE_3XL))
                                    .children(self.content_error.as_ref().map(|error| {
                                        div()
                                            .text_size(rpx(TEXT_SMALL))
                                            .text_color(c::FORM_ERROR())
                                            .p(rpx(SPACE_2XL))
                                            .child(error.clone())
                                    }))
                                    .child(summary),
                            )
                            .child(div().w_full().flex_shrink_0().child(dashboard_composer))
                            .child(
                                div()
                                    .w_full()
                                    .flex_shrink_0()
                                    .flex()
                                    .justify_center()
                                    .px(rpx(SPACE_20))
                                    .pt(rpx(SPACE_3XL))
                                    .pb(rpx(SPACE_3XL))
                                    .child(
                                        div()
                                            .w_full()
                                            .max_w(rpx(CHAT_MAX_W))
                                            .flex()
                                            .flex_col()
                                            .gap(rpx(SPACE_LG))
                                            .child(
                                                div()
                                                    .id("dashboard-activity-heading")
                                                    .debug_selector(|| {
                                                        "dashboard-activity-heading".into()
                                                    })
                                                    .text_size(rpx(TEXT_TITLE))
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                                    .child("Activity"),
                                            )
                                            .child(recent),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
fn seed_activity_fixture(
    sidebar: &mut Sidebar,
    provider: grove_core::codex::Provider,
    window: &mut Window,
    cx: &mut Context<Sidebar>,
) -> SessionId {
    let mut saved = sidebar
        .dashboard
        .as_ref()
        .unwrap()
        .session
        .read(cx)
        .state
        .saved
        .clone();
    saved.provider = provider;
    saved.title = Some("Implement dashboard navigation".into());
    saved.items = vec![
        serde_json::json!({"type":"userMessage","text":"Implement dashboard navigation"}),
        serde_json::json!({"type":"plan","plan":[{"id":"task-1","step":"Trace session closing and implement dashboard navigation","status":"inProgress"},{"id":"task-2","step":"Verify the change and commit it","status":"pending"}]}),
        serde_json::json!({"type":"agentMessage","text":"Traced the session-close handler."}),
        serde_json::json!({"type":"commandExecution","command":"cat product/design.md"}),
        serde_json::json!({"type":"agentMessage","text":"Added navigation back to the dashboard."}),
        serde_json::json!({"type":"plan","plan":[{"id":"task-1","step":"Trace session closing and implement dashboard navigation","status":"completed"},{"id":"task-2","step":"Verify the change and commit it","status":"inProgress"}]}),
        serde_json::json!({"type":"agentMessage","text":"Checked session-close navigation."}),
        serde_json::json!({"type":"agentMessage","text":"Verified dashboard return behavior."}),
        serde_json::json!({"type":"agentMessage","text":"Running navigation checks."}),
        serde_json::json!({"type":"commandExecution","command":"cargo test dashboard -- --nocapture","status":"inProgress"}),
    ].into();
    let (handle, _actions) = grove_core::codex::Handle::channel();
    let (_updates, receive) = std::sync::mpsc::channel();
    let mut snapshot = grove_core::codex::Snapshot::new(saved);
    snapshot.status = grove_core::codex::Status::Working;
    let session = cx.new(|cx| {
        CodexSession::from_connection(
            snapshot,
            grove_core::codex::Connection {
                handle: handle.clone(),
                updates: receive.into(),
            },
            cx,
        )
    });
    let registry = sidebar.runtime.read(cx).registry.clone();
    let cwd = session.read(cx).state.saved.cwd.clone();
    let project = session.read(cx).state.saved.project.clone();
    let id = registry.update(cx, |registry, _| {
        let id = registry.insert_meta(project, cwd, provider.agent());
        registry.attach_codex(id, session.clone(), handle);
        id
    });
    sidebar.sync(window, cx);
    let items = &session.read(cx).state.saved.items;
    let cache = sidebar.task_glances.entry(id).or_default();
    cache.refresh(items);
    cache.refresh_dashboard(items, grove_core::codex::Status::Working);
    id
}

#[cfg(test)]
mod tests;

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod native_snapshots;
#[test]
fn fresh_dashboard_defaults_to_claude_only_when_codex_is_confirmed_missing() {
    use grove_core::codex::Provider::{Claude, Codex};
    for (codex, claude, expected) in [
        (Some(false), Some(true), Claude),
        (Some(true), Some(false), Codex),
        (Some(true), Some(true), Codex),
        (Some(false), Some(false), Codex),
        (None, Some(true), Codex),
        (Some(false), None, Codex),
    ] {
        assert_eq!(fresh_dashboard_provider(codex, claude), expected);
    }
}
