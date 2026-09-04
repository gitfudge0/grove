# Grove design system contract

Status: dark-first neutral system for the native Rust and GPUI app. The light theme is a warm-white counterpart.

One semantic scheme drives both appearances. Dark is the product default; light is its derived counterpart, not a separate theme family.

`DESIGN.html` generated JavaScript values win if this Markdown copy drifts.

## Primitives

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| neutral-0 | `#ffffff` | `c::NEUTRAL_0()` |
| neutral-50 | `#f7f7f8` | `c::NEUTRAL_50()` |
| neutral-100 | `#ededee` | `c::NEUTRAL_100()` |
| neutral-200 | `#dedee0` | `c::NEUTRAL_200()` |
| neutral-300 | `#c3c3c7` | `c::NEUTRAL_300()` |
| neutral-400 | `#9a9aa0` | `c::NEUTRAL_400()` |
| neutral-500 | `#707078` | `c::NEUTRAL_500()` |
| neutral-600 | `#515158` | `c::NEUTRAL_600()` |
| neutral-700 | `#34343a` | `c::NEUTRAL_700()` |
| neutral-800 | `#232327` | `c::NEUTRAL_800()` |
| neutral-850 | `#1b1b1e` | `c::NEUTRAL_850()` |
| neutral-900 | `#141416` | `c::NEUTRAL_900()` |
| neutral-950 | `#0d0d0f` | `c::NEUTRAL_950()` |
| warm-white | `#fbfaf7` | `c::WARM_WHITE()` |
| violet-400 | `#a78bfa` | `c::VIOLET_400()` |
| violet-500 | `#8b5cf6` | `c::VIOLET_500()` |
| violet-600 | `#7c3aed` | `c::VIOLET_600()` |
| green-500 | `#78d98b` | `c::GREEN()` |
| amber-500 | `#e0ad63` | `c::AMBER()` |
| red-500 | `#ef7d8e` | `c::RED()` |
| blue-500 | `#79a9e8` | `c::BLUE()` |

Violet is limited to agent identity, connection state, focus, and active ports. Green means success or running. Amber means needs-you or warning. Red means destructive or error. Blue means tertiary information.

## Semantic light and dark

