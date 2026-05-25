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
    row:     { size: 13,   weight: 400, family: sans, use: "project / worktree primary names" }
    brand:   { size: 14,   weight: 600, family: mono, use: "grove wordmark only" }

rounded:
  none: "0"
  # GUI
  small: "4px"   # action pills, action-mini, tool buttons, badges
  control: "5px" # segmented controls, inputs, icon buttons, add-project button
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
    radius: "{rounded.control}"
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
    radius: "{rounded.control}"
    border: "1px {gui-colors.border} (shared)"
    activeBackground: "{gui-colors.bg-hl}"
    hoverBackground: "{gui-colors.bg-hover}"
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

The GUI runs only against the `tokyonight` palette today. The theme module remains the single binding layer; when more themes are added to the GUI, they bind to the same eleven semantic roles plus the **six GUI-only neutral surface tokens** introduced below.

**Key characteristics:**
- Three-row window grid: appbar (44px), main (sidebar 320px + workspace), status (26px). Fixed heights; content adapts.
- Two fonts: a proportional UI font (Inter / system sans) for chrome, the user's monospace for identifiers, paths, and the PTY. No third font.
- Density is high but not terminal-tight. Row height is **28px**, gutters are **8px–14px**, padding inside chrome rows is **12–18px**.
- Borders are 1px hairlines; corners are 4–5px. There are no shadows, no gradients, no blurs.
- Focus is signaled by border color shift, selection by `bg_hl`, hover by `bg_hover`. Three states, three roles — that is the entire interaction vocabulary.

## 8. Colors: Eleven Roles + Six Surface Tokens

The eleven semantic roles from §2 carry over unchanged. They mean exactly the same thing. The GUI cannot use 8-color ANSI fallback, so it adds six **surface tokens** that the TUI gets for free from terminal cell composition: hover backgrounds, hairline borders, and a darker bg for inset chrome bars.

These tokens are **not** new colors. They are tinted-neutral derivatives of `bg`, all on the same hue axis. Adding a twelfth *hue* is still banned (§2, The Eleven-Slot Rule). Adding lightness steps on the existing neutral axis is what desktop chrome physically requires.

### Surface tokens (GUI-only, tinted-neutral derivatives of `bg`)

| Token | OKLCH (tokyonight) | Role |
|---|---|---|
| `bg` | `oklch(0.20 0.020 270)` | Workspace canvas. Same as TUI `bg`. |
| `bg_rail` | `oklch(0.17 0.022 270)` | Sidebar fill. One step darker than canvas. |
| `bg_strip` | `oklch(0.155 0.018 270)` | App bar, status bar, session bar. Inset chrome. |
| `bg_hover` | `oklch(0.235 0.025 270)` | Pointer-hover on any clickable row, button, or icon. |
| `bg_hl` | `oklch(0.275 0.045 270)` | Selection. Same role as TUI `bg_highlight`. |
| `border` | `oklch(0.28 0.020 270)` | Hairline borders: appbar bottom, button outlines, segmented control. |
| `border_soft` | `oklch(0.235 0.018 270)` | Internal dividers between sidebar sections. Lower contrast than `border`. |

Foreground roles split into three steps to mirror the TUI's `fg` / `fg_dark` / `comment`:

| Token | TUI equivalent | Use |
|---|---|---|
| `fg` | `fg` | Primary text in focused surfaces. |
| `fg_dim` | `fg_dark` | Secondary text: unfocused row labels, metadata values, default icon stroke. |
| `fg_mute` | `comment` | Tertiary: placeholder text, captions, separator dots, branch tags, "no session" copy. |

Accent roles (`blue`, `cyan`, `magenta`, `green`, `yellow`, `red`) bind to the same semantics as §2. The Yellow-for-Keys Rule is **dormant** in the GUI: the GUI does not render footer-style key/label pairs because every action has a clickable affordance. If keybinding hints reappear (e.g., a help overlay), the rule reactivates.

### Named rules (GUI-specific)

**The Three-Surface Rule.** Every region of the window has exactly one background: `bg`, `bg_rail`, or `bg_strip`. There is no fourth surface tier. Nested cards, raised panels, and floating toolbars are violations.

**The Tinted-Neutral Rule.** All six surface tokens share the same hue (270° in tokyonight). When porting to a new theme, the *one* hue axis the theme uses for its neutral ramp is the hue all six tokens share. Drift between tokens (e.g., a warmer `bg_strip` than `bg`) is a bug.

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
| body | 12 / 12.5 | 400 | sans/mono | row labels (sans), identifiers in session bar (mono) |
| row | 13 | 400 | sans | project / worktree primary names |
| brand | 14 | 600 | mono | `grove` wordmark in the appbar; nowhere else |

There is no `h1` / `h2` / `h3`. The window has one brand mark and one row of section headers. Nesting beyond that is content, not chrome — content typography lives inside the PTY and is the user's terminal font, untouched.

### Named rules

**The Mono-for-Identifiers Rule.** Project names, worktree paths, branches, agent names, session labels, and PTY contents are mono. UI labels around them ("projects", "running", "backend") are sans. A project rendered in sans is a bug.

