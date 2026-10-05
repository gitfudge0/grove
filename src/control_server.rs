//! Local control requests cross onto the GPUI owner thread before touching live sessions.
use crate::runtime::Runtime;
use futures::{channel::mpsc, StreamExt as _};
use gpui::{Context, Global, Task};
use grove_core::{
    control::{ControlRequest, ControlResponse, Listener},
    control_state::ControlState,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
pub struct ControlEndpoint {
    pub listener: Option<Listener>,
    pub state: Option<ControlState>,
}
impl Global for ControlEndpoint {}
pub fn endpoint() -> Result<Option<ControlEndpoint>, String> {
    if cfg!(not(unix)) {
        return Ok(None);
    }
    let listener = Listener::bind()?;
    let path = grove_core::storage::config_dir()
        .map_err(|e| e.to_string())?
        .join("control")
        .join("state.json");
    let state = ControlState::load(path)?;
    Ok(Some(ControlEndpoint {
        listener: Some(listener),
        state: Some(state),
    }))
}
pub struct ServerGuard(Arc<AtomicBool>);
impl Drop for ServerGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
struct Message {
    request: ControlRequest,
    reply: std::sync::mpsc::Sender<ControlResponse>,
}
pub fn start(
    listener: Listener,
    cx: &mut Context<Runtime>,
) -> Result<(Task<()>, ServerGuard), String> {
    listener.set_nonblocking(true)?;
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let (tx, mut rx) = mpsc::unbounded::<Message>();
    std::thread::Builder::new()
        .name("grove-control".into())
        .spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let mut connection = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "control accept failed");
                        break;
                    }
                };
                let response = match connection.read_request() {
                    Ok(request) => {
                        let (reply, received) = std::sync::mpsc::channel();
                        if tx.unbounded_send(Message { request, reply }).is_err() {
                            break;
                        }
                        received
                            .recv_timeout(Duration::from_secs(30))
                            .unwrap_or_else(|_| {
                                ControlResponse::error(
                                    "timeout",
                                    "Request outcome is uncertain; retry with the same request ID",
                                )
                            })
                    }
                    Err(error) => ControlResponse::error("invalid_request", error),
                };
                if let Err(error) = connection.write_response(&response) {
                    tracing::debug!(%error, "control client disconnected");
                }
            }
        })
        .map_err(|e| e.to_string())?;
    let task = cx.spawn(async move |runtime, cx| {
        while let Some(message) = rx.next().await {
            let response = runtime
                .update(cx, |runtime, cx| {
                    runtime.handle_control(message.request, cx)
                })
                .unwrap_or_else(|_| {
                    ControlResponse::error(
                        "app_closed",
                        "Grove closed before processing the request",
                    )
                });
            let _ = message.reply.send(response);
        }
    });
    Ok((task, ServerGuard(stopped)))
}
