use crate::app::Modal;
use crate::gui::state::{ChangelogState, Grove, Msg, ToolStatus, UpgradeMsg, UpgradeState};
use grove_core::agent::Agent;
use iced::Task;

impl Grove {
    /// Update-check / changelog / self-upgrade family dispatch
    /// (`Msg::Upgrade`).
    pub(super) fn on_upgrade(&mut self, msg: UpgradeMsg) -> Task<Msg> {
        match msg {
            UpgradeMsg::CheckForUpdates { manual } => return self.on_check_for_updates(manual),
            UpgradeMsg::UpdateCheckResult(result, manual) => {
                self.on_update_check_result(result, manual)
            }
            UpgradeMsg::SkipVersion => self.on_skip_version(),
            UpgradeMsg::CopyReleaseUrl => self.on_copy_release_url(),
            UpgradeMsg::StartUpdate => return self.on_start_update(),
            UpgradeMsg::RestartApp => self.on_restart_app(),
            UpgradeMsg::OpenChangelog => return self.on_open_changelog(),
            UpgradeMsg::ChangelogLoaded(result) => self.on_changelog_loaded(result),
            UpgradeMsg::CloseChangelog => self.on_close_changelog(),
        }
        Task::none()
    }

    pub(in crate::gui) fn on_check_for_updates(&mut self, manual: bool) -> Task<Msg> {
        // Guard: don't fire a duplicate request if a check is already in-flight.
        if matches!(self.upgrade, UpgradeState::Checking) {
            return Task::none();
        }
        self.check_updates_task(manual)
    }

    pub(super) fn on_update_check_result(
        &mut self,
        result: Result<grove_core::upgrade::Release, String>,
        manual: bool,
    ) {
        // Record the check time regardless of outcome so the periodic trigger backs off.
        self.app.store.last_update_check = Some(super::now_unix());
        grove_core::storage::persist(&self.app.store);
        match result {
            Ok(release) => {
                let current = env!("CARGO_PKG_VERSION");
                let skipped = self.app.store.skipped_version.as_deref();
                if grove_core::upgrade::update_available(current, &release, skipped) {
                    self.upgrade = UpgradeState::Available(release);
                } else {
                    self.upgrade = UpgradeState::UpToDate;
                }
            }
            Err(e) => {
                eprintln!("update check failed: {e}");
                if manual {
                    // Manual checks surface the error inline so the user knows.
                    self.upgrade = UpgradeState::Error(e);
                } else {
                    // Launch/periodic checks fail silently (log only; no badge/error shown).
                    self.upgrade = UpgradeState::Idle;
                }
            }
        }
    }

    pub(super) fn on_skip_version(&mut self) {
        if let UpgradeState::Available(release) = &self.upgrade {
            self.app.store.skipped_version = Some(release.tag.clone());
            grove_core::storage::persist(&self.app.store);
            crate::telemetry::track(
                "update_declined",
                vec![("version", release.tag.clone().into())],
            );
        }
        self.upgrade = UpgradeState::UpToDate;
    }

    pub(super) fn on_copy_release_url(&mut self) {
        if let UpgradeState::Available(r) = &self.upgrade {
            crate::clipboard::copy(&r.html_url);
            self.app.set_toast("release url copied");
        }
    }

    pub(super) fn on_start_update(&mut self) -> Task<Msg> {
        let UpgradeState::Available(release) = self.upgrade.clone() else {
            return Task::none();
        };
        let method = self.upgrade_method;
        self.upgrade = UpgradeState::Updating(grove_core::upgrade::Stage::Downloading);
        self.set_modal(Modal::Updating);

        let handle = self.upgrade_progress.clone();
        std::thread::spawn(move || {
            let cb_handle = handle.clone();
            let cb = move |stage: grove_core::upgrade::Stage| {
                if let Ok(mut g) = cb_handle.lock() {
                    g.stage = Some(stage);
                }
            };
            let result =
                grove_core::upgrade::apply(method, &release, &cb).map_err(|e| e.to_string());
            if result.is_ok() {
                crate::telemetry::track(
                    "update_applied",
                    vec![("to_version", release.tag.clone().into())],
                );
            }
            if let Ok(mut g) = handle.lock() {
                g.finished = Some(result);
            }
        });
        Task::none()
    }

