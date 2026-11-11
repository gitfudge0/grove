//! OS text preference bridge. Display scaling remains owned by GPUI.
#![cfg_attr(target_os = "linux", allow(unsafe_code))]

pub fn initial_scale() -> f32 {
    #[cfg(all(target_os = "linux", not(test)))]
    {
        // SAFETY: boot and polling run exclusively on the GPUI foreground thread.
        unsafe { grove_text_scale_init() as f32 }
    }
    #[cfg(any(not(target_os = "linux"), test))]
    {
        1.0
    }
}

#[cfg(all(target_os = "linux", not(test)))]
pub fn poll(cx: &mut gpui::App) {
    // SAFETY: the main-thread bridge drains queued GSettings notifications.
    let scale = unsafe { grove_text_scale_poll() as f32 };
    let current = cx.global::<crate::zoom::ZoomState>();
    let next = crate::zoom::ZoomState::new(current.zoom).with_text_scale(scale);
    if next.effective_scale() != current.effective_scale() {
        tracing::info!(
            text_scale = scale,
            rem = next.rem_size(),
            "OS text size changed"
        );
        cx.set_global(next);
        sync_component_size(cx);
        cx.refresh_windows();
    }
}

pub fn sync_component_size(cx: &mut gpui::App) {
    let rem = cx.global::<crate::zoom::ZoomState>().rem_size();
    gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
}

#[cfg(all(target_os = "linux", not(test)))]
unsafe extern "C" {
    fn grove_text_scale_init() -> f64;
    fn grove_text_scale_poll() -> f64;
}
