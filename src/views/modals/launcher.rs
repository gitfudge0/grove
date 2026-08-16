//! View plus keyboard/click glue; the pure half lives in [`crate::launcher`]. Ported from `src/gui/session_launcher/{palette.rs,view/*}` and `keys.rs:26+`.
//! The canonical `wants_arrows` modal: ←/→ never move the caret here.

use crate::views::rpx;
use crate::views::tokens::*;
use gpui::{div, prelude::*, px, AnyElement, App, Context, Window};
use grove_core::agent::Agent;

use crate::launcher::{self, PaletteRow, SwitchRow};
use crate::settings::SettingsState;
use crate::theme as c;

use super::{Modal, ModalClick, ModalDispatch, ModalEvent, ModalLayer};
use crate::modal::{LauncherSlotState, LauncherView};
use crate::views::components::{
    body_text, cue_chip, divider_h, icon_slot, keycap_text, modal_footer_hints, modal_panel, mono,
    palette_row, section_header, ui,
};

/// Lines an inline note up with a row's title, not its icon: SPACE_2XL + the 24px icon_slot + SPACE_LG gap.
const ROW_TEXT_INDENT: f32 = SPACE_2XL + 24.0 + SPACE_LG;

/// Every `(proj, project_name, wt_path, agent)` combo the palette can list.
fn combos(
    cx: &App,
    tree_paths: &[(usize, String, Vec<String>)],
) -> Vec<(usize, String, String, Agent)> {
    let default_agent = cx
        .global::<SettingsState>()
        .store
        .default_agent
        .unwrap_or(Agent::Claude);
    let mut out = Vec::new();
    for (idx, name, worktrees) in tree_paths {
        for wt in worktrees {
            out.push((*idx, name.clone(), wt.clone(), default_agent));
        }
    }
    out
}

impl ModalLayer {
    /// `(proj_index, project_name, worktree paths)` for every active project.
    fn tree_paths(&self, cx: &App) -> Vec<(usize, String, Vec<String>)> {
        let store = &cx.global::<SettingsState>().store;
        let tree = self.tree.read(cx);
        let active = self.state.read(cx).proj_idx();
        store
            .active_projects()
            .map(|(i, p)| {
                let mut wts: Vec<String> = tree
                    .worktrees_for_project(i, active)
                    .iter()
                    .map(|w| w.path.clone())
                    .collect();
                // Un-cached, non-active projects report zero worktrees until warmed; fall back to the project root.
                if wts.is_empty() {
                    wts.push(p.path.clone());
                }
                (i, p.name.clone(), wts)
            })
            .collect()
    }

    /// Recent launches, most recent first, resolved to project indices.
    fn recents(&self, cx: &App) -> Vec<(usize, String, Agent)> {
        let store = &cx.global::<SettingsState>().store;
        store
            .recent_launches
            .iter()
            .filter_map(|r| {
                let idx = store.projects.iter().position(|p| p.name == r.project)?;
                Some((idx, r.wt_path.clone(), r.agent))
            })
            .collect()
    }

    /// The rows the palette is currently showing, rebuilt from scratch every
    /// time — the selection is re-resolved by identity, never by index.
    pub(super) fn palette_rows(&self, cx: &App) -> Vec<PaletteRow> {
        let Some(Modal::SessionLauncher(st)) = self.slot.get() else {
            return Vec::new();
        };
        let tree_paths = self.tree_paths(cx);
        let combos = combos(cx, &tree_paths);
        let recents = self.recents(cx);
        // Worktrees-only has no action block, so its empty-query state routes through typed_rows too.
        let scoped = st.scope == launcher::PaletteScope::WorktreesOnly;
        match st.view {
            LauncherView::Root if st.query.trim().is_empty() && !scoped => {
                let fallback: Vec<(usize, String, Agent)> = tree_paths
                    .iter()
                    .filter_map(|(i, _, wts)| {
                        wts.first().map(|w| {
                            (
                                *i,
                                w.clone(),
                                cx.global::<SettingsState>()
                                    .store
                                    .default_agent
                                    .unwrap_or(Agent::Claude),
                            )
                        })
                    })
                    .collect();
                launcher::root_rows(
                    &recents,
                    &fallback,
                    tree_paths.len(),
                    self.state.read(cx).proj_idx(),
                    self.active_run_script(cx).is_some(),
                    self.active_diff_wt(cx).is_some(),
                )
            }
            _ => launcher::typed_rows(
                &st.query,
                &combos,
                &recents,
                self.active_run_script(cx).is_some(),
                self.active_diff_wt(cx).is_some(),
                st.scope,
            ),
        }
    }

    /// Mirrors `Workspace::active_run_script`'s condition; that copy is private to its own view.
    fn active_run_script(&self, cx: &App) -> Option<(String, String)> {
        let id = self.state.read(cx).active_session()?;
        let meta = self.registry.read(cx).meta(id)?;
        let wt_path = meta.wt_path.clone();
        let store = &cx.global::<SettingsState>().store;
        let script = grove_core::storage::project_for_worktree_path(&store.projects, &wt_path)
            .and_then(|(_, p)| p.scripts.run.clone())
            .filter(|s| !s.trim().is_empty())?;
        Some((wt_path, script))
    }

    /// For "View diff": any session with a worktree can show one, whether or not it currently has changes.
    fn active_diff_wt(&self, cx: &App) -> Option<String> {
        let id = self.state.read(cx).active_session()?;
        let meta = self.registry.read(cx).meta(id)?;
        Some(meta.wt_path.clone())
    }

