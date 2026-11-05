# Header, return navigation and action consistency audit

Audit date: 2026-11-05. Baseline: working tree at `e434708`, including pre-existing local modifications. This is an audit and proposed direction, not an approved contract change or implementation. No application source was changed.

## Implementation follow-up

The user subsequently authorized implementation. The audit below is retained as the original source assessment; its “no changes/checks” statements describe that assessment, not the follow-up.

Shared `content_header`, `content_header_title`, `back_action` and `form_actions` recipes now own header composition, title hierarchy, labeled return controls and wrapping bottom actions. Settings, project management, project setup, history, file viewer, changes, expanded HTML and agent/script previews adopt the relevant recipes. Archived projects return through their header; wizard step navigation remains in its footer. Checkout and rewind confirmations group secondary actions before primaries; worktree creation follows the same order. Release Close retains the complete shared size. Update/restart gets primary emphasis, and plan feedback review keeps approval visible with secondary emphasis. DESIGN.md records the approved invariants and exceptions.

Independent source review found no material handler or provider-dispatch regression. A broader header fixture exposed a stale expectation for a non-grid session Close button: existing HEAD already renders that button only in Grid. The fixture now verifies its absence in the non-grid header and preserves the separate Grid close-placement checks. No production close behavior was changed for that correction.

Observed verification:

- Compilation passed (`cargo check -p grove`).
- Formatting and whitespace checks passed. Final header batch: 23 passed, one ignored native capture executed separately. Design conformance: 32 passed, including the new shared bottom-action guard.
- Settings: 16 tests passed, one native capture test excluded from the ordinary run.
- Project management: five tests passed.
- Shared header fixed-slot and wrapping-action geometry passed at wide/narrow widths and changed rem zoom.
- Both Codex and Claude native-plan review **fixtures** passed; these exercise local review/draft/send behavior, not live provider boundaries.
- Rewind confirmation background blocking and composer-preservation fixture passed.
- Linux real GPUI Settings capture passed and screenshots were inspected in Grove Dark, Grove Light and 24px rem zoom. Back, title and Close remained visible. Captures: `/tmp/grove-settings-header-grove-dark-16.png`, `/tmp/grove-settings-header-grove-light-16.png`, `/tmp/grove-settings-header-grove-dark-24.png`. Sampled solid Back-label foreground/background contrast was 8.18:1 dark and 5.70:1 light; this does not certify every interaction state.

Clippy was attempted and failed on four existing denied `unwrap()` calls in unchanged `grove-core/src/codex.rs` and `grove-core/src/review_brief.rs`; the same calls are present in HEAD. No lint rules were weakened. The initial broad header batch failed its stale non-grid Close expectation; after the fixture correction the entire batch passed.

`./install.sh` completed once after the final code changes, building the local profile and installing the executable and launcher under `~/.local`. The installed executable's SHA-256 matched `target/local/grove`. No release was cut and the running user app was not restarted.

Live Codex and Claude behavior remains **untested**. Native inspection covered the nested Settings screen, not every migrated view or hover/focus/pressed/disabled combination. macOS and Windows were not exercised.

## Finding

Grove shares control dimensions, typography tokens, colors and several complete button recipes, but does not share a semantic header composition. Each owner decides independently where return navigation, title, status, dismissal and actions belong. Consequently, individually familiar controls produce inconsistent screens. The main opportunity is a small family of headers and action groups with explicit roles and exceptions.

Evidence below comes from source inspection. “Confirmed” means the composition or recipe call exists in the current source; it does not mean the resulting native geometry or interaction has been observed. No overall visual score is assigned without native inspection.

## Prioritized findings

### 1. P1: Return navigation changes location and vocabulary