| Token | Dark | Light | Meaning | Rust / GPUI mapping |
|---|---|---|---|---|
| bg | `#141416` | `#fbfaf7` | main application background | `c::BG()` |
| bg-subtle | `#0d0d0f` | `#f7f7f8` | appbar and statusbar | `c::BG_STRIP()` |
| surface | `#1b1b1e` | `#ffffff` | sidebar, panels, and bounded regions | `c::SURFACE()` |
| surface-raised | `#232327` | `#ffffff` | menus, dialogs, and floating panels | `c::SURFACE_RAISED()` |
| hover | `#34343a` | `#ededee` | neutral pointer hover | `c::BG_HOVER()` |
| selected | `#232327` | `#ededee` | neutral selected row fill | `c::BG_HL()` |
| text-primary | `#f7f7f8` | `#141416` | required copy and primary labels | `c::FG()` |
| text-secondary | `#c3c3c7` | `#515158` | supporting copy | `c::FG_DIM()` |
| text-muted | `#707078` | `#707078` | nonessential metadata | `c::FG_MUTE()` |
| brand | `#f7f7f8` | `#141416` | Grove mark and high-contrast brand fill | `c::BRAND()` |
| brand-hover | `#ededee` | `#232327` | hover for brand-filled actions | `c::BRAND_HOVER()` |
| on-brand | `#141416` | `#f7f7f8` | text and icons on brand | `c::ON_BRAND()` |
| inverse-cta-bg | `#f7f7f8` | `#141416` | dominant inverse call to action | `c::INVERSE_CTA_BG()` |
| inverse-cta-text | `#141416` | `#f7f7f8` | content on inverse call to action | `c::INVERSE_CTA_TEXT()` |
| accent | `#8b5cf6` | `#7c3aed` | agent identity, connection, and active port | `c::MAGENTA()` |
| accent-hover | `#a78bfa` | `#8b5cf6` | hover for violet-only roles | `c::ACCENT_HOVER()` |
| accent-pressed | `#7c3aed` | `#7c3aed` | pressed violet-only roles | `c::ACCENT_PRESSED()` |
| focus | `#a78bfa` | `#7c3aed` | keyboard focus ring | `c::SEL_RING()` |
| accent-wash | `rgba(139,92,246,.14)` | `rgba(124,58,237,.10)` | quiet agent or connection backing | `c::SEL_TINT_SOFT()` |
| running / success | `#78d98b` | `#78d98b` | running process or completed action | `c::GREEN()` |
| needs-you / warning | `#e0ad63` | `#e0ad63` | required input or caution | `c::AMBER()` |
| destructive / error | `#ef7d8e` | `#ef7d8e` | destructive action or failure | `c::RED()` |
| tertiary / info | `#79a9e8` | `#79a9e8` | tertiary information only | `c::BLUE()` |
| border | `#34343a` | `#dedee0` | standard one-pixel seam | `c::BORDER()` |
| border-strong | `#515158` | `#c3c3c7` | emphasized structure | `c::BORDER_STRONG()` |
| divider-soft | `rgba(247,247,248,.08)` | `rgba(20,20,22,.09)` | quiet internal divider | `c::BORDER_SOFT()` |
| scrim | `rgba(13,13,15,.76)` | `rgba(20,20,22,.38)` | modal and blocking overlay | `c::SCRIM()` |
| terminal-bg | `#0d0d0f` | `#ffffff` | PTY background | `c::TERMINAL_BG()` |
| terminal-text | `#ededee` | `#232327` | PTY foreground | `c::TERMINAL_TEXT()` |

Components use semantic accessors only. Neutral selection never becomes violet. Violet remains reserved for agent identity, connection state, focus, and active ports.

## Typography

### Families

| Token | CSS family | Use | Rust / GPUI mapping |
|---|---|---|---|
| font-ui | `"IBM Plex Sans", Arial, sans-serif` | all application UI | `gpui::font(fonts::UI_FAMILY)` through `ui(text, size, color)` |
| font-mono | `"BlexMono Nerd Font Mono", "IBM Plex Mono", monospace` | terminal, path, branch, keycap, status | `gpui::font(fonts::MONO_FAMILY)` through `mono(text, size, color)` |

Both primary families are bundled. Arial and IBM Plex Mono are fallbacks, not replacements.

### Scale

`1rem = 16 design px.`

| Token | px / rem | Line height | Default weight | Rust / GPUI mapping |
|---|---:|---:|---:|---|
| text-10 | 10 / .625rem | 13px | 500 | `.text_size(rpx(TEXT_10))` where `TEXT_10 = 10.0` |
| text-11 | 11 / .6875rem | 15px | 400 | `.text_size(rpx(TEXT_11))` where `TEXT_11 = 11.0` |
| text-12 | 12 / .75rem | 17px | 400 | `.text_size(rpx(TEXT_12))` where `TEXT_12 = 12.0` |
| text-13 | 13 / .8125rem | 19px | 400 | `.text_size(rpx(TEXT_13))` where `TEXT_13 = 13.0` |
| text-15 | 15 / .9375rem | 20px | 600 | `.text_size(rpx(TEXT_15))` where `TEXT_15 = 15.0` |
| text-18 | 18 / 1.125rem | 23px | 600 | `.text_size(rpx(TEXT_18))` where `TEXT_18 = 18.0` |
| text-24 | 24 / 1.5rem | 29px | 700 | `.text_size(rpx(TEXT_24))` where `TEXT_24 = 24.0` |
| text-32 | 32 / 2rem | 36px | 700 | `.text_size(rpx(TEXT_32))` where `TEXT_32 = 32.0` |

