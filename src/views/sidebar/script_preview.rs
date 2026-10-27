//! Checkout-owned shell runs use the same hover, pin and Escape interaction as agent peeks.
use super::*;
use crate::runtime::ScriptRun;
use crate::views::terminal_view::TerminalView;

const PEEK_WIDTH: f32 = 480.;
const PEEK_HEIGHT: f32 = 360.;
const HOVER_INTENT: Duration = Duration::from_millis(220);
const HOVER_GRACE: Duration = Duration::from_millis(260);

pub(super) struct ScriptPeek {
    id: u64,
    selection: Option<Selection>,
    explicit: bool,
    focus: FocusHandle,
    terminal: Entity<TerminalView>,
}

impl Sidebar {
    pub(super) fn dismiss_script_peek(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.checkout_panel.script_peek_task = None;
        if let Some(peek) = self.checkout_panel.script_peek.take() {
            if peek.focus.contains_focused(window, cx)
                || peek.terminal.focus_handle(cx).contains_focused(window, cx)
            {
                if let Some(source) = self.checkout_panel.script_peek_focus.get(&peek.id) {
                    source.focus(window, cx);
                }
            }
        }
        cx.notify();
    }

    fn remove_script_run(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_script_peek(window, cx);
        self.runtime.update(cx, |runtime, cx| {
            if let Some(run) = runtime.script_runs.iter().find(|run| run.id == id) {
                run.terminal.update(
                    cx,
                    crate::entities::terminal_session::TerminalSession::stop_script,
                );
            }
            runtime.script_runs.retain(|run| run.id != id);
            cx.notify();
        });
        self.checkout_panel.script_terminals.remove(&id);
        self.checkout_panel.script_peek_focus.remove(&id);
        self.checkout_panel.script_peek_bounds.remove(&id);
        cx.notify();
    }

    fn open_script_peek(
        &mut self,
        run: ScriptRun,
        explicit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.confirmation_open()
            || self.project_panel.is_some()
            || self.menu.is_some()
            || self.project_setup.is_some()
            || self.pending_new_worktree.is_some()
            || self
                .settings_panel
                .as_ref()
                .is_some_and(|panel| panel.read(cx).is_open())
        {
            return;
        }
        self.dismiss_branch_peek(window, cx);
        self.dismiss_agent_peek(window, cx);
        self.dismiss_plan_peek(window, cx);
        self.checkout_panel.script_peek_task = None;
        let terminal = self
            .checkout_panel
            .script_terminals
            .entry(run.id)
            .or_insert_with(|| cx.new(|cx| TerminalView::new(run.terminal.clone(), cx)))
            .clone();
        if explicit {
            terminal.focus_handle(cx).focus(window, cx);
        }
        self.checkout_panel.script_peek = Some(ScriptPeek {
            id: run.id,
            selection: self.selection.clone(),
            explicit,
            focus: cx.focus_handle(),
            terminal,
        });
        cx.notify();
    }

