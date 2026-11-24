//! Provider account summaries, installer maintenance and row-attached details.

use super::{
    agent_display_label, c, components, rpx, status, Agent, Disableable, SettingsPanel, StatusKind,
};
use crate::{
    entities::provider_updates::ProviderRow,
    views::tokens::{
        FIELD_LABEL_COL_W, ICON_SM, RADIUS_PANEL, SPACE_2XL, SPACE_LG, SPACE_SM, TEXT_BODY,
        TEXT_SMALL,
    },
};
use gpui::prelude::{
    FluentBuilder, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
};
use gpui::{div, Context, FontWeight, KeyDownEvent, SharedString, Window};
use grove_core::provider_updates::Installer;

const PROVIDER_ACTION_COLUMN_W: f32 = 148.0;
const PROVIDER_IDENTITY_FRACTION: f32 = 0.23;

const PROVIDERS: [Agent; 3] = [Agent::Codex, Agent::Claude, Agent::OpenCode];

#[derive(Clone, Copy)]
enum ProviderAction {
    Check(Option<Agent>),
    Update(Agent),
    SignIn(Agent, usize),
    Resume(Agent),
    ToggleDetails(Agent, usize),
}

fn activation_key(event: &KeyDownEvent) -> bool {
    matches!(event.keystroke.key.as_str(), "enter" | "space")
        && !event.is_held
        && !event.keystroke.modifiers.control
        && !event.keystroke.modifiers.alt
        && !event.keystroke.modifiers.platform
}

fn version_summary(row: &ProviderRow) -> (StatusKind, SharedString) {
    let status = &row.status;
    let installed = status
        .installed
        .as_deref()
        .unwrap_or(if status.executable.is_some() {
            "Version unavailable"
        } else {
            "Not installed"
        });
    if row.updating {
        (StatusKind::Working, "Updating…".into())
    } else if row.checking {
        (StatusKind::Working, "Checking version…".into())
    } else if row.check_error().is_some() {
        (StatusKind::Failed, "Check failed".into())
    } else if row.update_error().is_some() {
        (StatusKind::Failed, "Update failed".into())
    } else if row.updated {
        (
            StatusKind::Success,
            format!("Updated to {installed}").into(),
        )
    } else if row.update_verified {
        (
            StatusKind::Success,
            format!("{installed} · Up to date").into(),
        )
    } else if matches!(status.installer, Installer::ClaudeNative) {
        (StatusKind::Idle, installed.to_owned().into())
    } else if let Some(latest) = status.latest.as_ref().filter(|_| status.update_available()) {
        if matches!(status.installer, Installer::External { .. }) {
            (
                StatusKind::Warning,
                format!("{installed} · Update externally").into(),
            )
        } else {
            (StatusKind::Idle, format!("{installed} → {latest}").into())
        }
    } else {
        (StatusKind::Idle, installed.to_owned().into())
    }
}

impl SettingsPanel {
    pub(super) fn toggle_provider_details(&mut self, agent: Agent, cx: &mut Context<Self>) {
        if !self.provider_details.insert(agent.label()) {
            self.provider_details.remove(agent.label());
        }
        cx.notify();
    }

    pub(crate) fn active_provider_account(&self, cx: &gpui::App) -> Option<Agent> {
        self.provider_accounts
            .iter()
            .zip(PROVIDERS)
            .find_map(|(panel, agent)| panel.read(cx).account_in_progress().then_some(agent))
    }

    pub(crate) fn resume_provider_account(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.active_provider_account(cx) else {
            return;
        };
        self.open_provider_updates(window, cx);
        if self.page == super::Page::Providers {
            self.reveal_provider_account(agent, window, cx);
        }
    }

    fn reveal_provider_account(
        &mut self,
        agent: Agent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.managed_provider = Some(agent);
        if let Some(index) = PROVIDERS.iter().position(|provider| *provider == agent) {
            self.provider_manage_focus[index].focus(window, cx);
        }
        cx.notify();
    }

