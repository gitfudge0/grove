---
name: Grove
description: Terminal worktree launchpad for AI coding agents — a TUI that themes itself out of the user's way.

# TUI semantic roles (11 slots, hue supplied by the active theme)
colors:
  bg: "#1a1b26"
  bg-highlight: "#292e42"
  fg: "#c0caf5"
  fg-dark: "#a9b1d6"
  comment: "#565f89"
  blue: "#7aa2f7"
  cyan: "#7dcfff"
  magenta: "#bb9af7"
  green: "#9ece6a"
  yellow: "#e0af68"
  red: "#f7768e"

# GUI surface tokens — tinted-neutral derivatives of bg (tokyonight values in OKLCH)
gui-colors:
  bg: "oklch(0.20 0.020 270)"          # Workspace canvas
  bg-rail: "oklch(0.17 0.022 270)"     # Sidebar fill
  bg-strip: "oklch(0.155 0.018 270)"   # App bar, status bar, session bar
  bg-hover: "oklch(0.235 0.025 270)"   # Pointer-hover on any clickable row/button/icon
  bg-hl: "oklch(0.275 0.045 270)"      # Selection (≡ TUI bg-highlight)
  border: "oklch(0.28 0.020 270)"      # Hairline borders (1px, always)
  border-soft: "oklch(0.235 0.018 270)" # Internal dividers between sidebar sections
  fg: "{colors.fg}"                    # Primary text
  fg-dim: "{colors.fg-dark}"           # Secondary text
  fg-mute: "{colors.comment}"          # Tertiary text

typography:
  # TUI: one cell, one weight — hierarchy via color
  tui-body:
    fontFamily: "user terminal monospace (no font is bundled)"
    fontSize: "1ch × 1 row (terminal cell)"
    fontWeight: 400
    lineHeight: 1
    letterSpacing: "normal"
  tui-emphasis:
    fontFamily: "user terminal monospace"
    fontWeight: 700
    fontFeature: "BOLD attribute"
  # GUI: two families, six steps, two weights (400 and 600 only)
  gui-ui-sans:
    fontFamily: "Inter, system-ui"
    use: "chrome labels, row labels, status segments, button labels, modal copy"
  gui-mono:
    fontFamily: "ui-monospace, JetBrains Mono"
    use: "paths, branches, project names, agent names, session labels, PTY contents"
  gui-scale:
    caption: { size: 10.5, weight: 400, family: mono, use: "kbd badges, inline counts" }
    micro:   { size: 11,   weight: 400, family: sans, use: "section headers, status segments, action-pill labels" }
    small:   { size: 11.5, weight: 400, family: sans/mono, use: "branch tags, captions, tool-button labels" }
    body:    { size: 12,   weight: 400, family: sans/mono, use: "row labels (sans), identifiers in session bar (mono)" }
    row:     { size: 13,   weight: 400, family: sans/mono, use: "project / worktree primary names, modal titles, text input content" }
    brand:   { size: 14,   weight: 600, family: mono, use: "grove wordmark only" }

rounded:
  none: "0"
  # GUI
  small: "4px"   # action mini, icon buttons, input fields, add-project button, agent menu, modal list container
  modal: "6px"   # modal panels, segmented control container wrapper
  row: "0px"     # list rows are full-width strips — rounding is forbidden

spacing:
  # TUI
  cell: "1ch"
  gutter: "1 row"
  # GUI scale (px) — only these ten values are legal
  gui: [4, 6, 8, 10, 12, 14, 16, 18, 22, 28]
  # GUI fixed heights (px)
  appbar: 44
  status: 26
  session-bar: 36
  list-row: 28
  subtitle-row: 14
  action-btn: 22
  icon-btn: 28
  sidebar: 320

components:
  # TUI components
  pane-active:
    backgroundColor: "{colors.bg}"
    textColor: "{colors.fg}"
    rounded: "{rounded.none}"
  pane-inactive:
    backgroundColor: "{colors.bg}"
    textColor: "{colors.fg-dark}"
  selection-row:
    backgroundColor: "{colors.bg-highlight}"
    textColor: "{colors.fg}"
  status-running:
    textColor: "{colors.green}"
  status-error:
    textColor: "{colors.red}"
  hint-key:
    textColor: "{colors.yellow}"
  hint-label:
    textColor: "{colors.comment}"
  # GUI components
  gui-row-default:
    backgroundColor: "transparent"
    textColor: "{gui-colors.fg-dim}"
  gui-row-hover:
    backgroundColor: "{gui-colors.bg-hover}"
    textColor: "{gui-colors.fg}"
  gui-row-selected:
    backgroundColor: "{gui-colors.bg-hl}"
    textColor: "{gui-colors.fg}"
  gui-icon-btn:
    size: "28×28"
    radius: "{rounded.small}"
    border: "none"
    hoverBackground: "{gui-colors.bg-hover}"
  gui-action-mini:
    size: "22×22"
    radius: "{rounded.small}"
    border: "none"
    hoverBackground: "{gui-colors.bg-hover}"
  gui-action-pill:
    height: 22
    radius: "{rounded.small}"
    border: "1px {gui-colors.border}"
    background: "{gui-colors.bg}"
    hoverBackground: "{gui-colors.bg-hover}"
  gui-tool-btn:
    height: 22
    radius: "{rounded.small}"
    border: "none"
    hoverBackground: "{gui-colors.bg-hover}"
  gui-seg-btn:
    height: 24
    containerRadius: "{rounded.modal}"
    containerBorder: "1px {gui-colors.border}"
    activeBackground: "{gui-colors.bg-hl}"
    hoverBackground: "{gui-colors.bg-hover}"
  gui-modal-panel:
    radius: "{rounded.modal}"
    border: "1px accent-color"
    background: "{gui-colors.bg}"
    padding: "[16, 20]"
