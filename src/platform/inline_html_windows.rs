//! STA-thread WebView2 documents. Author bytes load only after native policy is installed.
#![allow(unsafe_code)]

use super::LoadStatus;
use gpui::{Bounds, Pixels, Window};
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    sync::Arc,
};
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, ExecuteScriptCompletedHandler,
    Microsoft::Web::WebView2::Win32::*, NavigationCompletedEventHandler,
    NavigationStartingEventHandler, PermissionRequestedEventHandler, ProcessFailedEventHandler,
    WebMessageReceivedEventHandler, WebResourceRequestedEventHandler,
};
use windows::{
    core::{w, Interface, BOOL},
    Win32::{
        Foundation::{LPARAM, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            ClientToScreen, CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, RGN_DIFF,
        },
        UI::{
            Input::KeyboardAndMouse::{
                EnableWindow, GetFocus, GetKeyState, VIRTUAL_KEY, VK_CONTROL, VK_LBUTTON,
                VK_MBUTTON, VK_RBUTTON, VK_SHIFT,
            },
            WindowsAndMessaging::{
                GetAncestor, GetClientRect, IsChild, IsWindowVisible, KillTimer, PostMessageW,
                SetTimer, SystemParametersInfoW, GA_PARENT, GA_ROOT, SPI_GETWHEELSCROLLCHARS,
                SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WHEEL_DELTA,
                WM_MOUSEHWHEEL, WM_MOUSEWHEEL,
            },
        },
    },
};
use wry::{WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows};

const DOCUMENT_URL: &str = "https://grove-html.document/";
const GUARD: &str = "for(const name of ['RTCPeerConnection','webkitRTCPeerConnection','RTCIceGatherer','WebTransport','Worker','SharedWorker','ServiceWorker','Notification','showOpenFilePicker','showSaveFilePicker','showDirectoryPicker']){try{Object.defineProperty(globalThis,name,{value:undefined,writable:false,configurable:false});}catch{}}";
// Integrated reports share the conversation's scroll, but WebView2 never returns
// wheel input to the host. Only while the host enables it, wheel events become a
// fixed-shape message; the host validates it and synthesizes a native wheel.
const WHEEL_BRIDGE: &str = "(()=>{if(window.top!==window)return;const v=globalThis.chrome&&chrome.webview;if(!v)return;const post=v.postMessage.bind(v);let on=false;Object.defineProperty(globalThis,'__groveReportWheel',{value:o=>{on=o===true}});addEventListener('wheel',e=>{if(!on)return;e.preventDefault();post({grove_wheel:[e.deltaX,e.deltaY,e.deltaMode,e.clientX,e.clientY]})},{capture:true,passive:false})})()";
const MEASURE: &str = "Math.ceil(document.body ? document.body.getBoundingClientRect().height : 0)";
// Chromium's kScrollbarPixelsPerLine: one notch at three lines is 100 CSS px.
const PIXELS_PER_LINE: f64 = 100. / 3.;
const WHEEL_PAGESCROLL: u32 = u32::MAX;
const MAX_WHEEL_NOTCHES: f64 = 10.;
const MAX_WHEEL_MESSAGE: usize = 256;
const CSP: &str = "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval'; style-src 'unsafe-inline'; img-src data:; font-src data:; media-src data:; connect-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'";

thread_local! {
    static DOCUMENTS: RefCell<Vec<Weak<State>>> = const { RefCell::new(Vec::new()) };
    static INITIALIZATIONS: RefCell<std::collections::HashMap<usize, (Weak<State>, HostWindow)>> = RefCell::new(std::collections::HashMap::new());
    static RETIREMENTS: RefCell<std::collections::HashMap<usize, (Weak<State>, u64)>> = RefCell::new(std::collections::HashMap::new());
}

struct State {
    source: RefCell<Option<String>>,
    view: RefCell<Option<WebView>>,
    status: Cell<LoadStatus>,
    generation: Cell<u64>,
    navigation_id: Cell<Option<u64>>,
    initializing: Cell<bool>,
    integrated: Cell<bool>,
    report_height: Cell<f64>,
    measuring: Cell<bool>,
    error: RefCell<Option<String>>,
    // Drop the webview/controller before removing the isolated profile directory.
    profile: RefCell<Option<tempfile::TempDir>>,
}

