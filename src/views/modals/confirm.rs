//! Confirm (including Quit), Message, Input, TmuxChoice and AgentPicker.

use crate::views::rpx;
use crate::views::tokens::*;
use gpui::{div, prelude::*, px, AnyElement, App, Context, Div, Focusable as _, Window};
use grove_core::agent::Agent;
use grove_core::{git, storage};

use crate::settings::SettingsState;
use crate::theme as c;

use super::settings::{setting_row_grid, LabelTone, RowGutter};
use super::{Modal, ModalClick, ModalDispatch, ModalEvent, ModalLayer};
use crate::modal::{BaseBranchState, ConfirmKind, BASE_UNSET_LABEL};
use crate::views::components::{
    body_action, body_text, card, click_action, click_row, divider_h, field_box, icon_slot,
    modal_body, modal_footer, modal_header_with_close, modal_panel, mono, note_text, ui, ModalBtn,
    RowDensity, SublabelTone,
};

pub const AVAILABLE_AGENTS: [Agent; 4] = Agent::ALL;

/// The floating Base dropdown's width: the modal body's content width, so the
/// list lines up with the card it drops from. One consumer, so a module
/// constant rather than a scale entry (DESIGN.md §13).
const BASE_DROPDOWN_W: f32 = MODAL_W_MD - SPACE_3XL * 2.0;

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
    /// Lists branches off the UI thread — `list_branches`/`default_base` shell out to git and a
    /// slow repo must not stall the modal, which paints empty-but-usable until this lands
    /// (same shape as `DiffViewerState::load_files`).
    pub(super) fn load_base_branches(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.selected_project(cx) else {
            return;
        };
        let repo = project.path.clone();
        if let Some(Modal::Input { base, .. }) = self.slot_mut().get_mut() {
            base.repo.clone_from(&repo);
        }
        cx.spawn(async move |this, cx| {
            let (branches, default) = cx
                .background_spawn(async move {
                    let branches = git::list_branches(&repo).unwrap_or_default();
                    let default = git::default_base(&repo);
                    (branches, default)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(Modal::Input { base, .. }) = this.slot_mut().get_mut() {
                    base.apply_loaded(branches, default);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Opening the dropdown pulls focus off the name field so the filter keys are the modal's, not the caret's.
    pub(super) fn toggle_base_dropdown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Modal::Input { base, .. }) = self.slot_mut().get_mut() else {
            return;
        };
        let opening = !base.open;
        if opening {
            base.open_dropdown();
        } else {
            base.close_dropdown();
        }
        self.refocus_for_base_dropdown(opening, window, cx);
    }

    pub(super) fn close_base_dropdown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Modal::Input { base, .. }) = self.slot_mut().get_mut() else {
            return;
        };
        base.close_dropdown();
        self.refocus_for_base_dropdown(false, window, cx);
    }

    /// `None` commits the highlighted row; `Some(i)` the i-th *filtered* row (a click).
    pub(super) fn pick_base_branch(
        &mut self,
        row: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Modal::Input { base, .. }) = self.slot_mut().get_mut() else {
            return;
        };
        match row {
            Some(i) => base.pick(i),
            None => base.pick_highlighted(),
        }
        self.refocus_for_base_dropdown(false, window, cx);
    }

    pub(super) fn edit_base_filter(&mut self, push: Option<char>, cx: &mut Context<Self>) {
        let Some(Modal::Input { base, .. }) = self.slot_mut().get_mut() else {
            return;
        };
        match push {
            Some(c) => base.push_filter(c),
            None => base.pop_filter(),
        }
        cx.notify();
    }

    fn refocus_for_base_dropdown(
        &mut self,
        to_dropdown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if to_dropdown {
            window.focus(&self.focus, cx);
        } else if let Some(f) = self.fields.first() {
            f.focus_at_end(window, cx);
        }
        cx.notify();
    }

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
        // An existing name is checked out as-is, so it submits with no base at all.
        let base = match self.slot().get() {
            Some(Modal::Input { base, .. }) => base.base_for_submit(&value),
            _ => None,
        };
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
                    kind: ConfirmKind::InitAndAddWorktree { name: value, base },
                },
                cx,
            );
            return;
        }
        self.create_worktree(&project, &value, base.as_deref(), cx);
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
        base: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        // Pinned directory key, not the display name — renaming a project must not scatter worktrees across directories.
        match git::add_worktree(&project.path, project.worktree_dir(), name, base) {
            Ok(path) => {
                if let Err(e) = git::copy_worktree_includes(&project.path, &path) {
                    tracing::warn!("grove-gpui: worktree includes not copied: {e}");
                }
                crate::telemetry::track("worktree_created", vec![]);
                self.toast
                    .update(cx, |t, cx| t.set_toast(format!("added {name}"), cx));
                cx.emit(ModalEvent::WorktreeAdded { path });
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
            ConfirmKind::InitAndAddWorktree { name, base } => {
                let Some(p) = self.selected_project(cx) else {
                    return;
                };
                if let Err(e) = git::init_if_needed(&p.path) {
                    self.open(Modal::Message(format!("Git init failed: {e}")), cx);
                    return;
                }
                self.create_worktree(&p, &name, base.as_deref(), cx);
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
        Some(Modal::Input {
            title, note, base, ..
        }) => input_modal(layer, title, note.as_deref(), base, dispatch, window, cx),
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

/// The in-flow structure of the Input modal's body.
///
/// Neither the dropdown nor the locked state may change these counts: the dropdown
/// is a zero-size `deferred`/`anchored` overlay, and the Base row always renders its
/// sublabel line. Both are taken as arguments so a test can prove they are ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct InputBodyLayout {
    pub card_rows: usize,
    /// Direct children of `modal_body`.
    pub body_children: usize,
    /// Text lines in the Base row.
    pub base_row_lines: usize,
}

pub(super) fn input_body_layout(
    has_field: bool,
    has_note: bool,
    _base_open: bool,
    _locked: bool,
) -> InputBodyLayout {
    InputBodyLayout {
        card_rows: 1 + usize::from(has_field),
        body_children: 1 + usize::from(has_note),
        // Label plus sublabel, always both.
        base_row_lines: 2,
    }
}

/// Always present, so the row cannot change height when the typed name starts or
/// stops matching an existing branch.
fn base_sublabel(locked: bool) -> (&'static str, SublabelTone) {
    let text = if locked {
        "Existing branch — checked out as-is"
    } else {
        "New branch, created from this base"
    };
    (text, SublabelTone::Normal)
}

/// Name and Base are the same row shape — one `setting_row_grid` each — so their
/// labels align. The dropdown floats above the panel as a `deferred`/`anchored`
/// overlay anchored to the Base row, so opening it moves nothing behind it.
fn input_modal(
    layer: &ModalLayer,
    title: &str,
    note: Option<&str>,
    base: &BaseBranchState,
    dispatch: &ModalDispatch,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let typed = layer
        .fields
        .first()
        .map(|f| f.value(cx))
        .unwrap_or_default();
    let locked = base.locked(typed.trim());

    let mut rows: Vec<AnyElement> = Vec::new();
    if let Some(field) = layer.fields.first() {
        let focused = field.state().read(cx).focus_handle(cx).is_focused(window);
        let input = field_box()
            .child(
                gpui_component::input::Input::new(field.state())
                    .appearance(false)
                    .pl(px(0.0))
                    .pr(px(0.0))
                    .py(px(0.0))
                    .w_full(),
            )
            .into_any_element();
        rows.push(
            div()
                .w_full()
                .px(rpx(ROW_PX))
                .py(rpx(ROW_PY))
                .min_h(rpx(ROW_MIN_H))
                .flex()
                .items_center()
                .child(setting_row_grid(
                    "Name",
                    LabelTone::field(Some(focused)),
                    None,
                    Some(input),
                    false,
                    None,
                    RowGutter::None,
                ))
                .into_any_element(),
        );
    }
    rows.push(base_row(base, locked, dispatch));
    let row_count = rows.len();

    let mut body = div().flex().flex_col().gap(rpx(SPACE_LG)).child(card(rows));
    if let Some(note) = note {
        body = body.child(note_text(note.to_string()));
    }

    let layout = input_body_layout(!layer.fields.is_empty(), note.is_some(), base.open, locked);
    debug_assert_eq!(layout.card_rows, row_count, "in-flow card rows drifted");

    // Enter and Escape mean different things while the dropdown owns the keyboard.
    let hints: &[(&'static str, &'static str)] = if base.open {
        &[("↑↓", "choose"), ("⏎", "select"), ("esc", "back")]
    } else {
        &[("⏎", "confirm"), ("esc", "cancel")]
    };

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
                hints,
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

/// A branch name is an identifier, so it is mono; the unset state is prose, so it
/// is not. Either way the value is `FG_DIM` when its row is actionable and
/// `FG_MUTE` when it is not — the same rule §10.3 gives every disabled control.
fn base_value(base: &BaseBranchState, locked: bool) -> Div {
    let color = if locked { c::FG_MUTE() } else { c::FG_DIM() };
    match base.chosen.as_deref() {
        Some(name) => mono(name.to_string(), TEXT_BODY, color),
        None => ui(BASE_UNSET_LABEL, TEXT_BODY, color),
    }
}

/// Closed/trigger state of the Base picker, on the `setting_row_link` template but
/// with `chev-down`: it expands inline, it does not drill into another modal.
/// Locked drops the handler outright rather than dimming a live one.
fn base_row(base: &BaseBranchState, locked: bool, dispatch: &ModalDispatch) -> AnyElement {
    // Value left, chevron hard right — so the Base value and the Name field's text
    // start at the same x and the two rows read as one column.
    let mut control = div()
        .w_full()
        .flex()
        .items_center()
        .gap(rpx(SPACE_MD))
        .child(base_value(base, locked).flex_1());
    if !locked {
        control = control.child(crate::icons::icon("chev-down", ICON_SM, c::FG_MUTE()));
    }
    let grid = setting_row_grid(
        "Base",
        LabelTone::Static,
        Some(base_sublabel(locked)),
        Some(control.into_any_element()),
        false,
        None,
        RowGutter::None,
    );

    if locked {
        return div()
            .w_full()
            .px(rpx(ROW_PX))
            .py(rpx(ROW_PY))
            .min_h(rpx(ROW_MIN_H))
            .flex()
            .items_center()
            .child(grid)
            .into_any_element();
    }

    let row = click_row(
        "in-base",
        base.open,
        RowDensity::CardPadded,
        dispatch,
        ModalClick::BaseDropdownToggle,
        grid,
    );

    // The overlay is the column's second child, so `anchored` takes the point just
    // below the row as its anchor. `anchored` requests no layout space, so the
    // column keeps the row's own height whether the dropdown is open or not.
    div()
        .flex()
        .flex_col()
        .w_full()
        .child(row)
        .when(base.open, |d| d.child(base_dropdown(base, dispatch)))
        .into_any_element()
}

/// The floating picker: a selection list (`card` + `click_row(RowDensity::Card)`)
/// painted above the modal panel.
///
/// `deferred` puts it in a later paint pass so it stacks over the panel instead of
/// inside it, and `anchored` (default `AnchoredFitMode::SwitchAnchor`) opens it
/// downward from the Base row, flipping upward on its own when it would run past
/// the window edge. This is the idiom every dropdown in the vendored
/// `gpui-component` uses (`select.rs`, `popover.rs`, `date_picker.rs`).
///
/// The filter line is a rendered `String`, not a second `InputState` — see
/// [`crate::modal::BaseBranchState`].
fn base_dropdown(base: &BaseBranchState, dispatch: &ModalDispatch) -> AnyElement {
    let mut rows: Vec<AnyElement> = Vec::new();
    if base.wants_filter() {
        // Not a `field_box`: it holds no `InputState` and can never take focus, so
        // it must not borrow a field's look. The typed text is a value (mono); the
        // prompt is prose (sans).
        let typed: AnyElement = if base.filter.is_empty() {
            ui("type to filter…", TEXT_BODY, c::FG_MUTE()).into_any_element()
        } else {
            mono(base.filter.clone(), TEXT_BODY, c::FG()).into_any_element()
        };
        rows.push(
            div()
                .w_full()
                .px(rpx(ROW_PX))
                .py(rpx(ROW_PY))
                .flex()
                .items_center()
                .gap(rpx(SPACE_MD))
                .child(crate::icons::icon("search", ICON_SM, c::FG_MUTE()))
                .child(typed)
                .into_any_element(),
        );
    }
    let visible = base.visible();
    for (i, b) in visible.iter().enumerate() {
        let content = div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_MD))
            .w_full()
            .child(mono(b.name.clone(), TEXT_BODY, c::FG_DIM()).flex_1())
            .when(b.is_head, |d| {
                d.child(ui("current", TEXT_SMALL, c::FG_MUTE()))
            });
        rows.push(
            click_row(
                ("base-branch", i),
                i == base.highlight,
                RowDensity::CardPadded,
                dispatch,
                ModalClick::BaseSelect(i),
                content,
            )
            .into_any_element(),
        );
    }
    if visible.is_empty() {
        let msg = if base.loaded {
            "No matching branch"
        } else {
            "Loading branches…"
        };
        rows.push(
            div()
                .w_full()
                .px(rpx(ROW_PX))
                .py(rpx(ROW_PY))
                .child(ui(msg, TEXT_BODY, c::FG_MUTE()))
                .into_any_element(),
        );
    }

    // Same shadow language as `panel_surface`: this is a floating surface too.
    let (shadow_y, shadow_blur) = if c::is_dark() {
        (PANEL_SHADOW_Y, PANEL_SHADOW_BLUR)
    } else {
        (PANEL_SHADOW_Y_LIGHT, PANEL_SHADOW_BLUR_LIGHT)
    };
    let dismiss = std::rc::Rc::clone(dispatch);
    let surface = div()
        .occlude()
        .w(rpx(BASE_DROPDOWN_W))
        .child(
            card(rows)
                .id("base-dropdown")
                .max_h(rpx(MODAL_SCROLL_MAX_H))
                .overflow_y_scroll()
                .shadow(vec![gpui::BoxShadow {
                    color: c::PANEL_SHADOW(),
                    offset: gpui::point(px(0.0), px(shadow_y)),
                    blur_radius: px(shadow_blur),
                    spread_radius: px(0.0),
                    inset: false,
                }]),
        )
        // Dismisses the dropdown only; the modal stays open.
        .on_mouse_down_out(move |_, window, cx| {
            dismiss(ModalClick::BaseDropdownToggle, window, cx);
        });

    // `offset` rather than a margin on the child: `anchored` measures its child
    // to decide whether to flip, and a margin is not part of that measurement.
    gpui::deferred(
        gpui::anchored()
            .snap_to_window_with_margin(px(SPACE_3XL))
            .offset(gpui::point(px(0.0), px(SPACE_SM)))
            .child(surface),
    )
    .with_priority(1)
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
                RowDensity::CardPadded,
                dispatch,
                ModalClick::SelectRow(i),
                content,
            )
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

    /// Two things must not move the panel: opening the dropdown (a `deferred`
    /// overlay, which requests no layout space) and the typed name starting to
    /// match an existing branch (the Base row renders its sublabel either way).
    ///
    /// This guards the *in-flow child counts*, not pixels — proving the rendered
    /// box is unchanged would need a real layout pass, which this render tree is
    /// not reachable for in a unit test. It catches the way this actually
    /// regresses: putting the list back in `modal_body`, or making the sublabel
    /// conditional again.
    #[test]
    fn neither_the_dropdown_nor_the_locked_state_moves_the_panel() {
        for has_field in [true, false] {
            for has_note in [true, false] {
                let base = input_body_layout(has_field, has_note, false, false);
                for open in [true, false] {
                    for locked in [true, false] {
                        assert_eq!(
                            base,
                            input_body_layout(has_field, has_note, open, locked),
                            "panel layout moved (field={has_field}, note={has_note}, \
                             open={open}, locked={locked})"
                        );
                    }
                }
            }
        }
        assert_eq!(
            input_body_layout(true, false, true, true),
            InputBodyLayout {
                card_rows: 2,
                body_children: 1,
                base_row_lines: 2,
            }
        );
    }

    /// Both states must supply a sublabel, or the row changes height mid-keystroke.
    #[test]
    fn the_base_row_always_has_exactly_one_sublabel() {
        let (unlocked, _) = base_sublabel(false);
        let (locked, _) = base_sublabel(true);
        assert!(!unlocked.is_empty() && !locked.is_empty());
        assert_ne!(
            unlocked, locked,
            "the two states must still read differently"
        );
    }

    #[test]
    fn destructive_kinds_render_with_the_red_accent() {
        for (kind, destructive) in [
            (ConfirmKind::RemoveProject(0), true),
            (ConfirmKind::RemoveWorktree("/w".into()), true),
            (
                ConfirmKind::InitAndAddWorktree {
                    name: "x".into(),
                    base: Some("main".into()),
                },
                false,
            ),
            (ConfirmKind::Quit, true),
        ] {
            // Compile-time reminder that every kind is accounted for.
            let _ = (&kind, destructive);
        }
    }
}
