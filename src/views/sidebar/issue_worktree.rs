//! Optional issue lookup embedded in the existing worktree creation form.
use super::Sidebar;
use crate::{
    settings::SettingsState,
    theme as c,
    views::{
        components::{form_action, form_field, readonly_value},
        rpx,
        tokens::*,
    },
};
use gpui::{div, prelude::*, AnyElement, Context, Entity, Window};
#[cfg(test)]
use gpui_component::ElementExt as _;
use gpui_component::{input::InputState, Disableable};
use grove_core::issues::{self, ResolvedIssue};

pub(super) struct IssueWorktreeState {
    reference: Entity<InputState>,
    site: Entity<InputState>,
    email: Entity<InputState>,
    token: Entity<InputState>,
    provider: &'static str,
    connect: bool,
    pub(super) busy: bool,
    generation: u64,
    pub(super) resolved: Option<ResolvedIssue>,
    error: Option<String>,
    status: Option<String>,
    _observers: Vec<gpui::Subscription>,
    #[cfg(test)]
    reference_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
}
impl IssueWorktreeState {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Sidebar>) -> Self {
        let reference =
            cx.new(|cx| InputState::new(window, cx).placeholder("Issue URL or ABC-123"));
        let observer = cx.subscribe(&reference, |this, _, event, cx| {
            if matches!(event, gpui_component::input::InputEvent::Change) {
                this.issue_flow.generation += 1;
                this.issue_flow.busy = false;
                this.issue_flow.resolved = None;
                this.issue_flow.error = None;
                cx.notify();
            }
        });
        Self {
            reference,
            site: cx
                .new(|cx| InputState::new(window, cx).placeholder("https://team.atlassian.net")),
            email: cx.new(|cx| InputState::new(window, cx).placeholder("Atlassian account email")),
            token: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("API token")
                    .masked(true)
            }),
            provider: "Linear",
            connect: false,
            busy: false,
            generation: 0,
            resolved: None,
            error: None,
            status: None,
            _observers: vec![observer],
            #[cfg(test)]
            reference_bounds: std::rc::Rc::default(),
        }
    }
    pub(super) fn reset(&mut self, window: &mut Window, cx: &mut Context<Sidebar>) {
        self.generation += 1;
        self.busy = false;
        self.resolved = None;
        self.error = None;
        self.status = None;
        self.connect = false;
        for field in [&self.reference, &self.site, &self.email, &self.token] {
            field.update(cx, |i, cx| i.set_value("", window, cx));
        }
    }
}
impl Sidebar {
    fn issue_project(&self, cx: &Context<Self>) -> Option<String> {
        self.pending_new_worktree
            .and_then(|idx| cx.global::<SettingsState>().store.projects.get(idx))
            .map(|p| p.path.clone())
    }
    fn resolve_worktree_issue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.issue_flow.busy {
            return;
        }
        let Some(project) = self.issue_project(cx) else {
            return;
        };
        let reference = self.issue_flow.reference.read(cx).value().to_string();
        if let Err(e) = issues::parse_reference(&reference) {
            self.issue_flow.error = Some(e);
            cx.notify();
            return;
        }
        self.issue_flow.busy = true;
        self.issue_flow.error = None;
        self.issue_flow.resolved = None;
        self.issue_flow.generation += 1;
        let generation = self.issue_flow.generation;
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let lookup_project = project.clone();
            let result = cx
                .background_executor()
                .spawn(async move { issues::resolve(&lookup_project, &reference) })
                .await;
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.issue_flow.generation != generation
                        || this.issue_project(cx).as_deref() != Some(&project)
                    {
                        return;
                    }
                    this.issue_flow.busy = false;
                    match result {
                        Ok(issue) => {
                            let suggestion = issue.suggested_branch();
                            this.worktree_name
                                .update(cx, |i, cx| i.set_value(suggestion.clone(), window, cx));
                            this.worktree_branch
                                .update(cx, |i, cx| i.set_value(suggestion, window, cx));
                            this.worktree_errors = [None, None, None];
                            this.issue_flow.resolved = Some(issue);
                        }
                        Err(e) => this.issue_flow.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
        cx.notify();
    }
    fn save_issue_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.issue_flow.busy {
            return;
        }
        let Some(project) = self.issue_project(cx) else {
            return;
        };
        let provider = self.issue_flow.provider;
        let site = self.issue_flow.site.read(cx).value().to_string();
        let email = self.issue_flow.email.read(cx).value().to_string();
        let token = self.issue_flow.token.read(cx).value().to_string();
        self.issue_flow
            .token
            .update(cx, |i, cx| i.set_value("", window, cx));
        self.issue_flow.busy = true;
        self.issue_flow.error = None;
        self.issue_flow.generation += 1;
        let generation = self.issue_flow.generation;
        cx.spawn(async move |this, cx| {
            let result =
                cx.background_executor()
                    .spawn(async move {
                        issues::save_connection(&project, provider, &site, &email, &token)
                    })
                    .await;
            let _ = this.update(cx, |this, cx| {
                if this.issue_flow.generation != generation {
                    return;
                }
                this.issue_flow.busy = false;
                match result {
                    Ok(()) => {
                        this.issue_flow.connect = false;
                        this.issue_flow.status = Some(
                            "Connection saved securely. Resolve the issue to verify access.".into(),
                        );
                    }
                    Err(e) => this.issue_flow.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn render_issue_worktree_fields(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let busy = self.issue_flow.busy;
        let destination = self.pending_new_worktree.and_then(|idx| {
            let project = cx.global::<SettingsState>().store.projects.get(idx)?;
            let root = grove_core::git::worktrees_root().ok()?;
            Some(
                root.join(project.worktree_dir())
                    .join(self.worktree_name.read(cx).value().trim())
                    .to_string_lossy()
                    .to_string(),
            )
        });
        let issue_field = div().child(form_field(
            "issue-reference-field",
            "Create from issue (optional)",
            &self.issue_flow.reference,
            self.issue_flow.error.as_deref(),
            window,
            cx,
        ));
        #[cfg(test)]
        let issue_field = {
            let bounds = self.issue_flow.reference_bounds.clone();
            issue_field.on_prepaint(move |rect, _, _| bounds.set(rect))
        };
        div().flex().flex_col().gap(rpx(SPACE_LG))
            .when_some(self.issue_flow.status.as_ref(), |el, status| el.child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child(status.clone())))
            .child(issue_field)
            .child(div().flex().flex_wrap().gap(rpx(SPACE_SM))
                .child(form_action("resolve-worktree-issue",if busy{"Resolving…"}else{"Resolve issue"},false,window,cx).disabled(busy).on_click(cx.listener(|this,_,window,cx|this.resolve_worktree_issue(window,cx))))
                .child(form_action("issue-connection-settings","Issue connections",false,window,cx).disabled(busy).on_click(cx.listener(|this,_,_,cx|{this.issue_flow.connect = !this.issue_flow.connect;cx.notify();}))))
            .when(self.issue_flow.connect,|el|el.child(div().flex().flex_col().gap(rpx(SPACE_LG))
                .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child("GitHub uses gh auth login. Jira and Linear credentials are saved only in your OS credential store for this project."))
                .child(div().flex().gap(rpx(SPACE_SM))
                    .child(form_action("connect-linear","Linear",false,window,cx).disabled(busy).on_click(cx.listener(|this,_,_,cx|{this.issue_flow.provider = "Linear";cx.notify();})))
                    .child(form_action("connect-jira","Jira",false,window,cx).disabled(busy).on_click(cx.listener(|this,_,_,cx|{this.issue_flow.provider = "Jira";cx.notify();}))))
                .child(div().text_size(rpx(TEXT_SMALL)).child(format!("Connect {}",self.issue_flow.provider)))
                .when(self.issue_flow.provider=="Jira",|el|el
                    .child(form_field("jira-site-field","Jira Cloud site",&self.issue_flow.site,None,window,cx))
                    .child(form_field("jira-email-field","Account email",&self.issue_flow.email,None,window,cx)))
                .child(form_field("issue-token-field","Personal API token",&self.issue_flow.token,None,window,cx))
                .child(form_action("save-issue-connection","Save connection",true,window,cx).disabled(busy).on_click(cx.listener(|this,_,window,cx|this.save_issue_connection(window,cx))))))
            .when_some(destination, |el, path| el.child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child("Destination (edit Worktree name below)").child(readonly_value("issue-destination",path))))
            .when_some(self.issue_flow.resolved.as_ref(),|el,issue|el.child(div().flex().flex_col().gap(rpx(SPACE_SM))
                .child(div().child(readonly_value("resolved-issue-title",format!("{} · {} · {}",issue.provider,issue.id,issue.title))))
                .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child(readonly_value("resolved-issue-description",issue.description.chars().take(600).collect::<String>())))
                .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child("After creation, choose a tool from the worktree launch menu."))))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires macOS Metal renderer; writes screenshots to /tmp"]
    #[cfg(target_os = "macos")]
    fn issue_worktree_native_snapshots() {
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
            ("input", 1000., 16.),
            ("auth", 720., 16.),
            ("error", 720., 20.),
            ("resolved", 1000., 20.),
        ] {
            cx.update(|cx| {
                cx.set_global(SettingsState::new(grove_core::storage::Store {
                    projects: vec![grove_core::storage::Project {
                        name: "Issue fixture".into(),
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
                    sidebar.issue_flow.reference.update(cx, |i, cx| {
                        i.set_value("https://linear.app/grove/issue/GROVE-123", window, cx);
                    });
                });
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                fixture_sidebar.as_ref().unwrap().update(cx, |sidebar, cx| {
                    assert_eq!(sidebar.pending_new_worktree, Some(0));
                    sidebar.issue_flow.connect = state == "auth";
                    sidebar.issue_flow.provider = "Jira";

                    if state == "error" { sidebar.issue_flow.error = Some("Issue access denied. Repair the provider connection and retry.".into()); }
                    if state == "resolved" {
                        sidebar.issue_flow.resolved = Some(ResolvedIssue { provider:"Linear", immutable_id:"fixture-id".into(), id:"GROVE-123".into(), url:"https://linear.app/grove/issue/GROVE-123".into(), title:"A long issue title remains readable while configuring the new worktree".into(), description:"Resolved issue content is shown as data. Creating the worktree opens its launch menu and starts no agent.".into() });
                        sidebar.worktree_name.update(cx, |i,cx| i.set_value("grove-123-worktree",window,cx));
                        sidebar.worktree_branch.update(cx, |i,cx| i.set_value("grove-123-worktree",window,cx));
                        sidebar.worktree_base.update(cx, |i,cx| i.set_value("main",window,cx));
                    }
                    cx.notify();
                });
                let _ = window.draw(cx);
            }).unwrap();
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
                assert_eq!(sidebar.issue_flow.resolved.is_some(), state == "resolved");
                assert_eq!(sidebar.issue_flow.error.is_some(), state == "error");
                assert_eq!(sidebar.issue_flow.connect, state == "auth");
                let bounds = sidebar.issue_flow.reference_bounds.get();
                assert!(
                    bounds.size.width > gpui::px(0.) && bounds.size.height > gpui::px(0.),
                    "issue-reference-field must render before capturing {state}"
                );
            });
            let path = format!("/tmp/grove-issue-worktree-{state}.png");
            cx.capture_screenshot(window.into())
                .unwrap()
                .save(&path)
                .unwrap();
            eprintln!("Issue worktree native fixture: {path}");
            cx.update_window(window.into(), |_, window, _| window.remove_window())
                .unwrap();
        }
    }
}
