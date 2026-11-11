# Grove Linux platform patch

Source: Zed gpui_linux at 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba.
Apache-2.0 license is retained. Inherited manifest dependencies resolve to the
same upstream versions and pinned git sources.

The file-drop platform patch uses XdndPosition's root coordinates, translated into
window-local logical pixels, instead of querying the pointer. A live GNOME Files
Wayland-to-XWayland drag demonstrated that QueryPointer stayed at the window
edge while XdndPosition moved across the input. A live failed drop also delivered a plain path rather than a file URI.
The file decoder now requests only text/uri-list, which matches its URI parser.

Run `cargo test --test x11_file_drop` for packed-coordinate regression checks.
Live GNOME Files drops are required to verify the actual compositor boundary.

The X11 window also reads the immutable GPU subpixel-rendering capability from
its renderer. Glyph painting can occur while an X11 client callback holds the
client borrow; querying that client again from the glyph path panics. The
renderer-owned capability avoids the reentrant borrow without changing the GPU
feature decision. Verify by launching the native app on X11 and drawing text.

The dispatcher removes two copied `nightly_coverage` attributes from its
`ChannelError` forwarding methods. This vendored crate does not define that
feature; the unsupported annotations fail stable CI with dependency warnings
denied. Removing them does not change runtime behavior.

X11 startup scale selection preserves `GPUI_X11_SCALE_FACTOR` as the explicit
override (including `randr` mode). Otherwise it uses a valid `GDK_SCALE`, then
`Xft.dpi / 96`, then the RandR physical-DPI estimate, finally 1x. Invalid automatic
values warn and fall through; invalid explicit overrides retain their startup
error. The selected source and factor are logged. This avoids estimating 2.5x
from a monitor's physical dimensions when the desktop explicitly requests 2x.
`GDK_DPI_SCALE` adjusts text and is not a whole-window scale; Grove's app zoom
also remains independent of display scaling.

Run `cargo test --test x11_scale` for selection and validation regressions.
Selection is startup-only: XSettings-only desktops, runtime scale changes and
mixed-DPI monitor moves remain unsupported by this patch. Actual compositor-side
scaling and native/HTML alignment require isolated desktop verification.

Grove supplies the startup GSettings text factor through `set_xft_text_scale`
before platform creation. Only the Xft.dpi fallback divides out this factor,
because GNOME publishes text DPI multiplied by its window scale in Xft.dpi.
Explicit GPUI/GDK overrides and physical RandR estimates remain display-only.
Runtime text preference changes update application sizing, not display scale.
