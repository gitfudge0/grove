//! Session switcher presentation, filtering and focus routing.

use super::*;

impl Shell {
    pub(super) fn switcher_rows(&self, cx: &App) -> Vec<SwitcherRow> {
        let workspaces = &cx
            .global::<crate::settings::SettingsState>()
            .store
            .workspaces;
        let mut rows: Vec<_> = workspaces
            .rows
            .iter()
            .filter(|workspace| switcher_matches(&self.switcher_query, &workspace.name, None))
            .map(|workspace| SwitcherRow {
                target: SwitchTarget::Workspace(workspace.id),
                title: workspace.name.clone(),
                context: None,
                icon: "folder",
                status: None,
                current: workspace.id == workspaces.active,
            })
            .collect();
        let targets = self.sidebar.read(cx).visible_session_targets(cx);
        let runtime = self.runtime.read(cx);
        let registry = runtime.registry.read(cx);
        let activity = runtime.activity.read(cx);
        for (id, label) in targets {
            let Some(meta) = registry.meta(id) else {
                continue;
            };
            let (title, _) = label.rsplit_once(" · ").unwrap_or((&label, ""));
            let worktree = std::path::Path::new(&meta.wt_path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&meta.wt_path);
            let context = format!("{} / {worktree}", meta.project);
            if !switcher_matches(&self.switcher_query, title, Some(&context)) {
                continue;
            }
            let status = if let Some(session) = registry.codex(id) {
                {
                    let label = session.read(cx).state.session_status();
                    (label, super::super::status::session_kind(label).color())
                }
            } else if registry
                .session(id)
                .is_some_and(|session| session.read(cx).spawn_error().is_some())
            {
                ("Failed", c::RED())
            } else if registry
                .session(id)
                .is_none_or(|session| session.read(cx).is_pending_attach())
            {
                ("Starting", c::FG_DIM())
            } else if registry
                .session(id)
                .is_some_and(|session| session.read(cx).has_exited())
                || activity.state_of(id) == ActivityState::Exited
            {
                ("Exited", c::FG_DIM())
            } else if meta.agent == Agent::Terminal {
                ("Running", c::GREEN())
            } else {
                match activity.state_of(id) {
                    ActivityState::Working => ("Working", c::GREEN()),
                    ActivityState::WaitingForInput => ("Needs you", c::AMBER()),
                    ActivityState::Done => ("Done", c::FG_DIM()),
                    ActivityState::Idle => ("Idle", c::FG_DIM()),
                    ActivityState::Exited => ("Exited", c::FG_DIM()),
                }
            };
            let icon = match meta.agent {
                Agent::Claude => "claude",
                Agent::Codex => "codex",
                Agent::OpenCode => "opencode",
                Agent::Terminal => "terminal",
            };
            rows.push(SwitcherRow {
                target: SwitchTarget::Session(id),
                title: title.to_string(),
                context: Some(context),
                icon,
                status: Some(status),
                current: false,
            });
        }
        rows
    }

