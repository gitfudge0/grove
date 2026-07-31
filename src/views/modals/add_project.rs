//! The two-step add-project wizard and the full-viewport onboarding wizard.
//!
//! The pure half lives in [`crate::add_project`] and is tested there against a
//! temp tree. This module is the view plus the click/keyboard glue.
//!
//! Ports `src/gui/add_project.rs:439+` (the view), `modals.rs:117-136` (the
//! two cancel carve-outs), `src/gui/onboarding.rs` and
//! `src/gui/update/onboarding.rs` (incl. the `Modal::TmuxChoice` handoff at
//! :97).

use gpui::{div, prelude::*, px, AnyElement, App, Context, Window};

use crate::add_project::{self, ChooseOutcome, GitProbe, SubmitOutcome};
use crate::settings::SettingsState;
use crate::theme as c;

use super::shell::{
    body_text, click_action, click_checkbox, click_row, modal_body, modal_footer_hints,
    modal_header_row, modal_panel, note_text, ModalBtn,
};
use super::{Modal, ModalClick, ModalDispatch, ModalEvent, ModalLayer};
use crate::modal::{AddProjectStep, OnboardStep};

/// Rows of the directory match list kept on screen at once.
const DIR_ROWS: usize = 6;

impl ModalLayer {
    /// The wizard's clicks, plus every click Tasks 5-6's modals raise that is
    /// not already handled in [`ModalLayer::on_click`].
    pub(super) fn on_wizard_click(
        &mut self,
        click: ModalClick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match click {
            ModalClick::WizardBrowse => self.wizard_browse(window, cx),
            ModalClick::WizardPickDir(i) => {
                self.with_wizard(cx, |st| st.dir_sel = i);
                self.wizard_pick_dir(cx);
            }
            ModalClick::WizardNext => self.wizard_next(window, cx),
            ModalClick::WizardBack => {
                self.with_wizard(cx, add_project::change_source);
                self.rebuild_fields(window, cx);
            }
            ModalClick::WizardToggleInitGit => {
                self.with_wizard(cx, |st| st.init_git = !st.init_git);
            }
            ModalClick::OnboardSkip => self.onboard_skip(cx),
            ModalClick::OnboardAdvance => self.onboard_advance(window, cx),
            ModalClick::OnboardPickAgent(i) => {
                if let Some(Modal::Onboarding { agent_sel, .. }) = self.slot.get_mut() {
                    *agent_sel = i;
                }
                cx.notify();
            }
            ModalClick::OnboardPerms(skip) => {
                if let Some(Modal::Onboarding { perms_skip, .. }) = self.slot.get_mut() {
                    *perms_skip = skip;
                }
                cx.notify();
            }
            other => self.on_late_click(other, window, cx),
        }
    }