### Weights

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| weight-regular | 400 | `gpui::FontWeight::NORMAL` |
| weight-medium | 500 | `gpui::FontWeight::MEDIUM` |
| weight-semibold | 600 | `gpui::FontWeight::SEMIBOLD` |
| weight-bold | 700 | `gpui::FontWeight::BOLD` |

### Type roles

| Role | Family | Size | Weight | Use | Rust / GPUI mapping |
|---|---|---:|---:|---|---|
| brand mark | UI | 15 | 700 | GROVE wordmark | `ui("GROVE", TEXT_15, c::BRAND()).font_weight(FontWeight::BOLD)` |
| workspace trigger | UI | 12 | 500 | active workspace button | `ui(name, TEXT_12, c::FG()).font_weight(FontWeight::MEDIUM)` |
| section heading | UI | 18 | 600 | page and panel headings | `ui(label, TEXT_18, c::FG()).font_weight(FontWeight::SEMIBOLD)` |
| row label | UI | 12 | 500 | project, worktree, and session | `ui(label, TEXT_12, c::FG()).font_weight(FontWeight::MEDIUM)` |
| body | UI | 13 | 400 | guidance and dialog copy | `ui(copy, TEXT_13, c::FG())` |
| metadata | Mono | 11 | 400 | path and branch | `mono(meta, TEXT_11, c::FG_DIM())` |
| status | Mono | 10 | 500 | statusbar and counters | `mono(status, TEXT_10, c::FG_DIM()).font_weight(FontWeight::MEDIUM)` |
| terminal | Mono | 12 | 400 | PTY text | `mono(line, TEXT_12, c::TERMINAL_TEXT())` |
| display | UI | 32 | 700 | sparse empty-state title | `ui(title, TEXT_32, c::FG()).font_weight(FontWeight::BOLD)` |

## Radius

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| radius-4 | 4px | `.rounded(rpx(RADIUS_4))` with `RADIUS_4 = 4.0` |
| radius-8 | 8px | `.rounded(rpx(RADIUS_8))` with `RADIUS_8 = 8.0` |
| radius-12 | 12px | `.rounded(rpx(RADIUS_12))` with `RADIUS_12 = 12.0` |
| radius-full | full pill, status only | `.rounded_full()` for status pills and indicators only |

## Spacing

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| space-2 | 2px | `rpx(SPACE_2)` with `SPACE_2 = 2.0` |
| space-4 | 4px | `rpx(SPACE_4)` with `SPACE_4 = 4.0` |
| space-6 | 6px | `rpx(SPACE_6)` with `SPACE_6 = 6.0` |
| space-8 | 8px | `rpx(SPACE_8)` with `SPACE_8 = 8.0` |
| space-10 | 10px | `rpx(SPACE_10)` with `SPACE_10 = 10.0` |
| space-12 | 12px | `rpx(SPACE_12)` with `SPACE_12 = 12.0` |
| space-16 | 16px | `rpx(SPACE_16)` with `SPACE_16 = 16.0` |
| space-20 | 20px | `rpx(SPACE_20)` with `SPACE_20 = 20.0` |
| space-24 | 24px | `rpx(SPACE_24)` with `SPACE_24 = 24.0` |
| space-32 | 32px | `rpx(SPACE_32)` with `SPACE_32 = 32.0` |
| space-40 | 40px | `rpx(SPACE_40)` with `SPACE_40 = 40.0` |
| space-48 | 48px | `rpx(SPACE_48)` with `SPACE_48 = 48.0` |

Geometry is part of the spacing contract.

| Geometry token | Value | Rust / GPUI mapping |
|---|---:|---|
| appbar-h | 40px | `rpx(APPBAR_H)` with `APPBAR_H = 40.0` |
| sidebar-w | 236px | `rpx(SIDEBAR_W)` with `SIDEBAR_W = 236.0` |
| header-h | 36px | `rpx(HEADER_H)` with `HEADER_H = 36.0` |
| status-h | 26px | `rpx(STATUS_H)` with `STATUS_H = 26.0` |
| row-h | 28px | `rpx(ROW_H)` with `ROW_H = 28.0` |
| control-h | 24px | `rpx(CONTROL_H)` with `CONTROL_H = 24.0` |

