# Grove design system contract

This is the normative UI contract for Grove’s native Rust/GPUI app. It records approved hierarchy, component roles, interaction states, and exceptions. The executable source owns exact values: [tokens](src/views/tokens.rs), [semantic colors](src/theme.rs), [shared controls](src/views/components.rs), [icons](src/icons.rs), and [motion](src/views/motion.rs). These are requirements, not claims that every runtime state has been verified. Use source inspection and native checks to assess implementation; report any mismatch. Latest explicit user direction takes precedence; reconcile the contract in the same change.

Use the [project UI skill](.agents/skills/grove-ui/SKILL.md) for implementation and verification. Codex and Claude share that procedure. Historical CSS migration tables and proposed APIs are [archived](docs/design/2026-10-22-migration-catalog.md); neither that archive nor `DESIGN.html` defines current values or implies unfinished migration work.

## Foundations

- One app-level System/Dark/Light appearance setting resolves fixed palettes. Workspace and project views never override appearance. Main canvas and appbar/statusbar strips are opaque; sidebar material is the explicit exception below. Native editors, Markdown, selection, and integrated HTML reports follow the active appearance. Ordinary authored HTML retains its own styling.

- Use semantic `c::*()` colors, not copied hex values or a PTY palette for app chrome. Neutral fills express hover, selection, focus, and open state. Violet is reserved for agent identity, connection, and active-port roles; green means success, amber needs-you/warning, red error/destruction/interruption, and blue information or the explicit activity/effort roles below. Pair status colors with meaningful glyphs or text. On light surfaces, supporting status indicators accompany legible foreground copy; destructive copy uses the semantic error role.

- Geist (`UI_FAMILY`) is the application UI family for labels, forms, paths, branches, metadata, keycaps, native diffs, and script editors. Terminals and Markdown code blocks use their configured code font; preserve terminal cell metrics. Ordinary authored HTML owns its fonts. Use semantic type roles from tokens, explicit line height where needed, and normal tracking. Display type belongs to sparse empty states/onboarding, never chrome.

- Numeric geometry uses existing tokens via `rpx()` and scales once with app rem zoom. Hairlines remain unscaled logical pixels. `icon()` applies scaling itself; never scale its size twice. Shadows needing pixels convert rem-scaled geometry with the current rem size. A one-consumer dimension is a named module constant, not a new global scale tier. First fit the existing component recipe; a new shared token requires a repeated semantic need and approved direction.

- Fields/cards are flat, without bevels or inset shadows. Surface steps and neutral seams establish hierarchy; floating menus, tooltips, and panels use their shared recipes. Grid tiles meet flush with subtle seams, including the top edge, without item gaps or heavy tile borders.

## Controls, forms, and states

| Need | Shared recipe | Contract |
|---|---|---|
| Header/icon action | `header_control`, `header_control_with_tooltip` | Stable icon slot, accessible name, shared tooltip and geometry; owner supplies missing focus/activation behavior. |
| Full form field | `form_field` | Embedded supporting label above a Geist value in a filled well; semantic error attached below. |
| Project flow field | `project_form_field`, `project_field_well` | Match the existing project canvas recipe; editable and readonly values share the well. |
| Action button | `form_action`, `transcript_action`, `dense_action_with_colors` | One size: the complete smaller dense recipe, including label, glyph, gap, padding, radius, and vendor inner size tier. Semantic colors may differ; form actions do not grow. |
| Questions/settings | `question_choice`, `question_disclosure`, `settings_control` | Neutral selection/open treatment with component-owned keyboard focus; owner supplies interaction semantics. |
| Preferences/readonly/errors | `switch_track`, `readonly_value`, `attached_validation` | Owner supplies accessible role/input; readonly content remains literal and selectable; validation joins its field in normal flow. |
| Task/status content | `task_mark`, `task_meter`, shared status primitives | Reuse the same semantic status and progress representation across owning views. |

