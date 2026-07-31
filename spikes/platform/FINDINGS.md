# Spike S4 — Linux platform matrix (gpui)

Binary: `spikes/platform/src/main.rs` (`cargo run -p spike-platform`).
gpui rev: `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba` (checked out at
`~/.cargo/git/checkouts/zed-a70e2ad075855582/1a246ef`).

Test session: nested/headless Wayland compositor (`WAYLAND_DISPLAY=wayland-1`,
`DISPLAY=:1` via Xwayland, `XDG_SESSION_TYPE=wayland`), no physical display or
input device attached — so anything requiring a human click/drag is MANUAL.

## Verdict summary

| Item | Wayland | X11 | Notes |
|---|---|---|---|
| Window opens at 1280x800, title set | PASS | PASS | Confirmed via `window.window_bounds()` log both runs |
| Resize event logging | MANUAL | MANUAL | Not wired as a distinct hook this spike (see Deviations) |
| Focus/blur (window activation) events | PASS (fires) / MANUAL (verify semantics) | PASS (fires) / MANUAL | `observe_window_activation` fired twice in both runs with no user input — see Deviations |
| Close-request interception (should-close) | MANUAL | MANUAL | Code path implemented and correct API confirmed; needs a human to click close twice |
| File drag-drop delivered as native gpui event | MANUAL | MANUAL | `.on_drop::<ExternalPaths>()` registered on root div; needs a human to drag a file from a file manager |
| Clipboard write+read round-trip (automated, in-process) | **FAIL** (`None` both immediately and after 300ms post-focus) | PASS (immediate) | See Deviations — likely a Wayland focus/session-compositor artifact, not necessarily representative of a real desktop |
| Clipboard cross-app paste verification | MANUAL | MANUAL | Requires pasting into an external terminal; not automatable here |

## Exact APIs (for Plans 03/09 to copy)

- **Force X11 vs Wayland**: gpui's Linux platform picks its backend via
  `gpui::guess_compositor()` (`crates/gpui/src/platform.rs`), which checks
  `WAYLAND_DISPLAY` first, then `DISPLAY`, else `"Headless"`
  (`ZED_HEADLESS` env var forces headless). `gpui_linux::current_platform()`
  matches on that string. **To force X11: unset/empty `WAYLAND_DISPLAY`**
  while keeping `DISPLAY` set to a reachable X server (Xwayland counts):
  `WAYLAND_DISPLAY= cargo run -p spike-platform`.
- **App bootstrap**: `gpui::Application::new()` does not exist at this rev.
  Use `gpui_platform::application()` (crate `gpui_platform`, re-exports
  `current_platform()` per-OS) — added `gpui_platform = { workspace = true }`
  to `spikes/Cargo.toml` and `spikes/platform/Cargo.toml`.