    pub(super) fn on_restart_app(&mut self) {
        if let Ok(exe) = std::env::current_exe() {
            // The process exits below either way, so a failed relaunch is the
            // one chance to say anything at all about it.
            if let Err(e) = std::process::Command::new(&exe).spawn() {
                tracing::error!(exe = %exe.display(), error = %e, "failed to relaunch after update");
            }
        }
        self.flush_ui_zoom_save();
        std::process::exit(0);
    }

    pub(super) fn on_open_changelog(&mut self) -> Task<Msg> {
        self.changelog = ChangelogState::Loading;
        self.show_changelog = true;
        // The changelog modal takes over; close the Settings modal behind it.
        self.set_modal(Modal::None);
        self.fetch_changelog_task()
    }

    pub(super) fn on_changelog_loaded(
        &mut self,
        result: Result<Vec<grove_core::upgrade::ReleaseNote>, String>,
    ) {
        self.changelog = match result {
            Ok(notes) => ChangelogState::Loaded(notes),
            Err(e) => ChangelogState::Error(e),
        };
    }

    pub(super) fn on_close_changelog(&mut self) {
        self.show_changelog = false;
        // Return to Settings, where the button lives (mirrors ThemePicker return).
        self.set_modal(Modal::Settings);
    }

    /// The tools shown in the Settings Tools section, in display order.
    /// `Terminal` is omitted — always available, no version.
    const SETTINGS_TOOLS: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::OpenCode];

    /// Mark the Tools rows as detecting (drives the spinner) and dispatch the
    /// off-thread availability + version scan, which posts back
    /// `Msg::ToolVersionsDetected`.
    pub(in crate::gui) fn detect_tools_task(&mut self) -> Task<Msg> {
        self.settings_tools = Self::SETTINGS_TOOLS
            .iter()
            .map(|&agent| ToolStatus {
                agent,
                installed: false,
                version: None,
                detecting: true,
            })
            .collect();
        Task::perform(
            async {
                // `--version` is a short subprocess; running it on the executor
                // keeps the UI thread free even if a binary is slow.
                Self::SETTINGS_TOOLS
                    .iter()
                    .map(|&agent| {
                        let installed = agent.available();
                        let version = if installed { agent.version() } else { None };
                        (
                            agent,
                            ToolStatus {
                                agent,
                                installed,
                                version,
                                detecting: false,
                            },
                        )
                    })
                    .collect::<Vec<_>>()
            },
            Msg::ToolVersionsDetected,
        )
    }

    /// Returns a `check_updates_task` if the 24h periodic update check is due
    /// and no check/apply is already in flight. Shared by the tick handler
    /// and the focus-regained path, since the idle+unfocused window stops
    /// ticking and would otherwise miss a check that came due while away.
    pub(super) fn maybe_check_updates_due(&mut self) -> Option<Task<Msg>> {
        let due = match self.app.store.last_update_check {
            Some(ts) => super::now_unix() - ts >= 24 * 60 * 60,
            None => false, // launch check seeds the timestamp; don't double-fire at boot
        };
        if due && matches!(self.upgrade, UpgradeState::Idle | UpgradeState::UpToDate) {
            Some(self.check_updates_task(false))
        } else {
            None
        }
    }

    /// Set upgrade state to Checking and dispatch an off-thread release fetch,
    /// which posts back `Msg::Upgrade(UpgradeMsg::UpdateCheckResult)`. Mirrors `detect_tools_task`.
    /// `manual` is threaded into the result so the handler can apply the correct
    /// error policy (surface inline vs. fail silently).
    pub(super) fn check_updates_task(&mut self, manual: bool) -> Task<Msg> {
        self.upgrade = UpgradeState::Checking;
        // Mirrors detect_tools_task: short blocking work on the iced/tokio executor.
        Task::perform(
            async move { grove_core::upgrade::latest().map_err(|e| e.to_string()) },
            move |result| Msg::Upgrade(UpgradeMsg::UpdateCheckResult(result, manual)),
        )
    }

    /// Dispatch an off-thread release-notes fetch, posting back `Msg::Upgrade(UpgradeMsg::ChangelogLoaded)`.
    /// Mirrors `check_updates_task`.
    pub(super) fn fetch_changelog_task(&self) -> Task<Msg> {
        // Off-thread, mirroring the update check. 10 most recent releases.
        Task::perform(
            async { grove_core::upgrade::releases(10).map_err(|e| e.to_string()) },
            |v| Msg::Upgrade(UpgradeMsg::ChangelogLoaded(v)),
        )
    }
}
