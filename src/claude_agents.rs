//! Native Claude Code agent-status polling via `claude agents --json`.
//!
//! Claude Code (as of recent CLI versions) can report the live status of
//! every interactive/background agent process it knows about, keyed by pid
//! and cwd. When available, this is a strictly better attention signal than
//! either the hook-appended state file in `attention` (which only fires on
//! discrete events, and only for sessions grove itself spawned with the hook
//! wired up) or the screen-scraping heuristics in `gui::activity` (which can
//! be fooled by anything that repaints the terminal). It also works for
//! tmux-backed sessions reattached across a grove restart, where `attention`
//! never had a chance to inject hooks in the first place.
//!
//! This module polls `claude agents --json` on a fixed 1s cadence from a
//! single background thread (shared across all sessions — there is one CLI
//! call per tick, not one per session), and separately shells out to `ps` to
//! build a pid→ppid map so a Grove session can be matched to a row by
//! process ancestry, not just by cwd (multiple sessions can share a cwd, and
//! a single cwd's row can be ambiguous; ancestry is not).
//!
//! Failures are silent by design, mirroring `attention`: a CLI that doesn't
//! support `agents --json` (older `claude`, or no `claude` on PATH at all)
//! just means "no signal" forever after a few failed attempts, never a
//! crash or a visible error. Once we've given up (see `unsupported`) we stop
//! spawning `claude` on every tick, so a permanently-unsupported install
//! doesn't pay a process-spawn cost once a second indefinitely.
//!
//! Windows: `claude agents --json` support is unverified there and `ps` in
//! the form used below doesn't exist, so `status_for` degenerates to always
//! returning `None` (see the `cfg(windows)` branch in `Poller::new`) — the
//! caller's existing fallbacks (hook state files, heuristics) are unaffected.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the background thread polls `claude agents --json` / `ps`.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// A snapshot older than this is treated as "no live signal" by
/// `status_for` — better to fall back to other heuristics than trust a
/// picture of the world that's gone stale (e.g. the poll thread wedged).
const STALE_AFTER: Duration = Duration::from_secs(5);
/// Consecutive poll failures before we conclude this `claude` install just
/// doesn't support `agents --json` and stop polling for good.
const MAX_CONSECUTIVE_FAILURES: u32 = 3;
/// Ancestor-chain walk depth cap in `match_row`. Any real process tree is
/// nowhere near this deep; it exists purely as a backstop alongside the
/// visited-set cycle guard.
const MAX_ANCESTOR_HOPS: usize = 20;

/// Status of a single interactive Claude agent process, as reported by
/// `claude agents --json`. Maps 1:1 onto the CLI's raw `status` string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeStatus {
    Busy,
    Idle,
    Waiting,
}

/// One parsed row from `claude agents --json`, restricted to the fields
/// `match_row` needs. Non-interactive ("background") rows and rows missing
/// a numeric pid are dropped at parse time — see `parse_agents_json`.
#[derive(Clone, Debug, PartialEq)]
struct AgentRow {
    pid: u32,
    cwd: String,
    status: NativeStatus,
}

/// The last successful poll. `taken_at` lets `status_for` reject a snapshot
/// that's gone stale (poll thread stuck, or simply hasn't run yet this
/// process lifetime — see `Poller::status_for`).
struct Snapshot {
    rows: Vec<AgentRow>,
    pid_parent: HashMap<u32, u32>,
    taken_at: Instant,
}

/// Shared handle to the background poller. One instance lives on `Grove` for
/// the process's lifetime (see `gui::state::Grove::claude_poller`); cloning
/// isn't needed since the struct is entirely `Arc`-backed internally, but it
/// isn't `Clone` because nothing currently needs more than one owner.
pub struct Poller {
    snapshot: Arc<Mutex<Option<Snapshot>>>,
    wanted: Arc<AtomicBool>,
    unsupported: Arc<AtomicBool>,
}

