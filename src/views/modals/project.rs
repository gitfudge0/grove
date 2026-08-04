//! RemoveProject (+ its async teardown progress), ArchiveProject,
//! ArchivedProjects and Teardown.
//!
//! Ports of `src/gui/view/modals/confirm.rs:134-314,315-445,473-553` and
//! `archived_projects.rs:23-158`, with the behavior halves from
//! `src/gui/update/modals.rs` (`archive_gate_sessions` :703-720,
//! `open_archive_gate`/`refresh_archive_gate` :721-745, `on_archive`
//! :746-796, `kick_off_remove_project` :797-856, `advance_remove_project`
//! :857-906) and `src/app/teardown.rs:11-40,187-199`.

// The chrome, the input wrapper and the archive/teardown helpers are built
// once here and consumed by Tasks 4-6 of gpui rewrite plan 08.
#![allow(dead_code)]

use crate::views::rpx;
use crate::views::tokens::*;
use gpui::{div, prelude::*, AnyElement, App, Context};
use grove_core::{git, storage};

use crate::settings::SettingsState;
use crate::theme as c;

use super::{Modal, ModalClick, ModalDispatch, ModalEvent, ModalLayer};
use crate::modal::TeardownStage;
use crate::views::components::{
    body_text, click_action, click_checkbox, icon_btn, modal_body, modal_footer_hints,
    modal_header, modal_header_with_close, modal_panel, mono, status_dot, ui, ModalBtn,
};

// ── local layout geometry (§8.4: geometry lives in the owning module) ─────

/// The removal progress bar's girth. The `SPACE_MD` step is the smallest that
/// still reads as a bar rather than a rule.
const PROGRESS_BAR_H: f32 = SPACE_MD;

/// The archived-row action slot: exactly two [`CONTROL_H`] mini buttons plus
/// the `SPACE_XS` gap between them, so every row's name column ends on the
/// same x regardless of which actions are present.
const ROW_ACTION_SLOT_W: f32 = CONTROL_H * 2.0 + SPACE_XS;

/// Vertical breathing room for an inline "nothing here" block inside a modal
/// body — one modal zone padding step above and below.
const EMPTY_STATE_PY: f32 = SPACE_3XL * 2.0;

/// The scrolling archived-project list's ceiling, past which the panel would
/// outgrow a short window.
const LIST_MAX_H: f32 = 360.0;

/// The teardown modal's embedded PTY viewport: tall enough for a script's
/// last ~15 lines without pushing the stage message off a laptop screen.
const TEARDOWN_PTY_H: f32 = 240.0;

impl ModalLayer {
    pub(super) fn slot_mut(&mut self) -> &mut crate::modal::ModalSlot {
        &mut self.slot
    }

    // ── RemoveProject ───────────────────────────────────────────────────

    /// Open the project-removal modal, discovering the project's non-main
    /// worktrees up front so the modal can show "Also delete N worktrees on
    /// disk" without re-shelling-out per frame
    /// (`src/app/teardown.rs:11-40`).
    pub fn open_remove_project(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(p) = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .cloned()
        else {
            return;
        };
        let worktrees: Vec<String> = git::list_worktrees(&p.path)
            .into_iter()
            .filter(|w| !w.is_main)
            .map(|w| w.path)
            .collect();
        self.open(
            Modal::RemoveProject {
                idx,
                name: p.name,
                project_path: p.path,
                worktrees,
                also_remove_worktrees: false,
                in_progress: false,
                done: 0,
                current: String::new(),
                errors: Vec::new(),
            },
            cx,
        );
    }

    /// Begin executing a confirmed removal (`kick_off_remove_project`,
    /// `modals.rs:797-856`). The recursive worktree teardown runs on the
    /// background executor and reports `done`/`current`/`errors` back into
    /// the entity — **not** a tick, and never on the frame thread, so the
    /// window stays responsive while `git worktree remove` blocks.
    pub(super) fn kick_off_remove_project(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::RemoveProject {
            idx,
            name,
            project_path,
            worktrees,
            also_remove_worktrees,
            ..
        }) = self.slot.get()
        else {
            return;
        };
        let (idx, name, project_path) = (*idx, name.clone(), project_path.clone());
        let worktrees = worktrees.clone();
        let also = *also_remove_worktrees;

