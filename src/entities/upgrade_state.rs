//! The upgrade machine, as pure functions.
//!
//! Every decision the iced orchestration layer makes in `src/gui/update/upgrade.rs`
//! lives here without a network call, a timer or a gpui type in sight: when a
//! check is due, whether one may start, what an answer means, and how apply
//! progress folds into the state.
//!
//! The network itself is `grove_core::upgrade` — this module owns *policy*,
//! never transport, and never re-implements semver comparison
//! (`grove_core::upgrade::update_available` decides Available vs UpToDate).

use grove_core::upgrade::{update_available, Release, ReleaseNote, Stage};

/// Drives the Updates UI and the cog's dot (`src/gui/state.rs:447-458`).
#[derive(Debug, Clone)]
pub enum UpgradeState {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Error(String),
    Updating(Stage),
    Updated,
    UpdateFailed(String),
}

/// Drives the changelog modal (`src/gui/state.rs:439-446`).
#[derive(Debug, Clone)]
pub enum ChangelogState {
    Idle,
    Loading,
    Loaded(Vec<ReleaseNote>),
    Error(String),
}

/// The periodic check's cadence (`src/gui/update/upgrade.rs:199`).
pub const CHECK_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// Whether the 24h periodic check is due. `None` is **not** due: the 3s launch
/// check seeds the timestamp, and firing here as well would double-check at
/// boot (`src/gui/update/upgrade.rs:197-207`).
#[must_use]
pub fn check_due(last_update_check: Option<i64>, now: i64, state: &UpgradeState) -> bool {
    let due = match last_update_check {
        Some(ts) => now.saturating_sub(ts) >= CHECK_INTERVAL_SECS,
        None => false,
    };
    due && matches!(state, UpgradeState::Idle | UpgradeState::UpToDate)
}

/// The in-flight guard: a second check while one is running is refused
/// (`src/gui/update/upgrade.rs:26-32`). All three triggers route through this,
/// so a duplicate is impossible by construction.
#[must_use]
pub fn begin_check(state: &UpgradeState) -> bool {
    !matches!(state, UpgradeState::Checking)
}

/// Fold a check's answer into a state. `manual` selects the error policy:
/// a manual check surfaces the error inline, a launch/periodic one falls back
/// to `Idle` with no badge and no modal (`:52-61`).
#[must_use]
pub fn apply_check_result(
    result: Result<Release, String>,
    manual: bool,
    current: &str,
    skipped: Option<&str>,
) -> UpgradeState {
    match result {
        Ok(release) => {
            if update_available(current, &release, skipped) {
                UpgradeState::Available(release)
            } else {
                UpgradeState::UpToDate
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "update check failed");
            if manual {
                UpgradeState::Error(e)
            } else {
                UpgradeState::Idle
            }
        }
    }
}

/// Skip the offered release: the tag to persist into `Store::skipped_version`
/// (`None` when nothing is on offer) and the state to land on (`:65-75`).
#[must_use]
pub fn skip_version(state: &UpgradeState) -> (Option<String>, UpgradeState) {
    match state {
        UpgradeState::Available(r) => (Some(r.tag.clone()), UpgradeState::UpToDate),
        other => (None, other.clone()),
    }
}

/// A progress stage from the apply callback. Only an apply that is still
/// running accepts one — a stage that arrives *after* the finish must not
/// resurrect `Updating` (`src/gui/update/tick.rs:87-95`, which reads both
/// fields under one lock and is ordering-safe by construction; the channel
/// port has to be too).
#[must_use]
pub fn apply_progress(state: &UpgradeState, stage: Stage) -> UpgradeState {
    match state {
        UpgradeState::Updating(_) => UpgradeState::Updating(stage),
        other => other.clone(),
    }
}

/// The apply's terminal answer, accepted exactly once.
#[must_use]
pub fn apply_finished(state: &UpgradeState, result: Result<(), String>) -> UpgradeState {
    match state {
        UpgradeState::Updating(_) => match result {
            Ok(()) => UpgradeState::Updated,
            Err(e) => UpgradeState::UpdateFailed(e),
        },
        other => other.clone(),
    }
}

/// The cog's green dot (`src/gui/view/appbar.rs:29`): an offered release, and
/// nothing else — not a check in flight, not a failed apply.
#[must_use]
pub fn upgrade_available(state: &UpgradeState) -> bool {
    matches!(state, UpgradeState::Available(_))
}