---

# Design System: Grove

## 1. Overview

**Creative North Star: "The Borrowed Theme."**

Grove is a TUI. Its visible surface is whatever the user's terminal renders, in whatever theme the user picks from a roster of 36 (22 dark, 14 light). The design system is not a palette — it is a **semantic role map** that any of those 36 palettes can satisfy. The job of the design system is to make sure every screen still reads correctly whether the user is in `tokyonight`, `gruvbox-light`, `matrix`, or `solarized-light`.

This rejects three reflexes outright: the Electron-terminal aesthetic (cards, shadows, gradients in a buffer that has none of those primitives), the AI-tool dashboard reflex (hero metrics, status pills, "thinking…" shimmer), and the over-decorated CLI reflex (ASCII art banners, fancy box-drawing, color used for ornament). Grove's surface is two panes, a status line, and a footer hint. Everything else is content.

Density is high and uniform. The terminal cell is the spacing primitive; padding inside panes is one cell; gutters between sections are one row. There is no concept of "elevation"; depth is signaled by **focus** (which pane is bordered as active), not by shadow or layering.

**Key Characteristics:**
- Eight semantic color slots (`bg`, `bg_highlight`, `fg`, `fg_dark`, `comment`, `blue`, `cyan`, `magenta`, `green`, `yellow`, `red`) mapped to 36 concrete themes.
- Layout is always two-pane horizontal (30/70) with a 3-row banner, 1-row status, 1-row footer.
- No animation. No icons beyond a single Unicode dot `●`. No box-drawing beyond ratatui's default rounded borders.
- Focus is the only state worth painting; everything else stays in the default foreground.

## 2. Colors: The Semantic Eleven

Grove never owns absolute color values. It owns **roles**. Each role has one job; the user's chosen theme supplies the hex.

The example values throughout this document come from `tokyonight` (the default), and the same role lookups produce coherent results across every other theme because each theme is hand-tuned to satisfy the same semantic contract.

### Surface
- **`bg`** (`#1a1b26` in tokyonight): the canvas. Every pane, modal, and gutter sits on this. Never apply text directly to "no background" — the canvas is always painted.
- **`bg_highlight`** (`#292e42`): selection. The current row in any list, the focused tab, the highlighted theme preview. This is the *only* background variation in the system.

### Text
- **`fg`** (`#c0caf5`): primary text. Body content in the focused pane.
- **`fg_dark`** (`#a9b1d6`): secondary text. Body content in unfocused panes; metadata that supports a primary value.
- **`comment`** (`#565f89`): tertiary text. Footer hint labels, placeholder text, "no items" states, separator characters.

### Accent (used sparingly, by role)
- **`blue`** (`#7aa2f7`): identifiers — project names, worktree paths, headings that name a thing.
- **`cyan`** (`#7dcfff`): the active agent / current selection emphasis. Used to draw the eye when blue would compete.
- **`magenta`** (`#bb9af7`): meta-actions — modal titles, pickers, "choose an agent" affordances.
- **`green`** (`#9ece6a`): running. The `●` next to a project or worktree with a live session. Success messages.
- **`yellow`** (`#e0af68`): keybinding letters in footer hints (`q quit`, `?  help`). Always paired with a `comment`-colored label.
- **`red`** (`#f7768e`): errors and destructive prompts only. Never decorative.

### Named Rules

**The Eleven-Slot Rule.** A new piece of UI must map to one of these eleven roles. If a designer feels they need a twelfth color, they are decorating, not informing. Rewrite using the existing eleven.

**The Theme-Agnostic Rule.** No screenshot, mock, or component description in this codebase may reference a hex value. All references are by role name (`fg_dark`, `green`). The theme module is the single binding layer between roles and concrete RGB.

**The Yellow-for-Keys Rule.** Every keybinding letter shown to the user is `yellow`. Every label paired with it is `comment`. If you see a hint rendered without this pairing, it is a bug.

## 3. Typography

**Body Font:** the user's terminal font. Grove ships no fonts and assumes nothing beyond a monospace cell. All sizing is in terminal cells (1ch × 1 row).

**Character:** typographic hierarchy in a TUI is degenerate: there is no scale axis, no leading, no tracking. Hierarchy is carried by **color role and weight**, not size.

