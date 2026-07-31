//! The recents-first command palette: the three list states, the drill-ins,
//! and the live project-theme preview.
//!
//! The pure half — row building, fuzzy ranking, identity resolution, the
//! scroll window, recents ordering — lives in [`crate::launcher`] and is
//! tested there. This module is the view plus the keyboard/click glue.
//!
//! Ported from `src/gui/session_launcher/{palette.rs,view/*}` and
//! `keys.rs:26+` (`handle_session_launcher_key`).
//!
//! **The palette is the canonical `wants_arrows` modal** (carried decision 2):
//! ←/→ mean agent cycling / zoom / the update strip, never caret movement, and
//! a global-mods chord is an action, never text.

use gpui::{div, prelude::*, px, AnyElement, App, Context, Window};
use grove_core::agent::Agent;

use crate::launcher::{self, PaletteRow, SwitchRow};
use crate::settings::SettingsState;
use crate::theme as c;

use super::shell::{
    body_text, click_row, modal_body, modal_footer_hints, modal_panel, section_header,
};
use super::{Modal, ModalClick, ModalDispatch, ModalEvent, ModalLayer};
use crate::modal::{LauncherSlotState, LauncherView};

/// Rows visible in the palette at once.
pub const VISIBLE_ROWS: usize = 9;

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
                let wts = tree
                    .worktrees_for_project(i, active)
                    .iter()
                    .map(|w| w.path.clone())
                    .collect();
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
        match st.view {
            LauncherView::Root if st.query.trim().is_empty() => {
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
                )
            }
            _ => launcher::typed_rows(&st.query, &combos, &recents),
        }
    }

    /// The switch drill-in's display order: sessions, then home terminals.
    fn switch_rows(&self, cx: &App) -> Vec<SwitchRow> {
        let Some(Modal::SessionLauncher(st)) = self.slot.get() else {
            return Vec::new();
        };
        let registry = self.registry.read(cx);
        let sessions: Vec<usize> = registry
            .all()
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                let wt = std::path::Path::new(&m.wt_path)
                    .file_name()
                    .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
                launcher::fuzzy_match(&st.query, &m.project, &wt, m.agent.label())
            })
            .map(|(i, _)| i)
            .collect();
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
            LauncherView::RowActions => 2,
            _ => self.palette_rows(cx).len(),
        }
    }

    /// The palette owns its whole keyboard (`handle_session_launcher_key`), so
    /// [`crate::modal::key_verdict`] falls through to here.
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
        let Some(Modal::SessionLauncher(st)) = self.slot.get_mut() else {
            return false;
        };
        match key {
            K::Escape => {
                // Esc pops one drill-in level before closing the palette.
                if st.view == LauncherView::Root {
                    self.cancel(cx);
                } else {
                    st.view = LauncherView::Root;
                    st.sel = 0;
                    cx.notify();
                }
                true
            }
            K::Down => {
                st.sel = launcher::cycle(st.sel, 1, len);
                st.offset = launcher::scroll_offset_for(st.offset, st.sel, VISIBLE_ROWS, len);
                self.reanchor(cx);
                true
            }
            K::Up => {
                st.sel = launcher::cycle(st.sel, -1, len);
                st.offset = launcher::scroll_offset_for(st.offset, st.sel, VISIBLE_ROWS, len);
                self.reanchor(cx);
                true
            }
            // Tab reveals the row-action strip under the highlighted row.
            K::Tab => {
                st.view = if st.view == LauncherView::RowActions {
                    LauncherView::Root
                } else {
                    LauncherView::RowActions
                };
                st.sel = 0;
                cx.notify();
                true
            }
            // ←/→ NEVER move the caret here — they cycle the strip's agent.
            K::Left | K::Right if st.view == LauncherView::RowActions => {
                let delta = if key == K::Left { -1 } else { 1 };
                let agents = super::confirm::available_agents();
                st.sel = launcher::cycle(st.sel, delta, agents.len().max(1));
                cx.notify();
                true
            }
            K::Left | K::Right => {
                // Outside the strip the palette still claims them, so the
                // caret cannot silently eat a navigation key.
                cx.notify();
                true
            }
            K::Enter => {
                self.activate_palette_row(window, cx);
                true
            }
            _ if mods.platform => {
                // A global-mods chord is a command, never text.
                cx.notify();
                true
            }
            _ => false,
        }
    }

    /// Re-capture the selected row's identity whenever the cursor moves, so
    /// activation resolves by content rather than by a possibly-stale index
    /// (`state.rs:28-48` — the load-bearing invariant).
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
        if view == LauncherView::Switch {
            let switch = self.switch_rows(cx);
            let Some(row) = switch.get(st.sel).copied() else {
                return;
            };
            self.activate_switch_row(row, cx);
            return;
        }
        // Identity first; a vanished row activates nothing at all.
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
                    st.offset = 0;
                }
                cx.notify();
            }
            PaletteRow::SwitchToSession => {
                if let Some(Modal::SessionLauncher(st)) = self.slot.get_mut() {
                    st.view = LauncherView::Switch;
                    st.sel = 0;
                    st.offset = 0;
                }
                cx.notify();
            }
            PaletteRow::Settings => {
                if let Some(Modal::SessionLauncher(st)) = self.slot.get_mut() {
                    st.view = LauncherView::Settings;
                    st.sel = 0;
                    st.offset = 0;
                }
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

    /// Recents are deduped by `(project, wt_path, agent)` and moved to the
    /// front (`push_recent_launch`).
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

// ── the view ─────────────────────────────────────────────────────────────

pub fn render(layer: &ModalLayer, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    let Some(Modal::SessionLauncher(st)) = layer.slot().get() else {
        return div().into_any_element();
    };

    let search = layer.fields.first().map(|f| {
        div()
            .w_full()
            .px(px(14.0))
            .py(px(10.0))
            .child(gpui_component::input::Input::new(f.state()).w_full())
    });

    let list: AnyElement = match st.view {
        LauncherView::Switch => switch_list(layer, st, dispatch, cx),
        LauncherView::Settings => settings_list(st, dispatch, cx),
        LauncherView::RowActions => row_actions(st, dispatch),
        _ => row_list(layer, st, dispatch, cx),
    };

    let hints: &[(&'static str, &'static str)] = match st.view {
        LauncherView::RowActions => &[
            ("←→", "agent"),
            ("⏎", "launch"),
            ("tab", "back"),
            ("esc", "back"),
        ],
        LauncherView::Root => &[
            ("↑↓", "navigate"),
            ("tab", "actions"),
            ("⏎", "open"),
            ("esc", "close"),
        ],
        _ => &[("↑↓", "navigate"), ("⏎", "open"), ("esc", "back")],
    };

    modal_panel(
        640.0,
        div()
            .children(search)
            .child(div().h(px(1.0)).w_full().bg(c::BORDER_SOFT()))
            .child(modal_body(list))
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
        PaletteRow::SwitchToSession => ("Switch to session…".into(), String::new(), "restart"),
        PaletteRow::Settings => ("Settings…".into(), String::new(), "cog"),
        PaletteRow::Setting(s) => (
            s.label().to_string(),
            s.section().to_string(),
            s.icon_name(),
        ),
        PaletteRow::ReloadThemes => ("Reload themes".into(), String::new(), "restart"),
    }
}

fn row_list(
    layer: &ModalLayer,
    st: &LauncherSlotState,
    dispatch: &ModalDispatch,
    cx: &App,
) -> AnyElement {
    let rows = layer.palette_rows(cx);
    if rows.is_empty() {
        return body_text("no matches").into_any_element();
    }
    let offset = launcher::scroll_offset_for(st.offset, st.sel, VISIBLE_ROWS, rows.len());
    let mut list = div().flex().flex_col().gap(px(2.0));
    let mut last_section: Option<&'static str> = None;
    for (i, row) in rows.iter().enumerate().skip(offset).take(VISIBLE_ROWS) {
        let section = match row {
            PaletteRow::Recent { .. } => Some("RECENT"),
            PaletteRow::NewSession => Some("ACTIONS"),
            _ => None,
        };
        if let Some(s) = section {
            if last_section != Some(s) {
                list = list.child(section_header(s, 8.0, 4.0));
                last_section = Some(s);
            }
        }
        let (title, sub, icon) = row_label(row, cx);
        let selected = i == st.sel;
        list = list.child(click_row(
            gpui::SharedString::from(format!("palette-{i}")),
            selected,
            dispatch,
            ModalClick::SelectRow(i),
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .w_full()
                .child(crate::icons::icon(
                    icon,
                    13.0,
                    if selected { c::FG() } else { c::FG_MUTE() },
                ))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.0))
                        .text_color(if selected { c::FG() } else { c::FG_DIM() })
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(c::FG_MUTE())
                        .child(sub),
                ),
        ));
    }
    list.into_any_element()
}

