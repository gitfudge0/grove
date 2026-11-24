//! Transient provider authentication UI. It never writes credentials or interaction output.
use super::{
    components, rpx,
    status::{self, StatusKind},
    tokens::*,
};
use crate::{runtime::Runtime, settings::SettingsState, theme as c};
use gpui::Focusable as _;
use gpui::{div, prelude::*, App, Context, Entity, FontWeight, Task, Window};
use gpui_component::{input::InputState, Disableable, Selectable};
use grove_core::{
    connections::{
        AccountStatus, AuthJob, ConnectionAction, ConnectionContext, JobPhase, JobResult,
        ServerState, ServerStatus,
    },
    provider::{Action, Provider},
};
use std::time::Duration;

// Keep enough room for a detail value before placing actions alongside it.
const PROVIDER_DETAIL_VALUE_W: f32 = 208.0;
const REFRESH_ACCOUNT: &str = "Refresh account";

pub(super) fn provider_detail_fields(
    label: &'static str,
    value: gpui::Div,
    actions: impl IntoElement,
) -> gpui::Div {
    div()
        .flex()
        .flex_wrap()
        .items_start()
        .gap(rpx(SPACE_LG))
        .child(
            div()
                .w(rpx(FIELD_LABEL_COL_W))
                .flex_shrink_0()
                .pt(rpx(SPACE_XS))
                .font_weight(FontWeight::MEDIUM)
                .child(label),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_start()
                .gap(rpx(SPACE_LG))
                .flex_basis(rpx(PROVIDER_DETAIL_VALUE_W))
                .flex_grow_1()
                .min_w_0()
                .child(
                    value
                        .flex_basis(rpx(PROVIDER_DETAIL_VALUE_W))
                        .flex_grow_1()
                        .min_w_0()
                        .pt(rpx(SPACE_XS)),
                )
                .child(actions),
        )
}

pub struct ConnectionChanged(pub Provider);
impl gpui::EventEmitter<ConnectionChanged> for ConnectionsPanel {}

fn requests_account_code(snapshot: &grove_core::connections::JobSnapshot) -> bool {
    if snapshot.phase != JobPhase::Waiting {
        return false;
    }
    let output = grove_core::connections::strip_ansi(&snapshot.output).to_ascii_lowercase();
    // Match an input prompt, not instructions for entering a device code in a browser.
    output.contains("paste code here")
        || output.contains("paste the code here:")
        || output.contains("paste the redirect url here:")
        || output.contains("enter the authorization code here:")
}