    fn with_wizard(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut crate::modal::AddProjectState),
    ) {
        if let Some(Modal::AddProject(st)) = self.slot.get_mut() {
            f(st);
        }
        cx.notify();
    }

    /// The OS folder picker. One dialog at a time — a second click while the
    /// picker is up must not spawn another (`modals.rs:490-534`).
    fn wizard_browse(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.picker_open {
            return;
        }
        self.picker_open = true;
        cx.spawn(async move |this, cx| {
            let rx = cx.update(|cx| {
                cx.prompt_for_paths(gpui::PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: None,
                })
            });
            let picked: Option<Vec<std::path::PathBuf>> = match rx.await {
                Ok(Ok(v)) => v,
                _ => None,
            };
            let _ = this.update(cx, |this: &mut Self, cx| {
                this.picker_open = false;
                let Some(path) = picked.and_then(|v| v.into_iter().next()) else {
                    cx.notify();
                    return;
                };
                this.wizard_choose(&path, cx);
            });
        })
        .detach();
    }

    /// The single funnel every folder source ends in.
    fn wizard_choose(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let mut outcome = None;
        if let Some(Modal::AddProject(st)) = self.slot.get_mut() {
            outcome = Some(add_project::choose(st, path));
        } else if let Some(Modal::Onboarding { path: p, .. }) = self.slot.get_mut() {
            *p = path.display().to_string();
        }
        if let Some(ChooseOutcome::Advanced(probe)) = outcome {
            if let Some(Modal::AddProject(st)) = self.slot.get_mut() {
                st.git_branch = match probe {
                    GitProbe::Repo { branch } => Some(branch),
                    GitProbe::NotRepo => None,
                };
            }
        }
        self.needs_focus = true;
        cx.notify();
    }

    /// ↑↓ on either wizard walk the **directory match list**, not the caret —
    /// this is a `wants_arrows` modal (carried decision 2).
    pub(super) fn wizard_dir_move(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.sync_wizard_buffers(cx);
        match self.slot.get_mut() {
            Some(Modal::AddProject(st)) => add_project::dir_move(st, delta),
            Some(Modal::Onboarding { path, dir_sel, .. }) => {
                let entries = add_project::list_dirs(path);
                *dir_sel = add_project::cycle(*dir_sel, delta, entries.len());
            }
            _ => {}
        }
        cx.notify();
    }

    /// Tab alternates path/name focus on the onboarding project step
    /// (`modals.rs:296-308`). Single-line fields, so `wants_tab` applies.
    pub(super) fn onboard_toggle_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_wizard_buffers(cx);
        let focused_name = match self.slot.get_mut() {
            Some(Modal::Onboarding { name_focused, .. }) => {
                *name_focused = !*name_focused;
                *name_focused
            }
            _ => return,
        };
        if focused_name {
            if let Some(f) = self.fields.get(1) {
                f.focus_at_end(window, cx);
            }
        } else {
            // Tab on the path field also picks the highlighted directory, then
            // returns focus to the path input with the caret at the end.
            self.wizard_pick_dir(cx);
            self.rebuild_fields(window, cx);
        }
        cx.notify();
    }

    /// The wizard's own keyboard, for the keys the shared verdict table falls
    /// through on (`add_project::handle_key`).
    pub(super) fn wizard_key(
        &mut self,
        key: crate::modal::ModalKey,
        _mods: crate::modal::ModalMods,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use crate::modal::ModalKey as K;
        match key {
            K::Down => {
                self.wizard_dir_move(1, cx);
                true
            }
            K::Up => {
                self.wizard_dir_move(-1, cx);
                true
            }
            K::Tab => {
                self.wizard_pick_dir(cx);
                self.rebuild_fields(window, cx);
                true
            }
            K::Enter => {
                self.wizard_next(window, cx);
                true
            }
            K::Escape => {
                // Escape from the DETAILS step goes back a step rather than
                // cancelling (`add_project::handle_key`); the pick-source
                // step's Escape is already `Close` in the shared table.
                self.with_wizard(cx, add_project::change_source);
                self.rebuild_fields(window, cx);
                true
            }
            _ => false,
        }
    }

    pub(super) fn wizard_pick_dir(&mut self, cx: &mut Context<Self>) {
        match self.slot.get_mut() {
            Some(Modal::AddProject(st)) => add_project::dir_pick(st),
            Some(Modal::Onboarding { path, dir_sel, .. }) => {
                let entries = add_project::list_dirs(path);
                if let Some(pick) = entries.get(*dir_sel) {
                    *path = format!("{pick}/");
                    *dir_sel = 0;
                }
            }
            _ => {}
        }
        self.needs_focus = true;
        cx.notify();
    }

    /// Step-1 Enter / "Next": the choose funnel, then the details step's
    /// validation and the actual registration.
    pub(super) fn wizard_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Pull the live buffers back out of the fields before deciding.
        self.sync_wizard_buffers(cx);
        let step = match self.slot.get() {
            Some(Modal::AddProject(st)) => st.step,
            _ => return,
        };
        if step == AddProjectStep::PickSource {
            let mut outcome = None;
            if let Some(Modal::AddProject(st)) = self.slot.get_mut() {
                outcome = Some(add_project::choose_typed(st));
            }
            if let Some(ChooseOutcome::Advanced(probe)) = outcome {
                if let Some(Modal::AddProject(st)) = self.slot.get_mut() {
                    st.git_branch = match probe {
                        GitProbe::Repo { branch } => Some(branch),
                        GitProbe::NotRepo => None,
                    };
                }
            }
            self.rebuild_fields(window, cx);
            return;
        }

        let existing: Vec<(String, String)> = cx
            .global::<SettingsState>()
            .store
            .projects
            .iter()
            .map(|p| (p.name.clone(), p.path.clone()))
            .collect();
        let outcome = {
            let Some(Modal::AddProject(st)) = self.slot.get_mut() else {
                return;
            };
            let probe = st
                .git_branch
                .clone()
                .map_or(GitProbe::NotRepo, |branch| GitProbe::Repo { branch });
            add_project::validate_submit(st, &probe, &existing)
        };
        let SubmitOutcome::Register {
            name,
            path,
            init_git,
        } = outcome
        else {
            cx.notify();
            return;
        };
        if init_git {
            if let Err(e) = grove_core::git::init_if_needed(&path) {
                if let Some(Modal::AddProject(st)) = self.slot.get_mut() {
                    st.note = Some(format!("git init failed: {e}"));
                }
                cx.notify();
                return;
            }
        }
        self.register_project(name, path, cx);
    }

    /// Persist a new project and select it (`App::register_project`,
    /// `src/app/mod.rs:600-620`).
    pub(super) fn register_project(
        &mut self,
        name: String,
        path: String,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        if !grove_core::git::valid_project_name(&name) {
            self.open(
                Modal::Message(format!(
                    "'{name}' isn't a valid project name; use letters, digits, '.', '-' or '_'"
                )),
                cx,
            );
            return None;
        }
        let idx = cx.global::<SettingsState>().store.projects.len();
        let toast_name = name.clone();
        SettingsState::update(cx, move |store| {
            store.projects.push(grove_core::storage::Project {
                name,
                path,
                scripts: grove_core::storage::ProjectScripts::default(),
                theme: None,
                archived: false,
            });
        });
        SettingsState::flush_now(cx);
        self.toast
            .update(cx, |t, cx| t.set_toast(format!("added {toast_name}"), cx));
        self.close(cx);
        cx.emit(ModalEvent::TreeInvalidated);
        Some(idx)
    }

    // ── onboarding ──────────────────────────────────────────────────────

    /// Escape or "Skip": mark onboarding done and get out of the way.
    pub(super) fn onboard_skip(&mut self, cx: &mut Context<Self>) {
        SettingsState::update(cx, |store| store.onboarded = true);
        SettingsState::flush_now(cx);
        self.close(cx);
    }

    /// Enter / "Continue" (`src/gui/update/onboarding.rs`). The last step
    /// persists the permissions choice and hands off to `Modal::TmuxChoice`
    /// (`update/onboarding.rs:97`).
    pub(super) fn onboard_advance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_wizard_buffers(cx);
        let Some(Modal::Onboarding { step, .. }) = self.slot.get() else {
            return;
        };
        match step {
            OnboardStep::Welcome => {
                if let Some(Modal::Onboarding { step, .. }) = self.slot.get_mut() {
                    *step = OnboardStep::Environment;
                }
            }
            OnboardStep::Environment => {
                if let Some(Modal::Onboarding { step, .. }) = self.slot.get_mut() {
                    *step = OnboardStep::Project;
                }
            }
            OnboardStep::Project => {
                let (path, name) = match self.slot.get() {
                    Some(Modal::Onboarding { path, name, .. }) => {
                        (path.trim().to_string(), name.clone())
                    }
                    _ => return,
                };
                let expanded = add_project::shellexpand_tilde(&path);
                let pb = std::path::PathBuf::from(&expanded);
                if !pb.is_dir() {
                    if let Some(Modal::Onboarding { note, .. }) = self.slot.get_mut() {
                        *note = Some("not a folder; choose a directory".into());
                    }
                    cx.notify();
                    return;
                }
                let abs = fs_err::canonicalize(&pb)
                    .map(|p| p.display().to_string())
                    .unwrap_or(expanded);
                let project_name = name
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| add_project::path_basename(&abs));
                let idx = {
                    let store = &cx.global::<SettingsState>().store;
                    store.projects.iter().position(|p| p.path == abs)
                };
                let idx = match idx {
                    Some(i) => Some(i),
                    None => {
                        let added = cx.global::<SettingsState>().store.projects.len();
                        let n = project_name.clone();
                        SettingsState::update(cx, move |store| {
                            store.projects.push(grove_core::storage::Project {
                                name: n,
                                path: abs,
                                scripts: grove_core::storage::ProjectScripts::default(),
                                theme: None,
                                archived: false,
                            });
                        });
                        SettingsState::flush_now(cx);
                        Some(added)
                    }
                };
                if let Some(Modal::Onboarding {
                    step, added_proj, ..
                }) = self.slot.get_mut()
                {
                    *added_proj = idx;
                    *step = OnboardStep::Session;
                }
                cx.emit(ModalEvent::TreeInvalidated);
            }
            OnboardStep::Session => {
                let (agent_sel, perms_skip, added_proj) = match self.slot.get() {
                    Some(Modal::Onboarding {
                        agent_sel,
                        perms_skip,
                        added_proj,
                        ..
                    }) => (*agent_sel, *perms_skip, *added_proj),
                    _ => return,
                };
                let agents = super::confirm::available_agents();
                let agent = agents.get(agent_sel).copied();
                // An explicit store value, not an inferred one
                // (`Modal::Onboarding::perms_skip`).
                SettingsState::update(cx, move |store| {
                    store.dangerously_skip_permissions_enabled = Some(perms_skip);
                    store.onboarded = true;
                    if let Some(a) = agent {
                        store.default_agent = Some(a);
                    }
                });
                SettingsState::flush_now(cx);
                let _ = added_proj;
                // The wizard hands straight off to the tmux choice
                // (`update/onboarding.rs:97`).
                self.open(Modal::TmuxChoice, cx);
                return;
            }
        }
        self.rebuild_fields(window, cx);
    }
}

