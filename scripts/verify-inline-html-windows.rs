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

type Wheel = (u32, usize, isize, windows::Win32::Foundation::HWND);
// High bit means DispatchMessageW is in progress; zero means PeekMessageW.
static LAST_MESSAGE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
thread_local! {
    // Native wheel messages the backend posted to the host window.
    static WHEELS: std::cell::RefCell<Vec<Wheel>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn take_wheels() -> Vec<Wheel> {
    WHEELS.with(|wheels| std::mem::take(&mut *wheels.borrow_mut()))
}

fn settle(milliseconds: u64) {
    let start = std::time::Instant::now();
    pump_until(|| start.elapsed() > std::time::Duration::from_millis(milliseconds));
}

fn pump_until(mut predicate: impl FnMut() -> bool) {
    use windows::Win32::UI::WindowsAndMessaging::*;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let mut message = MSG::default();
        unsafe {
            // Bound each batch so a continuous stream of native messages cannot
            // starve the predicate or deadline checks below.
            for _ in 0..256 {
                LAST_MESSAGE.store(0, std::sync::atomic::Ordering::Relaxed);
                if !PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    break;
                }
                if message.message == WM_MOUSEWHEEL || message.message == WM_MOUSEHWHEEL {
                    WHEELS.with(|wheels| {
                        wheels.borrow_mut().push((
                            message.message,
                            message.wParam.0,
                            message.lParam.0,
                            message.hwnd,
                        ));
                    });
                }
                let _ = TranslateMessage(&message);
                LAST_MESSAGE.store(
                    0x8000_0000 | message.message,
                    std::sync::atomic::Ordering::Relaxed,
                );
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
    // The message pump's deadline cannot fire while a native COM/Win32 call
    // is blocked (for example in a modal loop). Bound the whole probe too.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(120));
        eprintln!(
            "WebView2 probe exceeded its 120-second process deadline; last message state: {:#x}",
            LAST_MESSAGE.load(std::sync::atomic::Ordering::Relaxed)
        );
        std::process::exit(1);
    });
    eprintln!("WebView2 probe: initializing host and document");
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
      globalThis.probe = {{guard: typeof RTCPeerConnection === 'undefined' && typeof Worker === 'undefined', clicks: 0, frameClicks: 0}};
      const frame = document.createElement('iframe'); document.body.append(frame);
      probe.freshRealm = typeof frame.contentWindow.RTCPeerConnection === 'undefined';
      const frameFile=frame.contentDocument.createElement('input');frameFile.type='file';frame.contentDocument.body.append(frameFile);
      frameFile.addEventListener('click',()=>probe.frameClicks++);
      globalThis.focusFrameFile=()=>{{frame.contentWindow.focus();frameFile.focus()}};
      const host=document.createElement('div');document.body.append(host);
      const closed=host.attachShadow({{mode:'closed'}});const file=document.createElement('input');file.type='file';closed.append(file);
      file.addEventListener('click',()=>probe.clicks++);
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
    eprintln!("WebView2 probe: checking security and clipping");
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
    // Overlay exclusion removes its pixels and lets the host receive dismissal
    // clicks; clearing it restores both the clip region and native input.
    document.exclude(viewport, clip, Some(bounds(80., 90., 40., 30.)), &window);
    assert_ne!(unsafe { GetWindowRgn(child, region) }, GDI_REGION_TYPE(0));
    assert!(unsafe { PtInRegion(region, 21, 31) }.as_bool());
    assert!(
        !unsafe { PtInRegion(region, 70, 70) }.as_bool(),
        "overlay exclusion left covered pixels in the native region"
    );
    assert!(!unsafe { IsWindowEnabled(child) }.as_bool());
    document.exclude(viewport, clip, None, &window);
    assert_ne!(unsafe { GetWindowRgn(child, region) }, GDI_REGION_TYPE(0));
    assert!(unsafe { PtInRegion(region, 70, 70) }.as_bool());
    assert!(!unsafe { PtInRegion(region, 0, 0) }.as_bool());
    assert!(unsafe { IsWindowEnabled(child) }.as_bool());
    unsafe {
        let _ = DeleteObject(region.into());
    }
    backend::probe_focus(&document);
    eprintln!("WebView2 probe: checking keyboard focus and file chooser");
    evaluate(&document, "document.getElementById('edit').focus()");
    eprintln!("WebView2 probe: sending editor character");
    unsafe {
        SendMessageW(
            GetFocus(),
            WM_CHAR,
            Some(windows::Win32::Foundation::WPARAM(65)),
            Some(windows::Win32::Foundation::LPARAM(1)),
        );
    }
    pump_until(|| evaluate(&document, "document.getElementById('edit').value") == "\"A\"");
    eprintln!("WebView2 probe: returning host focus");
    backend::focus_host(&window);
    assert_eq!(
        unsafe { GetFocus() },
        parent,
        "composer handoff failed to restore native host focus"
    );
    backend::probe_focus(&document);
    evaluate(&document, "focusFile()");
    // If interception fails, a native chooser can enter a modal loop inside
    // DispatchMessageW, preventing the assertion below from running. Watch
    // this test's own host for an owned popup and close it so the test fails
    // with an actionable assertion instead of hanging.
    let escaped_popup = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_watcher = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watcher_popup = escaped_popup.clone();
    let watcher_stop = stop_watcher.clone();
    let parent_address = parent.0 as usize;
    let watcher = std::thread::spawn(move || {
        let host = windows::Win32::Foundation::HWND(parent_address as *mut _);
        while !watcher_stop.load(std::sync::atomic::Ordering::Relaxed) {
            let popup = unsafe { GetLastActivePopup(host) };
            if popup != host && unsafe { IsWindowVisible(popup) }.as_bool() {
                watcher_popup.store(true, std::sync::atomic::Ordering::Relaxed);
                unsafe {
                    let mut class = [0u16; 256];
                    let mut title = [0u16; 256];
                    let class_len = GetClassNameW(popup, &mut class).max(0) as usize;
                    let title_len = GetWindowTextW(popup, &mut title).max(0) as usize;
                    eprintln!(
                        "WebView2 probe: closing escaped host popup class={:?} title={:?}",
                        String::from_utf16_lossy(&class[..class_len]),
                        String::from_utf16_lossy(&title[..title_len])
                    );
                    let _ = PostMessageW(
                        Some(popup),
                        WM_CLOSE,
                        Default::default(),
                        Default::default(),
                    );
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
    eprintln!("WebView2 probe: activating file input");
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
    eprintln!("WebView2 probe: pumping file activation");
    pump_until(|| activation.elapsed() > std::time::Duration::from_millis(100));
    settle(1500);
    assert_eq!(
        evaluate(&document, "probe.clicks"),
        "1",
        "closed-shadow file input was not activated"
    );
    assert!(
        !escaped_popup.load(std::sync::atomic::Ordering::Relaxed),
        "closed-shadow file chooser escaped native interception"
    );
    backend::probe_focus(&document);
    evaluate(&document, "focusFrameFile()");
    eprintln!("WebView2 probe: activating same-origin iframe file input");
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
    settle(1500);
    assert_eq!(
        evaluate(&document, "probe.frameClicks"),
        "1",
        "iframe file input was not activated"
    );
    assert!(
        !escaped_popup.load(std::sync::atomic::Ordering::Relaxed),
        "iframe file chooser escaped native interception"
    );
    assert_eq!(
        unsafe { GetLastActivePopup(parent) },
        parent,
        "file chooser escaped native interception"
    );
    eprintln!("WebView2 probe: creating host editor");
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
    eprintln!("WebView2 probe: checking retained surface lifecycle");
    assert_eq!(
        unsafe { GetFocus() },
        host_editor,
        "hiding unrelated preview stole host focus"
    );
    document.show(viewport, clip, &window);
    document.retire_frame();
    eprintln!("WebView2 probe: replacing retained surface");
    document.show(viewport, clip, &window);
    eprintln!("WebView2 probe: pumping replacement paint");
    let start = std::time::Instant::now();
    pump_until(|| start.elapsed() > std::time::Duration::from_millis(30));
    // WebView2 may open the chooser well after the triggering key event.
    // Keep watching long enough to observe the delayed native dialog.
    settle(1500);
    stop_watcher.store(true, std::sync::atomic::Ordering::Relaxed);
    watcher.join().unwrap();
    assert!(
        !escaped_popup.load(std::sync::atomic::Ordering::Relaxed),
        "file chooser escaped native interception"
    );
    eprintln!("WebView2 probe: checking replacement visibility");
    assert!(
        unsafe { IsWindowVisible(child) }.as_bool(),
        "replacement paint lost its retained surface"
    );
    document.retire_frame();
    eprintln!("WebView2 probe: retiring retained surface");
    pump_until(|| !unsafe { IsWindowVisible(child) }.as_bool());
    document.show(viewport, clip, &window);
    assert!(unsafe { IsWindowVisible(child) }.as_bool());
    backend::hide_all();
    assert!(!unsafe { IsWindowVisible(child) }.as_bool());
    eprintln!("WebView2 probe: dropping first document");
    drop(document);
    // The source limit is 2 MiB before sandbox prefixes. Exercise the synthetic
    // protocol beyond NavigateToString's 2 MiB ceiling as a separate retained view.
    eprintln!("WebView2 probe: constructing large document");
    let large = backend::Document::new(&format!(
        "<html><body>{}</body></html>",
        " ".repeat(2 * 1024 * 1024)
    ))
    .unwrap();
    large.prepare(&window).unwrap();
    eprintln!("WebView2 probe: loading large document");
    pump_until(|| large.status() != LoadStatus::Loading);
    assert_eq!(large.status(), LoadStatus::Ready);
    eprintln!("WebView2 probe: dropping large document");
    drop(large);
    eprintln!("WebView2 probe: checking integrated reports");
    verify_integrated_report(&window, parent);
    unsafe {
        DestroyWindow(parent).unwrap();
    }
    println!("PASS: native loading, 2 MiB source, fresh-realm guards, network denial, full viewport clipping/hit regions, focus and retained frame lifecycle, report measurement and wheel bridge");
}

fn screen(parent: windows::Win32::Foundation::HWND, x: i32, y: i32) -> isize {
    let mut point = windows::Win32::Foundation::POINT { x, y };
    assert!(unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(parent, &mut point) }.as_bool());
    ((i32::from(point.y as i16) << 16) | i32::from(point.x as u16)) as isize
}

fn wheel_lines(
    action: windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_ACTION,
) -> f64 {
    let mut value = 3u32;
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
            action,
            0,
            Some((&raw mut value).cast()),
            Default::default(),
        )
        .unwrap();
    }
    f64::from(if value == 0 || value == u32::MAX {
        3
    } else {
        value
    })
}

fn verify_integrated_report(window: &Window, parent: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::WindowsAndMessaging::*;
    let report = backend::Document::new(
        "<!doctype html><html><head></head><body><!-- grove:integrated-report --><p>report</p></body></html>",
    )
    .unwrap();
    assert_eq!(
        report.report_height(),
        0.,
        "unloaded report must not measure"
    );
    report.report_theme("document.body.dataset.early='yes'", true);
    report.prepare(window).unwrap();
    pump_until(|| report.status() != LoadStatus::Loading);
    assert_eq!(report.status(), LoadStatus::Ready);
    let viewport = bounds(20., 30., 600., 400.);
    report.show(viewport, viewport, window);

    // Body geometry is measured independently of the viewport, including shrink.
    let measure = |script: &str| {
        report.report_theme(
            &format!("document.documentElement.style.overflow='hidden';{script}"),
            true,
        );
        settle(200);
        report.report_height();
        settle(200);
        report.report_height()
    };
    let initial = measure("document.body.style.cssText='margin:0;height:420px'");
    let shorter = measure("document.body.style.height='180px'");
    let taller = measure("document.body.style.height='1200px'");
    assert_eq!(
        (initial, shorter, taller),
        (420., 180., 1200.),
        "report measurement"
    );
    assert_eq!(
        evaluate(&report, "typeof window.ipc"),
        "\"undefined\"",
        "report exposes a host bridge"
    );

    // A trusted-shape wheel over the integrated report becomes one host notch.
    let lines = wheel_lines(SPI_GETWHEELSCROLLLINES);
    let chars = wheel_lines(SPI_GETWHEELSCROLLCHARS);
    let notch = |delta: f64, lines: f64| (delta / (100. / 3. * lines) * 120.).round() as i16;
    take_wheels();
    assert_eq!(
        evaluate(
            &report,
            "dispatchEvent(new WheelEvent('wheel',{deltaX:0,deltaY:100,clientX:10,clientY:20,cancelable:true}))"
        ),
        "false",
        "integrated report must cancel document scrolling"
    );
    pump_until(|| WHEELS.with(|wheels| !wheels.borrow().is_empty()));
    settle(100);
    assert_eq!(
        take_wheels(),
        vec![(
            WM_MOUSEWHEEL,
            (usize::from(notch(-100., lines) as u16) << 16),
            screen(parent, 30, 50),
            parent
        )],
        "vertical wheel forwarding"
    );
    evaluate(
        &report,
        "dispatchEvent(new WheelEvent('wheel',{deltaX:100,deltaY:0,clientX:10,clientY:20,cancelable:true}))",
    );
    pump_until(|| WHEELS.with(|wheels| !wheels.borrow().is_empty()));
    settle(100);
    assert_eq!(
        take_wheels(),
        vec![(
            WM_MOUSEHWHEEL,
            (usize::from(notch(100., chars) as u16) << 16),
            screen(parent, 30, 50),
            parent
        )],
        "horizontal wheel forwarding"
    );

    // Author content can reach the channel directly, but only the exact shape
    // scrolls the conversation, with bounded delta and coordinates.
    evaluate(
        &report,
        r#"for (const message of [
            '{"grove_wheel":[0,100,0,1,1]}',
            {grove_wheel:[0,100,0,1]},
            {grove_wheel:[0,100,0,1,1,1]},
            {grove_wheel:[0,100,0,1,1],extra:1},
            {grove_wheel:[0,100,3,1,1]},
            {grove_wheel:[0,100,0.5,1,1]},
            {grove_wheel:[0,NaN,0,1,1]},
            {grove_wheel:['0',100,0,1,1]},
            {grove_wheel:{0:0,1:100,2:0,3:1,4:1}},
            {grove_wheel:[0,100,0,1,1],pad:'x'.repeat(512)},
            {other:[0,100,0,1,1]},
            [0,100,0,1,1],
            null,
        ]) chrome.webview.postMessage(message);
        chrome.webview.postMessage({grove_wheel:[0,1e9,0,-1e9,1e9]});"#,
    );
    pump_until(|| WHEELS.with(|wheels| !wheels.borrow().is_empty()));
    settle(200);
    assert_eq!(
        take_wheels(),
        vec![(
            WM_MOUSEWHEEL,
            (usize::from((-1200i16) as u16) << 16),
            screen(parent, 20, 30 + 399),
            parent
        )],
        "malformed or spoofed web messages must be ignored; valid ones clamped"
    );

    // Expanded documents scroll themselves; hidden ones never forward.
    report.report_theme("", false);
    assert_eq!(
        evaluate(
            &report,
            "dispatchEvent(new WheelEvent('wheel',{deltaY:100,cancelable:true}))"
        ),
        "true",
        "expanded report must keep document scrolling"
    );
    evaluate(
        &report,
        "chrome.webview.postMessage({grove_wheel:[0,100,0,1,1]})",
    );
    settle(300);
    assert!(
        take_wheels().is_empty(),
        "expanded report forwarded wheel input"
    );
    report.report_theme("", true);
    report.hide();
    evaluate(
        &report,
        "chrome.webview.postMessage({grove_wheel:[0,100,0,1,1]})",
    );
    settle(300);
    assert!(
        take_wheels().is_empty(),
        "hidden report forwarded wheel input"
    );
    drop(report);
}
