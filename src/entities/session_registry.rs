//! Many [`TerminalSession`]s under one owner, keyed by a stable [`SessionId`].
//! Unlike iced's index-based `Vec<Session>` (`src/gui/update/sessions.rs:270-280,109-113`), the id is monotonic and never reused, so removal moves nothing.
//! Home terminals live outside the map (`src/app/mod.rs:85-101`) and never appear as tree/activity rows or in cycling/kill machinery; they keep a positional index instead (`src/app/terminals.rs:61-84`).
//! Deviates from the plan's `IndexMap` sketch since `indexmap` isn't a workspace dependency; uses a `Vec<SessionMeta>` (`order`) beside a `HashMap` of live entities instead.

use std::collections::HashMap;
use std::time::Instant;

use gpui::Entity;
use grove_core::agent::Agent;
use grove_core::attention::{self, AttentionFiles};
use grove_core::session_meta;
use grove_core::tmux;

use crate::entities::terminal_session::TerminalSession;

/// Opaque, stable, monotonic session key. Never reused within a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(u64);

impl SessionId {
    /// Production ids only come from [`SessionRegistry::next_id`]; this is a test/fixture constructor.
    #[allow(dead_code)]
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
#[derive(Clone, Debug)]
pub struct SessionMeta {
    pub id: SessionId,
    /// `Project::name`, not index — indices move when a project is added or archived.
    pub project: String,
    pub wt_path: String,
    pub agent: Agent,
    pub context_roots: Vec<grove_core::session_meta::ContextRoot>,
    /// Internal label (`claude 1`, …); stripped from the OSC title to make the context text (`src/gui/rows.rs:778`).
    pub label: String,
    pub spawned_at: Instant,
    /// File name is `{our pid}-{our SessionId}.state`; the pid prefix, not the id, is what makes cross-run collision safe (`crates/grove-core/src/attention.rs:110-121`).
    pub attention: Option<AttentionFiles>,
    /// Recorded by [`SessionRegistry::attach`], since the spawn (not the metadata) decides the backend.
    pub tmux: bool,
    /// [`crate::reattach`]'s dedup key, shared by the startup scan and the tmux-toggle re-scan.
    pub tmux_name: Option<String>,
}

/// Where a new session opens. Replaces [`TerminalSession::spawn`]'s hardcoded single target.
#[derive(Clone, Debug)]
pub struct SpawnTarget {
    pub cwd: String,
    pub agent: Agent,
    /// Empty for home terminals, which belong to no project.
    pub project: String,
    pub label: String,
    /// Built the same way iced does (`src/app/spawn.rs:26-32`); chained before the attention `extra_args` on both backends (`crates/grove-core/src/session.rs:190,254-259`).
    pub args: Vec<String>,
    pub context_roots: Vec<grove_core::session_meta::ContextRoot>,
    /// Home terminals and panel shells must never be tmux-backed, or the next launch's discovery reimports them as agent sessions (`crates/grove-core/src/session.rs:149-175`).
    pub use_tmux: bool,
}

impl SpawnTarget {
    /// A native shell at `~` (`src/app/terminals.rs:7-10,86-95`).
    #[must_use]
    pub fn home(label: String) -> Self {
        Self {
            cwd: home_dir(),
            agent: Agent::Terminal,
            project: String::new(),
            label,
            args: Vec::new(),
            context_roots: Vec::new(),
            use_tmux: false,
        }
    }
}

/// Falls back to `/`, mirroring `src/app/terminals.rs:7-10`.
#[must_use]
pub fn home_dir() -> String {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "/".to_string())
}

#[derive(Default)]
pub struct SessionRegistry {
    /// Insertion order is the tree's within-worktree order.
    order: Vec<SessionMeta>,
    terms: HashMap<SessionId, Entity<TerminalSession>>,
    home: Vec<SessionMeta>,
    home_terms: Vec<Entity<TerminalSession>>,
    /// Keyed by absolute worktree path, with an active index per path.
    wt: HashMap<String, Vec<SessionMeta>>,
    wt_terms: HashMap<SessionId, Entity<TerminalSession>>,
    wt_active: HashMap<String, usize>,
    /// Awaiting the spawn call that consumes them ([`Self::take_attention_args`]).
    attention_args: HashMap<SessionId, Vec<String>>,
    next_id: u64,
    home_terminal_seq: usize,
}

