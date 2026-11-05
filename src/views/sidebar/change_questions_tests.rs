//! Frozen production-reader fixtures, not live provider or GitHub checks.
use super::*;
use gpui::AppContext as _;
use std::io::Write as _;
const PATH: &str = "/grove-ask-fixture";
struct QuestionFixture {
    sidebar: Entity<Sidebar>,
    source: FocusHandle,
    rem: f32,
    width: f32,
    height: f32,
}
impl Render for QuestionFixture {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(gpui::px(self.rem));
        let reader = self
            .sidebar
            .update(cx, |sidebar, cx| sidebar.render_questions(window, cx));
        let sidebar = self.sidebar.clone();
        div()
            .w(gpui::px(self.width))
            .h(gpui::px(self.height))
            .max_w_full()
            .max_h_full()
            .flex()
            .flex_col()
            .bg(c::BG())
            .text_color(c::FG())
            .font_family(crate::fonts::UI_FAMILY)
            .text_size(rpx(TEXT_BODY))
            .child(
                action(
                    "ask-fixture-source",
                    "Conversation source",
                    "arrow-left",
                    window,
                    cx,
                )
                .with_focus_handle(self.source.clone()),
            )
            .child(div().flex_1().min_h_0().child(reader))
            .on_key_down(
                cx.listener(move |_, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        sidebar.update(cx, |s, cx| s.close_questions(window, cx));
                        cx.stop_propagation();
                    }
                }),
            )
    }
}
fn initialize(cx: &mut App) {
    gpui_component::init(cx);
    cx.set_reduce_motion(true);
    cx.set_global(SettingsState::new(grove_core::storage::Store {
        projects: vec![grove_core::storage::Project {
            name: "Ask fixture".into(),
            path: PATH.into(),
            scripts: Default::default(),
            archived: false,
            worktree_dir: None,
        }],
        ..Default::default()
    }));
    cx.set_global(crate::zoom::CurrentPtyDims::default());
    cx.set_global(crate::zoom::ZoomState::new(1.0));
    c::sync_component_theme(cx);
}
fn frozen_answer() -> Answer {
    Answer{snapshot:snapshot::Snapshot{scope:Scope::Local,fingerprint:"frozen-identity".into(),label:"Local changes · worktree".into(),head:"123456789abcdef".into(),files:vec![snapshot::SnapshotFile{path:"src/webhooks/handler.ts".into(),added:1,removed:0,patch:"@@ -1,2 +1,3 @@\n function deliver(event) {\n+  if (!claim(event.id)) return;\n   createOrder(event.data);\n".into(),omitted:None},snapshot::SnapshotFile{path:"assets/logo.png".into(),added:0,removed:0,patch:"".into(),omitted:Some("Binary patch unavailable".into())}],original_body:None,title:"".into(),base:"".into(),checkout:PATH.into()},question:"How are duplicate deliveries handled?".into(),answer:"The handler checks the delivery ID before creating an order. A duplicate returns early. This diff does not establish how long IDs are retained.".into(),citations:vec![domain::Citation{path:"src/webhooks/handler.ts".into(),side:domain::Side::New,start:2,end:2,explanation:"The guard returns for deliveries that cannot be claimed.".into()}],examined_files:vec!["src/webhooks/handler.ts".into()],limitations:vec!["The claim implementation is outside this diff.".into()]}
}
fn create_sidebar(window: &mut Window, cx: &mut App) -> (Entity<Sidebar>, FocusHandle) {
    let runtime = cx.new(crate::runtime::Runtime::new);
    runtime.read(cx).tree.clone().update(cx, |tree, _| {
        tree.set_active_worktrees(
            0,
            vec![grove_core::git::Worktree {
                path: PATH.into(),
                branch: "fixture".into(),
                mtime: None,
                is_main: true,
            }],
        )
    });
    let source = cx.focus_handle();
    let return_source = source.clone();
    let sidebar = cx.new(|cx| {
        let mut sidebar = Sidebar::new(runtime, window, cx);
        sidebar.initial_selection_pending = false;
        sidebar.snapshot = sidebar
            .runtime
            .update(cx, |runtime, cx| runtime.snapshot(cx));
        sidebar.selection = Some(Selection::Worktree(0, PATH.into()));
        let answer = Arc::new(frozen_answer());
        let k = key(&sidebar.selection, PATH, &Scope::Local);
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .auto_grow(3, 10)
        });
        input.update(cx, |input, cx| {
            input.set_value(answer.question.clone(), window, cx)
        });
        let text = cx.new(|cx| TextViewState::markdown(&answer.markdown(), cx));
        sidebar.questions.entries.insert(
            k.clone(),
            Entry {
                scope: Some(Scope::Local),
                input: Some(input),
                result: Some(answer),
                text: Some(text),
                freshness: Some(Ok(true)),
                checking: true,
                checked_at: Some(Instant::now()),
                ..Default::default()
            },
        );
        sidebar.questions.open = Some(k);
        sidebar.active_change_reader = Some("ask");
        sidebar.questions.source = Some(return_source);
        sidebar.questions.focus_pending = true;
        sidebar
    });
    (sidebar, source)
}
fn fixture(cx: &mut gpui::TestAppContext) -> (Entity<Sidebar>, &mut gpui::VisualTestContext) {
    cx.update(initialize);
    let mut sidebar = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let (view, source) = create_sidebar(window, cx);
        sidebar = Some(view.clone());
        let fixture = cx.new(|_| QuestionFixture {
            sidebar: view,
            source,
            rem: 16.,
            width: 900.,
            height: 1100.,
        });
        gpui_component::Root::new(fixture, window, cx).bordered(false)
    });
    cx.simulate_resize(gpui::size(gpui::px(900.), gpui::px(1100.)));
    draw(cx);
    (sidebar.unwrap(), cx)
}
fn draw(cx: &mut gpui::VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}
#[gpui::test]
fn question_reader_opens_captured_reference_and_restores_focus(cx: &mut gpui::TestAppContext) {
    let (sidebar, cx) = fixture(cx);
    cx.update(|window, cx| {
        let s = sidebar.read(cx);
        let e = s.questions.entries.values().next().unwrap();
        assert!(e
            .input
            .as_ref()
            .unwrap()
            .focus_handle(cx)
            .is_focused(window));
    });
    let reference = cx.debug_bounds("ask-reference-0").unwrap();
    cx.simulate_click(reference.center(), gpui::Modifiers::default());
    draw(cx);
    sidebar.read_with(cx, |s, _| {
        let e = s.questions.entries.values().next().unwrap();
        assert_eq!(
            e.diff.as_ref().unwrap().0,
            "src/webhooks/handler.ts · new 2–2"
        );
        assert_eq!(
            e.result.as_ref().unwrap().snapshot.files[0].patch,
            frozen_answer().snapshot.files[0].patch
        );
    });
    cx.simulate_keystrokes("escape");
    draw(cx);
    sidebar.read_with(cx, |s, _| {
        assert!(s.questions.entries.values().next().unwrap().diff.is_none())
    });
    let source = sidebar.read_with(cx, |s, _| s.questions.source.clone().unwrap());
    cx.simulate_keystrokes("escape");
    draw(cx);
    cx.update(|window, cx| {
        assert!(sidebar.read(cx).questions.open.is_none());
        assert!(source.is_focused(window));
    });
}
#[gpui::test]
fn stale_answers_and_failed_retry_preserve_question_and_answer(cx: &mut gpui::TestAppContext) {
    let (sidebar, cx) = fixture(cx);
    cx.update(|_, cx| {
        sidebar.update(cx, |s, cx| {
            let e = s.questions.entries.values_mut().next().unwrap();
            e.freshness = Some(Ok(false));
            e.error = Some("Provider unavailable".into());
            cx.notify();
        })
    });
    draw(cx);
    assert!(cx.debug_bounds("ask-copy").is_some());
    cx.update(|window, cx| {
        sidebar.update(cx, |s, cx| {
            let k = s.questions.open.clone().unwrap();
            let e = s.questions.entries.get_mut(&k).unwrap();
            e.input
                .as_ref()
                .unwrap()
                .update(cx, |input, cx| input.set_value("", window, cx));
            s.submit_question(&k, cx);
            let e = s.questions.entries.get(&k).unwrap();
            assert_eq!(
                e.result.as_ref().unwrap().question,
                frozen_answer().question
            );
            assert!(e.error.as_deref().unwrap().contains("Enter a question"));
        })
    });
}
#[gpui::test]
fn open_without_question_does_not_start_provider_and_key_owns_scope(cx: &mut gpui::TestAppContext) {
    let (sidebar, cx) = fixture(cx);
    cx.update(|_, cx| {
        sidebar.update(cx, |s, cx| {
            let selection = s.selection.clone();
            s.open_question(PATH.into(), Scope::Local, String::new(), None, cx);
            assert!(s.questions.entries.values().all(|e| e.cancel.is_none()));
            let local = key(&selection, PATH, &Scope::Local);
            assert_ne!(local, key(&selection, "/different-checkout", &Scope::Local));
            assert_ne!(
                local,
                key(
                    &selection,
                    PATH,
                    &Scope::PullRequest {
                        url: "https://github.com/a/b/pull/1".into()
                    }
                )
            );
            assert_ne!(
                local,
                key(
                    &Some(Selection::Worktree(1, PATH.into())),
                    PATH,
                    &Scope::Local
                )
            );
        })
    });
}
#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens an isolated native fixture under Xvfb; no provider calls"]
fn ask_changes_linux_native_capture() {
    gpui_platform::application().with_assets(crate::assets::Assets).run(|cx|{initialize(cx);crate::fonts::register(cx).unwrap();let mut sidebar_handle=None;let mut fixture_handle=None;let capture_width=if std::env::var("GROVE_ASK_CAPTURE_STATE").is_ok_and(|s|s=="stale-narrow"){340.}else{1000.};
 let window=cx.open_window(gpui::WindowOptions{window_bounds:Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(gpui::point(gpui::px(20.),gpui::px(20.)),gpui::size(gpui::px(capture_width),gpui::px(1000.))))),..Default::default()},|window,cx|{window.set_window_title("Grove Ask changes QA");let(sidebar,source)=create_sidebar(window,cx);sidebar_handle=Some(sidebar.clone());let fixture=cx.new(|_|QuestionFixture{sidebar,source,rem:16.,width:capture_width,height:900.});fixture_handle=Some(fixture.clone());cx.new(|cx|gpui_component::Root::new(fixture,window,cx))}).unwrap();
 let sidebar=sidebar_handle.unwrap();let fixture=fixture_handle.unwrap();cx.activate(true);cx.spawn(async move|cx|{
 cx.background_executor().timer(Duration::from_millis(500)).await;
 for(name,theme,rem,width,state)in[("answer-dark","grove-dark",16.,1000.,"answer"),("answer-light-zoom","grove-light",20.,1000.,"answer"),("stale-narrow","grove-dark",16.,340.,"stale"),("diff-dark","grove-dark",16.,1000.,"diff"),("error-dark","grove-dark",16.,1000.,"error")] {
  if std::env::var("GROVE_ASK_CAPTURE_STATE").is_ok_and(|selected|selected!=name){continue;}
  cx.update(|cx|{grove_core::theme::set_by_name(theme);c::sync_component_theme(cx);fixture.update(cx,|f,cx|{f.width=width;f.rem=rem;cx.notify();});});
  cx.update_window(window.into(),|_,window,cx|{window.resize(gpui::size(gpui::px(width),gpui::px(1000.)));sidebar.update(cx,|s,cx|{let e=s.questions.entries.values_mut().next().unwrap();e.freshness=Some(Ok(state!="stale"));e.diff=None;e.error=None;e.scroll.set_offset(gpui::point(gpui::px(0.),gpui::px(0.)));if state=="error"{e.error=Some("Provider disconnected. Your previous answer and question are preserved.".into());}if state=="diff"{e.diff=Some(("src/webhooks/handler.ts · new 2–2".into(),cx.new(|cx|TextViewState::markdown(&patch_markdown(&e.result.as_ref().unwrap().snapshot.files[0].patch),cx))));}cx.notify();});fixture.update(cx,|_,cx|cx.notify());window.refresh();}).unwrap();
  cx.background_executor().timer(Duration::from_millis(500)).await;
  let output=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".grove-artifacts/ask-native").join(format!("{name}.png"));std::fs::create_dir_all(output.parent().unwrap()).unwrap();assert!(std::process::Command::new("import").args(["-window","Grove Ask changes QA"]).arg(&output).status().unwrap().success());let mut metadata=std::fs::OpenOptions::new().create(true).append(true).open(output.parent().unwrap().join("captures.txt")).unwrap();writeln!(metadata,"{name}: theme={theme}, rem={rem}, requested width={width}").unwrap();
 }
 cx.update_window(window.into(),|_,window,cx| {fixture.read(cx).source.clone().focus(window,cx);sidebar.update(cx,|s,cx|{s.questions=Questions::default();cx.notify();});window.refresh();}).unwrap();
 cx.background_executor().timer(Duration::from_millis(150)).await;
 cx.update(|cx|cx.quit());}).detach();});
}