    fn run_provider_action(
        &mut self,
        action: ProviderAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            ProviderAction::Check(agent) => {
                self.runtime.read(cx).provider_updates.clone().update(
                    cx,
                    |updates, cx| match agent {
                        Some(agent) => updates.check_provider(agent, cx),
                        None => updates.check(cx),
                    },
                );
            }
            ProviderAction::Update(agent) => self
                .runtime
                .read(cx)
                .provider_updates
                .clone()
                .update(cx, |updates, cx| updates.start_update(agent, cx)),
            ProviderAction::SignIn(agent, index) => {
                self.managed_provider = Some(agent);
                self.provider_accounts[index].update(cx, |panel, cx| panel.login(window, cx));
                cx.notify();
            }
            ProviderAction::Resume(agent) => self.reveal_provider_account(agent, window, cx),
            ProviderAction::ToggleDetails(agent, index) => {
                self.managed_provider = (self.managed_provider != Some(agent)).then_some(agent);
                self.provider_manage_focus[index].focus(window, cx);
                cx.notify();
            }
        }
    }

    fn provider_action(
        id: String,
        label: &'static str,
        glyph: &'static str,
        action: ProviderAction,
        enabled: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui_component::button::Button {
        components::form_action(id.clone(), label, false, window, cx)
            .debug_selector(move || id)
            .icon(
                gpui_component::Icon::default()
                    .path(glyph)
                    .size(rpx(ICON_SM)),
            )
            .disabled(!enabled)
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if enabled && activation_key(event) {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.run_provider_action(action, window, cx);
                }
            }))
            .when(enabled, |button| {
                button.on_click(cx.listener(move |this, _, window, cx| {
                    this.run_provider_action(action, window, cx);
                }))
            })
    }

    pub(super) fn check_all_providers_action(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui_component::button::Button {
        let updates = self.runtime.read(cx).provider_updates.read(cx);
        let checking = updates.checking();
        Self::provider_action(
            "settings-provider-check-all".into(),
            if checking {
                "Checking providers…"
            } else {
                "Check all providers"
            },
            "icons/refresh.svg",
            ProviderAction::Check(None),
            !checking && !updates.rows().iter().any(|row| row.updating),
            window,
            cx,
        )
    }

    pub(super) fn providers_body(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let updates = self.runtime.read(cx).provider_updates.read(cx);
        let checking = updates.checking();
        let updating = updates.rows().iter().any(|row| row.updating);
        let active_account = self.active_provider_account(cx);
        let compact = self.compact_layout(window);
        let mut body = div().w_full().min_w_0().flex().flex_col().pt(rpx(SPACE_LG));
        if !compact {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .pb(rpx(SPACE_SM))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(
                        div()
                            .w(gpui::relative(PROVIDER_IDENTITY_FRACTION))
                            .min_w(rpx(FIELD_LABEL_COL_W))
                            .child("Provider"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .gap(rpx(SPACE_LG))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(rpx(FIELD_LABEL_COL_W))
                                    .child("CLI account"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(rpx(FIELD_LABEL_COL_W))
                                    .child("Version"),
                            ),
                    )
                    .child(div().w(rpx(PROVIDER_ACTION_COLUMN_W))),
            );
        }

        for (index, agent) in PROVIDERS.into_iter().enumerate() {
            let row = updates.rows().iter().find(|row| row.status.agent == agent);
            if agent == Agent::OpenCode
                && !row.is_some_and(|row| {
                    row.status.executable.is_some()
                        || row.status.update_available()
                        || row.update_error().is_some()
                        || row.check_error().is_some()
                })
            {
                continue;
            }
            let id = format!("settings-provider-{}", agent.label());
            let account = self.provider_accounts.get(index);
            let (kind, label) = account.map_or((StatusKind::Idle, "Version only"), |account| {
                account.read(cx).summary()
            });
            let can_login = account.is_some_and(|account| account.read(cx).can_login());
            let account_busy = account.is_some_and(|account| account.read(cx).busy());
            let expanded = self.managed_provider == Some(agent);

            let enabled = !checking && !updating && active_account != Some(agent);
            let primary = if active_account == Some(agent) {
                (!expanded).then_some((
                    "resume",
                    "Resume account operation",
                    "icons/chev-right.svg",
                    ProviderAction::Resume(agent),
                    true,
                ))
            } else if row.is_some_and(|row| row.updating) {
                Some((
                    "update",
                    "Updating…",
                    "icons/refresh.svg",
                    ProviderAction::Update(agent),
                    false,
                ))
            } else if can_login {
                Some((
                    "login",
                    "Sign in",
                    "icons/external-link.svg",
                    ProviderAction::SignIn(agent, index),
                    !account_busy && active_account.is_none() && !updates.is_updating(agent),
                ))
            } else if row.is_some_and(|row| row.check_error().is_some()) {
                Some((
                    "retry-check",
                    "Retry check",
                    "icons/refresh.svg",
                    ProviderAction::Check(Some(agent)),
                    !checking && !updating,
                ))
            } else if row.is_some_and(|row| row.update_error().is_some() && row.can_update()) {
                Some((
                    "update",
                    "Retry update",
                    "icons/refresh.svg",
                    ProviderAction::Update(agent),
                    enabled,
                ))
            } else if row.is_some_and(|row| row.status.update_available() && row.can_update()) {
                Some((
                    "update",
                    "Update",
                    "icons/refresh.svg",
                    ProviderAction::Update(agent),
                    enabled,
                ))
            } else {
                None
            };
            let maintenance_in_row =
                primary.is_some_and(|(suffix, ..)| matches!(suffix, "update" | "retry-check"));
            let toggle = ProviderAction::ToggleDetails(agent, index);
            let details_label = format!(
                "{} {} details",
                if expanded { "Hide" } else { "Show" },
                agent_display_label(agent)
            );
            let toggle_id = format!("{id}-manage");
            let mut actions = div()
                .id(format!("{id}-actions"))
                .debug_selector({
                    let id = id.clone();
                    move || format!("{id}-actions")
                })
                .flex()
                .items_center()
                .justify_end()
                .gap(rpx(SPACE_SM))
                .min_w_0()
                .flex_wrap();
            if let Some((suffix, label, glyph, action, enabled)) = primary {
                actions = actions.child(Self::provider_action(
                    format!("{id}-{suffix}"),
                    label,
                    glyph,
                    action,
                    enabled,
                    window,
                    cx,
                ));
            }
            actions = actions.child(
                components::header_control_frame(
                    toggle_id.clone(),
                    details_label.clone(),
                    c::BG_HOVER(),
                )
                .debug_selector(move || toggle_id)
                .track_focus(&self.provider_manage_focus[index])
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .aria_expanded(expanded)
                .tooltip(move |window, cx| {
                    components::tooltip(details_label.clone(), window).build(window, cx)
                })
                .child(crate::icons::icon(
                    if expanded { "chev-down" } else { "chev-right" },
                    ICON_SM,
                    c::FG_DIM(),
                ))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    if activation_key(event) {
                        window.prevent_default();
                        cx.stop_propagation();
                        this.run_provider_action(toggle, window, cx);
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.run_provider_action(toggle, window, cx);
                })),
            );
            let (version_kind, version) = row.map_or(
                (StatusKind::Unknown, "Version not checked".into()),
                version_summary,
            );
            let identity = div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_SM))
                .min_w(rpx(FIELD_LABEL_COL_W))
                .child(crate::icons::icon(agent.icon_name(), ICON_SM, c::FG()))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(agent_display_label(agent)),
                );
            let metadata = div()
                .flex()
                .gap(rpx(SPACE_LG))
                .flex_wrap()
                .min_w_0()
                .child(
                    div()
                        .flex_1()
                        .min_w(rpx(FIELD_LABEL_COL_W))
                        .child(status::label(
                            format!("{id}-account-status"),
                            kind,
                            label,
                            ICON_SM,
                            cx,
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(rpx(FIELD_LABEL_COL_W))
                        .child(status::label(
                            format!("{id}-version-status"),
                            version_kind,
                            version,
                            ICON_SM,
                            cx,
                        )),
                );
            let row_header = if compact {
                div()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .flex_wrap()
                            .gap(rpx(SPACE_SM))
                            .child(identity)
                            .child(actions),
                    )
                    .child(metadata)
            } else {
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .flex_wrap()
                    .gap(rpx(SPACE_LG))
                    .child(identity.w(gpui::relative(PROVIDER_IDENTITY_FRACTION)))
                    .child(metadata.flex_1())
                    .child(actions.w(rpx(PROVIDER_ACTION_COLUMN_W)))
            };
            let mut card = div()
                .id(id.clone())
                .w_full()
                .min_w_0()
                .border_t_1()
                .border_color(c::BORDER_SOFT())
                .py(rpx(SPACE_2XL))
                .child(row_header);
            if expanded {
                let version =
                    self.provider_version_body(agent, row, maintenance_in_row, enabled, window, cx);
                card = card.child(
                    div()
                        .id(format!("{id}-management"))
                        .debug_selector(move || format!("{id}-management"))
                        .w_full()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .mt(rpx(SPACE_LG))
                        .px(rpx(SPACE_2XL))
                        .rounded(rpx(RADIUS_PANEL))
                        .bg(c::BG_RAIL())
                        .border_1()
                        .border_color(c::BORDER_SOFT())
                        .when_some(account.cloned(), gpui::ParentElement::child)
                        .when(active_account != Some(agent), |panel| panel.child(version)),
                );
            }
            body = body.child(card);
        }
        body.text_size(rpx(TEXT_BODY))
    }

    fn provider_version_body(
        &self,
        agent: Agent,
        row: Option<&ProviderRow>,
        maintenance_in_row: bool,
        enabled: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = format!("settings-provider-{}", agent.label());
        let mut value = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(rpx(FIELD_LABEL_COL_W))
            .gap(rpx(SPACE_SM))
            .text_size(rpx(TEXT_SMALL))
            .text_color(c::FG_DIM());
        let mut actions = components::form_actions().w_auto().max_w_full();
        if let Some(row) = row {
            let guidance = match &row.status.installer {
                Installer::Npm { .. } => "npm",
                Installer::ClaudeNative => "Native updater · configured channel",
                Installer::External { guidance } if row.status.executable.is_some() => guidance,
                Installer::External { .. } => "Install the provider CLI, then check again.",
            };
            value = value.child(div().child(guidance.to_owned()));
            if let Some(latest) = row
                .status
                .latest
                .as_ref()
                .filter(|_| row.latest_is_cached())
            {
                value = value.child(div().child(format!("Last known release {latest}")));
            }
            if let Some(error) = row.check_error().or_else(|| row.update_error()) {
                let expanded = self.provider_details.contains(agent.label());
                value = value
                    .child(div().child(if row.check_error().is_some() {
                        "Couldn’t verify the installed version or latest release."
                    } else {
                        "Couldn’t verify the update."
                    }))
                    .child(
                        Self::disclosure(
                            format!("{id}-details"),
                            format!("Error details for {}", agent_display_label(agent)),
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
                        .child("Error details")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_provider_details(agent, cx);
                        }))
                        .on_key_down(cx.listener(
                            move |this, event: &KeyDownEvent, window, cx| {
                                if activation_key(event) {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    this.toggle_provider_details(agent, cx);
                                }
                            },
                        )),
                    )
                    .when(expanded, |body| {
                        body.child(components::readonly_value(
                            format!("{id}-error"),
                            error.to_owned(),
                        ))
                    });
            }
            if !maintenance_in_row && row.check_error().is_some() {
                actions = actions.child(Self::provider_action(
                    format!("{id}-version-retry-check"),
                    "Retry check",
                    "icons/refresh.svg",
                    ProviderAction::Check(Some(agent)),
                    enabled,
                    window,
                    cx,
                ));
            } else if !maintenance_in_row && row.can_update() {
                actions = actions.child(Self::provider_action(
                    format!("{id}-version-update"),
                    if row.update_error().is_some() {
                        "Retry update"
                    } else if matches!(row.status.installer, Installer::ClaudeNative) {
                        "Run Claude updater"
                    } else {
                        "Update"
                    },
                    "icons/refresh.svg",
                    ProviderAction::Update(agent),
                    enabled,
                    window,
                    cx,
                ));
            }
        } else {
            value = value.child(div().child("Version not checked"));
        }
        div()
            .id(format!("{id}-version-actions"))
            .debug_selector(move || format!("{id}-version-actions"))
            .flex()
            .items_start()
            .flex_wrap()
            .gap(rpx(SPACE_LG))
            .py(rpx(SPACE_2XL))
            .when(agent != Agent::OpenCode, |el| {
                el.border_t_1().border_color(c::BORDER_SOFT())
            })
            .child(
                div()
                    .w(rpx(FIELD_LABEL_COL_W))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Installation"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(rpx(FIELD_LABEL_COL_W))
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .gap(rpx(SPACE_LG))
                    .child(value)
                    .child(actions),
            )
    }
}
