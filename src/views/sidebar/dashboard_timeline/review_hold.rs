//! Presentation-only review receipts: persistence and live status remain immediate.
use super::*;

const SETTLE_MS: u64 = 400;

#[derive(Default)]
pub(in crate::views::sidebar) struct ReviewHolds {
    latest: HashMap<Scope, SessionListEntry>,
    held: HashMap<Scope, SessionListEntry>,
    hovered: HashSet<Scope>,
    hovered_categories: HashSet<u64>,
    focused: HashSet<Scope>,
    keyboard: HashSet<Scope>,
    timers: HashMap<Scope, gpui::Task<()>>,
    focus: HashMap<Scope, (FocusHandle, Vec<gpui::Subscription>)>,
}

impl ReviewHolds {
    pub(in crate::views::sidebar) fn entries(
        &mut self,
        workspace: u64,
        entries: Vec<SessionListEntry>,
    ) -> Vec<SessionListEntry> {
        // A hidden destination must never retain a pointer/focus hold on return.
        self.held.retain(|key, _| key.0 == workspace);
        self.timers.retain(|key, _| key.0 == workspace);
        self.hovered_categories.retain(|id| *id == workspace);
        entries
            .into_iter()
            .map(|entry| {
                let key = (workspace, entry.id);
                self.latest.insert(key, entry);
                if !matches!(entry.status, "Review" | "Review · unread" | "Ready") {
                    self.held.remove(&key);
                    self.timers.remove(&key);
                }
                if matches!(entry.status, "Review" | "Review · unread")
                    && self
                        .held
                        .get(&key)
                        .is_some_and(|old| old.since != entry.since)
                {
                    self.held.remove(&key);
                    self.timers.remove(&key);
                }
                self.held.get(&key).copied().unwrap_or(entry)
            })
            .collect()
    }

    pub(in crate::views::sidebar) fn begin(
        &mut self,
        workspace: u64,
        id: SessionId,
        keyboard: bool,
    ) {
        let key = (workspace, id);
        if let Some(entry) = self
            .latest
            .get(&key)
            .copied()
            .filter(|entry| matches!(entry.status, "Review" | "Review · unread"))
        {
            self.held.insert(key, entry);
            if keyboard {
                self.keyboard.insert(key);
            } else {
                self.keyboard.remove(&key);
            }
        }
    }

    pub(super) fn clear(&mut self) {
        self.held.clear();
        self.timers.clear();
        self.hovered.clear();
        self.hovered_categories.clear();
        self.focused.clear();
        self.keyboard.clear();
    }

    pub(super) fn retain(&mut self, sessions: &HashSet<SessionId>, workspaces: &HashSet<u64>) {
        let keep = |key: &Scope| sessions.contains(&key.1) && workspaces.contains(&key.0);
        self.latest.retain(|key, _| keep(key));
        self.held.retain(|key, _| keep(key));
        self.timers.retain(|key, _| keep(key));
        self.focus.retain(|key, _| keep(key));
        self.hovered.retain(keep);
        self.hovered_categories.retain(|id| workspaces.contains(id));
        self.focused.retain(keep);
        self.keyboard.retain(keep);
    }
}

impl Sidebar {
    pub(in crate::views::sidebar) fn hover_review_category(
        &mut self,
        workspace: u64,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        let holds = &mut self.dashboard_timelines.review_holds;
        if hovered {
            holds.hovered_categories.insert(workspace);
        } else {
            holds.hovered_categories.remove(&workspace);
        }
        let keys = holds
            .held
            .keys()
            .copied()
            .filter(|key| key.0 == workspace)
            .collect::<Vec<_>>();
        for key in keys {
            self.settle_review_hold(key, cx);
        }
    }