    fn switch_rows(&self, cx: &App) -> Vec<SwitchRow> {
        let Some(Modal::SessionLauncher(st)) = self.slot.get() else {
            return Vec::new();
        };
        let registry = self.registry.read(cx);
        let state = self.state.read(cx);
        let active = state.active_session();
        let matched: Vec<(usize, u64, bool)> = registry
            .all()
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                let wt = std::path::Path::new(&m.wt_path)
                    .file_name()
                    .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
                launcher::fuzzy_match(&st.query, &m.project, &wt, m.agent.label())
            })
            .map(|(i, m)| (i, state.used(m.id), active == Some(m.id)))
            .collect();
        let sessions = launcher::order_switch_sessions(&matched);
        let labels: Vec<String> = registry
            .home_terminals()
            .iter()
            .map(|m| m.label.clone())
            .collect();
        let terminals = launcher::switch_terminal_rows(&labels, &st.query);
        launcher::merge_switch_rows(&sessions, &terminals)
    }

    fn palette_len(&self, cx: &App) -> usize {
        let Some(Modal::SessionLauncher(st)) = self.slot.get() else {
            return 0;
        };
        match st.view {
            LauncherView::Switch => self.switch_rows(cx).len(),
            LauncherView::Settings => crate::launcher::SettingRow::ALL.len(),
            LauncherView::RowActions => self.row_action_len(cx),
            _ => self.palette_rows(cx).len(),
        }
    }

    /// Backs `row_action_len`, the view, and `activate_row_action` so all three stay in sync.
    fn row_action_scripts(&self, wt_path: &str, cx: &App) -> Vec<(&'static str, String)> {
        let store = &cx.global::<SettingsState>().store;
        let Some((_, p)) = grove_core::storage::project_for_worktree_path(&store.projects, wt_path)
        else {
            return Vec::new();
        };
        [
            ("setup", &p.scripts.setup),
            ("run", &p.scripts.run),
            ("teardown", &p.scripts.teardown),
        ]
        .into_iter()
        .filter_map(|(kind, script)| {
            let s = script.as_deref()?.trim();
            if s.is_empty() {
                None
            } else {
                Some((kind, s.to_string()))
            }
        })
        .collect()
    }

    /// Main checkouts can't be deleted, so action 1 offers "Create worktree…" instead. Falls back to
    /// comparing against the project's registered root for a cold, non-active project's empty `wt_cache`.
    fn anchor_is_main(&self, proj: usize, wt_path: &str, cx: &App) -> bool {
        let found = self
            .tree
            .read(cx)
            .worktrees_for_project(proj, self.state.read(cx).proj_idx())
            .iter()
            .find(|w| w.path == wt_path)
            .map(|w| w.is_main);
        if let Some(is_main) = found {
            return is_main;
        }
        let Some(p) = cx.global::<SettingsState>().store.projects.get(proj) else {
            return false;
        };
        wt_path.trim_end_matches('/') == p.path.trim_end_matches('/')
    }

    fn row_action_len(&self, cx: &App) -> usize {
        let Some(Modal::SessionLauncher(st)) = self.slot.get() else {
            return 0;
        };
        let Some(crate::launcher::RowIdentity::Session { wt_path, .. }) = st.anchor.clone() else {
            return 2;
        };
        let base = if cx.global::<SettingsState>().store.project_themes_enabled {
            3
        } else {
            2
        };
        base + self.row_action_scripts(&wt_path, cx).len()
    }

    /// Only re-issued when the selection moved, since scroll_to_item runs in prepaint and would fight the mouse wheel.
    fn scroll_palette_to(&self, view: LauncherView, sel: usize, child_ix: usize) {
        if self.palette_scrolled_to.get() == Some((view, sel)) {
            return;
        }
        self.palette_scrolled_to.set(Some((view, sel)));
        self.palette_scroll.scroll_to_item(child_ix);
    }

    /// For transitions that replace the row set wholesale — the offset only means anything against that list.
    pub(super) fn reset_palette_scroll(&self) {
        self.palette_scroll.set_offset(gpui::Point::default());
        self.palette_scrolled_to.set(None);
    }

    pub(super) fn palette_key(
        &mut self,
        key: crate::modal::ModalKey,
        mods: crate::modal::ModalMods,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use crate::modal::ModalKey as K;
        self.sync_wizard_buffers(cx);
        let len = self.palette_len(cx);
        if key == K::Tab {
            return self.palette_tab(window, cx);
        }
        let Some(Modal::SessionLauncher(st)) = self.slot.get_mut() else {
            return false;
        };
        match key {
            K::Escape => {
                if st.view == LauncherView::Root {
                    self.cancel(cx);
                } else {
                    st.view = LauncherView::Root;
                    st.sel = 0;
                    self.reset_palette_scroll();
                    cx.notify();
                }
                true
            }
            K::Down => {
                st.sel = launcher::cycle(st.sel, 1, len);
                // These views draw rows from something other than palette_rows, so re-anchoring would clobber it.
                if matches!(
                    st.view,
                    LauncherView::RowActions | LauncherView::Settings | LauncherView::Switch
                ) {
                    cx.notify();
                } else {
                    self.reanchor(cx);
                }
                true
            }
            K::Up => {
                st.sel = launcher::cycle(st.sel, -1, len);
                if matches!(
                    st.view,
                    LauncherView::RowActions | LauncherView::Settings | LauncherView::Switch
                ) {
                    cx.notify();
                } else {
                    self.reanchor(cx);
                }
                true
            }
            // Cycles the strip's agent, only on row 0 — the one row with horizontal options.
            K::Left | K::Right if st.view == LauncherView::RowActions && st.sel == 0 => {
                let delta = if key == K::Left { -1 } else { 1 };
                let agents = super::confirm::available_agents();
                st.agent_sel = launcher::cycle(st.agent_sel, delta, agents.len().max(1));
                cx.notify();
                true
            }
            K::Left | K::Right => {
                // Outside the strip the palette still claims them so the caret can't eat a nav key.
                cx.notify();
                true
            }
            K::Enter => {
                self.activate_palette_row(window, cx);
                true
            }
            _ if mods.platform => {
                cx.notify();
                true
            }
            _ => false,
        }
    }

    /// Resolves the row FRESH from `palette_rows` at `st.sel` rather than trusting `st.anchor`,
    /// which stays `None` until an arrow key is pressed (`reanchor`).
    fn palette_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(Modal::SessionLauncher(st)) = self.slot.get() else {
            return false;
        };
        let view = st.view;
        let sel = st.sel;

        if view == LauncherView::RowActions {
            if let Some(Modal::SessionLauncher(st)) = self.slot.get_mut() {
                st.view = LauncherView::Root;
                st.sel = 0;
            }
            self.reset_palette_scroll();
            cx.notify();
            return true;
        }
        if view == LauncherView::Switch || view == LauncherView::Settings {
            cx.notify();
            return true;
        }

        let rows = self.palette_rows(cx);
        let Some(row) = rows.get(sel).cloned() else {
            cx.notify();
            return true;
        };
        match row {
            PaletteRow::Recent { agent, .. } | PaletteRow::Combo { agent, .. } => {
                let identity = launcher::row_identity(&row);
                let agent_sel = launcher::agent_sel_for(&super::confirm::available_agents(), agent);
                if let Some(Modal::SessionLauncher(st)) = self.slot.get_mut() {
                    st.anchor = Some(identity);
                    st.view = LauncherView::RowActions;
                    st.sel = 0;
                    st.agent_sel = agent_sel;
                }
                self.reset_palette_scroll();
            }
            PaletteRow::SwitchToSession => {
                if let Some(Modal::SessionLauncher(st)) = self.slot.get_mut() {
                    st.view = LauncherView::Switch;
                    st.sel = 0;
                }
                self.reset_palette_scroll();
            }
            PaletteRow::Settings => {
                if let Some(Modal::SessionLauncher(st)) = self.slot.get_mut() {
                    st.view = LauncherView::Settings;
                    st.sel = 0;
                }
                self.reset_palette_scroll();
            }
            PaletteRow::Setting(s) => self.activate_setting(s, window, cx),
            PaletteRow::NewSession
            | PaletteRow::TerminalHome
            | PaletteRow::TerminalWt
            | PaletteRow::AddProject
            | PaletteRow::RunScript
            | PaletteRow::ViewDiff
            | PaletteRow::ReloadThemes => {}
        }
        cx.notify();
        true
    }

    /// Activation resolves by content rather than by a possibly-stale index (`state.rs:28-48`).
    fn reanchor(&mut self, cx: &mut Context<Self>) {
        let rows = self.palette_rows(cx);
        if let Some(Modal::SessionLauncher(st)) = self.slot.get_mut() {
            st.anchor = rows.get(st.sel).map(launcher::row_identity);
        }
        cx.notify();
    }

    pub(super) fn activate_palette_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.palette_rows(cx);
        let Some(Modal::SessionLauncher(st)) = self.slot.get() else {
            return;
        };
        let view = st.view;
        let sel = st.sel;
        if view == LauncherView::RowActions {
            self.activate_row_action(cx);
            return;
        }
        if view == LauncherView::Switch {
            let switch = self.switch_rows(cx);
            let Some(row) = switch.get(st.sel).copied() else {
                return;
            };
            self.activate_switch_row(row, cx);
            return;
        }
        if view == LauncherView::Settings {
            let Some(row) = crate::launcher::SettingRow::ALL.get(sel).copied() else {
                return;
            };
            self.activate_setting(row, window, cx);
            return;
        }
        // A vanished row activates nothing.
        let Some(idx) = launcher::resolve_row_by_identity(&rows, st.anchor.as_ref(), st.sel) else {
            return;
        };
        let Some(row) = rows.get(idx).cloned() else {
            return;
        };
        match row {
            PaletteRow::Recent {
                proj,
                wt_path,
                agent,
            }
            | PaletteRow::Combo {
                proj,
                wt_path,
                agent,
            } => {
                let project = cx
                    .global::<SettingsState>()
                    .store
                    .projects
                    .get(proj)
                    .map(|p| p.name.clone());
                let Some(project) = project else { return };
                self.push_recent_launch(&project, &wt_path, agent, cx);
                self.close(cx);
                cx.emit(ModalEvent::SpawnAgent {
                    project,
                    wt_path,
                    agent,
                });
            }
            PaletteRow::NewSession => {
                if let Some(Modal::SessionLauncher(st)) = self.slot.get_mut() {
                    st.view = LauncherView::BrowseAll;
                    st.sel = 0;
                }
                self.reset_palette_scroll();
                cx.notify();
            }
            PaletteRow::SwitchToSession => {
                if let Some(Modal::SessionLauncher(st)) = self.slot.get_mut() {
                    st.view = LauncherView::Switch;
                    st.sel = 0;
                }
                self.reset_palette_scroll();
                cx.notify();
            }
            PaletteRow::Settings => {
                if let Some(Modal::SessionLauncher(st)) = self.slot.get_mut() {
                    st.view = LauncherView::Settings;
                    st.sel = 0;
                }
                self.reset_palette_scroll();
                cx.notify();
            }
            PaletteRow::AddProject => {
                self.open(
                    Modal::AddProject(Box::new(crate::add_project::opened())),
                    cx,
                );
            }
            PaletteRow::TerminalHome | PaletteRow::TerminalWt => {
                self.close(cx);
                cx.emit(ModalEvent::NewHomeTerminal);
            }
            PaletteRow::RunScript => {
                let Some((wt_path, script)) = self.active_run_script(cx) else {
                    cx.notify();
                    return;
                };
                self.close(cx);
                cx.emit(ModalEvent::RunScript { wt_path, script });
            }
            PaletteRow::ViewDiff => {
                let Some(wt_path) = self.active_diff_wt(cx) else {
                    cx.notify();
                    return;
                };
                self.open(Modal::DiffViewer { wt_path }, cx);
            }
            PaletteRow::Setting(s) => self.activate_setting(s, window, cx),
            PaletteRow::ReloadThemes => {
                let errors = grove_core::theme::load_custom();
                let msg = if errors.is_empty() {
                    "themes reloaded".to_string()
                } else {
                    format!("themes reloaded with {} skipped entries", errors.len())
                };
                self.toast.update(cx, |t, cx| t.set_toast(msg, cx));
                cx.notify();
            }
        }
    }

    fn activate_switch_row(&mut self, row: SwitchRow, cx: &mut Context<Self>) {
        match row {
            SwitchRow::Session(i) => {
                let id = self.registry.read(cx).all().get(i).map(|m| m.id);
                if let Some(id) = id {
                    self.close(cx);
                    cx.emit(ModalEvent::SelectSession(id));
                }
            }
            SwitchRow::Terminal(i) => {
                self.close(cx);
                cx.emit(ModalEvent::SelectTerminal(i));
            }
        }
    }

    /// Row 0 launches; row 1 opens create/delete-worktree; row 2 (if enabled) opens the theme picker;
    /// everything after that runs a configured lifecycle script.
    fn activate_row_action(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::SessionLauncher(st)) = self.slot.get() else {
            return;
        };
        let Some(crate::launcher::RowIdentity::Session { proj, wt_path, .. }) = st.anchor.clone()
        else {
            return;
        };
        let sel = st.sel;
        let agent_sel = st.agent_sel;
        let project_themes_enabled = cx.global::<SettingsState>().store.project_themes_enabled;
        let base = if project_themes_enabled { 3 } else { 2 };
        if sel == 0 {
            let agents = super::confirm::available_agents();
            let Some(agent) = agents.get(agent_sel).copied() else {
                return;
            };
            let project = cx
                .global::<SettingsState>()
                .store
                .projects
                .get(proj)
                .map(|p| p.name.clone());
            let Some(project) = project else { return };
            self.push_recent_launch(&project, &wt_path, agent, cx);
            self.close(cx);
            cx.emit(ModalEvent::SpawnAgent {
                project,
                wt_path,
                agent,
            });
        } else if sel == 1 {
            let is_main = self.anchor_is_main(proj, &wt_path, cx);
            // Both the removal confirm and the new-worktree prompt read the active project index.
            self.state.update(cx, |s, cx| {
                s.select_project(proj);
                cx.notify();
            });
            if is_main {
                self.open(
                    Modal::Input {
                        title: "New worktree".into(),
                        buffer: String::new(),
                        note: None,
                        // Seeded empty; `ModalLayer::open` kicks the background branch listing.
                        base: crate::modal::BaseBranchState::default(),
                    },
                    cx,
                );
            } else {
                self.open(
                    Modal::Confirm {
                        title: "Delete worktree?".into(),
                        prompt: format!("'{wt_path}' will be removed from disk."),
                        destructive: true,
                        kind: crate::modal::ConfirmKind::RemoveWorktree(wt_path),
                    },
                    cx,
                );
            }
        } else if sel == 2 && project_themes_enabled {
            let project_name = cx
                .global::<SettingsState>()
                .store
                .projects
                .get(proj)
                .map(|p| p.name.clone());
            let Some(project_name) = project_name else {
                return;
            };
            self.open_theme_picker(
                crate::modal::ThemePickerScope::Project(project_name),
                crate::modal::ThemePickerReturn::Close,
                cx,
            );
        } else if sel >= base {
            let scripts = self.row_action_scripts(&wt_path, cx);
            let Some((_, script)) = scripts.get(sel - base) else {
                return;
            };
            let script = script.clone();
            self.close(cx);
            cx.emit(ModalEvent::RunScript { wt_path, script });
        }
    }

    /// Deduped by `(project, wt_path, agent)` and moved to the front.
    fn push_recent_launch(
        &mut self,
        project: &str,
        wt_path: &str,
        agent: Agent,
        cx: &mut Context<Self>,
    ) {
        let (project, wt_path) = (project.to_string(), wt_path.to_string());
        SettingsState::update(cx, move |store| {
            store
                .recent_launches
                .retain(|r| !(r.project == project && r.wt_path == wt_path && r.agent == agent));
            store.recent_launches.insert(
                0,
                grove_core::storage::RecentLaunch {
                    project,
                    wt_path,
                    agent,
                },
            );
            store.recent_launches.truncate(12);
        });
    }
}

