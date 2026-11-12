#[path = "../vendor/gpui_linux/src/linux/x11/decorations.rs"]
mod decorations;
#[test]
fn xwayland_keeps_controls_without_legacy_compositor_hints() {
    for compositor in [false, true] {
        for gtk_extents in [false, true] {
            assert!(decorations::supports_client_decorations(
                true,
                compositor,
                gtk_extents
            ));
        }
    }
}
#[test]
fn traditional_x11_retains_server_fallback_without_compositing_support() {
    assert!(!decorations::supports_client_decorations(
        false, false, false
    ));
    assert!(!decorations::supports_client_decorations(
        false, true, false
    ));
    assert!(!decorations::supports_client_decorations(
        false, false, true
    ));
    assert!(decorations::supports_client_decorations(false, true, true));
}
