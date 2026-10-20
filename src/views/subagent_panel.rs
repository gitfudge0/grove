//! Provider-independent observed child activity, anchored above its session's input.
use super::{
    components::{tooltip, transcript_action},
    rpx,
    tokens::*,
};
use crate::theme as c;
use gpui::{
    div, prelude::*, App, Bounds, Context, EventEmitter, FocusHandle, Focusable, Pixels, Render,
    SharedString, Window,
};
use grove_core::codex::{Subagent, SubagentStatus};
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

const PANEL_W: f32 = 480.;
const PANEL_MAX_H: f32 = 280.;
const ROLE_MAX_W: f32 = 120.;

#[derive(Clone)]
pub(crate) enum SubagentPanelEvent {
    Stop(String),
}

pub(crate) struct SubagentPanel {
    scope: String,
    agents: Vec<Subagent>,
    open: bool,
    trigger_focus: FocusHandle,
    panel_focus: FocusHandle,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    scroll: gpui::ScrollHandle,
    stoppable: HashSet<String>,
    stopping: HashSet<String>,
    errors: HashMap<String, String>,
    focus_subscription: Option<gpui::Subscription>,
}
impl EventEmitter<SubagentPanelEvent> for SubagentPanel {}
impl SubagentPanel {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            scope: String::new(),
            agents: Vec::new(),
            open: false,
            trigger_focus: cx.focus_handle().tab_stop(true),
            panel_focus: cx.focus_handle().tab_stop(true),
            bounds: Rc::default(),
            scroll: gpui::ScrollHandle::new(),
            stoppable: HashSet::new(),
            stopping: HashSet::new(),
            errors: HashMap::new(),
            focus_subscription: None,
        }
    }
    pub(crate) fn set_agents(
        &mut self,
        scope: String,
        agents: Vec<Subagent>,
        cx: &mut Context<Self>,
    ) {
        let agents: Vec<_> = agents
            .into_iter()
            .filter(|agent| {
                matches!(
                    agent.status,
                    SubagentStatus::Working | SubagentStatus::Unknown
                )
            })
            .collect();
        let changed = self.scope != scope
            || self.agents.len() != agents.len()
            || self.agents.iter().zip(&agents).any(|(a, b)| {
                a.id != b.id
                    || a.name != b.name
                    || a.role != b.role
                    || a.task != b.task
                    || a.activity != b.activity
                    || a.status != b.status
            });
        if self.scope != scope {
            self.open = false;
            self.scope = scope;
            self.stoppable.clear();
            self.stopping.clear();
            self.errors.clear();
            self.scroll = gpui::ScrollHandle::new();
        }
        self.agents = agents;
        self.stopping
            .retain(|id| self.agents.iter().any(|agent| &agent.id == id));
        self.errors
            .retain(|id, _| self.agents.iter().any(|agent| &agent.id == id));
        if self.agents.is_empty() {
            self.open = false;
        }
        if changed {
            cx.notify();
        }
    }
    /// Only targets independently validated by a provider adapter may enable Stop.
    pub(crate) fn set_stop_targets(&mut self, targets: HashSet<String>, cx: &mut Context<Self>) {
        if self.stoppable != targets {
            self.stoppable = targets;
            cx.notify();
        }
    }
    pub(crate) fn stop_failed(&mut self, id: String, error: String, cx: &mut Context<Self>) {
        self.stopping.remove(&id);
        self.errors.insert(id, error);
        cx.notify();
    }
    fn request_stop(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.stoppable.contains(id)
            && self.agents.iter().any(|agent| agent.id == id)
            && self.stopping.insert(id.into())
        {
            self.errors.remove(id);
            cx.emit(SubagentPanelEvent::Stop(id.into()));
            cx.notify();
        }
    }
    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = !self.open;
        if self.open {
            self.panel_focus.focus(window, cx);
        } else {
            self.trigger_focus.focus(window, cx);
        }
        cx.notify();
    }
    fn dismiss(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        if restore {
            self.trigger_focus.focus(window, cx);
        }
        cx.notify();
    }
}
impl Focusable for SubagentPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.trigger_focus.clone()
    }
}
fn summary(agents: &[Subagent]) -> String {
    let active = agents
        .iter()
        .filter(|agent| agent.status == SubagentStatus::Working)
        .count();
    let unknown = agents.len() - active;
    if active == 0 {
        "Subagents · status unavailable".into()
    } else {
        let noun = if active == 1 { "subagent" } else { "subagents" };
        format!(
            "{active} {noun}{}",
            if unknown > 0 {
                format!(" · {unknown} status unavailable")
            } else {
                String::new()
            }
        )
    }
}
impl Render for SubagentPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_subscription.is_none() {
            self.focus_subscription =
                Some(
                    cx.on_focus_out(&self.panel_focus, window, |this, _, window, cx| {
                        if this.open && !this.trigger_focus.is_focused(window) {
                            this.dismiss(false, window, cx);
                        }
                    }),
                );
        }
        if self.agents.is_empty() {
            return div().into_any_element();
        }
        let bounds = self.bounds.clone();
        let mut root = div()
            .id("session-subagents")
            .debug_selector(|| "session-subagents".into())
            .flex()
            .min_w_0()
            .child(
                div()
                    .id("subagent-trigger")
                    .debug_selector(|| "subagent-trigger".into())
                    .relative()
                    .min_w_0()
                    .child(
                        gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                            .absolute()
                            .inset_0(),
                    )
                    .child(
                        transcript_action(
                            "subagent-pill",
                            summary(&self.agents),
                            false,
                            window,
                            cx,
                        )
                        .icon(gpui_component::Icon::default().path("icons/grid.svg"))
                        .with_focus_handle(self.trigger_focus.clone())
                        .rounded(rpx(RADIUS_FULL).to_pixels(window.rem_size()))
                        .border_1()
                        .border_color(c::BORDER())
                        .max_w_full()
                        .overflow_hidden()
                        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                && !event.is_held
                            {
                                this.toggle(window, cx);
                                window.prevent_default();
                                cx.stop_propagation();
                            }
                        }))
                        .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx))),
                    ),
            );
        if self.open {
            let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
            let width =
                PANEL_W.min(f32::from(window.viewport_size().width) / scale - SPACE_3XL * 2.);
            let height = PANEL_MAX_H
                .min(f32::from(window.viewport_size().height) / scale - APPBAR_H - SPACE_3XL);
            let panel = div()
                .id("subagent-popover")
                .debug_selector(|| "subagent-popover".into())
                .role(gpui::Role::Group)
                .aria_label("Subagents")
                .track_focus(&self.panel_focus)
                .w(rpx(width))
                .max_h(rpx(height))
                .flex()
                .flex_col()
                .p(rpx(SPACE_MD))
                .rounded(rpx(RADIUS_PANEL))
                .border_1()
                .border_color(c::BORDER())
                .bg(c::SURFACE_RAISED())
                .occlude()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down_out(
                    cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                        if !this.bounds.get().contains(&event.position) {
                            this.dismiss(false, window, cx);
                        }
                    }),
                )
                .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.dismiss(true, window, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }))
                .child(
                    div()
                        .px(rpx(SPACE_LG))
                        .py(rpx(SPACE_MD))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_MUTE())
                        .child("Subagents"),
                )
                .child(
                    div()
                        .id("subagent-list")
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .children(self.agents.iter().map(|agent| {
                            let role = agent
                                .role
                                .as_deref()
                                .filter(|text| !text.trim().is_empty())
                                .unwrap_or("Subagent");
                            let task = agent
                                .task
                                .as_deref()
                                .filter(|text| !text.trim().is_empty())
                                .unwrap_or("Task unavailable");
                            let state = if agent.status == SubagentStatus::Unknown {
                                "Status unavailable"
                            } else {
                                "Working"
                            };
                            let label: SharedString =
                                format!("{role}: {task}. {state}. {}", agent.activity).into();
                            let error = self.errors.get(&agent.id).cloned();
                            let id = agent.id.clone();
                            let stopping = self.stopping.contains(&id);
                            let can_stop = self.stoppable.contains(&id);
                            div()
                                .id(SharedString::from(format!("subagent-{}", agent.id)))
                                .group("subagent-row")
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .px(rpx(SPACE_LG))
                                .py(rpx(SPACE_LG))
                                .rounded(rpx(RADIUS_GROUP))
                                .hover(|style| style.bg(c::BG_HOVER()))
                                .aria_label(label.clone())
                                .tooltip(move |window, cx| {
                                    tooltip(label.clone(), window).build(window, cx)
                                })
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(rpx(SPACE_LG))
                                        .min_w_0()
                                        .child(crate::icons::icon("grid", ICON_MD, c::FG_MUTE()))
                                        .child(
                                            div()
                                                .max_w(rpx(ROLE_MAX_W))
                                                .flex_shrink_0()
                                                .truncate()
                                                .text_size(rpx(TEXT_SMALL))
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(c::FG())
                                                .child(role.to_owned()),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_size(rpx(TEXT_SMALL))
                                                .text_color(c::FG_DIM())
                                                .child(task.to_owned()),
                                        )
                                        .when(can_stop, |row| {
                                            row.child(
                                                transcript_action(
                                                    SharedString::from(format!("stop-child-{id}")),
                                                    if stopping { "Stopping…" } else { "Stop" },
                                                    false,
                                                    window,
                                                    cx,
                                                )
                                                .when(!stopping, |button| {
                                                    button
                                                        .opacity(0.)
                                                        .group_hover("subagent-row", |style| {
                                                            style.opacity(1.)
                                                        })
                                                        .focus_visible(|style| style.opacity(1.))
                                                        .on_key_down(cx.listener({
                                                            let id = id.clone();
                                                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                                                if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                                                                    this.request_stop(&id, cx);
                                                                    window.prevent_default(); cx.stop_propagation();
                                                                }
                                                            }
                                                        }))
                                                        .on_click(cx.listener(move |this, _, _, cx| this.request_stop(&id, cx)))
                                                }),
                                            )
                                        }),
                                )
                                .when(agent.status == SubagentStatus::Unknown, |row| {
                                    row.child(
                                        div()
                                            .text_size(rpx(TEXT_MICRO))
                                            .text_color(c::FG_MUTE())
                                            .child("Status unavailable"),
                                    )
                                })
                                .when_some(error, |row, error| {
                                    row.child(
                                        div()
                                            .text_size(rpx(TEXT_SMALL))
                                            .text_color(c::RED())
                                            .child(error),
                                    )
                                })
                        })),
                );
            let bounds = self.bounds.get();
            root = root.child(gpui::deferred(
                gpui::anchored()
                    .position_mode(gpui::AnchoredPositionMode::Window)
                    .anchor(gpui::Anchor::BottomLeft)
                    .position(gpui::point(
                        bounds.left(),
                        bounds.top() - rpx(SPACE_MD).to_pixels(window.rem_size()),
                    ))
                    .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
                    .child(panel),
            ));
        }
        root.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Entity, Modifiers};
    fn child(id: &str, status: SubagentStatus) -> Subagent {
        Subagent {
            id: id.into(),
            name: "Observed name".into(),
            role: Some("Researcher".into()),
            task: Some(format!("Inspect {id}")),
            activity: "Reading files".into(),
            status,
        }
    }
    #[test]
    fn count_reports_unknown_observations_without_calling_them_working() {
        assert_eq!(
            summary(&[child("a", SubagentStatus::Working)]),
            "1 subagent"
        );
        assert_eq!(
            summary(&[child("a", SubagentStatus::Unknown)]),
            "Subagents · status unavailable"
        );
        assert_eq!(
            summary(&[
                child("a", SubagentStatus::Working),
                child("b", SubagentStatus::Unknown)
            ]),
            "1 subagent · 1 status unavailable"
        );
    }
    struct Host(Entity<SubagentPanel>);
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .justify_end()
                .p(rpx(SPACE_3XL))
                .child(self.0.clone())
        }
    }
    fn fixture(
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<SubagentPanel>, &mut gpui::VisualTestContext) {
        cx.update(gpui_component::init);
        let mut panel = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let entity = cx.new(SubagentPanel::new);
            entity.update(cx, |panel, cx| {
                panel.set_agents(
                    "session-a".into(),
                    vec![child("a", SubagentStatus::Working)],
                    cx,
                )
            });
            panel = Some(entity.clone());
            let host = cx.new(|_| Host(entity));
            gpui_component::Root::new(host, window, cx).bordered(false)
        });
        (panel.unwrap(), cx)
    }
    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    #[gpui::test]
    fn panel_anchors_upward_without_moving_input_and_dismisses(cx: &mut gpui::TestAppContext) {
        let (panel, cx) = fixture(cx);
        draw(cx);
        let trigger = cx.debug_bounds("subagent-trigger").unwrap();
        cx.simulate_click(trigger.center(), Modifiers::default());
        draw(cx);
        assert!(panel.read_with(cx, |panel, _| panel.open));
        assert_eq!(cx.debug_bounds("subagent-trigger").unwrap(), trigger);
        assert!(cx.debug_bounds("subagent-popover").unwrap().bottom() <= trigger.top());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(!panel.read_with(cx, |panel, _| panel.open));
        assert!(cx.update(|window, cx| panel.read(cx).trigger_focus.is_focused(window)));
        cx.simulate_keystrokes("space");
        draw(cx);
        assert!(panel.read_with(cx, |panel, _| panel.open));
        cx.simulate_click(
            gpui::point(gpui::px(2.), gpui::px(2.)),
            Modifiers::default(),
        );
        draw(cx);
        assert!(!panel.read_with(cx, |panel, _| panel.open));
    }
    #[gpui::test]
    fn panel_reconciles_session_completion_capabilities_and_scroll(cx: &mut gpui::TestAppContext) {
        let (panel, cx) = fixture(cx);
        cx.simulate_resize(gpui::size(gpui::px(320.), gpui::px(500.)));
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.set_agents(
                    "session-a".into(),
                    (0..40)
                        .map(|id| child(&id.to_string(), SubagentStatus::Working))
                        .collect(),
                    cx,
                );
                panel.set_stop_targets(HashSet::from(["0".into()]), cx);
                panel.toggle(window, cx);
            })
        });
        draw(cx);
        let popup = cx.debug_bounds("subagent-popover").unwrap();
        assert!(popup.left() >= gpui::px(0.));
        assert!(popup.right() <= gpui::px(320.));
        assert!(popup.size.height <= gpui::px(PANEL_MAX_H));
        assert!(panel.read_with(cx, |panel, _| panel.scroll.max_offset().y > gpui::px(0.)));
        cx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel.request_stop("not-a-child", cx);
                assert!(panel.stopping.is_empty());
                panel.request_stop("0", cx);
                panel.request_stop("0", cx);
                assert_eq!(panel.stopping.len(), 1);
                panel.stop_failed("0".into(), "Provider disconnected".into(), cx);
                assert!(!panel.stopping.contains("0"));
                assert_eq!(panel.errors.get("0").unwrap(), "Provider disconnected");
                panel.set_agents(
                    "session-b".into(),
                    vec![child("0", SubagentStatus::Unknown)],
                    cx,
                );
                assert!(!panel.open);
                assert!(panel.stoppable.is_empty());
                assert!(panel.errors.is_empty());
                panel.open = true;
                panel.set_agents(
                    "session-b".into(),
                    vec![child("0", SubagentStatus::Completed)],
                    cx,
                );
                assert!(!panel.open);
                assert!(panel.agents.is_empty());
            })
        });
    }
}
