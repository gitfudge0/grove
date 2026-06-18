# Settings Page — Design

## Goal

Consolidate Grove's scattered appbar preference controls into a single Settings
surface, opened from a cog icon in the top-right of the appbar, and add a new
**Tools** section that shows each supported coding agent (claude, codex,
opencode) with its install status and version, plus selection of the global
default agent used when creating worktrees.

The three existing appbar controls — **app size** (UI zoom), **theme**, and
**terminal backend** (native/tmux) — move into Settings and are removed from the
appbar. The cog becomes the sole entry point.

## Surface

A centered modal — the same pattern as the existing theme picker and scripts
editor. It is a new `Modal::Settings` variant rendered through the existing
one-deep `stack![body, modal_layer()]` overlay (`view.rs`). One modal shows at a
time. Width ~580px, scrollable, built from `modal_panel` and the existing modal
widget helpers (`modal_list_row`, `seg_button`, etc.).

All changes **persist immediately** on interaction (matching how every existing
setting already saves on change). There is no Apply/Cancel footer. The header
`✕` and `Esc` close the modal.

```
┌──────────────── Settings ───────────────[✕]┐
│  APPEARANCE                                  │
│    Theme            Tokyo Night          ›   │
│    App size         [ –    100%    + ]       │
│                                              │
│  TERMINAL                                    │
│    Backend          ( native )  ( tmux )     │
│    New sessions only · tmux: detected        │
│                                              │
│  TOOLS                              ⟳ refresh │
│    ● claude    1.2.3            ( default )   │
│    ● codex     0.4.0            [ set default]│
│    ○ opencode  not installed                 │
│    The default launches for new worktrees.   │
└──────────────────────────────────────────────┘
```

## Appbar change

In `view.rs::appbar` (the `right` row, currently `seg, zoom, icon_btn("contrast",
…)`):

- **Remove** the `native|tmux` segmented backend toggle, the `– NN% +` zoom
  cluster, and the `icon_btn("contrast", Msg::OpenThemePicker)`.
- **Add** `icon_btn("cog", Msg::OpenSettings)`.

The `right` cluster collapses to zen + cog. The `cog` SVG already exists in
`icons.rs`; no new asset. The appbar (and thus the cog) stays hidden in zen mode,
unchanged from today.

## Appearance section

- **Theme** — a `modal_list_row` showing the current theme name
  (`theme::current().name`) with a chevron. Activating it opens the **existing**
  `Modal::ThemePicker`, preserving its live-preview grid and dark/light tabs. To
  return to Settings afterward, `Modal::ThemePicker` gains a
  `return_to_settings: bool` field; `open_theme_picker` is called with it set to
  `true` from Settings. `theme_picker_submit` and `theme_picker_cancel` check the
  flag and, when set, reopen `Modal::Settings` instead of `Modal::None`. All
  existing theme logic is reused — no duplication.
- **App size** — the `– NN% +` stepper moved verbatim from the appbar
  (`control_icon_btn("minus", Msg::ZoomOut, …)`, `control_btn_sized("{NN}%",
  Msg::ZoomReset, …)`, `control_icon_btn("plus", Msg::ZoomIn, …)`). Reuses the
  existing `set_ui_zoom`/`adjust_ui_zoom` clamp (0.6–2.0), 0.1 snapping, and
  immediate persist. Labeled "App size".

## Terminal section

- **Backend** — a `native | tmux` segmented control reusing `seg_button` +
  `Msg::BackendNative` / `Msg::BackendTmux` (which call `app.set_tmux_enabled`).
  When `tmux::available()` is `false`, the `tmux` segment is rendered disabled
  with a "tmux not found" hint and the backend reads `native`.
- Caption clarifies the choice **applies to new sessions only** — existing
  sessions keep their backend, which is the current behavior
  (`set_tmux_enabled` sets "… for new sessions").

## Tools section

Lists `Agent::Claude`, `Agent::Codex`, `Agent::OpenCode` (the `Terminal` agent is
omitted — it is always available and has no version; it remains reachable via the
per-worktree chips). Each row shows:

- a status dot — green when installed, muted when missing;
- the tool's existing icon (`claude` / `codex` / `opencode` sprites) and label;
- its **version** string, or `not installed`, or a spinner while detecting;
- a default selector — a `default` badge on the chosen tool, a `set default`
  action on the others. **Only installed tools** can be set as default.

A `⟳ refresh` affordance in the section header re-runs detection.

### Version detection (new capability)

No version detection exists today (`agent.rs::available()` is only a PATH scan).
This adds:

- `agent.rs`: `fn version(self) -> Option<String>` that runs `<program>
  --version`, captures stdout, and returns the trimmed first non-empty line
  (robust across the three CLIs' differing formats). Returns `None` if the
  command fails or yields nothing; callers fall back to displaying "installed".
- Detection runs **asynchronously**, off the UI thread, so a slow or hung binary
  cannot freeze the app. Opening Settings (and the refresh affordance) dispatch
  an iced `Task` that scans availability and runs `version()` for each installed
  tool, then posts `Msg::ToolVersionsDetected(Vec<(Agent, ToolStatus)>)`.
- Per-tool status is cached on the `Grove` GUI model — a
  `settings_tools: Vec<ToolStatus>` field — following the same "live state parked
  on the model" pattern as `scripts_editor`. `ToolStatus` holds the agent, an
  `installed` flag, `version: Option<String>`, and a `detecting` flag for the
  spinner.

### Default agent

The default selector writes the existing global `Store.default_agent`
(`Option<Agent>`), which `app.rs::launch_or_pick` already consumes when a
worktree is created. Selecting a tool sets it; re-selecting the current default
clears it (mirroring `picker_toggle_default`). This surfaces, in Settings, the
default that is today only editable buried inside the launch-time agent picker —
the picker keeps working unchanged. Scope is **global** (one default for all
projects), not per-project.

## New code surface

All additive; no storage schema migration (`default_agent`, `theme`, `ui_zoom`,
`tmux_enabled` already exist in `Store`).

- `app.rs`: `Modal::Settings` variant; `Modal::ThemePicker` gains
  `return_to_settings: bool`; `open_settings()` helper.
- `view.rs`: `settings_modal()` render fn; a `Modal::Settings` arm in
  `modal_layer()`; the appbar edit (remove three controls, add cog).
- `update.rs`: handlers for the new messages; an `Esc` arm for `Modal::Settings`
  in `handle_modal_key`; the return-to-settings branches in the theme-picker
  submit/cancel handlers.
- `state.rs`: new `Msg` variants `OpenSettings`, `SetDefaultAgent(Agent)`,
  `RefreshTools`, `ToolVersionsDetected(Vec<(Agent, ToolStatus)>)`; the
  `settings_tools` field and `ToolStatus` type. Reuses existing
  `ZoomIn/ZoomOut/ZoomReset`, `BackendNative/BackendTmux`, and `OpenThemePicker`.
- `agent.rs`: `version()` helper.

## Out of scope

- Per-project default agent override (default is global only).
- Per-agent configurable CLI flags / model / custom binary path (agents stay a
  fixed enum with hardcoded launch args).
- Changing the background `claude --model haiku` call that generates
  `.worktreeinclude` — it stays claude-specific by design; the configured default
  governs session launches only.
- Window-geometry persistence (only `ui_zoom` and `sidebar_width` are saved).
- A minimum-version gate or "update available" warnings — versions are display
  only.
- A general top-level page/route system — Settings is a modal, consistent with
  every other config surface.
