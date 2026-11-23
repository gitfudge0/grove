mod agents;

use super::*;
#[gpui::test]
fn projectless_provider_sessions_keep_directory_targets(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
    });
    let (_, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(crate::runtime::Runtime::new);
        let mut sidebar = Sidebar::new(runtime.clone(), window, cx);
        sidebar.dashboard_open = false;
        for agent in [Agent::Codex, Agent::Claude] {
            let id = runtime.read(cx).registry.clone().update(cx, |registry, _| {
                registry.insert_meta(String::new(), "/tmp/projectless-fixture".into(), agent)
            });
            sidebar.selection = Some(Selection::Session(id));
            assert_eq!(
                sidebar.checkout_targets(cx),
                vec![(usize::MAX, "/tmp/projectless-fixture".into())]
            );
        }
        sidebar
    });
    let _ = cx;
}

#[test]
fn invalidated_cached_status_is_polled_again() {
    let mut panel = CheckoutPanel {
        state: Some(Err("Last observation unavailable".into())),
        ..CheckoutPanel::default()
    };
    assert!(panel.local_refresh_due(true));
    panel.checked_at = Some(Instant::now());
    assert!(!panel.local_refresh_due(true));
    panel.checked_at = None;
    panel.loading = true;
    assert!(!panel.local_refresh_due(true));
    panel.loading = false;
    panel.operation.busy = true;
    assert!(!panel.local_refresh_due(true));
    panel.operation.busy = false;
    assert!(panel.local_refresh_due(true));
}

#[test]
fn background_samples_cannot_overwrite_a_completed_mutation() {
    let mut panel = CheckoutPanel {
        path: Some("/checkout".into()),
        generation: 4,
        ..CheckoutPanel::default()
    };
    assert!(panel.accepts_sample("/checkout", 4, 0));
    panel.operation.revision += 1;
    panel.operation.busy = true;
    assert!(!panel.accepts_sample("/checkout", 4, 0));
    panel.operation.busy = false;
    assert!(!panel.accepts_sample("/checkout", 4, 0));
    assert!(panel.accepts_sample("/checkout", 4, 1));
    panel.generation += 1;
    assert!(!panel.accepts_sample("/checkout", 4, 1));
    assert!(!panel.accepts_sample("/other", 5, 1));
}

#[test]
fn background_commit_provider_follows_session_then_supported_default() {
    use grove_core::codex::Provider;
    assert_eq!(
        commit_provider(Some(Agent::Claude), Some(Agent::Codex)),
        Ok(Provider::Claude)
    );
    assert_eq!(
        commit_provider(Some(Agent::Terminal), Some(Agent::Codex)),
        Ok(Provider::Codex)
    );
    assert!(commit_provider(Some(Agent::OpenCode), Some(Agent::OpenCode)).is_err());
    assert!(commit_provider(None, None).is_err());
}
#[test]
fn multiple_roots_require_a_current_explicit_choice() {
    let targets = vec![(0, "/one".into()), (1, "/two".into())];
    assert_eq!(selected_checkout(&targets, None), None);
    assert_eq!(selected_checkout(&targets, Some("/removed")), None);
    assert_eq!(
        selected_checkout(&targets, Some("/two")),
        Some((1, "/two".into()))
    );
    assert_eq!(
        selected_checkout(&targets[..1], None),
        Some((0, "/one".into()))
    );
}
#[gpui::test]
fn codex_and_claude_resolution_handoff_only_drafts_in_fresh_checkout(
    cx: &mut gpui::TestAppContext,
) {
    for provider in [
        grove_core::codex::Provider::Codex,
        grove_core::codex::Provider::Claude,
    ] {
        let (handle, actions) = grove_core::codex::Handle::channel();
        let (_updates, receive) = std::sync::mpsc::channel();
        let saved = grove_core::codex::SavedSession {
            html_policy: grove_core::visuals::HtmlPolicy::Legacy,
            visuals: Vec::new(),
            session_review: grove_core::codex::SessionReview::default(),
            rich_plans: grove_core::rich_plan::PlanStore::default(),
            side_chats: std::collections::HashMap::default(),
            fork_origin: None,
            claude_resume: None,
            claude_subagents: std::collections::HashMap::default(),
            edit_checkpoints: Vec::new(),
            pending_edit: None,
            rewind_drafts: Vec::new(),
            provider,
            composer: grove_core::codex::ComposerSettings::default(),
            context: None,
            context_details: None,
            key: "conflict-fixture".into(),
            project: "Fixture".into(),
            cwd: "/fixture/checkout".into(),
            label: provider.label().into(),
            title: None,
            title_generation: None,
            roots: Vec::new(),
            thread_id: None,
            items: Vec::new().into(),
            work_periods: Vec::new(),
            draft: String::new(),
            draft_images: Vec::new(),
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
            activity_reporting: false,
        };
        let session = cx.new(|cx| {
            crate::entities::codex_session::CodexSession::from_connection(
                grove_core::codex::Snapshot::new(saved),
                grove_core::codex::Connection {
                    handle,
                    updates: receive.into(),
                },
                cx,
            )
        });
        let pr: PullRequest = serde_json::from_value(serde_json::json!({"number":142,"title":"Fix checkout","url":"https://github.com/o/r/pull/142","state":"OPEN","isDraft":false,"headRefOid":"head","mergeable":"CONFLICTING"})).unwrap();
        cx.update(|cx| {
            assert!(
                prepare_resolution_draft(&session, "/wrong/checkout", provider, &pr, cx).is_err()
            );
            assert!(
                prepare_resolution_draft(&session, "/fixture/checkout", provider, &pr, cx).is_ok()
            );
        });
        let draft = session.read_with(cx, |session, _| session.state.saved.draft.clone());
        assert!(
            draft.contains("PR #142")
                && draft.contains("/fixture/checkout")
                && draft.contains("Do not push or merge")
        );
        assert!(
            matches!(actions.try_recv().unwrap(), grove_core::codex::Action::Draft(text) if text == draft)
        );
        assert!(actions.try_recv().is_err());
        cx.update(|cx| {
            assert!(
                prepare_resolution_draft(&session, "/fixture/checkout", provider, &pr, cx).is_err()
            );
        });
        assert_eq!(
            session.read_with(cx, |session, _| session.state.saved.draft.clone()),
            draft
        );
        assert!(actions.try_recv().is_err());
    }
}