- Nested Settings places arrow-left + Back after the title, beside Close: [settings_panel.rs](../../src/views/settings_panel.rs#L1766).
- Files places an icon-only arrow-left before the path: [file_browser.rs](../../src/views/file_browser.rs#L821).
- Commit history places chevron-left + Back before the heading: [branch_history.rs](../../src/views/sidebar/branch_history.rs#L826).
- Changes replaces the canvas but exits through a trailing Close: [session_diff.rs](../../src/views/sidebar/session_diff.rs#L219).
- Expanded HTML places Return to conversation after Source/Preview and version actions: [inline_html.rs](../../src/views/inline_html.rs#L1433).

Users must rediscover how to return whenever the content owner changes. This is a cross-screen consistency gap, not proof that each owner violates its current specific contract. File viewer's icon-only Back is expressly required today.

Proposed rule: replacing a screen gets a leading return slot, with arrow-left and an accessible destination-specific name. Dismissing a transient surface gets a trailing Close slot. Keep a visible Back label for general nested screens; compact document viewers can retain the approved icon-only form. Escape should perform the same return and restore source focus without losing the draft or reading state. Do not conflate Close with destructive cancellation.

### 2. P1: Project setup spreads bottom form actions apart

[project_setup.rs](../../src/views/sidebar/project_setup.rs#L735) builds a footer whose inner row uses `justify_between()` at line 751, with secondary then primary and no wrapping. DESIGN.md already requires bottom form actions to stay together in one left-aligned wrapping row at the content inset.

This is a confirmed source-level contract mismatch. Group Cancel/Back and Continue/Add project in the shared wrapping row. Preserve the distinction between Canceling the wizard and going to its previous step. Native narrow-width and zoom checks must establish whether clipping currently occurs; this audit does not claim observed clipping.

### 3. P1: Checkout confirmations stack primary and Cancel separately

[checkout_panel.rs](../../src/views/sidebar/checkout_panel.rs#L1843) builds a vertical form, adds operation-specific primary actions among its children, then appends Cancel at [line 1998](../../src/views/sidebar/checkout_panel.rs#L1998). Operation messages may appear between them.

This diverges from the documented common horizontal wrapping action row and from other confirmations. Put consequences and status in the body, followed by a single grouped action row. Preserve busy guards, destructive meaning, attached confirmation anchoring, and initial/final focus.

### 4. P2: Edit-from-here confirmation aligns its actions to the opposite edge

[codex_view.rs](../../src/views/codex_view.rs#L6783) uses a wrapping `justify_end()` row for Cancel, Keep files and Restore files. The contract's shared bottom-action alignment is left aligned. This is source-confirmed; the confirmation's safety copy and distinct Keep/Restore semantics should remain intact.

### 5. P2: Primary/secondary order is not consistent across creation forms

Project setup adds secondary then primary ([project_setup.rs](../../src/views/sidebar/project_setup.rs#L753)); New worktree adds Create worktree then Cancel ([content.rs](../../src/views/sidebar/content.rs#L1937)). The latter correctly wraps, but users encounter a reversed action order.

The current contract fixes grouping/alignment, not a universal order. Proposed addition: secondary first, primary last within the left-aligned wrapping group, with DOM/native keyboard order matching visual order. Choose and document the order once rather than silently treating the proposal as an existing rule.

### 6. P2: Equivalent preview dismissal moves between header and footer

Agent preview appends a labeled Close preview after its body and freshness metadata ([agent_preview.rs](../../src/views/sidebar/agent_preview.rs#L488)). Script preview places icon-only Close at the top right ([script_preview.rs](../../src/views/sidebar/script_preview.rs#L370)).

Both are anchored previews that can be pinned and dismissed. Use a fixed trailing header dismissal slot for this family, retaining each preview's content and pointer-intent behavior. This is a recommended cross-surface invariant, not an existing explicit placement requirement.

### 7. P2: Heading hierarchy is inconsistent; History uses display type in chrome

History's populated destination header uses `TEXT_DISPLAY` ([session_history.rs](../../src/views/sidebar/session_history.rs#L327)), while Settings uses semibold `TEXT_TITLE` ([settings_panel.rs](../../src/views/settings_panel.rs#L1759)) and project setup uses medium `TEXT_BODY` in `HEADER_H` ([project_setup.rs](../../src/views/sidebar/project_setup.rs#L683)). Insets vary across these owners. File viewer is deliberately compact and borderless.

The display-size History heading conflicts with the explicit display-type restriction to sparse empty states/onboarding in DESIGN.md and tokens.rs. Other differences need semantic classification rather than blanket normalization. Define a page-header title role and a compact viewer-header role, then make inset, title weight and action reservation consistent within each family.

### 8. P2: Plan feedback review gives two consequential actions equal primary emphasis

When review is open, [native_plan.rs](../../src/views/native_plan.rs#L1358) creates Send response with primary styling, then [line 1370](../../src/views/native_plan.rs#L1370) adds Approve & implement with primary styling as well. The same row also contains Back to plan and Copy response.

This is a hierarchy concern: sending feedback and authorizing implementation are different commitments. Their separation is required and must survive any redesign. Keep explicit approval visible with its unavailable reason, but visually distinguish it from the current feedback step, for example through a separate approval group. Do not merge the actions or make sending feedback implicitly approve.

Back to plan also uses `unified.svg` ([line 1331](../../src/views/native_plan.rs#L1331)), whereas other Back controls use directional navigation glyphs. It is an in-place body toggle, but its return wording should still use the shared return icon if retained.

### 9. P2: Some header actions bypass or override the complete action recipe

Changes header builds labeled Refresh and Close through `Sidebar::control`, then applies local width, padding and text overrides ([session_diff.rs](../../src/views/sidebar/session_diff.rs#L205)). History instead uses `dense_action_with_colors` ([branch_history.rs](../../src/views/sidebar/branch_history.rs#L876)); Settings intentionally uses `settings_control`.

Release highlights calls the complete `form_action` recipe, then overrides its vendor size with `.small().compact()` for Close ([release_highlights.rs](../../src/views/release_highlights.rs#L414)). The contract requires the complete shared action size, including the vendor inner tier. This override is a source-level recipe mismatch; visible size differences need rendered measurement.

Standardize header action roles through complete shared recipes. Do not classify sanctioned Settings selection controls as defects merely because their implementation differs.

### 10. P3: About update actions have weak prioritization

[settings_panel.rs](../../src/views/settings_panel.rs#L1416) can show Update now, Skip version, Copy URL, Check now and Changelog in one wrapping action group, all using the same unselected settings treatment. The appropriate next step has no distinct prominence in that group.

Keep the current update or restart action prominent and group secondary utilities separately. This is a hierarchy recommendation, not a demonstrated functional defect or a rule that every group must always have a primary button.

## Recommended pattern family

| Family | Leading area | Title/content | Trailing area | Exceptions |
|---|---|---|---|---|
| App/window header | Platform controls and sidebar toggle | App identity and drag region | Grid, Settings | Preserve platform window behavior and current persistent toolbar alignment |
| Destination/page header | Optional return to parent | Shared page title role, optional supporting context | Screen-scoped actions; Close only where it dismisses an owning surface | Settings retains its centered content column and fixed category navigation |
| Compact viewer header | Return action | Truncating path/title | Fixed view modes and utilities | File viewer retains icon-only Back and no divider; Git metadata can have a secondary row |
| Anchored peek header | Preview identity | Truncating identity/status | Expand where supported, Close | Preserve source attachment, pointer grace, pinning and source focus |
| Section/disclosure header | Disclosure or status mark | Section label and count | Local Add/Manage actions | Foreground-only disclosure hover; expansion is not selection |
| Wizard footer | Secondary Cancel or previous-step Back | No page-return control mixed into content | Primary within the same left-aligned group | Step navigation remains in the footer |
| Form/confirmation action row | Secondary actions | One wrapping group aligned to body column | Primary last in the group, as proposed | Destructive and alternate outcomes retain explicit names |

Across families: title text yields before controls; icon precedes text with `SPACE_SM`; loading preserves icon slots; action groups wrap deliberately; hover/focus/press/disabled states come from the shared complete recipes. Existing tokens, theme roles and control geometry remain authoritative. Add semantic composition helpers rather than another independent dimension scale.

## Differences to preserve

- Persistent Files, Side chat and Side panel controls belong outside individual transcript headers. Do not pull them into a universal page header.
- Checkout heading is project/worktree identity inside an inset field-well card, not a destination title.
- Session headers carry session status, title editing, drag behavior and project/checkout links. Preserve fixed controls and foreground-only link feedback.
- Workspace Save/Delete attached to an editable field is an inline editing pattern, not a page footer. Keep the compact form when it has a clear accessible action name.
- Session History detail has a persistent adjacent list; it need not gain redundant Back navigation unless that list disappears.
- Release carousel's Back/Next is sequence navigation. Its progress dots and sequence footer can remain distinct from full form footers.
- Markdown plan peeks are read-only references; feedback/approval belongs to the permitted inline or expanded surfaces. Task/status disclosures and question/approval cards retain their own attachment and selection semantics.
- Source/Preview are view modes. The file viewer's single toggle and expanded HTML's paired choices are not automatically interchangeable; normalize their header placement before deciding whether their behavior should converge.

## Coverage and verification limits

The audit inventoried all 58 files under `src/views` and searched header, navigation and action construction across the view tree. Focused reads covered shared components/tokens, shell/window header, Settings, workspace manager, project setup/management, worktree creation, checkout confirmation, file explorer/viewer, session and commit history, changes viewer, agent/script peeks, native plan actions, expanded HTML, release highlights, and transcript edit confirmation. Additional inventory covered dashboard/navigation, session content headers, side chat, worktree terminal, model picker and section/disclosure owners. This is broad source coverage, not an exhaustive execution of every conditional UI state or a full review of every line.

Two independent source assessments covered headers/return navigation and action composition. The applicable authority was current DESIGN.md plus tokens/components/theme, not archived CSS or generated mockups. The web markup detector was not run: this app's native GPUI Rust views are not a supported HTML/JSX/Vue/Svelte target. Targeted source searches provided the deterministic inventory instead.

No build, test suite, installation, screenshots or native UI interaction checks were run. Available computer-use tooling disables native app control; source inspection cannot establish runtime visual conformity. The changes in this task are limited to this audit document, which needs no build or installation. Existing application edits were left intact.

| Acceptance area | Status | Evidence still needed |
|---|---|---|
| Source composition findings | Verified by source inspection | Native appearance and behavior remain separate |
| Codex | Live behavior untested | Return/draft/focus preservation, shared transcript/plan actions and provider ownership in a live Codex session |
| Claude | Live behavior untested | Same acceptance checks in a live Claude session; no inferred parity from shared Rust views |
| Linux | Native rendering/interactions untested | Wide/narrow layouts, long labels, light/dark themes, default/changed rem zoom |
| macOS and Windows | Untested | Same relevant states plus platform window controls/keyboard routing |
| Interaction states | Untested | Rest, hover, keyboard focus, pressed/released, selected/open, disabled/loading and Escape/source focus |

## Suggested implementation sequence

1. Fix existing contract mismatches in form-action grouping, History title role and release Close size override.
2. Approve the return-versus-dismiss convention and the small header family; update DESIGN.md with those semantic rules.
3. Introduce shared header/action-group compositions, migrate Settings and replacement viewers, then anchored peeks and remaining page owners.
4. Verify affected native states and both live providers before claiming app-wide consistency. Extend conformance checks to guard composition invariants, without treating source checks as a replacement for native QA.
