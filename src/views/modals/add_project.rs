//! The two-step add-project wizard and the full-viewport onboarding wizard.
//!
//! The pure half lives in [`crate::add_project`] and is tested there against a
//! temp tree. This module is the view plus the click/keyboard glue.
//!
//! Ports `src/gui/add_project.rs:439+` (the view), `modals.rs:117-136` (the
//! two cancel carve-outs), `src/gui/onboarding.rs` and
//! `src/gui/update/onboarding.rs` (incl. the `Modal::TmuxChoice` handoff at
//! :97).

use crate::views::rpx;
use crate::views::tokens::*;
use gpui::{div, prelude::*, AnyElement, App, Context, Div, Focusable as _, Window};

use crate::add_project::{self, ChooseOutcome, GitProbe, SubmitOutcome};
use crate::settings::SettingsState;
use crate::theme as c;

use super::{Modal, ModalClick, ModalDispatch, ModalEvent, ModalLayer};
use crate::modal::{AddProjectStep, OnboardStep};
use crate::views::components::{
    body_action, body_text, card, click_action, click_checkbox, click_row, divider_h, field_box,
    modal_body, modal_footer, modal_header_slotted, modal_panel, mono, note_text, section_header,
    status_dot, ui, ModalBtn, RowDensity,
};

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
            ModalClick::OnboardBack => self.onboard_back(window, cx),
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
                // New project: the worktree dir is its name until a rename pins it.
                worktree_dir: None,
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

    /// "Back": step regression only (`app/onboarding.rs:127-135`). No
    /// unwinding of a project already registered on the way forward — the
    /// iced original doesn't either.
    pub(super) fn onboard_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_wizard_buffers(cx);
        let prev = match self.slot.get() {
            Some(Modal::Onboarding { step, .. }) => step.prev(),
            _ => None,
        };
        if let Some(prev) = prev {
            if let Some(Modal::Onboarding { step, .. }) = self.slot.get_mut() {
                *step = prev;
            }
            self.rebuild_fields(window, cx);
        }
    }

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
                                // New project: the worktree dir is its name until a rename pins it.
                                worktree_dir: None,
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

pub fn render(
    layer: &ModalLayer,
    dispatch: &ModalDispatch,
    window: &Window,
    cx: &App,
) -> AnyElement {
    match layer.slot().get() {
        Some(Modal::AddProject(st)) => match st.step {
            AddProjectStep::PickSource => pick_source(layer, st, dispatch, window, cx),
            AddProjectStep::Details => details(layer, st, dispatch, window, cx),
        },
        Some(Modal::Onboarding { .. }) => onboarding(layer, dispatch, window, cx),
        _ => div().into_any_element(),
    }
}

/// The step-1 directory match list, driven by the typed path. Every match
/// renders; the shared `MODAL_SCROLL_MAX_H` cap scrolls the overflow, and
/// `layer.list_scroll` keeps the arrowed-to row in view — retiring the old
/// `DIR_ROWS` window and its "↑N more" / "↓N more" indicators.
fn dir_list(
    layer: &ModalLayer,
    path: &str,
    sel: usize,
    dispatch: &ModalDispatch,
) -> impl IntoElement {
    let entries = add_project::list_dirs(path);
    let list = div().flex().flex_col().gap(rpx(SPACE_XS));
    if entries.is_empty() {
        return list.child(div().px(rpx(SPACE_LG)).py(rpx(SPACE_SM)).child(ui(
            "No matches",
            TEXT_BODY,
            c::FG_MUTE(),
        )));
    }
    let mut rows = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let name = format!("{}/", add_project::path_basename(entry));
        rows.push(
            click_row(
                gpui::SharedString::from(format!("dir-{i}")),
                i == sel,
                RowDensity::Card,
                dispatch,
                ModalClick::WizardPickDir(i),
                // A directory name is a token, not language (§5.2).
                mono(
                    name,
                    TEXT_BODY,
                    if i == sel { c::FG() } else { c::FG_DIM() },
                ),
            )
            .min_h(rpx(ROW_MIN_H))
            .px(rpx(ROW_PX))
            .py(rpx(ROW_PY))
            .into_any_element(),
        );
    }
    // The card *is* the scroll container: `scroll_to_item` addresses the
    // tracked element's direct children, and `card` interleaves a divider
    // after every row but the last, so row `k` sits at child `2k`.
    if sel < entries.len() {
        layer.scroll_list_to(0, sel, sel * 2);
    }
    list.child(
        card(rows)
            .id("dir-list")
            .max_h(rpx(MODAL_SCROLL_MAX_H))
            .overflow_y_scroll()
            .track_scroll(&layer.list_scroll),
    )
}

