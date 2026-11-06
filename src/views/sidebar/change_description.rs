//! Checkout-owned descriptions. Every run, freshness check and publication retains its source key.
use super::*;
use crate::views::{
    components::{form_action, project_field_well, transcript_action},
    status::{self, StatusKind},
};
use gpui_component::{
    input::{Input, InputState},
    text::{TextView, TextViewState},
    Disableable, Selectable,
};
use grove_core::{
    change_description::{self as domain, Description, Scope},
    commit_agent::CommitControl,
    describe_skill::ScopePreference,
};
use std::sync::Arc;

const DESCRIPTION_WIDTH: f32 = 700.;
const FRESHNESS_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    owner: String,
    path: String,
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
    return_focus: Option<FocusHandle>,
    open_source: Option<FocusHandle>,
    focus_pending: bool,
}
#[derive(Default)]
struct Entry {
    scope: Option<Scope>,
    result: Option<Arc<Description>>,
    run: u64,
    cancel: Option<Arc<CommitControl>>,
    publishing: bool,
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
fn text_view(view: &Entity<TextViewState>, window: &Window) -> TextView {
    TextView::new(view)
        .markdown_extensions(super::super::mermaid::extensions())
        .style(super::super::codex_view::markdown_style(window))
        .selectable(true)
}
impl Sidebar {
    pub(super) fn description_is_open(&self, cx: &App) -> bool {
        let targets = self.checkout_targets(cx);
        let selected = super::checkout_panel::selected_checkout(
            &targets,
            self.checkout_panel.chosen_root.as_deref(),
        );
        self.descriptions.open.as_ref().is_some_and(|key| {
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
            self.begin_description(
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
            self.begin_description(path, Scope::Local, Some(source), cx);
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
                    this.begin_description(path, scope, Some(source), cx);
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
        let provider = match self.checkout_commit_provider(cx) {
            Ok(p) => p,
            Err(error) => {
                self.description_error(error, cx);
                return;
            }
        };
        let key = key(&self.selection, &path, &scope);
        if self
            .descriptions
            .entries
            .get(&key)
            .is_some_and(|entry| entry.cancel.is_some() || entry.publishing)
        {
            self.descriptions.open = Some(key);
            self.descriptions.focus_pending = true;
            if let Some(source) = source {
                self.descriptions.return_focus = Some(source);
            }
            cx.notify();
            return;
        }
        self.descriptions.revision = self.descriptions.revision.wrapping_add(1);
        let run = self.descriptions.revision;
        let cancel = Arc::new(CommitControl::default());
        let entry = self.descriptions.entries.entry(key.clone()).or_default();
        entry.scope = Some(scope.clone());
        entry.cancel = Some(cancel.clone());
        entry.run = run;
        entry.error = None;
        entry.receipt = None;
        entry.preview = None;
        if let Some(source) = source {
            self.descriptions.return_focus = Some(source);
        }
        self.descriptions.open = Some(key.clone());
        self.descriptions.focus_pending = true;
        self.descriptions
            .focus
            .get_or_insert_with(|| cx.focus_handle());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    domain::generate(
                        std::path::Path::new(&path),
                        &scope,
                        provider,
                        &cancel,
                        |_| {},
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(entry) = this
                    .descriptions
                    .entries
                    .get_mut(&key)
                    .filter(|e| e.run == run)
                else {
                    return;
                };
                entry.cancel = None;
                match result {
                    Ok(description) => {
                        let summary = format!(
                            "# {}\n\n{}\n\n## Description\n\n{}",
                            description.title,
                            description.types.join(" · "),
                            description
                                .summary
                                .iter()
                                .map(|s| format!("- {s}"))
                                .collect::<Vec<_>>()
                                .join("\n")
                        );
                        entry.text = Some(cx.new(|cx| TextViewState::markdown(&summary, cx)));
                        entry.file_text.clear();
                        for file in &description.files {
                            let source = format!(
                                "**{}**\n\n{}",
                                file.title,
                                file.summary
                                    .iter()
                                    .map(|s| format!("- {s}"))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            );
                            entry.file_text.insert(
                                file.path.clone(),
                                cx.new(|cx| TextViewState::markdown(&source, cx)),
                            );
                        }
                        entry.diagram = description.diagram.as_ref().map(|diagram| {
                            cx.new(|cx| {
                                TextViewState::markdown(&format!("```mermaid\n{diagram}\n```"), cx)
                            })
                        });
                        entry.result = Some(Arc::new(description));
                        entry.freshness = None;
                        entry.checked_at = None;
                        entry.checking = false;
                        entry.expanded.clear();
                        entry.scroll = ScrollHandle::default();
                        entry.diff = None;
                        entry.receipt = Some("Description ready".into());
                    }
                    Err(error) => entry.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn cancel_description(&mut self, key: &Key, cx: &mut Context<Self>) {
        if let Some(entry) = self.descriptions.entries.get_mut(key) {
            if let Some(cancel) = &entry.cancel {
                cancel.cancel();
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
            .px(rpx(SPACE_2XL))
            .py(rpx(SPACE_LG));
        column = column.child(
            div()
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .child("Describe"),
        );
        let mut choices = div().flex().flex_wrap().gap(rpx(SPACE_SM));
        for (label, candidate, enabled, glyph) in [
            (
                "Local changes".to_string(),
                Scope::Local,
                local_changes,
                "diff",
            ),
            (
                pr.as_ref()
                    .map_or("Attached PR".into(), |p| format!("PR #{}", p.number)),
                pr.as_ref()
                    .map_or(Scope::Local, |p| Scope::PullRequest { url: p.url.clone() }),
                pr.is_some(),
                "git-pull-request",
            ),
        ] {
            if !enabled {
                continue;
            }
            let selected = candidate == scope;
            let target = owner_key.clone();
            choices = choices.child(
                action(format!("describe-scope-{glyph}"), label, glyph, window, cx)
                    .selected(selected)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.descriptions
                            .scopes
                            .insert(target.clone(), candidate.clone());
                        cx.notify();
                    })),
            );
        }
        column = column.child(choices);
        let focus = self
            .descriptions
            .source
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let target = path.to_owned();
        let chosen = scope.clone();
        column = column.child(
            super::checkout_panel::checkout_loading_row(
                "describe-changes",
                if entry.is_some_and(|e| e.publishing) {
                    "Updating description…"
                } else if busy {
                    "Describing changes…"
                } else if exists {
                    "Regenerate description"
                } else {
                    "Describe changes"
                },
                "diff",
                busy,
                window,
                cx,
            )
            .px(rpx(SPACE_LG))
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
        if exists {
            let open_focus = self
                .descriptions
                .open_source
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            let return_focus = open_focus.clone();
            let target = key.clone();
            column = column.child(
                action("describe-open", "Open description", "eye", window, cx)
                    .with_focus_handle(open_focus)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.descriptions.return_focus = Some(return_focus.clone());
                        this.descriptions.open = Some(target.clone());
                        this.descriptions.focus_pending = true;
                        cx.notify();
                    })),
            );
        }
        if busy {
            let target = key.clone();
            column = column.child(
                action("describe-cancel", "Cancel generation", "close", window, cx)
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
                    .child(if matches!(scope, Scope::Local) {
                        "Staged + unstaged + untracked · compared with HEAD"
                    } else {
                        "Attached PR · compared with its base"
                    }),
            )
            .into_any_element()
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
                    domain::publish(std::path::Path::new(&path), &description, &body)
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
            .px(rpx(SPACE_3XL));
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
                "Reading the diff and describing changes…",
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
                        "Changes updated since this description. Regenerate before publishing.",
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
                    .disabled(busy || publishing || !matches!(entry.freshness, Some(Ok(true))))
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
