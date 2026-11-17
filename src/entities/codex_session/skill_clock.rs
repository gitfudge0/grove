//! One foreground wake cadence for all connected-session skill checks.
use futures::channel::oneshot;
use gpui::{App, Task};
use std::{
    cell::RefCell,
    collections::HashMap,
    future::Future,
    pin::Pin,
    rc::{Rc, Weak},
    task::{Context, Poll},
    time::Duration,
};

const PERIOD: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Clock(Rc<RefCell<State>>);
impl gpui::Global for Clock {}

#[derive(Default)]
struct State {
    next_id: u64,
    waiters: HashMap<u64, oneshot::Sender<()>>,
    timer: Option<Task<()>>,
    #[cfg(test)]
    ticks: u64,
}

pub(super) struct Tick {
    id: u64,
    receiver: oneshot::Receiver<()>,
    clock: Weak<RefCell<State>>,
}
impl Future for Tick {
    type Output = Result<(), oneshot::Canceled>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.receiver).poll(cx)
    }
}
impl Drop for Tick {
    fn drop(&mut self) {
        if let Some(clock) = self.clock.upgrade() {
            let mut clock = clock.borrow_mut();
            clock.waiters.remove(&self.id);
            if clock.waiters.is_empty() {
                // Dropping the last session cancels the shared pending timer.
                clock.timer = None;
            }
        }
    }
}

pub(super) fn wait(cx: &mut App) -> Tick {
    if !cx.has_global::<Clock>() {
        cx.set_global(Clock::default());
    }
    let clock = cx.global::<Clock>().0.clone();
    let (sender, receiver) = oneshot::channel();
    let mut state = clock.borrow_mut();
    let id = state.next_id;
    state.next_id = state.next_id.wrapping_add(1);
    state.waiters.insert(id, sender);
    if state.timer.is_none() {
        let clock = Rc::downgrade(&clock);
        state.timer = Some(cx.spawn(async move |cx| {
            cx.background_executor().timer(PERIOD).await;
            let Some(clock) = clock.upgrade() else {
                return;
            };
            let waiters = {
                let mut state = clock.borrow_mut();
                state.timer = None;
                #[cfg(test)]
                {
                    state.ticks += 1;
                }
                std::mem::take(&mut state.waiters)
            };
            for waiter in waiters.into_values() {
                let _ = waiter.send(());
            }
        }));
    }
    Tick {
        id,
        receiver,
        clock: Rc::downgrade(&clock),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt as _;

    #[gpui::test]
    fn clients_share_one_tick_and_last_drop_cancels_it(cx: &mut gpui::TestAppContext) {
        let first = cx.update(wait);
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(4));
        let second = cx.update(wait);
        cx.update(|cx| {
            let state = cx.global::<Clock>().0.borrow();
            assert_eq!(state.waiters.len(), 2);
            assert!(state.timer.is_some());
        });
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert!(matches!(first.now_or_never(), Some(Ok(()))));
        assert!(matches!(second.now_or_never(), Some(Ok(()))));
        cx.update(|cx| {
            let state = cx.global::<Clock>().0.borrow();
            assert!(state.waiters.is_empty());
            assert!(state.timer.is_none());
        });

        let first = cx.update(wait);
        let second = cx.update(wait);
        cx.run_until_parked();
        drop(first);
        cx.update(|cx| {
            let state = cx.global::<Clock>().0.borrow();
            assert_eq!(state.waiters.len(), 1);
            assert!(state.timer.is_some());
        });
        drop(second);
        cx.update(|cx| {
            let state = cx.global::<Clock>().0.borrow();
            assert!(state.waiters.is_empty());
            assert!(state.timer.is_none());
        });
        cx.executor().advance_clock(Duration::from_secs(10));
        cx.run_until_parked();
        cx.update(|cx| {
            let state = cx.global::<Clock>().0.borrow();
            assert!(state.timer.is_none());
            assert_eq!(state.ticks, 1, "last-client drop cancels the armed timer");
        });
    }
}