// ── the views ────────────────────────────────────────────────────────────

pub fn render(layer: &ModalLayer, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    match layer.slot().get() {
        Some(Modal::AddProject(st)) => match st.step {
            AddProjectStep::PickSource => pick_source(layer, st, dispatch),
            AddProjectStep::Details => details(layer, st, dispatch),
        },
        Some(Modal::Onboarding { .. }) => onboarding(layer, dispatch, cx),
        _ => div().into_any_element(),
    }
}

/// The step-1 directory match list, driven by the typed path.
fn dir_list(path: &str, sel: usize, dispatch: &ModalDispatch) -> impl IntoElement {
    let entries = add_project::list_dirs(path);
    let offset = crate::launcher::scroll_offset_for(0, sel, DIR_ROWS, entries.len());
    let mut list = div().flex().flex_col().gap(px(2.0));
    if entries.is_empty() {
        return list.child(
            div()
                .px(px(8.0))
                .py(px(5.0))
                .text_size(px(11.0))
                .text_color(c::FG_MUTE())
                .child("no matching directories"),
        );
    }
    for (i, entry) in entries.iter().enumerate().skip(offset).take(DIR_ROWS) {
        let name = add_project::path_basename(entry);
        list = list.child(click_row(
            gpui::SharedString::from(format!("dir-{i}")),
            i == sel,
            dispatch,
            ModalClick::WizardPickDir(i),
            div()
                .flex_1()
                .font(gpui::font(crate::fonts::MONO_FAMILY))
                .text_size(px(11.0))
                .text_color(if i == sel { c::FG() } else { c::FG_DIM() })
                .child(name),
        ));
    }
    list
}