impl SessionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Never reused, including across removals — this is what makes [`SessionId`] a safe map key.
    fn next_id(&mut self) -> SessionId {
        self.next_id += 1;
        SessionId(self.next_id)
    }

    /// The live entity is attached separately by [`Self::attach`], so ordering stays testable without spawning anything.
    #[allow(dead_code)]
    pub fn insert_meta(&mut self, project: String, wt_path: String, agent: Agent) -> SessionId {
        self.insert_meta_with_context(project, wt_path, agent, Vec::new())
    }

    pub fn insert_meta_with_context(
        &mut self,
        project: String,
        wt_path: String,
        agent: Agent,
        context_roots: Vec<grove_core::session_meta::ContextRoot>,
    ) -> SessionId {
        let id = self.next_id();
        let label = self.next_agent_label(agent);
        // Skipped under test: `prepare` writes a real file under the user's config dir, which pure bookkeeping tests must not litter.
        let prepared = if cfg!(test) {
            None
        } else {
            attention::prepare(agent, id.0)
        };
        let (args, attention) = match prepared {
            Some((args, files)) => (args, Some(files)),
            None => (Vec::new(), None),
        };
        self.attention_args.insert(id, args);
        self.order.push(SessionMeta {
            id,
            project,
            wt_path,
            agent,
            context_roots,
            label,
            spawned_at: Instant::now(),
            attention,
            tmux: false,
            tmux_name: None,
        });
        id
    }

    /// No `attention::prepare`: the process predates this run and its hook files are keyed to the prior pid, so the classifier falls through to the screen-scrape (`crates/grove-core/src/session.rs:344`).
    pub fn insert_reattached(&mut self, at: usize, d: &tmux::DiscoveredSession) -> SessionId {
        let id = self.next_id();
        let at = at.min(self.order.len());
        self.order.insert(
            at,
            SessionMeta {
                id,
                project: d.project.clone(),
                wt_path: d.wt_path.clone(),
                agent: d.agent,
                context_roots: d.context_roots.clone(),
                label: d.label.clone(),
                spawned_at: Instant::now(),
                attention: None,
                tmux: true,
                tmux_name: Some(d.name.clone()),
            },
        );
        id
    }

    /// Taken, not borrowed: used exactly once, by the spawn that follows [`Self::insert_meta`].
    pub fn take_attention_args(&mut self, id: SessionId) -> Vec<String> {
        self.attention_args.remove(&id).unwrap_or_default()
    }

    /// Same shape as the home terminals' `terminal N`.
    fn next_agent_label(&self, agent: Agent) -> String {
        let n = self.order.iter().filter(|m| m.agent == agent).count() + 1;
        format!("{} {n}", agent.label().to_lowercase())
    }

    /// `tmux_name` is the actual backend; the requested one can differ (missing `$PATH` falls back to native).
    pub fn attach(
        &mut self,
        id: SessionId,
        term: Entity<TerminalSession>,
        tmux_name: Option<String>,
    ) {
        self.terms.insert(id, term);
        if let Some(m) = self.order.iter_mut().find(|m| m.id == id) {
            m.tmux = tmux_name.is_some();
            m.tmux_name = tmux_name;
        }
    }

    /// Also cleans up attention files, so a killed session leaves no `.state` for the next GC to guess at (`session.rs:530-535`).
    pub fn remove(&mut self, id: SessionId) -> Option<SessionMeta> {
        self.terms.remove(&id);
        self.attention_args.remove(&id);
        let pos = self.order.iter().position(|m| m.id == id)?;
        let meta = self.order.remove(pos);
        // Reported here so no kill path can miss it (`src/gui/update/sessions.rs:261-269`).
        crate::telemetry::track(
            "session_ended",
            vec![
                ("agent", meta.agent.label().into()),
                (
                    "duration_min",
                    (meta.spawned_at.elapsed().as_secs() / 60).into(),
                ),
                ("tmux", meta.tmux.into()),
            ],
        );
        if let Some(files) = meta.attention.as_ref() {
            attention::cleanup(files);
        }
        // Without this the tmux session outlives grove and gets reattached on the next launch (`crates/grove-core/src/session.rs:522-534`); unlike iced, native children are not killpg'd here.
        if let Some(name) = meta.tmux_name.as_deref() {
            tmux::kill_session(name);
            session_meta::delete(name);
        }
        Some(meta)
    }

    /// The state file to truncate when the user acknowledges this session.
    #[must_use]
    pub fn attention_files(&self, id: SessionId) -> Option<&AttentionFiles> {
        self.meta(id).and_then(|m| m.attention.as_ref())
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

    /// In insertion order, matching how the tree renders them (`src/gui/view/sidebar.rs:338-355`).
    #[must_use]
    pub fn sessions_in_worktree(&self, path: &str) -> Vec<SessionId> {
        self.order
            .iter()
            .filter(|m| m.wt_path == path)
            .map(|m| m.id)
            .collect()
    }

    /// The project row's count/roll-up input (`sidebar.rs:243-248`).
    #[must_use]
    pub fn by_project(&self, name: &str) -> Vec<SessionId> {
        self.order
            .iter()
            .filter(|m| m.project == name)
            .map(|m| m.id)
            .collect()
    }

    /// The live half of a rename; the on-disk sidecar rename is a separate call (`grove_core::session_meta::rename_project`). Returns sessions updated.
    pub fn rename_project(&mut self, from: &str, to: &str) -> usize {
        if from == to {
            return 0;
        }
        let mut count = 0;
        for m in &mut self.order {
            if m.project == from {
                m.project = to.to_string();
                count += 1;
            }
            for root in &mut m.context_roots {
                if root.project == from {
                    root.project = to.to_string();
                }
            }
        }
        count
    }

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

    /// Advancing the sequence is the caller's commitment to spawn (`src/app/terminals.rs:78-81`).
    pub fn next_home_label(&mut self) -> String {
        self.home_terminal_seq += 1;
        format!("terminal {}", self.home_terminal_seq)
    }

    /// The id space is shared with `order` so nothing can ever collide.
    pub fn next_home_id(&mut self) -> SessionId {
        self.next_id()
    }

    pub fn push_home(&mut self, meta: SessionMeta, term: Entity<TerminalSession>) {
        self.home.push(meta);
        self.home_terms.push(term);
    }

    /// The restart path (`src/app/terminals.rs:38-53`); caller must only call this after the replacement spawned successfully.
    pub fn replace_home(
        &mut self,
        i: usize,
        term: Entity<TerminalSession>,
    ) -> Option<Entity<TerminalSession>> {
        if i >= self.home_terms.len() {
            return None;
        }
        Some(std::mem::replace(&mut self.home_terms[i], term))
    }

    /// Respawn on a last-terminal close is the caller's job — see [`Self::home_terminals_need_spawn`].
    pub fn close_home(&mut self, i: usize) -> Option<Entity<TerminalSession>> {
        if i >= self.home.len() {
            return None;
        }
        self.home.remove(i);
        // Pure tests record metadata only, so a missing entity here is not an error.
        (i < self.home_terms.len()).then(|| self.home_terms.remove(i))
    }

    #[must_use]
    pub fn home_terminals_need_spawn(&self) -> bool {
        self.home.is_empty()
    }

    // Shells are ported in shape from `App::wt_terminals`/`wt_active_terminal` (`src/app/terminals.rs:110-176`); native, not tmux, like home terminals (`sidebar.rs:297-301`).

    #[must_use]
    pub fn wt_shells(&self, wt_path: &str) -> &[SessionMeta] {
        self.wt.get(wt_path).map_or(&[][..], Vec::as_slice)
    }

    #[must_use]
    pub fn active_wt_shell_idx(&self, wt_path: &str) -> Option<usize> {
        self.wt_active.get(wt_path).copied()
    }

    #[must_use]
    pub fn wt_shell(&self, wt_path: &str, idx: usize) -> Option<&Entity<TerminalSession>> {
        let id = self.wt.get(wt_path)?.get(idx)?.id;
        self.wt_terms.get(&id)
    }

    /// Panel shells share the home terminals' label sequence.
    pub fn next_wt_label(&mut self) -> String {
        self.next_home_label()
    }

    pub fn push_wt_shell(
        &mut self,
        wt_path: &str,
        meta: SessionMeta,
        term: Option<Entity<TerminalSession>>,
    ) {
        let id = meta.id;
        let shells = self.wt.entry(wt_path.to_string()).or_default();
        shells.push(meta);
        let idx = shells.len() - 1;
        if let Some(term) = term {
            self.wt_terms.insert(id, term);
        }
        self.wt_active.insert(wt_path.to_string(), idx);
    }

    /// The spawn itself needs a `Context`, so it stays the caller's job (`src/app/terminals.rs:133-149`).
    #[must_use]
    pub fn wt_shells_need_spawn(&self, wt_path: &str) -> bool {
        self.wt_shells(wt_path).is_empty()
    }

    /// Out of range is a no-op, never a clamp.
    pub fn select_wt_shell(&mut self, wt_path: &str, idx: usize) {
        if idx < self.wt_shells(wt_path).len() {
            self.wt_active.insert(wt_path.to_string(), idx);
        }
    }

    /// Unlike the home terminal, does not respawn when the last shell closes — an empty panel is valid (`src/app/terminals.rs:172-201`).
    pub fn close_wt_shell(&mut self, wt_path: &str, idx: usize) -> Option<Entity<TerminalSession>> {
        let shells = self.wt.get_mut(wt_path)?;
        if idx >= shells.len() {
            return None;
        }
        let meta = shells.remove(idx);
        let removed = self.wt_terms.remove(&meta.id);
        let len = shells.len();
        let new_active = match self.wt_active.get(wt_path).copied() {
            Some(a) if a == idx => (len > 0).then(|| idx.min(len - 1)),
            Some(a) if a > idx => Some(a - 1),
            other => other,
        };
        match new_active {
            Some(a) => {
                self.wt_active.insert(wt_path.to_string(), a);
            }
            None => {
                self.wt_active.remove(wt_path);
            }
        }
        removed
    }

    #[cfg(test)]
    fn push_wt_meta(&mut self, wt_path: &str) {
        let id = self.next_id();
        let label = self.next_wt_label();
        self.push_wt_shell(
            wt_path,
            SessionMeta {
                id,
                project: String::new(),
                wt_path: wt_path.to_string(),
                agent: Agent::Terminal,
                context_roots: Vec::new(),
                label,
                spawned_at: Instant::now(),
                attention: None,
                tmux: false,
                tmux_name: None,
            },
            None,
        );
    }

    #[cfg(test)]
    fn push_home_meta(&mut self, label: String) {
        let id = self.next_id();
        self.home.push(SessionMeta {
            id,
            project: String::new(),
            wt_path: home_dir(),
            agent: Agent::Terminal,
            context_roots: Vec::new(),
            label,
            spawned_at: Instant::now(),
            attention: None,
            tmux: false,
            tmux_name: None,
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

    #[test]
    fn rename_project_relabels_matching_sessions_only() {
        let mut r = SessionRegistry::new();
        let a = r.insert_meta("alpha".into(), "/a".into(), Agent::Claude);
        let g = r.insert_meta("gamma".into(), "/g".into(), Agent::Claude);
        let b = r.insert_meta("alpha".into(), "/a-x".into(), Agent::Codex);

        let count = r.rename_project("alpha", "alpha-renamed");
        assert_eq!(count, 2);
        assert_eq!(r.by_project("alpha"), Vec::<SessionId>::new());
        assert_eq!(r.by_project("alpha-renamed"), vec![a, b]);
        assert_eq!(r.meta(g).map(|m| m.project.as_str()), Some("gamma"));
    }

    #[test]
    fn rename_project_is_a_noop_when_names_match() {
        let mut r = SessionRegistry::new();
        r.insert_meta("alpha".into(), "/a".into(), Agent::Claude);
        assert_eq!(r.rename_project("alpha", "alpha"), 0);
    }

    /// The label sequence never rewinds (`src/app/terminals.rs:78-81`).
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

    #[test]
    fn reattached_session_keeps_its_persisted_multi_root_context() {
        let roots = vec![
            grove_core::session_meta::ContextRoot {
                project: "portfolio".into(),
                wt_path: "/portfolio".into(),
            },
            grove_core::session_meta::ContextRoot {
                project: "api".into(),
                wt_path: "/api".into(),
            },
        ];
        let discovered = grove_core::tmux::DiscoveredSession {
            name: "grove-portfolio-claude-1".into(),
            wt_path: "/portfolio".into(),
            project: "portfolio".into(),
            label: "claude 1".into(),
            agent: Agent::Claude,
            context_roots: roots.clone(),
        };
        let mut registry = SessionRegistry::new();
        let id = registry.insert_reattached(0, &discovered);
        assert_eq!(
            registry.meta(id).map(|meta| &meta.context_roots),
            Some(&roots)
        );
    }

    /// The first shell is spawned on demand, and something is always selected afterward (`src/app/terminals.rs:133-149`).
    #[test]
    fn a_worktree_starts_with_no_shell_and_selects_the_first_one_added() {
        let mut r = SessionRegistry::new();
        assert!(r.wt_shells_need_spawn("/a"));
        assert_eq!(r.active_wt_shell_idx("/a"), None);

        r.push_wt_meta("/a");
        assert!(!r.wt_shells_need_spawn("/a"));
        assert_eq!(r.wt_shells("/a").len(), 1);
        assert_eq!(r.active_wt_shell_idx("/a"), Some(0));
    }

    #[test]
    fn adding_a_shell_focuses_it_and_selection_is_bounds_checked() {
        let mut r = SessionRegistry::new();
        r.push_wt_meta("/a");
        r.push_wt_meta("/a");
        assert_eq!(r.active_wt_shell_idx("/a"), Some(1));

        r.select_wt_shell("/a", 0);
        assert_eq!(r.active_wt_shell_idx("/a"), Some(0));
        r.select_wt_shell("/a", 9);
        assert_eq!(r.active_wt_shell_idx("/a"), Some(0));
    }

    #[test]
    fn closing_shells_shifts_the_active_index_and_may_empty_the_panel() {
        let mut r = SessionRegistry::new();
        for _ in 0..3 {
            r.push_wt_meta("/a");
        }
        // Closing a shell *before* the active one shifts it down.
        r.select_wt_shell("/a", 2);
        r.close_wt_shell("/a", 0);
        assert_eq!(r.active_wt_shell_idx("/a"), Some(1));
        assert_eq!(r.wt_shells("/a").len(), 2);

        // Closing the active one refocuses whatever filled its slot.
        r.select_wt_shell("/a", 0);
        r.close_wt_shell("/a", 0);
        assert_eq!(r.active_wt_shell_idx("/a"), Some(0));

        // Closing the last one leaves an empty panel — no respawn.
        r.close_wt_shell("/a", 0);
        assert!(r.wt_shells("/a").is_empty());
        assert_eq!(r.active_wt_shell_idx("/a"), None);
        assert!(r.wt_shells_need_spawn("/a"));
        // Out-of-range closes are no-ops, not panics.
        assert!(r.close_wt_shell("/a", 4).is_none());
        assert!(r.close_wt_shell("/nope", 0).is_none());
    }

    #[test]
    fn closing_the_last_shell_clamps_the_focus_back() {
        let mut r = SessionRegistry::new();
        r.push_wt_meta("/a");
        r.push_wt_meta("/a");
        assert_eq!(r.active_wt_shell_idx("/a"), Some(1));
        r.close_wt_shell("/a", 1);
        assert_eq!(r.active_wt_shell_idx("/a"), Some(0));
    }

    /// Panels are per worktree: one path's shells never move another's.
    #[test]
    fn panels_are_keyed_per_worktree() {
        let mut r = SessionRegistry::new();
        r.push_wt_meta("/a");
        r.push_wt_meta("/b");
        r.push_wt_meta("/b");
        assert_eq!(r.wt_shells("/a").len(), 1);
        assert_eq!(r.wt_shells("/b").len(), 2);
        r.close_wt_shell("/b", 0);
        assert_eq!(r.wt_shells("/a").len(), 1);
        assert_eq!(r.active_wt_shell_idx("/a"), Some(0));
    }
}
