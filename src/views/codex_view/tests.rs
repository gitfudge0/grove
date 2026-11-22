//! Conversation tests and shared provider fixtures.

mod composer_input;
mod composer_layout;
mod configuration_controls;
mod conversation_layout;
mod dashboard_handoff;
mod draft_recovery;
mod message_rendering;
#[cfg(target_os = "macos")]
mod native_snapshots;
mod plans_and_reports;
mod queue_and_stop;
mod requests;
mod submission;
mod transcript_navigation;
mod work_display;

use super::*;
use grove_core::codex::{Connection, Handle, SavedSession, Snapshot, Status};
use serde_json::json;
use std::sync::mpsc;

fn actions_at_epoch(actions: &mpsc::Receiver<Action>, expected_epoch: u64) -> Vec<Action> {
    actions
        .try_iter()
        .map(|action| match action {
            Action::Interaction { epoch, action } => {
                assert_eq!(
                    epoch, expected_epoch,
                    "interaction captures its rendered epoch"
                );
                *action
            }
            action => action,
        })
        .collect()
}

// Pinned GPUI replays cached hitboxes but not its debug-selector map.
// Request a fresh inspection frame before looking up a cached marker.
fn minimap_bounds(
    cx: &mut gpui::VisualTestContext,
    selector: &'static str,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear();
    });
    cx.debug_bounds(selector)
}

fn test_image(format: image::ImageFormat) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 2)
        .write_to(&mut bytes, format)
        .unwrap();
    bytes.into_inner()
}
pub(super) fn snapshot() -> Snapshot {
    let mut state = Snapshot::new(SavedSession {
        html_policy: grove_core::visuals::HtmlPolicy::Legacy,
        visuals: Vec::new(),
        session_review: grove_core::codex::SessionReview::default(),
        rich_plans: grove_core::rich_plan::PlanStore::default(),
        side_chats: HashMap::default(),
        fork_origin: None,
        claude_resume: None,
        claude_subagents: std::collections::HashMap::default(),
        edit_checkpoints: Vec::new(),
        pending_edit: None,
        rewind_drafts: Vec::new(),
        provider: grove_core::codex::Provider::Codex,
        composer: grove_core::codex::ComposerSettings::default(),
        context: None,
        context_details: None,
        work_periods: Vec::new(),
        key: "native-ui-test".into(),
        project: "Demo".into(),
        cwd: "/unused".into(),
        label: "Codex".into(),
        title: None,
        title_generation: None,
        roots: Vec::new(),
        thread_id: Some("thread-ui".into()),
        items: Vec::new().into(),
        draft: String::new(),
        draft_images: Vec::new(),
        draft_reference: None,
        config_args: Vec::new(),
        native_config_version: 1,
        activity_reporting: false,
    });
    state.status = Status::Idle;
    state
}
/// Use Grove's real unconsumed Tab handler, rather than installing a
/// component keybinding that the production shell intentionally omits.
struct ConversationNavigationFixture(Entity<CodexView>);
impl Render for ConversationNavigationFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .on_key_down(super::super::shell::traverse_unhandled_tab)
            .child(self.0.clone())
    }
}

struct UnmountedDashboardHost;
impl Render for UnmountedDashboardHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child("Dashboard remains visible")
    }
}

fn decision_plan_snapshot(provider: codex::Provider) -> Snapshot {
    let mut state = snapshot();
    state.saved.provider = provider;
    state.saved.draft = "Keep my conversation draft".into();
    state.saved.rich_plans = serde_json::from_value(json!({"revisions":[{
        "message_id":"plan-message", "source":"plan.grove-plan.json", "assets":[],
        "document":{"schema_version":1,"plan_id":"invites","revision_id":"r1","title":"Invite teammates",
            "markdown":"# Invite teammates\n\nAdd workspace invitations with explicit access rules.\n\n## Access\nChoose the policy before implementation.\n\n## Verification\nCheck creation, acceptance and revocation.\n\n| State | Expected result |\n| --- | --- |\n| Valid invitation | Access matches the chosen role |\n| Expired invitation | Joining is blocked and renewal is available |",
            "decisions":[{"id":"access","section":"Access","prompt":"Who can invite?","options":[{"id":"admins","label":"Admins only","description":"Restrict membership changes"}]}]}
    }]})).unwrap();
    state.saved.items = vec![json!({"id":"plan-message","type":"agentMessage","status":"completed","text":"Here is the plan."})].into();
    state
}