    fn settle_review_hold(&mut self, key: Scope, cx: &mut Context<Self>) {
        let holds = &mut self.dashboard_timelines.review_holds;
        if holds.hovered_categories.contains(&key.0)
            || holds.hovered.contains(&key)
            || (holds.keyboard.contains(&key) && holds.focused.contains(&key))
        {
            holds.timers.remove(&key);
        } else if holds.held.contains_key(&key) && !holds.timers.contains_key(&key) {
            let task = cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(SETTLE_MS))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    let holds = &mut this.dashboard_timelines.review_holds;
                    holds.held.remove(&key);
                    holds.timers.remove(&key);
                    cx.notify();
                });
            });
            self.dashboard_timelines
                .review_holds
                .timers
                .insert(key, task);
        }
    }

    pub(in crate::views::sidebar) fn track_review_hold(
        &mut self,
        key: Scope,
        row: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.dashboard_timelines
            .review_holds
            .focus
            .entry(key)
            .or_insert_with(|| {
                let focus = cx.focus_handle();
                let subscriptions = vec![
                    cx.on_focus_in(&focus, window, move |this, _, cx| {
                        this.dashboard_timelines.review_holds.focused.insert(key);
                        this.settle_review_hold(key, cx);
                    }),
                    cx.on_focus_out(&focus, window, move |this, _, _, cx| {
                        this.dashboard_timelines.review_holds.focused.remove(&key);
                        this.settle_review_hold(key, cx);
                    }),
                ];
                (focus, subscriptions)
            });
        let focus = self.dashboard_timelines.review_holds.focus[&key].0.clone();
        self.settle_review_hold(key, cx);
        div()
            .id(("activity-review-hold", key.1.raw()))
            .w_full()
            .track_focus(&focus)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let holds = &mut this.dashboard_timelines.review_holds;
                if *hovered {
                    holds.hovered.insert(key);
                } else {
                    holds.hovered.remove(&key);
                }
                this.settle_review_hold(key, cx);
            }))
            .child(row)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_hold_preserves_order_but_new_work_bypasses_it() {
        let id = SessionId::from_raw(1);
        let mut holds = ReviewHolds::default();
        let old = SessionListEntry {
            id,
            state: ActivityState::Exited,
            status: "Review",
            since: Some(Instant::now()),
            active: None,
        };
        holds.entries(1, vec![old]);
        holds.begin(1, id, false);
        let ready = SessionListEntry {
            status: "Ready",
            since: None,
            ..old
        };
        assert_eq!(holds.entries(1, vec![ready])[0].status, "Review");
        assert_eq!(holds.entries(1, vec![ready])[0].since, old.since);
        let working = SessionListEntry {
            status: "Working",
            ..ready
        };
        assert_eq!(holds.entries(1, vec![working])[0].status, "Working");
        assert!(holds.held.is_empty());
    }
}

#[cfg(test)]
mod timer_tests {
    use super::*;

    #[gpui::test]
    fn review_hold_waits_400ms_and_cancels_on_return(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::settings::SettingsState::new(
                grove_core::storage::Store::default(),
            ));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        let key = (1, SessionId::from_raw(1));
        cx.update(|_, cx| {
            sidebar.update(cx, |this, cx| {
                let holds = &mut this.dashboard_timelines.review_holds;
                holds.entries(
                    1,
                    vec![SessionListEntry {
                        id: key.1,
                        state: ActivityState::Exited,
                        status: "Review",
                        since: None,
                        active: None,
                    }],
                );
                holds.begin(1, key.1, false);
                holds.hovered.insert(key);
                this.settle_review_hold(key, cx);
                assert!(this.dashboard_timelines.review_holds.timers.is_empty());
                this.dashboard_timelines.review_holds.hovered.remove(&key);
                this.settle_review_hold(key, cx);
            });
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(399));
        cx.run_until_parked();
        assert!(sidebar.read_with(cx, |this, _| this
            .dashboard_timelines
            .review_holds
            .held
            .contains_key(&key)));
        cx.update(|_, cx| {
            sidebar.update(cx, |this, cx| {
                this.dashboard_timelines.review_holds.hovered.insert(key);
                this.settle_review_hold(key, cx);
            });
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(401));
        cx.run_until_parked();
        assert!(sidebar.read_with(cx, |this, _| this
            .dashboard_timelines
            .review_holds
            .held
            .contains_key(&key)));
        // Crossing to a sibling leaves the original row, but stays in Review.
        cx.update(|_, cx| {
            sidebar.update(cx, |this, cx| {
                this.dashboard_timelines.review_holds.hovered.remove(&key);
                this.hover_review_category(key.0, true, cx);
            });
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(500));
        cx.run_until_parked();
        assert!(sidebar.read_with(cx, |this, _| this
            .dashboard_timelines
            .review_holds
            .held
            .contains_key(&key)));
        cx.update(|_, cx| {
            sidebar.update(cx, |this, cx| this.hover_review_category(key.0, false, cx));
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(399));
        cx.run_until_parked();
        // Returning anywhere in the category cancels the nearly expired timer.
        cx.update(|_, cx| {
            sidebar.update(cx, |this, cx| this.hover_review_category(key.0, true, cx));
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(500));
        cx.run_until_parked();
        assert!(sidebar.read_with(cx, |this, _| this
            .dashboard_timelines
            .review_holds
            .held
            .contains_key(&key)));
        cx.update(|_, cx| {
            sidebar.update(cx, |this, cx| {
                this.hover_review_category(key.0, false, cx);
                let holds = &mut this.dashboard_timelines.review_holds;
                holds.hovered.remove(&key);
                holds.keyboard.insert(key);
                holds.focused.insert(key);
                this.settle_review_hold(key, cx);
                assert!(this.dashboard_timelines.review_holds.timers.is_empty());
                this.dashboard_timelines.review_holds.focused.remove(&key);
                this.settle_review_hold(key, cx);
            });
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(399));
        cx.run_until_parked();
        assert!(sidebar.read_with(cx, |this, _| this
            .dashboard_timelines
            .review_holds
            .held
            .contains_key(&key)));
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(1));
        cx.run_until_parked();
        assert!(sidebar.read_with(cx, |this, _| !this
            .dashboard_timelines
            .review_holds
            .held
            .contains_key(&key)));
    }
}
