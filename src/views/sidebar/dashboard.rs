//! Dashboard destination state; the composer remains the shared native editor.
use super::*;
use crate::entities::codex_session::CodexSession;
use crate::views::codex_view::{CodexView, CodexViewEvent, CHAT_MAX_W};
use gpui_component::menu::{PopupMenu, PopupMenuItem};

const NO_PROJECT: usize = usize::MAX;
const WORKTREE_CACHE_FRESHNESS: std::time::Duration = std::time::Duration::from_secs(10);

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
    viewport_height: gpui::Pixels,
}
pub(super) struct Dashboard {
    pub session: Entity<CodexSession>,
    pub view: Entity<CodexView>,
    pub tray: Entity<DestinationTray>,
    viewport_height: std::rc::Rc<std::cell::Cell<gpui::Pixels>>,
    _events: Vec<gpui::Subscription>,
}
pub(crate) enum DestinationEvent {
    Project(usize, String),
    Worktree(String),
    NewWorktree,
    NewProject,
    Home,
    RefreshWorktrees,
}
pub(crate) struct DestinationTray {
    projects: Vec<(usize, String, String)>,
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
impl EventEmitter<DestinationEvent> for DestinationTray {}
impl DestinationTray {
    pub(crate) fn preparation(&self) -> (bool, Option<&str>) {
        (self.loading, self.error.as_deref())
    }
    pub(crate) fn retry_worktrees(&mut self, cx: &mut Context<Self>) {
        cx.emit(DestinationEvent::RefreshWorktrees);
    }
    #[cfg(test)]
    pub(crate) fn set_preparation_for_test(
        &mut self,
        loading: bool,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.generation = self.generation.wrapping_add(1);
        self.loading = loading;
        self.error = error;
        cx.notify();
    }
    #[cfg(test)]
    pub(crate) fn picker_test_tray(path: String) -> Self {
        Self::new(vec![(0, "Picker fixture".into(), path.clone())], 0, path)
    }
    fn new(projects: Vec<(usize, String, String)>, project: usize, path: String) -> Self {
        Self {
            projects,
            project,
            path,
            worktrees: Vec::new(),
            loading: false,
            generation: 0,
            error: None,
            preserve_on_refresh: false,
            worktree_discovery: WorktreeDiscovery::default(),
            popup: None,
            popup_events: None,
            popup_motion: motion::PopupMotion::default(),
            closing_popup: None,
            popup_generation: 0,
            hovered: [false; 2],
            open_selector: None,
            position: gpui::Point::default(),
            bounds: std::array::from_fn(|_| std::rc::Rc::default()),
        }
    }
    fn close_popup(&mut self, cx: &mut Context<Self>) {
        self.popup_generation = self.popup_generation.wrapping_add(1);
        self.closing_popup = None;
        if let Some(popup) = self.popup.take() {
            if !cx.reduce_motion() {
                self.closing_popup = Some((popup, self.position, self.open_selector == Some(1)));
                self.popup_motion.hide(cx);
                let generation = self.popup_generation;
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(motion::PopupMotion::exit_duration())
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if generation == this.popup_generation {
                            this.closing_popup = None;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
        self.open_selector = None;
        self.popup_events = None;
        cx.notify();
    }
    fn open(&mut self, project: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.popup_generation = self.popup_generation.wrapping_add(1);
        self.closing_popup = None;
        self.popup_motion.show(cx);
        let owner = cx.entity().downgrade();
        let projects = self.projects.clone();
        let worktrees = self.worktrees.clone();
        let selected_project = self.project;
        let selected_path = self.path.clone();
        let focus = window.focused(cx);
        let popup = PopupMenu::build(window, cx, move |mut menu, window, _| {
            menu = crate::views::components::dropdown_menu(menu, window);
            menu = menu.scrollable(false);
            if project {
                menu = menu.check_side(gpui_component::Side::Right);
                let home_owner = owner.clone();
                menu = menu
                    .item(
                        PopupMenuItem::element(|_, _| {
                            div()
                                .id("destination-no-project")
                                .aria_label("No project")
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .child(
                                    div()
                                        .size(rpx(ICON_20))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(icon("home", ICON_SM, c::FG_DIM())),
                                )
                                .child("No project")
                        })
                        .checked(selected_project == NO_PROJECT)
                        .on_click(move |_, _, cx| {
                            let _ = home_owner.update(cx, |_, cx| cx.emit(DestinationEvent::Home));
                        }),
                    )
                    .separator();
                for (idx, name, path) in projects {
                    let owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::element(move |_, _| {
                            div()
                                .id(format!("destination-project-{idx}"))
                                .aria_label(name.clone())
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .child(super::super::components::project_initials_badge(&name))
                                .child(name.clone())
                        })
                        .checked(idx == selected_project)
                        .on_click(move |_, _, cx| {
                            let _ = owner.update(cx, |_, cx| {
                                cx.emit(DestinationEvent::Project(idx, path.clone()));
                            });
                        }),
                    );
                }
                menu = menu.separator().item(
                    PopupMenuItem::element(|_, _| {
                        div()
                            .id("destination-new-project")
                            .aria_label("New project")
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_SM))
                            .child(
                                div()
                                    .size(rpx(ICON_20))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon("plus", ICON_SM, c::FG_DIM())),
                            )
                            .child("New project")
                    })
                    .on_click(move |_, _, cx| {
                        let _ = owner.update(cx, |_, cx| cx.emit(DestinationEvent::NewProject));
                    }),
                );
            } else {
                menu = menu.check_side(gpui_component::Side::Right);
                for (path, name) in worktrees {
                    let owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(name)
                            .icon(
                                gpui_component::Icon::default()
                                    .path("icons/git-branch.svg")
                                    .size(rpx(ICON_SM)),
                            )
                            .checked(path == selected_path)
                            .on_click(move |_, _, cx| {
                                let _ = owner.update(cx, |_, cx| {
                                    cx.emit(DestinationEvent::Worktree(path.clone()));
                                });
                            }),
                    );
                }
                menu = menu.separator().item(
                    PopupMenuItem::new("New worktree")
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/plus.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .on_click(move |_, _, cx| {
                            let _ =
                                owner.update(cx, |_, cx| cx.emit(DestinationEvent::NewWorktree));
                        }),
                );
            }
            menu
        });
        let bounds = self.bounds[usize::from(!project)].get();
        self.position = if project {
            bounds.bottom_left()
        } else {
            bounds.bottom_right()
        };
        self.popup_events = Some(cx.subscribe_in(
            &popup,
            window,
            move |this, _, _: &gpui::DismissEvent, window, cx| {
                this.close_popup(cx);
                if let Some(focus) = &focus {
                    focus.focus(window, cx);
                }
                cx.notify();
            },
        ));
        popup.focus_handle(cx).focus(window, cx);
        self.popup = Some(popup);
        self.open_selector = Some(usize::from(!project));
        cx.notify();
    }
}
impl Render for DestinationTray {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let project = self
            .projects
            .iter()
            .find(|p| p.0 == self.project)
            .map_or_else(|| "No project".into(), |p| p.1.clone());
        let worktree = self
            .worktrees
            .iter()
            .find(|p| p.0 == self.path)
            .map_or_else(|| "Select worktree".into(), |p| p.1.clone());
        let mut tray = div()
            .id("dashboard-destination")
            .debug_selector(|| "dashboard-destination".into())
            .relative()
            // Center a narrower context strip beneath the rounded writing surface.
            .w(gpui::relative(COMPOSER_CONTEXT_WIDTH_FRACTION))
            .mx_auto()
            .mt_0()
            .py(rpx(SPACE_SM))
            .px(rpx(SPACE_2XL))
            .rounded_b(rpx(RADIUS_PANEL))
            .border_l_1()
            .border_r_1()
            .border_b_1()
            .border_color(c::BORDER_SOFT())
            .bg(c::BG())
            .flex()
            .flex_wrap()
            .gap(rpx(SPACE_SM));
        for (idx, (is_project, label, glyph)) in
            [(true, project, "folder"), (false, worktree, "git-branch")]
                .into_iter()
                .enumerate()
        {
            if !is_project && self.project == NO_PROJECT {
                continue;
            }
            let bounds = self.bounds[idx].clone();
            let foreground = if self.hovered[idx] || self.open_selector == Some(idx) {
                c::FG()
            } else {
                c::FG_DIM()
            };
            tray = tray.child(
                div()
                    .id(("dashboard-destination-hover", idx))
                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                        if this.hovered[idx] != *hovered {
                            this.hovered[idx] = *hovered;
                            cx.notify();
                        }
                    }))
                    .relative()
                    .when(!is_project, gpui::Styled::ml_auto)
                    .max_w(rpx(MODAL_W_SM))
                    .child(
                        gpui::canvas(move |b, _, _| bounds.set(b), |_, (), _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(
                        super::super::components::dense_disclosure_action(
                            if is_project {
                                "dashboard-project"
                            } else {
                                "dashboard-worktree"
                            },
                            label.clone(),
                            window,
                            cx,
                        )
                        .when(self.open_selector == Some(idx), |button| {
                            button.text_color(c::FG())
                        })
                        .icon_only()
                        .max_w(rpx(MODAL_W_SM))
                        .icon(gpui_component::Icon::default().path(format!("icons/{glyph}.svg")))
                        .child(
                            div()
                                .min_w_0()
                                .max_w(rpx(MODAL_W_MD / 2.0))
                                .truncate()
                                .child(label),
                        )
                        .child(icon("chev-down", ICON_SM, foreground))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open(is_project, window, cx);
                            },
                        )),
                    ),
            );
        }
        let visible_popup = if let Some(popup) = &self.popup {
            Some((
                popup.clone(),
                self.position,
                self.open_selector == Some(1),
                false,
            ))
        } else if !cx.reduce_motion() {
            self.closing_popup
                .as_ref()
                .map(|(popup, position, worktree)| (popup.clone(), *position, *worktree, true))
        } else {
            None
        };
        if let Some((popup, position, worktree, exiting)) = visible_popup {
            if exiting {
                popup.update(cx, |popup, _| popup.set_passive(true));
            }
            tray = tray.child(gpui::deferred(
                gpui::anchored()
                    .position_mode(gpui::AnchoredPositionMode::Window)
                    .anchor(if worktree {
                        gpui::Anchor::TopRight
                    } else {
                        gpui::Anchor::TopLeft
                    })
                    .position(position)
                    .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
                    .child(
                        self.popup_motion.render(
                            div()
                                .id(if exiting {
                                    "dashboard-destination-menu-exit"
                                } else {
                                    "dashboard-destination-menu"
                                })
                                .when(!exiting, |surface| {
                                    surface.debug_selector(|| "dashboard-destination-menu".into())
                                })
                                .child(popup),
                            "destination-popup-rise",
                            cx,
                        ),
                    ),
            ));
        }
        tray
    }
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
            let provider = defaults
                .as_ref()
                .map_or(grove_core::codex::Provider::Codex, |defaults| {
                    defaults.provider
                });
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
                roots: Vec::new(),
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
            let session = cx.new(|cx| {
                CodexSession::draft(
                    saved,
                    provider.agent().launch_args_for_backend(
                        true,
                        cx.global::<SettingsState>().store.yolo_enabled,
                        cx.global::<SettingsState>()
                            .store
                            .chrome_enabled
                            .unwrap_or(false),
                    ),
                    cx,
                )
            });
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
                        CodexViewEvent::DashboardSubmit => this.launch_dashboard(window, cx),
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
                        DestinationEvent::NewProject => this.add_project(window, cx),
                        DestinationEvent::Home => {
                            if let Some(d) = &this.dashboard {
                                d.tray.update(cx, |t, cx| {
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
                            if let Some(d) = &this.dashboard {
                                let idx = d.tray.read(cx).project;
                                this.begin_worktree(idx, window, cx);
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
                viewport_height: std::rc::Rc::new(std::cell::Cell::new(
                    defaults
                        .as_ref()
                        .map_or(gpui::px(0.), |d| d.viewport_height),
                )),
                _events: events,
            });
        }
        let projects: Vec<_> = cx
            .global::<SettingsState>()
            .store
            .workspace_projects(self.active_workspace)
            .map(|(idx, p)| (idx, p.name.clone(), p.path.clone()))
            .collect();
        let mut destination_changed = false;
        if let Some(d) = &self.dashboard {
            let current = d.tray.read(cx).project;
            let fallback = Some(NO_PROJECT);
            let selected = if current == NO_PROJECT || projects.iter().any(|p| p.0 == current) {
                Some(current)
            } else {
                fallback
            };
            destination_changed = selected != Some(current);
            d.tray.update(cx, |t, cx| {
                t.projects = projects;
                if let Some(idx) = selected {
                    if t.project != idx {
                        t.project = idx;
                        t.path = cx
                            .global::<SettingsState>()
                            .store
                            .projects
                            .get(idx)
                            .map_or_else(dashboard_home_dir, |p| p.path.clone());
                        t.generation = t.generation.wrapping_add(1);
                        t.worktrees.clear();
                        t.loading = false;
                        t.error = None;
                    }
                }
                cx.notify();
            });
        }
        self.refresh_dashboard_worktrees(cx);
        if destination_changed {
            self.refresh_dashboard_catalog(cx);
        }
        self.dashboard_open = true;
        self.enter_dashboard_timeline_scope(cx);
        if let Some(d) = &self.dashboard {
            d.view.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }
    fn launch_dashboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = &self.dashboard else {
            return;
        };
        let (idx, path) = {
            let t = d.tray.read(cx);
            (t.project, t.path.clone())
        };
        let provider = d.session.read(cx).state.saved.provider;
        let Some((name, path, _)) = (if idx == NO_PROJECT {
            let home = dashboard_home_dir();
            (path == home && std::path::Path::new(&home).is_dir()).then_some((
                String::new(),
                home,
                provider.agent(),
            ))
        } else {
            self.launch_target(idx, path, provider.agent(), cx)
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
        let args = provider.agent().launch_args_for_backend(
            true,
            cx.global::<SettingsState>().store.yolo_enabled,
            cx.global::<SettingsState>()
                .store
                .chrome_enabled
                .unwrap_or(false),
        );
        let Some(d) = self.dashboard.as_ref() else {
            return;
        };
        if let Err(error) = d
            .session
            .update(cx, |s, cx| s.activate(name, path, args, cx))
        {
            self.content_error = Some(error);
            cx.notify();
            return;
        }
        let session = d.session.clone();
        let view = d.view.clone();
        self.adopt_dashboard_launch(idx, session, view, window, cx);
    }
    fn adopt_dashboard_launch(
        &mut self,
        project: usize,
        session: Entity<CodexSession>,
        view: Entity<CodexView>,
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
    }
    pub(super) fn refresh_dashboard_catalog(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &self.dashboard else {
            return;
        };
        let path = d.tray.read(cx).path.clone();
        let idx = d.tray.read(cx).project;
        let name = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let provider = d.session.read(cx).state.saved.provider;
        let args = provider.agent().launch_args_for_backend(
            true,
            cx.global::<SettingsState>().store.yolo_enabled,
            cx.global::<SettingsState>()
                .store
                .chrome_enabled
                .unwrap_or(false),
        );
        d.session.update(cx, |s, cx| {
            s.state.saved.cwd.clone_from(&path);
            s.state.saved.project = name;
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
                        let metadata = super::dashboard_timeline::RowMetadata {
                            project: if meta.project.is_empty() {
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
mod tests {
    use super::*;

    #[test]
    fn worktree_cache_reuses_only_fresh_matching_results_or_current_inflight() {
        let scope = (1, "/project".to_owned(), 7);
        let mut discovery = WorktreeDiscovery {
            scope: Some(scope.clone()),
            fetched_at: Some(std::time::Instant::now()),
            in_flight: None,
        };
        assert!(discovery.reusable(&scope, 4, true));
        assert!(!discovery.reusable(&scope, 4, false));
        assert!(!discovery.reusable(&(1, "/other".into(), 7), 4, true));
        assert!(!discovery.reusable(&(1, "/project".into(), 8), 4, true));
        discovery.fetched_at = Some(
            std::time::Instant::now()
                .checked_sub(WORKTREE_CACHE_FRESHNESS)
                .unwrap(),
        );
        assert!(!discovery.reusable(&scope, 4, true));
        discovery.in_flight = Some(4);
        assert!(discovery.reusable(&scope, 4, false));
        assert!(!discovery.reusable(&scope, 5, true));
        discovery.fetched_at = None;
        discovery.in_flight = None;
        assert!(
            !discovery.reusable(&scope, 4, true),
            "failures remain retryable"
        );
    }

    #[gpui::test]
    async fn dashboard_revisit_reuses_worktrees_and_retry_forces_refresh(
        cx: &mut gpui::TestAppContext,
    ) {
        let repo = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success());
        let path = repo.path().to_string_lossy().into_owned();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let mut store = grove_core::storage::Store::default();
            store.projects.push(grove_core::storage::Project {
                name: "Cached worktrees".into(),
                path: path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            });
            store.assign_project_to_active_workspace(&path);
            cx.set_global(SettingsState::new(store));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.open_dashboard(window, cx);
                let tray = sidebar.dashboard.as_ref().unwrap().tray.clone();
                // Seed a successful result, then exercise the real navigation
                // and request boundary synchronously so no clock or CLI timing is needed.
                tray.update(cx, |tray, _| {
                    tray.worktrees = vec![(path.clone(), "Main worktree".into())];
                    tray.worktree_discovery.scope =
                        Some((0, path.clone(), sidebar.active_workspace));
                    tray.worktree_discovery.fetched_at = Some(std::time::Instant::now());
                    tray.worktree_discovery.in_flight = None;
                    tray.loading = false;
                });
                let generation = tray.read(cx).generation;
                let rows = tray.read(cx).worktrees.clone();
                sidebar.open_dashboard(window, cx);
                assert_eq!(
                    tray.read(cx).generation,
                    generation,
                    "fresh revisit must not launch Git"
                );
                tray.update(cx, |tray, _| {
                    tray.worktree_discovery.fetched_at = Some(
                        std::time::Instant::now()
                            .checked_sub(WORKTREE_CACHE_FRESHNESS)
                            .unwrap(),
                    );
                });
                sidebar.open_dashboard(window, cx);
                let refreshing = tray.read(cx).generation;
                assert_ne!(refreshing, generation);
                assert_eq!(tray.read(cx).worktrees, rows);
                assert!(
                    !tray.read(cx).loading,
                    "retained rows remain usable during refresh"
                );
                sidebar.open_dashboard(window, cx);
                assert_eq!(
                    tray.read(cx).generation,
                    refreshing,
                    "concurrent revisit must join refresh"
                );
                tray.update(cx, |tray, _| tray.error = Some("retry me".into()));
                tray.update(cx, DestinationTray::retry_worktrees);
            });
        });
        // Retry is delivered through Sidebar's real subscription. A parked UI
        // scheduler does not imply the real Git subprocess has finished.
        cx.run_until_parked();
        let tray = cx.update(|_, cx| sidebar.read(cx).dashboard.as_ref().unwrap().tray.clone());
        cx.cx
            .condition(&tray, |tray, _| tray.worktree_discovery.in_flight.is_none())
            .await;
        cx.update(|_, cx| {
            let tray = &sidebar.read(cx).dashboard.as_ref().unwrap().tray;
            assert!(tray.read(cx).error.is_none());
            assert!(tray.read(cx).worktree_discovery.fetched_at.is_some());
            assert!(!tray.read(cx).worktrees.is_empty());
        });
        let (workspace, discarded_generation) = cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let workspace = sidebar.active_workspace;
                sidebar.force_refresh_dashboard_worktrees(cx);
                let generation = sidebar.dashboard.as_ref().unwrap().tray.read(cx).generation;
                // Change the workspace before the actual Git result is delivered.
                cx.global_mut::<SettingsState>().store.workspaces.active =
                    workspace.wrapping_add(1);
                (workspace, generation)
            })
        });
        cx.cx
            .condition(&tray, |tray, _| tray.worktree_discovery.in_flight.is_none())
            .await;
        cx.update(|window, cx| {
            cx.global_mut::<SettingsState>().store.workspaces.active = workspace;
            sidebar.update(cx, |sidebar, cx| {
                let tray = sidebar.dashboard.as_ref().unwrap().tray.clone();
                assert!(tray.read(cx).worktree_discovery.in_flight.is_none());
                assert!(tray.read(cx).worktree_discovery.fetched_at.is_none());
                sidebar.sync(window, cx);
                assert!(sidebar.open_dashboard_for_project(0, &path, window, cx));
                assert_ne!(
                    tray.read(cx).generation,
                    discarded_generation,
                    "returning must start a new request after discarding a hidden result"
                );
            });
        });
        cx.cx
            .condition(&tray, |tray, _| tray.worktree_discovery.in_flight.is_none())
            .await;
        cx.update(|_, cx| {
            let tray = &sidebar.read(cx).dashboard.as_ref().unwrap().tray;
            assert!(tray.read(cx).worktree_discovery.fetched_at.is_some());
            assert!(tray.read(cx).error.is_none());
        });
    }

    // Exercise the view emitted event through Sidebar's real subscription and runtime.
    // The invalid boundary deliberately stops before any provider process is launched.
    fn adopted_session_routes_fork_event(
        cx: &mut gpui::TestAppContext,
        provider: grove_core::codex::Provider,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        let view = cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.open_dashboard(window, cx);
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
                let (handle, _actions) = grove_core::codex::Handle::channel();
                let (_updates, receive) = std::sync::mpsc::channel();
                let mut state = grove_core::codex::Snapshot::new(saved);
                state.status = grove_core::codex::Status::Idle;
                let session = cx.new(|cx| {
                    CodexSession::from_connection(
                        state,
                        grove_core::codex::Connection {
                            handle,
                            updates: receive.into(),
                        },
                        cx,
                    )
                });
                let view = cx.new(|cx| CodexView::new(session.clone(), window, cx));
                sidebar.adopt_dashboard_launch(NO_PROJECT, session, view.clone(), window, cx);
                view
            })
        });
        view.update(cx, |_, cx| {
            cx.emit(CodexViewEvent::ForkRequested(usize::MAX, false));
        });
        cx.run_until_parked();
        sidebar.read_with(cx, |sidebar, cx| {
            let toast = sidebar.runtime.read(cx).toast.read(cx).current();
            assert_eq!(
                toast.map(|toast| toast.message.as_str()),
                Some("This conversation boundary is unavailable."),
                "the adopted view must route Fork to the runtime instead of silently dropping it"
            );
        });
    }

    #[gpui::test]
    fn dashboard_adopted_codex_routes_fork_event(cx: &mut gpui::TestAppContext) {
        adopted_session_routes_fork_event(cx, grove_core::codex::Provider::Codex);
    }
    #[gpui::test]
    fn dashboard_adopted_claude_routes_fork_event(cx: &mut gpui::TestAppContext) {
        adopted_session_routes_fork_event(cx, grove_core::codex::Provider::Claude);
    }

    #[gpui::test]
    fn activity_includes_projectless_codex_and_claude_without_other_workspace_sessions(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        let (visible, hidden) = cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.open_dashboard(window, cx);
                let registry = sidebar.runtime.read(cx).registry.clone();
                let ids = registry.update(cx, |registry, cx| {
                    let visible = [Agent::Codex, Agent::Claude].map(|agent| {
                        registry.insert_meta(String::new(), dashboard_home_dir(), agent)
                    });
                    let hidden = registry.insert_meta(
                        "Other workspace".into(),
                        "/tmp/other-workspace".into(),
                        Agent::Codex,
                    );
                    cx.notify();
                    (visible, hidden)
                });
                sidebar.sync(window, cx);
                ids
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        for id in visible {
            assert!(cx
                .debug_bounds(Box::leak(
                    format!("dashboard-terminal-session-{}", id.raw()).into_boxed_str()
                ))
                .is_some());
        }
        assert!(cx
            .debug_bounds(Box::leak(
                format!("dashboard-terminal-session-{}", hidden.raw()).into_boxed_str()
            ))
            .is_none());
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let canvas = sidebar.active_canvas_sessions(cx);
                assert!(visible.iter().all(|id| canvas.contains(&(*id, false))));
                assert!(!canvas.contains(&(hidden, false)));
                sidebar.select(Selection::Session(visible[0]), cx);
                sidebar.sync(window, cx);
                assert_eq!(sidebar.selection, Some(Selection::Session(visible[0])));
            });
        });
    }

    #[gpui::test]
    fn project_picker_destination_preserves_codex_and_claude_composer(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let mut store = grove_core::storage::Store::default();
            for (name, path) in [
                ("First", "/tmp/picker-first"),
                ("Second", "/tmp/picker-second"),
            ] {
                store.projects.push(grove_core::storage::Project {
                    name: name.into(),
                    path: path.into(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                });
                store.assign_project_to_active_workspace(path);
            }
            cx.set_global(SettingsState::new(store));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.open_dashboard(window, cx);
                for provider in [
                    grove_core::codex::Provider::Codex,
                    grove_core::codex::Provider::Claude,
                ] {
                    let dashboard = sidebar.dashboard.as_ref().unwrap();
                    let session = dashboard.session.clone();
                    let view = dashboard.view.clone();
                    session.update(cx, |session, _| {
                        session.state.saved.provider = provider;
                        session.state.saved.draft = "Retain literal draft".into();
                        session.state.saved.composer.model = Some("retained-model".into());
                        session.state.saved.composer.effort = Some("high".into());
                        session.state.saved.config_args = vec!["retained-config".into()];
                    });
                    assert!(!sidebar.open_dashboard_for_project(1, "/stale", window, cx));
                    assert!(sidebar.open_dashboard_for_project(
                        1,
                        "/tmp/picker-second",
                        window,
                        cx
                    ));
                    let dashboard = sidebar.dashboard.as_ref().unwrap();
                    assert_eq!(dashboard.session, session);
                    assert_eq!(dashboard.view, view);
                    let saved = &session.read(cx).state.saved;
                    assert_eq!(saved.provider, provider);
                    assert_eq!(saved.draft, "Retain literal draft");
                    assert_eq!(saved.composer.model.as_deref(), Some("retained-model"));
                    assert_eq!(saved.composer.effort.as_deref(), Some("high"));
                    assert_eq!(saved.config_args, ["retained-config"]);
                    assert_eq!(
                        sidebar.dashboard_destination_for_test(cx),
                        Some((1, "/tmp/picker-second".into()))
                    );
                    assert!(sidebar.dashboard_open);
                    assert_eq!(sidebar.runtime.read(cx).registry.read(cx).len(), 0);
                }
            });
        });
    }

    struct InsetDashboardFixture(Entity<Sidebar>);
    impl Render for InsetDashboardFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().pt(gpui::px(100.)).child(self.0.clone())
        }
    }

    #[gpui::test]
    fn dashboard_position_uses_its_own_inset_viewport(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let mut store = grove_core::storage::Store::default();
            let path = "/tmp/grove-dashboard-inset-fixture";
            store.projects.push(grove_core::storage::Project {
                name: "Dashboard fixture".into(),
                path: path.into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            });
            store.assign_project_to_active_workspace(path);
            cx.set_global(SettingsState::new(store));
        });
        let (fixture, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
            InsetDashboardFixture(sidebar)
        });
        cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(900.)));
        cx.run_until_parked();
        cx.update(|_, cx| {
            let sidebar = fixture.read(cx).0.clone();
            let session = sidebar.read(cx).dashboard.as_ref().unwrap().session.clone();
            session.update(cx, |session, cx| {
                session.catalog_loading = false;
                session.catalog_error = Some("Provider options unavailable".into());
                session.state.error = Some("Launch failed".into());
                cx.notify();
            });
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let page = cx.debug_bounds("dashboard-page").unwrap();
        let composer = cx.debug_bounds("codex-composer").unwrap();
        assert_eq!(page.top(), gpui::px(100.));
        assert!(
            (composer.top() - page.top() - page.size.height * DASHBOARD_COMPOSER_TOP_FRACTION)
                .abs()
                < gpui::px(2.),
            "page: {page:?}, composer: {composer:?}"
        );
    }

    #[gpui::test]
    fn dashboard_adoption_keeps_composer_anchored_on_first_frame(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let mut store = grove_core::storage::Store::default();
            let path = "/tmp/grove-dashboard-inset-adoption";
            store.projects.push(grove_core::storage::Project {
                name: "Dashboard fixture".into(),
                path: path.into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            });
            store.assign_project_to_active_workspace(path);
            cx.set_global(SettingsState::new(store));
        });
        let (fixture, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
            InsetDashboardFixture(sidebar)
        });
        cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(900.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let before = cx.debug_bounds("codex-composer").unwrap().top();
        let (_updates, receive) = std::sync::mpsc::channel();
        let (handle, _actions) = grove_core::codex::Handle::channel();
        cx.update(|window, cx| {
            let sidebar = fixture.read(cx).0.clone();
            sidebar.update(cx, |sidebar, cx| {
                let saved = sidebar
                    .dashboard
                    .as_ref()
                    .unwrap()
                    .session
                    .read(cx)
                    .state
                    .saved
                    .clone();
                let session = cx.new(|cx| {
                    CodexSession::from_connection(
                        grove_core::codex::Snapshot::new(saved),
                        grove_core::codex::Connection {
                            handle,
                            updates: receive.into(),
                        },
                        cx,
                    )
                });
                let view = cx.new(|cx| CodexView::new(session.clone(), window, cx));
                let measured = sidebar.dashboard.as_ref().unwrap().viewport_height.get();
                assert_eq!(measured, gpui::px(800.));
                assert_eq!(window.viewport_size().height, gpui::px(900.));
                sidebar.adopt_dashboard_launch(0, session, view, window, cx);
                assert_eq!(
                    sidebar.dashboard.as_ref().unwrap().viewport_height.get(),
                    measured,
                    "the replacement must have the measured height before its first layout"
                );
            });
            let _ = window.draw(cx);
        });
        assert_eq!(
            cx.debug_bounds("codex-composer").unwrap().top(),
            before,
            "the first replacement frame must use the measured dashboard height"
        );
        cx.run_until_parked();
        cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(1000.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let page = cx.debug_bounds("dashboard-page").unwrap();
        let composer = cx.debug_bounds("codex-composer").unwrap();
        assert!(
            (composer.top() - page.top() - page.size.height * DASHBOARD_COMPOSER_TOP_FRACTION)
                .abs()
                < gpui::px(2.)
        );
    }

    // Exercise the real adoption/fresh-draft owner with mocked connected sessions.
    // Live provider normalization and native submit are verified separately.
    fn sequential_dashboard_adoption(
        cx: &mut gpui::TestAppContext,
        provider: grove_core::codex::Provider,
        reduced_motion: bool,
    ) {
        let repo = tempfile::tempdir().unwrap();
        let first = repo.path().join("first");
        let selected = repo.path().join("selected");
        let checkout = repo.path().join("selected-checkout");
        for path in [&first, &selected, &checkout] {
            fs_err::create_dir(path).unwrap();
        }
        let checkout = checkout.to_string_lossy().into_owned();
        cx.update(|cx| {
            cx.set_reduce_motion(reduced_motion);
            gpui_component::init(cx);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let mut store = grove_core::storage::Store::default();
            for (name, path) in [("First", &first), ("Selected", &selected)] {
                let path = path.to_string_lossy().into_owned();
                store.projects.push(grove_core::storage::Project {
                    name: name.into(),
                    path: path.clone(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                });
                store.assign_project_to_active_workspace(&path);
            }
            cx.set_global(SettingsState::new(store));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.run_until_parked();
        let mut receivers = Vec::new();
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.open_dashboard(window, cx);
                let initial_key = sidebar
                    .dashboard
                    .as_ref()
                    .unwrap()
                    .session
                    .read(cx)
                    .state
                    .saved
                    .key
                    .clone();
                let mut fresh_keys = vec![initial_key];
                let mut adopted_ids = Vec::new();
                for turn in 1..=2 {
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
                    saved.project = "Selected".into();
                    saved.cwd = checkout.clone();
                    saved.key = format!("mock-dashboard-turn-{turn}");
                    saved.draft = format!("Independent prompt {turn}");
                    saved.composer.model = Some("fixture-model".into());
                    saved.composer.effort = Some("high".into());
                    saved.composer.access = Some("fixture-permission".into());
                    saved.composer.speed = Some(grove_core::codex::Speed::Fast);
                    saved.config_args = vec!["fixture-configuration".into()];
                    sidebar
                        .dashboard
                        .as_ref()
                        .unwrap()
                        .tray
                        .update(cx, |tray, _| {
                            tray.project = 1;
                            tray.path = checkout.clone();
                            tray.worktrees = vec![(checkout.clone(), "feature-branch".into())];
                            tray.loading = false;
                            tray.error = None;
                        });
                    let (handle, actions) = grove_core::codex::Handle::channel();
                    receivers.push(actions);
                    let (_updates, receive) = std::sync::mpsc::channel();
                    let session = cx.new(|cx| {
                        CodexSession::from_connection(
                            grove_core::codex::Snapshot::new(saved),
                            grove_core::codex::Connection {
                                handle,
                                updates: receive.into(),
                            },
                            cx,
                        )
                    });
                    let view = cx.new(|cx| CodexView::new(session.clone(), window, cx));
                    sidebar.adopt_dashboard_launch(1, session.clone(), view.clone(), window, cx);
                    assert!(
                        sidebar.dashboard_open,
                        "successful submit remains on Dashboard"
                    );
                    let d = sidebar.dashboard.as_ref().unwrap();
                    let next = &d.session.read(cx).state.saved;
                    assert_ne!(d.session, session);
                    assert_ne!(d.view, view);
                    assert!(d.session.read(cx).is_deferred());
                    assert_eq!(next.provider, provider);
                    assert_eq!(next.cwd, checkout);
                    assert_eq!(d.tray.read(cx).project, 1);
                    assert_eq!(d.tray.read(cx).path, checkout);
                    assert_eq!(
                        d.tray.read(cx).worktrees,
                        [(checkout.clone(), "feature-branch".into())]
                    );
                    assert!(
                        !d.tray.read(cx).loading,
                        "cached destination remains visible during refresh"
                    );
                    assert_eq!(next.composer.model.as_deref(), Some("fixture-model"));
                    assert_eq!(next.composer.effort.as_deref(), Some("high"));
                    assert_eq!(next.composer.access.as_deref(), Some("fixture-permission"));
                    assert_eq!(next.composer.speed, Some(grove_core::codex::Speed::Fast));
                    assert_eq!(next.config_args, ["fixture-configuration"]);
                    assert!(next.draft.is_empty());
                    assert!(next.draft_images.is_empty());
                    assert!(next.draft_reference.is_none());
                    assert!(next.thread_id.is_none());
                    assert!(next.items.is_empty());
                    assert!(!fresh_keys.contains(&next.key));
                    fresh_keys.push(next.key.clone());
                    let registry = sidebar.runtime.read(cx).registry.read(cx);
                    assert_eq!(registry.all().len(), turn);
                    adopted_ids.push(registry.all().last().unwrap().id);
                    assert_eq!(
                        sidebar
                            .dashboard_timelines
                            .is_entering(sidebar.active_workspace, *adopted_ids.last().unwrap()),
                        !reduced_motion,
                        "only adopted sessions receive entrance state; reduced motion skips it"
                    );
                    assert_eq!(
                        session.read(cx).state.saved.draft,
                        format!("Independent prompt {turn}")
                    );
                }
                assert_ne!(adopted_ids[0], adopted_ids[1]);
                sidebar.open_dashboard(window, cx);
                for id in adopted_ids {
                    assert!(!sidebar
                        .dashboard_timelines
                        .is_entering(sidebar.active_workspace, id));
                }
            });
        });
    }
    #[gpui::test]
    fn dashboard_sequential_codex_submits_keep_fresh_composer_and_destination(
        cx: &mut gpui::TestAppContext,
    ) {
        sequential_dashboard_adoption(cx, grove_core::codex::Provider::Codex, false);
    }
    #[gpui::test]
    fn dashboard_sequential_claude_submits_keep_fresh_composer_and_destination(
        cx: &mut gpui::TestAppContext,
    ) {
        sequential_dashboard_adoption(cx, grove_core::codex::Provider::Claude, false);
    }
    #[gpui::test]
    fn dashboard_codex_reduced_motion_skips_entrance(cx: &mut gpui::TestAppContext) {
        sequential_dashboard_adoption(cx, grove_core::codex::Provider::Codex, true);
    }
    #[gpui::test]
    fn dashboard_claude_reduced_motion_skips_entrance(cx: &mut gpui::TestAppContext) {
        sequential_dashboard_adoption(cx, grove_core::codex::Provider::Claude, true);
    }
    fn worktree_round_trip(cx: &mut gpui::TestAppContext, provider: grove_core::codex::Provider) {
        let fixture = tempfile::tempdir().unwrap();
        let repo = fixture.path().join("repo");
        fs_err::create_dir(&repo).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "Fixture",
            ],
        ] {
            assert!(std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let worktree_root = grove_core::git::worktrees_root().unwrap();
        fs_err::create_dir_all(&worktree_root).unwrap();
        let worktree_fixture = tempfile::Builder::new()
            .prefix("grove-dashboard-roundtrip-")
            .tempdir_in(worktree_root)
            .unwrap();
        let worktree_dir = worktree_fixture
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let project_path = repo.to_string_lossy().into_owned();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let mut store = grove_core::storage::Store::default();
            store.projects.push(grove_core::storage::Project {
                name: "Round trip".into(),
                path: project_path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: Some(worktree_dir),
            });
            store.assign_project_to_active_workspace(&project_path);
            cx.set_global(SettingsState::new(store));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(800.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.open_dashboard(window, cx);
                let d = sidebar.dashboard.as_ref().unwrap();
                d.session.update(cx, |session, cx| {
                    session.state.saved.provider = provider;
                    cx.notify();
                });
            });
            let _ = window.draw(cx);
        });
        cx.simulate_input("Keep this Dashboard draft");
        cx.run_until_parked();
        let (session_id, view_id) = cx.update(|_, cx| {
            let d = sidebar.read(cx).dashboard.as_ref().unwrap();
            assert_eq!(
                d.session.read(cx).state.saved.draft,
                "Keep this Dashboard draft"
            );
            (d.session.entity_id(), d.view.entity_id())
        });
        for cancel in [true, false] {
            cx.update(|_, cx| {
                let tray = sidebar.read(cx).dashboard.as_ref().unwrap().tray.clone();
                tray.update(cx, |_, cx| cx.emit(DestinationEvent::NewWorktree));
            });
            cx.run_until_parked();
            cx.update(|_, cx| assert_eq!(sidebar.read(cx).pending_new_worktree, Some(0)));
            cx.simulate_input(if cancel {
                "discarded-worktree"
            } else {
                "dashboard-round-trip"
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    if cancel {
                        sidebar.act(Action::Cancel, window, cx);
                    } else {
                        sidebar.submit_worktree(cx);
                        assert!(
                            sidebar.worktree_errors.iter().all(Option::is_none),
                            "{:?}",
                            sidebar.worktree_errors
                        );
                    }
                    assert!(sidebar.pending_new_worktree.is_none());
                    assert!(sidebar.dashboard_open);
                    let d = sidebar.dashboard.as_ref().unwrap();
                    assert_eq!(d.session.entity_id(), session_id);
                    assert_eq!(d.view.entity_id(), view_id);
                    assert_eq!(
                        d.session.read(cx).state.saved.draft,
                        "Keep this Dashboard draft"
                    );
                    assert!(d.session.read(cx).is_deferred());
                    assert_eq!(sidebar.runtime.read(cx).registry.read(cx).len(), 0);
                    if cancel {
                        assert_eq!(d.tray.read(cx).path, project_path);
                    } else {
                        let expected = worktree_fixture
                            .path()
                            .join("dashboard-round-trip")
                            .to_string_lossy()
                            .into_owned();
                        assert_eq!(d.tray.read(cx).path, expected);
                        assert_eq!(d.session.read(cx).state.saved.cwd, expected);
                        assert!(std::path::Path::new(&expected).join(".git").exists());
                    }
                    d.view.focus_handle(cx).focus(window, cx);
                });
            });
        }
        cx.simulate_input(" retained after creation");
        cx.run_until_parked();
        cx.update(|_, cx| {
            let d = sidebar.read(cx).dashboard.as_ref().unwrap();
            assert_eq!(
                d.session.read(cx).state.saved.draft,
                "Keep this Dashboard draft retained after creation"
            );
        });
    }
    #[gpui::test]
    fn codex_dashboard_worktree_round_trip_retains_composer(cx: &mut gpui::TestAppContext) {
        worktree_round_trip(cx, grove_core::codex::Provider::Codex);
    }
    #[gpui::test]
    fn claude_dashboard_worktree_round_trip_retains_composer(cx: &mut gpui::TestAppContext) {
        worktree_round_trip(cx, grove_core::codex::Provider::Claude);
    }

    #[gpui::test]
    fn activity_growth_keeps_heading_and_composer_anchored(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_reduce_motion(true);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let mut store = grove_core::storage::Store::default();
            let path = "/tmp/grove-dashboard-anchor-fixture";
            store.projects.push(grove_core::storage::Project {
                name: "Dashboard visual fixture".into(),
                path: path.into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            });
            store.assign_project_to_active_workspace(path);
            cx.set_global(SettingsState::new(store));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        for (width, height) in [(1100., 900.), (560., 500.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let summary = cx.debug_bounds("dashboard-summary").unwrap().top();
            let composer = cx.debug_bounds("codex-composer").unwrap().top();
            let id = cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.open_dashboard(window, cx);
                    seed_activity_fixture(sidebar, grove_core::codex::Provider::Codex, window, cx)
                })
            });
            for expanded in [false, true, false] {
                cx.update(|_, cx| {
                    sidebar.update(cx, |sidebar, cx| {
                        if sidebar
                            .dashboard_timelines
                            .is_open(sidebar.active_workspace, id)
                            != expanded
                        {
                            sidebar
                                .dashboard_timelines
                                .toggle((sidebar.active_workspace, id));
                        }
                        cx.notify();
                    });
                });
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                assert_eq!(summary, cx.debug_bounds("dashboard-summary").unwrap().top());
                assert_eq!(composer, cx.debug_bounds("codex-composer").unwrap().top());
            }
            cx.update(|_,cx| {
                let session=sidebar.read(cx).runtime.read(cx).registry.read(cx).codex(id).unwrap().clone();
                session.update(cx,|session,cx| {
                    session.state.saved.items.push(serde_json::json!({"type":"agentMessage","text":"A new observed activity step with enough text to wrap at narrow window widths."}));
                    cx.notify();
                });
                sidebar.update(cx,|sidebar,cx| {
                    let items=&session.read(cx).state.saved.items;
                    sidebar.task_glances.get_mut(&id).unwrap().refresh_dashboard(items,grove_core::codex::Status::Working);
                    cx.notify();
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert_eq!(summary, cx.debug_bounds("dashboard-summary").unwrap().top());
            assert_eq!(composer, cx.debug_bounds("codex-composer").unwrap().top());
        }
    }

    #[gpui::test]
    fn dashboard_opens_at_startup_and_groups_content(cx: &mut gpui::TestAppContext) {
        let repo = tempfile::tempdir().unwrap();
        let path = repo.path().to_string_lossy().into_owned();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let mut store = grove_core::storage::Store::default();
            store.projects.push(grove_core::storage::Project {
                name: "Test project".into(),
                path: path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            });
            store.assign_project_to_active_workspace(&path);
            cx.set_global(SettingsState::new(store));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(900.)));
        cx.run_until_parked();
        cx.update(|_, cx| {
            let session = sidebar.read(cx).dashboard.as_ref().unwrap().session.clone();
            session.update(
                cx,
                crate::entities::codex_session::CodexSession::settle_draft_catalog_for_test,
            );
        });
        cx.update(|_, cx| {
            let tray = sidebar.read(cx).dashboard.as_ref().unwrap().tray.clone();
            tray.update(cx, |tray, cx| {
                tray.set_preparation_for_test(false, None, cx);
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.update(|_, cx| {
            assert!(sidebar.read(cx).dashboard_open);
            assert!(sidebar.read(cx).dashboard.is_some());
            assert_eq!(sidebar.read(cx).runtime.read(cx).registry.read(cx).len(), 0);
        });
        let summary = cx.debug_bounds("dashboard-summary").unwrap();
        let composer = cx.debug_bounds("codex-composer").unwrap();
        let recent = cx.debug_bounds("dashboard-recent").unwrap();
        let tray = cx.debug_bounds("dashboard-destination").unwrap();
        let hints = cx.debug_bounds("codex-composer-hints").unwrap();
        let page = cx.debug_bounds("dashboard-page").unwrap();
        assert!(
            (composer.top() - page.top() - page.size.height * DASHBOARD_COMPOSER_TOP_FRACTION)
                .abs()
                < gpui::px(2.),
            "page: {page:?}, composer: {composer:?}"
        );
        assert!(composer.top() >= summary.bottom());
        assert!(composer.top() - summary.bottom() < gpui::px(100.));
        assert_eq!(tray.top(), composer.bottom());
        assert!(hints.top() >= tray.bottom());
        let heading = cx.debug_bounds("dashboard-activity-heading").unwrap();
        assert_eq!(heading.left(), recent.left());
        assert!(heading.bottom() < recent.top());
        assert_eq!(recent.left(), composer.left());
        assert_eq!(recent.size.width, composer.size.width);
        assert!(recent.top() >= hints.bottom());
        assert!(
            heading.top() - hints.bottom() < gpui::px(50.),
            "heading={heading:?}; hints={hints:?}"
        );
        assert!(composer.size.height > gpui::px(50.));
        assert!(
            composer.bottom() <= page.bottom(),
            "composer must fit beside checkout tools"
        );
        if let Some(checkout) = cx.debug_bounds("checkout-panel") {
            assert!(composer.right() <= checkout.left());
        }

        cx.simulate_resize(gpui::size(gpui::px(560.), gpui::px(500.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let summary = cx.debug_bounds("dashboard-summary").unwrap();
        let composer = cx.debug_bounds("codex-composer").unwrap();
        assert!(composer.top() >= summary.bottom());
        assert!(composer.size.width > gpui::px(100.));
        let recent = cx.debug_bounds("dashboard-recent").unwrap();
        let heading = cx.debug_bounds("dashboard-activity-heading").unwrap();
        assert_eq!(heading.left(), recent.left());
        assert!(heading.bottom() < recent.top());
        assert_eq!(recent.left(), composer.left());
        assert_eq!(recent.size.width, composer.size.width);
        assert!(composer.top() - summary.bottom() < gpui::px(100.));
        // The compact layout fits at 500px; use a short viewport to exercise scrolling.
        cx.simulate_resize(gpui::size(gpui::px(560.), gpui::px(200.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let page = cx.debug_bounds("dashboard-page").unwrap();
        let recent_top = cx.debug_bounds("dashboard-recent").unwrap().top();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: page.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-300.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("dashboard-recent").unwrap().top() < recent_top);

        // A workspace switch lands on the actual Dashboard even from grid mode.
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.toggle_grid(window, cx);
            });
            let store = &mut cx.global_mut::<SettingsState>().store;
            store.workspaces.create("Other workspace").unwrap();
            store.assign_project_to_active_workspace(&path);
            sidebar.update(cx, |sidebar, cx| sidebar.sync(window, cx));
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert_eq!(sidebar.read(cx).mode, ViewMode::Grid);
            assert!(sidebar.read(cx).dashboard_open);
            assert_eq!(sidebar.read(cx).runtime.read(cx).registry.read(cx).len(), 0);
        });
        assert!(cx.debug_bounds("dashboard-summary").is_some());
        assert!(cx.debug_bounds("codex-composer").is_some());
        assert!(cx.debug_bounds("dashboard-destination").is_some());
    }

    #[gpui::test]
    fn destination_popup_rise_cleanup_cannot_unmount_a_new_menu(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (tray, cx) = cx.add_window_view(|_, _| {
            DestinationTray::new(
                vec![(0, "Project".into(), "/checkout".into())],
                0,
                "/checkout".into(),
            )
        });
        cx.update(|window, cx| {
            cx.set_reduce_motion(false);
            tray.update(cx, |tray, cx| {
                tray.open(true, window, cx);
                tray.close_popup(cx);
                assert!(tray.popup.is_none());
                assert!(tray.closing_popup.is_some());
                tray.open(false, window, cx);
            });
        });
        cx.executor()
            .advance_clock(motion::PopupMotion::exit_duration() * 2);
        cx.run_until_parked();
        cx.update(|window, cx| {
            tray.update(cx, |tray, cx| {
                assert!(tray.popup.is_some());
                assert!(tray.open_selector == Some(1));
                cx.set_reduce_motion(true);
                tray.close_popup(cx);
                assert!(tray.closing_popup.is_none());
                tray.open(true, window, cx);
                tray.close_popup(cx);
                assert!(tray.closing_popup.is_none());
            });
        });
    }

    #[gpui::test]
    fn dashboard_destination_controls_size_to_content(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (tray, cx) = cx.add_window_view(|_, _| {
            let mut tray = DestinationTray::new(
                vec![(
                    0,
                    "A substantially longer project".into(),
                    "/checkout".into(),
                )],
                0,
                "/checkout".into(),
            );
            tray.worktrees = vec![("/checkout".into(), "main".into())];
            tray
        });
        cx.simulate_resize(gpui::size(gpui::px(1000.), gpui::px(300.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.update(|_, cx| {
            let tray = tray.read(cx);
            let project = tray.bounds[0].get();
            let worktree = tray.bounds[1].get();
            assert!(
                project.size.width > worktree.size.width,
                "long project grows independently of the short worktree"
            );
            assert!(project.size.width < gpui::px(400.));
            assert!(worktree.size.width < gpui::px(200.));
        });
        let strip = cx.debug_bounds("dashboard-destination").unwrap();
        assert_eq!(
            strip.size.width,
            gpui::px(1000. * COMPOSER_CONTEXT_WIDTH_FRACTION)
        );
        let bounds = cx.update(|_, cx| {
            let tray = tray.read(cx);
            [tray.bounds[0].get(), tray.bounds[1].get()]
        });
        assert_eq!(bounds[0].left(), strip.left() + gpui::px(SPACE_2XL + 1.));
        assert_eq!(bounds[1].right(), strip.right() - gpui::px(SPACE_2XL + 1.));
        for (idx, bounds) in bounds.iter().enumerate() {
            assert!(
                (f32::from(bounds.center().y - strip.center().y)).abs() <= 1.0,
                "selector is vertically centered in the strip"
            );
            cx.simulate_mouse_move(bounds.center(), None, gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
                let tray = tray.read(cx);
                assert!(tray.hovered[idx]);
                assert!(!tray.hovered[1 - idx]);
                assert_eq!(
                    tray.bounds[idx].get(),
                    *bounds,
                    "hover must not shift layout"
                );
            });
        }
        cx.simulate_mouse_move(
            gpui::point(strip.left() - gpui::px(1.), strip.center().y),
            None,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert_eq!(tray.read(cx).hovered, [false; 2]);
        });

        cx.simulate_resize(gpui::size(gpui::px(400.), gpui::px(300.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let strip = cx.debug_bounds("dashboard-destination").unwrap();
        cx.update(|_, cx| {
            let tray = tray.read(cx);
            let project = tray.bounds[0].get();
            let worktree = tray.bounds[1].get();
            assert!(
                worktree.top() >= project.bottom() || worktree.left() >= project.right(),
                "narrow selectors remain separate whether they fit or wrap"
            );
            assert_eq!(worktree.right(), strip.right() - gpui::px(SPACE_2XL + 1.));
        });
    }
    #[gpui::test]
    fn dashboard_status_follows_adopted_codex_and_claude_sessions(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_reduce_motion(true);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        for provider in [
            grove_core::codex::Provider::Codex,
            grove_core::codex::Provider::Claude,
        ] {
            let source = cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.open_dashboard(window, cx);
                    let saved = sidebar
                        .dashboard
                        .as_ref()
                        .unwrap()
                        .session
                        .read(cx)
                        .state
                        .saved
                        .clone();
                    let source = cx.new(|cx| {
                        let mut session = CodexSession::history(saved, cx);
                        session.state.saved.provider = provider;
                        session.state.status = grove_core::codex::Status::Starting;
                        session.begin_dashboard_handoff_for_test();
                        session
                    });
                    let old_view = cx.new(|cx| CodexView::new(source.clone(), window, cx));
                    sidebar.adopt_dashboard_launch(
                        NO_PROJECT,
                        source.clone(),
                        old_view.clone(),
                        window,
                        cx,
                    );
                    let dashboard = sidebar.dashboard.as_ref().unwrap();
                    assert_ne!(dashboard.view, old_view);
                    assert_ne!(dashboard.session, source);
                    dashboard.session.update(
                        cx,
                        crate::entities::codex_session::CodexSession::settle_draft_catalog_for_test,
                    );
                    assert_eq!(
                        dashboard
                            .view
                            .read(cx)
                            .dashboard_handoff_sessions(cx)
                            .as_slice(),
                        std::slice::from_ref(&source)
                    );
                    source
                })
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(
                cx.debug_bounds("dashboard-status-entry-0").is_some(),
                "the replacement Dashboard shows connection progress"
            );
            // Clear the actor-owned handoff, without touching the fresh draft.
            cx.update(|_, cx| {
                source.update(cx, |session, cx| {
                    session.cancel_dashboard_title_waiting();
                    session.state.status = grove_core::codex::Status::Done;
                    cx.notify();
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(
                cx.debug_bounds("dashboard-status-entry-0").is_none(),
                "completion removes the observed handoff"
            );
            sidebar.read_with(cx, |sidebar, cx| {
                let dashboard = sidebar.dashboard.as_ref().unwrap();
                assert!(dashboard.session.read(cx).state.saved.draft.is_empty());
                assert!(dashboard
                    .view
                    .read(cx)
                    .dashboard_handoff_sessions(cx)
                    .is_empty());
            });
        }
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod native_snapshots {
    use super::*;
    #[test]
    #[ignore = "requires native headless GPU renderer; writes isolated Dashboard screenshots to /tmp"]
    fn dashboard_native_snapshots() {
        use gpui::AppContext as _;
        let repo = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "Fixture",
            ],
        ] {
            assert!(std::process::Command::new("git")
                .current_dir(repo.path())
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let path = repo.path().to_string_lossy().into_owned();
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            std::sync::Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            let mut store = grove_core::storage::Store::default();
            store.projects.push(grove_core::storage::Project {
                name: "Dashboard visual fixture".into(),
                path: path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            });
            store.assign_project_to_active_workspace(&path);
            cx.set_global(SettingsState::new(store));
            grove_core::theme::set(grove_core::theme::by_name("grove-dark").unwrap());
            c::sync_component_theme(cx);
        });
        for (state, width) in [
            ("activity-projectless-codex", 1100.),
            ("activity-projectless-claude-narrow", 760.),
            ("activity-codex", 1100.),
            ("activity-claude-narrow", 760.),
            ("activity-codex-history", 1100.),
            ("activity-claude-narrow-history", 760.),
            ("normal", 1100.),
            ("narrow", 560.),
            ("worktree-menu", 1100.),
            ("project-menu", 1100.),
            ("narrow-worktree-menu", 560.),
            ("narrow-project-menu", 560.),
            ("hover", 1100.),
            ("worktree-hover", 1100.),
            ("light", 1100.),
            ("light-hover", 1100.),
        ] {
            cx.update(|cx| {
                let theme = if state.starts_with("light") {
                    "grove-light"
                } else {
                    "grove-dark"
                };
                grove_core::theme::set(grove_core::theme::by_name(theme).unwrap());
                c::sync_component_theme(cx);
            });
            let mut sidebar = None;
            let window = cx
                .open_window(gpui::size(gpui::px(width), gpui::px(760.)), |window, cx| {
                    let runtime = cx.new(crate::runtime::Runtime::new);
                    let view = cx.new(|cx| Sidebar::new(runtime, window, cx));
                    sidebar = Some(view.clone());
                    cx.new(|cx| {
                        gpui_component::Root::new(view, window, cx)
                            .bordered(false)
                            .bg(c::BG())
                    })
                })
                .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                sidebar.as_ref().unwrap().update(cx, |sidebar, cx| {
                    sidebar.open_dashboard(window, cx);
                    assert!(sidebar.dashboard_open);
                    assert!(sidebar.dashboard.is_some());
                    if state.starts_with("activity-") {
                        let provider = if state.contains("claude") {
                            grove_core::codex::Provider::Claude
                        } else {
                            grove_core::codex::Provider::Codex
                        };
                        if state.contains("projectless") {
                            sidebar
                                .dashboard
                                .as_ref()
                                .unwrap()
                                .session
                                .update(cx, |s, _| {
                                    s.state.saved.project.clear();
                                    s.state.saved.cwd = dashboard_home_dir();
                                });
                        }
                        let id = seed_activity_fixture(sidebar, provider, window, cx);
                        if state.ends_with("history") {
                            sidebar
                                .dashboard_timelines
                                .toggle((sidebar.active_workspace, id));
                        }
                    }
                });
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
            if state.ends_with("hover") {
                let idx = usize::from(state == "worktree-hover");
                cx.update_window(window.into(), |_, window, cx| {
                    let tray = sidebar
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .dashboard
                        .as_ref()
                        .unwrap()
                        .tray
                        .clone();
                    let position = tray.read(cx).bounds[idx].get().center();
                    window.dispatch_event(
                        gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                            position,
                            pressed_button: None,
                            modifiers: gpui::Modifiers::default(),
                        }),
                        cx,
                    );
                })
                .unwrap();
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    let _ = window.draw(cx);
                    let tray = &sidebar
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .dashboard
                        .as_ref()
                        .unwrap()
                        .tray;
                    assert!(
                        tray.read(cx).hovered[idx],
                        "native pointer must enter selector"
                    );
                })
                .unwrap();
            }
            if state.ends_with("menu") {
                cx.update_window(window.into(), |_, window, cx| {
                    let tray = sidebar
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .dashboard
                        .as_ref()
                        .unwrap()
                        .tray
                        .clone();
                    tray.update(cx, |tray, cx| {
                        tray.open(state.ends_with("project-menu"), window, cx);
                    });
                    let _ = window.draw(cx);
                })
                .unwrap();
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    let _ = window.draw(cx);
                })
                .unwrap();
            }
            let output = format!("/tmp/grove-dashboard-{state}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&output)
                .unwrap();
            eprintln!("Native Dashboard fixture: {output}");
        }
    }
}
