//! Provider management, release status and actions.

use super::{
    agent_display_label, c, components, rpx, status, Agent, Disableable, SettingsPanel, StatusKind,
};
use crate::views::tokens::{
    FIELD_LABEL_COL_W, ICON_SM, SPACE_2XL, SPACE_LG, SPACE_SM, SPACE_XS, TEXT_BODY, TEXT_SMALL,
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
};
use gpui::{div, Context, FontWeight, Window};

impl SettingsPanel {
    pub(super) fn toggle_provider_details(&mut self, agent: Agent, cx: &mut Context<Self>) {
        if !self.provider_details.insert(agent.label()) {
            self.provider_details.remove(agent.label());
        }
        cx.notify();
    }

    fn toggle_managed_provider(
        &mut self,
        agent: Agent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .provider_accounts
            .iter()
            .any(|panel| panel.read(cx).busy())
        {
            return;
        }
        // Disclosure is immediate input/focus layout feedback; focus moves with the current layout.
        self.managed_provider = if self.managed_provider == Some(agent) {
            None
        } else {
            Some(agent)
        };
        self.provider_manage_focus[match agent {
            Agent::Codex => 0,
            Agent::Claude => 1,
            _ => 2,
        }]
        .focus(window, cx);
        cx.notify();
    }

    pub(super) fn providers_body(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let mut body = div().w_full().min_w_0().flex().flex_col();
        let mut version_body = self
            .managed_provider
            .map(|agent| self.provider_version_body(agent, window, cx));
        let updates = self.runtime.read(cx).provider_updates.read(cx);
        let busy = self
            .provider_accounts
            .iter()
            .any(|panel| panel.read(cx).busy());
        for (index, agent) in [Agent::Codex, Agent::Claude, Agent::OpenCode]
            .into_iter()
            .enumerate()
        {
            if agent == Agent::OpenCode
                && !updates.rows().iter().any(|row| {
                    row.status.agent == agent
                        && (row.status.executable.is_some()
                            || row.status.update_available()
                            || row.error.is_some())
                })
            {
                continue;
            }
            let account = self.provider_accounts.get(index).cloned();
            let (kind, label) = account
                .as_ref()
                .map_or((StatusKind::Idle, "Version only"), |account| {
                    account.read(cx).summary()
                });
            let can_login = account
                .as_ref()
                .is_some_and(|account| account.read(cx).can_login());
            let expanded = self.managed_provider == Some(agent);
            let id = format!("settings-provider-{}", agent.label());
            let update = updates.rows().iter().find(|row| row.status.agent == agent);
            let mut actions = components::form_actions()
                .w_auto()
                .max_w_full()
                .flex_shrink_1()
                .id(format!("{id}-actions"))
                .debug_selector({
                    let id = id.clone();
                    move || format!("{id}-actions")
                });
            if can_login {
                actions = actions.child(
                    components::form_action(format!("{id}-login"), "Sign in", false, window, cx)
                        .icon(
                            gpui_component::Icon::default()
                                .path("icons/external-link.svg")
                                .size(rpx(ICON_SM)),
                        )
                        .disabled(busy)
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                if !busy
                                    && matches!(event.keystroke.key.as_str(), "enter" | "space")
                                    && !event.is_held
                                    && !event.keystroke.modifiers.control
                                    && !event.keystroke.modifiers.alt
                                    && !event.keystroke.modifiers.platform
                                {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    this.managed_provider = Some(agent);
                                    this.provider_accounts[index]
                                        .update(cx, |panel, cx| panel.login(window, cx));
                                    cx.notify();
                                }
                            },
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.managed_provider = Some(agent);
                            this.provider_accounts[index]
                                .update(cx, |panel, cx| panel.login(window, cx));
                            cx.notify();
                        })),
                );
            }
            if let Some(row) = update
                .filter(|row| row.updating || row.status.update_available() || row.error.is_some())
            {
                let enabled = !busy
                    && !updates.checking()
                    && !updates.rows().iter().any(|row| row.updating)
                    && row.status.can_update();
                actions = actions.child(
                    components::form_action(
                        format!("{id}-update"),
                        if row.updating {
                            "Updating…"
                        } else if row.error.is_some() {
                            "Retry"
                        } else {
                            "Update"
                        },
                        false,
                        window,
                        cx,
                    )
                    .debug_selector({
                        let id = id.clone();
                        move || format!("{id}-update")
                    })
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/refresh.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .disabled(!enabled)
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if enabled
                                && matches!(event.keystroke.key.as_str(), "enter" | "space")
                                && !event.is_held
                                && !event.keystroke.modifiers.control
                                && !event.keystroke.modifiers.alt
                                && !event.keystroke.modifiers.platform
                            {
                                window.prevent_default();
                                cx.stop_propagation();
                                this.runtime
                                    .read(cx)
                                    .provider_updates
                                    .clone()
                                    .update(cx, |updates, cx| updates.start_update(agent, cx));
                            }
                        },
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.runtime
                            .read(cx)
                            .provider_updates
                            .clone()
                            .update(cx, |updates, cx| updates.start_update(agent, cx));
                    })),
                );
            }
            actions = actions.child(
                components::form_action(
                    format!("{id}-manage"),
                    if expanded { "Less" } else { "Manage" },
                    false,
                    window,
                    cx,
                )
                .debug_selector({
                    let id = id.clone();
                    move || format!("{id}-manage")
                })
                .with_focus_handle(self.provider_manage_focus[index].clone())
                .aria_expanded(expanded)
                .icon(
                    gpui_component::Icon::default()
                        .path(if expanded {
                            "icons/chev-down.svg"
                        } else {
                            "icons/chev-right.svg"
                        })
                        .size(rpx(ICON_SM)),
                )
                .disabled(busy)
                .on_key_down(
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if !busy
                            && matches!(event.keystroke.key.as_str(), "enter" | "space")
                            && !event.is_held
                            && !event.keystroke.modifiers.control
                            && !event.keystroke.modifiers.alt
                            && !event.keystroke.modifiers.platform
                        {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.toggle_managed_provider(agent, window, cx);
                        }
                    }),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_managed_provider(agent, window, cx);
                })),
            );
            let mut card = div()
                .id(id.clone())
                .w_full()
                .min_w_0()
                .border_t_1()
                .border_color(c::BORDER_SOFT())
                .py(rpx(SPACE_2XL))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .child(
                            div()
                                .flex_1()
                                .min_w(rpx(FIELD_LABEL_COL_W))
                                .flex()
                                .gap(rpx(SPACE_SM))
                                .child(crate::icons::icon(agent.icon_name(), ICON_SM, c::FG()))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(rpx(SPACE_XS))
                                        .child(
                                            div()
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(agent_display_label(agent)),
                                        )
                                        .child(status::label(
                                            format!("{id}-account-status"),
                                            kind,
                                            label,
                                            ICON_SM,
                                            cx,
                                        )),
                                ),
                        )
                        .child(actions),
                );
            if expanded {
                card = card.child(
                    div()
                        .id(format!("{id}-management"))
                        .debug_selector({
                            let id = id.clone();
                            move || format!("{id}-management")
                        })
                        .w_full()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(rpx(SPACE_2XL))
                        .pt(rpx(SPACE_2XL))
                        .when_some(account, gpui::ParentElement::child)
                        .when_some(version_body.take(), gpui::ParentElement::child),
                );
            } else if let Some(error) = update.and_then(|row| row.error.as_ref()) {
                card = card.child(status::label(
                    format!("{id}-update-error"),
                    StatusKind::Failed,
                    error.clone(),
                    ICON_SM,
                    cx,
                ));
            }
            body = body.child(card);
        }
        body.border_b_1()
            .border_color(c::BORDER_SOFT())
            .text_size(rpx(TEXT_BODY))
    }

    fn provider_version_body(
        &self,
        selected: Agent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let updates = self.runtime.read(cx).provider_updates.clone();
        let state = updates.read(cx);
        let row = state.rows().iter().find(|row| row.status.agent == selected);
        let busy = state.checking()
            || state.rows().iter().any(|row| row.updating)
            || self
                .provider_accounts
                .iter()
                .any(|account| account.read(cx).busy());
        let id = format!("settings-provider-{}", selected.label());
        let mut actions = components::form_actions()
            .w_auto()
            .max_w_full()
            .flex_shrink_1()
            .id(format!("{id}-version-actions"))
            .debug_selector({
                let id = id.clone();
                move || format!("{id}-version-actions")
            })
            .child(
                components::form_action(
                    format!("{id}-check"),
                    if state.checking() {
                        "Checking…"
                    } else {
                        "Check for updates"
                    },
                    false,
                    window,
                    cx,
                )
                .icon(
                    gpui_component::Icon::default()
                        .path("icons/refresh.svg")
                        .size(rpx(ICON_SM)),
                )
                .disabled(busy)
                .on_key_down(
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if !busy
                            && matches!(event.keystroke.key.as_str(), "enter" | "space")
                            && !event.is_held
                            && !event.keystroke.modifiers.control
                            && !event.keystroke.modifiers.alt
                            && !event.keystroke.modifiers.platform
                        {
                            window.prevent_default();
                            cx.stop_propagation();
                            this.runtime.read(cx).provider_updates.clone().update(
                                cx,
                                crate::entities::provider_updates::ProviderUpdates::check,
                            );
                        }
                    }),
                )
                .when(!busy, |el| {
                    el.on_click(cx.listener(|this, _, _, cx| {
                        this.runtime.read(cx).provider_updates.clone().update(
                            cx,
                            crate::entities::provider_updates::ProviderUpdates::check,
                        );
                    }))
                }),
            );
        let versions =
            row.map_or_else(
                || "Version not checked".to_string(),
                |row| {
                    let installed = row.status.installed.as_deref().unwrap_or(
                        if row.status.executable.is_some() {
                            "Version unavailable"
                        } else {
                            "Not installed"
                        },
                    );
                    if row.status.update_available()
                        && !matches!(
                            row.status.installer,
                            grove_core::provider_updates::Installer::ClaudeNative
                        )
                    {
                        format!(
                            "{installed} → {}",
                            row.status.latest.as_deref().unwrap_or_default()
                        )
                    } else {
                        installed.into()
                    }
                },
            );
        if let Some(row) = row {
            if matches!(
                row.status.installer,
                grove_core::provider_updates::Installer::ClaudeNative
            ) && !row.status.update_available()
            {
                let enabled = !busy && row.status.can_update();
                actions = actions.child(
                    components::form_action(
                        format!("{id}-native-update"),
                        "Update",
                        false,
                        window,
                        cx,
                    )
                    .icon(
                        gpui_component::Icon::default()
                            .path("icons/refresh.svg")
                            .size(rpx(ICON_SM)),
                    )
                    .disabled(!enabled)
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if enabled
                                && matches!(event.keystroke.key.as_str(), "enter" | "space")
                                && !event.is_held
                                && !event.keystroke.modifiers.control
                                && !event.keystroke.modifiers.alt
                                && !event.keystroke.modifiers.platform
                            {
                                window.prevent_default();
                                cx.stop_propagation();
                                this.runtime
                                    .read(cx)
                                    .provider_updates
                                    .clone()
                                    .update(cx, |updates, cx| updates.start_update(selected, cx));
                            }
                        },
                    ))
                    .when(enabled, |el| {
                        el.on_click(cx.listener(move |this, _, _, cx| {
                            this.runtime
                                .read(cx)
                                .provider_updates
                                .clone()
                                .update(cx, |updates, cx| updates.start_update(selected, cx));
                        }))
                    }),
                );
            }
        }
        let mut body = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .pt(rpx(SPACE_2XL))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(
                        div()
                            .flex_1()
                            .min_w(rpx(FIELD_LABEL_COL_W))
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_SM))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Version · This machine"),
                            )
                            .child(
                                components::readonly_value(format!("{id}-versions"), versions)
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM()),
                            ),
                    )
                    .child(actions),
            );
        if let Some(row) = row {
            if let Some(error) = &row.error {
                let expanded = self.provider_details.contains(selected.label());
                body = body.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(rpx(SPACE_SM))
                        .child(status::label(
                            format!("{id}-error-summary"),
                            StatusKind::Failed,
                            if row.status.error.is_some() {
                                "Could not check."
                            } else {
                                "Update failed."
                            },
                            ICON_SM,
                            cx,
                        ))
                        .child(
                            Self::disclosure(
                                format!("{id}-details"),
                                format!("Error details for {}", agent_display_label(selected)),
                                expanded,
                            )
                            .debug_selector({
                                let id = id.clone();
                                move || format!("{id}-details")
                            })
                            .child(crate::icons::icon(
                                if expanded { "chev-down" } else { "chev-right" },
                                ICON_SM,
                                c::FG_DIM(),
                            ))
                            .child("Details")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_provider_details(selected, cx);
                            }))
                            .on_key_down(cx.listener(
                                move |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                        window.prevent_default();
                                        cx.stop_propagation();
                                        this.toggle_provider_details(selected, cx);
                                    }
                                },
                            )),
                        ),
                );
                if expanded {
                    body = body.child(
                        components::readonly_value(format!("{id}-error"), error.clone())
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM()),
                    );
                }
            } else if row.updated
                || row.update_verified
                || (!matches!(
                    row.status.installer,
                    grove_core::provider_updates::Installer::ClaudeNative
                ) && !row.checking
                    && row.status.installed.is_some()
                    && row.status.latest.is_some()
                    && !row.status.update_available())
            {
                body = body.child(status::label(
                    format!("{id}-current"),
                    StatusKind::Success,
                    if row.updated { "Updated" } else { "Up to date" },
                    ICON_SM,
                    cx,
                ));
            } else if matches!(
                row.status.installer,
                grove_core::provider_updates::Installer::ClaudeNative
            ) {
                body = body.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("The native installer checks its configured update channel."),
                );
            } else if row.status.update_available() && !row.status.can_update() {
                body = body.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("Update unavailable"),
                );
            }
        }
        body
    }
}
