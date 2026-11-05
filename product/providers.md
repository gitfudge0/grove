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