#[gpui::test]
fn codex_and_claude_named_prompt_dispatch_preserves_drafts(cx: &mut gpui::TestAppContext) {
    for provider in [
        grove_core::codex::Provider::Codex,
        grove_core::codex::Provider::Claude,
    ] {
        let project = scripts_project(0);
        let path = project.path.clone();
        cx.update(|cx| {
            gpui_component::init(cx);
            c::sync_component_theme(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![project.clone()],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
        });
        let (handle, actions) = grove_core::codex::Handle::channel();
        let (_updates, receive) = std::sync::mpsc::channel();
        let saved = grove_core::codex::SavedSession {
            html_policy: grove_core::visuals::HtmlPolicy::Legacy,
            visuals: Vec::new(),
            session_review: grove_core::codex::SessionReview::default(),
            rich_plans: grove_core::rich_plan::PlanStore::default(),
            side_chats: std::collections::HashMap::default(),
            fork_origin: None,
            claude_resume: None,
            claude_subagents: std::collections::HashMap::default(),
            edit_checkpoints: Vec::new(),
            pending_edit: None,
            rewind_drafts: Vec::new(),
            provider,
            composer: grove_core::codex::ComposerSettings::default(),
            context: None,
            context_details: None,
            key: "named-script-fixture".into(),
            project: project.name.clone(),
            cwd: path.clone(),
            label: provider.label().into(),
            title: None,
            title_generation: None,
            roots: Vec::new(),
            thread_id: Some("fixture-thread".into()),
            items: Vec::new().into(),
            work_periods: Vec::new(),
            draft: "Keep my draft".into(),
            draft_images: vec!["data:image/png;base64,aGVsbG8=".into()],
            draft_reference: None,
            config_args: Vec::new(),
            native_config_version: 1,
            activity_reporting: false,
        };
        let mut snapshot = grove_core::codex::Snapshot::new(saved);
        snapshot.status = grove_core::codex::Status::Idle;
        let session = cx.new(|cx| {
            crate::entities::codex_session::CodexSession::from_connection(
                snapshot,
                grove_core::codex::Connection {
                    handle: handle.clone(),
                    updates: receive.into(),
                },
                cx,
            )
        });
        let session_for_view = session.clone();
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            let id = registry.update(cx, |registry, _| {
                let id = registry.insert_meta(project.name.clone(), path.clone(), provider.agent());
                registry.attach_codex(id, session_for_view, handle);
                id
            });
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.selection = Some(Selection::Session(id));
            sidebar.initial_selection_pending = false;
            sidebar
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(
                    Action::RunNamedScript(path.clone(), path.clone(), "prompt-only".into()),
                    window,
                    cx,
                );
                assert!(sidebar.content_error.is_none());
                let receipt = sidebar
                    .checkout_panel
                    .feedback
                    .get(&path, "script-prompt-only")
                    .expect("queue receipt");
                assert_eq!(receipt.label, "Prompt queued");
                assert!(receipt.completed.is_some());
                assert!(sidebar
                    .checkout_panel
                    .feedback
                    .get("/wrong/checkout", "script-prompt-only")
                    .is_none());
            });
        });
        let submitted: Vec<_> = actions
            .try_iter()
            .filter_map(|action| match action {
                grove_core::codex::Action::ProjectPrompt { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(submitted, vec!["Review the changes".to_owned()]);
        session.read_with(cx, |session, _| {
            assert_eq!(session.state.saved.draft, "Keep my draft");
            assert_eq!(session.state.saved.draft_images.len(), 1);
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.act(
                    Action::RunNamedScript(
                        path.clone(),
                        "/wrong/checkout".into(),
                        "prompt-only".into(),
                    ),
                    window,
                    cx,
                );
                assert_eq!(
                    sidebar.content_error.as_deref(),
                    Some("The current chat belongs to another checkout")
                );
            });
        });
        assert!(!actions
            .try_iter()
            .any(|action| matches!(action, grove_core::codex::Action::ProjectPrompt { .. })));
    }
}

#[test]
fn commit_jobs_and_cancellation_follow_their_checkout_across_navigation() {
    let mut panel = CheckoutPanel::default();
    let first = std::sync::Arc::new(CommitControl::default());
    let second = std::sync::Arc::new(CommitControl::default());
    panel.select_path(Some("/project/one".into()));
    panel.operation.busy = true;
    panel.operation.commit_job = 1;
    panel.operation.commit_job_path = panel.path.clone();
    panel.operation.commit_control = Some(first.clone());
    panel.operation.commit_status = CommitUi::Generating;
    panel.select_path(Some("/project/two".into()));
    assert!(
        !panel.operation.busy,
        "another worktree must remain available"
    );
    panel.operation.busy = true;
    panel.operation.commit_job = 1;
    panel.operation.commit_job_path = panel.path.clone();
    panel.operation.commit_control = Some(second.clone());
    panel.operation.commit_status = CommitUi::Generating;
    panel.commit_event(
        "/project/one",
        1,
        CommitEvent::Progress(CommitPhase::Committing),
    );
    assert!(matches!(
        panel.operation.commit_status,
        CommitUi::Generating
    ));
    panel.select_path(Some("/project/one".into()));
    assert!(
        panel.operation.busy,
        "returning must preserve duplicate prevention"
    );
    assert!(matches!(
        panel.operation.commit_status,
        CommitUi::Committing
    ));
    assert!(std::sync::Arc::ptr_eq(
        panel.operation.commit_control.as_ref().unwrap(),
        &first
    ));
    panel.select_path(Some("/project/two".into()));
    assert!(panel.operation.commit_control.as_ref().unwrap().cancel());
    panel.operation.commit_status = CommitUi::Canceling;
    panel.select_path(None);
    panel.commit_event(
        "/project/two",
        1,
        CommitEvent::Finished(Box::new((Err("canceled".into()), Err("fixture".into())))),
    );
    panel.commit_event(
        "/project/one",
        1,
        CommitEvent::Finished(Box::new((
            Ok(CommitOutcome {
                subject: "First checkout".into(),
                hash: None,
            }),
            Err("fixture".into()),
        ))),
    );
    assert!(!panel.operation.busy);
    panel.select_path(Some("/project/two".into()));
    assert!(!panel.operation.busy);
    assert_eq!(panel.operation.message.as_deref(), Some("Commit canceled."));
    panel.select_path(Some("/project/one".into()));
    assert!(!panel.operation.busy);
    assert!(matches!(panel.operation.commit_status, CommitUi::Succeeded));
    assert_eq!(
        panel.operation.message.as_deref(),
        Some("Committed: First checkout")
    );
    panel.commit_event(
        "/project/one",
        0,
        CommitEvent::Progress(CommitPhase::Generating),
    );
    assert!(matches!(panel.operation.commit_status, CommitUi::Succeeded));
}