### Hierarchy
- **Banner title** (BOLD, `magenta` or `blue` per context): top-row identifier for what the user is looking at. One line. No subtitle.
- **List item — selected** (default weight, `fg` on `bg_highlight`): the cursor row in any pane.
- **List item — default** (default weight, `fg` in focused pane / `fg_dark` in unfocused): rows in projects/worktrees/sessions lists.
- **Metadata** (default weight, `comment`): paths, counts, "(no sessions)" placeholders.
- **Status line** (default weight, mixed roles): one row, left-aligned, semantic colors per segment.
- **Footer hint** (default weight, `yellow` key + `comment` label): single row, space-separated `key label` pairs.

### Named Rules

**The One-Row Rule.** Banner, status, and footer are each exactly the height ratatui's layout assigns them: 3, 1, 1. They do not grow. If a message will not fit, abbreviate; do not wrap.

**The No-Wrap-In-Chrome Rule.** Wrapping is permitted in body content (modal text, help). It is forbidden in the banner, status line, and footer. Those surfaces truncate.

## 4. Elevation

Grove has no elevation. There are no shadows, no gradients, no semi-transparent overlays. The terminal does not render them; pretending it does is a category error.

Depth is signaled by **focus and selection only**:
- The focused pane gets a border of `BorderType::Rounded` rendered in `fg` (or an accent role per context). Unfocused panes get the same border in `comment` or `fg_dark`.
- The selected row in a list gets `bg_highlight` as its background. That is the entire "lifted" vocabulary.

### Named Rules

**The Flat-Forever Rule.** No component in Grove may simulate elevation through Unicode block characters, repeated borders, or chained box-drawing. If a designer wants to imply "this is on top," they use a modal (a fully cleared Rect over the canvas), not a shadow.

## 5. Components

### Panes
- **Shape:** rectangular, `Block::default()` with `Borders::ALL` and `BorderType::Rounded`.
- **Active:** border in `fg` or a context accent (`blue` for the projects pane, `cyan` for sessions, etc.). Title rendered inline in the top border.
- **Inactive:** border in `comment`. Title in `fg_dark`.
- **Internal padding:** one cell. Content starts at `(x+1, y+1)` of the pane rect.

### Lists (projects, worktrees, sessions)
- **Row layout:** `<icon-cell> <primary> <metadata...>`. Primary is `fg`, metadata is `comment` separated by two spaces.
- **Selected row:** background = `bg_highlight`, foreground unchanged.
- **Running indicator:** a green `●` prefix followed by the session count, e.g. `●3`. The dot and count share the `green` role.
- **Empty state:** a single `comment`-colored line: `(no projects)`, `(no worktrees)`, `(no sessions)`. Never a button, never a call to action — the keybinding footer already advertises `a`.

### Modals
- **Trigger:** input prompts (add project, new worktree name), pickers (agent picker, theme picker).
- **Shape:** centered Rect, cleared with `Clear`, then drawn with the same rounded-border pane treatment. Title in `magenta`.
- **Body:** vertical stack, each item one row, selected item painted with `bg_highlight`. For input prompts: a single-line input field with a `cyan` cursor block.
- **Dismissal:** `esc` or `enter`. Never tap-outside; the terminal has no "outside."
- **Hint separator:** one empty row separates the last line of body content from the hint row. This is the only permitted inter-row gap inside a modal; it signals "content above / actions below" without resorting to a box-drawing divider.

### Status line
- **Position:** second-to-last row, height 1.
- **Style:** space-separated segments, each `comment label: fg value` or `role glyph + value` for state segments. Pane focus indicators live here.

### Footer
- **Position:** last row, height 1.
- **Style:** `yellow` key + space + `comment` label, repeated, two-space-separated, e.g. `j move  a add  d delete  r refresh  ? help  q quit`.
- **Rule:** every keystroke that is valid in the current screen must appear in the footer for that screen. If a binding is not in the footer, it does not exist.

### The session count indicator (signature)
The `●N` glyph appearing next to a project name or worktree row is Grove's only purely visual signal. It is `green`, always paired with a digit, never larger than what fits in one cell plus one digit. It collapses the answer to "is anything running here?" into one terminal cell, which is the entire point.

## 6. Do's and Don'ts

### Do:
- **Do** map every new UI surface to one of the eleven semantic color roles before writing any color into the source.
- **Do** signal focus with border color and title weight; signal selection with `bg_highlight`. Nothing else.
- **Do** keep banner / status / footer at their fixed heights (3 / 1 / 1). Truncate content; never grow chrome.
- **Do** pair every keybinding letter with `yellow` and every label with `comment` in footer hints.
- **Do** leave exactly one blank row between modal body content and the modal's hint row.
- **Do** use `●` (U+25CF) and only `●` as a state glyph. Pair it with a numeric count.
- **Do** test every change against `tokyonight`, one warm dark (`gruvbox`), one cool dark (`nord`), one light (`github-light`), and `matrix` — the five themes that break things first.

