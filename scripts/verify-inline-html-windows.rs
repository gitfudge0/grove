//! Standalone native backend probe. The tiny GPUI adapter supplies the real host
//! HWND and layout types, while the production Windows backend is copied verbatim.
extern crate self as gpui;

#[derive(Clone, Copy, PartialOrd, PartialEq)]
pub struct Pixels(f32);
impl Pixels {
    pub const ZERO: Self = Self(0.);
}
impl From<Pixels> for f32 {
    fn from(value: Pixels) -> Self {
        value.0
    }
}
impl std::ops::Add for Pixels {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self(self.0 + other.0)
    }
}
#[derive(Clone, Copy)]
pub struct Point<T> {
    pub x: T,
    pub y: T,
}
#[derive(Clone, Copy)]
pub struct Size<T> {
    pub width: T,
    pub height: T,
}
#[derive(Clone, Copy)]
pub struct Bounds<T> {
    pub origin: Point<T>,
    pub size: Size<T>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LoadStatus {
    Loading,
    Ready,
    Failed,
}
pub struct Window(windows::Win32::Foundation::HWND);
pub struct AnyWindowHandle;
impl Window {
    // GPUI has this incompatible inherent API; backend must call the raw-handle trait explicitly.
    pub fn window_handle(&self) -> AnyWindowHandle {
        AnyWindowHandle
    }
    pub fn scale_factor(&self) -> f32 {
        1.
    }
}
impl raw_window_handle::HasWindowHandle for Window {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let native = raw_window_handle::Win32WindowHandle::new(
            std::num::NonZeroIsize::new(self.0 .0 as isize).unwrap(),
        );
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(native.into()) })
    }
}
#[path = "inline_html_windows.rs"]
mod backend;

fn pump_until(mut predicate: impl FnMut() -> bool) {
    use windows::Win32::UI::WindowsAndMessaging::*;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let mut message = MSG::default();
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if predicate() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "WebView2 probe timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn evaluate(document: &backend::Document, script: &str) -> String {
    use std::{cell::RefCell, rc::Rc};
    let result = Rc::new(RefCell::new(None));
    let callback = result.clone();
    unsafe {
        backend::probe_native(document)
            .ExecuteScript(
                &windows::core::HSTRING::from(script),
                &webview2_com::ExecuteScriptCompletedHandler::create(Box::new(
                    move |status, value| {
                        status?;
                        *callback.borrow_mut() = Some(value);
                        Ok(())
                    },
                )),
            )
            .unwrap();
    }
    pump_until(|| result.borrow().is_some());
    let value = result.borrow_mut().take().unwrap();
    value
}

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: Point {
            x: Pixels(x),
            y: Pixels(y),
        },
        size: Size {
            width: Pixels(width),
            height: Pixels(height),
        },
    }
}

