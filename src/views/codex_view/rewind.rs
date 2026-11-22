//! Conversation rewind confirmation and saved draft recovery.

use super::*;

impl CodexView {
    pub(in crate::views) fn edit_confirmation_open(&self) -> bool {
        self.edit_confirmation.is_some()
    }

    pub(super) fn sync_composer_before_rewind(&self, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        let images = self.images.clone();
        let reference = self.reference.clone();
        self.session.update(cx, |session, cx| {
            session.send(Action::Draft(text), cx);
            session.send(Action::DraftImages(images), cx);
            session.send(Action::DraftReference(reference), cx);
        });
    }

    pub(super) fn load_restored_composer(
        &mut self,
        state: &codex::Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.images.clone_from(&state.saved.draft_images);
        self.previews = self
            .images
            .iter()
            .filter_map(|url| image_from_url(url).map(|image| (url.clone(), image)))
            .collect();
        self.reference.clone_from(&state.saved.draft_reference);
        self.reference_expanded = false;
        self.picked_tokens.clear();
        self.input.update(cx, |input, cx| {
            input.set_value(state.saved.draft.clone(), window, cx);
        });
        self.input.focus_handle(cx).focus(window, cx);
    }

    fn cancel_rewind(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(target) = self.edit_confirmation.take() {
            window.focus(&target.source_focus, cx);
        }
        cx.notify();
    }

    fn rewind_files_unavailable(&self, index: usize, cx: &App) -> Option<String> {
        let state = &self.session.read(cx).state;
        if let Some(reason) = state.rewind_files_unavailable(index) {
            return Some(reason);
        }
        let Some(runtime) = &self.script_runtime else {
            return Some(
                "Workspace ownership is unavailable. You can rewind and keep the files.".into(),
            );
        };
        let registry = runtime.read(cx).registry.read(cx);
        let roots = state.rewind_target(index).ok()?.files?.roots().clone();
        for other in registry.all() {
            if registry
                .codex(other.id)
                .is_some_and(|session| *session == self.session)
            {
                continue;
            }
            for path in std::iter::once(other.wt_path.as_str())
                .chain(other.context_roots.iter().map(|root| root.wt_path.as_str()))
            {
                let Ok(path) = std::path::Path::new(path).canonicalize() else {
                    continue;
                };
                let other_roots =
                    grove_core::checkpoint::repository_roots(std::slice::from_ref(&path))
                        .unwrap_or_else(|_| vec![path]);
                if roots.iter().any(|root| {
                    other_roots
                        .iter()
                        .any(|path| path.starts_with(root) || root.starts_with(path))
                }) {
                    return Some(
                        "Another session uses this worktree. You can rewind and keep the files."
                            .into(),
                    );
                }
            }
        }
        None
    }

    fn commit_rewind(&mut self, restore_files: bool, cx: &mut Context<Self>) {
        let Some(target) = self.edit_confirmation.as_ref() else {
            return;
        };
        let state = &self.session.read(cx).state;
        let error = if target.thread_id != state.saved.thread_id {
            Some("This conversation changed; select Edit again.".to_string())
        } else if restore_files {
            self.rewind_files_unavailable(target.index, cx)
        } else {
            None
        };
        if let Some(error) = error {
            self.session.update(cx, |session, cx| {
                session.state.error = Some(error);
                cx.notify();
            });
            return;
        }
        let reservation = if restore_files {
            let roots = state
                .rewind_target(target.index)
                .ok()
                .and_then(|target| target.files)
                .map(|files| files.roots())
                .unwrap_or_default();
            match grove_core::checkpoint::acquire_restore(&roots) {
                Ok(reservation) => Some(reservation),
                Err(error) => {
                    self.session.update(cx, |session, cx| {
                        session.state.error = Some(error);
                        cx.notify();
                    });
                    return;
                }
            }
        } else {
            None
        };
        let Some(target) = self.edit_confirmation.take() else {
            return;
        };
        self.sync_composer_before_rewind(cx);
        self.edit_request_pending = true;
        self.rewind_attempt = Some(self.session.read(cx).state.rewind_attempts);
        self.session.update(cx, |session, cx| {
            session.state.clear_error();
            let action = if let Some(reservation) = reservation {
                Action::RewindMessageReserved {
                    index: target.index,
                    message_id: target.message_id,
                    reservation,
                }
            } else {
                Action::RewindMessage {
                    index: target.index,
                    message_id: target.message_id,
                    restore_files: false,
                }
            };
            session.send(action, cx);
        });
        cx.notify();
    }

