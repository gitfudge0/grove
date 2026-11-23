//! Reviewed project and worktree removal lifecycle.

use super::*;

impl ProjectService {
    pub fn remove_project(
        &mut self,
        idx: usize,
        also_remove_worktrees: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .map(|p| p.path.clone())
        {
            let _ = self.remove_project_by_path(&path, also_remove_worktrees, cx);
        }
    }

    pub fn remove_project_by_path(
        &mut self,
        path: &str,
        also_remove_worktrees: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let worktrees = if also_remove_worktrees {
            ProjectWorktreesToRemove::All
        } else {
            ProjectWorktreesToRemove::None
        };
        self.remove_project_with_worktree_selection(path, worktrees, cx)
    }

    pub(crate) fn review_project_worktrees(path: &str) -> Result<ProjectWorktreeReview, String> {
        if !git::is_repo(path) {
            return Ok(ProjectWorktreeReview::default());
        }
        let worktrees = git::list_worktrees_checked(path).map_err(|error| error.to_string())?;
        let mut review = ProjectWorktreeReview::default();
        for tree in worktrees
            .into_iter()
            .filter(|tree| removable_worktree(path, &tree.path, tree.is_main))
        {
            match ReviewedWorktree::from_checkout(&tree) {
                Ok(worktree) => review.verified.push(worktree),
                Err(_) => review.skipped_paths.push(tree.path),
            }
        }
        Ok(review)
    }

    pub(crate) fn remove_project_with_reviewed_worktrees(
        &mut self,
        path: &str,
        worktrees: Vec<ReviewedWorktree>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.remove_project_with_worktree_selection(
            path,
            ProjectWorktreesToRemove::Reviewed(worktrees),
            cx,
        )
    }

