//! Manual worktree form: suggestions stop permanently at the first branch edit.
use super::Sidebar;
use crate::{
    settings::SettingsState,
    theme as c,
    views::{rpx, tokens::*},
};
use gpui::{div, prelude::*, AnyElement, Context, Window};
use std::process::Command;

fn suggested_branch(name: &str) -> String {
    let slug = name
        .trim()
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        String::new()
    } else {
        format!("feat/{slug}")
    }
}

fn resolve_source(project: &str, revision: &str) -> String {
    let output = Command::new("git")
        .args([
            "-C",
            project,
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ])
        .output();
    let Some(sha) = output.ok().filter(|out| out.status.success()).map(|out| {
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .chars()
            .take(7)
            .collect::<String>()
    }) else {
        return "Could not resolve source. Enter an existing branch or revision.".into();
    };
    let source = if revision == "HEAD" {
        Command::new("git")
            .args(["-C", project, "symbolic-ref", "--quiet", "--short", "HEAD"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map_or_else(
                || "Detached HEAD".into(),
                |out| String::from_utf8_lossy(&out.stdout).trim().to_string(),
            )
    } else {
        revision.to_owned()
    };
    format!("Starts from {source} · {sha}")
}

impl Sidebar {
    pub(super) fn worktree_field_changed(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pending_new_worktree.is_none() {
            return;
        }
        match index {
            0 if !self.worktree_branch_edited => {
                let suggestion = suggested_branch(self.worktree_name.read(cx).value().as_ref());
                // set_value suppresses Change events, so only real edits claim branch ownership.
                self.worktree_branch
                    .update(cx, |input, cx| input.set_value(suggestion, window, cx));
                self.worktree_errors[1] = None;
            }
            1 => self.worktree_branch_edited = true,
            2 => self.refresh_worktree_source(cx),
            _ => {}
        }
    }

    pub(super) fn refresh_worktree_source(&mut self, cx: &mut Context<Self>) {
        self.worktree_source_generation += 1;
        let generation = self.worktree_source_generation;
        let Some(idx) = self.pending_new_worktree else {
            return;
        };
        let Some(project) = cx
            .global::<SettingsState>()
            .store
            .projects
            .get(idx)
            .map(|p| p.path.clone())
        else {
            return;
        };
        let revision = self.worktree_base.read(cx).value().trim().to_owned();
        self.worktree_source = if revision.is_empty() {
            "Enter a base branch or revision."
        } else {
            "Resolving source…"
        }
        .into();
        if revision.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let lookup_project = project.clone();
            let lookup_revision = revision.clone();
            let source = cx
                .background_executor()
                .spawn(async move { resolve_source(&lookup_project, &lookup_revision) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.worktree_source_generation != generation
                    || this.pending_new_worktree != Some(idx)
                    || this.worktree_base.read(cx).value().trim() != revision
                    || cx
                        .global::<SettingsState>()
                        .store
                        .projects
                        .get(idx)
                        .is_none_or(|p| p.path != project)
                {
                    return;
                }
                this.worktree_source = source;
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_worktree_summary(&self, cx: &Context<Self>) -> AnyElement {
        let destination = self.pending_new_worktree.and_then(|idx| {
            let project = cx.global::<SettingsState>().store.projects.get(idx)?;
            Some(
                grove_core::git::worktrees_root()
                    .ok()?
                    .join(project.worktree_dir())
                    .join(self.worktree_name.read(cx).value().trim())
                    .to_string_lossy()
                    .to_string(),
            )
        });
        div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM))
            .min_w_0()
            .child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(self.worktree_source.clone()),
            )
            .when_some(destination, |el, path| {
                let escaped = path
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");
                el.child(
                    div()
                        .min_w_0()
                        .mt(rpx(SPACE_SM))
                        .child(
                            div()
                                .text_size(rpx(TEXT_MICRO))
                                .text_color(c::FG_DIM())
                                .child("Folder · derived from worktree name"),
                        )
                        .child(
                            gpui_component::text::TextView::html("worktree-destination", escaped)
                                .selectable(true)
                                .font_family(crate::fonts::UI_FAMILY)
                                .text_size(rpx(TEXT_SMALL))
                                .line_height(rpx(CARD_LINE_SM_H))
                                .text_color(c::FG_DIM())
                                .min_w_0(),
                        ),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn branch_suggestion_is_a_valid_git_name() {
        for (name, expected) in [
            ("billing-retry", "feat/billing-retry"),
            (" Billing retry ", "feat/billing-retry"),
            ("foo..bar/@{bad}.lock", "feat/foo-bar-bad-lock"),
            ("", ""),
            ("///", ""),
        ] {
            assert_eq!(suggested_branch(name), expected);
        }
    }
    #[test]
    fn source_resolves_actual_head_detached_and_invalid_revision() {
        let repo = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-qb", "actual-source"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "Fixture",
            ],
        ] {
            assert!(Command::new("git")
                .current_dir(repo.path())
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let path = repo.path().to_str().unwrap();
        assert!(resolve_source(path, "HEAD").starts_with("Starts from actual-source · "));
        assert!(resolve_source(path, "not-a-revision").starts_with("Could not resolve source"));
        assert!(Command::new("git")
            .current_dir(repo.path())
            .args(["checkout", "--detach", "-q"])
            .status()
            .unwrap()
            .success());
        assert!(resolve_source(path, "HEAD").starts_with("Starts from Detached HEAD · "));
    }
    #[gpui::test]
    fn manual_branch_edits_including_clear_survive_name_changes_until_fresh_form(
        cx: &mut gpui::TestAppContext,
    ) {
        let repo = super::super::tests::ChangedGitRepo::new();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "branch-input".into(),
                    path: repo.path(),
                    scripts: grove_core::storage::ProjectScripts::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            Sidebar::new(runtime, window, cx)
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.begin_worktree(0, window, cx));
        });
        for (field, value, expected) in [
            (0, "Billing retry", "feat/billing-retry"),
            (0, "next-name", "feat/next-name"),
            (1, "fix/customer-branch", "fix/customer-branch"),
            (0, "renamed", "fix/customer-branch"),
            (1, "", ""),
            (0, "renamed-again", ""),
        ] {
            cx.update(|window, cx| {
                sidebar.update(cx, |sidebar, cx| {
                    let input = if field == 0 {
                        &sidebar.worktree_name
                    } else {
                        &sidebar.worktree_branch
                    };
                    input.update(cx, |input, cx| input.replace_all(value, window, cx));
                });
            });
            cx.run_until_parked();
            assert_eq!(
                sidebar.read_with(cx, |sidebar, cx| sidebar
                    .worktree_branch
                    .read(cx)
                    .value()
                    .to_string()),
                expected
            );
        }
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                sidebar.begin_worktree(0, window, cx);
                sidebar
                    .worktree_name
                    .update(cx, |input, cx| input.replace_all("fresh-name", window, cx));
            });
        });
        cx.run_until_parked();
        assert_eq!(
            sidebar.read_with(cx, |sidebar, cx| sidebar
                .worktree_branch
                .read(cx)
                .value()
                .to_string()),
            "feat/fresh-name"
        );
    }
    #[test]
    #[ignore = "requires macOS Metal renderer; writes screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn manual_worktree_native_snapshots() {
        use gpui::AppContext as _;
        let repo = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "Fixture",
            ],
        ] {
            assert!(std::process::Command::new("git")
                .current_dir(repo.path())
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let repo_path = repo.path().to_string_lossy().to_string();
        let mut cx = gpui::HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            std::sync::Arc::new(crate::assets::Assets),
            gpui_platform::current_headless_renderer,
        );
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_reduce_motion(true);
            cx.set_global(crate::zoom::CurrentPtyDims::default());
        });
        for (state, width, rem) in [
            ("normal", 1000., 16.),
            ("narrow", 360., 16.),
            ("error", 720., 20.),
            ("long", 720., 20.),
        ] {
            cx.update(|cx| {
                cx.set_global(SettingsState::new(grove_core::storage::Store {
                    projects: vec![grove_core::storage::Project {
                        name: "Manual fixture".into(),
                        path: repo_path.clone(),
                        scripts: grove_core::storage::ProjectScripts::default(),
                        archived: false,
                        worktree_dir: None,
                    }],
                    ..Default::default()
                }));
                grove_core::theme::set(grove_core::theme::by_name("grove-dark").unwrap());
                c::sync_component_theme(cx);
                gpui_component::Theme::global_mut(cx).font_size = gpui::px(rem);
            });
            let mut fixture_sidebar = None;
            let window = cx
                .open_window(gpui::size(gpui::px(width), gpui::px(980.)), |window, cx| {
                    let runtime = cx.new(crate::runtime::Runtime::new);
                    let sidebar = cx.new(|cx| Sidebar::new(runtime, window, cx));
                    fixture_sidebar = Some(sidebar.clone());
                    cx.new(|cx| {
                        gpui_component::Root::new(sidebar, window, cx)
                            .bordered(false)
                            .bg(c::BG())
                    })
                })
                .unwrap();
            cx.run_until_parked();
            // Seed only after initial navigation/background discovery finishes.
            cx.update_window(window.into(), |_, window, cx| {
                fixture_sidebar.as_ref().unwrap().update(cx, |sidebar, cx| {
                    sidebar.begin_worktree(0, window, cx);
                    assert_eq!(sidebar.pending_new_worktree, Some(0));
                    sidebar
                        .worktree_name
                        .update(cx, |i, cx| i.replace_all("billing-retry", window, cx));
                });
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                fixture_sidebar.as_ref().unwrap().update(cx, |sidebar, cx| {
                    assert_eq!(sidebar.pending_new_worktree, Some(0));
                    assert_eq!(
                        sidebar.worktree_branch.read(cx).value().as_ref(),
                        "feat/billing-retry"
                    );
                    if state == "error" {
                        sidebar.worktree_errors[0] = Some(
                            "This worktree folder already exists. Choose another name.".into(),
                        );
                    }
                    if state == "long" {
                        sidebar.worktree_name.update(cx, |i, cx| {
                            i.replace_all(
                                "billing-retry-preserve-invoice-after-provider-timeout",
                                window,
                                cx,
                            );
                        });
                    }
                    cx.notify();
                });
                let _ = window.draw(cx);
            })
            .unwrap();
            // The selectable destination TextView parses updated source asynchronously.
            // Wait for that parse after the first seeded draw, then capture its next frame.
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
            cx.update(|cx| {
                let sidebar = fixture_sidebar.as_ref().unwrap().read(cx);
                assert_eq!(sidebar.pending_new_worktree, Some(0));
                assert!(sidebar.worktree_source.starts_with("Starts from "));
                assert!(!sidebar.worktree_branch_edited);
            });
            let path = format!("/tmp/grove-manual-worktree-{state}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&path)
                .unwrap();
            eprintln!("Manual worktree native fixture: {path}");
            if state == "normal" {
                // Exercise actual InputState Change events, including deleting the branch.
                for (field, value, expected) in [
                    (1, "fix/custom", "fix/custom"),
                    (0, "second-name", "fix/custom"),
                    (1, "", ""),
                    (0, "third-name", ""),
                ] {
                    cx.update_window(window.into(), |_, window, cx| {
                        fixture_sidebar.as_ref().unwrap().update(cx, |sidebar, cx| {
                            let input = if field == 0 {
                                &sidebar.worktree_name
                            } else {
                                &sidebar.worktree_branch
                            };
                            input.update(cx, |i, cx| i.replace_all(value, window, cx));
                        });
                    })
                    .unwrap();
                    cx.run_until_parked();
                    cx.update(|cx| {
                        let sidebar = fixture_sidebar.as_ref().unwrap().read(cx);
                        assert_eq!(sidebar.worktree_branch.read(cx).value().as_ref(), expected);
                        assert!(sidebar.worktree_branch_edited);
                    });
                }
                cx.update_window(window.into(), |_, window, cx| {
                    fixture_sidebar.as_ref().unwrap().update(cx, |sidebar, cx| {
                        sidebar.begin_worktree(0, window, cx);
                        sidebar
                            .worktree_name
                            .update(cx, |i, cx| i.replace_all("fresh-name", window, cx));
                    });
                })
                .unwrap();
                cx.run_until_parked();
                cx.update(|cx| {
                    let sidebar = fixture_sidebar.as_ref().unwrap().read(cx);
                    assert_eq!(
                        sidebar.worktree_branch.read(cx).value().as_ref(),
                        "feat/fresh-name"
                    );
                    assert!(!sidebar.worktree_branch_edited);
                });
                eprintln!("Native branch interaction: automatic suggestion, manual override, cleared override, fresh-form reset passed");
            }
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}