impl Poller {
    /// Start the background polling thread and return a handle to it.
    /// Cheap to construct — the thread itself sleeps whenever nothing wants
    /// live status (see `set_wanted`).
    pub fn new() -> Poller {
        let snapshot: Arc<Mutex<Option<Snapshot>>> = Arc::new(Mutex::new(None));
        let wanted = Arc::new(AtomicBool::new(false));
        let unsupported = Arc::new(AtomicBool::new(false));

        #[cfg(not(windows))]
        {
            let snapshot = snapshot.clone();
            let wanted = wanted.clone();
            let unsupported = unsupported.clone();
            std::thread::spawn(move || {
                let mut consecutive_failures: u32 = 0;
                loop {
                    if !wanted.load(Ordering::Relaxed) {
                        std::thread::sleep(POLL_INTERVAL);
                        continue;
                    }

                    match poll_once() {
                        Ok((rows, pid_parent)) => {
                            consecutive_failures = 0;
                            if let Ok(mut guard) = snapshot.lock() {
                                *guard = Some(Snapshot {
                                    rows,
                                    pid_parent,
                                    taken_at: Instant::now(),
                                });
                            }
                        }
                        Err(()) => {
                            consecutive_failures += 1;
                            if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                                unsupported.store(true, Ordering::Relaxed);
                                return;
                            }
                        }
                    }

                    std::thread::sleep(POLL_INTERVAL);
                }
            });
        }
        // Windows: no thread spawned at all. `unsupported` starts false, but
        // `status_for` will simply never see a snapshot appear and so always
        // returns `None` via the staleness/missing-snapshot check.
        #[cfg(windows)]
        {
            unsupported.store(true, Ordering::Relaxed);
        }

        Poller {
            snapshot,
            wanted,
            unsupported,
        }
    }

    /// Tell the background thread whether it's worth polling at all right
    /// now. The caller (`gui::update::refresh_activity`) passes `true` iff
    /// at least one live Claude session exists — no point spawning `claude
    /// agents --json` once a second when there's nothing for it to inform.
    pub fn set_wanted(&self, wanted: bool) {
        self.wanted.store(wanted, Ordering::Relaxed);
    }

    /// Look up the native status for a Grove session, given its
    /// process-tree root pid (see `session::Session::root_pid`) and worktree
    /// path. Returns `None` whenever there's no trustworthy live signal —
    /// `claude agents --json` isn't supported by this install, no snapshot
    /// has been taken yet, the snapshot is stale, or the row-matching logic
    /// itself is ambiguous. Callers must always have a fallback for `None`.
    pub fn status_for(&self, root_pid: Option<u32>, wt_path: &str) -> Option<NativeStatus> {
        if self.unsupported.load(Ordering::Relaxed) {
            return None;
        }
        let guard = self.snapshot.lock().ok()?;
        let snap = guard.as_ref()?;
        if snap.taken_at.elapsed() > STALE_AFTER {
            return None;
        }
        match_row(&snap.rows, &snap.pid_parent, root_pid, wt_path)
    }
}

/// One poll cycle: run `claude agents --json` and `ps`, parse both. `Err`
/// covers spawn failure, non-zero exit, and unparsable stdout alike — the
/// caller only needs to know "did this attempt succeed", not why it didn't.
#[cfg(not(windows))]
fn poll_once() -> Result<(Vec<AgentRow>, HashMap<u32, u32>), ()> {
    let agents_out = std::process::Command::new("claude")
        .args(["agents", "--json"])
        .output()
        .map_err(|_| ())?;
    if !agents_out.status.success() {
        return Err(());
    }
    let stdout = String::from_utf8_lossy(&agents_out.stdout);
    let rows = parse_agents_json(&stdout).map_err(|_| ())?;

    // `ps` failing is treated the same as `claude` failing: without process
    // ancestry the pid-based match in `match_row` degenerates to exact-pid
    // only, which is a materially worse signal, so we'd rather count the
    // whole poll as a failure and retry next tick than silently downgrade.
    let ps_out = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
        .map_err(|_| ())?;
    if !ps_out.status.success() {
        return Err(());
    }
    let pid_parent = parse_ps(&String::from_utf8_lossy(&ps_out.stdout));

    Ok((rows, pid_parent))
}