### Don't:
- **Don't** introduce a twelfth color slot. The eleven exist precisely because eight ANSI colors plus three neutrals is enough; a twelfth means you are decorating.
- **Don't** hardcode hex values anywhere outside `src/theme.rs`. The role lookup is the only contract.
- **Don't** simulate cards, shadows, or elevation with stacked box-drawing characters or repeated borders. The terminal is flat by physics.
- **Don't** add icons beyond `●`. No Nerd Font glyphs, no emoji in menus, no Unicode arrows in chrome.
- **Don't** animate. No spinners, no progress bars, no scrolling banners. Render the new state; let the terminal flip.
- **Don't** wrap text in the banner, status line, or footer. Truncate with no ellipsis if necessary.
- **Don't** prompt-question-prompt-question. Use a single modal with all needed inputs, or two keybindings for two actions.
- **Don't** ship a screen whose keybindings are not all visible in its footer. Hidden bindings are a contract violation.
- **Don't** assume dark mode. Light themes are first-class; 14 of them ship.
- **Don't** use `red` for anything except errors and destructive confirmations. It is the loudest role in every theme by design.

---

# Design System: Grove GUI (iced port)

## 7. Overview

**Creative North Star: "The TUI, with a window around it."**

The GUI is not a redesign. It is the same product — a worktree launchpad with embedded PTY sessions — rendered through `iced` instead of `ratatui`. Everything above (the eleven-slot color contract, the focus-and-selection-only depth model, `●N` as the only state glyph, keyboard primacy) still holds. The GUI adds three things the terminal could not give us: a pixel grid finer than one cell, a proportional UI font, and a mouse cursor. Those affordances are spent narrowly. The rest of the surface stays terminal-native.

The reflexes this rejects are the same three from the TUI, plus one more specific to desktop chrome: **the Electron-app-shell reflex** — title-bar gradients, sidebar avatars, "command palette" search bars that dominate the appbar, settings gear opening a 600px modal of tabs. Grove's GUI is a thin window around the same two panes, the same status row, and a PTY canvas where the body used to be.

The GUI runs only against the `tokyonight` palette today. The theme module remains the single binding layer; when more themes are added to the GUI, they bind to the same eleven semantic roles plus the **seven GUI-only neutral surface tokens** introduced below.

**Key characteristics:**
- Three-row window grid: appbar (44px), main (sidebar 320px + workspace), status (26px). Fixed heights; content adapts.
- Two fonts: a proportional UI font (Inter / system sans) for chrome, the user's monospace for identifiers, paths, and the PTY. No third font.
- Density is high but not terminal-tight. Row height is **28px**, gutters are **8px–16px**, padding inside chrome rows is **12–16px**.
- Borders are 1px hairlines; corners are **4px** (controls) or **6px** (containers and modals). There are no shadows, no gradients, no blurs.
- Focus is signaled by border color shift, selection by `bg_hl`, hover by `bg_hover`. Three states, three roles — that is the entire interaction vocabulary.

## 8. Colors: Eleven Roles + Seven Surface Tokens

The eleven semantic roles from §2 carry over unchanged. They mean exactly the same thing. The GUI cannot use 8-color ANSI fallback, so it adds seven **surface tokens** that the TUI gets for free from terminal cell composition: hover backgrounds, hairline borders, and a darker bg for inset chrome bars.

These tokens are **not** new colors. They are tinted-neutral derivatives of `bg`, all on the same hue axis. Adding a twelfth *hue* is still banned (§2, The Eleven-Slot Rule). Adding lightness steps on the existing neutral axis is what desktop chrome physically requires.

### Surface tokens (GUI-only, tinted-neutral derivatives of `bg`)

| Token | OKLCH (tokyonight) | Role |
|---|---|---|
| `bg` | `oklch(0.20 0.020 270)` | Workspace canvas. Same as TUI `bg`. |
| `bg_rail` | `oklch(0.17 0.022 270)` | Sidebar fill. One step darker than canvas. |
| `bg_strip` | `oklch(0.155 0.018 270)` | App bar, status bar, session bar. Inset chrome. |
| `bg_hover` | `oklch(0.235 0.025 270)` | Pointer-hover on any clickable row, button, or icon. |
| `bg_hl` | `oklch(0.275 0.045 270)` | Selection. Same role as TUI `bg_highlight`. |
| `border` | `oklch(0.28 0.020 270)` | Hairline borders: appbar bottom, button outlines, segmented control container. |
| `border_soft` | `oklch(0.235 0.018 270)` | Internal dividers: sidebar-to-add-project, sessbar bottom. Lower contrast than `border`. |

In practice `bg_hover` is synthesized as `mix(bg, bg_hl, 0.55)` — slightly closer to the selected state than a strict midpoint — making hover feel responsive. All tokens are recomputed from the live palette on every frame; theme swaps take effect without any re-initialization.

Foreground roles split into three steps to mirror the TUI's `fg` / `fg_dark` / `comment`:

| Token | TUI equivalent | Use |
|---|---|---|
| `fg` | `fg` | Primary text in focused surfaces. |
| `fg_dim` | `fg_dark` | Secondary text: unfocused row labels, metadata values, default icon stroke. |
| `fg_mute` | `comment` | Tertiary: placeholder text, captions, separator dots, branch tags, "no session" copy. |

