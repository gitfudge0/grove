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
