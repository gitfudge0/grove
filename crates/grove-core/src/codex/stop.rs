//! Codex stop requests, descendant ownership and completion tracking.

use super::*;

impl Actor {
    pub(super) fn interrupt(&mut self, turn: String, attempt: u8) -> Result<(), String> {
        self.request(
            "turn/interrupt",
            json!({"threadId":self.state.saved.thread_id,"turnId":turn}),
            Pending::Interrupt { attempt, turn },
        )
    }

    pub(super) fn observe_stop_work(&mut self, m: &Value) -> Result<(), String> {
        let p = &m["params"];
        let Some(root_thread) = self.state.saved.thread_id.clone() else {
            return Ok(());
        };
        let method = m["method"].as_str().unwrap_or_default();
        let thread = p["threadId"].as_str().unwrap_or_default();
        if method == "turn/started" && thread == root_thread {
            if let Some(turn) = p["turn"]["id"]
                .as_str()
                .filter(|turn| !self.state.settled_turns.iter().any(|old| old == turn))
            {
                if self.stop_tracking.root != turn && self.stop_tracking.stopping {
                    self.stop_error("A new Codex turn started while Stop was still checking work. Check running work manually.".into());
                } else if self.stop_tracking.root != turn
                    && self
                        .state
                        .turn_id
                        .as_deref()
                        .is_none_or(|active| active == turn)
                {
                    self.stop_tracking = StopTracking {
                        root: turn.into(),
                        ..Default::default()
                    };
                }
            }
        }
        if self.stop_tracking.root.is_empty() {
            if let Some(turn) = self.state.turn_id.as_deref() {
                self.stop_tracking.root = turn.into();
            }
        }
        if method == "turn/started"
            && thread != root_thread
            && p["turn"]["rootTurnId"].as_str() == Some(self.stop_tracking.root.as_str())
        {
            if let Some(turn) = p["turn"]["id"].as_str() {
                self.stop_tracking
                    .candidate_turns
                    .insert(thread.into(), turn.into());
            }
        }
        // Notifications can arrive before the spawn event establishes ancestry.
        // Keep the candidate, but never act until the provider's parent chain is known.
        let candidates: Vec<_> = self
            .stop_tracking
            .candidate_turns
            .iter()
            .map(|(t, u)| (t.clone(), u.clone()))
            .collect();
        for (candidate, turn) in candidates {
            if self.interaction_children.known(&candidate) {
                self.interaction_children.start(&candidate, Some(&turn));
                if self
                    .interaction_children
                    .owner(&candidate, Some(&turn))
                    .is_some()
                {
                    self.stop_tracking
                        .turns
                        .insert(candidate.clone(), turn.clone());
                    self.stop_tracking
                        .owned_turns
                        .insert((candidate.clone(), turn));
                }
                self.stop_tracking.candidate_turns.remove(&candidate);
            } else if self.stop_tracking.stopping {
                self.stop_error("Stop could not establish ownership of every Codex child turn. Check running work manually.".into());
            }
        }
        if method == "turn/started"
            && thread != root_thread
            && self.interaction_children.known(thread)
            && self.stop_tracking.stopping
            && p["turn"]["rootTurnId"].as_str().is_none()
        {
            self.stop_error(
                "Stop could not identify a Codex child turn's parent. Check running work manually."
                    .into(),
            );
        }
        if method == "turn/completed"
            && thread == root_thread
            && p["turn"]["id"].as_str() == Some(self.stop_tracking.root.as_str())
        {
            self.stop_tracking.root_pending = false;
        }
        if method == "turn/completed" && thread != root_thread {
            self.stop_tracking.candidate_turns.remove(thread);
            if let Some(turn) = p["turn"]["id"].as_str() {
                self.stop_tracking
                    .pending_turns
                    .remove(&(thread.into(), turn.into()));
                if self
                    .stop_tracking
                    .turns
                    .get(thread)
                    .is_some_and(|t| t == turn)
                {
                    self.stop_tracking.turns.remove(thread);
                }
            }
        }
        if matches!(method, "item/started" | "item/completed")
            && p["item"]["type"] == "commandExecution"
        {
            let turn = p["turnId"].as_str().unwrap_or_default();
            let owned = (thread == root_thread && turn == self.stop_tracking.root)
                || self
                    .stop_tracking
                    .owned_turns
                    .contains(&(thread.into(), turn.into()));
            if owned && p["item"]["source"] == "unifiedExecStartup" {
                if let Some(process) = p["item"]["processId"].as_str().filter(|v| !v.is_empty()) {
                    if let Some(item) = p["item"]["id"].as_str() {
                        let key = (thread.into(), item.into());
                        self.stop_tracking.missing_process.remove(&key);
                        if p["item"]["status"] == "inProgress" {
                            self.stop_tracking.running_processes.insert(key.clone());
                        }
                        if method == "item/completed" && p["item"]["exitCode"].as_i64().is_some() {
                            self.stop_tracking.running_processes.remove(&key);
                        }
                        if self
                            .stop_tracking
                            .processes
                            .get(&key)
                            .is_none_or(|old| old != process)
                        {
                            self.stop_tracking.processes.insert(key, process.into());
                            *self
                                .stop_tracking
                                .process_versions
                                .entry(thread.into())
                                .or_default() += 1;
                        }
                    }
                } else if let Some(item) = p["item"]["id"].as_str() {
                    let key = (thread.into(), item.into());
                    if method == "item/completed" && p["item"]["exitCode"].as_i64().is_some() {
                        self.stop_tracking.missing_process.remove(&key);
                        self.stop_tracking.running_processes.remove(&key);
                    } else if !self.stop_tracking.processes.contains_key(&key) {
                        self.stop_tracking.missing_process.insert(key);
                        if self.stop_tracking.stopping {
                            self.stop_error("Stop could not identify a Codex background terminal. Check running work manually.".into());
                        }
                    }
                } else if self.stop_tracking.stopping {
                    self.stop_error("Stop could not identify a Codex background terminal. Check running work manually.".into());
                }
            }
        }
        if self.stop_tracking.stopping {
            if matches!(
                method,
                "thread/started"
                    | "turn/started"
                    | "turn/completed"
                    | "item/started"
                    | "item/completed"
            ) {
                self.stop_tracking.settled_at = None;
            }
            self.stop_descendants()?;
        }
        Ok(())
    }

