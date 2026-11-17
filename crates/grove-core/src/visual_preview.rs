//! Bounded bridge from provider workers to the foreground native capture owner.
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
static QUEUE: OnceLock<Mutex<VecDeque<Request>>> = OnceLock::new();
static AVAILABLE: AtomicBool = AtomicBool::new(false);
type Wake = Arc<dyn Fn() + Send + Sync>;
static WAKE: OnceLock<Mutex<Option<Wake>>> = OnceLock::new();
/// Install the foreground wake before enabling capture. Requests stay queued
/// until the foreground accepts them; notification coalescing cannot lose work.
pub fn set_wake(wake: Wake) {
    *WAKE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(wake);
}

pub struct Request {
    pub html: String,
    pub width: u32,
    pub height: u32,
    pub digest: String,
    pub cancelled: Arc<AtomicBool>,
    pub reply: mpsc::SyncSender<Result<Value, String>>,
}
pub fn enable() {
    AVAILABLE.store(true, Ordering::Relaxed);
}
pub fn has_pending() -> bool {
    QUEUE
        .get()
        .and_then(|queue| queue.lock().ok())
        .is_some_and(|mut queue| {
            queue.retain(|request| !request.cancelled.load(Ordering::Relaxed));
            !queue.is_empty()
        })
}

pub fn next() -> Option<Request> {
    QUEUE.get()?.lock().ok()?.pop_front()
}
pub fn capture(
    html: String,
    width: u32,
    height: u32,
    stop: &AtomicBool,
    turn_cancel: &AtomicBool,
) -> Result<Value, String> {
    if !AVAILABLE.load(Ordering::Relaxed) {
        return Err("Native screenshot capture is unavailable: no Grove UI event loop is running. Publication is still available.".into());
    }
    let (reply, receive) = mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let digest = crate::visuals::digest(html.as_bytes());
    {
        let mut queue = QUEUE
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| "Native capture queue unavailable")?;
        if queue.len() >= 2 {
            return Err("Native capture is busy; retry after the current preview".into());
        }
        queue.push_back(Request {
            html,
            width,
            height,
            digest,
            cancelled: cancelled.clone(),
            reply,
        });
    }
    if let Some(wake) = WAKE.get().and_then(|wake| wake.lock().ok()) {
        if let Some(wake) = wake.as_ref() {
            wake();
        }
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if stop.load(Ordering::Relaxed)
            || turn_cancel.load(Ordering::Relaxed)
            || Instant::now() >= deadline
        {
            cancelled.store(true, Ordering::Relaxed);
            return Err("Native screenshot capture cancelled or timed out".into());
        }
        match receive.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Native screenshot backend stopped".into())
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coalesced_wake_retains_multiple_capture_requests() {
        const CHILD: &str = "GROVE_CAPTURE_QUEUE_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "visual_preview::tests::coalesced_wake_retains_multiple_capture_requests",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let (wake, receive) = mpsc::sync_channel(1);
        let notified = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = notified.clone();
        set_wake(Arc::new(move || {
            let _ = wake.try_send(());
            count.fetch_add(1, Ordering::SeqCst);
        }));
        enable();
        let workers: Vec<_> = (0..2)
            .map(|index| {
                std::thread::spawn(move || {
                    capture(
                        format!("<p>{index}</p>"),
                        100,
                        100,
                        &AtomicBool::new(false),
                        &AtomicBool::new(false),
                    )
                })
            })
            .collect();
        let deadline = Instant::now() + Duration::from_secs(2);
        while notified.load(Ordering::SeqCst) < 2 {
            assert!(
                Instant::now() < deadline,
                "capture requests failed to queue"
            );
            std::thread::yield_now();
        }

        receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(receive.try_recv().is_err(), "notification should coalesce");
        next()
            .unwrap()
            .reply
            .send(Ok(serde_json::json!(1)))
            .unwrap();
        assert!(
            has_pending(),
            "pump must keep draining after the first capture finishes"
        );
        next()
            .unwrap()
            .reply
            .send(Ok(serde_json::json!(2)))
            .unwrap();
        assert!(!has_pending());
        for worker in workers {
            assert!(worker.join().unwrap().is_ok());
        }
    }
}
