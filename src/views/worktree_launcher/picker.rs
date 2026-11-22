//! Worktree picker rows, filtering and selection.

use super::*;

impl WorktreeLauncher {
    pub(super) fn update_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        let rows = self.rows(cx);
        self.selected = if self.mode == PaletteMode::Single && self.selected_project.is_none() {
            0
        } else {
            launcher::resolve_row_by_identity(&rows, self.anchor.as_ref(), 0).unwrap_or(0)
        };
        self.anchor = rows.get(self.selected).map(launcher::row_identity);
        self.sync_agent_to_row(&rows);
        self.error = None;
        cx.notify();
    }

    pub(super) fn sync_agent_to_row(&mut self, rows: &[PaletteRow]) {
        if self.agent_touched || self.mode != PaletteMode::Root {
            return;
        }
        if let Some(PaletteRow::Recent { agent, .. } | PaletteRow::Combo { agent, .. }) =
            rows.get(self.selected)
        {
            self.agent_selected = launcher::agent_sel_for(&Agent::ALL, *agent);
        }
    }

    /// Cached worktrees for the active workspace, with a project root on a cold cache.
    pub(super) fn combos(&self, cx: &App) -> Vec<(usize, String, String, Agent)> {
        let store = &cx.global::<SettingsState>().store;
        let state = self.runtime.read(cx).state.read(cx);
        let tree = self.runtime.read(cx).tree.read(cx);
        let agent = preferred_agent(store.default_agent);
        store
            .workspace_projects(store.workspaces.active)
            .flat_map(|(idx, project)| {
                let cached = tree.worktrees_for_project(idx, state.proj_idx());
                let paths: Vec<String> = if cached.is_empty() {
                    vec![project.path.clone()]
                } else {
                    cached.iter().map(|wt| wt.path.clone()).collect()
                };
                paths
                    .into_iter()
                    .map(move |path| (idx, project.name.clone(), path, agent))
            })
            .collect()
    }

    pub(super) fn rows(&self, cx: &App) -> Vec<PaletteRow> {
        if self.mode != PaletteMode::Root && self.loading_worktrees {
            return Vec::new();
        }
        let store = &cx.global::<SettingsState>().store;
        let mut combos = self.combos(cx);
        if self.mode == PaletteMode::Single {
            if let Some(project) = self.selected_project {
                combos.retain(|(idx, _, _, _)| *idx == project);
                for (_, name, _, _) in &mut combos {
                    name.clear();
                }
            } else {
                let mut projects = store
                    .workspace_projects(store.workspaces.active)
                    .filter_map(|(proj, project)| {
                        let score = launcher::fuzzy_score(&self.query, &project.name, "", "")
                            .map(|score| score.saturating_add(1000))
                            .or_else(|| {
                                launcher::fuzzy_score(&self.query, "", &project.path, "")
                            })?;
                        Some((
                            score,
                            PaletteRow::Combo {
                                proj,
                                wt_path: project.path.clone(),
                                agent: preferred_agent(store.default_agent),
                            },
                        ))
                    })
                    .collect::<Vec<_>>();
                if !self.query.trim().is_empty() {
                    projects.sort_by_key(|row| std::cmp::Reverse(row.0));
                }
                return projects.into_iter().map(|(_, row)| row).collect();
            }
        }
        if self.mode != PaletteMode::Root {
            return launcher::typed_rows(
                &self.query,
                &combos,
                &[],
                false,
                false,
                PaletteScope::WorktreesOnly,
            );
        }
        let mut recent = Vec::new();
        for item in &store.recent_launches {
            if !item.agent.available() {
                continue;
            }
            if let Some((idx, _)) =
                store
                    .workspace_projects(store.workspaces.active)
                    .find(|(i, p)| {
                        p.name == item.project
                            && combos.iter().any(|(combo_idx, _, path, _)| {
                                combo_idx == i && path == &item.wt_path
                            })
                    })
            {
                if !recent
                    .iter()
                    .any(|(p, w, _)| *p == idx && w == &item.wt_path)
                {
                    recent.push((idx, item.wt_path.clone(), item.agent));
                }
            }
        }
        let mut rows = Vec::new();
        let mut seen = HashSet::new();
        for (proj, path, agent) in &recent {
            let name = store.projects.get(*proj).map_or("", |p| p.name.as_str());
            let wt_name = worktree_name(path);
            if launcher::fuzzy_match(&self.query, name, wt_name, agent.label()) {
                rows.push(PaletteRow::Recent {
                    proj: *proj,
                    wt_path: path.clone(),
                    agent: *agent,
                });
                seen.insert((*proj, path.clone()));
            }
        }
        let sidebar = self.sidebar.read(cx);
        let has_script = sidebar.palette_has_run_script(cx);
        let has_diff = sidebar.palette_has_diff(cx);
        let has_worktree = sidebar.selected_worktree().is_some();
        if self.query.trim().is_empty() {
            if recent.is_empty() {
                rows.extend(launcher::root_rows(
                    &[],
                    &combos
                        .iter()
                        .map(|(p, _, w, a)| (*p, w.clone(), *a))
                        .collect::<Vec<_>>(),
                    store.projects.len(),
                    self.runtime.read(cx).state.read(cx).proj_idx(),
                    has_script,
                    has_diff,
                ));
            } else {
                rows.truncate(launcher::MAX_ROOT_RECENTS);
                rows.extend(
                    launcher::root_rows(&[], &[], 0, 0, has_script, has_diff)
                        .into_iter()
                        .filter(|row| {
                            !matches!(row, PaletteRow::Recent { .. } | PaletteRow::Combo { .. })
                        }),
                );
            }
            rows.dedup_by(|a, b| launcher::row_identity(a) == launcher::row_identity(b));
        } else {
            rows.extend(
                launcher::typed_rows(
                    &self.query,
                    &combos,
                    &recent,
                    has_script,
                    has_diff,
                    PaletteScope::All,
                )
                .into_iter()
                .filter(|row| match row {
                    PaletteRow::Combo { proj, wt_path, .. } => {
                        !seen.contains(&(*proj, wt_path.clone()))
                    }
                    _ => true,
                }),
            );
        }
        rows.retain(|row| match row {
            PaletteRow::TerminalWt => has_worktree,
            _ => true,
        });
        rows
    }

    pub(super) fn selected_row(&self, cx: &App) -> Option<PaletteRow> {
        let rows = self.rows(cx);
        let idx = launcher::resolve_row_by_identity(&rows, self.anchor.as_ref(), self.selected)?;
        rows.get(idx).cloned()
    }

    /// Palette membership uses the tree inventory; Runtime performs fresh validation off-thread
    /// before creating a session. Filesystem resolution happens on activation, never in render.
    pub(super) fn valid_cached_worktree(&self, project: usize, target: &str, cx: &App) -> bool {
        let store = &cx.global::<SettingsState>().store;
        let Some(project_path) = store.projects.get(project).map(|project| &project.path) else {
            return false;
        };
        let Ok(target) = fs_err::canonicalize(target) else {
            return false;
        };
        if fs_err::canonicalize(project_path).is_ok_and(|path| path == target) {
            return true;
        }
        let runtime = self.runtime.read(cx);
        runtime
            .tree
            .read(cx)
            .worktrees_for_project(project, runtime.state.read(cx).proj_idx())
            .iter()
            .any(|worktree| fs_err::canonicalize(&worktree.path).is_ok_and(|path| path == target))
    }
}
