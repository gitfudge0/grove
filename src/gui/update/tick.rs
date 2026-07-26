//! `Msg::Tick` handling and the periodic session-activity classification it
//! drives (~480ms cadence, see `refresh_activity`'s doc comment).

use crate::gui::state::{Grove, Msg, UpgradeState};
use grove_core::agent::Agent;
use iced::Task;
use std::sync::Arc;

impl Grove {
    /// Handle `Msg::Tick`, fired every 60ms (idle-unfocused: every 1s) — see
    /// `Grove::subscription`'s doc comment for the cadence rules. Advances
    /// the blink counter, flushes debounced writes, drains background-job
    /// results, re-classifies session activity every 8th tick, polls git
    /// status, advances worktree teardown, and drains upgrade-apply progress.
    pub(super) fn on_tick(&mut self) -> Task<Msg> {
        // Advance the blink counter (~30 Hz at 60 ms tick interval).
        self.anim.blink_tick = self.anim.blink_tick.wrapping_add(1);
        // Debounced ui_zoom persistence: count down the quiet period
        // and flush once it elapses. See `set_ui_zoom`.
        if let Some(n) = self.pty_layout.zoom_save_countdown {
            if n <= 1 {
                self.flush_ui_zoom_save();
            } else {
                self.pty_layout.zoom_save_countdown = Some(n - 1);
            }
        }
        self.tick_drag_autoscroll();
        // Auto-dismiss the toast once its kind-dependent TTL elapses.
        if self
            .app
            .toast
            .as_ref()
            .is_some_and(|t| t.expired_at(std::time::Instant::now()))
        {
            self.app.toast = None;
        }
        // Surface results from background jobs (.worktreeinclude
        // generation runs off-thread).
        let bg = self.app.bg_status.lock().ok().and_then(|mut g| g.take());
        if let Some(msg) = bg {
            self.app.set_toast(msg);
            self.app.refresh_worktrees();
        }
        // Re-classify session activity every 8th tick (~480ms at 60ms).
        if self.anim.blink_tick.is_multiple_of(8) {
            self.refresh_activity();
        }
        // Throttled background git-status poll for visible worktrees
        // (dirty / ahead / behind sidebar suffix).
        self.maybe_poll_git_state();
        // Kick off a pending off-thread `git worktree list` sweep (requested
        // by `rebuild_wt_cache`). Held until here so a burst of rebuild
        // requests within one tick collapses into a single sweep.
        let wt_rebuild = self.maybe_rebuild_wt_cache();
        // Advance an in-progress worktree teardown (script exit → git
        // removal). Cheap no-op when none is running.
        if self.app.teardown.is_some() {
            let had_session = self
                .app
                .teardown
                .as_ref()
                .and_then(|t| t.session.as_ref())
                .map(|s| Arc::as_ptr(&s.dirty) as usize);
            self.app.poll_teardown();
            // The teardown PTY was dropped during removal — evict its
            // render-cache entry so a future session can't alias its
            // (now reusable) dirty-Arc address.
            let still = self
                .app
                .teardown
                .as_ref()
                .and_then(|t| t.session.as_ref())
                .is_some();
            if let (Some(key), false) = (had_session, still) {
                self.pty_cache.borrow_mut().remove(&key);
            }
        }
        // Drain apply progress (set by the background apply thread).
        {
            let drained = if let Ok(mut g) = self.upgrade_progress.lock() {
                let stage = g.stage.take();
                let finished = g.finished.take();
                (stage, finished)
            } else {
                (None, None)
            };
            if let Some(stage) = drained.0 {
                self.upgrade = UpgradeState::Updating(stage);
            }
            if let Some(result) = drained.1 {
                self.upgrade = match result {
                    Ok(()) => UpgradeState::Updated,
                    Err(e) => UpgradeState::UpdateFailed(e),
                };
            }
        }
        // Periodic update check: at most once per 24h while running.
        if let Some(task) = self.maybe_check_updates_due() {
            return Task::batch([wt_rebuild, task]);
        }
        wt_rebuild
    }

