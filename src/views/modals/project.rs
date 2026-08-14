//! RemoveProject (+ its async teardown progress), ArchiveProject, ArchivedProjects and Teardown.
//! Ports `src/gui/view/modals/confirm.rs`, `archived_projects.rs`, `src/gui/update/modals.rs` and `src/app/teardown.rs`.

use crate::views::rpx;
use crate::views::tokens::*;
use gpui::{div, prelude::*, AnyElement, App, Context};
use grove_core::{git, storage};

use crate::settings::SettingsState;
use crate::theme as c;

use super::{Modal, ModalClick, ModalDispatch, ModalEvent, ModalLayer};
use crate::modal::TeardownStage;
use crate::views::components::{
    body_action, body_text, card, click_action, click_action_enabled, click_checkbox, divider_h,
    icon_btn, modal_body, modal_footer, modal_footer_hints, modal_header_slotted,
    modal_header_with_close, modal_panel, mono, note_text, status_dot, ui, ModalBtn,
};

const PROGRESS_BAR_H: f32 = SPACE_MD;

/// Two [`CONTROL_H`] mini buttons plus their gap, so every row's name column ends on the same x.
const ROW_ACTION_SLOT_W: f32 = CONTROL_H * 2.0 + SPACE_XS;

const EMPTY_STATE_PY: f32 = SPACE_3XL * 2.0;

/// Tall enough for a script's last ~15 lines without pushing the stage message off a laptop screen.
const TEARDOWN_PTY_H: f32 = 240.0;

impl ModalLayer {
    pub(super) fn slot_mut(&mut self) -> &mut crate::modal::ModalSlot {
        &mut self.slot
    }

    /// Discovers the project's non-main worktrees up front so the modal can show "Also delete N worktrees" without re-shelling-out per frame.
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

    /// Teardown runs off the frame thread and reports progress back, so the window stays responsive while `git worktree remove` blocks.
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

        // Sessions under the project die with it either way — must not be left as orphans in the rail.
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

    fn finalize_remove_project(&mut self, idx: usize, name: &str, cx: &mut Context<Self>) {
        SettingsState::update(cx, move |store| {
            if idx < store.projects.len() {
                store.projects.remove(idx);
            }
        });
        SettingsState::flush_now(cx);
        self.state.update(cx, |s, cx| {
            s.on_project_removed(idx);
            cx.notify();
        });
        self.toast
            .update(cx, |t, cx| t.set_toast(format!("removed {name}"), cx));
        self.close(cx);
        cx.emit(ModalEvent::TreeInvalidated);
    }

    /// The same kill path the archive gate uses.
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

    /// One row per SESSION, not per worktree. Deliberately NOT filtered to live sessions — filtering would make the gate's count disagree with the killer's.
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

    /// So the modal re-renders cleared right after a kill instead of showing a stale count.
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

    pub fn archive_kill_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::ArchiveProject { name, .. }) = self.slot.get() else {
            return;
        };
        let name = name.clone();
        self.kill_sessions_for_project(&name, cx);
        self.refresh_archive_gate(cx);
    }

    /// `y` routes through here, which re-checks the blocked precondition, so it can't bypass a disabled Archive button.
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
        self.state.update(cx, |s, cx| {
            s.on_project_removed(idx);
            cx.notify();
        });
        cx.emit(ModalEvent::TreeInvalidated);
        cx.notify();
    }

    /// Kills sessions, then either runs the project's teardown script in a modal-embedded PTY (advancing to removal on exit) or removes the worktree immediately.
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

        // Modal-owned, never registered: a teardown PTY must not appear in the rail.
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

        // Poll rather than wait on the reader: EOF on the PTY and process reap are not the same event.
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

    /// The script finished (or Escape skipped it): drop the PTY, paint `Removing`, then run the blocking removal off-thread.
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
        // Set BEFORE the removal is kicked off so a `Removing` frame paints first.
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

    /// Escape during `RunningScript` means "skip the script and proceed to removal" — never "close".
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

/// The progress half of the two-stage variant (confirm+checkbox, then progress), bundled to keep one argument per concept.
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
    let body: AnyElement = if in_progress {
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
            list = list.child(note_text(format!(
                "{} worktree(s) failed to remove",
                errors.len()
            )));
        }
        list.into_any_element()
    } else {
        let mut d = div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_XL))
            .child(body_text(format!(
                "'{name}' will be unregistered from Grove. Its sessions will be ended."
            )))
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
            d = d.child(note_text(format!(
                "{} worktree(s) failed to remove",
                errors.len()
            )));
        }
        d.into_any_element()
    };

    // Cancel refused while in_progress: an in-flight `git worktree remove` can't be interrupted.
    let header = modal_header_slotted(
        Some("rm-proj-close"),
        "Remove project",
        c::RED(),
        None,
        None,
        if in_progress { None } else { Some(dispatch) },
    );

    let mut panel = div()
        .child(header)
        .child(divider_h())
        .child(modal_body(body));
    if !in_progress {
        let hints: &[(&'static str, &'static str)] = if worktree_count > 0 {
            &[
                ("y", "remove"),
                ("space", "toggle delete"),
                ("esc", "cancel"),
            ]
        } else {
            &[("y", "remove"), ("esc", "cancel")]
        };
        panel = panel.child(modal_footer(
            hints,
            vec![
                click_action(
                    "rm-proj-no",
                    "Cancel",
                    ModalBtn::Plain,
                    dispatch,
                    ModalClick::Cancel,
                )
                .into_any_element(),
                click_action(
                    "rm-proj-yes",
                    "Remove",
                    ModalBtn::Danger,
                    dispatch,
                    ModalClick::RemoveProjectConfirm,
                )
                .into_any_element(),
            ],
        ));
    }

    // MODAL_W_LG not MODAL_W_MD: an untruncated worktree name at 480 wraps and changes dialog height mid-removal.
    modal_panel(MODAL_W_LG, panel).into_any_element()
}