fn field(layer: &ModalLayer, idx: usize) -> Option<impl IntoElement> {
    layer.fields.get(idx).map(|f| {
        div()
            .w_full()
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .bg(c::BG())
            .border_1()
            .border_color(c::BORDER())
            .child(gpui_component::input::Input::new(f.state()).w_full())
    })
}

fn pick_source(
    layer: &ModalLayer,
    st: &crate::modal::AddProjectState,
    dispatch: &ModalDispatch,
) -> AnyElement {
    let mut body = div().flex().flex_col().gap(px(10.0));
    if let Some(f) = field(layer, 0) {
        body = body.child(f);
    }
    body = body.child(dir_list(&st.path, st.dir_sel, dispatch));
    if let Some(note) = &st.note {
        body = body.child(note_text(note.clone()));
    }
    body = body.child(
        div()
            .flex()
            .gap(px(8.0))
            .child(click_action(
                "ap-browse",
                "Browse…",
                ModalBtn::Plain,
                dispatch,
                ModalClick::WizardBrowse,
            ))
            .child(click_action(
                "ap-next",
                "Next",
                ModalBtn::Primary,
                dispatch,
                ModalClick::WizardNext,
            )),
    );
    modal_panel(
        520.0,
        div()
            .child(modal_header_row(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.0))
                            .text_color(c::CYAN())
                            .child("Add project"),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(c::FG_MUTE())
                            .child("1 / 2"),
                    ),
            ))
            .child(modal_body(body))
            .child(modal_footer_hints(&[
                ("↑↓", "walk"),
                ("tab", "pick"),
                ("⏎", "next"),
                ("esc / ctrl+c", "cancel"),
            ])),
    )
    .into_any_element()
}