A shared visual helper does not necessarily supply a complete interactive control. Where the helper does not provide them, the owner supplies a stable ID, focus/tab behavior, keyboard activation, action handling, and enabled/disabled semantics.

Full form geometry uses `FIELD_H`, `FIELD_INSET_X`, `FORM_VALUE_SIZE`, `FORM_VALUE_LINE_H`, and `TEXTAREA_H`. Form fields use `RADIUS_PANEL`; grouped preferences use `RADIUS_PREFERENCE`; all action buttons use `CONTROL_H` and `RADIUS_CONTROL`; compact menus use `RADIUS_CHROME`. Labels use supporting text contrast, not quiet metadata contrast. Split fields represent semantic pairs. Textareas grow vertically. Disabled opacity does not itself disable input: disabled controls must reject interaction while enabled and readonly labels retain normal contrast.

Focus and open states on inputs and dropdowns use neutral fill changes and stable borders. Never add white or foreground-colored borders/outlines to these controls. Preserve visible carets, labels, validation error borders, keyboard navigation, and accessible names. Keep hover, keyboard `focus_visible`, selected rows, and intentionally open state distinct. Action buttons return to rest after release; clicking must not leave a persistent highlight. Other controls retain appropriate keyboard focus indication without shifting geometry.

Action buttons carry catalog icons: leading by default, directional icons trailing only for disclosure/forward navigation. Loading replaces/animates the leading icon without changing its slot; disabled actions keep it. Icon-only actions have an accessible name and tooltip; visible text supplies the name when present and decorative glyphs stay hidden from accessibility. Switches/selection controls express state directly and need no decorative action icon. Use the catalog meaning (plus add, check confirm/save, close cancel, trash delete, refresh retry, folder/list/grid destinations, more manage, chevrons disclosure), never text characters as substitute icons, decorative wrapper boxes, violet backplates, or stretched glyphs. Clickable chrome glyphs do not exceed `ICON_LG`; `ICON_20` is the decorative command-palette exception and display glyphs remain decorative.

Use the shared inverse tooltip surface in both appearances. It wraps long names, scales with rem zoom, and preserves component-owned timing/placement. A tooltip does not replace the trigger’s accessible name or full identity.

## Alignment and responsive layout

Reserve leading icon, label, metadata, and trailing action/count space appropriate to the row. Truncating text yields before fixed controls. Revealing close, Copy, or task actions preserves the row’s geometry; loading feedback retains its icon slot. Worktree launchers intentionally replace Git counts on hover/focus and may occupy a different trailing width. Do not let a flex label consume the space required by a trailing control. Header height follows its remaining content when controls move elsewhere. Test long labels and narrow widths; avoid clipping and overlap rather than hiding meaningful actions.

## Overlays, keyboard, and motion

Blocking confirmation stays attached to the row, tile, switcher, or manager action that opened it. A low-opacity neutral veil makes the app inert while preserving a crisp source aperture. The tray shares the source edge, uses one neutral seam, and presents consequences inline. It is never a centered modal, blurred/blackout overlay, colored edge, or separated header/footer bands. Workspace creation morphs in an anchored switcher popover without moving; deletion expands in place. Escape/Cancel preserves state and returns focus to the source. Deferred/anchored paint order is not a CSS z-index API: verify hit testing, input ownership, outside dismissal, and focus separately. Sidebar platform material does not authorize blur on product overlays.

Use shared fast/base/slow motion and stable keys tied to meaningful state changes, not every render. With Reduce motion enabled, native chrome renders immediate endpoints/static activity without creating animation state or scheduling animation frames. Embedded HTML receives the preference: suppress CSS animations/transitions, cancel active Web Animations, and notify scripts through the preference event and `matchMedia`. Authored scripts must honor the preference; this integration does not stop arbitrary JavaScript timers or animation-frame callbacks. Keep focus, keyboard actions, PTY output, layout, and resizing immediate. The effort slider below is the explicit pointer-motion exception and still honors reduced motion.