        // Sessions under the project die with it either way; they must never
        // be left as orphans in the rail.
        self.kill_sessions_for_project(&name, cx);

        if !also || worktrees.is_empty() {
            self.finalize_remove_project(idx, &name, cx);
            return;
        }

        if let Some(Modal::RemoveProject { in_progress, .. }) = self.slot.get_mut() {
            *in_progress = true;
        }
        cx.notify();

        cx.spawn(async move |this, cx| {
            for (i, wt) in worktrees.iter().enumerate() {
                let wt_owned = wt.clone();
                let project_path = project_path.clone();
                // The blocking `git worktree remove` belongs on the background
                // executor; the foreground only ever paints the progress.
                let result = cx
                    .background_executor()
                    .spawn(async move { git::remove_worktree(&project_path, &wt_owned) })
                    .await;
                let wt = wt.clone();
                let ok = this.update(cx, |this: &mut Self, cx| {
                    let Some(Modal::RemoveProject {
                        done,
                        current,
                        errors,
                        ..
                    }) = this.slot.get_mut()
                    else {
                        return false;
                    };
                    *done = i + 1;
                    current.clone_from(&wt);
                    if let Err(e) = result {
                        errors.push(format!("{wt}: {e}"));
                    }
                    cx.notify();
                    true
                });
                if !matches!(ok, Ok(true)) {
                    return;
                }
            }
            let _ = this.update(cx, |this: &mut Self, cx| {
                this.finalize_remove_project(idx, &name, cx);
            });
        })
        .detach();
    }

    /// Drop the project from the store and close the modal.
    fn finalize_remove_project(&mut self, idx: usize, name: &str, cx: &mut Context<Self>) {
        SettingsState::update(cx, move |store| {
            if idx < store.projects.len() {
                store.projects.remove(idx);
            }
        });
        SettingsState::flush_now(cx);
        self.toast
            .update(cx, |t, cx| t.set_toast(format!("removed {name}"), cx));
        self.close(cx);
        cx.emit(ModalEvent::TreeInvalidated);
    }

    /// Kill every session belonging to `project`, by name. One kill path, the
    /// same one the archive gate uses (`App::kill_sessions_for_project`).
    fn kill_sessions_for_project(&mut self, project: &str, cx: &mut Context<Self>) {
        let ids: Vec<_> = self
            .registry
            .read(cx)
            .all()
            .iter()
            .filter(|m| m.project == project)
            .map(|m| m.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        self.registry.update(cx, |r, cx| {
            for id in &ids {
                r.remove(*id);
            }
            cx.notify();
        });
        self.state.update(cx, |s, cx| {
            for id in &ids {
                s.on_session_removed(*id);
            }
            cx.notify();
        });
    }

    // ── ArchiveProject ──────────────────────────────────────────────────

    /// One rendered row per SESSION of `project` — never per worktree, since
    /// sessions are per-worktree and one worktree can hold several.
    ///
    /// Deliberately NOT filtered to live sessions: exited sessions stay in the
    /// registry (that is how the sidebar shows exited rows) and killing the
    /// project's sessions clears them too, so filtering here would make the
    /// gate's count disagree with the killer's. Each row instead carries its
    /// real liveness so the modal can say "running" or "exited" truthfully.
    fn archive_gate_sessions(&self, project: &str, cx: &App) -> Vec<(String, String, bool)> {
        let activity = self.activity.read(cx);
        self.registry
            .read(cx)
            .all()
            .iter()
            .filter(|m| m.project == project)
            .map(|m| {
                let wt = std::path::Path::new(&m.wt_path)
                    .file_name()
                    .map_or_else(|| m.wt_path.clone(), |f| f.to_string_lossy().into_owned());
                let running = activity.state_of(m.id) != crate::activity::ActivityState::Exited;
                (wt, m.agent.label().to_string(), running)
            })
            .collect()
    }

    pub fn open_archive_gate(&mut self, proj: usize, cx: &mut Context<Self>) {
        let Some(name) = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(proj)
            .map(|p| p.name.clone())
        else {
            return;
        };
        let sessions = self.archive_gate_sessions(&name, cx);
        self.open(
            Modal::ArchiveProject {
                idx: proj,
                name,
                sessions,
            },
            cx,
        );
    }

    /// Recompute the gate's session snapshot in place, so the modal re-renders
    /// in the cleared state right after a kill instead of showing a cached
    /// count that no longer matches reality (`refresh_archive_gate`,
    /// `modals.rs:736-745`).
    fn refresh_archive_gate(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::ArchiveProject { name, .. }) = self.slot.get() else {
            return;
        };
        let fresh = self.archive_gate_sessions(&name.clone(), cx);
        if let Some(Modal::ArchiveProject { sessions, .. }) = self.slot.get_mut() {
            *sessions = fresh;
        }
        cx.notify();
    }

    /// The gate's kill button: the one and only kill path — the gate never
    /// grew its own (`on_archive`, `modals.rs:746-796`).
    pub fn archive_kill_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::ArchiveProject { name, .. }) = self.slot.get() else {
            return;
        };
        let name = name.clone();
        self.kill_sessions_for_project(&name, cx);
        self.refresh_archive_gate(cx);
    }

    /// `y` routes through here, which re-checks the blocked precondition, so
    /// it cannot bypass a disabled Archive button (`modals.rs:148-160`).
    pub(super) fn archive_confirm(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::ArchiveProject { idx, name, .. }) = self.slot.get() else {
            return;
        };
        let (idx, name) = (*idx, name.clone());
        // Re-derived, never read off the rendered snapshot.
        if !self.archive_gate_sessions(&name, cx).is_empty() {
            return;
        }
        SettingsState::update(cx, move |store| {
            if let Some(p) = store.projects.get_mut(idx) {
                p.archived = true;
            }
        });
        SettingsState::flush_now(cx);
        self.toast
            .update(cx, |t, cx| t.set_toast(format!("archived {name}"), cx));
        self.close(cx);
        cx.emit(ModalEvent::TreeInvalidated);
    }

    // ── ArchivedProjects ────────────────────────────────────────────────

    pub fn restore_archived(&mut self, idx: usize, cx: &mut Context<Self>) {
        SettingsState::update(cx, move |store| {
            if let Some(p) = store.projects.get_mut(idx) {
                p.archived = false;
            }
        });
        SettingsState::flush_now(cx);
        cx.emit(ModalEvent::TreeInvalidated);
        cx.notify();
    }

    pub fn delete_archived(&mut self, idx: usize, cx: &mut Context<Self>) {
        SettingsState::update(cx, move |store| {
            if idx < store.projects.len() {
                store.projects.remove(idx);
            }
        });
        SettingsState::flush_now(cx);
        cx.emit(ModalEvent::TreeInvalidated);
        cx.notify();
    }

    // ── Teardown ────────────────────────────────────────────────────────

    /// Begin tearing down `path` (`src/app/teardown.rs:187-199`): kill its
    /// sessions, then either run the project's teardown script in a
    /// modal-embedded PTY (advancing to removal when it exits) or remove the
    /// worktree immediately.
    pub fn start_teardown(&mut self, p: &storage::Project, path: String, cx: &mut Context<Self>) {
        self.kill_sessions_for_wt(&path, cx);
        let script = p
            .scripts
            .teardown
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        self.teardown_view = None;
        self.teardown_session = None;
        self.teardown_poll = None;

        let Some(script) = script else {
            self.open(
                Modal::Teardown {
                    wt_path: path.clone(),
                    project_path: p.path.clone(),
                    stage: TeardownStage::Removing,
                    message: "removing worktree…".into(),
                    removal_started: true,
                },
                cx,
            );
            self.run_worktree_removal(p.path.clone(), path, cx);
            return;
        };

        self.open(
            Modal::Teardown {
                wt_path: path.clone(),
                project_path: p.path.clone(),
                stage: TeardownStage::RunningScript,
                message: "running teardown script…".into(),
                removal_started: false,
            },
            cx,
        );

        // Modal-owned, never registered: a teardown PTY must not appear in the
        // rail, exactly as iced keeps it out of `app.sessions`.
        let session = cx.new(|cx| {
            crate::entities::terminal_session::TerminalSession::spawn_script(&script, &path, cx)
        });
        let clock = self.clock.clone();
        let view = cx.new({
            let session = session.clone();
            |cx| crate::views::terminal_view::TerminalView::new(session, None, clock, cx)
        });
        self.teardown_session = Some(session.clone());
        self.teardown_view = Some(view);

        // The script's exit is what advances the stage. Poll rather than wait
        // on the reader: EOF on the PTY and process reap are not the same
        // event, and `alive()` is the same latch the rest of the app uses.
        self.teardown_poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(120))
                .await;
            if session.update(cx, |s, _| s.alive()) {
                continue;
            }
            let _ = this.update(cx, |this: &mut Self, cx| this.advance_teardown(cx));
            return;
        }));
    }

    /// The script finished (or Escape skipped it): drop the PTY, paint a
    /// `Removing` frame, then run the blocking removal off the frame thread.
    fn advance_teardown(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::Teardown {
            wt_path,
            project_path,
            stage,
            message,
            removal_started,
        }) = self.slot.get_mut()
        else {
            return;
        };
        if *removal_started {
            return;
        }
        *stage = TeardownStage::Removing;
        *message = "removing worktree…".into();
        // Set BEFORE the removal is kicked off so a `Removing` frame paints
        // first (`src/app/modal.rs:171-174`). In gpui the removal is on the
        // background executor, which makes this a paint-ordering detail
        // rather than a hack — kept so the stage sequence stays observable.
        *removal_started = true;
        let (wt, project_path) = (wt_path.clone(), project_path.clone());
        self.teardown_poll = None;
        self.teardown_view = None;
        self.teardown_session = None;
        cx.notify();
        self.run_worktree_removal(project_path, wt, cx);
    }

    fn run_worktree_removal(&mut self, project_path: String, wt: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let failed = cx
                .background_executor()
                .spawn(async move { git::remove_worktree(&project_path, &wt) })
                .await
                .is_err();
            let _ = this.update(cx, |this: &mut Self, cx| {
                if let Some(Modal::Teardown { stage, message, .. }) = this.slot.get_mut() {
                    *stage = TeardownStage::Done { failed };
                    *message = if failed {
                        "worktree removal failed".into()
                    } else {
                        "worktree removed".into()
                    };
                }
                cx.emit(ModalEvent::TreeInvalidated);
                cx.notify();
            });
        })
        .detach();
    }

    /// Escape during `RunningScript` means "skip the script and proceed to
    /// removal" — never "close" (`cancel_modal`, `modals.rs:677-702`).
    pub(super) fn skip_teardown_script(&mut self, cx: &mut Context<Self>) {
        self.advance_teardown(cx);
    }

    fn kill_sessions_for_wt(&mut self, wt_path: &str, cx: &mut Context<Self>) {
        let ids: Vec<_> = self
            .registry
            .read(cx)
            .all()
            .iter()
            .filter(|m| m.wt_path == wt_path)
            .map(|m| m.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        self.registry.update(cx, |r, cx| {
            for id in &ids {
                r.remove(*id);
            }
            cx.notify();
        });
        self.state.update(cx, |s, cx| {
            for id in &ids {
                s.on_session_removed(*id);
            }
            cx.notify();
        });
    }
}

