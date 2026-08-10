//! The statusbar's transient message and its kind-dependent TTL.
//!
//! Ported from `src/app/mod.rs:26-52,149-160`. The data is unchanged; the
//! *expiry mechanism* is not. The iced build polls `expired_at(now)` from the
//! 60ms tick, so a superseded toast is simply overwritten and the poll keeps
//! working. gpui decomposes the tick (spec §4), so expiry is its own
//! `Timer::after(ttl)` task — which means an older toast's timer can outlive
//! the toast it was started for, and must not be allowed to clear the newer
//! one. A monotonic `seq` is what makes the timer idempotent; the test
//! `a_superseded_toast_does_not_clear_its_replacement` is the regression.
//!
//! **There is no floating toast widget** (recorded ambiguity 3): the iced
//! toast is a `text` in the statusbar row (`statusbar.rs:84-97`) and this one
//! is the statusbar's third slot. `animation_clock::toast_pulse` belongs to
//! Plan 07's grid-tile scrim (`terminal.rs:1098`), **not** to this toast, which
//! does not pulse — it stays unconsumed on purpose.

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
    // Read only by `expired_at`, which `#[cfg(test)]` code is what currently
    // calls; the live toast expires on its own gpui timer instead.
    #[allow(dead_code)]
    pub created: Instant,
}

impl Toast {
    /// How long a toast stays up before auto-dismissing: errors linger
    /// twice as long as informational messages.
    #[must_use]
    pub const fn ttl(kind: ToastKind) -> Duration {
        match kind {
            ToastKind::Info => Duration::from_secs(4),
            ToastKind::Error => Duration::from_secs(8),
        }
    }

    /// Whether the toast should be dismissed as of `now`. Pure so expiry is
    /// unit-testable without waiting.
    // The pure, injectable half of expiry — exercised only by this module's
    // `#[cfg(test)]` table so it needs no sleeping; production uses the timer.
    #[allow(dead_code)]
    #[must_use]
    pub fn expired_at(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.created) >= Self::ttl(self.kind)
    }
}

#[derive(Default)]
pub struct ToastState {
    current: Option<Toast>,
    /// Bumped on every set; the expiry task carries the value it was started
    /// with and clears only if it still matches.
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

    /// `App::set_toast` (`src/app/mod.rs:150`).
    pub fn set_toast(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.show(message, ToastKind::Info, cx);
    }

    /// `App::set_error_toast` (`:154`).
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
        // A newer toast supersedes an older one and gets its own full TTL. The
        // old task may still be in flight; `clear_if_current` makes it a no-op.
        self.timer = Some(cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            cx.background_executor().timer(Toast::ttl(kind)).await;
            let _ = this.update(cx, |this: &mut Self, cx| this.clear_if_current(seq, cx));
        }));
        cx.notify();
    }

    /// Expiry, guarded by the sequence the timer was started with.
    pub fn clear_if_current(&mut self, seq: u64, cx: &mut Context<Self>) {
        if seq != self.seq {
            return;
        }
        self.current = None;
        self.timer = None;
        cx.notify();
    }

    /// The pure half of [`Self::clear_if_current`], so supersession is
    /// testable without a gpui `App`.
    // Exercised only by this module's `#[cfg(test)]` supersession assertions.
    #[allow(dead_code)]
    #[must_use]
    pub const fn timer_still_owns_the_toast(&self, seq: u64) -> bool {
        seq == self.seq
    }

    /// Test/pure seam: record a toast without arming a timer.
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

    /// `src/app/mod.rs:721-757`, ported unchanged.
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

    /// The regression the timer mechanism introduces and the polled iced build
    /// got for free: an in-flight timer for a replaced toast must not clear the
    /// toast that replaced it.
    #[test]
    fn a_superseded_toast_does_not_clear_its_replacement() {
        let mut s = ToastState::new();
        let first = s.set_without_timer("saved", ToastKind::Info);
        let second = s.set_without_timer("failed", ToastKind::Error);
        assert_ne!(first, second);
        // The first toast's timer fires late — it no longer owns the slot.
        assert!(!s.timer_still_owns_the_toast(first));
        // The second toast's own timer does, and gets the full error TTL.
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