Workspace switching is appbar content and owns no separate width token.

## Gaps

| Token | Alias | Rust / GPUI mapping |
|---|---|---|
| gap-2 | space-2 | `.gap(rpx(GAP_2))` with `GAP_2 = SPACE_2` |
| gap-4 | space-4 | `.gap(rpx(GAP_4))` with `GAP_4 = SPACE_4` |
| gap-6 | space-6 | `.gap(rpx(GAP_6))` with `GAP_6 = SPACE_6` |
| gap-8 | space-8 | `.gap(rpx(GAP_8))` with `GAP_8 = SPACE_8` |
| gap-12 | space-12 | `.gap(rpx(GAP_12))` with `GAP_12 = SPACE_12` |
| gap-16 | space-16 | `.gap(rpx(GAP_16))` with `GAP_16 = SPACE_16` |

## Borders

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| border-thin | 1 physical px | `.border_1().border_color(c::BORDER())` |
| border-medium | 2px | `.border(rpx(BORDER_MEDIUM)).border_color(c::BORDER_STRONG())` |
| focus-ring | 2px | `.border(rpx(FOCUS_RING)).border_color(c::SEL_RING())` |

## Shadows

| Token | Dark | Light | Rust / GPUI mapping |
|---|---|---|---|
| shadow-sm | `0 1px 2px rgba(0,0,0,.36)` | `0 1px 2px rgba(13,13,15,.10)` | `gpui::BoxShadow { color: c::SHADOW_SM(), offset: gpui::point(px(0.), px(1.)), blur_radius: px(2.), spread_radius: px(0.), inset: false }` |
| shadow-md | `0 8px 24px rgba(0,0,0,.42)` | `0 8px 24px rgba(13,13,15,.14)` | `gpui::BoxShadow { color: c::SHADOW_MD(), offset: gpui::point(px(0.), px(8.)), blur_radius: px(24.), spread_radius: px(0.), inset: false }` |
| shadow-lg | `0 16px 48px rgba(0,0,0,.50)` | `0 16px 48px rgba(13,13,15,.18)` | `gpui::BoxShadow { color: c::SHADOW_LG(), offset: gpui::point(px(0.), px(16.)), blur_radius: px(48.), spread_radius: px(0.), inset: false }` |

Dark depth comes from surface steps and borders first. Shadows remain quiet.

## Motion

| Token | Value | Rust / GPUI mapping |
|---|---|---|
| duration-fast | 80ms | `Duration::from_millis(80)` |
| duration-base | 140ms | `Duration::from_millis(140)` |
| duration-slow | 220ms | `Duration::from_millis(220)` |
| ease-standard | `cubic-bezier(.2,.8,.2,1)` | `cubic_bezier(0.2, 0.8, 0.2, 1.0)` |
| ease-decelerate | `cubic-bezier(.16,1,.3,1)` | `cubic_bezier(0.16, 1.0, 0.3, 1.0)` |
| ease-accelerate | `cubic-bezier(.4,0,1,1)` | `cubic_bezier(0.4, 0.0, 1.0, 1.0)` |

Reduced motion uses `Duration::ZERO` and skips transform-based transitions. Do not animate a transform when the reduced-motion preference is active.

## Z-index

| Token | Order | Rust / GPUI mapping |
|---|---:|---|
| z-base | 0 | paint order 0, first normal `Stack` child |
| z-raised | 10 | paint order 10, later `Stack` child |
| z-sticky | 20 | paint order 20, after scrolling content |
| z-overlay | 40 | paint order 40, first `Overlay` entry |
| z-modal | 60 | paint order 60, later `Overlay` entry |
| z-toast | 80 | paint order 80, last `Overlay` entry |

GPUI has no CSS z-index property. These values are ordering semantics only.