/// A [`field_box`]-wrapped `Input` bound to `layer.fields[idx]`, or
/// `None` if that field doesn't exist (§9.1.1 rule 6). Every wizard/onboarding
/// single-line text field goes through this one helper so the `Input`'s own
/// insets stay zeroed per `field_box`'s doc comment — the five chained
/// calls there are a contract, not a style choice.
fn field_row(layer: &ModalLayer, idx: usize, window: &Window, cx: &App) -> Option<Div> {
    layer.fields.get(idx).map(|f| {
        let focused = f.state().read(cx).focus_handle(cx).is_focused(window);
        field_box(focused).child(
            gpui_component::input::Input::new(f.state())
                .appearance(false)
                .pl(gpui::px(0.0))
                .pr(gpui::px(0.0))
                .py(gpui::px(0.0))
                .w_full(),
        )
    })
}

/// The wizard's shared header: a title in `c::MAGENTA()`, a right-aligned
/// "Step {n} of 2" in the header's `meta` slot, and a close X dispatching
/// [`ModalClick::Cancel`] (`src/gui/add_project.rs`'s `view` header row).
/// `close_id` must be unique within the modal (gpui bleeds hover state
/// between duplicate ids, §9.1).
fn wizard_header(step_no: u8, close_id: &'static str, dispatch: &ModalDispatch) -> Div {
    modal_header_slotted(
        Some(close_id),
        "Add project",
        c::MAGENTA(),
        // A step counter is a count, so mono (§5.2).
        Some(mono(format!("Step {step_no} of 2"), TEXT_SMALL, c::FG_MUTE()).into_any_element()),
        None,
        Some(dispatch),
    )
}

fn pick_source(
    layer: &ModalLayer,
    st: &crate::modal::AddProjectState,
    dispatch: &ModalDispatch,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let browse_label = if layer.picker_open {
        "Waiting for the folder picker…"
    } else {
        "Browse for folder…"
    };
    // No bordered buttons inside bodies (plan.md §3) — the hero button becomes
    // a flat magenta-tinted body action, still wired to the same dispatch.
    let browse = body_action(
        "ap-browse-hero",
        browse_label,
        c::MAGENTA(),
        dispatch,
        ModalClick::WizardBrowse,
    )
    .w_full()
    .py(rpx(SPACE_XL))
    .flex()
    .items_center()
    .justify_center();

    let drop_hint = div()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .child(ui(
            "Or drop a folder anywhere in this window",
            TEXT_SMALL,
            c::FG_MUTE(),
        ));

    let or_divider = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_XL))
        .child(divider_h().flex_1())
        .child(ui("Or type a path", TEXT_SMALL, c::FG_MUTE()))
        .child(divider_h().flex_1());

    let mut body = div()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_2XL))
        .child(browse)
        .child(drop_hint)
        .child(or_divider);
    if let Some(f) = field_row(layer, 0, window, cx) {
        body = body.child(f);
    }
    body = body.child(dir_list(layer, &st.path, st.dir_sel, dispatch));
    if let Some(note) = &st.note {
        body = body.child(note_text(note.clone()));
    }
    modal_panel(
        MODAL_W_XL,
        div()
            .child(wizard_header(1, "ap-close-src", dispatch))
            .child(divider_h())
            .child(modal_body(body))
            .child(modal_footer(
                &[
                    ("tab", "complete"),
                    ("↑↓", "select"),
                    ("⏎", "continue"),
                    ("esc", "cancel"),
                ],
                vec![click_action(
                    // Distinct from the details step's Cancel: gpui bleeds
                    // hover state between duplicate ids (§9.1).
                    "ap-cancel-src",
                    "Cancel",
                    ModalBtn::Plain,
                    dispatch,
                    ModalClick::Cancel,
                )
                .into_any_element()],
            )),
    )
    .into_any_element()
}