pub struct Document {
    state: Rc<State>,
}

impl Document {
    pub fn new(source: &str) -> Option<Self> {
        let state = Rc::new(State {
            source: RefCell::new(Some(source.to_owned())),
            view: RefCell::new(None),
            status: Cell::new(LoadStatus::Loading),
            generation: Cell::new(0),
            navigation_id: Cell::new(None),
            initializing: Cell::new(false),
            integrated: Cell::new(false),
            report_height: Cell::new(0.),
            measuring: Cell::new(false),
            error: RefCell::new(None),
            profile: RefCell::new(None),
        });
        DOCUMENTS.with(|documents| documents.borrow_mut().push(Rc::downgrade(&state)));
        Some(Self { state })
    }

    pub fn prepare(&self, window: &Window) -> Result<(), String> {
        if let Some(error) = self.state.error.borrow().as_ref() {
            return Err(error.clone());
        }
        if self.state.view.borrow().is_some() {
            return Ok(());
        }
        if self.state.initializing.get() {
            return Ok(());
        }
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let handle = HasWindowHandle::window_handle(window).map_err(|error| error.to_string())?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err("HTML preview requires a native Windows host".into());
        };
        let host = HostWindow(windows::Win32::Foundation::HWND(handle.hwnd.get() as *mut _));
        // Wry pumps arbitrary Win32 messages while creating its environment and
        // controller. Start it after GPUI releases the render's App/Window
        // borrows, otherwise that nested dispatch could reenter borrowed GPUI.
        let timer = unsafe { SetTimer(None, 0, 1, Some(initialize_after_render)) };
        if timer == 0 {
            return Err("Couldn’t schedule the native HTML preview".into());
        }
        self.state.initializing.set(true);
        INITIALIZATIONS.with(|initializations| {
            initializations
                .borrow_mut()
                .insert(timer, (Rc::downgrade(&self.state), host));
        });
        Ok(())
    }

    fn create(
        owner: &Rc<State>,
        window: &impl raw_window_handle::HasWindowHandle,
    ) -> Result<WebView, Box<dyn std::error::Error>> {
        let profile = tempfile::tempdir()?;
        let mut context = wry::WebContext::new(Some(profile.path().to_owned()));
        let source = Arc::new(
            owner
                .source
                .borrow()
                .as_ref()
                .cloned()
                .ok_or("HTML source unavailable")?,
        );
        let initial_navigation = Rc::new(Cell::new(true));
        let navigation = initial_navigation.clone();
        // No authored URL/HTML at construction: all native filters below must succeed first.
        let view = WebViewBuilder::new_with_web_context(&mut context)
            .with_incognito(true)
            .with_visible(false)
            .with_focused(false)
            .with_devtools(false)
            .with_clipboard(false)
            .with_autoplay(false)
            .with_browser_accelerator_keys(false)
            .with_default_context_menus(false)
            .with_https_scheme(true)
            .with_initialization_script_for_main_only(GUARD, false)
            .with_initialization_script_for_main_only(WHEEL_BRIDGE, true)
            .with_permission_handler(|_| wry::PermissionResponse::Deny)
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_navigation_handler(move |url| {
                if url == DOCUMENT_URL && navigation.replace(false) {
                    return true;
                }
                // Same-document anchors preserve offline document interaction only.
                !navigation.get() && url.starts_with(&format!("{DOCUMENT_URL}#"))
            })
            .with_custom_protocol("grove-html".into(), move |_, request| {
                let allowed = request.uri() == "grove-html://document/"
                    && request.method() == wry::http::Method::GET;
                let mut response = wry::http::Response::new(if allowed {
                    Cow::Owned(source.as_bytes().to_vec())
                } else {
                    Cow::Borrowed(&[][..])
                });
                *response.status_mut() = if allowed {
                    wry::http::StatusCode::OK
                } else {
                    wry::http::StatusCode::FORBIDDEN
                };
                response.headers_mut().insert(
                    "Content-Type",
                    wry::http::HeaderValue::from_static("text/html; charset=utf-8"),
                );
                response.headers_mut().insert(
                    "Content-Security-Policy",
                    wry::http::HeaderValue::from_static(CSP),
                );
                response.headers_mut().insert(
                    "Cache-Control",
                    wry::http::HeaderValue::from_static("no-store"),
                );
                response
            })
            .build_as_child(window)?;
        // Wry older-runtime fallback cannot silently turn InPrivate into a disk profile.
        let _ = view.environment().cast::<ICoreWebView2Environment10>()?;
        let native = view.webview();
        // SAFETY: GPUI and WebView2 share this UI thread's COM STA; objects remain
        // retained by `view`, and every callback holds only a weak document state.
        unsafe {
            let settings = native.Settings()?;
            // The sole web message is the integrated-report wheel bridge;
            // anything else is dropped without a reply. No host objects.
            settings.SetIsWebMessageEnabled(true)?;
            settings.SetAreHostObjectsAllowed(false)?;
            settings.SetAreDefaultScriptDialogsEnabled(false)?;
            settings.SetAreDevToolsEnabled(false)?;
            settings.SetAreDefaultContextMenusEnabled(false)?;
            settings.SetIsStatusBarEnabled(false)?;
            // Disable arbitrary file drops into the document, including navigation.
            view.controller()
                .cast::<ICoreWebView2Controller4>()?
                .SetAllowExternalDrop(false)?;
            let mut token = 0;
            native.add_PermissionRequested(
                &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                    if let Some(args) = args {
                        args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            // Native interception supplements CSP, covering author attempts to remove
            // policy or load network/file/custom URLs using alternate DOM APIs.
            native
                .cast::<ICoreWebView2_22>()?
                .AddWebResourceRequestedFilterWithRequestSourceKinds(
                    w!("*"),
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
                )?;
            let blocked = view.environment().CreateWebResourceResponse(
                None,
                403,
                w!("Blocked"),
                w!("Cache-Control: no-store\r\n"),
            )?;
            native.add_WebResourceRequested(
                &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    // Malformed/failed URI access is denied as well. Prepare the
                    // response before author loading, rather than potentially
                    // failing its allocation inside the request callback.
                    let allowed = (|| -> windows::core::Result<bool> {
                        let request = args.Request()?;
                        let mut pointer = windows::core::PWSTR::null();
                        request.Uri(&mut pointer)?;
                        let url = pointer.to_string();
                        windows::Win32::System::Com::CoTaskMemFree(Some(pointer.0.cast()));
                        Ok(url.is_ok_and(|url| url == DOCUMENT_URL || url.starts_with("data:")))
                    })()
                    .unwrap_or(false);
                    if !allowed {
                        args.SetResponse(&blocked)?;
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let state = Rc::downgrade(owner);
            native.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                    if let (Some(state), Some(args)) = (state.upgrade(), args) {
                        if state.navigation_id.get().is_none() {
                            let mut pointer = windows::core::PWSTR::null();
                            args.Uri(&mut pointer)?;
                            let url = pointer.to_string();
                            windows::Win32::System::Com::CoTaskMemFree(Some(pointer.0.cast()));
                            if url.is_ok_and(|url| url == DOCUMENT_URL) {
                                let mut id = 0;
                                args.NavigationId(&mut id)?;
                                state.navigation_id.set(Some(id));
                            }
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let state = Rc::downgrade(owner);
            native.add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
                    if let (Some(state), Some(sender), Some(args)) = (state.upgrade(), sender, args) {
                        let mut id = 0;
                        args.NavigationId(&mut id)?;
                        // Ignore blank startup and later blocked navigations. A
                        // canceled author link must preserve the retained page.
                        if state.navigation_id.get() != Some(id)
                            || state.status.get() != LoadStatus::Loading
                        {
                            return Ok(());
                        }
                        let mut success = BOOL::default();
                        args.IsSuccess(&mut success)?;
                        if !success.as_bool() {
                            state.status.set(LoadStatus::Failed);
                            return Ok(());
                        }
                        // A cross-origin navigation can replace the CDP target.
                        // Reinstall the native policy on the loaded target before
                        // allowing the host to display or interact with it.
                        let weak = Rc::downgrade(&state);
                        let result = sender.CallDevToolsProtocolMethod(
                            w!("Page.setInterceptFileChooserDialog"),
                            w!("{\"enabled\":true,\"cancel\":true}"),
                            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                                move |result, _| {
                                    if let Some(state) = weak.upgrade() {
                                        match result {
                                            Ok(()) => state.status.set(LoadStatus::Ready),
                                            Err(error) => {
                                                state.status.set(LoadStatus::Failed);
                                                *state.error.borrow_mut() = Some(format!(
                                                    "Couldn’t install the HTML file chooser policy: {error}"
                                                ));
                                            }
                                        }
                                    }
                                    Ok(())
                                },
                            )),
                        );
                        if let Err(error) = result {
                            state.status.set(LoadStatus::Failed);
                            *state.error.borrow_mut() = Some(format!(
                                "Couldn’t install the HTML file chooser policy: {error}"
                            ));
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )?;
            let state = Rc::downgrade(owner);
            native.add_WebMessageReceived(
                &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                    let (Some(state), Some(args)) = (state.upgrade(), args) else {
                        return Ok(());
                    };
                    if !state.integrated.get() {
                        return Ok(());
                    }
                    let mut pointer = windows::core::PWSTR::null();
                    args.Source(&raw mut pointer)?;
                    let source = pointer.to_string();
                    windows::Win32::System::Com::CoTaskMemFree(Some(pointer.0.cast()));
                    if !source.is_ok_and(|url| {
                        url == DOCUMENT_URL || url.starts_with(&format!("{DOCUMENT_URL}#"))
                    }) {
                        return Ok(());
                    }
                    let mut pointer = windows::core::PWSTR::null();
                    args.WebMessageAsJson(&raw mut pointer)?;
                    let message = pointer.to_string();
                    windows::Win32::System::Com::CoTaskMemFree(Some(pointer.0.cast()));
                    if let Some(wheel) = message.ok().as_deref().and_then(parse_wheel) {
                        forward_wheel(&state, wheel);
                    }
                    Ok(())
                })),
                &raw mut token,
            )?;
            let state = Rc::downgrade(owner);
            native.add_ProcessFailed(
                &ProcessFailedEventHandler::create(Box::new(move |_, _| {
                    if let Some(state) = state.upgrade() {
                        state.status.set(LoadStatus::Failed);
                    }
                    Ok(())
                })),
                &mut token,
            )?;
        }
        // The browser process intercepts file chooser dialogs, including controls
        // hidden in closed declarative shadow roots. Author bytes cannot remove
        // this native policy. Await the acknowledgement before navigation: CDP
        // commands can otherwise complete out of order.
        let state = Rc::downgrade(owner);
        let navigation = native.clone();
        unsafe {
            native.CallDevToolsProtocolMethod(
                w!("Page.setInterceptFileChooserDialog"),
                w!("{\"enabled\":true,\"cancel\":true}"),
                &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, _| {
                    let Some(state) = state.upgrade() else {
                        return Ok(());
                    };
                    let result = result.and_then(|()| {
                        navigation.Navigate(&windows::core::HSTRING::from(DOCUMENT_URL))
                    });
                    if let Err(error) = result {
                        state.status.set(LoadStatus::Failed);
                        *state.error.borrow_mut() = Some(format!(
                            "Couldn’t install the HTML file chooser policy: {error}"
                        ));
                    }
                    Ok(())
                })),
            )?;
        }
        owner.source.borrow_mut().take();
        *owner.profile.borrow_mut() = Some(profile);
        Ok(view)
    }

    /// Last measured body height; each call starts at most one new measurement.
    pub fn report_height(&self) -> f64 {
        let state = &self.state;
        if state.status.get() == LoadStatus::Ready && !state.measuring.get() {
            if let Some(view) = state.view.borrow().as_ref() {
                let weak = Rc::downgrade(state);
                // Wry's callback must be Send; the document state is not.
                // SAFETY: STA-thread call on a live webview; the handler holds a weak state.
                let started = unsafe {
                    view.webview().ExecuteScript(
                        &windows::core::HSTRING::from(MEASURE),
                        &ExecuteScriptCompletedHandler::create(Box::new(move |result, value| {
                            if let Some(state) = weak.upgrade() {
                                state.measuring.set(false);
                                let height = value.trim().parse::<f64>().unwrap_or(0.);
                                if result.is_ok() && height.is_finite() && height > 0. {
                                    state.report_height.set(height);
                                }
                            }
                            Ok(())
                        })),
                    )
                };
                state.measuring.set(started.is_ok());
            }
        }
        state.report_height.get()
    }

    /// Apply host theme CSS. `integrated` forwards wheel input to the host.
    pub fn report_theme(&self, script: &str, integrated: bool) {
        self.state.integrated.set(integrated);
        if let Some(view) = self.state.view.borrow().as_ref() {
            let _ = view.evaluate_script(&format!(
                "globalThis.__groveReportWheel?.({integrated});{script}"
            ));
        }
    }

    pub fn failure_reason(&self) -> Option<String> {
        self.state.error.borrow().clone()
    }

    pub fn status(&self) -> LoadStatus {
        self.state.status.get()
    }
    pub fn hide(&self) {
        hide_state(&self.state);
    }
    pub fn retire_frame(&self) {
        // Timer callbacks run on this STA after GPUI finishes replacing its frame.
        // A paint in the meantime cancels retirement by advancing the generation.
        let timer = unsafe { SetTimer(None, 0, 1, Some(retire_after_paint)) };
        if timer == 0 {
            self.hide();
            return;
        }
        RETIREMENTS.with(|retirements| {
            retirements.borrow_mut().insert(
                timer,
                (Rc::downgrade(&self.state), self.state.generation.get()),
            );
        });
    }

    pub fn exclude(
        &self,
        bounds: Bounds<Pixels>,
        clip: Bounds<Pixels>,
        exclusion: Option<Bounds<Pixels>>,
        window: &Window,
    ) {
        let view = self.state.view.borrow();
        let Some(view) = view.as_ref() else {
            return;
        };
        // A disabled child passes the first outside click to its GPUI host,
        // matching the existing preview dismissal behavior while retaining pixels.
        // SAFETY: Wry owns this live native child HWND on the current UI thread.
        unsafe {
            let _ = EnableWindow(view.hwnd(), exclusion.is_none());
        }
        let scale = f64::from(window.scale_factor());
        let rect = |rect: Bounds<Pixels>, outward: bool| {
            let round_start = |n: f64| if outward { n.floor() } else { n.ceil() };
            let round_end = |n: f64| if outward { n.ceil() } else { n.floor() };
            unsafe {
                CreateRectRgn(
                    round_start(
                        (f64::from(f32::from(rect.origin.x))
                            - f64::from(f32::from(bounds.origin.x)))
                            * scale,
                    ) as i32,
                    round_start(
                        (f64::from(f32::from(rect.origin.y))
                            - f64::from(f32::from(bounds.origin.y)))
                            * scale,
                    ) as i32,
                    round_end(
                        (f64::from(f32::from(rect.origin.x + rect.size.width))
                            - f64::from(f32::from(bounds.origin.x)))
                            * scale,
                    ) as i32,
                    round_end(
                        (f64::from(f32::from(rect.origin.y + rect.size.height))
                            - f64::from(f32::from(bounds.origin.y)))
                            * scale,
                    ) as i32,
                )
            }
        };
        // SAFETY: owned regions, live Wry HWND. SetWindowRgn transfers ownership.
        unsafe {
            let region = rect(clip, false);
            if region.0.is_null() {
                let _ = view.set_visible(false);
                return;
            }
            if let Some(exclusion) = exclusion {
                let hole = rect(exclusion, true);
                if hole.0.is_null()
                    || CombineRgn(Some(region), Some(region), Some(hole), RGN_DIFF).0 == 0
                {
                    let _ = DeleteObject(hole.into());
                    let _ = DeleteObject(region.into());
                    let _ = view.set_visible(false);
                    return;
                }
                let _ = DeleteObject(hole.into());
            }
            if SetWindowRgn(view.hwnd(), Some(region), true) == 0 {
                let _ = DeleteObject(region.into());
                let _ = view.set_visible(false);
            }
        }
    }

    pub fn show(&self, bounds: Bounds<Pixels>, clip: Bounds<Pixels>, window: &Window) {
        if bounds.size.width <= Pixels::ZERO
            || bounds.size.height <= Pixels::ZERO
            || clip.size.width <= Pixels::ZERO
            || clip.size.height <= Pixels::ZERO
        {
            self.hide();
            return;
        }
        let view = self.state.view.borrow();
        let Some(view) = view.as_ref() else {
            return;
        };
        let scale = f64::from(window.scale_factor());
        let logical = |value: Pixels| f64::from(f32::from(value));
        let rect = wry::Rect {
            position: wry::dpi::LogicalPosition::new(
                logical(bounds.origin.x),
                logical(bounds.origin.y),
            )
            .into(),
            size: wry::dpi::LogicalSize::new(
                logical(bounds.size.width),
                logical(bounds.size.height),
            )
            .into(),
        };
        let left = ((logical(clip.origin.x) - logical(bounds.origin.x)) * scale).ceil() as i32;
        let top = ((logical(clip.origin.y) - logical(bounds.origin.y)) * scale).ceil() as i32;
        let right = ((logical(clip.origin.x + clip.size.width) - logical(bounds.origin.x)) * scale)
            .floor() as i32;
        let bottom = ((logical(clip.origin.y + clip.size.height) - logical(bounds.origin.y))
            * scale)
            .floor() as i32;
        if view.set_bounds(rect).is_err() {
            let _ = view.set_visible(false);
            return;
        }
        // SAFETY: Wry owns a live child HWND. Windows owns the region after a
        // successful SetWindowRgn. The region clips both pixels and hit testing;
        // the viewport retains its original size while scrolling under app chrome.
        unsafe {
            let region = CreateRectRgn(left, top, right, bottom);
            if region.0.is_null() {
                let _ = view.set_visible(false);
                return;
            }
            if SetWindowRgn(view.hwnd(), Some(region), true) == 0 {
                let _ = DeleteObject(region.into());
                let _ = view.set_visible(false);
                return;
            }
        }
        self.state
            .generation
            .set(self.state.generation.get().wrapping_add(1));
        let _ = view.set_visible(true);
    }
}