fn overview_plan_snapshot(provider: codex::Provider) -> Snapshot {
    let mut state = decision_plan_snapshot(provider);
    state.saved.rich_plans.revisions[0].document.presentation = Some(serde_json::from_value(json!({
        "outcome":"Invite teammates with explicit access, without losing your conversation draft.",
        "flow":[{"title":"Create invitation","detail":"Choose the teammate and role"},{"title":"Accept invitation","detail":"Verify identity and access"},{"title":"Continue working","detail":"Keep the current workspace"}],
        "included":["Invitation creation, acceptance and revocation"],"deferred":["Bulk invitations and directory sync"],
        "risks":["Expired invitations must never grant access."],"success_criteria":["Access matches the chosen policy.","Cancel and recovery preserve your conversation draft."],"revision_changes":[]
    })).unwrap());
    if std::env::var("GROVE_PLAN_OVERVIEW_REVISION").as_deref() == Ok("finalized") {
        state.saved.rich_plans.revisions[0]
            .review
            .answers
            .insert("access".into(), "admins".into());
        let (id, _) = state
            .saved
            .rich_plans
            .prepare_delivery("invites", "r1")
            .unwrap()
            .unwrap();
        state.saved.rich_plans.confirm_delivery(id);
        let mut finalized = state.saved.rich_plans.revisions[0].clone();
        finalized.document.revision_id = "r2".into();
        finalized.document.decisions.clear();
        finalized.document.markdown = Some("## Implementation\nOnly workspace admins can create invitations. Preserve the conversation draft.\n## Verification\nVerify admin authorization, expiration and draft restoration.".into());
        finalized.document.finalized_from = Some(grove_core::rich_plan::PlanFinalization {
            revision_id: "r1".into(),
            answers: HashMap::from([("access".into(), "Admins only".into())]),
        });
        finalized.document.presentation.as_mut().unwrap().revision_changes = vec!["Only workspace admins can invite; authorization and verification now follow that policy.".into()];
        finalized.review = grove_core::rich_plan::ReviewState::default();
        state.saved.rich_plans.revisions.push(finalized);
    }
    state
}

fn behavior_plan_snapshot(provider: codex::Provider) -> Snapshot {
    let mut state = decision_plan_snapshot(provider);
    state.saved.rich_plans.revisions[0].document = serde_json::from_value(json!({
        "schema_version":1,"plan_id":"invites","revision_id":"r1","title":"Remote desktop access in Grove",
        "presentation":{
            "outcome":"Use another desktop to work on this host. Execution stays here.",
            "included":["Native desktop pairing, encrypted transport and revocable control","Host-owned providers, files, Git and terminals"],
            "deferred":["Browser/mobile clients and repository sync","Agent delegation and fine-grained roles"],
            "risks":["A paired device can execute as the host OS user.","Extracting session ownership from GPUI is substantial."],
            "success_criteria":["Verify two-machine control with live Codex and Claude separately.","Verify revocation, reconnect, duplicate commands, stale approvals and native lifecycle behavior."],
            "revision_changes":[]
        },
        "claims":[
            {"id":"pairing","text":"You can control a host from another Grove desktop.",
                "exhibit":{"kind":"layout","nodes":[
                    {"id":"client","kind":"surface","text":"This desktop · Connect to a host"},
                    {"id":"host","kind":"surface","text":"Host desktop · Projects, sessions and terminals"}
                ]},
                "decisions":[{"id":"access","prompt":"How should the desktops connect?","default":"direct","options":[
                    {"id":"direct","label":"Direct connection","description":"Requires a reachable host or an existing private network."},
                    {"id":"relay","label":"Public relay","description":"Works across networks. Adds a hosted service to operate."}
                ]}],
                "children":[{"id":"invitation","text":"One invitation pairs one device.","detail":"The host grants control after identity verification. The host can revoke the paired device."}]
            },
            {"id":"availability","text":"You can use the host’s workspace while access is active.",
                "exhibit":{"kind":"state_machine","initial":"available","states":[
                    {"id":"available","label":"Host available · Remote control enabled"},
                    {"id":"unavailable","label":"Host unavailable · Draft retained, commands paused"}
                ],"transitions":[{"from":"available","to":"unavailable","label":"Host access ends"}]},
                "decisions":[{"id":"lifecycle","prompt":"When should host access run?","default":"open","options":[
                    {"id":"open","label":"While Grove is open","description":"Closing Grove ends remote access."},
                    {"id":"background","label":"In the background","description":"Access continues after close. Adds a background service."}
                ]}]
            },
            {"id":"recovery","text":"Your drafts survive a disconnect; uncertain commands never replay.",
                "detail":"Keep the draft. Confirm the host state after reconnect. Reject stale approvals and duplicate commands."}
        ]
    })).unwrap();
    if std::env::var_os("GROVE_PLAN_BEHAVIOR_REVIEW").is_some() {
        state.saved.rich_plans.revisions[0].review.answers = HashMap::from([
            ("access".into(), "direct".into()),
            ("lifecycle".into(), "open".into()),
        ]);
    }
    state
}