fn details(
    layer: &ModalLayer,
    st: &crate::modal::AddProjectState,
    dispatch: &ModalDispatch,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let chip = div()
        .w_full()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .px(rpx(SPACE_XL))
        .py(rpx(SPACE_MD))
        .rounded(rpx(RADIUS_CONTROL))
        .bg(c::BG_STRIP())
        .border_1()
        .border_color(c::BORDER())
        .child(crate::icons::icon("folder", ICON_MD, c::FG_DIM()))
        // A filesystem path is a token (§5.2).
        .child(
            mono(st.path.clone(), TEXT_BODY, c::FG())
                .flex_1()
                .overflow_hidden(),
        )
        .child(body_action(
            "ap-change",
            "Change",
            c::CYAN(),
            dispatch,
            ModalClick::WizardBack,
        ));

    let badge = match &st.git_branch {
        Some(branch) => div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_MD))
            .child(crate::icons::icon("git", ICON_MD, c::GREEN()))
            // A sentence about the folder, not a token — sans (§5.2), and the
            // same family as the "Not a git repository" arm below, which fills
            // this exact slot.
            .child(ui(
                format!("Git repository · branch {branch}"),
                TEXT_BODY,
                c::GREEN(),
            )),
        None => div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_MD))
            .child(crate::icons::icon("no-git", ICON_MD, c::AMBER()))
            .child(ui("Not a git repository", TEXT_BODY, c::AMBER())),
    };

    let default_name = add_project::path_basename(&st.path);

    let mut body = div()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_2XL))
        // Section labels are section_header (§5.2, §9.1.1 rule 7).
        .child(section_header("FOLDER", SPACE_2XL, SPACE_SM, 0.0))
        .child(chip)
        .child(badge)
        .child(
            div()
                .flex()
                .items_center()
                .child(section_header("NAME", SPACE_2XL, SPACE_SM, 0.0))
                .child(div().flex_1())
                // Quotes a directory name, so mono (§5.2).
                .child(mono(
                    format!("Empty uses '{default_name}'"),
                    TEXT_SMALL,
                    c::FG_MUTE(),
                )),
        );
    if let Some(f) = field_row(layer, 0, window, cx) {
        body = body.child(f);
    }
    if st.git_branch.is_none() {
        body = body.child(click_checkbox(
            "ap-init-git",
            "Initialize Git repository",
            st.init_git,
            c::MAGENTA(),
            true,
            dispatch,
            ModalClick::WizardToggleInitGit,
        ));
        if !st.init_git {
            body = body.child(ui(
                "Sessions will run directly in the project folder, no worktrees",
                TEXT_SMALL,
                c::FG_MUTE(),
            ));
        }
    }
    if let Some(note) = &st.note {
        body = body.child(note_text(note.clone()));
    }
    modal_panel(
        MODAL_W_XL,
        div()
            .child(wizard_header(2, "ap-close-name", dispatch))
            .child(divider_h())
            .child(modal_body(body))
            .child(modal_footer(
                &[("⏎", "add"), ("esc", "back")],
                vec![
                    click_action(
                        // See the pick-source step's Cancel — ids must be
                        // unique per view (§9.1).
                        "ap-cancel-name",
                        "Cancel",
                        ModalBtn::Plain,
                        dispatch,
                        ModalClick::Cancel,
                    )
                    .into_any_element(),
                    click_action(
                        "ap-add",
                        "Add project",
                        ModalBtn::Primary,
                        dispatch,
                        ModalClick::WizardNext,
                    )
                    .into_any_element(),
                ],
            )),
    )
    .into_any_element()
}

/// Whether `git` resolves on `PATH` (`git show main:src/gui/view/common.rs`'s
/// `git_on_path`, ported verbatim — no gpui equivalent exists yet).
fn git_on_path() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths)
                .any(|dir| fs_err::metadata(dir.join("git")).is_ok_and(|m| m.is_file()))
        })
    })
}

