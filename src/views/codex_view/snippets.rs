//! User-initiated shell runs owned by the original response, never by the agent.
use super::{
    references::{new_reference_id, COMMAND_OUTPUT_REFERENCE_ROLE},
    *,
};
use crate::entities::terminal_session::TerminalSession;
use crate::fonts::MONO_FAMILY;
use crate::views::terminal_view::TerminalView;
use gpui::{Anchor, AnyElement};
use gpui_component::popover::Popover;

const OUTPUT_HEIGHT: f32 = 240.0;
const COMMAND_MENU_W: f32 = 296.0;
const REFERENCED_OUTPUT_LINES: usize = 200;
static SHELL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Source validity is independent of render preparation: session observers may
/// arrive before the transcript's next render. Index once per item revision and
/// hash a completed response only when a script from it actually needs validation.
#[derive(Default)]
pub(super) struct SnippetSources {
    revision: Option<u64>,
    thread: Option<String>,
    sources: rustc_hash::FxHashMap<String, Rc<SnippetSource>>,
    reconciled: Option<(u64, Option<String>)>,
}

struct SnippetSource {
    item: codex::TranscriptItem,
    signature: std::cell::OnceCell<Option<u64>>,
}

impl SnippetSources {
    fn refresh(&mut self, state: &codex::Snapshot) {
        let revision = state.saved.items.revision();
        if self.revision == Some(revision) && self.thread == state.saved.thread_id {
            return;
        }
        let mut previous = if self.thread == state.saved.thread_id {
            std::mem::take(&mut self.sources)
        } else {
            rustc_hash::FxHashMap::default()
        };
        self.sources.clear();
        for (index, item) in state.saved.items.iter().enumerate() {
            if item["type"] != "agentMessage"
                || (item["groveCompleted"] != true && item["status"] != "completed")
                || item["status"] == "inProgress"
            {
                continue;
            }
            let id = item["id"].as_str().map_or_else(
                || std::borrow::Cow::Owned(format!("message-{index}")),
                std::borrow::Cow::Borrowed,
            );
            let (key, source) = previous
                .remove_entry(id.as_ref())
                .filter(|(_, source)| std::ptr::eq(source.item.as_ref(), item.as_ref()))
                .unwrap_or_else(|| {
                    (
                        id.into_owned(),
                        Rc::new(SnippetSource {
                            item: item.clone(),
                            signature: std::cell::OnceCell::new(),
                        }),
                    )
                });
            // Retain the original first-completed-match behavior for duplicate IDs.
            self.sources.entry(key).or_insert(source);
        }
        self.revision = Some(revision);
        self.thread.clone_from(&state.saved.thread_id);
    }

    fn signature(&mut self, state: &codex::Snapshot, key: &str) -> Option<u64> {
        self.refresh(state);
        let owner = key.rsplit_once(":code:")?.0;
        let id = owner
            .strip_prefix(self.thread.as_deref().unwrap_or(""))?
            .strip_prefix(':')?;
        let source = self.sources.get(id)?;
        *source.signature.get_or_init(|| {
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            source.item["text"].as_str()?.hash(&mut hash);
            Some(hash.finish())
        })
    }

    fn needs_reconciliation(&mut self, state: &codex::Snapshot) -> bool {
        let revision = state.saved.items.revision();
        if self.reconciled.as_ref().is_some_and(|(previous, thread)| {
            *previous == revision && *thread == state.saved.thread_id
        }) {
            return false;
        }
        self.reconciled = Some((revision, state.saved.thread_id.clone()));
        true
    }
}

pub(super) struct SnippetRun {
    command: String,
    cwd: String,
    pub(super) terminal: Entity<TerminalSession>,
    view: Entity<TerminalView>,
    pub(super) expanded: bool,
    stop_requested: bool,
    reference_key: String,
    toggle_focus: FocusHandle,
    _observer: Subscription,
}

pub(super) struct Snippet {
    source: String,
    source_signature: Option<u64>,
    cwd: String,
    cwd_available: bool,
    custom_directory: bool,
    editor: Option<Entity<InputState>>,
    editor_observer: Option<Subscription>,
    run_focus: FocusHandle,
    menu_focus: FocusHandle,
    menu: Option<(Entity<PopupMenu>, Subscription)>,
    review_focus: FocusHandle,
    review: Option<String>,
    error: Option<String>,
    pub(super) runs: Vec<SnippetRun>,
}

impl Snippet {
    fn is_running(&self, cx: &App) -> bool {
        self.runs
            .last()
            .is_some_and(|run| !run.terminal.read(cx).has_exited())
    }

    fn can_run_original(&self, cx: &App) -> bool {
        !self.is_running(cx) && self.review.is_none() && self.editor.is_none()
    }

    #[cfg(test)]
    pub(super) fn options_open(&self) -> bool {
        self.menu.is_some()
    }

    #[cfg(test)]
    pub(super) fn options_focused(&self, window: &Window) -> bool {
        self.menu_focus.is_focused(window)
    }
}

#[derive(Clone, Copy)]
enum SnippetMenuAction {
    Copy,
    Edit,
    Directory,
}

#[derive(Clone, Copy)]
pub(super) enum SnippetControl {
    Run,
    Menu,
}

pub(super) fn source_digest(source: &str) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hash);
    hash.finish()
}

#[cfg(test)]
pub(super) fn key(owner: &str, offset: usize, code: &str, source: &str) -> String {
    key_with_digest(owner, offset, code, source_digest(source))
}

#[cfg(test)]
pub(super) fn key_with_digest(owner: &str, offset: usize, code: &str, source: u64) -> String {
    key_with_digests(owner, offset, source_digest(code), source)
}

pub(super) fn key_with_digests(owner: &str, offset: usize, code: u64, source: u64) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut hash);
    source.hash(&mut hash);
    format!("{owner}:code:{offset}:{:x}", hash.finish())
}

fn action_icon(name: &str) -> gpui_component::Icon {
    gpui_component::Icon::default()
        .path(format!("icons/{name}.svg"))
        .size(rpx(ICON_SM))
}

// The visual recipe supplies focus styling; the owner supplies activation for
// both input methods. Disabled controls reject keyboard activation as well.
fn bound_action(
    button: gpui_component::button::Button,
    enabled: bool,
    cx: &Context<CodexView>,
    action: impl Fn(&mut CodexView, &mut Window, &mut Context<CodexView>) + 'static,
) -> gpui_component::button::Button {
    let action = Rc::new(action);
    let keyboard = action.clone();
    button
        .disabled(!enabled)
        .on_key_down(
            cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                if enabled && action_key(event) && !event.is_held {
                    keyboard(this, window, cx);
                    cx.stop_propagation();
                }
            }),
        )
        .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
}

fn needs_terminal_review(command: &str) -> bool {
    command
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|word| word == "sudo")
}

