//! Native Claude Code agent-status polling via `claude agents --json` — a strictly better
//! attention signal than `attention`'s hook state files or `gui::activity`'s screen-scraping
//! heuristics when available, and the only one that works for tmux-backed sessions reattached
//! across a grove restart. Polls at 1s only while requested by live sessions, and
//! separately shells out to `ps` to match a Grove session to a row by process ancestry rather
//! than cwd, since multiple sessions can share a cwd. Failures are silent: an unsupported
//! `claude` install just means permanent "no signal" after a few failed attempts, never a crash.
//! On Windows `status_for` always returns `None`; existing fallbacks are unaffected.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[cfg(not(windows))]
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// A snapshot older than this is "no live signal" — better to fall back than trust a stale picture.
const STALE_AFTER: Duration = Duration::from_secs(5);
#[cfg(not(windows))]
const MAX_CONSECUTIVE_FAILURES: u32 = 3;
/// Backstop alongside the visited-set cycle guard; no real process tree is this deep.
const MAX_ANCESTOR_HOPS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeStatus {
    Busy,
    Idle,
    Waiting,
}

/// Non-interactive ("background") rows and rows missing a numeric pid are dropped at parse time.
#[derive(Clone, Debug, PartialEq)]
struct AgentRow {
    pid: u32,
    cwd: String,
    status: NativeStatus,
}

struct Snapshot {
    rows: Vec<AgentRow>,
    pid_parent: HashMap<u32, u32>,
    taken_at: Instant,
}

