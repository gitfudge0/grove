//! Main-thread-only native document ownership and host integration.
#![cfg_attr(target_os = "macos", allow(unsafe_code))]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadStatus {
    Loading,
    Ready,
    Failed,
}

#[cfg(all(target_os = "windows", not(test)))]
#[path = "inline_html_windows.rs"]
mod backend;

#[cfg(all(target_os = "linux", not(test)))]
#[path = "inline_html_linux.rs"]
mod backend;

pub struct NativeDocument {
    #[cfg(all(any(target_os = "windows", target_os = "linux"), not(test)))]
    backend: backend::Document,
    #[cfg(test)]
    visibility_probe: std::rc::Rc<std::cell::Cell<bool>>,
    #[cfg(all(target_os = "macos", not(test)))]
    handle: std::ptr::NonNull<std::ffi::c_void>,
}

impl NativeDocument {
    /// Drive GTK's native event queue on the same thread as GPUI.
    #[cfg(all(target_os = "linux", not(test)))]
    pub fn pump() {
        backend::pump();
    }

    /// Attach the system webview to its live host before polling load status.
    /// Construction can happen before a GPUI window is available (file reads).
    pub fn prepare(&self, window: &gpui::Window) -> Result<(), String> {
        #[cfg(all(any(target_os = "windows", target_os = "linux"), not(test)))]
        {
            self.backend.prepare(window)
        }
        #[cfg(any(not(any(target_os = "windows", target_os = "linux")), test))]
        {
            let _ = window;
            Ok(())
        }
    }

    /// Return native keyboard ownership when the host composer is activated.
    pub fn focus_host(window: &gpui::Window) {
        #[cfg(all(any(target_os = "windows", target_os = "linux"), not(test)))]
        backend::focus_host(window);
        #[cfg(all(target_os = "macos", not(test)))]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let Ok(handle) = HasWindowHandle::window_handle(window) else {
                return;
            };
            let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
                return;
            };
            // SAFETY: the borrowed native view belongs to this live Window;
            // composer activation executes on GPUI's AppKit thread.
            unsafe { grove_html_focus_host(handle.ns_view.as_ptr()) };
        }
        #[cfg(any(not(target_os = "macos"), test))]
        let _ = window;
    }

    /// Visibility-only seam for exercising the real GPUI element/frame lifetime.
    /// WebKit policy and document behavior are tested by the standalone harness.
    #[cfg(test)]
    pub fn with_visibility_probe(visible: std::rc::Rc<std::cell::Cell<bool>>) -> Self {
        Self {
            visibility_probe: visible,
        }
    }

    pub fn hide_all() {
        #[cfg(all(any(target_os = "windows", target_os = "linux"), not(test)))]
        backend::hide_all();
        #[cfg(all(target_os = "macos", not(test)))]
        // SAFETY: called on GPUI's AppKit thread at visibility transitions.
        unsafe {
            grove_html_hide_all();
        };
    }

    pub fn new(source: &str) -> Option<Self> {
        #[cfg(all(any(target_os = "windows", target_os = "linux"), not(test)))]
        {
            backend::Document::new(source).map(|backend| Self { backend })
        }
        #[cfg(all(target_os = "macos", not(test)))]
        {
            // SAFETY: called only on GPUI's AppKit thread. The native owner copies
            // the UTF-8 bytes synchronously and returns a retained opaque object.
            let handle = unsafe { grove_html_create(source.as_ptr(), source.len()) };
            std::ptr::NonNull::new(handle).map(|handle| Self { handle })
        }
        #[cfg(any(
            not(any(target_os = "macos", target_os = "windows", target_os = "linux")),
            test
        ))]
        {
            let _ = source;
            None
        }
    }

    pub fn status(&self) -> LoadStatus {
        #[cfg(all(any(target_os = "windows", target_os = "linux"), not(test)))]
        {
            self.backend.status()
        }
        #[cfg(all(target_os = "macos", not(test)))]
        {
            // SAFETY: this object retains the handle for its entire lifetime.
            match unsafe { grove_html_status(self.handle.as_ptr()) } {
                0 => LoadStatus::Loading,
                1 => LoadStatus::Ready,
                _ => LoadStatus::Failed,
            }
        }
        #[cfg(all(
            not(any(target_os = "macos", target_os = "windows", target_os = "linux")),
            not(test)
        ))]
        {
            LoadStatus::Failed
        }
        #[cfg(test)]
        {
            LoadStatus::Ready
        }
    }

    pub fn failure_reason(&self) -> Option<String> {
        #[cfg(all(target_os = "windows", not(test)))]
        {
            self.backend.failure_reason()
        }
        #[cfg(any(not(target_os = "windows"), test))]
        {
            None
        }
    }

    pub fn retire_frame(&self) {
        #[cfg(all(any(target_os = "windows", target_os = "linux"), not(test)))]
        self.backend.retire_frame();
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
        #[cfg(all(any(target_os = "windows", target_os = "linux"), not(test)))]
        self.backend.hide();
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
        #[cfg(all(any(target_os = "windows", target_os = "linux"), not(test)))]
        self.backend.show(bounds, clip, window);
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
    fn grove_html_focus_host(parent: *mut std::ffi::c_void);
}

