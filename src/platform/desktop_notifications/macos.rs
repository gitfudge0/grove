//! Notification Center bridge. The retained native delegate and Rust callback
//! owner are both tied to the main-thread lifetime of this backend.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::ptr::NonNull;

use futures::channel::mpsc::UnboundedSender;

pub(crate) struct DesktopNotifications {
    native: Option<NonNull<c_void>>,
    _activated: Box<UnboundedSender<u64>>,
}

impl DesktopNotifications {
    pub(crate) fn new(activated: UnboundedSender<u64>) -> Self {
        let activated = Box::new(activated);
        // SAFETY: the boxed callback context has a stable address and outlives
        // the native delegate. Construction and disposal run on GPUI's thread.
        let native = unsafe {
            grove_notifications_create(std::ptr::from_ref(activated.as_ref()).cast(), activate)
        };
        Self {
            native: NonNull::new(native),
            _activated: activated,
        }
    }

    pub(crate) fn show(&mut self, id: u64, title: &'static str, body: String) {
        if let Some(native) = self.native {
            // SAFETY: the delegate is retained and copies the UTF-8 bytes
            // synchronously before starting its asynchronous work.
            unsafe {
                grove_notifications_show(
                    native.as_ptr(),
                    id,
                    title.as_ptr(),
                    title.len(),
                    body.as_ptr(),
                    body.len(),
                );
            }
        }
    }

    pub(crate) fn close(&mut self, id: u64) {
        if let Some(native) = self.native {
            // SAFETY: the retained delegate is accessed only on GPUI's thread.
            unsafe { grove_notifications_close(native.as_ptr(), id) };
        }
    }
}

impl Drop for DesktopNotifications {
    fn drop(&mut self) {
        if let Some(native) = self.native {
            // SAFETY: disposal clears the native callback before the boxed
            // sender is dropped, including for native work still queued.
            unsafe { grove_notifications_dispose(native.as_ptr()) };
        }
    }
}

extern "C" fn activate(context: *const c_void, id: u64) {
    // SAFETY: the delegate only invokes this callback on the main thread
    // with the original context and while its Rust owner is alive.
    let activated = unsafe { &*context.cast::<UnboundedSender<u64>>() };
    let _ = activated.unbounded_send(id);
}

extern "C" {
    fn grove_notifications_create(
        context: *const c_void,
        activate: extern "C" fn(*const c_void, u64),
    ) -> *mut c_void;
    fn grove_notifications_show(
        handle: *mut c_void,
        id: u64,
        title: *const u8,
        title_len: usize,
        body: *const u8,
        body_len: usize,
    );
    fn grove_notifications_close(handle: *mut c_void, id: u64);
    fn grove_notifications_dispose(handle: *mut c_void);
}