pub struct ConnectionsPanel {
    runtime: Entity<Runtime>,
    provider: Provider,
    account_only: bool,
    cwd: String,
    args: Vec<String>,
    context: Result<ConnectionContext, String>,
    account: Option<Result<AccountStatus, String>>,
    servers: Option<Result<Vec<ServerStatus>, String>>,
    checking: bool,
    checking_servers: bool,
    generation: u64,
    job: Option<AuthJob>,
    job_action: Option<ConnectionAction>,
    job_snapshot: Option<grove_core::connections::JobSnapshot>,
    auth_input: Option<Entity<InputState>>,
    auth_input_error: Option<String>,
    confirmation: Option<ConnectionAction>,
    message: Option<(StatusKind, String)>,
    contexts_open: bool,
    account_focus: gpui::FocusHandle,
    sign_out_focus: gpui::FocusHandle,
    confirm_cancel_focus: gpui::FocusHandle,
    auth_focus: [gpui::FocusHandle; 4],
    retired_auth_focus: Option<Vec<gpui::FocusHandle>>,
    auth_source_focus: Option<gpui::FocusHandle>,
    _poll: Task<()>,
}
impl ConnectionsPanel {
    pub fn new(runtime: Entity<Runtime>, cx: &mut Context<Self>) -> Self {
        let cwd = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .to_string_lossy()
            .into_owned();
        let context = Err("Refresh to check the provider configuration.".into());
        Self {
            runtime,
            provider: Provider::Codex,
            account_only: false,
            cwd,
            args: Vec::new(),
            context,
            account: None,
            servers: None,
            checking: false,
            checking_servers: false,
            generation: 0,
            job: None,
            job_action: None,
            job_snapshot: None,
            auth_input: None,
            auth_input_error: None,
            confirmation: None,
            message: None,
            contexts_open: false,
            account_focus: cx.focus_handle(),
            sign_out_focus: cx.focus_handle(),
            confirm_cancel_focus: cx.focus_handle(),
            auth_focus: [
                cx.focus_handle(),
                cx.focus_handle(),
                cx.focus_handle(),
                cx.focus_handle(),
            ],
            retired_auth_focus: None,
            auth_source_focus: None,
            _poll: cx.spawn(async move |this, cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                if this.update(cx, ConnectionsPanel::poll).is_err() {
                    return;
                }
            }),
        }
    }
    pub(super) fn account_panel(
        runtime: Entity<Runtime>,
        provider: Provider,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self::new(runtime, cx);
        panel.provider = provider;
        panel.account_only = true;
        panel
    }
    pub(super) fn busy(&self) -> bool {
        self.account_in_progress() || self.checking || self.checking_servers
    }
    pub(super) fn account_in_progress(&self) -> bool {
        self.job.is_some() || self.job_snapshot.is_some()
    }
    pub(super) fn summary(&self) -> (StatusKind, &'static str) {
        if self.account_in_progress() {
            let phase = self.job_snapshot.as_ref().map(|snapshot| snapshot.phase);
            let label = match (&self.job_action, phase) {
                (_, Some(JobPhase::Verifying)) => "Verifying account…",
                (Some(ConnectionAction::Logout), _) => "Signing out…",
                _ => "Signing in…",
            };
            return (StatusKind::Working, label);
        }
        if self.checking {
            return (StatusKind::Working, "Checking sign-in");
        }
        match &self.account {
            Some(Ok(AccountStatus::Connected { .. })) => (StatusKind::Success, "Signed in"),
            Some(Ok(AccountStatus::SignedOut)) => (StatusKind::NeedsInput, "Signed out"),
            Some(Ok(AccountStatus::External)) => (StatusKind::Idle, "Managed externally"),
            Some(Err(_)) => (StatusKind::Unknown, "Could not check sign-in"),
            None => (StatusKind::Unknown, "Sign-in not checked"),
        }
    }
    pub(super) fn can_login(&self) -> bool {
        !self.account_in_progress() && matches!(&self.account, Some(Ok(AccountStatus::SignedOut)))
    }
    pub(super) fn login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start(ConnectionAction::Login, window, cx);
    }
    pub fn select_context(
        &mut self,
        provider: Provider,
        cwd: String,
        args: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if self.job.is_some() {
            self.message = Some((
                StatusKind::Warning,
                "Finish or cancel the current sign-in before changing connection context.".into(),
            ));
            cx.notify();
            return;
        }
        self.provider = provider;
        self.cwd = cwd;
        self.args = args;
        self.context = Err("Checking provider configuration…".into());
        self.account = None;
        self.servers = None;
        self.message = None;
        self.confirmation = None;
        self.contexts_open = false;
        self.generation = self.generation.wrapping_add(1);
        self.checking = false;
        self.checking_servers = false;
        self.refresh(false, cx);
    }
    pub fn refresh(&mut self, servers: bool, cx: &mut Context<Self>) {
        // Native UI fixtures supply backend snapshots without touching developer accounts.
        if cfg!(test) || self.job.is_some() || self.checking || self.checking_servers {
            return;
        }
        let provider = self.provider;
        let cwd = self.cwd.clone();
        let args = self.args.clone();
        let generation = self.generation;
        if servers {
            self.checking_servers = true;
        } else {
            self.checking = true;
        }
        cx.spawn(async move |this, cx| {
            if servers {
                let (context, result) = cx
                    .background_spawn(async move {
                        match ConnectionContext::new(provider, cwd, args) {
                            Ok(context) => {
                                let result = context.check_servers();
                                (Ok(context), result)
                            }
                            Err(error) => (Err(error.clone()), Err(error)),
                        }
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.generation == generation {
                        this.context = context;
                        this.servers = Some(result);
                        this.checking_servers = false;
                        cx.notify();
                    }
                });
            } else {
                let (context, result) = cx
                    .background_spawn(async move {
                        match ConnectionContext::new(provider, cwd, args) {
                            Ok(context) => {
                                let result = context.account();
                                (Ok(context), result)
                            }
                            Err(error) => (Err(error.clone()), Err(error)),
                        }
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.generation == generation {
                        this.context = context;
                        this.account = Some(result);
                        this.checking = false;
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        cx.notify();
    }
    fn active_sessions(&self, cx: &App) -> bool {
        let runtime = self.runtime.read(cx);
        let registry = runtime.registry.read(cx);
        if runtime.restoring_codex || registry.has_stopping_provider_sessions() {
            return true;
        }
        registry.all().iter().any(|meta| {
            if meta.agent != self.provider.agent() {
                return false;
            }
            if let Some(session) = registry.codex(meta.id) {
                session.read(cx).state.provider_work_active()
            } else {
                registry
                    .session(meta.id)
                    .is_some_and(|terminal| !terminal.read(cx).has_exited())
            }
        })
    }
    fn start(&mut self, action: ConnectionAction, window: &mut Window, cx: &mut Context<Self>) {
        if self.job.is_some() || self.checking || self.checking_servers {
            return;
        }
        if self.active_sessions(cx) {
            self.message=Some((StatusKind::Warning,"Wait for this provider’s active turns or terminal sessions to finish before changing shared credentials.".into()));
            self.confirmation = None;
            if self.account_only && matches!(action, ConnectionAction::Logout) {
                self.account_focus.focus(window, cx);
            }
            cx.notify();
            return;
        }
        let job_action = action.clone();
        let result = self
            .context
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|context| context.start(action));
        match result {
            Ok(job) => {
                self.auth_source_focus = window.focused(cx);
                self.job = Some(job);
                self.job_action = Some(job_action);
                self.job_snapshot = None;
                self.auth_input_error = None;
                self.message = None;
                self.confirmation = None;
                self.auth_input = Some(cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("Provider code or redirect URL, only when requested")
                }));
                if self.account_only && matches!(self.job_action, Some(ConnectionAction::Login)) {
                    self.auth_focus[0].focus(window, cx);
                }
            }
            Err(e) => self.message = Some((StatusKind::Failed, e)),
        }
        cx.notify();
    }
    fn retire_auth_interaction(&mut self, cx: &App) {
        let mut handles = self.auth_focus.to_vec();
        if let Some(input) = &self.auth_input {
            handles.push(input.read(cx).focus_handle(cx));
        }
        if let Some(source) = self.auth_source_focus.take() {
            handles.push(source);
        }
        self.retired_auth_focus = Some(handles);
        self.auth_input = None;
        self.auth_input_error = None;
        self.job_snapshot = None;
    }
    #[cfg(test)]
    pub(super) fn fixture_finish(&mut self, cx: &mut Context<Self>) {
        self.retire_auth_interaction(cx);
        self.message = Some((StatusKind::Success, "Connection verified".into()));
        cx.notify();
    }
    #[cfg(test)]
    pub(super) fn fixture_auth_focus(&self, input: bool, cx: &App) -> gpui::FocusHandle {
        if input {
            self.auth_input.as_ref().unwrap().read(cx).focus_handle(cx)
        } else {
            self.auth_focus[2].clone()
        }
    }
    #[cfg(test)]
    pub(super) fn fixture_confirming(&self) -> bool {
        self.confirmation.is_some()
    }
    #[cfg(test)]
    pub(super) fn fixture_copy_focus(&self) -> gpui::FocusHandle {
        self.auth_focus[3].clone()
    }
    #[cfg(test)]
    pub(super) fn fixture_return_focus(&self) -> gpui::FocusHandle {
        self.account_focus.clone()
    }
    fn poll(&mut self, cx: &mut Context<Self>) {
        let Some(job) = &self.job else {
            return;
        };
        let Some(snapshot) = job.snapshot() else {
            return;
        };
        let finished = matches!(snapshot.phase, JobPhase::Finished | JobPhase::Canceled);
        if finished {
            self.retire_auth_interaction(cx);
            self.auth_input = None;
            self.job = None;
            self.job_snapshot = None;
            let affected = self.job_action.take();
            let server_action = matches!(
                &affected,
                Some(ConnectionAction::ServerLogin(_) | ConnectionAction::ServerLogout(_))
            );
            if !matches!(&snapshot.result, Some(Ok(_))) {
                match affected {
                    Some(ConnectionAction::Login|ConnectionAction::Logout)=>self.account=Some(Err("The latest authentication result is unknown. Refresh to check current account status.".into())),
                    Some(ConnectionAction::ServerLogin(name)|ConnectionAction::ServerLogout(name))=>{if let Some(Ok(servers))=&mut self.servers {if let Some(server)=servers.iter_mut().find(|server|server.name==name){server.state=ServerState::Unknown;server.authorized=false;}}},
                    None=>{},
                }
            }
            match snapshot.result {
                Some(Ok(JobResult::Account(account))) => {
                    let label = if self.account_only {
                        if account == AccountStatus::SignedOut {
                            "Shared CLI login cleared"
                        } else {
                            "Account verified"
                        }
                    } else if account == AccountStatus::SignedOut {
                        "Signed out"
                    } else {
                        "Signed in"
                    };
                    self.account = Some(Ok(account));
                    self.message = Some((StatusKind::Success, label.into()));
                    self.invalidate_catalogs(cx);
                    cx.emit(ConnectionChanged(self.provider));
                }
                Some(Ok(JobResult::Server(server))) => {
                    let ready = server.state == ServerState::Connected;
                    self.message = Some((
                        if ready {
                            StatusKind::Success
                        } else {
                            StatusKind::Warning
                        },
                        if ready {
                            format!(
                                "{} connected. Return to your conversation when ready.",
                                server.name
                            )
                        } else if server.authorized {
                            format!("{} authorized. Reconnect the conversation to use the saved grant; server availability has not been confirmed.",server.name)
                        } else {
                            format!("{} sign-in cleared", server.name)
                        },
                    ));
                    if let Some(Ok(servers)) = &mut self.servers {
                        if let Some(old) = servers.iter_mut().find(|s| s.name == server.name) {
                            *old = server;
                        }
                    }
                }
                Some(Err(error)) => {
                    self.message = Some((
                        if snapshot.phase == JobPhase::Canceled {
                            StatusKind::Canceled
                        } else {
                            StatusKind::Warning
                        },
                        error,
                    ));
                }
                None => {
                    self.message = Some((
                        StatusKind::Unknown,
                        "The provider did not confirm the result. Refresh before trying again."
                            .into(),
                    ));
                }
            }
            self.refresh(server_action, cx);
        } else {
            self.job_snapshot = Some(snapshot);
        }
        cx.notify();
    }
    fn invalidate_catalogs(&self, cx: &mut Context<Self>) {
        let runtime = self.runtime.read(cx);
        let registry = runtime.registry.read(cx);
        let saved: Vec<_> = registry
            .all()
            .iter()
            .filter_map(|meta| registry.codex(meta.id))
            .map(|session| session.read(cx).state.saved.clone())
            .filter(|saved| saved.provider == self.provider)
            .collect();
        cx.background_spawn(async move {
            for saved in saved {
                grove_core::composer_catalog::invalidate(&saved, &saved.config_args);
            }
        })
        .detach();
    }
    fn reconnect(&mut self, cx: &mut Context<Self>) {
        if self.job.is_some() || self.active_sessions(cx) {
            self.message = Some((
                StatusKind::Warning,
                "Reconnect is available after this provider’s active turns finish.".into(),
            ));
            cx.notify();
            return;
        }
        let registry = self.runtime.read(cx).registry.clone();
        let sessions: Vec<_> = registry
            .read(cx)
            .all()
            .iter()
            .filter_map(|meta| registry.read(cx).codex(meta.id).cloned())
            .filter(|session| {
                let saved = &session.read(cx).state.saved;
                saved.provider == self.provider
                    && saved.cwd == self.cwd
                    && saved.config_args == self.args
            })
            .collect();
        if sessions.is_empty() {
            self.message=Some((StatusKind::Info,"New sessions will use the saved credentials. No matching native conversation is open here.".into()));
        } else {
            let failed = false;
            for session in sessions {
                session.update(cx, |s, cx| s.send(Action::Reconnect, cx));
            }
            self.message=Some((if failed{StatusKind::Warning}else{StatusKind::Info},if failed {"A conversation could not start reconnecting. Its history and draft are preserved."}else{"Reconnect requested for matching conversations. Check their connection status when you return; no messages or tool calls are resent."}.into()));
        }
        self.confirmation = None;
        cx.notify();
    }
    fn confirm_account_sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirmation = Some(ConnectionAction::Logout);
        self.message = None;
        self.confirm_cancel_focus.focus(window, cx);
        cx.notify();
    }
    fn cancel_account_sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirmation = None;
        self.sign_out_focus.focus(window, cx);
        cx.notify();
    }
    fn submit_account_code(&mut self, cx: &mut Context<Self>) {
        let (Some(job), Some(input)) = (&self.job, &self.auth_input) else {
            return;
        };
        if !self
            .job_snapshot
            .as_ref()
            .is_some_and(requests_account_code)
        {
            return;
        }
        let value = input.read(cx).value();
        self.auth_input_error = if value.trim().is_empty() {
            Some("Enter the code or redirect URL requested by the provider.".into())
        } else {
            job.respond(value.to_string()).err()
        };
        cx.notify();
    }
    fn render_provider_account(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let body = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .py(rpx(SPACE_2XL))
            .gap(rpx(SPACE_SM));
        if self.account_in_progress() {
            return body.child(self.render_provider_auth(window, cx));
        }
        let busy = self.busy();
        let (value, note, can_logout) = match self.account.as_ref() {
            _ if self.checking => ("Checking account…".into(), None, false),
            Some(Ok(AccountStatus::Connected { identity, method })) => (
                identity.as_deref().unwrap_or(method).to_owned(),
                Some(if self.provider == Provider::Claude {
                    "Shared by Grove and the Claude Code CLI.".into()
                } else if identity.is_some() {
                    format!("{method} · Shared CLI login")
                } else {
                    "Shared CLI login".into()
                }),
                !self
                    .context
                    .as_ref()
                    .is_ok_and(ConnectionContext::externally_managed),
            ),
            Some(Ok(AccountStatus::SignedOut)) => ("Uses the shared CLI login".into(), None, false),
            Some(Ok(AccountStatus::External)) => {
                ("Managed in the CLI environment".into(), None, false)
            }
            Some(Err(error)) => (
                "Account status unavailable".into(),
                Some(error.clone()),
                false,
            ),
            None => ("Sign-in not checked".into(), None, false),
        };
        let mut values = div().child(value).when_some(note, |cell, note| {
            cell.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(note),
            )
        });
        let mut actions = components::form_actions()
            .w_auto()
            .max_w_full()
            .id("provider-account-actions")
            .debug_selector(|| "provider-account-actions".into());
        if self.confirmation.is_some() {
            values = div().child(
                div()
                    .id("connections-confirmation")
                    .debug_selector(|| "connections-confirmation".into())
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("Sign out of {}?", self.provider.label())),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(
                                "Clears the shared CLI login for Grove and other clients using it.",
                            ),
                    ),
            );
            actions = actions
                .child(
                    Self::button(
                        "connections-confirm-cancel",
                        "Cancel",
                        "close",
                        !busy,
                        false,
                        window,
                        cx,
                    )
                    .with_focus_handle(self.confirm_cancel_focus.clone())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.cancel_account_sign_out(window, cx)),
                    ),
                )
                .child(
                    Self::button(
                        "connections-confirm",
                        "Confirm sign-out",
                        "check",
                        !busy,
                        false,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.start(ConnectionAction::Logout, window, cx);
                    })),
                );
        } else {
            actions = actions.child(
                Self::button(
                    "connections-refresh",
                    REFRESH_ACCOUNT,
                    "refresh",
                    !busy,
                    false,
                    window,
                    cx,
                )
                .with_focus_handle(self.account_focus.clone())
                .on_click(cx.listener(|this, _, _, cx| this.refresh(false, cx))),
            );
            if can_logout {
                actions = actions.child(
                    Self::button(
                        "connections-logout",
                        "Sign out",
                        "arrow-right",
                        !busy,
                        false,
                        window,
                        cx,
                    )
                    .with_focus_handle(self.sign_out_focus.clone())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.confirm_account_sign_out(window, cx);
                    })),
                );
            }
        }
        body.child(provider_detail_fields("CLI account", values, actions))
            .when_some(self.message.as_ref(), |body, (kind, message)| {
                body.child(status::label(
                    "connections-result",
                    *kind,
                    message.clone(),
                    ICON_SM,
                    cx,
                ))
            })
    }
    fn render_provider_auth(&self, window: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let snapshot = self.job_snapshot.as_ref();
        let code_requested = snapshot.is_some_and(requests_account_code);
        let title = match (self.job_action.as_ref(), snapshot.map(|state| state.phase)) {
            (_, Some(JobPhase::Verifying)) => "Verifying account",
            (Some(ConnectionAction::Logout), _) => "Clearing the shared CLI login",
            _ if code_requested => "Enter the provider code",
            (_, Some(JobPhase::Waiting)) => "Continue in your browser",
            _ => "Starting sign-in",
        };
        let mut body = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .child(div().font_weight(FontWeight::MEDIUM).child(title));
        if let Some(input) = self.auth_input.as_ref().filter(|_| code_requested) {
            body = body
                .child(components::form_field(
                    "connections-auth-input",
                    "Provider code or redirect URL",
                    input,
                    self.auth_input_error.as_deref(),
                    window,
                    cx,
                ))
                .child(
                    components::form_actions().child(
                        Self::button(
                            "connections-auth-respond",
                            "Send to provider",
                            "check",
                            true,
                            true,
                            window,
                            cx,
                        )
                        .with_focus_handle(self.auth_focus[2].clone())
                        .on_click(cx.listener(|this, _, _, cx| this.submit_account_code(cx))),
                    ),
                );
        }
        let mut actions = components::form_actions();
        if let Some(url) = snapshot.and_then(|snapshot| snapshot.authorization_url.clone()) {
            let copy_url = url.clone();
            actions = actions
                .child(
                    Self::button(
                        "connections-open-browser",
                        "Open browser",
                        "external-link",
                        true,
                        !code_requested,
                        window,
                        cx,
                    )
                    .with_focus_handle(self.auth_focus[1].clone())
                    .on_click(move |_, _, cx| cx.open_url(&url)),
                )
                .child(
                    Self::button(
                        "connections-copy-link",
                        "Copy sign-in link",
                        "copy",
                        true,
                        false,
                        window,
                        cx,
                    )
                    .with_focus_handle(self.auth_focus[3].clone())
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_url.clone()));
                    }),
                );
        }
        if !matches!(self.job_action, Some(ConnectionAction::Logout)) {
            actions = actions.child(
                Self::button(
                    "connections-cancel",
                    "Cancel",
                    "close",
                    true,
                    false,
                    window,
                    cx,
                )
                .with_focus_handle(self.auth_focus[0].clone())
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(job) = &this.job {
                        job.cancel();
                    }
                    cx.notify();
                })),
            );
        }
        body.child(actions).child(
            div()
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .child("This operation continues outside Settings."),
        )
    }
    fn button(
        id: impl Into<gpui::ElementId>,
        label: &'static str,
        glyph: &'static str,
        enabled: bool,
        primary: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui_component::button::Button {
        let button = if label == "Confirm sign-out" {
            components::dense_action_with_colors(
                id,
                label,
                c::FIELD_FILL(),
                c::STATUS_ERROR(),
                c::BG_HOVER(),
                window,
                cx,
            )
        } else {
            components::form_action(id, label, primary, window, cx)
        };
        button
            .icon(
                gpui_component::Icon::default()
                    .path(format!("icons/{glyph}.svg"))
                    .size(rpx(ICON_SM)),
            )
            .disabled(!enabled)
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if this.account_only
                        && this.confirmation.is_some()
                        && matches!(label, "Cancel" | "Confirm sign-out")
                        && event.keystroke.key == "escape"
                        && !event.is_held
                    {
                        window.prevent_default();
                        cx.stop_propagation();
                        this.cancel_account_sign_out(window, cx);
                        return;
                    }
                    if !this.account_only
                        || !enabled
                        || event.is_held
                        || !matches!(event.keystroke.key.as_str(), "enter" | "space")
                        || event.keystroke.modifiers.control
                        || event.keystroke.modifiers.alt
                        || event.keystroke.modifiers.platform
                    {
                        return;
                    }
                    if ![
                        REFRESH_ACCOUNT,
                        "Sign in",
                        "Sign out",
                        "Cancel",
                        "Confirm sign-out",
                        "Copy sign-in link",
                        "Open browser",
                        "Send to provider",
                    ]
                    .contains(&label)
                    {
                        return;
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                    match label {
                        REFRESH_ACCOUNT => this.refresh(false, cx),
                        "Sign in" => this.start(ConnectionAction::Login, window, cx),
                        "Sign out" => this.confirm_account_sign_out(window, cx),
                        "Cancel" => {
                            if let Some(job) = &this.job {
                                job.cancel();
                                cx.notify();
                            } else {
                                this.cancel_account_sign_out(window, cx);
                            }
                        }
                        "Confirm sign-out" => {
                            if let Some(action) = this.confirmation.clone() {
                                this.start(action, window, cx);
                            }
                        }
                        "Copy sign-in link" => {
                            if let Some(url) = this
                                .job_snapshot
                                .as_ref()
                                .and_then(|snapshot| snapshot.authorization_url.as_ref())
                            {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(url.clone()));
                            }
                        }
                        "Open browser" => {
                            if let Some(url) = this
                                .job_snapshot
                                .as_ref()
                                .and_then(|snapshot| snapshot.authorization_url.as_ref())
                            {
                                cx.open_url(url);
                            }
                        }
                        "Send to provider" => this.submit_account_code(cx),
                        _ => {}
                    }
                }),
            )
    }
    fn contexts(&self, cx: &App) -> Vec<(String, Vec<String>)> {
        let mut contexts = vec![(
            dirs::home_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            Vec::new(),
        )];
        for project in &cx.global::<SettingsState>().store.projects {
            if !project.archived {
                contexts.push((project.path.clone(), Vec::new()));
            }
        }
        let registry = self.runtime.read(cx).registry.read(cx);
        for meta in registry.all() {
            if meta.agent == self.provider.agent() {
                let args = registry
                    .codex(meta.id)
                    .map(|s| s.read(cx).state.saved.config_args.clone())
                    .unwrap_or_default();
                contexts.push((meta.wt_path.clone(), args));
            }
        }
        contexts.sort();
        contexts.dedup();
        contexts
    }
    #[cfg(test)]
    pub(super) fn fixture(&mut self, scenario: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.message = None;
        self.confirmation = None;
        self.job_snapshot = None;
        self.auth_input = None;
        self.auth_input_error = None;
        self.checking = false;
        self.account = Some(Ok(AccountStatus::Connected {
            identity: Some("alex@example.test".into()),
            method: "Browser sign-in".into(),
        }));
        self.contexts_open = false;
        self.servers = Some(Ok(vec![
            ServerStatus {
                name: "Jira".into(),
                state: ServerState::NeedsAuth,
                oauth: true,
                authorized: false,
                source: "Project configuration".into(),
            },
            ServerStatus {
                name: "Filesystem".into(),
                state: ServerState::Connected,
                oauth: false,
                authorized: false,
                source: "Local stdio".into(),
            },
        ]));
        if scenario == "signed-out" {
            self.account = Some(Ok(AccountStatus::SignedOut));
        }
        if scenario == "external" {
            self.account = Some(Ok(AccountStatus::External));
        }
        if scenario == "failed" {
            self.message=Some((StatusKind::Warning,"Authorization succeeded. Jira’s server is still unavailable. Retry its connection without signing in again.".into()));
        }
        if scenario == "confirm" {
            self.confirmation = Some(ConnectionAction::Logout);
        }
        if matches!(scenario, "waiting" | "browser") {
            self.job_snapshot = Some(grove_core::connections::JobSnapshot {
                phase: JobPhase::Waiting,
                output: if scenario == "waiting" {
                    "Finish sign-in in your browser.\nPaste the redirect URL here:".into()
                } else {
                    "Finish sign-in in your browser.".into()
                },
                authorization_url: Some("https://identity.example.test/authorize".into()),
                result: None,
            });
            self.auth_input = Some(cx.new(|cx| {
                InputState::new(window, cx).placeholder("Provider code or redirect URL")
            }));
        }
        if scenario == "contexts" {
            self.contexts_open = true;
        }
        cx.notify();
    }
}
impl Render for ConnectionsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(handles) = self.retired_auth_focus.take() {
            if handles.iter().any(|handle| handle.is_focused(window)) {
                self.account_focus.focus(window, cx);
            }
        }
        if self.account_only {
            return self.render_provider_account(window, cx).into_any_element();
        }
        let busy = self.job.is_some() || self.checking || self.checking_servers;
        let mut body = div()
            .id("provider-connections")
            .debug_selector(|| "provider-connections".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_3XL))
            .text_size(rpx(TEXT_BODY));
        let mut providers = components::form_actions();
        for (provider, name, icon) in [
            (Provider::Codex, "Codex", "codex"),
            (Provider::Claude, "Claude Code", "claude"),
        ] {
            let selected = self.provider == provider;
            let cwd = self.cwd.clone();
            let args = if selected {
                self.args.clone()
            } else {
                Vec::new()
            };
            providers = providers.child(
                components::form_action(
                    format!("connections-provider-{icon}"),
                    name,
                    false,
                    window,
                    cx,
                )
                .icon(
                    gpui_component::Icon::default()
                        .path(format!("icons/{icon}.svg"))
                        .size(rpx(ICON_SM))
                        .text_color(if provider == Provider::Claude {
                            c::CLAUDE_BRAND()
                        } else {
                            c::OPENAI_BRAND()
                        }),
                )
                .disabled(busy)
                .selected(selected)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_context(provider, cwd.clone(), args.clone(), cx);
                })),
            );
        }
        if !self.account_only {
            body = body.child(providers).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_SM))
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child("Connection context"),
                    )
                    .child(div().min_w_0().child(self.cwd.clone()))
                    .child(
                        components::form_actions().child(
                            Self::button(
                                "connections-context",
                                "Change checkout",
                                "git-branch",
                                !busy,
                                false,
                                window,
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.contexts_open = !this.contexts_open;
                                cx.notify();
                            })),
                        ),
                    ),
            );
            if self.contexts_open {
                let mut rows = div().flex().flex_col().gap(rpx(SPACE_SM));
                for (index, (cwd, args)) in self.contexts(cx).into_iter().enumerate() {
                    let label = if args.is_empty() {
                        format!("{cwd} · Provider defaults")
                    } else {
                        format!("{cwd} · Saved session settings {}", index + 1)
                    };
                    rows = rows.child(
                        components::form_action(
                            ("connections-context", index),
                            label,
                            false,
                            window,
                            cx,
                        )
                        .icon(gpui_component::Icon::default().path("icons/folder.svg"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_context(this.provider, cwd.clone(), args.clone(), cx);
                        })),
                    );
                }
                body = body.child(rows);
            }
        }
        let mut account = div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .border_t_1()
            .border_color(c::BORDER_SOFT())
            .pt(rpx(SPACE_3XL))
            .child(div().font_weight(FontWeight::MEDIUM).child("Account"));
        let (kind, label, detail, can_login, can_logout) = if self.checking {
            (
                StatusKind::Working,
                "Checking sign-in".to_owned(),
                None,
                false,
                false,
            )
        } else {
            match &self.account {
            Some(Ok(AccountStatus::Connected{identity,method}))=>(StatusKind::Success,"Signed in".into(),Some(identity.as_ref().map_or_else(||method.clone(),|identity|format!("{identity} · {method}"))),false,!self.context.as_ref().is_ok_and(ConnectionContext::externally_managed)),
            Some(Ok(AccountStatus::SignedOut))=>(StatusKind::NeedsInput,"Signed out".into(),Some("Complete sign-in in your browser. Grove uses the provider’s credential storage.".into()),true,false),
            Some(Ok(AccountStatus::External))=>(StatusKind::Idle,"Managed externally".into(),Some("This provider uses credentials supplied by its environment. Grove will not replace or clear them.".into()),false,false),
            Some(Err(error))=>(StatusKind::Unknown,"Could not check sign-in".into(),Some(error.clone()),false,false),
            None=>(StatusKind::Unknown,"Sign-in not checked".into(),self.context.as_ref().err().cloned(),false,false),
        }
        };
        account = account
            .child(status::label(
                "connections-account-status",
                kind,
                label,
                ICON_SM,
                cx,
            ))
            .when_some(detail, |el, detail| {
                el.child(div().text_color(c::FG_DIM()).child(detail))
            });
        let mut actions = components::form_actions().child(
            Self::button(
                "connections-refresh",
                "Refresh",
                "refresh",
                !busy,
                false,
                window,
                cx,
            )
            .with_focus_handle(self.account_focus.clone())
            .on_click(cx.listener(|this, _, _, cx| {
                this.refresh(false, cx);
            })),
        );
        if can_login {
            actions = actions.child(
                Self::button(
                    "connections-login",
                    "Sign in",
                    "external-link",
                    !busy,
                    false,
                    window,
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.start(ConnectionAction::Login, window, cx);
                })),
            );
        }
        if can_logout {
            actions = actions.child(
                Self::button(
                    "connections-logout",
                    "Sign out",
                    "arrow-right",
                    !busy,
                    false,
                    window,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.confirmation = Some(ConnectionAction::Logout);
                    cx.notify();
                })),
            );
        }
        body = body.child(account.child(actions));
        if let Some(snapshot) = &self.job_snapshot {
            let title = match snapshot.phase {
                JobPhase::Starting => "Starting provider sign-in",
                JobPhase::Verifying => "Verifying the connection",
                _ => "Complete the provider sign-in",
            };
            let mut flow=div().flex().flex_col().gap(rpx(SPACE_LG)).border_1().border_color(c::BORDER_SOFT()).rounded(rpx(RADIUS_PANEL)).p(rpx(SPACE_2XL)).child(status::label("connections-auth-status",StatusKind::Working,title,ICON_SM,cx)).child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child("This interaction is temporary. Codes, links and provider output are not saved to your conversation."));
            let output = grove_core::connections::strip_ansi(&snapshot.output);
            if !output.trim().is_empty() {
                flow = flow.child(div().text_size(rpx(TEXT_SMALL)).child(output));
            }
            let mut controls = components::form_actions().child(
                Self::button(
                    "connections-cancel",
                    "Cancel",
                    "close",
                    true,
                    false,
                    window,
                    cx,
                )
                .with_focus_handle(self.auth_focus[0].clone())
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(job) = &this.job {
                        job.cancel();
                    }
                    cx.notify();
                })),
            );
            if let Some(url) = snapshot.authorization_url.clone() {
                let copy_url = url.clone();
                controls = controls.child(
                    Self::button(
                        "connections-copy-link",
                        "Copy sign-in link",
                        "copy",
                        true,
                        false,
                        window,
                        cx,
                    )
                    .with_focus_handle(self.auth_focus[3].clone())
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_url.clone()));
                    }),
                );
                controls = controls.child(
                    Self::button(
                        "connections-open-browser",
                        "Open browser",
                        "external-link",
                        true,
                        false,
                        window,
                        cx,
                    )
                    .with_focus_handle(self.auth_focus[1].clone())
                    .on_click(move |_, _, cx| cx.open_url(&url)),
                );
            }
            flow = flow.child(controls);
            if let Some(input) = &self.auth_input {
                flow = flow
                    .child(components::form_field(
                        "connections-auth-input",
                        "Provider code or redirect URL",
                        input,
                        None,
                        window,
                        cx,
                    ))
                    .child(
                        components::form_actions().child(
                            Self::button(
                                "connections-auth-respond",
                                "Send to provider",
                                "check",
                                snapshot.phase == JobPhase::Waiting,
                                false,
                                window,
                                cx,
                            )
                            .with_focus_handle(self.auth_focus[2].clone())
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    if let (Some(job), Some(input)) = (&this.job, &this.auth_input)
                                    {
                                        let value = input.read(cx).value().to_string();
                                        if let Err(error) = job.respond(value) {
                                            this.message = Some((StatusKind::Warning, error));
                                        }
                                        input.update(cx, |input, cx| {
                                            input.set_value("", window, cx);
                                        });
                                    }
                                },
                            )),
                        ),
                    );
            }
            body = body.child(flow);
        }
        if let Some((kind, message)) = &self.message {
            body = body.child(status::label(
                "connections-result",
                *kind,
                message.clone(),
                ICON_SM,
                cx,
            ));
        }
        if let Some(action) = &self.confirmation {
            let server = match action {
                ConnectionAction::ServerLogout(name) => Some(name.clone()),
                _ => None,
            };
            let message=server.as_ref().map_or_else(||format!("Sign out of {}? This clears the saved provider login used by Grove and any other clients sharing this configuration.",self.provider.label()),|name|format!("Clear {name} sign-in? This removes its saved credentials for this provider configuration and may affect other clients. Local credential clearing does not necessarily revoke remote access."));
            let action = action.clone();
            body = body.child(
                div()
                    .id("connections-confirmation")
                    .debug_selector(|| "connections-confirmation".into())
                    .border_1()
                    .border_color(c::BORDER_SOFT())
                    .rounded(rpx(RADIUS_PANEL))
                    .p(rpx(SPACE_2XL))
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_LG))
                    .child(message)
                    .child(
                        components::form_actions()
                            .child(
                                Self::button(
                                    "connections-confirm-cancel",
                                    "Cancel",
                                    "close",
                                    !busy,
                                    false,
                                    window,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.confirmation = None;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                Self::button(
                                    "connections-confirm",
                                    "Confirm sign-out",
                                    "check",
                                    !busy,
                                    false,
                                    window,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.start(action.clone(), window, cx);
                                    },
                                )),
                            ),
                    ),
            );
        }
        let mut servers=div().flex().flex_col().gap(rpx(SPACE_LG)).border_t_1().border_color(c::BORDER_SOFT()).pt(rpx(SPACE_3XL)).child(div().font_weight(FontWeight::MEDIUM).child("Configured MCP servers")).child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child("Check the selected provider’s existing servers. This may start approved server processes; it does not approve project servers or edit configuration.")).child(components::form_actions().child(Self::button("connections-servers-refresh",if self.checking_servers{"Checking servers…"}else{"Check servers"},if self.checking_servers{"status-working"}else{"refresh"},!busy,false,window,cx).on_click(cx.listener(|this,_,_,cx|this.refresh(true,cx)))));
        match &self.servers {
            None => {}
            Some(Err(error)) => {
                servers = servers.child(status::label(
                    "connections-servers-error",
                    StatusKind::Unknown,
                    error.clone(),
                    ICON_SM,
                    cx,
                ));
            }
            Some(Ok(rows)) => {
                if rows.is_empty() {
                    servers=servers.child(div().text_color(c::FG_DIM()).child("No configured MCP servers found in this context. Configure servers using the provider’s tooling."));
                }
                for (index, server) in rows.iter().enumerate() {
                    let (kind, label) = match server.state {
                        ServerState::Connected => (StatusKind::Success, "Connected"),
                        ServerState::NeedsAuth => (StatusKind::NeedsInput, "Needs sign-in"),
                        ServerState::Disabled => {
                            (StatusKind::Idle, "Disabled by provider configuration")
                        }
                        ServerState::Approval => (StatusKind::Warning, "Project approval required"),
                        ServerState::Unavailable => (StatusKind::Warning, "Server unavailable"),
                        ServerState::Unknown => (
                            StatusKind::Unknown,
                            if server.authorized {
                                "Authorized · Availability not checked"
                            } else {
                                "Availability not checked"
                            },
                        ),
                    };
                    let mut row = div()
                        .id(("connections-server", index))
                        .debug_selector(move || format!("connections-server-{index}"))
                        .flex()
                        .flex_col()
                        .gap(rpx(SPACE_LG))
                        .border_t_1()
                        .border_color(c::BORDER_SOFT())
                        .py(rpx(SPACE_2XL))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child(server.name.clone()),
                        )
                        .child(
                            div()
                                .text_size(rpx(TEXT_SMALL))
                                .text_color(c::FG_DIM())
                                .child(server.source.clone()),
                        )
                        .child(status::label(
                            format!("connections-server-status-{index}"),
                            kind,
                            label,
                            ICON_SM,
                            cx,
                        ));
                    let name = server.name.clone();
                    let logout_name = name.clone();
                    if server.oauth {
                        row = row.child(
                            components::form_actions()
                                .child(
                                    Self::button(
                                        ("connections-server-login", index),
                                        "Sign in",
                                        "external-link",
                                        !busy,
                                        false,
                                        window,
                                        cx,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.start(
                                                ConnectionAction::ServerLogin(name.clone()),
                                                window,
                                                cx,
                                            );
                                        },
                                    )),
                                )
                                .when(server.authorized, |actions| {
                                    actions.child(
                                        Self::button(
                                            ("connections-server-logout", index),
                                            "Clear sign-in",
                                            "close",
                                            !busy,
                                            false,
                                            window,
                                            cx,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.confirmation =
                                                    Some(ConnectionAction::ServerLogout(
                                                        logout_name.clone(),
                                                    ));
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                }),
                        );
                    }
                    servers = servers.child(row);
                }
            }
        }
        servers=servers.child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child("After authorization, reconnect matching idle conversations explicitly. Their thread identity, draft and queued inputs stay intact; failed messages and tools are not resent.")).child(components::form_actions().child(Self::button("connections-reconnect","Reconnect conversations","refresh",!busy,false,window,cx).on_click(cx.listener(|this,_,_,cx|this.reconnect(cx)))));
        body.child(servers).into_any_element()
    }
}
