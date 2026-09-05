# Grove design system contract

Status: approved dark-first target specification for the native Rust and GPUI app, updated from the form moodboard. Light uses white surfaces. Rust migration is pending.

One semantic scheme drives both appearances. Dark is the product default; light is its derived counterpart, not a separate theme family.

`DESIGN.html` CSS tokens drive its rendered catalog; this Markdown mirrors the specification. Changed and new Rust mappings are targets pending source migration.

## Primitives

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| neutral-0 | `#ffffff` | add: `c::NEUTRAL_0()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| neutral-50 | `#f7f7f8` | add: `c::NEUTRAL_50()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| neutral-100 | `#ededee` | add: `c::NEUTRAL_100()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| neutral-200 | `#dedee0` | add: `c::NEUTRAL_200()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| neutral-300 | `#c3c3c7` | add: `c::NEUTRAL_300()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| neutral-400 | `#9a9aa0` | add: `c::NEUTRAL_400()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| neutral-500 | `#707078` | add: `c::NEUTRAL_500()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| neutral-600 | `#515158` | add: `c::NEUTRAL_600()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| neutral-700 | `#34343a` | add: `c::NEUTRAL_700()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| neutral-800 | `#232327` | add: `c::NEUTRAL_800()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| neutral-850 | `#1b1b1e` | add: `c::NEUTRAL_850()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| neutral-900 | `#141416` | add: `c::NEUTRAL_900()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| neutral-950 | `#0d0d0f` | add: `c::NEUTRAL_950()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| warm-white | `#fbfaf7` | add: `c::WARM_WHITE()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| violet-400 | `#a78bfa` | add: `c::VIOLET_400()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| violet-500 | `#8b5cf6` | add: `c::VIOLET_500()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| violet-600 | `#7c3aed` | add: `c::VIOLET_600()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| green-500 | `#78d98b` | add: `c::GREEN_500()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| amber-500 | `#e0ad63` | add: `c::AMBER_500()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| red-500 | `#ef7d8e` | add: `c::RED_500()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| blue-500 | `#79a9e8` | add: `c::BLUE_500()` — Add immutable palette primitive; do not alias to a theme-dependent status accessor. |

Violet is limited to agent identity, connection state, and active ports. Green means success or running. Amber means needs-you or warning. Red means destructive or error. Blue means tertiary information.

## Semantic light and dark

| Token | Dark | Light | Meaning | Rust / GPUI mapping |
|---|---|---|---|---|
| field-fill | `#1b1b1b` | `#efefef` | filled form controls | add: `c::FIELD_FILL()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| switch-on | `#39b76c` | `#39b76c` | enabled switch track | add: `c::SWITCH_ON()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| switch-thumb | `#ffffff` | `#ffffff` | switch thumb | add: `c::SWITCH_THUMB()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| category-dot | `#79a9e8` | `#79a9e8` | category indicator; pair with text | add: `c::CATEGORY_DOT()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| error-wash | `rgba(239,125,142,.09)` | `rgba(182,56,76,.09)` | attached validation backing | add: `c::ERROR_WASH()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| bg | `#000000` | `#ffffff` | main application background | adapt: `c::BG()` — Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| bg-subtle | `#000000` | `#f7f7f8` | appbar and statusbar | adapt: `c::BG_STRIP()` — Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| surface | `#000000` | `#ffffff` | sidebar, panels, and bounded regions | add: `c::SURFACE()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| surface-raised | `#1b1b1b` | `#efefef` | menus, dialogs, and floating panels | add: `c::SURFACE_RAISED()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| hover | `#232327` | `#ededee` | neutral pointer hover | adapt: `c::BG_HOVER()` — Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| selected | `#232327` | `#ededee` | neutral selected row fill | adapt: `c::BG_HL()` — Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| text-primary | `#f7f7f8` | `#141416` | required copy and primary labels | adapt: `c::FG()` — Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| text-secondary | `#aaaab2` | `#66666d` | supporting copy and embedded labels | adapt: `c::FG_DIM()` — Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| text-muted | `#707078` | `#707078` | nonessential metadata | adapt: `c::FG_MUTE()` — Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| brand | `#f7f7f8` | `#141416` | Grove mark and high-contrast brand fill | add: `c::BRAND()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| brand-hover | `#ededee` | `#232327` | hover for brand-filled actions | add: `c::BRAND_HOVER()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| on-brand | `#141416` | `#f7f7f8` | text and icons on brand | add: `c::ON_BRAND()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| inverse-cta-bg | `#f7f7f8` | `#141416` | dominant inverse call to action | add: `c::INVERSE_CTA_BG()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| inverse-cta-text | `#141416` | `#f7f7f8` | content on inverse call to action | add: `c::INVERSE_CTA_TEXT()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| accent | `#8b5cf6` | `#7c3aed` | agent identity, connection, and active port | adapt: `c::MAGENTA()` — Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| accent-hover | `#a78bfa` | `#8b5cf6` | hover for violet-only roles | add: `c::ACCENT_HOVER()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| accent-pressed | `#7c3aed` | `#7c3aed` | pressed violet-only roles | add: `c::ACCENT_PRESSED()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| focus | `#f7f7f8` | `#141416` | neutral keyboard focus and selected-tile outline | adapt: `c::SEL_RING()` — Existing SEL_RING derives cyan; replace with neutral dark/light focus. Color alone does not implement ring geometry. |
| accent-wash | `rgba(139,92,246,.14)` | `rgba(124,58,237,.10)` | quiet agent or connection backing | adapt: `c::SEL_TINT_SOFT()` — Existing SEL_TINT_SOFT uses legacy color/alpha; match violet .14 dark / .10 light. |
| running / success | `#78d98b` | `#78d98b` | running process or completed action | adapt: `c::GREEN()` — Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| needs-you / warning | `#e0ad63` | `#e0ad63` | required input or caution | adapt: `c::AMBER()` — Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| destructive / error | `#ef7d8e` | `#b6384c` | destructive action or failure | adapt: `c::RED()` — Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| tertiary / info | `#79a9e8` | `#79a9e8` | tertiary information only | adapt: `c::BLUE()` — Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| border | `#34343a` | `#dedee0` | standard one-pixel seam | adapt: `c::BORDER()` — Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| border-strong | `#515158` | `#c3c3c7` | emphasized structure | add: `c::BORDER_STRONG()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| divider-soft | `rgba(247,247,248,.08)` | `rgba(20,20,22,.09)` | quiet internal divider | adapt: `c::BORDER_SOFT()` — Existing BORDER_SOFT is an opaque mix; replace with alpha-bearing foreground, preserving .08 dark / .09 light. |
| scrim | `rgba(13,13,15,.76)` | `rgba(20,20,22,.38)` | neutral veil source, reduced with an opacity token | adapt: `c::SCRIM()` — Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| terminal-bg | `#0d0d0f` | `#ffffff` | PTY background | add: `c::TERMINAL_BG()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. Coordinate PTY palette separately; preserve cell metrics and ANSI contrast. |
| terminal-text | `#ededee` | `#232327` | PTY foreground | add: `c::TERMINAL_TEXT()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. Coordinate PTY palette separately; preserve cell metrics and ANSI contrast. |

Components use semantic accessors only. Neutral selection never becomes violet. Violet remains reserved for agent identity, connection state, and active ports.

## Typography

### Families

| Token | CSS family | Use | Rust / GPUI mapping |
|---|---|---|---|
| font-ui | `-apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif` | system UI and all form values | adapt: `gpui::Font::default()` — System UI target: current ui() uses IBM Plex Sans. Update root font and ui(), retaining bold/medium weights. CSS fallback stack is platform-specific. |
| font-mono | `"BlexMono Nerd Font Mono", "IBM Plex Mono", monospace` | terminal, code metadata, keycap, status | verified: `gpui::font(crate::fonts::MONO_FAMILY)` — BlexMono primary family is bundled. CSS fallback stack is not reproduced automatically. PTY remains FONT_SIZE=12.5, CELL_W=7.5, CELL_H=17. |

System sans is the target UI family, including paths and branches inside forms. Blex Mono remains for terminal and code metadata. Existing bundled UI font usage requires source migration.

### Scale

`1rem = 16 design px.`

| Token | px / rem | Line height | Default weight | Rust / GPUI mapping |
|---|---:|---:|---:|---|
| text-10 | 10 / .625rem | 13px | 500 | verified: `.text_size(rpx(TEXT_MICRO))` — Existing size matches; ui()/mono() do not set explicit line height. |
| text-11 | 11 / .6875rem | 15px | 400 | verified: `.text_size(rpx(TEXT_SMALL))` — Existing size matches; ui()/mono() do not set explicit line height. |
| text-12 | 12 / .75rem | 17px | 400 | verified: `.text_size(rpx(TEXT_BODY))` — Existing size matches; ui()/mono() do not set explicit line height. |
| text-13 | 13 / .8125rem | 19px | 400 | verified: `.text_size(rpx(TEXT_TITLE))` — Existing size matches; ui()/mono() do not set explicit line height. |
| text-14 | 14 / .875rem | 20px | 400 | add: `.text_size(rpx(TEXT_14))` — Add TEXT_14 = 14.0; TEXT_DISPLAY is 20, not 24. Set line height separately. |
| text-16 | 16 / 1rem | 22px | 400 | add: `.text_size(rpx(TEXT_16))` — Add TEXT_16 = 16.0; TEXT_DISPLAY is 20, not 24. Set line height separately. |
| text-15 | 15 / .9375rem | 20px | 600 | add: `.text_size(rpx(TEXT_15))` — Add TEXT_15 = 15.0; TEXT_DISPLAY is 20, not 24. Set line height separately. |
| text-18 | 18 / 1.125rem | 23px | 600 | add: `.text_size(rpx(TEXT_18))` — Add TEXT_18 = 18.0; TEXT_DISPLAY is 20, not 24. Set line height separately. |
| text-24 | 24 / 1.5rem | 29px | 700 | add: `.text_size(rpx(TEXT_24))` — Add TEXT_24 = 24.0; TEXT_DISPLAY is 20, not 24. Set line height separately. |
| text-32 | 32 / 2rem | 36px | 700 | verified: `.text_size(rpx(TEXT_DISPLAY_LG))` — Existing size matches; ui()/mono() do not set explicit line height. |

Embedded labels use `line-field-label = 16px` with `text-12`, regular weight. Values use `line-field-value = 22px` with `text-16`. Both line-height aliases are targets pending source migration.

### Weights

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| weight-regular | 400 | verified: `gpui::FontWeight::NORMAL` — Apply with .font_weight(...); ensure the chosen font supplies or synthesizes the weight. |
| weight-medium | 500 | verified: `gpui::FontWeight::MEDIUM` — Apply with .font_weight(...); ensure the chosen font supplies or synthesizes the weight. |
| weight-semibold | 600 | verified: `gpui::FontWeight::SEMIBOLD` — Apply with .font_weight(...); ensure the chosen font supplies or synthesizes the weight. |
| weight-bold | 700 | verified: `gpui::FontWeight::BOLD` — Apply with .font_weight(...); ensure the chosen font supplies or synthesizes the weight. |

### Type roles

| Role | Family | Size | Weight | Use | Rust / GPUI mapping |
|---|---|---:|---:|---|---|
| embedded label | UI | 12 / 16px line | 400 | supporting field label | Composition target; combine audited family, size, explicit line-height, weight and semantic color rows. PTY uses existing cell metrics. |
| form value | UI | 16 / 22px line | 400 | all input values, including paths | Composition target; combine audited family, size, explicit line-height, weight and semantic color rows. PTY uses existing cell metrics. |
| brand mark | UI | 15 | 700 | GROVE wordmark | Composition target; combine audited family, size, explicit line-height, weight and semantic color rows. PTY uses existing cell metrics. |
| workspace trigger | UI | 12 | 500 | active workspace button | Composition target; combine audited family, size, explicit line-height, weight and semantic color rows. PTY uses existing cell metrics. |
| section heading | UI | 18 | 600 | page and panel headings | Composition target; combine audited family, size, explicit line-height, weight and semantic color rows. PTY uses existing cell metrics. |
| row label | UI | 12 | 500 | project, worktree, and session | Composition target; combine audited family, size, explicit line-height, weight and semantic color rows. PTY uses existing cell metrics. |
| body | UI | 13 | 400 | guidance and dialog copy | Composition target; combine audited family, size, explicit line-height, weight and semantic color rows. PTY uses existing cell metrics. |
| metadata | Mono | 11 | 400 | path and branch | Composition target; combine audited family, size, explicit line-height, weight and semantic color rows. PTY uses existing cell metrics. |
| status | Mono | 10 | 500 | statusbar and counters | Composition target; combine audited family, size, explicit line-height, weight and semantic color rows. PTY uses existing cell metrics. |
| terminal | Mono | 12 | 400 | PTY text | Composition target; combine audited family, size, explicit line-height, weight and semantic color rows. PTY uses existing cell metrics. |
| display | UI | 32 | 700 | sparse empty-state title | Composition target; combine audited family, size, explicit line-height, weight and semantic color rows. PTY uses existing cell metrics. |

## Forms and inputs

Grove forms use flat filled controls on black or white surfaces. Each field is 60px tall with a 12px regular label above a 16px value; label and value line heights are 16px and 22px. Labels use `text-secondary`, never the quiet metadata token. All form values use system sans, including paths and branches.

| Pattern | Anatomy | States | GPUI mapping |
|---|---|---|---|
| Embedded-label field | 60px field-fill well, 12px radius, label above value | empty, filled, focused, invalid, disabled, readonly | Target (pending): `FIELD_H`, `RADIUS_12`, `c::FIELD_FILL()` |
| Compound field | optional semantic icon, input/select/textarea, trailing action | native select, split values, textarea, date/time | Target (pending): form geometry and system UI value role; keep catalog icons |
| Attached validation | invalid field joins a local message below with a shared border and error-wash backing | single and multiple errors | Target (pending): error text and border use `c::RED()`; light error is `#b6384c` |
| Grouped switches | quiet outlined 16px group; 48×28 track, 22px white thumb, 3px inset | on, off, disabled | Target (pending): `SWITCH_*`, `c::SWITCH_ON()`, `c::SWITCH_THUMB()` |
| Selection tiles | min 42×44, 12px radius, neutral 2px total selected outline | single or multiple, selected/unselected | Target (pending): `TILE_*`, `c::SEL_RING()`; blue category dot pairs with visible text |
| Form buttons | 44px height, 12px radius, leading semantic icon | normal, focus, disabled, pending | Target (pending): `FORM_BUTTON_H`; dense chrome remains target `CONTROL_H = 24` (current value: 22) |