#[derive(Clone, Copy)]
struct HostWindow(windows::Win32::Foundation::HWND);
impl raw_window_handle::HasWindowHandle for HostWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let handle = std::num::NonZeroIsize::new(self.0 .0 as isize)
            .ok_or(raw_window_handle::HandleError::Unavailable)?;
        // SAFETY: the callback checks the live HWND before constructing Wry. This
        // adapter is used only on its owning STA and does not borrow GPUI state.
        Ok(unsafe {
            raw_window_handle::WindowHandle::borrow_raw(
                raw_window_handle::Win32WindowHandle::new(handle).into(),
            )
        })
    }
}

unsafe extern "system" fn initialize_after_render(
    _: windows::Win32::Foundation::HWND,
    _: u32,
    timer: usize,
    _: u32,
) {
    let _ = unsafe { KillTimer(None, timer) };
    let initialization =
        INITIALIZATIONS.with(|initializations| initializations.borrow_mut().remove(&timer));
    let Some((weak, host)) = initialization else {
        return;
    };
    let Some(state) = weak.upgrade() else {
        return;
    };
    let result =
        if unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(host.0)).as_bool() } {
            Document::create(&state, &host)
        } else {
            Err("HTML preview host window closed".into())
        };
    match result {
        Ok(view) => {
            *state.view.borrow_mut() = Some(view);
        }
        Err(error) => {
            state.status.set(LoadStatus::Failed);
            *state.error.borrow_mut() = Some(format!("Couldn’t initialize the HTML preview. Install or update Microsoft Edge WebView2 Runtime. {error}"));
        }
    }
    state.initializing.set(false);
}

