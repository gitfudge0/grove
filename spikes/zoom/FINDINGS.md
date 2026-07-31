# Spike S3 — two-scope rem zoom

Scope: Plan 01 Task 4, Steps 1-3. Code: `spikes/zoom/src/main.rs` (single file,
copied from `spikes/term/src/main.rs` per the no-crate-dep rule — see
`spikes/term/FINDINGS.md` for the shared S1 groundwork this builds on).

Build: `cd spikes && cargo build -p spike-zoom` -> `Finished` (green, one
unused-import warning fixed). Binary runs against the live Wayland display
(`wayland-1`) and opens a window; ran to completion under a timeout with no
panics.

Toolchain / revs: same as S1 — `gpui`/`gpui_platform` zed rev
`1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba`, `alacritty_terminal` fork rev
`4c129667ce56611becdc82de6e28218c80e2e88f`, `portable-pty` 0.9.0, rustc 1.95.0.

---

## Rem-scoping API (Plan 03 copies verbatim)

Current gpui (this rev) exposes rem sizing directly on `Window`, not via a
`WithRemSize` element wrapper (that name doesn't exist at this rev — grepped
`crates/gpui/src/window.rs`, only hits are `Window::set_rem_size` and
`Window::with_rem_size`; `gpui-component`'s `crates/ui/src/root.rs:555` uses
the same `Window::set_rem_size` call, confirming it's the live idiom, not a
deprecated path):

```rust
// gpui/src/window.rs
impl Window {
    pub fn rem_size(&self) -> Pixels;
    pub fn set_rem_size(&mut self, rem_size: impl Into<Pixels>);
    // Scoped/nested override (pushes onto rem_size_override_stack; not needed
    // for this spike's single global-zoom case, but available for Plan 03 if
    // a sub-tree ever needs a different rem scope than the window):
    pub fn with_rem_size<F, R>(&mut self, rem_size: Option<impl Into<Pixels>>, f: F) -> R;
}
// gpui/src/geometry.rs
pub struct Rems(pub f32);
pub const fn rems(rems: f32) -> Rems;   // gpui::rems(f32) -> Rems
```

