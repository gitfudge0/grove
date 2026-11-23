//! Native OS notification delivery. Activation returns an opaque, process-local ID.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub(crate) use linux::DesktopNotifications;
#[cfg(target_os = "macos")]
pub(crate) use macos::DesktopNotifications;
#[cfg(target_os = "windows")]
pub(crate) use windows::DesktopNotifications;