/// One bulleted value-prop line on the welcome step: a magenta mark, a bold
/// lead, and a muted explanation (iced `onboard_point`, onboarding.rs:387-409).
fn onboard_point(lead: &'static str, body: &'static str) -> Div {
    div()
        .flex()
        .gap(rpx(SPACE_XL))
        .child(
            div()
                .pt(rpx(SPACE_MD))
                .child(status_dot(DOT_SM, c::MAGENTA())),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_XS))
                .child(ui(lead, TEXT_TITLE, c::FG()))
                .child(body_text(body)),
        )
}

/// One detected-tool row on the environment step: a status dot, the tool
/// name, a muted description, and a right-aligned Found/Missing/Optional tag
/// (iced `onboard_env_row`, onboarding.rs:413-449).
fn onboard_env_row(found: bool, optional: bool, name: &'static str, meta: &'static str) -> Div {
    let (dotc, tag, tagc) = if found {
        (c::GREEN(), "Found", c::GREEN())
    } else if optional {
        (c::AMBER(), "Optional", c::AMBER())
    } else {
        (c::FG_MUTE(), "Missing", c::FG_MUTE())
    };
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(rpx(SPACE_XL))
        .px(rpx(SPACE_2XL))
        .py(rpx(SPACE_LG))
        .rounded(rpx(RADIUS_CONTROL))
        .border_1()
        .border_color(c::BORDER())
        .bg(c::BG_STRIP())
        .child(status_dot(DOT_SM, dotc))
        // The tool name is an executable on PATH and the tag is a status label,
        // so both are tokens; the description between them is language (§5.2).
        .child(mono(name, TEXT_TITLE, c::FG()))
        .child(ui(meta, TEXT_BODY, c::FG_MUTE()))
        .child(div().flex_1())
        .child(mono(tag, TEXT_SMALL, tagc))
}

