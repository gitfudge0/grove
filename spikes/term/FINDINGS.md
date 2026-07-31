# Spike S1 — gpui TerminalElement end-to-end

Scope: Plan 01 Task 2, steps 1–5. Code: `spikes/term/src/main.rs` (single file).
Measured on this machine (Linux, Wayland `wayland-1`, X11 `:1` also present),
toolchain 1.95.0.

Revs actually built:
- `gpui` / `gpui_platform` — zed rev `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba`
- `alacritty_terminal` — zed fork rev `4c129667ce56611becdc82de6e28218c80e2e88f`, version `0.26.1-dev`
- `vte` 0.15.0 (transitive, re-exported as `alacritty_terminal::vte`)
- `portable-pty` 0.9.0

Build: `cd spikes && cargo build -p spike-term` → `Finished` (debug and `--release` both green).

---

## Dependency note (blocking, must carry into Plan 03)

`gpui` alone is **not** enough to start an app. `Application::run` needs a
`Platform`, and its constructor lives in a *separate* crate in the zed repo:

```rust
use gpui_platform::application;          // crate `gpui_platform`
application().with_assets(Assets).run(|cx: &mut App| { ... });
```

So the workspace needs a second git dependency:

```toml
gpui_platform = { git = "https://github.com/zed-industries/zed", rev = "<ZED_REV>",
                  features = ["font-kit", "wayland", "x11"] }
```

`gpui_platform`'s default feature set is empty — without `wayland`/`x11` the
Linux platform has no backend. `spikes/term/Cargo.toml` also adds `futures` 0.3
(the PTY→foreground channel; gpui re-exports none) and `anyhow` (the
`AssetSource` return type; `gpui::Result` is a re-export of `anyhow::Result`).

---

## Step 1 — Font registration and cell metrics

**Record — exact API names/signatures used** (Plan 04 copies verbatim):

```rust
// gpui/src/assets.rs
pub trait AssetSource: 'static + Send + Sync {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>>;
    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>>;
}
// gpui/src/app.rs
impl Application { pub fn with_assets(self, asset_source: impl AssetSource) -> Self }
// gpui/src/text_system.rs
impl TextSystem {
    pub fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()>;
    pub fn all_font_names(&self) -> Vec<String>;
    pub fn shape_line(&self, text: SharedString, font_size: Pixels,
                      runs: &[TextRun], force_width: Option<Pixels>) -> ShapedLine;
}
pub fn font(family: impl Into<SharedString>) -> Font;   // + Font::bold()
// gpui/src/text_system/line.rs
impl ShapedLine {
    pub fn width(&self) -> Pixels;
    pub fn paint(&self, origin: Point<Pixels>, line_height: Pixels, align: TextAlign,
                 align_width: Option<Pixels>, window: &mut Window, cx: &mut App) -> Result<()>;
}
// gpui/src/window.rs
pub fn fill(bounds: impl Into<Bounds<Pixels>>, background: impl Into<Background>) -> PaintQuad;
impl Window { pub fn paint_quad(&mut self, quad: PaintQuad); }
```

`add_fonts` is called on `cx.text_system()` (an `App` method) *before* opening
the window; the bytes are pulled through the same `AssetSource` given to
`with_assets`.

**Record — font family name**: `fc-scan` and `TextSystem::all_font_names()`
agree: **`BlexMono Nerd Font Mono`**. After `add_fonts`, `all_font_names()`
contains it (`= true`).

**Record — measured advance** (`shape_line("M", px(pt), &[run], None).width()`):

| pt   | advance    |
|------|------------|
| 11.0 | 6.6000004  |
| 11.5 | 6.9        |
| 12.0 | 7.2000003  |
| **12.5** | **7.5000005** |
| 13.0 | 7.8        |
| 13.5 | 8.1        |
| 14.0 | 8.400001   |

`"MMMMMMMMMM"` @ 12.5pt = `75.00001px` → 7.500001 per cell: the advance is
exactly uniform, no per-glyph rounding drift over a 10-cell run. **The spec's
7.5px CELL_W at 12.5pt reproduces exactly** (advance = font_size × 0.6). Plan 03
may hardcode `CELL_W = 7.5`; cheap insurance is deriving it from one
`shape_line("M")` at startup.

Other window defaults (do **not** use them for the grid): `window.line_height()
= 26px`, `window.rem_size() = 16px`. CELL_H = 17.0 is a Grove constant and must
be passed explicitly to `ShapedLine::paint`.

**Record — CJK / Nerd-glyph fallback** (no `mono_covers`-style hack, plain
`shape_line` on the bundled family):

