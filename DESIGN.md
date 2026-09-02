# Grove design system contract

Status: fixed dark-first product scheme for the native Rust/GPUI app. The light theme is a derived accessibility and reference counterpart required by the design-system contract. It is not a reason to restore user-selectable colorways.

`DESIGN.html` renders specimens and GPUI strings from one JavaScript token source. It wins if this Markdown copy drifts.

## Primitives

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| plum-black-950 | `#0d0b0f` | `Color::Rgb(13, 11, 15)` |
| plum-black-900 | `#141216` | `Color::Rgb(20, 18, 22)` |
| plum-black-850 | `#18151b` | `Color::Rgb(24, 21, 27)` |
| plum-black-800 | `#201c23` | `Color::Rgb(32, 28, 35)` |
| plum-black-750 | `#26212a` | `Color::Rgb(38, 33, 42)` |
| plum-black-700 | `#302a34` | `Color::Rgb(48, 42, 52)` |
| plum-black-600 | `#43394a` | `Color::Rgb(67, 57, 74)` |
| plum-black-450 | `#6d6370` | `Color::Rgb(109, 99, 112)` |
| plum-black-300 | `#aaa0ad` | `Color::Rgb(170, 160, 173)` |
| plum-black-100 | `#e9e4eb` | `Color::Rgb(233, 228, 235)` |
| plum-black-50 | `#f5f0f6` | `Color::Rgb(245, 240, 246)` |
| plum-black-25 | `#fbf7fb` | `Color::Rgb(251, 247, 251)` |
| brand-700 | `#7a1f9f` | `Color::Rgb(122, 31, 159)` |
| brand-600 | `#8f27b8` | `Color::Rgb(143, 39, 184)` |
| brand-500 | `#a63bd2` | `Color::Rgb(166, 59, 210)` |
| brand-400 | `#c15ff5` | `Color::Rgb(193, 95, 245)` |
| brand-300 | `#d488f8` | `Color::Rgb(212, 136, 248)` |
| violet-600 / 400 | `#7650d6` / `#9a6cff` | `Color::Rgb(118, 80, 214)` / `Color::Rgb(154, 108, 255)` |
| green-700 / 400 | `#2d8b4d` / `#6fd083` | `Color::Rgb(45, 139, 77)` / `Color::Rgb(111, 208, 131)` |
| amber-700 / 400 | `#9b651f` / `#dcaa63` | `Color::Rgb(155, 101, 31)` / `Color::Rgb(220, 170, 99)` |
| red-700 / 400 | `#a83f55` / `#df7181` | `Color::Rgb(168, 63, 85)` / `Color::Rgb(223, 113, 129)` |
| blue-700 / 400 | `#355d8c` / `#82a7d6` | `Color::Rgb(53, 93, 140)` / `Color::Rgb(130, 167, 214)` |

Opaque primitives live in `grove-core` as `Color::Rgb(r,g,b)`. Components never use primitive literals directly.

## Semantic dark / light

