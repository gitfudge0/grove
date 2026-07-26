//! Two-step add-project wizard (`Modal::AddProject` / `Grove::add_project`):
//! pick a folder (native picker, drop, or typed path with tab-completion),
//! then confirm the details (name override, upfront git probe, init-git
//! choice) before anything is persisted.

use super::icons::icon;
use super::metrics::UI_FONT;
use super::palette as c;
use super::state::{Grove, Msg as GMsg};
use super::view::{input_field_style, modal_input_id, modal_name_id};
use super::widgets::{
    divider_h, modal_action, modal_checkbox, modal_footer_hints, modal_header_row, modal_panel,
    ModalBtn,
};
use crate::app::{App, Modal};
use fs_err as fs;
use grove_core::git;
use iced::border::Radius;
use iced::keyboard::{key::Named, Key};
use iced::widget::{button, column, container, row, text, text_input, Id, Space};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow, Task};

/// Which pane of the two-step add-project modal is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AddProjectStep {
    PickSource,
    Details,
}

/// Whether the caller must rebuild `Grove`'s worktree cache after an
/// `update`/`handle_key` call — a named replacement for a bare `bool` return
/// so call sites don't have to remember (or a reader guess) what the flag
/// means. Only `Submit` (mouse or Enter) produces `Rebuild`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum WtCacheRebuild {
    Skip,
    Rebuild,
}

/// Result of probing the chosen folder for a git repository when the
/// add-project modal enters its details step. Transient inspection state
/// scoped to the wizard's own lifetime — re-probed on every folder choice,
/// never persisted, and touched nowhere outside this module — so it lives
/// alongside the rest of the wizard's presentation state rather than in the
/// domain layer (`App`/`storage::Project` are the only things that persist).
#[derive(Clone)]
pub enum GitProbe {
    Repo { branch: String },
    NotRepo,
}

/// Live state for the two-step add-project wizard, when open. `Some` exactly
/// when `app.modal` is `Modal::AddProject`.
pub struct AddProjectState {
    pub step: AddProjectStep,
    /// Step 1: the typed path buffer. Step 2: the canonicalized folder.
    pub path: String,
    /// Directory-match cursor for the step-1 autocomplete list.
    pub dir_sel: usize,
    /// Project-name override. Left empty, the folder basename is used (shown
    /// as the field's placeholder). Edits survive a round-trip through
    /// "change".
    pub name: String,
    pub git: GitProbe,
    /// "Initialize git repository" checkbox (meaningful when `NotRepo`).
    pub init_git: bool,
    /// Inline validation message, cleared on the next edit.
    pub note: Option<String>,
}

/// Messages the add-project wizard can emit. `Open` is intercepted by the
/// parent (`Grove::update`'s `Msg::AddProject` arm) before it ever reaches
/// `update` here — it needs to clear `Grove::open_agent_menu`, a Grove-owned
/// field this module never touches (only `&mut App`), the same reason
/// `theme_manager_editor::Msg::Edit` is intercepted.
///
/// `AddProjectBrowse`/`AddProjectPicked` (the native-picker round trip) stay
/// as their own top-level `Msg` variants rather than folding in here: the
/// onboarding wizard's project step (`gui::onboarding`) reuses that exact
/// same picker affordance, so the handler has to stay shared at the parent
/// rather than living inside a module scoped to this wizard alone.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Open the wizard at its pick-source step.
    Open,
    /// Live edit of the step-1 path buffer.
    PathChanged(String),
    /// Step-1 Enter on the path field: feed the typed path to the choose funnel.
    ChooseTyped,
    /// Live edit of the step-2 project-name field.
    NameChanged(String),
    /// "change" on the step-2 folder chip: back to the pick-source step.
    ChangeSource,
    /// Toggle the step-2 "initialize git repository" checkbox.
    ToggleInitGit(bool),
    /// Final submit from the details step.
    Submit,
    /// Step-1 autocomplete list row picked (click, or Tab from the keyboard
    /// handler).
    PickDir(String),
}

/// Focuses the widget with the given [`Id`]. Local copy of `update.rs`'s
/// `focus` helper, scoped to this module's own `Msg` rather than the
/// parent's — see `scripts_editor`'s equivalent note for why: iced's
/// `Task::discard` is generic over the output message type, but the
/// parent's helper hardcodes its return type to `super::state::Msg`.
fn focus(id: Id) -> Task<Msg> {
    iced::advanced::widget::operate(iced::advanced::widget::operation::focusable::focus::<()>(
        id,
    ))
    .discard()
}