    pub(super) fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar
            .update(cx, super::super::sidebar::Sidebar::dismiss_project_context);
        if self.shortcut_blocked_in(window, cx) {
            return;
        }
        self.switcher_return_focus = window.focused(cx);
        self.switcher_query.clear();
        self.switcher_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.switcher_index = switcher_default_index(&self.switcher_rows(cx));
        self.switcher_scroll.set_offset(gpui::Point::default());
        self.switcher_error = None;
        self.switcher_open = true;
        cx.notify();
    }

    fn close_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.switcher_open = false;
        if let Some(focus) = self.switcher_return_focus.take() {
            focus.focus(window, cx);
        }
        cx.notify();
    }

    fn select_switcher_row(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.switcher_rows(cx).get(index).map(|row| row.target) else {
            return;
        };
        match target {
            SwitchTarget::Workspace(id) => {
                let workspaces = &cx
                    .global::<crate::settings::SettingsState>()
                    .store
                    .workspaces;
                if !workspaces.rows.iter().any(|row| row.id == id) {
                    self.switcher_error = Some("Workspace is no longer available.".into());
                    self.switcher_index = switcher_default_index(&self.switcher_rows(cx));
                    cx.notify();
                    return;
                }
                let current = workspaces.active;
                if id != current {
                    let ((), saved) =
                        crate::settings::SettingsState::update_and_flush_checked(cx, |store| {
                            store.workspaces.select(id);
                        });
                    if let Err(error) = saved {
                        self.switcher_error = Some(format!("Could not save workspace: {error}"));
                        cx.notify();
                        return;
                    }
                }
                self.close_switcher(window, cx);
                if id != current {
                    self.sidebar.update(cx, |sidebar, cx| {
                        sidebar.focus_grid_workspace_after_switch(window, cx);
                    });
                }
            }
            SwitchTarget::Session(id) => {
                if !self
                    .sidebar
                    .read(cx)
                    .visible_session_targets(cx)
                    .iter()
                    .any(|(visible, _)| *visible == id)
                {
                    self.switcher_error =
                        Some("Session is no longer available in this workspace.".into());
                    self.switcher_index = switcher_default_index(&self.switcher_rows(cx));
                    cx.notify();
                    return;
                }
                self.close_switcher(window, cx);
                self.sidebar
                    .update(cx, |sidebar, cx| sidebar.select_session_id(id, window, cx));
            }
        }
    }

    fn new_session_from_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_switcher(window, cx);
        self.open_new_session(window, cx);
    }

    pub(super) fn switcher_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows = self.switcher_rows(cx);
        let count = rows.len();
        let heading_offset = usize::from(
            rows.iter()
                .any(|row| matches!(row.target, SwitchTarget::Workspace(_))),
        );
        match event.keystroke.key.as_str() {
            "escape" => self.close_switcher(window, cx),
            "up" if count > 0 => {
                self.switcher_index = (self.switcher_index + count - 1) % count;
                self.switcher_scroll
                    .scroll_to_item(self.switcher_index + heading_offset);
                cx.notify();
            }
            "down" if count > 0 => {
                self.switcher_index = (self.switcher_index + 1) % count;
                self.switcher_scroll
                    .scroll_to_item(self.switcher_index + heading_offset);
                cx.notify();
            }
            "tab" => {
                let has_new_session = self.switcher_query.trim().is_empty()
                    && self.sidebar.read(cx).visible_session_targets(cx).is_empty();
                if has_new_session && !self.switcher_new_session_focus.is_focused(window) {
                    self.switcher_new_session_focus.focus(window, cx);
                } else {
                    self.switcher_input
                        .update(cx, |input, cx| input.focus(window, cx));
                }
            }
            "enter" if self.switcher_new_session_focus.is_focused(window) => {
                self.new_session_from_switcher(window, cx);
            }
            "enter" => self.select_switcher_row(self.switcher_index, window, cx),
            _ => return,
        }
        window.prevent_default();
        cx.stop_propagation();
    }

    fn switcher_row(
        &self,
        index: usize,
        row: SwitcherRow,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = index == self.switcher_index;
        let debug_id = format!("session-switcher-row-{index}");
        let target = row.target;
        let status = row.status;
        let overlay = div()
            .id(("session-switcher-row", index))
            .debug_selector(move || debug_id.clone())
            .role(gpui::Role::Button)
            .cursor_pointer()
            .aria_label(row.title.clone())
            .min_w_0()
            .px(rpx(ROW_PX))
            .py(rpx(ROW_PY))
            .flex()
            .items_center()
            .gap(rpx(SPACE_2XL))
            .rounded(rpx(RADIUS_GROUP))
            .text_color(if selected { c::FG() } else { c::FG_DIM() })
            .when(selected, |item| item.bg(c::BG_HL()))
            .hover(|item| item.bg(c::BG_HOVER()).text_color(c::FG()))
            .on_click(
                cx.listener(move |this, _, window, cx| this.select_switcher_row(index, window, cx)),
            );
        let row = overlay
            .child(icon(
                row.icon,
                ICON_MD,
                if matches!(row.target, SwitchTarget::Workspace(_)) {
                    c::FG_DIM()
                } else {
                    c::MAGENTA()
                },
            ))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_XS))
                    .child(div().truncate().text_size(rpx(TEXT_BODY)).child(row.title))
                    .when_some(row.context, |body, context| {
                        body.child(
                            div()
                                .truncate()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(context),
                        )
                    }),
            )
            .when(row.current, |item| {
                item.child(
                    div()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_SM))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(icon("check", ICON_SM, c::FG_DIM()))
                        .child("Current"),
                )
            })
            .when_some(status, |item, (label, _color)| {
                item.child(
                    div()
                        .flex_shrink_0()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(super::super::status::session_kind(label).color())
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_SM))
                        .child(super::super::status::mark(
                            format!("switcher-status-{target:?}"),
                            super::super::status::session_kind(label),
                            ICON_SM,
                            cx,
                        ))
                        .child(label),
                )
            });
        let idle_fill = c::alpha(c::BG(), 0.0);
        let selected_fill = c::BG_HL();
        motion::background(
            row,
            format!("session-switcher-row-selection-{target:?}-{selected}"),
            if selected { idle_fill } else { selected_fill },
            if selected { selected_fill } else { idle_fill },
            std::time::Duration::from_millis(MOTION_FAST_MS),
            cx,
        )
    }

    pub(super) fn session_switcher(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = self.switcher_rows(cx);
        let workspace_name = cx
            .global::<crate::settings::SettingsState>()
            .store
            .workspaces
            .name(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .workspaces
                    .active,
            )
            .to_string();
        let workspace_count = rows
            .iter()
            .filter(|row| matches!(row.target, SwitchTarget::Workspace(_)))
            .count();
        let session_count = rows.len() - workspace_count;
        let has_any_sessions = !self.sidebar.read(cx).visible_session_targets(cx).is_empty();
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let viewport_w = f32::from(window.viewport_size().width) / scale;
        let viewport_h = f32::from(window.viewport_size().height) / scale;
        let panel_w = MODAL_W_LG.min((viewport_w - SPACE_LG * 2.0).max(0.0));
        let top = SWITCHER_TOP.min((viewport_h - SWITCHER_MIN_H - SPACE_LG).max(SPACE_LG));
        let panel_h = SWITCHER_MAX_H.min((viewport_h - top - SPACE_LG).max(0.0));
        let compact = panel_h < SWITCHER_MIN_H + APPBAR_H;
        let empty_state = div()
            .pt(rpx(SPACE_3XL))
            .child(
                div()
                    .px(rpx(ROW_PX))
                    .pb(rpx(SPACE_LG))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_MUTE())
                    .child(format!("Sessions in {workspace_name}")),
            )
            .child(
                div()
                    .px(rpx(ROW_PX))
                    .py(rpx(ROW_PY))
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_2XL))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_XS))
                            .child(
                                div()
                                    .text_color(c::FG())
                                    .text_size(rpx(TEXT_BODY))
                                    .child(format!("No sessions in {workspace_name}")),
                            )
                            .child(
                                div()
                                    .text_color(c::FG_DIM())
                                    .text_size(rpx(TEXT_SMALL))
                                    .child("Open another workspace or start a session here."),
                            ),
                    )
                    .child(
                        div()
                            .id("session-switcher-new-session")
                            .debug_selector(|| "session-switcher-new-session".into())
                            .role(gpui::Role::Button)
                            .cursor_pointer()
                            .track_focus(&self.switcher_new_session_focus)
                            .tab_index(0)
                            .focus_visible(|style| style.bg(c::BG_HOVER()))
                            .px(rpx(SPACE_2XL))
                            .py(rpx(SPACE_LG))
                            .rounded(rpx(RADIUS_GROUP))
                            .border_1()
                            .border_color(c::BORDER_STRONG())
                            .text_color(c::FG())
                            .text_size(rpx(TEXT_SMALL))
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_SM))
                            .child(icon("plus", ICON_SM, c::FG()))
                            .child("New session")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_session_from_switcher(window, cx);
                            })),
                    ),
            );
        let overlay = div()
            .id("session-switcher-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .bg(c::SCRIM())
            .flex()
            .items_start()
            .justify_center()
            .pt(rpx(top))
            .capture_key_down(cx.listener(Self::switcher_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_switcher(window, cx)),
            );
        let overlay = overlay.child(
            div()
                .id("session-switcher")
                .debug_selector(|| "session-switcher".into())
                .w(rpx(panel_w))
                .max_h(rpx(panel_h))
                .flex()
                .flex_col()
                .rounded(rpx(RADIUS_PANEL))
                .border_1()
                .border_color(c::BORDER())
                .bg(c::SURFACE_RAISED())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex_shrink_0()
                        .px(rpx(SPACE_3XL))
                        .pt(rpx(if compact { SPACE_LG } else { SPACE_3XL }))
                        .pb(rpx(if compact { SPACE_SM } else { SPACE_2XL }))
                        .flex()
                        .flex_col()
                        .gap(rpx(SPACE_2XL))
                        .child(
                            div()
                                .text_size(rpx(TEXT_TITLE))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(c::FG())
                                .child("Switch to"),
                        )
                        .child(
                            div()
                                .id("session-switcher-search")
                                .h(rpx(APPBAR_H))
                                .px(rpx(SPACE_2XL))
                                .rounded(rpx(RADIUS_GROUP))
                                .bg(
                                    if self
                                        .switcher_input
                                        .read(cx)
                                        .focus_handle(cx)
                                        .is_focused(window)
                                    {
                                        c::BG_HOVER()
                                    } else {
                                        c::FIELD_FILL()
                                    },
                                )
                                .border_1()
                                .border_color(c::BORDER())
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_LG))
                                .child(icon("search", ICON_MD, c::FG_DIM()))
                                .child(
                                    Input::new(&self.switcher_input)
                                        .file_drop_overlay(
                                            crate::views::components::file_drop_overlay(),
                                        )
                                        .aria_label("Find a workspace or session")
                                        .appearance(false)
                                        .bordered(false)
                                        .focus_bordered(false)
                                        .text_size(rpx(TEXT_BODY))
                                        .text_color(c::FG())
                                        .p_0(),
                                ),
                        ),
                )
                .child(
                    div()
                        .id("session-switcher-list")
                        .debug_selector(|| "session-switcher-list".into())
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.switcher_scroll)
                        .px(rpx(SPACE_LG))
                        .pb(rpx(SPACE_LG))
                        .when(workspace_count > 0, |list| {
                            list.child(
                                div()
                                    .py(rpx(SPACE_LG))
                                    .px(rpx(ROW_PX))
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_MUTE())
                                    .child("Workspaces"),
                            )
                        })
                        .children(rows.into_iter().enumerate().map(|(index, row)| {
                            let is_session = matches!(row.target, SwitchTarget::Session(_));
                            div()
                                .when(is_session && index == workspace_count, |item| {
                                    item.child(
                                        div()
                                            .pt(rpx(SPACE_3XL))
                                            .pb(rpx(SPACE_LG))
                                            .px(rpx(ROW_PX))
                                            .text_size(rpx(TEXT_SMALL))
                                            .text_color(c::FG_MUTE())
                                            .child(format!("Sessions in {workspace_name}")),
                                    )
                                })
                                .child(self.switcher_row(index, row, cx))
                        }))
                        .when(
                            !has_any_sessions && self.switcher_query.trim().is_empty(),
                            |list| list.child(empty_state),
                        )
                        .when(workspace_count + session_count == 0, |list| {
                            list.child(
                                div()
                                    .p(rpx(SPACE_3XL))
                                    .text_size(rpx(TEXT_BODY))
                                    .text_color(c::FG_DIM())
                                    .child("No matching workspaces or sessions"),
                            )
                        }),
                )
                .when_some(self.switcher_error.as_ref(), |panel, error| {
                    panel.child(
                        div()
                            .id("session-switcher-error")
                            .role(gpui::Role::Alert)
                            .px(rpx(SPACE_3XL))
                            .py(rpx(SPACE_LG))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::RED())
                            .child(error.clone()),
                    )
                })
                .child(
                    div()
                        .flex_shrink_0()
                        .px(rpx(SPACE_3XL))
                        .py(rpx(SPACE_2XL))
                        .border_t_1()
                        .border_color(c::BORDER_SOFT())
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_MUTE())
                        .child("↑ ↓ navigate    Enter open    Esc close"),
                ),
        );
        motion::slow(overlay, "session-switcher-enter", cx)
    }
}