/// The search icon in every state except the two drill-ins, which show a static cue chip instead.
fn leading_glyph(view: LauncherView) -> AnyElement {
    match view {
        LauncherView::Switch => cue_chip("SWITCH TO SESSION").into_any_element(),
        LauncherView::Settings => cue_chip("SETTINGS").into_any_element(),
        _ => crate::icons::icon("search", ICON_LG, c::FG_MUTE()).into_any_element(),
    }
}

pub fn render(layer: &ModalLayer, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    let Some(Modal::SessionLauncher(st)) = layer.slot().get() else {
        return div().into_any_element();
    };

    let search = layer.fields.first().map(|f| {
        div()
            .w_full()
            .px(rpx(SPACE_3XL))
            .py(rpx(SPACE_3XL))
            .flex()
            .items_center()
            .gap(rpx(SPACE_LG))
            .child(leading_glyph(st.view))
            .child(
                div().flex_1().min_w_0().child(
                    gpui_component::input::Input::new(f.state())
                        .appearance(false)
                        .pl(px(0.0))
                        .pr(px(0.0))
                        .py(px(0.0))
                        .w_full(),
                ),
            )
    });

    let (children, selected_child) = match st.view {
        LauncherView::Switch => switch_list(layer, st, dispatch, cx),
        LauncherView::Settings => settings_list(st, dispatch, cx),
        LauncherView::RowActions => row_actions(layer, st, dispatch, cx),
        _ => row_list(layer, st, dispatch, cx),
    };
    // Section headers interleaved among the rows push the selected row off st.sel, so builders report where it landed.
    if let Some(ix) = selected_child {
        layer.scroll_palette_to(st.view, st.sel, ix);
    }
    // Rows are the scroll container's own children, not a nested column, so their indices mean something to scroll_to_item.
    let list_zone = div()
        .id("palette-list")
        .max_h(rpx(MODAL_SCROLL_MAX_H))
        .overflow_y_scroll()
        .track_scroll(&layer.palette_scroll)
        .p(rpx(SPACE_2XL))
        .flex()
        .flex_col()
        .gap(rpx(SPACE_MD))
        .children(children);

    let hints: &[(&'static str, &'static str)] = match st.view {
        LauncherView::RowActions if st.sel == 0 => &[
            ("←→", "agent"),
            ("↑↓", "navigate"),
            ("⏎", "select"),
            ("tab", "back"),
        ],
        LauncherView::RowActions => &[("↑↓", "navigate"), ("⏎", "select"), ("tab", "back")],
        LauncherView::Root => &[
            ("↑↓", "navigate"),
            ("tab", "actions"),
            ("⏎", "open"),
            ("esc", "close"),
        ],
        _ => &[("↑↓", "navigate"), ("⏎", "open"), ("esc", "back")],
    };

    modal_panel(
        MODAL_W_XL,
        div()
            .children(search)
            .child(divider_h())
            .child(list_zone)
            .child(modal_footer_hints(hints)),
    )
    .into_any_element()
}