fn details(
    layer: &ModalLayer,
    st: &crate::modal::AddProjectState,
    dispatch: &ModalDispatch,
) -> AnyElement {
    let mut body = div().flex().flex_col().gap(px(10.0)).child(
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .flex_1()
                    .font(gpui::font(crate::fonts::MONO_FAMILY))
                    .text_size(px(11.0))
                    .text_color(c::FG_DIM())
                    .child(st.path.clone()),
            )
            .child(click_action(
                "ap-change",
                "change",
                ModalBtn::Plain,
                dispatch,
                ModalClick::WizardBack,
            )),
    );
    if let Some(f) = field(layer, 0) {
        body = body.child(f);
    }
    body = body.child(match &st.git_branch {
        Some(branch) => body_text(format!("git repository on '{branch}'")),
        None => body_text("not a git repository"),
    });
    if st.git_branch.is_none() {
        body = body.child(click_checkbox(
            "ap-init-git",
            "Initialize git repository",
            st.init_git,
            c::GREEN(),
            true,
            dispatch,
            ModalClick::WizardToggleInitGit,
        ));
    }
    if let Some(note) = &st.note {
        body = body.child(note_text(note.clone()));
    }
    body = body.child(div().flex().gap(px(8.0)).child(click_action(
        "ap-add",
        "Add project",
        ModalBtn::Primary,
        dispatch,
        ModalClick::WizardNext,
    )));
    modal_panel(
        520.0,
        div()
            .child(modal_header_row(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.0))
                            .text_color(c::CYAN())
                            .child("Add project"),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(c::FG_MUTE())
                            .child("2 / 2"),
                    ),
            ))
            .child(modal_body(body))
            .child(modal_footer_hints(&[
                ("⏎", "add"),
                ("esc", "back"),
                ("ctrl+c", "cancel"),
            ])),
    )
    .into_any_element()
}