## Navigation and sidebar

Project and List share one borderless destination toggle: Project offers the List icon and “Switch to list view”; List offers the folder icon and “Switch to project view.” Grid has a separate borderless “Open grid view” button; in Grid the toggle offers the remembered non-grid destination. Use the shared dense control/glyph/gap recipe, transparent rest, neutral hover/selection, and keyboard focus. No segmented outline or visible mode text.

The saved Sidebar appearance setting offers Frosted and Solid. Frosted is the default: the sidebar rail and its appbar segment use a translucent theme tint over the window's blurred background. Solid uses the opaque rail color. The main canvas and the rest of the appbar remain opaque. Platform compositor support determines whether Frosted is blurred or simply translucent.

The sidebar uses the rem-scaled `SIDEBAR_COLLAPSED_W` icon rail. Dimensions in the following navigation and workspace recipes are design pixels, scaled through `rpx()`. Expanded, its workspace header places Collapse directly after Settings; collapsed, that header contains only Expand. Workspace switching and current-view controls lead the compact scroll region, followed by project/session navigation and a separate standalone-terminal group.

Settings, Archive, and Add project form the bottom utility group; Add terminal stays with standalone terminals. These groups share one scroll region under the fixed header. Compact tiles use contextual hover tooltips, accessible names, and keyboard focus; keyboard access must not depend on a tooltip appearing. Collapse persists independently of the remembered expanded width. Custom macOS window buttons hide only while the compact navigation rail is visible, and Grid and Zen retain their existing layouts.

Compact session and standalone-terminal tiles center the 16px identity glyph on the rail. Status overlays its lower-right corner: a 10px glyph within 12px circular clearance, positioned at the tile center (18px from the left, 16px from the top of the 36 × 32px tile). Clearance reveals the actual rail or interaction fill, including Frosted appearance. Status remains decorative within the session target; Close retains its upper-right slot.

### Compact project list

- Collapsed project anchors use short, collision-safe text identifiers and open-session counts. Hovering anywhere in a project section, including its session tiles, opens the named worktree/session list without changing selection or keyboard focus. The hover list stays open while crossing into or interacting with it, and closes shortly after leaving the project section and list. Clicking an anchor or opening by keyboard makes the list persistent until dismissal or selection; selecting a direct rail session suppresses reopening until the pointer leaves that project section. Menus and overlays block automatic hover lists.

- The list groups sessions by worktree, names the main worktree “Main checkout”, shows branch names, session titles, agent/state labels, and empty worktrees. All sessions remain directly accessible in snapshot order regardless of expanded-tree folds. Neither the compact rail nor this list uses vertical connector guides.

- Escape restores anchor focus; selecting a session dismisses the list and focuses its terminal. Project actions and the expanded project overview remain reachable from the list. Workspace changes, project removal, expanding the sidebar, and view changes dismiss the list and cancel pending hover dismissal.

- Lists reset inherited typography: normal UI body text, a 14px semibold project header without open counts, 12px medium worktree and session titles, and 11px normal branch/agent/state metadata. Session and footer icons share a stable icon slot; compact footer actions follow a quiet horizontal separator.

- Each worktree title row has a 24px plus control that replaces itself with Terminal, Claude Code, Codex, and OpenCode icon controls on the same row. Worktree names truncate while the branch stays below; no open counts appear in the list. Activating the plus pins the popup and focuses Terminal. Only one checkout picker is open at a time. Escape collapses it and returns focus to its plus before another Escape closes the list and restores anchor focus. Successful launch closes the popup and focuses the session; errors preserve the chosen checkout for retry.

### Expanded project tree

- Expanded project navigation follows the approved focused hierarchy: project names use 14px semibold type; worktrees inset 8px use 12px medium names in single-line rows, with branch names available in hover tooltips and accessible labels. Nesting and typography distinguish row types without explicit Project or Worktree labels. Worktrees and their sessions share one nested row inset beneath the project.

