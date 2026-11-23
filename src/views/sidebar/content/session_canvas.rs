//! Active session view, canvas header, and session event routing.

use super::{
    agent_name, c, canvas_state, form_action, icon, motion, rpx, session_display_title,
    session_signature, sidebar_worktree_name, Action, ActivityState, CanvasState, Selection,
    SettingsState, Sidebar, TerminalView, ViewMode, HEADER_PREFIX_FRACTION,
    HEADER_STATUS_LABEL_MIN_W, SESSION_HEADER_H, SESSION_ICON_SLOT,
};
use crate::views::tokens::{
    APPBAR_H, CHROME_CONTROL_H, ICON_LG, ICON_SM, MODAL_W_XL, MOTION_FAST_MS, RADIUS_CONTROL,
    RADIUS_PANEL, SPACE_3XL, SPACE_LG, SPACE_MD, SPACE_SM, TEXT_BODY, TEXT_MICRO, TEXT_SMALL,
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled,
};
use gpui::AppContext as _;
use gpui::{div, AnyElement, Context, Focusable, Window};

impl Sidebar {
    /// Both a freshly constructed session view and a Home composer adopted as a
    /// session must route the same events. Install once at either ownership handoff.
    pub(in crate::views::sidebar) fn observe_session_view_events(
        &mut self,
        id: crate::entities::session_registry::SessionId,
        view: &gpui::Entity<crate::views::codex_view::CodexView>,
        cx: &mut Context<Self>,
    ) {
        use crate::views::codex_view::CodexViewEvent;
        self.observers.push(cx.subscribe(view, move |this, _, event, cx| {
            match event {
                CodexViewEvent::OpenConnections(provider,cwd,args,focus) => {
                    if let Some(settings)=&this.settings_panel {settings.update(cx,|panel,cx|panel.request_connections(*provider,cwd.clone(),args.clone(),focus.clone(),cx));}
                    cx.notify();
                }
                CodexViewEvent::ForkRequested(boundary, restore_user) => {
                    let result = this.runtime.update(cx, |runtime, cx| {
                        runtime.fork_session(id, *boundary, *restore_user, cx)
                    });
                    match result {
                        Ok(fork) => this.select(Selection::Session(fork), cx),
                        Err(error) => this.runtime.read(cx).toast.clone().update(cx, |toast, cx| toast.set_error(error, cx)),
                    }
                }
                CodexViewEvent::ReviewChanges(preference, source) if matches!(this.selection, Some(Selection::Session(selected) | Selection::Home(selected)) if selected == id) => {
                    this.invoke_review_skill(preference.clone(), source.clone(), cx);
                }
                CodexViewEvent::DescribeChanges(preference, source) if matches!(this.selection, Some(Selection::Session(selected) | Selection::Home(selected)) if selected == id) => {
                    this.invoke_description_skill(preference.clone(), source.clone(), cx);
                }
                CodexViewEvent::FileOpened if matches!(this.selection, Some(Selection::Session(selected) | Selection::Home(selected)) if selected == id) => {
                    this.files_active = true;
                    this.files_owner = this.selection.clone();
                    this.checkout_panel.hidden = false;
                    this.checkout_panel.side_chat_owner = None;
                    this.hide_review();
                    this.dismiss_description();
                    cx.notify();
                }
                CodexViewEvent::FileClosed if matches!(this.selection, Some(Selection::Session(selected) | Selection::Home(selected)) if selected == id) => {
                    this.files_active = false;
                    this.checkout_panel.hidden = false;
                    this.checkout_panel.side_chat_owner = None;
                    cx.notify();
                }
                _ => {}
            }
        }));
    }

