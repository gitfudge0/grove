//! Observed subagent activity and the checkout section that displays it.

use super::*;

pub(super) const AGENT_STATUS_W: f32 = ICON_MD;
pub(super) const AGENT_LINE_H: f32 = 18.;

impl Sidebar {
    pub(super) fn render_checkout_agents(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Some(Selection::Session(id)) = self.selection else {
            self.checkout_panel.agent_observer = None;
            return None;
        };
        let Some(session) = self.runtime.read(cx).registry.read(cx).codex(id).cloned() else {
            self.checkout_panel.agent_observer = None;
            return None;
        };
        let saved = &session.read(cx).state.saved;
        let provider = saved.provider.agent().icon_name();
        let periods = saved.work_periods.clone();
        let agents = checkout_agents(&periods);
        if agents.is_empty() {
            return None;
        }
        let running = agents
            .iter()
            .filter(|agent| agent.status == grove_core::codex::SubagentStatus::Working)
            .count();
        let (active, previous) = checkout_agent_groups(&agents, &periods);
        let disclosure = self.checkout_panel.agent_disclosures.entry(id).or_default();
        let show_all = disclosure.show_all;
        let show_previous = disclosure.previous;
        let mut section = div()
            .id("checkout-agents")
            .debug_selector(|| "checkout-agents".into())
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .child(
                div()
                    .h(rpx(AGENT_ROW_H))
                    .px(rpx(SPACE_2XL))
                    .flex()
                    .items_center()
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .child(format!("Agents · {running} running")),
            )
            .children(
                active
                    .iter()
                    .take(if show_all { usize::MAX } else { AGENTS_VISIBLE })
                    .map(|agent| self.agent_peek_row(id, agent, provider, cx)),
            );
        if active.len() > AGENTS_VISIBLE {
            section = section.child(
                checkout_row(
                    "checkout-agents-more",
                    if show_all {
                        "Show fewer".to_owned()
                    } else {
                        format!("Show {} more", active.len() - AGENTS_VISIBLE)
                    },
                    "plus",
                    window,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let disclosure = this.checkout_panel.agent_disclosures.entry(id).or_default();
                    disclosure.show_all = !disclosure.show_all;
                    cx.notify();
                })),
            );
        }
        if !previous.is_empty() {
            section = section.child(
                checkout_action(
                    "checkout-agents-previous",
                    format!(
                        "{} previous agents ({})",
                        if show_previous { "Collapse" } else { "Expand" },
                        previous.len()
                    ),
                    false,
                    window,
                    cx,
                )
                .debug_selector(|| "checkout-agents-previous".into())
                .icon_only()
                .w_full()
                .h(rpx(AGENT_ROW_H))
                .px(rpx(SPACE_2XL))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_SM))
                        .w_full()
                        .min_w_0()
                        .text_size(rpx(TEXT_MICRO))
                        .text_color(c::FG_DIM())
                        .child(icon("chev-down", ICON_SM, c::FG_DIM()).when(
                            show_previous,
                            |glyph| {
                                glyph.with_transformation(gpui::Transformation::rotate(
                                    gpui::percentage(0.5),
                                ))
                            },
                        ))
                        .child(format!("Previous agents ({})", previous.len()))
                        .child(
                            div()
                                .flex_1()
                                .min_w(rpx(SPACE_LG))
                                .border_t_1()
                                .border_color(c::BORDER_SOFT()),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    let disclosure = this.checkout_panel.agent_disclosures.entry(id).or_default();
                    disclosure.previous = !disclosure.previous;
                    cx.notify();
                })),
            );
            if show_previous {
                section = section.children(
                    previous
                        .iter()
                        .map(|agent| self.agent_peek_row(id, agent, provider, cx)),
                );
            }
        }
        let visible: std::collections::HashSet<_> = active
            .iter()
            .take(if show_all { usize::MAX } else { AGENTS_VISIBLE })
            .chain(previous.iter().filter(|_| show_previous))
            .map(|agent| agent.id.as_str())
            .collect();
        if self
            .checkout_panel
            .peek
            .as_ref()
            .is_some_and(|peek| !visible.contains(peek.child.as_str()))
        {
            self.checkout_panel.peek = None;
            self.checkout_panel.peek_task = None;
            self.checkout_panel.peek_hover_target = None;
        }
        Some(section.into_any_element())
    }
}

const AGENTS_VISIBLE: usize = 5;
const AGENT_ROW_H: f32 = 32.;

/// Later work periods replace earlier observations, including terminal states.
pub(super) fn checkout_agents(
    periods: &[grove_core::codex::WorkPeriod],
) -> Vec<grove_core::codex::Subagent> {
    let mut latest: Vec<grove_core::codex::Subagent> = Vec::new();
    for agent in periods.iter().flat_map(|period| &period.subagents) {
        if let Some(previous) = latest.iter_mut().find(|previous| previous.id == agent.id) {
            *previous = agent.clone();
        } else {
            latest.push(agent.clone());
        }
    }
    latest
}