## Icons

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| icon-12 | 12px | `.size(rpx(ICON_12))` with `ICON_12 = 12.0` |
| icon-14 | 14px | `.size(rpx(ICON_14))` with `ICON_14 = 14.0` |
| icon-16 | 16px | `.size(rpx(ICON_16))` with `ICON_16 = 16.0` |
| icon-20 | 20px | `.size(rpx(ICON_20))` with `ICON_20 = 20.0` |
| icon-24 | 24px | `.size(rpx(ICON_24))` with `ICON_24 = 24.0` |
| icon-32 | 32px | `.size(rpx(ICON_32))` with `ICON_32 = 32.0` |

Use one-pixel or 1.5-pixel optical strokes and `currentColor`. Violet icons are restricted to the violet semantic roles.

## Opacity

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| opacity-disabled | .38 | `c::alpha(color, 0.38)` |
| opacity-muted | .62 | `c::alpha(color, 0.62)` |
| opacity-hover | .08 | `c::alpha(color, 0.08)` |
| opacity-pressed | .14 | `c::alpha(color, 0.14)` |
| opacity-scrim | .76 | `c::alpha(c::NEUTRAL_950(), 0.76)` |

## Blur

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| blur-0 | 0px | no filter; paint content directly |
| blur-4 | 4px | no core GPUI equivalent; platform compositor helper with 4px radius |
| blur-8 | 8px | no core GPUI equivalent; platform compositor helper with 8px radius |
| blur-16 | 16px | no core GPUI equivalent; platform compositor helper with 16px radius |

Blur is not a surface-building tool in Grove. Use it only when a platform overlay requires it.

## Assumptions

| Assumption | Record | GPUI mapping or restriction |
|---|---|---|
| Source | Palette, geometry, and density come from the selected A4 wireframe and written brief. | `DESIGN.html` token arrays and CSS blocks win over this copy. |
| Themes | Dark received primary design scrutiny. Light is a derived warm-white counterpart. | Both resolve through the same `c::*()` semantic names. |
| Fonts | IBM Plex Sans and Blex Mono Nerd Font Mono are bundled in `src/fonts.rs`. | Use `ui()` and `mono()` helpers only. |
| Units | 1rem is 16 design px. Layout values pass through `rpx()`; hairlines use `px(1.)`. | Do not place bare layout numbers in components. |
| Regularization | Spacing and radius are tidy scales rather than traced values. | Add or rename constants in `src/views/tokens.rs` before component work. |
| New aliases | Existing `c::BG*()`, `c::FG*()`, status colors, and `c::SEL_*()` map directly. New semantic names in this contract must become thin accessors in `src/theme.rs`. | Do not substitute component literals while aliases are pending. |
| Defaults | Shadows, motion, z order, opacity, and blur are defaults because a static wireframe cannot specify them. | Reduced motion is mandatory; blur stays rare. |
| Geometry | Appbar 40, sidebar 236, header 36, status 26, row 28, control 24. | Workspace switching adds no separate side strip. |
| Support color text | Green, amber, red, and blue are role colors, not default copy colors. | On warm white, pair their indicators with `c::FG()` text until each small-text pairing is separately validated. |

### Computed contrast

| Pair | Dark | Light | Restriction |
|---|---:|---:|---|
| brand / bg | 17.18:1 | 17.63:1 | AAA for normal text |
| on-brand / brand | 17.18:1 | 17.18:1 | AAA for normal text |
| primary / bg | 17.18:1 | 17.63:1 | AAA for normal text |
| secondary / bg | 10.47:1 | 7.54:1 | AAA for normal text |
| muted / bg | 3.75:1 | 4.70:1 | Dark is nonessential metadata or large text only. Light passes AA for normal text. |
| accent / bg | 4.35:1 | 5.46:1 | Keep out of body copy in both themes. Use for agent identity, connection, focus, and active ports. |

Ratios use WCAG relative luminance against `#141416` in dark and `#fbfaf7` in light.