// ── the views ────────────────────────────────────────────────────────────

pub fn render(layer: &ModalLayer, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    match layer.slot().get() {
        Some(Modal::RemoveProject {
            name,
            worktrees,
            also_remove_worktrees,
            in_progress,
            done,
            current,
            errors,
            ..
        }) => remove_project_modal(
            name,
            worktrees.len(),
            *also_remove_worktrees,
            &RemoveProgress {
                in_progress: *in_progress,
                done: *done,
                current,
                errors,
            },
            dispatch,
        ),
        Some(Modal::ArchiveProject { name, sessions, .. }) => {
            archive_project_modal(name, sessions, dispatch)
        }
        Some(Modal::ArchivedProjects) => archived_projects_modal(dispatch, cx),
        Some(Modal::Teardown {
            wt_path,
            stage,
            message,
            ..
        }) => teardown_modal(layer, wt_path, *stage, message, dispatch),
        _ => div().into_any_element(),
    }
}

/// `remove_project_modal` (`view/modals/confirm.rs:315-445`): two stages in
/// one variant — the confirm with its checkbox, then the progress view.
/// The progress half of the two-stage variant, bundled so the view keeps one
/// argument per concept.
struct RemoveProgress<'a> {
    in_progress: bool,
    done: usize,
    current: &'a str,
    errors: &'a [String],
}