    pub(super) fn stop_descendants(&mut self) -> Result<(), String> {
        let root = self.stop_tracking.root.clone();
        let turns: Vec<_> = self
            .stop_tracking
            .turns
            .iter()
            .map(|(a, b)| (a.clone(), b.clone()))
            .collect();
        for (thread, turn) in turns {
            if self
                .stop_tracking
                .sent_turns
                .insert((thread.clone(), turn.clone()))
            {
                self.stop_tracking
                    .pending_turns
                    .insert((thread.clone(), turn.clone()));
                self.request(
                    "turn/interrupt",
                    json!({"threadId":thread,"turnId":turn}),
                    Pending::StopChild {
                        root: root.clone(),
                        thread,
                        turn,
                    },
                )?;
            }
        }
        let running: Vec<_> = self
            .stop_tracking
            .running_processes
            .iter()
            .filter_map(|(thread, item)| {
                self.stop_tracking
                    .processes
                    .get(&(thread.clone(), item.clone()))
                    .map(|process| (thread.clone(), process.clone()))
            })
            .collect();
        for (thread, process) in running {
            if self
                .stop_tracking
                .sent_processes
                .insert((thread.clone(), process.clone()))
            {
                self.request(
                    "thread/backgroundTerminals/terminate",
                    json!({"threadId":thread,"processId":process}),
                    Pending::StopProcess {
                        root: root.clone(),
                        thread,
                        process,
                    },
                )?;
            }
        }
        let threads: HashSet<_> = self
            .stop_tracking
            .processes
            .keys()
            .map(|(thread, _)| thread.clone())
            .collect();
        for thread in threads {
            let version = self
                .stop_tracking
                .process_versions
                .get(&thread)
                .copied()
                .unwrap_or(0);
            if self.stop_tracking.listed.get(&thread).copied() == Some(version)
                || self.pending.values().any(|p| matches!(&p.kind, Pending::StopList { root: r, thread: t, .. } if r == &root && t == &thread)) { continue; }
            self.request(
                "thread/backgroundTerminals/list",
                json!({"threadId":thread}),
                Pending::StopList {
                    root: root.clone(),
                    thread,
                    version,
                },
            )?;
        }
        Ok(())
    }

    pub(super) fn expire_stop_requests(&mut self, now: Instant) {
        let expired: Vec<_> = self
            .pending
            .iter()
            .filter_map(|(id, request)| {
                if now <= request.deadline {
                    return None;
                }
                match &request.kind {
                    Pending::StopChild { root, .. }
                    | Pending::StopProcess { root, .. }
                    | Pending::StopList { root, .. } => Some((*id, root.clone())),
                    _ => None,
                }
            })
            .collect();
        let current = expired
            .iter()
            .any(|(_, root)| root == &self.stop_tracking.root);
        for (id, _) in expired {
            self.pending.remove(&id);
        }
        if current {
            self.stop_error(
                "Stop could not confirm all Codex work ended. Check running work manually.".into(),
            );
            self.dirty = true;
        }
    }

    pub(super) fn advance_stop(&mut self, now: Instant) {
        if !self.stop_tracking.stopping {
            return;
        }
        let root = &self.stop_tracking.root;
        let pending = self.stop_tracking.root_pending || !self.stop_tracking.pending_turns.is_empty()
            || self.pending.values().any(|r| matches!(&r.kind, Pending::StopChild { root: r, .. } | Pending::StopProcess { root: r, .. } | Pending::StopList { root: r, .. } if r == root));
        if self
            .stop_tracking
            .deadline
            .is_some_and(|deadline| now >= deadline)
        {
            if pending {
                self.stop_error(
                    "Stop could not confirm all Codex work ended. Check running work manually."
                        .into(),
                );
            }
            self.stop_tracking.stopping = false;
            self.dirty = true;
        } else if pending {
            self.stop_tracking.settled_at = None;
        } else {
            let since = *self.stop_tracking.settled_at.get_or_insert(now);
            if now.duration_since(since) >= Duration::from_millis(500) {
                self.stop_tracking.stopping = false;
                self.dirty = true;
            }
        }
    }

    pub(super) fn stop_error(&mut self, message: String) {
        self.stop_tracking.error = Some(message.clone());
        self.state.error = Some(message);
    }
}