fn row_label(row: &PaletteRow, cx: &App) -> (String, String, &'static str) {
    let store = &cx.global::<SettingsState>().store;
    match row {
        PaletteRow::Recent {
            proj,
            wt_path,
            agent,
        }
        | PaletteRow::Combo {
            proj,
            wt_path,
            agent,
        } => {
            let project = store
                .projects
                .get(*proj)
                .map_or_else(String::new, |p| p.name.clone());
            let wt = std::path::Path::new(wt_path)
                .file_name()
                .map_or_else(|| wt_path.clone(), |f| f.to_string_lossy().into_owned());
            (
                format!("{project} · {wt}"),
                agent.label().to_string(),
                agent.icon_name(),
            )
        }
        PaletteRow::NewSession => ("New session…".into(), String::new(), "plus"),
        PaletteRow::TerminalHome => ("Home terminal".into(), String::new(), "term"),
        PaletteRow::TerminalWt => ("Worktree terminal".into(), String::new(), "term"),
        PaletteRow::AddProject => ("Add project…".into(), String::new(), "plus"),
        PaletteRow::RunScript => ("Run script".into(), String::new(), "play"),
        PaletteRow::ViewDiff => ("View diff".into(), String::new(), "git"),
        PaletteRow::SwitchToSession => ("Switch to session…".into(), String::new(), "restart"),
        PaletteRow::Settings => ("Settings…".into(), String::new(), "cog"),
        // The value, not the section: the subtitle is the only thing that can report a toggle's flip.
        PaletteRow::Setting(s) => (
            s.label().to_string(),
            super::settings::setting_value(*s, cx),
            s.icon_name(),
        ),
        PaletteRow::ReloadThemes => ("Reload themes".into(), String::new(), "restart"),
    }
}

