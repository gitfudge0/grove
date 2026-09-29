//! Grove's app-owned header and the empty canvas for the UI rebuild.
use super::components::{form_action, header_control};
use super::motion;
use super::settings_panel::{SettingsPanel, SettingsPanelEvent};
use super::worktree_launcher::WorktreeLauncherEvent;
use super::{rpx, tokens::*};
use crate::{
    activity::ActivityState,
    entities::session_registry::SessionId,
    runtime::{Runtime, RuntimeEvent},
    theme::ThemeState,
};
use crate::{icons::icon, keymap as k, launcher::PaletteRow, theme as c};
use gpui::{
    actions, div, prelude::*, App, Context, Entity, FocusHandle, Focusable, FontWeight,
    MouseButton, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};
use grove_core::agent::Agent;
use grove_core::storage::SidebarAppearance;

const TRAFFIC_LIGHT_D: f32 = 12.0;
const TRAFFIC_CONTROL_W: f32 = 18.0;
const SWITCHER_TOP: f32 = APPBAR_H + SPACE_3XL * 2.0;
const SWITCHER_MIN_H: f32 = APPBAR_H * 3.0 + SPACE_2XL;
const SWITCHER_MAX_H: f32 = MODAL_SCROLL_MAX_H + APPBAR_H * 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SwitchTarget {
    Workspace(u64),
    Session(SessionId),
}

struct SwitcherRow {
    target: SwitchTarget,
    title: String,
    context: Option<String>,
    icon: &'static str,
    status: Option<(&'static str, gpui::Hsla)>,
    current: bool,
}

fn switcher_default_index(rows: &[SwitcherRow]) -> usize {
    rows.iter()
        .position(|row| matches!(row.target, SwitchTarget::Session(_)))
        .unwrap_or(0)
}

fn switcher_matches(query: &str, title: &str, context: Option<&str>) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || title.to_lowercase().contains(&query)
        || context.is_some_and(|context| context.to_lowercase().contains(&query))
}

actions!(shell, [Quit, CloseWindow]);

/// Bubble only unconsumed navigation keys. Mounting component Root would also
/// install its Copy action context, which can steal PTY Ctrl+C. TerminalView
/// consumes Tab as input first; open menus and confirmations trap it in capture.
fn traverse_unhandled_tab(event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut App) {
    let key = &event.keystroke;
    if key.key == "tab"
        && !key.modifiers.control
        && !key.modifiers.alt
        && !key.modifiers.platform
        && !key.modifiers.function
    {
        window.prevent_default();
        if key.modifiers.shift {
            window.focus_prev(cx);
        } else {
            window.focus_next(cx);
        }
        cx.stop_propagation();
    }
}