fn main() {
    let _browser_launcher = backend::open_in_browser;
    use windows::{
        core::w,
        Win32::{
            Graphics::Gdi::*,
            System::Ole::OleInitialize,
            UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
        },
    };
    unsafe {
        OleInitialize(None).unwrap();
    }
    let parent = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            w!("Grove HTML native probe"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            0,
            0,
            900,
            700,
            None,
            None,
            None,
            None,
        )
        .unwrap()
    };
    let window = Window(parent);
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    server.set_nonblocking(true).unwrap();
    let port = server.local_addr().unwrap().port();
    let document = backend::Document::new(&format!(r#"<!doctype html><html><body><input id="edit"><script>
      globalThis.probe = {{guard: typeof RTCPeerConnection === 'undefined' && typeof Worker === 'undefined'}};
      const frame = document.createElement('iframe'); document.body.append(frame);
      probe.freshRealm = typeof frame.contentWindow.RTCPeerConnection === 'undefined';
      const host=document.createElement('div');document.body.append(host);
      const closed=host.attachShadow({{mode:'closed'}});const file=document.createElement('input');file.type='file';closed.append(file);
      globalThis.focusFile=()=>file.focus();
      fetch('http://127.0.0.1:{port}/must-not-load').then(() => probe.network = false, () => probe.network = true);
      window.open('https://example.invalid/');
      </script></body></html>"#)).unwrap();
    document.prepare(&window).unwrap();
    assert_eq!(
        document.status(),
        LoadStatus::Loading,
        "prepare must defer native construction until after GPUI render returns"
    );
    pump_until(|| document.status() != LoadStatus::Loading);
    assert_eq!(document.status(), LoadStatus::Ready);
    assert!(document.failure_reason().is_none());
    let viewport = bounds(20., 30., 600., 400.);
    let clip = bounds(40., 60., 300., 180.);
    document.show(viewport, clip, &window);
    pump_until(|| evaluate(&document, "Boolean(probe.network)") == "true");
    assert_eq!(
        evaluate(&document, "probe.guard"),
        "true",
        "top-level transport guards missing"
    );
    assert_eq!(
        evaluate(&document, "probe.freshRealm"),
        "true",
        "fresh about:blank realm bypasses transport guards"
    );
    assert_eq!(
        evaluate(&document, "innerWidth"),
        "600",
        "clipping must preserve viewport layout"
    );
    assert!(
        matches!(server.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "HTML made a forbidden network request"
    );
    evaluate(&document, "document.getElementById('edit').value='retained';location.href='https://example.invalid/blocked'");
    let attempt = std::time::Instant::now();
    pump_until(|| attempt.elapsed() > std::time::Duration::from_millis(50));
    assert_eq!(
        document.status(),
        LoadStatus::Ready,
        "blocked navigation retired the loaded document"
    );
    assert_eq!(
        evaluate(&document, "document.getElementById('edit').value"),
        "\"retained\""
    );
    evaluate(&document, "document.getElementById('edit').value=''");
    let child = unsafe { GetWindow(parent, GW_CHILD).unwrap() };
    let region = unsafe { CreateRectRgn(0, 0, 0, 0) };
    assert_ne!(unsafe { GetWindowRgn(child, region) }, GDI_REGION_TYPE(0));
    assert!(unsafe { PtInRegion(region, 21, 31) }.as_bool());
    assert!(
        !unsafe { PtInRegion(region, 0, 0) }.as_bool(),
        "clipped pixels remain hit-testable"
    );
    unsafe {
        let _ = DeleteObject(region.into());
    }
    backend::probe_focus(&document);
    evaluate(&document, "document.getElementById('edit').focus()");
    unsafe {
        SendMessageW(
            GetFocus(),
            WM_CHAR,
            Some(windows::Win32::Foundation::WPARAM(65)),
            Some(windows::Win32::Foundation::LPARAM(1)),
        );
    }
    pump_until(|| evaluate(&document, "document.getElementById('edit').value") == "\"A\"");
    backend::focus_host(&window);
    assert_eq!(
        unsafe { GetFocus() },
        parent,
        "composer handoff failed to restore native host focus"
    );
    backend::probe_focus(&document);
    evaluate(&document, "focusFile()");
    unsafe {
        let focus = GetFocus();
        SendMessageW(
            focus,
            WM_KEYDOWN,
            Some(windows::Win32::Foundation::WPARAM(VK_SPACE.0 as usize)),
            Some(windows::Win32::Foundation::LPARAM(1)),
        );
        SendMessageW(
            focus,
            WM_KEYUP,
            Some(windows::Win32::Foundation::WPARAM(VK_SPACE.0 as usize)),
            Some(windows::Win32::Foundation::LPARAM(1)),
        );
    }
    let activation = std::time::Instant::now();
    pump_until(|| activation.elapsed() > std::time::Duration::from_millis(100));
    assert_eq!(
        unsafe { GetLastActivePopup(parent) },
        parent,
        "file chooser escaped native interception"
    );
    let host_editor = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("EDIT"),
            w!("host editor"),
            WS_CHILD | WS_VISIBLE,
            650,
            30,
            150,
            30,
            Some(parent),
            None,
            None,
            None,
        )
        .unwrap()
    };
    unsafe {
        let _ = SetFocus(Some(host_editor));
    }
    document.hide();
    assert_eq!(
        unsafe { GetFocus() },
        host_editor,
        "hiding unrelated preview stole host focus"
    );
    document.show(viewport, clip, &window);
    document.retire_frame();
    document.show(viewport, clip, &window);
    let start = std::time::Instant::now();
    pump_until(|| start.elapsed() > std::time::Duration::from_millis(30));
    assert!(
        unsafe { IsWindowVisible(child) }.as_bool(),
        "replacement paint lost its retained surface"
    );
    document.retire_frame();
    pump_until(|| !unsafe { IsWindowVisible(child) }.as_bool());
    document.show(viewport, clip, &window);
    assert!(unsafe { IsWindowVisible(child) }.as_bool());
    backend::hide_all();
    assert!(!unsafe { IsWindowVisible(child) }.as_bool());
    drop(document);
    // The source limit is 2 MiB before sandbox prefixes. Exercise the synthetic
    // protocol beyond NavigateToString's 2 MiB ceiling as a separate retained view.
    let large = backend::Document::new(&format!(
        "<html><body>{}</body></html>",
        " ".repeat(2 * 1024 * 1024)
    ))
    .unwrap();
    large.prepare(&window).unwrap();
    pump_until(|| large.status() != LoadStatus::Loading);
    assert_eq!(large.status(), LoadStatus::Ready);
    drop(large);
    unsafe {
        DestroyWindow(parent).unwrap();
    }
    println!("PASS: native loading, 2 MiB source, fresh-realm guards, network denial, full viewport clipping/hit regions, focus and retained frame lifecycle");
}