    fn remove_project_with_worktree_selection(
        &mut self,
        path: &str,
        worktrees: ProjectWorktreesToRemove,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.ensure_not_removing(path)?;
        let store = &cx.global::<SettingsState>().store;
        let idx = project_index(&store.projects, path)
            .ok_or_else(|| "Project no longer exists.".to_string())?;
        let project = store.projects[idx].clone();
        self.archive_blockers(path, cx)?;
        let path = project.path.clone();
        self.removals
            .insert(path.clone(), ProjectRemoval::default());
        self.kill_sessions_for_project_path(&project.path, cx)?;
        self.removal_changed(&path, cx);
        if matches!(worktrees, ProjectWorktreesToRemove::None) {
            self.finish_removal(&path, cx);
            return Ok(());
        }
        let registry = self.registry.clone();
        cx.spawn(async move |this, cx| {
            if let Err(error) = wait_for_session_cleanup(&registry, cx).await {
                let _ = this.update(cx, |this, cx| this.complete_project_removal(&path, Err(error), cx));
                return;
            }
            let repository = project.path.clone();
            let targets = match worktrees {
                ProjectWorktreesToRemove::None => unreachable!(),
                ProjectWorktreesToRemove::All => cx
                    .background_executor()
                    .spawn(async move {
                        git::list_worktrees_checked(&repository).map(|worktrees| {
                            worktrees
                                .into_iter()
                                .filter(|worktree| {
                                    removable_worktree(&repository, &worktree.path, worktree.is_main)
                                })
                                .map(|worktree| (worktree.path, None))
                                .collect::<Vec<_>>()
                        })
                    })
                    .await,
                ProjectWorktreesToRemove::Reviewed(worktrees) => Ok(worktrees
                    .into_iter()
                    .map(|worktree| (worktree.path.clone(), Some(worktree)))
                    .collect()),
            };
            let targets = match targets {
                Ok(targets) => targets,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        if let Some(operation) = this.removals.get_mut(&path) {
                            operation
                                .errors
                                .push(format!("Could not list worktrees: {error}"));
                        }
                        this.finish_removal(&path, cx);
                    });
                    return;
                }
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(operation) = this.removals.get_mut(&path) {
                    operation.total = targets.len();
                }
                this.removal_changed(&path, cx);
            });
            for (target, reviewed) in targets {
                let _ = this.update(cx, |this, cx| {
                    if let Some(operation) = this.removals.get_mut(&path) {
                        operation.current_target = Some(target.clone());
                    }
                    this.removal_changed(&path, cx);
                });
                let protected = this.update(cx, |_, cx| {
                    let canonical = Path::new(&target)
                        .canonicalize()
                        .map_err(|error| format!("Could not resolve worktree path: {error}"))?;
                    Ok::<_, String>(registered_project_at_path(
                        &cx.global::<SettingsState>().store.projects,
                        &target,
                        &canonical,
                    )
                    .map(|project| project.name.clone()))
                });
                let result = match protected {
                    Ok(Ok(Some(name))) => Err(format!(
                        "{target}: contains registered project '{name}'; remove that registration before deleting this worktree"
                    )),
                    Ok(Err(error)) => Err(format!("{target}: {error}")),
                    Err(_) => return,
                    Ok(Ok(None)) => {
                        let repository = project.path.clone();
                        let target_path = target.clone();
                        cx.background_executor()
                            .spawn(async move {
                                if let Some(reviewed) = reviewed {
                                    reviewed.verify(&repository)
                                        .map_err(|error| format!("{target_path}: {error}"))?;
                                }
                                git::remove_worktree(&repository, &target_path)
                                    .map_err(|error| format!("{target_path}: {error}"))
                            })
                            .await
                    }
                };
                let _ = this.update(cx, |this, cx| {
                    if let Some(operation) = this.removals.get_mut(&path) {
                        operation.complete_target(result);
                    }
                    this.removal_changed(&path, cx);
                });
            }
            let _ = this.update(cx, |this, cx| this.finish_removal(&path, cx));
        })
        .detach();
        Ok(())
    }

    fn removal_changed(&self, path: &str, cx: &mut Context<Self>) {
        cx.emit(ProjectEvent::ProjectRemovalChanged {
            path: path.to_string(),
        });
        cx.notify();
    }

    pub(super) fn sweep_removal_sessions(
        &mut self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.kill_sessions_for_project_path(path, cx)
    }

    fn finish_removal(&mut self, path: &str, cx: &mut Context<Self>) {
        let path = path.to_owned();
        let sweep = self.sweep_removal_sessions(&path, cx);
        let registry = self.registry.clone();
        cx.spawn(async move |this, cx| {
            let result = match sweep {
                Ok(()) => wait_for_session_cleanup(&registry, cx).await,
                Err(error) => Err(error),
            };
            let _ = this.update(cx, |this, cx| {
                let result = result.and_then(|()| this.unregister_project(&path, cx));
                this.complete_project_removal(&path, result, cx);
            });
        })
        .detach();
    }

    fn complete_project_removal(
        &mut self,
        path: &str,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(operation) = self.removals.get_mut(path) {
            operation.finished = true;
            operation.current_target = None;
            operation.unregistered = result.is_ok();
            if let Err(error) = result {
                operation.errors.push(error);
            }
            cx.emit(ProjectEvent::ProjectRemoved {
                errors: operation.errors.clone(),
            });
        }
        self.removal_changed(path, cx);
    }

    /// Validate against Git's checked inventory before touching any registered session.
    pub fn remove_worktree_by_path(
        &mut self,
        project_path: &str,
        wt_path: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.start_worktree_removal(project_path, wt_path, None, cx)
    }

    pub(crate) fn review_worktree(
        project_path: &str,
        path: &str,
        branch: &str,
    ) -> Result<ReviewedWorktree, String> {
        let worktree = git::list_worktrees_checked(project_path)
            .map_err(|error| format!("Could not list worktrees: {error}"))?
            .into_iter()
            .find(|tree| {
                tree.path == path && removable_worktree(project_path, &tree.path, tree.is_main)
            })
            .ok_or("The selected path is no longer listed as a linked worktree.")?;
        if worktree.branch != branch {
            return Err("The worktree branch changed since the project list loaded.".into());
        }
        ReviewedWorktree::from_checkout(&worktree)
    }

    pub(crate) fn remove_reviewed_worktree(
        &mut self,
        project_path: &str,
        reviewed: ReviewedWorktree,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let path = reviewed.path.clone();
        self.start_worktree_removal(project_path, &path, Some(reviewed), cx)
    }

    fn start_worktree_removal(
        &mut self,
        project_path: &str,
        wt_path: &str,
        reviewed: Option<ReviewedWorktree>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.ensure_not_removing(project_path)?;
        self.ensure_no_inline_scripts(wt_path, cx)?;
        if self.teardown_target.is_some()
            || self
                .worktree_removals
                .values()
                .any(|operation| operation.stage != WorktreeRemovalStage::Finished)
        {
            return Err("Another worktree is already being removed.".into());
        }
        if wt_path.ends_with("/.") {
            return Err("Use the exact worktree path listed by Git.".into());
        }
        let _project = cx
            .global::<SettingsState>()
            .store
            .projects
            .iter()
            .find(|project| project.path == project_path)
            .cloned()
            .ok_or_else(|| "Project no longer exists.".to_string())?;
        if fs::canonicalize(project_path)
            .ok()
            .zip(fs::canonicalize(wt_path).ok())
            .is_some_and(|(project, target)| project == target)
        {
            return Err("The main project checkout cannot be removed.".into());
        }
        let canonical_path = fs::canonicalize(wt_path)
            .map_err(|error| format!("Could not resolve worktree path: {error}"))?;
        if let Some(registered) = registered_project_at_path(
            &cx.global::<SettingsState>().store.projects,
            wt_path,
            &canonical_path,
        ) {
            return Err(format!("This worktree contains registered project '{}'. Remove that project registration before deleting its folder.", registered.name));
        }
        self.worktree_removals.insert(
            wt_path.to_owned(),
            WorktreeRemoval {
                stage: WorktreeRemovalStage::Checking,
                error: None,
                project_path: project_path.to_owned(),
                canonical_path: canonical_path.clone(),
                reviewed: reviewed.clone(),
            },
        );
        self.worktree_removal_changed(wt_path, cx);
        let repository = project_path.to_owned();
        let target = wt_path.to_owned();
        let check = cx.background_spawn(async move {
            let listed = git::list_worktrees_checked(&repository)
                .map_err(|error| format!("Could not list worktrees: {error}"))?;
            let worktree = listed
                .iter()
                .find(|worktree| worktree.path == target)
                .ok_or("Worktree is no longer listed by Git.")?;
            if !removable_worktree(&repository, &worktree.path, worktree.is_main) {
                return Err("The main project checkout cannot be removed.".into());
            }
            if let Some(reviewed) = reviewed {
                reviewed.verify_checkout(worktree)?;
            }
            let current = fs::canonicalize(&target)
                .map_err(|error| format!("Could not resolve worktree path: {error}"))?;
            if current != canonical_path {
                return Err("Worktree identity changed while checking removal.".into());
            }
            Ok(current)
        });
        let target = wt_path.to_owned();
        let repository = project_path.to_owned();
        cx.spawn(async move |this, cx| {
            let checked = check.await;
            let _ = this.update(cx, |this, cx| {
                let result = checked.and_then(|canonical| {
                    let project = cx.global::<SettingsState>().store.projects.iter().find(|project| project.path == repository).cloned().ok_or("Project no longer exists.")?;
                    if let Some(registered) = registered_project_at_path(&cx.global::<SettingsState>().store.projects, &target, &canonical) {
                        return Err(format!("This worktree contains registered project '{}'. Remove that project registration before deleting its folder.", registered.name));
                    }
                    this.begin_worktree_removal(project, &target, canonical, cx)
                });
                if let Err(error) = result {
                    if let Some(operation) = this.worktree_removals.get_mut(&target) {
                        operation.stage = WorktreeRemovalStage::Finished;
                        operation.error = Some(error);
                    }
                    cx.emit(ProjectEvent::TreeInvalidated);
                    this.worktree_removal_changed(&target, cx);
                }
            });
        }).detach();
        Ok(())
    }

    fn begin_worktree_removal(
        &mut self,
        project: storage::Project,
        wt_path: &str,
        canonical_path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(operation) = self
            .worktree_removals
            .get_mut(wt_path)
            .filter(|operation| operation.stage == WorktreeRemovalStage::Checking)
        else {
            return Err("Removal is no longer pending.".into());
        };
        operation.stage = WorktreeRemovalStage::Stopping;
        operation.canonical_path = canonical_path;
        let reviewed = operation.reviewed.clone();
        self.kill_sessions_touching_worktree(wt_path, cx);
        self.worktree_removal_changed(wt_path, cx);
        let target = wt_path.to_owned();
        let registry = self.registry.clone();
        cx.spawn(async move |this, cx| {
            let mut result = wait_for_session_cleanup(&registry, cx).await;
            if let (Ok(()), Some(reviewed)) = (&result, reviewed) {
                let repository = project.path.clone();
                result = cx
                    .background_executor()
                    .spawn(async move { reviewed.verify(&repository) })
                    .await;
            }
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.start_worktree_teardown(project, &target, cx),
                Err(error) => this.fail_worktree_removal(&target, error, cx),
            });
        })
        .detach();
        Ok(())
    }

    fn fail_worktree_removal(&mut self, target: &str, error: String, cx: &mut Context<Self>) {
        if let Some(operation) = self.worktree_removals.get_mut(target) {
            operation.stage = WorktreeRemovalStage::Finished;
            operation.error = Some(error.clone());
        }
        cx.emit(ProjectEvent::WorktreeRemoved { result: Err(error) });
        self.worktree_removal_changed(target, cx);
    }

    fn start_worktree_teardown(
        &mut self,
        project: storage::Project,
        wt_path: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(operation) = self.worktree_removals.get_mut(wt_path) {
            operation.stage = WorktreeRemovalStage::RunningScript;
        }
        self.teardown_target = Some((project.path.clone(), wt_path.to_owned()));
        self.worktree_removal_changed(wt_path, cx);

        let Some(script) = project
            .scripts
            .teardown
            .as_deref()
            .map(str::trim)
            .filter(|script| !script.is_empty())
        else {
            self.skip_worktree_teardown(cx);
            return;
        };
        let session = cx.new(|cx| TerminalSession::spawn_script(script, wt_path, cx));
        if let Some(error) = session.read(cx).spawn_error() {
            if let Some(operation) = self.worktree_removals.get_mut(wt_path) {
                operation.error = Some(format!("Teardown could not start: {error}"));
            }
            self.worktree_removal_changed(wt_path, cx);
            return;
        }
        self.teardown_session = Some(session.clone());
        // TerminalSession exposes liveness but not an exit code; a script that starts and then
        // exits nonzero is treated as complete. Spawn failures stay visible for explicit skip.
        self.teardown_poll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(120))
                .await;
            if let Some(error) =
                session.read_with(cx, |session, _| session.spawn_error().map(str::to_owned))
            {
                let _ = this.update(cx, |this, cx| {
                    if let Some((_, target)) = this.teardown_target.clone() {
                        if let Some(operation) = this.worktree_removals.get_mut(&target) {
                            operation.error = Some(format!("Teardown could not start: {error}"));
                        }
                        this.worktree_removal_changed(&target, cx);
                    }
                });
                return;
            }
            if session.update(cx, |session, _| session.alive()) {
                continue;
            }
            let _ = this.update(cx, ProjectService::skip_worktree_teardown);
            return;
        }));
    }

    fn kill_sessions_touching_worktree(&mut self, path: &str, cx: &mut Context<Self>) {
        let canonical_path = self
            .worktree_removals
            .get(path)
            .map(|operation| operation.canonical_path.clone())
            .or_else(|| Path::new(path).canonicalize().ok());
        self.kill_sessions(
            |meta| {
                path_within_worktree(&meta.wt_path, path, canonical_path.as_deref())
                    || meta.context_roots.iter().any(|root| {
                        path_within_worktree(&root.wt_path, path, canonical_path.as_deref())
                    })
            },
            cx,
        );
    }

    fn worktree_removal_changed(&self, path: &str, cx: &mut Context<Self>) {
        cx.emit(ProjectEvent::WorktreeRemovalChanged {
            path: path.to_string(),
        });
        cx.notify();
    }

    /// Skipping cancels the owned script PTY, then proceeds with Git removal once.
    pub(crate) fn skip_worktree_teardown_for(
        &mut self,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self
            .teardown_target
            .as_ref()
            .is_none_or(|(_, target)| target != path)
        {
            return Err("No active teardown for this exact worktree path".into());
        }
        self.skip_worktree_teardown(cx);
        Ok(())
    }

    pub fn skip_worktree_teardown(&mut self, cx: &mut Context<Self>) {
        let Some((project_path, wt_path)) = self.teardown_target.take() else {
            return;
        };
        self.teardown_poll = None;
        let script = self.teardown_session.take();
        if let Some(session) = &script {
            if let Err(error) = session.update(cx, TerminalSession::request_script_stop) {
                self.fail_worktree_removal(&wt_path, error, cx);
                return;
            }
        }
        let reviewed = if let Some(operation) = self.worktree_removals.get_mut(&wt_path) {
            operation.stage = WorktreeRemovalStage::Stopping;
            // An explicit skip acknowledges a script start failure; only Git removal errors
            // determine whether the finished operation failed.
            operation.error = None;
            operation.reviewed.clone()
        } else {
            None
        };
        self.worktree_removal_changed(&wt_path, cx);
        self.kill_sessions_touching_worktree(&wt_path, cx);
        let registry = self.registry.clone();
        cx.spawn(async move |this, cx| {
            let mut result = wait_for_session_cleanup(&registry, cx).await;
            while result.is_ok()
                && script.as_ref().is_some_and(|session| {
                    !session.read_with(cx, |session, _| session.has_exited())
                })
            {
                if let Some(error) = script.as_ref().and_then(|session| {
                    session.read_with(cx, |session, _| session.spawn_error().map(str::to_owned))
                }) {
                    result = Err(format!("Teardown process could not stop safely: {error}"));
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(20))
                    .await;
                result = wait_for_session_cleanup(&registry, cx).await;
            }
            if let Err(error) = result {
                let _ = this.update(cx, |this, cx| {
                    this.fail_worktree_removal(&wt_path, error, cx);
                });
                return;
            }
            let _ = this.update(cx, |this, cx| {
                if let Some(operation) = this.worktree_removals.get_mut(&wt_path) {
                    operation.stage = WorktreeRemovalStage::Removing;
                }
                this.worktree_removal_changed(&wt_path, cx);
            });
            let project = project_path.clone();
            let target = wt_path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    if let Some(reviewed) = reviewed {
                        reviewed
                            .verify(&project)
                            .map_err(|error| format!("{target}: {error}"))?;
                    }
                    git::remove_worktree(&project, &target)
                        .map_err(|error| format!("{target}: {error}"))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.kill_sessions_touching_worktree(&wt_path, cx);
                if let Some(operation) = this.worktree_removals.get_mut(&wt_path) {
                    operation.stage = WorktreeRemovalStage::Finished;
                    if let Err(error) = &result {
                        operation.error = Some(match operation.error.take() {
                            Some(previous) => format!("{previous}; {error}"),
                            None => error.clone(),
                        });
                    }
                }
                cx.emit(ProjectEvent::TreeInvalidated);
                cx.emit(ProjectEvent::WorktreeRemoved { result });
                this.worktree_removal_changed(&wt_path, cx);
            });
        })
        .detach();
    }
}
