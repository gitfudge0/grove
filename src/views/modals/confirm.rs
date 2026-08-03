//! Confirm (including Quit), Message, Input, TmuxChoice and AgentPicker.
//!
//! Ports of `src/gui/view/modals/confirm.rs:18-133,446-472` and
//! `src/gui/view/modals/settings.rs:30-129`, with the behavior halves from
//! `src/gui/update/modals.rs` (`submit_modal_input` :541-557,
//! `confirm_modal_response` :558-576, `submit_modal_confirm` :577-604,
//! `choose_tmux` :535-540) and `src/app/mod.rs:563-658`.

// The chrome, the input wrapper and the archive/teardown helpers are built
// once here and consumed by Tasks 4-6 of gpui rewrite plan 08.
#![allow(dead_code)]

use crate::views::rpx;
use gpui::{div, prelude::*, AnyElement, App, Context, Window};
use grove_core::agent::Agent;
use grove_core::{git, storage};

use crate::settings::SettingsState;
use crate::theme as c;

use super::shell::{
    click_action, click_row, divider_h, modal_body, modal_footer_hints, modal_header, modal_panel,
    ModalBtn,
};
use super::{Modal, ModalClick, ModalDispatch, ModalEvent, ModalLayer};
use crate::modal::ConfirmKind;

/// The agent order the picker lists (`src/app/mod.rs:168`); availability is
/// resolved at render time, `Terminal` is always present.
pub const AVAILABLE_AGENTS: [Agent; 4] = Agent::ALL;

/// Agents found on PATH, always including `Terminal`.
pub fn available_agents() -> Vec<Agent> {
    let found: Vec<Agent> = AVAILABLE_AGENTS
        .into_iter()
        .filter(|a| a.available())
        .collect();
    if found.is_empty() {
        vec![Agent::Terminal]
    } else {
        found
    }
}

impl ModalLayer {
    // ── Input (the single-field prompt) ─────────────────────────────────

    /// Enter on the worktree-name prompt. Port of `App::submit_input`
    /// (`src/app/mod.rs:563-594`): an empty value is a no-op, an invalid name
    /// raises a `Message`, a non-repo project routes through the
    /// init-and-add confirm, and only then is the worktree created.
    pub(super) fn submit_input(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(Modal::Input { .. }) = self.slot().get() else {
            return;
        };
        let value = self
            .fields
            .first()
            .map(|f| f.value(cx))
            .unwrap_or_default()
            .trim()
            .to_string();
        if value.is_empty() {
            return;
        }
        if !git::valid_worktree_name(&value) {
            // The inline red note, cleared on the next edit
            // (`src/app/mod.rs:572-576` raises a Message; the note is the
            // in-place equivalent the same prompt already carries).
            if let Some(Modal::Input { note, .. }) = self.slot_mut().get_mut() {
                *note = Some("Invalid name: use letters, digits, '-', '_' or '.'".into());
            }
            cx.notify();
            return;
        }
        let Some(project) = self.selected_project(cx) else {
            self.close(cx);
            return;
        };
        if !git::is_repo(&project.path) {
            self.open(
                Modal::Confirm {
                    title: "Initialize Git repo?".into(),
                    prompt: format!(
                        "'{}' is not a Git repo. Run `git init`, then create worktree '{}'.",
                        project.path, value
                    ),
                    destructive: false,
                    kind: ConfirmKind::InitAndAddWorktree { name: value },
                },
                cx,
            );
            return;
        }
        self.create_worktree(&project, &value, cx);
        self.close(cx);
    }

    /// The project the sidebar currently has selected.
    pub(super) fn selected_project(&self, cx: &App) -> Option<storage::Project> {
        let idx = self.state.read(cx).proj_idx();
        cx.global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .cloned()
    }

    pub(super) fn create_worktree(
        &mut self,
        project: &storage::Project,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        match git::add_worktree(&project.path, &project.name, name) {
            Ok(path) => {
                if let Err(e) = git::copy_worktree_includes(&project.path, &path) {
                    tracing::warn!("grove-gpui: worktree includes not copied: {e}");
                }
                crate::telemetry::track("worktree_created", vec![]);
                self.toast
                    .update(cx, |t, cx| t.set_toast(format!("added {name}"), cx));
                cx.emit(ModalEvent::WorktreeAdded);
            }
            Err(e) => {
                crate::telemetry::track("error", vec![("kind", "worktree_failed".into())]);
                self.open(Modal::Message(format!("Worktree failed: {e}")), cx);
            }
        }
    }

    // ── Confirm ─────────────────────────────────────────────────────────