fn remove_project_modal(
    name: &str,
    worktree_count: usize,
    also_remove_worktrees: bool,
    progress: &RemoveProgress<'_>,
    dispatch: &ModalDispatch,
) -> AnyElement {
    let RemoveProgress {
        in_progress,
        done,
        current,
        errors,
    } = *progress;
    let body = if in_progress {
        let status = if done >= worktree_count {
            "Finishing…".to_string()
        } else {
            format!("Removing {} of {}: {current}", done + 1, worktree_count)
        };
        let mut list =
            div()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_MD))
                .child(ui(status, TEXT_SMALL, c::FG_MUTE()));
        list = list.child(progress_bar(if worktree_count == 0 {
            1.0
        } else {
            (done as f32 / worktree_count as f32).clamp(0.0, 1.0)
        }));
        if !errors.is_empty() {
            list = list.child(ui(
                format!("{} worktree(s) failed to remove", errors.len()),
                TEXT_SMALL,
                c::RED(),
            ));
        }
        list
    } else {
        let mut d = div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_XL))
            .child(ui(
                format!("'{name}' will be unregistered from Grove. Its sessions will be ended."),
                TEXT_TITLE,
                c::FG_DIM(),
            ))
            .child(ui(
                "Running sessions for this project will be stopped.",
                TEXT_BODY,
                c::FG_MUTE(),
            ));
        if worktree_count > 0 {
            let label = if worktree_count == 1 {
                "Delete 1 non-main worktree from disk".to_string()
            } else {
                format!("Delete {worktree_count} non-main worktrees from disk")
            };
            d = d.child(click_checkbox(
                "rm-proj-wts",
                label,
                also_remove_worktrees,
                c::RED(),
                true,
                dispatch,
                ModalClick::ToggleRemoveWorktrees,
            ));
        }
        if !errors.is_empty() {
            d = d.child(ui(
                format!("{} worktree(s) failed to remove", errors.len()),
                TEXT_SMALL,
                c::RED(),
            ));
        }
        d.child(
            div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_LG))
                .child(div().flex_1())
                .child(click_action(
                    "rm-proj-no",
                    "Cancel",
                    ModalBtn::Plain,
                    dispatch,
                    ModalClick::Cancel,
                ))
                .child(click_action(
                    "rm-proj-yes",
                    "Remove",
                    ModalBtn::Danger,
                    dispatch,
                    ModalClick::RemoveProjectConfirm,
                )),
        )
    };

    let mut panel = div()
        .child(modal_header("Remove project", c::RED()))
        .child(modal_body(body));
    if !in_progress {
        // Cancel is refused while busy — no footer, no key, is offered then.
        panel = panel.child(modal_footer_hints(&[
            ("y", "remove"),
            ("space", "toggle delete"),
            ("esc", "cancel"),
        ]));
    }

    // MODAL_W_LG, not the default MODAL_W_MD: the removal-progress body prints
    // an untruncated worktree name above the progress bar, and at 480 a long
    // name wraps and changes the dialog's height mid-removal (§2.4).
    modal_panel(MODAL_W_LG, panel).into_any_element()
}

