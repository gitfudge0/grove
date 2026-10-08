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

    /// Refresh the offered release before installing; explicit intent overrides version skips.
    pub fn start_update(&mut self, cx: &mut Context<Self>) {
        self.start_update_with(
            async { upgrade::latest().map_err(|e| e.to_string()) },
            Self::install,
            cx,
        );
    }

    fn start_update_with(
        &mut self,
        fetch: impl std::future::Future<Output = Result<Release, String>> + Send + 'static,
        install: impl FnOnce(&mut Self, Release, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.available().is_none() {
            return;
        }
        self.state = UpgradeState::Checking;
        cx.notify();
        let fetch = cx.background_spawn(fetch);
        self.check_task = Some(cx.spawn(async move |this, cx| {
            let result = fetch.await;
            let _ = this.update(cx, |this, cx| {
                SettingsState::update(cx, |store| store.last_update_check = Some(now_unix()));
                this.state = apply_check_result(result, true, env!("CARGO_PKG_VERSION"), None);
                if let Some(release) = this.available().cloned() {
                    install(this, release, cx);
                } else {
                    cx.notify();
                }
            });
        }));
    }

    /// The channel closing orders the last stage before finish, so a late stage can never resurrect `Updating`.
    fn install(&mut self, release: Release, cx: &mut Context<Self>) {
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

    fn release(tag: &str) -> Release {
        Release {
            version: semver::Version::parse(tag.trim_start_matches('v')).unwrap(),
            tag: tag.into(),
            html_url: format!("https://example.invalid/{tag}"),
            body: String::new(),
            dmg_url: None,
            dmg_sha256_url: None,
            target_commitish: "main".into(),
        }
    }

    fn offered_upgrade(cx: &mut gpui::TestAppContext) -> gpui::Entity<Upgrade> {
        cx.update(|cx| {
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                skipped_version: Some("v99.0.0".into()),
                ..Default::default()
            }));
        });
        cx.new(|_| Upgrade {
            state: UpgradeState::Available(release("v98.0.0")),
            changelog: ChangelogState::Idle,
            method: InstallMethod::Unknown,
            _timers: vec![],
            check_task: None,
            changelog_task: None,
            apply_task: None,
        })
    }

    #[gpui::test]
    fn update_fetches_fresh_release_before_install_and_ignores_skip(cx: &mut gpui::TestAppContext) {
        let upgrade = offered_upgrade(cx);
        let (send, receive) = futures::channel::oneshot::channel();
        let installed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        upgrade.update(cx, |upgrade, cx| {
            upgrade.start_update_with(
                async move { receive.await.unwrap() },
                {
                    let installed = installed.clone();
                    move |upgrade, release, _| {
                        installed.borrow_mut().push(release.tag);
                        upgrade.state = UpgradeState::Updating(Stage::Downloading);
                    }
                },
                cx,
            );
            assert!(matches!(upgrade.state(), UpgradeState::Checking));
            // Duplicate update and check must neither install nor replace the pending fetch.
            upgrade.start_update_with(
                async { panic!("duplicate fetched") },
                |_, _, _| panic!("duplicate installed"),
                cx,
            );
            upgrade.check(true, cx);
        });
        cx.run_until_parked();
        assert!(installed.borrow().is_empty());
        send.send(Ok(release("v99.0.0"))).unwrap();
        cx.run_until_parked();
        assert_eq!(*installed.borrow(), ["v99.0.0"]);
        upgrade.update(cx, |upgrade, cx| {
            assert!(matches!(upgrade.state(), UpgradeState::Updating(_)));
            upgrade.start_update_with(
                async { panic!("install interrupted") },
                |_, _, _| panic!("installed twice"),
                cx,
            );
            upgrade.check(true, cx);
        });
        cx.run_until_parked();
        assert_eq!(*installed.borrow(), ["v99.0.0"]);
        cx.update(|cx| {
            assert!(cx
                .global::<SettingsState>()
                .store
                .last_update_check
                .is_some());
        });
    }

    #[gpui::test]
    fn failed_preflight_does_not_fall_back_to_cached_release(cx: &mut gpui::TestAppContext) {
        let upgrade = offered_upgrade(cx);
        upgrade.update(cx, |upgrade, cx| {
            upgrade.start_update_with(
                async { Err("offline".into()) },
                |_, _, _| panic!("cached release installed after fetch failure"),
                cx,
            );
        });
        cx.run_until_parked();
        upgrade.read_with(cx, |upgrade, _| {
            assert!(matches!(upgrade.state(), UpgradeState::Error(error) if error == "offline"));
        });
        cx.update(|cx| {
            assert!(cx
                .global::<SettingsState>()
                .store
                .last_update_check
                .is_some());
        });
    }

    #[gpui::test]
    fn current_and_older_preflight_releases_are_not_installed(cx: &mut gpui::TestAppContext) {
        for tag in [env!("CARGO_PKG_VERSION"), "v0.0.0"] {
            let upgrade = offered_upgrade(cx);
            let latest = release(tag);
            upgrade.update(cx, |upgrade, cx| {
                upgrade.start_update_with(
                    async move { Ok(latest) },
                    |_, _, _| panic!("non-newer release installed"),
                    cx,
                );
            });
            cx.run_until_parked();
            upgrade.read_with(cx, |upgrade, _| {
                assert!(matches!(upgrade.state(), UpgradeState::UpToDate));
            });
            cx.update(|cx| {
                assert!(cx
                    .global::<SettingsState>()
                    .store
                    .last_update_check
                    .is_some());
            });
        }
    }

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
