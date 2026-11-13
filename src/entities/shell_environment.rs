//! Refresh the launch environment independently of settings panel lifetime.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use futures::future::BoxFuture;
use gpui::{AppContext as _, Context, Task};

pub const REFRESH_INTERVAL: Duration = Duration::from_mins(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshState {
    Idle,
    Refreshing,
    Refreshed,
    Error,
}

type RefreshOperation = Arc<dyn Fn() -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

pub struct ShellEnvironment {
    state: RefreshState,
    refresh_operation: RefreshOperation,
    _timer: Task<()>,
    refresh_task: Option<Task<()>>,
    confirmation_task: Option<Task<()>>,
    refreshed_at: Option<Instant>,
    generation: u64,
}

impl ShellEnvironment {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self::with_operation(
            Arc::new(|| Box::pin(async { grove_core::env_path::refresh_login_environment() })),
            cx,
        )
    }

    fn with_operation(operation: RefreshOperation, cx: &mut Context<Self>) -> Self {
        let timer = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(REFRESH_INTERVAL).await;
            if this.update(cx, Self::refresh).is_err() {
                return;
            }
        });
        Self {
            state: RefreshState::Idle,
            refresh_operation: operation,
            _timer: timer,
            refresh_task: None,
            confirmation_task: None,
            refreshed_at: None,
            generation: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_fixture(&mut self, state: RefreshState, cx: &mut Context<Self>) {
        self.refresh_task = None;
        self.confirmation_task = None;
        self.state = state;
        self.generation += 1;
        self.refreshed_at = (state == RefreshState::Refreshed).then(Instant::now);
        cx.notify();
    }

    pub fn state(&self) -> RefreshState {
        self.state
    }

    pub fn confirmation(&self) -> Option<(u64, Instant)> {
        self.refreshed_at.map(|started| (self.generation, started))
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if cfg!(windows) || self.state == RefreshState::Refreshing {
            return;
        }
        self.confirmation_task = None;
        self.refreshed_at = None;
        self.generation = self.generation.wrapping_add(1);
        self.state = RefreshState::Refreshing;
        cx.notify();
        let operation = self.refresh_operation.clone();
        let fetch = cx.background_spawn(async move { operation().await });
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            let result = fetch.await;
            let _ = this.update(cx, |this, cx| {
                this.state = if result.is_ok() {
                    RefreshState::Refreshed
                } else {
                    RefreshState::Error
                };
                if this.state == RefreshState::Refreshed {
                    this.refreshed_at = Some(Instant::now());
                    let generation = this.generation;
                    this.confirmation_task = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(crate::views::status::CHECK_CONFIRMATION_LIFETIME)
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            if this.generation == generation
                                && this.state == RefreshState::Refreshed
                            {
                                this.state = RefreshState::Idle;
                                this.refreshed_at = None;
                                cx.notify();
                            }
                        });
                    }));
                }
                cx.notify();
            });
        }));
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[gpui::test]
    fn confirmation_expires_and_new_work_supersedes_it(cx: &mut gpui::TestAppContext) {
        let calls = Arc::new(AtomicUsize::new(0));
        let environment = cx.new(|cx| {
            ShellEnvironment::with_operation(
                Arc::new(move || {
                    let call = calls.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async move {
                        if call < 2 {
                            Ok(())
                        } else {
                            Err("Failure".into())
                        }
                    })
                }),
                cx,
            )
        });
        environment.update(cx, ShellEnvironment::refresh);
        cx.run_until_parked();
        assert!(environment.read_with(cx, |environment, _| environment.confirmation().is_some()));
        cx.executor()
            .advance_clock(crate::views::status::CHECK_CONFIRMATION_LIFETIME);
        cx.run_until_parked();
        assert_eq!(
            environment.read_with(cx, |environment, _| environment.state()),
            RefreshState::Idle
        );
        environment.update(cx, ShellEnvironment::refresh);
        cx.run_until_parked();
        environment.update(cx, ShellEnvironment::refresh);
        cx.run_until_parked();
        cx.executor()
            .advance_clock(crate::views::status::CHECK_CONFIRMATION_LIFETIME);
        cx.run_until_parked();
        environment.read_with(cx, |environment, _| {
            assert_eq!(environment.state(), RefreshState::Error);
            assert!(environment.confirmation().is_none());
        });
    }

    #[gpui::test]
    fn manual_refresh_deduplicates_and_reports_success(cx: &mut gpui::TestAppContext) {
        let calls = Arc::new(AtomicUsize::new(0));
        let (send, receive) = futures::channel::oneshot::channel();
        let receive = Arc::new(std::sync::Mutex::new(Some(receive)));
        let environment = cx.new({
            let calls = calls.clone();
            move |cx| {
                ShellEnvironment::with_operation(
                    Arc::new(move || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        let receive = receive.lock().unwrap().take().unwrap();
                        Box::pin(async move { receive.await.unwrap() })
                    }),
                    cx,
                )
            }
        });
        environment.update(cx, |environment, cx| {
            environment.refresh(cx);
            environment.refresh(cx);
            assert_eq!(environment.state(), RefreshState::Refreshing);
        });
        cx.run_until_parked();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        send.send(Ok(())).unwrap();
        cx.run_until_parked();
        environment.read_with(cx, |environment, _| {
            assert_eq!(environment.state(), RefreshState::Refreshed);
        });
    }

    #[gpui::test]
    fn periodic_refresh_waits_ten_minutes_and_stops_with_entity(cx: &mut gpui::TestAppContext) {
        let calls = Arc::new(AtomicUsize::new(0));
        let environment = cx.new({
            let calls = calls.clone();
            move |cx| {
                ShellEnvironment::with_operation(
                    Arc::new(move || {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Box::pin(async { Err("private shell output".into()) })
                    }),
                    cx,
                )
            }
        });
        cx.run_until_parked();
        cx.executor().advance_clock(
            REFRESH_INTERVAL
                .checked_sub(Duration::from_secs(1))
                .unwrap(),
        );
        cx.run_until_parked();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        environment.read_with(cx, |environment, _| {
            assert_eq!(environment.state(), RefreshState::Error);
        });
        cx.executor().advance_clock(REFRESH_INTERVAL);
        cx.run_until_parked();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        drop(environment);
        cx.run_until_parked();
        cx.executor().advance_clock(REFRESH_INTERVAL);
        cx.run_until_parked();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