/// Whether Escape may dismiss the Updating modal. Refused while an apply is in
/// flight; this is the `KeyCtx::update_in_flight` input the keyboard matrix
/// already asserts against (`src/keyboard_matrix.rs:502-519`).
#[must_use]
pub fn escape_closes(state: &UpgradeState) -> bool {
    !matches!(state, UpgradeState::Updating(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = CHECK_INTERVAL_SECS;

    fn release(tag: &str) -> Release {
        let Ok(version) = semver::Version::parse(tag.trim_start_matches('v')) else {
            unreachable!()
        };
        Release {
            version,
            tag: tag.to_string(),
            html_url: format!("https://example.invalid/{tag}"),
            body: String::new(),
            dmg_url: None,
            dmg_sha256_url: None,
            target_commitish: "main".to_string(),
        }
    }

    #[test]
    fn none_is_never_due() {
        assert!(!check_due(None, DAY * 10, &UpgradeState::Idle));
    }

    #[test]
    fn due_after_24h_only_when_idle_or_up_to_date() {
        assert!(check_due(Some(0), DAY, &UpgradeState::Idle));
        assert!(check_due(Some(0), DAY, &UpgradeState::UpToDate));
        assert!(!check_due(Some(0), DAY - 1, &UpgradeState::Idle));
        assert!(!check_due(Some(0), DAY, &UpgradeState::Checking));
        assert!(!check_due(
            Some(0),
            DAY,
            &UpgradeState::Available(release("v9.9.9"))
        ));
        assert!(!check_due(
            Some(0),
            DAY,
            &UpgradeState::Updating(Stage::Building)
        ));
    }

    #[test]
    fn a_second_check_while_checking_is_refused() {
        assert!(begin_check(&UpgradeState::Idle));
        assert!(begin_check(&UpgradeState::UpToDate));
        assert!(begin_check(&UpgradeState::Error("x".into())));
        assert!(!begin_check(&UpgradeState::Checking));
    }

    #[test]
    fn a_newer_release_is_available_and_an_older_one_is_not() {
        let newer = apply_check_result(Ok(release("v99.0.0")), false, "1.0.0", None);
        assert!(matches!(newer, UpgradeState::Available(_)));
        let older = apply_check_result(Ok(release("v0.0.1")), false, "1.0.0", None);
        assert!(matches!(older, UpgradeState::UpToDate));
    }

    #[test]
    fn a_skipped_tag_is_not_offered_but_a_newer_one_is() {
        let skipped = apply_check_result(Ok(release("v99.0.0")), false, "1.0.0", Some("v99.0.0"));
        assert!(matches!(skipped, UpgradeState::UpToDate));
        let newer = apply_check_result(Ok(release("v99.0.1")), false, "1.0.0", Some("v99.0.0"));
        assert!(matches!(newer, UpgradeState::Available(_)));
    }

    #[test]
    fn manual_checks_surface_errors_and_silent_ones_do_not() {
        let manual = apply_check_result(Err("no network".into()), true, "1.0.0", None);
        let UpgradeState::Error(e) = manual else {
            unreachable!()
        };
        assert_eq!(e, "no network");
        let silent = apply_check_result(Err("no network".into()), false, "1.0.0", None);
        assert!(matches!(silent, UpgradeState::Idle));
    }

    #[test]
    fn skipping_records_the_tag_and_lands_on_up_to_date() {
        let (tag, state) = skip_version(&UpgradeState::Available(release("v2.0.0")));
        assert_eq!(tag.as_deref(), Some("v2.0.0"));
        assert!(matches!(state, UpgradeState::UpToDate));
    }

    #[test]
    fn skipping_nothing_records_nothing_and_changes_nothing() {
        let (tag, state) = skip_version(&UpgradeState::UpToDate);
        assert_eq!(tag, None);
        assert!(matches!(state, UpgradeState::UpToDate));
    }

    #[test]
    fn stages_arrive_in_order() {
        let mut s = UpgradeState::Updating(Stage::Downloading);
        for stage in [Stage::Downloading, Stage::Building, Stage::Installing] {
            s = apply_progress(&s, stage);
            let UpgradeState::Updating(got) = s else {
                unreachable!()
            };
            assert_eq!(got, stage);
            s = UpgradeState::Updating(got);
        }
    }

    #[test]
    fn a_finish_lands_exactly_once_and_a_late_stage_cannot_resurrect_updating() {
        let running = UpgradeState::Updating(Stage::Installing);
        let done = apply_finished(&running, Ok(()));
        assert!(matches!(done, UpgradeState::Updated));
        // A stage that raced the finish is dropped.
        assert!(matches!(
            apply_progress(&done, Stage::Building),
            UpgradeState::Updated
        ));
        // A second finish is dropped too.
        assert!(matches!(
            apply_finished(&done, Err("late".into())),
            UpgradeState::Updated
        ));

        let failed = apply_finished(&running, Err("build failed".into()));
        let UpgradeState::UpdateFailed(e) = &failed else {
            unreachable!()
        };
        assert_eq!(e, "build failed");
        assert!(matches!(
            apply_progress(&failed, Stage::Done),
            UpgradeState::UpdateFailed(_)
        ));
    }

    #[test]
    fn the_cog_dot_lights_only_for_an_offered_release() {
        assert!(upgrade_available(&UpgradeState::Available(release(
            "v2.0.0"
        ))));
        for s in [
            UpgradeState::Idle,
            UpgradeState::Checking,
            UpgradeState::UpToDate,
            UpgradeState::Error("x".into()),
            UpgradeState::Updating(Stage::Building),
            UpgradeState::Updated,
            UpgradeState::UpdateFailed("x".into()),
        ] {
            assert!(!upgrade_available(&s), "{s:?}");
        }
    }

    #[test]
    fn escape_is_refused_only_mid_update() {
        assert!(!escape_closes(&UpgradeState::Updating(Stage::Downloading)));
        for s in [
            UpgradeState::Idle,
            UpgradeState::Checking,
            UpgradeState::UpToDate,
            UpgradeState::Available(release("v2.0.0")),
            UpgradeState::Error("x".into()),
            UpgradeState::Updated,
            UpgradeState::UpdateFailed("x".into()),
        ] {
            assert!(escape_closes(&s), "{s:?}");
        }
    }
}