/// The first-run wizard. Full-viewport with no sidebar, statusbar or scrim —
/// the layer already renders it as a screen replacement (recorded ambiguity 1)
/// and the entrance animation is applied there.
fn onboarding(layer: &ModalLayer, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    let Some(Modal::Onboarding {
        step,
        path,
        dir_sel,
        note,
        agent_sel,
        perms_skip,
        ..
    }) = layer.slot().get()
    else {
        return div().into_any_element();
    };
    let _ = cx;

    let steps = OnboardStep::ALL;
    let index = steps.iter().position(|s| s == step).unwrap_or(0);
    let dots = {
        let mut row = div().flex().items_center().gap(px(6.0));
        for i in 0..steps.len() {
            row = row.child(div().size(px(6.0)).rounded_full().bg(if i == index {
                c::CYAN()
            } else {
                c::BG_HL()
            }));
        }
        row
    };

    let body = match step {
        OnboardStep::Welcome => div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(24.0))
                    .text_color(c::FG())
                    .child("Welcome to Grove"),
            )
            .child(body_text(
                "A worktree launchpad for AI coding agents. Three short steps \
                 and you're running.",
            )),
        OnboardStep::Environment => {
            let agents = super::confirm::available_agents();
            let mut list = div().flex().flex_col().gap(px(4.0));
            for a in &agents {
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(crate::icons::icon(a.icon_name(), 13.0, c::GREEN()))
                        .child(body_text(a.label().to_string())),
                );
            }
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(18.0))
                        .text_color(c::FG())
                        .child("Your environment"),
                )
                .child(body_text("Agents found on your PATH:"))
                .child(list)
                .child(body_text(if grove_core::tmux::available() {
                    "tmux found — sessions can survive a restart."
                } else {
                    "tmux not found — sessions will be native."
                }))
        }
        OnboardStep::Project => {
            let mut d = div().flex().flex_col().gap(px(10.0)).child(
                div()
                    .text_size(px(18.0))
                    .text_color(c::FG())
                    .child("Add your first project"),
            );
            if let Some(f) = field(layer, 0) {
                d = d.child(f);
            }
            d = d.child(dir_list(path, *dir_sel, dispatch));
            if let Some(f) = field(layer, 1) {
                d = d.child(body_text("Project name (optional)")).child(f);
            }
            if let Some(note) = note {
                d = d.child(note_text(note.clone()));
            }
            d.child(div().flex().gap(px(8.0)).child(click_action(
                "ob-browse",
                "Browse…",
                ModalBtn::Plain,
                dispatch,
                ModalClick::WizardBrowse,
            )))
        }
        OnboardStep::Session => {
            let agents = super::confirm::available_agents();
            let mut list = div().flex().flex_col().gap(px(4.0));
            for (i, a) in agents.iter().enumerate() {
                list = list.child(click_row(
                    gpui::SharedString::from(format!("ob-agent-{i}")),
                    i == *agent_sel,
                    dispatch,
                    ModalClick::OnboardPickAgent(i),
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(crate::icons::icon(a.icon_name(), 13.0, c::FG_DIM()))
                        .child(body_text(a.label().to_string())),
                ));
            }
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(18.0))
                        .text_color(c::FG())
                        .child("Your first session"),
                )
                .child(body_text("Default agent:"))
                .child(list)
                .child(body_text("Permission prompts:"))
                .child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(click_action(
                            "ob-perms-safe",
                            "Ask me (safe)",
                            if *perms_skip {
                                ModalBtn::Plain
                            } else {
                                ModalBtn::Primary
                            },
                            dispatch,
                            ModalClick::OnboardPerms(false),
                        ))
                        .child(click_action(
                            "ob-perms-skip",
                            "Skip prompts",
                            if *perms_skip {
                                ModalBtn::Danger
                            } else {
                                ModalBtn::Plain
                            },
                            dispatch,
                            ModalClick::OnboardPerms(true),
                        )),
                )
        }
    };

    div()
        .w(px(680.0))
        .flex()
        .flex_col()
        .gap(px(24.0))
        .child(body)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(dots)
                .child(div().flex_1())
                .child(click_action(
                    "ob-skip",
                    "Skip",
                    ModalBtn::Plain,
                    dispatch,
                    ModalClick::OnboardSkip,
                ))
                .child(click_action(
                    "ob-next",
                    if *step == OnboardStep::Session {
                        "Finish"
                    } else {
                        "Continue"
                    },
                    ModalBtn::Primary,
                    dispatch,
                    ModalClick::OnboardAdvance,
                )),
        )
        .into_any_element()
}
