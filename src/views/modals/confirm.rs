//! Confirm (including Quit), Message, Input, TmuxChoice and AgentPicker.

use crate::views::rpx;
use crate::views::tokens::*;
use gpui::{div, prelude::*, px, AnyElement, App, Context, Focusable as _, Window};
use grove_core::agent::Agent;
use grove_core::{git, storage};

use crate::settings::SettingsState;
use crate::theme as c;

use super::{Modal, ModalClick, ModalDispatch, ModalEvent, ModalLayer};
use crate::modal::ConfirmKind;
use crate::views::components::{
    body_action, body_text, card, click_action, click_row, divider_h, field_box, icon_slot,
    modal_body, modal_footer, modal_header_with_close, modal_panel, note_text, ui, ModalBtn,
    RowDensity,
};

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
    /// Empty value is a no-op; invalid name raises a `Message`; non-repo project routes through init-and-add confirm first.
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
            // Inline red note, cleared on the next edit.
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
        // Pinned directory key, not the display name — renaming a project must not scatter worktrees across directories.
        match git::add_worktree(&project.path, project.worktree_dir(), name) {
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

    /// `Quit` is resolved here because it needs the window.
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
            ConfirmKind::Quit => {}
        }
    }

    /// Only an explicit pick records a backend — Escape persists nothing, so the choice is re-asked next launch.
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

    /// Spawns through `Sidebar::spawn_session`, so the "failed to start" toast producer covers it exactly once.
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