Use 14px horizontal inset, 8px label top inset, and a 27px value top inset. Rows and split fields use a 12px rhythm. Textareas start at 130px and grow vertically. Focus uses a 1px border plus a 1px outline, totaling 2px, in the neutral focus token. Fields and cards use no bevel, inset shadow, or default elevation.

Readonly values remain selectable. Disabled controls use the disabled opacity and cannot accept input; labels in enabled and readonly controls retain normal supporting contrast. Split fields are only for semantic pairs. Validation stays attached in normal flow and does not become a detached alert card.

## Radius

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| radius-4 | 4px | verified: `.rounded(rpx(RADIUS_CONTROL))` — Existing radius matches. |
| radius-8 | 8px | add: `.rounded(rpx(RADIUS_8))` — Add RADIUS_8 = 8.0; RADIUS_GROUP is 6 and cannot substitute. |
| radius-12 | 12px | verified: `.rounded(rpx(RADIUS_PANEL))` — Existing radius matches. |
| radius-16 | 16px, outlined groups | add: `.rounded(rpx(RADIUS_16))` — Add RADIUS_16 = 16.0; RADIUS_GROUP is 6 and cannot substitute. |
| radius-full | full pill | verified: `.rounded_full()` — Existing radius matches. |

Controls use radius-12; grouped preferences use radius-16; compact menus and field actions use radius-8.

## Spacing

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| space-2 | 2px | verified: `rpx(SPACE_XS)` — Existing named spacing matches. |
| space-4 | 4px | verified: `rpx(SPACE_SM)` — Existing named spacing matches. |
| space-6 | 6px | verified: `rpx(SPACE_MD)` — Existing named spacing matches. |
| space-8 | 8px | verified: `rpx(SPACE_LG)` — Existing named spacing matches. |
| space-10 | 10px | verified: `rpx(SPACE_XL)` — Existing named spacing matches. |
| space-12 | 12px | verified: `rpx(SPACE_2XL)` — Existing named spacing matches. |
| space-16 | 16px | verified: `rpx(SPACE_3XL)` — Existing named spacing matches. |
| space-20 | 20px | add: `rpx(SPACE_20)` — Add SPACE_20 = 20.0; numeric SPACE_* names are not existing constants. |
| space-24 | 24px | add: `rpx(SPACE_24)` — Add SPACE_24 = 24.0; numeric SPACE_* names are not existing constants. |
| space-32 | 32px | add: `rpx(SPACE_32)` — Add SPACE_32 = 32.0; numeric SPACE_* names are not existing constants. |
| space-40 | 40px | add: `rpx(SPACE_40)` — Add SPACE_40 = 40.0; numeric SPACE_* names are not existing constants. |
| space-48 | 48px | add: `rpx(SPACE_48)` — Add SPACE_48 = 48.0; numeric SPACE_* names are not existing constants. |

Geometry is part of the spacing contract.

