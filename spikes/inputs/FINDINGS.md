# Spike S2 — gpui-component text inputs

Source: `spikes/inputs/src/main.rs`. Locked revs: `gpui` @ `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba`
(zed-industries/zed), `gpui-component` @ `88f102d13654fe25aa2fede076274b6b751a3704`
(longbridge/gpui-component). Verified by grepping both checkouts under
`~/.cargo/git/checkouts/`, not from memory.

Evidence basis per row: **code** = read the gpui-component source directly;
**run** = observed from `eprintln!` instrumentation running the binary under a
live Wayland/X11 display (`cargo run -p spike-inputs`, headless-timeout kill);
**MANUAL** = requires a human driving mouse/keyboard, not drivable from this
harness.

## Step 1 — single-line input (palette search)

| Behavior | Result | Evidence |
|---|---|---|
| Focus on open | PASS | run — `[search] Focus` printed after `InputState::focus()` called in `Spike::new`; confirmed via `cx.subscribe(&search, ...)` on `InputEvent::Focus`. |
| Escape reaches an app-level handler while focused | PASS (code) / MANUAL (interactive) | code — `InputState::escape()` (`crates/ui/src/input/state.rs:1666`) calls `cx.propagate()` unless `clean_on_escape` is set or a context menu consumed it (neither applies here); an app-level `KeyBinding::new("escape", AppEscape, None)` therefore receives the action after the Input's `"Input"`-context binding declines it. This matches Grove's `should_forward` Escape carve-out. Actually pressing Escape in the live window is MANUAL. |
| ←/→ usable for app navigation when input is empty vs. editing | **FAIL** (no built-in distinction) | code — `movement.rs:139-154`: `left()`/`right()` unconditionally call `self.move_to(...)`, never `cx.propagate()`, regardless of whether the input is empty or the cursor is at a boundary. `KeyBinding::new("left", MoveLeft, Some("Input"))` / `"right"` are hard-bound in the `"Input"` context (`state.rs:180-181`). There is no dynamic `key_context()` (it's the fixed string `"Input"`, see `input.rs:400`, `search.rs:474`) that could be scoped to "empty" vs "editing". **Consequence: while the search Input is focused, Left/Right can never reach an app-level nav handler, empty or not** — Grove's palette would need to either (a) not focus the Input until the user types, (b) intercept Left/Right at a window-capture-phase before dispatch reaches the Input, or (c) patch gpui-component's `movement.rs` to propagate when at a boundary and the input is empty. |
| Cmd/Ctrl-chords do not insert characters | PASS (code) | code — `state.rs:216-225`: `cmd-c`/`ctrl-c` → `Copy`, `cmd-x`/`ctrl-x` → `Cut`, `cmd-v`/`ctrl-v` → `Paste`, `cmd-a`/`ctrl-a` → `SelectAll` are registered as `KeyBinding`s in the `"Input"` context, which GPUI's keymap dispatch resolves *before* falling through to IME/text-insertion handling — the chord is consumed as an action, never delivered as inserted text. Interactive confirmation is MANUAL. |
| Move-cursor-to-end API exists | PASS | run — used `InputState::set_selected_range(len..len, cx)` (public, `state.rs:2152`) after `set_value(...)`; observed stderr: `[app] moved search cursor to end via set_selected_range(18..18), cursor_position=Position { line: 0, character: 18 }`. There is also a dedicated `MoveToEnd` action (`state.rs:107`) if a keystroke-shaped API is preferred; `set_cursor_position(Position, window, cx)` (`state.rs:1236`) is a third equivalent option. |
| IME composition (accented char via compose key) | MANUAL | Explicitly out of scope for a harness with no real keyboard/compose-key driver, per task instructions. |
| Clipboard cut/copy/paste | PASS (code) / MANUAL (interactive) | code — `Copy`/`Cut`/`Paste` handlers (`state.rs:2040-2064`) call `cx.write_to_clipboard(ClipboardItem::new_string(..))` / `cx.read_from_clipboard()` — GPUI's own OS clipboard API, no `arboard` needed for this path. Interactive verification (paste in/out of a real terminal) is MANUAL. |

## Step 2 — three multiline editors (scripts-editor shape)

| Behavior | Result | Evidence |
|---|---|---|
| Independent instantiation / render, 3-up layout | PASS | run — binary builds and runs three `InputState::new(window, cx).multi_line(true).rows(6)` entities side by side in an `h_flex()`, mirroring `src/gui/scripts_editor.rs:31-33`'s three `text_editor::Content` buffers; no panics on open. |
| Tab focus traversal between the three editors | **FAIL as configured** | code — `IndentInline` is bound to plain `"tab"` in the `"Input"` context (`state.rs:184`). Its handler `indent_inline()` → `indent()` (`indent.rs:219-252`) only calls `cx.propagate()` when `!self.mode.is_indentable()`; `is_indentable()` (`indent.rs:57-64`) returns `true` whenever `multi_line` is set. **So a focused multiline `Input` always consumes Tab as an indent keystroke and it never reaches GPUI's built-in tab-stop focus traversal** (`Input::tab_index(..)` / `FocusHandle::tab_stop(true)`, confirmed present at `state.rs:464`, does exist and *would* work for single-line inputs, or once Tab is otherwise unconsumed). For three multiline scripts editors, Tab-to-next-field needs either: hand-rolled interception (capture Tab at a wrapping element before it reaches the Input when e.g. Escape-like carve-out logic decides it should be a focus-move), or accept Shift+Tab-style / click-only traversal. |
| Click-to-focus traversal | PASS (code) | code — `InputState`'s `focus_handle` is a normal GPUI `FocusHandle` with `tab_stop(true)` (`state.rs:464`) and the `Input` element is a standard interactive/stateful div; clicking any editor focuses it via GPUI's normal hit-testing + focus request path (same mechanism every other GPUI focusable widget uses). No special-casing needed. Interactive click-through is MANUAL. |
| Independent scroll per editor | PASS (code) | code — each `InputState` owns its own scroll offset (`scroll_offset()`/`set_scroll_offset()`, `state.rs:2125-2136`) and its own `EditorScrollbar`; there is no shared/global scroll state between instances. Visual confirmation while resizing/scrolling each pane is MANUAL. |
| Multi-line paste | PASS (code) | code — `paste()` (`state.rs:2060-2064`) reads `cx.read_from_clipboard()` text verbatim (including embedded `\n`/`\r\n`) and inserts it via the same `replace`/`insert` path used for typed text — no special single-line stripping is applied for `multi_line` inputs. Interactive paste-from-OS-clipboard is MANUAL. |
| Select-all / copy | PASS (code) | code — `SelectAll` (`cmd-a`/`ctrl-a`) and `Copy` (`cmd-c`/`ctrl-c`) are ordinary `"Input"`-context bindings, identical mechanism to the single-line case above; nothing multiline-specific changes their behavior. Interactive verification is MANUAL. |

## Build note (environment, not app code)

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

## API names used (for Plan 08)

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

## Recommendation

**gpui-component**, with two amendments the real implementation must account for (Plan 08):

1. Empty-vs-editing Left/Right app navigation while the palette search Input is focused is **not achievable out of the box** — `MoveLeft`/`MoveRight` are unconditionally consumed inside `"Input"` context regardless of cursor position or text emptiness, and the context string is static (no predicate hook). Plan 08 needs one of: don't focus the Input until first keystroke, capture-phase Left/Right interception ahead of GPUI's normal dispatch, or a small upstream/vendored patch to `crates/ui/src/input/movement.rs` to propagate at-boundary-and-empty.
2. Tab-to-next-editor traversal across the three scripts-editor panes is **not achievable via Tab** once `multi_line(true)` is set — Tab always indents. Click-to-focus works natively and costs nothing; if keyboard traversal is required, it needs a hand-rolled non-Tab chord (e.g. `ctrl-tab`) wired at the app level, not gpui-component's built-in tab-stop mechanism.

Everything else — focus-on-open, the Escape-propagates-to-app-handler contract, Cmd/Ctrl chords not inserting characters, a move-cursor-to-end API, clipboard cut/copy/paste via GPUI's own clipboard (no `arboard` needed), multi-line paste, select-all/copy, and independent per-instance scroll — is present and matches or exceeds what Grove's iced widgets do today. Hand-rolling text input/editing from scratch (cursor math, selection, IME, clipboard, undo/redo, syntax highlighting hooks) to only then still need the two patches above is a much larger surface than living with the two documented gaps.
