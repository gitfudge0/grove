//! Per-project lifecycle-scripts editor: the modal that edits a project's
//! setup/run/teardown scripts (`Modal::ScriptsEditor` / `Grove::scripts_editor`).

use super::icons::icon;
use super::metrics::ROW_H;
use super::palette as c;
use super::state::{Grove, Msg as GMsg};
use super::widgets::{
    divider_h, ghost_scrollable, modal_action, modal_footer_hints, modal_header, modal_list_row,
    modal_panel, section_header, ModalBtn,
};
use crate::app::{App, Modal};
use iced::border::Radius;
use iced::widget::{column, container, row, text, Space};
use iced::{Background, Border, Element, Length, Padding, Task};

/// Which lifecycle script an `Action` targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptField {
    Setup,
    Run,
    Teardown,
}

/// Live state for the per-project scripts editor overlay. Holds the three
/// `text_editor` buffers (which must persist across frames, so they can't live
/// in the cloneable `Modal`) plus the target project index.
pub struct ScriptsEditorState {
    pub proj: usize,
    pub project_name: String,
    pub setup: iced::widget::text_editor::Content,
    pub run: iced::widget::text_editor::Content,
    pub teardown: iced::widget::text_editor::Content,
}

/// Messages the scripts editor can emit. `OpenProjectThemePicker` is
/// intercepted by the parent (`Grove::update`'s `Msg::Scripts` arm) before it
/// ever reaches `update` here, since opening the theme picker is owned by a
/// not-yet-extracted component; it's still declared here because the child's
/// view is what emits it.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Open the per-project lifecycle-scripts editor.
    Open { proj: usize },
    /// Edit one of the three script buffers in the scripts editor.
    Action(ScriptField, iced::widget::text_editor::Action),
    /// Persist the edited scripts back to the project and close the editor.
    Save,
    /// Close the scripts editor without saving.
    Cancel,
    /// Open the theme picker scoped to one project's pinned "Project theme"
    /// (from the Project Settings modal's "Project theme" row). Handled by
    /// the parent, not by `update` in this module.
    OpenProjectThemePicker { proj: usize },
}

/// Open the per-project lifecycle-scripts editor, seeding the three
/// `text_editor` buffers from the project's stored scripts.
fn open(editor: &mut Option<ScriptsEditorState>, app: &mut App, proj: usize) {
    use iced::widget::text_editor::Content;
    let Some(p) = app.store.projects.get(proj) else {
        return;
    };
    *editor = Some(ScriptsEditorState {
        proj,
        project_name: p.name.clone(),
        setup: Content::with_text(p.scripts.setup.as_deref().unwrap_or("")),
        run: Content::with_text(p.scripts.run.as_deref().unwrap_or("")),
        teardown: Content::with_text(p.scripts.teardown.as_deref().unwrap_or("")),
    });
    app.modal = Modal::ScriptsEditor;
}

/// Persist the edited scripts back to the project and close the editor. An
/// empty/whitespace-only buffer clears that script (stored as `None`).
fn save(editor: &mut Option<ScriptsEditorState>, app: &mut App) {
    let Some(ed) = editor.take() else {
        app.modal = Modal::None;
        return;
    };
    let norm = |t: String| {
        let t = t.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    };
    if let Some(p) = app.store.projects.get_mut(ed.proj) {
        p.scripts.setup = norm(ed.setup.text());
        p.scripts.run = norm(ed.run.text());
        p.scripts.teardown = norm(ed.teardown.text());
    }
    if let Err(e) = grove_core::storage::save(&app.store) {
        app.modal = Modal::Message(format!("Failed to save scripts: {e}"));
        return;
    }
    app.set_toast("saved project scripts");
    app.modal = Modal::None;
}