#[test]
fn git_failures_stay_with_the_action_and_checkout_through_other_work() {
    let mut panel = CheckoutPanel::default();
    panel.select_path(Some("/one".into()));
    panel
        .operation
        .failures
        .insert("pull", "Branches have diverged. Resolve divergence".into());
    panel.operation.commit_job = 2;
    panel.commit_event(
        "/one",
        2,
        CommitEvent::Finished(Box::new((
            Err("Signing failed. Check signing".into()),
            Err("fixture".into()),
        ))),
    );
    assert_eq!(panel.operation.failures.len(), 2);
    panel.select_path(Some("/two".into()));
    assert!(panel.operation.failures.is_empty());
    panel.update_operation("/one", |panel, current| {
        assert!(!current);
        panel
            .operation
            .failures
            .insert("push", "Push rejected".into());
    });
    assert!(panel.operation.failures.is_empty());
    panel.select_path(Some("/one".into()));
    assert_eq!(panel.operation.failures.len(), 3);
    // Late commit events cannot overwrite the current attempt or another action.
    panel.commit_event(
        "/one",
        1,
        CommitEvent::Finished(Box::new((Err("stale".into()), Err("fixture".into())))),
    );
    assert_eq!(
        panel.operation.failures["commit"],
        "Signing failed. Check signing"
    );
    // Starting a Pull retry clears only that action; unrelated failures remain.
    panel.operation.failures.remove("pull");
    assert_eq!(panel.operation.failures.len(), 2);
    assert!(panel.operation.failures.contains_key("push"));
    assert!(panel.operation.failures.contains_key("commit"));
}

#[test]
fn git_failure_copy_names_the_operation_without_repeating_guidance() {
    let (summary, guidance) = checkout_failure_copy(
        "pull",
        "Branches have diverged. Resolve the branch divergence before pulling again",
    );
    assert_eq!(summary, "Couldn’t pull: Branches have diverged");
    assert_eq!(
        guidance.as_deref(),
        Some("Resolve the branch divergence before pulling again")
    );
    let (summary, guidance) = checkout_failure_copy("publish", "Selected remote is unavailable");
    assert!(summary.starts_with("Couldn’t publish branch:"));
    assert!(guidance.is_none());
}

#[gpui::test]
fn action_cycle_rows_keep_geometry_and_reject_completed_activation(cx: &mut gpui::TestAppContext) {
    use super::super::action_feedback::Feedback;
    struct Probe {
        completed: bool,
        clicks: std::rc::Rc<std::cell::Cell<usize>>,
    }
    impl gpui::Render for Probe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let clicks = self.clicks.clone();
            let feedback = Feedback {
                token: 1,
                label: if self.completed {
                    "Tracked edits discarded"
                } else {
                    "Discarding tracked edits…"
                }
                .into(),
                completed: self.completed.then(Instant::now),
                collapse: false,
            };
            div().w_full().child(
                checkout_cycle_row(
                    "cycle-fixture",
                    "Discard tracked edits…",
                    "git",
                    false,
                    Some(feedback),
                    window,
                    cx,
                )
                .disabled(true)
                .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
            )
        }
    }
    cx.update(|cx| {
        gpui_component::init(cx);
        c::sync_component_theme(cx);
        cx.set_reduce_motion(true);
    });
    let clicks = std::rc::Rc::new(std::cell::Cell::new(0));
    let (probe, cx) = cx.add_window_view(|_, _| Probe {
        completed: false,
        clicks: clicks.clone(),
    });
    for width in [280., 180.] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(140.)));
        let mut bounds = None;
        for completed in [false, true] {
            cx.update(|window, cx| {
                probe.update(cx, |probe, _| probe.completed = completed);
                let _ = window.draw(cx);
            });
            let row = cx.debug_bounds("cycle-fixture").unwrap();
            let mark = cx.debug_bounds("cycle-fixture-icon").unwrap();
            let label = cx.debug_bounds("cycle-fixture-label").unwrap();
            assert_eq!(label.left() - mark.right(), gpui::px(SPACE_SM));
            if let Some(previous) = bounds {
                assert_eq!(row, previous);
            } else {
                bounds = Some(row);
            }
            cx.simulate_click(row.center(), gpui::Modifiers::default());
            cx.run_until_parked();
        }
    }
    assert_eq!(clicks.get(), 0);
}

#[test]
fn hidden_network_completion_preserves_other_checkout_busy_and_error() {
    let mut panel = CheckoutPanel::default();
    panel.select_path(Some("/one".into()));
    panel.operation.busy = true;
    panel.operation.network_action = Some(("Pushing…", "git-push"));
    panel.select_path(Some("/two".into()));
    assert!(!panel.operation.busy);
    panel.operation.busy = true;
    panel.update_operation("/one", |panel, current| {
        assert!(!current);
        panel.operation.busy = false;
        panel.operation.network_action = None;
        panel.operation.message = Some("Push failed".into());
    });
    assert!(panel.operation.busy);
    assert!(panel.operation.message.is_none());
    panel.select_path(Some("/one".into()));
    assert!(!panel.operation.busy);
    assert_eq!(panel.operation.message.as_deref(), Some("Push failed"));
}

#[test]
fn completion_cannot_cross_targets_or_a_return_to_the_same_path() {
    let panel = CheckoutPanel {
        path: Some("/one".into()),
        generation: 3,
        ..Default::default()
    };
    assert!(panel.accepts_result("/one", 3));
    assert!(!panel.accepts_result("/two", 3));
    assert!(!panel.accepts_result("/one", 1));
}

