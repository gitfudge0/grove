//! Native, checkout/session-owned questions. Each submission captures a new immutable diff.
use super::*;
use crate::views::change_session::{ChangeSession, Phase as SessionPhase};
use crate::views::{
    components::{project_field_well, transcript_action},
    status::{self, StatusKind},
};
use gpui_component::{
    input::{Input, InputState},
    menu::{PopupMenu, PopupMenuItem},
    text::{TextView, TextViewState},
    Disableable, Selectable,
};
use grove_core::{
    ask_skill::Invocation,
    change_description::{self as snapshot, Scope},
    change_questions::{self as domain, Answer},
    describe_skill::ScopePreference,
};
use std::sync::Arc;
#[path = "change_questions_session.rs"]
mod session;
const READER_WIDTH: f32 = 700.;
const READER_TOOLBAR_INSET: f32 = CHROME_CONTROL_H * 3. + SPACE_LG * 2. + SPACE_20 * 2.;
const FRESHNESS_INTERVAL: Duration = Duration::from_secs(5);
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) struct Key {
    owner: String,
    pub(super) path: String,
    scope: String,
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
#[derive(Default)]
pub(super) struct Questions {
    entries: HashMap<Key, Entry>,
    open: Option<Key>,
    scopes: HashMap<(String, String), Scope>,
    revision: u64,
    focus: Option<FocusHandle>,
    source: Option<FocusHandle>,
    focus_pending: bool,
    control_focus: HashMap<(String, String), FocusHandle>,
    scope_focus: Option<FocusHandle>,
    scope_menu: Option<Entity<PopupMenu>>,
    scope_subscription: Option<gpui::Subscription>,
    scope_menu_owner: Option<(String, String)>,
    scope_position: gpui::Point<gpui::Pixels>,
    scope_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<gpui::Pixels>>>,
    linked: HashMap<SessionId, Key>,
    hydrated: HashSet<(SessionId, String)>,
    identities: HashMap<SessionId, String>,
    retired_entries: HashMap<(String, String, String), Entry>,
    #[cfg(test)]
    store_root: Option<std::path::PathBuf>,
}
#[derive(Default)]
struct Entry {
    pending_question: Option<String>,
    scope: Option<Scope>,
    input: Option<Entity<InputState>>,
    result: Option<Arc<Answer>>,
    text: Option<Entity<TextViewState>>,
    diff: Option<(String, Entity<TextViewState>)>,
    controller: Option<Entity<ChangeSession<Answer>>>,
    subscription: Option<gpui::Subscription>,
    session: Option<SessionId>,
    session_key: Option<String>,
    phase: SessionPhase,
    run: u64,
    error: Option<String>,
    resolving: bool,
    freshness: Option<Result<bool, String>>,
    checked_at: Option<Instant>,
    checking: bool,
    scroll: ScrollHandle,
    diff_scroll: ScrollHandle,
    receipt: Option<String>,
    receipt_started: Option<Instant>,
}
fn action(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    glyph: &str,
    window: &Window,
    cx: &App,
) -> gpui_component::button::Button {
    let id: SharedString = id.into();
    let selector = id.to_string();
    transcript_action(id, label, false, window, cx)
        .debug_selector(move || selector.clone())
        .icon(
            gpui_component::Icon::default()
                .path(SharedString::from(format!("icons/{glyph}.svg")))
                .size(rpx(ICON_SM)),
        )
}
fn question_source_label(scope: &Scope) -> String {
    match scope {
        Scope::Local => "Local changes".into(),
        Scope::PullRequest { url } => format!(
            "PR #{}",
            url.trim_end_matches('/').rsplit('/').next().unwrap_or("")
        ),
    }
}
fn question_source_text(label: String) -> gpui::Div {
    div()
        .debug_selector(|| "ask-source-context".into())
        .pl(rpx(SPACE_2XL + ICON_LG + SPACE_SM))
        .pr(rpx(SPACE_2XL))
        .text_size(rpx(TEXT_MICRO))
        .text_color(c::FG_DIM())
        .child(label)
}
fn text_view(text: &Entity<TextViewState>, window: &Window) -> TextView {
    TextView::new(text)
        .markdown_extensions(super::super::mermaid::extensions())
        .style(super::super::codex_view::markdown_style(window))
        .selectable(true)
}
fn patch_markdown(patch: &str) -> String {
    let n = patch
        .lines()
        .map(|l| l.trim_start().chars().take_while(|c| *c == '`').count())
        .max()
        .unwrap_or(0)
        .max(2)
        + 1;
    let f = "`".repeat(n);
    format!("{f}diff\n{patch}\n{f}")
}
impl Sidebar {
    pub(super) fn hide_questions(&mut self) {
        self.questions.open = None;
    }
    pub(super) fn questions_is_open(&self, cx: &App) -> bool {
        let targets = self.checkout_targets(cx);
        let selected = super::checkout_panel::selected_checkout(
            &targets,
            self.checkout_panel.chosen_root.as_deref(),
        );
        self.questions.open.as_ref().is_some_and(|k| {
            k.owner == owner(&self.selection)
                && (self.linked_question_key().as_ref() == Some(k)
                    || selected.is_some_and(|(_, p)| p == k.path))
        })
    }
    pub(super) fn close_questions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(e) = self
            .questions
            .open
            .as_ref()
            .and_then(|k| self.questions.entries.get_mut(k))
        {
            e.resolving = false;
            if e.diff.take().is_some() {
                if let Some(f) = &self.questions.focus {
                    f.focus(window, cx);
                }
                cx.notify();
                return;
            }
        }
        self.questions.open = None;
        if let Some(f) = self.questions.source.take() {
            f.focus(window, cx);
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
    fn question_error(&self, error: impl Into<String>, cx: &mut Context<Self>) {
        self.runtime
            .read(cx)
            .toast
            .clone()
            .update(cx, |t, cx| t.set_error(error.into(), cx));
    }
    pub(super) fn invoke_ask_skill(
        &mut self,
        invocation: Invocation,
        source: FocusHandle,
        cx: &mut Context<Self>,
    ) {
        // A retained answer's visible source is authoritative for an unqualified
        // command, including explicit PRs no longer attached to this checkout.
        if invocation.scope == ScopePreference::Current {
            if let Some(target) = self.linked_question_key() {
                if let Some(scope) = self
                    .questions
                    .entries
                    .get(&target)
                    .and_then(|entry| entry.scope.clone())
                {
                    self.open_question(target.path, scope, invocation.question, Some(source), cx);
                    return;
                }
            }
        }
        let targets = self.checkout_targets(cx);
        let Some((_, path)) = super::checkout_panel::selected_checkout(
            &targets,
            self.checkout_panel.chosen_root.as_deref(),
        ) else {
            self.question_error("Select a checkout before asking about changes.", cx);
            return;
        };
        self.files_active = false;
        self.checkout_panel.side_chat_owner = None;
        self.reviews.open = None;
        self.dismiss_description();
        if let ScopePreference::PullRequestUrl(url) = &invocation.scope {
            let preference_key = (owner(&self.selection), path.clone());
            let previous = self.questions.scopes.get(&preference_key).cloned();
            self.open_question(
                path,
                Scope::PullRequest { url: url.clone() },
                invocation.question,
                Some(source),
                cx,
            );
            if let Some(previous) = previous {
                self.questions.scopes.insert(preference_key, previous);
            } else {
                self.questions.scopes.remove(&preference_key);
            }
            return;
        }
        let scope = self
            .questions
            .scopes
            .get(&(owner(&self.selection), path.clone()))
            .cloned()
            .unwrap_or(Scope::Local);
        if invocation.scope == ScopePreference::Local
            || (invocation.scope == ScopePreference::Current && matches!(scope, Scope::Local))
        {
            self.open_question(path, Scope::Local, invocation.question, Some(source), cx);
            return;
        }
        let mut k = key(&self.selection, &path, &Scope::Local);
        k.scope = "resolving".into();
        self.questions.revision = self.questions.revision.wrapping_add(1);
        let run = self.questions.revision;
        let e = self.questions.entries.entry(k.clone()).or_default();
        e.run = run;
        e.resolving = true;
        e.pending_question = Some(invocation.question.clone());
        e.error = None;
        self.questions.open = Some(k.clone());
        self.questions.source = Some(source.clone());
        self.questions.focus_pending = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let result: Result<Option<grove_core::checkout_pr::PullRequest>, String> = cx
                .background_spawn(async move {
                    let state = grove_core::checkout_git::inspect(std::path::Path::new(&target))?;
                    let pr =
                        grove_core::checkout_pr::inspect(std::path::Path::new(&target), &state)?;
                    if grove_core::checkout_git::inspect(std::path::Path::new(&target))?
                        .target_identity()
                        != state.target_identity()
                    {
                        return Err(
                            "Checkout changed while resolving its PR. Run /ask again.".into()
                        );
                    }
                    Ok(pr)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.questions.open.as_ref() != Some(&k)
                    || !this
                        .questions
                        .entries
                        .get(&k)
                        .is_some_and(|e| e.run == run && e.resolving)
                {
                    return;
                }
                let targets = this.checkout_targets(cx);
                let selected = owner(&this.selection) == k.owner
                    && super::checkout_panel::selected_checkout(
                        &targets,
                        this.checkout_panel.chosen_root.as_deref(),
                    )
                    .is_some_and(|(_, p)| p == path);
                let scope = match result {
                    Ok(Some(pr)) if selected => Ok(Scope::PullRequest { url: pr.url }),
                    Ok(None) if selected => {
                        Err("No attached PR. Use /ask local to ask about local edits.".into())
                    }
                    Ok(_) => Err(
                        "Checkout selection changed. Invoke Ask in the intended checkout.".into(),
                    ),
                    Err(e) => Err(e),
                };
                if let Some(e) = this.questions.entries.get_mut(&k) {
                    e.resolving = false;
                    if let Err(error) = &scope {
                        e.error = Some(error.clone());
                    }
                }
                if let Ok(scope) = scope {
                    this.open_question(path, scope, invocation.question, Some(source), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn open_question(
        &mut self,
        path: String,
        scope: Scope,
        question: String,
        source: Option<FocusHandle>,
        cx: &mut Context<Self>,
    ) {
        if !question.trim().is_empty() {
            self.start_question_conversation(path, scope, question, cx);
            return;
        }
        let k = key(&self.selection, &path, &scope);
        let has_question = !question.trim().is_empty();
        self.questions
            .scopes
            .insert((owner(&self.selection), path), scope.clone());
        let e = self.questions.entries.entry(k.clone()).or_default();
        e.scope = Some(scope);
        e.resolving = false;
        // Keep an edited native question if a second invocation arrives while work is running.
        if has_question && !e.phase.busy() {
            e.pending_question = Some(question);
        }
        if let Some(s) = source {
            self.questions.source = Some(s);
        }
        self.questions.open = Some(k);
        self.questions.focus_pending = true;
        self.files_active = false;
        self.reviews.open = None;
        self.dismiss_description();
        cx.notify();
    }
    fn submit_question(&mut self, k: &Key, cx: &mut Context<Self>) {
        if !self.questions_is_open(cx) || self.questions.open.as_ref() != Some(k) {
            return;
        }
        let Some(e) = self.questions.entries.get(k) else {
            return;
        };
        if e.phase.busy() || e.resolving {
            return;
        }
        let Some(input) = &e.input else {
            return;
        };
        let question = input.read(cx).value().to_string();
        if question.trim().is_empty() || question.len() > 4000 {
            if let Some(e) = self.questions.entries.get_mut(k) {
                e.error = Some("Enter a question of at most 4,000 bytes.".into());
            }
            cx.notify();
            return;
        }
        let scope = e.scope.clone().unwrap_or(Scope::Local);
        if let Some(controller) = e.controller.clone() {
            controller.update(cx, |controller, cx| {
                controller.start(k.path.clone(), scope, question, cx);
            });
        } else {
            self.start_question_conversation(k.path.clone(), scope, question, cx);
        }
    }
    fn expire_question_receipt(
        &self,
        key: Key,
        run: u64,
        receipt: Instant,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(entry) = this
                    .questions
                    .entries
                    .get_mut(&key)
                    .filter(|e| e.run == run && e.receipt_started == Some(receipt))
                {
                    entry.receipt = None;
                    entry.receipt_started = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }
    fn check_question(&mut self, k: &Key, cx: &mut Context<Self>) {
        let Some(e) = self.questions.entries.get_mut(k) else {
            return;
        };
        if e.phase.busy()
            || e.checking
            || e.checked_at
                .is_some_and(|t| t.elapsed() < FRESHNESS_INTERVAL)
        {
            return;
        }
        let Some(answer) = e.result.clone() else {
            return;
        };
        e.checking = true;
        let run = e.run;
        let k = k.clone();
        cx.spawn(async move |this, cx| {
            let path = k.path.clone();
            let expected = answer.clone();
            let result = cx
                .background_spawn(async move {
                    snapshot::snapshot_is_current(std::path::Path::new(&path), &answer.snapshot)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(e) = this.questions.entries.get_mut(&k).filter(|e| {
                    e.run == run && e.result.as_ref().is_some_and(|r| Arc::ptr_eq(r, &expected))
                }) {
                    e.checking = false;
                    e.checked_at = Some(Instant::now());
                    e.freshness = Some(result);
                    cx.notify();
                }
            });
            cx.background_executor().timer(FRESHNESS_INTERVAL).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }
    pub(super) fn render_question_controls(
        &mut self,
        path: &str,
        local: bool,
        pr: Option<grove_core::checkout_pr::PullRequest>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let menu_owner = (owner(&self.selection), path.to_string());
        if self.questions.scope_menu.is_some()
            && (self.questions.scope_menu_owner.as_ref() != Some(&menu_owner)
                || !local
                || pr.is_none()
                || self.linked_question_key().is_some())
        {
            self.questions.scope_menu = None;
            self.questions.scope_subscription = None;
            self.questions.scope_menu_owner = None;
        }
        if let Some(target) = self.linked_question_key() {
            self.check_question(&target, cx);
            let entry = self.questions.entries.get(&target);
            let saved = entry.is_some_and(|e| e.result.is_some());
            let busy = entry.is_some_and(|e| e.phase.busy());
            let stale = entry.is_some_and(|e| matches!(e.freshness, Some(Ok(false))));
            let source = entry
                .and_then(|e| e.scope.as_ref())
                .map(question_source_label);
            let focus = self
                .questions
                .control_focus
                .entry(menu_owner)
                .or_insert_with(|| cx.focus_handle())
                .clone();
            let source_focus = focus.clone();
            return div()
                .id("ask-controls")
                .debug_selector(|| "ask-controls".into())
                .flex()
                .flex_col()
                .gap(rpx(SPACE_SM))
                .py(rpx(SPACE_LG))
                .child(
                    super::checkout_panel::checkout_loading_row(
                        "ask-open",
                        if saved {
                            "View answer"
                        } else {
                            "Ask about changes"
                        },
                        "message-circle",
                        false,
                        window,
                        cx,
                    )
                    .with_focus_handle(focus)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.reload_question(&target, cx);
                        this.questions.open = Some(target.clone());
                        this.questions.source = Some(source_focus.clone());
                        this.questions.focus_pending = true;
                        this.reviews.open = None;
                        this.dismiss_description();
                        cx.notify();
                    })),
                )
                .when_some(source, |column, label| {
                    column.child(question_source_text(label))
                })
                .when(busy || stale, |column| {
                    column.child(div().px(rpx(SPACE_2XL)).child(status::label(
                        "ask-panel-state",
                        if busy {
                            StatusKind::Working
                        } else {
                            StatusKind::Warning
                        },
                        if busy { "Generating…" } else { "Out of date" },
                        ICON_SM,
                        cx,
                    )))
                })
                .into_any_element();
        }
        if !local && pr.is_none() {
            return div().into_any_element();
        }
        let preferred = self
            .questions
            .scopes
            .get(&menu_owner)
            .cloned()
            .unwrap_or_else(|| {
                if local {
                    Scope::Local
                } else {
                    pr.as_ref().map_or(Scope::Local, |pr| Scope::PullRequest {
                        url: pr.url.clone(),
                    })
                }
            });
        let scope = match (&preferred, &pr) {
            (_, Some(pr)) if !local => Scope::PullRequest {
                url: pr.url.clone(),
            },
            (Scope::PullRequest { .. }, Some(pr)) => Scope::PullRequest {
                url: pr.url.clone(),
            },
            (Scope::PullRequest { .. }, None) => Scope::Local,
            _ => preferred,
        };
        self.questions
            .scopes
            .insert(menu_owner.clone(), scope.clone());
        let mut column = div()
            .id("ask-controls")
            .debug_selector(|| "ask-controls".into())
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM))
            .py(rpx(SPACE_LG));
        if let Some(pr) = pr.filter(|_| local) {
            let focus = self
                .questions
                .scope_focus
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            let selected = scope.clone();
            let target = menu_owner.clone();
            let pr_scope = Scope::PullRequest { url: pr.url };
            let keyboard_target = target.clone();
            let keyboard_selected = selected.clone();
            let keyboard_pr = pr_scope.clone();
            let keyboard_focus = focus.clone();
            let bounds = self.questions.scope_bounds.clone();
            column = column.child(
                div()
                    .mx(rpx(SPACE_2XL))
                    .on_prepaint(move |value, _, _| bounds.set(value))
                    .child(
                        action(
                            "ask-source",
                            question_source_label(&scope),
                            "chev-down",
                            window,
                            cx,
                        )
                        .justify_start()
                        .with_focus_handle(focus)
                        .selected(self.questions.scope_menu.is_some())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                let position = this.questions.scope_bounds.get().bottom_left();
                                this.open_question_sources(
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
                            if keyboard_focus.is_focused(window)
                                && matches!(event.keystroke.key.as_str(), "enter" | "space")
                            {
                                let position = this.questions.scope_bounds.get().bottom_left();
                                this.open_question_sources(
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
            if self.questions.scope_menu_owner.as_ref() == Some(&menu_owner) {
                if let Some(menu) = self.questions.scope_menu.clone() {
                    column = column.child(gpui::deferred(
                        gpui::anchored()
                            .position_mode(gpui::AnchoredPositionMode::Window)
                            .position(self.questions.scope_position)
                            .snap_to_window_with_margin(rpx(SPACE_LG).to_pixels(window.rem_size()))
                            .child(menu),
                    ));
                }
            }
        } else if matches!(scope, Scope::PullRequest { .. }) {
            column = column.child(question_source_text(question_source_label(&scope)));
        }
        let target = path.to_string();
        let focus = self
            .questions
            .control_focus
            .entry(menu_owner)
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let source_focus = focus.clone();
        column = column.child(
            super::checkout_panel::checkout_loading_row(
                "ask-open",
                "Ask about changes",
                "message-circle",
                false,
                window,
                cx,
            )
            .with_focus_handle(focus)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_question(
                    target.clone(),
                    scope.clone(),
                    String::new(),
                    Some(source_focus.clone()),
                    cx,
                );
            })),
        );
        if local {
            column = column.child(question_source_text(
                "Uncommitted changes, including new files.".into(),
            ));
        }
        column.into_any_element()
    }
    fn open_question_sources(
        &mut self,
        target: (String, String),
        selected: Scope,
        pr_scope: Scope,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = cx.entity().downgrade();
        let menu_owner = target.clone();
        let Some(focus) = self.questions.scope_focus.clone() else {
            return;
        };
        let menu = PopupMenu::build(window, cx, move |menu, window, _| {
            let mut menu =
                crate::views::components::dropdown_menu(menu, window).action_context(focus);
            for candidate in [Scope::Local, pr_scope.clone()] {
                let owner = owner.clone();
                let target = target.clone();
                menu = menu.item(
                    PopupMenuItem::new(question_source_label(&candidate))
                        .icon(gpui_component::Icon::default().path(match candidate {
                            Scope::Local => "icons/diff.svg",
                            Scope::PullRequest { .. } => "icons/git-pull-request.svg",
                        }))
                        .checked(candidate == selected)
                        .on_click(move |_, _, cx| {
                            let _ = owner.update(cx, |this, cx| {
                                this.questions
                                    .scopes
                                    .insert(target.clone(), candidate.clone());
                                cx.notify();
                            });
                        }),
                );
            }
            menu
        });
        self.questions.scope_subscription = Some(cx.subscribe_in(
            &menu,
            window,
            |this, _, _: &gpui::DismissEvent, window, cx| {
                this.questions.scope_menu = None;
                this.questions.scope_subscription = None;
                this.questions.scope_menu_owner = None;
                if let Some(focus) = &this.questions.scope_focus {
                    focus.focus(window, cx);
                }
                cx.notify();
            },
        ));
        self.questions.scope_menu_owner = Some(menu_owner);
        self.questions.scope_position = position;
        menu.focus_handle(cx).focus(window, cx);
        self.questions.scope_menu = Some(menu);
        cx.notify();
    }
    pub(super) fn render_questions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        super::super::mermaid::set_context(Some(cx.entity_id()), window, cx);
        let Some(k) = self.questions.open.clone() else {
            return div().into_any_element();
        };
        self.check_question(&k, cx);
        let focus = self
            .questions
            .focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        if let Some(e) = self.questions.entries.get_mut(&k) {
            if e.input.is_none() {
                e.input = Some(cx.new(|cx| {
                    InputState::new(window, cx)
                        .multi_line(true)
                        .auto_grow(3, 10)
                }));
            }
            if let Some(question) = e.pending_question.take() {
                if let Some(input) = &e.input {
                    input.update(cx, |input, cx| input.set_value(question, window, cx));
                }
            }
            if self.questions.focus_pending {
                if e.diff.is_none() {
                    if let Some(input) = &e.input {
                        input.focus_handle(cx).focus(window, cx);
                    } else {
                        focus.focus(window, cx);
                    }
                } else {
                    focus.focus(window, cx);
                }
                self.questions.focus_pending = false;
            }
        }
        let Some(e) = self.questions.entries.get(&k) else {
            return div().into_any_element();
        };
        let busy = e.phase.busy();
        let label = e.result.as_ref().map_or_else(
            || {
                e.scope.as_ref().map_or_else(
                    || "Ask about changes".into(),
                    |scope| match scope {
                        Scope::Local => "Local changes".into(),
                        Scope::PullRequest { url } => url
                            .trim_end_matches('/')
                            .strip_prefix("https://github.com/")
                            .unwrap_or(url)
                            .replace("/pull/", " #"),
                    },
                )
            },
            |a| a.snapshot.label.clone(),
        );
        let header = div()
            .debug_selector(|| "ask-reader-header".into())
            .min_h(rpx(APPBAR_TOTAL_H))
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(rpx(SPACE_SM))
            .px(rpx(SPACE_20))
            .py(rpx(SPACE_LG))
            .when(
                self.checkout_panel.toggle_available
                    && (self.checkout_panel.hidden
                        || Self::logical_window_width(window)
                            < super::checkout_panel::CHECKOUT_SIDE_VIEWPORT),
                |header| header.pr(rpx(READER_TOOLBAR_INSET)),
            )
            .child(
                action(
                    "ask-back",
                    if e.diff.is_some() {
                        "Back to answer"
                    } else {
                        "Conversation"
                    },
                    "arrow-left",
                    window,
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| this.close_questions(window, cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(c::FG_DIM())
                    .text_size(rpx(TEXT_SMALL))
                    .child(label),
            );
        let mut body = div()
            .id("ask-reader-body")
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .max_w(rpx(READER_WIDTH))
            .gap(rpx(SPACE_LG));
        if let Some((label, text)) = &e.diff {
            body = body
                .child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(format!("Captured reference · {label}")),
                )
                .child(text_view(text, window));
        } else {
            body = body.child(
                div()
                    .text_size(rpx(TEXT_TITLE))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Ask about changes"),
            );
            body=body.child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child("Each native question uses a fresh captured diff. Conversation opens the retained session for ordinary follow-up questions."));
            if let Some(input) = &e.input {
                body = body.child(
                    project_field_well(input.focus_handle(cx).is_focused(window), false)
                        .id("ask-question-well")
                        .debug_selector(|| "ask-question-well".into())
                        .w_full()
                        .min_w_0()
                        .h_auto()
                        .justify_start()
                        .py(rpx(SPACE_LG))
                        .overflow_hidden()
                        .child(
                            Input::new(input)
                                .aria_label("Question about changes")
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .p_0()
                                .text_size(rpx(TEXT_BODY))
                                .font_family(crate::fonts::UI_FAMILY)
                                .disabled(busy || e.resolving),
                        ),
                );
            }
            if e.resolving {
                body = body.child(status::label(
                    "ask-resolving",
                    StatusKind::Working,
                    "Finding the attached PR…",
                    ICON_SM,
                    cx,
                ));
            }
            if busy {
                let target = k.clone();
                body = body
                    .child(status::label(
                        "ask-generating",
                        StatusKind::Working,
                        if e.phase.saving() {
                            "Saving answer…"
                        } else {
                            "Reading the diff and answering…"
                        },
                        ICON_SM,
                        cx,
                    ))
                    .child(
                        action("ask-cancel", "Cancel", "close", window, cx)
                            .disabled(e.phase.saving())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(controller) = this
                                    .questions
                                    .entries
                                    .get(&target)
                                    .and_then(|e| e.controller.clone())
                                {
                                    controller.update(cx, ChangeSession::cancel);
                                }
                                cx.notify();
                            })),
                    );
            } else if e.scope.is_some() {
                let target = k.clone();
                body = body.child(
                    action(
                        "ask-submit",
                        if e.error.is_some() {
                            "Retry question"
                        } else {
                            "Ask question"
                        },
                        "arrow-up",
                        window,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.submit_question(&target, cx))),
                );
            }
            if e.error.is_some() && e.scope.is_none() && !e.resolving {
                let source = self.questions.source.clone();
                let target = k.clone();
                if let Some(source) = source {
                    body = body.child(
                        action("ask-retry-pr", "Retry PR lookup", "refresh", window, cx).on_click(
                            cx.listener(move |this, _, _, cx| {
                                if !this.questions_is_open(cx)
                                    || this.questions.open.as_ref() != Some(&target)
                                {
                                    return;
                                }
                                let question = this
                                    .questions
                                    .entries
                                    .get(&target)
                                    .and_then(|entry| entry.input.as_ref())
                                    .map(|input| input.read(cx).value().to_string())
                                    .unwrap_or_default();
                                this.invoke_ask_skill(
                                    Invocation {
                                        scope: ScopePreference::PullRequest,
                                        question,
                                    },
                                    source.clone(),
                                    cx,
                                );
                            }),
                        ),
                    );
                }
            }
            if let Some(error) = &e.error {
                body = body.child(status::label(
                    "ask-error",
                    if error.to_lowercase().contains("cancel") {
                        StatusKind::Canceled
                    } else {
                        StatusKind::Failed
                    },
                    error.clone(),
                    ICON_SM,
                    cx,
                ));
            }
            if let Some(answer) = &e.result {
                let freshness = match &e.freshness {
                    Some(Ok(true)) => "Snapshot is current",
                    Some(Ok(false)) => "Out of date · Ask again to capture a fresh diff.",
                    Some(Err(_)) => {
                        "Snapshot freshness could not be verified. Ask again to refresh."
                    }
                    None => "Checking snapshot freshness…",
                };
                body = body.child(
                    div()
                        .text_size(rpx(TEXT_SMALL))
                        .text_color(c::FG_DIM())
                        .child(format!(
                            "{} · HEAD {}",
                            freshness,
                            answer.snapshot.head.chars().take(12).collect::<String>()
                        )),
                );
                if let Some(text) = &e.text {
                    body = body.child(text_view(text, window));
                }
                for (i, cite) in answer.citations.iter().enumerate() {
                    let Some(file) = answer.snapshot.files.iter().find(|f| f.path == cite.path)
                    else {
                        continue;
                    };
                    let target = k.clone();
                    let patch = file.patch.clone();
                    let label = format!(
                        "{} · {} {}–{}",
                        cite.path,
                        if cite.side == domain::Side::New {
                            "new"
                        } else {
                            "old"
                        },
                        cite.start,
                        cite.end
                    );
                    let title = label.clone();
                    body = body.child(
                        action(
                            format!("ask-reference-{i}"),
                            format!("Open reference {}", i + 1),
                            "diff",
                            window,
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(e) = this.questions.entries.get_mut(&target) {
                                e.diff = Some((
                                    title.clone(),
                                    cx.new(|cx| {
                                        TextViewState::markdown(&patch_markdown(&patch), cx)
                                    }),
                                ));
                            }
                            this.questions.focus_pending = true;
                            cx.notify();
                        })),
                    );
                }
                let markdown = answer.markdown();
                let target = k.clone();
                body = body.child(
                    action("ask-copy", "Copy answer", "copy", window, cx).on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                markdown.clone(),
                            ));
                            if let Some(e) = this.questions.entries.get_mut(&target) {
                                e.receipt = Some("Answer copied".into());
                                e.receipt_started = Some(Instant::now());
                            }
                            let run = this.questions.entries.get(&target).map_or(0, |e| e.run);
                            if let Some(started) = this
                                .questions
                                .entries
                                .get(&target)
                                .and_then(|e| e.receipt_started)
                            {
                                this.expire_question_receipt(target.clone(), run, started, cx);
                            }
                            cx.notify();
                        },
                    )),
                );
            }
            if let Some(receipt) = &e.receipt {
                body = body.child(status::label(
                    "ask-receipt",
                    StatusKind::Success,
                    receipt.clone(),
                    ICON_SM,
                    cx,
                ));
            }
        }
        let scroll = if e.diff.is_some() {
            &e.diff_scroll
        } else {
            &e.scroll
        };
        let target = k.clone();
        let reader = div()
            .id("ask-reader")
            .debug_selector(|| "ask-reader".into())
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .track_focus(&focus)
            .bg(c::BG())
            .text_color(c::FG())
            .child(header)
            .child(
                div()
                    .id("ask-reader-scroll")
                    .debug_selector(|| "ask-reader-scroll".into())
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .track_scroll(scroll)
                    .px(rpx(SPACE_20))
                    .py(rpx(SPACE_2XL))
                    .flex()
                    .justify_center()
                    .child(body),
            )
            .capture_action(cx.listener(
                move |this, action: &gpui_component::input::Enter, window, cx| {
                    if action.secondary {
                        this.submit_question(&target, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                },
            ));
        motion::fast(reader, "ask-reader-enter", cx)
    }
}
#[cfg(test)]
#[path = "change_questions_tests.rs"]
mod tests;