| Geometry token | Value | Rust / GPUI mapping |
|---|---:|---|
| field-h | 60px | add: `rpx(FIELD_H)` — Add FIELD_H = 60.0; geometry constant absent. |
| form-button-h | 44px | add: `rpx(FORM_BUTTON_H)` — Add FORM_BUTTON_H = 44.0; geometry constant absent. |
| field-inset-x | 14px | add: `rpx(FIELD_INSET_X)` — Add FIELD_INSET_X = 14.0; geometry constant absent. |
| field-inset-top | 8px | add: `rpx(FIELD_INSET_TOP)` — Add FIELD_INSET_TOP = 8.0; geometry constant absent. |
| field-value-top | 27px | add: `rpx(FIELD_VALUE_TOP)` — Add FIELD_VALUE_TOP = 27.0; geometry constant absent. |
| textarea-h | 130px | add: `rpx(TEXTAREA_H)` — Add TEXTAREA_H = 130.0; geometry constant absent. |
| switch-w | 48px | add: `rpx(SWITCH_W)` — Add SWITCH_W = 48.0; geometry constant absent. |
| switch-h | 28px | add: `rpx(SWITCH_H)` — Add SWITCH_H = 28.0; geometry constant absent. |
| switch-thumb | 22px | add: `rpx(SWITCH_THUMB)` — Add SWITCH_THUMB = 22.0; geometry constant absent. |
| switch-inset | 3px | add: `rpx(SWITCH_INSET)` — Add SWITCH_INSET = 3.0; geometry constant absent. |
| tile-min-w | 42px | add: `rpx(TILE_MIN_W)` — Add TILE_MIN_W = 42.0; geometry constant absent. |
| tile-h | 44px | add: `rpx(TILE_H)` — Add TILE_H = 44.0; geometry constant absent. |
| category-dot-size | 8px | add: `rpx(CATEGORY_DOT_SIZE)` — Add CATEGORY_DOT_SIZE = 8.0; geometry constant absent. |
| appbar-h | 40px | add: `rpx(APPBAR_H)` — Add APPBAR_H = 40.0; geometry constant absent. |
| sidebar-w | 236px | add: `rpx(SIDEBAR_W)` — Add SIDEBAR_W = 236.0; geometry constant absent. |
| header-h | 36px | add: `rpx(HEADER_H)` — Add HEADER_H = 36.0; geometry constant absent. |
| status-h | 26px | add: `rpx(STATUS_H)` — Add STATUS_H = 26.0; geometry constant absent. |
| row-h | 28px | add: `rpx(ROW_H)` — Add ROW_H = 28.0; geometry constant absent. |
| control-h | 24px | adapt: `rpx(CONTROL_H)` — Current CONTROL_H = 22.0; target 24px. Audit all dependent geometry. |

Workspace switching is appbar content and owns no separate width token.

## Gaps

| Token | Alias | Rust / GPUI mapping |
|---|---|---|
| gap-2 | space-2 | verified: `.gap(rpx(SPACE_XS))` — Alias to --space-2; no separate GAP_* constant exists. |
| gap-4 | space-4 | verified: `.gap(rpx(SPACE_SM))` — Alias to --space-4; no separate GAP_* constant exists. |
| gap-6 | space-6 | verified: `.gap(rpx(SPACE_MD))` — Alias to --space-6; no separate GAP_* constant exists. |
| gap-8 | space-8 | verified: `.gap(rpx(SPACE_LG))` — Alias to --space-8; no separate GAP_* constant exists. |
| gap-12 | space-12 | verified: `.gap(rpx(SPACE_2XL))` — Alias to --space-12; no separate GAP_* constant exists. |
| gap-16 | space-16 | verified: `.gap(rpx(SPACE_3XL))` — Alias to --space-16; no separate GAP_* constant exists. |

## Borders

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| border-thin | 1 logical px | verified: `.border_1()` — One unscaled logical pixel, not one device pixel. Apply semantic border color separately. |
| border-medium | 2px | verified: `.border_2()` — Two unscaled logical pixels; CSS px border stays unscaled by app rem zoom. Apply semantic border color separately. |
| focus-ring | 2px total | add: `Add shared focus-ring composition: 1 logical px border + 1 logical px outer ring` — Do not substitute a 2px layout border: preserve outside ring and content geometry. Neutral c::SEL_RING also needs migration. |

## Shadows

| Token | Dark | Light | Rust / GPUI mapping |
|---|---|---|---|
| shadow-none | `none` | `none` | verified: `No BoxShadow` — Flat forms/cards: omit shadow. |
| shadow-sm | `0 1px 2px rgba(0,0,0,.36)` | `0 1px 2px rgba(13,13,15,.10)` | add: `gpui::BoxShadow { color: c::SHADOW_SM(), offset: gpui::point(gpui::px(0.), rpx(SHADOW_SM_Y).to_pixels(window.rem_size())), blur_radius: rpx(SHADOW_SM_BLUR).to_pixels(window.rem_size()), spread_radius: gpui::px(0.), inset: false }` — Add SHADOW_SM_Y = 1.0, SHADOW_SM_BLUR = 2.0 and theme alpha accessor. BoxShadow requires Pixels, so convert rem-scaled design dimensions with current rem_size. Hairlines remain unscaled logical px. Do not reuse PANEL_SHADOW. |
| shadow-md | `0 8px 24px rgba(0,0,0,.42)` | `0 8px 24px rgba(13,13,15,.14)` | add: `gpui::BoxShadow { color: c::SHADOW_MD(), offset: gpui::point(gpui::px(0.), rpx(SHADOW_MD_Y).to_pixels(window.rem_size())), blur_radius: rpx(SHADOW_MD_BLUR).to_pixels(window.rem_size()), spread_radius: gpui::px(0.), inset: false }` — Add SHADOW_SM_Y = 1.0, SHADOW_SM_BLUR = 2.0 and theme alpha accessor. BoxShadow requires Pixels, so convert rem-scaled design dimensions with current rem_size. Hairlines remain unscaled logical px. Do not reuse PANEL_SHADOW. |
| shadow-lg | `0 16px 48px rgba(0,0,0,.50)` | `0 16px 48px rgba(13,13,15,.18)` | add: `gpui::BoxShadow { color: c::SHADOW_LG(), offset: gpui::point(gpui::px(0.), rpx(SHADOW_LG_Y).to_pixels(window.rem_size())), blur_radius: rpx(SHADOW_LG_BLUR).to_pixels(window.rem_size()), spread_radius: gpui::px(0.), inset: false }` — Add SHADOW_SM_Y = 1.0, SHADOW_SM_BLUR = 2.0 and theme alpha accessor. BoxShadow requires Pixels, so convert rem-scaled design dimensions with current rem_size. Hairlines remain unscaled logical px. Do not reuse PANEL_SHADOW. |

Fields and cards default to shadow-none. No bevels or inset shadows. Surface steps and borders establish hierarchy; the other shadow tokens are reserved for exceptional floating separation.

## Motion

| Token | Value | Rust / GPUI mapping |
|---|---|---|
| duration-fast | 80ms | verified: `std::time::Duration::from_millis(80)` — Duration API exists; add shared motion tokens and reduced-motion policy. Duration::ZERO alone does not skip every animation path. |
| duration-base | 140ms | verified: `std::time::Duration::from_millis(140)` — Duration API exists; add shared motion tokens and reduced-motion policy. Duration::ZERO alone does not skip every animation path. |
| duration-slow | 220ms | verified: `std::time::Duration::from_millis(220)` — Duration API exists; add shared motion tokens and reduced-motion policy. Duration::ZERO alone does not skip every animation path. |
| ease-standard | `cubic-bezier(.2,.8,.2,1)` | add: `Add CSS bezier solver for cubic-bezier(.2,.8,.2,1); pass closure to gpui::Animation::with_easing` — gpui_component::animation::cubic_bezier in vendor/gpui-component/ui/src/animation.rs ignores computed x and returns y(progress). Exact CSS requires solving x(u)=progress before y(u); add an inverse-x solver. |
| ease-decelerate | `cubic-bezier(.16,1,.3,1)` | add: `Add CSS bezier solver for cubic-bezier(.16,1,.3,1); pass closure to gpui::Animation::with_easing` — gpui_component::animation::cubic_bezier in vendor/gpui-component/ui/src/animation.rs ignores computed x and returns y(progress). Exact CSS requires solving x(u)=progress before y(u); add an inverse-x solver. |
| ease-accelerate | `cubic-bezier(.4,0,1,1)` | add: `Add CSS bezier solver for cubic-bezier(.4,0,1,1); pass closure to gpui::Animation::with_easing` — gpui_component::animation::cubic_bezier in vendor/gpui-component/ui/src/animation.rs ignores computed x and returns y(progress). Exact CSS requires solving x(u)=progress before y(u); add an inverse-x solver. |

Reduced motion uses `Duration::ZERO` and skips transform-based transitions. Do not animate a transform when the reduced-motion preference is active.

## Z-index

| Token | Order | Rust / GPUI mapping |
|---|---:|---|
| z-base | 0 | reference: `No numeric z-index equivalent; normal child paint order` — CSS number is an ordering role, not a GPUI depth or Stack/Overlay API. Verify hit testing and focus separately. |
| z-raised | 10 | reference: `No numeric z-index equivalent; later sibling paint order` — CSS number is an ordering role, not a GPUI depth or Stack/Overlay API. Verify hit testing and focus separately. |
| z-sticky | 20 | reference: `No numeric z-index equivalent; paint after scroll content` — CSS number is an ordering role, not a GPUI depth or Stack/Overlay API. Verify hit testing and focus separately. |
| z-overlay | 40 | reference: `No numeric z-index equivalent; deferred anchored layer` — CSS number is an ordering role, not a GPUI depth or Stack/Overlay API. Verify hit testing and focus separately. |
| z-modal | 60 | reference: `No numeric z-index equivalent; deferred blocking layer with focus/input ownership` — CSS number is an ordering role, not a GPUI depth or Stack/Overlay API. Verify hit testing and focus separately. |
| z-toast | 80 | reference: `No numeric z-index equivalent; last nonblocking notification layer` — CSS number is an ordering role, not a GPUI depth or Stack/Overlay API. Verify hit testing and focus separately. |

