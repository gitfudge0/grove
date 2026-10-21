# Native agent controls

Grove renders provider requests as live, inline controls, aligned with the conversation and composer. Requests and answers are not saved in `SavedSession`. The subprocess owns tool execution, authentication and permission enforcement.

## Implemented controls

| Provider request | Grove control and response |
| --- | --- |
| Codex `item/tool/requestUserInput`; Claude `can_use_tool` for `AskUserQuestion` | Inset question list at its original transcript position, expandable questions, single/multiple choices, custom or written answers, masked input, validation and cancellation. |
| Codex `item/commandExecution/requestApproval` | Command or terminal-input review; allow once, allow for session, decline/continue, decline/stop; exact offered command-prefix and managed-network policy amendments. Command, directory, reason, additional sandbox permissions and policy proposals are visible. If `availableDecisions` is supplied, only its intersection with implemented decisions is offered. |
| Codex `item/fileChange/requestApproval` | File-change review, root grant details, once/session/decline/stop decisions. |
| Codex `item/permissions/requestApproval` | Display requested filesystem/network permissions; grant exactly that profile for the current turn or session, decline with no grant, or decline and interrupt the turn. |
| Codex deprecated `execCommandApproval`, `applyPatchApproval` | Compatibility review for requests addressed to the current conversation while a turn is active. Responses use the legacy `approved`, `approved_for_session`, structured denial, or `abort` contract. New Grove turns use the modern API. |
| Claude generic `can_use_tool` | Tool identity and original structured input; allow once; optional explicit JSON-object input editing; exact provider-suggested permission updates; decline with written feedback, with optional interruption. Suppressed standing permission suggestions are not offered. |
| Claude `ExitPlanMode` through `can_use_tool` | Named plan approval using the provider's original plan/input and offered permission updates. No synthetic plan-mode transitions are inferred. |
| Codex `mcpServer/elicitation/request`; Claude `elicitation` | Typed form or URL request, showing server identity and message. Forms support required/optional text, email, URI, date/date-time, bounded number/integer, boolean, single select and multiple select, including titled/legacy enum schemas and defaults. Accepted answers retain JSON types. Decline and cancel are distinct protocol responses. |
| MCP URL elicitation | Explicit HTTP(S) browser opening and separate user completion confirmation; no automatic navigation or automatic acceptance. The accepted response omits form content. |

Forms accept `form`, `openai/form` and `openaiForm` mode names when their schemas fit the supported typed subset. This does not advertise support for arbitrary extended JSON Schema. Unsupported patterns, nested objects and advanced constraints fail closed with an actionable message; they are not silently discarded. URL requests reject executable URL schemes and embedded credentials. Form errors prevent submission, including invalid numbers, dates, bounds, required fields and invalid choices.

Question lists keep one row open at a time. Collapsed rows show the actual draft answer, and the header counts answered questions. Reopening a question preserves its draft. Choice selection never submits: one explicit **Send answers** action submits the whole group. The composer remains available while the list scrolls with the transcript.

After a confirmed transport submission, the list becomes a compact read-only question → answer receipt in the same transcript position. A failed send preserves drafts for retry; cancellation and expired requests do not produce successful receipts. Secret answers appear only as **Provided securely** in summaries and receipts. Receipts are transient view state and are never added to `SavedSession`; reopening the view does not restore them. Confirmation means the answer was sent, without claiming the agent has resumed.

## Lifecycle and routing

Controls use exact provider request IDs. Duplicate deliveries do not create duplicate panels. Resolved or cancelled requests disappear; repeated or expired responses are rejected. Turn-bound requests require the current thread and turn. Codex standalone MCP requests with absent/null turn identity survive a turn completion, remain answerable, and keep the session waiting. Disconnect, reconnect and explicit interruption expire pending controls. Claude completion and SDK cancellation clear the matching live controls. Pending controls keep running/status notifications from hiding the waiting state.

Policy choices are constructed from the original provider request inside the core actor. UI data cannot introduce a command prefix, host, permission profile, or suggestion that the provider did not offer. No choice is selected automatically.

## Structured task lists

Both Codex and Claude receive guidance to break long-running work into concrete,
independently completable tasks named for their outcomes. Broad implementation
work is split by deliverable, with distinct checks, installation, and inspection
tracked separately when applicable. Agents refine tasks as scope becomes clear
and update their statuses as work progresses, including delegated work, without
adding trivial steps or inflating the checklist. Simple single-step requests do
not require a task list.

Both agents also receive the integrated HTML report contract: when a visual
report communicates better than prose, write a self-contained HTML document with
the `<!-- grove:integrated-report -->` marker and Grove's `--grove-*` CSS
variables. See [integrated HTML reports](../integrated-html-report/README.md).

Grove enables Codex's native `update_plan` tool with the per-thread configuration
`tools.update_plan.enabled = true` on both `thread/start` and `thread/resume`.
Codex otherwise defaults that tool to disabled; developer instructions asking for
it cannot register the tool. Native `turn/plan/updated` notifications remain the
source of persisted task-list snapshots. Existing sessions pick up this setting
when their Codex thread reconnects; an already running turn retains its tools.

## Explicit capability boundaries

The installed Codex `ServerRequest` schema contains ten variants. All ten are handled: seven user-facing request families above, plus the following three host capabilities:

- `item/tool/call`: returns a typed failed dynamic-tool result because Grove registers no client-side dynamic-tool executor. Configured MCP tools remain subprocess-owned.
- `account/chatgptAuthTokens/refresh`: returns an actionable protocol error directing CLI sign-in and reconnection. Grove does not mint or collect host-managed access tokens.
- `attestation/generate`: returns an actionable unsupported-host protocol error. Grove does not invent attestation tokens.

Claude can also originate host infrastructure requests for registered hook callbacks, SDK MCP message transports, remote tools/plumbing, OAuth/host credential refresh and other specialized remote-host services. Grove does not register those capabilities, and returns explicit protocol errors with visible terminal fallback guidance if they arrive.

Claude's newer `request_user_dialog` API is capability-negotiated through `supportedDialogKinds`. Grove advertises no specialized dialog kinds. An unexpected dialog receives the provider's `cancelled` response and a visible explanation; it is never interpreted as consent. Supporting arbitrary remote/enterprise/account dialogs requires their specific semantics and is outside the controls currently advertised. Existing CLI slash commands remain CLI-owned.

## Evidence and validation

Contracts were checked against the installed Codex app-server JSON schemas (`ServerRequest.json` and the approval/elicitation response schemas) and installed Claude Code 2.1.302 transport implementation. Core regression tests assert exact wire envelopes, offered-choice filtering, rejection of forged choices, permission scope preservation, legacy responses, edited-input validation, denial feedback, typed MCP values, URL confirmation without content, duplicate/stale request handling, cancellation, disconnect and standalone MCP lifetime. UI tests exercise rendered choices/forms, validation, submission and multiple requests. Final build/test/lint results are reported with delivery.