- Sessions use 32px single-line rows: activity glyph, blue launcher icon, and 12px normal session title. Session items contain no Git diff metadata. Worktree Git counts (+added green and −removed red) appear only when changes exist; clean and unavailable states have no visible placeholder. Worktree launcher options replace those counts on hover or keyboard focus; the Git changes action remains keyboard reachable and reveals itself when focused.

- Working and Starting use a rotating blue open ring (static with reduced motion), Needs you an amber dot, Done a green check, Idle and Exited a muted hollow hexagon, and Failed a red cross. Launcher and status text do not appear in the row; full session titles, launcher names, status, and multi-project root identities remain in accessible labels and hover tooltips.

- Titles truncate before the fixed icon and trailing close columns. Selection uses neutral fill; the close control appears on hover or keyboard focus in its own slot and never covers counts. Git changes remain clickable and keyboard accessible. Launcher controls appear on worktree hover and keyboard focus. The compact rail, project flyout, and list/grid layouts retain their separate approved treatments.

## Workspace and checkout flows

### Workspace controls and input copy

Dropdown triggers and menu items use neutral hover/selected fills for clicked, open, and focused states, with stable borders and no white or foreground-colored outline. Preserve keyboard navigation and accessible names. Other controls retain their keyboard focus rings.

Workspace menu rows are 28px high, with 8px horizontal padding and icon-to-label gap, 6px menu padding, zero adjacent item gap, and 4px vertical separator margins. Create workspace uses the plus icon; Manage workspaces uses the more icon. Pinned actions use the same row spacing. Workspace management offers rename and deletion, without workspace ordering controls. Moving projects between workspaces is a separate operation.

Project overflow menus offer “Move to workspace…” beside Edit project. The project panel shows the current workspace and existing destination workspaces with a clear selected state, then an explicit “Move project” action. The folder and worktrees stay in place and open sessions keep running. Cancel or Escape preserves membership and restores source focus. If no other workspace exists, explain that a destination must be created and disable submission. Errors remain inline for retry.

The “Open destination workspace after moving” switch starts off for each move. Successful moves close the panel and confirm the named destination in the status message. With the switch off, Grove keeps the current workspace active and clears stale source selection; with it on, membership and active workspace persist together, then Grove opens the destination and selects the moved project.

Placeholders show meaningful example values and never repeat the label or title: Workspace uses `e.g. Platform`. Destructive confirmation starts empty, with the exact target workspace name as its placeholder; enable deletion only after the user types the matching name.

### Main-content checkout card

- The checkout card appears only for a selected session with a registered project checkout and sits in reserved space at the upper right of the main content, outside transcript scrolling and the composer. It uses the same `RADIUS_PANEL` corners as input boxes and icon-led rows: Open in Finder; Add project script or Run script; PR; commit, publish or a counted Push action; Changes.

- Create PR opens an editable GitHub compare form after publication, with title, body and default base prepared for review; creating the PR remains an explicit browser submission. Existing PRs replace the creation action with a linked title and a compact second row containing the PR number, status or check counts, and one contextual action. Metadata and expanded checks/reviews align with the title, leaving the icon column empty.

- Conflicts replace compact check counts and offer Resolve, which opens a fresh native Codex or Claude session with an unsent conflict-resolution draft in the selected checkout; existing drafts are preserved. Expanded details do not repeat the conflict label.

- Merge appears only for a clean synchronized checkout at the PR head and GitHub-reported readiness, checks and reviews. It requires explicit confirmation naming the PR and merge-commit method, revalidates the checkout and PR identity/head/base/title/readiness, and requests a direct server merge pinned to the exact head SHA. Server branch protections apply; auto-merge, queues, admin bypass and automatic mutation retries are not enabled. A failed or uncertain outcome forces a refreshed status and a new confirmation. GitHub CLI, authentication and network failures remain distinct from no existing PR.