| Token | Dark | Light | Meaning | Rust / GPUI mapping |
|---|---|---|---|---|
| bg | `#141216` | `#fbf7fb` | main shell | `c::BG()` |
| bg-subtle / strip | `#0d0b0f` | `#f5f0f6` | appbar, status | `c::BG_SUBTLE()` |
| rail | `#18151b` | `#eee8ef` | workspace/navigation rail | `c::RAIL()` |
| surface | `#18151b` | `#f8f3f8` | bounded surface | `c::SURFACE()` |
| surface-raised | `#201c23` | `#fffaff` | active/elevated | `c::SURFACE_RAISED()` |
| hover | `#26212a` | `#ebe3ed` | pointer hover | `c::HOVER()` |
| selected | `rgba(193,95,245,.08)` | `rgba(143,39,184,.08)` | selected wash | `c::SELECTED()` returns Rgba |
| selected-border | `rgba(193,95,245,.30)` | `rgba(143,39,184,.36)` | focus-selected seam | `c::SELECTED_BORDER()` |
| text-primary | `#e9e4eb` | `#241d27` | primary copy | `c::TEXT_PRIMARY()` |
| text-secondary | `#aaa0ad` | `#514854` | secondary copy | `c::TEXT_SECONDARY()` |
| text-muted | `#6d6370` | `#746a76` | metadata only | `c::TEXT_MUTED()` |
| brand | `#c15ff5` | `#8f27b8` | Grove / active focus | `c::BRAND()` |
| brand-hover | `#d488f8` | `#7a1f9f` | hover | `c::BRAND_HOVER()` |
| brand-pressed | `#a63bd2` | `#681786` | pressed | `c::BRAND_PRESSED()` |
| on-brand | `#0d0b0f` | `#fff9ff` | text on brand | `c::ON_BRAND()` |
| focus | `#c15ff5` | `#8f27b8` | keyboard/focus | `c::FOCUS()` |
| focus-wash | `rgba(193,95,245,.10)` | `rgba(143,39,184,.10)` | focus background | `c::FOCUS_WASH()` |
| running / success | `#6fd083` | `#2d8b4d` | running or success only | `c::RUNNING()` / `c::SUCCESS()` |
| needs-you / warning | `#dcaa63` | `#9b651f` | user attention only | `c::NEEDS_YOU()` / `c::WARNING()` |
| amber-wash | `rgba(220,170,99,.09)` | `rgba(155,101,31,.09)` | waiting row | `c::AMBER_WASH()` |
| destructive / error | `#df7181` | `#a83f55` | destructive/error only | `c::DESTRUCTIVE()` / `c::ERROR()` |
| red-wash | `rgba(223,113,129,.10)` | `rgba(168,63,85,.09)` | destructive hover | `c::RED_WASH()` |
| info / navigation | `#9a6cff` | `#355d8c` | informational navigation | `c::INFO()` / `c::NAVIGATION()` |
| border | `#302a34` | `#d9cfdc` | 1px seam | `c::BORDER()` |
| border-strong | `#43394a` | `#b9aebe` | focus structure | `c::BORDER_STRONG()` |
| divider-soft | `rgba(233,228,235,.07)` | `rgba(36,29,39,.08)` | quiet divider | `c::DIVIDER_SOFT()` |
| scrim | `rgba(13,11,15,.72)` | `rgba(36,29,39,.42)` | modal scrim | `c::SCRIM()` |
| dot-grid | `rgba(67,57,74,.24)` | `rgba(81,72,84,.15)` | sparse field texture | `c::DOT_GRID()` |
| terminal-bg / text | `#141216` / `#e9e4eb` | `#fbf7fb` / `#241d27` | PTY | `c::TERMINAL_BG()` / `c::TERMINAL_TEXT()` |
| diff-add / bg | `#6fd083` / `rgba(111,208,131,.10)` | `#2d8b4d` / `rgba(45,139,77,.10)` | additions | `c::DIFF_ADD()` / `c::DIFF_ADD_BG()` |
| diff-delete / bg | `#df7181` / `rgba(223,113,129,.10)` | `#a83f55` / `rgba(168,63,85,.09)` | deletions | `c::DIFF_DELETE()` / `c::DIFF_DELETE_BG()` |
| shadow-color | `rgba(5,3,6,.28)` | `rgba(46,34,49,.14)` | shadow primitive | `c::SHADOW()` |

Alpha semantics use `gpui::Rgba { r, g, b, a }` through theme accessors. Meanings never swap: plum is brand/focus/active, green is running/success, amber is needs-user/warning, red is destructive/error, and violet or muted blue is informational navigation.

## Typography

CSS source: `--text-*`, `--line-*`, `--weight-*`, and reusable `--tracking-brand/heading/section/status`.

Families: UI `"IBM Plex Sans", -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif` maps to `ui()`. Terminal `"BlexMono Nerd Font Mono", "IBM Plex Mono", ui-monospace, monospace` maps to `mono()`. Both are bundled product fonts.

| Token | rem / px | Line | Weight | GPUI mapping |
|---|---:|---:|---:|---|
| micro | `.625 / 10` | 1.3 | 500 | `ui().text_size(rpx(10.)).font_weight(MEDIUM)` |
| caption | `.6875 / 11` | 1.35 | 400 | `ui().text_size(rpx(11.))` |
| body-sm | `.75 / 12` | 1.45 | 400 | `ui().text_size(rpx(12.))` |
| body | `.8125 / 13` | 1.5 | 400 | `ui().text_size(rpx(13.))` |
| title | `.9375 / 15` | 1.35 | 600 | `ui().text_size(rpx(15.)).font_weight(SEMIBOLD)` |
| heading | `1.125 / 18` | 1.25 | 600 | `ui().text_size(rpx(18.)).font_weight(SEMIBOLD)` |
| display | `1.5 / 24` | 1.15 | 700 | `ui().text_size(rpx(24.)).font_weight(BOLD)` |
| display-lg | `2 / 32` | 1.08 | 700 | `ui().text_size(rpx(32.)).font_weight(BOLD)` |
| weights | 400 / 500 / 600 / 700 | n/a | named | `NORMAL / MEDIUM / SEMIBOLD / BOLD` |