/// The first-run wizard. Full-viewport with no sidebar, statusbar or scrim —
/// the layer already renders it as a screen replacement (recorded ambiguity 1)
/// and the entrance animation is applied there.
fn onboarding(
    layer: &ModalLayer,
    dispatch: &ModalDispatch,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let Some(Modal::Onboarding {
        step,
        path,
        dir_sel,
        name,
        note,
        added_proj,
        agent_sel,
        perms_skip,
        ..
    }) = layer.slot().get()
    else {
        return div().into_any_element();
    };

    // ── progress rail: per-step label, magenta done/current/pending tri-state
    // (iced onboarding.rs:60-81). ────────────────────────────────────────────
    let mut rail = div().flex().items_center().gap(rpx(SPACE_XL));
    for s in OnboardStep::flow() {
        let s = *s;
        let (dotc, txtc) = if s == *step {
            (c::MAGENTA(), c::FG())
        } else if s.index_in() < step.index_in() {
            (c::MAGENTA(), c::FG_DIM())
        } else {
            (c::BORDER(), c::FG_MUTE())
        };
        rail = rail.child(
            div()
                .flex()
                .items_center()
                .gap(rpx(SPACE_SM))
                .child(status_dot(DOT_SM, dotc))
                // Progress-rail step labels read as section labels (§5.2).
                .child(mono(s.label(), TEXT_MICRO, txtc)),
        );
    }

    // ── step body ────────────────────────────────────────────────────────
    let body: AnyElement = match step {
        OnboardStep::Welcome => div()
            .flex()
            .flex_col()
            // Carries the rhythm the deleted 20px spacer used to add (§6.1).
            .gap(rpx(SPACE_2XL))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_XL))
                    // ICON_DISPLAY is onboarding-only; never chrome.
                    .child(crate::icons::icon("grid", ICON_DISPLAY, c::CYAN()))
                    .child(
                        ui("grove", TEXT_DISPLAY_LG, c::FG())
                            .font_weight(gpui::FontWeight::BOLD),
                    ),
            )
            .child(ui(
                "a worktree launchpad for AI coding agents",
                TEXT_TITLE,
                c::FG_DIM(),
            ))
            .child(onboard_point(
                "Sessions are the unit of work",
                "Every agent you spawn lives in a managed session that survives navigation; switch between them in two keystrokes.",
            ))
            .child(onboard_point(
                "Worktrees, not branches",
                "Grove treats Git worktrees as a first-class primitive: create, list, and run agents inside them.",
            ))
            .child(onboard_point(
                "Quiet and keyboard-first",
                "The app stays out of the way so terminal output stays primary. This takes about a minute.",
            ))
            .into_any_element(),

        OnboardStep::Environment => {
            let rows = [
                (git_on_path(), false, "Git", "Version control"),
                (
                    grove_core::agent::Agent::Claude.available(),
                    false,
                    "Claude",
                    "Claude Code",
                ),
                (
                    grove_core::agent::Agent::Codex.available(),
                    false,
                    "Codex",
                    "Codex CLI",
                ),
                (
                    grove_core::agent::Agent::OpenCode.available(),
                    false,
                    "OpenCode",
                    "OpenCode CLI",
                ),
                (
                    grove_core::tmux::available(),
                    true,
                    "tmux",
                    "Persists sessions across restarts",
                ),
            ];
            let mut list = div().flex().flex_col().gap(rpx(SPACE_MD));
            for (found, optional, n, meta) in rows {
                list = list.child(onboard_env_row(found, optional, n, meta));
            }
            div()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_XL))
                .child(ui("Environment", TEXT_DISPLAY, c::FG()))
                .child(body_text(
                    "Grove spawns agents from your PATH; it doesn't install or authenticate \
                     them. Only Git is required to get going.",
                ))
                .child(list)
                .into_any_element()
        }

        OnboardStep::Project => {
            let mut d = div()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .child(ui("Add your first project", TEXT_DISPLAY, c::FG()))
                .child(body_text(
                    "Point Grove at a Git repository, or any plain folder for ad-hoc sessions.",
                ))
                // Tracking is faked with U+2009 inside `section_header`, never
                // hand-spaced with literal U+0020 (§5.4).
                .child(section_header("REPOSITORY OR FOLDER", SPACE_2XL, SPACE_SM, 0.0));
            let mut path_row = div().flex().items_center().gap(rpx(SPACE_LG));
            if let Some(f) = field_row(layer, 0, window, cx) {
                path_row = path_row.child(div().flex_1().child(f));
            }
            path_row = path_row.child(body_action(
                "ob-browse",
                if layer.picker_open { "Waiting…" } else { "Browse…" },
                c::MAGENTA(),
                dispatch,
                ModalClick::WizardBrowse,
            ));
            d = d.child(path_row);

            if !path.trim().is_empty() {
                d = d
                    .child(section_header("MATCHES", SPACE_2XL, SPACE_SM, 0.0))
                    .child(dir_list(layer, path, *dir_sel, dispatch));
            }

            if name.is_some() {
                d = d.child(section_header("NAME", SPACE_2XL, SPACE_SM, 0.0));
                if let Some(f) = field_row(layer, 1, window, cx) {
                    d = d.child(f);
                }
            }

            if let Some(note) = note {
                d = d.child(note_text(note.clone()));
            }
            // A sentence of instructions, not a keycap run — sans (§5.2).
            d.child(ui(
                "Tab to complete · ↑↓ to select · Enter to continue · Or skip setup",
                TEXT_SMALL,
                c::FG_MUTE(),
            ))
            .into_any_element()
        }

        OnboardStep::Session => {
            let project = added_proj
                .and_then(|i| cx.global::<SettingsState>().store.projects.get(i).cloned());
            let mut d = div()
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG))
                .child(ui("Start your first session", TEXT_DISPLAY, c::FG()));

            match &project {
                Some(p) => {
                    d = d.child(body_text(format!("Launch an agent inside {}.", p.name)));
                    let agents = super::confirm::available_agents();
                    let mut rows = Vec::new();
                    for (i, a) in agents.iter().enumerate() {
                        let active = i == *agent_sel;
                        rows.push(
                            click_row(
                                gpui::SharedString::from(format!("ob-agent-{i}")),
                                active,
                                RowDensity::Card,
                                dispatch,
                                ModalClick::OnboardPickAgent(i),
                                ui(
                                    a.label().to_string(),
                                    TEXT_TITLE,
                                    if active { c::FG() } else { c::FG_DIM() },
                                ),
                            )
                            .min_h(rpx(ROW_MIN_H))
                            .px(rpx(ROW_PX))
                            .py(rpx(ROW_PY))
                            .into_any_element(),
                        );
                    }
                    d = d.child(card(rows));
                }
                None => {
                    d = d.child(body_text(
                        "No project added. You can add one any time from the sidebar. \
                         Finish to start using Grove.",
                    ));
                }
            }

            let (perms_label_color, perms_line) = if *perms_skip {
                (c::YELLOW(), "Skip: agents run any command without asking")
            } else {
                (c::FG_MUTE(), "Safe: agents ask before running commands")
            };
            d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_3XL))
                        .child(section_header("PERMISSIONS", SPACE_2XL, 0.0, 0.0))
                        .child(crate::views::components::seg_group(
                            div()
                                .flex()
                                .items_center()
                                .child(crate::views::components::seg_button(
                                    "ob-perms-skip",
                                    "Skip",
                                    *perms_skip,
                                    crate::views::components::SegSide::Left,
                                    true,
                                    (!*perms_skip).then(|| -> crate::views::components::OnToggle {
                                        let dispatch = std::rc::Rc::clone(dispatch);
                                        Box::new(move |window, cx| {
                                            dispatch(ModalClick::OnboardPerms(true), window, cx);
                                        })
                                    }),
                                ))
                                .child(crate::views::components::seg_button(
                                    "ob-perms-safe",
                                    "Safe",
                                    !*perms_skip,
                                    crate::views::components::SegSide::Right,
                                    false,
                                    (*perms_skip).then(|| -> crate::views::components::OnToggle {
                                        let dispatch = std::rc::Rc::clone(dispatch);
                                        Box::new(move |window, cx| {
                                            dispatch(ModalClick::OnboardPerms(false), window, cx);
                                        })
                                    }),
                                )),
                        ),
                    ),
                )
                .child(ui(perms_line, TEXT_SMALL, perms_label_color))
                .into_any_element()
        }
    };

    // ── footer ────────────────────────────────────────────────────────────
    let next_label = match step {
        OnboardStep::Welcome => "Get started",
        OnboardStep::Session => "Launch session",
        _ => "Continue",
    };
    let count = format!("{} / {}", step.index_in() + 1, OnboardStep::flow().len());
    let mut footer = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        // A step count, so mono (§5.2).
        .child(mono(count, TEXT_BODY, c::FG_MUTE()))
        .child(div().flex_1())
        .child(click_action(
            "ob-skip",
            "Skip setup",
            ModalBtn::Plain,
            dispatch,
            ModalClick::OnboardSkip,
        ));
    if step.prev().is_some() {
        footer = footer.child(click_action(
            "ob-back",
            "Back",
            ModalBtn::Plain,
            dispatch,
            ModalClick::OnboardBack,
        ));
    }
    footer = footer.child(click_action(
        "ob-next",
        next_label,
        ModalBtn::Primary,
        dispatch,
        ModalClick::OnboardAdvance,
    ));

    // Small top-left wordmark — the wizard's only persistent chrome, distinct
    // from the larger centered wordmark the Welcome step's body renders.
    let brand = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .child(crate::icons::icon("grid", ICON_MD, c::CYAN()))
        .child(ui("grove", TEXT_TITLE, c::MAGENTA()).font_weight(gpui::FontWeight::BOLD));

    let content = div()
        .w(rpx(MODAL_W_LG))
        .flex()
        .flex_col()
        .gap(rpx(SPACE_3XL))
        .child(rail)
        .child(body);

    div()
        .size_full()
        .flex()
        .flex_col()
        .child(div().px(rpx(SPACE_3XL)).py(rpx(SPACE_3XL)).child(brand))
        .child(
            div()
                .flex_1()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .child(content),
        )
        .child(
            div()
                .w_full()
                .px(rpx(SPACE_3XL))
                .py(rpx(SPACE_3XL))
                .child(footer),
        )
        .into_any_element()
}
