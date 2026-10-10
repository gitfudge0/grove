---
name: grove
description: Control the Grove desktop app through its CLI to launch, inspect, and coordinate agent sessions in separate git worktrees. Use for operating Grove and delegating work through it.
---

<!-- grove-managed-skill: v1 -->

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

Register a project when the user requests it, or choose an existing registered project. List workspaces before selecting an ID. Paths must be absolute. Commands mutate the desktop's live state and persisted configuration; do not edit its saved JSON directly.

```sh
grove workspaces list --json
grove workspaces create --name "Platform" --json
grove projects add --name "API" --path /absolute/api --workspace WORKSPACE_ID --json
grove projects edit --project /absolute/api --name "API service" --json
grove projects move --project /absolute/api --workspace WORKSPACE_ID --json
grove workspaces rename WORKSPACE_ID --name "Services" --json
grove workspaces select WORKSPACE_ID --json
```

If a registered folder is not yet a Git repository, use `projects init-git --project PATH` when the user wants to initialize it.

Project lifecycle scripts use `projects edit --setup-file FILE`, `--run-file FILE`, and `--teardown-file FILE`; an empty file clears the corresponding script. `sessions run --project PATH --worktree PATH` launches the configured run script. Inspect the script and the user's intent before triggering it.

Archive and restore with `projects archive --project PATH` and `projects restore --project PATH`. Archiving retains the project. `projects delete --project PATH --confirm EXACT_NAME` requires an archived project with no registered sessions and removes its registration while retaining checkout files. `projects remove --project PATH --confirm EXACT_NAME --remove-worktrees true` can also remove worktrees; `--remove-worktrees false` retains them. Project removal closes its sessions and optional worktree cleanup removes worktrees directly; it does not run their teardown scripts. Individual `worktrees remove` runs the configured teardown script before Git removal. Follow the returned operation state with `projects removal-status --path PATH` until finished. Delete an empty workspace with `workspaces delete ID --confirm EXACT_NAME`. Destructive operations require authorization for the target and scope; the confirmation flag alone is not authorization. If the desktop is unavailable, report that connection failure rather than independently launching agents or editing Grove's saved state.

`sessions shell` starts a standalone home terminal. `sessions shell --project PATH --worktree PATH` starts a checkout panel shell. Both appear in session list/show/logs/input/focus/stop. Panel shells need an open managed session in their checkout to become visible when focused; standalone shells do not. `sessions input` returns an error if a terminal cannot attach, and `outcome_unknown` if a write may have partially succeeded. Inspect the terminal before sending the same bytes again.

`--root PATH` accepts registered main checkouts or Git worktrees, not arbitrary accessible directories. Claude and Codex receive additional-root arguments; OpenCode and Terminal use Grove’s temporary context bundle via `GROVE_MULTI_ROOT`.

## Install or update this skill

In Grove Settings, choose the skill installation scope (your user account or a registered project), then Install or Update beside Codex, Claude Code, or OpenCode. The CLI offers the same operation:

```sh
grove skills status --agent codex --project /absolute/project --json
grove skills install --agent codex --project /absolute/project --json
```

Omit `--project` for user scope. Grove updates its own managed skill; it preserves foreign content and rejects symlinked agent directories. Use `--overwrite true` only when the user authorizes replacing a conflicting skill. User locations are `~/.agents/skills/grove`, `~/.claude/skills/grove`, and `$XDG_CONFIG_HOME/opencode/skills/grove` (default `~/.config/opencode/skills/grove`). Project locations are `.agents/skills/grove`, `.claude/skills/grove`, and `.opencode/skills/grove`. Installed agents discover `SKILL.md` there.

## Delegate bounded work

Use parallel sessions for independent subtasks with explicit ownership. Respect the user's chosen concurrency; otherwise start with at most two workers and avoid recursive delegation unless it is necessary within that limit. Give each editing worker a separate worktree and one writer per worktree. Record the starting commit SHA and use it as the base for each worktree:

```sh
git -C /absolute/project rev-parse HEAD
grove worktrees create --project /absolute/project --name worker-a --branch feature/worker-a --base BASE_SHA --json
```

Use the returned worktree path. Worktrees share the repository's refs and do not isolate ports, databases, credentials, or external services; allocate shared resources explicitly when subtasks need them.

Prepare a UTF-8 prompt file with the scope, owned files or responsibility, base SHA, expected outcome, relevant checks, and constraints. Tell the worker to preserve unrelated work and report unresolved issues. Grove supplies `GROVE_TASK_ID` and `GROVE_CONFIG_DIR` to the task's session; include instructions for submitting its result. Keep prompts and result files outside another worker's editable paths. Task creation requires both `--task-title` and `--task-file`; the task file stores plain text instructions and may be the same file as the prompt. Use `--parent-task TASK_ID` when delegating from an existing Grove task.

```sh
grove sessions start --project /absolute/project --worktree /returned/worktree \
  --agent codex --backend tmux --prompt-file /absolute/worker-prompt.txt \
  --task-title "Implement the parser" --task-file /absolute/worker-prompt.txt \
  --request-id UNIQUE_REQUEST_KEY --json
```

For additional registered main checkouts or Git worktrees, repeat `--root /absolute/path`. Claude and Codex receive their native additional-directory arguments; OpenCode and Terminal receive a temporary symlink context bundle through `GROVE_MULTI_ROOT`. Roots preserve the primary working checkout and do not give another worker ownership.

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

To send follow-up input, create a UTF-8 input file and run `grove sessions input SESSION_ID --input-file /absolute/input.txt --json`. This sends the file bytes directly to the running terminal without adding Enter. For a shell or interactive agent, include a carriage return (`\r`) to send the terminal’s Enter key; a newline (`\n`) may behave differently depending on that program’s input mode. Check session identity and state first. Treat it as executing that input in the session's current context, including a shell if the agent has exited. Use `sessions start --agent terminal` for a managed shell without prompt/task inputs.

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

Keep worktrees and result evidence until the work is integrated or explicitly discarded. To remove an authorized, disposable worktree, run `grove worktrees remove --project /absolute/project --worktree /absolute/worktree --confirm /absolute/worktree --json`, then poll `grove worktrees removal-status --path /absolute/worktree --json`. Removal stops its sessions and can execute teardown scripts; `grove worktrees skip-teardown --path /absolute/worktree --confirm /absolute/worktree` skips a pending teardown when explicitly desired. Inspect uncommitted work before removing it. Use the returned status rather than assuming acceptance means completion.