GPUI has no CSS z-index property. These values are ordering semantics only.

## Icons

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| icon-12 | 12px | verified: `crate::icons::icon(name, ICON_SM, color)` — Existing size matches; icon() applies rpx once. Verify sprite path independently. |
| icon-14 | 14px | verified: `crate::icons::icon(name, ICON_MD, color)` — Existing size matches; icon() applies rpx once. Verify sprite path independently. |
| icon-16 | 16px | verified: `crate::icons::icon(name, ICON_LG, color)` — Existing size matches; icon() applies rpx once. Verify sprite path independently. |
| icon-20 | 20px | add: `crate::icons::icon(name, ICON_20, color)` — Add ICON_20 = 20.0; illustration only, not chrome. |
| icon-24 | 24px | add: `crate::icons::icon(name, ICON_24, color)` — Add ICON_24 = 24.0; illustration only, not chrome. |
| icon-32 | 32px | verified: `crate::icons::icon(name, ICON_DISPLAY, color)` — Existing size matches; icon() applies rpx once. Verify sprite path independently. |

Use one-pixel or 1.5-pixel optical strokes and `currentColor`. Violet icons are restricted to the violet semantic roles.

### Product icon catalog

All product icons use `viewBox="0 0 16 16"`, round caps and joins, and an optical 1.5 stroke. The SVG is `display: block` and `flex: none`. The control box, not the glyph, owns hover, focus, and click state: 24px in dense chrome and 44px for form actions.

| Icon | Meaning | Rust / GPUI mapping |
|---|---|---|
| chevron-right / chevron-down | collapsed / expanded disclosure | `crate::icons::icon("chev-right" or "chev-down", ICON_SM, current_color)` |
| plus | create or add | `crate::icons::icon("plus", ICON_MD, current_color)` |
| more | contextual actions | `crate::icons::icon("more", ICON_MD, current_color)` |
| close | dismiss or close | `crate::icons::icon("close", ICON_MD, current_color)` |
| folder | local project | `crate::icons::icon("folder", ICON_MD, current_color)` |
| branch | Git worktree or branch | `crate::icons::icon("branch", ICON_MD, current_color)` |
| list / grid | workspace view | `crate::icons::icon("list" or "grid", ICON_MD, current_color)` |
| check | completed or selected | `crate::icons::icon("check", ICON_MD, current_color)` |
| warning | needs-you or warning | `crate::icons::icon("warning", ICON_MD, current_color)` |
| Codex | Codex agent identity | proposed `crate::icons::icon("codex", ICON_MD, c::MAGENTA())` |
| Claude Code | Claude Code agent identity | proposed `crate::icons::icon("claude", ICON_MD, c::MAGENTA())` |
| Terminal | terminal agent identity | `crate::icons::icon("term", ICON_MD, c::MAGENTA())` |
| sun / moon | switch to light / dark appearance | `crate::icons::icon("sun" or "moon", ICON_MD, current_color)` |
| trash | destructive delete or remove | `crate::icons::icon("trash", ICON_MD, c::RED())` |
| refresh | retry | `crate::icons::icon("refresh", ICON_MD, current_color)` |
| play | start a session or process | `crate::icons::icon("play", ICON_MD, current_color)` |

Use 12, 14, and 16px glyphs in controls and dense chrome. Reserve 20, 24, and 32px glyphs for illustrations and sparse empty states. Do use a real icon inside the standard 24px button box. Do not substitute text characters, mix viewBox sizes, add decorative wrapper boxes, place glyphs on violet squares, stretch glyphs to fill controls, or use violet for non-agent decoration.

### Button icon rule
Project and List share one borderless 24px destination toggle. Project shows the List icon and “Switch to list view”; List shows the folder icon and “Switch to project view.” Grid is a separate borderless 24px button labeled “Open grid view.” In Grid, the toggle shows the remembered non-grid destination. Use a 1-token gap, 14px glyphs, transparent default, hover fill, neutral selected fill, and focus ring. No segmented outline or visible mode text.


Action buttons carry a catalog icon. Switches and selection tiles communicate their state directly and do not require a decorative action icon. Text action buttons use a leading icon by default; only disclosure and forward navigation place a directional icon last. Icon-only buttons require an `aria-label` and tooltip. When visible text exists, that text is the accessible name and the SVG stays `aria-hidden="true"`. Loading replaces or animates the leading icon without removing its slot, and disabled buttons keep their icon. Use plus for add/create, check for confirm/approve/save, close for cancel/close, trash for delete/remove, refresh for retry, folder/list/grid for views, plus or play for new/start, more for manage, folder plus chevron-down for workspace triggers, chevrons for disclosure, and sun/moon for the appearance toggle.

## Opacity

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| opacity-disabled | .58 | verified: `.opacity(.58)` — Element opacity composes the subtree and does not disable input. For one paint use color.opacity(factor); c::alpha replaces alpha. |
| opacity-muted | .62 | verified: `.opacity(.62)` — Element opacity composes the subtree and does not disable input. For one paint use color.opacity(factor); c::alpha replaces alpha. |
| opacity-hover | .08 | verified: `.opacity(.08)` — Element opacity composes the subtree and does not disable input. For one paint use color.opacity(factor); c::alpha replaces alpha. |
| opacity-pressed | .14 | verified: `.opacity(.14)` — Element opacity composes the subtree and does not disable input. For one paint use color.opacity(factor); c::alpha replaces alpha. Quiet veil uses this .14 over scrim .76 dark / .38 light, giving .1064 / .0532 effective alpha. |
| opacity-scrim | .76 | verified: `.opacity(.76)` — Element opacity composes the subtree and does not disable input. For one paint use color.opacity(factor); c::alpha replaces alpha. This standalone .76 token is not the quiet veil multiplier: current COMPONENTS/screens use --opacity-pressed (.14) over scrim .76 dark / .38 light, giving .1064 / .0532 effective alpha. |

### Overlay behavior

Blocking confirmation stays attached to the row, tile, switcher, or manager action that opened it. A low-opacity neutral veil makes the underlying app inert while a crisp focus outline preserves the source aperture. The confirmation tray shares the source edge, uses one neutral seam, and reads as a temporary inspector or command tray. It is never centered, blurred, blacked out, decorated with a colored edge, or divided into header and footer bands. Consequences remain inline key-value text.

Workspace creation is an anchored switcher popover that morphs between normal, loading, and contextual error without moving. Workspace deletion expands in place inside the manager or popover as a danger confirmation row. Escape and Cancel close without state change; focus returns to the source control. GPUI composes these with `gpui::deferred(gpui::anchored())`, normal paint order, a neutral veil layer, and a new neutral focus-ring composition.

## Blur

| Token | Value | Rust / GPUI mapping |
|---|---:|---|
| blur-0 | 0px | reference: `No filter; paint directly` — Product overlays never blur. No platform compositor helper exists in Grove; do not invent one. |
| blur-4 | 4px | reference: `No core GPUI CSS blur equivalent; reference only` — Product overlays never blur. No platform compositor helper exists in Grove; do not invent one. |
| blur-8 | 8px | reference: `No core GPUI CSS blur equivalent; reference only` — Product overlays never blur. No platform compositor helper exists in Grove; do not invent one. |
| blur-16 | 16px | reference: `No core GPUI CSS blur equivalent; reference only` — Product overlays never blur. No platform compositor helper exists in Grove; do not invent one. |

Blur is not a surface-building tool in Grove. Product overlays never use it; these values remain reserved for platform-level needs outside this interaction contract.

## Assumptions

Forms use 60px flat filled wells, 12px embedded labels, and attached validation with a shared error border and wash. Neutral focus outlines identify active controls. Existing compact source-anchored popovers and repository Browse behavior remain interaction references; the new form geometry is a target pending implementation.

