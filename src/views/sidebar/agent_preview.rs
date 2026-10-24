//! Session-owned, read-only child activity preview.
use super::checkout_panel::{checkout_agent_presentation, checkout_agent_row, AgentPeek};
use super::*;
use crate::views::components::transcript_action;
const PEEK_WIDTH: f32 = 360.;
const PEEK_HEIGHT: f32 = 360.;
const HOVER_INTENT: Duration = Duration::from_millis(220);
const HOVER_GRACE: Duration = Duration::from_millis(260);

impl Sidebar {
    pub(super) fn selected_session_has_agents(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(Selection::Session(id)) = self.selection else {
            self.checkout_panel.agent_observer = None;
            return false;
        };
        let session = self.runtime.read(cx).registry.read(cx).codex(id).cloned();
        let Some(session) = session else {
            self.checkout_panel.agent_observer = None;
            return false;
        };
        if self
            .checkout_panel
            .agent_observer
            .as_ref()
            .is_none_or(|(owner, _)| *owner != session.entity_id())
        {
            let observer = cx.observe(&session, |_, _, cx| cx.notify());
            self.checkout_panel.agent_observer = Some((session.entity_id(), observer));
        }
        session
            .read(cx)
            .state
            .saved
            .work_periods
            .iter()
            .any(|period| !period.subagents.is_empty())
    }