Accent roles (`blue`, `cyan`, `magenta`, `green`, `yellow`, `red`) bind to the same semantics as §2. The Yellow-for-Keys Rule is **dormant** in the GUI: the GUI does not render footer-style key/label pairs because every action has a clickable affordance. If keybinding hints reappear (e.g., a help overlay), the rule reactivates.

### Named rules (GUI-specific)

**The Three-Surface Rule.** Every region of the window has exactly one background: `bg`, `bg_rail`, or `bg_strip`. There is no fourth surface tier. Nested cards, raised panels, and floating toolbars are violations. Modals use `bg` — they are not a fourth tier, they are a floating rect on the same canvas.

**The Tinted-Neutral Rule.** All seven surface tokens share the same hue (270° in tokyonight). When porting to a new theme, the *one* hue axis the theme uses for its neutral ramp is the hue all seven tokens share. Drift between tokens (e.g., a warmer `bg_strip` than `bg`) is a bug.

**The Hairline Rule.** Borders are always 1px. The focus state of an input thickens *color* (`border` → `cyan`), never *width*. 2px borders are reserved for nothing — they don't exist in Grove GUI.

## 9. Typography

The TUI's typographic axis is degenerate (one cell, one weight, color carries hierarchy). The GUI has a real type system, but it is **deliberately small** — six steps, two families, two weights.

