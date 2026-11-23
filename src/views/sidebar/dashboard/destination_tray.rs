//! Destination picker controls, popup lifecycle, and scope selection.

use super::{
    c, icon, motion, rpx, DestinationEvent, DestinationTray, PopupMenu, PopupMenuItem, ScopePicker,
    ScopePickerEvent, SettingsState, WorktreeDiscovery, NO_PROJECT,
};
use crate::views::tokens::{
    COMPOSER_CONTEXT_WIDTH_FRACTION, ICON_20, ICON_SM, MODAL_W_MD, MODAL_W_SM, RADIUS_PANEL,
    SPACE_2XL, SPACE_LG, SPACE_SM,
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled,
};
use gpui::AppContext as _;
use gpui::{div, Context, EventEmitter, Focusable, Window};

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
    pub(super) fn new(
        projects: Vec<(usize, String, String)>,
        project: usize,
        path: String,
    ) -> Self {
        Self {
            projects,
            scope: Vec::new(),
            scope_picker: None,
            scope_events: None,
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
    pub(super) fn open_scope(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(picker) = &self.scope_picker {
            picker.focus_handle(cx).focus(window, cx);
            return;
        }
        self.close_popup(cx);
        let selected = if self.scope.is_empty() {
            if self.project == NO_PROJECT {
                Vec::new()
            } else {
                vec![self.path.clone()]
            }
        } else {
            self.scope.iter().map(|root| root.path.clone()).collect()
        };
        let projects = self.projects.clone();
        let cwd = self.path.clone();
        let return_focus = window.focused(cx);
        let picker = cx.new(|cx| ScopePicker::new(projects, selected, cwd, window, cx));
        self.scope_events =
            Some(
                cx.subscribe_in(&picker, window, move |this, _, event, window, cx| {
                    if let ScopePickerEvent::Applied(targets, cwd) = event {
                        if let Some(primary) = targets.iter().find(|root| &root.path == cwd) {
                            this.project = primary.project;
                            this.path.clone_from(cwd);
                            this.scope.clone_from(targets);
                            this.generation = this.generation.wrapping_add(1);
                            this.loading = false;
                            this.error = None;
                            cx.emit(DestinationEvent::ScopeChanged);
                        }
                    }
                    this.scope_picker = None;
                    this.scope_events = None;
                    if let Some(focus) = &return_focus {
                        focus.focus(window, cx);
                    }
                    cx.notify();
                }),
            );
        picker.focus_handle(cx).focus(window, cx);
        self.scope_picker = Some(picker);
        cx.notify();
    }

    pub(super) fn close_popup(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn open(&mut self, project: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.popup_generation = self.popup_generation.wrapping_add(1);
        self.closing_popup = None;
        self.popup_motion.show(cx);
        let owner = cx.entity().downgrade();
        let projects = self.projects.clone();
        let worktrees = self.worktrees.clone();
        let selected_project = self.project;
        let scoped = !self.scope.is_empty();
        let selected_path = self.path.clone();
        let focus = window.focused(cx);
        let popup = PopupMenu::build(window, cx, move |mut menu, window, cx| {
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
                                .flex_1()
                                .min_w_0()
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
                        .checked(!scoped && selected_project == NO_PROJECT)
                        .on_click(move |_, _, cx| {
                            let _ = home_owner.update(cx, |_, cx| cx.emit(DestinationEvent::Home));
                        }),
                    )
                    .separator();
                for (idx, name, path) in projects {
                    let symbol = cx
                        .global::<SettingsState>()
                        .store
                        .project_appearance(&path)
                        .current();
                    let owner = owner.clone();
                    menu = menu.item(
                        PopupMenuItem::element(move |_, _| {
                            div()
                                .id(format!("destination-project-{idx}"))
                                .flex_1()
                                .min_w_0()
                                .aria_label(name.clone())
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .child(
                                    div()
                                        .size(rpx(ICON_20))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(crate::views::components::project_symbol(
                                            symbol, ICON_SM,
                                        )),
                                )
                                .child(name.clone())
                        })
                        .checked(!scoped && idx == selected_project)
                        .on_click(move |_, _, cx| {
                            let _ = owner.update(cx, |_, cx| {
                                cx.emit(DestinationEvent::Project(idx, path.clone()));
                            });
                        }),
                    );
                }
                let scope_owner = owner.clone();
                menu = menu.separator().item(
                    PopupMenuItem::element(|_, _| {
                        div()
                            .id("destination-across-worktrees")
                            .flex_1()
                            .min_w_0()
                            .debug_selector(|| "destination-across-worktrees".into())
                            .aria_label("Across worktrees")
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_SM))
                            .child(
                                div()
                                    .size(rpx(ICON_20))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon("across-worktrees", ICON_SM, c::FG_DIM())),
                            )
                            .child("Across worktrees…")
                    })
                    .checked(scoped)
                    .on_click(move |_, _, cx| {
                        let _ = scope_owner
                            .update(cx, |_, cx| cx.emit(DestinationEvent::AcrossWorktrees));
                    }),
                );
                let create_owner = owner.clone();
                menu = menu.separator().item(
                    PopupMenuItem::element(|_, _| {
                        div()
                            .id("destination-new-project")
                            .aria_label("Add project")
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
                            .child("Add project")
                    })
                    .on_click(move |_, _, cx| {
                        let _ =
                            create_owner.update(cx, |_, cx| cx.emit(DestinationEvent::NewProject));
                    }),
                );
                let manage_owner = owner.clone();
                menu = menu.item(
                    PopupMenuItem::element(|_, _| {
                        div()
                            .id("destination-manage-projects")
                            .aria_label("Manage projects")
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_SM))
                            .child(
                                div()
                                    .size(rpx(ICON_20))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(icon("folder", ICON_SM, c::FG_DIM())),
                            )
                            .child("Manage projects")
                    })
                    .on_click(move |_, _, cx| {
                        let _ = manage_owner
                            .update(cx, |_, cx| cx.emit(DestinationEvent::ManageProjects));
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
                let create_owner = owner.clone();
                menu = menu.separator().item(
                    PopupMenuItem::new("New worktree")
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/plus.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .on_click(move |_, _, cx| {
                            let _ = create_owner
                                .update(cx, |_, cx| cx.emit(DestinationEvent::NewWorktree));
                        }),
                );
                menu = menu.item(
                    PopupMenuItem::element(|_, _| {
                        div()
                            .debug_selector(|| "dashboard-manage-worktrees".into())
                            .child("Manage worktrees")
                    })
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/git-branch.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .on_click(move |_, _, cx| {
                        let _ =
                            owner.update(cx, |_, cx| cx.emit(DestinationEvent::ManageWorktrees));
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
        let scoped = !self.scope.is_empty();
        let project = if scoped {
            format!(
                "{} worktree{}",
                self.scope.len(),
                if self.scope.len() == 1 { "" } else { "s" }
            )
        } else {
            project
        };
        let worktree = if scoped {
            self.scope
                .iter()
                .find(|target| target.path == self.path)
                .map_or_else(
                    || "Review worktrees".into(),
                    |target| {
                        format!(
                            "Starts in {} / {}",
                            target.project_name,
                            if target.branch.is_empty() {
                                crate::paths::path_basename(&target.path)
                            } else {
                                target.branch.clone()
                            }
                        )
                    },
                )
        } else {
            worktree
        };
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
        for (idx, (is_project, label, glyph)) in [
            (
                true,
                project,
                if scoped { "across-worktrees" } else { "folder" },
            ),
            (false, worktree, "git-branch"),
        ]
        .into_iter()
        .enumerate()
        {
            if !is_project && (self.project == NO_PROJECT || self.scope.len() > 1) {
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
                    .debug_selector(move || {
                        if is_project {
                            "dashboard-project-selector".into()
                        } else {
                            "dashboard-worktree-selector".into()
                        }
                    })
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
                        crate::views::components::dense_disclosure_action(
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
                        .when(
                            !is_project || self.project == NO_PROJECT || scoped,
                            |button| {
                                button.icon(
                                    gpui_component::Icon::default()
                                        .path(format!("icons/{glyph}.svg")),
                                )
                            },
                        )
                        .when(
                            is_project && self.project != NO_PROJECT && !scoped,
                            |button| {
                                let path = self
                                    .projects
                                    .iter()
                                    .find(|p| p.0 == self.project)
                                    .map_or("", |p| p.2.as_str());
                                button.child(crate::views::components::project_symbol_for_path(
                                    path, ICON_SM, cx,
                                ))
                            },
                        )
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
                                if !is_project && !this.scope.is_empty() {
                                    this.open_scope(window, cx);
                                } else {
                                    this.open(is_project, window, cx);
                                }
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
        if let Some(picker) = &self.scope_picker {
            tray = tray.child(gpui::deferred(picker.clone()));
        }
        tray
    }
}