/// `selected` lights the icon/title; `show_hint` right-aligns a trailing "⏎" keycap.
fn palette_row_content(
    icon: &str,
    title: String,
    subtitle: String,
    selected: bool,
    show_hint: bool,
) -> impl IntoElement {
    let icon_color = if selected { c::YELLOW() } else { c::FG_MUTE() };
    let title_color = if selected { c::FG() } else { c::FG_DIM() };
    let mut title_col = div()
        .flex_1()
        .flex()
        .flex_col()
        .gap(rpx(SPACE_SM))
        .child(ui(title, TEXT_TITLE, title_color));
    if !subtitle.is_empty() {
        title_col = title_col.child(mono(subtitle, TEXT_SMALL, c::FG_MUTE()));
    }
    let mut row = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .w_full()
        .child(icon_slot(icon, ICON_LG, icon_color))
        .child(title_col);
    if selected && show_hint {
        row = row.child(keycap_text("⏎", c::FG_DIM()));
    }
    row
}

/// The scroll container's children, plus the child index the selected row landed at, for scroll_to_item.
type PaletteList = (Vec<AnyElement>, Option<usize>);

fn row_list(
    layer: &ModalLayer,
    st: &LauncherSlotState,
    dispatch: &ModalDispatch,
    cx: &App,
) -> PaletteList {
    let rows = layer.palette_rows(cx);
    if rows.is_empty() {
        return (vec![body_text("no matches").into_any_element()], None);
    }
    let mut list: Vec<AnyElement> = Vec::new();
    let mut selected_child = None;
    let mut last_section: Option<&'static str> = None;
    for (i, row) in rows.iter().enumerate() {
        let section = match row {
            PaletteRow::Recent { .. } => Some("RECENT"),
            PaletteRow::NewSession => Some("ACTIONS"),
            _ => None,
        };
        if let Some(s) = section {
            if last_section != Some(s) {
                list.push(section_header(s, SPACE_2XL, SPACE_LG, SPACE_SM).into_any_element());
                last_section = Some(s);
            }
        }
        let (title, sub, icon) = row_label(row, cx);
        let selected = i == st.sel;
        let show_hint = matches!(
            row,
            PaletteRow::Recent { .. } | PaletteRow::Combo { .. } | PaletteRow::Setting(_)
        );
        if selected {
            selected_child = Some(list.len());
        }
        list.push(
            palette_row(
                gpui::SharedString::from(format!("palette-{i}")),
                selected,
                dispatch,
                ModalClick::SelectRow(i),
                palette_row_content(icon, title, sub, selected, show_hint),
            )
            .into_any_element(),
        );
        // Same safety-warning string the Settings pane promotes.
        if selected
            && matches!(
                row,
                PaletteRow::Setting(crate::launcher::SettingRow::Permissions)
            )
        {
            list.push(
                div()
                    .pt(rpx(SPACE_SM))
                    .pb(rpx(SPACE_XS))
                    .pl(rpx(ROW_TEXT_INDENT))
                    .pr(rpx(SPACE_2XL))
                    .child(ui(
                        "Skip lets agents run any command without asking.",
                        TEXT_SMALL,
                        c::FG_DIM(),
                    ))
                    .into_any_element(),
            );
        }
    }
    (list, selected_child)
}

