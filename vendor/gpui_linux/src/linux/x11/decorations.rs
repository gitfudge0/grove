// XWayland composites client surfaces without requiring X11 compositor or
// GTK shadow-hint conventions. Traditional X11 keeps its existing fallback.
pub(super) fn supports_client_decorations(
    xwayland: bool,
    compositor_present: bool,
    gtk_frame_extents_supported: bool,
) -> bool {
    xwayland || (compositor_present && gtk_frame_extents_supported)
}