fn switch_list(
    layer: &ModalLayer,
    st: &LauncherSlotState,
    dispatch: &ModalDispatch,
    cx: &App,
) -> AnyElement {
    let rows = layer.switch_rows(cx);
    if rows.is_empty() {
        return body_text("no sessions").into_any_element();
    }
    let registry = layer.registry.read(cx);
    let mut list = div().flex().flex_col().gap(px(2.0));
    for (i, row) in rows.iter().enumerate() {
        let (label, sub) = match row {
            SwitchRow::Session(j) => registry.all().get(*j).map_or_else(
                || (String::new(), String::new()),
                |m| (m.project.clone(), m.agent.label().to_string()),
            ),
            SwitchRow::Terminal(j) => registry.home_terminals().get(*j).map_or_else(
                || (String::new(), String::new()),
                |m| (m.label.clone(), "home terminal".to_string()),
            ),
        };
        list = list.child(click_row(
            gpui::SharedString::from(format!("switch-{i}")),
            i == st.sel,
            dispatch,
            ModalClick::SelectRow(i),
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .w_full()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.0))
                        .text_color(c::FG_DIM())
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(c::FG_MUTE())
                        .child(sub),
                ),
        ));
    }
    list.into_any_element()
}

fn settings_list(st: &LauncherSlotState, dispatch: &ModalDispatch, cx: &App) -> AnyElement {
    let mut list = div().flex().flex_col().gap(px(2.0));
    let mut last_section: Option<&'static str> = None;
    for (i, s) in crate::launcher::SettingRow::ALL.into_iter().enumerate() {
        if last_section != Some(s.section()) {
            list = list.child(section_header(s.section(), 8.0, 4.0));
            last_section = Some(s.section());
        }
        list = list.child(click_row(
            gpui::SharedString::from(format!("lset-{i}")),
            i == st.sel,
            dispatch,
            ModalClick::SelectRow(i),
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .w_full()
                .child(crate::icons::icon(s.icon_name(), 13.0, c::FG_MUTE()))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.0))
                        .text_color(c::FG_DIM())
                        .child(s.label()),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(c::FG_MUTE())
                        .child(super::settings::setting_value(s, cx)),
                ),
        ));
    }
    list.into_any_element()
}

/// The Tab-revealed action strip, with its agent icon bar. ←/→ walk the bar,
/// which is exactly the carve-out the caret would otherwise eat.
fn row_actions(st: &LauncherSlotState, dispatch: &ModalDispatch) -> AnyElement {
    let agents = super::confirm::available_agents();
    let mut bar = div().flex().items_center().gap(px(6.0));
    for (i, a) in agents.iter().enumerate() {
        let selected = i == st.sel;
        bar = bar.child(click_row(
            gpui::SharedString::from(format!("strip-agent-{i}")),
            selected,
            dispatch,
            ModalClick::SelectRow(i),
            crate::icons::icon(
                a.icon_name(),
                14.0,
                if selected { c::FG() } else { c::FG_MUTE() },
            ),
        ));
    }
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(body_text("Launch session…"))
        .child(bar)
        .child(
            div()
                .text_size(px(11.0))
                .text_color(c::RED())
                .child("Delete worktree"),
        )
        .into_any_element()
}
