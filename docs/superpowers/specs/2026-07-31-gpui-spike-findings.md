# GPUI rewrite — spike findings

Resolved dependency pins (lockstep rule: gpui-component's Cargo.lock pin for
`gpui` is authoritative for ZED_REV; zed's own workspace pin for
`alacritty_terminal` at that rev is authoritative for the terminal backend).

- GPUI_COMPONENT_REV: `88f102d13654fe25aa2fede076274b6b751a3704` (longbridge/gpui-component, HEAD at resolution time)
- ZED_REV: `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba` (zed-industries/zed — taken from gpui-component's Cargo.lock; gpui-component's Cargo.toml itself has no rev/branch pin, it floats on zed's default branch, so the lockfile is the only authoritative source)
- alacritty_terminal: `git = "https://github.com/zed-industries/alacritty", rev = "4c129667ce56611becdc82de6e28218c80e2e88f"` (from zed's root Cargo.toml workspace.dependencies at ZED_REV)
- portable-pty: `0.9`

> **Dependency-pin hazard (carry to Plan 02+):** gpui-component pins zed with no rev and drifted to an incompatible zed HEAD mid-spike; spikes work around it with a `[patch."https://github.com/zed-industries/zed"]` entry pointing gpui at the locked local cargo checkout. That path lives in `~/.cargo/git/checkouts` and can be garbage-collected — production phases need a durable pin (fork gpui-component with a pinned rev, or vendor).

Note: gpui's `[features]` default set at ZED_REV is
`["font-kit", "wayland", "x11", "windows-manifest"]`, so no extra Cargo
features were needed for spikes/term to compile the Linux platform backends.

## S1 Terminal element

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

### Dependency note (blocking, must carry into Plan 03)

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

### Step 1 — Font registration and cell metrics

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

### Step 2 — PTY + alacritty grid + reflow

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

### Step 3 — Custom `Element`

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

### Step 4 — Input path

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

### Step 5 — Damage-driven repaint and idle cost

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

### PASS / FAIL

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

### MANUAL — user to verify

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

## S2 Text inputs

Source: `spikes/inputs/src/main.rs`. Locked revs: `gpui` @ `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba`
(zed-industries/zed), `gpui-component` @ `88f102d13654fe25aa2fede076274b6b751a3704`
(longbridge/gpui-component). Verified by grepping both checkouts under
`~/.cargo/git/checkouts/`, not from memory.

Evidence basis per row: **code** = read the gpui-component source directly;
**run** = observed from `eprintln!` instrumentation running the binary under a
live Wayland/X11 display (`cargo run -p spike-inputs`, headless-timeout kill);
**MANUAL** = requires a human driving mouse/keyboard, not drivable from this
harness.

### Step 1 — single-line input (palette search)

| Behavior | Result | Evidence |
|---|---|---|
| Focus on open | PASS | run — `[search] Focus` printed after `InputState::focus()` called in `Spike::new`; confirmed via `cx.subscribe(&search, ...)` on `InputEvent::Focus`. |
| Escape reaches an app-level handler while focused | PASS (code) / MANUAL (interactive) | code — `InputState::escape()` (`crates/ui/src/input/state.rs:1666`) calls `cx.propagate()` unless `clean_on_escape` is set or a context menu consumed it (neither applies here); an app-level `KeyBinding::new("escape", AppEscape, None)` therefore receives the action after the Input's `"Input"`-context binding declines it. This matches Grove's `should_forward` Escape carve-out. Actually pressing Escape in the live window is MANUAL. |
| ←/→ usable for app navigation when input is empty vs. editing | **FAIL** (no built-in distinction) | code — `movement.rs:139-154`: `left()`/`right()` unconditionally call `self.move_to(...)`, never `cx.propagate()`, regardless of whether the input is empty or the cursor is at a boundary. `KeyBinding::new("left", MoveLeft, Some("Input"))` / `"right"` are hard-bound in the `"Input"` context (`state.rs:180-181`). There is no dynamic `key_context()` (it's the fixed string `"Input"`, see `input.rs:400`, `search.rs:474`) that could be scoped to "empty" vs "editing". **Consequence: while the search Input is focused, Left/Right can never reach an app-level nav handler, empty or not** — Grove's palette would need to either (a) not focus the Input until the user types, (b) intercept Left/Right at a window-capture-phase before dispatch reaches the Input, or (c) patch gpui-component's `movement.rs` to propagate when at a boundary and the input is empty. |
| Cmd/Ctrl-chords do not insert characters | PASS (code) | code — `state.rs:216-225`: `cmd-c`/`ctrl-c` → `Copy`, `cmd-x`/`ctrl-x` → `Cut`, `cmd-v`/`ctrl-v` → `Paste`, `cmd-a`/`ctrl-a` → `SelectAll` are registered as `KeyBinding`s in the `"Input"` context, which GPUI's keymap dispatch resolves *before* falling through to IME/text-insertion handling — the chord is consumed as an action, never delivered as inserted text. Interactive confirmation is MANUAL. |
| Move-cursor-to-end API exists | PASS | run — used `InputState::set_selected_range(len..len, cx)` (public, `state.rs:2152`) after `set_value(...)`; observed stderr: `[app] moved search cursor to end via set_selected_range(18..18), cursor_position=Position { line: 0, character: 18 }`. There is also a dedicated `MoveToEnd` action (`state.rs:107`) if a keystroke-shaped API is preferred; `set_cursor_position(Position, window, cx)` (`state.rs:1236`) is a third equivalent option. |
| IME composition (accented char via compose key) | MANUAL | Explicitly out of scope for a harness with no real keyboard/compose-key driver, per task instructions. |
| Clipboard cut/copy/paste | PASS (code) / MANUAL (interactive) | code — `Copy`/`Cut`/`Paste` handlers (`state.rs:2040-2064`) call `cx.write_to_clipboard(ClipboardItem::new_string(..))` / `cx.read_from_clipboard()` — GPUI's own OS clipboard API, no `arboard` needed for this path. Interactive verification (paste in/out of a real terminal) is MANUAL. |

### Step 2 — three multiline editors (scripts-editor shape)

| Behavior | Result | Evidence |
|---|---|---|
| Independent instantiation / render, 3-up layout | PASS | run — binary builds and runs three `InputState::new(window, cx).multi_line(true).rows(6)` entities side by side in an `h_flex()`, mirroring `src/gui/scripts_editor.rs:31-33`'s three `text_editor::Content` buffers; no panics on open. |
| Tab focus traversal between the three editors | **FAIL as configured** | code — `IndentInline` is bound to plain `"tab"` in the `"Input"` context (`state.rs:184`). Its handler `indent_inline()` → `indent()` (`indent.rs:219-252`) only calls `cx.propagate()` when `!self.mode.is_indentable()`; `is_indentable()` (`indent.rs:57-64`) returns `true` whenever `multi_line` is set. **So a focused multiline `Input` always consumes Tab as an indent keystroke and it never reaches GPUI's built-in tab-stop focus traversal** (`Input::tab_index(..)` / `FocusHandle::tab_stop(true)`, confirmed present at `state.rs:464`, does exist and *would* work for single-line inputs, or once Tab is otherwise unconsumed). For three multiline scripts editors, Tab-to-next-field needs either: hand-rolled interception (capture Tab at a wrapping element before it reaches the Input when e.g. Escape-like carve-out logic decides it should be a focus-move), or accept Shift+Tab-style / click-only traversal. |
| Click-to-focus traversal | PASS (code) | code — `InputState`'s `focus_handle` is a normal GPUI `FocusHandle` with `tab_stop(true)` (`state.rs:464`) and the `Input` element is a standard interactive/stateful div; clicking any editor focuses it via GPUI's normal hit-testing + focus request path (same mechanism every other GPUI focusable widget uses). No special-casing needed. Interactive click-through is MANUAL. |
| Independent scroll per editor | PASS (code) | code — each `InputState` owns its own scroll offset (`scroll_offset()`/`set_scroll_offset()`, `state.rs:2125-2136`) and its own `EditorScrollbar`; there is no shared/global scroll state between instances. Visual confirmation while resizing/scrolling each pane is MANUAL. |
| Multi-line paste | PASS (code) | code — `paste()` (`state.rs:2060-2064`) reads `cx.read_from_clipboard()` text verbatim (including embedded `\n`/`\r\n`) and inserts it via the same `replace`/`insert` path used for typed text — no special single-line stripping is applied for `multi_line` inputs. Interactive paste-from-OS-clipboard is MANUAL. |
| Select-all / copy | PASS (code) | code — `SelectAll` (`cmd-a`/`ctrl-a`) and `Copy` (`cmd-c`/`ctrl-c`) are ordinary `"Input"`-context bindings, identical mechanism to the single-line case above; nothing multiline-specific changes their behavior. Interactive verification is MANUAL. |

### Build note (environment, not app code)

`gpui-component` @ `88f102d13654fe25aa2fede076274b6b751a3704` depends on plain
`gpui = { git = "https://github.com/zed-industries/zed" }` **with no `rev`
pinned** in its own `Cargo.toml` (its checked-in `Cargo.lock` happens to
resolve that to `1a246efd7e...`, the same commit we pin, but that lockfile
isn't inherited by our workspace). Without intervention, our workspace
resolved that floating edge to zed's then-current HEAD (`ae394f3d...`), a
newer commit where `gpui`'s public API had drifted, producing:

```
error[E0432]: unresolved imports `gpui::AssetSource`, `gpui::Result`, `gpui::SharedString`
 --> .../gpui-component-.../crates/assets/src/native_assets.rs:2:12
```

Fix applied in `spikes/Cargo.toml` (workspace root, not inside `inputs/`): a
`[patch."https://github.com/zed-industries/zed"]` entry pinning `gpui` to a
local `path` dependency at our already-fetched
`~/.cargo/git/checkouts/zed-*/1a246ef*/crates/gpui` checkout, plus one
`cargo update -p 'gpui@0.0.0'` to drop the stale lockfile entry. This unifies
gpui-component's floating `gpui` edge with our own pinned rev so only one
`gpui` crate is built. Plan 08/09 consumers should carry this patch forward
verbatim (or replace it with an explicit `rev =` once gpui-component's own
manifest pins one) — without it, `spikes/inputs` does not build at all, and
any future crate pulling in `gpui-component` in this workspace will hit the
same wall.

Also needed and not previously in `spikes/inputs/Cargo.toml`: a direct
`gpui_platform = { workspace = true }` dependency — at this rev, bootstrapping
is `gpui_platform::application()` (returns a `gpui::Application`), not
`gpui::Application::new()` (that constructor doesn't exist at this rev; only
`Application::with_platform` / `Application::new_inaccessible` do). Every
gpui-component example bootstraps this way.

### API names used (for Plan 08)

- `gpui_component::input::{Input, InputState, InputEvent}`
- `InputState::new(window, cx).placeholder(..).multi_line(true).rows(n)`
- `InputState::focus(window, cx)`
- `InputState::value() -> SharedString`, `InputState::set_value(.., window, cx)`
- `InputState::set_selected_range(Range<usize>, cx)` — move-cursor-to-end idiom: `set_selected_range(len..len, cx)`
- `InputState::set_cursor_position(Position, window, cx)` / `InputState::cursor_position() -> Position` (`gpui_component::input::Position`, re-exported from `lsp_types`)
- `InputState::scroll_offset()` / `set_scroll_offset()` — per-instance, independent
- Actions (all in `gpui_component::input`, `actions!(input, [...])`, `state.rs:76-113`): `Escape`, `MoveLeft`, `MoveRight`, `MoveToStart`, `MoveToEnd`, `SelectAll`, `Copy`, `Cut`, `Paste`, `IndentInline` (bound to bare Tab, always consumes Tab in multiline mode)
- `Input::new(&state).appearance(bool).cleanable(bool).tab_index(isize)`
- `gpui_component::{init(cx), Root, v_flex, h_flex, ActiveTheme}`
- Bootstrap: `gpui_platform::application().run(|cx| { gpui_component::init(cx); ... })`

### Recommendation

**gpui-component**, with two amendments the real implementation must account for (Plan 08):

1. Empty-vs-editing Left/Right app navigation while the palette search Input is focused is **not achievable out of the box** — `MoveLeft`/`MoveRight` are unconditionally consumed inside `"Input"` context regardless of cursor position or text emptiness, and the context string is static (no predicate hook). Plan 08 needs one of: don't focus the Input until first keystroke, capture-phase Left/Right interception ahead of GPUI's normal dispatch, or a small upstream/vendored patch to `crates/ui/src/input/movement.rs` to propagate at-boundary-and-empty.
2. Tab-to-next-editor traversal across the three scripts-editor panes is **not achievable via Tab** once `multi_line(true)` is set — Tab always indents. Click-to-focus works natively and costs nothing; if keyboard traversal is required, it needs a hand-rolled non-Tab chord (e.g. `ctrl-tab`) wired at the app level, not gpui-component's built-in tab-stop mechanism.

Everything else — focus-on-open, the Escape-propagates-to-app-handler contract, Cmd/Ctrl chords not inserting characters, a move-cursor-to-end API, clipboard cut/copy/paste via GPUI's own clipboard (no `arboard` needed), multi-line paste, select-all/copy, and independent per-instance scroll — is present and matches or exceeds what Grove's iced widgets do today. Hand-rolling text input/editing from scratch (cursor math, selection, IME, clipboard, undo/redo, syntax highlighting hooks) to only then still need the two patches above is a much larger surface than living with the two documented gaps.

## S3 Zoom

## S4 Linux platform

Binary: `spikes/platform/src/main.rs` (`cargo run -p spike-platform`).
gpui rev: `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba` (checked out at
`~/.cargo/git/checkouts/zed-a70e2ad075855582/1a246ef`).

Test session: nested/headless Wayland compositor (`WAYLAND_DISPLAY=wayland-1`,
`DISPLAY=:1` via Xwayland, `XDG_SESSION_TYPE=wayland`), no physical display or
input device attached — so anything requiring a human click/drag is MANUAL.

### Verdict summary

| Item | Wayland | X11 | Notes |
|---|---|---|---|
| Window opens at 1280x800, title set | PASS | PASS | Confirmed via `window.window_bounds()` log both runs |
| Resize event logging | MANUAL | MANUAL | Not wired as a distinct hook this spike (see Deviations) |
| Focus/blur (window activation) events | PASS (fires) / MANUAL (verify semantics) | PASS (fires) / MANUAL | `observe_window_activation` fired twice in both runs with no user input — see Deviations |
| Close-request interception (should-close) | MANUAL | MANUAL | Code path implemented and correct API confirmed; needs a human to click close twice |
| File drag-drop delivered as native gpui event | MANUAL | MANUAL | `.on_drop::<ExternalPaths>()` registered on root div; needs a human to drag a file from a file manager |
| Clipboard write+read round-trip (automated, in-process) | **FAIL** (`None` both immediately and after 300ms post-focus) | PASS (immediate) | See Deviations — likely a Wayland focus/session-compositor artifact, not necessarily representative of a real desktop |
| Clipboard cross-app paste verification | MANUAL | MANUAL | Requires pasting into an external terminal; not automatable here |

### Exact APIs (for Plans 03/09 to copy)

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

### Deviations from the plan / things that didn't go as expected

1. **`Application::new()` doesn't exist at this gpui rev.** Assumed a simple constructor going in; this rev requires `gpui_platform::application()` (or `Application::with_platform(current_platform(headless))` directly). Added the `gpui_platform` crate as a workspace dependency (same git/rev pin) to both `spikes/Cargo.toml` and `spikes/platform/Cargo.toml`.
2. **Per-resize-event logging not wired as a separate handler.** Given the time budget, window-level resize logging was not added as a distinct hook (that lives on the lower-level `PlatformWindow` trait, `fn on_resize`, invoked internally by `Window::new`, not exposed as a simple public per-window registration the way should-close is). Window-level activation (`observe_window_activation`) *is* wired and logs to stderr — this covers the "focus/blur" need called out in the plan for attention acknowledge-on-refocus. Flagging resize-event logging as a follow-up rather than blocking the spike.
3. **Activation fired twice with zero user input**, in both Wayland and X11 runs. Likely: once for initial window-shown/mapped, once for actual compositor-granted keyboard focus. Not investigated further — recommend MANUAL verification that this maps cleanly onto the "focused session never shows WaitingForInput" acknowledge-on-refocus need from `src/attention.rs`.
4. **Automated clipboard round-trip failed on Wayland, passed on X11 (Xwayland), in this environment.** Sequence tried: (a) write+read immediately in the `Application::run` startup closure — `None` on Wayland, `Some(marker)` on X11; (b) write+read again ~300ms later, after two activation events had already fired (so focus should be established) — still `None` on Wayland. Root cause not fully isolated in the spike time budget; gpui's Wayland `write_to_clipboard` (`gpui_linux::linux::wayland::client`) gates on `mouse_focused_window.is_some() || keyboard_focused_window.is_some()` before calling `wl_data_device.set_selection`, so this reads like a focus-state or nested-compositor artifact of this sandboxed session (no real input device, no real window manager) rather than a proven gpui defect — but it needs MANUAL verification against a normal Wayland desktop session before Plans 03/09 rely on in-process clipboard for anything user-facing.
5. **No X11/Wayland input-injection tool was available** (`xdotool`/`wtype` not installed; only `ydotool`, which needs a running daemon) — could not automate the close-click, drag-drop, or a real keypress-driven "press c / press v" test. Worked around this for clipboard by adding a parallel **automated** self-test (write immediately at startup, then again via `cx.spawn` + a 300ms timer) directly in `Application::run`, in addition to the interactive `c`/`v` keybindings — so the API round-trip itself got exercised without needing key injection. Close-interception, drag-drop, and the interactive clipboard keys are implemented and logging is in place, but need a human to actually click/drag/type.

### Arboard verdict

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

### How to manually verify the rest

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

## Build status

Toolchain: zed at ZED_REV requires rustc 1.95.0 (`std::hint::cold_path`,
stabilized in 1.95); Arch's packaged rustc is 1.94.1, which fails with
E0658. Resolved by installing rustup **user-locally** (`~/.cargo`, no system
package change) and pinning `spikes/rust-toolchain.toml` to `1.95.0`.
`cd spikes && cargo build` → `Finished` — all four spike binaries build,
including gpui + alacritty_terminal for spike-term. gpui's default features
already include `font-kit`/`wayland`/`x11`; nothing extra needed.

## Go/No-go