    fn script_peek_hover(
        &mut self,
        run: ScriptRun,
        hovered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !hovered
            && self
                .checkout_panel
                .script_hover_target
                .is_some_and(|id| id != run.id)
        {
            return;
        }
        self.checkout_panel.script_hover_target = hovered.then_some(run.id);
        self.checkout_panel.script_peek_task = None;
        if self
            .checkout_panel
            .script_peek
            .as_ref()
            .is_some_and(|peek| peek.explicit)
        {
            return;
        }
        let selection = self.selection.clone();
        self.checkout_panel.script_peek_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(if hovered { HOVER_INTENT } else { HOVER_GRACE })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.selection != selection {
                    return;
                }
                if hovered {
                    this.open_script_peek(run, false, window, cx);
                } else {
                    this.checkout_panel.script_peek = None;
                    cx.notify();
                }
            });
        }));
    }

    pub(super) fn script_run_row(
        &mut self,
        run: ScriptRun,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let id = run.id;
        let terminal = run.terminal.read(cx);
        let (kind, label) = if !terminal.has_exited() {
            (
                super::super::status::StatusKind::Working,
                "Running".to_string(),
            )
        } else if terminal.exit_code() == Some(0) {
            (
                super::super::status::StatusKind::Success,
                "Exited · 0".to_string(),
            )
        } else {
            (
                super::super::status::StatusKind::Failed,
                terminal
                    .exit_code()
                    .map_or("Exited".into(), |code| format!("Exited · {code}")),
            )
        };
        let focus = self
            .checkout_panel
            .script_peek_focus
            .entry(id)
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let bounds = self
            .checkout_panel
            .script_peek_bounds
            .entry(id)
            .or_default()
            .clone();
        let hover_run = run.clone();
        let click_run = run.clone();
        div()
            .id(SharedString::from(format!("script-run-{id}")))
            .debug_selector(move || format!("script-run-{id}"))
            .role(gpui::Role::Button)
            .aria_label(format!("Run {} · {label} · Preview output", id + 1))
            .tab_index(0)
            .track_focus(&focus)
            .relative()
            .cursor_pointer()
            .ml(rpx(SPACE_3XL))
            .px(rpx(SPACE_2XL))
            .h(rpx(CONTROL_H))
            .flex()
            .items_center()
            .gap(rpx(SPACE_SM))
            .text_size(rpx(TEXT_SMALL))
            .hover(|style| style.bg(c::BG_HOVER()))
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .child(
                gpui::canvas(move |rect, _, _| bounds.set(rect), |_, (), _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .child(
                div()
                    .text_color(c::FG_DIM())
                    .child(format!("Run {}", id + 1)),
            )
            .child(super::super::status::label(
                SharedString::from(format!("script-run-status-{id}")),
                kind,
                label,
                ICON_SM,
                cx,
            ))
            .child(div().flex_1())
            .child(
                super::super::components::header_control_frame(
                    SharedString::from(format!("script-run-close-{id}")),
                    "Stop and remove run",
                    c::BG_HOVER(),
                )
                .tab_index(0)
                .focus_visible(|style| style.bg(c::BG_HOVER()))
                .child(icon("close", ICON_SM, c::FG_DIM()))
                .on_key_down(
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            this.remove_script_run(id, window, cx);
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    }),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.remove_script_run(id, window, cx);
                })),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                this.script_peek_hover(hover_run.clone(), *hovered, window, cx);
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_script_peek(click_run.clone(), true, window, cx);
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.open_script_peek(run.clone(), true, window, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }),
            )
    }

    pub(super) fn render_script_peek(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let retained = self
            .runtime
            .read(cx)
            .script_runs
            .iter()
            .map(|run| run.id)
            .collect::<std::collections::HashSet<_>>();
        self.checkout_panel
            .script_terminals
            .retain(|id, _| retained.contains(id));
        self.checkout_panel
            .script_peek_focus
            .retain(|id, _| retained.contains(id));
        self.checkout_panel
            .script_peek_bounds
            .retain(|id, _| retained.contains(id));
        let peek = self.checkout_panel.script_peek.as_ref()?;
        let Some(run) = self
            .runtime
            .read(cx)
            .script_runs
            .iter()
            .find(|run| run.id == peek.id)
            .cloned()
        else {
            self.checkout_panel.script_peek = None;
            self.checkout_panel.script_peek_task = None;
            return None;
        };
        let valid = peek.selection == self.selection
            && self
                .selected_worktree()
                .is_some_and(|(_, path)| path == run.checkout_path)
            && cx
                .global::<SettingsState>()
                .store
                .active_projects()
                .any(|(_, project)| {
                    project.path == run.project_path
                        && project
                            .scripts
                            .effective_entries()
                            .iter()
                            .any(|entry| entry.id == run.script_id)
                })
            && self.project_panel.is_none()
            && self.project_setup.is_none()
            && self.pending_new_worktree.is_none()
            && !self.confirmation_open()
            && self.menu.is_none()
            && self
                .settings_panel
                .as_ref()
                .is_none_or(|panel| !panel.read(cx).is_open());
        if !valid {
            self.checkout_panel.script_peek = None;
            self.checkout_panel.script_peek_task = None;
            return None;
        }
        let terminal = peek.terminal.clone();
        let focus = peek.focus.clone();
        let source = self.checkout_panel.script_peek_bounds.get(&run.id)?.get();
        let margin = rpx(SPACE_LG).to_pixels(window.rem_size());
        let width = rpx(PEEK_WIDTH)
            .to_pixels(window.rem_size())
            .min(window.viewport_size().width - margin * 2.);
        let height = rpx(PEEK_HEIGHT)
            .to_pixels(window.rem_size())
            .min(window.viewport_size().height - margin * 2.);
        let hover_run = run.clone();
        let panel = div()
            .id("script-run-peek")
            .debug_selector(|| "script-run-peek".into())
            .role(gpui::Role::Dialog)
            .track_focus(&focus)
            .aria_label("Script run output preview")
            .occlude()
            .w(width)
            .h(height)
            .p(rpx(SPACE_3XL))
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .rounded(rpx(RADIUS_PANEL))
            .border_1()
            .border_color(c::BORDER_SOFT())
            .bg(c::SURFACE_RAISED())
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                if *hovered {
                    this.checkout_panel.script_peek_task = None;
                } else {
                    this.script_peek_hover(hover_run.clone(), false, window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    if let Some(peek) = this.checkout_panel.script_peek.as_mut() {
                        peek.explicit = true;
                    }
                }),
            )
            .on_mouse_down_out(
                cx.listener(|this, _, window, cx| this.dismiss_script_peek(window, cx)),
            )
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.dismiss_script_peek(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().min_w_0().truncate().child(format!(
                        "{} · Run {}",
                        run.name,
                        run.id + 1
                    )))
                    .child(
                        super::super::components::header_control(
                            "script-peek-close",
                            "Close script preview",
                        )
                        .tab_index(0)
                        .focus_visible(|style| style.bg(c::BG_HOVER()))
                        .child(icon("close", ICON_SM, c::FG_DIM()))
                        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                this.dismiss_script_peek(window, cx);
                                window.prevent_default();
                                cx.stop_propagation();
                            }
                        }))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.dismiss_script_peek(window, cx)),
                        ),
                    ),
            )
            .child(
                div()
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .truncate()
                    .child(run.command.clone()),
            )
            .child(div().flex_1().min_h_0().overflow_hidden().child(terminal));
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position_mode(gpui::AnchoredPositionMode::Window)
                    .anchor(gpui::Anchor::TopRight)
                    .position(gpui::point(source.left() - margin, source.top()))
                    .snap_to_window_with_margin(margin)
                    .child(motion::fast(panel, format!("script-peek-{}", run.id), cx)),
            )
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn script_peek_pins_restores_focus_and_rejects_changed_checkout(cx: &mut gpui::TestAppContext) {
        let repo = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Grove",
                "-c",
                "user.email=grove@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "fixture"
            ])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success());
        let path = repo
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "Script preview".into(),
                    path: path.clone(),
                    archived: false,
                    worktree_dir: None,
                    scripts: grove_core::storage::ProjectScripts {
                        run: Some("sleep 30".into()),
                        ..Default::default()
                    },
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.));
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.select(Selection::Worktree(0, path.clone()), cx);
                sidebar.runtime.update(cx, |runtime, cx| {
                    assert!(runtime.spawn_run_script(&path, &path, cx));
                });
                let run = sidebar.runtime.read(cx).script_runs[0].clone();
                let source = cx.focus_handle();
                source.focus(window, cx);
                sidebar
                    .checkout_panel
                    .script_peek_focus
                    .insert(run.id, source.clone());
                sidebar
                    .checkout_panel
                    .script_peek_bounds
                    .entry(run.id)
                    .or_default();
                sidebar.open_script_peek(run.clone(), false, window, cx);
                assert!(
                    !sidebar
                        .checkout_panel
                        .script_peek
                        .as_ref()
                        .unwrap()
                        .explicit
                );
                assert!(source.is_focused(window));
                sidebar.open_script_peek(run.clone(), true, window, cx);
                assert!(
                    sidebar
                        .checkout_panel
                        .script_peek
                        .as_ref()
                        .unwrap()
                        .explicit
                );
                assert!(sidebar
                    .checkout_panel
                    .script_peek
                    .as_ref()
                    .unwrap()
                    .terminal
                    .focus_handle(cx)
                    .is_focused(window));
                sidebar.dismiss_script_peek(window, cx);
                assert!(source.is_focused(window));
                sidebar.open_script_peek(run.clone(), true, window, cx);
                // The panel scope also owns header controls, outside TerminalView.
                sidebar
                    .checkout_panel
                    .script_peek
                    .as_ref()
                    .unwrap()
                    .focus
                    .focus(window, cx);
                sidebar.dismiss_script_peek(window, cx);
                assert!(source.is_focused(window));
                sidebar.open_script_peek(run.clone(), true, window, cx);
                sidebar.selection = Some(Selection::Project(0));
                assert!(sidebar.render_script_peek(window, cx).is_none());
                assert!(sidebar.checkout_panel.script_peek.is_none());
                run.terminal.update(
                    cx,
                    crate::entities::terminal_session::TerminalSession::stop_script,
                );
            });
        });
    }
}
