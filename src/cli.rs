//! Command-line client; storage and session mutations remain in the desktop.
use grove_core::{
    agent::Agent,
    control::{
        self, ControlCommand, ControlRequest, ControlResponse, TaskResult, TaskSpec, VERSION,
    },
};
use serde_json::json;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

const HELP: &str = r#"Grove local control (open the Grove desktop first)

Projects:
  grove projects list
  grove projects add --name NAME --path PATH [--workspace ID]
  grove projects edit --project PATH [--name NAME] [--setup-file FILE] [--run-file FILE] [--teardown-file FILE]
  grove projects init-git --project PATH
  grove projects move --project PATH --workspace ID
  grove projects archive|restore --project PATH
  grove projects delete --project PATH --confirm NAME
  grove projects remove --project PATH --confirm NAME [--remove-worktrees true|false]
  grove projects removal-status --path PATH
Workspaces:
  grove workspaces list
  grove workspaces create --name NAME
  grove workspaces rename ID --name NAME
  grove workspaces select ID
  grove workspaces delete ID --confirm NAME
Worktrees:
  grove worktrees list --project PATH
  grove worktrees create --project PATH --name NAME [--base REF] [--branch BRANCH]
  grove worktrees remove --project PATH --worktree PATH --confirm PATH
  grove worktrees removal-status --path PATH
  grove worktrees skip-teardown --path PATH --confirm PATH
Sessions:
  grove sessions list
  grove sessions show|focus|stop ID
  grove sessions start --project PATH --worktree PATH --agent claude|codex|opencode|terminal
    [--root PATH ...] [--prompt-file FILE] [--backend native|tmux]
    [--task-title TITLE --task-file FILE] [--parent-task ID]
  grove sessions shell [--project PATH --worktree PATH]
  grove sessions run --project PATH --worktree PATH
  grove sessions input ID --input-file FILE
  grove sessions logs ID [--lines N]
Tasks:
  grove tasks list
  grove tasks show ID
  grove tasks wait ID [--timeout SECONDS]
  grove tasks complete ID --result-file FILE
Skills:
  grove skills status --agent codex|claude|opencode [--project PATH]
  grove skills install --agent codex|claude|opencode [--project PATH] [--overwrite true|false]