pub(super) fn checkout_agent_groups<'a>(
    agents: &'a [grove_core::codex::Subagent],
    periods: &[grove_core::codex::WorkPeriod],
) -> (
    Vec<&'a grove_core::codex::Subagent>,
    Vec<&'a grove_core::codex::Subagent>,
) {
    let latest_ids: std::collections::HashSet<_> = periods
        .last()
        .into_iter()
        .flat_map(|period| &period.subagents)
        .map(|agent| agent.id.as_str())
        .collect();
    agents.iter().partition(|agent| {
        latest_ids.contains(agent.id.as_str())
            || matches!(
                agent.status,
                grove_core::codex::SubagentStatus::Working
                    | grove_core::codex::SubagentStatus::Unknown
            )
    })
}

/// Presentation uses only observed provider text; terminal activity is not an outcome.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::views::sidebar) struct AgentPresentation {
    pub(in crate::views::sidebar) name: String,
    pub(in crate::views::sidebar) activity: Option<String>,
    pub(in crate::views::sidebar) status: &'static str,
    pub(super) detail: String,
}

fn normalized_agent_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn informative_agent_activity(text: &str) -> Option<String> {
    let text = normalized_agent_text(text);
    (!text.is_empty() && !text.eq_ignore_ascii_case("Activity unavailable")).then_some(text)
}

pub(in crate::views::sidebar) fn checkout_agent_presentation(
    agent: &grove_core::codex::Subagent,
) -> AgentPresentation {
    use grove_core::codex::SubagentStatus;
    let status = match agent.status {
        SubagentStatus::Working => "Working",
        SubagentStatus::Completed => "Done",
        SubagentStatus::Failed => "Failed",
        SubagentStatus::Interrupted => "Interrupted",
        SubagentStatus::Unknown => "Unknown",
    };
    let name = normalized_agent_text(&agent.name);
    let name = if name.is_empty() {
        "Agent".into()
    } else {
        name
    };
    let task = agent
        .task
        .as_deref()
        .map(normalized_agent_text)
        .filter(|s| !s.is_empty());
    let observed = informative_agent_activity(&agent.activity);
    let activity = observed.or_else(|| task.clone());
    let mut detail = format!("{name} · {status}");
    if let Some(task) = task {
        detail.push_str(" · Task: ");
        detail.push_str(&task);
    }
    if let Some(observed) = informative_agent_activity(&agent.activity) {
        detail.push_str(" · Latest observed activity: ");
        detail.push_str(&observed);
    }
    AgentPresentation {
        name,
        activity,
        status,
        detail,
    }
}

pub(in crate::views::sidebar) fn checkout_agent_row(
    agent: &grove_core::codex::Subagent,
    provider: &'static str,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let presentation = checkout_agent_presentation(agent);
    let name_id = agent.id.clone();
    let activity_id = agent.id.clone();
    let status_id = agent.id.clone();
    let provider_label = if provider == Agent::Claude.icon_name() {
        "Claude"
    } else {
        "Codex"
    };
    let detail = format!("{provider_label} · {}", presentation.detail);
    div()
        .id(SharedString::from(format!("checkout-agent-{}", agent.id)))
        .role(gpui::Role::Button)
        .group("neutral-control")
        .tab_index(0)
        .focus_visible(|style| style.bg(c::BG_HOVER()))
        .hover(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
        .aria_label(detail.clone())
        .px(rpx(SPACE_2XL))
        .py(rpx(SPACE_SM))
        .flex()
        .flex_col()
        .min_w_0()
        .text_size(rpx(TEXT_BODY))
        .font_weight(gpui::FontWeight::NORMAL)
        .text_color(c::FG_DIM())
        .line_height(rpx(AGENT_LINE_H))
        .child(
            div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_SM))
                .min_w_0()
                .child(
                    div()
                        .flex_shrink_0()
                        .child(icon(provider, ICON_SM, c::FG())),
                )
                .child(
                    div()
                        .debug_selector(move || format!("checkout-agent-name-{name_id}"))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(presentation.name),
                )
                .child(
                    div()
                        .debug_selector(move || format!("checkout-agent-status-{status_id}"))
                        .w(rpx(AGENT_STATUS_W))
                        .flex_shrink_0()
                        .h(rpx(AGENT_LINE_H))
                        .flex()
                        .items_center()
                        .justify_end()
                        .child(status_icon_sized(
                            format!("checkout-agent-{}", agent.id).into(),
                            presentation.status,
                            ICON_MD,
                            cx,
                        )),
                ),
        )
        // Reserve one compact supporting line so activity updates keep rows stable.
        .child(
            div()
                .debug_selector(move || format!("checkout-agent-activity-{activity_id}"))
                .h(rpx(AGENT_LINE_H))
                .w_full()
                .min_w_0()
                .truncate()
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .group_hover("neutral-control", |text| text.text_color(c::FG()))
                .child(presentation.activity.unwrap_or_default()),
        )
}
