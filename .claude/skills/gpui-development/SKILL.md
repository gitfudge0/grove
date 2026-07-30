---
name: gpui-development
description: Use when writing, reviewing, or planning gpui code (Zed's Rust UI framework) — app bootstrap, Entity/Render, actions/keymaps, focus, custom Elements, async, text inputs, zoom, Linux/macOS platform behavior, or pinning the gpui dependency. Also use before answering any gpui API question from memory — training-data gpui is stale.
---

# gpui Development

## Overview

gpui is pre-1.0 with frequent breaking changes; the source IS the documentation. Rule zero: **never trust memory of the API — verify against the pinned rev.** Zed's repo (`crates/gpui/examples/`, `docs/key_dispatch.md`, `src/_ownership_and_data_flow.rs`, and production crates like `terminal_view`) is the reference.

Verified code patterns with sources: see [patterns.md](patterns.md).

## Corrections to stale training-data knowledge (as of mid-2026)

| You probably believe | Current reality |
|---|---|
| Linux renders via Blade/Vulkan | **wgpu since Feb 2026** (PR zed#46758) — broad backend fallback, fixed NVIDIA/Wayland freezes |
| Entry point is `Application::new().run()` | Recent revs bootstrap via **`gpui_platform::application().run()`**; platform backends are cargo features on `gpui_platform` (`"wayland", "x11", "font-kit"`) — verify against your pinned rev |
| crates.io `gpui` is a placeholder | It's real but **lags git**; the norm is still `git = "…/zed", rev = "<sha>"` |
| No scaffolding exists | **`zed-industries/create-gpui-app`** generates a correct starter project |

## Quick reference

- **Deps:** pin gpui by exact `rev`, never branch. If using `gpui-component` (the only viable Input/multiline-editor library; Apache-2.0), read ITS `Cargo.toml` first and use the zed rev IT pins — bump both in lockstep only.
- **State:** `Entity<T>` via `cx.new`; mutate with `.update(cx, |t, cx| { …; cx.notify() })`. `observe`/`notify` = "changed, re-render"; `subscribe`/`emit` = typed events (`impl EventEmitter<E> for T`). Granularity: one entity per stateful view/panel (Zed's practice); `cx.set_global` only for true singletons (theme, settings).
- **Keymaps:** `actions!` + `KeyBinding::new(key, Action, Some("Context"))`; element sets `.key_context("Context")` + `.track_focus(&handle)` + `.on_action(...)`. Dispatch walks the focus path. Same-specificity conflicts: **later-registered binding wins**.
- **Custom drawing** (grids, terminals): implement `Element` (`request_layout`/`prepaint`/`paint`); paint with `window.paint_quad(fill(...))` + `window.text_system().shape_line(...).paint(...)`. Never thousands of divs. Homogeneous rows → `uniform_list` with a stored scroll handle.
- **Async:** gpui's own executors — `cx.spawn` (foreground, entity access) / `cx.background_spawn` / `background_executor().timer(d)`. No tokio unless bridged in its own runtime on a background thread; never `block_on` tokio futures on the foreground executor.
- **Zoom:** rem-based — author sizes in `rems`, drive `set_rem_size` (or scope subtrees with `WithRemSize`, Zed's chrome-vs-content split). `px(...)` values do not scale; use px only for deliberately fixed hairlines.
- **Assets:** `AssetSource` impl (commonly `rust-embed`); `svg().path(...)` tints monochrome via text color.

## Pitfalls (each cost someone real time)

- `Subscription`/`Task` handles **cancel on drop** — `.detach()` or store them, else the feature silently dies.
- Mutation without `cx.notify()` never repaints; reads in `render` don't auto-subscribe.
- **BorrowMutError**: updating an entity from inside its own update (nested `cx.update`) panics — restructure via events or deferred spawn (zed#24545).
- Opening a modal does **not** move focus — call `focus()` on its handle on mount or typing goes nowhere (zed discussion #57205).
- Prefix-key chords (`ctrl-w` vs `ctrl-w left` in one context) add a ~1s disambiguation wait.
- Interactive elements need stable `.id(...)`; duplicated ids in lists bleed hover/scroll state between rows.
- Official `examples/` drift and have shipped broken (zed#46183, #46263) — compile-check any example against your pinned rev before trusting it.
- Nested window creation from a popup window can freeze the app (zed#42821); `cx.activate()` is platform-inconsistent (zed#37145).
- sccache won't fix the local edit-compile loop (link-heavy); it helps CI cold builds only.

## When answering gpui questions

1. Check the table above for known stale beliefs.
2. Prefer reading the pinned zed checkout / [patterns.md](patterns.md) over recalling.
3. Anything not covered here: say so and verify in source — do not extrapolate from iced/egui/2024-gpui habits.