    /// Recompute every session's `ActivityState` from its live signals.
    /// Runs every ~480ms; also prunes trackers for sessions that no longer
    /// exist and pushes dock badge/bounce updates on transitions.
    ///
    /// Signal precedence, highest first: native poll (`claude_agents`) >
    /// hook state file (`attention`) > screen-scraping heuristics
    /// (`activity::classify`). See the per-session loop below.
    fn refresh_activity(&mut self) {
        use crate::gui::activity::{classify, ActivityState, Signals};
        let now = std::time::Instant::now();
        let mut live_keys: std::collections::HashSet<u64> =
            std::collections::HashSet::with_capacity(self.app.sessions.len());
        let mut newly_waiting = false;

        // Only worth polling `claude agents --json` while at least one live
        // Claude session exists to inform — see `claude_agents::Poller`.
        let any_live_claude = self.app.sessions.iter().any(|s| {
            matches!(s.status(), grove_core::session::SessionStatus::Running)
                && s.agent == Agent::Claude
        });
        self.claude_poller.set_wanted(any_live_claude);

        for (i, s) in self.app.sessions.iter().enumerate() {
            live_keys.insert(s.id);
            let focused = self.app.active_session == Some(i) && self.window_focused;
            let tracker = self.activity.entry(s.id).or_default();

            // Consume new bells: pending only when they ring unfocused.
            let bells = s.bell_count();
            if bells < tracker.bell_seen {
                // The counter only goes backwards if the vt100 parser was
                // reset/replaced — resync instead of going silent forever.
                tracker.bell_seen = bells;
            } else if bells > tracker.bell_seen {
                tracker.bell_seen = bells;
                if !focused {
                    tracker.bell_pending = true;
                }
            }

            let alive = matches!(s.status(), grove_core::session::SessionStatus::Running);
            let t = *s.last_output_at.lock().unwrap_or_else(|e| e.into_inner());
            let output_age = now.saturating_duration_since(t);
            // Scraping the tail takes the parser lock and copies 15 lines out
            // of the vt100 screen, but the two higher-precedence signals below
            // (native poll, hook state file) discard it for most sessions —
            // so it stays lazy and is only paid for inside the `classify`
            // arms that actually consume it.
            let tail = || {
                if alive {
                    s.tail_contents(15)
                } else {
                    String::new()
                }
            };

            let scrolling = s
                .scroll_age()
                .is_some_and(|a| a < crate::gui::activity::SCROLL_QUIET);
            let interacting = s
                .input_age()
                .is_some_and(|a| a < crate::gui::activity::INPUT_QUIET);
            let sig = Signals {
                alive,
                output_age,
                bell_pending: tracker.bell_pending,
                was_working: tracker.was_working,
                focused,
                scrolling,
                interacting,
                // Structured OSC title — primary working signal for agents
                // that emit one; vt100 already tracks it from the PTY stream.
                title: if alive { s.current_title() } else { None },
            };
            // Precedence, highest first: native poll (`claude_agents`) >
            // hook state file (`attention`) > screen-scraping heuristics
            // (`activity::classify`). The native poll is the most
            // authoritative signal when available (it comes straight from
            // the Claude CLI, not a hook we injected or the terminal
            // contents), and it's also the only one of the three that works
            // for tmux sessions reattached across a grove restart. It's
            // consulted only for alive Claude sessions; everything else
            // falls straight through to the existing hook/heuristic chain,
            // unchanged.
            let native = if alive && s.agent == Agent::Claude {
                self.claude_poller.status_for(s.root_pid(), &s.wt_path)
            } else {
                None
            };
            let new_state = if let Some(native_status) = native {
                match native_status {
                    grove_core::claude_agents::NativeStatus::Busy => ActivityState::Working,
                    // A `Waiting` signal while focused is treated like the
                    // user has already seen it, mirroring the same downgrade
                    // rule the hook-state-file branch below applies to
                    // `NeedsYou` (never resurrect the highest-urgency state
                    // on the session they're looking at).
                    grove_core::claude_agents::NativeStatus::Waiting => {
                        if !focused {
                            ActivityState::WaitingForInput
                        } else {
                            ActivityState::Working
                        }
                    }
                    grove_core::claude_agents::NativeStatus::Idle => {
                        if tracker.was_working {
                            ActivityState::Done
                        } else {
                            ActivityState::Idle
                        }
                    }
                }
            } else {
                // Claude/Codex sessions with a hook/notify state file get a
                // deterministic signal that outranks the screen-scraping
                // heuristic below (but never a dead process — a stale `working`
                // left behind by a killed agent must still show Exited). A
                // `NeedsYou` signal while focused is treated like the user has
                // already seen it (never resurrect the highest-urgency state on
                // the session they're looking at, mirroring
                // `Tracker::acknowledge`'s existing downgrade rule).
                match (alive, s.attention_state()) {
                    (false, _) => classify(s.agent, &tail(), &sig),
                    (true, Some(grove_core::attention::AttentionState::NeedsYou)) if !focused => {
                        ActivityState::WaitingForInput
                    }
                    (true, Some(grove_core::attention::AttentionState::NeedsYou)) => {
                        ActivityState::Working
                    }
                    (true, Some(grove_core::attention::AttentionState::Done)) => {
                        ActivityState::Done
                    }
                    (true, Some(grove_core::attention::AttentionState::Working)) => {
                        ActivityState::Working
                    }
                    (true, None) => classify(s.agent, &tail(), &sig),
                }
            };
            if new_state == ActivityState::Working {
                tracker.was_working = true;
            }
            if !alive {
                tracker.was_working = false;
                tracker.bell_pending = false;
            }
            if focused {
                // Watching it = continuously acknowledged.
                tracker.bell_pending = false;
            }
            if new_state == ActivityState::WaitingForInput
                && tracker.state != ActivityState::WaitingForInput
            {
                newly_waiting = true;
            }
            tracker.state = new_state;
        }

        self.activity.retain(|k, _| live_keys.contains(k));

        // Dock: badge = waiting count; one bounce per enter-while-unfocused.
        let waiting = self
            .activity
            .values()
            .filter(|t| t.state == ActivityState::WaitingForInput)
            .count();
        if waiting != self.last_badge {
            crate::gui::dock::set_badge(waiting);
            self.last_badge = waiting;
        }
        // Start/stop the needs-attention pulse to match the waiting set.
        if (waiting > 0) != self.anim.attention_anim.value() {
            if waiting > 0 {
                self.anim.attention_anim.go_mut(true, now);
            } else {
                self.anim.attention_anim = Self::attention_animation();
            }
        }
        if newly_waiting && !self.window_focused {
            crate::gui::dock::request_attention();
        }
    }
}
