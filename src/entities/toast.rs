//! The statusbar's transient message and its kind-dependent TTL. Ported from
//! `src/app/mod.rs:26-52,149-160`; unlike the iced poll-based expiry, gpui expiry runs on its
//! own `Timer::after(ttl)` task, so a monotonic `seq` guards against an older toast's timer
//! outliving the toast it was started for and clearing its replacement.
//!
//! No floating toast widget: this is the statusbar's third slot. `animation_clock::toast_pulse`
//! belongs to the grid-tile scrim, not this toast, which stays unconsumed on purpose.

use std::time::{Duration, Instant};

use gpui::{Context, Task};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Error,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub message: String,
    pub kind: ToastKind,
    // Read only by expired_at, called from #[cfg(test)] only; the live toast uses a gpui timer.
    #[allow(dead_code)]
    pub created: Instant,
}

impl Toast {
    /// Errors linger twice as long as informational messages.
    #[must_use]
    pub const fn ttl(kind: ToastKind) -> Duration {
        match kind {
            ToastKind::Info => Duration::from_secs(4),
            ToastKind::Error => Duration::from_secs(8),
        }
    }

    /// Pure so expiry is unit-testable without waiting; production uses the timer.
    #[allow(dead_code)]
    #[must_use]
    pub fn expired_at(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.created) >= Self::ttl(self.kind)
    }
}

#[derive(Default)]
pub struct ToastState {
    current: Option<Toast>,
    /// Bumped on every set; the expiry task clears only if its captured value still matches.
    seq: u64,
    /// Dropping the task cancels it, so this field *is* the pending expiry.
    timer: Option<Task<()>>,
}

impl ToastState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn current(&self) -> Option<&Toast> {
        self.current.as_ref()
    }

    pub fn set_toast(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.show(message, ToastKind::Info, cx);
    }

    pub fn set_error(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.show(message, ToastKind::Error, cx);
    }

    fn show(&mut self, message: impl Into<String>, kind: ToastKind, cx: &mut Context<Self>) {
        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;
        self.current = Some(Toast {
            message: message.into(),
            kind,
            created: Instant::now(),
        });
        // The old task may still be in flight; clear_if_current makes it a no-op.
        self.timer = Some(cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            cx.background_executor().timer(Toast::ttl(kind)).await;
            let _ = this.update(cx, |this: &mut Self, cx| this.clear_if_current(seq, cx));
        }));
        cx.notify();
    }

    pub fn clear_if_current(&mut self, seq: u64, cx: &mut Context<Self>) {
        if seq != self.seq {
            return;
        }
        self.current = None;
        self.timer = None;
        cx.notify();
    }

    /// The pure half of [`Self::clear_if_current`], testable without a gpui `App`.
    #[allow(dead_code)]
    #[must_use]
    pub const fn timer_still_owns_the_toast(&self, seq: u64) -> bool {
        seq == self.seq
    }

    #[allow(dead_code)]
    fn set_without_timer(&mut self, message: &str, kind: ToastKind) -> u64 {
        self.seq = self.seq.wrapping_add(1);
        self.current = Some(Toast {
            message: message.to_string(),
            kind,
            created: Instant::now(),
        });
        self.seq
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_ttl_is_kind_dependent() {
        assert_eq!(Toast::ttl(ToastKind::Info), Duration::from_secs(4));
        assert_eq!(Toast::ttl(ToastKind::Error), Duration::from_secs(8));
    }

    #[test]
    fn toast_expiry_follows_ttl() {
        let t0 = Instant::now();
        let info = Toast {
            message: "copied".into(),
            kind: ToastKind::Info,
            created: t0,
        };
        let error = Toast {
            message: "failed".into(),
            kind: ToastKind::Error,
            created: t0,
        };
        assert!(!info.expired_at(t0 + Duration::from_secs(3)));
        assert!(info.expired_at(t0 + Duration::from_secs(4)));
        assert!(!error.expired_at(t0 + Duration::from_secs(7)));
        assert!(error.expired_at(t0 + Duration::from_secs(8)));
    }

    #[test]
    fn a_superseded_toast_does_not_clear_its_replacement() {
        let mut s = ToastState::new();
        let first = s.set_without_timer("saved", ToastKind::Info);
        let second = s.set_without_timer("failed", ToastKind::Error);
        assert_ne!(first, second);
        assert!(!s.timer_still_owns_the_toast(first));
        assert!(s.timer_still_owns_the_toast(second));
        assert_eq!(s.current().map(|t| t.kind), Some(ToastKind::Error));
        assert_eq!(
            s.current().map(|t| Toast::ttl(t.kind)),
            Some(Duration::from_secs(8))
        );
    }

    #[test]
    fn a_fresh_state_has_no_toast() {
        assert!(ToastState::new().current().is_none());
    }
}
