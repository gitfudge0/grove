//! Persistent workspace selection and management.
use super::{components::header_control, rpx, tokens::*};
use crate::{icons::icon, theme as c};
use gpui::{
    div, prelude::*, App, Context, Entity, FocusHandle, Focusable, FontWeight, MouseButton,
    Subscription, Window,
};
use gpui_component::input::{Input, InputEvent, InputState};

const MENU_W: f32 = 238.0;
const CREATE_W: f32 = 432.0;
const MANAGER_W: f32 = 648.0;
const ROW_H: f32 = 28.0;
const FIELD_H: f32 = 60.0;
const FIELD_VALUE: f32 = 16.0;
const FIELD_INSET: f32 = 14.0;
const SELECTOR_H: f32 = 32.0;
const MENU_ROW_H: f32 = 40.0;
const MENU_ROW_RADIUS: f32 = 8.0;

use crate::settings::SettingsState;
use grove_core::storage::Workspaces;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Panel {
    Closed,
    Menu,
    Create,
    Manage,
    Rename(u64),
    Delete(u64),
}

pub struct WorkspaceManager {
    state: Workspaces,
    panel: Panel,
    focus: FocusHandle,
    input: Entity<InputState>,
    selected: usize,
    error: Option<String>,
    _subscription: Subscription,
    _settings_subscription: Subscription,
    menu_scroll: gpui::ScrollHandle,
    trigger_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    popup_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
}
impl WorkspaceManager {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. Platform"));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.submit(window, cx),
                InputEvent::Change => {
                    this.error = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        let settings_subscription = cx.observe_global::<SettingsState>(|this, cx| {
            this.state = cx.global::<SettingsState>().store.workspaces.clone();
            cx.notify();
        });
        Self {
            state: cx
                .try_global::<SettingsState>()
                .map(|settings| settings.store.workspaces.clone())
                .unwrap_or_default(),
            panel: Panel::Closed,
            focus: cx.focus_handle().tab_stop(true),
            input,
            selected: 0,
            error: None,
            _subscription: subscription,
            _settings_subscription: settings_subscription,
            trigger_bounds: std::rc::Rc::default(),
            popup_bounds: std::rc::Rc::default(),
            menu_scroll: gpui::ScrollHandle::new(),
        }
    }
    fn persist(&mut self, cx: &mut Context<Self>) -> bool {
        if cx.try_global::<SettingsState>().is_none() {
            return true;
        }
        let state = self.state.clone();
        let ((), saved) =
            SettingsState::update_and_flush_checked(cx, |store| store.workspaces = state);
        if let Err(error) = saved {
            self.state = cx.global::<SettingsState>().store.workspaces.clone();
            self.error = Some(format!("Could not save workspace: {error}"));
            cx.notify();
            return false;
        }
        true
    }
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.panel = Panel::Closed;
        self.error = None;
        self.focus.focus(window, cx);
        cx.notify();
    }
    fn open(&mut self, panel: Panel, window: &mut Window, cx: &mut Context<Self>) {
        self.panel = panel;
        self.error = None;
        let value = match panel {
            Panel::Rename(id) => self.state.name(id).to_string(),
            _ => String::new(),
        };
        let placeholder = match panel {
            Panel::Delete(id) => self.state.name(id).to_string(),
            _ => "e.g. Platform".to_string(),
        };
        if matches!(panel, Panel::Create | Panel::Rename(_) | Panel::Delete(_)) {
            self.input.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx);
                input.set_value(value, window, cx);
                input.focus(window, cx);
            });
        } else {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value().to_string();
        let result = match self.panel {
            Panel::Create => self.state.create(&value),
            Panel::Rename(id) => self.state.rename(id, &value),
            Panel::Delete(id) => self.state.delete(id, &value),
            _ => return,
        };
        match result {
            Ok(()) => {
                if !self.persist(cx) {
                    return;
                }
                if self.panel == Panel::Create {
                    self.close(window, cx);
                } else {
                    self.open(Panel::Manage, window, cx);
                }
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
    fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected.cmp(&self.state.rows.len()) {
            std::cmp::Ordering::Less => {
                self.state.select(self.state.rows[self.selected].id);
                if !self.persist(cx) {
                    return;
                }
                self.close(window, cx);
            }
            std::cmp::Ordering::Equal => self.open(Panel::Create, window, cx),
            std::cmp::Ordering::Greater => self.open(Panel::Manage, window, cx),
        }
    }
    fn field(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let deleting = matches!(self.panel, Panel::Delete(_));
        let value = self.input.read(cx).value();
        let enabled = if let Panel::Delete(id) = self.panel {
            value.as_ref() == self.state.name(id) && self.state.delete_guard(id).is_ok()
        } else {
            !value.trim().is_empty()
        };
        let field_fill = if self.input.read(cx).focus_handle(cx).is_focused(window) {
            c::BG_HOVER()
        } else {
            c::FIELD_FILL()
        };
        div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_LG))
            .h(rpx(FIELD_H))
            .px(rpx(FIELD_INSET))
            .rounded(rpx(RADIUS_PANEL))
            .bg(field_fill)
            .border_1()
            .border_color(if self.error.is_some() {
                c::FORM_ERROR()
            } else {
                field_fill
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(rpx(TEXT_BODY))
                            .text_color(c::FG_DIM())
                            .child(if deleting {
                                "Confirm workspace name"
                            } else {
                                "Workspace"
                            }),
                    )
                    .child(
                        div()
                            .id("workspace-name-input")
                            .aria_label(if deleting {
                                "Confirm workspace name"
                            } else {
                                "Workspace name"
                            })
                            .child(
                                Input::new(&self.input)
                                    .aria_label(if deleting {
                                        "Confirm workspace name"
                                    } else {
                                        "Workspace name"
                                    })
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .text_size(rpx(FIELD_VALUE))
                                    .text_color(c::FG())
                                    .p_0(),
                            ),
                    ),
            )
            .child(
                header_control(
                    "workspace-submit",
                    if deleting {
                        "Delete workspace"
                    } else {
                        "Save workspace"
                    },
                )
                .when(enabled, |el| el.tab_index(0))
                .focus_visible(|s| s.bg(c::BG_HOVER()))
                .when(!enabled, |el| el.opacity(OPACITY_DISABLED))
                .when(enabled, |el| {
                    el.on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))
                })
                .child(icon(
                    if deleting { "trash" } else { "check" },
                    ICON_LG,
                    if deleting { c::FORM_ERROR() } else { c::FG() },
                )),
            )
    }
    fn popup(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menu = self.panel == Panel::Menu;
        let width = if menu {
            MENU_W
        } else if self.panel == Panel::Create {
            CREATE_W
        } else {
            MANAGER_W
        };
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width =
            width.min((f32::from(window.viewport_size().width) / scale - SPACE_3XL).max(0.0));
        let max_height =
            (f32::from(window.viewport_size().height) / scale - APPBAR_H - SPACE_3XL).max(FIELD_H);
        let popup_bounds = self.popup_bounds.clone();
        let mut panel = div()
            .id("workspace-popup")
            .debug_selector(|| "workspace-popup".into())
            .relative()
            .w(rpx(width))
            .max_h(rpx(max_height))
            .flex()
            .flex_col()
            .p(rpx(SPACE_2XL))
            .when(!menu, |el| el.gap(rpx(SPACE_LG)))
            .border_1()
            .border_color(c::BORDER())
            .rounded(rpx(RADIUS_PANEL))
            .bg(if menu { c::SURFACE_RAISED() } else { c::BG() })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        panel = panel.child(
            gpui::canvas(
                move |bounds, _, _| popup_bounds.set(bounds),
                |_, (), _, _| {},
            )
            .absolute()
            .size_full(),
        );
        if menu {
            panel = panel.role(gpui::Role::Menu).aria_label("Switch workspace");
            let mut rows = div()
                .id("workspace-menu-list")
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.menu_scroll);
            for (index, row) in self.state.rows.iter().enumerate() {
                let id = row.id;
                rows = rows.child(
                    div()
                        .id(("workspace", id))
                        .role(gpui::Role::MenuItem)
                        .aria_label(row.name.clone())
                        .min_h(rpx(MENU_ROW_H))
                        .py(rpx(SPACE_LG))
                        .text_size(rpx(TEXT_TITLE))
                        .font_weight(FontWeight::MEDIUM)
                        .px(rpx(SPACE_2XL))
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .rounded(rpx(MENU_ROW_RADIUS))
                        .when(self.selected == index, |el| el.bg(c::BG_HOVER()))
                        .hover(|s| s.bg(c::BG_HOVER()))
                        .child(icon("folder", ICON_SM, c::FG_DIM()))
                        .child(div().flex_1().truncate().child(row.name.clone()))
                        .when(self.state.active == id, |el| {
                            el.child(icon("check", ICON_SM, c::FG()))
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.state.select(id);
                            if !this.persist(cx) {
                                return;
                            }
                            this.close(window, cx);
                        })),
                );
            }
            panel = panel.child(rows).child(
                div()
                    .flex_shrink_0()
                    .my(rpx(SPACE_SM))
                    .border_t_1()
                    .border_color(c::BORDER()),
            );
            let mut actions = div().flex().flex_col().flex_shrink_0();
            for (offset, (label, glyph, destination)) in [
                ("Create workspace", "plus", Panel::Create),
                ("Manage workspaces", "more", Panel::Manage),
            ]
            .into_iter()
            .enumerate()
            {
                actions = actions.child(
                    div()
                        .id(("workspace-action", offset))
                        .debug_selector(move || format!("workspace-action-{offset}"))
                        .role(gpui::Role::MenuItem)
                        .aria_label(label)
                        .min_h(rpx(MENU_ROW_H))
                        .py(rpx(SPACE_LG))
                        .text_size(rpx(TEXT_TITLE))
                        .font_weight(FontWeight::MEDIUM)
                        .flex_shrink_0()
                        .px(rpx(SPACE_2XL))
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .rounded(rpx(MENU_ROW_RADIUS))
                        .when(self.selected == self.state.rows.len() + offset, |el| {
                            el.bg(c::BG_HOVER())
                        })
                        .hover(|s| s.bg(c::BG_HOVER()))
                        .child(icon(glyph, ICON_SM, c::FG_DIM()))
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open(destination, window, cx);
                        })),
                );
            }
            panel = panel.child(actions);
        } else {
            panel = panel
                .role(gpui::Role::Dialog)
                .aria_label("Workspace management")
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .flex_shrink_0()
                        .child(
                            div()
                                .font_weight(FontWeight::BOLD)
                                .text_size(rpx(TEXT_BRAND))
                                .child(if self.panel == Panel::Create {
                                    "New workspace"
                                } else {
                                    "Manage workspaces"
                                }),
                        )
                        .child(
                            header_control("close-workspace-panel", "Close workspace panel")
                                .tab_index(0)
                                .focus_visible(|s| s.bg(c::BG_HOVER()))
                                .child(icon("close", ICON_MD, c::FG_DIM()))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.close(window, cx)),
                                ),
                        ),
                );
            let mut body = div()
                .id("workspace-manager-body")
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG));
            if self.panel != Panel::Create {
                for row in &self.state.rows {
                    let id = row.id;
                    let mut controls = div().flex().items_center().gap(rpx(SPACE_SM));
                    for (key, label, glyph) in [
                        ("rename", "Rename workspace", "edit"),
                        ("delete", "Delete workspace", "trash"),
                    ] {
                        controls = controls.child(
                            div()
                                .id((key, id))
                                .role(gpui::Role::Button)
                                .tab_index(0)
                                .focus_visible(|s| s.bg(c::BG_HOVER()))
                                .aria_label(format!("{} {}", label, row.name))
                                .size(rpx(CHROME_CONTROL_H))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(rpx(RADIUS_CONTROL))
                                .hover(|s| s.bg(c::BG_HOVER()))
                                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if key == "rename" {
                                        this.open(Panel::Rename(id), window, cx);
                                    } else if let Err(error) = this.state.delete_guard(id) {
                                        this.error = Some(error);
                                        cx.notify();
                                    } else {
                                        this.open(Panel::Delete(id), window, cx);
                                    }
                                }))
                                .child(icon(
                                    glyph,
                                    ICON_SM,
                                    if key == "delete" {
                                        c::FORM_ERROR()
                                    } else {
                                        c::FG_DIM()
                                    },
                                )),
                        );
                    }
                    body = body.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(rpx(SPACE_LG))
                            .py(rpx(SPACE_LG))
                            .border_b_1()
                            .border_color(c::BORDER())
                            .child(icon("folder", ICON_MD, c::FG_DIM()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().truncate().child(row.name.clone()))
                                    .child(
                                        div()
                                            .text_size(rpx(TEXT_SMALL))
                                            .text_color(c::FG_DIM())
                                            .child(format!(
                                                "{} projects{}",
                                                row.projects,
                                                if id == self.state.active {
                                                    " · Current"
                                                } else {
                                                    ""
                                                }
                                            )),
                                    ),
                            )
                            .child(controls),
                    );
                }
            }
            if let Panel::Delete(id) = self.panel {
                body = body.child(div().text_color(c::FG_DIM()).child(format!(
                    "Delete {}? Type its exact name to confirm.",
                    self.state.name(id)
                )));
            }
            if matches!(
                self.panel,
                Panel::Create | Panel::Rename(_) | Panel::Delete(_)
            ) {
                body = body.child(self.field(window, cx));
            }
            if let Some(error) = &self.error {
                body = body.child(
                    div()
                        .id("workspace-validation-error")
                        .role(gpui::Role::Alert)
                        .text_size(rpx(TEXT_BODY))
                        .text_color(c::FORM_ERROR())
                        .child(error.clone()),
                );
            }
            panel = panel.child(body).child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .flex_shrink_0()
                    .child("Workspace changes are saved automatically."),
            );
            if self.panel != Panel::Create {
                panel = panel.child(
                    div()
                        .id("manager-create-workspace")
                        .tab_index(0)
                        .focus_visible(|s| s.bg(c::BG_HOVER()))
                        .role(gpui::Role::Button)
                        .aria_label("Create workspace")
                        .h(rpx(ROW_H))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .hover(|s| s.bg(c::BG_HOVER()))
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                        })
                        .child(icon("plus", ICON_SM, c::FG()))
                        .child("Create workspace")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open(Panel::Create, window, cx)),
                        ),
                );
            }
        }
        gpui::anchored()
            .position(gpui::point(
                self.trigger_bounds.get().left(),
                self.trigger_bounds.get().bottom() + gpui::px(SPACE_MD * scale),
            ))
            .snap_to_window_with_margin(gpui::px(SPACE_LG * scale))
            .child(panel)
    }
}
impl Focusable for WorkspaceManager {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for WorkspaceManager {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let trigger_bounds = self.trigger_bounds.clone();
        div()
            .id("workspace-management")
            .flex()
            .items_start()
            .tab_group()
            .relative()
            .min_w_0()
            .text_size(rpx(TEXT_BODY))
            .text_color(c::FG())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    if this.panel != Panel::Closed
                        && !this.popup_bounds.get().contains(&event.position)
                    {
                        this.close(window, cx);
                    }
                }),
            )
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "tab" if this.panel != Panel::Closed => {
                        window.prevent_default();
                        if event.keystroke.modifiers.shift {
                            window.focus_prev(cx);
                        } else {
                            window.focus_next(cx);
                        }
                        cx.stop_propagation();
                    }
                    "escape" if this.panel != Panel::Closed => {
                        this.close(window, cx);
                        cx.stop_propagation();
                    }
                    "down" | "up" if this.panel == Panel::Menu => {
                        let count = this.state.rows.len() + 2;
                        this.selected = if event.keystroke.key == "down" {
                            (this.selected + 1) % count
                        } else {
                            (this.selected + count - 1) % count
                        };
                        this.menu_scroll.scroll_to_item(
                            this.selected.min(this.state.rows.len().saturating_sub(1)),
                        );
                        cx.notify();
                        cx.stop_propagation();
                    }
                    "enter" | "space" if this.focus.is_focused(window) => {
                        window.prevent_default();
                        if this.panel == Panel::Menu {
                            this.choose(window, cx);
                        } else {
                            this.open(Panel::Menu, window, cx);
                        }
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(
                div()
                    .id("workspace-picker")
                    .debug_selector(|| "workspace-picker".into())
                    .track_focus(&self.focus)
                    .role(gpui::Role::Button)
                    .aria_label(format!(
                        "Switch workspace, {} selected",
                        self.state.name(self.state.active)
                    ))
                    .h(rpx(SELECTOR_H))
                    .px(rpx(SPACE_2XL))
                    .relative()
                    .min_w_0()
                    .max_w(rpx(MENU_W))
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_MD))
                    .rounded(rpx(RADIUS_PANEL))
                    .hover(|s| s.bg(c::BG_HOVER()))
                    .focus_visible(|s| s.bg(c::BG_HOVER()))
                    .when(self.panel != Panel::Closed, |el| el.bg(c::BG_HOVER()))
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .child(
                        gpui::canvas(
                            move |bounds, _, _| trigger_bounds.set(bounds),
                            |_, (), _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(rpx(TEXT_BRAND))
                            .line_height(rpx(CHROME_CONTROL_H))
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.state.name(self.state.active).to_string()),
                    )
                    .child(icon("chev-down", ICON_SM, c::FG_DIM()))
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.panel == Panel::Closed {
                            this.selected = this
                                .state
                                .rows
                                .iter()
                                .position(|row| row.id == this.state.active)
                                .unwrap_or(0);
                            this.open(Panel::Menu, window, cx);
                        } else {
                            this.close(window, cx);
                        }
                    })),
            )
            .when(self.panel != Panel::Closed, |el| {
                el.child(gpui::deferred(self.popup(window, cx)))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_trimmed_case_insensitive_names_and_preserves_identity() {
        let mut state = Workspaces::new();
        assert!(state.create(" grove ").is_err());
        assert!(state.create(" ").is_err());
        state.create(" Platform ").unwrap();
        assert_eq!(state.name(state.active), "Platform");
        let id = state.active;
        state.rename(id, "PLATFORM").unwrap();
        assert_eq!(state.active, id);
    }
    #[test]
    fn delete_requires_exact_name_and_uses_mru_not_order() {
        let mut state = Workspaces::new();
        state.create("Second").unwrap();
        let second = state.active;
        state.create("Third").unwrap();
        let third = state.active;
        state.select(second);
        state.select(third);
        assert!(state.delete(third, "third").is_err());
        state.delete(third, "Third").unwrap();
        assert_eq!(state.active, second);
    }
    #[test]
    fn deletion_protects_final_and_nonempty_workspaces() {
        let mut state = Workspaces::new();
        assert!(state.delete(1, "Grove").is_err());
        state.create("Second").unwrap();
        state.rows[0].projects = 1;
        assert!(state.delete(1, "Grove").is_err());
    }
    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn popup_mouse_down_does_not_dismiss_before_click(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (manager, cx) = cx.add_window_view(WorkspaceManager::new);
        cx.update(|window, cx| {
            manager.update(cx, |manager, cx| manager.open(Panel::Menu, window, cx));
        });
        draw(cx);
        let point = cx.debug_bounds("workspace-action-0").unwrap().center();
        cx.simulate_mouse_down(point, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert_eq!(
            manager.read_with(cx, |manager, _| manager.panel),
            Panel::Menu
        );
        cx.simulate_mouse_up(point, MouseButton::Left, gpui::Modifiers::default());
        draw(cx);
        assert_eq!(
            manager.read_with(cx, |manager, _| manager.panel),
            Panel::Create
        );
    }

    #[gpui::test]
    fn picker_enter_and_space_activate_once_per_press_release(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (manager, cx) = cx.add_window_view(WorkspaceManager::new);
        cx.update(|window, cx| manager.update(cx, |manager, cx| manager.close(window, cx)));
        draw(cx);
        for key in ["enter", "space"] {
            let keystroke = gpui::Keystroke::parse(key).unwrap();
            cx.simulate_event(gpui::KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            });
            draw(cx);
            assert_eq!(
                manager.read_with(cx, |manager, _| manager.panel),
                Panel::Menu
            );
            cx.simulate_event(gpui::KeyUpEvent { keystroke });
            draw(cx);
            assert_eq!(
                manager.read_with(cx, |manager, _| manager.panel),
                Panel::Menu
            );
            cx.update(|window, cx| manager.update(cx, |manager, cx| manager.close(window, cx)));
            draw(cx);
        }
    }
    struct HeaderFixture {
        manager: Entity<WorkspaceManager>,
    }
    impl Render for HeaderFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(c::BG())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_shrink_0()
                        .h(rpx(APPBAR_H))
                        .px(rpx(SPACE_2XL))
                        .child(div().w(rpx(MENU_W)).child("GROVE"))
                        .child(div().ml(rpx(SPACE_2XL)).child(self.manager.clone()))
                        .child(div().flex_1().h_full()),
                )
                .child(div().flex_1().min_h_0())
        }
    }
    #[gpui::test]
    fn popup_has_visible_bounds_when_nested_in_app_header(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (root, cx) = cx.add_window_view(|window, cx| HeaderFixture {
            manager: cx.new(|cx| WorkspaceManager::new(window, cx)),
        });
        let manager = root.read_with(cx, |root, _| root.manager.clone());
        draw(cx);
        cx.update(|window, cx| {
            manager.update(cx, |manager, cx| manager.open(Panel::Menu, window, cx));
        });
        draw(cx);
        let popup = cx.debug_bounds("workspace-popup").unwrap();
        let create = cx.debug_bounds("workspace-action-0").unwrap();
        assert!(f32::from(popup.size.height) > APPBAR_H);
        assert!(f32::from(popup.top()) >= APPBAR_H);
        assert!(popup.contains(&create.center()));
        cx.simulate_mouse_down(
            create.center(),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        draw(cx);
        assert_eq!(
            manager.read_with(cx, |manager, _| manager.panel),
            Panel::Menu
        );
        cx.simulate_mouse_up(
            create.center(),
            MouseButton::Left,
            gpui::Modifiers::default(),
        );
        draw(cx);
        assert_eq!(
            manager.read_with(cx, |manager, _| manager.panel),
            Panel::Create
        );
    }
}