impl CodexView {
    fn snippet_source_signature(&self, key: &str, cx: &App) -> Option<u64> {
        self.snippet_sources
            .borrow_mut()
            .signature(&self.session.read(cx).state, key)
    }

    fn ensure_snippet(&mut self, key: &str, source: &str, cwd: &str, cx: &App) -> &mut Snippet {
        // Split field borrows so the expensive source lookup belongs exclusively
        // to insertion; unchanged controls do not revisit the response text.
        let sources = &self.snippet_sources;
        let state = &self.session.read(cx).state;
        self.snippets
            .entry(key.to_owned())
            .or_insert_with(|| Snippet {
                source: source.to_owned(),
                source_signature: sources.borrow_mut().signature(state, key),
                cwd: cwd.to_owned(),
                cwd_available: std::path::Path::new(cwd).is_dir(),
                custom_directory: false,
                editor: None,
                editor_observer: None,
                run_focus: cx.focus_handle(),
                menu_focus: cx.focus_handle(),
                menu: None,
                review_focus: cx.focus_handle(),
                review: None,
                error: None,
                runs: Vec::new(),
            })
    }

    /// Static script controls keep their original focus handles when their row
    /// is projected offscreen. Dynamic editors/output retain the real row.
    pub(super) fn snippet_requires_eager(&self, key: &str) -> bool {
        self.snippets.get(key).is_some_and(|snippet| {
            snippet.custom_directory
                || snippet.menu.is_some()
                || snippet.editor.is_some()
                || snippet.review.is_some()
                || snippet.error.is_some()
                || !snippet.runs.is_empty()
        })
    }

    pub(super) fn snippet_projection_controls(
        &self,
        key: &str,
        cx: &App,
    ) -> Vec<(FocusHandle, SnippetControl)> {
        let Some(snippet) = self.snippets.get(key) else {
            return Vec::new();
        };
        let mut controls = Vec::new();
        if snippet.can_run_original(cx) {
            controls.push((snippet.run_focus.clone(), SnippetControl::Run));
        }
        controls.push((snippet.menu_focus.clone(), SnippetControl::Menu));
        controls
    }

    pub(super) fn activate_snippet_control(
        &mut self,
        key: &str,
        kind: SnippetControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match kind {
            SnippetControl::Run => {
                if self
                    .snippets
                    .get(key)
                    .is_some_and(|snippet| snippet.can_run_original(cx))
                {
                    self.request_snippet_run(key, false, window, cx);
                }
            }
            SnippetControl::Menu => self.toggle_snippet_menu(key, window, cx),
        }
    }

    fn toggle_snippet_menu(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let signature = self.snippet_source_signature(key, cx);
        if self.transcript_only() {
            return;
        }
        let Some(snippet) = self.snippets.get_mut(key) else {
            return;
        };
        if snippet.menu.take().is_some() {
            snippet.menu_focus.focus(window, cx);
            cx.notify();
            return;
        }
        if signature.is_none() || signature != snippet.source_signature {
            return;
        }

        let can_edit = snippet.can_run_original(cx);
        let can_change_directory = !snippet.is_running(cx);
        let directory_label = if snippet.cwd_available {
            "Change directory"
        } else {
            "Choose directory"
        };
        let path = snippet.cwd.clone();
        let shell = SHELL.get_or_init(grove_core::env_path::login_shell);
        let focus = snippet.menu_focus.clone();
        let key = key.to_owned();
        let owner = cx.entity().downgrade();
        let dismiss_key = key.clone();
        let menu_key = key.clone();
        let popup = PopupMenu::build(window, cx, move |popup, window, _| {
            let width = rpx(COMMAND_MENU_W)
                .to_pixels(window.rem_size())
                .min(window.viewport_size().width - rpx(SPACE_3XL).to_pixels(window.rem_size()));
            let mut popup = crate::views::components::dropdown_menu(popup, window)
                .action_context(focus)
                .min_w(width)
                .max_w(width)
                .item(
                    PopupMenuItem::element(move |_, _| {
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(rpx(SPACE_SM))
                            .py(rpx(SPACE_SM))
                            .child(format!("Local · {shell}"))
                            .child(crate::views::components::readonly_value(
                                "snippet-execution-path",
                                path.clone(),
                            ))
                    })
                    .disabled(true),
                )
                .separator();
            for (action, label, icon, enabled) in [
                (SnippetMenuAction::Copy, "Copy code", "copy", true),
                (SnippetMenuAction::Edit, "Edit & run", "edit", can_edit),
                (
                    SnippetMenuAction::Directory,
                    directory_label,
                    "folder",
                    can_change_directory,
                ),
            ] {
                let owner = owner.clone();
                let key = key.clone();
                let item = PopupMenuItem::new(label)
                    .icon(action_icon(icon))
                    .disabled(!enabled);
                popup = popup.item(item.on_click(move |_, window, cx| {
                    if !enabled {
                        return;
                    }
                    let owner = owner.clone();
                    let key = key.clone();
                    // PopupMenu restores its anchor focus after dispatch.
                    window.defer(cx, move |window, cx| {
                        let _ = owner.update(cx, |view, cx| {
                            let Some(snippet) = view.snippets.get(&key) else {
                                return;
                            };
                            if view.snippet_source_signature(&key, cx) != signature {
                                return;
                            }
                            match action {
                                SnippetMenuAction::Copy => {
                                    let code = snippet.source.clone();
                                    view.copy_code_block(key, code, cx);
                                }
                                SnippetMenuAction::Edit => view.edit_snippet(&key, window, cx),
                                SnippetMenuAction::Directory if !snippet.is_running(cx) => {
                                    view.choose_snippet_directory(key, window, cx);
                                }
                                _ => {}
                            }
                        });
                    });
                }));
            }
            popup
        });
        let dismissed = cx.subscribe_in(
            &popup,
            window,
            move |view, _, _: &gpui::DismissEvent, _, cx| {
                if let Some(snippet) = view.snippets.get_mut(&dismiss_key) {
                    snippet.menu = None;
                    cx.notify();
                }
            },
        );
        popup.focus_handle(cx).focus(window, cx);
        if let Some(snippet) = self.snippets.get_mut(&menu_key) {
            snippet.menu = Some((popup, dismissed));
        }
        cx.notify();
    }

