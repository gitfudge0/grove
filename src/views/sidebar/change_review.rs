//! Checkout-owned reviews. Every run and freshness check retains its source key.
use super::*;
use crate::views::{
    components::transcript_action,
    status::{self, StatusKind},
};
use gpui_component::{
    menu::{PopupMenu, PopupMenuItem},
    text::{TextView, TextViewState},
    Disableable, Selectable,
};
use grove_core::{
    change_description::Scope,
    change_review::{self as domain, Review},
    commit_agent::CommitControl,
    review_skill::ScopePreference,
};
use std::sync::Arc;

#[path = "change_review_session.rs"]
mod session;

const READER_TOOLBAR_INSET: f32 = CHROME_CONTROL_H * 3. + SPACE_LG * 2. + SPACE_20 * 2.;
const REVIEW_WIDTH: f32 = 700.;
const FRESHNESS_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Key {
    owner: String,
    pub(super) path: String,
    scope: String,
}
#[derive(Default)]
pub(super) struct Reviews {
    entries: HashMap<Key, Entry>,
    open: Option<Key>,
    scopes: HashMap<(String, String), Scope>,
    revision: u64,
    focus: Option<FocusHandle>,
    source: Option<FocusHandle>,
    return_focus: Option<FocusHandle>,
    open_source: Option<FocusHandle>,
    focus_pending: bool,
    scope_focus: Option<FocusHandle>,
    scope_menu: Option<Entity<PopupMenu>>,
    scope_subscription: Option<gpui::Subscription>,
    scope_menu_owner: Option<(String, String)>,
    scope_position: gpui::Point<gpui::Pixels>,
    scope_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    sessions: HashMap<SessionId, gpui::Subscription>,
    hydrated: HashSet<(SessionId, String)>,
    linked: HashMap<SessionId, Key>,
    #[cfg(test)]
    store_root: Option<std::path::PathBuf>,
}
#[derive(Default)]
struct Entry {
    scope: Option<Scope>,
    session: Option<SessionId>,
    controller: Option<Entity<crate::views::change_session::ChangeSession<Review>>>,
    saving: bool,
    result: Option<Arc<Review>>,
    run: u64,
    cancel: Option<Arc<CommitControl>>,
    resolving: bool,
    error: Option<String>,
    freshness: Option<Result<bool, String>>,
    checked_at: Option<Instant>,
    checking: bool,
    expanded: HashSet<String>,
    text: Option<Entity<TextViewState>>,
    file_text: HashMap<String, Entity<TextViewState>>,
    disclosure_focus: HashMap<String, FocusHandle>,
    diff: Option<(String, Entity<TextViewState>)>,
    receipt: Option<String>,
    scroll: ScrollHandle,
    diff_scroll: ScrollHandle,
}

