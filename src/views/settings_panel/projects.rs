use super::{Category, Page, SettingsPanel, SettingsPanelEvent};
use crate::{
    icons::icon,
    settings::SettingsState,
    theme as c,
    views::{components, rpx, sidebar::projects::Page as ProjectPage, tokens::*},
};
use gpui::{
    div, prelude::*, App, Bounds, Context, DismissEvent, Div, FocusHandle, Focusable, FontWeight,
    KeyDownEvent, Pixels, Point, Size, Stateful, Subscription, Task, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::ElementExt as _;
use grove_core::{git::Worktree, storage::Project};
use std::{
    cell::Cell,
    collections::HashSet,
    path::{Path, MAIN_SEPARATOR},
    rc::Rc,
};

const PROJECT_NAME_LINE_H: f32 = TEXT_BODY + SPACE_SM;
const PROJECT_META_LINE_H: f32 = TEXT_SMALL + SPACE_SM;
const WORKTREE_INDENT: f32 = SPACE_20 + SPACE_MD;

#[derive(Clone, Eq, Hash, PartialEq)]
pub(super) struct ProjectMenuTarget {
    project: String,
    worktree: Option<String>,
}

impl ProjectMenuTarget {
    fn project(path: &str) -> Self {
        Self {
            project: path.to_owned(),
            worktree: None,
        }
    }

    fn worktree(project: &str, path: &str) -> Self {
        Self {
            project: project.to_owned(),
            worktree: Some(path.to_owned()),
        }
    }
}

pub(super) struct ProjectActionReturn {
    target: ProjectMenuTarget,
    scroll_offset: Point<Pixels>,
}

pub(super) struct ProjectRowMenu {
    target: ProjectMenuTarget,
    position: Point<Pixels>,
    anchor_bounds: Option<Bounds<Pixels>>,
    viewport_size: Size<Pixels>,
    rem_size: Pixels,
    scroll_offset: Point<Pixels>,
    popup: gpui::Entity<PopupMenu>,
    _dismiss: Subscription,
}

#[derive(Clone, Copy)]
enum RowAction {
    Edit,
    Move,
    Restore,
    NewWorktree,
    UseMain,
    UseWorktree,
    Remove,
    Delete,
}

impl RowAction {
    fn label(self) -> &'static str {
        match self {
            Self::Edit => "Edit project…",
            Self::Move => "Move to workspace…",
            Self::Restore => "Restore project",
            Self::NewWorktree => "New worktree…",
            Self::UseMain => "Use main checkout",
            Self::UseWorktree => "Use worktree",
            Self::Remove => "Remove project…",
            Self::Delete => "Delete worktree…",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Move => "folder",
            Self::Restore => "restore",
            Self::NewWorktree => "plus",
            Self::UseMain | Self::UseWorktree => "check",
            Self::Remove | Self::Delete => "trash",
        }
    }
}

pub(super) struct ProjectReveal {
    path: String,
    anchor: gpui::ScrollAnchor,
    pending: Rc<Cell<bool>>,
}

pub(super) enum WorktreeInventory {
    Loading { _task: Task<()> },
    Loaded(Result<Vec<Worktree>, String>),
}

impl SettingsPanel {
    pub(super) fn project_receipt(&self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.runtime.read(cx).toast.clone().update(cx, |toast, cx| {
            toast.set_toast(message, cx);
        });
    }

    pub(crate) fn open_project_worktrees(
        &mut self,
        path: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project_operation_pending(cx) {
            return;
        }
        self.open_projects(None, window, cx);
        self.projects_archived = false;
        let store = &cx.global::<SettingsState>().store;
        let project = path.and_then(|path| {
            store
                .workspace_projects(store.workspaces.active)
                .find(|(_, project)| project.path == path)
                .map(|(_, project)| project.path.clone())
        });
        if let Some(path) = project {
            self.expanded_projects.insert(path.clone());
            self.reveal_project(&path, window, cx);
            if !self.project_inventory.contains_key(&path) {
                self.discover_worktrees(path, cx);
            }
        } else {
            self.error =
                Some("The selected project is no longer available in this workspace.".into());
        }
        cx.notify();
    }

    fn reveal_project(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.project_reveal = Some(ProjectReveal {
            path: path.to_owned(),
            anchor: gpui::ScrollAnchor::for_handle(self.scroll.clone()),
            pending: Rc::new(Cell::new(true)),
        });
        self.project_disclosure_focus
            .entry(path.to_owned())
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .focus(window, cx);
    }

