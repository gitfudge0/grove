//! Main-thread-only WebKit ownership. Non-macOS and GPUI test windows keep source.
#![cfg_attr(target_os = "macos", allow(unsafe_code))]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadStatus {
    Loading,
    Ready,
    Failed,
}

pub struct NativeDocument {
    #[cfg(test)]
    visibility_probe: std::rc::Rc<std::cell::Cell<bool>>,
    #[cfg(all(target_os = "macos", not(test)))]
    handle: std::ptr::NonNull<std::ffi::c_void>,
}

impl NativeDocument {
    /// Visibility-only seam for exercising the real GPUI element/frame lifetime.
    /// WebKit policy and document behavior are tested by the standalone harness.
    #[cfg(test)]
    pub fn with_visibility_probe(visible: std::rc::Rc<std::cell::Cell<bool>>) -> Self {
        Self {
            visibility_probe: visible,
        }
    }

    pub fn hide_all() {
        #[cfg(all(target_os = "macos", not(test)))]
        // SAFETY: called on GPUI's AppKit thread at visibility transitions.
        unsafe {
            grove_html_hide_all();
        };
    }

    pub fn new(source: &str) -> Option<Self> {
        #[cfg(all(target_os = "macos", not(test)))]
        {
            // SAFETY: called only on GPUI's AppKit thread. The native owner copies
            // the UTF-8 bytes synchronously and returns a retained opaque object.
            let handle = unsafe { grove_html_create(source.as_ptr(), source.len()) };
            std::ptr::NonNull::new(handle).map(|handle| Self { handle })
        }
        #[cfg(any(not(target_os = "macos"), test))]
        {
            let _ = source;
            None
        }
    }

    pub fn status(&self) -> LoadStatus {
        #[cfg(all(target_os = "macos", not(test)))]
        {
            // SAFETY: this object retains the handle for its entire lifetime.
            match unsafe { grove_html_status(self.handle.as_ptr()) } {
                0 => LoadStatus::Loading,
                1 => LoadStatus::Ready,
                _ => LoadStatus::Failed,
            }
        }
        #[cfg(all(not(target_os = "macos"), not(test)))]
        {
            LoadStatus::Failed
        }
        #[cfg(test)]
        {
            LoadStatus::Ready
        }
    }

    pub fn retire_frame(&self) {
        #[cfg(test)]
        self.visibility_probe.set(false);
        #[cfg(all(target_os = "macos", not(test)))]
        // SAFETY: the main-queue callback uses a weak native owner and compares
        // paint generations after GPUI completes its replacement frame.
        unsafe {
            grove_html_hide_after_paint(self.handle.as_ptr());
        }
    }

    pub fn hide(&self) {
        #[cfg(test)]
        self.visibility_probe.set(false);
        #[cfg(all(target_os = "macos", not(test)))]
        // SAFETY: GPUI element teardown and entity drops execute on the UI thread.
        unsafe {
            grove_html_hide(self.handle.as_ptr());
        };
    }

    pub fn show(
        &self,
        bounds: gpui::Bounds<gpui::Pixels>,
        clip: gpui::Bounds<gpui::Pixels>,
        window: &gpui::Window,
    ) {
        #[cfg(all(target_os = "macos", not(test)))]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let Ok(handle) = HasWindowHandle::window_handle(window) else {
                return;
            };
            let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                return;
            };
            // SAFETY: borrowed NSView is live for this Window; all coordinates are
            // logical points. AppKit handles backing scale, including display moves.
            unsafe {
                grove_html_show(
                    self.handle.as_ptr(),
                    handle.ns_view.as_ptr(),
                    f32::from(bounds.origin.x).into(),
                    f32::from(bounds.origin.y).into(),
                    f32::from(bounds.size.width).into(),
                    f32::from(bounds.size.height).into(),
                    f32::from(clip.origin.x).into(),
                    f32::from(clip.origin.y).into(),
                    f32::from(clip.size.width).into(),
                    f32::from(clip.size.height).into(),
                );
            }
        }
        #[cfg(test)]
        self.visibility_probe.set(true);
        #[cfg(any(not(target_os = "macos"), test))]
        let _ = (bounds, clip, window);
    }
}

impl Drop for NativeDocument {
    fn drop(&mut self) {
        #[cfg(all(target_os = "macos", not(test)))]
        // SAFETY: exactly balances the retained handle returned by create.
        unsafe {
            grove_html_destroy(self.handle.as_ptr());
        };
    }
}

#[cfg(all(target_os = "macos", not(test)))]
extern "C" {
    fn grove_html_create(bytes: *const u8, length: usize) -> *mut std::ffi::c_void;
    fn grove_html_status(handle: *mut std::ffi::c_void) -> i32;
    fn grove_html_hide_all();
    fn grove_html_hide_after_paint(handle: *mut std::ffi::c_void);
    fn grove_html_hide(handle: *mut std::ffi::c_void);
    fn grove_html_show(
        handle: *mut std::ffi::c_void,
        parent: *mut std::ffi::c_void,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        clip_x: f64,
        clip_y: f64,
        clip_width: f64,
        clip_height: f64,
    );
    fn grove_html_destroy(handle: *mut std::ffi::c_void);
}

/// This is invoked by app chrome, never by document JavaScript.
pub fn open_in_browser(url: String) -> impl std::future::Future<Output = Result<(), String>> {
    #[cfg(all(target_os = "macos", not(test)))]
    {
        use futures::channel::oneshot;
        type Completion = oneshot::Sender<Result<(), String>>;
        extern "C" fn complete(context: *mut std::ffi::c_void, error: *const u8, length: usize) {
            // SAFETY: native helper calls exactly once with the boxed sender;
            // error bytes are borrowed only until this callback returns.
            let sender = unsafe { Box::from_raw(context.cast::<Completion>()) };
            let result = if error.is_null() {
                Ok(())
            } else {
                let bytes = unsafe { std::slice::from_raw_parts(error, length) };
                Err(String::from_utf8_lossy(bytes).into_owned())
            };
            let _ = sender.send(result);
        }
        extern "C" {
            fn grove_html_open_browser(
                bytes: *const u8,
                length: usize,
                context: *mut std::ffi::c_void,
                completion: extern "C" fn(*mut std::ffi::c_void, *const u8, usize),
            );
        }
        let (sender, receiver) = oneshot::channel::<Result<(), String>>();
        // SAFETY: invoked on the main thread; the helper copies URL bytes before
        // returning. Callback owns the box even if the awaiting view disappears.
        unsafe {
            grove_html_open_browser(
                url.as_ptr(),
                url.len(),
                Box::into_raw(Box::new(sender)).cast(),
                complete,
            );
        }
        async move {
            receiver
                .await
                .map_err(|_| "Browser launch did not report a result".to_owned())?
        }
    }
    #[cfg(any(not(target_os = "macos"), test))]
    {
        let _ = url;
        std::future::ready(Err(
            "Explicit browser launch is unavailable on this platform".into(),
        ))
    }
}
