---
name: grove
description: Control the Grove desktop app through its CLI to launch, inspect, and coordinate agent sessions in separate git worktrees. Use for operating Grove and delegating work through it.
---

# Operate Grove

Grove owns agent sessions and worktrees; use its CLI to coordinate work while the user can inspect sessions in the desktop. This skill guides use of Grove, rather than development of Grove itself.

## Connect and discover

The desktop must already be running. Control commands use a private local Unix socket on Linux/macOS; Windows control is unsupported. There is no headless owner or automatic desktop launch. One desktop owns each configuration directory; a second bare `grove` errors while that owner is running. `grove --help` and `grove --version` work without a running app. Use the same `GROVE_CONFIG_DIR` as the desktop when it has a custom configuration directory.

Read `grove --help` and relevant command help before operating an unfamiliar version. All responses are JSON; `--json` is also accepted. Responses use `{ "version": 1, "ok": true, "data": ..., "error": null }`; executed requests also emit `request_id`, and errors have `ok: false`. Check both exit status and the envelope. Use returned opaque IDs rather than sidebar positions or labels.

```sh
grove projects list --json
grove worktrees list --project /absolute/project --json
grove sessions list --json
grove tasks list --json
```

Choose an existing registered project. If the desktop is unavailable, report that connection failure rather than independently launching agents or editing Grove's saved state.

## Delegate bounded work

Use parallel sessions for independent subtasks with explicit ownership. Respect the user's chosen concurrency; otherwise start with at most two workers and avoid recursive delegation unless it is necessary within that limit. Give each editing worker a separate worktree and one writer per worktree. Record the starting commit SHA and use it as the base for each worktree:

```sh
git -C /absolute/project rev-parse HEAD
grove worktrees create --project /absolute/project --name worker-a --base BASE_SHA --json
```

Use the returned worktree path. Worktrees share the repository's refs and do not isolate ports, databases, credentials, or external services; allocate shared resources explicitly when subtasks need them.

Prepare a UTF-8 prompt file with the scope, owned files or responsibility, base SHA, expected outcome, relevant checks, and constraints. Tell the worker to preserve unrelated work and report unresolved issues. Grove supplies `GROVE_TASK_ID` and `GROVE_CONFIG_DIR` to the task's session; include instructions for submitting its result. Keep prompts and result files outside another worker's editable paths. Task creation requires both `--task-title` and `--task-file`; the task file stores plain text instructions and may be the same file as the prompt. Use `--parent-task TASK_ID` when delegating from an existing Grove task.

```sh
grove sessions start --project /absolute/project --worktree /returned/worktree \
  --agent codex --backend tmux --prompt-file /absolute/worker-prompt.txt \
  --task-title "Implement the parser" --task-file /absolute/worker-prompt.txt \
  --request-id UNIQUE_REQUEST_KEY --json
```

Choose `claude`, `codex`, or `opencode` when their CLI is installed; `terminal` starts a shell and rejects task/prompt inputs. Select the backend explicitly: `tmux` survives desktop restarts when tmux is installed; `native` ends with the desktop. A failed tmux launch can fall back to native, so inspect the returned actual backend before relying on persistence. Launches honor Grove's existing agent permission settings; do not enable bypass settings for delegation.

Save the returned session, task, and request IDs. If a launch response is lost, retry with the same request ID and identical arguments/files. A new request ID can create another worker. Do not reuse a request ID for different instructions; conflicting inputs are rejected. Request receipts are durable and capped at 4,096. A pending receipt after a crash requires inspection of sessions/tasks before recovery; do not automatically start a replacement worker. If the receipt limit is reached, report the error rather than switching to unmanaged launches.

## Observe, collect, and verify

```sh
grove sessions show SESSION_ID --json
grove sessions logs SESSION_ID --lines 100 --json
grove tasks show TASK_ID --json
grove tasks wait TASK_ID --timeout 60 --json
grove sessions focus SESSION_ID --json
```

Logs expose bounded current-screen text with `scope: "live_screen"`, not a historical transcript. Use bounded waits and inspect the returned task state. A wait succeeds only for completed tasks; failure, cancellation, interruption, and timeout return nonzero. A timeout does not cancel the worker. Session activity and the sidebar's “done” indicator are hints, not proof of task completion. A task can be starting, running, awaiting a result, completed, failed, cancelled, or interrupted. Task records survive desktop restarts. Native sessions cannot be resumed after a restart; surviving tmux sessions can reconnect with their stable IDs. Investigate interrupted tasks and preserve their work before deciding whether to launch a replacement.

Workers submit a JSON result file with this shape:

```json
{
  "summary": "Describe the outcome",
  "changed_files": ["src/parser.rs"],
  "checks": ["cargo test parser: passed"],
  "unresolved": [],
  "status": "completed"
}
```

Use `"status": "failed"` when the requested outcome was not achieved. Submit from a task's session with:

```sh
grove tasks complete "$GROVE_TASK_ID" --result-file /absolute/result.json --json
```

A completed record is the worker's report, not independent verification. The coordinator reviews the actual diff and check evidence, resolves conflicts deliberately, and checks the integrated behavior before reporting success. Task delegation does not itself authorize commits, merges, pushes, publishing, or destructive cleanup; preserve the user's requested endpoint.

To cancel a worker, stop only its designated session and inspect the resulting task state:

```sh
grove sessions stop SESSION_ID --json
grove tasks show TASK_ID --json
```

Keep worktrees and result evidence until the work is integrated or explicitly discarded. The control CLI has no worktree deletion command. Do not use forced Git cleanup as a substitute.