    pub(super) fn render_edit_confirmation(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let pane = Rc::new(Cell::new(gpui::Bounds::default()));
        let measure_pane = pane.clone();
        let Some(target) = self.edit_confirmation.as_ref() else {
            return div().into_any_element();
        };
        let source = target.source_bounds.clone();
        let paint_source = source.clone();
        let source_viewport = self.scroll.clone();
        let unavailable = target.files_unavailable.clone();
        let restore_disabled = unavailable.is_some();
        let dialog = div()
            .id("codex-edit-confirmation")
            .debug_selector(|| "codex-edit-confirmation".into())
            .role(gpui::Role::Dialog)
            .aria_label("edit from here?")
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.cancel_rewind(window, cx);
                    cx.stop_propagation();
                } else if event.keystroke.key == "tab" {
                    let mut handles = vec![&this.edit_cancel_focus, &this.edit_keep_focus];
                    if this.edit_confirmation.as_ref().is_some_and(|target| target.files_unavailable.is_none()) {
                        handles.push(&this.edit_confirm_focus);
                    }
                    let current = handles.iter().position(|handle| handle.is_focused(window)).unwrap_or(0);
                    let next = if event.keystroke.modifiers.shift { (current + handles.len() - 1) % handles.len() } else { (current + 1) % handles.len() };
                    window.focus(handles[next], cx);
                    cx.stop_propagation();
                }
            }))
            .flex().flex_col().gap(rpx(SPACE_LG))
            .p(rpx(SPACE_2XL)).rounded(rpx(RADIUS_GROUP))
            .border_1().border_color(c::BORDER()).bg(c::SURFACE_RAISED())
            .child(div().text_size(rpx(TEXT_TITLE)).child("edit from here?"))
            .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child(
                "This message and all later replies will be removed. The message returns to the composer for editing and sending. Your current draft will be saved separately."
            ))
            .child(div().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child(unavailable.unwrap_or_else(||
                "Restoring files also removes later changes you made in these worktrees. Ignored files, commits, and staged changes stay as they are.".into()
            )))
            .child(super::super::components::form_actions()
                .child(form_action("codex-cancel-edit", "Cancel", false, window, cx)
                    .icon(gpui_component::Icon::default().path("icons/close.svg").size(rpx(ICON_SM)))
                    .with_focus_handle(self.edit_cancel_focus.clone())
                    .debug_selector(|| "codex-cancel-edit".into())
                    .on_click(cx.listener(|this, _, window, cx| this.cancel_rewind(window, cx))))
                .child(form_action("codex-keep-files", "Keep files", false, window, cx)
                    .icon(gpui_component::Icon::default().path("icons/restore.svg").size(rpx(ICON_SM)))
                    .with_focus_handle(self.edit_keep_focus.clone())
                    .debug_selector(|| "codex-keep-files".into())
                    .on_click(cx.listener(|this, _, _, cx| this.commit_rewind(false, cx))))
                .child(form_action("codex-confirm-edit", "Restore files", true, window, cx)
                    .icon(gpui_component::Icon::default().path("icons/restore.svg").size(rpx(ICON_SM)))
                    .disabled(restore_disabled)
                    .with_focus_handle(self.edit_confirm_focus.clone())
                    .debug_selector(|| "codex-confirm-edit".into())
                    .on_click(cx.listener(|this, _, _, cx| this.commit_rewind(true, cx)))))
            .into_any_element();
        gpui::deferred(
            div()
                .id("codex-edit-confirmation-overlay")
                .debug_selector(|| "codex-edit-confirmation-overlay".into())
                .absolute()
                .inset_0()
                .occlude()
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    gpui::canvas(
                        move |bounds, _, _| {
                            measure_pane.set(bounds);
                        },
                        move |bounds, (), window, _| {
                            let aperture = paint_source
                                .get()
                                .intersect(&source_viewport.bounds())
                                .intersect(&bounds);
                            paint_confirmation_veil(bounds, aperture, window);
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(gpui::deferred(EditConfirmationTray {
                    panel: Some(dialog),
                    placed: None,
                    pane,
                    source,
                    scroll: self.scroll.clone(),
                    source_anchor: target.source_anchor.clone(),
                    owner: cx.entity().downgrade(),
                })),
        )
        .into_any_element()
    }

    pub(super) fn request_edit_item(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.source_item_index(id, cx) {
            self.request_edit(index, window, cx);
        }
    }

    pub(super) fn request_edit(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transcript_only()
            || self.edit_request_pending
            || self.queue_editor.is_some()
            || self.submitted.is_some()
            || self.pending_image_pastes > 0
            || self.pending_attachment_picker
        {
            return;
        }
        let state = &self.session.read(cx).state;
        match state.rewind_target(index) {
            Ok(_) => {
                let id = state.saved.items[index]["id"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned();
                let key = format!(
                    "{}:{id}",
                    state.saved.thread_id.as_deref().unwrap_or_default()
                );
                let source_bounds = self
                    .message_action_bounds
                    .get(&key)
                    .cloned()
                    .unwrap_or_default();
                let source_focus = self
                    .message_focus
                    .get(&format!("{key}:edit"))
                    .cloned()
                    .unwrap_or_else(|| self.input.focus_handle(cx));
                let files_unavailable = self.rewind_files_unavailable(index, cx);
                self.edit_confirmation = Some(RewindConfirmation {
                    index,
                    message_id: id,
                    thread_id: state.saved.thread_id.clone(),
                    source_bounds,
                    source_anchor: gpui::ScrollAnchor::for_handle(self.scroll.clone()),
                    source_focus,
                    files_unavailable,
                });
                self.smooth_follow = false;
                self.close_menu(cx);
                window.focus(&self.edit_cancel_focus, cx);
                cx.notify();
            }
            Err(error) => self.session.update(cx, |session, cx| {
                session.state.error = Some(error);
                cx.notify();
            }),
        }
    }
}
