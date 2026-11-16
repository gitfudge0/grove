//! Delay navigation placement, while the session's visible status stays live.
use super::*;

const SETTLE: Duration = Duration::from_millis(500);
const LABELS: [&str; 4] = ["Needs you", "Review", "Working", "Ready"];

pub(super) fn section(status: &str) -> usize {
    match status {
        "Approval needed" | "Answer needed" | "Needs you" | "Failed" | "Disconnected"
        | "Reconnecting" | "Usage limit" | "Unknown" => 0,
        "Done" | "Review" | "Review · unread" | "Plan ready" => 1,
        "Starting" | "Working" | "Waiting" => 2,
        _ => 3,
    }
}

struct Placement {
    displayed: usize,
    position: u64,
    pending: Option<(usize, Instant)>,
}

#[derive(Default)]
pub(super) struct SessionPlacement {
    workspace: Option<u64>,
    next_position: u64,
    rows: HashMap<SessionId, Placement>,
}

impl SessionPlacement {
    pub(super) fn status(&self, id: SessionId, live: &'static str) -> &'static str {
        self.rows.get(&id).map_or(live, |row| LABELS[row.displayed])
    }

    fn position(&self, id: SessionId) -> u64 {
        self.rows.get(&id).map_or(u64::MAX, |row| row.position)
    }

    /// The dashboard shares status grouping, but only navigation uses stable slots.
    pub(super) fn sort(&self, ids: &mut [SessionId]) {
        ids.sort_by_key(|&id| (self.position(id), id));
    }

    fn observe(&mut self, id: SessionId, target: usize, now: Instant) -> Option<Instant> {
        let row = self.rows.entry(id).or_insert_with(|| {
            let position = self.next_position;
            self.next_position += 1;
            Placement {
                displayed: target,
                position,
                pending: None,
            }
        });
        if target == row.displayed || target == 0 {
            if row.displayed != target {
                row.position = self.next_position;
                self.next_position += 1;
            }
            row.displayed = target;
            row.pending = None;
            return None;
        }
        let (pending, since) = *row.pending.get_or_insert((target, now));
        if pending != target {
            row.pending = Some((target, now));
            return Some(now + SETTLE);
        }
        if now >= since + SETTLE {
            row.position = self.next_position;
            self.next_position += 1;
            row.displayed = target;
            row.pending = None;
            None
        } else {
            Some(since + SETTLE)
        }
    }
}

impl Sidebar {
    pub(super) fn sync_session_placement(&mut self, cx: &mut Context<Self>) {
        if self.session_placement.workspace != Some(self.active_workspace) {
            self.session_placement.rows.clear();
            self.session_placement.next_position = 0;
            self.session_placement.workspace = Some(self.active_workspace);
        }
        let now = Instant::now();
        let targets: Vec<_> = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .all()
            .iter()
            .map(|meta| (meta.id, section(self.status(meta, cx).0)))
            .collect();
        let known: HashSet<_> = targets.iter().map(|(id, _)| *id).collect();
        self.session_placement
            .rows
            .retain(|id, _| known.contains(id));
        let deadline = targets
            .into_iter()
            .filter_map(|(id, target)| self.session_placement.observe(id, target, now))
            .min();
        self.placement_timer = deadline.map(|deadline| {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(deadline.saturating_duration_since(Instant::now()))
                    .await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_arrivals_append_and_cancelled_transfers_keep_their_slot() {
        let mut placements = SessionPlacement::default();
        let ids = [1, 2, 3].map(SessionId::from_raw);
        let now = Instant::now();
        for id in ids {
            placements.observe(id, 2, now);
        }
        let order = |placements: &SessionPlacement| {
            let mut groups = Vec::new();
            for label in LABELS {
                let mut members: Vec<_> = ids
                    .into_iter()
                    .filter(|&id| placements.status(id, "Working") == label)
                    .collect();
                placements.sort(&mut members);
                if !members.is_empty() {
                    groups.push((label, members));
                }
            }
            groups
        };
        // Repeated updates and a cancelled move preserve the original order.
        placements.observe(ids[0], 1, now);
        placements.observe(ids[0], 2, now + SETTLE);
        placements.observe(ids[1], 2, now + SETTLE);
        assert_eq!(order(&placements), vec![("Working", ids.to_vec())]);
        // Attention is immediate; returning waits and then appends after residents.
        placements.observe(ids[0], 0, now + SETTLE);
        assert_eq!(
            order(&placements),
            vec![
                ("Needs you", vec![ids[0]]),
                ("Working", vec![ids[1], ids[2]])
            ]
        );
        placements.observe(ids[0], 2, now + SETTLE);
        placements.observe(ids[0], 2, now + SETTLE + SETTLE);
        assert_eq!(
            order(&placements),
            vec![("Working", vec![ids[1], ids[2], ids[0]])]
        );
        // Subsequent updates do not replay arrival ordering.
        placements.observe(ids[1], 2, now + SETTLE + SETTLE);
        assert_eq!(
            order(&placements),
            vec![("Working", vec![ids[1], ids[2], ids[0]])]
        );
        let new_id = SessionId::from_raw(4);
        placements.observe(new_id, 2, now + SETTLE + SETTLE);
        let mut residents = [new_id, ids[0], ids[2], ids[1]];
        placements.sort(&mut residents);
        assert_eq!(residents, [ids[1], ids[2], ids[0], new_id]);
    }

    /// Shared native state and navigation fixture, without live provider transports.
    #[gpui::test]
    fn codex_and_claude_session_order_ignores_activity_and_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(Runtime::new);
            let ids = runtime.read(cx).registry.clone().update(cx, |registry, _| {
                [Agent::Codex, Agent::Claude, Agent::Codex, Agent::Claude].map(|agent| {
                    registry.insert_meta(String::new(), "/tmp/stable-session-order".into(), agent)
                })
            });
            runtime.read(cx).activity.clone().update(cx, |activity, _| {
                for id in &ids[..2] {
                    activity.set_state_for_test(*id, ActivityState::Working);
                }
            });
            Sidebar::new(runtime, window, cx)
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let ids: Vec<_> = sidebar
                    .runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .all()
                    .iter()
                    .map(|meta| meta.id)
                    .collect();
                let activity = sidebar.runtime.read(cx).activity.clone();
                sidebar.sync_session_placement(cx);
                let before = sidebar.sidebar_session_groups(cx);
                assert_eq!(
                    before,
                    vec![
                        ("Working".into(), ids[..2].to_vec()),
                        ("Ready".into(), ids[2..].to_vec())
                    ]
                );
                // Reverse activity recency, then exercise actual selection/acknowledgment.
                activity.update(cx, |activity, cx| {
                    activity.acknowledge(ids[1], cx);
                    activity.acknowledge(ids[0], cx);
                });
                for id in ids.iter().rev() {
                    sidebar.select_session_id(*id, window, cx);
                }
                sidebar.sync_session_placement(cx);
                assert_eq!(sidebar.sidebar_session_groups(cx), before);
                assert_eq!(sidebar.visible_session_order(cx), ids);
                // Immediate Needs you moves only that session; removal preserves residents.
                activity.update(cx, |activity, _| {
                    activity.set_state_for_test(ids[0], ActivityState::WaitingForInput)
                });
                sidebar.sync_session_placement(cx);
                assert_eq!(
                    sidebar.sidebar_session_groups(cx),
                    vec![
                        ("Needs you".into(), vec![ids[0]]),
                        ("Working".into(), vec![ids[1]]),
                        ("Ready".into(), ids[2..].to_vec())
                    ]
                );
                let registry = sidebar.runtime.read(cx).registry.clone();
                registry.update(cx, |registry, _| {
                    registry.remove(ids[0]);
                });
                sidebar.sync_session_placement(cx);
                assert!(!sidebar.session_placement.rows.contains_key(&ids[0]));
                assert_eq!(sidebar.visible_session_order(cx), ids[1..]);
                sidebar.active_workspace += 1;
                sidebar.sync_session_placement(cx);
                assert_eq!(sidebar.session_placement.position(ids[1]), 0);
                assert_eq!(sidebar.visible_session_order(cx), ids[1..]);
            })
        });
    }

    #[test]
    fn placement_coalesces_bursts_without_delaying_attention() {
        let mut placements = SessionPlacement::default();
        let id = SessionId::from_raw(1);
        let now = Instant::now();
        placements.observe(id, 2, now);
        placements.observe(id, 1, now);
        placements.observe(id, 3, now + Duration::from_millis(200));
        placements.observe(id, 3, now + Duration::from_millis(699));
        assert_eq!(placements.status(id, "Ready"), "Working");
        placements.observe(id, 3, now + Duration::from_millis(700));
        assert_eq!(placements.status(id, "Ready"), "Ready");
        placements.observe(id, 2, now + Duration::from_millis(701));
        placements.observe(id, 0, now + Duration::from_millis(702));
        assert_eq!(placements.status(id, "Approval needed"), "Needs you");
        placements.observe(id, 2, now + Duration::from_millis(703));
        placements.observe(id, 0, now + Duration::from_millis(900));
        assert!(placements.rows[&id].pending.is_none());
    }

    #[test]
    fn returning_to_displayed_group_cancels_pending_move() {
        let mut placements = SessionPlacement::default();
        let id = SessionId::from_raw(2);
        let now = Instant::now();
        placements.observe(id, 2, now);
        placements.observe(id, 1, now);
        placements.observe(id, 2, now + Duration::from_millis(499));
        placements.observe(id, 1, now + Duration::from_millis(500));
        placements.observe(id, 1, now + Duration::from_millis(999));
        assert_eq!(placements.status(id, "Done"), "Working");
        placements.observe(id, 1, now + Duration::from_secs(1));
        assert_eq!(placements.status(id, "Done"), "Review");
    }
}
