---
name: html-to-iced
description: Use when converting an HTML/CSS mock (mock*.html, mockups/*.html, sandbox.html) into Iced Rust UI in src/gui/, or implementing/adjusting any Grove GUI element from a design spec or screenshot.
---

# HTML/CSS → Iced 0.13 (Grove)

## Overview

Grove's workflow is: HTML mock first, then port to Iced. The port is mechanical **if** you use the repo's existing token/widget layers instead of re-deriving CSS from scratch. Iced is not a browser — several CSS features silently don't exist, and naive translations compile but look or behave wrong.

## Step 0 — reuse before writing (mandatory)

Before writing any view code, check these modules and use them; do not re-invent:

| Module | Provides |
|---|---|
| `src/gui/palette.rs` | ALL colors as zero-arg **functions** (`c::BG()`, `c::BG_HOVER()`, `c::BORDER()`, `c::AMBER()`, `c::SCRIM()`, `mix()`), derived per-frame from `theme::current()` |
| `src/gui/metrics.rs` | Layout constants (`ROW_H`, `APPBAR_H`, font consts, glyph-coverage check `mono_covers`) |
| `src/gui/widgets.rs` | `dot`, `divider_h/v`, `vline`, `seg_button`, `modal_panel`, `modal_action`, `modal_checkbox`, dropdown/backdrop pattern |
| `src/gui/icons.rs` | Hand-drawn 16×16 SVG icons, colored via `svg::Style` |
| `src/gui/rows.rs` | Sidebar row + chip patterns |

**Never introduce static hex `Color` consts.** A CSS variable or `color-mix()` in the mock maps to a function in `palette.rs` (add one there if missing, via `mix()`), so runtime theme switching keeps working.

**Never render icons/symbols as font glyphs** (`●`, `⌘`, `▦`, emoji, `Font::MONOSPACE` tricks). Bundled fonts lack coverage for most symbol ranges. Add an SVG path to `icons.rs` instead.

## Translation table

| CSS in mock | Iced 0.13 |
|---|---|
| `display:flex; gap:N` | `row![]/column![].spacing(N)` |
| `justify-content:space-between` | `Space::with_width(Fill)` between items |
| `flex:1` | `.width(Length::Fill)` |
| fixed heights/paddings | copy values verbatim into `metrics.rs` consts / `.padding()` |
| `border-radius:N` | `Radius::from(N)`; circle = radius half of fixed size |
| joined group (`overflow:hidden` + inner segments) | per-corner `Radius { top_left, .. }` on each button, 1px `Space`/`vline` divider, outer bordered container. **Containers do NOT clip children to their radius** — never rely on clipping |
| `:hover` / `:active` on a button | `match status` in `button::Style` closure (`button::Status::Hovered/Pressed`) |
| descendant hover (`.row:hover .child`) | `mouse_area(...).on_enter/.on_exit` → hover field in `state.rs` → view builds the child conditionally. Keep the slot **fixed-width** so the row doesn't reflow on hover |
| `::before/::after` bars, badges, overlays | extra layer in `stack!` — never a layout sibling that shifts content |
| `position:absolute` popover/menu | `stack![full-screen invisible click-catcher button, positioned panel]` (see `widgets.rs` dropdown) |
| modal + scrim | `stack![body, scrim, centered modal_panel(...)]`; scrim = flat `c::SCRIM()` fill |
| `color-mix()` / rgba tint | `palette.rs` `mix()` helper or `Color { a, ..c::X() }` |
| `@keyframes` pulse/spin | per-frame math off the existing ~60ms `Msg::Tick`/`blink_tick` counter (triangle-wave alpha, rotation) — do **not** add a new subscription or use `Instant::now()` in view code |
| hairline / `gap:1px` seams | 1px filled `container(Space)` (`widgets.rs::divider_*`) |
| `text-overflow:ellipsis` | manual `truncate_ellipsis`/`truncate_middle` (see `rows.rs`) — Iced text has no truncation |
| `linear-gradient` | `Background::Gradient(gradient::Linear)` — works |

## Not translatable — approximate, don't attempt

| CSS | Do instead |
|---|---|
| `box-shadow` | Drop it (`shadow: Shadow::default()`). House style is flat; borders carry depth |
| `backdrop-filter: blur()` | Flat high-alpha color fill |
| CSS transitions/easing | Instant style swaps; animate only via tick math |
| radial/conic gradients | Solid color |
| native drag ghost | Dim source (BG @ ~.72 overlay) + border-highlight the drop target |

## Common mistakes (all observed in practice)

- Static `const BG: Color = ...` → theme switching breaks. Use `palette.rs` functions.
- Unicode glyph icons → tofu on the bundled fonts. Use `icons.rs` SVG.
- Faithfully porting `box-shadow` → off-style. Drop shadows.
- Accent bar as `row![bar, content]` → content shifts 3px. Use `stack!`.
- Trusting container radius to clip segment corners → corners poke out. Per-corner radius.
- New `iced::time::every` subscription per animation → redundant; reuse the tick.
- Hover-revealed actions collapsing to zero width → row jitters. Fixed-width slot swap.
