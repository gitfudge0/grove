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
    pending: Option<(usize, Instant)>,
}

#[derive(Default)]
pub(super) struct SessionPlacement {
    workspace: Option<u64>,
    rows: HashMap<SessionId, Placement>,
}

impl SessionPlacement {
    pub(super) fn status(&self, id: SessionId, live: &'static str) -> &'static str {
        self.rows.get(&id).map_or(live, |row| LABELS[row.displayed])
    }

    fn observe(&mut self, id: SessionId, target: usize, now: Instant) -> Option<Instant> {
        let row = self.rows.entry(id).or_insert(Placement {
            displayed: target,
            pending: None,
        });
        if target == row.displayed || target == 0 {
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
