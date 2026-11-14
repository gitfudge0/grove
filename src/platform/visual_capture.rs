//! Foreground-only, isolated native screenshot resources.
#![allow(unsafe_code)]
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod ffi {
    use std::{
        ffi::{c_void, CStr},
        ptr::NonNull,
    };
    pub struct Capture {
        handle: NonNull<c_void>,
    }
    impl Capture {
        pub fn new(source: &str, width: u32, height: u32) -> Result<Self, String> {
            // Native owner copies source and creates a nonactivating offscreen viewport.
            NonNull::new(unsafe { create(source.as_ptr(), source.len(), width, height) })
                .map(|handle| Self { handle })
                .ok_or("Native screenshot backend could not initialize".into())
        }
        pub fn poll(&self) -> Option<Result<(Vec<u8>, serde_json::Value), String>> {
            match unsafe { poll(self.handle.as_ptr()) } {
                n if n < 0 => Some(Err("Native screenshot capture failed".into())),
                0 => None,
                n => {
                    if n > 16 * 1024 * 1024 {
                        return Some(Err("Native screenshot exceeds 16 MiB".into()));
                    }
                    let mut png = vec![0; n as usize];
                    unsafe {
                        copy(self.handle.as_ptr(), png.as_mut_ptr(), png.len());
                    }
                    let metadata = unsafe { metadata(self.handle.as_ptr()) };
                    let metadata = if metadata.is_null() {
                        serde_json::json!({})
                    } else {
                        serde_json::from_slice(unsafe { CStr::from_ptr(metadata) }.to_bytes())
                            .unwrap_or_default()
                    };
                    Some(Ok((png, metadata)))
                }
            }
        }
    }
    impl Drop for Capture {
        fn drop(&mut self) {
            unsafe {
                destroy(self.handle.as_ptr());
            }
        }
    }
    unsafe extern "C" {
        #[cfg_attr(target_os = "linux", link_name = "grove_html_linux_capture_create")]
        #[cfg_attr(target_os = "macos", link_name = "grove_html_capture_create")]
        fn create(bytes: *const u8, length: usize, width: u32, height: u32) -> *mut c_void;
        #[cfg_attr(target_os = "linux", link_name = "grove_html_linux_capture_poll")]
        #[cfg_attr(target_os = "macos", link_name = "grove_html_capture_poll")]
        fn poll(handle: *mut c_void) -> i32;
        #[cfg_attr(target_os = "linux", link_name = "grove_html_linux_capture_copy")]
        #[cfg_attr(target_os = "macos", link_name = "grove_html_capture_copy")]
        fn copy(handle: *mut c_void, out: *mut u8, length: usize);
        #[cfg_attr(target_os = "linux", link_name = "grove_html_linux_capture_metadata")]
        #[cfg_attr(target_os = "macos", link_name = "grove_html_capture_metadata")]
        fn metadata(handle: *mut c_void) -> *const std::ffi::c_char;
        #[cfg_attr(target_os = "linux", link_name = "grove_html_linux_destroy")]
        #[cfg_attr(target_os = "macos", link_name = "grove_html_destroy")]
        fn destroy(handle: *mut c_void);
    }
}
#[cfg(target_os = "windows")]
pub use super::inline_html::backend::Capture;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use ffi::Capture;

/// Dispatch WebKitGTK on its foreground thread, also used by the isolated native fixture.
#[cfg(all(test, target_os = "linux"))]
pub fn pump_native() {
    #[cfg(target_os = "linux")]
    unsafe {
        unsafe extern "C" {
            fn grove_html_linux_pump();
        }
        grove_html_linux_pump();
    }
}