- Change summaries show populated Code, Tests, Docs, and Config parents, with Uncategorized only when purpose is unknown. Reliable path conventions supply optional subcategories; ambiguous files stay directly in their parent without an Other row. Parent disclosure expands populated children independently of the parent diff action, and expansion is scoped to the checkout. Optional checkout mappings override conventions. Parent and child actions open the diff using the same classifier, and zero removals are hidden while preserving aligned count columns.

- Existing folder, plus/play, git-commit, and unified changes glyphs share a stable leading column. The section dividers after script and after the complete Changes summary span the entire card, with padding on rows instead of the outer card. Agent rows follow the Changes divider when the session has observed agents; names yield to fixed status labels and full activity remains available in tooltips. There is no project, machine, path, or status header, file-manager submenu, or Fetch/Pull toolbar.

- Multi-root sessions show an explicit Choose checkout action in the Finder slot before enabling target actions. There is no branch row.

- Commit runs inline: a disposable read-only background agent generates a bounded structured message using Codex Luna or Claude Haiku, then Grove validates the captured snapshot and performs the native Git commit. The selected Codex/Claude session chooses the provider, otherwise the configured supported default does; there is no larger-model fallback or change to the selected conversation.

- Staged changes are preferred; with nothing staged, all nonignored tracked and untracked changes form the scope. Generation can be canceled before the atomic transition to committing; native committing disables cancellation. Generating, committing, brief success, and retryable errors stay in the commit row. Navigation never transfers a running job or its result to another checkout. AI attribution trailers are not added.

- A clean unpublished branch offers Publish to an existing remote with explicit confirmation; tracked outgoing commits offer a normal Push. Local clean status never claims remote synchronization. Changes opens the existing diff and shows semantic added/removed counts.

- Narrow layouts put a Checkout disclosure after the shared content header; expanded tools reserve space without covering the composer. Grid, Zen, standalone terminals, home, settings, and setup or decision forms omit the card. Native menus preserve keyboard navigation, Escape, outside dismissal, and trigger focus restoration.

### Composer model and effort

The combined model · effort toolbar control opens a compact rounded card directly above its trigger with a SPACE_LG gap. Access remains a separate control.

The card centers the current effort above a truncating model row with a trailing chevron, flanked by a decorative lightning glyph and a reset action. The lower discrete slider uses catalog-supported effort stops, a blue completed track and neutral remainder, small dots, and a contrasting round thumb. Blue effort text and track fill are permitted in this control; neutral hover and keyboard-focus fills retain stable borders. Light appearance uses the app palette and a dark contrasting thumb. Geometry maps to existing tokens; the card width is a local module constant rather than a new scale tier.

The model row opens the existing catalog and returns to the card after selection.

The slider keeps semantic stops tied to the selected model’s supported catalog. Its thumb and filled track share one inset coordinate: clicks ease toward the selected stop with base motion, while pointer drags use fast responsive motion to follow continuous positions between dots and preview the nearest supported label. Release settles onto that supported stop and commits once; arrow/Home/End input also commits supported efforts for the next turn without submitting. Retargeting begins at the currently displayed position rather than jumping to the old stop. Reduce motion uses immediate endpoints without scheduling animation frames.

Zero supported efforts show an unavailable label with no stops; one supported effort is fixed. Reset restores configured model and effort while preserving Access. Escape cancels a drag and dismisses the card with trigger focus restored; Escape from model choices returns to the model row. Outside clicks dismiss without stealing focus. Catalog/session changes cancel stale drag writes.

## Conversation, tasks, and composer

Assistant prose between user messages forms one response, including multiple work periods. Render one response-level footer; copying it includes all assistant prose, including collapsed work. Assistant timestamp and Copy controls appear on hover or keyboard focus while retaining mounted geometry so content does not jump. Edit belongs to user messages and follows the same geometry rule. Code-copy actions copy literal code and show local feedback.

