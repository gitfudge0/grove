//! The settings/storage Global.
//!
//! No new storage format and no new file: `grove_core::storage::{load, save,
//! persist}` and `Store` are used exactly as the iced app uses them
//! (`src/app/mod.rs:178`). Only the debounce mechanism differs — the iced app
//! counts 4 × 60ms ticks (`ZOOM_SAVE_QUIET_TICKS`,
//! `src/gui/update/mod.rs:52-56`); spec §4 replaces that with a single 250ms
//! timer shared by every setting, not just zoom.

// `is_dirty`/`flush_now` exist for Plan 09's quit paths; no caller yet.
#![allow(dead_code)]

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
        let epoch = cx.update_global::<Self, _>(|this, _| {
            f(&mut this.store);
            this.dirty = true;
            this.epoch += 1;
            this.epoch
        });
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
    /// this before the process exits (spec §7 `flush_ui_zoom_save`); wiring it
    /// into close-request/quit handling is **Plan 09**.
    pub fn flush_now(cx: &mut gpui::App) {
        cx.update_global::<Self, _>(|this, _| {
            if this.dirty {
                storage::persist(&this.store);
                this.dirty = false;
            }
        });
    }

    /// Whether a write is pending. Exposed for tests and for Plan 09's quit
    /// path to assert against.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
}
