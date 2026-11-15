//! Account allowance is independent of a conversation's context occupancy.
use super::{components, motion, rpx, tokens::*};
use crate::{entities::codex_session::CodexSession, icons::icon, theme as c};
use gpui::{div, prelude::*, App, Context, Entity, FocusHandle, Focusable, Subscription, Window};
use gpui_component::Disableable;
use grove_core::{
    account_usage::{AccountUsage, UsageStatus},
    connections::{AccountStatus, ConnectionContext, UsageReading},
    provider::Provider,
};

const PANEL_W: f32 = 380.;
const METER_H: f32 = 5.;
const REFRESH_W: f32 = 92.;

pub(super) struct UsageSource {
    pub provider: Provider,
    pub cwd: String,
    pub args: Vec<String>,
    pub session: Option<Entity<CodexSession>>,
}
struct Row {
    source: UsageSource,
    reading: Option<UsageReading>,
    error: Option<String>,
    checking: bool,
    usage: Option<AccountUsage>,
    conversation_reported: bool,
    baseline: Option<AccountUsage>,
    refresh_focus: FocusHandle,
    confirmed: Option<std::time::Instant>,
}
pub(super) struct DismissUsage(pub bool, pub Option<gpui::Point<gpui::Pixels>>);
pub(super) struct UsagePanel {
    rows: Vec<Row>,
    focus: FocusHandle,
    _observers: Vec<Subscription>,
    _age: gpui::Task<()>,
    pub(super) bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
}
impl gpui::EventEmitter<DismissUsage> for UsagePanel {}
impl Focusable for UsagePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn age(at: u64) -> String {
    match now().saturating_sub(at) {
        0..=59 => "just now".into(),
        seconds @ 60..=3599 => format!("{} minutes ago", seconds / 60),
        seconds @ 3600..=86399 => format!("{} hours ago", seconds / 3600),
        seconds => format!("{} days ago", seconds / 86400),
    }
}
fn reset(at: i64) -> String {
    grove_core::account_usage::reset_label(at, now())
}