Mono replaces `ui()` with `mono()` for terminal, keycaps, paths, and micro metadata. `1rem = 16 design px`.

### Type roles

UI roles use IBM Plex Sans. Mono roles use Blex Mono Nerd Font Mono. Terminal mono never becomes decorative UI identity. Tracked uppercase is reserved for micro section labels. Muted metadata never drops below caption. Body copy is never all caps.

| Role | Family | Scale | Weight | Letter spacing / case | Usage | Grove example | GPUI mapping |
|---|---|---|---:|---|---|---|---|
| Brand mark | UI | title | 600 | `.02em`, lowercase | product wordmark only | `grove` | `ui().text_size(rpx(15.)).font_weight(SEMIBOLD)` |
| Workspace name | UI | title | 600 | normal, title case | active workspace / tooltip | `Client platform` | `ui().text_size(rpx(15.)).font_weight(SEMIBOLD)` |
| Destination title | UI | heading | 600 | `-.01em`, sentence case | Monitor / Project heading | `Monitor` | `ui().text_size(rpx(18.)).font_weight(SEMIBOLD)` |
| Panel / section label | UI | micro | 600 | `.10em`, uppercase | short navigation group labels only | `NEEDS YOU` | `ui().text_size(rpx(10.)).font_weight(SEMIBOLD)` plus tracking helper |
| Session identity | UI | body-sm | 600 | normal | agent and task identity | `Claude Code · Refine global color system` | `ui().text_size(rpx(12.)).font_weight(SEMIBOLD)` |
| Worktree / project row | UI | body-sm | 500 | normal | tree and picker rows | `grove / main` | `ui().text_size(rpx(12.)).font_weight(MEDIUM)` |
| Body / help text | UI | body | 400 | normal, sentence case | explanation and guidance | `Choose a worktree to start a session.` | `ui().text_size(rpx(13.))` |
| Metadata / path / branch | Mono | caption | 400 | normal | paths, branch, elapsed | `~/Sandbox/grove · main` | `mono().text_size(rpx(11.))` |
| Status / telemetry label | Mono | micro | 500 | `.04em`, sentence case | statusbar and compact counters | `3 running · native` | `mono().text_size(rpx(10.)).font_weight(MEDIUM)` |
| Keycap / shortcut | Mono | micro | 500 | normal | command hints only | `⌘K` | `mono().text_size(rpx(10.)).font_weight(MEDIUM)` |
| Terminal content | Mono | body | 400 | normal | PTY output and prompt | `› cargo test theme` | `mono().text_size(rpx(13.))` |
| Empty-state display | UI | display | 600 | `-.015em`, sentence case | one short empty-state line | `No sessions need you` | `ui().text_size(rpx(24.)).font_weight(SEMIBOLD)` |

## Radius

| Token | px | GPUI mapping |
|---|---:|---|
| radius-0/2/4/6/8/12 | 0, 2, 4, 6, 8, 12 | `.rounded(rpx(RADIUS_*))` |
| radius-full | 999 | `.rounded_full()`; semantic pills only |

Dominant control radius is 4; grouped containers use 6.

## Spacing

| Token | px | GPUI mapping |
|---|---:|---|
| space-2/4/6/8/10/12 | 2,4,6,8,10,12 | constants in `src/views/tokens.rs`; `rpx(SPACE_*)` |
| space-16/20/24/32/40/48 | 16,20,24,32,40,48 | constants in `src/views/tokens.rs`; `rpx(SPACE_*)` |
| rail/context/appbar | 52 / 236 / 40 | `rpx(WORKSPACE_RAIL_W / CONTEXT_SIDEBAR_W / APPBAR_H)` |
| header/status/row/control | 36 / 26 / 28 / 22 | `rpx(HEADER_H / STATUS_H / ROW_H / CONTROL_H)` |
| grid-seam | 1 physical px | `px(1.0)` explicitly non-scaling |
| terminal-main-min / panel-min | 480 / 280 | `rpx(TERMINAL_MAIN_MIN_W / TERMINAL_PANEL_MIN_W)` |

Appbar is 40px: dense enough for the 22px controls plus 9px vertical padding, and more stable cross-platform than 38px.

## Gaps

CSS source: `--gap-xs/sm/md/lg/xl`.

| Token | Alias | GPUI mapping |
|---|---|---|
| gap-xs/sm/md/lg/xl | space-2/4/8/12/16 | `.gap(rpx(GAP_*))` or child margins |

## Border widths

