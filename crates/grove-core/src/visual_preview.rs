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