    fn discover_worktrees(&mut self, path: String, cx: &mut Context<Self>) {
        let lookup = path.clone();
        let work = cx.background_spawn(async move {
            use grove_core::git;
            if !Path::new(&lookup).is_dir() {
                return Err("The project folder is unavailable.".to_string());
            }
            match git::list_worktrees_checked(&lookup) {
                Err(git::GitError::Command { stderr, .. })
                    if stderr.contains("not a git repository") =>
                {
                    Ok(vec![Worktree {
                        path: lookup,
                        branch: "—".into(),
                        mtime: None,
                        is_main: true,
                    }])
                }
                result => result.map_err(|error| error.to_string()),
            }
        });
        let key = path.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.project_inventory
                    .insert(key, WorktreeInventory::Loaded(result));
                cx.notify();
            });
        });
        self.project_inventory
            .insert(path, WorktreeInventory::Loading { _task: task });
        cx.notify();
    }

    pub(super) fn refresh_expanded_projects(&mut self, cx: &mut Context<Self>) {
        self.project_inventory.clear();
        if !self.open || self.category != Category::Projects {
            return;
        }
        let store = &cx.global::<SettingsState>().store;
        let paths: Vec<_> = store
            .workspace_projects(store.workspaces.active)
            .filter(|(_, project)| self.expanded_projects.contains(&project.path))
            .map(|(_, project)| project.path.clone())
            .collect();
        for path in paths {
            self.discover_worktrees(path, cx);
        }
    }

    pub(super) fn projects_body(
        &self,
        window: &Window,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let rows = self.scoped_project_rows(cx);
        let mut choices = Self::choices();
        for (id, label, archived) in [("active", "Active", false), ("archived", "Archived", true)] {
            choices = choices.child(
                self.choice(
                    format!("projects-{id}"),
                    label,
                    self.projects_archived == archived,
                    true,
                )
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.dismiss_projects_menu(window, cx);
                    this.projects_archived = archived;
                    this.scroll.set_offset(Point::default());
                    cx.notify();
                })),
            );
        }
        let mut body = div().flex().flex_col().gap(rpx(SPACE_XL)).child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(rpx(SPACE_LG))
                .child(choices)
                .child(
                    self.button("settings-add-project", "Add project", false, true)
                        .debug_selector(|| "settings-add-project".into())
                        .track_focus(&self.add_project_focus)
                        .child(icon("plus", ICON_SM, c::FG_DIM()))
                        .child("Add project")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_project_focus.focus(window, cx);
                            this.start_project_creation(None, false, window, cx);
                        })),
                ),
        );
        if rows.is_empty() {
            body = body.child(div().py(rpx(SPACE_3XL)).text_color(c::FG_DIM()).child(
                if self.projects_archived {
                    "No archived projects. Archived folders remain on disk."
                } else {
                    "No projects yet. Add a folder to get started."
                },
            ));
        }

        let home = dirs::home_dir();
        let home = home.as_deref();
        let mut list = div().flex().flex_col().when(!rows.is_empty(), |list| {
            list.border_t_1().border_color(c::BORDER_SOFT())
        });
        for (visual_index, (store_index, project)) in rows.into_iter().enumerate() {
            let expanded = !project.archived && self.expanded_projects.contains(&project.path);
            let trigger_id = format!("settings-project-menu-{visual_index}");
            let target = ProjectMenuTarget::project(&project.path);
            let row_id = format!("settings-project-row-{visual_index}");
            let row = div()
                .id(format!("settings-project-row:{}", project.path))
                .debug_selector(move || row_id.clone())
                .flex()
                .items_center()
                .gap(rpx(SPACE_LG))
                .py(rpx(SPACE_XS))
                .min_w_0()
                .child(self.project_disclosure(&project, visual_index, expanded, home, cx))
                .child(self.project_row_action_trigger(
                    target,
                    format!("Actions for project {}", project.name),
                    trigger_id,
                    cx,
                ));
            list = list.child(
                div()
                    .id(("settings-project-group", visual_index))
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .border_b_1()
                    .border_color(c::BORDER_SOFT())
                    .when_some(
                        self.project_reveal
                            .as_ref()
                            .filter(|reveal| reveal.path == project.path),
                        |row, reveal| {
                            let anchor = reveal.anchor.clone();
                            let pending = reveal.pending.clone();
                            row.anchor_scroll(Some(anchor.clone())).on_prepaint(
                                move |_, window, cx| {
                                    // The target must be measured in the Projects viewport before scrolling.
                                    if pending.replace(false) {
                                        anchor.scroll_to(window, cx);
                                        window.request_animation_frame();
                                    }
                                },
                            )
                        },
                    )
                    .child(row)
                    .when(expanded, |project_row| {
                        project_row.child(self.project_checkouts(
                            &project,
                            store_index,
                            visual_index,
                            compact,
                            window,
                            cx,
                        ))
                    }),
            );
        }
        body.child(list)
            .when_some(self.project_menu.as_ref(), |body, menu| {
                body.child(gpui::deferred(
                    gpui::anchored()
                        .position_mode(gpui::AnchoredPositionMode::Window)
                        .anchor(gpui::Anchor::TopRight)
                        .position(menu.position)
                        .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
                        .child(menu.popup.clone()),
                ))
            })
    }

    fn scoped_project_rows(&self, cx: &App) -> Vec<(usize, Project)> {
        let store = &cx.global::<SettingsState>().store;
        store
            .projects
            .iter()
            .enumerate()
            .filter(|(_, project)| {
                project.archived == self.projects_archived
                    && store.project_workspace_id(&project.path) == store.workspaces.active
            })
            .map(|(index, project)| (index, project.clone()))
            .collect()
    }

    fn project_worktrees(&self, index: usize, cx: &App) -> Vec<Worktree> {
        let runtime = self.runtime.read(cx);
        if let Some(project) = cx.global::<SettingsState>().store.projects.get(index) {
            match self.project_inventory.get(&project.path) {
                Some(WorktreeInventory::Loaded(Ok(worktrees))) => return worktrees.clone(),
                Some(WorktreeInventory::Loaded(Err(_))) => return Vec::new(),
                // Keep already known rows visible while rechecking them.
                Some(WorktreeInventory::Loading { .. }) | None => {}
            }
        }
        let active = runtime.state.read(cx).proj_idx();
        runtime
            .tree
            .read(cx)
            .worktrees_for_project(index, active)
            .to_vec()
    }

    fn project_disclosure(
        &self,
        project: &Project,
        index: usize,
        expanded: bool,
        home: Option<&Path>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let label = div()
            .flex()
            .flex_1()
            .min_w_0()
            .flex_col()
            .child(
                div()
                    .truncate()
                    .text_size(rpx(TEXT_BODY))
                    .line_height(rpx(PROJECT_NAME_LINE_H))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c::FG())
                    .child(project.name.clone()),
            )
            .child(
                div()
                    .truncate()
                    .text_size(rpx(TEXT_SMALL))
                    .line_height(rpx(PROJECT_META_LINE_H))
                    .child(list_path(&project.path, home)),
            );
        let leading_glyph = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(rpx(PROJECT_NAME_LINE_H))
            .child(icon(
                if project.archived {
                    "folder"
                } else if expanded {
                    "chev-down"
                } else {
                    "chev-right"
                },
                ICON_SM,
                c::FG_DIM(),
            ));
        let tooltip_path = project.path.clone();
        let path_tooltip = move |window: &mut Window, cx: &mut App| {
            components::tooltip(tooltip_path.clone(), window).build(window, cx)
        };
        if project.archived {
            return div()
                .id(("settings-archived-project", index))
                .flex()
                .flex_1()
                .min_w_0()
                .items_start()
                .gap(rpx(SPACE_XL))
                .px(rpx(SPACE_MD))
                .py(rpx(SPACE_LG))
                .text_color(c::FG_DIM())
                .tooltip(path_tooltip)
                .child(leading_glyph)
                .child(label)
                .into_any_element();
        }

        let path = project.path.clone();
        let selector = format!("settings-project-worktrees-{index}");
        components::focusable_inline_disclosure(
            format!("settings-project-disclosure:{}", project.path),
            format!("{} worktrees, {}", project.name, project.path),
        )
        .debug_selector(move || selector.clone())
        .when_some(
            self.project_disclosure_focus.get(&project.path),
            gpui::InteractiveElement::track_focus,
        )
        .aria_expanded(expanded)
        .flex_1()
        .items_start()
        .justify_start()
        .w_auto()
        .h_auto()
        .min_w_0()
        .gap(rpx(SPACE_XL))
        .px(rpx(SPACE_MD))
        .py(rpx(SPACE_LG))
        .focus_visible(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
        .tooltip(path_tooltip)
        .child(leading_glyph)
        .child(components::project_symbol_for_path(
            &project.path,
            ICON_MD,
            cx,
        ))
        .child(label)
        .on_click(cx.listener(move |this, _, window, cx| {
            this.dismiss_projects_menu(window, cx);
            // Disclosure is immediate so the keyboard focus and the visible rows stay in sync.
            if !this.expanded_projects.remove(&path) {
                this.expanded_projects.insert(path.clone());
                this.discover_worktrees(path.clone(), cx);
            }
            if let Some(focus) = this.project_disclosure_focus.get(&path) {
                focus.focus(window, cx);
            }
            cx.notify();
        }))
        .into_any_element()
    }

    fn project_checkouts(
        &self,
        project: &Project,
        store_index: usize,
        visual_index: usize,
        compact: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let home = dirs::home_dir();
        let metadata_size = rpx(TEXT_SMALL).to_pixels(window.rem_size());
        let selector = format!("settings-project-worktree-list-{visual_index}");
        let mut list = div()
            .id(format!("settings-worktree-list:{}", project.path))
            .debug_selector(move || selector.clone())
            .flex()
            .flex_col()
            .min_w_0()
            .ml(rpx(WORKTREE_INDENT))
            .pl(rpx(SPACE_2XL))
            .border_l_1()
            .border_color(c::BORDER_SOFT());
        let worktrees = self.project_worktrees(store_index, cx);
        if worktrees.is_empty() {
            use crate::views::status::{self, StatusKind};
            let path = project.path.clone();
            let (kind, detail) = match self.project_inventory.get(&path) {
                Some(WorktreeInventory::Loading { .. }) => (StatusKind::Working, None),
                Some(WorktreeInventory::Loaded(Err(error))) => {
                    (StatusKind::Failed, Some(error.clone()))
                }
                _ => (StatusKind::Failed, None),
            };
            let working = kind == StatusKind::Working;
            list = list.child(
                div()
                    .py(rpx(SPACE_LG))
                    .text_size(rpx(TEXT_SMALL))
                    .flex()
                    .flex_col()
                    .gap(rpx(SPACE_SM))
                    .child(status::label(
                        format!("settings-worktrees-status-{visual_index}"),
                        kind,
                        if working {
                            "Loading worktrees…"
                        } else {
                            "Worktrees are unavailable. Check the project folder."
                        },
                        ICON_SM,
                        cx,
                    ))
                    .when_some(detail, |row, error| {
                        row.child(div().text_color(c::FG_DIM()).child(error))
                    })
                    .when(!working, |row| {
                        row.child(
                            self.button(
                                ("settings-worktrees-retry", visual_index),
                                "Retry worktree discovery",
                                false,
                                true,
                            )
                            .self_start()
                            .child(icon("refresh", ICON_SM, c::FG_DIM()))
                            .child("Retry")
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.discover_worktrees(path.clone(), cx),
                            )),
                        )
                    }),
            );
        }
        for (row, worktree) in worktrees.into_iter().enumerate() {
            let protected = worktree.is_main || worktree.path == project.path;
            let name = if protected {
                "Main checkout".to_string()
            } else {
                worktree_name(&worktree.path)
            };
            let action = if protected {
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .text_size(rpx(TEXT_SMALL))
                    .line_height(rpx(PROJECT_META_LINE_H))
                    .text_color(c::FG_DIM())
                    .child(icon("lock", ICON_SM, c::FG_DIM()))
                    .child("Protected")
                    .into_any_element()
            } else {
                self.project_row_action_trigger(
                    ProjectMenuTarget::worktree(&project.path, &worktree.path),
                    format!("Actions for worktree {name}"),
                    format!("settings-worktree-menu-{visual_index}-{row}"),
                    cx,
                )
                .into_any_element()
            };
            let branch_id = format!("settings-worktree-branch-{visual_index}-{row}");
            let path_id = format!("settings-worktree-path-{visual_index}-{row}");
            let branch_tooltip = worktree.branch.clone();
            let path_tooltip = worktree.path.clone();
            let mut branch_run = window.text_style().to_run(worktree.branch.len());
            branch_run.font.family = crate::fonts::UI_FAMILY.into();
            let branch_width = components::truncation_safe_label_width(
                &worktree.branch,
                metadata_size,
                &branch_run,
                window.text_system(),
            );
            let branch = div()
                .id(format!(
                    "settings-worktree-branch-tip:{}\0{}",
                    project.path, worktree.path
                ))
                .debug_selector(move || branch_id.clone())
                .flex()
                .flex_shrink_0()
                .min_w_0()
                .items_center()
                .gap(rpx(SPACE_SM))
                .max_w(gpui::relative(if compact { 1.0 } else { 0.5 }))
                .tooltip(move |window, cx| {
                    components::tooltip(branch_tooltip.clone(), window).build(window, cx)
                })
                .child(
                    div()
                        .flex_shrink_0()
                        .child(icon("git-branch", ICON_SM, c::FG_DIM())),
                )
                .child(
                    div()
                        .min_w_0()
                        .w(branch_width)
                        .truncate()
                        .font_family(crate::fonts::UI_FAMILY)
                        .child(worktree.branch),
                );
            let path = div()
                .id(format!(
                    "settings-worktree-path-tip:{}\0{}",
                    project.path, worktree.path
                ))
                .debug_selector(move || path_id.clone())
                .min_w_0()
                .when(compact, gpui::Styled::w_full)
                .when(!compact, gpui::Styled::flex_1)
                .tooltip(move |window, cx| {
                    components::tooltip(path_tooltip.clone(), window).build(window, cx)
                })
                .child(
                    components::readonly_value(
                        gpui::SharedString::from(format!(
                            "settings-worktree-path-{}\0{}",
                            project.path, worktree.path
                        )),
                        list_path(&worktree.path, home.as_deref()),
                    )
                    .text_size(rpx(TEXT_SMALL))
                    .line_height(rpx(PROJECT_META_LINE_H))
                    .text_color(c::FG_DIM())
                    .truncate(),
                );
            let metadata = div()
                .flex()
                .items_center()
                .min_w_0()
                .text_color(c::FG_DIM())
                .text_size(rpx(TEXT_SMALL))
                .line_height(rpx(PROJECT_META_LINE_H))
                .gap(rpx(SPACE_2XL))
                .when(compact, |metadata| {
                    metadata.flex_col().items_start().gap_0()
                })
                .child(branch)
                .child(path);
            list = list.child(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(rpx(SPACE_LG))
                    .py(rpx(SPACE_LG))
                    .border_t_1()
                    .border_color(c::BORDER_SOFT())
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .flex_col()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(rpx(TEXT_BODY))
                                    .line_height(rpx(PROJECT_NAME_LINE_H))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(name),
                            )
                            .child(metadata),
                    )
                    .child(action),
            );
        }
        list
    }

    fn project_row_action_trigger(
        &self,
        target: ProjectMenuTarget,
        label: String,
        selector: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
        let measured = bounds.clone();
        let focus = self.project_menu_focus.get(&target);
        let menu_anchor = self
            .project_menu
            .as_ref()
            .filter(|menu| menu.target == target)
            .map(|menu| (menu.popup.clone(), menu.anchor_bounds));
        let open = menu_anchor.is_some();
        let owner = cx.entity().downgrade();
        let click_target = target.clone();
        let key_target = target.clone();
        components::header_control_frame(
            match &target.worktree {
                Some(path) => format!("settings-worktree-actions:{}\0{path}", target.project),
                None => format!("settings-project-actions:{}", target.project),
            },
            label,
            c::BG_HOVER(),
        )
        .debug_selector(move || selector.clone())
        .tab_index(0)
        .when_some(focus, gpui::InteractiveElement::track_focus)
        .aria_expanded(open)
        .when(open, |trigger| trigger.bg(c::BG_HOVER()))
        .focus_visible(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
        .child(icon(
            "more",
            ICON_MD,
            if open { c::FG() } else { c::FG_DIM() },
        ))
        .child(
            gpui::canvas(
                move |bounds, window, cx| {
                    measured.set(bounds);
                    if let Some((popup, Some(anchor))) = &menu_anchor {
                        if *anchor != bounds {
                            let popup = popup.clone();
                            let owner = owner.clone();
                            window.defer(cx, move |window, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    if this
                                        .project_menu
                                        .as_ref()
                                        .is_some_and(|menu| menu.popup == popup)
                                    {
                                        this.dismiss_projects_menu(window, cx);
                                    }
                                });
                            });
                        }
                    }
                },
                |_, (), _, _| {},
            )
            .absolute()
            .inset_0(),
        )
        .on_key_down({
            let bounds = bounds.clone();
            cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "down" | "up") && !event.is_held {
                    this.open_project_row_menu(key_target.clone(), bounds.get(), window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            })
        })
        .on_click(cx.listener(move |this, _, window, cx| {
            if this
                .project_menu
                .as_ref()
                .is_some_and(|menu| menu.target == click_target)
            {
                this.dismiss_projects_menu(window, cx);
            } else {
                this.open_project_row_menu(click_target.clone(), bounds.get(), window, cx);
            }
        }))
    }

    fn row_menu_actions(&self, target: &ProjectMenuTarget, cx: &App) -> Vec<RowAction> {
        let Some((index, project)) = self
            .scoped_project_rows(cx)
            .into_iter()
            .find(|(_, project)| project.path == target.project)
        else {
            return Vec::new();
        };
        if let Some(path) = &target.worktree {
            if project.archived || !self.expanded_projects.contains(&project.path) {
                return Vec::new();
            }
            let listed = self.project_worktrees(index, cx).iter().any(|checkout| {
                &checkout.path == path && !checkout.is_main && checkout.path != project.path
            });
            return if listed {
                vec![RowAction::UseWorktree, RowAction::Delete]
            } else {
                Vec::new()
            };
        }
        if project.archived {
            return vec![RowAction::Restore, RowAction::Remove];
        }
        let mut actions = vec![RowAction::Edit, RowAction::Move, RowAction::NewWorktree];
        if self.main_checkout_path(index, cx).is_some() {
            actions.push(RowAction::UseMain);
        }
        actions.push(RowAction::Remove);
        actions
    }

    fn main_checkout_path(&self, index: usize, cx: &App) -> Option<String> {
        self.project_worktrees(index, cx)
            .into_iter()
            .find(|worktree| worktree.is_main)
            .map(|worktree| worktree.path)
    }

    fn open_project_row_menu(
        &mut self,
        target: ProjectMenuTarget,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let actions = self.row_menu_actions(&target, cx);
        let Some(focus) = self.project_menu_focus.get(&target).cloned() else {
            return;
        };
        if actions.is_empty() {
            return;
        }
        self.dismiss_projects_menu(window, cx);
        focus.focus(window, cx);
        let owner = cx.entity().downgrade();
        let menu_target = target.clone();
        let popup = PopupMenu::build(window, cx, move |menu, window, _| {
            let mut menu = components::dropdown_menu(menu, window).action_context(focus);
            for action in actions {
                let owner = owner.clone();
                let target = menu_target.clone();
                menu = menu.item(
                    PopupMenuItem::new(action.label())
                        .icon(
                            gpui_component::Icon::default()
                                .path(format!("icons/{}.svg", action.glyph())),
                        )
                        .on_click(move |_, window, cx| {
                            let target = target.clone();
                            let _ = owner.update(cx, |_, cx| {
                                // Vendor dismissal restores the trigger before entering a nested view.
                                cx.defer_in(window, move |this, window, cx| {
                                    this.apply_project_row_action(target, action, window, cx);
                                });
                            });
                        }),
                );
            }
            menu
        });
        let dismiss = cx.subscribe_in(
            &popup,
            window,
            |this, popup, _: &DismissEvent, window, cx| {
                if this
                    .project_menu
                    .as_ref()
                    .is_some_and(|menu| menu.popup == *popup)
                {
                    this.dismiss_projects_menu(window, cx);
                }
            },
        );
        popup.focus_handle(cx).focus(window, cx);
        self.project_menu = Some(ProjectRowMenu {
            target,
            position: gpui::point(
                bounds.right(),
                bounds.bottom() + rpx(SPACE_XS).to_pixels(window.rem_size()),
            ),
            anchor_bounds: Some(bounds),
            viewport_size: window.viewport_size(),
            rem_size: window.rem_size(),
            scroll_offset: self.scroll.offset(),
            popup,
            _dismiss: dismiss,
        });
        cx.notify();
    }

    pub(super) fn dismiss_projects_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = self.project_menu.take() {
            self.project_target_focus(&menu.target).focus(window, cx);
            cx.notify();
        }
    }

    pub(super) fn handle_projects_menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.project_menu.is_none() || !matches!(event.keystroke.key.as_str(), "escape" | "tab")
        {
            return false;
        }
        self.dismiss_projects_menu(window, cx);
        // Keep Tab inside Settings even when a popup reaches the final row.
        window.prevent_default();
        cx.stop_propagation();
        true
    }

    pub(super) fn sync_project_action_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut targets = HashSet::new();
        for (index, project) in self.scoped_project_rows(cx) {
            targets.insert(ProjectMenuTarget::project(&project.path));
            if !project.archived && self.expanded_projects.contains(&project.path) {
                targets.extend(
                    self.project_worktrees(index, cx)
                        .into_iter()
                        .filter(|worktree| !worktree.is_main && worktree.path != project.path)
                        .map(|worktree| ProjectMenuTarget::worktree(&project.path, &worktree.path)),
                );
            }
        }

        let lost_focus = self.project_menu_focus.iter().find_map(|(target, handle)| {
            (!targets.contains(target) && handle.is_focused(window)).then(|| target.clone())
        });
        self.project_menu_focus
            .retain(|target, _| targets.contains(target));
        for target in targets {
            self.project_menu_focus
                .entry(target)
                .or_insert_with(|| cx.focus_handle().tab_stop(true));
        }
        if self.project_menu.as_ref().is_some_and(|menu| {
            !self.project_menu_focus.contains_key(&menu.target)
                || menu.viewport_size != window.viewport_size()
                || menu.rem_size != window.rem_size()
                || menu.scroll_offset != self.scroll.offset()
        }) {
            self.dismiss_projects_menu(window, cx);
        } else if let Some(target) = lost_focus {
            self.project_target_focus(&target).focus(window, cx);
        }
    }

    fn project_target_focus(&self, target: &ProjectMenuTarget) -> FocusHandle {
        self.project_menu_focus
            .get(target)
            .or_else(|| {
                self.project_menu_focus
                    .get(&ProjectMenuTarget::project(&target.project))
            })
            .cloned()
            .unwrap_or_else(|| self.focus.clone())
    }

    pub(super) fn restore_project_action_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(source) = self.project_action_return.take() {
            self.sync_project_action_focus(window, cx);
            self.scroll.set_offset(source.scroll_offset);
            self.project_target_focus(&source.target).focus(window, cx);
        }
    }

    fn apply_project_row_action(
        &mut self,
        target: ProjectMenuTarget,
        action: RowAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.open || self.page != Page::Settings || self.category != Category::Projects {
            return;
        }
        let Some((index, project)) = self
            .scoped_project_rows(cx)
            .into_iter()
            .find(|(_, project)| project.path == target.project)
        else {
            return;
        };
        let root = project.path;
        self.dismiss_projects_menu(window, cx);
        let page = match action {
            RowAction::Edit => ProjectPage::Edit(root),
            RowAction::Move => ProjectPage::Move(root),
            RowAction::Remove => ProjectPage::Remove(root),
            RowAction::Delete => {
                let Some(path) = target.worktree.as_ref() else {
                    return;
                };
                let Some(worktree) = self
                    .project_worktrees(index, cx)
                    .into_iter()
                    .find(|entry| &entry.path == path && !entry.is_main && entry.path != root)
                else {
                    return;
                };
                ProjectPage::RemoveWorktree {
                    project_path: root,
                    name: worktree_name(path),
                    path: worktree.path,
                    branch: worktree.branch,
                }
            }
            RowAction::Restore => {
                let service = self.runtime.read(cx).projects.clone();
                self.error = service
                    .update(cx, |service, cx| {
                        service.restore_archived_by_path(&root, cx)
                    })
                    .err();
                if self.error.is_none() {
                    self.projects_archived = false;
                    self.scroll.set_offset(gpui::Point::default());
                    self.reveal_project(&root, window, cx);
                    self.project_receipt(format!("Restored {}", project.name), cx);
                }
                cx.notify();
                return;
            }
            RowAction::NewWorktree => {
                self.project_target_focus(&target).focus(window, cx);
                self.project_action_return = Some(ProjectActionReturn {
                    target,
                    scroll_offset: self.scroll.offset(),
                });
                self.start_project_creation(Some(&root), false, window, cx);
                return;
            }
            RowAction::UseMain | RowAction::UseWorktree => {
                let worktrees = self.project_worktrees(index, cx);
                let path = worktrees.into_iter().find(|checkout| match action {
                    RowAction::UseMain => checkout.is_main,
                    _ => target.worktree.as_ref() == Some(&checkout.path) && !checkout.is_main,
                });
                if let Some(checkout) = path {
                    cx.emit(SettingsPanelEvent::UseCheckout {
                        root,
                        path: checkout.path,
                    });
                }
                return;
            }
        };
        self.project_target_focus(&target).focus(window, cx);
        self.project_action_return = Some(ProjectActionReturn {
            target,
            scroll_offset: self.scroll.offset(),
        });
        self.open_project_page(page, window, cx);
    }
}