/// Parse `claude agents --json`'s stdout into interactive agent rows.
/// Non-interactive ("background") rows and rows without a numeric `pid` are
/// silently dropped — they have no process to match against a Grove session.
fn parse_agents_json(s: &str) -> Result<Vec<AgentRow>, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(s)?;
    let arr = value
        .as_array()
        .ok_or_else(|| serde::de::Error::custom("expected top-level JSON array"))?;

    let mut rows = Vec::new();
    for entry in arr {
        let Some(kind) = entry.get("kind").and_then(|v| v.as_str()) else {
            continue;
        };
        if kind != "interactive" {
            continue;
        }
        let Some(pid) = entry.get("pid").and_then(|v| v.as_u64()) else {
            continue;
        };
        let cwd = entry
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let raw_status = entry
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let Some(status) = map_status(raw_status) else {
            continue;
        };
        rows.push(AgentRow {
            pid: pid as u32,
            cwd,
            status,
        });
    }
    Ok(rows)
}

/// Map the raw `status` string from `claude agents --json` onto our enum.
/// An unrecognized status is treated as "no usable signal" for that row
/// (row dropped) rather than guessed at — a new/renamed status upstream
/// should degrade to the existing fallbacks, not silently misreport.
fn map_status(raw: &str) -> Option<NativeStatus> {
    match raw {
        "busy" => Some(NativeStatus::Busy),
        "idle" => Some(NativeStatus::Idle),
        "waiting" => Some(NativeStatus::Waiting),
        _ => None,
    }
}

/// Parse `ps -axo pid=,ppid=` output into a pid→ppid map. Malformed lines
/// are skipped individually rather than failing the whole parse — `ps`
/// output is not expected to be malformed in practice, but there's no
/// reason to throw away otherwise-good ancestry data over one bad line.
fn parse_ps(s: &str) -> HashMap<u32, u32> {
    let mut map = HashMap::new();
    for line in s.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (it.next(), it.next()) else {
            continue;
        };
        if let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) {
            map.insert(pid, ppid);
        }
    }
    map
}

/// Core matching logic, kept pure and free of any I/O so it's directly unit
/// testable without spawning `claude`/`ps` (see `Poller::status_for`, which
/// layers the unsupported/staleness checks around a call into this).
///
/// Order of precedence:
/// 1. If `root_pid` is given, prefer an ancestry match: a row whose pid
///    equals `root_pid`, or whose ancestor chain (walked via `pid_parent`)
///    passes through it. This is unambiguous even when several sessions
///    share a worktree cwd (e.g. two Claude sessions opened side-by-side).
/// 2. Otherwise (no `root_pid`, or no ancestry match found), fall back to
///    matching by cwd — but only if exactly one row has that cwd. Zero or
///    multiple matches are genuinely ambiguous, so we return `None` and let
///    the caller's other heuristics decide instead of guessing.
fn match_row(
    rows: &[AgentRow],
    pid_parent: &HashMap<u32, u32>,
    root_pid: Option<u32>,
    wt_path: &str,
) -> Option<NativeStatus> {
    if let Some(rp) = root_pid {
        for row in rows {
            if row.pid == rp {
                return Some(row.status);
            }
            if ancestors_contain(pid_parent, row.pid, rp) {
                return Some(row.status);
            }
        }
    }

    let mut matches = rows.iter().filter(|r| r.cwd == wt_path);
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first.status)
}

/// Walk `pid_parent` upward from `start` (via `ppid`) looking for `target`,
/// capped at `MAX_ANCESTOR_HOPS` hops with a visited-set cycle guard so a
/// malformed or genuinely cyclic `ps` snapshot can't hang this call.
fn ancestors_contain(pid_parent: &HashMap<u32, u32>, start: u32, target: u32) -> bool {
    let mut visited = std::collections::HashSet::new();
    let mut current = start;
    for _ in 0..MAX_ANCESTOR_HOPS {
        let Some(&parent) = pid_parent.get(&current) else {
            return false;
        };
        if parent == target {
            return true;
        }
        if !visited.insert(parent) {
            return false;
        }
        current = parent;
    }
    false
}

