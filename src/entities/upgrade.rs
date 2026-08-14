//! The live upgrade flow. Carried decision 2: the blocking/async boundary is gpui's background executor, not iced's hand-rolled thread + mutex drain.

use std::time::Duration;

use futures::StreamExt as _;
use gpui::{AppContext as _, Context, Task};
use grove_core::upgrade::{self, InstallMethod, Release, Stage};

use crate::entities::upgrade_state::{
    apply_check_result, apply_finished, apply_progress, begin_check, check_due, skip_version,
    ChangelogState, UpgradeState,
};
use crate::settings::SettingsState;

/// Exists so the first frame is up before the network round-trip; do not shorten it (`src/gui/mod.rs:56-63`).
pub const LAUNCH_CHECK_DELAY: Duration = Duration::from_secs(3);

/// The check itself is 24h (`check_due`); this is only how often that question gets asked (recorded ambiguity 2).
pub const PERIODIC_TICK: Duration = Duration::from_secs(1);

/// How many releases the changelog fetches (`src/gui/update/upgrade.rs:227`).
pub const CHANGELOG_LIMIT: usize = 10;

pub struct Upgrade {
    state: UpgradeState,
    changelog: ChangelogState,
    method: InstallMethod,
    /// Held, never read: dropping a `Task` cancels it, so this field *is* the timers' lifetime.
    _timers: Vec<Task<()>>,
    check_task: Option<Task<()>>,
    changelog_task: Option<Task<()>>,
    apply_task: Option<Task<()>>,
}

impl Upgrade {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let launch = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LAUNCH_CHECK_DELAY).await;
            let _ = this.update(cx, |this, cx| this.check(false, cx));
        });
        let periodic = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(PERIODIC_TICK).await;
            if this.update(cx, Self::check_if_due).is_err() {
                return;
            }
        });
        Self {
            state: UpgradeState::Idle,
            changelog: ChangelogState::Idle,
            method: upgrade::detect(),
            _timers: vec![launch, periodic],
            check_task: None,
            changelog_task: None,
            apply_task: None,
        }
    }

    pub fn state(&self) -> &UpgradeState {
        &self.state
    }

    pub fn changelog(&self) -> &ChangelogState {
        &self.changelog
    }

    pub fn method(&self) -> InstallMethod {
        self.method
    }

    /// Refocus path exists because an idle unfocused window stops ticking (`src/gui/update/upgrade.rs:193-207`).
    pub fn check_if_due(&mut self, cx: &mut Context<Self>) {
        let last = cx.global::<SettingsState>().store.last_update_check;
        if check_due(last, now_unix(), &self.state) {
            self.check(false, cx);
        }
    }

    /// `manual` only selects the error policy; all three triggers route through `begin_check`, so a duplicate is impossible.
    pub fn check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if !begin_check(&self.state) {
            return;
        }
        self.state = UpgradeState::Checking;
        cx.notify();
        let fetch = cx.background_spawn(async { upgrade::latest().map_err(|e| e.to_string()) });
        self.check_task = Some(cx.spawn(async move |this, cx| {
            let result = fetch.await;
            let _ = this.update(cx, |this, cx| {
                // Recorded ambiguity 3: written on every outcome, so a network-down machine backs off instead of retrying forever.
                SettingsState::update(cx, |store| store.last_update_check = Some(now_unix()));
                let skipped = cx.global::<SettingsState>().store.skipped_version.clone();
                this.state = apply_check_result(
                    result,
                    manual,
                    env!("CARGO_PKG_VERSION"),
                    skipped.as_deref(),
                );
                cx.notify();
            });
        }));
    }

    pub fn fetch_changelog(&mut self, cx: &mut Context<Self>) {
        self.changelog = ChangelogState::Loading;
        cx.notify();
        let fetch = cx.background_spawn(async {
            upgrade::releases(CHANGELOG_LIMIT).map_err(|e| e.to_string())
        });
        self.changelog_task = Some(cx.spawn(async move |this, cx| {
            let result = fetch.await;
            let _ = this.update(cx, |this, cx| {
                this.changelog = match result {
                    Ok(notes) => ChangelogState::Loaded(notes),
                    Err(e) => ChangelogState::Error(e),
                };
                cx.notify();
            });
        }));
    }

    pub fn available(&self) -> Option<&Release> {
        match &self.state {
            UpgradeState::Available(r) => Some(r),
            _ => None,
        }
    }

    pub fn skip(&mut self, cx: &mut Context<Self>) {
        let (tag, next) = skip_version(&self.state);
        if let Some(tag) = tag {
            SettingsState::update(cx, |store| store.skipped_version = Some(tag.clone()));
            SettingsState::flush_now(cx);
            crate::telemetry::track("update_declined", vec![("version", tag.into())]);
        }
        self.state = next;
        cx.notify();
    }

    /// The channel closing orders the last stage before finish, so a late stage can never resurrect `Updating`.
    pub fn start_update(&mut self, cx: &mut Context<Self>) {
        let Some(release) = self.available().cloned() else {
            return;
        };
        let method = self.method;
        self.state = UpgradeState::Updating(Stage::Downloading);
        cx.notify();

        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Stage>();
        let tag = release.tag.clone();
        let apply = cx.background_spawn(async move {
            let cb = move |stage: Stage| {
                let _ = tx.unbounded_send(stage);
            };
            upgrade::apply(method, &release, &cb).map_err(|e| e.to_string())
        });
        self.apply_task = Some(cx.spawn(async move |this, cx| {
            while let Some(stage) = rx.next().await {
                if this
                    .update(cx, |this, cx| {
                        this.state = apply_progress(&this.state, stage);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let result = apply.await;
            if result.is_ok() {
                crate::telemetry::track("update_applied", vec![("to_version", tag.into())]);
            }
            let _ = this.update(cx, |this, cx| {
                this.state = apply_finished(&this.state, result);
                cx.notify();
            });
        }));
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These are the oracle's numbers, not convenient round ones (`src/gui/update/upgrade.rs:227`).
    #[test]
    fn the_ported_constants_match_the_oracle() {
        assert_eq!(LAUNCH_CHECK_DELAY, Duration::from_secs(3));
        assert_eq!(CHANGELOG_LIMIT, 10);
        assert_eq!(PERIODIC_TICK, Duration::from_secs(1));
    }

    #[test]
    fn now_unix_is_sane() {
        assert!(now_unix() > 1_700_000_000);
    }
}