pub struct Poller {
    snapshot: Arc<Mutex<Option<Snapshot>>>,
    wanted: Arc<AtomicBool>,
    unsupported: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    control: Arc<(Mutex<()>, Condvar)>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Poller {
    /// The shared worker parks without a recurring timer while nobody needs
    /// native status. Changes and entity disposal wake its condition variable.
    pub fn new() -> Poller {
        let mut poller = Self::inert();
        poller.start();
        poller
    }

    fn inert() -> Self {
        Self {
            snapshot: Arc::new(Mutex::new(None)),
            wanted: Arc::new(AtomicBool::new(false)),
            unsupported: Arc::new(AtomicBool::new(cfg!(windows))),
            stopping: Arc::new(AtomicBool::new(false)),
            control: Arc::new((Mutex::new(()), Condvar::new())),
            worker: None,
        }
    }

    fn start(&mut self) {
        #[cfg(not(windows))]
        {
            self.worker = Some(self.start_worker(poll_once));
        }
    }

    #[cfg(not(windows))]
    fn start_worker(
        &self,
        mut operation: impl FnMut() -> Result<(Vec<AgentRow>, HashMap<u32, u32>), ()> + Send + 'static,
    ) -> std::thread::JoinHandle<()> {
        let (snapshot, wanted, unsupported, stopping, control) = (
            self.snapshot.clone(),
            self.wanted.clone(),
            self.unsupported.clone(),
            self.stopping.clone(),
            self.control.clone(),
        );
        std::thread::spawn(move || {
            let mut failures = 0;
            loop {
                let mut guard = control
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                while !wanted.load(Ordering::Acquire) && !stopping.load(Ordering::Acquire) {
                    guard = control
                        .1
                        .wait(guard)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                if stopping.load(Ordering::Acquire) {
                    return;
                }
                drop(guard);
                match operation() {
                    Ok((rows, pid_parent)) => {
                        failures = 0;
                        *snapshot
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Snapshot {
                            rows,
                            pid_parent,
                            taken_at: Instant::now(),
                        });
                    }
                    Err(()) => {
                        failures += 1;
                        if failures >= MAX_CONSECUTIVE_FAILURES {
                            unsupported.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                }
                let guard = control
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if wanted.load(Ordering::Acquire) && !stopping.load(Ordering::Acquire) {
                    drop(
                        control
                            .1
                            .wait_timeout(guard, POLL_INTERVAL)
                            .unwrap_or_else(std::sync::PoisonError::into_inner),
                    );
                }
            }
        })
    }

    /// `true` iff at least one live Claude terminal session needs native status.
    pub fn set_wanted(&self, wanted: bool) {
        let _guard = self
            .control
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.wanted.swap(wanted, Ordering::AcqRel) != wanted {
            self.control.1.notify_one();
        }
    }

    /// `None` whenever there's no trustworthy live signal; callers must always have a fallback.
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

impl Drop for Poller {
    fn drop(&mut self) {
        let _guard = self
            .control
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.stopping.store(true, Ordering::Release);
        self.control.1.notify_one();
        // Never join a subprocess-owning worker on GPUI's foreground. It exits
        // after the current status operation, or immediately when idle.
        let _worker = self.worker.take();
    }
}

/// `Err` covers spawn failure, non-zero exit, and unparsable stdout alike.
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

    // Treated the same as claude failing, since without ancestry the match degenerates to exact-pid only.
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

#[cfg(any(not(windows), test))]
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
        let Some(pid) = entry.get("pid").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let cwd = entry
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let raw_status = entry.get("status").and_then(|v| v.as_str()).unwrap_or("");
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

/// An unrecognized status drops the row rather than guessing, so upstream changes degrade to fallbacks.
#[cfg(any(not(windows), test))]
fn map_status(raw: &str) -> Option<NativeStatus> {
    match raw {
        "busy" => Some(NativeStatus::Busy),
        "idle" => Some(NativeStatus::Idle),
        "waiting" => Some(NativeStatus::Waiting),
        _ => None,
    }
}

/// Malformed lines are skipped individually rather than failing the whole parse.
#[cfg(any(not(windows), test))]
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

/// Prefers an ancestry match on `root_pid` (unambiguous even with a shared cwd); otherwise falls
/// back to cwd, but only if exactly one row matches — zero or multiple is ambiguous, so `None`.
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

/// Capped with a visited-set cycle guard so a malformed/cyclic `ps` snapshot can't hang this.
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    const FIXTURE_JSON: &str = r#"[
        {"kind": "interactive", "pid": 111, "cwd": "/repo/a", "status": "busy"},
        {"kind": "interactive", "pid": 222, "cwd": "/repo/b", "status": "idle"},
        {"kind": "interactive", "pid": 333, "cwd": "/repo/c", "status": "waiting"},
        {"kind": "background", "pid": 444, "cwd": "/repo/d", "status": "busy"}
    ]"#;

    #[cfg(not(windows))]
    #[test]
    fn idle_poller_waits_for_demand_and_stops_when_dropped() {
        let mut poller = Poller::inert();
        let (called, calls) = std::sync::mpsc::channel();
        poller.worker = Some(poller.start_worker(move || {
            called.send(()).unwrap();
            Ok((Vec::new(), HashMap::new()))
        }));
        assert!(calls.recv_timeout(Duration::from_millis(30)).is_err());
        poller.set_wanted(true);
        calls.recv_timeout(Duration::from_secs(2)).unwrap();
        poller.set_wanted(false);
        assert!(calls.recv_timeout(Duration::from_millis(30)).is_err());
        let worker = poller.worker.take().unwrap();
        drop(poller);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !worker.is_finished() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(
            worker.is_finished(),
            "idle worker failed to exit after its owner dropped"
        );
        worker.join().unwrap();
    }

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
        let mut pid_parent = HashMap::new();
        pid_parent.insert(6u32, 5);
        pid_parent.insert(5u32, 6);
        let rows = vec![AgentRow {
            pid: 6,
            cwd: "/repo/x".to_string(),
            status: NativeStatus::Idle,
        }];
        let result = match_row(&rows, &pid_parent, Some(999), "/repo/x");
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

        let rows = vec![AgentRow {
            pid: 1,
            cwd: "/repo/a".to_string(),
            status: NativeStatus::Busy,
        }];
        assert_eq!(match_row(&rows, &pid_parent, None, "/repo/nowhere"), None);

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
            taken_at: Instant::now().checked_sub(Duration::from_secs(10)).unwrap(),
        };
        assert!(snap.taken_at.elapsed() > STALE_AFTER);
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