unsafe extern "system" fn retire_after_paint(
    _: windows::Win32::Foundation::HWND,
    _: u32,
    timer: usize,
    _: u32,
) {
    // SAFETY: this timer was created on the STA and owns this registry entry.
    let _ = unsafe { KillTimer(None, timer) };
    let retirement = RETIREMENTS.with(|retirements| retirements.borrow_mut().remove(&timer));
    if let Some((weak, generation)) = retirement {
        if let Some(state) = weak.upgrade() {
            if state.generation.get() == generation {
                hide_state(&state);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Wheel {
    dx: f64,
    dy: f64,
    mode: WheelMode,
    x: f64,
    y: f64,
}

#[derive(Clone, Copy)]
enum WheelMode {
    Pixel,
    Line,
    Page,
}

/// Accept only `{"grove_wheel":[dx,dy,mode,x,y]}` with finite numbers.
fn parse_wheel(json: &str) -> Option<Wheel> {
    if json.len() > MAX_WHEEL_MESSAGE {
        return None;
    }
    let serde_json::Value::Object(message) = serde_json::from_str(json).ok()? else {
        return None;
    };
    let (Some(serde_json::Value::Array(values)), 1) = (message.get("grove_wheel"), message.len())
    else {
        return None;
    };
    let [dx, dy, mode, x, y] = values.as_slice() else {
        return None;
    };
    let mode = match mode.as_u64()? {
        0 => WheelMode::Pixel,
        1 => WheelMode::Line,
        2 => WheelMode::Page,
        _ => return None,
    };
    // serde_json numbers are always finite; strings, nulls and arrays fail here.
    Some(Wheel {
        dx: dx.as_f64()?,
        dy: dy.as_f64()?,
        mode,
        x: x.as_f64()?,
        y: y.as_f64()?,
    })
}

/// Convert a DOM wheel delta to signed `WHEEL_DELTA` units the way Chromium
/// derived it: pixels from notches x system lines x 100/3, or lines, or pages.
fn wheel_units(delta: f64, mode: WheelMode, zoom: f64, lines: u32) -> i16 {
    let lines = f64::from(if lines == 0 || lines == WHEEL_PAGESCROLL {
        3
    } else {
        lines
    });
    let notches = match mode {
        WheelMode::Pixel => delta * zoom / (PIXELS_PER_LINE * lines),
        WheelMode::Line => delta / lines,
        WheelMode::Page => delta,
    };
    (notches.clamp(-MAX_WHEEL_NOTCHES, MAX_WHEEL_NOTCHES) * f64::from(WHEEL_DELTA)).round() as i16
}

fn wheel_setting(
    action: windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_ACTION,
) -> u32 {
    let mut value = 3u32;
    // SAFETY: both queried actions write one UINT to the supplied pointer.
    let _ = unsafe {
        SystemParametersInfoW(
            action,
            0,
            Some((&raw mut value).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    value
}

fn forward_wheel(state: &State, wheel: Wheel) {
    let Wheel { dx, dy, mode, x, y } = wheel;
    let Ok(view) = state.view.try_borrow() else {
        return;
    };
    let Some(view) = view.as_ref() else {
        return;
    };
    let hwnd = view.hwnd();
    // SAFETY: STA-thread calls on Wry's live child HWND and its controller. Only
    // wheel messages are posted to the direct GPUI parent; no pointers escape.
    unsafe {
        if !state.integrated.get() || !IsWindowVisible(hwnd).as_bool() {
            return;
        }
        let controller = view.controller();
        let mut zoom = 1.;
        if controller.ZoomFactor(&raw mut zoom).is_err() || !zoom.is_finite() || zoom <= 0. {
            zoom = 1.;
        }
        let mut raster = 1.;
        let raster_ok = controller
            .cast::<ICoreWebView2Controller3>()
            .and_then(|controller| controller.RasterizationScale(&raw mut raster));
        if raster_ok.is_err() || !raster.is_finite() || raster <= 0. {
            raster = 1.;
        }
        let mut client = RECT::default();
        if GetClientRect(hwnd, &raw mut client).is_err() {
            return;
        }
        let scale = zoom * raster;
        let mut point = POINT {
            x: (x * scale).clamp(f64::from(client.left), f64::from(client.right - 1).max(0.))
                as i32,
            y: (y * scale).clamp(f64::from(client.top), f64::from(client.bottom - 1).max(0.))
                as i32,
        };
        if !ClientToScreen(hwnd, &raw mut point).as_bool() {
            return;
        }
        let host = GetAncestor(hwnd, GA_PARENT);
        if host.0.is_null() {
            return;
        }
        let pressed = |key: VIRTUAL_KEY| GetKeyState(i32::from(key.0)) < 0;
        let keys = [
            (VK_LBUTTON, 0x0001),
            (VK_RBUTTON, 0x0002),
            (VK_SHIFT, 0x0004),
            (VK_CONTROL, 0x0008),
            (VK_MBUTTON, 0x0010),
        ]
        .into_iter()
        .filter(|(key, _)| pressed(*key))
        .fold(0usize, |keys, (_, flag)| keys | flag);
        let lparam =
            LPARAM(((i32::from(point.y as i16) << 16) | i32::from(point.x as u16)) as isize);
        // DOM +deltaY scrolls down; WM_MOUSEWHEEL +delta scrolls up. Both use +X as right.
        let vertical = wheel_units(-dy, mode, zoom, wheel_setting(SPI_GETWHEELSCROLLLINES));
        let horizontal = wheel_units(dx, mode, zoom, wheel_setting(SPI_GETWHEELSCROLLCHARS));
        for (message, units) in [(WM_MOUSEWHEEL, vertical), (WM_MOUSEHWHEEL, horizontal)] {
            if units != 0 {
                let wparam = WPARAM((usize::from(units as u16) << 16) | keys);
                let _ = PostMessageW(Some(host), message, wparam, lparam);
            }
        }
    }
}

fn hide_state(state: &State) {
    if let Some(view) = state.view.borrow().as_ref() {
        // Only return focus if this document owns it. Hiding an unrelated
        // document must preserve another preview or host editor's focus.
        let focus = unsafe { GetFocus() };
        if focus == view.hwnd() || unsafe { IsChild(view.hwnd(), focus).as_bool() } {
            let _ = view.focus_parent();
        }
        let _ = view.set_visible(false);
    }
}

pub fn hide_all() {
    DOCUMENTS.with(|documents| {
        documents.borrow_mut().retain(|weak| {
            if let Some(state) = weak.upgrade() {
                hide_state(&state);
                true
            } else {
                false
            }
        });
    });
}

pub fn focus_host(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    DOCUMENTS.with(|documents| {
        for state in documents.borrow().iter().filter_map(Weak::upgrade) {
            if let Some(view) = state.view.borrow().as_ref() {
                // SAFETY: both HWNDs belong to live main-thread windows. Only
                // transfer focus for a document whose root is the requested host.
                let root = unsafe { GetAncestor(view.hwnd(), GA_ROOT) };
                let focus = unsafe { GetFocus() };
                if root.0 as isize == handle.hwnd.get()
                    && (focus == view.hwnd() || unsafe { IsChild(view.hwnd(), focus).as_bool() })
                {
                    let _ = view.focus_parent();
                }
            }
        }
    });
}

impl Drop for Document {
    fn drop(&mut self) {
        self.hide();
        self.state.view.borrow_mut().take();
        self.state.profile.borrow_mut().take();
    }
}

/// Resolve the user's HTTPS browser rather than the HTML editor association.
/// The caller validates a local file URL; Windows argument encoding avoids shell parsing.
pub fn open_in_browser(url: String) -> impl std::future::Future<Output = Result<(), String>> {
    let result = (|| {
        use windows::Win32::UI::Shell::{
            AssocQueryStringW, ASSOCF_IS_PROTOCOL, ASSOCSTR_EXECUTABLE,
        };
        let mut length = 0;
        // SAFETY: the first query obtains a UTF-16 buffer size, and the second
        // writes at most that size. The protocol name is a static wide string.
        let _ = unsafe {
            AssocQueryStringW(
                ASSOCF_IS_PROTOCOL,
                ASSOCSTR_EXECUTABLE,
                w!("https"),
                None,
                None,
                &mut length,
            )
        };
        if length == 0 {
            return Err("Couldn’t resolve the default HTTPS browser".into());
        }
        let mut buffer = vec![0; length as usize];
        unsafe {
            AssocQueryStringW(
                ASSOCF_IS_PROTOCOL,
                ASSOCSTR_EXECUTABLE,
                w!("https"),
                None,
                Some(windows::core::PWSTR(buffer.as_mut_ptr())),
                &mut length,
            )
        }
        .ok()
        .map_err(|error| format!("Couldn’t resolve the default HTTPS browser: {error}"))?;
        let end = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        use std::os::windows::ffi::OsStringExt;
        let executable = std::ffi::OsString::from_wide(&buffer[..end]);
        std::process::Command::new(executable)
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("Couldn’t open HTML in the default browser: {error}"))
    })();
    std::future::ready(result)
}