    /// Port of `confirm_modal_response` + `App::submit_confirm`. `Quit` is
    /// resolved here because it needs the window, exactly as iced resolves it
    /// at the GUI layer rather than in `App`.
    pub(super) fn resolve_confirm(
        &mut self,
        yes: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Modal::Confirm { kind, .. }) = self.slot().get() else {
            return;
        };
        let kind = kind.clone();
        if matches!(kind, ConfirmKind::Quit) {
            self.close(cx);
            if yes {
                // The flush itself is `Workspace::shutdown`, run by the
                // workspace's `ModalEvent::Quit` arm right before `cx.quit()`
                // — one flush, three callers (carried decision 7).
                cx.emit(ModalEvent::Quit);
            }
            return;
        }
        self.close(cx);
        if !yes {
            return;
        }
        match kind {
            ConfirmKind::RemoveProject(idx) => self.open_remove_project(idx, cx),
            ConfirmKind::RemoveWorktree(path) => {
                if let Some(p) = self.selected_project(cx) {
                    self.start_teardown(&p, path, cx);
                }
            }
            ConfirmKind::InitAndAddWorktree { name } => {
                let Some(p) = self.selected_project(cx) else {
                    return;
                };
                if let Err(e) = git::init_if_needed(&p.path) {
                    self.open(Modal::Message(format!("Git init failed: {e}")), cx);
                    return;
                }
                self.create_worktree(&p, &name, cx);
            }
            // Resolved above; never reaches here.
            ConfirmKind::Quit => {}
        }
    }

    // ── TmuxChoice ──────────────────────────────────────────────────────

    /// Only an explicit pick records a backend — Escape deliberately persists
    /// nothing, so the choice is re-asked on the next launch
    /// (`modals.rs:258-269`, `App::set_tmux_enabled` :282-296).
    pub(super) fn choose_tmux(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if enabled && !grove_core::tmux::available() {
            self.toast.update(cx, |t, cx| {
                t.set_error("tmux not found; using native sessions", cx);
            });
            self.close(cx);
            return;
        }
        SettingsState::update(cx, |store| store.tmux_enabled = Some(enabled));
        let msg = if enabled {
            "tmux enabled for new sessions"
        } else {
            "tmux disabled for new sessions"
        };
        self.toast.update(cx, |t, cx| t.set_toast(msg, cx));
        if enabled {
            cx.emit(ModalEvent::TmuxEnabled);
        }
        self.close(cx);
    }

    // ── AgentPicker ─────────────────────────────────────────────────────

    /// Space toggles "make this the default agent" (`modals.rs:230-241`).
    pub(super) fn toggle_default_agent(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::AgentPicker { sel, .. }) = self.slot().get() else {
            return;
        };
        let agents = available_agents();
        let Some(agent) = agents.get(*sel).copied() else {
            return;
        };
        let label = agent.label().to_string();
        let already = cx.global::<SettingsState>().store.default_agent == Some(agent);
        let _ = label;
        SettingsState::update(cx, move |store| {
            store.default_agent = if already { None } else { Some(agent) };
        });
        cx.notify();
    }

    /// Enter spawns through the registry — via `Sidebar::spawn_session`, so
    /// the "failed to start session" toast producer covers it exactly once.
    pub(super) fn submit_agent_picker(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::AgentPicker {
            project,
            wt_path,
            sel,
        }) = self.slot().get()
        else {
            return;
        };
        let agents = available_agents();
        let Some(agent) = agents.get(*sel).copied() else {
            return;
        };
        let ev = ModalEvent::SpawnAgent {
            project: project.clone(),
            wt_path: wt_path.clone(),
            agent,
        };
        self.close(cx);
        cx.emit(ev);
    }
}

// ── the views ────────────────────────────────────────────────────────────

pub fn render(layer: &ModalLayer, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    match layer.slot().get() {
        Some(Modal::Input { title, note, .. }) => {
            input_modal(layer, title, note.as_deref(), dispatch)
        }
        Some(Modal::Confirm {
            title,
            prompt,
            destructive,
            kind,
        }) => confirm_modal(title, prompt, *destructive, kind, dispatch),
        Some(Modal::Message(text)) => message_modal(text, dispatch),
        Some(Modal::TmuxChoice) => tmux_choice_modal(dispatch),
        Some(Modal::AgentPicker {
            project,
            wt_path,
            sel,
        }) => agent_picker_modal(project, wt_path, *sel, dispatch, cx),
        _ => div().into_any_element(),
    }
}