/// Moves the text-input cursor with the given [`Id`] to the end of its
/// content. See [`focus`] for why this local copy exists.
fn move_cursor_to_end(id: Id) -> Task<Msg> {
    iced::advanced::widget::operate(
        iced::advanced::widget::operation::text_input::move_cursor_to_end::<()>(id),
    )
    .discard()
}

/// Open the two-step add-project modal at its pick-source step.
pub fn open(app: &mut App, state: &mut Option<AddProjectState>) {
    *state = Some(AddProjectState {
        step: AddProjectStep::PickSource,
        path: "~/".into(),
        dir_sel: 0,
        name: String::new(),
        git: GitProbe::NotRepo,
        init_git: true,
        note: None,
    });
    app.modal = Modal::AddProject;
}

fn note(state: &mut Option<AddProjectState>, msg: String) {
    if let Some(st) = state.as_mut() {
        st.note = Some(msg);
    }
}

/// Live edit of the step-1 path buffer.
pub fn set_path(state: &mut Option<AddProjectState>, s: String) {
    if let Some(st) = state.as_mut() {
        if st.step == AddProjectStep::PickSource {
            st.path = s;
            st.dir_sel = 0;
            st.note = None;
        }
    }
}

/// Live edit of the step-2 name field.
pub fn set_name(state: &mut Option<AddProjectState>, s: String) {
    if let Some(st) = state.as_mut() {
        st.name = s;
        st.note = None;
    }
}

pub fn dir_move(state: &mut Option<AddProjectState>, delta: i32) {
    let Some(st) = state.as_mut() else {
        return;
    };
    if st.step != AddProjectStep::PickSource {
        return;
    }
    let entries = crate::app::list_dirs(&st.path);
    if entries.is_empty() {
        st.dir_sel = 0;
        return;
    }
    st.dir_sel = crate::app::cycle(st.dir_sel, delta, entries.len());
}

pub fn dir_pick(state: &mut Option<AddProjectState>) {
    let Some(st) = state.as_mut() else {
        return;
    };
    if st.step != AddProjectStep::PickSource {
        return;
    }
    let entries = crate::app::list_dirs(&st.path);
    if let Some(pick) = entries.get(st.dir_sel) {
        st.path = format!("{pick}/");
        st.dir_sel = 0;
    }
}

/// Step-1 Enter: feed the typed buffer into the choose funnel. Guarded to
/// the pick-source step so a doubled Enter (the text_input's on_submit plus
/// the key subscription) can't fall through and submit the details step.
pub fn choose_typed(state: &mut Option<AddProjectState>) {
    let Some(st) = state.as_ref() else {
        return;
    };
    if st.step != AddProjectStep::PickSource {
        return;
    }
    let pb = std::path::PathBuf::from(crate::app::shellexpand_tilde(st.path.trim()));
    choose(state, pb);
}

/// Single funnel for all three folder sources (native picker, drop, typed
/// path): validate, canonicalize, probe git upfront, and advance to the
/// details step. On failure an inline note is set and the step stays put.
pub fn choose(state: &mut Option<AddProjectState>, pb: std::path::PathBuf) {
    if state.is_none() {
        return;
    }
    if !pb.is_dir() {
        note(state, "not a folder; choose a directory".into());
        return;
    }
    let abs = match fs::canonicalize(&pb) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(e) => {
            note(state, format!("cannot resolve path: {e}"));
            return;
        }
    };
    let probe = if git::is_repo(&abs) {
        GitProbe::Repo {
            branch: git::current_branch(&abs),
        }
    } else {
        GitProbe::NotRepo
    };
    if let Some(st) = state.as_mut() {
        st.step = AddProjectStep::Details;
        st.path = abs;
        st.git = probe;
        st.note = None;
    }
}

/// "change" from the details step: back to pick-source with the buffer
/// primed to the current folder. The (possibly edited) name is kept so a
/// round-trip doesn't lose it.
pub fn change_source(state: &mut Option<AddProjectState>) {
    if let Some(st) = state.as_mut() {
        st.step = AddProjectStep::PickSource;
        st.note = None;
    }
}