    pub(super) fn dismiss_agent_peek(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.checkout_panel.peek_task = None;
        self.checkout_panel.peek_hover_target = None;
        if let Some(peek) = self.checkout_panel.peek.as_mut() {
            if peek.closing {
                return;
            }
            peek.closing = true;
            if peek.focus.contains_focused(window, cx) {
                if let Some(source) = self.checkout_panel.agent_focus.get(&peek.child) {
                    source.focus(window, cx);
                }
            }
            self.checkout_panel.peek_presence.set(false, cx);
            if cx.reduce_motion() {
                self.checkout_panel.peek = None;
            } else {
                let duration = self.checkout_panel.peek_presence.transition_duration();
                self.checkout_panel.peek_close_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(duration).await;
                    let _ = this.update(cx, |this, cx| {
                        this.checkout_panel.peek = None;
                        cx.notify();
                    });
                }));
            }
        }
        cx.notify();
    }

    pub(super) fn open_agent_peek(
        &mut self,
        session: SessionId,
        child: String,
        explicit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.checkout_panel.peek_task = None;
        if self.confirmation_open()
            || self.menu.is_some()
            || self.project_panel.is_some()
            || self.project_setup.is_some()
            || self.pending_new_worktree.is_some()
            || self
                .settings_panel
                .as_ref()
                .is_some_and(|panel| panel.read(cx).is_open())
        {
            return;
        }
        self.checkout_panel.peek_close_task = None;
        if self.checkout_panel.peek.is_none() {
            self.checkout_panel.peek_presence = motion::Presence::fast();
        }
        self.checkout_panel.peek_presence.set(true, cx);
        let focus = cx.focus_handle();
        if explicit {
            focus.focus(window, cx);
        }
        let Some(owner) = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(session)
            .cloned()
        else {
            return;
        };
        let preview_child = child.clone();
        let transcript = cx.new(|cx| {
            super::super::codex_view::CodexView::child_preview(owner, preview_child, window, cx)
        });
        let tick = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if !this
                .update(cx, |this, cx| {
                    cx.notify();
                    this.checkout_panel.peek.is_some()
                })
                .unwrap_or(false)
            {
                break;
            }
        });
        self.checkout_panel.peek = Some(AgentPeek {
            session,
            child,
            explicit,
            closing: false,
            focus,
            transcript,
            _tick: tick,
        });
        cx.notify();
    }

    pub(super) fn agent_peek_hover(
        &mut self,
        session: SessionId,
        child: String,
        hovered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // GPUI can deliver the next row's enter before the old row (or preview)
        // leaves. An old source must not cancel the new source's intent timer.
        if !hovered
            && self
                .checkout_panel
                .peek_hover_target
                .as_ref()
                .is_some_and(|(owner, target)| *owner != session || *target != child)
        {
            return;
        }
        self.checkout_panel.peek_hover_target = hovered.then(|| (session, child.clone()));
        self.checkout_panel.peek_task = None;
        if self
            .checkout_panel
            .peek
            .as_ref()
            .is_some_and(|peek| peek.explicit)
        {
            return;
        }
        self.checkout_panel.peek_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(if hovered { HOVER_INTENT } else { HOVER_GRACE })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.selection != Some(Selection::Session(session)) {
                    return;
                }
                if hovered {
                    this.open_agent_peek(session, child, false, window, cx);
                } else {
                    this.dismiss_agent_peek(window, cx);
                }
            });
        }));
    }

    pub(super) fn agent_peek_row(
        &mut self,
        session: SessionId,
        agent: &grove_core::codex::Subagent,
        provider: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let focus = self
            .checkout_panel
            .agent_focus
            .entry(agent.id.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let bounds = self
            .checkout_panel
            .agent_bounds
            .entry(agent.id.clone())
            .or_default()
            .clone();
        let hover_child = agent.id.clone();
        let click_child = agent.id.clone();
        let key_child = agent.id.clone();
        checkout_agent_row(agent, provider, cx)
            .role(gpui::Role::Button)
            .aria_expanded(self.checkout_panel.peek.as_ref().is_some_and(|peek| {
                !peek.closing && peek.session == session && peek.child == agent.id
            }))
            .debug_selector({
                let id = agent.id.clone();
                move || format!("checkout-agent-{id}")
            })
            .track_focus(&focus)
            .relative()
            .cursor_pointer()
            .child(
                gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                this.agent_peek_hover(session, hover_child.clone(), *hovered, window, cx);
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_agent_peek(session, click_child.clone(), true, window, cx);
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.dismiss_agent_peek(window, cx);
                        cx.stop_propagation();
                    }
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.open_agent_peek(session, key_child.clone(), true, window, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }),
            )
    }

    pub(super) fn render_agent_peek(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let peek = self.checkout_panel.peek.as_ref()?;
        if self.selection != Some(Selection::Session(peek.session))
            || !self.checkout_panel.agents_visible
        {
            self.checkout_panel.peek = None;
            self.checkout_panel.peek_task = None;
            self.checkout_panel.peek_hover_target = None;
            return None;
        }
        if peek.closing && cx.reduce_motion() {
            self.checkout_panel.peek = None;
            return None;
        }
        let closing = peek.closing;
        let child = peek.child.clone();
        let session_id = peek.session;
        let session = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(session_id)
            .cloned();
        let Some(session) = session else {
            self.checkout_panel.peek = None;
            self.checkout_panel.peek_task = None;
            self.checkout_panel.peek_hover_target = None;
            return None;
        };
        let state = &session.read(cx).state;
        let agent = state
            .saved
            .work_periods
            .iter()
            .rev()
            .flat_map(|period| period.subagents.iter())
            .find(|agent| agent.id == child)
            .cloned();
        let Some(agent) = agent else {
            self.checkout_panel.peek = None;
            self.checkout_panel.peek_task = None;
            self.checkout_panel.peek_hover_target = None;
            return None;
        };
        let activity = state.subagent_activity.get(&child).cloned();
        let evicted = state.subagent_activity_evicted.contains(&child);
        let provider = if state.saved.provider == grove_core::codex::Provider::Claude {
            "Claude"
        } else {
            "Codex"
        };
        let presentation = checkout_agent_presentation(&agent);
        let focus = peek.focus.clone();
        let transcript = peek.transcript.clone();
        let source = self.checkout_panel.agent_bounds.get(&child)?.get();
        let margin = rpx(SPACE_LG).to_pixels(window.rem_size());
        // The transcript excludes the dock/composer; use its measured rectangle,
        // never a viewport fraction, to keep previews inside the conversation.
        let pane = self
            .codex_views
            .get(&session_id)?
            .read(cx)
            .transcript_bounds
            .get();
        if pane.size.width <= margin * 2. || pane.size.height <= margin * 2. {
            return None;
        }
        let height = rpx(PEEK_HEIGHT)
            .to_pixels(window.rem_size())
            .min(pane.size.height - margin * 2.);
        let width = rpx(PEEK_WIDTH)
            .to_pixels(window.rem_size())
            .min(pane.size.width - margin * 2.);
        let y = source
            .top()
            .min(pane.bottom() - height - margin)
            .max(pane.top() + margin);
        let right = (source.left() - margin)
            .max(pane.left() + width + margin)
            .min(pane.right() - margin);
        let mut content = div()
            .id("agent-peek")
            .debug_selector(|| "agent-peek".into())
            .role(gpui::Role::Dialog)
            .aria_label(format!("{} activity preview", presentation.name))
            .when(!closing, |panel| panel.track_focus(&focus).tab_index(0))
            .when(!closing, gpui::InteractiveElement::occlude)
            .w(width)
            .h(height)
            .overflow_hidden()
            .p(rpx(SPACE_3XL))
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .rounded(rpx(RADIUS_PANEL))
            .border_1()
            .border_color(c::BORDER_SOFT())
            .bg(c::SURFACE_RAISED())
            .text_size(rpx(TEXT_SMALL))
            .text_color(c::FG())
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                if this
                    .checkout_panel
                    .peek
                    .as_ref()
                    .is_some_and(|peek| peek.closing)
                {
                    return;
                }
                if *hovered {
                    this.checkout_panel.peek_task = None;
                } else if let Some(peek) = &this.checkout_panel.peek {
                    this.agent_peek_hover(peek.session, peek.child.clone(), false, window, cx);
                }
            }))
            .on_mouse_down_out(
                cx.listener(|this, _, window, cx| this.dismiss_agent_peek(window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.dismiss_agent_peek(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex_shrink_0()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_MD))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(presentation.name.clone()),
                    )
                    .child(
                        super::super::status::label(
                            format!("agent-preview-status-{}", agent.id),
                            super::super::status::session_kind(presentation.status),
                            presentation.status,
                            ICON_SM,
                            cx,
                        )
                        .items_center()
                        .flex_shrink_0(),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .min_w_0()
                    .truncate()
                    .text_color(c::FG_DIM())
                    .child(format!(
                        "{provider}{}",
                        agent
                            .role
                            .as_ref()
                            .map_or(String::new(), |role| format!(" · {role}"))
                    )),
            );
        let freshness = activity
            .as_ref()
            .and_then(|activity| {
                activity.observed_at.map(|observed| {
                    format!(
                        "Observed {}s ago · {} messages and tool calls",
                        observed.elapsed().as_secs(),
                        activity.items.len()
                    )
                })
            })
            .unwrap_or_else(|| "Observation time unavailable".into());
        if let Some(activity) = &activity {
            if activity.refresh_error {
                content = content.child(div().text_color(c::FG_DIM()).child(
                    super::super::status::label(
                        "child-refresh-error",
                        super::super::status::StatusKind::Warning,
                        "Child activity could not be refreshed",
                        ICON_SM,
                        cx,
                    ),
                ));
            } else if activity.refreshing {
                content = content.child(div().text_color(c::FG_DIM()).child(
                    super::super::status::label(
                        "child-refreshing",
                        super::super::status::StatusKind::Working,
                        "Refreshing child activity…",
                        ICON_SM,
                        cx,
                    ),
                ));
            }
        }
        if let Some(task) = &agent.task {
            content = content.child(
                div()
                    .flex_shrink_0()
                    .text_color(c::FG_DIM())
                    .child(task.clone()),
            );
        }
        if activity
            .as_ref()
            .is_none_or(|activity| activity.items.is_empty())
        {
            content = content.child(div().text_color(c::FG_DIM()).child(if evicted {
                "Child transcript is no longer retained"
            } else {
                "No child messages received yet"
            }));
        }
        content = content
            .child(
                div()
                    .id("agent-peek-body")
                    .min_h_0()
                    .flex_1()
                    .child(transcript),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .child(freshness),
            )
            .child(
                transcript_action("agent-peek-close", "Close preview", false, window, cx)
                    .debug_selector(|| "agent-peek-close".into())
                    .flex_shrink_0()
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/close.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| this.dismiss_agent_peek(window, cx)),
                    ),
            );
        content = content.relative().when(closing, |panel| {
            panel.child(
                div()
                    .absolute()
                    .inset_0()
                    .occlude()
                    .capture_any_mouse_down(|_, _, cx| cx.stop_propagation())
                    .capture_any_mouse_up(|_, _, cx| cx.stop_propagation()),
            )
        });
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position_mode(gpui::AnchoredPositionMode::Window)
                    .anchor(gpui::Anchor::TopRight)
                    .position(gpui::point(right, y))
                    .snap_to_window_with_margin(margin)
                    .child(self.checkout_panel.peek_presence.render(
                        content,
                        "agent-peek-presence",
                        cx,
                    )),
            )
            .into_any_element(),
        )
    }
}
