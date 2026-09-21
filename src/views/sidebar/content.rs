//! Main canvas driven by the sidebar's stable selection.
use super::{Selection, Sidebar, ViewMode};
use crate::{
    settings::SettingsState,
    theme as c,
    views::{
        components::{form_action, form_field},
        rpx,
        terminal_view::TerminalView,
        tokens::*,
    },
};
use gpui::{div, prelude::*, AnyElement, Context, Focusable, FontWeight, Window};
use gpui_component::Disableable;

const FORM_ACTION_H: f32 = 44.;
const FORM_PRIMARY_MIN_W: f32 = 130.;
const FORM_SECONDARY_MIN_W: f32 = 100.;
const GRID_TILE_MIN_H: f32 = 160.;
const GRID_TILE_MIN_W: f32 = 320.;
const GRID_MAX_COLUMNS: usize = 3;

impl Sidebar {
    pub(super) fn begin_worktree(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.worktree_return_focus.is_none() {
            self.worktree_return_focus = window.focused(cx);
        }
        self.pending_new_worktree = Some(idx);
        self.content_error = None;
        self.worktree_errors = [None, None, None];
        self.worktree_name.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.worktree_branch
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.worktree_base
            .update(cx, |input, cx| input.set_value("HEAD", window, cx));
        cx.notify();
    }
    pub(super) fn submit_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(idx) = self.pending_new_worktree else {
            return;
        };
        let Some(project) = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .cloned()
        else {
            return;
        };
        let name = self.worktree_name.read(cx).value().trim().to_string();
        let branch = self.worktree_branch.read(cx).value().trim().to_string();
        let base = self.worktree_base.read(cx).value().trim().to_string();
        self.worktree_errors = validate_worktree_fields(&name, &branch, &base);
        if self.worktree_errors.iter().any(Option::is_some) {
            cx.notify();
            return;
        }
        let service = self.runtime.read(cx).projects.clone();
        match service.update(cx, |service, cx| {
            service.create_worktree_with_branch(&project, &name, &branch, Some(&base), cx)
        }) {
            Ok(path) => {
                self.pending_new_worktree = None;
                self.content_error = None;
                self.select(Selection::Worktree(idx, path), cx);
            }
            Err(error) => {
                let field = worktree_error_field(&error);
                self.worktree_errors[field] = Some(error);
            }
        }
        cx.notify();
    }
    pub(super) fn add_project(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let workspace = cx.global::<SettingsState>().store.workspaces.active;
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add project".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.first() else {
                return;
            };
            let canonical = fs_err::canonicalize(path).and_then(|path| {
                if path.is_dir() { Ok(path) } else { Err(std::io::Error::other("Select an existing directory.")) }
            });
            let path = match canonical {
                Ok(path) => path.to_string_lossy().into_owned(),
                Err(error) => { let _ = this.update(cx, |this,cx| { this.content_error = Some(error.to_string()); cx.notify(); }); return; }
            };
            let name = crate::add_project::path_basename(&path);
            let _ = this.update(cx, |this, cx| {
                let store = &cx.global::<SettingsState>().store;
                if store.workspaces.active != workspace {
                    this.content_error = Some("Workspace changed while choosing a folder. Add the project again in the intended workspace.".into()); cx.notify(); return;
                }
                if store
                    .projects
                    .iter()
                    .any(|p| p.path == path || p.name == name || fs_err::canonicalize(&p.path).is_ok_and(|existing| existing.to_string_lossy() == path))
                {
                    this.content_error =
                        Some("This project is already added, or its name is in use.".into());
                    cx.notify();
                    return;
                }
                let service = this.runtime.read(cx).projects.clone();
                match service.update(cx, |service, cx| {
                    service.register_project(name, path.clone(), cx)
                }) {
                    Ok(idx) => {
                        SettingsState::update(cx, |store| {
                            store.assign_project_to_active_workspace(&path);
                        });
                        SettingsState::flush_now(cx);
                        this.content_error = None;
                        this.select(Selection::Project(idx), cx);
                    }
                    Err(error) => this.content_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn terminal_content(
        &mut self,
        id: crate::entities::session_registry::SessionId,
        home: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let registry = self.runtime.read(cx).registry.read(cx);
        let workspace = &cx.global::<SettingsState>().store.workspaces;
        let workspace_name = workspace.name(workspace.active);
        let (session, project, label, context) = if home {
            let i = registry
                .home_terminals()
                .iter()
                .position(|meta| meta.id == id)?;
            (
                registry.home_terminal(i)?.clone(),
                None,
                registry.home_terminals()[i].label.clone(),
                format!("{workspace_name} · Standalone terminal · ~"),
            )
        } else {
            let meta = registry.meta(id)?;
            let worktree = self
                .snapshot
                .projects
                .iter()
                .flat_map(|project| &project.worktrees)
                .find(|worktree| worktree.path == meta.wt_path);
            let location = worktree.map_or_else(
                || meta.wt_path.clone(),
                |worktree| format!("{} · {}", worktree.name, worktree.branch),
            );
            (
                registry.session(id)?.clone(),
                Some(meta.project.clone()),
                meta.label.clone(),
                format!("{workspace_name} · {} · {location}", meta.project),
            )
        };
        let views = if home {
            &mut self.home_terminal_views
        } else {
            &mut self.terminal_views
        };
        let newly_created = !views.contains_key(&id);
        let view = views
            .entry(id)
            .or_insert_with(|| cx.new(|cx| TerminalView::new(session, project, cx)))
            .clone();
        if newly_created
            && self.mode != ViewMode::Grid
            && matches!(self.selection, Some(Selection::Session(selected) | Selection::Home(selected)) if selected == id)
        {
            view.focus_handle(cx).focus(window, cx);
        }
        let accessible_label = format!("{label} · {context}");
        Some(
            div()
                .id(gpui::SharedString::from(format!("terminal-pane-{id:?}")))
                .role(gpui::Role::Group)
                .aria_label(accessible_label.clone())
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.select(
                            if home {
                                Selection::Home(id)
                            } else {
                                Selection::Session(id)
                            },
                            cx,
                        );
                    }),
                )
                .flex()
                .flex_col()
                .size_full()
                .min_h_0()
                .child(
                    div()
                        .id(gpui::SharedString::from(format!("terminal-header-{id:?}")))
                        .aria_label(accessible_label.clone())
                        .tooltip(move |window, cx| {
                            gpui_component::tooltip::Tooltip::new(accessible_label.clone())
                                .bg(c::BG_STRIP())
                                .text_color(c::FG())
                                .build(window, cx)
                        })
                        .h(rpx(APPBAR_H))
                        .flex_shrink_0()
                        .min_w_0()
                        .px(rpx(SPACE_3XL))
                        .flex()
                        .items_center()
                        .border_b_1()
                        .border_color(c::BORDER())
                        .text_size(rpx(TEXT_BODY))
                        .gap(rpx(SPACE_LG))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(c::FG_DIM())
                                .child(context),
                        ),
                )
                .child(div().flex_1().min_h_0().child(view))
                .into_any_element(),
        )
    }
    pub(super) fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.pending_new_worktree.is_some() {
            let errors = validate_worktree_fields(
                self.worktree_name.read(cx).value().as_ref(),
                self.worktree_branch.read(cx).value().as_ref(),
                self.worktree_base.read(cx).value().as_ref(),
            );
            let enabled = errors.iter().all(Option::is_none);
            return div()
                .id("worktree-editor-scroll")
                .size_full()
                .min_w_0()
                .min_h_0()
                .overflow_y_scroll()
                .p(rpx(SPACE_3XL))
                .child(
                    div().w_full().min_w_0().flex().justify_center().child(
                        div()
                            .w_full()
                            .max_w(rpx(MODAL_W_SM))
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_2XL))
                            .child(
                                div()
                                    .text_size(rpx(TEXT_DISPLAY))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("New worktree"),
                            )
                            .child(
                                div()
                                    .text_color(c::FG_DIM())
                                    .child("Create a branch and isolated working directory."),
                            )
                            .child(form_field(
                                "worktree-name-field",
                                "Worktree name",
                                &self.worktree_name,
                                self.worktree_errors[0].as_deref(),
                                window,
                                cx,
                            ))
                            .child(form_field(
                                "worktree-branch-field",
                                "Branch name",
                                &self.worktree_branch,
                                self.worktree_errors[1].as_deref(),
                                window,
                                cx,
                            ))
                            .child(form_field(
                                "worktree-base-field",
                                "Base branch",
                                &self.worktree_base,
                                self.worktree_errors[2].as_deref(),
                                window,
                                cx,
                            ))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(rpx(SPACE_LG))
                                    .child(
                                        form_action(
                                            "create-worktree",
                                            "Create worktree",
                                            true,
                                            window,
                                            cx,
                                        )
                                        .h(rpx(FORM_ACTION_H))
                                        .flex_1()
                                        .min_w(rpx(FORM_PRIMARY_MIN_W))
                                        .disabled(!enabled)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.submit_worktree(cx)),
                                        ),
                                    )
                                    .child(
                                        form_action("cancel-worktree", "Cancel", false, window, cx)
                                            .h(rpx(FORM_ACTION_H))
                                            .flex_1()
                                            .min_w(rpx(FORM_SECONDARY_MIN_W))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.act(super::Action::Cancel, window, cx);
                                            })),
                                    ),
                            ),
                    ),
                )
                .into_any_element();
        }
        if self.mode == ViewMode::Grid {
            let mut ids: Vec<_> = self
                .snapshot
                .projects
                .iter()
                .flat_map(|p| p.sessions.iter().copied().map(|id| (id, false)))
                .collect();
            ids.extend(
                self.runtime
                    .read(cx)
                    .registry
                    .read(cx)
                    .home_terminals()
                    .iter()
                    .filter(|meta| {
                        self.terminal_owners.get(&meta.id).copied().unwrap_or(1)
                            == self.active_workspace
                    })
                    .map(|meta| (meta.id, true)),
            );
            if !ids.is_empty() {
                let tiles: Vec<_> = ids
                    .into_iter()
                    .filter_map(|(id, home)| self.terminal_content(id, home, window, cx))
                    .collect();
                let scale = f32::from(window.rem_size()) / crate::zoom::REM_BASE;
                let width = f32::from(window.viewport_size().width) / scale;
                return session_grid(tiles, width);
            }
        } else {
            match self.selection.clone() {
                Some(Selection::Session(id)) => {
                    if let Some(el) = self.terminal_content(id, false, window, cx) {
                        return el;
                    }
                }
                Some(Selection::Home(id)) => {
                    if let Some(el) = self.terminal_content(id, true, window, cx) {
                        return el;
                    }
                }
                _ => {}
            }
        }
        let selected = match self.selection.clone() {
            _ if self.mode == ViewMode::Grid => None,
            Some(Selection::Project(idx) | Selection::Worktree(idx, _)) => Some(idx),
            _ => None,
        };
        let project = selected
            .and_then(|idx| self.snapshot.projects.iter().find(|p| p.idx == idx))
            .cloned();
        let mut body = div()
            .id("canvas-overview")
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .p(rpx(SPACE_3XL))
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL));
        if let Some(project) = project {
            body = body.child(div().text_size(rpx(TEXT_DISPLAY)).font_weight(FontWeight::SEMIBOLD).child(project.name))
                .child(div().text_color(c::FG_DIM()).child("Select a session, or hover or focus a worktree in the sidebar to launch Codex, Claude Code, or Terminal."));
            for wt in project
                .worktrees
                .into_iter()
                .filter(|wt| match &self.selection {
                    Some(Selection::Worktree(_, path)) => path == &wt.path,
                    _ => true,
                })
            {
                body = body.child(
                    div()
                        .min_w_0()
                        .py(rpx(SPACE_LG))
                        .border_b_1()
                        .border_color(c::BORDER_SOFT())
                        .child(div().font_weight(FontWeight::MEDIUM).child(wt.name))
                        .child(
                            div()
                                .text_size(rpx(TEXT_BODY))
                                .text_color(c::FG_DIM())
                                .child(format!("{} · {} sessions", wt.branch, wt.sessions.len())),
                        ),
                );
            }
        } else if self.snapshot.projects.is_empty() {
            body = body
                .child(
                    div()
                        .text_size(rpx(TEXT_DISPLAY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Your workspace starts here"),
                )
                .child(
                    div()
                        .text_color(c::FG_DIM())
                        .child("Add a project, then choose a worktree to start working."),
                )
                .child(
                    div().flex().child(
                        form_action("add-project", "Add project", true, window, cx)
                            .h(rpx(FORM_ACTION_H))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_project(window, cx)),
                            ),
                    ),
                );
        } else {
            body = body.child(div().text_size(rpx(TEXT_DISPLAY)).font_weight(FontWeight::SEMIBOLD).child(if self.mode == ViewMode::Grid { "No sessions yet" } else { "Select a project or session" }))
                .child(div().text_color(c::FG_DIM()).child(if self.mode == ViewMode::Grid { "Switch to Project view and use a worktree’s launch actions to start a session." } else { "Choose a session in the sidebar, or hover or focus a worktree to reveal its launch actions." }));
        }
        body.when_some(self.content_error.clone(), |el, error| {
            el.child(div().text_color(c::FORM_ERROR()).child(error))
        })
        .into_any_element()
    }
}