/// Handles every `Msg` variant except `OpenProjectThemePicker`, which the
/// parent intercepts before calling this function (see `Msg`'s doc comment).
pub fn update(editor: &mut Option<ScriptsEditorState>, app: &mut App, msg: Msg) -> Task<Msg> {
    match msg {
        Msg::Open { proj } => open(editor, app, proj),
        Msg::Action(field, action) => {
            if let Some(ed) = editor.as_mut() {
                match field {
                    ScriptField::Setup => ed.setup.perform(action),
                    ScriptField::Run => ed.run.perform(action),
                    ScriptField::Teardown => ed.teardown.perform(action),
                }
            }
        }
        Msg::Save => save(editor, app),
        // Handled by the parent (`cancel_modal` is shared with other modals);
        // unreachable here in practice, but keep the match exhaustive.
        Msg::Cancel => {}
        Msg::OpenProjectThemePicker { .. } => {}
    }
    Task::none()
}

/// Per-project modal: lifecycle scripts editor plus (new) the "Project
/// theme" row. Still backed by `Modal::ScriptsEditor` / `Grove::scripts_editor`
/// — only the presentation grew a second section.
///
/// Returns the *parent's* `Msg` (wrapped in `Msg::Scripts`/`GMsg::Scripts`)
/// rather than this module's own `Msg`: the shared modal widgets
/// (`modal_action`, `modal_list_row`, `ghost_scrollable`, …) are hardcoded to
/// `super::state::Msg` rather than generic over a message type, so there is
/// no child-`Msg` `Element` to `.map()` at the call site — the wrapping has
/// to happen inline, at each leaf that actually produces a message.
pub fn view(grove: &Grove) -> Element<'_, GMsg> {
    let Some(ed) = &grove.scripts_editor else {
        return Space::new().width(0).into();
    };

    // ── PROJECT THEME ────────────────────────────────────────────────
    let themes_enabled = grove.app.project_themes_enabled();
    let project = grove.app.store.projects.get(ed.proj);
    // The pin itself always stays persisted regardless of the toggle —
    // but while Project themes is off nothing actually applies it, so
    // the displayed value must show "Default" rather than the stale
    // pinned name (which would otherwise look active when it isn't).
    let pinned_name = if themes_enabled {
        project.and_then(|p| p.theme.as_deref())
    } else {
        None
    };
    let value_text = pinned_name.unwrap_or("Default (follow app)");
    let value_color = if pinned_name.is_some() {
        c::CYAN()
    } else {
        c::FG_DIM()
    };

    let theme_row: Element<'_, GMsg> = if themes_enabled {
        modal_list_row(
            row![
                text("Project theme").size(12).color(c::FG()),
                Space::new().width(Length::Fill),
                text(value_text.to_string()).size(12).color(value_color),
                Space::new().width(8),
                icon("chev-right", 12.0, c::FG_MUTE()),
            ]
            .align_y(iced::Alignment::Center),
            false,
            GMsg::Scripts(Msg::OpenProjectThemePicker { proj: ed.proj }),
        )
    } else {
        container(
            row![
                text("Project theme").size(12).color(c::FG_MUTE()),
                Space::new().width(Length::Fill),
                text(value_text.to_string()).size(12).color(c::FG_MUTE()),
            ]
            .align_y(iced::Alignment::Center),
        )
        .height(ROW_H)
        .padding(Padding::from([0, 10]))
        .into()
    };
    let theme_caption = if themes_enabled {
        "Pin every PTY in this project to a specific theme"
    } else {
        "Enable Project themes in Settings to use this"
    };
    let project_theme_section = column![
        section_header("PROJECT THEME", 0.0, 0.0),
        Space::new().height(2),
        theme_row,
        container(text(theme_caption).size(11).color(c::FG_MUTE())).padding(Padding::from([0, 10])),
    ]
    .spacing(4);

    let field = |label: &str, desc: &str, placeholder: &str, content, which: ScriptField| {
        // Shrink height grows the editor with its content (Iced sizes a
        // Shrink text_editor to its measured line count), so it never
        // scrolls internally — the outer scroll area absorbs any overflow.
        let editor = iced::widget::text_editor(content)
            .height(Length::Shrink)
            .font(iced::Font::MONOSPACE)
            .size(12)
            .padding(8)
            .placeholder(placeholder.to_string())
            .style(|_, status| {
                use iced::widget::text_editor::Status;
                // Cyan border on focus mirrors the modal accent and tells the
                // user which field has keyboard focus without relying on color
                // alone (the caret and selection move with it too).
                let border_color = match status {
                    Status::Focused { .. } => c::CYAN(),
                    Status::Hovered => c::BORDER(),
                    _ => c::BORDER_SOFT(),
                };
                iced::widget::text_editor::Style {
                    background: Background::Color(c::BG_STRIP()),
                    border: Border {
                        color: border_color,
                        width: 1.0,
                        radius: Radius::from(4.0),
                    },
                    placeholder: c::FG_MUTE(),
                    value: c::FG(),
                    selection: c::BG_HL(),
                }
            })
            .on_action(move |a| GMsg::Scripts(Msg::Action(which, a)));
        column![
            text(label.to_string()).size(12).color(c::FG()),
            text(desc.to_string())
                .size(11)
                .color(c::FG_MUTE())
                .wrapping(iced::widget::text::Wrapping::Word),
            editor,
        ]
        .spacing(5)
    };

    let fields = column![
        field(
            "Setup",
            "Runs once when a new worktree is created, inside the new worktree's directory. \
             Use it to install dependencies, copy ignored env files, or start the services \
             an agent needs before you begin working.",
            "npm install",
            &ed.setup,
            ScriptField::Setup,
        ),
        field(
            "Run",
            "Runs on demand when you press the play button (worktree row or session header). \
             It opens an interactive terminal tab, so it suits dev servers, test watchers, \
             or any command you want to watch and interact with.",
            "npm run dev",
            &ed.run,
            ScriptField::Run,
        ),
        field(
            "Teardown",
            "Runs when you delete the worktree, before it is removed from disk. Use it to \
             stop services, tear down databases, or clean up anything setup created. \
             Deletion proceeds once it exits.",
            "docker compose down",
            &ed.teardown,
            ScriptField::Teardown,
        ),
    ]
    .spacing(16);

    // The fields size to their content (min-height) and only scroll once
    // they exceed `max_height` — so on a tall enough window no scrollbar
    // appears at all.
    let scroll_area = container(
        ghost_scrollable(container(fields).padding(Padding::from([0, 10]))).height(Length::Shrink),
    )
    .max_height(480.0);

    let header = modal_header(
        &format!("Project Settings — {}", ed.project_name),
        c::CYAN(),
    );

    let body_zone = column![
        project_theme_section,
        column![
            section_header("LIFECYCLE SCRIPTS", 0.0, 0.0),
            Space::new().height(2),
            container(
                text("Shell snippets shared by every worktree of this project, run via $SHELL -lc. Leave a field blank to disable that step.")
                    .size(11)
                    .color(c::FG_MUTE())
                    .wrapping(iced::widget::text::Wrapping::Word),
            )
            .padding(Padding::from([0, 10])),
            Space::new().height(4),
            scroll_area,
        ]
        .spacing(4),
        row![
            Space::new().width(Length::Fill),
            modal_action("Cancel", ModalBtn::Plain, GMsg::Scripts(Msg::Cancel)),
            modal_action("Save", ModalBtn::Primary, GMsg::Scripts(Msg::Save)),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    ]
    .spacing(12);

    let body = column![
        header,
        divider_h(c::BORDER_SOFT()),
        container(body_zone).padding(Padding::from([14, 16])),
        divider_h(c::BORDER_SOFT()),
        modal_footer_hints(&[("esc", "cancel")]),
    ];

    modal_panel(body.into(), 560.0)
}