fn scripts_project(count: usize) -> grove_core::storage::Project {
    use grove_core::storage::{ProjectAction, ProjectActionKind, ProjectScripts};
    let mut entries: Vec<_> = (0..count)
        .map(|index| ProjectAction {
            id: format!("shell-{index}"),
            name: if index == 0 {
                "Run integration tests against local services with a long name".into()
            } else {
                format!("Script {}", index + 1)
            },
            kind: ProjectActionKind::Shell {
                command: if index == 4 {
                    (0..30)
                        .map(|line| {
                            format!("echo detailed command line {line} with full readable context")
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    format!("printf 'script {index}\\n'")
                },
            },
        })
        .collect();
    entries.push(ProjectAction {
        id: "prompt-only".into(),
        name: "Review".into(),
        kind: ProjectActionKind::Prompt {
            template: "Review the changes".into(),
        },
    });
    grove_core::storage::Project {
        name: "Scripts QA".into(),
        path: "/tmp/grove-panel-scripts-fixture".into(),
        archived: false,
        worktree_dir: None,
        scripts: ProjectScripts {
            setup: Some("echo automatic setup".into()),
            entries,
            ..Default::default()
        },
    }
}

struct ScriptsFixture(Entity<Sidebar>, grove_core::storage::Project);
impl gpui::Render for ScriptsFixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0.update(cx, |sidebar, cx| {
            if let Some(panel) = &sidebar.project_panel {
                div().size_full().child(panel.clone()).into_any_element()
            } else {
                div()
                    .id("scripts-fixture-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .child(
                        project_field_well(false, false)
                            .w_full()
                            .h_auto()
                            .px_0()
                            .py(rpx(SPACE_SM))
                            .child(checkout_row(
                                "checkout-finder",
                                "Open in Finder",
                                "folder",
                                window,
                                cx,
                            ))
                            .child(checkout_divider("checkout-scripts-divider"))
                            .child(sidebar.render_checkout_scripts(
                                &self.1,
                                &self.1.path,
                                window,
                                cx,
                            ))
                            .child(checkout_divider("checkout-divider"))
                            .child(checkout_git_heading())
                            .child(checkout_row(
                                "checkout-pr",
                                "main",
                                "git-branch",
                                window,
                                cx,
                            ))
                            .child(
                                checkout_row(
                                    "checkout-commit-publish",
                                    "No local commits",
                                    "git",
                                    window,
                                    cx,
                                )
                                .disabled(true),
                            )
                            .child(checkout_row(
                                "checkout-changes",
                                "Changes",
                                "unified",
                                window,
                                cx,
                            )),
                    )
                    .into_any_element()
            }
        })
    }
}

#[gpui::test]
fn scripts_register_limits_rows_preserves_actions_and_opens_editor(cx: &mut gpui::TestAppContext) {
    let project = scripts_project(7);
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![project.clone()],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
    });
    let owner = std::rc::Rc::new(std::cell::RefCell::new(None));
    let owner_for_view = owner.clone();
    let (_, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(crate::runtime::Runtime::new);
        let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
        *owner_for_view.borrow_mut() = Some(sidebar.clone());
        ScriptsFixture(sidebar, project.clone())
    });
    cx.simulate_resize(gpui::size(gpui::px(240.), gpui::px(800.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let sidebar = owner.borrow().clone().unwrap();
    assert!(cx.debug_bounds("checkout-script-shell-4").is_some());
    assert!(cx.debug_bounds("checkout-script-shell-5").is_none());
    assert!(cx.debug_bounds("checkout-script-prompt-only").is_none());
    let heading = cx.debug_bounds("checkout-scripts-heading").unwrap();
    let git_heading = cx.debug_bounds("checkout-git-heading").unwrap();
    assert_eq!(heading.left(), git_heading.left());
    let label_column = cx.debug_bounds("checkout-finder-label").unwrap().left();
    assert!(heading.left() < label_column);
    let finder = cx.debug_bounds("checkout-finder").unwrap();
    for selector in ["checkout-scripts-divider", "checkout-divider"] {
        let divider = cx.debug_bounds(selector).unwrap();
        assert_eq!(divider.left(), finder.left());
        assert_eq!(divider.right(), finder.right());
    }
    for selector in [
        "checkout-finder-label",
        "checkout-pr-label",
        "checkout-commit-publish-label",
        "checkout-changes-label",
        "checkout-scripts-disclosure-label",
    ] {
        assert_eq!(cx.debug_bounds(selector).unwrap().left(), label_column);
    }
    for selector in [
        "checkout-script-name-shell-0",
        "checkout-script-name-shell-1",
        "checkout-script-name-shell-2",
        "checkout-script-name-shell-3",
        "checkout-script-name-shell-4",
    ] {
        let label = cx.debug_bounds(selector).unwrap();
        assert_eq!(label.left(), label_column);
    }
    let run = cx.debug_bounds("checkout-script-run-shell-0").unwrap();
    let row = cx.debug_bounds("checkout-script-shell-0").unwrap();
    assert!(run.left() > row.left() && run.right() <= row.right());
    assert!(cx.debug_bounds("checkout-script-add").is_some());
    assert!(cx.debug_bounds("checkout-script-manage").is_some());
    let name = cx.debug_bounds("checkout-script-name-shell-4").unwrap();
    let run = cx.debug_bounds("checkout-script-run-shell-4").unwrap();
    assert!(name.right() <= run.left());
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.checkout_panel.script_details_focus[&(project.path.clone(), "shell-4".into())]
                .focus(window, cx);
        });
    });
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let context = cx.debug_bounds("checkout-script-details-text").unwrap();
    assert!(context.left() >= gpui::px(0.) && context.right() <= gpui::px(240.));
    assert!(context.size.height > gpui::px(100.));
    let open_run = cx.debug_bounds("checkout-script-run-shell-4").unwrap();
    assert_eq!(open_run, run, "the floating preview does not move the row");
    sidebar.read_with(cx, |sidebar, cx| {
        assert!(sidebar.runtime.read(cx).script_runs.is_empty());
    });
    cx.simulate_keystrokes("pagedown");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        sidebar.update(cx, |sidebar, _| {
            assert!(
                sidebar.checkout_panel.script_details_scroll
                    [&(project.path.clone(), "shell-4".into())]
                    .offset()
                    .y
                    < gpui::px(0.),
                "Page Down reveals later lines without moving focus"
            );
        });
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        sidebar.update(cx, |sidebar, _| {
            assert!(sidebar.checkout_panel.script_details_focus
                [&(project.path.clone(), "shell-4".into())]
                .is_focused(window));
        });
    });
    assert!(cx.debug_bounds("checkout-script-details-text").is_none());
    cx.simulate_resize(gpui::size(gpui::px(240.), gpui::px(800.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let disclosure = cx.debug_bounds("checkout-scripts-disclosure").unwrap();
    cx.simulate_click(disclosure.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("checkout-script-shell-6").is_some());
    assert!(cx.debug_bounds("checkout-script-prompt-only").is_some());
    let prompt_run = cx.debug_bounds("checkout-script-run-prompt-only").unwrap();
    assert_eq!(prompt_run.size.width, prompt_run.size.height);
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.act(
                Action::RunNamedScript(
                    project.path.clone(),
                    project.path.clone(),
                    "prompt-only".into(),
                ),
                window,
                cx,
            );
            assert_eq!(
                sidebar.content_error.as_deref(),
                Some("Open a native Codex or Claude chat in this checkout first")
            );
        });
    });
    let disclosure = cx.debug_bounds("checkout-scripts-disclosure").unwrap();
    cx.simulate_click(disclosure.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("checkout-script-shell-5").is_none());
    assert!(cx.debug_bounds("checkout-script-prompt-only").is_some());
    let manage = cx.debug_bounds("checkout-script-manage").unwrap();
    cx.simulate_click(manage.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("project-scripts-register").is_some());
    assert!(cx.debug_bounds("Shell command").is_none());
    cx.update(|window, cx| {
        assert_eq!(
            cx.global::<SettingsState>().store.projects[0]
                .scripts
                .entries
                .len(),
            8
        );
        sidebar.update(cx, |sidebar, cx| {
            sidebar.project_panel = None;
            sidebar.act(
                Action::EditProjectScripts(project.path.clone(), true),
                window,
                cx,
            );
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("Shell command").is_some());
    cx.update(|_, cx| {
        assert_eq!(
            cx.global::<SettingsState>().store.projects[0]
                .scripts
                .entries
                .len(),
            8
        );
    });
}

#[test]
#[ignore = "requires macOS Metal renderer; writes scripts panel screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn checkout_scripts_native_snapshots() {
    use gpui::AppContext as _;
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        std::sync::Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        c::sync_component_theme(cx);
        cx.set_reduce_motion(true);
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
    });
    for (label, width, rem, count, focus, light) in [
        ("wide", 280., 16., 7, false, false),
        ("expanded", 280., 16., 7, false, false),
        ("narrow", 220., 16., 7, false, false),
        ("focus", 280., 16., 7, true, false),
        ("bottom-context", 220., 16., 7, true, false),
        ("prompt-only", 280., 16., 0, false, false),
        ("empty", 280., 16., 0, false, false),
        ("add-many", 768., 16., 15, false, false),
        ("manage", 768., 16., 7, false, false),
        ("light-zoom", 280., 20., 7, false, true),
    ] {
        let mut project = scripts_project(count);
        if label == "empty" {
            project.scripts.entries.clear();
        }
        cx.update(|cx| {
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![project.clone()],
                ..Default::default()
            }));
            assert!(grove_core::theme::set_by_name(if light {
                c::DEFAULT_LIGHT_THEME
            } else {
                c::DEFAULT_DARK_THEME
            }));
            c::sync_component_theme(cx);
            gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
        });
        let owner = std::rc::Rc::new(std::cell::RefCell::new(None));
        let owner_for_window = owner.clone();
        let window = cx
            .open_window(
                gpui::size(
                    gpui::px(width),
                    gpui::px(if label == "bottom-context" {
                        220.
                    } else if matches!(label, "add-many" | "manage") {
                        720.
                    } else {
                        600.
                    }),
                ),
                |window, cx| {
                    let runtime = cx.new(crate::runtime::Runtime::new);
                    let sidebar = cx.new(|cx| {
                        let mut sidebar = Sidebar::new(runtime, window, cx);
                        if label == "expanded" {
                            sidebar
                                .checkout_panel
                                .scripts_show_all
                                .insert(project.path.clone());
                        }
                        sidebar
                    });
                    *owner_for_window.borrow_mut() = Some(sidebar.clone());
                    let fixture = cx.new(|_| ScriptsFixture(sidebar, project.clone()));
                    cx.new(|cx| {
                        gpui_component::Root::new(fixture, window, cx)
                            .bordered(false)
                            .bg(c::BG())
                    })
                },
            )
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            let _ = window.draw(cx);
        })
        .unwrap();
        if matches!(label, "add-many" | "manage") {
            cx.update_window(window.into(), |_, window, cx| {
                owner.borrow().as_ref().unwrap().update(cx, |sidebar, cx| {
                    sidebar.act(
                        Action::EditProjectScripts(project.path.clone(), label == "add-many"),
                        window,
                        cx,
                    );
                });
            })
            .unwrap();
            for _ in 0..3 {
                cx.run_until_parked();
                cx.update_window(window.into(), |_, window, cx| {
                    window.simulate_next_frame(cx);
                    let _ = window.draw(cx);
                })
                .unwrap();
            }
        }
        if focus {
            cx.update_window(window.into(), |_, window, cx| {
                owner.borrow().as_ref().unwrap().update(cx, |sidebar, cx| {
                    let key = (
                        project.path.clone(),
                        format!("shell-{}", if label == "bottom-context" { 4 } else { 0 }),
                    );
                    sidebar.checkout_panel.open_script_preview = Some(key.clone());
                    sidebar.checkout_panel.script_details_focus[&key].focus(window, cx);
                    cx.notify();
                });
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
        }
        let path = format!("/tmp/grove-checkout-scripts-{label}.png");
        cx.capture_screenshot(window.into())
            .unwrap()
            .save(&path)
            .unwrap();
        eprintln!("Scripts native fixture: {path}");
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
    }
}