### Families
- **UI sans** (`Inter`, system-ui fallback): chrome — appbar text, row labels, status segments, button labels, modal copy.
- **Mono** (user's monospace, `ui-monospace` / `JetBrains Mono` fallback): anything that names a path, branch, project, agent, session label, command, or appears inside the PTY. If it could be a shell token, it is mono.

### Scale (px / weight / role)

| Step | Size | Weight | Family | Use |
|---|---|---|---|---|
| caption | 10.5 | 400 | mono | kbd badges (when shown), inline counts |
| micro | 11 | 400 | sans | section headers (`projects`), status segments, action-pill labels |
| small | 11.5 | 400 | sans/mono | branch tags, captions, tool-button labels |
| body | 12 | 400 | sans/mono | row labels (sans), identifiers in session bar (mono) |
| row | 13 | 400 | sans/mono | project / worktree primary names (sans), modal titles and text input content (mono) |
| brand | 14 | 600 | mono | `grove` wordmark in the appbar; nowhere else |

There is no `h1` / `h2` / `h3`. The window has one brand mark and one row of section headers. Nesting beyond that is content, not chrome — content typography lives inside the PTY and is the user's terminal font, untouched.

### Named rules

**The Mono-for-Identifiers Rule.** Project names, worktree paths, branches, agent names, session labels, and PTY contents are mono. UI labels around them ("projects", "running", "backend") are sans. A project rendered in sans is a bug.

**The Two-Weight Rule.** Only `400` and `600` exist. The brand mark is the only `600` glyph in the chrome. No semibold subheads, no light captions.

**The No-Italics Rule.** Italics are never used. The terminal cannot render them faithfully; the chrome refuses them to stay coherent with the terminal.

## 10. Elevation, Radius, Spacing

### Elevation
There is none. Identical to §4. The GUI's mouse hover does not "lift" anything — it only fills `bg_hover`. Shadows are banned. Blurs are banned. Translucent overlays are banned. A modal, when it exists, is an opaque centered rect with `bg` fill and a 1px accent-colored border. The modal scrim is `rgba(0, 0, 0, 0.16)` — near-transparent, providing just enough darkening to signal "something is on top" without a heavy veil.

### Radius
- **4px**: action mini buttons (22×22), icon buttons (28×28), input fields, add-project button, agent menu panel, modal list containers. The default for any clickable or inset rectangle.
- **6px**: modal panels, segmented control container wrapper. Used where the boundary is a prominent chrome container rather than an individual control. The individual `seg_button` elements inside the container carry no radius of their own.
- **0px**: rows. Project/worktree/session list rows are full-width strips; rounding them looks like a card.
- **No other radius values exist.** The split-start compound button applies left-only or right-only 4px radius to its outer segments — this is still 4px, applied directionally.

### Spacing scale (px)
`4 · 6 · 8 · 10 · 12 · 14 · 16 · 18 · 22 · 28`. These are the only values that may appear in `Padding`, `Space::with_width`, or `spacing()` calls. A `padding: 7` in a PR is a review reject.

Vertical chrome heights are **fixed** and defined as constants in `src/gui/metrics.rs`:

| Constant | Value | Surface |
|---|---|---|
| `APPBAR_H` | 44px | Top chrome bar |
| `STATUS_H` | 26px | Bottom status bar |
| `SESSBAR_H` | 36px | Per-session bar above PTY |
| `ROW_H` | 28px | Every sidebar list row |
| `SUBTITLE_H` | 14px | Session row subtitle line (terminal title) |
| `RAIL_W` | 320px | Sidebar width |

### Named rules

**The Flat-Forever Rule (restated for GUI).** No `box-shadow`. No `drop-shadow`. No multi-layered borders. No "card" component. If a designer reaches for a card to group three items, they group them with a section header and a `border_soft` divider instead.

**The Scale-Eight Rule.** Only the ten spacing values above are legal. A new value being introduced means either an existing value should be reused or the design needs to be reconsidered.

## 11. Components (GUI)

### Window
- Three-row grid: `44px / 1fr / 26px`. Hard heights for top and bottom; main row fills.
- Window background `bg`, with a 1px `border` ring (when the OS draws no chrome of its own).

### App bar
- Background `bg_strip`, bottom border `border` 1px.
- Three columns: brand block (container width = `RAIL_W` = 320px), flexible middle (empty; reserved for future search), right cluster.
- Brand: `grove` (mono / 14 / 600 / `magenta`).
- Right cluster: segmented control container + icon button (`cog`). Spacing 4px. Padding `[0, 16]`.
- Segmented control: the two `seg_button` pills (`native` / `tmux`) are wrapped in a `container` with 1px `border` and **6px radius**. The pills themselves carry no individual border or radius. Active pill fills `bg_hl`; hovered fills `bg_hover`.

### Sidebar (rail)
- Width fixed at **320px**. Background `bg_rail`.
- Scrollable tree area: padding top 8px, bottom 12px.
- Divider `border_soft` 1px separates the scrollable tree from the add-project button.
- Footer: full-width `+ add project` button, 28px tall, **4px radius**, 1px `border`, label 12pt `fg_dim`. Wrapped in a container with `[12, 12]` padding. The only full-width button in the app.

### List row (project / worktree / session)
- Height **28px** (session rows may be **42px** when showing a subtitle — see Session row below). Padding is handled by leading `Space` widths; never by `Padding` on the row itself.
- Hover background: `bg_hover`. Selected background: `bg_hl`. Default: transparent over the rail.

**Project row:** left-pad 12px + chevron (10px, `fg_mute`, in a 14px container) + name (sans / 13 / `fg`, clipped) + 22×22 `+` icon button (12px icon, `fg_mute`, adds worktree on press) + flex spacer + `● N` count (mono / 11 / `green` when N>0, `fg_mute` otherwise). The `+` and count are right-aligned with 8px right-pad. The project row itself is not a single clickable button — the name area and the `+` button are siblings to avoid nested-button event collisions.

**Worktree row:** left-pad 28px + chevron (10px, `fg_mute`, 14px container) + name (sans / 13 / `fg_dim`, clipped) optionally followed by ` · branch` (mono / 11 / `fg_mute`) when branch differs from name + flex + split-start compound button (right-pad 8px). The branch suffix is inline, not a separate fixed-width column. Active worktree row has `bg_hl` painted on both the left button and the outer container. Split into sibling row elements (not nested buttons) to work around an iced 0.13 event propagation limitation.

**Split-start compound button:** a three-segment row that replaces the separate action-pill + action-mini pattern. Segments: `play` icon (9px, `green`) in a 28×22 container (left radius 4px); `term` icon (12px, `fg_mute`) in a 28×22 container (no radius); `more` icon (12px, `fg_mute`) in a 22×22 container (right radius 4px). All three share the same 1px `border` style. On hover: background shifts to `bg_hover`, text/icon to `fg`. Each segment fires a distinct message (`StartSession`, `StartTerminal`, `ToggleAgentMenu`).

**Session row:** left-pad 16px + 28px indent spacer + `dot` (7×7, green=running, `fg_mute`=exited) + meta cluster + `close` action-mini (right-pad 8px). Spacing 8px. The meta cluster is a clipped row: agent label (mono / 12 / `cyan` when active, `fg` when inactive) + `·` separator (11pt, `fg_mute`) + session label (mono / 11 / `fg_mute`). No fixed-width column for agent — it flows inline.

**Session row subtitle:** when the session's current terminal title differs from both the session label and the agent name, a second line is appended below the main row. Height grows from `ROW_H` (28px) to `ROW_H + SUBTITLE_H` (42px). Subtitle: mono / 11 / `fg_mute`, left-pad 48px, clipped, no wrapping. The agent menu position calculator accounts for this variable height.

**Add-worktree row:** appears at the bottom of each expanded project's worktree list. Left-pad 16px + 28px spacer + `+ new worktree` (sans / 12 / `fg_mute`). No border, no radius (explicitly `Radius::from(0.0)`). Fires `AddWorktree` on press.

### Buttons
There are **four** button shapes and no others:

| Shape | Size | Background | Border | Use |
|---|---|---|---|---|
| **icon-btn** | 28×28 | none → `bg_hover` on hover | none (radius 4px, transparent) | Appbar `cog`, project-row `+` |
| **action-mini** | 22×22 | none → `bg_hover` on hover | none (radius 4px, transparent) | Row-level secondary actions (`close`); chevron row `+` |
| **split-start** | compound×22 | `bg` → `bg_hover` on hover | 1px `border`, directional 4px radius | Worktree row actions (`play` / `term` / `more`) |
| **tool-btn** | auto×22 | none → `bg_hover` on hover | none (radius 4px, transparent) | Session-bar actions |
| **seg-btn** | auto×24 | active = `bg_hl`, hover = `bg_hover` | container: 1px `border`, 6px radius | Mode toggle (`native` / `tmux`) |

**Destructive variant.** A `kill` or `trash` tool-btn shifts its label and icon to `red` *only on hover*. At rest it is `fg_dim` like any other tool button. This keeps `red` reserved for confirmed intent, not idle threat.

### Agent menu overlay
- **Trigger:** the `more` segment of a worktree row's split-start button.
- **Shape:** a 120px-wide popover panel positioned via pixel-offset stacking. Background `bg`, 1px `border`, 4px radius, top/bottom padding 3px.
- **Items:** 24px tall, mono / 11 / `fg_dim`. On hover: `bg_hover` background, `fg` text. Available agents: `Codex`, `OpenCode`.
- **Destructive item:** for non-main worktrees only, a `delete` item appears below a `border` divider. Text is `red` at rest (not just on hover) — the only exception to the at-rest `fg_dim` rule. This signals that delete is categorically different from launching a session.
- **Dismissal:** clicking anywhere outside the menu fires `CloseAgentMenu` via an invisible full-size backdrop button stacked beneath the menu.
- **Position:** computed by walking the tree in render order and accumulating row heights (including `SUBTITLE_H` for sessions that have one). Appears flush right inside the sidebar with 8px right margin.

### Session bar
- Height **36px**, background `bg_strip`, bottom border `border_soft` 1px.
- Left cluster (spacing 12px, padding `[0, 16]`): running `●` + state label (`running` green / `exited` `fg_mute`) + `vline` + agent (mono / 12 / `magenta`) + `·` (`fg_mute`) + project (mono / 12 / `blue`) + `/` (`fg_mute`) + session label (mono / 12 / `fg`) + `[branch]` (mono / 12 / `fg_mute`) + flex spacer + cwd path (mono / 12 / `fg_mute`, truncates naturally) + `vline` + `kill` tool-btn.
- **Current tool buttons:** only `kill` (trash icon, danger=true). The `split` and `rename` icons are defined in the sprite but not yet wired in the session bar.
- Spacing between all segments is 12px. The `vline` (1×18px, `border` color) is the visual separator between identity and action clusters.

### PTY canvas
- Background `bg`. Padding **12px top/bottom, 16px left/right** around the scrollable canvas container.
- The PTY is the **only** part of the GUI that owns its own type, color, and grid. Cell metrics: `CELL_W = 7.6px`, `CELL_H = 17.0px`, font `12.5pt mono`. Chrome does not impose styling on PTY contents.
- ANSI indices 0–15 bind back to palette tokens via `ansi_idx()`. Index 0 (ANSI black) maps to `bg_strip` — the darkest surface — so terminal "black" backgrounds blend with the chrome rather than creating a hard-cut rectangle.
- PTY dimensions (`cols`, `rows`) are computed from window size by subtracting fixed chrome heights and the container padding (36px horizontal, 28px vertical) then dividing by cell metrics. Minimum: 10 cols, 4 rows.
- **Selection overlay:** a `rgba(0.40, 0.50, 0.78, 0.35)` blue-lavender rectangle drawn per selected cell range. This color is hardcoded in `pty.rs` and does not respond to theme swaps — a known limitation.
- **Cursor:** full `CELL_W × CELL_H` filled rectangle in `fg`. Blinks at ~500ms on / 500ms off (`blink_tick % 16 < 8` at ~60ms tick interval). Hidden when the terminal sets hide-cursor mode.

### Status bar
- Height **26px**, background `bg_strip`. Padding `[0, 16]`.
- Left cluster (pair-group spacing 16px, within-pair spacing 6px): `●` dot (green if N>0, `fg_mute` otherwise) + `N running` (11pt, `fg_dim`) — `backend` label (`fg_mute`) + value (`fg_dim`) — `theme` label (`fg_mute`) + value (`fg_dim`).
- Center: toast message when present — 11pt `green`. Sits after a fixed 24px spacer from the left cluster, then right-aligned via `Space::Fill`. No background, no border, no timer animation. Appears and disappears in a single frame.
- Right: `v{version}` (11pt, `fg_dim`). Always visible. The only place a version number appears in the chrome.

### Dot glyph (`●`)
- 7×7px circle (radius 3.5px). Always `green` when "running," always `fg_mute` when "exited" or "idle," never any other color.
- This is the *only* shape primitive Grove draws by hand. Every other surface is a rectangle.

### Icons
- All chrome icons are 16×16 viewBox SVGs from the inline sprite in `svg_for()` (`src/gui/icons.rs`). Stroke = `currentColor`, width 1.6px, round caps & joins. Fills are forbidden except where a glyph is intrinsically a fill (the `play` triangle, the dots in `more`).
- Sizes used: **9** (play icon inside split-start left segment), **10** (chevrons in tree), **12** (action-mini, tool-btn, project-row `+`), **15** (icon-btn).
- **Current sprite:** `plus`, `close`, `play`, `chev-down`, `chev-right`, `cog`, `search`, `term`, `more`, `split`, `edit`, `trash`. Of these, `split` and `edit` are defined but not yet wired to any button in the current UI — they are reserved for future session-bar actions.
- The icon set is closed. Adding a new icon means adding a new entry to `svg_for()` and justifying it in review. No external icon library is depended on.

### Modals
- **Panel chrome:** background `bg`, 1px border in accent color, **6px radius**, padding `[16, 20]` (top/bottom 16, left/right 20). Centered in the window via a full-size `container` with `center_x` / `center_y`.
- **Scrim:** `rgba(0, 0, 0, 0.16)` — a near-transparent black overlay on the full window, below the panel. Below the 20% ceiling from §10.
- **Title:** 13pt in the accent color. The accent color varies by modal type (see table below).
- **Body text:** 13pt `fg_dim`, word-wrapped.
- **Action buttons:** `modal_action` shape — 12pt text, `[6, 12]` padding, 4px radius, 1px `border`. Primary button: `bg_hl` at rest → `bg_hover` on hover, `fg` text. Secondary: `bg` at rest → `bg_hover` on hover, `fg_dim` text.

Modal type summary:

| Modal | Width | Height | Accent | Title |
|---|---|---|---|---|
| Input (text only) | 480 | 180 | `magenta` | prompt string |
| Input (path with dir picker) | 640 | 192 + (1–6 matches × 28px) | `magenta` | prompt string |
| Confirm (normal) | 480 | 180 | `magenta` | action name |
| Confirm (destructive) | 480 | 180 | `red` | action name |
| Message / Notice | 480 | 180 | `cyan` | "notice" |
| Tmux choice | 480 | 180 | `cyan` | "session backend" |
| Theme picker | 460 | 140 + (up to 12 themes × 28px) | `magenta` | "theme" |

**Input modal:** text field is 36px tall, `bg_strip` background, 1px `border`, 4px radius, 13pt mono text, 12px horizontal padding. Cursor is a 7×15px `cyan` rectangle — custom-drawn, not iced's `text_input`. The path variant shows a `matches` section below the field with up to 6 directory rows (`modal_dir_row`, 28px, mono 12pt `cyan` at rest / `fg` when active or hovered).

**Theme picker modal:** two tab pills (`Dark` / `Light`) above a scrollable list container (`bg_strip` background, 1px `border`, 4px radius). List rows are 28px, 12pt text, `fg` when selected / `fg_dim` otherwise. Tab pills: 11pt, `bg_hl` when active (`magenta` text) / `bg_hover` when hovered (`fg_mute` text), 3px radius (internal detail; not a new radius in the system — these are non-interactive-looking small pills inside a modal).

## 12. Do's and Don'ts (GUI-specific addenda)

These are *in addition to* §6. The TUI rules still apply to the GUI surface; the items below cover what the TUI rules can't reach.

### Do:
- **Do** keep every interactive rectangle on the radius pair **4px** (controls) or **6px** (container wrappers and modal panels). 4px for anything you click directly; 6px for the housing around a group of controls.
- **Do** use mono for every identifier, even when it sits next to sans labels. The contrast is the hierarchy.
- **Do** render hover as a `bg_hover` fill only. Never as a border thicken, never as a color shift on the text alone, never as a transform.
- **Do** keep the sidebar at exactly 320px. It is not a draggable splitter; resizing it is a future feature, not a today freedom.
- **Do** add new icons by extending `svg_for()` with a 16×16 stroked path that matches the sprite's stroke-width and line caps.
- **Do** account for `SUBTITLE_H` when computing pixel offsets in the tree (e.g., agent menu positioning). Session rows are not always 28px.

### Don't:
- **Don't** introduce a fourth surface (`bg`, `bg_rail`, `bg_strip` are the only three). Floating panels, popovers with a different fill, and "card" backgrounds are all bans.
- **Don't** use a shadow, gradient, blur, or any translucent overlay above 20% opacity. Anywhere. The Flat-Forever Rule is absolute.
- **Don't** add a third font family. Inter and the user's mono are the whole type system. Display fonts, icon fonts, and serif accents are bans.
- **Don't** add a font size outside the six-step scale. If a designer needs a 12.75px label, they are smoothing over a layout problem with type — fix the layout.
- **Don't** use color on hover to indicate intent (e.g., turning a save icon green on hover). Hover is `bg_hover` plus a foreground promotion from `fg_dim` to `fg`. The one exception is the destructive tool-btn (`kill`), which shifts to `red` on hover — this is intentional and bounded to danger actions only.
- **Don't** ship a clickable element without a hover state and a discernible default-vs-hovered contrast. Every interactive rectangle must answer "can I click this?" within 100ms of cursor entry.
- **Don't** animate the appearance/disappearance of UI elements. Tree expansion is an instant re-layout. Toasts appear and disappear in a single frame. The agent menu opens and closes with no transition.
- **Don't** add a scrollbar style. The host platform's native scrollbar is the right answer; restyling it is decoration.
- **Don't** introduce a window-level title bar, traffic-light glyphs, or a custom close button. The OS owns window chrome; Grove owns the appbar inside it.
- **Don't** add a settings modal. Configuration lives in keystrokes (`native`/`tmux` toggle is already in the appbar) and, when truly needed, in the user's config file — not in a tabbed dialog.
- **Don't** add a radius value that isn't 0, 4, or 6px. The three-value radius vocabulary is intentional. Directional radii (left-only, right-only) still use 4px on the active corners.