fn plan_request_snapshot(provider: codex::Provider) -> Snapshot {
    let mut state = decision_plan_snapshot(provider);
    state.saved.rich_plans.revisions[0]
        .review
        .own_answers
        .insert("access".into(), "Workspace owners and team leads".into());
    let (id, _) = state
        .saved
        .rich_plans
        .prepare_delivery("invites", "r1")
        .unwrap()
        .unwrap();
    state.saved.rich_plans.confirm_delivery(id);
    let mut finalized = state.saved.rich_plans.revisions[0].clone();
    finalized.document.revision_id = "r2".into();
    finalized.document.decisions.clear();
    finalized.document.finalized_from = Some(grove_core::rich_plan::PlanFinalization {
        revision_id: "r1".into(),
        answers: [("access".into(), "Workspace owners and team leads".into())].into(),
    });
    finalized.review = grove_core::rich_plan::ReviewState::default();
    finalized.review.request_decision_revision = true;
    state.saved.rich_plans.revisions.push(finalized);
    let (_, payload) = state
        .saved
        .rich_plans
        .prepare_delivery("invites", "r2")
        .unwrap()
        .unwrap();
    state.saved.items = vec![json!({"id":"decision-request", "type":"userMessage", "content":[{"type":"inputText", "text":payload}]})].into();
    state
}

#[cfg(target_os = "linux")]
struct PlanCaptureShell(Entity<CodexView>);
#[cfg(target_os = "linux")]
impl Render for PlanCaptureShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(self.0.clone())
            .children(
                std::env::var_os("GROVE_PLAN_BEHAVIOR_CAPTURE")
                    .is_some()
                    .then(|| super::super::native_plan::render_expanded(cx))
                    .flatten(),
            )
    }
}

pub(super) fn fixture(
    cx: &mut gpui::TestAppContext,
) -> (
    Entity<CodexView>,
    &mut gpui::VisualTestContext,
    mpsc::Receiver<Action>,
    mpsc::Sender<Snapshot>,
) {
    fixture_with_navigation(cx, false)
}
fn fixture_with_navigation(
    cx: &mut gpui::TestAppContext,
    shell_navigation: bool,
) -> (
    Entity<CodexView>,
    &mut gpui::VisualTestContext,
    mpsc::Receiver<Action>,
    mpsc::Sender<Snapshot>,
) {
    cx.update(|cx| {
        gpui_component::init(cx);
        if cx.try_global::<crate::settings::SettingsState>().is_none() {
            cx.set_global(crate::settings::SettingsState::new(
                grove_core::storage::Store::default(),
            ));
        }
    });
    let (handle, actions) = Handle::channel();
    let (updates, receive) = mpsc::channel();
    let connection = Connection {
        handle,
        updates: receive.into(),
    };
    let mut view = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let session = cx.new(|cx| CodexSession::from_connection(snapshot(), connection, cx));
        let chat = cx.new(|cx| CodexView::new(session, window, cx));
        chat.update(cx, |view, _| {
            view.next_reply_generate = |_, _| Ok(Vec::new());
        });
        view = Some(chat.clone());
        if shell_navigation {
            let shell = cx.new(|_| ConversationNavigationFixture(chat));
            gpui_component::Root::new(shell, window, cx).bordered(false)
        } else {
            gpui_component::Root::new(chat, window, cx).bordered(false)
        }
    });
    (view.unwrap(), cx, actions, updates)
}

include!("performance.rs");
include!("native_hit_geometry.rs");
include!("live_performance.rs");