#[test]
#[ignore = "requires macOS Metal renderer; writes screenshots to /tmp"]
#[cfg(target_os = "macos")]
fn checkout_pr_native_snapshots() {
    use gpui::AppContext as _;
    struct PrFixture(Entity<Sidebar>, CheckoutGitState);
    impl gpui::Render for PrFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |sidebar, cx| {
                div().children(sidebar.render_checkout_pr("/fixture", Some(&self.1), window, cx))
            })
        }
    }
    let mut cx = gpui::HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        std::sync::Arc::new(crate::assets::Assets),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::fonts::register(cx).unwrap();
        c::sync_component_theme(cx);
        cx.set_reduce_motion(true);
        cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
    });
    for (width, expanded) in [(280., true), (220., true), (280., false), (220., false)] {
        let mut sidebar_entity = None;
        let window = cx.open_window(gpui::size(gpui::px(width),gpui::px(700.)), |window,cx| {
                let runtime = cx.new(crate::runtime::Runtime::new);
                let sidebar = cx.new(|cx| { let mut sidebar = Sidebar::new(runtime,window,cx);
                    sidebar.checkout_panel.pr_state = Some(Ok(Some(serde_json::from_value(serde_json::json!({"number":142,"title":"A long pull request title that must yield to fixed actions and wrap readiness", "url":"https://github.com/example/project/pull/142","state":"OPEN","isDraft":false,"headRefOid":"head","headRefName":"topic","baseRefName":"main","mergeable":"MERGEABLE","mergeStateStatus":"BLOCKED","statusCheckRollup":[{"name":"CI / integration","status":"COMPLETED","conclusion":"SUCCESS"}]})).unwrap())));
                    sidebar.checkout_panel.pr_expanded = expanded; sidebar });
                sidebar_entity = Some(sidebar.clone());
                let repo = tempfile::tempdir().unwrap();
                assert!(std::process::Command::new("git").current_dir(repo.path()).args(["init", "-b", "topic"]).output().unwrap().status.success());
                let state = grove_core::checkout_git::inspect(repo.path()).unwrap();
                let fixture = cx.new(|_| PrFixture(sidebar,state));
                cx.new(|cx| gpui_component::Root::new(fixture,window,cx).bordered(false).bg(c::BG()))
            }).unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            let _ = window.draw(cx);
        })
        .unwrap();
        let image = cx.capture_screenshot(window.into()).unwrap();
        let path = if expanded {
            format!("/tmp/grove-pr-workflows-{width}.png")
        } else {
            format!("/tmp/grove-pr-workflows-{width}-collapsed.png")
        };
        image.save(&path).unwrap();
        let sidebar = sidebar_entity.unwrap();
        for (loading, pr_loading) in [(true, false), (false, true), (true, true)] {
            cx.update(|cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.checkout_panel.loading = loading;
                    sidebar.checkout_panel.pr_loading = pr_loading;
                    cx.notify();
                });
            });
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
            let refreshing = cx.capture_screenshot(window.into()).unwrap();
            assert!(
                image.as_raw() == refreshing.as_raw(),
                "cached PR summary and expanded gates must remain unchanged during refresh \
                     (width={width}, expanded={expanded}, git={loading}, pr={pr_loading})"
            );
        }
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
    }
}