/// The removal progress bar: [`PROGRESS_BAR_H`] girth, `BG_STRIP` track, `RED`
/// fill, `BORDER` border, radius 4 (`confirm.rs:409-419`). gpui has no
/// `progress_bar` primitive, so the fill is a plain scaled child div.
fn progress_bar(frac: f32) -> AnyElement {
    div()
        .w_full()
        .h(rpx(PROGRESS_BAR_H))
        .rounded(rpx(RADIUS_CONTROL))
        .border_1()
        .border_color(c::BORDER())
        .bg(c::BG_STRIP())
        .child(
            div()
                .h_full()
                .rounded(rpx(RADIUS_CONTROL))
                .bg(c::RED())
                .w(gpui::relative(frac.clamp(0.0, 1.0))),
        )
        .into_any_element()
}

/// `archive_project_modal` (`view/modals/confirm.rs:134-314`): one row per
/// **session**, unfiltered by liveness, honestly labelled.
fn archive_project_modal(
    name: &str,
    sessions: &[(String, String, bool)],
    dispatch: &ModalDispatch,
) -> AnyElement {
    let blocked = !sessions.is_empty();

    let body = if blocked {
        let mut strip = div().flex().flex_col().gap(rpx(SPACE_SM));
        for (wt, agent, running) in sessions {
            strip = strip.child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .child(status_dot(
                        DOT_SM,
                        if *running { c::GREEN() } else { c::FG_MUTE() },
                    ))
                    .child(mono(wt.clone(), TEXT_SMALL, c::FG()))
                    .child(ui(agent.clone(), TEXT_SMALL, c::FG_DIM()))
                    .child(div().flex_1())
                    .child(ui(
                        if *running { "running" } else { "exited" },
                        TEXT_SMALL,
                        c::FG_MUTE(),
                    )),
            );
        }
        strip = strip.child(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .pt(rpx(SPACE_SM))
                .child(click_action(
                    "arch-kill",
                    format!("Kill all sessions ({})", sessions.len()),
                    ModalBtn::Danger,
                    dispatch,
                    ModalClick::ArchiveKillSessions,
                )),
        );
        let strip = div()
            .w_full()
            .px(rpx(SPACE_XL))
            .py(rpx(SPACE_MD))
            .rounded(rpx(RADIUS_GROUP))
            .bg(c::BG())
            .border_1()
            .border_color(c::BORDER_SOFT())
            .child(strip);

        div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_XL))
            .child(body_text(format!(
                "'{name}' still has open sessions. Kill them before archiving."
            )))
            .child(strip)
            .child(ui(
                "Nothing on disk is deleted. Worktrees stay exactly as they are.",
                TEXT_SMALL,
                c::FG_MUTE(),
            ))
    } else {
        div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL))
            .child(body_text(format!(
                "'{name}' will be hidden from the sidebar. Nothing is deleted — its scripts, \
             theme, and worktrees all stay exactly as they are."
            )))
    };

    let archive_btn: AnyElement = if blocked {
        div()
            .px(rpx(SPACE_2XL))
            .py(rpx(SPACE_MD))
            .rounded(rpx(RADIUS_CONTROL))
            .border_1()
            .border_color(c::BORDER_SOFT())
            .child(ui("Archive", TEXT_BODY, c::FG_MUTE()))
            .into_any_element()
    } else {
        click_action(
            "arch-yes",
            "Archive",
            ModalBtn::Primary,
            dispatch,
            ModalClick::ArchiveConfirm,
        )
        .into_any_element()
    };

    let mut footer_row = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_3XL))
        .child(crate::views::components::footer_hint("y", "archive"))
        .child(crate::views::components::footer_hint("n", "cancel"))
        .child(div().flex_1());
    if blocked {
        footer_row = footer_row.child(ui(
            "Archive is unavailable while sessions are running.",
            TEXT_SMALL,
            c::FG_MUTE(),
        ));
    }
    let footer = crate::views::components::modal_footer_row(
        footer_row
            .child(click_action(
                "arch-no",
                "Cancel",
                ModalBtn::Plain,
                dispatch,
                ModalClick::Cancel,
            ))
            .child(archive_btn),
    );

    modal_panel(
        MODAL_W_SM,
        div()
            .child(modal_header(format!("Archive '{name}'?"), c::AMBER()))
            .child(modal_body(body))
            .child(footer),
    )
    .into_any_element()
}