fn switch_list(
    layer: &ModalLayer,
    st: &LauncherSlotState,
    dispatch: &ModalDispatch,
    cx: &App,
) -> PaletteList {
    let rows = layer.switch_rows(cx);
    if rows.is_empty() {
        return (vec![body_text("no sessions").into_any_element()], None);
    }
    let registry = layer.registry.read(cx);
    let mut list: Vec<AnyElement> = Vec::new();
    let mut selected_child = None;
    let mut printed_sessions = false;
    let mut printed_terminals = false;
    for (i, row) in rows.iter().enumerate() {
        let selected = i == st.sel;
        let (icon, label, sub, waiting) = match row {
            SwitchRow::Session(j) => {
                if !printed_sessions {
                    list.push(
                        section_header("SESSIONS", SPACE_2XL, 0.0, SPACE_MD).into_any_element(),
                    );
                    printed_sessions = true;
                }
                registry.all().get(*j).map_or_else(
                    || ("term", String::new(), String::new(), false),
                    |m| {
                        let waiting = layer.activity.read(cx).state_of(m.id)
                            == crate::activity::ActivityState::WaitingForInput;
                        let label = if waiting {
                            format!("{} (waiting)", m.agent.label())
                        } else {
                            m.agent.label().to_string()
                        };
                        (
                            m.agent.icon_name(),
                            label,
                            format!(
                                "{} / {}",
                                m.project,
                                std::path::Path::new(&m.wt_path)
                                    .file_name()
                                    .map_or_else(String::new, |f| f.to_string_lossy().into_owned())
                            ),
                            waiting,
                        )
                    },
                )
            }
            SwitchRow::Terminal(j) => {
                if !printed_terminals {
                    let top = if i == 0 { 0.0 } else { 12.0 };
                    list.push(
                        section_header("TERMINALS", SPACE_2XL, top, SPACE_MD).into_any_element(),
                    );
                    printed_terminals = true;
                }
                registry.home_terminals().get(*j).map_or_else(
                    || ("term", String::new(), String::new(), false),
                    |m| ("term", m.label.clone(), "home terminal".to_string(), false),
                )
            }
        };
        let content = palette_row_content(icon, label, sub, selected, true);
        let row_el = palette_row(
            gpui::SharedString::from(format!("switch-{i}")),
            selected,
            dispatch,
            ModalClick::SelectRow(i),
            content,
        );
        if selected {
            selected_child = Some(list.len());
        }
        if waiting {
            list.push(
                div()
                    .rounded(rpx(RADIUS_GROUP))
                    .bg(c::AMBER_ROW_TINT())
                    .child(row_el)
                    .into_any_element(),
            );
        } else {
            list.push(row_el.into_any_element());
        }
    }
    (list, selected_child)
}

fn settings_list(st: &LauncherSlotState, dispatch: &ModalDispatch, cx: &App) -> PaletteList {
    let mut list: Vec<AnyElement> = Vec::new();
    let mut selected_child = None;
    let mut last_section: Option<&'static str> = None;
    for (i, s) in crate::launcher::SettingRow::ALL.into_iter().enumerate() {
        if last_section != Some(s.section()) {
            list.push(
                section_header(s.section(), SPACE_2XL, SPACE_LG, SPACE_SM).into_any_element(),
            );
            last_section = Some(s.section());
        }
        let selected = i == st.sel;
        if selected {
            selected_child = Some(list.len());
        }
        list.push(
            palette_row(
                gpui::SharedString::from(format!("lset-{i}")),
                selected,
                dispatch,
                ModalClick::SelectRow(i),
                palette_row_content(
                    s.icon_name(),
                    s.label().to_string(),
                    super::settings::setting_value(s, cx),
                    selected,
                    true,
                ),
            )
            .into_any_element(),
        );
        if selected && matches!(s, crate::launcher::SettingRow::Permissions) {
            list.push(
                div()
                    .pt(rpx(SPACE_SM))
                    .pb(rpx(SPACE_XS))
                    .pl(rpx(ROW_TEXT_INDENT))
                    .pr(rpx(SPACE_2XL))
                    .child(ui(
                        "Skip lets agents run any command without asking.",
                        TEXT_SMALL,
                        c::FG_DIM(),
                    ))
                    .into_any_element(),
            );
        }
    }
    (list, selected_child)
}