/// Final submit from the details step: validate, optionally `git init`, then
/// register the project. Nothing is persisted until every check has passed.
/// Always closes the wizard (`state` cleared) once it hands off to
/// `App::register_project` — on that call's own failure the modal is swapped
/// to a plain error message, mirroring the pre-extraction
/// `submit_add_project`'s `Err` propagation to its caller.
pub fn submit(app: &mut App, state: &mut Option<AddProjectState>) {
    let Some((path, name, git, init_git)) = state.as_ref().and_then(|st| {
        if st.step != AddProjectStep::Details {
            return None;
        }
        Some((
            st.path.clone(),
            st.name.trim().to_string(),
            st.git.clone(),
            st.init_git,
        ))
    }) else {
        return;
    };
    // The name field is a pure override: left empty, the folder's basename
    // is used (mirrored by the field's placeholder in the view).
    let name = if name.is_empty() {
        crate::app::path_basename(&path)
    } else {
        name
    };
    if name.is_empty() {
        note(state, "name required".into());
        return;
    }
    if app.store.projects.iter().any(|p| p.name == name) {
        note(state, format!("project '{name}' already exists"));
        return;
    }
    if let Some(p) = app.store.projects.iter().find(|p| p.path == path) {
        note(state, format!("folder already added as '{}'", p.name));
        return;
    }
    if matches!(git, GitProbe::NotRepo) && init_git {
        if let Err(e) = git::init_if_needed(&path) {
            note(state, format!("git init failed: {e}"));
            return;
        }
    }
    app.modal = Modal::None;
    *state = None;
    if let Err(e) = app.register_project(name, path) {
        app.modal = Modal::Message(format!("Add project failed: {e}"));
    }
}

/// After a choose-funnel attempt, focus whichever add-project field is now
/// primary: the name field once the details step is showing, else the
/// step-1 path input (the funnel rejected the folder).
pub fn focus_field(state: &Option<AddProjectState>) -> Task<Msg> {
    match state.as_ref().map(|st| st.step) {
        Some(AddProjectStep::Details) => focus(modal_name_id()),
        Some(AddProjectStep::PickSource) => focus(modal_input_id()),
        None => Task::none(),
    }
}

/// Handles every `Msg` variant except `Open`, which the parent intercepts
/// before calling this function (see `Msg`'s doc comment). Returns whether
/// the parent must call `Grove::rebuild_wt_cache` — only `Submit` does,
/// unconditionally (success or failure), mirroring the pre-extraction
/// `Msg::AddProjectSubmit` handler.
pub fn update(
    app: &mut App,
    state: &mut Option<AddProjectState>,
    msg: Msg,
) -> (Task<Msg>, WtCacheRebuild) {
    match msg {
        // Unreachable in practice — the parent intercepts `Open` before
        // calling `update`. Kept for match exhaustiveness.
        Msg::Open => (Task::none(), WtCacheRebuild::Skip),
        Msg::PathChanged(s) => {
            set_path(state, s);
            (Task::none(), WtCacheRebuild::Skip)
        }
        Msg::ChooseTyped => {
            choose_typed(state);
            (focus_field(state), WtCacheRebuild::Skip)
        }
        Msg::NameChanged(s) => {
            set_name(state, s);
            (Task::none(), WtCacheRebuild::Skip)
        }
        Msg::ChangeSource => {
            change_source(state);
            (focus(modal_input_id()), WtCacheRebuild::Skip)
        }
        Msg::ToggleInitGit(v) => {
            if let Some(st) = state.as_mut() {
                st.init_git = v;
            }
            (Task::none(), WtCacheRebuild::Skip)
        }
        Msg::Submit => {
            submit(app, state);
            (Task::none(), WtCacheRebuild::Rebuild)
        }
        Msg::PickDir(path) => {
            let is_pick_source = matches!(
                state.as_ref().map(|st| st.step),
                Some(AddProjectStep::PickSource)
            );
            if is_pick_source {
                set_path(state, format!("{path}/"));
                (move_cursor_to_end(modal_input_id()), WtCacheRebuild::Skip)
            } else {
                (Task::none(), WtCacheRebuild::Skip)
            }
        }
    }
}

