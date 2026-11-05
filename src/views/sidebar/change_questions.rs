//! Native, checkout/session-owned questions. Each submission captures a new immutable diff.
use super::*;
use crate::views::{
    components::{project_field_well, transcript_action},
    status::{self, StatusKind},
};
use gpui_component::{
    input::{Input, InputState},
    text::{TextView, TextViewState},
    Selectable,
};
use grove_core::{
    ask_skill::Invocation,
    change_description::{self as snapshot, Scope},
    change_questions::{self as domain, Answer},
    commit_agent::CommitControl,
    describe_skill::ScopePreference,
};
use std::sync::Arc;
const READER_WIDTH: f32 = 700.;
const FRESHNESS_INTERVAL: Duration = Duration::from_secs(5);
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Key {
    owner: String,
    path: String,
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
}
#[derive(Default)]
struct Entry {
    pending_question: Option<String>,
    auto_submit: bool,
    scope: Option<Scope>,
    input: Option<Entity<InputState>>,
    result: Option<Arc<Answer>>,
    text: Option<Entity<TextViewState>>,
    diff: Option<(String, Entity<TextViewState>)>,
    cancel: Option<Arc<CommitControl>>,
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
impl Drop for Questions {
    fn drop(&mut self) {
        for e in self.entries.values() {
            if let Some(c) = &e.cancel {
                c.cancel();
            }
        }
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
    let selector = id.to_string();
    transcript_action(id, label, false, window, cx)
        .debug_selector(move || selector.clone())
        .icon(
            gpui_component::Icon::default()
                .path(SharedString::from(format!("icons/{glyph}.svg")))
                .size(rpx(ICON_SM)),
        )
}
fn text_view(text: &Entity<TextViewState>, window: &Window) -> TextView {
    TextView::new(text)
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
        if self.active_change_reader == Some("ask") {
            self.active_change_reader = None;
        }
    }
    pub(super) fn questions_is_open(&self, cx: &App) -> bool {
        if self.active_change_reader != Some("ask") {
            return false;
        }
        let targets = self.checkout_targets(cx);
        let selected = super::checkout_panel::selected_checkout(
            &targets,
            self.checkout_panel.chosen_root.as_deref(),
        );
        self.questions.open.as_ref().is_some_and(|k| {
            k.owner == owner(&self.selection) && selected.is_some_and(|(_, p)| p == k.path)
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
        if self.active_change_reader == Some("ask") {
            self.active_change_reader = None;
        }
        if let Some(f) = self.questions.source.take() {
            f.focus(window, cx);
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
        self.dismiss_description();
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
        self.active_change_reader = Some("ask");
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
                if this.active_change_reader != Some("ask")
                    || this.questions.open.as_ref() != Some(&k)
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
        let k = key(&self.selection, &path, &scope);
        let has_question = !question.trim().is_empty();
        self.questions
            .scopes
            .insert((owner(&self.selection), path), scope.clone());
        let e = self.questions.entries.entry(k.clone()).or_default();
        e.scope = Some(scope);
        e.resolving = false;
        // Keep an edited native question if a second invocation arrives while work is running.
        if has_question && e.cancel.is_none() {
            e.pending_question = Some(question);
        }
        if let Some(s) = source {
            self.questions.source = Some(s);
        }
        self.questions.open = Some(k);
        self.active_change_reader = Some("ask");
        self.questions.focus_pending = true;
        self.files_active = false;
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
        if e.cancel.is_some() || e.resolving {
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
        let provider = match self.checkout_commit_provider(cx) {
            Ok(p) => p,
            Err(e) => {
                self.question_error(e, cx);
                return;
            }
        };
        self.questions.revision = self.questions.revision.wrapping_add(1);
        let run = self.questions.revision;
        let control = Arc::new(CommitControl::default());
        let Some(e) = self.questions.entries.get_mut(k) else {
            return;
        };
        e.run = run;
        // An older freshness callback is fenced by run identity. Release its
        // in-flight flag now so a failed replacement can recheck the old answer.
        e.checking = false;
        e.checked_at = None;
        e.freshness = None;
        e.cancel = Some(control.clone());
        e.error = None;
        e.receipt = None;
        e.receipt_started = None;
        let k = k.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let path = k.path.clone();
            let result = cx
                .background_spawn(async move {
                    domain::generate(
                        std::path::Path::new(&path),
                        &scope,
                        &question,
                        provider,
                        &control,
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(e) = this.questions.entries.get_mut(&k).filter(|e| e.run == run) else {
                    return;
                };
                e.cancel = None;
                match result {
                    Ok(answer) => {
                        e.text = Some(cx.new(|cx| TextViewState::markdown(&answer.markdown(), cx)));
                        e.result = Some(Arc::new(answer));
                        e.freshness = None;
                        e.checked_at = None;
                        e.checking = false;
                        e.diff = None;
                        e.receipt = Some("Answer ready".into());
                        e.receipt_started = Some(Instant::now());
                    }
                    Err(error) => e.error = Some(error),
                }
                if let Some(receipt) = this
                    .questions
                    .entries
                    .get(&k)
                    .and_then(|e| e.receipt_started)
                {
                    this.expire_question_receipt(k.clone(), run, receipt, cx);
                }
                cx.notify();
            });
        })
        .detach();
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
        if e.cancel.is_some()
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
        if !local && pr.is_none() {
            return div().into_any_element();
        }
        let owner_key = (owner(&self.selection), path.to_string());
        let preferred = self
            .questions
            .scopes
            .get(&owner_key)
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
            (Scope::PullRequest { .. }, Some(pr)) => Scope::PullRequest {
                url: pr.url.clone(),
            },
            (Scope::PullRequest { .. }, None) => Scope::Local,
            _ => preferred,
        };
        self.questions
            .scopes
            .insert(owner_key.clone(), scope.clone());
        let mut row = div()
            .id("ask-controls")
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM))
            .px(rpx(SPACE_2XL))
            .py(rpx(SPACE_LG));
        row = row.child(
            div()
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .child("Ask about changes"),
        );
        let mut choices = div().flex().flex_wrap().gap(rpx(SPACE_SM));
        for (label, candidate, enabled, glyph) in [
            ("Local changes", Scope::Local, local, "diff"),
            (
                "Attached PR",
                pr.map_or(Scope::Local, |p| Scope::PullRequest { url: p.url }),
                matches!(scope, Scope::PullRequest { .. })
                    || self.checkout_description_pr().is_some(),
                "git-pull-request",
            ),
        ] {
            if !enabled {
                continue;
            }
            let selected = candidate == scope;
            let choice_key = owner_key.clone();
            choices = choices.child(
                action(format!("ask-scope-{label}"), label, glyph, window, cx)
                    .selected(selected)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.questions
                            .scopes
                            .insert(choice_key.clone(), candidate.clone());
                        cx.notify();
                    })),
            );
        }
        let target = path.to_string();
        let source = self
            .questions
            .control_focus
            .entry(owner_key)
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let return_source = source.clone();
        row.child(choices)
            .child(
                action(
                    "ask-open",
                    "Ask about changes",
                    "message-circle",
                    window,
                    cx,
                )
                .with_focus_handle(source)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_question(
                        target.clone(),
                        scope.clone(),
                        String::new(),
                        Some(return_source.clone()),
                        cx,
                    );
                })),
            )
            .into_any_element()
    }
    pub(super) fn render_questions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
                    e.auto_submit = true;
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
        let auto = self
            .questions
            .entries
            .get_mut(&k)
            .is_some_and(|e| std::mem::take(&mut e.auto_submit));
        if auto {
            let target = k.clone();
            cx.defer_in(window, move |this, _, cx| this.submit_question(&target, cx));
        }
        let Some(e) = self.questions.entries.get(&k) else {
            return div().into_any_element();
        };
        let busy = e.cancel.is_some();
        let label = e
            .result
            .as_ref()
            .map_or("Ask about changes".into(), |a| a.snapshot.label.clone());
        let header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(rpx(SPACE_SM))
            .px(rpx(SPACE_20))
            .py(rpx(SPACE_LG))
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
            body=body.child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child("Each question uses a fresh captured diff, independently of previous answers. Your conversation and files stay unchanged."));
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
                        "Reading the diff and answering…",
                        ICON_SM,
                        cx,
                    ))
                    .child(
                        action("ask-cancel", "Cancel", "close", window, cx).on_click(cx.listener(
                            move |this, _, _, cx| {
                                if let Some(c) = this
                                    .questions
                                    .entries
                                    .get(&target)
                                    .and_then(|e| e.cancel.as_ref())
                                {
                                    c.cancel();
                                }
                                cx.notify();
                            },
                        )),
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
                    Some(Ok(false)) => "Changes have moved on. Ask again to capture a fresh diff.",
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