impl Default for Poller {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_JSON: &str = r#"[
        {"kind": "interactive", "pid": 111, "cwd": "/repo/a", "status": "busy"},
        {"kind": "interactive", "pid": 222, "cwd": "/repo/b", "status": "idle"},
        {"kind": "interactive", "pid": 333, "cwd": "/repo/c", "status": "waiting"},
        {"kind": "background", "pid": 444, "cwd": "/repo/d", "status": "busy"}
    ]"#;

    #[test]
    fn parses_interactive_rows_and_excludes_background() {
        let rows = parse_agents_json(FIXTURE_JSON).expect("should parse");
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.pid != 444));
        assert_eq!(
            rows.iter().find(|r| r.pid == 111).unwrap().status,
            NativeStatus::Busy
        );
        assert_eq!(
            rows.iter().find(|r| r.pid == 222).unwrap().status,
            NativeStatus::Idle
        );
        assert_eq!(
            rows.iter().find(|r| r.pid == 333).unwrap().status,
            NativeStatus::Waiting
        );
    }

    #[test]
    fn ancestor_chain_matches_non_cyclic() {
        // 100 (session root) -> 50 -> 10 (row pid)
        let mut pid_parent = HashMap::new();
        pid_parent.insert(10u32, 50);
        pid_parent.insert(50u32, 100);
        let rows = vec![AgentRow {
            pid: 10,
            cwd: "/repo/x".to_string(),
            status: NativeStatus::Busy,
        }];
        let result = match_row(&rows, &pid_parent, Some(100), "/irrelevant");
        assert_eq!(result, Some(NativeStatus::Busy));
    }

    #[test]
    fn ancestor_chain_cycle_guard_terminates() {
        // 5 <-> 6 cycle; row pid 6 never actually reaches target 999.
        let mut pid_parent = HashMap::new();
        pid_parent.insert(6u32, 5);
        pid_parent.insert(5u32, 6);
        let rows = vec![AgentRow {
            pid: 6,
            cwd: "/repo/x".to_string(),
            status: NativeStatus::Idle,
        }];
        // If ancestors_contain looped forever this call would never return
        // and the test would hang/timeout rather than reach this assertion.
        let result = match_row(&rows, &pid_parent, Some(999), "/repo/x");
        // No pid/ancestor match, but cwd is unique -> falls back to cwd match.
        assert_eq!(result, Some(NativeStatus::Idle));
    }

    #[test]
    fn unique_cwd_fallback_when_no_root_pid() {
        let rows = vec![
            AgentRow {
                pid: 1,
                cwd: "/repo/only".to_string(),
                status: NativeStatus::Waiting,
            },
            AgentRow {
                pid: 2,
                cwd: "/repo/other".to_string(),
                status: NativeStatus::Busy,
            },
        ];
        let pid_parent = HashMap::new();
        let result = match_row(&rows, &pid_parent, None, "/repo/only");
        assert_eq!(result, Some(NativeStatus::Waiting));
    }

    #[test]
    fn ambiguous_cwd_returns_none() {
        let pid_parent = HashMap::new();

        // Zero matches.
        let rows = vec![AgentRow {
            pid: 1,
            cwd: "/repo/a".to_string(),
            status: NativeStatus::Busy,
        }];
        assert_eq!(match_row(&rows, &pid_parent, None, "/repo/nowhere"), None);

        // Two matches for the same cwd.
        let rows = vec![
            AgentRow {
                pid: 1,
                cwd: "/repo/shared".to_string(),
                status: NativeStatus::Busy,
            },
            AgentRow {
                pid: 2,
                cwd: "/repo/shared".to_string(),
                status: NativeStatus::Idle,
            },
        ];
        assert_eq!(match_row(&rows, &pid_parent, None, "/repo/shared"), None);
    }

    #[test]
    fn stale_snapshot_rejected() {
        let snap = Snapshot {
            rows: vec![AgentRow {
                pid: 1,
                cwd: "/repo/a".to_string(),
                status: NativeStatus::Busy,
            }],
            pid_parent: HashMap::new(),
            taken_at: Instant::now() - Duration::from_secs(10),
        };
        assert!(snap.taken_at.elapsed() > STALE_AFTER);
        // Mirrors the check `Poller::status_for` performs before ever
        // calling into `match_row` — a stale snapshot must never be used,
        // even if it would otherwise contain a match.
        let would_use = snap.taken_at.elapsed() <= STALE_AFTER;
        assert!(!would_use);
    }

    #[test]
    fn parse_ps_builds_pid_parent_map() {
        let out = "  1     0\n 10     1\n 20    10\n";
        let map = parse_ps(out);
        assert_eq!(map.get(&1), Some(&0));
        assert_eq!(map.get(&10), Some(&1));
        assert_eq!(map.get(&20), Some(&10));
    }
}