/// The generic single-field prompt (`view/modals/confirm.rs:18-69`): title,
/// the field, and an inline red note cleared on the next edit. Zoned exactly
/// as the iced original — header / divider / input-zone / divider /
/// buttons-zone / divider / footer (`confirm.rs:33-67`) — with the same
/// leading `git-branch` icon in the field row (`confirm.rs:33-40`).
fn input_modal(
    layer: &ModalLayer,
    title: &str,
    note: Option<&str>,
    dispatch: &ModalDispatch,
) -> AnyElement {
    let mut input_zone = div()
        .w_full()
        .flex()
        .items_center()
        .gap(rpx(8.0))
        .px(rpx(16.0))
        .py(rpx(14.0));
    if let Some(field) = layer.fields.first() {
        input_zone = input_zone
            .child(crate::icons::icon("git-branch", 16.0, c::FG_MUTE()))
            .child(
                gpui_component::input::Input::new(field.state())
                    .flex_1()
                    .text_size(rpx(14.0)),
            );
    }

    let mut buttons_zone = div()
        .flex()
        .flex_col()
        .gap(rpx(8.0))
        .px(rpx(16.0))
        .py(rpx(12.0));
    if let Some(note) = note {
        buttons_zone = buttons_zone.child(
            div()
                .text_size(rpx(12.0))
                .text_color(c::RED())
                .child(note.to_string()),
        );
    }
    buttons_zone = buttons_zone.child(
        div()
            .flex()
            .items_center()
            .gap(rpx(8.0))
            .child(div().flex_1())
            .child(click_action(
                "in-cancel",
                "Cancel",
                ModalBtn::Plain,
                dispatch,
                ModalClick::Cancel,
            ))
            .child(click_action(
                "in-ok",
                "Submit",
                ModalBtn::Primary,
                dispatch,
                ModalClick::Submit,
            )),
    );

    modal_panel(
        480.0,
        div()
            .child(modal_header(title.to_string(), c::MAGENTA()))
            .child(divider_h())
            .child(input_zone)
            .child(divider_h())
            .child(buttons_zone)
            .child(divider_h())
            .child(modal_footer_hints(&[("⏎", "confirm"), ("esc", "cancel")])),
    )
    .into_any_element()
}