/// A 22×22 row action mini-button. `glyph`/`hover_bg` distinguish restore
/// (cyan on the neutral hover fill) from delete (red on a red wash) — the
/// same idiom as `iced`'s `mini` (`archived_projects.rs:18-40`).
fn archive_mini(
    id: &'static str,
    icon_name: &'static str,
    glyph: gpui::Hsla,
    hover_bg: gpui::Hsla,
    dispatch: &ModalDispatch,
    click: ModalClick,
) -> AnyElement {
    let dispatch = dispatch.clone();
    icon_btn(
        id,
        icon_name,
        CONTROL_H,
        CONTROL_H,
        ICON_SM,
        glyph,
        hover_bg,
        None,
        false,
        move |window, cx| dispatch(click.clone(), window, cx),
    )
    .into_any_element()
}

/// `archived_projects_modal` (`view/modals/archived_projects.rs:23-158`): a
/// marker modal whose every row derives live from `store.archived_projects()`.
fn archived_projects_modal(dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    let store = &cx.global::<SettingsState>().store;
    let rows: Vec<(usize, String, String)> = store
        .archived_projects()
        .map(|(i, p)| (i, p.name.clone(), p.path.clone()))
        .collect();

    let body: AnyElement = if rows.is_empty() {
        div()
            .w_full()
            .py(rpx(EMPTY_STATE_PY))
            .flex()
            .items_center()
            .justify_center()
            .child(ui("No archived projects.", TEXT_BODY, c::FG_MUTE()))
            .into_any_element()
    } else {
        let mut list = div()
            .id("archived-projects-list")
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM))
            .max_h(rpx(LIST_MAX_H))
            .overflow_y_scroll();
        for (idx, name, path) in &rows {
            let slot = div()
                .w(rpx(ROW_ACTION_SLOT_W))
                .flex()
                .items_center()
                .gap(rpx(SPACE_XS))
                .child(archive_mini(
                    "arch-restore",
                    "restore",
                    c::CYAN(),
                    c::BG_HOVER(),
                    dispatch,
                    ModalClick::RestoreArchived(*idx),
                ))
                .child(archive_mini(
                    "arch-delete",
                    "trash",
                    c::RED(),
                    c::RED_WASH(),
                    dispatch,
                    ModalClick::DeleteArchived(*idx),
                ));

            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_XL))
                    .w_full()
                    .px(rpx(SPACE_XL))
                    .py(rpx(SPACE_MD))
                    .rounded(rpx(RADIUS_GROUP))
                    .hover(|s| s.bg(c::BG_HL()))
                    .child(
                        ui(name.clone(), TEXT_TITLE, c::FG())
                            .flex_1()
                            .overflow_hidden(),
                    )
                    .child(
                        mono(
                            crate::views::session_header::truncate_middle(path, 44),
                            TEXT_SMALL,
                            c::FG_MUTE(),
                        )
                        .flex_1()
                        .overflow_hidden(),
                    )
                    .child(slot),
            );
        }
        list.into_any_element()
    };

    let header = modal_header_with_close(
        "arch-list-close",
        "Archived projects",
        c::MAGENTA(),
        dispatch,
    );

    modal_panel(
        MODAL_W_LG,
        div()
            .child(header)
            .child(modal_body(body))
            .child(modal_footer_hints(&[("esc", "close")])),
    )
    .into_any_element()
}