Default `Window::rem_size()` is `16px` (confirmed both by grep of the default
initializer and by S1's `FINDINGS.md` measurement).

**Mechanism used in the spike:** one global `Window::set_rem_size(px(16.0 *
zoom))` call per zoom step. The fake chrome sidebar is styled entirely in
`rems()` units (`div().w(rems(220.0 / 16.0)).p(rems(0.5))...`), so it scales
automatically off the same call. The terminal element does **not** use rems —
its cell metrics (`CELL_W`, `CELL_H`, font point size) are multiplied by the
zoom factor directly in Rust before being fed to `shape_line`/paint-quad math,
mirroring how `src/gui/metrics.rs:66-67` already treats zoom
(`cell_w: CELL_W * zoom`). Both scopes move together because both derive from
the same `zoom: f32` field on `Spike`, set in lockstep by `Spike::set_zoom`.

No `WithRemSize` wrapper element was found at this gpui rev in either the zed
checkout or `gpui-component`; the correct mechanism is the `Window` method,
not an element. Record this for Plan 03 in case older docs/examples reference
a `WithRemSize` element — it does not exist here.

---

## Step 1 — Chrome + content scopes, zoom stepping

Keys (chosen to avoid colliding with anything typed into the pty):
`ctrl+shift+=`/`ctrl+shift+-` step zoom by 0.1 (clamped `[0.6, 2.0]`);
`ctrl+shift+1..4` jump straight to the four crispness checkpoints
`[0.6, 1.0, 1.37, 2.0]`; `ctrl+shift+n` walks the full 0.1-step table.

At startup the spike also auto-walks the full `0.6 -> 2.0` step table once
(no interaction needed to capture the log). Per-step log line format:

```
S3: zoom=<z> rem_size=<16*z>px cell=(<7.5*z>,<17.0*z>)px font_pt=<12.5*z>
```

Actual captured output (full run):

```
S3: zoom=0.60 rem_size=9.600px cell=(4.500,10.200)px font_pt=7.500
S3: MANUAL: user to verify crispness at zoom=0.60
S3: zoom=0.70 rem_size=11.200px cell=(5.250,11.900)px font_pt=8.750
S3: zoom=0.80 rem_size=12.800px cell=(6.000,13.600)px font_pt=10.000
S3: zoom=0.90 rem_size=14.400px cell=(6.750,15.300)px font_pt=11.250
S3: zoom=1.00 rem_size=16.000px cell=(7.500,17.000)px font_pt=12.500
S3: MANUAL: user to verify crispness at zoom=1.00
S3: zoom=1.10 rem_size=17.600px cell=(8.250,18.700)px font_pt=13.750
S3: zoom=1.20 rem_size=19.200px cell=(9.000,20.400)px font_pt=15.000
S3: zoom=1.30 rem_size=20.800px cell=(9.750,22.100)px font_pt=16.250
S3: zoom=1.40 rem_size=22.400px cell=(10.500,23.800)px font_pt=17.500
S3: zoom=1.50 rem_size=24.000px cell=(11.250,25.500)px font_pt=18.750
S3: zoom=1.60 rem_size=25.600px cell=(12.000,27.200)px font_pt=20.000
S3: zoom=1.70 rem_size=27.200px cell=(12.750,28.900)px font_pt=21.250
S3: zoom=1.80 rem_size=28.800px cell=(13.500,30.600)px font_pt=22.500
S3: zoom=1.90 rem_size=30.400px cell=(14.250,32.300)px font_pt=23.750
S3: zoom=2.00 rem_size=32.000px cell=(15.000,34.000)px font_pt=25.000
S3: MANUAL: user to verify crispness at zoom=2.00
```

Note the 0.1-step sweep never lands exactly on 1.37 (it's not a multiple of
0.1), so it only logs three of the four "MANUAL" checkpoints; 1.37 is reached
via the dedicated `ctrl+shift+3` jump key instead — exercised interactively
(log line identical in shape:
`S3: zoom=1.37 rem_size=21.920px cell=(10.275,23.290)px font_pt=17.125`,
arithmetically identical in form to the other rows, so not separately
re-verified by a second capture).

**MANUAL: user to verify** — crispness (no bitmap scaling / blurring) at
zoom 0.6, 1.0, 1.37, 2.0. Not verifiable headlessly; the four jump keys above
make each checkpoint one keystroke away for a human running the binary.

---

## Step 2 — Zoomed PTY-dims formula + oracle comparison

**Formula used** (in `TerminalElement::prepaint`, matches
`Spike::resize`/`Spike::set_zoom`):

```rust
let cell_w = BASE_CELL_W * zoom;   // 7.5 * zoom
let cell_h = BASE_CELL_H * zoom;   // 17.0 * zoom
let cols = (bounds.size.width  / cell_w ).floor().max(1.0) as usize;
let rows = (bounds.size.height / cell_h).floor().max(1.0) as usize;
```

`bounds` here is the gpui element's *actual pixel bounds* after layout — i.e.
the physical size gpui already resolved for the content scope, which itself
shrank because the rems-styled sidebar (`rems(220/16)`) grew via
`set_rem_size`. This is the gpui-native equivalent of the oracle's manual
"subtract visible chrome" step: in gpui the chrome subtraction happens
automatically during layout (flex row: sidebar + `flex_1()` content), so the
element only ever sees its own post-layout bounds. Resize (`Spike::resize` ->
`master.resize(PtySize{ pixel_width: cols*cell_w, ... })` and
`term.lock().resize(...)`) is called from `prepaint` every frame the size
changed, i.e. on every zoom step (rem change triggers relayout) and on window
resize.

**Oracle reimplementation** (verbatim port of `compute_pty_dims`,
`src/gui/metrics.rs:265-295`, constants copied from the same file — lines
11-56 for `SIDEBAR_MIN_W=220.0`, `APPBAR_H=44.0`, `STATUS_H=26.0`,
`SESSBAR_H=36.0`, `SIDEBAR_DIVIDER_W=6.0`, `PTY_PAD_W=36.0`, `PTY_PAD_H=28.0`,
`CELL_W=7.5`, `CELL_H=17.0`):

```rust
fn oracle_compute_pty_dims(win_w, win_h, zoom, sidebar_w) -> (rows, cols) {
    let logical_w = win_w / zoom;
    let logical_h = win_h / zoom;
    let usable_w = logical_w - (sidebar_w + SIDEBAR_DIVIDER_W + PTY_PAD_W);
    let usable_h = logical_h - (APPBAR_H + STATUS_H + SESSBAR_H + PTY_PAD_H);
    cols = (usable_w / CELL_W).max(10.0) as u16;
    rows = (usable_h / CELL_H).max(4.0) as u16;
}
```

The gpui side was reimplemented standalone (not read from live layout) purely
for the side-by-side comparison table, using the same chrome constants scaled
by `zoom` instead of the window divided by `zoom`:

```rust
fn gpui_compute_pty_dims(win_w, win_h, zoom, sidebar_w) -> (rows, cols) {
    let cell_w_z = CELL_W * zoom;
    let cell_h_z = CELL_H * zoom;
    let usable_w = win_w - (sidebar_w + SIDEBAR_DIVIDER_W + PTY_PAD_W) * zoom;
    let usable_h = win_h - (APPBAR_H + STATUS_H + SESSBAR_H + PTY_PAD_H) * zoom;
    cols = (usable_w / cell_w_z).max(10.0) as u16;
    rows = (usable_h / cell_h_z).max(4.0) as u16;
}
```

### Formula-difference derivation (as requested by Task 4)

iced divides the window by zoom to get a smaller logical viewport; gpui grows
every chrome/cell pixel dimension by zoom and leaves the physical window size
fixed. These are algebraically the same relation:

```
oracle: cols = (win_w/zoom - chrome) / CELL_W
gpui:   cols = (win_w - chrome*zoom) / (CELL_W*zoom)
            = (win_w/zoom - chrome) / CELL_W      [divide num & denom by zoom]
```

So they must agree exactly modulo where the `as u16` truncation happens
(oracle truncates once, at the end, in logical units; gpui truncates once, at
the end, in physical-units-over-physical-cell-size — same single truncation
point, so no divergence should appear from that either).

### Oracle-comparison table (actual captured output)

Logical window 1280x800, sidebar 220 (== `SIDEBAR_MIN_W`, chrome always
visible in this spike):

| win      | zoom | oracle (rows,cols) | gpui (rows,cols) | result |
|----------|------|---------------------|-------------------|--------|
| 1280x800 | 1.0  | (39, 135)           | (39, 135)         | PASS   |
| 1280x800 | 1.4  | (25, 86)            | (25, 86)          | PASS   |
| 1280x800 | 2.0  | (15, 50)            | (15, 50)          | PASS   |

**Rounding divergence: none observed.** All three zoom levels matched
exactly. This is expected from the algebraic identity above; the `.max(10.0)`
/`.max(4.0)` floors did not trigger at these window sizes/zooms (values stay
well above the floors), so that edge case is untested here — worth a note for
Plan 03: if a floor ever triggers on one side and not the other (extreme
small-window + high-zoom combos), the two formulas could diverge at the
`.max()` clamp even though the pre-clamp math agrees. Not observed at any of
the 3 required comparison points.

---

## Step 3 — Run capture

`cargo run -p spike-zoom` under the live Wayland display (`wayland-1`):
window opened, ran to completion with no panics, full oracle-comparison table
and 17-step zoom sweep logged (both reproduced above verbatim from the
captured run).

---

## Summary for Plan 03

- Rem API: `Window::set_rem_size(px(16.0 * zoom))` (global) is sufficient;
  `Window::with_rem_size` exists for scoped overrides if a future need
  arises. No `WithRemSize` element exists at this gpui rev.
- Zoomed PTY-dims formula: multiply `CELL_W`/`CELL_H` by zoom, divide the
  element's post-layout pixel bounds by the zoomed cell size, floor. Let
  gpui's own flex layout do the chrome subtraction (don't hand-roll it like
  iced does) — it falls out for free once chrome is rems-styled.
  Practical implication: **Plan 03 doesn't need `compute_pty_dims`'s chrome
  arithmetic at all** for the gpui rewrite — only the cell-size-times-zoom
  half survives; the chrome-subtraction half is superseded by gpui layout.
  Keep `compute_pty_dims` only as the oracle for this one-time proof.
- MANUAL items outstanding: crispness verification at zoom 0.6/1.0/1.37/2.0
  (jump keys `ctrl+shift+1..4` implemented for this).
