//! Grove's app-owned header and the empty canvas for the UI rebuild.
use super::components::{form_action, header_control_with_label_tooltip};
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

const TRAFFIC_CONTROL_W: f32 = 22.0;
const TRAFFIC_LIGHT_D: f32 = 12.0;

const SWITCHER_TOP: f32 = APPBAR_TOTAL_H + SPACE_3XL * 2.0;
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

actions!(
    shell,
    [
        Quit,
        CloseWindow,
        PreviewReleaseHighlights,
        ToggleFullscreen
    ]
);

/// Bubble only unconsumed navigation keys. Mounting component Root would also
/// install its Copy action context, which can steal PTY Ctrl+C. TerminalView
/// consumes Tab as input first; open menus and confirmations trap it in capture.
pub(crate) fn traverse_unhandled_tab(
    event: &gpui::KeyDownEvent,
    window: &mut Window,
    cx: &mut App,
) {
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
    highlights: Option<Entity<super::release_highlights::ReleaseCarousel>>,
    highlights_events: Option<gpui::Subscription>,
    highlights_startup_checked: bool,
    highlights_startup_scheduled: bool,
    focus: FocusHandle,
    header_focus: std::collections::HashMap<&'static str, FocusHandle>,
    runtime: Entity<Runtime>,
    workspaces: Entity<super::workspace_manager::WorkspaceManager>,
    sidebar: Entity<super::sidebar::Sidebar>,
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
    backend_choice_source: gpui::Bounds<gpui::Pixels>,
    backend_choice_focus: FocusHandle,
    backend_choice_return_focus: Option<FocusHandle>,
    backend_choice_index: usize,
    backend_selected_tmux: bool,
    backend_choice_error: Option<String>,
    window_observers: Option<Vec<gpui::Subscription>>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        #[cfg(target_os = "macos")]
        cx.on_release(|_, cx| {
            crate::app_menus::refresh(crate::app_menus::MenuState::default(), cx);
        })
        .detach();
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
        let toast = runtime.read(cx).toast.clone();
        cx.observe(&toast, |_, _, cx| cx.notify()).detach();
        let provider_updates = runtime.read(cx).provider_updates.clone();
        cx.observe(&provider_updates, |_, _, cx| cx.notify())
            .detach();
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
                    SettingsPanelEvent::OpenHighlights { debug } => {
                        this.settings
                            .update(cx, |panel, cx| panel.close(window, cx));
                        this.open_highlights(*debug, window, cx);
                    }
                },
            );
        let sidebar_events =
            cx.subscribe_in(&sidebar, window, |this, _, event, window, cx| match event {
                super::sidebar::SidebarEvent::NewSessionRequested => {
                    this.open_session_picker(window, cx)
                }
                super::sidebar::SidebarEvent::NewMultiProjectSessionRequested => {
                    if !this.shortcut_blocked_in(window, cx) {
                        this.sidebar
                            .update(cx, super::sidebar::Sidebar::dismiss_project_context);
                        this.launcher
                            .update(cx, |launcher, cx| launcher.open_multi_project(window, cx));
                    }
                }
                super::sidebar::SidebarEvent::SettingsRequested => {
                    this.sidebar
                        .update(cx, super::sidebar::Sidebar::dismiss_project_context);
                    this.settings
                        .update(cx, |settings, cx| settings.open(window, cx));
                }
            });
        let runtime_events =
            cx.subscribe_in(&runtime, window, |this, _, event, window, cx| match event {
                RuntimeEvent::BackendChoiceRequested => {
                    this.backend_choice_return_focus = window.focused(cx);
                    this.backend_choice_source = if this.launcher.read(cx).is_open() {
                        this.launcher
                            .read(cx)
                            .launch_source_bounds()
                            .unwrap_or_default()
                    } else {
                        this.sidebar.read(cx).action_source_bounds()
                    };
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
            highlights: None,
            highlights_events: None,
            highlights_startup_checked: cfg!(test),
            highlights_startup_scheduled: false,
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
            backend_choice_source: gpui::Bounds::default(),
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
            header_focus: [
                "window-close",
                "window-minimize",
                "window-fullscreen",
                "sidebar-toggle",
                "header-grid",
                "header-settings",
            ]
            .into_iter()
            .map(|id| (id, cx.focus_handle().tab_stop(true)))
            .collect(),
        }
    }

    fn open_highlights(&mut self, debug: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(release) = super::release_highlights::selected(debug) else {
            return;
        };
        self.open_highlights_release(release, window, cx);
    }

    fn open_highlights_release(
        &mut self,
        release: grove_core::release_highlights::ReleaseHighlights,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shortcut_blocked_in(window, cx)
            || self.sidebar.read(cx).highlights_blocked()
            || release.slides.is_empty()
        {
            return;
        }
        let carousel =
            cx.new(|cx| super::release_highlights::ReleaseCarousel::new(release, window, cx));
        self.highlights_events = Some(cx.subscribe(
            &carousel,
            |this, _, _: &super::release_highlights::Closed, cx| {
                this.highlights = None;
                this.highlights_events = None;
                cx.notify();
            },
        ));
        self.highlights = Some(carousel);
        cx.notify();
    }

    fn check_startup_highlights(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.highlights_startup_checked
            || self.shortcut_blocked_in(window, cx)
            || self.sidebar.read(cx).highlights_blocked()
        {
            return;
        }
        self.highlights_startup_checked = true;
        let debug =
            cfg!(debug_assertions) && std::env::var("GROVE_HIGHLIGHTS_DEBUG").as_deref() == Ok("1");
        let Some(release) = super::release_highlights::selected(debug) else {
            return;
        };
        if !debug
            && !release.should_auto_open(
                env!("CARGO_PKG_VERSION"),
                &cx.global::<crate::settings::SettingsState>()
                    .store
                    .seen_highlights_versions,
            )
        {
            return;
        }
        self.open_highlights(debug, window, cx);
        if self.highlights.is_some() && !debug {
            let ((), result) =
                crate::settings::SettingsState::update_and_flush_checked(cx, |store| {
                    release.mark_seen(&mut store.seen_highlights_versions);
                });
            if let Err(error) = result {
                eprintln!("Could not remember release highlights: {error}");
            }
        }
    }

    fn provider_update_notice(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let updates = self.runtime.read(cx).provider_updates.clone();
        let text = updates.read(cx).notice().unwrap_or_default();
        div()
            .id("provider-update-notice")
            .role(gpui::Role::Status)
            .flex()
            .items_center()
            .flex_wrap()
            .gap(rpx(SPACE_MD))
            .px(rpx(SPACE_LG))
            .py(rpx(SPACE_SM))
            .bg(c::BG_STRIP())
            .border_t_1()
            .border_color(c::BORDER())
            .text_size(rpx(TEXT_SMALL))
            .text_color(c::FG())
            .child(div().flex_1().min_w_0().child(super::status::label(
                "provider-update-notice-status",
                super::status::StatusKind::Info,
                text,
                ICON_SM,
                cx,
            )))
            .child(
                form_action(
                    "provider-update-review",
                    "Review updates",
                    false,
                    window,
                    cx,
                )
                .icon(gpui_component::Icon::default().path("icons/arrow-up.svg"))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.settings.update(cx, |settings, cx| {
                        settings.open_provider_updates(window, cx);
                    });
                })),
            )
            .child(
                form_action("provider-update-dismiss", "Dismiss", false, window, cx)
                    .icon(gpui_component::Icon::default().path("icons/close.svg"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.runtime.read(cx).provider_updates.clone().update(
                            cx,
                            crate::entities::provider_updates::ProviderUpdates::dismiss,
                        );
                    })),
            )
    }

    fn header_action(
        &self,
        id: &'static str,
        label: &'static str,
        action: fn(&mut Self, &mut Window, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let focus = self.header_focus[id].clone();
        let enabled = !matches!(id, "sidebar-toggle" | "header-grid" | "header-settings")
            || !self.shortcut_blocked(cx);
        header_control_with_label_tooltip(id, label)
            .debug_selector(move || id.into())
            .track_focus(&focus)
            .tab_index(if enabled { 0 } else { -1 })
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .when(!enabled, |control| {
                control
                    .group("disabled-control")
                    .cursor_default()
                    .opacity(OPACITY_DISABLED)
            })
            .when(enabled, |control| {
                control
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        focus.focus(window, cx);
                    })
                    .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                && !event.is_held
                            {
                                action(this, window, cx);
                                window.prevent_default();
                                cx.stop_propagation();
                            }
                        },
                    ))
            })
    }

    fn header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let collapsed = self.sidebar.read(cx).navigation_collapsed(window, cx);
        let fullscreen = window.is_fullscreen();
        let compact = f32::from(window.viewport_size().width)
            / (f32::from(window.rem_size()) / crate::zoom::REM_BASE)
            < 400.0;
        let traffic = |control: gpui::Stateful<gpui::Div>, color, glyph| {
            control.w(rpx(TRAFFIC_CONTROL_W)).child(
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
                            .group_hover("traffic-lights", |style| style.opacity(1.0)),
                    ),
            )
        };
        div()
            .id("app-header")
            .relative()
            .debug_selector(|| "app-header".into())
            .h(rpx(APPBAR_TOTAL_H))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_LG))
            .pl(rpx(SPACE_LG))
            .pr(rpx(SPACE_LG))
            .pt(rpx(APPBAR_TOP_INSET))
            .bg(c::BG_RAIL())
            .child(
                div()
                    .group("traffic-lights")
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .child(traffic(
                        self.header_action(
                            "window-close",
                            "Close window",
                            Self::close_after_codex,
                            cx,
                        ),
                        c::WINDOW_CLOSE(),
                        "close",
                    ))
                    .child(traffic(
                        self.header_action(
                            "window-minimize",
                            "Minimize window",
                            |_, window, _| window.minimize_window(),
                            cx,
                        ),
                        c::WINDOW_MINIMIZE(),
                        "minus",
                    ))
                    .child(traffic(
                        self.header_action(
                            "window-fullscreen",
                            if fullscreen {
                                "Exit full screen"
                            } else {
                                "Enter full screen"
                            },
                            |_, window, _| window.toggle_fullscreen(),
                            cx,
                        ),
                        c::WINDOW_FULLSCREEN(),
                        "plus",
                    )),
            )
            .child(
                self.header_action(
                    "sidebar-toggle",
                    if collapsed {
                        "Expand sidebar"
                    } else {
                        "Collapse sidebar"
                    },
                    |this, window, cx| {
                        if !this.shortcut_blocked_in(window, cx) {
                            this.sidebar
                                .update(cx, |sidebar, cx| sidebar.toggle_sidebar(window, cx));
                        }
                    },
                    cx,
                )
                .child(icon(
                    if collapsed {
                        "sidebar-expand"
                    } else {
                        "sidebar-collapse"
                    },
                    ICON_LG,
                    c::FG_DIM(),
                )),
            )
            .when(!compact, |header| {
                header.child(
                    div().flex_1().min_w_0().child(
                        self.sidebar
                            .update(cx, |sidebar, cx| sidebar.header_workspace_control(cx)),
                    ),
                )
            })
            .child(
                div()
                    .id("header-main-drag-region")
                    .absolute()
                    .top_0()
                    .right_0()
                    .w(rpx(SPACE_LG))
                    .min_w_0()
                    .h_full()
                    .on_mouse_down(MouseButton::Left, |event, window, _| {
                        if event.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    }),
            )
    }

    fn close_after_codex(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stopped = self.runtime.update(cx, Runtime::stop_codex);
        cx.spawn_in(window, async move |this, cx| {
            stopped.await;
            let _ = this.update_in(cx, |_, window, _| window.remove_window());
        })
        .detach();
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

    fn set_zoom(&self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        if self.shortcut_blocked_in(window, cx) {
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

    fn shortcut_blocked_in(&self, window: &Window, cx: &App) -> bool {
        super::mermaid::is_expanded(window, cx) || self.shortcut_blocked(cx)
    }

    fn shortcut_blocked(&self, cx: &App) -> bool {
        self.highlights.is_some()
            || self.backend_choice_open
            || self.launcher.read(cx).is_open()
            || self.settings.read(cx).is_open()
            || self.switcher_open
            || self.workspaces.read(cx).is_open()
            || self.sidebar.read(cx).confirmation_open()
            || self.sidebar.read(cx).edit_confirmation_open(cx)
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
            let status = if let Some(session) = registry.codex(id) {
                {
                    let label = session.read(cx).state.session_status();
                    (label, super::status::session_kind(label).color())
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

    fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar
            .update(cx, super::sidebar::Sidebar::dismiss_project_context);
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

    fn open_session_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shortcut_blocked_in(window, cx) {
            self.sidebar
                .update(cx, super::sidebar::Sidebar::dismiss_project_context);
            self.launcher
                .update(cx, |launcher, cx| launcher.open_session_picker(window, cx));
        }
    }

    fn open_new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shortcut_blocked_in(window, cx) {
            self.sidebar
                .update(cx, super::sidebar::Sidebar::dismiss_project_context);
            self.launcher
                .update(cx, |launcher, cx| launcher.open(window, cx));
        }
    }

    fn new_session_from_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_switcher(window, cx);
        self.sidebar
            .update(cx, super::sidebar::Sidebar::dismiss_project_context);
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
                    .cursor_pointer()
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
                    .hover(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(rpx(TEXT_BODY))
                            .text_color(c::FG())
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_SM))
                            .child(crate::icons::icon("terminal", ICON_SM, c::FG_DIM()))
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
            .bg(c::QUIET_VEIL())
            .track_focus(&self.backend_choice_focus)
            .capture_key_down(cx.listener(Self::backend_choice_key))
            .child(
                gpui::anchored()
                    .position_mode(gpui::AnchoredPositionMode::Window)
                    .position(gpui::point(self.backend_choice_source.left(), self.backend_choice_source.bottom()))
                    .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
                    .child(div()
                    .id("backend-choice-dialog")
                    .debug_selector(|| "backend-choice-dialog".into())
                    .role(gpui::Role::Dialog)
                    .aria_label("Choose a terminal session backend")
                    .w(rpx(MODAL_W_SM))
                    .max_w_full()
                    .max_h(rpx(MODAL_SCROLL_MAX_H))
                    .overflow_y_scroll()
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
                            .child("Choose before starting this terminal session. You can change this later in Settings. Existing sessions keep their backend."),
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
                                        form_action("backend-choice-back", "Back", false, window, cx).icon(gpui_component::Icon::default().path("icons/arrow-left.svg").size(rpx(ICON_SM)))
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
                                        form_action("backend-choice-start-action", "Start session", true, window, cx).icon(gpui_component::Icon::default().path("icons/play.svg").size(rpx(ICON_SM)))
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
                                .text_color(c::FORM_ERROR())
                                .child(error.clone()),
                        )
                    }),
            ));
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
                        .text_color(super::status::session_kind(label).color())
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_SM))
                        .child(super::status::mark(
                            format!("switcher-status-{target:?}"),
                            super::status::session_kind(label),
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

impl Focusable for Shell {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if let Some(carousel) = &self.highlights {
            carousel.read(cx).focus_handle(cx)
        } else if self.backend_choice_open {
            self.backend_choice_focus.clone()
        } else {
            self.focus.clone()
        }
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Resolve legacy System before pinning the native material's tone.
        if cx.global::<ThemeState>().follow_system {
            ThemeState::set_system_mode(cx, window.appearance());
        }
        if !cfg!(test) {
            crate::platform::window_chrome::sync_theme_appearance(window, c::is_dark());
        }

        #[cfg(target_os = "macos")]
        {
            let mut state = self.sidebar.read(cx).menu_state(window, cx);
            state.navigation &= !self.shortcut_blocked_in(window, cx);
            state.switcher = self.highlights.is_none()
                && !self.backend_choice_open
                && !self.launcher.read(cx).is_open()
                && !self.settings.read(cx).is_open()
                && !self.workspaces.read(cx).is_open()
                && !self.switcher_open
                && self.sidebar.read(cx).navigation_available();
            crate::app_menus::refresh(state, cx);
        }
        let html_owner = self.sidebar.read(cx).html_owner(cx);
        super::mermaid::set_context(html_owner, window, cx);
        let mermaid_expanded = super::mermaid::render_expanded(window, cx);
        let html_blocked =
            self.shortcut_blocked_in(window, cx) || self.sidebar.read(cx).html_overlay_open();
        super::inline_html::set_context(html_owner, html_blocked, cx);
        let html_expanded = super::inline_html::render_expanded(window, cx);
        super::native_plan::set_context(html_owner, html_blocked, cx);
        let project_hover_blocked = self.shortcut_blocked_in(window, cx);
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_project_hover_blocked(project_hover_blocked, cx);
        });
        if !self.highlights_startup_checked
            && !self.highlights_startup_scheduled
            && !self.shortcut_blocked_in(window, cx)
            && !self.sidebar.read(cx).highlights_blocked()
        {
            self.highlights_startup_scheduled = true;
            let shell = cx.entity().downgrade();
            window.defer(cx, move |window, cx| {
                // Deferred after the first rendered frame; retry once other panels close.
                let _ = shell.update(cx, |this, cx| {
                    this.highlights_startup_scheduled = false;
                    this.check_startup_highlights(window, cx);
                });
            });
        }
        // The sidebar supplies the sole workspace trigger; its popover uses compact anchoring.
        let compact_workspace_picker = true;
        let workspace_hover_blocked = self.highlights.is_some()
            || self.backend_choice_open
            || self.launcher.read(cx).is_open()
            || self.settings.read(cx).is_open()
            || self.switcher_open
            || self.sidebar.read(cx).confirmation_open()
            || self.sidebar.read(cx).edit_confirmation_open(cx);
        self.workspaces.update(cx, |manager, _| {
            manager.set_hover_blocked(workspace_hover_blocked);
            manager.set_compact(compact_workspace_picker);
        });
        if self.backend_choice_open && !self.backend_choice_focus.is_focused(window) {
            self.backend_choice_focus.focus(window, cx);
        }
        if self.window_observers.is_none() {
            let runtime = self.runtime.clone();
            window.on_window_should_close(cx, move |window, cx| {
                let stopped = runtime.update(cx, Runtime::stop_codex);
                let handle = window.window_handle();
                cx.spawn(async move |cx| {
                    stopped.await;
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                })
                .detach();
                false
            });
            let runtime = self.runtime.clone();
            self.window_observers = Some(vec![
                cx.observe_window_appearance(window, |_, window, cx| {
                    ThemeState::set_system_mode(cx, window.appearance());
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

        // Custom app-header controls replace the AppKit buttons without duplicating them.
        if !cfg!(test) {
            crate::platform::window_chrome::sync_native_buttons(window);
        }
        window.set_rem_size(gpui::px(cx.global::<crate::zoom::ZoomState>().rem_size()));
        div()
            .id("grove-shell")
            .font_family(crate::fonts::UI_FAMILY)
            .track_focus(&self.focus)
            .when(
                self.sidebar.read(cx).is_grid()
                    && !self.sidebar.read(cx).is_zen()
                    && !self.shortcut_blocked_in(window, cx),
                |shell| shell.key_context(k::Screen::Grid.key_context()),
            )
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape"
                    && this
                        .workspaces
                        .update(cx, |manager, cx| manager.dismiss_hover_menu(window, cx))
                {
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
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
            .on_action(
                cx.listener(|this, _: &PreviewReleaseHighlights, window, cx| {
                    if cfg!(debug_assertions) {
                        this.open_highlights(true, window, cx);
                    }
                }),
            )
            .when(cfg!(target_os = "macos"), |root| {
                #[cfg(target_os = "macos")]
                let root =
                    root.on_action(cx.listener(|_, _: &crate::app_menus::About, window, cx| {
                        let detail = format!("Version {}", env!("CARGO_PKG_VERSION"));
                        drop(window.prompt(
                            gpui::PromptLevel::Info,
                            "About Grove",
                            Some(&detail),
                            &["OK"],
                            cx,
                        ));
                    }))
                    .on_action(cx.listener(|_, _: &crate::app_menus::Minimize, window, _| {
                        window.minimize_window();
                    }))
                    .on_action(cx.listener(
                        |_, _: &crate::app_menus::ZoomWindow, window, _| window.zoom_window(),
                    ));
                root
            })
            .on_action(cx.listener(|this, _: &Quit, _, cx| {
                this.runtime
                    .update(cx, |runtime, cx| runtime.request_exit(false, cx));
            }))
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| {
                this.close_after_codex(window, cx);
            }))
            .on_action(cx.listener(|_, _: &ToggleFullscreen, window, cx| {
                window.toggle_fullscreen();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &k::NewSession, window, cx| {
                this.open_new_session(window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &k::NewSessionInWorktree, window, cx| {
                    if !this.shortcut_blocked_in(window, cx) {
                        if let Some((project, path)) = this.sidebar.read(cx).selected_worktree() {
                            this.sidebar
                                .update(cx, super::sidebar::Sidebar::dismiss_project_context);
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
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.select_next_session(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::PrevSession, window, cx| {
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar.update(cx, |sidebar, cx| {
                        sidebar.select_previous_session(window, cx);
                    });
                }
            }))
            .on_action(cx.listener(|this, action: &k::GridMove, window, cx| {
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar.update(cx, |sidebar, cx| {
                        sidebar.grid_move(action.dx, action.dy, window, cx);
                    });
                }
            }))
            .on_action(cx.listener(|this, action: &k::GridSwap, window, cx| {
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar.update(cx, |sidebar, cx| {
                        sidebar.grid_swap(action.dx, action.dy, window, cx);
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &k::OpenNavigation1, window, cx| {
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar.update(cx, |sidebar, cx| {
                        sidebar.open_numbered_destination(1, window, cx);
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &k::OpenNavigation2, window, cx| {
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar.update(cx, |sidebar, cx| {
                        sidebar.open_numbered_destination(2, window, cx);
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &k::OpenNavigation3, window, cx| {
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar.update(cx, |sidebar, cx| {
                        sidebar.open_numbered_destination(3, window, cx);
                    });
                }
            }))
            .on_action(cx.listener(|this, action: &k::SelectSession, window, cx| {
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar.update(cx, |sidebar, cx| {
                        sidebar.select_numbered_session(action.index, window, cx);
                    });
                }
            }))
            .on_action(
                cx.listener(|this, _: &k::JumpToWaitingSession, window, cx| {
                    if !this.shortcut_blocked_in(window, cx) {
                        this.sidebar
                            .update(cx, |sidebar, cx| sidebar.select_waiting_session(window, cx));
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &k::ToggleSidebar, window, cx| {
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.toggle_sidebar(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::ToggleRailMode, window, cx| {
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.toggle_tree_list(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::ToggleGrid, window, cx| {
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.toggle_grid(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::ToggleZen, window, cx| {
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.toggle_zen(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::NewHomeTerminal, window, cx| {
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.add_terminal(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::CloseFocusedSession, window, cx| {
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar
                        .update(cx, |sidebar, cx| sidebar.request_close_focused(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::Settings, window, cx| {
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar
                        .update(cx, super::sidebar::Sidebar::dismiss_project_context);
                    this.settings
                        .update(cx, |settings, cx| settings.open(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::ShortcutOverlay, window, cx| {
                if !this.shortcut_blocked_in(window, cx) {
                    this.sidebar
                        .update(cx, super::sidebar::Sidebar::dismiss_project_context);
                    this.settings
                        .update(cx, |settings, cx| settings.open_shortcuts(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &k::ZoomIn, window, cx| {
                this.set_zoom(crate::zoom::ZOOM_STEP, window, cx);
            }))
            .on_action(cx.listener(|this, _: &k::ZoomOut, window, cx| {
                this.set_zoom(-crate::zoom::ZOOM_STEP, window, cx);
            }))
            .on_action(cx.listener(|this, _: &k::ZoomReset, window, cx| {
                this.set_zoom(0.0, window, cx);
            }))
            .child(div().flex_1().min_h_0().child(self.sidebar.clone()))
            .child(
                div()
                    .child(self.header(window, cx))
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(rpx(self
                        .sidebar
                        .read(cx)
                        .animated_sidebar_width(window, cx))),
            )
            .when(
                self.runtime
                    .read(cx)
                    .provider_updates
                    .read(cx)
                    .notice()
                    .is_some(),
                |root| root.child(self.provider_update_notice(window, cx)),
            )
            .when_some(
                self.runtime.read(cx).toast.read(cx).current().cloned(),
                |root, toast| root.child(super::toast_notice::render(toast, cx)),
            )
            .when(self.launcher.read(cx).is_visible(cx), |root| {
                root.child(gpui::deferred(self.launcher.clone()))
            })
            .when(self.switcher_open, |root| {
                root.child(gpui::deferred(self.session_switcher(window, cx)))
            })
            .when(self.backend_choice_open, |root| {
                root.child(gpui::deferred(self.backend_choice(window, cx)))
            })
            .when_some(self.highlights.clone(), |root, carousel| {
                root.child(gpui::deferred(carousel))
            })
            .when_some(mermaid_expanded, |root, overlay| {
                root.child(gpui::deferred(overlay))
            })
            .when_some(html_expanded, |root, overlay| {
                root.child(gpui::deferred(overlay))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn highlights_fixture() -> grove_core::release_highlights::ReleaseHighlights {
        use grove_core::release_highlights::{
            HighlightMedia, HighlightSlide, MediaKind, ReleaseHighlights,
        };
        ReleaseHighlights {
            version: "99.0.0".into(),
            enabled: false,
            title: "Test highlights".into(),
            slides: vec![HighlightSlide {
                id: "fixture".into(),
                title: "An example improvement".into(),
                description: "A fixture independent of release authoring.".into(),
                media: Some(HighlightMedia {
                    kind: MediaKind::Image,
                    src: "highlights/test-missing.png".into(),
                    alt: "Test preview".into(),
                    poster: None,
                    captions: None,
                    frame: None,
                }),
            }],
        }
    }

    #[gpui::test]
    fn debug_highlights_replay_blocks_commands_without_marking_seen(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let seen = cx.update(|_, cx| {
            cx.global::<crate::settings::SettingsState>()
                .store
                .seen_highlights_versions
                .clone()
        });
        for _ in 0..2 {
            cx.update(|window, cx| {
                shell.update(cx, |this, cx| {
                    this.open_highlights_release(highlights_fixture(), window, cx);
                });
            });
            draw(cx);
            cx.update(|window, cx| {
                assert!(shell.read(cx).highlights.is_some());
                assert!(shell.read(cx).shortcut_blocked(cx));
                window.dispatch_action(Box::new(k::NewSession), cx);
                assert!(!shell.read(cx).launcher.read(cx).is_open());
            });
            cx.simulate_keystrokes("escape");
            draw(cx);
            cx.update(|_, cx| {
                assert!(shell.read(cx).highlights.is_none());
                assert_eq!(
                    cx.global::<crate::settings::SettingsState>()
                        .store
                        .seen_highlights_versions,
                    seen
                );
            });
        }
    }

    #[gpui::test]
    fn startup_highlights_wait_for_settings_backend_and_sidebar_panels(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        cx.update(|window, cx| {
            shell.update(cx, |this, cx| {
                cx.global_mut::<crate::settings::SettingsState>()
                    .store
                    .seen_highlights_versions
                    .push(env!("CARGO_PKG_VERSION").into());
                this.highlights_startup_checked = false;
                this.backend_choice_open = true;
                this.check_startup_highlights(window, cx);
                assert!(!this.highlights_startup_checked);
                assert!(this.highlights.is_none());
                this.backend_choice_open = false;
                this.settings.update(cx, |panel, cx| panel.open(window, cx));
                this.check_startup_highlights(window, cx);
                assert!(!this.highlights_startup_checked);
                this.settings
                    .update(cx, |panel, cx| panel.close(window, cx));
                this.sidebar
                    .update(cx, |sidebar, cx| sidebar.open_archived_projects(window, cx));
                assert!(this.sidebar.read(cx).highlights_blocked());
                this.check_startup_highlights(window, cx);
                assert!(!this.highlights_startup_checked);
            });
        });
        draw(cx);
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            shell.update(cx, |this, cx| {
                assert!(!this.sidebar.read(cx).highlights_blocked());
                this.check_startup_highlights(window, cx);
                assert!(this.highlights_startup_checked);
                // Already seen installed releases remain closed after setup completes.
                assert!(this.highlights.is_none());
            });
        });
    }

    #[gpui::test]
    fn zen_shows_one_terminal_and_restores_the_previous_view(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        cx.update(|_, cx| {
            let sidebar = shell.read(cx).sidebar.clone();
            sidebar.update(
                cx,
                crate::views::sidebar::Sidebar::select_project_canvas_for_test,
            );
        });
        draw(cx);
        cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
            window.dispatch_action(Box::new(k::ToggleZen), cx);
        });
        draw(cx);
        assert!(cx.debug_bounds("app-header").is_some());
        assert!(cx.debug_bounds("statusbar").is_none());
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        assert!(cx.debug_bounds("projects-destination").is_some());
        cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
        draw(cx);
        assert!(cx.debug_bounds("app-header").is_some());
        assert!(cx.debug_bounds("global-navigation").is_some());
        assert!(cx.debug_bounds("statusbar").is_none());
        assert!(cx.debug_bounds("global-navigation").is_some());
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
        assert!(cx.debug_bounds("app-header").is_some());
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        assert!(cx.debug_bounds("terminal-header-1").is_none());
        assert!(cx.debug_bounds("terminal-pane-1").is_some());
        cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
        draw(cx);
        assert!(cx.debug_bounds("app-header").is_some());
        assert!(cx.debug_bounds("global-navigation").is_some());
        assert!(cx.debug_bounds("global-navigation").is_some());
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
        assert!(cx.debug_bounds("app-header").is_some());
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        assert!(cx.debug_bounds("terminal-pane-1").is_some());
        cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
        assert!(cx.debug_bounds("app-header").is_some());
        assert!(cx.debug_bounds("global-navigation").is_some());
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
        assert!(cx.debug_bounds("projects-destination").is_some());
        cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
    }

    #[gpui::test]
    fn empty_grid_zen_shows_one_overview_and_returns_to_grid(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        cx.update(|_, cx| {
            let sidebar = shell.read(cx).sidebar.clone();
            sidebar.update(
                cx,
                crate::views::sidebar::Sidebar::select_project_canvas_for_test,
            );
        });
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
        assert!(cx.debug_bounds("app-header").is_some());
        assert!(cx.debug_bounds("projects-destination").is_some());
        assert!(cx.debug_bounds("session-grid").is_none());
        cx.update(|window, cx| window.dispatch_action(Box::new(k::ToggleZen), cx));
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
        assert!(cx.debug_bounds("app-header").is_some());
        assert!(cx.debug_bounds("global-navigation").is_some());
    }

    fn init(cx: &mut App) {
        gpui_component::init(cx);
        // Shell navigation checks measure stable endpoints; motion has owner-level coverage.
        cx.set_reduce_motion(true);
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

    fn init_projects(cx: &mut App) {
        init(cx);
        cx.global_mut::<crate::settings::SettingsState>()
            .store
            .default_landing_page = grove_core::storage::LandingPage::Projects;
    }

    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn new_session_sidebar_and_shortcuts_open_launcher_preserving_home_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init);
        cx.update(|cx| cx.bind_keys(k::shell_bindings()));
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let composer = cx.debug_bounds("codex-composer-input").unwrap();
        cx.simulate_click(composer.center(), gpui::Modifiers::default());
        cx.simulate_input("Keep my Home overview draft");
        draw(cx);
        for source in ["sidebar", "new-session"] {
            let before = cx.update(|window, cx| {
                if source == "sidebar" {
                    Some(shell.read(cx).sidebar.read(cx).new_session_focus_for_test())
                } else {
                    window.focused(cx)
                }
            });
            if source == "sidebar" {
                let row = cx.debug_bounds("global-nav-new-session").unwrap();
                cx.simulate_click(row.center(), gpui::Modifiers::default());
            } else {
                cx.simulate_keystrokes(&format!(
                    "{}{}",
                    k::platform_mod_prefix(),
                    if source == "numbered" { "1" } else { "p" }
                ));
            }
            draw(cx);
            assert!(
                cx.debug_bounds("worktree-launcher-panel").is_some(),
                "{source}"
            );
            assert!(cx.debug_bounds("worktree-launcher-search").is_some());
            assert!(cx.debug_bounds("dashboard-summary").is_some());
            cx.update(|_, cx| {
                let owner = shell.read(cx);
                assert!(owner.launcher.read(cx).is_open());
                let launcher = owner.launcher.read(cx);
                assert_eq!(
                    launcher.title_for_test(),
                    if source == "new-session" {
                        "Command palette"
                    } else {
                        "New session"
                    }
                );
                if source != "new-session" {
                    assert!(launcher
                        .rows_for_test(cx)
                        .iter()
                        .all(|row| matches!(row, PaletteRow::Combo { .. })));
                }
                assert_eq!(
                    owner
                        .sidebar
                        .read(cx)
                        .dashboard_draft_for_test(cx)
                        .as_deref(),
                    Some("Keep my Home overview draft")
                );
                assert_eq!(owner.runtime.read(cx).registry.read(cx).len(), 0);
            });
            cx.simulate_keystrokes("escape");
            draw(cx);
            if source != "new-session" {
                cx.update(|_, cx| {
                    let launcher = shell.read(cx).launcher.read(cx);
                    assert!(launcher.is_open());
                    assert_eq!(launcher.title_for_test(), "Command palette");
                });
                cx.simulate_keystrokes("escape");
                draw(cx);
            }
            cx.update(|window, cx| {
                assert!(!shell.read(cx).launcher.read(cx).is_open());
                assert_eq!(window.focused(cx), before);
                assert_eq!(
                    shell
                        .read(cx)
                        .sidebar
                        .read(cx)
                        .dashboard_draft_for_test(cx)
                        .as_deref(),
                    Some("Keep my Home overview draft")
                );
            });
        }
        // Opening the modal from another destination must not navigate to Home.
        cx.simulate_keystrokes(&format!("{}3", k::platform_mod_prefix()));
        draw(cx);
        assert!(cx.debug_bounds("projects-destination").is_some());
        let plus = cx.debug_bounds("global-nav-new-session").unwrap();
        cx.simulate_click(plus.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("worktree-launcher-panel").is_some());
        cx.simulate_keystrokes("escape escape");
        draw(cx);
        assert!(cx.debug_bounds("projects-destination").is_some());
        let plus = cx.debug_bounds("global-nav-new-session").unwrap();
        cx.simulate_click(plus.center(), gpui::Modifiers::default());
        draw(cx);
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(cx.debug_bounds("dashboard-summary").is_some());
        cx.update(|_, cx| {
            assert_eq!(
                shell
                    .read(cx)
                    .sidebar
                    .read(cx)
                    .dashboard_draft_for_test(cx)
                    .as_deref(),
                Some("Keep my Home overview draft")
            )
        });
    }

    #[gpui::test]
    fn new_session_landing_opens_launcher_over_home_overview(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            init(cx);
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .default_landing_page = grove_core::storage::LandingPage::NewSession;
        });
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        assert!(cx.debug_bounds("worktree-launcher-panel").is_some());
        assert!(cx.debug_bounds("dashboard-summary").is_some());
        cx.update(|_, cx| {
            let launcher = shell.read(cx).launcher.read(cx);
            assert_eq!(launcher.title_for_test(), "New session");
            assert!(launcher
                .rows_for_test(cx)
                .iter()
                .all(|row| matches!(row, PaletteRow::Combo { .. })));
        });
        cx.simulate_keystrokes("escape escape");
        draw(cx);
        assert!(!cx.update(|_, cx| shell.read(cx).launcher.read(cx).is_open()));
        assert!(cx.debug_bounds("dashboard-summary").is_some());
    }

    #[gpui::test]
    fn numbered_destinations_route_and_respect_blocking_overlays(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        cx.update(|cx| cx.bind_keys(k::shell_bindings()));
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        for (key, destination) in [
            ("3", "projects-destination"),
            ("2", "history-list-scroll"),
            ("1", "dashboard-page"),
        ] {
            cx.update(|window, cx| {
                let focus = shell.read(cx).focus.clone();
                focus.focus(window, cx);
            });
            cx.simulate_keystrokes(&format!("{}{key}", k::platform_mod_prefix()));
            draw(cx);
            assert!(
                cx.debug_bounds(destination).is_some(),
                "{key} must open {destination}"
            );
        }
        cx.simulate_keystrokes("escape escape");
        draw(cx);
        cx.update(|window, cx| window.dispatch_action(Box::new(k::Settings), cx));
        draw(cx);
        cx.simulate_keystrokes(&format!("{}2", k::platform_mod_prefix()));
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).settings.read(cx).is_open()));
        assert!(cx.debug_bounds("projects-destination").is_none());
        assert!(cx.debug_bounds("statusbar").is_none());
        cx.update(|window, cx| {
            shell
                .read(cx)
                .settings
                .clone()
                .update(cx, |settings, cx| settings.close(window, cx))
        });
        draw(cx);
        cx.update(|window, cx| {
            let launcher = shell.read(cx).launcher.clone();
            launcher.update(cx, |launcher, cx| launcher.open(window, cx));
        });
        draw(cx);
        cx.simulate_keystrokes(&format!("{}3", k::platform_mod_prefix()));
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).launcher.read(cx).is_open()));
        assert!(cx.debug_bounds("session-history-page").is_none());
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
    fn header_controls_support_keyboard_and_restore_settings_source(cx: &mut gpui::TestAppContext) {
        cx.update(init_projects);
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        cx.update(|window, cx| {
            let focus = shell.read(cx).header_focus["window-fullscreen"].clone();
            focus.focus(window, cx);
        });
        cx.simulate_keystrokes("enter");
        draw(cx);
        cx.update(|window, _| assert!(window.is_fullscreen()));
        cx.simulate_keystrokes("space");
        draw(cx);
        cx.update(|window, _| assert!(!window.is_fullscreen()));
        let settings = cx.debug_bounds("header-settings").unwrap();
        cx.simulate_click(settings.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("settings-panel").is_some());
        let collapsed = cx.update(|_, cx| {
            cx.global::<crate::settings::SettingsState>()
                .store
                .sidebar_collapsed
        });
        let toggle = cx.debug_bounds("sidebar-toggle").unwrap();
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_collapsed,
                collapsed
            )
        });
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            assert!(!shell.read(cx).settings.read(cx).is_open());
            assert!(shell.read(cx).header_focus["header-settings"].is_focused(window));
        });
    }

    #[gpui::test]
    fn sidebar_collapse_shortcut_toggles_and_respects_blocking(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            init_projects(cx);
            cx.bind_keys(k::shell_bindings());
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .sidebar_width = Some(300.0);
        });
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        let shortcut = k::SHORTCUTS
            .iter()
            .find(|def| def.action == Some(k::GlobalShortcut::ToggleSidebar))
            .expect("sidebar shortcut registry row");
        let chord = k::keystrokes_for(shortcut).remove(0);
        cx.update(|window, cx| {
            let focus = shell.read(cx).focus.clone();
            focus.focus(window, cx);
        });
        cx.simulate_keystrokes(&chord);
        draw(cx);
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        assert!(cx.debug_bounds("global-navigation").is_none());
        assert!(cx.debug_bounds("header-workspace").is_none());
        assert!(cx.debug_bounds("header-settings").is_some());
        assert!(cx.debug_bounds("window-close").is_some());
        assert!(cx.debug_bounds("sidebar-divider").is_none());
        cx.update(|_, cx| {
            let settings = &cx.global::<crate::settings::SettingsState>().store;
            assert!(settings.sidebar_collapsed);
            assert_eq!(settings.sidebar_width, Some(300.0));
        });
        cx.simulate_keystrokes(&chord);
        draw(cx);
        assert_rail_alignment(cx, 300.0);
        cx.update(|window, cx| {
            assert!(
                !cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_collapsed
            );
            let settings = shell.read(cx).settings.clone();
            settings.update(cx, |settings, cx| settings.open(window, cx));
            assert!(shell.read(cx).shortcut_blocked(cx));
        });
        draw(cx);
        cx.simulate_keystrokes(&chord);
        draw(cx);
        cx.update(|window, cx| {
            assert!(
                !cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_collapsed
            );
            window.dispatch_action(Box::new(k::ToggleSidebar), cx);
            assert!(
                !cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_collapsed
            );
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_width,
                Some(300.0)
            );
        });
    }

    #[gpui::test]
    fn sidebar_collapse_restores_width_and_header_controls(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            init_projects(cx);
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .sidebar_width = Some(300.);
        });
        let (_, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
        draw(cx);
        assert_rail_alignment(cx, 300.);
        for _ in 0..2 {
            let toggle = cx.debug_bounds("sidebar-toggle").unwrap();
            cx.simulate_click(toggle.center(), gpui::Modifiers::default());
            draw(cx);
            assert!(cx.debug_bounds("global-navigation").is_none());
            assert!(cx.debug_bounds("header-workspace").is_none());
            assert_eq!(
                f32::from(cx.debug_bounds("sidebar-canvas").unwrap().left()),
                0.
            );
            for id in [
                "window-close",
                "window-minimize",
                "window-fullscreen",
                "sidebar-toggle",
                "header-settings",
            ] {
                assert!(cx.debug_bounds(id).is_some(), "{id}");
            }
            let toggle = cx.debug_bounds("sidebar-toggle").unwrap();
            cx.simulate_click(toggle.center(), gpui::Modifiers::default());
            draw(cx);
            assert_rail_alignment(cx, 300.);
        }
        cx.update(|_, cx| {
            assert_eq!(
                cx.global::<crate::settings::SettingsState>()
                    .store
                    .sidebar_width,
                Some(300.)
            )
        });
    }

    #[gpui::test]
    fn global_projects_control_returns_from_grid_while_settings_open(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init_projects);
        let (shell, cx) = cx.add_window_view(Shell::new);
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
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        let projects = cx.debug_bounds("global-nav-projects").unwrap();
        cx.simulate_click(projects.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(!cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
        assert!(!cx.update(|_, cx| shell.read(cx).settings.read(cx).is_open()));
        assert_rail_alignment(cx, 260.);
    }

    #[gpui::test]
    fn context_workspace_header_opens_picker_and_escape_preserves_destination(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init_projects);
        let (_, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let trigger = cx.debug_bounds("header-workspace").unwrap();
        let rail = cx.debug_bounds("global-navigation").unwrap();
        assert!(rail.contains(&trigger.center()));
        cx.simulate_click(trigger.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("workspace-popup").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.debug_bounds("workspace-popup").is_none());
        assert_eq!(cx.debug_bounds("global-navigation").unwrap(), rail);
        assert!(cx.debug_bounds("global-nav-projects").is_some());
    }

    #[gpui::test]
    fn home_content_starts_below_one_shared_window_header(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (_, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let rail = cx.debug_bounds("global-navigation").unwrap();
        let canvas = cx.debug_bounds("sidebar-canvas").unwrap();
        let header = cx.debug_bounds("app-header").unwrap();
        assert_eq!(rail.top(), header.bottom());
        assert_eq!(canvas.top(), header.bottom());
        assert_eq!(canvas.left(), rail.right());
        let workspace = cx
            .debug_bounds("header-workspace")
            .expect("shared workspace access");
        assert!(rail.contains(&workspace.center()));
        cx.simulate_click(workspace.center(), gpui::Modifiers::default());
        draw(cx);
        let popup = cx
            .debug_bounds("workspace-popup")
            .expect("workspace picker");
        assert!((f32::from(popup.left() - workspace.left())).abs() <= SPACE_LG);
        assert!(popup.top() >= workspace.bottom());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.debug_bounds("workspace-popup").is_none());
        for selector in [
            "window-close",
            "window-minimize",
            "window-fullscreen",
            "sidebar-toggle",
            "header-settings",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
        assert!(cx.debug_bounds("context-workspace").is_none());
        assert!(cx.debug_bounds("global-nav-workspace").is_none());
    }

    fn assert_rail_alignment(cx: &mut gpui::VisualTestContext, expected: f32) {
        let rail = cx
            .debug_bounds("global-navigation")
            .expect("single sidebar");
        let divider = cx.debug_bounds("sidebar-divider").expect("resize hit zone");
        let canvas = cx.debug_bounds("sidebar-canvas").expect("canvas");
        let header = cx.debug_bounds("app-header").expect("window header");
        assert!((f32::from(rail.size.width) - expected).abs() <= 1.0);
        assert_eq!(rail.left(), gpui::px(0.));
        assert_eq!(rail.top(), header.bottom());
        assert_eq!(canvas.top(), header.bottom());
        assert_eq!(canvas.left(), rail.right());
        assert!((f32::from(divider.right() - rail.right())).abs() <= 1.0);
        assert!(divider.left() >= rail.left());
    }

    #[gpui::test]
    fn sidebar_drag_clamps_persists_on_release_and_survives_reconstruction(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init_projects);
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
        let home = cx.debug_bounds("global-nav-home").unwrap();
        cx.simulate_click(home.center(), gpui::Modifiers::default());
        draw(cx);
        assert_rail_alignment(cx, 220.0);
        let projects = cx.debug_bounds("global-nav-projects").unwrap();
        cx.simulate_click(projects.center(), gpui::Modifiers::default());
        draw(cx);
        assert_rail_alignment(cx, 220.0);
    }

    #[gpui::test]
    fn sidebar_narrow_cap_and_double_click_reset_keep_saved_preference(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init_projects);
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
    fn header_grid_control_toggles_and_respects_blocking(cx: &mut gpui::TestAppContext) {
        cx.update(init_projects);
        let (shell, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let grid = cx.debug_bounds("header-grid").unwrap();
        let settings = cx.debug_bounds("header-settings").unwrap();
        assert!(grid.right() < settings.left());
        cx.simulate_click(grid.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
        cx.update(|window, cx| {
            let focus = shell.read(cx).header_focus["header-grid"].clone();
            focus.focus(window, cx);
        });
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(!cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
        cx.simulate_click(settings.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).settings.read(cx).is_open()));
        cx.simulate_click(grid.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(!cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_grid()));
        assert!(cx.update(|_, cx| shell.read(cx).settings.read(cx).is_open()));
    }

    #[gpui::test]
    fn settings_stays_in_header_across_destination_changes(cx: &mut gpui::TestAppContext) {
        cx.update(init_projects);
        let (_, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        for destination in [
            "global-nav-home",
            "global-nav-projects",
            "global-nav-history",
        ] {
            let target = cx.debug_bounds(destination).expect("destination");
            cx.simulate_click(target.center(), gpui::Modifiers::default());
            draw(cx);
            let header = cx.debug_bounds("app-header").unwrap();
            let settings = cx.debug_bounds("header-settings").unwrap();
            assert!(header.contains(&settings.center()));
            assert_eq!(
                cx.debug_bounds("sidebar-canvas").unwrap().top(),
                header.bottom()
            );
        }
    }

    #[gpui::test]
    fn ordinary_sidebar_controls_traverse_forward_and_backward(cx: &mut gpui::TestAppContext) {
        cx.update(init_projects);
        let (_, cx) = cx.add_window_view(Shell::new);
        draw(cx);
        let project = cx
            .debug_bounds("global-nav-projects")
            .expect("Projects navigation")
            .center();
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
    fn window_header_and_picker_remain_stable_in_fullscreen(cx: &mut gpui::TestAppContext) {
        cx.update(init_projects);
        let (_, cx) = cx.add_window_view(Shell::new);
        for zoom in [1.0, 1.5] {
            cx.update(|_, cx| cx.set_global(crate::zoom::ZoomState::new(zoom)));
            for width in [1280., 500., 320.] {
                cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(600.)));
                let mut previous = None;
                for fullscreen in [false, true, false] {
                    cx.update(|window, _| {
                        if window.is_fullscreen() != fullscreen {
                            window.toggle_fullscreen();
                        }
                    });
                    draw(cx);
                    let header = cx.debug_bounds("app-header").unwrap();
                    let rail = cx.debug_bounds("global-navigation").unwrap();
                    let canvas = cx.debug_bounds("sidebar-canvas").unwrap();
                    let picker = cx.debug_bounds("header-workspace").unwrap();
                    let settings = cx.debug_bounds("header-settings").unwrap();
                    assert_eq!(rail.top(), header.bottom());
                    assert_eq!(canvas.top(), header.bottom());
                    assert!(rail.contains(&picker.center()));
                    assert!(header.contains(&settings.center()));
                    let grid = cx.debug_bounds("header-grid").unwrap();
                    assert!(header.contains(&grid.center()));
                    assert!(grid.right() <= settings.left());
                    assert!(f32::from(header.right() - settings.right()) >= SPACE_3XL * zoom);
                    for selector in [
                        "window-close",
                        "window-minimize",
                        "window-fullscreen",
                        "sidebar-toggle",
                    ] {
                        assert!(cx.debug_bounds(selector).is_some(), "{selector}");
                    }
                    let geometry = (header, rail, canvas, picker, settings);
                    if let Some(prior) = previous {
                        assert_eq!(
                            geometry, prior,
                            "fullscreen shifted header at {width}px/{zoom}x"
                        );
                    }
                    previous = Some(geometry);
                }
            }
        }
    }

    #[gpui::test]
    fn sidebar_workspace_control_traverses_and_activates(cx: &mut gpui::TestAppContext) {
        cx.update(init_projects);
        let (_, cx) = cx.add_window_view(Shell::new);
        for zoom in [1.0, 1.5] {
            cx.update(|_, cx| cx.set_global(crate::zoom::ZoomState::new(zoom)));
            for width in [1280., 500., 320.] {
                cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(600.)));
                draw(cx);
                let rail = cx.debug_bounds("global-navigation").unwrap();
                let control = cx.debug_bounds("header-workspace").unwrap();
                assert!(rail.contains(&control.center()));
                assert!(control.bottom() <= cx.debug_bounds("global-nav-home").unwrap().top());
            }
        }
        cx.update(|_, cx| cx.set_global(crate::zoom::ZoomState::new(1.0)));
        cx.simulate_resize(gpui::size(gpui::px(500.), gpui::px(600.)));
        draw(cx);
        let trigger = cx.debug_bounds("header-workspace").unwrap();
        cx.simulate_click(trigger.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("workspace-popup").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        let source_focus =
            cx.update(|window, cx| window.focused(cx).expect("workspace trigger focus"));
        cx.simulate_keystrokes("tab");
        draw(cx);
        cx.update(|window, cx| assert_ne!(window.focused(cx), Some(source_focus.clone())));
        cx.simulate_keystrokes("shift-tab");
        draw(cx);
        cx.update(|window, cx| assert_eq!(window.focused(cx), Some(source_focus.clone())));
        for key in ["enter", "space"] {
            cx.simulate_keystrokes(key);
            draw(cx);
            let popup = cx.debug_bounds("workspace-popup").expect("workspace popup");
            assert_eq!(popup.left(), trigger.left());
            assert!(popup.top() >= trigger.bottom());
            cx.simulate_keystrokes("escape");
            draw(cx);
            assert!(cx.debug_bounds("workspace-popup").is_none());
            cx.update(|window, cx| assert_eq!(window.focused(cx), Some(source_focus.clone())));
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
        let projects = cx
            .debug_bounds("global-nav-projects")
            .expect("Projects destination");
        cx.simulate_click(projects.center(), gpui::Modifiers::default());
        draw(cx);
        let project = cx.debug_bounds("projects-open-0").expect("Open project");
        cx.simulate_click(project.center(), gpui::Modifiers::default());
        draw(cx);
        let menu = cx
            .debug_bounds("project-menu-0")
            .expect("project menu")
            .center();
        cx.simulate_mouse_down(menu, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(menu, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        cx.simulate_keystrokes("down down enter");
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
    fn footer_is_absent_and_transient_toast_does_not_reserve_layout(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.simulate_resize(gpui::size(
            gpui::px(crate::WINDOW_MIN_W),
            gpui::px(crate::WINDOW_MIN_H),
        ));
        draw(cx);
        assert!(cx.debug_bounds("statusbar").is_none());
        let canvas = cx.debug_bounds("global-navigation").unwrap();
        assert_eq!(f32::from(canvas.bottom()), crate::WINDOW_MIN_H);
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
        let toast = cx.debug_bounds("app-toast").expect("error toast");
        assert!(f32::from(toast.right()) <= crate::WINDOW_MIN_W);
        assert!(f32::from(toast.left()) >= 0.);
        assert_eq!(cx.debug_bounds("global-navigation").unwrap(), canvas);
        cx.executor()
            .advance_clock(crate::entities::toast::Toast::ttl(
                crate::entities::toast::ToastKind::Error,
            ));
        draw(cx);
        assert!(cx.debug_bounds("app-toast").is_none());
    }

    #[gpui::test]
    fn canvas_session_scope_changes_before_sidebar_render(cx: &mut gpui::TestAppContext) {
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
            // active workspace and the canvas's scope read.
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
    fn cross_project_section_is_hidden_in_sidebar(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        let (shell, cx) = cx.add_window_view(Shell::new);
        cx.update(|_, cx| {
            let runtime = shell.read(cx).runtime.clone();
            runtime
                .read(cx)
                .registry
                .clone()
                .update(cx, |registry, cx| {
                    registry.insert_meta_with_context(
                        "navigation".into(),
                        "/grove-shell-navigation-test".into(),
                        Agent::Claude,
                        vec![grove_core::session_meta::ContextRoot {
                            project: "WEB".into(),
                            wt_path: "/web/main".into(),
                        }],
                        None,
                    );
                    cx.notify();
                });
            runtime.read(cx).tree.clone().update(cx, |tree, cx| {
                tree.set_active_worktrees(
                    0,
                    vec![grove_core::git::Worktree {
                        path: "/grove-shell-navigation-test".into(),
                        branch: "main".into(),
                        mtime: None,
                        is_main: true,
                    }],
                );
                cx.notify();
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("multi-project-sessions").is_none());
        assert!(cx.debug_bounds("multi-project-anchor").is_none());
    }

    #[gpui::test]
    fn removed_header_launcher_preserves_selected_canvas_alignment(cx: &mut gpui::TestAppContext) {
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
            assert!(cx.debug_bounds("header-new-session").is_none());
        }

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
    fn collapsed_close_shortcut_confirms_from_terminal_focus(cx: &mut gpui::TestAppContext) {
        cx.update(init);
        cx.update(|cx| {
            cx.bind_keys(k::shell_bindings());
            cx.global_mut::<crate::settings::SettingsState>()
                .store
                .sidebar_collapsed = false;
        });
        let (shell, cx) = cx.add_window_view(Shell::new);
        let selected = cx.update(|_, cx| {
            let registry = shell.read(cx).runtime.read(cx).registry.clone();
            let terminal = cx.new(|cx| {
                crate::entities::terminal_session::TerminalSession::spawn_script(
                    "\0",
                    "/grove-shell-navigation-test",
                    cx,
                )
            });
            registry.update(cx, |registry, cx| {
                let id = registry.insert_meta(
                    "navigation".into(),
                    "/grove-shell-navigation-test".into(),
                    Agent::Terminal,
                );
                registry.attach(id, terminal, None);
                cx.notify();
                id
            })
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
        draw(cx);
        cx.update(|window, cx| {
            shell.read(cx).sidebar.clone().update(cx, |sidebar, cx| {
                sidebar.select_session_id(selected, window, cx);
            });
        });
        draw(cx);
        let prior = cx.update(|window, cx| {
            let prior = shell
                .read(cx)
                .sidebar
                .read(cx)
                .canvas_terminal_focus(selected, false, cx)
                .expect("selected terminal");
            prior.focus(window, cx);
            prior
        });
        let rail = cx.debug_bounds("global-navigation").unwrap();
        cx.simulate_keystrokes(&format!("{}w", k::platform_mod_prefix()));
        draw(cx);
        assert!(cx.debug_bounds("sidebar-confirmation").is_some());
        assert_eq!(cx.debug_bounds("global-navigation").unwrap(), rail);
        assert!(cx.debug_bounds("canvas-confirmation").is_none());
        let cancel = cx.debug_bounds("cancel-close").unwrap();
        let confirm = cx.debug_bounds("confirm-close").unwrap();
        assert_eq!(cancel.top(), confirm.top());
        assert!(cancel.right() <= confirm.left());
        assert!(f32::from(cancel.size.height) >= 24.0);
        assert!(f32::from(confirm.size.height) >= 24.0);
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            assert!(prior.is_focused(window));
            assert!(!shell.read(cx).sidebar.read(cx).confirmation_open());
            assert!(shell
                .read(cx)
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .meta(selected)
                .is_some());
        });
        cx.update(|window, cx| prior.focus(window, cx));
        cx.simulate_keystrokes(&format!("{}w", k::platform_mod_prefix()));
        draw(cx);
        assert!(cx.debug_bounds("sidebar-confirmation").is_some());
        for zoom in [1.0, 1.5] {
            cx.update(|_, cx| cx.set_global(crate::zoom::ZoomState::new(zoom)));
            cx.simulate_resize(gpui::size(gpui::px(420.0), gpui::px(640.0)));
            draw(cx);
            let popup = cx.debug_bounds("sidebar-confirmation").unwrap();
            assert!(popup.left() >= gpui::px(0.0));
            assert!(popup.right() <= gpui::px(420.0));
            let cancel = cx.debug_bounds("cancel-close").unwrap();
            let confirm = cx.debug_bounds("confirm-close").unwrap();
            assert_eq!(cancel.top(), confirm.top());
            assert!(cancel.left() >= popup.left());
            assert!(confirm.right() <= popup.right());
        }
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, _| assert!(prior.is_focused(window)));
    }

    #[gpui::test]
    fn command_palette_shortcut_works_after_closing_session_without_click(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(init);
        cx.update(|cx| cx.bind_keys(k::shell_bindings()));
        let (shell, cx) = cx.add_window_view(Shell::new);
        let selected = cx.update(|_, cx| {
            let registry = shell.read(cx).runtime.read(cx).registry.clone();
            let terminal = cx.new(|cx| {
                crate::entities::terminal_session::TerminalSession::spawn_script(
                    "\0",
                    "/grove-shell-navigation-test",
                    cx,
                )
            });
            registry.update(cx, |registry, cx| {
                let id = registry.insert_meta(
                    "navigation".into(),
                    "/grove-shell-navigation-test".into(),
                    Agent::Terminal,
                );
                registry.attach(id, terminal, None);
                cx.notify();
                id
            })
        });
        draw(cx);
        cx.update(|window, cx| {
            shell.read(cx).sidebar.clone().update(cx, |sidebar, cx| {
                sidebar.select_session_id(selected, window, cx);
            });
        });
        draw(cx);
        cx.simulate_keystrokes(&format!("{}w", k::platform_mod_prefix()));
        draw(cx);
        assert!(cx.debug_bounds("sidebar-confirmation").is_some());
        cx.simulate_keystrokes("tab enter");
        draw(cx);
        cx.update(|_, cx| {
            let shell = shell.read(cx);
            assert!(shell
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .meta(selected)
                .is_none());
            assert!(!shell.sidebar.read(cx).confirmation_open());
            assert_eq!(shell.sidebar.read(cx).selected_session(), None);
        });
        cx.simulate_keystrokes(&format!("{}p", k::platform_mod_prefix()));
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).launcher.read(cx).is_open()));
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
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        assert!(cx.debug_bounds("global-navigation").is_some());
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
        assert!(cx.debug_bounds("app-header").is_some());
        cx.update(|window, cx| window.dispatch_action(Box::new(k::Settings), cx));
        draw(cx);
        assert!(cx.debug_bounds("app-header").is_some());
        assert!(cx.debug_bounds("global-navigation").is_some());
        assert!(cx.debug_bounds("sidebar-rail").is_none());
        assert!(cx.debug_bounds("global-navigation").is_some());
        assert_eq!(
            cx.debug_bounds("settings-panel"),
            cx.debug_bounds("sidebar-canvas")
        );
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx.update(|_, cx| shell.read(cx).sidebar.read(cx).is_zen()));
        assert!(cx.debug_bounds("app-header").is_some());
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
    fn grid_workspace_switch_lands_on_dashboard(cx: &mut gpui::TestAppContext) {
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
            shell.read(cx).sidebar.clone().update(cx, |sidebar, cx| {
                sidebar.select_session_id(prior, window, cx);
            });
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
        cx.update(|window, cx| {
            let sidebar = shell.read(cx).sidebar.clone();
            let sidebar = sidebar.read(cx);
            assert!(sidebar.is_grid());
            assert_eq!(
                sidebar.active_canvas_sessions(cx),
                vec![(first, false), (second, false)]
            );
            assert!(!prior_focus.is_focused(window));
        });
        assert!(cx.debug_bounds("dashboard-summary").is_some());
        assert!(cx.debug_bounds("codex-composer").is_some());
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
        assert!(cx.debug_bounds("dashboard-summary").is_some());
        assert!(cx.debug_bounds("codex-composer").is_some());
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
            assert!(!prior_focus.is_focused(window));
        });
        assert!(cx.debug_bounds("dashboard-summary").is_some());
        assert!(cx.debug_bounds("codex-composer").is_some());
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
        let point = cx.debug_bounds("header-settings").unwrap().center();
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
}