/// `teardown_modal` (`view/modals/confirm.rs:473-553`). The embedded live PTY
/// is Task 4's remaining half; the stage machine is complete.
fn teardown_modal(
    layer: &ModalLayer,
    wt_path: &str,
    stage: TeardownStage,
    message: &str,
    dispatch: &ModalDispatch,
) -> AnyElement {
    let done = matches!(stage, TeardownStage::Done { .. });
    let wt_name = std::path::Path::new(wt_path)
        .file_name()
        .map_or_else(|| wt_path.to_string(), |f| f.to_string_lossy().into_owned());

    let mut body = div()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_XL))
        // The ONE terminal renderer, reused — there is no second one (Task 3
        // Step 4).
        .children(layer.teardown_view.clone().map(|v| {
            div()
                .h(rpx(TEARDOWN_PTY_H))
                .w_full()
                .rounded(rpx(RADIUS_GROUP))
                .overflow_hidden()
                .child(v)
        }))
        .child(ui(
            message.to_string(),
            TEXT_TITLE,
            if done { c::FG_DIM() } else { c::FG_MUTE() },
        ));
    body = body
        .when(done, |d| {
            d.child(click_action(
                "td-close",
                "Close",
                ModalBtn::Primary,
                dispatch,
                ModalClick::Cancel,
            ))
        })
        .when(matches!(stage, TeardownStage::RunningScript), |d| {
            d.child(click_action(
                "td-skip",
                "Skip & remove",
                ModalBtn::Plain,
                dispatch,
                ModalClick::Cancel,
            ))
        });
    // Mid-removal (`Removing`): an in-flight `git worktree remove` cannot be
    // safely interrupted, so there is genuinely no button and no footer for
    // this stage — not even a disabled hint.
    let footer = match stage {
        TeardownStage::Done { .. } => Some(modal_footer_hints(&[("esc", "close")])),
        TeardownStage::RunningScript => Some(modal_footer_hints(&[("esc", "skip & remove")])),
        TeardownStage::Removing => None,
    };

    let mut panel = div()
        .child(modal_header(
            format!("Delete worktree / {wt_name}"),
            c::RED(),
        ))
        .child(modal_body(body));
    if let Some(footer) = footer {
        panel = panel.child(footer);
    }

    modal_panel(MODAL_W_LG, panel).into_any_element()
}