/// Key handling for `Modal::AddProject` — the non-cancel branch of
/// `handle_modal_key`'s `Modal::AddProject` arm, extracted verbatim. Esc
/// from the pick-source step and Ctrl+C from either step both cancel the
/// whole modal via the shared `cancel_modal` (a `Grove` method that also
/// clears `Grove::add_project`), so the parent checks those two cases first
/// and only reaches this function for everything else — same convention as
/// `theme_manager_editor::handle_key`'s "checked first" note.
pub fn handle_key(
    app: &mut App,
    state: &mut Option<AddProjectState>,
    key: Key,
) -> (Task<Msg>, WtCacheRebuild) {
    let Some(step) = state.as_ref().map(|st| st.step) else {
        return (Task::none(), WtCacheRebuild::Skip);
    };
    match step {
        AddProjectStep::PickSource => match key {
            Key::Named(Named::Enter) => {
                choose_typed(state);
                (focus_field(state), WtCacheRebuild::Skip)
            }
            Key::Named(Named::ArrowDown) => {
                dir_move(state, 1);
                (Task::none(), WtCacheRebuild::Skip)
            }
            Key::Named(Named::ArrowUp) => {
                dir_move(state, -1);
                (Task::none(), WtCacheRebuild::Skip)
            }
            Key::Named(Named::Tab) => {
                // Tab completes the path in the buffer; move the caret to
                // the end so subsequent typing appends instead of
                // inserting where the caret happened to sit before
                // completion.
                dir_pick(state);
                (move_cursor_to_end(modal_input_id()), WtCacheRebuild::Skip)
            }
            _ => (Task::none(), WtCacheRebuild::Skip),
        },
        AddProjectStep::Details => match key {
            // Esc is a cheap undo back to pick-source; a second Esc (from
            // step 1) cancels the modal outright (handled by the parent).
            Key::Named(Named::Escape) => {
                change_source(state);
                (focus(modal_input_id()), WtCacheRebuild::Skip)
            }
            Key::Named(Named::Enter) => {
                submit(app, state);
                (Task::none(), WtCacheRebuild::Rebuild)
            }
            _ => (Task::none(), WtCacheRebuild::Skip),
        },
    }
}

/// `dir_matches`' `on_pick: fn(String) -> Msg` requires an actual function
/// pointer (not a closure) — this composes `GMsg::AddProject` with
/// `Msg::PickDir` the way the bare `Msg::ModalPickDir` tuple-variant
/// constructor did before extraction.
fn pick_dir_msg(path: String) -> GMsg {
    GMsg::AddProject(Msg::PickDir(path))
}

/// `modal_checkbox`'s `on_toggle: Option<fn(bool) -> Msg>` has the same
/// function-pointer constraint as `pick_dir_msg`.
fn toggle_init_git_msg(v: bool) -> GMsg {
    GMsg::AddProject(Msg::ToggleInitGit(v))
}