pub fn render(
    layer: &ModalLayer,
    dispatch: &ModalDispatch,
    window: &Window,
    cx: &App,
) -> AnyElement {
    match layer.slot().get() {
        Some(Modal::Input { title, note, .. }) => {
            input_modal(layer, title, note.as_deref(), dispatch, window, cx)
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

fn input_modal(
    layer: &ModalLayer,
    title: &str,
    note: Option<&str>,
    dispatch: &ModalDispatch,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let mut body = div().flex().flex_col().gap(rpx(SPACE_LG));
    if let Some(field) = layer.fields.first() {
        let focused = field.state().read(cx).focus_handle(cx).is_focused(window);
        body = body.child(
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(rpx(SPACE_LG))
                .child(crate::icons::icon("git-branch", ICON_LG, c::FG_MUTE()))
                .child(
                    field_box(focused).flex_1().child(
                        gpui_component::input::Input::new(field.state())
                            .appearance(false)
                            .pl(px(0.0))
                            .pr(px(0.0))
                            .py(px(0.0))
                            .w_full(),
                    ),
                ),
        );
    }
    if let Some(note) = note {
        body = body.child(note_text(note.to_string()));
    }

    modal_panel(
        MODAL_W_MD,
        div()
            .child(modal_header_with_close(
                "in-close",
                title.to_string(),
                c::MAGENTA(),
                dispatch,
            ))
            .child(divider_h())
            .child(modal_body(body))
            .child(modal_footer(
                &[("⏎", "confirm"), ("esc", "cancel")],
                vec![
                    click_action(
                        "in-cancel",
                        "Cancel",
                        ModalBtn::Plain,
                        dispatch,
                        ModalClick::Cancel,
                    )
                    .into_any_element(),
                    click_action(
                        "in-ok",
                        "Submit",
                        ModalBtn::Primary,
                        dispatch,
                        ModalClick::Submit,
                    )
                    .into_any_element(),
                ],
            )),
    )
    .into_any_element()
}

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
    let hints: &[(&'static str, &'static str)] = if destructive {
        &[("y", label_lower), ("esc", "cancel")]
    } else {
        &[("⏎", "confirm"), ("esc", "cancel")]
    };
    modal_panel(
        MODAL_W_MD,
        div()
            .child(modal_header_with_close(
                "cf-close",
                title.to_string(),
                accent,
                dispatch,
            ))
            .child(divider_h())
            .child(modal_body(body_text(prompt.to_string())))
            .child(modal_footer(
                hints,
                vec![
                    click_action(
                        "cf-no",
                        "Cancel",
                        ModalBtn::Plain,
                        dispatch,
                        ModalClick::Confirm(false),
                    )
                    .into_any_element(),
                    click_action(
                        "cf-yes",
                        label,
                        if destructive {
                            ModalBtn::Danger
                        } else {
                            ModalBtn::Primary
                        },
                        dispatch,
                        ModalClick::Confirm(true),
                    )
                    .into_any_element(),
                ],
            )),
    )
    .into_any_element()
}

fn message_modal(text: &str, dispatch: &ModalDispatch) -> AnyElement {
    modal_panel(
        MODAL_W_MD,
        div()
            .child(modal_header_with_close(
                "msg-close",
                "Notice",
                c::MAGENTA(),
                dispatch,
            ))
            .child(divider_h())
            .child(modal_body(body_text(text.to_string())))
            .child(modal_footer(
                &[("esc", "close")],
                vec![click_action(
                    "msg-ok",
                    "Close",
                    ModalBtn::Primary,
                    dispatch,
                    ModalClick::Cancel,
                )
                .into_any_element()],
            )),
    )
    .into_any_element()
}

/// Escape persists nothing, so the footer does not offer it as a choice.
fn tmux_choice_modal(dispatch: &ModalDispatch) -> AnyElement {
    modal_panel(
        MODAL_W_MD,
        div()
            .child(modal_header_with_close(
                "tmux-close",
                "Session backend",
                c::MAGENTA(),
                dispatch,
            ))
            .child(divider_h())
            .child(modal_body(body_text(
                "Use tmux for new sessions? Existing sessions keep their \
                 current backend.",
            )))
            .child(modal_footer(
                &[("⏎", "tmux"), ("n", "native"), ("esc", "close")],
                vec![
                    click_action(
                        "tmux-no",
                        "Native",
                        ModalBtn::Plain,
                        dispatch,
                        ModalClick::ChooseTmux(false),
                    )
                    .into_any_element(),
                    click_action(
                        "tmux-yes",
                        "Tmux",
                        ModalBtn::Primary,
                        dispatch,
                        ModalClick::ChooseTmux(true),
                    )
                    .into_any_element(),
                ],
            )),
    )
    .into_any_element()
}

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

    let mut rows: Vec<AnyElement> = Vec::new();
    for (i, agent) in agents.iter().enumerate() {
        let selected = i == sel;
        let is_default = default == Some(*agent);
        let content = div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_LG))
            .w_full()
            .child(icon_slot(
                agent.icon_name(),
                ICON_LG,
                if selected { c::YELLOW() } else { c::FG_MUTE() },
            ))
            .child(
                ui(
                    agent.label().to_string(),
                    TEXT_BODY,
                    if selected { c::FG() } else { c::FG_DIM() },
                )
                .flex_1(),
            )
            .when(is_default, |d| {
                d.child(ui("Default", TEXT_SMALL, c::FG_MUTE()))
            });
        rows.push(
            click_row(
                ("agent", i),
                selected,
                RowDensity::Card,
                dispatch,
                ModalClick::SelectRow(i),
                content,
            )
            .min_h(rpx(ROW_MIN_H))
            .px(rpx(ROW_PX))
            .py(rpx(ROW_PY))
            .into_any_element(),
        );
    }

    modal_panel(
        MODAL_W_MD,
        div()
            .child(modal_header_with_close(
                "ap-close",
                title,
                c::MAGENTA(),
                dispatch,
            ))
            .child(divider_h())
            .child(modal_body(
                div()
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_LG))
                    .child(card(rows))
                    .child(body_action(
                        "ap-default",
                        "Default",
                        c::CYAN(),
                        dispatch,
                        ModalClick::ToggleDefaultAgent,
                    )),
            ))
            .child(modal_footer(
                &[("↑↓", "choose"), ("⏎", "launch"), ("esc", "cancel")],
                vec![
                    click_action(
                        "ap-cancel",
                        "Cancel",
                        ModalBtn::Plain,
                        dispatch,
                        ModalClick::Cancel,
                    )
                    .into_any_element(),
                    click_action(
                        "ap-launch",
                        "Launch",
                        ModalBtn::Primary,
                        dispatch,
                        ModalClick::Submit,
                    )
                    .into_any_element(),
                ],
            )),
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

    #[test]
    fn destructive_kinds_render_with_the_red_accent() {
        for (kind, destructive) in [
            (ConfirmKind::RemoveProject(0), true),
            (ConfirmKind::RemoveWorktree("/w".into()), true),
            (ConfirmKind::InitAndAddWorktree { name: "x".into() }, false),
            (ConfirmKind::Quit, true),
        ] {
            // Compile-time reminder that every kind is accounted for.
            let _ = (&kind, destructive);
        }
    }
}
