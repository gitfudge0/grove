//! OS clipboard formats beyond the GPUI string/image interface.

use std::cell::RefCell;

thread_local! {
    // X11 and Wayland serve copied data for the lifetime of the clipboard owner.
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

pub(crate) fn copy_html(html: &str, plain: &str) -> bool {
    CLIPBOARD.with_borrow_mut(|clipboard| {
        if clipboard.is_none() {
            *clipboard = arboard::Clipboard::new().ok();
        }
        clipboard
            .as_mut()
            .is_some_and(|clipboard| clipboard.set_html(html, Some(plain)).is_ok())
    })
}
