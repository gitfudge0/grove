//! Session-owned, read-only side conversations.
use super::*;
use crate::views::components::{project_field_well, shimmer_label, transcript_action};
use futures::StreamExt;
use gpui::AnimationExt;
use gpui_component::{
    input::{Input, InputState, TextDecoration, TextDecorationCollection},
    text::{TextView, TextViewState},
    Disableable,
};

pub(super) const SIDE_CHAT_WIDTH: f32 = 400.;
const SIDE_CHAT_NARROW_HEIGHT: f32 = 280.;
const SIDE_CHAT_USER_MAX_WIDTH: f32 = 320.;

pub(super) struct SideChat {
    input: Entity<InputState>,
    markdown_decorations: TextDecorationCollection,
    markdown_source: Option<(String, u64, Vec<TextDecoration>)>,
    _input_subscription: gpui::Subscription,
    messages: Vec<(String, String)>,
    message_views: Vec<MessageView>,
    hovered_messages: HashSet<usize>,
    entrances: HashMap<usize, Instant>,
    copied_messages: HashMap<usize, Instant>,
    backend: std::sync::Arc<std::sync::Mutex<grove_core::side_chat::Session>>,
    provider_session: Option<Box<grove_core::codex::SavedSession>>,
    busy: bool,
    error: Option<String>,
    retryable: bool,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    scroll: gpui::ScrollHandle,
    follow_latest: bool,
    pending_scroll: Option<bool>,
    motion_frame: Option<Instant>,
    motion_frame_pending: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Drop for SideChat {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

struct MessageView {
    view: Entity<TextViewState>,
    stream_text: (String, usize),
    focus: FocusHandle,
    _subscriptions: Vec<gpui::Subscription>,
}

#[cfg(test)]
#[path = "side_chat_native_tests.rs"]
mod native_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn conversation_and_pending_status_have_room_above_composer(cx: &mut gpui::TestAppContext) {
        let fixture = tempfile::tempdir().unwrap();
        let path = fixture.path().to_string_lossy().into_owned();
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["init", "-b", "main"])
            .output()
            .unwrap()
            .status
            .success());
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_reduce_motion(true);
            cx.set_global(SettingsState::new(grove_core::storage::Store {
                projects: vec![grove_core::storage::Project {
                    name: "Side chat".into(),
                    path: path.clone(),
                    scripts: Default::default(),
                    archived: false,
                    worktree_dir: None,
                }],
                ..Default::default()
            }));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
        });
        let (updates, receive) = std::sync::mpsc::channel();
        let (sidebar, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            let registry = runtime.read(cx).registry.clone();
            let id = registry.update(cx, |registry, _| {
                registry.insert_meta("Side chat".into(), path.clone(), Agent::Codex)
            });
            runtime.read(cx).tree.clone().update(cx, |tree, _| {
                tree.set_active_worktrees(
                    0,
                    vec![grove_core::git::Worktree {
                        path: path.clone(),
                        branch: "main".into(),
                        mtime: None,
                        is_main: true,
                    }],
                );
            });
            let saved = grove_core::codex::SavedSession {
                session_review: Default::default(),
                        rich_plans: Default::default(),
                side_chats: Default::default(),
                claude_resume: None,
                claude_subagents: Default::default(),
                edit_checkpoints: Vec::new(),
                pending_edit: None,
            rewind_drafts: Vec::new(),
                provider: grove_core::codex::Provider::Codex,
                composer: Default::default(),
                context: None,
                key: "side-chat-layout".into(),
                project: "Side chat".into(),
                cwd: path.clone(),
                label: "Codex".into(),
                title: None,
                title_generation: None,
                roots: Vec::new(),
                thread_id: Some("side-chat-layout-thread".into()),
                items: vec![serde_json::json!({"id":"main-history", "type":"agentMessage", "text":"Main transcript content."})].into(),
                work_periods: Vec::new(),
                draft: String::new(),
                draft_images: Vec::new(),
                draft_reference: None,
                config_args: Vec::new(),
                native_config_version: 1,
                activity_reporting: false,
            };
            let (handle, _actions) = grove_core::codex::Handle::channel();
            let session = cx.new(|cx| {
                crate::entities::codex_session::CodexSession::from_connection(
                    grove_core::codex::Snapshot::new(saved),
                    grove_core::codex::Connection {
                        handle: handle.clone(),
                        updates: receive.into(),
                    },
                    cx,
                )
            });
            registry.update(cx, |registry, _| registry.attach_codex(id, session, handle));
            let mut sidebar = Sidebar::new(runtime, window, cx);
            sidebar.selection = Some(Selection::Session(id));
            sidebar.initial_selection_pending = false;
            sidebar.checkout_panel.hidden = false;
            sidebar.checkout_panel.side_chat_owner = sidebar.selection.clone();
            sidebar
        });
        cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        // Reopening the wide panel must keep its full height from the first
        // animated frame; otherwise the composer can be clipped during entry.
        let full_height = cx.debug_bounds("side-chat-panel").unwrap().size.height;
        cx.update(|window, cx| {
            cx.set_reduce_motion(false);
            sidebar.update(cx, |sidebar, cx| {
                sidebar.toggle_side_chat(window, cx);
                sidebar.toggle_side_chat(window, cx);
            });
            let _ = window.draw(cx);
        });
        let opening_height = cx.debug_bounds("side-chat-panel").unwrap().size.height;
        assert_eq!(
            opening_height, full_height,
            "panel height changed on the opening frame"
        );
        cx.update(|_, cx| cx.set_reduce_motion(true));
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let key = sidebar
                    .checkout_panel
                    .side_chats
                    .keys()
                    .next()
                    .unwrap()
                    .clone();
                let input = sidebar.checkout_panel.side_chats[&key].input.clone();
                let chat = sidebar.checkout_panel.side_chats.get_mut(&key).unwrap();
                chat.messages.push((
                    "assistant".into(),
                    "[link](https://example.com) **Bold**\n\n```rust\nlet x = 1;\n```".into(),
                ));
                chat.messages
                    .push(("user".into(), "Earlier question".into()));
                input.update(cx, |input, cx| {
                    input.set_value("Can you explain this?", window, cx);
                    input.focus(window, cx);
                });
                // Simulate a stale selection without redrawing. Enter must report
                // the rejection and retain the draft, without starting a provider.
                sidebar.selection = None;
                input.update(cx, |_, cx| {
                    cx.emit(gpui_component::input::InputEvent::PressEnter {
                        secondary: false,
                        shift: false,
                    })
                });
            })
        });
        cx.update(|window, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let key = sidebar
                    .checkout_panel
                    .side_chats
                    .keys()
                    .next()
                    .unwrap()
                    .clone();
                let chat = sidebar
                    .checkout_panel
                    .side_chats
                    .values_mut()
                    .next()
                    .unwrap();
                assert!(chat
                    .error
                    .as_deref()
                    .unwrap_or_default()
                    .contains("changed"));
                assert_eq!(
                    chat.input.read(cx).value().to_string(),
                    "Can you explain this?"
                );
                assert!(
                    !chat.retryable,
                    "an unsent draft must never retry an older question"
                );
                sidebar.selection = Some(Selection::Session(key.0));
                sidebar.retry_side_chat(key.clone(), window, cx);
                assert!(!sidebar.checkout_panel.side_chats[&key].busy);
                assert_eq!(
                    sidebar.checkout_panel.side_chats[&key]
                        .messages
                        .last()
                        .unwrap()
                        .1,
                    "Earlier question"
                );
                sidebar.send_side_chat(key.clone(), window, cx);
                let chat = &sidebar.checkout_panel.side_chats[&key];
                assert!(chat.busy);
                assert_eq!(chat.messages.last().unwrap().1, "Can you explain this?");
                // Stop before the background executor can launch a provider.
                chat.cancel
                    .as_ref()
                    .unwrap()
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            })
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let chat = sidebar.checkout_panel.side_chats.values().next().unwrap();
                assert_eq!(
                    chat.message_views[0].view.read(cx).source().as_ref(),
                    "[link](https://example.com) **Bold**\n\n```rust\nlet x = 1;\n```"
                );
            });
        });
        assert!(
            cx.debug_bounds("side-chat-message-0").is_some(),
            "assistant Markdown is mounted in the transcript"
        );
        let main_composer = cx.debug_bounds("codex-composer").unwrap();
        let card = cx.debug_bounds("side-chat-card").unwrap();
        assert!(
            (main_composer.bottom() - card.bottom()).abs() <= gpui::px(1.),
            "side chat must align with the main composer: {main_composer:?} {card:?}"
        );
        let composer = cx.debug_bounds("side-chat-composer").unwrap();
        let input = cx.debug_bounds("side-chat-input").unwrap();
        let footer = cx.debug_bounds("side-chat-composer-footer").unwrap();
        assert!(
            input.top() >= composer.top() && input.bottom() <= footer.top(),
            "input must fit above composer footer: {input:?} {footer:?}"
        );
        assert!(
            footer.bottom() < composer.bottom(),
            "footer must fit within composer: {footer:?} {composer:?}"
        );
        let bubble = cx.debug_bounds("side-chat-bubble-1").unwrap();
        let user_copy = cx.debug_bounds("side-chat-copy-1").unwrap();
        assert!(
            user_copy.top() >= bubble.bottom(),
            "Copy belongs outside the user bubble: {user_copy:?} {bubble:?}"
        );
        let history = cx.debug_bounds("side-chat-history").unwrap();
        let responding = cx.debug_bounds("side-chat-responding").unwrap();
        assert!(
            history.size.height > gpui::px(100.),
            "history was squeezed by the composer: {history:?}"
        );
        assert!(
            responding.top() >= history.top() && responding.bottom() <= history.bottom(),
            "pending feedback must be visible: {responding:?} vs {history:?}"
        );
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, _| {
                if let Some(cancel) = sidebar
                    .checkout_panel
                    .side_chats
                    .values()
                    .next()
                    .and_then(|chat| chat.cancel.as_ref())
                {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            cx.debug_bounds("side-chat-copy-code").is_some(),
            "Markdown code blocks expose the shared copy action"
        );
        let body = cx.debug_bounds("side-chat-body-0").unwrap();
        cx.simulate_click(
            body.origin + gpui::point(gpui::px(8.), gpui::px(8.)),
            gpui::Modifiers::default(),
        );
        assert_eq!(cx.opened_url().as_deref(), Some("https://example.com"));
        let code_copy = cx.debug_bounds("side-chat-copy-code").unwrap();
        cx.simulate_click(code_copy.center(), gpui::Modifiers::default());
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text()))
                .as_deref(),
            Some("let x = 1;")
        );
        let message_copy = cx.debug_bounds("side-chat-copy-0").unwrap();
        cx.simulate_click(message_copy.center(), gpui::Modifiers::default());
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().and_then(|clip| clip.text()))
                .as_deref(),
            Some("[link](https://example.com) **Bold**\n\n```rust\nlet x = 1;\n```")
        );
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, _| {
                assert!(sidebar
                    .checkout_panel
                    .side_chats
                    .values()
                    .next()
                    .unwrap()
                    .copied_messages
                    .contains_key(&0));
            })
        });
        // Measure the actual main and side surfaces together for both providers
        // and changed rem zoom, rather than comparing isolated panel snapshots.
        for provider in [
            grove_core::codex::Provider::Codex,
            grove_core::codex::Provider::Claude,
        ] {
            for (width, height, rem) in
                [(1280., 800., 16.), (1100., 760., 16.), (1600., 1000., 20.)]
            {
                cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(height)));
                cx.update(|window, cx| {
                    window.set_rem_size(gpui::px(rem));
                    sidebar.update(cx, |sidebar, cx| {
                        let id = sidebar.checkout_panel.side_chats.keys().next().unwrap().0;
                        let session = sidebar
                            .runtime
                            .read(cx)
                            .registry
                            .read(cx)
                            .codex(id)
                            .unwrap()
                            .clone();
                        session.update(cx, |session, cx| {
                            session.state.saved.provider = provider;
                            cx.notify();
                        });
                    });
                });
                for _ in 0..3 {
                    cx.run_until_parked();
                    cx.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                let main = cx.debug_bounds("codex-composer").unwrap();
                let side = cx.debug_bounds("side-chat-card").unwrap();
                assert!(
                    (main.bottom() - side.bottom()).abs() <= gpui::px(1.),
                    "{provider:?} at {width}/{rem}: {main:?} vs {side:?}"
                );
            }
        }
        cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(800.)));
        cx.update(|window, _| window.set_rem_size(gpui::px(16.)));
        // An ambiguous multi-checkout session must not overflow a narrow window.
        let second_path = fixture.path().join("second").to_string_lossy().into_owned();
        cx.update(|_, cx| {
            sidebar.update(cx, |sidebar, cx| {
                let runtime = sidebar.runtime.read(cx);
                let registry = runtime.registry.clone();
                let tree = runtime.tree.clone();
                let id = registry.update(cx, |registry, _| {
                    registry.insert_meta_with_context(
                        "Side chat".into(),
                        path.clone(),
                        Agent::Codex,
                        vec![grove_core::session_meta::ContextRoot {
                            project: "Side chat".into(),
                            wt_path: second_path.clone(),
                        }],
                        None,
                    )
                });
                tree.update(cx, |tree, _| {
                    tree.set_active_worktrees(
                        0,
                        vec![
                            grove_core::git::Worktree {
                                path: path.clone(),
                                branch: "main".into(),
                                mtime: None,
                                is_main: true,
                            },
                            grove_core::git::Worktree {
                                path: second_path.clone(),
                                branch: "other".into(),
                                mtime: None,
                                is_main: false,
                            },
                        ],
                    )
                });
                sidebar.selection = Some(Selection::Session(id));
                sidebar.checkout_panel.side_chat_owner = sidebar.selection.clone();
                sidebar.checkout_panel.chosen_root = None;
                cx.notify();
            })
        });
        cx.simulate_resize(gpui::size(gpui::px(320.), gpui::px(800.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let required = cx.debug_bounds("side-chat-checkout-required").unwrap();
        assert!(
            required.size.width <= gpui::px(320.),
            "checkout selection must fit narrow windows: {required:?}"
        );
        drop(updates);
    }
}

impl Sidebar {
    fn copy_side_message(
        &mut self,
        key: (SessionId, String),
        index: usize,
        text: String,
        cx: &mut Context<Self>,
    ) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        let stamp = cx.background_executor().now();
        if let Some(chat) = self.checkout_panel.side_chats.get_mut(&key) {
            chat.copied_messages.insert(index, stamp);
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(chat) = this.checkout_panel.side_chats.get_mut(&key) {
                    if chat.copied_messages.get(&index) == Some(&stamp) {
                        chat.copied_messages.remove(&index);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn persist_side_chat_draft(&self, key: &(SessionId, String), cx: &mut Context<Self>) {
        let Some(chat) = self.checkout_panel.side_chats.get(key) else {
            return;
        };
        let draft = chat.input.read(cx).value().to_string();
        let session = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(key.0)
            .cloned();
        if let Some(session) = session {
            session.update(cx, |session, cx| {
                session.send(
                    grove_core::codex::Action::SideChatDraft {
                        checkout: key.1.clone(),
                        draft,
                    },
                    cx,
                )
            });
        }
    }
    fn persist_side_chat(&self, key: &(SessionId, String), cx: &mut Context<Self>) {
        let Some(chat) = self.checkout_panel.side_chats.get(key) else {
            return;
        };
        let mut messages = chat.messages.clone();
        // Keep partial output visible in memory, but persist an unanswered user
        // question so restoring an interrupted request offers a truthful retry.
        if (chat.busy || chat.retryable)
            && messages.last().is_some_and(|(role, _)| role == "assistant")
        {
            messages.pop();
        }
        let saved = grove_core::codex::SavedSideChat {
            draft: chat.input.read(cx).value().to_string(),
            messages,
            session: chat.provider_session.clone(),
        };
        let session = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(key.0)
            .cloned();
        if let Some(session) = session {
            session.update(cx, |session, cx| {
                session.send(
                    grove_core::codex::Action::SideChat {
                        checkout: key.1.clone(),
                        chat: saved,
                    },
                    cx,
                )
            });
        }
    }
    pub(super) fn toggle_side_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was_visible = !self.checkout_panel.hidden;
        if self.checkout_panel.side_chat_owner == self.selection {
            self.checkout_panel.side_chat_owner = None;
            self.checkout_panel.hidden = true;
        } else {
            self.dismiss_agent_peek(window, cx);
            self.dismiss_plan_peek(window, cx);
            self.dismiss_script_peek(window, cx);
            self.checkout_panel.side_chat_owner = self.selection.clone();
            self.checkout_panel.hidden = false;
            self.checkout_panel.expanded = true;
        }
        self.start_panel_motion(was_visible, !self.checkout_panel.hidden, cx);
        cx.notify();
    }

    fn send_side_chat(
        &mut self,
        key: (SessionId, String),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.selection, Some(Selection::Session(id) | Selection::Home(id)) if id == key.0)
            || super::checkout_panel::selected_checkout(
                &self.checkout_targets(cx),
                self.checkout_panel.chosen_root.as_deref(),
            )
            .as_ref()
            .map(|(_, path)| path)
                != Some(&key.1)
        {
            if let Some(chat) = self.checkout_panel.side_chats.get_mut(&key) {
                chat.error = Some(
                    "The selected session or checkout changed. Select it again to send.".into(),
                );
                chat.retryable = false;
            }
            cx.notify();
            return;
        }
        let Some(session) = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(key.0)
            .cloned()
        else {
            if let Some(chat) = self.checkout_panel.side_chats.get_mut(&key) {
                chat.error = Some("This session is unavailable. Your draft was kept.".into());
                chat.retryable = false;
            }
            cx.notify();
            return;
        };
        let parent = session.read(cx).state.saved.clone();
        let cwd = key.1.clone();
        let Some(chat) = self.checkout_panel.side_chats.get_mut(&key) else {
            return;
        };
        let question = chat.input.read(cx).value().to_string();
        if chat.busy || question.trim().is_empty() {
            return;
        }
        chat.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        chat.messages.push(("user".into(), question));
        if !cx.reduce_motion() {
            chat.entrances
                .insert(chat.messages.len() - 1, cx.background_executor().now());
        }
        chat.error = None;
        chat.retryable = false;
        chat.busy = true;
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        chat.cancel = Some(cancel.clone());
        let conversation = chat.messages.clone();
        let backend = chat.backend.clone();
        chat.follow_latest = true;
        self.persist_side_chat(&key, cx);
        let answer_index = conversation.len();
        cx.spawn(async move |this, cx| {
            let (updates, mut incoming) = futures::channel::mpsc::unbounded();
            let result = cx.background_executor().spawn(async move {
                let result = match backend.lock() {
                    Ok(mut backend) => {
                        backend.answer(&parent, &cwd, &conversation, &cancel, |text, saved| {
                            let _ = updates.unbounded_send((text, saved));
                        })
                    }
                    Err(_) => Err("Side session is unavailable. Reopen Grove to continue.".into()),
                };
                drop(updates);
                result
            });
            while let Some((text, saved)) = incoming.next().await {
                let _ = this.update(cx, |this, cx| {
                    if let Some(chat) = this.checkout_panel.side_chats.get_mut(&key) {
                        chat.provider_session = Some(saved);
                        if text.is_empty() {
                            this.persist_side_chat(&key, cx);
                            return;
                        }
                        if chat.messages.len() == answer_index {
                            chat.messages.push(("assistant".into(), text));
                            if !cx.reduce_motion() {
                                chat.entrances
                                    .insert(answer_index, cx.background_executor().now());
                            }
                        } else if let Some((_, answer)) = chat.messages.get_mut(answer_index) {
                            *answer = text;
                        }
                    }
                    this.persist_side_chat(&key, cx);
                    cx.notify();
                });
            }
            let result = result.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(chat) = this.checkout_panel.side_chats.get_mut(&key) {
                    chat.busy = false;
                    chat.cancel = None;
                    match result {
                        Ok(answer) => {
                            let new_message = chat.messages.len() == answer_index;
                            if new_message {
                                chat.messages.push(("assistant".into(), answer));
                            } else if let Some((_, text)) = chat.messages.get_mut(answer_index) {
                                *text = answer;
                            }
                            if new_message && !cx.reduce_motion() {
                                chat.entrances.insert(
                                    chat.messages.len() - 1,
                                    cx.background_executor().now(),
                                );
                            }
                        }
                        Err(error) => {
                            chat.error = Some(error);
                            chat.retryable = true;
                        }
                    }
                }
                this.persist_side_chat(&key, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn retry_side_chat(
        &mut self,
        key: (SessionId, String),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.selection, Some(Selection::Session(id) | Selection::Home(id)) if id == key.0)
            || super::checkout_panel::selected_checkout(
                &self.checkout_targets(cx),
                self.checkout_panel.chosen_root.as_deref(),
            )
            .as_ref()
            .map(|(_, path)| path)
                != Some(&key.1)
        {
            return;
        }
        let Some(chat) = self.checkout_panel.side_chats.get_mut(&key) else {
            return;
        };
        if chat.retryable
            && chat
                .messages
                .last()
                .is_some_and(|(role, _)| role == "assistant")
        {
            chat.messages.pop();
            chat.message_views.truncate(chat.messages.len());
        }
        let Some((role, question)) = chat.messages.last() else {
            return;
        };
        if role != "user" || chat.busy || !chat.retryable {
            return;
        }
        let question = question.clone();
        let draft = chat.input.read(cx).value().to_string();
        let input = chat.input.clone();
        chat.messages.pop();
        chat.input
            .update(cx, |input, cx| input.set_value(question, window, cx));
        self.send_side_chat(key, window, cx);
        input.update(cx, |input, cx| input.set_value(draft, window, cx));
    }

    pub(super) fn render_side_chat(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(Selection::Session(id) | Selection::Home(id)) = self.selection else {
            return div().into_any_element();
        };
        let targets = self.checkout_targets(cx);
        let selected = super::checkout_panel::selected_checkout(
            &targets,
            self.checkout_panel.chosen_root.as_deref(),
        );
        // Only recover a previously bound conversation when no checkout is
        // available. Ambiguous multi-root sessions still require a selection.
        let recovery_paths = if targets.is_empty() {
            let mut paths = self
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .codex(id)
                .map(|session| {
                    session
                        .read(cx)
                        .state
                        .saved
                        .side_chats
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            paths.extend(
                self.checkout_panel
                    .side_chats
                    .keys()
                    .filter(|(session, _)| *session == id)
                    .map(|(_, path)| path.clone()),
            );
            paths.sort();
            paths.dedup();
            paths
        } else {
            Vec::new()
        };
        let recovered = self
            .checkout_panel
            .chosen_root
            .as_ref()
            .filter(|path| recovery_paths.contains(path))
            .cloned()
            .or_else(|| (recovery_paths.len() == 1).then(|| recovery_paths[0].clone()));
        let available = selected.is_some();
        let wide = Self::logical_window_width(window) >= super::checkout_panel::WIDE_VIEWPORT;
        let Some(path) = selected.map(|(_, path)| path).or(recovered) else {
            let mut panel = div()
                .id("side-chat-checkout-required")
                .debug_selector(|| "side-chat-checkout-required".into())
                .when(wide, |panel| panel.w(rpx(SIDE_CHAT_WIDTH)))
                .when(!wide, |panel| panel.w_full())
                .h_full()
                .pt(rpx(APPBAR_TOTAL_H + SPACE_3XL))
                .p(rpx(SPACE_3XL))
                .text_size(rpx(TEXT_BODY))
                .text_color(c::FG_DIM())
                .flex()
                .flex_col()
                .gap(rpx(SPACE_LG));
            if recovery_paths.is_empty() {
                panel = panel.child("Choose a checkout in the right panel before using Side chat.");
            } else {
                panel = panel.child(
                    "These checkouts are unavailable. Choose a conversation to view its history.",
                );
                for (index, path) in recovery_paths.into_iter().enumerate() {
                    let target = path.clone();
                    panel = panel.child(
                        transcript_action(("side-chat-recover", index), path, false, window, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.checkout_panel.chosen_root = Some(target.clone());
                                cx.notify();
                            })),
                    );
                }
            }
            return panel.into_any_element();
        };
        let key = (id, path.clone());
        let provider = self
            .runtime
            .read(cx)
            .registry
            .read(cx)
            .codex(id)
            .map(|session| session.read(cx).state.saved.provider);
        let supported = available && provider.is_some();
        let context_disclosure = match (available, provider) {
            (false, _) => "Context is unavailable; sending is disabled.",
            (true, Some(grove_core::codex::Provider::Codex)) => "Read-only · Luna · Codex sign-in · Separate resumable session with refreshed context and read-only commands.",
            (true, Some(grove_core::codex::Provider::Claude)) => "Read-only tools · Haiku · Claude Code sign-in · Normal startup hooks may run.",
            (true, None) => "Session context is unavailable; sending is disabled.",
        };
        if !self.checkout_panel.side_chats.contains_key(&key) {
            let saved = self
                .runtime
                .read(cx)
                .registry
                .read(cx)
                .codex(id)
                .and_then(|session| session.read(cx).state.saved.side_chats.get(&path).cloned());
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .auto_grow(2, 5)
                    .submit_on_enter(true)
                    .markdown_auto_continue(true)
                    .placeholder("Ask a question…")
            });
            if let Some(saved) = &saved {
                input.update(cx, |input, cx| input.set_value(&saved.draft, window, cx));
            }
            let markdown_decorations = input.update(cx, |input, cx| {
                input.create_decorations_collection(Vec::new(), cx)
            });
            let subscription_key = key.clone();
            let subscription = cx.subscribe_in(
                &input,
                window,
                move |this, _, event, window, cx| match event {
                    gpui_component::input::InputEvent::Change => {
                        this.persist_side_chat_draft(&subscription_key, cx);
                        cx.notify();
                    }
                    gpui_component::input::InputEvent::PressEnter { shift: false, .. } => {
                        this.send_side_chat(subscription_key.clone(), window, cx);
                    }
                    _ => {}
                },
            );
            let interrupted = saved.as_ref().is_some_and(|saved| {
                saved
                    .messages
                    .last()
                    .is_some_and(|(role, _)| role == "user")
            });
            self.checkout_panel.side_chats.insert(
                key.clone(),
                SideChat {
                    input,
                    markdown_decorations,
                    markdown_source: None,
                    _input_subscription: subscription,
                    backend: std::sync::Arc::new(std::sync::Mutex::new(
                        grove_core::side_chat::Session::restored(
                            saved.as_ref().and_then(|saved| saved.session.clone()),
                        ),
                    )),
                    provider_session: saved.as_ref().and_then(|saved| saved.session.clone()),
                    messages: saved.map(|saved| saved.messages).unwrap_or_default(),
                    message_views: Vec::new(),
                    hovered_messages: HashSet::new(),
                    entrances: HashMap::new(),
                    copied_messages: HashMap::new(),
                    busy: false,
                    error: interrupted.then(|| {
                        "The previous response was interrupted. Retry to continue.".into()
                    }),
                    retryable: interrupted,
                    cancel: None,
                    scroll: gpui::ScrollHandle::new(),
                    follow_latest: true,
                    pending_scroll: None,
                    motion_frame: None,
                    motion_frame_pending: Default::default(),
                },
            );
        }
        let Some(chat) = self.checkout_panel.side_chats.get_mut(&key) else {
            return div().into_any_element();
        };
        let source = chat.input.read(cx).value().to_string();
        let generation = grove_core::theme::generation();
        if chat
            .markdown_source
            .as_ref()
            .is_none_or(|(previous, theme, _)| previous != &source || *theme != generation)
        {
            let decorations = super::super::codex_view::composer_markdown_decorations(&source);
            let replacements = super::super::codex_view::composer_markdown_replacements(&source);
            chat.input.update(cx, |input, cx| {
                input.set_text_replacements(replacements, cx)
            });
            chat.markdown_source = Some((source, generation, decorations));
        }
        if let Some((_, _, decorations)) = &chat.markdown_source {
            chat.markdown_decorations.set(decorations.clone(), cx);
        }
        if let Some(toward_bottom) = chat.pending_scroll.take() {
            chat.follow_latest = toward_bottom
                && chat.scroll.max_offset().y + chat.scroll.offset().y <= gpui::px(1.);
        }
        let now = cx.background_executor().now();
        let elapsed = chat
            .motion_frame
            .replace(now)
            .map_or(0.0, |previous| now.duration_since(previous).as_secs_f32());
        let mut needs_frame = false;
        if chat.follow_latest {
            if cx.reduce_motion() || chat.message_views.is_empty() {
                chat.scroll.scroll_to_bottom();
            } else {
                let target = -chat.scroll.max_offset().y;
                let current = chat.scroll.offset();
                let fraction = 1.0 - (-elapsed / (MOTION_FAST_MS as f32 / 1000.0)).exp();
                let next = current.y + (target - current.y) * fraction;
                let settled = (target - next).abs() <= gpui::px(0.5);
                chat.scroll
                    .set_offset(gpui::point(current.x, if settled { target } else { next }));
                needs_frame |= !settled;
            }
        }
        let scroll = chat.scroll.clone();
        let scroll_key = key.clone();
        let mut history = div()
            .id("side-chat-history")
            .debug_selector(|| "side-chat-history".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .on_scroll_wheel(
                cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                    let delta = match event.delta {
                        gpui::ScrollDelta::Lines(delta) => delta.y,
                        gpui::ScrollDelta::Pixels(delta) => f32::from(delta.y),
                    };
                    if delta != 0.0 {
                        if let Some(chat) = this.checkout_panel.side_chats.get_mut(&scroll_key) {
                            chat.follow_latest = false;
                            chat.pending_scroll = Some(delta < 0.0);
                        }
                        cx.notify();
                    }
                }),
            )
            .flex()
            .flex_col()
            .gap(rpx(SPACE_2XL));
        chat.message_views.truncate(chat.messages.len());
        let now = cx.background_executor().now();
        chat.entrances.retain(|index, started| {
            *index < chat.messages.len()
                && !cx.reduce_motion()
                && now.duration_since(*started) < Duration::from_millis(MOTION_SLOW_MS)
        });
        for (index, (role, text)) in chat.messages.iter().enumerate() {
            if chat.message_views.len() <= index {
                let view = cx.new(|cx| TextViewState::markdown(text, cx));
                let focus = cx.focus_handle();
                let subscriptions = vec![
                    cx.observe(&view, |_, _, cx| cx.notify()),
                    cx.on_focus_in(&focus, window, |_, _, cx| cx.notify()),
                    cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify()),
                ];
                chat.message_views.push(MessageView {
                    view,
                    stream_text: (
                        text.clone(),
                        if role == "assistant" && chat.busy && !cx.reduce_motion() {
                            0
                        } else {
                            text.len()
                        },
                    ),
                    focus,
                    _subscriptions: subscriptions,
                });
            }
            let message_view = &mut chat.message_views[index];
            let shown = motion::streamed_text(
                text,
                &mut message_view.stream_text,
                elapsed,
                role == "assistant" && chat.busy && !cx.reduce_motion(),
            );
            needs_frame |= shown.len() < text.len();
            let view = &message_view.view;
            if view.read(cx).source().as_ref() != shown {
                view.update(cx, |view, cx| view.set_text_incremental(shown, cx));
            }
            let user = role == "user";
            let body = TextView::new(view)
                .style(super::super::codex_view::markdown_style(window))
                .selectable(true)
                .code_block_actions(|block, _, _| {
                    super::super::codex_view::copy_control(
                        "side-chat-copy-code",
                        "Copy code",
                        block.code().to_string(),
                    )
                });
            let copied = chat.copied_messages.contains_key(&index);
            let copy_key = key.clone();
            let copy_text = text.clone();
            let keyboard_key = key.clone();
            let keyboard_text = text.clone();
            let copy = if copied {
                crate::views::components::header_control_frame(
                    ("side-chat-copy", index),
                    "Copied",
                    c::BG_HOVER(),
                )
            } else {
                crate::views::components::header_control_frame(
                    ("side-chat-copy", index),
                    "Copy message",
                    c::BG_HOVER(),
                )
            }
            .debug_selector(move || format!("side-chat-copy-{index}"))
            .tab_index(0)
            .focus_visible(|style| style.bg(c::BG_HOVER()))
            .child(crate::icons::icon(
                if copied { "check" } else { "copy" },
                ICON_MD,
                c::FG_DIM(),
            ))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                    this.copy_side_message(keyboard_key.clone(), index, keyboard_text.clone(), cx);
                    cx.stop_propagation();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.copy_side_message(copy_key.clone(), index, copy_text.clone(), cx)
            }));
            let mut row = div()
                .id(("side-chat-message", index))
                .debug_selector(move || format!("side-chat-message-{index}"))
                .flex()
                .flex_col()
                .min_w_0()
                .gap(rpx(SPACE_LG));
            if user {
                row = row.w_full().max_w(rpx(SIDE_CHAT_USER_MAX_WIDTH));
            } else {
                row = row.w_full();
            }
            let focus = chat.message_views[index].focus.clone();
            let visible = copied
                || chat.hovered_messages.contains(&index)
                || focus.contains_focused(window, cx);
            let hover_key = key.clone();
            row = row
                .track_focus(&focus)
                .tab_index(0)
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if let Some(chat) = this.checkout_panel.side_chats.get_mut(&hover_key) {
                        if *hovered {
                            chat.hovered_messages.insert(index);
                        } else {
                            chat.hovered_messages.remove(&index);
                        }
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .debug_selector(move || format!("side-chat-body-{index}"))
                        .min_w_0()
                        .when(user, |bubble| {
                            bubble
                                .debug_selector(move || format!("side-chat-bubble-{index}"))
                                .p(rpx(SPACE_2XL))
                                .rounded(rpx(RADIUS_PANEL))
                                .bg(c::BG_RAIL())
                        })
                        .when(user, |bubble| {
                            bubble.child(
                                div()
                                    .mb(rpx(SPACE_LG))
                                    .text_size(rpx(TEXT_SMALL))
                                    .text_color(c::FG_DIM())
                                    .child("You"),
                            )
                        })
                        .child(body),
                )
                .child({
                    let actions = div()
                        .flex()
                        .justify_end()
                        .h(rpx(CHROME_CONTROL_H))
                        .child(copy);
                    if visible {
                        motion::fast(actions, ("side-chat-actions", index), cx)
                    } else {
                        actions.opacity(0.0).into_any_element()
                    }
                });
            let row = if !cx.reduce_motion()
                && chat.entrances.get(&index).is_some_and(|started| {
                    cx.background_executor().now().duration_since(*started)
                        < Duration::from_millis(MOTION_SLOW_MS)
                }) {
                let started = chat.entrances[&index];
                let executor = cx.background_executor().clone();
                row.relative()
                    .with_animation(
                        ("side-chat-entrance", index),
                        gpui::Animation::new(Duration::from_millis(MOTION_SLOW_MS))
                            .with_easing(motion::ease_standard),
                        move |row, _| {
                            let elapsed = executor.now().duration_since(started).as_secs_f32()
                                / Duration::from_millis(MOTION_SLOW_MS).as_secs_f32();
                            let progress = motion::ease_standard(elapsed.min(1.0));
                            let remaining = 1.0 - progress;
                            row.opacity(MOTION_FADE_FROM + (1.0 - MOTION_FADE_FROM) * progress)
                                .top(rpx(SPACE_3XL
                                    * remaining
                                    * remaining
                                    * (1.0 - 2.0 * progress)))
                        },
                    )
                    .into_any_element()
            } else {
                row.into_any_element()
            };
            history = history.child(if user {
                div()
                    .w_full()
                    .flex()
                    .justify_end()
                    .child(row)
                    .into_any_element()
            } else {
                row
            });
        }
        if chat.busy {
            history = history.child(
                div()
                    .id("side-chat-responding")
                    .debug_selector(|| "side-chat-responding".into())
                    .text_color(c::FG_DIM())
                    .child(shimmer_label(
                        "side-chat-working".into(),
                        "Responding…".into(),
                        true,
                        cx,
                    )),
            );
        }
        let retry_key = key.clone();
        if let Some(error) = &chat.error {
            history = history.child(div().text_color(c::FORM_ERROR()).child(error.clone()));
            if chat.retryable {
                history = history.child(
                    transcript_action("side-chat-retry", "Retry", false, window, cx)
                        .disabled(!supported)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.retry_side_chat(retry_key.clone(), window, cx)
                        })),
                );
            }
        }
        if needs_frame && !cx.reduce_motion() && !chat.motion_frame_pending.replace(true) {
            let owner = cx.entity().downgrade();
            let pending = chat.motion_frame_pending.clone();
            window.on_next_frame(move |_, cx| {
                pending.set(false);
                let _ = owner.update(cx, |_, cx| cx.notify());
            });
        }
        let input = chat.input.clone();
        let busy = chat.busy;
        let action_key = key.clone();
        let keyboard_key = key.clone();
        let mut recovery_choices = div().flex().flex_col().gap(rpx(SPACE_SM));
        if recovery_paths.len() > 1 {
            recovery_choices = recovery_choices.child("Other saved conversations:");
            for (index, candidate) in recovery_paths
                .iter()
                .enumerate()
                .filter(|(_, candidate)| *candidate != &path)
            {
                let target = candidate.clone();
                recovery_choices = recovery_choices.child(
                    transcript_action(
                        ("side-chat-recover", index),
                        candidate.clone(),
                        false,
                        window,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.checkout_panel.chosen_root = Some(target.clone());
                        cx.notify();
                    })),
                );
            }
        }
        let context_label = match provider {
            Some(grove_core::codex::Provider::Codex) => "Read-only · Luna",
            Some(grove_core::codex::Provider::Claude) => {
                "Haiku · Read tools only · Startup hooks may run"
            }
            None => "Read-only",
        };
        let input_label = format!("Ask about this session or worktree. {context_disclosure}");
        div().id("side-chat-panel").debug_selector(|| "side-chat-panel".into())
            .flex_shrink_0().min_h_0()
            .when(wide, |panel| panel.w(rpx(SIDE_CHAT_WIDTH)).h_full().pt(rpx(APPBAR_TOTAL_H + SPACE_3XL)))
            .when(!wide, |panel| panel.w_full().h(rpx(SIDE_CHAT_NARROW_HEIGHT)).pt(rpx(SPACE_3XL)))
            .px(rpx(SPACE_3XL)).pb(if wide { super::super::codex_view::composer_bottom_inset(window) } else { rpx(SPACE_3XL).to_pixels(window.rem_size()) }).flex().flex_col()
            .text_size(rpx(TEXT_BODY)).text_color(c::FG())
            .child(project_field_well(false, false).debug_selector(|| "side-chat-card".into()).h_full().min_h_0().w_full().p(rpx(SPACE_2XL)).justify_start().gap(rpx(SPACE_LG))
                .when(!available, |panel| panel.child(div().text_color(c::FG_DIM()).child("This checkout is unavailable. History remains readable; restore the checkout to ask another question.")))
                .when(!available && recovery_paths.len() > 1, |panel| panel.child(recovery_choices))
                .when(available && !supported, |panel| panel.child(div().text_color(c::FG_DIM()).child("Side chat is unavailable for this provider.")))
                .child(history)
                .child(div().id("side-chat-composer").debug_selector(|| "side-chat-composer".into())
                  .w_full().min_w_0().flex_shrink_0().flex().flex_col()
                  .p(rpx(SPACE_2XL)).gap(rpx(SPACE_LG))
                  .rounded(rpx(RADIUS_PANEL)).border_1().border_color(c::BORDER_SOFT()).bg(c::BG_RAIL())
                  .child(div().id("side-chat-input").debug_selector(|| "side-chat-input".into()).min_h(rpx(FORM_VALUE_LINE_H * 2.0))
                    .child(Input::new(&input).aria_label(input_label).appearance(false).bordered(false).focus_bordered(false).text_color(c::FG()).p_0()))
                  .child(div().id("side-chat-composer-footer").debug_selector(|| "side-chat-composer-footer".into()).flex().items_center().justify_between().gap(rpx(SPACE_SM))
                    .child(div().id("side-chat-context").min_w_0().flex_1().text_size(rpx(TEXT_SMALL)).text_color(c::FG_DIM()).child(context_label))
                    .child(if busy {
                    super::super::codex_view::composer_icon("side-chat-stop", "Stop response", "stop", false, true)
                        .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                                if let Some(cancel) = this.checkout_panel.side_chats.get(&keyboard_key).and_then(|chat| chat.cancel.as_ref()) { cancel.store(true, std::sync::atomic::Ordering::Relaxed); }
                                cx.stop_propagation(); cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(cancel) = this.checkout_panel.side_chats.get(&action_key).and_then(|chat| chat.cancel.as_ref()) {
                                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            cx.notify();
                        }))
                } else {
                    super::super::codex_view::composer_icon("side-chat-send", "Send", "arrow-up", !supported, true)
                        .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                            if supported && matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                                this.send_side_chat(keyboard_key.clone(), window, cx); cx.stop_propagation();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| this.send_side_chat(action_key.clone(), window, cx)))
                }))))
            .into_any_element()
    }
}
