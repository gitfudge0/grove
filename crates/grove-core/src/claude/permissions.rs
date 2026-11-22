//! Claude permission-mode discovery, cache, and launch capability checks.

use super::{launch_options, SavedSession};
use std::{
    collections::HashMap,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Condvar, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

pub(super) fn permission_modes_from_help(help: &str) -> Vec<String> {
    let Some(section) = help.split("--permission-mode <mode>").nth(1) else {
        return Vec::new();
    };
    let Some(choices) = section
        .split("(choices:")
        .nth(1)
        .and_then(|v| v.split(')').next())
    else {
        return Vec::new();
    };
    choices
        .split(',')
        .filter_map(|value| {
            let mode = value.trim().trim_matches('"');
            [
                "default",
                "manual",
                "acceptEdits",
                "plan",
                "dontAsk",
                "bypassPermissions",
                "auto",
            ]
            .contains(&mode)
            .then(|| mode.to_owned())
        })
        .collect()
}
pub(super) fn discover_permission_modes(
    mut command: Command,
    stopping: &AtomicBool,
    timeout: Duration,
) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut output) = tempfile::tempfile() else {
        return Vec::new();
    };
    let Ok(writer) = output.try_clone() else {
        return Vec::new();
    };
    command
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let Ok(mut child) = command.spawn() else {
        return Vec::new();
    };
    let started = Instant::now();
    let success = loop {
        if stopping.load(Ordering::SeqCst)
            || started.elapsed() >= timeout
            || output.metadata().map_or(true, |v| v.len() > 128 * 1024)
        {
            break false;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => break false,
        }
    };
    // Reap the owned helper and its group even if a CLI shim left descendants.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    if !success || output.seek(SeekFrom::Start(0)).is_err() {
        return Vec::new();
    }
    let mut help = String::new();
    if output.take(128 * 1024).read_to_string(&mut help).is_err() {
        return Vec::new();
    }
    permission_modes_from_help(&help)
}
const PERMISSION_HELP_MAX_AGE: Duration = Duration::from_hours(24);
pub(super) const PERMISSION_HELP_RETRY: Duration = Duration::from_secs(30);

pub(super) enum PermissionProbe {
    Loading,
    Ready(Vec<String>, Instant),
}
#[derive(Default)]
pub(super) struct PermissionProbeCache {
    pub(super) entries: Mutex<HashMap<String, PermissionProbe>>,
    changed: Condvar,
}
impl PermissionProbeCache {
    pub(super) fn get(
        &self,
        key: String,
        stopping: &AtomicBool,
        load: impl FnOnce() -> Vec<String>,
    ) -> Vec<String> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if stopping.load(Ordering::SeqCst) {
                return Vec::new();
            }
            match entries.get(&key) {
                Some(PermissionProbe::Ready(modes, at))
                    if at.elapsed()
                        < if modes.is_empty() {
                            PERMISSION_HELP_RETRY
                        } else {
                            PERMISSION_HELP_MAX_AGE
                        } =>
                {
                    return modes.clone()
                }
                Some(PermissionProbe::Loading) => {
                    entries = self
                        .changed
                        .wait_timeout(entries, Duration::from_millis(20))
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0;
                }
                _ => break,
            }
        }
        // Bound completed identities without disturbing in-flight owners.
        if entries.len() >= 16 {
            entries.retain(|_, value| matches!(value, PermissionProbe::Loading));
        }
        entries.insert(key.clone(), PermissionProbe::Loading);
        drop(entries);
        let modes = load();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if stopping.load(Ordering::SeqCst) {
            entries.remove(&key);
        } else {
            entries.insert(key, PermissionProbe::Ready(modes.clone(), Instant::now()));
        }
        self.changed.notify_all();
        modes
    }
}

fn permission_probe_key(
    program: &str,
    args: &[String],
    environment: &[(String, String)],
) -> String {
    use std::hash::{Hash, Hasher};
    let mut key = std::collections::hash_map::DefaultHasher::new();
    program.hash(&mut key);
    args.hash(&mut key);
    let mut environment = environment.to_vec();
    environment.sort();
    environment.hash(&mut key);
    // session_invocation can be a shell wrapper; also fingerprint the actual CLI.
    let (binary, prefix) = crate::agent::Agent::Claude.invocation();
    for path in std::iter::once(binary).chain(prefix) {
        path.hash(&mut key);
        if let Ok(metadata) = fs_err::metadata(path) {
            metadata.len().hash(&mut key);
            metadata.modified().ok().hash(&mut key);
        }
    }
    format!("{:016x}", key.finish())
}

pub(super) fn available_permission_modes(stopping: &AtomicBool) -> Vec<String> {
    static CACHE: OnceLock<PermissionProbeCache> = OnceLock::new();
    let (program, args) = crate::agent::Agent::Claude.session_invocation(&["--help".into()]);
    let environment = crate::env_path::session_environment()
        .map_or_else(|| std::env::vars().collect(), |vars| vars.to_vec());
    let key = permission_probe_key(&program, &args, &environment);
    CACHE
        .get_or_init(PermissionProbeCache::default)
        .get(key, stopping, || {
            let mut command = Command::new(program);
            command.args(args).env_clear().envs(environment);
            discover_permission_modes(command, stopping, Duration::from_secs(2))
        })
}

pub(super) fn permission_mode(access: &str) -> Result<&str, String> {
    match access {
        "read-only" | "plan" => Ok("plan"),
        "workspace-write" | "acceptEdits" => Ok("acceptEdits"),
        "full-access" | "danger-full-access" | "bypassPermissions" => Ok("bypassPermissions"),
        "default" | "manual" | "dontAsk" | "auto" => Ok(access),
        _ => Err(format!("Unsupported Claude Code permission mode: {access}")),
    }
}

/// Bypass is a process launch capability, not a permission the live picker can grant.
pub fn can_select_permission_mode(saved: &SavedSession, mode: &str) -> bool {
    if mode != "bypassPermissions" {
        return true;
    }
    let Ok((options, _)) = launch_options(&saved.config_args) else {
        return false;
    };
    let mut index = 0;
    while let Some(option) = options.get(index) {
        match option.as_str() {
            "--dangerously-skip-permissions" | "--allow-dangerously-skip-permissions" => {
                return true;
            }
            "--strict-mcp-config" | "--disable-slash-commands" | "--chrome" | "--no-chrome" => {
                index += 1;
            }
            _ => index += 2, // The remaining accepted launch options take a value.
        }
    }
    false
}