**The Two-Weight Rule.** Only `400` and `600` exist. The brand mark is the only `600` glyph in the chrome. No semibold subheads, no light captions.

**The No-Italics Rule.** Italics are never used. The terminal cannot render them faithfully; the chrome refuses them to stay coherent with the terminal.

## 10. Elevation, Radius, Spacing

### Elevation
There is none. Identical to §4. The GUI's mouse hover does not "lift" anything — it only fills `bg_hover`. Shadows are banned. Blurs are banned. Translucent overlays are banned. A modal, when it exists, is an opaque centered rect with the same `bg` and a 1px `border` ring. No backdrop scrim above 20% opacity; preferably no scrim at all.

### Radius
- **4px**: action pills, action-mini buttons, tool buttons, badges. The default for any clickable rectangle smaller than a row.
- **5px**: segmented controls, input fields, icon buttons (28×28), the "add project" full-width button. The default for control-sized rectangles.
- **0px**: rows (project/worktree/session list rows are full-width strips; rounding them looks like a card).
- **No other radius values exist.**

### Spacing scale (px)
`4 · 6 · 8 · 10 · 12 · 14 · 16 · 18 · 22 · 28`. These are the only values that may appear in `Padding`, `Space::with_width`, or `spacing()` calls. A `padding: 7` in a PR is a review reject.

Vertical chrome heights are **fixed**: appbar 44, status 26, session bar 36, list row 28, action buttons 22, icon buttons 28. These are constants in `src/gui.rs` — never inline.

### Named rules

**The Flat-Forever Rule (restated for GUI).** No `box-shadow`. No `drop-shadow`. No multi-layered borders. No "card" component. If a designer reaches for a card to group three items, they group them with a section header and a `border_soft` divider instead.

**The Scale-Eight Rule.** Only the ten spacing values above are legal. A new value being introduced means either an existing value should be reused or the design needs to be reconsidered.

## 11. Components (GUI)

### Window
- Three-row grid: `44px / 1fr / 26px`. Hard heights for top and bottom; main row fills.
- Window background `bg`, with a 1px `border` ring (when the OS draws no chrome of its own).

### App bar
- Background `bg_strip`, bottom border `border` 1px.
- Three columns: brand block (width = sidebar width = 320px), flexible middle (currently empty; reserved for future search), right cluster.
- Brand: `grove` (mono / 14 / 600 / `magenta`) + tagline `worktree launchpad for ai agents` (sans / 11.5 / `fg_mute`). The tagline truncates; it never wraps.
- Right cluster: segmented control (`native` / `tmux`) + icon buttons (`cog`, `help`). Gap 4px. Padding 12px.

### Sidebar (rail)
- Width fixed at **320px**. Background `bg_rail`.
- Header strip (36px) with section label `projects` (sans / 11 / `fg_mute`) and a `+` icon button.
- Divider `border_soft` 1px.
- Scrollable tree: projects → worktrees → sessions. Indentation is via leading `Space` widths (12 / 28 / 16), not via nested containers.
- Footer: full-width `+ add project` button, 28px tall, 5px radius, 1px `border`, label `fg_dim`. The only full-width button in the app.

### List row (project / worktree / session)
- Height **28px**. Padding handled by leading spaces; never by `Padding` on the row itself.
- Hover background: `bg_hover`. Selected background: `bg_hl`. Default: transparent over the rail.
- **Project row:** chevron (10px, `fg_mute`) + name (sans / 13 / `fg`) + flex spacer + `●N` count chip (`green` when N>0, `fg_mute` otherwise).
- **Worktree row:** 28px indent + chevron + name (sans / 13 / `fg_dim`, 112px clipped) + branch tag (mono / 11 / `fg_mute`, 118px fixed, clipped — the "right-edge alignment rule") + flex + `start` action pill + `term` action-mini + `more` action-mini.
- **Session row:** 28px indent + colored `●` (green=running, `fg_mute`=exited) + agent label (mono / 12 / `cyan` when active, `fg` when inactive, 64px column) + `·` separator + session label (mono / 11 / `fg_mute`, clipped) + flex + `close` action-mini.

### Buttons
There are **four** button shapes and no others:

| Shape | Size | Background | Border | Use |
|---|---|---|---|---|
| **icon-btn** | 28×28 | none → `bg_hover` on hover | none | Appbar gear/help, sidebar `+` |
| **action-mini** | 22×22 | none → `bg_hover` on hover | none | Row-level secondary actions (`term`, `more`, `close`) |
| **action-pill** | auto×22 | `bg` → `bg_hover` on hover | 1px `border`, 4px radius | Row-level primary action (`start`) |
| **tool-btn** | auto×22 | none → `bg_hover` on hover | none | Session-bar actions (`split`, `rename`, `kill`) |
| **seg-btn** | auto×24 | active = `bg_hl`, hover = `bg_hover` | shared `border` 1px, 5px radius | Mode toggle (`native` / `tmux`) |