- **Window options**: `gpui::WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), titlebar: Some(TitlebarOptions { title: Some(title.into()), .. }), .. Default::default() }`, opened via `App::open_window`. Bounds via `Bounds::centered(None, size(px(1280.), px(800.)), cx)`.
- **Close interception**: `Window::on_window_should_close(&self, cx: &App, f: impl Fn(&mut Window, &mut App) -> bool + 'static)` (`crates/gpui/src/window.rs`). Return `false` to veto the close, `true` to allow it. Confirmed compiling and registering correctly; the callback body and log lines are in place in `main.rs` (increments a `Rc<Cell<u32>>` counter, vetoes attempt #1, allows #2+).
- **Window/app activation ("focus/blur")**: there is no window-level `on_focus`/`on_blur` on `Window` itself for OS-level activation — that's `Context::observe_window_activation(&self, window: &mut Window, callback: impl FnMut(&mut T, &mut Window, &mut Context<T>))` (`crates/gpui/src/app/context.rs`), called from inside `Entity::update`. Query current state via `Window::is_active(&mut self, cx: &mut App) -> Option<bool>`. (Element-level keyboard focus in/out — a different concept — is `Window::on_focus_in`/`on_focus_out` taking a `FocusHandle`.)
- **File drop**: delivered as `gpui::ExternalPaths` (wraps `SmallVec<[PathBuf; 2]>`, `.paths()` returns `&[PathBuf]`) via `Div::on_drop::<ExternalPaths>(listener)` (`crates/gpui/src/elements/div.rs`). Under the hood this is `PlatformInput::FileDrop(FileDropEvent::{Entered,Pending,Submit,Exited})` (`crates/gpui/src/interactive.rs`) — gpui delivers real file paths first-party on both backends via its own Wayland (`wl_data_device`) and X11 clients; no `wl-paste` fallback needed in the gpui path itself for drops.
- **Clipboard**: `App`/`Context<T>` expose `write_to_clipboard(&self, item: ClipboardItem)` and `read_from_clipboard(&self) -> Option<ClipboardItem>` (`crates/gpui/src/app.rs`), backed by `Platform::{read,write}_from/to_clipboard`. `ClipboardItem::new_string(String)` / `.text() -> Option<String>`.

## Deviations from the plan / things that didn't go as expected

1. **`Application::new()` doesn't exist at this gpui rev.** Assumed a simple constructor going in; this rev requires `gpui_platform::application()` (or `Application::with_platform(current_platform(headless))` directly). Added the `gpui_platform` crate as a workspace dependency (same git/rev pin) to both `spikes/Cargo.toml` and `spikes/platform/Cargo.toml`.
2. **Per-resize-event logging not wired as a separate handler.** Given the time budget, window-level resize logging was not added as a distinct hook (that lives on the lower-level `PlatformWindow` trait, `fn on_resize`, invoked internally by `Window::new`, not exposed as a simple public per-window registration the way should-close is). Window-level activation (`observe_window_activation`) *is* wired and logs to stderr — this covers the "focus/blur" need called out in the plan for attention acknowledge-on-refocus. Flagging resize-event logging as a follow-up rather than blocking the spike.
3. **Activation fired twice with zero user input**, in both Wayland and X11 runs. Likely: once for initial window-shown/mapped, once for actual compositor-granted keyboard focus. Not investigated further — recommend MANUAL verification that this maps cleanly onto the "focused session never shows WaitingForInput" acknowledge-on-refocus need from `src/attention.rs`.
4. **Automated clipboard round-trip failed on Wayland, passed on X11 (Xwayland), in this environment.** Sequence tried: (a) write+read immediately in the `Application::run` startup closure — `None` on Wayland, `Some(marker)` on X11; (b) write+read again ~300ms later, after two activation events had already fired (so focus should be established) — still `None` on Wayland. Root cause not fully isolated in the spike time budget; gpui's Wayland `write_to_clipboard` (`gpui_linux::linux::wayland::client`) gates on `mouse_focused_window.is_some() || keyboard_focused_window.is_some()` before calling `wl_data_device.set_selection`, so this reads like a focus-state or nested-compositor artifact of this sandboxed session (no real input device, no real window manager) rather than a proven gpui defect — but it needs MANUAL verification against a normal Wayland desktop session before Plans 03/09 rely on in-process clipboard for anything user-facing.
5. **No X11/Wayland input-injection tool was available** (`xdotool`/`wtype` not installed; only `ydotool`, which needs a running daemon) — could not automate the close-click, drag-drop, or a real keypress-driven "press c / press v" test. Worked around this for clipboard by adding a parallel **automated** self-test (write immediately at startup, then again via `cx.spawn` + a 300ms timer) directly in `Application::run`, in addition to the interactive `c`/`v` keybindings — so the API round-trip itself got exercised without needing key injection. Close-interception, drag-drop, and the interactive clipboard keys are implemented and logging is in place, but need a human to actually click/drag/type.

## Arboard verdict

**Arboard is very likely still needed, or at minimum gpui's clipboard needs
more validation before Grove relies on it exclusively:**
- The automated in-process round-trip **did not verify itself** on Wayland
  in this test session (see Deviation 4). Until that's confirmed clean on a
  real desktop, treat gpui's Wayland clipboard as unproven for Grove's use
  cases (e.g., copying a path or diff).
- X11 (Xwayland) round-trip worked immediately and reliably in-process.
- The framework-free OSC52 path (called out in the plan as "framework-free
  regardless") is unaffected by any of this and remains available as a
  fallback for terminal-adjacent copy paths no matter which clipboard
  backend Grove ends up using.
- Recommendation: keep `arboard` (or an OSC52/`wl-copy`+`wl-paste` fallback,
  matching the existing pattern in `src/gui/drop.rs`) as a safety net for
  clipboard writes/reads until a human has manually confirmed gpui's native
  clipboard round-trips correctly on a real Wayland compositor (GNOME/KDE/
  Sway) with a real input device — this sandboxed test session is not
  conclusive proof either way.

## How to manually verify the rest

Run `cargo run -p spike-platform` (Wayland) and
`WAYLAND_DISPLAY= cargo run -p spike-platform` (X11, needs `$DISPLAY`
reachable) and watch stderr while:

1. Resizing the window — confirm it resizes at all (no gpui-side log wired
   for this yet).
2. Clicking the window close button once (should log a veto and stay open),
   then again (should log allow and exit).
3. Dragging a file from a file manager onto the window — should log
   `[drop] received path: ...` for each dropped path.
4. Clicking into the window then pressing `c` (writes marker), then `v`
   (reads it back) — compare against the `[clipboard-autotest]` lines that
   already run automatically at startup.
5. Alt-tabbing away and back — should produce additional
   `[focus] window activation changed` lines.