| Assumption | Record | GPUI mapping or restriction |
|---|---|---|
| Source | The approved form moodboard sets surfaces, focus, typography, radius, and form geometry. Existing dense workspace chrome retains its dimensions. | `DESIGN.html` CSS tokens drive rendered tables; this Markdown mirrors them. |
| Themes | Dark received primary design scrutiny. Light is a derived white counterpart. | Both resolve through the same `c::*()` semantic names. |
| Fonts | System sans is the target UI family; Blex Mono remains for terminal and code metadata. | Composition target; combine audited family, size, explicit line-height, weight and semantic color rows. PTY uses existing cell metrics. |
| Units | 1rem is 16 design px. Layout values pass through `rpx()`; hairlines use `px(1.)`. | Do not place bare layout numbers in components. |
| Regularization | Spacing and radius are tidy scales rather than traced values. | Add or rename constants in `src/views/tokens.rs` before component work. |
| New aliases | Changed values and new semantic names are target mappings pending source migration in `src/theme.rs` and token definitions. | Do not substitute component literals while aliases are pending. |
| Defaults | Forms and cards default to no shadow. Floating separation, motion, z order, opacity, and platform blur retain documented defaults. | Reduced motion is mandatory; blur stays rare. |
| Geometry | Appbar 40, sidebar 236, header 36, status 26, row 28, dense control 24; form field 60 and form button 44. | Workspace switching adds no separate side strip. |
| Support color text | Green, amber, red, and blue are role colors, not default copy colors. | On white, pair indicators with `c::FG()` text; destructive text uses the validated light error token. |
| Icon drawings | The 19 currentColor SVG paths regularize wireframe glyphs into one 16-unit grid. Codex and Claude marks are distinct geometric stand-ins until final brand artwork exists. | Add matching sprite names to `crate::icons`; keep 12/14/16 for chrome and 20/24/32 for illustration only. |
| Button icon coverage | Every action button uses the semantic icon catalog, including loading and disabled states. Switches and selection tiles are state controls and are exempt. | Keep icon slots stable when state changes and audit rendered buttons, not only source templates. |
| Icon restraint | Product icons carry meaning through the glyph and current text color. | No text-glyph substitutes, mixed viewBoxes, decorative wrapper boxes, or violet square backplates. |

### Computed contrast

| Pair | Dark | Light | Restriction |
|---|---:|---:|---|
| brand / bg | 19.61:1 | 18.40:1 | add: `c::BRAND()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| on-brand / brand | 17.18:1 | 17.18:1 | add: `c::ON_BRAND()` — Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| primary / bg | 19.61:1 | 18.40:1 | AAA for normal text |
| secondary / bg | 9.10:1 | 5.70:1 | AA for normal text |
| muted / bg | 4.28:1 | 4.91:1 | Quiet metadata only; never embedded labels |
| accent / bg | 4.96:1 | 5.70:1 | adapt: `c::MAGENTA()` — Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| label / field | 7.47:1 | 4.95:1 | AA for normal text |
| value / field | 16.09:1 | 16.00:1 | AAA for normal text |
| focus / field | 16.09:1 | 16.00:1 | adapt: `c::SEL_RING()` — Existing SEL_RING derives cyan; replace with neutral dark/light focus. Color alone does not implement ring geometry. |
| error / field | 6.56:1 | 4.99:1 | AA for normal text |

Ratios use WCAG relative luminance against final six-digit token values: background `#000000` / `#ffffff`, field fill `#1b1b1b` / `#efefef`. Status colors are indicators unless their text pairing is validated. Required labels use text-secondary.

## GPUI implementation contract

The exhaustive table below is mirrored from `var gpuiTokenMap` in `DESIGN.html`; regenerate the Markdown rows when that data changes. The CSS `:root` and light override blocks remain the visual authority. No Rust migration has been performed by this handoff.

`verified` means an existing value or GPUI API was found in source, not that the finished component matches. `adapt` marks an existing seam whose value or behavior differs. `add` identifies absent constants/accessors/helpers; their code strings are proposed interfaces, not compiling claims. `reference` identifies board-only or unsupported semantics. `c` means `crate::theme`; numeric constants belong in `src/views/tokens.rs`, and `rpx` is `crate::views::rpx`.

`rpx(v) = rems(v / 16)` and Workspace sets rem size to `16 * zoom`. Layout and type measured in design pixels therefore scale once. `gpui::px` creates logical pixels, not device pixels. Hairlines remain unscaled; shadow geometry uses `rpx(value).to_pixels(window.rem_size())` to convert design dimensions into the Pixels required by BoxShadow. Never pass an already-scaled value to `icon()`. Exact CSS unitless line-height multipliers produce fractional values (for example 12 × 1.417 = 17.004); the older rounded px labels are explanatory only. Apply explicit line height to UI labels; do not feed these values into PTY metrics.

Current `src/theme.rs` derives Tokyo Night colors. Existing names do not establish visual parity. `sync_component_theme` currently only synchronizes `muted_foreground`; the component library theme still requires a deliberate migration. Preserve PTY cell width/height/font metrics when changing the UI font to `Font::default()` system UI. Catalog icons below are adaptation seams: compare actual SVG paths with the HTML, including missing sprite keys; a function name does not prove glyph parity.

No CSS z-index, outline, blur, easing or accessibility attribute should be translated by inventing an API. The table records composition work explicitly. Reduced-motion preference plumbing and immediate final-state behavior are still implementation work. Border rings, attached overlays, focus restoration and blocking input require component-level verification.

