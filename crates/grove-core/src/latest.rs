//! Single-consumer state transport: pending states are replaced, never queued.
//! This is for complete snapshots, not events or acknowledgements that disappear
//! from subsequent snapshots. Standard channels can be adapted for test fixtures.
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub enum Sender<T> {
    Latest {
        slot: Arc<Mutex<Option<T>>>,
        wake: mpsc::SyncSender<()>,
    },
    Channel(mpsc::Sender<T>),
}

pub enum Receiver<T> {
    Latest {
        slot: Arc<Mutex<Option<T>>>,
        wake: mpsc::Receiver<()>,
    },
    Channel(mpsc::Receiver<T>),
}

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let slot = Arc::new(Mutex::new(None));
    let (send, receive) = mpsc::sync_channel(1);
    (
        Sender::Latest {
            slot: slot.clone(),
            wake: send,
        },
        Receiver::Latest {
            slot,
            wake: receive,
        },
    )
}

impl<T> From<mpsc::Sender<T>> for Sender<T> {
    fn from(value: mpsc::Sender<T>) -> Self {
        Self::Channel(value)
    }
}
impl<T> From<mpsc::Receiver<T>> for Receiver<T> {
    fn from(value: mpsc::Receiver<T>) -> Self {
        Self::Channel(value)
    }
}
impl<T> Sender<T> {
    pub fn send(&self, value: T) -> Result<(), mpsc::SendError<T>> {
        match self {
            Self::Channel(send) => send.send(value),
            Self::Latest { slot, wake } => {
                let mut slot = slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match wake.try_send(()) {
                    Ok(()) | Err(mpsc::TrySendError::Full(())) => {
                        *slot = Some(value);
                        Ok(())
                    }
                    Err(mpsc::TrySendError::Disconnected(())) => {
                        slot.take();
                        Err(mpsc::SendError(value))
                    }
                }
            }
        }
    }
}
impl<T> Receiver<T> {
    pub fn try_recv(&self) -> Result<T, mpsc::TryRecvError> {
        match self {
            Self::Channel(receive) => receive.try_recv(),
            Self::Latest { slot, wake } => loop {
                wake.try_recv()?;
                if let Some(value) = slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                {
                    return Ok(value);
                }
            },
        }
    }
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, mpsc::RecvTimeoutError> {
        match self {
            Self::Channel(receive) => receive.recv_timeout(timeout),
            Self::Latest { slot, wake } => {
                let started = Instant::now();
                loop {
                    wake.recv_timeout(timeout.saturating_sub(started.elapsed()))?;
                    if let Some(value) = slot
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                    {
                        return Ok(value);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn burst_keeps_one_snapshot_and_delivers_latest_before_disconnect() {
        struct State(usize, Arc<AtomicUsize>);
        impl Drop for State {
            fn drop(&mut self) {
                self.1.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let live = Arc::new(AtomicUsize::new(0));
        let (baseline_send, baseline_receive) = mpsc::channel();
        for index in 0..10_000 {
            live.fetch_add(1, Ordering::SeqCst);
            assert!(baseline_send.send(State(index, live.clone())).is_ok());
        }
        assert_eq!(live.load(Ordering::SeqCst), 10_000);
        drop(baseline_receive);
        drop(baseline_send);
        assert_eq!(live.load(Ordering::SeqCst), 0);
        let (send, receive) = channel();
        for index in 0..10_000 {
            live.fetch_add(1, Ordering::SeqCst);
            assert!(send.send(State(index, live.clone())).is_ok());
            assert_eq!(live.load(Ordering::SeqCst), 1);
        }
        drop(send);
        let latest = receive.recv_timeout(Duration::ZERO).unwrap();
        assert_eq!(latest.0, 9_999);
        drop(latest);
        assert_eq!(live.load(Ordering::SeqCst), 0);
        assert!(matches!(
            receive.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn receiver_drop_rejects_send_and_empty_mailbox_times_out() {
        let (send, receive) = channel::<usize>();
        assert!(matches!(receive.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert!(matches!(
            receive.recv_timeout(Duration::ZERO),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        drop(receive);
        assert_eq!(send.send(7).unwrap_err().0, 7);
    }

    #[test]
    fn concurrent_consumer_observes_monotonic_states_and_final_value() {
        let (send, receive) = channel();
        let producer = std::thread::spawn(move || {
            for state in 0..10_000 {
                send.send(state).unwrap();
            }
        });
        let mut previous = None;
        loop {
            match receive.recv_timeout(Duration::from_secs(5)) {
                Ok(state) => {
                    assert!(previous.is_none_or(|previous| state > previous));
                    previous = Some(state);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("lost mailbox wakeup"),
            }
        }
        producer.join().unwrap();
        assert_eq!(previous, Some(9_999));
    }
}