#[gpui::test]
fn panel_controls_remain_reachable_on_home_and_when_hidden(cx: &mut gpui::TestAppContext) {
    let fixture = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(fixture.path())
        .args(["init", "-b", "main"])
        .output()
        .unwrap()
        .status
        .success());
    fs_err::write(fixture.path().join("main.rs"), "fn main() {}\n").unwrap();
    fs_err::write(fixture.path().join("README.md"), "# Example\n").unwrap();
    let fixture_path = fixture.path().to_string_lossy().into_owned();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "Checkout QA".into(),
                path: fixture_path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(crate::runtime::Runtime::new);
        let path = fixture_path.clone();
        let id = runtime.read(cx).registry.clone().update(cx, |registry, _| {
            registry.insert_meta("Checkout QA".into(), path.clone(), Agent::Codex)
        });
        runtime.read(cx).tree.clone().update(cx, |tree, _| {
            tree.set_active_worktrees(
                0,
                vec![grove_core::git::Worktree {
                    path,
                    branch: "main".into(),
                    mtime: None,
                    is_main: true,
                }],
            );
        });
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.selection = Some(Selection::Session(id));
        sidebar.initial_selection_pending = false;
        sidebar
    });
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.open_dashboard(window, cx));
        let _ = window.draw(cx);
    });
    for width in [1280., 760.] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.)));
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                cx.set_reduce_motion(true);
                sidebar.checkout_panel.expanded = true;
            });
            let _ = window.draw(cx);
        });
        let controls = cx.debug_bounds("side-container-controls").unwrap();
        if width == 760. {
            // Automatic fitting releases the transcript without persisting a close.
            assert!(cx.debug_bounds("checkout-card").is_none());
            assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.hidden));
            let toggle = cx.debug_bounds("session-panel-toggle").unwrap();
            cx.simulate_click(toggle.center(), gpui::Modifiers::default());
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        assert!(cx.debug_bounds("checkout-summary").is_none());
        let panel = cx.debug_bounds("checkout-panel").unwrap();
        assert_eq!(panel.top(), gpui::px(0.));
        assert_eq!(panel.size.width, gpui::px(CARD_WIDTH));
        assert!(cx.debug_bounds("side-chat-toggle").is_some());
        let toggle = cx.debug_bounds("session-panel-toggle").unwrap();
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.hidden));
        assert!(cx.debug_bounds("checkout-card").is_none());
        assert_eq!(
            cx.debug_bounds("side-container-controls").unwrap(),
            controls
        );
        let toggle = cx.debug_bounds("session-panel-toggle").unwrap();
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.hidden));
        assert!(cx.debug_bounds("checkout-card").is_some());
    }
}