fn session_grid(tiles: Vec<AnyElement>, width: f32) -> AnyElement {
    let columns = grid_columns(tiles.len(), width);
    let count = tiles.len();
    let mut rows = Vec::new();
    let mut tiles = tiles.into_iter();
    for _ in 0..count.div_ceil(columns) {
        let row_tiles: Vec<_> = tiles.by_ref().take(columns).collect();
        rows.push(
            div()
                .flex_1()
                .min_h(if count <= columns {
                    rpx(0.)
                } else {
                    rpx(GRID_TILE_MIN_H)
                })
                .flex()
                .gap(rpx(SPACE_LG))
                .children(row_tiles.into_iter().map(|tile| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .border_1()
                        .border_color(c::BORDER())
                        .rounded(rpx(RADIUS_GROUP))
                        .overflow_hidden()
                        .child(tile)
                })),
        );
    }
    div()
        .id("session-grid")
        .debug_selector(|| "session-grid".into())
        .size_full()
        .min_w_0()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_LG))
        .p(rpx(SPACE_LG))
        .children(rows)
        .into_any_element()
}

fn validate_worktree_fields(name: &str, branch: &str, base: &str) -> [Option<String>; 3] {
    let name = name.trim();
    let branch = branch.trim();
    let base = base.trim();
    [
        if name.is_empty() {
            Some("Enter a worktree name.".into())
        } else if !grove_core::git::valid_worktree_name(name) {
            Some(
                "Use letters, numbers, dots, hyphens, or underscores for the worktree name.".into(),
            )
        } else {
            None
        },
        if branch.is_empty() {
            Some("Enter a branch name.".into())
        } else if !valid_branch_name(branch) {
            Some("Enter a valid branch name, such as feat/billing-retry.".into())
        } else {
            None
        },
        if base.is_empty() {
            Some("Enter a base branch or revision.".into())
        } else if base.starts_with('-') || base.contains(char::is_whitespace) {
            Some("Enter a valid base branch or revision.".into())
        } else {
            None
        },
    ]
}
fn valid_branch_name(branch: &str) -> bool {
    !branch.is_empty()
        && branch != "@"
        && !branch.starts_with('-')
        && !branch.ends_with('.')
        && !branch.contains("..")
        && !branch.contains("@{")
        && !branch.chars().any(|ch| {
            ch.is_control()
                || ch.is_whitespace()
                || matches!(ch, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
        && branch
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}
fn worktree_error_field(error: &str) -> usize {
    let error = error.to_lowercase();
    if error.contains("rev-parse")
        || error.contains("base")
        || error.contains("invalid reference")
        || error.contains("path")
    {
        2
    } else if error.contains("worktree name") {
        0
    } else {
        usize::from(error.contains("branch"))
    }
}
fn grid_columns(count: usize, width: f32) -> usize {
    if count <= 1 {
        return 1;
    }
    let fit = ((width - SPACE_LG * 2.) / GRID_TILE_MIN_W).floor().max(1.) as usize;
    fit.min((count as f32).sqrt().ceil() as usize)
        .min(count)
        .clamp(1, GRID_MAX_COLUMNS)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid_fills_single_session_and_stacks_at_narrow_widths() {
        assert_eq!(grid_columns(1, 1440.), 1);
        assert_eq!(grid_columns(2, 900.), 2);
        assert_eq!(grid_columns(6, 1200.), 3);
        assert_eq!(grid_columns(4, 320.), 1);
    }
    #[test]
    fn missing_values_report_the_corresponding_field() {
        let errors = validate_worktree_fields("", "", "");
        assert_eq!(errors[0].as_deref(), Some("Enter a worktree name."));
        assert_eq!(errors[1].as_deref(), Some("Enter a branch name."));
        assert_eq!(
            errors[2].as_deref(),
            Some("Enter a base branch or revision.")
        );
        assert!(
            validate_worktree_fields("billing-retry", "feat/billing-retry", "main")
                .iter()
                .all(Option::is_none)
        );
    }
    #[test]
    fn invalid_branch_does_not_enable_submit() {
        for branch in [
            "feat:billing",
            "feat//billing",
            ".hidden",
            "feat/billing.lock",
            "feat/bill ing",
            "feat@{billing",
            "-",
        ] {
            assert!(
                validate_worktree_fields("billing", branch, "main")[1].is_some(),
                "{branch}"
            );
        }
    }
    #[test]
    fn server_errors_attach_to_their_source_field() {
        assert_eq!(
            worktree_error_field("Branch feat/billing already exists."),
            1
        );
        assert_eq!(worktree_error_field("Worktree path /x already exists."), 2);
        assert_eq!(worktree_error_field("git rev-parse --verify failed"), 2);
    }
    struct GridFixture;
    impl gpui::Render for GridFixture {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            session_grid(
                vec![div()
                    .id("single-terminal-probe")
                    .debug_selector(|| "single-terminal-probe".into())
                    .size_full()
                    .into_any_element()],
                f32::from(window.viewport_size().width),
            )
        }
    }
    fn draw(cx: &mut gpui::VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }
    #[gpui::test]
    fn single_grid_session_fills_rendered_canvas_after_resize(cx: &mut gpui::TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| GridFixture);
        for (width, height) in [(1280., 800.), (320., 200.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
            draw(cx);
            let tile = cx.debug_bounds("single-terminal-probe").unwrap();
            assert!(f32::from(tile.size.width) >= width - 24., "{tile:?}");
            assert!(f32::from(tile.size.height) >= height - 24., "{tile:?}");
            assert!(f32::from(tile.right()) <= width, "{tile:?}");
        }
    }
    #[gpui::test]
    fn worktree_form_fields_stay_within_narrow_and_desktop_canvas(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.begin_worktree(0, window, cx));
        });
        for (width, height) in [(1280., 800.), (320., 200.)] {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
            draw(cx);
            for field in [
                "worktree-name-field",
                "worktree-branch-field",
                "worktree-base-field",
            ] {
                let bounds = cx.debug_bounds(field).unwrap();
                assert!(f32::from(bounds.size.width) > 100., "{field}: {bounds:?}");
                assert!(f32::from(bounds.right()) <= width, "{field}: {bounds:?}");
                assert!(f32::from(bounds.left()) >= 0., "{field}: {bounds:?}");
            }
        }
    }
}