/// A 26px rounded square, ringed yellow when it's the selected agent.
const AGENT_BTN: f32 = 26.0;

fn strip_row_content(icon: &str, label: &'static str, color: gpui::Hsla) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .child(icon_slot(icon, ICON_SM, color))
        .child(ui(label, TEXT_BODY, color))
}

/// The agent bar's icons are deliberately non-clickable: their index space collides with the
/// strip's own row selection, so a click there would misfire as an agent pick. ←/→ only.
fn row_actions(
    layer: &ModalLayer,
    st: &LauncherSlotState,
    dispatch: &ModalDispatch,
    cx: &App,
) -> PaletteList {
    let Some(crate::launcher::RowIdentity::Session { proj, wt_path, .. }) = st.anchor.clone()
    else {
        return (Vec::new(), None);
    };

    let agents = super::confirm::available_agents();
    let sel_label = agents.get(st.agent_sel).map_or("", |a| a.label());
    let mut bar = div().flex().items_center().gap(rpx(SPACE_MD));
    for (i, a) in agents.iter().enumerate() {
        let selected = i == st.agent_sel;
        bar = bar.child(
            div()
                .size(rpx(AGENT_BTN))
                .rounded(rpx(RADIUS_GROUP))
                .flex()
                .items_center()
                .justify_center()
                .when(selected, |d| d.border_1().border_color(c::YELLOW()))
                .child(crate::icons::icon(
                    a.icon_name(),
                    ICON_MD,
                    if selected { c::YELLOW() } else { c::FG_MUTE() },
                )),
        );
    }

    let launch_content = div()
        .flex()
        .items_center()
        .gap(rpx(SPACE_LG))
        .w_full()
        .child(icon_slot("play", ICON_SM, c::MAGENTA()))
        .child(ui("Launch session…", TEXT_BODY, c::MAGENTA()))
        .child(div().flex_1())
        .child(mono(sel_label.to_string(), TEXT_BODY, c::FG_DIM()))
        .child(bar);

    let mut list: Vec<AnyElement> = vec![palette_row(
        gpui::SharedString::from("strip-0"),
        st.sel == 0,
        dispatch,
        ModalClick::SelectRow(0),
        launch_content,
    )
    .into_any_element()];

    let is_main = layer.anchor_is_main(proj, &wt_path, cx);
    list.push(if is_main {
        palette_row(
            gpui::SharedString::from("strip-1"),
            st.sel == 1,
            dispatch,
            ModalClick::SelectRow(1),
            strip_row_content("plus", "Create worktree…", c::MAGENTA()),
        )
        .into_any_element()
    } else {
        palette_row(
            gpui::SharedString::from("strip-1"),
            st.sel == 1,
            dispatch,
            ModalClick::SelectRow(1),
            strip_row_content("trash", "Delete worktree", c::RED()),
        )
        .into_any_element()
    });

    let project_themes_enabled = cx.global::<SettingsState>().store.project_themes_enabled;
    if project_themes_enabled {
        list.push(
            palette_row(
                gpui::SharedString::from("strip-2"),
                st.sel == 2,
                dispatch,
                ModalClick::SelectRow(2),
                strip_row_content("contrast", "Project theme…", c::CYAN()),
            )
            .into_any_element(),
        );
    }

    let base = if project_themes_enabled { 3 } else { 2 };
    for (i, (kind, _)) in layer
        .row_action_scripts(&wt_path, cx)
        .into_iter()
        .enumerate()
    {
        let (label, color) = match kind {
            "setup" => ("Setup script", c::GREEN()),
            "run" => ("Run script", c::CYAN()),
            "teardown" => ("Teardown script", c::AMBER()),
            _ => continue,
        };
        let idx = base + i;
        list.push(
            palette_row(
                gpui::SharedString::from(format!("strip-{idx}")),
                st.sel == idx,
                dispatch,
                ModalClick::SelectRow(idx),
                strip_row_content("play", label, color),
            )
            .into_any_element(),
        );
    }

    let selected_child = (st.sel < list.len()).then_some(st.sel);
    (list, selected_child)
}

#[cfg(test)]
mod tests {
    use super::*;

    use gpui::{Entity, FocusHandle, Focusable, Render, TestAppContext};
    use grove_core::storage::{Project, Store};

    use crate::entities::activity_store::ActivityStore;
    use crate::entities::animation_clock::AnimationClock;
    use crate::entities::project_tree::ProjectTree;
    use crate::entities::session_registry::SessionRegistry;
    use crate::entities::toast::ToastState;
    use crate::entities::upgrade::Upgrade;
    use crate::entities::workspace_state::WorkspaceState;
    use crate::modal::LauncherView;