pub(super) fn draw(cx: &mut gpui::VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

fn task_list(statuses: &[TaskStatus]) -> TaskList {
    TaskList {
        tasks: statuses
            .iter()
            .enumerate()
            .map(|(index, status)| grove_core::tasks::Task {
                id: index.to_string(),
                step: format!("Step {index}"),
                active_form: (index == 1).then(|| "Doing step 1".to_owned()),
                status: *status,
                started_at: index,
                identity_at: index,
            })
            .collect(),
        explanation: None,
        item: 0,
    }
}

fn seed_next_reply(
    view: &mut CodexView,
    provider: codex::Provider,
    draft: &str,
    proposal: &str,
    window: &mut Window,
    cx: &mut Context<CodexView>,
) {
    view.dismiss_next_reply(cx);
    view.session.update(cx, |session, _| {
        session.state.saved.provider = provider;
        session.state.status = Status::Done;
        session.state.saved.items = vec![
            json!({"id":"reply-user", "type":"userMessage", "content":[{"type":"inputText", "text":"Fix draft preservation."}]}),
            json!({"id":"reply-agent", "type":"agentMessage", "text":"The fix is ready. I can add regression coverage for draft preservation."}),
        ].into();
    });
    view.input.update(cx, |input, cx| {
        input.set_value(draft, window, cx);
        input.set_selected_range(draft.len()..draft.len(), cx);
    });
    view.input.focus_handle(cx).focus(window, cx);
    let request = grove_core::next_reply::Request::capture(&view.session.read(cx).state).unwrap();
    view.next_reply_seen = Some(request.key.clone());
    view.next_reply_request = Some(request.key);
    view.next_reply = grove_core::next_reply::Candidate::pending();
    view.next_reply.resolve(vec![proposal.into()]);
}

fn wrapped_question_receipt() -> QuestionReceipt {
    QuestionReceipt {
        anchor: 0,
        echo_text: None,
        echo_after: 0,
        title: None,
        answers: vec![
            ("Which web (care-convoy-ai-web) SHA is in prod now, to use as the CC_AI_R5_2 baseline? There's no CC_AI_R5_2 branch in careconvoy-ai-core, so I'm treating this as a web-only release. CC_AI_R5_1 tip 909b3b6 is an ancestor of R5_2, and R5_2 adds just one commit on top of it (d213549, AVA-209 Bun pin).".into(), "909b3b6 (CC_AI_R5_1 tip)".into()),
            ("Is careconvoy-ai-core really out of scope for this release? It has no CC_AI_R5_2 branch.".into(), "Web only (Recommended)".into()),
        ],
    }
}

fn question_review_snapshot(provider: codex::Provider) -> codex::Snapshot {
    let mut state = snapshot();
    state.saved.provider = provider;
    state.status = Status::Waiting;
    state.saved.draft = "Keep my conversation draft".into();
    state.saved.items.push(json!({"type":"agentMessage","id":"release-question","text":"Choose how the release should work before continuing."}));
    let mut request = codex::QuestionRequest::from_codex(json!("review-release"), &json!({"questions":[
        {"id":"audience","question":"Who receives the first release?","header":"Single choice","isOther":true,"options":[{"label":"Pilot workspaces","description":"Validate with a smaller audience first.","preview":"Internal workspaces → observe → decide whether to expand."},{"label":"All workspaces","description":"Make the release available to everyone immediately."}]},
        {"id":"checks","question":"Which checks gate the release?","header":"Multiple choice","options":[{"label":"Unit tests"},{"label":"Native UI inspection"}]},
        {"id":"criteria","question":"What would make this release successful?","header":"Free text"},
        {"id":"credential","question":"Provide a test credential","header":"Secret answer","isSecret":true},
        {"id":"duration","question":"How long should the pilot run?","header":"Number"}
    ]})).unwrap();
    request.title = Some("Release decisions".into());
    request.questions[1].multiple = true;
    request.questions[4].kind = codex::QuestionKind::Number(codex::QuestionNumber {
        min: 1.,
        max: 14.,
        step: Some(1.),
        default_value: Some(7.),
        unit: Some("days".into()),
    });
    state.questions.push(request);
    state
}

fn inline_html_toolbar_fixture(cx: &mut gpui::TestAppContext, provider: codex::Provider) {
    let (_, cx, _, updates) = fixture(cx);
    let mut response = snapshot();
    response.saved.provider = provider;
    response.saved.items.push(serde_json::json!({
        "type":"agentMessage", "id":"inline-report",
        "text":"```html\n<!doctype html><html><body><!-- grove:integrated-report -->Small mock</body></html>\n```"
    }));
    updates.send(response).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    assert!(cx.debug_bounds("html-artifact").is_some());
    assert!(cx.debug_bounds("html-document-toolbar").is_none());
}

fn selection_reply_pointer_fixture(cx: &mut gpui::TestAppContext, provider: codex::Provider) {
    let (view, cx, actions, updates) = fixture_with_navigation(cx, true);
    let mut response = snapshot();
    response.saved.provider = provider;
    response.saved.items.push(serde_json::json!({
        "type":"agentMessage", "id":"selection-source", "text":"literal selected passage for reply"
    }));
    updates.send(response).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    draw(cx);
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.input.update(cx, |input, cx| {
                input.set_value("my existing draft", window, cx);
            });
        });
    });
    draw(cx);
    let body = cx.debug_bounds("codex-message-body-0").unwrap();
    let start = body.origin + gpui::point(gpui::px(1.), gpui::px(8.));
    let end = body.origin + gpui::point(gpui::px(110.), gpui::px(8.));
    cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_move(
        end,
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::default(),
    );
    draw(cx);
    assert!(
        cx.debug_bounds("codex-selection-reply").is_none(),
        "hidden during drag"
    );
    cx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::default());
    draw(cx);
    let selected = cx.update(gpui_component::WindowExt::selected_text);
    assert!(!selected.is_empty());
    let reply = cx
        .debug_bounds("codex-selection-reply")
        .expect("release reveals action");
    if provider == codex::Provider::Claude {
        let mut focused = false;
        for _ in 0..20 {
            cx.simulate_keystrokes("tab");
            draw(cx);
            focused =
                cx.update(|window, cx| view.read(cx).selection_reply_focus.is_focused(window));
            if focused {
                break;
            }
        }
        assert!(focused, "Reply action must be reachable by Tab");
        cx.simulate_keystrokes("enter");
    } else {
        cx.simulate_click(reply.center(), gpui::Modifiers::default());
    }
    draw(cx);
    assert!(cx.debug_bounds("codex-reference-preview").is_some());
    let reference = view.read_with(cx, |view, cx| {
        assert_eq!(view.input.read(cx).value().as_ref(), "my existing draft");
        let quote = view.reference.as_ref().unwrap();
        assert_eq!(quote.text, selected);
        assert_eq!(quote.sources[0].message_id, "selection-source");
        quote.clone()
    });
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.submit_mode(SubmissionMode::Send, window, cx);
        });
    });
    let dispatched: Vec<_> = actions.try_iter().collect();
    let submitted_at = dispatched
        .iter()
        .position(|action| matches!(action, Action::Submit { .. }))
        .unwrap();
    assert!(
        submitted_at > 0
            && matches!(&dispatched[submitted_at - 1], Action::DraftReference(Some(current)) if current == &reference),
        "current reference must be synchronized immediately before dispatch"
    );
    let outgoing = dispatched.into_iter().find_map(|action| match action {
        Action::Submit { text, .. } => Some(text),
        _ => None,
    });
    assert_eq!(
        outgoing,
        Some(reference.serialize("my existing draft")),
        "{provider:?}"
    );
}

fn approval_receipt_snapshot(provider: codex::Provider) -> Snapshot {
    let mut state = snapshot();
    state.saved.provider = provider;
    state.saved.rich_plans = serde_json::from_value(json!({"revisions":[{
        "message_id":"plan-message", "source":"plan.grove-plan.json", "assets":[],
        "document":{"schema_version":1,"plan_id":"receipt-plan","revision_id":"reviewed-r1",
            "title":"Deterministic Grove Reference Translation with a longer approved plan title",
            "claims":[{"id":"outcome","text":"Preserve the exact approved document and decision answers."}]}
    }]})).unwrap();
    let (id, payload) = state
        .saved
        .rich_plans
        .prepare_approval("receipt-plan", "reviewed-r1")
        .unwrap()
        .unwrap();
    state.saved.rich_plans.confirm_delivery(id);
    state.saved.items = vec![
        json!({"id":"approval-message","type":"userMessage","content":[{"type":"text","text":payload}]}),
    ].into();
    // Exercise the persisted journal rather than relying on ephemeral UI state.
    state.saved = serde_json::from_slice(&serde_json::to_vec(&state.saved).unwrap()).unwrap();
    state
}