| text | width | in 7.5px cells |
|---|---|---|
| `漢字` | 20px | 2.667 |
| `\u{f07b}\u{e0b0}` (nerd folder + powerline) | 15.000001px | 2.0 |
| `→│` | 15.000001px | 2.0 |
| `█▀` | 15.000001px | 2.0 |

Nerd-font glyphs, box-drawing and arrows all shape at exactly 1 cell out of the
bundled font — **no fallback hack needed**, unlike the iced build. CJK falls
back to a system CJK face at 10px/glyph, i.e. **1.333 cells, not 2** — a wide
char under-fills its two-cell slot. Mitigation already in the spike: text is
painted as *per-run* shaped lines anchored at `col * CELL_W`, so a width
mismatch inside one run can never drift the next run; the artifact is confined
to the wide glyph. `shape_line`'s `force_width: Option<Pixels>` is the obvious
fix if needed (untested).

---

## Step 2 — PTY + alacritty grid + reflow

Wiring: `portable_pty::native_pty_system().openpty(PtySize{..})` →
`CommandBuilder::new("tmux").args(["new-session","-A","-s","grove-spike"])` →
`pair.slave.spawn_command(cmd)`; `pair.master.take_writer()` /
`try_clone_reader()`. Reader is a plain `std::thread` doing blocking `read()`
into an unbounded `futures` channel. Parsing is
`Processor::<StdSyncHandler>::new().advance(&mut *term, &chunk)` with
`Term::new(Config { scrolling_history: 5000, ..Default::default() }, &size,
Listener)`, `Term` behind `alacritty_terminal::sync::FairMutex`. `TermSize`
implements `alacritty_terminal::grid::Dimensions`
(`total_lines`/`screen_lines`/`columns`).

**Record — reflow verdict.** Probe: three 60-char lines into a 40-col × 6-row
`Term`, then `term.resize(20 cols)`, dump the viewport.

Primary screen:
```
before(40): ["AAAA…40", "AAAA…20", "BBBB…40", "BBBB…20", "CCCC…40", "CCCC…20"]
after (20): ["BBBB…20", "BBBB…20", "BBBB…20", "CCCC…20", "CCCC…20", "CCCC…20"]
occupied rows 6 -> 9 ;  REFLOWED = true
```
Alternate screen (`\x1b[?1049h` first):
```
after (20): ["AAAA…20", "AAAA…20", "BBBB…20", "BBBB…20", "CCCC…20", "CCCC…20"]
occupied rows 6 -> 6 ;  REFLOWED = false
```

**There is no config knob.** `Term::resize` hardcodes the reflow flag from the
screen mode (`alacritty_terminal/src/term/mod.rs:677`):

```rust
let is_alt = self.mode.contains(TermMode::ALT_SCREEN);
self.grid.resize(!is_alt, num_lines, num_cols);
self.inactive_grid.resize(is_alt, num_lines, num_cols);
```

i.e. **the active grid reflows iff it is the primary screen**; the alternate
screen never reflows. `Config` exposes only `scrolling_history`,
`default_cursor_style`, `vi_mode_cursor_style`, `semantic_escape_chars`,
`kitty_keyboard`, `osc52` — nothing about reflow.

Consequence for Grove: a **non-issue in practice**, because every Grove agent
session runs inside tmux and tmux puts itself on the alternate screen, so the
grid Grove renders is the non-reflowing one. Reflow is only reachable for a
session attached to a bare shell on the primary screen. Options if that ever
matters: (a) accept reflow, (b) vendor/patch `alacritty_terminal` to thread a
`reflow: bool` through `Term::resize`, (c) never shrink the column count. No
user decision needed for the tmux path.

---

## Step 3 — Custom `Element`

`TerminalElement` with `RequestLayoutState = ()` and
`PrepaintState { quads, runs, cursor }`. Trait shape required at this rev (note
the two extra params vs older gpui):

```rust
impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;
    fn id(&self) -> Option<ElementId>;
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>>;
    fn request_layout(&mut self, Option<&GlobalElementId>, Option<&InspectorElementId>,
                      &mut Window, &mut App) -> (LayoutId, ());
    fn prepaint(&mut self, .., bounds: Bounds<Pixels>, &mut (), &mut Window, &mut App)
        -> PrepaintState;
    fn paint(&mut self, .., bounds, &mut (), &mut PrepaintState, &mut Window, &mut App);
}
// plus: impl IntoElement for TerminalElement { type Element = Self; fn into_element(self) -> Self }
```
`request_layout` uses `Style::default()` with
`style.size.{width,height} = relative(1.).into()` and
`window.request_layout(style, [], cx)`.