impl UsagePanel {
    pub(super) fn new(sources: Vec<UsageSource>, cx: &mut Context<Self>) -> Self {
        let mut observers = Vec::new();
        for (index, source) in sources.iter().enumerate() {
            if let Some(session) = &source.session {
                observers.push(cx.observe(session, move |this, session, cx| {
                    let state = &session.read(cx).state;
                    let row = &mut this.rows[index];
                    // Never restore old account measurements into a new account check.
                    // Only events arriving after opening this exact context are eligible.
                    if !row.checking
                        && row.error.is_none()
                        && state.saved.cwd == row.source.cwd
                        && state.saved.config_args == row.source.args
                        && matches!(
                            row.reading.as_ref().map(|r| &r.account),
                            Some(AccountStatus::Connected { .. })
                        )
                        && session.read(cx).has_transport()
                    {
                        if let Some(usage) = state
                            .account_usage
                            .as_ref()
                            .filter(|u| Some(*u) != row.baseline.as_ref())
                        {
                            if row.usage.as_ref().is_none_or(|old| old != usage) {
                                row.usage = Some(usage.clone());
                                row.baseline = Some(usage.clone());
                                row.conversation_reported = true;
                                cx.notify();
                            }
                        }
                    }
                }));
            }
        }
        let rows = sources
            .into_iter()
            .map(|source| Row {
                reading: None,
                error: None,
                checking: false,
                usage: None,
                conversation_reported: false,
                baseline: source
                    .session
                    .as_ref()
                    .and_then(|s| s.read(cx).state.account_usage.clone()),
                refresh_focus: cx.focus_handle().tab_stop(true),
                confirmed: None,
                source,
            })
            .collect();
        let mut this = Self {
            rows,
            focus: cx.focus_handle(),
            _observers: observers,
            bounds: std::rc::Rc::new(std::cell::Cell::new(gpui::Bounds::default())),
            _age: cx.spawn(async move |this, cx| loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_mins(1))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }),
        };
        for i in 0..this.rows.len() {
            this.refresh(i, cx);
        }
        this
    }
    fn refresh(&mut self, index: usize, cx: &mut Context<Self>) {
        let row = &mut self.rows[index];
        if row.checking {
            return;
        }
        row.checking = true;
        row.confirmed = None;
        row.error = None;
        row.baseline = row
            .source
            .session
            .as_ref()
            .and_then(|s| s.read(cx).state.account_usage.clone());
        let provider = row.source.provider;
        let cwd = row.source.cwd.clone();
        let args = row.source.args.clone();
        if cfg!(test) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(
                    async move { ConnectionContext::new(provider, cwd, args)?.usage() },
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                let row = &mut this.rows[index];
                row.checking = false;
                // Events received during this read cannot supersede its newer result.
                // Rebase even on failure so an unrelated redraw cannot resurrect them.
                row.baseline = row.source.session.as_ref().and_then(|s| s.read(cx).state.account_usage.clone());
                match result {
                    Ok(reading) => {
                        // Configuration/account changes cannot inherit another balance.
                        let same = row.reading.as_ref().is_some_and(|old| {
                            old.context_key.is_some()
                                && old.context_key == reading.context_key
                                && matches!((&old.account, &reading.account),
                                    (AccountStatus::Connected { identity: Some(a), .. }, AccountStatus::Connected { identity: Some(b), .. }) if a == b)
                        });
                        if reading.usage.is_some() || !same {
                            row.usage.clone_from(&reading.usage);
                            row.conversation_reported = false;
                        }
                        row.reading = Some(reading);
                        let started = std::time::Instant::now();
                        row.confirmed = Some(started);
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(super::status::CHECK_CONFIRMATION_LIFETIME).await;
                            let _ = this.update(cx, |this, cx| {
                                if this.rows[index].confirmed == Some(started) {
                                    this.rows[index].confirmed = None;
                                    cx.notify();
                                }
                            });
                        }).detach();
                    }
                    Err(error) => {
                        row.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn render_row(
        &self,
        index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let row = &self.rows[index];
        let provider = row.source.provider;
        let mut body = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL))
            .p(rpx(SPACE_3XL))
            .border_b_1()
            .border_color(c::BORDER());
        body = body.child(
            div()
                .flex()
                .items_start()
                .gap(rpx(SPACE_LG))
                .child(icon(provider.agent().label(), ICON_LG, c::FG()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(provider.label()),
                )
                .child(
                    div()
                        .max_w(rpx(PANEL_W / 2.))
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(match row.reading.as_ref().map(|r| &r.account) {
                            Some(AccountStatus::Connected {
                                identity: Some(identity),
                                ..
                            }) => identity.clone(),
                            Some(AccountStatus::Connected { .. }) => "Signed in".into(),
                            Some(AccountStatus::External) => "Managed externally".into(),
                            Some(AccountStatus::SignedOut) => "Signed out".into(),
                            None => "Account unverified".into(),
                        }),
                ),
        );
        if let Some(usage) = &row.usage {
            for (i, quota) in usage.windows.iter().enumerate() {
                let mut line = div().flex().flex_col().gap(rpx(SPACE_SM));
                line = line.child(
                    div()
                        .flex()
                        .justify_between()
                        .gap(rpx(SPACE_LG))
                        .child(div().flex_1().child(quota.name.clone()))
                        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(
                            quota.remaining_percent.map_or_else(
                                || "Percentage unavailable".into(),
                                |value| format!("{value:.0}% left"),
                            ),
                        )),
                );
                if let Some(percent) = quota.remaining_percent {
                    line = line.child(
                        div()
                            .id(format!("usage-meter-{index}-{i}"))
                            .debug_selector(move || format!("usage-meter-{index}-{i}"))
                            .role(gpui::Role::Meter)
                            .aria_label(format!("{}: {percent:.0}% remaining", quota.name))
                            .h(rpx(METER_H))
                            .w_full()
                            .rounded_full()
                            .bg(c::BORDER())
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .w(gpui::relative((percent / 100.).clamp(0., 1.) as f32))
                                    .bg(c::FG_MUTE()),
                            ),
                    );
                }
                if let Some(at) = quota.resets_at {
                    line = line.child(
                        div()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child(reset(at)),
                    );
                }
                body = body.child(line);
            }
            let (label, glyph, color) = match usage.status {
                UsageStatus::Limited => {
                    ("Usage limit reached", "status-limited", c::STATUS_WARNING())
                }
                UsageStatus::Warning => (
                    "Approaching usage limit",
                    "status-warning",
                    c::STATUS_WARNING(),
                ),
                UsageStatus::Allowed
                    if usage.windows.iter().all(|w| w.remaining_percent.is_none()) =>
                {
                    (
                        "Within limits · percentage unavailable",
                        "ring",
                        c::FG_DIM(),
                    )
                }
                _ => ("", "ring", c::FG_DIM()),
            };
            if !label.is_empty() {
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_SM))
                        .child(icon(glyph, ICON_SM, color))
                        .child(div().text_color(color).child(label)),
                );
            }
        } else {
            let message = match row.reading.as_ref().map(|r| &r.account) {
                Some(AccountStatus::External) => "Remaining allowance isn’t reported for this connection.",
                Some(AccountStatus::SignedOut) => "Sign in through Settings to check this provider’s usage.",
                Some(AccountStatus::Connected { .. }) if provider == Provider::Claude => "No usage reported yet. Claude’s usage events appear here as this conversation runs.",
                Some(AccountStatus::Connected { .. }) => "Usage unavailable. The provider did not report an allowance.",
                None if row.checking => "Checking usage…",
                None => "Usage unavailable.",
            };
            body = body.child(
                div()
                    .debug_selector(move || format!("usage-empty-{index}"))
                    .text_color(c::FG_DIM())
                    .child(message),
            );
        }
        if let Some(error) = &row.error {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_SM))
                    .child(div().text_color(c::AMBER()).child(if row.usage.is_some() {
                        "Last known · refresh failed"
                    } else {
                        "Couldn’t check usage"
                    }))
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(error.clone()),
                    ),
            );
        }
        let freshness = row.usage.as_ref().map_or_else(
            || {
                if row.checking {
                    "Checking…".into()
                } else {
                    "No allowance measurement".into()
                }
            },
            |usage| {
                format!(
                    "{} {}",
                    if row.conversation_reported {
                        "Conversation-reported"
                    } else if row.error.is_some() {
                        "Last checked"
                    } else {
                        "Updated"
                    },
                    age(usage.observed_at)
                )
            },
        );
        let can_refresh = provider == Provider::Codex || row.error.is_some();
        body = body.child(
            div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_LG))
                .child(
                    div()
                        .flex_1()
                        .text_size(rpx(TEXT_MICRO))
                        .text_color(c::FG_DIM())
                        .child(freshness),
                )
                .when(can_refresh, |line| {
                    line.child(
                        components::transcript_action(
                            format!("usage-refresh-{index}"),
                            if row.checking {
                                "Checking…"
                            } else if row.confirmed.is_some() {
                                "Updated"
                            } else if row.error.is_some() {
                                "Retry"
                            } else {
                                "Refresh"
                            },
                            false,
                            window,
                            cx,
                        )
                        .icon_only()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .child(if row.checking {
                                    super::status::mark(
                                        format!("usage-refresh-{index}"),
                                        super::status::StatusKind::Working,
                                        ICON_SM,
                                        cx,
                                    )
                                } else if let Some(started) = row.confirmed {
                                    super::status::drawn_check(
                                        format!("usage-refresh-{index}"),
                                        0,
                                        started,
                                        ICON_SM,
                                        cx,
                                    )
                                } else {
                                    icon("restart", ICON_SM, c::FG_DIM()).into_any_element()
                                })
                                .child(if row.checking {
                                    "Checking…"
                                } else if row.confirmed.is_some() {
                                    "Updated"
                                } else if row.error.is_some() {
                                    "Retry"
                                } else {
                                    "Refresh"
                                }),
                        )
                        .w(rpx(REFRESH_W))
                        .with_focus_handle(row.refresh_focus.clone())
                        .disabled(row.checking)
                        .on_click(cx.listener(move |this, _, _, cx| this.refresh(index, cx))),
                    )
                }),
        );
        body.into_any_element()
    }
}
impl Render for UsagePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
        let width = PANEL_W.min(f32::from(window.viewport_size().width) / scale - SPACE_3XL);
        let rows = (0..self.rows.len())
            .map(|i| self.render_row(i, window, cx))
            .collect::<Vec<_>>();
        let bounds = self.bounds.clone();
        let content = div()
            .id("provider-usage-panel")
            .debug_selector(|| "provider-usage-panel".into())
            .role(gpui::Role::Group)
            .aria_label("Provider usage")
            .track_focus(&self.focus)
            .relative()
            .child(
                gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .w(rpx(width))
            .max_h(window.viewport_size().height - rpx(SPACE_3XL).to_pixels(window.rem_size()))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .bg(c::BG_RAIL())
            .text_color(c::FG())
            .text_size(rpx(TEXT_SMALL))
            .border_1()
            .border_color(c::BORDER())
            .rounded(rpx(RADIUS_PANEL))
            .shadow(vec![gpui::BoxShadow {
                color: c::PANEL_SHADOW(),
                offset: gpui::point(
                    gpui::px(0.),
                    rpx(if c::is_dark() {
                        PANEL_SHADOW_Y
                    } else {
                        PANEL_SHADOW_Y_LIGHT
                    })
                    .to_pixels(window.rem_size()),
                ),
                blur_radius: rpx(if c::is_dark() {
                    PANEL_SHADOW_BLUR
                } else {
                    PANEL_SHADOW_BLUR_LIGHT
                })
                .to_pixels(window.rem_size()),
                spread_radius: gpui::px(0.),
                inset: false,
            }])
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|_, event: &gpui::MouseDownEvent, _, cx| {
                cx.emit(DismissUsage(false, Some(event.position)));
            }))
            .on_key_down(cx.listener(|_, e: &gpui::KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" {
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.emit(DismissUsage(true, None));
                }
            }))
            .child(
                div()
                    .p(rpx(SPACE_3XL))
                    .border_b_1()
                    .border_color(c::BORDER())
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_size(rpx(TEXT_TITLE))
                    .child("Provider usage")
                    .child(
                        div()
                            .text_size(rpx(TEXT_MICRO))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_color(c::FG_DIM())
                            .child("Remaining account allowance"),
                    ),
            )
            .children(rows)
            .child(
                div()
                    .p(rpx(SPACE_3XL))
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .child("Shared across conversations using these accounts.")
                    .child(div().child(format!(
                        "Configuration: {}",
                        self.rows.first().map_or("", |r| r.source.cwd.as_str())
                    ))),
            );
        // Popup entrance uses the shared restrained motion; values update immediately.
        motion::fast(content, "provider-usage-entrance", cx)
    }
}