#[gpui::test]
fn active_reader_navigation_preserves_question_and_answer_cache(cx: &mut gpui::TestAppContext) {
    let (sidebar, cx) = fixture(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |s, cx| {
            let k = s.questions.open.clone().unwrap();
            let answer = s.questions.entries.get(&k).unwrap().result.clone().unwrap();
            s.active_change_reader = Some("describe");
            assert!(!s.questions_is_open(cx));
            assert_eq!(
                s.questions
                    .entries
                    .get(&k)
                    .unwrap()
                    .result
                    .as_ref()
                    .unwrap()
                    .question,
                answer.question
            );
            assert_eq!(
                s.questions
                    .entries
                    .get(&k)
                    .unwrap()
                    .input
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .value()
                    .as_ref(),
                answer.question
            );
            s.active_change_reader = Some("ask");
            s.toggle_files(window, cx);
            assert!(s.active_change_reader.is_none());
            assert!(s.questions.entries.get(&k).unwrap().result.is_some());
        })
    });
}

#[gpui::test]
fn deferred_question_cannot_submit_after_checkout_switch(cx: &mut gpui::TestAppContext) {
    let (sidebar, cx) = fixture(cx);
    cx.update(|_, cx| {
        sidebar.update(cx, |s, cx| {
            let k = s.questions.open.clone().unwrap();
            s.checkout_panel.chosen_root = Some("/another-checkout".into());
            s.selection = Some(Selection::Worktree(0, "/another-checkout".into()));
            s.submit_question(&k, cx);
            assert!(s.questions.entries.get(&k).unwrap().cancel.is_none());
            assert!(s.questions.entries.get(&k).unwrap().result.is_some());
        })
    });
}