fn list_path(path: &str, home: Option<&Path>) -> String {
    home.and_then(|home| Path::new(path).strip_prefix(home).ok())
        .map_or_else(
            || path.to_owned(),
            |relative| {
                if relative.as_os_str().is_empty() {
                    "~".to_owned()
                } else {
                    format!("~{MAIN_SEPARATOR}{}", relative.display())
                }
            },
        )
}

fn worktree_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_owned()
}

#[cfg(test)]
fn fixture_git_project(repo: &Path, worktrees: &[(&str, &Path)]) {
    fs_err::create_dir_all(repo).unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(repo)
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "user.name=Grove QA",
                "-c",
                "user.email=qa@example.invalid",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-b", "main"]);
    git(&["commit", "--allow-empty", "-m", "Fixture"]);
    for (branch, path) in worktrees {
        git(&["worktree", "add", "-b", branch, path.to_str().unwrap()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{runtime::Runtime, theme::ThemeState, zoom::ZoomState};
    use grove_core::storage::{ProjectScripts, Store};

    #[gpui::test]
    fn contextual_worktrees_reveal_the_active_project_and_keep_creation_in_settings(
        cx: &mut gpui::TestAppContext,
    ) {
        let fixture = tempfile::tempdir().unwrap();
        let base = fixture.path().canonicalize().unwrap();
        let repo = base.join("Atlas");
        let linked = base.join("review-worktree");
        fixture_git_project(&repo, &[("review", &linked)]);
        let root = repo.to_str().unwrap();

        cx.update(|cx| {
            gpui_component::init(cx);
            let mut store = Store::default();
            for (name, path, archived) in [
                ("Drafts", base.join("Drafts"), false),
                ("Atlas", repo.clone(), false),
                ("Old", base.join("Old"), true),
            ] {
                store.projects.push(Project {
                    name: name.into(),
                    path: path.to_string_lossy().into_owned(),
                    scripts: ProjectScripts::default(),
                    archived,
                    worktree_dir: None,
                });
            }
            cx.set_global(SettingsState::new(store));
            cx.set_global(ZoomState::new(1.0));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(750.)));
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.projects_archived = true;
                panel.open_project_worktrees(Some(root), window, cx);
                assert_eq!(panel.category, Category::Projects);
                assert!(!panel.projects_archived);
                assert!(panel.expanded_projects.contains(root));
                assert!(panel.project_disclosure_focus[root].is_focused(window));
                assert!(matches!(
                    panel.project_inventory[root],
                    WorktreeInventory::Loading { .. }
                ));
            });
        });
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        };
        draw(cx);
        assert!(cx
            .debug_bounds("settings-project-worktree-list-0")
            .is_none());
        assert!(cx
            .debug_bounds("settings-project-worktree-list-1")
            .is_some());
        assert!(cx.debug_bounds("settings-worktree-menu-1-0").is_none());
        assert!(cx.debug_bounds("settings-worktree-menu-1-1").is_some());

        let trigger = ProjectMenuTarget::project(root);
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.apply_project_row_action(trigger.clone(), RowAction::NewWorktree, window, cx);
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("settings-worktree-setup").is_some());
        assert!(cx.debug_bounds("worktree-name-field").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        assert!(cx
            .debug_bounds("settings-project-worktree-list-1")
            .is_some());
        cx.update(|window, cx| {
            assert!(panel.read(cx).project_menu_focus[&trigger].is_focused(window));
        });

        let add = cx.debug_bounds("settings-add-project").unwrap();
        cx.simulate_click(add.center(), gpui::Modifiers::default());
        draw(cx);
        assert!(cx.debug_bounds("project-setup-scroll").is_some());
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            assert_eq!(panel.read(cx).page, Page::Settings);
            assert!(panel.read(cx).add_project_focus.is_focused(window));
            panel.update(cx, |panel, cx| {
                panel.open_project_worktrees(Some("/stale-project"), window, cx);
                assert!(panel
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("no longer available"));
            });
        });
        draw(cx);
        assert!(cx.debug_bounds("settings-project-row-1").is_some());

        let missing = base.join("Drafts");
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open_project_worktrees(missing.to_str(), window, cx);
            });
        });
        draw(cx);
        cx.update(|_, cx| {
            let panel = panel.read(cx);
            assert!(matches!(
                panel.project_inventory[missing.to_str().unwrap()],
                WorktreeInventory::Loaded(Err(_))
            ));
        });

        cx.update(|window, cx| {
            let store = &mut cx.global_mut::<SettingsState>().store;
            store.projects.splice(
                0..0,
                (0..25).map(|index| Project {
                    name: format!("Earlier project {index}"),
                    path: base
                        .join(format!("project-{index}"))
                        .to_string_lossy()
                        .into_owned(),
                    scripts: ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }),
            );
            panel.update(cx, |panel, cx| {
                panel.open_project_worktrees(Some(root), window, cx)
            });
        });
        draw(cx);
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
        draw(cx);
        assert!(panel.read_with(cx, |panel, _| panel.scroll.offset().y < gpui::px(0.)));
        let viewport = cx.debug_bounds("settings-scroll").unwrap();
        let row = cx.debug_bounds("settings-project-row-26").unwrap();
        assert!(row.top() >= viewport.top());
        assert!(row.bottom() <= viewport.bottom());
    }

    #[gpui::test]
    fn project_menu_keeps_worktree_identity_and_restores_focus_on_cancel(
        cx: &mut gpui::TestAppContext,
    ) {
        let workspace = tempfile::tempdir().unwrap();
        let base = workspace.path().canonicalize().unwrap();
        let repo = base.join("One");
        let linked = base.join(format!("linked-{}", "long-directory-".repeat(8)));
        fixture_git_project(&repo, &[("review", &linked)]);
        let repo = repo.to_str().unwrap();
        let linked = linked.to_str().unwrap();
        cx.update(|cx| {
            gpui_component::init(cx);
            let mut store = Store::default();
            for (name, path) in [
                ("One", repo.to_owned()),
                ("Two", base.join("Two").to_str().unwrap().to_owned()),
            ] {
                store.projects.push(Project {
                    name: name.into(),
                    path,
                    scripts: ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                });
            }
            cx.set_global(SettingsState::new(store));
            cx.set_global(ZoomState::new(1.0));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(ThemeState::new(
                false,
                c::DEFAULT_DARK_THEME.into(),
                c::DEFAULT_LIGHT_THEME.into(),
            ));
            c::sync_component_theme(cx);
        });
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let runtime = cx.new(Runtime::new);
            SettingsPanel::new(runtime, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(740.), gpui::px(760.)));
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.open_projects(None, window, cx);
                let runtime = panel.runtime.read(cx);
                let active = runtime.state.read(cx).proj_idx();
                let tree = runtime.tree.clone();
                tree.update(cx, |tree, cx| {
                    tree.register_project_worktrees(
                        0,
                        active,
                        grove_core::git::list_worktrees_checked(repo).unwrap(),
                    );
                    cx.notify();
                });
            });
        });

        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        };
        draw(cx);
        for id in [
            "settings-project-worktrees-0",
            "settings-project-worktrees-1",
        ] {
            let row = cx.debug_bounds(id).unwrap();
            cx.simulate_click(row.center(), gpui::Modifiers::default());
            draw(cx);
        }
        assert!(cx
            .debug_bounds("settings-project-worktree-list-0")
            .is_some());
        assert!(cx
            .debug_bounds("settings-project-worktree-list-1")
            .is_some());
        assert!(cx.debug_bounds("settings-worktree-menu-0-0").is_none());
        let wide_branch = cx.debug_bounds("settings-worktree-branch-0-1").unwrap();
        let wide_path = cx.debug_bounds("settings-worktree-path-0-1").unwrap();
        assert_eq!(wide_path.center().y, wide_branch.center().y);
        assert!(wide_branch.right() < wide_path.left());
        let menu_trigger = cx.debug_bounds("settings-worktree-menu-0-1").unwrap();
        let project_trigger = cx.debug_bounds("settings-project-menu-0").unwrap();
        assert_eq!(menu_trigger.right(), project_trigger.right());
        cx.simulate_click(menu_trigger.center(), gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| assert!(panel.read(cx).project_menu.is_some()));

        cx.simulate_keystrokes("down down enter");
        draw(cx);
        cx.update(|_, cx| {
            let panel = panel.read(cx);
            let source = panel.project_action_return.as_ref().unwrap();
            assert_eq!(panel.page, Page::Project);
            assert_eq!(source.target.project, repo);
            assert_eq!(source.target.worktree.as_deref(), Some(linked));
        });
        cx.simulate_keystrokes("escape");
        draw(cx);
        cx.update(|window, cx| {
            let panel = panel.read(cx);
            assert_eq!(panel.page, Page::Settings);
            let target = ProjectMenuTarget::worktree(repo, linked);
            assert!(panel.project_menu_focus[&target].is_focused(window));
        });
        assert!(cx
            .debug_bounds("settings-project-worktree-list-0")
            .is_some());
        assert!(cx
            .debug_bounds("settings-project-worktree-list-1")
            .is_some());

        let menu_trigger = cx.debug_bounds("settings-worktree-menu-0-1").unwrap();
        cx.simulate_click(menu_trigger.center(), gpui::Modifiers::default());
        draw(cx);
        cx.update(|_, cx| assert!(panel.read(cx).project_menu.is_some()));
        cx.simulate_resize(gpui::size(gpui::px(560.), gpui::px(720.)));
        draw(cx);
        let narrow_branch = cx.debug_bounds("settings-worktree-branch-0-1").unwrap();
        let narrow_path = cx.debug_bounds("settings-worktree-path-0-1").unwrap();
        assert_eq!(narrow_path.left(), narrow_branch.left());
        assert!(narrow_path.top() >= narrow_branch.bottom());
        assert_eq!(narrow_path.size.height, wide_path.size.height);
        cx.update(|window, cx| {
            let panel = panel.read(cx);
            assert!(panel.project_menu.is_none());
            let target = ProjectMenuTarget::worktree(repo, linked);
            assert!(panel.project_menu_focus[&target].is_focused(window));
        });
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "native_capture.rs"]
mod native_capture;
