//! Home-only account contribution summary below Checkout.
use super::*;
use crate::views::components::{form_action, project_field_well, tooltip};
use gpui_component::Disableable;
use grove_core::contributions::{self, Activity};

const REFRESH_INTERVAL: Duration = Duration::from_secs(300);
const CELL_SIZE: f32 = 9.;
const LEGEND_SIZE: f32 = 8.;
const REFRESH_ACTION_W: f32 = 108.;
const FEEDBACK_OWNER: &str = "github-account";
const FEEDBACK_ACTION: &str = "contributions";

#[derive(Default)]
pub(super) struct Contributions {
    result: Option<Result<Activity, String>>,
    loading: bool,
    checked_at: Option<Instant>,
    task: Option<gpui::Task<()>>,
}

impl Sidebar {
    fn load_contributions(&mut self, cx: &mut Context<Self>) {
        if self.contributions.loading {
            return;
        }
        self.contributions.loading = true;
        let token = self.begin_panel_feedback(FEEDBACK_OWNER, FEEDBACK_ACTION, "Refreshing…", cx);
        self.contributions.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { contributions::load() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.finish_panel_feedback(
                    FEEDBACK_OWNER,
                    FEEDBACK_ACTION,
                    token,
                    result.as_ref().ok().map(|_| "Refreshed"),
                    cx,
                );
                this.contributions.result = Some(result);
                this.contributions.loading = false;
                this.contributions.checked_at = Some(Instant::now());
                cx.notify();
            });
            cx.background_executor().timer(REFRESH_INTERVAL).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        }));
    }

    pub(super) fn render_contributions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self
            .contributions
            .checked_at
            .is_none_or(|t| t.elapsed() >= REFRESH_INTERVAL)
        {
            self.load_contributions(cx);
        }
        let feedback = self
            .checkout_panel
            .feedback
            .get(FEEDBACK_OWNER, FEEDBACK_ACTION);
        let refresh_label = feedback
            .as_ref()
            .map_or("Refresh", |f| f.label.as_str())
            .to_owned();
        let refresh_color = feedback.as_ref().map_or(c::FG_DIM(), |f| {
            if f.completed.is_some() {
                c::STATUS_SUCCESS()
            } else {
                c::STATUS_INFO()
            }
        });
        let refresh_mark = feedback.as_ref().map_or_else(
            || icon("refresh", ICON_SM, c::FG_DIM()).into_any_element(),
            |f| super::action_feedback::mark("contribution-refresh-mark", f, ICON_SM, cx),
        );
        let mut card = project_field_well(false, false)
            .id("home-contributions")
            .debug_selector(|| "home-contributions".into())
            .h_auto()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .p(rpx(SPACE_2XL))
            .gap(rpx(SPACE_MD))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap(rpx(SPACE_SM))
                    .items_center()
                    .child(div().text_size(rpx(TEXT_BODY)).child("Commit activity"))
                    .child(
                        form_action(
                            "refresh-contributions",
                            refresh_label.clone(),
                            false,
                            window,
                            cx,
                        )
                        .debug_selector(|| "contribution-refresh-action".into())
                        .w(rpx(REFRESH_ACTION_W))
                        .icon_only()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(rpx(SPACE_SM))
                                .text_color(refresh_color)
                                .child(refresh_mark)
                                .child(div().child(refresh_label)),
                        )
                        .disabled(self.contributions.loading)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.load_contributions(cx);
                            cx.notify();
                        })),
                    ),
            );
        match &self.contributions.result {
            Some(Ok(activity)) => {
                card = card.child(
                    div().id("contribution-account-scope")
                        .text_size(rpx(TEXT_MICRO)).text_color(c::FG_DIM()).truncate()
                        .tooltip(|window, cx| tooltip("Account-wide GitHub contributions for the last four months (UTC). GitHub contribution rules and account access apply.", window).build(window, cx))
                        .child(format!("@{} · All repositories · 4 months", activity.login)),
                );
                let metrics = [
                    (activity.commits.to_string(), "commits"),
                    (activity.pull_requests.to_string(), "PRs opened"),
                    (activity.repositories.to_string(), "repos"),
                    (format!("{}d", activity.longest_streak), "longest streak"),
                ];
                for row in metrics.chunks(2) {
                    card = card.child(div().flex().w_full().gap(rpx(SPACE_LG)).children(
                        row.iter().map(|(value, label)| {
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_baseline()
                                .gap(rpx(SPACE_SM))
                                .child(div().text_size(rpx(TEXT_BODY)).child(value.clone()))
                                .child(
                                    div()
                                        .text_size(rpx(TEXT_MICRO))
                                        .text_color(c::FG_DIM())
                                        .child(*label),
                                )
                        }),
                    ));
                }
                let colors = c::CONTRIBUTION_HEATMAP();
                let peak = activity
                    .days
                    .iter()
                    .map(|day| day.commits)
                    .max()
                    .unwrap_or(0);
                if let (Some(first), Some(last)) = (activity.days.first(), activity.days.last()) {
                    card = card.child(
                        div()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child(format!(
                                "{} – {} · UTC",
                                first.date.format("%b %-d"),
                                last.date.format("%b %-d")
                            )),
                    );
                    let offset = contributions::weekday(first.date);
                    let slots = offset + activity.days.len();
                    let mut grid = div().flex().gap(rpx(SPACE_XS)).justify_center().w_full();
                    for week in 0..slots.div_ceil(7) {
                        let mut column = div().flex().flex_col().gap(rpx(SPACE_XS));
                        for weekday in 0..7 {
                            let index = (week * 7 + weekday).checked_sub(offset);
                            let day = index.and_then(|i| activity.days.get(i));
                            let mut cell = div()
                                .id(("contribution-day", week * 7 + weekday))
                                .size(rpx(CELL_SIZE))
                                .rounded(rpx(SWATCH_RADIUS));
                            if let Some(day) = day {
                                let label: SharedString = format!(
                                    "{} · {} commits",
                                    day.date.format("%b %-d, %Y"),
                                    day.commits
                                )
                                .into();
                                cell = cell
                                    .aria_label(label.clone())
                                    .bg(colors[heat_level(day.commits, peak)])
                                    .tooltip(move |window, cx| {
                                        tooltip(label.clone(), window).build(window, cx)
                                    });
                            }
                            column = column.child(cell);
                        }
                        grid = grid.child(column);
                    }
                    card = card.child(grid);
                }
                let mut legend = div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(rpx(SPACE_SM))
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .child("Fewer");
                for color in colors {
                    legend = legend.child(
                        div()
                            .size(rpx(LEGEND_SIZE))
                            .rounded(rpx(SWATCH_RADIUS))
                            .bg(color),
                    );
                }
                card = card.child(legend.child("More commits"));
                card = card.child(
                    div()
                        .text_size(rpx(TEXT_MICRO))
                        .text_color(c::FG_DIM())
                        .child(activity.busiest().map_or_else(
                            || "No commits in this window".into(),
                            |day| {
                                format!(
                                    "Busiest day · {} · {} commits",
                                    day.date.format("%b %-d"),
                                    day.commits
                                )
                            },
                        )),
                );
            }
            Some(Err(message)) => {
                card = card.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(message.clone()),
                );
            }
            None => {
                card = card.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child("Loading account activity…"),
                );
            }
        }
        card.into_any_element()
    }
}