#[test]
fn captured_markdown_context_cannot_close_diff_fence() {
    let patch = "@@ -1,3 +1,3 @@\n ```\n-previous\n+![untrusted](https://example.invalid)\n ```";
    let rendered = patch_markdown(patch);
    assert!(rendered.starts_with("````diff\n"));
    assert!(rendered.ends_with("\n````"));
    assert!(rendered.contains(patch));
}

#[gpui::test]
fn hidden_checkout_ask_opens_without_mounting_checkout_or_starting_provider(
    cx: &mut gpui::TestAppContext,
) {
    let (sidebar, cx) = fixture(cx);
    cx.update(|_, cx| {
        sidebar.update(cx, |s, cx| {
            s.questions = Questions::default();
            s.checkout_panel.hidden = true;
            s.files_active = true;
            let source = cx.focus_handle();
            s.invoke_ask_skill(
                Invocation {
                    scope: ScopePreference::Local,
                    question: String::new(),
                },
                source,
                cx,
            );
            let k = s.questions.open.as_ref().unwrap();
            assert_eq!(k.path, PATH);
            assert!(s.questions.entries.get(k).unwrap().cancel.is_none());
            assert!(!s.files_active);
            assert!(s.checkout_panel.hidden);
            assert_eq!(s.active_change_reader, Some("ask"));
        })
    });
    draw(cx);
    assert!(cx.debug_bounds("ask-question-well").is_some());
}
#[gpui::test]
fn failed_pr_lookup_retry_reads_latest_question_at_activation(cx: &mut gpui::TestAppContext) {
    let (sidebar, cx) = fixture(cx);
    cx.update(|_, cx| {
        sidebar.update(cx, |s, cx| {
            let e = s.questions.entries.values_mut().next().unwrap();
            e.scope = None;
            e.error = Some("Lookup failed".into());
            cx.notify();
        })
    });
    draw(cx);
    let retry = cx.debug_bounds("ask-retry-pr").unwrap();
    cx.update(|window, cx| {
        sidebar.update(cx, |s, cx| {
            let e = s.questions.entries.values_mut().next().unwrap();
            e.input.as_ref().unwrap().update(cx, |input, cx| {
                input.set_value("Use the freshly edited question", window, cx)
            });
        })
    });
    cx.simulate_click(retry.center(), gpui::Modifiers::default());
    sidebar.read_with(cx, |s, cx| {
        let e = s
            .questions
            .entries
            .iter()
            .find(|(k, _)| k.scope == "resolving")
            .unwrap()
            .1;
        let draft = e.pending_question.clone().or_else(|| {
            e.input
                .as_ref()
                .map(|input| input.read(cx).value().to_string())
        });
        assert_eq!(draft.as_deref(), Some("Use the freshly edited question"));
        assert!(e.cancel.is_none());
    });
}

#[gpui::test]
fn question_keyboard_keeps_plain_enter_multiline_and_ctrl_enter_explicit(
    cx: &mut gpui::TestAppContext,
) {
    let (sidebar, cx) = fixture(cx);
    cx.update(|window, cx| {
        sidebar.update(cx, |s, cx| {
            s.questions
                .entries
                .values()
                .next()
                .unwrap()
                .input
                .as_ref()
                .unwrap()
                .update(cx, |input, cx| input.set_value("", window, cx));
        })
    });
    cx.simulate_keystrokes("ctrl-enter");
    draw(cx);
    sidebar.read_with(cx, |s, cx| {
        let e = s.questions.entries.values().next().unwrap();
        assert!(e.cancel.is_none());
        assert!(e
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Enter a question")));
        assert_eq!(e.input.as_ref().unwrap().read(cx).value().as_ref(), "");
    });
    cx.simulate_keystrokes("enter");
    draw(cx);
    sidebar.read_with(cx, |s, cx| {
        assert!(s
            .questions
            .entries
            .values()
            .next()
            .unwrap()
            .input
            .as_ref()
            .unwrap()
            .read(cx)
            .value()
            .contains('\n'))
    });
}
