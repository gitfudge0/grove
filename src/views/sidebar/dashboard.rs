//! Dashboard destination state; the composer remains the shared native editor.
use super::*;
use crate::entities::codex_session::CodexSession;
use crate::views::codex_view::{CodexView, CodexViewEvent, CHAT_MAX_W};

const DASHBOARD_RECENT_MAX_W: f32 = CHAT_MAX_W * 0.7;

/// Preferences for a fresh dashboard draft after adopting the previous one.
struct DashboardDefaults {
    project: usize,
    path: String,
    provider: grove_core::codex::Provider,
    composer: grove_core::codex::ComposerSettings,
    config_args: Vec<String>,
}
pub(super) struct Dashboard {
    pub session: Entity<CodexSession>,
    pub view: Entity<CodexView>,
    pub tray: Entity<DestinationTray>,
    _events: Vec<gpui::Subscription>,
}
pub(crate) enum DestinationEvent {
    Project(usize, String),
    Worktree(String),
    NewWorktree,
}
pub(crate) struct DestinationTray {
    projects: Vec<(usize, String, String)>,
    worktrees: Vec<(String, String)>,
    loading: bool,
    generation: u64,
    error: Option<String>,
    project: usize,
    pub(super) path: String,
    popup: Option<Entity<gpui_component::menu::PopupMenu>>,
    popup_events: Option<gpui::Subscription>,
    hovered: [bool; 2],
    open_selector: Option<usize>,
    position: gpui::Point<gpui::Pixels>,
    bounds: [std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>; 2],
}
impl EventEmitter<DestinationEvent> for DestinationTray {}
impl DestinationTray {
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
            popup: None,
            popup_events: None,
            hovered: [false; 2],
            open_selector: None,
            position: gpui::Point::default(),
            bounds: std::array::from_fn(|_| std::rc::Rc::default()),
        }
    }
    fn open(&mut self, project: bool, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_component::menu::{PopupMenu, PopupMenuItem};
        let owner = cx.entity().downgrade();
        let projects = self.projects.clone();
        let worktrees = self.worktrees.clone();
        let selected_project = self.project;
        let selected_path = self.path.clone();
        let focus = window.focused(cx);
        let popup = PopupMenu::build(window, cx, move |mut menu, window, _| {
            menu = menu
                .scrollable(true)
                .item_padding(
                    rpx(SPACE_2XL).to_pixels(window.rem_size()),
                    rpx(SPACE_MD).to_pixels(window.rem_size()),
                )
                .max_h(rpx(MODAL_SCROLL_MAX_H).to_pixels(window.rem_size()));
            if project {
                for (idx, name, path) in projects {
                    let owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(name)
                            .checked(idx == selected_project)
                            .on_click(move |_, _, cx| {
                                let _ = owner.update(cx, |_, cx| {
                                    cx.emit(DestinationEvent::Project(idx, path.clone()));
                                });
                            }),
                    );
                }
            } else {
                for (path, name) in worktrees {
                    let owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::new(name)
                            .checked(path == selected_path)
                            .on_click(move |_, _, cx| {
                                let _ = owner.update(cx, |_, cx| {
                                    cx.emit(DestinationEvent::Worktree(path.clone()));
                                });
                            }),
                    );
                }
                menu = menu
                    .separator()
                    .item(
                        PopupMenuItem::new("New worktree").on_click(move |_, _, cx| {
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
                this.popup = None;
                this.open_selector = None;
                this.popup_events = None;
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
            .map_or_else(|| "Select project".into(), |p| p.1.clone());
        let worktree = self
            .worktrees
            .iter()
            .find(|p| p.0 == self.path)
            .map_or_else(|| "Select worktree".into(), |p| p.1.clone());
        let mut tray = div()
            .id("dashboard-destination")
            .debug_selector(|| "dashboard-destination".into())
            .relative()
            .mx(rpx(SPACE_2XL))
            // Cancel the composer rail gap so this strip meets its bottom seam.
            .mt(rpx(-SPACE_MD))
            .pt_0()
            .pb(rpx(SPACE_XS))
            .px(rpx(SPACE_2XL))
            .rounded_b(rpx(RADIUS_PANEL))
            .border_l_1()
            .border_r_1()
            .border_b_1()
            .border_color(c::BORDER_SOFT())
            .bg(c::BG_RAIL())
            .flex()
            .flex_wrap()
            .gap(rpx(SPACE_SM));
        for (idx, (is_project, label, glyph)) in
            [(true, project, "folder"), (false, worktree, "git-branch")]
                .into_iter()
                .enumerate()
        {
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
                        super::super::components::dense_action_with_colors(
                            if is_project {
                                "dashboard-project"
                            } else {
                                "dashboard-worktree"
                            },
                            label.clone(),
                            c::BG_RAIL(),
                            foreground,
                            c::BG_RAIL(),
                            window,
                            cx,
                        )
                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                        .when(self.open_selector == Some(idx), |button| {
                            button.bg(c::BG_HOVER())
                        })
                        .icon_only()
                        .tooltip(label.clone())
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
        if self.loading {
            tray = tray.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child("Loading worktrees…"),
            );
        }
        if let Some(error) = &self.error {
            tray = tray.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FORM_ERROR())
                    .child(error.clone()),
            );
        }
        if let Some(popup) = &self.popup {
            tray = tray.child(gpui::deferred(
                gpui::anchored()
                    .position_mode(gpui::AnchoredPositionMode::Window)
                    .anchor(if self.open_selector == Some(1) {
                        gpui::Anchor::TopRight
                    } else {
                        gpui::Anchor::TopLeft
                    })
                    .position(self.position)
                    .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
                    .child(
                        div()
                            .id("dashboard-destination-menu")
                            .debug_selector(|| "dashboard-destination-menu".into())
                            .child(popup.clone()),
                    ),
            ));
        }
        tray
    }
}
impl Sidebar {
    pub(crate) fn open_dashboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_dashboard_with_defaults(None, window, cx);
    }
    fn open_dashboard_with_defaults(
        &mut self,
        defaults: Option<DashboardDefaults>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            let Some((idx, _, _)) = selected else {
                self.dashboard_open = true;
                cx.notify();
                return;
            };
            let idx = *idx;
            let project = cx.global::<SettingsState>().store.projects[idx].clone();
            let provider = defaults
                .as_ref()
                .map_or(grove_core::codex::Provider::Codex, |defaults| {
                    defaults.provider
                });
            let checkout = defaults
                .as_ref()
                .filter(|defaults| defaults.project == idx)
                .map_or_else(|| project.path.clone(), |defaults| defaults.path.clone());
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
                rich_plans: grove_core::rich_plan::PlanStore::default(),
                claude_resume: None,
                claude_subagents: HashMap::default(),
                edit_checkpoints: Vec::new(),
                pending_edit: None,
                provider,
                composer: defaults
                    .as_ref()
                    .map(|defaults| defaults.composer.clone())
                    .unwrap_or_default(),
                context: None,
                key,
                project: project.name,
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
            let tray = cx.new(|_| DestinationTray::new(projects, idx, checkout));
            let view = cx.new(|cx| CodexView::new(session.clone(), window, cx));
            view.update(cx, |view, cx| view.attach_dashboard_tray(tray.clone(), cx));
            let events = vec![
                cx.subscribe_in(&view, window, |this, source, event, window, cx| {
                    if matches!(event, CodexViewEvent::DashboardSubmit)
                        && this
                            .dashboard
                            .as_ref()
                            .is_some_and(|draft| &draft.view == source)
                    {
                        this.launch_dashboard(window, cx);
                    }
                }),
                cx.subscribe_in(&tray, window, |this, _, event, window, cx| {
                    match event {
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
            let fallback = projects.first().map(|p| p.0);
            let selected = if projects.iter().any(|p| p.0 == current) {
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
                        t.path
                            .clone_from(&cx.global::<SettingsState>().store.projects[idx].path);
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
        let Some((name, path, _)) = self.launch_target(idx, path, provider.agent(), cx) else {
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
        {
            self.content_error =
                Some("This provider is updating. Wait for the update to finish.".into());
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
            DashboardDefaults {
                project,
                path: saved.cwd.clone(),
                provider: saved.provider,
                composer: saved.composer.clone(),
                config_args: saved.config_args.clone(),
            }
        };
        let id = self.runtime.update(cx, |runtime, cx| {
            runtime.adopt_dashboard_session(session, cx)
        });
        view.update(cx, |view, cx| view.launch_dashboard(window, cx));
        self.codex_views.insert(id, view);
        self.dashboard = None;
        self.open_dashboard_with_defaults(Some(defaults), window, cx);
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
        let generation = tray.update(cx, |t, cx| {
            t.generation = t.generation.wrapping_add(1);
            t.loading = true;
            t.error = None;
            t.worktrees = vec![(root.clone(), "Main worktree".into())];
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
                let store = &cx.global::<SettingsState>().store;
                if t.project != idx
                    || t.generation != generation
                    || store.workspaces.active != workspace
                    || !store
                        .workspace_projects(workspace)
                        .any(|(i, p)| i == idx && p.path == expected_root)
                {
                    return;
                }
                t.loading = false;
                match result {
                    Ok(rows) => {
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
    pub(super) fn render_dashboard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(d) = self
            .dashboard
            .as_ref()
            .filter(|d| !d.tray.read(cx).projects.is_empty())
        else {
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
        // Use the workspace snapshot rather than the current navigation mode: collapsed
        // projects still contribute to the overview.
        let registry = self.runtime.read(cx).registry.read(cx);
        let activity = self.runtime.read(cx).activity.read(cx);
        let mut sessions: Vec<_> = registry
            .all()
            .iter()
            .filter(|meta| {
                self.snapshot
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
        sessions.sort_by_key(|session| std::cmp::Reverse(session.3));
        let working = sessions
            .iter()
            .filter(|(_, status, _, _)| matches!(*status, "Working" | "Starting"))
            .count();
        let needs_input = sessions
            .iter()
            .filter(|(_, status, _, _)| *status == "Needs you")
            .count();
        let projects = self.snapshot.projects.len();
        let summary = div()
            .id("dashboard-summary")
            .debug_selector(|| "dashboard-summary".into())
            .w_full()
            .max_w(rpx(CHAT_MAX_W))
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child("WORKSPACE OVERVIEW"),
            )
            .child(
                div()
                    .text_size(rpx(TEXT_DISPLAY))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Your workspace"),
            )
            .child(
                div()
                    .text_size(rpx(TEXT_BODY))
                    .text_color(c::FG_DIM())
                    .child("Start a session or pick up where you left off."),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(rpx(SPACE_3XL))
                    .pt(rpx(SPACE_LG))
                    .border_t_1()
                    .border_color(c::BORDER_SOFT())
                    .children(
                        [
                            (working, "Working"),
                            (needs_input, "Needs input"),
                            (projects, "Projects"),
                        ]
                        .into_iter()
                        .map(|(count, label)| {
                            div()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .child(
                                    div()
                                        .text_size(rpx(TEXT_TITLE))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .child(count.to_string()),
                                )
                                .child(
                                    div()
                                        .text_size(rpx(TEXT_SMALL))
                                        .text_color(c::FG_DIM())
                                        .child(label),
                                )
                        }),
                    ),
            );
        let mut recent = div()
            .id("dashboard-recent")
            .debug_selector(|| "dashboard-recent".into())
            .w_full()
            .max_w(rpx(DASHBOARD_RECENT_MAX_W))
            .border_1()
            .border_color(c::BORDER_SOFT())
            .rounded(rpx(RADIUS_PANEL))
            .p(rpx(SPACE_2XL))
            .flex()
            .flex_col()
            .child(
                div()
                    .pb(rpx(SPACE_SM))
                    .text_size(rpx(TEXT_TITLE))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Recent activity"),
            );
        if sessions.is_empty() {
            recent = recent.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child("Your sessions will appear here."),
            );
        } else {
            let mut quiet = 0;
            for (meta, status, title, since) in sessions {
                let expanded = self
                    .dashboard_timelines
                    .is_open(self.active_workspace, meta.id);
                if !matches!(status, "Working" | "Starting") && !expanded {
                    if quiet >= 3 {
                        continue;
                    }
                    quiet += 1;
                }

                let id = meta.id;
                let age = session_age_label(since, Instant::now());
                let label = format!(
                    "{title} · {} · {} · {status} · {age} · {}",
                    meta.project,
                    meta.agent.label(),
                    meta.wt_path
                );
                let structured = self
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .codex(id)
                    .map(|session| {
                        let state = &session.read(cx).state;
                        (
                            state.status,
                            state.saved.provider,
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
                if let Some((structured_status, provider, effort)) = structured {
                    let worktree = self
                        .snapshot
                        .projects
                        .iter()
                        .flat_map(|project| &project.worktrees)
                        .find(|worktree| worktree.path == meta.wt_path);
                    let worktree =
                        worktree.map_or_else(
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
                        project: meta.project.clone(),
                        worktree,
                        provider,
                        effort,
                        context: label,
                    };
                    recent = recent.child(self.render_dashboard_progress(
                        id,
                        &title,
                        structured_status,
                        &age,
                        &metadata,
                        cx,
                    ));
                    continue;
                }
                recent = recent.child(
                    self.row(
                        format!("dashboard-session-{}", id.raw()),
                        label,
                        false,
                        Action::Select(Selection::Session(id)),
                        cx,
                    )
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
            }
        }
        div()
            .id("dashboard-page")
            .debug_selector(|| "dashboard-page".into())
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .min_h_full()
                    .flex()
                    .flex_col()
                    .py(rpx(SPACE_3XL))
                    .children(self.content_error.as_ref().map(|error| {
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FORM_ERROR())
                            .p(rpx(SPACE_2XL))
                            .child(error.clone())
                    }))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .justify_center()
                            .px(rpx(SPACE_20))
                            .child(summary),
                    )
                    .child(div().w_full().pt(rpx(SPACE_3XL)).child(dashboard_composer))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .justify_center()
                            .px(rpx(SPACE_20))
                            .pt(rpx(SPACE_3XL))
                            .child(recent),
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
    saved.title = Some("Remove tooltip and background highlight".into());
    saved.items = vec![
        serde_json::json!({"type":"userMessage","text":"Make recent activity concise"}),
        serde_json::json!({"type":"agentMessage","text":"Located the recent activity view. Additional details stay in the conversation."}),
        serde_json::json!({"type":"commandExecution","command":"cat DESIGN.md"}),
        serde_json::json!({"type":"agentMessage","text":"Checked the shared status symbols."}),
        serde_json::json!({"type":"agentMessage","text":"Removed the tooltip and background highlight."}),
        serde_json::json!({"type":"agentMessage","text":"Checking native behavior across compact and expanded activity histories."}),
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
    let id = registry.update(cx, |registry, _| {
        let id = registry.insert_meta("Dashboard visual fixture".into(), cwd, provider.agent());
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

    // Exercise the real adoption/fresh-draft owner with mocked connected sessions.
    // Live provider normalization and native submit are verified separately.
    fn sequential_dashboard_adoption(
        cx: &mut gpui::TestAppContext,
        provider: grove_core::codex::Provider,
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
                        session.read(cx).state.saved.draft,
                        format!("Independent prompt {turn}")
                    );
                }
                assert_ne!(adopted_ids[0], adopted_ids[1]);
            });
        });
    }
    #[gpui::test]
    fn dashboard_sequential_codex_submits_keep_fresh_composer_and_destination(
        cx: &mut gpui::TestAppContext,
    ) {
        sequential_dashboard_adoption(cx, grove_core::codex::Provider::Codex);
    }
    #[gpui::test]
    fn dashboard_sequential_claude_submits_keep_fresh_composer_and_destination(
        cx: &mut gpui::TestAppContext,
    ) {
        sequential_dashboard_adoption(cx, grove_core::codex::Provider::Claude);
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
    fn activity_growth_keeps_overview_and_composer_anchored(cx: &mut gpui::TestAppContext) {
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
        assert!(composer.top() >= summary.bottom());
        assert!(composer.top() - summary.bottom() < gpui::px(100.));
        assert_eq!(tray.top(), composer.bottom());
        assert!(hints.top() >= tray.bottom());
        assert!(recent.top() >= hints.bottom());
        assert!(recent.top() - hints.bottom() < gpui::px(50.));
        assert!(composer.size.height > gpui::px(50.));
        assert!(composer.size.height < gpui::px(130.));

        cx.simulate_resize(gpui::size(gpui::px(560.), gpui::px(500.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let summary = cx.debug_bounds("dashboard-summary").unwrap();
        let composer = cx.debug_bounds("codex-composer").unwrap();
        assert!(composer.top() >= summary.bottom());
        assert!(composer.size.width > gpui::px(100.));
        assert!(composer.top() - summary.bottom() < gpui::px(100.));

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
        let bounds = cx.update(|_, cx| {
            let tray = tray.read(cx);
            [tray.bounds[0].get(), tray.bounds[1].get()]
        });
        assert_eq!(bounds[0].left(), strip.left() + gpui::px(SPACE_2XL + 1.));
        assert_eq!(bounds[1].right(), strip.right() - gpui::px(SPACE_2XL + 1.));
        for (idx, bounds) in bounds.iter().enumerate() {
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
            gpui::point(gpui::px(900.), gpui::px(250.)),
            None,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        cx.update(|_, cx| assert_eq!(tray.read(cx).hovered, [false; 2]));

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
