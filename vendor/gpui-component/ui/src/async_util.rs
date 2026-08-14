
#[cfg(not(target_family = "wasm"))]
pub use smol::channel::{Receiver, Sender, unbounded};

#[cfg(target_family = "wasm")]
pub use async_channel::{Receiver, Sender, unbounded};
