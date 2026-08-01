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
use gpui::{div, prelude::*, AnyElement, App, Context};
use grove_core::{git, storage};

use crate::settings::SettingsState;
use crate::theme as c;

use super::shell::{
    body_text, click_action, click_checkbox, modal_body, modal_footer_hints, modal_header,
    modal_panel, ModalBtn,
};
use super::{Modal, ModalClick, ModalDispatch, ModalEvent, ModalLayer};
use crate::modal::TeardownStage;

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
        Some(Modal::Teardown { stage, message, .. }) => {
            teardown_modal(layer, *stage, message, dispatch)
        }
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
        let mut list = div()
            .flex()
            .flex_col()
            .gap(rpx(6.0))
            .child(body_text(format!(
                "removing worktrees… {done}/{worktree_count}"
            )))
            .child(
                div()
                    .font(gpui::font(crate::fonts::MONO_FAMILY))
                    .text_size(rpx(11.0))
                    .text_color(c::FG_MUTE())
                    .child(current.to_string()),
            );
        for e in errors {
            list = list.child(
                div()
                    .text_size(rpx(11.0))
                    .text_color(c::RED())
                    .child(e.clone()),
            );
        }
        list
    } else {
        let mut d = div()
            .flex()
            .flex_col()
            .gap(rpx(10.0))
            .child(body_text(format!(
                "Remove '{name}' from Grove? Its sessions will be ended."
            )));
        if worktree_count > 0 {
            d = d.child(click_checkbox(
                "rm-proj-wts",
                format!("Also delete {worktree_count} worktrees on disk"),
                also_remove_worktrees,
                c::RED(),
                true,
                dispatch,
                ModalClick::ToggleRemoveWorktrees,
            ));
        }
        d.child(
            div()
                .flex()
                .gap(rpx(8.0))
                .child(click_action(
                    "rm-proj-yes",
                    "Remove",
                    ModalBtn::Danger,
                    dispatch,
                    ModalClick::RemoveProjectConfirm,
                ))
                .child(click_action(
                    "rm-proj-no",
                    "Cancel",
                    ModalBtn::Plain,
                    dispatch,
                    ModalClick::Cancel,
                )),
        )
    };

    let footer = if in_progress {
        // Cancel is refused while busy — do not offer it.
        modal_footer_hints(&[("", "removing… cannot be cancelled")])
    } else {
        modal_footer_hints(&[
            ("y", "remove"),
            ("esc / n", "cancel"),
            ("space", "toggle worktrees"),
        ])
    };

    modal_panel(
        460.0,
        div()
            .child(modal_header("Remove project", c::RED()))
            .child(modal_body(body))
            .child(footer),
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
    let mut list = div().flex().flex_col().gap(rpx(4.0));
    for (wt, agent, running) in sessions {
        list = list.child(
            div()
                .flex()
                .items_center()
                .gap(rpx(8.0))
                .px(rpx(8.0))
                .py(rpx(4.0))
                .rounded(rpx(4.0))
                .bg(c::BG_HL())
                .child(
                    div()
                        .flex_1()
                        .text_size(rpx(12.0))
                        .text_color(c::FG_DIM())
                        .child(format!("{wt} · {agent}")),
                )
                .child(
                    div()
                        .text_size(rpx(10.0))
                        .text_color(if *running { c::GREEN() } else { c::FG_MUTE() })
                        .child(if *running { "running" } else { "exited" }),
                ),
        );
    }

    let body = if blocked {
        div()
            .flex()
            .flex_col()
            .gap(rpx(10.0))
            .child(body_text(format!(
                "'{name}' still has {} session(s). Archiving would strand them \
                 under a project you can no longer see.",
                sessions.len()
            )))
            .child(list)
            .child(
                div()
                    .flex()
                    .gap(rpx(8.0))
                    .child(click_action(
                        "arch-kill",
                        "Kill all sessions",
                        ModalBtn::Danger,
                        dispatch,
                        ModalClick::ArchiveKillSessions,
                    ))
                    .child(click_action(
                        "arch-cancel",
                        "Cancel",
                        ModalBtn::Plain,
                        dispatch,
                        ModalClick::Cancel,
                    )),
            )
    } else {
        div()
            .flex()
            .flex_col()
            .gap(rpx(12.0))
            .child(body_text(format!(
                "Archive '{name}'? It disappears from the tree; nothing is deleted."
            )))
            .child(
                div()
                    .flex()
                    .gap(rpx(8.0))
                    .child(click_action(
                        "arch-yes",
                        "Archive",
                        ModalBtn::Primary,
                        dispatch,
                        ModalClick::ArchiveConfirm,
                    ))
                    .child(click_action(
                        "arch-no",
                        "Cancel",
                        ModalBtn::Plain,
                        dispatch,
                        ModalClick::Cancel,
                    )),
            )
    };

    modal_panel(
        460.0,
        div()
            .child(modal_header("Archive project", c::AMBER()))
            .child(modal_body(body))
            .child(if blocked {
                modal_footer_hints(&[("esc / n", "cancel")])
            } else {
                modal_footer_hints(&[("y", "archive"), ("esc / n", "cancel")])
            }),
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
    div()
        .id(id)
        .size(rpx(22.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(rpx(4.0))
        .hover(move |s| s.bg(hover_bg))
        .child(crate::icons::icon(icon_name, 12.0, glyph))
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
            dispatch(click.clone(), window, cx);
        })
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
            .py(rpx(30.0))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_size(rpx(12.0))
                    .text_color(c::FG_MUTE())
                    .child("No archived projects."),
            )
            .into_any_element()
    } else {
        let mut list = div()
            .id("archived-projects-list")
            .flex()
            .flex_col()
            .gap(rpx(4.0))
            .max_h(rpx(360.0))
            .overflow_y_scroll();
        for (idx, name, path) in &rows {
            let slot = div()
                .w(rpx(48.0))
                .flex()
                .items_center()
                .gap(rpx(2.0))
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
                    .gap(rpx(10.0))
                    .w_full()
                    .px(rpx(10.0))
                    .py(rpx(6.0))
                    .rounded(rpx(6.0))
                    .hover(|s| s.bg(c::BG_HL()))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_size(rpx(13.0))
                            .text_color(c::FG())
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .font(gpui::font(crate::fonts::MONO_FAMILY))
                            .text_size(rpx(11.0))
                            .text_color(c::FG_MUTE())
                            .child(crate::views::session_header::truncate_middle(path, 44)),
                    )
                    .child(slot),
            );
        }
        list.into_any_element()
    };

    let close_dispatch = dispatch.clone();
    let header = div().w_full().px(rpx(16.0)).py(rpx(14.0)).child(
        div()
            .flex()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .font(gpui::font(crate::fonts::UI_FAMILY))
                    .text_size(rpx(13.0))
                    .text_color(c::CYAN())
                    .child("Archived projects"),
            )
            .child(
                div()
                    .id("arch-list-close")
                    .size(rpx(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(rpx(4.0))
                    .hover(|s| s.bg(c::BG_HOVER()))
                    .child(crate::icons::icon("close", 12.0, c::FG_MUTE()))
                    .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                        close_dispatch(ModalClick::Cancel, window, cx);
                    }),
            ),
    );

    modal_panel(
        520.0,
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
    stage: TeardownStage,
    message: &str,
    dispatch: &ModalDispatch,
) -> AnyElement {
    let (accent, footer): (_, &[(&'static str, &'static str)]) = match stage {
        TeardownStage::RunningScript => (c::CYAN(), &[("esc", "skip the teardown script")]),
        // An in-flight `git worktree remove` cannot be safely interrupted, so
        // there is no key and no button for this stage.
        TeardownStage::Removing => (c::AMBER(), &[("", "removing… cannot be cancelled")]),
        TeardownStage::Done { failed } => (
            if failed { c::RED() } else { c::GREEN() },
            &[("esc / ⏎", "close")],
        ),
    };
    modal_panel(
        520.0,
        div()
            .child(modal_header("Remove worktree", accent))
            .child(modal_body(
                div()
                    .flex()
                    .flex_col()
                    .gap(rpx(10.0))
                    .child(body_text(message.to_string()))
                    // The ONE terminal renderer, reused — there is no second
                    // one (Task 3 Step 4).
                    .children(layer.teardown_view.clone().map(|v| {
                        div()
                            .h(rpx(240.0))
                            .w_full()
                            .rounded(rpx(6.0))
                            .overflow_hidden()
                            .child(v)
                    }))
                    .when(matches!(stage, TeardownStage::Done { .. }), |d| {
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
                            "Skip script",
                            ModalBtn::Plain,
                            dispatch,
                            ModalClick::Cancel,
                        ))
                    }),
            ))
            .child(modal_footer_hints(footer)),
    )
    .into_any_element()
}