fn heat_level(count: u64, peak: u64) -> usize {
    if count == 0 {
        0
    } else {
        1 + ((count - 1) * 4 / peak.max(1)).min(3) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contribution_density_handles_empty_sparse_and_peak_days() {
        assert_eq!(heat_level(0, 0), 0);
        assert_eq!(heat_level(1, 1), 1);
        assert_eq!(
            (1..=12).map(|n| heat_level(n, 12)).collect::<Vec<_>>(),
            vec![1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 4]
        );
    }

    #[gpui::test]
    fn contribution_panel_is_home_only_and_survives_empty_account_data(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext as _;
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
            cx.set_reduce_motion(true);
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.dashboard_open = true;
            sidebar.initial_selection_pending = false;
            sidebar.contributions.result = Some(Err(
                "Sign in with gh auth login to see account activity".into(),
            ));
            sidebar.contributions.checked_at = Some(Instant::now());
            sidebar
        });
        for width in [1280., 760.] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(1000.)));
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(cx.debug_bounds("home-contributions").is_some());
        }
        // Loading, success and restored action keep exactly the same hit bounds.
        let mut action_bounds = None;
        for state in 0..3 {
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, _| {
                    sidebar.contributions.loading = state == 0;
                    if state == 0 {
                        sidebar.checkout_panel.feedback.begin(
                            FEEDBACK_OWNER,
                            FEEDBACK_ACTION,
                            "Refreshing…",
                        );
                    } else {
                        let receipt = sidebar
                            .checkout_panel
                            .feedback
                            .get(FEEDBACK_OWNER, FEEDBACK_ACTION)
                            .unwrap();
                        if state == 1 {
                            sidebar.checkout_panel.feedback.finish(
                                FEEDBACK_OWNER,
                                FEEDBACK_ACTION,
                                receipt.token,
                                Some("Refreshed"),
                                Instant::now(),
                            );
                        } else {
                            sidebar.checkout_panel.feedback.expire(
                                FEEDBACK_OWNER,
                                FEEDBACK_ACTION,
                                receipt.token,
                            );
                        }
                    }
                });
                let _ = window.draw(cx);
            });
            let bounds = cx.debug_bounds("contribution-refresh-action").unwrap();
            if let Some(previous) = action_bounds {
                assert_eq!(bounds, previous);
            }
            action_bounds = Some(bounds);
            assert_eq!(
                cx.debug_bounds("session-spinner-phase-contribution-refresh-mark-0")
                    .is_some(),
                state == 0
            );
        }
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, _| sidebar.dashboard_open = false);
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("home-contributions").is_none());
    }
}