#[cfg(test)]
impl UsagePanel {
    pub(super) fn fixture(&mut self, scenario: &str, cx: &mut Context<Self>) {
        use grove_core::account_usage::UsageWindow;
        for row in &mut self.rows {
            row.checking = false;
            row.reading = Some(UsageReading {
                context_key: Some("fixture".into()),
                account: AccountStatus::Connected {
                    identity: Some("developer-with-a-long-name@example.test".into()),
                    method: "Fixture".into(),
                },
                usage: None,
            });
            let known = row.source.provider == Provider::Codex || scenario == "healthy";
            row.usage = Some(AccountUsage {
                observed_at: now().saturating_sub(if scenario == "stale" { 2280 } else { 0 }),
                status: if known {
                    UsageStatus::Unknown
                } else {
                    UsageStatus::Allowed
                },
                windows: if known {
                    vec![
                        UsageWindow {
                            name: "5 hours".into(),
                            remaining_percent: Some(62.),
                            resets_at: Some(now() as i64 + 8100),
                        },
                        UsageWindow {
                            name: "Weekly".into(),
                            remaining_percent: Some(28.),
                            resets_at: Some(now() as i64 + 200000),
                        },
                    ]
                } else {
                    vec![]
                },
            });
            row.error =
                (scenario == "stale" && known).then(|| "Could not refresh provider usage.".into());
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup(cx: &mut App) {
        gpui_component::init(cx);
        cx.set_reduce_motion(true);
        cx.set_global(crate::settings::SettingsState::new(
            grove_core::storage::Store::default(),
        ));
        cx.set_global(crate::zoom::ZoomState::new(1.));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        c::sync_component_theme(cx);
    }
    #[gpui::test]
    fn usage_unknown_percentage_has_no_fabricated_meter(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        let (_, cx) = cx.add_window_view(|window, cx| {
            let panel = cx.new(|cx| {
                UsagePanel::new(
                    [Provider::Codex, Provider::Claude]
                        .into_iter()
                        .map(|provider| UsageSource {
                            provider,
                            cwd: "/fixture".into(),
                            args: vec![],
                            session: None,
                        })
                        .collect(),
                    cx,
                )
            });
            panel.update(cx, |panel, cx| panel.fixture("partial", cx));
            gpui_component::Root::new(panel, window, cx)
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("usage-meter-0-0").is_some());
        assert!(cx.debug_bounds("usage-meter-0-1").is_some());
        assert!(cx.debug_bounds("usage-meter-1-0").is_none());
    }
    #[gpui::test]
    fn usage_panel_fits_narrow_and_zoomed_windows(cx: &mut gpui::TestAppContext) {
        cx.update(setup);
        for (width, rem) in [(900., 16.), (320., 24.)] {
            let (_, cx) = cx.add_window_view(|window, cx| {
                window.set_rem_size(gpui::px(rem));
                let panel = cx.new(|cx| {
                    UsagePanel::new(
                        [Provider::Codex, Provider::Claude]
                            .into_iter()
                            .map(|provider| UsageSource {
                                provider,
                                cwd: "/a/long/checkout/path".into(),
                                args: vec![],
                                session: None,
                            })
                            .collect(),
                        cx,
                    )
                });
                panel.update(cx, |panel, cx| panel.fixture("healthy", cx));
                gpui_component::Root::new(panel, window, cx)
            });
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.)));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            let panel = cx.debug_bounds("provider-usage-panel").unwrap();
            assert!(f32::from(panel.size.width) <= width);
            assert!(f32::from(panel.size.height) <= 800.);
            for name in ["usage-meter-0-0", "usage-meter-1-0"] {
                let meter = cx.debug_bounds(name).unwrap();
                assert!(meter.left() >= panel.left() && meter.right() <= panel.right());
            }
        }
    }
}
