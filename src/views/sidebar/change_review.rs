//! Checkout-owned reviews. Every run and freshness check retains its source key.
use super::*;
use crate::views::{
    components::transcript_action,
    status::{self, StatusKind},
};
use gpui_component::{
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

const REVIEW_WIDTH: f32 = 700.;
const FRESHNESS_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    owner: String,
    path: String,
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
}
#[derive(Default)]
struct Entry {
    scope: Option<Scope>,
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
impl Drop for Reviews {
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
    pub(super) fn dismiss_review(&mut self) {
        self.reviews.open = None;
        if self.active_change_reader == Some("review") {
            self.active_change_reader = None;
        }
    }
    pub(super) fn review_is_open(&self, cx: &App) -> bool {
        if self.active_change_reader != Some("review") {
            return false;
        }
        let targets = self.checkout_targets(cx);
        let selected = super::checkout_panel::selected_checkout(
            &targets,
            self.checkout_panel.chosen_root.as_deref(),
        );
        self.reviews.open.as_ref().is_some_and(|key| {
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
        if self.active_change_reader == Some("review") {
            self.active_change_reader = None;
        }
        if let Some(source) = self.reviews.return_focus.take() {
            source.focus(window, cx);
        }
        cx.notify();
    }
    pub(super) fn invoke_review_skill(
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
            self.review_error("Select a checkout before reviewing changes.", cx);
            return;
        };
        // Native skill invocation replaces Files/Side chat without changing their drafts or requiring an already mounted Checkout card.
        self.dismiss_description();
        self.files_active = false;
        self.checkout_panel.side_chat_owner = None;
        let selected_scope = self
            .reviews
            .scopes
            .get(&(owner(&self.selection), path.clone()))
            .cloned();
        if preference == ScopePreference::Local
            || (preference == ScopePreference::Current
                && !matches!(selected_scope, Some(Scope::PullRequest { .. })))
        {
            self.reviews
                .scopes
                .insert((owner(&self.selection), path.clone()), Scope::Local);
            self.begin_review(path, Scope::Local, Some(source), cx);
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

        self.active_change_reader = Some("review");
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
                let active = this.active_change_reader == Some("review") && this.reviews.open.as_ref() == Some(&resolving_key) && this.reviews.entries.get(&resolving_key).is_some_and(|entry| entry.run == run && entry.resolving);
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
                    this.begin_review(path, scope, Some(source), cx);
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
        let provider = match self.checkout_commit_provider(cx) {
            Ok(p) => p,
            Err(error) => {
                self.review_error(error, cx);
                return;
            }
        };
        self.dismiss_description();
        let key = key(&self.selection, &path, &scope);
        if self
            .reviews
            .entries
            .get(&key)
            .is_some_and(|entry| entry.cancel.is_some())
        {
            self.reviews.open = Some(key);

            self.active_change_reader = Some("review");
            self.reviews.focus_pending = true;
            if let Some(source) = source {
                self.reviews.return_focus = Some(source);
            }
            cx.notify();
            return;
        }
        self.reviews.revision = self.reviews.revision.wrapping_add(1);
        let run = self.reviews.revision;
        let cancel = Arc::new(CommitControl::default());
        let entry = self.reviews.entries.entry(key.clone()).or_default();
        entry.scope = Some(scope.clone());
        entry.cancel = Some(cancel.clone());
        entry.run = run;
        // A callback from the previous run can no longer clear its in-flight flag.
        entry.checking = false;
        entry.checked_at = None;
        entry.freshness = None;
        entry.error = None;
        entry.receipt = None;
        if let Some(source) = source {
            self.reviews.return_focus = Some(source);
        }
        self.reviews.open = Some(key.clone());

        self.active_change_reader = Some("review");
        self.reviews.focus_pending = true;
        self.reviews.focus.get_or_insert_with(|| cx.focus_handle());
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
                let Some(entry) = this.reviews.entries.get_mut(&key).filter(|e| e.run == run)
                else {
                    return;
                };
                entry.cancel = None;
                match result {
                    Ok(review) => {
                        let summary = review.summary_markdown();
                        entry.text = Some(cx.new(|cx| TextViewState::markdown(&summary, cx)));
                        entry.file_text.clear();
                        for file in &review.files {
                            let source =
                                format!("**{}**\n\n{}", file.title, file.summary.join("\n\n"));
                            entry.file_text.insert(
                                file.path.clone(),
                                cx.new(|cx| TextViewState::markdown(&source, cx)),
                            );
                        }
                        entry.result = Some(Arc::new(review));
                        entry.freshness = None;
                        entry.checked_at = None;
                        entry.checking = false;
                        entry.expanded.clear();
                        entry.scroll = ScrollHandle::default();
                        entry.diff = None;
                        entry.receipt = Some("Review ready".into());
                    }
                    Err(error) => entry.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn cancel_review(&mut self, key: &Key, cx: &mut Context<Self>) {
        if let Some(entry) = self.reviews.entries.get_mut(key) {
            if let Some(cancel) = &entry.cancel {
                cancel.cancel();
            }
            // Do not report cancellation until the child process has acknowledged exit.
        }
        cx.notify();
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
            (Scope::PullRequest { .. }, Some(pr)) => Scope::PullRequest {
                url: pr.url.clone(),
            },
            (Scope::PullRequest { .. }, None) => Scope::Local,
            _ => scope,
        };
        self.reviews.scopes.insert(owner_key.clone(), scope.clone());
        let key = key(&self.selection, path, &scope);
        let entry = self.reviews.entries.get(&key);
        let busy = entry.is_some_and(|e| e.cancel.is_some());
        let exists = entry.is_some_and(|e| e.result.is_some());
        let mut column = div()
            .id("review-controls")
            .debug_selector(|| "review-controls".into())
            .flex()
            .flex_col()
            .gap(rpx(SPACE_SM))
            .px(rpx(SPACE_2XL))
            .py(rpx(SPACE_LG));
        column = column.child(
            div()
                .text_size(rpx(TEXT_SMALL))
                .text_color(c::FG_DIM())
                .child("Review"),
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
                action(format!("review-scope-{glyph}"), label, glyph, window, cx)
                    .selected(selected)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.reviews
                            .scopes
                            .insert(target.clone(), candidate.clone());
                        cx.notify();
                    })),
            );
        }
        column = column.child(choices);
        let focus = self
            .reviews
            .source
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let target = path.to_owned();
        let chosen = scope.clone();
        column = column.child(
            super::checkout_panel::checkout_loading_row(
                "review",
                if busy {
                    "Reviewing changes…"
                } else if exists {
                    "Regenerate review"
                } else {
                    "Review changes"
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
                this.begin_review(
                    target.clone(),
                    chosen.clone(),
                    this.reviews.source.clone(),
                    cx,
                );
            })),
        );
        if exists {
            let open_focus = self
                .reviews
                .open_source
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            let return_focus = open_focus.clone();
            let target = key.clone();
            column = column.child(
                action("review-open", "Open review", "eye", window, cx)
                    .with_focus_handle(open_focus)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.reviews.return_focus = Some(return_focus.clone());
                        this.reviews.open = Some(target.clone());

                        this.active_change_reader = Some("review");
                        this.reviews.focus_pending = true;
                        cx.notify();
                    })),
            );
        }
        if busy {
            let target = key.clone();
            column = column.child(
                action("review-cancel", "Cancel generation", "close", window, cx)
                    .on_click(cx.listener(move |this, _, _, cx| this.cancel_review(&target, cx))),
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
            .px(rpx(SPACE_3XL));
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
                "Reading the diff and reviewing changes…",
                ICON_SM,
                cx,
            ));
            let target = key.clone();
            body = body.child(
                action("review-cancel", "Cancel generation", "close", window, cx)
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
                        "Changes updated since this review. Regenerate to review the latest snapshot.",
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
            body = body.child(div().mt(rpx(SPACE_LG)).text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child(format!("{} of {} files reviewed · {omitted} files omitted or partial. Based on the diff; tests were not executed by this review.", review.files.len(), review.snapshot.files.len())));
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
