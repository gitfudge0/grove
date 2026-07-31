//! Many [`TerminalSession`]s under one owner, keyed by a stable [`SessionId`].
//!
//! The iced build keeps sessions in a `Vec<Session>` and refers to them by
//! index, which forces an index-shifting dance on every removal
//! (`src/gui/update/sessions.rs:270-280`, `:109-113`). Here the key is a
//! monotonic id that is never reused, so removing a session moves nothing.
//!
//! **Home terminals live outside the map**, mirroring `App::home_terminals`
//! (`src/app/mod.rs:85-101`, whose comment is the contract): they must never
//! appear as tree rows, activity rows, or in the session-cycling / kill
//! machinery. Their row order *is* their identity, so they keep a positional
//! index (`src/app/terminals.rs:61-84`).
//!
//! **Deviation from the plan's `IndexMap`:** the plan's "Tech stack additions:
//! none" clause outranks its `IndexMap` sketch, and `indexmap` is not a
//! workspace dependency. Insertion order is kept by a `Vec<SessionMeta>`
//! (`order`) beside a `HashMap` of the live entities, which also lets every
//! pure bookkeeping rule below be unit-tested without spawning a PTY.

// The registry's full surface lands in one go so Tasks 4-7 are mechanical.
#![allow(dead_code)]

use std::collections::HashMap;
use std::time::Instant;

use gpui::Entity;
use grove_core::agent::Agent;

use crate::entities::terminal_session::TerminalSession;

/// Opaque, stable, monotonic session key. Never reused within a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(u64);

impl SessionId {
    /// Test/fixture constructor. Production ids only come from
    /// [`SessionRegistry::next_id`].
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// What the sidebar needs to know about a session without touching its PTY.
/// Constraint 3 candidate 3: if the sidebar wants something grove-core's
/// `Agent` does not provide, it goes **here**, not into grove-core.
#[derive(Clone, Debug)]
pub struct SessionMeta {
    pub id: SessionId,
    /// `Project::name` — the tree groups by name, not by index, because
    /// indices move when a project is added or archived.
    pub project: String,
    /// Absolute worktree path: the session's stable placement in the tree.
    pub wt_path: String,
    pub agent: Agent,
    /// Internal label (`claude 1`, `terminal 2`, …). Not displayed; stripped
    /// out of the OSC title to make the context text (`src/gui/rows.rs:778`).
    pub label: String,
    pub spawned_at: Instant,
}

/// Where a new session opens. Replaces [`TerminalSession::spawn`]'s hardcoded
/// single target.
#[derive(Clone, Debug)]
pub struct SpawnTarget {
    pub cwd: String,
    pub agent: Agent,
    /// Project name stamped into the tmux sidecar metadata; empty for home
    /// terminals, which belong to no project.
    pub project: String,
    pub label: String,
}

impl SpawnTarget {
    /// The home-terminal target: a native shell at `~`
    /// (`src/app/terminals.rs:7-10,86-95`).
    #[must_use]
    pub fn home(label: String) -> Self {
        Self {
            cwd: home_dir(),
            agent: Agent::Terminal,
            project: String::new(),
            label,
        }
    }
}

/// Absolute path of the home directory, falling back to `/`
/// (`src/app/terminals.rs:7-10`). Read from `$HOME` rather than through the
/// `dirs` crate: "Tech stack additions: none" — grove-gpui does not depend on
/// `dirs`, and on the platforms grove ships `$HOME` is what `dirs` reads too.
#[must_use]
pub fn home_dir() -> String {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "/".to_string())
}

#[derive(Default)]
pub struct SessionRegistry {
    /// Insertion order — the id space and the tree's within-worktree order.
    order: Vec<SessionMeta>,
    terms: HashMap<SessionId, Entity<TerminalSession>>,
    /// Pinned TERMINALS section. Positional identity, parallel vectors.
    home: Vec<SessionMeta>,
    home_terms: Vec<Entity<TerminalSession>>,
    next_id: u64,
    /// Monotonic counter behind each terminal's internal label
    /// (`src/app/mod.rs:96-101`).
    home_terminal_seq: usize,
}

