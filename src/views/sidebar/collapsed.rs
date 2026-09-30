//! Compact navigation uses the same snapshots, actions and ordering as the full rail.
use super::*;

impl Sidebar {
    pub(super) fn collapse_control(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let collapsed = self.is_collapsed(cx);
        self.control(
            "sidebar-collapse-toggle",
            if collapsed {
                "Expand sidebar"
            } else {
                "Collapse sidebar"
            },
            Action::ToggleSidebar,
            cx,
        )
        .debug_selector(|| "sidebar-collapse-toggle".into())
        .track_focus(&self.collapse_focus)
        .child(icon(
            if collapsed {
                "sidebar-expand"
            } else {
                "sidebar-collapse"
            },
            ICON_MD,
            c::FG_DIM(),
        ))
    }

    fn compact_item(
        &self,
        id: String,
        label: String,
        glyph: &str,
        selected: bool,
        action: Action,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let hierarchy_inset = match action {
            Action::Select(Selection::Worktree(..)) => 4.0,
            Action::Select(Selection::Session(_)) if self.mode == ViewMode::Project => 8.0,
            _ => 0.0,
        };
        let agent_identity = matches!(action, Action::Select(Selection::Session(_)));
        let selection_bar =
            selected && matches!(action, Action::Select(_) | Action::ToggleProject(_));
        self.control(SharedString::from(id.clone()), label, action, cx)
            .debug_selector(move || id.clone())
            .relative()
            .w(rpx(36.0))
            .h(rpx(32.0))
            .rounded(rpx(RADIUS_CHROME))
            .aria_selected(selected)
            .when(selected, |item| item.bg(c::alpha(c::FG(), 0.14)))
            .when(selection_bar, |item| {
                item.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(rpx(8.0))
                        .w(rpx(2.0))
                        .h(rpx(16.0))
                        .rounded_full()
                        .bg(c::FG()),
                )
            })
            .when(hierarchy_inset > 0.0, |item| {
                item.child(
                    div()
                        .absolute()
                        .left(rpx(hierarchy_inset))
                        .top(rpx(9.0))
                        .w(rpx(4.0))
                        .h(rpx(8.0))
                        .border_l_1()
                        .border_b_1()
                        .border_color(c::BORDER_SOFT()),
                )
            })
            .child(
                icon(
                    glyph,
                    16.0,
                    if agent_identity {
                        c::MAGENTA()
                    } else if selected {
                        c::FG()
                    } else {
                        c::FG_DIM()
                    },
                )
                .ml(rpx(hierarchy_inset * 2.0)),
            )
    }

    pub(super) fn collapsed_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let target = if self.mode == ViewMode::Grid {
            self.last_mode
        } else if self.mode == ViewMode::Project {
            ViewMode::List
        } else {
            ViewMode::Project
        };
        let current = if self.is_grid() {
            self.last_mode
        } else {
            self.mode
        };
        let workspace = &cx.global::<SettingsState>().store.workspaces;
        let workspace_name = workspace.name(workspace.active).to_string();
        div()
            .id("sidebar-compact-controls")
            .debug_selector(|| "sidebar-compact-controls".into())
            .flex()
            .flex_col()
            .items_center()
            .flex_shrink_0()
            .gap(rpx(2.0))
            .py(rpx(SPACE_SM))
            .mb(rpx(SPACE_SM))
            .border_b_1()
            .border_color(c::BORDER_SOFT())
            .when(self.workspace_selector.is_some(), |controls| {
                controls.child(self.compact_item(
                    "sidebar-workspaces".into(),
                    format!("Switch workspace · {workspace_name} · Expands sidebar"),
                    "workspaces",
                    false,
                    Action::OpenWorkspaces,
                    cx,
                ))
            })
            .child(
                self.compact_item(
                    "sidebar-view".into(),
                    if self.is_grid() {
                        if target == ViewMode::List {
                            "Return to sessions view"
                        } else {
                            "Return to project view"
                        }
                    } else if current == ViewMode::Project {
                        "Projects view · Switch to sessions view"
                    } else {
                        "Sessions view · Switch to project view"
                    }
                    .into(),
                    if current == ViewMode::Project {
                        "rail-tree"
                    } else {
                        "list"
                    },
                    !self.is_grid(),
                    Action::Mode(target),
                    cx,
                ),
            )
            .child(
                self.compact_item(
                    "sidebar-grid".into(),
                    if self.is_grid() {
                        "Grid view · Return to sidebar view"
                    } else {
                        "Open grid view"
                    }
                    .into(),
                    "grid",
                    self.is_grid(),
                    Action::Mode(if self.is_grid() {
                        self.last_mode
                    } else {
                        ViewMode::Grid
                    }),
                    cx,
                ),
            )
            .into_any_element()
    }

    pub(super) fn collapsed_utilities(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("sidebar-compact-utilities")
            .debug_selector(|| "sidebar-compact-utilities".into())
            .flex()
            .flex_col()
            .items_center()
            .flex_shrink_0()
            .gap(rpx(2.0))
            .pt(rpx(SPACE_SM))
            .mt(rpx(SPACE_SM))
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .child(self.compact_item(
                "sidebar-settings".into(),
                "Open settings".into(),
                "cog",
                false,
                Action::OpenSettings,
                cx,
            ))
            .child(self.compact_item(
                "projects-archive".into(),
                "Archived projects".into(),
                "archive",
                false,
                Action::ArchivedProjects,
                cx,
            ))
            .child(self.compact_item(
                "projects-add".into(),
                "Add project".into(),
                "add-project",
                false,
                Action::AddProject,
                cx,
            ))
            .into_any_element()
    }

    fn compact_session(
        &self,
        meta: &SessionMeta,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = meta.id;
        let registry = self.runtime.read(cx).registry.read(cx);
        let session = registry.session(id).map(|session| session.read(cx));
        let title = session_display_title(
            meta,
            session.and_then(crate::entities::terminal_session::TerminalSession::title),
        );
        let (status, color) = if session.is_some_and(|session| session.spawn_error().is_some()) {
            ("Failed", c::RED())
        } else if session
            .is_some_and(crate::entities::terminal_session::TerminalSession::is_pending_attach)
        {
            ("Starting", c::FG_DIM())
        } else {
            self.status(meta, cx)
        };
        let worktree = self
            .snapshot
            .projects
            .iter()
            .flat_map(|project| &project.worktrees)
            .find(|worktree| {
                crate::paths::normalize_wt_path(&worktree.path)
                    == crate::paths::normalize_wt_path(&meta.wt_path)
            });
        let context = worktree.map_or_else(
            || meta.wt_path.clone(),
            |worktree| {
                format!(
                    "{} · {}",
                    sidebar_worktree_name(&worktree.name, worktree.is_main),
                    worktree.branch
                )
            },
        );
        let mut row = self
            .compact_item(
                format!("session-{}", id.raw()),
                format!(
                    "{title} · {} · {context} · {} · {status}",
                    meta.project,
                    meta.agent.label()
                ),
                meta.agent.icon_name(),
                self.selection == Some(Selection::Session(id)),
                Action::Select(Selection::Session(id)),
                cx,
            )
            .child(
                div()
                    .id(("compact-session-status", id.raw()))
                    .debug_selector(move || format!("compact-session-status-{}", id.raw()))
                    .absolute()
                    .right(rpx(1.0))
                    .bottom(rpx(1.0))
                    .size(rpx(11.0))
                    .rounded_full()
                    .bg(rail_background(cx))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(compact_status_glyph(status), 11.0, color)),
            )
            .group("compact-session-row")
            .child(
                self.control(
                    ("compact-close-session", id.raw()),
                    format!("Close {} in {}", meta.label, meta.project),
                    Action::Close(id),
                    cx,
                )
                .debug_selector(move || format!("compact-close-session-{}", id.raw()))
                .absolute()
                .right_0()
                .top_0()
                .w(rpx(SPACE_20))
                .h(rpx(ICON_MD))
                .opacity(0.0)
                .group_hover("compact-session-row", |button| button.opacity(1.0))
                .focus_visible(|button| button.opacity(1.0))
                .when_some(
                    self.session_close_bounds.get(&id).cloned(),
                    |button, bounds| {
                        button.child(
                            gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                                .absolute()
                                .inset_0(),
                        )
                    },
                )
                .child(icon("close", ICON_XS, c::FG_DIM())),
            );
        if self.pending_close == Some(id) && self.canvas_close_anchor.is_none() {
            if let Some(bounds) = self.session_close_bounds.get(&id) {
                row = row.child(gpui::deferred(self.confirmation_popup(
                    &format!(
                        "Close {} in {}? Its process will stop. The worktree stays on disk.",
                        meta.label, meta.project
                    ),
                    Action::ConfirmClose(id),
                    bounds.get(),
                    window,
                    cx,
                )));
            }
        }
        row.into_any_element()
    }

    pub(super) fn collapsed_navigation(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut body = div()
            .id("sidebar-compact-navigation")
            .debug_selector(|| "sidebar-compact-navigation".into())
            .flex()
            .flex_col()
            .items_center()
            .flex_shrink_0()
            .gap(rpx(2.0));
        if self.mode == ViewMode::List {
            for (heading, ids) in self.list_session_groups(cx) {
                body = body.child(
                    div()
                        .id(SharedString::from(format!("compact-group-{heading}")))
                        .aria_label(format!("{heading}, {} sessions", ids.len()))
                        .w(rpx(24.0))
                        .h(rpx(1.0))
                        .my(rpx(SPACE_SM))
                        .bg(c::BORDER()),
                );
                for id in ids {
                    if let Some(meta) = self.runtime.read(cx).registry.read(cx).meta(id).cloned() {
                        body = body.child(self.compact_session(&meta, window, cx));
                    }
                }
            }
        } else {
            for (position, project) in self.snapshot.projects.iter().enumerate() {
                let idx = project.idx;
                let Some(path) = self.project_paths.get(&idx) else {
                    continue;
                };
                let expanded = !self.collapsed_projects.contains(path);
                let mut group = div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(rpx(2.0))
                    .when(position > 0, |group| {
                        group
                            .mt(rpx(SPACE_LG))
                            .pt(rpx(SPACE_SM))
                            .border_t_1()
                            .border_color(c::BORDER_SOFT())
                    })
                    .child(
                        self.compact_item(
                            format!("project-{idx}"),
                            format!(
                                "{} project · {path} · {}",
                                project.name,
                                if expanded {
                                    "Collapse project"
                                } else {
                                    "Expand project"
                                }
                            ),
                            if expanded { "folder-open" } else { "folder" },
                            self.selection == Some(Selection::Project(idx)),
                            Action::ToggleProject(path.clone()),
                            cx,
                        )
                        .when_some(
                            self.project_toggle_focus.get(path),
                            gpui::InteractiveElement::track_focus,
                        )
                        .when(!project.worktrees.is_empty(), |item| {
                            item.child(div().absolute().right_0().bottom(rpx(1.0)).child(icon(
                                if expanded { "chev-down" } else { "chev-right" },
                                8.0,
                                c::FG_DIM(),
                            )))
                        }),
                    );
                if expanded {
                    for worktree in &project.worktrees {
                        group = group.child(
                            self.compact_item(
                                format!("worktree-{}", worktree.path),
                                format!(
                                    "{} · {} · {} · {}",
                                    project.name,
                                    sidebar_worktree_name(&worktree.name, worktree.is_main),
                                    worktree.branch,
                                    worktree.path
                                ),
                                "git-branch",
                                self.selection
                                    == Some(Selection::Worktree(idx, worktree.path.clone())),
                                Action::Select(Selection::Worktree(idx, worktree.path.clone())),
                                cx,
                            )
                            .when_some(
                                self.worktree_focus.get(&worktree.path),
                                gpui::InteractiveElement::track_focus,
                            ),
                        );
                        for id in &worktree.sessions {
                            if let Some(meta) =
                                self.runtime.read(cx).registry.read(cx).meta(*id).cloned()
                            {
                                group = group.child(self.compact_session(&meta, window, cx));
                            }
                        }
                    }
                }
                body = body.child(group);
            }
        }
        body.into_any_element()
    }

    pub(super) fn collapsed_terminals(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut panel = div()
            .id("sidebar-compact-terminals")
            .debug_selector(|| "sidebar-compact-terminals".into())
            .flex()
            .flex_col()
            .items_center()
            .flex_shrink_0()
            .gap(rpx(2.0))
            .py(rpx(SPACE_SM))
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .mt(rpx(SPACE_SM))
            .child(
                self.compact_item(
                    "fold-terminals".into(),
                    if self.terminals_collapsed {
                        "Expand terminals"
                    } else {
                        "Collapse terminals"
                    }
                    .into(),
                    "terminal",
                    false,
                    Action::FoldTerminals,
                    cx,
                )
                .child(div().absolute().right_0().bottom(rpx(1.0)).child(icon(
                    if self.terminals_collapsed {
                        "chev-right"
                    } else {
                        "chev-down"
                    },
                    8.0,
                    c::FG_DIM(),
                ))),
            );
        if !self.terminals_collapsed {
            let terminals = self
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .home_terminals()
                .to_vec();
            for (index, meta) in terminals.iter().enumerate() {
                if self.terminal_owners.get(&meta.id).copied().unwrap_or(1) != self.active_workspace
                {
                    continue;
                }
                let terminal = self
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .home_terminal(index)
                    .cloned();
                let (title, status, directory) = terminal.map_or_else(
                    || {
                        (
                            meta.label.clone(),
                            "Starting",
                            "Directory unavailable".to_string(),
                        )
                    },
                    |terminal| {
                        terminal.update(cx, |terminal, _| {
                            (
                                session_display_title(meta, terminal.title()),
                                home_terminal_status(
                                    terminal.spawn_error().is_some(),
                                    terminal.is_pending_attach(),
                                    terminal.alive(),
                                ),
                                terminal
                                    .current_cwd()
                                    .or(terminal.initial_cwd())
                                    .unwrap_or("Directory unavailable")
                                    .to_string(),
                            )
                        })
                    },
                );
                let id = meta.id;
                panel = panel.child(
                    self.compact_item(
                        format!("home-{}", id.raw()),
                        format!("{title} · {directory} · Terminal · {status}"),
                        "terminal",
                        self.selection == Some(Selection::Home(id)),
                        Action::Select(Selection::Home(id)),
                        cx,
                    )
                    .group("compact-home-row")
                    .child(
                        div()
                            .absolute()
                            .right(rpx(1.0))
                            .bottom(rpx(1.0))
                            .size(rpx(11.0))
                            .rounded_full()
                            .bg(rail_background(cx))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(
                                compact_status_glyph(status),
                                11.0,
                                match status {
                                    "Running" => c::GREEN(),
                                    "Failed" => c::RED(),
                                    _ => c::FG_DIM(),
                                },
                            )),
                    )
                    .child(
                        self.control(("compact-close-home", id.raw()), format!("Close {}", meta.label), Action::CloseHome(id), cx)
                            .debug_selector(move || format!("compact-close-home-{}", id.raw()))
                            .absolute().right_0().top_0().w(rpx(SPACE_20)).h(rpx(ICON_MD))
                            .opacity(0.0)
                            .group_hover("compact-home-row", |button| button.opacity(1.0))
                            .focus_visible(|button| button.opacity(1.0))
                            .when_some(self.home_close_bounds.get(&id).cloned(), |button, bounds| {
                                button.child(gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {}).absolute().inset_0())
                            })
                            .child(icon("close", ICON_XS, c::FG_DIM())),
                    )
                    .when(self.pending_home_close == Some(id) && self.canvas_close_anchor.is_none(), |row| {
                        if let Some(bounds) = self.home_close_bounds.get(&id) {
                            row.child(gpui::deferred(self.confirmation_popup(
                                &format!("Close {}? Its shell and running commands will stop. Files remain on disk.", meta.label),
                                Action::ConfirmHome(id), bounds.get(), window, cx,
                            )))
                        } else { row }
                    }),
                );
            }
        }
        panel
            .child(self.compact_item(
                "add-terminal".into(),
                "Add terminal".into(),
                "add-terminal",
                false,
                Action::AddTerminal,
                cx,
            ))
            .into_any_element()
    }
}

fn compact_status_glyph(status: &str) -> &'static str {
    match status {
        "Needs you" => "question",
        "Failed" => "close",
        "Starting" => "ring",
        "Working" | "Running" => "dot",
        "Done" => "check",
        _ => "ring",
    }
}