    pub(super) fn snippet_actions(
        &mut self,
        key: &str,
        code: &str,
        cwd: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let copied = self.copied_code_blocks.contains_key(key);
        let snippet = self.ensure_snippet(key, code, cwd, cx);
        let can_run = snippet.can_run_original(cx);
        let folder = std::path::Path::new(&snippet.cwd)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&snippet.cwd);
        let destination = format!("Local · {folder}");
        let menu = snippet.menu.as_ref().map(|(menu, _)| menu.clone());
        let run_key = key.to_owned();
        let keyboard_key = key.to_owned();
        let pointer_key = key.to_owned();
        let owner = cx.entity().downgrade();
        let trigger =
            transcript_action("snippet-options", "More command actions", false, window, cx)
                .icon(action_icon(if copied { "check" } else { "more" }))
                .icon_only()
                .with_focus_handle(snippet.menu_focus.clone())
                .debug_selector(|| "snippet-options".into())
                .on_key_down(
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        if action_key(event) && !event.is_held {
                            this.toggle_snippet_menu(&keyboard_key, window, cx);
                            cx.stop_propagation();
                        }
                    }),
                );
        let options = Popover::new(format!("{key}:options"))
            .anchor(Anchor::TopRight)
            .appearance(false)
            .overlay_closable(false)
            .open(menu.is_some())
            .on_open_change(move |open, window, cx| {
                let _ = owner.update(cx, |view, cx| {
                    if *open
                        != view
                            .snippets
                            .get(&pointer_key)
                            .is_some_and(|snippet| snippet.menu.is_some())
                    {
                        view.toggle_snippet_menu(&pointer_key, window, cx);
                    }
                });
            })
            .trigger(trigger)
            .content(move |_, _, _| {
                menu.clone()
                    .map_or_else(|| div().into_any_element(), |menu| menu.into_any_element())
            });
        div()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(rpx(SPACE_SM))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(rpx(SPACE_SM))
                    .text_size(rpx(TEXT_SMALL))
                    .text_color(c::FG_DIM())
                    .child(crate::icons::icon("folder", ICON_SM, c::FG_DIM()))
                    .child(div().min_w_0().truncate().child(destination)),
            )
            .child(bound_action(
                transcript_action("codex-run-code", "Run", false, window, cx)
                    .with_focus_handle(snippet.run_focus.clone())
                    .debug_selector(|| "codex-run-code".into())
                    .icon(action_icon("play")),
                can_run,
                cx,
                move |this, window, cx| this.request_snippet_run(&run_key, false, window, cx),
            ))
            .child(options)
            .into_any_element()
    }

    fn edit_snippet(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.transcript_only() || self.snippet_source_signature(key, cx).is_none() {
            return;
        }

        let Some(snippet) = self.snippets.get_mut(key) else {
            return;
        };
        if snippet.review.is_some()
            || snippet.editor.is_some()
            || snippet
                .runs
                .last()
                .is_some_and(|run| !run.terminal.read(cx).has_exited())
        {
            return;
        }
        snippet.cwd_available = std::path::Path::new(&snippet.cwd).is_dir();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .auto_grow(3, 10)
                .default_value(snippet.source.clone())
        });
        snippet.editor_observer = Some(cx.observe(&input, |_, _, cx| cx.notify()));
        input.focus_handle(cx).focus(window, cx);
        snippet.editor = Some(input);
        cx.notify();
    }

    fn request_snippet_run(
        &mut self,
        key: &str,
        confirmed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transcript_only() {
            return;
        }
        let signature = self.snippet_source_signature(key, cx);
        let runtime = self.script_runtime.clone();
        let Some(snippet) = self.snippets.get_mut(key) else {
            return;
        };
        if signature.is_none() || signature != snippet.source_signature {
            snippet.error =
                Some("This script's response has changed. Use the current response.".into());
            cx.notify();
            return;
        }
        if confirmed && snippet.review.is_none() {
            return;
        }
        if snippet
            .runs
            .last()
            .is_some_and(|run| !run.terminal.read(cx).has_exited())
        {
            return;
        }
        // Availability is a last observation for footer copy, never an action
        // gate: an external process may create or remove this directory while
        // the transcript is idle. Recheck on activation, not on every frame.
        snippet.cwd_available = std::path::Path::new(&snippet.cwd).is_dir();
        let command = snippet.review.clone().unwrap_or_else(|| {
            snippet.editor.as_ref().map_or_else(
                || snippet.source.clone(),
                |input| input.read(cx).value().to_string(),
            )
        });
        if command.trim().is_empty() {
            return;
        }
        if needs_terminal_review(&command) && !confirmed {
            snippet.review = Some(command);
            snippet.review_focus.focus(window, cx);
            cx.notify();
            return;
        }
        let target = fs_err::canonicalize(&snippet.cwd)
            .map_err(|error| format!("Execution directory unavailable: {error}"));
        let target = match target.and_then(|path| {
            if let Some(error) = crate::entities::codex_session::workspace_restore_error(
                std::slice::from_ref(&path),
                None,
            ) {
                Err(error)
            } else if !path.is_dir() {
                Err("Choose an execution directory.".into())
            } else {
                Ok(path)
            }
        }) {
            Ok(target) => target.to_string_lossy().into_owned(),
            Err(error) => {
                snippet.error = Some(error);
                cx.notify();
                return;
            }
        };
        let project_service = runtime.as_ref().and_then(|runtime| {
            grove_core::session_meta::project_owner(
                &cx.global::<crate::settings::SettingsState>().store.projects,
                &target,
            )
            .ok()
            .map(|project| (runtime.read(cx).projects.clone(), project.path.clone()))
        });
        if let Some((projects, project)) = &project_service {
            if projects.read(cx).is_removing(project)
                || projects.read(cx).is_worktree_removing(&target)
            {
                snippet.error =
                    Some("This checkout is being removed. Choose an available directory.".into());
                cx.notify();
                return;
            }
        }
        let terminal = cx.new(|cx| TerminalSession::spawn_script(&command, &target, cx));
        if let Some(error) = terminal.read(cx).spawn_error() {
            snippet.error = Some(format!("Could not start script: {error}"));
            cx.notify();
            return;
        }
        if let Some((projects, project)) = project_service {
            projects.update(cx, |projects, _| {
                projects.track_inline_script(project, target.clone(), &terminal);
            });
        }
        let view = cx.new(|cx| TerminalView::new(terminal.clone(), cx));
        let observer = cx.observe(&terminal, |_, _, cx| cx.notify());
        // Ordinary output is hidden by default. Explicit terminal continuation
        // opens the real PTY so authentication never goes through a chat field.
        let toggle_focus = cx.focus_handle();
        if confirmed {
            view.focus_handle(cx).focus(window, cx);
        } else {
            toggle_focus.focus(window, cx);
        }
        snippet.runs.push(SnippetRun {
            command,
            cwd: target,
            terminal,
            view,
            expanded: confirmed,
            stop_requested: false,
            reference_key: format!("command-output-{}:", new_reference_id()),
            toggle_focus,
            _observer: observer,
        });
        snippet.review = None;
        snippet.error = None;
        snippet.editor = None;
        snippet.editor_observer = None;
        cx.notify();
    }

    fn choose_snippet_directory(
        &mut self,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let signature = self.snippet_source_signature(&key, cx);
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose execution directory".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.first() else { return };
            let _ = this.update_in(cx, |this, _, cx| {
                if this.snippet_source_signature(&key, cx) != signature {
                    return;
                }
                if let Some(snippet) = this.snippets.get_mut(&key) {
                    // A custom path can wrap the footer independently of Markdown.
                    // Keep that uncommon mutable footer on the eager layout path.
                    snippet.custom_directory = true;
                    snippet.cwd = path.to_string_lossy().into_owned();
                    snippet.cwd_available = true;
                    snippet.error = None;
                    snippet.review = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn reconcile_snippets(&mut self, cx: &mut Context<Self>) {
        if !self
            .snippet_sources
            .borrow_mut()
            .needs_reconciliation(&self.session.read(cx).state)
        {
            return;
        }
        let removed = self
            .snippets
            .iter()
            .filter(|(key, snippet)| {
                self.snippet_source_signature(key, cx) != snippet.source_signature
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in removed {
            if let Some(snippet) = self.snippets.get_mut(&key) {
                for run in &mut snippet.runs {
                    if !run.terminal.read(cx).has_exited() {
                        run.terminal.update(cx, TerminalSession::stop_script);
                        run.stop_requested = true;
                    }
                }
                snippet.review = None;
                snippet.editor = None;
                snippet.editor_observer = None;
            }
            self.snippets.remove(&key);
        }
    }

    pub(super) fn prune_idle_snippets(&mut self, owners: &HashSet<String>) {
        self.snippets.retain(|key, snippet| {
            !snippet.runs.is_empty()
                || snippet.editor.is_some()
                || snippet.review.is_some()
                || snippet.error.is_some()
                || key
                    .rsplit_once(":code:")
                    .is_some_and(|(owner, _)| owners.contains(owner))
        });
    }

    fn can_reference_snippet_output(&self, cx: &App) -> bool {
        !self.transcript_only()
            && !self.dashboard_pending
            && self.plan_answer.is_none()
            && self.queue_editor.is_none()
            && self.submitted.is_none()
            && !self.edit_request_pending
            && self.pending_image_pastes == 0
            && !self.pending_attachment_picker
            && self.session.read(cx).state.saved.pending_edit.is_none()
    }

    fn reference_snippet_output(
        &mut self,
        key: &str,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_reference_snippet_output(cx) {
            return;
        }
        let signature = self.snippet_source_signature(key, cx);
        if signature.is_none()
            || self
                .snippets
                .get(key)
                .is_none_or(|snippet| snippet.source_signature != signature)
        {
            return;
        }
        let Some(run) = self
            .snippets
            .get(key)
            .and_then(|snippet| snippet.runs.get(index))
        else {
            return;
        };
        if !run.terminal.read(cx).has_exited()
            || self
                .reference
                .as_ref()
                .is_some_and(|reference| reference.id.starts_with(&run.reference_key))
        {
            return;
        }
        let thread_id = self.session.read(cx).state.saved.thread_id.clone();
        let Some(message_id) = key
            .rsplit_once(":code:")
            .and_then(|(owner, _)| owner.strip_prefix(thread_id.as_deref().unwrap_or("")))
            .and_then(|owner| owner.strip_prefix(':'))
        else {
            return;
        };
        let output = run.terminal.update(cx, |terminal, _| {
            terminal
                .selection_all_bounds()
                .and_then(|(mut start, end)| {
                    start.a_row = start.a_row.min(REFERENCED_OUTPUT_LINES - 1);
                    terminal.selection_text(start, end)
                })
                .unwrap_or_default()
                .trim_end()
                .to_owned()
        });
        let exit = run
            .terminal
            .read(cx)
            .exit_code()
            .map_or("unknown".into(), |code| code.to_string());
        let excerpt = if output.is_empty() {
            "No command output was captured."
        } else {
            &output
        };
        let result = format!(
            "{excerpt}\n\nCommand run in {} (exit {exit}):\n{}",
            run.cwd, run.command,
        );
        let reference = codex::ReferenceDraft {
            id: format!("{}{}", run.reference_key, new_reference_id()),
            thread_id,
            sources: vec![codex::ReferenceSource {
                message_id: message_id.to_owned(),
                role: COMMAND_OUTPUT_REFERENCE_ROLE.to_owned(),
            }],
            text: result,
        };
        self.set_reference(Some(reference), cx);
        self.set_composer_collapsed(false, cx);
        self.input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    pub(super) fn snippet_footer(
        &mut self,
        key: &str,
        code: &str,
        cwd: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let reference_enabled = self.can_reference_snippet_output(cx);
        let current_reference = self
            .reference
            .as_ref()
            .map(|reference| reference.id.clone());
        let snippet = self.ensure_snippet(key, code, cwd, cx);
        let mut body = div()
            .flex()
            .flex_col()
            .gap(rpx(SPACE_MD))
            .text_size(rpx(TEXT_SMALL))
            .text_color(c::FG_DIM());
        if !snippet.cwd_available {
            body = body.mt(rpx(SPACE_MD)).child(
                "Execution directory unavailable. Choose a directory from More command actions.",
            );
        }
        if let Some(error) = &snippet.error {
            body = body.child(div().text_color(c::RED()).child(error.clone()));
        }
        if let Some(editor) = &snippet.editor {
            let cancel_key = key.to_owned();
            let run_key = key.to_owned();
            body = body
                .child(
                    project_field_well(editor.focus_handle(cx).is_focused(window), false).child(
                        Input::new(editor)
                            .aria_label("Edited script")
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false)
                            .p_0(),
                    ),
                )
                .child("Runs a separate edited copy; the response stays intact.")
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(rpx(SPACE_SM))
                        .child(
                            transcript_action("snippet-cancel-edit", "Cancel", false, window, cx)
                                .icon(action_icon("close"))
                                .map(|button| {
                                    bound_action(button, true, cx, move |this, window, cx| {
                                        if let Some(snippet) = this.snippets.get_mut(&cancel_key) {
                                            snippet.editor = None;
                                            snippet.editor_observer = None;
                                            snippet.review = None;
                                            snippet.menu_focus.focus(window, cx);
                                        }
                                        cx.notify();
                                    })
                                }),
                        )
                        .child(
                            transcript_action(
                                "snippet-run-edited",
                                "Run edited copy",
                                false,
                                window,
                                cx,
                            )
                            .icon(action_icon("play"))
                            .disabled(
                                editor.read(cx).value().trim().is_empty()
                                    || snippet.review.is_some(),
                            )
                            .map(|button| {
                                bound_action(
                                    button,
                                    !editor.read(cx).value().trim().is_empty()
                                        && snippet.review.is_none(),
                                    cx,
                                    move |this, window, cx| {
                                        this.request_snippet_run(&run_key, false, window, cx);
                                    },
                                )
                            }),
                        ),
                );
        }
        if let Some(command) = &snippet.review {
            let cancel_key = key.to_owned();
            let confirm_key = key.to_owned();
            body = body.child(div().border_t_1().border_color(c::BORDER()).pt(rpx(SPACE_LG))
                .child(div().text_color(c::YELLOW()).child("Review privileged script"))
                .child("This script uses sudo and can change this machine. Authentication and transaction confirmation stay in the terminal.")
                .child(div().font_family(MONO_FAMILY).text_color(c::FG()).child(command.clone()))
                .child(div().flex().flex_wrap().gap(rpx(SPACE_SM))
                    .child(transcript_action("snippet-cancel-review", "Cancel", false, window, cx).icon(action_icon("close"))
                        .map(|button| bound_action(button, true, cx, move |this, window, cx| { if let Some(snippet) = this.snippets.get_mut(&cancel_key) { snippet.review = None; snippet.editor.as_ref().map_or_else(|| snippet.run_focus.clone(), |input| input.focus_handle(cx)).focus(window, cx); } cx.notify(); })))
                    .child(transcript_action("snippet-confirm-review", "Continue in terminal", false, window, cx).with_focus_handle(snippet.review_focus.clone()).icon(action_icon("terminal"))
                        .map(|button| bound_action(button, true, cx, move |this, window, cx| this.request_snippet_run(&confirm_key, true, window, cx))))));
        }
        for (index, run) in snippet.runs.iter().enumerate() {
            let terminal = run.terminal.read(cx);
            let done = terminal.has_exited();
            let (icon, color, label) = if !done {
                (
                    "status-working",
                    c::STATUS_INFO(),
                    if run.stop_requested {
                        "Stopping".into()
                    } else {
                        "Running locally".into()
                    },
                )
            } else if run.stop_requested && terminal.exit_code() != Some(0) {
                ("stop", c::FG_DIM(), "Stopped".into())
            } else {
                match terminal.exit_code() {
                    Some(0) => ("status-success", c::STATUS_SUCCESS(), "Exit 0".into()),
                    Some(code) => (
                        "status-failed",
                        c::STATUS_ERROR(),
                        format!("Failed · exit {code}"),
                    ),
                    None => (
                        "status-unknown",
                        c::FG_DIM(),
                        "Closed · exit status unavailable".into(),
                    ),
                }
            };
            let toggle_key = key.to_owned();
            let reference_key = key.to_owned();
            let stop_key = key.to_owned();
            let mut actions = div()
                .flex()
                .flex_shrink_0()
                .ml_auto()
                .gap(rpx(SPACE_SM))
                .child(
                    transcript_action(
                        ("snippet-toggle", index),
                        if run.expanded {
                            "Hide output"
                        } else {
                            "Output"
                        },
                        false,
                        window,
                        cx,
                    )
                    .with_focus_handle(run.toggle_focus.clone())
                    .aria_expanded(run.expanded)
                    .debug_selector(move || format!("snippet-output-{index}"))
                    .icon(action_icon("terminal"))
                    .map(|button| {
                        bound_action(button, true, cx, move |this, window, cx| {
                            if let Some(run) = this
                                .snippets
                                .get_mut(&toggle_key)
                                .and_then(|s| s.runs.get_mut(index))
                            {
                                run.expanded = !run.expanded;
                                run.toggle_focus.focus(window, cx);
                            }
                            cx.notify();
                        })
                    }),
                );
            let is_referenced = current_reference
                .as_ref()
                .is_some_and(|id| id.starts_with(&run.reference_key));
            let reference_output = (done && run.expanded).then(|| {
                let label = if is_referenced {
                    "Output referenced"
                } else if current_reference.is_some() {
                    "Replace reference with output"
                } else {
                    "Reference output"
                };
                let enabled = reference_enabled && !is_referenced;
                let button =
                    transcript_action(("snippet-reference", index), label, false, window, cx)
                        .debug_selector(move || format!("snippet-reference-{index}"))
                        .icon(action_icon(if is_referenced { "check" } else { "reply" }))
                        .disabled(!enabled);
                bound_action(button, enabled, cx, move |this, window, cx| {
                    this.reference_snippet_output(&reference_key, index, window, cx);
                })
            });
            if !done {
                actions = actions.child(
                    transcript_action(("snippet-stop", index), "Stop", false, window, cx)
                        .debug_selector(move || format!("snippet-stop-{index}"))
                        .icon(action_icon("stop"))
                        .disabled(run.stop_requested)
                        .map(|button| {
                            bound_action(button, !run.stop_requested, cx, move |this, _, cx| {
                                if let Some(snippet) = this.snippets.get_mut(&stop_key) {
                                    if let Some(run) = snippet.runs.get_mut(index) {
                                        match run
                                            .terminal
                                            .update(cx, TerminalSession::request_script_stop)
                                        {
                                            Ok(()) => run.stop_requested = true,
                                            Err(error) => snippet.error = Some(error),
                                        }
                                    }
                                }
                                cx.notify();
                            })
                        }),
                );
            }
            body = body.child(
                div()
                    .border_t_1()
                    .border_color(c::BORDER())
                    .mt(rpx(SPACE_SM))
                    .pt(rpx(SPACE_SM))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(rpx(SPACE_LG))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(rpx(SPACE_SM))
                                    .text_color(color)
                                    .child(status::mark(
                                        format!("snippet-{key}-{index}"),
                                        match icon {
                                            "status-working" => StatusKind::Working,
                                            "status-success" => StatusKind::Success,
                                            "status-failed" => StatusKind::Failed,
                                            "stop" => StatusKind::Stopped,
                                            _ => StatusKind::Unknown,
                                        },
                                        ICON_SM,
                                        cx,
                                    ))
                                    .min_w_0()
                                    .child(label)
                                    .when(snippet.runs.len() > 1 && !run.expanded, |status| {
                                        status.child(
                                            div()
                                                .text_color(c::FG_DIM())
                                                .child(format!("· {}", index + 1)),
                                        )
                                    }),
                            )
                            .child(actions),
                    )
                    .when(run.expanded, |row| {
                        row.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .justify_between()
                                .gap(rpx(SPACE_SM))
                                .child(format!(
                                    "Attempt {}{}",
                                    index + 1,
                                    if run.command == snippet.source {
                                        ""
                                    } else {
                                        " · edited copy"
                                    },
                                ))
                                .children(reference_output),
                        )
                        .when(run.cwd != snippet.cwd, |row| {
                            row.child(
                                div()
                                    .text_color(c::FG_DIM())
                                    .child(format!("Run in {}", run.cwd)),
                            )
                        })
                        .when(run.command != snippet.source, |row| {
                            row.child(
                                div()
                                    .font_family(MONO_FAMILY)
                                    .text_color(c::FG())
                                    .child(run.command.clone()),
                            )
                        })
                        .child(
                            div()
                                .mt(rpx(SPACE_MD))
                                .h(rpx(OUTPUT_HEIGHT))
                                .id(("snippet-terminal", index))
                                .debug_selector(move || format!("snippet-terminal-{index}"))
                                .w_full()
                                .bg(c::BG())
                                .child(run.view.clone()),
                        )
                    }),
            );
        }
        // PTY output and its viewport resize immediately; animating the cell
        // grid would disturb input/selection. The shared status mark owns motion.
        let escape_key = key.to_owned();
        body.on_key_down(
            cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    if let Some(snippet) = this.snippets.get_mut(&escape_key) {
                        if snippet.review.take().is_some() {
                            snippet
                                .editor
                                .as_ref()
                                .map_or_else(
                                    || snippet.run_focus.clone(),
                                    |input| input.focus_handle(cx),
                                )
                                .focus(window, cx);
                        } else if snippet.editor.take().is_some() {
                            snippet.editor_observer = None;
                            snippet.menu_focus.focus(window, cx);
                        } else {
                            return;
                        }
                        cx.notify();
                        cx.stop_propagation();
                    }
                }
            }),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::fixture;
    use super::*;
    use serde_json::json;

    #[test]
    fn source_index_reuses_unchanged_responses_and_invalidates_edits() {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let mut state = super::super::tests::snapshot();
            state.saved.provider = provider;
            state.saved.thread_id = Some("thread:with:colons".into());
            state.saved.items = vec![
                json!({"id":"answer:one", "type":"agentMessage", "status":"completed", "text":"first"}),
                json!({"id":"answer:two", "type":"agentMessage", "groveCompleted":true, "text":"second"}),
            ].into();
            let mut sources = SnippetSources::default();
            sources.refresh(&state);
            assert!(sources
                .sources
                .values()
                .all(|source| source.signature.get().is_none()));
            let key = "thread:with:colons:answer:one:code:0:hash";
            let initial = sources.signature(&state, key).unwrap();
            let retained = sources.sources["answer:one"].clone();
            assert!(sources.needs_reconciliation(&state));
            state.model = Some("metadata-only notification".into());
            assert!(!sources.needs_reconciliation(&state));
            state.saved.items[1]["text"] = json!("changed other response");
            assert!(sources.needs_reconciliation(&state));
            assert_eq!(sources.signature(&state, key), Some(initial));
            assert!(Rc::ptr_eq(&retained, &sources.sources["answer:one"]));
            assert!(sources.sources["answer:two"].signature.get().is_none());
            state.saved.items[0]["text"] = json!("changed source");
            assert_ne!(sources.signature(&state, key), Some(initial));
            state.saved.items[0]["status"] = json!("inProgress");
            assert_eq!(sources.signature(&state, key), None);
            state.saved.items[0]["status"] = json!("completed");
            state.saved.thread_id = Some("another-thread".into());
            assert_eq!(sources.signature(&state, key), None);
            assert!(sources
                .signature(&state, "another-thread:answer:one:code:0:hash")
                .is_some());
        }
    }

    #[test]
    fn source_index_retains_idless_fallback_and_first_completed_duplicate() {
        let mut state = super::super::tests::snapshot();
        state.saved.thread_id = None;
        state.saved.items = vec![
            json!({"type":"agentMessage", "status":"completed", "text":"idless"}),
            json!({"id":"duplicate", "type":"agentMessage", "status":"inProgress", "text":"pending"}),
            json!({"id":"duplicate", "type":"agentMessage", "status":"completed", "text":"first completed"}),
            json!({"id":"duplicate", "type":"agentMessage", "status":"completed", "text":"later"}),
        ].into();
        let mut sources = SnippetSources::default();
        assert!(sources
            .signature(&state, ":message-0:code:0:hash")
            .is_some());
        assert!(sources
            .signature(&state, ":duplicate:code:0:hash")
            .is_some());
        assert_eq!(sources.sources["duplicate"].item["text"], "first completed");
        state.saved.items.clear();
        assert!(sources.needs_reconciliation(&state));
        assert_eq!(sources.signature(&state, ":message-0:code:0:hash"), None);
    }

    fn prepare(
        view: &mut CodexView,
        command: &str,
        provider: codex::Provider,
        cwd: &str,
        window: &mut Window,
        cx: &mut Context<CodexView>,
    ) -> String {
        cx.set_global(crate::zoom::ZoomState::new(1.));
        cx.set_global(c::ThemeState::new(
            false,
            "grove-dark".into(),
            "grove-light".into(),
        ));
        view.session.update(cx, |session, cx| {
            session.state.saved.provider = provider;
            session.state.saved.cwd = cwd.into();
            session.state.saved.items = vec![json!({"id":"inline-fixture","type":"agentMessage","groveCompleted":true,"text":format!("```sh\n{command}\n```")})].into();
            cx.notify();
        });
        let owner = format!(
            "{}:inline-fixture",
            view.session
                .read(cx)
                .state
                .saved
                .thread_id
                .as_deref()
                .unwrap_or("")
        );
        let key = key(&owner, 0, command, &format!("```sh\n{command}\n```"));
        view.ensure_snippet(&key, command, cwd, cx);
        view.input
            .update(cx, |input, cx| input.set_value("keep my draft", window, cx));
        key
    }

    #[gpui::test]
    #[cfg(unix)]
    fn inline_snippet_edit_failure_sharing_and_source_invalidation(cx: &mut gpui::TestAppContext) {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let (view, cx, actions, _updates) = fixture(cx);
            let key = cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    let key = prepare(view, "printf original", provider, "/tmp", window, cx);
                    view.edit_snippet(&key, window, cx);
                    let snippet = &view.snippets[&key];
                    snippet
                        .editor
                        .as_ref()
                        .unwrap()
                        .update(cx, |input, cx| input.set_value("exit 7", window, cx));
                    assert_eq!(view.input.read(cx).value().as_ref(), "keep my draft");
                    view.request_snippet_run(&key, false, window, cx);
                    let snippet = &view.snippets[&key];
                    assert_eq!(snippet.source, "printf original");
                    assert_eq!(snippet.runs[0].command, "exit 7");
                    assert!(!snippet.runs[0].expanded);
                    key
                })
            });
            let terminal =
                view.read_with(cx, |view, _| view.snippets[&key].runs[0].terminal.clone());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while terminal.update(cx, |terminal, _| terminal.alive()) {
                cx.run_until_parked();
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    assert_eq!(terminal.read(cx).exit_code(), Some(7));
                    terminal.update(cx, |terminal, cx| {
                        terminal.ingest_for_test(b"fixture output\r\n", cx);
                    });
                    view.reference_snippet_output(&key, 0, window, cx);
                    assert_eq!(view.input.read(cx).value().as_ref(), "keep my draft");
                    let reference = view.reference.clone().unwrap();
                    assert_eq!(reference.sources[0].role, COMMAND_OUTPUT_REFERENCE_ROLE);
                    assert!(reference.text.contains("exit 7"));
                    assert!(reference.text.starts_with("fixture output"));
                    view.reference_snippet_output(&key, 0, window, cx);
                    assert_eq!(view.reference.as_ref(), Some(&reference));
                    view.remove_reference(window, cx);
                    view.reference_snippet_output(&key, 0, window, cx);
                    assert_ne!(view.reference.as_ref().unwrap().id, reference.id);
                    assert_eq!(view.input.read(cx).value().as_ref(), "keep my draft");
                    view.session.update(cx, |session, _| {
                        session.state.saved.thread_id = Some("another-thread".into());
                    });
                    view.request_snippet_run(&key, false, window, cx);
                    assert_eq!(view.snippets[&key].runs.len(), 1);
                    assert!(view.snippets[&key]
                        .error
                        .as_ref()
                        .unwrap()
                        .contains("response has changed"));
                });
            });
            assert!(
                !actions.try_iter().any(|action| matches!(
                    action,
                    codex::Action::Submit { .. }
                        | codex::Action::SubmitWithMode { .. }
                        | codex::Action::Prompt(_)
                        | codex::Action::PromptWithImages { .. }
                )),
                "Referencing output does not submit a message"
            );
        }
    }

    #[gpui::test]
    #[cfg(unix)]
    fn inline_snippet_directory_creation_can_recover_through_projected_run(
        cx: &mut gpui::TestAppContext,
    ) {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let root = tempfile::tempdir().unwrap();
            let directory = root.path().join("created-later");
            let (view, cx, actions, _updates) = fixture(cx);
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    let key = prepare(
                        view,
                        "exit 0",
                        provider,
                        directory.to_str().unwrap(),
                        window,
                        cx,
                    );
                    assert!(!view.snippets[&key].cwd_available);
                    assert_eq!(view.snippet_projection_controls(&key, cx).len(), 2);
                    assert!(!view.snippet_requires_eager(&key));
                    // No item revision or render occurs between the directory
                    // appearing and activation of its original offscreen action.
                    fs_err::create_dir(&directory).unwrap();
                    view.activate_snippet_control(&key, SnippetControl::Run, window, cx);
                    let snippet = &view.snippets[&key];
                    assert!(snippet.cwd_available);
                    assert!(snippet.error.is_none());
                    assert_eq!(snippet.runs.len(), 1);
                    assert_eq!(
                        snippet.runs[0].terminal.read(cx).initial_cwd(),
                        directory.to_str()
                    );
                    assert!(view.snippet_requires_eager(&key));
                    assert_eq!(view.input.read(cx).value().as_ref(), "keep my draft");
                });
            });
            assert!(!actions.try_iter().any(|action| matches!(
                action,
                codex::Action::Submit { .. }
                    | codex::Action::SubmitWithMode { .. }
                    | codex::Action::Prompt(_)
                    | codex::Action::PromptWithImages { .. }
            )));
        }
    }

    #[gpui::test]
    #[cfg(unix)]
    fn inline_snippet_review_duplicate_stop_and_missing_directory(cx: &mut gpui::TestAppContext) {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let (view, cx, _actions, _updates) = fixture(cx);
            let key = cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    // Literal sudo tests review without requesting real privileges.
                    let key = prepare(view, "printf sudo; sleep 30", provider, "/tmp", window, cx);
                    view.request_snippet_run(&key, false, window, cx);
                    assert!(view.snippets[&key].runs.is_empty());
                    assert!(view.snippets[&key].review.is_some());
                    view.request_snippet_run(&key, true, window, cx);
                    assert_eq!(view.snippets[&key].runs.len(), 1);
                    assert!(view.snippets[&key].runs[0].expanded);
                    view.request_snippet_run(&key, false, window, cx);
                    assert_eq!(
                        view.snippets[&key].runs.len(),
                        1,
                        "Duplicate launch rejected"
                    );
                    view.snippets[&key].runs[0]
                        .terminal
                        .update(cx, TerminalSession::request_script_stop)
                        .unwrap();
                    key
                })
            });
            let terminal =
                view.read_with(cx, |view, _| view.snippets[&key].runs[0].terminal.clone());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !terminal.read_with(cx, |terminal, _| terminal.has_exited()) {
                assert!(std::time::Instant::now() < deadline);
                cx.run_until_parked();
                cx.executor()
                    .advance_clock(std::time::Duration::from_millis(10));
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    let key = prepare(
                        view,
                        "printf never",
                        provider,
                        "/grove-inline-fixture-does-not-exist",
                        window,
                        cx,
                    );
                    view.request_snippet_run(&key, false, window, cx);
                    assert!(view.snippets[&key].runs.is_empty());
                    assert!(view.snippets[&key]
                        .error
                        .as_ref()
                        .unwrap()
                        .contains("directory unavailable"));
                    view.snippets.get_mut(&key).unwrap().cwd = "/tmp".into();
                    view.request_snippet_run(&key, false, window, cx);
                    assert_eq!(
                        view.snippets[&key].runs.len(),
                        1,
                        "Resolved directory can run"
                    );
                    view.snippets[&key].runs[0]
                        .terminal
                        .update(cx, TerminalSession::stop_script);
                });
            });
        }
    }

    #[gpui::test]
    #[cfg(unix)]
    fn inline_snippet_source_removal_stops_owned_process(cx: &mut gpui::TestAppContext) {
        for provider in [codex::Provider::Codex, codex::Provider::Claude] {
            let (view, cx, _actions, _updates) = fixture(cx);
            let terminal = cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    let key = prepare(view, "sleep 30", provider, "/tmp", window, cx);
                    view.request_snippet_run(&key, false, window, cx);
                    let terminal = view.snippets[&key].runs[0].terminal.clone();
                    assert!(!terminal.read(cx).has_exited());
                    view.session.update(cx, |session, _| {
                        session.state.saved.items = Vec::new().into();
                    });
                    view.reconcile_snippets(cx);
                    assert!(
                        !view.snippets.contains_key(&key),
                        "removed source releases its snippet state"
                    );
                    terminal
                })
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !terminal.read_with(cx, |terminal, _| terminal.has_exited()) {
                cx.run_until_parked();
                assert!(
                    std::time::Instant::now() < deadline,
                    "cancelled startup must acknowledge before reporting exit"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn inline_snippet_identity_includes_source_and_offset() {
        assert_ne!(
            key("thread:answer", 0, "echo a", "old response"),
            key("thread:answer", 0, "echo a", "revised response")
        );
        assert_ne!(
            key("thread:answer", 0, "echo a", "response"),
            key("thread:answer", 0, "echo b", "response")
        );
        assert_ne!(
            key("thread:answer", 0, "echo a", "response"),
            key("thread:answer", 10, "echo a", "response")
        );
        assert_ne!(
            key("thread:answer", 0, "echo a", "response"),
            key("other:answer", 0, "echo a", "response")
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
mod capture {
    use super::super::tests::snapshot;
    use super::*;
    use grove_core::codex::{Connection, Handle};
    use serde_json::json;
    use std::sync::mpsc;

    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    #[test]
    #[ignore = "opens isolated native inline-script QA window; requires X11 and import"]
    fn inline_snippet_native_capture() {
        assert!(std::env::var_os("GROVE_CONFIG_DIR").is_some());
        let output = std::env::var("GROVE_SNIPPET_CAPTURE_DIR").unwrap();
        fs_err::create_dir_all(&output).unwrap();
        gpui_platform::application().with_assets(crate::assets::Assets).run(move |cx| {
            gpui_component::init(cx);
            crate::fonts::register(cx).unwrap();
            cx.set_global(crate::settings::SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::ZoomState::new(1.));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(c::ThemeState::new(false, "grove-dark".into(), "grove-light".into()));
            cx.set_reduce_motion(true);
            grove_core::theme::set_by_name("grove-dark");
            c::sync_component_theme(cx);
            let (handle, actions) = Handle::channel();
            let (updates, receive) = mpsc::channel();
            let connection = Connection { handle, updates: receive.into() };
            let mut retained = None;
            let window = cx.open_window(gpui::WindowOptions {
                app_id: Some("dev.grove.inline-script-qa".into()),
                kind: gpui::WindowKind::PopUp,
                window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(gpui::point(gpui::px(40.), gpui::px(40.)), gpui::size(gpui::px(980.), gpui::px(800.))))),
                ..Default::default()
            }, |window, cx| {
                window.set_window_title("Grove inline script QA");
                let mut state = snapshot();
                state.saved.cwd = "/tmp".into();
                state.saved.draft = "Keep my conversation draft".into();
                state.saved.items = vec![json!({"type":"agentMessage","id":"snippet-capture","groveCompleted":true,"text":"- Check the workspace before continuing.\n\n  ```sh\n  printf 'Working tree clean\\n'\n  ```\n\nThe next paragraph follows the complete command card."})].into();
                let session = cx.new(|cx| CodexSession::from_connection(state, connection, cx));
                let view = cx.new(|cx| CodexView::new(session, window, cx));
                view.update(cx, |view, _| view.next_reply_generate = |_, _| Ok(Vec::new()));
                retained = Some(view.clone());
                cx.new(|cx| gpui_component::Root::new(view, window, cx).bordered(false))
            }).unwrap();
            let view = retained.unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let _transport = (actions, updates);
                cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                // Use a real process; native text is a fixture because libtest
                // deliberately disables the foreign-thread PTY reader bridge.
                cx.update_window(window.into(), |_, window, cx| view.update(cx, |view, cx| {
                    let key = view.snippets.keys().next().unwrap().clone();
                    view.request_snippet_run(&key, false, window, cx);
                })).unwrap();
                cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
                cx.update_window(window.into(), |_, _, cx| view.update(cx, |view, cx| {
                    let terminal = view.snippets.values().next().unwrap().runs[0].terminal.clone();
                    terminal.update(cx, |terminal, cx| { assert!(!terminal.alive()); terminal.ingest_for_test(b"Working tree clean\r\n", cx); });
                    cx.notify();
                })).unwrap();
                let capture_reference = std::env::var_os("GROVE_SNIPPET_REFERENCE_CAPTURE").is_some();
                for (name, provider, theme, width, rem, expanded) in [
                    ("codex-dark-collapsed", codex::Provider::Codex, "grove-dark", 980., 16., false),
                    ("claude-light-narrow", codex::Provider::Claude, "grove-light", 540., 16., false),
                    ("codex-dark-expanded", codex::Provider::Codex, "grove-dark", 980., 16., true),
                    ("claude-light-zoom", codex::Provider::Claude, "grove-light", 760., 20., true),
                ] {
                    if capture_reference && !expanded {
                        continue;
                    }
                    cx.update(|cx| { grove_core::theme::set_by_name(theme); c::sync_component_theme(cx); cx.set_global(crate::zoom::ZoomState::new(rem / 16.)); });
                    cx.update_window(window.into(), |_, window, cx| {
                        window.resize(gpui::size(gpui::px(width), gpui::px(800.)));
                        window.set_rem_size(gpui::px(rem));
                        view.update(cx, |view, cx| {
                            view.session.update(cx, |session, cx| { session.state.saved.provider = provider; cx.notify(); });
                            view.snippets.values_mut().next().unwrap().runs[0].expanded = expanded;
                            view.smooth_follow = false;
                            view.set_composer_collapsed(true, cx);
                            if capture_reference {
                                let key = view.snippets.keys().next().unwrap().clone();
                                view.reference_snippet_output(&key, 0, window, cx);
                                assert!(view.reference.is_some());
                            }
                            view.scroll.scroll_to_top_of_item(0);
                            cx.notify();
                        });
                        window.refresh();
                    }).unwrap();
                    if let Some(helper) = std::env::var_os("GROVE_SNIPPET_EXPOSE_HELPER") {
                        assert!(std::process::Command::new(helper).arg("Grove inline script QA").status().unwrap().success());
                    }
                    cx.background_executor().timer(std::time::Duration::from_millis(650)).await;
                    cx.update_window(window.into(), |_, window, cx| { let _ = window.draw(cx); }).unwrap();
                    let path = format!("{output}/{name}.png");
                    assert!(std::process::Command::new("import").args(["-window", "Grove inline script QA", &path]).status().unwrap().success());
                    assert!(image::open(&path).unwrap().to_rgb8().pixels().any(|pixel| pixel.0.iter().any(|channel| *channel > 32)), "Native capture was blank");
                    eprintln!("Native snippet capture: {path}");
                }
                cx.update_window(window.into(), |_, window, cx| { window.blur(); window.replace_root(cx, |_, _| Empty); window.refresh(); }).unwrap();
                drop(view);
                cx.background_executor().timer(std::time::Duration::from_millis(150)).await;
                cx.update_window(window.into(), |_, window, cx| { let _ = window.draw(cx); }).unwrap();
                cx.background_executor().timer(std::time::Duration::from_millis(150)).await;
                cx.update_window(window.into(), |_, window, _| window.remove_window()).unwrap();
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                cx.update(|cx| cx.quit());
            }).detach();
        });
    }
}