impl SessionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint the next id. Monotonic and never reused, including across
    /// removals — that is what makes [`SessionId`] a safe map key.
    fn next_id(&mut self) -> SessionId {
        self.next_id += 1;
        SessionId(self.next_id)
    }

    // ── bookkeeping (pure — no PTY, no gpui App) ────────────────────────

    /// Record a session's metadata and hand back its id. The live entity is
    /// attached separately by [`Self::attach`], so the ordering rules stay
    /// testable without spawning anything.
    pub fn insert_meta(&mut self, project: String, wt_path: String, agent: Agent) -> SessionId {
        let id = self.next_id();
        let label = self.next_agent_label(agent);
        self.order.push(SessionMeta {
            id,
            project,
            wt_path,
            agent,
            label,
            spawned_at: Instant::now(),
        });
        id
    }

    /// `agent N`, N counting that agent kind's sessions ever recorded — the
    /// same shape as the home terminals' `terminal N`.
    fn next_agent_label(&self, agent: Agent) -> String {
        let n = self.order.iter().filter(|m| m.agent == agent).count() + 1;
        format!("{} {n}", agent.label().to_lowercase())
    }

    pub fn attach(&mut self, id: SessionId, term: Entity<TerminalSession>) {
        self.terms.insert(id, term);
    }

    pub fn remove(&mut self, id: SessionId) -> Option<SessionMeta> {
        self.terms.remove(&id);
        let pos = self.order.iter().position(|m| m.id == id)?;
        Some(self.order.remove(pos))
    }

    #[must_use]
    pub fn meta(&self, id: SessionId) -> Option<&SessionMeta> {
        self.order.iter().find(|m| m.id == id)
    }

    #[must_use]
    pub fn session(&self, id: SessionId) -> Option<&Entity<TerminalSession>> {
        self.terms.get(&id)
    }

    #[must_use]
    pub fn all(&self) -> &[SessionMeta] {
        &self.order
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Sessions living in `path`, in insertion order — the order the tree
    /// renders them in (`src/gui/view/sidebar.rs:338-355`).
    #[must_use]
    pub fn sessions_in_worktree(&self, path: &str) -> Vec<SessionId> {
        self.order
            .iter()
            .filter(|m| m.wt_path == path)
            .map(|m| m.id)
            .collect()
    }

    /// Sessions belonging to a project, by name — the project row's count and
    /// roll-up input (`sidebar.rs:243-248`).
    #[must_use]
    pub fn by_project(&self, name: &str) -> Vec<SessionId> {
        self.order
            .iter()
            .filter(|m| m.project == name)
            .map(|m| m.id)
            .collect()
    }

    // ── home terminals ──────────────────────────────────────────────────

    #[must_use]
    pub fn home_terminals(&self) -> &[SessionMeta] {
        &self.home
    }

    #[must_use]
    pub fn home_terminal_count(&self) -> usize {
        self.home.len()
    }

    #[must_use]
    pub fn home_terminal(&self, i: usize) -> Option<&Entity<TerminalSession>> {
        self.home_terms.get(i)
    }

    /// The next `terminal N` label (`src/app/terminals.rs:78-81`). Advancing
    /// the sequence is the caller's commitment to spawn.
    pub fn next_home_label(&mut self) -> String {
        self.home_terminal_seq += 1;
        format!("terminal {}", self.home_terminal_seq)
    }

    /// Mint an id for a home terminal. They live outside the map, but the id
    /// space is shared so nothing can ever collide.
    pub fn next_home_id(&mut self) -> SessionId {
        self.next_id()
    }

    pub fn push_home(&mut self, meta: SessionMeta, term: Entity<TerminalSession>) {
        self.home.push(meta);
        self.home_terms.push(term);
    }

    /// Bookkeeping half of a home-terminal close. Returns the removed entity so
    /// the caller can kill it. The **respawn** of a last-terminal close is the
    /// caller's job (it needs a `Context` to spawn) — see
    /// [`Self::home_terminals_need_spawn`].
    pub fn close_home(&mut self, i: usize) -> Option<Entity<TerminalSession>> {
        if i >= self.home.len() {
            return None;
        }
        self.home.remove(i);
        // `home_terms` is parallel to `home` in production; the pure tests
        // record metadata only, so a missing entity is not an error here.
        (i < self.home_terms.len()).then(|| self.home_terms.remove(i))
    }

    /// Spec's "pinned TERMINALS section (always ≥1 home terminal)": true both
    /// for the lazy first spawn and immediately after the last one is closed
    /// (`src/app/terminals.rs:21-30`).
    #[must_use]
    pub fn home_terminals_need_spawn(&self) -> bool {
        self.home.is_empty()
    }

    /// Test seam: record a home terminal's metadata without an entity.
    #[cfg(test)]
    fn push_home_meta(&mut self, label: String) {
        let id = self.next_id();
        self.home.push(SessionMeta {
            id,
            project: String::new(),
            wt_path: home_dir(),
            agent: Agent::Terminal,
            label,
            spawned_at: Instant::now(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_monotonic_and_stable_across_removals() {
        let mut r = SessionRegistry::new();
        let a = r.insert_meta("p".into(), "/a".into(), Agent::Claude);
        let b = r.insert_meta("p".into(), "/a".into(), Agent::Claude);
        assert!(b > a);
        r.remove(a);
        let c = r.insert_meta("p".into(), "/a".into(), Agent::Claude);
        // The freed id is never handed out again, and `b` still means `b`.
        assert!(c > b);
        assert!(r.meta(b).is_some());
        assert!(r.meta(a).is_none());
    }

    #[test]
    fn sessions_in_worktree_keeps_insertion_order() {
        let mut r = SessionRegistry::new();
        let a = r.insert_meta("p".into(), "/a".into(), Agent::Claude);
        let other = r.insert_meta("p".into(), "/b".into(), Agent::Claude);
        let b = r.insert_meta("p".into(), "/a".into(), Agent::Codex);
        assert_eq!(r.sessions_in_worktree("/a"), vec![a, b]);
        assert_eq!(r.sessions_in_worktree("/b"), vec![other]);
        assert!(r.sessions_in_worktree("/nope").is_empty());
    }

    #[test]
    fn removal_does_not_disturb_the_remaining_order() {
        let mut r = SessionRegistry::new();
        let a = r.insert_meta("p".into(), "/a".into(), Agent::Claude);
        let b = r.insert_meta("p".into(), "/a".into(), Agent::Claude);
        let c = r.insert_meta("p".into(), "/a".into(), Agent::Claude);
        r.remove(b);
        assert_eq!(r.sessions_in_worktree("/a"), vec![a, c]);
    }

    #[test]
    fn by_project_groups_by_name() {
        let mut r = SessionRegistry::new();
        let a = r.insert_meta("alpha".into(), "/a".into(), Agent::Claude);
        let _ = r.insert_meta("gamma".into(), "/g".into(), Agent::Claude);
        let b = r.insert_meta("alpha".into(), "/a-x".into(), Agent::Codex);
        assert_eq!(r.by_project("alpha"), vec![a, b]);
        assert_eq!(r.by_project("nope"), Vec::<SessionId>::new());
    }

    /// `src/app/terminals.rs:78-81` — the label sequence never rewinds, so a
    /// closed terminal's number is not handed to its replacement.
    #[test]
    fn home_terminal_labels_are_sequential_and_never_reused() {
        let mut r = SessionRegistry::new();
        let first = r.next_home_label();
        r.push_home_meta(first.clone());
        let second = r.next_home_label();
        r.push_home_meta(second.clone());
        assert_eq!(first, "terminal 1");
        assert_eq!(second, "terminal 2");

        r.close_home(0);
        let third = r.next_home_label();
        assert_eq!(third, "terminal 3");
    }

    #[test]
    fn closing_the_last_home_terminal_asks_for_a_respawn() {
        let mut r = SessionRegistry::new();
        assert!(r.home_terminals_need_spawn());
        let label = r.next_home_label();
        r.push_home_meta(label);
        assert!(!r.home_terminals_need_spawn());
        r.close_home(0);
        assert!(r.home_terminals_need_spawn());
        // Out-of-range closes are no-ops, not panics.
        assert!(r.close_home(7).is_none());
    }

    #[test]
    fn agent_labels_are_per_kind_sequences() {
        let mut r = SessionRegistry::new();
        let a = r.insert_meta("p".into(), "/a".into(), Agent::Claude);
        let b = r.insert_meta("p".into(), "/a".into(), Agent::Codex);
        let c = r.insert_meta("p".into(), "/a".into(), Agent::Claude);
        let label = |id| r.meta(id).map(|m| m.label.clone());
        assert_eq!(label(a).as_deref(), Some("claude 1"));
        assert_eq!(label(b).as_deref(), Some("codex 1"));
        assert_eq!(label(c).as_deref(), Some("claude 2"));
    }
}
