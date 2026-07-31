# gpui Rewrite Plan 03: App shell — entities/globals, theme, fonts, zoom, keymap skeleton, AnimationClock, storage

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. This is **production** code, not a spike: the workspace clippy denies apply (`unwrap_used`/`expect_used`), superpowers:test-driven-development governs every pure helper (tests before implementation, red before green), and superpowers:verification-before-completion governs every "done" claim — read raw command output, never a summary line. Also load the `gpui-development` skill before writing any gpui code; training-data gpui is stale and this rev is pinned.

**Goal:** Land `crates/grove-gpui` — the first gpui binary in the main workspace — booting to a themed, empty-but-real Grove window. It owns: the `gpui_platform::application()` bootstrap, the embedded-asset/font pipeline with the **startup cell-metric assertion**, the theme Global (grove-core `Theme` → gpui colors, the full token vocabulary ported from `src/gui/palette.rs`, follow-system + startup resolution order), the settings/storage Global over grove-core's existing `storage::Store`, the `AnimationClock` entity replacing `blink_tick`, zoom via `Window::set_rem_size`, and an `actions!`/`KeyBinding` skeleton generated from a ported `SHORTCUTS` registry. Exit gate: **shell opens, themed, metric assertion passes**; `./install.sh` green; the iced app still builds and is untouched; one commit.

No terminal grid, no sidebar tree, no modals, no PTYs. Chrome regions are themed placeholder rectangles with the correct dimensions — they become real in Plans 04-07.

**Architecture:**

```
crates/grove-gpui/
  Cargo.toml         member of the MAIN workspace but NOT in default-members (toolchain
                     split — see Global Constraints). deps: gpui + gpui_platform (pinned
                     rev), grove-core (path), rust-embed, futures, anyhow, tracing.
                     gpui-component is FORBIDDEN in this phase.
  src/main.rs        gpui_platform::application().with_assets(Assets).run(..); login-PATH
                     recovery, panic hook + telemetry parity stub, window options
                     (1280x800, title "grove"), close-request hookup point (Plan 09).
  src/app.rs         root wiring: startup sequence + global installation order
  src/assets.rs      rust-embed AssetSource over assets/ (fonts + icon SVGs)
  src/fonts.rs       add_fonts + CELL_W/CELL_H/FONT_SIZE constants + the startup metric
                     assertion (the exit gate) + GROVE_GPUI_SELFTEST harness hook
  src/theme.rs       ThemeState global: grove-core Theme -> gpui Hsla, the fn-per-token
                     palette port, follow-system resolution, invalidation generation
  src/settings.rs    SettingsState global over grove_core::storage::Store + the 250ms
                     debounced zoom persist
  src/zoom.rs        zoom state, clamp/step table, set_rem_size idiom, zoomed cell metrics
  src/keymap.rs      SHORTCUTS registry (ported) -> actions! + Vec<KeyBinding>; key contexts
  src/entities/
    animation_clock.rs  AnimationClock entity: adaptive 60ms/1s background_executor timer,
                        monotonic tick counter, derived phase accessors
  src/views/
    workspace.rs     root view: flex row (sidebar placeholder | content column of
                     appbar/body/statusbar placeholders), all rems-styled
```

`crates/grove-core` is consumed **unchanged** — no new storage format, no new theme format, no edits to that crate. `crates/grove-terminal` is *not* a dependency yet (Plan 04 adds it). `src/` (the iced app) is read-only reference material in this phase and must keep building.

**Tech Stack:**
- `gpui` = `{ git = "https://github.com/zed-industries/zed", rev = "1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba" }`
- `gpui_platform` = same repo/rev, `features = ["font-kit", "wayland", "x11"]` (its own defaults are **empty** — without these the Linux platform has no backend; findings amendment 1)
- `rust-embed` 8 (AssetSource backing), `futures` 0.3 (gpui re-exports none), `anyhow`, `tracing`
- rustc **1.95.0** via user-local rustup (`PATH="$HOME/.cargo/bin:$PATH"`); the rest of the workspace stays on the system default (1.94.1)

## Global Constraints