Paint order: one full-bounds background quad, then merged per-row background
quads, then shaped runs, then the cursor quad. All shaping happens in
`prepaint` (it needs `&mut Window`).

- **Merged bg quads**: adjacent cells with equal `Option<Hsla>` background
  coalesce into one `fill(Bounds…)`. Default-background cells emit no quad.
- **Text runs**: adjacent non-blank cells with equal `(fg, bold)` coalesce into
  one `shape_line`, painted at `origin.x + col * 7.5`. Blanks are skipped, so a
  mostly-whitespace screen costs almost nothing.
- **Bold** is `gpui::font(FAMILY).bold()` — the bundled `-Bold.ttf` is selected
  by weight from the same family, no separate family name.
- **Cursor**: `fill(Bounds::new(point(col*7.5, row*17), size(7.5, 17)), fg)`,
  gated on `TermMode::SHOW_CURSOR` and the blink phase, positioned with
  `grid.cursor.point.line + grid.display_offset()` so it stays put when
  scrolled back.
- **Colors**: `ansi_idx()` is a line-for-line port of `src/gui/pty.rs:390-421`
  against the hardcoded TokyoNight defaults from
  `crates/grove-core/src/theme.rs` (`bg 1a1b26`, `fg c0caf5`, `comment 565f89`,
  `blue 7aa2f7`, `cyan 7dcfff`, `magenta bb9af7`, `green 9ece6a`,
  `yellow e0af68`, `red f7768e`), with `bg_strip = bg mixed 32 % toward black`
  per `src/gui/palette.rs`. `vte::ansi::Color::{Named,Indexed,Spec}` maps onto
  it; `NamedColor::Foreground/Background` are the "default" cases and
  correspond to `vt100::Color::Default` in the iced code. `Flags::INVERSE`
  swaps fg/bg; `Flags::WIDE_CHAR_SPACER` cells are skipped.

**Record — visual verdict vs real Grove: MANUAL (see end).**

---

## Step 4 — Input path

All handlers hang off the wrapping `div()` (simpler than `insert_hitbox` +
`window.on_mouse_event`, and it gets focus for free):

```rust
div().track_focus(&self.focus).key_context("Terminal")
     .on_key_down(cx.listener(Self::on_key))
     .on_scroll_wheel(cx.listener(Self::on_scroll))
     .on_mouse_down(MouseButton::Left, cx.listener(..))   // &MouseDownEvent { position, button, modifiers, .. }
     .on_mouse_move(cx.listener(..))                      // &MouseMoveEvent
     .on_mouse_up(MouseButton::Left, cx.listener(..))     // &MouseUpEvent
     .child(TerminalElement { state: cx.entity() })
```

