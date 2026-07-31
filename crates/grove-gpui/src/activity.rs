//! Per-session activity state as the sidebar sees it.
//!
//! [`ActivityState`] (`src/gui/activity.rs:30-36`) and [`most_urgent`] /
//! `urgency_rank` (`:201-222`) are ported verbatim — they are pure. The
//! classifier is **not**: `classify`, `Signals`, `Tracker` and every timing
//! constant belong to Plan 06, which also owns the 480ms classification task,
//! the hook state file, the native poller and the 1s auto-reverse pulse.
//!
//! [`ActivityStore`] is the stub interface those live sources will fill
//! (carried amendment 3). Every call site reads through it, so no view ever
//! branches on "attention isn't implemented yet".

// The store's readers land in Task 5; the entity is created in Task 6.
#![allow(dead_code)]

use crate::entities::session_registry::SessionId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityState {
    Working,
    WaitingForInput,
    Done,
    Idle,
    Exited,
}

/// Roll-up urgency for collapsed parent rows: waiting > working > done;
/// idle/exited contribute nothing (`src/gui/activity.rs:201-215`).
pub fn most_urgent(states: impl Iterator<Item = ActivityState>) -> Option<ActivityState> {
    let mut best: Option<ActivityState> = None;
    for s in states {
        let rank = urgency_rank(s);
        if rank.is_none() {
            continue;
        }
        if rank > best.and_then(urgency_rank) {
            best = Some(s);
        }
    }
    best
}

fn urgency_rank(s: ActivityState) -> Option<u8> {
    match s {
        ActivityState::WaitingForInput => Some(3),
        ActivityState::Working => Some(2),
        ActivityState::Done => Some(1),
        ActivityState::Idle | ActivityState::Exited => None,
    }
}

/// The sidebar-facing interface for activity and attention.
#[derive(Default)]
pub struct ActivityStore;

impl ActivityStore {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// The classified state of a session. Unknown sessions render `Idle`, the
    /// same as the iced build before its first classification tick
    /// (`update/mod.rs:710-716`).
    #[must_use]
    pub fn state_of(&self, _id: SessionId) -> ActivityState {
        // Plan 06: data source
        ActivityState::Idle
    }

    /// Needs-attention pulse phase in `[0, 1]` (0 = fully opaque, 1 = maximum
    /// dim), so callers interpolate unconditionally
    /// (`update/mod.rs:719-726`).
    #[must_use]
    pub fn pulse(&self) -> f32 {
        // Plan 06: data source
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_outranks_working_outranks_done() {
        use ActivityState::{Done, WaitingForInput, Working};
        assert_eq!(
            most_urgent([Done, Working, WaitingForInput].into_iter()),
            Some(WaitingForInput)
        );
        assert_eq!(most_urgent([Done, Working].into_iter()), Some(Working));
        assert_eq!(most_urgent([Done].into_iter()), Some(Done));
    }

    #[test]
    fn idle_and_exited_contribute_nothing() {
        use ActivityState::{Exited, Idle};
        assert_eq!(most_urgent([Idle, Exited].into_iter()), None);
        assert_eq!(
            most_urgent([Idle, ActivityState::Done, Exited].into_iter()),
            Some(ActivityState::Done)
        );
    }

    #[test]
    fn an_empty_iterator_has_no_rollup() {
        assert_eq!(most_urgent(std::iter::empty()), None);
    }

    #[test]
    fn the_stub_store_classifies_everything_idle_with_no_pulse() {
        let store = ActivityStore::new();
        assert_eq!(store.state_of(SessionId::from_raw(1)), ActivityState::Idle);
        assert!((store.pulse() - 0.0).abs() < f32::EPSILON);
    }
}