pub struct Shell {
    focus: FocusHandle,
    runtime: Entity<Runtime>,
    workspaces: Entity<super::workspace_manager::WorkspaceManager>,
    sidebar: Entity<super::sidebar::Sidebar>,
    statusbar: Entity<super::statusbar::Statusbar>,
    launcher: Entity<super::worktree_launcher::WorktreeLauncher>,
    settings: Entity<SettingsPanel>,
    _settings_events: gpui::Subscription,
    _runtime_events: gpui::Subscription,
    _launcher_events: gpui::Subscription,
    _sidebar_events: gpui::Subscription,
    switcher_open: bool,
    switcher_index: usize,
    switcher_return_focus: Option<FocusHandle>,
    switcher_new_session_focus: FocusHandle,
    switcher_input: Entity<InputState>,
    _switcher_input_events: gpui::Subscription,
    switcher_query: String,
    switcher_error: Option<String>,
    switcher_scroll: gpui::ScrollHandle,
    backend_choice_open: bool,
    backend_choice_focus: FocusHandle,
    backend_choice_return_focus: Option<FocusHandle>,
    backend_choice_index: usize,
    backend_selected_tmux: bool,
    backend_choice_error: Option<String>,
    window_observers: Option<Vec<gpui::Subscription>>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let runtime = cx.new(Runtime::new);
        #[cfg(not(test))]
        runtime.update(cx, Runtime::discover_tmux_sessions);
        let workspaces = cx.new(|cx| super::workspace_manager::WorkspaceManager::new(window, cx));
        let switcher_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Find a workspace or session"));
        let switcher_input_events =
            cx.subscribe_in(&switcher_input, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.switcher_query = this.switcher_input.read(cx).value().to_string();
                    this.switcher_index = switcher_default_index(&this.switcher_rows(cx));
                    this.switcher_scroll.set_offset(gpui::Point::default());
                    this.switcher_error = None;
                    cx.notify();
                }
            });
        let sidebar = cx.new(|cx| super::sidebar::Sidebar::new(runtime.clone(), window, cx));
        let focus = cx.focus_handle();
        sidebar.update(cx, |sidebar, _| {
            sidebar.set_workspace_selector(workspaces.clone());
            sidebar.set_shell_focus(focus.clone());
        });
        cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
        let statusbar =
            cx.new(|cx| super::statusbar::Statusbar::new(runtime.clone(), sidebar.clone(), cx));
        let launcher = cx.new(|cx| {
            super::worktree_launcher::WorktreeLauncher::new(
                runtime.clone(),
                sidebar.clone(),
                window,
                cx,
            )
        });
        let settings = cx.new(|cx| SettingsPanel::new(runtime.clone(), cx));
        sidebar.update(cx, |sidebar, cx| {
            sidebar.set_settings_panel(settings.clone(), cx);
        });
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        let launcher_events = cx.subscribe_in(&launcher, window, |this, _, event, window, cx| {
            let WorktreeLauncherEvent::Command(row) = event;
            this.activate_palette_command(row.clone(), window, cx);
        });
        let settings_events =
            cx.subscribe_in(
                &settings,
                window,
                |this, _, event, window, cx| match event {
                    SettingsPanelEvent::Closed => cx.notify(),
                    SettingsPanelEvent::OpenArchivedProjects => {
                        this.sidebar.update(cx, |sidebar, cx| {
                            sidebar.open_archived_projects(window, cx);
                        });
                    }
                },
            );
        let sidebar_events =
            cx.subscribe_in(&sidebar, window, |this, _, event, window, cx| match event {
                super::sidebar::SidebarEvent::SettingsRequested => {
                    this.settings
                        .update(cx, |settings, cx| settings.open(window, cx));
                }
            });
        let runtime_events =
            cx.subscribe_in(&runtime, window, |this, _, event, window, cx| match event {
                RuntimeEvent::BackendChoiceRequested => {
                    this.backend_choice_return_focus = window.focused(cx);
                    this.backend_choice_open = true;
                    this.backend_choice_index = 0;
                    this.backend_selected_tmux = false;
                    this.backend_choice_error = None;
                    this.backend_choice_focus.focus(window, cx);
                    cx.notify();
                }
            });
        let backend_choice_focus = cx.focus_handle();
        Self {
            statusbar,
            launcher,
            settings,
            _settings_events: settings_events,
            _runtime_events: runtime_events,
            _launcher_events: launcher_events,
            _sidebar_events: sidebar_events,
            switcher_open: false,
            switcher_index: 0,
            switcher_return_focus: None,
            switcher_new_session_focus: cx.focus_handle(),
            switcher_input,
            _switcher_input_events: switcher_input_events,
            switcher_query: String::new(),
            switcher_error: None,
            switcher_scroll: gpui::ScrollHandle::new(),
            backend_choice_open: false,
            backend_choice_focus,
            backend_choice_return_focus: None,
            backend_choice_index: 0,
            backend_selected_tmux: false,
            backend_choice_error: None,
            sidebar,
            focus,
            runtime,
            workspaces,
            window_observers: None,
        }
    }

    fn header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fullscreen_label = if window.is_fullscreen() {
            "Exit full screen"
        } else {
            "Enter full screen"
        };
        let traffic = |id, label, color, glyph| {
            header_control(id, label)
                .debug_selector(move || id.into())
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .tab_index(0)
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .w(rpx(TRAFFIC_CONTROL_W))
                .child(
                    div()
                        .size(rpx(TRAFFIC_LIGHT_D))
                        .rounded_full()
                        .bg(color)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            icon(glyph, ICON_XS, c::WINDOW_CONTROL_GLYPH())
                                .opacity(0.0)
                                .group_hover("traffic-lights", |s| s.opacity(1.0)),
                        ),
                )
        };
        let grid = self.sidebar.read(cx).is_grid() && !self.settings.read(cx).is_open();
        let rail_visible = self.sidebar.read(cx).rail_visible(window, cx);
        let collapsed = rail_visible && self.sidebar.read(cx).is_collapsed(cx);
        let solid_sidebar = cx
            .global::<crate::settings::SettingsState>()
            .store
            .sidebar_appearance
            == SidebarAppearance::Solid;
        let header = div()
            .id("app-header")
            .debug_selector(|| "app-header".into())
            .flex()
            .items_center()
            .flex_shrink_0()
            .h(rpx(APPBAR_H))
            .when(grid, |header| header.pr(rpx(SPACE_2XL)))
            .when(grid, |header| header.bg(c::BG_STRIP()))
            .child(
                div()
                    .id("header-rail-segment")
                    .debug_selector(|| "header-rail-segment".into())
                    .group("traffic-lights")
                    .h_full()
                    .px(rpx(SPACE_2XL))
                    .flex_shrink_0()
                    .when(!grid, |segment| {
                        segment
                            .w(rpx(self.sidebar.read(cx).rail_width(window, cx).max(
                                if rail_visible {
                                    0.0
                                } else {
                                    TRAFFIC_CONTROL_W * 3.0 + SPACE_2XL * 2.0
                                },
                            )))
                            .border_r_1()
                            .border_color(c::BORDER())
                            .when(solid_sidebar, |segment| segment.bg(c::BG_RAIL()))
                    })
                    .on_mouse_down(MouseButton::Left, |event, window, _| {
                        if event.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    })
                    .flex()
                    .items_center()
                    .when(!collapsed, |segment| {
                        segment
                            .child(
                                traffic("window-close", "Close window", c::WINDOW_CLOSE(), "close")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.flush(cx);
                                        window.remove_window();
                                    })),
                            )
                            .child(
                                traffic(
                                    "window-minimize",
                                    "Minimize window",
                                    c::WINDOW_MINIMIZE(),
                                    "minus",
                                )
                                .on_click(|_, window, cx| {
                                    cx.stop_propagation();
                                    window.minimize_window();
                                }),
                            )
                            .child(
                                traffic(
                                    "window-fullscreen",
                                    fullscreen_label,
                                    c::WINDOW_FULLSCREEN(),
                                    "plus",
                                )
                                .on_click(|_, window, cx| {
                                    cx.stop_propagation();
                                    window.toggle_fullscreen();
                                }),
                            )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_1()
                    .h_full()
                    .bg(c::BG_STRIP())
                    .when(grid, |header| {
                        header.child(div().ml(rpx(SPACE_2XL)).child(self.workspaces.clone()))
                    })
                    .when(grid, |header| {
                        header.child(
                            div()
                                .id("header-empty-drag-region")
                                .flex_1()
                                .h_full()
                                .on_mouse_down(MouseButton::Left, |event, window, _| {
                                    if event.click_count == 2 {
                                        window.titlebar_double_click();
                                    } else {
                                        window.start_window_move();
                                    }
                                }),
                        )
                    })
                    .when(grid, |header| {
                        header.child(
                            self.sidebar
                                .update(cx, |sidebar, cx| sidebar.view_controls(cx)),
                        )
                    })
                    .when(!grid, |header| {
                        header.child(
                            div()
                                .id("header-main-drag-region")
                                .flex_1()
                                .h_full()
                                .on_mouse_down(MouseButton::Left, |event, window, _| {
                                    if event.click_count == 2 {
                                        window.titlebar_double_click();
                                    } else {
                                        window.start_window_move();
                                    }
                                }),
                        )
                    })
                    .child(
                        header_control("header-new-session", "New session")
                            .debug_selector(|| "header-new-session".into())
                            .when(!grid, |control| control.mr(rpx(SPACE_2XL)))
                            .tab_index(0)
                            .focus_visible(|style| style.bg(c::BG_HOVER()))
                            .child(icon("plus", ICON_MD, c::FG()))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.launcher
                                    .update(cx, |launcher, cx| launcher.open(window, cx));
                            })),
                    ),
            );
        motion::base(header, format!("app-header-grid-{grid}"), cx)
    }

    fn flush(&self, cx: &mut Context<Self>) {
        self.runtime.update(cx, Runtime::shutdown);
    }

    fn activate_palette_command(
        &mut self,
        row: PaletteRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match row {
            PaletteRow::TerminalHome => self
                .sidebar
                .update(cx, |sidebar, cx| sidebar.add_terminal(window, cx)),
            PaletteRow::TerminalWt => self.sidebar.update(cx, |sidebar, cx| {
                sidebar.add_worktree_terminal_from_palette(cx);
            }),
            PaletteRow::AddProject => self.sidebar.update(cx, |sidebar, cx| {
                sidebar.add_project_from_palette(window, cx);
            }),
            PaletteRow::RunScript => self.sidebar.update(cx, |sidebar, cx| {
                sidebar.run_selected_script_from_palette(window, cx);
            }),
            PaletteRow::ViewDiff => self.sidebar.update(cx, |sidebar, cx| {
                sidebar.open_selected_diff_from_palette(window, cx);
            }),
            PaletteRow::SwitchToSession => self.open_switcher(window, cx),
            PaletteRow::Settings | PaletteRow::Setting(_) => {
                self.settings
                    .update(cx, |settings, cx| settings.open(window, cx));
            }
            PaletteRow::Recent { .. }
            | PaletteRow::Combo { .. }
            | PaletteRow::NewSession
            | PaletteRow::NewMultiProjectSession => {}
        }
    }

    fn set_zoom(&self, delta: f32, cx: &mut Context<Self>) {
        if self.shortcut_blocked(cx) {
            return;
        }
        let current = cx.global::<crate::zoom::ZoomState>().zoom;
        let next = if delta == 0.0 {
            crate::zoom::ZOOM_DEFAULT
        } else {
            crate::zoom::snap(current + delta)
        };
        if next == current {
            return;
        }
        let ((), saved) = crate::settings::SettingsState::update_and_flush_checked(cx, |store| {
            store.ui_zoom = Some(next);
        });
        if saved.is_ok() {
            cx.update_global::<crate::zoom::ZoomState, _>(|zoom, _| zoom.zoom = next);
            cx.refresh_windows();
        }
    }

    fn shortcut_blocked(&self, cx: &App) -> bool {
        self.backend_choice_open
            || self.launcher.read(cx).is_open()
            || self.settings.read(cx).is_open()
            || self.switcher_open
            || self.workspaces.read(cx).is_open()
            || self.sidebar.read(cx).confirmation_open()
    }

    fn switcher_rows(&self, cx: &App) -> Vec<SwitcherRow> {
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
            let status = if registry
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

    fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shortcut_blocked(cx) {
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
        self.launcher
            .update(cx, |launcher, cx| launcher.open(window, cx));
    }

    fn switcher_key(
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

    fn choose_backend(&mut self, tmux: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.backend_choice_open || !self.runtime.read(cx).has_pending_managed_launch() {
            return;
        }
        let ((), saved) = crate::settings::SettingsState::update_and_flush_checked(cx, |store| {
            store.tmux_enabled = Some(tmux);
        });
        match saved {
            Ok(()) => {
                self.backend_choice_open = false;
                self.backend_choice_error = None;
                let launched = self
                    .runtime
                    .update(cx, Runtime::resume_pending_managed_launch);
                if launched {
                    self.launcher.update(cx, |launcher, cx| {
                        launcher.close_after_deferred_launch(window, cx);
                    });
                    self.sidebar.update(cx, |sidebar, cx| {
                        sidebar.select_active_session(cx);
                        sidebar.focus_active_session_after_palette(window, cx);
                    });
                }
                if let Some(focus) = self.backend_choice_return_focus.take() {
                    if !launched {
                        focus.focus(window, cx);
                    }
                }
            }
            Err(error) => {
                self.backend_choice_error = Some(format!("Could not save settings: {error}"));
            }
        }
        cx.notify();
    }

    fn select_backend(&mut self, tmux: bool, cx: &mut Context<Self>) {
        self.backend_selected_tmux = tmux;
        self.backend_choice_index = usize::from(tmux);
        self.backend_choice_error = None;
        cx.notify();
    }

    fn cancel_backend_choice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.runtime
            .update(cx, |runtime, _| runtime.cancel_pending_managed_launch());
        self.backend_choice_open = false;
        self.backend_choice_error = None;
        if let Some(focus) = self.backend_choice_return_focus.take() {
            focus.focus(window, cx);
        } else {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    fn backend_choice_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "left" | "up" => self.select_backend(false, cx),
            "right" | "down" => self.select_backend(true, cx),
            "tab" => self.backend_choice_index = (self.backend_choice_index + 1) % 4,
            "enter" | "space" => match self.backend_choice_index {
                0 | 1 => self.select_backend(self.backend_choice_index == 1, cx),
                2 => self.cancel_backend_choice(window, cx),
                _ => self.choose_backend(self.backend_selected_tmux, window, cx),
            },
            "escape" => self.cancel_backend_choice(window, cx),
            _ => return,
        }
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }

    fn backend_choice(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut choices = div().flex().flex_col().gap(rpx(SPACE_LG));
        for (index, label, detail) in [
            (0usize, "Native", "Sessions end when Grove closes."),
            (1usize, "Tmux", "Sessions survive Grove restarts."),
        ] {
            choices = choices.child(
                div()
                    .id(("backend-choice-option", index))
                    .debug_selector(move || format!("backend-choice-option-{index}"))
                    .role(gpui::Role::Button)
                    .aria_label(format!(
                        "{label}: {detail}{}",
                        if self.backend_selected_tmux == (index == 1) {
                            " Selected."
                        } else {
                            ""
                        }
                    ))
                    .w_full()
                    .min_w_0()
                    .p(rpx(SPACE_2XL))
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_SM))
                    .rounded(rpx(RADIUS_GROUP))
                    .border_1()
                    .border_color(if self.backend_choice_index == index {
                        c::SEL_RING()
                    } else {
                        c::BORDER()
                    })
                    .bg(if self.backend_selected_tmux == (index == 1) {
                        c::BG_HOVER()
                    } else {
                        c::FIELD_FILL()
                    })
                    .hover(|style| style.bg(c::BG_HOVER()))
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(rpx(TEXT_BODY))
                            .text_color(c::FG())
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(detail),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let _ = window;
                        this.select_backend(index == 1, cx);
                    })),
            );
        }
        let overlay = div()
            .id("backend-choice-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .bg(c::SCRIM())
            .flex()
            .items_center()
            .justify_center()
            .track_focus(&self.backend_choice_focus)
            .capture_key_down(cx.listener(Self::backend_choice_key))
            .child(
                div()
                    .id("backend-choice-dialog")
                    .debug_selector(|| "backend-choice-dialog".into())
                    .role(gpui::Role::Dialog)
                    .aria_label("Choose a session backend")
                    .w(rpx(MODAL_W_SM))
                    .max_w_full()
                    .p(rpx(SPACE_3XL))
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_2XL))
                    .rounded(rpx(RADIUS_PANEL))
                    .border_1()
                    .border_color(c::BORDER())
                    .bg(c::SURFACE_RAISED())
                    .child(
                        div()
                            .text_size(rpx(TEXT_TITLE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(c::FG())
                    .child("How should sessions run?"),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_BODY))
                            .text_color(c::FG_DIM())
                            .child("Choose before starting this session. You can change this later in Settings. Existing sessions keep their backend."),
                    )
                    .child(choices)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(rpx(SPACE_LG))
                            .child(
                                div()
                                    .rounded(rpx(RADIUS_PANEL))
                                    .border_1()
                                    .border_color(if self.backend_choice_index == 2 { c::SEL_RING() } else { c::SURFACE_RAISED() })
                                    .child(
                                        form_action("backend-choice-back", "Back", false, window, cx)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.cancel_backend_choice(window, cx);
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .id("backend-choice-start")
                                    .debug_selector(|| "backend-choice-start".into())
                                    .rounded(rpx(RADIUS_PANEL))
                                    .border_1()
                                    .border_color(if self.backend_choice_index == 3 { c::SEL_RING() } else { c::SURFACE_RAISED() })
                                    .child(
                                        form_action("backend-choice-start-action", "Start session", true, window, cx)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.choose_backend(this.backend_selected_tmux, window, cx);
                                            })),
                                    ),
                            ),
                    )
                    .when_some(self.backend_choice_error.as_ref(), |dialog, error| {
                        dialog.child(
                            div()
                                .id("backend-choice-error")
                                .role(gpui::Role::Alert)
                                .text_size(rpx(TEXT_BODY))
                                .text_color(c::RED())
                                .child(error.clone()),
                        )
                    }),
            );
        motion::base(overlay, "backend-choice-enter", cx)
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
            .aria_label(row.title.clone())
            .min_w_0()
            .px(rpx(ROW_PX))
            .py(rpx(ROW_PY))
            .flex()
            .items_center()
            .gap(rpx(SPACE_2XL))
            .rounded(rpx(RADIUS_GROUP))
            .text_color(c::FG())
            .when(selected, |item| item.bg(c::BG_HL()))
            .hover(|item| item.bg(c::BG_HOVER()))
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
            .when_some(status, |item, (label, color)| {
                item.child(
                    div()
                        .flex_shrink_0()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(color)
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

    fn session_switcher(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
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

impl Focusable for Shell {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        if self.backend_choice_open {
            self.backend_choice_focus.clone()
        } else {
            self.focus.clone()
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.backend_choice_open && !self.backend_choice_focus.is_focused(window) {
            self.backend_choice_focus.focus(window, cx);
        }
        if self.window_observers.is_none() {
            let runtime = self.runtime.clone();
            window.on_window_should_close(cx, move |_, cx| {
                runtime.update(cx, Runtime::shutdown);
                true
            });
            let runtime = self.runtime.clone();
            self.window_observers = Some(vec![
                cx.observe_window_appearance(window, |_, window, cx| {
                    ThemeState::set_system_mode(cx, window.appearance());
                    if cx.global::<ThemeState>().follow_system {
                        c::set_chrome_light(matches!(
                            window.appearance(),
                            gpui::WindowAppearance::Light | gpui::WindowAppearance::VibrantLight
                        ));
                    }
                    cx.notify();
                }),
                cx.observe_window_activation(window, move |_, window, cx| {
                    let active = window.is_window_active();
                    runtime.update(cx, |runtime, cx| {
                        runtime
                            .activity
                            .update(cx, |a, cx| a.set_window_focused(active, cx));
                        if active {
                            runtime
                                .upgrade
                                .update(cx, crate::entities::upgrade::Upgrade::check_if_due);
                        }
                    });
                }),
            ]);
        }

        // AppKit may restore the native buttons during fullscreen transitions.
        if !cfg!(test) {
            crate::platform::window_chrome::hide_native_buttons(window);
        }
        window.set_rem_size(gpui::px(cx.global::<crate::zoom::ZoomState>().rem_size()));
        div()
            .id("grove-shell")
            .font_family(crate::fonts::UI_FAMILY)
            .track_focus(&self.focus)
            .when(
                self.sidebar.read(cx).is_grid()
                    && !self.sidebar.read(cx).is_zen()
                    && !self.shortcut_blocked(cx),
                |shell| shell.key_context(k::Screen::Grid.key_context()),
            )
            .on_key_down(traverse_unhandled_tab)
            .on_key_down(cx.listener(|this, event, window, cx| {
                if this.switcher_open {
                    this.switcher_key(event, window, cx);
                }
            }))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &Quit, _, cx| {
                this.flush(cx);
                cx.quit();
            }))
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| {
                this.flush(cx);
                window.remove_window();
            }))
            .on_action(cx.listener(|this, _: &k::NewSession, window, cx| {
                if !this.shortcut_blocked(cx) {
                    this.launcher
                        .update(cx, |launcher, cx| launcher.open(window, cx));
                }
            }))
            .on_action(
                cx.listener(|this, _: &k::NewSessionInWorktree, window, cx| {
                    if !this.shortcut_blocked(cx) {
                        if let Some((project, path)) = this.sidebar.read(cx).selected_worktree() {
                            this.launcher.update(cx, |launcher, cx| {
                                launcher.open_for_worktree(project, &path, window, cx);
                            });
                        }
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &k::SwitchSession, window, cx| {
                this.open_switcher(window, cx);
            }))
            .on_action(cx.listener(|this, _: &k::NextSession, window, cx| {
                if !this.shortcut_blocked(cx) {
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.select_next_session(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::PrevSession, window, cx| {
                if !this.shortcut_blocked(cx) {
                    this.sidebar.update(cx, |sidebar, cx| {
                        sidebar.select_previous_session(window, cx);
                    });
                }
            }))
            .on_action(cx.listener(|this, action: &k::GridMove, window, cx| {
                if !this.shortcut_blocked(cx) {
                    this.sidebar.update(cx, |sidebar, cx| {
                        sidebar.grid_move(action.dx, action.dy, window, cx);
                    });
                }
            }))
            .on_action(cx.listener(|this, action: &k::GridSwap, window, cx| {
                if !this.shortcut_blocked(cx) {
                    this.sidebar.update(cx, |sidebar, cx| {
                        sidebar.grid_swap(action.dx, action.dy, window, cx);
                    });
                }
            }))
            .on_action(cx.listener(|this, action: &k::SelectSession, window, cx| {
                if !this.shortcut_blocked(cx) {
                    this.sidebar.update(cx, |sidebar, cx| {
                        sidebar.select_numbered_session(action.index, window, cx);
                    });
                }
            }))
            .on_action(
                cx.listener(|this, _: &k::JumpToWaitingSession, window, cx| {
                    if !this.shortcut_blocked(cx) {
                        this.sidebar
                            .update(cx, |sidebar, cx| sidebar.select_waiting_session(window, cx));
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &k::ToggleRailMode, window, cx| {
                if !this.shortcut_blocked(cx) {
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.toggle_tree_list(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::ToggleGrid, window, cx| {
                if !this.shortcut_blocked(cx) {
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.toggle_grid(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::ToggleZen, window, cx| {
                if !this.shortcut_blocked(cx) {
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.toggle_zen(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::NewHomeTerminal, window, cx| {
                if !this.shortcut_blocked(cx) {
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.add_terminal(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::CloseFocusedSession, window, cx| {
                if !this.shortcut_blocked(cx) {
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.request_close_focused(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::Settings, window, cx| {
                if !this.shortcut_blocked(cx) {
                    this.settings
                        .update(cx, |settings, cx| settings.open(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::ShortcutOverlay, window, cx| {
                if !this.shortcut_blocked(cx) {
                    this.settings
                        .update(cx, |settings, cx| settings.open_shortcuts(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::ZoomIn, _, cx| {
                this.set_zoom(crate::zoom::ZOOM_STEP, cx);
            }))
            .on_action(cx.listener(|this, _: &k::ZoomOut, _, cx| {
                this.set_zoom(-crate::zoom::ZOOM_STEP, cx);
            }))
            .on_action(cx.listener(|this, _: &k::ZoomReset, _, cx| {
                this.set_zoom(0.0, cx);
            }))
            .when(
                !self.sidebar.read(cx).is_zen() || self.settings.read(cx).is_open(),
                |root| {
                    root.child({
                        let grid =
                            self.sidebar.read(cx).is_grid() && !self.settings.read(cx).is_open();
                        let header = div()
                            .relative()
                            .flex_shrink_0()
                            .when(!grid, |header| {
                                header.absolute().top_0().left_0().w_full().h(rpx(APPBAR_H))
                            })
                            .child(self.header(window, cx))
                            .when(self.sidebar.read(cx).confirmation_open(), |header| {
                                header.child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .occlude()
                                        .bg(c::alpha(c::BG(), 0.4))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        }),
                                )
                            });
                        if grid {
                            header.into_any_element()
                        } else {
                            gpui::deferred(header).into_any_element()
                        }
                    })
                },
            )
            .child(div().flex_1().min_h_0().child(self.sidebar.clone()))
            .when(
                !self.sidebar.read(cx).is_zen() || self.settings.read(cx).is_open(),
                |root| root.child(self.statusbar.clone()),
            )
            .when(self.launcher.read(cx).is_open(), |root| {
                root.child(gpui::deferred(self.launcher.clone()))
            })
            .when(self.switcher_open, |root| {
                root.child(gpui::deferred(self.session_switcher(window, cx)))
            })
            .when(self.backend_choice_open, |root| {
                root.child(gpui::deferred(self.backend_choice(window, cx)))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn zen_shows_one_terminal_and_restores_the_previous_view(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
            window.dispatch_action(Box::new(k::ToggleZen), cx);
        });
        draw(cx);
        assert!(cx.debug_bounds("app-header").is_none());
        assert!(cx.debug_bounds("statusbar").is_none());
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        assert!(cx.debug_bounds("canvas-overview").is_some());
        cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
        draw(cx);
        assert!(cx.debug_bounds("app-header").is_some());
        assert!(cx.debug_bounds("statusbar").is_some());
        assert!(cx.debug_bounds("sidebar-rail").is_some());
        let id = cx.update(|_, cx| {
            let registry = shell.read(cx).runtime.read(cx).registry.clone();
            registry.update(cx, |registry, cx| {
                let id = registry.insert_meta(
                    "navigation".into(),
                    "/grove-shell-navigation-test".into(),
                    grove_core::agent::Agent::Terminal,
                );
                cx.notify();
                id
            })
        });
        draw(cx);
        cx.update(|window, cx| {
            let sidebar = shell.read(cx).sidebar.clone();
            sidebar.update(cx, |sidebar, cx| {
                sidebar.select_session_id(id, window, cx);
            });
            shell.update(cx, |shell, _| shell.backend_choice_open = true);
            window.dispatch_action(Box::new(k::ToggleZen), cx);
            assert!(!shell.read(cx).sidebar.read(cx).is_zen());
        });
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.cancel_backend_choice(window, cx);
            });
        });
        draw(cx);
        cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
        draw(cx);
        assert!(cx.debug_bounds("app-header").is_none());
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        assert!(cx.debug_bounds("terminal-header-1").is_none());
        assert!(cx.debug_bounds("terminal-pane-1").is_some());
        cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
        draw(cx);
        assert!(cx.debug_bounds("app-header").is_some());
        assert!(cx.debug_bounds("sidebar-rail").is_some());
        assert!(cx.debug_bounds("terminal-header-1").is_some());
        cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
            window.dispatch_action(Box::new(k::ToggleGrid), cx);
        });
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
        cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
        draw(cx);
        assert!(cx.debug_bounds("app-header").is_none());
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        assert!(cx.debug_bounds("terminal-pane-1").is_some());
        cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
        assert!(cx.debug_bounds("app-header").is_some());
        cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
        draw(cx);
        cx.update(|_, cx| {
            let registry = shell.read(cx).runtime.read(cx).registry.clone();
            registry.update(cx, |registry, cx| {
                registry.remove(id);
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_zen()));
        assert!(cx.debug_bounds("session-grid").is_none());
        assert!(cx.debug_bounds("canvas-overview").is_some());
        cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
    }

    #[gpui::test]
    fn empty_grid_zen_shows_one_overview_and_returns_to_grid(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
            window.dispatch_action(Box::new(k::ToggleGrid), cx);
        });
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
        cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
        draw(cx);
        assert!(cx.debug_bounds("app-header").is_none());
        assert!(cx.debug_bounds("canvas-overview").is_some());
        assert!(cx.debug_bounds("session-grid").is_none());
        cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
        assert!(cx.debug_bounds("app-header").is_some());
    }

    fn init(cx: &mut App) {
        gpui_component::init(cx);
        let project = grove_core::storage::Project {
            name: "navigation".into(),
            path: "/grove-shell-navigation-test".into(),
            scripts: grove_core::storage::ProjectScripts::default(),
            archived: false,
            worktree_dir: None,
        };
        cx.set_global(crate::settings::SettingsState::new(
            grove_core::storage::Store {
                projects: vec![project],
                tmux_enabled: Some(false),
                ..grove_core::storage::Store::default()
            },
        ));
        cx.set_global(crate::zoom::ZoomState::new(1.0));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(ThemeState::new(
            false,
            "tokyonight".into(),
            "tokyonight-day".into(),
        ));
    }

    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn backend_choice_waits_for_launch_then_back_restores_shortcuts(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            init(cx);
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .tmux_enabled = None;
        });
        let (shell, cx) = cx.add_window_view(Shell::new);
        assert!(!cx.update(|_, cx| shell.read(cx).backend_choice_open));
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.backend_choice_open = true;
                cx.notify();
            });
            // A requested choice keeps keyboard input inside the dialog.
            let handle = shell.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
            assert_eq!(
                window.focused(cx),
                Some(shell.read(cx).backend_choice_focus.clone())
            );
        });
        draw(cx);
        assert!(cx.debug_bounds("backend-choice-dialog").is_some());
        assert!(cx.debug_bounds("backend-choice-option-0").is_some());
        assert!(cx.debug_bounds("backend-choice-option-1").is_some());
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|_, cx| assert_eq!(shell.read(cx).backend_choice_index, 1));
        cx.update(|window, cx| {
            assert!(shell.read(cx).backend_choice_open);
            assert!(shell.read(cx).shortcut_blocked(cx));
            window.dispatch_action(Box::new(k::NewSession), cx);
            assert!(!shell.read(cx).launcher.read(cx).is_open());
        });
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|_, cx| {
            assert!(!shell.read(cx).backend_choice_open);
            assert!(!shell.read(cx).shortcut_blocked(cx));
        });
    }

    #[gpui::test]
    fn selecting_backend_does_not_start_or_save_before_confirmation(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            init(cx);
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .tmux_enabled = None;
        });
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                shell.backend_choice_open = true;
                shell.backend_choice_focus.focus(window, cx);
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("backend-choice-start").is_some());
        let tmux = cx.debug_bounds("backend-choice-option-1").unwrap().center();
        cx.simulate_mouse_down(tmux, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(tmux, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| {
            let shell = shell.read(cx);
            assert!(shell.backend_choice_open);
            assert!(shell.backend_selected_tmux);
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .tmux_enabled,
                None
            );
            assert!(shell.runtime.read(cx).registry.read(cx).is_empty());
        });
    }

    #[gpui::test]
    fn sidebar_collapse_restores_width_and_header_controls(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            init(cx);
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .sidebar_width = Some(300.0);
        });
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        assert_rail_alignment(cx, 300.0);
        let settings = cx.debug_bounds("sidebar-settings").unwrap();
        let toggle = cx.debug_bounds("sidebar-collapse-toggle").unwrap();
        assert!(toggle.left() >= settings.right());
        assert_eq!(toggle.center().y, settings.center().y);
        assert!(cx.debug_bounds("window-close").is_some());
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        draw(cx);
        let rail = cx.debug_bounds("sidebar-rail").unwrap();
        let header = cx.debug_bounds("header-rail-segment").unwrap();
        let canvas = cx.debug_bounds("sidebar-canvas").unwrap();
        assert_eq!(f32::from(rail.size.width), 52.0);
        assert_eq!(header.right(), rail.right());
        assert_eq!(canvas.left(), rail.right());
        assert!(cx.debug_bounds("sidebar-divider").is_none());
        for selector in [
            "window-close",
            "window-minimize",
            "window-fullscreen",
            "workspace-picker",
        ] {
            assert!(cx.debug_bounds(selector).is_none(), "{selector}");
        }
        let workspace_header = cx.debug_bounds("sidebar-workspace-header").unwrap();
        let settings = cx.debug_bounds("sidebar-settings").unwrap();
        assert!(settings.top() >= workspace_header.bottom());
        assert_eq!(
            cx.debug_bounds("sidebar-collapse-toggle")
                .unwrap()
                .center()
                .y,
            workspace_header.center().y
        );
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_width,
                Some(300.0)
            );
            assert!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_collapsed
            );
        });
        // The replacement Expand control retains focus and activates with Space.
        cx.simulate_keystrokes("space");
        draw(cx);
        assert_rail_alignment(cx, 300.0);
        assert!(cx.debug_bounds("window-close").is_some());
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert_eq!(
            f32::from(cx.debug_bounds("sidebar-rail").unwrap().size.width),
            52.0
        );
        cx.update(|window, cx| {
            shell
                .read(cx)
                .sidebar
                .clone()
                .update(cx, |sidebar, cx| sidebar.toggle_grid(window, cx));
        });
        draw(cx);
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        assert!(cx.debug_bounds("window-close").is_some());
        cx.update(|window, cx| {
            shell
                .read(cx)
                .sidebar
                .clone()
                .update(cx, |sidebar, cx| sidebar.toggle_grid(window, cx));
        });
        draw(cx);
        assert_eq!(
            f32::from(cx.debug_bounds("sidebar-rail").unwrap().size.width),
            52.0
        );
        assert!(cx.debug_bounds("window-close").is_none());
        // Workspace switching stays available as a compact icon and opens the full selector.
        let workspace = cx.debug_bounds("sidebar-workspaces").unwrap().center();
        cx.simulate_click(workspace, gpui::Modifiers::default());
        draw(cx);
        assert_rail_alignment(cx, 300.0);
        assert!(cx.debug_bounds("workspace-popup").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        let point = cx.debug_bounds("sidebar-collapse-toggle").unwrap().center();
        cx.simulate_click(point, gpui::Modifiers::default());
        draw(cx);
        cx.simulate_resize(gpui::size(gpui::px(500.0), gpui::px(600.0)));
        draw(cx);
        let point = cx.debug_bounds("projects-archive").unwrap().center();
        cx.simulate_click(point, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        let header = cx.debug_bounds("header-rail-segment").unwrap();
        for selector in ["window-close", "window-minimize", "window-fullscreen"] {
            assert!(
                header.contains(&cx.debug_bounds(selector).unwrap().center()),
                "{selector}"
            );
        }
    }

    #[gpui::test]
    fn sidebar_compact_grid_control_returns_from_grid_while_settings_open(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            init(cx);
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .sidebar_collapsed = true;
        });
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        cx.update(|window, cx| {
            shell
                .read(cx)
                .sidebar
                .clone()
                .update(cx, |sidebar, cx| sidebar.toggle_grid(window, cx));
            shell
                .read(cx)
                .settings
                .clone()
                .update(cx, |settings, cx| settings.open(window, cx));
        });
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
        assert!(cx.debug_bounds("sidebar-rail").is_some());
        let point = cx.debug_bounds("sidebar-grid").unwrap().center();
        cx.simulate_click(point, gpui::Modifiers::default());
        draw(cx);
        assert!(!cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
        assert_eq!(
            f32::from(cx.debug_bounds("sidebar-rail").unwrap().size.width),
            52.0
        );
    }

    #[gpui::test]
    fn sidebar_initial_collapsed_workspace_picker_uses_visible_anchor(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            init(cx);
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .sidebar_collapsed = true;
        });
        let (_, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        let workspace = cx.debug_bounds("sidebar-workspaces").unwrap().center();
        cx.simulate_click(workspace, gpui::Modifiers::default());
        draw(cx);
        let popup = cx.debug_bounds("workspace-popup").unwrap();
        let picker = cx.debug_bounds("workspace-picker").unwrap();
        assert_eq!(popup.left(), picker.left());
        assert!(popup.top() >= picker.bottom());
    }

    fn assert_rail_alignment(cx: &mut gpui::VisualTestContext, expected: f32) {
        let rail = cx.debug_bounds("sidebar-rail").expect("sidebar rail");
        let divider = cx.debug_bounds("sidebar-divider").expect("resize hit zone");
        let header = cx
            .debug_bounds("header-rail-segment")
            .expect("header rail segment");
        let canvas = cx.debug_bounds("sidebar-canvas").expect("canvas");
        assert!((f32::from(rail.size.width) - expected).abs() <= 1.0);
        assert_eq!(header.right(), rail.right());
        assert_eq!(canvas.left(), rail.right());
        assert!((f32::from(divider.center().x - rail.right())).abs() <= 1.0);
    }

    #[gpui::test]
    fn sidebar_drag_clamps_persists_on_release_and_survives_reconstruction(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init);
        let (_, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        assert_rail_alignment(cx, 260.0);
        let edge = cx.debug_bounds("sidebar-divider").unwrap().center();
        cx.simulate_mouse_down(edge, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(edge, MouseButton::Left, gpui::Modifiers::default());
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_width,
                None
            );
        });

        cx.simulate_mouse_down(edge, MouseButton::Left, gpui::Modifiers::default());
        let far_right = gpui::point(gpui::px(1100.0), edge.y);
        cx.simulate_mouse_move(far_right, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert_rail_alignment(cx, 640.0);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_width,
                None
            );
        });
        cx.simulate_mouse_up(far_right, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_width,
                Some(640.0)
            );
        });

        let edge = cx.debug_bounds("sidebar-divider").unwrap().center();
        cx.simulate_mouse_down(edge, MouseButton::Left, gpui::Modifiers::default());
        let far_left = gpui::point(gpui::px(30.0), edge.y);
        cx.simulate_mouse_move(far_left, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert_rail_alignment(cx, 220.0);
        cx.simulate_mouse_up(far_left, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_width,
                Some(220.0)
            );
        });

        let (_, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        assert_rail_alignment(cx, 220.0);
        let list = cx.debug_bounds("sidebar-view").unwrap().center();
        cx.simulate_mouse_down(list, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(list, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert_rail_alignment(cx, 220.0);
        let grid = cx.debug_bounds("sidebar-grid").unwrap().center();
        cx.simulate_mouse_down(grid, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(grid, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("sidebar-divider").is_none());
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        assert_eq!(
            cx.debug_bounds("sidebar-canvas").unwrap().left(),
            gpui::px(0.0)
        );
        let grid = cx.debug_bounds("sidebar-grid").unwrap().center();
        cx.simulate_mouse_down(grid, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(grid, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert_rail_alignment(cx, 220.0);
    }

    #[gpui::test]
    fn sidebar_narrow_cap_and_double_click_reset_keep_saved_preference(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init);
        cx.update(|cx| {
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .sidebar_width = Some(500.0);
        });
        let (_, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(gpui::px(500.0), gpui::px(800.0)));
        draw(cx);
        assert_rail_alignment(cx, 200.0);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_width,
                Some(500.0)
            );
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        assert_rail_alignment(cx, 500.0);

        cx.update(|_, cx| cx.global_mut::<crate::zoom::ZoomState>().zoom = 1.5);
        cx.simulate_resize(gpui::size(gpui::px(1500.0), gpui::px(800.0)));
        draw(cx);
        assert_rail_alignment(cx, 750.0);
        let edge = cx.debug_bounds("sidebar-divider").unwrap().center();
        cx.simulate_mouse_down(edge, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(edge, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_event(gpui::MouseDownEvent {
            position: edge,
            modifiers: gpui::Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        cx.simulate_event(gpui::MouseUpEvent {
            position: edge,
            modifiers: gpui::Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
        });
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_width,
                Some(260.0)
            );
        });
        assert_rail_alignment(cx, 390.0);
    }

    #[gpui::test]
    fn project_marker_and_settings_follow_navigation_cluster(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (_, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let project = cx.debug_bounds("project-0").expect("project row");
        let marker = cx.debug_bounds("project-no-git-0").expect("non-git marker");
        assert!(project.contains(&marker.center()));
        let grid = cx.debug_bounds("sidebar-grid").expect("grid control");
        let settings = cx
            .debug_bounds("sidebar-settings")
            .expect("settings indicator");
        let workspace_header = cx
            .debug_bounds("sidebar-workspace-header")
            .expect("workspace header");
        let workspace_picker = cx
            .debug_bounds("workspace-picker")
            .expect("workspace picker");
        assert_eq!(settings.size, grid.size);
        assert!(workspace_header.contains(&settings.center()));
        assert_eq!(settings.center().y, workspace_picker.center().y);
        assert!(settings.bottom() < grid.top());
        assert!(cx.debug_bounds("settings").is_none());
        cx.simulate_mouse_down(grid.center(), MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(grid.center(), MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        let header = cx.debug_bounds("app-header").unwrap();
        let canvas = cx.debug_bounds("sidebar-canvas").unwrap();
        assert_eq!(canvas.top(), header.bottom());
        assert_eq!(f32::from(header.size.height), APPBAR_H);
        let grid = cx
            .debug_bounds("sidebar-grid")
            .expect("grid control in app header");
        assert!(header.contains(&grid.center()));
        assert_eq!(grid.center().y, header.center().y);
        assert!(cx.debug_bounds("sidebar-settings").is_none());
        assert!(cx.debug_bounds("settings").is_none());
    }

    #[gpui::test]
    fn ordinary_sidebar_controls_traverse_forward_and_backward(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (_, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let project = cx.debug_bounds("project-0").expect("project row").center();
        cx.simulate_mouse_down(project, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(project, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        let initial = cx.update(|window, cx| window.focused(cx).expect("project focus"));
        cx.simulate_keystrokes("tab");
        draw(cx);
        let next = cx.update(|window, cx| window.focused(cx).expect("next control"));
        assert_ne!(initial, next);
        cx.simulate_keystrokes("shift-tab");
        draw(cx);
        cx.update(|window, cx| assert_eq!(window.focused(cx), Some(initial)));
    }

    #[gpui::test]
    fn sidebar_workspace_control_traverses_and_activates(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(gpui::px(500.0), gpui::px(600.0)));
        draw(cx);
        let rail = cx.debug_bounds("sidebar-rail").expect("sidebar rail");
        let segment = cx
            .debug_bounds("header-rail-segment")
            .expect("header rail segment");
        assert_eq!(segment.right(), rail.right());
        assert_eq!(segment.top(), rail.top());
        assert_eq!(
            cx.debug_bounds("canvas-section-header").unwrap().bottom(),
            segment.bottom()
        );
        let workspace_header = cx.debug_bounds("sidebar-workspace-header").unwrap();
        assert_eq!(segment.bottom(), workspace_header.top());
        assert_eq!(
            cx.debug_bounds("canvas-section-header").unwrap().top(),
            rail.top()
        );
        let header = cx
            .debug_bounds("sidebar-workspace-header")
            .expect("workspace header");
        let trigger = cx
            .debug_bounds("workspace-picker")
            .expect("workspace selector");
        assert!(rail.contains(&trigger.center()));
        assert!(header.contains(&trigger.center()));
        let picker = cx.update(|window, cx| {
            let picker = shell.read(cx).workspaces.focus_handle(cx);
            picker.focus(window, cx);
            picker
        });
        cx.simulate_keystrokes("shift-tab");
        draw(cx);
        cx.update(|window, cx| assert_ne!(window.focused(cx), Some(picker.clone())));
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| assert_eq!(window.focused(cx), Some(picker.clone())));
        for key in ["enter", "space"] {
            cx.simulate_keystrokes(key);
            draw(cx);
            let popup = cx.debug_bounds("workspace-popup").expect("workspace popup");
            assert_eq!(popup.left(), trigger.left());
            assert!(popup.bottom() > header.bottom());
            assert!(
                popup.right() > rail.right(),
                "menu may extend beyond the rail"
            );
            cx.simulate_keystrokes("escape");
            draw(cx);
            assert!(cx.debug_bounds("workspace-popup").is_none());
        }
    }

    #[gpui::test]
    fn worktree_inputs_tab_in_order_then_reach_cancel(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            init(cx);
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .projects[0]
                .path = env!("CARGO_MANIFEST_DIR").into();
        });
        let (_, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let menu = cx
            .debug_bounds("project-menu-0")
            .expect("project menu")
            .center();
        cx.simulate_mouse_down(menu, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(menu, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        cx.simulate_keystrokes("down enter");
        draw(cx);
        let mut fields = Vec::new();
        for selector in [
            "worktree-name-field",
            "worktree-branch-field",
            "worktree-base-field",
        ] {
            let bounds = cx.debug_bounds(selector).expect("worktree field");
            let point = gpui::point(bounds.center().x, bounds.bottom() - gpui::px(SPACE_LG));
            cx.simulate_mouse_down(point, MouseButton::Left, gpui::Modifiers::default());
            cx.simulate_mouse_up(point, MouseButton::Left, gpui::Modifiers::default());
            draw(cx);
            fields.push(cx.update(|window, cx| window.focused(cx).expect("input focus")));
        }
        assert_ne!(fields[0], fields[1]);
        assert_ne!(fields[1], fields[2]);
        cx.update(|window, cx| fields[0].focus(window, cx));
        for expected in &fields[1..] {
            cx.simulate_keystrokes("tab");
            draw(cx);
            cx.update(|window, cx| assert_eq!(window.focused(cx), Some(expected.clone())));
        }
        cx.simulate_keystrokes("shift-tab");
        draw(cx);
        cx.update(|window, cx| assert_eq!(window.focused(cx), Some(fields[1].clone())));
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| assert_eq!(window.focused(cx), Some(fields[2].clone())));
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| {
            assert_ne!(
                window.focused(cx),
                Some(fields[2].clone()),
                "Tab leaves base for Cancel"
            );
        });
        cx.simulate_keystrokes("enter");
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").unwrap(),
        });
        draw(cx);
        assert!(
            cx.debug_bounds("worktree-name-field").is_none(),
            "Cancel closes the worktree editor; disabled Create is skipped"
        );
    }

    #[gpui::test]
    fn statusbar_spans_narrow_window_and_tracks_toast_lifecycle(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(
            gpui::px(crate::WINDOW_MIN_W),
            gpui::px(crate::WINDOW_MIN_H),
        ));
        draw(cx);
        let footer = cx.debug_bounds("statusbar").expect("persistent footer");
        assert_eq!(
            f32::from(footer.size.height),
            super::super::statusbar::STATUS_H
        );
        assert_eq!(f32::from(footer.size.width), crate::WINDOW_MIN_W);
        assert_eq!(f32::from(footer.bottom()), crate::WINDOW_MIN_H);
        cx.update(|_, cx| {
            shell
                .read(cx)
                .runtime
                .read(cx)
                .toast
                .clone()
                .update(cx, |toast, cx| toast.set_error("Example failure", cx));
        });
        draw(cx);
        let toast = cx.debug_bounds("status-toast").expect("error toast");
        assert!(toast.right() <= footer.right());
        assert!(toast.left() >= footer.left());
        cx.executor()
            .advance_clock(crate::entities::toast::Toast::ttl(
                crate::entities::toast::ToastKind::Error,
            ));
        draw(cx);
        assert!(cx.debug_bounds("status-toast").is_none());
    }

    #[gpui::test]
    fn footer_session_scope_changes_before_sidebar_render(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        cx.update(|_, cx| {
            let sidebar = shell.read(cx).sidebar.clone();
            let registry = shell.read(cx).runtime.read(cx).registry.clone();
            let id = registry.update(cx, |registry, cx| {
                let id = registry.insert_meta(
                    "navigation".into(),
                    "/grove-shell-navigation-test".into(),
                    grove_core::agent::Agent::Terminal,
                );
                cx.notify();
                id
            });
            assert_eq!(
                sidebar.read(cx).active_canvas_sessions(cx),
                vec![(id, false)]
            );
            cx.update_global::<crate::settings::SettingsState, _>(|settings, _| {
                settings.store.workspaces.create("Other").unwrap();
            });
            // No draw or artificial entity notification between changing the
            // active workspace and the footer's scope read.
            assert!(sidebar.read(cx).active_canvas_sessions(cx).is_empty());
            cx.update_global::<crate::settings::SettingsState, _>(|settings, _| {
                settings.store.workspaces.select(1);
            });
            assert_eq!(
                sidebar.read(cx).active_canvas_sessions(cx),
                vec![(id, false)]
            );
        });
    }

    struct TerminalFixture {
        terminal: Entity<super::super::terminal_view::TerminalView>,
        session: Entity<crate::entities::terminal_session::TerminalSession>,
    }

    impl Render for TerminalFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .on_key_down(traverse_unhandled_tab)
                .child(div().id("other-control").tab_index(0).child("Other"))
                .child(self.terminal.clone())
        }
    }

    #[gpui::test]
    fn terminal_consumes_tab_before_shell_traversal(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (fixture, cx) = cx.add_window_view(|_, cx| {
            // A failed native spawn exercises the real terminal input routing
            // without creating a persistent shell or depending on tmux.
            let session = cx.new(|cx| {
                crate::entities::terminal_session::TerminalSession::spawn_script("\0", "/", cx)
            });
            assert!(
                session.read(cx).spawn_error().is_some(),
                "NUL script must fail before any PTY reader starts"
            );
            let terminal =
                cx.new(|cx| super::super::terminal_view::TerminalView::new(session.clone(), cx));
            TerminalFixture { terminal, session }
        });
        draw(cx);
        cx.update(|window, cx| {
            let fixture = fixture.read(cx);
            fixture.terminal.focus_handle(cx).focus(window, cx);
        });
        let before_key = std::time::Instant::now();
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| {
            let fixture = fixture.read(cx);
            assert!(fixture.terminal.focus_handle(cx).is_focused(window));
            assert!(
                fixture
                    .session
                    .read(cx)
                    .input_age()
                    .is_some_and(|age| age <= before_key.elapsed()),
                "Tab reached TerminalSession::send"
            );
        });
        let before_control_c = std::time::Instant::now();
        cx.simulate_keystrokes("ctrl-c");
        draw(cx);
        cx.update(|window, cx| {
            let fixture = fixture.read(cx);
            assert!(fixture.terminal.focus_handle(cx).is_focused(window));
            assert!(
                fixture
                    .session
                    .read(cx)
                    .input_age()
                    .is_some_and(|age| age <= before_control_c.elapsed()),
                "Ctrl+C reached TerminalSession::send"
            );
        });
    }

    #[gpui::test]
    fn header_launcher_opens_palette_and_selected_canvas_stays_aligned(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init);
        let path = env!("CARGO_MANIFEST_DIR").to_string();
        cx.update(|cx| {
            let store = &mut cx.global_mut::<crate::settings::SettingsState>().store;
            store.projects[0].name = "navigation".into();
            store.projects[0].path = path.clone();
            store.recent_launches = vec![grove_core::storage::RecentLaunch {
                project: "navigation".into(),
                wt_path: path.clone(),
                agent: grove_core::agent::Agent::Terminal,
            }];
        });
        let (shell, cx) = cx.add_window_view(Shell::new);
        for width in [1280.0, 320.0] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(640.0)));
            draw(cx);
            let control = cx
                .debug_bounds("header-new-session")
                .expect("new session control");
            let header = cx.debug_bounds("app-header").expect("header");
            assert_eq!(control.right() + gpui::px(SPACE_2XL), header.right());
            assert!(control.left() >= cx.debug_bounds("sidebar-rail").expect("rail").right());
        }
        let control = cx.debug_bounds("header-new-session").unwrap().center();
        cx.simulate_mouse_down(control, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(control, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| assert!(shell.read(cx).launcher.read(cx).is_open()));
        assert!(cx.debug_bounds("launcher-row-0").is_some());
        cx.simulate_keystrokes("tab");
        draw(cx);
        assert!(cx.debug_bounds("launcher-agent-0").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|_, cx| assert!(!shell.read(cx).launcher.read(cx).is_open()));

        // Exercise the selected canvas with a registered session whose invalid
        // script cannot start a PTY reader. A live reader wakes GPUI's test
        // scheduler from another thread and makes this visual test nondeterministic.
        let selected = cx.update(|window, cx| {
            let session = cx.new(|cx| {
                crate::entities::terminal_session::TerminalSession::spawn_script("\0", &path, cx)
            });
            let registry = shell.read(cx).runtime.read(cx).registry.clone();
            let selected = registry.update(cx, |registry, cx| {
                let id = registry.insert_meta(
                    "navigation".into(),
                    path.clone(),
                    grove_core::agent::Agent::Terminal,
                );
                registry.attach(id, session, None);
                cx.notify();
                id
            });
            let sidebar = shell.read(cx).sidebar.clone();
            sidebar.update(cx, |sidebar, cx| {
                sidebar.select_session_id(selected, window, cx);
            });
            selected
        });
        draw(cx);
        cx.update(|_, cx| {
            let shell = shell.read(cx);
            assert_eq!(shell.sidebar.read(cx).selected_session(), Some(selected));
            assert_eq!(
                shell.runtime.read(cx).state.read(cx).active_session(),
                Some(selected)
            );
        });
        let selector: &'static str =
            Box::leak(format!("terminal-header-{}", selected.raw()).into_boxed_str());
        let terminal_header = cx.debug_bounds(selector).expect("selected canvas header");
        let canvas = cx.debug_bounds("sidebar-canvas").expect("canvas");
        assert_eq!(terminal_header.left(), canvas.left());
        assert_eq!(terminal_header.right(), canvas.right());
    }

    #[gpui::test]
    fn settings_shortcut_opens_panel_and_escape_restores_focus(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        cx.update(|cx| cx.bind_keys(k::shell_bindings()));
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let prior = cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
            focus
        });
        cx.simulate_keystrokes(&format!("{},", k::platform_mod_prefix()));
        draw(cx);
        assert!(cx.debug_bounds("settings-panel").is_some());
        let canvas = cx.debug_bounds("sidebar-canvas").expect("sidebar canvas");
        let settings = cx.debug_bounds("settings-panel").expect("settings page");
        assert_eq!(settings, canvas);
        assert!(cx.debug_bounds("settings-overlay").is_none());
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            assert!(prior.is_focused(window));
            assert!(!shell.read(cx).settings.read(cx).is_open());
        });
    }

    #[gpui::test]
    fn settings_uses_canvas_from_grid_and_zen(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
            window.dispatch_action(Box::new(k::ToggleGrid), cx);
        });
        draw(cx);
        cx.update(|window, cx| window.dispatch_action(Box::new(k::Settings), cx));
        draw(cx);
        assert!(cx.debug_bounds("sidebar-rail").is_some());
        assert_eq!(
            cx.debug_bounds("settings-panel"),
            cx.debug_bounds("sidebar-canvas")
        );
        cx.simulate_resize(gpui::size(gpui::px(500.0), gpui::px(640.0)));
        draw(cx);
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        assert_eq!(
            cx.debug_bounds("settings-panel"),
            cx.debug_bounds("sidebar-canvas")
        );
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
        cx.simulate_resize(gpui::size(gpui::px(1920.0), gpui::px(1080.0)));
        draw(cx);
        cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
        draw(cx);
        assert!(cx.debug_bounds("app-header").is_none());
        cx.update(|window, cx| window.dispatch_action(Box::new(k::Settings), cx));
        draw(cx);
        assert!(cx.debug_bounds("app-header").is_some());
        assert!(cx.debug_bounds("sidebar-rail").is_some());
        assert_eq!(
            cx.debug_bounds("settings-panel"),
            cx.debug_bounds("sidebar-canvas")
        );
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_zen()));
        assert!(cx.debug_bounds("app-header").is_none());
    }

    #[gpui::test]
    fn switch_session_shows_scoped_picker_and_escape_restores_focus(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let prior = cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
            focus
        });
        cx.update(|window, cx| window.dispatch_action(Box::new(crate::keymap::SwitchSession), cx));
        draw(cx);
        assert!(cx.debug_bounds("session-switcher").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            assert!(prior.is_focused(window));
            assert!(!shell.read(cx).switcher_open);
        });
    }

    #[gpui::test]
    fn switch_session_uses_visible_order_and_selects_existing_process(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        let (first, second) = cx.update(|_, cx| {
            let registry = shell.read(cx).runtime.read(cx).registry.clone();
            registry.update(cx, |registry, cx| {
                let first = registry.insert_meta(
                    "navigation".into(),
                    "/grove-shell-navigation-test".into(),
                    grove_core::agent::Agent::Terminal,
                );
                let second = registry.insert_meta(
                    "navigation".into(),
                    "/grove-shell-navigation-test".into(),
                    grove_core::agent::Agent::Terminal,
                );
                cx.notify();
                (first, second)
            })
        });
        draw(cx);
        let targets =
            cx.update(|_, cx| shell.read(cx).sidebar.read(cx).visible_session_targets(cx));
        assert_eq!(targets.len(), 2);
        assert!(targets.iter().any(|(id, _)| *id == first));
        assert!(targets.iter().any(|(id, _)| *id == second));
        cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
            window.dispatch_action(Box::new(k::SwitchSession), cx);
        });
        draw(cx);
        assert!(cx.debug_bounds("session-switcher-row-1").is_some());
        cx.simulate_keystrokes("down enter");
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                shell.read(cx).sidebar.read(cx).selected_session(),
                Some(targets[1].0)
            );
            assert!(!shell.read(cx).switcher_open);
            assert_eq!(shell.read(cx).runtime.read(cx).registry.read(cx).len(), 2);
        });
    }

    #[gpui::test]
    fn switcher_groups_workspaces_and_current_workspace_sessions(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.update(|_, cx| {
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .workspaces
                .create("Other")
                .unwrap();
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .workspaces
                .select(1);
        });
        let session = cx.update(|_, cx| {
            let registry = shell.read(cx).runtime.read(cx).registry.clone();
            registry.update(cx, |registry, cx| {
                let id = registry.insert_meta(
                    "navigation".into(),
                    "/grove-shell-navigation-test".into(),
                    Agent::Terminal,
                );
                cx.notify();
                id
            })
        });
        draw(cx);
        cx.update(|_, cx| {
            let shell = shell.read(cx);
            let rows = shell.switcher_rows(cx);
            assert_eq!(rows.len(), 3);
            assert_eq!(rows[0].target, SwitchTarget::Workspace(1));
            assert!(rows[0].current);
            assert_eq!(rows[1].target, SwitchTarget::Workspace(2));
            assert!(!rows[1].current);
            assert_eq!(rows[2].target, SwitchTarget::Session(session));
            assert_eq!(switcher_default_index(&rows), 2);
        });
    }

    #[gpui::test]
    fn switcher_search_matches_names_and_session_context_case_insensitively(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.update(|_, cx| {
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .workspaces
                .create("Other")
                .unwrap();
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .workspaces
                .select(1);
        });
        cx.update(|_, cx| {
            let registry = shell.read(cx).runtime.read(cx).registry.clone();
            registry.update(cx, |registry, cx| {
                registry.insert_meta(
                    "navigation".into(),
                    "/grove-shell-navigation-test".into(),
                    Agent::Terminal,
                );
                cx.notify();
            });
        });
        draw(cx);
        cx.update(|_, cx| {
            shell.update(cx, |shell, cx| {
                shell.switcher_query = "OTHER".into();
                let rows = shell.switcher_rows(cx);
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].target, SwitchTarget::Workspace(2));
                assert_eq!(switcher_default_index(&rows), 0);
                shell.switcher_query = "navigation-test".into();
                let rows = shell.switcher_rows(cx);
                assert_eq!(rows.len(), 1);
                assert!(matches!(rows[0].target, SwitchTarget::Session(_)));
                shell.switcher_query = "unmatched".into();
                assert!(shell.switcher_rows(cx).is_empty());
            });
        });
    }

    #[gpui::test]
    fn switcher_search_enter_activates_workspace(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.update(|_, cx| {
            let workspaces = &mut cx
                .global_mut::<crate::settings::SettingsState>()
                .store
                .workspaces;
            workspaces.create("Other").unwrap();
            workspaces.select(1);
        });
        draw(cx);
        cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
            window.dispatch_action(Box::new(k::SwitchSession), cx);
        });
        draw(cx);
        cx.simulate_input("Other");
        draw(cx);
        cx.update(|_, cx| {
            let shell = shell.read(cx);
            assert_eq!(shell.switcher_query, "Other");
            assert_eq!(shell.switcher_rows(cx).len(), 1);
            assert_eq!(
                shell.switcher_rows(cx)[0].target,
                SwitchTarget::Workspace(2)
            );
        });
        cx.simulate_keystrokes("enter");
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .workspaces
                    .active,
                2,
                "error={:?}",
                shell.read(cx).switcher_error
            );
            assert!(!shell.read(cx).switcher_open);
        });
    }

    #[gpui::test]
    fn grid_workspace_switch_focuses_first_destination_tile(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        let destination_path = "/grove-shell-grid-destination".to_string();
        cx.update(|_, cx| {
            let store = &mut cx.global_mut::<crate::settings::SettingsState>().store;
            store.projects.push(grove_core::storage::Project {
                name: "destination".into(),
                path: destination_path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            });
            store.workspaces.create("Other").unwrap();
            store.assign_project_to_active_workspace(&destination_path);
            store.workspaces.select(1);
        });
        let (prior, first, second) = cx.update(|_, cx| {
            let registry = shell.read(cx).runtime.read(cx).registry.clone();
            let prior_session = cx.new(|cx| {
                crate::entities::terminal_session::TerminalSession::spawn_script(
                    "\0",
                    "/grove-shell-navigation-test",
                    cx,
                )
            });
            let first_session = cx.new(|cx| {
                crate::entities::terminal_session::TerminalSession::spawn_script(
                    "\0",
                    &destination_path,
                    cx,
                )
            });
            let second_session = cx.new(|cx| {
                crate::entities::terminal_session::TerminalSession::spawn_script(
                    "\0",
                    &destination_path,
                    cx,
                )
            });
            registry.update(cx, |registry, cx| {
                let prior = registry.insert_meta(
                    "navigation".into(),
                    "/grove-shell-navigation-test".into(),
                    Agent::Terminal,
                );
                registry.attach(prior, prior_session, None);
                let first = registry.insert_meta(
                    "destination".into(),
                    destination_path.clone(),
                    Agent::Terminal,
                );
                registry.attach(first, first_session, None);
                let second = registry.insert_meta(
                    "destination".into(),
                    destination_path.clone(),
                    Agent::Terminal,
                );
                registry.attach(second, second_session, None);
                cx.notify();
                (prior, first, second)
            })
        });
        cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
            window.dispatch_action(Box::new(k::ToggleGrid), cx);
        });
        draw(cx);
        let prior_focus = cx.update(|_, cx| {
            shell
                .read(cx)
                .sidebar
                .read(cx)
                .canvas_terminal_focus(prior, false, cx)
                .expect("prior tile")
        });
        cx.update(|window, cx| prior_focus.focus(window, cx));
        cx.update(|window, cx| window.dispatch_action(Box::new(k::SwitchSession), cx));
        draw(cx);
        cx.simulate_input("Other");
        draw(cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        let first_focus = cx.update(|window, cx| {
            let sidebar = shell.read(cx).sidebar.clone();
            let sidebar = sidebar.read(cx);
            assert!(sidebar.is_grid());
            assert_eq!(
                sidebar.active_canvas_sessions(cx),
                vec![(first, false), (second, false)]
            );
            let first_focus = sidebar
                .canvas_terminal_focus(first, false, cx)
                .expect("first destination tile");
            assert!(first_focus.is_focused(window));
            assert!(!prior_focus.is_focused(window));
            first_focus
        });
        cx.update(|window, cx| window.dispatch_action(Box::new(k::SwitchSession), cx));
        draw(cx);
        cx.simulate_input("Grove");
        draw(cx);
        cx.simulate_keystrokes("up enter");
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .workspaces
                    .active,
                1
            );
        });
        cx.update(|window, cx| window.dispatch_action(Box::new(k::SwitchSession), cx));
        draw(cx);
        cx.simulate_input("Other");
        draw(cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        cx.update(|window, _| {
            assert!(
                first_focus.is_focused(window),
                "mounted first tile must regain focus"
            );
        });
        cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .workspaces
                .select(1);
        });
        draw(cx);
        cx.update(|window, _| {
            assert!(
                prior_focus.is_focused(window),
                "Grid focus should also follow workspace changes outside the picker"
            );
        });
    }

    #[gpui::test]
    fn empty_switcher_keeps_workspace_and_new_session_action_visible(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
            window.dispatch_action(Box::new(k::SwitchSession), cx);
        });
        draw(cx);
        assert!(cx.debug_bounds("session-switcher-row-0").is_some());
        cx.update(|_, cx| {
            assert!(shell
                .read(cx)
                .sidebar
                .read(cx)
                .visible_session_targets(cx)
                .is_empty());
        });
        assert!(cx.debug_bounds("session-switcher-new-session").is_some());
        cx.update(|_, cx| {
            let shell = shell.read(cx);
            assert_eq!(shell.switcher_rows(cx).len(), 1);
            assert_eq!(shell.switcher_index, 0);
        });
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| {
            assert!(shell.read(cx).switcher_new_session_focus.is_focused(window));
        });
        cx.simulate_keystrokes("shift-tab");
        cx.update(|window, cx| {
            let input = shell.read(cx).switcher_input.clone();
            assert!(input.read(cx).focus_handle(cx).is_focused(window));
        });
        cx.simulate_keystrokes("tab");
        cx.simulate_keystrokes("enter");
        draw(cx);
        cx.update(|_, cx| {
            let shell = shell.read(cx);
            assert!(!shell.switcher_open);
            assert!(shell.launcher.read(cx).is_open());
        });
    }

    #[gpui::test]
    fn switcher_panel_fits_small_window(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(gpui::px(320.0), gpui::px(200.0)));
        cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
            window.dispatch_action(Box::new(k::SwitchSession), cx);
        });
        draw(cx);
        let panel = cx.debug_bounds("session-switcher").expect("switcher panel");
        assert!(f32::from(panel.left()) >= 0.0);
        assert!(f32::from(panel.right()) <= 320.0);
        assert!(f32::from(panel.bottom()) <= 200.0);
        assert!(cx.debug_bounds("session-switcher-list").is_some());
    }

    #[gpui::test]
    fn settings_control_opens_panel(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let point = cx.debug_bounds("sidebar-settings").unwrap().center();
        cx.simulate_mouse_down(point, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(point, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| assert!(shell.read(cx).settings.read(cx).is_open()));
    }

    #[gpui::test]
    fn palette_setting_command_opens_settings_panel(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.update(|window, cx| {
            let launcher = shell.read(cx).launcher.clone();
            launcher.update(cx, |launcher, cx| launcher.open(window, cx));
        });
        draw(cx);
        cx.simulate_input("appearance");
        draw(cx);
        assert!(cx.debug_bounds("launcher-row-0").is_some());
        cx.simulate_keystrokes("enter");
        draw(cx);
        cx.update(|_, cx| {
            assert!(!shell.read(cx).launcher.read(cx).is_open());
            assert!(shell.read(cx).settings.read(cx).is_open());
        });
    }

    #[gpui::test]
    fn zoom_shortcut_persists_the_same_value_as_the_settings_panel(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
            window.dispatch_action(Box::new(k::ZoomIn), cx);
        });
        cx.update(|_, cx| {
            let zoom = cx.global::<crate::zoom::ZoomState>().zoom;
            assert_eq!(zoom, 1.1);
            assert_eq!(
                cx.global::<crate::settings::SettingsState>().store.ui_zoom,
                Some(zoom)
            );
        });
    }

    #[gpui::test]
    fn settings_archived_event_opens_sidebar_project_panel(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        cx.update(|window, cx| {
            let settings = shell.read(cx).settings.clone();
            settings.update(cx, |settings, cx| settings.open(window, cx));
        });
        draw(cx);
        cx.update(|window, cx| {
            let settings = shell.read(cx).settings.clone();
            settings.update(cx, |settings, cx| {
                settings.close(window, cx);
                cx.emit(SettingsPanelEvent::OpenArchivedProjects);
            });
        });
        draw(cx);
        cx.update(|_, cx| assert!(!shell.read(cx).settings.read(cx).is_open()));
        assert!(cx.debug_bounds("project-panel").is_some());
    }
}