**Keys** — `KeyDownEvent { keystroke: Keystroke { modifiers: Modifiers {
control, alt, shift, platform, function }, key: String, key_char: Option<String>
}, is_held, .. }`. Shape difference from iced: gpui gives a *named* key string
(`"enter"`, `"escape"`, `"pageup"`, `"left"`…) plus a separate `key_char`
holding what would have been typed. The spike's table: enter→`\r`, tab→`\t` /
shift-tab→`\x1b[Z`, backspace→`\x7f`, escape→`\x1b`, delete→`\x1b[3~`,
home→`\x1b[H`, end→`\x1b[F`, pageup/pagedown→`\x1b[5~`/`[6~`, arrows→`\x1b[A..D`
with the CSI modifier form `\x1b[1;{1+shift+2*alt+4*ctrl}{A-D}` when modified,
ctrl-space→NUL, ctrl-letter→`c-'a'+1`, ctrl-`[`/`\`/`]`→`\x1b`/`\x1c`/`\x1d`,
alt-<char>→ESC prefix, otherwise the `key_char` bytes. `modifiers.platform`
(super on Linux) is dropped so app chords never reach the PTY — the natural
place for the Grove keymap carve-outs.

**Scroll** — `ScrollWheelEvent { delta: ScrollDelta, position, modifiers,
touch_phase }` with exactly two variants at this rev:
```rust
pub enum ScrollDelta { Pixels(Point<Pixels>), Lines(Point<f32>) }
```
The handler implements the 17.0px accumulation for `Pixels` (`accum += y;
n = trunc(accum / 17.0); accum -= n * 17.0`) and passes `Lines` straight through
(resetting the accumulator so a device switch cannot leak a partial line).
Scrolling on the alternate screen emits N × `\x1b[A`/`\x1b[B` to the PTY; on the
primary screen it calls
`term.scroll_display(alacritty_terminal::grid::Scroll::Delta(n))`. Every event
logs `S1: ScrollDelta::Pixels(...)` / `::Lines(...)`.
**Record — delta variants observed per device: MANUAL** (no interactive input
was possible in this session; the logging is in place and prints on first scroll).

**Mouse** — `cell_at()` divides the event position by (7.5, 17.0) and clamps.
SGR encode, gated on `term.mode().contains(TermMode::SGR_MOUSE)`:
`\x1b[<{btn};{col+1};{row+1}M` on press / `m` on release, `btn = 0` for left,
`32` for left-drag motion.

---

## Step 5 — Damage-driven repaint and idle cost

**Record — notify/timer pattern used.** Blocking PTY reader `std::thread` →
`futures::channel::mpsc::unbounded::<Vec<u8>>` → one foreground
`cx.spawn(async move |this: WeakEntity<Spike>, cx| { while let Some(chunk) =
rx.next().await { .. } })`. Inside the loop, under the `FairMutex`:

```rust
proc.advance(&mut *t, &chunk);
let dirty = match t.damage() {
    TermDamage::Full => true,
    TermDamage::Partial(mut it) => it.next().is_some(),
};
t.reset_damage();
if dirty { this.update(cx, |this, cx| cx.notify())?; }
```

`Term::damage(&mut self) -> TermDamage<'_>` and `Term::reset_damage(&mut self)`.
Because the channel is a real async stream, the task parks with zero wakeups
when the PTY is silent — no polling timer anywhere on the data path.

Cursor blink is a second `cx.spawn` loop:
`cx.background_executor().timer(Duration::from_millis(533)).await;` then flip a
bool and `cx.notify()`. `Task`s are kept alive in the entity
(`_tasks: Vec<Task<()>>`).

**Record — idle CPU, 60s windows, window open and unfocused**, from
`/proc/<pid>/stat` utime+stime deltas (`pidstat`/`sysstat` is not installed on
this box; the /proc delta is the same measurement):

| build | %CPU over 60s |
|---|---|
| spike-term, debug, blink on | **5.37 %** |
| spike-term, release, blink on | **1.23 %** |
| spike-term, release, `SPIKE_NO_BLINK=1` | **0.00 %** |
| real Grove (`~/.local/bin/grove`, release), same windows | **3.85 % / 3.55 %** |

The damage-driven data path costs literally nothing when idle; the entire
1.23 % is the 533ms cursor blink repainting a 120×34 grid — still ~⅓ of running
iced Grove's idle draw. **Idle cost is comfortably ≤ Grove.** If it ever
matters, the blink can repaint only the cursor bounds or stop while unfocused.

---

## PASS / FAIL

| criterion | verdict | evidence |
|---|---|---|
| Metrics reproducible (7.5 × 17 @ 12.5pt) | **PASS** | advance = 7.5000005px, uniform over 10 cells |
| Reflow suppressible | **PASS (conditionally)** | no config knob, but the alt screen never reflows and tmux is always on the alt screen |
| Glyph fallback OK without a cmap hack | **PASS for Nerd/box/arrows, PARTIAL for CJK** | nerd + box glyphs = exactly 1 cell; CJK = 1.33 cells, under-fills its 2-cell slot |
| Damage-driven repaint viable | **PASS** | `Term::damage`/`reset_damage` + channel task; 0.00 % idle with blink off |
| Idle cost ≈ Grove | **PASS (better)** | 1.23 % vs Grove 3.55 % |
| gpui app bootstrap from a plain git dep | **PASS with a caveat** | needs the extra `gpui_platform` crate + `wayland`/`x11` features |

Overall S1 verdict: **GO.**

---

## MANUAL — user to verify

1. **Side-by-side visual verdict vs real Grove** — glyph alignment on a real
   `claude` session, bold weight, background-quad seams, cursor size/position.
   Run `cd spikes && cargo run --release -p spike-term` next to a real Grove
   window (the spike attaches to tmux session `grove-spike`).
2. **CJK eyeballing** — a wide char shapes to 10px inside a 15px two-cell slot.
   Decide whether the gap is acceptable or whether Plan 04 needs the
   `force_width: Some(px(15.))` path.
3. **`ScrollDelta` variants per device** — scroll with a trackpad and with a
   wheel mouse, read the `S1: ScrollDelta::…` lines on stderr; confirm trackpad
   yields `Pixels` and wheel yields `Lines`.
4. **Whether the 17.0px accumulation feels identical to Grove** — subjective;
   the arithmetic is a direct port.
5. **Mouse reporting inside tmux** — click and drag to select in a tmux pane
   with `mouse on` and confirm the SGR sequences land.
