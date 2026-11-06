//! Checkout-owned descriptions. Every run, freshness check and publication retains its source key.
use super::*;
use crate::views::{
    components::{form_action, project_field_well, transcript_action},
    status::{self, StatusKind},
};
use gpui_component::{
    input::{Input, InputState},
    menu::{PopupMenu, PopupMenuItem},
    text::{TextView, TextViewState},
    Disableable, Selectable,
};
use grove_core::{
    change_description::{self as domain, Description, Scope},
    commit_agent::CommitControl,
    describe_skill::ScopePreference,
};
use std::sync::Arc;
#[path = "change_description_session.rs"]
mod session;

const DESCRIPTION_WIDTH: f32 = 700.;
// The persistent side toolbar has three shared square controls and two gaps.
const READER_TOOLBAR_INSET: f32 = CHROME_CONTROL_H * 3. + SPACE_LG * 2. + SPACE_20 * 2.;
const FRESHNESS_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Key {
    owner: String,
    pub(super) path: String,
    scope: String,
}
#[derive(Default)]
pub(super) struct Descriptions {
    entries: HashMap<Key, Entry>,
    open: Option<Key>,
    scopes: HashMap<(String, String), Scope>,
    revision: u64,
    focus: Option<FocusHandle>,
    source: Option<FocusHandle>,
    scope_focus: Option<FocusHandle>,
    scope_menu: Option<Entity<PopupMenu>>,
    scope_subscription: Option<gpui::Subscription>,
    scope_menu_owner: Option<(String, String)>,
    scope_position: gpui::Point<gpui::Pixels>,
    scope_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    return_focus: Option<FocusHandle>,
    open_source: Option<FocusHandle>,
    focus_pending: bool,
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
    session_key: Option<String>,
    pending: Option<session::Pending>,
    result: Option<Arc<Description>>,
    run: u64,
    cancel: Option<Arc<CommitControl>>,
    publishing: bool,
    finalizing: bool,
    resolving: bool,
    error: Option<String>,
    freshness: Option<Result<bool, String>>,
    checked_at: Option<Instant>,
    checking: bool,
    expanded: HashSet<String>,
    text: Option<Entity<TextViewState>>,
    file_text: HashMap<String, Entity<TextViewState>>,
    disclosure_focus: HashMap<String, FocusHandle>,
    diagram: Option<Entity<TextViewState>>,
    diff: Option<(String, Entity<TextViewState>)>,
    preview: Option<Entity<InputState>>,
    receipt: Option<String>,
    scroll: ScrollHandle,
    diff_scroll: ScrollHandle,
    preview_scroll: ScrollHandle,
}
impl Drop for Descriptions {
    fn drop(&mut self) {
        for entry in self.entries.values() {
            if let Some(cancel) = &entry.cancel {
                cancel.cancel();
            }
        }
    }
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
fn description_source_label(scope: &Scope) -> String {
    match scope {
        Scope::Local => "Local changes".into(),
        Scope::PullRequest { url } => format!(
            "PR #{}",
            url.trim_end_matches('/').rsplit('/').next().unwrap_or("")
        ),
    }
}
fn description_source_text(label: String) -> gpui::Div {
    div()
        .debug_selector(|| "describe-source-context".into())
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
    pub(super) fn dismiss_description(&mut self) {
        self.descriptions.open = None;
    }

    pub(super) fn description_is_open(&self, cx: &App) -> bool {
        let targets = self.checkout_targets(cx);
        let selected = super::checkout_panel::selected_checkout(
            &targets,
            self.checkout_panel.chosen_root.as_deref(),
        );
        self.descriptions.open.as_ref().is_some_and(|key| {
            if key.owner == owner(&self.selection)
                && self.linked_description_key().as_ref() == Some(key)
            {
                return true;
            }
            key.owner == owner(&self.selection)
                && selected.as_ref().is_some_and(|(_, path)| path == &key.path)
        })
    }
    pub(super) fn close_description(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = self.descriptions.open.clone() {
            if let Some(entry) = self.descriptions.entries.get_mut(&key) {
                if entry.publishing {
                    return;
                }
                // A read-only PR lookup may finish, but leaving must not start a model run later.
                entry.resolving = false;
                if entry.preview.is_some() {
                    entry.preview = None;
                    if let Some(focus) = &self.descriptions.focus {
                        focus.focus(window, cx);
                    }
                    cx.notify();
                    return;
                }
                if entry.diff.is_some() {
                    entry.diff = None;
                    if let Some(focus) = &self.descriptions.focus {
                        focus.focus(window, cx);
                    }
                    cx.notify();
                    return;
                }
            }
        }
        self.descriptions.open = None;
        if let Some(source) = self.descriptions.return_focus.take() {
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
    pub(super) fn invoke_description_skill(
        &mut self,
        preference: ScopePreference,
        source: FocusHandle,
        cx: &mut Context<Self>,
    ) {
        let targets = self.checkout_targets(cx);
        let Some((_, path)) = super::checkout_panel::selected_checkout(
            &targets,
            self.checkout_panel.chosen_root.as_deref(),
        ) else {
            self.description_error("Select a checkout before describing changes.", cx);
            return;
        };
        // Native skill invocation replaces Files/Side chat without changing their drafts or requiring an already mounted Checkout card.
        self.files_active = false;
        self.checkout_panel.side_chat_owner = None;
        let selected_scope = self
            .descriptions
            .scopes
            .get(&(owner(&self.selection), path.clone()))
            .cloned();
        if let ScopePreference::PullRequestUrl(url) = &preference {
            // An explicit URL does not replace Checkout's local/attached-PR choice.
            self.start_description_conversation(
                path,
                Scope::PullRequest { url: url.clone() },
                Some(source),
                cx,
            );
            return;
        }
        if preference == ScopePreference::Local
            || (preference == ScopePreference::Current
                && !matches!(selected_scope, Some(Scope::PullRequest { .. })))
        {
            self.descriptions
                .scopes
                .insert((owner(&self.selection), path.clone()), Scope::Local);
            self.start_description_conversation(path, Scope::Local, Some(source), cx);
            return;
        }
        // Resolve the attached PR independently of the visibility and cached loading state of Checkout.
        let mut resolving_key = key(&self.selection, &path, &Scope::Local);
        resolving_key.scope = "resolving".into();
        self.descriptions.revision = self.descriptions.revision.wrapping_add(1);
        let run = self.descriptions.revision;
        let entry = self
            .descriptions
            .entries
            .entry(resolving_key.clone())
            .or_default();
        entry.run = run;
        entry.resolving = true;
        entry.error = None;
        self.reviews.open = None;
        self.hide_questions();
        self.descriptions.open = Some(resolving_key.clone());
        self.descriptions.return_focus = Some(source.clone());
        self.descriptions.focus_pending = true;
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
                let active = this.descriptions.open.as_ref() == Some(&resolving_key) && this.descriptions.entries.get(&resolving_key).is_some_and(|entry| entry.run == run && entry.resolving);
                if !active { return; }
                let targets = this.checkout_targets(cx);
                let still_selected = owner(&this.selection) == resolving_key.owner && super::checkout_panel::selected_checkout(&targets, this.checkout_panel.chosen_root.as_deref()).is_some_and(|(_, current)| current == path);
                let scope: Result<Scope, String> = match result {
                    Ok(Some(pr)) if still_selected => Ok(Scope::PullRequest { url: pr.url }),
                    Ok(None) if still_selected => Err("This checkout has no attached PR. Run /describe-changes local to describe local edits.".into()),
                    Ok(_) => Err("Checkout selection changed. Run the skill again in its intended checkout.".into()),
                    Err(error) => Err(error),
                };
                if let Some(entry) = this.descriptions.entries.get_mut(&resolving_key) { entry.resolving = false; if let Err(error) = &scope { entry.error = Some(error.clone()); } }
                if let Ok(scope) = scope {
                    this.descriptions.scopes.insert((resolving_key.owner.clone(), path.clone()), scope.clone());
                    this.start_description_conversation(path, scope, Some(source), cx);
                }
                cx.notify();
            });
        }).detach();
    }
    fn description_error(&self, error: impl Into<String>, cx: &mut Context<Self>) {
        self.runtime
            .read(cx)
            .toast
            .clone()
            .update(cx, |toast, cx| toast.set_error(error.into(), cx));
    }
    fn begin_description(
        &mut self,
        path: String,
        scope: Scope,
        source: Option<FocusHandle>,
        cx: &mut Context<Self>,
    ) {
        // Regeneration stays in the owning description conversation. Every new
        // command/Checkout request starts an independent session and links it to
        // the same canonical source file.
        let key = key(&self.selection, &path, &scope);
        if self
            .descriptions
            .entries
            .get(&key)
            .is_some_and(|entry| entry.session.is_some())
        {
            self.run_session_description(key, cx);
        } else {
            self.start_description_conversation(path, scope, source, cx);
        }
    }

    fn cancel_description(&mut self, key: &Key, cx: &mut Context<Self>) {
        let retired = self
            .descriptions
            .entries
            .get(key)
            .filter(|entry| !entry.finalizing)
            .and_then(|entry| {
                Some((
                    entry.session_key.clone()?,
                    entry.pending.as_ref()?.generation.clone(),
                ))
            });
        if let Some((session_key, generation)) = retired {
            self.retire_description_request(session_key, generation, cx);
        }
        if let Some(entry) = self.descriptions.entries.get_mut(key) {
            if entry.finalizing {
                return;
            }
            if let Some(cancel) = &entry.cancel {
                cancel.cancel();
            }
            if let Some(id) = entry.session {
                if let Some(session) = self.runtime.read(cx).registry.read(cx).codex(id).cloned() {
                    session.update(cx, |session, cx| {
                        if session.state.turn_id.is_some() {
                            session.send(grove_core::codex::Action::Interrupt, cx);
                        } else {
                            cx.notify();
                        }
                    });
                }
            }
            // Do not report cancellation until the child process has acknowledged exit.
        }
        cx.notify();
    }
    fn check_description(&mut self, key: &Key, cx: &mut Context<Self>) {
        let Some(entry) = self.descriptions.entries.get_mut(key) else {
            return;
        };
        if entry.cancel.is_some()
            || entry.publishing
            || entry.checking
            || entry
                .checked_at
                .is_some_and(|t| t.elapsed() < FRESHNESS_INTERVAL)
        {
            return;
        }
        let Some(description) = entry.result.clone() else {
            return;
        };
        entry.checking = true;
        let run = entry.run;
        let key = key.clone();
        cx.spawn(async move |this, cx| {
            let path = key.path.clone();
            let expected_result = description.clone();
            let result = cx
                .background_spawn(async move {
                    domain::is_current(std::path::Path::new(&path), &description)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(entry) = this.descriptions.entries.get_mut(&key).filter(|entry| {
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
    pub(super) fn render_description_controls(
        &mut self,
        path: &str,
        local_changes: bool,
        pr: Option<grove_core::checkout_pr::PullRequest>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.restore_session_description(cx);
        let menu_owner = (owner(&self.selection), path.to_owned());
        if self.descriptions.scope_menu.is_some()
            && (self.descriptions.scope_menu_owner.as_ref() != Some(&menu_owner)
                || !local_changes
                || pr.is_none()
                || self.linked_description_key().is_some())
        {
            self.descriptions.scope_menu = None;
            self.descriptions.scope_subscription = None;
            self.descriptions.scope_menu_owner = None;
        }
        if let Some(target) = self.linked_description_key() {
            self.check_description(&target, cx);
            let entry = self.descriptions.entries.get(&target);
            let busy = entry.is_some_and(|e| e.cancel.is_some());
            let stale = entry.is_some_and(|e| matches!(e.freshness, Some(Ok(false))));
            let unavailable =
                entry.is_some_and(|e| matches!(e.freshness, Some(Err(_))) || e.error.is_some());
            let saved = entry.is_some_and(|e| e.result.is_some());
            let source = entry
                .and_then(|e| e.scope.as_ref())
                .map(description_source_label);
            let source_focus = self
                .descriptions
                .open_source
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            let return_focus = source_focus.clone();
            return div()
                .id("describe-controls")
                .debug_selector(|| "describe-controls".into())
                .flex()
                .flex_col()
                .gap(rpx(SPACE_SM))
                .py(rpx(SPACE_LG))
                .child(
                    super::checkout_panel::checkout_loading_row(
                        "describe-open",
                        if saved {
                            "View description"
                        } else {
                            "Describe changes"
                        },
                        "diff",
                        false,
                        window,
                        cx,
                    )
                    .with_focus_handle(source_focus)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.reload_description_result(&target, cx);
                        this.descriptions.return_focus = Some(return_focus.clone());
                        this.reviews.open = None;
                        this.hide_questions();
                        this.descriptions.open = Some(target.clone());
                        this.descriptions.focus_pending = true;
                        cx.notify();
                    })),
                )
                .when_some(source, |column, source| {
                    column.child(description_source_text(source))
                })
                .when(busy || stale || unavailable, |column| {
                    column.child(div().px(rpx(SPACE_2XL)).child(status::label(
                        "describe-panel-state",
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
            .descriptions
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
        self.descriptions
            .scopes
            .insert(owner_key.clone(), scope.clone());
        let key = key(&self.selection, path, &scope);
        let entry = self.descriptions.entries.get(&key);
        let busy = entry.is_some_and(|e| e.cancel.is_some() || e.publishing);
        let exists = entry.is_some_and(|e| e.result.is_some());
        let mut column = div()
            .id("describe-controls")
            .debug_selector(|| "describe-controls".into())
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
                .descriptions
                .scope_focus
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            let keyboard_target = target.clone();
            let keyboard_selected = selected.clone();
            let keyboard_pr = pr_scope.clone();
            let keyboard_focus = focus.clone();
            let bounds = self.descriptions.scope_bounds.clone();
            column = column.child(
                div()
                    .mx(rpx(SPACE_2XL))
                    .on_prepaint(move |value, _, _| bounds.set(value))
                    .child(
                        action(
                            "describe-source",
                            description_source_label(&scope),
                            "chev-down",
                            window,
                            cx,
                        )
                        .justify_start()
                        .with_focus_handle(focus)
                        .selected(
                            self.descriptions.scope_menu.is_some()
                                && self.descriptions.scope_menu_owner.as_ref() == Some(&owner_key),
                        )
                        .disabled(busy)
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                let position = this.descriptions.scope_bounds.get().bottom_left();
                                this.open_description_sources(
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
                                let position = this.descriptions.scope_bounds.get().bottom_left();
                                this.open_description_sources(
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
            if self.descriptions.scope_menu_owner.as_ref() == Some(&owner_key) && !busy {
                if let Some(menu) = self.descriptions.scope_menu.clone() {
                    column = column.child(gpui::deferred(
                        gpui::anchored()
                            .position_mode(gpui::AnchoredPositionMode::Window)
                            .position(self.descriptions.scope_position)
                            .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
                            .child(menu),
                    ));
                }
            } else {
                self.descriptions.scope_menu = None;
                self.descriptions.scope_subscription = None;
                self.descriptions.scope_menu_owner = None;
            }
        } else if exists || matches!(scope, Scope::PullRequest { .. }) {
            column = column.child(description_source_text(description_source_label(&scope)));
        }
        let focus = self
            .descriptions
            .source
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let target = path.to_owned();
        let chosen = scope.clone();
        if !exists {
            column = column.child(
                super::checkout_panel::checkout_loading_row(
                    "describe-changes",
                    if entry.is_some_and(|e| e.publishing) {
                        "Updating description…"
                    } else if busy {
                        "Describing changes…"
                    } else {
                        "Describe changes"
                    },
                    "diff",
                    busy,
                    window,
                    cx,
                )
                .with_focus_handle(focus)
                .disabled(busy || (matches!(scope, Scope::Local) && !local_changes))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.begin_description(
                        target.clone(),
                        chosen.clone(),
                        this.descriptions.source.clone(),
                        cx,
                    );
                })),
            );
        }
        if exists {
            let open_focus = self
                .descriptions
                .open_source
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            let return_focus = open_focus.clone();
            let target = key.clone();
            column = column.child(
                super::checkout_panel::checkout_loading_row(
                    "describe-open",
                    "View description",
                    "diff",
                    false,
                    window,
                    cx,
                )
                .with_focus_handle(open_focus)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.descriptions.return_focus = Some(return_focus.clone());
                    this.reviews.open = None;
                    this.hide_questions();
                    this.descriptions.open = Some(target.clone());
                    this.descriptions.focus_pending = true;
                    cx.notify();
                })),
            );
        }
        if exists && entry.is_some_and(|e| matches!(e.freshness, Some(Ok(false)))) {
            column = column.child(div().px(rpx(SPACE_2XL)).child(status::label(
                "describe-panel-state",
                StatusKind::Warning,
                "Out of date",
                ICON_SM,
                cx,
            )));
        }
        if busy {
            let target = key.clone();
            column = column.child(
                action("describe-cancel", "Cancel generation", "close", window, cx)
                    .mx(rpx(SPACE_2XL))
                    .disabled(entry.is_some_and(|e| e.publishing))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.cancel_description(&target, cx)),
                    ),
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
    fn open_description_sources(
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
        let Some(focus) = self.descriptions.scope_focus.clone() else {
            return;
        };
        let menu = PopupMenu::build(window, cx, move |menu, window, _| {
            let mut menu =
                crate::views::components::dropdown_menu(menu, window).action_context(focus);
            for candidate in [Scope::Local, pr_scope.clone()] {
                let owner = owner.clone();
                let target = target.clone();
                menu = menu.item(
                    PopupMenuItem::new(description_source_label(&candidate))
                        .icon(gpui_component::Icon::default().path(match candidate {
                            Scope::Local => "icons/diff.svg",
                            Scope::PullRequest { .. } => "icons/git-pull-request.svg",
                        }))
                        .checked(candidate == selected)
                        .on_click(move |_, _, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                this.descriptions
                                    .scopes
                                    .insert(target.clone(), candidate.clone());
                                cx.notify();
                            });
                        }),
                );
            }
            menu
        });
        self.descriptions.scope_subscription = Some(cx.subscribe_in(
            &menu,
            window,
            |this, _, _: &gpui::DismissEvent, window, cx| {
                this.descriptions.scope_menu = None;
                this.descriptions.scope_subscription = None;
                this.descriptions.scope_menu_owner = None;
                if let Some(focus) = &this.descriptions.scope_focus {
                    focus.focus(window, cx);
                }
                cx.notify();
            },
        ));
        self.descriptions.scope_menu_owner = Some(source);
        self.descriptions.scope_position = position;
        menu.focus_handle(cx).focus(window, cx);
        self.descriptions.scope_menu = Some(menu);
        cx.notify();
    }
    fn open_description_preview(&mut self, key: &Key, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.descriptions.entries.get_mut(key) else {
            return;
        };
        if entry
            .freshness
            .as_ref()
            .is_none_or(|f| !matches!(f, Ok(true)))
            || entry.cancel.is_some()
            || entry.publishing
        {
            return;
        }
        let Some(description) = &entry.result else {
            return;
        };
        if std::path::Path::new(&description.snapshot.checkout) != std::path::Path::new(&key.path) {
            return;
        }
        let body = description.pr_body();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .auto_grow(8, 20)
        });
        input.update(cx, |input, cx| input.set_value(body, window, cx));
        input.focus_handle(cx).focus(window, cx);
        entry.preview = Some(input);
        cx.notify();
    }
    fn publish_description(&mut self, key: &Key, cx: &mut Context<Self>) {
        let store = self.description_store();
        let Some(entry) = self.descriptions.entries.get_mut(key) else {
            return;
        };
        if entry.publishing || entry.cancel.is_some() {
            return;
        }
        let (Some(description), Some(input)) = (entry.result.clone(), entry.preview.clone()) else {
            return;
        };
        let body = input.read(cx).value().to_string();
        if body.trim().is_empty() {
            entry.error = Some("Enter a PR description before updating.".into());
            cx.notify();
            return;
        }
        let published_body = body.clone();
        entry.publishing = true;
        entry.error = None;
        let run = entry.run;
        let key = key.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let path = key.path.clone();
            let result = cx
                .background_spawn(async move {
                    store?.publish_current(std::path::Path::new(&path), &description, &body)?;
                    Ok::<_, String>(())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(entry) = this
                    .descriptions
                    .entries
                    .get_mut(&key)
                    .filter(|e| e.run == run)
                {
                    entry.publishing = false;
                    match result {
                        Ok(()) => {
                            entry.preview = None;
                            if let Some(description) = &mut entry.result {
                                Arc::make_mut(description).snapshot.original_body =
                                    Some(published_body.clone());
                            }
                            entry.freshness = None;
                            entry.checking = false;
                            entry.receipt = Some("PR description updated".into());
                            entry.checked_at = None;
                        }
                        Err(error) => {
                            entry.error = Some(error);
                            entry.checked_at = None;
                        }
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }
    pub(super) fn render_description(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(key) = self.descriptions.open.clone() else {
            return div().into_any_element();
        };
        super::super::mermaid::set_context(Some(cx.entity_id()), window, cx);
        self.check_description(&key, cx);
        let focus = self
            .descriptions
            .focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        if self.descriptions.focus_pending {
            focus.focus(window, cx);
            self.descriptions.focus_pending = false;
        }
        if let Some(entry) = self.descriptions.entries.get_mut(&key) {
            if let Some(description) = &entry.result {
                for file in &description.files {
                    entry
                        .disclosure_focus
                        .entry(file.path.clone())
                        .or_insert_with(|| cx.focus_handle());
                }
            }
        }
        let Some(entry) = self.descriptions.entries.get(&key) else {
            return div().into_any_element();
        };
        let busy = entry.cancel.is_some();
        let publishing = entry.publishing;
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
                "describe-back",
                if entry.preview.is_some() || entry.diff.is_some() {
                    "Back to description"
                } else {
                    "Conversation"
                },
                "arrow-left",
                window,
                cx,
            )
            .disabled(publishing)
            .on_click(cx.listener(|this, _, window, cx| this.close_description(window, cx))),
        );
        let label = entry
            .result
            .as_ref()
            .map_or("Describe changes".into(), |d| d.snapshot.label.clone());
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
            .id("description-reader-body")
            .flex()
            .flex_col()
            .gap(rpx(SPACE_LG))
            .w_full()
            .min_w_0()
            .max_w(rpx(DESCRIPTION_WIDTH));
        if entry.resolving {
            body = body.child(status::label(
                "description-resolving",
                StatusKind::Working,
                "Finding the attached pull request…",
                ICON_SM,
                cx,
            ));
        }
        if busy {
            body = body.child(status::label(
                "description-generating",
                StatusKind::Working,
                if entry.finalizing {
                    "Saving description…"
                } else {
                    "Reading the diff and describing changes…"
                },
                ICON_SM,
                cx,
            ));
            let target = key.clone();
            body = body.child(
                action(
                    "description-cancel",
                    "Cancel generation",
                    "close",
                    window,
                    cx,
                )
                .disabled(entry.finalizing)
                .on_click(cx.listener(move |this, _, _, cx| this.cancel_description(&target, cx))),
            );
        }
        if let Some(error) = &entry.error {
            body = body.child(status::label(
                "description-error",
                if error.to_lowercase().contains("cancel") {
                    StatusKind::Canceled
                } else {
                    StatusKind::Failed
                },
                error.clone(),
                ICON_SM,
                cx,
            ));
            if !busy && !publishing && entry.preview.is_none() && entry.scope.is_some() {
                let path = key.path.clone();
                let scope = entry.scope.clone().unwrap_or(Scope::Local);
                body = body.child(
                    action("description-retry", "Retry", "refresh", window, cx).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.begin_description(path.clone(), scope.clone(), None, cx);
                        }),
                    ),
                );
            }
        }
        if let Some(input) = &entry.preview {
            let description = entry.result.as_ref();
            body = body
                .child(
                    div()
                        .text_size(rpx(TEXT_TITLE))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Update PR description"),
                )
                .child(
                    div()
                        .text_color(c::FG_DIM())
                        .child("Review the replacement below. The PR title stays unchanged."),
                );
            if let Some(original) = description.and_then(|d| d.snapshot.original_body.clone()) {
                body = body
                    .child(
                        div()
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child("Current description"),
                    )
                    .child(
                        div()
                            .p(rpx(SPACE_LG))
                            .bg(c::FIELD_FILL())
                            .rounded(rpx(RADIUS_GROUP))
                            .child(
                                TextView::markdown("description-original-context", original)
                                    .style(super::super::codex_view::markdown_style(window))
                                    .selectable(true),
                            ),
                    );
            }
            body = body.child(
                project_field_well(false, false)
                    .debug_selector(|| "description-preview-well".into())
                    .h_auto()
                    .justify_start()
                    .py(rpx(SPACE_LG))
                    .overflow_hidden()
                    .child(
                        div()
                            .min_w_0()
                            .w_full()
                            .debug_selector(|| "description-preview-input".into())
                            .child(
                                Input::new(input)
                                    .aria_label("New PR description")
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .p_0()
                                    .disabled(publishing),
                            ),
                    ),
            );
            let confirm = key.clone();
            body =
                body.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(rpx(SPACE_LG))
                        .child(
                            action("description-preview-cancel", "Cancel", "close", window, cx)
                                .disabled(publishing)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_description(window, cx);
                                })),
                        )
                        .child(
                            form_action(
                                "description-publish-confirm",
                                if publishing {
                                    "Updating description…"
                                } else {
                                    "Update description"
                                },
                                false,
                                window,
                                cx,
                            )
                            .debug_selector(|| "description-publish-confirm".into())
                            .icon(gpui_component::Icon::default().path(if publishing {
                                "icons/status-working.svg"
                            } else {
                                "icons/check.svg"
                            }))
                            .disabled(publishing)
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.publish_description(&confirm, cx),
                            )),
                        ),
                );
        } else if let Some((path, view)) = &entry.diff {
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
                        .child("Read-only diff from the described snapshot"),
                )
                .child(text_view(view, window));
        } else if let Some(description) = &entry.result {
            let omitted = description
                .snapshot
                .files
                .iter()
                .filter(|file| {
                    file.omitted.is_some()
                        || !description
                            .files
                            .iter()
                            .any(|described| described.path == file.path)
                })
                .count();
            body = body.child(
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(format!(
                        "{} files · snapshot {}",
                        description.snapshot.files.len(),
                        description
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
                        "description-stale",
                        StatusKind::Warning,
                        if matches!(description.snapshot.scope, Scope::Local) {
                            "Changes updated since this description. Regenerate to refresh."
                        } else {
                            "Changes updated since this description. Regenerate before publishing."
                        },
                        ICON_SM,
                        cx,
                    ));
                }
                Some(Err(error)) => {
                    body = body.child(status::label(
                        "description-freshness-error",
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
            if !description.content.is_empty() {
                body = body.child(
                    TextView::markdown("description-custom-content", description.content.clone())
                        .markdown_extensions(super::super::mermaid::extensions())
                        .style(super::super::codex_view::markdown_style(window))
                        .selectable(true),
                );
            }
            body = body.child(
                div()
                    .mt(rpx(SPACE_LG))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("File walkthrough"),
            );
            let mut previous = String::new();
            let mut ordered_files = description.files.iter().collect::<Vec<_>>();
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
                let snapshot_file = description
                    .snapshot
                    .files
                    .iter()
                    .find(|f| f.path == file.path);
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(rpx(SPACE_LG))
                        .child(
                            crate::views::components::inline_disclosure(
                                SharedString::from(format!("describe-file-{}", file.path)),
                                file.path.clone(),
                            )
                            .debug_selector({
                                let path = file.path.clone();
                                move || format!("describe-file-{path}")
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
                                    if let Some(entry) = this.descriptions.entries.get_mut(&target)
                                    {
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
                                        if let Some(entry) =
                                            this.descriptions.entries.get_mut(&target)
                                        {
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
                                format!("describe-diff-{}", file.path),
                                "Open diff",
                                "diff",
                                window,
                                cx,
                            )
                            .disabled(patch.is_empty())
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(entry) = this.descriptions.entries.get_mut(&target)
                                    {
                                        let fence = "`".repeat(
                                            patch
                                                .lines()
                                                .map(|line| {
                                                    line.chars().take_while(|c| *c == '`').count()
                                                })
                                                .max()
                                                .unwrap_or(0)
                                                .max(2)
                                                + 1,
                                        );
                                        entry.diff = Some((
                                            path.clone(),
                                            cx.new(|cx| {
                                                TextViewState::markdown(
                                                    &format!("{fence}diff\n{patch}\n{fence}"),
                                                    cx,
                                                )
                                            }),
                                        ));
                                    }
                                    this.descriptions.focus_pending = true;
                                    cx.notify();
                                },
                            )),
                        );
                    }
                }
            }
            if let Some(diagram) = &entry.diagram {
                body = body.child(div().mt(rpx(SPACE_LG)).child(text_view(diagram, window)));
            }
            body = body.child(div().mt(rpx(SPACE_LG)).text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child(format!("{} of {} files described · {omitted} files omitted or partial. Based on the diff; tests were not executed by this description.", description.files.len(), description.snapshot.files.len())));
            for omitted_file in description.snapshot.files.iter().filter(|file| {
                !description
                    .files
                    .iter()
                    .any(|described| described.path == file.path)
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
                                .unwrap_or("Not returned by the description model")
                        )),
                );
            }
            let markdown = description.markdown();
            let target = key.clone();
            let path = key.path.clone();
            let scope = entry.scope.clone().unwrap_or(Scope::Local);
            let mut actions = div()
                .flex()
                .flex_wrap()
                .gap(rpx(SPACE_LG))
                .child(
                    action("describe-copy", "Copy Markdown", "copy", window, cx).on_click(
                        cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                markdown.clone(),
                            ));
                            if let Some(entry) = this.descriptions.entries.get_mut(&target) {
                                entry.receipt = Some("Markdown copied".into());
                            }
                            cx.notify();
                        }),
                    ),
                )
                .child(
                    action("describe-regenerate", "Regenerate", "refresh", window, cx)
                        .disabled(busy || publishing)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.begin_description(path.clone(), scope.clone(), None, cx);
                        })),
                );
            if matches!(description.snapshot.scope, Scope::PullRequest { .. }) {
                let target = key.clone();
                actions = actions.child(
                    action(
                        "describe-publish",
                        "Update PR description",
                        "git-pull-request",
                        window,
                        cx,
                    )
                    .disabled(
                        busy || publishing
                            || !matches!(entry.freshness, Some(Ok(true)))
                            || description.snapshot.checkout != key.path,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_description_preview(&target, window, cx);
                    })),
                );
            }
            body = body.child(actions);
            if let Some(receipt) = &entry.receipt {
                body = body.child(status::label(
                    "description-receipt",
                    StatusKind::Success,
                    receipt.clone(),
                    ICON_SM,
                    cx,
                ));
            }
        }
        let reader = div()
            .id("description-reader")
            .debug_selector(|| "description-reader".into())
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .track_focus(&focus)
            .child(header)
            .child(
                div()
                    .id("description-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(if entry.preview.is_some() {
                        &entry.preview_scroll
                    } else if entry.diff.is_some() {
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
        motion::fast(reader, "description-reader-enter", cx)
    }
}

#[cfg(test)]
#[path = "change_description_tests.rs"]
mod tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "change_description_live_tests.rs"]
mod live_tests;