| Token | Value | GPUI mapping |
|---|---:|---|
| border-thin | 1px | `px(1.0)` for physical hairlines |
| border-medium | 2px | `rpx(BORDER_MEDIUM)` |

## Shadows

CSS source: `--shadow-sm/md/lg`, overridden by theme.

| Token | Dark | Light | GPUI mapping |
|---|---|---|---|
| shadow-sm | `0 1px 2px rgba(5,3,6,.18)` | `0 1px 2px rgba(46,34,49,.08)` | proposed `shadow_sm(c::SHADOW())` helper |
| shadow-md | `0 4px 12px rgba(5,3,6,.22)` | `0 4px 12px rgba(46,34,49,.11)` | proposed `shadow_md(c::SHADOW())` helper |
| shadow-lg | `0 10px 28px rgba(5,3,6,.28)` | `0 10px 24px rgba(46,34,49,.14)` | proposed `shadow_lg(c::SHADOW())` helper |

Dark depth uses color and seams first. Shadows are nearly invisible by design.

## Motion

| Token | Value | GPUI mapping |
|---|---|---|
| duration-fast/base/slow | 80 / 140 / 220ms | `Duration::from_millis(DURATION_*)` |
| ease-standard | `cubic-bezier(.2,.8,.2,1)` | `gpui_component::animation::cubic_bezier(.2,.8,.2,1.)` |
| ease-decelerate | `cubic-bezier(.16,1,.3,1)` | `cubic_bezier(.16,1.,.3,1.)` |
| ease-accelerate | `cubic-bezier(.4,0,1,1)` | `cubic_bezier(.4,0.,1.,1.)` |

Use `gpui::Animation::new(Duration::from_millis(...)).with_easing(...)`. Reduced motion uses app-level setting/media equivalent, 0ms, and skips transform animations. No bounce.

## Z-index

CSS source: `--z-base/raised/sticky/overlay/modal/toast`.

| Token | Order | GPUI mapping |
|---|---:|---|
| base / raised / sticky / overlay / modal / toast | 0 / 10 / 20 / 40 / 60 / 80 | semantic paint order or overlay insertion order only; GPUI has no CSS z-index property |

## Icon sizes

CSS source: `--icon-*` and `--dot-*`.

| Token | px | GPUI mapping |
|---|---:|---|
| icon-10/12/14/16/20/24/32 | 10,12,14,16,20,24,32 | `.size(rpx(ICON_*))` |
| dot-6/7/8 | 6,7,8 | `.size(rpx(DOT_*))` |

## Opacity

CSS source: `--opacity-*`.

| Token | Value | GPUI mapping |
|---|---:|---|
| disabled / muted / hover / pressed / scrim | .38 / .62 / .08 / .14 / .72 | `c::alpha(token, value)` or `.opacity(value)` where supported |

## Blur

CSS source: `--blur-*`.

| Token | px | GPUI mapping |
|---|---:|---|
| blur-0/4/8/16 | 0,4,8,16 | proposed blur helper; rare, because Grove has no glass surfaces |

## Assumptions

| Assumption | Record | GPUI mapping / restriction |
|---|---|---|
| Palette | Moodboard values were visually inferred, then regularized around approved Smoked plum values. | Recheck on calibrated displays. |
| Themes | Dark received design scrutiny. Light is a warm derived accessibility/reference counterpart. | Do not restore theme proliferation. |
| Typography | IBM Plex Sans and Blex Mono Nerd Font Mono are actual bundled product families and substitute for the moodboard's neutral grotesk/mono character. | Load via `ui()` / `mono()`. |
| Units | 1rem is 16 design px; design px become `rpx()` except explicit physical seams. | Never use raw numeric component values. |
| Spacing/radius | Regularized, tidy rather than pixel-traced. 2px and 6px are deliberate dense half-steps. | Constants in `src/views/tokens.rs`. |
| Motion, z, opacity, blur | Defaults because static screenshots cannot specify them. | Reduced motion is mandatory; blur is rare. |
| Shadows | Defaulted and intentionally subtle. | Prefer color/seams. |
| Contrast dark | brand/bg 5.52:1; primary/bg 14.86:1; muted/bg 3.25:1; on-brand/brand 5.81:1. | Muted is metadata or large/nonessential text only, never required body copy. |
| Contrast light | brand/bg 6.28:1; primary/bg 15.47:1; muted/bg 4.87:1; on-brand/brand 6.43:1. | All listed normal-text pairs pass AA. |
| Source of truth | Markdown is a hand-copy. | `DESIGN.html` generated source wins on conflict. |