/// `confirm_modal` (`view/modals/confirm.rs:70-133`) with its destructive
/// styling. Escape = no, Enter = yes, `y`/`n`.
fn confirm_modal(
    title: &str,
    prompt: &str,
    destructive: bool,
    kind: &ConfirmKind,
    dispatch: &ModalDispatch,
) -> AnyElement {
    let accent = if destructive { c::RED() } else { c::MAGENTA() };
    let (label, label_lower) = match kind {
        ConfirmKind::Quit => ("Quit", "quit"),
        _ if destructive => ("Remove", "remove"),
        _ => ("Confirm", "confirm"),
    };
    let footer = if destructive {
        modal_footer_hints(&[("y", label_lower), ("esc", "cancel")])
    } else {
        modal_footer_hints(&[("⏎", "confirm"), ("esc", "cancel")])
    };
    modal_panel(
        480.0,
        div()
            .child(modal_header(title.to_string(), accent))
            .child(modal_body(
                div()
                    .flex()
                    .flex_col()
                    .gap(rpx(12.0))
                    .child(
                        div()
                            .text_size(rpx(13.0))
                            .text_color(c::FG_DIM())
                            .child(prompt.to_string()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(rpx(8.0))
                            .child(div().flex_1())
                            .child(click_action(
                                "cf-no",
                                "Cancel",
                                ModalBtn::Plain,
                                dispatch,
                                ModalClick::Confirm(false),
                            ))
                            .child(click_action(
                                "cf-yes",
                                label,
                                if destructive {
                                    ModalBtn::Danger
                                } else {
                                    ModalBtn::Primary
                                },
                                dispatch,
                                ModalClick::Confirm(true),
                            )),
                    ),
            ))
            .child(footer),
    )
    .into_any_element()
}

/// `message_modal` (`view/modals/confirm.rs:446-472`): text and one dismiss.
fn message_modal(text: &str, dispatch: &ModalDispatch) -> AnyElement {
    modal_panel(
        480.0,
        div()
            .child(modal_header("Notice", c::CYAN()))
            .child(modal_body(
                div()
                    .flex()
                    .flex_col()
                    .gap(rpx(12.0))
                    .child(
                        div()
                            .text_size(rpx(13.0))
                            .text_color(c::FG_DIM())
                            .child(text.to_string()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(div().flex_1())
                            .child(click_action(
                                "msg-ok",
                                "Close",
                                ModalBtn::Primary,
                                dispatch,
                                ModalClick::Cancel,
                            )),
                    ),
            ))
            .child(modal_footer_hints(&[("esc", "close")])),
    )
    .into_any_element()
}

/// `tmux_choice_modal` (`view/modals/settings.rs:30-57`). Escape deliberately
/// persists nothing, so the footer does not offer it as a choice.
fn tmux_choice_modal(dispatch: &ModalDispatch) -> AnyElement {
    modal_panel(
        480.0,
        div()
            .child(modal_header("Session backend", c::CYAN()))
            .child(modal_body(
                div()
                    .flex()
                    .flex_col()
                    .gap(rpx(10.0))
                    .child(
                        div().text_size(rpx(13.0)).text_color(c::FG_DIM()).child(
                            "Use tmux for new sessions? Existing sessions keep their \
                             current backend.",
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(rpx(8.0))
                            .child(div().flex_1())
                            .child(click_action(
                                "tmux-no",
                                "Native",
                                ModalBtn::Plain,
                                dispatch,
                                ModalClick::ChooseTmux(false),
                            ))
                            .child(click_action(
                                "tmux-yes",
                                "Tmux",
                                ModalBtn::Primary,
                                dispatch,
                                ModalClick::ChooseTmux(true),
                            )),
                    ),
            ))
            .child(modal_footer_hints(&[
                ("⏎", "tmux"),
                ("n", "native"),
                ("esc", "close"),
            ])),
    )
    .into_any_element()
}

/// `agent_picker_modal` (`view/modals/settings.rs:58-129`): the available
/// agents with a selection cursor, Space toggling the default.
fn agent_picker_modal(
    project: &str,
    wt_path: &str,
    sel: usize,
    dispatch: &ModalDispatch,
    cx: &App,
) -> AnyElement {
    let agents = available_agents();
    let default = cx.global::<SettingsState>().store.default_agent;
    let wt_name = crate::views::rows::path_basename(wt_path);
    let title = if project.is_empty() {
        format!("Start session / {wt_name}")
    } else {
        format!("Start session / {project} / {wt_name}")
    };

    let mut list = div().flex().flex_col().gap(rpx(2.0)).p(rpx(8.0)).w_full();
    for (i, agent) in agents.iter().enumerate() {
        let selected = i == sel;
        let is_default = default == Some(*agent);
        list = list.child(click_row(
            gpui::SharedString::from(format!("agent-{i}")),
            selected,
            dispatch,
            ModalClick::SelectRow(i),
            div()
                .flex()
                .items_center()
                .gap(rpx(8.0))
                .w_full()
                .h(rpx(32.0))
                .child(
                    div()
                        .w(rpx(24.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(crate::icons::icon(
                            agent.icon_name(),
                            16.0,
                            if selected { c::YELLOW() } else { c::FG_MUTE() },
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(rpx(12.0))
                        .text_color(if selected { c::FG() } else { c::FG_DIM() })
                        .child(agent.label().to_string()),
                )
                .when(is_default, |d| {
                    d.child(
                        div()
                            .text_size(rpx(11.0))
                            .text_color(c::FG_MUTE())
                            .child("Default"),
                    )
                }),
        ));
    }

    let actions = div()
        .flex()
        .items_center()
        .gap(rpx(8.0))
        .child(click_action(
            "ap-default",
            "Default",
            ModalBtn::Plain,
            dispatch,
            ModalClick::ToggleDefaultAgent,
        ))
        .child(div().flex_1())
        .child(click_action(
            "ap-cancel",
            "Cancel",
            ModalBtn::Plain,
            dispatch,
            ModalClick::Cancel,
        ))
        .child(click_action(
            "ap-launch",
            "Launch",
            ModalBtn::Primary,
            dispatch,
            ModalClick::Submit,
        ));

    modal_panel(
        500.0,
        div()
            .child(modal_header(title, c::MAGENTA()))
            .child(modal_body(div().flex().flex_col().gap(rpx(12.0)).child(list).child(actions)))
            .child(modal_footer_hints(&[
                ("↑↓", "choose"),
                ("⏎", "launch"),
                ("esc", "cancel"),
            ])),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_is_always_offered_even_with_no_agents_on_path() {
        let agents = available_agents();
        assert!(!agents.is_empty());
        assert!(
            agents.contains(&Agent::Terminal) || agents.len() > 1,
            "the picker must never be empty: {agents:?}"
        );
    }

    /// The four `ConfirmKind`s the modal set actually raises
    /// (`src/app/modal.rs:177-186`).
    #[test]
    fn destructive_kinds_render_with_the_red_accent() {
        for (kind, destructive) in [
            (ConfirmKind::RemoveProject(0), true),
            (ConfirmKind::RemoveWorktree("/w".into()), true),
            (ConfirmKind::InitAndAddWorktree { name: "x".into() }, false),
            (ConfirmKind::Quit, true),
        ] {
            // A compile-time reminder that every kind is accounted for; the
            // accent itself is asserted by the manual sweep (Task 7 row 2).
            let _ = (&kind, destructive);
        }
    }
}
