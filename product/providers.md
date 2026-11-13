# Native provider contracts

`grove-core::provider` owns shared session state, commands, questions, subagents,
submission receipts, recovery and persistence. `codex` and `claude` own their
native transports. Existing `codex` imports remain compatible through reexports;
serialized sessions and the `codex-sessions` recovery directory keep their format.

Capabilities describe support represented by the current snapshot: Codex editing
follows observed history mode, Claude editing uses native forks, and steering
combines Codex transport support with Claude live readiness. Remembered support
does not authorize an action while disconnected; active-turn availability also
checks turn identity, status, compaction and recovery. There is no second
persisted capability cache.

## Model discovery

The Dashboard discovers only its selected provider. Model browsing discovers the
visible provider, or providers with saved favorites in the available scope.
Browsing never changes the launch provider or creates a conversation.

Last successful model lists are stored separately from workspace skills and
configured defaults under `provider-models` in Grove's configuration directory.
Only model metadata, its fetch timestamp and a versioned fingerprint are persisted, using private
atomic writes; credentials, config contents, drafts and transcripts are excluded.
The fingerprint separates provider/account environment, launch configuration,
context roots, local provider overrides and executable identity. Ordinary empty
checkouts can share a catalog; custom configuration keeps conservative isolation.
Disk loading and fingerprinting run on background/provider threads.

Model lists require rediscovery after 24 hours (or a backward clock change), while
remembered rows remain available for display. Source changes invalidate workspace
metadata through shared, provider-specific background scans at five-second
intervals; explicit retry bypasses the scan memo. Matching sessions share a scan
and pass its input fingerprint through discovery rather than scanning twice.
Account/configuration changes also invalidate the model identity.
Codex initialization reuses a fresh known model list
while still reading skills and configured defaults. Claude's required initialize
response includes models, so a necessary workspace/session handshake still
receives them. Model-only browsing can reuse either provider without a handshake,
including after an app restart.

Claude permission-mode discovery shares one `--help` probe per executable and
environment identity, including concurrent requests from different checkouts.
Successful probes expire after 24 hours; failed probes retry after 30 seconds.
Draft command delivery wakes on queued actions rather than polling an idle timer.
New Session retains its draft and worktree rows, reuses matching worktree discovery
for ten seconds and joins an existing refresh. Expired rows stay visible during
background refresh; Retry and successful worktree creation force rediscovery.

**Refresh models** reloads the visible provider scope. Existing rows remain usable
while refreshing; failure retains them and exposes Retry. Refreshed capabilities
reach the owning session and transport without reconfiguring its model, access or
conversation. Successful refresh replaces the shared model catalog. The last
successful disk record survives a failed refresh; it does not turn that refresh
into a success.

`native_catalog_smoke PROVIDER EMPTY_CWD` checks full metadata discovery and warm
reuse. Its optional `models` mode checks model-only reuse in another process;
`refresh-models` forces a fresh lookup. Use disposable `GROVE_CONFIG_DIR` and
`GROVE_PROVIDER_DIAGNOSTICS_DIR` directories to count actual subprocess handshakes.

## Completion and recovery

Codex accepts late tool events for settled turns. Claude waits for authoritative
session idle when the installed CLI reports session-state notifications; a root
result alone does not settle unfinished tool rows. Late tool results retain their
own status and output. Older CLIs without session-state notifications settle at
the result boundary. An `api_error` terminal reason is a failure even when the
result's `is_error` flag is false.

Native thread ownership is held by an OS file lock for the owning provider
process's lifetime. Within one Grove configuration, a second connection to the
same provider/thread refuses before resuming it; new thread IDs are claimed when
reported. Fork targets have independent ownership. Lock files remain on disk so
unlinking cannot split ownership; closing or crashing the process releases the
OS lock. This guard does not detect other native CLI clients or Grove instances
using different configuration directories. Native resume errors preserve identity
and never silently start a replacement conversation.

## Opt-in protocol diagnostics

Set `GROVE_PROVIDER_DIAGNOSTICS_DIR` to an absolute directory before launching
Grove to record one randomly named NDJSON file per Codex or Claude subprocess.
The feature is off by default. Each file stops at 1 MiB; existing files are never
overwritten. Unix captures are created with mode `0600`. Directory ownership and
retention remain with the user; remove captures when no longer needed.

