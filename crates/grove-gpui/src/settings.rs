//! The settings/storage Global.
//!
//! No new storage format and no new file: `grove_core::storage::{load, save,
//! persist}` and `Store` are used exactly as the iced app uses them
//! (`src/app/mod.rs:178`). Only the debounce mechanism differs — the iced app
//! counts 4 × 60ms ticks (`ZOOM_SAVE_QUIET_TICKS`,
//! `src/gui/update/mod.rs:52-56`); spec §4 replaces that with a single 250ms
//! timer shared by every setting, not just zoom.

use std::time::Duration;

use gpui::{BorrowAppContext as _, Task};
use grove_core::storage::{self, Store};

/// Quiet period a mutation must survive before it is written to disk. Long
/// enough to outlast a wheel/keyboard burst, short enough that a crash right
/// after a deliberate change rarely loses it.
pub const PERSIST_DEBOUNCE: Duration = Duration::from_millis(250);

pub struct SettingsState {
    pub store: Store,
    dirty: bool,
    /// Bumped by every `update`. A pending flush that wakes up to find a newer
    /// epoch has been superseded and does nothing, so a burst of mutations
    /// costs exactly one write.
    epoch: u64,
    /// Dropping a `Task` cancels it, so the previous timer dies the moment
    /// this is overwritten — that is the debounce's re-arm.
    flush: Option<Task<()>>,
}

impl gpui::Global for SettingsState {}

impl SettingsState {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            dirty: false,
            epoch: 0,
            flush: None,
        }
    }

    /// The only way to mutate persisted settings: applies `f`, marks dirty,
    /// and (re)arms the debounced flush.
    pub fn update(cx: &mut gpui::App, f: impl FnOnce(&mut Store)) {
        let epoch = cx.update_global::<Self, _>(|this, _| this.mark(f));
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(PERSIST_DEBOUNCE).await;
            cx.update(|cx| {
                if cx.global::<Self>().epoch == epoch {
                    Self::flush_now(cx);
                }
            });
        });
        cx.update_global::<Self, _>(|this, _| this.flush = Some(task));
    }

    /// Synchronous write of any pending change. Every quit path must call
    /// this before the process exits (spec §7 `flush_ui_zoom_save`) — and does,
    /// through the single `Workspace::shutdown`.
    pub fn flush_now(cx: &mut gpui::App) {
        cx.update_global::<Self, _>(|this, _| this.persist_if_dirty());
    }

    /// The mutating half of [`Self::update`], factored out so the debounce's
    /// bookkeeping is testable without a gpui `App`.
    fn mark(&mut self, f: impl FnOnce(&mut Store)) -> u64 {
        f(&mut self.store);
        self.dirty = true;
        self.epoch += 1;
        self.epoch
    }

    /// The write itself. Returns whether anything was written, which is what
    /// makes the quit path's idempotence assertable: the second call in a row
    /// writes nothing.
    fn persist_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        storage::persist(&self.store);
        self.dirty = false;
        true
    }

    /// Whether a write is pending. Exposed for tests and for the quit path to
    /// assert against.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Whether the stored telemetry preference is on. **The default is `true`**
    /// (`src/app/mod.rs:331-333`); this is the single accessor so no call site
    /// can re-introduce the `unwrap_or(false)` parity bug this replaced.
    pub fn telemetry_enabled(store: &Store) -> bool {
        store.telemetry_enabled.unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Points `grove_core::storage` at a private directory for this process
    /// so the tests below never touch the developer's real config.
    /// The returned guard also serializes the disk tests against each other:
    /// they share one config file, so they must not interleave.
    fn isolate_config_dir() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, Once, OnceLock};
        static ONCE: Once = Once::new();
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        ONCE.call_once(|| {
            let dir =
                std::env::temp_dir().join(format!("grove-gpui-settings-{}", std::process::id()));
            let _ = fs_err::create_dir_all(&dir);
            std::env::set_var("GROVE_CONFIG_DIR", &dir);
        });
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Task 2 Step 4: the whole point of `flush_ui_zoom_save`. A setting
    /// mutated inside the 250ms debounce window, with no timer ever allowed to
    /// fire, must still be on disk after the quit path runs.
    #[test]
    fn a_quit_inside_the_debounce_window_still_persists() {
        let _guard = isolate_config_dir();
        let mut settings = SettingsState::new(Store::default());
        settings.mark(|s| s.ui_zoom = Some(1.7));
        assert!(settings.is_dirty());
        // No `PERSIST_DEBOUNCE` wait, no timer: straight to the quit path.
        assert!(settings.persist_if_dirty());
        let reloaded = storage::load().unwrap_or_default();
        assert_eq!(reloaded.ui_zoom, Some(1.7));
    }

    /// Task 2 Step 1: `shutdown` is idempotent because its flush is — calling
    /// it twice writes once.
    #[test]
    fn flushing_twice_writes_once() {
        let _guard = isolate_config_dir();
        let mut settings = SettingsState::new(Store::default());
        settings.mark(|s| s.sidebar_width = Some(321.0));
        assert!(settings.persist_if_dirty());
        assert!(!settings.is_dirty());
        assert!(!settings.persist_if_dirty());
    }

    #[test]
    fn telemetry_defaults_to_on() {
        let mut store = Store::default();
        assert!(SettingsState::telemetry_enabled(&store));
        store.telemetry_enabled = Some(false);
        assert!(!SettingsState::telemetry_enabled(&store));
        store.telemetry_enabled = Some(true);
        assert!(SettingsState::telemetry_enabled(&store));
    }
}