    fn boot_globals(cx: &mut App) {
        cx.set_global(SettingsState::new(Store::default()));
        cx.set_global(crate::theme::ThemeState::new(
            false,
            crate::theme::DEFAULT_DARK_THEME.to_string(),
            crate::theme::DEFAULT_LIGHT_THEME.to_string(),
        ));
        cx.set_global(crate::zoom::ZoomState::new(1.0));
        gpui_component::init(cx);
        cx.bind_keys(crate::keymap::bindings());
    }

    struct TestRoot {
        focus: FocusHandle,
        modals: Entity<ModalLayer>,
    }

    impl Focusable for TestRoot {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl Render for TestRoot {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .track_focus(&self.focus)
                .size_full()
                .child(self.modals.clone())
        }
    }

    fn new_modal_layer(cx: &mut Context<TestRoot>, store: Store) -> Entity<ModalLayer> {
        let state = cx.new(|_| WorkspaceState::new(&store, 1280.0));
        cx.set_global(SettingsState::new(store));
        let registry = cx.new(|_| SessionRegistry::new());
        let tree = cx.new(|_| ProjectTree::new());
        let toast = cx.new(|_| ToastState::new());
        let activity = cx.new(|_| ActivityStore::new());
        let clock = cx.new(AnimationClock::new);
        let upgrade = cx.new(Upgrade::new);
        cx.new(|cx| ModalLayer::new(state, registry, tree, toast, activity, clock, upgrade, cx))
    }

    fn build_root(cx: &mut Context<TestRoot>, store: Store) -> TestRoot {
        let modals = new_modal_layer(cx, store);
        TestRoot {
            focus: cx.focus_handle(),
            modals,
        }
    }

    fn store_with_one_project() -> Store {
        Store {
            projects: vec![Project {
                name: "alpha".to_string(),
                path: "/tmp/alpha".to_string(),
                scripts: grove_core::storage::ProjectScripts::default(),
                theme: None,
                archived: false,
                worktree_dir: None,
            }],
            ..Store::default()
        }
    }

    /// Regression: open the palette, never touch an arrow key, press Tab.
    #[gpui::test]
    fn tab_on_a_session_row_populates_anchor_and_enters_row_actions(cx: &mut TestAppContext) {
        cx.update(boot_globals);
        let (root, vcx) = cx.add_window_view(|_, cx| build_root(cx, store_with_one_project()));
        vcx.run_until_parked();
        let modals = root.read_with(vcx, |r, _| r.modals.clone());

        modals.update(vcx, |l, cx| {
            l.open(Modal::SessionLauncher(Box::default()), cx);
        });
        vcx.run_until_parked();

        modals.update(vcx, |l, cx| {
            let rows = l.palette_rows(cx);
            assert!(
                matches!(rows.first(), Some(PaletteRow::Combo { .. })),
                "expected row 0 to be a session row, got {:?}",
                rows.first()
            );
        });

        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();

        modals.read_with(vcx, |l, _| {
            let Some(Modal::SessionLauncher(st)) = l.slot.get() else {
                panic!("launcher modal closed unexpectedly");
            };
            assert_eq!(st.view, LauncherView::RowActions);
            assert!(
                matches!(
                    st.anchor,
                    Some(crate::launcher::RowIdentity::Session { .. })
                ),
                "anchor was not populated by Tab: {:?}",
                st.anchor
            );
        });
    }

    #[gpui::test]
    fn tab_on_a_non_session_row_leaves_view_at_root(cx: &mut TestAppContext) {
        cx.update(boot_globals);
        let (root, vcx) = cx.add_window_view(|_, cx| build_root(cx, Store::default()));
        vcx.run_until_parked();
        let modals = root.read_with(vcx, |r, _| r.modals.clone());

        modals.update(vcx, |l, cx| {
            l.open(Modal::SessionLauncher(Box::default()), cx);
        });
        vcx.run_until_parked();

        modals.update(vcx, |l, cx| {
            let rows = l.palette_rows(cx);
            assert!(
                matches!(rows.first(), Some(PaletteRow::NewSession)),
                "expected row 0 to be NewSession with zero projects, got {:?}",
                rows.first()
            );
        });

        vcx.simulate_keystrokes("tab");
        vcx.run_until_parked();

        modals.read_with(vcx, |l, _| {
            let Some(Modal::SessionLauncher(st)) = l.slot.get() else {
                panic!("launcher modal closed unexpectedly");
            };
            assert_eq!(st.view, LauncherView::Root);
            assert_eq!(st.anchor, None);
        });
    }

    /// Project 1 is guaranteed non-active since proj_idx() defaults to 0, so its tree cache is cold.
    #[gpui::test]
    fn anchor_is_main_falls_back_to_project_root_for_a_cold_non_active_project(
        cx: &mut TestAppContext,
    ) {
        cx.update(boot_globals);
        let store = Store {
            projects: vec![
                Project {
                    name: "alpha".to_string(),
                    path: "/tmp/alpha".to_string(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    theme: None,
                    archived: false,
                    worktree_dir: None,
                },
                Project {
                    name: "beta".to_string(),
                    path: "/tmp/beta".to_string(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    theme: None,
                    archived: false,
                    worktree_dir: None,
                },
            ],
            ..Store::default()
        };
        let (root, vcx) = cx.add_window_view(|_, cx| build_root(cx, store));
        vcx.run_until_parked();
        let modals = root.read_with(vcx, |r, _| r.modals.clone());

        modals.read_with(vcx, |l, cx| {
            assert_eq!(
                l.state.read(cx).proj_idx(),
                0,
                "project 1 must be non-active"
            );
            assert!(
                l.anchor_is_main(1, "/tmp/beta", cx),
                "cold, non-active project should still report its own root as main"
            );
        });
    }
}