Records contain sequence, timestamp, direction, a fixed protocol-event label,
and an error flag. Unknown event names become `other`. No request IDs, native
thread IDs, prompts, model text, tool arguments/results, stderr, configuration,
or credentials are recorded. Capture failure never interrupts a conversation.
These traces reveal ordering/timing, not full provider payloads.

## Focused live verification

Build the `codex_native_smoke` and `claude_native_smoke` examples. Run each with a
fresh `GROVE_CONFIG_DIR` and a separate empty disposable cwd. They exercise the
production Rust transport, command execution, duplicate-thread refusal, persisted
resume/context retention, and interruption. Enable the diagnostics variable with
a separate disposable capture directory to check actual metadata emission.
Fixtures and unit tests do not establish live provider behavior; record Codex and
Claude results separately, including platform and unavailable checks.

## MCP connection failures

MCP server health is separate from conversation health. In main conversations,
Codex startup-status notifications and Claude's initialization inventory/status
queries produce literal transcript warnings naming the unavailable server,
including the provider-reported error and CLI authentication/reconnect guidance.
Healthy tools and the model turn remain usable. Repeated unchanged failures are
suppressed within a connection; recovery permits a later failure to be reported.
Grove does not retry or change external MCP credentials automatically.

Claude health queries run after initialization, at most once every five seconds.
Unsupported or unanswered queries disable polling for that connection without
failing or blocking the conversation. Configuration and message editing do not
wait for these optional requests. Configurations, headers, and credentials are
not copied from status inventories into notices. Codex CLIs without startup-status
notifications do not provide this warning coverage.

## Context measurements

`SavedSession.context` retains used/window occupancy. Optional `context_details`
preserves provider attribution, model, receipt time and measurement source.
Fresh occupancy replaces old attribution so totals and breakdowns cannot silently
refer to different requests. Claude’s optional `get_context_usage` query retains
supported categories and child counts; reserved/free/deferred rows are not added
to used context. Known legacy category names are supported without guessing the
meaning of new categories. `RefreshContext` is a coalesced, bounded optional query;
unsupported responses and timeouts leave the conversation usable. Its reply is
accepted only for the current context generation/model. Codex uses its latest
thread usage notification and does not invent content attribution.

`context_usage_smoke` exercises the production adapter with disposable cwd and
`GROVE_CONFIG_DIR`, separately for `claude` and `codex`. It reports numeric
measurement coverage without printing prompts, tool payloads or credentials.

Codex context details also retain optional `accounting` from `tokenUsage.last`
and `tokenUsage.total`. Each counter remains optional so older payloads and
reported zero values stay distinct. Input cache subcounts and reasoning output
are preserved without recomputing the provider total. Model changes and
compaction invalidate obsolete latest-request accounting. The Context card does not render request accounting or tool activity.

The selected connected Claude panel requests one full measurement per new
context generation after the provider becomes idle, also recovering restored or
invalidated measurements. Busy sessions continue to receive occupancy events
without repeated category queries. Idle time and redraws alone do not poll full
counting. Errors/timeouts stay nonfatal; manual Refresh can retry. Category
aggregation is presentation-only: original provider categories and child counts
remain retained without fabricated attribution.

## Suggested next replies

Selected native conversation views can request one transient next-user-reply
candidate after a newly observed successful completed response. These isolated,
structured requests use `gpt-6-luna` for Codex and `haiku` for Claude regardless of
the main session model, through existing CLI authentication. They consume provider
usage but never enter the main conversation or perform repository actions.

Input context is bounded to the latest user request and final assistant response;
commentary, tools and reasoning are excluded. Codex jobs ignore user config and
rules, disable hooks and integrations, and use a private cwd and read-only sandbox.
Claude jobs disable tools, hooks, slash commands and session persistence. Model
requests time out after 15 seconds; unavailable models/authentication, malformed
output and cancellation leave normal typing usable without a user-facing error.
Stale results are discarded and process cancellation follows the owning view.
Candidates are not persisted, and typing only matches the fixed candidate locally.
