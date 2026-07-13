---
name: html-to-iced
description: Use when converting an HTML/CSS mock (mock*.html, mockups/*.html, sandbox.html) into Iced Rust UI in src/gui/, or implementing/adjusting any Grove GUI element from a design spec or screenshot.
---

# HTML/CSS → Iced 0.14 (Grove)

## Overview

Grove's workflow is: HTML mock first, then port to Iced. The port is mechanical **if** you use the repo's existing token/widget layers instead of re-deriving CSS from scratch. Iced uses the Elm architecture (State → Update → View) and is inherently not a browser. It strictly separates layout, logic, and styling.

**Note on Iced 0.14 [1.1.1]:** Iced now uses **Reactive Rendering** by default [1.1.1, 1.1.4]. It only redraws modified widgets instead of the whole window [1.1.1]. For this (and the Comet time-travel debugger [1.1.1, 1.2.3]) to work correctly, your `update` functions must remain 100% pure. Never read `Instant::now()` or external state inside `update` [1.1.1].

## Step 0 — Reuse Before Writing (Mandatory)

Before writing any view code, check these modules and use them; do not re-invent:

| Module               | Provides                                                                                                                                                         |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/gui/palette.rs` | ALL colors as zero-arg **functions** (`c::BG()`, `c::BG_HOVER()`, `c::BORDER()`, `c::AMBER()`, `c::SCRIM()`, `mix()`), derived per-frame from `theme::current()` |
| `src/gui/metrics.rs` | Layout constants (`ROW_H`, `APPBAR_H`, font consts, glyph-coverage check `mono_covers`)                                                                          |
| `src/gui/widgets.rs` | `dot`, `divider_h/v`, `vline`, `seg_button`, `modal_panel`, `modal_action`, `modal_checkbox`, dropdown/backdrop pattern                                          |
| `src/gui/icons.rs`   | Hand-drawn 16×16 SVG icons, colored via `svg::Style`                                                                                                             |
| `src/gui/rows.rs`    | Sidebar row + chip patterns                                                                                                                                      |

**Never introduce static hex `Color` consts.** A CSS variable or `color-mix()` in the mock maps to a function in `palette.rs` (add one there if missing, via `mix()`), so runtime theme switching keeps working.

**Never render icons/symbols as font glyphs** (`●`, `⌘`, `▦`, emoji, `Font::MONOSPACE` tricks). Bundled fonts lack coverage for most symbol ranges. Add an SVG path to `icons.rs` instead.

## Translation Table

| CSS in mock                                               | Iced 0.14                                                                                                                                                                                |
| --------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `display: flex; gap: N`                                   | `row![]` / `column![]` with `.spacing(N)`                                                                                                                                                |
| `display: grid`                                           | Native `grid![]` widget [1.1.2, 1.2.3] (do not use nested rows/columns for 2D grids)                                                                                                     |
| `display: table`                                          | Native `table` widget [1.1.2, 1.2.3]                                                                                                                                                     |
| `justify-content: space-between`                          | `Space::with_width(Fill)` between items                                                                                                                                                  |
| `flex: 1`                                                 | `.width(Length::Fill)`                                                                                                                                                                   |
| `flex: 2` (Proportional)                                  | `.width(Length::FillPortion(2))`                                                                                                                                                         |
| fixed heights/paddings                                    | Copy values verbatim into `metrics.rs` consts / `.padding()`                                                                                                                             |
| `border-radius: N`                                        | `Radius::from(N)`; circle = radius half of fixed size                                                                                                                                    |
| joined group (`overflow:hidden` + inner segments)         | per-corner `Radius { top_left, .. }` on each button, 1px `Space`/`vline` divider, outer bordered container. **Containers do NOT clip children to their radius** — never rely on clipping |
| `overflow-y: auto / scroll`                               | `scrollable(content)` widget. **Parent must have bounded height** (`Length::Fill` or fixed), otherwise the UI will collapse.                                                             |
| Mixed text styling (`<span style="color:red">...</span>`) | `rich_text![]` macro. Do not use a `row![]` of `text()` elements, as they won't wrap correctly.                                                                                          |
| `:hover` / `:active` on a button                          | `match status` in `button::Style` closure (`button::Status::Hovered/Pressed`)                                                                                                            |
| descendant hover (`.row:hover .child`)                    | `mouse_area(...).on_enter/.on_exit` → hover field in `state.rs` → view builds the child conditionally. Keep the slot **fixed-width** so the row doesn't reflow on hover                  |
| `::before/::after` bars, badges, overlays                 | extra layer in `stack!` — never a layout sibling that shifts content                                                                                                                     |
| `position: absolute` popover/menu                         | `stack![full-screen invisible click-catcher button, positioned panel]` (see `widgets.rs` dropdown)                                                                                       |
| modal + scrim                                             | `stack![body, scrim, centered modal_panel(...)]`; scrim = flat `c::SCRIM()` fill                                                                                                         |
| `color-mix()` / rgba tint                                 | `palette.rs` `mix()` helper or `Color { a, ..c::X() }`                                                                                                                                   |
| CSS Animations / Transitions                              | Use the built-in **`Animation` API** [1.1.2, 1.1.4]. Do not manually calculate tick-math with `Instant::now()` anymore.                                                                  |
| hairline / `gap: 1px` seams                               | 1px filled `container(Space)` (`widgets.rs::divider_*`)                                                                                                                                  |
| `text-overflow: ellipsis`                                 | manual `truncate_ellipsis`/`truncate_middle` (see `rows.rs`) — Iced text has no auto-truncation                                                                                          |
| `linear-gradient`                                         | `Background::Gradient(gradient::Linear)` — works                                                                                                                                         |

## Not Translatable — Approximate, Don't Attempt

| CSS                                                 | Do instead                                                                                |
| --------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| `box-shadow`                                        | Drop it (`shadow: Shadow::default()`). House style is flat; borders carry depth.          |
| `backdrop-filter: blur()`                           | Flat high-alpha color fill.                                                               |
| radial/conic gradients                              | Solid color.                                                                              |
| native drag ghost                                   | Dim source (BG @ ~.72 overlay) + border-highlight the drop target.                        |
| direct DOM mutation (`element.style.color = 'red'`) | Fire a `Message`, update the `State`, and let the `view()` redraw based on the new state. |

## Common Mistakes (All Observed in Practice)

- **Static `const BG: Color = ...`** → Theme switching breaks. Always use `palette.rs` functions.
- **Unicode glyph icons** → Tofu on the bundled fonts. Use `icons.rs` SVG.
- **Faithfully porting `box-shadow`** → Off-style. Drop shadows entirely.
- **Accent bar as `row![bar, content]`** → Content shifts 3px. Use `stack!`.
- **Trusting container radius to clip segment corners** → Corners poke out. Define per-corner radius explicitly.
- **Unbounded Scrollables** → Putting a `scrollable` inside a `Column` without `.height(Length::Fill)` causes overlapping content or invisible widgets.
- **Time/State mutations in `update`** → Breaks the Comet time-travel debugger [1.1.1]. `update` must be pure. Use `Subscription::Time::every` if you need time events [1.1.1].
- **Hover-revealed actions collapsing to zero width** → Row jitters and reflows continuously. Use a fixed-width slot swap.
