//! GTK and WebKitGTK live exclusively on GPUI's foreground thread.
#![allow(unsafe_code)]
use super::LoadStatus;
use gpui::{Bounds, Pixels, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{cell::Cell, ffi::c_void, ptr::NonNull};

pub(super) struct Document {
    source: String,
    handle: Cell<Option<NonNull<c_void>>>,
}
impl Document {
    pub(super) fn new(source: &str) -> Option<Self> {
        Some(Self {
            source: source.to_owned(),
            handle: Cell::new(None),
        })
    }
    pub(super) fn prepare(&self, window: &Window) -> Result<(), String> {
        if self.handle.get().is_some() {
            return Ok(());
        }
        let parent =
            parent(window).ok_or("Inline HTML requires an X11 window (XWayland is supported)")?;
        // SAFETY: foreground-thread-only owner; native code copies source synchronously.
        let pointer =
            unsafe { grove_html_linux_create(self.source.as_ptr(), self.source.len(), parent) };
        let handle = NonNull::new(pointer).ok_or("Could not initialize WebKitGTK inline HTML")?;
        self.handle.set(Some(handle));
        Ok(())
    }
    pub(super) fn status(&self) -> LoadStatus {
        let Some(handle) = self.handle.get() else {
            return LoadStatus::Loading;
        };
        // SAFETY: this document retains the native owner until Drop.
        match unsafe { grove_html_linux_status(handle.as_ptr()) } {
            0 => LoadStatus::Loading,
            1 => LoadStatus::Ready,
            _ => LoadStatus::Failed,
        }
    }
    pub(super) fn exclude(
        &self,
        _: Bounds<Pixels>,
        _: Bounds<Pixels>,
        exclusion: Option<Bounds<Pixels>>,
        window: &Window,
    ) {
        let Some(handle) = self.handle.get() else {
            return;
        };
        let rect = exclusion.unwrap_or_default();
        let scale = f64::from(window.scale_factor());
        // SAFETY: foreground-thread native owner; X11 coordinates are physical pixels.
        unsafe {
            grove_html_linux_exclude(
                handle.as_ptr(),
                f64::from(f32::from(rect.origin.x)) * scale,
                f64::from(f32::from(rect.origin.y)) * scale,
                f64::from(f32::from(rect.size.width)) * scale,
                f64::from(f32::from(rect.size.height)) * scale,
            );
        }
    }
    pub(super) fn show(&self, bounds: Bounds<Pixels>, clip: Bounds<Pixels>, window: &Window) {
        let Some(handle) = self.handle.get() else {
            return;
        };
        let Some(parent) = parent(window) else {
            self.hide();
            return;
        };
        let scale = f64::from(window.scale_factor());
        // SAFETY: coordinates refer to this live host window; X11 uses physical pixels.
        unsafe {
            grove_html_linux_show(
                handle.as_ptr(),
                parent,
                f64::from(f32::from(bounds.origin.x)) * scale,
                f64::from(f32::from(bounds.origin.y)) * scale,
                f64::from(f32::from(bounds.size.width)) * scale,
                f64::from(f32::from(bounds.size.height)) * scale,
                f64::from(f32::from(clip.origin.x)) * scale,
                f64::from(f32::from(clip.origin.y)) * scale,
                f64::from(f32::from(clip.size.width)) * scale,
                f64::from(f32::from(clip.size.height)) * scale,
                scale,
            );
        }
    }
    pub(super) fn hide(&self) {
        if let Some(handle) = self.handle.get() {
            // SAFETY: live foreground-thread-only owner.
            unsafe { grove_html_linux_hide(handle.as_ptr()) };
        }
    }
    pub(super) fn report_height(&self) -> f64 {
        let Some(handle) = self.handle.get() else {
            return 0.;
        };
        // SAFETY: live foreground-thread-only owner; only a bounded scalar returns.
        let height = unsafe { grove_html_linux_report_height(handle.as_ptr()) };
        if height.is_finite() && height > 0. {
            height
        } else {
            0.
        }
    }
    pub(super) fn report_theme(&self, script: &str, integrated: bool) {
        if let Some(handle) = self.handle.get() {
            // SAFETY: live owner; native code copies the UTF-8 bytes synchronously.
            unsafe {
                grove_html_linux_report_theme(
                    handle.as_ptr(),
                    script.as_ptr(),
                    script.len(),
                    integrated,
                );
            };
        }
    }
    pub(super) fn retire_frame(&self) {
        if let Some(handle) = self.handle.get() {
            // SAFETY: callback retains owner and checks its paint generation.
            unsafe { grove_html_linux_retire(handle.as_ptr()) };
        }
    }
}
impl Drop for Document {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // SAFETY: balances exactly one native create.
            unsafe { grove_html_linux_destroy(handle.as_ptr()) };
        }
    }
}
fn parent(window: &Window) -> Option<std::ffi::c_ulong> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Xcb(handle) => Some(handle.window.get().into()),
        RawWindowHandle::Xlib(handle) => Some(handle.window),
        _ => None,
    }
}
pub(super) fn hide_all() {
    // SAFETY: called only on the foreground thread, including before GTK init.
    unsafe { grove_html_linux_hide_all() };
}
pub(super) fn focus_host(window: &Window) {
    if let Some(parent) = parent(window) {
        // SAFETY: parent belongs to the live GPUI window.
        unsafe { grove_html_linux_focus_host(parent) };
    }
}
pub(super) fn pump(cx: &mut gpui::App) {
    // SAFETY: only GPUI's foreground executor pumps GTK's default context.
    unsafe { grove_html_linux_pump() };
    // GTK callbacks only queue; GPUI dispatch happens here, outside GTK.
    let mut pending: Option<Wheel> = None;
    let mut next = Wheel::default();
    // SAFETY: foreground-thread queue; `next` is a valid out-pointer.
    while unsafe { grove_html_linux_next_wheel(&raw mut next) } {
        match &mut pending {
            Some(wheel) if wheel.parent == next.parent && wheel.modifiers == next.modifiers => {
                (wheel.x, wheel.y) = (next.x, next.y);
                wheel.dx += next.dx;
                wheel.dy += next.dy;
            }
            _ => {
                if let Some(wheel) = pending.replace(next) {
                    dispatch_wheel(wheel, cx);
                }
            }
        }
    }
    if let Some(wheel) = pending {
        dispatch_wheel(wheel, cx);
    }
}
/// Mirrors gpui_linux X11: a notch is `SCROLL_LINES` lines, smooth valuators
/// scale by `SCROLL_LINES / increment` with +y meaning up, and Shift turns
/// vertical scrolling horizontal. GDK reports one notch as 1.0 with +y down.
fn dispatch_wheel(wheel: Wheel, cx: &mut gpui::App) {
    const SCROLL_LINES: f64 = 3.0;
    let modifiers = gpui::Modifiers {
        control: wheel.modifiers & 1 != 0,
        alt: wheel.modifiers & 2 != 0,
        shift: wheel.modifiers & 4 != 0,
        platform: wheel.modifiers & 8 != 0,
        function: false,
    };
    let (dx, dy) = (-wheel.dx * SCROLL_LINES, -wheel.dy * SCROLL_LINES);
    let delta = if modifiers.shift {
        gpui::point(dy as f32, 0.)
    } else {
        gpui::point(dx as f32, dy as f32)
    };
    for handle in cx.windows() {
        let dispatched = handle.update(cx, |_, window, cx| {
            if parent(window) != Some(wheel.parent) {
                return false;
            }
            let scale = f64::from(window.scale_factor());
            let event = gpui::ScrollWheelEvent {
                position: gpui::point(
                    gpui::px((wheel.x / scale) as f32),
                    gpui::px((wheel.y / scale) as f32),
                ),
                delta: gpui::ScrollDelta::Lines(delta),
                modifiers,
                touch_phase: gpui::TouchPhase::default(),
            };
            let _ = window.dispatch_event(gpui::PlatformInput::ScrollWheel(event), cx);
            true
        });
        if dispatched.unwrap_or(false) {
            return;
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Wheel {
    parent: std::ffi::c_ulong,
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    modifiers: u32,
}
extern "C" {
    fn grove_html_linux_create(
        bytes: *const u8,
        length: usize,
        parent: std::ffi::c_ulong,
    ) -> *mut c_void;
    fn grove_html_linux_status(handle: *mut c_void) -> i32;
    fn grove_html_linux_report_height(handle: *mut c_void) -> f64;
    fn grove_html_linux_report_theme(
        handle: *mut c_void,
        bytes: *const u8,
        length: usize,
        integrated: bool,
    );
    fn grove_html_linux_next_wheel(wheel: *mut Wheel) -> bool;
    fn grove_html_linux_exclude(handle: *mut c_void, x: f64, y: f64, width: f64, height: f64);
    fn grove_html_linux_show(
        handle: *mut c_void,
        parent: std::ffi::c_ulong,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        clip_x: f64,
        clip_y: f64,
        clip_width: f64,
        clip_height: f64,
        scale: f64,
    );
    fn grove_html_linux_hide(handle: *mut c_void);
    fn grove_html_linux_retire(handle: *mut c_void);
    fn grove_html_linux_destroy(handle: *mut c_void);
    fn grove_html_linux_hide_all();
    fn grove_html_linux_focus_host(parent: std::ffi::c_ulong);
    fn grove_html_linux_pump();
}
