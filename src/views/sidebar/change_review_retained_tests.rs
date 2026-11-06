//! Retained Review bindings with channel transports and an isolated durable store.
use super::*;
use grove_core::{
    change_result_store::PendingRequest,
    codex::{Connection, Handle, Provider, Snapshot, Status},
};
use serde_json::json;
struct RetainedFixture {
    sidebar: Entity<Sidebar>,
    path: String,
    _observer: gpui::Subscription,
}
impl Render for RetainedFixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sidebar.update(cx, |s, cx| {
            // This is a storage/navigation fixture, not a remote freshness request.
            for entry in s.reviews.entries.values_mut() {
                entry.checking = true;
            }
            if s.reviews.open.is_some() {
                s.render_review(window, cx)
            } else {
                s.render_review_controls(&self.path, false, None, window, cx)
            }
        })
    }
}
#[gpui::test]
fn retained_review_restores_canonical_result_and_permanent_view_for_both_providers(
    cx: &mut gpui::TestAppContext,
) {
    for provider in [Provider::Codex, Provider::Claude] {
        let directory = tempfile::tempdir().unwrap();
        let frozen = super::super::tests::frozen_review();
        let review=domain::from_conversation(frozen.snapshot, &json!({"summary":frozen.summary,"reviewed_files":frozen.reviewed_files,"findings":frozen.findings}).to_string()).unwrap();
        let source = Source {
            checkout: review.snapshot.checkout.clone(),
            scope: review.snapshot.scope.clone(),
        };
        let store = Store::<Review>::new(directory.path().into());
        let mut saved = SavedSession::empty(
            "review-retained-fixture".into(),
            provider,
            String::new(),
            source.checkout.clone(),
        );
        // Ordinary later prose belongs to this conversation, not to the original typed result.
        saved.items = vec![
            json!({"type":"userMessage","content":[{"type":"text","text":"Explain the earlier review"}]}),
            json!({"type":"agentMessage","text":"Ordinary follow-up prose; no new structured review."}),
        ].into();
        store.bind(&saved.key, &source).unwrap();
        store
            .save_pending(
                &saved.key,
                &PendingRequest {
                    snapshot: review.snapshot.clone(),
                    input: String::new(),
                    prompt: domain::conversation_prompt(&review.snapshot).unwrap(),
                    generation: "retained-fixture-generation".into(),
                },
            )
            .unwrap();
        store
            .save_generation(&saved.key, &review, "retained-fixture-generation")
            .unwrap();
        cx.update(|cx| {
            gpui_component::init(cx);
            cx.set_reduce_motion(true);
            cx.set_global(SettingsState::new(grove_core::storage::Store::default()));
            cx.set_global(crate::zoom::CurrentPtyDims::default());
            cx.set_global(crate::zoom::ZoomState::new(1.0));
            c::sync_component_theme(cx);
        });
        let mut sidebar_handle = None;
        let mut selected = None;
        let mut action_receivers = Vec::new();
        let mut updates = Vec::new();
        let (_, cx) = cx.add_window_view(|window, cx| {
            let runtime = cx.new(crate::runtime::Runtime::new);
            let ids = [0, 1].map(|_| {
                let (handle, actions) = Handle::channel();
                action_receivers.push(actions);
                let (send, receive) = std::sync::mpsc::channel();
                updates.push(send);
                let mut state = Snapshot::new(saved.clone());
                state.status = Status::Idle;
                let session = cx.new(|cx| {
                    crate::entities::codex_session::CodexSession::from_connection(
                        state,
                        Connection {
                            handle,
                            updates: receive.into(),
                        },
                        cx,
                    )
                });
                runtime.update(cx, |r, cx| r.adopt_dashboard_session(session, cx))
            });
            selected = Some(ids[1]);
            let sidebar = cx.new(|cx| {
                let mut s = Sidebar::new(runtime, window, cx);
                s.initial_selection_pending = false;
                s.dashboard_open = false;
                s.selection = Some(Selection::Session(ids[1]));
                s.checkout_panel.hidden = true;
                s.reviews.store_root = Some(directory.path().into());
                s.reviews.hydrated.insert((ids[0], saved.key.clone()));
                s
            });
            sidebar_handle = Some(sidebar.clone());
            let fixture = cx.new(|cx| {
                let observer = cx.observe(&sidebar, |_, _, cx| cx.notify());
                RetainedFixture {
                    sidebar,
                    path: source.checkout.clone(),
                    _observer: observer,
                }
            });
            gpui_component::Root::new(fixture, window, cx).bordered(false)
        });
        let sidebar = sidebar_handle.unwrap();
        let id = selected.unwrap();
        for _ in 0..150 {
            cx.run_until_parked();
            if sidebar.read_with(cx, |s, _| {
                s.reviews
                    .linked
                    .get(&id)
                    .and_then(|k| s.reviews.entries.get(k))
                    .is_some_and(|e| e.result.is_some())
            }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            cx.debug_bounds("review-open").is_some(),
            "permanent View review must survive a clean/detached source"
        );
        sidebar.read_with(cx, |s, _| {
            let target = s.linked_review_key().unwrap();
            let entry = &s.reviews.entries[&target];
            assert_eq!(entry.session, Some(id));
            assert_eq!(entry.result.as_deref(), Some(&review));
            assert_eq!(s.review_scope_for_session(id), Some(source.scope.clone()));
            assert!(
                s.reviews.open.is_none(),
                "hydration preserves the conversation rather than forcing the reader"
            );
        });
        let open = cx.debug_bounds("review-open").unwrap();
        cx.simulate_click(open.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| {
            sidebar.update(cx, |s, cx| {
                assert_eq!(s.selection, Some(Selection::Session(id)));
                assert!(s.reviews.open.is_some());
                s.close_review(window, cx);
                assert_eq!(s.selection, Some(Selection::Session(id)));
            })
        });
        assert!(
            action_receivers
                .iter()
                .all(|actions| actions.try_iter().next().is_none()),
            "hydration/View/Conversation must not submit provider prompts"
        );
        assert_eq!(store.load(&source).unwrap(), Some(review.clone()));
        fs_err::remove_file(store.result_path(&source).unwrap()).unwrap();
        cx.update(|_, cx| {
            sidebar.update(cx, |s, cx| {
                let target = s.linked_review_key().unwrap();
                let entry = s.reviews.entries.get_mut(&target).unwrap();
                entry.result = None;
                entry.text = None;
                // Recreate the controller as after restart; an in-memory old result
                // is intentionally retained by reload when its disk file vanishes.
                let controller = s
                    .review_controller(id, target.clone(), store.clone(), saved.key.clone(), cx)
                    .unwrap();
                controller.update(cx, |c, cx| c.hydrate(cx));
                s.reviews.open = Some(target);
                cx.notify();
            })
        });
        let mut settled = false;
        for _ in 0..150 {
            cx.run_until_parked();
            settled = sidebar.read_with(cx, |s, cx| {
                let target = s.linked_review_key().unwrap();
                let entry = &s.reviews.entries[&target];
                entry
                    .controller
                    .as_ref()
                    .is_some_and(|c| !c.read(cx).phase.busy())
                    && entry.cancel.is_none()
                    && entry.result.is_none()
            });
            if settled {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            settled,
            "fresh controller must finish missing-result hydration before checking UI"
        );
        cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        assert!(
            cx.debug_bounds("review-empty-generate").is_some(),
            "a saved source link with no completed result must offer explicit generation"
        );
        assert!(
            action_receivers
                .iter()
                .all(|actions| actions.try_iter().next().is_none()),
            "missing result recovery must not resubmit automatically"
        );
    }
}

#[test]
fn independent_review_factory_keeps_selected_model_and_source_draft() {
    for provider in [Provider::Codex, Provider::Claude] {
        let mut original = SavedSession::empty(
            "source-session".into(),
            provider,
            "Project".into(),
            "/source".into(),
        );
        original.composer.model = Some("selected-model".into());
        original.composer.effort = Some("high".into());
        original.draft = "/review local".into();
        original.draft_images = vec!["/source-image.png".into()];
        original.thread_id = Some("source-thread".into());
        let before = serde_json::to_value(&original).unwrap();
        let source = Source {
            checkout: "/captured-checkout".into(),
            scope: Scope::PullRequest {
                url: "https://github.com/example/repo/pull/42".into(),
            },
        };
        let session = retained_review_session(
            Some(original.clone()),
            "review-session".into(),
            provider,
            &source,
            "Captured project".into(),
        );
        assert_eq!(session.provider, provider);
        assert_eq!(session.composer.model, original.composer.model);
        assert_eq!(session.composer.effort, original.composer.effort);
        assert_eq!(session.cwd, source.checkout);
        assert!(session.thread_id.is_none());
        assert!(session.items.is_empty());
        assert!(session.draft.is_empty());
        assert!(session.draft_images.is_empty());
        assert_eq!(serde_json::to_value(&original).unwrap(), before);
    }
}