fn owner(selection: &Option<Selection>) -> String {
    format!("{selection:?}")
}
fn key(selection: &Option<Selection>, path: &str, scope: &Scope) -> Key {
    Key {
        owner: owner(selection),
        path: path.into(),
        scope: match scope {
            Scope::Local => "local".into(),
            Scope::PullRequest { url } => url.clone(),
        },
    }
}
fn action(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    glyph: &str,
    window: &Window,
    cx: &App,
) -> gpui_component::button::Button {
    let id: SharedString = id.into();
    let debug_id = id.to_string();
    transcript_action(id, label, false, window, cx)
        .debug_selector(move || debug_id.clone())
        .icon(
            gpui_component::Icon::default()
                .path(SharedString::from(format!("icons/{glyph}.svg")))
                .size(rpx(ICON_SM)),
        )
}
fn review_source_label(scope: &Scope) -> String {
    match scope {
        Scope::Local => "Local changes".into(),
        Scope::PullRequest { url } => format!(
            "PR #{}",
            url.trim_end_matches('/').rsplit('/').next().unwrap_or("")
        ),
    }
}
fn review_source_text(label: String) -> gpui::Div {
    div()
        .debug_selector(|| "review-source-context".into())
        .pl(rpx(SPACE_2XL + ICON_LG + SPACE_SM))
        .pr(rpx(SPACE_2XL))
        .text_size(rpx(TEXT_MICRO))
        .text_color(c::FG_DIM())
        .child(label)
}
fn text_view(view: &Entity<TextViewState>, window: &Window) -> TextView {
    TextView::new(view)
        .markdown_extensions(super::super::mermaid::extensions())
        .style(super::super::codex_view::markdown_style(window))
        .selectable(true)
}
impl Sidebar {
    pub(super) fn review_is_open(&self, cx: &App) -> bool {
        let targets = self.checkout_targets(cx);
        let selected = super::checkout_panel::selected_checkout(
            &targets,
            self.checkout_panel.chosen_root.as_deref(),
        );
        self.reviews.open.as_ref().is_some_and(|key| {
            if key.owner == owner(&self.selection) && self.linked_review_key().as_ref() == Some(key)
            {
                return true;
            }
            key.owner == owner(&self.selection)
                && selected.as_ref().is_some_and(|(_, path)| path == &key.path)
        })
    }
    pub(super) fn close_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = self.reviews.open.clone() {
            if let Some(entry) = self.reviews.entries.get_mut(&key) {
                // A read-only PR lookup may finish, but leaving must not start a model run later.
                entry.resolving = false;
                if entry.diff.is_some() {
                    entry.diff = None;
                    if let Some(focus) = &self.reviews.focus {
                        focus.focus(window, cx);
                    }
                    cx.notify();
                    return;
                }
            }
        }
        self.reviews.open = None;
        if let Some(source) = self.reviews.return_focus.take() {
            source.focus(window, cx);
        } else if let Some(Selection::Session(id)) = self.selection {
            if let Some(view) = self.codex_views.get(&id) {
                view.focus_handle(cx).focus(window, cx);
            } else {
                self.focus.focus(window, cx);
                self.pending_canvas_focus = Some(id);
            }
        }
        cx.notify();
    }
    fn review_command_source(
        &self,
        preference: &ScopePreference,
        cx: &App,
    ) -> Result<(String, Option<Scope>), String> {
        let linked = self.linked_review_key();
        if *preference == ScopePreference::Current {
            if let Some(target) = &linked {
                if let Some(scope) = self
                    .reviews
                    .entries
                    .get(target)
                    .and_then(|e| e.scope.clone())
                {
                    return Ok((target.path.clone(), Some(scope)));
                }
            }
        }
        let targets = self.checkout_targets(cx);
        let path = super::checkout_panel::selected_checkout(
            &targets,
            self.checkout_panel.chosen_root.as_deref(),
        )
        .map(|(_, path)| path)
        .or_else(|| linked.map(|target| target.path))
        .ok_or("Select a checkout before reviewing changes.")?;
        let scope = match preference {
            ScopePreference::Local => Some(Scope::Local),
            ScopePreference::PullRequest => None,
            ScopePreference::PullRequestUrl(url) => Some(Scope::PullRequest { url: url.clone() }),
            ScopePreference::Current => match self
                .reviews
                .scopes
                .get(&(owner(&self.selection), path.clone()))
            {
                Some(Scope::PullRequest { .. }) => None,
                _ => Some(Scope::Local),
            },
        };
        Ok((path, scope))
    }
    pub(super) fn invoke_review_skill(
        &mut self,
        preference: ScopePreference,
        source: FocusHandle,
        cx: &mut Context<Self>,
    ) {
        let (path, scope) = match self.review_command_source(&preference, cx) {
            Ok(source) => source,
            Err(error) => {
                self.review_error(error, cx);
                return;
            }
        };
        self.files_active = false;
        self.checkout_panel.side_chat_owner = None;
        if let Some(scope) = scope {
            if matches!(scope, Scope::Local) && preference != ScopePreference::PullRequest {
                self.reviews
                    .scopes
                    .insert((owner(&self.selection), path.clone()), scope.clone());
            }
            self.start_review_conversation(path, scope, Some(source), cx);
            return;
        }
        // Resolve the attached PR independently of the visibility and cached loading state of Checkout.
        let mut resolving_key = key(&self.selection, &path, &Scope::Local);
        resolving_key.scope = "resolving".into();
        self.reviews.revision = self.reviews.revision.wrapping_add(1);
        let run = self.reviews.revision;
        let entry = self
            .reviews
            .entries
            .entry(resolving_key.clone())
            .or_default();
        entry.run = run;
        entry.resolving = true;
        entry.error = None;
        self.reviews.open = Some(resolving_key.clone());
        self.reviews.return_focus = Some(source.clone());
        self.reviews.focus_pending = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let result: Result<Option<grove_core::checkout_pr::PullRequest>, String> = cx.background_spawn(async move {
                let state = grove_core::checkout_git::inspect(std::path::Path::new(&target))?;
                let pr = grove_core::checkout_pr::inspect(std::path::Path::new(&target), &state)?;
                if grove_core::checkout_git::inspect(std::path::Path::new(&target))?.target_identity() != state.target_identity() {
                    return Err("Checkout changed while resolving its pull request. Run the skill again.".into());
                }
                Ok(pr)
            }).await;
            let _ = this.update(cx, |this, cx| {
                let active = this.reviews.open.as_ref() == Some(&resolving_key) && this.reviews.entries.get(&resolving_key).is_some_and(|entry| entry.run == run && entry.resolving);
                if !active { return; }
                let targets = this.checkout_targets(cx);
                let still_selected = owner(&this.selection) == resolving_key.owner && super::checkout_panel::selected_checkout(&targets, this.checkout_panel.chosen_root.as_deref()).is_some_and(|(_, current)| current == path);
                let scope: Result<Scope, String> = match result {
                    Ok(Some(pr)) if still_selected => Ok(Scope::PullRequest { url: pr.url }),
                    Ok(None) if still_selected => Err("This checkout has no attached PR. Run /review local to review local edits.".into()),
                    Ok(_) => Err("Checkout selection changed. Run the skill again in its intended checkout.".into()),
                    Err(error) => Err(error),
                };
                if let Some(entry) = this.reviews.entries.get_mut(&resolving_key) { entry.resolving = false; if let Err(error) = &scope { entry.error = Some(error.clone()); } }
                if let Ok(scope) = scope {
                    this.reviews.scopes.insert((resolving_key.owner.clone(), path.clone()), scope.clone());
                    this.start_review_conversation(path, scope, Some(source), cx);
                }
                cx.notify();
            });
        }).detach();
    }
    fn review_error(&self, error: impl Into<String>, cx: &mut Context<Self>) {
        self.runtime
            .read(cx)
            .toast
            .clone()
            .update(cx, |toast, cx| toast.set_error(error.into(), cx));
    }
    fn begin_review(
        &mut self,
        path: String,
        scope: Scope,
        source: Option<FocusHandle>,
        cx: &mut Context<Self>,
    ) {
        if let Some(target) = self.linked_review_key() {
            if target.path == path
                && self
                    .reviews
                    .entries
                    .get(&target)
                    .and_then(|e| e.scope.as_ref())
                    == Some(&scope)
            {
                self.run_review(&target, cx);
                return;
            }
        }
        self.start_review_conversation(path, scope, source, cx);
    }
    fn cancel_review(&mut self, key: &Key, cx: &mut Context<Self>) {
        if let Some(entry) = self.reviews.entries.get(key) {
            if !entry.saving {
                if let Some(controller) = entry.controller.clone() {
                    controller.update(
                        cx,
                        crate::views::change_session::ChangeSession::<Review>::cancel,
                    );
                }
            }
        }
    }
    fn check_review(&mut self, key: &Key, cx: &mut Context<Self>) {
        let Some(entry) = self.reviews.entries.get_mut(key) else {
            return;
        };
        if entry.cancel.is_some()
            || entry.checking
            || entry
                .checked_at
                .is_some_and(|t| t.elapsed() < FRESHNESS_INTERVAL)
        {
            return;
        }
        let Some(review) = entry.result.clone() else {
            return;
        };
        entry.checking = true;
        let run = entry.run;
        let key = key.clone();
        cx.spawn(async move |this, cx| {
            let path = key.path.clone();
            let expected_result = review.clone();
            let result = cx
                .background_spawn(async move {
                    domain::is_current(std::path::Path::new(&path), &review)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(entry) = this.reviews.entries.get_mut(&key).filter(|entry| {
                    entry.run == run
                        && entry
                            .result
                            .as_ref()
                            .is_some_and(|current| Arc::ptr_eq(current, &expected_result))
                }) {
                    entry.checking = false;
                    entry.checked_at = Some(Instant::now());
                    entry.freshness = Some(result);
                    cx.notify();
                }
            });
            cx.background_executor().timer(FRESHNESS_INTERVAL).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }
    pub(super) fn render_review_controls(
        &mut self,
        path: &str,
        local_changes: bool,
        pr: Option<grove_core::checkout_pr::PullRequest>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.restore_session_review(cx);
        let menu_owner = (owner(&self.selection), path.to_owned());
        if self.reviews.scope_menu.is_some()
            && (self.reviews.scope_menu_owner.as_ref() != Some(&menu_owner)
                || !local_changes
                || pr.is_none()
                || self.linked_review_key().is_some())
        {
            self.reviews.scope_menu = None;
            self.reviews.scope_subscription = None;
            self.reviews.scope_menu_owner = None;
        }
        if let Some(target) = self.linked_review_key() {
            self.check_review(&target, cx);
            let entry = self.reviews.entries.get(&target);
            let busy = entry.is_some_and(|e| e.cancel.is_some());
            let stale = entry.is_some_and(|e| matches!(e.freshness, Some(Ok(false))));
            let unavailable =
                entry.is_some_and(|e| matches!(e.freshness, Some(Err(_))) || e.error.is_some());
            let saved = entry.is_some_and(|e| e.result.is_some());
            let source = entry
                .and_then(|e| e.scope.as_ref())
                .map(review_source_label);
            let source_focus = self
                .reviews
                .open_source
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            let return_focus = source_focus.clone();
            return div()
                .id("review-controls")
                .debug_selector(|| "review-controls".into())
                .flex()
                .flex_col()
                .gap(rpx(SPACE_SM))
                .py(rpx(SPACE_LG))
                .child(
                    super::checkout_panel::checkout_loading_row(
                        "review-open",
                        if saved {
                            "View review"
                        } else {
                            "Review changes"
                        },
                        "diff",
                        false,
                        window,
                        cx,
                    )
                    .with_focus_handle(source_focus)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.reload_review_result(&target, cx);
                        this.reviews.return_focus = Some(return_focus.clone());
                        this.reviews.open = Some(target.clone());
                        this.reviews.focus_pending = true;
                        cx.notify();
                    })),
                )
                .when_some(source, |column, source| {
                    column.child(review_source_text(source))
                })
                .when(busy || stale || unavailable, |column| {
                    column.child(div().px(rpx(SPACE_2XL)).child(status::label(
                        "review-panel-state",
                        if busy {
                            StatusKind::Working
                        } else {
                            StatusKind::Warning
                        },
                        if busy {
                            "Generating…"
                        } else if stale {
                            "Out of date"
                        } else {
                            "Needs attention"
                        },
                        ICON_SM,
                        cx,
                    )))
                })
                .into_any_element();
        }
        if !local_changes && pr.is_none() {
            return div().into_any_element();
        }
        let owner_key = (owner(&self.selection), path.to_owned());
        let scope = self
            .reviews
            .scopes
            .entry(owner_key.clone())
            .or_insert_with(|| {
                pr.as_ref().map_or(Scope::Local, |pr| Scope::PullRequest {
                    url: pr.url.clone(),
                })
            })
            .clone();
        let scope = match (&scope, &pr) {
            (_, Some(pr)) if !local_changes => Scope::PullRequest {
                url: pr.url.clone(),
            },
            (Scope::PullRequest { .. }, Some(pr)) => Scope::PullRequest {
                url: pr.url.clone(),
            },
            (Scope::PullRequest { .. }, None) => Scope::Local,
            _ => scope,
        };
        self.reviews.scopes.insert(owner_key.clone(), scope.clone());
        let key = key(&self.selection, path, &scope);
        let entry = self.reviews.entries.get(&key);
        let busy = entry.is_some_and(|e| e.cancel.is_some() || e.saving);
        let exists = entry.is_some_and(|e| e.result.is_some());
        let mut column = div()
            .id("review-controls")
            .debug_selector(|| "review-controls".into())
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM))
            .py(rpx(SPACE_LG));
        // A sole source is context, not a choice. Only offer a picker when
        // local edits and the attached PR are both available.
        if let Some(pr) = pr.as_ref().filter(|_| local_changes) {
            let selected = scope.clone();
            let target = owner_key.clone();
            let pr_scope = Scope::PullRequest {
                url: pr.url.clone(),
            };
            let focus = self
                .reviews
                .scope_focus
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            let keyboard_target = target.clone();
            let keyboard_selected = selected.clone();
            let keyboard_pr = pr_scope.clone();
            let keyboard_focus = focus.clone();
            let bounds = self.reviews.scope_bounds.clone();
            column = column.child(
                div()
                    .mx(rpx(SPACE_2XL))
                    .on_prepaint(move |value, _, _| bounds.set(value))
                    .child(
                        action(
                            "review-source",
                            review_source_label(&scope),
                            "chev-down",
                            window,
                            cx,
                        )
                        .justify_start()
                        .with_focus_handle(focus)
                        .selected(
                            self.reviews.scope_menu.is_some()
                                && self.reviews.scope_menu_owner.as_ref() == Some(&owner_key),
                        )
                        .disabled(busy)
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                let position = this.reviews.scope_bounds.get().bottom_left();
                                this.open_review_sources(
                                    target.clone(),
                                    selected.clone(),
                                    pr_scope.clone(),
                                    position,
                                    window,
                                    cx,
                                );
                            },
                        )),
                    )
                    .on_key_down(cx.listener(
                        move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if !busy
                                && keyboard_focus.is_focused(window)
                                && matches!(event.keystroke.key.as_str(), "enter" | "space")
                            {
                                let position = this.reviews.scope_bounds.get().bottom_left();
                                this.open_review_sources(
                                    keyboard_target.clone(),
                                    keyboard_selected.clone(),
                                    keyboard_pr.clone(),
                                    position,
                                    window,
                                    cx,
                                );
                                window.prevent_default();
                                cx.stop_propagation();
                            }
                        },
                    )),
            );
            if self.reviews.scope_menu_owner.as_ref() == Some(&owner_key) && !busy {
                if let Some(menu) = self.reviews.scope_menu.clone() {
                    column = column.child(gpui::deferred(
                        gpui::anchored()
                            .position_mode(gpui::AnchoredPositionMode::Window)
                            .position(self.reviews.scope_position)
                            .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
                            .child(menu),
                    ));
                }
            } else {
                self.reviews.scope_menu = None;
                self.reviews.scope_subscription = None;
                self.reviews.scope_menu_owner = None;
            }
        } else if exists || matches!(scope, Scope::PullRequest { .. }) {
            column = column.child(review_source_text(review_source_label(&scope)));
        }
        let focus = self
            .reviews
            .source
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let target = path.to_owned();
        let chosen = scope.clone();
        if !exists {
            column = column.child(
                super::checkout_panel::checkout_loading_row(
                    "review-changes",
                    if entry.is_some_and(|e| e.saving) {
                        "Saving review…"
                    } else if busy {
                        "Reviewing changes…"
                    } else {
                        match &scope {
                            Scope::Local => "Review local changes",
                            Scope::PullRequest { .. } => "Review pull request",
                        }
                    },
                    "diff",
                    busy,
                    window,
                    cx,
                )
                .with_focus_handle(focus)
                .disabled(busy || (matches!(scope, Scope::Local) && !local_changes))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.begin_review(
                        target.clone(),
                        chosen.clone(),
                        this.reviews.source.clone(),
                        cx,
                    );
                })),
            );
        }
        if exists {
            let open_focus = self
                .reviews
                .open_source
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            let return_focus = open_focus.clone();
            let target = key.clone();
            column = column.child(
                super::checkout_panel::checkout_loading_row(
                    "review-open",
                    "View review",
                    "diff",
                    false,
                    window,
                    cx,
                )
                .with_focus_handle(open_focus)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.reviews.return_focus = Some(return_focus.clone());
                    this.reviews.open = Some(target.clone());
                    this.reviews.focus_pending = true;
                    cx.notify();
                })),
            );
        }
        if exists && entry.is_some_and(|e| matches!(e.freshness, Some(Ok(false)))) {
            column = column.child(div().px(rpx(SPACE_2XL)).child(status::label(
                "review-panel-state",
                StatusKind::Warning,
                "Out of date",
                ICON_SM,
                cx,
            )));
        }
        if busy {
            let target = key.clone();
            column = column.child(
                action("review-cancel", "Cancel generation", "close", window, cx)
                    .mx(rpx(SPACE_2XL))
                    .disabled(entry.is_some_and(|e| e.saving))
                    .on_click(cx.listener(move |this, _, _, cx| this.cancel_review(&target, cx))),
            );
        }
        column
            .child(
                div()
                    .text_size(rpx(TEXT_MICRO))
                    .text_color(c::FG_DIM())
                    .pl(rpx(SPACE_2XL + ICON_LG + SPACE_SM))
                    .pr(rpx(SPACE_2XL))
                    .child(if matches!(scope, Scope::Local) {
                        "Uncommitted changes, including new files."
                    } else {
                        "Pull request changes compared with its base."
                    }),
            )
            .into_any_element()
    }
    fn open_review_sources(
        &mut self,
        target: (String, String),
        selected: Scope,
        pr_scope: Scope,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = cx.entity().downgrade();
        let source = target.clone();
        let Some(focus) = self.reviews.scope_focus.clone() else {
            return;
        };
        let menu = PopupMenu::build(window, cx, move |menu, window, _| {
            let mut menu =
                crate::views::components::dropdown_menu(menu, window).action_context(focus);
            for candidate in [Scope::Local, pr_scope.clone()] {
                let owner = owner.clone();
                let target = target.clone();
                menu = menu.item(
                    PopupMenuItem::new(review_source_label(&candidate))
                        .icon(gpui_component::Icon::default().path(match candidate {
                            Scope::Local => "icons/diff.svg",
                            Scope::PullRequest { .. } => "icons/git-pull-request.svg",
                        }))
                        .checked(candidate == selected)
                        .on_click(move |_, _, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                this.reviews
                                    .scopes
                                    .insert(target.clone(), candidate.clone());
                                cx.notify();
                            });
                        }),
                );
            }
            menu
        });
        self.reviews.scope_subscription = Some(cx.subscribe_in(
            &menu,
            window,
            |this, _, _: &gpui::DismissEvent, window, cx| {
                this.reviews.scope_menu = None;
                this.reviews.scope_subscription = None;
                this.reviews.scope_menu_owner = None;
                if let Some(focus) = &this.reviews.scope_focus {
                    focus.focus(window, cx);
                }
                cx.notify();
            },
        ));
        self.reviews.scope_menu_owner = Some(source);
        self.reviews.scope_position = position;
        menu.focus_handle(cx).focus(window, cx);
        self.reviews.scope_menu = Some(menu);
        cx.notify();
    }
    pub(super) fn render_review(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(key) = self.reviews.open.clone() else {
            return div().into_any_element();
        };
        super::super::mermaid::set_context(Some(cx.entity_id()), window, cx);
        self.check_review(&key, cx);
        let focus = self
            .reviews
            .focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        if self.reviews.focus_pending {
            focus.focus(window, cx);
            self.reviews.focus_pending = false;
        }
        if let Some(entry) = self.reviews.entries.get_mut(&key) {
            if let Some(review) = &entry.result {
                for file in &review.files {
                    entry
                        .disclosure_focus
                        .entry(file.path.clone())
                        .or_insert_with(|| cx.focus_handle());
                }
            }
        }
        let Some(entry) = self.reviews.entries.get(&key) else {
            return div().into_any_element();
        };
        let busy = entry.cancel.is_some();
        let mut header = div()
            .flex()
            .items_center()
            .gap(rpx(SPACE_LG))
            .min_h(rpx(APPBAR_TOTAL_H))
            .px(rpx(SPACE_3XL))
            .when(
                self.checkout_panel.toggle_available
                    && (self.checkout_panel.hidden
                        || Self::logical_window_width(window)
                            < super::checkout_panel::CHECKOUT_SIDE_VIEWPORT),
                |header| header.pr(rpx(READER_TOOLBAR_INSET)),
            );
        header = header.child(
            action(
                "review-back",
                if entry.diff.is_some() {
                    "Back to review"
                } else {
                    "Conversation"
                },
                "arrow-left",
                window,
                cx,
            )
            .on_click(cx.listener(|this, _, window, cx| this.close_review(window, cx))),
        );
        let label = entry
            .result
            .as_ref()
            .map_or("Review changes".into(), |d| d.snapshot.label.clone());
        header = header.child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(c::FG_DIM())
                .text_size(rpx(TEXT_SMALL))
                .child(label),
        );
        let mut body = div()
            .id("review-reader-body")
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .w_full()
            .min_w_0()
            .max_w(rpx(REVIEW_WIDTH));
        if entry.resolving {
            body = body.child(status::label(
                "review-resolving",
                StatusKind::Working,
                "Finding the attached pull request…",
                ICON_SM,
                cx,
            ));
        }
        if busy {
            body = body.child(status::label(
                "review-generating",
                StatusKind::Working,
                if entry.saving {
                    "Saving review…"
                } else {
                    "Reading the diff and reviewing changes…"
                },
                ICON_SM,
                cx,
            ));
            let target = key.clone();
            body = body.child(
                action("review-cancel", "Cancel generation", "close", window, cx)
                    .disabled(entry.saving)
                    .on_click(cx.listener(move |this, _, _, cx| this.cancel_review(&target, cx))),
            );
        }
        if let Some(error) = &entry.error {
            body = body.child(status::label(
                "review-error",
                if error.to_lowercase().contains("cancel") {
                    StatusKind::Canceled
                } else {
                    StatusKind::Failed
                },
                error.clone(),
                ICON_SM,
                cx,
            ));
            if !busy && entry.scope.is_some() {
                let path = key.path.clone();
                let scope = entry.scope.clone().unwrap_or(Scope::Local);
                body = body.child(
                    action("review-retry", "Retry", "refresh", window, cx).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.begin_review(path.clone(), scope.clone(), None, cx);
                        },
                    )),
                );
            }
        }
        if !busy
            && !entry.resolving
            && entry.result.is_none()
            && entry.error.is_none()
            && entry.controller.is_some()
        {
            let target = key.clone();
            body = body
                .child(
                    div()
                        .text_color(c::FG_DIM())
                        .child("No saved review yet. Generate it from this conversation."),
                )
                .child(
                    action(
                        "review-empty-generate",
                        "Generate review",
                        "diff",
                        window,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.run_review(&target, cx))),
                );
        }
        if let Some((path, view)) = &entry.diff {
            body = body
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(path.clone()),
                )
                .child(
                    div()
                        .text_color(c::FG_DIM())
                        .text_size(rpx(TEXT_SMALL))
                        .child("Read-only diff from the reviewed snapshot"),
                )
                .child(text_view(view, window));
        } else if let Some(review) = &entry.result {
            let omitted = review
                .snapshot
                .files
                .iter()
                .filter(|file| {
                    file.omitted.is_some()
                        || !review
                            .files
                            .iter()
                            .any(|reviewed| reviewed.path == file.path)
                })
                .count();
            body = body.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(format!(
                        "{} files · snapshot {}",
                        review.snapshot.files.len(),
                        review
                            .snapshot
                            .fingerprint
                            .chars()
                            .take(8)
                            .collect::<String>()
                    )),
            );
            match entry.freshness.as_ref() {
                Some(Ok(false)) => {
                    body = body.child(status::label(
                        "review-stale",
                        StatusKind::Warning,
                        "Out of date. Regenerate to review the latest snapshot.",
                        ICON_SM,
                        cx,
                    ));
                }
                Some(Err(error)) => {
                    body = body.child(status::label(
                        "review-freshness-error",
                        StatusKind::Warning,
                        format!("Could not verify current changes: {error}"),
                        ICON_SM,
                        cx,
                    ));
                }
                None => {
                    body = body.child(
                        div()
                            .text_color(c::FG_DIM())
                            .text_size(rpx(TEXT_SMALL))
                            .child("Checking whether these changes are current…"),
                    );
                }
                _ => {}
            }
            if let Some(text) = &entry.text {
                body = body.child(text_view(text, window));
            }
            body = body.child(
                div()
                    .mt(rpx(SPACE_LG))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Reviewed files"),
            );
            let mut previous = String::new();
            let mut ordered_files = review.files.iter().collect::<Vec<_>>();
            ordered_files.sort_by(|a, b| a.category.cmp(&b.category).then(a.path.cmp(&b.path)));
            for file in ordered_files {
                if previous != file.category {
                    body = body.child(
                        div()
                            .mt(rpx(SPACE_LG))
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(file.category.clone()),
                    );
                    previous.clone_from(&file.category);
                }
                let expanded = entry.expanded.contains(&file.path);
                let target = key.clone();
                let path = file.path.clone();
                let snapshot_file = review.snapshot.files.iter().find(|f| f.path == file.path);
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .child(
                            crate::views::components::inline_disclosure(
                                SharedString::from(format!("review-file-{}", file.path)),
                                file.path.clone(),
                            )
                            .debug_selector({
                                let path = file.path.clone();
                                move || format!("review-file-{path}")
                            })
                            .w_full()
                            .flex_1()
                            .min_w_0()
                            .justify_start()
                            .gap(rpx(SPACE_SM))
                            .aria_expanded(expanded)
                            .track_focus(&entry.disclosure_focus[&file.path])
                            .tab_index(0)
                            .focus_visible(|style| style.bg(c::BG_HOVER()).text_color(c::FG()))
                            .child(icon(
                                if expanded { "chev-down" } else { "chev-right" },
                                ICON_SM,
                                c::FG_DIM(),
                            ))
                            .child(div().min_w_0().truncate().child(file.path.clone()))
                            .on_click(cx.listener({
                                let focus = entry.disclosure_focus[&file.path].clone();
                                let target = target.clone();
                                let path = path.clone();
                                move |this, _, window, cx| {
                                    focus.focus(window, cx);
                                    if let Some(entry) = this.reviews.entries.get_mut(&target) {
                                        if !entry.expanded.remove(&path) {
                                            entry.expanded.insert(path.clone());
                                        }
                                    }
                                    cx.notify();
                                }
                            }))
                            .on_key_down(cx.listener({
                                let focus = entry.disclosure_focus[&file.path].clone();
                                move |this, event: &gpui::KeyDownEvent, window, cx| {
                                    if focus.is_focused(window)
                                        && matches!(event.keystroke.key.as_str(), "enter" | "space")
                                    {
                                        if let Some(entry) = this.reviews.entries.get_mut(&target) {
                                            if !entry.expanded.remove(&path) {
                                                entry.expanded.insert(path.clone());
                                            }
                                        }
                                        window.prevent_default();
                                        cx.stop_propagation();
                                        cx.notify();
                                    }
                                }
                            })),
                        )
                        .when_some(snapshot_file, |row, f| {
                            row.child(
                                div()
                                    .flex_shrink_0()
                                    .text_color(c::FG_DIM())
                                    .text_size(rpx(TEXT_SMALL))
                                    .child(format!("+{} / −{}", f.added, f.removed)),
                            )
                        }),
                );
                if expanded {
                    if let Some(view) = entry.file_text.get(&file.path) {
                        body = body.child(div().pl(rpx(SPACE_3XL)).child(text_view(view, window)));
                    }
                    if let Some(f) = snapshot_file {
                        if let Some(reason) = &f.omitted {
                            body = body.child(
                                div()
                                    .pl(rpx(SPACE_3XL))
                                    .text_color(c::FG_DIM())
                                    .child(reason.clone()),
                            );
                        }
                        let target = key.clone();
                        let path = file.path.clone();
                        let patch = f.patch.clone();
                        body = body.child(
                            action(
                                format!("review-diff-{}", file.path),
                                "Open diff",
                                "diff",
                                window,
                                cx,
                            )
                            .disabled(patch.is_empty())
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(entry) = this.reviews.entries.get_mut(&target) {
                                        entry.diff = Some((
                                            path.clone(),
                                            cx.new(|cx| {
                                                TextViewState::markdown(
                                                    &domain::patch_markdown(&patch),
                                                    cx,
                                                )
                                            }),
                                        ));
                                    }
                                    this.reviews.focus_pending = true;
                                    cx.notify();
                                },
                            )),
                        );
                    }
                }
            }
            body = body.child(div().mt(rpx(SPACE_LG)).text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child(format!("{} of {} files reviewed · {omitted} files omitted or partial. Based on the diff; test results were not verified by this review.", review.files.len(), review.snapshot.files.len())));
            for omitted_file in review.snapshot.files.iter().filter(|file| {
                !review
                    .files
                    .iter()
                    .any(|reviewed| reviewed.path == file.path)
            }) {
                body = body.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(format!(
                            "{} · {}",
                            omitted_file.path,
                            omitted_file
                                .omitted
                                .as_deref()
                                .unwrap_or("Not returned by the review model")
                        )),
                );
            }
            let markdown = review.markdown();
            let target = key.clone();
            let path = key.path.clone();
            let scope = entry.scope.clone().unwrap_or(Scope::Local);
            let actions = div()
                .flex()
                .flex_wrap()
                .gap(rpx(SPACE_LG))
                .child(
                    action("review-copy", "Copy Markdown", "copy", window, cx).on_click(
                        cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                markdown.clone(),
                            ));
                            if let Some(entry) = this.reviews.entries.get_mut(&target) {
                                entry.receipt = Some("Markdown copied".into());
                            }
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    action("review-regenerate", "Regenerate", "refresh", window, cx)
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.begin_review(path.clone(), scope.clone(), None, cx);
                        })),
                );
            body = body.child(actions);
            if let Some(receipt) = &entry.receipt {
                body = body.child(status::label(
                    "review-receipt",
                    StatusKind::Success,
                    receipt.clone(),
                    ICON_SM,
                    cx,
                ));
            }
        }
        let reader = div()
            .id("review-reader")
            .debug_selector(|| "review-reader".into())
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .track_focus(&focus)
            .child(header)
            .child(
                div()
                    .id("review-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(if entry.diff.is_some() {
                        &entry.diff_scroll
                    } else {
                        &entry.scroll
                    })
                    .px(rpx(SPACE_20))
                    .py(rpx(SPACE_3XL))
                    .flex()
                    .justify_center()
                    .child(body),
            );
        // Reading/selection is immediate; content replacement uses shared restrained entrance.
        motion::fast(reader, "review-reader-enter", cx)
    }
}

#[cfg(test)]
#[path = "change_review_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "change_review_controls_tests.rs"]
mod controls_tests;