#[gpui::test]
fn checkout_rows_pr_disclosure_and_filtered_changes_keep_layout(cx: &mut gpui::TestAppContext) {
    let fixture = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(fixture.path())
        .args(["init", "-b", "main"])
        .output()
        .unwrap()
        .status
        .success());
    fs_err::write(fixture.path().join("main.rs"), "fn main() {}\n").unwrap();
    fs_err::write(fixture.path().join("README.md"), "# Example\n").unwrap();
    let fixture_path = fixture.path().to_string_lossy().into_owned();
    cx.update(|cx| {
        gpui_component::init(cx);
        cx.set_global(SettingsState::new(grove_core::storage::Store {
            projects: vec![grove_core::storage::Project {
                name: "Checkout QA".into(),
                path: fixture_path.clone(),
                scripts: grove_core::storage::ProjectScripts::default(),
                archived: false,
                worktree_dir: None,
            }],
            ..Default::default()
        }));
        cx.set_global(crate::zoom::CurrentPtyDims::default());
        cx.set_global(crate::zoom::ZoomState::new(1.0));
    });
    let (sidebar, cx) = cx.add_window_view(|window, cx| {
        let runtime = cx.new(crate::runtime::Runtime::new);
        let path = fixture_path.clone();
        let id = runtime.read(cx).registry.clone().update(cx, |registry, _| {
            registry.insert_meta("Checkout QA".into(), path.clone(), Agent::Codex)
        });
        runtime.read(cx).tree.clone().update(cx, |tree, _| {
            tree.set_active_worktrees(
                0,
                vec![grove_core::git::Worktree {
                    path,
                    branch: "main".into(),
                    mtime: None,
                    is_main: true,
                }],
            );
        });
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.selection = Some(Selection::Session(id));
        sidebar.initial_selection_pending = false;
        sidebar
    });
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let card = cx.debug_bounds("checkout-card").unwrap();
    let panel = cx.debug_bounds("checkout-panel").unwrap();
    let toggle = cx.debug_bounds("files-toggle").unwrap();
    let checkout_toggle = cx.debug_bounds("session-panel-toggle").unwrap();
    assert!(checkout_toggle.right() < toggle.left());
    assert!((panel.right() - toggle.right() - gpui::px(SPACE_20)).abs() <= gpui::px(1.));
    assert!(card.top() >= checkout_toggle.bottom() + gpui::px(SPACE_LG));
    let divider = cx.debug_bounds("checkout-divider").unwrap();
    assert!(divider.left() <= card.left() + gpui::px(SPACE_XS));
    assert!(divider.right() >= card.right() - gpui::px(SPACE_XS));
    let rows = [
        (
            "checkout-finder",
            "checkout-finder-icon",
            "checkout-finder-label",
        ),
        (
            "checkout-commit-publish",
            "checkout-commit-publish-icon",
            "checkout-commit-publish-label",
        ),
        (
            "checkout-changes",
            "checkout-changes-icon",
            "checkout-changes-label",
        ),
    ];
    assert!(cx.debug_bounds("checkout-script").is_none());
    let scripts_more = cx.debug_bounds("checkout-script-manage").unwrap();
    let recovery_more = cx.debug_bounds("checkout-recovery-menu").unwrap();
    assert_eq!(scripts_more.right(), recovery_more.right());
    assert_eq!(scripts_more.size, recovery_more.size);
    let mut icon_x = None;
    for (id, icon_id, label_id) in rows {
        let row = cx.debug_bounds(id).unwrap();
        let glyph = cx.debug_bounds(icon_id).unwrap();
        let label = cx.debug_bounds(label_id).unwrap();
        assert_eq!(label.left() - glyph.right(), gpui::px(SPACE_SM));
        assert_eq!(glyph.left() - row.left(), gpui::px(SPACE_2XL));
        assert!(row.top() >= card.top() && row.bottom() <= card.bottom());
        assert_eq!(row.size.height, gpui::px(ROW_H));
        if let Some(x) = icon_x {
            assert_eq!(glyph.left(), x);
        } else {
            icon_x = Some(glyph.left());
        }
    }
    assert!(cx.debug_bounds("checkout-history").is_none());
    let changes = cx.debug_bounds("checkout-changes").unwrap();
    assert!(changes.left() <= card.left() + gpui::px(SPACE_XS));
    assert!(changes.right() >= card.right() - gpui::px(SPACE_XS));
    let initial_selection = sidebar.read_with(cx, |sidebar, _| sidebar.selection.clone());
    for provider in [Agent::Codex, Agent::Claude] {
        cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    let registry = sidebar.runtime.read(cx).registry.clone();
                    let id = registry.update(cx, |registry, _| registry.insert_meta("Checkout QA".into(), fixture_path.clone(), provider));
                    sidebar.selection = Some(Selection::Session(id));
                    sidebar.checkout_panel.root_owner = sidebar.selection.clone();
                    let state = checkout_git::inspect(fixture.path()).unwrap();
                    sidebar.checkout_panel.pr_error = None;
                    sidebar.checkout_panel.state_error = None;
                    sidebar.checkout_panel.pr_identity = Some(state.target_identity());
                sidebar.checkout_panel.pr_error = None;
                sidebar.checkout_panel.state_error = None;
                sidebar.checkout_panel.pr_checked_at = Some(Instant::now() + Duration::from_hours(1));
                    sidebar.checkout_panel.state = Some(Ok(state));
                    // This layout fixture owns synthetic Git/PR state; periodic
                    // refreshes must not replace it during slow parallel runs.
                    sidebar.checkout_panel.checked_at = None;
                    sidebar.checkout_panel.main_checked_at = Some(Instant::now() + Duration::from_hours(1));
                    sidebar.checkout_panel.pr_checked_at =
                        Some(Instant::now() + Duration::from_hours(1));
                    sidebar.checkout_panel.pr_state = Some(Ok(Some(serde_json::from_value(serde_json::json!({
                        "number":142,"url":"https://github.com/example/project/pull/142","state":"OPEN","isDraft":false,"headRefOid":"fixture","reviewDecision":"APPROVED","mergeable":"MERGEABLE","mergeStateStatus":"BLOCKED",
                        "latestReviews":[{"state":"APPROVED"}],"reviewRequests":[],"statusCheckRollup":[{"status":"COMPLETED","conclusion":"FAILURE"}]
                    })).unwrap())));
                    sidebar.checkout_panel.summary = Some(diff::summarize(&diff::changed_files(&fixture_path)));
                    sidebar.checkout_panel.pr_expanded = false;
                    cx.notify();
                });
            });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let pr = cx.debug_bounds("checkout-pr").unwrap();
        let commit = cx.debug_bounds("checkout-commit-publish").unwrap();
        let changes = cx.debug_bounds("checkout-changes").unwrap();
        assert!(changes.bottom() <= commit.top() && commit.bottom() <= pr.top());
        assert!(cx.debug_bounds("describe-changes").is_none());
        let local_more = cx.debug_bounds("checkout-local-tools").unwrap();
        assert_eq!(local_more.right(), scripts_more.right());
        assert_eq!(local_more.size, scripts_more.size);
        let changes_heading = cx.debug_bounds("checkout-changes-heading").unwrap();
        let git_heading = cx.debug_bounds("checkout-git-heading").unwrap();
        let pr_heading = cx.debug_bounds("checkout-pr-heading").unwrap();
        let scripts_heading = cx.debug_bounds("checkout-scripts-heading").unwrap();
        assert!(changes_heading.bottom() <= changes.top());
        assert!(changes.bottom() <= git_heading.top());
        assert!(git_heading.bottom() <= commit.top());
        assert!(commit.bottom() <= pr_heading.top() && pr_heading.bottom() <= pr.top());
        assert!(pr.bottom() <= scripts_heading.top());

        assert!(cx.debug_bounds("checkout-local-heading").is_none());
        assert!(cx.debug_bounds("checkout-pr-heading").is_some());
        assert!(cx.debug_bounds("checkout-merge-total").is_none());
        assert!(cx.debug_bounds("checkout-pr-readiness").is_none());
        let panel = cx.debug_bounds("checkout-panel").unwrap();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: panel.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-2000.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let details = cx.debug_bounds("checkout-pr-details").unwrap();
        cx.simulate_click(details.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.pr_expanded));
        cx.simulate_keystrokes("space");
        cx.run_until_parked();
        assert!(!sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.pr_expanded));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(sidebar.read_with(cx, |sidebar, _| sidebar.checkout_panel.pr_expanded));
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let expanded = cx.debug_bounds("checkout-pr-expanded").unwrap();
        let disclosure_label = cx.debug_bounds("checkout-pr-label").unwrap();
        let checks = cx.debug_bounds("checkout-pr-checks").unwrap();
        assert_eq!(checks.left(), disclosure_label.left());
        assert!(cx.debug_bounds("checkout-pr-number").is_none());
        assert!(cx.debug_bounds("checkout-pr-resolve").is_none());
        let commit = cx.debug_bounds("checkout-commit-publish").unwrap();
        let changes = cx.debug_bounds("checkout-changes").unwrap();
        assert!(changes.bottom() <= commit.top() && commit.bottom() <= expanded.top());
        for group in [diff::ChangeGroup::Code, diff::ChangeGroup::Docs] {
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            // Expanded readiness shares the existing checkout scroll region.
            // Scroll the actual panel, then prove lower actions are visible and clickable.
            let panel = cx.debug_bounds("checkout-panel").unwrap();
            let group_row = cx.debug_bounds(group.label()).unwrap();
            if group_row.bottom() > panel.bottom() || group_row.top() < panel.top() {
                cx.simulate_event(gpui::ScrollWheelEvent {
                    position: panel.center(),
                    delta: gpui::ScrollDelta::Pixels(gpui::point(
                        gpui::px(0.),
                        panel.center().y - group_row.center().y,
                    )),
                    modifiers: gpui::Modifiers::default(),
                    touch_phase: gpui::TouchPhase::Moved,
                });
                cx.run_until_parked();
                cx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
            }
            let group_row = cx.debug_bounds(group.label()).unwrap();
            assert!(
                group_row.top() >= panel.top() && group_row.bottom() <= panel.bottom(),
                "checkout scrolling reveals the complete change category action"
            );
            cx.simulate_click(group_row.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    let viewer = sidebar.diff_viewer.as_ref().unwrap().read(cx);
                    assert_eq!(viewer.group, Some(group));
                    assert!(!viewer.files.is_empty());
                    assert!(viewer
                        .files
                        .iter()
                        .all(|file| diff::ChangeGroup::of(&file.path) == group));
                    sidebar.act(Action::CloseDiff, window, cx);
                });
            });
        }
        let panel = cx.debug_bounds("checkout-panel").unwrap();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: panel.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-2000.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let pr_row = cx.debug_bounds("checkout-pr").unwrap();
        assert!(
            pr_row.top() >= panel.top() && pr_row.bottom() <= panel.bottom(),
            "checkout scrolling returns to PR actions"
        );
    }
    // A server-ready PR exposes merge only with a synchronized, clean checkout.
    let (saved_state, saved_pr) = sidebar.read_with(cx, |sidebar, _| {
        (
            sidebar.checkout_panel.state.clone(),
            sidebar.checkout_panel.pr_state.clone(),
        )
    });
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar
                .checkout_panel
                .operation
                .failures
                .insert("recovery", "No tracked edits to discard".into());
            let state = sidebar
                .checkout_panel
                .state
                .as_mut()
                .unwrap()
                .as_mut()
                .unwrap();
            state.head = Some("fixture".into());
            state.dirty = false;
            state.upstream = Some("origin/main".into());
            state.ahead = Some(0);
            state.behind = Some(0);
            sidebar.checkout_panel.pr_identity = Some(state.target_identity());
            let pr = sidebar
                .checkout_panel
                .pr_state
                .as_mut()
                .unwrap()
                .as_mut()
                .unwrap()
                .as_mut()
                .unwrap();
            pr.title =
                "Improve checkout actions with a deliberately long title for truncation".into();
            pr.mergeable = "MERGEABLE".into();
            pr.merge_state_status = "CLEAN".into();
            pr.status_check_rollup = Some(vec![
                serde_json::json!({"status":"COMPLETED","conclusion":"SUCCESS"}),
            ]);
            cx.notify();
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let merge = cx.debug_bounds("checkout-pr-merge").unwrap();
    cx.simulate_click(merge.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("checkout-confirm-merge").is_some());
    assert!(cx.debug_bounds("checkout-recovery-failure").is_none());
    assert!(cx.debug_bounds("checkout-card").is_some());
    assert!(sidebar.read_with(cx, |sidebar, _| sidebar.confirmation_open()));
    cx.simulate_keystrokes("tab");
    cx.update(|window, cx| assert!(sidebar.read(cx).cancel_focus.is_focused(window)));
    cx.simulate_keystrokes("tab");
    cx.update(|window, cx| assert!(sidebar.read(cx).confirm_focus.is_focused(window)));
    cx.simulate_keystrokes("shift-tab");
    cx.update(|window, cx| assert!(sidebar.read(cx).cancel_focus.is_focused(window)));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        assert!(sidebar.read(cx).checkout_panel.trigger_focus["pr-merge"].is_focused(window));
    });
    assert!(cx.debug_bounds("checkout-confirm-merge").is_none());
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.checkout_panel.state = saved_state;
            sidebar.checkout_panel.pr_state = saved_pr;
            sidebar.checkout_panel.pr_identity = sidebar
                .checkout_panel
                .state
                .as_ref()
                .and_then(|state| state.as_ref().ok())
                .map(CheckoutGitState::target_identity);
            cx.notify();
        });
    });
    cx.update(|_, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.selection.clone_from(&initial_selection);
            sidebar
                .checkout_panel
                .root_owner
                .clone_from(&initial_selection);
            cx.notify();
        });
    });
    assert!(cx.debug_bounds("checkout-path").is_none());
    assert!(cx.debug_bounds("checkout-branch").is_none());
    cx.simulate_resize(gpui::size(gpui::px(500.), gpui::px(600.)));
    cx.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.checkout_panel.hidden = true;
            sidebar.toggle_session_panel(window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let header = cx.debug_bounds("terminal-header-1").unwrap();
    let panel = cx.debug_bounds("checkout-panel").unwrap();
    assert_eq!(header.top(), gpui::px(0.));
    assert!(panel.top() >= header.bottom());
    assert!(cx.debug_bounds("checkout-summary").is_none());
    let narrow_card = cx.debug_bounds("checkout-card").unwrap();
    let narrow_pr = cx.debug_bounds("checkout-pr").unwrap();
    assert!(narrow_pr.left() >= narrow_card.left() && narrow_pr.right() <= narrow_card.right());
    assert!(cx.debug_bounds("checkout-pr-expanded").is_some());
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    assert!(cx.debug_bounds("checkout-card").is_some());

    for width in [1280., 500.] {
        cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.)));
        for selection in [
            Some(Selection::Project(0)),
            Some(Selection::Session(SessionId::from_raw(u64::MAX))),
            None,
        ] {
            cx.update(|_, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    sidebar.selection = selection;
                    sidebar.checkout_panel.expanded = true;
                    assert!(sidebar.checkout_targets(cx).is_empty());
                    cx.notify();
                });
            });
            cx.run_until_parked();
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
            assert!(cx.debug_bounds("checkout-card").is_none());
            assert!(cx.debug_bounds("checkout-panel").is_none());
            assert!(cx.debug_bounds("checkout-summary").is_none());
        }
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.selection = Some(Selection::Worktree(0, fixture_path.clone()));
                sidebar.checkout_panel.expanded = true;
                // Restoring the selected checkout deliberately reopens the
                // panel after each viewport change in this geometry fixture.
                sidebar.checkout_panel.hidden = true;
                sidebar.toggle_session_panel(window, cx);
                assert_eq!(
                    sidebar.checkout_targets(cx),
                    vec![(0, fixture_path.clone())]
                );
                cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(cx.debug_bounds("checkout-card").is_some());
        assert!(cx.debug_bounds("checkout-terminal").is_some());
    }
}