**Destructive variant.** A `kill` or `trash` tool-btn shifts its label and icon to `red` *only on hover*. At rest it is `fg_dim` like any other tool button. This keeps `red` reserved for confirmed intent, not idle threat.

### Session bar
- Height **36px**, background `bg_strip`, bottom border `border_soft` 1px.
- Left cluster: running `●` + state label (`running` green / `exited` `fg_mute`) + `|` divider + agent (mono / 12 / `magenta`) + `·` + project (mono / 12 / `blue`) + `/` + label (mono / 12 / `fg`) + `[branch]` (mono / 12 / `fg_mute`).
- Right cluster: cwd path (mono / 12 / `fg_mute`, right-aligned, truncates left) + `|` + tool-btns `split`, `rename`, `kill`.
- The 10px spacing between left-cluster segments is the only place gaps reach 10px — the rhythm signals "structured identifier" the way breadcrumbs do.

### PTY canvas
- Background `bg`. Padding 14px top/bottom, 18px left/right.
- The PTY is the **only** part of the GUI that owns its own type, color, and grid (cell metrics 7.6 × 17px, font 12.5pt mono). Chrome does not impose styling on PTY contents.
- ANSI indices 0–15 bind back to the eleven semantic roles via `ansi_idx()` — the ANSI 8-color palette inside the PTY is a *projection* of the same role contract, not a separate system.

### Status bar
- Height **26px**, background `bg_strip`. Padding 14px horizontal.
- Left: `● {n} running` (green dot + `fg_dim` count) · `backend {value}` · `theme {name}`. Pairs are `fg_mute` label + `fg_dim` value, separated by 6px; pair-groups are separated by 14px.
- Right: `v{version}` in `fg_dim`. Always visible. The only place a version number appears in the chrome.
- Toast: when present, sits in the middle, `green` text, 11px. It does not have a background, a border, a dismiss button, or a timer animation. It replaces itself or disappears; that is all.

### Dot glyph (`●`)
- 7×7px circle (radius 3.5px). Always green when "running," always `fg_mute` when "exited" or "idle," never any other color.
- This is the *only* shape primitive Grove draws by hand. Every other surface is a rectangle.

### Icons
- All chrome icons are 16×16 viewBox SVGs from the inline sprite in `svg_for()`. Stroke = `currentColor`, width 1.6px, round caps & joins. Fills are forbidden except where a glyph is intrinsically a fill (the `play` triangle, the dots in `more`).
- Sizes used: **9** (inside action-pill before a label), **10** (chevrons in tree), **12** (action-mini, tool-btn), **15** (icon-btn).
- The icon set is closed. Adding a new icon means adding a new entry to `svg_for()` and justifying it in review. No external icon library is depended on.

## 12. Do's and Don'ts (GUI-specific addenda)

These are *in addition to* §6. The TUI rules still apply to the GUI surface; the items below cover what the TUI rules can't reach.

### Do:
- **Do** keep every clickable rectangle on the radius pair `4 / 5px`. Pills/buttons at 4, controls/inputs/icon-btns at 5.
- **Do** use mono for every identifier, even when it sits next to sans labels. The contrast is the hierarchy.
- **Do** render hover as a `bg_hover` fill only. Never as a border thicken, never as a color shift on the text alone, never as a transform.
- **Do** keep the sidebar at exactly 320px. It is not a draggable splitter; resizing it is a future feature, not a today freedom.
- **Do** add new icons by extending `svg_for()` with a 16×16 stroked path that matches the sprite's stroke-width and line caps.

### Don't:
- **Don't** introduce a fourth surface (`bg`, `bg_rail`, `bg_strip` are the only three). Floating panels, popovers with a different fill, and "card" backgrounds are all bans.
- **Don't** use a shadow, gradient, blur, or any translucent overlay. Anywhere. The Flat-Forever Rule is absolute.
- **Don't** add a third font family. Inter and the user's mono are the whole type system. Display fonts, icon fonts, and serif accents are bans.
- **Don't** add a font size outside the six-step scale. If a designer needs a 12.75px label, they are smoothing over a layout problem with type — fix the layout.
- **Don't** use color on hover to indicate intent (e.g., turning a save icon green on hover). Hover is `bg_hover` plus a foreground promotion from `fg_dim` to `fg`. That is the entire hover vocabulary.
- **Don't** ship a clickable element without a hover state and a discernible default-vs-hovered contrast. Every interactive rectangle must answer "can I click this?" within 100ms of cursor entry.
- **Don't** animate the appearance/disappearance of UI elements. Tree expansion is an instant re-layout. Toasts appear and disappear in a single frame.
- **Don't** add a scrollbar style. The host platform's native scrollbar is the right answer; restyling it is decoration.
- **Don't** introduce a window-level title bar, traffic-light glyphs, or a custom close button. The OS owns window chrome; Grove owns the appbar inside it.
- **Don't** add a settings modal. Configuration lives in keystrokes (`native`/`tmux` toggle is already in the appbar) and, when truly needed, in the user's config file — not in a tabbed dialog.