/// This is invoked by app chrome, never by document JavaScript.
pub fn open_in_browser(url: String) -> impl std::future::Future<Output = Result<(), String>> {
    let validation = validate_browser_url(&url);
    async move {
        validation?;
        open_in_browser_unchecked(url).await
    }
}

fn validate_browser_url(url: &str) -> Result<(), String> {
    let url = gpui::http_client::Url::parse(url)
        .map_err(|_| "The browser target must be a local HTML file".to_owned())?;
    if url
        .host_str()
        .is_some_and(|host| !host.is_empty() && host != "localhost")
    {
        return Err("The browser target must be a local HTML file".into());
    }
    let path = url
        .to_file_path()
        .map_err(|()| "The browser target must be a local HTML file".to_owned())?;
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| {
            !extension.eq_ignore_ascii_case("html") && !extension.eq_ignore_ascii_case("htm")
        })
    {
        return Err("The browser target must be a local HTML file".into());
    }
    Ok(())
}

fn open_in_browser_unchecked(url: String) -> impl std::future::Future<Output = Result<(), String>> {
    #[cfg(all(target_os = "windows", not(test)))]
    {
        backend::open_in_browser(url)
    }
    #[cfg(all(target_os = "linux", not(test)))]
    {
        let (sender, receiver) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = sender.send(launch_linux_browser(&url));
        });
        async move {
            receiver
                .await
                .map_err(|_| "Browser launch did not report a result".to_owned())?
        }
    }

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
    #[cfg(any(
        not(any(target_os = "macos", target_os = "windows", target_os = "linux")),
        test
    ))]
    {
        let _ = url;
        std::future::ready(Err(
            "Explicit browser launch is unavailable on this platform".into(),
        ))
    }
}

#[cfg(all(target_os = "linux", not(test)))]
fn launch_linux_browser(url: &str) -> Result<(), String> {
    let handler = std::process::Command::new("xdg-mime")
        .args(["query", "default", "x-scheme-handler/https"])
        .output()
        .map_err(|error| format!("Couldn’t find the default web browser: {error}"))?;
    if !handler.status.success() {
        return Err("Couldn’t find the default web browser".into());
    }
    let handler = String::from_utf8(handler.stdout)
        .map_err(|_| "Invalid browser desktop entry".to_owned())?;
    let handler = handler.trim();
    if handler.is_empty() || handler.contains(['/', '\\']) || !handler.ends_with(".desktop") {
        return Err("No default web browser is configured".into());
    }
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/share")));
    let data_dirs =
        std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let desktop = data_home
        .into_iter()
        .chain(std::env::split_paths(&data_dirs))
        .map(|directory| directory.join("applications").join(handler))
        .find(|path| path.is_file())
        .ok_or_else(|| "Couldn’t locate the default browser’s desktop entry".to_owned())?;
    let output = std::process::Command::new("gio")
        .arg("launch")
        .arg(desktop)
        .arg(url)
        .output()
        .map_err(|error| format!("Couldn’t launch the default web browser: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Couldn’t launch the default web browser: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod browser_tests {
    use super::validate_browser_url;
    #[test]
    fn browser_action_accepts_only_local_html_files() {
        for target in [
            "https://example.com/a.html",
            "file://remote/a.html",
            "javascript:alert(1)",
            "file:///tmp/a.txt",
        ] {
            assert!(validate_browser_url(target).is_err(), "{target}");
        }
        for name in ["a.html", "a.HTM"] {
            let mut target =
                gpui::http_client::Url::from_file_path(std::env::temp_dir().join(name)).unwrap();
            target.set_query(Some("theme=dark"));
            target.set_fragment(Some("view"));
            assert!(validate_browser_url(target.as_str()).is_ok(), "{target}");
        }
    }
}