- Branch: `gpui-rewrite` (already exists; do not create a new one).
- **Pins are law.** ZED_REV `1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba`. Record it once in the root `Cargo.toml` `[workspace.dependencies]` by exact rev; never a branch, tag, or version range. Never bump mid-phase.
- **gpui-component is FORBIDDEN in this phase.** The shell skeleton has no text inputs (they arrive in Plan 08), so the durable-pin question from findings amendment 2 (gpui-component floats `gpui` with no rev onto zed's default branch; the spikes worked around it with a garbage-collectable `[patch."https://github.com/zed-industries/zed"]`) is **deferred again — the fork-or-vendor decision lands in Plan 08.** Do **not** add `gpui-component` and do **not** add any `[patch]` section to the main workspace. If a resolution error mentioning an unpinned `zed-industries/zed` appears, STOP: it means gpui-component leaked in.
- **Toolchain split (read this twice).** zed at ZED_REV needs `std::hint::cold_path` → rustc ≥ 1.95. The machine default is 1.94.1. Therefore:
  - Do **NOT** add a root `rust-toolchain.toml`. Pinning the whole product's toolchain is a user-level decision, already declined.
  - `crates/grove-gpui` is a workspace **member** (so it shares `Cargo.lock` and `[workspace.lints]`) but is **excluded from `default-members`**, so bare `cargo build` / `cargo test` on 1.94.1 keeps working for `grove`, `grove-core`, `grove-terminal`.
  - grove-gpui is always built explicitly with the rustup toolchain:
    ```bash
    PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 build -p grove-gpui
    ```
  - **`cargo build --workspace` is retired for the duration of the branch.** `--workspace` means *all members* by definition and cannot be made to skip one; it will try grove-gpui and fail on 1.94.1. Every `--workspace` invocation in `.github/workflows/ci.yml` (lines ~60, 65, 75, 79) must be rewritten to the explicit member list `-p grove -p grove-core -p grove-terminal`. This is a required step (Task 1), not an optional cleanup. Report it to the orchestrator if CI's shape has drifted from those four lines.
- **The iced app is untouched.** No edits under `src/` in this phase (the one exception is *reading* it). `crates/grove-core` gets no edits either. If you find yourself wanting to refactor grove-core to share code with grove-gpui, STOP and report — the spec says grove-core is reused *unchanged*, so grove-gpui **copies** the iced-side glue it needs (e.g. `migrate_stale_theme_names`) rather than hoisting it.
- Behavior questions are answered by reading the iced code, never by guessing. Canonical references for this phase: `src/gui/mod.rs:50-89` (window options, font registration, title, 1280×800, exit-on-close-request), `src/app/mod.rs:176-215` (startup order: stale-attention GC → storage load → custom-theme load → stale-name migration → follow-system resolution), `src/app/theme_picker.rs:210-217` (`apply_system_theme`), `src/gui/palette.rs` (the whole token vocabulary), `src/gui/metrics.rs:7-70` (constants + `pty_metrics`), `src/gui/update/mod.rs:52-56,368-390,420-435` (60ms/1s adaptive tick + its gating, `ZOOM_SAVE_QUIET_TICKS`), `src/gui/update/shortcuts.rs:104-175+` (`ShortcutDef`/`Scope`/`SHORTCUTS`).
- **Carried spike amendments (do not re-derive):**
  1. Bootstrap is `gpui_platform::application()`, **not** `gpui::Application::new()` (does not exist at this rev).
  2. `AssetSource` / `add_fonts` / `shape_line` signatures are copied verbatim from findings §S1 Step 1 — reproduced in Task 2.
  3. CELL_W is **exactly 7.5** at 12.5pt on `BlexMono Nerd Font Mono` (measured 7.5000005; advance == font_size × 0.6, uniform over a 10-cell run). CELL_H = 17.0 is a Grove constant, not a font metric — `window.line_height()` (26px) and `window.rem_size()` (16px) defaults must **never** be used for the grid.
  4. Zoom is `Window::set_rem_size(px(16.0 * zoom))`. **`WithRemSize` does not exist at this rev** — if you find a doc or example using it, it is stale. `Window::with_rem_size` exists for scoped overrides.
  5. **`compute_pty_dims`'s chrome-subtraction arithmetic is superseded** by gpui layout (findings amendment 7). Only the `CELL_W * zoom` / `CELL_H * zoom` half survives. Do **not** port `src/gui/metrics.rs:265-295` into grove-gpui.
  6. No `mono_covers`-style cmap hack is needed: Nerd/box/arrow glyphs shape at exactly 1 cell out of the bundled font. CJK under-fills (1.333 cells) — a **Plan 04** painting concern, out of scope here.
- Quality bars: `cargo +1.95.0 clippy -p grove-gpui --all-targets -- -D warnings` clean under the workspace deny (production paths; `#[cfg(test)]` may unwrap freely). `rustfmt --edition 2021` on **touched files only**, never crate-wide.
- No `git` commands until Task 7. Do not commit intermediate tasks.

---

### Task 1: Crate scaffold, workspace membership, and the toolchain split

**Files:**
- Create: `crates/grove-gpui/Cargo.toml`, `crates/grove-gpui/src/main.rs` (stub)
- Modify: root `Cargo.toml` (`members`, `default-members`, `[workspace.dependencies]`), `.github/workflows/ci.yml`

**Interfaces:**
- Produces: a buildable (window-less) `grove-gpui` binary on rustc 1.95.0, the pinned `gpui`/`gpui_platform` workspace dependencies every later task consumes, and a workspace whose default build is unchanged on 1.94.1.

- [ ] **Step 1: Confirm the toolchain exists before touching anything**

```bash
rustc --version                                        # expect 1.94.1 (system default)
PATH="$HOME/.cargo/bin:$PATH" rustup toolchain list    # expect a 1.95.0 entry
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 --version
```

If 1.95.0 is absent, STOP AND REPORT — do not install toolchains unprompted.

- [ ] **Step 2: Workspace membership + default-members**

Root `Cargo.toml`:

```toml
[workspace]
members = ["crates/grove-core", "crates/grove-terminal", "crates/grove-gpui"]
# grove-gpui needs rustc >= 1.95 (zed at ZED_REV uses std::hint::cold_path); the
# system default is 1.94.1. Excluding it from default-members keeps bare
# `cargo build`/`cargo test` working for the iced app and the core crates.
# Build it explicitly: `cargo +1.95.0 build -p grove-gpui`.
default-members = [".", "crates/grove-core", "crates/grove-terminal"]
```

And under `[workspace.dependencies]`, beside the existing alacritty pin:

```toml
# Pinned by exact rev (gpui rewrite plan 03): never a branch or version range.
gpui = { git = "https://github.com/zed-industries/zed", rev = "1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba" }
# gpui_platform's own default features are EMPTY — without wayland/x11 the Linux
# platform has no backend. Bootstrap lives here, not in gpui.
gpui_platform = { git = "https://github.com/zed-industries/zed", rev = "1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba", features = ["font-kit", "wayland", "x11"] }
rust-embed = "8"
futures = "0.3"
```

- [ ] **Step 3: Write `crates/grove-gpui/Cargo.toml`**

Mirror `crates/grove-terminal/Cargo.toml`'s conventions exactly (`publish = false`, `version.workspace`, `edition.workspace`, `license.workspace`, `[lints] workspace = true`). Note it must **not** inherit `rust-version.workspace = true` (that says 1.94, which is false for this crate) — set `rust-version = "1.95"` literally, with a comment pointing at this plan.

```toml
[[bin]]
name = "grove-gpui"
path = "src/main.rs"

[dependencies]
grove-core = { path = "../grove-core" }
gpui.workspace = true
gpui_platform.workspace = true
rust-embed.workspace = true
futures.workspace = true
anyhow.workspace = true
tracing.workspace = true
# NO gpui-component in this phase — see Global Constraints (durable-pin decision
# is deferred to Plan 08). NO grove-terminal yet — Plan 04 adds it.
```

- [ ] **Step 4: Stub `src/main.rs` and prove the bootstrap compiles**

```rust
//! Grove's gpui shell. Bootstrap is `gpui_platform::application()` — `gpui`
//! alone has no `Platform` constructor at this rev (spike findings §S1).
#![forbid(unsafe_code)]

fn main() {
    gpui_platform::application().run(|_cx: &mut gpui::App| {});
}
```

```bash
cd /home/gitfudge/dev/gitfudge0/grove
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 build -p grove-gpui 2>&1 | tail -20
```

Expected: `Finished`. First build pulls the zed git checkout and is slow (several minutes) — that is normal, do not interrupt it.

If the resolver reports an **unpinned** `zed-industries/zed` source, a gpui-component-shaped dependency leaked in: STOP and report.

- [ ] **Step 5: Prove the default workspace is unharmed**

```bash
rustc --version                          # still the 1.94.1 default
cargo build 2>&1 | tail -5               # default-members only — must Finish
cargo test 2>&1 | tail -5                # iced app + core + terminal, unaffected
```

Expected: both `Finished`/all-green, and **no** attempt to compile gpui in the output. If gpui appears in a bare `cargo build`, `default-members` is wrong — fix it before proceeding.

- [ ] **Step 6: Fix CI's `--workspace` invocations**

`--workspace` includes non-default members, so CI would build gpui (and needs the 1.95 toolchain plus Linux GUI system deps it does not install). Rewrite each `--workspace` in `.github/workflows/ci.yml` to the explicit list:

```yaml
cargo clippy -p grove -p grove-core -p grove-terminal --all-targets -- -D warnings -A clippy::unwrap_used -A clippy::expect_used
cargo clippy -p grove -p grove-core -p grove-terminal -- -D warnings
cargo nextest run -p grove -p grove-core -p grove-terminal --locked --profile ci
cargo test --doc -p grove -p grove-core -p grove-terminal --locked
```

Add a comment above the first one naming this plan and saying grove-gpui is CI-excluded until Plan 10 (when iced is deleted and grove-gpui becomes the product). **Do not** add a grove-gpui CI job in this phase.

```bash
grep -n -- "--workspace" .github/workflows/ci.yml    # expect no output
```

---

### Task 2: Assets, fonts, and the startup metric assertion (the exit gate)

**Files:**
- Create: `crates/grove-gpui/src/assets.rs`, `crates/grove-gpui/src/fonts.rs`
- Modify: `crates/grove-gpui/src/main.rs`

**Interfaces:**
- Produces: `Assets` (an `AssetSource` over the existing `assets/` tree), `fonts::register(cx)`, the constants `CELL_W = 7.5` / `CELL_H = 17.0` / `FONT_SIZE = 12.5` / `MONO_FAMILY` / `UI_FAMILY`, and `fonts::assert_cell_metrics(cx) -> Result<f32, MetricError>` — **the exit-gate assertion**.

- [ ] **Step 1: TDD the pure part first**

The measurement needs a live gpui `App`; the *comparison* does not. Write, in `src/fonts.rs`, tests before implementation:

```rust
/// Epsilon is 0.001 px: the spike measured 7.5000005 at 12.5pt, so float noise
/// is ~5e-7 while a genuinely wrong font/size is off by >= 0.3px per cell.
pub const CELL_W_EPSILON: f32 = 0.001;
pub fn metric_ok(measured: f32) -> bool;
```

Tests: `metric_ok(7.5)`, `metric_ok(7.5000005)`, `!metric_ok(7.2)` (12pt — the nearest wrong size), `!metric_ok(7.8)` (13pt), `!metric_ok(0.0)` (font missing entirely). Run red, then implement:

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -20
```

- [ ] **Step 2: `src/assets.rs` — the AssetSource**

`rust-embed` over the repo's existing `assets/` directory (`assets/fonts/*.ttf` already exist: `BlexMonoNerdFontMono-Regular.ttf`, `BlexMonoNerdFontMono-Bold.ttf`, `IBMPlexSans-Regular.ttf`, `IBMPlexSans-Bold.ttf`). Implement the trait with the **verbatim** signatures from findings §S1:

```rust
// gpui/src/assets.rs
pub trait AssetSource: 'static + Send + Sync {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>>;
    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>>;
}
```

`load` returns `Ok(None)` for a miss (not an `Err`). No `unwrap`/`expect`. The SVG icon set (`src/gui/icons.rs`'s generated single-color SVGs) is **not** wired here — leave a `// Plan 06` comment; only fonts are needed for the exit gate.

- [ ] **Step 3: `src/fonts.rs` — registration**

```rust
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
```

`cx.text_system().add_fonts(..)` is called **before the window opens**, with bytes pulled through the same `Assets` handed to `with_assets`. Register all four TTFs. Constants (copy the values and the rationale comment from `src/gui/metrics.rs:24-32`):

```rust
pub const CELL_W: f32 = 7.5;      // measured advance at FONT_SIZE; == FONT_SIZE * 0.6
pub const CELL_H: f32 = 17.0;     // a GROVE constant, NOT a font metric — never use
                                  // window.line_height() (26px) or rem_size() (16px)
pub const FONT_SIZE: f32 = 12.5;
pub const MONO_FAMILY: &str = "BlexMono Nerd Font Mono";  // fc-scan and all_font_names agree
pub const UI_FAMILY: &str = "IBM Plex Sans";
```

- [ ] **Step 4: The assertion itself**

```rust
/// Measures the em advance of the bundled mono font and fails loudly if it is
/// not exactly CELL_W. The grid maps (row, col) directly to pixels, so a wrong
/// advance silently drifts the cursor across long rows — mirrors the iced-side
/// test at src/gui/metrics.rs:388-392, but at RUNTIME, because gpui's text
/// system only exists inside a live App.
pub fn assert_cell_metrics(cx: &mut gpui::App) -> Result<f32, MetricError>;
```

Implementation: `cx.text_system().shape_line("M".into(), px(FONT_SIZE), &[run], None).width()`, where `run` is a `TextRun` over the `MONO_FAMILY` font. Also assert `all_font_names()` contains `MONO_FAMILY` and `UI_FAMILY` — a missing family produces a *fallback* measurement, which is the failure mode most likely to look plausible. Return a `thiserror`-free small enum or `anyhow::Error` carrying both the expected and measured values plus the family list on failure.

Failure is **fatal and loud**: log at `tracing::error!` with the measured value, then `std::process::exit(1)` before the window opens. A shell that renders a subtly-misaligned grid is worse than one that refuses to start.

- [ ] **Step 5: The headless self-test hook**

So the exit gate is checkable without a human eyeballing a window, `main` honours an env var: when `GROVE_GPUI_SELFTEST=1`, print one line and quit **after** the assertion and **before** opening the window:

```
GROVE_GPUI_SELFTEST: cell_w=7.5000005 cell_h=17 font_size=12.5 family="BlexMono Nerd Font Mono" OK
```

- [ ] **Step 6: Verify**

```bash
cd /home/gitfudge/dev/gitfudge0/grove
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 build -p grove-gpui 2>&1 | tail -5
GROVE_GPUI_SELFTEST=1 PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run -p grove-gpui 2>&1 | tail -5
```

Expected: the `OK` line with `cell_w` within 0.001 of 7.5, exit status 0. Paste the raw line into your report — this is the plan's exit gate and a summary is not acceptable evidence.

---

### Task 3: The theme Global — grove-core `Theme` → gpui colors

**Files:**
- Create: `crates/grove-gpui/src/theme.rs`
- Modify: `crates/grove-gpui/src/main.rs` (module wiring)

**Interfaces:**
- Produces: `ThemeState` (a gpui `Global`) exposing the full fn-per-token vocabulary as `gpui::Hsla`, `ThemeState::set_by_name`, `apply_system_theme`, and a `generation: u64` bumped on every change (Plan 04's element uses it as a repaint/cache key).

- [ ] **Step 1: Read the oracle before writing anything**

Read `src/gui/palette.rs` in full (231 lines) and `crates/grove-core/src/theme.rs:1-150`. The architecture to preserve exactly:
- grove-core owns ~40 runtime-derived themes (31 built-in + custom `themes.json`) as a flat `Theme { name, kind, bg, bg_highlight, fg, fg_dark, comment, blue, cyan, magenta, green, yellow, red, orange }` of `theme::Color::Rgb(u8,u8,u8)`.
- The GUI's richer surface vocabulary (`BG`, `BG_RAIL`, `BG_STRIP`, `BG_HOVER`, two border weights, `AMBER()` = yellow/red 75/25 mix, etc.) is **synthesized** by blending base colors at fixed ratios, with `is_dark_of(t)`-dependent ratios.
- Every token is a **function**, read fresh per call through `theme::with_current` (a per-thread snapshot behind a generation counter — an atomic load, not a lock), so a theme swap takes effect on the next frame with no invalidation bookkeeping.

- [ ] **Step 2: Port the token module**

Write `src/theme.rs` with **one function per token, same names, same blend ratios, same dark/light branches** as `palette.rs`, returning `gpui::Hsla` instead of `iced::Color`. Keep `#![allow(non_snake_case)]` and the SCREAMING_CASE token names — matching names are what make the Plan 04-07 ports mechanical and reviewable side-by-side. Port `ic()` (grove-core `Color` → gpui) and `mix()` first, then every token in `palette.rs` order.

Conversion goes through gpui's `Rgba { r, g, b, a }` → `Hsla` (`gpui::rgb`/`Rgba::into`), and **blending must happen in linear-ish RGB exactly as `palette.rs` does it** (component-wise lerp on 0..1 sRGB floats) — do **not** blend in HSL space, which would shift hues and visibly change ~40 themes at once. Convert to `Hsla` only at the end of each token function. Add a comment saying so.

- [ ] **Step 3: `ThemeState` as a Global**

```rust
pub struct ThemeState {
    pub follow_system: bool,
    pub dark_name: String,
    pub light_name: String,
    pub system_mode: WindowAppearance,   // seeded at startup, updated on observation
    pub generation: u64,
}
impl gpui::Global for ThemeState {}
```

The *colors* are not stored here — grove-core's `theme::ACTIVE` remains the single source of truth and the token functions read it, exactly as today. `ThemeState` holds only the resolution policy plus the generation counter. `set_by_name`/`apply_system_theme` call `grove_core::theme::set_by_name` and then bump `generation` + `cx.refresh()`.

Port `apply_system_theme` from `src/app/theme_picker.rs:210-217` verbatim in behavior. Follow-system observation uses gpui's window-appearance observation (`Window::appearance()` + the appearance-change observer) in place of `iced::system::theme_changes()`; seed it on the first frame so follow-system resolves to the real mode immediately rather than after the first OS notification (`src/gui/mod.rs:63-68` documents why this ordering matters).

- [ ] **Step 4: Tests**

Unit-test the pure helpers with `#[cfg(test)]` (no `App` needed): `mix` endpoints and midpoint; `ic()` round-trip for a few known RGB values; that `AMBER()` sits between yellow and red for the default theme; and that `BG_STRIP` is strictly darker than `BG_RAIL` which is darker than `BG` on the default dark theme (the ordering the chrome depends on). Default theme must resolve to **TokyoNight dark (#1a1b26)** — assert it.

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -20
```

---

### Task 4: Storage/settings Global and the startup sequence

**Files:**
- Create: `crates/grove-gpui/src/settings.rs`, `crates/grove-gpui/src/app.rs`
- Modify: `crates/grove-gpui/src/main.rs`

**Interfaces:**
- Produces: `SettingsState` (Global) wrapping `grove_core::storage::Store` with a debounced persist, and `app::boot(cx)` — the single ordered startup sequence every later plan appends to.

- [ ] **Step 1: `SettingsState`**

```rust
pub struct SettingsState { pub store: grove_core::storage::Store, dirty: bool }
impl gpui::Global for SettingsState {}
```

**No new storage format and no new file.** `grove_core::storage::{load, save, persist}` and `Store` are used exactly as the iced app uses them (`src/app/mod.rs:178`). Mutation goes through `SettingsState::update(cx, |s| ...)`, which marks dirty and schedules the debounced flush.

- [ ] **Step 2: The debounced persist**

The iced app debounces the zoom write by 4 × 60ms ticks (`ZOOM_SAVE_QUIET_TICKS`, `src/gui/update/mod.rs:52-56`). The spec §4 replaces this with a **250ms timer**. Implement one shared debounce on `SettingsState` (not zoom-specific): each `update` (re)arms a 250ms `cx.background_executor().timer(..)`; on fire, `storage::persist(&store)` off the foreground thread. Also expose `flush_now(cx)` — the synchronous flush every quit path must call (spec §7 `flush_ui_zoom_save`); wiring it into quit paths is **Plan 09**, but the method exists now with a doc comment naming its future callers.

- [ ] **Step 3: `app::boot(cx)` — the startup sequence, in this exact order**

Ported from `src/app/mod.rs:176-215`; read it before writing. Order is load-bearing and each step gets a comment saying why:

1. `grove_core::env_path::ensure_login_path()` — before anything spawns (`src/gui/mod.rs:52-54`).
2. `grove_core::attention::cleanup_stale_files()` — stale-file GC, before any session id is reused.
3. `storage::load()` → `Store`. Failure here is genuinely unrecoverable (no UI to report into); mirror the iced app's deliberate hard-fail, but as a `tracing::error!` + `exit(1)` rather than a panic — `expect_used` is denied and this is a production path.
4. `theme::load_custom()` — user themes must exist before a persisted custom name is resolved.
5. `migrate_stale_theme_names(&mut store)` — **copy** this one-time migration from `src/app/theme_picker.rs` into grove-gpui (grove-core stays unchanged per Global Constraints); if it mutated, `storage::persist`.
6. Resolve the active theme: if `store.theme_follow_system`, seed from `store.theme_dark` (falling back to the default dark theme) so the first frame has a concrete theme, then re-resolve from the real appearance once the window exists; else `set_by_name(store.theme)`.
7. Clamp zoom: `store.ui_zoom.unwrap_or(1.0).clamp(0.6, 2.0)`.
8. Install globals in dependency order: `SettingsState` → `ThemeState` → `ZoomState` → `AnimationClock` (Tasks 5-6 fill the last two in).

Telemetry (`app_launched`, heartbeat, panic-hook scrubbing — `src/main.rs:9-31`, `src/gui/update/mod.rs:101-118`) is **Plan 09**. Leave a single `// Plan 09: telemetry + panic hook` marker at the right spot in `boot`; do not port it now.

- [ ] **Step 4: Verify**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -20
GROVE_CONFIG_DIR=$(mktemp -d) GROVE_GPUI_SELFTEST=1 PATH="$HOME/.cargo/bin:$PATH" \
  cargo +1.95.0 run -p grove-gpui 2>&1 | tail -10
```

`GROVE_CONFIG_DIR` (`grove_core::storage::CONFIG_DIR_ENV`) points the run at a throwaway config so the self-test never mutates the developer's real `store.json`. Expected: the selftest `OK` line, exit 0, and a `store.json` written into the temp dir.

---

### Task 5: `AnimationClock` entity — the `blink_tick` replacement

**Files:**
- Create: `crates/grove-gpui/src/entities/animation_clock.rs`, `crates/grove-gpui/src/entities/mod.rs`

**Interfaces:**
- Produces: `AnimationClock` — a gpui `Entity` owning a monotonic `tick: u64` advanced by a `background_executor` timer at the adaptive 60ms/1s cadence, plus the derived phase accessors every animated view will read.

- [ ] **Step 1: Read the oracle**

`src/gui/update/mod.rs:368-390` (the subscription that picks 60ms vs 1s) and `:420-435` (the gating predicate). Reproduce the rule exactly: **60ms when `busy || (has_ptys && (focused || animating || dirty))`, else 1s** (spec §4). Getting this wrong is an idle-power regression, which the spikes measured as one of the port's wins (release idle 1.23% vs Grove's ~3.7%).

- [ ] **Step 2: The entity**

```rust
pub struct AnimationClock { tick: u64, fast: bool, _task: Task<()> }
impl AnimationClock {
    pub fn new(cx: &mut Context<Self>) -> Self;   // spawns the timer loop
    pub fn tick(&self) -> u64;
    /// Recomputes the cadence from the gating inputs; restarts the timer only
    /// when the cadence actually changes (restarting every frame would defeat
    /// the whole point of the slow lane).
    pub fn set_busy_inputs(&mut self, busy: bool, has_ptys: bool, focused: bool,
                           animating: bool, dirty: bool, cx: &mut Context<Self>);
}
```

The loop is `cx.spawn` + `cx.background_executor().timer(Duration::from_millis(60 | 1000))` in a loop, `cx.update(|this, cx| { this.tick += 1; cx.notify(); })` per beat (findings §S1 Step 5 pattern). Every observer repaints off `cx.notify()`; nothing polls the counter.

- [ ] **Step 3: Derived phases — one counter, preserved relationships**

All blink phases derive from this **single** counter so their phase relationships and the idle-power profile match today (spec §4). Port the exact arithmetic from the iced side and unit-test each as a pure function of `tick`:

- `cursor_visible(tick) = tick % 16 < 8` — at 60ms/beat this is the **533ms** cursor blink (16 × 60ms = 960ms period, 480ms on/off; keep the iced formula, do not re-derive from the 533ms figure — the *formula* is the parity contract, and `src/gui/state.rs:296` documents it).
- `dots(tick) = (tick / 5) % 3` — the 3-dot animation.
- `toast_pulse(tick) = tick % 40`.
- `spinner_frame(tick) = (tick / 3) % 12` — 12 pre-rotated frames advanced every 3 ticks.

The attention amber pulse (1s auto-reverse EaseInOut) and the onboarding entrance are **not** clock-derived — they map to gpui `with_animation` (spec §4). Add a doc comment saying so, so nobody wires them to the tick later.

- [ ] **Step 4: Tests + verify**

Pure-function tests over the four phase accessors (period, duty cycle, and that `cursor_visible` and `dots` stay in the phase relationship they have today at ticks 0..240). Plus one integration-shaped test that the cadence selector returns 60ms/1s for the full truth table of the five gating inputs.

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -20
```

---

### Task 6: Zoom and the `SHORTCUTS`-generated keymap skeleton

**Files:**
- Create: `crates/grove-gpui/src/zoom.rs`, `crates/grove-gpui/src/keymap.rs`

**Interfaces:**
- Produces: `ZoomState` (Global) + the `set_rem_size` application point; `actions!`-declared actions and a `Vec<KeyBinding>` generated from a ported `SHORTCUTS` registry, with key-context strings for `Workspace`/`Grid`/`Zen`.

- [ ] **Step 1: `ZoomState`**

```rust
pub struct ZoomState { pub zoom: f32 }   // clamp [0.6, 2.0], step 0.1 — src/gui/metrics.rs:46-49
impl ZoomState {
    pub fn cell_w(&self) -> f32 { fonts::CELL_W * self.zoom }
    pub fn cell_h(&self) -> f32 { fonts::CELL_H * self.zoom }
    pub fn font_size(&self) -> f32 { fonts::FONT_SIZE * self.zoom }
}
```

Applying zoom is **one call per frame in the root view's render**, verbatim from findings §S3:

```rust
window.set_rem_size(px(16.0 * zoom));
```

`WithRemSize` does not exist at this rev. All chrome is styled in `rems()` (e.g. a 220px sidebar is `rems(220.0 / 16.0)`) so it scales off that single call; the terminal content scope instead multiplies cell metrics by zoom in Rust (Plan 04 consumes `cell_w()`/`cell_h()`/`font_size()`).

**Do not port `compute_pty_dims`.** Its chrome-subtraction arithmetic is superseded by gpui layout (findings amendment 7): PTY dims are `(bounds.size.width / cell_w).floor().max(1.0)` over the element's own post-layout bounds, computed in Plan 04's `prepaint`. Record that formula as a doc comment on `cell_w()` so Plan 04 has it at hand.

- [ ] **Step 2: Zoom actions + persistence**

Actions `ZoomIn` / `ZoomOut` / `ZoomReset`, bound per platform (mod = Cmd on macOS, Ctrl+Shift elsewhere — the same `platform_mod_label` rule the registry already encodes). Each writes `ZoomState`, calls `SettingsState::update` (which arms the 250ms debounce from Task 4 Step 2), and `cx.refresh()`. Pinch-to-zoom is Plan 04 (it needs the terminal element's scroll handling).

Unit-test the clamp/step table: 1.0 → 9 steps down clamps at 0.6, 10 steps up clamps at 2.0, `ZoomReset` → 1.0, and that no step produces a value outside `[0.6, 2.0]`.

- [ ] **Step 3: Port the `SHORTCUTS` registry**

Copy `ShortcutDef` / `Scope` / `Screen` and the whole `SHORTCUTS` table from `src/gui/update/shortcuts.rs:104-...` into `src/keymap.rs`, **unchanged in content** — it stays the single source of truth for both bindings and (Plan 08's) overlay display, exactly as the spec §5 requires. Adapt only the mechanism: `triggers: &[&str]` (iced modifier-independent key names) become gpui keystroke strings.

- [ ] **Step 4: Generate actions and bindings**

Declare the action set with `actions!` (one action per `GlobalShortcut` variant), then write:

```rust
/// The registry is the only source of key bindings — a shortcut that exists in
/// SHORTCUTS but has no binding here is a bug, not a feature. Asserted below.
pub fn bindings() -> Vec<gpui::KeyBinding>;
```

driven by iterating `SHORTCUTS` and mapping `(triggers, requires_alt, literal, scopes)` → keystroke string + key context. Key contexts for this phase: `"Workspace"`, `"Grid"`, `"Zen"` (modal contexts arrive in Plan 08). `Scope::Global` → no context; `Scope::Screen(_)` → that screen's context string.

**Test:** every `SHORTCUTS` row with `action: Some(_)` produces at least one `KeyBinding`, and no two rows in the same context produce the same keystroke. This is the drift guard the spec's "cannot drift" claim rests on; it must be a real test, not a comment.

Handlers are **skeleton only**: each action's handler logs `tracing::debug!` and does nothing, except the three zoom actions, which are fully functional (they are part of the exit gate). Add a `// Plan NN` marker beside each stub naming the plan that implements it.

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -20
```

---

### Task 7: The shell view, verification, and phase close-out

**Files:**
- Create: `crates/grove-gpui/src/views/workspace.rs`, `crates/grove-gpui/src/views/mod.rs`
- Modify: `crates/grove-gpui/src/main.rs`, `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` (row 03 → done)

**Interfaces:**
- Produces: a real window rendering themed placeholder chrome at the right dimensions, all globals live, zoom keybindings working. Plans 04-07 replace the placeholders region by region.

- [ ] **Step 1: Window options**

In `main`, matching `src/gui/mod.rs:73-86` exactly: title `"grove"`, size 1280×800. `with_assets(Assets)`; `fonts::register` + `fonts::assert_cell_metrics` **before** `open_window`; `app::boot(cx)` before that. Close-request interception is Plan 09 — leave the hookup point with a comment, and let the window close normally for now.

- [ ] **Step 2: The root view**

`Workspace` entity implementing `Render`. `render` first calls `window.set_rem_size(px(16.0 * zoom))`, then builds a flex row:
- left: sidebar placeholder, `rems(320.0 / 16.0)` wide (`RAIL_W`, `src/gui/metrics.rs:9`), filled `BG_RAIL()`;
- a `rems(6.0 / 16.0)` divider filled with the border token (`SIDEBAR_DIVIDER_W`);
- right: `flex_1()` column of appbar `rems(44/16)` (`APPBAR_H`, `BG_STRIP()`), a `flex_1()` body filled `BG()` centered-texting the active theme name + current zoom + the live `AnimationClock` tick, and a statusbar `rems(26/16)` (`STATUS_H`, `BG_STRIP()`).

The body text exists purely so a human can see theme, zoom and clock all working in one glance — Plan 04 deletes it. Every dimension comes from a named constant with the `metrics.rs` line cited; no bare numbers.

The view observes `AnimationClock` (`cx.observe(..)`) so the tick display repaints, proving the clock drives repaints end to end.

- [ ] **Step 3: Full verification**

```bash
cd /home/gitfudge/dev/gitfudge0/grove
# grove-gpui (1.95 toolchain)
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 build -p grove-gpui 2>&1 | tail -5
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 test -p grove-gpui 2>&1 | tail -30
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 clippy -p grove-gpui --all-targets -- -D warnings 2>&1 | tail -20
GROVE_CONFIG_DIR=$(mktemp -d) GROVE_GPUI_SELFTEST=1 PATH="$HOME/.cargo/bin:$PATH" \
  cargo +1.95.0 run -p grove-gpui 2>&1 | tail -5
# the rest of the workspace, on the DEFAULT 1.94.1 toolchain — must be untouched
rustc --version
cargo build 2>&1 | tail -5
cargo test 2>&1 | tail -10
grep -rn "gpui" Cargo.toml crates/grove-core/Cargo.toml crates/grove-terminal/Cargo.toml
rustfmt --edition 2021 --check crates/grove-gpui/src/*.rs crates/grove-gpui/src/*/*.rs
```

Expected: grove-gpui builds/tests/clippy clean; the selftest prints `cell_w=7.5…  OK`; the default workspace builds and tests exactly as before; the `grep` shows gpui **only** in the root `[workspace.dependencies]` (never in grove-core or grove-terminal, which stay gpui-free per Plan 02). Read the raw output.

- [ ] **Step 4: MANUAL — the visual exit gate (human, on a real desktop)**

```bash
PATH="$HOME/.cargo/bin:$PATH" cargo +1.95.0 run --release -p grove-gpui
```

Checklist for the human — report each as pass/fail, do not claim these yourself:
1. A 1280×800 window titled **grove** opens (Wayland; then re-run with `WAYLAND_DISPLAY= DISPLAY=:1` for X11).
2. Chrome is **TokyoNight dark** by default: body `#1a1b26`, sidebar visibly darker, appbar/statusbar darker still.
3. The tick counter in the body is advancing (~1/s while unfocused, faster while focused).
4. Zoom in / zoom out / reset chords work; all chrome scales together; nothing clips or overlaps at 0.6 and at 2.0.
5. Quit, relaunch: the zoom level persisted (the 250ms debounce flushed).
6. Flip the OS appearance with follow-system enabled in `store.json` and confirm the chrome follows.

- [ ] **Step 5: `./install.sh`**

```bash
./install.sh 2>&1 | tail -20
```

Expected: release build + install of the **iced** `grove` binary succeeds (several minutes). Project rule; non-negotiable. Confirm the script did not attempt grove-gpui — if it uses `--workspace`, fix it the same way as CI (Task 1 Step 6) and re-run.

- [ ] **Step 6: Update the master plan and commit**

Mark row 03 `done` in `docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md` with a one-line note recording: the measured `cell_w` from the self-test, the `default-members` toolchain split (and that CI's `--workspace` invocations were rewritten), and that the **gpui-component durable-pin decision is deferred to Plan 08** (this phase needs no text inputs).

```bash
git add crates/grove-gpui Cargo.toml Cargo.lock .github/workflows/ci.yml \
        docs/superpowers/plans/2026-07-31-gpui-rewrite-00-master.md
git commit -m "feat(gpui): app shell — globals, theme, fonts, zoom, keymap skeleton, clock"
```

**Exit gate met when:** the shell opens themed on a real display, the startup metric assertion passes (`cell_w` within 0.001 of 7.5), zoom keybindings work and persist, `./install.sh` is green, the iced app and both existing crates are untouched and still build on the default toolchain, and no `gpui-component` dependency exists anywhere.
