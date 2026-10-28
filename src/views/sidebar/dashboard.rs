//! Dashboard destination state; the composer remains the shared native editor.
use super::*;
use crate::entities::codex_session::CodexSession;
use crate::views::codex_view::{CodexView, CodexViewEvent};

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
    position: gpui::Point<gpui::Pixels>,
    bounds: [std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>; 2],
}
impl EventEmitter<DestinationEvent> for DestinationTray {}
impl DestinationTray {
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
        self.position = self.bounds[usize::from(!project)].get().bottom_left();
        self.popup_events = Some(cx.subscribe_in(
            &popup,
            window,
            move |this, _, _: &gpui::DismissEvent, window, cx| {
                this.popup = None;
                this.popup_events = None;
                if let Some(focus) = &focus {
                    focus.focus(window, cx);
                }
                cx.notify();
            },
        ));
        popup.focus_handle(cx).focus(window, cx);
        self.popup = Some(popup);
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
            .mt(rpx(-SPACE_MD))
            .pt(rpx(SPACE_LG))
            .pb(rpx(SPACE_LG))
            .px(rpx(SPACE_2XL))
            .rounded_b(rpx(RADIUS_PANEL))
            .border_1()
            .border_color(c::BORDER_SOFT())
            .bg(c::BG_RAIL())
            .flex()
            .flex_wrap()
            .gap(rpx(SPACE_3XL));
        for (idx, (is_project, label, glyph)) in
            [(true, project, "folder"), (false, worktree, "git-branch")]
                .into_iter()
                .enumerate()
        {
            let bounds = self.bounds[idx].clone();
            tray = tray.child(
                div()
                    .relative()
                    .max_w(rpx(MODAL_W_SM))
                    .child(
                        gpui::canvas(move |b, _, _| bounds.set(b), |_, (), _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(
                        super::super::components::transcript_action(
                            if is_project {
                                "dashboard-project"
                            } else {
                                "dashboard-worktree"
                            },
                            label.clone(),
                            false,
                            window,
                            cx,
                        )
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
                        .child(icon("chev-down", ICON_SM, c::FG_DIM()))
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
                    .anchor(gpui::Anchor::TopLeft)
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
            let Some((idx, _, _)) = projects.first() else {
                self.dashboard_open = true;
                cx.notify();
                return;
            };
            let idx = *idx;
            let project = cx.global::<SettingsState>().store.projects[idx].clone();
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
                provider: grove_core::codex::Provider::Codex,
                composer: grove_core::codex::ComposerSettings::default(),
                context: None,
                key,
                project: project.name,
                cwd: project.path.clone(),
                label: "Codex".into(),
                title: None,
                title_generation: None,
                roots: Vec::new(),
                thread_id: None,
                items: Vec::new(),
                work_periods: Vec::new(),
                draft: String::new(),
                draft_images: Vec::new(),
                draft_reference: None,
                config_args: Vec::new(),
                native_config_version: 1,
            };
            let session = cx.new(|cx| {
                CodexSession::draft(
                    saved,
                    Agent::Codex.launch_args_for_backend(
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
            let tray = cx.new(|_| DestinationTray::new(projects, idx, project.path));
            let view = cx.new(|cx| CodexView::new(session.clone(), window, cx));
            view.update(cx, |view, _| view.dashboard_tray = Some(tray.clone()));
            let events = vec![
                cx.subscribe_in(&view, window, |this, _, event, _, cx| {
                    if matches!(event, CodexViewEvent::DashboardSubmit) {
                        this.launch_dashboard(cx);
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
        if let Some(d) = &self.dashboard {
            d.view.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }
    fn launch_dashboard(&mut self, cx: &mut Context<Self>) {
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
        let id = self.runtime.update(cx, |runtime, cx| {
            runtime.adopt_dashboard_session(session, cx)
        });
        view.update(cx, CodexView::launch_dashboard);
        self.codex_views.insert(id, view);
        self.dashboard = None;
        self.dashboard_open = false;
        self.select_active_session(cx);
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
        div()
            .id("dashboard-page")
            .debug_selector(|| "dashboard-page".into())
            .size_full()
            .flex()
            .flex_col()
            .children(self.content_error.as_ref().map(|error| {
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FORM_ERROR())
                    .p(rpx(SPACE_2XL))
                    .child(error.clone())
            }))
            .child(div().flex_1().min_h_0().child(d.view.clone()))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            ("normal", 1100.),
            ("narrow", 560.),
            ("worktree-menu", 1100.),
        ] {
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
                });
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
            if state == "worktree-menu" {
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
                    tray.update(cx, |tray, cx| tray.open(false, window, cx));
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