| Exact CSS token | Dark / default | Light | GPUI target | Status | Source | Constraint |
|---|---|---|---|---|---|---|
| --neutral-0 | #ffffff | #ffffff | `c::NEUTRAL_0()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --neutral-50 | #f7f7f8 | #f7f7f8 | `c::NEUTRAL_50()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --neutral-100 | #ededee | #ededee | `c::NEUTRAL_100()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --neutral-200 | #dedee0 | #dedee0 | `c::NEUTRAL_200()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --neutral-300 | #c3c3c7 | #c3c3c7 | `c::NEUTRAL_300()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --neutral-400 | #9a9aa0 | #9a9aa0 | `c::NEUTRAL_400()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --neutral-500 | #707078 | #707078 | `c::NEUTRAL_500()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --neutral-600 | #515158 | #515158 | `c::NEUTRAL_600()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --neutral-700 | #34343a | #34343a | `c::NEUTRAL_700()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --neutral-800 | #232327 | #232327 | `c::NEUTRAL_800()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --neutral-850 | #1b1b1e | #1b1b1e | `c::NEUTRAL_850()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --neutral-900 | #141416 | #141416 | `c::NEUTRAL_900()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --neutral-950 | #0d0d0f | #0d0d0f | `c::NEUTRAL_950()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --warm-white | #fbfaf7 | #fbfaf7 | `c::WARM_WHITE()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --violet-400 | #a78bfa | #a78bfa | `c::VIOLET_400()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --violet-500 | #8b5cf6 | #8b5cf6 | `c::VIOLET_500()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --violet-600 | #7c3aed | #7c3aed | `c::VIOLET_600()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --green-500 | #78d98b | #78d98b | `c::GREEN_500()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --amber-500 | #e0ad63 | #e0ad63 | `c::AMBER_500()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --red-500 | #ef7d8e | #ef7d8e | `c::RED_500()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --blue-500 | #79a9e8 | #79a9e8 | `c::BLUE_500()` | add | src/theme.rs | Add immutable palette primitive; do not alias to a theme-dependent status accessor. |
| --color-board | #0d0d0f | #efeeeb | `No native equivalent; HTML catalog/flow canvas only` | reference | DESIGN.html | Do not use the board background as the application background. |
| --color-bg | #000000 | #ffffff | `c::BG()` | adapt | src/theme.rs | Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| --color-bg-subtle | #000000 | #f7f7f8 | `c::BG_STRIP()` | adapt | src/theme.rs | Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| --color-surface | #000000 | #ffffff | `c::SURFACE()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| --color-surface-raised | #1b1b1b | #efefef | `c::SURFACE_RAISED()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| --color-hover | #232327 | #ededee | `c::BG_HOVER()` | adapt | src/theme.rs | Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| --color-selected | #232327 | #ededee | `c::BG_HL()` | adapt | src/theme.rs | Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| --color-text-primary | #f7f7f8 | #141416 | `c::FG()` | adapt | src/theme.rs | Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| --color-text-secondary | #aaaab2 | #66666d | `c::FG_DIM()` | adapt | src/theme.rs | Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| --color-text-muted | #707078 | #707078 | `c::FG_MUTE()` | adapt | src/theme.rs | Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| --color-brand | #f7f7f8 | #141416 | `c::BRAND()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| --color-brand-hover | #ededee | #232327 | `c::BRAND_HOVER()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| --color-on-brand | #141416 | #f7f7f8 | `c::ON_BRAND()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| --color-inverse-cta-bg | #f7f7f8 | #141416 | `c::INVERSE_CTA_BG()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| --color-inverse-cta-text | #141416 | #f7f7f8 | `c::INVERSE_CTA_TEXT()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| --color-accent | #8b5cf6 | #7c3aed | `c::MAGENTA()` | adapt | src/theme.rs | Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| --color-accent-hover | #a78bfa | #8b5cf6 | `c::ACCENT_HOVER()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| --color-accent-pressed | #7c3aed | #7c3aed | `c::ACCENT_PRESSED()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| --color-focus | #f7f7f8 | #141416 | `c::SEL_RING()` | adapt | src/theme.rs | Existing SEL_RING derives cyan; replace with neutral dark/light focus. Color alone does not implement ring geometry. |
| --color-accent-wash | rgba(139,92,246,.14) | rgba(124,58,237,.10) | `c::SEL_TINT_SOFT()` | adapt | src/theme.rs | Existing SEL_TINT_SOFT uses legacy color/alpha; match violet .14 dark / .10 light. |
| --color-running | #78d98b | #78d98b | `c::GREEN()` | adapt | src/theme.rs | Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| --color-needs-you | #e0ad63 | #e0ad63 | `c::AMBER()` | adapt | src/theme.rs | Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| --color-destructive | #ef7d8e | #b6384c | `c::RED()` | adapt | src/theme.rs | Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| --color-tertiary | #79a9e8 | #79a9e8 | `c::BLUE()` | adapt | src/theme.rs | Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| --color-border | #34343a | #dedee0 | `c::BORDER()` | adapt | src/theme.rs | Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| --color-border-strong | #515158 | #c3c3c7 | `c::BORDER_STRONG()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| --color-divider-soft | rgba(247,247,248,.08) | rgba(20,20,22,.09) | `c::BORDER_SOFT()` | adapt | src/theme.rs | Existing BORDER_SOFT is an opaque mix; replace with alpha-bearing foreground, preserving .08 dark / .09 light. |
| --color-scrim | rgba(13,13,15,.76) | rgba(20,20,22,.38) | `c::SCRIM()` | adapt | src/theme.rs | Existing accessor resolves legacy theme values; migrate both appearances to this row. |
| --color-terminal-bg | #0d0d0f | #ffffff | `c::TERMINAL_BG()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. Coordinate PTY palette separately; preserve cell metrics and ANSI contrast. |
| --color-terminal-text | #ededee | #232327 | `c::TERMINAL_TEXT()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. Coordinate PTY palette separately; preserve cell metrics and ANSI contrast. |
| --shadow-sm | 0 1px 2px rgba(0,0,0,.36) | 0 1px 2px rgba(13,13,15,.10) | `gpui::BoxShadow { color: c::SHADOW_SM(), offset: gpui::point(gpui::px(0.), rpx(SHADOW_SM_Y).to_pixels(window.rem_size())), blur_radius: rpx(SHADOW_SM_BLUR).to_pixels(window.rem_size()), spread_radius: gpui::px(0.), inset: false }` | add | src/views/components.rs; GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/style.rs | Add SHADOW_SM_Y = 1.0, SHADOW_SM_BLUR = 2.0 and theme alpha accessor. BoxShadow requires Pixels, so convert rem-scaled design dimensions with current rem_size. Hairlines remain unscaled logical px. Do not reuse PANEL_SHADOW. |
| --shadow-md | 0 8px 24px rgba(0,0,0,.42) | 0 8px 24px rgba(13,13,15,.14) | `gpui::BoxShadow { color: c::SHADOW_MD(), offset: gpui::point(gpui::px(0.), rpx(SHADOW_MD_Y).to_pixels(window.rem_size())), blur_radius: rpx(SHADOW_MD_BLUR).to_pixels(window.rem_size()), spread_radius: gpui::px(0.), inset: false }` | add | src/views/components.rs; GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/style.rs | Add SHADOW_SM_Y = 1.0, SHADOW_SM_BLUR = 2.0 and theme alpha accessor. BoxShadow requires Pixels, so convert rem-scaled design dimensions with current rem_size. Hairlines remain unscaled logical px. Do not reuse PANEL_SHADOW. |
| --shadow-lg | 0 16px 48px rgba(0,0,0,.50) | 0 16px 48px rgba(13,13,15,.18) | `gpui::BoxShadow { color: c::SHADOW_LG(), offset: gpui::point(gpui::px(0.), rpx(SHADOW_LG_Y).to_pixels(window.rem_size())), blur_radius: rpx(SHADOW_LG_BLUR).to_pixels(window.rem_size()), spread_radius: gpui::px(0.), inset: false }` | add | src/views/components.rs; GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/style.rs | Add SHADOW_SM_Y = 1.0, SHADOW_SM_BLUR = 2.0 and theme alpha accessor. BoxShadow requires Pixels, so convert rem-scaled design dimensions with current rem_size. Hairlines remain unscaled logical px. Do not reuse PANEL_SHADOW. |
| --font-ui | -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif | -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif | `gpui::Font::default()` | adapt | src/fonts.rs; src/views/mod.rs | System UI target: current ui() uses IBM Plex Sans. Update root font and ui(), retaining bold/medium weights. CSS fallback stack is platform-specific. |
| --font-mono | "BlexMono Nerd Font Mono", "IBM Plex Mono", monospace | "BlexMono Nerd Font Mono", "IBM Plex Mono", monospace | `gpui::font(crate::fonts::MONO_FAMILY)` | verified | src/fonts.rs; src/views/mod.rs | BlexMono primary family is bundled. CSS fallback stack is not reproduced automatically. PTY remains FONT_SIZE=12.5, CELL_W=7.5, CELL_H=17. |
| --text-10 | .625rem | .625rem | `.text_size(rpx(TEXT_MICRO))` | verified | src/views/tokens.rs | Existing size matches; ui()/mono() do not set explicit line height. |
| --text-11 | .6875rem | .6875rem | `.text_size(rpx(TEXT_SMALL))` | verified | src/views/tokens.rs | Existing size matches; ui()/mono() do not set explicit line height. |
| --text-12 | .75rem | .75rem | `.text_size(rpx(TEXT_BODY))` | verified | src/views/tokens.rs | Existing size matches; ui()/mono() do not set explicit line height. |
| --text-13 | .8125rem | .8125rem | `.text_size(rpx(TEXT_TITLE))` | verified | src/views/tokens.rs | Existing size matches; ui()/mono() do not set explicit line height. |
| --text-14 | .875rem | .875rem | `.text_size(rpx(TEXT_14))` | add | src/views/tokens.rs | Add TEXT_14 = 14.0; TEXT_DISPLAY is 20, not 24. Set line height separately. |
| --text-16 | 1rem | 1rem | `.text_size(rpx(TEXT_16))` | add | src/views/tokens.rs | Add TEXT_16 = 16.0; TEXT_DISPLAY is 20, not 24. Set line height separately. |
| --line-14 | 1.429 | 1.429 | `.line_height(rpx(LINE_14))` | add | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; src/views/mod.rs | Add LINE_14 = 14.0 * 1.429 in src/views/tokens.rs. Preserve exact CSS fractional multiplier. ui()/mono() set no line height; PTY is excluded. |
| --line-16 | 1.375 | 1.375 | `.line_height(rpx(LINE_16))` | add | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; src/views/mod.rs | Add LINE_16 = 16.0 * 1.375 in src/views/tokens.rs. Preserve exact CSS fractional multiplier. ui()/mono() set no line height; PTY is excluded. |
| --line-field-label | 16px | 16px | `.line_height(rpx(LINE_FIELD_LABEL))` | add | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; src/views/mod.rs | Add LINE_FIELD_LABEL = 16.0 in src/views/tokens.rs. Preserve exact CSS fractional multiplier. ui()/mono() set no line height; PTY is excluded. |
| --line-field-value | 22px | 22px | `.line_height(rpx(LINE_FIELD_VALUE))` | add | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; src/views/mod.rs | Add LINE_FIELD_VALUE = 22.0 in src/views/tokens.rs. Preserve exact CSS fractional multiplier. ui()/mono() set no line height; PTY is excluded. |
| --text-15 | .9375rem | .9375rem | `.text_size(rpx(TEXT_15))` | add | src/views/tokens.rs | Add TEXT_15 = 15.0; TEXT_DISPLAY is 20, not 24. Set line height separately. |
| --text-18 | 1.125rem | 1.125rem | `.text_size(rpx(TEXT_18))` | add | src/views/tokens.rs | Add TEXT_18 = 18.0; TEXT_DISPLAY is 20, not 24. Set line height separately. |
| --text-24 | 1.5rem | 1.5rem | `.text_size(rpx(TEXT_24))` | add | src/views/tokens.rs | Add TEXT_24 = 24.0; TEXT_DISPLAY is 20, not 24. Set line height separately. |
| --text-32 | 2rem | 2rem | `.text_size(rpx(TEXT_DISPLAY_LG))` | verified | src/views/tokens.rs | Existing size matches; ui()/mono() do not set explicit line height. |
| --line-10 | 1.3 | 1.3 | `.line_height(rpx(LINE_10))` | add | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; src/views/mod.rs | Add LINE_10 = 10.0 * 1.3 in src/views/tokens.rs. Preserve exact CSS fractional multiplier. ui()/mono() set no line height; PTY is excluded. |
| --line-11 | 1.364 | 1.364 | `.line_height(rpx(LINE_11))` | add | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; src/views/mod.rs | Add LINE_11 = 11.0 * 1.364 in src/views/tokens.rs. Preserve exact CSS fractional multiplier. ui()/mono() set no line height; PTY is excluded. |
| --line-12 | 1.417 | 1.417 | `.line_height(rpx(LINE_12))` | add | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; src/views/mod.rs | Add LINE_12 = 12.0 * 1.417 in src/views/tokens.rs. Preserve exact CSS fractional multiplier. ui()/mono() set no line height; PTY is excluded. |
| --line-13 | 1.462 | 1.462 | `.line_height(rpx(LINE_13))` | add | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; src/views/mod.rs | Add LINE_13 = 13.0 * 1.462 in src/views/tokens.rs. Preserve exact CSS fractional multiplier. ui()/mono() set no line height; PTY is excluded. |
| --line-15 | 1.333 | 1.333 | `.line_height(rpx(LINE_15))` | add | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; src/views/mod.rs | Add LINE_15 = 15.0 * 1.333 in src/views/tokens.rs. Preserve exact CSS fractional multiplier. ui()/mono() set no line height; PTY is excluded. |
| --line-18 | 1.278 | 1.278 | `.line_height(rpx(LINE_18))` | add | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; src/views/mod.rs | Add LINE_18 = 18.0 * 1.278 in src/views/tokens.rs. Preserve exact CSS fractional multiplier. ui()/mono() set no line height; PTY is excluded. |
| --line-24 | 1.208 | 1.208 | `.line_height(rpx(LINE_24))` | add | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; src/views/mod.rs | Add LINE_24 = 24.0 * 1.208 in src/views/tokens.rs. Preserve exact CSS fractional multiplier. ui()/mono() set no line height; PTY is excluded. |
| --line-32 | 1.125 | 1.125 | `.line_height(rpx(LINE_32))` | add | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; src/views/mod.rs | Add LINE_32 = 32.0 * 1.125 in src/views/tokens.rs. Preserve exact CSS fractional multiplier. ui()/mono() set no line height; PTY is excluded. |
| --weight-regular | 400 | 400 | `gpui::FontWeight::NORMAL` | verified | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/text_system.rs | Apply with .font_weight(...); ensure the chosen font supplies or synthesizes the weight. |
| --weight-medium | 500 | 500 | `gpui::FontWeight::MEDIUM` | verified | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/text_system.rs | Apply with .font_weight(...); ensure the chosen font supplies or synthesizes the weight. |
| --weight-semibold | 600 | 600 | `gpui::FontWeight::SEMIBOLD` | verified | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/text_system.rs | Apply with .font_weight(...); ensure the chosen font supplies or synthesizes the weight. |
| --weight-bold | 700 | 700 | `gpui::FontWeight::BOLD` | verified | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/text_system.rs | Apply with .font_weight(...); ensure the chosen font supplies or synthesizes the weight. |
| --radius-4 | 4px | 4px | `.rounded(rpx(RADIUS_CONTROL))` | verified | src/views/tokens.rs | Existing radius matches. |
| --radius-8 | 8px | 8px | `.rounded(rpx(RADIUS_8))` | add | src/views/tokens.rs | Add RADIUS_8 = 8.0; RADIUS_GROUP is 6 and cannot substitute. |
| --radius-12 | 12px | 12px | `.rounded(rpx(RADIUS_PANEL))` | verified | src/views/tokens.rs | Existing radius matches. |
| --radius-16 | 16px | 16px | `.rounded(rpx(RADIUS_16))` | add | src/views/tokens.rs | Add RADIUS_16 = 16.0; RADIUS_GROUP is 6 and cannot substitute. |
| --radius-full | 999px | 999px | `.rounded_full()` | verified | src/views/tokens.rs | Existing radius matches. |
| --space-2 | 2px | 2px | `rpx(SPACE_XS)` | verified | src/views/tokens.rs | Existing named spacing matches. |
| --space-4 | 4px | 4px | `rpx(SPACE_SM)` | verified | src/views/tokens.rs | Existing named spacing matches. |
| --space-6 | 6px | 6px | `rpx(SPACE_MD)` | verified | src/views/tokens.rs | Existing named spacing matches. |
| --space-8 | 8px | 8px | `rpx(SPACE_LG)` | verified | src/views/tokens.rs | Existing named spacing matches. |
| --space-10 | 10px | 10px | `rpx(SPACE_XL)` | verified | src/views/tokens.rs | Existing named spacing matches. |
| --space-12 | 12px | 12px | `rpx(SPACE_2XL)` | verified | src/views/tokens.rs | Existing named spacing matches. |
| --space-16 | 16px | 16px | `rpx(SPACE_3XL)` | verified | src/views/tokens.rs | Existing named spacing matches. |
| --space-20 | 20px | 20px | `rpx(SPACE_20)` | add | src/views/tokens.rs | Add SPACE_20 = 20.0; numeric SPACE_* names are not existing constants. |
| --space-24 | 24px | 24px | `rpx(SPACE_24)` | add | src/views/tokens.rs | Add SPACE_24 = 24.0; numeric SPACE_* names are not existing constants. |
| --space-32 | 32px | 32px | `rpx(SPACE_32)` | add | src/views/tokens.rs | Add SPACE_32 = 32.0; numeric SPACE_* names are not existing constants. |
| --space-40 | 40px | 40px | `rpx(SPACE_40)` | add | src/views/tokens.rs | Add SPACE_40 = 40.0; numeric SPACE_* names are not existing constants. |
| --space-48 | 48px | 48px | `rpx(SPACE_48)` | add | src/views/tokens.rs | Add SPACE_48 = 48.0; numeric SPACE_* names are not existing constants. |
| --gap-2 | var(--space-2) | var(--space-2) | `.gap(rpx(SPACE_XS))` | verified | src/views/tokens.rs | Alias to --space-2; no separate GAP_* constant exists. |
| --gap-4 | var(--space-4) | var(--space-4) | `.gap(rpx(SPACE_SM))` | verified | src/views/tokens.rs | Alias to --space-4; no separate GAP_* constant exists. |
| --gap-6 | var(--space-6) | var(--space-6) | `.gap(rpx(SPACE_MD))` | verified | src/views/tokens.rs | Alias to --space-6; no separate GAP_* constant exists. |
| --gap-8 | var(--space-8) | var(--space-8) | `.gap(rpx(SPACE_LG))` | verified | src/views/tokens.rs | Alias to --space-8; no separate GAP_* constant exists. |
| --gap-12 | var(--space-12) | var(--space-12) | `.gap(rpx(SPACE_2XL))` | verified | src/views/tokens.rs | Alias to --space-12; no separate GAP_* constant exists. |
| --gap-16 | var(--space-16) | var(--space-16) | `.gap(rpx(SPACE_3XL))` | verified | src/views/tokens.rs | Alias to --space-16; no separate GAP_* constant exists. |
| --border-thin | 1px | 1px | `.border_1()` | verified | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs | One unscaled logical pixel, not one device pixel. Apply semantic border color separately. |
| --border-medium | 2px | 2px | `.border_2()` | verified | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs | Two unscaled logical pixels; CSS px border stays unscaled by app rem zoom. Apply semantic border color separately. |
| --focus-ring | 2px | 2px | `Add shared focus-ring composition: 1 logical px border + 1 logical px outer ring` | add | src/views/components.rs | Do not substitute a 2px layout border: preserve outside ring and content geometry. Neutral c::SEL_RING also needs migration. |
| --duration-fast | 80ms | 80ms | `std::time::Duration::from_millis(80)` | verified | Rust std::time::Duration; GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/elements/animation.rs | Duration API exists; add shared motion tokens and reduced-motion policy. Duration::ZERO alone does not skip every animation path. |
| --duration-base | 140ms | 140ms | `std::time::Duration::from_millis(140)` | verified | Rust std::time::Duration; GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/elements/animation.rs | Duration API exists; add shared motion tokens and reduced-motion policy. Duration::ZERO alone does not skip every animation path. |
| --duration-slow | 220ms | 220ms | `std::time::Duration::from_millis(220)` | verified | Rust std::time::Duration; GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/elements/animation.rs | Duration API exists; add shared motion tokens and reduced-motion policy. Duration::ZERO alone does not skip every animation path. |
| --ease-standard | cubic-bezier(.2,.8,.2,1) | cubic-bezier(.2,.8,.2,1) | `Add CSS bezier solver for cubic-bezier(.2,.8,.2,1); pass closure to gpui::Animation::with_easing` | add | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/elements/animation.rs | gpui_component::animation::cubic_bezier in vendor/gpui-component/ui/src/animation.rs ignores computed x and returns y(progress). Exact CSS requires solving x(u)=progress before y(u); add an inverse-x solver. |
| --ease-decelerate | cubic-bezier(.16,1,.3,1) | cubic-bezier(.16,1,.3,1) | `Add CSS bezier solver for cubic-bezier(.16,1,.3,1); pass closure to gpui::Animation::with_easing` | add | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/elements/animation.rs | gpui_component::animation::cubic_bezier in vendor/gpui-component/ui/src/animation.rs ignores computed x and returns y(progress). Exact CSS requires solving x(u)=progress before y(u); add an inverse-x solver. |
| --ease-accelerate | cubic-bezier(.4,0,1,1) | cubic-bezier(.4,0,1,1) | `Add CSS bezier solver for cubic-bezier(.4,0,1,1); pass closure to gpui::Animation::with_easing` | add | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/elements/animation.rs | gpui_component::animation::cubic_bezier in vendor/gpui-component/ui/src/animation.rs ignores computed x and returns y(progress). Exact CSS requires solving x(u)=progress before y(u); add an inverse-x solver. |
| --z-base | 0 | 0 | `No numeric z-index equivalent; normal child paint order` | reference | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/elements/deferred.rs; elements/anchored.rs | CSS number is an ordering role, not a GPUI depth or Stack/Overlay API. Verify hit testing and focus separately. |
| --z-raised | 10 | 10 | `No numeric z-index equivalent; later sibling paint order` | reference | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/elements/deferred.rs; elements/anchored.rs | CSS number is an ordering role, not a GPUI depth or Stack/Overlay API. Verify hit testing and focus separately. |
| --z-sticky | 20 | 20 | `No numeric z-index equivalent; paint after scroll content` | reference | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/elements/deferred.rs; elements/anchored.rs | CSS number is an ordering role, not a GPUI depth or Stack/Overlay API. Verify hit testing and focus separately. |
| --z-overlay | 40 | 40 | `No numeric z-index equivalent; deferred anchored layer` | reference | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/elements/deferred.rs; elements/anchored.rs | CSS number is an ordering role, not a GPUI depth or Stack/Overlay API. Verify hit testing and focus separately. |
| --z-modal | 60 | 60 | `No numeric z-index equivalent; deferred blocking layer with focus/input ownership` | reference | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/elements/deferred.rs; elements/anchored.rs | CSS number is an ordering role, not a GPUI depth or Stack/Overlay API. Verify hit testing and focus separately. |
| --z-toast | 80 | 80 | `No numeric z-index equivalent; last nonblocking notification layer` | reference | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/elements/deferred.rs; elements/anchored.rs | CSS number is an ordering role, not a GPUI depth or Stack/Overlay API. Verify hit testing and focus separately. |
| --icon-12 | 12px | 12px | `crate::icons::icon(name, ICON_SM, color)` | verified | src/views/tokens.rs; src/icons.rs | Existing size matches; icon() applies rpx once. Verify sprite path independently. |
| --icon-14 | 14px | 14px | `crate::icons::icon(name, ICON_MD, color)` | verified | src/views/tokens.rs; src/icons.rs | Existing size matches; icon() applies rpx once. Verify sprite path independently. |
| --icon-16 | 16px | 16px | `crate::icons::icon(name, ICON_LG, color)` | verified | src/views/tokens.rs; src/icons.rs | Existing size matches; icon() applies rpx once. Verify sprite path independently. |
| --icon-20 | 20px | 20px | `crate::icons::icon(name, ICON_20, color)` | add | src/views/tokens.rs; src/icons.rs | Add ICON_20 = 20.0; illustration only, not chrome. |
| --icon-24 | 24px | 24px | `crate::icons::icon(name, ICON_24, color)` | add | src/views/tokens.rs; src/icons.rs | Add ICON_24 = 24.0; illustration only, not chrome. |
| --icon-32 | 32px | 32px | `crate::icons::icon(name, ICON_DISPLAY, color)` | verified | src/views/tokens.rs; src/icons.rs | Existing size matches; icon() applies rpx once. Verify sprite path independently. |
| --opacity-disabled | .58 | .58 | `.opacity(.58)` | verified | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; color.rs; src/theme.rs | Element opacity composes the subtree and does not disable input. For one paint use color.opacity(factor); c::alpha replaces alpha. |
| --opacity-muted | .62 | .62 | `.opacity(.62)` | verified | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; color.rs; src/theme.rs | Element opacity composes the subtree and does not disable input. For one paint use color.opacity(factor); c::alpha replaces alpha. |
| --opacity-hover | .08 | .08 | `.opacity(.08)` | verified | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; color.rs; src/theme.rs | Element opacity composes the subtree and does not disable input. For one paint use color.opacity(factor); c::alpha replaces alpha. |
| --opacity-pressed | .14 | .14 | `.opacity(.14)` | verified | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; color.rs; src/theme.rs | Element opacity composes the subtree and does not disable input. For one paint use color.opacity(factor); c::alpha replaces alpha. Quiet veil uses this .14 over scrim .76 dark / .38 light, giving .1064 / .0532 effective alpha. |
| --opacity-scrim | .76 | .76 | `.opacity(.76)` | verified | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/styled.rs; color.rs; src/theme.rs | Element opacity composes the subtree and does not disable input. For one paint use color.opacity(factor); c::alpha replaces alpha. This standalone .76 token is not the quiet veil multiplier: current COMPONENTS/screens use --opacity-pressed (.14) over scrim .76 dark / .38 light, giving .1064 / .0532 effective alpha. |
| --blur-0 | 0 | 0 | `No filter; paint directly` | reference | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/style.rs | Product overlays never blur. No platform compositor helper exists in Grove; do not invent one. |
| --blur-4 | 4px | 4px | `No core GPUI CSS blur equivalent; reference only` | reference | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/style.rs | Product overlays never blur. No platform compositor helper exists in Grove; do not invent one. |
| --blur-8 | 8px | 8px | `No core GPUI CSS blur equivalent; reference only` | reference | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/style.rs | Product overlays never blur. No platform compositor helper exists in Grove; do not invent one. |
| --blur-16 | 16px | 16px | `No core GPUI CSS blur equivalent; reference only` | reference | GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/style.rs | Product overlays never blur. No platform compositor helper exists in Grove; do not invent one. |
| --appbar-h | 40px | 40px | `rpx(APPBAR_H)` | add | src/views/tokens.rs | Add APPBAR_H = 40.0; geometry constant absent. |
| --sidebar-w | 236px | 236px | `rpx(SIDEBAR_W)` | add | src/views/tokens.rs | Add SIDEBAR_W = 236.0; geometry constant absent. |
| --header-h | 36px | 36px | `rpx(HEADER_H)` | add | src/views/tokens.rs | Add HEADER_H = 36.0; geometry constant absent. |
| --status-h | 26px | 26px | `rpx(STATUS_H)` | add | src/views/tokens.rs | Add STATUS_H = 26.0; geometry constant absent. |
| --row-h | 28px | 28px | `rpx(ROW_H)` | add | src/views/tokens.rs | Add ROW_H = 28.0; geometry constant absent. |
| --control-h | 24px | 24px | `rpx(CONTROL_H)` | adapt | src/views/tokens.rs | Current CONTROL_H = 22.0; target 24px. Audit all dependent geometry. |
| --field-h | 60px | 60px | `rpx(FIELD_H)` | add | src/views/tokens.rs | Add FIELD_H = 60.0; geometry constant absent. |
| --form-button-h | 44px | 44px | `rpx(FORM_BUTTON_H)` | add | src/views/tokens.rs | Add FORM_BUTTON_H = 44.0; geometry constant absent. |
| --field-inset-x | 14px | 14px | `rpx(FIELD_INSET_X)` | add | src/views/tokens.rs | Add FIELD_INSET_X = 14.0; geometry constant absent. |
| --field-inset-top | 8px | 8px | `rpx(FIELD_INSET_TOP)` | add | src/views/tokens.rs | Add FIELD_INSET_TOP = 8.0; geometry constant absent. |
| --field-value-top | 27px | 27px | `rpx(FIELD_VALUE_TOP)` | add | src/views/tokens.rs | Add FIELD_VALUE_TOP = 27.0; geometry constant absent. |
| --textarea-h | 130px | 130px | `rpx(TEXTAREA_H)` | add | src/views/tokens.rs | Add TEXTAREA_H = 130.0; geometry constant absent. |
| --switch-w | 48px | 48px | `rpx(SWITCH_W)` | add | src/views/tokens.rs | Add SWITCH_W = 48.0; geometry constant absent. |
| --switch-h | 28px | 28px | `rpx(SWITCH_H)` | add | src/views/tokens.rs | Add SWITCH_H = 28.0; geometry constant absent. |
| --switch-thumb | 22px | 22px | `rpx(SWITCH_THUMB)` | add | src/views/tokens.rs | Add SWITCH_THUMB = 22.0; geometry constant absent. |
| --switch-inset | 3px | 3px | `rpx(SWITCH_INSET)` | add | src/views/tokens.rs | Add SWITCH_INSET = 3.0; geometry constant absent. |
| --tile-min-w | 42px | 42px | `rpx(TILE_MIN_W)` | add | src/views/tokens.rs | Add TILE_MIN_W = 42.0; geometry constant absent. |
| --tile-h | 44px | 44px | `rpx(TILE_H)` | add | src/views/tokens.rs | Add TILE_H = 44.0; geometry constant absent. |
| --category-dot-size | 8px | 8px | `rpx(CATEGORY_DOT_SIZE)` | add | src/views/tokens.rs | Add CATEGORY_DOT_SIZE = 8.0; geometry constant absent. |
| --shadow-none | none | none | `No BoxShadow` | verified | src/views/components.rs; GPUI 1a246efd7e1b83ab568ec5e3e6c1a43a42e1abba crates/gpui/src/style.rs | Flat forms/cards: omit shadow. |
| --color-field-fill | #1b1b1b | #efefef | `c::FIELD_FILL()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| --color-switch-on | #39b76c | #39b76c | `c::SWITCH_ON()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| --color-switch-thumb | #ffffff | #ffffff | `c::SWITCH_THUMB()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| --color-category-dot | #79a9e8 | #79a9e8 | `c::CATEGORY_DOT()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
| --color-error-wash | rgba(239,125,142,.09) | rgba(182,56,76,.09) | `c::ERROR_WASH()` | add | src/theme.rs | Add accessor; absent in current theme. Resolve the exact dark/light values in this row. |