    pub(in crate::views::sidebar) fn terminal_content(
        &mut self,
        id: crate::entities::session_registry::SessionId,
        home: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let native = self.runtime.read(cx).registry.read(cx).codex(id).cloned();
        let native_view = if let Some(session) = native.as_ref() {
            session.update(
                cx,
                crate::entities::codex_session::CodexSession::ensure_connected,
            );
            let newly_created = !self.codex_views.contains_key(&id);
            let view = self
                .codex_views
                .entry(id)
                .or_insert_with(|| {
                    cx.new(|cx| {
                        crate::views::codex_view::CodexView::new(session.clone(), window, cx)
                    })
                })
                .clone();
            if newly_created {
                // The tools section precedes the canvas in render order. Publish the
                // newly constructed plan entities on the next parent render.
                cx.notify();
                self.observe_session_view_events(id, &view, cx);
            }
            let description_scope = self.description_scope_for_session(id);
            let review_scope = self.review_scope_for_session(id);
            view.update(cx, |view, cx| {
                if view.review_scope != review_scope {
                    view.review_scope = review_scope;
                    cx.notify();
                }
            });
            view.update(cx, |view, cx| {
                if view.description_scope != description_scope {
                    view.description_scope = description_scope;
                    cx.notify();
                }
                view.set_script_runtime(self.runtime.clone(), cx);
                if view.agents_in_checkout != self.checkout_panel.agents_visible {
                    view.agents_in_checkout = self.checkout_panel.agents_visible;
                    cx.notify();
                }
            });
            self.canvas_focus_observers.entry(id).or_insert_with(|| {
                cx.on_focus_in(&view.focus_handle(cx), window, move |this, _, cx| {
                    this.select(Selection::Session(id), cx);
                })
            });
            let grid_focus = self.pending_grid_workspace_focus == Some(id)
                && self.grid_workspace_focus_allowed(window, cx);
            if (grid_focus
                || ((newly_created || self.pending_canvas_focus == Some(id))
                    && self.canvas_focus_allowed(window, cx)))
                && matches!(self.selection, Some(Selection::Session(selected)) if selected == id)
            {
                view.focus_handle(cx).focus(window, cx);
                self.pending_canvas_focus = None;
                self.pending_grid_workspace_focus = None;
            }
            Some(view)
        } else {
            None
        };
        let registry = self.runtime.read(cx).registry.read(cx);
        let (meta, session) = if home {
            let index = registry
                .home_terminals()
                .iter()
                .position(|meta| meta.id == id)?;
            (
                registry.home_terminals()[index].clone(),
                registry.home_terminal(index).cloned(),
            )
        } else {
            (registry.meta(id)?.clone(), registry.session(id).cloned())
        };
        let workspaces = &cx.global::<SettingsState>().store.workspaces;
        let workspace = workspaces.name(workspaces.active).to_owned();
        let worktree = self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| &project.worktrees)
            .find(|worktree| worktree.path == meta.wt_path);
        let branch = worktree
            .map(|worktree| worktree.branch.clone())
            .unwrap_or_default();
        let location =
            worktree.map_or_else(|| meta.wt_path.clone(), |worktree| worktree.name.clone());
        let launch_location = worktree.map_or_else(
            || location.clone(),
            |worktree| sidebar_worktree_name(&worktree.name, worktree.is_main).to_owned(),
        );
        let context = if home {
            format!("{workspace} / Standalone terminal / ~")
        } else {
            format!("{workspace} / {} / {location}", meta.project)
        };
        let error = session
            .as_ref()
            .and_then(|session| session.read(cx).spawn_error().map(str::to_owned));
        let pending = session
            .as_ref()
            .is_some_and(|session| session.read(cx).is_pending_attach());
        let exited = session
            .as_ref()
            .is_some_and(|session| session.read(cx).has_exited());
        let activity = if home {
            ActivityState::Idle
        } else {
            self.runtime.read(cx).activity.read(cx).state_of(id)
        };
        let state = if let Some(native) = &native {
            use grove_core::codex::Status;
            match native.read(cx).state.status {
                Status::Starting => CanvasState::Starting,
                Status::Working => CanvasState::Working,
                Status::Waiting => CanvasState::NeedsYou,
                Status::Done => CanvasState::Done,
                Status::Failed | Status::Disconnected => CanvasState::Failed,
                Status::Idle | Status::Interrupted => CanvasState::Idle,
            }
        } else {
            canvas_state(
                meta.agent,
                activity,
                error.is_some(),
                pending,
                session.is_none(),
                exited,
            )
        };
        let task = session_display_title(
            &meta,
            session
                .as_ref()
                .and_then(|session| session.read(cx).title()),
        );
        let agent = agent_name(meta.agent);
        let grid = self.mode == ViewMode::Grid;
        let status_label = if home || (native.is_none() && state == CanvasState::Starting) {
            state.label()
        } else {
            self.status(&meta, cx).0
        };
        let accessible_label = format!("{agent} · {task} · {context} · {status_label}");
        let mut view = None;
        if let Some(session) = session {
            if !self.canvas_observers.contains_key(&id) {
                let signature = session_signature(&session, cx);
                self.canvas_signatures.insert(id, signature);
                self.canvas_observers.insert(
                    id,
                    cx.observe(&session, move |this, session, cx| {
                        let signature = session_signature(&session, cx);
                        if this.canvas_signatures.get(&id) != Some(&signature) {
                            this.canvas_signatures.insert(id, signature);
                            cx.notify();
                        }
                    }),
                );
            }
            let views = if home {
                &mut self.home_terminal_views
            } else {
                &mut self.terminal_views
            };
            let newly_created = !views.contains_key(&id);
            let terminal = views
                .entry(id)
                .or_insert_with(|| cx.new(|cx| TerminalView::new(session, cx)))
                .clone();
            #[cfg(target_os = "macos")]
            if newly_created {
                cx.observe(&terminal, |_, _, cx| cx.notify()).detach();
            }
            self.canvas_focus_observers.entry(id).or_insert_with(|| {
                cx.on_focus_in(&terminal.focus_handle(cx), window, move |this, _, cx| {
                    this.select(
                        if home {
                            Selection::Home(id)
                        } else {
                            Selection::Session(id)
                        },
                        cx,
                    );
                })
            });
            let grid_workspace_focus = self.pending_grid_workspace_focus == Some(id)
                && self.grid_workspace_focus_allowed(window, cx);
            if (grid_workspace_focus
                || ((newly_created || self.pending_canvas_focus == Some(id))
                    && self.canvas_focus_allowed(window, cx)))
                && matches!(self.selection,Some(Selection::Session(selected) | Selection::Home(selected)) if selected == id)
            {
                terminal.focus_handle(cx).focus(window, cx);
                self.pending_canvas_focus = None;
                self.pending_grid_workspace_focus = None;
            }
            view = Some(terminal);
        }
        let bounds = self.canvas_bounds.entry(id).or_default().clone();
        let close_name = format!("Close {agent} session {task} in {context}");
        let close = self.canvas_close_button(id, home, close_name, cx).when(
            bounds.get().size.width > rpx(MODAL_W_XL).to_pixels(window.rem_size()),
            |button| {
                button
                    .opacity(0.0)
                    .group_hover("session-header", |button| button.opacity(1.0))
                    .focus(|button| button.opacity(1.0))
            },
        );
        let breadcrumb_project = (!home)
            .then(|| {
                self.snapshot
                    .projects
                    .iter()
                    .find(|project| project.sessions.contains(&id))
                    .and_then(|project| {
                        cx.global::<SettingsState>()
                            .store
                            .projects
                            .get(project.idx)
                            .map(|stored| (project.idx, stored.path.clone()))
                    })
            })
            .flatten();
        let active = matches!(
            self.selection,
            Some(Selection::Session(selected) | Selection::Home(selected)) if selected == id
        );
        let compact_status =
            bounds.get().size.width <= rpx(HEADER_STATUS_LABEL_MIN_W).to_pixels(window.rem_size());
        let header_owner = cx.entity().entity_id();
        let mut header = div()
            .id(gpui::SharedString::from(format!(
                "terminal-header-{}",
                id.raw()
            )))
            .debug_selector(move || format!("terminal-header-{}", id.raw()))
            .aria_label(accessible_label.clone())
            .group("session-header")
            .relative()
            .child(
                gpui::canvas(
                    move |rect, window, cx| {
                        let threshold = rpx(MODAL_W_XL).to_pixels(window.rem_size());
                        let was_wide = bounds.get().size.width > threshold;
                        let compact_threshold =
                            rpx(HEADER_STATUS_LABEL_MIN_W).to_pixels(window.rem_size());
                        let was_compact = bounds.get().size.width <= compact_threshold;
                        bounds.set(rect);
                        if was_wide != (rect.size.width > threshold)
                            || was_compact != (rect.size.width <= compact_threshold)
                        {
                            cx.notify(header_owner);
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_MD))
            .px(rpx(SPACE_3XL))
            .bg(if grid && active {
                c::BG_HOVER()
            } else {
                c::BG()
            })
            .when(!grid, |header| header.rounded_tl(rpx(RADIUS_PANEL)))
            .when(grid && !active, |header| {
                header.border_b_1().border_color(c::BORDER())
            })
            .when(grid, |header| {
                header.cursor_grab().on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                        this.begin_grid_session_drag((id, home), event.position, window, cx);
                    }),
                )
            });
        let agent_icon = div()
            .debug_selector(move || format!("session-header-provider-{}", id.raw()))
            .w(rpx(SESSION_ICON_SLOT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .child(icon(meta.agent.icon_name(), ICON_LG, c::FG()));
        let status_kind = crate::views::status::session_kind(status_label);
        let status = div()
            .id(("header-status", id.raw()))
            .debug_selector(move || format!("session-header-status-{}", id.raw()))
            .role(gpui::Role::Status)
            .aria_label(status_label)
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_SM))
            .text_size(rpx(TEXT_MICRO))
            .text_color(status_kind.color())
            .whitespace_nowrap()
            .child(crate::views::status::mark(
                format!("canvas-status-{}", id.raw()),
                status_kind,
                ICON_SM,
                cx,
            ))
            .when(!compact_status, |status| status.child(status_label));
        let status = motion::fast(
            status,
            format!("canvas-session-status-{}-{}", id.raw(), status_label),
            cx,
        );
        let task_progress = native.as_ref().filter(|_| grid).and_then(|native| {
            self.header_task_progress(id, native.read(cx).state.status, window, cx)
        });
        let header_status = div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_MD))
            .child(status)
            .when_some(task_progress, gpui::ParentElement::child)
            .into_any_element();
        header = header
            .h(rpx(if grid { SESSION_HEADER_H } else { APPBAR_H }))
            .child(
                div()
                    .id(gpui::SharedString::from(format!(
                        "terminal-header-left-{}",
                        id.raw()
                    )))
                    .debug_selector(move || format!("terminal-header-left-{}", id.raw()))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_MD))
                            .text_size(rpx(TEXT_BODY))
                            .text_color(c::FG_MUTE())
                            .child(
                                div()
                                    .min_w_0()
                                    .max_w(gpui::relative(HEADER_PREFIX_FRACTION))
                                    .flex()
                                    .items_center()
                                    .gap(rpx(SPACE_MD))
                                    .text_size(rpx(TEXT_SMALL))
                                    .child(self.session_breadcrumb(
                                        id,
                                        false,
                                        if home {
                                            workspace.clone()
                                        } else {
                                            meta.project.clone()
                                        },
                                        breadcrumb_project.clone(),
                                        meta.wt_path.clone(),
                                        cx,
                                    ))
                                    .child(
                                        div()
                                            .debug_selector(move || {
                                                format!(
                                                    "session-header-project-separator-{}",
                                                    id.raw()
                                                )
                                            })
                                            .flex_shrink_0()
                                            .child("/"),
                                    )
                                    .child(self.session_breadcrumb(
                                        id,
                                        true,
                                        if home {
                                            "~".to_owned()
                                        } else {
                                            location.clone()
                                        },
                                        breadcrumb_project,
                                        meta.wt_path.clone(),
                                        cx,
                                    ))
                                    .child(
                                        div()
                                            .debug_selector(move || {
                                                format!(
                                                    "session-header-title-separator-{}",
                                                    id.raw()
                                                )
                                            })
                                            .flex_shrink_0()
                                            .child("/"),
                                    ),
                            )
                            .child(agent_icon)
                            .child(self.session_header_title(
                                id,
                                &task,
                                native.is_some(),
                                header_status,
                                window,
                                cx,
                            )),
                    ),
            )
            .child(
                div()
                    .id(gpui::SharedString::from(format!(
                        "terminal-header-right-{}",
                        id.raw()
                    )))
                    .debug_selector(move || format!("terminal-header-right-{}", id.raw()))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_MD))
                    .when(
                        !grid
                            && self.checkout_panel.toggle_available
                            && (self.checkout_panel.hidden
                                || Self::logical_window_width(window)
                                    < super::super::checkout_panel::WIDE_VIEWPORT),
                        |actions| actions.w(rpx(CHROME_CONTROL_H * 3. + SPACE_MD * 2. + SPACE_3XL)),
                    )
                    .when(grid, |actions| actions.child(close)),
            );
        let header = if grid {
            let selected = matches!(
                self.selection,
                Some(Selection::Session(selected) | Selection::Home(selected)) if selected == id
            );
            motion::background(
                header,
                format!("grid-terminal-header-{}-{selected}", id.raw()),
                if selected {
                    c::BG_STRIP()
                } else {
                    c::BG_HOVER()
                },
                if selected {
                    c::BG_HOVER()
                } else {
                    c::BG_STRIP()
                },
                std::time::Duration::from_millis(MOTION_FAST_MS),
                cx,
            )
        } else {
            motion::fast(header, format!("terminal-header-enter-{}", id.raw()), cx)
        };
        let mut pane = div()
            .id(gpui::SharedString::from(format!(
                "terminal-pane-{}",
                id.raw()
            )))
            .debug_selector(move || format!("terminal-pane-{}", id.raw()))
            .role(gpui::Role::Group)
            .aria_label(accessible_label)
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.select(
                        if home {
                            Selection::Home(id)
                        } else {
                            Selection::Session(id)
                        },
                        cx,
                    );
                }),
            )
            .when(!self.is_zen(), |pane| pane.child(header))
            .when_some(
                self.header_task_disclosure(id, window, cx),
                gpui::ParentElement::child,
            )
            .when_some(
                self.checkout_panel.narrow_content.take(),
                gpui::ParentElement::child,
            );
        let terminal_review = native
            .is_none()
            .then(|| self.runtime.read(cx).activity.read(cx).terminal_review(id))
            .flatten();
        if let Some(completion) =
            terminal_review.filter(|_| matches!(status_label, "Review" | "Review · unread"))
        {
            pane = pane.child(
                div()
                    .id(("terminal-review", id.raw()))
                    .role(gpui::Role::Status)
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .p(rpx(SPACE_LG))
                    .border_b_1()
                    .border_color(c::BORDER())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_SM))
                            .text_color(c::STATUS_SUCCESS())
                            .child(crate::views::status::mark(
                                format!("terminal-review-mark-{}", id.raw()),
                                crate::views::status::StatusKind::Success,
                                ICON_SM,
                                cx,
                            ))
                            .child(status_label),
                    )
                    .child(
                        self.control(
                            ("terminal-mark-reviewed", id.raw()),
                            "Mark reviewed",
                            Action::MarkTerminalReviewed(id, completion),
                            cx,
                        )
                        .w_auto()
                        .px(rpx(SPACE_LG))
                        .gap(rpx(SPACE_SM))
                        .child(icon("check", ICON_SM, c::FG_DIM()))
                        .child("Mark reviewed"),
                    ),
            );
        }
        if let Some(message) = state.message(error.as_deref()).filter(|_| native.is_none()) {
            let mut banner = div()
                .id(gpui::SharedString::from(format!(
                    "session-state-{}",
                    id.raw()
                )))
                .role(if matches!(state, CanvasState::Failed) {
                    gpui::Role::Alert
                } else {
                    gpui::Role::Status
                })
                .flex_shrink_0()
                .min_w_0()
                .p(rpx(SPACE_LG))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(rpx(SPACE_LG))
                .bg(c::BG_STRIP())
                .border_b_1()
                .border_color(c::BORDER())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(state.color())
                        .child(motion::fast(
                            div().child(message),
                            format!("canvas-session-message-{}-{}", id.raw(), state.label()),
                            cx,
                        )),
                );
            if state == CanvasState::NeedsYou {
                if let Some(terminal) = view.clone() {
                    banner = banner.child(
                        form_action(
                            "focus-session-terminal",
                            "Focus terminal",
                            false,
                            window,
                            cx,
                        )
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/terminal.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.select(
                                    if home {
                                        Selection::Home(id)
                                    } else {
                                        Selection::Session(id)
                                    },
                                    cx,
                                );
                                terminal.focus_handle(cx).focus(window, cx);
                            },
                        )),
                    );
                }
            } else if matches!(state, CanvasState::Failed | CanvasState::Exited) {
                if !home {
                    banner = banner.child(
                        self.control(
                            ("retry-canvas", id.raw()),
                            format!("Retry {agent} session"),
                            Action::Retry(id),
                            cx,
                        )
                        .w_auto()
                        .px(rpx(SPACE_LG))
                        .child(icon("restart", ICON_SM, c::FG_DIM()))
                        .child("Retry"),
                    );
                }
                banner = banner.child(
                    self.canvas_close_button(id, home, "Remove session".into(), cx)
                        .w_auto()
                        .px(rpx(SPACE_LG))
                        .child("Remove"),
                );
            }
            pane = pane.child(banner);
        }
        if let Some(view) = native_view.as_ref() {
            view.update(cx, |view, cx| {
                view.set_checkout_context(launch_location, branch.clone(), cx);
            });
        }
        if native_view.is_none() && view.is_some() {
            self.rendered_terminal_ids.insert(id);
        }
        let body = native_view
            .map(gpui::IntoElement::into_any_element)
            .or_else(|| view.map(gpui::IntoElement::into_any_element));
        pane = if let Some(terminal) = body {
            pane.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .p(rpx(SPACE_3XL))
                    .overflow_hidden()
                    .child(terminal),
            )
        } else {
            pane.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(rpx(TEXT_BODY))
                    .text_color(c::FG_DIM())
                    .child("Preparing terminal…"),
            )
        };
        Some(
            pane.when_some(self.canvas_confirmation(id, window, cx), |pane, popup| {
                pane.child(popup)
            })
            .into_any_element(),
        )
    }
    fn session_breadcrumb(
        &mut self,
        id: crate::entities::session_registry::SessionId,
        worktree: bool,
        label: String,
        project: Option<(usize, String)>,
        checkout: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selector = format!(
            "session-header-{}-{}",
            if worktree { "worktree" } else { "project" },
            id.raw()
        );
        let Some((project_idx, project_path)) = project else {
            return div()
                .debug_selector(move || selector.clone())
                .min_w_0()
                .truncate()
                .child(label)
                .into_any_element();
        };
        let focus = self
            .canvas_breadcrumb_focus
            .entry((id, worktree))
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let click_path = project_path.clone();
        let click_checkout = checkout.clone();
        div()
            .id(gpui::SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .track_focus(&focus)
            .role(gpui::Role::Button)
            .tab_index(0)
            .aria_label(label.clone())
            .cursor_pointer()
            .min_w_0()
            .truncate()
            .hover(|style| style.text_color(c::FG()))
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                focus.focus(window, cx);
                cx.stop_propagation();
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                let target = if worktree {
                    &click_checkout
                } else {
                    &click_path
                };
                this.open_dashboard_for_checkout(project_idx, &click_path, target, window, cx);
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                        let target = if worktree { &checkout } else { &project_path };
                        this.open_dashboard_for_checkout(
                            project_idx,
                            &project_path,
                            target,
                            window,
                            cx,
                        );
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }),
            )
            .flex()
            .items_center()
            .gap(rpx(SPACE_SM))
            .when(worktree, |link| {
                link.child(crate::icons::icon("git-branch", ICON_SM, c::FG_DIM()))
            })
            .when(!worktree, |link| {
                let store = &cx.global::<SettingsState>().store;
                link.when_some(store.projects.get(project_idx), |link, project| {
                    link.child(crate::views::components::project_symbol_for_path(
                        &project.path,
                        ICON_SM,
                        cx,
                    ))
                })
            })
            .child(label)
            .into_any_element()
    }
    fn canvas_close_button(
        &mut self,
        id: crate::entities::session_registry::SessionId,
        home: bool,
        label: String,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let removal = label == "Remove session";
        let focus = self
            .canvas_close_focus
            .entry((id, removal))
            .or_insert_with(|| cx.focus_handle())
            .clone();
        div()
            .id(gpui::SharedString::from(format!(
                "close-canvas-{}-{removal}",
                id.raw()
            )))
            .debug_selector(move || {
                format!(
                    "canvas-{}-{}",
                    if removal { "remove" } else { "close" },
                    id.raw()
                )
            })
            .track_focus(&focus)
            .role(gpui::Role::Button)
            .cursor_pointer()
            .aria_label(label.clone())
            .tab_index(0)
            .size(rpx(CHROME_CONTROL_H))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(rpx(RADIUS_CONTROL))
            .hover(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .tooltip(move |window, cx| {
                crate::views::components::tooltip(label.clone(), window).build(window, cx)
            })
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                focus.focus(window, cx);
                cx.stop_propagation();
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.request_canvas_close(id, home, window, cx);
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                        cx.stop_propagation();
                        this.request_canvas_close(id, home, window, cx);
                    }
                }),
            )
            .child(icon("close", ICON_SM, c::FG_DIM()))
    }
}