All responses are JSON envelopes; --json may appear anywhere.
--request-id ID identifies a retry of the same mutation and inputs.
Paths should be absolute. Removal acceptance is asynchronous; inspect removal-status until finished and check errors.
Project remove unregisters and closes sessions; optional worktree cleanup preserves the main checkout.
Archived project delete removes registration only. Confirm exact project name or canonical worktree path.
An empty script file clears that script. Input files send literal UTF-8 PTY bytes including any newline.
Task instructions are plain text. Result files are JSON:
{"status":"completed|failed","summary":"...","changed_files":[],"checks":[],"unresolved":[]}
Bare grove opens the desktop. --help and --version work without it."#;
struct Parsed {
    command: ControlCommand,
    request_id: String,
    wait: Option<u64>,
}
fn take_required(flags: &mut HashMap<String, String>, name: &str) -> Result<String, String> {
    flags
        .remove(name)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("Missing --{name}"))
}
fn read_file(path: &str) -> Result<String, String> {
    let meta = fs_err::metadata(path).map_err(|e| format!("Cannot read {path}: {e}"))?;
    if meta.len() > 512 * 1024 {
        return Err("Input file exceeds 512 KiB".into());
    }
    fs_err::read_to_string(path).map_err(|e| format!("Cannot read {path}: {e}"))
}
fn parse(args: Vec<String>) -> Result<Parsed, String> {
    let mut positionals = Vec::new();
    let mut flags = HashMap::new();
    let mut roots = Vec::new();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "--json" {
            continue;
        }
        if let Some(name) = arg.strip_prefix("--") {
            let value = it
                .next()
                .ok_or_else(|| format!("Missing value for {arg}"))?;
            if value.starts_with("--") {
                return Err(format!("Missing value for {arg}"));
            }
            if name == "root" {
                roots.push(value);
                continue;
            }
            if flags.insert(name.to_owned(), value).is_some() {
                return Err(format!("Duplicate {arg}"));
            }
        } else {
            positionals.push(arg);
        }
    }
    let request_id = match flags.remove("request-id") {
        Some(id) if !id.is_empty() && id.len() <= 128 => id,
        Some(_) => return Err("--request-id must contain 1–128 bytes".into()),
        None => control::random_id()?,
    };
    let mut wait = None;
    let command = match positionals
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["skills", "status"] => ControlCommand::SkillStatus {
            agent: take_required(&mut flags, "agent")?,
            project: flags.remove("project"),
        },
        ["skills", "install"] => ControlCommand::InstallSkill {
            agent: take_required(&mut flags, "agent")?,
            project: flags.remove("project"),
            overwrite: match flags.remove("overwrite").as_deref() {
                None | Some("false") => false,
                Some("true") => true,
                _ => return Err("--overwrite must be true or false".into()),
            },
        },
        ["projects", "init-git"] => ControlCommand::InitProjectGit {
            project: take_required(&mut flags, "project")?,
        },
        ["worktrees", "skip-teardown"] => ControlCommand::SkipWorktreeTeardown {
            path: take_required(&mut flags, "path")?,
            confirm: take_required(&mut flags, "confirm")?,
        },
        ["projects", "list"] => ControlCommand::ListProjects,
        ["projects", "add"] => ControlCommand::AddProject {
            name: take_required(&mut flags, "name")?,
            path: take_required(&mut flags, "path")?,
            workspace: flags
                .remove("workspace")
                .map(|v| v.parse::<u64>().map_err(|_| "Invalid workspace ID"))
                .transpose()?,
        },
        ["projects", "edit"] => ControlCommand::EditProject {
            project: take_required(&mut flags, "project")?,
            name: flags.remove("name"),
            setup: flags
                .remove("setup-file")
                .map(|p| read_file(&p))
                .transpose()?,
            run: flags
                .remove("run-file")
                .map(|p| read_file(&p))
                .transpose()?,
            teardown: flags
                .remove("teardown-file")
                .map(|p| read_file(&p))
                .transpose()?,
        },
        ["projects", "move"] => ControlCommand::MoveProject {
            project: take_required(&mut flags, "project")?,
            workspace: take_required(&mut flags, "workspace")?
                .parse()
                .map_err(|_| "Invalid workspace ID")?,
        },
        ["projects", "archive"] => ControlCommand::ArchiveProject {
            project: take_required(&mut flags, "project")?,
        },
        ["projects", "restore"] => ControlCommand::RestoreProject {
            project: take_required(&mut flags, "project")?,
        },
        ["projects", "delete"] => ControlCommand::DeleteProject {
            project: take_required(&mut flags, "project")?,
            confirm: take_required(&mut flags, "confirm")?,
        },
        ["projects", "remove"] => ControlCommand::RemoveProject {
            project: take_required(&mut flags, "project")?,
            confirm: take_required(&mut flags, "confirm")?,
            remove_worktrees: match flags.remove("remove-worktrees").as_deref() {
                None | Some("false") => false,
                Some("true") => true,
                _ => return Err("--remove-worktrees must be true or false".into()),
            },
        },
        ["projects", "removal-status"] => ControlCommand::ProjectRemovalStatus {
            path: take_required(&mut flags, "path")?,
        },
        ["worktrees", "remove"] => ControlCommand::RemoveWorktree {
            project: take_required(&mut flags, "project")?,
            worktree: take_required(&mut flags, "worktree")?,
            confirm: take_required(&mut flags, "confirm")?,
        },
        ["worktrees", "removal-status"] => ControlCommand::WorktreeRemovalStatus {
            path: take_required(&mut flags, "path")?,
        },
        ["workspaces", "list"] => ControlCommand::ListWorkspaces,
        ["workspaces", "create"] => ControlCommand::CreateWorkspace {
            name: take_required(&mut flags, "name")?,
        },
        ["workspaces", "rename", id] => ControlCommand::RenameWorkspace {
            id: id.parse().map_err(|_| "Invalid workspace ID")?,
            name: take_required(&mut flags, "name")?,
        },
        ["workspaces", "select", id] => ControlCommand::SelectWorkspace {
            id: id.parse().map_err(|_| "Invalid workspace ID")?,
        },
        ["workspaces", "delete", id] => ControlCommand::DeleteWorkspace {
            id: id.parse().map_err(|_| "Invalid workspace ID")?,
            confirm: take_required(&mut flags, "confirm")?,
        },
        ["sessions", "run"] => ControlCommand::RunScript {
            project: take_required(&mut flags, "project")?,
            worktree: take_required(&mut flags, "worktree")?,
        },
        ["sessions", "input", id] => ControlCommand::SessionInput {
            id: (*id).into(),
            text: read_file(&take_required(&mut flags, "input-file")?)?,
        },
        ["worktrees", "list"] => ControlCommand::ListWorktrees {
            project: take_required(&mut flags, "project")?,
        },
        ["worktrees", "create"] => ControlCommand::CreateWorktree {
            project: take_required(&mut flags, "project")?,
            name: take_required(&mut flags, "name")?,
            base: flags.remove("base"),
            branch: flags.remove("branch"),
        },
        ["sessions", "shell"] => {
            let project = flags.remove("project");
            let worktree = flags.remove("worktree");
            if project.is_some() != worktree.is_some() {
                return Err("Shell requires both --project and --worktree, or neither for a standalone terminal".into());
            }
            ControlCommand::StartShell { project, worktree }
        }
        ["sessions", "list"] => ControlCommand::ListSessions,
        ["sessions", "show", id] => ControlCommand::ShowSession { id: (*id).into() },
        ["sessions", "logs", id] => {
            let lines = flags.remove("lines").map_or(Ok(100), |v| {
                v.parse::<usize>()
                    .map_err(|_| "--lines must be an integer".to_owned())
            })?;
            if lines == 0 || lines > 10_000 {
                return Err("--lines must be between 1 and 10000".into());
            }
            ControlCommand::Logs {
                id: (*id).into(),
                lines,
            }
        }
        ["sessions", "focus", id] => ControlCommand::Focus { id: (*id).into() },
        ["sessions", "stop", id] => ControlCommand::Stop { id: (*id).into() },
        ["sessions", "start"] => {
            let agent = match take_required(&mut flags, "agent")?.as_str() {
                "claude" => Agent::Claude,
                "codex" => Agent::Codex,
                "opencode" => Agent::OpenCode,
                "terminal" => Agent::Terminal,
                _ => return Err("--agent must be claude, codex, opencode or terminal".into()),
            };
            let backend = flags
                .remove("backend")
                .map(|v| match v.as_str() {
                    "native" => Ok(false),
                    "tmux" => Ok(true),
                    _ => Err("--backend must be native or tmux".to_owned()),
                })
                .transpose()?;
            let prompt = flags
                .remove("prompt-file")
                .map(|p| read_file(&p))
                .transpose()?;
            let title = flags.remove("task-title");
            let task_file = flags.remove("task-file");
            let parent = flags.remove("parent-task");
            let task = match (title, task_file) {
                (Some(title), Some(file)) if !title.trim().is_empty() => Some(TaskSpec { title, instructions: read_file(&file)?, parent }),
                (None, None) if parent.is_none() => None,
                _ => return Err("Tasks require both --task-title and --task-file; --parent-task requires a task".into()),
            };
            ControlCommand::StartSession {
                project: take_required(&mut flags, "project")?,
                worktree: take_required(&mut flags, "worktree")?,
                agent,
                prompt,
                backend,
                task,
                roots: std::mem::take(&mut roots),
            }
        }
        ["tasks", "list"] => ControlCommand::ListTasks,
        ["tasks", "show", id] => ControlCommand::ShowTask { id: (*id).into() },
        ["tasks", "wait", id] => {
            let timeout = flags.remove("timeout").map_or(Ok(300), |v| {
                v.parse::<u64>()
                    .map_err(|_| "--timeout must be an integer".to_owned())
            })?;
            if timeout > 86400 {
                return Err("--timeout must not exceed 86400 seconds".into());
            }
            wait = Some(timeout);
            ControlCommand::ShowTask { id: (*id).into() }
        }
        ["tasks", "complete", id] => {
            let file = take_required(&mut flags, "result-file")?;
            let result: TaskResult = serde_json::from_str(&read_file(&file)?)
                .map_err(|e| format!("Invalid task result: {e}"))?;
            result.validate()?;
            ControlCommand::CompleteTask {
                id: (*id).into(),
                result,
            }
        }
        _ => return Err("Unknown command or unexpected arguments; use grove --help".into()),
    };
    if !roots.is_empty() {
        return Err("--root is only valid with sessions start".into());
    }
    if let Some(flag) = flags.keys().next() {
        return Err(format!("Unexpected --{flag}"));
    }
    Ok(Parsed {
        command,
        request_id,
        wait,
    })
}
fn print_response(response: &ControlResponse) {
    print_request_response(response, None);
}
fn print_request_response(response: &ControlResponse, request_id: Option<&str>) {
    let mut value = match serde_json::to_value(response) {
        Ok(value) => value,
        Err(_) => {
            json!({"version": VERSION, "ok": false, "data": null, "error": {"code": "serialization_error", "message": "Cannot encode response"}})
        }
    };
    if let Some(object) = value.as_object_mut() {
        object.insert("request_id".into(), json!(request_id));
    }
    match serde_json::to_string(&value) {
        Ok(text) => println!("{text}"),
        Err(_) => println!("{{\"version\":1,\"ok\":false,\"data\":null,\"error\":{{\"code\":\"serialization_error\",\"message\":\"Cannot encode response\"}}}}"),
    }
}
fn terminal_task_status(status: Option<&str>) -> bool {
    matches!(
        status,
        Some("completed" | "failed" | "cancelled" | "interrupted")
    )
}
pub fn run(args: Vec<String>) -> i32 {
    let args: Vec<String> = args.into_iter().filter(|arg| arg != "--json").collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_response(&ControlResponse::success(json!({"help": HELP})));
        return 0;
    }
    if args.as_slice() == ["--version"] {
        print_response(&ControlResponse::success(
            json!({"version": env!("CARGO_PKG_VERSION"), "protocol_version": VERSION}),
        ));
        return 0;
    }
    let parsed = match parse(args) {
        Ok(parsed) => parsed,
        Err(e) => {
            print_response(&ControlResponse::error("invalid_arguments", e));
            return 2;
        }
    };
    let request = ControlRequest {
        version: VERSION,
        request_id: parsed.request_id,
        command: parsed.command,
    };
    let started = Instant::now();
    loop {
        let response = match control::send(&request) {
            Ok(response) => response,
            Err(e) => {
                let uncertain = e.starts_with("Request outcome uncertain:");
                let message = if uncertain {
                    format!("{e}; retry with --request-id {}", request.request_id)
                } else {
                    e
                };
                print_request_response(
                    &ControlResponse::error(
                        if uncertain {
                            "outcome_unknown"
                        } else {
                            "connection_error"
                        },
                        message,
                    ),
                    Some(&request.request_id),
                );
                return 1;
            }
        };
        if !response.ok {
            print_request_response(&response, Some(&request.request_id));
            return 1;
        }
        if let Some(timeout) = parsed.wait {
            let status = response
                .data
                .get("status")
                .and_then(serde_json::Value::as_str);
            if terminal_task_status(status) {
                print_request_response(&response, Some(&request.request_id));
                return i32::from(status != Some("completed"));
            }
            if started.elapsed() >= Duration::from_secs(timeout) {
                print_request_response(
                    &ControlResponse::error(
                        "timeout",
                        "Task did not finish before the wait timeout",
                    ),
                    Some(&request.request_id),
                );
                return 3;
            }
            std::thread::sleep(Duration::from_millis(250));
        } else {
            print_request_response(&response, Some(&request.request_id));
            return 0;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }
    #[test]
    fn parses_lifecycle_roots_and_rejects_unsafe_ambiguity() {
        assert!(matches!(
            parse(args(
                "projects remove --project demo --confirm demo --remove-worktrees true"
            ))
            .unwrap()
            .command,
            ControlCommand::RemoveProject {
                remove_worktrees: true,
                ..
            }
        ));
        assert!(parse(args("projects remove --project demo")).is_err());
        assert!(parse(args(
            "projects remove --project demo --confirm demo --remove-worktrees yes"
        ))
        .is_err());
        assert!(parse(args("projects list --root a")).is_err());
        assert!(parse(args("sessions shell --project a")).is_err());
        match parse(args(
            "sessions start --project a --worktree b --agent codex --root c --root d",
        ))
        .unwrap()
        .command
        {
            ControlCommand::StartSession { roots, .. } => assert_eq!(roots, vec!["c", "d"]),
            _ => panic!("wrong command"),
        }
        assert!(matches!(
            parse(args(
                "worktrees create --project a --name b --branch existing"
            ))
            .unwrap()
            .command,
            ControlCommand::CreateWorktree {
                branch: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn validates_unknown_duplicate_and_backend_flags() {
        assert!(parse(args("sessions list --typo yes")).is_err());
        assert!(parse(args("worktrees list --project x --project y")).is_err());
        assert!(parse(args(
            "sessions start --project x --worktree y --agent codex --backend other"
        ))
        .is_err());
        assert!(parse(args("sessions logs 1 --lines 10001")).is_err());
    }
    #[test]
    fn parses_start_and_retry_id_without_shell_interpretation() {
        let parsed = parse(
            vec![
                "sessions",
                "start",
                "--project",
                "$(touch owned)",
                "--worktree",
                "a path",
                "--agent",
                "codex",
                "--backend",
                "native",
                "--json",
                "--request-id",
                "retry",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        )
        .unwrap();
        assert_eq!(parsed.request_id, "retry");
        match parsed.command {
            ControlCommand::StartSession {
                project, backend, ..
            } => {
                assert_eq!(project, "$(touch owned)");
                assert_eq!(backend, Some(false));
            }
            _ => panic!("wrong command"),
        }
    }
    #[test]
    fn rejects_incomplete_task_and_parses_wait() {
        assert!(parse(args(
            "sessions start --project x --worktree y --agent codex --parent-task p"
        ))
        .is_err());
        assert_eq!(
            parse(args("tasks wait task --timeout 0")).unwrap().wait,
            Some(0)
        );
    }
    #[test]
    fn interrupted_task_ends_wait() {
        for status in ["completed", "failed", "cancelled", "interrupted"] {
            assert!(terminal_task_status(Some(status)));
        }
        assert!(!terminal_task_status(Some("running")));
        assert!(!terminal_task_status(None));
    }
}