/// The two-step add-project modal: pick a folder (native picker, drop, or
/// typed path), then confirm the details with the git probe inline.
pub fn view(grove: &Grove) -> Element<'_, GMsg> {
    let Some(st) = &grove.add_project else {
        return Space::new().width(0).into();
    };
    let accent = c::MAGENTA();
    let step_no = match st.step {
        AddProjectStep::PickSource => 1,
        AddProjectStep::Details => 2,
    };
    let header = modal_header_row(
        row![
            text("Add project").size(13).color(accent),
            Space::new().width(Length::Fill),
            text(format!("Step {step_no} of 2"))
                .size(11)
                .color(c::FG_MUTE()),
        ]
        .align_y(iced::Alignment::Center)
        .into(),
    );

    let mut body = column![].spacing(12);
    #[allow(unused_assignments)]
    let mut footer: Option<Element<'_, GMsg>> = None;

    match st.step {
        AddProjectStep::PickSource => {
            // Hero action: a full-width primary Browse button with the
            // drop affordance as its caption.
            let accent_soft = Color { a: 0.45, ..accent };
            let browse = button(
                container(
                    text(if grove.picker_open {
                        "Waiting for the folder picker…"
                    } else {
                        "Browse for folder…"
                    })
                    .size(13),
                )
                .width(Length::Fill)
                .align_x(iced::Alignment::Center),
            )
            .on_press(GMsg::AddProjectBrowse)
            .width(Length::Fill)
            .padding(Padding::from([10, 12]))
            .style(move |_, status| {
                let hovered = matches!(status, button::Status::Hovered);
                button::Style {
                    background: Some(Background::Color(if hovered {
                        c::BG_HOVER()
                    } else {
                        c::BG_HL()
                    })),
                    text_color: c::FG(),
                    border: Border {
                        color: if hovered { accent } else { accent_soft },
                        width: 1.0,
                        radius: Radius::from(5.0),
                    },
                    shadow: Shadow::default(),
                    snap: false,
                }
            });
            let drop_hint = container(
                text("Or drop a folder anywhere in this window")
                    .size(11)
                    .color(c::FG_MUTE()),
            )
            .width(Length::Fill)
            .align_x(iced::Alignment::Center);

            let or_divider = row![
                container(divider_h(c::BORDER_SOFT())).width(Length::Fill),
                text("Or type a path").size(11).color(c::FG_MUTE()),
                container(divider_h(c::BORDER_SOFT())).width(Length::Fill),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center);

            let path_input = text_input("~/code/my-repo", &st.path)
                .id(modal_input_id())
                .font(UI_FONT)
                .size(13)
                .padding(Padding::from([8, 12]))
                .on_input(|s| GMsg::AddProject(Msg::PathChanged(s)))
                .on_submit(GMsg::AddProject(Msg::ChooseTyped))
                .style(input_field_style);

            body = body
                .push(Space::new().height(2))
                .push(browse)
                .push(drop_hint)
                .push(Space::new().height(2))
                .push(or_divider)
                .push(path_input)
                .push(grove.dir_matches(&st.path, st.dir_sel, 6, pick_dir_msg));

            if let Some(note) = &st.note {
                body = body.push(text(note.clone()).size(12).color(c::RED()));
            }
            body = body.push(
                row![
                    Space::new().width(Length::Fill),
                    modal_action("Cancel", ModalBtn::Plain, GMsg::ModalCancel),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
            footer = Some(modal_footer_hints(&[
                ("tab", "complete"),
                ("↑↓", "select"),
                ("⏎", "continue"),
                ("esc", "cancel"),
            ]));
        }
        AddProjectStep::Details => {
            let chip = container(
                row![
                    icon("folder", 14.0, c::FG_DIM()),
                    text(st.path.clone())
                        .size(12)
                        .color(c::FG())
                        .wrapping(iced::widget::text::Wrapping::None),
                    Space::new().width(Length::Fill),
                    modal_action(
                        "Change",
                        ModalBtn::Plain,
                        GMsg::AddProject(Msg::ChangeSource)
                    ),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            )
            .width(Length::Fill)
            .padding(Padding::from([6, 10]))
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP())),
                border: Border {
                    color: c::BORDER(),
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                ..Default::default()
            });

            let badge: Element<'_, GMsg> = match &st.git {
                GitProbe::Repo { branch } => row![
                    icon("git", 14.0, c::GREEN()),
                    text(format!("Git repository · branch {branch}"))
                        .size(12)
                        .color(c::GREEN()),
                ]
                .spacing(7)
                .align_y(iced::Alignment::Center)
                .into(),
                GitProbe::NotRepo => row![
                    icon("no-git", 14.0, c::AMBER()),
                    text("Not a git repository").size(12).color(c::AMBER()),
                ]
                .spacing(7)
                .align_y(iced::Alignment::Center)
                .into(),
            };

            // The placeholder is the default (folder basename); typing
            // overrides it without having to clear pre-filled text.
            let default_name = crate::app::path_basename(&st.path);
            let name_input = text_input(&default_name, &st.name)
                .id(modal_name_id())
                .font(UI_FONT)
                .size(13)
                .padding(Padding::from([8, 12]))
                .on_input(|s| GMsg::AddProject(Msg::NameChanged(s)))
                .on_submit(GMsg::AddProject(Msg::Submit))
                .style(input_field_style);

            body = body
                .push(text("Folder").size(11).color(c::FG_MUTE()))
                .push(chip)
                .push(badge)
                .push(
                    row![
                        text("Name").size(11).color(c::FG_MUTE()),
                        Space::new().width(Length::Fill),
                        text(format!("Empty uses '{default_name}'"))
                            .size(11)
                            .color(c::FG_MUTE()),
                    ]
                    .align_y(iced::Alignment::Center),
                )
                .push(name_input);

            if matches!(st.git, GitProbe::NotRepo) {
                body = body.push(modal_checkbox(
                    "Initialize Git repository".into(),
                    st.init_git,
                    accent,
                    Some(toggle_init_git_msg),
                ));
                if !st.init_git {
                    body = body.push(
                        text("Sessions will run directly in the project folder, no worktrees")
                            .size(11)
                            .color(c::FG_MUTE()),
                    );
                }
            }
            if let Some(note) = &st.note {
                body = body.push(text(note.clone()).size(12).color(c::RED()));
            }
            body = body.push(
                row![
                    Space::new().width(Length::Fill),
                    modal_action("Cancel", ModalBtn::Plain, GMsg::ModalCancel),
                    modal_action(
                        "Add project",
                        ModalBtn::Primary,
                        GMsg::AddProject(Msg::Submit)
                    ),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
            footer = Some(modal_footer_hints(&[("⏎", "add"), ("esc", "back")]));
        }
    }

    let mut panel_body = column![
        header,
        divider_h(c::BORDER_SOFT()),
        container(body).padding(Padding::from([14, 16])),
    ];
    if let Some(footer) = footer {
        panel_body = panel_body.push(divider_h(c::BORDER_SOFT())).push(footer);
    }

    modal_panel(panel_body.into(), 640.0)
}