/// gpui has no `progress_bar` primitive, so the fill is a plain scaled child div.
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

/// One row per session, unfiltered by liveness, honestly labelled.
fn archive_project_modal(
    name: &str,
    sessions: &[(String, String, bool)],
    dispatch: &ModalDispatch,
) -> AnyElement {
    let blocked = !sessions.is_empty();

    let body: AnyElement = if blocked {
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
                .child(body_action(
                    "arch-kill",
                    format!("Kill all sessions ({})", sessions.len()),
                    c::RED(),
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
            .into_any_element()
    } else {
        div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL))
            .child(body_text(format!(
                "'{name}' will be hidden from the sidebar. Nothing is deleted — its scripts, \
             theme, and worktrees all stay exactly as they are."
            )))
            .into_any_element()
    };

    let panel_content = div()
        .child(modal_header_with_close(
            "arch-close",
            format!("Archive '{name}'?"),
            c::AMBER(),
            dispatch,
        ))
        .child(divider_h())
        .child(modal_body(body))
        .child(modal_footer(
            &[("y", "archive"), ("n", "cancel")],
            vec![
                click_action(
                    "arch-no",
                    "Cancel",
                    ModalBtn::Plain,
                    dispatch,
                    ModalClick::Cancel,
                )
                .into_any_element(),
                click_action_enabled(
                    "arch-yes",
                    "Archive",
                    ModalBtn::Primary,
                    !blocked,
                    dispatch,
                    ModalClick::ArchiveConfirm,
                ),
            ],
        ));

    if blocked {
        modal_panel(MODAL_W_LG, panel_content)
    } else {
        modal_panel(MODAL_W_SM, panel_content)
    }
    .into_any_element()
}

/// `glyph`/`hover_bg` distinguish restore (cyan on neutral hover) from delete (red on a red wash).
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

/// Every row derives live from `store.archived_projects()`.
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
        // The row stays inert (no row-level click): it used to also fire Restore alongside whichever mini button was clicked, since nothing stopped propagation.
        let card_rows: Vec<AnyElement> = rows
            .iter()
            .map(|(idx, name, path)| {
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

                div()
                    .id(gpui::SharedString::from(format!("arch-row-{idx}")))
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_XL))
                    .w_full()
                    .min_h(rpx(ROW_MIN_H))
                    .px(rpx(ROW_PX))
                    .py(rpx(ROW_PY))
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
                    .child(slot)
                    .into_any_element()
            })
            .collect();

        div()
            .id("archived-projects-list")
            .max_h(rpx(MODAL_SCROLL_MAX_H))
            .overflow_y_scroll()
            .w_full()
            .child(card(card_rows))
            .into_any_element()
    };

    modal_panel(
        MODAL_W_LG,
        div()
            .child(modal_header_with_close(
                "arch-list-close",
                "Archived projects",
                c::MAGENTA(),
                dispatch,
            ))
            .child(divider_h())
            .child(modal_body(body))
            .child(modal_footer_hints(&[("esc", "close")])),
    )
    .into_any_element()
}

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

    // Mid-flight the message stays one notch dimmer (FG_MUTE) than Done's body_text — how this modal tells "still working" from "final word".
    let message_el = if done {
        body_text(message.to_string()).into_any_element()
    } else {
        ui(message.to_string(), TEXT_TITLE, c::FG_MUTE()).into_any_element()
    };

    let body = div()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_XL))
        .children(layer.teardown_view.clone().map(|v| {
            div()
                .h(rpx(TEARDOWN_PTY_H))
                .w_full()
                .rounded(rpx(RADIUS_GROUP))
                .overflow_hidden()
                .child(v)
        }))
        .child(message_el);

    // Mid-removal: an in-flight `git worktree remove` can't be safely interrupted, so no footer at all for this stage.
    let footer = match stage {
        TeardownStage::Done { .. } => Some(modal_footer(
            &[("esc", "close")],
            vec![click_action(
                "td-close",
                "Close",
                ModalBtn::Primary,
                dispatch,
                ModalClick::Cancel,
            )
            .into_any_element()],
        )),
        // "esc" names what it does (skip & remove), not a plain dismissal — Escape here is a real semantic action.
        TeardownStage::RunningScript => Some(modal_footer(
            &[("esc", "skip & remove")],
            vec![click_action(
                "td-skip",
                "Skip & remove",
                ModalBtn::Plain,
                dispatch,
                ModalClick::Cancel,
            )
            .into_any_element()],
        )),
        TeardownStage::Removing => None,
    };

    // `Done` is the only stage where Cancel really closes, so it's the only stage with a close X.
    let header = modal_header_slotted(
        Some("td-close-x"),
        format!("Delete worktree / {wt_name}"),
        c::RED(),
        None,
        None,
        if done { Some(dispatch) } else { None },
    );

    let mut panel = div()
        .child(header)
        .child(divider_h())
        .child(modal_body(body));
    if let Some(footer) = footer {
        panel = panel.child(footer);
    }

    modal_panel(MODAL_W_LG, panel).into_any_element()
}