Task checklists live in the dock above the composer, expanded by default, and persist across turns. Completed work is not stale or dimmed merely because another turn starts. Browse may temporarily compact the dock without overwriting the user’s disclosure preference; compaction preserves completed-work disclosure state. Reserve trailing action space before truncating task titles and expose full titles in tooltips. Waiting for you uses neutral treatment. Tree-row progress sits at the trailing edge; list cards retain their separate treatment. Keep the minimap anchored to the conversation pane independently of task and composer height. Avoid empty choice gaps. Task activity uses the dock. The checkout card owns compact session agent rows with provider glyphs, a running count, Show more, and Previous agents disclosures; disclosure preferences belong to the session. Work-period transcript headers retain historical aggregate summaries. Expanded transcript agent details remain available when the checkout card is absent or collapsed. Git diff metadata belongs to worktree navigation and the checkout tools.

Send and Stop share one composer action position. Stop uses semantic red for interruption; busy text follows the provider’s queue/steering behavior. Steering appears immediately without duplicating a queued message. Keep state hints concise and Shift+Enter discoverable through accessible help. Editing a sent message stays inline and preserves the unrelated composer draft and images.

An edited-message send that restores workspace state uses a pane-local confirmation attached to the actual inline Send control, aligned to its right edge above or below within the pane. Its source aperture is visual only: background mouse, scroll, and embedded HTML interaction remain blocked. Tab stays within Cancel/Confirm; Escape/Cancel restores the editor. Incoming updates must not dislodge the tray, restart follow scrolling, or retarget it. Validate thread and message identity when targeting editing/confirmation; a row index alone is insufficient. A restored editor that bypasses workspace recovery does not require this confirmation.

## Reports and native plans

All inline HTML previews omit a toolbar. Expanded documents retain their controls and return navigation. Integrated reports additionally render as a canvas without an enclosing card/border, with content-measured height subject to the shared cap. The exact `<!-- grove:integrated-report -->` marker enables Grove theme/font/zoom integration. Explicit Open in browser links open externally without inline expansion. Preference updates, including Reduce motion on ordinary HTML and reports, preserve the live document and input state. Check macOS, Linux, and Windows paths when platform behavior changes; one platform’s rendering does not establish parity elsewhere.

Native plan response text preserves authored line breaks using separate literal line/paragraph blocks with readable spacing at narrow and wide widths. Authored feedback markup stays inert. Preview-only removal of presentation markers must not alter the canonical content delivered by Copy or Send. Review responses preserve anchors, and sending requires the explicit Send response action. Review feedback does not authorize implementation.

## Settings and diff headers

Settings uses General, Agents, Terminal, Privacy, and About categories in the shared centered content column. Keep heading/category navigation reachable while the body scrolls; changing category resets body scroll. Nested pages return with Back or Escape and restore source focus; Tab remains within the owning surface. Use shared switch tracks. Copy describes what Run in terminal does for the user, independent of inverted storage flags.

Diff header controls do not shrink or overlap: titles yield, and text actions size automatically with the shared padding. Loading/status feedback belongs in its originating checkout row with a stable icon slot; shimmer does not move adjacent content. Checking status is distinct from a verified clean checkout.

## Maintaining this contract

When a UI change introduces or changes an invariant, update its rule here in the same change. Update the skill when its procedure or verification checks need to change. A fix that restores an already documented rule needs no redundant documentation edit. Record the reusable invariant, not a screenshot-specific literal or an already superseded layout. New shared behavior should use the existing primitive; add focused conformance or interaction coverage when it can catch that failure. Source checks complement native rendering and interaction evidence, and cannot prove visual correctness or live provider behavior.

The [drift audit](.agents/skills/grove-ui/references/drift-audit-2026-10-22.md) maps the October 20–22 fixes to these rules and identifies superseded treatments. The historical migration catalog is provenance only.
