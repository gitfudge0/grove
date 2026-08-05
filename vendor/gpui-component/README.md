# Vendored `gpui-component`

This directory is a **plain copy** of a subset of
[`longbridge/gpui-component`](https://github.com/longbridge/gpui-component).
It is not a fork and it is not a submodule. Nothing here is "upstream" — treat
it as frozen third-party source.

| | |
|---|---|
| Upstream | `https://github.com/longbridge/gpui-component` |
| Rev (GPUI_COMPONENT_REV) | `88f102d13654fe25aa2fede076274b6b751a3704` |
| Upstream version | `gpui-component` 0.5.2, `gpui-component-macros` 0.5.1, `gpui-component-assets` 0.5.1 |
| License | Apache-2.0 — see `LICENSE-APACHE` beside this file |
| Consumed as | path dependency, workspace-**excluded** (root `Cargo.toml` `[workspace] exclude`) |

## How the copy was made

```bash
cd /path/to/grove
SRC=~/.cargo/git/checkouts/gpui-component-95ce574d8a0da8b8/88f102d
mkdir -p vendor/gpui-component
cp -r "$SRC/crates/ui"      vendor/gpui-component/ui
cp -r "$SRC/crates/macros"  vendor/gpui-component/macros
cp -r "$SRC/crates/assets"  vendor/gpui-component/assets
cp    "$SRC/LICENSE-APACHE" vendor/gpui-component/LICENSE-APACHE
find vendor/gpui-component \( -name '*.rs.orig' -o \( -name target -type d \) \) -print0 | xargs -0 -r rm -rf
```

If that checkout is gone, re-fetch it by adding the git dependency at
GPUI_COMPONENT_REV once, building, then copying. **Never** copy from a
different rev.

## Why only three crates

Upstream's workspace has six crates. Only three are reachable from
`gpui_component::input`, which is all Grove uses:

- **`ui`** — the monolith holding `Input`/`InputState`/the editor. It cannot be
  trimmed further: the input lives in the same crate as every other component
  and has no feature gate. Its `[features]` set has **no `default` key**, so
  tree-sitter, decimal, inspector and the 30+ grammar deps stay off. We enable
  **nothing**.
- **`macros`** (`gpui-component-macros`) — a `[dependencies]` entry of `ui`.
- **`assets`** (`gpui-component-assets`) — a `[dependencies]` entry of `ui`;
  its `assets/` folder of icon SVGs is embedded via `rust-embed` in `build.rs`,
  and it publishes the icons path to `ui`'s proc macro through cargo's
  `links = "gpui-component-default-icons"` / `DEP_*` bridge.

`story`, `story-web`, `webview` and every `examples/*` are **not** copied: they
pull `reqwest`, `wasm-bindgen` and a WebView stack Grove must never link.

## Why vendoring, not `[patch]` and not a fork

Upstream pins `gpui` with **no rev**, so it floats onto zed's default branch.
Grove pins ZED_REV exactly. A `[patch."https://github.com/zed-industries/zed"]`
entry **cannot** redirect a same-source git dependency to a different rev of
that same source (spike findings §S2 "Build note", amendment 2) — the spike only
worked by patching to a local path under `~/.cargo/git/checkouts`, which is
garbage-collectable and therefore not durable. A fork is outward-facing (a
second repo to own) and unavailable to an offline build. Vendoring is the only
option that is durable, offline and reviewable.

## The tree is unmodified except for these manifest edits

The Rust source under `ui/src`, `macros/src`, `assets/src`, the `build.rs`
files, `ui/locales` and `assets/assets` are **byte-identical to the rev above**.
Only the three `Cargo.toml` files were edited, as follows. **Any future edit to
the vendored source must be appended to this list or it is invisible.**

Applied to all three manifests:

1. `edition.workspace = true` → `edition = "2024"` (upstream's
   `[workspace.package] edition`). rustc 1.95 supports it; Grove's own crates
   stay on 2021.
2. Every `<dep>.workspace = true` resolved to the concrete spec from upstream's
   root `[workspace.dependencies]` (`anyhow = "1"`, `notify = "7.0.0"`,
   `ropey = "=2.0.0-beta.1"` + its two features, `rust-i18n = "4.2.0"`,
   `schemars = "1"`, `serde = "1.0.219"` + derive, `serde_json = "1"`,
   `serde_repr = "0.1"`, `smallvec = "1"`, `sum-tree = "0.2.0"`
   (package `zed-sum-tree`), `tracing = "0.1.41"`, `log = "0.4"`,
   `lsp-types = "0.97.0"` + `proposed`, `smol = "2"`,
   `raw-window-handle = "0.6.2"`, `windows = "0.58.0"` with upstream's
   workspace features `Wdk`/`Wdk_System`/`Wdk_System_SystemServices` merged
   into the per-crate feature list).
3. **`gpui` and `gpui_macros` re-pointed at Grove's pinned ZED_REV**
   (`git = "https://github.com/zed-industries/zed", rev = "1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba"`).
   This is the entire point of the exercise: exactly one `gpui` in the graph.
4. `gpui-component-macros` / `gpui-component-assets` re-pointed at the sibling
   vendored paths (`../macros`, `../assets`).
5. Dropped `publish = true`, `[lints] workspace = true` and
   `[package.metadata.cargo-machete]` — there is no enclosing workspace here,
   and vendored code must not inherit Grove's lints.

Applied to `ui/Cargo.toml` only:

6. `readme = "../../README.md"` commented out — the path is outside the
   vendored subset.

Applied to `assets/Cargo.toml` only:

7. The `[target.'cfg(target_family = "wasm")'.dependencies]` section (`reqwest`
   = zed's `reqwest` fork, `wasm-bindgen`, `wasm-bindgen-futures`) was
   **removed**. Cargo resolves target-gated deps for *every* target, so keeping
   it would drag the zed `reqwest` git fork into `Cargo.lock`. Grove never
   builds for wasm.

Applied to `ui/src` (source, not manifests):

8. **`Root` is optional.** Grove uses its own `Workspace` as the window root
   view — mounting `ui::Root` would bind `ctrl-c` to `Root`'s `Copy` action in
   the `"Root"` context and shadow the PTY's Ctrl+C for every terminal
   (`src/app.rs`). `Input` nevertheless reached `Root::read`/`Root::update`,
   both of which `.expect(...)` a mounted `Root`, so opening any modal with a
   text field panicked. Added `Root::try_read` / `Root::try_update`
   (`ui/src/root.rs`) — the same lookup via `window.root::<Root>()`, returning
   `None` instead of panicking — and switched the three `Input`-reachable call
   sites to them: the focused-input set and its `on_next_frame` reset
   (`ui/src/input/element.rs`) and the blur path (`ui/src/input/state.rs`).
   The panicking `Root::read`/`Root::update` are unchanged, as are the
   `Root`-only paths (dialogs, sheets, notifications, the window text-selection
   controller) that can only run once a `Root` is actually mounted.
9. **`grove-test-support`, an opt-in no-op for the native content-type sync.**
   Rendering an `Input` calls `sync_native_content_type`
   (`ui/src/input/content_type.rs`), which on macOS reaches
   `native::set_text_content_type` → `native::macos::ns_view`
   (`ui/src/input/native.rs`). `ns_view` is already defensive — it does
   `HasWindowHandle::window_handle(window).ok()?` — but gpui's test platform
   window answers that call with `unimplemented!("Test Windows are not backed
   by a real platform window")` (`gpui/src/platform/test/window.rs:47`), so it
   panics instead of yielding the `Err` the `.ok()?` exists to absorb. Every
   `#[gpui::test]` that renders a modal with a text field therefore aborted,
   and gpui exposes no public "am I on the test platform?" predicate to branch
   on at runtime. Added a pure marker feature `grove-test-support` to
   `ui/Cargo.toml` (it enables no dependencies) and gated the body of
   `sync_native_content_type` on it: with the feature on, the macOS branch is
   `cfg`-ed out and the existing `let _ = (window, content_type);` idiom
   absorbs the arguments so neither `cfg` combination warns. The `disabled`
   early return and the macOS/non-macOS split are otherwise untouched. Grove
   turns the feature on only through a `[dev-dependencies]` entry in the root
   `Cargo.toml`; edition 2021 selects feature resolver v2, which does not
   unify dev-dependency features into a normal `cargo build`, so the release
   binary still compiles and runs the real AppKit path.
