//! Grove's app-owned header and the empty canvas for the UI rebuild.
mod switcher;

use super::components::{form_action, header_control_with_label_tooltip};
use super::motion;
use super::settings_panel::{SettingsPanel, SettingsPanelEvent};
use super::worktree_launcher::WorktreeLauncherEvent;
use super::{rpx, tokens::*};
use crate::platform::window_chrome::{self, ControlPlatform, WindowAction};
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
const MAC_FIXED_HEADER_W: f32 = TRAFFIC_CONTROL_W * 3.0 + CHROME_CONTROL_H + SPACE_LG * 4.0;

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
        // GPUI propagates child invalidation through the rendered view path.
        // Sidebar events below own semantic shell actions; forwarding every
        // notification here duplicates the engine's ancestor invalidation.
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
                    SettingsPanelEvent::UseCheckout { root, path } => {
                        this.sidebar.update(cx, |sidebar, cx| {
                            sidebar
                                .open_project_checkout_from_settings(root, path, false, window, cx);
                        });
                    }
                    SettingsPanelEvent::NewWorktree(root) => {
                        this.sidebar.update(cx, |sidebar, cx| {
                            sidebar
                                .open_project_checkout_from_settings(root, root, true, window, cx);
                        });
                    }
                    SettingsPanelEvent::AddProject => {
                        this.settings
                            .update(cx, |panel, cx| panel.close(window, cx));
                        this.sidebar.update(cx, |sidebar, cx| {
                            sidebar.add_project_from_palette(window, cx);
                        });
                    }
                    SettingsPanelEvent::ConnectionsChanged(provider) => this
                        .sidebar
                        .update(cx, |sidebar, cx| sidebar.credentials_changed(*provider, cx)),
                    SettingsPanelEvent::OpenHighlights { debug } => {
                        this.settings
                            .update(cx, |panel, cx| panel.close(window, cx));
                        this.open_highlights(*debug, window, cx);
                    }
                },
            );
        let sidebar_events =
            cx.subscribe_in(&sidebar, window, |this, _, event, window, cx| match event {
                super::sidebar::SidebarEvent::NewSession => {
                    this.open_new_session(window, cx);
                }
                #[cfg(test)]
                super::sidebar::SidebarEvent::NewMultiProjectSession => {
                    if !this.shortcut_blocked_in(window, cx) {
                        this.sidebar
                            .update(cx, super::sidebar::Sidebar::dismiss_project_context);
                        this.launcher
                            .update(cx, |launcher, cx| launcher.open_multi_project(window, cx));
                    }
                }
                super::sidebar::SidebarEvent::Settings => {
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
                "window-maximize",
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

    fn window_controls(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let platform = ControlPlatform::current();
        let decorations = window.window_decorations();
        // GPUI TestWindow cannot negotiate client decorations. These layout fixtures
        // exercise the client-owned header; policy tests cover server ownership.
        #[cfg(test)]
        let decorations = {
            let _ = decorations;
            gpui::Decorations::Client {
                tiling: gpui::Tiling::default(),
            }
        };
        let mut group = div()
            .group("traffic-lights")
            .flex()
            .flex_shrink_0()
            .items_center();
        for action in window_chrome::controls(platform, decorations, window.window_controls()) {
            let label = action.label(window.is_fullscreen(), window.is_maximized());
            let handler: fn(&mut Self, &mut Window, &mut Context<Self>) = match action {
                WindowAction::Close => Self::close_after_codex,
                WindowAction::Minimize => |_, window, _| window.minimize_window(),
                WindowAction::Fullscreen => |_, window, cx| {
                    window.toggle_fullscreen();
                    cx.notify();
                },
                WindowAction::Maximize => |_, window, cx| {
                    window.zoom_window();
                    cx.notify();
                },
            };
            let control = self.header_action(action.id(), label, handler, cx);
            if platform == ControlPlatform::Mac {
                let (color, glyph) = match action {
                    WindowAction::Close => (c::WINDOW_CLOSE(), "close"),
                    WindowAction::Minimize => (c::WINDOW_MINIMIZE(), "minus"),
                    _ => (c::WINDOW_FULLSCREEN(), "plus"),
                };
                group = group.child(
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
                    ),
                );
            } else {
                let glyph = match action {
                    WindowAction::Close => "close",
                    WindowAction::Minimize => "minus",
                    WindowAction::Maximize if window.is_maximized() => "window-restore",
                    _ => "window-maximize",
                };
                group = group.child(control.child(icon(glyph, ICON_SM, c::FG_DIM())));
            }
        }
        group
    }
    fn window_drag_area(id: &'static str) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .debug_selector(move || id.into())
            .on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
    }

    fn header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let collapsed = self.sidebar.read(cx).navigation_collapsed(window, cx);
        let compact = f32::from(window.viewport_size().width)
            / (f32::from(window.rem_size()) / crate::zoom::REM_BASE)
            < 400.0;
        let mac = ControlPlatform::current() == ControlPlatform::Mac;
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
            .when(mac, |header| header.child(self.window_controls(window, cx)))
            .when(compact, |header| header.child(div().flex_1()))
            .when(!compact, |header| {
                header.child(
                    div().flex_1().min_w_0().child(
                        self.sidebar
                            .update(cx, |sidebar, cx| sidebar.header_workspace_control(cx)),
                    ),
                )
            })
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
            .when(!mac, |header| {
                header.child(
                    Self::window_drag_area("header-main-drag-region")
                        .absolute()
                        .top_0()
                        .right_0()
                        .w(rpx(SPACE_LG))
                        .min_w_0()
                        .h_full(),
                )
            })
    }

    fn window_header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mac = ControlPlatform::current() == ControlPlatform::Mac;
        let sidebar = self.sidebar.read(cx);
        let sidebar_width = sidebar.animated_sidebar_width(window, cx);
        let drag_width = if mac
            && sidebar_width == 0.0
            && sidebar.navigation_collapsed(window, cx)
            && !sidebar.is_grid()
            && !sidebar.is_zen()
        {
            // The fixed controls still have a top inset when the rail is
            // closed. Leave the full canvas interactive in Grid and Zen.
            MAC_FIXED_HEADER_W
        } else {
            sidebar_width
        };
        let leading_width = if mac {
            sidebar_width
        } else {
            sidebar_width.max(CHROME_CONTROL_H + SPACE_LG * 2.0)
        };
        div()
            .id("window-header")
            .debug_selector(|| "window-header".into())
            .absolute()
            .top_0()
            .left_0()
            .h(rpx(APPBAR_TOTAL_H))
            .flex()
            .items_center()
            .bg(c::BG_RAIL())
            .when(mac, |row| row.w(rpx(leading_width)))
            .when(!mac, gpui::Styled::w_full)
            .child(
                div()
                    .w(rpx(leading_width))
                    .flex_shrink_0()
                    .child(self.header(window, cx)),
            )
            .when(mac && drag_width > SPACE_LG * 2.0, |row| {
                row.child(
                    Self::window_drag_area("mac-titlebar-drag-region")
                        .absolute()
                        .top_0()
                        .left(rpx(SPACE_LG))
                        .w(rpx(drag_width - SPACE_LG * 2.0))
                        .h(rpx(APPBAR_TOP_INSET))
                        .block_mouse_except_scroll(),
                )
            })
            .when(!mac, |row| {
                row.child(
                    Self::window_drag_area("window-header-drag-region")
                        .flex_1()
                        .min_w_0()
                        .h_full(),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .pt(rpx(APPBAR_TOP_INSET))
                        .pr(rpx(SPACE_LG))
                        .child(self.window_controls(window, cx)),
                )
            })
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
            crate::platform::text_preferences::sync_component_size(cx);
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

    fn open_new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shortcut_blocked_in(window, cx) {
            self.sidebar.update(cx, |sidebar, cx| {
                sidebar.dismiss_project_context(cx);
                sidebar.open_dashboard(window, cx);
            });
        }
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
            sidebar.set_grid_blocked_by_shell(project_hover_blocked, cx);
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
                cx.observe_window_bounds(window, |_, _, cx| cx.notify()),
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
                    && (this
                        .workspaces
                        .update(cx, |manager, cx| manager.dismiss_hover_menu(window, cx))
                        || super::inline_html::dismiss_expanded(window, cx))
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
            .child(self.window_header(window, cx))
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
mod tests;
